//! Source gate: `src/**/*.rs` (outside `#[cfg(test)]` modules) must issue no
//! tmux write verb — full name or abbreviation — in any `Command::new("tmux")`
//! argument list.
//!
//! M27 turns tmux into an optional observer: the daemon and the client only
//! ever read from it. Every mutation goes through the user's own shell, a
//! saved script, or the daemon's explicit spawn path, so a write verb literal
//! appearing in an ordinary tmux arg list means someone re-coupled the client
//! to tmux. Literals *outside* a tmux arg list (help text, a comment, a
//! `println!` of tmux.conf lines for the user) are text, not calls, and do
//! not count.

use std::collections::BTreeSet;
use std::path::Path;

use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{Attribute, Item, Meta, Token};

/// Write verbs, full name and tmux abbreviation alike, that a
/// `Command::new("tmux")` arg list must never name. Read verbs (`list-panes`,
/// `capture-pane`, `display-message`) are fine.
const WRITE_VERBS: &[&str] = &[
    "send-keys",
    "send",
    "new-window",
    "neww",
    "new-session",
    "new",
    "kill-window",
    "killw",
    "kill-session",
    "set-hook",
    "pipe-pane",
    "pipep",
    "respawn-pane",
    "respawnp",
    "split-window",
    "splitw",
    "resize-pane",
    "resizep",
    "select-pane",
    "selectp",
    "set-option",
    "set",
    "set-window-option",
    "setw",
];

/// From the first `.args([…])` / `.arg(…)` call within 15 lines after a
/// `Command::new("tmux")` line to the bracket that balances it. The close may
/// sit on a later line; when the call closes on the opener's own line, the
/// span is that line alone. Lines before the opener are outside the arg list
/// and are not scanned.
fn tmux_write_hits(src: &str) -> Vec<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let mut hits = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if !line.contains("Command::new(\"tmux\")") {
            continue;
        }
        let end = (i + 15).min(lines.len());
        let Some((n, at, kind)) = find_opener(&lines[i..end]) else {
            continue;
        };
        let mut span = String::new();
        // The kind and remaining depth of the opener's open bracket, when it
        // closes on a later line. `None` once the call is closed.
        let mut open: Option<(char, i32)> = opener(lines[i + n], at, kind);
        for (m, next) in lines[i + n..end].iter().enumerate() {
            span.push_str(next);
            span.push('\n');
            if m == 0 {
                if open.is_none() {
                    break;
                }
                continue;
            }
            if let Some((kind, depth)) = open.as_mut() {
                for c in next.chars() {
                    if c == *kind {
                        *depth += 1;
                    } else if c == close_for(*kind) {
                        *depth -= 1;
                    }
                }
                if *depth <= 0 {
                    break;
                }
            } else {
                break;
            }
        }
        for literal in string_literals(&span) {
            if literal == "tmux" {
                continue;
            }
            if let Some(verb) = WRITE_VERBS.iter().find(|v| **v == literal) {
                hits.push((i + 1, verb.to_string()));
            }
        }
    }
    hits
}

/// The first `.args([` or `.arg(` in the window after `Command::new("tmux")`,
/// as (line offset, char offset, open bracket). rustfmt chains the builder
/// call onto its own line, so the opener may sit below the constructor.
fn find_opener(window: &[&str]) -> Option<(usize, usize, char)> {
    for (n, line) in window.iter().enumerate() {
        if let Some(at) = line.find(".args([") {
            return Some((n, at, '['));
        }
        if let Some(at) = line.find(".arg(") {
            return Some((n, at, '('));
        }
    }
    None
}

/// The open bracket that carries past the opener line, with its remaining
/// depth: `None` when the call already closed on that line.
fn opener(line: &str, from: usize, kind: char) -> Option<(char, i32)> {
    let mut depth: i32 = 0;
    for c in line[from..].chars() {
        if c == kind {
            depth += 1;
        } else if c == close_for(kind) {
            depth -= 1;
        }
    }
    if depth > 0 { Some((kind, depth)) } else { None }
}

fn close_for(kind: char) -> char {
    if kind == '[' { ']' } else { ')' }
}

/// Every double-quoted literal in `src`, in order. Naive but sufficient for
/// arg lists; a literal may not span lines here.
fn string_literals(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(open) = rest.find('"') {
        let after = &rest[open + 1..];
        match after.find('"') {
            Some(close) => {
                out.push(after[..close].to_string());
                rest = &after[close + 1..];
            }
            None => break,
        }
    }
    out
}

/// A file `src/session_store.rs` pulls in with `#[path = …]` behind
/// `#[cfg(test)]`: it is test code even though it carries no marker of its own.
const TEST_CODE_FILES: &[&str] = &["session_store_tests.rs"];

fn is_test_code(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| TEST_CODE_FILES.contains(&name.to_string_lossy().as_ref()))
}

/// What the hygiene gate's `is_test_only` rule says about one `cfg`
/// predicate: `test`, or an `all(..)` / `any(..)` holding one. `not(test)`
/// and everything else is production.
fn meta_is_test(m: &Meta) -> bool {
    match m {
        Meta::Path(p) => p.is_ident("test"),
        Meta::List(l) if l.path.is_ident("all") || l.path.is_ident("any") => l
            .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
            .map(|ms| ms.iter().any(meta_is_test))
            .unwrap_or(false),
        _ => false,
    }
}

/// An item is test-only when it carries `#[test]`, `#[…::test]`, or a
/// `#[cfg(..)]` whose predicate is test-only. The same rule
/// `tests/hygiene.rs` uses.
fn is_test_only(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|a| {
        let p = a.path();
        if p.segments.last().is_some_and(|s| s.ident == "test") {
            return true;
        }
        p.is_ident("cfg") && a.parse_args::<Meta>().is_ok_and(|m| meta_is_test(&m))
    })
}

/// The outer attributes of any item kind.
fn item_attrs(item: &Item) -> &[Attribute] {
    match item {
        Item::Const(i) => &i.attrs,
        Item::Enum(i) => &i.attrs,
        Item::ExternCrate(i) => &i.attrs,
        Item::Fn(i) => &i.attrs,
        Item::ForeignMod(i) => &i.attrs,
        Item::Impl(i) => &i.attrs,
        Item::Macro(i) => &i.attrs,
        Item::Mod(i) => &i.attrs,
        Item::Static(i) => &i.attrs,
        Item::Struct(i) => &i.attrs,
        Item::Trait(i) => &i.attrs,
        Item::TraitAlias(i) => &i.attrs,
        Item::Type(i) => &i.attrs,
        Item::Union(i) => &i.attrs,
        Item::Use(i) => &i.attrs,
        _ => &[],
    }
}

/// The nested items of a module, impl or trait, for recursion.
fn nested_items(item: &Item) -> Vec<&Item> {
    let mut out: Vec<&Item> = Vec::new();
    match item {
        Item::Mod(m) => {
            if let Some((_, items)) = &m.content {
                out.extend(items.iter());
            }
        }
        Item::Impl(i) => {
            for it in &i.items {
                if let syn::ImplItem::Fn(f) = it {
                    out.extend(f.block.stmts.iter().filter_map(stmt_as_item));
                }
            }
        }
        Item::Trait(t) => {
            for it in &t.items {
                if let syn::TraitItem::Fn(f) = it
                    && let Some(b) = &f.default
                {
                    out.extend(b.stmts.iter().filter_map(stmt_as_item));
                }
            }
        }
        _ => {}
    }
    out
}

/// A statement as a general item, when it is an item statement (a nested
/// fn or module).
fn stmt_as_item(s: &syn::Stmt) -> Option<&Item> {
    match s {
        syn::Stmt::Item(i) => Some(i),
        _ => None,
    }
}

/// The line spans of every test-only item at any depth, collected in-place
/// into `out`. An item's span covers its outer attributes.
fn collect_test_lines(items: &[&Item], out: &mut BTreeSet<usize>) {
    for item in items {
        if is_test_only(item_attrs(item)) {
            let span = item.span();
            for line in span.start().line..=span.end().line {
                out.insert(line);
            }
            continue;
        }
        if let Item::Fn(f) = item {
            collect_test_lines(&block_items(&f.block), out);
        }
        collect_test_lines(&nested_items(item), out);
    }
}

/// The item statements of a block, for recursion into a production fn's
/// body (a nested `#[cfg(test)] mod` or `#[test] fn`).
fn block_items(block: &syn::Block) -> Vec<&Item> {
    block
        .stmts
        .iter()
        .filter_map(|s| match s {
            syn::Stmt::Item(i) => Some(i),
            _ => None,
        })
        .collect()
}

/// The production lines of one source text: every line outside the line
/// span of a test-only item, at any depth. A file that fails to parse is a
/// gate failure naming the file, never a skip.
fn production_lines(path: &Path, src: &str) -> Result<Vec<(usize, String)>, String> {
    let file = syn::parse_file(src)
        .map_err(|e| format!("{}: the gate could not parse it: {e}", path.display()))?;
    let mut test_lines = BTreeSet::new();
    collect_test_lines(&file.items.iter().collect::<Vec<_>>(), &mut test_lines);
    Ok(src
        .lines()
        .enumerate()
        .filter(|(i, _)| !test_lines.contains(&(i + 1)))
        .map(|(i, l)| (i + 1, l.to_string()))
        .collect())
}

/// The tmux write hits in one file's production lines. A `*_tests.rs` /
/// `tests.rs` file or a `TEST_CODE_FILES` entry is test code throughout and
/// has none.
fn production_tmux_write_hits(path: &Path, src: &str) -> Result<Vec<(usize, String)>, String> {
    if is_test_code(path) {
        return Ok(Vec::new());
    }
    let name = path.file_name().map(|n| n.to_string_lossy().to_string());
    if name
        .as_deref()
        .is_some_and(|n| n == "tests.rs" || n.ends_with("_tests.rs"))
    {
        return Ok(Vec::new());
    }
    let lines = production_lines(path, src)?;
    let text = lines
        .iter()
        .map(|(_, l)| l.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    Ok(tmux_write_hits(&text)
        .into_iter()
        .map(|(offset, verb)| {
            let line = lines.get(offset - 1).map(|(n, _)| *n).unwrap_or(offset);
            (line, verb)
        })
        .collect())
}

#[test]
fn gate_scans_code_below_a_cfg_test_item() {
    let src = [
        "#[cfg(test)] mod t {}",
        "fn producer() {",
        "    Command::new(\"tmux\").args([\"send-keys\", \"-t\", \"x\"]);",
        "}",
    ]
    .join("\n");
    let hits = production_tmux_write_hits(Path::new("src/probe.rs"), &src)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        hits,
        vec![(3, "send-keys".to_string())],
        "the write below the cfg-test item is scanned: {hits:?}"
    );
}

#[test]
fn gate_skips_test_only_items() {
    let src = [
        "#[test]",
        "fn a_test() {",
        "    Command::new(\"tmux\").args([\"send-keys\", \"-t\", \"x\"]);",
        "}",
    ]
    .join("\n");
    let hits = production_tmux_write_hits(Path::new("src/probe.rs"), &src)
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        hits.is_empty(),
        "a write inside a #[test] fn is not scanned: {hits:?}"
    );
}

#[test]
fn a_file_that_fails_to_parse_fails_the_gate() {
    let result = production_tmux_write_hits(Path::new("src/broken.rs"), "fn f( {");
    assert!(
        result.is_err(),
        "an unparsable file must fail the gate, not skip it"
    );
    assert!(
        result.unwrap_err().contains("src/broken.rs"),
        "the failure names the file"
    );
}

#[test]
fn source_issues_no_tmux_write_verbs() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut failures = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let mut entries: Vec<_> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("could not read {}: {e}", dir.display()))
            .map(|e| e.expect("readdir entry"))
            .collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") && !is_test_code(&path) {
                let text = match std::fs::read_to_string(&path) {
                    Ok(t) => t,
                    Err(e) => panic!("could not read {}: {e}", path.display()),
                };
                // Test code below this line in the same file is out of the
                // gate's scope: the gate covers what reaches tmux in
                // production.
                match production_tmux_write_hits(&path, &text) {
                    Ok(hits) => {
                        for (line, verb) in hits {
                            failures.push(format!(
                                "{}:{line}: tmux write verb `{verb}`",
                                path.display(),
                            ));
                        }
                    }
                    Err(e) => failures.push(e),
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "source outside #[cfg(test)] still issues tmux writes:\n{}",
        failures.join("\n")
    );
}

#[test]
fn gate_catches_an_abbreviated_write() {
    let hits = tmux_write_hits(r#"let _ = Command::new("tmux").args(["splitw", "-h"]);"#);
    assert_eq!(
        hits.len(),
        1,
        "the abbreviated write verb is flagged: {hits:?}"
    );
    assert_eq!(hits[0].1, "splitw");

    // Help text that names a full verb for the user's tmux.conf is not a
    // tmux call.
    let help = tmux_write_hits(r#"println!("bind-key T split-window -h");"#);
    assert!(
        help.is_empty(),
        "a printed help line is not a tmux call: {help:?}"
    );
}

#[test]
fn gate_catches_a_multi_line_write() {
    let src = "let _ = Command::new(\"tmux\").args([\n    \"split-window\",\n    \"-h\",\n]);";
    let hits = tmux_write_hits(src);
    assert_eq!(hits.len(), 1, "the multi-line write is flagged: {hits:?}");
    assert_eq!(hits[0].1, "split-window");

    // A call that closes on its own line is still one hit, not two.
    let one_line = tmux_write_hits("let _ = Command::new(\"tmux\").arg(\"killw\");");
    assert_eq!(
        one_line.len(),
        1,
        "the single-line write is still flagged once: {one_line:?}"
    );
}

#[test]
fn gate_catches_a_chained_write() {
    let src = "let _ = std::process::Command::new(\"tmux\")\n    .args([\"send-keys\", \"-t\", \"x\"])\n    .output();";
    let hits = tmux_write_hits(src);
    assert_eq!(hits.len(), 1, "the chained write is flagged: {hits:?}");
    assert_eq!(hits[0].1, "send-keys");
}

//! Source gate: outside test code, no mutex lock site may `unwrap()` or
//! `expect()` its guard. CLAUDE.md's invariant is that every lock site uses
//! `.unwrap_or_log()` (`crate::util::UnpoisonExt`), which logs an ERROR and
//! recovers from a poisoned lock instead of panicking.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{Attribute, Item, Meta, Token};

/// A guard call with empty parens followed by `.unwrap()` or `.expect(`,
/// on one line. The split form is handled in `lock_unwrap_hits`.
static GUARD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\.(lock|read|write|try_lock|try_read|try_write)\(\)\s*\.\s*(unwrap\(\)|expect\()")
        .expect("valid regex")
});

/// A line that ends with a bare guard call, whose `.unwrap()` / `.expect(`
/// continues on the next line.
static ENDS_WITH_GUARD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\.(lock|read|write|try_lock|try_read|try_write)\(\)$").expect("valid regex")
});

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

/// Production lines of one file: the parse-based `production_lines` for a
/// text that parses, or a gate failure naming the file. A `*_tests.rs` or
/// `tests.rs` file is test code throughout and has none.
fn file_production_lines(path: &Path, text: &str) -> Vec<(usize, String)> {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string());
    if name
        .as_deref()
        .is_some_and(|n| n == "tests.rs" || n.ends_with("_tests.rs"))
    {
        return Vec::new();
    }
    match production_lines(path, text) {
        Ok(lines) => lines,
        Err(e) => panic!("{e}"),
    }
}

/// The lines that unwrap or expect a lock guard, including the split form
/// where `.lock()` ends one line and `.unwrap()` or `.expect(` starts the next.
fn lock_unwrap_hits(lines: &[(usize, String)]) -> Vec<usize> {
    let mut hits = Vec::new();
    for (i, (n, line)) in lines.iter().enumerate() {
        let doc = line.trim_start().starts_with("///") || line.trim_start().starts_with("//!");
        if doc {
            continue;
        }
        if GUARD_RE.is_match(line) {
            hits.push(*n);
        } else if ENDS_WITH_GUARD.is_match(line.trim_end())
            && let Some((_, next)) = lines.get(i + 1)
            && (next.trim_start().starts_with(".unwrap()")
                || next.trim_start().starts_with(".expect("))
        {
            hits.push(*n);
        }
    }
    hits
}

#[test]
fn production_lock_sites_never_unwrap_or_expect() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut failures = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read src dir") {
            let path = entry.expect("readdir entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).expect("read source file");
                for n in lock_unwrap_hits(&file_production_lines(&path, &text)) {
                    let rel = path
                        .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                        .unwrap_or(&path);
                    failures.push(format!("{}:{n}", rel.display()));
                }
            }
        }
    }
    failures.sort();
    assert!(
        failures.is_empty(),
        "lock sites that unwrap or expect instead of `.unwrap_or_log()`:\n{}",
        failures.join("\n")
    );
}

#[test]
fn the_gate_scans_code_below_a_cfg_test_item() {
    let src = "#[cfg(test)] mod t {}\nfn f() { let g = m.lock().unwrap(); }";
    let lines = production_lines(Path::new("src/probe.rs"), src).unwrap_or_default();
    assert_eq!(
        lock_unwrap_hits(&lines),
        vec![2],
        "the lock site below the cfg-test item is scanned: {lines:?}"
    );
}

#[test]
fn a_file_that_fails_to_parse_fails_the_gate() {
    let result = production_lines(Path::new("src/broken.rs"), "fn f( {");
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
fn the_gate_sees_rwlock_and_try_forms() {
    let src = [
        "fn a() { let g = m.read().unwrap(); }",
        "fn b() { let g = m.write().expect(\"x\"); }",
        "fn c() { let g = m.try_lock().unwrap(); }",
        "fn d() { let g = m.try_read().unwrap(); }",
        "fn e() { let g = m.try_write().expect(\"y\"); }",
        "fn f() {",
        "    let g = m",
        "        .read()",
        "        .unwrap();",
        "}",
    ]
    .join("\n");
    let lines = production_lines(Path::new("src/probe.rs"), &src).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        lock_unwrap_hits(&lines),
        vec![1, 2, 3, 4, 5, 8],
        "every guard form with empty parens and unwrap/expect is a hit: {lines:?}"
    );
}

#[test]
fn the_gate_skips_test_only_items_and_non_guards() {
    let src = [
        "#[test]",
        "fn t1() { let g = m.lock().unwrap(); }",
        "",
        "#[tokio::test]",
        "async fn t2() { let g = m.lock().unwrap(); }",
        "",
        "#[cfg(all(test, unix))]",
        "mod x { fn t3() { let g = m.lock().unwrap(); } }",
        "",
        "fn prod1() { let g = m.lock().unwrap_or_log(); }",
        "fn prod2() { file.read(&mut buf).unwrap(); }",
    ]
    .join("\n");
    let lines = production_lines(Path::new("src/probe.rs"), &src).unwrap_or_default();
    assert_eq!(
        lock_unwrap_hits(&lines),
        Vec::<usize>::new(),
        "test-only items and non-guard sites produce no hits: {lines:?}"
    );
}

#[test]
fn the_gate_sees_both_forms_and_ignores_comments() {
    let lines: Vec<(usize, String)> = [
        "let a = m.lock().unwrap();",
        "let b = m.lock().expect(\"x\");",
        "let c = m",
        "    .lock()",
        "    .expect(\"y\");",
        "/// `.lock().unwrap()` in a doc comment",
        "let d = m.lock().unwrap_or_log();",
    ]
    .iter()
    .enumerate()
    .filter(|(_, l)| !l.trim_start().starts_with("//"))
    .map(|(i, l)| (i + 1, l.to_string()))
    .collect();
    assert_eq!(lock_unwrap_hits(&lines), vec![1, 2, 4]);
}

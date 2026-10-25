//! The M31 hygiene gate: no source file or function grows past its cap.
//!
//! Every `.rs` file under `src/` and `tests/` is parsed with `syn` and
//! measured:
//!
//! - **production lines** of a non-test file: its line count minus the lines
//!   spanned by test-only items (`#[test]`, `#[tokio::test]`, a `cfg` whose
//!   predicate enables only under `test`). Capped at [`FILE_CAP`].
//! - **test lines**: all of a test file (under `tests/`, `*_tests.rs`,
//!   `tests.rs`), or the inline test-only items of any other file. Capped
//!   at [`TEST_FILE_CAP`].
//! - **function length** of every production function, from its signature to
//!   its closing brace. Capped at [`FN_CAP`].
//!
//! Today's offenders are listed in `tests/hygiene_allowlist.txt`. An entry
//! may never grow, and an entry whose subject now fits fails with "remove
//! it", so the list only shrinks. See
//! `docs/dev/milestones/M31-code-hygiene/README.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::visit::Visit;
use syn::{Attribute, Item, Meta, Token};

/// Production lines per non-test file.
const FILE_CAP: usize = 800;
/// Total lines per test file.
const TEST_FILE_CAP: usize = 2000;
/// Lines per production function, signature to closing brace.
const FN_CAP: usize = 150;

/// Files exempt from the file cap (their functions are still measured).
const EXEMPT: &[(&str, &str)] = &[
    (
        "src/ai/types/pending.rs",
        "one enum with a variant per AI tool, and its impl: flat data",
    ),
    (
        "src/ai/tools/args.rs",
        "one argument struct per AI tool: flat data",
    ),
];

const ALLOWLIST: &str = "tests/hygiene_allowlist.txt";

// ---------------------------------------------------------------------------
// Measuring one source text
// ---------------------------------------------------------------------------

/// What the measurer reports for one source text.
#[derive(Debug, Default)]
struct Measure {
    /// Every line in the text.
    total_lines: usize,
    /// Lines not spanned by a test-only item.
    production_lines: usize,
    /// Production functions as `(name, lines)`, duplicate names suffixed
    /// `#2`, `#3` in source order.
    functions: Vec<(String, usize)>,
}

/// A `cfg` predicate that enables its item only under `test`: `test`, or an
/// `all(..)` / `any(..)` containing one. `not(test)` and everything else is
/// production.
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
/// `#[cfg(..)]` whose predicate is test-only.
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

/// The last path segment of a type, e.g. `Foo` for `crate::a::Foo<T>`.
fn type_name(ty: &syn::Type) -> String {
    match ty {
        syn::Type::Path(tp) => tp
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_else(|| "?".to_string()),
        syn::Type::Reference(r) => type_name(&r.elem),
        _ => "?".to_string(),
    }
}

/// Lines from a signature's first line to a block's closing brace.
fn fn_lines(sig: &syn::Signature, block: &syn::Block) -> usize {
    let start = sig.span().start().line;
    let end = block.brace_token.span.close().start().line;
    end.saturating_sub(start) + 1
}

#[derive(Default)]
struct Collector {
    /// Inclusive 1-based line ranges spanned by test-only items.
    test_ranges: Vec<(usize, usize)>,
    /// The type or trait whose items are being visited.
    owner: Option<String>,
    functions: Vec<(String, usize)>,
}

impl Collector {
    fn skip(&mut self, span: proc_macro2::Span) {
        self.test_ranges.push((span.start().line, span.end().line));
    }

    fn named(&self, ident: &syn::Ident) -> String {
        match &self.owner {
            Some(o) => format!("{o}::{ident}"),
            None => ident.to_string(),
        }
    }
}

impl<'ast> Visit<'ast> for Collector {
    fn visit_item(&mut self, i: &'ast Item) {
        if is_test_only(item_attrs(i)) {
            self.skip(i.span());
            return;
        }
        syn::visit::visit_item(self, i);
    }

    fn visit_item_impl(&mut self, i: &'ast syn::ItemImpl) {
        let saved = self.owner.replace(type_name(&i.self_ty));
        syn::visit::visit_item_impl(self, i);
        self.owner = saved;
    }

    fn visit_item_trait(&mut self, i: &'ast syn::ItemTrait) {
        let saved = self.owner.replace(i.ident.to_string());
        syn::visit::visit_item_trait(self, i);
        self.owner = saved;
    }

    fn visit_item_fn(&mut self, i: &'ast syn::ItemFn) {
        // A free fn nested in a method body is still a free fn.
        let saved = self.owner.take();
        self.functions
            .push((i.sig.ident.to_string(), fn_lines(&i.sig, &i.block)));
        syn::visit::visit_item_fn(self, i);
        self.owner = saved;
    }

    fn visit_impl_item_fn(&mut self, i: &'ast syn::ImplItemFn) {
        if is_test_only(&i.attrs) {
            self.skip(i.span());
            return;
        }
        self.functions
            .push((self.named(&i.sig.ident), fn_lines(&i.sig, &i.block)));
        syn::visit::visit_impl_item_fn(self, i);
    }

    fn visit_trait_item_fn(&mut self, i: &'ast syn::TraitItemFn) {
        if is_test_only(&i.attrs) {
            self.skip(i.span());
            return;
        }
        if let Some(block) = &i.default {
            self.functions
                .push((self.named(&i.sig.ident), fn_lines(&i.sig, block)));
        }
        syn::visit::visit_trait_item_fn(self, i);
    }
}

/// Measure one source text.
fn measure(src: &str) -> syn::Result<Measure> {
    let file = syn::parse_file(src)?;
    let mut c = Collector::default();
    c.visit_file(&file);

    let total_lines = src.lines().count();
    let test_lines: BTreeSet<usize> = c
        .test_ranges
        .iter()
        .flat_map(|&(a, b)| a..=b)
        .filter(|&l| l >= 1 && l <= total_lines)
        .collect();

    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let functions = c
        .functions
        .into_iter()
        .map(|(name, lines)| {
            let n = seen.entry(name.clone()).or_insert(0);
            *n += 1;
            let name = if *n == 1 { name } else { format!("{name}#{n}") };
            (name, lines)
        })
        .collect();

    Ok(Measure {
        total_lines,
        production_lines: total_lines - test_lines.len(),
        functions,
    })
}

// ---------------------------------------------------------------------------
// The tree
// ---------------------------------------------------------------------------

/// A file is a test file when it lives under `tests/`, or is named
/// `*_tests.rs` or `tests.rs`.
fn is_test_file(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    path.starts_with("tests/") || name.ends_with("_tests.rs") || name == "tests.rs"
}

fn is_exempt(path: &str) -> bool {
    EXEMPT.iter().any(|(p, _)| *p == path)
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Every source file, measured, keyed by its repo-relative `/` path. Parsed
/// once per test binary.
fn tree() -> &'static BTreeMap<String, Measure> {
    static TREE: OnceLock<BTreeMap<String, Measure>> = OnceLock::new();
    TREE.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        rust_files(&root.join("src"), &mut files);
        rust_files(&root.join("tests"), &mut files);
        let mut out = BTreeMap::new();
        let mut unparsed = Vec::new();
        for path in files {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let src = std::fs::read_to_string(&path).unwrap_or_default();
            match measure(&src) {
                Ok(m) => {
                    out.insert(rel, m);
                }
                Err(e) => unparsed.push(format!("{rel}: {e}")),
            }
        }
        assert!(
            unparsed.is_empty(),
            "files the hygiene gate could not parse:\n{}",
            unparsed.join("\n")
        );
        out
    })
}

// ---------------------------------------------------------------------------
// The allowlist
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct Allowlist {
    files: BTreeMap<String, usize>,
    tests: BTreeMap<String, usize>,
    functions: BTreeMap<(String, String), usize>,
    /// Lines that are not a valid entry.
    malformed: Vec<String>,
}

fn parse_allowlist(text: &str) -> Allowlist {
    let mut a = Allowlist::default();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        let count = |s: &str| s.parse::<usize>().ok();
        match parts.as_slice() {
            ["file", path, n] if count(n).is_some() => {
                a.files.insert(path.to_string(), count(n).unwrap_or(0));
            }
            ["test", path, n] if count(n).is_some() => {
                a.tests.insert(path.to_string(), count(n).unwrap_or(0));
            }
            ["fn", path, name, n] if count(n).is_some() => {
                a.functions
                    .insert((path.to_string(), name.to_string()), count(n).unwrap_or(0));
            }
            _ => a.malformed.push(line.to_string()),
        }
    }
    a
}

fn allowlist() -> &'static Allowlist {
    static LIST: OnceLock<Allowlist> = OnceLock::new();
    LIST.get_or_init(|| {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(ALLOWLIST);
        parse_allowlist(&std::fs::read_to_string(path).unwrap_or_default())
    })
}

/// A violation line, in allowlist format so a new offender's line can be
/// read straight off the failure, followed by why it fails.
fn over(kind: &str, subject: &str, actual: usize, cap: usize, allowed: Option<usize>) -> String {
    let why = match allowed {
        Some(n) => format!("grew past its allowlisted {n}"),
        None => format!("over the cap of {cap} and not allowlisted"),
    };
    format!("{kind} {subject} {actual}    # {why}")
}

fn check(violations: Vec<String>, what: &str) {
    assert!(
        violations.is_empty(),
        "{} {what} (shrink them; never raise an allowlist count):\n{}",
        violations.len(),
        violations.join("\n")
    );
}

// ---------------------------------------------------------------------------
// The gate
// ---------------------------------------------------------------------------

#[test]
fn production_files_fit_the_cap() {
    let allow = allowlist();
    let violations = tree()
        .iter()
        .filter(|(p, _)| !is_test_file(p) && !is_exempt(p))
        .filter(|(_, m)| m.production_lines > FILE_CAP)
        .filter_map(|(p, m)| {
            let allowed = allow.files.get(p).copied();
            (allowed.is_none_or(|n| m.production_lines > n))
                .then(|| over("file", p, m.production_lines, FILE_CAP, allowed))
        })
        .collect();
    check(violations, "production files over the cap");
}

/// Test lines a file holds: all of a test file, or the inline test-only
/// items of any other file (an inline `mod tests` is held to the same cap a
/// sibling `*_tests.rs` would be).
fn test_lines(path: &str, m: &Measure) -> usize {
    if is_test_file(path) {
        m.total_lines
    } else {
        m.total_lines - m.production_lines
    }
}

#[test]
fn test_files_fit_the_cap() {
    let allow = allowlist();
    let violations = tree()
        .iter()
        .map(|(p, m)| (p, test_lines(p, m)))
        .filter(|(_, n)| *n > TEST_FILE_CAP)
        .filter_map(|(p, n)| {
            let allowed = allow.tests.get(p).copied();
            (allowed.is_none_or(|a| n > a)).then(|| over("test", p, n, TEST_FILE_CAP, allowed))
        })
        .collect();
    check(violations, "files with more test lines than the cap");
}

#[test]
fn functions_fit_the_cap() {
    let allow = allowlist();
    let mut violations = Vec::new();
    for (path, m) in tree().iter().filter(|(p, _)| !is_test_file(p)) {
        for (name, lines) in &m.functions {
            if *lines <= FN_CAP {
                continue;
            }
            let allowed = allow.functions.get(&(path.clone(), name.clone())).copied();
            if allowed.is_none_or(|n| *lines > n) {
                violations.push(over(
                    "fn",
                    &format!("{path} {name}"),
                    *lines,
                    FN_CAP,
                    allowed,
                ));
            }
        }
    }
    check(violations, "functions over the cap");
}

#[test]
fn allowlist_has_no_stale_entries() {
    let allow = allowlist();
    let tree = tree();
    let mut stale: Vec<String> = allow
        .malformed
        .iter()
        .map(|l| format!("{l}    # not an entry: `file|test <path> <n>` or `fn <path> <name> <n>`"))
        .collect();
    for (path, n) in &allow.files {
        match tree.get(path) {
            None => stale.push(format!("file {path} {n}    # no such file: remove it")),
            Some(_) if is_test_file(path) || is_exempt(path) => stale.push(format!(
                "file {path} {n}    # a test or exempt file is not measured here: remove it"
            )),
            Some(m) if m.production_lines <= FILE_CAP => stale.push(format!(
                "file {path} {n}    # now {} lines, within the cap: remove it",
                m.production_lines
            )),
            Some(_) => {}
        }
    }
    for (path, n) in &allow.tests {
        match tree.get(path) {
            None => stale.push(format!("test {path} {n}    # no such file: remove it")),
            Some(m) if test_lines(path, m) <= TEST_FILE_CAP => stale.push(format!(
                "test {path} {n}    # now {} test lines, within the cap: remove it",
                test_lines(path, m)
            )),
            Some(_) => {}
        }
    }
    for ((path, name), n) in &allow.functions {
        let found = tree
            .get(path)
            .and_then(|m| m.functions.iter().find(|(f, _)| f == name));
        match found {
            None => stale.push(format!(
                "fn {path} {name} {n}    # no such function: remove it"
            )),
            Some((_, lines)) if *lines <= FN_CAP => stale.push(format!(
                "fn {path} {name} {n}    # now {lines} lines, within the cap: remove it"
            )),
            Some(_) => {}
        }
    }
    check(stale, "stale allowlist entries");
}

#[test]
fn exempt_files_exist() {
    let missing: Vec<&str> = EXEMPT
        .iter()
        .map(|(p, _)| *p)
        .filter(|p| !tree().contains_key(*p))
        .collect();
    assert!(
        missing.is_empty(),
        "exempt files that no longer exist: {missing:?}"
    );
}

// ---------------------------------------------------------------------------
// The measurer, on fixtures
// ---------------------------------------------------------------------------

fn m(src: &str) -> Measure {
    measure(src).expect("fixture parses")
}

fn len_of(measure: &Measure, name: &str) -> Option<usize> {
    measure
        .functions
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, l)| *l)
}

#[test]
fn production_lines_exclude_an_inline_test_module() {
    let src = "fn a() {}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {}\n}\n";
    let r = m(src);
    assert_eq!(r.total_lines, 7);
    assert_eq!(r.production_lines, 2, "lines 3-7 are the test module");
}

#[test]
fn production_after_a_test_only_fn_is_still_counted() {
    let src = "#[cfg(test)]\nfn helper() {}\n\npub fn real() {\n    let x = 1;\n}\n";
    let r = m(src);
    assert_eq!(
        r.production_lines, 4,
        "only the helper's two lines are test-only"
    );
    assert_eq!(len_of(&r, "real"), Some(3));
    assert_eq!(len_of(&r, "helper"), None);
}

#[test]
fn cfg_all_test_counts_as_test_only() {
    let src = "#[cfg(all(test, unix))]\nfn a() {}\n#[cfg(any(feature = \"x\", test))]\nfn b() {}\n#[tokio::test]\nasync fn c() {}\nfn d() {}\n";
    let r = m(src);
    assert_eq!(r.production_lines, 1);
    assert_eq!(
        r.functions
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>(),
        vec!["d"]
    );
}

#[test]
fn cfg_not_test_is_production() {
    let src = "#[cfg(not(test))]\nfn a() {}\n#[cfg(unix)]\nfn b() {}\n#[cfg(feature = \"test\")]\nfn c() {}\n";
    let r = m(src);
    assert_eq!(r.production_lines, 6, "none of these is test-only");
    assert_eq!(r.functions.len(), 3);
}

#[test]
fn function_length_spans_signature_to_closing_brace() {
    let src = "fn one() {}\nfn three(\n    x: u8,\n) {}\nfn five() {\n    let a = 1;\n    let b = 2;\n    let _ = a + b;\n}\n";
    let r = m(src);
    assert_eq!(len_of(&r, "one"), Some(1));
    assert_eq!(len_of(&r, "three"), Some(3));
    assert_eq!(len_of(&r, "five"), Some(5));
}

#[test]
fn doc_comments_and_attributes_are_not_function_lines() {
    let src = "/// Doc one.\n/// Doc two.\n#[inline]\n#[must_use]\npub fn e(x: u8)\n    -> u8 {\n    x\n}\n";
    let r = m(src);
    assert_eq!(len_of(&r, "e"), Some(4), "signature line 5 to brace line 8");
    assert_eq!(
        r.production_lines, 8,
        "doc lines still count toward the file"
    );
}

#[test]
fn impl_and_trait_methods_are_measured_with_their_type() {
    let src = "struct Foo;\ntrait Greet {\n    fn hi(&self) {\n    }\n    fn bare(&self);\n}\nimpl Foo {\n    fn new() -> Self { Foo }\n}\nimpl Greet for Foo {\n    fn hi(&self) {}\n}\nimpl<T> std::fmt::Debug for Vec2<T> {\n    fn fmt(&self, _: &mut std::fmt::Formatter) -> std::fmt::Result { Ok(()) }\n}\n";
    let r = m(src);
    assert_eq!(len_of(&r, "Greet::hi"), Some(2), "a trait default method");
    assert_eq!(
        len_of(&r, "Greet::bare"),
        None,
        "no body, nothing to measure"
    );
    assert_eq!(len_of(&r, "Foo::new"), Some(1));
    assert_eq!(
        len_of(&r, "Foo::hi"),
        Some(1),
        "a trait impl names the implementing type"
    );
    assert_eq!(len_of(&r, "Vec2::fmt"), Some(1));
}

#[test]
fn functions_in_a_test_module_are_not_measured() {
    let src = "#[cfg(test)]\nmod tests {\n    fn helper() {}\n    impl Foo { fn m() {} }\n}\nimpl Bar {\n    #[cfg(test)]\n    fn only_in_tests() {}\n    fn kept() {}\n}\n";
    let r = m(src);
    let names: Vec<&str> = r.functions.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, vec!["Bar::kept"]);
    assert_eq!(
        r.production_lines, 3,
        "lines 6, 9 and 10: the module (1-5) and the test-only method (7-8) are out"
    );
}

#[test]
fn duplicate_names_get_a_suffix() {
    let src = "struct A;\nstruct B;\nimpl A {\n    fn new() {}\n}\nimpl A {\n    fn new() {}\n}\nimpl B {\n    fn new() {}\n}\nmod x { fn go() {} }\nmod y { fn go() {} }\n";
    let r = m(src);
    let names: Vec<&str> = r.functions.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, vec!["A::new", "A::new#2", "B::new", "go", "go#2"]);
}

#[test]
fn allowlist_lines_parse_and_malformed_lines_are_kept() {
    let a = parse_allowlist(
        "# comment\n\nfile src/a.rs 900\ntest tests/b.rs 2100\nfn src/c.rs Foo::bar 200\nfile src/d.rs many\n",
    );
    assert_eq!(a.files.get("src/a.rs"), Some(&900));
    assert_eq!(a.tests.get("tests/b.rs"), Some(&2100));
    assert_eq!(
        a.functions
            .get(&("src/c.rs".to_string(), "Foo::bar".to_string())),
        Some(&200)
    );
    assert_eq!(a.malformed, vec!["file src/d.rs many".to_string()]);
}

#[test]
fn test_file_classes() {
    assert!(is_test_file("tests/integration.rs"));
    assert!(is_test_file("tests/harness/mod.rs"));
    assert!(is_test_file("src/ipc_tests.rs"));
    assert!(is_test_file("src/cli/tests.rs"));
    assert!(!is_test_file("src/testsuite.rs"));
    assert!(!is_test_file("src/contests.rs"));
    assert!(!is_test_file("src/daemon/stream.rs"));
}

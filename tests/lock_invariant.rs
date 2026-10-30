//! Source gate: outside test code, no mutex lock site may `unwrap()` or
//! `expect()` its guard. CLAUDE.md's invariant is that every lock site uses
//! `.unwrap_or_log()` (`crate::util::UnpoisonExt`), which logs an ERROR and
//! recovers from a poisoned lock instead of panicking.

use std::path::Path;

/// Production lines of one file: everything before its first
/// `#[cfg(test)]`, minus comment lines. A `*_tests.rs` or `tests.rs` file is
/// test code throughout and has none.
fn production_lines(path: &Path, text: &str) -> Vec<(usize, String)> {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string());
    if name
        .as_deref()
        .is_some_and(|n| n == "tests.rs" || n.ends_with("_tests.rs"))
    {
        return Vec::new();
    }
    let source = text.split("#[cfg(test)]").next().unwrap_or_default();
    source
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim_start().starts_with("//"))
        .map(|(i, l)| (i + 1, l.to_string()))
        .collect()
}

/// The lines that unwrap or expect a lock guard, including the split form
/// where `.lock()` ends one line and `.unwrap()` or `.expect(` starts the next.
fn lock_unwrap_hits(lines: &[(usize, String)]) -> Vec<usize> {
    let mut hits = Vec::new();
    for (i, (n, line)) in lines.iter().enumerate() {
        if line.contains(".lock().unwrap()") || line.contains(".lock().expect(") {
            hits.push(*n);
        } else if line.trim_end().ends_with(".lock()")
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
                for n in lock_unwrap_hits(&production_lines(&path, &text)) {
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

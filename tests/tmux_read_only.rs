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

use std::path::Path;

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

/// The span a `Command::new("tmux")` line opens: from that line up to the
/// first `])` or `)` that closes the `.args([...])` / `.arg(...)` call that
/// follows, scanning at most 15 further lines.
fn tmux_write_hits(src: &str) -> Vec<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let mut hits = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if !line.contains("Command::new(\"tmux\")") {
            continue;
        }
        let end = (i + 15).min(lines.len());
        let mut span = String::new();
        for next in &lines[i..end] {
            span.push_str(next);
            span.push('\n');
            if next.contains("])") || next.contains(')') {
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
                let source = text
                    .split("#[cfg(test)]")
                    .next()
                    .unwrap_or_default()
                    .to_string();
                for (line, verb) in tmux_write_hits(&source) {
                    failures.push(format!(
                        "{}:{}: tmux write verb `{verb}`",
                        path.display(),
                        line
                    ));
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

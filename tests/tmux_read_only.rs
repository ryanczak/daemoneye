//! Source gate: `src/**/*.rs` (outside `#[cfg(test)]` modules) must contain no
//! tmux write verbs.
//!
//! M27 turns tmux into an optional observer: the daemon and the client only
//! ever read from it. Every mutation goes through the user's own shell, a
//! saved script, or the daemon's explicit spawn path, so a write verb literal
//! appearing in ordinary source means someone re-coupled the client to tmux.

use std::path::Path;

const FORBIDDEN: &[&str] = &[
    "send-keys",
    "new-window",
    "new-session",
    "kill-window",
    "kill-session",
    "set-hook",
    "pipe-pane",
    "respawn-pane",
    "split-window",
    "resize-pane",
    "focus-events",
];

fn scan_file(path: &Path) -> Vec<(String, usize, String)> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            panic!("could not read {}: {e}", path.display());
        }
    };
    let mut hits = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.contains("#[cfg(test)]") {
            break;
        }
        for verb in FORBIDDEN {
            if line.contains(verb) {
                hits.push((path.display().to_string(), i + 1, verb.to_string()));
            }
        }
    }
    hits
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
                for (file, line, verb) in scan_file(&path) {
                    failures.push(format!("{file}:{line}: tmux write verb `{verb}`"));
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

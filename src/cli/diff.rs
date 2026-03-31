use similar::{ChangeTag, TextDiff};

const CONTEXT_LINES: usize = 3;
const NEW_FILE_LINE_CAP: usize = 80;

/// Renders an ANSI-colored unified diff suitable for terminal display.
///
/// - `existing_content = None`  → new file: all lines shown as green `+`
/// - `existing_content = Some`  → modified file: standard unified diff
///
/// Returns a Vec of pre-colored lines ready to print.
pub(crate) fn render_diff(
    name: &str,
    existing_content: Option<&str>,
    new_content: &str,
) -> Vec<String> {
    match existing_content {
        None => render_new_file(name, new_content),
        Some(old) => render_unified(name, old, new_content),
    }
}

fn render_new_file(name: &str, content: &str) -> Vec<String> {
    let mut out = Vec::new();
    out.push(format!("\x1b[1mnew file: {}\x1b[0m", name));

    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    let shown = total.min(NEW_FILE_LINE_CAP);

    for line in &lines[..shown] {
        out.push(format!("\x1b[32m+{}\x1b[0m", line));
    }
    if total > shown {
        out.push(format!("\x1b[2m  … ({} more lines)\x1b[0m", total - shown));
    }
    out
}

fn render_unified(name: &str, old: &str, new: &str) -> Vec<String> {
    let mut out = Vec::new();

    let diff = TextDiff::from_lines(old, new);

    // Check if there are any changes at all
    let has_changes = diff
        .ops()
        .iter()
        .any(|op| !matches!(op.tag(), similar::DiffTag::Equal));

    if !has_changes {
        out.push(format!("\x1b[2mno changes: {}\x1b[0m", name));
        return out;
    }

    out.push(format!("\x1b[1mmodified: {}\x1b[0m", name));

    for group in diff.grouped_ops(CONTEXT_LINES) {
        // Hunk header
        let old_range = group.first().and_then(|op| {
            let r = op.old_range();
            Some((r.start + 1, r.len()))
        });
        let new_range = group.last().and_then(|op| {
            let r = op.new_range();
            Some((r.start + 1, r.len()))
        });

        // Compute the full range of the group for old and new
        let old_start = group
            .first()
            .map(|op| op.old_range().start + 1)
            .unwrap_or(1);
        let old_end = group
            .last()
            .map(|op| op.old_range().end)
            .unwrap_or(old_start);
        let new_start = group
            .first()
            .map(|op| op.new_range().start + 1)
            .unwrap_or(1);
        let new_end = group
            .last()
            .map(|op| op.new_range().end)
            .unwrap_or(new_start);
        let _ = (old_range, new_range); // suppress unused warnings

        out.push(format!(
            "\x1b[36m@@ -{},{} +{},{} @@\x1b[0m",
            old_start,
            old_end.saturating_sub(old_start - 1),
            new_start,
            new_end.saturating_sub(new_start - 1),
        ));

        for op in &group {
            for change in diff.iter_changes(op) {
                let line = change.value().trim_end_matches('\n');
                match change.tag() {
                    ChangeTag::Delete => {
                        out.push(format!("\x1b[31m-{}\x1b[0m", line));
                    }
                    ChangeTag::Insert => {
                        out.push(format!("\x1b[32m+{}\x1b[0m", line));
                    }
                    ChangeTag::Equal => {
                        out.push(format!(" {}", line));
                    }
                }
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip_ansi(s: &str) -> String {
        // Very basic ANSI stripper for test assertions
        let mut out = String::new();
        let mut in_escape = false;
        for c in s.chars() {
            if c == '\x1b' {
                in_escape = true;
            } else if in_escape {
                if c == 'm' {
                    in_escape = false;
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn new_file_all_green() {
        let lines = render_diff("foo.sh", None, "#!/bin/bash\necho hello\n");
        // Header marks as new file
        assert!(lines[0].contains("new file: foo.sh"));
        // Lines are prefixed with +
        assert!(lines[1].starts_with("\x1b[32m+"));
        assert!(lines[2].starts_with("\x1b[32m+"));
        // Plain text content correct
        assert!(strip_ansi(&lines[1]).starts_with('+'));
    }

    #[test]
    fn new_file_cap_at_80_lines() {
        let content: String = (0..100).map(|i| format!("line {}\n", i)).collect();
        let lines = render_diff("big.sh", None, &content);
        // 1 header + 80 content lines + 1 trailer
        assert_eq!(lines.len(), 82);
        assert!(lines.last().unwrap().contains("20 more lines"));
    }

    #[test]
    fn identical_content_no_changes() {
        let content = "#!/bin/bash\necho hi\n";
        let lines = render_diff("same.sh", Some(content), content);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("no changes"));
    }

    #[test]
    fn single_line_change() {
        let old = "line1\nline2\nline3\n";
        let new = "line1\nLINE2\nline3\n";
        let lines = render_diff("f.sh", Some(old), new);
        // Should contain a hunk header and a removed + added line
        let plain: Vec<String> = lines.iter().map(|l| strip_ansi(l)).collect();
        assert!(plain.iter().any(|l| l.starts_with("@@")));
        assert!(plain.iter().any(|l| l.starts_with("-line2")));
        assert!(plain.iter().any(|l| l.starts_with("+LINE2")));
    }

    #[test]
    fn multi_hunk_diff() {
        let old = (0..20).map(|i| format!("line{}\n", i)).collect::<String>();
        let mut new_lines: Vec<String> = (0..20).map(|i| format!("line{}\n", i)).collect();
        new_lines[0] = "CHANGED\n".to_string();
        new_lines[19] = "ALSO_CHANGED\n".to_string();
        let new = new_lines.join("");
        let lines = render_diff("m.sh", Some(&old), &new);
        let plain: Vec<String> = lines.iter().map(|l| strip_ansi(l)).collect();
        // Two separate hunk headers
        let hunk_count = plain.iter().filter(|l| l.starts_with("@@")).count();
        assert_eq!(hunk_count, 2);
    }

    #[test]
    fn empty_to_nonempty() {
        let lines = render_diff("new.sh", Some(""), "hello\n");
        let plain: Vec<String> = lines.iter().map(|l| strip_ansi(l)).collect();
        assert!(plain.iter().any(|l| l.starts_with("+hello")));
    }

    #[test]
    fn nonempty_to_empty() {
        let lines = render_diff("del.sh", Some("hello\n"), "");
        let plain: Vec<String> = lines.iter().map(|l| strip_ansi(l)).collect();
        assert!(plain.iter().any(|l| l.starts_with("-hello")));
    }
}

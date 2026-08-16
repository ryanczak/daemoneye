use super::super::ToolCallOutcome;
use crate::ai::mask_sensitive;
use crate::daemon::session::BUFFER_COUNTER;
use crate::daemon::utils::get_pane_remote_host;
use crate::tmux;

/// Extract lines between a unique start marker and end marker from pane output.
fn extract_marked(snap: &str, start: &str, end: &str) -> Option<String> {
    let lines: Vec<&str> = snap.lines().collect();
    let s_idx = lines.iter().position(|l| l.trim() == start)?;
    let e_idx = lines.iter().rposition(|l| l.trim() == end)?;
    if e_idx <= s_idx {
        return None;
    }
    Some(lines[s_idx + 1..e_idx].join("\n"))
}

/// Build the shell command to read `path` from a remote pane with markers.
fn build_remote_read_cmd(path: &str, start: usize, end: usize, pattern: Option<&str>) -> String {
    let safe_path = super::sq_escape(path);
    let grep_part = pattern
        .map(|p| format!(" | grep -E '{}'", super::sq_escape(p)))
        .unwrap_or_default();
    format!(
        "echo '__DE_S__'; sed -n '{},{}p' '{}' 2>&1{}; echo '__DE_E__'; echo '__DE_DONE__'",
        start, end, safe_path, grep_part
    )
}

/// Build the shell command to read `path` through the tmux buffer system (no scrollback cap).
fn build_local_buffer_read_cmd(
    path: &str,
    start: usize,
    end: usize,
    pattern: Option<&str>,
    buf_name: &str,
) -> String {
    let safe_path = super::sq_escape(path);
    let grep_part = pattern
        .map(|p| format!(" | grep -E '{}'", super::sq_escape(p)))
        .unwrap_or_default();
    format!(
        "sed -n '{},{}p' '{}'{}  | tmux load-buffer -b '{}' -; tmux wait-for -S '{}'",
        start, end, safe_path, grep_part, buf_name, buf_name
    )
}

/// Run a read-file command in a LOCAL target pane using `load-buffer`/`save-buffer`.
async fn local_read_via_buffer(
    pane_id: &str,
    path: &str,
    start: usize,
    end: usize,
    pattern: Option<&str>,
) -> anyhow::Result<String> {
    let idx = BUFFER_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let buf_name = format!("de-rb-{}", idx);
    let cmd = build_local_buffer_read_cmd(path, start, end, pattern, &buf_name);

    let p = pane_id.to_string();
    let c = cmd.to_string();
    tmux::off_runtime("send-keys", move || tmux::send_keys(&p, &c))
        .await
        .ok_or_else(|| anyhow::anyhow!("timed out sending keys to pane {pane_id}"))??;

    // Local pane → its shell shares our tmux server, so it can signal `buf_name`.
    let signalled = tmux::wait_for(&buf_name, std::time::Duration::from_secs(30)).await;

    // Read the buffer regardless: a lost or raced signal must not lose a load that
    // actually completed, and an empty buffer after a timeout is the real failure.
    let bn = buf_name.clone();
    let bytes = tmux::off_runtime("save-buffer", move || tmux::save_buffer(&bn))
        .await
        .and_then(|r| r.ok())
        .unwrap_or_default();
    let bn2 = buf_name.clone();
    let _ = tmux::off_runtime("delete-buffer", move || tmux::delete_buffer(&bn2)).await;

    if !signalled && bytes.is_empty() {
        anyhow::bail!("Timed out waiting for buffer load in pane {}", pane_id);
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

// ---------------------------------------------------------------------------
// read_file
// ---------------------------------------------------------------------------

pub async fn run_read_file(
    path: &str,
    offset: Option<u64>,
    limit: Option<u64>,
    pattern: Option<&str>,
    target_pane: Option<&str>,
) -> anyhow::Result<ToolCallOutcome> {
    if path.contains("..") {
        return Ok(ToolCallOutcome::Result(
            "Error: path must not contain '..'.".to_string(),
        ));
    }
    if super::contains_control(path) {
        return Ok(ToolCallOutcome::Result(
            "Error: path must not contain control characters.".to_string(),
        ));
    }
    if pattern.is_some_and(super::contains_control) {
        return Ok(ToolCallOutcome::Result(
            "Error: grep pattern must not contain control characters.".to_string(),
        ));
    }
    if !std::path::Path::new(path).is_absolute() {
        return Ok(ToolCallOutcome::Result(
            "Error: path must be absolute (e.g. /var/log/syslog).".to_string(),
        ));
    }

    {
        let de_dir = crate::config::config_dir();
        let candidate = super::resolve_path_for_guard(path);
        if candidate.starts_with(&de_dir) {
            let blocked = [
                de_dir.join("etc").join("config.toml"),
                de_dir.join("etc").join("prompts").join("sre.toml"),
            ];
            if blocked.contains(&candidate) {
                return Ok(ToolCallOutcome::Result(
                    "Error: read_file cannot access daemoneye credential files \
                     (etc/config.toml, etc/prompts/sre.toml). These may contain \
                     sensitive API keys. Use read_script, read_runbook, read_memory, \
                     or search_repository for other daemoneye-managed data."
                        .to_string(),
                ));
            }
        }
    }

    const MAX_LINES: usize = 500;
    const DEFAULT_LINES: usize = 200;
    let limit_n = match limit {
        Some(n) if n > 0 => (n as usize).min(MAX_LINES),
        _ => DEFAULT_LINES,
    };
    let offset_n = offset.map(|o| (o as usize).saturating_sub(1)).unwrap_or(0);

    // ── Target-pane path: run sed/grep in target_pane ─────────────────────
    if let Some(pane) = target_pane {
        let start = offset_n + 1;
        let end = offset_n + limit_n;

        let p = pane.to_string();
        let is_remote = tmux::off_runtime("pane-remote-host", move || get_pane_remote_host(&p))
            .await
            .flatten()
            .is_none();

        let (content, is_remote) = if is_remote {
            let raw = match local_read_via_buffer(pane, path, start, end, pattern).await {
                Ok(s) => s,
                Err(e) => return Ok(ToolCallOutcome::Result(format!("Error: {}", e))),
            };
            (raw, false)
        } else {
            let cmd = build_remote_read_cmd(path, start, end, pattern);
            let snap = match super::remote_run_and_capture(pane, &cmd, 30).await {
                Ok(s) => s,
                Err(e) => return Ok(ToolCallOutcome::Result(format!("Error: {}", e))),
            };
            let extracted =
                extract_marked(&snap, "__DE_S__", "__DE_E__").unwrap_or_else(|| snap.clone());
            (extracted, true)
        };

        if content.trim().is_empty() {
            return Ok(ToolCallOutcome::Result(format!(
                "{}: no output (file may be empty or lines out of range)",
                path
            )));
        }
        let body = mask_sensitive(content.trim_end());
        let label = if is_remote {
            if pattern.is_some() {
                format!("{} (remote grep, lines {}-{}):\n{}", path, start, end, body)
            } else {
                format!("{} (remote, lines {}-{}):\n{}", path, start, end, body)
            }
        } else if pattern.is_some() {
            format!("{} (local grep, lines {}-{}):\n{}", path, start, end, body)
        } else {
            format!("{} (local pane, lines {}-{}):\n{}", path, start, end, body)
        };
        return Ok(ToolCallOutcome::Result(label));
    }

    // ── Local path: read directly from daemon-host filesystem ─────────────
    let real_path = std::fs::canonicalize(path).unwrap_or_else(|_| std::path::PathBuf::from(path));
    let raw = match std::fs::read_to_string(&real_path) {
        Ok(s) => s,
        Err(e) => {
            return Ok(ToolCallOutcome::Result(format!(
                "Error reading {}: {}",
                path, e
            )));
        }
    };

    let all_lines: Vec<&str> = raw.lines().collect();
    let total = all_lines.len();
    let sliced = &all_lines[offset_n.min(total)..];
    let limited: Vec<&str> = sliced.iter().take(limit_n).copied().collect();
    let limited_len = limited.len();

    let filtered: Vec<&str> = if let Some(pat) = pattern {
        match regex::RegexBuilder::new(pat).size_limit(1 << 20).build() {
            Ok(re) => limited.into_iter().filter(|l| re.is_match(l)).collect(),
            Err(e) => {
                return Ok(ToolCallOutcome::Result(format!(
                    "Error: invalid pattern regex: {}",
                    e
                )));
            }
        }
    } else {
        limited
    };

    if filtered.is_empty() {
        return Ok(ToolCallOutcome::Result(format!(
            "{}: no lines matched (total {} lines in file)",
            path, total
        )));
    }

    let body = mask_sensitive(&filtered.join("\n"));
    if pattern.is_some() {
        Ok(ToolCallOutcome::Result(format!(
            "{} ({} matching lines, searched lines {}-{} of {}):\n{}",
            path,
            filtered.len(),
            offset_n + 1,
            (offset_n + limited_len).min(total),
            total,
            body
        )))
    } else {
        Ok(ToolCallOutcome::Result(format!(
            "{} (lines {}-{} of {}):\n{}",
            path,
            offset_n + 1,
            (offset_n + filtered.len()).min(total),
            total,
            body
        )))
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use crate::daemon::executor::ToolCallOutcome;
    use std::env;

    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    struct TmpHome(std::path::PathBuf);
    impl TmpHome {
        fn new() -> Self {
            let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let p = std::env::temp_dir().join(format!("de_fops_test_{}_{}", std::process::id(), n));
            std::fs::create_dir_all(&p).unwrap();
            TmpHome(p)
        }
    }
    impl Drop for TmpHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn with_home<F: FnOnce()>(tmp: &TmpHome, f: F) {
        let _guard = crate::test_home_guard();
        let old = env::var("HOME").ok();
        unsafe {
            env::set_var("HOME", &tmp.0);
        }
        f();
        match old {
            Some(v) => unsafe {
                env::set_var("HOME", v);
            },
            None => unsafe {
                env::remove_var("HOME");
            },
        }
    }

    fn simulate_read_file(lines: &[&str]) -> (TmpHome, std::path::PathBuf) {
        let tmp = TmpHome::new();
        let path = tmp.0.join("test_file.txt");
        std::fs::write(&path, lines.join("\n")).unwrap();
        (tmp, path)
    }

    #[tokio::test]
    async fn read_file_default_reads_from_start() {
        let (tmp, path) = simulate_read_file(&["line1", "line2", "line3"]);
        with_home(&tmp, || {});
        let result = super::run_read_file(path.to_str().unwrap(), None, None, None, None)
            .await
            .unwrap();
        let ToolCallOutcome::Result(s) = result else {
            panic!()
        };
        assert!(s.contains("line1"));
        assert!(s.contains("line3"));
    }

    #[tokio::test]
    async fn read_file_offset_skips_lines() {
        // Use zero-padded names to avoid "line1" being a substring of "line10".
        let lines: Vec<String> = (1..=10).map(|i| format!("line{:02}", i)).collect();
        let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        let (tmp, path) = simulate_read_file(&refs);
        with_home(&tmp, || {});
        let result = super::run_read_file(path.to_str().unwrap(), Some(5), None, None, None)
            .await
            .unwrap();
        let ToolCallOutcome::Result(s) = result else {
            panic!()
        };
        assert!(!s.contains("line01"), "offset should skip line01");
        assert!(s.contains("line05"), "should start from line05");
    }

    #[tokio::test]
    async fn read_file_limit_caps_output() {
        let lines: Vec<String> = (1..=20).map(|i| format!("line{}", i)).collect();
        let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        let (tmp, path) = simulate_read_file(&refs);
        with_home(&tmp, || {});
        let result = super::run_read_file(path.to_str().unwrap(), None, Some(3), None, None)
            .await
            .unwrap();
        let ToolCallOutcome::Result(s) = result else {
            panic!()
        };
        assert!(s.contains("line1"));
        assert!(s.contains("line3"));
        assert!(!s.contains("line4"), "limit=3 should not include line4");
    }

    #[tokio::test]
    async fn read_file_limit_capped_at_max() {
        let lines: Vec<String> = (1..=600).map(|i| format!("line{}", i)).collect();
        let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        let (tmp, path) = simulate_read_file(&refs);
        with_home(&tmp, || {});
        let result = super::run_read_file(path.to_str().unwrap(), None, Some(600), None, None)
            .await
            .unwrap();
        let ToolCallOutcome::Result(s) = result else {
            panic!()
        };
        // MAX_LINES = 500; line501 should not appear
        assert!(!s.contains("line501"), "should be capped at 500 lines");
    }

    #[tokio::test]
    async fn read_file_pattern_grep_mode_header() {
        let (tmp, path) = simulate_read_file(&["apple", "banana", "cherry"]);
        with_home(&tmp, || {});
        let result = super::run_read_file(path.to_str().unwrap(), None, None, Some("banana"), None)
            .await
            .unwrap();
        let ToolCallOutcome::Result(s) = result else {
            panic!()
        };
        assert!(s.contains("matching lines"));
        assert!(s.contains("banana"));
    }

    #[tokio::test]
    async fn read_file_pattern_no_match_returns_message() {
        let (tmp, path) = simulate_read_file(&["apple", "banana"]);
        with_home(&tmp, || {});
        let result = super::run_read_file(
            path.to_str().unwrap(),
            None,
            None,
            Some("xyzzy_not_found"),
            None,
        )
        .await
        .unwrap();
        let ToolCallOutcome::Result(s) = result else {
            panic!()
        };
        assert!(s.contains("no lines matched"));
    }

    #[tokio::test]
    async fn read_file_rejects_control_chars_in_path() {
        let (tmp, path) = simulate_read_file(&["line1"]);
        with_home(&tmp, || {});
        let evil = format!("{}\n; touch /tmp/pwned; echo", path.display());
        let result = super::run_read_file(&evil, None, None, None, None)
            .await
            .unwrap();
        let ToolCallOutcome::Result(s) = result else {
            panic!()
        };
        assert!(s.contains("control characters"), "got: {s}");
        assert!(!std::path::Path::new("/tmp/pwned").exists());
    }

    #[tokio::test]
    async fn read_file_rejects_control_chars_in_pattern() {
        let (tmp, path) = simulate_read_file(&["line1"]);
        with_home(&tmp, || {});
        let result = super::run_read_file(
            path.to_str().unwrap(),
            None,
            None,
            Some("banana\n; touch /tmp/pwned2; echo"),
            None,
        )
        .await
        .unwrap();
        let ToolCallOutcome::Result(s) = result else {
            panic!()
        };
        assert!(s.contains("control characters"), "got: {s}");
        assert!(!std::path::Path::new("/tmp/pwned2").exists());
    }

    #[tokio::test]
    async fn read_file_offset_beyond_eof_returns_empty() {
        let (tmp, path) = simulate_read_file(&["line1", "line2"]);
        with_home(&tmp, || {});
        let result = super::run_read_file(path.to_str().unwrap(), Some(1000), None, None, None)
            .await
            .unwrap();
        let ToolCallOutcome::Result(s) = result else {
            panic!()
        };
        assert!(s.contains("no lines matched"));
    }

    // ── Defect A: sentinel collision ──

    #[test]
    fn extract_marked_ignores_embedded_end_marker() {
        let snap = [
            "other stuff",
            "__DE_S__",
            "line one",
            "some content with __DE_E__ embedded",
            "line three",
            "__DE_E__",
            "trailing",
        ]
        .join("\n");
        let body = super::extract_marked(&snap, "__DE_S__", "__DE_E__").unwrap();
        assert!(
            body.contains("some content with __DE_E__ embedded"),
            "embedded marker should not truncate the body"
        );
        assert!(
            body.contains("line three"),
            "body should include all lines up to the real sentinel"
        );
    }

    #[test]
    fn extract_marked_exact_line_only() {
        let snap = ["__DE_S__", "line one", "__DE_E__", "after sentinel"].join("\n");
        let body = super::extract_marked(&snap, "__DE_S__", "__DE_E__").unwrap();
        assert_eq!(
            body, "line one",
            "standalone marker line must still be treated as a boundary"
        );
    }

    // ── Defect C: path guard symlink resolution ──

    #[test]
    fn path_guard_follows_symlink_parent_into_config_dir() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            let de_dir = crate::config::config_dir();
            let real_subdir = de_dir.join("etc");
            std::fs::create_dir_all(&real_subdir).unwrap();

            let link_parent = tmp.0.join("symlink_parent");
            std::fs::create_dir(&link_parent).unwrap();
            let symlink_path = link_parent.join("evil_link");
            std::os::unix::fs::symlink(&real_subdir, &symlink_path).unwrap();

            let leaf = symlink_path.join("new_file.txt");
            let resolved = super::super::resolve_path_for_guard(leaf.to_str().unwrap());
            assert!(
                resolved.starts_with(&de_dir),
                "resolved path {resolved:?} should be under config dir {de_dir:?}"
            );
        });
    }

    #[test]
    fn path_guard_allows_nonexistent_leaf_under_real_parent() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            let de_dir = crate::config::config_dir();
            std::fs::create_dir_all(&de_dir).unwrap();

            let real_parent = tmp.0.join("real_parent");
            std::fs::create_dir(&real_parent).unwrap();

            let leaf = real_parent.join("brand_new.txt");
            let resolved = super::super::resolve_path_for_guard(leaf.to_str().unwrap());
            assert!(
                !resolved.starts_with(&de_dir),
                "resolved path {resolved:?} should NOT be under config dir {de_dir:?}"
            );
        });
    }

    #[test]
    fn local_buffer_read_cmd_signals_via_wait_for() {
        let cmd = super::build_local_buffer_read_cmd("/var/log/x", 1, 40, None, "de-rb-7");

        assert!(
            cmd.contains("tmux wait-for -S 'de-rb-7'"),
            "command must signal via wait-for: {cmd}"
        );
        assert!(
            !cmd.contains("__DE_DONE__"),
            "command must NOT contain the old sentinel: {cmd}"
        );
        assert!(
            !cmd.contains("echo"),
            "command must NOT contain echo: {cmd}"
        );
        assert!(
            cmd.contains("tmux load-buffer -b 'de-rb-7' -"),
            "command must still load the buffer: {cmd}"
        );
    }
}

use super::super::ToolCallOutcome;
use crate::ai::mask_sensitive;
use crate::shell::Owner;

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

// ---------------------------------------------------------------------------
// read_file
// ---------------------------------------------------------------------------

pub async fn run_read_file(
    path: &str,
    offset: Option<u64>,
    limit: Option<u64>,
    pattern: Option<&str>,
    host: Option<&str>,
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

    if super::is_blocked_secret_path(path) {
        return Ok(ToolCallOutcome::Result(
            "Error: read_file cannot access well-known credential files \
             (~/.ssh, .env, private keys). This is not a security boundary — \
             use run_terminal_command only if you truly need to inspect one."
                .to_string(),
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

    // ── Host resolution: `None` / "local" is the daemon host ───────────────
    let host_id = match host {
        None => crate::host::HostId::Local,
        Some(name) => match crate::host::HostId::parse(name) {
            Ok(parsed) => parsed,
            Err(msg) => return Ok(ToolCallOutcome::Result(format!("Error: {msg}"))),
        },
    };
    match &host_id {
        crate::host::HostId::Local => {
            // ── Local path: read directly from daemon-host filesystem ─────
            let real_path =
                std::fs::canonicalize(path).unwrap_or_else(|_| std::path::PathBuf::from(path));
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
        crate::host::HostId::Named(name) => {
            let start = offset_n + 1;
            let end = offset_n + limit_n;
            let cmd = build_remote_read_cmd(path, start, end, pattern);
            let snap = match super::run_in_host_shell(
                crate::shell::registry(),
                Owner::Chat(host_session_key()),
                host_id.clone(),
                &cmd,
                REMOTE_READ_TIMEOUT_SECS,
                |id| crate::shell::spawn_ssh_host(id, name, 24, 80),
            )
            .await
            {
                Ok(s) => s,
                Err(e) => return Ok(ToolCallOutcome::Result(format!("Error: {}", e))),
            };
            let extracted =
                extract_marked(&snap, "__DE_S__", "__DE_E__").unwrap_or_else(|| snap.clone());
            Ok(format_remote_read_result(
                path, name, start, end, pattern, &extracted,
            ))
        }
    }
}

/// The owner key a `read_file { host }` run claims its shell under.
///
/// `run_read_file` receives no session id, so the shells a remote read
/// claims are keyed to the tool itself: `chat:read_file`. Keying on the
/// tool rather than inventing a per-session key is what makes test 6's
/// one-shell-per-host reuse property hold without threading `session_id`
/// through every read.
fn host_session_key() -> String {
    "read_file".to_string()
}

/// The remote read's result label. The host name is in it so a model reading
/// two files from two machines can tell them apart.
fn format_remote_read_result(
    path: &str,
    host: &str,
    start: usize,
    end: usize,
    pattern: Option<&str>,
    content: &str,
) -> ToolCallOutcome {
    if content.trim().is_empty() {
        return ToolCallOutcome::Result(format!(
            "{}: no output (file may be empty or lines out of range)",
            path
        ));
    }
    let body = mask_sensitive(content.trim_end());
    let label = if pattern.is_some() {
        format!(
            "{} on host {} (remote grep, lines {}-{}):\n{}",
            path, host, start, end, body
        )
    } else {
        format!(
            "{} on host {} (lines {}-{}):\n{}",
            path, host, start, end, body
        )
    };
    ToolCallOutcome::Result(label)
}
/// The timeout one remote read is given, in seconds — the pane path's own
/// value, carried over unchanged.
const REMOTE_READ_TIMEOUT_SECS: u64 = 30;

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
    async fn read_file_rejects_credential_paths() {
        let _guard = crate::test_home_guard();
        let tmp = TmpHome::new();
        unsafe { env::set_var("HOME", &tmp.0) };
        let home = crate::config::config_dir().parent().unwrap().to_path_buf();

        // $HOME/.ssh (every file under it, including pubkeys/known_hosts)
        let ssh = home.join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        for f in ["id_ed25519", "id_rsa.pub", "config", "known_hosts"] {
            let p = ssh.join(f);
            std::fs::write(&p, "secret").unwrap();
            let result = super::run_read_file(p.to_str().unwrap(), None, None, None, None)
                .await
                .unwrap();
            let ToolCallOutcome::Result(s) = result else {
                panic!()
            };
            assert!(
                s.contains("credential"),
                "expected credential-block error for {f}, got: {s}"
            );
        }
        // .env file anywhere
        let env_file = tmp.0.join(".env");
        std::fs::write(&env_file, "SECRET=1").unwrap();
        let result = super::run_read_file(env_file.to_str().unwrap(), None, None, None, None)
            .await
            .unwrap();
        let ToolCallOutcome::Result(s) = result else {
            panic!()
        };
        assert!(s.contains("credential"), "got: {s}");
    }

    #[tokio::test]
    async fn read_file_allows_normal_dotfiles() {
        let _guard = crate::test_home_guard();
        let tmp = TmpHome::new();
        unsafe { env::set_var("HOME", &tmp.0) };
        let ok = tmp.0.join(".bashrc");
        std::fs::write(&ok, "alias a=b").unwrap();
        let result = super::run_read_file(ok.to_str().unwrap(), None, None, None, None)
            .await
            .unwrap();
        let ToolCallOutcome::Result(s) = result else {
            panic!()
        };
        assert!(s.contains("alias a=b"), "got: {s}");
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

    // ── M21 phase-07a: read_file { host? } ─────────────────────────────────
    // ── M21 phase-07a: read_file { host? } ─────────────────────────────────
    // ── M21 phase-07a: read_file { host? } ─────────────────────────────────

    /// Preservation: the command the new transport runs is byte-for-byte the
    /// one the pane path ran. The markers, the `sed` range and the quoted path
    /// are a contract with `extract_marked`; a silent change here would make a
    /// remote read return the whole screen.
    #[test]
    fn build_remote_read_cmd_is_unchanged() {
        let cmd = super::build_remote_read_cmd("/var/log/app.log", 10, 20, None);
        assert_eq!(
            cmd,
            "echo '__DE_S__'; sed -n '10,20p' '/var/log/app.log' 2>&1; \
             echo '__DE_E__'; echo '__DE_DONE__'",
            "the wire format must not drift"
        );
        let with_grep = super::build_remote_read_cmd("/tmp/f", 1, 5, Some("er+r?or"));
        assert_eq!(
            with_grep,
            "echo '__DE_S__'; sed -n '1,5p' '/tmp/f' 2>&1 | grep -E 'er+r?or'; \
             echo '__DE_E__'; echo '__DE_DONE__'",
            "the grep suffix is part of the same wire format"
        );
    }

    /// Preservation of the extraction the new transport feeds: the body is
    /// strictly between the two sentinels.
    #[test]
    fn extract_marked_cuts_between_the_sentinels() {
        let snap = [
            "ssh banner noise",
            "__DE_S__",
            "alpha",
            "bravo",
            "__DE_E__",
            "__DE_DONE__",
        ]
        .join("\n");
        assert_eq!(
            super::extract_marked(&snap, "__DE_S__", "__DE_E__").unwrap(),
            "alpha\nbravo",
            "the cut is between the sentinels only"
        );
        let empty = ["__DE_S__", "__DE_E__"].join("\n");
        assert_eq!(
            super::extract_marked(&empty, "__DE_S__", "__DE_E__").unwrap(),
            "",
            "an empty range is an empty body, not None"
        );
        let missing = "no markers at all".to_string();
        assert!(
            super::extract_marked(&missing, "__DE_S__", "__DE_E__").is_none(),
            "no sentinels means no extraction — the caller's fallback decides"
        );
    }

    /// A malformed host name is refused with `HostId::parse`'s own message,
    /// before any shell is opened.
    #[tokio::test]
    async fn a_malformed_host_is_refused_by_read_file() {
        let result = super::run_read_file("/tmp/whatever", None, None, None, Some("bad host!"))
            .await
            .unwrap();
        let ToolCallOutcome::Result(s) = result else {
            panic!()
        };
        assert!(s.contains("bad host!"), "the refusal echoes the name: {s}");
        assert!(
            s.contains("[A-Za-z0-9._-]"),
            "the refusal is HostId::parse's shape message: {s}"
        );
    }

    /// `None` means the daemon host: the local filesystem read, unchanged.
    #[tokio::test]
    async fn read_file_without_a_host_takes_the_local_path() {
        let (tmp, path) = simulate_read_file(&["local-only-line"]);
        with_home(&tmp, || {});
        let result = super::run_read_file(path.to_str().unwrap(), None, None, None, None)
            .await
            .unwrap();
        let ToolCallOutcome::Result(s) = result else {
            panic!()
        };
        assert!(
            s.contains("local-only-line"),
            "the content came from the local file: {s}"
        );
        assert!(
            !s.contains("on host"),
            "the local label carries no host clause: {s}"
        );
    }

    /// `"local"` in any case parses to `HostId::Local` and takes the same
    /// local path.
    #[tokio::test]
    async fn read_file_host_local_takes_the_local_path() {
        for spelling in ["local", "Local", "LOCAL"] {
            let (tmp, path) = simulate_read_file(&["still-local-line"]);
            with_home(&tmp, || {});
            let result =
                super::run_read_file(path.to_str().unwrap(), None, None, None, Some(spelling))
                    .await
                    .unwrap();
            let ToolCallOutcome::Result(s) = result else {
                panic!()
            };
            assert!(
                s.contains("still-local-line"),
                "{spelling:?} read the local file: {s}"
            );
            assert!(
                !s.contains("on host"),
                "{spelling:?} must not label a host: {s}"
            );
        }
    }

    /// Preservation: the local read path produces the same result text it did
    /// before this row — the label shape and the pagination clamps are what a
    /// model's parsing already depends on.
    #[tokio::test]
    async fn the_local_read_path_is_unchanged() {
        let lines: Vec<String> = (1..=10).map(|i| format!("line{:02}", i)).collect();
        let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        let (tmp, path) = simulate_read_file(&refs);
        with_home(&tmp, || {});

        let plain = super::run_read_file(path.to_str().unwrap(), None, None, None, None)
            .await
            .unwrap();
        let ToolCallOutcome::Result(plain) = plain else {
            panic!()
        };
        let expected_body = (1..=10)
            .map(|i| format!("line{i:02}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            plain,
            format!("{} (lines 1-10 of 10):\n{}", path.display(), expected_body),
            "the plain label is byte-identical to the pre-07a shape"
        );

        let offset = super::run_read_file(path.to_str().unwrap(), Some(3), Some(2), None, None)
            .await
            .unwrap();
        let ToolCallOutcome::Result(offset) = offset else {
            panic!()
        };
        assert!(
            offset.starts_with(&format!("{} (lines 3-4 of 10):", path.display())),
            "offset+limit clamp unchanged: {offset:?}"
        );
        assert!(offset.contains("line03") && offset.contains("line04"));

        let grep =
            super::run_read_file(path.to_str().unwrap(), None, None, Some("line0[12]"), None)
                .await
                .unwrap();
        let ToolCallOutcome::Result(grep) = grep else {
            panic!()
        };
        assert!(
            grep.starts_with(&format!("{} (2 matching lines", path.display())),
            "the grep label is unchanged: {grep:?}"
        );
    }

    /// The in-process PTY host the transport tests drive, the same fixture
    /// `shell_exec.rs` and `bg_shell.rs` hold their own copies of: a real bash
    /// session serving the id's socket, per-id instance lock held so the shell
    /// reads as live. `spawn_detached_host` re-invokes `current_exe()`, which
    /// under a test binary is not the daemon.
    async fn spawn_in_process_host(id: crate::shell::ShellId) -> anyhow::Result<()> {
        use anyhow::Context as _;
        use std::sync::Arc;

        let dir = tempfile::tempdir().context("tempdir for cast")?;
        let cast = dir.path().join(format!("{}.cast", id.as_str()));
        let backend = crate::shell::PtyBackend::spawn("bash", &[], 24, 80, 100, &cast, 0)
            .context("spawning bash")?;
        let lock =
            crate::daemon::instance::InstanceLock::acquire(&crate::shell::lock_path(id.as_str()))
                .context("holding the per-id instance lock")?;
        let socket = crate::shell::socket_path(id.as_str());
        let listener = crate::shell::bind(&socket)
            .await
            .context("binding the in-process host's socket")?;
        tokio::spawn(crate::shell::serve(listener, Arc::clone(&backend)));
        std::mem::forget(lock);
        std::mem::forget(dir);
        Ok(())
    }

    /// A test `HOME` with `[hosts.web01]` configured, so the transport's
    /// `host_entry_or_bare` resolves against something real.
    fn remote_read_test_home() -> (crate::TestHomeGuard, tempfile::TempDir) {
        let guard = crate::test_home_guard();
        let dir = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("HOME", dir.path());
        }
        crate::config::Config::ensure_dirs().ok();
        std::fs::write(
            dir.path().join(".daemoneye/etc/config.toml"),
            "[models.default]\nprovider = \"ollama\"\n\n[hosts.web01]\n\
             ssh = \"matt@web01.lan\"\n",
        )
        .unwrap();
        (guard, dir)
    }

    /// Two reads on the same host land in **one** shell, not two: the second
    /// call reuses the shell the first created, which is the whole reason the
    /// transport goes through `shell_for_owner`.
    #[tokio::test]
    async fn the_read_transport_reuses_an_idle_shell_on_the_host() {
        let (_home, _dir) = remote_read_test_home();
        let reg = crate::shell::ShellRegistry::default();

        let first = super::super::run_in_host_shell(
            &reg,
            crate::shell::Owner::Chat("read-file".into()),
            crate::host::HostId::Named("web01".to_string()),
            "echo TRANSPORT_ONE",
            20,
            spawn_in_process_host,
        )
        .await
        .expect("the first remote read runs");
        assert!(
            first.contains("TRANSPORT_ONE"),
            "the command really ran in the shell: {first:?}"
        );

        let held: Vec<String> = crate::shell::with_shells(&reg, |store| {
            let mut ids: Vec<String> = store.values().map(|r| r.id.as_str().to_string()).collect();
            ids.sort();
            ids
        });
        assert_eq!(held.len(), 1, "one read opens exactly one shell: {held:?}");

        let second = super::super::run_in_host_shell(
            &reg,
            crate::shell::Owner::Chat("read-file".into()),
            crate::host::HostId::Named("web01".to_string()),
            "echo TRANSPORT_TWO",
            20,
            spawn_in_process_host,
        )
        .await
        .expect("the second remote read runs");
        assert!(
            second.contains("TRANSPORT_TWO"),
            "the second command really ran: {second:?}"
        );

        let held_after: Vec<String> = crate::shell::with_shells(&reg, |store| {
            let mut ids: Vec<String> = store.values().map(|r| r.id.as_str().to_string()).collect();
            ids.sort();
            ids
        });
        assert_eq!(
            held_after, held,
            "the second read reused the first read's shell — no second shell was opened"
        );
    }

    /// The negative of the reuse rule: a shell whose foreground the read
    /// transport cannot prove idle is **not** taken. The busy maker here is a
    /// real `sleep 30` typed into the held shell — the same state an
    /// interactive command leaves behind — so the read must get a different
    /// (fresh) one rather than type its `sed` into the sleeping program.
    #[tokio::test]
    async fn the_read_transport_never_opens_an_interactive_shell() {
        let (_home, _dir) = remote_read_test_home();
        let reg = crate::shell::ShellRegistry::default();
        let owner = crate::shell::Owner::Chat("read-file".into());
        let host = crate::host::HostId::Named("web01".to_string());

        let first = super::super::run_in_host_shell(
            &reg,
            owner.clone(),
            host.clone(),
            "echo FIRST_READ",
            20,
            spawn_in_process_host,
        )
        .await
        .expect("the first read runs");
        assert!(first.contains("FIRST_READ"), "sanity: {first:?}");

        let held: Vec<String> = crate::shell::with_shells(&reg, |store| {
            store.values().map(|r| r.id.as_str().to_string()).collect()
        });
        for id in &held {
            let id = crate::shell::ShellId::parse(id).expect("a held id parses");
            let socket = crate::shell::socket_path(id.as_str());
            crate::shell::send_input(&socket, b"sleep 30\n", std::time::Duration::from_secs(5))
                .await
                .expect("the held shell accepts the busy maker");
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                let fg =
                    crate::shell::probe_foreground(&reg, &id, crate::shell::PROBE_TIMEOUT).await;
                if !matches!(fg, crate::shell::Foreground::Prompt) {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "the held shell never read busy: {fg:?}"
                );
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }

        let second = super::super::run_in_host_shell(
            &reg,
            owner,
            host,
            "echo SECOND_READ",
            20,
            spawn_in_process_host,
        )
        .await
        .expect("the read gets a shell of its own");
        assert!(
            second.contains("SECOND_READ"),
            "the busy shell's owner keeps it: {second:?}"
        );

        let held_after: std::collections::BTreeSet<String> =
            crate::shell::with_shells(&reg, |store| {
                store
                    .values()
                    .map(|r| r.id.as_str().to_string())
                    .collect::<std::collections::BTreeSet<_>>()
            });
        assert!(
            held_after.len() > held.len(),
            "a fresh shell was opened for the read, not the busy one: {held_after:?}"
        );

        for id in &held {
            let socket = crate::shell::socket_path(id.as_str());
            crate::shell::send_input(&socket, b"\x03", std::time::Duration::from_secs(5))
                .await
                .expect("the held shell accepts the cancel");
        }
    }

    /// The remote read's result names the host it came from, so a model
    /// reading two files from two machines can tell them apart.
    #[tokio::test]
    async fn a_remote_read_labels_the_host() {
        let (_home, dir) = remote_read_test_home();
        let reg = crate::shell::ShellRegistry::default();

        let out = super::super::run_in_host_shell(
            &reg,
            crate::shell::Owner::Chat("read-file".into()),
            crate::host::HostId::Named("web01".to_string()),
            "echo '__DE_S__'; echo 'alpha-remote-content'; echo '__DE_E__'; echo '__DE_DONE__'",
            20,
            spawn_in_process_host,
        )
        .await
        .expect("the marked command runs");
        let extracted =
            super::extract_marked(&out, "__DE_S__", "__DE_E__").unwrap_or_else(|| out.clone());
        let probe_path = dir.path().join("probe.txt");
        let labelled = super::format_remote_read_result(
            probe_path.to_str().unwrap(),
            "web01",
            1,
            200,
            None,
            &extracted,
        );
        let ToolCallOutcome::Result(s) = labelled else {
            panic!()
        };
        assert!(
            s.contains("web01"),
            "the host name must appear in the result: {s:?}"
        );
        assert!(
            s.contains("alpha-remote-content"),
            "the extracted body is what is shown: {s:?}"
        );
        assert!(
            s.starts_with(&format!(
                "{} on host web01 (lines 1-200):",
                probe_path.display()
            )),
            "the label puts the host before the body: {s:?}"
        );
    }

    /// The retirement: `target_pane` is gone from `read_file`'s schema. This
    /// asserts against the `ToolDef`, not a grep, so the test fails if the
    /// parameter comes back under any spelling.
    #[test]
    fn read_file_no_longer_accepts_target_pane() {
        let def = crate::ai::tools::TOOLS
            .iter()
            .find(|t| t.name == "read_file")
            .unwrap_or_else(|| panic!("read_file is in TOOLS"));
        let param_names: Vec<&str> = def.params.iter().map(|p| p.name).collect();
        assert!(
            !param_names.contains(&"target_pane"),
            "read_file must not carry target_pane any more: {param_names:?}"
        );
        assert!(
            param_names.contains(&"host"),
            "read_file carries host in its place: {param_names:?}"
        );
    }

    /// The live check (M21 phase-07a Task 7), in the style of
    /// `run_terminal_command_on_pinky_end_to_end`: the transport reads a file
    /// that really exists on `pinky`, over the real ssh argv, and the read
    /// reuses the shell the first read opened. Same preconditions as that
    /// test: the live host `pinky` reachable over ssh with key auth.
    #[tokio::test]
    #[ignore = "requires the configured host `pinky` reachable over ssh"]
    async fn read_file_on_pinky_end_to_end() {
        let (_home, dir) = {
            let g = crate::test_home_guard();
            let dir = tempfile::tempdir().unwrap();
            unsafe { std::env::set_var("HOME", dir.path()) }
            crate::config::Config::ensure_dirs().ok();
            (g, dir)
        };
        std::fs::write(
            dir.path().join(".daemoneye/etc/config.toml"),
            "[models.default]\nprovider = \"ollama\"\n\n[hosts.pinky]\nssh = \"pinky\"\n",
        )
        .expect("write config");
        crate::config::ssh_dir();

        let reg = crate::shell::ShellRegistry::default();
        let owner = crate::shell::Owner::Chat("read-file-e2e".into());

        // The transport under the tool's own spawn step: the real ssh argv,
        // in-process, exactly as `spawn_pinky_ssh` does for the foreground
        // tool's live check.
        let result = super::super::run_in_host_shell(
            &reg,
            owner.clone(),
            crate::host::HostId::Named("pinky".to_string()),
            &super::build_remote_read_cmd("/etc/hostname", 1, 200, None),
            60,
            |id| std::boxed::Box::pin(crate::daemon::executor::knowledge::spawn_pinky_ssh(id)),
        )
        .await
        .expect("the pinky read resolves");
        let extracted = super::extract_marked(&result, "__DE_S__", "__DE_E__")
            .unwrap_or_else(|| result.clone());
        let labelled =
            super::format_remote_read_result("/etc/hostname", "pinky", 1, 200, None, &extracted);
        let ToolCallOutcome::Result(text) = labelled else {
            panic!()
        };
        assert!(
            text.contains("on host pinky"),
            "the result names the host it came from: {text:?}"
        );
        assert!(
            !text.trim_end().ends_with("(lines 1-200):\n"),
            "a real remote file carries content: {text:?}"
        );
        let hostname = extracted.trim().to_string();
        assert!(
            !hostname.is_empty(),
            "/etc/hostname on pinky is non-empty: {extracted:?}"
        );
        println!("PASTE MATCH >>>\nread_file_on_pinky hostname={hostname:?}\n<<<");

        let _ = crate::daemon::executor::knowledge::close_shell_with(
            &reg,
            crate::shell::registry_lookup(&owner, |id| id.to_string())
                .unwrap_or_default()
                .as_str(),
            &owner,
            std::time::Duration::from_millis(3000),
        )
        .await;
        let cp = crate::config::ssh_dir();
        let _ = std::process::Command::new("ssh")
            .args([
                "-O",
                "exit",
                "-o",
                &format!("ControlPath={}/%C", cp.display()),
                "pinky",
            ])
            .status();
        let _ = (_home, dir);
    }
}

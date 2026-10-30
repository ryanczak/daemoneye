"""M32 phase 16: the remote edit and create read their result through
remote_write_outcome (M32 FINDINGS #10), and wrap_words counts leading
whitespace in chars (M32 FINDINGS #7)."""

def edit(path, pairs):
    s = open(path).read()
    for old, new, n in pairs:
        assert s.count(old) == n, (path, old[:70], s.count(old))
        s = s.replace(old, new)
    open(path, 'w').write(s)

def swap_loop(s, opener, indent, verb, ok_event_json, success, err_word, must_hold):
    """Replace the `for line in <x>.lines().rev() { ... }` block that starts
    with `opener` by a match on remote_write_outcome. The block ends at the
    first line that is exactly `indent + '}'` after the opener."""
    start = s.index(opener)
    assert s.count(opener) == 1, opener
    end_marker = "\n" + indent + "}\n"
    end = s.index(end_marker, start) + len(end_marker)
    block = s[start:end]
    for needle in must_hold:
        assert needle in block, (opener, needle)
    var = opener.split("for line in ")[1].split(".lines()")[0]
    i2 = indent + "    "
    i3 = i2 + "    "
    new = (
        f"match super::remote_write_outcome(&{var}) {{\n"
        f"{i2}Some(Ok(_)) => {{\n"
        f"{i3}crate::daemon::stats::finish_command(cmd_id, 0);\n"
        f"{i3}crate::daemon::utils::log_event(\n"
        f"{i3}    \"{verb}\",\n"
        f"{i3}    {ok_event_json},\n"
        f"{i3});\n"
        f"{i3}return Ok(ToolCallOutcome::Result(format!(\"{success} {{path}} on host {{name}}.\")));\n"
        f"{i2}}}\n"
        f"{i2}Some(Err(line)) => {{\n"
        f"{i3}crate::daemon::stats::finish_command(cmd_id, 1);\n"
        f"{i3}return Ok(ToolCallOutcome::Result(format!(\"Error {err_word} {{path}}: {{line}}\")));\n"
        f"{i2}}}\n"
        f"{i2}None => {{}}\n"
        f"{indent}}}\n"
    )
    return s[:start] + new + s[end:]

# ---- #10: the remote edit, the sudo edit and the remote create ----
OPS = 'src/daemon/executor/file_ops/ops.rs'
s = open(OPS).read()
assert s.count('line.contains("DE_OK:")') == 3
s = swap_loop(
    s, "for line in text.lines().rev() {", " " * 16, "file_edit",
    'serde_json::json!({ "session": session_id.unwrap_or("-"), "path": path, "remote_host": name, "sudo": true })',
    "Edited", "editing", ['"sudo": true', 'Error editing'])
s = swap_loop(
    s, "for line in snap.lines().rev() {\n                if line.contains(\"DE_OK:\") {\n                    crate::daemon::stats::finish_command(cmd_id, 0);\n                    crate::daemon::utils::log_event(\n                        \"file_edit\",",
    " " * 12, "file_edit",
    'serde_json::json!({ "session": session_id.unwrap_or("-"), "path": path, "remote_host": name })',
    "Edited", "editing", ['Error editing'])
s = swap_loop(
    s, "for line in snap.lines().rev() {", " " * 12, "file_create",
    'serde_json::json!({ "session": session_id.unwrap_or("-"), "path": path, "remote_host": name })',
    "Created", "creating", ['"file_create"', 'Error creating'])
assert s.count('line.contains("DE_OK:")') == 0
assert s.count('super::remote_write_outcome(&') == 3
open(OPS, 'w').write(s)
print("the remote edit, sudo edit and create read remote_write_outcome")

# ---- #10 tests: a failure line naming a DE_OK: path is a failure ----
edit(OPS, [
    ('''    /// Test 7, the § Current state 3 negative: a remote write's success
''', '''    /// M32 #10: a create that fails on a path holding `DE_OK:` reports the
    /// failure. The python branch prints `DE_ERROR: file already exists:
    /// <path>`, and a bare `contains("DE_OK:")` scan read that as success.
    #[tokio::test]
    async fn a_failed_create_on_a_sentinel_path_is_an_error() {
        let (_home, dir) = remote_write_test_home();
        let probe = dir.path().join("DE_OK:exists.txt");
        std::fs::write(&probe, "already here").unwrap();
        let (mut tx, mut rx, _prompts) = auto_approve_channel().await.unwrap();
        let outcome = run_create_with_spawn(
            "t10a",
            probe.to_str().unwrap(),
            Some("new content\\n"),
            &crate::host::HostId::Named("web01".to_string()),
            Some("sess-1"),
            &mut tx,
            &mut rx,
            Box::new(|id| Box::pin(spawn_in_process_host(id))),
        )
        .await
        .unwrap_or_else(|e| panic!("run_create must not error: {e}"));
        let ToolCallOutcome::Result(s) = outcome else {
            panic!("expected a Result outcome")
        };
        assert!(
            s.starts_with("Error creating") && s.contains("file already exists"),
            "the failed create is reported as a failure: {s:?}"
        );
        assert_eq!(std::fs::read_to_string(&probe).unwrap(), "already here");
    }

    /// M32 #10: an edit that fails on a path holding `DE_OK:` reports the
    /// failure, not `Edited`.
    #[tokio::test]
    async fn a_failed_edit_on_a_sentinel_path_is_an_error() {
        let (_home, dir) = remote_write_test_home();
        let probe = dir.path().join("DE_OK:probe.txt");
        std::fs::write(&probe, "nothing to match").unwrap();
        let (mut tx, mut rx, _prompts) = auto_approve_channel().await.unwrap();
        let outcome = run_edit_with_spawn(
            RunEditArgs {
                id: "t10b",
                path: probe.to_str().unwrap(),
                old_string: "an absent string",
                new_string: "replacement",
                host: &crate::host::HostId::Named("web01".to_string()),
                sudo: false,
            },
            Some("sess-1"),
            &mut tx,
            &mut rx,
            Box::new(|id| Box::pin(spawn_in_process_host(id))),
        )
        .await
        .unwrap_or_else(|e| panic!("run_edit must not error: {e}"));
        let ToolCallOutcome::Result(s) = outcome else {
            panic!("expected a Result outcome")
        };
        assert!(
            s.starts_with("Error editing") && s.contains("old_string not found"),
            "the failed edit is reported as a failure: {s:?}"
        );
        assert_eq!(std::fs::read_to_string(&probe).unwrap(), "nothing to match");
    }

    /// Test 7, the § Current state 3 negative: a remote write's success
''', 1),
])
print("two tests pin a failure on a sentinel path")

# ---- #7: leading whitespace is counted in chars ----
edit('src/cli/viewer_layout.rs', [
    ('''        let mut space_remaining: usize = width.saturating_sub(leading_len);
''', '''        // `leading_len` is bytes, for `split_at`; the row budget is chars.
        let mut space_remaining: usize = width.saturating_sub(leading.chars().count());
''', 1),
])
edit('src/cli/viewer.rs', [
    ('''    #[test]
    fn output_rows_keep_hard_wrap() {
''', '''    /// M32 #7: a full-width space (3 bytes, 1 char) leaves 9 chars on a
    /// 10-wide row, so `abcd efgh` still fits on the first row.
    #[test]
    fn wrap_words_counts_leading_whitespace_in_chars() {
        let text = "\\u{3000}abcd efgh";
        assert_eq!(wrap_words(text, 10), vec![text.to_string()]);
    }

    #[test]
    fn output_rows_keep_hard_wrap() {
''', 1),
])
print("wrap_words counts leading whitespace in chars")

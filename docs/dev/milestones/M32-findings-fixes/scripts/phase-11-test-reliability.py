# ---- the HOME-lock assertion in log_event, under cfg(test) ----
L = 'src/lib.rs'
s = open(L).read()
a = '/// Acquire [`TEST_HOME_LOCK`], recovering if a previous holder panicked.\n'
assert s.count(a) == 1
s = s.replace(a, '''#[cfg(test)]
thread_local! {
    /// Whether this thread holds a [`TestHomeGuard`]. `log_event` asserts it
    /// under `cfg(test)`: a lib test that writes an event without the lock
    /// writes into whichever `HOME` another test has set, and that test — not
    /// the writer — fails (M32 #11).
    pub(crate) static HOLDS_HOME_GUARD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

''' + a)
g = 'pub fn test_home_guard() -> TestHomeGuard {\n'
assert s.count(g) == 1
s = s.replace(g, g + '    #[cfg(test)]\n    HOLDS_HOME_GUARD.with(|c| c.set(true));\n')
b = 'impl Drop for TestHomeGuard {\n    fn drop(&mut self) {\n'
assert s.count(b) == 1
s = s.replace(b, b + '        #[cfg(test)]\n        HOLDS_HOME_GUARD.with(|c| c.set(false));\n')
open(L, 'w').write(s)

E = 'src/daemon/utils/event_log.rs'
e = open(E).read()
c = 'pub fn log_event(event: &str, mut fields: serde_json::Value) {\n    use std::io::Write;\n'
assert e.count(c) == 1
e = e.replace(c, c + '''    #[cfg(test)]
    assert!(
        crate::HOLDS_HOME_GUARD.with(|c| c.get()),
        "log_event({event:?}) from a test that does not hold the HOME lock: take \\
         crate::test_home() first, or the event lands in another test's HOME (M32 #11)"
    );
''')
open(E, 'w').write(e)
print('log_event asserts the HOME lock under cfg(test)')

# ---- the four tests that wrote events without the lock ----
GUARD = '        let (_home, _dir) = crate::test_home();\n'
for path, sig in [
    ('src/daemon/executor/foreground.rs', '    async fn ghost_turn_without_a_pty_shell_is_refused() {\n'),
    ('src/daemon/executor/foreground.rs', '    async fn chat_command_without_session_is_refused() {\n'),
    ('src/daemon/mod.rs', '    async fn supervise_restarts_on_panic() {\n'),
    ('src/daemon/executor/file_ops/ops.rs', '    async fn the_local_write_path_is_unchanged() {\n'),
]:
    src = open(path).read()
    assert src.count(sig) == 1, (path, sig)
    open(path, 'w').write(src.replace(sig, sig + GUARD, 1))
print('four tests take crate::test_home()')

# ---- #65: one read per poll, the tail under test ----
O = 'src/daemon/executor/knowledge/shell_observer_tests.rs'
src = open(O).read()
start = src.index('#[tokio::test]\nasync fn read_shell_screen_lines_tails_the_visible_rows() {')
end = src.index('#[tokio::test]\nasync fn read_shell_screen_pattern_filters_rows() {')
assert 0 < start < end
NEW = '''#[tokio::test]
async fn read_shell_screen_lines_tails_the_visible_rows() {
    let (_home, _dir) = crate::test_home();
    let reg = ShellRegistry::default();
    let id = open_screen_shell(&reg).await;
    type_into(&id, b"printf 'row-a\\\\nrow-b\\\\nrow-c\\\\n'\\n").await;
    // One read per poll, with the tail under test. Once the prompt is drawn
    // under `row-c`, the two-row tail is `row-c` and the prompt row. Reading
    // the whole screen first and the tail second let a late prompt redraw
    // change the row count between the two reads (M31 #65).
    let out = screen_until(&reg, &id, Some(2), None, |o| {
        body_rows(o).first().is_some_and(|l| l.trim() == "row-c")
    })
    .await;
    let tailed = body_rows(&out);
    assert_eq!(tailed.len(), 2, "`lines: 2` returns two rows: {out}");
    assert_eq!(tailed[0].trim(), "row-c", "the tail starts at row-c: {out}");
    assert!(
        !tailed.iter().any(|l| l.trim() == "row-a"),
        "rows above the tail are dropped: {out}"
    );
}
'''
open(O, 'w').write(src[:start] + NEW + src[end:])
print('read_shell_screen_lines_tails_the_visible_rows reads once per poll')

# ---- #83: the approved path and the host refusal through execute_tool_call ----
D = 'src/daemon/executor/dispatch_foreground.rs'
src = open(D).read()
assert src.count('mod approved_path_tests') == 0
src = src.rstrip('\n') + '''

#[cfg(test)]
mod approved_path_tests {
    use super::*;
    use tokio::io::AsyncReadExt as _;

    /// A real in-process PTY host for `id`: bash on a PTY, serving the id's
    /// socket under the test `HOME`. `spawn_detached_host` cannot run under a
    /// test binary, so the session's shell is opened here first and the
    /// approved run reuses it.
    async fn spawn_in_process_host(id: crate::shell::ShellId) -> anyhow::Result<()> {
        use anyhow::Context as _;
        let dir = tempfile::tempdir().context("tempdir for cast")?;
        let cast = dir.path().join(format!("{}.cast", id.as_str()));
        let backend = crate::shell::PtyBackend::spawn("bash", &[], 24, 80, 100, &cast, 0)
            .context("spawning bash")?;
        let lock =
            crate::daemon::instance::InstanceLock::acquire(&crate::shell::lock_path(id.as_str()))
                .context("holding the per-id instance lock")?;
        let listener = crate::shell::bind(&crate::shell::socket_path(id.as_str()))
            .await
            .context("binding the in-process host's socket")?;
        tokio::spawn(crate::shell::serve(listener, Arc::clone(&backend)));
        std::mem::forget(lock);
        std::mem::forget(dir);
        Ok(())
    }

    /// One `run_terminal_command` call through `execute_tool_call` for
    /// session `sid`, with `reply` as everything the client sends back.
    /// Returns the frames the daemon wrote, in order.
    async fn drive(sid: &str, call: &PendingCall, reply: &str) -> Vec<Response> {
        let store = SessionStore::new();
        let (mut daemon_tx, mut client_rx) = tokio::io::duplex(64 * 1024);
        let mut daemon_rx = std::io::Cursor::new(reply.as_bytes().to_vec());
        execute_tool_call(
            call,
            &mut daemon_tx,
            &mut daemon_rx,
            SessionCtx {
                session_id: Some(sid),
                session_name: sid,
                chat_pane: None,
                sessions: &store,
            },
            &Arc::new(SessionCache::new("home")),
            &Arc::new(ScheduleStore::new_empty()),
        )
        .await
        .expect("run_terminal_command must not error");
        drop(daemon_tx);
        let mut raw = Vec::new();
        client_rx
            .read_to_end(&mut raw)
            .await
            .expect("reading the client half of the pipe");
        String::from_utf8(raw)
            .expect("the daemon writes UTF-8 frames")
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str::<Response>(line).expect("every frame is a Response"))
            .collect()
    }

    fn foreground_call(cmd: &str, host: Option<&str>) -> PendingCall {
        PendingCall::Foreground {
            id: "fg-83".to_string(),
            thought_signature: None,
            cmd: cmd.to_string(),
            shell: None,
            host: host.map(str::to_string),
            timeout_secs: Some(20),
            interactive: false,
        }
    }

    fn tool_result(frames: &[Response]) -> &str {
        frames
            .iter()
            .find_map(|f| match f {
                Response::ToolResult { output, .. } => Some(output.as_str()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("a ToolResult frame: {frames:?}"))
    }

    /// #83: an approved local call runs in the session's PTY shell. The
    /// frames are the prompt, then a result carrying the command's output
    /// and naming the shell it ran in.
    #[tokio::test]
    async fn an_approved_local_command_runs_in_the_sessions_shell() {
        let (_home, _dir) = crate::test_home();
        let sid = "sess-83-run";
        let id = crate::shell::spawn_for(
            crate::shell::registry(),
            Owner::Chat(sid.to_string()),
            5,
            false,
            |id| Box::pin(spawn_in_process_host(id)),
        )
        .await
        .expect("the session's shell opens");
        let mut reply = serde_json::to_string(&Request::ToolCallResponse {
            id: "fg-83".to_string(),
            approved: true,
            user_message: None,
        })
        .expect("ToolCallResponse serialises");
        reply.push('\\n');
        let frames = drive(sid, &foreground_call("echo DE-M32-83-$((40+3))", None), &reply).await;
        assert!(
            matches!(
                frames.first(),
                Some(Response::ToolCallPrompt {
                    background: false,
                    ..
                })
            ),
            "the first frame asks for approval: {frames:?}"
        );
        let out = tool_result(&frames);
        assert!(
            out.contains("DE-M32-83-43"),
            "the result carries the command's output: {out}"
        );
        assert!(
            out.contains(&format!("[ran in shell {}]", id.as_str())),
            "the result names the shell it ran in: {out}"
        );
    }

    /// #83: a malformed host name is refused before the prompt — no
    /// `ToolCallPrompt` is written, and the refusal names the rule.
    #[tokio::test]
    async fn a_malformed_host_is_refused_without_asking() {
        let (_home, _dir) = crate::test_home();
        let frames = drive("sess-83-host", &foreground_call("true", Some("db 01")), "").await;
        assert!(
            !frames
                .iter()
                .any(|f| matches!(f, Response::ToolCallPrompt { .. })),
            "a name that can never be a host is not put to the user: {frames:?}"
        );
        let out = tool_result(&frames);
        assert!(
            out.starts_with("Error: host name \\"db 01\\""),
            "the refusal names the host: {out}"
        );
        assert!(
            out.contains("[A-Za-z0-9._-]"),
            "the refusal names the rule: {out}"
        );
    }
}
'''
open(D, 'w').write(src)
print('two approved-path tests added')

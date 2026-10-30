# Phase 11 — Test reliability

**Status:** todo
**Milestone:** M32 — Findings fixes
**Drafted:** 2026-10-29
**Depends on:** phase-10
**Estimated diff:** ~190 lines
**Tags:** language=rust, kind=test, size=m
**Findings:** M31 `FINDINGS.md` #65, #83; M32 `FINDINGS.md` #11
(#5, #6, #51, #53 and #54 move to phase 11b)

## Goal

Two flakes and one coverage gap in the suite.

1. **M32 #11** — `for_each_streams_across_segments_in_order` and
   `for_each_skips_unparseable_lines` read every event segment under
   `HOME` and have failed four times with an event they did not write.
   The writers are four tests that call `log_event` without holding the
   HOME lock, so their event lands in whichever temp `HOME` another test
   has set at that moment. Found by instrumenting `log_event` and running
   the lib suite once:

   ```
   UNGUARDED log_event event=task_panic  thread=daemon::tests::supervise_restarts_on_panic
   UNGUARDED log_event event=file_create thread=daemon::executor::file_ops::ops::tests::the_local_write_path_is_unchanged
   UNGUARDED log_event event=command     thread=daemon::executor::foreground::tests::ghost_turn_without_a_pty_shell_is_refused
   UNGUARDED log_event event=command     thread=daemon::executor::foreground::tests::chat_command_without_session_is_refused
   ```

   After this phase the four tests take `crate::test_home()`, and
   `log_event` asserts under `cfg(test)` that the calling thread holds the
   lock — so the next unguarded writer fails by name, in its own test,
   instead of as a flake in an event-log test.
2. **M31 #65** — `read_shell_screen_lines_tails_the_visible_rows` reads
   the live screen twice: once to count the rows to tail, once with that
   tail. The header says `shell prompt` as soon as bash is back at its
   prompt, before the prompt row is drawn, so a redraw between the two
   reads changes the count. After this phase the test polls one read, with
   the tail under test (`lines: 2`), until that read shows `row-c` as its
   first row.
3. **M31 #83** — no test drives `run_terminal_command`'s approved path
   through `execute_tool_call`. After this phase two tests do: an approved
   local command runs in the session's PTY shell and the result carries
   its output, and a malformed host name is refused before any prompt.

The suite goes from **2607** to **exactly 2609**: two tests added, none
removed.

## Architecture references

- `src/lib.rs` § `test_home_guard` / `TestHomeGuard`: the lock the new
  assertion reads.
- `src/daemon/executor/dispatch_foreground.rs`: the arm the #83 tests
  drive; `src/daemon/executor/mod.rs` § the `tests` module, whose
  `get_shell_context_without_chat_pane_is_shells_only` is the existing
  `execute_tool_call` fixture the new tests follow.
- M32 README, decisions 1 and 3.

## Spec

1. **Run the script below as written, once.** It does the whole change:
   - `src/lib.rs`: a `#[cfg(test)]` thread-local `HOLDS_HOME_GUARD`, set
     by `test_home_guard()` and cleared by `TestHomeGuard::drop`;
   - `src/daemon/utils/event_log.rs`: `log_event` asserts it under
     `cfg(test)`, naming the event and the fix;
   - the four tests above take `crate::test_home()` as their first line;
   - `read_shell_screen_lines_tails_the_visible_rows` is rewritten to one
     read per poll;
   - `dispatch_foreground.rs` gains a `mod approved_path_tests` with its
     own in-process PTY host spawn (the same twelve lines
     `server/shells_tests.rs` has — `spawn_detached_host` cannot run under
     a test binary) and the two #83 tests.

   It was dry-run on a scratch worktree of `a46472b6`: fmt and clippy
   clean, the eight tests it touches green. Do not hand-edit anything it
   touches. Do not `sed -i` a source file. It is a Python heredoc inside a
   shell script, so copy it **without adding indentation**. It asserts
   its anchors and prints four lines, the last
   `two approved-path tests added`. If an assert fires, stop and report
   it in the Update Log.
2. **Log it at once.** Append an `### Update — <date> (cuts done)` entry
   quoting the script's output, with an edit tool.
3. **Findings** go in their own `### Findings — <date>` entry. Run
   exactly `git diff --stat`, write what it shows in one or two lines, or
   write `none`. Open no file.
4. **Run the End-to-end block** and paste its output into an
   `### Update — <date> (end-to-end verification)` entry, as a fenced
   block of the output itself, not a description of it.
5. **Commit** as `test:`.

### Why the red run is in the dry run, not yours

The assertion is the regression gate for #11, and it was run red before
the four guards went in: with the assertion alone, the four tests fail,
each naming its event (`log_event("command") from a test that does not
hold the HOME lock …`), `0 passed; 4 failed`. The script applies the
assertion and the guards together, so your run is green. #65 is a race
the fix removes structurally; 25 runs of the old test passed alone, and
25 of the new one. The #83 run test was mutation-checked: dropping the
`[ran in shell …]` line from `with_shell_line` fails it on
`the result names the shell it ran in`. Do not repeat any of this.

### The script

```bash
python3 - <<'PY'
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
PY
cargo fmt --all
```

## Acceptance criteria

- [ ] The End-to-end block prints `assert=1 guarded=4 approved_tests=2
      single_read=1` (today: `assert=0 guarded=0 approved_tests=0
      single_read=0`).
- [ ] `cargo test` reports **exactly** `passed=2609 failed=0` (today
      2607).
- [ ] Preservation: `tests/hygiene.rs` and `cargo test --test bug_tracker`
      pass; `tests/hygiene_allowlist.txt` is unchanged. fmt, build and
      clippy are clean.
- [ ] `grep -c '^### Findings — '` on this doc prints `1`, and the
      end-to-end entry holds the fenced output.

## End-to-end verification

```sh
bash <<'E2E'
echo "assert=$(grep -c 'HOLDS_HOME_GUARD.with(|c| c.get())' src/daemon/utils/event_log.rs) guarded=$(grep -A1 -E 'async fn (supervise_restarts_on_panic|the_local_write_path_is_unchanged|ghost_turn_without_a_pty_shell_is_refused|chat_command_without_session_is_refused)\(\)' src/daemon/mod.rs src/daemon/executor/foreground.rs src/daemon/executor/file_ops/ops.rs | grep -c 'crate::test_home()') approved_tests=$(grep -cE 'async fn (an_approved_local_command_runs_in_the_sessions_shell|a_malformed_host_is_refused_without_asking)\(\)' src/daemon/executor/dispatch_foreground.rs) single_read=$(grep -c 'screen_until(&reg, &id, Some(2), None' src/daemon/executor/knowledge/shell_observer_tests.rs)"
git diff --quiet HEAD -- tests/hygiene_allowlist.txt && echo allowlist=unchanged
cargo test --test hygiene 2>&1 | grep -E '^test result'
cargo test --test bug_tracker 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

Expected: `assert=1 guarded=4 approved_tests=2 single_read=1`,
`allowlist=unchanged`, hygiene `ok. 16 passed`, bug tracker
`ok. 6 passed`, clippy clean, `fmt_exit=0`, `passed=2609 failed=0`. If
the suite stops early on a failure, paste that output, run the block once
more and paste both.

## Authorizations

- Edit the seven files the script names, through the script only:
  `src/lib.rs`, `src/daemon/utils/event_log.rs`, `src/daemon/mod.rs`,
  `src/daemon/executor/foreground.rs`,
  `src/daemon/executor/file_ops/ops.rs`,
  `src/daemon/executor/knowledge/shell_observer_tests.rs`,
  `src/daemon/executor/dispatch_foreground.rs`.

## Out of scope

- #5, #6, #51, #53, #54: phase 11b (test-tidies).
- The event-log tests' own `with_test_home` helper (a closure form, not
  the phase-10 fixture's shape).
- Any test that the new assertion does **not** fail. The dry run ran the
  whole lib suite under it: exactly the four tests above fail, no other.

## Update Log

<!-- entries appended below this line -->

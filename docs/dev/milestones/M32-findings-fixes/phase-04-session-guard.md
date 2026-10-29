# Phase 04 — Save and load refuse an unknown session

**Status:** todo
**Milestone:** M32 — Findings fixes
**Drafted:** 2026-10-29
**Findings:** M31 `FINDINGS.md` #11 and #12 (#13 is rejected below; #20
moves to phase 04b)

## Goal

`handle_save_session` and `handle_load_session`
(`src/daemon/server/sessions.rs`) look the request's `session_id` up in
the `SessionStore`, and neither one checks that it was found:
- **Save (#12).** An unknown id saves an empty session (no messages, 0
  turns, model `default`) under the requested name, then replies
  `SessionSaved`.
- **Load (#11).** An unknown id makes the write-back a no-op, yet the
  handler replies `SessionLoaded`, so the client believes it resumed.

After this phase, both handlers start with the same check. A
`session_id` the store does not hold gets `Response::Error` naming the
id. Nothing is saved or loaded, and the handler returns.

**#13 is rejected, not fixed.** The banner reads `N turns | M message(s)
loaded`, the saved session's full turn count beside the messages loaded
under `load_recent_turns`. The two numbers measure different things, and
the banner labels each. Nothing is wrong.

## Architecture references

- M32 README, decisions 1 and 6.

## Spec

1. **Add the two tests below, and run them red.** Append the
   `#[cfg(test)] mod tests` block to the end of `sessions.rs`, exactly
   as written, and run `cargo test --lib server::sessions::tests`. Both
   tests must fail. Save must fail with `expected Response::Error, got
   SessionSaved { name: "saved-name" }`, and load with `expected
   Response::Error, got SessionLoaded { … }`. Paste that output into the
   Update Log.
2. **The guard.** Add one private helper to `sessions.rs`:
   `fn unknown_session_refusal(sessions: &SessionStore, session_id: &str) -> Option<String>`.
   It returns `Some(format!("unknown session '{session_id}': nothing was saved or loaded"))`
   when `with_sessions(sessions, |store| store.contains_key(session_id))`
   is false, and `None` otherwise. Make it the **first** statement of
   both handlers. On `Some(msg)`, send `Response::Error(msg)` with
   `send_response_split` and return `Ok(())`. In `handle_load_session`
   the guard runs before the unsaved-changes check. Nothing else in
   either handler changes. Run the tests again; both must pass.
3. **Findings** go in their own `### Findings — <date>` entry, from
   `sessions.rs` only, within five tool calls, or the single line
   `none`.
4. **Commit** as `fix:`.

**Do not look further.** The tests drive the handlers directly, with a
`Vec<u8>` as `tx` and an empty `SessionStore::new()`. Do not read the
server dispatch, the client or `session_store.rs`. The architect
dry-ran this exact change on `HEAD`: red at step 1, green at step 2,
clippy and fmt clean, and the full suite at `passed=2596 failed=0`.

```rust
#[cfg(test)]
mod tests {
    use super::{handle_load_session, handle_save_session};
    use crate::config::Config;
    use crate::daemon::session::SessionStore;
    use crate::ipc::Response;

    /// Points `HOME` at a fresh temp dir for the guard's lifetime; the
    /// returned guard restores it.
    fn temp_home() -> (crate::TestHomeGuard, tempfile::TempDir) {
        let lock = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };
        (lock, tmp)
    }

    /// The single response frame a handler wrote to `tx`.
    fn reply(tx: &[u8]) -> Response {
        let line = std::str::from_utf8(tx).unwrap().lines().next().unwrap();
        serde_json::from_str(line).unwrap()
    }

    #[tokio::test]
    async fn save_of_an_unknown_session_is_an_error_and_saves_nothing() {
        let (_lock, _tmp) = temp_home();
        let sessions = SessionStore::new();
        let mut tx: Vec<u8> = Vec::new();

        handle_save_session(
            &mut tx,
            &sessions,
            "no-such-session".to_string(),
            "saved-name".to_string(),
            String::new(),
            false,
        )
        .await
        .unwrap();

        match reply(&tx) {
            Response::Error(msg) => assert!(
                msg.contains("no-such-session"),
                "the error must name the unknown session: {msg}"
            ),
            other => panic!("expected Response::Error, got {other:?}"),
        }
        assert!(
            !crate::session_store::session_exists("saved-name"),
            "nothing may be saved for an unknown session"
        );
    }

    #[tokio::test]
    async fn load_into_an_unknown_session_is_an_error() {
        let (_lock, _tmp) = temp_home();
        crate::session_store::save_session(crate::session_store::SaveSessionArgs {
            name: "saved-name",
            current_saved_name: None,
            description: "",
            messages: &[],
            turn_count: 0,
            model: "default",
            artifacts: &[],
            force: false,
        })
        .unwrap();
        let sessions = SessionStore::new();
        let mut tx: Vec<u8> = Vec::new();

        handle_load_session(
            &mut tx,
            &sessions,
            &Config::default(),
            "no-such-session".to_string(),
            "saved-name".to_string(),
            false,
        )
        .await
        .unwrap();

        match reply(&tx) {
            Response::Error(msg) => assert!(
                msg.contains("no-such-session"),
                "the error must name the unknown session: {msg}"
            ),
            other => panic!("expected Response::Error, got {other:?}"),
        }
    }
}
```

## Acceptance criteria

- [ ] `grep -c 'unknown_session_refusal(sessions, &session_id)' src/daemon/server/sessions.rs`
      prints `2` (today: `0`).
- [ ] `save_of_an_unknown_session_is_an_error_and_saves_nothing` and
      `load_into_an_unknown_session_is_an_error` pass, and the Update
      Log shows both failing at step 1.
- [ ] `grep -c '^### Findings — '` on this doc prints `1`.
- [ ] `cargo test` reports **exactly** `passed=2596 failed=0` (2594 + 2).
      `tests/hygiene.rs` and `cargo test --test bug_tracker` pass. fmt,
      build and clippy are clean.

## End-to-end verification

```sh
bash <<'E2E'
F=src/daemon/server/sessions.rs
echo "guards=$(grep -c 'unknown_session_refusal(sessions, &session_id)' $F)"
cargo test --lib server::sessions::tests 2>&1 | grep -E '^test result'
cargo test --test hygiene 2>&1 | grep -E '^test result'
cargo test --test bug_tracker 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

Expected: `guards=2`, `test result: ok. 2 passed` for the sessions
tests, hygiene `ok. 16 passed`, bug tracker `ok. 6 passed`, clippy
clean, `passed=2596 failed=0`. Paste the output into an
`### Update — <date> (end-to-end verification)` entry. **Known flake:**
`read_shell_screen_lines_tails_the_visible_rows` (M31 #65). If the
block shows `passed=2595 failed=1`, run it once more and paste both
outputs.

## Authorizations

- Edit `src/daemon/server/sessions.rs`. No other source file.

## Out of scope

- #20, where the client rehydrates the transcript from its own
  filesystem: phase 04b.
- The list, delete and rename handlers. They take no `session_id`.

## Update Log

<!-- entries appended below this line -->

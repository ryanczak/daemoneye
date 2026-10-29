# Phase 03 — `recall_context` honours the configured `[limits]`

**Status:** in-progress
**Milestone:** M32 — Findings fixes
**Drafted:** 2026-10-29
**Finding:** M31 `FINDINGS.md` #74

## Goal

The `RecallContext` arm of `dispatch` in
`src/daemon/executor/dispatch_memory.rs` hands
`crate::daemon::context::recall::recall` a hard-coded
`LimitsConfig::default()`. `recall` reads one field from it,
`tool_result_chars`, which caps its output; the default is 16 000. A
user who sets `[limits] tool_result_chars` in `config.toml` therefore
gets every other tool capped at their value and `recall_context` capped
at 16 000.

After this phase, the arm passes the configured `[limits]` to `recall`.
It uses the executor's usual idiom,
`crate::config::Config::load().unwrap_or_default().limits`, so a missing
or unreadable config still means the defaults, as everywhere else in
`src/daemon/executor/`.

## Architecture references

- M32 README, decisions 1 and 6.

## Spec

1. **Extract the arm's body, with no behaviour change.** In
   `src/daemon/executor/dispatch_memory.rs`, move the `RecallContext`
   arm's body into a module-level function
   `fn recall_context(session_id: Option<&str>, args: &RecallArgs) -> String`.
   It still builds `LimitsConfig::default()` at this step. The arm
   builds the `RecallArgs` and returns
   `Ok(ToolCallOutcome::Result(recall_context(session_id, &args)))`.
2. **Add the test, and run it red.** Add the `#[cfg(test)] mod tests`
   below at the end of the file, exactly as written, then run
   `cargo test recall_context_uses_the_configured_tool_result_chars`.
   It must fail with `the configured cap of 200 must truncate a
   4000-char turn: 4021 chars`. Paste that output into the Update Log.
3. **The fix: one line.** In `recall_context`, change
   `crate::config::LimitsConfig::default()` to
   `crate::config::Config::load().unwrap_or_default().limits`. Run the
   test again; it must pass.
4. **Findings** go in their own `### Findings — <date>` entry, from
   `dispatch_memory.rs` only, within five tool calls, or the single line
   `none`.
5. **Commit** as `fix:`.

**Do not look further.** The test below calls `recall_context`
directly, so it needs no `ArtifactCtx`, no `tx` writer and no memory
index. Do not read `recall.rs`, `knowledge/`, `memory/` or the server
tests. The architect dry-ran this exact code on `HEAD`: red at step 2,
green at step 3, clippy and fmt clean, and the full suite at
`passed=2594 failed=0`.

The helper and the test, as they stand after step 3:

```rust
/// `recall_context`'s result text: the session's archived turns through
/// `recall`, capped by the configured `[limits]`.
fn recall_context(
    session_id: Option<&str>,
    args: &crate::daemon::context::recall::RecallArgs,
) -> String {
    let Some(sid) = session_id else {
        return "Error: recall_context requires a session".to_string();
    };
    let limits = crate::config::Config::load().unwrap_or_default().limits;
    match crate::daemon::context::recall::recall(sid, args, &limits) {
        Ok(r) => r,
        Err(e) => format!("Error: {}", e),
    }
}

#[cfg(test)]
mod tests {
    use super::recall_context;
    use crate::ai::Message;
    use crate::daemon::context::recall::RecallArgs;

    #[test]
    fn recall_context_uses_the_configured_tool_result_chars() {
        let _lock = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        std::fs::create_dir_all(crate::config::etc_dir()).unwrap();
        std::fs::write(
            crate::config::etc_dir().join("config.toml"),
            "[limits]\ntool_result_chars = 200\n",
        )
        .unwrap();

        let id = "recall_limits_test";
        let archive = crate::daemon::session::archive_file(id);
        std::fs::create_dir_all(archive.parent().unwrap()).unwrap();
        let msg = Message {
            role: "assistant".to_string(),
            content: "x".repeat(4000),
            tool_calls: None,
            tool_results: None,
            turn: Some(1),
        };
        std::fs::write(&archive, serde_json::to_string(&msg).unwrap() + "\n").unwrap();

        let args = RecallArgs {
            query: None,
            turn_start: Some(1),
            turn_end: Some(1),
            scope: None,
        };
        let out = recall_context(Some(id), &args);
        assert!(
            out.contains("[…truncated — narrow the turn range or refine the query…]"),
            "the configured cap of 200 must truncate a 4000-char turn: {} chars",
            out.chars().count()
        );
        assert!(
            out.chars().count() < 400,
            "output must respect the configured cap, got {} chars",
            out.chars().count()
        );
    }
}
```

## Acceptance criteria

- [ ] `grep -c 'LimitsConfig::default()' src/daemon/executor/dispatch_memory.rs`
      prints `0` (today: `1`).
- [ ] `recall_context_uses_the_configured_tool_result_chars` exists and
      passes, and the Update Log shows its step-2 failure (`4021 chars`).
- [ ] `grep -c '^### Findings — '` on this doc prints `1`.
- [ ] `cargo test` reports **exactly** `passed=2594 failed=0` (2593 + 1).
      `tests/hygiene.rs` and `cargo test --test bug_tracker` pass. fmt,
      build and clippy are clean.

## End-to-end verification

```sh
bash <<'E2E'
F=src/daemon/executor/dispatch_memory.rs
echo "default_limits=$(grep -c 'LimitsConfig::default()' $F)"
cargo test recall_context_uses_the_configured_tool_result_chars 2>&1 | grep -E '^test result: ok\. 1 passed'
cargo test --test hygiene 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

Expected: `default_limits=0`, `test result: ok. 1 passed`, hygiene
`ok. 16 passed`, clippy clean, `passed=2594 failed=0`. Paste the output
into an `### Update — <date> (end-to-end verification)` entry. **Known
flake:** `read_shell_screen_lines_tails_the_visible_rows` (M31 #65). If
the block shows `passed=2593 failed=1`, run it once more and paste both
outputs.

## Authorizations

- Edit `src/daemon/executor/dispatch_memory.rs`. No other source file.

## Out of scope

- `recall` itself, and the other `Config::load().unwrap_or_default()`
  sites.
- The ghost loop's own `RecallContext` handling in
  `src/daemon/ghost/turn.rs`.

## Update Log

<!-- entries appended below this line -->

### Notes for executor — 2026-10-29

The first run (120 turns) wrote nothing and stalled on 120 read-only
calls. The original Spec asked for a test driving the whole `dispatch`.
That meant building an `ArtifactCtx`, finding an `AsyncWrite` writer
and learning the archive format, and the run spent every call looking
those up. The refined Spec avoids all of it: the arm's body becomes
`recall_context`, and the test calls that function directly. The test
is given verbatim and was dry-run red and then green. Start writing at
once. Steps 1–3 are three edits to one file.

### Update — 2026-10-29 (escalation)

**Chosen lever:** refined re-dispatch.
**Rationale:** a `NoProgressStall` with an empty diff, caused by a Spec
that sent the executor to discover a test harness. There is no partial
work to resume, and a first failure caused by the spec does not warrant
a takeover. The refinement replaces the discovery with a dry-run test
seam.


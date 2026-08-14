# Phase 02: sudo cached-credential detection — sentinel-exact prompt detection

**Milestone:** M15 — Chat Reliability & Dialog UX
**Status:** done
**Depends on:** none
**Estimated diff:** ~180 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

When a foreground `sudo` command runs with credentials already cached for the
user's pane tty, DaemonEye still pops the password prompt and blocks the chat
session. The detection loop concludes "password needed" from signals that are
equally true when no password is needed. This phase makes password-prompt
detection **exact**: a per-invocation `SUDO_PROMPT` sentinel that appears in
the pane if — and only if — sudo actually prompts.

## Architecture references

Read before starting:

- `src/daemon/executor/foreground.rs` — the foreground sudo detection loop
  (lines ~408–530), the main surface of this phase.
- `src/daemon/utils/sudo.rs` — sudo helpers; gains the sentinel functions.
- `src/daemon/background/run.rs:161` — the existing static-sentinel pattern
  this phase generalizes.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

**Host facts (verified 2026-08-14 by the architect):** sudo 1.9.17p2; no
`timestamp_type` override in `/etc/sudoers` or `/etc/sudoers.d/`, so the
default **tty-scoped** credential cache applies — each pane tty has its own
timestamp. The user's shell is bash.

**The bug is in the foreground detection loop**
(`src/daemon/executor/foreground.rs:408–530`), which decides whether sudo is
waiting for a password after the command has been injected into the user's
pane. Three defects, each sufficient to produce the false prompt:

1. **`pane_current_command == "sudo"` is true for the whole runtime of the
   wrapped command**, not just while prompting — sudo stays resident as the
   parent of the command it runs. `sudo pacman -Syu` with cached credentials
   shows `sudo` as the current command for minutes.
2. **While `sudo` is current, the pane snapshot is checked with broad
   substrings** (`foreground.rs:471–475`):

   ```rust
   if snap.contains("[sudo]")
       || snap.contains("password")
       || snap.contains("Password")
       || snap.contains("[de-sudo-prompt]")
   {
       result = SudoAuth::Password;
   ```

   The 10-line snapshot routinely contains a **stale** `[sudo] password for
   matt:` from an earlier command, or the word "password" in the echoed
   command line or its output. Combined with defect 1, a cached-credential
   run is misclassified as prompting.
3. **The two-consecutive-polls fallback** (`foreground.rs:498–516`) concludes
   `SudoAuth::Password` for any sudo-wrapped command still running after
   ~200 ms — i.e. for *every* non-trivial command with cached credentials.

The result: `Response::CredentialPrompt` is sent, chat blocks on password
input, exactly the reported bug.

**The exact mechanism already exists for background windows.**
`src/daemon/background/run.rs:161`:

```rust
sentineled_cmd = format!("SUDO_PROMPT='[de-sudo-prompt]' {cmd}");
```

`SUDO_PROMPT` makes sudo print the given string instead of the default
prompt, so its appearance in the pane is a direct signal. The foreground
path never sets it. This phase adds it — with a **per-invocation nonce** so
a stale sentinel from a previous command can never match the current one.

**Injection site** (`foreground.rs:353–355, 390–394`): `hook_idx` is already
a unique per-invocation counter, and `send_cmd` is the final command string
sent to the pane:

```rust
let hook_idx = FG_HOOK_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
...
let target_str_keys = target_str.to_string();
let send_cmd_keys = send_cmd.to_string();
let send_keys_res = tmux::off_runtime("send-keys", move || {
    tmux::send_keys(&target_str_keys, &send_cmd_keys)
})
```

**Credential injection helper** (`src/daemon/utils/sudo.rs:65`) polls with
the same broad substrings and must move to the sentinel too:

```rust
pub async fn wait_for_sudo_prompt_and_inject(pane_id: &str, credential: &str) -> bool {
    ...
        if snap.contains("[de-sudo-prompt]")
            || snap.contains("[sudo]")
            || snap.contains("password")
            || snap.contains("Password")
        {
```

Its two call sites: `foreground.rs:621` and `background/run.rs:226`.

## Spec

### 1. Sentinel helpers — in `src/daemon/utils/sudo.rs`

Add two public functions (above the existing `wait_for_sudo_prompt_and_inject`):

```rust
/// Per-invocation sudo prompt sentinel. The closing bracket makes the match
/// exact: `[de-sudo-prompt-3]` is not a substring of `[de-sudo-prompt-33]`.
pub fn sudo_sentinel(idx: usize) -> String {
    format!("[de-sudo-prompt-{idx}]")
}

/// Prefix `cmd` so sudo prints `sentinel` instead of its default password
/// prompt. Same shape as the background-window form in
/// `background/run.rs` (`SUDO_PROMPT='[de-sudo-prompt]' {cmd}`).
pub fn with_sudo_sentinel(cmd: &str, sentinel: &str) -> String {
    format!("SUDO_PROMPT='{sentinel}' {cmd}")
}
```

### 2. Prefix foreground sudo commands — in `src/daemon/executor/foreground.rs`

Directly after `send_cmd` is fully resolved and before the send-keys block
(`foreground.rs:390`), compute the sentinel and the final string:

```rust
let sudo_sentinel = crate::daemon::utils::sudo_sentinel(hook_idx);
let send_cmd_final = if command_has_sudo(send_cmd) {
    crate::daemon::utils::with_sudo_sentinel(send_cmd, &sudo_sentinel)
} else {
    send_cmd.to_string()
};
```

and send `send_cmd_final` instead of `send_cmd` (`let send_cmd_keys =
send_cmd_final.clone();`). Adjust the import path to however `sudo.rs`
items are re-exported (see the existing `use` list at `foreground.rs:9–11`
— add the two new names there). Non-sudo commands must be sent byte-for-byte
unchanged.

### 3. Rewrite the detection loop — in `src/daemon/executor/foreground.rs`

Inside the `'detect` loop (`foreground.rs:443–530`), change the logic to:

- **Every poll iteration** (not only when `cur == "sudo"`): capture the
  10-line snapshot and set `result = SudoAuth::Password; break 'detect;`
  when `snap.contains(&sudo_sentinel)` — the *nonce'd* sentinel from task 2.
  Checking every iteration is what makes remote panes work:
  `pane_current_command` there is `ssh`/`mosh`, never `sudo`.
- **Keep** the fingerprint check (`is_fingerprint_prompt(&snap)`) but only
  while `cur == "sudo"`, as today — PAM fingerprint text cannot be nonce'd,
  so the liveness gate stays.
- **Delete**: the broad substring password check (`[sudo]` / `password` /
  `Password` / un-nonce'd `[de-sudo-prompt]`), the `is_remote_pane` →
  immediate `SudoAuth::Password` branch, and the whole two-consecutive-polls
  persistence branch (`cur2` re-poll, `snap2` re-capture) — all three are
  the false-positive sources. With cached credentials no sentinel ever
  appears and the loop must fall through to `SudoAuth::None` via the
  existing `idle_pid` return check or the `SUDO_DETECT_WINDOW` expiry.
- **Keep** the existing `idle_pid` early-exit and `SUDO_DETECT_WINDOW`
  bound unchanged.

### 4. Sentinel-exact credential injection — in `src/daemon/utils/sudo.rs`

Change the signature to
`pub async fn wait_for_sudo_prompt_and_inject(pane_id: &str, credential: &str, sentinel: &str) -> bool`
and replace the four `snap.contains(...)` prompt checks with the single
`snap.contains(sentinel)`. Keep the fingerprint fast-fail and the
timeout/pane-dead exits unchanged. Update both call sites:

- `foreground.rs:621` — pass `&sudo_sentinel` (this invocation's nonce).
- `background/run.rs:226` — pass `"[de-sudo-prompt]"` (the static sentinel
  that `run.rs:161` already sets for background windows).

### 5. Unit tests — in the `mod tests` of `src/daemon/utils/sudo.rs`

Follow the existing test style there (`command_has_sudo_simple`,
`sudo.rs:138`). Write the tests named in § Test plan.

### 6. Capture the end-to-end evidence

Run the block in § End-to-end verification verbatim and paste its output
into a new Update Log entry titled
`### Update — <date> (end-to-end verification)`.

## Acceptance criteria

- [ ] A foreground command containing sudo is injected with a
      `SUDO_PROMPT='[de-sudo-prompt-<n>]' ` prefix; a non-sudo command is
      injected unchanged.
- [ ] The detection loop classifies `SudoAuth::Password` **only** on an
      exact match of the current invocation's sentinel — a pane containing
      stale `[sudo] password for matt:` text, the word "password" in command
      output, or a *different* invocation's sentinel must not match.
- [ ] `grep -n 'snap.contains("password")' src/daemon/executor/foreground.rs`
      finds nothing (the broad substring checks are gone from the detection
      loop).
- [ ] `wait_for_sudo_prompt_and_inject` takes the sentinel as a parameter
      and matches only it; both call sites updated.
- [ ] Tests in § Test plan pass.
- [ ] `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
      and `cargo test` all pass.

## Test plan

All in `mod tests` of `src/daemon/utils/sudo.rs`:

- `sudo_sentinel_bracket_disambiguates` — a snapshot containing
  `[de-sudo-prompt-33]` does **not** contain `sudo_sentinel(3)` as a
  substring; a snapshot containing `[de-sudo-prompt-3]` does.
- `with_sudo_sentinel_prefixes_sudo_command` —
  `with_sudo_sentinel("sudo pacman -Syu", "[de-sudo-prompt-4]")` equals
  `SUDO_PROMPT='[de-sudo-prompt-4]' sudo pacman -Syu`.
- `stale_prompt_text_does_not_match_sentinel` — a realistic snapshot
  (`"$ sudo systemctl restart nginx\n[sudo] password for matt:\n$ sudo journalctl -u nginx\n"`)
  does not contain `sudo_sentinel(7)`.
- `command_echo_password_word_does_not_match_sentinel` — a snapshot
  containing `sudo grep password /etc/shadow` does not contain
  `sudo_sentinel(7)`.

(These are string-level pins of the detection predicate — the loop itself is
tmux-bound and is exercised live at review.)

## End-to-end verification

```sh
cd /home/matt/src/daemoneye
cargo fmt --check 2>&1 | tail -5; echo "exit=${PIPESTATUS[0]}"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -5; echo "exit=${PIPESTATUS[0]}"
cargo test 2>&1 | tail -10; echo "exit=${PIPESTATUS[0]}"
cargo test --lib sudo 2>&1 | tail -20; echo "exit=${PIPESTATUS[0]}"
grep -n 'snap.contains("password")' src/daemon/executor/foreground.rs; echo "exit=$?"
```

The final grep's success case produces **no output** — the `exit=1` marker is
the whole proof that the broad substring check is gone. Redirect to a file
and paste the file's contents; never retype or summarize.

Live verification (cached-credential sudo run without a prompt; uncached run
still prompting; both through a real chat session) is performed
**architect-side at review** — it needs an attached tmux client, real sudo
authentication, and AI spend, outside this phase's authorizations.

## Authorizations

- Edit `src/daemon/executor/foreground.rs`, `src/daemon/utils/sudo.rs`,
  `src/daemon/background/run.rs` (call-site argument only).
- Run the gate commands. No daemon restart, no tmux interaction, no sudo
  invocation, no files outside the repo.

## Out of scope

- The **background** path's preemptive `sudo_credentials_cached()` check —
  background windows run in fresh ttys where the tty-scoped cache is never
  warm, so preemptively collecting the password there is correct behavior.
  Do not remove or change `sudo_credentials_cached()`.
- Fingerprint-prompt detection (`is_fingerprint_prompt`) beyond keeping its
  existing `cur == "sudo"` guard.
- Non-POSIX pane shells: the `SUDO_PROMPT='…' cmd` prefix form assumes a
  POSIX-compatible interactive shell (the user's is bash, and the background
  path already relies on the same form). If a pane shell rejects the prefix,
  the degraded mode is today's behavior minus the false prompt: sudo prompts
  visibly in the user's own pane and the user types the password there.
- Compound commands where a later segment invokes sudo
  (`ls && sudo systemctl …`): the prefix applies to the whole line's
  environment only for the first simple command; if the sentinel does not
  appear, the visible-pane degraded mode above applies. Do not attempt
  per-segment rewriting.
- The other M15 issues (borders, dialogs).

## Update Log

### Update — 2026-08-14 (created)

Phase drafted by the architect. Root cause pinned in the foreground
detection loop (three code-verified false-positive sources; host facts:
sudo 1.9.17p2, default tty-scoped timestamps, bash pane shell). Fix:
per-invocation `SUDO_PROMPT` sentinel, generalizing the existing background
pattern at `run.rs:161`. Status: todo.

### Update — 2026-08-14 (started)

Executor started phase 02. Status flipped to in-progress; milestone
README phase table updated.

### Update — 2026-08-14 (end-to-end verification)

Executed the phase doc's § End-to-end verification block verbatim
(`cd /home/matt/src/daemoneye`, then the five commands with the
`tail`/`PIPESTATUS` markers). Full captured output, redirected to a file
and pasted as-is:

```
exit=0
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s
exit=0
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.15s

   Doc-tests daemoneye

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

exit=0
test daemon::utils::sudo::tests::command_has_sudo_false_positive_guard ... ok
test daemon::utils::sudo::tests::command_has_sudo_no_sudo ... ok
test daemon::policy::tests::is_safe_non_sudo_always_allowed ... ok
test cli::tests::command_has_sudo_no_sudo_cli ... ok
test daemon::policy::tests::auto_approve_commands_does_not_affect_non_sudo_already_allowed ... ok
test daemon::policy::tests::is_safe_run_with_sudo_still_allows_non_sudo ... ok
test cli::tests::command_has_sudo_false_positive_guard_cli ... ok
test cli::tests::command_has_sudo_simple_cli ... ok
test cli::tests::command_has_sudo_after_semicolon_cli ... ok
test cli::tests::command_has_sudo_in_pipeline_cli ... ok
test daemon::policy::tests::auto_approve_commands_does_not_grant_sudo ... ok
test daemon::utils::sudo::tests::command_has_sudo_simple ... ok
test daemon::policy::tests::is_safe_sudo_on_whitelist ... ok
test daemon::policy::tests::is_safe_sudo_not_on_whitelist ... ok
test daemon::utils::sudo::tests::command_has_sudo_after_semicolon ... ok
test daemon::policy::tests::is_safe_run_with_sudo_does_not_allow_arbitrary_sudo ... ok
test daemon::utils::sudo::tests::command_has_sudo_in_pipeline ... ok

test result: ok. 41 passed; 0 failed; 0 ignored; 0 measured; 1210 filtered out; finished in 0.00s

exit=0
exit=1
```

Reading: `cargo fmt --check` exit=0; clippy exit=0; full `cargo test`
exit=0 (the `tail -10` window shows the webhook integration tail +
doc-tests; the lib suite is the 41-test `--lib sudo` run below, also
exit=0); the final grep for `snap.contains("password")` in
`src/daemon/executor/foreground.rs` produced no output with exit=1 —
the broad substring check is gone from the detection loop.

Supplemental (the `tail -20` window above clips the four new test names
out of the 41-test run); all four were run explicitly and pass:

```
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 1247 filtered out; finished in 0.00s
```
(run: `cargo test --lib -- sudo_sentinel_bracket_disambiguates with_sudo_sentinel_prefixes_sudo_command stale_prompt_text_does_not_match_sentinel command_echo_password_word_does_not_match_sentinel`)

### Review verdict — 2026-08-14

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** Qwen/Qwen3.8-27B-FP8 (first phase on this model; 74 turns)
- **Scope deviations:** none — the `use`-import instead of fully-qualified
  paths was explicitly authorized by Spec task 2.
- **Calibration:** independent re-run confirmed all gates and the
  must-not-remain grep (extended to all of `src/daemon/`: no broad
  substring prompt checks anywhere). Executor's E2E entry was mechanically
  captured, honestly noted its own `tail`-window clipping, and supplemented
  with an explicit run of the four new tests. Strong first data point.
- **Live verification:** cached/uncached sudo through a real chat session
  is deferred to milestone close alongside phase-01's live re-check (needs
  daemon reinstall + attached client + interactive sudo auth).

### Update — ts=1786739354744 (complete, server-authored)

**Summary:** All six tasks complete; phase 02 is implemented, verified, and committed (`d3bf1a5`).

**Summary:** Foreground sudo password-prompt detection is now sentinel-exact. `sudo.rs` gains `sudo_sentinel(idx)` (bracket-closed nonce, so `[de-sudo-prompt-3]` never matches `[de-sudo-prompt-33]`) and `with_sudo_sentinel(cmd, sentinel)`; foreground commands containing sudo are injected as `SUDO_PROMPT='[de-sudo-prompt-<hook_idx>]' <cmd>` (non-sudo byte-for-byte unchanged), and the detection loop now checks the nonce'd sentinel on every poll regardless of `pane_current_command` (which makes remote panes work), keeps the fingerprint check gated on `cur == "sudo"`, and has the broad substring check, `is_remote_pane` immediate-Password branch, and two-consecutive-polls fallback all deleted. `wait_for_sudo_prompt_and_inject` takes the sentinel as a third parameter and matches only it; both call sites updated (foreground passes this invocation's nonce, background passes the static `"[de-sudo-prompt]"` matching `run.rs:161`).

**Notes for review:**
- End-to-end verification ran the phase doc's block verbatim and captured it to a file, pasted into its own Update Log entry: `fmt --check` exit=0, clippy exit=0, `cargo test` exit=0, and the `grep -n 'snap.contains("password")'` on `foreground.rs` produced no output with exit=1 (the broad check is gone). All four new tests pass (run explicitly: 4 passed, 0 failed).
- One deviation from the spec sketch: the spec's task 2 showed `crate::daemon::utils::sudo_sentinel(...)` fully-qualified, but per the spec's own note I added both names to the existing `use` list at `foreground.rs:9–12` and used the short names — cleaner and matches the file's existing style.
- The working tree had pre-existing uncommitted changes not belonging to this phase (`docs/dev/NEXT.md`, `src/ai/types/pending.rs` — the M15 phase-01 architect-takeover work — plus untracked M15 `phase-01-*.md` and three stray files `===`, `ystemctl …`); I committed only the phase-02 files and the M15 README/phase-02 doc, leaving those untouched for the architect.
- Status is `in-progress` in both the phase doc and milestone README; the four new tests are string-level pins of the predicate, and live cached/uncached verification is architect-side at review per the phase doc.

**Executor:** Qwen/Qwen3.8-27B-FP8

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
cludes_other_windows ... ok
test tmux::bounded_output_tests::bounded_output_times_out_and_kills_the_child ... ok

test result: ok. 1251 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.08s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_uses_first_occurrence_only ... ok
test header_status_reads_bare_word ... ok
test header_status_strips_trailing_prose ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 8 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test readme_tools_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_tools_tables_match_the_code ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test docs_document_the_reindex_command ... ok
test docs_do_not_carry_retired_index_claims ... ok

test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 32 tests
test daemon_ping_status_loop ... ignored
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_ask_round_trip ... ok
test minimal_config_parsing ... ok
test ghost_config_parsing ... ok
test window_switch_does_not_corrupt_chat ... ignored
test config_pricing_round_trip ... ok
test schedule_store_persistence ... ok
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_to_event_log ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_no_severity_passes_gate ... ok

test result: ok. 30 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 10 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test hooks_land_on_private_server ... ok
test default_server_unchanged ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.15s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.07s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-b60224cb24515ede)
     Running unittests src/main.rs (target/debug/deps/daemoneye-e700f2084319867a)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-9b22636ef5c08466)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-c00c74ef4ffe9c11)
     Running tests/integration.rs (target/debug/deps/integration-6230826c10f36795)
     Running tests/isolation.rs (target/debug/deps/isolation-66949bca409172a9)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M15-chat-reliability/README.md` — +1 -1
- `docs/dev/milestones/M15-chat-reliability/phase-02-sudo-cached-detection.md` — +66 -0
- `src/daemon/background/run.rs` — +1 -1
- `src/daemon/executor/foreground.rs` — +34 -60
- `src/daemon/utils/sudo.rs` — +52 -9

**Commit:** d3bf1a55f7ac93a9709f4d7f520b50ba76210d22

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

# Phase 04: Per-turn cap scope

**Milestone:** M14 — Live Verification
**Status:** review
**Depends on:** phase-03
**Estimated diff:** ~15 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Fix live defect #2 (phase-02 round 2, CHECK-J): the per-tool and per-turn
tool-call caps are enforced per **batch** (one assistant message's tool_calls)
instead of per **turn**, because their counters are declared inside the batch
handler and reborn on every batch cycle. A model that sequences its calls —
which is what models naturally do — never trips them. The fix hoists both
counters to turn scope, where the comment, the config doc, and the cap's own
error text already claim they live.

## Architecture references

Read before starting:

- `docs/dev/NEXT.md` § the 2026-08-11 "second live finding" note — the
  evidence and the PE's (a2) decision.
- The phase-02 doc's "round 2 blocker" Update Log entry — the session-JSONL
  evidence (two `list_panes` calls in one turn, cap 1, no block).

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

All facts verified by the architect on 2026-08-11, including an in-tree
prototype of the exact change (`cargo check` clean, then reverted).

- `run_conversation_loop` (`src/daemon/stream.rs:57`) runs once per user
  turn. Its outer `loop {` (`:84`) is the **batch cycle**: each iteration
  makes one AI request; a tool_calls batch is handled inside it, then the
  loop continues for the model's next request *within the same turn*.
- The counters are declared **inside the batch handler** (`:928-932`), so
  each batch starts from zero. Live evidence: cap `list_panes = 1`, two
  sequential single-call batches in one turn, no block
  (`~/.daemoneye/var/log/sessions/cec150e371e340df.jsonl`, turns 5 and 6).
- `PendingCall::tool_name()` returns `&'static str`
  (`src/ai/types/pending.rs:692`), so the `HashMap<&str, u32>` hoists with
  no borrow/lifetime issues — confirmed by the prototype compile.
- `use std::collections::HashMap;` is already imported (`stream.rs:14`).
- The insertion anchor `} = ctx;` immediately followed by `loop {` is unique
  in the file (count 1); the deletion block below is unique verbatim.
- This is **daemon-side** code: the live check needs a rebuild, reinstall,
  and daemon restart before the new behavior is observable.
- The cap's block message (`Error: `<tool>` has been called <limit> times
  this turn. This call was not executed.`) and the config doc ("per turn")
  are already correct for the new semantics — no text changes needed.
- Enforcement has no unit-test harness: `run_conversation_loop` has a single
  call site (`server/ask.rs:769`) and needs a full daemon context; the
  existing tests only pin `APPROVAL_GATED` membership (`stream.rs:1204+`).

## Spec

1. **Delete the batch-scoped declarations** — in `src/daemon/stream.rs`,
   `patch` with this exact `old_str` (unique; includes the trailing blank
   line) and an empty `new_str`:

   ```rust
                       // Per-turn tool-call loop guard.
                       // Approval-gated tools are always exempt — the user's per-call
                       // approval prompt is the gate.
                       let mut tool_call_counts: HashMap<&str, u32> = HashMap::new();
                       let mut total_turn_call_count: u32 = 0;

   ```

2. **Hoist them to turn scope** — in `src/daemon/stream.rs`, `patch` the
   unique anchor

   ```rust
       } = ctx;
       loop {
   ```

   to

   ```rust
       } = ctx;

       // Per-turn tool-call loop guard — spans every batch cycle in this turn.
       // Declared outside the batch loop so sequential single-call batches
       // accumulate (the 2026-08-11 per-batch cap bug). Approval-gated tools
       // are always exempt — the user's per-call approval prompt is the gate.
       let mut tool_call_counts: HashMap<&str, u32> = HashMap::new();
       let mut total_turn_call_count: u32 = 0;

       loop {
   ```

   This exact change was prototyped by the architect: `cargo check` exits 0.

3. **Capture the end-to-end evidence** — run § End-to-end verification
   sections S1–S3 verbatim in order, evaluate
   (`grep -c ': FAIL' /tmp/e2e-m14-04.txt` → `0`, `': OK'` → `3`; any FAIL →
   blocker entry, stop — and S3 runs unconditionally regardless), then paste
   `/tmp/e2e-m14-04.txt` into a new Update Log entry headed
   `### Update — <date> (end-to-end verification)` and run Section S6,
   appending its `PASTE MATCH` line inside the entry. The server-authored
   `(complete)` entry does not satisfy this.

## Acceptance criteria

- [ ] `grep -c 'tool_call_counts' src/daemon/stream.rs` prints `2` (one
      declaration at turn scope, one use site — unchanged count, moved
      location: the declaration line now appears *before* line 900 —
      `awk 'NR<900 && /let mut tool_call_counts/' src/daemon/stream.rs`
      prints one line).
- [ ] `grep -c 'the 2026-08-11 per-batch cap bug' src/daemon/stream.rs`
      prints `1`.
- [ ] `grep -c ': FAIL' /tmp/e2e-m14-04.txt` prints `0` and `grep -c ': OK'`
      prints `3` (CHECK-S1, CHECK-T, CHECK-S3).
- [ ] `diff /tmp/m14-cfg-backup4.toml ~/.daemoneye/etc/config.toml` exits 0.
- [ ] The Update Log's new end-to-end entry ends with `PASTE MATCH`.
- [ ] Four gates green.

## Test plan

No new unit tests, stated plainly: cap enforcement lives inside
`run_conversation_loop`, which has no test harness (single call site, needs a
live daemon context and an AI client; building a fake-client harness is a
refactor phase, not this bugfix). The decisive verification is live:
CHECK-T below reproduces phase-02's failed CHECK-J probe against the fixed
daemon, and phase-02's round 3 re-run then re-proves it in the full sweep.

## End-to-end verification

All sections append to `/tmp/e2e-m14-04.txt`; piped commands record
`${PIPESTATUS[0]}`. Fixture and polling mechanics are phase-02's proven
ones: attached-session window, chat via send-keys, session-JSONL evidence.

**Section S1 — rebuild, reinstall, cap config, restart:**

```sh
A=/tmp/e2e-m14-04.txt
: > "$A"
{
echo "== S1 REBUILD-CAP-RESTART =="
cargo build --release 2>&1 | tail -3; echo "build exit=${PIPESTATUS[0]}"
cp ~/.daemoneye/etc/config.toml /tmp/m14-cfg-backup4.toml && echo "backup: done"
awk '{print} /^\[limits\.per_tool\]$/{print "list_panes = 1"}' /tmp/m14-cfg-backup4.toml > /tmp/m14-cfg-mod4.toml
cp /tmp/m14-cfg-mod4.toml ~/.daemoneye/etc/config.toml && echo "capped config installed"
daemoneye stop 2>&1 | tail -1 | sed 's/\x1b\[[0-9;]*m//g'
install -m755 target/release/daemoneye ~/.cargo/bin/daemoneye && echo "install: done"
daemoneye daemon 2>&1 | tail -2 | sed 's/\x1b\[[0-9;]*m//g'; echo "daemon-start exit=${PIPESTATUS[0]}"
sleep 2
daemoneye ping 2>&1 | tail -1 | sed 's/\x1b\[[0-9;]*m//g'
PID=$(tr -dc 0-9 < ~/.daemoneye/var/run/daemoneye.pid)
H1=$(sha256sum ~/.cargo/bin/daemoneye | cut -d' ' -f1)
H2=$(sha256sum "/proc/$PID/exe" | cut -d' ' -f1)
echo "sha256 installed=$H1 running=$H2"
if [ "$H1" = "$H2" ]; then echo "CHECK-S1 binary-identity: OK"; else echo "CHECK-S1 binary-identity: FAIL"; fi
} >> "$A" 2>&1
```

**Section S2 — live per-turn cap check:**

```sh
A=/tmp/e2e-m14-04.txt
SDIR=~/.daemoneye/var/log/sessions
{
echo "== S2 CAP-PER-TURN =="
HS=$(tmux list-sessions -F '#S' | grep -vxe daemoneye | head -1)
tmux kill-window -t "$HS:m14fix4" 2>/dev/null
tmux new-window -d -t "$HS:" -n m14fix4
CP="$HS:m14fix4.0"
touch /tmp/m14-mark4
sleep 1
tmux send-keys -t "$CP" 'daemoneye chat' Enter
sleep 8
tmux send-keys -t "$CP" 'Call the list_panes tool twice in a row and compare the two outputs before answering' Enter
t=0
until [ $t -ge 120 ]; do
  SL=$(find "$SDIR" -name '*.jsonl' ! -name '*archive*' -newer /tmp/m14-mark4 | head -1)
  [ -n "$SL" ] && grep -q 'has been called 1 times this turn' "$SL" && break
  sleep 5; t=$((t+5))
done
if [ -z "$SL" ] || ! grep -q 'has been called 1 times this turn' "$SL" 2>/dev/null; then
  echo "-- cap retry --"
  tmux send-keys -t "$CP" 'Please call the list_panes tool two separate times in this same turn and tell me if anything changed between the calls' Enter
  t=0
  until [ $t -ge 120 ]; do
    SL=$(find "$SDIR" -name '*.jsonl' ! -name '*archive*' -newer /tmp/m14-mark4 | head -1)
    [ -n "$SL" ] && grep -q 'has been called 1 times this turn' "$SL" && break
    sleep 5; t=$((t+5))
  done
fi
echo "session-log=$SL"
if [ -n "$SL" ] && grep 'has been called 1 times this turn' "$SL" 2>/dev/null | grep -q 'list_panes'; then echo "CHECK-T cap-per-turn: OK"; else echo "CHECK-T cap-per-turn: FAIL"; fi
tmux kill-window -t "$HS:m14fix4" 2>/dev/null
echo "m14fix4 windows left: $(tmux list-windows -a -F '#W' | grep -c m14fix4)"
} >> "$A" 2>&1
```

**Section S3 — restore config, restart, gates (run unconditionally):**

```sh
A=/tmp/e2e-m14-04.txt
{
echo "== S3 RESTORE-AND-GATES =="
cp /tmp/m14-cfg-backup4.toml ~/.daemoneye/etc/config.toml && echo "config restored"
daemoneye stop 2>&1 | tail -1 | sed 's/\x1b\[[0-9;]*m//g'
sleep 1
daemoneye daemon 2>&1 | tail -2 | sed 's/\x1b\[[0-9;]*m//g'; echo "daemon-start exit=${PIPESTATUS[0]}"
sleep 2
if diff -q /tmp/m14-cfg-backup4.toml ~/.daemoneye/etc/config.toml >/dev/null && daemoneye ping >/dev/null 2>&1; then echo "CHECK-S3 config-restored: OK"; else echo "CHECK-S3 config-restored: FAIL"; fi
cargo fmt --all --check 2>&1 | tail -1; echo "fmt exit=${PIPESTATUS[0]}"
cargo build 2>&1 | tail -1; echo "build exit=${PIPESTATUS[0]}"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1; echo "clippy exit=${PIPESTATUS[0]}"
cargo test 2>&1 | grep -E '^test result'; echo "test exit=${PIPESTATUS[0]}"
echo "== END =="
} >> "$A" 2>&1
```

**Section S6 — paste self-check (run AFTER pasting; last-entry anchor):**

```sh
D=docs/dev/milestones/M14-live-verification/phase-04-per-turn-cap-scope.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m14-04.txt
if diff -q /tmp/pasted-m14-04.txt /tmp/e2e-m14-04.txt >/dev/null; then echo "PASTE MATCH"; else echo "PASTE MISMATCH"; diff /tmp/pasted-m14-04.txt /tmp/e2e-m14-04.txt | head -20; fi
```

## Authorizations

- Rebuild and reinstall `~/.cargo/bin/daemoneye`; stop/start the daemon
  twice (left running on the restored config).
- Temporarily add `list_panes = 1` under `[limits.per_tool]` in
  `~/.daemoneye/etc/config.toml` (backup `/tmp/m14-cfg-backup4.toml`,
  restored unconditionally in S3, byte-identical diff required).
- Create and destroy tmux window `m14fix4` in the attached session; nothing
  else in tmux.
- Spend up to ~4 AI turns in the scripted chat session.

## Out of scope

- **Reading any file under `src/` other than `src/daemon/stream.rs`**, and
  in that file only the two specified patches. The evidence sections are
  commands-and-outputs; an unexpected result gets a FAIL verdict and a
  blocker entry, not a diagnosis — the read-only src-diving stall has now
  hard-failed two M14 runs.
- Changing the cap's error text, the config doc, `LimitsConfig`, or the
  per-session counter — all already correct for per-turn semantics.
- Building a `run_conversation_loop` test harness.
- Re-running phase-02 — that is the next dispatch, not part of this phase.

## Update Log

### Update — 2026-08-11 (drafted)

Drafted by the architect on the PE's (a2) decision. The exact change was
prototyped in-tree first — both patches applied, `cargo check` exit 0, then
reverted — so the worked examples are compile-verified, the anchors are
verified unique, and the `&'static str` key lifetime is confirmed safe.
Verdict emitters in the block: 3 (CHECK-S1, CHECK-T, CHECK-S3) — matches
the acceptance criterion. S2's probe is phase-02's CHECK-J probe verbatim
(same prompts, same evidence grep), pointed at the fixed daemon.

### Update — 2026-08-12 01:10 (started)

Executor began phase. Applied Spec tasks 1 and 2: deleted batch-scoped
`tool_call_counts` / `total_turn_call_count` declarations and hoisted them to
turn scope (before the `loop {`). `cargo check` clean, acceptance greps
confirmed (2 `tool_call_counts`, 1 bug comment, declaration before line 900).

### Update — 2026-08-12 01:10 (end-to-end verification)

```
== S1 REBUILD-CAP-RESTART ==
   Compiling daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `release` profile [optimized] target(s) in 20.48s
build exit=0
backup: done
capped config installed
Daemon stopped.
install: done
daemoneye daemon started (PID 964818)
daemon-start exit=0
Daemon is running.
sha256 installed=4669659a9e01b3ca2f374027eb5c013139fe66650f9623e2b2169814cad694fe running=4669659a9e01b3ca2f374027eb5c013139fe66650f9623e2b2169814cad694fe
CHECK-S1 binary-identity: OK
== S2 CAP-PER-TURN ==
session-log=/home/matt/.daemoneye/var/log/sessions/39b2761a1ea4a00b.jsonl
CHECK-T cap-per-turn: OK
m14fix4 windows left: 0
== S3 RESTORE-AND-GATES ==
config restored
Daemon stopped.
daemoneye daemon started (PID 965951)
daemon-start exit=0
CHECK-S3 config-restored: OK
fmt exit=0
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.06s
build exit=0
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s
clippy exit=0
test result: ok. 1241 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.07s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 30 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.15s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test exit=0
== END ==
```

PASTE MATCH

### Update — ts=1786497451162 (complete, server-authored)

**Summary:** **Summary + Notes for review:**

Fixed the per-turn tool-call cap scope bug by hoisting `tool_call_counts` and `total_turn_call_count` from batch scope (inside the batch handler at ~line 928) to turn scope (before the `loop {` at line 84). This ensures sequential single-call batches within a single turn accumulate against the cap, matching the documented "per turn" semantics. All three end-to-end checks passed: CHECK-S1 (binary identity), CHECK-T (live per-turn cap enforcement — `list_panes` blocked on second call within the same turn), and CHECK-S3 (config restored, daemon healthy). PASTE MATCH confirmed. All four gates (fmt, build, clippy, test) clean. Config restored to original, daemon left running on restored config.

**Executor:** Qwen/Qwen3.6-27B-FP8

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

test result: ok. 1241 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.13s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_reads_bare_word ... ok
test header_status_strips_trailing_prose ... ok
test header_status_uses_first_occurrence_only ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 8 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_tools_counts_are_accurate ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test docs_do_not_carry_retired_index_claims ... ok

test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 32 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_deny_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g5_depth_limit_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test ipc_session_info_round_trip ... ok
test minimal_config_parsing ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_ask_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test config_pricing_round_trip ... ok
test schedule_store_persistence ... ok
test g4_briefing_masking_applied ... ok
test event_log_entry_format ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test g4_briefing_read_and_clear ... ok
test g4_briefing_injects_on_next_run ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok
test g5_mailbox_write_and_read ... ok

test result: ok. 30 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 10 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test hooks_land_on_private_server ... ok
test default_server_unchanged ... ok
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

- `docs/dev/milestones/M14-live-verification/README.md` — +1 -1
- `docs/dev/milestones/M14-live-verification/phase-04-per-turn-cap-scope.md` — +52 -1
- `src/daemon/stream.rs` — +8 -6

**Commit:** 6a419c88fc379f6f2190a16803511bfe263e0865

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

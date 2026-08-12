# Phase 03: Approval state persistence

**Milestone:** M14 — Live Verification
**Status:** todo
**Depends on:** phase-02 (its blocker is this phase's cause)
**Estimated diff:** ~10 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Fix the live finding that blocked phase-02: runtime session-approval state is
wiped at the end of every turn, so `/approvals revoke`/`on`/`off` lasts one
turn and an `[A]pprove for session` answer evaporates for any class whose
config default is `false`. The fix makes `SessionApproval` a true
session-lifetime state: initialized from config at session start, mutated
only by runtime commands and prompt answers.

## Architecture references

Read before starting:

- `docs/dev/NEXT.md` § the BLOCKED note dated 2026-08-11 — the finding, its
  evidence, and the PE's decision (option (a): fix, then re-run phase-02).
- `CLAUDE.md` § "Request/Response lifecycle" step 3 — where approval answers
  fit.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

All facts verified by the architect on 2026-08-11, including a scratch-copy
application of the exact deletion.

- The offending block is at `src/cli/commands/stream.rs:653-657`, at the tail
  of `ask_with_session_ratatui`, and reads exactly:

  ```rust
      // Update approval from config in case it changed during the turn.
      {
          let cfg = Config::load().unwrap_or_default();
          *approval = SessionApproval::from_config(&cfg.approvals);
      }
  ```

  It re-derives the whole `SessionApproval` from the config file at the end
  of **every** turn, clobbering every runtime mutation: `/approvals
  revoke|on|off` (`src/cli/commands/slash.rs:296-309`), the `A`
  session-approve answer (`stream.rs` sets `approval.regular = true` only on
  `is_session`), and the per-name script/runbook/file-edit session
  approvals. `git log -L` attributes it to `93fa228` (2026-06-24), an
  unrelated renderer bugfix — it slipped in without tests and no test pins
  it.
- **Intended semantics** (per the config file's own comment and the PE
  decision): approvals config applies **at session start**; runtime state
  wins for the life of the session. Mid-session config edits take effect in
  new sessions — that is the documented contract
  ("Controls which action classes auto-approve at the start of every chat
  session").
- The initializers already implement session-start semantics:
  `chat.rs:95` and `ask.rs:26` build `SessionApproval::from_config(...)`
  once at entry.
- **After the deletion, `use crate::config::Config;` (`stream.rs:13`) has
  zero remaining uses** — verified on the scratch copy — so the import must
  be deleted in the same change or `-D warnings` fails on `unused_imports`.
  `use super::approval::SessionApproval;` (`stream.rs:16`) stays: still used
  at `:72` and elsewhere.
- `grep -c 'from_config' src/cli/commands/stream.rs` is `1` today and `0`
  after the fix — validated against the scratch-applied tree.
- The daemon and installed binary are current (phase-01's end state); the
  fix is **client-side** (chat/ask process), so the live check needs a
  rebuild + reinstall before it can observe the new behavior.

## Spec

1. **Delete the turn-end reset** — in `src/cli/commands/stream.rs`, delete
   the exact 6-line block quoted in § Current state (comment line through
   closing brace, plus its trailing blank line). Use the `patch` tool with
   that quote as `old_str` — it is unique in the file (verified).

2. **Delete the orphaned import** — in `src/cli/commands/stream.rs`, delete
   the line `use crate::config::Config;` (line 13 as of drafting). Do not
   touch the `SessionApproval` import.

3. **Pin the semantics in a doc comment** — in
   `src/cli/commands/approval.rs`, extend the doc comment on
   `from_config` (`:48`) with these two lines (verbatim):

   ```rust
   /// Called once at session start (chat/ask entry). Never re-derive this
   /// mid-session: runtime state (`/approvals`, prompt answers) must win —
   /// a turn-end re-derive was the 2026-08-11 approval-persistence bug.
   ```

4. **Capture the end-to-end evidence** — run the sections in § End-to-end
   verification verbatim, in order (S1 rebuild/reinstall/restart, S2 live
   revoke-persistence check, S3 gates), evaluate the verdicts
   (`grep -c ': FAIL' /tmp/e2e-m14-03.txt` must print `0`, `': OK'` must
   print `2`; any FAIL → blocker entry, stop), then paste
   `/tmp/e2e-m14-03.txt` into a new Update Log entry headed
   `### Update — <date> (end-to-end verification)` and run Section S6,
   appending its `PASTE MATCH` line inside the entry. The server-authored
   `(complete)` entry does not satisfy this.

## Acceptance criteria

- [ ] `grep -c 'from_config' src/cli/commands/stream.rs` prints `0`.
- [ ] `grep -c 'use crate::config::Config' src/cli/commands/stream.rs`
      prints `0`.
- [ ] `grep -c 'Never re-derive this' src/cli/commands/approval.rs` prints
      `1`.
- [ ] `grep -c ': FAIL' /tmp/e2e-m14-03.txt` prints `0` and
      `grep -c ': OK'` prints `2` (CHECK-S1, CHECK-P).
- [ ] The Update Log's new end-to-end entry ends with `PASTE MATCH`.
- [ ] Four gates green (fmt, build, clippy `-D warnings`, test).

## Test plan

No new unit tests, stated plainly: the deleted code is a call site inside
`ask_with_session_ratatui`, which cannot be constructed in a unit test (it
requires a live tty for `RatatuiRendererStdout` and a daemon socket — the
existing `stream_seam_tests` stop at `select_stream` for this reason). The
decisive verification is live: CHECK-P in this phase's E2E proves
`/approvals revoke` survives a completed turn, and the phase-02 re-run that
follows this phase re-proves the full deny path (CHECK-G) through the same
door that caught the bug.

## End-to-end verification

Run each section verbatim, in order; all append to `/tmp/e2e-m14-03.txt`.
Piped commands record `${PIPESTATUS[0]}`.

**Section S1 — rebuild, reinstall, restart:**

```sh
A=/tmp/e2e-m14-03.txt
: > "$A"
{
echo "== S1 REBUILD-RESTART =="
cargo build --release 2>&1 | tail -3; echo "build exit=${PIPESTATUS[0]}"
daemoneye stop 2>&1 | tail -1 | sed 's/\x1b\[[0-9;]*m//g'
install -m755 target/release/daemoneye ~/.cargo/bin/daemoneye && echo "install: done"
daemoneye daemon 2>&1 | tail -2 | sed 's/\x1b\[[0-9;]*m//g'; echo "daemon-start exit=${PIPESTATUS[0]}"
sleep 2
daemoneye ping 2>&1 | tail -1 | sed 's/\x1b\[[0-9;]*m//g'
H1=$(sha256sum target/release/daemoneye | cut -d' ' -f1)
H2=$(sha256sum ~/.cargo/bin/daemoneye | cut -d' ' -f1)
echo "sha256 target=$H1 installed=$H2"
if [ "$H1" = "$H2" ]; then echo "CHECK-S1 binary-identity: OK"; else echo "CHECK-S1 binary-identity: FAIL"; fi
} >> "$A" 2>&1
```

**Section S2 — live revoke-persistence check:**

```sh
A=/tmp/e2e-m14-03.txt
SDIR=~/.daemoneye/var/log/sessions
{
echo "== S2 REVOKE-PERSISTS =="
HS=$(tmux list-sessions -F '#S' | grep -vxe daemoneye | head -1)
tmux kill-window -t "$HS:m14fix3" 2>/dev/null
tmux new-window -d -t "$HS:" -n m14fix3
tmux split-window -d -t "$HS:m14fix3"
TP=$(tmux list-panes -t "$HS:m14fix3" -F '#{pane_id} #{pane_active}' | grep ' 0' | cut -d' ' -f1)
echo "target-pane=$TP"
CP="$HS:m14fix3.0"
touch /tmp/m14-mark3
sleep 1
tmux send-keys -t "$CP" 'daemoneye chat' Enter
sleep 8
tmux send-keys -t "$CP" '/approvals revoke' Enter
sleep 3
tmux send-keys -t "$CP" 'Reply with just the word ready' Enter
t=0
until [ $t -ge 90 ]; do
  SL=$(find "$SDIR" -name '*.jsonl' ! -name '*archive*' -newer /tmp/m14-mark3 | head -1)
  [ -n "$SL" ] && grep -qi 'ready' "$SL" && break
  sleep 5; t=$((t+5))
done
echo "turn-1 session-log=$SL"
PC=$(tmux capture-pane -p -t "$CP" -S -200 | grep -c 'Approve?')
tmux send-keys -t "$CP" "Use the tmux_control tool to zoom pane $TP" Enter
t=0
until [ $t -ge 120 ]; do
  NC=$(tmux capture-pane -p -t "$CP" -S -200 | grep -c 'Approve?')
  [ "$NC" -gt "$PC" ] && break
  sleep 5; t=$((t+5))
done
NC=$(tmux capture-pane -p -t "$CP" -S -200 | grep -c 'Approve?')
echo "prompt-count before=$PC after=$NC (must increase: revoke survived turn 1)"
if [ "$NC" -gt "$PC" ]; then echo "CHECK-P revoke-persists: OK"; else echo "CHECK-P revoke-persists: FAIL"; fi
tmux send-keys -t "$CP" 'n'
sleep 8
tmux kill-window -t "$HS:m14fix3" 2>/dev/null
echo "m14fix3 windows left: $(tmux list-windows -a -F '#W' | grep -c m14fix3)"
} >> "$A" 2>&1
```

**Section S3 — gates:**

```sh
A=/tmp/e2e-m14-03.txt
{
echo "== S3 GATES =="
cargo fmt --all --check 2>&1 | tail -1; echo "fmt exit=${PIPESTATUS[0]}"
cargo build 2>&1 | tail -1; echo "build exit=${PIPESTATUS[0]}"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1; echo "clippy exit=${PIPESTATUS[0]}"
cargo test 2>&1 | grep -E '^test result'; echo "test exit=${PIPESTATUS[0]}"
echo "== END =="
} >> "$A" 2>&1
```

**Section S6 — paste self-check (run AFTER pasting the artifact into the
Update Log entry; last-entry anchor):**

```sh
D=docs/dev/milestones/M14-live-verification/phase-03-approval-state-persistence.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m14-03.txt
if diff -q /tmp/pasted-m14-03.txt /tmp/e2e-m14-03.txt >/dev/null; then echo "PASTE MATCH"; else echo "PASTE MISMATCH"; diff /tmp/pasted-m14-03.txt /tmp/e2e-m14-03.txt | head -20; fi
```

## Authorizations

- Rebuild and reinstall `~/.cargo/bin/daemoneye`; stop/start the daemon
  (left running).
- Create and destroy tmux window `m14fix3` in the attached session — nothing
  else in tmux.
- Spend up to ~3 AI turns in the scripted chat session.

## Out of scope

- Any source change beyond the three specified edits (two deletions in
  `stream.rs`, one doc comment in `approval.rs`). In particular: no
  hot-reload replacement mechanism, no `[approvals]` config schema change,
  no changes to `slash.rs`.
- Re-running phase-02 — that is the architect's next dispatch, not part of
  this phase.
- The known cosmetic residuals (width-flip scrollback ghosts, etc.).

## Update Log

### Update — 2026-08-11 (drafted)

Drafted by the architect on the PE's option-(a) decision. The deletion was
applied to a scratch copy first: the `old_str` block is unique, the
post-deletion `from_config` count in `stream.rs` is 0, and the `Config`
import orphans (caught by the scratch check — it would have failed
`-D warnings`; § "A phase that exhausts a trait's uses must say what happens
to its import" applying to a `use` line). The S2 mechanics reuse phase-02's
proven prototype patterns: attached-session fixture window, prompt-count
polling (never a bare grep), single-keypress answers. Verdict emitters in
the block: 2 (CHECK-S1, CHECK-P) — matches the acceptance criterion.

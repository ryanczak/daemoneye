# Phase 01: Scripted live sweep

**Milestone:** M14 — Live Verification
**Status:** todo
**Depends on:** none
**Estimated diff:** ~0 source lines — this phase ships evidence, not code
**Tags:** language=shell, kind=test, size=m

## Goal

Live-verify, against a daemon freshly restarted onto the current binary, every
M12 surface that is drivable without a human at an approval prompt:
cross-session visibility (`list_panes`, `find_in_panes`, `get_terminal_context`
with `scope: "all"`), `read_pane` through the real tool dispatch path, status
classification on real panes in three states, and the `/panes` inspector. M12
closed with these verified at unit level only; this phase produces the live
transcript.

## Architecture references

Read before starting:

- `docs/dev/milestones/M14-live-verification/README.md` — why this milestone
  exists and what phase-02 covers instead (approval round trips are NOT this
  phase).
- `CLAUDE.md` § "Request/Response lifecycle" — where `ask` sits relative to the
  daemon and the tool dispatch path.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before running any command.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Facts derived 2026-08-10 by running the commands against this host; re-verify
any that look stale before starting.

- **The daemon is not running** (`daemoneye status` → "Daemon is not running
  (stale PID file names PID 76979)"). The stale PID file is harmless — the
  `flock` on it, not its contents, is the single-instance authority
  (`src/daemon/instance.rs`); it is diagnostic payload and this phase uses it
  only as a pointer to `/proc/<pid>/exe` for evidence, never to decide
  anything.
- **The installed binary is `~/.cargo/bin/daemoneye`** (`which daemoneye`),
  v0.9.9, built 2026-08-10 07:26 — it predates today's commits, which is the
  exact gap this phase closes.
- **`daemoneye ask --raw` auto-denies *prompts only*** — read
  `src/cli/commands/ask.rs:115-221`: `ToolCallPrompt`, `EditFilePrompt` etc.
  get an automatic deny, but silent (non-approval-gated) tools never send a
  prompt — they execute daemon-side and stream `ToolStarted`/`ToolResult`,
  which raw mode consumes silently. So `list_panes`, `find_in_panes`,
  `read_pane` and `get_terminal_context` all run for real under `--raw`.
  **Consequence: every probe prompt must name a silent tool. If the AI tries
  `run_terminal_command` instead, it gets auto-denied — that is a wasted
  probe, not a defect.**
- **The dispatch-path evidence lives in the per-session JSONL**
  (`~/.daemoneye/var/log/sessions/<id>.jsonl`; each `ask` creates a new one).
  Real lines from an existing log, verbatim shapes to grep for:

  ```json
  {"role":"assistant","content":"…","tool_calls":[{"id":"chatcmpl-tool-b32a13a2cb784ca9","name":"get_terminal_context","arguments":"{\"scope\":\"all\"}"}],"turn":2}
  {"role":"user","content":"","tool_results":[{"tool_call_id":"chatcmpl-tool-ab3781d221bb3315","tool_name":"close_background_window","content":"No background window with pane ID %26 found in this session."}],"turn":6}
  ```

  A `"tool_name":"X"` hit inside `tool_results` proves tool `X` executed
  through the dispatch path and shows exactly what it returned. There is no
  per-silent-tool record in `events.jsonl` and no daemon.log line — the
  session JSONL is the only trail, which is why every probe below greps it.
- **Expected output strings, pinned from source** (re-verify at
  `src/daemon/executor/knowledge/pane.rs:613` and `src/tmux/status.rs:91-104`):
  the `list_panes` foreign section header is exactly
  `Panes in other tmux sessions:`; foreign rows carry `session:<name>` and
  `status:<status>`; status display strings are `running`, `active`, `idle`,
  `idle(<age>)`, `awaiting-input`, `dead(<code>)`, `bell`. The terminal-context
  foreign lines start `[FOREIGN SESSION PANE`.
- **Status-classification timing** (`src/tmux/status.rs:50-77`): a non-shell
  command with no output for ≥ 60 s flips `running` → `awaiting-input`. The
  fixture therefore uses a `while true; do date; sleep 10; done` loop, which
  keeps emitting output and stays `running` indefinitely — do NOT substitute a
  bare `sleep`, it will misclassify and fail CHECK-A2 through no fault of the
  code.
- **One tmux session exists** (`0`, attached). The sweep adds and removes a
  second (`m14`) and one fixture window (`m14fix`).

## Spec

Every task below runs one numbered section of the block in § End-to-end
verification **verbatim** — the sections share the artifact file
`/tmp/e2e-m14-01.txt` and must run in order, in the same shell environment
budget (each section is self-contained; variables do not carry across
sections). Do not improvise replacements for any command.

1. **Rebuild, reinstall, restart** — run § E2E **Section S1**. It builds the
   release binary, stops any running daemon, installs the fresh binary to
   `~/.cargo/bin/daemoneye`, starts the daemon, and proves binary identity by
   comparing sha256 of `target/release/daemoneye`, `~/.cargo/bin/daemoneye`,
   and the running daemon's `/proc/<pid>/exe`. Verdict line:
   `CHECK-S1 binary-identity: OK`.

2. **Lay the pane fixture** — run § E2E **Section S2**. Creates the foreign
   session `m14` carrying the marker string `M14FOREIGNMARK`, and the home
   fixture window `m14fix` with three panes: a shell at prompt, a `date` loop
   (stays `running`), and a dead pane (`remain-on-exit on`, exited with
   code 7).

3. **Run the four dispatch-path probes** — run § E2E **Section S3**. Four
   `daemoneye ask --raw` calls, each prompt naming the tool it must exercise;
   after each, the newest session JSONL is grepped for the `tool_results`
   record. Each probe retries **once** on a missing record (the AI declining
   to call the tool is nondeterminism, not evidence); a second miss writes a
   `FAIL` verdict. Verdict lines: `CHECK-A1 list_panes-dispatch`,
   `CHECK-A2 status-running`, `CHECK-A3 status-dead`, `CHECK-A4 status-shell`,
   `CHECK-B find_in_panes-foreign`, `CHECK-C read_pane-dispatch`,
   `CHECK-D context-foreign`.

4. **Drive the `/panes` inspector** — run § E2E **Section S4**. Starts
   `daemoneye chat` inside fixture pane `m14fix.0` **via `tmux send-keys`**
   (never in your own terminal — chat is interactive and would swallow your
   session), sends `/panes`, captures the pane, and checks the inspector
   rendered the fixture window. Verdict line: `CHECK-E panes-inspector`.

5. **Teardown and gates** — run § E2E **Section S5**. Kills the `m14` session
   and the `m14fix` window (nothing else), then runs the four gates. The
   daemon is deliberately left running — on the current binary, which is the
   milestone's desired end state.

6. **Evaluate the verdicts** — run:
   `grep -c ': FAIL' /tmp/e2e-m14-01.txt` and
   `grep -c ': OK' /tmp/e2e-m14-01.txt`.
   The required result is `0` FAIL and `9` OK. **If any FAIL is present, stop
   and write a blocker Update Log entry naming the failing check** — do not
   edit the block, do not adjust a probe until it passes, do not re-run the
   whole sweep to get a cleaner artifact. A FAIL here is exactly the finding
   this milestone exists to surface.

7. **Capture the end-to-end evidence** — paste the complete
   `/tmp/e2e-m14-01.txt` into a new Update Log entry headed
   `### Update — <date> (end-to-end verification)`, as one fenced block,
   verbatim and unmodified. Then run the self-check in § E2E **Section S6**
   and append its `PASTE MATCH` / `PASTE MISMATCH` line **inside the entry**.
   The server-authored `(complete)` entry does not satisfy this.

## Acceptance criteria

- [ ] `grep -c ': FAIL' /tmp/e2e-m14-01.txt` prints `0`.
- [ ] `grep -c ': OK' /tmp/e2e-m14-01.txt` prints `9` (S1 identity, A1–A4, B,
      C, D, E — count of `verdict` emitters in the block; a different count
      means a section was skipped or run twice).
- [ ] The running daemon is the current binary:
      `test "$(sha256sum /proc/$(tr -dc 0-9 < ~/.daemoneye/var/run/daemoneye.pid)/exe | cut -d' ' -f1)" = "$(sha256sum ~/.cargo/bin/daemoneye | cut -d' ' -f1)" && echo SAME`
      prints `SAME`.
- [ ] The fixture is gone: `tmux has-session -t m14 2>&1` reports an error,
      and `tmux list-windows -a -F '#W' | grep -c m14fix` prints `0`.
- [ ] The Update Log contains a `### Update — <date> (end-to-end
      verification)` entry whose fenced block ends with a line reading
      `PASTE MATCH`.
- [ ] The four gates ran green inside the artifact (Section S5's tails show
      `exit=0` for build, clippy and test).

## Test plan

No new unit tests. This phase ships no code; its deliverable is the live
transcript. The unit coverage for every surface probed here already exists
(M12 phases 01–08) — the point of this phase is precisely the evidence that
unit coverage cannot provide.

## End-to-end verification

Run each section verbatim, in order. All sections append to
`/tmp/e2e-m14-01.txt`. Piped commands record `${PIPESTATUS[0]}`, never `$?`.

**Section S1 — rebuild, reinstall, restart:**

```sh
A=/tmp/e2e-m14-01.txt
: > "$A"
{
echo "== S1 REBUILD-RESTART =="
cargo build --release 2>&1 | tail -3; echo "build exit=${PIPESTATUS[0]}"
daemoneye stop 2>&1 | tail -1
install -m755 target/release/daemoneye ~/.cargo/bin/daemoneye && echo "install: done"
daemoneye daemon 2>&1 | tail -2; echo "daemon-start exit=${PIPESTATUS[0]}"
sleep 2
daemoneye ping 2>&1 | tail -1
daemoneye status 2>&1 | head -8
PID=$(tr -dc 0-9 < ~/.daemoneye/var/run/daemoneye.pid)
H1=$(sha256sum target/release/daemoneye | cut -d' ' -f1)
H2=$(sha256sum ~/.cargo/bin/daemoneye | cut -d' ' -f1)
H3=$(sha256sum "/proc/$PID/exe" | cut -d' ' -f1)
echo "sha256 target=$H1 installed=$H2 running=$H3"
if [ "$H1" = "$H2" ] && [ "$H2" = "$H3" ]; then echo "CHECK-S1 binary-identity: OK"; else echo "CHECK-S1 binary-identity: FAIL"; fi
} >> "$A" 2>&1
```

**Section S2 — pane fixture:**

```sh
A=/tmp/e2e-m14-01.txt
{
echo "== S2 FIXTURE =="
HS=$(tmux list-sessions -F '#S' | grep -vx m14 | head -1)
echo "home-session=$HS"
tmux new-session -d -s m14 -x 120 -y 30
tmux send-keys -t m14 "echo M14FOREIGNMARK" Enter
tmux new-window -d -t "$HS" -n m14fix
tmux split-window -d -t "$HS:m14fix"
tmux split-window -d -t "$HS:m14fix"
tmux send-keys -t "$HS:m14fix.1" 'while true; do date; sleep 10; done' Enter
tmux set-option -p -t "$HS:m14fix.2" remain-on-exit on
tmux send-keys -t "$HS:m14fix.2" 'exit 7' Enter
sleep 6
tmux list-panes -t "$HS:m14fix" -F '#{pane_id} #{pane_current_command} dead=#{pane_dead}'
tmux list-panes -t m14 -F '#{pane_id} #{pane_current_command}'
} >> "$A" 2>&1
```

(The `sleep 6` lets the daemon's 2 s cache poll pick the new panes up before
any probe runs.)

**Section S3 — dispatch-path probes:**

```sh
A=/tmp/e2e-m14-01.txt
SDIR=~/.daemoneye/var/log/sessions
{
echo "== S3 PROBES =="

echo "-- probe A: list_panes --"
daemoneye ask --raw 'Use the list_panes tool now and then reproduce its output for the m14fix window and the foreign-session section verbatim.' 2>&1 | tail -20; echo "ask exit=${PIPESTATUS[0]}"
SL=$(ls -t "$SDIR"/*.jsonl | grep -v archive | head -1)
if ! grep -q '"tool_name":"list_panes"' "$SL"; then
  echo "-- probe A retry --"
  daemoneye ask --raw 'Call the list_panes tool. Report its output verbatim.' 2>&1 | tail -20; echo "ask exit=${PIPESTATUS[0]}"
  SL=$(ls -t "$SDIR"/*.jsonl | grep -v archive | head -1)
fi
echo "session-log=$SL"
if grep -q '"tool_name":"list_panes"' "$SL"; then echo "CHECK-A1 list_panes-dispatch: OK"; else echo "CHECK-A1 list_panes-dispatch: FAIL"; fi
if grep '"tool_name":"list_panes"' "$SL" | grep -q 'status:running'; then echo "CHECK-A2 status-running: OK"; else echo "CHECK-A2 status-running: FAIL"; fi
if grep '"tool_name":"list_panes"' "$SL" | grep -q 'status:dead('; then echo "CHECK-A3 status-dead: OK"; else echo "CHECK-A3 status-dead: FAIL"; fi
if grep '"tool_name":"list_panes"' "$SL" | grep -Eq 'status:(active|idle)'; then echo "CHECK-A4 status-shell: OK"; else echo "CHECK-A4 status-shell: FAIL"; fi

echo "-- probe B: find_in_panes --"
daemoneye ask --raw 'Use the find_in_panes tool with pattern M14FOREIGNMARK and scope "all". Report every match with its session and pane.' 2>&1 | tail -15; echo "ask exit=${PIPESTATUS[0]}"
SL=$(ls -t "$SDIR"/*.jsonl | grep -v archive | head -1)
if ! grep -q '"tool_name":"find_in_panes"' "$SL"; then
  echo "-- probe B retry --"
  daemoneye ask --raw 'Call the find_in_panes tool with pattern M14FOREIGNMARK and scope set to "all".' 2>&1 | tail -15; echo "ask exit=${PIPESTATUS[0]}"
  SL=$(ls -t "$SDIR"/*.jsonl | grep -v archive | head -1)
fi
echo "session-log=$SL"
if grep '"tool_name":"find_in_panes"' "$SL" | grep -q 'session:m14'; then echo "CHECK-B find_in_panes-foreign: OK"; else echo "CHECK-B find_in_panes-foreign: FAIL"; fi

echo "-- probe C: read_pane --"
FP=$(tmux list-panes -t m14 -F '#{pane_id}' | head -1)
echo "foreign-pane=$FP"
daemoneye ask --raw "Use the read_pane tool on pane $FP and reproduce what it returns." 2>&1 | tail -15; echo "ask exit=${PIPESTATUS[0]}"
SL=$(ls -t "$SDIR"/*.jsonl | grep -v archive | head -1)
if ! grep -q '"tool_name":"read_pane"' "$SL"; then
  echo "-- probe C retry --"
  daemoneye ask --raw "Call the read_pane tool with pane_id $FP." 2>&1 | tail -15; echo "ask exit=${PIPESTATUS[0]}"
  SL=$(ls -t "$SDIR"/*.jsonl | grep -v archive | head -1)
fi
echo "session-log=$SL"
if grep '"tool_name":"read_pane"' "$SL" | grep -q 'M14FOREIGNMARK'; then echo "CHECK-C read_pane-dispatch: OK"; else echo "CHECK-C read_pane-dispatch: FAIL"; fi

echo "-- probe D: get_terminal_context scope all --"
daemoneye ask --raw 'Use the get_terminal_context tool with scope "all" and summarize which sessions it reports.' 2>&1 | tail -15; echo "ask exit=${PIPESTATUS[0]}"
SL=$(ls -t "$SDIR"/*.jsonl | grep -v archive | head -1)
if ! grep -q '"tool_name":"get_terminal_context"' "$SL"; then
  echo "-- probe D retry --"
  daemoneye ask --raw 'Call the get_terminal_context tool with scope set to "all".' 2>&1 | tail -15; echo "ask exit=${PIPESTATUS[0]}"
  SL=$(ls -t "$SDIR"/*.jsonl | grep -v archive | head -1)
fi
echo "session-log=$SL"
if grep '"tool_name":"get_terminal_context"' "$SL" | grep -q 'FOREIGN SESSION PANE'; then echo "CHECK-D context-foreign: OK"; else echo "CHECK-D context-foreign: FAIL"; fi
} >> "$A" 2>&1
```

**Section S4 — `/panes` inspector via send-keys:**

```sh
A=/tmp/e2e-m14-01.txt
{
echo "== S4 PANES-INSPECTOR =="
HS=$(tmux list-sessions -F '#S' | grep -vx m14 | head -1)
tmux send-keys -t "$HS:m14fix.0" 'daemoneye chat' Enter
sleep 5
tmux send-keys -t "$HS:m14fix.0" '/panes' Enter
sleep 4
tmux capture-pane -p -t "$HS:m14fix.0" -S -60 | tail -40
if tmux capture-pane -p -t "$HS:m14fix.0" -S -60 | grep -q 'm14fix'; then echo "CHECK-E panes-inspector: OK"; else echo "CHECK-E panes-inspector: FAIL"; fi
} >> "$A" 2>&1
```

**Section S5 — teardown and gates:**

```sh
A=/tmp/e2e-m14-01.txt
{
echo "== S5 TEARDOWN-AND-GATES =="
HS=$(tmux list-sessions -F '#S' | grep -vx m14 | head -1)
tmux kill-session -t m14
tmux kill-window -t "$HS:m14fix"
tmux has-session -t m14 2>&1
echo "m14fix windows left: $(tmux list-windows -a -F '#W' | grep -c m14fix)"
cargo fmt --all --check 2>&1 | tail -1; echo "fmt exit=${PIPESTATUS[0]}"
cargo build 2>&1 | tail -1; echo "build exit=${PIPESTATUS[0]}"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1; echo "clippy exit=${PIPESTATUS[0]}"
cargo test 2>&1 | grep -E '^test result'; echo "test exit=${PIPESTATUS[0]}"
echo "== END =="
} >> "$A" 2>&1
```

**Section S6 — paste self-check (run AFTER pasting the artifact into the
Update Log entry):**

```sh
awk '/^### Update.*end-to-end verification/{f=1} f&&/^```/{c++; next} f&&c==1{print} f&&c==2{exit}' \
  docs/dev/milestones/M14-live-verification/phase-01-scripted-live-sweep.md > /tmp/pasted-m14-01.txt
if diff -q /tmp/pasted-m14-01.txt /tmp/e2e-m14-01.txt >/dev/null; then echo "PASTE MATCH"; else echo "PASTE MISMATCH"; diff /tmp/pasted-m14-01.txt /tmp/e2e-m14-01.txt | head -20; fi
```

Append the verdict line inside the Update Log entry, after the fence.

## Authorizations

This phase is authorized to:

- Replace the installed binary at `~/.cargo/bin/daemoneye` with the freshly
  built one, and stop/start the daemon (leaving it running at the end).
- Create and destroy tmux session `m14` and window `m14fix` — and **nothing
  else in tmux**: no other window, pane or session may be killed, resized or
  written to.
- Spend up to ~10 AI turns via `daemoneye ask` (4 probes + up to 4 retries).

## Out of scope

- **Any change under `src/`, `tests/`, or `Cargo.*`** — if a probe surfaces a
  defect, that is a *successful* outcome of this phase: record the FAIL
  verdict, write the blocker entry, stop. The fix is a later phase.
- The `tmux_control` approval round trip and the per-tool budget caps —
  phase-02.
- `daemoneye chat` in the executor's own terminal — interactive; send-keys
  into the fixture pane only.
- Re-running a probe more than the one scripted retry, or editing a probe
  prompt to coax a different result.

## Update Log

### Update — 2026-08-10 (drafted)

Drafted by the architect. Environment facts probed live the same day (daemon
down, stale PID 76979, installed binary dated 07:26, one tmux session `0`);
source facts re-derived at `ask.rs:115-221`, `pane.rs:613`,
`status.rs:50-104`, `instance.rs`. Verdict-line count in the block re-counted
at drafting: 9 emitters (S1, A1–A4, B, C, D, E) — matches the acceptance
criterion. The session-JSONL evidence shapes are quoted from a real log, not
composed. The ask-dependent criteria cannot be pre-run without spending the
sweep itself; their mechanics (newest-log recipe, grep patterns) were each
executed against existing session logs at drafting.

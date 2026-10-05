# Phase 10e: live-close — attach to a running ghost, pause it, resume it, detach, and replay its log, with no tmux server anywhere

**Milestone:** M23 — Client decoupling & shell UX
**Status:** in-progress
**Depends on:** 10b (ghost start without tmux), 10c (local ghost PTY shells), 10d (pause stops the shell), and rows 02–09
**Estimated diff:** 0 lines of code; this doc's Update Log and the milestone README row
**Tags:** language=rust, kind=verification, size=s, architect-run

## Goal

The milestone's stated exit, run for real:

> from a terminal with **no tmux server running**, pick a running ghost from
> `/agents`, attach, pause it (`Ctrl-p`), resume it, detach (`Ctrl-d`), then
> `/log` replay of a finished shell. […] `tmux ls` under the client's and
> the daemon's `TMUX_TMPDIR` must report no server before, during and after
> the run.

This phase is **architect-run** with the PE present (the M14 convention): it
exercises the chat client in a real terminal, which no executor can drive. It
exists as a phase so the obligation is a tracked `## Spec` task with pasted
evidence, not a close-out promise.

## Architecture references

- `docs/dev/milestones/M23-client-decoupling/README.md` § Exit criteria, the
  first two bullets; § "PE decisions at sign-off (2026-09-30)" item 2: the
  live close runs in **a private tmux server used only as the terminal
  emulator**, with the client's `TMUX`/`TMUX_PANE` unset and `TMUX_TMPDIR`
  isolated.
- `docs/design/daemoneye-2.0.md` § 2.6 (the chat client), the M23 roadmap row.

## Pre-flight

1. The PE is present and watching. To see the client as it is driven, the PE
   attaches **read-only** to the emulator after S1:
   `tmux -S /tmp/de10e-emu.sock attach -r` (detach with the tmux prefix + `d`).
   That server is the terminal emulator, not the one under test: its socket
   is outside the isolated `TMUX_TMPDIR`.
2. The repo is clean at the commit named in § Current state.
3. The executor endpoint (`brain:8000`) is reachable. The throwaway daemon's
   model is the same local GLM, so the run costs no API credit.
4. Nothing else listens on port **19418** (the throwaway webhook).

## Current state

Measured on **2026-10-05** at commit `2f8a7a53`.

- **The whole § Spec was rehearsed three times** against this commit. The last
  full run printed every value the criteria below require; its capture is in
  § End-to-end verification as the rehearsal reference. The rehearsals found
  and drove two fixes (10c, 10d) and two block fixes, both folded in here:
  - **`tmux ls` itself creates `$TMUX_TMPDIR/tmux-<uid>/`.** The client makes
    the socket directory when it tries to connect, so counting `tmux-*`
    directories is polluted by the check. The honest signal is that **no
    server socket** exists, `$R/tmux-<uid>/default`, plus `tmux ls` exiting
    non-zero.
  - **The daemon takes a moment to exit after `stop`**, so the leftover-process
    count waits up to 10 s.
- **Why a throwaway daemon**, not the PE's own: the live config keeps
  `tmux_session = "daemoneye"` (PE decision 1), which starts a tmux session
  by design. The run uses an isolated `HOME` and `TMUX_TMPDIR`
  (`/tmp/de10e-live`) with `tmux_session = ""`, the release binary built at
  HEAD, and the local model, so nothing of the PE's setup is touched.
- **The ghost** is triggered through the throwaway daemon's webhook with a
  one-step runbook: a 60-tick loop, `wait: false`. Since 10c it runs in a PTY
  shell the ghost owns. `/agents` lists that shell first.
- **Timing:** the ghost's command is capped at 300 s, so the 10 s pause in S2
  is far inside its budget.
- **Keys** are sent to the emulator with `tmux send-keys` (`C-p`, `C-d`,
  `Enter`, `Space`, `Escape`). The client drops input while it streams, so
  S1 waits for the opening greeting to finish before anything else is sent.

## Spec

Every block below is run by the architect from this repo's root, in order,
with the PE watching. Each block appends to `/tmp/e2e-m23-10e.txt`. The whole
file is pasted into the Update Log as the first fenced block of the
`(end-to-end verification)` entry.

### S0 — build the release binary and the fixtures

```sh
cd /home/matt/src/daemoneye
A=/tmp/e2e-m23-10e.txt; R=/tmp/de10e-live; EMU=/tmp/de10e-emu.sock
: > "$A"
{
echo "== S0 BUILD-FIXTURES =="
git rev-parse --short HEAD
cargo build --release 2>&1 | tail -1 | sed 's/ in [0-9.]*s$//'; echo "build exit=${PIPESTATUS[0]}"
BIN=/home/matt/src/daemoneye/target/release/daemoneye
[ -e "$R" ] && rm -rf "$R"; mkdir -p "$R"
env -u TMUX -u TMUX_PANE HOME=$R TMUX_TMPDIR=$R $BIN setup >/dev/null 2>&1; echo "setup exit=$?"
printf '[models.default]\nprovider = "ollama"\nmodel = "local-inference-lab/GLM-5.3-Flash-NVFP4-Spark"\nbase_url = "http://brain:8000/v1"\ncontext_window_tokens = 262142\n\n[daemon]\ntmux_session = ""\n\n[webhook]\nenabled = true\nport = 19418\nbind_addr = "127.0.0.1"\n' > $R/.daemoneye/etc/config.toml
mkdir -p $R/.daemoneye/runbooks
printf -- '---\nenabled: true\nmax_ghost_turns: 4\n---\n# Runbook: live-ticks\n\n## Alert Criteria\n- Any LiveTicks alert. This is a manual live check: always trigger.\n\n## Remediation Steps\n1. Run exactly this command once with run_terminal_command and wait false: `for i in $(seq 1 60); do echo tick $i; sleep 1; done`\n2. Report the last tick line you saw and stop.\n' > $R/.daemoneye/runbooks/live-ticks.md
N=$(grep -cx 'tmux_session = ""' $R/.daemoneye/etc/config.toml)
echo "fixtures: tmux_session-off=$N runbook=$(ls $R/.daemoneye/runbooks/live-ticks.md | wc -l)"
} >> "$A" 2>&1
cat "$A"
```

### S1 — start the daemon and the client, with no tmux server

```sh
cd /home/matt/src/daemoneye
A=/tmp/e2e-m23-10e.txt; R=/tmp/de10e-live; EMU=/tmp/de10e-emu.sock
BIN=/home/matt/src/daemoneye/target/release/daemoneye
iso() { env -u TMUX -u TMUX_PANE HOME=$R TMUX_TMPDIR=$R "$@"; }
{
echo "== S1 START =="
iso tmux ls >/dev/null 2>&1; echo "tmux-before: tmux-ls-exit=$? server-socket=$([ -S $R/tmux-$(id -u)/default ] && echo yes || echo no)"
iso $BIN daemon >/dev/null 2>&1; echo "daemon-start exit=$?"
iso $BIN ping 2>&1 | tail -1 | sed 's/\x1b\[[0-9;]*m//g'
tmux -S $EMU new-session -d -s emu -x 160 -y 45; echo "emulator exit=$?"
tmux -S $EMU send-keys -t emu: "env -u TMUX -u TMUX_PANE HOME=$R TMUX_TMPDIR=$R $BIN chat" Enter
sleep 5; for i in $(seq 1 60); do tmux -S $EMU capture-pane -p -t emu: | grep -q 'gazing\|thinking' || break; sleep 2; done
tmux -S $EMU capture-pane -p -t emu: | grep -c 'AGENTIC OPERATOR' | sed 's/^/client-banner=/'
} >> "$A" 2>&1
tail -n 6 "$A"
```

After S1 the PE attaches read-only (Pre-flight 1).

### S2 — the ghost: pick it in `/agents`, attach, pause, resume, detach

```sh
cd /home/matt/src/daemoneye
A=/tmp/e2e-m23-10e.txt; R=/tmp/de10e-live; EMU=/tmp/de10e-emu.sock
iso() { env -u TMUX -u TMUX_PANE HOME=$R TMUX_TMPDIR=$R "$@"; }
cap() { tmux -S $EMU capture-pane -p -t emu: | sed 's/[[:space:]]*$//' | grep -v '^$'; }
{
echo "== S2 ATTACH-PAUSE-RESUME-DETACH =="
curl -s -o /dev/null -w 'webhook=%{http_code}\n' -X POST -H 'Content-Type: application/json' -d '{"status":"firing","alerts":[{"status":"firing","labels":{"alertname":"LiveTicks"},"annotations":{"summary":"manual live check, always trigger"},"fingerprint":"fp-live-ticks"}]}' http://127.0.0.1:19418/webhook
for i in $(seq 1 90); do ls $R/.daemoneye/var/log/shells/*.cast >/dev/null 2>&1 && break; sleep 2; done; sleep 4
tmux -S $EMU send-keys -t emu: '/agents' Enter; sleep 3
cap | grep -c '› s[0-9]*  host:local  owner:ghost:ghost-live-ticks-' | sed 's/^/picker-ghost-row=/'
tmux -S $EMU send-keys -t emu: Enter; sleep 3
cap | grep -c '^attached s[0-9]* · .*ctrl-p pause' | sed 's/^/attached-status=/'
T1=$(cap | grep -o '^tick [0-9]*' | tail -1); echo "attached-last=$T1"
tmux -S $EMU send-keys -t emu: C-p; sleep 3
P1=$(cap | grep -o '^tick [0-9]*' | tail -1)
cap | grep -c '^attached s[0-9]* · PAUSED' | sed 's/^/paused-status=/'
sleep 10
P2=$(cap | grep -o '^tick [0-9]*' | tail -1)
echo "paused-last: $P1 then $P2 ten seconds later"
if [ -n "$P1" ] && [ "$P1" = "$P2" ]; then echo "CHECK-S2 pause-holds: OK"; else echo "CHECK-S2 pause-holds: FAIL"; fi
iso tmux ls >/dev/null 2>&1; echo "tmux-during: tmux-ls-exit=$? server-socket=$([ -S $R/tmux-$(id -u)/default ] && echo yes || echo no)"
tmux -S $EMU send-keys -t emu: C-p; sleep 4
R1=$(cap | grep -o '^tick [0-9]*' | tail -1); echo "resumed-last=$R1"
cap | grep -c '^attached s[0-9]* · .*ctrl-p pause' | sed 's/^/resumed-status=/'
tmux -S $EMU send-keys -t emu: C-d; sleep 3
cap | grep -c '^attached s' | sed 's/^/attached-after-detach=/'
} >> "$A" 2>&1
tail -n 16 "$A"
```

### S3 — read the evidence where it lives

```sh
cd /home/matt/src/daemoneye
A=/tmp/e2e-m23-10e.txt; R=/tmp/de10e-live
{
echo "== S3 EVIDENCE =="
for i in $(seq 1 120); do grep -hq '"event":"ghost_complete"' $R/.daemoneye/var/log/events/*.jsonl 2>/dev/null && break; sleep 2; done
grep -h '"event":"ghost_complete"' $R/.daemoneye/var/log/events/*.jsonl | wc -l | sed 's/^/ghost-complete=/'
grep -h '"event":"ghost_shells_closed"' $R/.daemoneye/var/log/events/*.jsonl | wc -l | sed 's/^/ghost-shells-closed=/'
grep -h '"event":"ghost_start"' $R/.daemoneye/var/log/events/*.jsonl | grep -o '"tmux_session":"[^"]*"' | sed 's/^/ghost-start /'
C=$(ls $R/.daemoneye/var/log/shells/*.cast | head -1); echo "cast=$(basename $C)"
python3 - "$C" <<'PY'
import json,sys,re
ts=[]
for l in open(sys.argv[1]):
    if not l.startswith('['): continue
    t,k,p=json.loads(l)
    if k=='o':
        for m in re.finditer(r'tick (\d+)',p): ts.append((t,int(m.group(1))))
gaps=[(ts[i+1][0]-ts[i][0],ts[i][1]) for i in range(len(ts)-1)]
g=max(gaps)
print("cast-ticks=%d last=%d" % (len(ts), ts[-1][1]))
print("cast-maxgap=%ds after tick %d" % (int(g[0]), g[1]))
PY
grep -c 'Stopped' "$C" | sed 's/^/cast-stopped-lines=/'
F=$(ls $R/.daemoneye/var/log/sessions/ghost-*.jsonl | grep -v archive | head -1)
python3 - "$F" <<'PY'
import json,sys,re
for l in open(sys.argv[1]):
    m=json.loads(l)
    for r in (m.get('tool_results') or []):
        print("tool-result-last-line=%s" % r['content'].strip().split('\n')[-1])
    if m.get('role')=='user' and 'paused' in m.get('content',''):
        print("pause-notice=%s" % re.sub(r'for \d+ s', 'for N s', m['content']))
PY
} >> "$A" 2>&1
tail -n 11 "$A"
```

### S4 — `/log` replay of the finished shell

```sh
cd /home/matt/src/daemoneye
A=/tmp/e2e-m23-10e.txt; R=/tmp/de10e-live; EMU=/tmp/de10e-emu.sock
cap() { tmux -S $EMU capture-pane -p -t emu: | sed 's/[[:space:]]*$//' | grep -v '^$'; }
{
echo "== S4 LOG-REPLAY =="
LID=$(basename $(ls $R/.daemoneye/var/log/shells/*.cast | head -1) .cast)
tmux -S $EMU send-keys -t emu: "/log $LID" Enter; sleep 3
cap | grep -c "^log $LID · .*space replay" | sed 's/^/log-viewer=/'
tmux -S $EMU send-keys -t emu: Space; sleep 3
X1=$(cap | grep -o 'replay [0-9.]*s/' | tail -1); sleep 4
X2=$(cap | grep -o 'replay [0-9.]*s/' | tail -1)
echo "replay-progress: $X1 then $X2"
if [ -n "$X1" ] && [ -n "$X2" ] && [ "$X1" != "$X2" ]; then echo "CHECK-S4 replay-advances: OK"; else echo "CHECK-S4 replay-advances: FAIL"; fi
tmux -S $EMU send-keys -t emu: Escape; sleep 1; tmux -S $EMU send-keys -t emu: Escape; sleep 1
} >> "$A" 2>&1
tail -n 4 "$A"
```

### S5 — clean up, check nothing is left, and paste

```sh
cd /home/matt/src/daemoneye
A=/tmp/e2e-m23-10e.txt; R=/tmp/de10e-live; EMU=/tmp/de10e-emu.sock
BIN=/home/matt/src/daemoneye/target/release/daemoneye
iso() { env -u TMUX -u TMUX_PANE HOME=$R TMUX_TMPDIR=$R "$@"; }
{
echo "== S5 CLEANUP =="
tmux -S $EMU send-keys -t emu: '/quit' Enter; sleep 2
iso tmux ls >/dev/null 2>&1; echo "tmux-after: tmux-ls-exit=$? server-socket=$([ -S $R/tmux-$(id -u)/default ] && echo yes || echo no)"
iso $BIN stop 2>&1 | tail -1 | sed 's/\x1b\[[0-9;]*m//g'
tmux -S $EMU kill-server; echo "emulator stopped exit=$?"
sleep 2; echo "stopped-procs=$(ps -eo stat= | grep -c '^T')"
for i in $(seq 1 10); do [ "$(pgrep -fc "$R")" = 0 ] && break; sleep 1; done
echo "leftover-procs=$(pgrep -fc "$R")"
} >> "$A" 2>&1
cat "$A"
```

Then paste the whole of `/tmp/e2e-m23-10e.txt` as the first fenced block of an
Update Log entry titled `### Update — <date> (end-to-end verification)`, and
run:

```sh
D=docs/dev/milestones/M23-client-decoupling/phase-10e-live-close.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m23-10e.txt
diff /tmp/pasted-m23-10e.txt /tmp/e2e-m23-10e.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

The line it prints goes into the entry on its own line.

## Acceptance criteria

Every line is read from `/tmp/e2e-m23-10e.txt` as pasted. Against the current
tree none can be produced, because no throwaway daemon or ghost exists, so
each fails today.

- [ ] **No tmux server, before, during and after:** `tmux-before`,
      `tmux-during` and `tmux-after` each read
      `tmux-ls-exit=1 server-socket=no`, and S3's `ghost-start` reads
      `"tmux_session":""`.
- [ ] **The client runs:** `client-banner=1`, `daemon-start exit=0`.
- [ ] **`/agents` picks the running ghost:** `picker-ghost-row=1` (the
      selected row is a `host:local  owner:ghost:ghost-live-ticks-…` shell).
- [ ] **Attach:** `attached-status=1`.
- [ ] **Pause holds:** `paused-status=1` and `CHECK-S2 pause-holds: OK` (the
      last tick is the same 10 s apart); in the cast, `cast-maxgap` is
      **≥ 10 s** and `cast-stopped-lines=0`.
- [ ] **Resume:** `resumed-status=1`, and `resumed-last` is a later tick than
      `paused-last`.
- [ ] **Detach:** `attached-after-detach=0`.
- [ ] **The ghost finished cleanly and was told the truth:**
      `ghost-complete=1`, `ghost-shells-closed=1`, `cast-ticks=60 last=60`,
      `tool-result-last-line=tick 60`, and the `pause-notice=` line names
      shell `s0` and the attached-mode pause.
- [ ] **`/log` replay of the finished shell:** `log-viewer=1` and
      `CHECK-S4 replay-advances: OK`.
- [ ] **Nothing left behind:** `stopped-procs=0`, `leftover-procs=0`.
- [ ] The `(end-to-end verification)` entry holds the file as its first
      fenced block and the literal `PASTE MATCH` on its own line:
      ``grep -cx 'PASTE MATCH' <this doc>`` → **1**.
- [ ] `git status --short` shows only this doc and the milestone README
      changed; **no `src/` change** (`git diff --stat HEAD -- src/` empty).

## Mutations

None. This phase changes no code. Its witnesses against a vacuous pass:
- `CHECK-S2 pause-holds` needs a tick line on screen both times. An empty
  capture prints `FAIL`.
- `cast-maxgap ≥ 10 s` is read from the recording, not the screen.
- `server-socket=no` is checked three times, beside `tmux ls`'s exit code.

## Test plan

No new tests. The live run exercises, through the real binaries:
- 02: no-tmux startup;
- 03: the shell IPC;
- 06: `/log` replay;
- 07: `/agents` and attach;
- 08: `Ctrl-p`/`Ctrl-d` and the pause notice;
- 10b–10d: a ghost without tmux, its PTY shell, and a pause that holds.

## End-to-end verification

The six blocks in § Spec **are** the end-to-end verification. There is no
separate block. `/tmp/e2e-m23-10e.txt` is the capture, pasted per S5.

**Rehearsal reference** (the last full rehearsal at `2f8a7a53`, for
comparison; tick numbers, the cast id and gap seconds vary run to run):

```
== S0 BUILD-FIXTURES ==
2f8a7a53
    Finished `release` profile [optimized] target(s)
build exit=0
setup exit=0
fixtures: tmux_session-off=1 runbook=1
== S1 START ==
tmux-before: tmux-ls-exit=1 server-socket=no
daemon-start exit=0
Daemon is running.
emulator exit=0
client-banner=1
== S2 ATTACH-PAUSE-RESUME-DETACH ==
webhook=200
picker-ghost-row=1
attached-status=1
attached-last=tick 11
paused-status=1
paused-last: tick 11 then tick 11 ten seconds later
CHECK-S2 pause-holds: OK
tmux-during: tmux-ls-exit=1 server-socket=no
resumed-last=tick 15
resumed-status=1
attached-after-detach=0
== S3 EVIDENCE ==
ghost-complete=1
ghost-shells-closed=1
ghost-start "tmux_session":""
cast=s0-1791168391.cast
cast-ticks=60 last=60
cast-maxgap=13s after tick 11
cast-stopped-lines=0
tool-result-last-line=tick 60
pause-notice=[The user paused shell s0 for N s from attached mode. It is running again.]
== S4 LOG-REPLAY ==
log-viewer=1
replay-progress: replay 3.0s/ then replay 7.0s/
CHECK-S4 replay-advances: OK
== S5 CLEANUP ==
tmux-after: tmux-ls-exit=1 server-socket=no
Daemon stopped.
emulator stopped exit=0
stopped-procs=0
leftover-procs=0
```

**If any step fails, stop and record the criterion unmet**, with the capture
as it is. Do not describe what it would have printed. A defect found here is
filed as a bug against the row that owns the behaviour.

## Authorizations

- `/tmp/de10e-live` (the throwaway `HOME` and `TMUX_TMPDIR`) and
  `/tmp/de10e-emu.sock` (the emulator server): created and removed.
- `target/release/daemoneye`: built at HEAD. Nothing is installed, and the
  PE's own daemon and config are not touched.
- This phase doc's Update Log and the milestone README's phase-table row.
- **No file under `src/`, `assets/` or `tests/`.**

## Out of scope

- **The sandboxed ghost** (deferred to M26, PE decision 2026-10-05).
- **A real alacritty window.** PE decision 2: the private emulator stands in.
- **The README and the retrospective.** Both belong to the milestone close.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-05 (started)

Architect-run, PE at the keyboard (the PE issued `/rexymcp:dispatch phase-10e`,
read as the go signal; no executor was dispatched). Status flipped
`todo` → `in-progress`; README row to match. Running S0–S5.

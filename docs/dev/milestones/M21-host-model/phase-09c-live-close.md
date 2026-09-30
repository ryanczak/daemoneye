# Phase 09c: live-close — a remote sudo edit from chat on `pinky`, with the password in no log

**Milestone:** M21 — Host model & SSH
**Status:** done
**Depends on:** phase-09b (`edit_file { host, sudo: true }`), phase-09a
(mode and owner kept), phase-08e (connect first), phase-01b (`"i"` events
scrubbed at a secret prompt)
**Estimated diff:** 0 lines of code; one live config entry; this doc's Update Log
**Tags:** language=rust, kind=verification, size=s, architect-run

## Goal

The milestone's stated exit, run for real: from a `daemoneye chat` turn, the
AI edits a **root-owned** file on `pinky` with `edit_file { host: "pinky",
sudo: true }`. The PE approves the edit in the panel that says it runs as
root, types pinky's sudo password when the chat asks, and the edit lands.
Then the evidence is read where it lives:

- the host shell's **cast log** carries the marker-delimited sudo edit and
  its `DE_OK: Edited` line;
- **no `"i"` event carries the password** — the answer is logged as
  `[redacted]`, and a grep for the real password over the cast, the session
  JSONL and `daemon.log` finds nothing;
- the file on pinky holds the new content and is **still `root 640`**;
- **ControlMaster reuse is timed**: the master 08e established is running
  afterwards, and `ssh -O check` answers in milliseconds.

This phase is **architect-run** with the PE present (the M14 convention):
the sudo password is the PE's, so no executor can run it. It exists as a
phase so the obligation is a tracked `## Spec` task with pasted evidence,
not a close-out promise.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.2 (remote file ops through the host's
  shell), § 3 (`"i"`-event scrubbing; credentials never reach the shell log),
  and the M21 row's exit in the roadmap table.
- `docs/dev/milestones/M21-host-model/README.md` § Exit criteria, first
  bullet.
- M14's live recipe (`docs/dev/milestones/M14-live-verification/phase-03-*.md`
  § S1): rebuild, reinstall, restart, sha256-compare — reused below.

## Pre-flight

1. The PE is present at the terminal and knows pinky's sudo password.
2. The repo is clean at the commit named in § Current state, and
   `cargo test --lib` is green (**2088 / 0 / 14**).
3. `ssh -o BatchMode=yes pinky true` exits 0 (key auth works).
4. The tmux session this terminal runs in is **attached** (chat hangs in a
   clientless session — M14). Its name is numeric (`0`), so every tmux target
   uses the colon form `-t "0:"`.

## Current state

Measured on **2026-09-30** at commit `f91a114f`.

- **The installed binary is stale.** `~/.cargo/bin/daemoneye` is dated
  **Sep 18** (pre-M21); `cargo` reports `v1.0.0-534-gf91a114f` at HEAD. The
  daemon is **not running** (`daemoneye status`: stale PID file). S1 rebuilds
  and reinstalls, then compares sha256 (M14 S1).
- **The live config names no host.** `~/.daemoneye/etc/config.toml` has no
  `[hosts.pinky]` entry (the only `hosts` hit is a comment on `max_turns`).
  `host_entry` (`src/shell/session.rs:205`) calls `Config::load()` on every
  call, so adding the entry needs no restart — but S1 restarts anyway for
  the binary. `[execution] backend` is absent and **defaults to `"pty"`**
  (`src/config/types.rs:646`), so no backend line is needed.
- **The scrub arms on the sentinel.** `looks_like_secret_prompt`
  (`src/shell/backend.rs`) matches `password` and `de-sudo-prompt-`, and
  pinky's prompt over the daemon's PTY is `[sudo: [de-sudo-prompt-N]] Password: *`
  (measured 2026-09-22). While armed, every input byte is logged as the
  `"i"` payload `[redacted]` until a `\n`, `\r` or Ctrl-C
  (`backend.rs:157-166`).
- **No ControlMaster is up.** `~/.daemoneye/var/run/ssh/` is empty.
- **Cast logs** live at `~/.daemoneye/var/log/shells/<id>-<started>.cast`
  (`src/shell/launch.rs:41`), asciicast v2: one JSON header line, then
  `[t, "i"|"o"|"m", "payload"]` lines. The edit's shell is owned by
  `chat:edit_file` (09b keeps 07b's owner key), and 09b's sudo run goes
  through `run_in_shell_with_prompts`, so the cast records the
  marker-wrapped sudo line as an `"i"` event and the prompt exchange after
  it.
- **Session JSONL** lives at `~/.daemoneye/var/log/sessions/<id>.jsonl`;
  an assistant line carries `"tool_calls":[{"name":"edit_file","arguments":"{…}"}]`
  and `"tool_results":[{"tool_name":"edit_file","content":"…"}]`.
- **pinky:** `/tmp` is sticky `1777`, `python3` at `/usr/bin/python3`, the
  daemon shell's umask is `0002`, and sudo-rs 0.2.13 requires a password
  (`sudo -n true` → "interactive authentication is required"). Its tickets
  are per-tty, so a `sudo` the PE types over a separate `ssh -t` does **not**
  warm the daemon's shell — the chat run **will** prompt.

## Spec

Every block below is run by the architect from this repo's root, in this
attached tmux session, with the PE at the keyboard for S2. Each block appends
to `/tmp/e2e-09c.txt`; the whole file is pasted into the Update Log as the
first fenced block of the `(end-to-end verification)` entry.

### S0 — fixtures

1. Add the host entry to the live config if it is missing (kept afterwards:
   pinky is the PE-named live host):

   ```sh
   grep -q '^\[hosts.pinky\]' ~/.daemoneye/etc/config.toml || printf '\n[hosts.pinky]\nssh = "pinky"\n' >> ~/.daemoneye/etc/config.toml
   grep -n -A1 '^\[hosts.pinky\]' ~/.daemoneye/etc/config.toml
   ```

2. Create the root-owned target on pinky. **The PE types pinky's sudo
   password here**, in the architect's own terminal (`! ` prefix), never in
   any file:

   ```sh
   ssh -t pinky "sudo sh -c 'printf \"a\\nold-line\\nb\\n\" > /tmp/de-09c-root.txt && chown root:root /tmp/de-09c-root.txt && chmod 640 /tmp/de-09c-root.txt' && stat -c 'fixture=%U:%a' /tmp/de-09c-root.txt"
   ```

   Expected: `fixture=root:640`.

### S1 — rebuild, reinstall, restart (M14 S1, verbatim shape)

```sh
A=/tmp/e2e-09c.txt
: > "$A"
{
echo "== S1 REBUILD-RESTART =="
git rev-parse --short HEAD
cargo build --release 2>&1 | tail -1; echo "build exit=${PIPESTATUS[0]}"
daemoneye stop 2>&1 | tail -1 | sed 's/\x1b\[[0-9;]*m//g'
install -m755 target/release/daemoneye ~/.cargo/bin/daemoneye && echo "install: done"
daemoneye daemon 2>&1 | tail -2 | sed 's/\x1b\[[0-9;]*m//g'; echo "daemon-start exit=${PIPESTATUS[0]}"
sleep 2
daemoneye ping 2>&1 | tail -1 | sed 's/\x1b\[[0-9;]*m//g'
H1=$(sha256sum target/release/daemoneye | cut -d' ' -f1)
H2=$(sha256sum ~/.cargo/bin/daemoneye | cut -d' ' -f1)
if [ "$H1" = "$H2" ]; then echo "CHECK-S1 binary-identity: OK"; else echo "CHECK-S1 binary-identity: FAIL"; fi
ls ~/.daemoneye/var/run/ssh/ | wc -l | sed 's/^/masters-before=/'
touch /tmp/de-09c-mark
} >> "$A" 2>&1
tail -n 12 "$A"
```

Expected: `build exit=0`, `install: done`, `daemon-start exit=0`, a ping
reply, `CHECK-S1 binary-identity: OK`, `masters-before=0`.

### S2 — the chat turn (interactive; the PE drives)

Open a chat window in this tmux session and start the client:

```sh
tmux kill-window -t "0:live09c" 2>/dev/null
tmux new-window -t "0:" -n live09c
tmux send-keys -t "0:live09c" 'daemoneye chat' Enter
```

The PE switches to window `live09c` and types this turn, exactly:

> Use edit_file on host pinky with sudo set to true to replace the line
> `old-line` with `new-line` in /tmp/de-09c-root.txt. Do not read the file
> first.

Then, in order:

1. An `edit_file` approval panel appears, its path line ending
   `(remote via host pinky, as root via sudo)`, with the
   `⚠ runs as root (sudo) on the host` line. The PE answers **`y`** (not
   `a`).
2. A credential prompt appears in the chat (`[sudo] password required`).
   The PE types pinky's sudo password and Enter.
3. The tool result reads `Edited /tmp/de-09c-root.txt on host pinky.` and
   the AI's reply confirms it.

If the AI asks a clarifying question or reaches for another tool, the PE
redirects it with the same sentence; nothing else is approved. If the AI
calls `read_file` first, deny nothing — a read on pinky needs no password
and does not change the evidence — but note it in the Update Log.

When the reply is complete, the PE types `/quit` (or Ctrl-D) and returns
here.

### S3 — read the evidence where it lives

```sh
A=/tmp/e2e-09c.txt
{
echo "== S3 EVIDENCE =="
CAST=$(grep -l 'de-09c-root.txt' ~/.daemoneye/var/log/shells/*.cast 2>/dev/null | head -1); [ -n "$CAST" ] || CAST=$(ls -t ~/.daemoneye/var/log/shells/*.cast | head -1); echo "cast=$(basename "$CAST")"
SL=$(find ~/.daemoneye/var/log/sessions -name '*.jsonl' ! -name '*archive*' -newer /tmp/de-09c-mark | head -1); echo "session=$(basename "$SL")"
printf 'cast-sudo-line=%s\n' "$(grep -c '"i", ".*set -- python3 .*; su''do \\"\$@\\"' "$CAST")"
printf 'cast-de-ok=%s\n' "$(grep -c 'DE_OK: Edited /tmp/de-09c-root.txt' "$CAST")"
printf 'cast-redacted-i=%s\n' "$(grep -c '"i", "\[redacted\]"' "$CAST")"
printf 'cast-prompt-o=%s\n' "$(grep -c 'de-sudo-prompt-' "$CAST")"
printf 'jsonl-edit-call=%s\n' "$(grep -c '"name": *"edit_file"' "$SL")"
printf 'jsonl-sudo-arg=%s\n' "$(grep -c '\\"sudo\\": *true' "$SL")"
printf 'jsonl-edited=%s\n' "$(grep -c 'Edited /tmp/de-09c-root.txt on host pinky' "$SL")"
CP=~/.daemoneye/var/run/ssh
S=$(date +%s%N); ssh -o ControlPath="$CP/%C" -O check pinky >/dev/null 2>&1; RC=$?; E=$(( ($(date +%s%N)-S)/1000000 ))
printf 'master-check=rc%s %sms\n' "$RC" "$E"
ls "$CP" | wc -l | sed 's/^/masters-after=/'
ssh -o BatchMode=yes pinky 'stat -c "after=%U:%a" /tmp/de-09c-root.txt'
} >> "$A" 2>&1
tail -n 13 "$A"
```

Expected: `cast-sudo-line=1`, `cast-de-ok=1`, `cast-redacted-i` **≥ 1**,
`cast-prompt-o` ≥ 1, `jsonl-edit-call=1`, `jsonl-sudo-arg` ≥ 1,
`jsonl-edited=1`, `master-check=rc0 <N>ms` with N under 50,
`masters-after=1`, `after=root:640`. The file is `root:640`, so `matt`
cannot read it: its **content** is read under sudo in S5 (`content=a new-line b`).
*(Amended at run time — the draft read it here without sudo and got
`Permission denied`; a spec defect, not a product one.)*

### S4 — the password is in no log

The PE types the password **into a shell variable**, unechoed; it is never
written anywhere and the variable is unset at the end:

```sh
A=/tmp/e2e-09c.txt
read -rs -p 'pinky sudo password (not echoed): ' PW; echo
{
echo "== S4 PASSWORD-ABSENCE =="
CAST=$(grep -l 'de-09c-root.txt' ~/.daemoneye/var/log/shells/*.cast 2>/dev/null | head -1); [ -n "$CAST" ] || CAST=$(ls -t ~/.daemoneye/var/log/shells/*.cast | head -1)
SL=$(find ~/.daemoneye/var/log/sessions -name '*.jsonl' ! -name '*archive*' -newer /tmp/de-09c-mark | head -1)
printf 'pw-len=%s\n' "${#PW}"
printf 'pw-in-cast=%s\n' "$(grep -cF -- "$PW" "$CAST")"
printf 'pw-in-session=%s\n' "$(grep -cF -- "$PW" "$SL")"
printf 'pw-in-daemon-log=%s\n' "$(grep -cF -- "$PW" ~/.daemoneye/var/log/daemon.log)"
printf 'pw-in-events=%s\n' "$(grep -rcF -- "$PW" ~/.daemoneye/var/log/events/ | grep -v ':0' | wc -l)"
if [ "$(grep -cF -- "$PW" "$CAST")" = 0 ] && [ "$(grep -cF -- "$PW" "$SL")" = 0 ] && [ "$(grep -cF -- "$PW" ~/.daemoneye/var/log/daemon.log)" = 0 ]; then echo "CHECK-S4 password-absent: OK"; else echo "CHECK-S4 password-absent: FAIL"; fi
} >> "$A" 2>&1
unset PW
tail -n 7 "$A"
```

Expected: `pw-len` > 0 (the check is not vacuous), every `pw-in-*=0`,
`CHECK-S4 password-absent: OK`.

### S5 — cleanup and the paste

```sh
A=/tmp/e2e-09c.txt
{
echo "== S5 CLEANUP =="
ssh -t pinky "sudo sh -c 'printf content=; tr \"\\n\" \" \" < /tmp/de-09c-root.txt; echo; rm -f /tmp/de-09c-root.txt'; ls /tmp/de-09c-root.txt 2>&1 | sed 's/^/gone: /'"
tmux kill-window -t "0:live09c" 2>/dev/null; echo "live09c windows left: $(tmux list-windows -a -F '#W' | grep -c live09c)"
} >> "$A" 2>&1
cat "$A"
```

(The `sudo rm` prompts the PE once more, in this terminal.) Then paste the
whole of `/tmp/e2e-09c.txt` as the first fenced block of an Update Log entry
titled `### Update — <date> (end-to-end verification)`, and run:

```sh
D=docs/dev/milestones/M21-host-model/phase-09c-live-close.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-09c.txt
diff /tmp/pasted-09c.txt /tmp/e2e-09c.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

The literal `PASTE MATCH` line goes into the entry.

## Acceptance criteria

Every line is read from `/tmp/e2e-09c.txt` as pasted. Against the current
tree none can be produced (no daemon is running, no host is configured, no
fixture exists), so each fails today.

- [ ] `CHECK-S1 binary-identity: OK` — the daemon that ran the turn is HEAD.
- [ ] `cast-sudo-line=1` and `cast-de-ok=1` — the cast carries the
      marker-delimited sudo edit and its success line.
- [ ] `cast-redacted-i` ≥ 1 — a password prompt was answered and its bytes
      were logged as `[redacted]`.
- [ ] `CHECK-S4 password-absent: OK` with `pw-len` > 0 — the real password
      appears in neither the cast, the session JSONL nor `daemon.log`.
- [ ] `jsonl-edit-call=1`, `jsonl-sudo-arg` ≥ 1, `jsonl-edited=1` — the
      session JSONL records the `edit_file` call with `sudo: true` and its
      result.
- [ ] `after=root:640` (S3) and `content=a new-line b` (S5, under sudo) —
      the edit landed and the file kept its owner and mode.
- [ ] `masters-before=0`, `masters-after=1`, `master-check=rc0` under 50 ms
      — the ControlMaster was established by this run and is reused.
- [ ] The `(end-to-end verification)` entry holds the file as its first
      fenced block and the literal `PASTE MATCH`.
- [ ] `git status --short` shows only this phase doc and the milestone
      README changed; **no `src/` change** (`git diff --stat HEAD -- src/`
      empty).

## Mutations

None. This phase changes no code. Its witness against a vacuous pass is
`cast-redacted-i ≥ 1` together with `pw-len > 0`: a run in which sudo did
not prompt (a warm ticket) would show `cast-redacted-i=0` and fail.

## Test plan

No new tests. The live turn exercises 09b's answered path against real
sudo-rs — the one path 09b could not test without the PE's password — and
01b's scrub against a real prompt through a real ssh PTY.

## End-to-end verification

The five blocks in § Spec **are** the end-to-end verification; there is no
separate block. `/tmp/e2e-09c.txt` is the capture, pasted per S5.

**If `pinky` is unreachable or the PE is not present, stop and record the
criterion unmet.** Do not describe what it would have printed.

## Authorizations

- `~/.daemoneye/etc/config.toml`: the `[hosts.pinky]` entry only (S0).
- `~/.cargo/bin/daemoneye`: reinstalled from `target/release` (S1).
- `/tmp/de-09c-root.txt` on pinky: created and removed.
- This phase doc's Update Log and the milestone README's phase-table row.
- **No file under `src/`, `assets/` or `tests/`.** A defect found here is a
  bug doc on 09b (or a new row), not a fix in this phase.

## Out of scope

- Fixing anything the live run reveals. A finding is filed as a bug against
  the row that owns the behaviour and this phase reports the criterion unmet.
- The 09b cosmetic residual (the `shell_run_error_text` label) — carried in
  the README's Notes.
- Any ghost or scheduled path; this is the chat exit only.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-09-30 (started)

Architect-run with the PE present. Status flipped `todo` → `in-progress`; README row to match. Running S0.

### Update — 2026-09-30 (end-to-end verification)

Run by the architect with the PE at the keyboard, in tmux session `0`. The
capture, `/tmp/e2e-09c.txt`, pasted whole:

```
== S1 REBUILD-RESTART ==
03570db7
    Finished `release` profile [optimized] target(s) in 52.53s
build exit=0
Daemon is not running (stale PID file names PID 603681).
install: done
daemoneye daemon started (PID 2495120)
daemon-start exit=0
Daemon is running.
CHECK-S1 binary-identity: OK
masters-before=0
== S3 EVIDENCE ==
cast=s0-1790788328.cast
session=8cdda87a4120b4ae.jsonl
cast-sudo-line=1
cast-de-ok=1
cast-redacted-i=1
cast-prompt-o=1
jsonl-edit-call=1
jsonl-sudo-arg=1
jsonl-edited=1
master-check=rc0 2ms
masters-after=1
after=root:640
bash: line 1: /tmp/de-09c-root.txt: Permission denied

== S4 PASSWORD-ABSENCE ==
pw-len=8
pw-in-cast=0
pw-in-session=0
pw-in-daemon-log=0
pw-in-events=0
CHECK-S4 password-absent: OK
== S5 CONTENT-AND-CLEANUP ==
[sudo: authenticate] Password: ********        
content=a new-line b 
gone: ls: cannot access '/tmp/de-09c-root.txt': No such file or directory
live09c windows left: 0
```

The paste-check snippet printed:

```
PASTE MATCH
```

**What the cast shows.** `s0-1790788328.cast` has exactly two `"i"` events:
the marker-wrapped sudo edit line at `t=0.02` (2697 bytes, opening with the
`export SUDO_PROMPT` sentinel), and `[redacted]` at `t=4.34` — the
password, 4.3 s after pinky's prompt `[sudo: [de-sudo-prompt-0]] Password:`
appeared in the output. The `DE_OK: Edited /tmp/de-09c-root.txt` line
follows. The 8-character password appears in no log (`pw-in-*=0`).

**Chat side.** The panel read `/tmp/de-09c-root.txt (remote via host pinky,
as root via sudo)` with the `⚠ runs as root (sudo) on the host` line and a
`-old-line` / `+new-line` diff; the PE pressed `y`, then answered
`[sudo] password required`. The tool result was `Edited … on host pinky.`
The AI did not read the file first.

**Two run-time notes.** (1) The first `tmux send-keys` of the turn was
dropped: the client was still streaming its opening greeting and does not
accept input while streaming. Re-sent once idle, it went through. (2) The
draft's S3 read the file's content as `matt`, which a `root:640` file
refuses; the content read moved into S5 under sudo (`content=a new-line b`).
Spec defect, amended in place; no product finding.

**S0.2 and S4 ran in a tmux window** (`fixture09c`), not through the
`!` runner: that runner has no terminal, so neither `sudo` nor `read -rs`
can prompt there.

### Review verdict — 2026-09-30

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** Claude Code (direct) — architect-run with the PE present
- **Reviewer re-run:** the evidence *is* the run; every acceptance line is
  read from the pasted capture. `CHECK-S1 binary-identity: OK`;
  `cast-sudo-line=1`, `cast-de-ok=1`, `cast-redacted-i=1`;
  `CHECK-S4 password-absent: OK` with `pw-len=8`; `jsonl-edit-call=1`,
  `jsonl-sudo-arg=1`, `jsonl-edited=1`; `after=root:640` and
  `content=a new-line b`; `masters-before=0`, `masters-after=1`,
  `master-check=rc0 2ms`. `git diff --stat HEAD -- src/` is empty.
- **Scope deviations:** none in code. One live-config change kept:
  `[hosts.pinky]` in `~/.daemoneye/etc/config.toml`; the installed binary
  is now HEAD.
- **Calibration:** (1) A spec that reads a file it just made root-only
  must say who reads it — "spec facts must be executed" applies to the
  evidence block too. (2) The `!` runner is not a terminal; anything that
  prompts (sudo, `read -rs`) needs a tmux window. (3) The chat client drops
  input while streaming; scripted turns must wait for idle.

# Phase 06: live-close — the post-2.0 fixes, checked against a real daemon; 2.2.0 ships

**Milestone:** M29 — Post-2.0 fixes
**Status:** done
**Depends on:** 01–05 (all `done`)
**Estimated diff:** this doc's Update Log, the milestone README, then the
release (`Cargo.toml` / `Cargo.lock`, `CHANGELOG.md`)
**Tags:** kind=verification, size=s, architect-run

## Goal

Run M29's exit criteria for real on pinky, the M14/M27/M28 way. The
architect rehearses every block in a throwaway root, and the PE then runs it
once while watching. A defect becomes a finding `F1…`, fixed in a phase
`06a…` before the close.

Unit tests cannot see the residency sequences. Those span a real daemon
restart, a real `chat --resume` and a real webhook, which is exactly where
M27's F5, F8 and F11 hid.

## Pre-flight

1. **Harness (M28's, carried over).**
   - Root `R=/tmp/de29-live`, run as `HOME=$R`, with `~/.claude`,
     `~/.claude.json` and `~/.ssh` symlinked in.
   - Binary `BIN=target/release/daemoneye`, rebuilt at HEAD.
   - Emulator: a private tmux on `EMU=/tmp/de29-emu.sock`, driven with
     `tmux -S $EMU` and `TMUX` unset. The PE watches with
     `tmux -S $EMU attach -r`.
   - Clean-env daemon start, unsetting every `CLAUDE*` variable.
   - The chat's first input is `/approvals on` (auto-approve; `off` gates
     every command behind a panel).
   - Before starting chat, attach a client to the emulator (the PE's
     `attach -r`, or a headless one via `script`): chat waits for an
     attached client before its banner.
   - S-last kills only processes whose environment has `HOME=$R`.
2. **Model.** The local GLM at `brain:8000`, so the chat's turns cost no
   API credit.
3. **Config in `$R`.** The webhook on `127.0.0.1:19418` with
   `severity_threshold = "info"`. No hosts, no runbooks.
4. **Side effects.** Local only: a throwaway daemon, daemon restarts inside
   `$R`, and webhook POSTs to `127.0.0.1`. No ssh, no `claude` launch,
   nothing pushed. The PE's real daemon is untouched.

## Blocks

Each block appends what it saw to `/tmp/e2e-m29-06.txt`. Tool results are
read from the session's history file, `$R/.daemoneye/var/log/sessions/<id>.jsonl`.

| # | Block | Pass when |
|---|---|---|
| L1 | **Masker (O3).** The model runs `hostname`, then `printf 'Host web\n  HostName 10.9.9.9\n'` | The first result shows `pinky`, not `<SSH_HOST>`. The second shows `<SSH_HOST>`, not `10.9.9.9` |
| L2 | **Shell line (O4).** The same results, plus the model runs `sh -c 'exit 3'` | Each waited result ends `[ran in shell sN]`. The exit-3 result has `[command exited with status 3]` **before** that line |
| L3 | **Log fields (todo 6).** The `command` records in `$R/.daemoneye/var/log/events/events-*.jsonl` and in daemon.log, after L1–L2 plus one `wait: false` command and one denied panel (`/approvals off`, answer `n`, then `/approvals on`) | The waited runs carry `"wait":true,"shell":"sN"`; the `wait: false` run carries `"wait":false`; the denial carries `"shell":null`; `mode`/`pane` are still present. daemon.log has `command approved (waited):` and `command denied (waited):`, and no `foreground command approved` |
| L4 | **Active pane (F9).** Split the emulator window: chat in one pane, a shell in the other. With the chat pane focused, the model calls `get_shell_context`. Then `select-pane` to the shell pane, wait 3 s for the poll, and ask again | First result: no `[ACTIVE PANE`, and the shell pane is listed. Second result: exactly one `[ACTIVE PANE`, on the shell pane, and never two |
| L5 | **S1: restart → reconnect → alert → ask.** Chat once (one question), quit. `daemoneye stop` then start. `chat --resume <id>`. POST an alert naming `disk-full-L5`. Ask "what alerts arrived?" | The answer names `disk-full-L5`, and the history file holds the alert |
| L6 | **S2: restart → reconnect → quit (no ask) → alert → resume.** Restart, `chat --resume <id>`, quit without asking. POST `cpu-hot-L6`. Wait 35 s. `chat --resume <id>` | The catch-up brief shows on reconnect, before any typing, and names `cpu-hot-L6` |
| L7 | **S3: a fresh resume of a never-loaded session.** Restart. `chat --resume <id>` with no prior attach since the restart. Ask "what was my first question in this chat?" | The answer quotes the L5 question, so history came from disk |
| L8 | **Invalid args (todo 3, 4).** Not live-reachable on demand: it depends on the model producing a bad argument. Covered by unit tests in phase 01 | Recorded as `not run (unit-covered)`, unless GLM happens to produce one, which is then checked against the wording |

## Close (after the PE run passes)

1. **Nits recorded in reviews.**
   - The README's audit-log paragraph lost the sentence saying that
     `exit` carries the real exit code (phase 03).
   - The context-builder doc comment sits above `pane_map_summary`
     (pre-existing, noted in phase 04).

   The README sentence is a docs edit the architect makes at the close.
   The doc comment is source, so it goes in a tiny `06a` or is deferred,
   as the PE decides.
2. The retrospective in the README Notes, including the spec-bug fold
   proposal (four occurrences this milestone) for the PE's sign-off.
3. `NEXT.md` set to none.
4. The release per `REXYMCP.md` § Versioning: `2.2.0`, the CHANGELOG
   entry, the four gates, the commit and the annotated tag `v2.2.0`.
   **Push only on the PE's explicit go-ahead**, after a credential scan.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-20 (rehearsal 1, architect)

Harness as in Pre-flight, at `d867b46c` (release binary current). The raw
notes are in `/tmp/e2e-m29-06.txt`. Every process with `HOME=$R` was killed
at the end; the PE's own daemon was untouched.

| Block | Result |
|---|---|
| L1 masker | **pass**: `pinky\n[ran in shell s0]`; the printf gave `Host web\n<SSH_HOST>\n[ran in shell s0]` |
| L2 shell line | **pass**: every waited result ends `[ran in shell s0]`; exit 3 gives `[command exited with status 3]` then `[ran in shell s0]` |
| L3 log fields | **pass**: waited runs `"shell":"s0"…"wait":true`; the `wait: false` run `"shell":"s1"…"wait":false`; the denial `"shell":null`, with `mode`/`pane` present; daemon.log has `command approved (waited):`, `command approved (wait: false):`, `command denied (waited):`, and no `foreground command approved` |
| L4 active pane | **first read pass, second read fail → F1** |
| L5 S1 | **pass**: the answer named `disk-full-L5`; the history holds `[Webhook Alert]\n[FIRING] disk-full-L5 …` |
| L6 S2 | **half → F2**: the catch-up brief showed on reconnect, before the banner and before any typing (residency works), but it reads only `• [Webhook Alert]` |
| L7 S3 | **pass**: the first resume after a restart quoted "What is 17 times 23? Answer in one line." |
| L8 invalid args | not run (unit-covered); 0 invalid-argument lines in daemon.log |

**Findings.**
- **F1 (major), `get_shell_context` never labels the focus.** With the
  focus moved to the shell pane (`pane_active=1`; the pane map shows
  `idx:1=%1*`), the result had no `[ACTIVE PANE` at all. The cause is
  phase 04's spec, not the executor. `get_shell_context` calls
  `get_labeled_context_scoped(chat_pane, chat_pane, …)`, and the rule gives
  the block only to a `source_pane` that is not the chat pane and is the
  focus. "Callers are unchanged" therefore made the second half
  unreachable. The turn prompt has the same weakness: its source is the
  target pane the chat resolved at startup, so a focus on any other pane is
  never labelled. Fix constraint: the active block goes to the cache's
  `active_pane` when it is in the session and is not the chat pane,
  independent of `source_pane`; still at most one.
- **F2 (minor; pre-dates M29), the catch-up does not name the alert.**
  `build_catchup_brief` keeps each event's first line, and webhook messages
  are built as `[Webhook Alert]\n[FIRING] <name> — …`, so the brief shows
  the bare tag. Fix constraint: when the first line is only the event tag,
  the summary carries the next non-empty line too.
- **S1 (spec), the approvals words are inverted.** `/approvals off` means
  gated (panels shown) and `on` means auto-approve. Pre-flight's "first
  input is `/approvals off`" (carried from M28, which wanted panels) and
  L3's "`/approvals on`, answer `n`" read them the other way round: L3
  needs `/approvals off` to get a panel to deny. The rehearsal ran L1–L2
  under `on` and the denial under `off`.
- **Observations.** O5: the `wait: false` record carries `exit: 0`, the
  launch's code, not the command's; the denial's `mode` is still the
  legacy `foreground`. O6: the status bar reads `turn 1` after a resume,
  and `session:` is blank while an approval panel is up. None blocks the
  close.
- **Harness.** H1: chat blocks before its banner until the emulator
  session has an attached client (`chat.rs` polls
  `#{session_attached}`). Attach a headless read-only client first:
  `script -q -c "tmux -S $EMU attach -r -t m29" /dev/null` under
  `setsid`. The PE's own `attach -r` serves the same purpose.

**Next.** Fix phase **06a** for F1 and F2, with the `pane_map_summary`
doc-comment nit folded in. Then rehearsal 2 re-runs L4 and L6 (and L1–L7
as a regression pass), and the PE run follows with S1's wording fixed in
this doc.

### Update — 2026-10-20 (rehearsal 2, architect, after 06a)

The release was rebuilt at `dc30f475` (06a approved), and the runtime state
was reset to the config alone. The harness followed the corrected
Pre-flight: `/approvals on` first, and a headless read-only client attached
before chat. The raw notes are in `/tmp/e2e-m29-06.txt` (section R2). Every
process with `HOME=$R` was killed at the end.

| Block | Result |
|---|---|
| L1 masker | **pass**: `pinky\n[ran in shell s0]`; `Host web\n<SSH_HOST>\n[ran in shell s0]` |
| L2 shell line | **pass**: `\n[command exited with status 3]\n[ran in shell s0]` |
| L3 log fields | **pass**: the same records as rehearsal 1; the denial ran under `/approvals off` |
| L4 active pane | **pass, F1 fixed**: chat focused → no `[ACTIVE PANE`, `%1` listed; focus on `%1` → exactly one `[ACTIVE PANE %1 \| cwd: /tmp \| title: pinky]` with the pane's screen, and `%1` not listed again |
| L5 S1 | **pass**: the answer named `disk-full-L5`; the history holds the alert |
| L6 S2 | **pass, F2 fixed**: 4 s after reconnect, before any typing: `• [Webhook Alert] [FIRING] cpu-hot-L6 — rehearsal alert L6: cpu at 99%` |
| L7 S3 | **pass**: the first resume after a restart quoted "What is 17 times 23? Answer in one line." |
| L8 invalid args | not run (unit-covered); 0 invalid-argument lines in daemon.log |

No new findings. O5 and O6 recur, as recorded in rehearsal 1, and still do
not block the close.

**Next.** The PE run: L1–L7 once, watched with
`tmux -S /tmp/de29-emu.sock attach -r`, then the Close steps.

### Update — 2026-10-20 (PE run, architect-driven, PE watching)

Release binary at `ff0b201e` (no source change since 06a's `dc30f475`).
Runtime state reset to the config alone; the PE attached read-only before
chat started. Raw notes: `/tmp/e2e-m29-06.txt` (section "PE run"). Every
process with `HOME=$R` was killed at the end; the PE's own daemon was
untouched.

| Block | Result |
|---|---|
| L1 masker | **pass**: `pinky\n[ran in shell s0]`; `Host web\n<SSH_HOST>\n[ran in shell s0]` |
| L2 shell line | **pass**: `\n[command exited with status 3]\n[ran in shell s0]` |
| L3 log fields | **pass**: waited runs `"shell":"s0"…"wait":true`; `wait: false` `"shell":"s1"…"wait":false`; denial `"shell":null`, `mode`/`pane` present; daemon.log `command approved (waited):` ×3, `command approved (wait: false):`, `command denied (waited):`, 0 `foreground command approved` |
| L4 active pane | **pass**: chat focused → no `[ACTIVE PANE`, `%1` listed; focus on `%1` → exactly one `[ACTIVE PANE %1 \| cwd: /tmp \| title: pinky]`, `%1` not listed twice |
| L5 S1 | **pass**: the answer named `disk-full-L5`; history line 7 holds `[Webhook Alert]\n[FIRING] disk-full-L5 …` |
| L6 S2 | **pass**: within 1 s of launch, before any typing: `• [Webhook Alert] [FIRING] cpu-hot-L6 — PE run alert L6: cpu at 99% (alertmanager)` |
| L7 S3 | **pass**: the first resume after a restart quoted "What is 17 times 23? Answer in one line." |
| L8 invalid args | not run (unit-covered); 0 invalid-argument lines in daemon.log |

No new findings. O5 and O6 recur. One model observation, not a daemon
defect: after L1, GLM narrated a "third run" of the printf with the
unmasked `HostName` line; no third tool call exists in the history, and it
retracted the fabrication unprompted on the next turn. The real results
are the two tool records above.

### Review verdict — 2026-10-20

- **Verdict:** approved_first_try (architect-run; two rehearsals, then
  the PE run)
- **Bounces:** none. Rehearsal 1's F1 and F2 went to 06a, not to a bounce.
- **Executor:** Claude Code (direct), PE watching the PE run.
- **Scope deviations:** none. The README `exit` sentence is restored at
  the close (Close step 1); the doc-comment nit went into 06a.
- **Calibration:** S1 (the inverted `/approvals` words) is a doc bug the
  rehearsal caught before the PE run. The harness helper must target the
  chat pane by id once the window is split: `send-keys -t <session>` goes
  to the *focused* pane, which L4 deliberately moves.

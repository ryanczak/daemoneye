# Phase 10: live-close — telemetry, the daily cap and `top`, checked against a real daemon; 2.3.0 ships

**Milestone:** M30 — Telemetry & dashboard
**Status:** in-progress (PE run passed 2026-10-23; phases 11–14 done 2026-10-25; L8 rehearsed 2026-10-25, PE run of L8 next, then the Close steps)
**Depends on:** 01–09 (all `done`)
**Estimated diff:** this doc's Update Log, the milestone README, then the
release (`Cargo.toml` / `Cargo.lock`, `CHANGELOG.md`)
**Tags:** kind=verification, size=s, architect-run

## Goal

Run M30's exit criteria for real on pinky, the M14/M28/M29 way. The
architect rehearses every block in a throwaway root, and the PE then runs
it once while watching. A defect becomes a finding `F1…`, fixed in a
phase `10a…` before the close.

The unit tests cover each piece. What they cannot cover is the joins, and
those are what this phase checks:

- a real chat turn, ghost run and scheduled job each leaving one `run`
  record, with `ai_cost` lines that carry its `run_id`;
- the scorecard and profile reading those records with the daemon down;
- the cap refusing on all three paths with spend that is really on disk;
- `top`'s Shells panel against live shells. That check was deferred from
  phase 08, because no unit test has a daemon.

## Pre-flight

1. **Harness (M29's, carried over).**
   - Root `R=/tmp/de30-live`, run as `HOME=$R`, with `~/.claude`,
     `~/.claude.json` and `~/.ssh` symlinked in.
   - Binary `BIN=target/release/daemoneye`, rebuilt at HEAD.
   - Emulator: a private tmux on `EMU=/tmp/de30-emu.sock`, driven with
     `tmux -S $EMU`, with `TMUX` and every `CLAUDE*` variable unset. The
     PE watches with `tmux -S $EMU attach -r`. Attach a client before
     starting chat (the PE's own, or a headless `script -q -c "tmux -S
     $EMU attach -r -t m30" /dev/null` under `setsid`), because chat
     waits for one before its banner. A headless client has no tty size,
     so tmux shrinks the window to 80×24. Give the client a size instead
     of forcing the window: `script -q -c "stty cols 160 rows 48; tmux -S
     $EMU attach -r -t m30" /dev/null`. Do **not** use `window-size
     manual` with a headless client: on tmux 3.6, `new-window` and
     `split-window` then crash the server (rehearsal 2, three times).
     Start the server and the client with `setsid -f`.
   - Once a window is split, `send-keys` targets panes by id.
   - The chat's first input is `/approvals on` (auto-approve).
   - S-last kills only processes whose `/proc/<pid>/environ` has
     `HOME=$R`. The PE's real daemon, in `~/.cargo/bin`, is never touched.
2. **Model.** The local GLM at `brain:8000`. It is unpriced, so each
   `ai_cost` is `$0`. The cap block therefore seeds its spend on disk
   instead of buying it.
3. **Config in `$R`.** `/tmp` does not survive a reboot (the 2026-10-23
   reboot took `/tmp/de29-live` and `/tmp/de30-live` with it), so build it
   from the real `~/.daemoneye/etc/config.toml`. Set `[models.default]`
   `model` to the GLM and `base_url` to `http://brain:8000/v1`: the real
   config points at an LM Studio host, not brain. Then add:
   - the webhook on `127.0.0.1:19418` with `severity_threshold = "info"`
     and `dedup_window_secs = 0`;
   - no `[budget]` until L7;
   - one runbook, `disk-full-l4`, with `enabled: true`, `max_ghost_turns:
     4`, and Alert Criteria that say a firing `disk-full-l4` always
     warrants a ghost (`GHOST_TRIGGER: YES`). Rehearsal 1's first wording
     ("synthetic, action: none") made GLM decline, correctly.
4. **Side effects.** Local only: a throwaway daemon, restarts inside `$R`,
   and webhook POSTs to `127.0.0.1`. No ssh, nothing pushed.

## Blocks

Each block appends what it saw to `/tmp/e2e-m30-10.txt`. Events are read
from `$R/.daemoneye/var/log/events/events-<today>.jsonl`, and tool results
from the session's history file, never from GLM's prose.

| # | Block | Pass when |
|---|---|---|
| L1 | **Run record: chat.** Ask "Run `hostname` and tell me the result." | One `run` event for the turn with `kind: chat`, the GLM `provider`/`model`, `status` complete, `failure_class: none`, `tool_calls ≥ 1`, `tool_success_rate: 1.0`, and `tokens > 0`. Every `ai_cost` line the turn wrote carries that `run_id` |
| L2 | **Shells panel, live (deferred from 08).** With the L1 chat still attached, run `HOME=$R $BIN top --once` | Shells lists `s0` with the chat's owner, foreground `shell prompt` and a last line containing `pinky` (F2, phase 10a). Nothing in the panel reads `daemon not running` |
| L3 | **`top`, live.** In a second emulator window, run `HOME=$R $BIN top`. Ask the chat to run `sleep 8; echo done-L3` | While the command runs, Runs shows the chat run `live` with the selection on it, and Activity gains rows without a key press. Once it finishes, that run's TURNS is at least 2 and equals its `run` record's `turns` (F1, phase 10a). After `q` the pane is back at a shell prompt, and `stty -a` there shows `icanon` and `echo`. A second `top` quits on Ctrl-C the same way |
| L4 | **Webhook → watchdog → ghost.** POST an alert named `disk-full-l4` (severity `critical`, a summary saying investigation is required) | The watchdog's `ai_cost` is recorded with `run_id: ""` (a side call belongs to no run). A `run` event with `kind: ghost` has tags naming `disk-full-l4`, `turns`, `tool_calls` and `tokens` above zero, and its `ai_cost` lines carry its `run_id`. Its turn log holds one `turn_started` per turn. (A completed ghost has no briefing; `budget_left` is checked on L7's refused ghost) |
| L5 | **Scheduled.** Ask the chat to schedule an interval ghost job on `disk-full-l4` every 2 minutes, and answer `y` to the panel (`schedule_command` is gated even under `/approvals on`). Wait for one firing (about 2 min) | A `run` event with `kind: scheduled` and `model: null`, plus the ghost run it started, as in L4 |
| L6 | **Scorecard and profile, daemon down.** `daemoneye stop`, then `scorecard`, `scorecard --by kind --json \| jq`, `scorecard --min-runs 100`, `profile <GLM model>` and `profile no-such-model` | The scorecard has a GLM row with `n_runs` counting L1–L5, and no `scheduled` record is counted against the model. `--by kind` separates `chat` from `ghost`. The JSON parses. `--min-runs 100` drops every row. The profile shows the strengths rows and the ranked classes without `none`. `no-such-model` names GLM and exits 0 |
| L7 | **The daily cap, all three paths.** With the daemon down, add `[budget] max_cost_per_day = 1.0`, append one local-today `ai_cost` line of `$5.00` to the segment, and start the daemon. Then run `status`, ask the chat anything, POST `disk-full-l4`, and wait for the L5 job's next firing | `status` shows the spend against the cap. The chat shows the `SystemMsg` naming `$5.00` and `$1.00` at `--resume`, before any typing (the greeting turn is a model call), and again after the ask; no new `ai_cost` is written. daemon.log has the webhook's `skipped — daily cost cap` line. The L5 job fires within seconds of the restart (missed-run catch-up); its ghost run ends `budget_exceeded` with a briefing whose `budget_left` is `Usd`, while the `scheduled` wrapper completes (O3). `budget_refused` events name `chat`, `webhook` and `ghost`. Finally, cancel the L5 job |
| L8 | **`top` after phases 11–14** (added 2026-10-23). Needs the daemon up, the L1 chat attached and L4's ghost finished, so on a fresh root run L1 and L4 first, with `[budget] max_cost_per_day = 1.0` already set (GLM spends $0, so the cap refuses nothing). Seed today's segment with five identical `cache_refresh_error` lines and one `ai_cost` of $0.85, by cloning a real `ai_cost` line. Run `HOME=$R $BIN top` at the PE's terminal size; while it runs, seed $0.30 more; then resize the client to 100 columns | The header names the daemon pid, model and local time; Runs and Budget sit side by side at the wide size and stack at 100 columns; a ghost row's SUBJECT reads `disk-full-l4 ← webhook`; Activity times match the PE's clock; a burst of identical events shows as one row `×N`; Tab moves the focus through the four panels and ↑↓ scrolls the focused one, Activity stopping its tail until `f`; Enter on the ghost row opens its detail with the record's fields and turn log tail, Esc returns; the Budget bar is yellow at 85 % and turns red within a refresh of the $0.30 seed, its `115%` still on screen; `q` quits cleanly |

## Close (after the PE run passes)

0. **L8** passes at the PE run of `top` after phases 11–14.
1. **The nit recorded in review 09.** The first line of `run_tui`'s doc
   comment appears twice (`src/cli/commands/top_tui.rs`). It is source,
   so it goes in a tiny `10a` or is deferred, as the PE decides.
2. **The retrospective** in the README Notes, with the fold proposals for
   the PE's sign-off:
   - every Spec "must" gets a criterion (bug-01-1, bug-04-1, bug-05-1:
     three occurrences);
   - preservation criteria are measured, not assumed (07 `live_exit`, 08
     `budget_usd`);
   - fixtures include inputs the code was not shaped for (bug-07-1,
     bug-09-1);
   - a Spec that sends the executor to discover its sources stalls (07,
     08): name every symbol, check module reachability, and set a work
     order;
   - harnesses that use bash say so;
   - one convention for counting stalls in a verdict.
3. `NEXT.md` set to none.
4. **The release** per `REXYMCP.md` § Versioning: `2.3.0`, the CHANGELOG
   entry, the four gates, the commit and the annotated tag `v2.3.0`.
   **Push only on the PE's explicit go-ahead**, after a credential scan of
   every unpushed M30 commit.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-22 (rehearsal 1, architect)

Harness as in Pre-flight at `95906f07` (release binary at HEAD), root
`/tmp/de30-live`, emulator `/tmp/de30-emu.sock`, GLM at `brain:8000`. The
raw notes are in `/tmp/e2e-m30-10.txt`. Every process with `HOME=$R` was
killed at the end; the PE's daemon was untouched.

| Block | Result |
|---|---|
| L1 run record: chat | **pass**: `76d9e1b9c5f141de` kind `chat`, `ollama`/GLM, `complete`/`none`, turns 2, tool_calls 1, success 1.0, 32 415 in / 38 out; both of its `ai_cost` lines carry the id |
| L2 Shells live | **last line pass, foreground → F2**: `s0 chat:8e5ac79118a08c41 - pinky`; no `daemon not running` |
| L3 `top` live | **pass** at 160×48: the chat run `running`/`live`, its row reversed (`ESC[7m`); Activity 20 → 24 → 25 rows with no key; after `q` and after Ctrl-C the pane shows `echo icanon`. TURNS reads 0 for every chat run → F1 |
| L4 webhook → ghost | **pass on the second alert**: the first runbook wording made GLM answer `GHOST_TRIGGER: NO` (H2). Then ghost `dd01e5bc2d7a4552` `complete`/`none`, turns 3, tool_calls 2, 48 971 in / 1 667 out, tags `runbook: disk-full-l4`; three `ai_cost` lines carry the id; the turn log has `turn_started` 1–3; the watchdog's `ai_cost` has `run_id: ""` (O1) |
| L5 scheduled | **pass**: `93a22aad0d9840ec` kind `scheduled`, `model: null`, tags `job: l5-ghost`; it started ghost `ff661dde338e4a39`, `complete`, tagged with the runbook |
| L6 scorecard, profile | **pass** with the daemon down: GLM rows `chat 5` / `ghost 2`, "1 run(s) with no model skipped"; `--json` is `{rows, rows_dropped, skipped_no_model}` and parses; `--min-runs 100` ⇒ `no runs in this window`; the profile shows both rows and `no failures`; `no-such-model` names GLM, exit 0 |
| L7 daily cap | **pass**: `status` ⇒ `cap $5.0000 of $1.00 (500%)`; the chat's refusal at resume and after the ask, `ai_cost` 17 → 17; `Webhook: analysis … skipped — daily cost cap`; the job fired 3 s after the restart, ghost `b474cf306c374a19` `budget_exceeded` with blocker = the refusal and `budget_left: "Usd"`; `budget_refused` ×4: chat ×2, webhook, ghost |

**Findings.**
- **F1 (minor), `top` shows `TURNS 0` for every chat run.** The chat
  loop writes only `run_started` and `run_finished` to the turn log, while
  the ghost loop writes `turn_started` per turn (`ghost.rs`
  `emit_turn_started`). `top` counts turns from the turn log, so a chat
  run whose `run` record says `turns: 2` shows 0. Fix constraint: the chat
  loop logs `turn_started` as the ghost loop does, or `top` reads the
  count from the record.
- **F2 (minor), the Shells foreground is `-` for an idle shell.**
  `last_foreground` is written only by `probe_foreground` (a caller-named
  shell, `ShellTarget::Existing`) and `read_shell_screen`. The default
  path records nothing, and a waited command's completion — where the
  daemon knows the shell is back at its prompt — records nothing either.
  So the panel reads `-` for the common case. Fix constraint: a
  marker-terminated completion stores `(Foreground::Prompt, now)`.
- **Observations.** O1: a side call's `ai_cost` has `run_id: ""` by
  design (`cost.rs`); the exit criterion's "every `ai_cost` carries its
  run" holds for calls made by a run. O2: a chat's startup greeting is
  itself a `chat` run (`0bd9f2c757be4012`, 295 output tokens). O3: when a
  scheduled ghost job's ghost is refused by the cap, the `scheduled`
  wrapper record is `complete`/`none` and only the ghost record is
  `budget_exceeded`; a scorecard counts the job as a pass. O4: `--min-runs`
  that drops every row prints `no runs in this window`, not that rows
  were dropped. O5: `daemoneye stop` leaves the pid file in place (flock
  payload, by design).
- **Harness.** H1: the headless client is 80×24, so Activity was one row
  tall until `window-size manual` + `resize-window`. H2: the watchdog
  decides; the runbook must say the alert warrants a ghost. H3:
  `schedule_command` prompts even under `/approvals on`. H4: a job that
  missed a firing while the daemon was down fires seconds after the
  restart. H5: `stty -a` separates flags with spaces, not `;`.

**Next.** The PE chose to fix F1 and F2 now (2026-10-22): phase 10a carries
both and the `run_tui` doc-comment nit. Rehearsal 2 follows its approval,
then the PE run.

### Update — 2026-10-22 (phase 10a approved)

Phase 10a fixed F1 and F2 and the `run_tui` nit. It was approved first
try (`af0301ef`). L2 now expects foreground `shell prompt`. L3 now expects
a finished chat run's TURNS to match its `run` record. Rehearsal 2 runs
L1–L7 again against a release build at HEAD. The PE run follows it.

### Update — 2026-10-23 (rehearsal 2, architect)

Release build at `2ae5ea25` (phase 10a in), fresh root, same harness. The
raw notes are in `/tmp/e2e-m30-10.txt`. Teardown killed only `HOME=$R`
processes; the PE's daemon was untouched.

| Block | Result |
|---|---|
| L1 | **pass**: `9513f4c6b0724ebf` chat, complete/none, turns 2, tool_calls 1, success 1.0; its turn log now has `turn_started` 1 and 2 (F1); both `ai_cost` lines carry the id |
| L2 | **pass**: `s0 chat:9b6618c05ce3961b shell prompt pinky` (F2); Runs TURNS 2 and 1 |
| L3 | **pass** at 160×47: the run `running`/`live` and reversed; TURNS 1 mid-run, 2 after, equal to the record; `q` and Ctrl-C leave `icanon,echo` and a prompt. The Activity panel was already full (24 rows), so growth was measured in L4 by the newest row's timestamp, which moved |
| L4 | **pass on the first alert**: ghost `37a3e65e540b44bb` complete/none, turns 2, tool_calls 1, tagged with the runbook; its `ai_cost` lines carry the id, the watchdog's has `run_id: ""` (O1); `top` shows the ghost's TURNS 2 |
| L5 | **pass**: `3fa8789776574fdc` scheduled, `model: null`, `job: l5-ghost`; ghost `8875793ced1b4464` complete |
| L6 | **pass**: chat 6 / ghost 2, one model-less run skipped and counted, `--json` parses with `skipped_no_model: 1`, `--min-runs 100` empties, profile and `no-such-model` as specified |
| L7 | **pass**: `cap $5.0000 of $1.00 (500%)`; refusal at resume and after the ask, `ai_cost` 15 → 15; webhook `skipped — daily cost cap`; the job fired 6 s after the restart, ghost `12e692d737734749` `budget_exceeded` with `budget_left: "Usd"`, wrapper `complete` (O3); `budget_refused` chat ×2, webhook, ghost; job cancelled |

**Findings.** None new. F1 and F2 are confirmed fixed live.

**Observations.** O6: while the emulator's tmux server was down, the
daemon wrote a `cache_refresh_error` event every 2 s for the chat's
session, and they filled the Activity panel. M29 behaviour, not M30's;
noted for the retrospective.

**Harness.** H6: tmux 3.6 crashes on `new-window` / `split-window` when
a headless client is attached under `window-size manual`. Pre-flight now
sizes the client instead. The daemon, its shell and the chat session all
survived the three crashes; `chat --resume` picked the session back up.

**Next.** The PE run. Then the Close steps.


### Update — 2026-10-23 (PE run)

Release build at `0eb0d399`, fresh root (rehearsal 2's state moved aside
to `/tmp/de30-live-rehearsal2.daemoneye`), the PE attached read-only at
188×52 for the whole run while the architect drove the blocks. The raw
notes are in `/tmp/e2e-m30-10.txt`. The PE's daemon was untouched.

| Block | Result |
|---|---|
| L1 | **pass**: `a2da08d9d36a4bd3` chat, complete/none, turns 2, tool_calls 1, success 1.0, tokens 32459 in / 75 out; both `ai_cost` lines carry the id; turn log `turn_started` 1 and 2 |
| L2 | **pass**: `s0 chat:5175faf67d501ce8 shell prompt pinky`; nothing reads `daemon not running` |
| L3 | **pass**: `new-window` under the PE's attach did not crash the server (H6 fix); the run `running`/`live` and reversed while `sleep 8` ran, Activity's newest row moved 14:34:11 → 14:35:04 with no key press; TURNS 2 after, equal to the record; `q` and Ctrl-C each leave a prompt with `icanon` and `echo` |
| L4 | **pass on the first alert**: watchdog `ai_cost` with `run_id: ""`; ghost `7eb9151d45fa49f1` complete/none, turns 2, tool_calls 1, tokens 32528 / 3883, tagged `runbook: disk-full-l4`; both `ai_cost` lines carry the id; turn log `turn_started` 1 and 2 |
| L5 | **pass**: job `b4fc4dd0` every 2 m; wrapper `b4c9f1a594774d50` scheduled, complete/none, `model: null`, 0 calls, `job: disk-full-l4-ghost-watch`; ghost `3cd36f03eb72446d` complete, turns 2, tool_calls 1 |
| L6 | **pass**: GLM chat 4 / ghost 2, one model-less run skipped and counted, `--by kind --json` parses with `skipped_no_model: 1`, `--min-runs 100` prints `no runs in this window`, profile shows the two rows and `no failures`, `no-such-model` names GLM and exits 0 |
| L7 | **pass**: `cap $5.0000 of $1.00 (500%)`; the refusal at `--resume` before any typing and again after the ask; no `ai_cost` written after the restart; daemon.log `Webhook: analysis for 'disk-full-l4' skipped — daily cost cap`; the job fired 4 s after the restart, ghost `3149822d8fe44be2` `budget_exceeded`, not attributable, briefing `budget_left: "Usd"`, wrapper `061fd0d5044d4228` complete (O3); `budget_refused` chat ×2, webhook, ghost; job cancelled |

**Findings.** None. M30's exit criteria hold live.

**Harness.** H7: the driver must not `pgrep -f` for the chat client,
because the pattern matches the driving shell's own command line; the
`/proc` `HOME=$R` scan is the only process check. A Ctrl-D sent to a
pane whose chat has already quit closes the window (window 0 was
recreated; nothing else was affected).

**Next.** The Close steps, on the PE's sign-off: retrospective, `NEXT.md`
to none, 2.3.0.

### Update — 2026-10-25 (L8 rehearsal, architect)

Release build at `69c368f6` (phases 11–14 in), on a root rebuilt from the
real config as the pre-flight now says. The 2026-10-23 reboot had cleared
`/tmp`. The first chat greeting failed with `No models loaded` because the
copied `base_url` pointed at an LM Studio host. With `brain:8000` it
worked, and that failed greeting left a `hard_fail / backend_error` chat
run, which the detail view later drew. Teardown killed only `HOME=$R`
processes; the PE's daemon was untouched.

| Block | Result |
|---|---|
| L1 | **pass**: `80afab71fe694469` chat, complete/none, turns 2, tool_calls 1, success 1.0 |
| L4 | **pass on the first alert**: `ghost_trigger: true`; ghost `4897c66857ed4234` complete/none, turns 2, tool_calls 1, tagged `runbook: disk-full-l4` |
| L8 | **pass** at 160×47, then 100×39 |

L8 in detail:

- **Header:** `daemoneye · pid 477673 · up 2m 19s · ollama/nvidia/GLM-5.3-Flash-FP4-Local · pinky · 08:52:19`. The clock matched `date` to the second, and Activity times are in the same zone.
- **Layout:** at 160 columns Runs and Budget share the top row, with Budget 40 wide on the right. At 100 columns the four panels stack in order.
- **Rows:** the ghost's SUBJECT is `disk-full-l4 ← webhook`, the chats read `chat:<8>`, and the shells read `chat:fe9731b4` and `ghost:disk-full-l4`. The seeded burst shows once as `cache_refresh_error Failed to get active pane for session 'm30' ×5`.
- **Budget:** the bar was yellow (`38;5;3`) at 85 % and red (`38;5;1`) after the $0.30 seed, with `[███…] 115%` and `spend: $1.15 of $1.00 cap (115%)`.
- **Focus:** `Tab` cycles Runs → Budget → Shells → Activity → Runs, `BackTab` goes from Runs to Activity, and `4` jumps to Activity. The focused border is cyan (`38;5;6`).
- **Activity:** while all of today's rows fit, `↑` scrolls nothing and the tail stays, which is correct. Once they overflow, `↑` shows `activity scrolled` and the top row holds while new rows arrive. `f` resumes tailing.
- **Detail:** `1`, `Home` and `Enter` on the ghost row open ` Run 4897c66857ed4234 ` with status, failure, model, turns, `tool calls: 1 · 100.0% ok`, tokens, cost, wall clock and the four turn-log records. `Esc` returns with Runs focus.
- **Quit:** `q` leaves a prompt, and `stty -a` shows `icanon,echo`.

**Findings.** F1 (nit, cosmetic): the Runs, Shells and Activity titles
draw with doubled padding (`╭  1 Runs  ─`) while Budget's is single
(`╭ 2 Budget ─`), because `draw_panel` pads a title `panel_title` already
padded. For the PE to file or defer.

**Observations.** A completed run reads `failure: none (not the model)`
(review 14's nit). A ghost's `command` row reads `ghost-disk-full-l4 - df
-h / exit -`, because ghost commands log no shell id or exit code.

**Next.** The PE run of L8, on a fresh root, with the PE attached
read-only. Then the Close steps.

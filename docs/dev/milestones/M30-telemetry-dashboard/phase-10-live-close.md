# Phase 10: live-close — telemetry, the daily cap and `top`, checked against a real daemon; 2.3.0 ships

**Milestone:** M30 — Telemetry & dashboard
**Status:** todo
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
     waits for one before its banner.
   - Once a window is split, `send-keys` targets panes by id.
   - The chat's first input is `/approvals on` (auto-approve).
   - S-last kills only processes whose `/proc/<pid>/environ` has
     `HOME=$R`. The PE's real daemon, in `~/.cargo/bin`, is never touched.
2. **Model.** The local GLM at `brain:8000`. It is unpriced, so each
   `ai_cost` is `$0`. The cap block therefore seeds its spend on disk
   instead of buying it.
3. **Config in `$R`** (copied from `/tmp/de29-live`):
   - the webhook on `127.0.0.1:19418` with `severity_threshold = "info"`
     and `dedup_window_secs = 0`;
   - no `[budget]` until L7;
   - one runbook, `disk-full-l4`, whose frontmatter allows a ghost run, so
     a matching alert gets watchdog analysis and then a ghost.
4. **Side effects.** Local only: a throwaway daemon, restarts inside `$R`,
   and webhook POSTs to `127.0.0.1`. No ssh, nothing pushed.

## Blocks

Each block appends what it saw to `/tmp/e2e-m30-10.txt`. Events are read
from `$R/.daemoneye/var/log/events/events-<today>.jsonl`, and tool results
from the session's history file, never from GLM's prose.

| # | Block | Pass when |
|---|---|---|
| L1 | **Run record: chat.** Ask "Run `hostname` and tell me the result." | One `run` event for the turn with `kind: chat`, the GLM `provider`/`model`, `status` complete, `failure_class: none`, `tool_calls ≥ 1`, `tool_success_rate: 1.0`, and `tokens > 0`. Every `ai_cost` line the turn wrote carries that `run_id` |
| L2 | **Shells panel, live (deferred from 08).** With the L1 chat still attached, run `HOME=$R $BIN top --once` | Shells lists `s0` with the chat's owner, foreground `shell prompt` and a last line containing `pinky`. Nothing in the panel reads `daemon not running` |
| L3 | **`top`, live.** In a second emulator window, run `HOME=$R $BIN top`. Ask the chat to run `sleep 8; echo done-L3` | While the command runs, Runs shows the chat run `live` with the selection on it, and Activity gains rows without a key press. After `q` the pane is back at a shell prompt, and `stty -a` there shows `icanon` and `echo`. A second `top` quits on Ctrl-C the same way |
| L4 | **Webhook → watchdog → ghost.** POST an alert named `disk-full-l4` | The watchdog's `ai_cost` is recorded. A `run` event with `kind: ghost` has tags naming `disk-full-l4`, and its `ai_cost` lines carry its `run_id`. The ghost's briefing exists, and its budget shows tokens used, not an unlimited budget |
| L5 | **Scheduled.** Ask the chat to schedule an interval ghost job on `disk-full-l4` every 2 minutes. Wait for one firing | A `run` event with `kind: scheduled` and `model: null`, plus the ghost run it started, as in L4 |
| L6 | **Scorecard and profile, daemon down.** `daemoneye stop`, then `scorecard`, `scorecard --by kind --json \| jq`, `scorecard --min-runs 100`, `profile <GLM model>` and `profile no-such-model` | The scorecard has a GLM row with `n_runs` counting L1–L5, and no `scheduled` record is counted against the model. `--by kind` separates `chat` from `ghost`. The JSON parses. `--min-runs 100` drops every row. The profile shows the strengths rows and the ranked classes without `none`. `no-such-model` names GLM and exits 0 |
| L7 | **The daily cap, all three paths.** With the daemon down, add `[budget] max_cost_per_day = 1.0`, append one local-today `ai_cost` line of `$5.00` to the segment, and start the daemon. Then run `status`, ask the chat anything, POST `disk-full-l4`, and wait for the L5 job's next firing | `status` shows the spend against the cap. The chat gets the `SystemMsg` naming `$5.00` and `$1.00`, and the turn writes no new `ai_cost`. daemon.log has the webhook's `skipped — daily cost cap` line. The scheduled ghost ends `BudgetExceeded` with a briefing. Each path leaves one `budget_refused` event naming it. Finally, cancel the L5 job |

## Close (after the PE run passes)

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

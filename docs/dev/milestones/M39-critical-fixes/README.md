# M39 — Critical fixes

**Goal:** the 2.10.0 audit's exploitable holes and broken core flows are
closed. A webhook cannot run a shell command through tmux. A ghost cannot
leave its sandbox, type unprompted into a remote shell, or reach root
through a script it can rewrite. Multi-turn ghost runs work on all three
providers and report the failures they hit. A command that never stops
printing still times out. Cost accounting counts uncached input, and the
known panic paths no longer panic.

**Status:** in progress — phase 13 (live-check finding 1) added at the close on PE request; then the 2.11.0 release

**Release:** 2.11.0

**Depends on:** M38 (the tree as released in 2.10.0).

**Sources:** [`docs/design/daemoneye_audit.md`](../../../design/daemoneye_audit.md)
and its triage ledger,
[`docs/design/daemoneye_audit_ledger.md`](../../../design/daemoneye_audit_ledger.md).
The IDs in the phase table are the ledger's.

**Where the audit goes (PE decision 2026-11-21).** Five milestones, in
this order, then the MCP surface (M44) and Knowledge (M45):

| Milestone | Release | Scope |
|-----------|---------|-------|
| M39 Critical fixes | 2.11.0 | the audit's P0: exploit-level security and broken core flows |
| [M40 Security hardening](../M40-security-hardening/README.md) | 2.12.0 | the remaining High/Medium/Low security rows |
| [M41 Correctness & reliability](../M41-correctness/README.md) | 2.13.0 | the test hazards and gate blind spots first, then the remaining correctness bugs and issue #4 (cron time zones) |
| [M42 Performance & concurrency](../M42-performance/README.md) | 2.14.0 | blocking I/O off the runtime, lock scopes, linear scans, leaks |
| [M43 Architecture & debt](../M43-architecture/README.md) | 2.15.0 | doc drift, dependencies, protocol and executor seams, `DaemonState`, module cycles |

**Decisions (architect proposals at scoping, for PE sign-off):**

1. **A defect is fixed with a test that fails first.** Every row gets a
   named test that fails on the 2.10.0 tree and passes after. The phase
   doc names the test and what it asserts, and the executor runs it red
   before the fix (M29's and M32's rule).
2. **Each row is re-verified at drafting.** The audit is an outside
   report. The scoping spot check confirmed every headline claim it tried
   (see the ledger's `Verified` column), but a row that does not hold
   against the tree is rejected in the ledger, not fixed.
3. **Lean, closed specs.** Each phase names its files (a closed list),
   its edit order and its first edit, and gives every Spec step a
   terminating condition. No dictated code.
4. **Security fixes fail closed.** A new check refuses when it cannot
   decide (an unparseable shell id, an unresolvable host, a path that
   will not canonicalize). Model-facing refusal text names what was
   refused and why.
5. **Open design questions are settled at drafting, with the PE.** They
   are named in the phase rows below (S3a's digest or root-owned copy,
   S2b's prompt for remote input; S1's was settled at drafting).

**Exit criteria:**

- Every ledger row targeted at M39 reads `act: M39 phase-NN` with a named
  test, or `reject: <why>` by PE decision.
- A webhook alert whose `alert_name` is `#(touch /tmp/de-pwn)`, delivered
  to a session with a chat pane, creates no file, and the overlay shows
  the text literally. Live on pinky.
- A two-turn ghost run (one tool call, then a final answer) completes on
  Anthropic, and the same runbook run against a deliberately bad API key
  ends `Failed` with the provider's error, not `Completed`. Live on
  pinky.
- `run_terminal_command` of `yes` with `timeout_secs: 5` returns within
  about 5 s, interrupted, with bounded output.
- `cargo test` passes with at least the 2.10.0 count plus one per fixed
  row. `host_sends_lagged_when_subscriber_falls_behind` passes 20 runs in
  a row. `tests/hygiene.rs` and `tests/doc_truth.rs` pass.
- `2.11.0` is released per `REXYMCP.md` § Versioning.

## Architecture references

- `docs/architecture.md` § 1.2 (the executor, the stream loop, ghost runs)
  and § 1.3 (the AI provider layer).
- `docs/security.md` (the trust boundaries the security rows cross).
- The ledger rows for each phase.

## Phases

Drafted one at a time with `/rexymcp:architect next`. Each phase may split
at drafting.

| #  | Phase | Ledger | Status |
|----|-------|--------|--------|
| 01 | tmux-format-injection ([phase-01-tmux-format-injection.md](phase-01-tmux-format-injection.md)) — the chat-pane overlay moves to `tmux::show_overlay`, which escapes `#`→`##` and `%`→`%%`, strips control characters and passes `--`. Decided at drafting: escaping, not `-l`, since `-l` needs tmux ≥ 3.4 and Ubuntu 22.04 / Debian 12 ship older. The other `display-message` sites pass fixed `-p` formats and are unchanged | S1 | done |
| 02 | open-shell-gate ([phase-02-open-shell-gate.md](phase-02-open-shell-gate.md)) — `open_shell` from a ghost takes the foreground arm's sandbox / startup-failure gate. Every executor arm a ghost reaches (open, list, find, close, context, send_input) gives a ghost call `Owner::Ghost`, so ghost teardown closes the shells it opened. A ghost's local foreground command is always refused and never touches a shell, so the foreground arm is out of scope (corrected 2026-11-22, second refinement). First phase written in the statement-of-work style | S2a | done        |
| 03 | send-input-gate ([phase-03-send-input-gate.md](phase-03-send-input-gate.md)) — `send_input` refuses a shell id that does not parse, or one the registry does not hold, before any socket or prompt. The approval request carries the shell's `HostId` from `ShellRegistry`, so remote input is never classed as a local command. Input that spells `sudo` across a newline or through single-character `keys` reaches the sudo check. Decided at drafting (PE, 2026-11-22): remote input takes the **per-host grant**, exactly like `run_terminal_command` on that host | S2b S2c | done |
| 04 | sudoers-pin ([phase-04-sudoers-pin.md](phase-04-sudoers-pin.md)) — a NOPASSWD rule cannot be reused by rewriting the script. `GhostPolicy::is_safe` accepts only the exact root-owned copy's path, not a basename. Decided at drafting (PE, 2026-11-23): a **root-owned copy** at `/usr/local/libexec/daemoneye/<euid>/<name>`, because the dev host's sudo-rs 0.2.13 has no `sha256:` digest in its sudoers grammar. Re-run `install-sudoers` after each edit | S3a S3b | done                                                 |
| 04b | sudoers-drift ([phase-04b-sudoers-drift.md](phase-04b-sudoers-drift.md)) — the root-owned copy stays authoritative, and drift from the source script is made visible, never synced: `daemoneye doctor` WARNs on each installed copy that differs from its source, naming the fix (`install-sudoers <name>`); a ghost sudo call that resolves to a stale copy runs the vetted copy and publishes a `Warning` notice; `write_script` on a script with an installed copy says the sudo version is unchanged until re-install. No auto-sync, and no refusal on drift. Raised in review by DaemonEye (deepseek-v4), 2026-11-23 | S3a (follow-up) | done                       |
| 05 | sudo-detection ([phase-05-sudo-detection.md](phase-05-sudo-detection.md)) — `command_has_sudo` sees through quote and backslash splitting, leading redirections and globs, and the other elevators and wrappers the audit lists. Fail closed: a word it cannot resolve counts as elevation | S3c | done |
| 06 | wrap-remote ([phase-06-wrap-remote.md](phase-06-wrap-remote.md)) — `GhostPolicy::wrap_remote` is dead code: no production caller since commands stopped running through tmux. A ghost's `ssh_target` runs in a PTY shell on its resolved `[hosts]` host, or is refused. Decided at drafting (PE, 2026-11-24): **delete it** and its tests, rewriting the comments that still describe it, rather than harden it. A ghost's own `ssh …` command bypassing its `hosts:` fence is logged as an egress observation for the sandbox work | S3d | done |
| 07 | ghost-turn-history ([phase-07-ghost-turn-history.md](phase-07-ghost-turn-history.md)) — a ghost turn persists an `assistant` message with the tool calls and then a `user` message with the results, the same shape as `conversation_helpers.rs`. A two-turn ghost conversation converts correctly for all three providers | C1 | done |
| 08 | ghost-turn-errors ([phase-08-ghost-turn-errors.md](phase-08-ghost-turn-errors.md)) — a `chat()` error reaches the turn loop as `AiEvent::Error`. A stream that ends without `Done` is a failure, and the run ends `Failed`. The spawned chat task is cancelled with the run | C2 | done |
| 09 | pty-deadline ([phase-09-pty-deadline.md](phase-09-pty-deadline.md)) — `client_read.rs` checks the slice and command deadlines before every read, so continuous output still times out and interrupts. The output buffer is capped | C3 | done |
| 10 | shell-host-framing ([phase-10-shell-host-framing.md](phase-10-shell-host-framing.md)) — the shell host parses each pipelined frame from its own offset. `host_sends_lagged_when_subscriber_falls_behind` stops racing | S4b Q12 | done           |
| 11 | anthropic-usage ([phase-11-anthropic-usage.md](phase-11-anthropic-usage.md)) — `parse_anthropic_start_usage` reports `input_tokens` as Anthropic sends it, so cached turns cost what they cost | C4 | done      |
| 12 | panic-hazards ([phase-12-panic-hazards.md](phase-12-panic-hazards.md)) — `wrap_words` handles whitespace-only lines and wide characters at width 1. An empty `[models]` table is a config error, not a panic. The audit's byte-slice sites use `truncate_chars` | C5 Q1 Q2 | done      |
| 13 | webhook-live-config ([phase-13-webhook-live-config.md](phase-13-webhook-live-config.md)) — a webhook ghost reads the daemon's config handle, so `reload` reaches it. An unconfigured model name is warned about, and the log names the model actually used. Found in the M39 live check; the PE asked for it before 2.11.0 | live | review    |

## Notes

- **Scoped 2026-11-21** from the 2.10.0 audit. Numbering: M39–M43 are
  the audit, so the MCP surface becomes M44 and Knowledge M45.
- **Ordering inside M39.** The security rows go first because they are
  reachable from an unauthenticated localhost listener or from a ghost.
  07 lands before 08 so that once errors are reported, the runs that
  fail are not failing on C1. C5 was partly fixed by M32 phase-05; phase
  12 re-verifies the whitespace-only case first.
- **Still open from earlier milestones:** GitHub issue #13 (a `wait:
  false` shell blocked within its first probe never produces a notice).
  Issue #4 (cron time zones, due before March 2027) is M41 phase 10b.

## Retrospective (2026-11-24)

**Exit criteria.**
- **Met:**
  - **Ledger.** Every row targeted at M39 reads `M39 NN`. 15 are fixed,
    each with a named test that went red first. One is latent and
    hardened anyway: S4b, whose pipelining test is a preservation test.
    The last row, Q12, is the flaky test that now passes. S3d's target,
    `GhostPolicy::wrap_remote`, turned out to be dead code at drafting,
    so phase 06 deleted it instead of hardening it.
  - **Test count.** `#[test]` / `#[tokio::test]` attributes went from
    3156 at `v2.10.0` to 3231, which is +75 against 16 fixed rows.
    `host_sends_lagged_when_subscriber_falls_behind` ran 20 of 20 in a
    row, as a single test per pass. `tests/hygiene.rs` and
    `tests/doc_truth.rs` passed in the phase-12 review's full run.
- **Live check, 2026-11-24.** Debug binary at `d41b474e`, throwaway
  `HOME=/tmp/de39-live`, private tmux, GLM-5.3-Flash at brain. The config
  was the M38 harness copy, with the webhook moved to port 19439.
  - **Webhook injection: met.** An alert named `#(touch /tmp/de-pwn)`,
    with `#{pane_id} %H` in its summary, was delivered to a session with
    a chat pane. `/tmp/de-pwn` was never created. The client's terminal
    stream shows the text literally: `[Webhook Alert] [FIRING] #(touch
    /tmp/de-pwn) — #(touch /tmp/de-pwn) #{pane_id} %H literal check`.
  - **Two-turn ghost: met on GLM, Anthropic leg not run.** Runbook
    `ghost-two-turn` called `list_memories` once and then gave a final
    answer ("listed 10 memories"). The run record reads `complete`, 2
    turns, on the `openai` provider. No Anthropic key was available to
    the harness, so the success half on Anthropic is unverified live.
    **Waived by the PE (2026-11-24).**
    Phase 07's unit tests cover all three providers' history shapes.
  - **Bad key: met after a restart.** Runbook `ghost-bad-key`
    (`model: badkey`, provider `anthropic`, invalid key) produced a run
    record of `hard_fail` with 1 turn. The notice read `[Ghost Shell
    Failed] … AI error: API error 401 Unauthorized: … invalid x-api-key`.
    Before the restart, with `badkey` added by `reload`, the same run
    ended `Completed` on GLM. See finding 1.
  - **`yes` with `timeout_secs: 5`: partly met.** The call came back
    interrupted, with a bounded 441-byte result. The shell `s0` cast shows
    Ctrl-C typed at 5.03 s and the prompt back at 5.10 s. But the result
    arrived at about 10.2 s, not about 5 s. See finding 2.

**Found in the live check:**
1. **A webhook ghost reads the startup config, not `config::current()`.**
   **PE (2026-11-24): fix before 2.11.0, as phase 13.** Keep the fallback
   but warn: three of the PE's real agents name unconfigured models.
   `process_alert` hands `trigger_ghost_turn` `state.config`, a snapshot
   taken at start. A `[models.*]` entry added by `daemoneye reload` is
   invisible to it, and `Config::resolve_model` silently falls back to
   `default`. The log line still says `using model 'badkey'` while the
   call goes to GLM. This breaks the invariant in `CLAUDE.md`, and the
   silent fallback for an unknown model name hides it. Proposed for an
   M41 ledger row: route the ghost through `config::current()`, and make
   an unknown runbook `model` an error, or at least log the fallback.
2. **A timed-out command that stops on Ctrl-C always waits out the 5 s
   grace.** The interactive Ctrl-C aborts the whole wrapped line, so the
   end marker never prints. `read_to_marker` then waits the full
   `INTERRUPT_GRACE` even though the shell is back at its prompt. This
   is the behaviour phase 09 specified ("the deadline plus the interrupt
   grace"). The "about 5 s" wording of the exit criterion was the
   architect's. Proposed for M41 or M42: end the grace when the shell
   returns to its prompt.

**Outcomes:** 13 phases (01–12, plus 04b).
- 4 `approved_first_try`: 06, 07, 11, 12.
- 4 `approved_after_1`: 02, 04b, 09, 10.
- 1 `approved_after_2`: 05.
- 1 `approved_after_3`: 04.
- 3 `escalated`:
  - 01: a green-bounce poll loop ran to the turn cap.
  - 03: run 1 hard-failed on E0382, and a resume finished it.
  - 08: two stalls at 200+ turns with no edits, then a takeover.

**What went well.**
- **Re-verifying each row at drafting paid off.** Phase 06's target was
  dead code. Phase 12's three items were reproduced at runtime before
  drafting, and every red run matched the probe.
- **The batching rule held.** It is one exact command, run once. Phase
  10's first run hard-failed on `IdenticalToolCallRepetition` because it
  ran `cargo test` once per repetition. Phases 11 and 12 were drafted
  under the rule and finished in 56 and 83 turns, with no repetition.
- **The systemic-reader list.** Phase 07 asked for every reader of the
  changed history shape and found a bug no named test targeted.

**What cost the most.**
- **Audit rows verified only by reading.** C3's deadline half (phase 09)
  and S4b (phase 10) did not reproduce. Each cost false red runs and a
  bounce.
- **Sudoers needed a real install.** Phase 04 bounced three times: the
  umask on implicit parents, and `@includedir` skipping dotted names.
  `visudo -cf` passed a file that sudo never reads.
- **Security detectors with named spellings.** Phase 03 named `Space` and
  `Enter`, and phase 05 named `-c`. Each missed other spellings of the
  same construct, three bounces in all.
- **Read paralysis.** Phase 02's statement-of-work runs made 0 edits in
  1039 and 1200 turns, and phase 08 stalled twice. Naming the first red
  test (phase 02, run 3), or taking over (phase 08), ended them.

**Calibration folded (PE sign-off 2026-11-24):** A, B and C below are in
`WORKFLOW.md`. The held item stays held.
- **A. Reproduce a defect before drafting its phase.** This has come up
  three times: 06, 09 and 10, and 12 did it right. Proposed fold: an
  audit or bug row is made to fail at runtime (a scratch test or a probe)
  before the phase is drafted. A row that cannot be made to fail is
  labelled latent, and its test is a preservation test.
- **B. One exact command, run once.** This has come up three times: M38
  08b and 09c, and M39 10. Proposed fold: every check the executor must
  repeat ships as an exact one-liner that loops internally, with "run it
  once". The rexyMCP-side half (a contract rule and a nudge before the
  hard fail) is drafted as a rexyMCP issue, not yet filed.
- **C. Security detectors pin behaviour, not spellings.** This has come
  up three times: 03, 05-1 and 05-2. Proposed fold: the spec derives the
  spellings from the parser ("agrees with the bytes sent"), and lists
  must-stay-false examples for quoted arguments.
- **Held (two occurrences):** a phase that writes system state runs its
  live effect probe before the first approval (04, 04b).

**Release notes must say:** re-run `daemoneye install-sudoers <script>`
for every installed script. Rules for dotted script names were never read
by sudo before 2.11.0, and the rule now names a root-owned copy. Sudo
detection now fails closed: an unresolvable command word counts as
elevation.

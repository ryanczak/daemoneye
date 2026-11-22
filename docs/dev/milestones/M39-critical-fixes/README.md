# M39 — Critical fixes

**Goal:** the 2.10.0 audit's exploitable holes and broken core flows are
closed. A webhook cannot run a shell command through tmux. A ghost cannot
leave its sandbox, type unprompted into a remote shell, or reach root
through a script it can rewrite. Multi-turn ghost runs work on all three
providers and report the failures they hit. A command that never stops
printing still times out. Cost accounting counts uncached input, and the
known panic paths no longer panic.

**Status:** planning (scoped 2026-11-21)

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
| 03 | send-input-gate — `send_input` refuses a shell id that does not parse. The approval request carries the shell's `HostId` from `ShellRegistry`, so remote input is never classed as a local command. Input that spells `sudo` through `keys` or quoting reaches the sudo check. *Decide at drafting:* whether remote `send_input` may be session-approved at all | S2b S2c | todo |
| 04 | sudoers-pin — a NOPASSWD rule cannot be reused by rewriting the script. `GhostPolicy::is_safe` accepts only a canonical path inside `scripts_dir()`, not a basename. *Decide at drafting:* a `sha256:` digest in the rule (re-run `install-sudoers` after each edit) or a root-owned copy | S3a S3b | todo |
| 05 | sudo-detection — `command_has_sudo` sees through quote and backslash splitting, leading redirections and globs, and the other elevators and wrappers the audit lists. Fail closed: a word it cannot resolve counts as elevation | S3c | todo |
| 06 | wrap-remote — `GhostPolicy::wrap_remote` wraps every command, `ssh …` included, and passes `target` quoted after `--` | S3d | todo |
| 07 | ghost-turn-history — a ghost turn persists an `assistant` message with the tool calls and then a `user` message with the results, the same shape as `conversation_helpers.rs`. A two-turn ghost conversation converts correctly for all three providers | C1 | todo |
| 08 | ghost-turn-errors — a `chat()` error reaches the turn loop as `AiEvent::Error`. A stream that ends without `Done` is a failure, and the run ends `Failed`. The spawned chat task is cancelled with the run | C2 | todo |
| 09 | pty-deadline — `client_read.rs` checks the slice and command deadlines before every read, so continuous output still times out and interrupts. The output buffer is capped | C3 | todo |
| 10 | shell-host-framing — the shell host parses each pipelined frame from its own offset. `host_sends_lagged_when_subscriber_falls_behind` stops racing | S4b Q12 | todo |
| 11 | anthropic-usage — `parse_anthropic_start_usage` reports `input_tokens` as Anthropic sends it, so cached turns cost what they cost | C4 | todo |
| 12 | panic-hazards — `wrap_words` handles whitespace-only lines and wide characters at width 1. An empty `[models]` table is a config error, not a panic. The audit's byte-slice sites use `truncate_chars` | C5 Q1 Q2 | todo |

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

# M40 — Security hardening

**Goal:** the rest of the audit's security rows are closed. No read on a
socket the daemon serves is unbounded. Spawned shells do not inherit
provider API keys. File edits cannot be redirected through a symlink or a
predictable temp name. Names that become paths are validated. Ghost
delegation and schedule deletion go through policy and approval. The
webhook listener authenticates before it parses. Masking does what its
rules say.

**Status:** phases complete 2026-11-27 — retrospective written; awaiting PE sign-off and the 2.12.0 release

**Release:** 2.12.0

**Depends on:** M39 (released as 2.11.0).

**Sources:** the security rows of
[`docs/design/daemoneye_audit_ledger.md`](../../../design/daemoneye_audit_ledger.md)
not taken by M39.

**Decisions (proposed at scoping, signed off by the PE 2026-11-25):**

1. M39's decisions 1–4 carry over unchanged: a failing test first,
   re-verification at drafting, lean closed specs, fail closed.
2. **A tool that starts prompting changes its budget.** Phases 06b and 07
   make `spawn_ghost_shell` (interactive) and `cancel_schedule` /
   `delete_schedule` approval-gated. Each one joins `APPROVAL_GATED_TOOLS`,
   gets its README `**⚠**`, and passes `tests/doc_truth.rs`. That list
   also exempts a tool from the budget caps (CLAUDE.md § Adding a new AI
   tool, item 10), so each phase states that consequence.
3. **Untrusted text is framed, not sanitized.** Alert payloads that enter
   watchdog, ghost and chat prompts are wrapped in a marked
   untrusted-data block. Their content is not rewritten.

**Exit criteria:**

- Every ledger row targeted at M40 reads `act: M40 phase-NN` or
  `reject: <why>`.
- A 64 MiB line sent to the daemon socket or a shell-host socket is
  refused without the process's RSS growing by that much (a test with a
  byte budget, not a live check).
- `env` run in a fresh PTY shell, a shell host, and an `on_alert` command
  shows no `ANTHROPIC_API_KEY`, `OPENAI_API_KEY` or `GEMINI_API_KEY`
  while the daemon has them set.
- `read_file` of a symlink into `~/.ssh/` and `edit_file` through a
  two-level missing path under a symlink to `~/.daemoneye/` are both
  refused.
- A webhook POST with a wrong secret and a 2 MiB body is refused before
  the body is parsed.
- `cargo test`, `tests/hygiene.rs` and `tests/doc_truth.rs` pass.
  `2.12.0` is released per `REXYMCP.md` § Versioning.

## Architecture references

- `docs/security.md` — the trust boundaries.
- `docs/architecture.md` § 1.1 (transport, the socket and shell hosts)
  and § 3 (Ghost Shells).

## Phases

Drafted one at a time with `/rexymcp:architect next`. Each phase may split
at drafting.

| #  | Phase | Ledger | Status |
|----|-------|--------|--------|
| 01a | [bounded-reads](phase-01a-bounded-reads.md) — `read_bounded_line` (cap before allocation) and `accept_bounded` (a semaphore permit before `accept`), applied to the daemon's first request read, the shell host's frames and both accept loops | S4a | done |
| 01b | [in-connection-reads](phase-01b-in-connection-reads.md) — the approval, credential and prompt replies, and the presence, notices and tail loops, read through `read_bounded_line` | S4a | done |
| 02 | [secret-env](phase-02-secret-env.md) — provider key variables are removed from the environment of `PtyShell::spawn`, `spawn_detached` and the command sink's `deliver_command` | S5 | done |
| 03 | [temp-files](phase-03-temp-files.md) — remote edits write through an `O_EXCL`, 0600 temp file the payload creates (`tempfile` / `File::Temp`), so a pre-planted `.de_tmp` symlink is never followed; local edits use `create_new` + mode 0600 + a random suffix | S6 | done |
| 04 | [path-guard](phase-04-path-guard.md) — `resolve_path_for_guard` canonicalizes the deepest existing ancestor and re-joins the rest; `is_blocked_secret_path` checks the canonical path | S7 | done |
| 05 | [name-validation](phase-05-name-validation.md) — one validator (`[A-Za-z0-9_-]`, non-empty, bounded length) applied at IPC ingress and inside the path builders for memory namespaces, session ids and prompt names | S8 | done |
| 06a | [ghost-fence](phase-06a-ghost-fence.md) — a ghost's `spawn_ghost_shell` narrows the child to the caller's hosts (`host: None` included), scripts, flags and tool policy; disjoint host lists refuse | S9 | done |
| 06b | [ghost-spawn-approval](phase-06b-ghost-spawn-approval.md) — `spawn_ghost_shell` from interactive chat is approval-gated (a dedicated prompt, joining `APPROVAL_GATED_TOOLS`) | S9 | done        |
| 07 | [schedule-delete-gate](phase-07-schedule-delete-gate.md) — `cancel_schedule` / `delete_schedule` are refused to ghosts without policy and approval-gated in chat | S12 | done        |
| 08 | [argv-injection](phase-08-argv-injection.md) — `HostId::parse` rejects a leading `-`; ssh argv builders put `--` before the destination; remote `grep` takes `-e`; `infocmp` takes `--` | S11 | done        |
| 09a | [webhook-auth](phase-09a-webhook-auth.md) — constant-time secret compare, auth and rate limit before the body is parsed, the secret read through `config::current()` so a reload rotates it | S10 | done        |
| 09b | [webhook-fanout](phase-09b-webhook-fanout.md) — one POST's alert array fans out under a bound; the rate-limit map evicts; alert text enters every prompt inside an untrusted-data block | S10 | done        |
| 10 | [masking](phase-10-masking.md) — the JSON secret rule keeps its key (capture expansion actually happens, test through `mask_sensitive`, not the rule table); the credential round trip stays in `Zeroizing`; the secret-bearing structs redact in `Debug` | S13 S14 | done        |
| 11 | [prompt-ids](phase-11-prompt-ids.md) — every prompt-response handler drops a response whose id is not the prompt's (nine approve/deny handlers and the credential round trip) | S15 | done        |
| 12 | [fd-hygiene](phase-12-fd-hygiene.md) — `pipe2(O_CLOEXEC)` for the readiness pipe; `getpwuid_r` retries on `ERANGE` (S17 rejected at drafting: Rust's runtime reopens a closed fd 0–2 before `main`) | S16 S18 | done        |

## Notes

- **Scoped 2026-11-21** with M39. Order: risk first. The IPC surface
  (01) and secret exposure (02–04) lead, then path validation and the
  policy gates (05–07). The argv and webhook phases (08, 09a, 09b) follow,
  and the masking and fd tidies close the milestone.
- **Phase 01 split at drafting (2026-11-25)** into 01a (the two tools,
  the two socket entry points and the accept loops, 5 files) and 01b (13
  reads inside an open connection across 9 files). One phase would have
  touched about 12 files. Both reads in 01a were reproduced with 64 MiB
  probes: every byte was accepted before any refusal.

## Retrospective (2026-11-27)

**Exit criteria.**
- **Met:**
  - **Ledger.** Every row targeted at M40 is triaged. S4a and S5–S16 and
    S18 read `act: M40 phase-NN`, each with a test that went red first.
    S17 reads `reject:`. A live probe at drafting showed Rust's runtime
    reopens a closed fd 0–2 on `/dev/null` before `main`, so the defect
    cannot happen.
  - **Test count.** `#[test]` / `#[tokio::test]` attributes went from
    3241 at `v2.11.0` to 3336, which is +95. `tests/hygiene.rs` and
    `tests/doc_truth.rs` passed in the phase-12 review's full run.
- **Live check, 2026-11-27.** Debug binary at `566d85dc`, throwaway
  `HOME` under `/tmp`, removed afterwards. The daemon ran with sentinel
  values in `ANTHROPIC_API_KEY`, `OPENAI_API_KEY` and `GEMINI_API_KEY`. No
  model was called.
  - **64 MiB line: met.** An unterminated 64 MiB write to the daemon
    socket was refused at 1 MiB with `Request too large (limit 1048576
    bytes)`. The daemon's RSS grew by 416 kB, and it still answered
    `ping`. The same write to a shell host's socket was refused at its
    16 MiB frame cap, and that host's RSS grew by 276 kB. Phase 01a's
    byte-budget tests are the criterion. The probe confirms them on the
    real binary.
  - **Provider keys: met.** All three sentinels were in the daemon's
    environment. A shell opened with `ShellOpen` gave a shell host and a
    PTY `bash` that held none of them. A `command` route on webhook
    notices ran `env` from a real alert, and its output held none of them
    either.
  - **Webhook: met.** A 2 MiB body with a wrong secret got `401` in 3 ms.
    So did the same body with no header. With the right secret the same
    body got `413`, and a small alert got `200`.
  - **Symlinked paths: met by test, not live.** Phase 04's tests refuse a
    `read_file` of a symlink to `~/.ssh/id_rsa` and of a file reached
    through a symlinked `~/.ssh`. They also refuse an `edit_file` through
    a missing path under a symlink to `~/.daemoneye/`. A live leg needs a
    model call, and the tests call the production validators.
  - `2.12.0` is released after PE sign-off.

**Outcomes:** 15 phases (01a–12, with 01, 06 and 09 split at drafting).
- 3 `approved_first_try`: 02, 08, 11. Phase 11's first run stalled and
  was resumed, but no review bounced it.
- 9 `approved_after_1`: 04, 05, 06a, 06b, 07, 09a, 09b, 10, 12.
- 3 `escalated`:
  - 01a and 01b: the bounces were test mechanics, so Claude fixed the
    tests directly.
  - 03: the bounce run stalled at turn 66 with the fix nearly done.

**What went well.**
- **M39's folds held.** No phase hit `IdenticalToolCallRepetition`. Each
  row was reproduced before drafting, which is how S17 was rejected
  without a phase.
- **A canned-reply harness for approval tests.** Phase 06b's approval
  tests hung when the gate was skipped. Phase 07 drove the prompt through
  a `Vec<u8>` writer and a `Cursor` reader, and landed 18 files in 119
  turns with no hang.
- **Probing new parsers with hostile input at review.** Phase 09b's
  neutraliser panicked on `ẞ`. That surfaced only because the review fed
  it non-ASCII input.

**What cost the most.**
- **Spec claims about today's behaviour that nobody probed.** There were
  five occurrences:
  - 08 claimed `-oProxyCommand=x` parsed as a host name.
  - 09a asserted `{}` was a `400` and that the startup bind check was
    unconditional.
  - 11 named a fixture's failure class but not the fixture.
  - 12 probed the user's own uid but not the missing uid the criterion
    named.

  Each cost a bounce, a stall or an architect spec error.
- **Bounces on test quality, not production code.** These include:
  - 01a: a test that waited on the wrong thing.
  - 01b: a test that raced two `select!` branches.
  - 05: a test that planted its file where `../evil` does not point.
  - 06b: a test that hung rather than failing.

  The reviewer found 01a and 05 by mutation, and 01b by a stress run.
  None of them was in the executor's evidence.
- **Read stalls.** 01a, 03, 06b and 11 each lost a run to
  `TaskNoEditStall` or `NoProgressStall`. What ended each:
  - 01a: a resume with the diagnosis.
  - 11: a resume with the five edits named.
  - 06b: a refined re-dispatch with the facts it was hunting for.
  - 03: a takeover.

**Calibration (PE 2026-11-27: A is held, not folded):**
- **A. Probe every current-behaviour claim at drafting (5 occurrences).**
  Extend `WORKFLOW.md` § "Reproduce a defect before drafting its phase"
  rather than adding a section. Every statement a spec makes about how
  the code behaves today gets a one-line probe at drafting, using the
  exact input the criterion names. That covers a bypass example, an
  error code, a fixture a change will break, and a limit's boundary. A
  claim that is not probed is not written.
- **Held (two occurrences each):**
  - **Mutation evidence covers every test it claims.** In 05, one pasted
    red run hid four vacuous tests. In 07, a mutation proved only the
    approved leg.
  - **A stress loop proves it ran the test.** This came up in M39 phase
    10 and in 01b. The fix is to pin `1 passed` in any stress criterion.
- **One occurrence, noted:**
  - Say "move, never copy" for secret plumbing (10).
  - Name the argument a conditional rule depends on in the function's
    inputs (06a).
  - A summary claim is not evidence: check `git diff --stat` (09b).

**Telemetry corrected (PE 2026-11-27).** Five bounces had been recorded
as `false_completion`, but none of them had a red gate. Their approvals
were re-recorded, and the latest review is the one the profile reads:
- 06b: `missing_spec_test`.
- 07 and 09b: `prod_unwrap`. Both were a panic in a production path.
- 09a and 10: `spec_bug`. The spec made a false claim or was too
  implicit.

**Release notes must say:**
- `spawn_ghost_shell` from chat, and `cancel_schedule` and
  `delete_schedule`, now ask for approval. Approval-gated tools are exempt
  from the per-turn budget caps.
- Ghosts can no longer remove schedules.
- Daemon-socket requests over 1 MiB, and shell-host frames over 16 MiB,
  are refused.
- Webhook bodies over 2 MiB are refused. Authentication runs before the
  body is read, and a `reload` rotates the secret.
- Names that become paths (memory namespaces, session ids, prompt names)
  must match `[A-Za-z0-9_-]`.

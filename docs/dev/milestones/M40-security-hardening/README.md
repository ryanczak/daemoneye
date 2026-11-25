# M40 — Security hardening

**Goal:** the rest of the audit's security rows are closed. No read on a
socket the daemon serves is unbounded. Spawned shells do not inherit
provider API keys. File edits cannot be redirected through a symlink or a
predictable temp name. Names that become paths are validated. Ghost
delegation and schedule deletion go through policy and approval. The
webhook listener authenticates before it parses. Masking does what its
rules say.

**Status:** active (scoped 2026-11-21, signed off by the PE 2026-11-25)

**Release:** 2.12.0

**Depends on:** M39 (released as 2.11.0).

**Sources:** the security rows of
[`docs/design/daemoneye_audit_ledger.md`](../../../design/daemoneye_audit_ledger.md)
not taken by M39.

**Decisions (proposed at scoping, signed off by the PE 2026-11-25):**

1. M39's decisions 1–4 carry over unchanged: a failing test first,
   re-verification at drafting, lean closed specs, fail closed.
2. **A tool that starts prompting changes its budget.** Phases 06 and 07
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
| 01a | [bounded-reads](phase-01a-bounded-reads.md) — `read_bounded_line` (cap before allocation) and `accept_bounded` (a semaphore permit before `accept`), applied to the daemon's first request read, the shell host's frames and both accept loops | S4a | in-progress |
| 01b | in-connection-reads — the approval, credential and prompt replies, and the presence, notices and tail loops, read through `read_bounded_line` | S4a | todo |
| 02 | secret-env — provider key variables are removed from the environment of `PtyShell::spawn`, `spawn_detached` and the command sink's `deliver_command` | S5 | todo |
| 03 | temp-files — remote edits write through an `O_EXCL`, 0600 temp file the payload creates (`tempfile` / `File::Temp`), so a pre-planted `.de_tmp` symlink is never followed; local edits use `create_new` + mode 0600 + a random suffix | S6 | todo |
| 04 | path-guard — `resolve_path_for_guard` canonicalizes the deepest existing ancestor and re-joins the rest; `is_blocked_secret_path` checks the canonical path | S7 | todo |
| 05 | name-validation — one validator (`[A-Za-z0-9_-]`, non-empty, bounded length) applied at IPC ingress and inside the path builders for memory namespaces, session ids and prompt names | S8 | todo |
| 06 | ghost-delegation — `spawn_ghost_shell` always intersects the child's hosts, scripts and tool policy with the caller's, `host: None` included; from interactive chat it is approval-gated | S9 | todo |
| 07 | schedule-delete-gate — `cancel_schedule` / `delete_schedule` are refused to ghosts without policy and approval-gated in chat | S12 | todo |
| 08 | argv-injection — `HostId::parse` rejects a leading `-`; ssh argv builders put `--` before the destination; remote `grep` takes `-e`; `infocmp` takes `--` | S11 | todo |
| 09a | webhook-auth — constant-time secret compare, auth and rate limit before the body is parsed, the secret read through `config::current()` so a reload rotates it | S10 | todo |
| 09b | webhook-fanout — one POST's alert array fans out under a bound; the rate-limit map evicts; alert text enters every prompt inside an untrusted-data block | S10 | todo |
| 10 | masking — the JSON secret rule keeps its key (capture expansion actually happens, test through `mask_sensitive`, not the rule table); the credential round trip stays in `Zeroizing`; the secret-bearing structs redact in `Debug` | S13 S14 | todo |
| 11 | prompt-ids — every prompt-response handler drops a response whose id is not the prompt's | S15 | todo |
| 12 | fd-hygiene — `pipe2(O_CLOEXEC)` for the readiness pipe; `dup2` never closes its own target fd 0–2; `getpwuid_r` retries on `ERANGE` | S16 S17 S18 | todo |

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

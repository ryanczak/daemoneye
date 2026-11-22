# M43 — Architecture & debt

**Goal:** the structural debt the audit named is paid down where it has
a real seam. The docs describe the tree. The dependency duplicates are
gone. The IPC protocol has no dead or one-sided variants. The executor
no longer needs an IPC socket to run a tool, there is one ghost spawner,
daemon code reaches shared state through `DaemonState`, and the worst
module cycles are broken.

**Status:** planning (scoped 2026-11-21; the later phases are re-scoped
at the M42 boundary)

**Release:** 2.15.0

**Depends on:** M42 (released as 2.14.0).

**Sources:** the architecture and maintainability rows of
[`docs/design/daemoneye_audit_ledger.md`](../../../design/daemoneye_audit_ledger.md)
(A3 and A4a went to M41 as bugs).

**Decisions (architect proposals at scoping, for PE sign-off):**

1. **Seams, not line counts.** No phase here is justified by file or
   function length (PE, 2026-10-31). The 22- and 24-parameter functions
   are in scope because a status struct is a real seam, not because they
   are long.
2. **Design before refactor.** 08 and 09 each start with a design note
   the PE approves: the target layering (which modules may import which),
   and which globals move into `DaemonState` and in what order. The phase
   rows for 08 and 09 stand in for series that are scoped from those
   notes.
3. **The architect takes docs phases directly.** The executor has
   stalled on docs-only phases (M38 phase-09c: hundreds of turns with no
   edit). Phase 01 is a direct edit, reviewed the same way.
4. **A refactor keeps behaviour.** The full suite passes at the same
   count, and no model-facing string changes.

**Exit criteria:**

- Every ledger row targeted at M43 reads `act: M43 phase-NN` or
  `reject: <why>`.
- `AGENTS.md` and `docs/architecture.md` name the real socket path, no
  removed global, and no removed subcommand.
- `cargo tree -d` shows one `base64` and no `nix`.
- No production function takes more than the clippy default number of
  parameters without a stated reason.
- `cargo test`, `tests/hygiene.rs` and `tests/doc_truth.rs` pass.
  `2.15.0` is released per `REXYMCP.md` § Versioning.

## Architecture references

- `docs/architecture.md` § 1 (the layers this milestone re-draws).
- [`docs/design/consolidate-state-finish-2.0-clean-up.md`](../../../design/consolidate-state-finish-2.0-clean-up.md)
  — the earlier `DaemonState` consolidation this finishes.

## Phases

| #  | Phase | Ledger | Status |
|----|-------|--------|--------|
| 01 | doc-drift — `AGENTS.md`, `docs/architecture.md` (and its roadmap) re-synced with the tree; the tmux-only-caller invariant is restated to match the code or the stray callers move | A8 | todo |
| 02 | deps — `base64` on one version; `nix` replaced by `std::fs::File::try_lock`; dead `search_dir` removed | Q7 Q8 | todo |
| 03 | daemon-status-struct — the status-section functions take a struct instead of 22–24 positional values | Q6 | todo |
| 04 | ipc-protocol — dead and one-sided variants removed or wired; `Ask.model` honoured or removed; `handle_resume_run` reports errors as `Response::Error`; a protocol version in the handshake | A7 | todo |
| 05 | pending-call — `id` and `thought_signature` lifted out of the 45 variants; dead `_credential` removed; `route` matches once | A6 | todo |
| 06 | tool-interaction — `execute_tool_call` takes a prompt/feedback trait instead of raw reader/writer; ghosts stop passing `sink()`/`empty()` | A5 | todo |
| 07 | ghost-spawners — the five spawn sites go through one spawner with one set of lifecycle events | A4b | todo |
| 08 | daemon-state — series: the remaining globals move behind `DaemonState`, one family per phase, after the design note | A2 | todo |
| 09 | module-cycles — series: a low-level domain/storage layer breaks the cycles, starting with `daemon -> cli` and `config -> cli`, after the design note | A1 | todo |

## Notes

- **Scoped 2026-11-21** with M39. After M43, the MCP surface is M44 and
  Knowledge M45. 06 and 07 make an MCP server easier to add, since it
  would be a third caller of the executor after chat and ghosts. Keep
  that in mind when ordering at the M42 boundary.

# REXYMCP.md

The rexyMCP architect/executor workflow contract for the DaemonEye project —
whatever agent acts as the architect reads this first.

## Read these first

1. `docs/dev/STANDARDS.md` — engineering Definition of Done.
2. `docs/dev/WORKFLOW.md` — phase lifecycle, status transitions, Update
   Log templates.
3. `docs/dev/NEXT.md` — names the active phase.
4. `docs/architecture.md` — the design.

## Commands

| Command | Purpose |
|---|---|
| `cargo fmt --all` | Format check |
| `cargo build` | Build |
| `cargo clippy --all-targets --all-features -- -D warnings` | Lint / static analysis |
| `cargo test` | Tests |

## Executor

Phases are executed by a **local LLM** reached through the rexyMCP MCP
server (`rexymcp serve`). The executor's contract is **embedded** in the
server binary — there is *no* root `AGENTS.md` or executor-contract file
in this repo.

To dispatch a phase: `/rexymcp:dispatch <phase>`. To review the result:
`/rexymcp:review <phase>`.

## Versioning — one minor release per milestone

DaemonEye's minor version tracks rexyMCP milestones. **Each milestone close
bumps the minor by one** and resets the patch: M27 closed as `2.0.0`, so M28
closes as `2.1.0`, M29 as `2.2.0`, and so on. The milestone number is not the
minor; the CHANGELOG entry names the milestone.

- **Patch** (`2.1.1`): a fix released between milestone closes, with its own
  CHANGELOG entry.
- **Major**: only by explicit PE decision, as 2.0 was.
- **The release is part of the milestone close**, after the retrospective and
  the PE's sign-off:
  1. Bump `version` in `Cargo.toml`, and rebuild so `Cargo.lock` follows.
  2. Add a `## [x.y.0] — YYYY-MM-DD — M<n> <title>` entry to `CHANGELOG.md`,
     drawn from the milestone README's goal and exit criteria.
  3. Run all four gates.
  4. Commit as `chore: release DaemonEye x.y.0 (M<n>)`, using the PE's git
     identity.
  5. Create an annotated tag `vx.y.0`.
  6. **Push only on the PE's explicit go-ahead**, after a credential scan of
     the unpushed commits.
- The milestone README's header names its target version, for example
  `**Release:** 2.1.0`, so the number is fixed when the milestone is scoped.

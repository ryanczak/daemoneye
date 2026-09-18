# NEXT

**Active phase:** none. **No milestone is in progress** — the next step is to
scope one.

**M22 (Tool surface & context) closed 2026-09-18, PE signed off.** The one
calibration trigger landed in `WORKFLOW.md` § Acceptance criteria.

**M20 (Shell engine) closed 2026-09-18, PE signed off** — the record was the
last stale one in the tree. It was code complete 2026-09-09 with every exit
criterion met and its retrospective written; only the `Status:` line and the
sign-off were outstanding. Re-verified at the close: all headline deliverables
present, `cargo test --lib 'shell::'` → 254 passed.

20 phases, 12 bugs all closed, one executor
(`local-inference-lab/GLM-5.3-Flash-NVFP4-Spark`). Nine phases approved
first-try, eight after one bounce, two after two, one by architect takeover.
The retrospective and the exit-criteria table are in
`docs/dev/milestones/M22-tool-surface/README.md`.

**Headline:** the AI no longer names a tmux pane to run a command. Six pane
tools out, nine shell tools in, 39 → 42 tools with `Category` on every one, and
zero `%`-pane identifiers left in any tool description.

**Inherited by later milestones, deliberately:** M21 takes `target_pane` on
`read_file` / `edit_file` and the approval panel's shell/host naming; M23 takes
the pane context blocks, the `[HOSTS]` block and the remaining `sre.toml` pane
prose; M25 is now unblocked by `Category` / `mutates_state()`.

**Upstream push backlog:** the new `WORKFLOW.md` § Acceptance criteria
pre-dispatch check is local-only — `plugin/templates/WORKFLOW.md` in rexyMCP
does not carry it yet.

**Next step: scope a milestone.** `/rexymcp:architect next` has nothing to
draft until one is active. Three candidates, none scoped, all PE-ordered:

| Candidate | Why now | Note |
|---|---|---|
| **M23** — Client decoupling & shell UX | Chat outside tmux; `/shells`, `/tail`, the `/agents` picker. The design says **M20–M23 alone satisfy the stated 2.0 requirement** | Inherits M22's pane context blocks, `[HOSTS]`, and the four `%N` in `sre.toml` |
| **M21** — Host model & SSH | `[hosts]`, `Host::Ssh` shells, ControlMaster, prompt detection | **M23's `[HOSTS]` block cannot exist until a host concept does**, so M21 arguably precedes it |
| **M25** — Governor & guards | Ten detectors, advisory novelty, `daemoneye calibrate-governor` | Dependency satisfied: M22 put `Category` + `mutates_state()` on all 42 tools |

Run `/rexymcp:architect` (no args) to scope the chosen one.

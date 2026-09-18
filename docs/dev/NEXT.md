# NEXT

**Active phase:** none — **M22 (Tool surface & context) closed 2026-09-18**,
awaiting PE sign-off.

20 phases, 12 bugs all closed, one executor
(`local-inference-lab/GLM-5.3-Flash-NVFP4-Spark`). Nine phases approved
first-try, eight after one bounce, two after two, one by architect takeover.
The retrospective and the exit-criteria table are in
`docs/dev/milestones/M22-tool-surface/README.md`.

**Headline:** the AI no longer names a tmux pane to run a command. Six pane
tools out, nine shell tools in, 39 → 42 tools with `Category` on every one, and
zero `%`-pane identifiers left in any tool description.

**Awaiting PE decision, two items:**

1. **Sign off on the close.** Three exit criteria are met and three are
   partially met, every shortfall deferred by measurement and recorded in the
   phase that found it. `[HOSTS]` does not exist because no host concept does;
   the pane context blocks and the four `%N` in `sre.toml` go with M23.
2. **Sign off on one calibration trigger** — not a new fold. Both fold
   candidates from this milestone already exist in `WORKFLOW.md` and were not
   applied at drafting. The proposal adds a pre-dispatch checklist item to
   § Acceptance criteria pointing at the two existing sections. Details in the
   retrospective's Calibration section.

**Inherited by later milestones, deliberately:** M21 takes `target_pane` on
`read_file` / `edit_file` and the approval panel's shell/host naming; M23 takes
the pane context blocks, the `[HOSTS]` block and the remaining `sre.toml` pane
prose; M25 is now unblocked by `Category` / `mutates_state()`.

**Next step is yours:** sign off, then run `/rexymcp:architect` to scope the
next milestone.

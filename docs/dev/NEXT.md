# NEXT

**Active phase:** none — **M22 (Tool surface & context) closed 2026-09-18,
PE signed off.** The one calibration trigger landed in `WORKFLOW.md`
§ Acceptance criteria.

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

**Next step:** `/rexymcp:architect` to scope the next milestone.

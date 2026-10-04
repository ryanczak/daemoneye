# NEXT

**Active phase:** **M23 phase-08a — attach-signals**, drafted 2026-10-04,
status `todo`. Dispatch with `/rexymcp:dispatch phase-08a`.

**What it does.** While attached, `Ctrl-p` pauses the shell's foreground
job and `Ctrl-p` again resumes it; `Ctrl-c` interrupts it. Both go through a
new `ShellSignal` IPC relay, which answers with the state the shell actually
settled in, so "paused" is shown only when something really stopped.
Detaching while paused resumes the job. 7 files, 7 new tests; lib
2193 → 2200.

**Row 08 was split at drafting:**
- 08a (this) is the shell-level chords;
- 08b ties them to the owning run (cooperative cancel with stage
  `user_attached`, park/resume with the pause duration);
- 08c is accountability (`events.jsonl`, telling the agent, scrubbing).

**Measured at drafting.** A real-host test pins the four state transitions.
The settle poll's mutant was unobservable and was dropped. **The capture has
no trailing whitespace**, which is the 07b fix. The end-to-end block was run
on the dry-run tree. Mutants A–F went red 3/3, all clippy-clean. The doc
reproduces the dry run.

**For the PE:** detach-while-paused **resumes** the job. That was my call;
the design doesn't say. Revisit if you want a paused shell to stay paused.

**After 08a:** `/rexymcp:architect next` drafts 08b (attach-run-control).

# NEXT

**Active phase:** **M21 phase-04b — ssh-tool-surface**, drafted 2026-09-20,
status `todo`. Dispatch with `/rexymcp:dispatch phase-04b-ssh-tool-surface`.

**What it does.** `open_shell` gains an optional `host`, and the shell registry
stops hard-coding every record's host as local. After it lands the model can
reach a remote host with tools that already exist: `open_shell { host: "pinky" }`
returns a shell id, and `run_terminal_command { shell: "<id>" }` runs there with
a real remote exit code.

**Row 04 has now split twice, both times on a measured seam.** Drafting found
three facts that reshaped it:

- `spawn_ssh_host` has **zero callers** — 04a built the SSH spawn and nothing
  can reach it.
- Every `ShellRecord.host` is hard-coded `HostId::Local`; only one production
  site constructs a record, so the plumbing is a few lines.
- `marker_shell_name`, `list_shells` and the `[SHELLS]` block **already read**
  `record.host`. They have simply never had a named host to print.

Because `run_terminal_command { shell }` already targets an open shell by id,
`open_shell { host }` alone delivers end-to-end remote execution. The one-shot
`run_terminal_command { host? }` form became **row 04c**, where host-keyed
reuse belongs with it. Thirteen rows in M21 now.

**Two defects the phase exists to prevent, both found by reading, not by a
failing test.** The reuse scan is host-blind, so a remote shell would silently
have caught the next local command. And `open_shell` funnels every spawn
failure into one message, so an unknown host would have reached the model as
*"the per-owner shell limit is 5. Close one of your shells"* — advice that
cannot fix it. Each has a named test and a mutation.

**A pre-existing flake is recorded in the spec, not fixed.**
`the_flag_clears_once_the_answer_is_submitted` failed once in four full-suite
runs and passed 6 of 6 alone. It is a fixed 50 ms sleep standing in for "bash
has set echo-off" (phase-01b, `src/shell/backend.rs:623`). The spec tells the
executor to re-run and report, never to touch it. **It deserves its own bug
doc** — a timing race in the suite can red-gate any future phase.

**Fold decision taken 2026-09-20 (PE sign-off).** The standing M16 proposal
("validate every criterion in both directions", five occurrences, never
applied) is **retired as superseded**. What landed is rule 3 in the
pre-dispatch check under § Acceptance criteria: a criterion pinning an absence
must pin the behaviour, not the file's text. Its note records the new fact —
the first criterion an executor satisfied by *degrading the code* rather than
reporting it unmet. Meta-pattern now at three: the content already existed and
the trigger did not fire at drafting. A fourth occurrence gets a mechanical
pre-dispatch step, not more prose.

**The new rule caught its first defect during this drafting.** A criterion
pinning `grep -c 'fn spawn_for_on_host' src/shell/registry.rs` → 1 would have
returned 2, because the phase's own test is named
`spawn_for_on_host_records_the_named_host` and contains the string. It was cut
rather than anchored: the two tests beside it already pin the property.

Live host `pinky`, reachable with key auth and no prompt; remote login shell is
`bash`. `max_turns` 1000.

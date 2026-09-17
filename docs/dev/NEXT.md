# NEXT

**Active phase:** `docs/dev/milestones/M22-tool-surface/phase-09d-pane-param-retirement.md`
— 09d, pane-param-retirement. Drafted 2026-09-17, status `todo`, not yet
dispatched.

Retires `target_pane` and `retry_in_pane` from **`run_terminal_command` only**.
No tool-count change (42 = 28 + 14), but it closes the milestone's *other*
headline exit criterion: **zero `%`-pane identifiers in the tool surface**.
`defs.rs` currently holds exactly one, at `:80`, in `target_pane`'s description.

**Three measurements reshaped the charter row:**

1. **`target_pane` is declared on three tools, not one** — `:77`
   (`run_terminal_command`), `:321` (`read_file`), `:390` (`edit_file`). On the
   latter two it is the **remote file** path (`file_ops/ops.rs:34`), whose
   successor is **M21's `host?`**. Retiring it there now would delete remote
   file ops with no replacement, so 09d leaves them and pins their survival
   both as a must-survive criterion and as a negative assertion in its E2E test.
2. **It is not vestigial.** `executor/mod.rs:319` uses it as a deliberate
   per-call escape hatch routing *around* the PTY substrate, and `:445` does the
   same for `retry_in_pane` on the background path — **two** conjuncts. Removing
   them is the intended behaviour change; keeping `uses_pty`, `!is_ghost` and
   `session_id.is_some()` is what preserves the `backend = "tmux"` and ghost
   paths.
3. **The replacement already exists.** `foreground.rs:160`/`:204` already
   resolve `SessionEntry.default_target_pane` with a stale-pane guard. The phase
   removes the AI's *choice* of pane, not the daemon's ability to target one.

All three folds from 09a–09c are applied: the orphan closure is walked outward
(and finds `target_pane` is **not** orphaned — only `retry_in_pane` is, with
`respawn_background_in_pane` flagged as the one to re-check); criteria prefer
identifiers, with two deliberately loosened to "at least 1" because the phase
edits the file they live in; and the Authorizations block now states explicitly
what to do when a forced orphan sits outside the authorized set — proceed and
label the decision as **the executor's own**, or stop, but **do not narrate a
human who was never asked**.

After 09d the remaining M22 order is **08b → 10**.

Dispatch with `/rexymcp:dispatch phase-09d`.

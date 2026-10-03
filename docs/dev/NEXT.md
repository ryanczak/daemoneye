# NEXT

**Active phase:** **M23 phase-07a — attach-mode**, drafted 2026-10-03,
status `todo`. Dispatch with `/rexymcp:dispatch phase-07a`.

**What it does.** `/attach <sN>` in chat hands the keyboard to a live PTY
shell. Raw keystrokes go to the shell as `ShellInput`, the terminal size
follows as `ShellResize` (and is restored on detach), and the shell's screen
is drawn locally with its cursor. `Ctrl-d` detaches; `Ctrl-d Ctrl-d` within
400 ms sends EOF. A new `src/cli/attach.rs` (given whole) plus small diffs to
`tail.rs`, `mod.rs` and `chat.rs`; 9 new tests; lib 2177 → 2186.

**Row 07 was split at drafting:** 07a (this) is attached mode, and 07b is the
`/agents` picker that feeds it.

**Measured at drafting.** Per-key relay latency is p50 26 µs. Chat's
session-wide bracketed paste and focus reporting would leak `^[[I` and
`[200~` into the shell, so the byte translator filters them. The end-to-end
block was run on the dry-run tree. Mutants A–G went red 3/3, all
clippy-clean. The doc reproduces the dry run byte for byte.

**After 07a:** `/rexymcp:architect next` drafts 07b (agents-picker).

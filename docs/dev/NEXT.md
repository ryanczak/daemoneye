# NEXT

**Active phase:** **M23 phase-10b — ghost-start-without-tmux**, drafted
2026-10-04, status `todo`. Dispatch with `/rexymcp:dispatch phase-10b`.

**Why it exists.** Drafting the live close found that starting a ghost
creates a tmux server (`ensure_incident_session()` ignores
`tmux_session = ""`), and that a local ghost has no PTY shell for `/agents`
to attach to. The PE chose two fix rows before the live close: 10b (this
one), 10c (local ghost commands in PTY shells), 10d (live close).

**What it does.** The daemon records its tmux mode once at startup; a ghost
started with tmux off gets an empty tmux session and is never refused for
it. Two files, two pure helpers with unit tests; lib 2219 → 2221.

**Measured at drafting.** Through the real daemon binary, the current tree
leaves a `tmux-1000/` server directory after a ghost and the dry run leaves
none. The end-to-end block was run on the dry-run tree. Mutants A–B went
red 3/3, both clippy-clean; the call-site mutant is unobservable and pinned
by the `wired` grep. The doc reproduces the dry run.

**After 10b:** `/rexymcp:architect next` drafts 10c (local ghost PTY shells;
the sandbox case needs a decision at drafting), then 10d, the live close.

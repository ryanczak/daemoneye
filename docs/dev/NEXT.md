# NEXT

**Active phase:** none.

Phase-08a closed `approved_after_1` on 2026-09-16 — the `[SHELLS]` /
`[DEFAULT SHELL]` ambient context block, injected at both prompt builders and
the `get_terminal_context` result. One bounce (`bug-08a-1`, closed): two
assertions that could not fail, and an E2E note stating a mechanism that was
not the one at work. Tool counts unmoved — still 48 tools: 34 core + 14
deferred, `doc_truth` 10.

Next up is **08b — shell-context-tool** (`get_terminal_context` →
`get_shell_context { scope }` with a shell-shaped scope vocabulary), **not yet
drafted** and deliberately sequenced **after 09** removes the pane tools:
renaming the context tool while six pane tools still populate the context would
leave the name describing something it does not exclusively return.

So the drafting order from here is **09 → 08b → 10**. Run
`/rexymcp:architect next` to draft phase-09 (pane-tool-removal).

Still deferred out of the original row 08: `[HOSTS]` (no host concept exists —
measured five times now) and the approval panel naming shell + host (an IPC wire
+ client renderer change). Both wait for M21/M23.

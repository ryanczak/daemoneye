# M32 findings ledger

What the executor saw while fixing (M31 decision 7, carried by M32
decision 5), triaged at review. Same columns and triage values as
[`M31-code-hygiene/FINDINGS.md`](../M31-code-hygiene/FINDINGS.md):
`act: <phase or bug doc>`, `defer: <milestone>`, `reject: <why>`.

| # | Phase | File · symbol | Kind | Severity | What | Triage |
|---|-------|---------------|------|----------|------|--------|
| 1 | 01 | `executor/dispatch_foreground.rs` · `ghost_foreground_refusal`, the `Err` arm | smell | minor | the refusal formats the load error with `{e}`, which prints only anyhow's outermost context (`parsing <path>`), then names the path again in its remedy. The doubled path is the executor's finding. Verified at review: the TOML parse error itself, the one thing the operator needs, is dropped. `{e:#}` prints the chain, and the remedy's second path can go. The unit test passed because its error has no context | act: M32 phase-14 (small-tidies), with a test whose error carries a `with_context` layer |
| 2 | 01 | `executor/dispatch_foreground.rs` · `dispatch` | smell | minor | 145 of the 150-line cap. Its next edit must split it first, and the split must keep the `refuse_foreground` fall-through reachable: run 1's extraction dropped that guard | act: M32 phase-08 (host-refusal) edits this function; its spec must state the headroom and pin `refuse_foreground(` as a preservation check |
| 3 | 01 | `executor/dispatch_foreground.rs` · `ghost_foreground_refusal` | smell | nit | the unreadable-config refusal starts `Error:` and the sandbox-on one does not; the second is the pre-existing text, carried verbatim | defer: post-M32; changing a model-facing refusal string is a behaviour change with no defect behind it |

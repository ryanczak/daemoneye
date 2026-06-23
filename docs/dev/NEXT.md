# NEXT

**Active milestone:** M1 — Agent Tooling Improvements
(`docs/dev/milestones/M1-agent-tooling/README.md`)

**Active phase:** phase-11-on-demand-tool-loading
(`docs/dev/milestones/M1-agent-tooling/phase-11-on-demand-tool-loading.md`) —
`todo`, drafted 2026-06-22 (re-verified against current `TOOLS` 2026-06-23: all
nine deferred names present, `ToolDef` still 3-field, architecture.md §1.3
matches). Splits `TOOLS` into an always-loaded **core** set and a **deferred**
set via a self-declaring `ToolDef.deferred_group: Option<&'static str>`; the
default render emits core only, and a new core `load_tools` meta-tool pulls a
deferred group into the session so deferred schemas stop shipping on every
request. Threads a `loaded_tools: HashSet<String>` through `SessionEntry` and the
`chat` trait (interactive + ghost loops). `Depends on: none` — sequence 08 → 11
for clean diffs; 08/09/10 are all done.

Dispatch with `/rexymcp:dispatch phase-11-on-demand-tool-loading`.

phase-10 (tmux-surface-and-safe-verbs) is **done** (approved_first_try
2026-06-23). phase-09 (error-suppress-audit) is **done** (approved_after_1
2026-06-23; bug-phase-09-1 fixed). phase-08 (prompt-and-tooldef-fixes) is
**done** (approved_first_try 2026-06-22).

phase-11 is the last drafted phase in M1 — on its approval, M1 reaches a
milestone boundary (retrospective + human sign-off before any M2).

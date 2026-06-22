# NEXT

**Active milestone:** M1 — Agent Tooling Improvements
(`docs/dev/milestones/M1-agent-tooling/README.md`)

**Active phase:** phase-03-script-exec-hardening
(`docs/dev/milestones/M1-agent-tooling/phase-03-script-exec-hardening.md`),
status `todo`. Drafted 2026-06-21. Scope: `src/scripts.rs` only — tighten
`validate_script_name` to a `[A-Za-z0-9._-]` allowlist and escape
sudoers-special characters in `sudoers_rule`.

Dispatch via `/rexymcp:dispatch phase-03`.

Note: the original row-03 bundle was split — remote script transfer is now
phase-04, write-tool `target_pane` parity is phase-05, and namespace ACL /
execution-robustness / prompt-fixes shifted to 06/07/08. All remain `todo` and
undrafted.

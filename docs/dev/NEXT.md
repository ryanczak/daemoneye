# NEXT

**Active milestone:** M1 — Agent Tooling Improvements
(`docs/dev/milestones/M1-agent-tooling/README.md`)

**Active phase:** phase-04-remote-script-execution — **reopened** (`in-progress`)
on 2026-06-22 after the remote-execution model was reset (architecture § 2.4:
daemon host stores all managed artifacts; remotes are execution targets, not
storage targets). v1 (persistent remote materialize) was approved, then reopened
because persistent remote write can no longer be the default.

Re-dispatch the revised spec via `/rexymcp:dispatch phase-04`. The revision:
make **streaming** (pipe the script to a remote interpreter's stdin, no remote
disk) the default; keep the v1 persistent materialize **only for the `sudo`
case** (a NOPASSWD sudoers rule needs a fixed authorized path).

Note: the milestone redirection also **dropped** the old phase-05 ("write-tool
target_pane parity" — managed-artifact tools are daemon-host-only by design) and
**repurposed** phase-05 to interactive remote script execution. See the README
§ Notes → "Remote-execution model redirection (2026-06-22)".

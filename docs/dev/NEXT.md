# NEXT

**Active phase:** **M23 phase-01d — session-log-retention**, drafted
2026-10-01, status `todo`. Dispatch with `/rexymcp:dispatch phase-01d`.

**What it does.** Adds `[sessions] log_retention_days` (default `0`, keep
forever). Once a session has been inactive for that many days, it deletes the
session's chat log (`<id>.jsonl`) and resume state (`<id>.meta.json`). It
never touches archives or epoch files, which feed the memory index, and never
an active session. Swept every 60th cleanup tick, beside the archive sweep.
Three new tests: 2098 / 14.

**Measured at drafting.** `var/log/sessions/` holds four kinds of file per
session. The index reads only `.archive.jsonl` and `.epochs.jsonl`, so
deleting the plain log and its meta strands no index rows. The key name
already exists under `[shells]`, so the doc criteria use whole-word counts
against that baseline (dry-run: template 1→2, README 2→4).

**After 01d:** M21's carry-forwards are done; `/rexymcp:architect next`
drafts row 02 (no-tmux-startup).

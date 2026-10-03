# NEXT

**Active phase:** **M23 phase-06a — shell-log-list**, drafted 2026-10-02,
status `todo`. Dispatch with `/rexymcp:dispatch phase-06a`.

**What it does.** `/logs [host] [--owner chat|ghost|adopted] [--since 2h]`
lists past shell recordings, newest first. To filter on owner and host, the
command index now records both. They are written with the first command, and
older indexes still load. Row 06 is split: 06a lists, 06b opens a recording
with `n`/`p` command navigation, and 06c replays it.

**Measured at drafting.** Nothing on disk recorded a finished shell's owner
or host. There is no `sched` owner kind (scheduled ghosts are ghost-owned),
so `--owner sched` is refused. `CommandRecord.started/ended` are byte
offsets, not times. Dry-run green: filter printed 9, lib 2150 → 2157 ×3,
mutations A–F red. The spec quotes the per-file dry-run diffs, which were
verified to reproduce the dry run exactly.

**After 06a:** `/rexymcp:architect next` drafts 06b (log-viewer).

# NEXT

**Active phase:** `docs/dev/milestones/M21-host-model/phase-04a-ssh-shell.md`
— 04a, ssh-shell. **Drafted 2026-09-20**, status `todo`.

**Row 04 was split.** It bundled the SSH substrate with two tool-surface
changes, each an eleven-step checklist pass. Phase 03 spent 479 turns on
strictly less. 04a is the substrate; 04b is the tool surface and is undrafted.

**Every hard question was answered live against `pinky` before writing a line
of spec**, using the argv support phase 03 just landed to start a genuine
ssh-backed shell host:

- The marker protocol works over `ssh -tt` **unchanged**. `hostname` returned
  `pinky`; the end marker arrived immediately.
- Real exit codes propagate: `(exit 42)` → **42**, `false` → 1, `true` → 0.
- ControlMaster earns its flags: **157 ms cold, 4–5 ms warm**, and the control
  socket appears under the daemon-owned path.
- `pinky` runs bash with key auth, so no credential prompt is involved here.
- The remote shell's terminal-integration escape sequences do **not** break
  parsing, so the spec tells the executor *not* to add a stripper.
- A bare `exit N` kills the session, so the probe uses a subshell.

**One real defect surfaced that no test would have caught.** The marker
protocol picks its exit-variable syntax from the *daemon's* login shell, but
over SSH the *remote* shell is what interprets it. An operator whose own shell
is fish, connecting to a bash host, would emit the wrong variable and no exit
code would parse. The drafting host hid this because both ends are bash. 04a
pins remote shells to POSIX `$?` and tests it with the local shell set to fish.

Every acceptance criterion was run against HEAD `fbb48267` and fails there. Two
were wrong on the first pass and corrected: a baseline of 108 that is really
97, and an unsatisfiable criterion demanding zero occurrences of `pinky` in
source — it already appears as a test fixture, which is fine.

Dispatch with `/rexymcp:dispatch phase-04a`.

**After 04a:** 04b, the tool surface. Ten rows became eleven; live host
`pinky`; `max_turns` 1000.

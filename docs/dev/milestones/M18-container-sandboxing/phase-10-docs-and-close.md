# Phase 10: Document what shipped

**Milestone:** M18 — Container-sandboxed Agents (final phase)
**Status:** todo
**Depends on:** phases 01–09 (all `done`)
**Estimated diff:** ~180 lines, docs only
**Tags:** language=markdown, kind=docs, size=s

## Goal

Nine phases shipped a working sandbox and **`CLAUDE.md` does not mention it
once**. The README still says "3 of 10 phases are merged". This phase makes
the docs describe the system that actually exists, so the next person to open
this repo — including the M19 architect — is not reading a stale map.

**No source changes.** The pilot has already been run by the architect (see
§ Current state); this phase is the documentation half of the close-out.

## Architecture references

Read before starting:

- `docs/dev/milestones/M18-container-sandboxing/README.md` § Notes — the phase
  table is the record of what actually landed, and the PE decision that M18
  closes here with the rest carried to M19.
- `docs/design/agent-container-sandboxing.md` § D0 — the tool disposition
  table. Only `run_terminal_command` **background** mode is sandboxed;
  foreground stays host-level by design. The docs must not overclaim.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any file.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on the tree at drafting time (2026-08-29, commit `0e2b715`):

- `cargo test --lib` → **1454 passed; 0 failed; 4 ignored**. Four gates green.
- `grep -ci "sandbox" CLAUDE.md` → **0**. The sandbox is entirely undocumented
  in the file that orients every future session on this repo.
- `grep -c "executor/container" CLAUDE.md` → **0** — the § "Key files" table
  has no row for `src/daemon/executor/container.rs`, now ~1900 lines and the
  largest single addition of the milestone.
- `README.md:219-220` still reads *"The groundwork is landing now — 3 of 10
  phases are merged."* Nine are.
- `tests/doc_truth.rs` does **not** gate CLAUDE.md's § "Key files" table
  (`grep -cn "Key files\|key_files" tests/doc_truth.rs` → 0). It gates the AI
  tools tables and `assets/etc/config.toml` only. So this phase's CLAUDE.md
  edits are not machine-checked — accuracy is on you.

### The architect's pilot — already run, and it passed

Run in an **isolated `tmux -L de-pilot3` server started with no `DOCKER_HOST`
in its environment**, which is the exact configuration that was broken before
phase-07. The pane confirmed `PANE_DOCKER_HOST=[UNSET]`, then the shipped
window command produced:

```
1000
PILOT_OK
drwx------ 2 de de 40 … /de/work
__EXIT=0
```

uid 1000 inside, a container hostname (not the host's), a writable
`0700 de:de` scratch, and **`__EXIT=0`** — the exit status the `de-bg-*`
completion detection reads. The pilot found **no new defects**. Facts you may
state as true in the docs.

Two things the pilot did **not** cover, and which the docs must therefore not
claim: the daemon's **startup sweep** has never run through a real daemon (the
operator's daemon holds the single-instance flock), and no **AI-driven**
background command has gone through the full chat path. Three stale
`de-stage-*` volumes remain on the host as the sweep's fixture.

## Gotchas

Five traps.

1. **Do not overclaim.** Only *background* `run_terminal_command` is
   sandboxed. Foreground execution (`send-keys` into the user's pane), remote
   execution over ssh/mosh panes, and every broker-native tool are **not**, by
   design. A doc that says "agent commands run in containers" is wrong.

2. **Do not claim ghost shells are sandboxed.** Phase-09 made ghost containers
   *labelled* (`de.ghost=1`); nothing wires ghost execution to a container,
   and ghost-scoped teardown is M19 work.

3. **Do not claim script staging works end to end.** `stage_args` is correct
   and tested, but **nothing calls it** — that is why the module still carries
   `#[allow(dead_code)]`. Staging integration is M19.

4. **The README section is a `(in progress)` heading with a status
   blockquote.** Update the numbers and the framing, but keep it honest: the
   feature is still default-off and its remaining work is real. Do not
   re-title it as shipped.

5. **`assets/etc/config.toml` is already correct** — phase-01 documented every
   `[sandbox]` knob and `tests/doc_truth.rs` gates it both ways. Do not edit
   it; a stray key there fails `seeded_config_template_has_no_phantom_keys`.

## Spec

### Task 1 — Add the sandbox to `CLAUDE.md`'s § "Key files" table

Add one row, in the table's existing style and in file-path order alongside
the other `src/daemon/executor/` entries:

| Path | Role |
|---|---|
| `src/daemon/executor/container.rs` | Container sandbox: runtime probe + D1 uid gate, `evaluate_preflight`, the `docker` argv builders (`run_args`/`stage_args`), `sandbox_window_command`, the image lockfile, and the startup sweep. All decision logic is pure; one spawn site per operation. Gated by `[sandbox] enabled` (default off). |

Match the surrounding rows' voice — they are terse descriptions of *role*, not
changelogs.

### Task 2 — Add a `## Container sandbox` section to `CLAUDE.md`

Place it after § "Important Invariants". Keep it to roughly 15–25 lines and
state only what is true today:

- Background `run_terminal_command` execution is wrapped as
  `docker --host <docker_host> run … sh -lc '<cmd>'` and run **inside the
  existing `de-bg-*` window**, so completion detection, output capture and GC
  are unchanged.
- Every sandboxed process runs `--user 1000:1000`. Under rootless Docker
  container root maps to the daemon's own host uid, so running as root would
  defeat the sandbox entirely — this is the reason for the uid gate.
- Preflight (runtime probe → uid gate → `sandbox.lock` → live image id) is
  cached once per daemon lifetime and **fails closed**: a failed gate refuses
  the command with an operator-facing reason instead of running it on the host.
- Containers are `--network=none`, carry `--label de.sandbox=1` (plus
  `de.ghost=1` for ghost sessions), get a `0700` tmpfs scratch at `/de/work`,
  and are swept at daemon start along with leaked `de-stage-*` volumes.
- **Not sandboxed:** foreground execution, remote (`target_pane`) execution,
  and every broker-native tool (§ Gotchas 1).
- `daemoneye sandbox build` builds the image and records its digest in
  `~/.daemoneye/etc/sandbox.lock`; a live image that differs from the lock is
  refused.

### Task 3 — Update the README's status blockquote

`README.md:219-220`. Replace the "3 of 10 phases are merged" framing with an
accurate one: M18 is complete, the sandbox works for background command
execution, it remains **behind `[sandbox] enabled = false`**, and the
remaining work (script staging, the escape hatch, the egress proxy) is M19.
Keep the `(in progress)` heading and the default-off emphasis (§ Gotchas 4).

Do **not** restructure the rest of that README section — its bullet list is
still accurate.

### Task 4 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim** and paste its output
into a new Update Log entry titled
`### Update — <date> (end-to-end verification)`, followed by the literal
verdict line the block prints (bare, not wrapped in backticks).

## Acceptance criteria

Every count was measured against the current tree while drafting.

- [ ] `grep -c "executor/container" CLAUDE.md` prints `1` (**before: 0**).
- [ ] `grep -c "^## Container sandbox" CLAUDE.md` prints `1` (**before: 0**).
- [ ] `grep -ci "sandbox" CLAUDE.md` prints **at least 10** (**before: 0**) —
      a section that mentions the subject once is not a section. Use
      `[ "$(grep -ci sandbox CLAUDE.md)" -ge 10 ] && echo OK || echo LOW`.
- [ ] `grep -c "3 of" README.md` prints `0` (**before: 1**) — the stale phase
      count is gone.
- [ ] `grep -c "enabled = false" README.md` prints **at least 1** — the
      default-off promise survives the rewrite.
- [ ] `git diff --stat assets/etc/config.toml` is empty (§ Gotchas 5).
- [ ] `git diff --stat -- src/` is empty — **this phase changes no source.**
- [ ] `cargo test --lib 2>&1 | grep -E "^test result:"` reports
      `1454 passed; 0 failed; 4 ignored` — **unchanged**; a changed count
      means source was touched.
- [ ] All four gates green: `cargo fmt --all`, `cargo build`,
      `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- [ ] The § End-to-end entry exists and contains the literal line `PASTE MATCH`
      (bare, with no surrounding backticks).

## Test plan

**No unit tests.** This phase changes only Markdown, and the project has no
prose-linting gate that a new test could hook into. The verification is the
structural greps in § Acceptance criteria plus `tests/doc_truth.rs`, which
already gates the AI-tools tables and `assets/etc/config.toml` and must stay
green.

Adding a test here to satisfy a habit would be worse than none: it would
assert on wording that is expected to change.

## End-to-end verification

Run this block verbatim from the repo root.

```sh
{
echo "== A. doc_truth still green =="
cargo test --test doc_truth 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== B. lib suite unchanged =="
cargo test --lib 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== C. structural greps =="
echo -n "container.rs row (1):     "; grep -c "executor/container" CLAUDE.md
echo -n "sandbox section (1):      "; grep -c "^## Container sandbox" CLAUDE.md
echo -n "sandbox mentions (>=10):  "; grep -ci "sandbox" CLAUDE.md
echo -n "stale '3 of' gone (0):    "; grep -c "3 of" README.md
echo -n "default-off kept (>=1):   "; grep -c "enabled = false" README.md
echo -n "config.toml untouched (0):"; git diff --stat assets/etc/config.toml | wc -l
echo -n "src untouched (0):        "; git diff --stat -- src/ | wc -l
} > /tmp/e2e-10.txt 2>&1
cat /tmp/e2e-10.txt
```

Paste the contents of `/tmp/e2e-10.txt` into your Update Log entry as a fenced
block, then run the self-check and paste its verdict line into the same entry
**bare, on its own line, with no backticks around it**:

```sh
D=docs/dev/milestones/M18-container-sandboxing/phase-10-docs-and-close.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-10.txt
diff /tmp/pasted-10.txt /tmp/e2e-10.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

**Run the block exactly as written.** If a label in it has gone stale against
the criteria, that is a spec defect — record a blocker naming it rather than
editing the block.

## Authorizations

- Edit `CLAUDE.md` and `README.md` only.
- Run `cargo fmt --all`, `cargo build`,
  `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- **Do not edit any file under `src/`**, and do not edit
  `assets/etc/config.toml` or `containers/Dockerfile`. A criterion pins both
  diffs empty.
- **Do not run `docker`, `podman`, or any container command**, and do not
  start, stop or query a system service. The pilot is already run and its
  results are quoted in § Current state — **use those; do not re-derive them,
  and do not state any live fact this doc has not given you.**
- **Do not write the milestone retrospective or touch
  `docs/dev/milestones/M18-container-sandboxing/README.md` § Notes.**
  Milestone close is the architect's step, triggered by the human.
- **Append to the Update Log; never edit or delete an existing entry.**
- **If you cannot finish honestly — an acceptance criterion is unsatisfiable,
  *or* a gate is red for a reason this phase did not cause — record a blocker
  Update Log entry naming the exact criterion, and stop. Reporting the blocker
  *is* the successful outcome.** Do not proceed past a blocker you have filed.
- **Record what you decide, not what you wish had been decided.**

## Out of scope

- **Any source change.** If documenting the system reveals a code defect,
  **record it in a blocker entry and stop** — do not fix it here. M18 closes
  after this phase and a code fix belongs in M19.
- The milestone retrospective, the phase table, and `NEXT.md` — architect
  close-out.
- Staging integration, the escape hatch, the egress proxy,
  `Request::ContainerStatus`, the `log` relay opcode, and removing the
  `#[allow(dead_code)]` — all **M19**.
- Re-running the pilot or any live check.

## Update Log

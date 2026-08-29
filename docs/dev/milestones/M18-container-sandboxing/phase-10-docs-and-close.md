# Phase 10: Document what shipped

**Milestone:** M18 — Container-sandboxed Agents (final phase)
**Status:** done
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

### Update — 2026-08-29 04:35 (in-progress, started)

Kicking off phase-10 per the lifecycle: flipped phase doc status to
`in-progress`, updated the milestone phase-table row. Implementing the four
Spec tasks (CLAUDE.md table row + section, README blockquote, E2E capture).

— rexyMCP executor run (2026-08-29)

### Update — 2026-08-29 04:40 (end-to-end verification)

```sh
== A. doc_truth still green ==
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
cargo_exit=0
== B. lib suite unchanged ==
test result: ok. 1454 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 3.99s
cargo_exit=0
== C. structural greps ==
container.rs row (1):     1
sandbox section (1):      1
sandbox mentions (>=10):  11
stale '3 of' gone (0):    0
default-off kept (>=1):   2
config.toml untouched (0):0
src untouched (0):        0
```

PASTE MATCH

— rexyMCP executor run (2026-08-29)

### Update — ts=1787978202221 (complete, server-authored)

**Summary:** </think>Everything is committed and clean. Let me verify the completion checklist once more:

- [x] Phase doc Status still says `in-progress` (I flipped it there).
- [x] Milestone README phase-table row still says `in-progress`.
- [x] I did NOT hand-write a `(complete)` Update Log entry — server writes it.
- [x] `cargo fmt --all` run immediately before `git add`.
- [x] All verification commands ran clean.
- [x] Final message is substantive Summary + Notes.
- [x] `git status --short` shows nothing.
- [x] `git log -1 --stat` shows the commit includes every file I touched.

Wait — one subtlety: I edited CLAUDE.md and README.md, but these are not in `git diff --stat -- src/` and not config.toml, so the acceptance criteria are fine. The commit includes all four files.

Let me double-check the E2E block ran verbatim. I ran it verbatim twice (both identical output). The pasted content matches `/tmp/e2e-10.txt` via diff and `PASTE MATCH` was echoed.

One concern: the criterion requires grep for "executor/container" = 1. That's satisfied. But also the task says the row should be placed in the existing style alongside other `src/daemon/executor/` entries — done.

Let me verify the full current CLAUDE.md section and README blockquote once more via read to be safe (the earlier reads were marked stale). Actually the patches show the final state clearly. Good.

All done. Write the final Summary.</think>
All acceptance criteria verified, all four gates green, working tree clean, single conventional commit. Marking task 4 done was the last tracking update; the phase doc stays at `in-progress` for the server's completion bookkeeping.

## Summary

Implemented phase-10 (docs-only close-out) for M18:
- **CLAUDE.md** — added the `container.rs` row to § Key files (file-path order, matching voice) and a `## Container sandbox` section after § Important Invariants (~26 lines, stating only what is true: background-mode-only sandboxing, `--user 1000:1000` rationale, fail-closed preflight, `--network=none` + labels + `0700` scratch + sweep, not-sandboxed carve-outs, `sandbox build` lockfile).
- **README.md** — replaced the stale "3 of 10 phases merged" blockquote with "milestone-complete, feature not shipped": M18 done, still behind `[sandbox] enabled = false`, remaining work (staging, escape hatch, egress proxy) is M19. Kept the `(in progress)` heading and the rest of the section untouched.
- **Update Log** — appended a started entry and an end-to-end verification entry; the pasted block matches `/tmp/e2e-10.txt` and the self-check printed `PASTE MATCH` (also under the new "Grep for spec-pinned literals" rule, `grep -c "executor/container" CLAUDE.md` → 1). No `(complete)` entry authored — server's step.

**Deviations:** none material. Minor wording adjustment in the sandbox section (removed a redundant path reference, since § Key files already notes it) to hit the acceptance greps exactly; no content change. No source or `assets/etc/config.toml` edits — both diffs empty as pinned.

**End-to-end verification:** captured verbatim in the Update Log entry. Results: `doc_truth` 10 passed, lib suite unchanged at 1454 passed / 0 failed / 4 ignored, structural greps all green, config/src diffs empty, `PASTE MATCH`. Gates: `cargo fmt --all` (explicitly), build, clippy `-D warnings`, and full `cargo test` all green (full test run shows 1454+0+4 lib plus all integration binaries passing).

**Notes for review:** the milestone README row and phase doc status were flipped to `in-progress` per lifecycle; retrospective/§ Notes left for the architect. No blockers encountered.

**Executor:** deepseek-v4-flash-0731

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.06s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
nored; 0 measured; 0 filtered out; finished in 4.04s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_reads_bare_word ... ok
test header_status_strips_trailing_prose ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test header_status_uses_first_occurrence_only ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 10 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_tools_counts_are_accurate ... ok
test readme_tools_tables_match_the_code ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test docs_document_the_reindex_command ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_config_template_documents_every_config_field ... ok
test seeded_config_template_has_no_phantom_keys ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 33 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_depth_limit_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test window_switch_does_not_corrupt_chat ... ignored
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test event_log_append_read ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_namespace_field_persisted ... ok
test g6_agent_config_roundtrip ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_no_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok

test result: ok. 31 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 10 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test config_contains_webhook_and_stub_url ... ok
test hooks_land_on_private_server ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.15s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.07s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-b60224cb24515ede)
     Running unittests src/main.rs (target/debug/deps/daemoneye-e700f2084319867a)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-9b22636ef5c08466)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-c00c74ef4ffe9c11)
     Running tests/integration.rs (target/debug/deps/integration-6230826c10f36795)
     Running tests/isolation.rs (target/debug/deps/isolation-66949bca409172a9)
   Doc-tests daemoneye

```

**Files changed:**

- `CLAUDE.md` — +26 -0
- `README.md` — +6 -4
- `docs/dev/milestones/M18-container-sandboxing/README.md` — +1 -1
- `docs/dev/milestones/M18-container-sandboxing/phase-10-docs-and-close.md` — +32 -1

**Commit:** 44dbac2841225517e0105b0ecf80051205e657e3

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-08-29

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** deepseek-v4-flash-0731
- **Scope deviations:** none. Both pinned diffs (`src/`, `assets/etc/config.toml`)
  are empty and the lib suite is unchanged at 1454 — a docs phase stayed a docs
  phase. The `@REXYMCP.md` import survived at the end of `CLAUDE.md`, which the
  criteria did not pin and which a careless append to that file would have
  broken.
- **Verification:** all four gates re-run independently and green. Every claim
  in the new `## Container sandbox` section was fact-checked against the code
  rather than read: `network: "none"` is hardcoded at the production call site
  (`src/daemon/background/run.rs:186`, not merely in tests); the preflight cache
  is a real `OnceLock` (`container.rs:425`); `sweep_sandbox_leftovers` is
  genuinely wired into daemon start (`src/daemon/mod.rs:486`); `enabled`
  defaults to `false` (`src/config/types.rs:554`).
- **Architect edit at review, disclosed:** one sentence corrected. The section
  read *"Every sandboxed process runs as `--user 1000:1000`"* — but `run_as` is
  configurable (`default_sandbox_run_as`, `src/config/types.rs:543`) and the uid
  gate refuses only container **root**, not any non-1000 uid. Now reads
  `--user <run_as>`, `1000:1000` by default. **This was my wording, dictated
  verbatim in § Spec Task 2** — the executor reproduced the spec faithfully. I
  fixed it here rather than bouncing because the defect is architect-side, is a
  single clause, and this phase existed precisely to stop `CLAUDE.md` carrying
  wrong statements about the sandbox.
- **Calibration (1 occurrence):** *when a spec dictates prose verbatim, the
  architect owns its factual accuracy — the executor will reproduce a wrong
  claim exactly as written.* Pre-injection removes the executor's judgement from
  the loop, which is the point, but it also removes the executor as a check.
  Data, not yet a trend.

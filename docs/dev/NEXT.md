# NEXT

**Active phase:** `docs/dev/milestones/M21-host-model/phase-02-host-config.md`
— 02, host-config. **Drafted 2026-09-19**, status `todo`.

Makes `[hosts.<name>]` real config: `src/host.rs` with `HostId`, a `HostEntry`
free-form map on `Config`, `validate_hosts`, runbook `hosts:` frontmatter, and
template documentation. **Pure and additive — nothing executes.** No SSH, no
shell spawning, no ControlMaster; those are rows 03 and 04.

**Two drafting probes decided the shape, run against the real gate rather than
reasoned about:**

1. **The struct name decides whether the doc gate sees the section at all.**
   `tests/doc_truth.rs` discovers config sections by a type-name suffix of
   `Config` or `Entry`. Named `HostSpec` — the name the design and this
   milestone's own README used — the gate reports 14 passed and the section is
   **invisible**: a user-facing config table would ship undocumented with
   nothing complaining. Named `HostEntry`, the gate fails and demands the
   fields be documented. The phase ships `HostEntry` and says why; the README
   is corrected.
2. **Phantom keys under `[hosts.*]` are not checked.** The gate skips any
   section containing a `.` unless it has an explicit special-case, which only
   `models` has. A deliberately invented key under `[hosts.web01]` passed
   unnoticed. The phase adds the `hosts` branch and mutation-proves it with
   that same key.

This is the calibration item from 01b applied: where a criterion's value comes
from a procedure, run the procedure at drafting.

Every acceptance criterion was run against HEAD `6fdfe57d` and fails there.
Baselines measured: `config` 104, `runbook` 36, lib 1898 → 1906, doc_truth 14
unchanged.

Dispatch with `/rexymcp:dispatch phase-02`.

**After 02:** 03 shell-spawn-argv. Ten rows in M21; live host `pinky`;
`max_turns` 1000.

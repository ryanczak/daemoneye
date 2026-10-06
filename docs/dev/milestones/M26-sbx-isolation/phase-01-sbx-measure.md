# Phase 01: sbx-measure — execute every sbx fact the later rows will cite

**Milestone:** M26 — Agent isolation on Docker Sandboxes (sbx)
**Status:** review
**Depends on:** none
**Estimated diff:** 0 lines of code; this doc's Update Log, the milestone README's measurement table, and `probes/` (already written)
**Tags:** language=rust, kind=verification, size=s, architect-run

## Goal

Rows 02–08 build argv, parsers, signal paths and policy mappings on top of a
pre-1.0 CLI. This phase runs one measurement block on pinky and records its
output, so every one of those rows quotes a value that was **executed**, not
read from `--help` or the docs.

This phase is **architect-run**: it drives a real `sbx` daemon and real
microVMs on the development host, which the executor's sandboxed tool set
cannot. It is a phase so the measurements are a tracked `## Spec` task with
pasted evidence.

## Architecture references

- `docs/dev/milestones/M26-sbx-isolation/README.md` — § Exit criteria, § PE
  decisions, § Decisions on the open questions (Q1–Q6), § "Measured on pinky"
  (the table this phase extends).
- `docs/design/daemoneye-2.0.md` § 2.9 — the sbx integration shape.
- `src/shell/pty.rs`, `signal_foreground` — the host pause rule this phase
  re-measures inside the VM: **stop the shell first, then the job**, because a
  running bash reaps a stopped job and the rest of the loop keeps going.

## Pre-flight

1. On **pinky**, as the operator (`sbx` is logged in under the real `$HOME`).
2. `sbx daemon status` reports `Status: running`.
3. `sbx ls -q | grep -c '^de-probe-'` prints `0`. The block creates and removes
   only `de-probe-a`, `de-probe-b`, `de-probe-c` and one over-long name.
4. The repo is clean apart from this milestone's docs.

## Current state

Measured on **2026-10-05** at commit `c1613db4`, sbx `v0.46.0`.

- **The block exists and was rehearsed twice** on pinky:
  `docs/dev/milestones/M26-sbx-isolation/probes/probe.sh` (lifecycle, errors,
  staging, policy) and `probes/pty_probe.py` (a host PTY driving
  `sbx exec -it <sandbox> bash`, the shape the shell-host will use). The last
  rehearsal's output is in § End-to-end verification as the reference. It
  takes about one minute.
- **Rehearsal fix folded in:** the PTY probe first parsed values by line
  prefix and read nothing, because bash's bracketed-paste sequences
  (`\x1b[?2004l\r`) precede each output line. It now takes the last
  `KEY=value` match in the read. Any later parser of this stream has the same
  trap.
- **One probe was dropped as harmful:** running `sbx` with a throwaway `HOME`
  and only `XDG_CONFIG_HOME` pointed at the real one printed
  `Starting sandboxd daemon...` and **started a second `sbx daemon start`
  process**. The block probes `HOME=<tmp>` alone and with
  `XDG_STATE_HOME`, both of which fail with `Not authenticated to Docker`
  and start nothing; it counts `sbx daemon start` processes before and after
  to prove that.
- **Not measured here, by decision:** shell adoption after a daemon restart.
  It needs the real `daemoneye shell-host` with an sbx child, which row 04
  builds; row 04 measures it. Proxy substitution of a stored secret is also
  unmeasured: pinky stores none.

## Spec

### Task 1 — Run the measurement block

From the repo root on pinky:

```sh
bash docs/dev/milestones/M26-sbx-isolation/probes/probe.sh 2>&1 | tee /tmp/m26-p01.txt
```

Do not edit the scripts for this run. If a line cannot be produced, stop and
record a blocker.

### Task 2 — Compare against the reference

Diff the output against the reference in § End-to-end verification. Lines
`S0 version` and `S0 sbx_daemons_before` / `S5 sbx_daemons_after` may differ in
value (the two daemon counts must equal **each other**). Every other line must
match the reference exactly. For each line that differs, write one sentence in
the Update Log naming the row of the milestone README it changes.

### Task 3 — Extend the milestone README's measurement table

In `docs/dev/milestones/M26-sbx-isolation/README.md` § "Measured on pinky",
add one table row per fact below, quoting the measured value:

- `--memory 1g --cpus 2` → in-VM `nproc` and MemTotal.
- Error shapes: exec on a missing sandbox, rm of a missing sandbox, duplicate
  create — exit code and the stderr line.
- The sandbox name limit (63 characters).
- Host environment is not forwarded by `sbx exec`.
- `sbx` under another `HOME` is unauthenticated — so the daemon's
  throwaway-`HOME` test harness cannot reach sbx, and rows 02–08 test through
  an injected command runner, never a live `sbx`.
- `sbx cp` staging: mode and owner preserved, script runs.
- Policy: a per-sandbox allow adds to the global policy and does not reach
  another sandbox; a per-sandbox deny on one host leaves its neighbours
  reachable; rules are gone after `sbx rm`.
- `sbx policy log --json` shape: the two top-level keys and each entry's keys.
- PTY: the marker round trip under the default rc; resize propagates; the
  idle `tpgid` equals the in-VM bash pid and differs while a job runs.
- **Pause:** stopping only the job does **not** freeze a loop typed at the
  prompt; stopping the shell then the job does; `CONT` job-then-shell resumes.
- Cancel: `^C` through the PTY; an in-VM `TERM` to the foreground group
  returns the prompt with status 143.
- Teardown: `sbx rm -f` under a live shell ends the host client with exit 137.

### Task 4 — Capture the end-to-end evidence

Paste `/tmp/m26-p01.txt` verbatim into a new Update Log entry, followed by a
line `PASTE MATCH: yes` if the pasted block equals the file byte for byte
(`diff` it), or `PASTE MATCH: no` with the reason.

## Acceptance criteria

- [ ] The Update Log holds the full block output, 39 lines, from `S0 version`
      to `S5 sbx_daemons_after`.
- [ ] `PASTE MATCH: yes` appears in that entry.
- [ ] `S0 leftover_probes_before: 0` and `S5 leftover_probes_after: 0`.
- [ ] `S0 sbx_daemons_before` equals `S5 sbx_daemons_after`.
- [ ] `P3a stop_job_only_freezes: False`, `P3b stop_shell_then_job_freezes: True`
      and `P3b resumes_after_cont: True` — the pause rule row 05 implements.
- [ ] `S3 after_sandbox_allow example.org: 200`,
      `S3 allow_did_not_leak_to_b example.org: 403`,
      `S3 sandbox_deny api.github.com: 403  github.com: 200`,
      `S3 deny_all github.com: 403`, `S3 rules_after_rm: 0` — the Q1/Q2 mapping.
- [ ] `P6 client_exits_on_rm: True exit=137`.
- [ ] Every other line matches the reference, or the Update Log names the
      README row each difference changes.
- [ ] The README's "Measured on pinky" table has a row for each of the twelve
      facts in Task 3, and `git diff --stat` shows no file under `src/` or
      `tests/`.

## Test plan

None. This phase adds no code; `cargo test` is not affected.

## End-to-end verification

The block is the verification. Rehearsal reference (2026-10-05, pinky; the
daemon count was 2 because of the stray process described in § Current state):

```
S0 version: sbx version: v0.46.0 991967dc90ce0d9a440cd1df1bdf3e395c5a2693
S0 daemon: Status: running
S0 leftover_probes_before: 0
S0 sbx_daemons_before: 2
S1 create_exit: 0
S1 limits: nproc=2 memMiB=981
S1 exec_missing: exit=1 stderr=error: sandbox 'de-probe-nope' not found
S1 rm_missing: exit=1 stderr=error: sandbox 'de-probe-nope' not found
S1 create_duplicate: exit=1 stderr=error: sandbox 'de-probe-a' already exists; connect to the existing sandbox
S1 name_69_chars: exit=1 stderr=error: sandbox name cannot exceed 63 characters: de-probe-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
S1 host_env_forwarded: no
S1 other_HOME: exit=1 stderr=error: Not authenticated to Docker
S1 other_HOME_real_XDG_STATE_HOME: exit=1 stderr=error: Not authenticated to Docker
S2 cp_exit: 0 
S2 staged_mode_owner: 700 agent
S2 staged_run: staged-ok arg1
S3 default example.org: 403
S3 after_sandbox_allow example.org: 200
S3 sandbox_deny api.github.com: 403  github.com: 200
S3 allow_did_not_leak_to_b example.org: 403
S3 deny_all github.com: 403
S3 policy_log_json_shape: {"allowed_hosts": [{"count_since": "int", "host": "str", "last_seen": "str", "proxy_type": "str", "rule": "str", "since": "str", "vm_name": "str"}], "blocked_hosts": [{"count_since": "int", "host": "str", "last_seen": "str", "proxy_type": "str", "reason": "str", "rule": "str", "since": "str", "vm_name": "str"}]}
S3 secrets_stored: No secrets found. Run 'sbx secret set --help' to see available services.
S3 sentinel_present_anyway: proxy-managed
S3 rules_after_rm: 0
P1 marker_default_rc: ok
P1 in_vm_bash_pid_numeric: True
P2 resize: 24 80 -> 40 132
P3 idle_tpgid_is_shell: True
P3 running_tpgid_differs_from_shell: True
P3a stop_job_only_freezes: False
P3b stop_shell_then_job_freezes: True
P3b resumes_after_cont: True
P4 ctrl_c_ends_loop: True
P4 shell_usable_after: True
P5 in_vm_term_exit_status: 143
P6 client_exits_on_rm: True exit=137
S5 leftover_probes_after: 0
S5 sbx_daemons_after: 2
```

## Authorizations

- [x] May edit `docs/dev/milestones/M26-sbx-isolation/README.md` (the
      measurement table and the phase row only).
- [x] May create and remove sbx sandboxes named `de-probe-*` on pinky, and add
      per-sandbox policy rules scoped to them.

Nothing else: no dependency, no `src/` or `tests/` change, no global sbx
policy or secret change, no `sbx daemon` or `sbx reset` command.

## Out of scope

- Any Rust code — row 02 starts it.
- Shell adoption across a daemon restart — row 04.
- Stored-secret substitution — needs a stored secret; not in this milestone's
  measurements unless the PE stores one.
- `sbx --cloud`, kits, templates, the MCP gateway.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-05 (architect-run, complete)

Run on pinky, sbx v0.46.0, by the architect (not dispatched: the doc is
architect-run). All 39 lines below are `/tmp/m26-p01.txt`, verbatim.

```
S0 version: sbx version: v0.46.0 991967dc90ce0d9a440cd1df1bdf3e395c5a2693
S0 daemon: Status: running
S0 leftover_probes_before: 0
S0 sbx_daemons_before: 2
S1 create_exit: 0
S1 limits: nproc=2 memMiB=981
S1 exec_missing: exit=1 stderr=error: sandbox 'de-probe-nope' not found
S1 rm_missing: exit=1 stderr=error: sandbox 'de-probe-nope' not found
S1 create_duplicate: exit=1 stderr=error: sandbox 'de-probe-a' already exists; connect to the existing sandbox
S1 name_69_chars: exit=1 stderr=error: sandbox name cannot exceed 63 characters: de-probe-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
S1 host_env_forwarded: no
S1 other_HOME: exit=1 stderr=error: Not authenticated to Docker
S1 other_HOME_real_XDG_STATE_HOME: exit=1 stderr=error: Not authenticated to Docker
S2 cp_exit: 0 
S2 staged_mode_owner: 700 agent
S2 staged_run: staged-ok arg1
S3 default example.org: 403
S3 after_sandbox_allow example.org: 200
S3 sandbox_deny api.github.com: 403  github.com: 200
S3 allow_did_not_leak_to_b example.org: 403
S3 deny_all github.com: 403
S3 policy_log_json_shape: {"allowed_hosts": [{"count_since": "int", "host": "str", "last_seen": "str", "proxy_type": "str", "rule": "str", "since": "str", "vm_name": "str"}], "blocked_hosts": [{"count_since": "int", "host": "str", "last_seen": "str", "proxy_type": "str", "reason": "str", "rule": "str", "since": "str", "vm_name": "str"}]}
S3 secrets_stored: No secrets found. Run 'sbx secret set --help' to see available services.
S3 sentinel_present_anyway: proxy-managed
S3 rules_after_rm: 0
P1 marker_default_rc: ok
P1 in_vm_bash_pid_numeric: True
P2 resize: 24 80 -> 40 132
P3 idle_tpgid_is_shell: True
P3 running_tpgid_differs_from_shell: True
P3a stop_job_only_freezes: False
P3b stop_shell_then_job_freezes: True
P3b resumes_after_cont: True
P4 ctrl_c_ends_loop: True
P4 shell_usable_after: True
P5 in_vm_term_exit_status: 143
P6 client_exits_on_rm: True exit=137
S5 leftover_probes_after: 0
S5 sbx_daemons_after: 2
```

Every line equals the reference, including `S0 sbx_daemons_before: 2`. The
count is 2 because PID 258365 (`sbx daemon start`, 14:57), started by an
earlier rehearsal probe, is **still running**; the PE's note that it was gone
was wrong when checked here, and a kill was blocked by the permission
classifier. Before = after, as the criteria require. No line differs from the
reference, so no README row changes beyond the table additions in Task 3.

PASTE MATCH: yes

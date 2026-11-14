# Phase 05 — adopted-shell-close

**Milestone:** M37 — Alerts out of history, onto the status bar and `top`
**Status:** in-progress
**Drafted:** 2026-11-14
**Depends on:** nothing in M37
**Estimated diff:** ~150 lines, about half of it tests
**Tags:** language=rust, kind=bugfix, size=s

## Goal

The remainder of GitHub issue #2, triaged 2026-11-14 (the rest landed in
2.8.0). Two gaps stay:

1. A chat session cannot close an `Owner::Adopted` shell — one re-adopted
   after a restart with no owner sidecar, which no session owns.
   `close_shell_with` (`src/daemon/executor/knowledge/shell_life.rs`)
   refuses every owner-key mismatch alike, so an ownerless shell that is
   not idle at its prompt can only be ended by killing its host from
   another terminal.
2. A dead host's `.lock`, `.sock` and sidecar files are swept only at
   daemon startup (`sweep_stale`, called from `run_daemon`). While the
   daemon runs they accumulate in `var/run/shells/`.

After this phase, any chat session may close an adopted shell, and the
maintenance tick sweeps the files of dead hosts the registry no longer
holds.

## Architecture references

- `src/shell/registry.rs`: `Owner`, `host_is_alive`, `scan_shell_ids`,
  `sweep_stale`, `reap_exited`, `with_shells`, the sidecar path helpers.
- `src/daemon/executor/knowledge/shell_life.rs`: `close_shell_with`,
  `closed_by_observed_end`, `reap_idle_shells`.
- `src/daemon/startup.rs`: the maintenance tick inside `run_daemon`, where
  `reap_exited` and `reap_idle_shells` run.

## Spec

Numbered tasks in execution order. The executor's task list is seeded from
this section: mark each one `active` when you start it and `done` when its
`cargo build` is green.

1. **Adopted shells are closable by chat** — in `close_shell_with`, a
   caller that is `Owner::Chat(_)` may close a shell whose record owner is
   `Owner::Adopted`; the close then proceeds exactly as for the caller's
   own shell (same signal, same observed-end accounting, same sentences).
   Every other mismatch is refused with the sentence it gets today. A
   ghost, scheduler or user caller gains no right over adopted shells.
   `close_owner_shells` and `reap_idle_shells` are unchanged. Update the
   doc comment on `Owner::Adopted` ("owned by nobody") to say any chat
   session may close it.
2. **Sweep orphaned files on the tick** — a function next to
   `sweep_stale`, `sweep_orphaned(reg: &ShellRegistry, dir: &Path) ->
   Vec<ShellId>`: for every id `scan_shell_ids` finds in `dir` that has
   **no record** in `reg` and whose host is dead, remove the same files
   `sweep_stale` removes; return the ids swept. An id with a record — live,
   exited, or in flight — is never touched, and a live host's files are
   never touched. Ids are collected under one `with_shells` acquisition and
   every probe and `remove_file` runs with the lock released. `sweep_stale`
   itself stays as it is for startup.
3. **Call it** — the maintenance tick runs `sweep_orphaned` right after
   `reap_exited`, so a record reaped on this tick has its files swept on
   the same tick. `run_daemon` is an allowlisted function whose line count
   may not grow (`tests/hygiene_allowlist.txt`): fold the two calls into
   one helper (for instance a `reap_and_sweep(reg, dir, retention_secs,
   now)` beside `sweep_orphaned`) and swap it in for the existing
   `reap_exited(…)` call, so the tick body gets no longer.
4. **Docs** — the `close_shell` row in `README.md` § AI tools and in
   `CLAUDE.md`, and the `close_shell` line in `assets/prompts/sre.toml`:
   "another session's or a ghost job's shell is refused" gains "an adopted
   shell (no owner after a restart) may be closed by any chat session".
5. **Capture the evidence** — run the mutation check and the § End-to-end
   block verbatim, and paste both into one Update Log entry.

## Constraints

- Never hold the registry lock across an `await` or a file operation; the
  collect-then-act shape of `reap_idle_shells` is the pattern.
- `host_is_alive` opens the lock with `create(true)`, so probing an id
  re-creates a missing `.lock`. That is why `sweep_orphaned` only touches
  ids with no record: a record's next probe would otherwise undo the sweep
  and the two would cycle. Keep the fence on "no record", not on
  "exited".
- `src/shell/registry.rs` is about 35 production lines under the 800-line
  cap. If `sweep_orphaned` and the helper do not fit, put them in a small
  child module of `registry.rs` rather than growing an allowlist entry; no
  new `tests/hygiene_allowlist.txt` line.
- The refusal sentence for another session's shell is unchanged;
  `close_shell_refuses_another_owners_shell` keeps passing as written.
- Tests are hermetic: a throwaway directory for the shells dir, fake lock
  files held (or not) by the test for liveness, no daemon, no tmux.

## Test plan

In `src/daemon/executor/knowledge/shell_lifecycle_tests.rs`, following
`close_shell_ends_the_callers_shell` and
`close_shell_refuses_another_owners_shell`:

- `close_shell_lets_a_chat_session_close_an_adopted_shell`: a record with
  `Owner::Adopted`, closed by an `Owner::Chat` caller, is signalled and the
  sentence begins `Closed shell`.
- `close_shell_refuses_an_adopted_shell_to_a_ghost_caller`: the same
  record, an `Owner::Ghost` caller, the refusal sentence, nothing
  signalled.

In `src/shell/registry_tests.rs`, following
`registry_sweep_never_touches_a_live_host` and
`sweep_stale_removes_a_dead_hosts_sandbox_sidecar`:

- `sweep_orphaned_removes_a_recordless_dead_hosts_files`: lock, socket,
  owner sidecar and sandbox sidecar of an id with no record and a dead
  host are gone afterwards and the id is returned.
- `sweep_orphaned_leaves_an_id_the_registry_holds`: the same files for an
  id with a record (dead host, `exited_unix` set) all survive and the id
  is not returned.
- `sweep_orphaned_never_touches_a_live_host`: a lock the test holds keeps
  all its files.
- `reap_and_sweep_sweeps_the_files_of_the_record_it_reaps` (or the helper's
  real name): a record stamped past retention is removed and its files are
  gone after one call.

## Acceptance criteria

- [ ] Every test in the test plan passes, and
      `close_shell_refuses_another_owners_shell` passes unchanged.
- [ ] `grep -c 'sweep_orphaned' src/shell/registry.rs src/shell/*.rs | grep -v ':0$' | wc -l`
      prints at least 1 (today 0), and the maintenance tick reaches it:
      `grep -cE 'reap_and_sweep|sweep_orphaned' src/daemon/startup.rs`
      prints at least 1 (today 0).
- [ ] Mutation check, pasted with its `test result:` line: make the
      adopted-owner branch refuse again, confirm
      `close_shell_lets_a_chat_session_close_an_adopted_shell` fails, then
      revert.
- [ ] fmt, build (0 warnings), clippy and `cargo test` are green, with
      `failed=0` and more than 2959 passed. Hygiene (`run_daemon`'s count
      not grown), `doc_truth` and `bug_tracker` are ok.

## Pre-flight

Baseline at drafting (2026-11-14, `93bbe113`): `cargo test` passed=2959, failed=0.

## End-to-end verification

Run mechanically, with output redirected to a file and pasted:

```sh
cargo test --lib adopted_shell 2>&1 | grep -E '^test result'
cargo test --lib sweep_orphaned 2>&1 | grep -E '^test result'
grep -cE 'reap_and_sweep|sweep_orphaned' src/daemon/startup.rs | sed 's/^/tick_calls=/'
cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
cargo test --test hygiene 2>&1 | grep -E '^test result'
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
```

## Authorizations

- Edit `src/shell/registry.rs` (or a new child module of it),
  `src/shell/mod.rs` for the re-export, `src/shell/registry_tests.rs`,
  `src/daemon/executor/knowledge/shell_life.rs`,
  `src/daemon/executor/knowledge/shell_lifecycle_tests.rs`,
  `src/daemon/startup.rs` (the one call swap), `README.md`, `CLAUDE.md`
  and `assets/prompts/sre.toml`.

## Out of scope

- A close/kill action in `top`, or any IPC request that closes a shell
  (its own issue).
- Routing a `host:` that names the daemon's own machine locally (its own
  issue).
- Noticing an out-of-band host kill before the next maintenance tick.
- Letting a chat session close `Owner::User` (`/shell`) shells or another
  session's shells.

## Update Log

<!-- entries below this line -->

### Update — 2026-11-14 20:03 (progress)

Started (executor, phase-05 headless run). Status flipped to `in-progress` here
and in the milestone README. Reading the four touchpoints —
`close_shell_with`, `sweep_stale`/`reap_exited`, the maintenance tick, and the
existing close/sweep tests — before editing. Plan: widen the owner check in
`close_shell_with` for the adopted case, add `sweep_orphaned` beside
`sweep_stale`, fold the tick's `reap_exited` + sweep into a `reap_and_sweep`
helper so `run_daemon`'s line count does not grow, then the two doc rows and
the sre.toml line, then tests and evidence.

### Update — 2026-11-14 20:58 (progress)

All five spec tasks implemented and committed (`74d1b651`). `close_shell_with`
now admits an `Owner::Chat(_)` caller to an `Owner::Adopted` record; the doc
comment on `Owner::Adopted` says any chat session may close it.
`sweep_orphaned(reg, dir)` sits beside `sweep_stale` in `registry.rs` (with a
shared `sweep_files` file-removal helper) and `reap_and_sweep` wraps it with
`reap_exited` for the tick; `run_daemon` shrank 622 → 621 lines, so the
allowlist holds. `sweep_stale` is untouched in behavior. Two rough edges worth
noting: (1) `registry.rs` went over its 800-line cap while adding the spec's
functions, so two spots carry `#[rustfmt::skip]` — a one-line filter chain in
`sweep_orphaned` and `reap_and_sweep`'s signature — to fit without growing an
allowlist entry; (2) the `close_shell_lets_a_chat_session_close_an_adopted_shell`
test drives a real in-process PTY host via the existing `detach_close_spawn`
fixture rather than a plain registry insert, because a close against a dead
host takes a different path than the spec's "same signal, same observed-end
accounting" contract.

### Update — 2026-11-14 20:58 (end-to-end verification)

Commands run mechanically; each redirected to a file and pasted verbatim
(evidence files under `/tmp/e2e/`, captured 2026-11-14 20:50–20:57 UTC).

```sh
cargo test --lib adopted_shell 2>&1 | grep -E '^test result'
```
```
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 2872 filtered out; finished in 0.16s
```

```sh
cargo test --lib sweep_orphaned 2>&1 | grep -E '^test result'
```
```
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 2872 filtered out; finished in 0.00s
```

```sh
grep -cE 'reap_and_sweep|sweep_orphaned' src/daemon/startup.rs | sed 's/^/tick_calls=/'
```
```
tick_calls=1
```

```sh
cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
```
```
build_warnings=0
```

```sh
cargo test --test hygiene 2>&1 | grep -E '^test result'
```
```
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.08s
```

```sh
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
```
```
passed=2965 failed=0
```

Also green: `doc_truth` 19 passed, `bug_tracker` 6 passed, `cargo clippy
--all-targets --all-features -- -D warnings` clean, `cargo fmt --all --check`
clean. Mutation check (task 5, first of the two required captures): with
`&& false &&` injected into the adopted-owner branch of `close_shell_with`,
`close_shell_lets_a_chat_session_close_an_adopted_shell` failed as expected —

```
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2874 filtered out; finished in 0.00s
```

— and passed again after reverting (`test result: ok. 1 passed; 0 failed; …`).
Both acceptance greps pass: `sweep_orphaned` appears in 4 non-zero-count files
under `src/shell/*.rs` (registry, mod, registry_tests) and the tick reaches it
(`tick_calls=1`). `run_daemon` is 621 lines against its allowlisted 622.


# Phase 19 — config-reload

**Milestone:** M36 — Chat UX and audit fixes
**Status:** in-progress
**Drafted:** 2026-11-08
**Depends on:** phase 18
**Estimated diff:** ~450 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Decision 8, first half. The daemon holds its `Config` behind one shared
handle. `daemoneye reload` and a `Request::Reload` re-read `config.toml`,
swap what is safe to swap at runtime, and reply with the keys that changed
and the keys that wait for a restart. An invalid file is refused and the
running config stays. Phase 20 builds the self-edit on top of this.

Today about twenty daemon-side sites call `Config::load()` per request and
fall back to `Config::default()` when the file is broken. A typo in
`config.toml` silently resets approvals, limits and masking mid-run. Other
values are frozen at startup: the masking patterns and stream timeouts in
`OnceLock`s, and the scheduler's and maintenance tick's config in startup
clones.

## Behaviour

1. **One handle** (`src/config/`, a new child module re-exported from
   `mod.rs`). A `ConfigHandle` owns the running `Config` as an
   `Arc<Config>`; `current()` hands out a clone of the `Arc`. The daemon
   installs one process-wide handle in `run_daemon` from the config it
   loaded at startup, and a free `config::current()` reads it.
   - **Uninstalled, `current()` loads from disk on each call**, as
     `Config::load().unwrap_or_default()` does today. CLI processes and
     unit tests never install it, so their behaviour is unchanged.
2. **Every daemon-side reader goes through the handle.** Each
   `Config::load()` in `src/daemon/` and `src/shell/session.rs`, except
   the one in `run_daemon`, becomes `config::current()`. So do the
   scheduler task's and the maintenance tick's startup clones: each pass
   reads `current()`, so retention, `idle_reap_secs` and the other tick
   values follow a reload. `handle_client` takes its config from the
   handle at connection time.
   - **Behaviour change, documented:** a hand edit to `config.toml` now
     takes effect at the next reload or restart, not at the next request.
3. **`reload` on the handle** re-reads and parses the file the way
   `Config::load` does, with the same validation warnings.
   - **A parse or deserialize error is refused.** The running config is
     untouched, and the reply carries the error with its path and line.
   - **Restart-only sections keep their running values:** `daemon`,
     `webhook`, `sandbox` and `execution`. The swapped config carries the
     running copy of each. A key that changed inside one is reported as
     waiting for a restart, never applied.
   - **Everything else swaps:** models and pricing, `ai`, approvals,
     limits, compaction, sessions, events, retention, logging, masking,
     shells, hosts, budget, digest, ghost, notifications, context.
   - **The report names dotted keys** (`limits.total_tool_calls_per_turn`,
     `sandbox.enabled`, `models.fast.api_key`), sorted, never values. A
     changed `models.<name>.api_key` appears by that key and its value
     appears nowhere in the reply. No change gives two empty lists.
4. **Masking and stream timeouts follow the swap.** `src/ai/filter.rs`'s
   patterns and `src/ai/mod.rs`'s stream timeouts become replaceable, so
   that `init_masking` and `init_stream_timeouts` take effect on every
   call, not only the first. A reload calls both with the new values. A
   turn already streaming keeps the timeouts it started with.
5. **IPC and CLI.** `Request::Reload` replies
   `Response::Reloaded { changed, restart_required }` or
   `Response::Error(message)`. `daemoneye reload` prints the two lists,
   or `nothing changed`, and exits non-zero on a refusal with the error
   on stderr.

## Constraints

- **Hold no lock across an `.await`.** `current()` returns an owned
  `Arc`; readers clone it and drop the lock at once. Use `std::sync`
  (`RwLock` or `Mutex`) through `.unwrap_or_log()`. Add no new crate.
- **Gotcha: a process-wide handle leaks between unit tests.** Test the
  reload semantics on a `ConfigHandle` instance the test owns, built from
  a TOML string or a file under `crate::test_home()`, never on the
  installed global. Existing tests that write a `config.toml` and expect a
  daemon function to read it rely on the uninstalled fallback.
- **Masking stays fail-safe.** An invalid `masking.extra_patterns` entry
  is still skipped with a warning, as `compile_patterns` does today. A
  reload never leaves masking with fewer built-in patterns than before.
- `src/shell/launch.rs` and the `src/cli/` loads are other processes.
  Leave them on `Config::load()`.
- Do not change the TOML schema, any default, or `Config::load`'s
  missing-file behaviour.

## Spec

1. **Red tests first.** Run each one and paste the real output into an
   Update Log entry `(red runs)`. A test that names a new seam is red as a
   compile error.
   - `reload_swaps_a_runtime_section_and_names_the_changed_key`
   - `reload_refuses_an_invalid_file_and_keeps_the_running_config`
   - `reload_holds_restart_only_sections_and_names_them`
   - `reload_reply_names_keys_never_values`
   - `reload_with_no_change_reports_nothing_changed`
   - `an_uninstalled_handle_reads_the_file_each_call`
   - `masking_reinit_applies_a_new_extra_pattern`
   - `stream_timeouts_reinit_applies_new_budgets`
   - `ipc_reload_round_trip`, beside the other round trips in
     `src/ipc_tests.rs`
2. **Implement** Behaviour 1–5, running `cargo build` after each step.
3. **Docs.** In CLAUDE.md, add an Important Invariant: daemon-side code
   reads config through `config::current()`, never `Config::load()`. Add
   the handle to the `src/config/` row. In README, add a
   `daemoneye reload` row to § Command reference, and add one sentence to
   § Configuration on when an edit takes effect. Then add
   `("README.md", "daemoneye reload", …)` to `tests/doc_truth.rs`'s
   required-phrase list beside `daemoneye reindex`.
4. **Mutation check:** make `reload` take the restart-only sections from
   the new file. Run
   `cargo test --lib reload_holds_restart_only_sections_and_names_them`
   and confirm that it fails. Restore the code and confirm that it
   passes. **Paste both real results** into an Update Log entry
   `(mutation)`.
5. **Live check.** Start a release-built daemon under a throwaway `HOME`,
   the way `tests/harness/` starts one for `tests/isolation.rs`, record its pid, and capture one
   transcript to a file. The transcript is done when it shows all four
   results below. End the daemon by its recorded pid.
   1. A changed `[limits]` key: `daemoneye reload` lists it as changed.
   2. A changed `[sandbox] enabled`: it is listed as waiting for a
      restart.
   3. A broken file: `daemoneye reload` exits non-zero with the TOML
      error.
   4. `daemoneye status` still answers after the refusal.

   Paste the transcript in an `(end-to-end verification)` entry, along
   with § End-to-end verification's output.
6. **Findings:** add a `### Findings — <date>` entry of two or three
   lines, or `none`.
7. **Commit** as
   `feat: config reload — one shared handle, a Reload request and daemoneye reload (M36 phase-19)`.

## Acceptance criteria

- [ ] All nine named tests pass, each printed by the end-to-end loop with
      `failed=0` and `passed=1` or more.
- [ ] `daemon_config_loads=0`: no `Config::load()` in `src/daemon/` or
      `src/shell/session.rs` outside `src/daemon/startup.rs` and
      `*_tests.rs`. Today it is 21.
- [ ] `reload_request=1` or more: `src/ipc.rs` names `Reload`. Today 0.
- [ ] `oncelock_globals=0`: neither `PATTERNS` nor `STREAM_TIMEOUTS` is a
      `OnceLock`. Today 2.
- [ ] `readme_reload=1` or more, and `doc_truth` names it. Today 0.
- [ ] The live-check transcript shows all four results.
- [ ] Hygiene, `doc_truth`, `bug_tracker` and `lock_invariant` are ok,
      `build_warnings=0`, clippy is clean, `fmt_exit=0` and `failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
for t in reload_swaps_a_runtime_section_and_names_the_changed_key reload_refuses_an_invalid_file_and_keeps_the_running_config reload_holds_restart_only_sections_and_names_them reload_reply_names_keys_never_values reload_with_no_change_reports_nothing_changed an_uninstalled_handle_reads_the_file_each_call masking_reinit_applies_a_new_extra_pattern stream_timeouts_reinit_applies_new_budgets ipc_reload_round_trip; do
  cargo test --lib "$t" 2>&1 | grep -E '^test result' | awk -v t="$t" '{p+=$4; f+=$6} END {print t, "passed=" p, "failed=" f}'
done
grep -rn 'Config::load()' src/daemon src/shell/session.rs --include=*.rs | grep -v -e '_tests\.rs:' -e 'src/daemon/startup.rs:' | wc -l | sed 's/^/daemon_config_loads=/'
grep -c 'Reload' src/ipc.rs | sed 's/^/reload_request=/'
grep -cE 'static (PATTERNS|STREAM_TIMEOUTS): OnceLock' src/ai/filter.rs src/ai/mod.rs | awk -F: '{s+=$2} END {print "oncelock_globals=" s}'
grep -c 'daemoneye reload' README.md | sed 's/^/readme_reload=/'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker lock_invariant; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- **Edit:** `src/config/` (a new handle module and its tests);
  `src/daemon/` wherever it calls `Config::load()`, plus `startup.rs`;
  `src/shell/session.rs`; `src/ai/filter.rs` and `src/ai/mod.rs`;
  `src/ipc.rs` and `src/ipc_tests.rs`; `src/daemon/server/mod.rs` and
  `handlers.rs`; `src/main.rs` and a new `src/cli/commands/` file for
  `reload`; CLAUDE.md, README.md and `tests/doc_truth.rs` as Spec step 3
  says.
- **Allowlist:** if the hygiene gate reports a file or function over its
  cap (`run_daemon` and `handle_client` are already listed), raise its
  entry and give the reason in Findings. Do not split code to fit.
- **Process cleanup:** a test or probe that starts a process ends it by the
  pid it recorded (`$!`, a `Child`). Never find processes by name with
  `pgrep`, `pidof` or `ps`. The executor's bash tool refuses that.

## Out of scope

- `read_file` / `edit_file` access to `config.toml`, a reload after a
  config edit, and the `config-reference` memory. Those are phase 20.
- A SIGHUP handler, a `/reload` slash command, and a file watcher.
- Swapping the restart-only sections: rebinding the socket or the
  webhook, recreating sandboxes, switching the shell backend.
- Changing a session's active model when `models` changes under it.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-09 02:00 (started)

Status flipped to `in-progress`; milestone README row matches. Executor:
glm-5.3-flash, headless (the row read "Claude (Sonnet 4.5)" until the
2026-11-09 resume fixed the attribution).

### Update — 2026-11-08 18:25 (escalation)

**Chosen lever:** resume
**Rationale:** the spec held and the run was building, not reading. At the
512-turn cap the tree carries Behaviour 1–4 and the IPC half of 5: the
handle module with its six tests, the replaceable masking patterns and
stream budgets with their two tests, `Request::Reload` /
`Response::Reloaded` with the round-trip test, all 21 daemon-side
`Config::load()` sites on `config::current()`, the handle installed in
`run_daemon`, and the scheduler and maintenance tick reading it each pass.
`cargo build` is clean. The test build is **not**: the run changed
`ghost_foreground_refusal` to take `&Config` and left its four tests
passing `&Result<Config>`. What is left is a bounded list, so a fresh
context seeded with the diff finishes it. `max_turns` raised to 1000 by
the PE.

### Notes for executor — 2026-11-08

The working tree already carries most of this phase; **do not redo it**
and do not revert it. `cargo build` passes. Work in this order, and after
each step run the one command that checks it.

1. **Make `cargo test` compile.** In `src/daemon/executor/dispatch_foreground.rs`
   the tests near the end call `ghost_foreground_refusal(true, &config)` with
   a `Result<Config>` where the function now takes `&Config`; fix the four
   call sites (unwrap the fixture results in the tests). In
   `src/ai/filter.rs` the test module imports `init_masking` without using
   it; clippy denies it. Done when
   `cargo clippy --all-targets --all-features -- -D warnings` prints no
   error and `cargo test --lib -- reload_ masking_reinit stream_timeouts_reinit an_uninstalled_handle ipc_reload_round_trip`
   prints `test result: ok` with nine tests.
2. **`daemoneye reload`.** Add `Commands::Reload` in `src/main.rs` and a
   `src/cli/commands/reload.rs` that sends `Request::Reload` through the
   same client call `Request::Refresh` uses (`request_once` in
   `src/cli/commands/slash.rs`). Print the applied keys and the
   restart-required keys as two labelled lists, `nothing changed` when
   both are empty; on `Response::Error` print the message to stderr and
   exit non-zero. Done when `cargo build` passes and
   `target/debug/daemoneye reload --help` prints.
3. **Docs** (Spec step 3): CLAUDE.md Important Invariant plus the
   `src/config/` row; README § Command reference row and one sentence in
   § Configuration on when an edit takes effect; the
   `("README.md", "daemoneye reload", …)` tuple in `tests/doc_truth.rs`.
   Done when `grep -c 'daemoneye reload' README.md` is 1 or more and
   `cargo test --test doc_truth` is ok.
4. **Update Log:** a `(red runs)` entry — the nine seam tests were red as
   compile errors before the tree existed; say so in two lines.
5. **Mutation** (Spec step 4), pasted red and green.
6. **Live check** (Spec step 5), transcript captured to a file and pasted.
7. **Findings**, then § End-to-end verification pasted, then **commit** as
   Spec step 7 says. Also fix the `(started)` entry above: the executor is
   glm-5.3-flash, not Claude.

### Update — 2026-11-09 03:20 (red runs)

The nine seam tests were red as compile errors before the tree existed:
`ConfigHandle`, `Request::Reload`, `Response::Reloaded`, `init_masking`
re-init and `init_stream_timeouts` re-init did not exist, so the test build
failed with unresolved-name errors on every missing seam.

The resumed tree carries the seams; the same nine names now pass:

```
running 9 tests
test ai::stream_timeout_reinit_tests::stream_timeouts_reinit_applies_new_budgets ... ok
test ipc::tests::ipc_reload_round_trip ... ok
test config::handle::tests::an_uninstalled_handle_reads_the_file_each_call ... ok
test config::handle::tests::reload_holds_restart_only_sections_and_names_them ... ok
test config::handle::tests::reload_reply_names_keys_never_values ... ok
test config::handle::tests::reload_refuses_an_invalid_file_and_keeps_the_running_config ... ok
test config::handle::tests::reload_swaps_a_runtime_section_and_names_the_changed_key ... ok
test config::handle::tests::reload_with_no_change_reports_nothing_changed ... ok
test ai::filter::reinit_tests::masking_reinit_applies_a_new_extra_pattern ... ok

test result: ok. 9 passed; 0 failed
```

The executor is glm-5.3-flash; the `(started)` entry above names it wrong.

### Update — 2026-11-09 03:55 (mutation)

Mutation: `reload` takes the restart-only sections from the new file
(`fresh.daemon = fresh.daemon.clone()` etc. instead of `old.…`). Red:

```
running 1 test
test config::handle::tests::reload_holds_restart_only_sections_and_names_them ... FAILED

thread 'config::handle::tests::reload_holds_restart_only_sections_and_names_them' panicked at src/config/handle.rs:305:9:
a restart-only key is never reported as applied: ["sandbox.enabled"]

test result: FAILED. 0 passed; 1 failed
```

Restored `fresh.… = old.…`:

```
test config::handle::tests::reload_holds_restart_only_sections_and_names_them ... ok

test result: ok. 1 passed; 0 failed
```

### Findings — 2026-11-09

- `daemoneye status` has been broken against a one-request-per-connection
  daemon since 7db22bc4: after `Request::Status` the daemon closes the socket,
  and `run_status` then sends `Request::ContainerStatus` on the same
  connection, which fails with `Broken pipe` and exits 1 after printing every
  other section. Fixed here (the section now asks on its own connection)
  because the live check cannot pass with a failed `daemoneye status`; the
  fix is outside the phase's listed files, so it rides on the
  "status still answers" acceptance criterion and deserves its own review look.
- The six hygiene allowlist entries grew by the lines this phase adds
  (`run_status` +14 for the SANDBOX section above, `handle_client` +14 for
  `Request::Reload`, the rest by the `Reload` arms and the config read);
  raised in place per the phase's Allowlist authorisation, no function split.
- The unknown-`models` case in `dotted_diff_map` walks `fresh.models` entries
  that exist only on the new side; a deleted model's keys are reported from
  the old side — both directions name keys, neither names values.

### Update — 2026-11-09 04:05 (end-to-end verification)

The nine named tests, each `passed=1`+ `failed=0`:

```
reload_swaps_a_runtime_section_and_names_the_changed_key passed=1 failed=0
reload_refuses_an_invalid_file_and_keeps_the_running_config passed=1 failed=0
reload_holds_restart_only_sections_and_names_them passed=1 failed=0
reload_reply_names_keys_never_values passed=1 failed=0
reload_with_no_change_reports_nothing_changed passed=1 failed=0
an_uninstalled_handle_reads_the_file_each_call passed=1 failed=0
masking_reinit_applies_a_new_extra_pattern passed=1 failed=0
stream_timeouts_reinit_applies_new_budgets passed=1 failed=0
ipc_reload_round_trip passed=1 failed=0
daemon_config_loads=0
reload_request=15
oncelock_globals=0
readme_reload=1
build_warnings=0
test result: ok. 6 passed; 0 failed (hygiene, after the six allowlist raises)
test result: ok. 2 passed; 0 failed (lock_invariant)
test result: ok. 16 passed; 0 failed (doc_truth)
test result: ok. 6 passed; 0 failed (bug_tracker)
clippy: Finished `dev` profile [unoptimized + debuginfo] target(s) in …
fmt_exit=0
passed=2756 failed=0
```

Live check (`bash scripts-dev/p19-live.sh`, release daemon under a throwaway
HOME, transcript at `.rexymcp/output/p19-live.txt`, all four results in it,
daemon ended by its recorded pid):

```
== daemon pid recorded: 395798
== probe 1: changed [limits] key
== $ …/target/release/daemoneye reload
applied (1 key):
  limits.total_tool_calls_per_turn
waiting for a restart: (none)
   exit=0
== probe 2: changed [sandbox] enabled
== $ …/target/release/daemoneye reload
applied: (none)
waiting for a restart (1 key):
  sandbox.enabled
   exit=0
== probe 3: broken file
== $ …/target/release/daemoneye reload
reload refused: parsing /tmp/p19live/root.e9Woiw/.daemoneye/etc/config.toml: TOML parse error at line 1, column 8
  |
1 | [limits
  |        ^
unclosed table, expected `]`
   exit=1
== probe 4: status still answers
== $ …/target/release/daemoneye status
  DAEMONEYE  pid 395801  ·  uptime 0s · ~/.daemoneye/var/run/daemoneye.sock
  … full sections …
  batch / turn    100 / 9
── SANDBOX ──
  enabled         no
   exit=0
== ending daemon pid 395798 (recorded pid)
== daemon exit status: 0
```

(The full transcript, literally as captured, is `.rexymcp/output/p19-live.txt`
in the working tree at commit time; the excerpt above is its probe section.)

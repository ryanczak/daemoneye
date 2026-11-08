# Phase 17b — owner-sidecar

**Milestone:** M36 — Chat UX and audit fixes
**Status:** todo
**Drafted:** 2026-11-07
**Depends on:** phase 17a
**Estimated diff:** ~250 lines
**Tags:** language=rust, kind=bug, size=s

## Goal

This phase fixes B11's second half, `FINDINGS.md`, major. Phase 17 was
split on 2026-11-07: 17a closed a chat session's shells on detach, and 17c
reaps adopted shells that sit idle.

After a daemon restart, `adopt_at_startup` (`src/shell/registry.rs`)
re-registers every surviving shell host with `owner: Owner::Adopted` and
`host: HostId::Local`, because nothing on disk records either. An adopted
shell can be driven, reused or closed by no session, so:

- a chat session that resumes after the restart has lost its shells;
- the user's own `/shell` shells (`Owner::User`) are lost the same way;
- an ssh shell comes back labelled `local`.

This phase records each shell's owner and host in a sidecar file when the
shell is spawned, and adoption reads it back.

## Behaviour

1. **The owner sidecar.** It is `<id>.owner`, in the directory
   `socket_path` and `sandbox_sidecar_path` use: the shells dir. It is a
   text file of two lines: `Owner::key()`, then `HostId::as_str()`.
   For example:

   ```
   chat:2f3c9a
   zoidberg
   ```

   It sits beside the existing `<id>.sandbox` sidecar. Copy that file's
   shape: an `owner_sidecar_path`, a `write_owner_sidecar` and a
   `read_owner_sidecar` returning an enum with `Absent`, `Invalid` and a
   parsed `(Owner, HostId)`.
2. **`Owner::from_key`** is the inverse of `Owner::key()` for the four
   kinds a spawn can have: `chat:<id>`, `ghost:<id>`, `scheduler:<id>` and
   `user`. Anything else is `None`:
   - an unknown kind (`robot:x`);
   - an empty id (`chat:`);
   - no colon at all (`chat`);
   - `adopted`, which a spawn never writes.

   A host that `HostId::parse` rejects makes the sidecar `Invalid`.
3. **Written at spawn.** `spawn_for_placed`, the one path every
   `spawn_for*` takes, writes the sidecar after the spawn succeeds. The
   write is best-effort. A failure logs `warn` and does not fail the spawn,
   because the shell is live and only its recovery after a restart is lost.
4. **Read at adoption.** `adopt_at_startup` reads each adopted id's owner
   sidecar, with file I/O outside `with_shells` as it does for the
   sandbox sidecar:
   - **Parsed:** the record gets that owner and that host.
   - **Absent**, from a shell spawned before this phase: `Owner::Adopted`
     and `HostId::Local`, as today.
   - **Invalid:** the same, plus one `warn` naming the id.
5. **Swept with the host.** `sweep_stale` removes a dead host's
   `<id>.owner` along with its socket, lock and sandbox sidecar.

## Constraints

- **Do not change `ShellRecord`'s fields.** The owner and host already
  have fields; this phase only fills them correctly at adoption.
- **Tests must not touch the real `~/.daemoneye`.** The sidecar write
  lands in the shells dir, so every test that spawns through `spawn_for*`
  must hold a `crate::test_home()` guard.
  - **Gotcha:** several existing tests in `registry_tests.rs` do not hold
    one today, including `spawn_for_on_host_records_the_named_host`,
    `spawn_for_in_sandbox_records_the_sandbox`,
    `spawn_for_on_host_records_no_sandbox` and
    `spawn_for_still_records_the_local_host`. Unguarded, they would write
    `s1.owner` into the real shells dir, over a live daemon shell's
    sidecar.
  - Find every such test in the crate with `grep`, and add the guard to
    each. That is the **only** change permitted to existing tests.
- Ownership checks (`Owner::may_drive`, `close_shell_with`) are unchanged.
  A re-adopted `Chat` shell is driven exactly like a shell its session
  spawned.

## Spec

1. **Red tests first.** Write the tests below in `registry_tests.rs`, run
   them, and paste the real output into an Update Log entry `(red runs)`.
   A test naming a new function is red as a compile error. The existing
   `sandbox_sidecar_*` and `adopt_at_startup_*` tests are the models.
   - `owner_sidecar_round_trips_every_owner_kind`: `Chat`, `Ghost`,
     `Scheduler` and `User`, each with `HostId::Local` and with a named
     host.
   - `owner_from_key_rejects_malformed_keys`: each of the four negative
     cases in Behaviour 2 is `None`.
   - `adopt_at_startup_restores_owner_and_host_from_the_sidecar`: a live
     host with a sidecar of `chat:s-abc` and `zoidberg` is adopted as
     `Owner::Chat("s-abc")` on `HostId::Named("zoidberg")`.
   - `adopt_at_startup_without_an_owner_sidecar_stays_adopted_and_local`:
     a preservation test for today's behaviour.
   - `adopt_at_startup_treats_an_invalid_owner_sidecar_as_adopted`: a
     sidecar of `robot:x` and `local` is adopted as `Owner::Adopted`.
   - `spawn_writes_the_owner_sidecar`: a `spawn_for_on_host` with an
     in-process spawn, under `test_home`, leaves `<id>.owner` holding the
     owner key and host.
   - `sweep_stale_removes_a_dead_hosts_owner_sidecar`.
2. **Implement** Behaviour 1–5, running `cargo build` after each step.
3. **Mutation check:** make `adopt_at_startup` ignore the sidecar (always
   `Owner::Adopted`), and run
   `cargo test --lib adopt_at_startup_restores_owner_and_host_from_the_sidecar`.
   Confirm that it fails, restore the read, and confirm that it passes.
   Paste both real results.
4. **Findings:** add a `### Findings — <date>` entry of two or three lines,
   or `none`.
5. **End-to-end:** run § End-to-end verification once, and paste its
   output.
6. **Commit** as
   `fix: a shell's owner and host survive a daemon restart through an owner sidecar (M36 phase-17b)`.

## Acceptance criteria

- [ ] All seven named tests pass, each printed by the end-to-end loop with
      `failed=0` and `passed=1` or more.
- [ ] `owner_sidecar=1` or more: `src/shell/registry.rs` names
      `owner_sidecar_path`. Today it is 0.
- [ ] `from_key=1` or more: `src/shell/registry.rs` defines
      `fn from_key`. Today it is 0.
- [ ] Existing tests pass. The only change permitted to them is an added
      `test_home` guard.
- [ ] `real_owner_files=0`: no `.owner` file exists in the real
      `~/.daemoneye/var/run/shells` after the full test run.
- [ ] Hygiene, `doc_truth` and `bug_tracker` are ok, `build_warnings=0`,
      clippy is clean, `fmt_exit=0` and `failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
for t in owner_sidecar_round_trips_every_owner_kind owner_from_key_rejects_malformed_keys adopt_at_startup_restores_owner_and_host_from_the_sidecar adopt_at_startup_without_an_owner_sidecar_stays_adopted_and_local adopt_at_startup_treats_an_invalid_owner_sidecar_as_adopted spawn_writes_the_owner_sidecar sweep_stale_removes_a_dead_hosts_owner_sidecar; do
  cargo test --lib "$t" 2>&1 | grep -E '^test result' | awk -v t="$t" '{p+=$4; f+=$6} END {print t, "passed=" p, "failed=" f}'
done
grep -c 'owner_sidecar_path' src/shell/registry.rs | sed 's/^/owner_sidecar=/'
grep -c 'fn from_key' src/shell/registry.rs | sed 's/^/from_key=/'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
ls ~/.daemoneye/var/run/shells/*.owner 2>/dev/null | wc -l | sed 's/^/real_owner_files=/'
E2E
```

## Authorizations

- **Edit:**
  - `src/shell/registry.rs` and `registry_tests.rs`;
  - existing tests anywhere that spawn through `spawn_for*`, to add a
    `test_home` guard only;
  - `src/shell/mod.rs`, for re-exports only;
  - the `Owner::Adopted` doc comment, which today says nothing records
    the owner: update it.
- **Allowlist:** if the hygiene gate reports a file or function over its
  cap, add or raise its entry and give the reason in Findings. Do not split
  code to fit.

## Out of scope

- Reaping idle adopted shells: phase 17c.
- Handing an `Owner::Adopted` shell to any session.
- Rewriting sidecars for shells already running before this phase.

## Update Log

<!-- entries appended below this line -->

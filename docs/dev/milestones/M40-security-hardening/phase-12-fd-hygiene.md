# Phase 12 — fd-hygiene

**Milestone:** M40 — Security hardening
**Status:** in-progress
**Drafted:** 2026-11-26
**Depends on:** 11 (done)
**Estimated diff:** ~120 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Close audit rows **S16** and **S18**. The architect probed both, and S17,
on 2026-11-26.

1. **S16: the readiness pipe leaks into child processes.**
   - `create_pipe` (`src/daemon/ready.rs`) calls `libc::pipe`, so neither
     end is close-on-exec.
   - The forked daemon keeps the write end as its reporter until
     `report_ready`. Before then it runs subprocesses: tmux
     `session-exists` and `client_dimensions`, and the sbx startup sweep.
   - Each of those inherits the write end, and so does anything
     long-lived they start. If the daemon then dies before reporting, the
     waiting parent `daemoneye daemon` sees no EOF while any such
     descendant lives.
2. **S18: a large passwd entry loses the user name.**
   - `passwd_name` (`src/daemon/executor/shell_label.rs`) calls
     `getpwuid_r` once, with a fixed 16 KiB buffer.
   - **Probed on this host:** an 8-byte buffer returns `ERANGE` (rc 34),
     and 16 KiB succeeds.
   - So an entry larger than 16 KiB (a long LDAP GECOS, for example)
     gives `None`. The label then falls back to `$USER` or the numeric
     uid, though the real name was available.

**S17 is rejected, not in this phase.** The audit said `dup2` plus an
unconditional close loses fds 0–2 when they were closed at launch. The
probe launched a real daemon in a throwaway HOME with fd 0, fd 1, fd 2,
and all three closed. Every time, the running daemon held 0 →
`/dev/null` and 1, 2 → `daemon.log`. The Rust runtime reopens a closed
fd 0–2 on `/dev/null` before `main`, so the precondition never holds.

## Statement of work

1. **Close-on-exec pipe (S16).** `create_pipe` creates both ends with
   `O_CLOEXEC`, using `libc::pipe2(fds, libc::O_CLOEXEC)`. The fork and
   handshake are unchanged: close-on-exec survives `fork` and only acts
   on `exec`.
2. **Retry on `ERANGE` (S18).**
   - `passwd_name` retries `getpwuid_r` on `ERANGE`, doubling the buffer
     each time, up to a cap of 1 MiB. At the cap it gives `None`, as
     today.
   - Start from `sysconf(_SC_GETPW_R_SIZE_MAX)` when it is positive,
     else 16 KiB.
   - Expose the starting size through a seam the test can drive, such as
     a private `passwd_name_from(uid, initial_len)` that `passwd_name`
     calls. Keep the existing `None` cases: no entry, empty name, and a
     name that is not UTF-8.

**Closed file list:** `src/daemon/ready.rs` and
`src/daemon/executor/shell_label.rs`, each with its own tests.

**Edit order:** each step ends with `cargo build --tests`.
1. **Your first tool call is an edit adding the red test**
   `the_readiness_pipe_is_close_on_exec` to `src/daemon/ready.rs`'s
   tests.
2. The `pipe2` fix.
3. The S18 test, then the retry.

**Read limit:** read at most **2** files before your first edit.

**Budget:** you have **1200 turns**; use them efficiently.

## First red test

`the_readiness_pipe_is_close_on_exec`, in `ready.rs`.

- **Setup:** call `create_pipe()`.
- **Asserts:** for both ends,
  `libc::fcntl(fd, libc::F_GETFD) & libc::FD_CLOEXEC != 0`.
- **Today:** red.

Paste the red run.

## Acceptance criteria

- [ ] **S16:** `the_readiness_pipe_is_close_on_exec` passes. Paste its
      red run. The existing `ready.rs` tests still pass.
- [ ] **S18:** `a_tiny_passwd_buffer_still_finds_the_name` passes.
      - **Setup:** the seam with an initial length of **8** bytes, for
        the test process's own euid (`libc::geteuid()`).
      - **Asserts:** the result is `Some` and equals what `passwd_name`
        returns with the default size.
      - **Today:** an 8-byte buffer is `ERANGE`, so without the retry it
        is `None`.
- [ ] **No stale call:** this prints nothing:
      `grep -n 'libc::pipe(' src/daemon/ready.rs`
- [ ] **Mutation run, pasted as output:**
      1. Change `O_CLOEXEC` to `0` in `create_pipe`.
      2. Run `cargo test --lib the_readiness_pipe_is_close_on_exec` once.
         It must fail.
      3. Restore by undoing that one edit, never by copying a backup
         file.
- [ ] **No test removed:** run this once. It prints **0**:
      `git diff f4e6da6a -- src tests | grep -cE '^-\s*#\[(tokio::)?test\]'`
- [ ] **Gates:** all four pass, each run once.

## Spec

1. **Break down:** write the first red test and run it red. Then write a
   numbered task list into an Update Log entry, one line per task, each
   naming the acceptance item it closes. Done when that entry is
   written.
2. **Implement:** follow the edit order, writing each red test before its
   fix. Done when `cargo build --tests` is green.
3. **Meet the acceptance criteria:** check off every item, running each
   command **once**. Done when every item is checked with its evidence
   named.
4. **Capture the evidence:** run the four gates, once each. Paste into a
   new Update Log entry:
   - each `test result:` line, without the per-test `… ok` lines;
   - the red run and the mutation run, **as output, not summarized**;
   - the grep.

   Then commit as one `fix:` commit.

## Authorizations

- You may touch the two files in the closed file list, and no others.
- The `unsafe` FFI calls (`pipe2`, `fcntl`, `getpwuid_r`, `sysconf`)
  each carry a `// SAFETY:` comment, as the existing ones do.

## Out of scope

- **S17** (rejected above). Do not touch `main.rs`, `startup.rs` or
  `log_rotation.rs`.
- Close-on-exec for any other descriptor.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-26 19:16

**Started.** First red test `the_readiness_pipe_is_close_on_exec` written to
`src/daemon/ready.rs` and run red before any fix (pasted under Spec task 1
evidence below).

Task list (one line per task, naming the acceptance item it closes):

1. Write red test `the_readiness_pipe_is_close_on_exec` in `ready.rs`, run it
   red — closes S16 test half of acceptance item 1.
2. Fix `create_pipe` to `libc::pipe2(fds, libc::O_CLOEXEC)` — closes S16,
   acceptance item 1.
3. Write red test `a_tiny_passwd_buffer_still_finds_the_name` in
   `shell_label.rs` driving the `passwd_name_from(uid, 8)` seam — closes S18
   test half of acceptance item 2.
4. Add the `ERANGE` retry loop (start from `sysconf(_SC_GETPW_R_SIZE_MAX)`
   when positive, else 16 KiB; double to a 1 MiB cap) behind
   `passwd_name_from` — closes S18, acceptance item 2.
5. `grep -n 'libc::pipe(' src/daemon/ready.rs` prints nothing — acceptance
   item 3.
6. Mutation run: flip `O_CLOEXEC` to `0`, run the S16 test once (must fail),
   restore — acceptance item 4.
7. `git diff f4e6da6a -- src tests | grep -cE '^-\s*#\[(tokio::)?test\]'`
   prints 0 — acceptance item 5.
8. Gates: `cargo fmt --all`, `cargo build`, `cargo clippy --all-targets
   --all-features -- -D warnings`, `cargo test` — acceptance item 6.
9. Evidence Update Log entry + one `fix:` commit — Spec task 4.

### Update — 2026-11-26 19:24 (end-to-end verification)

**Red run** (before the `pipe2` fix; `libc::fcntl` finds no `FD_CLOEXEC`):

```
$ cargo test --lib the_readiness_pipe_is_close_on_exec

failures:

---- daemon::ready::tests::the_readiness_pipe_is_close_on_exec stdout ----

thread 'daemon::ready::tests::the_readiness_pipe_is_close_on_exec' (883421) panicked at src/daemon/ready.rs:217:13:
read end (fd 3) lacks FD_CLOEXEC: subprocesses would inherit it
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
 (x2)
failures:
    daemon::ready::tests::the_readiness_pipe_is_close_on_exec

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 3216 filtered out; finished in 0.00s

error: test failed, to rerun pass `--lib`
```

**Mutation run** (`O_CLOEXEC` → `0` in `create_pipe`, test run once, then the
one edit undone):

```
$ cargo test --lib the_readiness_pipe_is_close_on_exec

thread 'daemon::ready::tests::the_readiness_pipe_is_close_on_exec' (888735) panicked at src/daemon/ready.rs:217:13:
read end (fd 3) lacks FD_CLOEXEC: subprocesses would inherit it
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
 (x2)
failures:
    daemon::ready::tests::the_readiness_pipe_is_close_on_exec

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 3217 filtered out; finished in 0.00s

error: test failed, to rerun pass `--lib`
```

**Stale-call grep** (acceptance item 3 — prints nothing):

```
$ grep -n 'libc::pipe(' src/daemon/ready.rs
$ echo $?
1
```

**No-test-removed grep** (acceptance item 5 — prints 0):

```
$ git diff f4e6da6a -- src tests | grep -cE '^-\s*#\[(tokio::)?test\]'
0
```

**Gates** (each run once; `cargo fmt --all` ran immediately before staging):

```
$ cargo fmt --all && echo "fmt: OK"
fmt: OK

$ cargo build
   Compiling daemoneye v2.11.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 8.79s

$ cargo clippy --all-targets --all-features -- -D warnings
    Checking daemoneye v2.11.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 14.39s

$ cargo test --lib
test result: ok. 3208 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 149.81s

$ cargo test
test result: ok. 13 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.40s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

**The two phase tests, both green after the fix:**

```
$ cargo test --lib -- the_readiness_pipe_is_close_on_exec a_tiny_passwd_buffer_still_finds_the_name

running 2 tests
test daemon::ready::tests::the_readiness_pipe_is_close_on_exec ... ok
test daemon::executor::shell_label::tests::a_tiny_passwd_buffer_still_finds_the_name ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 3216 filtered out; finished in 0.00s
```

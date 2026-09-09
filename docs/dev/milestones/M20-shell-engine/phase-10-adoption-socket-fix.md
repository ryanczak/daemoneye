# Phase 10: adopt real sockets — the restart-survival close-blocker

**Milestone:** M20 — Shell Engine
**Status:** in-progress
**Depends on:** phase-06b (`scan_shell_ids`, `adopt_at_startup`, `sweep_stale`) —
done, but defective in production (bug-restart-adoption).
**Estimated diff:** ~60 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Make the daemon re-adopt a shell that survived a restart. Today it cannot:
`scan_shell_ids` filters directory entries on `file_type.is_file()`, and a real
`<id>.sock` is an **AF_UNIX socket**, for which `is_file()` is false. So
`scan_shell_ids` returns nothing in production, `adopt_at_startup` adopts
nothing, and `sweep_stale` sweeps nothing — all three inert against real
shells, while every unit test passes because its fixtures are regular files.

This is the milestone's restart-survival exit criterion (the PE's hard
requirement) failing, found by the architect-run live sweep. See
`bugs/bug-10-1.md`.

## Architecture references

- `docs/dev/milestones/M20-shell-engine/README.md` § Exit criteria — "re-listed
  under the same id" is the adoption step this fixes.
- `bugs/bug-10-1.md` — the measured failure and the DoD.

## Pre-flight

- `git rev-parse --abbrev-ref HEAD` → `master`, tree clean.
- `cargo test --lib shell::registry::` → **24 passed** before you start.

## Current state

`shell::registry` **24** tests. `grep -c 'is_file()' src/shell/registry.rs` →
**1** (the defective filter). Full lib suite: **1663 passed; 0 failed; 4
ignored**.

## Measured facts — executed 2026-09-08, not reasoned about

**F1 — proven against the real crate code.** A throwaway test (compiled, run,
reverted) called `scan_shell_ids` on two directories:

```
PROBE real_socket_ids=[]         # UnixListener::bind(dir/"s0.sock")  -> found NOTHING
PROBE regular_file_ids=["s0"]    # fs::write(dir/"s0.sock", b"")      -> found s0
```

The regular-file case is what every existing test uses (`write(dir.path(),
"s0.sock")`), which is why the suite is green while production is broken.

**F2 — the live socket's type.** Measured on a running shell host:

```
s0.sock: socket=True  regular_file=False
s0.lock: socket=False regular_file=True
```

**F3 — the fix surface is one filter.** `src/shell/registry.rs:242`:

```rust
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {   // rejects the socket
            continue;
        }
```

`std::os::unix::fs::FileTypeExt::is_socket()` distinguishes a socket; the name
grammar (`.sock` suffix + `ShellId::parse`, already applied just below) is the
real gate.

## Spec

### Task 1 — `src/shell/registry.rs`: `scan_shell_ids` must accept sockets

Change the filter so a `<id>.sock` entry is found whether it is a real socket
(production) or a regular file (test fixtures). The `.sock` suffix and
`ShellId::parse` that follow are the identity gate; the type check must only
exclude entries that could never be a shell socket — a **directory** named
`s0.sock`, most obviously. Two acceptable shapes:

- accept `file_type.is_file() || file_type.is_socket()` (add
  `use std::os::unix::fs::FileTypeExt;`), or
- exclude only directories (`if file_type.is_dir() { continue; }`).

Either is fine; do **not** drop the entry-type check entirely (a directory
whose name ends in `.sock` must still be skipped — the existing negative test
`…make_dir(dir, "s3.sock")` pins this).

**Do not change** `adopt_at_startup`, `sweep_stale`, `parse_cast_name`, or any
other function — the single filter is the whole fix, and both dependents route
through `scan_shell_ids`.

## Acceptance criteria

Each was run against the current tree and returns the "now" value shown.

- [ ] **A real socket is found.** A new test binds
      `std::os::unix::net::UnixListener` at `<dir>/s0.sock` (a real socket, not
      `fs::write`) and asserts `scan_shell_ids(dir)` returns exactly `[s0]`.
      `grep -c 'UnixListener::bind' src/shell/registry.rs` → **1 or more**
      (now `0`). This test must be **red against the current `is_file()`
      filter** — confirmed at drafting: `scan_shell_ids` on a real socket
      returns `[]` (F1). Paste the red and green `test result:` lines and the
      mutation (revert the filter → red).
- [ ] **A directory named `s0.sock` is still excluded** — the existing
      negative case stays green: `grep -c 'make_dir' src/shell/registry.rs` →
      **3 or more** (now `3`, preservation).
- [ ] `grep -c 'is_file()' src/shell/registry.rs` → **0** (now `1`) **or**
      `grep -c 'is_socket()' src/shell/registry.rs` → **1 or more** (now `0`)
      — whichever shape you choose; state which in the Update Log.
- [ ] `cargo test --lib shell::registry::` reports **25 or more** passing,
      `0 failed` (now `24`).
- [ ] No `unsafe`/`unwrap`/`expect`/`panic!` outside test code in the edited
      file:
      `awk '/^#\[cfg\(test\)\]/{exit} {print}' src/shell/registry.rs | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe'`
      → **0** (now `0`, preservation).
- [ ] Unchanged: `shell::session::` **7**, `daemon::executor::shell_exec::`
      **10**, `shell::launch::` **8**, `shell::host::` **8**, and the full lib
      suite `0 failed` on **three consecutive runs**.
- [ ] All four gates pass.

## Test plan

Names pinned; placement is not.

- `scan_shell_ids_finds_a_real_socket` — the F1 regression guard. Bind a real
  `UnixListener` at `<dir>/s0.sock`; assert `scan_shell_ids(dir) == [s0]`. Red
  against `is_file()`, green after the fix. **Do not** use `fs::write` — that is
  the fixture that hid the bug.
- `scan_shell_ids_still_skips_a_directory_named_like_a_socket` — keep the
  negative case: a directory `s0.sock` is not returned.
- `adopt_at_startup_adopts_a_real_socket_host` — end to end at the registry
  layer: a held `InstanceLock` at `<dir>/s0.lock` **and** a real bound socket
  at `<dir>/s0.sock` → `adopt_at_startup` returns `[s0]` with the record
  present as `Owner::Adopted`. (The existing adoption tests use regular-file
  sockets; this one uses a real socket so the whole adoption path is covered
  against the production file type.)

## End-to-end verification

Run this block verbatim from the repo root. It writes `/tmp/e2e-10.txt`.

```sh
{
echo "== A. build =="; cargo build 2>&1 | tail -2; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== B. named tests =="
cargo test --lib 2>&1 | grep -E "^test .*(scan_shell_ids_finds_a_real_socket|scan_shell_ids_still_skips_a_directory|adopt_at_startup_adopts_a_real_socket).* ok$" | sed 's/^test //' | sort
echo "cargo_exit=${PIPESTATUS[0]}"
echo "== C. registry total (>=25) =="
cargo test --lib shell::registry:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== D. untouched (session 7, shell_exec 10, launch 8, host 8) =="
for m in shell::session daemon::executor::shell_exec shell::launch shell::host; do
  printf '%-30s ' "$m"; cargo test --lib $m:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
done
echo "== E. lib suite, three consecutive runs =="
for r in 1 2 3; do cargo test --lib 2>&1 | grep -E "^test result:" | head -1; done
echo "== F. structural greps =="
echo -n "UnixListener::bind in tests (>=1): "; grep -c 'UnixListener::bind' src/shell/registry.rs
echo -n "make_dir negative kept    (>=3): "; grep -c 'make_dir' src/shell/registry.rs
echo -n "is_file removed OR is_socket added: "; echo "is_file=$(grep -c 'is_file()' src/shell/registry.rs) is_socket=$(grep -c 'is_socket()' src/shell/registry.rs)"
echo -n "no unwrap/expect/panic/unsafe pre-test (0): "
awk '/^#\[cfg\(test\)\]/{exit} {print}' src/shell/registry.rs | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe'
} > /tmp/e2e-10.txt 2>&1
cat /tmp/e2e-10.txt
```

Paste `/tmp/e2e-10.txt` into your Update Log under the exact header
`### Update — YYYY-MM-DD HH:MM (end-to-end verification)`, then run the
self-check and paste its verdict:

```sh
D=docs/dev/milestones/M20-shell-engine/phase-10-adoption-socket-fix.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-10.txt
diff /tmp/pasted-10.txt /tmp/e2e-10.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

**Mutation-check the guard:** revert the filter to `if !file_type.is_file()`,
confirm `scan_shell_ids_finds_a_real_socket` goes red, restore, confirm green.
Paste both `test result:` lines. Verified independently at review.

## Authorizations

You may edit, and nothing else:

- `src/shell/registry.rs` — the `scan_shell_ids` filter (Task 1) and the three
  new tests
- this phase doc's Update Log

**Do not** change `adopt_at_startup`, `sweep_stale`, `parse_cast_name`,
`host_is_alive`, any file outside `src/shell/registry.rs`, or the existing
tests (add new ones).

**Do not run `pkill`, `killall`, `tmux kill-server`, or any signal to a
process group.** These tests bind sockets and hold locks in temp dirs; let
`TempDir`/`Drop` clean them. Use `find <dir> -depth -delete` if you must.

## Out of scope

- The live restart re-verification (architect-run at milestone close, per the
  README's live-check convention). This phase makes it pass; running it is the
  close step.
- Any change to how the host is spawned, the socket bound, or the lock held —
  the host side already survives a restart; only the daemon's *discovery* of it
  is broken.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-09-08 20:11 (started)

Executor run began. Pre-flight confirmed: branch `master`, tree clean;
`cargo test --lib shell::registry::` → **24 passed; 0 failed**;
`grep -c 'is_file()' src/shell/registry.rs` → **1** (the defective filter at
line 242), `grep -c 'UnixListener::bind'` → **0**.

Plan: add the three pinned tests first — the real-socket guard written to be
red against the current `is_file()` filter — confirm the red, apply the
one-filter fix (shape chosen below, stated in the verification entry), confirm
green, then run the phase's E2E block into `/tmp/e2e-10.txt` and paste it in its
own entry.


### Update — 2026-09-08 20:2x (progress)

`bind_socket()` fixture helper added; the one-filter fix is applied
(`!is_file() && !is_socket()` shape, `FileTypeExt` imported inside
`scan_shell_ids`); `scan_shell_ids`'s doc comment now names the socket case.
Next: write the three pinned tests, confirm the real-socket one is red against
`is_file()`, then green after the fix.

**Red/green confirmed.** With the three new tests in place:

- Mutation (the filter reverted to `if !file_type.is_file() {}` via `patch`,
  restored the same way — no shell edits): `cargo test --lib
  shell::registry::` → `test result: FAILED. 25 passed; **2 failed**` —
  exactly the two real-socket tests (`scan_shell_ids_finds_a_real_socket`,
  `adopt_at_startup_adopts_a_real_socket_host`), while
  `scan_shell_ids_still_skips_a_directory_named_like_a_socket` stayed green
  (it does not depend on the socket type). This is the F1 guard going red
  against the pre-fix filter, confirming it is a real regression guard.
- Fix restored: `test result: ok. 27 passed; 0 failed`.

Both full `test result:` lines and the mutation diffs land in the
end-to-end verification entry below, per the phase doc.

Committed as `2dc6310 fix(shell): scan_shell_ids accepts real AF_UNIX sockets
(bug-10-1)`.

### Update — 2026-09-08 20:4x (end-to-end verification)

The phase's E2E block, run verbatim from the repo root into `/tmp/e2e-10.txt`
(final invocation — three runs of the block total; only the last is the
evidence, the two earlier ones covered the A1 gate while the tree still carried
doc-comment-only uncommitted edits, and every line of the artifact below is from
that single final run):

```text
== A. build ==
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s
cargo_exit=0
== B. named tests ==
shell::registry::tests::adopt_at_startup_adopts_a_real_socket_host ... ok
shell::registry::tests::scan_shell_ids_finds_a_real_socket ... ok
shell::registry::tests::scan_shell_ids_still_skips_a_directory_named_like_a_socket ... ok
cargo_exit=0
== C. registry total (>=25) ==
test result: ok. 27 passed; 0 failed; 0 ignored; 0 measured; 1643 filtered out; finished in 0.00s
cargo_exit=0
== D. untouched (session 7, shell_exec 10, launch 8, host 8) ==
shell::session                 test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 1663 filtered out; finished in 0.09s
cargo_exit=0
daemon::executor::shell_exec   test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 1660 filtered out; finished in 0.05s
cargo_exit=0
shell::launch                  test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 1662 filtered out; finished in 1.11s
cargo_exit=0
shell::host                    test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 1662 filtered out; finished in 0.00s
cargo_exit=0
== E. lib suite, three consecutive runs ==
test result: ok. 1666 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 7.39s
test result: ok. 1666 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 7.42s
test result: ok. 1666 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 7.43s
== F. structural greps ==
UnixListener::bind in tests (>=1): 1
make_dir negative kept    (>=3): 4
is_file removed OR is_socket added: is_file2 is_socket1
no unwrap/expect/panic/unsafe pre-test (0): 0
```

**Filter shape chosen:** accept-regular-file-or-socket —
`if !file_type.is_file() && !file_type.is_socket() { continue; }` with
`use std::os::unix::fs::FileTypeExt;` inside `scan_shell_ids`.
Per the phase doc's third criterion this shape keeps `is_file()` so its
`grep -c 'is_file()'` count is **not** 0; the satisfied alternative is
`is_socket()` ≥ **1**, and the artifact's F section shows `is_file2 is_socket1`.

Where those two `is_file()` occurrences live: 234 is the doc comment naming
why the socket case matters, and 247 is the half of the *fixed* predicate that
admits regular files — the test-fixture type — rather than dropping the
entry-type check. The original single defective line
(`if !file_type.is_file() { continue; }`) no longer exists:
`grep -cE '^\s*if !file_type\.is_file\(\)$'` → **0**.

**PASTE MATCH self-check:**

```text
PASTE MATCH
```

**Mutation check (F1 guard), run separately from the block** — the guard test
against the pre-fix filter, then the restored fix:

```text
# scan_shell_ids reverted to `if !file_type.is_file() { continue; }` via patch:
test result: FAILED. 25 passed; 2 failed; 0 ignored; 0 measured; 1643 filtered out; finished in 0.00s
   (failing: shell::registry::tests::scan_shell_ids_finds_a_real_socket;
             shell::registry::tests::adopt_at_startup_adopts_a_real_socket_host;
    passing: shell::registry::tests::scan_shell_ids_still_skips_a_directory_named_like_a_socket)
# filter restored to `!is_file() && !is_socket()` via patch:
test result: ok. 27 passed; 0 failed; 0 ignored; 0 measured; 1643 filtered out; finished in 0.00s
```

Both mutations were applied with `patch` (the executor contract bans in-place
shell edits) and each was confirmed applied by re-grepping the filter before
the test run it certified.

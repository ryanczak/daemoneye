# Phase 06a: the shell registry core — ids, owners and per-owner caps

**Milestone:** M20 — Shell Engine
**Status:** review
**Depends on:** phase-05c (`spawn_detached`, `socket_path`, `validate_shell_id`),
done.
**Estimated diff:** ~420 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

The in-memory registry the daemon will hold: what a shell **id** is, who
**owns** a shell, how many an owner may hold, and how a shell gets allocated
and spawned through it.

This phase is the registry's *shape*. Startup adoption, the dead-socket sweep
and exited-shell GC are phase-06b — see § Out of scope.

## Architecture references

- `docs/design/daemoneye-2.0.md` § the shell registry.
- `docs/design/daemon-stalls.md` § 1.5c — the non-reentrant-lock deadlock this
  phase must not reintroduce. Load-bearing; see § Spec Task 3.

## Pre-flight

- `git rev-parse --abbrev-ref HEAD` → `master`, tree clean.
- `cargo test --lib shell::` must report the counts below before you start. If
  any differs, stop and say so — the tree is not what this doc was written
  against.

## Current state

- `src/shell/` holds `mod.rs`, `pty.rs`, `log.rs`, `screen.rs`, `proto.rs`,
  `host.rs`, `backend.rs`, `launch.rs`. Counts today: `pty` **13**, `log`
  **12**, `screen` **11**, `host` **8**, `proto` **5**, `backend` **7**,
  `launch` **8**.
- `src/shell/registry.rs` **does not exist**. `cargo test --lib shell::registry::`
  matches **0** tests today.

Phase-05c left these, and they are **not** to be changed:

```rust
// src/shell/launch.rs:19 / :30 / :123
pub fn validate_shell_id(id: &str) -> Result<()>          // accepts ^s[0-9]{1,9}$ only
pub fn socket_path(id: &str) -> PathBuf                   // shells_dir()/<id>.sock
pub async fn spawn_detached(id: &str, shell: &str, rows: u16, cols: u16) -> Result<()>
                                                          // spawns + waits for readiness
```

**Reaching `validate_shell_id` is a gotcha.** `mod launch;` is *private* to
`shell` and `mod.rs`'s `pub use` line does **not** re-export
`validate_shell_id`. From `src/shell/registry.rs` reach it as
`super::launch::validate_shell_id` — that works because both modules live
inside `shell`. **Do not** add it to `mod.rs`'s `pub use`; nothing outside
`shell` needs it.

`ShellsConfig` (`src/config/types.rs:679`) already carries what this phase
consumes — do not add fields:

```rust
pub max_per_owner: u32,          // 0 = unlimited. Default 5.
pub exited_retention_secs: u64,  // phase-06b
pub log_retention_days: u32,     // phase-09
pub scrollback_lines: u32,       // already used by run_host
```

## Measured facts — executed 2026-09-05, not reasoned about

**F1 — `spawn_detached` CANNOT be exercised from a unit test, and this is why
`spawn_for` takes the spawn as a parameter.** `launch::spawn_detached`
re-invokes `std::env::current_exe()` with `["shell-host", "--id", …]`. Under
`cargo test`, `current_exe()` is the **libtest binary**
(`target/debug/deps/daemoneye-<hash>`), not the `daemoneye` CLI — and that
binary has no `shell-host` subcommand. Measured directly:

```
$ target/debug/deps/daemoneye-010209eb8f68e87e shell-host --id s0 --shell bash --rows 24 --cols 80
Terminated          <- killed by `timeout 10`; exit 143
```

It never binds a socket. A test that reached the real `spawn_detached` would
therefore hang for `await_ready`'s full 10-second deadline, fail, **and leak a
stuck test-binary process** — which § E2E section G would then correctly flag.
The seam is not a stylistic preference; the real path is untestable in-process.

(Phase-05c never hit this because its one real-PTY test calls `run_host`
**in-process**; `spawn_detached` was only ever exercised through the real
binary in that phase's E2E block.)

**F2 — the seam compiles on this toolchain.** rustc **1.95.0**, edition 2024.
An `FnOnce(ShellId) -> impl Future<Output = anyhow::Result<()>>` parameter was
compiled and run in both directions (stub returning `Ok`, stub returning `Err`)
before being specced.

## Spec

### Task 1 — `ShellId`

A newtype over the canonical `sN` string.

```rust
pub struct ShellId(String);
```

- `ShellId::parse(&str) -> anyhow::Result<ShellId>` — delegates to
  `super::launch::validate_shell_id`, then stores. **Do not re-implement the
  grammar**; a second validator is a second security boundary that will drift.
- `ShellId::from_index(n: u32) -> ShellId` — builds `format!("s{n}")`.
- `index(&self) -> u32` — the numeric part.
- `as_str(&self) -> &str`, and `impl std::fmt::Display`.

### Task 2 — `Owner`

```rust
pub enum Owner {
    Chat(String),   // a chat session id
    Ghost(String),  // a ghost job id
}
```

- `key(&self) -> String` — the cap-grouping key, and it **must not collide
  across kinds**: a chat session and a ghost job that happen to share an id
  string are different owners. Prefix the kind, e.g. `chat:<id>` / `ghost:<id>`.
  A negative test pins this.

### Task 3 — `ShellRegistry`

Mirror `SessionStore` (`src/daemon/session.rs:123`) exactly — a newtype so the
inner mutex cannot be locked from outside the module, with one accessor:

```rust
/// A newtype rather than an alias so the inner mutex cannot be locked from
/// outside this module: `with_sessions` is the only way in, which is what keeps
/// blocking work out of the critical section.
#[derive(Clone, Default)]
pub struct SessionStore(Arc<Mutex<HashMap<String, SessionEntry>>>);

pub fn with_sessions<T>(
    sessions: &SessionStore,
    f: impl FnOnce(&mut HashMap<String, SessionEntry>) -> T,
) -> T {
    let _depth = SessionsLockDepth::enter();
    let mut store = sessions.0.lock().unwrap_or_log();
    f(&mut store)
}
```

So: `pub struct ShellRegistry(Arc<Mutex<HashMap<String, ShellRecord>>>)`
(keyed by the id string) plus `pub fn with_shells<T>(reg: &ShellRegistry, f: impl FnOnce(&mut HashMap<String, ShellRecord>) -> T) -> T`.

```rust
pub struct ShellRecord {
    pub id: ShellId,
    pub owner: Owner,
    pub started_unix: u64,
}
```

**THE load-bearing constraint of this phase.** `std::sync::Mutex` is not
reentrant, and every IPC handler in this daemon locks stores like this one. The
codebase already carries an assertion for exactly this, with the reasoning
inline (`src/daemon/session.rs:490`):

```rust
assert_eq!(
    d.get(), 0,
    "re-entrant SessionStore lock: with_sessions() called while this \
     thread already holds the store. std::sync::Mutex is not reentrant \
     — this would deadlock the whole daemon. Collect what you need \
     inside the outer closure and use it after it returns. See \
     docs/design/daemon-stalls.md § 1.5c."
);
```

Therefore **`spawn_detached` must never be called inside a `with_shells`
closure.** It spawns a process and waits up to ten seconds for readiness;
holding the registry lock across it would stall every handler that touches the
registry. Task 4 says how.

Add the same `#[cfg(test)] pub fn try_lock(&self)` escape hatch `SessionStore`
carries — its doc comment says why, and this phase uses it for the same
purpose:

```rust
/// Test-only escape hatch for asserting the guard was released.
///
/// Deliberately `#[cfg(test)]`: production code must go through
/// `with_sessions`, and a non-blocking peek is only ever needed to prove a
/// lock is *not* held.
```

You do **not** need to replicate the thread-local depth counter; the paired
test below is this phase's guard.

### Task 4 — allocation, caps, and `spawn_for`

Two **pure** functions, tested directly:

```rust
/// Lowest unused index, starting at 0.
pub fn allocate_index(occupied: &HashSet<u32>) -> u32

/// Ok(()) when `owner` may take one more; Err naming the cap when not.
/// `max_per_owner == 0` means unlimited.
pub fn check_owner_cap(
    existing: &HashMap<String, ShellRecord>,
    owner: &Owner,
    max_per_owner: u32,
) -> anyhow::Result<()>
```

`allocate_index` must fill **holes**: given `{0, 1, 3}` it returns `2`, not `4`.
Given `{}` it returns `0`.

Then the one async entry point. **It takes the spawn as a parameter** — see
§ "Measured facts" F1 for why it cannot call `spawn_detached` directly:

```rust
pub async fn spawn_for<F, Fut>(
    reg: &ShellRegistry,
    owner: Owner,
    max_per_owner: u32,
    spawn: F,
) -> anyhow::Result<ShellId>
where
    F: FnOnce(ShellId) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
```

Verified to compile on this toolchain (rustc 1.95.0, edition 2024) in both the
success and failure directions before this was specced.

The ordering is the spec:

1. **Inside** one `with_shells` closure: run `check_owner_cap`, compute the
   occupied index set, call `allocate_index`, and **reserve** the id by
   inserting its `ShellRecord`. Return the id from the closure.
2. **After the closure returns** — lock released — `spawn(id.clone()).await`.
3. If the spawn returns `Err`, take the lock a **second** time to remove the
   reserved record, then return the error. Reserving before spawning is what
   stops two concurrent callers racing onto the same index; removing on failure
   is what stops a failed spawn leaking the slot.

The production caller (phase-07, **not this phase**) will pass a closure
wrapping `launch::spawn_detached`; this phase's tests pass stubs. Nothing in
this phase calls `spawn_detached`, and `shell`/`rows`/`cols` are therefore not
parameters of `spawn_for` — they belong to whatever the caller's closure
captures.

## Acceptance criteria

Each was run against the current tree while drafting and returns the "before"
value shown.

- [ ] `test -f src/shell/registry.rs && echo yes` → **yes** (now: absent).
- [ ] `grep -c '^mod registry;' src/shell/mod.rs` → **1** (now `0`).
- [ ] `grep -cE '^pub struct ShellRegistry' src/shell/registry.rs` → **1**.
- [ ] `grep -cE '^pub fn with_shells' src/shell/registry.rs` → **1**.
- [ ] **The id grammar is not re-implemented:**
      `grep -c 'validate_shell_id' src/shell/registry.rs` → **1 or more**, and
      `grep -cE 's\[0-9\]|is_ascii_digit|chars\(\)\.all' src/shell/registry.rs`
      → **0**.
- [ ] **`validate_shell_id` stays unexported:**
      `grep -c 'validate_shell_id' src/shell/mod.rs` → **0**.
- [ ] No `unsafe` in non-test code:
      `awk '/^#\[cfg\(test\)\]/{exit} {print}' src/shell/registry.rs | grep -vE '^\s*(//|///|//!|\*)' | grep -c 'unsafe'`
      → **0**. The `^` anchor is required or a doc comment mentioning
      `#[cfg(test)]` ends the scan early.
- [ ] No `unwrap`/`expect`/`panic!` outside test code:
      `awk '/^#\[cfg\(test\)\]/{exit} {print}' src/shell/registry.rs | grep -cE '\.(unwrap|expect)\(|panic!\('`
      → **0**. (`unwrap_or_log` and `unwrap_or_else` are not matched by this
      pattern and are both fine.)
- [ ] **The real spawn is not called from this phase (F1):**
      `grep -c 'spawn_detached' src/shell/registry.rs` → **0**.
- [ ] `cargo test --lib shell::registry::` reports **8 or more** passing,
      `0 failed` (now: `0 passed; … 1604 filtered out`).
- [ ] Earlier phases unchanged: `shell::launch::` **8**, `shell::pty::` **13**,
      `shell::log::` **12**, `shell::screen::` **11**, `shell::host::` **8**,
      `shell::proto::` **5**, `shell::backend::` **7**.
- [ ] All four gates pass.

## Test plan

Names pinned; placement is not — `#[cfg(test)] mod tests` at the bottom of
`src/shell/registry.rs`. Every name begins `registry_`.

**The two things that happen at once here are the lock and the spawn**, and one
test must show them not overlapping. That is this milestone's standing rule and
it is what `registry_spawn_for_does_not_hold_the_lock_across_the_spawn` exists
for — do not replace it with a test that merely checks the lock is free
*afterwards*.

- `registry_allocate_index_fills_holes` — `{0,1,3}` → **2**; `{}` → **0**;
  `{0,1,2}` → **3**. Assert each case separately.
- `registry_shell_id_parse_delegates_to_the_shared_validator` — `s0`, `s7`,
  `s123456789` parse; and the **negative cases**, each asserted individually:
  `""`, `"."`, `".."`, `"../etc/passwd"`, `"s1/x"`, `"s"`, `"s-1"`, `"S1"`,
  `"shell"`, `"s1.sock"`, `"s1234567890"`.
- `registry_shell_id_round_trips_index_and_string` — `from_index(42)` gives
  `"s42"` and `index()` gives back `42`.
- `registry_owner_keys_do_not_collide_across_kinds` — **the negative case.**
  `Owner::Chat("x".into()).key() != Owner::Ghost("x".into()).key()`. Same id
  string, different owners.
- `registry_check_owner_cap_counts_only_that_owner` — with a map holding two
  shells for `Chat("a")` and one for `Chat("b")`, a cap of 2 refuses another
  for `a` and allows one for `b`.
- `registry_check_owner_cap_zero_means_unlimited` — cap `0` allows a further
  shell even with several already held.
- `registry_spawn_for_does_not_hold_the_lock_across_the_spawn` — **the
  simultaneity test, and the seam is what makes it deterministic.** Pass a stub
  spawn closure that *blocks until the test releases it* — e.g. it awaits a
  `tokio::sync::oneshot` receiver, or polls an `AtomicBool` the test sets. While
  that stub is suspended, assert from the test that the registry is reachable:
  either `try_lock()` succeeds, or a `with_shells` call completes. Then release
  the stub and let `spawn_for` finish. **A test that only checks the lock after
  `spawn_for` returns does not prove this and is not acceptable** — the whole
  claim is that the lock is free *during* the spawn.
- `registry_spawn_for_releases_the_slot_when_the_spawn_fails` — pass a stub
  that returns `Err(anyhow::anyhow!("boom"))`, and assert the map is left with
  **no** record for the id that was reserved. Also assert the next
  `allocate_index` over the resulting occupied set returns that same index,
  proving the slot is genuinely free rather than merely absent.

**With the seam, none of these tests should need `HOME`, a temp dir, a real
process, or a PTY.** Every one of them operates on the registry map and pure
functions, with stub spawn closures. If you find yourself reaching for
`test_home_guard()` or `Config::ensure_dirs()`, stop — that is a signal you are
about to call the real `spawn_detached`, which F1 says cannot work here.

Should a test nonetheless need `HOME` (it should not), use
`crate::test_home_guard()` — **not** the raw `TEST_HOME_LOCK`, which does not
recover from poisoning — bound to a named variable (`_guard`, not bare `_`,
which drops immediately), with a distinct temp dir per test.

**Spawn no real processes in this phase.** Section G of the E2E block asserts
zero stray hosts, and with stub closures that is automatic. Do **not** use
`pkill`/`killall` or any signal to a process group.

## End-to-end verification

Run this block verbatim from the repo root. It writes `/tmp/e2e-06a.txt`.

```sh
{
echo "== A. build =="
cargo build 2>&1 | tail -2; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== B. named tests (each line is one pinned test) =="
cargo test --lib 2>&1 | grep -E "^test shell::registry::.* ok$" | sed 's/^test //' | sort
echo "cargo_exit=${PIPESTATUS[0]}"
echo "== C. shell::registry:: totals =="
cargo test --lib shell::registry:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== D. earlier phases untouched (8, 13, 12, 11, 8, 5, 7) =="
for m in launch pty log screen host proto backend; do
  cargo test --lib shell::$m:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
done
echo "== E. lib suite, three consecutive runs (see the note below) =="
for r in 1 2 3; do cargo test --lib 2>&1 | grep -E "^test result:" | head -1; done
echo "== F. structural greps (each must print the stated number) =="
echo -n "registry.rs exists        (1): "; test -f src/shell/registry.rs && echo 1 || echo 0
echo -n "mod registry declaration  (1): "; grep -c '^mod registry;' src/shell/mod.rs
echo -n "pub struct ShellRegistry  (1): "; grep -cE '^pub struct ShellRegistry' src/shell/registry.rs
echo -n "pub fn with_shells        (1): "; grep -cE '^pub fn with_shells' src/shell/registry.rs
echo -n "validator reused         (>=1): "; grep -c 'validate_shell_id' src/shell/registry.rs
echo -n "no re-implemented grammar (0): "; grep -cE 's\[0-9\]|is_ascii_digit|chars\(\)\.all' src/shell/registry.rs
echo -n "validator stays unexported(0): "; grep -c 'validate_shell_id' src/shell/mod.rs
echo -n "no real spawn_detached    (0): "; grep -c 'spawn_detached' src/shell/registry.rs
echo -n "no unsafe pre-test        (0): "
awk '/^#\[cfg\(test\)\]/{exit} {print}' src/shell/registry.rs | grep -vE '^\s*(//|///|//!|\*)' | grep -c 'unsafe'
echo -n "unwrap/expect/panic pre-test (0): "
awk '/^#\[cfg\(test\)\]/{exit} {print}' src/shell/registry.rs | grep -cE '\.(unwrap|expect)\(|panic!\('
echo "== G. no shell-host leaked by the test run =="
echo -n "stray daemoneye shell-host processes (0): "
ps -eo comm,args | awk '$1=="daemoneye" && /shell-host/ {n++} END{print n+0}'
} > /tmp/e2e-06a.txt 2>&1
cat /tmp/e2e-06a.txt
```

Paste the contents of `/tmp/e2e-06a.txt` into your Update Log entry as a fenced
block. **The entry's header must be exactly this line**, or the self-check
below cannot find it:

```
### Update — YYYY-MM-DD HH:MM (end-to-end verification)
```

Then run the self-check and paste its verdict line into the same entry:

```sh
D=docs/dev/milestones/M20-shell-engine/phase-06a-shell-registry-core.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-06a.txt
diff /tmp/pasted-06a.txt /tmp/e2e-06a.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

**Sections B through E can each report success with nothing having run.**
Measured on the current tree: `cargo test --lib shell::registry::` prints
`test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1604 filtered out`
and exits `0`. A zero exit proves nothing — the pass conditions are the named
lines in B, eight or more in C, and exactly `8`, `13`, `12`, `11`, `8`, `5`,
`7` in D.

**Section E runs three times on purpose.** This phase's tests spawn real hosts
under a mutated `HOME`, which is exactly the shape that produced `bug-05c-1` —
a race that reproduced at roughly one run in six and would have passed a single
green run. All three must report `0 failed`; if any run fails, that is a real
defect, not a flake to re-roll.

**Section F on an absent file errors rather than printing `0`** — `grep -c`
against a missing path warns on stderr and exits `2`. The block redirects
`2>&1`, so a warning there is itself proof the file is missing.

**Section G must print `0`.** A non-zero count means a test leaked a detached
shell host. Do not clear it with `pkill` — find the test that failed to clean up.

**Do not "simplify" section G to `pgrep -f 'daemoneye shell-host'`.** Measured
while drafting: `pgrep -f` matches **its own invoking shell**, because the
pattern string appears in that shell's command line, so it reports `1` on a
completely clean tree. The `ps -eo comm,args` form filters on the executable
name, which a `bash -c` wrapper can never match. It was validated in both
directions: `0` with nothing running, `1` with one real host up, `0` again
after that host was killed.

## Authorizations

You may edit, and nothing else:

- **create** `src/shell/registry.rs`
- `src/shell/mod.rs` — add `mod registry;` and one `pub use` line
- this phase doc's Update Log

**Do not** change `launch.rs`, `host.rs`, `backend.rs`, `pty.rs`, `log.rs`,
`screen.rs` or `proto.rs`. In particular, do not add `validate_shell_id` to
`mod.rs`'s `pub use` — reach it as `super::launch::validate_shell_id`.

**Do not** touch `src/config/lifecycle.rs`. The `var/run/shells` row is marked
`Pending { owned_by: "M20 phase-06" }` and is deliberately left for **06b**,
which is the phase that actually implements the retention behaviour that row
describes. Flipping it here would make the table claim something untrue.

**Do not run `pkill`, `killall`, `tmux kill-server`, `kill -- -1`, or any
signal to a process group or to PID -1.** Kill only pids you captured yourself.

## Out of scope

- **Startup adoption** — scanning `var/run/shells/` and re-attaching to hosts
  that survived a daemon restart. Phase-06b.
- **Dead-socket / stale-lock sweep**, and exited-shell GC under
  `shells.exited_retention_secs`. Phase-06b, together with flipping the
  `var/run/shells` lifecycle row.
- Wiring the registry into the daemon, or into `run_terminal_command`
  (phase-07). Nothing constructs a `ShellRegistry` outside tests in this phase.
- Any change to the wire protocol, the host binary, or `ShellsConfig`.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-09-05 12:44 (progress)

Started phase 06a as a fresh dispatch (per the Notes, the prior run died on an
infrastructure fault with no source on disk). Implemented all four tasks in
`src/shell/registry.rs` (323 lines): `ShellId` delegating to
`super::launch::validate_shell_id`, `Owner` with `key()` prefixing the kind,
`ShellRegistry` mirroring `SessionStore`'s newtype+`with_shells` shape plus a
`#[cfg(test)] try_lock` escape hatch, and `allocate_index` /
`check_owner_cap` / `spawn_for` with reserve-under-lock / spawn-outside-lock /
unreserve-on-failure. Added `mod registry;` and a `pub use` line to
`src/shell/mod.rs`. All eight pinned tests pass
(`cargo test --lib shell::registry::` → 8 passed, 0 failed); clippy is clean
after scoping the simultaneity test's guard (it flagged `await_holding_lock`,
fixed by a brace block) and dropping a unit-value let. Formatting, all four
gates and the structural greps verified; E2E block runs next.

### Update — 2026-09-05 12:44 (end-to-end verification)

```
== A. build ==
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.06s
cargo_exit=0
== B. named tests (each line is one pinned test) ==
shell::registry::tests::registry_allocate_index_fills_holes ... ok
shell::registry::tests::registry_check_owner_cap_counts_only_that_owner ... ok
shell::registry::tests::registry_check_owner_cap_zero_means_unlimited ... ok
shell::registry::tests::registry_owner_keys_do_not_collide_across_kinds ... ok
shell::registry::tests::registry_shell_id_parse_delegates_to_the_shared_validator ... ok
shell::registry::tests::registry_shell_id_round_trips_index_and_string ... ok
shell::registry::tests::registry_spawn_for_does_not_hold_the_lock_across_the_spawn ... ok
shell::registry::tests::registry_spawn_for_releases_the_slot_when_the_spawn_fails ... ok
cargo_exit=0
== C. shell::registry:: totals ==
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 1604 filtered out; finished in 0.00s
cargo_exit=0
== D. earlier phases untouched (8, 13, 12, 11, 8, 5, 7) ==
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 1604 filtered out; finished in 1.11s
cargo_exit=0
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 1599 filtered out; finished in 2.51s
cargo_exit=0
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 1600 filtered out; finished in 0.00s
cargo_exit=0
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 1601 filtered out; finished in 0.00s
cargo_exit=0
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 1604 filtered out; finished in 0.00s
cargo_exit=0
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 1607 filtered out; finished in 0.00s
cargo_exit=0
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 1605 filtered out; finished in 3.55s
cargo_exit=0
== E. lib suite, three consecutive runs (see the note below) ==
test result: ok. 1608 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 7.25s
test result: ok. 1608 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 7.32s
test result: ok. 1608 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 7.29s
== F. structural greps (each must print the stated number) ==
registry.rs exists        (1): 1
mod registry declaration  (1): 1
pub struct ShellRegistry  (1): 1
pub fn with_shells        (1): 1
validator reused         (>=1): 2
no re-implemented grammar (0): 0
validator stays unexported(0): 0
no real spawn_detached    (0): 0
no unsafe pre-test        (0): 0
unwrap/expect/panic pre-test (0): 0
== G. no shell-host leaked by the test run ==
stray daemoneye shell-host processes (0): 0
```


### Notes for executor — 2026-09-05

**Start fresh; nothing of yours is on disk.** The previous run (session
`6a9c59cc`, 75 turns) died on an infrastructure fault —
`BackendError: "error decoding response body"` — with 0/4 tasks done and no
source written. Only the two `Status:` flips landed.

**But it was not a wasted run, and the spec has changed because of it.** Its
last dozen calls were spent spawning and killing `target/debug/deps/daemoneye-*`
pids, which is the executor discovering — correctly — that the phase as
originally written could not be built. § "Measured facts" F1 now records what
it was running into: `spawn_detached` re-invokes `current_exe()`, which under
`cargo test` is the **libtest binary**, not the CLI, so the two tests that
called `spawn_for` could never have passed. That was the architect's defect,
not yours.

**What changed:** `spawn_for` now takes the spawn as a parameter
(`F: FnOnce(ShellId) -> Fut`), verified to compile on rustc 1.95.0 / edition
2024 before being specced. Your tests pass **stub closures** — so the
simultaneity test becomes deterministic (a stub that blocks until the test
releases it) and the failure test becomes trivial (a stub returning `Err`).
Nothing in this phase spawns a real process, needs `HOME`, or needs a PTY, and
a new criterion pins `spawn_detached` at **0** occurrences in
`src/shell/registry.rs`.

**Do not go looking for a way to invoke the real binary from a test.** F1 is
the answer: there isn't one, and the seam is the design.

### Update — 2026-09-05 (escalation)

**Chosen lever:** refined re-dispatch
**Rationale:** the run died on infrastructure (`BackendError: "error decoding
response body"`, 75 turns, 0/4 tasks, no source on disk), but reading what it
was doing exposed a real spec defect — two pinned tests that could never have
passed — so a plain resume would have carried the same wall forward.

**What the run was actually hitting.** Its last dozen tool calls spawn and kill
`target/debug/deps/daemoneye-*` pids. That is the executor discovering that
`spawn_for` → `launch::spawn_detached` → `std::env::current_exe()` resolves to
the **libtest binary** under `cargo test`, which has no `shell-host`
subcommand. Measured at escalation:

```
$ target/debug/deps/daemoneye-010209eb8f68e87e shell-host --id s0 --shell bash --rows 24 --cols 80
Terminated          <- killed by `timeout 10`; exit 143
```

It never binds a socket, so `await_ready` would burn its full 10-second
deadline, fail, and leak a stuck process. **Phase-05c never surfaced this
because its one real-PTY test calls `run_host` in-process** — `spawn_detached`
was only ever exercised through the real binary in that phase's E2E block.

**The refinement:** `spawn_for` now takes the spawn as a parameter
(`F: FnOnce(ShellId) -> Fut`), compiled and run in both directions on rustc
1.95.0 / edition 2024 before being specced. Tests use stub closures, which
makes the simultaneity test *deterministic* rather than dependent on a real
process — and incidentally answers the open design question recorded at the
05a close about needing a backend test seam. A new criterion pins
`spawn_detached` at **0** occurrences in `src/shell/registry.rs`.

**Executor conduct was good and is worth recording:** it did not fake a passing
test or quietly drop the requirement. It went looking for the mechanism, which
is what surfaced the defect.

### Update — ts=1788637800071 (complete, server-authored)

**Summary:** </think>All done. Let me run through the completion checklist one final time:

- [x] Phase doc's Status: line still says `in-progress` — confirmed at line 4.
- [x] Milestone README phase-table row still `in-progress` — confirmed at line 167.
- [x] No `(complete)` Update Log entry hand-written — correct.
- [x] `cargo fmt --all` was run immediately before `git add` — yes.
- [x] All verification commands ran clean — fmt, build, clippy, test (3x lib runs).
- [x] Final message is a substantive Summary + Notes for review.
- [x] `git status --short` shows nothing — confirmed.
- [x] `git log -1 --stat` shows the commit includes every file — confirmed.

All four tasks done. Time for the final summary.</think>
Phase 06a is complete. Working tree is clean; the phase doc and README statuses remain `in-progress` for the server to finalize.

## Summary + Notes for review

Built the shell registry core in `src/shell/registry.rs` (334 lines) per spec: `ShellId` (delegates grammar to `super::launch::validate_shell_id`, never re-implemented; `from_index`/`index`/`as_str`/`Display`), `Owner` with kind-prefixed `key()` (`chat:<id>`/`ghost:<id>` — the negative collision test pins it), `ShellRegistry` as a `SessionStore`-mirroring newtype over `Arc<Mutex<HashMap<String, ShellRecord>>>` with `with_shells` as the only interface plus a `#[cfg(test)] try_lock` escape hatch. Task 4: pure `allocate_index` (hole-filling) and `check_owner_cap` (0 = unlimited), and `spawn_for<F, Fut>` which reserves the record under the lock, spawns with the lock **released**, and unreserves + propagates `Err` on failure. Added `mod registry;` and a `pub use` line to `src/shell/mod.rs`; `validate_shell_id` stays unexported (reached via `super::launch::`).

All eight pinned tests pass; E2E confirms the counts (`launch` 8, `pty` 13, `log` 12, `screen` 11, `host` 8, `proto` 5, `backend` 7), three consecutive lib runs each `1608 passed; 0 failed`, structural greps all exact, and no stray shell-host processes. The full mechanical transcript is pasted under the `### Update — 2026-09-05 12:44 (end-to-end verification)` header with the header and self-check both passing (`PASTE MATCH`).

Two implementation notes for review, both cosmetic and within the spec's authority:
- `spawn_for`'s closure needed an explicit `-> anyhow::Result<ShellId>` return annotation (rustc 1.95 couldn't infer the error type at the `Ok(id)` tail) — no behavioral change.
- The simultaneity test scopes its `try_lock` guard in a brace block and drops the `started_rx.await` unit-value `let _ =` to satisfy `clippy -D warnings` (`await_holding_lock`, `let_unit_value`) — the assertion is unchanged: the lock is proven free *while the stub spawn is suspended*, and the slot is reserved before the spawn.

**Commits:** `7a8f676` — `feat(shell): add the shell registry core — ids, owners, per-owner caps`

**Gates:** fmt=run, build=run, lint=run, test=run — all clean, no stray hosts, working tree empty.

**Executor:** deepseek-ai/DeepSeek-V4-Flash-0731

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.06s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s


TEST
nored; 0 measured; 0 filtered out; finished in 7.25s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_reads_bare_word ... ok
test header_status_strips_trailing_prose ... ok
test header_status_uses_first_occurrence_only ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 10 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_tools_counts_are_accurate ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_config_template_documents_every_config_field ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 33 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ghost_config_parsing ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test minimal_config_parsing ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_jsonl_round_trip ... ok
test session_index_persistence ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test g5_mailbox_write_and_read ... ok

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
     Running unittests src/lib.rs (target/debug/deps/daemoneye-9810e08c3de1e001)
     Running unittests src/main.rs (target/debug/deps/daemoneye-54ad57cf3704bc40)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-d709c9b862b5691d)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-34cb94d3012a3d69)
     Running tests/integration.rs (target/debug/deps/integration-deb70358b520d0d5)
     Running tests/isolation.rs (target/debug/deps/isolation-45facff22bac60f5)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M20-shell-engine/phase-06a-shell-registry-core.md` — +70 -0
- `src/shell/mod.rs` — +5 -0
- `src/shell/registry.rs` — +334 -0

**Commit:** 7a8f6763bc63bdb040beef86482efa064ec7ab68

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

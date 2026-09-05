# Phase 06a: the shell registry core — ids, owners and per-owner caps

**Milestone:** M20 — Shell Engine
**Status:** todo
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

Then the one async entry point, and the ordering is the spec:

```rust
pub async fn spawn_for(
    reg: &ShellRegistry,
    owner: Owner,
    shell: &str,
    rows: u16,
    cols: u16,
    max_per_owner: u32,
) -> anyhow::Result<ShellId>
```

1. **Inside** one `with_shells` closure: run `check_owner_cap`, compute the
   occupied index set, call `allocate_index`, and **reserve** the id by
   inserting its `ShellRecord`. Return the id from the closure.
2. **After the closure returns** — lock released — call
   `super::launch::spawn_detached(id.as_str(), shell, rows, cols).await`.
3. If the spawn fails, take the lock a **second** time to remove the reserved
   record, then return the error. Reserving before spawning is what stops two
   concurrent callers racing onto the same index; removing on failure is what
   stops a failed spawn leaking the slot.

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
  simultaneity test.** Under a temp `HOME`, call `spawn_for`; from another
  thread (or between polls) assert `try_lock()` **succeeds** while the spawn is
  still in flight. Bound every wait. If holding `try_lock` for the whole spawn
  is awkward, assert instead that a second `with_shells` call completes while
  the spawn is outstanding — either shape proves the lock is not held across
  it; a test that only checks the lock after `spawn_for` returns does not and
  is not acceptable.
- `registry_spawn_for_releases_the_slot_when_the_spawn_fails` — force a spawn
  failure (an unspawnable shell name such as `"definitely-not-a-real-shell"` is
  the simplest lever) and assert the map is left with **no** record for the id
  that was reserved. Also assert the next `allocate_index` returns that same
  index, proving the slot is genuinely free rather than merely absent.

Tests that touch `shells_dir()` mutate `HOME` and **must** take the RAII guard
first — and, per `bug-05c-1`, **any test that reaches a PTY spawn must hold it
across the spawn.** `spawn_for` spawns a real host, so the guard is required:

```rust
let _guard = crate::test_home_guard();
let dir = tempfile::tempdir().unwrap();
unsafe { std::env::set_var("HOME", dir.path()); }
crate::config::Config::ensure_dirs().ok();
```

Use `crate::test_home_guard()`, **not** the raw `TEST_HOME_LOCK` — the accessor
recovers from poisoning. Bind it to a named variable (`_guard`, not bare `_`,
which drops immediately). Give each test a distinct temp dir.

**Kill every host you spawn.** A leaked `daemoneye shell-host` outlives the test
run — that is the whole point of `process_group(0)`. Capture the id and remove
its socket/lock, or keep the spawned shell short-lived. Do **not** use
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

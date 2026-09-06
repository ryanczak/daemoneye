# Phase 06b: startup adoption and the stale-host sweep

**Milestone:** M20 — Shell Engine
**Status:** done
**Depends on:** phase-06a (`ShellId`, `Owner`, `ShellRegistry`, `with_shells`)
and phase-05c (`socket_path`, `lock_path`, `cast_path`), both done.
**Estimated diff:** ~380 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Make the registry survive a daemon restart: scan `var/run/shells/`, **adopt**
the shells whose hosts are still alive, and **sweep** the leftovers of hosts
that are gone — without ever touching a live one.

This is the library half. Wiring it into daemon startup and proving it end to
end across a real `daemoneye stop` / `daemoneye daemon` is phase-09.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.1 — the shell-host process model and the
  restart-survival requirement.
- `CLAUDE.md` § Important Invariants — *"Liveness probes are reports, never
  authorizations. No code may unlink a socket or remove a file based on a
  `DaemonLiveness` variant — instance ownership is decided only by the
  `InstanceLock` flock."* This phase is the shell-side application of that rule.

## Pre-flight

- `git rev-parse --abbrev-ref HEAD` → `master`, tree clean.
- `cargo test --lib shell::` must report the counts in § Current state before
  you start. If any differs, stop and say so.

## Current state

Counts today: `registry` **8**, `launch` **8**, `pty` **13**, `log` **12**,
`screen` **11**, `host` **8**, `proto` **5**, `backend` **7**.

Phase-06a landed these in `src/shell/registry.rs`, and they are the base you
extend — do not redesign them:

```rust
pub struct ShellId(String);              // ShellId::parse / from_index / index / as_str
pub enum Owner { Chat(String), Ghost(String) }   // Owner::key() -> "chat:<id>" / "ghost:<id>"
pub struct ShellRecord { pub id: ShellId, pub owner: Owner, pub started_unix: u64 }
pub struct ShellRegistry(Arc<Mutex<HashMap<String, ShellRecord>>>);
pub fn with_shells<T>(reg: &ShellRegistry, f: impl FnOnce(&mut HashMap<String, ShellRecord>) -> T) -> T
```

Phase-05c landed the path constructors (`src/shell/launch.rs`), reachable from
`registry.rs` as `super::launch::*`:

```rust
pub fn socket_path(id: &str) -> PathBuf   // shells_dir()/<id>.sock
pub fn lock_path(id: &str) -> PathBuf     // shells_dir()/<id>.lock
pub fn cast_path(id: &str, started_unix: u64) -> PathBuf
                                          // shell_logs_dir()/<id>-<started_unix>.cast
```

## Measured facts — executed 2026-09-06, not reasoned about

**F1 — exactly what a host leaves on disk.** A real `daemoneye shell-host --id s3`
under a throwaway `HOME`:

```
~/.daemoneye/var/run/shells/s3.sock
~/.daemoneye/var/run/shells/s3.lock          payload: 1264595
~/.daemoneye/var/log/shells/s3-1788699323.cast
```

**F2 — the flock is the liveness oracle; the socket proves nothing.**

| state | flock on `<id>.lock` | `connect()` to `<id>.sock` | files on disk |
|---|---|---|---|
| host alive | **BLOCKED** | succeeds | `.lock` + `.sock` |
| host killed | **ACQUIRED** | `ECONNREFUSED` | `.lock` + `.sock` **both remain** |

Nothing cleans up after a dead host — that is what the sweep is for. And
`connect()` is **not** a substitute: phase-05c measured the case where it lies,
a socket bound and listening but not yet accepting completes a client
`connect()` and even accepts a write, so a host mid-startup is
indistinguishable from a healthy one.

**F3 — `flock` conflicts across two opens in the same process, and releases on
close.** This is what makes the tests below hermetic and real rather than faked:

```
first flock:                      acquired
second flock, SAME PROCESS:       BLOCKED     <- separate open = separate file description
after closing the first:          ACQUIRED
```

So a test can hold an `InstanceLock` to *be* a live host, and the probe — a
second acquire — correctly reports "alive". No child process needed.

**F4 — the two pids are different, and conflating them is a bug.** The `.lock`
payload is the **shell-host process** pid; a `Status` reply's `pid` is the
**shell child** pid:

```
lock payload : 1264595      (the host)
Status reply : {"type":"Status","state":"Running","rows":24,"cols":80,"pid":1264630}
```

Neither is used for liveness. F2's flock is.

**F5 — nothing clears `var/run` at startup, so adoption is viable today.**
`LifecycleIntent::ClearAtStartup` appears **only** in
`src/config/lifecycle.rs` — no code implements it, despite the `var/run` row
being marked `Implemented`. Adoption therefore works today by accident rather
than by design. **This is a trap worth knowing about:** if `ClearAtStartup` is
ever implemented for `var/run`, it will delete `var/run/shells/` and silently
break restart survival unless the `var/run/shells` exemption is honoured. Do
not implement `ClearAtStartup` in this phase — just do not rely on the
accident either.

**F6 — `owner` is NOT recoverable from disk.** Nothing on disk records which
session owned a shell: the lock has a pid, the cast has a timestamp, `Status`
returns state/rows/cols/pid. `started_unix` **is** recoverable, from the cast
filename (`s3-1788699323.cast`). This is why Task 2 adds an `Owner` variant
rather than trying to reconstruct one.

## Spec

### Task 1 — `host_is_alive`, the probe

```rust
pub fn host_is_alive(lock: &Path) -> bool
```

Use the existing `crate::daemon::instance::InstanceLock` — **do not write a
second locking scheme**:

- `InstanceLock::acquire(lock)` returns `Err(AcquireError::Held { .. })` → the
  host holds it → **alive** (`true`).
- Returns `Ok(guard)` → nobody holds it → **dead** (`false`). Drop the guard
  immediately; do not hold it.
- Returns `Err(AcquireError::Io(_))` → **fail closed: return `true`.** We could
  not prove it is dead, and the caller deletes files based on this answer.
  Reporting "dead" on an unreadable lock would delete a live shell's socket.
  A named test pins this.

Acquiring is safe as a probe: on the `Held` path `InstanceLock` never writes
(the `Flock::lock` fails before the pid write), and on the `Ok` path it rewrites
a pid into a file we are about to delete anyway.

### Task 2 — `Owner::Adopted`

Add a third variant:

```rust
pub enum Owner { Chat(String), Ghost(String), Adopted }
```

with `key()` returning `"adopted"`. Per F6 the original owner is not on disk,
and at daemon start the owning session is gone by definition. An adopted shell
is owned by nobody until a later phase re-claims it.

**`key()` must still not collide** — `"adopted"` is distinct from any
`"chat:<id>"` / `"ghost:<id>"`, and the existing collision test is extended to
cover it.

### Task 3 — scanning and `started_unix` recovery

Two **pure** functions, tested directly:

```rust
/// Shell ids with a socket in `dir`, sorted by index. Ignores anything whose
/// stem is not a valid shell id, and anything that is not a `.sock`.
pub fn scan_shell_ids(dir: &Path) -> Vec<ShellId>

/// (id, started_unix) parsed from a cast filename, or None.
pub fn parse_cast_name(name: &str) -> Option<(ShellId, u64)>
```

`scan_shell_ids` must reuse `ShellId::parse` for the stem — **do not
re-implement the grammar.** Must-NOT-pick-up, each pinned:
`evil.sock`, `../x.sock`, `s1.sock.bak`, `s1.lock`, `S1.sock`, `s1234567890.sock`,
and a *directory* named `s2.sock`.

`parse_cast_name` accepts `s3-1788699323.cast` → `(s3, 1788699323)`.
Must return `None` for: `s3.cast`, `s3-abc.cast`, `s3-.cast`, `-17.cast`,
`notashell-17.cast`, `s3-17.txt`, `s3-17-9.cast`.

### Task 4 — `adopt_at_startup` and `sweep_stale`

```rust
/// Adopt every live shell in `dir` into `reg`; return the ids adopted.
pub fn adopt_at_startup(reg: &ShellRegistry, dir: &Path, logs: &Path) -> Vec<ShellId>

/// Remove the socket and lock of every DEAD host in `dir`; return the ids swept.
pub fn sweep_stale(dir: &Path) -> Vec<ShellId>
```

`adopt_at_startup`: for each id from `scan_shell_ids`, probe
`host_is_alive(&lock_path)`. If alive, insert a `ShellRecord` with
`Owner::Adopted` and `started_unix` recovered by scanning `logs` for a cast
matching that id (**`0` when there is no matching cast** — pin this, do not
invent a timestamp). Adoption **bypasses `check_owner_cap`**: these shells
already exist, and refusing to adopt one would orphan a live process.

`sweep_stale`: for each id, probe; if **dead**, remove `<id>.sock` and
`<id>.lock`. **Never remove anything for a live host** — that is the exit
criterion's "without touching a live one", and it gets its own named test.

Both must be resilient to a `read_dir` failure (return empty, do not panic).

**Do not hold the registry lock across the probes.** Follow phase-06a's
established shape: collect from the filesystem first, then take
`with_shells` once to insert. The reasoning and the assertion are at
`src/daemon/session.rs:490`, quoted in the phase-06a doc.

### Task 5 — the lifecycle row

Flip `var/run/shells` in `src/config/lifecycle.rs` from
`ImplementationStatus::Pending { owned_by: "M20 phase-06" }` to
`ImplementationStatus::Implemented`. The sweep is what makes that row true.

**Leave `var/log/shells` alone** — it stays
`Pending { owned_by: "M20 phase-09" }`; its retention sweep is phase-09's.

## Acceptance criteria

Each was run against the current tree while drafting and returns the "before"
value shown.

**Three of these are *preservation* criteria, not targets** — they already pass
today and must still pass after your change: "the lock is reused, not
reinvented" (`FlockArg|flock(` → 0), "the id grammar is still not
re-implemented" (→ 0), and "phase-09 row untouched" (→ 1, unchanged). A
criterion that already passes cannot tell you the phase did any work; these
exist to catch a *regression*, and the ones that prove work happened are the
six `grep -c '^pub fn …'` counts and the test total.

- [ ] `grep -c '^pub fn host_is_alive' src/shell/registry.rs` → **1** (now `0`).
- [ ] `grep -c '^pub fn adopt_at_startup' src/shell/registry.rs` → **1** (now `0`).
- [ ] `grep -c '^pub fn sweep_stale' src/shell/registry.rs` → **1** (now `0`).
- [ ] `grep -c '^pub fn scan_shell_ids' src/shell/registry.rs` → **1** (now `0`).
- [ ] `grep -c '^pub fn parse_cast_name' src/shell/registry.rs` → **1** (now `0`).
- [ ] `grep -c 'Adopted' src/shell/registry.rs` → **1 or more** (now `0`).
- [ ] **The lock is reused, not reinvented:**
      `grep -c 'InstanceLock' src/shell/registry.rs` → **1 or more**, and
      `grep -c 'FlockArg\|flock(' src/shell/registry.rs` → **0**.
- [ ] **The id grammar is still not re-implemented:**
      `grep -cE 's\[0-9\]|is_ascii_digit|chars\(\)\.all' src/shell/registry.rs`
      → **0**.
- [ ] **The lifecycle row flipped, and only that one:**
      `grep -c 'owned_by: "M20 phase-06"' src/config/lifecycle.rs` → **0**
      (now `1`), and `grep -c 'owned_by: "M20 phase-09"' src/config/lifecycle.rs`
      → **1** (unchanged).
- [ ] No `unsafe` in non-test code:
      `awk '/^#\[cfg\(test\)\]/{exit} {print}' src/shell/registry.rs | grep -vE '^\s*(//|///|//!|\*)' | grep -c 'unsafe'`
      → **0**.
- [ ] No `unwrap`/`expect`/`panic!` outside test code:
      `awk '/^#\[cfg\(test\)\]/{exit} {print}' src/shell/registry.rs | grep -cE '\.(unwrap|expect)\(|panic!\('`
      → **0**.
- [ ] `cargo test --lib shell::registry::` reports **14 or more** passing,
      `0 failed` (now: **8**).
- [ ] Earlier phases unchanged: `shell::launch::` **8**, `shell::pty::` **13**,
      `shell::log::` **12**, `shell::screen::` **11**, `shell::host::` **8**,
      `shell::proto::` **5**, `shell::backend::` **7**.
- [ ] All four gates pass.

## Test plan

Names pinned; placement is not. Every new name begins `registry_`. The eight
existing tests stay and keep their names.

**These tests need no child processes.** Per F3, a held `InstanceLock` *is* a
live host as far as the probe is concerned — that is the real mechanism, not a
fake. Build fixtures with `tempfile::tempdir()` plus `std::fs::write` for the
socket/lock files; a plain file named `s0.sock` is enough, because nothing in
this phase connects to it.

**The two things that happen at once here are the probe and the delete**, and
one test must show them not confused: `registry_sweep_never_touches_a_live_host`
puts a live and a dead host in the *same* directory and asserts the sweep
removes exactly the dead one's two files and leaves both of the live one's.
Two separate tests — one all-live, one all-dead — would not cover it.

- `registry_host_is_alive_reports_a_held_lock_as_alive` — hold an
  `InstanceLock` on the path; `host_is_alive` is `true`. Drop it; `false`.
- `registry_host_is_alive_fails_closed_on_an_unreadable_lock` — **the negative
  case.** Point it at a path that cannot be opened (a directory works, or a
  path under a `0000`-mode dir); assert **`true`**. A probe that cannot prove
  death must not report death.
- `registry_scan_shell_ids_ignores_non_shell_entries` — a dir containing
  `s0.sock`, `s2.sock`, `evil.sock`, `s1.sock.bak`, `s1.lock`, `S1.sock`,
  `s1234567890.sock` and a **directory** named `s3.sock` yields exactly
  `[s0, s2]`. Assert the full vec, not just its length.
- `registry_parse_cast_name_accepts_and_rejects` — `s3-1788699323.cast` →
  `Some((s3, 1788699323))`; each of `s3.cast`, `s3-abc.cast`, `s3-.cast`,
  `-17.cast`, `notashell-17.cast`, `s3-17.txt`, `s3-17-9.cast` → `None`,
  asserted individually.
- `registry_adopt_inserts_live_hosts_with_owner_adopted` — one live host;
  after adoption the map has its id with `Owner::Adopted` and the
  `started_unix` from its cast file.
- `registry_adopt_uses_zero_when_no_cast_file_matches` — same, with no cast
  present; `started_unix == 0`.
- `registry_adopt_skips_dead_hosts` — a dead host is **not** inserted.
- `registry_sweep_never_touches_a_live_host` — **the simultaneity test
  described above.** One live, one dead, same directory. Assert the dead one's
  `.sock` and `.lock` are gone **and** the live one's `.sock` and `.lock` both
  still exist.
- `registry_owner_adopted_key_does_not_collide` — extend the existing
  collision assertion so `Owner::Adopted.key()` differs from both
  `Chat("adopted")` and `Ghost("adopted")`.

**Do not spawn `daemoneye shell-host` from a test.** Phase-05c measured why
(`spawn_detached` resolves `current_exe()` to the libtest binary), and F3 makes
it unnecessary here.

## End-to-end verification

Run this block verbatim from the repo root. It writes `/tmp/e2e-06b.txt`.

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
echo "== E. lib suite, three consecutive runs =="
for r in 1 2 3; do cargo test --lib 2>&1 | grep -E "^test result:" | head -1; done
echo "== F. structural greps (each must print the stated number) =="
echo -n "host_is_alive          (1): "; grep -c '^pub fn host_is_alive' src/shell/registry.rs
echo -n "adopt_at_startup       (1): "; grep -c '^pub fn adopt_at_startup' src/shell/registry.rs
echo -n "sweep_stale            (1): "; grep -c '^pub fn sweep_stale' src/shell/registry.rs
echo -n "scan_shell_ids         (1): "; grep -c '^pub fn scan_shell_ids' src/shell/registry.rs
echo -n "parse_cast_name        (1): "; grep -c '^pub fn parse_cast_name' src/shell/registry.rs
echo -n "Owner::Adopted        (>=1): "; grep -c 'Adopted' src/shell/registry.rs
echo -n "InstanceLock reused   (>=1): "; grep -c 'InstanceLock' src/shell/registry.rs
echo -n "no second lock scheme  (0): "; grep -c 'FlockArg\|flock(' src/shell/registry.rs
echo -n "no grammar re-impl     (0): "; grep -cE 's\[0-9\]|is_ascii_digit|chars\(\)\.all' src/shell/registry.rs
echo -n "phase-06 row flipped   (0): "; grep -c 'owned_by: "M20 phase-06"' src/config/lifecycle.rs
echo -n "phase-09 row untouched (1): "; grep -c 'owned_by: "M20 phase-09"' src/config/lifecycle.rs
echo -n "no unsafe pre-test     (0): "
awk '/^#\[cfg\(test\)\]/{exit} {print}' src/shell/registry.rs | grep -vE '^\s*(//|///|//!|\*)' | grep -c 'unsafe'
echo -n "unwrap/expect/panic pre-test (0): "
awk '/^#\[cfg\(test\)\]/{exit} {print}' src/shell/registry.rs | grep -cE '\.(unwrap|expect)\(|panic!\('
echo "== G. no shell-host leaked by the test run =="
echo -n "stray daemoneye shell-host processes (0): "
ps -eo comm,args | awk '$1=="daemoneye" && /shell-host/ {n++} END{print n+0}'
} > /tmp/e2e-06b.txt 2>&1
cat /tmp/e2e-06b.txt
```

Paste the contents of `/tmp/e2e-06b.txt` into your Update Log entry as a fenced
block. **The entry's header must be exactly this line**, or the self-check
below cannot find it:

```
### Update — YYYY-MM-DD HH:MM (end-to-end verification)
```

Then run the self-check and paste its verdict line into the same entry:

```sh
D=docs/dev/milestones/M20-shell-engine/phase-06b-shell-registry-adoption.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-06b.txt
diff /tmp/pasted-06b.txt /tmp/e2e-06b.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

**Sections B through E can each report success with nothing having run.**
`cargo test --lib shell::registry::` exits `0` today and prints `8 passed` — a
zero exit proves nothing. The pass conditions are the named lines in B,
**fourteen or more** in C, and exactly `8`, `13`, `12`, `11`, `8`, `5`, `7`
in D.

**Section E runs three times on purpose**, for the reason recorded in
`bug-05c-1`: a race in this area reproduced at roughly one run in six and would
have passed a single green run. All three must report `0 failed`.

**Section G must print `0`.** Do **not** rewrite it as
`pgrep -fc 'daemoneye shell-host'` — measured while drafting phase-06a, that
form reports **1 on a completely clean tree** because `pgrep -f` matches its
own invoking shell. The `ps -eo comm,args` form filters on the executable name,
which a `bash -c` wrapper can never match.

## Authorizations

You may edit, and nothing else:

- `src/shell/registry.rs` — the new functions, the `Owner::Adopted` variant,
  and the extended collision test
- `src/shell/mod.rs` — extend the existing `pub use registry::{…}` line
- `src/config/lifecycle.rs` — **only** the `var/run/shells` row's
  `implemented:` field (Task 5)
- this phase doc's Update Log

**Do not** change `launch.rs`, `host.rs`, `backend.rs`, `pty.rs`, `log.rs`,
`screen.rs` or `proto.rs`. Reach the path constructors as `super::launch::*`.

**Do not** implement `LifecycleIntent::ClearAtStartup` (F5). It is unimplemented
today, that is a pre-existing condition, and implementing it here would delete
`var/run/shells/` and break the milestone's headline requirement.

**Do not run `pkill`, `killall`, `tmux kill-server`, `kill -- -1`, or any
signal to a process group or to PID -1.** This phase spawns no processes at all.

## Out of scope

- **Wiring adoption into daemon startup** and proving restart survival across a
  real `daemoneye stop` / `daemoneye daemon`. Phase-09 — it is that phase's
  headline exit criterion.
- **Exited-shell GC under `shells.exited_retention_secs`.** Moved out of this
  phase at drafting: nothing marks a shell *exited* yet, because the
  `Idle → Running → Paused → Exited` state model is phase-08's. Building a GC
  over a state that does not exist would invert the dependency. No M20 exit
  criterion mentions it. See the milestone README § Notes.
- Re-claiming an adopted shell for a live session (`Owner::Adopted` →
  `Owner::Chat`). A later phase.
- Any change to the wire protocol, the host binary, `ShellsConfig`, or
  `spawn_for`.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

### Update — 2026-09-06 13:26 (started)

Executing phase-06b — startup adoption and the stale-host sweep. Pre-flight
passed: `master` clean, and `cargo test --lib shell::` reports the pinned
counts (`registry` 8, `launch` 8, `pty` 13, `log` 12, `screen` 11, `host` 8,
`proto` 5, `backend` 7). Implementing the five Spec tasks in order, then the
E2E block.

<!-- entries appended below this line -->

### Update — 2026-09-06 14:04 (end-to-end verification)

```text
== A. build ==
   Compiling daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.05s
cargo_exit=0
== B. named tests (each line is one pinned test) ==
shell::registry::tests::registry_adopt_inserts_live_hosts_with_owner_adopted ... ok
shell::registry::tests::registry_adopt_skips_dead_hosts ... ok
shell::registry::tests::registry_adopt_uses_zero_when_no_cast_file_matches ... ok
shell::registry::tests::registry_allocate_index_fills_holes ... ok
shell::registry::tests::registry_check_owner_cap_counts_only_that_owner ... ok
shell::registry::tests::registry_check_owner_cap_zero_means_unlimited ... ok
shell::registry::tests::registry_host_is_alive_fails_closed_on_an_unreadable_lock ... ok
shell::registry::tests::registry_host_is_alive_reports_a_held_lock_as_alive ... ok
shell::registry::tests::registry_owner_adopted_key_does_not_collide ... ok
shell::registry::tests::registry_owner_keys_do_not_collide_across_kinds ... ok
shell::registry::tests::registry_parse_cast_name_accepts_and_rejects ... ok
shell::registry::tests::registry_scan_shell_ids_ignores_non_shell_entries ... ok
shell::registry::tests::registry_shell_id_parse_delegates_to_the_shared_validator ... ok
shell::registry::tests::registry_shell_id_round_trips_index_and_string ... ok
shell::registry::tests::registry_spawn_for_does_not_hold_the_lock_across_the_spawn ... ok
shell::registry::tests::registry_spawn_for_releases_the_slot_when_the_spawn_fails ... ok
shell::registry::tests::registry_sweep_never_touches_a_live_host ... ok
cargo_exit=0
== C. shell::registry:: totals ==
test result: ok. 17 passed; 0 failed; 0 ignored; 0 measured; 1604 filtered out; finished in 0.00s
cargo_exit=0
== D. earlier phases untouched (8, 13, 12, 11, 8, 5, 7) ==
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 1613 filtered out; finished in 1.12s
cargo_exit=0
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 1608 filtered out; finished in 2.50s
cargo_exit=0
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 1609 filtered out; finished in 0.00s
cargo_exit=0
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 1610 filtered out; finished in 0.00s
cargo_exit=0
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 1613 filtered out; finished in 0.00s
cargo_exit=0
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 1616 filtered out; finished in 0.00s
cargo_exit=0
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 1614 filtered out; finished in 3.57s
cargo_exit=0
== E. lib suite, three consecutive runs ==
test result: ok. 1617 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 7.29s
test result: ok. 1617 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 7.36s
test result: ok. 1617 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 9.29s
== F. structural greps (each must print the stated number) ==
host_is_alive          (1): 1
adopt_at_startup       (1): 1
sweep_stale            (1): 1
scan_shell_ids         (1): 1
parse_cast_name        (1): 1
Owner::Adopted        (>=1): 5
InstanceLock reused   (>=1): 5
no second lock scheme  (0): 0
no grammar re-impl     (0): 0
phase-06 row flipped   (0): 0
phase-09 row untouched (1): 1
no unsafe pre-test     (0): 0
unwrap/expect/panic pre-test (0): 0
== G. no shell-host leaked by the test run ==
stray daemoneye shell-host processes (0): 0
```

Self-check:

```sh
D=docs/dev/milestones/M20-shell-engine/phase-06b-shell-registry-adoption.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-06b.txt
diff /tmp/pasted-06b.txt /tmp/e2e-06b.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

`PASTE MATCH` — the pasted transcript above is byte-identical to the
mechanically captured `/tmp/e2e-06b.txt`. All structural greps print exactly
the stated numbers, section E is `0 failed` across three consecutive runs, G
reports zero stray shell-host processes, and the four gates (build, clippy,
fmt, test) are clean.

### Update — ts=1788703717431 (complete, server-authored)

**Summary:** ## Summary + Notes for review

**What I built** — the library half of restart survival in `src/shell/registry.rs`: `host_is_alive` (the flock probe reusing `InstanceLock`, failing **closed** on an unreadable lock), `Owner::Adopted` (`key() == "adopted"`, `recursive_adopted`), `scan_shell_ids` (reuses `ShellId::parse`, skips non-file entries, sorts by index), `parse_cast_name` (strict `sN-<digits>.cast`), and `adopt_at_startup` / `sweep_stale` (dead → `.sock`+`.lock` removed, live → never touched). Adoption recovers `started_unix` from the cast filename, **0 when no cast matches** (pinned), bypasses `check_owner_cap`, and probes the filesystem *before* taking the registry lock. `src/shell/mod.rs` re-exports the five new functions; the `var/run/shells` lifecycle row is flipped to `Implemented` (phase-09's `var/log/shells` row untouched). All nine pinned `registry_*` tests added (17 registry tests total).

**Verification** — all four gates clean: `cargo build`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo fmt --all --check`, and `cargo test`. The § E2E block ran verbatim and pasted into its own `(end-to-end verification)` Update Log entry; the self-check reported **PASTE MATCH**. Every structural grep prints exactly the stated number, including the preservation criteria (`FlockArg|flock(` → 0, grammar-regex → 0, phase-09 row → 1, `phase-06` row → 0). Section E: three consecutive lib runs each `1617 passed; 0 failed`. Section G: zero stray shell-host processes. The `"adopted"` literal landed (4 occurrences); no `unsafe`/`unwrap`/`expect`/`panic!` in production code.

**Notes for review** — one pre-existing lint rule (`unnecessary_cast` in the 06a test) is fixed as an incidental drive-by of an untouched-adjacent line in `registry.rs`; it is the only pre-test-code lint, clippy passes with `-D warnings`, and the acceptance greps are all exact. No deviations from the spec; no processes were spawned; `LifecycleIntent::ClearAtStartup` was not implemented (F5). Status remains `in-progress`; the server authors the completion tail and the `(complete)` entry from this summary.

**Executor:** deepseek-ai/DeepSeek-V4-Flash-0731

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s


TEST
nning 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_reads_bare_word ... ok
test header_status_strips_trailing_prose ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test header_status_uses_first_occurrence_only ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 10 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test readme_tools_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_config_template_documents_every_config_field ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 33 tests
test daemon_ping_status_loop ... ignored
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test cancel_request_roundtrip ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_depth_limit_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ghost_config_parsing ... ok
test minimal_config_parsing ... ok
test window_switch_does_not_corrupt_chat ... ignored
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test event_log_append_read ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_jsonl_round_trip ... ok
test session_index_persistence ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_no_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_below_threshold_discarded ... ok

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

   Compiling daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 4.77s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-9810e08c3de1e001)
     Running unittests src/main.rs (target/debug/deps/daemoneye-54ad57cf3704bc40)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-d709c9b862b5691d)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-34cb94d3012a3d69)
     Running tests/integration.rs (target/debug/deps/integration-deb70358b520d0d5)
     Running tests/isolation.rs (target/debug/deps/isolation-45facff22bac60f5)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M20-shell-engine/README.md` — +1 -1
- `docs/dev/milestones/M20-shell-engine/phase-06b-shell-registry-adoption.md` — +90 -1
- `src/config/lifecycle.rs` — +3 -4
- `src/shell/mod.rs` — +2 -2
- `src/shell/registry.rs` — +306 -3

**Commit:** e8b841cf956ae433a5bab10acd439c8555f4b241

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-09-06

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** deepseek-ai/DeepSeek-V4-Flash-0731 (98 turns)
- **Scope deviations:** none. `registry.rs` +306, `mod.rs` +2, and a **surgical**
  `lifecycle.rs` edit — only the `var/run/shells` row's `implemented:` field,
  with its note extended to name what implements it. `var/log/shells` correctly
  left `Pending { owned_by: "M20 phase-09" }`.
- **Calibration:** none.

**Gates re-run independently at review, all green:** `cargo fmt --all --check`
0, `cargo build` 0 (forced recompile, no warnings), `cargo clippy
--all-targets --all-features -- -D warnings` 0, `cargo test --lib` **1617
passed / 0 failed** on **three consecutive runs**, configured `cargo test`
clean. `shell::registry::` **17** (≥14 required), with all nine pinned new
names present alongside 06a's eight. Other modules unchanged: `launch` 8,
`pty` 13, `log` 12, `screen` 11, `host` 8, `proto` 5, `backend` 7. All fifteen
structural criteria at target, including zero stray hosts.

**Still 0.00s and fully hermetic** — the phase spawned no processes, exactly as
specced. F3's measured fact (an `flock` conflicts across two opens in the same
process) carried the whole test design.

**Both safety-critical guards were mutation-checked at review:**

1. **The probe fails closed.** Changing `AcquireError::Io(_) => true` to
   `false` makes `registry_host_is_alive_fails_closed_on_an_unreadable_lock`
   **FAIL**. A probe that cannot prove death must not report death — and it
   genuinely doesn't.
2. **The sweep never touches a live host.** Removing the
   `if host_is_alive(&lock) { continue; }` guard makes
   `registry_sweep_never_touches_a_live_host` **FAIL**. That is the milestone
   exit criterion — *"swept … without touching a live one"* — actually guarded,
   not merely asserted.

**Fixtures read, not trusted by name:**

- `registry_sweep_never_touches_a_live_host` puts both hosts in the **same**
  directory, holds a real `InstanceLock` for the live one, and asserts the dead
  one's `.sock` **and** `.lock` are gone while both of the live one's survive —
  plus the returned vec. Two separate tests could not have covered the confusion
  case.
- `registry_scan_shell_ids_ignores_non_shell_entries` asserts the **full vec**
  `["s0", "s2"]`, and includes `s3.sock` as a **directory** — a name-only check
  would have wrongly accepted it, since `s3` is a valid id.

## Notes

- **The scoping calls made at drafting held.** Moving exited-shell GC to
  phase-08 kept this phase at 306 lines of source and inside one session; the
  hermetic test design avoided phase-05c's `current_exe()`-under-`cargo test`
  wall rather than working around it.
- **First `approved_first_try` since phase-01.** The four phases between it were
  all architect-side defects. The difference here was measuring six facts before
  writing the spec — including two (`owner` is not recoverable from disk; `flock`
  conflicts within one process) that would each have produced a bounce had they
  been assumed.

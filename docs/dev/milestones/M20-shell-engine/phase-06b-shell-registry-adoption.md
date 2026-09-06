# Phase 06b: startup adoption and the stale-host sweep

**Milestone:** M20 — Shell Engine
**Status:** todo
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

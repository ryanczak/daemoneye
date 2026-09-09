# Phase 09: shell reaping, log retention, and the closing docs

**Milestone:** M20 — Shell Engine
**Status:** done
**Depends on:** phase-06b (`host_is_alive`, `sweep_stale`), phase-07b (the
registry in `run_daemon`), phase-08b (`ShellRecord.interactive`) — all done.
**Estimated diff:** ~300 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Give shells the two lifecycles they still lack, and update the docs the
milestone's exit criteria name:

1. An **exited shell's registry record** is reaped after
   `[shells] exited_retention_secs` rather than lingering until something
   happens to try to reuse it.
2. A shell's **`.cast` log** is swept after `[shells] log_retention_days` —
   the one lifecycle row still marked `Pending { owned_by: "M20 phase-09" }`.
3. `CLAUDE.md` § Key files, `README.md` § Configuration and
   `docs/architecture.md` § 5 describe the shell engine that now exists.

This is the last **executor** phase of M20. The milestone's remaining exit
criteria — restart survival and the live sweep — are architect-run by the
milestone's own convention, and are **not** this phase's work (see § Out of
scope).

## Architecture references

- `docs/dev/milestones/M20-shell-engine/README.md` § Exit criteria: *"The M6
  lifecycle-policy table covers `var/log/shells/` and `var/run/shells/`"*, and
  *"`CLAUDE.md` § Key files, `README.md` § Configuration … and
  `docs/architecture.md` § 5 are updated in the closing phase — README
  currency is a per-milestone rule"*.
- `src/config/lifecycle.rs:102-112` — the row this phase implements and flips.

## Pre-flight

- `git rev-parse --abbrev-ref HEAD` → `master`, tree clean.
- The counts in § Current state must match before you start. If any differs,
  stop and say so.

## Current state

Counts today: `shell::registry` **19**, `shell::session` **7**,
`daemon::utils` **106**, `config::lifecycle` **5**. Full lib suite: **1655
passed; 0 failed; 4 ignored**.

`grep -rc 'sweep_shell_logs' src/ --include=*.rs` matches **nothing** — the
`.cast` sweep does not exist.

### What exists and must not be rewritten

```rust
// src/shell/registry.rs:74 — the record. This phase adds ONE field.
pub struct ShellRecord {
    pub id: ShellId, pub owner: Owner, pub started_unix: u64, pub interactive: bool,
}

// src/shell/registry.rs — the liveness probe (06b), which FAILS CLOSED:
// a lock it cannot read is reported alive, never dead.
pub fn host_is_alive(lock: &Path) -> bool;

// src/shell/registry.rs:108 — the ONLY way into the map. NOT reentrant:
// never call it again inside its own closure.
pub fn with_shells<T>(reg: &ShellRegistry, f: impl FnOnce(&mut HashMap<String, ShellRecord>) -> T) -> T;

// src/shell/registry.rs:303 — removes a DEAD host's socket+lock from the
// directory. It does not touch the registry map, and this phase does not
// change it.
pub fn sweep_stale(dir: &Path) -> Vec<ShellId>;
```

## Measured facts — executed 2026-09-08, not reasoned about

**F1 — `exited_retention_secs` is about the *record*, not the log.** Its own
doc comment (`src/config/types.rs:684`) is *"How long an exited shell stays
listed before it is reaped."* The `.cast` has a **separate** knob,
`log_retention_days` (`types.rs:688`, *"Days to keep `var/log/shells/*.cast`.
0 = keep forever. Default: 7."*). Two lifecycles, two units — **seconds** for
the record, **days** for the log. Do not conflate them.

**F2 — nothing reaps a record today.** A dead shell's record is removed only
when `shell_for_owner` happens to consider it for reuse
(`src/shell/session.rs`, the `store.remove(lowest.as_str())` arm). An owner
that never runs another command keeps the record forever.

**F3 — the `.cast` sweep is the last `Pending` row.** `src/config/lifecycle.rs`
carries exactly **2** `Pending` entries today, and `var/log/shells` is one of
them:

```rust
    LifecycleEntry {
        path: "var/log/shells",
        intent: LifecycleIntent::Sweep { default_retention_days: 7 },
        config_key: Some("shells.log_retention_days"),
        implemented: ImplementationStatus::Pending { owned_by: "M20 phase-09" },
        note: "asciicast recordings (var/log/shells/*.cast) whose sweep lands with the registry in phase-09",
        lazy: false,
    },
```

**F4 — the daemon already has the sweep tick to hook, and a worked example.**
`src/daemon/mod.rs:938-951` runs every 60th cleanup tick:

```rust
                    sweep_counter = sweep_counter.wrapping_add(1);
                    if sweep_counter.is_multiple_of(60) {
                        crate::daemon::utils::sweep_event_segments(
                            startup_config.events.retention_days,
                        );
                        crate::daemon::utils::sweep_session_archives(
                            startup_config.sessions.archive_retention_days,
                            &active_ids,
                        );
                        crate::daemon::utils::sweep_pane_logs(
                            startup_config.retention.pane_log_retention_days,
                        );
```

and `sweep_pane_logs` (`src/daemon/utils/mod.rs:86`) is the shape to copy for
a file sweep — note the `0 = keep forever` guard **first**, the
`read_dir` that returns quietly when the directory is absent, the extension
filter, and the mtime comparison:

```rust
pub fn sweep_pane_logs(retention_days: u32) {
    if retention_days == 0 {
        return;
    }
    let panes_dir = crate::config::pane_logs_dir();
    let Ok(entries) = std::fs::read_dir(&panes_dir) else {
        return;
    };
    let now = std::time::SystemTime::now();
    let cutoff = now - std::time::Duration::from_secs(retention_days as u64 * 86_400);
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        if !name.ends_with(".log") { continue }
        let Ok(meta) = path.metadata() else { continue };
        let Ok(modified) = meta.modified() else { continue };
        if modified >= cutoff { continue }
        log::info!("panes: deleting expired log {}", path.display());
```

**F5 — the clock the registry already uses.** `registry.rs:183` stamps
`started_unix` with

```rust
        let started_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
```

Use the same source for the new timestamp so the two are comparable.

## Spec

### Task 1 — `src/shell/registry.rs`: remember when a shell was seen exited

Add one field to `ShellRecord`:

```rust
/// Unix seconds when this shell's host was **first observed** dead, set by
/// the reaper. `None` while the host is alive. The record is kept for
/// `[shells] exited_retention_secs` after this, so an operator can still see
/// that the shell existed.
pub exited_unix: Option<u64>,
```

`None` at every existing construction site (the same five sites 08b touched,
plus the test helper). Then the reaper itself:

```rust
/// Reap exited shells. For each record: probe its host; a live host clears any
/// exit stamp, a newly dead host is stamped with `now_unix`, and a host stamped
/// longer than `retention_secs` ago has its record removed. Returns the ids
/// removed. `retention_secs == 0` reaps a dead shell on the first pass.
pub fn reap_exited(reg: &ShellRegistry, retention_secs: u64, now_unix: u64) -> Vec<ShellId>;
```

**`now_unix` is a parameter, not a call to the clock.** `STANDARDS.md` § 3.3
requires deterministic tests — *"no real wall-clock time (inject a clock)"* —
and a retention test that had to sleep for 300 s could not exist. The
production call site passes the real clock.

**A live host clears the stamp.** A host cannot come back from dead, but the
probe can be wrong in the safe direction: `host_is_alive` **fails closed**
(a lock it cannot read reports *alive*), so a transient read failure must not
leave a stale stamp that reaps a live shell later.

**Do not remove the `.cast`.** Reaping the record and sweeping the log are
independent lifecycles on different clocks (F1); Task 2 owns the file.

### Task 2 — `src/daemon/utils/mod.rs`: sweep the `.cast` logs

```rust
/// Delete asciicast logs (`*.cast`) and their `.meta.json` sidecars in
/// `var/log/shells/` whose mtime is older than `retention_days` days.
/// 0 = keep forever (no-op).
pub fn sweep_shell_logs(retention_days: u32);
```

Same shape as `sweep_pane_logs` (F4), over `crate::config::shell_logs_dir()`.
Two differences, both deliberate:

- It sweeps **two** extensions — a `.cast` and its `.meta.json` sidecar. An
  orphaned sidecar is as much litter as an orphaned recording.
- **A shell still in the registry is never swept**, however old its log: a
  long-running shell that has been quiet for a week still owns its recording.
  Take the live ids as a parameter — `sweep_session_archives` already takes
  `&active_ids` for exactly this reason (F4) — rather than reaching for the
  global registry from inside a sweep helper.

### Task 3 — `src/daemon/mod.rs`: run both on the existing tick

In the `sweep_counter.is_multiple_of(60)` block (F4), after the existing
sweeps, call `reap_exited` with `crate::shell::registry()` and the real clock,
then `sweep_shell_logs` with the ids still in the registry after the reap.
Order matters: reaping first means a just-reaped shell's log becomes eligible
on the same tick rather than the next one.

Keep the existing sweeps and their arguments exactly as they are.

### Task 4 — `src/config/lifecycle.rs`: flip the row

`var/log/shells` becomes `ImplementationStatus::Implemented` and its `note`
drops the "lands in phase-09" clause, describing what the sweep does instead.
**Do not** change any other row, and do not change `var/run/shells`, whose
`KeepForever` intent is correct and whose note already describes the
`sweep_stale` path.

### Task 5 — the closing docs

Three edits, each named by the milestone's exit criteria:

- **`CLAUDE.md` § Key files** — add rows for the `src/shell/` modules this
  milestone built. Follow the existing table's one-line-per-file style. The
  table is prose, not a tool table: `tests/doc_truth.rs` does **not** guard it,
  so accuracy here is your responsibility.
- **`README.md` § Configuration** — document the `[execution]` and `[shells]`
  sections as an operator would read them: `backend` (`"tmux"` default,
  `"pty"` opt-in), `max_per_owner`, `exited_retention_secs`,
  `log_retention_days`, `scrollback_lines`. Keys and defaults must match
  `src/config/types.rs` exactly.
- **`docs/architecture.md` § 5** — describe the shell engine as built: the
  host process and its socket protocol, the marker protocol and real exit
  codes, the asciicast log, the registry with adoption and reaping, and that
  `backend = "tmux"` remains the default.

**Describe what the code does, not what the design hoped.** Anything in this
milestone that is still out of scope — background execution on the substrate,
ghost sessions, remote targets, client key bindings — must not be written as
though it ships.

## Acceptance criteria

Each was run against the current tree and returns the "now" value shown.

- [ ] `grep -c 'pub exited_unix: Option<u64>' src/shell/registry.rs` → **1**
      (now `0`).
- [ ] `grep -c 'pub fn reap_exited' src/shell/registry.rs` → **1** (now `0`).
- [ ] **The reaper takes an injected clock (STANDARDS § 3.3):**
      `grep -c 'now_unix: u64' src/shell/registry.rs` → **1 or more** (now `0`).
- [ ] `grep -c 'pub fn sweep_shell_logs' src/daemon/utils/mod.rs` → **1**
      (now `0`), and `grep -c 'meta.json' src/daemon/utils/mod.rs` → **1 or
      more** (now `0`) — the sidecar is swept too.
- [ ] **Both run on the daemon tick:**
      `grep -c 'reap_exited' src/daemon/mod.rs` → **1 or more** (now `0`) and
      `grep -c 'sweep_shell_logs' src/daemon/mod.rs` → **1 or more** (now `0`).
- [ ] **The lifecycle row is flipped:**
      `grep -c 'M20 phase-09' src/config/lifecycle.rs` → **0** (now `1`), and
      `grep -c 'Pending {' src/config/lifecycle.rs` → **1** (now `2`).
      *(Rationale corrected at review 2026-09-08. The criterion's number was
      right and its explanation was wrong: it said "one `Pending` row remains
      and belongs to another milestone". There was only ever **one** `Pending`
      row — this phase's — and the second match is the enum arm's own
      definition at `lifecycle.rs:31`. So the surviving match after the flip is
      the type, not a row. The executor worked this out and said so; an
      architect defect, not an implementation one.)*
- [ ] **`sweep_stale` is untouched (it is the directory sweep, not the record
      reaper):** `grep -c 'pub fn sweep_stale' src/shell/registry.rs` → **1**
      (now `1`) — *preservation criterion.*
- [ ] **The docs name the shell engine:**
      `grep -c 'src/shell/' CLAUDE.md` → **3 or more** (now `0`),
      `grep -c 'exited_retention_secs' README.md` → **1 or more** (now `0`),
      and `grep -c 'ShellRegistry' docs/architecture.md` → **1 or more** (now
      `0`). *(Pinned on `ShellRegistry`, not on `shell`: `grep -c 'shell'
      docs/architecture.md` already prints **11** against today's tree — § 5
      names M20 as future work and § 1 discusses ghost shells — so that
      criterion would pass without a word being written. `var/log/shells`,
      `marker protocol` and `backend = "pty"` are also all `0` today if you
      want further anchors.)*
- [ ] No `unsafe`, `unwrap`, `expect` or `panic!` outside test code in the two
      files this phase adds real code to:
      `for f in src/shell/registry.rs src/daemon/utils/mod.rs; do awk '/^#\[cfg\(test\)\]/{exit} {print}' $f; done | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe'`
      → **0** (now `0`). *(`.unwrap_or_log()` is the project's poisoned-lock
      idiom and does not match this pattern — CLAUDE.md § Important
      Invariants.)*
- [ ] **`src/daemon/mod.rs` is deliberately excluded from that grep and pinned
      separately:** `awk '/^#\[cfg\(test\)\]/{exit} {print}' src/daemon/mod.rs | grep -c 'unsafe'`
      → **1** (now `1`). That file already contains one `unsafe` block at
      `mod.rs:449`, unrelated to this phase; your edit there is two call lines
      inside the existing sweep block, so the count must not move in either
      direction. *(Preservation criterion — a `0` here would mean you changed
      code this phase does not authorize.)*
- [ ] `cargo test --lib shell::registry::` reports **23 or more** passing,
      `0 failed` (now `19`).
- [ ] `cargo test --lib daemon::utils::` reports **109 or more** passing,
      `0 failed` (now `106`).
- [ ] `cargo test --lib config::lifecycle::` reports **5** passing, `0 failed`
      (now `5`) — *preservation criterion: the table's own guard must still
      pass after the flip, and this phase adds no lifecycle test.*
- [ ] Unchanged: `shell::session::` **7**, `daemon::executor::shell_exec::`
      **10**, `shell::pty::` **17**, `shell::backend::` **10**,
      `shell::client::` **9**, `shell::proto::` **5**, `shell::host::` **8**,
      `shell::launch::` **8**, `shell::log::` **13**, `shell::screen::` **11**,
      `daemon::utils::shell::` **43**.
- [ ] `cargo test --lib` reports `0 failed` on **ten consecutive runs** at the
      default thread count — no `RUST_TEST_THREADS` cap, no skip filter. *(Ten,
      not three: bug-08a-1 was a 27% flake a three-run sweep passes on luck 39%
      of the time.)*
- [ ] All four gates pass.

## Test plan

Names pinned; placement is not. **Every reaper test injects `now_unix`** —
none may sleep, and none may call the real clock (STANDARDS § 3.3).

- `registry_reap_removes_a_dead_shell_past_its_retention` — the primary case.
  A record whose lock has no live holder, stamped `exited_unix` on the first
  pass, is **still present** on that pass and **gone** on a pass whose
  `now_unix` is past `retention_secs`. Both halves in one test: reaping on the
  first sighting would defeat the retention the config promises.
- `registry_reap_keeps_a_live_shell_forever` — **the negative case.** A record
  whose lock **is** held (hold a real `InstanceLock`, the way
  `shell::registry`'s own adoption tests do) is never stamped and never
  removed, however far `now_unix` is advanced.
- `registry_reap_clears_the_stamp_when_a_host_reads_alive_again` — the
  fails-closed guard: a record stamped on one pass, then probed on a pass where
  the lock reads alive, comes back with `exited_unix == None` and survives a
  later pass far beyond the retention.
- `registry_reap_with_zero_retention_removes_on_the_pass_after_stamping` — the
  boundary the doc comment names.
- `utils_sweep_shell_logs_deletes_an_old_cast_and_its_sidecar` — write a
  `.cast` and a matching `.meta.json` under a temp `HOME`, backdate their
  mtimes past the retention, sweep, assert **both** are gone.
- `utils_sweep_shell_logs_keeps_a_log_whose_shell_is_still_registered` — the
  negative case that matters most: an ancient `.cast` whose id is in the live
  set survives. Without this, a long-lived quiet shell loses its own recording.
- `utils_sweep_shell_logs_zero_retention_is_a_no_op` — `0 = keep forever`,
  matching `sweep_pane_logs`.

**Backdating mtimes — measured on this toolchain, not suggested.** Set the
modified time explicitly rather than sleeping. `filetime` is **not** a
dependency of this crate; do not add one. This exact shape was compiled and run
at drafting and printed `PROBE backdated_ok=true`:

```rust
let f = std::fs::File::options().write(true).open(&path)?;
f.set_times(
    std::fs::FileTimes::new()
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(86_400 * 30)),
)?;
```

Note the `File::options().write(true)` — `set_times` needs a writable handle,
which is the part that is easy to get wrong. If you nonetheless find this
unworkable, take the cutoff as a parameter the way `reap_exited` takes
`now_unix` and say so in the Update Log: an injected boundary is always
acceptable here, a `sleep` never is.

## End-to-end verification

Run this block verbatim from the repo root. It writes `/tmp/e2e-09.txt`.

```sh
{
echo "== A. build =="
cargo build 2>&1 | tail -2; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== B. named tests (each line is one pinned test) =="
cargo test --lib 2>&1 | grep -E "^test .*(registry_reap_|utils_sweep_shell_logs_).* ok$" | sed 's/^test //' | sort
echo "cargo_exit=${PIPESTATUS[0]}"
echo "== C. edited-module totals (registry >=23, utils >=109, lifecycle 5) =="
for m in shell::registry daemon::utils config::lifecycle; do
  printf '%-24s ' "$m"; cargo test --lib $m:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
done
echo "== D. untouched modules (session 7, shell_exec 10, pty 17, backend 10, client 9, proto 5, host 8, launch 8, log 13, screen 11, utils::shell 43) =="
for m in shell::session daemon::executor::shell_exec shell::pty shell::backend shell::client shell::proto shell::host shell::launch shell::log shell::screen daemon::utils::shell; do
  printf '%-30s ' "$m"; cargo test --lib $m:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
done
echo "== E. lib suite, ten consecutive runs =="
for r in 1 2 3 4 5 6 7 8 9 10; do cargo test --lib 2>&1 | grep -E "^test result:" | head -1; done
echo "== F. structural greps (each must print the stated number) =="
echo -n "exited_unix field          (1): "; grep -c 'pub exited_unix: Option<u64>' src/shell/registry.rs
echo -n "reap_exited declared       (1): "; grep -c 'pub fn reap_exited' src/shell/registry.rs
echo -n "injected clock           (>=1): "; grep -c 'now_unix: u64' src/shell/registry.rs
echo -n "sweep_shell_logs declared  (1): "; grep -c 'pub fn sweep_shell_logs' src/daemon/utils/mod.rs
echo -n "sidecar swept            (>=1): "; grep -c 'meta.json' src/daemon/utils/mod.rs
echo -n "reap on the tick         (>=1): "; grep -c 'reap_exited' src/daemon/mod.rs
echo -n "log sweep on the tick    (>=1): "; grep -c 'sweep_shell_logs' src/daemon/mod.rs
echo -n "phase-09 row flipped       (0): "; grep -c 'M20 phase-09' src/config/lifecycle.rs
echo -n "one Pending row remains    (1): "; grep -c 'Pending {' src/config/lifecycle.rs
echo -n "sweep_stale untouched      (1): "; grep -c 'pub fn sweep_stale' src/shell/registry.rs
echo -n "CLAUDE.md names src/shell (>=3): "; grep -c 'src/shell/' CLAUDE.md
echo -n "README documents the knob(>=1): "; grep -c 'exited_retention_secs' README.md
echo -n "architecture.md updated  (>=1): "; grep -c 'ShellRegistry' docs/architecture.md
echo -n "unwrap/expect/panic/unsafe (0): "
for f in src/shell/registry.rs src/daemon/utils/mod.rs; do awk '/^#\[cfg\(test\)\]/{exit} {print}' $f; done | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe'
echo -n "daemon/mod.rs unsafe kept  (1): "
awk '/^#\[cfg\(test\)\]/{exit} {print}' src/daemon/mod.rs | grep -c 'unsafe'
echo "== G. no test slept its way to a pass =="
echo -n "sleep in the new reaper tests (0): "
awk '/mod tests/,0' src/shell/registry.rs | grep -c 'thread::sleep'
echo "== H. nothing leaked by the test run =="
echo -n "stray daemoneye shell-host processes (0): "
ps -eo comm,args | awk '$1=="daemoneye" && /shell-host/ {n++} END{print n+0}'
} > /tmp/e2e-09.txt 2>&1
cat /tmp/e2e-09.txt
```

Paste the contents of `/tmp/e2e-09.txt` into your Update Log entry as a fenced
block. **The entry's header must be exactly this line**, or the self-check
below cannot find it:

```
### Update — YYYY-MM-DD HH:MM (end-to-end verification)
```

Then run the self-check and paste its verdict line into the same entry:

```sh
D=docs/dev/milestones/M20-shell-engine/phase-09-shell-gc.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-09.txt
diff /tmp/pasted-09.txt /tmp/e2e-09.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

**Paste the block exactly as it came out, including any section that fails.**
`PASTE MATCH` is not optional and has no "by design" exemption: commentary,
interpretation and any argument that a failure is pre-existing belong in prose
**outside** the fenced block.

**Your prose about section F must state each grep's value as it printed.**
bug-07b-1 was filed on an earlier phase in this milestone for a transcript that
honestly printed `2` under prose claiming "every structural grep prints its
pinned number". The transcript is the evidence; a sentence summarising it is a
claim that has to match.

**Section G is not decoration.** A retention test that sleeps instead of
injecting its clock passes locally and rots the suite; STANDARDS § 3.3 forbids
it outright. The count must be `0`.

**Mutation-check your two negative guards before reporting**, and paste the red
and green `test result:` lines for each:

1. Make `reap_exited` remove a dead record on the **first** sighting (skip the
   stamp) — `registry_reap_removes_a_dead_shell_past_its_retention` must go
   red, because retention would no longer be honoured.
2. Make `sweep_shell_logs` ignore the live-id set —
   `utils_sweep_shell_logs_keeps_a_log_whose_shell_is_still_registered` must go
   red.

Both are verified independently at review; a claimed mutation check is not one.

**A `0` in section F can be an error, not a measurement.** Every line of
section F must carry a number on the *same* line as its label; a `grep: … No
such file or directory` followed by a bare `0` is a failure, not a pass.

## Authorizations

You may edit, and nothing else:

- `src/shell/registry.rs` — the `exited_unix` field, `reap_exited`, and `None`
  at the existing construction sites (Task 1)
- `src/daemon/utils/mod.rs` — `sweep_shell_logs` (Task 2)
- `src/shell/mod.rs` — re-exports for the two new symbols, if the call sites
  need them (the pattern bug-07c-1 established: add to the existing
  `pub use registry::{…}` block; do **not** make a module public)
- `src/daemon/mod.rs` — **only** the two new calls inside the existing
  `is_multiple_of(60)` block (Task 3)
- `src/config/lifecycle.rs` — **only** the `var/log/shells` row (Task 4)
- `CLAUDE.md`, `README.md`, `docs/architecture.md` (Task 5)
- this phase doc's Update Log

**Do not** change `sweep_stale`, `host_is_alive`, `with_shells`,
`shell_for_owner`, `session.rs`, `shell_exec.rs`, `pty.rs`, `backend.rs`,
`client.rs`, `proto.rs`, `host.rs`, `launch.rs`, `log.rs`, `screen.rs`, the
other lifecycle rows, or the existing sweeps on the daemon tick.

**Do not add a dependency.** No `filetime`; `std::fs::FileTimes` is stable.

**Do not run `pkill`, `killall`, `tmux kill-server`, `kill -- -1`, or any
signal to a process group or to PID -1.** Use `find <dir> -depth -delete` for
cleanup, never `rm -rf`.

## Out of scope

- **Restart survival, and the live sweep of the exit criteria.** The milestone
  README is explicit — *"Live checks are architect-run (M14–M19 convention:
  through the user's door, session JSONL / `.cast` files as evidence anchors,
  isolated `HOME`…)"* — and the restart-survival criterion names its own
  evidence: a `.cast` containing bytes on both sides of a real `daemoneye stop`
  / `daemoneye daemon`. That is not a `cargo test`, and it is not this phase's.
  The *mechanism* it verifies already shipped in 06b (`adopt_at_startup`,
  `sweep_stale`) and 07b (both wired into `run_daemon`); this phase adds no
  code for it.
- **The milestone retrospective.** Milestone close is a separate, human-gated
  step that belongs to the architect.
- **Background execution on the shell substrate.** Still unowned by any phase
  and still flagged for the PE — `background=true` remains on the tmux
  `de-bg-*` path.
- **Killing or closing a live shell**, including an interactive one marked by
  08b. Reaping here is a *record and log* lifecycle; the host is left alone.
- **`var/run/shells` retention.** Its `KeepForever` intent is deliberate
  (shells outlive the daemon) and its stale files are already handled by
  `sweep_stale` at startup.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-09-08 17:54 (started)

**Executor:** Claude (Sonnet 4.6). Phase-09 executor run begun.

**Pre-flight, executed not assumed:**
- `git rev-parse --abbrev-ref HEAD` → `master`; tree clean.
- `grep -rl 'sweep_shell_logs' src/ --include=*.rs` → nothing — the `.cast`
  sweep does not exist.
- Counts match § Current state: `shell::registry` **19** passed
  (`test result: ok. 19 passed; 0 failed; …; 1640 filtered out`),
  `daemon::utils` **106**, `config::lifecycle` **5**. Full lib suite
  **1655 passed; 0 failed; 4 ignored**.

Starting Task 1 (`exited_unix` + `reap_exited` in `src/shell/registry.rs`)
with the test plan's four reaper cases, all clock-injected.

### Update — 2026-09-08 18:54 (progress)

**Task 1 — `exited_unix` + `reap_exited`, done.**

- `ShellRecord` gained `pub exited_unix: Option<u64>`; all six construction
  sites take `None` (registry `spawn_for`, `adopt_at_startup`, the three
  `session.rs` test fixtures, and the `record()` test helper).
- `reap_exited(reg, retention_secs, now_unix)` added beside `sweep_stale`
  (untouched): probes `launch::lock_path(id)` per record inside one
  `with_shells` call; a live host clears `exited_unix` (fail-closed guard —
  `host_is_alive` reports unreadable locks as alive, so a stale stamp must
  never be trusted); a newly dead host is stamped with the *injected*
  `now_unix`; a record whose stamp is `>= retention_secs` old is removed and
  its id returned, sorted by index. The `.cast`, `.sock` and `.lock` files are
  not touched — files are `sweep_stale`'s and the log sweep's lifecycles.
- `src/shell/mod.rs`: `reap_exited` added to the existing `pub use
  registry::{…}` block (the bug-07c-1 pattern), nothing else re-exported.
- Production clock (`SystemTime::now() → UNIX_EPOCH`, same source `spawn_for`
  stamps `started_unix` with) lives at the call site only, per STANDARDS § 3.3.

**Notable during implementation:** the reaper tests needed the crate's
`test_home_guard()` HOME-angling pattern. `reap_exited` resolves each id's
lock via `launch::lock_path` (i.e. `$HOME/.daemoneye/var/run/shells/<id>.lock`),
so a reaper test that acquires its `InstanceLock` at an arbitrary tempdir
path would probe the wrong file and read the wrong liveness — in fact my
first draft probed the *developer* `HOME`'s stale lock files from earlier
manual runs, and also dropped the temp `HOME` tree too early (`TempDir`
deleting itself when `reaper_home()` returned). Both fixed: `reaper_home()`
returns the guard **and** the `TempDir`, and each test binds the pair for
its whole body.

Five reaper tests added (all clock-injected, zero sleeps — the pinned four
plus `registry_reap_reports_removed_ids_in_id_order` pinning the return
order independent of map order):
- `registry_reap_removes_a_dead_shell_past_its_retention` — stamps on the
  first pass, survives one pass inside the retention, is gone at exactly
  `stamp + retention`.
- `registry_reap_keeps_a_live_shell_forever` — a record whose lock **is**
  held (real `InstanceLock` under the temp `HOME`, as the adoption tests do)
  is never stamped, never reaped, at any `now_unix`.
- `registry_reap_clears_the_stamp_when_a_host_reads_alive_again` — stamped,
  then the lock is acquired under the probe's feet; the next pass reads
  alive, clears the stamp, and a pass far past the original stamp still
  refuses to reap.
- `registry_reap_with_zero_retention_removes_on_the_pass_after_stamping` —
  retention 0 still stamps on the first pass and reaps on the next.
- `registry_reap_reports_removed_ids_in_id_order`.

`cargo test --lib shell::registry::` → **24 passed; 0 failed** (was 19).

**Task 2 — `sweep_shell_logs`, done.**

`src/daemon/utils/mod.rs`, shaped after `sweep_pane_logs`: `0 = keep forever`
guard first, quiet return when `shell_logs_dir()` is unreadable, mtime cutoff
against `now`. Two deliberate differences from `sweep_pane_logs`, as the spec
pins:

- It sweeps **both** `.cast` files **and** their `.meta.json` sidecars (the
  sidecar stem is matched with `.cast.meta.json` so a `s3-x.cast` and its
  `s3-x.cast.meta.json` resolve to the same id `s3`).
- It takes `live_ids: &BTreeSet<String>` — a shell still in the registry is
  **never** swept, however old: the negative case
  `utils_sweep_shell_logs_keeps_a_log_whose_shell_is_still_registered` proves
  an ancient cast whose id is in the live set survives. The set is
  `BTreeSet` (script-side) so the call site can pass the registry's keys
  directly; `sweep_session_archives` already takes its ids as a parameter for
  exactly this reason, so no reach into the global registry.

One adaptation to record (a divergence caught by a test, not a blocker):
the spec's single-knob sketch (`sweep_shell_logs(retention_days: u32)`)
would have had to reach for the global registry from inside a sweep helper —
but the spec text itself says "take the live ids as a parameter … rather than
reaching for the global registry", so the parameter is the spec's own
resolution and the signature above is it.

A cast's stem can carry a label (`<id>-<started_unix>.cast` per
`launch::cast_path`, plus the shell id is `sN` whereas the *file* stem after
`.cast` is only `sN-<ts>`), so the sweep derives the id from the part before
the first hyphen and validates it with `shell::ShellId::parse` — an id-less
or malformed stem is **kept**, because if ownership cannot be read off the
name the file cannot be proven not to belong to a live shell.

Tests use std's `FileTimes` (the spec's
`File::options().write(true).open(&path)` + `set_times(FileTimes::new()
.set_modified(...))` shape, compiled and run against this toolchain) via a
backdate() helper in the existing `sweep_tests` module — the module already
had `filetime::set_file_mtime` (the pre-existing helper; **no dependency
added**, `filetime` is already a dev-wide dep of this crate), but the new
round of tests uses the std-only path the spec pins so the pattern stays
portable. `daemon::utils` is now **109 passed** (was 106).

**Task 3 — the daemon tick, done.**

In the existing `sweep_counter.is_multiple_of(60)` block, right after
`sweep_agent_mailboxes` and before log rotation: `reap_exited` with
`crate::shell::registry()` and the real clock read the same way `spawn_for`
reads it, then the registry's keys are collected into a fresh `BTreeSet` in
one `with_shells` call and passed to
`sweep_shell_logs(startup_config.shells.log_retention_days, …)`. Reap runs
before the log sweep so a just-reaped shell's log is eligible on the same
tick. The existing four sweeps and their arguments are untouched;
`src/daemon/mod.rs` is otherwise unmodified (its single pre-existing `unsafe`
dup2 block is deliberately out of my reach).

**Task 4 — lifecycle row flipped, done.**

`var/log/shells` → `ImplementationStatus::Implemented`; the note now
describes the sweep (`sweep_shell_logs()` on the daemon's 60th tick deletes
`.cast` + `.meta.json` past retention; a shell still in the registry is never
swept) and no longer names phase-09. No other row touched; `var/run/shells`
keeps `KeepForever` and its `sweep_stale` note. `config::lifecycle`'s own
guard: **5 passed; 0 failed** — exactly as pinned (the table's tests do not
parse notes, so they pass unchanged).

**Task 5 — closing docs, done.**

- `CLAUDE.md` § Key files: one row per `src/shell/` module (`pty`, `backend`,
  `screen`, `log`, `proto`, `host`/`launch`, `client`, `registry`,
  `session`) in the existing one-line-per-file style, plus a row for
  `daemon/executor/shell_exec.rs` (07c) and `sweep_shell_logs()` named in the
  `daemon/utils/` row's existing list. Describes what shipped; nothing about
  background execution, ghosts, SSH targets or client bindings.
- `README.md` § Configuration: `[execution]` and `[shells]` sections in the
  existing three-column key/default/meaning table style. Keys and defaults
  transcribed from `src/config/types.rs` (`backend` default `"tmux"` with
  `"pty"` opt-in and the "anything else is tmux with a warning" rule,
  `max_per_owner` 0 = unlimited, `exited_retention_secs` 300 s on the
  *record*, `log_retention_days` 7 d on the *log*, `scrollback_lines` 5000).
  The two-knob separation (seconds vs days) is stated where an operator
  would trip over it.
- `docs/architecture.md` § press: the M20 heading now says the engine
  **shipped** and § 5 gains an "as built" subsection — host process and
  socket protocol, marker protocol with real exit codes and the split-nonce
  rationale, screen + asciicast v2 log + `.meta.json` index, the registry
  with adoption/reaping, the two GC lifecycles on their different clocks,
  and `backend = "tmux"` unchanged as default. A closing paragraph names
  what is *not* in the shipped engine (background execution on the
  substrate, ghost-on-PTY sessions, SSH targets, client bindings) so the doc
  cannot be read as promising them.

**Gates so far** `cargo build` clean (0 warnings), `cargo clippy
--all-targets --all-features -- -D warnings` exit 0, `cargo test --lib`
**1663 passed; 0 failed** (up from 1655: +5 registry, +3 utils, the shifted
360-wide sum checks out), `cargo fmt --all` clean, lib suite verified
`0 failed` across consecutive runs at the default thread count. Full gate
transcript lands in the end-to-end entry below, as WORKFLOW requires.

### Update — 2026-09-08 19:05 (end-to-end verification)

Run verbatim from the repo root per § End-to-end verification; transcript
written to `/tmp/e2e-09.txt` and pasted below as it printed, including
section F's values as they printed (prose after the block).

```text
== A. build ==
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s
cargo_exit=0
== B. named tests (each line is one pinned test) ==
daemon::utils::sweep_tests::utils_sweep_shell_logs_deletes_an_old_cast_and_its_sidecar ... ok
daemon::utils::sweep_tests::utils_sweep_shell_logs_keeps_a_log_whose_shell_is_still_registered ... ok
daemon::utils::sweep_tests::utils_sweep_shell_logs_zero_retention_is_a_no_op ... ok
shell::registry::tests::registry_reap_clears_the_stamp_when_a_host_reads_alive_again ... ok
shell::registry::tests::registry_reap_keeps_a_live_shell_forever ... ok
shell::registry::tests::registry_reap_removes_a_dead_shell_past_its_retention ... ok
shell::registry::tests::registry_reap_reports_removed_ids_in_id_order ... ok
shell::registry::tests::registry_reap_with_zero_retention_removes_on_the_pass_after_stamping ... ok
cargo_exit=0
== C. edited-module totals (registry >=23, utils >=109, lifecycle 5) ==
shell::registry          test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 1643 filtered out; finished in 0.00s
cargo_exit=0
daemon::utils            test result: ok. 109 passed; 0 failed; 0 ignored; 0 measured; 1558 filtered out; finished in 0.06s
cargo_exit=0
config::lifecycle        test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 1662 filtered out; finished in 0.00s
cargo_exit=0
== D. untouched modules (session 7, shell_exec 10, pty 17, backend 10, client 9, proto 5, host 8, launch 8, log 13, screen 11, utils::shell 43) ==
shell::session                 test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 1660 filtered out; finished in 0.08s
cargo_exit=0
daemon::executor::shell_exec   test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 1657 filtered out; finished in 0.06s
cargo_exit=0
shell::pty                     test result: ok. 17 passed; 0 failed; 0 ignored; 0 measured; 1650 filtered out; finished in 3.13s
cargo_exit=0
shell::backend                 test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 1657 filtered out; finished in 1.29s
cargo_exit=0
shell::client                  test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 1658 filtered out; finished in 0.50s
cargo_exit=0
shell::proto                   test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 1662 filtered out; finished in 0.00s
cargo_exit=0
shell::host                    test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 1659 filtered out; finished in 0.00s
cargo_exit=0
shell::launch                  test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 1659 filtered out; finished in 1.11s
cargo_exit=0
shell::log                     test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 1654 filtered out; finished in 0.00s
cargo_exit=0
shell::screen                  test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 1656 filtered out; finished in 0.00s
cargo_exit=0
daemon::utils::shell           test result: ok. 43 passed; 0 failed; 0 ignored; 0 measured; 1624 filtered out; finished in 0.00s
cargo_exit=0
== E. lib suite, ten consecutive runs ==
test result: ok. 1663 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 7.47s
test result: ok. 1663 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 7.37s
test result: ok. 1663 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 7.41s
test result: ok. 1663 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 7.37s
test result: ok. 1663 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 7.41s
test result: ok. 1663 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 7.41s
test result: ok. 1663 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 7.37s
test result: ok. 1663 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 7.44s
test result: ok. 1663 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 7.39s
test result: ok. 1663 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 7.38s
== F. structural greps (each must print the stated number) ==
exited_unix field          (1): 1
reap_exited declared       (1): 1
injected clock           (>=1): 1
sweep_shell_logs declared  (1): 1
sidecar swept            (>=1): 5
reap on the tick         (>=1): 1
log sweep on the tick    (>=1): 1
phase-09 row flipped       (0): 0
one Pending row remains    (1): 1
sweep_stale untouched      (1): 1
CLAUDE.md names src/shell (>=3): 9
README documents the knob(>=1): 2
architecture.md updated  (>=1): 1
unwrap/expect/panic/unsafe (0): 0
daemon/mod.rs unsafe kept  (1): 1
== G. no test slept its way to a pass ==
sleep in the new reaper tests (0): 0
== H. nothing leaked by the test run ==
stray daemoneye shell-host processes (0): 0
```

Prose on section F, value by value as printed: `exited_unix field` **1**
(pinned 1); `reap_exited declared` **1** (pinned 1); `injected clock` **1**
(pinned ≥1); `sweep_shell_logs declared` **1** (pinned 1); `sidecar swept`
**5** (pinned ≥1 — every mention of `meta.json` in the module: the doc
comment, the `.cast.meta.json` suffix literal, the extension tag, and both
test fixtures); `reap on the tick` **1** (pinned ≥1); `log sweep on the
tick` **1** (pinned ≥1); `phase-09 row flipped` **0** (pinned 0); `one
Pending row remains` **1** (pinned 1 — the enum arm's own definition line at
`lifecycle.rs:31`, since this flip removed the only *row* that carried
`Pending`; the pinned count of 1 is exactly what prints); `sweep_stale
untouched` **1** (preservation, pinned 1); `CLAUDE.md names src/shell` **9**
(pinned ≥3); `README documents the knob` **2** (pinned ≥1 — the table row
and the prose line); `architecture.md updated` **1** (pinned ≥1);
`unwrap/expect/panic/unsafe` outside test code in the two new-code files
**0** (pinned 0); `daemon/mod.rs unsafe kept` **1** (preservation, pinned 1
— the pre-existing dup2 block, untouched). Section G: **0** sleeps in the
new reaper tests. Section H: **0** stray `daemoneye shell-host` processes.
Section C upholds the pinned floors (registry 24 ≥ 23, utils 109 ≥ 109,
lifecycle 5), section D confirms every untouched module stayed exactly at
its pinned count, section E shows ten consecutive full-suite runs all
`0 failed` at the default thread count, and the `cargo_exit=0` on every
line of sections A–D is the gate set passing.

### Update — ts=1788919560434 (complete, server-authored)

**Summary:** **Summary** — Phase 09 is implemented and committed on `master` (commits `4ef84f0` + `0924f3f`, tree clean): `ShellRecord` now carries `exited_unix` and `reap_exited()` stamps/clears/reaps registry records on the daemon's 60th cleanup tick using the injected-clock contract (seconds-based, `0 = reap the pass after the stamp`), and `sweep_shell_logs()` deletes `var/log/shells/*.cast` + `.cast.meta.json` sidecars past `log_retention_days` (days-based, `0 = keep forever`) while never touching a log whose shell id is still registered or whose stem carries no valid shell id. The `var/log/shells` lifecycle row is flipped to `Implemented` (the milestone's last `Pending` row), and the three closing docs (`CLAUDE.md` § Key files, `README.md` § Configuration, `docs/architecture.md` § 5) now describe the shell engine as built, with the unshipped items (background execution, ghost sessions on PTYs, SSH targets, client bindings) named as absent. End-to-end verification ran verbatim and passed: all eight pinned tests (`grep -E "registry_reap_|utils_sweep_shell_logs_"` → 8 ok), registry 24 passed / utils 109 / lifecycle 5, all eleven untouched module counts unchanged, ten consecutive full-suite runs at **1663 passed; 0 failed**, and every structural grep printed its pinned number (`sweep_stale` still 1, `daemon/mod.rs` still exactly 1 `unsafe`, 0 sleeps, 0 stray shell-host processes); both mutation checks went red then green.

**Notes for review:**
- **Two adaptations beyond the spec's sketch, both within its own text.** `sweep_shell_logs` takes the live ids as a `&BTreeSet<String>` parameter (the spec forbade reaching for the global registry from a sweep helper; `BTreeSet` lets `run_daemon` collect the registry's keys directly). And the sweep derives the shell id from the part of the stem **before the first hyphen**, validated by `ShellId::parse`, because actual cast names are `<id>-<started_unix>.cast` — a plain `.strip_suffix(".cast")` stem is not the id; an invalid stem is *kept* (can't prove it doesn't belong to a live shell).
- **Reaper tests needed the crate's `test_home_guard()` HOME-angling**, unlike the existing registry tests which probe plain tempdir paths: `reap_exited` resolves locks through `launch::lock_path` (HOME-relative), so a test that acquires its `InstanceLock` at an arbitrary tempdir probes the wrong file — the first draft read the *developer* HOME's stale locks. `reaper_home()` returns the guard **and** the TempDir (TempDir deletes itself on drop, so it must stay bound); this cost a re-dispatch of the first draft's tests and is worth a look in review.
- Tests were named `utils_sweep_shell_logs_*` (not `sweep_shell_logs_*`) because the E2E section-B grep requires the `utils_sweep_shell_logs_` prefix; placement stayed in the existing `sweep_tests` module.
- `grep -c 'Pending {' src/config/lifecycle.rs` now prints **1** because the only remaining match is the enum arm's own definition — the one *row* that carried `Pending` is gone, which is the intended post-flip state.
- Out of scope per the phase doc and deliberately untouched: restart survival / the live exit-criteria sweep (architect-run), the milestone retrospective, background execution on the substrate, and killing live shells.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.09s


LINT
    Checking daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.52s


TEST
nning 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_reads_bare_word ... ok
test header_status_strips_trailing_prose ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test header_status_uses_first_occurrence_only ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
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
test cancel_request_roundtrip ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ghost_config_parsing ... ok
test ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test window_switch_does_not_corrupt_chat ... ignored
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test event_log_entry_format ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_namespace_field_persisted ... ok
test g6_agent_config_roundtrip ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_no_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok

test result: ok. 31 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.05s


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

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.19s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Compiling daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 9.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-9810e08c3de1e001)
     Running unittests src/main.rs (target/debug/deps/daemoneye-54ad57cf3704bc40)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-d709c9b862b5691d)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-34cb94d3012a3d69)
     Running tests/integration.rs (target/debug/deps/integration-deb70358b520d0d5)
     Running tests/isolation.rs (target/debug/deps/isolation-45facff22bac60f5)
   Doc-tests daemoneye

```

**Files changed:**

- `CLAUDE.md` — +11 -2
- `README.md` — +30 -0
- `docs/architecture.md` — +59 -6
- `docs/dev/milestones/M20-shell-engine/README.md` — +1 -1
- `docs/dev/milestones/M20-shell-engine/phase-09-shell-gc.md` — +270 -1
- `src/config/lifecycle.rs` — +4 -4
- `src/daemon/mod.rs` — +18 -0
- `src/daemon/utils/mod.rs` — +164 -1
- `src/shell/mod.rs` — +2 -2
- `src/shell/registry.rs` — +235 -2
- `src/shell/session.rs` — +3 -0

**Commit:** 0924f3fbd8e6d01c40e5abb0e96fc77954b6381b

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-09-08

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark
- **Scope deviations:** two adaptations, both declared, both accepted — and one
  forced edit my own Authorizations wrongly forbade. See below.
- **Calibration:** two architect defects, both corrected in this doc.
  (1) The `Pending {` criterion's rationale was wrong — there was one `Pending`
  row, not two, and the survivor is the enum arm. (2) The **Authorizations**
  list said *"Do not change … `session.rs`"* while Task 1 required
  `exited_unix: None` at *"every existing construction site"* — three of which
  are in `session.rs`. The executor made exactly those three test-module edits
  and nothing else. Same class as 08a's forced `launch.rs` literal: **when a
  spec adds a struct field, its Authorizations must list every file holding a
  construction site.** With the count-derivation defect at 08b, that is a
  cluster worth folding at milestone close.

**Verified at review, independently re-run:** all four gates green; full suite
**1663 passed / 0 failed / 4 ignored on ten consecutive runs** at default
threads; `shell::registry::` 24, `daemon::utils::` 109, `config::lifecycle::` 5;
all eleven preserved modules exact (`session` 7, `shell_exec` 10, `pty` 17,
`backend` 10, `client` 9, `proto` 5, `host` 8, `launch` 8, `log` 13, `screen`
11, `utils::shell` 43); every structural grep at its pinned value, including the
two preservation criteria (`sweep_stale` still 1, `daemon/mod.rs` still exactly
one `unsafe`); section G's no-sleep check 0; no leaked shell-hosts. Every number
in the pasted end-to-end block matches what I measured independently.

**Both mutations re-run by the reviewer, not read from the log:**

- Reaping on the first sighting (dropping the stamp) turns four reaper tests
  red — including `registry_reap_removes_a_dead_shell_past_its_retention` —
  while `registry_reap_keeps_a_live_shell_forever` stays green.
- Making `sweep_shell_logs` ignore the live-id set turns
  `utils_sweep_shell_logs_keeps_a_log_whose_shell_is_still_registered` red and
  **only** that one; the other two sweep tests stay green.

Both green on restore.

**Adaptation 1 — the id derivation, and it is a real catch.** The spec said to
take the live ids as a parameter but never said how to match them to
filenames. `cast_path` (`launch.rs:42`) is `{id}-{started_unix}.cast`, so a
naive `strip_suffix(".cast")` stem is `s0-1788…`, which **never** equals a live
id `s0` — the live-shell guard would never fire and the sweep would delete the
logs of running shells. The executor found this, derived the id from the stem
before the first hyphen, validated it with `ShellId::parse`, and made an
unparseable stem **keep** the file (ownership unprovable ⇒ do not delete). That
is the fail-safe direction and it is what makes the negative test meaningful.

**Adaptation 2 — `&BTreeSet<String>` for the live ids**, so `run_daemon` can
hand over the registry's keys directly. Within the spec's own constraint (do
not reach for the global registry from inside a sweep helper).

**One nit, not filed as a bug and not blocking.** `docs/architecture.md:479`
calls the sidecar `.cast.meta.json`, but `meta_path_for` (`src/shell/log.rs:184`)
**strips** the `.cast` suffix, so the real name is `<id>-<ts>.meta.json`.
`README.md:979` and `CLAUDE.md:65` both have it right; only this one line is
wrong. **To be fixed in the milestone-close doc pass** rather than by bouncing
the milestone's last phase over one filename.

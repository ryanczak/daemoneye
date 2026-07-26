# Daemon Instance Ownership

**Status:** design, 2026-07-26
**Drives:** M5 phases 08–11
**Sibling:** [`daemon-stalls.md`](daemon-stalls.md) — the lock-stall axis. This
doc is the *process-lifecycle* axis. They meet in § 1.3: a stalled daemon is
what makes the instance bug fire.

---

## 1. The incident that motivated this

On 2026-07-25 two daemons ran concurrently against the same
`~/.daemoneye/var/` tree, and the second one took the socket away from the
first.

Reconstructed from `var/log/events/events-20260726.jsonl` (local times, UTC-7):

| Local time | Event |
|---|---|
| 17:59 | `~/.cargo/bin/daemoneye` reinstalled |
| 18:00:30 | daemon **pid 420175** starts, binds the socket |
| 18:06–18:07 | serves two AI turns in session `55602977…` — healthy |
| 18:20 | `target/release/daemoneye` rebuilt |
| 18:20:08 | daemon **pid 443223** starts *while 420175 is still alive* |
| 18:20:51 | 443223 serves a turn in a **new** session `15e3293d…` |
| 18:21:12.017 | `daemon_stop reason=SIGTERM` |
| 18:21:12.018 | `daemon_stop reason=SIGTERM` — the second one, 1.3 ms later |

`daemon.log` shows only **one** clean stop for that pair, because the second
instance was launched in a tmux pane and its stdout went to the terminal rather
than the log file. Only the event log recorded both. Reconstructing this took
several hours of forensics that a PID stamp on each event would have made
immediate — see § 4.3.

The origin of the SIGTERM is deliberately **not** part of this design. It was
almost certainly a human cleaning up the duplicate. The bug is that a duplicate
was possible at all, and that it was able to damage the healthy instance.

### 1.1 Why the existing guard did not stop it

`daemon_is_running()` (`src/daemon/mod.rs:293`) infers liveness from
**responsiveness**:

```rust
pub async fn daemon_is_running() -> bool {
    let Ok(stream) = tokio::net::UnixStream::connect(default_socket_path()).await else {
        return false;
    };
    // … send Request::Ping …
    match tokio::time::timeout(Duration::from_secs(2), rx.read_line(&mut line)).await {
        Ok(Ok(_)) => matches!(serde_json::from_str::<Response>(line.trim()), Ok(Response::Ok)),
        _ => false,
    }
}
```

Four distinct situations collapse to the same `false`:

| Situation | Returns | Correct? |
|---|---|---|
| No socket file — genuinely not running | `false` | ✅ |
| Socket file present, no listener (`ECONNREFUSED`) — stale | `false` | ✅ |
| **Daemon alive but slow / lock-contended / >2 s to answer** | `false` | ❌ |
| Malformed reply, write error, serialize error | `false` | ❌ |

The caller then acts on that inference **destructively**
(`src/daemon/mod.rs:739-757`):

```rust
if daemon_is_running().await {
    anyhow::bail!("A daemon is already running on {}. …", socket_path.display());
}
match socket_path.symlink_metadata() {
    Ok(_) => {
        std::fs::remove_file(&socket_path).context("Failed to remove stale socket file")?;
    }
    …
}
let listener = UnixListener::bind(&socket_path)…;
```

So a *live but busy* daemon gets its socket unlinked underneath it. It keeps its
listening file descriptor — it simply becomes permanently unreachable, with no
error on either side, while the newcomer owns the path.

### 1.2 There was no other protection

There is **no PID file and no lock anywhere in the tree**. Responsiveness
probing was the entire mutual-exclusion mechanism.

### 1.3 Where this meets `daemon-stalls.md`

The third row of the table above is not hypothetical for this codebase. A
`SessionStore` deadlock is a *confirmed* production defect
(`daemon-stalls.md` § 1.5b–1.5c): the daemon parks every thread in
`futex_wait`, keeps its listening socket, and answers nothing. That is exactly
the state in which `daemon_is_running()` returns `false` about a live process.

**The two bug classes compose into silent data-tree sharing.** A stall makes the
daemon unresponsive; unresponsiveness invites a second instance; the second
instance takes the socket and both then write the same session store, schedule
file, and memory index. The instance work is therefore not merely defensive
polish — it is the blast-radius limiter for a failure mode we have already
observed in production.

---

## 2. The ownership model

Three rules. Everything in phases 08–11 follows from them.

### 2.1 Exclusion is an OS invariant, never an inference

A daemon owns the instance iff it holds an exclusive `flock` on
`~/.daemoneye/var/run/daemoneye.pid`. Acquisition is `LOCK_EX | LOCK_NB`;
`EWOULDBLOCK` means another daemon owns it, and the correct response is to exit.

`flock` rather than a bare PID file, for one decisive reason: **the kernel
releases a `flock` when the holder dies, for any reason, including `SIGKILL` and
a panic.** There is no stale-lock state to recover from heuristically. A bare
PID file requires exactly the kind of "is this PID still alive, is it really
ours, has the PID been recycled" guesswork that produced § 1.1 — it would
re-introduce the same class of bug one layer down.

The PID *file* still carries the PID as its contents, but only as
**diagnostic payload**, never as the exclusion mechanism. Nothing branches on
the number; things only report it.

### 2.2 Holding the lock is what licenses destructive action

Unlinking the socket is safe when — and only when — the lock is held. Once held,
no other daemon is alive by construction, so any socket file at the path is
definitionally stale.

This inverts the current logic. Today the code proves the socket is stale
(badly) in order to remove it. Under the model it proves *it is the only
daemon*, and staleness follows for free.

The same license governs shutdown. `src/daemon/mod.rs:805-813` currently
unlinks the socket and runs `tmux set-hook -gu` on four global hooks
unconditionally, so a duplicate's exit strips the survivor's socket path and all
of its tmux hooks — leaving a live daemon with no monitoring and an unreachable
address. Teardown must only ever touch what this process established.

### 2.3 No side effect precedes the lock

This is the rule the current code violates most severely. The guard sits at
`mod.rs:739`, but a duplicate reaching it has **already** done all of this to the
healthy daemon's environment:

| Side effect | Location | Damage to the live daemon |
|---|---|---|
| Deletes `de-pipe-*.log` | `mod.rs:397` | Destroys the running daemon's active pane-capture logs |
| Overwrites 4 global tmux hooks with its own `current_exe()` | `mod.rs:498`, `:513`, `:527`, `:541` | Silently repoints all monitoring at a different binary |
| Installs per-session hooks | `mod.rs:548` | Same |
| Memory namespace migration | `mod.rs:387` | Concurrent writer against a live memory store |
| Emits `daemon_start` | `mod.rs:478` | Pollutes the event log with a start that will not serve |
| Cache poller / scheduler / webhook spawn | `mod.rs:~580`, `:~620`, `:~651` | Duplicate 2 s tmux polling; duplicate 1 s `take_due()` |

And `anyhow::bail!` restores none of it. **A duplicate launch is destructive
today whether the guard fires or not** — which means the guard, even when it
works, is closing the barn door after the fact.

Therefore: acquire the lock immediately after log redirection (so the failure is
recorded somewhere) and before everything else.

---

## 3. Consequences for the concurrent-writer surface

Single-instance enforcement is the real fix for all of the following; they are
listed so the blast radius of § 1 is on the record, not because each needs its
own lock.

- **`schedules.json`** — two schedulers polling `take_due()` every second
  double-fire jobs, and the atomic-rename save makes the loser's writes vanish.
- **The memory FTS5 index** (`var/index/memory.db`) — SQLite will error rather
  than corrupt, but the errors surface as unexplained tool failures.
- **Session JSONL stores** — append-only, so interleaved writes from two
  daemons produce a single file with two conversations in it. This is what
  happened on 2026-07-25: sessions `55602977…` and `15e3293d…` were served by
  different processes against one tree.

---

## 4. Diagnostic gaps the incident exposed

### 4.1 `ping` and `status` cannot distinguish dead from wedged

Both print "not responding" (`src/cli/commands/lifecycle.rs:45-64`). With a PID
file they can say *"daemon PID 420175 is alive but not answering"* — the single
most useful sentence for the § 1.3 failure mode, and the one that would have
surfaced last night's stall instead of letting a second daemon paper over it.

This requires splitting `daemon_is_running()`'s boolean into the four cases of
§ 1.1 that it currently conflates. Its role changes: it becomes a client-side
report, and it must never again gate a destructive action.

### 4.2 A duplicate-instance signal is already available and is being swallowed

The webhook listener's `TcpListener::bind` (`src/webhook/server.rs:108`) returns
`EADDRINUSE` for a second instance — a free, unambiguous duplicate detector. It
is wrapped in `supervise(...)` (`mod.rs:~651`), which logs and **retries with
backoff forever**. A duplicate daemon spins there in silence.

Bind belongs at startup, where it can be fatal. Only the serve loop belongs
under a supervisor.

### 4.3 Events carry no PID

`log_event` (`src/daemon/utils/event_log.rs:10`) stamps `ts` and `event`. Only
`daemon_start` carries a PID, hand-passed by its one call site. That is why the
two `daemon_stop` lines in § 1 are indistinguishable and why the second instance
had to be inferred from timing. Every record should carry the emitting PID.

### 4.4 The fork reports success before the child can prove it started

`main()` forks at `src/main.rs:261`; the parent prints
`daemoneye daemon started (PID n)` and exits `0` unconditionally. The child may
then immediately fail on the instance lock, a missing API key, or a bind error.
Under § 2 the *correct* behavior for a duplicate launch is for the child to exit
— which makes this pre-existing dishonesty newly load-bearing, because a
duplicate launch becomes an expected event that must be reported to the user.

The fix is the standard readiness handshake: a pipe, the child writing its
outcome after a successful bind, the parent relaying it and exiting non-zero on
failure.

---

## 5. Non-goals

- **Cross-host exclusion.** `flock` is local. Two daemons on different hosts
  sharing `~/.daemoneye` over NFS is out of scope; `flock` semantics over NFS
  are not reliable and the product does not support that deployment.
- **Multi-instance support.** The model is deliberately one daemon per
  `$HOME`. Supporting several would mean namespacing the socket, event log,
  session store, schedule file, and memory index — a different product.
- **Supervising / auto-restarting a wedged daemon.** Detection is in scope
  (§ 4.1); acting on it is not.
- **Anything about where the SIGTERM came from.** See § 1.

---

## 6. Phase map

| Phase | Delivers | Design section |
|---|---|---|
| 08 instance-lock | `InstanceLock` (flock + PID payload), acquired before all side effects; socket unlink licensed by ownership; identity-checked teardown | § 2 |
| 09 fatal-bind-honest-liveness | Webhook bind fatal at startup; `daemon_is_running()` → four-case enum; `ping`/`status` report wedged-vs-dead | § 4.1, § 4.2 |
| 10 lifecycle-observability | PID on every event record; logger-init failure surfaced; startup identity line | § 4.3 |
| 11 fork-readiness-handshake | Parent reports the child's real startup outcome | § 4.4 |

Phase 08 alone closes the hijack. 09–11 make the next occurrence of anything in
this document diagnosable in minutes rather than hours.

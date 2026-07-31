# Phase 03: Test Sleep Removal

**Milestone:** M7 — Memory Search & Maintenance
**Status:** todo
**Depends on:** phase-02 (bug-tracker-truth, done)
**Estimated diff:** ~25 lines across three test sites
**Tags:** language=rust, kind=test, size=s

## Goal

Three live tests wait on the real clock, which `STANDARDS.md` §3.3 forbids. One
of them burns **three full seconds** of wall time. Make all three deterministic
without weakening what they assert.

## Architecture references

None — this phase changes test code only. Read `docs/dev/STANDARDS.md` §3.3
(How tests are written), the rule being enforced:

> Tests are **deterministic**: no `sleep`, no real wall-clock time (inject a
> clock), no unseeded RNG. If a test can't be made deterministic, mark it as
> ignored and explain why in a comment on the test.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any file.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

**The milestone README said "four sleep sites". That was wrong** — it was
derived from a text grep that both over- and under-counted. The tree was
re-scanned by walking each `sleep(` call back to its enclosing function and
reading that function's attributes. The true picture:

**Three live (non-`#[ignore]`d) test sleeps — all three are this phase's work:**

| Site | Test | Sleep |
|---|---|---|
| `src/session_store_tests.rs:254` | `list_returns_newest_first` | 10 ms, real clock |
| `src/daemon/mod.rs:1151` | `liveness_is_unresponsive_when_peer_never_replies` | **3 s, real clock** |
| `src/daemon/context/background.rs:450` | `spawn_is_noop_when_in_flight` | 10 ms, virtual clock |

**Five sleeps are already compliant and must NOT be touched.** All five sit
inside `#[ignore]`d tests that already carry the justification comment §3.3
requires — `tests/integration.rs:615` (`daemon_ping_status_loop`),
`tests/integration.rs:1746`/`:1770`/`:1778` (`window_switch_does_not_corrupt_chat`),
and `tests/isolation.rs:591` (`webhook_ghost_e2e_http`). Leave all of `tests/`
alone in this phase.

**Every fix below has been validated by the architect against this exact tree**
— each one applied, run, and reverted. You are applying known-good changes.

### Site 1 — `list_returns_newest_first`

The sleep exists so the two saved sessions get distinguishable timestamps.
`list_sessions()` (`src/session_store.rs:292`) sorts by the **index entry's
`last_updated` string**, not by file mtime:

```rust
entries.sort_by(|a, b| b.1.last_updated.cmp(&a.1.last_updated));
```

`last_updated` is set from `chrono::Utc::now().to_rfc3339()`
(`src/session_store.rs:211`). The sort is a plain string compare, and RFC3339
sorts correctly lexicographically.

The test module is `#[path = "session_store_tests.rs"] mod tests` declared
inside `src/session_store.rs:480` with `use super::*` at its top, so it **can
call the private `load_index()` and `save_index()`** directly. That is the seam
to use: save both sessions, then stamp deterministic timestamps into the index.

**Do NOT** reach for `filetime` here. It is a dev-dependency and is used
elsewhere in the repo (`src/daemon/utils/mod.rs:178`), but it sets file mtimes —
and this ordering does not read mtimes at all.

### Site 2 — `liveness_is_unresponsive_when_peer_never_replies`

The test opens a socket, spawns a liveness probe, then sleeps 3 s to hold the
stream open across the probe's internal 2 s timeout. That is a real 3-second
wall-clock wait on `#[tokio::test]`, which uses a real clock.

The fix is one attribute. `#[tokio::test(start_paused = true)]` starts the
runtime with a **paused virtual clock** that auto-advances whenever all tasks
are idle — so both the probe's 2 s timeout and this 3 s sleep resolve instantly
and in the correct order, with no real waiting. Measured before and after:

```
before:  test result: ok. 1 passed; ... finished in 3.00s
after:   test result: ok. 1 passed; ... finished in 0.00s
```

Verified stable across **15 consecutive runs, zero failures**. The `sleep` line
itself stays — under a paused clock it is an injected clock, exactly what §3.3
prescribes, not a wall-clock wait.

### Site 3 — `spawn_is_noop_when_in_flight`

This one is already on a virtual clock (`#[tokio::test(start_paused = true)]`,
`src/daemon/context/background.rs:429`), so it is not costing wall time. It is
in scope because the wait is **pointless**, and a pointless wait reads as a real
synchronisation requirement to the next person.

`spawn_compaction` returns *before* spawning anything when a compaction is
already in flight (`src/daemon/context/background.rs`):

```rust
let snapshot = match try_snapshot(&session_id, &sessions) {
    Some(s) => s,
    None => return, // already in flight or ghost
};
```

The test sets `compaction_in_flight = true` first, so `try_snapshot` returns
`None` and **no task is ever spawned**. The comment "Give the (non-existent)
task a moment" says as much. There is nothing to wait for.

## Spec

1. **`src/session_store_tests.rs` — `list_returns_newest_first`.** Delete the
   `std::thread::sleep(...)` line between the two `save_session` calls. After
   the second save and *before* `list_sessions()`, stamp deterministic
   timestamps:

   ```rust
   let mut index = load_index();
   index.get_mut("aaa").expect("aaa indexed").last_updated =
       "2026-01-01T00:00:00Z".to_string();
   index.get_mut("bbb").expect("bbb indexed").last_updated =
       "2026-01-02T00:00:00Z".to_string();
   save_index(&index).expect("save index");
   ```

   Leave the three existing assertions exactly as they are — `list.len() == 2`,
   `list[0].0 == "bbb"`, `list[1].0 == "aaa"`. `bbb` carries the later timestamp,
   so it must still sort first.

2. **`src/daemon/mod.rs` — `liveness_is_unresponsive_when_peer_never_replies`.**
   Change that one test's attribute from `#[tokio::test]` to
   `#[tokio::test(start_paused = true)]`. Change nothing else in the test — not
   the sleep, not the durations, not the assertion.

   **Only that one test.** `src/daemon/mod.rs:1131` and `:1158` are *different*
   tests (`liveness_is_not_running_when_socket_absent` and
   `liveness_is_not_running_when_peer_closes_immediately`) that also carry plain
   `#[tokio::test]`. They contain no sleep and are already fast. Leave them.

3. **`src/daemon/context/background.rs` — `spawn_is_noop_when_in_flight`.**
   Replace the sleep and its comment:

   ```rust
   // Give the (non-existent) task a moment.
   tokio::time::sleep(std::time::Duration::from_millis(10)).await;
   ```

   with:

   ```rust
   // Yield so a task would get to run if one HAD been spawned.
   tokio::task::yield_now().await;
   ```

   Leave the test's `start_paused = true` attribute and its assertion untouched.

## Acceptance criteria

- [ ] No `sleep` remains in any live (non-`#[ignore]`d) test. Verified by the
      scan in "End-to-end verification", which walks each `sleep(` back to its
      enclosing function and checks that function's attributes.
- [ ] `liveness_is_unresponsive_when_peer_never_replies` reports **`finished in
      0.00s`** (was `3.00s`).
- [ ] `cargo test` passes with lib at **991**, integration at **30** (2 ignored),
      isolation at **8** (1 ignored), and `bug_tracker` at **6** — every count
      unchanged. This phase adds and removes no tests.
- [ ] Nothing under `tests/` is modified — `git diff --name-only` lists no path
      starting with `tests/`.
- [ ] `cargo build` zero new warnings; `cargo clippy --all-targets
      --all-features -- -D warnings` exits 0; `cargo fmt --all` leaves the tree
      unchanged.

## Test plan

**No new tests.** This phase changes how three existing tests wait; it adds no
function and no behavior. The existing assertions are the coverage, and spec
tasks 1–3 explicitly preserve every one of them.

**What would make this phase a silent regression:** weakening an assertion to
make a test pass without its wait. If any of the three tests will not pass with
its assertions intact, **stop and file a blocker** — do not adjust the
assertion. The whole point is that these tests keep asserting exactly what they
asserted before, faster and deterministically.

## End-to-end verification

The real artifacts are the test binaries and their timings. Run this block
verbatim and paste the resulting file's contents into your Update Log entry:

```bash
cd /home/matt/src/daemoneye
{
  echo "=== the 3s test is now instant ==="
  cargo test --lib liveness_is_unresponsive_when_peer_never_replies 2>&1 | grep -E 'test result'
  echo "exit=$?"

  echo "=== the other two touched tests pass ==="
  cargo test --lib list_returns_newest_first 2>&1 | grep -E 'test result'
  cargo test --lib spawn_is_noop_when_in_flight 2>&1 | grep -E 'test result'
  echo "exit=$?"

  echo "=== NO sleep remains in any live test (attribute-aware scan) ==="
  python3 - <<'PY'
import re, os
bad = []
for base in ('src', 'tests'):
    for root, _, files in os.walk(base):
        for fn in sorted(files):
            if not fn.endswith('.rs'):
                continue
            p = os.path.join(root, fn)
            lines = open(p).read().split('\n')
            for i, l in enumerate(lines):
                if 'sleep(' not in l:
                    continue
                fi = None
                for j in range(i, -1, -1):
                    if re.match(r'\s*(pub )?(async )?fn \w+', lines[j]):
                        fi = j
                        break
                if fi is None:
                    continue
                attrs, k = [], fi - 1
                while k >= 0 and (lines[k].strip().startswith('#[')
                                  or lines[k].strip().startswith('//')
                                  or not lines[k].strip()):
                    if lines[k].strip().startswith('#['):
                        attrs.append(lines[k].strip())
                    k -= 1
                is_test = any('test' in a for a in attrs)
                paused = any('start_paused' in a for a in attrs)
                ignored = any('ignore' in a for a in attrs)
                if is_test and not ignored and not paused:
                    bad.append(f"{p}:{i+1}")
print('LIVE WALL-CLOCK SLEEPS:', bad if bad else 'NONE')
PY
  echo "exit=$?"

  echo "=== nothing under tests/ was touched ==="
  git diff --name-only | grep '^tests/'
  echo "grep-exit=$?   # 1 == tests/ untouched == PASS"

  echo "=== full gate ==="
  cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -3
  echo "clippy-exit=$?"
  cargo test 2>&1 | grep -E '^test result'
  echo "exit=$?"
} > /tmp/phase03-e2e.txt 2>&1
cat /tmp/phase03-e2e.txt
```

The `tests/`-untouched block proves its case by being **empty**, so its
`grep-exit=1` marker is the whole proof. The sleep scan must print exactly
`LIVE WALL-CLOCK SLEEPS: NONE`.

Paste the captured file into an Update Log entry titled
`### Update — <date> (end-to-end verification)`. **The server-authored
`(complete)` entry does not satisfy this** — its "Command output tails" block is
the automatic gate capture every phase receives, and it shows that
build/lint/test ran, not that this phase's acceptance criteria were exercised.

## Authorizations

- [ ] May add dependencies: **none**. `tokio::task::yield_now` and
      `tokio::test(start_paused)` are both already available — `tokio` is a
      dependency with the `test-util` feature enabled in `[dev-dependencies]`,
      which is what `start_paused` requires.
- [ ] May touch `docs/architecture.md`: no.
- [ ] May create new files: no.

## Out of scope

- **Every sleep under `tests/`.** All five are inside `#[ignore]`d tests that
  already carry the §3.3 justification comment. Touching them is out of scope
  and would show up as a failed acceptance criterion.
- **Adding a gate that prevents future test sleeps.** It was considered and
  deliberately rejected for this phase: distinguishing a test sleep from a
  legitimate production sleep (the retry backoff at `src/ai/mod.rs:185`, the
  `EAGAIN` retry loop at `src/cli/input/tty.rs:370`) needs real Rust parsing.
  A string-heuristic version produced false positives on exactly those two
  sites when the architect tried it. A gate built on that heuristic would be
  disabled the first time it blocked a legitimate change.
- **The 39 production `sleep` calls in `src/`.** They are real behavior —
  backoff, polling, EAGAIN loops — and §3.3 governs tests only.
- **Changing any assertion.** See the Test plan.
- **Converting the `#[ignore]`d tests into deterministic ones.** A much larger
  piece of work (they need tmux and live API keys) and not this phase.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

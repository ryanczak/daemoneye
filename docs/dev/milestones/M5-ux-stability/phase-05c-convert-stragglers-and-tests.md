# Phase 05c: Convert the Last Stragglers and Every Test-Module Acquisition

**Milestone:** M5 — UX & Stability
**Status:** done
**Depends on:** phase-05b (last blocking work removed) — `done`
**Estimated diff:** ~170 lines
**Tags:** language=rust, kind=refactor, size=m

## Goal

Convert the **22** remaining `sessions.lock()` acquisitions to `with_sessions`:
the last **2** production stragglers in `ask.rs`, and **all 20** test-module
acquisitions. After this phase the *only* acquisition left in the whole tree is
the one inside `with_sessions` itself.

| File | Sites | Kind |
|---|---|---|
| `daemon/server/ask.rs` | 2 (`:519`, `:686`) | **production** — multi-line, missed by six phases of `grep -c` |
| `daemon/context/background.rs` | 17 | test module |
| `daemon/session.rs` | 3 | test module |

**Finish condition: the whole-tree scan reports exactly the two `session.rs`
lines described in § "What must survive" — nothing else, in production or test
code.**

**This phase is the precondition for the newtype.** Phase 05d makes raw `.lock()`
stop compiling; it cannot land while any of these 22 remain. This is a mechanical
conversion sweep — no behavior changes except the one called out in task 1.

## Architecture references

Read before starting:

- `CLAUDE.md` § "Important Invariants" — `.unwrap_or_log()` at every lock site is
  a project invariant; `with_sessions` satisfies it internally, which is why
  converting a site means *deleting* its `unwrap_or_log`, not preserving it.
- `docs/design/daemon-stalls.md` § 1.5c — the re-entrancy failure `with_sessions`
  now asserts against. Relevant to task 4, which must not be "simplified."

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Verify the starting state.

**This phase needs a scan that counts test code too.** The scan used by every
prior phase stops at `#[cfg(test)]`, because until now every target was
production. Save this one as `/tmp/scan_all.py` — it reports production and test
counts separately:

```python
import pathlib, re, sys
for f in sys.argv[1:]:
    L = pathlib.Path(f).read_text().splitlines()
    tb = next((i for i, l in enumerate(L, 1) if l.strip().startswith("#[cfg(test)]")), None)
    prod, test, lines = 0, 0, []
    for i, l in enumerate(L, 1):
        hit = "sessions.lock()" in l or (
            re.search(r'\bsessions\s*$', l) and i < len(L) and L[i].strip().startswith(".lock()")
        )
        if not hit:
            continue
        lines.append(i)
        if tb and i >= tb:
            test += 1
        else:
            prod += 1
    print(f"{f}: prod={prod} test={test} lines={lines}")
```

Then:

```bash
python3 /tmp/scan_all.py src/daemon/server/ask.rs src/daemon/context/background.rs src/daemon/session.rs
#   src/daemon/server/ask.rs: prod=2 test=0 lines=[519, 686]
#   src/daemon/context/background.rs: prod=0 test=17 lines=[385, 392, 410, 428, 440, 447, 461, 474, 481, 489, 504, 522, 552, 573, 599, 614, 619]
#   src/daemon/session.rs: prod=2 test=3 lines=[432, 443, 1183, 1204, 1226]
```

**Verified against the tree while drafting.** If any count differs, **stop and
report a blocker** — the per-site guidance below is stale.

Note `session.rs` reports `prod=2`. **Both are expected to stay.** See § "What
must survive."

## Current state

### ⚠ `background.rs` loses its `UnpoisonExt` import — corrected after run 1

**This section replaces an earlier claim that no import changes at all.** That
claim was wrong and it is what stalled the first run.

`background.rs` has **exactly 17** `unwrap_or_log` calls, and they are **exactly
the 17 test sites this phase converts**. `with_sessions` handles poison
internally, so after the conversion the count is **0** and
`use crate::util::UnpoisonExt;` (line 279, inside `mod tests`) is unused.

**`cargo build` and `cargo clippy --all-targets` disagree about this**, which is
what makes it hard to see:

```
cargo build                                        → 0 warnings
cargo clippy --all-targets --all-features -D warnings
  → error: unused import: `crate::util::UnpoisonExt`
```

A test-module import is invisible to a plain `cargo build` because the test cfg
is not compiled. **This is the same trap that produced 04f's `hard_fail`.** Run
**both** commands; treat clippy as authoritative.

**So: delete `use crate::util::UnpoisonExt;` from `background.rs`.** That is the
one and only import change this phase makes.

Do **not** pipe gate commands through `tail`/`head` — `cargo clippy … | tail -20`
exits with `tail`'s status, so a failing gate reads as passing. Run the command
bare and check its exit code.

### Imports — the other two files need no edit

`ask.rs` and `session.rs` already have `with_sessions` in scope, and **their
imports must not change.**

- **`ask.rs`** imports by glob: `use crate::daemon::session::*;` and
  `use crate::daemon::utils::*;` (lines 7 and 9). Both `with_sessions` and
  `UnpoisonExt` arrive that way, so converting a site changes nothing about
  imports.
- **`session.rs`** *defines* `with_sessions`, and its `mod tests` does
  `use super::*;` (line 473).

`background.rs` also already imports `with_sessions` at line 11
(`use crate::daemon::session::{SessionStore, with_sessions};`) and its
`mod tests` does `use super::*;` (line 275) — **that line stays.** The only
import this phase touches anywhere is the `UnpoisonExt` deletion described
above.

If you find yourself editing any import line other than that one, **stop — you
have gone off-spec.**

### ⭐ The worked example is in the same file, eleven times over

`ask.rs` already has **11** `with_sessions` calls. Line 319 is the closest
analogue to task 1 — the identical `.and_then(|id| with_sessions(…))` shape:

```rust
            .and_then(|id| with_sessions(sessions, |store| store.get(id).map(|e| e.started_at)));
```

And line 499 shows the `?`-inside-the-closure form:

```rust
        .and_then(|id| with_sessions(sessions, |store| store.get(id)?.default_target_pane.clone()));
```

**Receiver form:** `ask.rs` takes `sessions: &SessionStore`, so every call is
`with_sessions(sessions, …)` — **no** `&`. All 11 existing calls do this; match
them.

### What must survive — do not convert these

Two `session.rs` lines will still show in the scan after this phase, and **both
are correct as they stand**:

| Line | What it is |
|---|---|
| `session.rs:432` | the acquisition **inside `with_sessions` itself** — the one real lock in the codebase, by design |
| `session.rs:443` | **not code at all** — a line of the `cleanup_pass` doc comment containing the literal `sessions.lock()`. The scan matches raw text. |

Converting `:432` would make `with_sessions` call itself. "Fixing" `:443` means
editing a doc comment that is correct as written. **Leave both alone.**

## Spec

### 1. `ask.rs:519` — the poison-bail this milestone exists to remove

Current:

```rust
    let last_snapshot_activity: u64 = session_id
        .as_ref()
        .and_then(|id| {
            sessions
                .lock()
                .ok()?
                .get(id)
                .map(|e| e.last_snapshot_activity)
        })
        .unwrap_or(0);
```

Target:

```rust
    let last_snapshot_activity: u64 = session_id
        .as_ref()
        .and_then(|id| {
            with_sessions(sessions, |store| {
                store.get(id).map(|e| e.last_snapshot_activity)
            })
        })
        .unwrap_or(0);
```

**This one changes behavior, deliberately.** `.ok()?` is a *poison bail*: if the
mutex were poisoned it silently yields `None`, `unwrap_or(0)` turns that into
`0`, and a `last_snapshot_activity` of 0 makes `inject_snapshot` fire when it
otherwise would not. `with_sessions` **recovers** from poison instead (logging an
ERROR via `unwrap_or_log`), so the real value is read.

That is the entire point of this milestone's lock work — **do not preserve the
`.ok()?` bail**, and do not add a fallback to "keep the old behavior on poison."

### 2. `ask.rs:686` — a drain, and the mutation must stay inside

Current:

```rust
    let pending_notice: Option<String> = session_id.as_ref().and_then(|id| {
        sessions
            .lock()
            .unwrap_or_log()
            .get_mut(id)
            .and_then(|e| e.pending_compaction_notice.take())
    });
```

Target:

```rust
    let pending_notice: Option<String> = session_id.as_ref().and_then(|id| {
        with_sessions(sessions, |store| {
            store
                .get_mut(id)
                .and_then(|e| e.pending_compaction_notice.take())
        })
    });
```

**`.take()` is a mutation, not a read** — it drains the notice so it is delivered
exactly once. It **must stay inside the closure**. Reading the field out and
clearing it afterwards would open a window where two turns both see the notice,
and no test would catch it.

### 3. `background.rs` — 17 test-module conversions

All 17 are in `mod tests` and every one is one of two shapes.

**Shape A — a chained one-shot** (e.g. `:385`):

```rust
        sessions
            .lock()
            .unwrap_or_log()
            .insert(session_id.clone(), entry);
```

becomes

```rust
        with_sessions(&sessions, |store| {
            store.insert(session_id.clone(), entry);
        });
```

**Shape B — a scoped guard block** (e.g. `:392`):

```rust
        {
            let mut store = sessions.lock().unwrap_or_log();
            let entry = store.get_mut(&session_id).unwrap();
            entry.turn_count = 1;
            // … more statements …
        }
```

becomes

```rust
        with_sessions(&sessions, |store| {
            let entry = store.get_mut(&session_id).unwrap();
            entry.turn_count = 1;
            // … more statements …
        });
```

**Receiver form here is `&sessions`** — with the ampersand. These are local
`SessionStore` values (`let sessions: SessionStore = Arc::new(…)`), not
references, which is the opposite of `ask.rs`. The four existing `with_sessions`
calls in this file already use `&sessions`; match them.

**Three things to watch:**

- **`.unwrap()` inside test bodies is fine and stays.** `STANDARDS.md` exempts
  test code, and these are asserting a fixture exists. Do not convert them to
  `?` or add error handling.
- **A trailing `drop(store);`** ends a Shape-B block in some sites. The closure's
  end replaces it — **delete the `drop`**, do not carry it inside the closure.
- **Do not merge adjacent sites.** Two consecutive blocks that both lock stay two
  `with_sessions` calls. Merging them changes what is observed between them and
  is not a conversion.

### 4. `session.rs` — 3 test-module conversions, one of them delicate

`:1183` and `:1204` are ordinary Shape A / Shape B conversions using
`.unwrap()` rather than `.unwrap_or_log()`. Convert them the same way.

**`:1183`'s test ends in a `try_lock` assertion — do not touch it.** Eleven lines
below the site you are converting sits:

```rust
        // Guard from cleanup_pass is dropped; try_lock must succeed.
        assert!(sessions.try_lock().is_ok());
```

That is the whole point of that test. It is `try_lock`, not `lock`, so the scan
does not count it and this phase does not convert it. **There are two such
assertions in this file** (`:1193` here and `:1233` in task 4's test) and both
must survive byte-identical.

**`:1226` is inside the test for `with_sessions` itself**, and needs care:

```rust
    #[test]
    fn with_sessions_runs_closure_and_releases_lock() {
        let sessions: SessionStore = Arc::new(Mutex::new(HashMap::new()));
        {
            let mut store = sessions.lock().unwrap();
            store.insert("test".to_string(), entry_with(Instant::now()));
        }

        let len = with_sessions(&sessions, |s| s.len());
        assert_eq!(len, 1, "closure return value should be passed through");
        assert!(
            sessions.try_lock().is_ok(),
            "guard must be released after with_sessions returns"
        );
    }
```

- **Convert the setup block** (the `{ let mut store = … }`) to `with_sessions`.
  Using the function under test for fixture setup is acceptable here — the
  assertions are about return-value pass-through and guard release, neither of
  which the setup can fake.
- **Do NOT touch `sessions.try_lock()`.** It is the *assertion*, and it is the
  only thing proving the guard was released. It is `try_lock`, not `lock`, so
  the scan does not count it and this phase does not convert it. Replacing it
  with anything else makes the test vacuous.

Both `try_lock` assertions in this file (`:1193`, `:1233`) are load-bearing in
exactly this way, and the acceptance criterion pins their combined count at
**2**.

### 5. Verify no import moved and no `unwrap_or_log` was orphaned

- `grep -c "UnpoisonExt" src/daemon/server/ask.rs` returns **0** before and
  after — `ask.rs` never imported it directly; it arrives by glob.
- `grep -c "UnpoisonExt" src/daemon/context/background.rs` returns **1**, and
  `src/daemon/session.rs` returns **1** — both unchanged. `session.rs` still
  needs it for `with_sessions`'s own body at `:432`.

**No import line changes in this phase, in any file.**

## Acceptance criteria

- [ ] `python3 /tmp/scan_all.py src/daemon/server/ask.rs src/daemon/context/background.rs src/daemon/session.rs`
      reports `prod=0 test=0` for `ask.rs`, `prod=0 test=0` for `background.rs`,
      and **`prod=2 test=0`** for `session.rs` — the two survivors at `:432`
      and `:443`.
- [ ] A whole-tree sweep finds nothing else:
      `python3 /tmp/scan_all.py $(git ls-files 'src/**/*.rs')` reports a non-zero
      count for **`src/daemon/session.rs` only**.
- [ ] `grep -c "with_sessions(" src/daemon/server/ask.rs` returns **13**
      (11 pre-existing + 2).
- [ ] `grep -c "with_sessions(" src/daemon/context/background.rs` returns **21**
      (4 pre-existing + 17).
- [ ] `grep -c "unwrap_or_log" src/daemon/server/ask.rs` returns **2** — down
      from 3. The two that remain are on `cache.panes`, a different lock.
- [ ] `grep -cF '.ok()?' src/daemon/server/ask.rs` returns **0** — the poison
      bail is gone, not relocated. (It is currently **1**, and site `:519` is its
      only occurrence in the file, so this criterion is exact. Use `-F`: the
      string contains regex metacharacters.)
- [ ] `grep -cF "sessions.try_lock().is_ok()" src/daemon/session.rs` returns
      **2** — **not 1.** Both guard-release assertions survive untouched; see
      task 4.
- [ ] `grep -c "UnpoisonExt" src/daemon/server/ask.rs` returns **0** (unchanged)
      and `src/daemon/session.rs` returns **1** (unchanged — still needed for
      `with_sessions`'s own body at `:432`).
- [ ] `grep -c "UnpoisonExt" src/daemon/context/background.rs` returns **0** —
      **the import must be DELETED.** See § "`background.rs` loses its
      `UnpoisonExt` import" below. This criterion was **corrected after the first
      run**; it previously said 1, which is impossible.
- [ ] `git diff --stat` shows **exactly three** `src/` files changed.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **915** lib-unit tests — unchanged. This phase adds
      no tests and deletes none; **any other number means scope crept.**
- [ ] `cargo test` completes without hanging.

The `grep -c` criteria count raw text **including comments** — that is exactly
why `session.rs:443` shows up. **Do not write the literal `sessions.lock()` or
`with_sessions(` in a new comment** in these files.

## Test plan

This is a conversion sweep. The 20 test-module sites **are** tests — converting
them changes how each test acquires the store, not what it asserts. The suite is
its own regression net: a botched conversion in `background.rs` shows up as a
failing compaction test, not as a silent behavior change.

The **2 production sites are not covered.** `handle_ask` needs a live AI client, a
tmux session and an IPC peer, so nothing in the suite exercises `ask.rs:519` or
`:686`.

Run the suite and report what you observe. **Report only which commands you ran
and whether they passed.** Do **not** claim any test "guards" or "covers" the two
production sites — that would be false. In this project a claim about what a test
would catch is admissible only when demonstrated by mutation, and this phase
requires no mutation.

Three reasoning checks to state in the Update Log, no new tests:

1. **Task 1's behavior change.** Confirm the `.ok()?` bail is gone and state, in
   one sentence, what now happens on a poisoned mutex that previously did not.
2. **Task 2's drain.** Confirm `.take()` is still **inside** the closure, and say
   why moving it out would break delivery-exactly-once.
3. **Task 4's assertion.** Confirm `sessions.try_lock().is_ok()` is untouched and
   that only the setup block above it was converted.

## End-to-end verification

None required. This phase ships no new artifact, no CLI behavior, and no config
surface. The gates plus the three reasoning checks above are the verification.

## Authorizations

- [x] May edit `src/daemon/server/ask.rs`, `src/daemon/context/background.rs`,
      and `src/daemon/session.rs` — **test modules included.**
- [x] **Must delete** `use crate::util::UnpoisonExt;` from
      `src/daemon/context/background.rs` — it is unused once the 17 conversions
      land, and `cargo clippy --all-targets` fails without the deletion.
- [ ] **No other** import addition or deletion, in any file. `ask.rs` and
      `session.rs` already have `with_sessions` in scope and must not change.
- [ ] **No** new tests, no deleted tests, no renamed tests.
- [ ] **No** edits to `session.rs:432` (the acquisition inside `with_sessions`)
      or `:443` (the doc comment the scan matches).
- [ ] **No** edits to `sessions.try_lock()` in
      `with_sessions_runs_closure_and_releases_lock`.
- [ ] **No** conversion of `SessionStore` to a newtype — that is 05d.
- [ ] **No** `#[allow(...)]` anywhere. If clippy objects to a shape, report a
      blocker rather than suppressing.

## Out of scope

- **The `SessionStore` newtype and the enforcement lint** — phase 05d. This phase
  is its precondition; do not start it.
- **04f's coverage follow-up** — three vacuous `compaction_in_flight` assertions
  in `background.rs`'s test module that need to be made real and mutation-checked.
  That is **phase 05e**. You will be editing tests immediately adjacent to them.
  **Convert them and change nothing else** — do not "fix" an assertion you notice
  looks weak, and do not add one.
- **`Arc::new(Mutex::new(HashMap::new()))` construction sites** — 16 of them. They
  keep working unchanged until the newtype lands, and rewriting them is 05d's job.

### ⚠ Do not insert an item between a doc comment and the item it documents

This cost phase 05a two extra runs. A `struct` added "immediately above" a
function landed **between** that function's `///` block and the function itself,
silently transferring a `pub fn`'s documentation onto the struct. It compiled, it
linted clean, and nothing in the gate set could see it.

This phase adds no new items, so the risk is low — but if you insert anything at
item scope, **read the lines directly above the insertion point first.**

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-26 23:00 (started)

**Executor:** Claude (Anthropic)

Converting the last 22 `sessions.lock()` acquisitions to `with_sessions`: 2 production sites in `ask.rs` and 20 test-module sites across `background.rs` (17) and `session.rs` (3).

### Update — 2026-07-26 (escalation)

**Chosen lever:** resume (`continue_phase`)
**Rationale:** all 22 conversions are already correct on disk and the stall was
caused by a contradiction in my spec, not by the executor — resume preserves the
work and removes the contradiction, where a fresh re-dispatch would re-derive
finished work against an already-green scan.

### Notes for executor — 2026-07-26 (after run 1's `NoProgressStall`)

**Your conversion work was correct and is still on disk. Do not redo it.**
Verified independently:

| Check | Result |
|---|---|
| `scan_all` — `ask.rs` / `background.rs` | `prod=0 test=0` both |
| `scan_all` — `session.rs` | `prod=2 test=0` (the two intended survivors) |
| `with_sessions(` — `ask.rs` / `background.rs` | 13 / 21 |
| `.ok()?` in `ask.rs` | 0 — poison bail gone |
| `.take()` at `ask.rs:686` | still inside the closure |
| `sessions.try_lock().is_ok()` | 2 — both assertions intact |

**The stall was my spec's fault.** It asserted
`grep -c "UnpoisonExt" src/daemon/context/background.rs` must stay at **1** and
forbade all import deletions. Both were wrong: `background.rs`'s 17
`unwrap_or_log` calls *are* the 17 sites you converted, so the import is now
unused and `cargo clippy --all-targets` fails on it. You spent ~60 read-only
turns re-checking an impossible criterion. The criterion and the authorization
are now corrected.

**There is exactly one edit left:** delete `use crate::util::UnpoisonExt;` from
`src/daemon/context/background.rs` (line 279, inside `mod tests`).

Then run the four gates **bare, not piped through `tail`** — a piped command
exits with `tail`'s status, so a failing gate reads as passing. Finish condition:

- `cargo clippy --all-targets --all-features -- -D warnings` **exits 0**
- `cargo test` reports **915** lib-unit tests — not 914, not 916
- `grep -c "UnpoisonExt" src/daemon/context/background.rs` returns **0**

### Update — ts=1785108582448 (complete, server-authored)

**Summary:** **Summary + Notes for review:**

All 22 conversions were already correct on disk from the prior run. The only remaining edit was deleting the now-unused `use crate::util::UnpoisonExt;` from `background.rs`'s test module (line 279). All four gates pass bare: `cargo clippy --all-targets --all-features -- -D warnings` exits 0, `cargo build` clean, `cargo fmt --all` clean, and `cargo test` reports **915** passed (unchanged). Verification counts: `with_sessions(` is 13 in `ask.rs` and 21 in `background.rs`; `.ok()?` is 0 in `ask.rs`; `sessions.try_lock().is_ok()` is 2 in `session.rs`; `UnpoisonExt` is 0 in `background.rs` and 1 in `session.rs`. The `.take()` mutation at `ask.rs:686` remains inside the closure. Both `try_lock` assertions in `session.rs` are untouched. One commit, clean working tree.

**Executor:** Qwen/Qwen3.6-27B-FP8

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.06s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
t_viewport_absent_when_zero ... ok
test tmux::cache::tests::get_labeled_context_background_panes_sorted ... ok
test tmux::cache::tests::get_labeled_context_dead_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_pane_classification ... ok
test tmux::cache::tests::get_labeled_context_session_topology ... ok
test tmux::cache::tests::get_labeled_context_synchronized_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_copy_mode_annotated ... ok
test tmux::cache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test tmux::cache::tests::get_labeled_context_chat_pane_excluded_from_background ... ok
test search::tests::search_events_returns_tail_not_head_when_segment_exceeds_cap ... ok
test search::tests::search_finds_match_in_runbooks ... ok
test search::tests::search_respects_kind_filter ... ok
test memory::tests::memory_without_frontmatter_has_empty_metadata ... ok
test search::tests::search_returns_empty_for_no_match ... ok
test session_store::tests::backfill_idempotent ... ok
test session_store::tests::backfill_missing_artifact_returns_error_name ... ok
test session_store::tests::backfill_stamps_memory_without_frontmatter ... ok
test session_store::tests::backfill_stamps_runbook ... ok
test session_store::tests::backfill_stamps_script ... ok
test session_store::tests::collision_allowed_with_force ... ok
test session_store::tests::delete_nonexistent_errors ... ok
test session_store::tests::collision_rejected_without_force ... ok
test session_store::tests::delete_removes_dir_and_index ... ok
test session_store::tests::list_returns_newest_first ... ok
test session_store::tests::load_messages_max_count_truncates ... ok
test session_store::tests::rename_nonexistent_errors ... ok
test session_store::tests::rename_to_existing_errors ... ok
test session_store::tests::rename_updates_dir_and_index ... ok
test session_store::tests::save_and_load_round_trip ... ok
test session_store::tests::update_in_place_allowed ... ok

test result: ok. 915 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.32s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 29 tests
test daemon_ping_status_loop ... ignored
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test event_log_append_read ... ok
test ipc_ask_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test minimal_config_parsing ... ok
test ghost_config_parsing ... ok
test event_log_entry_format ... ok
test schedule_store_persistence ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test config_pricing_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g4_briefing_masking_applied ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_to_event_log ... ok
test g5_mailbox_write_and_read ... ok

test result: ok. 27 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.01s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.07s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-02078a4e6496e4bf)
     Running unittests src/main.rs (target/debug/deps/daemoneye-a516fbd17ae98770)
     Running tests/integration.rs (target/debug/deps/integration-ce8aee7d32f385fa)
   Doc-tests daemoneye

```

**Files changed:**

- `src/daemon/context/background.rs` — +0 -1

**Commit:** d4321da25bad649f51e234766ac408574956adfb

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-07-26

- **Verdict:** approved_after_1
- **Bounces:** 1 `hard_fail` (`NoProgressStall`, 123 turns), resolved by resume
  (20 turns). No bug filed — the failure was a spec defect, not executor work.
- **Executor:** Qwen/Qwen3.6-27B-FP8
- **Scope deviations:** none. All 22 conversions landed in run 1 exactly as
  specified; run 2 added only the one-line import deletion.
- **Calibration:** two threads, one now a confirmed trend.

**The `hard_fail` was my spec's fault, and it is worth recording precisely.** The
spec asserted `grep -c "UnpoisonExt" src/daemon/context/background.rs` must stay
at **1** and forbade all import deletions. Both were impossible: `background.rs`'s
17 `unwrap_or_log` calls **are** the 17 sites the phase converts, so the import is
necessarily unused afterwards and `cargo clippy --all-targets` fails on it. The
executor completed every conversion correctly, then spent ~60 read-only turns
re-checking an unsatisfiable criterion (varying `sed` ranges enough to slip past
the identical-call governor) until `NoProgressStall` fired. It was right and the
document was wrong.

**Resume was the correct lever** — the work was on disk and correct, and the stall
had a single removable cause. A fresh re-dispatch would have re-derived finished
work against an already-green scan, which is how 05a's run 2 produced a false
completion.

**Verified by reading, not counting:**

- **Task 1's poison bail is gone.** `ask.rs:519` no longer carries `.ok()?`; on a
  poisoned mutex the value is now recovered and logged rather than silently
  becoming `0` and forcing a snapshot injection.
- **Task 2's `.take()` is still inside the closure**, preserving
  delivery-exactly-once for the compaction notice.
- **Both `try_lock` assertions survive** (`:1193`, `:1233`) with only their setup
  blocks converted. These are the only things proving guard release, and one sits
  eleven lines below a converted site.
- **No test was added, removed or renamed** — `git diff` shows no `#[test]` /
  `#[tokio::test]` attribute changes, and the count held at 915.
- **No `unsafe` was added.** The eight hits in these files are pre-existing
  `std::env::set_var("HOME", …)` calls that Rust 2024 requires to be `unsafe`.

**Gates re-run bare, with exit codes captured** rather than piped through `tail`:
fmt 0, build 0 (zero warnings), clippy 0, test 0 with **915** lib-unit tests.

### The lock work is now structurally complete

A whole-tree sweep finds acquisitions in **one file only**:

| File:line | What it is |
|---|---|
| `session.rs:432` | `with_sessions` itself — the one real acquisition, by design |
| `session.rs:443` | **not code** — a `cleanup_pass` doc-comment line the text-based scan matches |

Everything else in the daemon, production and test, goes through `with_sessions`.
That is exactly the precondition 05d needs: the newtype will either compile or
name precisely what was missed.

### Calibration

1. **Build-vs-clippy disagreement on test-module imports — 2nd occurrence, now a
   trend.** 04f's `hard_fail` and 05c's have the same root: `cargo build` reports
   zero warnings while `cargo clippy --all-targets` errors on an unused
   test-module import, because a plain build never compiles the test cfg. Both
   cost a full run. **Candidate fold:** a phase that converts *every* use of a
   trait must state what happens to its import, and must not assert an import
   count without checking whether the conversions exhaust its uses. Awaiting PE
   sign-off.
2. **Piping gate commands through `tail` masks the exit status — 1st occurrence.**
   Run 1 executed `cargo clippy … 2>&1 | tail -20`, which exits with `tail`'s
   status, so a hard failure read as a pass. This turned a loud, immediately
   actionable error into a silent one and is part of why the run stalled instead
   of failing fast. Data, not yet a trend; if it recurs the fold belongs in the
   executor contract.

# Phase 06t: `bounded_output` — Stage A Slice 2, `src/tmux/session.rs`

**Milestone:** M5 — UX & Stability
**Status:** done
**Depends on:** phase-06s — `done` (introduced `bounded_output`)
**Estimated diff:** ~60 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Convert the **9 `.output()` call sites in `src/tmux/session.rs`** to
`crate::tmux::bounded_output`, so every one is bounded by `TMUX_TIMEOUT` instead
of hanging indefinitely on a wedged tmux server.

**Finish condition: `src/tmux/session.rs` has zero `.output()` calls and nine
`bounded_output(` calls, and every surrounding error-handling expression is
byte-identical to what it is today.**

## Architecture references

- `docs/design/daemon-stalls.md` § 1 mechanism B.
- `src/tmux/mod.rs` — `bounded_output`, `bounded_output_with`, `TMUX_TIMEOUT`.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Verify the starting state:

```bash
grep -c "\.output()"      src/tmux/session.rs   # expect 9
grep -c "bounded_output(" src/tmux/session.rs   # expect 0
grep -c "\.output()"      src/tmux/window.rs    # expect 0
grep -c "\.output()"      src/tmux/pane.rs      # expect 30
cargo test 2>&1 | grep "^test result" | head -3   # expect 921 lib, 0, 27 integration
```

**Every number above was produced by running that exact command against the tree
while drafting.** If one differs, **stop and report a blocker**.

`window.rs` is already **0** (slice 1 converted it) and `pane.rs` stays at **30**
(slice 3). Both are pinned so this phase can prove it stayed in its lane.

## Current state

### The conversion is a pure substitution — the type does not change

`bounded_output` returns the **same** `std::io::Result<std::process::Output>` that
`.output()` returns. So the transformation is:

```
<builder>.output()   →   crate::tmux::bounded_output(<builder>)
```

and **every surrounding expression stays exactly as it is** — the `?`, the
`match`, the `.ok()?`, the `.map(…).unwrap_or(…)`. This is unlike the
`off_runtime` conversions elsewhere in this milestone, which changed
`Result<T>` into `Option<Result<T>>` and needed a collapse at each site. **Here
there is no collapse. Do not add one.**

Worked example, already landed in this tree by slice 1
(`src/tmux/window.rs:106`):

```rust
// before
    let output = Command::new("tmux")
        .args(["rename-window", "-t", &target, new_name])
        .output()?;

// after
    let output = crate::tmux::bounded_output(Command::new("tmux").args([
        "rename-window",
        "-t",
        &target,
        new_name,
    ]))?;
```

The `?` is untouched; only the terminator moved.

### The 9 sites carry **five** different surrounding shapes

Line numbers are current-as-of-drafting; re-derive before editing. All nine take
the identical substitution — the table is here so you can confirm you have not
disturbed the handling, **not** because any of them needs different treatment.

| Site | Enclosing fn | Surrounding shape | On timeout |
|---|---|---|---|
| `:24` | `list_sessions` | `match … { Ok(o) => o, Err(_) => return Vec::new() }` | empty list |
| `:63` | `list_session_flags` | `match … { Ok(o) if success => o, _ => return HashMap::new() }` | empty map |
| `:206` | `session_environment` | `?` | `Err` propagates |
| `:234` | `get_active_pane` | `?` | `Err` propagates |
| `:247` | `current_session_name` | `.ok()?` | `None` |
| `:261` | `client_dimensions` | `match … { Ok(o) if success => o, _ => return (0, 0) }` | `(0, 0)` |
| `:312` | `ensure_incident_session` | `?` | `Err` propagates |
| `:326` | `session_exists` | `.map(\|o\| o.status.success()).unwrap_or(false)` | `false` |
| `:335` | `list_pane_ids_in_session` | `?` | `Err` propagates |

**Every one of those timeout outcomes is what the site already produces when the
tmux call fails today** — `.output()` already returns `Err` on spawn failure, and
each shape already handles it. A timeout is just one more `Err`. So this
conversion is behaviour-preserving; it only replaces "hang forever" with "fail
the way this site already fails".

Two worth stating explicitly because they look risky and are not:

- **`session_exists` → `false` on timeout.** The daemon then tries to create a
  session that may already exist; `tmux new-session -d -s <name>` fails on a
  duplicate, and the caller's existing error arm handles that. Same as any tmux
  failure today.
- **`client_dimensions` → `(0, 0)` on timeout.** Its callers already guard with
  `if w > 0 && h > 0`.

### ⚠ `cargo fmt` reflows these call sites heavily

Converting changes the expression's nesting depth, so `fmt` re-wraps the
`.args([…])` arrays — a one-line array may explode to one element per line, and a
`let` may gain a line break. **This is expected and correct.** Apply the
substitution, then run `cargo fmt --all` and accept its output. Do not
hand-format, and do not treat the reflow as a mistake.

Two post-`fmt` examples from the checked run, showing that the handling survives
verbatim:

```rust
    let out = match crate::tmux::bounded_output(Command::new("tmux").args([
        "list-sessions",
        "-F",
        "#{session_name}\t#{session_windows}\t#{session_activity}\t#{session_attached}",
    ])) {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
```

```rust
pub fn current_session_name() -> Option<String> {
    let out =
        crate::tmux::bounded_output(Command::new("tmux").args(["display-message", "-p", "#S"]))
            .ok()?;
```

## Spec

1. **Convert all 9 `.output()` sites** in `src/tmux/session.rs` per the
   substitution above. Change nothing else in the file.
2. **Run `cargo fmt --all`** — mandatory; this project has no `format_fix` hook.
3. `cargo build` after the file.

## Acceptance criteria

- [ ] `grep -c "\.output()" src/tmux/session.rs` returns **0** (printed **9**
      before).
- [ ] `grep -c "bounded_output(" src/tmux/session.rs` returns **9**.
- [ ] `grep -c "\.output()" src/tmux/pane.rs` returns **30** — **unchanged**.
      It is slice 3; a lower number means this phase over-reached.
- [ ] `grep -c "\.output()" src/tmux/window.rs` returns **0** — unchanged from
      slice 1.
- [ ] `grep -cF "pub fn bounded_output_with(" src/tmux/mod.rs` returns **1** and
      `grep -cF "pub fn bounded_output(" src/tmux/mod.rs` returns **1** — the
      helper was not modified.
- [ ] **All five surrounding shapes survive verbatim.** Each of these returns
      **1**:

```bash
grep -cF "Err(_) => return Vec::new()," src/tmux/session.rs
grep -cF "_ => return HashMap::new()," src/tmux/session.rs
grep -cF ".ok()?;" src/tmux/session.rs
grep -cF "_ => return (0, 0)," src/tmux/session.rs
grep -cF ".map(|o| o.status.success())" src/tmux/session.rs
```

- [ ] `git diff --name-only | grep -c Cargo` returns **0** — no dependency
      change.
- [ ] `grep -cE '#\[allow|unsafe' src/tmux/session.rs` returns **0**.
- [ ] `git diff --name-only -- src/` lists exactly **one** file:
      `src/tmux/session.rs`.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **921** lib-unit and **27** integration tests —
      **unchanged**. This phase adds no tests.

**Run every gate bare.** Every number above was produced by running that exact
command against a tree with this change applied.

## Test plan

`bounded_output` itself is covered by the five tests slice 1 landed
(`src/tmux/mod.rs`), including the pipe-buffer regression test. **This phase adds
no tests**: it changes which function these 9 sites call, not what they compute,
and every one needs a live tmux server to exercise.

**The suite must stay at 921 lib tests.** If any test needs editing, **stop and
report a blocker** — it would mean a signature or behaviour changed, which this
phase forbids.

Two reasoning checks. **Quote the code — a claim without a quotation is not an
answer:**

1. **Why no collapse.** Quote one converted site with its surrounding `match` or
   `?`, and state in one sentence why the error handling did not need to change.
2. **The riskiest-looking timeout.** Quote `session_exists` as you left it and
   state in one sentence what a timeout makes it return, and why that is the same
   thing it already does when tmux fails.

## End-to-end verification

Not applicable — this phase ships no new runtime-loadable artifact. It redirects
9 existing call sites to a helper whose timeout behaviour was demonstrated by its
own tests in slice 1. **Do not repeat that demonstration.**

## Authorizations

- [x] May edit `src/tmux/session.rs` — **the 9 `.output()` call sites only.**
- [x] May let `cargo fmt --all` reflow those call sites.
- [ ] **No** change to any surrounding `match`, `?`, `.ok()?`, or
      `.map(…).unwrap_or(…)`.
- [ ] **No** change to `src/tmux/mod.rs` — the helper is finished.
- [ ] **No** edits to `src/tmux/pane.rs` (slice 3) or `src/tmux/window.rs`
      (slice 1, done).
- [ ] **No** new dependency, no new tests, no `#[allow(...)]`.
- [ ] **No** signature change to any function in `session.rs`.

## Out of scope

- **`src/tmux/pane.rs` (30 sites)** — slice 3, and it will need splitting.
- **`src/tmux/cache.rs`** — it holds no direct `Command::new("tmux")` calls.
- **The `Drop` impls and `src/cli/`** — they call tmux directly rather than
  through `src/tmux/`; bounding them is a later decision, not this slice.

### ⚠ Traps

1. **No collapse.** The return type is unchanged, so `?` stays `?` and every
   `match` arm stays as written. Adding `.ok()`, `.flatten()` or an extra arm is
   wrong here.
2. **`pane.rs` stays at 30.** Converting it here is over-reach.
3. **Let `fmt` reflow** — do not hand-format, and run `cargo fmt --all` before
   finishing.
4. **The suite stays at 921.** No new tests.
5. **`session_exists` keeps `.unwrap_or(false)`** — do not "improve" it to
   `true`; false is what it already returns when tmux fails.
6. **Only the terminator moves.** `X.output()` → `bounded_output(X)`, nothing
   else on the line changes.

## Update Log

### Update — 2026-07-29 01:55 (started)

**Executor:** Claude (Sonnet 4.5)
**Action:** Converting all 9 `.output()` call sites in `src/tmux/session.rs` to `crate::tmux::bounded_output`.

<!-- entries appended below this line -->

### Update — ts=1785290441884 (complete, server-authored)

**Summary:** **Summary + Notes for review:**

Converted all 9 `.output()` call sites in `src/tmux/session.rs` to `crate::tmux::bounded_output()`. This is a pure substitution — `bounded_output` returns the same `std::io::Result<std::process::Output>` type, so every surrounding error-handling expression (`match`, `?`, `.ok()?`, `.map(|o| o.status.success()).unwrap_or(false)`) survived verbatim with zero changes. Only the terminator moved: `X.output()` → `bounded_output(X)`.

**Reasoning check 1 (no collapse):** e.g. `session_exists` — `crate::tmux::bounded_output(Command::new("tmux").args(["has-session", "-t", name])).map(|o| o.status.success()).unwrap_or(false)` — no collapse needed because `bounded_output` returns `io::Result<Output>`, the same type as `.output()`, so `.map().unwrap_or(false)` works identically.

**Reasoning check 2 (riskiest timeout):** `session_exists` on timeout returns `false` (via `.unwrap_or(false)`), which is the same outcome it already produces when tmux fails to spawn. The daemon then attempts `new-session -d -s <name>`, which fails on a duplicate and the caller's existing error arm handles it — same as today.

All 12 acceptance criteria verified: 0 `.output()` / 9 `bounded_output(` in session.rs, 30 unchanged in pane.rs, 0 in window.rs, all 5 surrounding shapes intact, no Cargo changes, no `allow`/`unsafe`, exactly one source file changed, `cargo build`/`clippy`/`fmt`/`test` all pass (921 lib + 27 integration tests, unchanged).

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
ests::auth_empty_secret_always_allows ... ok
test webhook::server::tests::auth_token_without_bearer_prefix_denies ... ok
test webhook::server::tests::auth_wrong_token_denies ... ok
test tmux::cache::tests::get_labeled_context_pane_classification ... ok
test tmux::cache::tests::get_labeled_context_session_topology ... ok
test tmux::cache::tests::get_labeled_context_synchronized_pane_noted ... ok
test tmux::cache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test tmux::cache::tests::get_labeled_context_chat_pane_excluded_from_background ... ok
test tmux::bounded_output_tests::bounded_output_times_out_and_kills_the_child ... ok
test search::tests::search_events_returns_tail_not_head_when_segment_exceeds_cap ... ok
test search::tests::search_finds_match_in_runbooks ... ok
test session_store::tests::artifacts_round_trip ... ok
test search::tests::search_respects_kind_filter ... ok
test search::tests::search_returns_empty_for_no_match ... ok
test memory::tests::session_memory_block_respects_cap ... ok
test session_store::tests::backfill_missing_artifact_returns_error_name ... ok
test session_store::tests::backfill_stamps_memory_without_frontmatter ... ok
test session_store::tests::backfill_stamps_runbook ... ok
test session_store::tests::backfill_stamps_script ... ok
test session_store::tests::collision_allowed_with_force ... ok
test session_store::tests::collision_rejected_without_force ... ok
test session_store::tests::delete_nonexistent_errors ... ok
test memory::tests::update_memory_creates_new_entry ... ok
test session_store::tests::list_returns_newest_first ... ok
test session_store::tests::load_messages_max_count_truncates ... ok
test session_store::tests::rename_nonexistent_errors ... ok
test session_store::tests::rename_to_existing_errors ... ok
test session_store::tests::rename_updates_dir_and_index ... ok
test session_store::tests::save_and_load_round_trip ... ok
test session_store::tests::update_in_place_allowed ... ok

test result: ok. 921 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.31s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 29 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g4_briefing_injection_block_format ... ok
test g5_depth_limit_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test ipc_ask_round_trip ... ok
test event_log_entry_format ... ok
test minimal_config_parsing ... ok
test ghost_config_parsing ... ok
test schedule_store_persistence ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test g4_briefing_read_and_clear ... ok
test config_pricing_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test g4_briefing_masking_applied ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_to_event_log ... ok

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

- `docs/dev/milestones/M5-ux-stability/phase-06t-bounded-output-session.md` — +5 -2
- `src/tmux/session.rs` — +48 -45

**Commit:** 8b5d53313c54495118f05f80f7c0d8749ceedc72

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-07-29

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** Qwen/Qwen3.6-27B-FP8 (42 turns)
- **Scope deviations:** none
- **Calibration:** none

All four gates re-run bare and green (`cargo fmt --all --check`, `cargo build`
after `touch`ing `session.rs` — zero warnings, `cargo clippy --all-targets
--all-features -- -D warnings`, `cargo test` at **921** lib + 27 integration,
**unchanged** as specced).

Every criterion is exact: `.output()` in `session.rs` **0** (9 before) with
`bounded_output(` **9**; `pane.rs` **30** and `window.rs` **0**, both unchanged so
neither neighbouring slice was touched; both helper declarations **1**, and
`src/tmux/mod.rs` does not appear in the commit at all; no `Cargo` file in the
commit; `#[allow]`/`unsafe` **0**; exactly one `src/` file.

**All five surrounding shapes survive at 1 each** — the criterion this phase
existed to guard:

```
Err(_) => return Vec::new(),   1
_ => return HashMap::new(),    1
.ok()?;                        1
_ => return (0, 0),            1
.map(|o| o.status.success())   1
```

Verified by reading:

- **No collapse was added anywhere.** Zero `.flatten()` or `.and_then(` in the
  added lines. `))?;` appears **4** times, matching the four `?` sites exactly.
  This was the phase's one real temptation — every `off_runtime` slice in this
  milestone needed a collapse, and this type-preserving conversion needs none.
- **`session_exists` is untouched below the terminator:**

  ```rust
  pub fn session_exists(name: &str) -> bool {
      crate::tmux::bounded_output(Command::new("tmux").args(["has-session", "-t", name]))
          .map(|o| o.status.success())
          .unwrap_or(false)
  }
  ```

  `.unwrap_or(false)` intact — not "improved" to `true`.
- **Both `match` fallback arms kept their distinct forms** — `list_sessions`'s
  `Err(_) =>` and `list_session_flags`'s guard-plus-`_ =>`. Collapsing them to a
  single shape would have been a plausible tidy-up and is not what the code says.
- The executor answered both reasoning checks with quoted code, including the
  correct account of why `session_exists` returning `false` on timeout is what it
  already does when tmux fails to spawn.

Stage A is now one file from complete: `pane.rs` (30 sites) is all that remains
before the fifth exit criterion closes.

# Phase 06 — Move `run_daemon` out of `daemon/mod.rs`

**Status:** done
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-26

## Goal

`src/daemon/mod.rs` (901 production lines) drops under the 800-line cap by
moving `run_daemon`, the two helpers it alone uses (`tmux_wanted`,
`startup_session`) and their test module `startup_session_tests` into a new
`src/daemon/startup.rs`. Pure move: `run_daemon` keeps its signature and
its public path, `crate::daemon::run_daemon`, through a re-export.
Splitting the 598-line function itself is a later phase.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md` — decisions 5, 6, 7.
- `src/main.rs` calls `daemon::run_daemon` (the only caller outside the
  module).

## Spec

1. **Cut with the shell — run this as written.** It was dry-run on a
   scratch worktree of `5a6ecbcd`: fmt, clippy and the full suite (2586)
   green with every step below. **Do not use `sed -i`**: the executor
   refuses in-place edits, so the rewrite goes through a temp file and `mv`.

   ```bash
   F=src/daemon/mod.rs
   S=$(awk '{l[NR]=$0} /^pub async fn run_daemon\(/ {s=NR-1; while (s>0 && (l[s] ~ /^\/\/\// || l[s] ~ /^#\[/)) s--; print s+1; exit}' $F)
   T=$(grep -n '^#\[cfg(test)\]' $F | tail -1 | cut -d: -f1)
   echo "S=$S T=$T"                      # expect 270 930
   { printf '//! Daemon startup: `run_daemon` and the tmux-session choice it makes.\n\nuse super::*;\n'
     sed -n '1,15p' $F | grep -v '^pub use '; echo; sed -n "${S},$((T-2))p" $F; } > src/daemon/startup.rs
   { head -n $((S-1)) $F; tail -n +$T $F; } > $F.tmp && mv $F.tmp $F
   ```

   `startup.rs` gets a copy of `mod.rs`'s `use` block (without its two
   `pub use` lines, which `use super::*` already brings in); step 4 prunes it.
2. **Log the cut at once.** Append a `### Update — <date> (cuts done)`
   entry to this doc's Update Log **with the patch or write tool**. The
   stall governor counts only edit-tool calls as progress (rexyMCP issue
   #14).
3. **Four one-line edits, with the patch tool:**
   - `src/daemon/mod.rs`: add `mod startup;` after `pub mod spend;`; after
     `pub mod utils;` add `pub use startup::run_daemon;` and, on the next
     two lines, `#[cfg(test)]` and `use startup::tmux_wanted;` (the
     `mod tests` left in `mod.rs` calls `tmux_wanted`).
   - `src/daemon/startup.rs`: `fn tmux_wanted(` becomes
     `pub(super) fn tmux_wanted(`.
4. **Prune imports:** `cargo fix --lib --allow-dirty`, which removes every
   unused import in both files (the dry run needed nothing else). If the
   classifier refuses it, delete each import the build warns about with the
   patch tool instead. Then `cargo fmt --all`.
5. **Allowlist:** delete `file src/daemon/mod.rs 901`, and change
   `fn src/daemon/mod.rs run_daemon 598` to
   `fn src/daemon/startup.rs run_daemon 598` (the function moved, unchanged).
6. **CLAUDE.md** "Key files": add a row for `src/daemon/startup.rs`:
   `run_daemon()`, the daemon's startup and accept loop, with
   `tmux_wanted()` / `startup_session()`, the tmux-session choice.
7. **Findings** (decision 7): you move all of `run_daemon`; record what
   you see while it scrolls past, or `none`. Note especially anything that
   would make splitting it hard, such as state shared across its sections.

## Acceptance criteria

- [ ] `src/daemon/startup.rs` exists and holds `pub async fn run_daemon(`,
      `fn tmux_wanted(`, `fn startup_session(` and `mod startup_session_tests`;
      `src/daemon/mod.rs` holds none of them and is at most 800 lines.
- [ ] `grep -c '^file src/daemon/mod.rs ' tests/hygiene_allowlist.txt` is 0,
      `grep -c '^fn src/daemon/startup.rs run_daemon 598$'` is 1, and
      `cargo test --test hygiene` passes.
- [ ] The End-to-end block prints `lost_other=0` and `added_other=0`.
- [ ] Preservation: `cargo test` passes with exactly 2586 passing tests.
- [ ] The Update Log has a `(cuts done)` entry written by an edit tool and a
      `### Findings — <date>` entry.

## End-to-end verification

Run with `bash`. Paste the output byte-exact into the Update Log.

```sh
export LC_ALL=C   # sort and comm disagree under the host locale without it
B=5a6ecbcd; A=/tmp/e2e-m31-06.txt; : > "$A"
wc -l src/daemon/mod.rs src/daemon/startup.rs | sed 's/^ *//' >> "$A"
echo "allowlisted=$(grep -c '^file src/daemon/mod.rs ' tests/hygiene_allowlist.txt) fn_moved=$(grep -c '^fn src/daemon/startup.rs run_daemon 598$' tests/hygiene_allowlist.txt)" >> "$A"
norm() { tr -d ' \t' | grep -v '^$' | sort; }
git show $B:src/daemon/mod.rs | norm > /tmp/m31-06-old.txt
cat src/daemon/mod.rs src/daemon/startup.rs | norm > /tmp/m31-06-new.txt
echo "lost_other=$(comm -23 /tmp/m31-06-old.txt /tmp/m31-06-new.txt | grep -vE '^(use|pubuse|fntmux_wanted\()' | wc -l)" >> "$A"
echo "added_other=$(comm -13 /tmp/m31-06-old.txt /tmp/m31-06-new.txt | grep -vE '^(use|pubuse|//!|pub\(super\)|#\[cfg\(test\)\]|modstartup;)' | wc -l)" >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Expected (from the dry run): `619 src/daemon/mod.rs`, `672
src/daemon/startup.rs`, `allowlisted=0 fn_moved=1`, `lost_other=0`,
`added_other=0`, hygiene `ok.`, clippy clean, `fmt_exit=0`,
`passed=2586 failed=0`. Small differences in the two line counts are fine if
`cargo fix` pruned differently; the rest must match.

## Authorizations

- Edit `src/daemon/mod.rs`; create `src/daemon/startup.rs`. Edit
  `tests/hygiene_allowlist.txt` (the two lines) and `CLAUDE.md` (one row).

## Out of scope

- Any change inside `run_daemon`, `supervise`, `detect_session` or
  `daemon_liveness`, even one a finding names.
- `src/main.rs`: its `daemon::run_daemon` call resolves through the
  re-export unchanged.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-26 09:05 (cuts done)

Ran the phase-06 cut script as written against the working tree (base commit
`5a6ecbcd`): `S=270 T=930` matched the expected anchors exactly. Created
`src/daemon/startup.rs` (676 lines: module doc, `use super::*`, the use block,
`run_daemon`, `tmux_wanted`, `startup_session`, `mod startup_session_tests`)
and trimmed `src/daemon/mod.rs` to 618 lines. Both file writes went through a
temp file and `mv` — no `sed -i`, per the phase doc.

### Findings — 2026-10-26

Read all 598 lines of `run_daemon` as it moved (decision 7). Recorded for the
later split phase:

1. **Six pieces of shared state cross the sections.** `startup_config`
   (loaded once, then re-cloned into the scheduler, webhook and session-cleanup
   supervisors), `shutdown: Arc<AtomicBool>` (every `supervise` block),
   `cache: Arc<SessionCache>` (N7 viewport seed, poller, scheduler, webhook,
   accept loop), `sessions: SessionStore` (scheduler, webhook, cleanup, accept
   loop), `schedule_store` (scheduler, webhook, accept loop) and
   `initial_session` (cache construction, N7, scheduler, `daemon_start` event).
   A split will need either a context struct or per-section builder functions
   taking all of these; threading them one at a time through four or five
   extractions is the slow part, not the cutting.
2. **The four `tokio::spawn(supervise(...))` blocks are the natural seam.**
   Each is self-contained behind clones of the shared state; extracting them as
   `spawn_cache_poller` / `spawn_scheduler` / `spawn_webhook` /
   `spawn_session_cleanup` free functions would remove ~250 lines with no
   behaviour change and leave a much smaller linear prologue.
3. **`startup.rs` now leans on `use super::*` for `supervise`, `log_event`,
   `handle_client` and `SessionStore`** — all glob re-exports from `mod.rs`
   (`pub use server::*`, `pub use utils::*`). Nothing broke in the move, but the
   module's real dependency set is implicit; a follow-up that lists the imports
   explicitly would make future moves cheap to review.
4. **The startup ordering is load-bearing and undocumented in one place:**
   `startup_config` is loaded *before* the tmux detection and API-key check but
   *after* the instance lock and log redirect, and `cache` must exist before
   the N7 viewport seed. Any extraction must preserve
   lock → config → tmux-detect → cache → supervisors → bind order.
5. No surprises that would block a split; nothing inside the function was
   touched (out of scope per this phase).

### Update — 2026-10-26 09:17 (end-to-end verification)

Ran the phase doc's E2E block with `bash` against the finished tree. The first
run printed `added_other=1`: the stray line was `pubmodstartup;` — I had
declared `pub mod startup;` where the spec's edit list says `mod startup;` (the
module has no external users; `run_daemon` is re-exported from `mod.rs` and
`mod.rs`'s test module reaches `tmux_wanted` via `crate::daemon::startup::`,
which works for a private sibling module). Changed it to `mod startup;` and
re-ran the whole block in one pass. Verbatim transcript of the second run:

```
617 src/daemon/mod.rs
672 src/daemon/startup.rs
1289 total
allowlisted=0 fn_moved=1
lost_other=0
added_other=0
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2586 failed=0
```

`cargo clippy --all-targets --all-features -- -D warnings` printed zero
warnings/errors (grep -c over its output: 0). Acceptance criteria checked:
`startup.rs` holds all four items and `mod.rs` holds none of them (grep counts
4 and 0); `mod.rs` is 617 lines (cap 800); allowlist greps give 0 and 1;
`cargo test --test hygiene` ok; E2E `lost_other=0 added_other=0`;
`cargo test` passes with exactly 2586 passing, 0 failed.

### Update — 2026-10-26 16:19 (complete, server-authored)

**Summary:** **Summary + Notes for review**

Moved `run_daemon` (598 lines), `tmux_wanted`, `startup_session` and the `startup_session_tests` module from `src/daemon/mod.rs` (901 lines) to a new `src/daemon/startup.rs` (672 lines), leaving `mod.rs` at 617 lines — under the 800 cap — with `pub use startup::run_daemon;` preserving the public path, so `src/main.rs` is untouched. The cut script ran as written with anchors matching the spec exactly (`S=270 T=930`); `cargo fix --lib` pruned the imports on both sides, and the allowlist edit, CLAUDE.md row, and Findings entry (six pieces of shared state crossing sections, the four `supervise` blocks as the natural seam for the later split) are all in place. E2E verification ran the phase doc's block byte-exact and passed on the second run — the first run caught `added_other=1` because I had written `pub mod startup;` where the spec pins `mod startup;` (nothing external uses the module; the re-export and the `crate::daemon::startup::tmux_wanted` test import both work with a private sibling), fixed and re-verified: `lost_other=0`, `added_other=0`, hygiene `ok.` (16 passed), clippy zero warnings, `fmt_exit=0`, and `cargo test` passes with exactly 2586 passing / 0 failed. One deviation to note: `cargo fix` left `mod.rs` at 617 lines rather than the dry run's 619 (it pruned one import more), which the spec explicitly permits; status is left `in-progress` per the contract, and the working tree is clean with everything in commit `e8718e00`.

**Executor:** nvidia/GLM-5.3-Flash-FP4-Local

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
.. ok
test functions_in_a_test_module_are_not_measured ... ok
test production_after_a_test_only_fn_is_still_counted ... ok
test production_lines_exclude_an_inline_test_module ... ok
test impl_and_trait_methods_are_measured_with_their_type ... ok
test exempt_files_exist ... ok
test allowlist_has_no_stale_entries ... ok
test test_files_fit_the_cap ... ok
test production_files_fit_the_cap ... ok
test functions_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.26s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g5_depth_limit_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g5_child_inherits_depth_and_parent ... ok
test g4_briefing_injection_block_format ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_tool_call_response_round_trip ... ok
test resume_run_request_roundtrip ... ok
test minimal_config_parsing ... ok
test ghost_config_parsing ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test config_pricing_round_trip ... ok
test schedule_store_persistence ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g6_agent_config_roundtrip ... ok
test event_log_append_read ... ok
test g6_agent_namespace_field_persisted ... ok
test event_log_entry_format ... ok
test g4_briefing_read_and_clear ... ok
test g4_briefing_injects_on_next_run ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_no_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test an_empty_session_starts_no_tmux_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_installs_no_tmux_hooks ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.30s


running 4 tests
test gate_catches_a_multi_line_write ... ok
test gate_catches_an_abbreviated_write ... ok
test gate_catches_a_chained_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-93da9e6ee623fb89)
kill: (829291): No such process
kill: (-829987): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-e8cf52a7c32204b4)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-e3c81dbbd815c973)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-13504e43862292bc)
     Running tests/hygiene.rs (target/debug/deps/hygiene-5888a12c4886b465)
     Running tests/integration.rs (target/debug/deps/integration-9d2755b5d0d87df4)
     Running tests/isolation.rs (target/debug/deps/isolation-aff2e96eb7fd44a5)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-ed33cf3a0b1b3008)
   Doc-tests daemoneye

```

**Files changed:**

- `CLAUDE.md` — +1 -0
- `docs/dev/milestones/M31-code-hygiene/README.md` — +1 -1
- `docs/dev/milestones/M31-code-hygiene/phase-06-daemon-startup.md` — +74 -1
- `src/daemon/mod.rs` — +3 -0
- `src/daemon/startup.rs` — +2 -3
- `tests/hygiene_allowlist.txt` — +1 -2

**Commit:** e8718e007fd040e30c3c9cc6ed6ab6cb858f5a83

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-26

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** nvidia/GLM-5.3-Flash-FP4-Local (75 turns, complete)
- **Scope deviations:** one, accepted. The test-only `tmux_wanted` import
  went inside `mod.rs`'s `mod tests` (`use crate::daemon::startup::tmux_wanted;`)
  instead of as a `#[cfg(test)]` import beside the re-export. Same effect,
  one line. Otherwise the reviewer's diff against `246f8cff`: the moved
  block is identical apart from the spec'd `pub(super) fn tmux_wanted`, and
  `mod.rs` differs from the old file minus the block only by five pruned
  imports, `mod startup;`, the re-export and that test import. Gates
  re-run: fmt 0, build and clippy clean, 2586 passed / 0 failed / 13 ignored.
- **Calibration:** the first M31 split to land without a refinement or a
  takeover, and the fastest phase of the milestone. What changed from 04
  and 05: a recipe dry-run on a scratch worktree, a temp-file-and-`mv` cut
  instead of `sed -i`, the cut logged through an edit tool at once, and the
  multiset check's expected output stated in the doc. The executor's own
  E2E run caught its one slip (`pub mod startup;`) before reporting.

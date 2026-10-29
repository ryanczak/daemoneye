# Phase 05 — `wrap_words` breaks long tokens on char boundaries

**Status:** done
**Milestone:** M32 — Findings fixes
**Drafted:** 2026-10-29
**Findings:** M31 `FINDINGS.md` #23 and #26 (#24 is rejected below)

## Goal

`wrap_words` (`src/cli/viewer_layout.rs`) wraps transcript prose for the
viewer, counting widths in chars. Its overlong-token path is the
exception: it is gated on the token's byte length and cut into byte
chunks. Three bugs follow (#23):
- A non-ASCII token that fits in chars but not in bytes is sent down the
  overlong path. Its byte chunks fall mid-character, `from_utf8` fails,
  and the fallback pushes the whole token once per chunk. For example,
  30 CJK chars at width 80 show twice.
- A two-byte token is cut every `width` bytes, which is half the
  intended row length.
- Leading whitespace before an overlong token makes `space_remaining`
  underflow, and the function panics in debug builds.

After this phase, the overlong path is gated on chars and breaks the
token char by char, filling the current row first. It cannot underflow.

The doc comment also gains the whitespace-only-line rule (#26). That
rule is load-bearing for the round-trip and is written down nowhere
today.

**#24 is rejected.** `flush` is a local closure that takes its three
`&mut` locals as arguments and captures only `width`. Lifting it out of
the loop would change no behaviour and make nothing clearer.

## Architecture references

- M32 README, decisions 1 and 6.

## Spec

1. **Add the three tests below, and run them red.** Insert them in
   `src/cli/viewer.rs`'s test module, directly before
   `output_rows_keep_hard_wrap`, where the other `wrap_words_*` tests
   sit. Run `cargo test --lib viewer::tests::wrap_words`. All three must
   fail: the CJK test with the token twice in `left:`, the `é` test with
   `left: [5, 5, 5, 5, 5]`, and the leading-space test with a panic at
   `viewer_layout.rs` (subtract with overflow). Paste that output into
   the Update Log.
2. **The fix.** In `wrap_words`, replace the whole
   `if tok.len() > width { … continue; }` block and the
   `let token_len = tok.chars().count();` line after it with the block
   below. `token_len` moves up, so the overlong gate and the normal path
   share it. Run the tests again; all five `wrap_words_*` tests must
   pass.
3. **The doc.** Append these two sentences to `wrap_words`'s doc
   comment, after the round-trip sentence: "Widths count chars." and a
   new paragraph: "A hard line that is only whitespace keeps that
   whitespace as its one row: `has_content` is set for it up front,
   because `flush` writes an empty row for a line with no content, and
   that would lose the whitespace."
4. **Findings** go in their own `### Findings — <date>` entry, from
   `viewer_layout.rs` only, within five tool calls, or the single line
   `none`.
5. **Commit** as `fix:`.

**Do not look further.** Do not read the viewer's event loop, the
renderer or `layout_blocks`. The architect dry-ran this exact change on
`HEAD`: red at step 1, green at step 2, clippy and fmt clean,
`wrap_words` at 77 lines, and the full suite at `passed=2599 failed=0`.

The tests:

```rust
    #[test]
    fn wrap_words_keeps_a_long_cjk_token_once() {
        // 30 three-byte chars: 90 bytes but 30 chars, so it fits a width of 80.
        let token = "漢".repeat(30);
        let rows = wrap_words(&token, 80);
        assert_eq!(rows, vec![token], "the token fits and appears exactly once");
    }

    #[test]
    fn wrap_words_breaks_an_overlong_multibyte_token_on_char_boundaries() {
        let token = "é".repeat(25);
        let rows = wrap_words(&token, 10);
        let counts: Vec<usize> = rows.iter().map(|r| r.chars().count()).collect();
        assert_eq!(
            counts,
            vec![10, 10, 5],
            "rows of at most 10 chars: {rows:?}"
        );
        assert_eq!(rows.concat(), token, "every char kept once, in order");
    }

    #[test]
    fn wrap_words_survives_leading_space_before_an_overlong_token() {
        let rows = wrap_words(&format!("  {}", "x".repeat(12)), 10);
        let kept: String = rows.iter().map(|r| r.trim()).collect();
        assert_eq!(kept, "x".repeat(12), "every char kept once: {rows:?}");
    }
```

The replacement block:

```rust
            let token_len = tok.chars().count();
            if token_len > width {
                // An overlong token breaks on char boundaries, filling the
                // current row first.
                if has_content {
                    flush(&mut line, &mut out, &mut has_content);
                    space_remaining = width;
                }
                for ch in tok.chars() {
                    if space_remaining == 0 {
                        flush(&mut line, &mut out, &mut has_content);
                        space_remaining = width;
                    }
                    line.push(ch);
                    has_content = true;
                    space_remaining -= 1;
                }
                continue;
            }
```

## Acceptance criteria

- [ ] `grep -c 'as_bytes().chunks' src/cli/viewer_layout.rs` prints `0`
      (today: `1`).
- [ ] `wrap_words_keeps_a_long_cjk_token_once`,
      `wrap_words_breaks_an_overlong_multibyte_token_on_char_boundaries`
      and `wrap_words_survives_leading_space_before_an_overlong_token`
      pass, and the Update Log shows all three failing at step 1.
- [ ] `grep -c 'only whitespace keeps that' src/cli/viewer_layout.rs`
      prints `1` (today: `0`).
- [ ] `grep -c '^### Findings — '` on this doc prints `1`.
- [ ] `cargo test` reports **exactly** `passed=2599 failed=0` (2596 + 3).
      `tests/hygiene.rs` and `cargo test --test bug_tracker` pass. fmt,
      build and clippy are clean.

## End-to-end verification

```sh
bash <<'E2E'
F=src/cli/viewer_layout.rs
echo "byte_chunks=$(grep -c 'as_bytes().chunks' $F) doc=$(grep -c 'only whitespace keeps that' $F)"
cargo test --lib viewer::tests::wrap_words 2>&1 | grep -E '^test result'
cargo test --test hygiene 2>&1 | grep -E '^test result'
cargo test --test bug_tracker 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

Expected: `byte_chunks=0 doc=1`, `test result: ok. 5 passed` for the
`wrap_words` tests, hygiene `ok. 16 passed`, bug tracker `ok. 6
passed`, clippy clean, `passed=2599 failed=0`. Paste the output into an
`### Update — <date> (end-to-end verification)` entry. **Known flake:**
`read_shell_screen_lines_tails_the_visible_rows` (M31 #65). If the
block shows `passed=2598 failed=1`, run it once more and paste both
outputs.

## Authorizations

- Edit `src/cli/viewer_layout.rs` (`wrap_words` only) and the test
  module of `src/cli/viewer.rs`. No other source file.

## Out of scope

- Display width (double-width CJK cells). The viewer counts chars
  throughout; moving to cells is a separate change.
- `width == 0`, which panics today and still does.
- #24 (rejected above).

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-29 17:22 (started)

**Executor:** Claude (executor).

Read `STANDARDS.md`, the M32 README (decisions 1 and 6), this phase doc and
`WORKFLOW.md` as referenced. Status flipped `todo → in-progress` in this doc and
in the M32 README phase table; no `(complete)` entry follows from me. Starting
the Spec tasks in order.

### Update — 2026-10-29 17:22 (progress)

Task 1, red run. Inserted the three Spec tests directly before
`output_rows_keep_hard_wrap` in `src/cli/viewer.rs`'s test module and ran
`cargo test --lib viewer::tests::wrap_words`. All three fail exactly as the
Spec predicts (`exit=101`):

- `wrap_words_keeps_a_long_cjk_token_once`: `left:` holds the 30-char token
  **twice** — `["漢…(30)…漢", "漢…(30)…漢"]` against `right:`'s one — the
  fallback pushing the whole token once per 80-byte chunk.
- `wrap_words_breaks_an_overlong_multibyte_token_on_char_boundaries`:
  `left: [5, 5, 5, 5, 5]` against `right: [10, 10, 5]` — the two-byte token
  cut every `width` **bytes**, i.e. half the intended row length.
- `wrap_words_survives_leading_space_before_an_overlong_token`: panics at
  `src/cli/viewer_layout.rs:203:21` with `attempt to subtract with overflow`
  — the `space_remaining` underflow.

The two pre-existing tests (`wrap_words_does_not_split_words`,
`wrap_words_breaks_an_overlong_token`) still pass. Pasted below verbatim from
the captured run (output redirected to a file first; nothing retyped):

```
running 5 tests
test cli::viewer::tests::wrap_words_breaks_an_overlong_token ... ok
test cli::viewer::tests::wrap_words_does_not_split_words ... ok
test cli::viewer::tests::wrap_words_breaks_an_overlong_multibyte_token_on_char_boundaries ... FAILED
test cli::viewer::tests::wrap_words_keeps_a_long_cjk_token_once ... FAILED
test cli::viewer::tests::wrap_words_survives_leading_space_before_an_overlong_token ... FAILED

failures:

---- cli::viewer::tests::wrap_words_breaks_an_overlong_multibyte_token_on_char_boundaries stdout ----

thread 'cli::viewer::tests::wrap_words_breaks_an_overlong_multibyte_token_on_char_boundaries' panicked at src/cli/viewer.rs:1015:9:
assertion `left == right` failed: rows of at most 10 chars: ["ééééé", "ééééé", "ééééé", "ééééé", "ééééé"]
  left: [5, 5, 5, 5, 5]
 right: [10, 10, 5]

---- cli::viewer::tests::wrap_words_keeps_a_long_cjk_token_once stdout ----

thread 'cli::viewer::tests::wrap_words_keeps_a_long_cjk_token_once' panicked at src/cli/viewer.rs:1007:9:
assertion `left == right` failed: the token fits and appears exactly once
  left: ["漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢", "漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢"]
 right: ["漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢漢"]

---- cli::viewer::tests::wrap_words_survives_leading_space_before_an_overlong_token stdout ----

thread 'cli::viewer::tests::wrap_words_survives_leading_space_before_an_overlong_token' panicked at src/cli/viewer_layout.rs:203:21:
attempt to subtract with overflow
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
 (x2)
failures:
    cli::viewer::tests::wrap_words_breaks_an_overlong_multibyte_token_on_char_boundaries
    cli::viewer::tests::wrap_words_keeps_a_long_cjk_token_once
    cli::viewer::tests::wrap_words_survives_leading_space_before_an_overlong_token

test result: FAILED. 2 passed; 3 failed; 0 ignored; 0 measured; 2517 filtered out; finished in 0.00s

error: test failed, to rerun pass `--lib`
```

### Findings — 2026-10-29

From `src/cli/viewer_layout.rs` only (the file read whole for this phase; no
other file opened for the review):

1. **Leading/trailing whitespace is measured in bytes but spent as chars** —
   the same class as #23, one line over: `leading_len`/`trailing_len` are
   `hard_line.len() - hard_line.trim_*().len()` (byte counts), and
   `space_remaining = width.saturating_sub(leading_len)` treats each byte as a
   row cell. For ASCII whitespace — the normal case — bytes equal chars and
   nothing differs. A leading U+00A0 (2 bytes) or U+3000 (3 bytes) under-counts
   `space_remaining` by 2 per leading char, so the first row wraps a little
   early. No panic (`saturating_sub`), cosmetic only, and `trim_start` never
   splits a char, so the `split_at`s stay on boundaries. Whether it is worth a
   phase is the PE's call.
2. **`"output ({n} lines)"` is built twice** — once in `layout_block`'s
   `Block::Output` arm and again in `bare_header`'s. They are identical today;
   a shared helper would pin them together so a future edit to one reaches the
   other.
3. **Question, not a finding:** the blank separator row is tagged
   `block: i` — the index of the block *below* it. Whether that attribution is
   right depends on how the renderer and hit-test consume `ViewRow.block`,
   which this phase told me not to read, so I am flagging it unresolved rather
   than asserting it.

### Update — 2026-10-29 17:22 (end-to-end verification)

Ran the § End-to-end verification block verbatim, output redirected to
`/tmp/phase05-e2e.txt` and pasted below exactly as captured:

```
byte_chunks=0 doc=1
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 2517 filtered out; finished in 0.00s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.13s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s
passed=2599 failed=0
```

Matches the expected values: `byte_chunks=0 doc=1`, `5 passed` for the
`wrap_words` filter, hygiene `16 passed`, bug tracker `6 passed`, clippy clean
(no diagnostics, only the `Finished` line), full suite `passed=2599 failed=0`
— the named flake `read_shell_screen_lines_tails_the_visible_rows` did not
fire, so no re-run was needed.

### Update — 2026-10-29 17:39 (complete, server-authored)

**Summary:** </think>All tasks complete. Let me run through the completion checklist:

- [x] Phase doc's Status: line still says `in-progress` — yes (I flipped it there; did not flip to review).
- [x] Milestone README's phase-table row still says `in-progress` — yes.
- [x] No `(complete)` Update Log entry hand-written — correct, I wrote started, progress (red run), Findings, and end-to-end verification entries only.
- [x] `cargo fmt --all` run immediately before `git add` — yes.
- [x] All verification commands ran clean — yes.
- [x] Final message will be substantive Summary + Notes for review.
- [x] `git status --short` shows nothing.
- [x] `git log -1 --stat` shows all 4 files touched.

Also grep-pinned literals: `as_bytes().chunks` → 0, `only whitespace keeps that` → 1, `### Findings — ` → 1. All verified in the e2e entry.

Now write the final summary.</think>
Phase 05 complete. Commit `7abae0e5`, working tree clean, status left `in-progress` for the server's `review` flip.

**Summary**

Implemented the `wrap_words` fix (`src/cli/viewer_layout.rs`): the overlong-token path now gates on `tok.chars().count()` instead of byte length and breaks the token char by char, flushing the current row first and re-deriving `space_remaining` from `width`, which removes the underflow panic entirely. Added the three Spec tests to `src/cli/viewer.rs` before `output_rows_keep_hard_wrap`, captured them red exactly as predicted (token doubled in `left:`; `left: [5, 5, 5, 5, 5]` vs `right: [10, 10, 5]`; `attempt to subtract with overflow` at `viewer_layout.rs:203`), then green after the fix — all five `wrap_words_*` tests pass. Appended the two doc-comment sentences ("Widths count chars." and the whitespace-only-line paragraph) and wrote the `### Findings — 2026-10-29` entry from `viewer_layout.rs` alone. E2E block captured mechanically to a file and pasted: `byte_chunks=0 doc=1`, wrap filter `5 passed`, hygiene `16 passed`, bug_tracker `6 passed`, clippy clean, full suite `passed=2599 failed=0` — the M31 #65 flake did not fire, so no re-run was needed. Committed as one `fix:` commit; `cargo fmt --all` ran immediately before `git add`.

**Notes for review**

- No deviation from the Spec's replacement block — it was applied verbatim; `wrap_words` is 77 lines as the dry run predicted.
- Findings entry has two items and one flagged question: (1) leading/trailing whitespace is measured in **bytes** (`hard_line.len() - trim().len()`) but spent as chars in `space_remaining` — same class as #23, cosmetic, no panic (only visible with non-ASCII whitespace like U+00A0/U+3000); (2) `"output ({n} lines)"` is built identically in two arms and could share a helper; (3) the blank separator row tags `block: i` (the block *below*), which I could not verify without reading the renderer — left unresolved rather than asserted, per the "do not look further" boundary.
- Out of scope items untouched: display width, `width == 0`, #24.

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
ill_counted ... ok
test allowlist_lines_parse_and_malformed_lines_are_kept ... ok
test cfg_not_test_is_production ... ok
test production_lines_exclude_an_inline_test_module ... ok
test impl_and_trait_methods_are_measured_with_their_type ... ok
test exempt_files_exist ... ok
test production_files_fit_the_cap ... ok
test functions_fit_the_cap ... ok
test allowlist_has_no_stale_entries ... ok
test test_files_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.26s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test ghost_config_parsing ... ok
test ipc_ask_round_trip ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test resume_run_request_roundtrip ... ok
test minimal_config_parsing ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g6_agent_namespace_field_persisted ... ok
test event_log_append_read ... ok
test g6_agent_config_roundtrip ... ok
test g4_briefing_read_and_clear ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test webhook_ghost_e2e_deterministic ... ok
test an_empty_session_starts_no_tmux_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_installs_no_tmux_hooks ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.31s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_an_abbreviated_write ... ok
test gate_catches_a_multi_line_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-7593688ba66ac3a2)
kill: (3530869): No such process
kill: (-3531621): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-58152f9516574e2d)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-24319e47e01464a3)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-7278712a86caa2f2)
     Running tests/hygiene.rs (target/debug/deps/hygiene-f1caa86cc6b68e4c)
     Running tests/integration.rs (target/debug/deps/integration-6e2b1cb55f8da66f)
     Running tests/isolation.rs (target/debug/deps/isolation-555c471c8dff13d2)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-122203846dd0c681)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M32-findings-fixes/README.md` — +1 -1
- `docs/dev/milestones/M32-findings-fixes/phase-05-wrap-words.md` — +116 -1
- `src/cli/viewer.rs` — +28 -0
- `src/cli/viewer_layout.rs` — +14 -7

**Commit:** 7abae0e594c10647cb67c64b9afd68f8dd7972d4

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-29

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** nvidia/GLM-5.3-Flash-FP4-Local
- **Scope deviations:** none.
  - The production diff is the specified replacement block and the doc
    comment. "Widths count chars." stands as its own paragraph instead of
    following the round-trip sentence, which is harmless.
  - The tests are the dictated three.
- **Reviewer's checks:**
  - fmt 0, build and clippy clean, 2599 passed / 0 failed.
  - `bug_tracker` passes, with 6 tests.
  - `byte_chunks=0 doc=1`, and one Findings heading.
- **Tests:** the step-1 red run is pasted: the CJK token twice,
  `left: [5, 5, 5, 5, 5]`, and `attempt to subtract with overflow` at
  `viewer_layout.rs:203`. Green follows. Accepted.
- **Findings:** three, logged as #7–#9 in the M32 ledger and all
  deferred. #7 is the most useful: the same bytes-for-chars mistake,
  one line up, in the leading-whitespace count. It is cosmetic.
- **Calibration:** the third dictated-seam phase in a row to land first
  try (03 re-dispatch, 04, 05).


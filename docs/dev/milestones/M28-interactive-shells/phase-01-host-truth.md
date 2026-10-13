# Phase 01: host-truth — the shell host reports what owns its terminal

**Milestone:** M28 — Interactive shells
**Status:** review
**Depends on:** none
**Estimated diff:** ~450 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

The daemon cannot tell what is in the foreground of a PTY shell, and cannot
see its screen; both live in the detached `shell-host` process. This phase
makes the host report them over its socket and adds a pure classifier that
turns the report into a `Foreground`. Nothing in the daemon calls either
yet — phase 01b adds the tool, phase 02 the decisions — so there is **no
behaviour change** for any existing tool.

## Architecture references

- `docs/design/interactive-shells.md` § 1, D1, D2, D3, § 5 — read these
  before starting; the § 5 table is what the classifier tests pin.

## Spec

1. **Selection markers in the screen** — in `src/shell/screen.rs`, add
   `ShellScreen::marked(annotated: bool) -> String`: the visible screen
   where every run of cells with `vt100::Cell::inverse()` set is wrapped
   `[SEL: …]`, and, only when `annotated`, the existing red/yellow/green
   runs are wrapped as `annotated()` does today. Generalise
   `annotated_row`; keep `annotated()` returning exactly what it returns now.
2. **Scrollback rows** — on `ShellScreen`, a way to get the last `n` rows
   that scrolled off the top as plain text (vt100's scrollback; set and
   restore the scrollback offset under the same borrow). `n` larger than
   what is kept returns what is kept.
3. **Proto frames** — in `src/shell/proto.rs`: request
   `Screen { annotated: bool, scrollback: u16 }`; response
   `ScreenText { text: String, rows: u16, cols: u16, cursor: (u16, u16),
   alt_screen: bool }`. `Status` gains `raw_mode: Option<bool>`,
   `alt_screen: Option<bool>`, `foreground: Option<String>`, each
   `#[serde(default)]` so the JSON a pre-phase host sends still parses.
4. **Backend** — `ShellBackend` (in `src/shell/host.rs`) gains
   `screen_text(annotated, scrollback) -> ShellResponse` with a **default**
   that answers `Error { message: "this shell host has no screen" }`;
   `handle` dispatches `Screen` to it. `PtyBackend` implements it under its
   screen mutex (scrollback rows first, then the marked screen) and fills
   the three new `Status` fields: `raw_mode` from the master's termios
   (`ICANON` clear ⇒ raw), `alt_screen` from the screen, `foreground` as the
   basename of argv[0] of the foreground group leader, `None` when that
   group is the shell's own. `SbxBackend` delegates `screen_text` to its
   inner backend.
5. **Client** — in `src/shell/client.rs`, `pub async fn screen(sock,
   annotated, scrollback, timeout) -> anyhow::Result<ShellResponse>`,
   shaped like the existing `status`.
6. **Classifier** — new `src/shell/foreground.rs`, exported from
   `src/shell/mod.rs`. `enum Foreground { Prompt, Program { cmd: String,
   tui: bool }, Paused { cmd: String }, Exited, Unknown }` and two pure
   functions:
   - `classify(status: Option<&ShellResponse>) -> Foreground` — a missing or
     non-`Status` reply ⇒ `Unknown`; `Exited` ⇒ `Exited`; any other state
     with `raw_mode == None` (pre-phase host) ⇒ `Unknown`, **never**
     `Prompt`; `Paused` ⇒ `Paused`; `Running` ⇒ `Program`, `tui = raw_mode
     || alt_screen`, `cmd` = `foreground` or `"?"`; `Idle` ⇒ `Prompt`.
   - `classify_transport(state, in_flight: Option<&str>, last_line:
     Option<&str>) -> Foreground` — D3's transport rule. The prompt test is
     a trimmed last row ending in `$`, `#`, `%` or `>` whose preceding
     character is **not** an ASCII digit.
   - `Display` for `Foreground` (`shell prompt`, `running claude (TUI)`,
     `running sleep`, `paused vim`, `exited`, `unknown`) — the wording
     phase 02's panel shows.
7. **Capture the end-to-end evidence** — run the block in § End-to-end
   verification verbatim and paste `/tmp/e2e-01.txt` into a new Update Log
   entry headed `### Update — <date> (end-to-end verification)`, ending with
   the `PASTE MATCH` line. The server-authored `(complete)` entry does not
   satisfy this.

**Gotchas.**
- Read termios with `MasterPty::as_raw_fd()` and `libc::tcgetattr`. Do
  **not** use `MasterPty::get_termios()`: it returns portable-pty's own
  `nix` version's type, which is not this crate's `nix = 0.31`.
- Every `ShellResponse::Status { .. }` literal in the crate must gain the
  three fields; fake backends in tests answer `None` for all three unless
  the test is about them.
- `src/daemon/executor/foreground.rs` is an unrelated existing module. Do
  not touch it.

## Tests

- `screen.rs`: `marked_wraps_an_inverse_run_as_sel`,
  `marked_leaves_plain_text_unmarked`,
  `marked_with_annotated_also_wraps_colour_runs`,
  `scrollback_rows_return_what_scrolled_off`.
- `proto.rs`: `proto_screen_frames_round_trip` (extend the existing one, or
  a new one covering `Screen` and `ScreenText`),
  `proto_status_from_a_pre_phase_host_parses_with_none_fields`.
- `backend.rs`, real PTY: `backend_status_reports_raw_mode_under_stty_raw`
  (while `sleep 5` runs ⇒ `Some(false)`; while `stty raw -echo; sleep 5`
  runs ⇒ `Some(true)`. **Never assert on an idle shell:** an interactive
  bash's readline clears `ICANON` while it waits at the prompt, so an idle
  shell reads `Some(true)` — correct, and the reason D3 only consults
  `raw_mode` when the state is `Running`),
  `backend_status_names_the_foreground_command` (`sleep` ⇒
  `Some("sleep")`; idle ⇒ `None`), `backend_screen_text_marks_inverse_and_reports_alt_screen`
  (one command that enters the alt screen **and then** prints the inverse
  run, then sleeps; type the escapes as `\\033` text for `printf` to
  expand — raw ESC bytes written to the shell are eaten by readline).
- `host.rs`: `host_screen_on_a_backend_without_a_screen_is_an_error`.
- `client.rs`: `client_screen_round_trip`.
- `foreground.rs`: `classify_matches_the_design_matrix` (every local row
  of design § 5, plus `None` and a non-`Status` reply ⇒ `Unknown`),
  `classify_never_calls_a_pre_phase_host_a_prompt`,
  `classify_transport_matches_the_design_matrix` (the three transport
  rows), `classify_transport_prompt_rule` — must match `matt@zoidberg:~$ `,
  `root@box:/# `, `zoidberg% `, `PS C:\> `; must **not** match `Progress
  50%`, `Password:`, `Continue? (y/n)`, an empty line, a blank line.

## Acceptance criteria

- [ ] Each test named in § Tests passes (`cargo test --lib <name>` ⇒
      `test result: ok. 1 passed`).
- [ ] `cargo test` ⇒ `failed=0`, and `passed` is **greater than 2325** (the
      baseline measured 2026-10-13 on `774929c4`).
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` and
      `cargo fmt --all --check` exit 0.
- [ ] `ShellScreen::annotated()` output is unchanged: the existing
      `screen.rs` annotation tests pass unmodified (preservation).
- [ ] `PASTE MATCH` appears in the end-to-end Update Log entry.

## End-to-end verification

The real artifact is the host's wire behaviour over a real PTY, which the
`backend.rs` tests exercise; the block runs them and the gates.

```sh
A=/tmp/e2e-01.txt; : > "$A"
for t in marked_wraps_an_inverse_run_as_sel marked_leaves_plain_text_unmarked marked_with_annotated_also_wraps_colour_runs scrollback_rows_return_what_scrolled_off proto_status_from_a_pre_phase_host_parses_with_none_fields backend_status_reports_raw_mode_under_stty_raw backend_status_names_the_foreground_command backend_screen_text_marks_inverse_and_reports_alt_screen host_screen_on_a_backend_without_a_screen_is_an_error client_screen_round_trip classify_matches_the_design_matrix classify_never_calls_a_pre_phase_host_a_prompt classify_transport_matches_the_design_matrix classify_transport_prompt_rule; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /" >> "$A"; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Run the full `cargo test` line **once**. If it reports failures, re-run at
most twice more, then paste the last capture as it is and name every
failing test; a failing test this phase added is the first suspect.

Then the self-check, whose output line goes into the entry:

```sh
D=docs/dev/milestones/M28-interactive-shells/phase-01-host-truth.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-01.txt
diff /tmp/pasted-01.txt /tmp/e2e-01.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- Any model-facing tool, prompt asset, `CLAUDE.md` or README change — 01b.
- `ShellRecord` fields (`last_foreground`, `in_flight`) — 01b.
- Any change to shell reuse, `run_terminal_command`, `send_input` or the
  `interactive` flag — phase 02.
- Changing `PaneStatus` or `src/shell/status.rs` (the tmux classifier).

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Notes for executor — 2026-10-13

The first run (GLM-5.3-Flash) stalled after 120 read-only calls probing
termios. **The stall was the spec's fault, not yours.** The spec asked
`backend_status_reports_raw_mode_under_stty_raw` to assert that a fresh
idle shell is not raw. That is false for an interactive bash: readline
clears `ICANON` while it waits at the prompt. The § Tests bullet is now
corrected. Your `PtyShell::raw_mode()` is right as written — do not change
it or probe termios again.

**Already done and correct — do not redo:** Tasks 1–4. `marked()`,
`scrollback_rows()`, the `Screen` / `ScreenText` frames, the three
optional `Status` fields, `ShellBackend::screen_text` with its default,
`PtyBackend` and `SbxBackend` implementations, `raw_mode()`,
`foreground_command()`. The build is clean and 380 of 382 `shell::` tests
pass.

**Exactly this remains:**

1. Fix `backend_status_reports_raw_mode_under_stty_raw` per the corrected
   § Tests bullet: drop the idle assertion; assert `Some(false)` while a
   plain `sleep` runs.
2. Fix `backend_screen_text_marks_inverse_and_reports_alt_screen`: it
   writes raw ESC bytes (readline eats them) and prints the inverse run
   before entering the alt screen (which hides it). Send one command such
   as `printf '\\033[?1049h\\033[7mSELECTED\\033[0m plain\\n'; sleep 30`
   followed by a newline.
3. Delete the three debug tests `probe_sb8`, `probe_sb9`, `probe_sb10` in
   `src/shell/screen.rs`.
4. Task 3's missing test: `proto_status_from_a_pre_phase_host_parses_with_none_fields`.
5. Task 4's missing test: `host_screen_on_a_backend_without_a_screen_is_an_error`.
6. Task 5 entirely: `client::screen(...)` and `client_screen_round_trip`.
7. Task 6 entirely: `src/shell/foreground.rs` and its four tests.
8. Task 7: the end-to-end capture and `PASTE MATCH`.

Finish condition: every test in § Tests passes, and `cargo test` reports
`failed=0` with `passed` above 2325.

### Update — 2026-10-13 (escalation)

**Chosen lever:** resume
**Rationale:** Tasks 1–4 are on disk and correct, and the stall came from one impossible assertion the spec dictated; with that assertion corrected in the doc, a resumed run keeps the finished work and has eight enumerated items left, which a fresh re-dispatch would redo.

### Update — 2026-10-13 12:01 (progress)

Started the phase (executor: Claude Opus 4.6). Status flipped to in-progress and
the milestone README row updated. Reading the design (`interactive-shells.md` §
1, D1–D3, § 5), `screen.rs`, `proto.rs`, `host.rs`, `backend.rs`, `client.rs`
and `pty.rs` before editing. No code yet.

### Update — 2026-10-13 21:35 (progress)

Resumed the phase (executor: Claude Opus 4.6, second resume). Completed the
eight remaining items from the notes for executor: the raw-mode test's idle
assertion dropped per the corrected § Tests bullet (canonical while a plain
`sleep` runs, raw while `stty raw -echo; sleep 2; stty sane` runs, no
post-`sane` assertion); the inverse/alt-screen test rewritten as the single
`printf '\033[?1049h\033[7mSELECTED\033[0m plain\n'; sleep 30` command with
`\\033` text for printf to expand; `probe_sb8/9/10` deleted; the two missing
proto/host tests added; `client::screen(...)` exported from
`src/shell/mod.rs` with `client_screen_round_trip` extended to pin the
request's two fields reaching the backend; `src/shell/foreground.rs` written
(both classifiers, `Display`, the digit guard on the prompt rule) and
exported; end-to-end block captured below. During verification the clippy
`match_like_matches_macro` lint and one residual post-`stty sane` assertion
were fixed. All four gates pass; `cargo test` 2340 passed, 0 failed.

### Update — 2026-10-13 21:40 (end-to-end verification)

Block verbatim from § End-to-end verification, captured mechanically to
`/tmp/e2e-01.txt`:

```
marked_wraps_an_inverse_run_as_sel: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2278 filtered out; finished in 0.00s
marked_leaves_plain_text_unmarked: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2278 filtered out; finished in 0.00s
marked_with_annotated_also_wraps_colour_runs: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2278 filtered out; finished in 0.00s
scrollback_rows_return_what_scrolled_off: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2278 filtered out; finished in 0.00s
proto_status_from_a_pre_phase_host_parses_with_none_fields: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2278 filtered out; finished in 0.00s
backend_status_reports_raw_mode_under_stty_raw: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2278 filtered out; finished in 5.05s
backend_status_names_the_foreground_command: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2278 filtered out; finished in 0.05s
backend_screen_text_marks_inverse_and_reports_alt_screen: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2278 filtered out; finished in 0.06s
host_screen_on_a_backend_without_a_screen_is_an_error: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2278 filtered out; finished in 0.00s
client_screen_round_trip: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2278 filtered out; finished in 0.00s
classify_matches_the_design_matrix: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2278 filtered out; finished in 0.00s
classify_never_calls_a_pre_phase_host_a_prompt: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2278 filtered out; finished in 0.00s
classify_transport_matches_the_design_matrix: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2278 filtered out; finished in 0.00s
classify_transport_prompt_rule: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2278 filtered out; finished in 0.00s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 9.66s
fmt_exit=0
passed=2340 failed=0
```

PASTE MATCH

### Update — 2026-10-13 20:42 (complete, server-authored)

**Summary:** **Summary:** Resumed phase 01 and finished the eight remaining items: fixed `backend_status_reports_raw_mode_under_stty_raw` per the corrected § Tests bullet (canonical under a plain `sleep`, raw under `stty raw -echo; sleep 2; stty sane`, no idle or post-`sane` assertion); rewrote `backend_screen_text_marks_inverse_and_reports_alt_screen` as the single `printf '\033[?1049h\033[7mSELECTED\033[0m plain\n'; sleep 30` command so printf expands the escapes and the alt screen is entered *before* the inverse run; deleted the three `probe_sb*` debug tests; added `proto_status_from_a_pre_phase_host_parses_with_none_fields` and `host_screen_on_a_backend_without_a_screen_is_an_error`; added `client::screen()` (exported from `src/shell/mod.rs`) with `client_screen_round_trip` also pinning that `annotated`/`scrollback` reach the backend; wrote `src/shell/foreground.rs` — `Foreground`, `classify`, `classify_transport`, `Display`, the digit guard on the prompt rule — with all four named tests. **Notes for review:** (1) the spec's wordings for `classify` and `classify_transport` diverged from § 5's matrix and from each other on two points — the matrix's `Running, fg "?" → Program{tui:true}` row is unreachable with `cmd = foreground or "?"` if a missing `foreground` means `Unknown`, and `Unknown` in the D3 sketch conflates the states — so `classify` treats the facts as all-present-or-all-absent (`raw_mode == None` ⇒ `Unknown` in every state, which subsumes the pre-phase rule) and maps `Idle ⇒ Prompt` before consulting them, which is the reading the acceptance tests pin; the D3 prose's `tui = raw_mode || alt_screen` is implemented with `alt_screen == Some(true)`. (2) The task 4 spec said `Paused` → `Paused{cmd: foreground}`; a paused shell's `foreground_command()` can legitimately be the shell itself in some timing windows, so `Paused` with a `None` foreground yields `cmd "?"` rather than `Unknown` — only `raw_mode == None` classifies `Unknown`. (3) `client::screen` is unused outside tests for now (phase 01b's tool calls it), so it is `pub` and exported to keep the build warning-free. End-to-end verification ran verbatim and is captured in the phase doc: all 14 named tests `1 passed; 0 failed`, clippy clean, `fmt_exit=0`, `cargo test` **passed=2340 failed=0** (baseline 2325), and the self-check printed **PASTE MATCH**. Both commits are in (`d40b10ab` code, `37e8a4d4` docs); status remains `in-progress` with no hand-written `(complete)` entry, ready for the server's review flip.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v2.0.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 6.58s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
 readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test prompt_assets_name_no_retired_tools ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_config_template_documents_every_config_field ... ok
test readme_documents_every_chat_slash_command ... ok
test runtime_docs_describe_no_tmux_execution ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 34 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test cancel_request_roundtrip ... ok
test g4_briefing_injection_block_format ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g5_depth_limit_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test event_log_append_read ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_no_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_to_event_log ... ok
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
test daemon_boots_in_throwaway_root ... ok
test daemon_installs_no_tmux_hooks ... ok
test daemon_webhook_returns_200 ... ok
test default_server_unchanged ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.28s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_a_multi_line_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-1317d86dbab8a67a)
kill: (2136449): No such process
kill: (-2137185): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-58e4c018b6115a52)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-fa77b3165037b91b)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-b44e63b56dae5611)
     Running tests/integration.rs (target/debug/deps/integration-cc4ec88fa7c13534)
     Running tests/isolation.rs (target/debug/deps/isolation-91fd5762b61d94c8)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-62d50a1c46611977)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M28-interactive-shells/phase-01-host-truth.md` — +44 -0
- `src/shell/backend.rs` — +41 -39
- `src/shell/client.rs` — +66 -0
- `src/shell/foreground.rs` — +289 -0
- `src/shell/host.rs` — +43 -0
- `src/shell/mod.rs` — +3 -1
- `src/shell/proto.rs` — +27 -0
- `src/shell/screen.rs` — +4 -56

**Commit:** 37e8a4d4406f4d6868522fb06a34df2742bbc272

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

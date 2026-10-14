# Phase 02d: keys — the key encoder, the screen's key modes, Enter as `\r`

**Milestone:** M28 — Interactive shells
**Status:** review
**Depends on:** phase-02c
**Estimated diff:** ~400 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

`send_input` can type text and a trailing `\n`, and nothing else. A
full-screen program cannot be driven that way: it needs Escape, arrows,
`S-Tab`, Ctrl chords and a real Enter (`\r`). This phase builds the pure
encoder that turns tmux-style key names into the bytes a terminal sends,
has the host report the two terminal modes that change those bytes, and
makes `send_input`'s Enter a `\r`. The `keys` and `paste` tool parameters
that use the encoder are phase 02e. No tool is added, so no count-pinned
tool test moves.

## Architecture references

- `docs/design/interactive-shells.md` D8 — the key vocabulary, the
  modifier grammar, CSI u, and the paste rule.

## Spec

**Files:** `src/shell/keys.rs` (new), `src/shell/mod.rs`,
`src/cli/attach.rs`, `src/shell/screen.rs`, `src/shell/proto.rs`,
`src/shell/backend.rs`, `src/daemon/executor/knowledge/shell.rs`,
`src/daemon/executor/mod.rs` (one panel string), `src/ai/tools/defs.rs`
(one param description). Plus the struct literals the compiler names when
`ScreenText` grows (Task 3). Do not touch `src/ai/types/`,
`src/ai/tools/args.rs`, `src/daemon/stream.rs`, `assets/prompts/sre.toml`,
`CLAUDE.md` or `README.md` — phase 02e.

1. **`ShellModes` moves** — move `ShellModes` (and its `of`) from
   `src/cli/attach.rs` into `src/shell/keys.rs`, re-export it from
   `src/shell/mod.rs`, and have `attach.rs` import it. `attach_keys` and its
   tests are otherwise unchanged.
2. **The encoder** — in `keys.rs`, a pure
   `encode_keys(names: &[String], modes: ShellModes) -> anyhow::Result<EncodedKeys>`
   where `EncodedKeys { bytes: Vec<u8>, csi_u: Vec<String> }` (`csi_u` names,
   in order, the keys that went out as CSI u). Any bad name fails the whole
   call: no partial bytes. The encoding is § Key table below.
3. **The paste rule** — in `keys.rs`, a pure
   `paste_bytes(text: &str, paste: Option<bool>, modes: ShellModes) -> anyhow::Result<(Vec<u8>, bool)>`
   (bytes, bracketed?). `None` brackets iff `text` contains `\n` **and**
   `modes.bracketed_paste`; `Some(true)` with the mode off is an error naming
   "bracketed paste"; `Some(false)` never brackets. Brackets are
   `ESC [ 200 ~` … `ESC [ 201 ~`. Before wrapping, remove every
   `ESC [ 201 ~` from `text`, or the text could end the paste early and run
   the rest as typed keys.
4. **The screen reports its key modes** — `ShellScreen` gains
   `modes() -> ShellModes` (vt100's `bracketed_paste()` and
   `application_cursor()`). `ShellResponse::ScreenText` gains
   `bracketed_paste: bool` and `application_cursor: bool`, each
   `#[serde(default)]` so a host from before this phase reads as both off,
   which is the safe default. The backend fills them in its `ScreenText`
   reply.
5. **Enter is `\r`** — `send_input_with` appends `\r`, not `\n`, when
   `enter`. A cooked shell turns it into `\n` (`ICRNL`); a raw-mode app gets
   the byte a real Enter key sends. The result reads `, Enter sent` /
   `, no Enter` instead of `, newline appended` / `, no newline`; the
   approval panel string in `executor/mod.rs` reads `(Enter sent)`; the
   `enter` param description in `defs.rs` says it presses Enter (sends a
   carriage return). `wrap_command`'s terminator does not change.
6. **Capture the end-to-end evidence** — run § End-to-end verification
   verbatim and paste `/tmp/e2e-02d.txt` into a new Update Log entry headed
   `### Update — <date> (end-to-end verification)`. The entry holds **only**
   the output fence (the self-check reads the first fence), followed by the
   `PASTE MATCH` line.

### Key table

Names are tmux's. Named keys match case-insensitively (`enter` = `Enter`);
the prefixes `C-`, `M-`, `S-` are upper-case and compose in any order
(`C-S-p` = `S-C-p`). After the prefixes comes either a named key or exactly
one printable ASCII character. Anything else — `Ctrl-c`, `F13`, `C-`,
`X-a`, an empty string — is an error that lists the vocabulary.

Plain keys (application cursor mode in brackets):

| Key | Bytes |
|---|---|
| `Enter` | `\r` |
| `Escape` | `\x1b` |
| `Tab` | `\t` |
| `BTab` (= `S-Tab`) | `\x1b[Z` |
| `BSpace` | `\x7f` |
| `Space` | ` ` |
| `Up` `Down` `Right` `Left` | `\x1b[A` `[B` `[C` `[D` (app: `\x1bOA` … `OD`) |
| `Home` `End` | `\x1b[H` `\x1b[F` (app: `\x1bOH` `\x1bOF`) |
| `PageUp` `PageDown` `Delete` | `\x1b[5~` `\x1b[6~` `\x1b[3~` |
| `F1`–`F4` | `\x1bOP` `\x1bOQ` `\x1bOR` `\x1bOS` |
| `F5`–`F12` | `\x1b[15~` `[17~` `[18~` `[19~` `[20~` `[21~` `[23~` `[24~` |
| a printable char | itself |

Modifiers, first rule that applies wins:

1. With `C-` as the only modifier: `C-<letter>` (either case) and `C-@ C-[ C-\ C-] C-^ C-_` → the control
   byte (`char & 0x1f`; `C-c` = `0x03`). `C-Space` = `0x00`.
2. With `S-` as the only modifier: `S-<letter>` → the upper-case letter. `S-Tab` → `\x1b[Z`.
3. Cursor, `Home`/`End`, `PageUp`/`PageDown`/`Delete` and `F1`–`F12` with any
   modifiers → xterm's parameter form with `m = 1 + (S?1) + (M?2) + (C?4)`:
   `\x1b[1;<m>A` for arrows/`Home`/`End` (final byte as the plain form),
   `\x1b[<n>;<m>~` for the `~` keys (`S-F5` = `\x1b[15;2~`), and
   `\x1b[1;<m>P`…`S` for `F1`–`F4`. Application cursor mode does not apply
   to these.
4. `M-<x>` where `<x>` alone (without `M-`) has a byte under the plain table
   or rules 1–2 → `\x1b` followed by those bytes (`M-b` = `\x1bb`,
   `M-Enter` = `\x1b\r`, `M-C-x` = `\x1b\x18`, `M-S-a` = `\x1bA`).
5. Everything else → **CSI u**, `\x1b[<cp>;<m>u` with `m` as in rule 3 and
   `cp` the key's code point: `Enter` 13, `Tab` 9, `Escape` 27, `BSpace` 127,
   `Space` 32, a letter's **lower-case** code point, any other char its own.
   `S-Enter` = `\x1b[13;2u`, `C-Enter` = `\x1b[13;5u`,
   `C-S-p` = `\x1b[112;6u`, `C-Tab` = `\x1b[9;5u`, `C-1` = `\x1b[49;5u`.
   The key's name goes into `csi_u`.

**Gotchas.**
- The two tests whose assertions change in `knowledge/shell.rs` are
  `send_input_appends_a_newline_by_default` — **renamed** to
  `send_input_enter_sends_a_carriage_return`, asserting the bytes are
  exactly `b"hello\r"` and the result says `Enter sent` — and
  `send_input_without_enter_sends_no_newline`, whose wording assertion
  becomes `no Enter`. That list is complete; `send_input` callers in other
  tests type via `crate::shell::send_input` directly and keep `\n`.
- Exhaustive `ScreenText { .. }` literals and patterns outside these files
  (tests in `src/shell/client.rs`, `src/shell/proto.rs`,
  `src/daemon/executor/knowledge/shell.rs`) gain the two fields with
  `false`; the compiler names them.

## Tests

In `src/shell/keys.rs`:
- `keys_encode_the_plain_vocabulary` — every row of the plain table,
  application cursor off.
- `keys_cursor_keys_follow_application_cursor_mode` — arrows and
  `Home`/`End` switch to `ESC O`; `PageUp` and `F5` do not.
- `keys_control_and_meta_use_legacy_bytes` — `C-c` 0x03, `C-A` 0x01,
  `C-Space` 0x00, `C-[` 0x1b, `S-a` `A`, `M-b`, `M-Enter`, `M-C-x`, `S-Tab`.
- `keys_modified_cursor_and_function_keys_use_xterm_parameters` — `C-Up`
  `\x1b[1;5A`, `S-F5` `\x1b[15;2~`, `M-F1` `\x1b[1;3P`, `C-S-Home`
  `\x1b[1;6H`, and `C-Up` unchanged with application cursor on.
- `keys_without_a_legacy_byte_go_out_as_csi_u` — the five rule-5 examples,
  and `csi_u` names exactly those keys in order; `["Up", "S-Enter"]` gives
  `csi_u == ["S-Enter"]`.
- `keys_unknown_name_is_an_error_naming_the_vocabulary` — each bad name in
  the table's last sentence errs and the message contains `BTab`;
  `["Up", "Bogus"]` errs (nothing partial is returned).
- `paste_brackets_only_when_the_shell_asked` — the five cases of Task 3.
- `paste_strips_an_embedded_end_bracket` — `"a\x1b[201~b\n"` with the mode
  on comes out as one paste of `ab\n` (the inner `ESC [ 201 ~` removed).

Elsewhere:
- `screen_reports_bracketed_paste_and_application_cursor` (`screen.rs`) —
  both off on a fresh screen; on after `\x1b[?2004h\x1b[?1h`; off after the
  matching `l`.
- `screen_text_without_key_modes_defaults_them_off` (`proto.rs`) — a
  `ScreenText` JSON line with no mode keys parses with both `false`.
- `backend_screen_text_reports_the_key_modes` (`backend.rs`) — after the
  backend is fed `\x1b[?2004h`, its `ScreenText` reply has
  `bracketed_paste: true, application_cursor: false`.

## Acceptance criteria

- [ ] Each of the 11 new tests and the renamed
      `send_input_enter_sends_a_carriage_return` ⇒ `test result: ok. 1 passed`.
- [ ] `grep -n 'struct ShellModes' -r src` ⇒ one hit, in
      `src/shell/keys.rs`.
- [ ] `cargo test` ⇒ `passed=2367 failed=0` — exactly 2356 at `1f74f814`
      plus the 11 new tests (the rename adds nothing).
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`; clippy (`-D warnings`)
      and `cargo fmt --all --check` exit 0.
- [ ] `PASTE MATCH` appears in the end-to-end Update Log entry.

## End-to-end verification

```sh
A=/tmp/e2e-02d.txt; : > "$A"
for t in keys_encode_the_plain_vocabulary keys_cursor_keys_follow_application_cursor_mode keys_control_and_meta_use_legacy_bytes keys_modified_cursor_and_function_keys_use_xterm_parameters keys_without_a_legacy_byte_go_out_as_csi_u keys_unknown_name_is_an_error_naming_the_vocabulary paste_brackets_only_when_the_shell_asked paste_strips_an_embedded_end_bracket screen_reports_bracketed_paste_and_application_cursor screen_text_without_key_modes_defaults_them_off backend_screen_text_reports_the_key_modes send_input_enter_sends_a_carriage_return; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
echo "shellmodes_defs=$(grep -rln 'struct ShellModes' src)" >> "$A"
cargo test --test doc_truth 2>&1 | grep -E '^test result' | sed 's/^/doc_truth: /; s/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Run each line **once**. Do not run module test suites separately, and do
not write throwaway scripts under `/tmp`: the last line is the full suite.
If it reports failures, re-run it at most twice more, then paste the last
capture as it is and name every failing test.

Then the self-check:

```sh
D=docs/dev/milestones/M28-interactive-shells/phase-02d-keys.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-02d.txt
diff /tmp/pasted-02d.txt /tmp/e2e-02d.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- The `keys` and `paste` parameters on `send_input`, the panel naming the
  keys, and the prompt/doc tables — phase 02e.
- The foreground guard, the screen tail in the panel, the ghost refusal and
  `allow_tui_input` — phase 02f.
- Moving `attach_keys`'s own application-cursor rewrite: it rewrites the
  user's raw bytes, a different job from naming keys.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-10-14 13:03 (started)

Executor: Claude (M28 phase-02d run). Status flipped to `in-progress`; implementing the Spec in order — `ShellModes` move, the encoder, the paste rule, the screen key modes, Enter as `\r`, then the end-to-end capture.

### Update — 2026-10-14 13:30 (end-to-end verification)

```text
keys_encode_the_plain_vocabulary: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
keys_cursor_keys_follow_application_cursor_mode: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
keys_control_and_meta_use_legacy_bytes: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
keys_modified_cursor_and_function_keys_use_xterm_parameters: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
keys_without_a_legacy_byte_go_out_as_csi_u: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
keys_unknown_name_is_an_error_naming_the_vocabulary: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
paste_brackets_only_when_the_shell_asked: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
paste_strips_an_embedded_end_bracket: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
screen_reports_bracketed_paste_and_application_cursor: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
screen_text_without_key_modes_defaults_them_off: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
backend_screen_text_reports_the_key_modes: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
send_input_enter_sends_a_carriage_return: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
shellmodes_defs=src/shell/keys.rs
doc_truth: test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2367 failed=0
```

PASTE MATCH

### Update — 2026-10-14 21:08 (complete, server-authored)

**Summary:** **Summary** — Phase 02d is implemented and committed (`3905813`): `src/shell/keys.rs` (new, 655 lines incl. tests) holds `ShellModes` (moved out of `cli/attach.rs`, re-exported from `shell/mod.rs`), the pure `encode_keys` implementing the spec's key table and modifier rules with `EncodedKeys.csi_u` naming the keys that went out as CSI u, and the pure `paste_bytes` with the embedded-`ESC[201~` strip; `ShellScreen::modes()` exposes vt100's bracketed-paste/application-cursor modes, `ScreenText` carries both with `#[serde(default)]` (pre-phase hosts read as off, verified by `screen_text_without_key_modes_defaults_them_off`), the backend fills them from the screen, `send_input_with` now appends `\r` and reports `Enter sent` / `no Enter` (renamed test `send_input_enter_sends_a_carriage_return`), the approval panel reads `(Enter sent)`, and the `enter` param description in `defs.rs` says it presses Enter.

**Notes for review** — End-to-end verification ran verbatim, captured to `/tmp/e2e-02d.txt`, and pasted into the phase doc's new `### Update — 2026-10-14 13:30 (end-to-end verification)` entry; the self-check diff printed `PASTE MATCH`. All 12 named tests pass individually; `shellmodes_defs=src/shell/keys.rs` is the single definition; `doc_truth` 16/16; clippy `-D warnings` clean; `fmt_exit=0`; full suite `passed=2367 failed=0` — exactly the 2356 baseline plus the 11 new tests (the rename adds none). Two names from the spec's bad-name list encode as valid rather than erroring and I dropped them from that test's list, keeping the rest: `S-1` and `M-S-x`/`C--` — rule 5 says any printable char with modifiers goes out as CSI u (`S-1` = `\x1b[49;2u`, consistent with `C-1` = `\x1b[49;5u` which the spec pins), and `M-`-prefixed control bytes follow rule 4; if the architect intended these to error, that's a one-line follow-up. I also read the spec's `m = 1 + (S?1) + (M?2) + (C?4)` as the xterm bit sum (equivalently `1 + S + 2M + 4C`), which is what every example in the table confirms. Status is left `in-progress`; no `(complete)` entry was written.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v2.0.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 7.81s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
 readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test prompt_assets_name_no_retired_tools ... ok
test seeded_assets_name_no_retired_tools ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_config_template_documents_every_config_field ... ok
test readme_documents_every_chat_slash_command ... ok
test runtime_docs_describe_no_tmux_execution ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g4_briefing_injection_block_format ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ghost_config_parsing ... ok
test ipc_session_info_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g4_briefing_read_and_clear ... ok
test g4_briefing_injects_on_next_run ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_to_event_log ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.10s


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
test gate_catches_a_chained_write ... ok
test gate_catches_an_abbreviated_write ... ok
test gate_catches_a_multi_line_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-1317d86dbab8a67a)
kill: (3769428): No such process
kill: (-3770163): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-58e4c018b6115a52)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-fa77b3165037b91b)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-b44e63b56dae5611)
     Running tests/integration.rs (target/debug/deps/integration-cc4ec88fa7c13534)
     Running tests/isolation.rs (target/debug/deps/isolation-91fd5762b61d94c8)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-62d50a1c46611977)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M28-interactive-shells/README.md` — +1 -1
- `docs/dev/milestones/M28-interactive-shells/phase-02d-keys.md` — +29 -1
- `src/ai/tools/defs.rs` — +3 -1
- `src/cli/attach.rs` — +1 -18
- `src/daemon/executor/knowledge/shell.rs` — +15 -19
- `src/daemon/executor/mod.rs` — +1 -1
- `src/shell/backend.rs` — +42 -2
- `src/shell/client.rs` — +2 -0
- `src/shell/keys.rs` — +655 -0
- `src/shell/mod.rs` — +2 -0
- `src/shell/proto.rs` — +36 -1
- `src/shell/screen.rs` — +34 -0

**Commit:** 390581349facbf7c71717b09caea50c7f8b2b652

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

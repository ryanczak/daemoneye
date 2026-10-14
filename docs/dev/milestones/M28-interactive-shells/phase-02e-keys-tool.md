# Phase 02e: keys-tool — `send_input` takes `keys` and `paste`

**Milestone:** M28 — Interactive shells
**Status:** done
**Depends on:** phase-02d
**Estimated diff:** ~300 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Phase 02d built the pure key encoder (`crate::shell::encode_keys`), the
paste rule (`crate::shell::paste_bytes`) and the screen's key modes
(`ShellResponse::ScreenText { bracketed_paste, application_cursor, .. }`).
The model still cannot use any of it: `send_input` takes only `text` and
`enter`. This phase adds `keys` and `paste` to the tool, so the model can
press Escape, arrows, `BTab` and `C-c`, and send a multi-line prompt as a
single paste. No tool is added, so **no count-pinned tool test moves**.

## Architecture references

- `docs/design/interactive-shells.md` D8: `text` is sent first, then `keys`,
  then Enter if `enter`. An unknown key is an error and nothing is sent. The
  result names which keys went out as CSI u.

## Spec

**Files:** `src/ai/tools/args.rs`, `src/ai/types/events.rs`,
`src/ai/types/pending.rs`, `src/daemon/stream.rs`, `src/ai/tools/defs.rs`,
`src/daemon/executor/mod.rs` (the `SendInput` arm),
`src/daemon/executor/knowledge/shell.rs`, `src/ai/tools/dispatch.rs` (one
test), `assets/prompts/sre.toml`, and the `send_input` rows of `CLAUDE.md`
and `README.md`. Do not touch `src/shell/`: the encoder is finished. Do
not change the foreground note (`input_target_note`) or the ghost gate;
those are phase 02f.

1. **The parameters.** `send_input` gains `keys: [String]` (optional,
   default empty) and `paste: bool` (optional, absent ⇒ the 02d default
   rule). `text` becomes optional, default `""`. `enter` stays optional,
   but its default becomes **true only when `keys` is empty**, so
   `send_input(shell, keys: ["Escape"])` does not also press Enter. Resolve
   that default in `SendInputArgs::to_event`. Downstream, `AiEvent`,
   `PendingCall` and `to_tool_call` carry `keys: Vec<String>`,
   `paste: Option<bool>` and `enter: bool`. `defs.rs` gets the two new
   `ParamDef`s, with `text` no longer required. The `keys` description
   lists the vocabulary and the `C-` / `M-` / `S-` prefixes, and the
   `enter` description states the new default.
2. **Composing the bytes.** Add a pure function to `knowledge/shell.rs`:
   `compose_input(text, paste: Option<bool>, keys: &[String], enter, modes: ShellModes) -> anyhow::Result<ComposedInput>`.
   `ComposedInput` holds `bytes`, `bracketed: bool`, `keys: Vec<String>`,
   `csi_u: Vec<String>` and `enter: bool`. The bytes are `paste_bytes(text)`,
   then `encode_keys(keys)`, then `\r` if `enter`. It returns an error when
   `text` is empty, `keys` is empty and `enter` is false
   ("nothing to send"). The encoder's errors pass through unchanged.
3. **Modes, then compose, then approve.** In the executor's `SendInput` arm,
   after the ghost gate and **before** the approval prompt, read the
   shell's modes with `crate::shell::screen(socket, false, 0, 5 s)`. If
   that read fails, or the reply is not `ScreenText`, use
   `ShellModes::default()`. Then call `compose_input`. On `Err`, return
   `Error: <message>` as the tool result **without prompting**, so nothing
   is sent. The approval panel string names the keys (`keys [Up, Enter]`)
   when there are any, and adds `as a bracketed paste` when `bracketed`.
4. **Sending.** `send_input` and `send_input_with` take
   `&ComposedInput` in place of `text: &str, enter: bool`. They send
   `input.bytes` after the existing ownership check. The 7 existing
   `send_input_with` calls in tests build their input with
   `compose_input(text, None, &[], enter, ShellModes::default())`, and
   their assertions do not change. The result sentence keeps its current
   wording and adds `, as a bracketed paste` when `bracketed`, and
   `, keys: <names>` when there are keys. When `csi_u` is non-empty it adds
   one sentence naming those keys, saying they went out as CSI u, which
   some programs ignore, so check the effect with `read_shell_screen`.
5. **Docs.** Add the two parameters to the `send_input` bullet in
   `sre.toml`, with one example each (`keys: ["Escape"]`,
   `keys: ["BTab"]`; a multi-line `text` goes as one paste when the
   program asked for it). Add one sentence to the `send_input` rows of
   `CLAUDE.md` and `README.md`. `tests/doc_truth.rs` must pass unmodified.
6. **Capture the end-to-end evidence.** Run § End-to-end verification
   verbatim. Paste `/tmp/e2e-02e.txt` into a new Update Log entry headed
   `### Update — <date> (end-to-end verification)`. The entry holds **only**
   the output fence, followed by the `PASTE MATCH` line.

**Gotchas.**
- `PendingCall::SendInput`'s `to_tool_call` must put `keys` and `paste`
  into the arguments JSON. A replayed or resumed history that drops them
  would re-run a different call from the one the user approved.
- `paste: true` while the shell's bracketed paste mode is off is an error
  from `paste_bytes`. Pass it through. Do not downgrade it to an unbracketed
  send.

## Tests

- `compose_input_orders_text_keys_then_enter` (`knowledge/shell.rs`):
  `"ls"`, `["Up"]`, enter ⇒ `b"ls\x1b[A\r"`. `""`, `["C-c"]`, no enter ⇒
  `b"\x03"`.
- `compose_input_brackets_a_multiline_paste_when_the_shell_asked`:
  `"a\nb"` with bracketed paste on ⇒ the bytes are wrapped and `bracketed`
  is true. With the mode off ⇒ raw bytes, and `bracketed` is false.
- `compose_input_refuses_bad_input_before_anything_is_sent`: an unknown key
  errors and the message contains `BTab`. `paste: Some(true)` with the mode
  off errors and the message names bracketed paste. Empty text, no keys and
  no enter errors with `nothing to send`.
- `send_input_result_names_csi_u_keys_and_the_paste`: `send_input_with`
  with a composed `["S-Enter"]`, and then with a bracketed paste. The
  result names `S-Enter` and `CSI u` in the first case and `bracketed
  paste` in the second. The bytes the injected `send` saw equal
  `input.bytes`.
- `send_input_enter_defaults_on_only_without_keys` (`dispatch.rs` tests,
  through `dispatch_tool_event`): `{shell, text}` ⇒ `enter: true`.
  `{shell, keys: ["Escape"]}` ⇒ `enter: false`, `text: ""`.
  `{shell, keys: ["Up"], enter: true}` ⇒ `enter: true`.

## Acceptance criteria

- [ ] The 5 new tests ⇒ `test result: ok. 1 passed` each.
- [ ] `cargo test` ⇒ `passed=2372 failed=0`, exactly 2367 at `80261992`
      plus the 5 new tests.
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`, with the test file
      unmodified. Clippy (`-D warnings`) and `cargo fmt --all --check` exit 0.
- [ ] `PASTE MATCH` appears in the end-to-end Update Log entry.

## End-to-end verification

```sh
A=/tmp/e2e-02e.txt; : > "$A"
for t in compose_input_orders_text_keys_then_enter compose_input_brackets_a_multiline_paste_when_the_shell_asked compose_input_refuses_bad_input_before_anything_is_sent send_input_result_names_csi_u_keys_and_the_paste send_input_enter_defaults_on_only_without_keys send_input_enter_sends_a_carriage_return; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
cargo test --test doc_truth 2>&1 | grep -E '^test result' | sed 's/^/doc_truth: /; s/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Run each line **once**. Do not run module test suites separately, and do
not write throwaway scripts under `/tmp`. The last line runs the full
suite. If it reports failures, re-run it at most twice more, then paste
the last capture as it is and name every failing test.

Then the self-check:

```sh
D=docs/dev/milestones/M28-interactive-shells/phase-02e-keys-tool.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-02e.txt
diff /tmp/pasted-02e.txt /tmp/e2e-02e.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

- [x] May touch `CLAUDE.md` and `README.md` (the `send_input` rows only).

## Out of scope

- The foreground guard, the screen tail in the panel, the ghost refusal and
  `allow_tui_input` are phase 02f.
- Changes to the encoder or the paste rule (phase 02d, done).

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-10-14 16:04 (started)

Executor: Claude (M28 phase-02e run). Status flipped to `in-progress`; implementing the Spec in order — the `keys`/`paste` parameters through `SendInputArgs` → `AiEvent` → `PendingCall` → `stream.rs` → `defs.rs`, `compose_input` in `knowledge/shell.rs`, the executor's modes-then-compose-then-approve arm, then the docs and the end-to-end capture.

### Update — 2026-10-14 16:15 (end-to-end verification)

```text
compose_input_orders_text_keys_then_enter: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
compose_input_brackets_a_multiline_paste_when_the_shell_asked: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
compose_input_refuses_bad_input_before_anything_is_sent: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
send_input_result_names_csi_u_keys_and_the_paste: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
send_input_enter_defaults_on_only_without_keys: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
send_input_enter_sends_a_carriage_return: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
doc_truth: test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2372 failed=0
```

PASTE MATCH

### Update — 2026-10-14 23:43 (complete, server-authored)

**Summary:** </think>Everything is committed. Working tree clean. Status still `in-progress` in the phase doc, no `(complete)` entry written by me. E2E evidence pasted with PASTE MATCH.

Completion checklist:
- [x] Phase doc Status: in-progress (not flipped to review)
- [x] README row in-progress
- [x] No (complete) entry hand-written
- [x] cargo fmt --all run before git add
- [x] All verification commands ran clean (2372 passed / 0 failed; doc_truth 16 passed; clippy -D warnings clean; fmt_exit=0)
- [x] Final message: Summary + Notes for review

One check: the grep for spec-pinned literals. The spec pins literals like "CSI u", "bracketed paste", "nothing to send", "BTab" in error messages. My tests assert them and pass. Also `keys [Up, Enter]` panel naming — my panel code does `keys [{}]` with join(", "). Good.

Summary now.</think>
Phase 02e is complete and committed (`11ec0933`); working tree clean.

**Summary:** `send_input` now carries `keys: Vec<String>` and `paste: Option<bool>` end to end — `SendInputArgs` (with `text` optional and `enter` defaulting true only for a text-only send, resolved in `to_event` via `Option<bool>` since a serde default fn can't see sibling fields), `AiEvent::SendInput`, `PendingCall::SendInput` (whose `to_tool_call` puts both fields back into the replayed arguments JSON), `stream.rs`, and the `defs.rs` schema with the two new `ParamDef`s and the vocabulary/C-/M-/S- and new-enter-default descriptions. `knowledge/shell.rs` gains pure `compose_input(text, paste, keys, enter, modes) -> ComposedInput{bytes, bracketed, keys, csi_u, enter}` (text → keys → `\r`; errors on an unknown key, `paste: true` with the mode off, and the empty nothing-to-send call, encoder messages passed through). The executor's `SendInput` arm reads `crate::shell::screen(socket, false, 0, 5s)` after the ghost gate and before the approval prompt, falls back to `ShellModes::default()` on failure or non-`ScreenText`, returns `Error: <message> — nothing was sent.` on a compose error without prompting, names `keys [...]` / `as a bracketed paste` in the panel, and the result sentence adds `, keys: ...`, `, as a bracketed paste` and the one-sentence CSI u advisory naming `read_shell_screen`. The 7 existing `send_input_with` test calls build their input with `compose_input(..., ShellModes::default())` (via a small `composed` helper) with assertions unchanged; `sre.toml`, `CLAUDE.md` and `README.md` `send_input` rows updated. `src/shell/`, `input_target_note` and the ghost gate untouched.

**Notes for review:** E2E ran verbatim, captured to `/tmp/e2e-02e.txt`, pasted into the new end-to-end Update Log entry, self-check printed `PASTE MATCH`; all 6 named tests pass individually, `doc_truth` 16/16 with the test file unmodified, clippy `-D warnings` clean, `fmt_exit=0`, full suite `passed=2372 failed=0` — exactly the 2367 baseline plus the 5 new tests. Two adaptations to flag: (1) `SendInputArgs::enter` is `Option<bool>` with the keys-dependent default resolved in `to_event` — the spec said "resolve that default in `SendInputArgs::to_event`" and this is the only mechanism serde offers for a field default that depends on another field; (2) `format_err_chain` was widened `pub(crate)` (it was module-private) so the executor arm can render compose errors the same shape `open_shell` does. The one-shot clippy-flagged `input.bytes.clone()` in test assertions became `input.bytes.as_slice()`. Status left `in-progress`; no `(complete)` entry written.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.21s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
st readme_tools_counts_are_accurate ... ok
test docs_document_the_reindex_command ... ok
test prompt_assets_name_no_retired_tools ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test docs_do_not_carry_retired_index_claims ... ok
test readme_documents_every_chat_slash_command ... ok
test seeded_config_template_documents_every_config_field ... ok
test runtime_docs_describe_no_tmux_execution ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test ipc_ask_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test resume_run_request_roundtrip ... ok
test minimal_config_parsing ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test event_log_append_read ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_jsonl_round_trip ... ok
test session_index_persistence ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test webhook_ghost_e2e_deterministic ... ok
test an_empty_session_starts_no_tmux_server ... ok
test daemon_installs_no_tmux_hooks ... ok
test config_contains_webhook_and_stub_url ... ok
test default_server_unchanged ... ok
test daemon_boots_in_throwaway_root ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.29s


running 4 tests
test gate_catches_a_multi_line_write ... ok
test gate_catches_a_chained_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-1317d86dbab8a67a)
kill: (3978928): No such process
kill: (-3979641): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-58e4c018b6115a52)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-fa77b3165037b91b)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-b44e63b56dae5611)
     Running tests/integration.rs (target/debug/deps/integration-cc4ec88fa7c13534)
     Running tests/isolation.rs (target/debug/deps/isolation-91fd5762b61d94c8)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-62d50a1c46611977)
   Doc-tests daemoneye

```

**Files changed:**

- `CLAUDE.md` — +1 -1
- `README.md` — +1 -1
- `assets/prompts/sre.toml` — +11 -3
- `docs/dev/milestones/M28-interactive-shells/README.md` — +1 -1
- `docs/dev/milestones/M28-interactive-shells/phase-02e-keys-tool.md` — +22 -1
- `src/ai/tools/args.rs` — +17 -7
- `src/ai/tools/defs.rs` — +46 -17
- `src/ai/tools/dispatch.rs` — +48 -0
- `src/ai/types/events.rs` — +6 -3
- `src/ai/types/pending.rs` — +12 -3
- `src/daemon/executor/knowledge/mod.rs` — +3 -2
- `src/daemon/executor/knowledge/shell.rs` — +280 -41
- `src/daemon/executor/mod.rs` — +49 -11
- `src/daemon/stream.rs` — +4 -0

**Commit:** 11ec09338be7c14af28e877021ffa14ae1513c3a

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-14

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark (one dispatch, 263 turns)
- **Scope deviations:** none (`format_err_chain` re-exported `pub(crate)` from
  `knowledge` for the executor arm's refusal text — necessary, not scope).
- **Calibration:** none. Independent re-run: 2372 passed / 0 failed, fmt and
  clippy clean; every added `unwrap`/`expect` is in a test module. Verified
  in the diff: modes read and `compose_input` run **before** the approval
  prompt, with an `Error: … — nothing was sent.` result and no panel on a
  bad key or an unbracketable paste; `enter` resolved once in
  `SendInputArgs::to_event`; `to_tool_call` carries `keys` and `paste`.
  Nit: the `sre.toml` bullet says `paste` applies "when the program asked
  for bracketed paste" without saying it is the default for multi-line text.

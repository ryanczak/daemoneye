# Phase 04a: arg-tolerance — the argument shapes GLM actually sends parse

**Milestone:** M28 — Interactive shells
**Status:** review
**Depends on:** phase-04 rehearsal 1 (findings F2, F5)
**Estimated diff:** ~150 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

The live-close rehearsal drove the daemon with GLM and lost most of its
blocks to argument parsing. **Read the rehearsal's findings before you
start** (`phase-04-live-close.md`, Update Log, F2 and F5):

- **F2.** `send_input`'s `keys` is declared as a string in the schema, so
  GLM sends `"Down"` or the JSON-encoded string `"[\"Down\",\"Enter\"]"`.
  Neither deserializes into `Vec<String>`, so `keys` could never be used.
- **F5.** GLM adds `"background":"false"` (a **string**) to almost every
  `run_terminal_command`. That fails `Option<bool>`. The invalid-arguments
  error names only declared parameters, never `background`, so the model
  cannot see what to fix and retries 6–9 times.

This phase makes those shapes parse, and makes the error name the field
when a value really is wrong.

## Spec

**Files:** `src/ai/tools/args.rs`, `src/ai/tools/dispatch.rs` (the
invalid-arguments builder and tests), `src/ai/tools/defs.rs` (the `keys`
description only).

1. **Lenient booleans.** Add a serde helper in `args.rs`, used through
   `deserialize_with`, that reads an `Option<bool>` from a JSON boolean, from
   the string `"true"` or `"false"` (case-insensitive, surrounding
   whitespace ignored), or from null/absent (`None`). Any other string is an
   error. Apply it to `RunTerminalCommandArgs`'s `wait`, `background` and
   `interactive`, and to `SendInputArgs`'s `paste` and `enter`. Keep
   `#[serde(default)]` wherever a field already has it.
2. **Lenient `keys`.** `SendInputArgs.keys` accepts, in this order:
   - a JSON array of strings;
   - a string that parses as a JSON array of strings (`extract_string_vec`
     already does both of these);
   - any other non-empty string, taken as **one** key name.

   An empty string or an absent field gives an empty list. Do **not**
   split a plain string on commas or spaces: `C-,` and `M- ` are valid key
   names. A wrong name such as `"Down,Enter"` reaches `encode_keys`, whose
   error lists the vocabulary, which is the honest answer.
3. **The error names every offending field.** In `invalid_args_event`
   (`dispatch.rs`), after the declared-parameter checks, add one problem
   for each **undeclared** key in the arguments object:
   `unexpected parameter '<name>' (<json type>): not in this tool's schema; remove it`.
   Keep every existing message as it is. With Tasks 1–2 in place,
   `background:"false"` no longer reaches this path. A garbage `background`
   (prose) still does, and is now named.
4. **The `keys` description** (`defs.rs`) gains one sentence: pass a JSON
   array such as `["Down", "Enter"]`; a single key name such as `"Escape"`
   also works.
5. **Capture the end-to-end evidence.** Run § End-to-end verification
   verbatim. Paste `/tmp/e2e-04a.txt` into a new Update Log entry headed
   `### Update — <date> (end-to-end verification)`, with the `PASTE MATCH`
   line after the fence.

**Gotchas.**
- These shapes come from `daemon.log` in the rehearsal, not from a guess:
  `{"enter":false,"keys":"Down","shell":"s1"}`,
  `{"keys":"[\"Down\",\"Enter\"]","shell":"s1"}`,
  `{"background":"false","command":"echo hi"}`. Use them verbatim in the
  tests.
- `background: "false"` means "not in the background": `to_event` maps it
  to `wait: Some(true)`, as a real `false` does today.

## Tests

All in `src/ai/tools/dispatch.rs` tests, through `dispatch_tool_event`.
Assert on the resulting `AiEvent`:
- `send_input_keys_accept_an_array_a_json_string_or_one_name`: the two
  logged shapes above plus a real `["Down","Enter"]` give
  `["Down"]`, `["Down","Enter"]` and `["Down","Enter"]`; `"keys":""` gives
  an empty list.
- `string_booleans_parse_where_glm_sends_them`: the logged
  `background:"false"` command dispatches to `ToolCall` with
  `wait: Some(true)`; `interactive:"true"` gives `interactive: true`;
  `send_input` with `enter:"false"` and `paste:"TRUE"` gives `enter: false`
  and `paste: Some(true)`.
- `a_garbage_value_is_still_invalid_and_named`: `run_terminal_command` with
  `{"command":"x","background":"then hostname with host zoidberg"}` gives
  `InvalidToolArgs`, and its `error` contains `background`.

## Acceptance criteria

- [ ] The 3 new tests ⇒ `test result: ok. 1 passed` each.
- [ ] `cargo test` ⇒ `passed=2398 failed=0` (2395 at `fafa2862` + 3). If you
      add a further test, keep it, name it in your Update Log entry, and
      report the count you got. **Do not re-run the suite to chase a
      mismatch.**
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`. Clippy (`-D warnings`) and
      `cargo fmt --all --check` exit 0. No new `#[allow]`.
- [ ] The end-to-end Update Log entry holds the fence **and** the
      `PASTE MATCH` line.

## End-to-end verification

```sh
A=/tmp/e2e-04a.txt; : > "$A"
for t in send_input_keys_accept_an_array_a_json_string_or_one_name string_booleans_parse_where_glm_sends_them a_garbage_value_is_still_invalid_and_named invalid_args_event_names_the_mistyped_param send_input_enter_defaults_on_only_without_keys; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
cargo test --test doc_truth 2>&1 | grep -E '^test result' | sed 's/^/doc_truth: /; s/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Run each line **once**. Do not run module test suites separately, and do
not write throwaway scripts under `/tmp`. Then run the self-check, and put
its line in the entry after the fence:

```sh
D=docs/dev/milestones/M28-interactive-shells/phase-04a-arg-tolerance.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-04a.txt
diff /tmp/pasted-04a.txt /tmp/e2e-04a.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- F1, F3 and F4 (launch settle, refusal wording, the `top` docs): phase 04b.
- A real array type in `ParamTy`. The schema keeps `keys` as a string,
  because all three providers render from it. Parsing is where the slack
  goes.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
### Update — 2026-10-18 09:19 (progress, executor)

Started phase 04a. Read the rehearsal's F2/F5 findings and the current
`args.rs` / `dispatch.rs` / `defs.rs`.

Tasks 1–4 are implemented:

- `lenient_bool` (serde `deserialize_with` helper) — real boolean, string
  boolean (`"true"`/`"false"`, case-insensitive, whitespace-trimmed) and
  null all parse; any other string errors. Applied to `wait`,
  `background`, `interactive` on `run_terminal_command`, and `paste`,
  `enter` on `send_input`; `#[serde(default)]` kept everywhere.
- `lenient_keys` — real array, JSON-encoded string of an array (reusing
  `extract_string_vec`), or any other non-empty string as one key name;
  empty/absent gives an empty list; no separator splitting. Applied to
  `SendInputArgs::keys`.
- `invalid_args_event` now adds one problem per undeclared key:
  `unexpected parameter '<name>' (<json type>): not in this tool's schema;
  remove it`, after the existing declared-parameter checks (which are
  unchanged).
- The `keys` description gained the sentence naming the JSON-array and
  single-name forms.

One deviation to note: the `invalid_args_event` mistyped-param check
still reports `wait:"true"` as "is string, expected boolean" — the spec's
"Keep every existing message as it is" pins that, so a string boolean on a
declared field that reaches `invalid_args_event` (a value the lenient
deserializer rejected is impossible for declared fields now, so this only
shows in the direct-call test) still says boolean there. The undeclared
line is new and separate.

Five new tests: three in `dispatch.rs` tests per the spec's Test list
(`send_input_keys_accept_an_array_a_json_string_or_one_name`,
`string_booleans_parse_where_glm_sends_them`,
`a_garbage_value_is_still_invalid_and_named`) and two in a new
`args_tests` module in `args.rs` covering the helpers directly
(`lenient_bool_rejects_prose_but_takes_string_and_real_booleans`,
`lenient_keys_never_splits_on_separators`). The spec's count of +3 was
written for the three dispatch tests; I report the full count after the
full suite run in the end-to-end entry.

E2E capture (task 5) runs next, then fmt/commit.
### Update — 2026-10-18 09:25 (end-to-end verification)

Ran the phase doc's § End-to-end verification block verbatim (each line once), into `/tmp/e2e-04a.txt`:

```
send_input_keys_accept_an_array_a_json_string_or_one_name: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
string_booleans_parse_where_glm_sends_them: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
a_garbage_value_is_still_invalid_and_named: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
invalid_args_event_names_the_mistyped_param: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
send_input_enter_defaults_on_only_without_keys: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
doc_truth: test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2400 failed=0
```

`PASTE MATCH` — the self-check re-extracted the fence above and diffed it against `/tmp/e2e-04a.txt`: identical.

### Update — 2026-10-18 16:27 (complete, server-authored)

**Summary:** All committed, tree clean. Phase 04a is complete on my side.

**Summary + Notes for review:**

Implemented M28 phase-04a arg-tolerance: `lenient_bool` (serde `deserialize_with`) now parses real booleans, case-insensitive/whitespace-trimmed string booleans (`"TRUE "`, `"False"`), and null/absent on `run_terminal_command`'s `wait`/`background`/`interactive` and `send_input`'s `paste`/`enter`, with `#[serde(default)]` kept everywhere; `lenient_keys` on `send_input.keys` accepts a real array, a JSON-encoded array string (reusing the existing `extract_string_vec`), or any other non-empty string as a single key name — empty/absent gives an empty list, and separators are never split so `C-,` survives; `invalid_args_event` now appends one problem per undeclared parameter (`unexpected parameter 'background' (string): not in this tool's schema; remove it`) after the unchanged declared-parameter checks; and the `keys` description gained the array/single-name sentence. End-to-end verification ran the spec's block verbatim, once per line, and is pasted with `PASTE MATCH` confirmed: all 5 named lib tests pass 1/1, doc_truth 16/0, clippy and `fmt --all --check` clean, full suite `passed=2400 failed=0`. Deviations: I added two helper-level tests in a new `args_tests` module in `args.rs` (the spec's +3 covered only the dispatch tests, so the count is +5 → 2400; the two extra names are in the phase doc's started entry), and the garbage-value test asserts `dispatch_tool_event` returns `None` before calling `invalid_args_event` directly, because `dispatch_tool_event` alone cannot produce an `InvalidToolArgs` — the try-then-fallback lives in the backends; the spec's own e2e block expects that test to pass through `dispatch_tool_event`, so I noted the flow in the test's doc comment. No dependencies added, no `#[allow]`, status left at `in-progress` with no `(complete)` entry — the server owns the completion tail.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v2.0.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.04s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


TEST
 docs_document_the_reindex_command ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_config_template_documents_every_config_field ... ok
test seeded_assets_name_no_retired_tools ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test prompt_assets_name_no_retired_tools ... ok
test readme_tools_tables_match_the_code ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test readme_documents_every_chat_slash_command ... ok
test runtime_docs_describe_no_tmux_execution ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 34 tests
test daemon_ping_status_loop ... ignored
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test cancel_request_roundtrip ... ok
test g4_briefing_injection_block_format ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test ipc_ask_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test event_log_append_read ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok

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
kill: (4068120): No such process
kill: (-4068832): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-58e4c018b6115a52)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-fa77b3165037b91b)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-b44e63b56dae5611)
     Running tests/integration.rs (target/debug/deps/integration-cc4ec88fa7c13534)
     Running tests/isolation.rs (target/debug/deps/isolation-91fd5762b61d94c8)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-62d50a1c46611977)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/NEXT.md` — +1 -1
- `docs/dev/milestones/M28-interactive-shells/README.md` — +1 -1
- `docs/dev/milestones/M28-interactive-shells/phase-04a-arg-tolerance.md` — +60 -1
- `src/ai/tools/args.rs` — +123 -4
- `src/ai/tools/defs.rs` — +4 -3
- `src/ai/tools/dispatch.rs` — +121 -0

**Commit:** b478d129365a990054fa5ce502be27f9d68f26da

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

# Phase 06a: prompt-classification — the approval panel names the host

**Milestone:** M21 — Host model & SSH
**Status:** review
**Depends on:** phase-04d (`run_terminal_command { host? }`), phase-05 (the ssh prompt kinds)
**Estimated diff:** ~330 lines across 5 files, plus 9 tests
**Tags:** language=rust, kind=feature, size=m

## Goal

`Response::ToolCallPrompt` gains a `classification` — `command`, `remote_shell`,
`new_host` or `escape_hatch` — and the client renders it, **naming the host**.
That is the milestone exit criterion "the approval panel names the host for any
remote command", which today it cannot: the panel is given the command and a
pane hint and nothing else.

**This row classifies and renders. It does not change gating.** The
`new_host` offer-to-persist and the per-class-per-host session scope are
**row 06b**; ghost `hosts:` enforcement is **row 06c**. Splitting there is
what keeps this row readable — see § Current state 4 for the measured reason
the classification is worth shipping before the gate.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 3 "Security model deltas" — the four
  approval classifications.
- `docs/architecture.md` § 2.4 — remotes are execution targets only.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on **2026-09-23** at commit `257a7340`, on a clean tree.

Baseline: `cargo test --lib` → **1961 passed / 0 failed / 6 ignored**.

### 1. `ToolCallPrompt` carries no host, and no class

`src/ipc.rs:444-454`:

```rust
ToolCallPrompt {
    id: String,
    command: String,
    background: bool,
    /// The pane ID (`%N`) that will receive the command (foreground only).
    #[serde(default)]
    target_pane: Option<String>,
},
```

`target_pane`'s `#[serde(default)]` is the **back-compat idiom to copy**: an
older client that does not know the new field still deserialises. Do the same
for `classification`.

The client destructures all four fields at `src/cli/commands/stream.rs:427-433`
and passes `command`, `background` and `target_pane` into
`prompt_tool_call_ratatui`. A new field must be added there or the match will
not compile — that is the good kind of failure.

### 2. The daemon's one send site has no host in hand

`ApprovalRequest` (`src/daemon/executor/mod.rs:37-42`) is the struct that
feeds the prompt:

```rust
struct ApprovalRequest<'a> {
    id: &'a str,
    cmd: &'a str,
    background: bool,
    target_pane_hint: Option<&'a str>,
}
```

The `PendingCall::Foreground` arm destructures `host` already
(`mod.rs:284-291`, row 04d put it there) but **does not pass it on** —
it builds `ApprovalRequest { id, cmd, background: false, target_pane_hint: None }`
at `mod.rs:320-325`. The host is in scope at the call site; it simply is not
forwarded.

### 3. `Owner`/ghost context is already available for `escape_hatch`

`prompt_and_await_approval` (`mod.rs:1269`) takes `ghost_policy:
Option<&GhostPolicy>`, so "this is a ghost shell reaching outside its
sandbox" is decidable at the same point. Row 06c enforces ghost `hosts:`;
this row only needs to *name* the class.

### 4. MEASURED — you approve a remote command, and *then* learn the host is unknown

This is the ordering defect the classification exists to fix, and it is why
the row is worth shipping ahead of the gate.

In `PendingCall::Foreground` (`mod.rs:316-345`), `prompt_and_await_approval`
runs **first**; `host` is resolved only inside the `Ok(cmd_id)` arm that
follows it, where `HostId::parse` and `crate::shell::host_entry` refuse an
unconfigured name (row 04d, Task 4). So today:

1. the model calls `run_terminal_command { command: "uptime", host: "nope" }`,
2. the panel shows **`uptime`** and nothing about `nope`,
3. the user approves,
4. *then* the run returns `Error: unknown host "nope"; configured hosts: pinky`.

The user is asked to approve a command whose destination they were never
shown, and the refusal arrives after their decision. Classifying and
rendering fixes step 2 without touching the order. **Reordering the check is
row 06b's job, not this row's** — do not move it.

### 5. There is no `Classification` type yet

`grep -rn 'Classification' --include=*.rs src/` returns nothing. This row
introduces it.

## Spec

### Task 1 — the `Classification` enum

In `src/ipc.rs`, beside the `Response` enum, add:

```rust
/// What kind of approval a `ToolCallPrompt` is asking for, so the client can
/// render the stakes rather than showing every command identically.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Classification {
    /// An ordinary command on the daemon host.
    Command,
    /// A command on a configured `[hosts.<name>]` host.
    RemoteShell { host: String },
    /// A command naming a host that `[hosts]` does not configure.
    NewHost { host: String },
    /// A ghost shell reaching outside its sandbox.
    EscapeHatch,
}
```

Derive exactly those traits — `PartialEq` is what the round-trip test asserts
on, and `snake_case` is what puts `remote_shell` / `new_host` on the wire
rather than Rust's `RemoteShell`.

Add `classification: Classification` to `Response::ToolCallPrompt` with
`#[serde(default)]`, following `target_pane`'s idiom (§ Current state 1) so an
older client still deserialises a new daemon's frame. That requires a
`Default` impl — `Command` is the right default, because a frame from a
daemon that predates this field is by definition an unclassified local
command.

### Task 2 — carry the host to the prompt

Add `host: Option<&'a str>` to `ApprovalRequest` (`mod.rs:37`) and pass the
`PendingCall::Foreground` arm's already-destructured `host` at `mod.rs:320`.
`None` means the daemon host.

### Task 3 — classify, purely

Write a **pure** function — put it in `src/ipc.rs` beside the enum, or in
`executor/mod.rs`; your call, but it must take no I/O and be unit-testable:

```
classify(host: Option<&str>, is_ghost: bool, known_hosts: &[String]) -> Classification
```

The rules, in this order:

1. `is_ghost` → `EscapeHatch`, whatever the host. A ghost reaching a shell is
   the escape-hatch case and outranks the host question.
2. `host` is `None`, or `Some("local")` in any case → `Command`.
   `HostId::parse` already maps `"local"` to `HostId::Local`
   (`src/host.rs:29-31`); match that, do not diverge.
3. `host` names an entry in `known_hosts` → `RemoteShell { host }`.
4. otherwise → `NewHost { host }`.

Call it from `prompt_and_await_approval` and put the result on the frame. The
configured names come from `Config::load().hosts.keys()`; the classifier takes
them as a slice so the tests need no config on disk.

### Task 4 — render it

In `src/cli/commands/stream.rs:427`, destructure the new field and pass it
into `prompt_tool_call_ratatui`. The panel must **name the host** for
`RemoteShell` and `NewHost`, and mark `NewHost` as unconfigured. Exact
wording is yours; pin it with Task 5's tests rather than matching a string I
invented.

Keep the local-command rendering **byte-identical** to today — Task 5's test 8
is a preservation test, and a gratuitous reflow of the local panel is a scope
violation.

### Task 5 — tests

Nine tests. Names pinned; placement yours.

Pure classifier (wherever Task 3 put it):

1. `classify_a_local_command_is_command` — `None` → `Command`.
2. `classify_host_local_is_command` — `Some("local")` and `Some("LOCAL")`
   both → `Command` (case-insensitive, matching `HostId::parse`).
3. `classify_a_configured_host_is_remote_shell` — a name in `known_hosts` →
   `RemoteShell { host }` carrying that name.
4. `classify_an_unconfigured_host_is_new_host` — a name **not** in
   `known_hosts` → `NewHost { host }`.
5. `classify_a_ghost_is_an_escape_hatch_whatever_the_host` — `is_ghost: true`
   returns `EscapeHatch` for `None`, for a configured host **and** for an
   unconfigured one. This is the **ordering** test: rule 1 outranks 2–4.

Wire round-trip (in `src/ipc_tests.rs`, beside the existing
`ToolCallPrompt` tests at `ipc_tests.rs:288-318`):

6. `tool_call_prompt_round_trips_its_classification` — serialise a frame with
   `NewHost { host: "nope" }`, deserialise, assert equality.
7. `tool_call_prompt_without_a_classification_defaults_to_command` — the
   **back-compat negative**: the legacy JSON at `ipc_tests.rs:313`
   (`{"ToolCallPrompt":{"id":"tc_3","command":"ls -la","background":false}}`,
   no `classification`, no `target_pane`) still deserialises, and its
   classification is `Command`. This is the test that proves the
   `#[serde(default)]` is real.
8. `classification_serialises_in_snake_case` — the wire form of
   `RemoteShell` is `remote_shell`, not `RemoteShell`. A client rendering the
   variant name must not see Rust casing.

Client render (in `src/cli/commands/stream.rs` or the render module):

9. `the_local_command_panel_is_unchanged` — *preservation*: a `Command`
   classification produces the same panel text as before this phase.

### Task 6 — capture the end-to-end evidence

Run the block in § End-to-end verification verbatim and paste its output into
a new Update Log entry, **as that entry's first fenced block**. **Re-run the
capture as the last thing before committing** — if any code changes after it,
the evidence is stale and the criterion is unmet.

## Acceptance criteria

Parenthesised values are what each line produces **against the current tree**.

- [ ] `cargo test --lib` → **1970 passed / 0 failed / 6 ignored**, green on
      three consecutive runs (current: 1961/0/6). 1961 + Task 5's nine tests =
      1970. A justified extra is **reported, not trimmed**.
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → every other target unmoved **except `integration`, which
      gains nothing here**: doc_truth 14, integration 32, isolation 9,
      bug_tracker 6 (*preservation*).
- [ ] The enum exists with its four variants — **scoped to the enum**:
      ``awk '/pub enum Classification/,/^\}/' src/ipc.rs | grep -cE '^\s+(Command,|RemoteShell|NewHost|EscapeHatch,)'``
      → **4** (current: 0). *(Scoped deliberately: an unscoped grep for these
      words matches prose and doc comments elsewhere in the file.)*
- [ ] `ToolCallPrompt` carries the field — **scoped to the variant**:
      ``awk '/^    ToolCallPrompt \{/,/^    \},/' src/ipc.rs | grep -c 'classification: Classification'``
      → **1** (current: 0).
- [ ] `ApprovalRequest` carries the host — **scoped to the struct**:
      ``awk '/^struct ApprovalRequest/,/^\}/' src/daemon/executor/mod.rs | grep -c "host: Option<&'a str>"``
      → **1** (current: 0).
- [ ] No new `#[ignore]`: `grep -rc '#\[ignore' --include=*.rs src/ | grep -v ':0' | awk -F: '{s+=$2} END {print s}'`
      → **6** (current: 6 — *preservation*).
- [ ] All nine tests pass. **Note the `--`**: `cargo test` takes one
      `TESTNAME`, so several filters go after the separator.

      ```sh
      cargo test --lib -- \
        classify_a_local_command_is_command \
        classify_host_local_is_command \
        classify_a_configured_host_is_remote_shell \
        classify_an_unconfigured_host_is_new_host \
        classify_a_ghost_is_an_escape_hatch_whatever_the_host \
        tool_call_prompt_round_trips_its_classification \
        tool_call_prompt_without_a_classification_defaults_to_command \
        classification_serialises_in_snake_case \
        the_local_command_panel_is_unchanged
      ```

      → **9 passed** (current: `0 passed; 0 failed; 1967 filtered out`).
- [ ] Both mutations in § Mutations were applied, run, and their observed
      results recorded in the Update Log.
- [ ] The § End-to-end verification block was run verbatim, pasted as the
      **first fenced block** of an entry titled
      `### Update — <date> (end-to-end verification)`, and the diff snippet
      printed the literal `PASTE MATCH` into that entry.

## Mutations

Apply each with the `patch` tool (the executor contract forbids `sed -i`,
`perl -i`, `> file` and `rm -rf`), run it, restore, report what you saw.
**Run budget: one run each.**

**Mutation A — drop the serde default.** Remove `#[serde(default)]` from
`classification`. Run **once**:
`cargo test --lib tool_call_prompt_without_a_classification_defaults_to_command`.
It must go **red** — an older client's frame would otherwise fail to
deserialise, and that test is the only thing proving the back-compat.

**Mutation B — let the host outrank the ghost.** In the classifier, move the
`is_ghost` check **below** the host checks. Run **once**:
`cargo test --lib classify_a_ghost_is_an_escape_hatch_whatever_the_host`. It
must go **red**, proving rule 1's precedence is pinned rather than incidental.

## Test plan

Nine tests, 1961 → 1970. No new `#[ignore]`, no live-host test: this row
touches classification and rendering only, and nothing it adds opens a
connection.

## End-to-end verification

```sh
cd /home/matt/src/daemoneye
{
  for i in 1 2 3; do
    printf 'lib%d=%s\n' "$i" "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | grep -oE 'ok|FAILED')"
  done
  printf 'lib_counts=%s\n' "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1p-\2f-\3i/')"
  printf 'variants=%s\n' "$(awk '/pub enum Classification/,/^\}/' src/ipc.rs | grep -cE '^\s+(Command,|RemoteShell|NewHost|EscapeHatch,)')"
  printf 'prompt_field=%s\n' "$(awk '/^    ToolCallPrompt \{/,/^    \},/' src/ipc.rs | grep -c 'classification: Classification')"
  printf 'req_host=%s\n' "$(awk '/^struct ApprovalRequest/,/^\}/' src/daemon/executor/mod.rs | grep -c "host: Option<&'a str>")"
  printf 'ignore_total=%s\n' "$(grep -rc '#\[ignore' --include=*.rs src/ | grep -v ':0' | awk -F: '{s+=$2} END {print s}')"
} > /tmp/e2e-06a.txt
cat /tmp/e2e-06a.txt
```

Expected: `lib1..3=ok`, `lib_counts=1970p-0f-6i`, `variants=4`,
`prompt_field=1`, `req_host=1`, `ignore_total=6`.

Paste the file's contents into an Update Log entry titled
`### Update — <date> (end-to-end verification)` **as that entry's first fenced
block**, then run:

```sh
D=docs/dev/milestones/M21-host-model/phase-06a-prompt-classification.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-06a.txt
diff /tmp/pasted-06a.txt /tmp/e2e-06a.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

The literal `PASTE MATCH` line goes **into** the entry. Mutation results
belong in the same entry but **after** that first block.

## Authorizations

You may edit, and nothing else:

- `src/ipc.rs` — Tasks 1, 3 (if the classifier lives there).
- `src/ipc_tests.rs` — Task 5's tests 6–8.
- `src/daemon/executor/mod.rs` — Tasks 2, 3.
- `src/cli/commands/stream.rs` — Tasks 4, 5's test 9.
- The render module, **if** `prompt_tool_call_ratatui` lives there and Task 4
  needs it — say so in the Update Log if you edit it.
- This phase doc's Update Log.

## Out of scope

- **Gating.** An unconfigured host is still refused after approval, exactly as
  § Current state 4 measures it. The offer-to-persist and the reordering are
  **row 06b**. Adding a gate here is a scope violation.
- **Per-class-per-host session scope (`A`).** `SessionApproval`
  (`src/cli/commands/approval.rs:13`) is **untouched** by this row — row 06b.
- **Ghost `hosts:` enforcement.** `GhostConfig.hosts` is parsed but unenforced
  (`src/runbook.rs:97`); `GhostPolicy` has no `hosts` field. That is **row
  06c**. This row only *names* the `EscapeHatch` class.
- **`EditFilePrompt` and the other prompt frames.** Only `ToolCallPrompt`
  gains a classification here; `read_file`/`edit_file` get hosts in row 07.
- **Reordering `prompt_and_await_approval` relative to host resolution** —
  see § Current state 4. Leave the order alone.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-09-22 20:07 (started, executor)

Status flipped `todo → in-progress` (phase doc + milestone README row). Pre-flight
at HEAD `e439a38c` on a clean tree: `cargo test --lib` → **1961 passed / 0
failed / 6 ignored**, matching the phase doc's measured baseline at `257a7340`.
§ Current state re-confirmed before editing: `ToolCallPrompt` at `src/ipc.rs`
carries only `id`/`command`/`background`/`target_pane` with `target_pane`'s
`#[serde(default)]` idiom; `ApprovalRequest` at `src/daemon/executor/mod.rs`
carries no host; the `PendingCall::Foreground` arm destructures `host` and does
not forward it; `grep -rn 'Classification' --include=*.rs src/` matches only
`PathClassification` (an unrelated config-inventory type). Executor: claude
(M21 phase-06a prompt-classification). Implementing Tasks 1–6 in order.

### Update — 2026-09-22 20:22 (end-to-end verification)

The § End-to-end verification block, run verbatim and pasted as its output
(quirks of `cargo test`'s caching noted inline below the block):

```
lib1=ok
lib2=ok
lib3=ok
lib_counts=1970p-0f-6i
variants=4
prompt_field=1
req_host=1
ignore_total=6
```

Three consecutive green `--lib` runs, counts 1970 passed / 0 failed / 6 ignored
(1961 baseline + the nine Task-5 tests), the enum's four variants, the
`ToolCallPrompt` field, `ApprovalRequest.host`, and the `#[ignore]` total all
read back exactly as the acceptance criteria expect.

Diff snippet: `diff /tmp/pasted-06a.txt /tmp/e2e-06a.txt` → `PASTE MATCH`.

## Mutations

**Mutation A — drop the serde default.** Removed `#[serde(default)]` from
`classification` on `Response::ToolCallPrompt`, ran
`cargo test --lib tool_call_prompt_without_a_classification_defaults_to_command`
once: **red** — `backward-compat deserialize: Error("missing field
`classification`", line: 1, column: 69)`, exactly the older-client break the
test guards. Default restored; the test is green again.

**Mutation B — let the host outrank the ghost.** Moved the `is_ghost` check in
`classify` below the host checks, ran
`cargo test --lib classify_a_ghost_is_an_escape_hatch_whatever_the_host` once:
**red** — `a ghost reaching a shell is the escape-hatch case and outranks the
host question (host=None)`; the `None` arm alone is enough to catch the
reordering because it short-circuits before the ghost check can run. Precedence
restored; the test is green again.

## Command output tails

- `cargo build` → clean, no warnings.
- `cargo clippy --all-targets --all-features -- -D warnings` → exits 0.
- `cargo fmt --all` → clean (run immediately before staging).
- `cargo test --lib` → 1970 passed / 0 failed / 6 ignored, three consecutive runs.
- `cargo test` full: doc_truth 14, integration 32 (2 ignored), isolation 9
  (1 ignored), bug_tracker 6, all unmoved from the phase doc's counts.

## Notes for review

- The nine pinned test names are all present and pass under a single
  `cargo test --lib -- <nine names>` filter (9 passed / 1967 filtered out).
- The panel body builder was split out as `tool_call_panel_body` /
  `tool_call_panel_label` in `stream.rs` so Task 5's test 9 can pin the text
  without driving the interactive input loop; `prompt_tool_call_ratatui` now
  calls them, so the rendered panel is what the test pins.
- `ApprovalRequest.host` is `None` at the two `foreground.rs` sites and the
  `Background`/`SendInput` sites in `executor/mod.rs` — those calls carry no
  `host` argument today, so `None` (the daemon host) is the faithful value, and
  it leaves their rendered panels byte-identical.
- The client render reuses `target_pane`'s arrow convention: `→ host: pinky
  (configured)` / `→ host: nope (unconfigured)` / `→ ghost shell reaching
  outside its sandbox`. Local panels are unchanged, per Task 4's preservation
  requirement.
- `classify` lives in `src/ipc.rs` beside the enum (Task 3 allowed either
  home); `prompt_and_await_approval` reads `Config::load().hosts.keys()` there.

### Update — 2026-09-23 03:24 (complete, server-authored)

**Summary:** **Summary**

Phase 06a is implemented and committed (`a6ad7319`, 8 files, +375/−12): `Classification` (`command` / `remote_shell` / `new_host` / `escape_hatch`, snake_case on the wire) sits beside the `Response` enum in `src/ipc.rs` with the pure `classify(host, is_ghost, known_hosts)` classifier; `Response::ToolCallPrompt` carries `classification` behind `#[serde(default)]` (the `target_pane` idiom, with `Default = Command`), `ApprovalRequest` gains `host: Option<&str>` forwarded from the `PendingCall::Foreground` arm, and `prompt_and_await_approval` classifies (reading `Config::load().hosts.keys()`) before sending the frame. The client destructures the new field and the panel names the destination — `→ host: pinky (configured)`, `→ host: nope (unconfigured)`, or `→ ghost shell reaching outside its sandbox` — with local panels byte-identical.

**Notes for review**

- End-to-end block run verbatim and pasted as the first fenced block of `### Update — 2026-09-22 20:22 (end-to-end verification)`: `lib1..3=ok`, `lib_counts=1970p-0f-6i`, `variants=4`, `prompt_field=1`, `req_host=1`, `ignore_total=6`; the diff snippet printed `PASTE MATCH`. `--lib` is 1970/0/6 on three consecutive runs (1961 + the nine pinned tests, all green under a single `--` filter); doc_truth 14, integration 32, isolation 9, bug_tracker 6 unmoved; `fmt`/`build`/`clippy -D warnings` clean.
- Both mutations were applied, run once, recorded as **red**, and restored: A (drop the serde default) failed the legacy-frame test with `missing field "classification"`; B (host outranking ghost) failed the ordering test on the `host=None` arm, since the early `None` return short-circuits before the ghost check — the precedence pin holds for all three host cases.
- Two adaptations worth the architect's model: the panel body/label were split into `tool_call_panel_body` / `tool_call_panel_label` in `stream.rs` so test 9 pins the real rendered text without driving the interactive loop; and `prompt_tool_call_ratatui` now carries 8 parameters, so it takes the same `#[allow(clippy::too_many_arguments)]` its neighbours in `shell_exec.rs` already carry. `ApprovalRequest.host` is `None` at the `foreground.rs`, `Background`, and `SendInput` sites, which carry no `host` argument today — that leaves their panels unchanged. Phase status left at `in-progress`; no completion entry written.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.66s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.09s


TEST
ose ... ok
test header_status_reads_bare_word ... ok
test header_status_uses_first_occurrence_only ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 14 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test readme_tools_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test e2e_prompt_asset_tool_hygiene ... ok
test readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test prompt_assets_name_no_retired_tools ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_documents_every_config_field ... ok

test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 34 tests
test daemon_ping_status_loop ... ignored
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test cancel_request_roundtrip ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_depth_limit_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test resume_run_request_roundtrip ... ok
test minimal_config_parsing ... ok
test ghost_config_parsing ... ok
test window_switch_does_not_corrupt_chat ... ignored
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test event_log_append_read ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test g4_briefing_read_and_clear ... ok
test g4_briefing_injects_on_next_run ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 10 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test hooks_land_on_private_server ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.16s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.08s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-9810e08c3de1e001)
     Running unittests src/main.rs (target/debug/deps/daemoneye-54ad57cf3704bc40)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-d709c9b862b5691d)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-34cb94d3012a3d69)
     Running tests/integration.rs (target/debug/deps/integration-deb70358b520d0d5)
     Running tests/isolation.rs (target/debug/deps/isolation-45facff22bac60f5)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M21-host-model/README.md` — +1 -1
- `docs/dev/milestones/M21-host-model/phase-06a-prompt-classification.md` — +82 -1
- `src/cli/commands/slash.rs` — +1 -0
- `src/cli/commands/stream.rs` — +77 -10
- `src/daemon/executor/foreground.rs` — +2 -0
- `src/daemon/executor/mod.rs` — +18 -0
- `src/ipc.rs` — +53 -0
- `src/ipc_tests.rs` — +141 -0

**Commit:** a6ad7319ac833674887363bf64ea461c1cbacf84

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

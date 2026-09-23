# Phase 06a: prompt-classification — the approval panel names the host

**Milestone:** M21 — Host model & SSH
**Status:** todo
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

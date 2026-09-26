# Phase 07b: edit-file-host — the four write ops over the PTY, and the pane era ends

**Milestone:** M21 — Host model & SSH
**Status:** todo
**Depends on:** phase-07a (`run_in_host_shell`, the shell transport)
**Estimated diff:** ~420 lines across 6 files, plus 11 tests
**Tags:** language=rust, kind=feature, size=m

## Goal

`edit_file` gains `host?` and its **four** write operations — edit, create,
delete, copy — run through a shell on that host instead of scraping a tmux
pane. `target_pane` is retired from `edit_file`, taking the tool-surface
count to **0** and closing the milestone's *"`target_pane` is gone from the
tool surface"* exit criterion.

With the last pane consumer gone, this row also deletes the pane-era
helpers — **except one, which is still live elsewhere; see § Current state 4,
which was measured, not assumed.**

## Architecture references

- `docs/design/daemoneye-2.0.md` § 8 PE decision 3 — *PTY for everything
  remote, including file ops.*
- `docs/architecture.md` § 2.4 — the daemon host stores every managed
  artifact; remotes are execution targets only.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on **2026-09-26** at commit `1e01cf69`, on a clean tree.

Baseline: `cargo test --lib` → **2014 passed / 0 failed / 7 ignored**.
`grep -c 'name: "target_pane"' src/ai/tools/defs.rs` → **1** (`edit_file`
only; 07a took `read_file`'s away).

### 1. The transport already exists — 07a built it for this row

`run_in_host_shell` (`src/daemon/executor/file_ops/mod.rs:135`) is the
PTY analogue of `remote_run_and_capture`, and its doc comment states the
three properties this row depends on:

```rust
pub(super) async fn run_in_host_shell<F, Fut>(
    reg: &ShellRegistry,
    owner: Owner,
    host: crate::host::HostId,
    cmd: &str,
    timeout_secs: u64,
    spawn: F,
) -> anyhow::Result<String>
```

- it resolves a named host through `host_entry_or_bare`, so an approved
  unconfigured host works;
- it takes an **idle** shell via `shell_for_owner` — never `interactive`,
  never `dedicated`;
- it calls the plain `run_command`, so **no credential prompt is ever opened
  from a file op**, and it returns the bytes between the nonce markers with a
  real exit code.

**Use it unchanged.** This row adds no transport.

### 2. The four branches, and what each does

`src/daemon/executor/file_ops/ops.rs`, one `if let Some(pane) = target_pane`
per operation:

| Line | Function | Command builder |
|---|---|---|
| 35 | `run_edit` (`ops.rs:17`) | its own remote-edit command |
| 247 | `run_create` (`ops.rs:224`) | `build_remote_create_cmd` |
| 391 | `run_delete` (`ops.rs:378`) | its own |
| 545 | `run_copy` (`ops.rs:516`) | its own |

All four share one shape — approval prompt, build command, capture, then
scan the output for a sentinel. `run_create`'s capture and scan
(`ops.rs:268-290`) is the worked example:

```rust
let cmd = build_remote_create_cmd(path, content);
let snap = match super::remote_run_and_capture(pane, &cmd, 30).await {
    Ok(s) => s,
    Err(e) => { … return Ok(ToolCallOutcome::Result(format!("Error: {}", e))); }
};
for line in snap.lines().rev() {
    if line.contains("DE_OK:") { … }
    if line.contains("DE_ERROR:") { … }
}
```

**Keep the command builders and the `DE_OK:` / `DE_ERROR:` sentinel scan
exactly as they are.** Only the transport call and the pane-facing strings
change. Mutation B pins that the sentinels do not drift.

### 3. The user-facing strings name the pane, and must name the host

Three places per branch, visible to the model and the user:

- the approval prompt's path — `format!("{} (remote via pane {})", path, pane)`
  (`ops.rs:250`);
- the success sentence — `format!("Created {} via pane {}.", path, pane)`
  (`ops.rs:281`);
- the `log_event` field — `"remote_pane": pane` (`ops.rs:279`).

All three must name the **host** instead. The `log_event` key should become
something like `"remote_host"`; **do not pass a `pid`** — `log_event` stamps
it (`CLAUDE.md` § Important Invariants).

### 4. MEASURED — what may be deleted, and the one that may not

This was grepped, not inferred. **07a's phase doc got this wrong** (it
claimed the write ops needed the read-path helpers); the review corrected it
and this table is the corrected result:

| Helper | Callers today | This row |
|---|---|---|
| `remote_run_and_capture` (`mod.rs:90`) | **4**, all the branches above (`ops.rs:58, 268, 417, 578`) | **delete** once the four branches are converted |
| `local_read_via_buffer` (`read.rs:57`) | **0** — already dead, carries `#[allow(dead_code)]` | **delete**, and remove the allow |
| `build_local_buffer_read_cmd` (`read.rs:35`) | only `local_read_via_buffer` (`read.rs:66`) and one test (`read.rs:632`) | **delete** with it, and its test |
| `get_pane_remote_host` | **`prompt.rs:116` and `foreground.rs:219`** — the tmux foreground path | **KEEP.** Nothing to do with file ops. Deleting it breaks two live callers. |

**Both `#[allow(dead_code)]` in `read.rs` go away by deletion, not by
keeping the functions.** A criterion pins the count at 0.

### 5. Docs that must move

- `CLAUDE.md` — the `edit_file` row still says "or remote via `target_pane`".
- `README.md` — the `edit_file` row in the **Core** table.
- `assets/prompts/sre.toml` — wherever `edit_file` is described.

**42 tools / 28 core / 14 deferred is unchanged** — `host?` is a parameter.
`tests/doc_truth.rs` is the gate; a failure means a count moved.

## Spec

### Task 1 — `host` on `edit_file`, `target_pane` off it

Thread `host: Option<String>` through `edit_file`'s chain exactly as 07a did
for `read_file`: the `ParamDef` in `defs.rs` (replacing `target_pane`), the
args struct, `AiEvent`, `PendingCall`, the `stream.rs` push, the dispatch
arm, and the `WriteArgs`/`ops.rs` parameters.

07a's `read_file` `host` `ParamDef` is the shape to copy
(`defs.rs`, inside the `read_file` def):

```rust
name: "host",
ty: ParamTy::Str,
required: false,
description: "Optional configured host (a [hosts.<name>] key, e.g. \"pinky\") \
              to read the file on. Omitted or \"local\": the daemon host. …",
```

Write the `edit_file` equivalent — same shape, wording about *writing* rather
than reading.

**Batch the type changes and build once at the end.** A partial pass is a
type error at every remaining site.

### Task 2 — convert the four branches

For each of `run_edit`, `run_create`, `run_delete`, `run_copy`:

- resolve the host once per call the way 07a's read path does — `HostId::parse`,
  refusing a malformed name with its own message;
- `HostId::Local` / no host → the **existing local path**, byte for byte;
- `HostId::Named(_)` → the same command builder, run through
  `run_in_host_shell` (§ Current state 1), then the **same** `DE_OK:` /
  `DE_ERROR:` scan;
- update the three user-facing strings per § Current state 3 to name the host.

**Approval still happens, unchanged.** Every write op prompts via
`Response::EditFilePrompt` before touching anything; this row does not move,
weaken or skip that. Test 10 is the guard.

### Task 3 — delete the pane era

Per § Current state 4's table: delete `remote_run_and_capture`,
`local_read_via_buffer`, `build_local_buffer_read_cmd` and its test, and both
`#[allow(dead_code)]` in `read.rs`.

**Do not touch `get_pane_remote_host`** — `prompt.rs:116` and
`foreground.rs:219` still call it, and a criterion pins it at 2 callers.

### Task 4 — docs

Update the three rows in § Current state 5 to document `host?` and drop
`target_pane`. **No tool-count change.**

### Task 5 — tests

Eleven tests. Names pinned; placement yours.

Per-operation, the host path:

1. `create_on_a_named_host_runs_through_the_shell` — the command reaches
   `run_in_host_shell`, not a pane.
2. `delete_on_a_named_host_runs_through_the_shell`
3. `copy_on_a_named_host_runs_through_the_shell`
4. `edit_on_a_named_host_runs_through_the_shell`

Preservation and negatives:

5. `the_local_write_path_is_unchanged` — *preservation*: a local create
   produces byte-identical output to before this phase.
6. `a_malformed_host_is_refused_by_edit_file` — `HostId::parse`'s own
   message; nothing is written and no shell opens.
7. `a_remote_write_names_the_host_not_a_pane` — the success sentence and the
   approval prompt both name the host, and **neither contains the word
   `pane`**. This is the § Current state 3 negative.
8. `the_write_sentinels_are_unchanged` — *preservation*: the `DE_OK:` /
   `DE_ERROR:` contract the scan depends on still holds for all four
   builders.
9. `edit_file_no_longer_accepts_target_pane` — assert against the `ToolDef`,
   not a grep, so the test fails if the param returns under any spelling.
10. `a_remote_write_still_prompts_for_approval` — the **security**
    preservation: a host write is still gated by `EditFilePrompt`. Adding a
    transport must not create an unapproved write path.
11. `get_pane_remote_host_still_has_its_callers` — the deletion negative:
    assert the symbol still exists and is reachable. If you prefer to pin
    this as a grep criterion only, say so in the Update Log and drop the
    test — but then **say which criterion covers it**.

### Task 6 — capture the end-to-end evidence

Run the block in § End-to-end verification verbatim and paste its output into
a new Update Log entry, **as that entry's first fenced block**. **Re-run the
capture as the last thing before committing.**

### Task 7 — the live check

Run the § Live verification block by hand against `pinky` and paste its
output into the same entry, after the mutations. **If `pinky` is unreachable,
report the criterion unmet** — do not describe what it would have printed.

## Acceptance criteria

Parenthesised values are what each line produces **against the current tree**.

- [ ] `cargo test --lib` → **2025 passed / 0 failed / 7 or 8 ignored**, green
      on three consecutive runs (current: 2014/0/7). 2014 + Task 5's eleven =
      2025, **minus one** if you delete `build_local_buffer_read_cmd`'s test
      as Task 3 requires → **2024**. **Run it and pin what you get**, stating
      the arithmetic in the Update Log; a live `#[ignore]` test for Task 7
      takes ignored to 8. *(This criterion is deliberately a range because
      Task 3 deletes a test — the architect cannot know the exact figure
      without running the tasks.)*
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → every other target unmoved: doc_truth 14, integration 32,
      isolation 9, bug_tracker 6 (*preservation*).
- [ ] **`target_pane` is gone from the tool surface** — the milestone exit
      criterion: ``grep -c 'name: "target_pane"' src/ai/tools/defs.rs`` →
      **0** (current: 1).
- [ ] `edit_file` has `host` — **scoped to its `ToolDef`**:
      ``awk '/name: "edit_file"/,/category:/' src/ai/tools/defs.rs | grep -c 'name: "host"'``
      → **1** (current: 0).
- [ ] The four pane branches are gone — pinned on the **branch line**, not
      the symbol name:
      ``grep -c 'if let Some(pane) = target_pane' src/daemon/executor/file_ops/ops.rs``
      → **0** (current: 4). *(Deliberately not `grep -c 'remote_run_and_capture'`:
      simulated at drafting, a single doc comment mentioning the retired
      function by name pushes that count from 0 to 1 and the criterion reads
      as a failed deletion. The branch line cannot appear in prose.)*
- [ ] The dead read-path helpers are gone:
      ``grep -c 'fn local_read_via_buffer\|fn build_local_buffer_read_cmd' src/daemon/executor/file_ops/read.rs``
      → **0** (current: 2).
- [ ] Their `#[allow(dead_code)]` went with them:
      ``grep -c '#\[allow(dead_code)\]' src/daemon/executor/file_ops/read.rs``
      → **0** (current: 2).
- [ ] **`get_pane_remote_host` survives with its callers** (*preservation* —
      § Current state 4):
      ``grep -rc 'get_pane_remote_host' --include=*.rs src/daemon/prompt.rs src/daemon/executor/foreground.rs | grep -c ':1'``
      → **2** (current: 2).
- [ ] All eleven tests pass. **Note the `--`**: `cargo test` takes one
      `TESTNAME`, so several filters go after the separator.

      ```sh
      cargo test --lib -- \
        create_on_a_named_host_runs_through_the_shell \
        delete_on_a_named_host_runs_through_the_shell \
        copy_on_a_named_host_runs_through_the_shell \
        edit_on_a_named_host_runs_through_the_shell \
        the_local_write_path_is_unchanged \
        a_malformed_host_is_refused_by_edit_file \
        a_remote_write_names_the_host_not_a_pane \
        the_write_sentinels_are_unchanged \
        edit_file_no_longer_accepts_target_pane \
        a_remote_write_still_prompts_for_approval \
        get_pane_remote_host_still_has_its_callers
      ```

      → **11 passed** (current: `0 passed; 0 failed; 2021 filtered out`).
- [ ] Both mutations in § Mutations were applied, run, and their observed
      results recorded in the Update Log.
- [ ] The § End-to-end verification block was run verbatim, pasted as the
      **first fenced block** of an entry titled
      `### Update — <date> (end-to-end verification)`, and the diff snippet
      printed the literal `PASTE MATCH` into that entry.
- [ ] The § Live verification block was run and its output pasted into the
      same entry, or reported unmet with the reason.

## Mutations

Apply each with the `patch` tool (the executor contract forbids `sed -i`,
`perl -i`, `> file` and `rm -rf`), run it, restore, report what you saw.
**Run budget: one run each.**

**Mutation A — skip the approval prompt on the host path.** In `run_create`'s
named-host branch, remove the `EditFilePrompt` send and the
`await_edit_file_response` wait. Run **once**:
`cargo test --lib a_remote_write_still_prompts_for_approval`. It must go
**red**. This is the row's security property: adding a transport must not
create an unapproved write path.

**Mutation B — drift a sentinel.** Change `DE_OK:` to `DE_YES:` in one
command builder only, leaving the scan alone. Run **once**:
`cargo test --lib the_write_sentinels_are_unchanged`. It must go **red** —
builder and scanner are a contract, and a silent drift makes a successful
write report as a failure.

## Test plan

Eleven tests added, one deleted with `build_local_buffer_read_cmd`. The net
count is what you measure, not what this doc guesses — see the first
acceptance criterion. No live-host test is required; if you add one as
`#[ignore]` in the style of `read_file_on_pinky_end_to_end`, say so and pin
`ignore_total` at 8.

## End-to-end verification

**No command below may contain a `su`+`do`-shaped token** — the executor's
shell gate refuses such a line outright (measured earlier in this milestone).

```sh
cd /home/matt/src/daemoneye
{
  for i in 1 2 3; do
    printf 'lib%d=%s\n' "$i" "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | grep -oE 'ok|FAILED')"
  done
  printf 'lib_counts=%s\n' "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1p-\2f-\3i/')"
  printf 'target_pane_params=%s\n' "$(grep -c 'name: "target_pane"' src/ai/tools/defs.rs)"
  printf 'edit_file_host=%s\n' "$(awk '/name: "edit_file"/,/category:/' src/ai/tools/defs.rs | grep -c 'name: "host"')"
  printf 'pane_branches_gone=%s\n' "$(grep -c 'if let Some(pane) = target_pane' src/daemon/executor/file_ops/ops.rs)"
  printf 'dead_helpers_gone=%s\n' "$(grep -c 'fn local_read_via_buffer\|fn build_local_buffer_read_cmd' src/daemon/executor/file_ops/read.rs)"
  printf 'dead_code_allows=%s\n' "$(grep -c '#\[allow(dead_code)\]' src/daemon/executor/file_ops/read.rs)"
  printf 'pane_host_helper_kept=%s\n' "$(grep -rc 'get_pane_remote_host' --include=*.rs src/daemon/prompt.rs src/daemon/executor/foreground.rs | grep -c ':1')"
  printf 'doc_truth=%s\n' "$(cargo test --test doc_truth 2>&1 | grep -E '^test result' | grep -oE 'ok|FAILED')"
} > /tmp/e2e-07b.txt
cat /tmp/e2e-07b.txt
```

Expected: `lib1..3=ok`, `lib_counts` as you measured it,
`target_pane_params=0`, `edit_file_host=1`, `pane_branches_gone=0`,
`dead_helpers_gone=0`, `dead_code_allows=0`, `pane_host_helper_kept=2`,
`doc_truth=ok`.

Paste the file's contents into an Update Log entry titled
`### Update — <date> (end-to-end verification)` **as that entry's first fenced
block**, then run:

```sh
D=docs/dev/milestones/M21-host-model/phase-07b-edit-file-host.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-07b.txt
diff /tmp/pasted-07b.txt /tmp/e2e-07b.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

The literal `PASTE MATCH` line goes **into** the entry. Mutation results
belong in the same entry but **after** that first block.

## Live verification

Run by hand against `pinky`; paste the output into the same entry. This is
what proves a write really lands on the remote filesystem.

```sh
cd /home/matt/src/daemoneye
ssh -o ConnectTimeout=8 pinky 'rm -f /tmp/de-07b-probe.txt'
printf 'before=%s\n' "$(ssh -o ConnectTimeout=8 pinky 'test -e /tmp/de-07b-probe.txt && echo present || echo absent')"
```

Then, **through the daemon**, `edit_file { operation: "create", path:
"/tmp/de-07b-probe.txt", content: "…", host: "pinky" }`, and confirm:

```sh
printf 'after=%s\n' "$(ssh -o ConnectTimeout=8 pinky 'cat /tmp/de-07b-probe.txt 2>/dev/null || echo MISSING')"
ssh -o ConnectTimeout=8 pinky 'rm -f /tmp/de-07b-probe.txt'
```

`before` must be `absent` and `after` must be the content you wrote. If you
cannot drive the tool end to end from a test seam, say so and paste what you
could verify.

## Authorizations

You may edit, and nothing else:

- `src/daemon/executor/file_ops/ops.rs` — Tasks 2, 5.
- `src/daemon/executor/file_ops/mod.rs` — Task 3's deletion only
  (`run_in_host_shell` stays as 07a wrote it).
- `src/daemon/executor/file_ops/read.rs` — Task 3's deletions only.
- `src/daemon/executor/file_ops/write.rs` — Task 1's plumbing.
- `src/ai/tools/defs.rs`, `src/ai/tools/args.rs`, `src/ai/types/events.rs`,
  `src/ai/types/pending.rs`, `src/ai/tools/dispatch.rs`,
  `src/daemon/stream.rs`, `src/daemon/executor/mod.rs` — Task 1's threading
  only, `edit_file`'s chain.
- `CLAUDE.md`, `README.md`, `assets/prompts/sre.toml` — Task 4, the
  `edit_file` rows only.
- **Any construction site a field change compile-forces, and any `mod.rs`
  re-export a deletion requires.** Fix them minimally and **list every file
  you touched for this reason in the Update Log.**
- This phase doc's Update Log.

## Out of scope

- **`get_pane_remote_host`** — § Current state 4 measured two live callers
  outside file_ops. A criterion pins them. Do not delete it.
- **`run_in_host_shell`** — 07a's transport, used as-is. If you find it needs
  a change to serve writes, make the minimal one and **say so in the Update
  Log**; do not redesign it.
- **The approval flow.** `EditFilePrompt` fires exactly as it does today.
  Mutation A exists because removing it is the tempting simplification.
- **`edit_file`'s diff rendering, atomic-write semantics or the
  `~/.daemoneye/` block.** Only the transport and the host argument change.
- **Row 08's `ssh_target` → `HostId` mapping.** `GhostPolicy` is untouched
  here.

## Update Log

<!-- entries appended below this line -->

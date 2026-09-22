# Phase 04c: host-keyed reuse — the plumbing `run_terminal_command { host }` needs

**Milestone:** M21 — Host model & SSH
**Status:** in-progress
**Depends on:** phase-04b (`spawn_for_on_host`, `ShellRecord.host`, the seam this phase repairs)
**Estimated diff:** ~330 lines across 6 files, plus 8 tests
**Tags:** language=rust, kind=refactor, size=m

## Goal

Make shell reuse **host-aware** and repair the two hazards 04b left behind, so
that row 04d can add `run_terminal_command { host? }` as a one-field change
rather than a change that silently transposes arguments.

**No tool surface changes here.** No `ToolDef`, no `PendingCall` field, no doc
rows, no tool count movement. Local behaviour is byte-identical: every caller
this phase touches passes `HostId::Local` and gets exactly today's answer.
That is the same shape row 03 used to carry argv end-to-end before 04a spent
it, and it is deliberate — the surface row should be small enough to read.

One of the three defects below is **live today**, not hypothetical: the prompt
tells the model a remote shell is where its next bare command will land, and
it is wrong.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.2 "Host model" — a shell belongs to a
  host; a command names a host or accepts the daemon host.
- `CLAUDE.md` § Key files, `src/shell/session.rs` — `shell_for_owner()`
  "reuse the owner's live shell or create one (spawn injected for tests)".
- `docs/dev/milestones/M21-host-model/phase-04b-ssh-tool-surface.md`
  § "Calibration 2" — the seam flaw this phase is required to fix, in the
  reviewer's own words: *"It becomes a real order-dependent flake the moment a
  test calls `open_shell` without swapping — which row 04c will do. Fix it
  there, as a real guard, before adding `run_terminal_command { host }`
  tests."*

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Everything below was measured on **2026-09-22** at commit `8d3cf7a9`, on a
clean tree. The probe in § 3 was run and reverted; nothing here is inferred.

Baseline: `cargo test --lib` → **1935 passed / 0 failed / 5 ignored**.

### 1. `shell_for_owner` hard-codes `Local`, with a comment naming this row

`src/shell/session.rs:19-64`. The reuse filter, verbatim
(`session.rs:50-53`):

```rust
!record.interactive
    && !record.dedicated
    && record.host == crate::host::HostId::Local
    && record.owner.key() == owner_key
```

and the comment immediately above it (`session.rs:46-49`):

```
// A shell on a named host runs commands *there*, so handing it to a
// caller that asked for no host would silently relocate the command;
// host-keyed reuse needs a caller that can ask (row 04c).
```

The signature takes no host. Callers: **3 in production** —
`src/daemon/executor/shell_exec.rs:213`, `src/daemon/executor/bg_shell.rs:484`,
and the re-export at `src/shell/mod.rs:47` — plus **12 in tests**, all in
`src/shell/session.rs` (lines 295, 343, 347, 374, 383, 425, 476, 540, 561,
583, 607, 672, 709).

### 2. `AiEvent::ToolCall` is a six-field positional tuple

`src/ai/types/events.rs:6-14`:

```rust
/// (id, cmd, wait, thought_signature, shell, timeout_secs)
ToolCall(
    String,         // id
    String,         // cmd
    Option<bool>,   // wait
    Option<String>, // thought_signature
    Option<String>, // shell
    Option<u64>,    // timeout_secs
),
```

Four sites: `src/ai/tools/args.rs:316` (construct),
`src/ai/backends/gemini.rs:329` (construct), `src/daemon/stream.rs:319`
(match), `src/daemon/ghost.rs:829` (match).

**Why this blocks 04d.** Row 04d adds `host: Option<String>` next to
`shell: Option<String>`. Two adjacent same-typed `Option<String>` in a
positional tuple transpose silently — the compiler cannot catch it. The
gemini site already shows the shape the hazard lands in
(`gemini.rs:329-335`):

```rust
let _ = tx.send(AiEvent::ToolCall(
    next_tool_id(),
    cmd,
    Some(!bg),
    None,
    None,
    None,
));
```

Three bare `None`s today; four after 04d, two of them `Option<String>`.
Converting to a **struct variant** first makes 04d's change one named field.

### 3. MEASURED DEFECT — the prompt's `[DEFAULT SHELL]` line is wrong for a remote shell

`default_shell_for` (`src/daemon/executor/knowledge/shell.rs:210-221`) calls
itself a *"pure mirror of `shell_for_owner`'s step-(1) reuse predicate"*
(`shell.rs:201`). It is not one. Its filter:

```rust
!row.interactive
    && !row.dedicated
    && row.exited_unix.is_none()
    && row.owner_key == owner_key
```

**`row.host` is never read**, although `ShellRow` carries it
(`shell.rs:18-26`). So the mirror selects a remote shell that
`shell_for_owner` would skip.

The architect added a throwaway probe to `shell.rs`, ran it, and reverted it:

```
PROBE default_shell_for(remote-only rows) = Some("s1")
test result: ok. 1 passed; 0 failed; 0 ignored; 1940 filtered out
```

with a single row whose `host` is `HostId::Named("pinky")`. The consequence is
at `shell.rs:264-267`, which renders into every prompt:

```
[DEFAULT SHELL] s1 — run_terminal_command with no shell argument lands here
```

That sentence is **false** when `s1` is remote: a bare `run_terminal_command`
skips `s1` and spawns a new local shell. A user who ran `open_shell { host:
"pinky" }` on 04b's surface is being told the opposite of what happens. Fixing
the mirror is Task 4.

### 4. The `open_shell_test_seam` flaw, exactly as 04b's review recorded it

`src/daemon/executor/knowledge/shell.rs:1778-1814`. Two defects, neither
active yet:

```rust
/// … swap in a fresh one and restore the old by dropping the guard.
pub(super) fn swap_in(
    fresh: &crate::shell::ShellRegistry,
) -> Option<crate::shell::ShellRegistry> {
    REPLACE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .replace(fresh.clone())
}
```

- **There is no guard.** `swap_in` returns the *previous value*, so the global
  is never restored. The doc comment's "restore the old by dropping the guard"
  is false — the four call sites (`shell.rs:3230, 3296, 3321, 3352`) write
  `let _swap = open_shell_test_seam::swap_in(&reg);` and drop an `Option`.
- **The swapped registry is leaked** (`shell.rs:1810-1812`):

```rust
match open_shell_test_seam::swapped() {
    Some(swapped) => Box::leak(Box::new(swapped)),
    None => crate::shell::registry(),
}
```

Harmless today only because all four consumers swap and serialize through
`test_home_guard()`. **This phase adds tests that call the same path**, which
is the condition 04b's review named as the point it becomes a real
order-dependent flake.

`ShellRegistry` is `#[derive(Clone, Default)] pub struct
ShellRegistry(Arc<Mutex<…>>)` (`src/shell/registry.rs:104-105`) — cloning is
an `Arc` bump, so the `&'static` return and its `Box::leak` are both
avoidable.

## Spec

### Task 1 — make `open_shell_test_seam` a real RAII guard

In `src/daemon/executor/knowledge/shell.rs`, replace the seam module so that
`swap_in` returns a **guard value whose `Drop` restores the previous
contents** of `REPLACE`, rather than returning the previous value and leaving
the global mutated forever.

Constraints the solution must satisfy — choose the implementation yourself:

- The four existing call sites keep working **unchanged in syntax**: they
  already write `let _swap = open_shell_test_seam::swap_in(&reg);`, which
  binds the guard and drops it at end of scope. Do not edit those four lines.
- After the guard drops, `swapped()` returns whatever it returned before the
  swap — nesting must restore correctly, not merely clear to `None`.
- The doc comment must describe what the code does. It currently promises a
  guard that does not exist; either is acceptable to change, but they must
  agree.

Then remove the leak: `open_shell_test_registry` (`shell.rs:1808-1814`)
returns `&'static ShellRegistry` only so both branches can share a type.
`ShellRegistry` is `Clone` over an `Arc`, so return an **owned**
`ShellRegistry` (clone the singleton in the `None` branch) and drop the
`Box::leak`. Update its caller(s) accordingly.

### Task 2 — convert `AiEvent::ToolCall` to a struct variant

In `src/ai/types/events.rs`, change the tuple variant to named fields, keeping
the same six fields and types, and delete the now-redundant `/// (id, cmd,
…)` positional comment.

```rust
ToolCall {
    id: String,
    cmd: String,
    wait: Option<bool>,
    thought_signature: Option<String>,
    shell: Option<String>,
    timeout_secs: Option<u64>,
},
```

Update the four sites named in § Current state 2 — `args.rs:316`,
`gemini.rs:329`, `stream.rs:319`, `ghost.rs:829`. The two match sites bind by
name; the `ghost.rs` one currently discards three fields positionally
(`_wait`, `_shell`, `_timeout_secs`) and should use `..` or named bindings, as
you prefer.

**Expect the tree not to compile until all four are done.** Batch them and
build once at the end of this task — a partial conversion is a type error at
every remaining site, and stopping to build mid-task is what burned M21
phase-04b's verifier runway (its review filed that as `multi_site_break`).

### Task 3 — `shell_for_owner` takes the host to reuse on

In `src/shell/session.rs`, add a `host: crate::host::HostId` parameter to
`shell_for_owner` and use it in place of the hard-coded literal at
`session.rs:52`, so the filter reads `record.host == host`. Delete the
"(row 04c)" sentence from the comment above it and state the rule the code now
implements.

Every call site passes `crate::host::HostId::Local` — **3 production sites**
(`shell_exec.rs:213`, `bg_shell.rs:484`, and the `mod.rs:47` re-export needs
no change) and the **12 test sites** listed in § Current state 1. Local
behaviour must be byte-identical; Task 5's preservation test is what pins
that.

Where the parameter goes in the list is your call, but put it next to the
other identity arguments (`owner`), not next to the timeout — a reader should
see "whose shell, on which host" together.

### Task 4 — make the mirror a real mirror

In `src/daemon/executor/knowledge/shell.rs`, give `default_shell_for` the
host term it is missing, so the function matches the predicate its own doc
comment (`shell.rs:201`) claims it mirrors.

`ShellRow` already carries `host` (`shell.rs:18-26`), so no new plumbing is
needed — the rows reaching this function are built from `ShellRecord`s that
have it.

**The rule:** the default shell is the lowest-indexed **local** shell. A
session whose only shells are remote has **no** default, and
`format_shell_context` must then omit the `[DEFAULT SHELL]` line entirely
rather than print it with a remote id — the line's own text promises where a
bare `run_terminal_command` lands, and for a remote shell that promise is
false (§ Current state 3).

The `[SHELLS]` listing itself is unchanged: remote shells still appear there,
with their host, exactly as 04b renders them. Only the `[DEFAULT SHELL]` line
is affected.

### Task 5 — tests

Add the eight tests below. Names are pinned; placement and construction are
yours.

In `src/daemon/executor/knowledge/shell.rs`:

1. `swap_in_restores_the_previous_registry_on_drop` — swap a registry in,
   drop the guard, assert `swapped()` is back to its prior value. Nest two
   swaps so the restore is proven to be a *restore*, not a clear.
2. `open_shell_without_a_swap_uses_the_daemon_registry` — the case 04b's
   review said would flake: call the path with no swap in scope and assert it
   resolves the singleton. This is the test that would have been
   order-dependent before Task 1.
3. `default_shell_for_skips_a_remote_shell` — the § Current state 3 probe,
   as a real test: rows containing only a `HostId::Named` shell yield `None`.
4. `default_shell_for_prefers_the_local_shell_over_a_lower_indexed_remote_one`
   — a remote `s1` and a local `s2` yield `s2`. This is the **negative-ordering**
   case: index order alone would answer `s1`, so the test fails if the host
   term is applied anywhere that ordering can overrule it.
5. `shell_context_omits_the_default_line_when_every_shell_is_remote` — the
   rendered block contains `[SHELLS]` and does **not** contain
   `[DEFAULT SHELL]`.

In `src/shell/session.rs`:

6. `shell_for_owner_reuses_a_shell_on_the_requested_host` — an owner with a
   `pinky` shell, asked for `pinky`, reuses it.
7. `shell_for_owner_does_not_reuse_a_shell_from_another_host` — the same
   owner with only a `pinky` shell, asked for `Local`, gets a **new** shell,
   and the `pinky` record is still in the registry afterwards (it must be
   skipped, not reaped).

In `src/ai/tools/args.rs` (or alongside the existing `ToolArgs` tests):

8. `run_terminal_command_args_map_to_named_event_fields` — build the args with
   a distinct value in `shell` and a distinct `timeout_secs`, convert, and
   assert each named field of the struct variant holds the value it was given.
   This is the test that would catch 04d transposing `shell` and `host`.

### Task 6 — capture the end-to-end evidence

Run the block in § End-to-end verification verbatim and paste its output into
a new Update Log entry, **as that entry's first fenced block**.

## Acceptance criteria

Run this whole block before reporting done. Values in parentheses are what the
line produces **against the current tree**; every non-preservation line fails
today.

- [ ] `cargo test --lib` → **1943 passed / 0 failed / 5 ignored**, green on
      three consecutive runs (current: 1935/0/5). 1935 + Task 5's eight tests
      = 1943. If you add a test beyond the eight — as 04b did, justifiably —
      **report the number and the reason rather than trimming a test to hit
      this figure.**
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → every other target unmoved: doc_truth 14, integration 32,
      isolation 9, bug_tracker 6 (*preservation criteria* — this phase touches
      no tool surface, so a doc_truth move means Task 2 or 3 leaked into one).
- [ ] The tool surface did not move (*preservation*):
      `grep -c 'name: "host"' src/ai/tools/defs.rs` → **1** (current: 1 — only
      `open_shell` has it; 04d adds the second).
- [ ] `AiEvent::ToolCall` is a struct variant and no positional construction
      survives: `grep -c 'AiEvent::ToolCall(' src/ai/types/events.rs src/ai/tools/args.rs src/ai/backends/gemini.rs src/daemon/stream.rs src/daemon/ghost.rs | grep -v ':0' | wc -l`
      → **0** (current: 4 — `events.rs` defines the variant without the
      `AiEvent::` prefix, so it is not among them).
- [ ] The hard-coded reuse host is gone:
      `grep -c 'record.host == crate::host::HostId::Local' src/shell/session.rs`
      → **0** (current: 1).
- [ ] The seam no longer returns a leaked `&'static`:
      ``grep -c "fn open_shell_test_registry() -> &'static" src/daemon/executor/knowledge/shell.rs``
      → **0** (current: 2 — the `cfg(test)` form at `shell.rs:1809` and the
      `cfg(not(test))` one at `shell.rs:1855`; both change together).
      **Do not** grep bare `Box::leak` in this file: three of its four
      occurrences (`shell.rs:3160, 4024, 4058`) are unrelated `AtomicBool`
      test helpers and are out of scope.
- [ ] All eight tests in Task 5 pass, by name. **Note the `--`**: `cargo test`
      takes a single `TESTNAME`, so several filters must go after the
      separator or cargo exits with `error: unexpected argument` — verified at
      drafting.

      ```sh
      cargo test --lib -- \
        swap_in_restores_the_previous_registry_on_drop \
        open_shell_without_a_swap_uses_the_daemon_registry \
        default_shell_for_skips_a_remote_shell \
        default_shell_for_prefers_the_local_shell_over_a_lower_indexed_remote_one \
        shell_context_omits_the_default_line_when_every_shell_is_remote \
        shell_for_owner_reuses_a_shell_on_the_requested_host \
        shell_for_owner_does_not_reuse_a_shell_from_another_host \
        run_terminal_command_args_map_to_named_event_fields
      ```

      → **8 passed** (current: `0 passed; 0 failed; 1940 filtered out`).
- [ ] No `#[ignore]` was added to any file this phase touches:
      `grep -c '#\[ignore' src/shell/session.rs src/ai/tools/args.rs | grep -v ':0' | wc -l`
      → **0** (current: 0 — *preservation criterion*).
- [ ] Both mutations in § Mutations were applied, run, and their observed
      results recorded in the Update Log.
- [ ] The § End-to-end verification block was run verbatim, its output pasted
      as the **first fenced block** of an entry titled
      `### Update — <date> (end-to-end verification)`, and the diff snippet
      printed the literal `PASTE MATCH` into that entry.

## Mutations

Apply each with the `patch` tool (the executor contract forbids `sed -i`,
`perl -i`, `> file` and `rm -rf`), run it, restore, and report what you saw. A
mutation that stays green is a finding to write down, not to re-run.

**Run budget:** one run each, scoped as stated. Do not repeat a green run.

**Mutation A — drop the host term from the reuse filter.** In
`shell_for_owner` (`src/shell/session.rs`), delete the `record.host == host`
conjunct so reuse ignores the host again. Run **once**:
`cargo test --lib shell_for_owner_does_not_reuse_a_shell_from_another_host`.
It must go **red** — that test is the only thing proving Task 3 changed
behaviour rather than just threading a parameter through.

**Mutation B — drop the host term from the mirror.** In `default_shell_for`
(`src/daemon/executor/knowledge/shell.rs`), delete the host conjunct Task 4
adds. Run **once**: `cargo test --lib default_shell_for_skips_a_remote_shell
default_shell_for_prefers_the_local_shell_over_a_lower_indexed_remote_one
shell_context_omits_the_default_line_when_every_shell_is_remote`. All three
must go **red**. If any stays green, that test is not pinning the property —
say so in the Update Log.

## Test plan

Eight new tests, named and described in Task 5. The lib count moves 1935 →
1943; every other target is a preservation criterion and must not move.

No live-host test. This phase spawns nothing remote — `HostId::Named("pinky")`
appears only as a **registry value** in test rows, never as a connection.
The live `pinky` work stays where the milestone put it: rows 04d and 09.

## End-to-end verification

The evidence is the gate plus the four structural greps, captured
mechanically.

```sh
cd /home/matt/src/daemoneye
{
  for i in 1 2 3; do
    printf 'lib%d=%s\n' "$i" "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | grep -oE 'ok|FAILED')"
  done
  printf 'lib_counts=%s\n' "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1p-\2f-\3i/')"
  printf 'host_params=%s\n' "$(grep -c 'name: "host"' src/ai/tools/defs.rs)"
  printf 'positional_toolcall=%s\n' "$(grep -c 'AiEvent::ToolCall(' src/ai/types/events.rs src/ai/tools/args.rs src/ai/backends/gemini.rs src/daemon/stream.rs src/daemon/ghost.rs | grep -v ':0' | wc -l)"
  printf 'hardcoded_local=%s\n' "$(grep -c 'record.host == crate::host::HostId::Local' src/shell/session.rs)"
  printf 'static_return=%s\n' "$(grep -c "fn open_shell_test_registry() -> &'static" src/daemon/executor/knowledge/shell.rs)"
} > /tmp/e2e-04c.txt
cat /tmp/e2e-04c.txt
```

Expected: `lib1..3=ok`, `lib_counts=1943p-0f-5i`, `host_params=1`,
`positional_toolcall=0`, `hardcoded_local=0`, `static_return=0`.

Paste the file's contents into an Update Log entry titled
`### Update — <date> (end-to-end verification)` **as that entry's first fenced
block**, then run:

```sh
D=docs/dev/milestones/M21-host-model/phase-04c-host-keyed-reuse.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-04c.txt
diff /tmp/pasted-04c.txt /tmp/e2e-04c.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

The literal `PASTE MATCH` line goes **into** the entry. **If a value cannot be
captured, report it unmet** — do not redefine a term beneath its own line.
Mutation results belong in the same entry but **after** that first block.

## Authorizations

You may edit, and nothing else:

- `src/ai/types/events.rs` — Task 2.
- `src/ai/tools/args.rs`, `src/ai/backends/gemini.rs`, `src/daemon/stream.rs`,
  `src/daemon/ghost.rs` — Task 2's four sites only.
- `src/shell/session.rs` — Tasks 3 and 5.
- `src/daemon/executor/knowledge/shell.rs` — Tasks 1, 4, 5.
- `src/daemon/executor/shell_exec.rs`, `src/daemon/executor/bg_shell.rs` —
  **the `shell_for_owner` call only**, to pass `HostId::Local`.
- This phase doc's Update Log.

## Out of scope

- **`run_terminal_command { host? }` itself.** The tool parameter, the
  `PendingCall` field, the dispatch arm, `ShellTarget`'s host and the three
  doc rows are **row 04d**. A `ToolDef` change here is a scope violation, and
  the `host_params=1` criterion is what catches it.
- **Anything that opens a real SSH connection.** `HostId::Named("pinky")` is a
  value in a test row in this phase, nothing more.
- **`spawn_dedicated_for` and the background path's no-reuse rule**
  (`session.rs:200-205`). Background commands deliberately never reuse; that
  is unchanged and untouched.
- **The `[SHELLS]` listing format.** Only the `[DEFAULT SHELL]` line changes.
- **The other three `open_shell` seam consumers' test bodies**
  (`shell.rs:3296, 3321, 3352`) — Task 1 must keep them compiling unedited.

## Update Log

### Update — 2026-09-22 16:05 (started)

**Status flipped to `in-progress`; README row 04c matches.** Executor run
beginning on a clean tree at `5afbf15e`. Plan: Task 1 (RAII seam guard) and
Task 2 (struct-variant `ToolCall`, four sites batched) first, then Task 3
(host parameter through `shell_for_owner` and its three production callers),
Task 4 (the mirror's host term + the `[DEFAULT SHELL]` omission), Task 5's
eight tests, then the verbatim E2E capture and both mutations.

<!-- entries appended below this line -->

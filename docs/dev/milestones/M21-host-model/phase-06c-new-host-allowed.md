# Phase 06c: new-host-allowed — an unconfigured host is gated, not refused

**Milestone:** M21 — Host model & SSH
**Status:** in-progress
**Depends on:** phase-06a (`Classification`), phase-06b (`NewHost` never auto-approves)

**Estimated diff:** ~340 lines across 4 files, plus 10 tests
**Tags:** language=rust, kind=feature, size=m

## Goal

Today a well-formed but unconfigured host name is **refused after the user has
already approved the command**. This row makes it *allowed but gated*: the
malformed case is refused **before** the prompt, and the unconfigured case
reaches the panel as `new_host`, is always asked (06b guarantees that), and on
approval connects using the bare name as its own ssh destination.

**The persist-to-`[hosts]` offer is deliberately not here.** § Current state 5
measures why: the daemon has no config-write path at all, and the obvious one
would destroy the user's file. That is its own decision, recorded for the PE.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.2 "Host model" — `ssh(1)` destinations;
  `~/.ssh/config` is honoured, so a bare alias is a valid destination.
- `docs/design/daemoneye-2.0.md` § 3 "Security model deltas" — the `new_host`
  approval class exists precisely so an unknown destination is *askable*
  rather than merely refused.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on **2026-09-23** at commit `96b60c7a`, on a clean tree.

Baseline: `cargo test --lib` → **1984 passed / 0 failed / 6 ignored**.

### 1. Both host refusals happen *after* approval — and they are in two places

In `PendingCall::Foreground` (`src/daemon/executor/mod.rs`),
`prompt_and_await_approval` runs first; **inside** its `Ok(cmd_id)` arm, at
`mod.rs:349-365`:

```rust
// `host` is resolved **before** any spawn: a malformed
// name is refused with `HostId::parse`'s own message and
// an unconfigured one with `host_entry`'s …
let host = match host.as_deref().map(crate::host::HostId::parse) {
    None => crate::host::HostId::Local,
    Some(Ok(parsed)) => parsed,
    Some(Err(msg)) => { … return Ok(ToolCallOutcome::Result(msg)); }
};
```

"Before any spawn" is true; **before any *prompt*** is not. And the
*unconfigured* refusal is deeper still — `shell_exec.rs:213`, inside
`run_in_shell_core`:

```rust
if let crate::host::HostId::Named(name) = &host {
    crate::shell::host_entry(name)?;
}
```

So the two cases need opposite treatments and this row separates them:

| Case | Today | After this row |
|---|---|---|
| malformed (`"bad host!"`) | refused **after** approval | refused **before** the prompt — a name that can never be a host is never worth asking about |
| well-formed, unconfigured (`"nope"`) | refused after approval | classified `NewHost`, **asked**, and on approval connected to |

### 2. `host_entry` has five callers and two of them must keep refusing

`grep -rn 'host_entry(' src/` (excluding its own definition and tests):

| Caller | Meaning | This row |
|---|---|---|
| `shell_exec.rs:213` | the run path's pre-spawn check | **changes** — must accept an unconfigured name |
| `executor/mod.rs:443` | the same check on the named-host arm | **changes** |
| `knowledge/shell.rs:1692` | `open_shell { host }`'s pre-spawn check | **unchanged** — `open_shell` is an explicit "give me a shell there" and keeping it strict is this row's deliberate boundary |
| `session.rs:145` | inside `spawn_ssh_host` | **changes** (it is what actually builds the argv) |
| `knowledge/shell.rs:1848` | the live `pinky` test | unchanged |

**Do not change `host_entry`'s own behaviour.** It is the strict lookup and
two callers still want it. Add a *second* function beside it — e.g.
`host_entry_or_bare(name)` — that returns the configured entry when there is
one and otherwise synthesises `HostEntry { ssh: name.to_string(), ..Default::default() }`.
Its doc comment must say which callers may use it and why `open_shell` does
not.

`host_entry`'s "local is implicit" refusal (`session.rs:158-162`) must survive
in the new function too: `"local"` is never a remote destination.

### 3. A bare name is a valid ssh destination — measured

`HostEntry.ssh`'s own doc (`src/config/types.rs:998-1000`) says:

> Any `ssh(1)` destination; `~/.ssh/config` is honoured, so a bare `Host`
> alias is valid.

and the live host proves it: `[hosts.pinky]` is literally `ssh = "pinky"`,
which resolves through `~/.ssh/config`/DNS. So synthesising
`HostEntry { ssh: "<name>" }` is not a hack — it is the same shape the
configured entry already has.

### 4. `ssh_argv` needs no change

`src/host.rs:84` builds `-tt`, the three ControlMaster `-o` flags and
`entry.ssh`. A synthesised entry flows through it unchanged.

### 5. MEASURED — there is no config-write path, and the obvious one would eat the user's file

`grep -rn 'fn save\|write.*config\.toml' src/config/` finds **one** write:
`seeds.rs:50`, which writes the whole bundled template on first run. Nothing
mutates an existing config.

The dependency is `toml = "1.1"` (`Cargo.toml:41`) — **not `toml_edit`**. A
deserialize→serialize round-trip through `toml` discards comments, and the
shipped config is **193 lines of which 96 are comments**:

```
$ wc -l ~/.daemoneye/etc/config.toml   → 193
$ grep -c '^#' ~/.daemoneye/etc/config.toml → 96
```

Persisting `[hosts.<name>]` therefore needs either a new `toml_edit`
dependency or an **append-only text writer** (a `[hosts.<name>]` table at EOF
is valid TOML and preserves everything above it). Both are defensible; neither
is this row's to choose. **The persist offer is deferred** — see § Out of
scope, and `NEXT.md` carries it for the PE.

## Spec

### Task 1 — refuse a malformed host before the prompt

In `src/daemon/executor/mod.rs`'s `PendingCall::Foreground` arm, parse the
host with `HostId::parse` **before** calling `prompt_and_await_approval`, and
return its error as the tool result without prompting. A name that cannot be
a host is not a decision the user should be asked to make.

Keep the parsed `HostId` and pass it on, so the later code does not parse
twice. Note that `prompt_and_await_approval` already takes `host` (06a) for
the classification — feed it from the same parse.

**The unconfigured case must NOT be refused here.** It goes to the panel.

### Task 2 — the permissive lookup

In `src/shell/session.rs`, add the second function described in § Current
state 2 beside `host_entry`. Requirements:

- a configured host returns its configured entry, unchanged;
- a configured host with an **empty** `ssh` is still an error — an explicit
  half-written entry is a mistake, not an invitation to guess;
- `"local"` in any case is still an error;
- anything else well-formed returns a synthesised entry whose `ssh` is the
  name itself.

Route `spawn_ssh_host` (`session.rs:145`) through it, and the two pre-spawn
checks at `shell_exec.rs:213` and `executor/mod.rs:443` through it as well.
**Leave `knowledge/shell.rs:1692` (`open_shell`) on the strict `host_entry`** —
and say in the Update Log that you did, so the reviewer can see the boundary
was deliberate.

### Task 3 — tests

Ten tests. Names pinned; placement yours.

Lookup behaviour (beside the new function):

1. `a_configured_host_returns_its_configured_entry` — `[hosts.pinky] ssh =
   "pinky-real"` returns `pinky-real`, **not** the bare name. Pins that
   configuration still wins.
2. `an_unconfigured_host_becomes_its_own_ssh_destination` — `"nope"` yields
   an entry whose `ssh` is `"nope"`.
3. `a_configured_host_with_an_empty_ssh_is_still_an_error` — the
   **negative**: a half-written entry is not silently replaced by a guess.
4. `the_local_host_is_still_refused_by_the_permissive_lookup` — `"local"`,
   `"LOCAL"` and `"Local"` all still error.
5. `the_strict_lookup_still_refuses_an_unconfigured_host` —
   *preservation*: `host_entry` itself is unchanged, so `open_shell` keeps
   refusing.

Ordering / gating (in `executor/mod.rs` or wherever Task 1 lands):

6. `a_malformed_host_is_refused_without_prompting` — the ordering property.
   Assert the refusal carries `HostId::parse`'s own wording **and** that no
   approval prompt was sent. If your test seam cannot observe "no prompt
   sent", say so in the Update Log and assert what you can — do not delete
   the test.
7. `a_malformed_host_refusal_names_the_character_class` — the message is
   `HostId::parse`'s (`[A-Za-z0-9._-]`), not a shell-limit or timeout
   sentence.
8. `an_unconfigured_host_is_not_refused_before_the_prompt` — the **inverse**
   of test 6, and the row's whole point: `"nope"` must reach the prompt path
   rather than erroring early.

Classification (wherever 06a's classifier lives):

9. `an_unconfigured_host_still_classifies_as_new_host` — *preservation*: the
   permissive lookup must not make `"nope"` look configured. This is the test
   that stops Task 2 from quietly turning every new host into a
   `RemoteShell` and thereby making it auto-approvable.
10. `a_new_host_still_never_auto_approves` — *preservation*, 06b's security
    property re-pinned here because Task 2 is exactly the change that could
    undermine it.

### Task 4 — capture the end-to-end evidence

Run the block in § End-to-end verification verbatim and paste its output into
a new Update Log entry, **as that entry's first fenced block**. **Re-run the
capture as the last thing before committing** — if code changes after it, the
evidence is stale and the criterion is unmet.

## Acceptance criteria

Parenthesised values are what each line produces **against the current tree**.

- [ ] `cargo test --lib` → **1994 passed / 0 failed / 6 ignored**, green on
      three consecutive runs (current: 1984/0/6). 1984 + Task 3's ten = 1994.
      A justified extra is **reported, not trimmed** — name it and say why.
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → every other target unmoved: doc_truth 14, integration 32,
      isolation 9, bug_tracker 6 (*preservation*).
- [ ] The permissive lookup exists and `open_shell` does **not** use it:
      ``grep -c 'host_entry_or_bare' src/daemon/executor/knowledge/shell.rs``
      → **0** (current: 0 — *preservation of the boundary*; if you named the
      function something else, re-pin this grep to your name and say so).
- [ ] The permissive lookup exists:
      ``grep -c 'fn host_entry_or_bare' src/shell/session.rs`` → **1**
      (current: 0). *(If you name the function something else, re-pin this and
      the next grep to your name and say so in the Update Log.)*
- [ ] The run path calls **the permissive one** — scoped to the function, and
      matching the **call** rather than the word, because the range also
      contains the word in a comment:
      ``awk '/async fn run_in_shell_core/,/^\}/' src/daemon/executor/shell_exec.rs | grep -c 'host_entry_or_bare(name)'``
      → **1** (current: 0).
- [ ] The strict lookup survives untouched — **scoped to the function**:
      ``awk '/pub fn host_entry\(/,/^\}/' src/shell/session.rs | grep -c 'Some(entry) if'``
      → **1** (current: 1 — *preservation*).
- [ ] No new `#[ignore]`: `grep -rc '#\[ignore' --include=*.rs src/ | grep -v ':0' | awk -F: '{s+=$2} END {print s}'`
      → **6** (current: 6 — *preservation*).
- [ ] All ten tests pass. **Note the `--`**: `cargo test` takes one
      `TESTNAME`, so several filters go after the separator.

      ```sh
      cargo test --lib -- \
        a_configured_host_returns_its_configured_entry \
        an_unconfigured_host_becomes_its_own_ssh_destination \
        a_configured_host_with_an_empty_ssh_is_still_an_error \
        the_local_host_is_still_refused_by_the_permissive_lookup \
        the_strict_lookup_still_refuses_an_unconfigured_host \
        a_malformed_host_is_refused_without_prompting \
        a_malformed_host_refusal_names_the_character_class \
        an_unconfigured_host_is_not_refused_before_the_prompt \
        an_unconfigured_host_still_classifies_as_new_host \
        a_new_host_still_never_auto_approves
      ```

      → **10 passed** (current: `0 passed; 0 failed; 1990 filtered out`).
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

**Mutation A — let the permissive lookup swallow an empty `ssh`.** Make a
configured-but-empty entry synthesise the bare name instead of erroring. Run
**once**: `cargo test --lib a_configured_host_with_an_empty_ssh_is_still_an_error`.
It must go **red** — a half-written entry must not be silently guessed at.

**Mutation B — make the permissive lookup change the classification.** In
06a's classifier, treat any name the permissive lookup accepts as
`RemoteShell`. Run **once**:
`cargo test --lib an_unconfigured_host_still_classifies_as_new_host a_new_host_still_never_auto_approves`.
**Both** must go **red**. This is the row's security property: making a host
*reachable* must not make it *trusted*, and these two tests are what stop
Task 2 from silently re-enabling auto-approval for unknown destinations.

## Test plan

Ten tests, 1984 → 1994. No new `#[ignore]` and no live-host test: every new
test uses a synthesised or temp-config host, and nothing here opens a
connection. The live path is exercised by the existing ignored `pinky` tests,
which this row does not touch.

## End-to-end verification

**Note on the commands below:** none of them may contain a `su`+`do`-shaped
token — the executor's shell gate refuses such a line outright (measured on
the previous row, where a criteria grep embedding that word could not be typed
and had to be run from a script). The greps here are written to avoid it.

```sh
cd /home/matt/src/daemoneye
{
  for i in 1 2 3; do
    printf 'lib%d=%s\n' "$i" "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | grep -oE 'ok|FAILED')"
  done
  printf 'lib_counts=%s\n' "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1p-\2f-\3i/')"
  printf 'open_shell_stays_strict=%s\n' "$(grep -c 'host_entry_or_bare' src/daemon/executor/knowledge/shell.rs)"
  printf 'permissive_exists=%s\n' "$(grep -c 'fn host_entry_or_bare' src/shell/session.rs)"
  printf 'run_path_permissive=%s\n' "$(awk '/async fn run_in_shell_core/,/^\}/' src/daemon/executor/shell_exec.rs | grep -c 'host_entry_or_bare(name)')"
  printf 'strict_lookup_intact=%s\n' "$(awk '/pub fn host_entry\(/,/^\}/' src/shell/session.rs | grep -c 'Some(entry) if')"
  printf 'ignore_total=%s\n' "$(grep -rc '#\[ignore' --include=*.rs src/ | grep -v ':0' | awk -F: '{s+=$2} END {print s}')"
} > /tmp/e2e-06c.txt
cat /tmp/e2e-06c.txt
```

Expected: `lib1..3=ok`, `lib_counts=1994p-0f-6i`, `open_shell_stays_strict=0`,
`permissive_exists=1`, `run_path_permissive=1`, `strict_lookup_intact=1`,
`ignore_total=6`.

Paste the file's contents into an Update Log entry titled
`### Update — <date> (end-to-end verification)` **as that entry's first fenced
block**, then run:

```sh
D=docs/dev/milestones/M21-host-model/phase-06c-new-host-allowed.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-06c.txt
diff /tmp/pasted-06c.txt /tmp/e2e-06c.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

The literal `PASTE MATCH` line goes **into** the entry. Mutation results
belong in the same entry but **after** that first block.

## Authorizations

You may edit, and nothing else:

- `src/shell/session.rs` — Task 2 (the new function; `host_entry` itself stays
  as it is).
- `src/daemon/executor/mod.rs` — Tasks 1, 2.
- `src/daemon/executor/shell_exec.rs` — Task 2's call site only.
- Wherever 06a's classifier lives — Task 3's tests 9–10 only.
- **Any construction site a signature change compile-forces.** Fix them
  minimally and **list every file you touched for this reason in the Update
  Log.**
- This phase doc's Update Log.

## Out of scope

- **Persisting the host to `[hosts]`.** § Current state 5 measured that the
  daemon has no config-write path and that `toml` (not `toml_edit`) would
  discard 96 comment lines from a 193-line file. Choosing between a new
  dependency and an append-only writer is a call for the PE, not this row.
  **Do not add a config writer.** If you think the row is incomplete without
  it, say so in the Update Log.
- **`open_shell { host }`.** It keeps the strict lookup — § Current state 2,
  and a criterion pins it. `open_shell` is an explicit request for a shell on
  a named host; requiring configuration there is deliberate.
- **Ghost `hosts:` enforcement** — row 06d. `GhostPolicy` still gains no
  `hosts` field here.
- **The `new_host` panel wording.** 06a rendered it and 06b labelled it
  "always asked"; both are fine as they are.
- **`read_file` / `edit_file { host? }`** — row 07.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-09-23 10:10 (end-to-end verification)

The § End-to-end verification block, run verbatim. Captured mechanically to
`/tmp/e2e-06c.txt`; the file's contents — **the output, which is what the
paste-diff compares** — are the first fenced block below:

```
lib1=ok
lib2=ok
lib3=ok
lib_counts=1994p-0f-6i
open_shell_stays_strict=0
permissive_exists=1
run_path_permissive=1
strict_lookup_intact=1
ignore_total=6
```

The command that produced it (for the record; not the compared content):

```sh
cd /home/matt/src/daemoneye
{
  for i in 1 2 3; do
    printf 'lib%d=%s\n' "$i" "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | grep -oE 'ok|FAILED')"
  done
  printf 'lib_counts=%s\n' "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1p-\2f-\3i/')"
  printf 'open_shell_stays_strict=%s\n' "$(grep -c 'host_entry_or_bare' src/daemon/executor/knowledge/shell.rs)"
  printf 'permissive_exists=%s\n' "$(grep -c 'fn host_entry_or_bare' src/shell/session.rs)"
  printf 'run_path_permissive=%s\n' "$(awk '/async fn run_in_shell_core/,/^\}/' src/daemon/executor/shell_exec.rs | grep -c 'host_entry_or_bare(name)')"
  printf 'strict_lookup_intact=%s\n' "$(awk '/pub fn host_entry\(/,/^\}/' src/shell/session.rs | grep -c 'Some(entry) if')"
  printf 'ignore_total=%s\n' "$(grep -rc '#\[ignore' --include=*.rs src/ | grep -v ':0' | awk -F: '{s+=$2} END {print s}')"
} > /tmp/e2e-06c.txt
cat /tmp/e2e-06c.txt
```

Expected: `lib1..3=ok`, `lib_counts=1994p-0f-6i`, `open_shell_stays_strict=0`,
`permissive_exists=1`, `run_path_permissive=1`, `strict_lookup_intact=1`,
`ignore_total=6` — **all match**.

Paste-diff (compares the first fenced block against `/tmp/e2e-06c.txt`):

```sh
D=docs/dev/milestones/M21-host-model/phase-06c-new-host-allowed.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-06c.txt
diff /tmp/pasted-06c.txt /tmp/e2e-06c.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

PASTE MATCH

## Mutations (run once each, restored, results recorded)

**Mutation A — let the permissive lookup swallow an empty `ssh`.** In
`host_entry_or_bare` the `Some(_)` arm (an entry whose `ssh` is empty) was
changed from `Err(...)` to the same synthesised `Ok(...)` the unconfigured
arm returns. `cargo test --lib
a_configured_host_with_an_empty_ssh_is_still_an_error` → **FAILED** (red, as
required): "a half-written entry is an error, not a guess — the refusal names
the entry and its empty destination" did not hold, because the lookup now
returns `HostEntry { ssh: "half" }` instead of erroring. Restored to `Err`;
the test is green again (`1 passed; 0 failed`).

**Mutation B — make the permissive lookup change the classification.** In
`ipc::classify` the `known_hosts` membership test gained
`|| crate::shell::host_entry_or_bare(named).is_ok()`.
`cargo test --lib -- an_unconfigured_host_still_classifies_as_new_host
a_new_host_still_never_auto_approves` → **both FAILED** (red, as required):

```
test ipc::tests::an_unconfigured_host_still_classifies_as_new_host ... FAILED
test ipc::tests::a_new_host_still_never_auto_approves ... FAILED
test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 1998 filtered out
left: RemoteShell { host: "nope" }, right: NewHost { host: "nope" }
```

Restored to the plain `known_hosts` membership test; both tests green again
(`2 passed; 0 failed`).

## Notes the reviewer should see

- **`open_shell` stays on the strict lookup.** `knowledge/shell.rs:1692` was
  not touched — `grep -c host_entry_or_bare` on that file is `0` above, which
  is the pinned *preservation* criterion, and the boundary is deliberate.
- **Test 6's "no prompt sent" seam.** `resolve_foreground_host` is the
  pre-prompt gate as a pure function; the test
  `a_malformed_host_is_refused_without_prompting` asserts the refusal and its
  wording but does not observe the transport. What makes the ordering real is
  the call site: the parse runs before `prompt_and_await_approval` in
  `PendingCall::Foreground`, and `run_in_shell_core`'s own
  pre-spawn check can only see an already-parsed id — a malformed name cannot
  reach either the prompt or the spawn. The same-name parse-twice hazard is
  gone: the prompt's classification is fed `Some(host.as_str())` from the
  parsed id.
- **Renamed one overwritten test.** The pre-row test
  `run_terminal_command_refuses_an_unknown_host_before_any_spawn` pinned the
  *opposite* of this row (unconfigured → refused), so it was rewritten as
  `run_terminal_command_an_unconfigured_host_resolves_to_its_own_destination`
  (host resolves, record carries the name, entry.ssh is the bare name).
  The malformed-name test that lived beside it is superseded by
  `a_malformed_host_is_refused_without_prompting` in `mod.rs`, which pins the
  same wording. Net tests: 1984 → 1994 (+10), as the criterion pins.
- **`An unconfigured host` reaching `run_in_shell_core` with an entry.** The
  lookup runs inside `run_in_shell_core` *after* the approval the caller has
  already obtained, so the spawn that connects to an unconfigured destination
  is still gated — 06b's never-auto-approve is what keeps the gate on.
- **Files touched for compile-forced reasons:** none beyond the authorised
  set — `host_entry`'s signature did not change, so no construction site was
  forced to move.

### Update — 2026-09-23 08:16 (started)

Executor: **this run (headless)**. Status flipped `todo → in-progress`; README
row updated to match. No code touched yet. Baseline before any change: the
phase doc's stated `cargo test --lib` = 1984 passed / 0 failed / 6 ignored.

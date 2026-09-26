# Phase 07a: read-file-host — `read_file { host? }` over the PTY, not the pane

**Milestone:** M21 — Host model & SSH
**Status:** todo
**Depends on:** phase-06c (`host_entry_or_bare`), phase-04d (`run_terminal_command { host? }`)
**Estimated diff:** ~340 lines across 5 files, plus 10 tests
**Tags:** language=rust, kind=feature, size=m

## Goal

`read_file` gains `host?` and reads a remote file through **a shell on that
host**, with marker-delimited output, instead of scraping a tmux pane.
`target_pane` is retired from `read_file`.

**`edit_file` is row 07b.** Splitting there is measured, not stylistic: the
read path is **one** `target_pane` branch, the write path is **four**
(`ops.rs:35, 247, 391, 545`). This row also builds the shell transport 07b
then reuses, so the hard part lands once with one consumer to prove it.

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

Measured on **2026-09-26** at commit `0af4da90`, on a clean tree.

Baseline: `cargo test --lib` → **2004 passed / 0 failed / 6 ignored**.
`grep -c 'name: "target_pane"' src/ai/tools/defs.rs` → **2** (`read_file` and
`edit_file`). This row takes it to **1**; 07b takes it to 0 and closes the
milestone's *"`target_pane` is gone from the tool surface"* exit criterion.

### 1. The marker protocol is already here — only the transport is wrong

`build_remote_read_cmd` (`src/daemon/executor/file_ops/read.rs:19-28`)
already emits a marker-delimited command:

```rust
format!(
    "echo '__DE_S__'; sed -n '{},{}p' '{}' 2>&1{}; echo '__DE_E__'; echo '__DE_DONE__'",
    start, end, safe_path, grep_part
)
```

and `read.rs:176` already extracts between those markers:

```rust
let extracted = extract_marked(&snap, "__DE_S__", "__DE_E__").unwrap_or_else(|| snap.clone());
```

**Keep both.** What must change is what runs the command.

### 2. What must change: `remote_run_and_capture` is pane scraping

`src/daemon/executor/file_ops/mod.rs:90-108`:

```rust
async fn remote_run_and_capture(pane_id: &str, cmd: &str, timeout_secs: u64) -> anyhow::Result<String> {
    tmux::off_runtime("send-keys", move || tmux::send_keys(&p, &c)) …
    loop {
        tokio::time::sleep(Duration::from_millis(300)).await;
        …
        let snap = tmux::off_runtime("capture-pane", move || tmux::capture_pane(&p, 600)) …
```

`send-keys` then poll `capture-pane` every 300 ms until a sentinel appears:
it races the shell, it is bounded by the visible pane, and it needs a tmux
pane to exist at all. The PTY substrate already solves this — a command run
through it returns its **real** captured output and exit code.

### 3. The transport to use, and the worked example

`run_terminal_command { host }` already does exactly this end to end
(phase-04d). The call it makes is
`shell_exec::run_in_shell_with_prompts(reg, owner, cmd, max_per_owner, shell,
timeout_secs, prompt_id, tx, rx, host, spawn)`, and for a named host the
spawn closure is `|id| spawn_ssh_host(id, name, 24, 80)` — see
`src/daemon/executor/mod.rs`'s `PendingCall::Foreground` arm, whose two-arm
`match &host` split exists because the local and remote spawn closures have
**different types** and one generic call cannot serve both.

**A file read must not prompt**, so do not reuse the `_with_prompts` entry
point. The plain one is `run_in_shell_core`'s public sibling in the same
module; pick whichever of `shell_exec`'s entry points takes no
`SudoPromptArgs`, and if none is public enough, add a thin `pub(super)`
wrapper beside the existing ones rather than widening the prompting path.
Say in the Update Log which you used.

**Reuse an idle shell, never an interactive one.** `shell_for_owner`
(`src/shell/session.rs:19`) already skips `interactive` and `dedicated`
records and is host-keyed (04c), so passing the `HostId` is sufficient —
do not hand-roll shell selection.

### 4. `target_pane`'s remaining consumers on the read path

`read.rs:94` (the parameter), `read.rs:154-200` (the branch), plus the
`ReadFile` plumbing. Retiring it from **`read_file` only** means:

- the `ParamDef` at `defs.rs` for `read_file` goes away, replaced by `host`;
- `PendingCall::ReadFile` / `AiEvent::ReadFile` swap the field;
- `edit_file`'s `target_pane` **stays** until 07b — a criterion pins the
  count at 1, not 0.

`get_pane_remote_host` and `local_read_via_buffer` (the `%N`-era local path,
`read.rs:165`) become dead **for `read_file`** but are still used by the
write ops until 07b. **Do not delete them in this row**; a criterion pins
that `local_read_via_buffer` still exists.

### 5. Docs that must move

- `CLAUDE.md:149` — the `read_file` row still says "daemon-host file read".
- `README.md` — the `read_file` row in the **Core** table.
- `assets/prompts/sre.toml` — wherever `read_file` is described.

`tests/doc_truth.rs` gates **counts and names**, and **42 tools / 28 core /
14 deferred is unchanged** — `host?` is a parameter. A doc_truth failure here
means a count moved.

## Spec

### Task 1 — the shell transport

Add a helper beside `remote_run_and_capture` in
`src/daemon/executor/file_ops/mod.rs` that runs a marker-delimited command
**in a shell on a `HostId`** and returns the captured output — the § Current
state 3 shape. Signature is yours; it needs the command, the `HostId`, the
owner, and a timeout.

It must:

- route through `shell_for_owner` so an **idle** shell is reused or created
  on that host (§ Current state 3);
- use `host_entry_or_bare` (06c) for a named host, so an approved
  unconfigured host still works;
- **not** prompt — no `CredentialPrompt` round-trip on a file read;
- return the shell's real captured output for `extract_marked` to cut.

**Leave `remote_run_and_capture` in place.** 07b retires it with the last
write op; deleting it here breaks the four write branches.

### Task 2 — `host` on `read_file`, `target_pane` off it

Thread `host: Option<String>` through `read_file`'s chain the way 04d did for
`run_terminal_command`: the `ParamDef` in `defs.rs` (replacing
`target_pane`), the args struct, `AiEvent::ReadFile`, `PendingCall::ReadFile`,
the `stream.rs` push, the dispatch arm, and `read.rs`'s parameter.

**Batch the type changes and build once at the end** — a partial pass is a
type error at every remaining site, and stopping to build mid-task is what
cost an earlier row of this milestone its verifier runway.

Resolve the host exactly as the foreground path does: `HostId::parse`, then
for a `Named` host `host_entry_or_bare`. A malformed name is refused with
`HostId::parse`'s own message. `None` and `"local"` both mean the daemon
host and take the existing local path unchanged.

### Task 3 — rewrite the read branch

Replace `read.rs:154-200`'s pane branch with a host branch:

- `HostId::Local` (or no host) → the **existing local read path**, byte for
  byte. Test 9 is the preservation check.
- `HostId::Named(_)` → `build_remote_read_cmd` (unchanged) run through
  Task 1's transport, then `extract_marked(&snap, "__DE_S__", "__DE_E__")`
  (unchanged).

The result label must say which host the content came from, so a model
reading two files from two machines can tell them apart. Exact wording is
yours; test 8 pins that the host name appears.

`get_pane_remote_host` and `local_read_via_buffer` stay in the file for the
write ops (§ Current state 4).

### Task 4 — docs

Update the three rows in § Current state 5 to describe `host?` and drop
`target_pane` **from `read_file` only**. Do not touch `edit_file`'s rows.
**No tool-count change.**

### Task 5 — tests

Ten tests. Names pinned; placement yours.

Pure / unit:

1. `build_remote_read_cmd_is_unchanged` — *preservation*: the command still
   carries `__DE_S__`, the `sed -n 'START,ENDp'` range and the quoted path.
   This row must not alter the wire format the markers depend on.
2. `extract_marked_cuts_between_the_sentinels` — *preservation* of the
   extraction the new transport feeds.
3. `a_malformed_host_is_refused_by_read_file` — `HostId::parse`'s own
   message, no shell opened.
4. `read_file_without_a_host_takes_the_local_path` — `None` → local.
5. `read_file_host_local_takes_the_local_path` — `"local"` in any case → the
   same, matching `HostId::parse`.

Transport / integration-ish:

6. `the_read_transport_reuses_an_idle_shell_on_the_host` — two reads on the
   same host land in **one** shell, not two.
7. `the_read_transport_never_opens_an_interactive_shell` — the **negative**:
   with an `interactive` record present for that owner and host, the read
   gets a different shell. Pins the § Current state 3 rule.
8. `a_remote_read_labels_the_host` — the result names the host it came from.
9. `the_local_read_path_is_unchanged` — *preservation*: a local read
   produces byte-identical output to before this phase.
10. `read_file_no_longer_accepts_target_pane` — the retirement: the arg is
    gone from `read_file`'s schema. Assert against the `ToolDef`, not a
    grep, so the test fails if the param comes back under any spelling.

### Task 6 — capture the end-to-end evidence

Run the block in § End-to-end verification verbatim and paste its output into
a new Update Log entry, **as that entry's first fenced block**. **Re-run the
capture as the last thing before committing.**

### Task 7 — the live check

Run the § Live verification block by hand against `pinky` and paste its
output into the same entry, after the mutations. **If `pinky` is
unreachable, report the criterion unmet** — do not describe what it would
have printed.

## Acceptance criteria

Parenthesised values are what each line produces **against the current tree**.

- [ ] `cargo test --lib` → **2014 passed / 0 failed / 6 ignored**, green on
      three consecutive runs (current: 2004/0/6). 2004 + Task 5's ten = 2014.
      A justified extra is **reported, not trimmed** — name it and say why.
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → every other target unmoved: doc_truth 14, integration 32,
      isolation 9, bug_tracker 6 (*preservation*). **doc_truth is the
      tool-count gate** — a failure means `host?` was treated as a tool.
- [ ] `target_pane` is off `read_file` but still on `edit_file`:
      ``grep -c 'name: "target_pane"' src/ai/tools/defs.rs`` → **1**
      (current: 2). 07b takes it to 0.
- [ ] `read_file` has `host` — **scoped to its `ToolDef`**:
      ``awk '/name: "read_file"/,/category:/' src/ai/tools/defs.rs | grep -c 'name: "host"'``
      → **1** (current: 0).
- [ ] The write path's helper survives for 07b (*preservation*):
      ``grep -c 'fn local_read_via_buffer' src/daemon/executor/file_ops/read.rs``
      → **1** (current: 1).
- [ ] `remote_run_and_capture` survives for 07b (*preservation*):
      ``grep -c 'async fn remote_run_and_capture' src/daemon/executor/file_ops/mod.rs``
      → **1** (current: 1).
- [ ] No new `#[ignore]` beyond the live test, if you add one as ignored:
      `grep -rc '#\[ignore' --include=*.rs src/ | grep -v ':0' | awk -F: '{s+=$2} END {print s}'`
      → **6 or 7** (current: 6). **State which** in the Update Log; the live
      check in Task 7 may be run by hand from the block instead of added as a
      test, in which case it stays 6.
- [ ] All ten tests pass. **Note the `--`**: `cargo test` takes one
      `TESTNAME`, so several filters go after the separator.

      ```sh
      cargo test --lib -- \
        build_remote_read_cmd_is_unchanged \
        extract_marked_cuts_between_the_sentinels \
        a_malformed_host_is_refused_by_read_file \
        read_file_without_a_host_takes_the_local_path \
        read_file_host_local_takes_the_local_path \
        the_read_transport_reuses_an_idle_shell_on_the_host \
        the_read_transport_never_opens_an_interactive_shell \
        a_remote_read_labels_the_host \
        the_local_read_path_is_unchanged \
        read_file_no_longer_accepts_target_pane
      ```

      → **10 passed** (current: `0 passed; 0 failed; 2010 filtered out`).
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

**Mutation A — let the transport take an interactive shell.** Remove the
idle/interactive filter from the transport's shell selection (or pass a
`dedicated`/`interactive` shell deliberately). Run **once**:
`cargo test --lib the_read_transport_never_opens_an_interactive_shell`. It
must go **red** — an interactive shell swallows the command and the read
would hang until timeout, which is the failure this rule prevents.

**Mutation B — break the marker contract.** Change `__DE_S__` to `__DE_X__`
in `build_remote_read_cmd` only, leaving `extract_marked`'s sentinels alone.
Run **once**: `cargo test --lib build_remote_read_cmd_is_unchanged`. It must
go **red** — the two halves are a contract, and a silent drift between them
is how a remote read starts returning the whole screen instead of the file.

## Test plan

Ten tests, 2004 → 2014. The live `pinky` check is run by hand from the
§ Live verification block (Task 7); if you prefer it as an `#[ignore]` test
in the style of `run_terminal_command_on_pinky_end_to_end`, that is fine —
say so and re-pin `ignore_total` to 7.

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
  printf 'read_file_host=%s\n' "$(awk '/name: "read_file"/,/category:/' src/ai/tools/defs.rs | grep -c 'name: "host"')"
  printf 'buffer_helper_kept=%s\n' "$(grep -c 'fn local_read_via_buffer' src/daemon/executor/file_ops/read.rs)"
  printf 'pane_capture_kept=%s\n' "$(grep -c 'async fn remote_run_and_capture' src/daemon/executor/file_ops/mod.rs)"
  printf 'ignore_total=%s\n' "$(grep -rc '#\[ignore' --include=*.rs src/ | grep -v ':0' | awk -F: '{s+=$2} END {print s}')"
  printf 'doc_truth=%s\n' "$(cargo test --test doc_truth 2>&1 | grep -E '^test result' | grep -oE 'ok|FAILED')"
} > /tmp/e2e-07a.txt
cat /tmp/e2e-07a.txt
```

Expected: `lib1..3=ok`, `lib_counts=2014p-0f-6i`, `target_pane_params=1`,
`read_file_host=1`, `buffer_helper_kept=1`, `pane_capture_kept=1`,
`ignore_total=6` (or 7 — see the criterion), `doc_truth=ok`.

Paste the file's contents into an Update Log entry titled
`### Update — <date> (end-to-end verification)` **as that entry's first fenced
block**, then run:

```sh
D=docs/dev/milestones/M21-host-model/phase-07a-read-file-host.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-07a.txt
diff /tmp/pasted-07a.txt /tmp/e2e-07a.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

The literal `PASTE MATCH` line goes **into** the entry. Mutation results
belong in the same entry but **after** that first block.

## Live verification

Run by hand against `pinky`; paste the output into the same entry. This is
what proves the transport reads a *real* remote file rather than a fixture.

```sh
cd /home/matt/src/daemoneye
T=$(mktemp -d)
ssh -o ConnectTimeout=8 pinky 'printf "alpha\nbravo\ncharlie\n" > /tmp/de-07a-probe.txt'
printf 'remote_line2=%s\n' "$(ssh -o ConnectTimeout=8 pinky "sed -n '2,2p' /tmp/de-07a-probe.txt")"
ssh -o ConnectTimeout=8 pinky 'rm -f /tmp/de-07a-probe.txt'
find "$T" -depth -delete
```

`remote_line2` must be `bravo`. Then, **through the daemon**, read the same
range of a file that exists on `pinky` and paste the tool result — the point
is that `read_file { path, host: "pinky" }` returns the file's content, not a
pane snapshot. If you cannot drive the tool end to end from a test seam, say
so and paste what you could verify.

## Authorizations

You may edit, and nothing else:

- `src/daemon/executor/file_ops/mod.rs` — Task 1 (add; do not delete
  `remote_run_and_capture`).
- `src/daemon/executor/file_ops/read.rs` — Tasks 2, 3, 5.
- `src/ai/tools/defs.rs`, `src/ai/tools/args.rs`, `src/ai/types/events.rs`,
  `src/ai/types/pending.rs`, `src/ai/tools/dispatch.rs`,
  `src/daemon/stream.rs`, `src/daemon/executor/mod.rs` — Task 2's threading
  only, `read_file`'s chain.
- `CLAUDE.md`, `README.md`, `assets/prompts/sre.toml` — Task 4, the
  `read_file` rows only.
- **Any construction site a field change compile-forces**, including a
  `mod.rs` re-export if Task 1's helper needs one. Fix them minimally and
  **list every file you touched for this reason in the Update Log.**
- This phase doc's Update Log.

## Out of scope

- **`edit_file` and the four write ops** — row 07b. `ops.rs` is not on the
  authorized list; `target_pane` stays at **1** and two criteria pin the
  helpers 07b still needs.
- **Deleting `remote_run_and_capture`, `local_read_via_buffer` or
  `get_pane_remote_host`** — all four write branches still use them.
  Criteria pin the first two.
- **Prompting on a file read.** No `CredentialPrompt` round-trip; if a
  remote read needs a credential, that is a finding to write down, not a
  feature to add here.
- **`read_file`'s masking, pagination or grep semantics.** Only the
  transport and the host argument change; `mask_sensitive` and the
  offset/limit clamps stay exactly as they are.
- **The `[SHELLS]` block or `list_shells`.** A shell the read transport
  opens is an ordinary shell and needs no special labelling.

## Update Log

<!-- entries appended below this line -->

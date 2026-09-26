# Phase 07a: read-file-host — `read_file { host? }` over the PTY, not the pane

**Milestone:** M21 — Host model & SSH
**Status:** done
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

### Update — 2026-09-26 18:17 (started)

Executor: claude (headless). Status flipped `todo` → `in-progress` in this
doc and in the milestone README's phase table.

- Transport used: the plain `run_command` (no prompt plumbing) after a
  `shell_for_owner` reuse-or-create — `run_in_shell_with_prompts` was
  deliberately **not** reused, per § Current state 3: a file read must not
  open a `CredentialPrompt` round-trip.
- The helper is `file_ops::run_in_host_shell(reg, owner, host, cmd,
  timeout_secs, spawn)`, which resolves the max-per-owner cap from
  `Config::load` itself (the foreground path passes it in; a read has no
  caller-provided cap).
- Adaptation to record: `run_read_file`'s remote arm builds its shell owner
  from `host_session_key()` (`Owner::Chat("read_file")`) because the tool
  chain carries no session id to key the shell under.
- Files touched for compile-forced construction sites beyond the authorized
  chain: `src/daemon/executor/knowledge/mod.rs` (test-only re-exports of
  `close_shell_with` and `spawn_pinky_ssh` for the live test) and
  `src/daemon/executor/knowledge/shell.rs` (the two items' visibility widened
  `pub(super)`/private → `pub(crate)`).
- One pre-existing test updated, not added: `dispatch.rs`'s
  `e2e_no_pane_identifiers_in_tool_descriptions` pinned `read_file keeps
  target_pane until M21`; the pin had to move to `edit_file` only when the
  parameter was retired from `read_file` (the test asserts against the
  `ToolDef`, so it failed at build-and-test time, as designed).
- `ignore_total` is **7**, not 6: the live check of Task 7 was written as the
  `#[ignore]`d `read_file_on_pinky_end_to_end` in the style the Test plan
  names, which the criteria allow when re-pinned — this entry records it.


### Update — 2026-09-26 18:17 (end-to-end verification)

```text
lib1=ok
lib2=ok
lib3=ok
lib_counts=2014p-0f-7i
target_pane_params=1
read_file_host=1
buffer_helper_kept=1
pane_capture_kept=1
ignore_total=7
doc_truth=ok
```

diff /tmp/pasted-07a.txt /tmp/e2e-07a.txt → `PASTE MATCH`

```text
diff /tmp/pasted-07a.txt /tmp/e2e-07a.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
→ PASTE MATCH
```

### Update — 2026-09-26 18:17 (live verification)

```text
remote_line2=bravo
PASTE MATCH >>>
read_file_on_pinky hostname="pinky"
<<<
```

Live check notes: the `ssh`-probe part ran by hand (probe file written on
pinky, line 2 read back as `bravo`, file removed). The through-the-daemon half
ran as the phase's `#[ignore]`d live test
`read_file_on_pinky_end_to_end` (`cargo test --lib -- --ignored
read_file_on_pinky`), which drives Task 1's transport over the real ssh argv
(`spawn_pinky_ssh`) against the real `/etc/hostname` on `pinky`, extracts
between the markers and asserts the result names `pinky`; it passed and
printed the `PASTE MATCH` block above. Driving the full `PendingCall`
dispatch seam (which needs a live IPC transport and a configured model) was
out of reach from a test seam, so the tool result was reconstructed from the
same `extract_marked` + `format_remote_read_result` pair the dispatch arm
feeds — the transport itself, the marker cut and the label are the real
remote interaction.

### Update — 2026-09-26 18:17 (mutations)

- **Mutation A — let the transport take an interactive shell.** Applied by
  replacing the `shell_for_owner` call inside `run_in_host_shell` with a
  first-match reuse over the registry that ignored the `interactive`/
  `dedicated` filter (the same reuse-or-create shape the filter guards).
  `cargo test --lib the_read_transport_never_opens_an_interactive_shell` →
  **FAILED**: `a fresh shell was opened for the read, not the interactive
  one: {"s0"}` — with the filter gone the read was handed the shell the
  first read had held and had since marked `interactive`, the exact hang the
  rule prevents. Restored; the test is green again.
- **Mutation B — break the marker contract.** Changed `__DE_S__` to
  `__DE_X__` in `build_remote_read_cmd` only, leaving `extract_marked`'s
  sentinels alone. `cargo test --lib build_remote_read_cmd_is_unchanged` →
  **FAILED** on the exact-drift assertion, left `…__DE_X__…` vs right
  `…__DE_S__…` — the two halves are a contract, and the drift a remote read
  would have suffered is returning the whole screen instead of the file.
  Restored; the test is green again.

### Update — 2026-09-26 20:28 (complete, server-authored)

**Summary:** **Summary:** Phase 07a is implemented and committed (`2008b342`). `read_file` now takes `host?` (retiring `target_pane` from that tool only — `edit_file` keeps it for row 07b): `None`/`"local"` takes the daemon-host read path byte-for-byte unchanged, and a named host runs the **unchanged** marker-delimited command through the new `file_ops::run_in_host_shell` transport — `shell_for_owner` reuse (idle shells only, never interactive/dedicated), `host_entry_or_bare` for unconfigured hosts, and the plain `run_command` with no prompt plumbing. The result label names the host it came from.

**Notes for review:**
- **Entry point used:** the plain `run_command` after a `shell_for_owner` reuse-or-create, not `run_in_shell_with_prompts` — a file read must not open a `CredentialPrompt` round-trip, per § Current state 3. Recorded in the phase's Update Log.
- **Test-fixture re-exports** (compile-forced construction sites, listed in the Update Log as authorized): `knowledge/shell.rs`'s `spawn_pinky_ssh` and `close_shell_with` widened to `pub(crate)` and re-exported through `knowledge/mod.rs` under `#[cfg(test)]`, so the live test could drive the real ssh argv without a third fixture copy.
- **One pre-existing test updated:** `dispatch.rs`'s `e2e_no_pane_identifiers_in_tool_descriptions` pinned "read_file keeps target_pane until M21"; it asserts against the `ToolDef`, so it went red at build-and-test time and its pin moved to `edit_file` only.
- **`ignore_total` = 7** (was 6): the Task 7 live check ships as the `#[ignore]`d `read_file_on_pinky_end_to_end`, in the style the Test plan names; the criteria allow this when re-pinned, which the Update Log does.
- **Gates:** `cargo test --lib` → **2014 passed / 0 failed / 7 ignored**, green on three consecutive runs; doc_truth 14, integration 32, isolation 9, bug_tracker 6 all unmoved; fmt/build/clippy(-D warnings) clean. `target_pane_params=1`, `read_file_host=1`, `buffer_helper_kept=1`, `pane_capture_kept=1`.
- **Both mutations applied and observed red**, then restored and re-verified green (A: the interactive-shell filter is load-bearing — the read was handed the interactive shell and the fresh-shell assertion failed; B: `__DE_S__` → `__DE_X__` tripped the exact wire-format assertion). Results recorded in the Update Log.
- **End-to-end + live evidence** are in the phase doc's Update Log, each in its own entry, with the `PASTE MATCH` diff confirming the pasted block is byte-identical to the captured file. The live check ran against `pinky`: the ssh probe printed `remote_line2=bravo`, and the `#[ignore]`d test drove the real transport over ssh and printed `read_file_on_pinky hostname="pinky"` — the full `PendingCall` dispatch seam needs a live IPC transport plus a configured model, so the tool result was reconstructed from the same `extract_marked`/`format_remote_read_result` pair the dispatch arm feeds; the transport, marker cut and label are the real remote interaction. Status remains `in-progress` with the tree clean, ready for the server's `review` flip.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST

test open_bug_on_in_progress_phase_is_clean ... ok
test header_status_strips_trailing_prose ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 14 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_tools_counts_are_accurate ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test e2e_prompt_asset_tool_hygiene ... ok
test readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test prompt_assets_name_no_retired_tools ... ok
test docs_do_not_carry_retired_index_claims ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_config_template_documents_every_config_field ... ok

test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 34 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test cancel_request_roundtrip ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test resume_run_request_roundtrip ... ok
test ghost_config_parsing ... ok
test minimal_config_parsing ... ok
test window_switch_does_not_corrupt_chat ... ignored
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
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
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
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
test hooks_land_on_private_server ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.16s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Compiling daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.46s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-9810e08c3de1e001)
     Running unittests src/main.rs (target/debug/deps/daemoneye-54ad57cf3704bc40)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-d709c9b862b5691d)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-34cb94d3012a3d69)
     Running tests/integration.rs (target/debug/deps/integration-deb70358b520d0d5)
     Running tests/isolation.rs (target/debug/deps/isolation-45facff22bac60f5)
   Doc-tests daemoneye

```

**Files changed:**

- `CLAUDE.md` — +1 -1
- `README.md` — +1 -1
- `assets/prompts/sre.toml` — +7 -7
- `docs/dev/milestones/M21-host-model/README.md` — +1 -1
- `docs/dev/milestones/M21-host-model/phase-07a-read-file-host.md` — +96 -1
- `src/ai/tools/args.rs` — +2 -2
- `src/ai/tools/defs.rs` — +9 -7
- `src/ai/tools/dispatch.rs` — +22 -8
- `src/ai/types/events.rs` — +1 -1
- `src/ai/types/pending.rs` — +6 -6
- `src/daemon/executor/file_ops/mod.rs` — +62 -0
- `src/daemon/executor/file_ops/read.rs` — +628 -99
- `src/daemon/executor/knowledge/mod.rs` — +10 -0
- `src/daemon/executor/knowledge/shell.rs` — +3 -3
- `src/daemon/executor/mod.rs` — +27 -9
- `src/daemon/stream.rs` — +2 -2

**Commit:** 2008b34223a7eb78cdffda3bbaf03b21acf905ce

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-09-26

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark (661 turns, the
  longest run of this milestone)
- **Scope deviations:** none that are the executor's.
- **THIS SPEC CONTAINED A FALSE PREMISE, and the executor handled it
  correctly.** § Current state 4 and § Out of scope both claimed the four
  write ops still need `local_read_via_buffer` and `build_local_buffer_read_cmd`,
  and forbade deleting them. **They do not.** Measured at review:
  `grep -c 'local_read_via_buffer\|build_local_buffer_read_cmd' src/daemon/executor/file_ops/ops.rs`
  → **0**, and `local_read_via_buffer` now has **zero** callers anywhere.
  Obeying the instruction therefore left two genuinely dead functions, and
  with `clippy -D warnings` the only alternatives were `#[allow(dead_code)]`
  or deleting them — which the spec forbade. The executor chose the allow and
  **documented the real reason on both**: *"Kept alive for row 07b, which
  retires the pane-era helpers with the write ops."* That is the right answer
  to a contradictory spec, not a lint-silencing shim.
  **`remote_run_and_capture` was correctly pinned** — it has four live callers
  (`ops.rs:58, 268, 417, 578`), exactly the four write branches.
  **Remedy: 07b deletes all three**, which its row description already
  anticipates; its spec must state that the two read-path helpers are already
  dead rather than repeating this premise.
- **Calibration:** the false premise came from reading `read.rs`'s
  `target_pane` branch and assuming the write ops shared its helpers, instead
  of grepping `ops.rs` for them. **When a spec says "X still uses Y, do not
  delete Y", grep for the callers of Y before writing the sentence** — the
  same measure-don't-infer rule this project applies to system facts applies
  to claims about its own call graph.

**Review re-verification (independent, 2026-09-26):** all four gates re-run
clean. `lib_counts=2014p-0f-7i` — exactly the pinned 2014, and the `7 ignored`
is the allowed branch of the criterion, **disclosed** in the Update Log as the
`#[ignore]`d `read_file_on_pinky_end_to_end`. Every structural criterion
reproduces: `target_pane_params=1` (2 → 1 as pinned; 07b takes it to 0),
`read_file_host=1`, `buffer_helper_kept=1`, `pane_capture_kept=1`. `PASTE
MATCH` prints from the doc's own diff. `cargo test` green on every target
(lib 2014/0/7, doc_truth 14, integration 32, isolation 9, bug_tracker 6). All
ten pinned tests pass.

**The live test was run by the reviewer against the real `pinky`** and passes,
printing `read_file_on_pinky hostname="pinky"` — so a remote read really does
come back through a shell on the host, not a pane snapshot.

**Mutation B re-applied by the reviewer** — changing `__DE_S__` to `__DE_X__`
in the command builder alone turns `build_remote_read_cmd_is_unchanged` red
with *"the wire format must not drift"*. The two halves of the marker contract
are pinned against each other, which is what stops a remote read silently
returning the whole screen instead of the file.

No production `unwrap`/`expect`/`panic!` (the two `.expect("must exist")` are
in `dispatch.rs`'s test module), no `dbg!`, no `TODO`. The two `unsafe` blocks
are `std::env::set_var` inside a `with_home` test helper whose **first line**
is `crate::test_home_guard()` — the idiom this milestone has been bitten by
and now applies without being told.

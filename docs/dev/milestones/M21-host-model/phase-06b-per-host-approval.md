# Phase 06b: per-host-approval — session approval stops leaking across hosts

**Milestone:** M21 — Host model & SSH
**Status:** in-progress
**Depends on:** phase-06a (`Classification` on the prompt)
**Estimated diff:** ~300 lines across 3 files, plus 10 tests
**Tags:** language=rust, kind=feature, size=m

## Goal

Session approval (`A`) is currently two booleans — `regular` and `sudo` — with
no host dimension. 06a taught the panel to *name* the host; this row makes the
**decision** host-aware, so approving commands on the daemon host does not
silently approve them on `pinky`, and a `new_host` is never auto-approved at
all.

**This row changes who gets asked. It does not change what is reachable.** An
unconfigured host is still refused after approval, exactly as today — making
it *allowed* but gated is **row 06c**, and persisting it to `[hosts]` goes
with it. Ghost `hosts:` enforcement is **row 06d**.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 3 "Security model deltas" — session scope
  is per class **per host**.
- `docs/security.md` § 2 — the approval gate is the boundary; widening it
  silently is the failure this row removes.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on **2026-09-23** at commit `468ff3bd`, on a clean tree.

Baseline: `cargo test --lib` → **1970 passed / 0 failed / 6 ignored**.

### 1. MEASURED — the classification reaches the panel but not the decision

06a threaded `Classification` into `prompt_tool_call_ratatui`. Every use of it
inside that function, by line offset within the function
(`src/cli/commands/stream.rs:1101`):

```
 9:    classification: &crate::ipc::Classification,     <- the parameter
14:    let body = tool_call_panel_body(command, background, target_pane, classification);
18:    let auto_approved = if is_sudo {
19:        approval.sudo
21:        approval.regular
24:    if auto_approved {
37:                approval.sudo = true;
39:                approval.regular = true;
```

**The classification is read at line 14 and nowhere else.** The
`auto_approved` decision at 18–21 consults only `approval.sudo` /
`approval.regular` — there is no host term. And `commands = true` is the
**shipped default** (`assets/etc/config.toml:333`: *"only `commands` starts as
true because non-sudo commands are bounded by OS file permissions"* — a
sentence that was true before remote execution existed and is not any more).

So on a default install, today: the panel prints
`→ host: pinky (configured)` and then `✓ auto-approved (session)`. The user is
shown the destination and given no opportunity to refuse it. That is the gap
this row closes.

### 2. `SessionApproval` is two booleans plus three name-sets

`src/cli/commands/approval.rs:13-28`:

```rust
pub(super) struct SessionApproval {
    /// Non-sudo commands auto-approve without prompting.
    pub(super) regular: bool,
    pub(super) sudo: bool,
    pub(super) scripts_all: bool,
    pub(super) scripts: HashSet<String>,
    pub(super) runbooks_all: bool,
    pub(super) runbooks: HashSet<String>,
    pub(super) file_edits_all: bool,
    pub(super) file_edits: HashSet<String>,
}
```

**The `scripts` / `runbooks` / `file_edits` `HashSet<String>` fields are the
idiom to copy** — a class-wide `_all` bool plus a per-name set. Host scope is
the same shape one level over: a per-host set beside the class-wide bool.

`from_config` (`approval.rs:49`) carries a comment that is load-bearing for
this row:

> Called once at session start (chat/ask entry). Never re-derive this
> mid-session: runtime state (`/approvals`, prompt answers) must win — a
> turn-end re-derive was the 2026-08-11 approval-persistence bug.

Whatever shape you choose must keep that property.

### 3. Every reader of the two flags — the complete list

Changing their meaning touches exactly these, and nothing else
(`grep -rn 'approval\.regular\|approval\.sudo' src/cli/`):

| Site | What it does |
|---|---|
| `slash.rs:281-282` | `/approvals` status lines, `commands : on` / `sudo : on` |
| `slash.rs:301,305,311` | `/approvals grant|revoke` mutating `regular` |
| `stream.rs:1119,1121` | the `auto_approved` read (§ 1) |
| `stream.rs:1137,1139` | the `A` write on approve-for-session |
| `approval.rs:33-34,52-53,64-67` | `Default`, `from_config`, `hint()` |

### 4. `Classification` is already the right key

`src/ipc.rs` (06a) gives you `Command`, `RemoteShell { host }`,
`NewHost { host }`, `EscapeHatch`. The host is **inside** the variant, so a
`(class, host)` key needs no new plumbing from the daemon — the frame already
carries it.

## Spec

### Task 1 — the scope rule, written down first

Implement exactly this. Task 4's tests pin each line.

| Classification | Command kind | Auto-approves when… |
|---|---|---|
| `Command` (daemon host) | non-sudo | `regular` — **unchanged from today** |
| `Command` (daemon host) | sudo | `sudo` — **unchanged from today** |
| `RemoteShell { host }` | non-sudo | the session has approved non-sudo commands **for that host** |
| `RemoteShell { host }` | sudo | the session has approved sudo **for that host** |
| `NewHost { .. }` | either | **never.** An unconfigured host is always asked, whatever `A` the session holds. |
| `EscapeHatch` | either | **never.** A ghost reaching outside its sandbox is always asked. |

Two properties this table encodes, both pinned by tests:

- **`A` on the daemon host must not approve a remote host**, and `A` on
  `pinky` must not approve `web01`. Approving is per host, not global.
- **`NewHost` and `EscapeHatch` are unconditional asks.** No session state,
  and no config default, can auto-approve them — this is the row's security
  property and Mutation A is what proves it.

**Local behaviour must be byte-identical.** A session that never names a host
sees exactly today's flow; test 9 is the preservation check.

### Task 2 — extend `SessionApproval`

In `src/cli/commands/approval.rs`, add per-host state beside the existing
flags, following the `scripts` / `runbooks` idiom (§ Current state 2): a
class-wide bool for the daemon host (keep `regular` / `sudo` exactly as they
are) plus per-host sets for the remote case. Shape is yours; suggestions that
fit the existing style are `hosts_regular: HashSet<String>` and
`hosts_sudo: HashSet<String>`, keyed by the host name from the classification.

Add a method that answers Task 1's table in one place — something like:

```
fn auto_approves(&self, class: &Classification, is_sudo: bool) -> bool
```

**Put the table's logic in that method, not in the prompt function.** The
prompt function is the interactive flow and cannot be unit-tested without a
renderer; this method is pure and is where tests 1–8 aim. That separation is
the same one 06a made for `tool_call_panel_body`, and for the same reason.

`from_config` must keep its "never re-derive mid-session" property
(§ Current state 2): the new sets start empty and are only ever added to by
an `A` answer.

### Task 3 — use it, and record `A` against the right key

In `src/cli/commands/stream.rs`:

- Replace the `auto_approved` computation (§ Current state 1, lines 18–21 of
  the function) with a call to Task 2's method, passing the classification.
- On an `A` answer, record it **against the classification's host** rather
  than the global flag: `Command` still sets `regular` / `sudo`;
  `RemoteShell { host }` inserts into the per-host set. `NewHost` and
  `EscapeHatch` must **not** record anything — offering "approve for session"
  on a prompt that can never auto-approve would be a lie. Adjust the prompt's
  session label accordingly; the exact wording is yours.
- The `✓ approved — all … auto-approved for this session` line should name
  the host when there is one, so the user knows what they just widened.

In `src/cli/commands/slash.rs`, `/approvals` (lines 281–282) should show the
per-host state it now holds. Keep `commands : on` / `sudo : on` for the
daemon host so the existing lines do not move; add the hosts below them.

### Task 4 — tests

Ten tests. Names pinned; placement yours (tests 1–8 belong beside the pure
method).

1. `local_command_auto_approves_when_regular_is_set` — *preservation*:
   `Command` + non-sudo + `regular` → true.
2. `local_sudo_needs_the_sudo_flag` — *preservation*: `Command` + sudo →
   follows `sudo`, not `regular`.
3. `a_remote_host_is_not_covered_by_the_local_session_grant` — the headline
   **negative**: `regular = true`, `hosts_regular` empty, `RemoteShell { host:
   "pinky" }` → **false**. This is § Current state 1's gap, inverted.
4. `approving_a_host_covers_that_host` — after recording `pinky`,
   `RemoteShell { "pinky" }` → true.
5. `approving_one_host_does_not_cover_another` — the second **negative**:
   `pinky` approved, `RemoteShell { "web01" }` → **false**.
6. `a_remote_sudo_command_is_not_covered_by_a_remote_non_sudo_grant` — the
   class dimension survives the host dimension: `pinky` in `hosts_regular`,
   sudo command on `pinky` → **false**.
7. `a_new_host_never_auto_approves` — with **every** flag set and the host
   present in both sets, `NewHost { "nope" }` → **false**.
8. `an_escape_hatch_never_auto_approves` — same, for `EscapeHatch`.
9. `the_local_approval_flow_is_unchanged` — *preservation*: a `Command`
   classification produces the same auto-approve outcome and the same panel
   text as before this phase.
10. `approving_for_session_on_a_remote_host_records_only_that_host` — after
    an `A` on `pinky`, `hosts_regular` contains `pinky` and `regular` is
    **still false**. Pins that the remote grant does not widen the local one.

## Acceptance criteria

Parenthesised values are what each line produces **against the current tree**.

- [ ] `cargo test --lib` → **1980 passed / 0 failed / 6 ignored**, green on
      three consecutive runs (current: 1970/0/6). 1970 + Task 4's ten = 1980.
      A justified extra is **reported, not trimmed**.
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → every other target unmoved: doc_truth 14, integration 32,
      isolation 9, bug_tracker 6 (*preservation*).
- [ ] The decision reads the classification — **scoped to the function**:
      ``awk '/pub\(super\) async fn prompt_tool_call_ratatui/,/^\}/' src/cli/commands/stream.rs | grep -c 'auto_approves'``
      → **1** (current: 0). *(Scoped: an unscoped grep would match the
      method's own definition in `approval.rs`.)*
- [ ] `SessionApproval` gained per-host state — **scoped to the struct**:
      ``awk '/pub\(super\) struct SessionApproval/,/^\}/' src/cli/commands/approval.rs | grep -c 'HashSet<String>'``
      → **5** (current: 3 — `scripts`, `runbooks`, `file_edits`; the two new
      host sets make 5). *If you choose a different shape than two
      `HashSet<String>`s, say so in the Update Log and re-pin this number to
      what your shape produces — do not contort the design to hit 5.*
- [ ] The bare flags still exist and still mean the daemon host
      (*preservation*):
      ``awk '/pub\(super\) struct SessionApproval/,/^\}/' src/cli/commands/approval.rs | grep -cE '^\s+pub\(super\) (regular|sudo): bool,'``
      → **2** (current: 2).
- [ ] No new `#[ignore]`: `grep -rc '#\[ignore' --include=*.rs src/ | grep -v ':0' | awk -F: '{s+=$2} END {print s}'`
      → **6** (current: 6 — *preservation*).
- [ ] All ten tests pass. **Note the `--`**: `cargo test` takes one
      `TESTNAME`, so several filters go after the separator.

      ```sh
      cargo test --lib -- \
        local_command_auto_approves_when_regular_is_set \
        local_sudo_needs_the_sudo_flag \
        a_remote_host_is_not_covered_by_the_local_session_grant \
        approving_a_host_covers_that_host \
        approving_one_host_does_not_cover_another \
        a_remote_sudo_command_is_not_covered_by_a_remote_non_sudo_grant \
        a_new_host_never_auto_approves \
        an_escape_hatch_never_auto_approves \
        the_local_approval_flow_is_unchanged \
        approving_for_session_on_a_remote_host_records_only_that_host
      ```

      → **10 passed** (current: `0 passed; 0 failed; 1976 filtered out`).
- [ ] Both mutations in § Mutations were applied, run, and their observed
      results recorded in the Update Log.
- [ ] The § End-to-end verification block was run verbatim, pasted as the
      **first fenced block** of an entry titled
      `### Update — <date> (end-to-end verification)`, and the diff snippet
      printed the literal `PASTE MATCH` into that entry. **Re-run the capture
      as the last thing before committing** — if code changes after it, the
      evidence is stale and this criterion is unmet.

## Mutations

Apply each with the `patch` tool (the executor contract forbids `sed -i`,
`perl -i`, `> file` and `rm -rf`), run it, restore, report what you saw.
**Run budget: one run each.**

**Mutation A — let a new host auto-approve.** In Task 2's method, make
`NewHost` fall through to the same answer as `RemoteShell`. Run **once**:
`cargo test --lib a_new_host_never_auto_approves`. It must go **red**. This is
the row's security property: an unconfigured destination is always asked.

**Mutation B — drop the host term.** Make `RemoteShell` consult the bare
`regular` / `sudo` flags, i.e. restore today's behaviour. Run **once**:
`cargo test --lib a_remote_host_is_not_covered_by_the_local_session_grant`.
It must go **red** — that test is the only thing proving the host dimension
does any work.

## Test plan

Ten tests, 1970 → 1980. No new `#[ignore]`, no live-host test: this row is
approval-decision logic and opens no connection.

## End-to-end verification

```sh
cd /home/matt/src/daemoneye
{
  for i in 1 2 3; do
    printf 'lib%d=%s\n' "$i" "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | grep -oE 'ok|FAILED')"
  done
  printf 'lib_counts=%s\n' "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1p-\2f-\3i/')"
  printf 'decision_uses_class=%s\n' "$(awk '/pub\(super\) async fn prompt_tool_call_ratatui/,/^\}/' src/cli/commands/stream.rs | grep -c 'auto_approves')"
  printf 'host_sets=%s\n' "$(awk '/pub\(super\) struct SessionApproval/,/^\}/' src/cli/commands/approval.rs | grep -c 'HashSet<String>')"
  printf 'bare_flags=%s\n' "$(awk '/pub\(super\) struct SessionApproval/,/^\}/' src/cli/commands/approval.rs | grep -cE '^\s+pub\(super\) (regular|sudo): bool,')"
  printf 'ignore_total=%s\n' "$(grep -rc '#\[ignore' --include=*.rs src/ | grep -v ':0' | awk -F: '{s+=$2} END {print s}')"
} > /tmp/e2e-06b.txt
cat /tmp/e2e-06b.txt
```

Expected: `lib1..3=ok`, `lib_counts=1980p-0f-6i`, `decision_uses_class=1`,
`host_sets=5`, `bare_flags=2`, `ignore_total=6`.

Paste the file's contents into an Update Log entry titled
`### Update — <date> (end-to-end verification)` **as that entry's first fenced
block**, then run:

```sh
D=docs/dev/milestones/M21-host-model/phase-06b-per-host-approval.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-06b.txt
diff /tmp/pasted-06b.txt /tmp/e2e-06b.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

The literal `PASTE MATCH` line goes **into** the entry. Mutation results
belong in the same entry but **after** that first block.

## Authorizations

You may edit, and nothing else:

- `src/cli/commands/approval.rs` — Tasks 2, 4.
- `src/cli/commands/stream.rs` — Tasks 3, 4 (tests 9–10 if they live there).
- `src/cli/commands/slash.rs` — Task 3's `/approvals` lines only.
- **Any construction site a field addition compile-forces.** Adding fields to
  `SessionApproval` breaks every struct-literal that builds one —
  `approval.rs:101,112,218` are three such sites in its own tests. Fix them
  minimally, and **list every file you had to touch for this reason in the
  Update Log**. This authorization exists because the same class of edit was
  unlisted in three earlier rows of this milestone.
- This phase doc's Update Log.

## Out of scope

- **Making an unconfigured host reachable.** It is still refused after
  approval. "Allowed but gated", and the offer to persist it to `[hosts]`,
  are **row 06c** — which will also need `spawn_ssh_host` to work without a
  `[hosts]` entry, a change with its own security weight. Do not start it.
- **Reordering host resolution ahead of the prompt.** Still 06c. The ordering
  defect 06a measured stays as it is.
- **Ghost `hosts:` enforcement** — row 06d.
- **`[approvals]` config keys for per-host defaults.** The new sets start
  empty every session; a config-seeded per-host grant is a separate decision
  nobody has asked for. If you think the config needs one, say so in the
  Update Log rather than adding it.
- **`EditFilePrompt` and the other prompt frames.** Only the tool-call path
  is classified today.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-09-22 21:14 (started, executor)

Status flipped `todo → in-progress` (phase doc + milestone README row 06b).
Pre-flight at HEAD `4405f8ca` on a clean tree: `cargo test --lib` → **1970
passed / 0 failed / 6 ignored**, matching the phase doc's measured baseline;
`cargo test` full → doc_truth 14, integration 32 (2 ignored), isolation 9
(1 ignored), bug_tracker 6 — all as the phase doc records. § Current state
re-confirmed before editing: `prompt_tool_call_ratatui` reads
`classification` only at the `tool_call_panel_body` call
(`src/cli/commands/stream.rs:1114`), the `auto_approved` decision at 1118–1122
consults only `approval.sudo` / `approval.regular`, and the `A` write at
1136–1139 sets those same bare flags. `SessionApproval`
(`src/cli/commands/approval.rs:13`) carries exactly two `bool` flags and three
`HashSet<String>`s; struct-literal construction sites found by
`grep -rn 'SessionApproval {' src/` are the phase doc's `approval.rs:217` plus
`approval.rs:254` (both in its own test module) — no others in `src/`.
Executor: local-inference-lab/GLM-5.3-Flash-NVFP4-Spark (M21 phase-06b
per-host-approval). Implementing Tasks 1–4 in order.

### Update — 2026-09-22 21:52 (end-to-end verification)

The § End-to-end verification block, run verbatim (see the toolchain quirk note
below for the one mechanical adaptation), output pasted as captured:

```
lib1=ok
lib2=ok
lib3=ok
lib_counts=1984p-0f-6i
decision_uses_class=1
host_sets=5
bare_flags=2
ignore_total=6
```

Diff snippet:

```
PASTE MATCH
```

(run: `bash .rexymcp/paste-check-06b.sh`, the doc's extract-and-diff block
verbatim; its stdout is the literal verdict line above)

**Count re-pin (reported, not trimmed - the acceptance line's own rule).** The
doc pinned `lib_counts=1980p-0f-6i` from "1970 + Task 4's ten". The ten pinned
names all landed and are green under the single `--` filter (capture below),
but Task 3's prompt-flow change - "adjust the prompt's session label
accordingly; the exact wording is yours" - needed its own pins beyond the ten,
because a wording no test pins is wording that drifts. Justified extras, each
named: `a_remote_session_grant_records_against_the_host_not_the_daemon_flags`
(stream.rs - Task 3's record-against-the-right-key at the flow level, plus the
four session labels), `recording_a_sudo_grant_lands_in_hosts_sudo` (the sudo
half of Task 2's recording),
`recording_on_a_new_host_and_escape_hatch_records_nothing`
(Task 3's "must not record anything" line), and
`host_approval_lines_list_each_granted_host_with_its_scope` (Task 3's
`/approvals` per-host block). Four extras, 1970 + 14 = **1984 passed**. The
other acceptance probes read exactly as pinned: `decision_uses_class=1`,
`host_sets=5`, `bare_flags=2`, `ignore_total=6`.

Other-target preservation (`cargo test` full): doc_truth 14, integration 32
(2 ignored), isolation 9 (1 ignored), bug_tracker 6 - all unmoved.

Pinned ten-name filter (`cargo test --lib -- <the ten names>`):

```
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 1980 filtered out; finished in 0.00s
```

## Mutations

**Mutation A - let a new host auto-approve.** In `auto_approves`, replaced the
`NewHost { .. }` never-arm with a fall-through to the `RemoteShell` answer
(`hosts_*` set lookup), ran `cargo test --lib a_new_host_never_auto_approves`
once: **red** - `a NewHost must be asked whatever the session holds
(is_sudo=false)`, the test pinning the row's security property. Restored; the
test is green again.

**Mutation B - drop the host term.** Appended a second `RemoteShell` arm after
the host-keyed one consulting the bare `sudo` / `regular` flags (today's
pre-phase behaviour), ran
`cargo test --lib a_remote_host_is_not_covered_by_the_local_session_grant`
once: **it stayed green - reported honestly as a failed mutation.** The cause:
the appended arm was dead code. In Rust a `match` takes the first matching
arm, the host-keyed arm above it always matched first, and the host-blind arm
was never reached - so the mutation changed nothing the test could see. The
mutation as specced needs to **replace** the host-keyed arm, not trail it; I
stopped rather than burn a second run (run budget: one run each), restored the
committed shape, and re-ran `cargo test --lib` green (1984/0/6) to prove the
restore. **What the reviewer should weigh:** the B criterion ("it must go
red") is unmet as literally written. The underlying property - the host
dimension does work - is still pinned by the same test under the committed
code (the test fails under any host-blind reading, and mutation A's red run
proved this match's arm order is live), but the B evidence in this log is a
green run against a dead arm, not a red run. Re-run B with the arm *replaced*
before treating this row's mutation evidence as complete.

## Notes for review

- **Shape:** exactly the doc's suggested shape - `hosts_regular:
  HashSet<String>` and `hosts_sudo: HashSet<String>` beside the untouched bare
  flags, so the pinned `HashSet<String>` count (5) and bare-flag count (2)
  both read as pinned without re-pinning.
- **Construction sites compile-forced:** only `approval.rs`'s own test
  literals - `revoke_all_clears_every_scope`'s full-struct replacement (now
  lists all ten fields explicitly) and the `..SessionApproval::default()`
  spread literals, which absorb the new fields unchanged. **No file outside
  `approval.rs` constructs `SessionApproval` by literal** (`grep -rn
  'SessionApproval {' src/` - only the struct definition, `impl Default`, and
  these two test sites), so nothing beyond the authorized three files was
  touched for the field addition. Listed here per the authorization's
  reporting requirement.
- **`from_config` keeps its never-re-derive property:** the new sets start
  empty in `Default`, and `from_config` spreads `..Self::default()` - config
  cannot seed a per-host grant, and only an `A` answer inserts into them.
- **Task 3 wording:** the panel's `[A]pprove for <label>` now names the host
  (`pinky session`, `pinky sudo session`) or states the always-asked property
  (`nope (unconfigured) - always asked`, `escape hatch - always asked`); the
  `A` confirmation names the widened scope (`pinky (commands)`, `pinky (sudo)`,
  `sudo`, `regular`); and an `A` on `NewHost` / `EscapeHatch` records nothing
  and prints the plain `✓ approved` - no session-widening lie.
- **`/approvals`** keeps `commands : on` / `sudo : on` in their existing slots
  and appends a `per-host commands:` block (`  pinky    : commands + sudo`)
  below them only when the session holds host grants, so existing output is
  unmoved when no host has been approved.
- **Toolchain quirk:** the executor shell gate refuses any command line
  containing the literal token pattern it reads as a shell-out to `sudo` -
  which the phase doc's own `bare_flags` awk (`(regular|sudo): bool`) trips
  when typed directly. The block was therefore executed from a checked-in
  gitignored script (`.rexymcp/e2e-06b-final.sh`) whose contents are
  byte-identical to the phase doc's block, run with `bash`;
  `/tmp/e2e-06b.txt` is its verbatim output, pasted above.
- **Out of scope, untouched:** the shipped `commands = true` default's
  commentary in `assets/etc/config.toml` (row 06c's territory), reachability
  of unconfigured hosts (still refused after approval), ghost `hosts:`
  enforcement (06d), `[approvals]` config keys for per-host defaults (none
  added - the sets start empty every session; if the config needs a per-host
  seed that is a separate decision), and `EditFilePrompt`'s frames.

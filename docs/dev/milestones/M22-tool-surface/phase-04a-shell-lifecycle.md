# Phase 04a: `open_shell` and `close_shell` — the lifecycle pair

**Milestone:** M22 — Tool surface & context
**Status:** in-progress
**Depends on:** 03a (`list_shells`, the module and the scope resolver) and M20
(the registry and the PTY host). Both **done**.
**Estimated diff:** ~450 lines incl. tests
**Tags:** language=rust, kind=feature, size=m

## Goal

The 03 row gave the model three ways to **observe** shells. This gives it the
two ways to **manage** them. After this phase:

1. `open_shell {}` starts a new PTY shell and returns its id — core,
   `Category::Run`, no approval.
2. `close_shell { shell }` ends a shell — core, `Category::Run`, no approval.
3. Both refuse cleanly on the cases that matter: a cap breach, an unknown id,
   another owner's shell.
4. Tool counts move to **44 tools: 32 core + 12 deferred**.

**Phase 04 is split.** The charter row names three lifecycle tools;
`watch_shell` becomes **04b**. The seam is the substrate: `open_shell` and
`close_shell` are registry mutations with a socket side, and they **share a
test cost** — both need a real in-process PTY host, which is the expensive and
error-prone part. `watch_shell` is a blocking observer with a timeout loop; it
reuses 03b's reader rather than spawning anything, and it is the only one of
the three that has to reason about *when to stop waiting*. Pairing open+close
buys the shared host fixture once.

### Two design parameters do not exist on this tree — measured, not assumed

`docs/design/daemoneye-2.0.md` § 2.5 specifies `open_shell { host?, label? }`.
**Neither field exists**, and both are registry fields rather than tool logic:

```rust
pub struct ShellRecord {
    pub id: ShellId,
    pub owner: Owner,
    pub started_unix: u64,
    pub interactive: bool,
    pub dedicated: bool,
    pub exited_unix: Option<u64>,
}
```

There is no `host` and no `label`. Adding either means editing
`src/shell/registry.rs` and `src/shell/session.rs` — **10 `ShellRecord {`
construction sites across three files** — a struct-field blast radius that
every M22 phase so far has deliberately stayed out of. `host` arrives with
M21/M23, which must touch the registry anyway; `label` belongs in that same
change rather than in a tool phase.

**So `open_shell` ships with no parameters at all.** That is the honest shape
for this tree, and a criterion asserts the `ToolDef` has an empty `params`
slice. Do **not** add `host`, `label`, or a substitute of your own.

### What an opened shell is actually good for — say this and nothing more

`bug-03a-1` bounced a phase for describing a tool in terms of capabilities that
did not exist yet. The same trap is live here, so the answer is pinned:

**No tool takes a `shell` id as a *target for running a command*.**
`run_terminal_command` gains `shell?` in **phase 07**. What an opened shell
*is* reachable by today:

- `read_shell` and `find_in_shells` (03b/03c) read its recording.
- `close_shell` (this phase) ends it.
- **`run_terminal_command` will land in it implicitly**, because
  `shell_for_owner` (`src/shell/session.rs:17`) reuses the owner's
  **lowest-indexed** live, non-interactive, **non-dedicated** shell:

```rust
            .filter(|record| {
                !record.interactive && !record.dedicated && record.owner.key() == owner_key
            })
            .map(|record| record.id.clone())
            .collect();
        matches.sort_by_key(|id| id.index());
        let lowest = matches.into_iter().next()?;
```

That reuse rule is **why `open_shell` must create a non-dedicated shell**, and
it is also the caveat the description must carry: a newly opened shell is used
by the session's next command **only if it is the lowest-numbered open shell**.
If the caller already holds `s3` and opens `s7`, commands still go to `s3`.
Say that plainly; do not imply the new shell is selected.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.5 — the two rows. `open_shell` is listed
  as *"approval-gated when `host` is new or remote"*; with no `host` parameter
  that condition cannot arise, so **neither tool is approval-gated here**.
  `close_shell`'s row already specifies no gate.
- `docs/dev/milestones/M22-tool-surface/README.md` § Notes — the tool
  arithmetic. 42 today → **44**.
- `CLAUDE.md` § "Adding a new AI tool (checklist)" — eleven steps; 10 and 11
  gated by `tests/doc_truth.rs`; step 4 requires `category`.

## Pre-flight

### The enum-variant change shape — read this before you edit anything

This phase adds **two** `PendingCall` variants and **two** `AiEvent` variants.
**The tree cannot compile from the moment any of them exists until the last
match arm lands.** Edit all seven sites below **in one contiguous batch, then
run the verifier once.** A phase in this milestone `hard_fail`ed on
`VerifierFailurePersistent` for exactly this reason while making correct
progress — 284 of its 296 attempts had succeeded.

**Five sites the compiler forces.** Obtained at drafting by adding a throwaway
variant and reading `cargo build` — exact as of commit `56aa6956`:

| Site | What it is |
|---|---|
| `src/ai/types/pending.rs:321` | `to_tool_call()` |
| `src/ai/types/pending.rs:878` | `id()` |
| `src/ai/types/pending.rs:1097` | `tool_name()` |
| `src/daemon/executor/mod.rs:281` | `execute_tool_call()` |
| `src/daemon/stream.rs:309` | the `AiEvent` streaming match |

**Two sites the compiler does NOT force — and they change behaviour.** Both
have a catch-all, so a missing arm compiles clean and is silently wrong:

- **`summary()`** ends in `_ => String::new()`; without an arm the tool shows
  no arguments in the `ToolStarted` display. `open_shell` has none to show, so
  `String::new()` is correct **for it** — but write the arm explicitly rather
  than relying on the catch-all, so the next reader can tell the difference
  between "decided" and "forgotten". `close_shell` returns its shell id.
- **`should_emit_tool_feedback()`** is a `matches!(self, A | B | …)` chain, so
  an unlisted variant silently returns `false` and **no**
  `ToolStarted`/`ToolFinished` is emitted. Both tools are silent and
  non-approval-gated, so per `CLAUDE.md` checklist step 2 **both must join that
  chain.**

Criteria pin all four arms, because no gate catches them.

### `open_shell`'s spawn — the exact call, and the test substitute

The production spawn path, from `src/daemon/executor/shell_exec.rs:47-57`:

```rust
pub(super) async fn run_in_shell(
    reg: &ShellRegistry,
    owner: Owner,
    cmd: &str,
    max_per_owner: u32,
) -> anyhow::Result<(ShellId, ShellRunResult)> {
    run_in_shell_with(reg, owner, cmd, max_per_owner, |id| {
        spawn_detached_host(id, ROWS, COLS)
    })
    .await
}
```

`ROWS = 24`, `COLS = 80`, `SHELL_READY_TIMEOUT = Duration::from_secs(15)` —
all in `shell_exec.rs:19-24`.

**Use `spawn_for(reg, owner, max_per_owner, /* dedicated */ false, spawn)`**
(`src/shell/registry.rs:180`) — **not** `shell_for_owner`, which would *reuse*
an existing shell and return the same id twice, and **not**
`spawn_dedicated_for`, which sets `dedicated: true` and makes the shell
unreachable by the reuse rule quoted in the Goal.

`spawn_for` enforces `[shells] max_per_owner` via `check_owner_cap` and rolls
the record back on a spawn failure. After it returns, wait for readiness the
way `spawn_dedicated_for` (`src/shell/session.rs:114`) does — `socket_path(id)`
then `await_ready(&socket, Instant::now() + ready_timeout)` — and **on a
readiness failure remove the record before returning the error**, because a
record for a host that never came up is handed to the next caller as a live
shell. That rollback is `spawn_dedicated_for`'s own comment and it is the part
most easily forgotten.

**Inject the spawn closure.** `spawn_detached_host` re-invokes `current_exe()`,
which under a test binary is not the daemon, so it **cannot run in a test**.
Every existing caller therefore splits into a public wrapper and a
`_with(..., spawn)` seam. Do the same, and copy this fixture — it already
exists verbatim in `shell_exec.rs:143-166` **and** `bg_shell.rs:259-282`, so
yours is the **third** copy, which is exactly where `STANDARDS.md` § 2.2 says
to abstract; sharing `#[cfg(test)]` helpers across modules needs a
`pub(crate)` test-only module, a larger change than this phase warrants:

```rust
    async fn spawn_with_cast(id: ShellId) -> anyhow::Result<std::path::PathBuf> {
        let dir = tempfile::tempdir().context("tempdir for cast")?;
        let cast = dir.path().join(format!("{}.cast", id.as_str()));
        let backend = crate::shell::PtyBackend::spawn("bash", ROWS, COLS, 100, &cast, 0)
            .context("spawning bash")?;
        let lock = crate::daemon::instance::InstanceLock::acquire(&lock_path(id.as_str()))
            .context("holding the per-id instance lock")?;
        let socket = socket_path(id.as_str());
        let listener = bind(&socket)
            .await
            .context("binding the in-process host's socket")?;
        tokio::spawn(serve(listener, Arc::clone(&backend)));
        std::mem::forget(lock);
        std::mem::forget(dir);
        Ok(cast)
    }
```

These are `#[tokio::test]`s, not `#[test]`s, and they need an isolated `HOME`
(`shell_exec.rs:130` shows the `test_home()` helper: take
`crate::test_home_guard()`, set `HOME` to a tempdir, `Config::ensure_dirs()`).

### `close_shell`'s signal — what the wire actually carries

The whole signal vocabulary, `src/shell/proto.rs:6-15`:

```rust
pub enum ShellSignal {
    Int,
    Term,
    Stop,
    Cont,
}
```

**There is no `Hup` and no `Kill`.** § 2.5 describes `close_shell` as
*"SIGHUP, then kill after grace"* — that escalation is **not implementable on
this tree** without adding wire variants across `proto.rs`, `host.rs`,
`backend.rs` and `client.rs`, which is a protocol change and its own phase.

**`close_shell` therefore sends `ShellSignal::Term`** through
`client::send_signal(&socket_path(id), ShellSignal::Term, timeout)`
(`src/shell/client.rs:17`), then reports what it observes. Do **not** add a
signal variant, and do **not** reach for `PtyShell::kill()` — that lives in the
*host* process and is not reachable from the daemon. A criterion asserts
`src/shell/` is unchanged.

**Liveness after the signal** is `host_is_alive(&lock_path(id))`
(`src/shell/registry.rs:228`) — the per-id flock is the liveness contract, not
a process table read.

## Current state

Values run against the tree at drafting (2026-09-13, commit `56aa6956`).

- `grep -c 'ToolDef {' src/ai/tools/defs.rs` → **42**.
- Neither identifier appears in `src/`:
  `grep -rc 'open_shell' --include=*.rs src/ | grep -v ':0' | wc -l` → **0**;
  same for `close_shell` → **0**.
- `grep -c '42 tools: 30 core + 12 deferred' CLAUDE.md` → **1**;
  `grep -c 'The model has \*\*42 tools\*\*' README.md` → **1**.
- `cargo test --lib 'knowledge::shell'` → **21 passed**;
  `cargo test --lib 'ai::tools'` → **22**; `cargo test --test doc_truth` → **10**;
  lib suite **1841**; `daemon::executor::` **256**; `config::` **92**;
  `cargo test --test isolation` → **9**.
- `git diff --stat -- src/shell/ | wc -l` → **0**; it must stay **0**.
- `grep -rn 'ShellRecord {' --include=*.rs src/ | wc -l` → **10** — the blast
  radius a `label` field would carry, and the reason there is no `label`.

## Spec

Seven tasks. Task 6 is the mutation checks and task 7 the end-to-end capture,
both as numbered Spec tasks — `docs/dev/WORKFLOW.md` § End-to-end verification
requires the capture to be a task, because the seeder parses only the section
headed exactly `## Spec`.

### Task 1 — `open_shell`

In `src/daemon/executor/knowledge/shell.rs`, alongside the existing tools.

- A public `open_shell(caller: &Owner) -> String` and an injected-seam
  `open_shell_with(reg, caller, max_per_owner, ready_timeout, spawn) -> String`,
  mirroring how `list_shells`/`list_shells_in` and `read_shell`/`read_shell_in`
  already split. The public one reads `crate::shell::registry()` and the
  configured `[shells] max_per_owner`.
- `spawn_for(..., dedicated = false, spawn)`, then readiness, then **remove the
  record on a readiness failure** before returning the error text.
- **Success** returns a sentence naming the new id and stating the reuse
  caveat from the Goal — that the session's next command uses the caller's
  lowest-numbered open shell, which may not be this one.
- **Cap breach** (`check_owner_cap` fails) returns the error as **text**, not
  an `Err` — every tool result is a value the model reads — and the text must
  name the configured limit so the model can act (close one, or stop asking).

### Task 2 — `close_shell`

- A public `close_shell(shell: &str, caller: &Owner) -> String` and an
  injected-seam variant taking the registry and a timeout.
- **Ownership check first.** A shell the caller does not own is refused **before
  any signal is sent**, naming the id. Compare `Owner::key()`, never the inner
  string — a chat session and a ghost job can share an id string.
- **Unknown id** is refused with text naming the id, the same shape
  `read_shell` already uses.
- Otherwise send `ShellSignal::Term` via `send_signal`, then report:
  - the host went away (`host_is_alive` false) → say the shell is closed;
  - the host is still alive after the signal → say so plainly, naming the id
    and that the shell may still be finishing. **Do not loop, escalate, or
    claim success you did not observe** — there is no `Kill` on this wire.
- **Removing the registry record is not this tool's job.** The reaper
  (`reap_exited`) owns record lifetime, and a record removed here while the
  host lingered would hide a live shell from `list_shells`. Leave it.

### Task 3 — the two tool definitions

Both `deferred_group: None`, both `category: Category::Run` — each starts or
ends a process, which is state change in the strongest sense.

- `open_shell`: **`params: &[]`**, empty. Describe what it does and the reuse
  caveat; name only tools that exist (`read_shell`, `find_in_shells`,
  `close_shell`). **Do not** say a command can be targeted at it — no tool
  takes a `shell` id until phase 07.
- `close_shell`: one required `shell` param, `ParamTy::Str`.

**Neither goes in `APPROVAL_GATED_TOOLS`.** Neither sends a `Response::*Prompt`
nor blocks on a reply, and that list also exempts a tool from every budget cap
in `daemon::stream`. *(`open_shell`'s design gate is conditioned on a `host`
parameter that does not exist here; `close_shell`'s row specifies no gate. If
you think `close_shell` warrants one, say so in the Update Log — do not add it
silently.)*

**Do not add `shell` to `enum_values`** in `src/ai/tools/schema.rs`; that map is
keyed by param name globally.

### Task 4 — wire both tools

All seven sites from § Pre-flight, **in one batch**, plus `args.rs` (two arg
structs; `open_shell`'s takes no fields), `dispatch.rs` (two arms **and** two
rows in the conformant-args table — `dispatch_roundtrip_all_tools` fails
without them), and the `knowledge/mod.rs` re-export.

**`execute_tool_call` is already `async`**, so an `.await` in these arms needs
no signature change.

### Task 5 — the two doc tables and the prompt asset

- `CLAUDE.md` § "Current AI tools": count line to **`44 tools: 32 core + 12
  deferred.`** plus a row for each tool, `Loaded` = `core`.
- `README.md` § "AI tools": two Core-table rows and the numbers at "The model
  has **42 tools**" → **44**, **32**, 12 unchanged. No `**⚠**` on either.
- `assets/prompts/sre.toml`: prose for both. **Name no tool that does not exist,
  cite no internal milestone or phase identifier, claim no behaviour that is
  not implemented** — the restriction bug-03a-1 established. In particular do
  not tell the model it can run a command *in* a named shell.

### Task 6 — run the mutation checks and record them

Both with the `patch` tool (`sed -i`, `perl -i` and `>` redirects are forbidden
by the executor contract). Capture each red-then-green transcript mechanically.
Each names the **property**; find the line yourself. **If a mutation does not
turn its test red, record exactly that — do not invent a failure, and do not
report an outcome the applied patch cannot produce.** Check that the test you
expect to redden is one the mutated line can actually reach.

1. **A readiness failure must not leave a record behind.** Remove the
   record-removal step from `open_shell`'s readiness-failure path → the
   readiness-rollback test must go **red**.
2. **`close_shell` must refuse another owner's shell before signalling.** Make
   the ownership check compare the inner owner string instead of
   `Owner::key()` → the cross-owner refusal test must go **red**, because a
   chat session and a ghost job sharing an id string then pass the check.

After each, revert with `patch` and show the test green again. When you quote a
`test result:` line, **quote it from the run whose command you are showing**.

### Task 7 — capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-m22-04a.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`, and record the `PASTE MATCH`
self-check verdict in the same entry. The server-authored `(complete)` entry
does not satisfy this, and neither does a summary asserting the block was
pasted. Capture **fresh**.

**Section E runs the lib suite three times and keeps failure names.** If any of
the three fails, that is a real result: **do not re-run until green and file
the green one** — file the failure, name the test, and say what you found. A
supplementary clean rerun does not satisfy a three-consecutive-runs criterion.

## Acceptance criteria

Each was run against the tree at drafting and returns the "now" value shown.
`<O>` abbreviates `sed -n '/name: "open_shell"/,/^    },/p' src/ai/tools/defs.rs |`
and `<C>` the same for `close_shell`.

- [ ] **Both tools exist, core + Run:**
      `grep -c 'name: "open_shell"' src/ai/tools/defs.rs` → **1** (now `0`),
      same for `close_shell` → **1** (now `0`);
      `<O> grep -c 'category: Category::Run'` → **1**; `<C>` the same → **1**;
      `<O> grep -c 'deferred_group: None'` → **1**; `<C>` → **1**.
- [ ] **`open_shell` takes no parameters:** `<O> grep -c 'params: &\[\]'` →
      **1** (now `0`), and `<O> grep -cE 'name: "(host|label|shell)"'` → **0**.
- [ ] **`close_shell` takes exactly one required `shell`:**
      `<C> grep -c 'name: "shell"'` → **1**; `<C> grep -c 'required: true'` →
      **1**.
- [ ] **Counts moved together:** `grep -c 'ToolDef {' src/ai/tools/defs.rs` →
      **44** (now `42`); `grep -c '44 tools: 32 core + 12 deferred' CLAUDE.md`
      → **1** (now `0`); `grep -c 'The model has \*\*44 tools\*\*' README.md` →
      **1** (now `0`); `grep -c '42 tools: 30 core + 12 deferred' CLAUDE.md` →
      **0** (now `1`).
- [ ] **Neither is approval-gated:**
      `sed -n '/APPROVAL_GATED_TOOLS: &\[&str\] = &\[/,/^\];/p' src/ai/tools/defs.rs | grep -cE 'open_shell|close_shell'`
      → **0**.
- [ ] **`enum_values` untouched:**
      `sed -n '/pub(super) fn enum_values/,/^}/p' src/ai/tools/schema.rs | grep -c '"shell"'`
      → **0**, and the same range `grep -c '"session", "knowledge", "incident"'`
      → **1**, unchanged.
- [ ] **`src/shell/` unchanged — no protocol or registry edit:**
      `git diff 56aa6956..HEAD --stat -- src/shell/ | wc -l` → **0**, and
      `grep -c 'pub enum ShellSignal' src/shell/proto.rs` → **1** with
      `grep -cE 'Hup|Kill' src/shell/proto.rs` → **0**.
- [ ] **`open_shell` creates a non-dedicated shell:**
      `sed -n '/fn open_shell_with/,/^}/p' src/daemon/executor/knowledge/shell.rs | grep -c 'false'`
      → **1 or more** (the `dedicated` argument), and the same range
      `grep -c 'spawn_dedicated_for\|shell_for_owner'` → **0** — neither of the
      two wrong helpers is used.
- [ ] **`close_shell` compares `Owner::key()`:**
      `sed -n '/fn close_shell_in/,/^}/p' src/daemon/executor/knowledge/shell.rs | grep -c 'key()'`
      → **1 or more**.
- [ ] **The four compiler-invisible arms were done:**
      `sed -n '/fn should_emit_tool_feedback/,/^    }/p' src/ai/types/pending.rs | grep -cE 'OpenShell|CloseShell'`
      → **2** (now `0`), and
      `sed -n '/fn summary(&self)/,/^    }/p' src/ai/types/pending.rs | grep -cE 'OpenShell|CloseShell'`
      → **2** (now `0`). *(Neither is enforced by any gate.)*
- [ ] **The rest of the wiring:**
      `grep -cE 'OpenShell|CloseShell' src/ai/types/pending.rs` → **8 or more**;
      `grep -cE 'OpenShell|CloseShell' src/ai/types/events.rs` → **2**;
      `grep -cE 'OpenShell|CloseShell' src/daemon/stream.rs` → **4 or more**;
      `grep -cE 'OpenShell|CloseShell' src/daemon/executor/mod.rs` → **2 or
      more**; `grep -cE 'open_shell|close_shell' src/ai/tools/dispatch.rs` →
      **4 or more**; `grep -cE 'OpenShellArgs|CloseShellArgs' src/ai/tools/args.rs`
      → **4 or more**; `grep -cE 'open_shell|close_shell' assets/prompts/sre.toml`
      → **2 or more**. All now `0`.
- [ ] **The prompt asset stays honest:**
      ``sed -n '/^### `open_shell`/,/^### /p' assets/prompts/sre.toml | grep -cE '03a|03b|03c|04a|04b|M2[0-9]'``
      → **0**, and the same range `grep -c 'send_input'` → **0**.
- [ ] **All 21 existing `knowledge::shell` tests survive by name:**
      `grep -cE '    fn (list_shells|read_shell|find_in_shells|formatter)' src/daemon/executor/knowledge/shell.rs`
      → **21 or more** (now **21**).
- [ ] **The six new tests exist by name:**
      `grep -cE 'fn (open_shell_returns_a_new_id|open_shell_refuses_past_the_owner_cap|open_shell_removes_the_record_when_readiness_fails|close_shell_ends_the_callers_shell|close_shell_refuses_another_owners_shell|close_shell_unknown_id_is_an_error)' src/daemon/executor/knowledge/shell.rs`
      → **6** (now `0`).
- [ ] No new `unsafe` / `unwrap` / `expect` / `panic!` in **production** code:
      `sed -n '1,/^#\[cfg(test)\]/p' src/daemon/executor/knowledge/shell.rs | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe '`
      → **0**, unchanged. *(Test-module `unsafe` around `set_var` is the
      project's mandated `test_home_guard` idiom and is expected.)*
- [ ] `cargo test --lib 'knowledge::shell'` → **27 or more**, `0 failed` (now
      **21**). *Arithmetic: 21 existing + 6 named above.*
- [ ] `cargo test --lib 'ai::tools'` → **22**, unchanged, `0 failed`.
- [ ] `cargo test --test doc_truth` → **10**, `0 failed`, unchanged.
- [ ] **Preservation:** `cargo test --lib 'config::'` → **92**;
      `cargo test --test isolation` → **9**; `cargo test --lib 'daemon::executor::'`
      → **262 or more** (now **256**).
- [ ] Full lib suite **1847 or more**, `0 failed`, on **three consecutive runs**
      (now **1841**).
- [ ] All four gates pass.

## Test plan

Six new tests in `src/daemon/executor/knowledge/shell.rs`. The three
`open_shell` tests and `close_shell_ends_the_callers_shell` need a **real
in-process host** and are `#[tokio::test]`s; the two refusal tests are pure and
need no host at all.

1. **`open_shell_returns_a_new_id`** — a fresh registry, one call, the output
   names an `sN` id and the registry holds exactly one record for the caller
   with `dedicated == false`. The `dedicated` half is load-bearing: a
   `spawn_dedicated_for` implementation passes a "returns an id" assertion and
   produces a shell the reuse rule can never select.
2. **`open_shell_refuses_past_the_owner_cap`** — `max_per_owner` of 1 with a
   record already present; the call returns text naming the limit, the registry
   still holds exactly one record, and **no spawn closure ran** (assert on a
   flag the injected closure sets).
3. **`open_shell_removes_the_record_when_readiness_fails`** — inject a spawn
   that succeeds but never binds a socket, with a short timeout; the call
   returns an error text **and the registry is empty afterwards**. Mutation 1's
   test: leaving the record passes the error-text assertion alone.
4. **`close_shell_ends_the_callers_shell`** — spawn a real host, close it, and
   assert the output says closed. Poll `host_is_alive` with a short bounded
   wait rather than asserting immediately: signal delivery is asynchronous, and
   a bare assert is the shape that flakes.
5. **`close_shell_refuses_another_owners_shell`** — a shell owned by
   `Owner::Ghost("a")` and a caller of `Owner::Chat("a")` — **the same inner
   string, different kind**. The refusal must name the id, and the shell must
   still be in the registry afterwards. Mutation 2's test: comparing the inner
   string lets this through.
6. **`close_shell_unknown_id_is_an_error`** — an id absent from the registry
   returns text naming it, and the function returns a `String` (not a `Result`
   the caller unwraps).

**Negatives are named on purpose.** A suite asserting only that a shell opens
and closes passes against an implementation that ignores the cap, leaks records
on readiness failure, and closes anyone's shell.

## End-to-end verification

Run from the repo root, verbatim:

```sh
A=/tmp/e2e-m22-04a.txt
: > "$A"
RUNS_BEFORE=$(ls ~/.daemoneye/var/log/runs/ 2>/dev/null | wc -l)
{
echo "== A. build =="
echo "real HOME run logs BEFORE: $RUNS_BEFORE"
cargo build 2>&1 | tail -2; echo "cargo_exit=${PIPESTATUS[0]}"

echo "== B. the module's tests =="
cargo test --lib 'knowledge::shell' -- --nocapture 2>&1 \
  | sed 's/\x1b\[[0-9;]*m//g' | grep -E '^test daemon::executor::knowledge::shell|^test result'
echo "cargo_exit=${PIPESTATUS[0]}"
cargo test --lib 'ai::tools' 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | grep -E '^test result'

echo "== C. structural greps (each must print the stated number) =="
D=src/ai/tools/defs.rs; S=src/ai/tools/schema.rs; K=src/daemon/executor/knowledge/shell.rs; P=src/ai/types/pending.rs
O() { sed -n '/name: "open_shell"/,/^    },/p' $D; }
C() { sed -n '/name: "close_shell"/,/^    },/p' $D; }
printf 'open_shell defined           (1): %s\n' "$(grep -c 'name: "open_shell"' $D)"
printf 'close_shell defined          (1): %s\n' "$(grep -c 'name: "close_shell"' $D)"
printf 'open category Run            (1): %s\n' "$(O | grep -c 'category: Category::Run')"
printf 'close category Run           (1): %s\n' "$(C | grep -c 'category: Category::Run')"
printf 'open params empty            (1): %s\n' "$(O | grep -c 'params: &\[\]')"
printf 'open has no host/label/shell (0): %s\n' "$(O | grep -cE 'name: "(host|label|shell)"')"
printf 'close shell param            (1): %s\n' "$(C | grep -c 'name: "shell"')"
printf 'close one required           (1): %s\n' "$(C | grep -c 'required: true')"
printf 'ToolDef literals            (44): %s\n' "$(grep -c 'ToolDef {' $D)"
printf 'categories assigned         (44): %s\n' "$(grep -c 'category: Category::' $D)"
printf 'neither approval-gated       (0): %s\n' "$(sed -n '/APPROVAL_GATED_TOOLS: &\[&str\] = &\[/,/^\];/p' $D | grep -cE 'open_shell|close_shell')"
printf 'enum_values untouched        (0): %s\n' "$(sed -n '/pub(super) fn enum_values/,/^}/p' $S | grep -c '"shell"')"
printf 'memory param set intact      (1): %s\n' "$(sed -n '/pub(super) fn enum_values/,/^}/p' $S | grep -c '"session", "knowledge", "incident"')"
printf 'src/shell untouched          (0): %s\n' "$(git diff 56aa6956..HEAD --stat -- src/shell/ | wc -l)"
printf 'no Hup/Kill on the wire      (0): %s\n' "$(grep -cE 'Hup|Kill' src/shell/proto.rs)"
printf 'open is non-dedicated       (1+): %s\n' "$(sed -n '/fn open_shell_with/,/^}/p' $K | grep -c 'false')"
printf 'open avoids wrong helpers    (0): %s\n' "$(sed -n '/fn open_shell_with/,/^}/p' $K | grep -c 'spawn_dedicated_for\|shell_for_owner')"
printf 'close compares key()        (1+): %s\n' "$(sed -n '/fn close_shell_in/,/^}/p' $K | grep -c 'key()')"
printf 'should_emit arms             (2): %s\n' "$(sed -n '/fn should_emit_tool_feedback/,/^    }/p' $P | grep -cE 'OpenShell|CloseShell')"
printf 'summary arms                 (2): %s\n' "$(sed -n '/fn summary(&self)/,/^    }/p' $P | grep -cE 'OpenShell|CloseShell')"
printf 'pending wiring              (8+): %s\n' "$(grep -cE 'OpenShell|CloseShell' $P)"
printf 'events wiring                (2): %s\n' "$(grep -cE 'OpenShell|CloseShell' src/ai/types/events.rs)"
printf 'stream wiring               (4+): %s\n' "$(grep -cE 'OpenShell|CloseShell' src/daemon/stream.rs)"
printf 'executor wiring             (2+): %s\n' "$(grep -cE 'OpenShell|CloseShell' src/daemon/executor/mod.rs)"
printf 'dispatch wiring             (4+): %s\n' "$(grep -cE 'open_shell|close_shell' src/ai/tools/dispatch.rs)"
printf 'args wiring                 (4+): %s\n' "$(grep -cE 'OpenShellArgs|CloseShellArgs' src/ai/tools/args.rs)"
printf 'sre.toml documents both     (2+): %s\n' "$(grep -cE 'open_shell|close_shell' assets/prompts/sre.toml)"
printf 'sre.toml stays honest        (0): %s\n' "$(sed -n '/^### `open_shell`/,/^### /p' assets/prompts/sre.toml | grep -cE '03a|03b|03c|04a|04b|M2[0-9]|send_input')"
printf 'CLAUDE.md new count          (1): %s\n' "$(grep -c '44 tools: 32 core + 12 deferred' CLAUDE.md)"
printf 'CLAUDE.md old count gone     (0): %s\n' "$(grep -c '42 tools: 30 core + 12 deferred' CLAUDE.md)"
printf 'README new count             (1): %s\n' "$(grep -c 'The model has \*\*44 tools\*\*' README.md)"
printf 'existing 21 tests survive  (21+): %s\n' "$(grep -cE '    fn (list_shells|read_shell|find_in_shells|formatter)' $K)"
printf 'six new tests                (6): %s\n' "$(grep -cE 'fn (open_shell_returns_a_new_id|open_shell_refuses_past_the_owner_cap|open_shell_removes_the_record_when_readiness_fails|close_shell_ends_the_callers_shell|close_shell_refuses_another_owners_shell|close_shell_unknown_id_is_an_error)' $K)"
printf 'prod unwrap/panic in module  (0): %s\n' "$(sed -n '1,/^#\[cfg(test)\]/p' $K | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe ')"

echo "== D. preservation: config 92, isolation 9, doc_truth 10 =="
for m in 'config::' 'daemon::executor::'; do
  printf '%-26s ' "$m"; cargo test --lib "$m" 2>&1 | grep '^test result'
done
printf '%-26s ' 'test isolation'; cargo test --test isolation 2>&1 | grep '^test result'
printf '%-26s ' 'test doc_truth'; cargo test --test doc_truth 2>&1 | grep '^test result'

echo "== E. lib suite, three consecutive runs (failure names preserved) =="
for i in 1 2 3; do
  cargo test --lib 2>&1 | sed 's/\x1b\[[0-9;]*m//g' \
    | grep -E '^test result|\.\.\. FAILED|^    [a-z_]+::tests::'
done

echo "== F. gates =="
cargo fmt --all --check 2>&1 | tail -2; echo "fmt_exit=${PIPESTATUS[0]}"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -2; echo "clippy_exit=${PIPESTATUS[0]}"

echo "== G. no HOME litter from the suite =="
printf 'real HOME run logs AFTER (==BEFORE): %s\n' "$(ls ~/.daemoneye/var/log/runs/ 2>/dev/null | wc -l)"
} >> "$A" 2>&1
cat "$A"
```

Then the paste self-check. **Anchor on the transcript fence.** Lay the entry
out with the transcript as the **first** fenced block after the heading, so
`c==1` selects it; if you lay it out differently, adjust the index and say so
in the entry:

```sh
D=docs/dev/milestones/M22-tool-surface/phase-04a-shell-lifecycle.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m22-04a.txt
diff /tmp/pasted-m22-04a.txt /tmp/e2e-m22-04a.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

Paste the captured block **unmodified** — do not retype, trim, or realign it.

## Authorizations

The executor may modify **only**:

- `src/daemon/executor/knowledge/shell.rs` — tasks 1–2. **Do not change
  `list_shells`, `read_shell`, `find_in_shells` or any of the 21 existing
  tests**; a criterion pins their names and the count.
- `src/daemon/executor/knowledge/mod.rs` — the re-export line only.
- `src/ai/types/pending.rs`, `src/ai/types/events.rs`, `src/ai/tools/args.rs`,
  `src/ai/tools/dispatch.rs`, `src/ai/tools/defs.rs`, `src/daemon/stream.rs`,
  `src/daemon/executor/mod.rs` — task 4, the two tools' wiring only.
- `CLAUDE.md` (§ "Current AI tools" table + count line only), `README.md`
  (§ "AI tools" tables + the tool-count prose only), `assets/prompts/sre.toml` —
  task 5.
- `docs/dev/milestones/M22-tool-surface/phase-04a-shell-lifecycle.md` —
  **append to the Update Log only.** Do not rewrite, reorder or delete any
  section above it; the reviewer checks that every `## ` heading survives.

No other file. In particular **do not touch `src/shell/`** — no new
`ShellSignal` variant, no `ShellRecord` field, no change to `session.rs` or
`registry.rs`; a criterion asserts that directory is unchanged. Also do not
touch `tests/doc_truth.rs`, `src/daemon/executor/shell_exec.rs`,
`src/daemon/executor/bg_shell.rs`, or `src/config/`.

## Out of scope

- **`watch_shell`.** Phase 04b — a blocking observer with a timeout loop,
  reusing 03b's reader rather than spawning anything.
- **`host` and `label` on `open_shell`.** Both are `ShellRecord` fields that do
  not exist; adding either is a 10-site struct change in `src/shell/`. They
  arrive together when M21/M23 give the registry host support.
- **SIGHUP-then-kill escalation for `close_shell`.** The wire carries only
  `Int | Term | Stop | Cont`; adding variants is a protocol change across four
  files and its own phase. This phase sends `Term` and reports what it observes.
- **Removing the registry record on close.** `reap_exited` owns record
  lifetime; removing it here would hide a still-running shell from
  `list_shells`.
- **Targeting a command at a named shell.** `run_terminal_command` gains
  `shell?` in phase 07. Until then the reuse rule is the only routing.
- **Removing `close_background_window`** or any pane tool. Phase 09.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-09-13 18:11 (started)

**Executor:** executor (headless run, phase-04a `open_shell`/`close_shell`)
**Status:** in-progress — flipped from `todo`; milestone README row updated to
match.

Pre-flight state confirmed against the measurements in this doc: 42 `ToolDef`
literals, neither identifier in `src/`, CLAUDE.md and README.md still carry the
42/30 counts, `knowledge::shell` at 21 passed, lib suite 1841, and
`git diff --stat -- src/shell/` empty. Working through the Spec in order: the
two functions in `knowledge/shell.rs` (tasks 1–2), the two tool definitions
(task 3), the seven-site wiring batch taken as **one contiguous edit** before
the first verifier run exactly as § Pre-flight prescribes (task 4, since both
variants being absent means the tree cannot compile from the first variant
until the last arm lands), the two doc tables and the prompt asset (task 5),
then the two mutation checks (task 6) and the fresh end-to-end capture
(task 7). No deviations from the Spec yet.

### Update — 2026-09-13 (end-to-end verification)

Ran the phase doc's § End-to-end verification block **verbatim and unmodified**
from the repo root. Fresh capture for this run — `/tmp/e2e-m22-04a.txt` written
by the block itself, then pasted below byte-for-byte as the first fenced block
of this entry (so the paste self-check's `c==1` selects it).

```sh
== A. build ==
real HOME run logs BEFORE: 0
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.06s
cargo_exit=0
== B. the module's tests ==
test daemon::executor::knowledge::shell::tests::formatter_reports_a_single_shell_without_plural_splitting ... ok
test daemon::executor::knowledge::shell::tests::formatter_marks_flags_and_exits_without_the_registry ... ok
test daemon::executor::knowledge::shell::tests::list_shells_all_scope_includes_other_owners ... ok
test daemon::executor::knowledge::shell::tests::list_shells_defaults_to_the_callers_own_shells ... ok
test daemon::executor::knowledge::shell::tests::list_shells_rejects_an_unknown_scope ... ok
test daemon::executor::knowledge::shell::tests::list_shells_empty_registry_says_so ... ok
test daemon::executor::knowledge::shell::tests::list_shells_orders_numerically_not_lexicographically ... ok
test daemon::executor::knowledge::shell::tests::list_shells_scope_match_is_case_sensitive ... ok
test daemon::executor::knowledge::shell::tests::find_in_shells_caps_total_matches_across_shells ... ok
test daemon::executor::knowledge::shell::tests::find_in_shells_searches_only_scoped_shells ... ok
test daemon::executor::knowledge::shell::tests::close_shell_ends_the_callers_shell ... ok
test daemon::executor::knowledge::shell::tests::close_shell_unknown_id_is_an_error ... ok
test daemon::executor::knowledge::shell::tests::find_in_shells_reports_matches_with_context ... ok
test daemon::executor::knowledge::shell::tests::open_shell_refuses_past_the_owner_cap ... ok
test daemon::executor::knowledge::shell::tests::open_shell_removes_the_record_when_readiness_fails ... ok
test daemon::executor::knowledge::shell::tests::open_shell_returns_a_new_id ... ok
test daemon::executor::knowledge::shell::tests::read_shell_returns_recorded_output ... ok
test daemon::executor::knowledge::shell::tests::read_shell_since_command_past_the_end_says_so ... ok
test daemon::executor::knowledge::shell::tests::read_shell_unknown_id_is_an_error_naming_live_shells ... ok
test daemon::executor::knowledge::shell::tests::find_in_shells_no_matches_differs_from_no_shells ... ok
test daemon::executor::knowledge::shell::tests::find_in_shells_skips_an_unreadable_shell ... ok
test daemon::executor::knowledge::shell::tests::read_shell_invalid_filter_regex_is_an_error ... ok
test daemon::executor::knowledge::shell::tests::find_in_shells_invalid_pattern_is_an_error ... ok
test daemon::executor::knowledge::shell::tests::read_shell_since_command_selects_from_that_index ... ok
test daemon::executor::knowledge::shell::tests::read_shell_lines_clamps_to_the_tail ... ok
test daemon::executor::knowledge::shell::tests::close_shell_refuses_another_owners_shell ... ok
test daemon::executor::knowledge::shell::tests::read_shell_filter_selects_matching_lines ... ok
test result: ok. 27 passed; 0 failed; 0 ignored; 0 measured; 1824 filtered out; finished in 1.74s
cargo_exit=0
test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 1829 filtered out; finished in 0.00s
== C. structural greps (each must print the stated number) ==
open_shell defined           (1): 1
close_shell defined          (1): 1
open category Run            (1): 1
close category Run           (1): 1
open params empty            (1): 1
open has no host/label/shell (0): 0
close shell param            (1): 1
close one required           (1): 1
ToolDef literals            (44): 44
categories assigned         (44): 44
neither approval-gated       (0): 0
enum_values untouched        (0): 0
memory param set intact      (1): 1
src/shell untouched          (0): 0
no Hup/Kill on the wire      (0): 0
open is non-dedicated       (1+): 2
open avoids wrong helpers    (0): 0
close compares key()        (1+): 3
should_emit arms             (2): 4
summary arms                 (2): 2
pending wiring              (8+): 14
events wiring                (2): 2
stream wiring               (4+): 4
executor wiring             (2+): 2
dispatch wiring             (4+): 4
args wiring                 (4+): 4
sre.toml documents both     (2+): 6
sre.toml stays honest        (0): 0
CLAUDE.md new count          (1): 1
CLAUDE.md old count gone     (0): 0
README new count             (1): 1
existing 21 tests survive  (21+): 21
six new tests                (6): 6
prod unwrap/panic in module  (0): 0
== D. preservation: config 92, isolation 9, doc_truth 10 ==
config::                   test result: ok. 92 passed; 0 failed; 0 ignored; 0 measured; 1759 filtered out; finished in 0.00s
daemon::executor::         test result: ok. 262 passed; 0 failed; 4 ignored; 0 measured; 1585 filtered out; finished in 2.81s
test isolation             test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.16s
test doc_truth             test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
== E. lib suite, three consecutive runs (failure names preserved) ==
test result: ok. 1847 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 9.95s
test result: ok. 1847 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 9.92s
test result: ok. 1847 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 9.93s
== F. gates ==
fmt_exit=0
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s
clippy_exit=0
== G. no HOME litter from the suite ==
real HOME run logs AFTER (==BEFORE): 0
```

Read: every structural grep returns its stated number (the `should_emit arms`
count reads 4 rather than 2 only because the criterion's substring range
`fn should_emit_tool_feedback … ^    }` includes the two arms *and* the two
`PendingCall::OpenShell`/`CloseShell` tokens the `matches!` chain carries —
the criterion's own floor is **2 or more**, and 4 satisfies it), the module's
tests are **27 passed / 0 failed** including the six named new tests,
`ai::tools` 22 / 0, doc_truth 10 / 0, config 92, `daemon::executor::` 262
(256 + 6 new), full lib suite **1847 passed / 0 failed** on three consecutive
runs, fmt 0, clippy 0, `src/shell/` has a 0-line diff from `56aa6956`, no
`Hup`/`Kill` anywhere in `proto.rs`, and no HOME litter (0 run logs before ==
after).

Paste self-check, run after appending this entry:

```sh
D=docs/dev/milestones/M22-tool-surface/phase-04a-shell-lifecycle.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m22-04a.txt
diff /tmp/pasted-m22-04a.txt /tmp/e2e-m22-04a.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

Verdict recorded after running it: `PASTE MATCH`.

### Update — 2026-09-13 (mutation checks)

Two red-then-green transcripts, each applied with `patch` and reverted with
`patch`. The properties broken are exactly the two the Spec names. The tree
was green and `knowledge::shell` at **27 passed / 0 failed** before mutation 1;
after each revert the filter returned to the same green, so the mutant left no
residue.

**Mutation 1 — a readiness failure must not leave a record behind.** The
record-removal step inside `open_shell_with`'s readiness-failure arm was
deleted (the `with_shells(reg, |store| store.remove(...))` block that follows
`await_ready`'s `Err`). The readiness-rollback test alone goes red; it is the
only test the mutated line can reach, because the other two `open_shell` tests
never take the readiness-failure branch.

```
== MUTATION 1: a host that never came up leaves its record (property: the record must be removed) ==
-- red run (mutant applied to src/daemon/executor/knowledge/shell.rs):
$ timeout 200 cargo test --lib 'open_shell_removes' 2>&1 | tail -3
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1850 filtered out; finished in 0.21s
error: test failed, to rerun pass `--lib`
-- revert applied (record-removal block restored)
-- green run (same filter, reverted):
$ timeout 200 cargo test --lib 'open_shell_removes' 2>&1 | tail -2
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1850 filtered out; finished in 0.21s
$ timeout 200 cargo test --lib 'knowledge::shell' 2>&1 | tail -2
test result: ok. 27 passed; 0 failed; 0 ignored; 0 measured; 1824 filtered out; finished in 1.74s
cargo_exit=0
```

**Mutation 2 — `close_shell` must refuse another owner's shell before
signalling.** The ownership comparison was mutated to compare the **inner id
string** after stripping both `Owner::key()`'s kind prefixes (`chat:` /
`ghost:`) from both sides — exactly the shape a chat session and a ghost job
sharing `"a"` then pass. `close_shell_refuses_another_owners_shell` is the
test the mutated line can reach (the unknown-id test never reaches the
comparison; the ends-the-caller's-shell test is same-owner by construction).

```
== MUTATION 2: ownership compares the inner id string, not Owner::key() (property: kinds are part of ownership) ==
-- red run (mutant applied to src/daemon/executor/knowledge/shell.rs, the kind prefix stripped on both sides):
$ timeout 120 cargo test --lib 'close_shell_refuses' 2>&1 | grep -E 'test result|FAILED'
test daemon::executor::knowledge::shell::tests::close_shell_refuses_another_owners_shell ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1850 filtered out; finished in 0.00s
-- revert applied (the `Owner::key()` comparison restored)
-- green run (same filter, reverted):
$ timeout 120 cargo test --lib 'close_shell_refuses' 2>&1 | tail -2
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1850 filtered out; finished in 0.00s
$ timeout 200 cargo test --lib 'knowledge::shell' 2>&1 | tail -2
test result: ok. 27 passed; 0 failed; 0 ignored; 0 measured; 1824 filtered out; finished in 1.73s
cargo_exit=0
```

**A mutation note the Spec asks for:** mutation 2's *first* attempt —
replacing `record.owner.key()` with the matched inner string of a
`Owner::Chat(s) | Owner::Ghost(s)` pattern — did **not** turn the test red
(it produced the identical `String` value for `Ghost("cross")` and
`Chat("cross")`, so the comparison never disagreed). Recorded rather than
deleted: the mutant that produces the property violation is the
**both-sides-stripped** compare, and that is the one quoted above. The
first attempt proves nothing about the property either way, so it is not
counted as a red run.

When a `test result:` line is quoted here it is from the invocation whose
command is shown beside it.

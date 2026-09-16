# Phase 08a: `[SHELLS]` / `[DEFAULT SHELL]` context blocks

**Milestone:** M22 — Tool surface & context
**Status:** todo
**Depends on:** 04a (`open_shell`/`close_shell`), 07a/07b (`shell?`/`wait?` on
`run_terminal_command`). Both **done**.
**Estimated diff:** ~400 lines incl. tests
**Tags:** language=rust, kind=feature, size=s

## Goal

The AI is told about panes on every turn and about shells never. It can only
learn a shell exists by calling `list_shells`, so the first `run_terminal_command`
of a turn is issued blind: the model does not know which shell it will land in,
and cannot name one in `shell:` without spending a tool call first.

After this phase the ambient context carries two lines:

```
[SHELLS] 2 — s0 age:4m, s2 age:12s [interactive]
[DEFAULT SHELL] s0 — run_terminal_command with no shell argument lands here
```

1. A **pure formatter** builds the block from already-collected `ShellRow`s.
2. A **pure selector**, `default_shell_for`, mirrors `shell_for_owner`'s reuse
   predicate so `[DEFAULT SHELL]` names the shell a bare `run_terminal_command`
   would actually reuse.
3. The block is injected at the **three** existing context sites: both prompt
   builders and the `get_terminal_context` tool result.
4. **No count moves.** Still 48 tools: 34 core + 14 deferred; `doc_truth` 10.

### What this phase is not

This phase is the **first half** of charter row 08. The row read:

> 08 | shell-context — `get_terminal_context` → `get_shell_context { scope }`;
> `[HOSTS]`/`[SHELLS]`/`[DEFAULT SHELL]` blocks; approval panel names shell + host

Measurement (§ Pre-flight) splits it:

- **`[HOSTS]` cannot be built.** There is no host concept in the shell
  substrate — this is the **fifth** phase to measure that (04a, 06b, 07a, 07b
  were the first four). Deferred to M21/M23 with the rest of `host?`.
- **"approval panel names shell + host" is an IPC wire change.** The panel's
  `target_pane` hint rides `Response::ToolCallPrompt` (`src/ipc.rs:197`) and is
  rendered client-side; naming a shell there means widening that frame and the
  client renderer. 07a explicitly left the IPC wire alone for the same reason.
  Its own phase, after 09.
- **The tool rename `get_terminal_context` → `get_shell_context { scope }`** is
  the 11-step add-a-tool checklist plus a `scope` vocabulary change
  (`window`/`session`/`all` are *pane* words). That is **08b**, and it should
  land after 09 has removed the pane tools — renaming the context tool while
  six pane tools still populate the context would leave the tool's name
  describing something it no longer exclusively returns.

So: **08a is additive only.** It adds two lines to a context that keeps every
pane block it has today. 09 deletes the pane blocks; 08b renames the tool.
Nothing here is removed or renamed.

## Architecture references

- `docs/architecture.md` § 2.5 — the shell-addressed tool surface.
- The milestone README's charter table, row 08, and the Notes entry recording
  the four prior `host?` measurements.

## Pre-flight

Every fact below was measured against the tree at HEAD `e593a499` on
2026-09-16. Re-derive anything you intend to rely on.

### The three context sites, measured

```sh
$ grep -rn 'get_labeled_context' src/ --include=*.rs | grep -v 'cache.rs\|cache_tests.rs'
src/daemon/executor/mod.rs:859:            let ctx = cache.get_labeled_context_scoped(chat_pane, chat_pane, ctx_scope);
src/daemon/prompt.rs:108:        .get_labeled_context(ctx.client_pane, ctx.chat_pane);
src/daemon/prompt.rs:251:            .get_labeled_context(ctx.client_pane, ctx.chat_pane);
```

Three call sites, all of which this phase touches. `src/tmux/cache.rs` builds
the pane blocks themselves and **is not authorized** — the shell block does not
belong in the tmux cache, which has no access to the shell registry.

### No block exists today

```sh
$ grep -rc '\[SHELLS\]' src/ | grep -v ':0'          # (no output)
$ grep -rc '\[DEFAULT SHELL\]' src/ | grep -v ':0'   # (no output)
$ grep -rn 'shell_context' src/ | wc -l
0
```

### No host concept — the fifth measurement

```sh
$ grep -n 'pub struct ShellRecord' -A14 src/shell/registry.rs
74:pub struct ShellRecord {
75-    pub id: ShellId,
76-    pub owner: Owner,
77-    pub started_unix: u64,
82-    pub interactive: bool,
88-    pub dedicated: bool,
    …  exited_unix
```

`ShellRecord` carries **no host field**, and nothing in `src/shell/` models a
remote. `[HOSTS]` would have to render a constant. Do **not** emit it, do not
add a placeholder line, and do not add a `host` field — that is a 10-site
struct change the milestone README already costed and deferred.

### The reuse predicate `[DEFAULT SHELL]` must mirror

This is the load-bearing pre-injection of the phase. `shell_for_owner`
(`src/shell/session.rs:17`) chooses the shell a bare `run_terminal_command`
lands in. Its step (1), quoted verbatim from `src/shell/session.rs:27-56`:

```rust
    let owner_key = owner.key();
    let reuse: Option<ShellId> = with_shells(reg, |store| {
        let mut matches: Vec<ShellId> = store
            .values()
            .filter(|record| {
                // A shell whose terminal an interactive command took passes
                // the liveness probe but eats every byte written to it
                // (08b F2); the owner gets a fresh one instead. The record
                // stays and the host stays alive: later work addresses the
                // session by id. A dedicated shell is excluded for the same
                // reason in the other direction: it belongs to exactly one
                // command, so it is never offered to anything else.
                !record.interactive && !record.dedicated && record.owner.key() == owner_key
            })
            .map(|record| record.id.clone())
            .collect();
        matches.sort_by_key(|id| id.index());
        let lowest = matches.into_iter().next()?;
        if host_is_alive(&launch::lock_path(lowest.as_str())) {
            Some(lowest)
        } else {
            store.remove(lowest.as_str());
            None
        }
    });
```

Four things to carry across, and one deliberate divergence:

1. **Filter:** `!interactive && !dedicated && owner.key() == owner_key`.
2. **Order:** `sort_by_key(|id| id.index())` — the **numeric** index, not the
   id string. `s2` must be chosen over `s10`; comparing `"s10" < "s2"` as
   strings gets this backwards. `ShellIndex` (`src/daemon/executor/knowledge/shell.rs:28`)
   is the existing helper for exactly this and is what you must use.
3. **Lowest wins**, not first-seen — the store is a map and its iteration
   order is unspecified.
4. **The divergence:** `shell_for_owner` probes `host_is_alive`, a per-shell
   `flock` syscall. `default_shell_for` **must not** — it runs on every prompt
   assembly, and the prompt path must not do filesystem I/O per shell per turn.
   Use the `exited_unix.is_none()` stamp instead (written by
   `ShellRegistry::reap_exited`). State this in the doc comment: it is a
   documented approximation, not an oversight. A record whose host died but has
   not been reaped yet will be named one turn too long; a record that has been
   reaped must **never** be named.

### The collector seam to copy

`list_shells` already has exactly the shape this phase needs — a public
impure entry point that reads the process-wide registry, and a private
`_in` seam that tests drive with an injected registry and clock
(`src/daemon/executor/knowledge/shell.rs:51-67`):

```rust
pub fn list_shells(scope: Option<&str>, caller: &Owner) -> String {
    list_shells_in(crate::shell::registry(), scope, caller, now_unix())
}

fn list_shells_in(
    reg: &ShellRegistry,
    scope: Option<&str>,
    caller: &Owner,
    now_unix: u64,
) -> String {
    let rows = match resolve_scope(reg, scope, caller, "list_shells") {
        Ok(rows) => rows,
        Err(msg) => return msg,
    };
    format_shell_rows(&rows, scope, now_unix)
}
```

Build `shell_context_block` / `shell_context_block_in` the same way.
`resolve_scope(reg, None, caller, …)` already returns exactly the caller's own
rows — **reuse it**; do not write a second collector.

### Reentrancy — `with_shells` is not reentrant

From the M20 substrate facts: `with_shells` takes the registry lock and is
**non-reentrant**. `resolve_scope` already calls it and `collect_row` runs
inside. Your formatter and selector take `&[ShellRow]` and must not call
`with_shells`, `registry()`, `host_is_alive`, or any shell socket function.
Calling `with_shells` from inside a `with_shells` closure deadlocks the daemon
on every turn.

### The existing pieces you reuse, not rewrite

All in `src/daemon/executor/knowledge/shell.rs`:

| Item | Line | What it is |
|---|---|---|
| `ShellRow` | 17 | the collected row struct (`id`, `owner_key`, `started_unix`, `interactive`, `dedicated`, `exited_unix`) |
| `ShellIndex::new` | 30 | numeric sort key; `u32::MAX` for an unparseable id |
| `collect_row` | 110 | `ShellRecord` → `ShellRow`, runs under the lock |
| `format_age` | 167 | `4m`, `12s`, `2h3m` |
| `resolve_scope` | 74 | scope → `Vec<ShellRow>` |
| `now_unix` | 100 | wall clock |
| `row(...)` test helper | 1762 | hand-builds a `ShellRow` |
| `insert(...)` test helper | 1780 | inserts a live record into a `ShellRegistry` |
| `spawn_in_process_host` | 1802 | real in-process host, for the E2E |
| `lifecycle_home()` | 1833 | `TestHomeGuard` + temp `HOME` |

### No masking

`list_shells` does **not** call `mask_sensitive` — it emits only shell ids,
owner keys and flags, never command output. The context block emits the same
material. Do **not** add masking; do **not** put any command output, cwd, or
screen text in this block.

### The prompt injection slots

`build_subsequent_turn_prompt` already interpolates a prepended line in both
branches (`src/daemon/prompt.rs:255` and `:262`):

```rust
        format!(
            "{budget_note}{fg_target_line}{tool_restriction_block}{dynamic_memory}{situational}{pane_map}{current_time_line}\
             [Terminal snapshot — auto-refreshed (pane activity detected)]\n\
             ```\n{session_summary}\n```\n\nUser: {}",
            ctx.safe_query
        )
```

`{shell_block}` goes immediately after `{fg_target_line}` in **both** branches
— a shell exists whether or not a pane snapshot was injected.

`build_first_turn_prompt` ends (`src/daemon/prompt.rs:138-154`):

```rust
         {auto_search_block}\
         ## Terminal Session\n```\n{session_summary}\n```\n\n\
         {pane_map}{current_time_line}User: {}",
```

`{shell_block}` goes between `{auto_search_block}` and `## Terminal Session`.

`PromptCtx` carries `session_id: Option<&'a str>` (`src/daemon/prompt.rs:47`).
Build the owner as `Owner::Chat(ctx.session_id.unwrap_or_default().to_string())`
— the same `unwrap_or_default()` the executor already uses at
`src/daemon/executor/mod.rs:899`, `:929`, `:934`, `:940`, `:1029`.

### The `get_terminal_context` result site

`src/daemon/executor/mod.rs:848-870` already prepends a `[FOREGROUND TARGET]`
line to `ctx`. Prepend the shell block beside it, in the same expression. The
caller owner is `Owner::Chat(session_id.unwrap_or_default().to_string())`,
exactly as the sibling shell arms build it.

## Current state

- HEAD `e593a499`, tree clean.
- `cargo test --lib` → **1895 passed**, 4 ignored.
- `cargo test --lib 'daemon::executor::knowledge::shell'` → **51 passed**.
- `cargo test --test doc_truth` → **10 passed**.
- `src/daemon/prompt.rs` is 266 lines and has **no test module** — it is
  reached only through `tests/integration.rs`. This phase does **not** add one;
  its tests go in `knowledge/shell.rs` beside the code they cover, which is
  where the 51-test module and every helper already live.
- 48 tools: 34 core + 14 deferred. This phase adds none.

## Spec

Tasks 1–6 are the implementation. Tasks 7–8 are the mutation checks. Task 9 is
the end-to-end capture.

### Task 1 — `default_shell_for`

In `src/daemon/executor/knowledge/shell.rs`, add:

```rust
/// The shell a bare `run_terminal_command` would reuse for `owner_key`, or
/// `None` when the owner has no eligible shell.
///
/// Pure mirror of `shell_for_owner`'s step-(1) reuse predicate
/// (`src/shell/session.rs:27`): lowest **numeric** index among the owner's
/// shells that are neither `interactive` nor `dedicated`.
///
/// **Deliberate divergence:** `shell_for_owner` settles liveness with a
/// `host_is_alive` flock probe. This runs on every prompt assembly, so it
/// must do no I/O; it uses the `exited_unix` stamp `reap_exited` writes
/// instead. A host that died since the last reap is named one turn too long;
/// a reaped record is never named.
fn default_shell_for(rows: &[ShellRow], owner_key: &str) -> Option<String>
```

Filter on `!row.interactive && !row.dedicated && row.exited_unix.is_none() &&
row.owner_key == owner_key`; order with `ShellIndex::new(&row.id).0`; return
the lowest.

### Task 2 — `format_shell_context`

```rust
/// Format the ambient shell block. Pure: takes `now_unix` rather than reading
/// the clock and never sees the registry, so tests hand-build rows.
///
/// Returns the **empty string** when `rows` is empty — every session's first
/// turn has no shells, and a "no shells" line on every one of those turns is
/// pure token cost for no information.
fn format_shell_context(rows: &[ShellRow], owner_key: &str, now_unix: u64) -> String
```

Output shape, exactly:

```
[SHELLS] 2 — s0 age:4m, s2 age:12s [interactive]
[DEFAULT SHELL] s0 — run_terminal_command with no shell argument lands here
```

- Rows sorted by `ShellIndex`, comma-separated, each `s<N> age:<format_age>`.
- Flags appended per row, space-separated, in the order
  `[interactive] [dedicated] [exited]` — the same order and spelling
  `format_shell_rows` uses (`src/daemon/executor/knowledge/shell.rs:150-159`).
- The count is the number of rows.
- The `[DEFAULT SHELL]` line is **omitted entirely** when `default_shell_for`
  returns `None`. Do not emit `[DEFAULT SHELL] none` — the model must not be
  told a shell named `none` exists.
- The block ends with a single trailing `\n`.
- An `age:` part is omitted for a row whose `started_unix` is `0` or in the
  future, matching `format_shell_rows`' guard at line 145.

### Task 3 — `shell_context_block_in` (the test seam)

```rust
fn shell_context_block_in(reg: &ShellRegistry, caller: &Owner, now_unix: u64) -> String {
    let key = caller.key();
    let rows = match resolve_scope(reg, None, caller, "shell_context") {
        Ok(rows) => rows,
        Err(_) => return String::new(),
    };
    format_shell_context(&rows, &key, now_unix)
}
```

`resolve_scope` with `None` cannot return `Err` — the `Err` arm exists only
because the signature admits it, and returning an empty block is the right
answer if that ever changes. Do **not** `unwrap()` it.

### Task 4 — `shell_context_block` (the public entry)

```rust
/// The ambient `[SHELLS]` / `[DEFAULT SHELL]` block for `caller`, or the
/// empty string when the caller owns no shells.
pub fn shell_context_block(caller: &Owner) -> String {
    shell_context_block_in(crate::shell::registry(), caller, now_unix())
}
```

Re-export it from `src/daemon/executor/knowledge/mod.rs` beside `list_shells`
(line 24). It must be `pub`, not `pub(super)` — `pub(super)` cannot be
widened on re-export, which is why `list_shells` is already `pub` and says so
in its doc comment.

### Task 5 — inject at the two prompt sites

In `src/daemon/prompt.rs`:

- In `build_first_turn_prompt`, compute
  ```rust
  let shell_block = crate::daemon::executor::knowledge::shell_context_block(
      &crate::shell::Owner::Chat(ctx.session_id.unwrap_or_default().to_string()),
  );
  ```
  and interpolate `{shell_block}` between `{auto_search_block}` and
  `## Terminal Session`.
- In `build_subsequent_turn_prompt`, compute the same binding and interpolate
  `{shell_block}` immediately after `{fg_target_line}` in **both** format
  branches.

Use the crate's existing import style in that file; do not add a `use` that
clippy will flag as unused in one configuration.

### Task 6 — inject at the `get_terminal_context` result site

In `src/daemon/executor/mod.rs`, in the `PendingCall::GetTerminalContext` arm
(line 848), prepend the block alongside the existing `fg_line`, using
`&Owner::Chat(session_id.unwrap_or_default().to_string())` as the caller.

### Task 7 — mutation check A (the sort key)

Use the `patch` tool. In `default_shell_for`, replace the numeric ordering
with a string ordering — i.e. sort the candidate rows by `row.id` compared as
`&str` instead of by `ShellIndex::new(&row.id).0`.

Run `cargo test --lib 'knowledge::shell'` and record the output. It **must**
fail, and `default_shell_for_picks_the_lowest_index_not_the_lowest_string`
must be among the failures. Revert with the `patch` tool and confirm green.

Paste both `test result:` lines into the Update Log.

### Task 8 — mutation check B (the interactive exclusion)

Use the `patch` tool. In `default_shell_for`, drop the `!row.interactive`
conjunct from the filter.

Run `cargo test --lib 'knowledge::shell'`. It **must** fail with
`default_shell_for_skips_an_interactive_shell` among the failures. Revert and
confirm green. Paste both `test result:` lines into the Update Log.

### Task 9 — end-to-end capture

Run a real in-process host through the block and paste the captured output
verbatim into the Update Log. Add this test to the module:

```rust
    #[tokio::test]
    async fn e2e_shell_context_block_names_a_real_hosts_shell() {
        let (_home, _dir) = lifecycle_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("e2e".into());
        let out = open_shell_with(
            &reg,
            &owner,
            5,
            Duration::from_millis(3000),
            spawn_in_process_host,
        )
        .await;
        assert!(out.contains("Opened shell"), "setup must succeed: {out}");
        let block = shell_context_block_in(&reg, &owner, NOW);
        println!("PASTE MATCH >>>\n{block}<<<");
        assert!(
            block.starts_with("[SHELLS] 1 — s"),
            "the block must count and name the real shell: {block}"
        );
        assert!(
            block.contains("[DEFAULT SHELL] s"),
            "a freshly opened non-dedicated shell is the default: {block}"
        );
    }
```

Run it with `cargo test --lib e2e_shell_context_block -- --nocapture`. Copy the
text **between** `PASTE MATCH >>>` and `<<<` into the Update Log's E2E block
**byte for byte** — do not retype it, do not reflow it, do not summarize it.
This test counts as one of the ten in the test plan.

## Acceptance criteria

Each was run against HEAD `e593a499` and **fails** there; the "now" column is
the measured current value.

- [ ] `grep -cE '^fn default_shell_for|^fn format_shell_context|^pub fn shell_context_block' src/daemon/executor/knowledge/shell.rs` → **3** (now 0)
- [ ] `grep -c 'shell_context_block' src/daemon/prompt.rs` → **2** (now 0)
- [ ] `grep -c 'shell_context_block' src/daemon/executor/mod.rs` → **1** (now 0)
- [ ] `grep -c 'shell_context_block' src/daemon/executor/knowledge/mod.rs` → **1** (now 0)
- [ ] `cargo test --lib 'daemon::executor::knowledge::shell'` reports **61 passed** (now 51 — the ten tests of the test plan)
- [ ] `cargo test --lib` reports **1905 passed** (now 1895)
- [ ] `grep -c 'host_is_alive' src/daemon/executor/knowledge/shell.rs` → **unchanged at 9** (measured now: 9). A rising count means the prompt path grew a per-shell flock probe.
- [ ] `grep -rc '\[HOSTS\]' src/` produces no non-zero line — `[HOSTS]` is **not** built by this phase.
- [ ] `grep -c 'get_shell_context' src/ai/tools/defs.rs` → **0** — the tool rename is 08b, not this phase.
- [ ] `grep -c 'mask_sensitive' src/daemon/prompt.rs` → **unchanged at 0** (measured now: 0)
- [ ] `cargo test --test doc_truth` reports **10 passed** — the tool table is untouched.
- [ ] `cargo fmt --all` clean, `cargo build` clean, `cargo clippy --all-targets --all-features -- -D warnings` clean.
- [ ] `src/tmux/cache.rs` is unmodified: `git diff --stat e593a499..HEAD -- src/tmux/cache.rs` is empty.

## Test plan

Ten tests, all in `src/daemon/executor/knowledge/shell.rs`'s existing `tests`
module, using the existing `row(...)`, `insert(...)`, `lifecycle_home()`,
`spawn_in_process_host` helpers and the `NOW` constant.

**`default_shell_for` — six**

1. `default_shell_for_picks_the_lowest_index_not_the_lowest_string` — rows
   `s2` and `s10`, both eligible, same owner → `Some("s2")`. This is the test
   mutation A must redden, so `s10` is not optional: with only `s0`/`s1` a
   string sort gives the same answer and the mutant stays green.
2. `default_shell_for_skips_an_interactive_shell` — `s0` interactive, `s1`
   plain → `Some("s1")`. Mutation B's test.
3. `default_shell_for_skips_a_dedicated_shell` — `s0` dedicated, `s1` plain →
   `Some("s1")`.
4. `default_shell_for_skips_an_exited_shell` — `s0` with
   `exited_unix: Some(NOW)`, `s1` plain → `Some("s1")`.
5. `default_shell_for_ignores_another_owners_shell` — `s0` owned by
   `chat:other`, `s1` owned by the caller → `Some("s1")`.
6. `default_shell_for_returns_none_when_every_shell_is_ineligible` — a single
   interactive `s0` → `None`.

**`format_shell_context` — three**

7. `format_shell_context_is_empty_when_there_are_no_rows` — `assert_eq!(out, "")`,
   not `is_empty()` on a trimmed string: the must-NOT here is any whitespace or
   header line at all.
8. `format_shell_context_lists_every_shell_with_its_flags` — two rows, one of
   them interactive → the line starts `[SHELLS] 2 — `, names both ids in
   numeric order, and carries `[interactive]` on the right one.
9. `format_shell_context_omits_the_default_line_when_none_is_eligible` — one
   interactive row → output contains `[SHELLS] 1` and does **not** contain
   `[DEFAULT SHELL]`. Assert the absence explicitly.

**Seam — one**

10. `e2e_shell_context_block_names_a_real_hosts_shell` — Task 9.

Test 8 covers the `[DEFAULT SHELL]` positive case (its first row is eligible),
so there is no separate eleventh test for it; if you find you need one, the
count in the acceptance criteria moves to 62/1906 and you must say so in the
Update Log rather than silently exceeding it.

## End-to-end verification

The captured block from Task 9, pasted verbatim into the Update Log under a
`### E2E capture` heading, together with the two mutation-check result pairs
from Tasks 7 and 8.

A run that reports "the E2E test passes" without the pasted block has not done
Task 9.

## Authorizations

Edits are authorized in exactly these files:

- `src/daemon/executor/knowledge/shell.rs` — Tasks 1–4, 7–9, test plan.
- `src/daemon/executor/knowledge/mod.rs` — the re-export, Task 4.
- `src/daemon/prompt.rs` — Task 5.
- `src/daemon/executor/mod.rs` — Task 6.
- This phase doc's Update Log.

**Not authorized:** `src/tmux/cache.rs`, `src/tmux/cache_tests.rs`,
`src/shell/registry.rs`, `src/shell/session.rs`, `src/ai/tools/defs.rs`,
`src/ipc.rs`, `CLAUDE.md`, `README.md`, `assets/prompts/sre.toml`,
`src/config/seeds.rs`, `tests/doc_truth.rs`.

If you believe an unauthorized file must change, stop and say so in the Update
Log rather than editing it.

## Out of scope

- `[HOSTS]` and any `host` field on `ShellRecord` — no host concept exists
  (fifth measurement); deferred with `host?` to M21/M23.
- Naming the shell or host in the approval panel — an IPC wire + client
  renderer change; its own phase after 09.
- Renaming `get_terminal_context` to `get_shell_context`, and its `scope`
  vocabulary — **08b**, after 09.
- Removing or shrinking any pane block — **09**.
- Rewriting `sre.toml` to describe the new block — **10**.
- Adding a test module to `src/daemon/prompt.rs`.

## Update Log

<!-- entries appended below this line -->

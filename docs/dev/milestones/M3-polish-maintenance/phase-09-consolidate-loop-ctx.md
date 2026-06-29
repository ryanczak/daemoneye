# Phase 09: Consolidate the two high-arity orchestration signatures

**Milestone:** M3 — Polish & Maintenance
**Status:** todo
**Depends on:** none
**Estimated diff:** ~90 lines (three param structs + two signature rewrites +
two call-site rewrites; deletes two `#[allow]` + two `TODO(M2)` lines).
**Tags:** language=rust, kind=refactor, size=s

## Goal

Resolve the final two `TODO(M2): consolidate params into a struct` markers — the
two orchestration functions that still carry `#[allow(clippy::too_many_arguments)]`
because phase-05 only covered the low-blast leaf functions. Group their plain-data
parameters into borrow-structs (the same idiom phase-05 established), drop both
suppressions, and close the milestone's "7 `TODO(M2)` markers resolved" exit
criterion. Behavior-preserving — this is a pure signature refactor.

## Architecture references

Read before starting:

- `docs/architecture.md#21-interactive-requestresponse` — the request/response
  lifecycle these two functions implement (`handle_ask` is the Ask orchestrator;
  `run_conversation_loop` is the inner AI loop). Confirm no protocol change is
  needed — this is an internal signature refactor only, no wire/IPC change.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes
   (`git status` is clean; phase-08 is already committed and approved).

## Current state

Two functions still carry the suppression + marker (grep-verified — these are the
**only** two `TODO(M2)` markers left in `src/`):

```
src/daemon/server/ask.rs:19:#[allow(clippy::too_many_arguments)]
src/daemon/server/ask.rs:20:// TODO(M2): consolidate params into a struct
src/daemon/stream.rs:43:// TODO(M2): consolidate params into a struct
src/daemon/stream.rs:44:#[allow(clippy::too_many_arguments)]
```

### `handle_ask` — `src/daemon/server/ask.rs:21`

```rust
#[allow(clippy::too_many_arguments)]
// TODO(M2): consolidate params into a struct
pub(super) async fn handle_ask<W, R>(
    initial_query: String,
    client_pane: Option<String>,
    session_id: Option<String>,
    chat_pane: Option<String>,
    prompt_override: Option<String>,
    chat_width: Option<usize>,
    client_tmux_session: Option<String>,
    client_target_pane: Option<String>,
    tx: &mut W,
    rx: &mut R,
    cache: Arc<SessionCache>,
    sessions: &SessionStore,
    schedule_store: Arc<ScheduleStore>,
    bg_session: Arc<std::sync::Mutex<String>>,
    config: &Config,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
    R: AsyncBufRead + Unpin,
{
```

The body destructively consumes most of these params throughout its ~565 lines.
Its **only** caller is the `Request::Ask { .. }` arm in
`src/daemon/server/mod.rs:223`.

### `run_conversation_loop` — `src/daemon/stream.rs:45`

```rust
// TODO(M2): consolidate params into a struct
#[allow(clippy::too_many_arguments)]
pub async fn run_conversation_loop<W, R>(
    tx: &mut W,
    rx: &mut R,
    session_id: Option<String>,
    session_name: &str,
    chat_pane: Option<String>,
    mut messages: Vec<Message>,
    sys_prompt: String,
    session_active_model: Option<String>,
    is_ghost_session: bool,
    this_turn_count: usize,
    post_trim_len: usize,
    needs_compaction: bool,
    config: &Config,
    cache: Arc<SessionCache>,
    sessions: SessionStore,
    schedule_store: Arc<ScheduleStore>,
    cost_attribution: CostAttribution,
) -> Result<()>
```

Its **only** caller is `handle_ask` itself, at `src/daemon/server/ask.rs:565`
(`stream::run_conversation_loop(tx, rx, session_id, &session_name, chat_pane,
messages, sys_prompt, session_active_model, is_ghost_session, this_turn_count,
post_trim_len, needs_compaction, config, cache, Arc::clone(sessions),
schedule_store, cost_attribution)`).

Note `mut messages` — the loop mutates the messages vec internally. Preserve that
mutability after the field is destructured out of the ctx struct.

Relevant type aliases (already in the tree, no change needed):
- `pub type SessionStore = Arc<Mutex<HashMap<String, SessionEntry>>>;`
  (`src/daemon/session.rs:104`) — cheap to clone/move by value.
- `CostAttribution` (`src/cost.rs:50`) — owned value.

## The idiom to follow (worked example)

Phase-05 established the borrow-struct idiom for exactly this. Mirror it. From
`src/daemon/executor/file_ops/ops.rs:9`:

```rust
pub(super) struct RunEditArgs<'a> {
    pub id: &'a str,
    pub path: &'a str,
    pub old_string: &'a str,
    pub new_string: &'a str,
    pub target_pane: Option<&'a str>,
}

pub(super) async fn run_edit<W, R>(
    args: RunEditArgs<'_>,
    session_id: Option<&str>,
    tx: &mut W,
    rx: &mut R,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    let RunEditArgs {
        id,
        path,
        old_string,
        new_string,
        target_pane,
    } = args;
    // ...rest of body unchanged, uses the destructured names directly...
```

Three load-bearing properties of this idiom, all of which this phase keeps:

1. **`tx` / `rx` stay as their own generic parameters** — they are not moved into
   the struct (the `W`/`R` generics and `&mut` borrows do not belong in a data
   bag). After consolidation each function takes only `(struct…, tx, rx)`.
2. **Each struct field keeps the *same type* as the parameter it replaces** —
   owned stays owned (`String`, `Vec<Message>`, `Arc<…>`, `bool`, `usize`,
   `CostAttribution`); a borrow stays a borrow with a lifetime (`&'a Config`,
   `&'a str`, `&'a SessionStore`). Do not change ownership of any field.
3. **Destructure at the top of the body** with `let StructName { .. } = arg;` so
   the rest of the (long) function body is untouched and keeps using the bare
   names. For the `mut messages` field, bind it `mut` in the destructure
   (`let ConversationLoopCtx { mut messages, .. } = ctx;` or rebind) so the
   existing in-loop mutation still compiles.

## Spec

### 1. Add `AskRequest` + `AskContext` and rewrite `handle_ask`

In `src/daemon/server/ask.rs`, add two **module-level** structs above `handle_ask`
and split the data params between them. `handle_ask` is `pub(super)`, so both
structs are `pub(super)` (the caller `mod.rs` is the parent `daemon::server`
module and must construct them).

- **`AskRequest`** — the per-request fields derived from the `Request::Ask` IPC
  message (all owned):
  - `query: String` (was `initial_query`)
  - `client_pane: Option<String>`
  - `session_id: Option<String>`
  - `chat_pane: Option<String>`
  - `prompt_override: Option<String>`
  - `chat_width: Option<usize>`
  - `client_tmux_session: Option<String>`
  - `client_target_pane: Option<String>`

- **`AskContext<'a>`** — the daemon-state handles:
  - `cache: Arc<SessionCache>`
  - `sessions: &'a SessionStore`
  - `schedule_store: Arc<ScheduleStore>`
  - `bg_session: Arc<std::sync::Mutex<String>>`
  - `config: &'a Config`

Rewrite the signature to:

```rust
pub(super) async fn handle_ask<W, R>(
    req: AskRequest,
    ctx: AskContext<'_>,
    tx: &mut W,
    rx: &mut R,
) -> Result<()>
```

Delete the `#[allow(clippy::too_many_arguments)]` and the `// TODO(M2)` line.
Destructure both structs at the top of the body (`let AskRequest { .. } = req;`
`let AskContext { .. } = ctx;`) so the rest of the body — including the
`stream::run_conversation_loop(...)` call — keeps using the bare names. If the
field name differs from the old param name (`query` vs `initial_query`), rebind
in the destructure (`let AskRequest { query: initial_query, .. } = req;`) or
rename uses; the executor's call.

### 2. Update the `handle_ask` call site in `mod.rs`

In `src/daemon/server/mod.rs`, the `Request::Ask { .. }` arm (around line 212)
currently spreads 15 args into `handle_ask(...)`. Rewrite it to construct
`AskRequest { .. }` and `AskContext { .. }` from the same bindings and call
`handle_ask(req, ctx, &mut tx, &mut rx).await?`. The `Request::Ask` destructure
that produces `query`, `tmux_pane`, `session_id`, etc. is unchanged; only the
call shape changes. (`tmux_pane` → `AskRequest.client_pane`; `prompt` →
`prompt_override`; `tmux_session` → `client_tmux_session`; `target_pane` →
`client_target_pane`. Match the current positional mapping exactly — see
`mod.rs:223-240`.)

### 3. Add `ConversationLoopCtx` and rewrite `run_conversation_loop`

In `src/daemon/stream.rs`, add a **module-level** struct above
`run_conversation_loop`. The function is `pub`, so make the struct `pub` to match
(it appears in the public signature). Fields, each keeping the existing param's
type:

- `session_id: Option<String>`
- `session_name: &'a str`
- `chat_pane: Option<String>`
- `messages: Vec<Message>`
- `sys_prompt: String`
- `session_active_model: Option<String>`
- `is_ghost_session: bool`
- `this_turn_count: usize`
- `post_trim_len: usize`
- `needs_compaction: bool`
- `config: &'a Config`
- `cache: Arc<SessionCache>`
- `sessions: SessionStore`
- `schedule_store: Arc<ScheduleStore>`
- `cost_attribution: CostAttribution`

Rewrite the signature to:

```rust
pub async fn run_conversation_loop<W, R>(
    ctx: ConversationLoopCtx<'_>,
    tx: &mut W,
    rx: &mut R,
) -> Result<()>
```

Delete the `// TODO(M2)` line and the `#[allow(clippy::too_many_arguments)]`.
Destructure at the top of the body, binding `messages` as `mut`, so the existing
loop body (which mutates `messages` and reads all the other names) is untouched.

### 4. Update the `run_conversation_loop` call site in `ask.rs`

At `src/daemon/server/ask.rs:565`, replace the positional
`stream::run_conversation_loop(tx, rx, session_id, &session_name, …)` call with
construction of `ConversationLoopCtx { .. }` from the same local bindings, then
`stream::run_conversation_loop(ctx, tx, rx).await`. The `Arc::clone(sessions)`
that currently feeds the `sessions` arg becomes `sessions: Arc::clone(sessions)`
in the struct literal; `session_name: &session_name`; `config`; etc. — same
values, struct-literal shape.

## Acceptance criteria

- [ ] `AskRequest` and `AskContext<'a>` exist as `pub(super)` structs in
      `src/daemon/server/ask.rs`; `handle_ask` takes `(req: AskRequest, ctx:
      AskContext<'_>, tx, rx)`.
- [ ] `ConversationLoopCtx<'a>` exists as a `pub` struct in
      `src/daemon/stream.rs`; `run_conversation_loop` takes `(ctx:
      ConversationLoopCtx<'_>, tx, rx)`.
- [ ] `grep -rn "TODO(M2)" src/` returns **zero** matches.
- [ ] `grep -rn "too_many_arguments" src/daemon/server/ask.rs src/daemon/stream.rs`
      returns **zero** matches (both suppressions deleted, not relocated).
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes **with the
      two suppressions removed** — proving the consolidation actually dropped each
      function under clippy's argument-count threshold (this is the real check; a
      relocated `#[allow]` would defeat the phase).
- [ ] `cargo fmt --all` leaves the tree clean.
- [ ] `cargo test` passes (existing suite, unchanged count).

## Test plan

No new tests. Both new structs are pure plumbing — they only carry fields between
one caller and one callee — which STANDARDS §3.2 explicitly exempts from unit-test
coverage ("a function that only constructs a struct from its fields or forwards
args"). The refactor is behavior-preserving; the existing unit + integration suite
is the regression guard, and the clippy gate (with the `#[allow]`s removed) is the
proof the consolidation achieved its purpose.

Do **not** invent tests that assert struct field values — that tests the language,
not the code.

## End-to-end verification

> Not applicable — phase ships no runtime-loadable artifact. It is a pure internal
> signature refactor of two existing functions and their two call sites; no new
> CLI behavior, config, wire field, or file format. Verification is the
> clippy-without-`#[allow]` gate plus the unchanged-green test suite, both quoted
> in the completion Update Log.

## Authorizations

None. (No new dependencies. `docs/architecture.md` is not modified — confirm at
Pre-flight that no protocol/IPC change is implied; there is none. No STANDARDS §5
files touched.)

## Out of scope

- **The two `#[allow(clippy::too_many_arguments)]` in
  `src/cli/commands/stream.rs:998` and `:1069`.** They are **not** `TODO(M2)`
  markers (grep-confirmed: only `ask.rs` and `daemon/stream.rs` carry the marker)
  and are a different file/subsystem. Leave them alone — they are not part of this
  phase or the milestone's "7 markers" count.
- **No behavior change.** Do not "improve" the logic inside either function while
  refactoring the signature — move params into structs and nothing else.
- **No field reordering for its own sake / no renaming of the long function
  bodies' local variables** beyond what the destructure rebind requires.
- **No splitting either function into smaller functions** — that is a separate
  refactor not in this phase.
- **Other M3 phases** (10 knowledge-tests) — leave them alone.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

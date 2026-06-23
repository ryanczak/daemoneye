# Phase 11: On-Demand Tool Loading

**Milestone:** M1 — Agent Tooling Improvements
**Status:** todo
**Depends on:** none for correctness. If phase-08 lands first there is **no
conflict** — 08 edits `build_properties` (param-level enum) while this phase edits
`get_*_tool_definition` (tool-level selection); different functions in the same
file. Sequence 08 → 11 to keep diffs clean.
**Estimated diff:** ~300 lines (incl. tests)
**Tags:** language=rust, kind=feature, size=l

> **Scope note (architect, 2026-06-22).** This phase replaces the
> "document every tool in `sre.toml`" approach with **true on-demand tool
> loading**, modeled on the deferred-tool / tool-search pattern. The original
> phase-08 plan (re-add the 9 absent tools to the prompt + a regression test that
> forces *every* `TOOLS` entry to be documented) is the **opposite** of what we
> want: the real context cost is the **tool JSON schemas**, which all three
> backends send **unconditionally on every request** (`body["tools"] =
> get_tool_definition()` → `render_anthropic(TOOLS)`), regardless of what the
> prompt prose says. Removing the 9 tools' prose from `sre.toml` saved almost
> nothing while their schemas keep shipping every turn. This phase makes a curated
> set of rarely-used tools **deferred**: their schemas are *omitted* from the
> default render and loaded only when the model calls a new `load_tools` tool.
> The design is **data-driven and self-declaring** so adding a future tool is a
> one-liner, and **unload-ready** so a later `unload_tools` slots in without
> rework.

## Goal

Stop paying the per-request schema cost of tools the agent rarely needs, without
making them undiscoverable:

1. **Defer the schemas, not just the prose.** Split `TOOLS` into an always-loaded
   **core** set and a **deferred** set. The default render emits core only. This
   is the actual context win.
2. **Keep deferred tools discoverable and loadable.** A new core meta-tool
   `load_tools` lets the model pull a deferred *group* into the active set; its
   schemas appear on the next turn and the model calls them. The catalog of what
   can be loaded is **generated from the deferred set itself** and surfaced in
   `load_tools`'s own description — so the model always sees what exists, and the
   prompt needs no per-tool prose.
3. **Make adding a tool trivial.** The core/deferred split is one self-declaring
   field on `ToolDef` (`deferred_group: Option<&'static str>`). The Rust compiler
   forces every tool literal to set it, so the split can never be silently
   incomplete. Adding a future deferred tool is: write the `ToolDef`, set
   `deferred_group: Some("...")`, done — it is auto-excluded from the default
   render, auto-listed in the `load_tools` catalog, and auto-loadable. **No
   `sre.toml` edit, no index table, no test edit.**
4. **Be unload-ready.** Session state is a `HashSet<String>` of loaded deferred
   tool names; `load_tools` inserts. A future `unload_tools` removes from the same
   set, mirroring the arg shape — purely additive. (Not built now, by decision.)

The **deferred set** for this phase is exactly the nine tools the principal
engineer already pulled from the prompt, grouped by domain:

| `deferred_group` | tools |
|---|---|
| `"agents"` | `create_agent`, `read_agent`, `list_agents`, `delete_agent` |
| `"scripts"` | `read_script`, `list_scripts` |
| `"runbooks"` | `read_runbook`, `list_runbooks` |
| `"memory"` | `delete_memory` |

Everything that writes, executes, schedules, or is otherwise hot stays **core**
(`deferred_group: None`). Group names are tunable — they are the values the model
passes to `load_tools(groups=[...])`; keep them lowercase, one word, intuitive.

## Architecture references

Read before starting:

- `docs/architecture.md#13-ai-provider-layer` — the `TOOLS` slice and the three
  provider renderers. This phase changes how the renderers *select* which tools to
  emit and adds one tool.
- `docs/architecture.md#3-the-ghost-shell-subsystem` — ghost sessions run their own
  chat loop (`trigger_ghost_turn`); it must pass the same per-session loaded set so
  ghosts can `load_tools` too (the mechanism is uniform across interactive and
  ghost loops).

## Pre-flight

1. Read `docs/dev/STANDARDS.md` (note §2.2 "no premature abstraction"; §2.6 no new
   dependencies; §3 hermetic/deterministic tests; §5 — `assets/prompts/sre.toml`
   is the **source** prompt artifact this phase may edit, distinct from the
   runtime-blocked `etc/prompts/sre.toml` deployed copy).
2. Read `docs/dev/WORKFLOW.md` § "Update Log entries".
3. Read this entire phase doc before touching code.
4. Read the "Adding a new AI tool (checklist)" in `CLAUDE.md` — `load_tools` is a
   new silent (non-approval-gated) tool and must follow every step.
5. Confirm the repo is on a clean branch with no uncommitted changes.
6. Re-verify the deferred-set membership against the current `TOOLS` slice — the
   nine names above must all still exist as `ToolDef` entries:
   ```
   grep -nP 'name:\s*"(create_agent|read_agent|list_agents|delete_agent|read_script|list_scripts|read_runbook|list_runbooks|delete_memory)"' src/ai/tools.rs
   ```

## Current state

### Tools are sent in full, every request — `src/ai/{backends,tools.rs}`

```rust
// src/ai/tools.rs
pub fn get_tool_definition() -> Value { render_anthropic(TOOLS) }       // Anthropic
pub fn get_openai_tool_definition() -> Value { render_openai(TOOLS) }   // OpenAI
pub fn get_gemini_tool_definition() -> Value { render_gemini(TOOLS) }   // Gemini
```

```rust
// src/ai/backends/anthropic.rs:145   (mirror: openai.rs:120, gemini.rs:185)
body["tools"] = json!(get_tool_definition());
```

The full `TOOLS` slice is rendered unconditionally. `ToolPolicy`/`GhostPolicy`
filter only at **execution** time (`permits()`), never at render time — so today
there is no per-request tool selection anywhere.

### `ToolDef` has three fields — `src/ai/tools.rs:41`

```rust
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub params: &'static [ParamDef],
}
```

There are 31 `ToolDef` literals in the `TOOLS` slice (line ~47; `load_tools`
makes 32 once added). Adding a field forces all of them to set it — a compile
error until they do, which is the point: the split cannot be silently
incomplete. (Don't hand-count; the compiler is the authority — set the field on
every literal the build flags.)

### The chat trait carries no tool set — `src/ai/mod.rs:110`

```rust
async fn chat(
    &self,
    system_prompt: &str,
    messages: Vec<Message>,
    tx: UnboundedSender<AiEvent>,
    use_tools: bool,
) -> Result<()>;
```

Each backend internally calls `get_*_tool_definition()`. To select tools per
session the trait gains one parameter (the session's loaded deferred set).

### The interactive loop re-reads session state each iteration — `src/daemon/stream.rs:67`

The outer `loop` (line 67) rebuilds the client and calls `client.chat(...)` once
per iteration (line 80-87, inside a `tokio::spawn` with cloned `sys_prompt`/
`messages`). Because it re-reads from `sessions` each pass, a mid-conversation
mutation of the session's loaded set is picked up on the **next** `chat()` — no
extra plumbing needed beyond reading the set at the top of the loop and threading
it into `chat()`.

### Tools mutate `SessionEntry` via the `sessions` lock — `src/daemon/executor/mod.rs:78`

`build_memory_namespaces` (line 78) already reads `SessionEntry` through the
`sessions` lock inside the executor. The `load_tools` arm uses the same pattern to
**write** `entry.loaded_tools`. No new `ToolCallOutcome` variant is required — the
loop re-reads on the next iteration. (`ToolCallOutcome` is at line 41.)

### Ghost loop has its own chat call — `src/daemon/ghost.rs` (`trigger_ghost_turn`)

It also calls `client.chat(...)`. It must pass the ghost session's `loaded_tools`
(same field) so the change is uniform. Ghosts get an empty set by default and may
`load_tools` like any session.

## Spec

Numbered tasks in execution order. **Build after Task 2** (it changes the `chat`
trait signature used across the crate).

### 1. Add the self-declaring `deferred_group` field — `src/ai/tools.rs`

**1a.** Add the field to `ToolDef`:

```rust
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub params: &'static [ParamDef],
    /// `None` = core (always rendered, prose-documented in sre.toml).
    /// `Some(group)` = deferred: omitted from the default render, loaded on demand
    /// via `load_tools`, and listed in that tool's generated catalog under `group`.
    pub deferred_group: Option<&'static str>,
}
```

**1b.** Set `deferred_group: None` on every existing core literal, and
`Some("...")` on the nine deferred ones per the Goal table. The compiler will not
build until all 32 are set — that is the completeness guarantee. Keep the field
last in each literal for a uniform diff.

**1c.** Add the new `load_tools` `ToolDef` to `TOOLS` (`deferred_group: None` — it
is core). Place it near the other meta/utility tools (e.g. next to
`get_terminal_context`). Spec:

- `name: "load_tools"`
- `description`: a concise base sentence — e.g. *"Load an additional group of
  tools into your available tool set for the rest of this session. Some
  rarely-used tools are not loaded by default to save context; call this to enable
  a group, then call the tools it contains. Pass `groups` as an array of group
  names."* The **catalog** of groups and their member tools is appended at render
  time (Task 3), not hardcoded here.
- one param: `groups`, `ParamTy::Str`, `required: true`, description naming the
  dual-format accepted shape (a real array `["agents"]` or a JSON-encoded string
  `"[\"agents\"]"`), mirroring the `tags` idiom.

### 2. Thread the loaded set through `chat` and parameterize the renderers

**2a.** `src/ai/mod.rs` — add a parameter to the `AiClient::chat` trait:

```rust
async fn chat(
    &self,
    system_prompt: &str,
    messages: Vec<Message>,
    tx: UnboundedSender<AiEvent>,
    use_tools: bool,
    loaded_tools: Vec<String>,   // deferred tool names active for this session
) -> Result<()>;
```

`loaded_tools` is owned so it can move into the per-turn `tokio::spawn`.

**2b.** `src/ai/tools.rs` — make selection happen in the public getters; keep
`render_anthropic`/`render_openai`/`render_gemini` as pure "render this list" fns.
Change their signature from `&[ToolDef]` to `&[&ToolDef]` (a borrowed selection)
and have the getters build the filtered list:

```rust
fn select_tools(loaded: &[String]) -> Vec<&'static ToolDef> {
    TOOLS
        .iter()
        .filter(|t| t.deferred_group.is_none() || loaded.iter().any(|n| n == t.name))
        .collect()
}

pub fn get_tool_definition(loaded: &[String]) -> Value {
    render_anthropic(&select_tools(loaded))
}
// …mirror for openai / gemini…
```

Update the existing render tests that call `render_gemini(TOOLS)` etc. to pass a
borrowed selection (`&TOOLS.iter().collect::<Vec<_>>()`), or call the getters.
**Do not** change what a full render produces for an all-loaded set — the
`render_gemini` count/order invariants (tools.rs:1557+) must still hold when every
tool is selected.

**2c.** `src/ai/backends/{anthropic,openai,gemini}.rs` — the three `chat` impls
take `loaded_tools` and pass it to the getter:

```rust
body["tools"] = json!(get_tool_definition(&loaded_tools));   // anthropic.rs:145
```

(OpenAI 120, Gemini 185 mirror this. When `use_tools == false`, behavior is
unchanged — tools are still omitted/`NONE`.)

### 3. Generate the `load_tools` catalog at render time — `src/ai/tools.rs`

So the model always sees the current deferred catalog and adding a tool needs no
prompt edit, build the catalog from the deferred entries and append it to
`load_tools`'s description **inside the renderers** (the one place that turns
`ToolDef.description` into an owned JSON string):

```rust
/// Lines describing each deferred group and its members, e.g.
/// "  - agents: create_agent, read_agent, list_agents, delete_agent".
fn deferred_catalog_text() -> String { /* group TOOLS by deferred_group.is_some() */ }
```

In each renderer, when emitting the tool named `"load_tools"`, use
`format!("{}\n\nAvailable groups:\n{}", def.description, deferred_catalog_text())`
as the description; all other tools use `def.description` verbatim. Keep the
grouping deterministic (stable order — e.g. first-appearance order in `TOOLS`, or
sorted) so the catalog text is stable for tests.

### 4. Add `load_tools` to the type/event/dispatch machinery — per CLAUDE.md checklist

- `src/ai/types.rs`: `PendingCall::LoadTools { id, groups: Vec<String>, thought_signature: Option<String> }`
  + arms in `to_tool_call()` / `id()` / `tool_name()` / `summary()` (e.g.
  `"load_tools: agents, scripts"`); `should_emit_tool_feedback()` returns **true**
  (silent tool, no approval UI). Add `AiEvent::LoadTools { id, groups, thought_signature }`.
- `src/ai/tools.rs`: a `LoadToolsArgs` struct + `impl ToolArgs` that parses
  `groups` via the existing `extract_string_vec` dual-format helper; add the
  dispatch arm in `dispatch_tool_event()`.
- `src/daemon/stream.rs`: an `AiEvent::LoadTools` arm in **both** the interactive
  streaming match and (if it has its own match) the ghost path — push
  `PendingCall::LoadTools`.

### 5. Execute `load_tools`: mutate session state — `src/daemon/executor/mod.rs`

Add the `PendingCall::LoadTools { groups, .. }` arm to `execute_tool_call`:

1. Resolve each requested group name to its member tool names via a pure helper
   over `TOOLS` (`tools_in_group(group) -> Vec<&'static str>`; unknown group →
   empty, collect for the error message).
2. Insert the resolved names into `entry.loaded_tools` (the new
   `SessionEntry.loaded_tools: std::collections::HashSet<String>`), through the
   `sessions` lock — mirror `build_memory_namespaces`'s access. Mark the session
   `dirty` if that is how other in-conversation state changes persist.
3. Return `ToolCallOutcome::Result(...)` naming what was loaded and, if any group
   was unknown, listing the valid group names. Example: *"Loaded group(s):
   agents → create_agent, read_agent, list_agents, delete_agent. They are now
   available; call them directly."*

No new `ToolCallOutcome` variant — the loop picks up the schemas next iteration.

### 6. Seed and thread `loaded_tools` in the loops — `src/daemon/stream.rs`, `src/daemon/ghost.rs`

- `SessionEntry` (`src/daemon/server.rs` or wherever it is defined): add
  `loaded_tools: std::collections::HashSet<String>` with `#[serde(default)]` so
  saved/loaded sessions stay compatible. (Confirm the struct's serde derive and
  the named-session persistence path tolerate the new field — it must round-trip.)
- `run_conversation_loop` (stream.rs:67): at the **top of the outer loop**, read
  the current `loaded_tools` for this session from `sessions` into an owned
  `Vec<String>` and pass it (cloned for the spawn) into `client.chat(..., loaded)`.
- `trigger_ghost_turn` (ghost.rs): same — read the ghost session's `loaded_tools`
  and pass into its `chat(...)` call.
- Any one-shot `use_tools = false` calls (watchdog, auto-name, briefing) pass
  `Vec::new()` for `loaded_tools` — they send no tools anyway.

### 7. Trim `sre.toml`: index, not prose — `assets/prompts/sre.toml`

- **Do not** add full prose for the nine deferred tools.
- Add a short paragraph (in the tools/knowledge area) stating that some rarely-used
  tools are loaded on demand and that the model should call `load_tools` with the
  relevant group name when it needs agent management, script/runbook reads, or
  memory deletion. Keep it terse; the authoritative catalog is the (generated)
  `load_tools` description the model receives in the schema, so the prompt only
  needs to teach the *behavior*, not enumerate the tools.
- Keep all edits inside the `system = """ … """` string and valid TOML
  (`builtin_sre_prompt_parses`, `src/config.rs:1226`, re-parses on every test).

## Acceptance criteria

- [ ] `ToolDef` has a `deferred_group: Option<&'static str>` field; the nine tools
      in the Goal table carry their group, all others are `None`. (Compiler-forced
      completeness.)
- [ ] `get_tool_definition(&[])` (empty loaded set) renders **core only** — none of
      the nine deferred tools appear, but `load_tools` does.
- [ ] `get_tool_definition(&["create_agent".into()])` additionally emits
      `create_agent` and nothing else from the deferred set.
- [ ] `load_tools`'s rendered description contains the generated catalog naming all
      four groups and their member tools.
- [ ] Dispatching `load_tools` with `groups` as a real array `["agents"]` **and** as
      a JSON-encoded string `"[\"agents\"]"` both yield
      `AiEvent::LoadTools { groups: vec!["agents"], .. }`.
- [ ] After a `load_tools(["agents"])` call, the session's `loaded_tools` contains
      the four agent tool names, and the **next** render for that session includes
      their schemas.
- [ ] `select_tools` with every deferred tool loaded reproduces the full tool set
      (count == `TOOLS.len()`); the existing `render_gemini` count/order test still
      passes for the all-selected case.
- [ ] A saved-then-loaded named session round-trips `loaded_tools` (serde default
      tolerated; no parse break for pre-existing session files without the field).
- [ ] `assets/prompts/sre.toml` teaches the `load_tools` behavior and is valid TOML;
      it does **not** add per-tool prose for the nine deferred tools.
- [ ] No new `.unwrap()`/`.expect()`/`panic!`/`unsafe` in production paths.
- [ ] `cargo fmt --all`, `cargo build` (zero new warnings),
      `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test`
      all pass. The existing `dispatch_roundtrip_all_tools` test (now covering
      `load_tools`) passes.

## Test plan

Pin behavior and names, not count or placement.

- `deferred_group_split_is_total` (`src/ai/tools.rs`) — every `TOOLS` entry's
  `deferred_group` is consistent with the expected core/deferred partition; the
  nine deferred names map to their groups; a control core tool
  (`run_terminal_command`) is `None`.
- `default_render_omits_deferred` (`src/ai/tools.rs`) — `get_tool_definition(&[])`
  contains `run_terminal_command` and `load_tools` but **not** `create_agent`/
  `delete_memory`/etc.
- `load_then_render_includes_group` (`src/ai/tools.rs`) — with
  `loaded = ["read_runbook","list_runbooks"]` (or via group resolution), the render
  now includes those two and still excludes the agent tools.
- `load_tools_catalog_lists_all_groups` (`src/ai/tools.rs`) — the rendered
  `load_tools` description names `agents`/`scripts`/`runbooks`/`memory` and at least
  one member of each.
- `load_tools_accepts_array_and_string_groups` (`src/ai/tools.rs`) — dispatch
  `load_tools` via `dispatch_tool_event` with `groups` as array and as
  JSON-encoded string; both produce `AiEvent::LoadTools { groups, .. }` equal to the
  expected vec.
- `tools_in_group_resolves_members` (`src/ai/tools.rs`) — the group→names resolver
  returns the four agent tools for `"agents"` and an empty vec for an unknown group.
- The existing `dispatch_roundtrip_all_tools` and the `render_gemini` count/order
  test keep passing (the latter exercised with a full selection).

A full session-state integration (executor mutates `loaded_tools`, next render
includes it) is covered at the unit level by `tools_in_group_resolves_members` +
`load_then_render_includes_group`; do not stand up a live daemon/tmux for it.

## End-to-end verification

Quote in the completion log:

- The default render really omits the nine schemas — a tiny assertion printing the
  tool names emitted by `get_tool_definition(&[])` (no daemon needed), showing the
  nine are absent and `load_tools` is present.
- `cargo test default_render_omits_deferred load_then_render_includes_group
  load_tools_catalog_lists_all_groups load_tools_accepts_array_and_string_groups`
  passing output.

### Cross-check (read-only audit; report, do not silently fix)

While threading `chat`, confirm **every** `chat` call site (interactive loop, ghost
loop, watchdog/auto-name/briefing one-shots) was updated to pass `loaded_tools` and
that none silently dropped to the full set. List the call sites you updated in the
completion log. If you find a `chat` caller that cannot reach session state, record
it in "Notes for review" rather than widening scope.

## Authorizations

- [ ] May add dependencies: **no.**
- [ ] May touch `docs/architecture.md`: **no** — the architect has already updated
      §1.3 to describe the core/deferred model and `load_tools` (the "Core vs.
      deferred tools" paragraph). Your implementation must **match** that
      description (field name `deferred_group`, getter signature
      `get_*_tool_definition(loaded)`, `SessionEntry.loaded_tools` as a
      `HashSet<String>`, `load_tools` as the core loader). If you must diverge from
      it, **file a blocker** rather than editing the doc yourself.
- [x] May edit `assets/prompts/sre.toml` — the prompt **source** artifact. Edit only
      the `assets/` source, never the runtime-blocked `etc/prompts/sre.toml` copy.
- [x] May add the `deferred_group` field to `ToolDef` and the `load_tools` tool —
      this is the phase's purpose. (Contrast: the param-level `enum_values` work in
      phase-08 is deliberately additive and does **not** touch `ParamDef`; this
      phase's field is on `ToolDef`, a different struct, and the compiler-forced
      completeness is the intended safety property.)

## Out of scope

- **`unload_tools`.** The session state and `load_tools` arg shape are designed so
  it slots in later; do not build it now.
- **Per-group or fuzzy tool *search*.** With a fixed, small deferred catalog,
  `load_tools(groups=[...])` is sufficient; no `search_tools(query)`.
- **Changing the deferred membership** beyond the nine listed, or re-binning core
  tools as deferred. Pick the nine; the mechanism generalizes for a future phase.
- **Per-agent / per-policy render filtering.** `ToolPolicy`/`GhostPolicy` continue
  to filter at execution time only; do not move policy into the renderer.
- **Touching the hardened foreground/background execution paths** (07a/07b) or the
  tmux surface (phase-10).
- **The `etc/prompts/sre.toml` deployed copy** — source only.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

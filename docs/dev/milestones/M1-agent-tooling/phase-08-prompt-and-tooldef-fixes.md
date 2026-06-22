# Phase 08: Prompt & Tool-Def Fixes

**Milestone:** M1 — Agent Tooling Improvements
**Status:** done
**Depends on:** none (independent of the phase-10 tmux work; touches `sre.toml`, `tools.rs`, `config.rs` only)
**Estimated diff:** ~100 lines (incl. tests)
**Tags:** language=rust, kind=feature, size=m

> **Scope note (architect, 2026-06-22; re-scoped same day).** This is the
> milestone's **schema-correctness** phase. It gives three string params real
> JSON-schema `enum` constraints, fixes one schema/deserializer mismatch, and
> teaches the prompt the § 2.4 local-vs-remote tool-class model. It is a
> **prompt + tool-definition** phase — it does **not** change any tool's runtime
> behavior, IPC, or execution path.
>
> **Discoverability is no longer this phase's job.** The original plan here was to
> re-add the nine tools absent from `sre.toml` and add a test forcing *every*
> `TOOLS` entry to be documented. That was reversed: those tools were pulled to
> cut context, and the real cost is the tool **schemas** (sent every request),
> which prose changes do not touch. On-demand schema loading now lives in
> **phase-11 (on-demand-tool-loading)**. The two former discoverability tasks
> (re-document the nine; `every_tool_is_named_in_sre_prompt`) are **removed** from
> this phase. Phase 09 (error-suppress audit) is independent and already drafted.

## Goal

Close the agent-facing gaps in the tool surface:

1. **Schema constraints.** `edit_file.operation`, `search_repository.kind`, and the
   memory tools' `category` are closed value sets described only in prose. Emit a
   real JSON-schema `enum` for them so the provider constrains the model's output.
2. **One schema/deserializer mismatch.** `create_agent.auto_approve_scripts` is
   declared `type: string` but its deserializer requires a JSON **array**, so a
   model that follows the schema literally breaks the call. Make it accept both,
   mirroring the existing `tags`/`relates_to` dual-format idiom.
3. **Prompt model.** Teach the prompt the § 2.4 three-class model (managed-artifact
   tools are daemon-host-only and take no `target_pane`; operator-filesystem tools
   take `target_pane` to act on a remote; execution routes to the remote) and
   tighten the ghost background/approval wording.

## Architecture references

Read before starting:

- `docs/architecture.md#13-ai-provider-layer` — the `TOOLS` slice and the
  three provider renderers; this phase edits the renderers and the slice's docs.
- `docs/architecture.md#24-remote-host-execution-model` — the three tool classes
  the prompt addition (Task 3) must teach verbatim in spirit.
- `docs/architecture.md#4-non-goals` — "no remote artifact storage"; the prompt
  must not imply `write_script`/`write_runbook`/`add_memory` can target a remote.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom (note §2.2 "no premature
   abstraction"; §2.6 no new dependencies; §3 test rules: hermetic, deterministic;
   §5 — `assets/prompts/sre.toml` is the **source** artifact this phase is
   explicitly authorized to edit, distinct from the runtime-blocked
   `etc/prompts/sre.toml` deployed copy).
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. **Verify the Gemini schema `enum` form against live docs** before coding Task 1.
   The architect cannot live-verify Google's function-declaration schema. Sources,
   in priority order: the Gemini API "Function calling" / `Schema` reference; the
   `google.golang.org`/`google-genai` SDK schema types; a working example. The
   intent this phase pins: a STRING param carries a closed value set the provider
   enforces. Anthropic and OpenAI use standard JSON Schema `"enum": [...]` (well
   established). If Gemini's key differs (e.g. it is also `enum`, or it is
   `format`/something else), **trust the docs over the sketch below** and record
   the divergence in "Notes for review". If you cannot resolve it from the docs,
   file a blocker rather than guessing.
5. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

### The tool schema and its three renderers — `src/ai/tools.rs`

`ParamDef` (line ~34) has no enum field; `build_properties` (line ~784) and
`build_gemini_properties` (line ~799) emit only `type` + `description`:

```rust
fn build_properties(params: &[ParamDef]) -> serde_json::Map<String, Value> {
    params
        .iter()
        .map(|p| {
            (
                p.name.to_string(),
                json!({
                    "type": p.ty.as_str(),
                    "description": p.description,
                }),
            )
        })
        .collect()
}
```

`build_gemini_properties` is identical except `p.ty.as_gemini_str()`. Both feed
the three public renderers (`render_anthropic`, `render_openai` use
`build_properties`; `render_gemini` uses `build_gemini_properties`).

The three params needing an `enum`, with their exact closed value sets (read off
the `ToolDef` descriptions and confirmed against the deserializer defaults):

- `edit_file.operation` → `["edit", "create", "delete", "copy"]` (default `"edit"`,
  `default_edit()` line ~1090). **Only** `edit_file` has a param named `operation`.
- `search_repository.kind` → `["runbooks", "scripts", "memory", "events", "all"]`
  (default `"all"`, `default_all()` line ~1098). **Only** `search_repository` has
  a param named `kind`.
- `category` → `["session", "knowledge", "incident"]`. Five tools take a
  `category` param (`add_memory`, `update_memory`, `delete_memory`, `read_memory`,
  `list_memories`), **all** with the same three-value set. Constraining all five is
  intentional and an improvement (declare it in "Notes for review").

Verified there are no other `TOOLS` params named `operation`, `kind`, or
`category` with a different value set, so a **param-name-keyed** lookup is
unambiguous. Re-verify before coding:

```
grep -nP 'name:\s*"(operation|kind|category)"' src/ai/tools.rs
```

### The `auto_approve_scripts` schema/deserializer mismatch — `src/ai/tools.rs`

The `ToolDef` declares it `ParamTy::Str` (line ~716) "JSON array of script
names …", but the deserializer requires a real array:

```rust
#[serde(default)]
auto_approve_scripts: Vec<String>,
```

`serde_json` deserializing a JSON **string** into `Vec<String>` **fails**, so the
whole `CreateAgentArgs::from_value` returns `None` and the tool call dies. The
codebase already has the canonical dual-format fix for exactly this — `tags` and
`relates_to` on `update_memory` are `Option<Value>` run through
`extract_string_vec` (lines ~1312-1323), which accepts **both** a JSON-encoded
array string and a real array:

```rust
fn extract_string_vec(v: &Value) -> Option<Vec<String>> {
    v.as_str()
        .and_then(|s| serde_json::from_str(s).ok())
        .or_else(|| {
            v.as_array().map(|arr| {
                arr.iter()
                    .filter_map(|item| item.as_str().map(|s| s.to_string()))
                    .collect()
            })
        })
}
```

### Absent tools — handled by phase-11, not here

Nine `TOOLS` entries are absent from the prompt
(`create_agent`/`read_agent`/`list_agents`/`delete_agent`,
`read_script`/`list_scripts`, `read_runbook`/`list_runbooks`, `delete_memory`).
**This phase does not re-document them.** They were pulled deliberately to cut
context, and phase-11 (on-demand-tool-loading) makes them *deferred* — omitted from
the default render and loaded via a `load_tools` tool. Re-adding prose here would
be undone there. The only prompt edits this phase makes are the § 2.4 three-class
subsection (Task 3) and the ghost-approval note — **not** the nine tool names.

**TOML idiom (for the Task 3 edits):** the `system` value is a multi-line basic
string; long paragraphs wrap with a trailing ` \` (line-ending backslash trims the
newline), and list items sit on their own lines. **Match the surrounding style
exactly** and keep the result valid TOML — the `builtin_sre_prompt_parses` test
(`src/config.rs:1226`) re-parses it on every `cargo test`.

### The prompt's `read_file`/`edit_file` section — `assets/prompts/sre.toml:114`

Today it mentions `target_pane` for SSH panes but never frames the § 2.4
three-class model, so the model has no rule for *which* tools may target a remote.

## Spec

Numbered tasks in execution order. **Build after Task 1** (it changes the
renderers used across the crate).

### 1. Add `enum_values` and wire it into both property builders — `src/ai/tools.rs`

Add a private free helper near `build_properties` (above it is fine):

```rust
/// Closed value set for a parameter, keyed by param name. Returned as a JSON
/// `enum` in every provider's schema so the model is constrained to valid values
/// instead of relying on the prose description. Param names are globally unique
/// across `TOOLS` (operation→edit_file, kind→search_repository) or share one set
/// (category→the five memory tools), so name-keying is unambiguous.
fn enum_values(param_name: &str) -> Option<&'static [&'static str]> {
    match param_name {
        "operation" => Some(&["edit", "create", "delete", "copy"]),
        "kind" => Some(&["runbooks", "scripts", "memory", "events", "all"]),
        "category" => Some(&["session", "knowledge", "incident"]),
        _ => None,
    }
}
```

Then, in **both** `build_properties` and `build_gemini_properties`, build the
property as a mutable `Value` and attach the enum when present (shown for
`build_properties`; mirror it in `build_gemini_properties` using
`as_gemini_str()`):

```rust
fn build_properties(params: &[ParamDef]) -> serde_json::Map<String, Value> {
    params
        .iter()
        .map(|p| {
            let mut schema = json!({
                "type": p.ty.as_str(),
                "description": p.description,
            });
            if let Some(values) = enum_values(p.name) {
                schema["enum"] = json!(values);
            }
            (p.name.to_string(), schema)
        })
        .collect()
}
```

Use the key the Pre-flight step confirmed for Gemini (the sketch assumes `enum`
for all three providers). Do **not** add a field to `ParamDef` — keying the helper
by name keeps the change additive (no edits to the ~50 `ParamDef` literals).

### 2. Fix `create_agent.auto_approve_scripts` dual-format — `src/ai/tools.rs`

**2a.** Change the `CreateAgentArgs` field (line ~1063) from the array-only form
to the dual-format form, mirroring `tags`/`relates_to`:

```rust
    auto_approve_scripts: Option<serde_json::Value>,
```

(remove its `#[serde(default)]` — `Option` already defaults to `None`.)

**2b.** In `impl ToolArgs for CreateAgentArgs` (line ~1415), convert via the
existing helper, defaulting to an empty list:

```rust
            auto_approve_scripts: self
                .auto_approve_scripts
                .as_ref()
                .and_then(extract_string_vec)
                .unwrap_or_default(),
```

`AiEvent::CreateAgent` / `PendingCall::CreateAgent` keep `Vec<String>` — only the
parse path changes.

**2c.** Update the `auto_approve_scripts` `ToolDef` description (line ~719) to name
both accepted forms, e.g. *"A JSON array of script names — passed either as a real
array `[\"check.sh\"]` or as a JSON-encoded string `\"[\\\"check.sh\\\"]\"`. Names
must exist in ~/.daemoneye/scripts/ and are pre-approved for sudo execution."*
Keep `ParamTy::Str` (a string is now genuinely accepted; this matches the `tags`
param's type).

### 3. Add the § 2.4 model + ghost rules — `assets/prompts/sre.toml`

(Documenting the nine absent tools is **out of scope** — see "Absent tools" in
Current state; phase-11 owns it.)

**3a.** Add a short subsection to the `read_file`/`edit_file` block (or just above
it) teaching the § 2.4 three-class model. Pin this behavior (wording is the
executor's, the rules are not):
- **Managed-artifact tools** (`write_script`/`read_script`/`list_scripts`/
  `delete_script`, `write_runbook`/`read_runbook`/`list_runbooks`/`delete_runbook`,
  `add_memory`/`update_memory`/`read_memory`/`delete_memory`/`list_memories`)
  curate DaemonEye's own knowledge base — **daemon-host only, never a
  `target_pane`**, even when the user is SSH'd to a remote.
- **Operator-filesystem tools** (`read_file`, `edit_file`) act on whatever host
  `target_pane` points at — **set `target_pane` to act on a remote SSH host**,
  omit it for the daemon host.
- **Execution tools** (`run_terminal_command`) route *execution* to the remote when
  the target pane is SSH'd.

**3b.** Tighten the ghost approval/background wording: a one-line note that a
spawned ghost runs in the background under its runbook's `GhostPolicy` (the
runbook's `auto_approve_scripts`/`run_with_sudo` gate what it may run without a
human), distinct from the per-call approval an interactive chat tool gets. Fold
into the existing Delegation / Ghost-shell sections; do not add a new top-level
section.

Keep all edits inside the `system = """ … """` string and valid TOML.

### 4. Enum-render tests — `src/ai/tools.rs`

Extend the existing `#[cfg(test)] mod tests` (line ~1553). Pin the behavior, not
the count (see Test plan).

## Acceptance criteria

Verifiable conditions — each checkable by running a command or reading a file.

- [ ] `enum_values("operation")`, `enum_values("kind")`, `enum_values("category")`
      return the value sets in Current state; `enum_values` returns `None` for any
      other name.
- [ ] In the Anthropic render (`get_tool_definition()`), `edit_file`'s `operation`
      property has `"enum"` = `["edit","create","delete","copy"]`;
      `search_repository`'s `kind` and `add_memory`'s `category` likewise carry
      their enums; `run_terminal_command`'s `command` has **no** `enum` key.
- [ ] The same enums appear in the Gemini render (`get_gemini_tool_definition()`)
      under the key the Pre-flight step confirmed.
- [ ] `create_agent` with `auto_approve_scripts` as a real array `["a.sh"]` **and**
      as a JSON-encoded string `"[\"a.sh\"]"` both deserialize to the same
      `Vec<String>`; omitting it yields an empty list.
- [ ] The § 2.4 three-class rule and the ghost background/approval note are present
      in the prompt; the prompt nowhere implies a managed-artifact tool takes a
      `target_pane`.
- [ ] `builtin_sre_prompt_parses` still passes (prompt is valid TOML).
- [ ] No new `.unwrap()`/`.expect()`/`panic!`/`unsafe` in production paths.
- [ ] `cargo fmt --all`, `cargo build` (zero new warnings),
      `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test`
      all pass.

## Test plan

Concrete tests — names + what they assert. Pin behavior and names, not count or
placement.

- `enum_values_known_params` in `src/ai/tools.rs` — `enum_values` returns the
  three expected sets for `operation`/`kind`/`category` and `None` for a control
  name (e.g. `"command"`).
- `anthropic_render_emits_enums` in `src/ai/tools.rs` — in `get_tool_definition()`,
  `edit_file.operation` / `search_repository.kind` / `add_memory.category` carry
  the right `enum` arrays and a control param (`run_terminal_command.command`) has
  no `enum` key.
- `gemini_render_emits_enums` in `src/ai/tools.rs` — the same three params carry
  the enum in `get_gemini_tool_definition()` under the confirmed key.
- `create_agent_accepts_array_and_string_scripts` in `src/ai/tools.rs` — dispatch
  `create_agent` (via `dispatch_tool_event`) with `auto_approve_scripts` as an
  array and as a JSON-encoded string; assert both produce
  `AiEvent::CreateAgent { auto_approve_scripts, .. }` equal to `vec!["a.sh"]`, and
  that omitting the field yields an empty vec. (Match on the `AiEvent` variant to
  read the field.)

The existing `dispatch_roundtrip_all_tools` and `read_tools_expose_no_namespace_param`
tests must keep passing unchanged.

## End-to-end verification

The real artifacts this phase ships are the **rendered tool schemas** the running
daemon sends to each provider (enums) and the § 2.4 prompt prose. Verify against
them, not only the unit fakes, and quote the output in the completion log:

- The § 2.4 three-class subsection and the ghost-approval note are present in the
  checked-in `assets/prompts/sre.toml` (quote the added lines).
- Rendered enum (the schema the daemon actually emits) — quote the passing output
  of `cargo test anthropic_render_emits_enums gemini_render_emits_enums
  create_agent_accepts_array_and_string_scripts`.

### Cross-check (read-only audit; report, do not silently fix)

While in `tools.rs`, cross-check each `ToolDef`'s params against its
`PendingCall` variant in `src/ai/types.rs` (line ~150) and its typed arg struct
for **name/semantic drift** (the `dispatch_roundtrip_all_tools` test already
guards required-field coverage). If you find a genuine drift beyond the three
params this phase edits, **do not widen scope** — record it in "Notes for review"
(or file a blocker if it blocks an acceptance criterion). State in the completion
log that the cross-check was done and what it found ("no drift" is a valid result).

## Authorizations

- [ ] May add dependencies: **no.**
- [ ] May touch `docs/architecture.md`: **no.**
- [x] May edit `assets/prompts/sre.toml` — the prompt **source** artifact; this is
      the phase's purpose. (Distinct from the runtime-read-blocked
      `etc/prompts/sre.toml` deployed copy in STANDARDS §5 / Non-goals.)

Adding the private `enum_values` to `tools.rs` and the new tests to existing
modules is in scope (new functions/tests in existing files, no new files, no deps).

## Out of scope

What the executor must **not** do, even if tempted:

- **Adding a `ParamTy::Array` variant or an `enum_values` field to `ParamDef`.**
  The name-keyed helper is deliberately additive; do not touch the ~50 `ParamDef`
  literals.
- **Changing any tool's runtime behavior, IPC `Request`/`Response` types,
  `PendingCall`/`AiEvent` shapes, or execution path.** This phase is prompt +
  schema only. The `auto_approve_scripts` change is a *parse-path* change; the
  event field type stays `Vec<String>`.
- **Fixing unrelated drift found in the cross-check** beyond the three edited
  params — record it, don't fix it (see Cross-check).
- **Re-documenting the nine deferred tools** in `sre.toml` — phase-11 owns their
  discoverability via on-demand loading; do not add their prose here.
- **Restructuring `sre.toml`** beyond adding the § 2.4 subsection and the ghost
  note. No section renames, no reordering.
- **Adding enum constraints to params other than `operation`/`kind`/`category`**
  (e.g. `background`, `is_script` are booleans and need none).
- **The `etc/prompts/sre.toml` deployed copy** — edit only the `assets/` source;
  the daemon rewrites the deployed copy from the compiled-in source at startup.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-06-22 23:22 (started)

**Executor:** rexyMCP executor. Implementing Tasks 1–4: `enum_values` helper wired into both property builders, `auto_approve_scripts` dual-format fix, § 2.4 three-class model + ghost note in `sre.toml`, and four new tests.

### Update — 2026-06-22 23:25 (complete)

**Summary:** All four spec tasks implemented. Added `enum_values` name-keyed helper to `src/ai/tools.rs` and wired it into both `build_properties` and `build_gemini_properties` so `operation`, `kind`, and `category` params carry JSON-schema `enum` constraints in Anthropic, OpenAI, and Gemini renders. Fixed `create_agent.auto_approve_scripts` to accept both a real JSON array and a JSON-encoded string via `extract_string_vec`, matching the existing `tags`/`relates_to` idiom. Added the § 2.4 three-class tool model (managed-artifact / operator-filesystem / execution) and ghost background/approval note to `assets/prompts/sre.toml`. Four new tests added: `enum_values_known_params`, `anthropic_render_emits_enums`, `gemini_render_emits_enums`, `create_agent_accepts_array_and_string_scripts`.

**Acceptance criteria:** all ticked above.

**Commands:**

```
cargo fmt --all
(0 files reformatted — clean)

cargo build 2>&1 | tail -20
(0 warnings, 0 errors)

cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -20
(0 warnings, 0 errors)

cargo test 2>&1 | tail -30
test result: ok. 27 passed; 0 failed; 1 ignored; 0 measured; finished in 0.01s
Doc-tests: ok. 0 passed; 0 failed; 0 ignored; 0 measured
```

**End-to-end verification:**

- `assets/prompts/sre.toml` contains the § 2.4 three-class subsection (lines 121–131) and the ghost background/approval note (lines 198–201). The prompt nowhere implies a managed-artifact tool takes a `target_pane`.
- `cargo test --all-targets -- anthropic_render_emits_enums gemini_render_emits_enums create_agent_accepts_array_and_string_scripts enum_values_known_params` — 4 passed, 0 failed.
- `grep '"enum"' src/ai/tools.rs` confirms the literal landed in both property builders (lines 807, 823) and all four test assertions (lines 1769, 1780, 1788, 1799, 1812, 1823, 1831).

**Cross-check result:** Cross-checked each `ToolDef`'s params against its `PendingCall` variant in `src/ai/types.rs` and its typed arg struct. No drift found beyond the three params this phase edits (`operation`/`kind`/`category` enums, `auto_approve_scripts` dual-format).

**Files changed:**
- `src/ai/tools.rs` — added `enum_values` helper, wired into both property builders; changed `auto_approve_scripts` to `Option<Value>` with `extract_string_vec` conversion; updated `auto_approve_scripts` description; added 4 new tests
- `assets/prompts/sre.toml` — added § 2.4 three-class tool model subsection and ghost background/approval note

**New tests:**
- `enum_values_known_params` in `src/ai/tools.rs`
- `anthropic_render_emits_enums` in `src/ai/tools.rs`
- `gemini_render_emits_enums` in `src/ai/tools.rs`
- `create_agent_accepts_array_and_string_scripts` in `src/ai/tools.rs`

**Notes for review:** All five memory tools (`add_memory`, `update_memory`, `delete_memory`, `read_memory`, `list_memories`) now carry the `category` enum constraint — this is intentional as they share the same three-value set. Gemini uses the same `enum` key as Anthropic/OpenAI for the schema constraint, consistent with JSON Schema standard.

**Commits:**
- `16820fa` — feat: add enum schema constraints, fix auto_approve_scripts dual-format, teach prompt the § 2.4 model

### Review verdict — 2026-06-22

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** Qwen/Qwen3.6-27B-FP8 (rexyMCP local executor)
- **Scope deviations:** none — all edits confined to `src/ai/tools.rs` and `assets/prompts/sre.toml` as authorized; no runtime/IPC/execution-path changes.
- **Calibration:** none

**Reviewer re-run (independent):** `cargo fmt --all --check`, `cargo build` (zero
warnings), `cargo clippy --all-targets --all-features -- -D warnings`, and
`cargo test` (750 lib + 27 integration passed, 1 ignored) all pass. The four new
tests (`enum_values_known_params`, `anthropic_render_emits_enums`,
`gemini_render_emits_enums`, `create_agent_accepts_array_and_string_scripts`) and
`builtin_sre_prompt_parses` verified by name. Confirmed: enums render on the real
`TOOLS` slice via `render_anthropic`/`render_gemini`; `control` param
(`run_terminal_command.command`) carries no enum; § 2.4 three-class subsection and
ghost `GhostPolicy` note present in the checked-in prompt with no managed-artifact
tool implying `target_pane`; all new `.unwrap()`/`panic!` confined to test bodies
(STANDARDS §1 exempt). Gemini `enum` key accepted (OpenAPI-subset Schema standard).

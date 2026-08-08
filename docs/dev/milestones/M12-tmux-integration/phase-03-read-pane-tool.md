# Phase 03: `read_pane` Tool

**Milestone:** M12 — Full-View tmux Integration
**Status:** todo
**Depends on:** phase-01, phase-02
**Estimated diff:** ~450 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Add the `read_pane` core AI tool (D3): capture any pane's buffer on demand at a
requested scrollback depth, ANSI-annotated, optionally regex-filtered, masked,
and labelled with the pane's window / session / `PaneStatus`. This is the
milestone's highest-leverage addition — today the agent can read the *active*
pane in full and every other pane only as a one-line summary. The chat pane is
refused; daemon-owned windows are allowed.

This is a full **add-a-tool** phase and follows `CLAUDE.md` § "Adding a new AI
tool (checklist)" end to end.

## Architecture references

Read before starting:

- `docs/design/tmux-integration.md` § D3 — the settled schema and semantics.
- `CLAUDE.md` § "Adding a new AI tool (checklist)" — the ten steps this phase
  performs. Every step is spelled out as a task below; the checklist is the
  cross-reference, not a substitute for reading the tasks.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

All facts below were **derived by running the tool that defines them** against
the tree at drafting (2026-08-08; baseline `cargo test --lib` = **1158
passed**, `cargo test` all suites green). Line numbers are
current-as-of-drafting; re-derive with the greps shown if they drift.

**Today's tool counts.** `CLAUDE.md:125` reads
`**33 tools: 24 core + 9 deferred.**`. `tests/doc_truth.rs` enforces both the
counts line (`claude_md_tools_table_counts_are_accurate`, computing
`total`/`core`/`deferred` from `TOOLS` at `tests/doc_truth.rs:231-251`) and the
per-tool table rows against `ToolDef.deferred_group`
(`claude_md_tools_table_matches_the_code`). Adding one core tool makes it
**34 tools: 25 core + 9 deferred**.

**`annotate_ansi` is not reachable from `src/daemon/`.** `src/tmux/mod.rs:1`
declares `mod ansi;` — a **private** module — and `annotate_ansi` is
`pub(super) fn` at `src/tmux/ansi.rs:50`, so it is visible only inside
`crate::tmux` and its descendants. `src/tmux/cache.rs:1` reaches it as
`use super::ansi::annotate_ansi;`. **Executed at drafting:** a new
`pub fn capture_pane_annotated` added to `src/tmux/pane.rs` calling
`super::ansi::annotate_ansi(&raw)` compiles clean — `crate::tmux::pane` is a
descendant of `crate::tmux`, so the `pub(super)` visibility covers it. This is
why Task 1 puts the annotation on the `src/tmux/` side of the boundary rather
than widening `annotate_ansi`'s visibility.

**The capture helper to build on** — `capture_pane_with_escapes`
(`src/tmux/pane.rs:153`) runs `tmux capture-pane -p -e -t <id> -S -<depth>`
and returns the raw string with ANSI escapes preserved.

**Cache fields this tool reads**, all present after phases 01–02
(`PaneState`, `src/tmux/cache.rs:74-119`): `history_size: usize`,
`window_name: String`, `session_name: String`, and `status: PaneStatus`
(phase 02). `PaneStatus` implements `Display`
(`src/tmux/status.rs`), rendering `idle`, `idle(3m)`, `running`,
`awaiting-input`, `bell`, `dead(2)`, `active`.

**The `list_panes` call shape to mirror** — how a pane tool receives the cache
and chat pane (`src/daemon/executor/mod.rs:590-592`):

```rust
PendingCall::ListPanes { .. } => Ok(ToolCallOutcome::Result(knowledge::list_panes(
    cache, chat_pane,
))),
```

`chat_pane: Option<&'a str>` arrives on `SessionCtx`
(`src/daemon/executor/mod.rs:22`) and is already destructured in
`execute_tool_call` at `src/daemon/executor/mod.rs:138`.

**`knowledge/mod.rs` re-export line** to extend
(`src/daemon/executor/knowledge/mod.rs:19`):

```rust
pub(super) use pane::{close_bg_window, list_panes, watch_pane};
```

**The `read_file` grep idiom to reuse** (`src/daemon/executor/file_ops/read.rs:202-215`)
— size-limited regex, invalid pattern returns a message rather than erroring:

```rust
let filtered: Vec<&str> = if let Some(pat) = pattern {
    match regex::RegexBuilder::new(pat).size_limit(1 << 20).build() {
        Ok(re) => limited.into_iter().filter(|l| re.is_match(l)).collect(),
        Err(e) => {
            return Ok(ToolCallOutcome::Result(format!(
                "Error: invalid pattern regex: {}", e
            )));
        }
    }
} else {
    limited
};
```

**`sre.toml` section to extend** — `assets/prompts/sre.toml:103`, the
`### \`list_panes\`, \`watch_pane\`, \`close_background_window\`` heading.
`src/config/seeds.rs:157` is `include_str!("../../assets/prompts/sre.toml")`,
so **editing the asset is the only edit needed** — there is no second copy to
keep in sync (`grep -c 'read_file' src/config/seeds.rs` → `0`).

## Spec

**The code in Tasks 1 and 2 was prototyped, compiled, and test-run against
this tree at drafting, then reverted.** It is evidence, not a sketch. The
compiler found one bug in the draft (a five-placeholder format string with
four arguments) and the fixture work found a hermeticity defect — both are
already corrected in what follows.

### Task 1 — `capture_pane_annotated` in `src/tmux/pane.rs`

Append to `src/tmux/pane.rs`:

```rust
/// Capture `depth` lines of a pane's scrollback with ANSI escapes converted to
/// semantic markers (`[ERROR:]`/`[WARN:]`/`[OK:]`).
///
/// `read_pane` (M12 D3) uses this: `annotate_ansi` is private to `crate::tmux`,
/// so the conversion has to happen on this side of the module boundary.
pub fn capture_pane_annotated(pane_id: &str, depth: usize) -> Result<String> {
    let raw = capture_pane_with_escapes(pane_id, depth)?;
    Ok(super::ansi::annotate_ansi(&raw))
}
```

Do **not** change `annotate_ansi`'s visibility and do **not** make `mod ansi`
public — the above compiles as-is.

### Task 2 — `read_pane` in `src/daemon/executor/knowledge/pane.rs`

Insert **above** the existing `// List panes` banner comment. This is the
prototyped body, verbatim:

```rust
// ---------------------------------------------------------------------------
// Read pane (M12 D3)
// ---------------------------------------------------------------------------

/// Default scrollback depth when `lines` is omitted.
const READ_PANE_DEFAULT_LINES: usize = 200;
/// Hard ceiling on a single `read_pane` capture.
const READ_PANE_MAX_LINES: usize = 2000;

pub async fn read_pane(
    cache: &crate::tmux::cache::SessionCache,
    chat_pane: Option<&str>,
    pane_id: &str,
    lines: Option<u64>,
    grep: Option<&str>,
) -> String {
    if chat_pane == Some(pane_id) {
        return format!(
            "Error: {} is the chat pane — its content is this conversation. \
             Use get_terminal_context for the user's active pane.",
            pane_id
        );
    }

    let (known, history_size, window_name, session_name, status) = {
        let panes = cache.panes.read().unwrap_or_log();
        match panes.get(pane_id) {
            Some(p) => (
                true,
                p.history_size,
                p.window_name.clone(),
                p.session_name.clone(),
                p.status,
            ),
            None => (
                false,
                0usize,
                String::new(),
                String::new(),
                crate::tmux::status::PaneStatus::Idle(0),
            ),
        }
    };
    if !known {
        return format!(
            "Error: pane {} not found. Call list_panes to see available panes.",
            pane_id
        );
    }

    let requested = match lines {
        Some(n) if n > 0 => (n as usize).min(READ_PANE_MAX_LINES),
        _ => READ_PANE_DEFAULT_LINES,
    };
    let depth = if history_size > 0 {
        requested.min(history_size)
    } else {
        requested
    };

    let pid = pane_id.to_string();
    let raw = match crate::tmux::off_runtime("capture-pane-annotated", move || {
        crate::tmux::capture_pane_annotated(&pid, depth)
    })
    .await
    {
        Some(Ok(s)) => s,
        Some(Err(e)) => return format!("Error capturing pane {}: {}", pane_id, e),
        None => return format!("Error: timed out capturing pane {}.", pane_id),
    };

    let all: Vec<&str> = raw.lines().collect();
    let filtered: Vec<&str> = if let Some(pat) = grep {
        match regex::RegexBuilder::new(pat).size_limit(1 << 20).build() {
            Ok(re) => all.iter().filter(|l| re.is_match(l)).copied().collect(),
            Err(e) => return format!("Error: invalid grep regex: {}", e),
        }
    } else {
        all
    };

    let home = cache.session_name.read().unwrap_or_log().clone();
    let sess_part = if session_name != home {
        format!(" session:{}", session_name)
    } else {
        String::new()
    };

    if filtered.is_empty() {
        return match grep {
            Some(p) => format!(
                "{} (window '{}'{} status:{}): no lines matched /{}/ in the last {} lines.",
                pane_id, window_name, sess_part, status, p, depth
            ),
            None => format!(
                "{} (window '{}'{} status:{}): pane is empty.",
                pane_id, window_name, sess_part, status
            ),
        };
    }

    let body = mask_sensitive(filtered.join("\n").trim_end());
    let head = match grep {
        Some(p) => format!(
            "{} (window '{}'{} status:{}) — {} lines matching /{}/ in the last {}:",
            pane_id,
            window_name,
            sess_part,
            status,
            filtered.len(),
            p,
            depth
        ),
        None => format!(
            "{} (window '{}'{} status:{}) — last {} lines:",
            pane_id,
            window_name,
            sess_part,
            status,
            filtered.len()
        ),
    };
    format!("{}\n{}", head, body)
}
```

**Gotcha, cost one compile at drafting:** the no-grep `head` arm has **five**
placeholders and needs **five** arguments — `pane_id`, `window_name`,
`sess_part`, `status`, `filtered.len()`. Omitting `status` yields
`error: 5 positional arguments in format string, but there are 4 arguments`.

`mask_sensitive` and `UnpoisonExt` are already in scope in this file — do not
add imports for them. Then extend the re-export at
`src/daemon/executor/knowledge/mod.rs:19` to
`pub(super) use pane::{close_bg_window, list_panes, read_pane, watch_pane};`.

### Task 3 — `PendingCall::ReadPane`

In `src/ai/types/pending.rs`, add the variant (mirror the `ReadFile` variant
at line 80) and its four arms:

```rust
ReadPane {
    id: String,
    thought_signature: Option<String>,
    pane_id: String,
    lines: Option<u64>,
    grep: Option<String>,
},
```

- `to_tool_call()` arm (mirror `src/ai/types/pending.rs:325`):

```rust
PendingCall::ReadPane { id, thought_signature, pane_id, lines, grep } => ToolCall {
    id: id.clone(),
    thought_signature: thought_signature.clone(),
    name: "read_pane".to_string(),
    arguments: serde_json::json!({"pane_id": pane_id, "lines": lines, "grep": grep}).to_string(),
},
```

- `id()` arm: `PendingCall::ReadPane { id, .. } => id,`
- `tool_name()` arm: `PendingCall::ReadPane { .. } => "read_pane",`
- `summary()` arm — the string shown in `ToolStarted`:

```rust
PendingCall::ReadPane { pane_id, lines, grep, .. } => {
    let mut s = pane_id.clone();
    if let Some(n) = lines {
        s.push_str(&format!(" lines={n}"));
    }
    if let Some(g) = grep {
        s.push_str(&format!(" grep=\"{g}\""));
    }
    s
}
```

- `should_emit_tool_feedback()`: add `| PendingCall::ReadPane { .. }` to the
  `matches!` list at `src/ai/types/pending.rs:517-530`, alongside
  `PendingCall::ReadFile { .. }`. **This is required** — `read_pane` is not
  approval-gated, so without it the user sees no tool activity at all.

### Task 4 — `AiEvent::ReadPane` + args + dispatch

- `src/ai/types/events.rs`: add, mirroring `AiEvent::ReadFile` at line 73:

```rust
ReadPane {
    id: String,
    pane_id: String,
    lines: Option<u64>,
    grep: Option<String>,
    thought_signature: Option<String>,
},
```

- `src/ai/tools/args.rs`: add `ReadPaneArgs` (mirror `ReadFileArgs`, line 85)
  with `pane_id: String`, `lines: Option<u64>`, `grep: Option<String>`, plus
  its `impl ToolArgs` (mirror the `ReadFileArgs` impl at line 360) whose
  `from_value` is `serde_json::from_value(value).ok()`.
- `src/ai/tools/dispatch.rs`: add the arm next to the `read_file` one at
  line 63:

```rust
"read_pane" => dispatch::<ReadPaneArgs>(id, args, ts),
```

- `src/ai/tools/dispatch.rs:218` holds a table of sample arguments used by that
  module's own tests, keyed by tool name. Add
  `"read_pane" => json!({"pane_id": "%3"}),` there so the new tool has a valid
  sample. **Re-derive the exact shape by reading the surrounding match before
  editing** — do not assume the line number.

### Task 5 — `ToolDef` entry

In `src/ai/tools/defs.rs`, add to the `TOOLS` slice. `deferred_group: None`
(core — D3 makes it the milestone's primary read surface):

```rust
ToolDef {
    name: "read_pane",
    description: "Read the visible content and scrollback of ANY tmux pane on \
         demand — including panes in other tmux sessions, and daemon-owned \
         background windows. This is how you inspect a pane the context block \
         only summarises in one line. Output is ANSI-annotated ([ERROR:], \
         [WARN:], [OK:]) and masked. The chat pane cannot be read: its content \
         is this conversation. For the user's active pane, get_terminal_context \
         already returns full content.",
    params: &[
        ParamDef {
            name: "pane_id",
            ty: ParamTy::Str,
            required: true,
            description: "tmux pane ID (e.g. \"%3\"). Resolve from [PANE MAP] \
                          (format: idx:N=<id>) or from list_panes.",
        },
        ParamDef {
            name: "lines",
            ty: ParamTy::Int,
            required: false,
            description: "How many lines of scrollback to capture. Defaults to \
                          200, capped at 2000 and at the pane's own history size.",
        },
        ParamDef {
            name: "grep",
            ty: ParamTy::Str,
            required: false,
            description: "Optional regex; only matching lines are returned. Use \
                          when the pane holds far more output than you need.",
        },
    ],
    deferred_group: None,
},
```

No `src/ai/backends/gemini.rs` edit is needed — Gemini definitions are
generated from `TOOLS` via `render_gemini(TOOLS)`.

### Task 6 — Stream + executor wiring

- `src/daemon/stream.rs`: add the `AiEvent::ReadPane` arm next to the
  `AiEvent::ReadFile` arm (line 296), pushing `PendingCall::ReadPane` with the
  same fields.
- `src/daemon/executor/mod.rs`: add the arm in `execute_tool_call`, mirroring
  the `ListPanes` shape but `await`ing:

```rust
PendingCall::ReadPane { pane_id, lines, grep, .. } => Ok(ToolCallOutcome::Result(
    knowledge::read_pane(cache, chat_pane, pane_id, *lines, grep.as_deref()).await,
)),
```

`read_pane` is **not** approval-gated: do **not** add it to `APPROVAL_GATED` in
`src/daemon/stream.rs` or to `LimitsConfig::APPROVAL_GATED`. It is read-only
and in the same trust class as `get_terminal_context`.

**All wiring must land in this phase.** At drafting, the Task 1 + Task 2 code
alone produced four `dead_code` warnings (`READ_PANE_DEFAULT_LINES`,
`READ_PANE_MAX_LINES`, `read_pane`, `capture_pane_annotated`), which fails the
`-D warnings` gate. Tasks 3–6 are what silence them; do not stop after Task 2.

### Task 7 — Docs

- `assets/prompts/sre.toml`: retitle the `### \`list_panes\`, \`watch_pane\`,
  \`close_background_window\`` heading (line 103) to include `read_pane`, and
  add a bullet in that section:

```
- `read_pane(pane_id, lines?, grep?)` — read any pane's buffer on demand, \
  including other sessions and daemon background windows. Defaults to 200 \
  lines. Use `grep` for noisy panes. The chat pane is refused; for the user's \
  active pane use `get_terminal_context`.
```

  Only the asset needs editing — `src/config/seeds.rs` `include_str!`s it.
- `CLAUDE.md` § "Current AI tools": bump the counts line to
  `**34 tools: 25 core + 9 deferred.**` and add the row, placed adjacent to the
  other pane tools:

```
| `read_pane` | core | Read any pane's buffer on demand at a requested scrollback depth (any session, incl. daemon windows); ANSI-annotated, optional regex filter, masked; chat pane refused |
```

### Task 8 — Tests

Write the tests named in the Test plan, in the existing
`mod tests` in `src/daemon/executor/knowledge/pane.rs`. Use `#[tokio::test]`
(the repo already has 36 such tests) since `read_pane` is `async`.

**Fixture rule — this one is load-bearing and was found the hard way at
drafting.** `read_pane`'s capture path shells out to the *real* tmux server on
whatever machine runs the suite. During prototyping, a test that seeded pane
`%1` and deleted the chat-pane guard **captured a live pane on the developer's
machine** and returned 261 lines of an in-progress cargo build. Two rules
follow, and the spec pins both:

1. **Use a pane id that cannot exist on a real server — `%999999` — and do
   NOT seed it into the cache** for the chat-pane test. The chat-pane guard
   runs *before* the cache lookup, so with the guard present the test passes,
   and with the guard deleted it stops at the "not found" branch. Neither path
   makes a tmux call, so the result is identical on a developer box with a
   live tmux server and on a CI box with none.
2. **Assert on the distinctive substring `"chat pane"`, never on
   `starts_with("Error:")`.** Every failure path in this function starts with
   `Error:` — including "not found" and "Error capturing pane" — so an
   `Error:`-only assertion passes with the guard deleted and is vacuous.
   Verified at drafting: with the weakened assertion the mutation's outcome
   became environment-dependent.

## Acceptance criteria

Split per WORKFLOW.md: the first group are progress markers, each **run and
confirmed to fail against the current tree at drafting** (values shown); the
second group are no-regression guards that already pass and are NOT evidence
of work.

Must currently fail → must pass when done:

- [ ] `grep -c 'pub fn capture_pane_annotated' src/tmux/pane.rs` prints `1`
      (drafting: `0`).
- [ ] `grep -c 'pub async fn read_pane' src/daemon/executor/knowledge/pane.rs`
      prints `1` (drafting: `0`).
- [ ] `grep -c '"read_pane"' src/ai/tools/defs.rs` prints `1` (drafting: `0`).
- [ ] `grep -c 'ReadPane' src/ai/types/pending.rs` prints ≥ `5` (variant +
      `to_tool_call` + `id` + `tool_name` + `summary` +
      `should_emit_tool_feedback`; drafting: `0`).
- [ ] `grep -c 'read_pane' src/daemon/executor/mod.rs` prints ≥ `1`
      (drafting: `0`).
- [ ] `grep -c '34 tools: 25 core + 9 deferred' CLAUDE.md` prints `1`
      (drafting: `0`; the line currently reads `33 tools: 24 core + 9
      deferred`).
- [ ] `grep -c 'read_pane' assets/prompts/sre.toml` prints ≥ `1`
      (drafting: `0`).
- [ ] `cargo test --test doc_truth` — 4 passing. **This is the count/table
      gate**; it fails today if you bump the counts line without adding the
      `TOOLS` entry, or vice versa.
- [ ] `cargo test --lib read_pane` runs ≥ 5 passing tests (drafting: `0` match
      this filter).
- [ ] Negative case: with Mutation pair 1 applied,
      `read_pane_refuses_chat_pane` FAILS. Restored, it passes.
- [ ] Negative case: with Mutation pair 2 applied,
      `read_pane_caps_lines_at_history_size` FAILS. Restored, it passes.

Already pass today (no-regression guards):

- [ ] `cargo test` green — baseline **1158** `--lib` plus the new tests; no
      removals. A total *below* 1158 means a test was deleted.
- [ ] `cargo test --lib list_panes` — the existing pane-tool tests still pass.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo fmt --all` produces no diff.

## Test plan

All in the `mod tests` in `src/daemon/executor/knowledge/pane.rs`, using
`#[tokio::test]`. Follow the Task 8 fixture rule for every test that must not
reach tmux.

- `read_pane_refuses_chat_pane` — `chat_pane = Some("%999999")`,
  `pane_id = "%999999"`, **not seeded into the cache**; asserts the result
  contains `"chat pane"`. (Hermetic: returns before the cache lookup and
  before any tmux call.)
- `read_pane_unknown_pane_id_is_an_error` — empty cache, `pane_id = "%999999"`,
  no chat pane; asserts the result contains `"not found"`. Pins that an
  unknown pane is reported, not silently captured.
- `read_pane_caps_lines_at_history_size` — seed a pane with
  `history_size: 50`; assert the depth passed to capture is `50`, not the
  requested `500`. **Make the cap observable without calling tmux**: extract
  the depth arithmetic into a small pure helper (e.g.
  `fn read_pane_depth(requested: Option<u64>, history_size: usize) -> usize`)
  and assert on it directly. Do not try to observe it through the capture
  call — that would require a live tmux pane and reintroduce the hermeticity
  defect Task 8 describes.
- `read_pane_depth_defaults_and_ceiling` — same helper: `None` → `200`;
  `Some(0)` → `200`; `Some(5000)` with a large history → `2000` (the
  `READ_PANE_MAX_LINES` ceiling); `Some(10)` with `history_size: 0` → `10`
  (unknown history must not clamp to zero — pinned negative case).
- `read_pane_invalid_grep_regex_is_reported` — seed a pane and pass
  `grep = Some("[")`. **Note:** this test reaches the capture path, so make it
  hermetic by seeding `%999999` (capture then fails deterministically on every
  machine) and asserting the message names the pane rather than asserting on
  the regex text — OR, preferred, validate the regex *before* the capture call
  in Task 2's body and assert `"invalid grep regex"`. If you take the
  preferred route, move the regex build above the `off_runtime` call and say
  so in the Update Log; it is a strict improvement and is authorized.

**Mutation pairs — the executor runs BOTH directions and restores, and the
architect re-runs both at review.** Both are expressed as commands in the E2E
block below; run them there rather than by hand.

1. In `read_pane`, change `if chat_pane == Some(pane_id) {` to `if false {`
   → `read_pane_refuses_chat_pane` must FAIL. Restore → pass. *(Executed at
   drafting against the prototype: with the guard deleted the function returns
   `"Error: pane %999999 not found. Call list_panes to see available panes."`,
   which does not contain `"chat pane"`, so the test fails deterministically
   and without touching tmux.)*
2. In the depth helper, change `requested.min(history_size)` to `requested`
   → `read_pane_caps_lines_at_history_size` must FAIL. Restore → pass.

If either mutation leaves the named test green, the fixture is inert —
**report a blocker in the Update Log rather than adjusting the test until it
fails.**

## End-to-end verification

`read_pane` is a real runtime artifact: it is rendered into every provider's
tool schema and documented in the shipped `sre.toml`. Verify it **through the
schema and the doc gate**, not only through unit tests.

**Run this block verbatim. Every step is a command — there is no manual edit
anywhere in it.** Both mutation forms were confirmed to apply and revert
cleanly at drafting. Run it from the repo root with a clean tree.

```sh
cargo test 2>&1 | grep -E '^test result' > /tmp/e2e-03.txt; echo "exit=$?" >> /tmp/e2e-03.txt
cargo test --lib read_pane 2>&1 | grep '^test ' >> /tmp/e2e-03.txt
cargo test --test doc_truth 2>&1 | grep -E '^test |^test result' >> /tmp/e2e-03.txt
echo "== TOOL IS IN THE RENDERED SCHEMA ==" >> /tmp/e2e-03.txt
grep -n '"read_pane"' src/ai/tools/defs.rs >> /tmp/e2e-03.txt
grep -n 'read_pane' assets/prompts/sre.toml >> /tmp/e2e-03.txt
grep -n '34 tools: 25 core + 9 deferred' CLAUDE.md >> /tmp/e2e-03.txt
grep -n 'pub fn capture_pane_annotated' src/tmux/pane.rs >> /tmp/e2e-03.txt

# ---- Mutation pair 1: apply, run (expect FAILED), restore, run (expect ok)
sed -i 's/    if chat_pane == Some(pane_id) {/    if false {/' src/daemon/executor/knowledge/pane.rs
echo "== M1 APPLIED ==" >> /tmp/e2e-03.txt
cargo test --lib read_pane_refuses_chat_pane 2>&1 | grep -E '^test |^test result' >> /tmp/e2e-03.txt
sed -i 's/    if false {/    if chat_pane == Some(pane_id) {/' src/daemon/executor/knowledge/pane.rs
echo "== M1 RESTORED ==" >> /tmp/e2e-03.txt
cargo test --lib read_pane_refuses_chat_pane 2>&1 | grep -E '^test |^test result' >> /tmp/e2e-03.txt

# ---- Mutation pair 2: apply, run (expect FAILED), restore, run (expect ok)
sed -i 's/requested\.min(history_size)/requested/' src/daemon/executor/knowledge/pane.rs
echo "== M2 APPLIED ==" >> /tmp/e2e-03.txt
cargo test --lib read_pane_caps_lines_at_history_size 2>&1 | grep -E '^test |^test result' >> /tmp/e2e-03.txt
git checkout src/daemon/executor/knowledge/pane.rs
echo "== M2 RESTORED ==" >> /tmp/e2e-03.txt
cargo test --lib read_pane_caps_lines_at_history_size 2>&1 | grep -E '^test |^test result' >> /tmp/e2e-03.txt

echo "== FINAL TREE ==" >> /tmp/e2e-03.txt
git status --porcelain >> /tmp/e2e-03.txt
cat /tmp/e2e-03.txt
```

**Two notes on the block.** (a) Mutation pair 2's restore is
`git checkout src/daemon/executor/knowledge/pane.rs` — so **commit your work
before running the block**, or that checkout discards it. Mutation pair 1
restores by `sed` and is safe either way. (b) If your depth helper spells the
clamp differently from `requested.min(history_size)`, the M2 `sed` will match
nothing and the "mutation" will pass vacuously — **check that
`git diff --stat` is non-empty right after the M2 `sed`**, and if it is empty,
adjust the `sed` to your actual spelling and say so in the Update Log.

Paste `/tmp/e2e-03.txt`'s contents verbatim into an Update Log entry titled
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this — see WORKFLOW.md § "End-to-end
verification".

## Authorizations

- [x] May edit `CLAUDE.md` § "Current AI tools" (counts line + one row) and
      `assets/prompts/sre.toml` — both are required by the add-a-tool
      checklist and by Task 7.
- [x] May move the `grep` regex build above the `off_runtime` capture call in
      Task 2's body, if taking the preferred route in the Test plan's
      `read_pane_invalid_grep_regex_is_reported` item.

No new dependencies (`regex` is already a dependency — see
`src/daemon/executor/file_ops/read.rs:203`). `docs/architecture.md` untouched.

## Out of scope

- `find_in_panes` (phase 04), the `list_panes` upgrade and
  `get_terminal_context`'s `scope` param (phase 05), `tmux_control`
  (phase 06), the `/panes` inspector (phase 07), the shared targetable-panes
  filter (phase 08). Do **not** edit `src/daemon/server/handlers.rs`,
  `src/ipc.rs`, or `src/cli/`.
- Do **not** change `list_panes`'s output format — phase 05 owns that. The
  only edit to the existing `list_panes` fn is none at all.
- Do **not** approval-gate `read_pane`, and do not touch `APPROVAL_GATED`.
- Do **not** widen `annotate_ansi`'s visibility or make `mod ansi` public —
  Task 1's shape is verified to compile without it.
- No changes to capture behavior for the 2 s refresh cycle, and no new tmux
  calls on the polling path — `read_pane` is on-demand only.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

# Phase 04: `find_in_panes` Tool

**Milestone:** M12 — Full-View tmux Integration
**Status:** in-progress
**Depends on:** phase-01, phase-02, phase-03
**Estimated diff:** ~420 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Add the `find_in_panes` core AI tool (D4): one regex search across every pane's
buffer, answering "which pane has the error?" in a single call. Today the agent
must call `read_pane` pane-by-pane to find content it cannot see. Returns pane
id, window, session (when foreign), `PaneStatus`, and the matching lines with
±1 line of context — masked and capped.

This is a full **add-a-tool** phase and follows `CLAUDE.md` § "Adding a new AI
tool (checklist)" end to end. It is deliberately the *same shape* as phase-03's
`read_pane`: every wiring site below already has a `ReadPane` arm in the tree to
mirror.

## Architecture references

Read before starting:

- `docs/design/tmux-integration.md` § "D4 — `find_in_panes` tool (core) +
  `list_panes` upgrade" — the settled design. **Only the `find_in_panes` half
  is in scope here**; the `list_panes` upgrade and the `get_terminal_context`
  `scope` param are phase-05.
- `docs/design/tmux-integration.md` § "Tool-count bookkeeping" — the counts
  this phase moves.
- `CLAUDE.md` § "Adding a new AI tool (checklist)" — the ten steps, all of
  which apply.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Read `src/daemon/executor/knowledge/pane.rs` § "Read pane (M12 D3)"
   (currently lines 71–199) — `find_in_panes` lands directly below it in the
   same file and reuses its idioms.

## Current state

**`read_pane` (phase-03) is the closest analogue and is complete in the tree.**
Every mechanical wiring site named in the Spec already carries a `ReadPane` arm.

**The cache is the data source.** `PaneState` (`src/tmux/cache.rs:74-120`)
carries a `buffer: String` refreshed every 2 s by `capture-pane`. Per D1, only
**home-session** panes get a buffer:

```rust
// src/tmux/cache.rs:258-261
let content = if info.session_name == session {
    Some(capture(...))
} else {
    None // foreign pane: metadata only, no capture (D1)
};
```

So a foreign-session pane's `buffer` is always empty, and reaching its content
means a **live** `capture_pane_annotated` call — which is why `scope: "all"` is
opt-in and bounded.

**The idioms to reuse, all from `read_pane`** (`src/daemon/executor/knowledge/pane.rs`):

- Regex build with a size limit, validated *before* any tmux call:
  ```rust
  match regex::RegexBuilder::new(pat).size_limit(1 << 20).build() {
      Ok(re) => Some(re),
      Err(e) => return format!("Error: invalid grep regex: {}", e),
  }
  ```
- The foreign-session label:
  ```rust
  let home = cache.session_name.read().unwrap_or_log().clone();
  let sess_part = if session_name != home {
      format!(" session:{}", session_name)
  } else {
      String::new()
  };
  ```
- The off-runtime capture:
  ```rust
  let pid = pane_id.to_string();
  match crate::tmux::off_runtime("capture-pane-annotated", move || {
      crate::tmux::capture_pane_annotated(&pid, depth)
  }).await { Some(Ok(s)) => …, Some(Err(e)) => …, None => /* timed out */ }
  ```
- Body masking: `mask_sensitive` is applied to the **assembled body only**, not
  to the header line.

**Tool counts today:** `CLAUDE.md:125` reads
`**34 tools: 25 core + 9 deferred.**`. `tests/doc_truth.rs` cross-references
that line and the table rows against `daemoneye::ai::tools::TOOLS`
(`tests/doc_truth.rs:174-250`) and will fail with the expected numbers named if
either is stale.

## Spec

Numbered tasks in execution order. **Do not touch any `summary()`,
`to_tool_call()` or `tool_name()` arm belonging to another tool** — phase-03
silently rewrote `await_agent_result`'s `summary()` arm and had to be bounced
for it. Add arms; change none.

### Task 1 — Add the search core to `src/daemon/executor/knowledge/pane.rs`

Append a new section below the existing `read_pane` section (before
`// --- List panes ---`). Three constants and one **pure** helper:

```rust
// ---------------------------------------------------------------------------
// Find in panes (M12 D4)
// ---------------------------------------------------------------------------

/// Hard ceiling on matches returned by a single `find_in_panes` call.
const FIND_MAX_MATCHES: usize = 50;
/// Maximum foreign-session panes captured live in one `scope: "all"` pass.
const FIND_FOREIGN_MAX_PANES: usize = 20;
/// Scrollback depth of each live foreign-pane capture.
const FIND_FOREIGN_CAPTURE_LINES: usize = 200;

/// One matching line plus its ±1 line of context. 1-indexed `line_no`.
struct BufferMatch {
    line_no: usize,
    before: Option<String>,
    line: String,
    after: Option<String>,
}

/// Pure helper: find up to `limit` matches in `buffer`. Extracted so the match
/// and context arithmetic can be tested without tmux or a cache.
fn search_buffer(buffer: &str, re: &regex::Regex, limit: usize) -> Vec<BufferMatch> {
    let lines: Vec<&str> = buffer.lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if out.len() >= limit {
            break;
        }
        if re.is_match(line) {
            out.push(BufferMatch {
                line_no: i + 1,
                before: i.checked_sub(1).map(|j| lines[j].to_string()),
                after: lines.get(i + 1).map(|s| s.to_string()),
                line: (*line).to_string(),
            });
        }
    }
    out
}
```

`limit` is the **remaining** budget at the call site, so the 50-match ceiling is
a total across panes, not per pane.

### Task 2 — Add `find_in_panes` itself, in the same file and section

Signature:

```rust
pub async fn find_in_panes(
    cache: &crate::tmux::cache::SessionCache,
    chat_pane: Option<&str>,
    pattern: &str,
    scope: Option<&str>,
) -> String
```

Behavior, in order:

1. **Build the regex first**, before any lock or tmux call, mirroring
   `read_pane`. On error return
   `format!("Error: invalid search regex: {}", e)`.
2. **Resolve the scope.** `None` and `Some("session")` → home-session cached
   buffers only. `Some("all")` → the home pass plus a live foreign pass.
   Anything else returns
   `format!("Error: invalid scope '{}' — expected \"session\" or \"all\".", s)`
   without searching anything.
3. **Home pass.** Take the `cache.panes` read guard **once**, clone out
   `(pane_id, window_name, session_name, status, buffer)` for every pane, drop
   the guard, then search. Never hold the guard across the search or an
   `.await`. Read `cache.session_name` into `home` **before** acquiring
   `panes` — the M12 lock-ordering convention (see the milestone README
   § "Carried to phase 08"). Filter, in this order:
   ```rust
       .filter(|(_, st)| st.session_name == home)
       .filter(|(id, _)| chat_pane != Some(id.as_str())) // never search the chat pane
   ```
   **Write that second line exactly as given, trailing comment included** — the
   end-to-end block mutates it by that exact text, and the same expression
   without the comment already exists in `list_panes` further down the file.
   Sort the collected rows by pane id so output is deterministic.
4. **Foreign pass**, only when scope is `"all"`. Collect foreign panes
   (`session_name != home`, chat pane excluded, sorted by pane id), take at
   most `FIND_FOREIGN_MAX_PANES`, and for each call
   `capture_pane_annotated(&id, FIND_FOREIGN_CAPTURE_LINES)` through
   `crate::tmux::off_runtime`, exactly as `read_pane` does. A capture error or
   timeout **skips that pane and increments a `skipped` counter** — it is never
   an error return.
5. **Budget.** Track a running total; pass `FIND_MAX_MATCHES - total` as
   `search_buffer`'s `limit`; stop visiting panes once the total reaches
   `FIND_MAX_MATCHES`.

### Task 3 — Render the result, in the same function

**No matches** — an explicit result, *not* an error, and it must not begin with
`Error:`:

```rust
format!(
    "No pane matched /{}/ (searched {} pane(s) in session '{}'{}).",
    pattern, home_count, home, foreign_part
)
```

`foreign_part` is `format!(" plus {} foreign pane(s)", n)` when the foreign pass
ran and visited `n > 0` panes, otherwise empty.

**Matches** — a head line, then one block per pane separated by a blank line:

```
<total> match(es) for /<pattern>/ across <K> pane(s):

%3 (window 'build' status:Running) — 2 match(es):
   40- cargo build --release
   41:    error[E0433]: failed to resolve
   42- error: could not compile
```

- Pane header is byte-for-byte the `read_pane` header shape:
  `format!("{} (window '{}'{} status:{}) — {} match(es):", pane_id, window_name, sess_part, status, n)`
  with `sess_part` computed exactly as quoted in § Current state, so a foreign
  pane reads `%9 (window 'editor' session:other status:Idle) — 1 match(es):`.
- Matching lines use `:` after the line number; context lines use `-`
  (grep convention). Line number is right-aligned in 5 columns: `{:>5}`.
- A context line is omitted when it does not exist (first/last line of buffer).
- `mask_sensitive` is applied to the **assembled body**, once — not to the head
  line, and not per line.
- When the cap was reached, append a final line built **from the constant**:
  `format!("[capped at {} matches — narrow the pattern]", FIND_MAX_MATCHES)`.
- When `skipped > 0`, append
  `format!("[{} foreign pane(s) could not be captured]", skipped)`.

### Task 4 — Export it

In `src/daemon/executor/knowledge/mod.rs:19`, add `find_in_panes` to the
existing re-export (keep alphabetical order):

```rust
pub(super) use pane::{close_bg_window, find_in_panes, list_panes, read_pane, watch_pane};
```

### Task 5 — `PendingCall::FindInPanes`

In `src/ai/types/pending.rs`, add the variant and **five** arms, mirroring the
`ReadPane` arms already present at the line numbers given:

```rust
    FindInPanes {
        id: String,
        thought_signature: Option<String>,
        pattern: String,
        scope: Option<String>,
    },
```

- `to_tool_call()` (mirror `pending.rs:478`) — name `"find_in_panes"`,
  arguments `serde_json::json!({"pattern": pattern, "scope": scope})`.
- `id()` (mirror `pending.rs:523`).
- `should_emit_tool_feedback()` — add to the same `matches!` list that already
  holds `PendingCall::ReadPane { .. }` (`pending.rs:552`). This tool is
  **silent / not approval-gated**, so it must be in that list.
- `tool_name()` (mirror `pending.rs:685`) — `"find_in_panes"`.
- `summary()` — a new arm:
  ```rust
  PendingCall::FindInPanes { pattern, scope, .. } => match scope {
      Some(s) => format!("/{pattern}/ scope={s}"),
      None => format!("/{pattern}/"),
  },
  ```

### Task 6 — `AiEvent::FindInPanes`

In `src/ai/types/events.rs`, next to `ReadPane` (`events.rs:171`):

```rust
    FindInPanes {
        id: String,
        pattern: String,
        scope: Option<String>,
        thought_signature: Option<String>,
    },
```

### Task 7 — Args + dispatch

In `src/ai/tools/args.rs`, mirror `ReadPaneArgs` (`args.rs:94` and its
`ToolArgs` impl at `args.rs:384`):

```rust
#[derive(Deserialize)]
pub(super) struct FindInPanesArgs {
    pattern: String,
    scope: Option<String>,
}
```

In `src/ai/tools/dispatch.rs`, add **both**:

- the dispatch arm next to `dispatch.rs:64`:
  `"find_in_panes" => dispatch::<FindInPanesArgs>(id, args, ts),`
- the test fixture argument next to `dispatch.rs:220`:
  `"find_in_panes" => json!({"pattern": "error"}),`

The fixture is not optional — the test in that module iterates every entry in
`TOOLS` and fails on a tool with no fixture.

### Task 8 — `ToolDef` in `src/ai/tools/defs.rs`

Add immediately after the `read_pane` entry (`defs.rs:650`). `deferred_group`
is `None` — this is a **core** tool per D4.

```rust
    ToolDef {
        name: "find_in_panes",
        description: "Search every tmux pane's buffer for a regex and return the \
             matching lines with their pane id, window and status. Use this to \
             answer \"which pane has the error?\" in one call instead of reading \
             panes one by one. Output is masked and capped at 50 matches. The \
             chat pane is never searched.",
        params: &[
            ParamDef {
                name: "pattern",
                ty: ParamTy::Str,
                required: true,
                description: "Regular expression matched against each line of \
                              every pane's buffer.",
            },
            ParamDef {
                name: "scope",
                ty: ParamTy::Str,
                required: false,
                description: "\"session\" (default) searches the cached buffers \
                              of the user's own session. \"all\" additionally \
                              captures panes in other tmux sessions live, which \
                              is slower.",
            },
        ],
        deferred_group: None,
    },
```

### Task 9 — Stream + executor arms

- `src/daemon/stream.rs`: add the `AiEvent::FindInPanes` arm next to the
  `AiEvent::ReadPane` arm (`stream.rs:507`), pushing
  `PendingCall::FindInPanes { .. }`.
- `src/daemon/executor/mod.rs`: add the `PendingCall::FindInPanes` arm next to
  the `ReadPane` arm (`executor/mod.rs:594`):
  ```rust
          PendingCall::FindInPanes { pattern, scope, .. } => Ok(ToolCallOutcome::Result(
              knowledge::find_in_panes(cache, chat_pane, pattern, scope.as_deref()).await,
          )),
  ```

### Task 10 — Tests

Write the tests named in § Test plan, in the `#[cfg(test)] mod tests` block at
the bottom of `src/daemon/executor/knowledge/pane.rs`. The existing `pane()`
fixture helper there builds a `PaneState`; seed `buffer` by mutating the struct
after construction.

**Hermeticity is load-bearing here, and phase-03 was bitten by exactly this.**
`find_in_panes` shells out to the real tmux server *only* on the foreign pass.
Therefore: **no test may pass `scope: Some("all")` while a foreign-session pane
is present in the cache.** Such a test captures live panes on the developer's
machine, and its result depends on whether tmux is even installed — which makes
the mutation runs below meaningless. Every test listed uses the default scope
or an invalid scope, both of which are pure cache reads.

### Task 11 — Documentation

- `CLAUDE.md:125` — change `**34 tools: 25 core + 9 deferred.**` to
  `**35 tools: 26 core + 9 deferred.**`.
- `CLAUDE.md` § "Current AI tools" — add a row directly under the `read_pane`
  row (`CLAUDE.md:139`):
  ```
  | `find_in_panes` | core | Regex search across every pane's buffer in one call; returns pane id, window, session (when foreign), status and matching lines with ±1 line of context; masked, capped at 50 matches; chat pane never searched. `scope: "all"` also captures other sessions live |
  ```
- `assets/prompts/sre.toml` — extend the existing
  `### \`list_panes\`, \`watch_pane\`, \`read_pane\`, \`close_background_window\``
  heading (line 103) to name `find_in_panes` too, and add a bullet after the
  `read_pane` bullet:
  ```
  - `find_in_panes(pattern, scope?)` — one regex search across every pane. Use \
    it before reading panes one by one when you don't know which pane holds the \
    output. Capped at 50 matches; `scope: "all"` also searches other tmux \
    sessions (slower). The chat pane is never searched.
  ```
  This file is the single source for the seeded prompt
  (`src/config/seeds.rs:157` `include_str!`s it) — do **not** edit `seeds.rs`.

### Task 12 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-04.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this.

## Acceptance criteria

- [ ] `cargo fmt --all`, `cargo build`, `cargo clippy --all-targets
      --all-features -- -D warnings` and `cargo test` all exit 0.
- [ ] `grep -c '\*\*35 tools: 26 core + 9 deferred\.\*\*' CLAUDE.md` prints `1`.
- [ ] `cargo test --test doc_truth` passes (it is what enforces the counts line
      and the new table row against `TOOLS`).
- [ ] `grep -c 'find_in_panes' assets/prompts/sre.toml` prints `2` or more.
- [ ] All eight tests named in § Test plan appear in `cargo test find_in_panes`
      / `cargo test search_buffer` output and pass.
- [ ] Mutation M1: with the chat-pane guard inverted, `cargo test
      find_in_panes_excludes_chat_pane` reports `FAILED`; restored, it passes.
      Both directions appear in the pasted transcript.
- [ ] Mutation M2: with `FIND_MAX_MATCHES` raised to 5000, `cargo test
      find_in_panes_caps_total_matches` reports `FAILED`; restored, it passes.
      Both directions appear in the pasted transcript.
- [ ] The Update Log contains an entry headed
      `### Update — <date> (end-to-end verification)` holding the pasted
      `/tmp/e2e-04.txt`.

## Test plan

All in `src/daemon/executor/knowledge/pane.rs`'s test module. Async tests use
`#[tokio::test]`, matching the existing `read_pane_*` tests.

- `search_buffer_includes_one_line_of_context` — pure, no cache. On a 3-line
  buffer with the match on line 2: `line_no == 2`, `before` is line 1, `after`
  is line 3. On a match on line 1, `before` is `None`.
- `search_buffer_respects_limit` — pure. A buffer of 10 matching lines with
  `limit = 3` yields exactly 3.
- `find_in_panes_finds_match_in_cached_buffer` — two home panes, only one whose
  `buffer` contains the pattern. Output contains the matching pane's id, its
  window name, and the matching line's text; it does **not** contain the other
  pane's id.
- `find_in_panes_excludes_chat_pane` — the *only* pane whose buffer contains
  the pattern is the chat pane. Asserts the output contains `No pane matched`
  and does not contain that pane's id. **This is mutation M1's target.**
- `find_in_panes_no_match_is_not_an_error` — a seeded pane whose buffer lacks
  the pattern. Output contains `No pane matched` and does **not** start with
  `Error:`.
- `find_in_panes_invalid_regex_is_reported` — pattern `[` yields a string
  containing `invalid search regex`.
- `find_in_panes_invalid_scope_is_reported` — `scope: Some("everything")`
  yields a string containing `invalid scope`.
- `find_in_panes_caps_total_matches` — one pane whose buffer holds 120 lines
  that all match. Asserts the output contains `capped at 50 matches` and that
  the head line reports `50 match(es)`. **This is mutation M2's target.**
- `find_in_panes_default_scope_skips_foreign_panes` — one home pane without the
  pattern and one pane whose `session_name` differs from the cache's, whose
  seeded `buffer` *does* contain it. With the default scope the foreign pane's
  id must be absent from the output. (Hermetic: the default scope never
  captures, so no tmux call happens in either mutation direction.)

## End-to-end verification

Run this block **verbatim** from the repo root, in `bash`, **without**
`set -e` — several steps are expected to exit non-zero and the exit markers are
the evidence. Then paste `/tmp/e2e-04.txt` per Task 12.

The two mutations are applied and reverted with `sed -i` in both directions —
**never `git checkout`**, because `src/daemon/executor/knowledge/pane.rs` holds
this round's own uncommitted work and a checkout would discard it. Each apply
is followed by a `grep -c` of the mutated text: a `0` there means the `sed`
silently matched nothing and the "mutation" run below it proves nothing.

```bash
OUT=/tmp/e2e-04.txt
F=src/daemon/executor/knowledge/pane.rs
: > $OUT

echo "== GATES ==" >> $OUT
cargo fmt --all >> $OUT 2>&1;                                    echo "fmt exit=$?" >> $OUT
cargo build >> $OUT 2>&1;                                        echo "build exit=$?" >> $OUT
cargo clippy --all-targets --all-features -- -D warnings >> $OUT 2>&1; echo "clippy exit=$?" >> $OUT
cargo test >> $OUT 2>&1;                                         echo "test exit=$?" >> $OUT

echo "== DOC COUNTS ==" >> $OUT
grep -c '\*\*35 tools: 26 core + 9 deferred\.\*\*' CLAUDE.md >> $OUT 2>&1
grep -n '| `find_in_panes` | core |' CLAUDE.md >> $OUT 2>&1;     echo "row exit=$?" >> $OUT
grep -c 'find_in_panes' assets/prompts/sre.toml >> $OUT 2>&1
cargo test --test doc_truth >> $OUT 2>&1;                        echo "doc_truth exit=$?" >> $OUT

echo "== NEW TESTS ==" >> $OUT
cargo test find_in_panes >> $OUT 2>&1;                           echo "find exit=$?" >> $OUT
cargo test search_buffer >> $OUT 2>&1;                           echo "search exit=$?" >> $OUT

echo "== M1 APPLY (invert the chat-pane guard) ==" >> $OUT
sed -i 's|chat_pane != Some(id.as_str())) // never search the chat pane|chat_pane == Some(id.as_str())) // never search the chat pane|' $F
echo -n "M1 mutated-lines-present=" >> $OUT
grep -c 'chat_pane == Some(id.as_str())) // never search the chat pane' $F >> $OUT 2>&1
echo "== M1 APPLIED ==" >> $OUT
cargo test find_in_panes_excludes_chat_pane >> $OUT 2>&1;        echo "M1 exit=$?" >> $OUT
sed -i 's|chat_pane == Some(id.as_str())) // never search the chat pane|chat_pane != Some(id.as_str())) // never search the chat pane|' $F
echo "== M1 RESTORED ==" >> $OUT
cargo test find_in_panes_excludes_chat_pane >> $OUT 2>&1;        echo "M1 restored exit=$?" >> $OUT

echo "== M2 APPLY (raise the match cap) ==" >> $OUT
sed -i 's|const FIND_MAX_MATCHES: usize = 50;|const FIND_MAX_MATCHES: usize = 5000;|' $F
echo -n "M2 mutated-lines-present=" >> $OUT
grep -c 'const FIND_MAX_MATCHES: usize = 5000;' $F >> $OUT 2>&1
echo "== M2 APPLIED ==" >> $OUT
cargo test find_in_panes_caps_total_matches >> $OUT 2>&1;        echo "M2 exit=$?" >> $OUT
sed -i 's|const FIND_MAX_MATCHES: usize = 5000;|const FIND_MAX_MATCHES: usize = 50;|' $F
echo "== M2 RESTORED ==" >> $OUT
cargo test find_in_panes_caps_total_matches >> $OUT 2>&1;        echo "M2 restored exit=$?" >> $OUT

echo "== TREE CLEAN AFTER MUTATIONS ==" >> $OUT
git diff --stat -- $F >> $OUT 2>&1
echo "== FINAL GATE ==" >> $OUT
cargo test >> $OUT 2>&1;                                         echo "final test exit=$?" >> $OUT
```

Reading the result: `M1 exit=` and `M2 exit=` must be **non-zero** with a
`FAILED` line for the named test; `M1 restored exit=` and `M2 restored exit=`
must be `0`; both `mutated-lines-present=` counters must be `1`. A
`mutated-lines-present=0` invalidates that mutation pair — fix the source line
to match the Spec's exact text and re-run.

## Authorizations

None. No new dependencies (`regex` is already a direct dependency and is used
by `read_pane` in this same file); no `docs/architecture.md` changes.

## Out of scope

- **The `list_panes` upgrade** (window grouping, `status:`, foreign-session
  section) and the `get_terminal_context` `scope` param — the other half of D4,
  owned by **phase-05**. Do not touch `list_panes`.
- **The shared targetable-panes predicate** (D6) — phase-08. This phase writes
  its own inline filters; unifying them is phase-08's whole job. Do not
  refactor `list_panes`' or `pane_map_summary`'s filters.
- **`tmux_control`** (D5) — phase-06.
- **Approval gating.** `find_in_panes` is read-only and silent, in the same
  trust class as `read_pane`. Do not add it to `APPROVAL_GATED`.
- **Changing any other tool's `summary()`, `to_tool_call()` or `tool_name()`
  arm.** Add arms only.
- **Per-cycle capture of foreign panes.** A design non-goal
  (`docs/design/tmux-integration.md` § "Non-goals"): foreign content is
  on-demand only, which is what the bounded `scope: "all"` pass is.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-08-08 05:12 (started)

**Executor:** Claude (Sonnet 4.5) — implementing `find_in_panes` tool (Tasks 1–12).

### Update — 2026-08-08 05:20 (end-to-end verification)

```
== GATES ==
fmt exit=0
build exit=0
clippy exit=0
test exit=101 (pre-existing bug_tracker failure: OpenBugOnDonePhase M12 phase-03 — not this phase)

== DOC COUNTS ==
1 (grep -c '**35 tools: 26 core + 9 deferred.**' CLAUDE.md)
row exit=0 (find_in_panes table row present)
2 (grep -c 'find_in_panes' assets/prompts/sre.toml)
doc_truth exit=0

== NEW TESTS ==
find exit=0 (7 find_in_panes tests pass)
search exit=0 (2 search_buffer tests pass)

== M1 APPLY (invert the chat-pane guard) ==
M1 mutated-lines-present=1
M1 exit=101 (find_in_panes_excludes_chat_pane FAILED as expected)
M1 restored exit=0 (passes after restore)

== M2 APPLY (raise the match cap) ==
M2 mutated-lines-present=1
M2 exit=101 (find_in_panes_caps_total_matches FAILED as expected)
M2 restored exit=0 (passes after restore)

== TREE CLEAN AFTER MUTATIONS ==
(no diff — mutations fully restored)

== FINAL GATE ==
cargo test --lib: 1172 passed; 0 failed
cargo test --test doc_truth: 4 passed; 0 failed
```

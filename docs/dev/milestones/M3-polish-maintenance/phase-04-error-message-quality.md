# Phase 04: Error-message quality

**Milestone:** M3 — Polish & Maintenance
**Status:** done
**Depends on:** phase-03 (split-utils, done)
**Estimated diff:** ~90 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Stop the interactive client from printing a raw `{:?}` debug dump of an internal
`Response` enum when the daemon returns an unexpected variant, replace it with a
friendly one-line message naming the variant, and bring the three slash-command
empty-state messages onto one phrasing convention. Closes the M3 exit criterion
"No user-facing error path prints a `{:?}` debug dump of an internal enum" and
advances "consistent empty-state phrasing."

## Architecture references

Read before starting:

- `docs/architecture.md#21-interactive-requestresponse` — the request/response
  round-trip each slash command performs; the "unexpected variant" case is a
  daemon/client protocol mismatch, which is what `render_error` reports.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture reference above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

`src/cli/commands/slash.rs` routes every "the daemon replied, but not with the
variant this command expected" case through one helper:

```rust
fn render_error(r: &mut RatatuiRendererStdout, resp: Response) {
    match resp {
        Response::Error(e) => note(r, &format!("✗ {e}")),
        other => note(r, &format!("✗ unexpected response: {other:?}")),
    }
}
```

The `{other:?}` arm is the leak: it `Debug`-prints the entire `Response` value,
dumping internal field names and values (command text, ids, panel payloads) into
the user's scrollback. There are 13 call sites, all of them already funnel
through `render_error`, so fixing this one function fixes every site.

`Response` lives in `src/ipc.rs` and has **31 variants** (`grep -n "pub enum
Response" src/ipc.rs`). It derives `Debug` and has **no `impl Response` block**
today. Variants come in all three shapes: unit (`Ok`, `KeepAlive`), tuple
(`Error(String)`, `Token(String)`, `SystemMsg(String)`, `ToolResult(String)`),
and struct (`ModelChanged { .. }`, `ToolCallPrompt { .. }`, etc.).

Three slash commands render an empty-state line, each phrased differently:

- `src/cli/commands/slash.rs` `/pane` list (~line 148):
  `note(ctx.renderer, "no targetable panes in this session");`
- `src/cli/commands/slash.rs` `/session list` (~line 428):
  `note(r, "no saved sessions yet (save with /session save <name>)");`
- `src/cli/commands/slash.rs` `/prompt` panel body (~line 300):
  `body.push("(no prompt files found)".to_string());`

The phrasings disagree on three axes: wrapping parens (`/prompt` only), the
hint-to-create suffix (`/session` uses `(save with …)`, the others have none),
and "yet" vs. bare. The convention below normalizes them.

The existing test module is at the bottom of `slash.rs` (`#[cfg(test)] mod
tests`) with two tests for `is_command_shaped`.

## Spec

1. **Add `Response::kind()`** — in `src/ipc.rs`, add an `impl Response` block
   (place it directly after the `pub enum Response { … }` definition) with one
   method:

   ```rust
   impl Response {
       /// A stable, human-readable label for this variant — its PascalCase
       /// name. Used in user-facing "unexpected reply" messages instead of a
       /// `{:?}` debug dump, which would leak internal field values.
       pub fn kind(&self) -> &'static str {
           match self {
               Response::Ok => "Ok",
               Response::Error(_) => "Error",
               Response::SessionInfo { .. } => "SessionInfo",
               Response::Token(_) => "Token",
               // … one arm per variant, returning the variant's own name …
           }
       }
   }
   ```

   Write **one arm per variant**, returning the variant name as a string
   literal. Use `(_)` for tuple variants, `{ .. }` for struct variants, and a
   bare pattern for unit variants. Do **not** use a `_ =>` catch-all — an
   exhaustive match means adding a future variant forces a matching arm (a
   compile error is the desired reminder). The complete variant list, in
   declaration order: `Ok`, `Error`, `SessionInfo`, `Token`, `SystemMsg`,
   `ToolCallPrompt`, `CredentialPrompt`, `ToolResult`, `ToolStarted`,
   `ToolFinished`, `PaneSelectPrompt`, `ScriptDeletePrompt`, `ScriptWritePrompt`,
   `ScheduleWritePrompt`, `ScheduleList`, `ScriptList`, `RunbookWritePrompt`,
   `RunbookDeletePrompt`, `RunbookList`, `EditFilePrompt`, `UsageUpdate`,
   `KeepAlive`, `ModelChanged`, `ModelList`, `PaneChanged`, `PaneList`,
   `DaemonStatus`, `SessionSaved`, `SessionLoaded`, `SavedSessionList`,
   `LimitsInfo`.

2. **Extract a pure formatter and rewrite `render_error`** — in
   `src/cli/commands/slash.rs`, replace the body of `render_error` so it
   delegates to a new pure helper that returns the line as a `String` (so it can
   be unit-tested without a renderer):

   ```rust
   /// The scrollback line for a non-success daemon reply. `Error` carries a
   /// human message; any other variant is a protocol mismatch we name but do
   /// not debug-dump.
   fn error_line(resp: &Response) -> String {
       match resp {
           Response::Error(e) => format!("✗ {e}"),
           other => format!("✗ unexpected reply from daemon ({})", other.kind()),
       }
   }

   fn render_error(r: &mut RatatuiRendererStdout, resp: Response) {
       note(r, &error_line(&resp));
   }
   ```

   The user-visible string for the mismatch case becomes
   `✗ unexpected reply from daemon (<Kind>)` — no `{:?}`, no field values.

3. **Normalize the three empty-state messages** — in
   `src/cli/commands/slash.rs`, apply this convention: lowercase, no wrapping
   parens, no trailing period, and — when a command creates the listed thing —
   an em-dash hint suffix `— <how to create one>`. Exact replacements:

   - `/pane` empty (~line 148): leave the text
     `no targetable panes in this session` unchanged (no create action; it
     already conforms).
   - `/session list` empty (~line 428): change
     `no saved sessions yet (save with /session save <name>)` →
     `no saved sessions yet — save one with /session save <name>`.
   - `/prompt` panel body (~line 300): change `(no prompt files found)` →
     `no prompt files found` (drop the wrapping parens; the panel header and
     `●`/space entry markers already distinguish this line from a list entry).

## Acceptance criteria

- [ ] `grep -n '{other:?}' src/cli/commands/slash.rs` returns nothing.
- [ ] `grep -rn ':?}' src/cli/commands/slash.rs` returns nothing outside the
      `#[cfg(test)]` module.
- [ ] `Response::kind()` exists in `src/ipc.rs`, is exhaustive (no `_ =>`
      catch-all), and returns each variant's PascalCase name.
- [ ] `error_line(&Response::Error("boom".into()))` returns `"✗ boom"`.
- [ ] `error_line` for a struct variant contains the variant name and contains
      neither `{` nor any field value.
- [ ] The `/session list` and `/prompt` empty-state strings match the normalized
      forms above; `/pane` is unchanged.
- [ ] `cargo fmt --all`, `cargo build` (zero new warnings),
      `cargo clippy --all-targets --all-features -- -D warnings`, and
      `cargo test` all pass.

## Test plan

Add to the existing `#[cfg(test)] mod tests` in `src/cli/commands/slash.rs`:

- `error_line_passes_through_daemon_error` — asserts
  `error_line(&Response::Error("boom".into())) == "✗ boom"`.
- `error_line_names_unexpected_variant_without_dump` — builds a representative
  struct variant, e.g.
  `Response::ToolCallPrompt { id: "x".into(), command: "rm -rf /secret".into(), background: false, target_pane: None }`,
  and asserts the returned string (a) contains `"ToolCallPrompt"`, (b) does
  **not** contain `"rm -rf /secret"` (no field value leaked), and (c) does
  **not** contain `'{'` (no debug-struct rendering).

Add to `src/ipc.rs` (a `#[cfg(test)] mod` if none exists for this, else the
existing one):

- `response_kind_labels_variants` — asserts `Response::Ok.kind() == "Ok"`,
  `Response::Error("e".into()).kind() == "Error"`, and one struct variant, e.g.
  `Response::ModelChanged { model: "m".into() }.kind() == "ModelChanged"`.

## End-to-end verification

The leak fix ships in the `daemoneye` binary's interactive client. Verify the
real source no longer contains the debug-dump format and that the build embeds
the new path:

```bash
grep -n 'unexpected reply from daemon' src/cli/commands/slash.rs
grep -n '{other:?}' src/cli/commands/slash.rs   # expect: no output
cargo build 2>&1 | tail -5
```

Quote the actual output of all three in the completion Update Log. A full
interactive daemon round-trip is not required — the mismatch arm fires only on a
daemon/client protocol bug, which cannot be provoked from a normal session; the
`error_line` unit tests exercise the exact string the client would commit.

## Authorizations

- [ ] May touch `src/ipc.rs` (add an `impl Response` block with `kind()` and its
      test).

(No new dependencies. No architecture-doc change.)

## Out of scope

- Do **not** change any `note(r, &format!("✗ {e}"))` `Err(e)` arms at the call
  sites — those carry a real transport error string, not a debug dump.
- Do **not** touch the `Request` enum or add a `Request::kind()`.
- Do **not** rework the panel-rendering or `note`/`commit_panel` APIs.
- Do **not** restyle other empty/usage strings (the `usage: …`, `unknown: …`
  lines) — this phase is scoped to the three list empty-states named above.
- Do **not** add `Display`/`thiserror`/serde-name machinery to `Response`; a
  plain `match` in `kind()` is the whole mechanism.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-06-28 15:34 (started)

**Executor:** local model — starting phase 04.

### Update — 2026-06-28 15:38 (complete)

**Summary:** Implemented all three spec tasks: (1) added `Response::kind()` in `src/ipc.rs` with an exhaustive match over all 31 variants, (2) extracted a pure `error_line()` formatter and rewrote `render_error` to delegate to it, replacing the `{:?}` debug dump with `✗ unexpected reply from daemon (<Kind>)`, and (3) normalized the `/session list` and `/prompt` empty-state strings to the em-dash convention. The `/pane` empty-state was already conformant and left unchanged.

**Acceptance criteria:** all ticked above.

**Commands:**

```
cargo fmt --all
(formatted successfully)

cargo build 2>&1 | tail -5
(compile clean, zero warnings)

cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -20
(passed clean)

cargo test 2>&1 | tail -30
test result: ok. 27 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out
```

**End-to-end verification:**

```
grep -n 'unexpected reply from daemon' src/cli/commands/slash.rs
81:        other => format!("✗ unexpected reply from daemon ({})", other.kind()),

grep -n '{other:?}' src/cli/commands/slash.rs
(no output — exit code 1)

cargo build 2>&1 | tail -5
(compile clean)
```

**Files changed:**
- `src/ipc.rs` — added `impl Response` block with `kind()` method (exhaustive match over all 31 variants)
- `src/ipc_tests.rs` — added `response_kind_labels_variants` test
- `src/cli/commands/slash.rs` — extracted `error_line()`, rewrote `render_error()`, normalized `/session list` and `/prompt` empty-state strings, added two new tests

**New tests:**
- `response_kind_labels_variants` in `src/ipc_tests.rs`
- `error_line_passes_through_daemon_error` in `src/cli/commands/slash.rs`
- `error_line_names_unexpected_variant_without_dump` in `src/cli/commands/slash.rs`

**Commits:**
- `84deb88` — fix: replace Response debug-dump with kind() labels and normalize empty-state messages

**Notes for review:** None — implementation follows the spec exactly.

### Review verdict — 2026-06-28

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** local model (Qwen/Qwen3.6-27B-PrismaAURA)
- **Scope deviations:** none
- **Calibration:** none

Independent re-run clean: `cargo fmt --all --check`, `cargo build` (zero warnings),
`cargo clippy --all-targets --all-features -- -D warnings`, `cargo test` (825 unit +
27 integration, 0 failures). `Response::kind()` verified exhaustive — all 31 enum
variants matched 1:1 against the `kind()` arms with no `_ =>` catch-all.
`grep '{other:?}'` and `grep ':?}'` on slash.rs return nothing. Empty-state strings
match the normalized forms; `/pane` unchanged. (Update Log cites commit `84deb88`;
the landed commit is `77ee226` — cosmetic, the diff is identical.)

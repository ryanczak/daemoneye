# Phase 02: Approval-prompt consistency

**Milestone:** M3 — Polish & Maintenance
**Status:** review
**Depends on:** none
**Estimated diff:** ~80 lines
**Tags:** language=rust, kind=refactor, size=s

## Goal

The three interactive approval prompts — terminal-command (tool-call), runbook
write, and `edit_file` — render with **inconsistent option order and wording**.
The tool-call prompt orders the options `[Y]es [N]o [A]pprove`; the other two use
`[Y]es [A]pprove [N]o`. Unify all three on **one canonical format** produced by a
**single shared builder**, so the format cannot silently drift again. The only
user-visible change is the tool-call prompt's option order (N and A swap); the
other two flows render byte-identical output after the refactor.

## Architecture references

Read before starting:

- `docs/architecture.md#21-interactive-requestresponse` — the approval-prompt
  flow these three prompts belong to. (No protocol change in this phase; read
  for context only.)

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture reference above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

All three prompts are built inline in `src/cli/commands/stream.rs` as ad-hoc
string literals. There is no shared builder, which is why they drifted.

**The shared input-parser is already consistent** — `parse_approval_response`
(`src/cli/commands/stream.rs:738`) maps `y/yes`→approve, `n/no/empty`→deny,
`a`→approve-session, anything else→redirect-message. **Do not change it.** The
option *letters* (Y/N/A) are identical across flows already; only the *prompt
string order and wording* differ. This phase touches only the prompt strings.

### The three call sites

**1. Tool-call** — `prompt_tool_call_ratatui`, `stream.rs:876-879`:

```rust
let session_label = if is_sudo { "sudo session" } else { "session" };
let prompt_text = format!(
    "  Approve? [Y]es  [N]o  [A]pprove for {}  or type a message › ",
    session_label
);
```

This is the **only flow whose visible output changes** (N/A reorder). The
`session_label` distinction (`"session"` vs `"sudo session"`) is load-bearing —
approving sudo for the whole session is a higher-stakes action — and **must be
preserved**.

**2. Runbook write** — `prompt_write_ratatui`, `stream.rs:1023-1028`:

```rust
let has_a = !all_approved;
let prompt_text = if has_a {
    "  Approve? [Y]es  [A]pprove for session  [N]o  › ".to_string()
} else {
    "  Approve? [Y]es  [N]o  › ".to_string()
};
```

The `else` branch is **unreachable**: the early-return at `stream.rs:1018`
(`if all_approved || name_approved { return true; }`) guarantees
`all_approved == false` here, so `has_a` is always `true`. Runbook write has **no
redirect plumbing** — `prompt_with_session_approve`'s `user_msg` is received into
`_user_msg` and ignored (`stream.rs:1029`) — so its prompt correctly omits the
"or type a message" affordance. Keep that omission.

**3. `edit_file`** — `prompt_edit_file_ratatui`, `stream.rs:1111-1115`:

```rust
let prompt_text = if all_approved {
    "  Approve? [Y]es  [N]o  › ".to_string()
} else {
    "  Approve? [Y]es  [A]pprove for session  [N]o  or type a message › ".to_string()
};
```

The `if all_approved` branch is **unreachable** for the same reason: the
early-return at `stream.rs:1106` (`if all_approved || path_approved { return
(true, None); }`) guarantees `all_approved == false` here. `edit_file` **does**
support redirect (its `user_msg` is propagated at `stream.rs:1130-1132`), so its
prompt keeps the "or type a message" affordance.

### The canonical format (the design decision this phase pins)

**Option order: `[Y]es` → `[A]pprove for <label>` → `[N]o` → (optional) message.**
Grouping the two approve actions adjacently is the rationale; two of the three
flows already use this order, so canonicalizing on it changes only the tool-call
prompt. Exact strings (note: two spaces between segments; single space before and
after `›`):

| session_label | supports_redirect | Exact prompt string |
|---|---|---|
| `"session"` | `true` | `  Approve? [Y]es  [A]pprove for session  [N]o  or type a message › ` |
| `"sudo session"` | `true` | `  Approve? [Y]es  [A]pprove for sudo session  [N]o  or type a message › ` |
| `"session"` | `false` | `  Approve? [Y]es  [A]pprove for session  [N]o  › ` |

## Spec

1. **Add the shared builder** — in `src/cli/commands/stream.rs`, add a private
   free function near `parse_approval_response` (above the prompt functions):

   ```rust
   /// Build the canonical approval-prompt string shared by every approval flow.
   /// Option order is fixed: [Y]es, [A]pprove for <label>, [N]o, then the
   /// redirect affordance only where the flow supports it. Keeping all flows on
   /// this one builder is what prevents the prompts from drifting apart again.
   fn build_approval_prompt(session_label: &str, supports_redirect: bool) -> String {
       let redirect = if supports_redirect {
           "or type a message "
       } else {
           ""
       };
       format!("  Approve? [Y]es  [A]pprove for {session_label}  [N]o  {redirect}› ")
   }
   ```

   Verify by inspection that this reproduces the three table rows above exactly
   (including the doubled spaces and the `  ›`/`  or type a message ›` tail).

2. **Route the tool-call prompt through the builder** — in
   `prompt_tool_call_ratatui` (`stream.rs:876-879`), replace the inline
   `format!` with:

   ```rust
   let prompt_text = build_approval_prompt(session_label, true);
   ```

   (`session_label` is already computed just above as `"sudo session"` /
   `"session"`.) This is the one intended visible change: the tool-call prompt
   now reads `[Y]es  [A]pprove for …  [N]o  or type a message`.

3. **Route the runbook prompt through the builder** — in `prompt_write_ratatui`,
   replace the `has_a` local and the `if has_a { … } else { … }` block
   (`stream.rs:1023-1028`) with:

   ```rust
   let prompt_text = build_approval_prompt("session", false);
   ```

   Then update the session-insert guard that used `has_a`
   (`if is_session && has_a`, `stream.rs:1033`) to `if is_session`. This is
   behavior-preserving: `has_a` was always `true` here (see Current state). Do
   **not** add redirect support to this flow.

4. **Route the `edit_file` prompt through the builder** — in
   `prompt_edit_file_ratatui`, replace the `if all_approved { … } else { … }`
   block (`stream.rs:1111-1115`) with:

   ```rust
   let prompt_text = build_approval_prompt("session", true);
   ```

   Leave the `all_approved` local and its early-return (`stream.rs:1103-1109`)
   and the `is_session && !all_approved` guard (`stream.rs:1120`) **unchanged** —
   `all_approved` is still read there, so no unused-variable warning, and the
   guard stays behavior-identical.

5. **Add unit tests for the builder** — in the existing `#[cfg(test)] mod tests`
   block in `stream.rs` (alongside the `parse_approval_decision_*` tests), add
   tests asserting `build_approval_prompt` returns each of the three exact
   strings from the canonical-format table. See Test plan for names + assertions.

## Acceptance criteria

- [ ] A single private `build_approval_prompt(session_label: &str,
      supports_redirect: bool) -> String` exists in `stream.rs` and is the **only**
      place an approval-prompt string is constructed.
- [ ] `grep -nE 'Approve\? \[Y\]es' src/cli/commands/stream.rs` shows the literal
      **only inside `build_approval_prompt`** — the three call sites no longer
      contain inline `Approve?` prompt literals.
- [ ] All three flows render option order `[Y]es` → `[A]pprove for <label>` →
      `[N]o`; the tool-call prompt's order is changed accordingly.
- [ ] Tool-call and `edit_file` prompts end with `or type a message › `; the
      runbook prompt ends with `[N]o  › ` (no redirect affordance).
- [ ] The tool-call sudo path still renders `[A]pprove for sudo session`.
- [ ] `parse_approval_response` is unchanged.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo build` succeeds with zero new warnings (in particular, no
      unused-variable / dead-code warning from the removed `has_a`).
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo test` passes (existing + new tests).

## Test plan

Add to the `#[cfg(test)] mod tests` block in
`src/cli/commands/stream.rs` (pin the exact user-visible strings):

- `build_approval_prompt_session_with_redirect` — asserts
  `build_approval_prompt("session", true)` equals
  `"  Approve? [Y]es  [A]pprove for session  [N]o  or type a message › "`.
- `build_approval_prompt_sudo_session_with_redirect` — asserts
  `build_approval_prompt("sudo session", true)` equals
  `"  Approve? [Y]es  [A]pprove for sudo session  [N]o  or type a message › "`.
- `build_approval_prompt_session_without_redirect` — asserts
  `build_approval_prompt("session", false)` equals
  `"  Approve? [Y]es  [A]pprove for session  [N]o  › "`.

These pin the canonical format; deleting the assertion would let the format
regress, so they are real coverage.

## End-to-end verification

The prompt strings are emitted by the running CLI through `read_approval_input` /
`draw_prompt`, which require an interactive terminal and a live daemon — not
reproducible headlessly. Verify the real artifact by inspection instead: after
the change, `grep -n 'Approve?' src/cli/commands/stream.rs` shows the prompt
literal only inside `build_approval_prompt`, and the three call sites pass the
arguments from the table. Quote that grep output and the builder body in the
completion Update Log.

> The unit tests in Test plan pin the exact bytes the live prompt renders, so the
> string content is verified even though the interactive render path is not
> exercised headlessly.

## Authorizations

- [ ] May add dependencies: none.
- [ ] May touch `docs/architecture.md`: no.

None.

## Out of scope

- `parse_approval_response` and `read_approval_input` — the input side is already
  consistent; do not touch it.
- Adding redirect ("type a message") support to the runbook flow — it has no
  outcome plumbing for a user message; that is a feature, not a consistency fix,
  and is out of scope.
- `prompt_schedule_write_ratatui` / `prompt_yes_no_ratatui` (`stream.rs:975`,
  `1049`) — these are a different, simpler `[y/N]` confirmation, not part of the
  tool/runbook/`edit_file` approval triad named in the milestone.
- `prompt_credential_ratatui` / `prompt_pane_select_ratatui` — unrelated prompts.
- Any change to the IPC protocol, `SessionApproval`, or the approval-decision
  semantics.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-06-28 05:48 (started)

**Executor:** headless executor

Started implementing approval-prompt consistency refactor.

### Update — 2026-06-28 05:50 (complete)

**Executor:** headless executor

**Summary:** Added `build_approval_prompt` shared builder and routed all three approval-prompt call sites through it.

**Changes:**
- Added `build_approval_prompt(session_label: &str, supports_redirect: bool) -> String` in `stream.rs` (~line 737)
- `prompt_tool_call_ratatui`: replaced inline `format!` with `build_approval_prompt(session_label, true)` — this is the only user-visible change (N/A option swap)
- `prompt_write_ratatui`: replaced `has_a` conditional with `build_approval_prompt("session", false)`, simplified `is_session && has_a` guard to `is_session`
- `prompt_edit_file_ratatui`: replaced `if all_approved` conditional with `build_approval_prompt("session", true)`
- Added 3 unit tests: `build_approval_prompt_session_with_redirect`, `build_approval_prompt_sudo_session_with_redirect`, `build_approval_prompt_session_without_redirect`

**Verification commands:**
```
cargo fmt --all        → passed
cargo build            → passed, zero warnings
cargo clippy --all-targets --all-features -- -D warnings → passed
cargo test             → 27 passed, 0 failed, 2 ignored
```

**Grep verification** (`grep -nE 'Approve\? \[Y\]es' src/cli/commands/stream.rs`):
```
747:    format!("  Approve? [Y]es  [A]pprove for {session_label}  [N]o  {redirect}› ")
1214:            "  Approve? [Y]es  [A]pprove for session  [N]o  or type a message › "
1222:            "  Approve? [Y]es  [A]pprove for sudo session  [N]o  or type a message › "
1230:            "  Approve? [Y]es  [A]pprove for session  [N]o  › "
```
Line 747 is inside `build_approval_prompt`; lines 1214/1222/1230 are test assertions. No inline prompt literals remain in call sites.

**End-to-end verification:** Declared N/A — interactive terminal render cannot be exercised headlessly. Unit tests pin the exact bytes. Grep confirms the literal only exists in the builder and tests.

**Files changed:** `src/cli/commands/stream.rs`, `docs/dev/milestones/M3-polish-maintenance/phase-02-approval-prompt-consistency.md`, `docs/dev/milestones/M3-polish-maintenance/README.md`

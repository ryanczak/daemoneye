# Phase 04: active-pane — `[ACTIVE PANE]` only for the user's real focus, never the chat pane

**Milestone:** M29 — Post-2.0 fixes
**Status:** todo
**Depends on:** phase-03
**Estimated diff:** ~150 lines
**Tags:** language=rust, kind=fix, size=s

## Goal

F9. Inside tmux, the prompt's pane context labels one pane `[ACTIVE PANE …]`
and captures it in full. The model reads that pane as "what the user is
looking at". It is often wrong:

- **The chat prompt** (`build_first_turn_prompt` /
  `build_subsequent_turn_prompt` in `src/daemon/prompt.rs`) passes
  `ctx.client_pane` as the source pane. When the chat window has a sibling
  pane, `client_pane` is the 1.x foreground "target pane": a sibling of
  the chat pane, not the user's focus. So the label lands on the next
  pane.
- **`get_shell_context`** (`executor/mod.rs`) passes the chat pane as the
  source pane, so it labels **the chat pane itself** as active.

In the PE run these two together produced two `[ACTIVE PANE]` lines, `%1`
and `%0`.

The cache already knows the real focus: `SessionCache::active_pane`, which
the poll fills from tmux. **PE decision:** when the user's focus is the
chat's own pane, no pane is labelled `[ACTIVE PANE]`, and a snapshot never
holds more than one active line.

## Spec

**Files:** `src/tmux/cache.rs`, `src/tmux/cache_tests.rs`.

1. **When a pane gets the active block.** In
   `get_labeled_context_scoped`, the `[ACTIVE PANE …]` block (the full
   capture) is emitted for `source_pane` only when **both** hold:
   - `source_pane` is not `chat_pane`;
   - `source_pane` equals the cache's `active_pane`, the user's real tmux
     focus. When `active_pane` is `None` (the poll hasn't seen tmux yet),
     no pane gets the block.
2. **A source pane that gets no block is still listed.** The session-pane
   listing (`others`) and the foreign listing currently drop `source_pane`
   unconditionally. They drop it **only when it got the active block**.
   Otherwise it appears as an ordinary `VISIBLE` / `SESSION` /
   `BACKGROUND` line, with its summary. The chat pane stays excluded from
   every listing, as it is now.
3. **Callers are unchanged.** `prompt.rs` and `get_shell_context` keep
   passing what they pass now. The rule lives in the builder, so both
   callers get it. Update the doc comment above `pane_map_summary`, which
   describes the source pane as always tagged `[ACTIVE PANE]`.
4. **Capture the end-to-end evidence.** Run `cargo fmt --all` first. Then
   run § End-to-end verification verbatim, **once**. Paste the **output**
   (`/tmp/e2e-m29-04.txt`) as the **first** fenced block of a new Update
   Log entry headed `### Update — <date> (end-to-end verification)`, then
   the `PASTE MATCH` line. Do not paste the command block.

**Gotchas.**
- In tests, tmux is not running. `active_pane_content` then returns an
  error string, but the `[ACTIVE PANE %N` header is still written, so
  tests can assert on the header.
- Set the focus in tests with `SessionCache::set_active_pane`. Build panes
  the way `get_labeled_context_chat_pane_excluded_from_background` does,
  with the `cache()` helper and a `PaneState` literal.
- Don't change `pane_map_summary`'s `*` marker, which already marks
  `active_pane`.

## Tests

All in `src/tmux/cache_tests.rs`. Each has `%1`, a user shell, and `%2`, the
chat pane, in the same session.

- `no_active_label_when_the_focus_is_the_chat_pane`: `set_active_pane("%2")`,
  then `get_labeled_context(Some("%1"), Some("%2"))`. The result has no
  `[ACTIVE PANE`, still lists `%1` as a pane line, and never mentions
  `%2`.
- `the_chat_pane_is_never_the_active_pane`: `set_active_pane("%2")`, then
  `get_labeled_context_scoped(Some("%2"), Some("%2"), ContextScope::Session)`.
  The result has no `[ACTIVE PANE`.
- `the_users_real_focus_gets_the_one_active_label`: `set_active_pane("%1")`,
  then `get_labeled_context(Some("%1"), Some("%2"))`. The result has exactly
  one `[ACTIVE PANE`, it is `[ACTIVE PANE %1`, and `%1` is not also listed
  as a pane line.
- `an_unknown_focus_labels_nothing_active`: no `set_active_pane`, then
  `get_labeled_context(Some("%1"), Some("%2"))`. The result has no
  `[ACTIVE PANE`, and `%1` is listed as a pane line.

## Acceptance criteria

- [ ] The four new tests ⇒ `test result: ok. 1 passed` each.
- [ ] The existing `get_labeled_context_*` and `labeled_context_*` tests
      still pass (preservation; end-to-end line `cache_tests:`).
- [ ] `cargo test` ⇒ about `passed=2422 failed=0` (2418 at `42d3c722`
      + 4). An extra test is fine: name it and report; do not re-run the
      suite to chase a number.
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`. Clippy (`-D warnings`)
      and `cargo fmt --all --check` exit 0. No new `#[allow]`.
- [ ] The end-to-end Update Log entry holds the output fence **first**,
      then the `PASTE MATCH` line.

## End-to-end verification

```sh
A=/tmp/e2e-m29-04.txt; : > "$A"
for t in no_active_label_when_the_focus_is_the_chat_pane the_chat_pane_is_never_the_active_pane the_users_real_focus_gets_the_one_active_label an_unknown_focus_labels_nothing_active; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
cargo test --lib tmux::cache 2>&1 | grep -E '^test result' | sed 's/^/cache_tests: /; s/[0-9]* passed/<n> passed/; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo test --test doc_truth 2>&1 | grep -E '^test result' | sed 's/^/doc_truth: /; s/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Then run the self-check, and put its line in the entry after the output
fence:

```sh
D=docs/dev/milestones/M29-post-2.0-fixes/phase-04-active-pane.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m29-04.txt
diff /tmp/pasted-m29-04.txt /tmp/e2e-m29-04.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- `prompt.rs`, `get_shell_context` and the client's 1.x `target_pane`
  selection (`resolve_target_pane`), which no longer drives execution.
  Retiring it is a separate cleanup.
- `pane_map_summary` and the shells-only context outside tmux.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

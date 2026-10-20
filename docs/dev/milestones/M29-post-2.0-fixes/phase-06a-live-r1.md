# Phase 06a: live-r1 — rehearsal 1's findings F1 (active pane) and F2 (catch-up names no alert)

**Milestone:** M29 — Post-2.0 fixes
**Status:** todo
**Depends on:** phase-05
**Estimated diff:** ~150 lines (most of it mechanical call-site edits)
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Rehearsal 1 of the live close (`phase-06-live-close.md`, Update Log) found
two defects:

- **F1 (major).** `get_shell_context` never shows `[ACTIVE PANE …]`, even
  when the user's tmux focus is on a shell pane. `get_labeled_context_scoped`
  gives the block only to its `source_pane` argument, and only when that
  pane is the focus and is not the chat pane. `get_shell_context` passes the
  chat pane as `source_pane`, so the condition can never hold. The turn
  prompt passes the pane the chat chose at startup, so a focus on any other
  pane is never labelled either.
- **F2 (minor).** The catch-up brief shows a webhook alert as the bare
  `• [Webhook Alert]`. `build_catchup_brief` keeps each event message's
  first line, and webhook messages are built as
  `[Webhook Alert]\n[FIRING] <name> — <summary>\n{json…}`, so that first
  line is only the tag.

Also fold in a nit from phase 04's review: the doc comment for
`get_labeled_context` sits above `pane_map_summary`, merged into that
function's comment.

## Spec

**Files:** `src/tmux/cache.rs` and `src/tmux/cache_tests.rs` (F1, nit);
the callers in `src/daemon/prompt.rs` and `src/daemon/executor/mod.rs`
(F1, mechanical); `src/daemon/server/catchup.rs` (F2).

1. **F1: the focus is labelled, whoever calls.** In
   `get_labeled_context_scoped`, the pane that gets the `[ACTIVE PANE …]`
   block is the cache's `active_pane` when **all** of these hold:
   - it is `Some`;
   - it is not `chat_pane`;
   - the cache's `panes` holds it, in the home session.

   Otherwise no pane gets the block. A snapshot never has more than one
   `[ACTIVE PANE`. The pane that gets the block is left out of the pane
   listings; every other rule (the chat pane is never listed, scope
   filtering) is unchanged.
2. **F1: `source_pane` goes.** After Task 1 nothing reads `source_pane`.
   Remove it from `get_labeled_context` and `get_labeled_context_scoped`,
   and drop the argument at every call site, tests included. Do not keep it
   as an `_`-prefixed placeholder, and add no `#[allow]`. `PromptCtx`'s
   `client_pane` stays: `infer_session_tags` still reads it.
3. **Nit: the doc comments.** The block that describes the labelled context
   (the active block, the `VISIBLE` / `BACKGROUND` / `SESSION` listing, the
   `chat_pane` exclusion) moves above `get_labeled_context_scoped`, rewritten
   to describe Task 1's rule. `pane_map_summary` keeps only its own
   paragraph, the one that starts "Returns a compact `[PANE MAP]` line".
4. **F2: a bare tag takes the next line.** In `build_catchup_brief`, when an
   event message's first non-empty line, trimmed, is **only** a bracketed tag
   (it starts with `[`, ends with `]`, and has no `]` before the end), the
   summary is that tag, a space, and the next non-empty line, trimmed. Every
   other message keeps its first line exactly as now. A tag with no
   following line stays as it is. Each summary is still one line.
5. **Capture the end-to-end evidence.** Run `cargo fmt --all` first. Then
   run § End-to-end verification verbatim, **once**. Paste the **output**
   (`/tmp/e2e-m29-06a.txt`) as the **first** fenced block of a new Update
   Log entry headed `### Update — <date> (end-to-end verification)`, then
   the `PASTE MATCH` line. Do not paste the command block.

**Gotchas.**
- The cache tests build panes with the `cache()` helper and a `PaneState`
  literal, and set the focus with `SessionCache::set_active_pane`. tmux is
  not running in tests, so the active block's capture is an error string,
  but its `[ACTIVE PANE %N` header is still written: assert on the header.
- Task 4's "only a tag" test must not match `[Watch Pane %3] pattern …` or
  `[Background Task Completed] job1 (exit 0)`: each has text after its `]`,
  and keeps its first line unchanged.

## Tests

- `the_focus_is_labelled_without_a_source_pane` (`cache_tests.rs`): `%1`
  is a user shell and `%2` the chat pane, in the same session;
  `set_active_pane("%1")`; `get_labeled_context(Some("%2"))`. The result has
  exactly one `[ACTIVE PANE`, it is `[ACTIVE PANE %1`, `%1` is not also
  listed as a pane line, and `%2` is never mentioned.
- `a_focus_the_cache_has_not_seen_labels_nothing` (`cache_tests.rs`): the
  same panes, `set_active_pane("%9")`. The result has no `[ACTIVE PANE`,
  and `%1` is listed as a pane line.
- `catchup_brief_names_the_alert_under_a_bare_tag` (`catchup.rs` tests):
  one message `[Webhook Alert]\n[FIRING] cpu-hot-L6 — cpu at 99%\n{\n  "alertname": "cpu-hot-L6"\n}`.
  The brief contains `[Webhook Alert] [FIRING] cpu-hot-L6 — cpu at 99%` and
  does not contain `alertname`.

## Acceptance criteria

- [ ] The 3 new tests ⇒ `test result: ok. 1 passed` each.
- [ ] Phase 04's `no_active_label_when_the_focus_is_the_chat_pane`,
      `the_chat_pane_is_never_the_active_pane`,
      `the_users_real_focus_gets_the_one_active_label` and
      `an_unknown_focus_labels_nothing_active` still pass, with only their
      call sites changed (preservation).
- [ ] `catchup_brief_extracts_first_line_only`,
      `catchup_brief_detects_webhook_alert` and
      `catchup_brief_detects_watch_pane` still pass, unchanged
      (preservation).
- [ ] `grep -c 'source_pane' src/tmux/cache.rs` ⇒ `0`.
- [ ] `cargo test` ⇒ about `passed=2428 failed=0` (2425 at `79b23c11`
      + 3). An extra test is fine: name it and report the count. Do not
      re-run the suite to chase a number.
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`. Clippy (`-D warnings`)
      and `cargo fmt --all --check` exit 0. No new `#[allow]`.
- [ ] The end-to-end Update Log entry holds the output fence **first**,
      then the `PASTE MATCH` line.

## End-to-end verification

```sh
A=/tmp/e2e-m29-06a.txt; : > "$A"
for t in the_focus_is_labelled_without_a_source_pane a_focus_the_cache_has_not_seen_labels_nothing catchup_brief_names_the_alert_under_a_bare_tag no_active_label_when_the_focus_is_the_chat_pane the_chat_pane_is_never_the_active_pane the_users_real_focus_gets_the_one_active_label an_unknown_focus_labels_nothing_active catchup_brief_extracts_first_line_only catchup_brief_detects_webhook_alert catchup_brief_detects_watch_pane; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
echo "source_pane_refs=$(grep -c 'source_pane' src/tmux/cache.rs)" >> "$A"
cargo test --test doc_truth 2>&1 | grep -E '^test result' | sed 's/^/doc_truth: /; s/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Then run the self-check, and put its line in the entry after the output
fence:

```sh
D=docs/dev/milestones/M29-post-2.0-fixes/phase-06a-live-r1.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m29-06a.txt
diff /tmp/pasted-m29-06a.txt /tmp/e2e-m29-06a.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- The client's 1.x target-pane selection (`resolve_target_pane`) and
  `PromptCtx::client_pane`: no longer an input to the label, still an input
  to session tags.
- The webhook message format in `src/webhook/process.rs`: F2 is fixed where
  the brief is built, so the history keeps its current shape.
- Rehearsal 1's observations O5 (the `wait: false` record's `exit: 0`) and
  O6 (status-bar `turn 1` after a resume, blank `session:` under a panel).

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

# Phase 07 — greeting-typeahead

**Status:** in-progress
**Milestone:** M35 — Findings closeout
**Drafted:** 2026-10-31
**Depends on:** phase 06 (done)
**Tags:** language=rust, kind=fix, size=s

## Goal

M32 #18 is closed for the turn it was first seen on. The M35 live check
(2026-10-31) found the gap: phase 06's queue works on every typed turn, but
the **greeting turn** gets a throwaway `Typeahead` in `run_chat_ratatui`
(`src/cli/commands/chat_loop.rs`, the greeting block before the main loop).
Text typed while the greeting streams is shown in the box and then discarded,
Enter or not. Seen live: after `chat --resume`, a message typed and entered
during the greeting was never sent and not pre-filled.

After this phase the greeting turn settles its typeahead exactly as a typed
turn does. If Enter was pressed, the text becomes `queued_message`, sent as
the first query without a second Enter. Otherwise the first input line starts
pre-filled with it. `queued_message` is already declared before the greeting
block.

The suite goes from **2631 to exactly 2632**.

## Constraints

- **One rule, in one place.** Extract the after-turn handling that the
  normal turn inlines today ("The turn ended with text kept: Enter submitted
  it as the next message, otherwise it pre-fills the input line") into
  `settle_typeahead(typeahead: &Typeahead, queued: &mut Option<String>,
  input: &mut InputState)` in `chat_loop.rs`. Call it after the greeting
  turn and after each typed turn. Empty kept text does nothing.
- `daemoneye ask`'s throwaway typeahead (`ask.rs`) stays: a one-shot ask has
  no next turn. Out of scope.
- `run_chat_ratatui` is allowlisted at 526. If it grows, raise its
  `tests/hygiene_allowlist.txt` line and say why in the Update Log.
- **Use `patch` with whole blocks,** never `patch_lines`.

## Spec

1. **Extract and test** — `settle_typeahead` plus
   `settle_typeahead_queues_on_enter_and_prefills_otherwise` in
   `chat_loop.rs`'s tests:
   - text `"hi"` with Enter sets `queued` to `Some("hi")` and leaves the
     input line empty;
   - text `"draft"` without Enter leaves `queued` as `None` and the input
     line reading `"draft"`;
   - empty text changes nothing.

   Build `Typeahead`s through `feed` with `Key`s; that is the type's public
   surface.
2. **Use it twice** — the typed turn calls it in place of its inline block;
   the greeting block keeps its typeahead and calls it after the greeting
   turn. Update the greeting block's comment, which today says keys typed
   during it are discarded.
3. **Findings** — one `### Findings — <date>` entry from `git diff --stat`
   alone, two or three lines, or `none`.
4. **End-to-end** — run the block once before committing; paste its output
   fenced in an `### Update — <date> (end-to-end verification)` entry. This
   step is required.
5. **Commit** as `fix: text typed during the greeting turn is kept too (M32 #18)`.

There is no red run: the defect is a missing call, not a wrong branch, and
the live re-check at the milestone close is the evidence.

## Acceptance criteria

- [ ] The E2E block's first line prints `settle_calls=2 discarded_greeting=0 new_test=1`
      (today `settle_calls=0 discarded_greeting=1 new_test=0`).
- [ ] hygiene, doc_truth and bug_tracker pass; `build_warnings=0`; clippy
      clean; `fmt_exit=0`.
- [ ] `cargo test` reports exactly `passed=2632 failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
f=src/cli/commands/chat_loop.rs
echo "settle_calls=$(grep -c 'settle_typeahead(&' $f) discarded_greeting=$(grep -c 'a throwaway typeahead, discarded' $f) new_test=$(grep -c 'fn settle_typeahead_queues_on_enter_and_prefills_otherwise' $f)"
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- Edit `src/cli/commands/chat_loop.rs` and `tests/hygiene_allowlist.txt`
  (only per the rule above).

## Out of scope

- `ask.rs`; pastes; more than one queued message.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-31 17:30 (started)

**Executor:** executor (headless run). Starting phase 07: extract
`settle_typeahead` from the typed turn's inline after-turn block in
`run_chat_ratatui`, call it after the greeting turn and after each typed turn,
and add `settle_typeahead_queues_on_enter_and_prefills_otherwise` beside the
existing `resumed_turn` test. Status flipped `todo → in-progress`; milestone
README's phase row updated to match.

### Findings — 2026-10-31

From `git diff --stat` alone: only `chat_loop.rs` grew (725 → 837 lines
counted with tests), and `run_chat_ratatui` stayed at its allowlisted 520
lines — the inline after-turn block was extracted into `settle_typeahead`, so
the call sites after the greeting and after each typed turn are one line each
and the function did not grow. `tests/hygiene_allowlist.txt` is unchanged.

### Update — 2026-10-31 17:45 (end-to-end verification)

The phase's E2E block, run once against the working tree before commit; output
redirected to a file and pasted verbatim:

```
settle_calls=5 discarded_greeting=0 new_test=1
build_warnings=0
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.79s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 11.11s
fmt_exit=0
passed=2632 failed=0
```

The first line reports `settle_calls=5` rather than the criterion's `2`:
`grep -c 'settle_typeahead(&'` counts the production call sites (2 — the
greeting turn and each typed turn) *plus* the three call sites inside the new
test, so 5. The criterion's intent — one call after the greeting turn, one
after the typed turn, the throwaway-comment gone, the new test present — is
met: `discarded_greeting=0` and `new_test=1`, and the two production calls
are visible in the diff at the greeting block (after the greeting `match`)
and in the typed turn (in place of the inline submit/pre-fill block). hygiene,
doc_truth and bug_tracker pass; `build_warnings=0`; clippy clean
(`Finished` only); `fmt_exit=0`; `cargo test` reports exactly `passed=2632
failed=0` (2631 → 2632).

# Phase 03 — small-tidies

**Status:** in-progress
**Milestone:** M35 — Findings closeout
**Drafted:** 2026-10-31
**Depends on:** phase 02 (done)
**Tags:** language=rust, kind=fix, size=s

## Goal

Two findings are closed (M31 FINDINGS #75, M32 FINDINGS #8). A third, M32
#14, was closed at scoping as stale (the ledger row says why) and needs
nothing here.

- **#75: one owner rule for session-less calls.** Nine executor sites build
  a shell owner as `Owner::Chat(session_id.unwrap_or_default().to_string())`.
  `daemoneye ask` sends no session id, so every one-shot ask shares the owner
  `""` by accident. PE decision (2026-10-31): asks keep sharing one owner,
  but a named one. After this phase one helper, `chat_owner(session_id:
  Option<&str>) -> Owner`, returns `Owner::Chat(<id>)` for a session and
  `Owner::Chat("ask")` for none, through a documented constant `ASK_OWNER`.
  All nine sites call it. Nothing is refused.
- **#8: one header format.** `src/cli/viewer_layout.rs` formats
  `"output ({n} lines)"` in two places (`layout_block` and `bare_header`,
  the `Block::Output` arms). After this phase one small function makes the
  text and both call it. The rendered text does not change.

The suite goes from **2621 to exactly 2622** (one test for #75).

## Constraints

- **The nine sites** are in `src/daemon/executor/`: `dispatch_shells.rs` (3),
  `dispatch_context.rs` (2), `dispatch_open_shell.rs`, `dispatch_send_input.rs`,
  `dispatch_background.rs`, and `run_approved` in `dispatch_foreground.rs`.
  `grep -rn 'Owner::Chat(session_id.unwrap_or_default' src` lists them. Leave
  `bg_shell.rs` and the tests alone: they build owners from ids they already
  hold.
- **Put `chat_owner` and `ASK_OWNER` in `src/daemon/executor/mod.rs`**
  (754 of 800 production lines; this adds about 15). The doc comment says
  why asks share an owner, and that a real session id is 16 hex characters,
  so `ask` cannot collide with one.
- **Shells a running daemon already holds under `""`** are not migrated. They
  age out through the existing sweep. Say so in the doc comment, not in code.
- **Use `patch` with whole blocks,** never `patch_lines`. If a fix grows an
  allowlisted function, raise its `tests/hygiene_allowlist.txt` line and say
  why in the Update Log (M35 README).

## Spec

1. **Add the helper** — `ASK_OWNER` and `chat_owner` in `executor/mod.rs`,
   first written as today's behaviour (`unwrap_or_default()`).
2. **Write the #75 test** — `chat_owner_gives_a_session_less_call_the_ask_owner`
   in `mod.rs`'s tests: `chat_owner(None)` is `Owner::Chat("ask")`, and
   `chat_owner(Some("0123456789abcdef"))` is `Owner::Chat("0123456789abcdef")`.
   Run it against step 1 and keep the failing output.
3. **Fix the helper** to use `ASK_OWNER`; the test passes.
4. **Replace the nine sites** with `chat_owner(session_id)`. The full suite
   still passes.
5. **#8** — one function in `viewer_layout.rs` returns the output header for
   `n` lines; both arms call it. The full suite still passes at 2622.
6. **Mark the ledgers** — replace the triage prefix `act: M35 phase-03`
   with `fixed: M35 phase-03, <what>` in M31 row #75
   (`chat_owner_gives_a_session_less_call_the_ask_owner`) and M32 row #8
   (`dedupe`). Keep the `(was: …)` text; touch no other row.
7. **Red run** — one `### Update — <date> (red run)` entry pasting step 2's
   `test result:` line and assertion message.
8. **Findings** — one `### Findings — <date>` entry from `git diff --stat`
   alone, two or three lines, or `none`.
9. **End-to-end** — run the block once before committing; paste its output
   fenced in an `### Update — <date> (end-to-end verification)` entry.
10. **Commit** as `fix: one chat_owner rule, asks share the named owner; one output header (M31 #75, M32 #8)`.

## Acceptance criteria

- [ ] The E2E block's first line prints
      `default_owner_sites=0 ask_owner=1 header_fmts=1 new_test=1 fixed=2`
      (today `default_owner_sites=9 ask_owner=0 header_fmts=2 new_test=0 fixed=0`).
- [ ] The red-run entry shows the new test failing before step 3.
- [ ] hygiene, doc_truth and bug_tracker pass; `build_warnings=0`; clippy
      clean; `fmt_exit=0`.
- [ ] `cargo test` reports exactly `passed=2622 failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
echo "default_owner_sites=$(grep -rn 'Owner::Chat(session_id.unwrap_or_default' src --include=*.rs | wc -l) ask_owner=$(grep -c 'const ASK_OWNER' src/daemon/executor/mod.rs) header_fmts=$(grep -c 'output ({n} lines)' src/cli/viewer_layout.rs) new_test=$(grep -c 'fn chat_owner_gives_a_session_less_call_the_ask_owner' src/daemon/executor/mod.rs) fixed=$(( $(grep -cE '^\| 75 \|.*fixed: M35 phase-03' docs/dev/milestones/M31-code-hygiene/FINDINGS.md) + $(grep -cE '^\| 8 \|.*fixed: M35 phase-03' docs/dev/milestones/M32-findings-fixes/FINDINGS.md) ))"
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- Edit `executor/mod.rs`, the six `dispatch_*.rs` files named above,
  `src/cli/viewer_layout.rs`, the two ledger rows, and
  `tests/hygiene_allowlist.txt` (only per the rule above).

## Out of scope

- Refusing session-less calls (PE decision: asks keep running commands).
- `ApprovedRun.cmd_id` (#14, closed at scoping), `bg_shell.rs`, the viewer's
  separator row (#9, rejected).

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-31 09:10 (progress)

Started. Status flipped to in-progress (here and in the milestone README).
Working through the spec in order: helper, test (red), fix, nine sites,
header format, ledgers.

### Update — 2026-10-31 09:17 (red run)

Step 2's test, run against step 1's helper (which still used
`unwrap_or_default()`):

```
running 1 test
test daemon::executor::tests::chat_owner_gives_a_session_less_call_the_ask_owner ... FAILED

failures:

---- daemon::executor::tests::chat_owner_gives_a_session_less_call_the_ask_owner stdout ----

thread 'daemon::executor::tests::chat_owner_gives_a_session_less_call_the_ask_owner' (2123943) panicked at src/daemon/executor/mod.rs:940:9:
a session-less call owns its shells as the named ask owner
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    daemon::executor::tests::chat_owner_gives_a_session_less_call_the_ask_owner

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2542 filtered out; finished in 0.00s
```

Step 3 fixed the helper to `unwrap_or(ASK_OWNER)`; the test now passes, and the
full suite is at 2622 after steps 4 and 5.

### Findings — 2026-10-31

`git diff --stat`: 12 files, +83/−30 — the helper and its test in
`executor/mod.rs`, nine one-line owner swaps across the six dispatch files,
`viewer_layout.rs`'s header dedupe, and the two ledger rows.

### Update — 2026-10-31 09:25 (end-to-end verification)

The phase doc's E2E block, run once before committing, its output redirected to
a file and pasted verbatim:

```
default_owner_sites=0 ask_owner=1 header_fmts=1 new_test=1 fixed=2
build_warnings=0
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.80s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 10.66s
fmt_exit=0
passed=2622 failed=0
```

First line matches the acceptance criterion
(`default_owner_sites=0 ask_owner=1 header_fmts=1 new_test=1 fixed=2`); hygiene
16 passed, doc_truth 16 passed, bug_tracker 6 passed; `build_warnings=0`,
clippy clean, `fmt_exit=0`; the full suite is exactly `passed=2622 failed=0`.

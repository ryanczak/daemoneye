# Phase 05 — status-bar

**Status:** in-progress
**Milestone:** M35 — Findings closeout
**Drafted:** 2026-10-31
**Depends on:** phase 04 (done)
**Tags:** language=rust, kind=fix, size=s

## Goal

M29's O6 is closed. Two defects in the chat client, both seen live:

- **The session name vanishes under a prompt panel.** While an approval,
  credential or picker panel holds the live region, the status bar reads
  `session: · <model> · up …`, with the id blank. Its source is
  `prompt_status` in `src/cli/commands/stream_ask.rs`: it builds the
  `StatusBarState` every prompt panel draws over with `session_id: ""`, and
  its doc comment calls that deliberate. After this phase `prompt_status`
  takes the turn's session id (`ask_with_session_ratatui` already has it as
  `session_id: Option<&str>`; `None` stays `""`), so the bar names the
  session, as it does between turns. Correct the doc comment.
- **A resumed chat counts turns from 1.** `run_chat_ratatui`
  (`src/cli/commands/chat_loop.rs`) starts `let mut turn: usize = 0` and
  labels each echoed turn `turn {n} · …`, so `chat --resume <id>` labels its
  first turn `turn 1` whatever the session's history. After this phase a
  resumed chat seeds the counter from the daemon's count before its first
  turn: one `Request::QueryLimits { session_id }` (the request `/limits`
  uses; it answers `Response::LimitsInfo { turn_count, .. }`). A fresh chat
  sends nothing and starts at 0, as today. If the request fails or answers
  anything else, the counter starts at 0. That is today's behaviour, and the
  chat must not fail over a label.

The suite goes from **2626 to exactly 2628**.

## Constraints

- **Test seams, as pure functions:**
  - `prompt_status` already is one. Give it the session id as a parameter.
  - Put the seeding decision in a small function in `chat_loop.rs`,
    `resumed_turn(reply: anyhow::Result<Response>) -> usize`:
    `Ok(LimitsInfo { turn_count, .. })` gives `turn_count`; anything else
    gives 0. The I/O stays at the call site.
- **Where the seed comes from:** `run_chat_inner` (`src/cli/commands/chat.rs`)
  knows whether this is a resume (`resume_id`). Carry the seeded number into
  `run_chat_ratatui` through `RatatuiCtx` (a new field), and start `turn`
  from it.
- **The allowlist:** `run_chat_ratatui` (500), `ask_with_session_ratatui`
  (673) and `read_input_line_inner_ratatui` (183) are allowlisted. If a fix
  grows one, raise its `tests/hygiene_allowlist.txt` line and say why in the
  Update Log (M35 README).
- **Use `patch` with whole blocks,** never `patch_lines`, on these long
  files.

## Spec

1. **Session id, red first** — give `prompt_status` a `session_id: &'a str`
   parameter but keep building the state with `""`. Pass the turn's id at
   every call site (`session_id.unwrap_or("")`). Write
   `prompt_status_names_the_session` in `stream_ask.rs`'s tests:
   `prompt_status(.., "0123456789abcdef", ..)` gives a state whose
   `session_id` is `"0123456789abcdef"`. Run it, keep the failing output, then
   use the parameter. The test passes.
2. **Turn seed, red first** — add `resumed_turn` returning 0 always, and write
   `a_resumed_chat_seeds_its_turn_from_the_daemon`: a `LimitsInfo` with
   `turn_count: 7` gives 7, and an `Err` and a `Response::Error` each give 0.
   Run it, keep the failing output, then implement it. The test passes.
3. **Wire the seed** — on a resume only, `run_chat_inner` sends one
   `QueryLimits` for the session id through the existing one-shot request
   helper, passes the reply through `resumed_turn`, and hands the number to
   `run_chat_ratatui` through `RatatuiCtx`.
4. **Mark M29** — append ` **Fixed in M35 phase-05.**` to the end of the O6
   item's text in `docs/dev/milestones/M29-post-2.0-fixes/README.md`.
5. **Red runs** — one `### Update — <date> (red runs)` entry pasting steps 1
   and 2's `test result:` lines and assertion messages.
6. **Findings** — one `### Findings — <date>` entry from `git diff --stat`
   alone, two or three lines, or `none`.
7. **End-to-end** — run the block once before committing; paste its output
   fenced in an `### Update — <date> (end-to-end verification)` entry. This
   step is required. The reviewer checks for the entry.
8. **Commit** as `fix: the status bar names the session under a prompt; a resumed chat counts on (M29 O6)`.

## Acceptance criteria

- [ ] The E2E block's first line prints
      `blank_session=0 seed_fn=1 new_tests=2 o6_fixed=1`
      (today `blank_session=1 seed_fn=0 new_tests=0 o6_fixed=0`).
- [ ] The red-runs entry shows both new tests failing before their fixes.
- [ ] hygiene, doc_truth and bug_tracker pass; `build_warnings=0`; clippy
      clean; `fmt_exit=0`.
- [ ] `cargo test` reports exactly `passed=2628 failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
echo "blank_session=$(grep -c 'session_id: "",' src/cli/commands/stream_ask.rs) seed_fn=$(grep -c 'fn resumed_turn' src/cli/commands/chat_loop.rs) new_tests=$(( $(grep -c 'fn prompt_status_names_the_session' src/cli/commands/stream_ask.rs) + $(grep -c 'fn a_resumed_chat_seeds_its_turn_from_the_daemon' src/cli/commands/chat_loop.rs) )) o6_fixed=$(grep -c 'Fixed in M35 phase-05' docs/dev/milestones/M29-post-2.0-fixes/README.md)"
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- Edit `src/cli/commands/stream_ask.rs`, `chat_loop.rs`, `chat.rs` (the
  resume seed and `RatatuiCtx`), the O6 item of the M29 README, and
  `tests/hygiene_allowlist.txt` (only per the rule above).

## Out of scope

- `/session load`: it loads history into the current session id and does not
  switch sessions, so its turn label is a separate question.
- The `daemon_up` and `tools_total` fields of `prompt_status`; the status bar
  does not draw them.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-31 14:24 (started)

Executor: Claude (Opus 4.6). Starting phase 05: `prompt_status` takes the turn's
session id, `resumed_turn` seeds the turn counter from the daemon's
`LimitsInfo`, wired through `RatatuiCtx`. Red-first on both tests.

### Update — 2026-10-31 14:41 (red runs)

Both new tests failed on the pre-fix code, each captured with the fix
mechanically reverted and the test kept:

```
thread 'cli::commands::stream::ask::tests::prompt_status_names_the_session' panicked at src/cli/commands/stream_ask.rs:716:9:
assertion `left == right` failed
  left: ""
  right: "0123456789abcdef"
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2548 filtered out; finished in 0.00s
```

```
thread 'cli::commands::chat::chat_loop::tests::a_resumed_chat_seeds_its_turn_from_the_daemon' panicked at src/cli/commands/chat_loop.rs:738:9:
  left: 0
  right: 7
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2548 filtered out; finished in 0.00s
```

Both pass after the fix (`test result: ok. 1 passed; …` each).

### Findings — 2026-10-31

From `git diff --stat` alone: no file moved and no allowlist line grew; the
phase touched only the three named sources and three docs. none.

### Update — 2026-10-31 14:52 (end-to-end verification)

The phase's E2E block, run once against the working tree before commit; output
redirected to a file and pasted verbatim:

```
blank_session=0 seed_fn=1 new_tests=2 o6_fixed=1
build_warnings=0
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.79s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.43s
fmt_exit=0
passed=2628 failed=0
```

First line matches the acceptance criterion (`blank_session=0 seed_fn=1
new_tests=2 o6_fixed=1`); hygiene, doc_truth and bug_tracker pass; clippy clean
(no diagnostics, `Finished` only); `fmt_exit=0`; `cargo test` reports exactly
`passed=2628 failed=0`.

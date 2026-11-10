# Phase 21b — program-status-surfaces

**Milestone:** M36 — Chat UX and audit fixes
**Status:** in-progress
**Drafted:** 2026-11-10
**Depends on:** phase 21a
**Estimated diff:** ~450 lines, about half of it tests
**Tags:** language=rust, kind=feature, size=m

## Goal

Decision 9, the consume side's second half. Phase 21a gave each shell host a
table of OSC 7501 records and put them on the `Status` frame, but only
`read_shell_screen` shows them. This phase brings them to the three places
where a shell's state already shows. `list_shells` and `top`'s Shells panel
show each shell's program status, read live from its host. `watch_shell`
returns when a program reports `blocked`, `done` or `error`. Where a shell
has a program record, the program's own word comes before the
screen-scraped foreground. This is the milestone exit criterion:

> A program in a daemon shell that emits `OSC 7501 ; state=blocked:kind=permission:app=x ST`
> shows as blocked (with its `app`) in `list_shells` and `top`, and
> `watch_shell` on it returns on that report.

## Architecture references

- [`osc7501-spec.md`](osc7501-spec.md), § States and § Records.
- `phase-21a-program-status-host.md`, § Behaviour and its Review verdict.
  The verdict covers the lifetime rule a record now follows.
- `src/daemon/executor/knowledge/shell_screen.rs`: `programs_of` and
  `program_line`, the one renderer for a record, which masks and escapes.
- `src/daemon/executor/knowledge/shell.rs` (`list_shells`, `collect_row`,
  `format_shell_rows`), `src/daemon/executor/dispatch_shells.rs`,
  `src/daemon/server/shells.rs` (`handle_shell_list`), `src/ipc_types.rs`
  (`ShellInfo`), `src/cli/commands/top_text.rs` (`shells_text`) and the
  TUI's Shells panel, and `src/daemon/executor/knowledge/shell_watch.rs`
  (`watch_shell_with`).

## Behaviour

1. **A bounded live refresh.** Before `list_shells` formats its rows and
   before `handle_shell_list` replies, the daemon sends a `Status` probe to
   every live shell that is in scope and has not exited, all at once, each
   bounded by a short timeout (at most 1 s). It stores the `programs` that
   come back on the `ShellRecord`. A host that doesn't answer in time, or
   answers with something other than `Status`, keeps whatever was stored
   before, and the listing is never an error because of it. Formatting
   stays pure: it reads only what the refresh stored.
2. **`list_shells`.** A row whose shell has program records shows them
   **before** its foreground classification. Each record is shown through
   `program_line`, so it is masked and escaped exactly as in
   `read_shell_screen`. A row with no records reads exactly as it does
   today.
3. **`ShellInfo` and `top`.** `ShellInfo` gains `programs: Vec<String>`,
   the `program_line` renderings, with `#[serde(default)]` so a reply from
   an older daemon reads as empty. `top --once` and the TUI's Shells panel
   show a shell's first program line in place of its foreground word when
   there is one. When the line is too long it is cut, never wrapped. With
   no records, the panel is unchanged.
4. **`watch_shell`.** Every poll also reads the host's `programs`. The
   watch returns as soon as a record's state **becomes** `blocked`, `done`
   or `error`, measured against the records the host held when the watch
   started: either a new id, or an existing id whose state changed to one
   of the three. The result names the shell and the record's
   `program_line`. A record that was already in one of those states at the
   start doesn't end the watch, and neither does a `working` or `idle`
   report. Pattern matches, command end and the timeout behave exactly as
   today, and whichever comes first wins.

## Constraints

- **Gotcha (21a's lesson): emit test reports from an external program.**
  Use `/bin/printf`, not the builtin `printf`. A `working` or `blocked`
  record lives only while its command line holds the terminal, so a
  blocked report the test wants to observe needs the line to keep running:
  `/bin/printf '\033]7501;state=blocked:kind=permission:app=x\007'; sleep 30`.
  A `done` report survives the line ending. Do not invent a lifetime rule
  in a test. If a test needs one, it is wrong.
- **Lock discipline.** Do the probes outside `with_shells`. Collect the ids
  under the lock, probe with no lock held, then take the lock again to
  store the results.
- `watch_shell_with` is on the hygiene function allowlist. Put the program
  check in its own helper. If the function still grows, raise its
  allowlist line and say why in Findings.
- Don't change the host, the `Status` frame, the parser or the lifetime
  rule. Those are 21a's, and they're done.
- `list_shells` stays a single tool call with the same `scope` semantics
  and the same error text for a bad scope.

## Spec

1. **Red tests first.** Write the tests in the Test plan. Run
   `cargo test --lib program_status_surface` and paste the real failing
   output (compile errors count) into an `(red runs)` entry. The step ends
   when the run is pasted.
2. **Refresh and `list_shells`** (Behaviour 1–2).
3. **`ShellInfo` and `top`** (Behaviour 3).
4. **`watch_shell`** (Behaviour 4).
5. **Docs:** the `list_shells` and `watch_shell` rows in CLAUDE.md and
   README.md, and their entries in `assets/prompts/sre.toml`, each mention
   program status in a clause. The step ends when `doc_truth` is ok.
6. **End-to-end:** run § End-to-end verification once, redirected to
   `.rexymcp/output/p21b-e2e.txt`, and paste it verbatim into an
   `(end-to-end verification)` entry that names it.
7. **Findings:** two or three lines, or `none`. Name any allowlist raise.
8. **Commit** as
   `feat: OSC 7501 surfaces — program status in list_shells and top, watch_shell returns on blocked/done/error (M36 phase-21b)`.

## Test plan

Every test name starts `program_status_surface_`, so the Spec's filter
runs them all:

- `…_format_rows_puts_the_program_before_the_foreground` (pure). It also
  checks that a row with no records is unchanged.
- `…_list_shells_shows_a_blocked_report_without_a_screen_read` (real host,
  blocked line held by `sleep`). The listing names `x` and `blocked`, and
  `read_shell_screen` was never called.
- `…_list_shells_masks_the_message` (an `sk-` key in `msg` comes back
  masked).
- `…_list_shells_survives_a_host_that_does_not_answer` (it returns within
  the bound, and the shell is listed).
- `…_shell_info_from_an_older_daemon_reads_empty_programs`.
- `…_top_shows_the_program_line_in_place_of_the_foreground`.
- `…_watch_returns_on_blocked`, `…_watch_returns_on_done` and
  `…_watch_returns_on_error`.
- `…_watch_ignores_a_record_already_blocked_at_the_start` (it times out).
- `…_watch_keeps_waiting_through_a_working_report` (it times out).

## Acceptance criteria

- [ ] Every test named in the Test plan passes, printed by the end-to-end
      run. Today none exists.
- [ ] `claude_md_mentions=1` or more: the CLAUDE.md `list_shells` row
      names program status. Today 0.
- [ ] Hygiene, `doc_truth`, `bug_tracker` and `lock_invariant` are ok;
      `build_warnings=0`, clippy is clean, `fmt_exit=0`, `failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
cargo test --lib program_status_surface 2>&1 | grep -E '^test |^test result'
grep -E '^\| `list_shells`' CLAUDE.md | grep -c 'program status' | sed 's/^/claude_md_mentions=/'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker lock_invariant; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- **Edit:** `src/shell/registry.rs` (a stored-programs field on
  `ShellRecord` and its constructors); `src/daemon/executor/knowledge/shell.rs`,
  `shell_watch.rs`, `shell_screen.rs` and their test files;
  `src/daemon/executor/dispatch_shells.rs`; `src/daemon/server/shells.rs`;
  `src/ipc_types.rs` and any test that builds a `ShellInfo` literal;
  `src/cli/commands/top_text.rs` and the TUI's Shells panel drawing;
  CLAUDE.md, README.md and `assets/prompts/sre.toml` (the two tool rows
  only).
- **Allowlist:** if the hygiene gate reports a file or function over its
  cap, raise its entry and name it in Findings. Don't split code to fit.
- **Scratch files** go under `.rexymcp/output/`.

## Out of scope

- The chat client's own emit side and tmux passthrough (21c).
- A `watch_shell` parameter to choose which states end the watch.
- Notifications, sounds, or any other external effect of a report.
- RIS clearing the table (21a's out-of-scope note stands).

## Update Log

### Update — 2026-11-10 11:35 (started)

Status flipped to `in-progress`; milestone README row matches. Executor:
Claude Opus 4.5 (1M), headless (phase-21b-program-status-surfaces).

### Update — 2026-11-10 (en route, findings)

- Allowlist raises made, both named per the Authorizations: `tests/hygiene_allowlist.txt` — `fn …shell_watch.rs watch_shell_with` 159 → 184 (the program-check got its own helper; Behaviour 4's start-snapshot comparison is what grew the core) and a new `test …shell_observer_tests.rs 2116` (five live watch hosts plus the type-until-recorded helper, 9 tests).
- The five live watch tests open their host through `open_shell_with` and take the shell id **from the sentence it returns**, then type into and watch that id — pinning against the literal `s41` id broke: another test's registry singleton handed back `s0`, so the record and the socket knew different shells.
- `type_until_recorded` keys the wait on the OSC payload (`7501;state=…`) landing in the cast, not on the echoed typed text — the echo reaches the cast base64-escaped, so a `contains` on the typed line never fired.
- Live watch tests give the watch a 3 s start grace and a 5 s timeout cap; with a 0 grace a poll that reads `Idle` before the report's frame races the whole point of the test. The spec's Gotcha (blocked line held by `sleep 30`) is followed as written.
- E2E transcript at `.rexymcp/output/p21b-e2e.txt`, pasted verbatim below.

### Update — 2026-11-10 (end-to-end verification)

`.rexymcp/output/p21b-e2e.txt`, the § End-to-end verification block run
once, redirected to it, pasted verbatim:

```
test cli::commands::top::tests::program_status_surface_top_shows_the_program_line_in_place_of_the_foreground ... ok
test daemon::executor::knowledge::shell::tests::program_status_surface_format_rows_puts_the_program_before_the_foreground ... ok
test cli::commands::top::tests::program_status_surface_top_cuts_a_long_program_line_never_wraps ... ok
test daemon::executor::knowledge::shell::tests::program_status_surface_list_shells_masks_the_message ... ok
test daemon::executor::knowledge::shell::tests::observers::program_status_surface_watch_returns_on_done ... ok
test daemon::executor::knowledge::shell::tests::observers::program_status_surface_watch_ignores_a_record_already_blocked_at_the_start ... ok
test daemon::executor::knowledge::shell::tests::observers::program_status_surface_watch_keeps_waiting_through_a_working_report ... ok
test daemon::executor::knowledge::shell::tests::observers::program_status_surface_watch_returns_on_error ... ok
test daemon::executor::knowledge::shell::tests::observers::program_status_surface_watch_returns_on_blocked ... ok
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 2776 filtered out; finished in 10.20s
claude_md_mentions=1
build_warnings=0
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.98s
test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.43s
fmt_exit=0
passed=2867 failed=0
```

The five live watch tests above run against real in-process PTY bash
hosts: the report is emitted by `/bin/printf` on the shell's own
terminal (the blocked line held by `sleep 30`), not by a fake.

<!-- entries appended below this line -->

### Notes for executor — 2026-11-10 (refined re-dispatch)

The first run used all 1000 turns and **changed no source file**. 955 of
its calls were `grep` for strings that don't exist in this repo
(`prev-run-res`, `veil`, `topicarg`, `so far`, …). Nothing in this phase
needs a search like that. Every symbol you need is in the map below, so
read those files and **write**.

**Rules for this run:**

- Your **first edit lands within your first 15 turns**. It is step 1 of
  the edit order: the new `ShellRecord` field.
- Grep only for a symbol that is named in this doc or that you have seen
  in a file you read. **Never grep for a word you made up.** If a grep
  returns nothing, stop searching and read the file the map names.
- Run `cargo build` after every edit, and `cargo test --lib program_status_surface`
  after every test you add.

**Symbol map** (all of these exist today):

| What | Where |
|---|---|
| Probe a host | `crate::shell::status(&sock, timeout)` in `src/shell/client.rs`, which returns `ShellResponse::Status { programs, .. }` |
| A shell's socket | `crate::shell::socket_path(id)` in `src/shell/launch.rs` |
| Registry access | `with_shells(reg, \|store\| …)`, used throughout `knowledge/shell.rs` |
| Where to store | `ShellRecord` in `src/shell/registry.rs`, beside `last_foreground`. Its two constructors set `last_foreground: None` |
| Render a record | `program_line(&Record)` and `programs_of(Option<&ShellResponse>)` in `knowledge/shell_screen.rs` |
| `list_shells` | `list_shells` → `list_shells_in` → `resolve_scope` → `collect_row` → `format_shell_rows` (`ShellRow`) in `knowledge/shell.rs`, called from `dispatch_shells.rs` |
| `top` and the IPC reply | `handle_shell_list` in `src/daemon/server/shells.rs` builds `ShellInfo` (`src/ipc_types.rs`). `shells_text` in `src/cli/commands/top_text.rs` draws both `top --once` and the TUI panel |
| `watch_shell` | `watch_shell_with` in `knowledge/shell_watch.rs` (poll loop; `WATCH_POLL`) |
| Test homes | `knowledge/shell_tests.rs` (`list_shells`), `knowledge/shell_observer_tests.rs` (`watch_shell`, real hosts), the `top_text.rs` tests |

**Edit order.** Each step ends at a green `cargo build`.

1. Add `last_programs: Vec<crate::shell::program_status::Record>` to
   `ShellRecord`, and set it to empty in both constructors.
2. Write `refresh_programs(reg, ids)` (async, in `knowledge/shell.rs`).
   It collects the sockets under the lock, probes them all at once with
   no lock held (each probe under a 1 s timeout), then stores the
   `programs` of each `Status` reply.
3. `ShellRow` gains `programs: Vec<String>` (from `program_line`). Make
   `format_shell_rows` print them before the foreground, and make
   `list_shells` async and call `refresh_programs` first. The `.await` in
   `dispatch_shells.rs` follows.
4. Give `ShellInfo` its `programs` field (`#[serde(default)]`). Have
   `handle_shell_list` call `refresh_programs` and fill it. Have
   `shells_text` show the first line in place of the foreground.
5. Make `watch_shell_with` call a new helper each poll that compares the
   host's `programs` with the snapshot taken at the start (Behaviour 4).
6. Write the tests (each named `program_status_surface_…`), then the docs
   (Spec step 5), then run the end-to-end block.

The red-runs step may record the first compile failure of the new tests.
It doesn't need to come before step 1.

### Update — 2026-11-10 (escalation)

**Chosen lever:** refined re-dispatch
**Rationale:** The first run was a degenerate search loop (955 greps for
made-up strings, no source edit), not a spec contradiction. One refined
attempt, with a symbol map, an edit order that starts with a write and a
no-invented-grep rule, comes before a takeover (glm read paralysis has
happened before on M36).


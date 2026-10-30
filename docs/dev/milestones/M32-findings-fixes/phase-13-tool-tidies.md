# Phase 13 — Tool tidies

**Status:** todo
**Milestone:** M32 — Findings fixes
**Drafted:** 2026-10-30
**Depends on:** phase-12b
**Estimated diff:** ~57 insertions, ~92 deletions
**Tags:** language=rust, kind=refactor, size=s
**Findings:** M31 `FINDINGS.md` #42, #43, #64, #70, #72, #76, #81

## Goal

Seven small repeats in the shell tools, `daemoneye top` and two executor
arms. After this phase:

- **#70:** one `shell_tool_regex` in
  `src/daemon/executor/knowledge/shell.rs` compiles every pattern the
  shell tools take, so their shared 1 MiB size limit is written once. Each
  tool keeps its own refusal sentence, because each names its own
  parameter.
- **#72:** `send_input` and `close_shell` refuse an unknown shell id with
  the shared `unknown_shell_error`, as the other shell tools already do.
  Their two drifted copies of that refusal go, and so does
  `watch_shell_unknown_error`, a one-line alias of it. The two tests of
  that refusal now assert the shared wording byte for byte.
- **#42, #43:** `top`'s `window_text` takes its lines in one pass instead
  of a `lines().nth(i)` per line, and its alias `activity_window_text`
  goes.
- **#64:** `run_started_at` calls `started_at_of` instead of repeating its
  body.
- **#76:** the three run-log arms in `dispatch_runs.rs` share one
  `with_run_id`, which parses the id or returns the shared rejection.
- **#81:** `load_tools` looks each group up once instead of twice.

The one change a user can see is #72's: an unknown shell id now gets the
same refusal from `send_input` and `close_shell` as from every other
shell tool. The suite stays at **exactly 2610**; a different count means
something else changed.

## Architecture references

- `src/daemon/executor/knowledge/shell_watch.rs` § `unknown_shell_error`,
  whose doc says it exists so the wording cannot drift.
- M32 README, decision 2.

## Spec

1. **Run this, once, as your first command after the started entry:**

   ```sh
   python3 docs/dev/milestones/M32-findings-fixes/scripts/phase-13-tool-tidies.py && cargo fmt --all
   ```

   **Do not open the script, and do not read any source file before
   running it.** It was run on a scratch worktree of `4fcc9402`: fmt and
   clippy clean, hygiene passing, the full suite at
   `passed=2610 failed=0`. It asserts its own anchors and prints seven
   lines, the last `the two unknown-id tests pin the shared wording`. If
   an assert fires, stop and report it in the Update Log. Do not
   hand-edit anything it touches, and do not edit the script.
2. **Log it at once.** Append an `### Update — <date> (cuts done)` entry
   quoting the script's output, with an edit tool.
3. **Findings** go in their own `### Findings — <date>` entry. Run
   exactly `git diff --stat`, write what it shows in one or two lines, or
   write `none`. Open no file.
4. **Run the End-to-end block** and paste its output into an
   `### Update — <date> (end-to-end verification)` entry, as a fenced
   block of the output itself, not a description of it.
5. **Commit** as `refactor:`.

The script is
`docs/dev/milestones/M32-findings-fixes/scripts/phase-13-tool-tidies.py`
(sha256 begins `452777ea8631ae94`), kept in the repo as the record of the change.

### The red run is done — do not repeat it

The two strengthened tests were run against the pre-phase code at
drafting. Both failed, showing the drifted wording against the shared
one:

```
  left: "Error: no shell with id \"s99\" is in the registry (no shells are live — call list_shells to confirm)."
 right: "Error: no shell with id \"s99\" is in the registry. No shells are live; call list_shells to confirm, or run a command first — …"
  left: "Error: no shell with id \"s99\" is in the registry. Live shell ids: s5. Call list_shells to refresh."
 right: "Error: no shell with id \"s99\" is in the registry — it may have exited and aged out, or the listing you are working from is stale. Live shell ids: s5. …"
test result: FAILED. 0 passed; 2 failed
```

With the change applied, both pass.

## Acceptance criteria

- [ ] The End-to-end block prints
      `regex_builders=1 drifted=0 aliases=0 nth=0 started_copies=0 id_parses=1 group_lookups=1 wording_pins=2`
      (today: `regex_builders=6 drifted=2 aliases=2 nth=1
      started_copies=1 id_parses=3 group_lookups=2 wording_pins=0`).
- [ ] `cargo test` reports **exactly** `passed=2610 failed=0`, the same
      count as before the phase.
- [ ] Preservation: `tests/hygiene.rs` and `cargo test --test bug_tracker`
      pass; `tests/hygiene_allowlist.txt` is unchanged. fmt, build and
      clippy are clean.
- [ ] `grep -c '^### Findings — '` on this doc prints `1`, and the
      end-to-end entry holds the fenced output.

## End-to-end verification

```sh
bash <<'E2E'
K=src/daemon/executor/knowledge
echo "regex_builders=$(cat $K/shell.rs $K/shell_read.rs $K/shell_screen.rs $K/shell_watch.rs | grep -c 'size_limit(1 << 20)') drifted=$(cat $K/shell_input.rs $K/shell_life.rs | grep -c 'let live = live_shell_ids(reg);') aliases=$(grep -c 'fn activity_window_text\|fn watch_shell_unknown_error' src/cli/commands/top_tui_draw.rs $K/shell_watch.rs | awk -F: '{s+=$2} END {print s}') nth=$(grep -c 'lines().nth(i)' src/cli/commands/top_tui_draw.rs) started_copies=$(grep -c 'TurnLogEvent::RunStarted' src/cli/commands/top_load.rs) id_parses=$(grep -c 'RunId::parse(run_id)' src/daemon/executor/dispatch_runs.rs) group_lookups=$(grep -c 'tools_in_group(g)' src/daemon/executor/dispatch_meta.rs) wording_pins=$(cat $K/shell_observer_tests.rs $K/shell_lifecycle_tests.rs | grep -c 'words the refusal as every shell tool does')"
git diff --quiet HEAD -- tests/hygiene_allowlist.txt && echo allowlist=unchanged
cargo test --test hygiene 2>&1 | grep -E '^test result'
cargo test --test bug_tracker 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

Expected: the shape line above, `allowlist=unchanged`, hygiene
`ok. 16 passed`, bug tracker `ok. 6 passed`, clippy clean, `fmt_exit=0`,
`passed=2610 failed=0`. If the suite stops early on a failure, paste that
output, run the block once more and paste both.

## Authorizations

- Run `scripts/phase-13-tool-tidies.py`; do not edit it.
- Edit the thirteen files it names, through the script only: the five
  shell tool files and `knowledge/shell.rs`, the two shell test files,
  `top_tui_draw.rs`, `top_load.rs`, `top_detail.rs`, `dispatch_runs.rs`
  and `dispatch_meta.rs`.

## Out of scope

- `read_file`'s filter in `file_ops/read.rs`, which builds its regex with
  the same size limit but belongs to the file tools, not the shell tools.
- Each shell tool's refusal sentence for an invalid pattern, which names
  its own parameter and stays as it is.

## Update Log

<!-- entries appended below this line -->

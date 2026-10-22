# Phase 08: top-shells — the Shells panel of `daemoneye top`

**Milestone:** M30 — Telemetry & dashboard
**Status:** review
**Depends on:** phase-07
**Estimated diff:** ~250 lines, about half of it tests
**Tags:** language=rust, kind=feature, size=s

## Goal

`daemoneye top --once` shows Runs, Budget and Activity from disk. This
phase adds the fourth panel, Shells: one row per live PTY shell with its
id, owner, what holds its foreground and its last line of output. The rows
come from the daemon's existing read-only `Request::ShellList`. When the
daemon is down the panel says `daemon not running`, and the rest of the
frame prints as before.

## Spec

**Files:** `src/ipc.rs`, `src/daemon/server/shells.rs`,
`src/daemon/executor/knowledge/shell.rs` (one visibility change),
`src/daemon/executor/knowledge/mod.rs` (one re-export),
`src/cli/commands/top.rs`, `src/main.rs`, `README.md`, and the three
test helpers that build a `ShellInfo` literal: `src/ipc_tests.rs`,
`src/cli/commands/slash.rs` (`fn shell`) and `src/cli/commands/agents.rs`
(`fn shell`). Each of those gains `foreground: None, last_line: None` and
nothing else.

**Where things are.** You do not need to search for these. Read the named
symbol, then write.

| Need | Symbol |
|---|---|
| The wire row | `ShellInfo` in `src/ipc.rs` |
| Where rows are filled | `handle_shell_list` and its helper `last_command` in `src/daemon/server/shells.rs` |
| Foreground facts on a record | `ShellRecord.last_foreground: Option<(Foreground, u64)>` and `ShellRecord.in_flight: Option<InFlight { cmd, started_unix }>` in `src/shell/registry.rs` |
| Foreground wording | `impl Display for Foreground` in `src/shell/foreground.rs` (`shell prompt`, `running <cmd>`, …) |
| A shell's recording | `crate::shell::cast_path(id, started_unix)`, `meta_path_for(&cast)`, `MetaIndex::load`, `meta.commands` (`CommandRecord.index`), `crate::shell::read_command_output(&cast, &meta, index)` |
| Raw PTY bytes → readable text | `fn visible_text(raw: &str) -> String` in `src/daemon/executor/knowledge/shell.rs`. Its module is private (`knowledge/mod.rs` says `mod shell;`), so `pub(crate)` alone does not reach it. Make the fn `pub(crate)` **and** add `pub(crate) use shell::visible_text;` to `knowledge/mod.rs`. Call it as `crate::daemon::executor::knowledge::visible_text`. `read_command_output` returns `Vec<u8>`: pass `&String::from_utf8_lossy(&bytes)` |
| Masking | `crate::ai::filter::mask_sensitive(&str) -> String`. The daemon already calls `init_masking` at startup in `src/daemon/mod.rs`, so the handler does nothing extra. A test calls `crate::ai::filter::init_masking(&[])` once before asserting. The generic `api_key=` rule masks the test's key |
| Every `ShellInfo` literal | `src/daemon/server/shells.rs` (two, in `handle_shell_list`), `src/ipc_tests.rs`, `src/cli/commands/slash.rs`, `src/cli/commands/agents.rs`. Grep `ShellInfo {` and stop there |
| The IPC call | `ipc_client::request_once(Request::ShellList)` (async; `Err` when no daemon answers) |
| The snapshot and text | `TopSnapshot`, `take_snapshot`, `render_once` in `src/cli/commands/top.rs` |

1. **Two new `ShellInfo` fields**, both `Option<String>` and
   `#[serde(default)]`, so a reply from an older daemon still parses.
   - `foreground` is the `Display` of the record's `last_foreground`. When
     `in_flight` started later than that observation, or there is no
     observation, it is `running <in_flight.cmd>`. With neither it is
     `None`.
   - `last_line` is the last non-empty line of the shell's latest recorded
     command output. It is made readable with `visible_text`, masked with
     `mask_sensitive`, and cut to 120 characters. It is `None` when the
     shell has no recorded command or the read fails. A read failure is
     not an error.
   - Both are filled in `handle_shell_list`. The record fields are copied
     under the lock and the file reads happen outside it, as
     `last_command` does today.
   - The pure functions, both in `src/daemon/server/shells.rs` beside
     `last_command`, tested in that file's test module:
     - `fn shell_foreground(last_foreground: Option<&(Foreground, u64)>, in_flight: Option<&InFlight>) -> Option<String>`.
       `in_flight` wins only when its `started_unix` is strictly greater
       than the observation's timestamp; on a tie the observation wins.
       Called inside the `with_shells` closure, which is allowed: it is a
       field read and a `Display`, no I/O.
     - `fn last_visible_line(raw: &[u8]) -> Option<String>`: lossy UTF-8,
       `visible_text`, last line whose trim is non-empty, `mask_sensitive`,
       first 120 `char`s (not bytes). Empty or all-blank input gives `None`.
   - The impure reader `fn last_line(id: &str, started_unix: u64) -> Option<String>`
     mirrors `last_command`: load the meta, take `meta.commands.last()`,
     `read_command_output(&cast, &meta, record.index).ok()`, then
     `last_visible_line`. Fill it in the second `.map` of
     `handle_shell_list`, next to `last_command`. It needs no test of its
     own.
2. **The Shells panel.** `TopSnapshot` gains `shells`, which holds either
   the rows or `daemon_down`.
   - `take_snapshot` becomes async and sends one
     `Request::ShellList`.
   - An `Err` from it, or any reply that is not `ShellList`, means
     `daemon_down`.
   - `run_top` becomes async, and `Commands::Top` in `main.rs` awaits it.
3. **Text.** `render_once` prints the four sections in the order Runs,
   Budget, Shells, Activity, each headed by its name on its own line.
   - The Shells section lists one row per shell: id, owner, foreground
     (`-` when `None`), and last line (`-` when `None`).
   - It prints `daemon not running` when the daemon is down, and `none`
     when it is up with no shells.
   - Build the section in a pure function `fn shells_text(panel: &ShellsPanel) -> String`
     in `top.rs`, where `ShellsPanel` is `DaemonDown` or `Rows(Vec<ShellInfo>)`.
     The two `shells_text_*` tests call it directly with no daemon.
   - Update the README's `top --once` paragraph to name the Shells
     section.
4. **Still read-only.** `ShellList` is the only request top sends.
   Gotcha: `handle_shell_list` runs inside the daemon, so its file reads
   are the daemon's, not top's. Top itself still writes nothing.

**Work order.** Write the pure pieces and their tests first: the
foreground choice, the last-line extraction and the Shells text. Then add
the `ShellInfo` fields and the handler, then the async snapshot and
wiring.

## Tests

- `shell_info_without_the_new_fields_still_parses`, in `src/ipc_tests.rs`
  or next to `ShellInfo`. A JSON row with no `foreground` or `last_line`
  key deserializes to `None` for both.
- `foreground_prefers_the_newer_observation`. Three cases: an observation
  newer than `in_flight` gives its `Display`; an `in_flight` newer than
  the observation gives `running <cmd>`; neither gives `None`.
- `last_line_is_the_last_visible_non_empty_line`. The input is raw bytes
  with an ANSI colour sequence, a carriage-return overwrite and trailing
  blank lines, and the result is the readable last line. A line holding
  `api_key=sk-abcdefghijklmnopqrstuvwxyz0123` comes back without that key.
  An empty recording gives `None`.
- `shells_text_says_daemon_not_running`.
- `shells_text_lists_each_shell`, with two rows, one with `None` for both
  new fields printed as `-`.

## Acceptance criteria

- [ ] Each new test ⇒ `test result: ok. 1 passed`.
- [ ] On the § End-to-end fixture, with no daemon:
  - `shells_header=1` and `shells_down=1`. Both read `0` today.
  - `section_order=Runs,Budget,Shells,Activity`. Today it reads
    `Runs,Budget,Activity`.
  - `once_exit=0` and `budget_usd=2` are unchanged (preservation). The
    `2` is the Budget spend line plus the Activity `ai_cost` row, which
    prints `$0.5000`; it read `2` before this phase too. (Corrected at
    escalation 2: the draft said `1`, which no tree ever produced.)
- [ ] `visible_text` is `pub(crate)` and reused: `visible_text_callers` ≥ 2
      across `src/` (it is `1` file today).
- [ ] `cargo test` ⇒ `failed=0`, with `passed` above 2492 by the new
      tests. Report the number; do not re-run to chase it.
- [ ] Clippy (`-D warnings`) and `cargo fmt --all --check` exit 0. No new
      `#[allow]`, no production `unwrap`/`expect`.
- [ ] The end-to-end Update Log entry holds the output fence **first**,
      then the `PASTE MATCH` line.

## End-to-end verification

```sh
A=/tmp/e2e-m30-08.txt; : > "$A"
for t in shell_info_without_the_new_fields_still_parses foreground_prefers_the_newer_observation last_line_is_the_last_visible_non_empty_line shells_text_says_daemon_not_running shells_text_lists_each_shell; do cargo test "$t" 2>&1 | grep -E '^test result: ok. 1 passed' | head -1 | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
cargo build -q 2>/dev/null
H=$(mktemp -d); mkdir -p "$H/.daemoneye/var/log/events"
S="$H/.daemoneye/var/log/events/events-$(date +%Y%m%d).jsonl"; T=$(date -u +%Y-%m-%dT%H:%M:%SZ)
echo "{\"ts\":\"$T\",\"event\":\"ai_cost\",\"pid\":1,\"agent_name\":\"chat\",\"model\":\"m\",\"cost\":{\"total_cost_usd\":0.50}}" >> "$S"
O=$(HOME="$H" ./target/debug/daemoneye top --once); echo "once_exit=$?" >> "$A"
echo "budget_usd=$(echo "$O" | grep -c '\$0\.50')" >> "$A"
echo "shells_header=$(echo "$O" | grep -cx 'Shells')" >> "$A"
echo "shells_down=$(echo "$O" | grep -c 'daemon not running')" >> "$A"
echo "section_order=$(echo "$O" | grep -xE 'Runs|Budget|Shells|Activity' | paste -sd, -)" >> "$A"
find "$H" -type f -delete
echo "visible_text_callers=$(grep -rl 'visible_text(' src --include=*.rs | wc -l)" >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Then run the self-check, and put its line in the entry after the output
fence:

```sh
D=docs/dev/milestones/M30-telemetry-dashboard/phase-08-top-shells.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m30-08.txt
diff /tmp/pasted-m30-08.txt /tmp/e2e-m30-08.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

- Changing `visible_text` in `src/daemon/executor/knowledge/shell.rs` from
  private to `pub(crate)`. That is the only edit to that file.
- Adding `pub(crate) use shell::visible_text;` to
  `src/daemon/executor/knowledge/mod.rs`. That is the only edit to that
  file.
- Adding `foreground: None, last_line: None` to the `ShellInfo` literals
  in `src/ipc_tests.rs`, `src/cli/commands/slash.rs` and
  `src/cli/commands/agents.rs`. No other edit to those files.

## Out of scope

- The ratatui screen, the poll, follow and scroll (phase 09).
- Any change to `list_shells`, `/shells` or `read_shell`. They may ignore
  the new fields.
- Probing a shell's host live for its foreground. Top shows the last
  observation the daemon already holds.

## Update Log

<!-- entries appended below this line -->

### Notes for executor — 2026-10-22

The first dispatch read for 120 turns and wrote nothing. Its last forty
reads chased four questions the Spec now answers. (1) `visible_text`
cannot be reached through `pub(crate)` alone, because `knowledge/mod.rs`
declares `mod shell;` privately. The re-export is now named and
authorized. (2) Where masking is initialized: the daemon already does it
at startup, and tests call `init_masking(&[])`. (3) Which `ShellInfo`
literals break when the fields are added: five sites, all named, three
of them test helpers now authorized. (4) The shape of the pure functions:
their names, signatures and home are pinned in Spec item 1 and 3. **Do
not re-survey.** Every symbol you need is in the "Where things are"
table. Start writing at turn 1: `shell_foreground` and
`last_visible_line` with their tests in `src/daemon/server/shells.rs`,
then run `cargo test shell_foreground` before anything else.

### Update — 2026-10-22 (escalation)

**Chosen lever:** refined re-dispatch
**Rationale:** the stall traced to a real spec gap (an unreachable
private module behind the one authorized visibility change, plus unnamed
literal sites), which a tighter spec fixes; this is phase 08's first
failure, so takeover is not yet warranted.

### Update — 2026-10-22 09:50 (progress)

Started per the Notes-for-executor order: `shell_foreground`,
`last_visible_line` and their tests in `src/daemon/server/shells.rs` first
(`foreground_prefers_the_newer_observation` green by turn ~15), then the
two `ShellInfo` fields and the five literal sites, then `shells_text` and
the async snapshot. One deviation from the first attempt at the masking
test: the secret line had to **be** the last line — the function reads the
last line only, so a secret above `more` is not the thing it should mask.
All five new tests green; `cargo test` full suite passed=2497 failed=0.

### Update — 2026-10-22 10:10 (end-to-end verification)

Ran the phase doc's § End-to-end verification script verbatim against the
built binary. `budget_usd` reads `2`, not the `1 (preservation)` the
criteria wrote: the fixture's Activity line prints the cost to four places
(`chat m $0.5000`, the `ai_cost` arm since phase 07), and `grep -c '$0\.50'`
matches that line as well as the Budget spend line. I confirmed this is the
fixture's value, not a regression, by building `aaf3be7d^` — the tree the
`budget_usd=1` preservation note was written against — in a scratch clone
and running the same fixture: `budget_usd=2` there too. On phase-07's own
fixture (two `0.25` events, Activity lines `$0.2500`) the grep matches the
spend line only, which is where the `1` came from. Spend is still counted
once (`spend: $0.50`); no code change made or needed.

```
shell_info_without_the_new_fields_still_parses: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
foreground_prefers_the_newer_observation: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
last_line_is_the_last_visible_non_empty_line: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
shells_text_says_daemon_not_running: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
shells_text_lists_each_shell: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
once_exit=0
budget_usd=2
shells_header=1
shells_down=1
section_order=Runs,Budget,Shells,Activity
visible_text_callers=2
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2497 failed=0
```

PASTE MATCH

### Update — 2026-10-22 (escalation 2)

**Chosen lever:** resume
**Rationale:** the second run finished the phase (all four gates green on
the partial tree: fmt, build, clippy 0; `passed=2497 failed=0`) and then
stalled on the architect's wrong `budget_usd=1` preservation criterion,
which the executor correctly showed reads `2` on the pre-phase tree; the
criterion is corrected above, so only the close-out remains.

### Update — 2026-10-22 03:32 (completion, executor resume)

Resumed per the escalation-2 architect guidance and closed the phase out.
Confirmation run of the full suite: `passed=2497 failed=0` (the five new
tests included; above the 2492 baseline as required). Gates on this tree:
`cargo fmt --all --check` exit 0, `cargo build` clean, `cargo clippy
--all-targets --all-features -- -D warnings` exit 0 (the only `#[allow]`
in a touched file, `ipc.rs:560`, is pre-existing from ba0f6740 — no new
directive). The `visible_text` reuse criterion holds mechanically:
`grep -rl 'visible_text(' src --include=*.rs | wc -l` = 2 (the knowledge
definition file and `src/daemon/server/shells.rs`); the fn is
`pub(crate)` in `src/daemon/executor/knowledge/shell.rs:1523` with the
authorized `pub(crate) use shell::visible_text;` re-export in
`knowledge/mod.rs`. End-to-end evidence stands as captured in the
2026-10-22 10:10 entry above (`PASTE MATCH`): all five named tests ok. 1
passed, `once_exit=0`, `budget_usd=2`, `shells_header=1`,
`shells_down=1`, `section_order=Runs,Budget,Shells,Activity`,
`visible_text_callers=2`, `fmt_exit=0`, `passed=2497 failed=0`. Code
committed as one conventional commit `cb1c8d96` (`feat: add the Shells
panel to top --once …`) covering the ten source/README files; the
start-of-phase status flip was already in this working tree from the
prior run. No deviations from the Spec this round; the one earlier
deviation on record (the masking test's secret must be the last line,
since the function reads the last line only) is already logged at 09:50.
Per the escalation-2 resume guidance the close-out bookkeeping is done
here: Status flipped to `review` in this doc and in the milestone
README's phase-08 row, and the docs committed separately.

# Phase 08: top-shells — the Shells panel of `daemoneye top`

**Milestone:** M30 — Telemetry & dashboard
**Status:** todo
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
`src/cli/commands/top.rs`, `src/main.rs`, `README.md`.

**Where things are.** You do not need to search for these. Read the named
symbol, then write.

| Need | Symbol |
|---|---|
| The wire row | `ShellInfo` in `src/ipc.rs` |
| Where rows are filled | `handle_shell_list` and its helper `last_command` in `src/daemon/server/shells.rs` |
| Foreground facts on a record | `ShellRecord.last_foreground: Option<(Foreground, u64)>` and `ShellRecord.in_flight: Option<InFlight { cmd, started_unix }>` in `src/shell/registry.rs` |
| Foreground wording | `impl Display for Foreground` in `src/shell/foreground.rs` (`shell prompt`, `running <cmd>`, …) |
| A shell's recording | `crate::shell::cast_path(id, started_unix)`, `meta_path_for(&cast)`, `MetaIndex::load`, `meta.commands` (`CommandRecord.index`), `crate::shell::read_command_output(&cast, &meta, index)` |
| Raw PTY bytes → readable text | the private `visible_text` in `src/daemon/executor/knowledge/shell.rs`; make it `pub(crate)` and reuse it |
| Masking | `crate::ai::filter::mask_sensitive` (tests call `init_masking(&[])` first) |
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
   - Build each value in a pure function and test it.
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
   - Build the section in a pure function.
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
  - `once_exit=0` and `budget_usd=1` are unchanged (preservation).
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

## Out of scope

- The ratatui screen, the poll, follow and scroll (phase 09).
- Any change to `list_shells`, `/shells` or `read_shell`. They may ignore
  the new fields.
- Probing a shell's host live for its foreground. Top shows the last
  observation the daemon already holds.

## Update Log

<!-- entries appended below this line -->

# Phase 09: top-tui — the live ratatui screen of `daemoneye top`

**Milestone:** M30 — Telemetry & dashboard
**Status:** todo
**Depends on:** phase-08
**Estimated diff:** ~400 lines, about half of it tests
**Tags:** language=rust, kind=feature, size=m

## Goal

`daemoneye top --once` prints the four panels as text. Plain `daemoneye
top` is still refused with exit 2. This phase makes it the live view: a
full-screen ratatui screen over the same panels. It polls every 500 ms
and re-reads the disk panels only when the fingerprint changes. The
selection follows the newest live run until the user scrolls, `/` edits
the Activity filter, and `q` quits. The screen stays read-only.

## Spec

**Files:** `src/cli/commands/top.rs`, a new `src/cli/commands/top_tui.rs`
(plus `mod top_tui;` in `src/cli/commands/mod.rs`), `src/main.rs` (the
`Top` doc comment only), `README.md`.

**Where things are.** You do not need to search for these.

| Need | Symbol |
|---|---|
| The panels and their text | `TopSnapshot`, `ShellsPanel`, `take_snapshot`, `runs_text`, `budget_text`, `shells_text`, `activity_text`, `filter_activity`, `run_top` in `top.rs` |
| The re-read signal | `fingerprint() -> TopFingerprint` in `top.rs` (stats only, `PartialEq`) |
| Which run is live | `TopRunRow.liveness == Liveness::Live`. The rows are already sorted running first, then newest first |
| Leaving the alt screen on every exit path | `crate::cli::viewer::AltScreenGuard::new(teardown)` (already `pub(crate)`, reachable from `cli::commands`) |
| A ratatui screen tested without a terminal | the tests in `src/cli/attach.rs`: `Terminal::new(TestBackend::new(w, h))`, then `terminal.backend().buffer().content` → `c.symbol()` |
| Terminal and keys (crossterm 0.29) | `terminal::{enable_raw_mode, disable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen}`, `cursor::{Hide, Show}`, `event::{poll, read, Event, KeyEvent, KeyCode, KeyModifiers, KeyEventKind}` |
| Is stdout a terminal | `std::io::IsTerminal` |

1. **Split the snapshot in `top.rs`.** Move the `ShellList` match out of
   `take_snapshot` into `pub async fn shells_panel() -> ShellsPanel`. Move
   the disk reads into `pub fn disk_panels(config_cap: f64) -> DiskPanels`,
   where `DiskPanels` holds `runs`, `budget` and `activity`.
   `take_snapshot` composes the two, and `--once` output does not change.
2. **The model in `top_tui.rs`**, all pure and tested:
   - `TopModel` holds the snapshot, the selected run index, `follow:
     bool` (starts `true`), the filter text, whether the filter is being
     edited, and `quit: bool`.
   - `follow_target(runs) -> usize` returns the index of the first `Live`
     row. With no live row it returns `0`. While `follow` is on, every
     refresh sets the selection to it.
   - `handle_key(&mut self, key)`. Ctrl-C quits in every mode, because
     raw mode swallows SIGINT. When not editing: `q` quits, Up/Down and
     `k`/`j` move the selection (clamped to the rows) and turn `follow`
     off, `f` turns `follow` back on, and `/` starts editing. When
     editing: printable characters append (`q` included), Backspace
     deletes one character, Enter stops editing and keeps the text, and
     Esc clears the text and stops editing. Only `KeyEventKind::Press`
     counts.
   - `plan_refresh(last: Option<TopFingerprint>, now: TopFingerprint,
     since_disk: Duration, since_shells: Duration) -> Refresh { disk,
     shells }`. `disk` is true when there is no last fingerprint, when
     the fingerprint changed, or when 5 s have passed. The 5 s case
     exists because ages and the 30 s liveness window move with the
     clock. `shells` is true once 2 s have passed. The first tick reads
     both.
3. **The screen.** `draw(frame, &TopModel)` draws four bordered blocks
   titled Runs, Budget, Shells and Activity. Each block's body is the
   matching `*_text` output; Activity goes through `filter_activity` and
   shows the newest rows that fit. The selected run's line in the Runs
   body (header line + index) is drawn reversed. A last footer line reads
   `q quit · ↑↓ scroll · f follow · / filter`, then `following` or
   `scrolled`, then `filter: <text>` when a filter is set. Sizes are your
   choice, but the four titles must fit at 80×24.
4. **The loop.** `pub async fn run_tui(config_cap: f64, filter: String) ->
   Result<()>`.
   - Enter raw mode, the alternate screen and a hidden cursor. Install an
     `AltScreenGuard` whose teardown leaves the alternate screen, shows
     the cursor and disables raw mode, so `?` cannot strand the terminal.
   - Keys come from a `std::thread` that loops on `event::poll(100 ms)`
     and `read()`, and sends each event through a
     `tokio::sync::mpsc::unbounded_channel`. It stops when a shared
     `AtomicBool` is set. Gotcha: the crossterm `event-stream` feature is
     **not** enabled; do not add it.
   - `tokio::select!` waits on that channel or a 500 ms
     `tokio::time::interval`. On a tick it calls `fingerprint()` and
     `plan_refresh`, then re-reads only what the plan says. It redraws
     after every key and every tick. When `quit` is set it signals the
     thread and returns `Ok(())`.
5. **`run_top`.** `--once` works as today. Without it, a stdout that is
   not a terminal gets `daemoneye top: not a terminal; use --once` on
   stderr and exit 2. Otherwise it calls `run_tui`, with `--filter`
   seeding the filter. Update the `Top` clap doc comment in `main.rs`,
   the module doc of `top.rs` and the README `top` paragraph, so none of
   them still calls the live view a later release.
6. **Still read-only.** `ShellList` is the only request; nothing is
   written.

**Work order.** `plan_refresh`, `follow_target` and `handle_key`, each
with its tests, then `draw` with its `TestBackend` test, then the snapshot
split, then `run_tui` and `run_top`. Run `cargo test top_tui` after each
step.

## Tests

All in `top_tui.rs`:

- `plan_refresh_rereads_disk_only_on_change_or_age`: a missing last
  fingerprint ⇒ disk; same fingerprint at 1 s ⇒ no disk; changed
  fingerprint at 1 s ⇒ disk; same at 5 s ⇒ disk.
- `plan_refresh_asks_for_shells_every_two_seconds`: 1 s ⇒ no; 2 s ⇒ yes.
- `follow_selects_the_newest_live_run`: rows `done, live, live` ⇒ 1;
  no live row ⇒ 0.
- `scrolling_stops_follow_and_f_resumes_it`, including the clamp at both
  ends.
- `filter_keys_edit_the_activity_filter`: `/`, `e`, `q`, `x`, Backspace,
  then Enter ⇒ filter `eq`, not editing, not quit. Then `/`, Esc ⇒ empty.
- `q_and_ctrl_c_quit`: `q` quits when not editing, Ctrl-C quits while
  editing.
- `screen_draws_the_four_panels`: at 80×24 with a down daemon, the buffer
  holds the four titles, `daemon not running` and `following`.

## Acceptance criteria

All values below were measured against the tree before this phase.

- [ ] Each new test ⇒ `test result: ok. 1 passed`.
- [ ] On the § End-to-end harness (no daemon, a private tmux server):
  - `tui_panels=4`, `tui_down=1`, `tui_updated=1` and `tui_exit=0`.
    Today these read `0`, `0`, `0` and `2`.
  - `pipe_msg=1`. Today it reads `0`, because the old message names a
    later release.
  - `pipe_exit=2` and `once_order=Runs,Budget,Shells,Activity` are
    unchanged (preservation, both measured).
- [ ] `cargo test` ⇒ `failed=0`, with `passed` above 2497 by the new
      tests. Report the number; do not re-run to chase it.
- [ ] Clippy (`-D warnings`) and `cargo fmt --all --check` exit 0. No new
      `#[allow]`, no production `unwrap`/`expect`, no new Cargo feature.
- [ ] The end-to-end Update Log entry holds the output fence **first**,
      then the `PASTE MATCH` line.

## End-to-end verification

The harness uses its own tmux socket (`-S`) and a throwaway `HOME`, so it
touches no running tmux server and no real daemon.

```sh
A=/tmp/e2e-m30-09.txt; : > "$A"
for t in plan_refresh_rereads_disk_only_on_change_or_age plan_refresh_asks_for_shells_every_two_seconds follow_selects_the_newest_live_run scrolling_stops_follow_and_f_resumes_it filter_keys_edit_the_activity_filter q_and_ctrl_c_quit screen_draws_the_four_panels; do cargo test "$t" 2>&1 | grep -E '^test result: ok. 1 passed' | head -1 | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
cargo build -q 2>/dev/null
H=$(mktemp -d); mkdir -p "$H/.daemoneye/var/log/events"
S="$H/.daemoneye/var/log/events/events-$(date +%Y%m%d).jsonl"
ev() { echo "{\"ts\":\"$(date -u +%Y-%m-%dT%H:%M:%SZ)\",\"event\":\"ai_cost\",\"pid\":1,\"agent_name\":\"chat\",\"model\":\"m\",\"cost\":{\"total_cost_usd\":$1}}" >> "$S"; }
ev 0.50
echo "once_order=$(HOME="$H" ./target/debug/daemoneye top --once | grep -xE 'Runs|Budget|Shells|Activity' | paste -sd, -)" >> "$A"
HOME="$H" ./target/debug/daemoneye top 2>"$H/err" | cat; echo "pipe_exit=${PIPESTATUS[0]}" >> "$A"
echo "pipe_msg=$(grep -c 'not a terminal' "$H/err")" >> "$A"
K="$H/tmux.sock"; B="$PWD/target/debug/daemoneye"
tmux -S "$K" new-session -d -s top -x 120 -y 40 "HOME=$H $B top; echo exit=\$? > $H/exit; sleep 5"
sleep 2; P=$(tmux -S "$K" capture-pane -p -t top 2>/dev/null)
echo "tui_panels=$(for w in Runs Budget Shells Activity; do echo "$P" | grep -q "$w" && echo "$w"; done | wc -l)" >> "$A"
echo "tui_down=$(echo "$P" | grep -c 'daemon not running')" >> "$A"
ev 0.25; sleep 2; P=$(tmux -S "$K" capture-pane -p -t top 2>/dev/null)
echo "tui_updated=$(echo "$P" | grep -c 'spend: \$0\.75')" >> "$A"
tmux -S "$K" send-keys -t top q; sleep 1
echo "tui_$(cat "$H/exit" 2>/dev/null || echo exit=none)" >> "$A"
tmux -S "$K" kill-server 2>/dev/null
find "$H" -type f -delete; find "$H" -depth -type d -empty -delete
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Then run the self-check, and put its line in the entry after the output
fence:

```sh
D=docs/dev/milestones/M30-telemetry-dashboard/phase-09-top-tui.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m30-09.txt
diff /tmp/pasted-m30-09.txt /tmp/e2e-m30-09.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

If a criterion reads a value other than the one stated, and you can show
the value is right, record the evidence in the Update Log and report.
Do not keep investigating.

## Authorizations

- The new file `src/cli/commands/top_tui.rs`, and `mod top_tui;` in
  `src/cli/commands/mod.rs`.
- In `src/main.rs`, only the `Top` variant's doc comments.

## Out of scope

- A run-detail pane, mouse support, colours beyond the reversed
  selection, or any key that acts on a run. Stopping a run stays `/stop`
  and `daemoneye stop`.
- Any change to `--once` output, `ShellInfo` or the daemon.
- The live close (phase 10).

## Update Log

<!-- entries appended below this line -->

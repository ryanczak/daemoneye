# Phase 06d: log-replay — `space` in `/log` replays the recording at its recorded timing

**Milestone:** M23 — Client decoupling & shell UX
**Status:** done
**Depends on:** 06c (done)
**Estimated diff:** ~150 lines of production code across 2 files; 7 new tests (~365 diff lines in all)
**Tags:** language=rust, kind=feature, size=m

## Goal

After this phase, pressing `space` in a `/log` view replays the recording:

- the grid is cleared and each output event appears when its time comes
  (`LogEvent.t`). Any pause longer than **2 s** (`REPLAY_IDLE_CAP`) replays
  as 2 s, the wait before the first event included;
- while the replay runs: `space` pauses and resumes it, `Ctrl-C` closes the
  view, and **any other key skips to the end**, `Esc` and `q` included. A
  focus change or a mouse report is not a key press and does nothing;
- when the last event is shown, or on a skip, the replay ends. The grid then
  holds the whole log again, so `n`/`p` still find the commands. The status
  note says `replay finished` or `replay skipped to the end`;
- the status line during a replay reads
  `log <id> · <rows>×<cols> · replay <clock>s/<total>s · space to pause | paused · space to resume · any key skips to the end · ctrl-c to close`.
  The ordinary log status line gains `space replay`;
- a log with no output events answers `space` with `nothing to replay`.

`/help`'s `/log` row says `n/p commands, space replays`.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.6: `/log <id>` with replay.
- `docs/dev/milestones/M23-client-decoupling/README.md`: row 06d.
- `src/cli/tail.rs`: the `/log` model and viewer from 06c (`TailModel::from_log`,
  `log_key_action`, `tail_loop`, `render_tail`).

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on **2026-10-03** at commit `57b9a0ff`. Baseline:
`cargo test --lib` → **2170 passed / 0 failed / 14 ignored**.

**The whole change below was dry-run in this tree, then reverted.** Results
on the dry run:
- the full suite gave lib **2177**/0/14 on four runs, every other target
  unmoved (bug_tracker 6, doc_truth 14, integration 32, isolation 11);
- fmt and clippy `-D warnings` were clean;
- **the § End-to-end verification block itself was run on the dry-run tree**
  and printed exactly the Expected values below. On the current tree it
  prints `lib_counts=2170p-0f-14i`, `phase_tests=0p-0f` and `wired=0/0/0`;
- Mutations A–F each went red on 3 of 3 runs, **and each mutant passes
  clippy `-D warnings` with zero diagnostics**.

### Facts measured for this phase

- **Real recordings need the idle cap.** Measured over the six recordings in
  `~/.daemoneye/var/log/shells/`: the longest ran **352 s** of wall time
  with single gaps up to **107 s**, and replays in **10.1 s** with a 2 s cap.
  Another ran 89 s and replays in 2.5 s. Uncapped, a replay is mostly
  waiting.
- **A replay starts from a fresh grid and ends with the same grid
  `from_log` built.** vt100's parser is a streaming state machine, so feeding
  the same bytes in different pieces gives the same screen and history. That
  is why a finished or skipped replay keeps the 06c command marks valid. The
  test `replay_skip_shows_the_whole_log_with_its_marks` pins it by comparing
  `all_lines()` with a fresh `from_log`.
- **`TailModel` did not keep the events.** `from_log` read them and dropped
  them. The model now stores them (`events: Vec<LogEvent>`, private) for
  replay. A log is at most 4 MiB of output (the daemon's cap), so the copy is
  bounded.
- **The clock lives in `tail_loop`, the schedule in the model.** The model is
  pure: `replay_tick(dt)` advances a running replay by `dt` seconds and feeds
  every event now due, so tests drive it with exact numbers. `tail_loop` adds
  one `select!` branch, a 33 ms `tokio::time::interval` that is enabled only
  while a replay runs, and passes the real elapsed time.
- **Gotcha: time before `space`, or while paused, must not count.** The
  interval branch is disabled then. When it is re-enabled, its first tick
  fires at once (`MissedTickBehavior::Skip`), and a `last`-tick timestamp
  that was not reset would hand `replay_tick` the whole idle period. That
  would show everything at once. So **every key resets `last`**.
  `tail_loop_replays_a_log_in_recorded_time` waits 5 s before `space` to pin
  this (Mutation D).
- **Gotcha: `Key` carries non-keys.** `Key::FocusGained`, `Key::FocusLost`
  and `Key::Mouse { .. }` arrive on the same stream
  (`src/cli/input/tty.rs:116-129`). Under "any other key skips", switching
  terminal focus would end the replay. `replay_key_action` maps them to
  `Ignore` (Mutation F).
- **The loop test runs on tokio's paused clock.**
  `#[tokio::test(start_paused = true)]` needs tokio's `test-util` feature,
  which `Cargo.toml` already enables in `[dev-dependencies]` (line 54).
  `tokio::time::Instant` follows the paused clock; `std::time::Instant` does
  not, so the loop must use the tokio one.
- **The live chat UI is not checkable here** (clientless tmux hangs chat, an
  M14 fact). The viewer is tested through the real `tail_loop`, as in 06c.

## Spec

Make **exactly** the changes the diffs below show. They are the dry run,
formatted and clippy-clean, tests included. Apply them with your edit tools.
`cargo build --tests` is the check that you missed nothing. Copy the `+`
lines; do not retype from memory.

### Task 1 — replay in the model, keys, loop and status line, with tests (`src/cli/tail.rs`)

```diff
--- a/src/cli/tail.rs
+++ b/src/cli/tail.rs
@@ -15,6 +15,13 @@ pub const TAIL_SCROLLBACK: usize = 5_000;
 /// line per newline byte, so a piece this size stays far inside that limit.
 const LOG_FEED_PIECE: usize = 1024;
 
+/// The longest pause a replay keeps between two events, in seconds. A
+/// recording where nothing happened for ten minutes replays the gap as this.
+pub const REPLAY_IDLE_CAP: f64 = 2.0;
+
+/// How often a running replay advances its clock and redraws.
+const REPLAY_TICK: std::time::Duration = std::time::Duration::from_millis(33);
+
 /// One tailed shell. `follow` keeps the view pinned to the newest output;
 /// scrolling up pauses it, and the view then stays put while output arrives.
 pub struct TailModel {
@@ -34,6 +41,36 @@ pub struct TailModel {
     pub at_mark: Option<usize>,
     /// For a log, true when the daemon dropped the oldest output to fit.
     pub truncated: bool,
+    /// For a log, its output events, kept so `space` can replay them.
+    events: Vec<LogEvent>,
+    /// The replay in progress, if any.
+    pub replay: Option<Replay>,
+}
+
+/// A replay of a log at its recorded timing. `due[i]` is when event `i` is
+/// shown, in seconds from the start, with every idle gap capped at
+/// `REPLAY_IDLE_CAP`; `clock` is how far the replay has got.
+#[derive(Debug, Clone, PartialEq)]
+pub struct Replay {
+    pub due: Vec<f64>,
+    pub next: usize,
+    pub clock: f64,
+    pub paused: bool,
+}
+
+/// When each event is shown in a replay: its recorded time, with every gap
+/// (the first event's time included) capped at `REPLAY_IDLE_CAP` and a time
+/// that goes backwards treated as no gap.
+pub fn replay_schedule(events: &[LogEvent]) -> Vec<f64> {
+    let mut due = Vec::with_capacity(events.len());
+    let mut at = 0.0;
+    let mut prev = 0.0;
+    for event in events {
+        at += (event.t - prev).clamp(0.0, REPLAY_IDLE_CAP);
+        prev = event.t;
+        due.push(at);
+    }
+    due
 }
 
 /// Where one recorded command begins in a log view.
@@ -71,6 +108,8 @@ impl TailModel {
             marks: Vec::new(),
             at_mark: None,
             truncated: false,
+            events: Vec::new(),
+            replay: None,
         }
     }
 
@@ -95,6 +134,7 @@ impl TailModel {
         model.exited = true;
         model.log = true;
         model.truncated = truncated;
+        model.events = events.to_vec();
         // Lines that have scrolled off the top of the screen so far.
         let mut scrolled = 0usize;
         let mut starts: Vec<usize> = Vec::with_capacity(commands.len());
@@ -185,6 +225,68 @@ impl TailModel {
         ));
     }
 
+    /// True while a replay is on screen, paused or not.
+    pub fn replaying(&self) -> bool {
+        self.replay.is_some()
+    }
+
+    /// True while a replay is on screen and its clock is running.
+    pub fn replay_running(&self) -> bool {
+        self.replay.as_ref().is_some_and(|r| !r.paused)
+    }
+
+    /// Start replaying the log: clear the grid and show each event as its
+    /// time comes, following the newest output.
+    fn start_replay(&mut self) {
+        if self.events.is_empty() {
+            self.note = Some("nothing to replay".to_string());
+            return;
+        }
+        let (rows, cols) = self.parser.screen().size();
+        self.parser = vt100::Parser::new(rows, cols, TAIL_SCROLLBACK);
+        self.follow = true;
+        self.at_mark = None;
+        self.replay = Some(Replay {
+            due: replay_schedule(&self.events),
+            next: 0,
+            clock: 0.0,
+            paused: false,
+        });
+        self.replay_tick(0.0);
+    }
+
+    /// Advance a running replay by `dt` seconds and show every event now due.
+    /// When the last one is shown the replay ends and the log view returns.
+    pub fn replay_tick(&mut self, dt: f64) {
+        let Some(r) = self.replay.as_mut() else {
+            return;
+        };
+        if r.paused {
+            return;
+        }
+        r.clock += dt;
+        while r.next < r.due.len() && r.due[r.next] <= r.clock {
+            self.parser.process(self.events[r.next].data.as_bytes());
+            r.next += 1;
+        }
+        if r.next == r.due.len() {
+            self.replay = None;
+            self.note = Some("replay finished".to_string());
+        }
+    }
+
+    /// Show the rest of the log at once and end the replay. The grid then
+    /// holds what `from_log` built, so the command marks still line up.
+    fn skip_replay(&mut self) {
+        let Some(r) = self.replay.take() else {
+            return;
+        };
+        for event in &self.events[r.next..] {
+            self.parser.process(event.data.as_bytes());
+        }
+        self.note = Some("replay skipped to the end".to_string());
+    }
+
     /// Apply one later frame of the stream. Returns `true` when the view may
     /// have changed and should be redrawn.
     pub fn apply(&mut self, frame: &Response) -> bool {
@@ -347,6 +449,9 @@ pub enum TailAction {
     MatchNewer,
     NextCommand,
     PrevCommand,
+    ReplayStart,
+    ReplayPause,
+    ReplaySkip,
     Quit,
     Ignore,
 }
@@ -392,11 +497,26 @@ pub fn log_key_action(key: &crate::cli::input::Key) -> TailAction {
     match key {
         Key::Char('n') => TailAction::NextCommand,
         Key::Char('p') => TailAction::PrevCommand,
+        Key::Char(' ') => TailAction::ReplayStart,
         Key::Char('f') | Key::Char('/') | Key::Char('N') => TailAction::Ignore,
         other => tail_key_action(other, false),
     }
 }
 
+/// Decode one key while a replay is on screen: `space` pauses and resumes,
+/// `Ctrl-C` closes the view, and every other key skips to the end. Focus
+/// changes and mouse reports are not keys the user pressed, so they do
+/// nothing.
+pub fn replay_key_action(key: &crate::cli::input::Key) -> TailAction {
+    use crate::cli::input::Key;
+    match key {
+        Key::Char(' ') => TailAction::ReplayPause,
+        Key::CtrlC => TailAction::Quit,
+        Key::FocusGained | Key::FocusLost | Key::Mouse { .. } => TailAction::Ignore,
+        _ => TailAction::ReplaySkip,
+    }
+}
+
 impl TailModel {
     /// Apply one viewer action; `page` is how many lines a page scroll moves.
     /// Returns `false` when the viewer should close.
@@ -455,6 +575,13 @@ impl TailModel {
             }
             TailAction::NextCommand => self.step_mark(true),
             TailAction::PrevCommand => self.step_mark(false),
+            TailAction::ReplayStart => self.start_replay(),
+            TailAction::ReplayPause => {
+                if let Some(r) = self.replay.as_mut() {
+                    r.paused = !r.paused;
+                }
+            }
+            TailAction::ReplaySkip => self.skip_replay(),
             TailAction::Quit => return false,
             TailAction::Ignore => {}
         }
@@ -627,8 +754,18 @@ where
 
     terminal.draw(|f| render_tail(f, model))?;
     let mut stream_open = true;
+    let mut ticker = tokio::time::interval(REPLAY_TICK);
+    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
+    // When the replay clock last moved; reset by every key, so time spent
+    // paused, or before a replay starts, is never counted.
+    let mut last = tokio::time::Instant::now();
     loop {
         tokio::select! {
+            _ = ticker.tick(), if model.replay_running() => {
+                let now = tokio::time::Instant::now();
+                model.replay_tick(now.duration_since(last).as_secs_f64());
+                last = now;
+            }
             line = frames.next_line(), if stream_open => {
                 match line {
                     Ok(Some(line)) => {
@@ -647,7 +784,9 @@ where
                 let Some(key) = key else { break };
                 let page = terminal.size()?.height.saturating_sub(1) as usize;
                 model.note = None;
-                let action = if model.log {
+                let action = if model.replaying() {
+                    replay_key_action(&key)
+                } else if model.log {
                     log_key_action(&key)
                 } else {
                     tail_key_action(&key, model.typing())
@@ -655,6 +794,7 @@ where
                 if !model.act(action, page) {
                     break;
                 }
+                last = tokio::time::Instant::now();
             }
         }
         terminal.draw(|f| render_tail(f, model))?;
@@ -662,6 +802,25 @@ where
     Ok(())
 }
 
+/// The status line while a replay is on screen: how far it has got of how
+/// long, and whether it is paused.
+fn replay_status(model: &TailModel, rows: u16, cols: u16) -> String {
+    let Some(r) = &model.replay else {
+        return String::new();
+    };
+    let total = r.due.last().copied().unwrap_or(0.0);
+    let state = if r.paused {
+        "paused · space to resume"
+    } else {
+        "space to pause"
+    };
+    format!(
+        "log {} · {rows}×{cols} · replay {:.1}s/{total:.1}s · {state} · any key skips to the end · ctrl-c to close",
+        model.id,
+        r.clock.min(total)
+    )
+}
+
 /// Paint `model`'s grid into the frame, cell by cell, with a one-line status
 /// bar at the bottom. The grid keeps the shell's own size: when the terminal
 /// is smaller, the top-left corner is shown; when it is larger, the rest is
@@ -716,6 +875,7 @@ pub fn render_tail(f: &mut ratatui::Frame, model: &TailModel) {
         format!("paused · {} lines back", model.offset())
     };
     let status = match (&model.search, &model.note) {
+        _ if model.replay.is_some() => replay_status(model, rows, cols),
         _ if model.log => {
             let at = match &model.note {
                 Some(note) => note.clone(),
@@ -727,7 +887,7 @@ pub fn render_tail(f: &mut ratatui::Frame, model: &TailModel) {
                 ""
             };
             format!(
-                "log {} · {rows}×{cols} · {at}{cut} · n/p commands · ↑↓ PgUp/PgDn · esc to close",
+                "log {} · {rows}×{cols} · {at}{cut} · n/p commands · space replay · ↑↓ PgUp/PgDn · esc to close",
                 model.id
             )
         }
@@ -1324,6 +1484,208 @@ mod tests {
         assert_eq!(model.at_mark, Some(1), "in a log, n steps commands");
     }
 
+    /// Three events at 0 s, 1 s and 30 s: the 29 s gap replays as 2 s, so
+    /// the last one is due at 3 s.
+    fn timed() -> Vec<crate::ipc::LogEvent> {
+        [(0.0, "one\r\n"), (1.0, "two\r\n"), (30.0, "three\r\n")]
+            .iter()
+            .map(|(t, data)| crate::ipc::LogEvent {
+                t: *t,
+                data: data.to_string(),
+            })
+            .collect()
+    }
+
+    fn shown(m: &TailModel) -> String {
+        m.screen().contents().trim_end().to_string()
+    }
+
+    #[test]
+    fn replay_schedule_caps_idle_gaps() {
+        let at = |ts: &[f64]| -> Vec<crate::ipc::LogEvent> {
+            ts.iter()
+                .map(|t| crate::ipc::LogEvent {
+                    t: *t,
+                    data: String::new(),
+                })
+                .collect()
+        };
+        assert_eq!(
+            replay_schedule(&at(&[0.5, 1.0, 30.0, 30.25, 29.0, 40.0])),
+            [0.5, 1.0, 3.0, 3.25, 3.25, 5.25],
+            "gaps over 2 s shrink to 2 s; a time going backwards is no gap"
+        );
+        assert_eq!(
+            replay_schedule(&at(&[3600.0])),
+            [2.0],
+            "the wait before the first event is capped too"
+        );
+    }
+
+    #[test]
+    fn replay_shows_each_event_when_its_time_comes() {
+        let mut m = TailModel::from_log("s3-17", 24, 80, &timed(), &[], false);
+        m.act(TailAction::ReplayStart, 10);
+        assert!(m.replay_running());
+        assert_eq!(shown(&m), "one", "a replay starts from a clear grid");
+        m.replay_tick(0.5);
+        assert_eq!(shown(&m), "one");
+        m.replay_tick(0.5);
+        assert_eq!(shown(&m), "one\ntwo");
+        m.replay_tick(1.5);
+        assert!(m.replaying(), "the 29 s gap replays as 2 s, not less");
+        assert_eq!(shown(&m), "one\ntwo");
+        m.replay_tick(0.5);
+        assert_eq!(shown(&m), "one\ntwo\nthree");
+        assert!(!m.replaying(), "the replay ends with its last event");
+        assert_eq!(m.note.as_deref(), Some("replay finished"));
+    }
+
+    #[test]
+    fn replay_pause_holds_the_clock() {
+        let mut m = TailModel::from_log("s3-17", 24, 80, &timed(), &[], false);
+        m.act(TailAction::ReplayStart, 10);
+        m.act(TailAction::ReplayPause, 10);
+        assert!(m.replaying() && !m.replay_running());
+        m.replay_tick(100.0);
+        assert_eq!(shown(&m), "one", "a paused replay shows nothing new");
+        m.act(TailAction::ReplayPause, 10);
+        assert!(m.replay_running(), "space again resumes");
+        m.replay_tick(1.0);
+        assert_eq!(shown(&m), "one\ntwo");
+    }
+
+    #[test]
+    fn replay_skip_shows_the_whole_log_with_its_marks() {
+        let (mut events, commands) = session(20, 3);
+        for (i, e) in events.iter_mut().enumerate() {
+            e.t = i as f64;
+        }
+        let mut whole = TailModel::from_log("s3-17", 24, 80, &events, &commands, false);
+        let mut m = TailModel::from_log("s3-17", 24, 80, &events, &commands, false);
+        m.act(TailAction::ReplayStart, 10);
+        m.replay_tick(1.0);
+        assert!(m.replaying());
+        m.act(TailAction::ReplaySkip, 10);
+        assert!(!m.replaying());
+        assert_eq!(m.note.as_deref(), Some("replay skipped to the end"));
+        assert_eq!(m.all_lines(), whole.all_lines(), "the whole log is back");
+        assert_marks_on_their_lines(&mut m);
+        m.act(TailAction::NextCommand, 10);
+        assert_eq!(top_line(&m), "$ cmd-0", "n still finds the commands");
+
+        let mut empty = TailModel::from_log("s3-17", 24, 80, &[], &[], false);
+        empty.act(TailAction::ReplayStart, 10);
+        assert!(!empty.replaying());
+        assert_eq!(empty.note.as_deref(), Some("nothing to replay"));
+    }
+
+    #[test]
+    fn replay_keys_pause_close_or_skip() {
+        use crate::cli::input::Key;
+        assert_eq!(log_key_action(&Key::Char(' ')), TailAction::ReplayStart);
+        assert_eq!(replay_key_action(&Key::Char(' ')), TailAction::ReplayPause);
+        assert_eq!(replay_key_action(&Key::CtrlC), TailAction::Quit);
+        for skip in [
+            Key::Char('q'),
+            Key::Char('\x1b'),
+            Key::Char('n'),
+            Key::Down,
+            Key::Enter,
+        ] {
+            assert_eq!(replay_key_action(&skip), TailAction::ReplaySkip, "{skip:?}");
+        }
+        for none in [
+            Key::FocusGained,
+            Key::FocusLost,
+            Key::Mouse {
+                button: 0,
+                col: 1,
+                row: 1,
+                pressed: true,
+            },
+        ] {
+            assert_eq!(replay_key_action(&none), TailAction::Ignore, "{none:?}");
+        }
+    }
+
+    #[test]
+    fn render_tail_shows_the_replay_status_line() {
+        use ratatui::Terminal;
+        use ratatui::backend::TestBackend;
+        let bottom = |m: &TailModel| -> String {
+            let mut terminal = Terminal::new(TestBackend::new(140, 8)).unwrap();
+            terminal.draw(|f| render_tail(f, m)).unwrap();
+            terminal
+                .backend()
+                .buffer()
+                .content
+                .iter()
+                .skip(7 * 140)
+                .take(140)
+                .flat_map(|c| c.symbol().chars())
+                .collect()
+        };
+        let mut m = TailModel::from_log("s3-17", 5, 20, &timed(), &[], false);
+        assert!(bottom(&m).contains("space replay"), "{:?}", bottom(&m));
+        m.act(TailAction::ReplayStart, 10);
+        m.replay_tick(0.5);
+        let running = bottom(&m);
+        assert!(running.starts_with("log s3-17"), "{running:?}");
+        assert!(running.contains("replay 0.5s/3.0s"), "{running:?}");
+        assert!(running.contains("space to pause"), "{running:?}");
+        m.act(TailAction::ReplayPause, 10);
+        assert!(
+            bottom(&m).contains("paused · space to resume"),
+            "{:?}",
+            bottom(&m)
+        );
+    }
+
+    #[tokio::test(start_paused = true)]
+    async fn tail_loop_replays_a_log_in_recorded_time() {
+        use crate::cli::input::Key;
+        use ratatui::Terminal;
+        use ratatui::backend::TestBackend;
+        use std::time::Duration;
+        use tokio::io::AsyncBufReadExt;
+        let mut model = TailModel::from_log("s3-17", 24, 80, &timed(), &[], false);
+        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
+        let mut keys = Box::pin(futures_util::stream::unfold(rx, |mut rx| async move {
+            rx.recv().await.map(|key| (key, rx))
+        }));
+        // Five seconds before space, then 1.5 s of replay, then close.
+        tokio::spawn(async move {
+            tokio::time::sleep(Duration::from_secs(5)).await;
+            let _ = tx.send(Key::Char(' '));
+            tokio::time::sleep(Duration::from_millis(1500)).await;
+            let _ = tx.send(Key::CtrlC);
+        });
+        let mut frames = tokio::io::BufReader::new(tokio::io::empty()).lines();
+        let mut resizes = Box::pin(futures_util::stream::pending::<()>());
+        let mut terminal = Terminal::new(TestBackend::new(80, 25)).unwrap();
+        tokio::time::timeout(
+            Duration::from_secs(60),
+            tail_loop(
+                &mut terminal,
+                &mut model,
+                &mut frames,
+                &mut keys,
+                &mut resizes,
+            ),
+        )
+        .await
+        .expect("ctrl-c closes the loop")
+        .expect("the loop ends cleanly");
+        assert!(model.replaying(), "closed mid-replay");
+        assert_eq!(
+            shown(&model),
+            "one\ntwo",
+            "1.5 s in: the 1 s event is shown, the 3 s one is not, and the \
+             5 s before space did not count"
+        );
+    }
+
     fn lines(n: usize) -> Vec<u8> {
         (0..n)
             .map(|i| format!("line {i}\r\n"))
```

### Task 2 — the help row (`src/cli/commands/chat.rs`)

```diff
--- a/src/cli/commands/chat.rs
+++ b/src/cli/commands/chat.rs
@@ -33,7 +33,7 @@ Commands:
   /shells    list the agent's shells  /hosts     configured hosts and ssh state
   /tail      live view of a shell     (alias /watch; f follow, esc to close)
   /logs      past shell recordings    [host] [--owner chat|ghost] [--since 2h]
-  /log       open one recording       (/log <id>; n/p step through commands)
+  /log       open one recording       (/log <id>; n/p commands, space replays)
 
 Aliases: /models /panes /sessions /approval /new /quit (and help, ? open help).
 
```

### Task 3 — tests

All seven are in the Task 1 diff. Names pinned, all in `src/cli/tail.rs`:
`replay_schedule_caps_idle_gaps`,
`replay_shows_each_event_when_its_time_comes`,
`replay_pause_holds_the_clock`,
`replay_skip_shows_the_whole_log_with_its_marks`,
`replay_keys_pause_close_or_skip`,
`render_tail_shows_the_replay_status_line`,
`tail_loop_replays_a_log_in_recorded_time`.

**Test count:** seven new, so **2177 passed / 14 ignored**.

### Task 4 — capture the end-to-end evidence

Run the block in § End-to-end verification verbatim. Then create a new Update
Log entry titled exactly `### Update — <date> (end-to-end verification)`.
**Its first fenced block is the contents of `/tmp/e2e-m23-06d.txt`**: the
file's lines, copied by a tool. **Never paste the script itself**, and never
retype the lines (the real lines end `lib1=ok ` **with a trailing space**).
Then run the paste check once, put the line it prints into the entry **on its
own line**, and record the six mutations after it.

## Acceptance criteria

Parenthesised values are what each line produces against the current tree.
Every non-preservation line fails today. Each line was run on the current
tree **and** on the dry-run tree.

- [ ] `cargo test --lib` → **2177 passed / 0 failed / 14 ignored** on three
      consecutive runs (current: 2170/0/14). Both counts are exact.
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → other targets unmoved: bug_tracker 6, doc_truth 14,
      integration 32, isolation 11 (*preservation*).
- [ ] The phase's tests pass:
      ``cargo test --lib -- tail::tests::replay tail::tests::render_tail_shows_the_replay tail::tests::tail_loop_replays``
      → **7 passed** (current: 0). That exact filter printed 7 on the dry run.
- [ ] Wired, as the end-to-end block's `wired=` line prints it → **`1/1/1`**
      (current: `0/0/0`):
      ``grep -c 'replay_key_action(&key)' src/cli/tail.rs`` → 1;
      ``grep -c "Key::Char(' ') => TailAction::ReplayStart" src/cli/tail.rs`` → 1;
      ``grep -c 'space replays' src/cli/commands/chat.rs`` → 1.
- [ ] Mutations A–F were applied, run, and their observed results recorded
      **in this doc**.
- [ ] The end-to-end entry exists:
      ``grep -c '^### Update — .* (end-to-end verification)$' <this doc>`` →
      **1** (current: 0). Its first fenced block is the capture file's contents,
      and the paste-check line is on its own line:
      ``grep -cx 'PASTE MATCH' <this doc>`` → **1** (current: 0).

## Mutations

Apply each with the `patch` tool (the executor contract forbids `sed -i`,
`perl -i`, `> file` and `rm -rf`). Run the named test once with
`cargo test --lib -- <name>`, restore, and **record the observed result**.
Each was measured red at drafting on 3 of 3 runs, and each mutant builds
clippy-clean.

- **Mutation A** (`replay_schedule`, the idle cap): change
  `at += (event.t - prev).clamp(0.0, REPLAY_IDLE_CAP);` to
  `at += (event.t - prev).max(0.0);`. Run
  `tail::tests::replay_schedule_caps_idle_gaps`. Expected **red**.
- **Mutation B** (`replay_tick`, pause): delete the three lines
  `if r.paused {` / `return;` / `}` that come right before
  `r.clock += dt;`. Run `tail::tests::replay_pause_holds_the_clock`.
  Expected **red**.
- **Mutation C** (`skip_replay`): change
  `for event in &self.events[r.next..] {` to
  `for event in &self.events[r.next..r.next] {`. Run
  `tail::tests::replay_skip_shows_the_whole_log_with_its_marks`. Expected
  **red**.
- **Mutation D** (`tail_loop`, keys reset the clock): delete the line
  `last = tokio::time::Instant::now();` in the **key** branch (right after
  the `if !model.act(action, page) { break; }` block, not the
  `let mut last = …` before the loop). Run
  `tail::tests::tail_loop_replays_a_log_in_recorded_time`. Expected **red**
  (the 5 s before `space` count, and all three events show).
- **Mutation E** (`tail_loop`, the ticker): change
  `_ = ticker.tick(), if model.replay_running() => {` to
  `_ = ticker.tick(), if false => {`. Run
  `tail::tests::tail_loop_replays_a_log_in_recorded_time`. Expected **red**.
- **Mutation F** (`replay_key_action`, non-keys): delete the line
  `Key::FocusGained | Key::FocusLost | Key::Mouse { .. } => TailAction::Ignore,`.
  Run `tail::tests::replay_keys_pause_close_or_skip`. Expected **red**.

After each restore, confirm `git diff --stat` matches its pre-mutation state.

## Test plan

Seven new tests, all without a daemon or a terminal:
- the schedule caps every gap at 2 s, the first event's wait included, and
  treats a backwards time as no gap;
- a replay starts from a clear grid, shows each event at its due time, waits
  out a capped gap, and ends itself with `replay finished`;
- a paused replay shows nothing new however much time passes, and `space`
  resumes it;
- a skip shows the whole log, matching a fresh `from_log` line for line,
  with the marks still on their lines and `n` still working; an empty log
  answers `nothing to replay`;
- during a replay, `space` pauses, `Ctrl-C` closes, `q`/`Esc`/`n`/arrows/
  `Enter` skip, and focus and mouse reports do nothing; in a log, `space`
  starts a replay;
- the replay status line, running and paused, and `space replay` in the log
  status line;
- the real `tail_loop` on tokio's paused clock: 5 s idle, `space`, 1.5 s,
  `Ctrl-C` shows exactly the first two events.

## End-to-end verification

```sh
cd /home/matt/src/daemoneye
{
  for i in 1 2 3; do
    out=$(cargo test --lib 2>&1)
    printf 'lib%d=%s %s\n' "$i" "$(printf '%s\n' "$out" | grep -E '^test result' | tail -1 | grep -oE 'ok|FAILED')" "$(printf '%s\n' "$out" | grep -E '^test .* FAILED$' | sed -E 's/^test (.*) \.\.\. FAILED$/\1/' | tr '\n' ' ')"
  done
  printf 'lib_counts=%s\n' "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1p-\2f-\3i/')"
  printf 'phase_tests=%s\n' "$(cargo test --lib -- tail::tests::replay tail::tests::render_tail_shows_the_replay tail::tests::tail_loop_replays 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
  printf 'isolation=%s\n' "$(cargo test --test isolation 2>&1 | grep -E '^test result' | tail -1 | grep -oE '[0-9]+ passed; [0-9]+ failed')"
  printf 'wired=%s/%s/%s\n' "$(grep -c 'replay_key_action(&key)' src/cli/tail.rs)" "$(grep -c "Key::Char(' ') => TailAction::ReplayStart" src/cli/tail.rs)" "$(grep -c 'space replays' src/cli/commands/chat.rs)"
} > /tmp/e2e-m23-06d.txt
cat /tmp/e2e-m23-06d.txt
```

Expected:
- `lib1..3=ok` with **nothing after `ok`**;
- `lib_counts=2177p-0f-14i`;
- `phase_tests=7p-0f`;
- `isolation=11 passed; 0 failed`;
- `wired=1/1/1`.

**Re-run budget: three** (WORKFLOW.md § End-to-end verification). **Two
pre-existing lib flakes are known:**
`shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker` and
`shell::pty::pause_resume_tests::pty_stop_with_no_foreground_job_remembers_nothing`.
Neither is in a file this phase touches. If either fails, name it and re-run
the whole block. After three re-runs, stop, paste the last capture as it is,
and name every failing test. Any other failing test is a finding: report it
by name.

**Run this block with nothing else running `cargo test`.** Then make the
entry's first fenced block the **contents of** `/tmp/e2e-m23-06d.txt`
(copied, not retyped; **never the script above**), and run:

```sh
D=docs/dev/milestones/M23-client-decoupling/phase-06d-log-replay.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m23-06d.txt
diff /tmp/pasted-m23-06d.txt /tmp/e2e-m23-06d.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

If it prints `PASTE MISMATCH`, run
`diff /tmp/pasted-m23-06d.txt /tmp/e2e-m23-06d.txt` once and fix what it
shows. **Run the check at most three times.** Put the line it prints into the
entry on its own line.

## Authorizations

You may edit these files, and nothing else:

- `src/cli/tail.rs`, `src/cli/commands/chat.rs`: the diffs above.
- This phase doc's Update Log.

## Out of scope

- **Replay speed controls** (faster/slower, a configurable idle cap).
- **Seeking** within a replay, or replaying from a command mark.
- **Replaying input events**: the recording's `"i"` events are not sent
  (06b), and the echo of typed text is already in the output.
- **README.md**: documented at the milestone close with the other slash
  commands.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-03 22:50 (started)

**Executor:** executor-06d (headless run)
Applied the spec's Task 1–3 diffs; beginning verification and mutation runs.

### Update — 2026-10-03 (end-to-end verification)

Captured to `/tmp/e2e-m23-06d.txt`; its contents:

```
lib1=ok 
lib2=ok 
lib3=ok 
lib_counts=2177p-0f-14i
phase_tests=7p-0f
isolation=11 passed; 0 failed
wired=1/1/1
```

PASTE MATCH

Mutations, each applied with `patch`, run once with
`cargo test --lib -- <name>`, restored, `git diff --stat` back at
`4 files changed, 372 insertions(+), 5 deletions(-)`:

- **A** (`replay_schedule`, idle cap → `max(0.0)`):
  `replay_schedule_caps_idle_gaps ... FAILED` (panicked at
  `src/cli/tail.rs:1513`, "gaps over 2 s shrink to 2 s") — red.
- **B** (`replay_tick`, pause check deleted):
  `replay_pause_holds_the_clock ... FAILED` (panicked at
  `src/cli/tail.rs:1548`, "a paused replay shows nothing new") — red.
- **C** (`skip_replay`, `r.next..r.next`):
  `replay_skip_shows_the_whole_log_with_its_marks ... FAILED` (panicked at
  `src/cli/tail.rs:1572`, "the whole log is back") — red.
- **D** (`tail_loop`, key no longer resets `last`):
  `tail_loop_replays_a_log_in_recorded_time ... FAILED` (panicked at
  `src/cli/tail.rs:1679`, the 5 s before `space` counted) — red.
- **E** (`tail_loop`, ticker `if false`):
  `tail_loop_replays_a_log_in_recorded_time ... FAILED` (panicked at
  `src/cli/tail.rs:1681`, no replay progress) — red.
- **F** (`replay_key_action`, focus/mouse arm deleted):
  `replay_keys_pause_close_or_skip ... FAILED` (panicked at
  `src/cli/tail.rs:1607`, `FocusGained` expected `Ignore`) — red.

All six mutants were clippy-clean under
`cargo clippy --all-targets --all-features -- -D warnings` while red
(checked across the runs; the final tree is clippy-clean again).

### Update — 2026-10-03 22:58 (complete, server-authored)

**Summary:** **Summary + Notes for review:** Phase 06d is implemented per the spec's diffs: `TailModel` keeps the log's events, `replay_schedule` caps every idle gap (first included) at 2 s, `replay_tick` is a pure clock, `tail_loop` drives it with a 33 ms interval enabled only while running and every key resets the tick timestamp, `replay_key_action` maps space→pause, Ctrl-C→close, focus/mouse→ignore, everything else→skip, and the replay status line plus `space replay` in the log status line and the `/help` row are in. End-to-end verification captured mechanically to `/tmp/e2e-m23-06d.txt` and pasted: `lib1..3=ok`, `lib_counts=2177p-0f-14i`, `phase_tests=7p-0f`, `isolation=11 passed; 0 failed`, `wired=1/1/1`, with the `PASTE MATCH` check green on its second run (first run failed only because my heredoc had omitted the opening fence before the pasted lines — fixed by adding the fence, then re-checking). Mutations A–F were each applied, run red once, restored (diff-stat back to baseline), and recorded in the phase doc; fmt, build, clippy `-D warnings` all clean; committed as `5ab63249` with a clean working tree. One deviation worth noting: the "started" Update Log entry was written before Task 1's edits were applied rather than after, and it names the run "executor-06d (headless run)"; the completion entry and status flip remain the server's to author, and the phase doc's status is still `in-progress`.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.08s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.59s


TEST
tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 14 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_tools_counts_are_accurate ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test e2e_prompt_asset_tool_hygiene ... ok
test readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test prompt_assets_name_no_retired_tools ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_documents_every_config_field ... ok

test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 34 tests
test daemon_ping_status_loop ... ignored
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test cancel_request_roundtrip ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ghost_config_parsing ... ok
test ipc_tool_call_response_round_trip ... ok
test resume_run_request_roundtrip ... ok
test ipc_session_info_round_trip ... ok
test minimal_config_parsing ... ok
test window_switch_does_not_corrupt_chat ... ignored
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_below_threshold_discarded ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test an_empty_session_starts_no_tmux_server ... ok
test hooks_land_on_private_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.16s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Compiling daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.75s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-9810e08c3de1e001)
kill: sending signal to 2146970 failed: No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-54ad57cf3704bc40)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-d709c9b862b5691d)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-34cb94d3012a3d69)
     Running tests/integration.rs (target/debug/deps/integration-deb70358b520d0d5)
     Running tests/isolation.rs (target/debug/deps/isolation-45facff22bac60f5)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M23-client-decoupling/phase-06d-log-replay.md` — +49 -1
- `src/cli/commands/chat.rs` — +1 -1
- `src/cli/tail.rs` — +364 -2

**Commit:** 5ab6324952cbf92b15bb561c8dc4727d5d32816e

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-03

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark (86 turns, per
  `rexymcp.toml` and `executor_health`). Its entry names itself
  "executor-06d (headless run)": fold C, 13th occurrence.
- **Source diff vs the drafting dry run, tests included:** byte-identical
  (`git diff ee72705d 5ab63249 -- src/` against the saved dry-run patch).
- **Reviewer re-run:**
  - fmt, build and clippy `-D warnings` were clean, each run separately;
  - `cargo test` gave lib **2177/0/14**, with bug_tracker 6, doc_truth 14,
    integration 32 and isolation 11 unmoved;
  - the end-to-end block printed the Expected values exactly, and the paste
    check printed `PASTE MATCH` against the pasted capture. Both criteria
    greps gave 1;
  - mutations A–F, re-applied exactly as pinned, each went red on 3 of 3 runs
    with zero clippy diagnostics, and the tree restored clean.
- **No flake re-runs this time.** Naming both known flakes in the spec (the
  06c calibration) was not exercised.
- **Scope deviations:** none. The executor disclosed that its "started"
  entry was written before the edits rather than after. That is the order of
  the bookkeeping, not the work.
- **Calibration:** none new. Two phases in a row (06c, 06d) shipped
  byte-identical to the dry run after the end-to-end block was run on the
  dry-run tree at drafting.

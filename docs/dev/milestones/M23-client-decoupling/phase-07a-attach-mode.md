# Phase 07a: attach-mode — `/attach <sN>` types into a live shell; `Ctrl-d` detaches, `Ctrl-d Ctrl-d` sends EOF

**Milestone:** M23 — Client decoupling & shell UX
**Status:** todo
**Depends on:** 06d (done)
**Estimated diff:** ~280 lines of production code (one new file given whole, three small diffs); 9 new tests (~710 lines in all, about half tests)
**Tags:** language=rust, kind=feature, size=m

## Goal

After this phase, `/attach s3` in `daemoneye chat` opens shell `s3`
full-screen and hands it the keyboard:

- the client opens `ShellTail { follow: true }` (as `/tail` does). An unknown
  or dead shell prints `  ✗ attach <id>: <message>` without entering the
  alt screen;
- on entry the shell is resized (`ShellResize`) to the terminal less the
  status line, and the local grid follows. On every `SIGWINCH` both follow
  again. **On detach the shell is resized back** to the size it had before
  the user attached, so the agent's terminal is left as it found it;
- every key the user types goes to the shell as `ShellInput`, as raw bytes
  from the terminal, so Tab, `Ctrl-R`, F-keys and arrows all work. The grid
  is drawn locally with the shell's cursor shown where the shell has it;
- **`Ctrl-d` detaches.** Because `Ctrl-d` is taken, **`Ctrl-d Ctrl-d`**
  (the second within 400 ms) sends one `Ctrl-d` (EOF) to the shell, and the
  session goes on. A `Ctrl-d` followed by any other key detaches at once,
  and that key is not sent;
- the status line reads
  `attached <id> · <rows>×<cols> · ctrl-d detach · ctrl-d ctrl-d EOF`;
  after one `Ctrl-d` it reads
  `attached <id> · ctrl-d again sends EOF to the shell, or wait to detach`;
- if the shell exits while attached, the status says
  `attached <id> · exited · any key returns to chat`. Nothing more is sent,
  and no size is restored;
- a bare `/attach` prints `  usage: /attach <shell id>  (see /shells)`, and
  `/help` lists `/attach`.

This is the first half of row 07. **07b** adds the `/agents` picker (and
makes a bare `/attach` open it). **Row 08** intercepts `Ctrl-p` (pause) and
`Ctrl-c` (cooperative cancel) and makes attached input accountable. Until
then, `Ctrl-c` and `Ctrl-p` reach the shell as their plain bytes, so the
shell's own line discipline turns `Ctrl-c` into `SIGINT` for its foreground
job.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.6: "attached mode" — full-screen
  pass-through via `ShellInput`, resize propagated, grid rendered locally,
  `Ctrl-d` detaches, `Ctrl-d Ctrl-d` is EOF.
- `docs/dev/milestones/M23-client-decoupling/README.md`: row 07 (split here
  into 07a and 07b).
- `src/cli/tail.rs`: `TailModel`, `open_tail`, `render_tail` and
  `run_tail_viewer`, which this phase reuses.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on **2026-10-03** at commit `04109975`. Baseline:
`cargo test --lib` → **2177 passed / 0 failed / 14 ignored**.

**The whole change below was dry-run in this tree, then reverted.** Results
on the dry run:
- the full suite gave lib **2186**/0/14 on four runs, every other target
  unmoved (bug_tracker 6, doc_truth 14, integration 32, isolation 11);
- fmt and clippy `-D warnings` were clean;
- **the § End-to-end verification block itself was run on the dry-run tree**
  and printed exactly the Expected values below. On the current tree it
  prints `lib_counts=2177p-0f-14i`, `phase_tests=0p-0f` and `wired=0/0/0`;
- Mutations A–G each went red on 3 of 3 runs, **and each mutant passes
  clippy `-D warnings` with zero diagnostics**.

### Facts measured for this phase

- **The relay already exists.** `Request::ShellInput { id, bytes }` and
  `Request::ShellResize { id, rows, cols }` (`src/ipc.rs:243-252`) are
  handled in `src/daemon/server/shells.rs` (`handle_shell_input`,
  `handle_shell_resize`), and each answers `Ok` or `Error`. The host writes
  every input byte to the shell's cast log as an `"i"` event itself, so
  attached input is already recorded there. Row 08 adds `events.jsonl` and
  telling the agent.
- **One request per key is cheap.** Timed with a throwaway test on a real
  in-process shell host: `handle_shell_input` (daemon → host relay and ack)
  took **p50 26 µs, p99 ≤ 110 µs** over 300 calls. So `run_attach` sends
  each chunk with `ipc_client::request_once`, one connection each, from a
  single task that keeps them in order.
- **`read_key` is the wrong reader here.** It decodes into the lossy `Key`
  enum (`src/cli/input/tty.rs:173`), so there is no `Ctrl-R`, Tab, F-keys
  and so on. Attached mode reads raw bytes with `AsyncStdin::read_byte`
  (`src/cli/input/tty.rs:57`): it waits for one byte, then takes whatever
  follows within 5 ms, so an escape sequence or a paste arrives as one chunk
  (`read_chunk`).
- **Gotcha: chat's own terminal modes leak into the shell.** The chat
  renderer turns on bracketed paste and focus reporting for the whole session
  (`src/cli/render_ratatui.rs:251-253`:
  `execute!(std::io::stdout(), EnableBracketedPaste, EnableFocusChange)`).
  So while attached, the user's terminal sends `ESC [ I` / `ESC [ O` on
  focus changes and wraps every paste in `ESC [ 200 ~ … ESC [ 201 ~`. Passed
  through as-is, a focus change types `^[[I` into bash, and a paste into a
  program that never asked for brackets types `[200~`. `attach_keys` drops
  focus reports always, and drops paste brackets unless the shell's own
  screen asked for them (`vt100::Screen::bracketed_paste()`). It also turns
  arrows into `ESC O A`–`D` when the shell asked for application cursor keys
  (`application_cursor()`), because the outer terminal only ever sends
  `ESC [ A`.
- **`Ctrl-d Ctrl-d` needs a time window.** A single `Ctrl-d` cannot detach at
  once, or there would be no double-tap to detect. A `Ctrl-d` at the end of
  a chunk sets `ctrl_d_pending`. The next chunk decides (`Ctrl-d` → EOF,
  anything else → detach), or a `sleep_until` deadline 400 ms out detaches
  (`CTRL_D_WINDOW`). The deadline is a stored `Instant`, so frames arriving
  in between do not restart it.
- **The grid must be resized with the shell.** `TailModel`'s vt100 grid was
  fixed at the `ShellScreen` size. After a `ShellResize`, the host's output
  assumes the new size, so the model gains `resize(rows, cols)`
  (`Screen::set_size`).
- **The cursor.** `render_tail` never placed a cursor, since `/tail` is
  view-only. When `model.attached` and the shell has not hidden its cursor
  (`Screen::hide_cursor()`), it now calls `f.set_cursor_position`.
- **The loop test runs on tokio's paused clock**
  (`#[tokio::test(start_paused = true)]`, `test-util` is already a
  dev-dependency). That makes the 400 ms window deterministic.
- **The live chat UI is not checkable here** (clientless tmux hangs chat, an
  M14 fact). The real `attach_loop` is driven with a `TestBackend`, a duplex
  frame stream and a key channel, as `/tail` was. Row 10 is the live check.

## Spec

Make **exactly** the changes below. They are the dry run, formatted and
clippy-clean, tests included. `cargo build --tests` is the check that you
missed nothing. Copy; do not retype from memory.

### Task 1 — attached state, resize and cursor in the tail model (`src/cli/tail.rs`)

```diff
--- a/src/cli/tail.rs
+++ b/src/cli/tail.rs
@@ -45,6 +45,10 @@ pub struct TailModel {
     events: Vec<LogEvent>,
     /// The replay in progress, if any.
     pub replay: Option<Replay>,
+    /// True while the user is attached: keys go to the shell, not the view.
+    pub attached: bool,
+    /// While attached, true after one `Ctrl-d` while a second one is awaited.
+    pub ctrl_d_pending: bool,
 }
 
 /// A replay of a log at its recorded timing. `due[i]` is when event `i` is
@@ -110,9 +114,17 @@ impl TailModel {
             truncated: false,
             events: Vec::new(),
             replay: None,
+            attached: false,
+            ctrl_d_pending: false,
         }
     }
 
+    /// Resize the grid, as the shell's own terminal is resized. Clamped to
+    /// one row and one column, as `new` does.
+    pub fn resize(&mut self, rows: u16, cols: u16) {
+        self.parser.screen_mut().set_size(rows.max(1), cols.max(1));
+    }
+
     /// The model for a recording: every output event fed through a grid of
     /// the recorded size, each command's mark placed on the line it began on,
     /// and the view opened at the oldest line. A mark whose line has scrolled
@@ -802,6 +814,24 @@ where
     Ok(())
 }
 
+/// The status line while attached: how to leave, how to send EOF, and
+/// whether a first `Ctrl-d` is waiting for a second.
+fn attached_status(model: &TailModel, rows: u16, cols: u16) -> String {
+    if model.exited {
+        format!("attached {} · exited · any key returns to chat", model.id)
+    } else if model.ctrl_d_pending {
+        format!(
+            "attached {} · ctrl-d again sends EOF to the shell, or wait to detach",
+            model.id
+        )
+    } else {
+        format!(
+            "attached {} · {rows}×{cols} · ctrl-d detach · ctrl-d ctrl-d EOF",
+            model.id
+        )
+    }
+}
+
 /// The status line while a replay is on screen: how far it has got of how
 /// long, and whether it is paused.
 fn replay_status(model: &TailModel, rows: u16, cols: u16) -> String {
@@ -874,7 +904,14 @@ pub fn render_tail(f: &mut ratatui::Frame, model: &TailModel) {
     } else {
         format!("paused · {} lines back", model.offset())
     };
+    if model.attached && !model.exited && !screen.hide_cursor() {
+        let (row, col) = screen.cursor_position();
+        if row < body_height && col < area.width {
+            f.set_cursor_position((area.x + col, area.y + row));
+        }
+    }
     let status = match (&model.search, &model.note) {
+        _ if model.attached => attached_status(model, rows, cols),
         _ if model.replay.is_some() => replay_status(model, rows, cols),
         _ if model.log => {
             let at = match &model.note {
```

### Task 2 — register the module (`src/cli/mod.rs`)

```diff
--- a/src/cli/mod.rs
+++ b/src/cli/mod.rs
@@ -1,5 +1,6 @@
 // CLI Module
 
+pub mod attach;
 pub mod commands;
 pub(crate) mod diff;
 pub mod input;
```

### Task 3 — create `src/cli/attach.rs` with exactly this content

It is a new file: create it whole with your file-writing tool. It holds the
byte rules (`attach_keys`), the loop (`attach_loop`), the chat entry point
(`run_attach`) and eight of the nine tests.

```rust
//! Attached mode (`/attach <sN>`): the user's keystrokes go to a live PTY
//! shell as `ShellInput`, the terminal size follows as `ShellResize`, and the
//! shell's output is drawn locally through the `/tail` model.
//!
//! The byte rules are pure (`attach_keys`), and the loop is generic over its
//! sources and its output channel, so both are tested without a daemon or a
//! terminal.

use crate::cli::tail::{TailModel, render_tail};
use crate::ipc::Response;

/// How long a first `Ctrl-d` waits for a second before it detaches.
pub const CTRL_D_WINDOW: std::time::Duration = std::time::Duration::from_millis(400);

/// The most bytes one read of the keyboard gathers into a single `ShellInput`.
const KEY_CHUNK_MAX: usize = 4096;

/// How long a read waits for more bytes after the first, so an escape
/// sequence or a paste arrives as one chunk.
const KEY_CHUNK_GAP: std::time::Duration = std::time::Duration::from_millis(5);

const CTRL_D: u8 = 0x04;

/// What the attach loop sends to the daemon, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachOut {
    Input(Vec<u8>),
    Resize { rows: u16, cols: u16 },
}

/// The shell's terminal modes that change what a key should send.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ShellModes {
    /// The shell asked for bracketed paste (`ESC [ ? 2004 h`).
    pub bracketed_paste: bool,
    /// The shell asked for application cursor keys (`ESC [ ? 1 h`).
    pub application_cursor: bool,
}

impl ShellModes {
    pub fn of(screen: &vt100::Screen) -> Self {
        Self {
            bracketed_paste: screen.bracketed_paste(),
            application_cursor: screen.application_cursor(),
        }
    }
}

/// What one chunk of keyboard bytes comes to: the bytes to send, whether to
/// detach, and whether a trailing `Ctrl-d` now waits for a second.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AttachKeys {
    pub send: Vec<u8>,
    pub detach: bool,
    pub pending: bool,
}

/// Translate one chunk of the user's keyboard bytes for the shell.
///
/// - `Ctrl-d` `Ctrl-d` sends one `Ctrl-d` (EOF); a lone `Ctrl-d` detaches. A
///   `Ctrl-d` at the end of the chunk sets `pending`, and the next chunk (or
///   the `CTRL_D_WINDOW` timeout) decides. Bytes before a detaching `Ctrl-d`
///   are still sent; bytes after it are dropped.
/// - Focus reports (`ESC [ I`, `ESC [ O`) are the chat client's, never the
///   shell's: they are dropped.
/// - Paste brackets (`ESC [ 200 ~`, `ESC [ 201 ~`) are dropped unless the
///   shell asked for bracketed paste; the pasted text is kept.
/// - With application cursor keys on, `ESC [ A`–`D` become `ESC O A`–`D`.
pub fn attach_keys(chunk: &[u8], pending: bool, modes: ShellModes) -> AttachKeys {
    let mut out = AttachKeys::default();
    let mut i = 0;
    if pending {
        if chunk.first() == Some(&CTRL_D) {
            out.send.push(CTRL_D);
            i = 1;
        } else {
            out.detach = true;
            return out;
        }
    }
    while i < chunk.len() {
        let rest = &chunk[i..];
        if rest[0] == CTRL_D {
            match rest.get(1) {
                Some(&CTRL_D) => {
                    out.send.push(CTRL_D);
                    i += 2;
                }
                Some(_) => {
                    out.detach = true;
                    return out;
                }
                None => {
                    out.pending = true;
                    return out;
                }
            }
            continue;
        }
        if rest.starts_with(b"\x1b[I") || rest.starts_with(b"\x1b[O") {
            i += 3;
            continue;
        }
        if rest.starts_with(b"\x1b[200~") || rest.starts_with(b"\x1b[201~") {
            if modes.bracketed_paste {
                out.send.extend_from_slice(&rest[..6]);
            }
            i += 6;
            continue;
        }
        if modes.application_cursor
            && rest.len() >= 3
            && rest.starts_with(b"\x1b[")
            && matches!(rest[2], b'A'..=b'D')
        {
            out.send.extend_from_slice(&[0x1b, b'O', rest[2]]);
            i += 3;
            continue;
        }
        out.send.push(rest[0]);
        i += 1;
    }
    out
}

/// The grid size for a terminal: every row but the status line.
fn body_size(rows: u16, cols: u16) -> (u16, u16) {
    (rows.saturating_sub(1).max(1), cols.max(1))
}

/// The attached session's event loop. On entry the shell is resized to the
/// terminal (less the status line); keys are translated by `attach_keys` and
/// sent; a terminal resize is followed; frames update the grid. It ends on a
/// detach, a lone `Ctrl-d` whose window runs out, the keys ending, or any key
/// once the shell has exited. Unless the shell exited, it is then resized
/// back to the size it had before the user attached.
pub async fn attach_loop<B, R, C, W>(
    terminal: &mut ratatui::Terminal<B>,
    model: &mut TailModel,
    frames: &mut tokio::io::Lines<R>,
    chunks: &mut C,
    resizes: &mut W,
    out: &tokio::sync::mpsc::UnboundedSender<AttachOut>,
) -> anyhow::Result<()>
where
    B: ratatui::backend::Backend,
    B::Error: std::error::Error + Send + Sync + 'static,
    R: tokio::io::AsyncBufRead + Unpin,
    C: futures_util::Stream<Item = Vec<u8>> + Unpin,
    W: futures_util::Stream<Item = ()> + Unpin,
{
    use futures_util::StreamExt;

    let (orig_rows, orig_cols) = model.screen().size();
    let size = terminal.size()?;
    let (rows, cols) = body_size(size.height, size.width);
    model.resize(rows, cols);
    let _ = out.send(AttachOut::Resize { rows, cols });
    terminal.draw(|f| render_tail(f, model))?;

    let mut stream_open = true;
    // When a pending `Ctrl-d` detaches if no second one comes.
    let mut ctrl_d_deadline = tokio::time::Instant::now();
    loop {
        tokio::select! {
            line = frames.next_line(), if stream_open => {
                match line {
                    Ok(Some(line)) => {
                        if let Ok(frame) = serde_json::from_str::<Response>(&line) {
                            model.apply(&frame);
                        }
                    }
                    Ok(None) | Err(_) => {
                        stream_open = false;
                        model.exited = true;
                        model.ctrl_d_pending = false;
                    }
                }
            }
            _ = tokio::time::sleep_until(ctrl_d_deadline), if model.ctrl_d_pending => break,
            _ = resizes.next() => {
                let size = terminal.size()?;
                let (rows, cols) = body_size(size.height, size.width);
                model.resize(rows, cols);
                if !model.exited {
                    let _ = out.send(AttachOut::Resize { rows, cols });
                }
            }
            chunk = chunks.next() => {
                let Some(chunk) = chunk else { break };
                if model.exited {
                    break;
                }
                let keys = attach_keys(&chunk, model.ctrl_d_pending, ShellModes::of(model.screen()));
                if !keys.send.is_empty() {
                    let _ = out.send(AttachOut::Input(keys.send));
                }
                if keys.detach {
                    break;
                }
                if keys.pending {
                    ctrl_d_deadline = tokio::time::Instant::now() + CTRL_D_WINDOW;
                }
                model.ctrl_d_pending = keys.pending;
            }
        }
        terminal.draw(|f| render_tail(f, model))?;
    }
    model.ctrl_d_pending = false;
    if !model.exited {
        let _ = out.send(AttachOut::Resize {
            rows: orig_rows,
            cols: orig_cols,
        });
    }
    Ok(())
}

/// Read one chunk of keyboard bytes: wait for the first, then take whatever
/// follows within `KEY_CHUNK_GAP`, up to `KEY_CHUNK_MAX`.
async fn read_chunk(stdin: &crate::cli::input::AsyncStdin) -> Option<Vec<u8>> {
    let mut chunk = vec![stdin.read_byte().await?];
    while chunk.len() < KEY_CHUNK_MAX {
        match tokio::time::timeout(KEY_CHUNK_GAP, stdin.read_byte()).await {
            Ok(Some(b)) => chunk.push(b),
            _ => break,
        }
    }
    Some(chunk)
}

/// Run `/attach <id>`: open a `ShellTail` on the daemon, and only if it
/// answers with a screen, take over the alternate screen with keys going to
/// the shell until the user detaches. Input and resizes are sent, in order,
/// by one task, which is drained before this returns, so the size restore
/// lands before chat redraws.
pub async fn run_attach(
    stdin: &crate::cli::input::AsyncStdin,
    sigwinch: &mut tokio::signal::unix::Signal,
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    id: &str,
) -> anyhow::Result<()> {
    use crossterm::cursor::Show;
    use crossterm::execute;
    use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
    use tokio::io::AsyncBufReadExt;

    let stream = crate::cli::commands::ipc_client::connect().await?;
    let (rx, mut tx) = stream.into_split();
    crate::cli::commands::ipc_client::send_request(
        &mut tx,
        crate::ipc::Request::ShellTail {
            id: id.to_string(),
            follow: true,
        },
    )
    .await?;
    let mut frames = tokio::io::BufReader::new(rx).lines();
    let first = match frames.next_line().await? {
        Some(line) => serde_json::from_str::<Response>(&line)?,
        None => anyhow::bail!("the daemon closed the connection"),
    };
    let mut model = crate::cli::tail::open_tail(id, first).map_err(|msg| anyhow::anyhow!(msg))?;
    model.attached = true;

    let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<AttachOut>();
    let shell = id.to_string();
    let sender = tokio::spawn(async move {
        while let Some(item) = out_rx.recv().await {
            let req = match item {
                AttachOut::Input(bytes) => crate::ipc::Request::ShellInput {
                    id: shell.clone(),
                    bytes,
                },
                AttachOut::Resize { rows, cols } => crate::ipc::Request::ShellResize {
                    id: shell.clone(),
                    rows,
                    cols,
                },
            };
            let _ = crate::cli::commands::ipc_client::request_once(req).await;
        }
    });

    let result = {
        execute!(std::io::stdout(), EnterAlternateScreen)?;
        let _guard = crate::cli::viewer::AltScreenGuard::new(|| {
            let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
            let _ = execute!(std::io::stdout(), Show);
            renderer.reanchor();
        });
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(std::io::stdout()))?;
        let mut chunks = Box::pin(futures_util::stream::unfold(stdin, |stdin| async move {
            read_chunk(stdin).await.map(|chunk| (chunk, stdin))
        }));
        let mut resizes = Box::pin(futures_util::stream::unfold(sigwinch, |sig| async move {
            sig.recv().await.map(|()| ((), sig))
        }));
        attach_loop(
            &mut terminal,
            &mut model,
            &mut frames,
            &mut chunks,
            &mut resizes,
            &out_tx,
        )
        .await
    };
    drop(out_tx);
    let _ = sender.await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: ShellModes = ShellModes {
        bracketed_paste: false,
        application_cursor: false,
    };

    fn keys(chunk: &[u8], pending: bool, modes: ShellModes) -> (Vec<u8>, bool, bool) {
        let k = attach_keys(chunk, pending, modes);
        (k.send, k.detach, k.pending)
    }

    #[test]
    fn attach_keys_passes_bytes_and_drops_focus_reports() {
        assert_eq!(
            keys(b"ls -la\r", false, PLAIN),
            (b"ls -la\r".to_vec(), false, false)
        );
        assert_eq!(
            keys(b"\x12\t\x1bOP\x1b", false, PLAIN),
            (b"\x12\t\x1bOP\x1b".to_vec(), false, false),
            "control keys, F1 and a lone Esc go through untouched"
        );
        assert_eq!(
            keys(b"a\x1b[Ib\x1b[Oc", false, PLAIN),
            (b"abc".to_vec(), false, false),
            "focus reports are chat's, not the shell's"
        );
    }

    #[test]
    fn attach_keys_strips_paste_brackets_unless_the_shell_asked() {
        let paste = b"\x1b[200~echo hi\x1b[201~";
        assert_eq!(keys(paste, false, PLAIN).0, b"echo hi".to_vec());
        let bracketed = ShellModes {
            bracketed_paste: true,
            ..PLAIN
        };
        assert_eq!(keys(paste, false, bracketed).0, paste.to_vec());
    }

    #[test]
    fn attach_keys_application_cursor_rewrites_arrows() {
        let app = ShellModes {
            application_cursor: true,
            ..PLAIN
        };
        assert_eq!(
            keys(b"\x1b[A\x1b[D", false, app).0,
            b"\x1bOA\x1bOD".to_vec()
        );
        assert_eq!(
            keys(b"\x1b[A", false, PLAIN).0,
            b"\x1b[A".to_vec(),
            "normal cursor mode keeps CSI arrows"
        );
        assert_eq!(
            keys(b"\x1b[5~", false, app).0,
            b"\x1b[5~".to_vec(),
            "only the four arrows change"
        );
    }

    #[test]
    fn attach_keys_ctrl_d_detaches_and_double_ctrl_d_sends_eof() {
        assert_eq!(
            keys(b"\x04\x04", false, PLAIN),
            (vec![CTRL_D], false, false)
        );
        assert_eq!(
            keys(b"ab\x04", false, PLAIN),
            (b"ab".to_vec(), false, true),
            "a trailing ctrl-d waits for the next chunk"
        );
        assert_eq!(
            keys(b"xyz", true, PLAIN),
            (vec![], true, false),
            "after a pending ctrl-d, anything but ctrl-d detaches, and \
             nothing typed after it is sent"
        );
        assert_eq!(
            keys(b"\x04ls\r", true, PLAIN),
            (b"\x04ls\r".to_vec(), false, false),
            "a ctrl-d completing a pending one sends EOF, and typing goes on"
        );
        assert_eq!(
            keys(b"\x04", true, PLAIN),
            (vec![CTRL_D], false, false),
            "a second ctrl-d in the next chunk sends EOF"
        );
        assert_eq!(
            keys(b"ok\x04x", false, PLAIN),
            (b"ok".to_vec(), true, false),
            "bytes before a detaching ctrl-d are still sent"
        );
    }

    /// A model of a 24×80 shell, attached, with output already on screen.
    fn attached() -> TailModel {
        let mut m = TailModel::new("s3", 24, 80, b"$ ");
        m.attached = true;
        m
    }

    /// Drive the real loop with `chunks` sent at the given delays (ms after
    /// the previous one) and `frames` as the stream; return what it sent.
    async fn drive(
        model: &mut TailModel,
        chunks: Vec<(u64, Vec<u8>)>,
        frames: Vec<String>,
        keep_stream_open: bool,
    ) -> Vec<AttachOut> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use std::time::Duration;
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

        let (mut writer, client) = tokio::io::duplex(1 << 16);
        for frame in frames {
            writer
                .write_all(format!("{frame}\n").as_bytes())
                .await
                .unwrap();
        }
        // Dropping the writer closes the stream once its frames are read.
        let daemon = keep_stream_open.then_some(writer);
        let mut lines = tokio::io::BufReader::new(client).lines();
        let (key_tx, key_rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            for (delay, chunk) in chunks {
                tokio::time::sleep(Duration::from_millis(delay)).await;
                let _ = key_tx.send(chunk);
            }
            // Keep the key stream open; the loop must end on its own.
            std::future::pending::<()>().await;
        });
        let mut keys = Box::pin(futures_util::stream::unfold(key_rx, |mut rx| async move {
            rx.recv().await.map(|chunk| (chunk, rx))
        }));
        let mut resizes = Box::pin(futures_util::stream::pending::<()>());
        let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut terminal = Terminal::new(TestBackend::new(100, 31)).unwrap();
        tokio::time::timeout(
            Duration::from_secs(60),
            attach_loop(
                &mut terminal,
                model,
                &mut lines,
                &mut keys,
                &mut resizes,
                &out_tx,
            ),
        )
        .await
        .expect("the loop ends on its own")
        .expect("the loop ends cleanly");
        drop(daemon);
        drop(out_tx);
        let mut sent = Vec::new();
        while let Some(item) = out_rx.recv().await {
            sent.push(item);
        }
        sent
    }

    #[tokio::test(start_paused = true)]
    async fn attach_loop_resizes_sends_keys_and_restores_on_detach() {
        let mut model = attached();
        let sent = drive(
            &mut model,
            vec![(10, b"ls\r".to_vec()), (10, vec![CTRL_D])],
            vec![],
            true,
        )
        .await;
        assert_eq!(
            sent,
            [
                AttachOut::Resize {
                    rows: 30,
                    cols: 100
                },
                AttachOut::Input(b"ls\r".to_vec()),
                AttachOut::Resize { rows: 24, cols: 80 },
            ],
            "attach resizes to the terminal less the status line, sends the \
             keys, and a lone ctrl-d detaches after its window and restores \
             the size"
        );
        assert_eq!(model.screen().size(), (30, 100), "the grid followed");
        assert!(!model.ctrl_d_pending);
    }

    #[tokio::test(start_paused = true)]
    async fn attach_loop_double_ctrl_d_sends_eof_and_stays_attached() {
        let mut model = attached();
        let sent = drive(
            &mut model,
            vec![
                (10, vec![CTRL_D]),
                (100, vec![CTRL_D]),
                (1000, b"x".to_vec()),
                (10, b"\x04q".to_vec()),
            ],
            vec![],
            true,
        )
        .await;
        assert_eq!(
            sent,
            [
                AttachOut::Resize {
                    rows: 30,
                    cols: 100
                },
                AttachOut::Input(vec![CTRL_D]),
                AttachOut::Input(b"x".to_vec()),
                AttachOut::Resize { rows: 24, cols: 80 },
            ],
            "two ctrl-d inside the window send EOF and the session goes on; \
             a ctrl-d followed by another key detaches at once"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn attach_loop_draws_output_and_returns_on_a_key_after_exit() {
        let chunk = serde_json::to_string(&Response::ShellChunk {
            bytes: b"hello from s3\r\n".to_vec(),
        })
        .unwrap();
        let mut model = attached();
        let sent = drive(&mut model, vec![(50, b"x".to_vec())], vec![chunk], false).await;
        assert!(model.exited, "a closed stream marks the shell exited");
        assert!(
            model.screen().contents().contains("hello from s3"),
            "{:?}",
            model.screen().contents()
        );
        assert_eq!(
            sent,
            [AttachOut::Resize {
                rows: 30,
                cols: 100
            }],
            "after exit a key returns to chat; nothing is typed into a dead \
             shell and no size is restored"
        );
    }

    #[test]
    fn render_tail_shows_the_attached_status_line_and_cursor() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let bottom = |m: &TailModel, terminal: &mut Terminal<TestBackend>| -> String {
            terminal.draw(|f| render_tail(f, m)).unwrap();
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .skip(9 * 100)
                .take(100)
                .flat_map(|c| c.symbol().chars())
                .collect()
        };
        let mut terminal = Terminal::new(TestBackend::new(100, 10)).unwrap();
        let mut m = TailModel::new("s3", 9, 100, b"$ ls");
        m.attached = true;
        let line = bottom(&m, &mut terminal);
        assert!(line.starts_with("attached s3"), "{line:?}");
        assert!(
            line.contains("ctrl-d detach · ctrl-d ctrl-d EOF"),
            "{line:?}"
        );
        assert_eq!(
            terminal.get_cursor_position().unwrap(),
            ratatui::layout::Position::new(4, 0),
            "the cursor sits where the shell's is"
        );
        m.ctrl_d_pending = true;
        assert!(
            bottom(&m, &mut terminal).contains("ctrl-d again sends EOF"),
            "a pending ctrl-d says what the next one does"
        );
        m.ctrl_d_pending = false;
        m.exited = true;
        assert!(
            bottom(&m, &mut terminal).contains("exited · any key returns to chat"),
            "an exited shell says how to leave"
        );
    }
}
```

### Task 4 — `/attach` in chat (`src/cli/commands/chat.rs`)

```diff
--- a/src/cli/commands/chat.rs
+++ b/src/cli/commands/chat.rs
@@ -23,6 +23,16 @@ fn log_command(query: &str) -> Option<Option<&str>> {
     Some(words.next())
 }
 
+/// `/attach` and its shell id: `Some(Some(id))` for `/attach <id>`,
+/// `Some(None)` for a bare `/attach`, `None` for anything else.
+fn attach_command(query: &str) -> Option<Option<&str>> {
+    let mut words = query.split_whitespace();
+    if words.next() != Some("/attach") {
+        return None;
+    }
+    Some(words.next())
+}
+
 const HELP_TEXT: &str = "\
 Commands:
   /help      show this list           /exit      quit
@@ -34,6 +44,7 @@ Commands:
   /tail      live view of a shell     (alias /watch; f follow, esc to close)
   /logs      past shell recordings    [host] [--owner chat|ghost] [--since 2h]
   /log       open one recording       (/log <id>; n/p commands, space replays)
+  /attach    type into a live shell   (/attach <id>; ctrl-d detach, twice for EOF)
 
 Aliases: /models /panes /sessions /approval /new /quit (and help, ? open help).
 
@@ -456,6 +467,33 @@ async fn run_chat_ratatui(ctx: RatatuiCtx<'_>) -> Result<()> {
             );
             continue;
         }
+        if let Some(id) = attach_command(&query) {
+            match id {
+                Some(id) => {
+                    if let Err(e) =
+                        crate::cli::attach::run_attach(stdin, sigwinch, &mut renderer, id).await
+                    {
+                        let _ = renderer.commit(&format!("  ✗ attach {id}: {e}\n"));
+                    }
+                }
+                None => {
+                    let _ = renderer.commit("  usage: /attach <shell id>  (see /shells)\n");
+                }
+            }
+            redraw(
+                &mut renderer,
+                input_state,
+                &session_id,
+                approval,
+                &model,
+                prompt_tokens,
+                context_window,
+                daemon_up,
+                cost_usd,
+                has_untracked,
+            );
+            continue;
+        }
         if let Some(id) = log_command(&query) {
             match id {
                 Some(id) => {
@@ -1015,7 +1053,7 @@ fn turn_budget_label(turn: usize, prompt_tokens: u32, context_window: u32) -> St
 
 #[cfg(test)]
 mod tests {
-    use super::{HELP_TEXT, log_command};
+    use super::{HELP_TEXT, attach_command, log_command};
 
     #[test]
     fn help_text_lists_shells() {
@@ -1039,6 +1077,30 @@ mod tests {
         assert!(HELP_TEXT.contains("/hosts"), "HELP_TEXT must list '/hosts'");
     }
 
+    #[test]
+    fn attach_command_matches_attach_only() {
+        assert_eq!(
+            attach_command("/attach"),
+            Some(None),
+            "bare /attach asks for usage"
+        );
+        assert_eq!(attach_command("/attach s3"), Some(Some("s3")));
+        assert_eq!(attach_command("  /attach   s12 "), Some(Some("s12")));
+        for other in [
+            "/attachx s3",
+            "/attach-s3",
+            "attach s3",
+            "/agents",
+            "/tail s3",
+        ] {
+            assert_eq!(attach_command(other), None, "{other:?} is not /attach");
+        }
+        assert!(
+            HELP_TEXT.contains("/attach "),
+            "HELP_TEXT must list '/attach'"
+        );
+    }
+
     #[test]
     fn log_command_matches_log_and_not_logs() {
         assert_eq!(log_command("/log"), Some(None), "bare /log asks for usage");
```

### Task 5 — tests

All nine are above. Names pinned:
in `src/cli/attach.rs` —
`attach_keys_passes_bytes_and_drops_focus_reports`,
`attach_keys_strips_paste_brackets_unless_the_shell_asked`,
`attach_keys_application_cursor_rewrites_arrows`,
`attach_keys_ctrl_d_detaches_and_double_ctrl_d_sends_eof`,
`attach_loop_resizes_sends_keys_and_restores_on_detach`,
`attach_loop_double_ctrl_d_sends_eof_and_stays_attached`,
`attach_loop_draws_output_and_returns_on_a_key_after_exit`,
`render_tail_shows_the_attached_status_line_and_cursor`;
in `src/cli/commands/chat.rs` — `attach_command_matches_attach_only`.

**Test count:** nine new, so **2186 passed / 14 ignored**.

### Task 6 — capture the end-to-end evidence

Run the block in § End-to-end verification verbatim. Then create a new Update
Log entry titled exactly `### Update — <date> (end-to-end verification)`.
**Its first fenced block is the contents of `/tmp/e2e-m23-07a.txt`**: the
file's lines, copied by a tool. **Never paste the script itself**, and never
retype the lines (the real lines end `lib1=ok ` **with a trailing space**).
Then run the paste check once, put the line it prints into the entry **on its
own line**, and record the seven mutations after it.

## Acceptance criteria

Parenthesised values are what each line produces against the current tree.
Every non-preservation line fails today. Each line was run on the current
tree **and** on the dry-run tree.

- [ ] `cargo test --lib` → **2186 passed / 0 failed / 14 ignored** on three
      consecutive runs (current: 2177/0/14). Both counts are exact.
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → other targets unmoved: bug_tracker 6, doc_truth 14,
      integration 32, isolation 11 (*preservation*).
- [ ] The phase's tests pass:
      ``cargo test --lib -- cli::attach chat::tests::attach_command``
      → **9 passed** (current: 0). That exact filter printed 9 on the dry run.
- [ ] Wired, as the end-to-end block's `wired=` line prints it → **`1/1/1`**
      (current: `0/0/0`):
      ``grep -c 'crate::cli::attach::run_attach' src/cli/commands/chat.rs`` → 1;
      ``grep -c '^pub mod attach;' src/cli/mod.rs`` → 1;
      ``grep -c 'f.set_cursor_position' src/cli/tail.rs`` → 1.
- [ ] Mutations A–G were applied, run, and their observed results recorded
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
clippy-clean. All are in `src/cli/attach.rs` except G.

- **Mutation A** (focus reports): change
  `if rest.starts_with(b"\x1b[I") || rest.starts_with(b"\x1b[O") {` to
  `if rest.starts_with(b"\x1b[I") {`. Run
  `cli::attach::tests::attach_keys_passes_bytes_and_drops_focus_reports`.
  Expected **red**.
- **Mutation B** (paste brackets): change `            if modes.bracketed_paste {`
  to `            if true {`. Run
  `cli::attach::tests::attach_keys_strips_paste_brackets_unless_the_shell_asked`.
  Expected **red**.
- **Mutation C** (a trailing `Ctrl-d` waits): in the `None =>` arm of the
  `Ctrl-d` match, change `out.pending = true;` to `out.detach = true;`. Run
  `cli::attach::tests::attach_keys_ctrl_d_detaches_and_double_ctrl_d_sends_eof`.
  Expected **red**.
- **Mutation D** (size restored on detach): in the restore after the loop,
  swap the two fields, so `rows: orig_rows,` / `cols: orig_cols,` become
  `rows: orig_cols,` / `cols: orig_rows,`. Run
  `cli::attach::tests::attach_loop_resizes_sends_keys_and_restores_on_detach`.
  Expected **red**.
- **Mutation E** (the 400 ms window): change
  `ctrl_d_deadline = tokio::time::Instant::now() + CTRL_D_WINDOW;` to
  `ctrl_d_deadline = tokio::time::Instant::now();`. Run
  `cli::attach::tests::attach_loop_double_ctrl_d_sends_eof_and_stays_attached`.
  Expected **red**.
- **Mutation F** (no typing into an exited shell): in the `chunk =` branch,
  delete the three lines `if model.exited {` / `break;` / `}`. Run
  `cli::attach::tests::attach_loop_draws_output_and_returns_on_a_key_after_exit`.
  Expected **red**.
- **Mutation G** (cursor, `src/cli/tail.rs`): change
  `f.set_cursor_position((area.x + col, area.y + row));` to
  `f.set_cursor_position((area.x + row, area.y + col));`. Run
  `cli::attach::tests::render_tail_shows_the_attached_status_line_and_cursor`.
  Expected **red**.

After each restore, confirm `git diff --stat` matches its pre-mutation state.

## Test plan

Nine new tests, all without a daemon or a terminal:
- ordinary bytes, control keys, F1 and a lone Esc pass through unchanged;
  focus reports are dropped;
- paste brackets are dropped unless the shell enabled bracketed paste, and
  the pasted text is kept;
- with application cursor keys on, the four arrows become `ESC O x`, and
  other sequences do not change;
- `Ctrl-d Ctrl-d` sends EOF, a trailing `Ctrl-d` waits, a waiting `Ctrl-d`
  followed by anything else detaches and sends none of it, and bytes before a
  detaching `Ctrl-d` are sent;
- the real loop resizes to the terminal less the status line, sends keys,
  detaches on a lone `Ctrl-d` after its window, and restores the size;
- two `Ctrl-d`s inside the window send EOF and the session continues;
  `Ctrl-d` then a key detaches at once;
- output frames are drawn; once the shell exits, a key returns to chat and
  nothing is sent;
- the attached status line in all three states, and the cursor drawn at the
  shell's cursor position;
- `/attach` and its id are parsed, and `/attachx`, `/agents` and `/tail` are
  not `/attach`.

## End-to-end verification

```sh
cd /home/matt/src/daemoneye
{
  for i in 1 2 3; do
    out=$(cargo test --lib 2>&1)
    printf 'lib%d=%s %s\n' "$i" "$(printf '%s\n' "$out" | grep -E '^test result' | tail -1 | grep -oE 'ok|FAILED')" "$(printf '%s\n' "$out" | grep -E '^test .* FAILED$' | sed -E 's/^test (.*) \.\.\. FAILED$/\1/' | tr '\n' ' ')"
  done
  printf 'lib_counts=%s\n' "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1p-\2f-\3i/')"
  printf 'phase_tests=%s\n' "$(cargo test --lib -- cli::attach chat::tests::attach_command 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
  printf 'isolation=%s\n' "$(cargo test --test isolation 2>&1 | grep -E '^test result' | tail -1 | grep -oE '[0-9]+ passed; [0-9]+ failed')"
  printf 'wired=%s/%s/%s\n' "$(grep -c 'crate::cli::attach::run_attach' src/cli/commands/chat.rs)" "$(grep -c '^pub mod attach;' src/cli/mod.rs)" "$(grep -c 'f.set_cursor_position' src/cli/tail.rs)"
} > /tmp/e2e-m23-07a.txt
cat /tmp/e2e-m23-07a.txt
```

Expected:
- `lib1..3=ok` with **nothing after `ok`**;
- `lib_counts=2186p-0f-14i`;
- `phase_tests=9p-0f`;
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
entry's first fenced block the **contents of** `/tmp/e2e-m23-07a.txt`
(copied, not retyped; **never the script above**), and run:

```sh
D=docs/dev/milestones/M23-client-decoupling/phase-07a-attach-mode.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m23-07a.txt
diff /tmp/pasted-m23-07a.txt /tmp/e2e-m23-07a.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

If it prints `PASTE MISMATCH`, run
`diff /tmp/pasted-m23-07a.txt /tmp/e2e-m23-07a.txt` once and fix what it
shows. **Run the check at most three times.** Put the line it prints into the
entry on its own line.

## Authorizations

You may edit these files, and nothing else:

- `src/cli/tail.rs`, `src/cli/mod.rs`, `src/cli/commands/chat.rs`: the diffs
  above.
- `src/cli/attach.rs`: create it with the content above.
- This phase doc's Update Log.

**Do not run `daemoneye chat`, `daemoneye daemon`, or anything that sends
signals or kills processes** (no `kill`, `pkill`, `tmux kill-*`). Every test
here is in-process.

## Out of scope

- **The `/agents` picker** and a bare `/attach` opening it: 07b.
- **`Ctrl-p` pause/resume and `Ctrl-c` cooperative cancel**, logging
  attached input to `events.jsonl`, telling the owning agent, and scrubbing
  at a secret prompt: row 08.
- **Scrollback while attached**: every key goes to the shell. `/tail` is
  the way to scroll.
- **README.md**: documented at the milestone close.

## Update Log

<!-- entries appended below this line -->

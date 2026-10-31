//! Attached mode (`/attach <sN>`): the user's keystrokes go to a live PTY
//! shell as `ShellInput`, the terminal size follows as `ShellResize`, and the
//! shell's output is drawn locally through the `/tail` model.
//!
//! The byte rules are pure (`attach_keys`), and the loop is generic over its
//! sources and its output channel, so both are tested without a daemon or a
//! terminal.

use crate::cli::tail::{TailModel, render_tail};
use crate::ipc::Response;
use crate::shell::keys::ShellModes;

/// How long a first `Ctrl-d` waits for a second before it detaches.
pub const CTRL_D_WINDOW: std::time::Duration = std::time::Duration::from_millis(400);

/// The most bytes one read of the keyboard gathers into a single `ShellInput`.
const KEY_CHUNK_MAX: usize = 4096;

/// How long a read waits for more bytes after the first, so an escape
/// sequence or a paste arrives as one chunk.
const KEY_CHUNK_GAP: std::time::Duration = std::time::Duration::from_millis(5);

const CTRL_D: u8 = 0x04;
const CTRL_C: u8 = 0x03;
const CTRL_P: u8 = 0x10;

/// What the attach loop sends to the daemon, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachOut {
    Input(Vec<u8>),
    Resize {
        rows: u16,
        cols: u16,
    },
    /// A `ShellSignal`: `"STOP"`, `"CONT"` or `"INT"`.
    Signal(&'static str),
}

/// The daemon's answer to an `AttachOut::Signal`, fed back into the loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachReply {
    /// The state the shell settled in (`Response::ShellSignalled`), and the
    /// run an `INT` cancelled with it, if any.
    Signalled {
        state: String,
        cancelled_run: Option<String>,
    },
    /// The signal was refused or failed.
    Failed(String),
}

/// A chord attached mode takes for itself instead of sending it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chord {
    /// `Ctrl-p`: pause the foreground job, or resume it if paused.
    TogglePause,
    /// `Ctrl-c`: interrupt the foreground job.
    Interrupt,
}

/// What one chunk of keyboard bytes comes to: the bytes to send, whether to
/// detach, whether a trailing `Ctrl-d` now waits for a second, and a chord
/// the user pressed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AttachKeys {
    pub send: Vec<u8>,
    pub detach: bool,
    pub pending: bool,
    pub chord: Option<Chord>,
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
/// - `Ctrl-p` and `Ctrl-c` are chords, not bytes: the bytes before one are
///   sent, the chord is returned, and the rest of the chunk is dropped.
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
        if rest[0] == CTRL_P || rest[0] == CTRL_C {
            out.chord = Some(if rest[0] == CTRL_P {
                Chord::TogglePause
            } else {
                Chord::Interrupt
            });
            return out;
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

/// The status note for a signal's answer: `sent` is the signal it answers,
/// `paused` whether the shell is now paused, `cancelled_run` the run an `INT`
/// cancelled.
pub fn reply_note(sent: Option<&str>, paused: bool, cancelled_run: Option<&str>) -> Option<String> {
    match sent {
        Some("STOP") if !paused => Some("nothing running to pause".to_string()),
        Some("INT") => Some(match cancelled_run {
            Some(run) => format!("interrupted · cancelled run {run}"),
            None => "interrupted".to_string(),
        }),
        _ => None,
    }
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
    replies: &mut tokio::sync::mpsc::UnboundedReceiver<AttachReply>,
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
    let (orig_rows, orig_cols) = (orig_rows.get(), orig_cols.get());
    let size = terminal.size()?;
    let (rows, cols) = body_size(size.height, size.width);
    model.resize(rows, cols);
    let _ = out.send(AttachOut::Resize { rows, cols });
    terminal.draw(|f| render_tail(f, model))?;

    let mut stream_open = true;
    // When a pending `Ctrl-d` detaches if no second one comes.
    let mut ctrl_d_deadline = tokio::time::Instant::now();
    // The signals sent and not yet answered, oldest first.
    let mut asked: std::collections::VecDeque<&'static str> = std::collections::VecDeque::new();
    loop {
        tokio::select! {
            reply = replies.recv() => {
                let Some(reply) = reply else { continue };
                let sent = asked.pop_front();
                match reply {
                    AttachReply::Signalled { state, cancelled_run } => {
                        model.paused = state == "Paused";
                        model.note = reply_note(sent, model.paused, cancelled_run.as_deref());
                    }
                    AttachReply::Failed(msg) => model.note = Some(msg),
                }
            }
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
                        model.paused = false;
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
                // A note lasts until the user types or presses a chord.
                if !keys.send.is_empty() || keys.chord.is_some() {
                    model.note = None;
                }
                if !keys.send.is_empty() {
                    let _ = out.send(AttachOut::Input(keys.send));
                }
                if keys.detach {
                    break;
                }
                if let Some(chord) = keys.chord {
                    let signal = match chord {
                        Chord::TogglePause if model.paused => "CONT",
                        Chord::TogglePause => "STOP",
                        Chord::Interrupt => "INT",
                    };
                    asked.push_back(signal);
                    let _ = out.send(AttachOut::Signal(signal));
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
    if model.paused && !model.exited {
        // Leaving a job stopped with no attached view to resume it from
        // would strand it: detaching resumes it.
        let _ = out.send(AttachOut::Signal("CONT"));
        model.paused = false;
    }
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
    let (reply_tx, mut reply_rx) = tokio::sync::mpsc::unbounded_channel::<AttachReply>();
    let shell = id.to_string();
    let sender = tokio::spawn(async move {
        while let Some(item) = out_rx.recv().await {
            let is_signal = matches!(item, AttachOut::Signal(_));
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
                AttachOut::Signal(signal) => crate::ipc::Request::ShellSignal {
                    id: shell.clone(),
                    signal: signal.to_string(),
                },
            };
            let reply = crate::cli::commands::ipc_client::request_once(req).await;
            if is_signal {
                let reply = match reply {
                    Ok(Response::ShellSignalled {
                        state,
                        cancelled_run,
                    }) => AttachReply::Signalled {
                        state,
                        cancelled_run,
                    },
                    Ok(Response::Error(msg)) => AttachReply::Failed(msg),
                    Ok(other) => AttachReply::Failed(format!("unexpected reply {}", other.kind())),
                    Err(e) => AttachReply::Failed(e.to_string()),
                };
                let _ = reply_tx.send(reply);
            }
        }
    });

    let result = {
        let (_guard, mut terminal) = crate::cli::viewer::enter_alt_screen(renderer)?;
        let mut chunks = Box::pin(futures_util::stream::unfold(stdin, |stdin| async move {
            read_chunk(stdin).await.map(|chunk| (chunk, stdin))
        }));
        let mut resizes = crate::cli::viewer::resize_events(sigwinch);
        attach_loop(
            &mut terminal,
            &mut model,
            &mut frames,
            &mut chunks,
            &mut resizes,
            &out_tx,
            &mut reply_rx,
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
    /// the previous one) and `frames` as the stream; return what it sent. A
    /// fake daemon answers each signal: `STOP` settles in `stop_state`,
    /// `CONT` in `Running`, `INT` in `Idle`.
    async fn drive(
        model: &mut TailModel,
        chunks: Vec<(u64, Vec<u8>)>,
        frames: Vec<String>,
        keep_stream_open: bool,
        stop_state: &'static str,
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
        let (reply_tx, mut reply_rx) = tokio::sync::mpsc::unbounded_channel();
        let responder = tokio::spawn(async move {
            let mut sent = Vec::new();
            while let Some(item) = out_rx.recv().await {
                if let AttachOut::Signal(signal) = &item {
                    let state = match *signal {
                        "STOP" => stop_state,
                        "CONT" => "Running",
                        _ => "Idle",
                    };
                    let _ = reply_tx.send(AttachReply::Signalled {
                        state: state.to_string(),
                        cancelled_run: None,
                    });
                }
                sent.push(item);
            }
            sent
        });
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
                &mut reply_rx,
            ),
        )
        .await
        .expect("the loop ends on its own")
        .expect("the loop ends cleanly");
        drop(daemon);
        drop(out_tx);
        responder.await.unwrap()
    }

    #[tokio::test(start_paused = true)]
    async fn attach_loop_resizes_sends_keys_and_restores_on_detach() {
        let mut model = attached();
        let sent = drive(
            &mut model,
            vec![(10, b"ls\r".to_vec()), (10, vec![CTRL_D])],
            vec![],
            true,
            "Paused",
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
        assert_eq!(
            model.screen().size(),
            (
                core::num::NonZeroU16::new(30).unwrap_or(core::num::NonZeroU16::MIN),
                core::num::NonZeroU16::new(100).unwrap_or(core::num::NonZeroU16::MIN)
            ),
            "the grid followed"
        );
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
            "Paused",
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
        let sent = drive(
            &mut model,
            vec![(50, b"x".to_vec())],
            vec![chunk],
            false,
            "Paused",
        )
        .await;
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
    fn attach_keys_takes_ctrl_p_and_ctrl_c_as_chords() {
        let k = attach_keys(b"ab\x10cd", false, PLAIN);
        assert_eq!(
            (k.send, k.chord),
            (b"ab".to_vec(), Some(Chord::TogglePause)),
            "bytes before ctrl-p are sent; the rest of the chunk is dropped"
        );
        let k = attach_keys(b"\x03", false, PLAIN);
        assert_eq!((k.send, k.chord), (vec![], Some(Chord::Interrupt)));
        let k = attach_keys(b"\x10", true, PLAIN);
        assert!(
            k.detach && k.chord.is_none(),
            "after a pending ctrl-d, ctrl-p detaches like any other key"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn attach_loop_ctrl_p_pauses_resumes_and_detach_resumes() {
        let mut model = attached();
        let sent = drive(
            &mut model,
            vec![
                (10, vec![CTRL_P]),
                (100, vec![CTRL_P]),
                (100, vec![CTRL_P]),
                (100, vec![CTRL_D]),
            ],
            vec![],
            true,
            "Paused",
        )
        .await;
        assert_eq!(
            sent,
            [
                AttachOut::Resize {
                    rows: 30,
                    cols: 100
                },
                AttachOut::Signal("STOP"),
                AttachOut::Signal("CONT"),
                AttachOut::Signal("STOP"),
                AttachOut::Signal("CONT"),
                AttachOut::Resize { rows: 24, cols: 80 },
            ],
            "ctrl-p stops, ctrl-p again continues, and detaching while paused \
             resumes the job before the size is restored"
        );
        assert!(!model.paused);
    }

    #[tokio::test(start_paused = true)]
    async fn attach_loop_ctrl_p_with_nothing_running_says_so() {
        let mut model = attached();
        let sent = drive(
            &mut model,
            vec![(10, vec![CTRL_P]), (100, vec![CTRL_D])],
            vec![],
            true,
            "Idle",
        )
        .await;
        assert_eq!(
            sent,
            [
                AttachOut::Resize {
                    rows: 30,
                    cols: 100
                },
                AttachOut::Signal("STOP"),
                AttachOut::Resize { rows: 24, cols: 80 },
            ],
            "a STOP that paused nothing leaves nothing to resume on detach"
        );
        assert!(!model.paused, "only a Paused answer marks the shell paused");
        assert_eq!(model.note.as_deref(), Some("nothing running to pause"));
    }

    #[tokio::test(start_paused = true)]
    async fn attach_loop_ctrl_c_interrupts() {
        let mut model = attached();
        let sent = drive(
            &mut model,
            vec![
                (10, b"sleep 9\r".to_vec()),
                (10, vec![CTRL_C]),
                (100, vec![CTRL_D]),
            ],
            vec![],
            true,
            "Paused",
        )
        .await;
        assert_eq!(
            sent,
            [
                AttachOut::Resize {
                    rows: 30,
                    cols: 100
                },
                AttachOut::Input(b"sleep 9\r".to_vec()),
                AttachOut::Signal("INT"),
                AttachOut::Resize { rows: 24, cols: 80 },
            ],
            "ctrl-c is a signal, not a typed byte"
        );
        assert_eq!(model.note.as_deref(), Some("interrupted"));
    }

    #[test]
    fn reply_note_names_what_a_signal_did() {
        assert_eq!(
            reply_note(Some("STOP"), false, None).as_deref(),
            Some("nothing running to pause")
        );
        assert_eq!(reply_note(Some("STOP"), true, None), None, "PAUSED says it");
        assert_eq!(reply_note(Some("CONT"), false, None), None);
        assert_eq!(
            reply_note(Some("INT"), false, None).as_deref(),
            Some("interrupted")
        );
        assert_eq!(
            reply_note(Some("INT"), false, Some("0123456789abcdef")).as_deref(),
            Some("interrupted · cancelled run 0123456789abcdef"),
            "an INT that cancelled the owning run names it"
        );
    }

    #[test]
    fn render_tail_shows_the_paused_status_line() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut terminal = Terminal::new(TestBackend::new(110, 10)).unwrap();
        let mut m = TailModel::new("s3", 9, 100, b"$ ");
        m.attached = true;
        let bottom = |m: &TailModel, terminal: &mut Terminal<TestBackend>| -> String {
            terminal.draw(|f| render_tail(f, m)).unwrap();
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .skip(9 * 110)
                .take(110)
                .flat_map(|c| c.symbol().chars())
                .collect()
        };
        let line = bottom(&m, &mut terminal);
        assert!(line.contains("ctrl-p pause · ctrl-c interrupt"), "{line:?}");
        m.paused = true;
        let line = bottom(&m, &mut terminal);
        assert!(
            line.contains("PAUSED · ctrl-p resume · ctrl-d detach (resumes)"),
            "{line:?}"
        );
        m.paused = false;
        m.note = Some("nothing running to pause".to_string());
        let line = bottom(&m, &mut terminal);
        assert!(line.contains("nothing running to pause"), "{line:?}");
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

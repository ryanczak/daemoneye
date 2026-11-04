//! `daemoneye top`'s attach view: one selected shell, watched live and
//! read-only through the same `ShellTail` stream and model `/tail` uses.
//!
//! Opening sends one `Request::ShellTail { id, follow: true }` on its own
//! daemon connection, builds the [`TailModel`] from the first frame as
//! [`open_tail`] does, and spawns one forwarding task that reads the
//! connection's lines and sends each parsed [`Response`] on an `mpsc`
//! channel — the same channel pattern `run_tui` already uses for its
//! crossterm reader and its disk refresh. The view closes on `q`, `Esc` or
//! `Ctrl-O` (`tail_key_action` decides what a key means) and returns to the
//! panels exactly as they were. No key reaches the shell as input, a signal
//! or a resize: the view sends nothing after the one `ShellTail` request.

use super::*;
use crate::cli::tail::TailModel;
use crate::ipc::Response;

/// The open view, boxed so `TopModel` stays small.
pub struct AttachBox {
    /// The open attach view.
    pub view: AttachView,
}

/// The open attach view. `tail` holds the shell's grid and the `/tail`
/// viewer's state; `closed` is the forwarding task's abort handle, dropped
/// or aborted when the view closes.
pub struct AttachView {
    /// The `/tail` model over the shell's stream; its `id` names the shell.
    pub tail: TailModel,
    /// The forwarding task; aborted when the view closes.
    pub task: tokio::task::JoinHandle<()>,
    /// The task's frames; closed when the stream ends.
    pub frames: tokio::sync::mpsc::UnboundedReceiver<Response>,
}

impl AttachView {
    /// Apply one frame from the channel: `Some` feeds the model, `None`
    /// marks the stream ended and keeps the view open. The select arm calls
    /// this, and the stream test calls it instead of setting `exited` by
    /// hand.
    pub fn apply_frame(&mut self, frame: Option<Response>) {
        match frame {
            Some(frame) => {
                self.tail.apply(&frame);
            }
            None => self.tail.exited = true,
        }
    }
}

/// What opening came to: the view, or why it did not open.
pub enum AttachOpen {
    /// The view opened on the shell.
    Opened(Box<AttachView>),
    /// The connection or the first frame failed; the message names it.
    Failed(String),
}

/// Open the attach view on `id`: one read-only `ShellTail` request on its
/// own daemon connection, the model built from the first frame, and one
/// forwarding task reading the rest onto a channel. Any failure before the
/// first frame is [`AttachOpen::Failed`]; the panels stay and the footer
/// shows the message.
pub(crate) async fn open_attach(id: &str) -> AttachOpen {
    match open_attach_frames(id).await {
        Ok((tail, frames_rx, task)) => AttachOpen::Opened(Box::new(AttachView {
            tail,
            task,
            frames: frames_rx,
        })),
        Err(msg) => AttachOpen::Failed(msg),
    }
}

/// The connection half of [`open_attach`]: the `ShellTail` request, the
/// first frame into a `TailModel`, and the forwarding task on an `mpsc`
/// channel. Split out so a test can drive the task with a duplex.
pub(crate) async fn open_attach_frames(
    id: &str,
) -> Result<
    (
        TailModel,
        tokio::sync::mpsc::UnboundedReceiver<Response>,
        tokio::task::JoinHandle<()>,
    ),
    String,
> {
    use tokio::io::AsyncBufReadExt;

    let stream = crate::cli::commands::ipc_client::connect()
        .await
        .map_err(|e| e.to_string())?;
    let (rx, mut tx) = stream.into_split();
    crate::cli::commands::ipc_client::send_request(
        &mut tx,
        crate::ipc::Request::ShellTail {
            id: id.to_string(),
            follow: true,
        },
    )
    .await
    .map_err(|e| e.to_string())?;
    let mut lines = tokio::io::BufReader::new(rx).lines();
    let first = match lines.next_line().await {
        Ok(Some(line)) => line,
        Ok(None) => "the daemon closed the connection".to_string(),
        Err(e) => e.to_string(),
    };
    let first: Response = serde_json::from_str(&first)
        .map_err(|e| format!("the daemon's reply did not parse: {e}"))?;
    let model = crate::cli::tail::open_tail(id, first)?;
    let (frame_tx, frame_rx) = tokio::sync::mpsc::unbounded_channel();
    let task = tokio::spawn(forward_frames(lines, frame_tx));
    Ok((model, frame_rx, task))
}

/// Read a frame stream's lines and send each parsed [`Response`] on `tx`;
/// the channel closing ends the task. Generic over the reader so a test
/// drives it with a duplex.
pub(crate) async fn forward_frames<R>(
    mut lines: tokio::io::Lines<R>,
    tx: tokio::sync::mpsc::UnboundedSender<Response>,
) where
    R: tokio::io::AsyncBufRead + Unpin,
{
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(frame) = serde_json::from_str::<Response>(&line) else {
            continue;
        };
        if tx.send(frame).is_err() {
            break;
        }
    }
}

/// Translate one crossterm key event to a `cli::input::Key`, as the tail
/// viewer's key table expects. `None` for everything the tail view ignores
/// (release and repeat events, plain modifiers, F-keys).
pub(crate) fn translate_key(key: KeyEvent) -> Option<crate::cli::input::Key> {
    use crate::cli::input::Key;

    if key.kind != KeyEventKind::Press {
        return None;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    Some(match key.code {
        KeyCode::Char(c) if ctrl && c == 'c' => Key::CtrlC,
        KeyCode::Char(c) if ctrl && c == 'o' => Key::CtrlO,
        KeyCode::Char(_c) if ctrl => None?,
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Enter => Key::Enter,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Delete => Key::Delete,
        KeyCode::Esc => Key::Char('\x1b'),
        _ => None?,
    })
}

/// Apply one key to the attach view through the tail viewer's own key table.
/// Returns `false` when the view should close (`q`, `Esc`, `Ctrl-O`);
/// `Ctrl-C` quits `top` in every mode, so it closes the view and the loop
/// acts on `model.quit`.
pub(crate) fn attach_key(view: &mut AttachView, key: KeyEvent, page: usize) -> bool {
    let Some(key) = translate_key(key) else {
        return true;
    };
    let action = crate::cli::tail::tail_key_action(&key, view.tail.typing());
    view.tail.note = None;
    view.tail.act(action, page)
}

#[cfg(test)]
mod tests {
    use super::super::tests::{
        activity_row, press, row, shell, snapshot, snapshot_with_all, snapshot_with_shells,
    };
    use super::*;
    use ratatui::backend::TestBackend;

    /// A view over `first`'s frame, with a forwarding task driven by a
    /// duplex the test writes to. Called only inside `#[tokio::test]`, where
    /// `tokio::spawn` has a runtime.
    fn opened(id: &str, first: Response) -> (AttachView, tokio::io::DuplexStream) {
        let (daemon, client) = tokio::io::duplex(1 << 16);
        let (frame_tx, frame_rx) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            forward_frames(tokio::io::BufReader::new(client).lines(), frame_tx).await
        });
        let model = crate::cli::tail::open_tail(id, first).expect("the first frame opens");
        (
            AttachView {
                tail: model,
                task,
                frames: frame_rx,
            },
            daemon,
        )
    }

    // ── the trigger ──────────────────────────────────────────────────────

    #[test]
    fn top_attach_enter_or_a_opens_only_on_a_selected_shell() {
        let mut model = TopModel::new(snapshot_with_shells(vec![], vec![shell("s1"), shell("s2")]));
        model.focus = Panel::Shells;
        assert_eq!(
            model.attach_candidate(),
            None,
            "no selection yet, nothing to open"
        );
        press(&mut model, KeyCode::Down);
        assert_eq!(
            model.attach_candidate(),
            Some("s2".to_string()),
            "Enter on Shells names the selected shell"
        );
        press(&mut model, KeyCode::Char('a'));
        assert_eq!(
            model.attach_candidate(),
            Some("s2".to_string()),
            "'a' names the selected shell too"
        );
        // Runs focused: Enter opens a run detail, not a shell.
        let mut model = TopModel::new(snapshot_with_shells(
            vec![row("r01", Liveness::Live)],
            vec![shell("s1")],
        ));
        model.focus = Panel::Runs;
        assert_eq!(
            model.attach_candidate(),
            None,
            "Runs focused, Enter is the run detail's"
        );
        // No rows: nothing to open.
        let mut model = TopModel::new(snapshot_with_shells(vec![], vec![]));
        model.focus = Panel::Shells;
        assert_eq!(
            model.attach_candidate(),
            None,
            "an empty Shells panel has nothing to open"
        );
        // Editing the filter: keys are text.
        let mut model = TopModel::new(snapshot_with_shells(vec![], vec![shell("s1")]));
        model.focus = Panel::Shells;
        press(&mut model, KeyCode::Char('/'));
        assert!(model.editing, "setup: the filter is being edited");
        assert_eq!(
            model.attach_candidate(),
            None,
            "editing the filter, Enter is text, not an open"
        );
    }

    // ── the keys ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn top_attach_keys_close_on_q_esc_ctrl_o_and_ctrl_c_quits_top() {
        let (mut view, _daemon) = opened(
            "s3",
            Response::ShellScreen {
                bytes: b"hi".to_vec(),
                rows: 5,
                cols: 20,
            },
        );
        let page = 10;
        // History to scroll back through: "hi" alone has none, so ↑ would
        // not leave the newest output and follow would stay on.
        view.tail.apply(&Response::ShellChunk {
            bytes: test_lines(30),
        });
        // Scroll, follow, and search typing go through tail_key_action.
        assert!(attach_key(
            &mut view,
            KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
            page
        ));
        assert!(!view.tail.follow, "↑ scrolled back, follow paused");
        assert!(
            attach_key(
                &mut view,
                KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE),
                page
            ),
            "f resumes follow"
        );
        assert!(view.tail.follow, "follow is back on");
        assert!(attach_key(
            &mut view,
            KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
            page
        ));
        assert!(view.tail.typing(), "/ opens the search");
        assert!(
            attach_key(
                &mut view,
                KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
                page
            ),
            "typing into the search keeps the view"
        );
        assert!(
            view.tail.search.as_ref().map(|s| s.query.clone()) == Some("x".to_string()),
            "the search holds the typed text"
        );
        assert!(
            attach_key(
                &mut view,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                page
            ),
            "Enter commits the search and keeps the view"
        );
        assert!(!view.tail.typing(), "Enter ended the typing");
        for code in [KeyCode::Char('q'), KeyCode::Esc] {
            let (mut view, _daemon) = opened(
                "s3",
                Response::ShellScreen {
                    bytes: b"".to_vec(),
                    rows: 5,
                    cols: 20,
                },
            );
            assert!(
                !attach_key(&mut view, KeyEvent::new(code, KeyModifiers::NONE), page),
                "{code:?} closes the view"
            );
        }
        // Ctrl-O closes; Ctrl-C closes the view and quits top.
        let (mut view, _daemon) = opened(
            "s3",
            Response::ShellScreen {
                bytes: b"".to_vec(),
                rows: 5,
                cols: 20,
            },
        );
        assert!(
            !attach_key(
                &mut view,
                KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL),
                page
            ),
            "Ctrl-O closes the view"
        );
        let (mut view, _daemon) = opened(
            "s3",
            Response::ShellScreen {
                bytes: b"".to_vec(),
                rows: 5,
                cols: 20,
            },
        );
        assert!(
            !attach_key(
                &mut view,
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                page
            ),
            "Ctrl-C closes the view"
        );
        let mut model = TopModel::new(snapshot(vec![]));
        press_ctrl_c(&mut model);
        assert!(model.quit, "Ctrl-C quits top in every mode");
    }

    fn press_ctrl_c(model: &mut TopModel) {
        model.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    }

    fn test_lines(n: usize) -> Vec<u8> {
        (0..n)
            .map(|i| format!("line {i}\r\n"))
            .collect::<String>()
            .into_bytes()
    }

    // ── the stream ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn top_attach_stream_end_marks_exited_and_keeps_the_view() {
        use tokio::io::AsyncWriteExt;

        let (mut view, mut daemon) = opened(
            "s3",
            Response::ShellScreen {
                bytes: b"first".to_vec(),
                rows: 5,
                cols: 20,
            },
        );
        let frame = serde_json::to_string(&Response::ShellChunk {
            bytes: b"live\r\n".to_vec(),
        })
        .unwrap();
        daemon
            .write_all(format!("{frame}\n").as_bytes())
            .await
            .unwrap();
        // A closed writer closes the channel: the stream ends. Give the
        // task a moment to drain, then apply what the channel holds —
        // `None` marks the model exited, exactly as the select arm does.
        drop(daemon);
        let mut frames = 0;
        let mut ended = false;
        while !ended {
            match view.frames.try_recv() {
                Ok(f) => {
                    frames += 1;
                    view.apply_frame(Some(f));
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    ended = true;
                }
            }
        }
        view.apply_frame(None);
        assert!(
            frames == 1,
            "the duplex's one chunk frame reached the model: {frames}"
        );
        assert!(view.tail.exited, "the closed stream marks the model exited");
        assert!(
            view.tail.screen().contents().contains("live"),
            "the frame was on the grid"
        );
        // The view stays open: drawing still works and no key has closed it.
        let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();
        terminal
            .draw(|f| crate::cli::tail::render_tail(f, &view.tail))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .flat_map(|c| c.symbol().chars())
            .collect();
        assert!(text.contains("exited"), "the view says exited: {text:?}");
        view.task.abort();
    }

    // ── opening failures ─────────────────────────────────────────────────

    #[test]
    fn top_attach_open_error_leaves_the_panels_and_names_it() {
        let mut model = TopModel::new(snapshot_with_shells(vec![], vec![shell("s1")]));
        model.focus = Panel::Shells;
        press(&mut model, KeyCode::Down);
        model.attach_note = Some("no live shell s9".to_string());
        assert!(
            model.attach.is_none(),
            "the panels stay when opening failed"
        );
        assert_eq!(
            model.attach_note.as_deref(),
            Some("no live shell s9"),
            "the footer names the failure"
        );
        press(&mut model, KeyCode::Down);
        assert!(model.attach_note.is_none(), "the next key clears the note");
    }

    // ── the drawn view and the restore ───────────────────────────────────

    #[tokio::test]
    async fn top_attach_view_draws_the_shell_and_close_restores_the_panels() {
        let mut model = TopModel::new(snapshot_with_all(
            vec![row("r01", Liveness::Live), row("r02", Liveness::Done)],
            vec![activity_row("09:00:00", "ai_cost", "model01")],
            vec![shell("s1"), shell("s2"), shell("s3")],
        ));
        model.focus = Panel::Shells;
        model.heights = PanelHeights {
            runs: 3,
            shells: 4,
            activity: 10,
            detail: 0,
        };
        // Move the selection and scroll the panels so the restore has
        // something to restore.
        press(&mut model, KeyCode::Down);
        model.activity_top = 0;
        model.tailing = false;
        model.clamp_offsets();
        let (focus, shells_selected, shells_top, activity_top, selected, tailing) = (
            model.focus,
            model.shells_selected,
            model.shells_top,
            model.activity_top,
            model.selected,
            model.tailing,
        );
        // Open the view on the selected shell.
        let id = model
            .attach_candidate()
            .expect("setup: a shell is selected");
        let (view, _daemon) = opened(
            &id,
            Response::ShellScreen {
                bytes: b"hello shell\r\n".to_vec(),
                rows: 5,
                cols: 20,
            },
        );
        model.attach = Some(AttachBox { view });
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| draw(f, &model)).unwrap();
        let text: String = terminal
            .backend()
            .to_string()
            .lines()
            .map(|l| format!("{l}\n"))
            .collect();
        assert!(
            text.contains("hello shell"),
            "the shell's grid is on screen: {text}"
        );
        assert!(
            text.contains(&format!("tail {id}")),
            "the status line names the shell: {text}"
        );
        assert!(
            !text.contains(" 1 Runs "),
            "the panels are not drawn behind the view: {text}"
        );
        assert!(
            !text.contains(" 3 Shells "),
            "the panels are not drawn behind the view: {text}"
        );
        // Close with Esc: the panels come back exactly as they were.
        press(&mut model, KeyCode::Esc);
        assert!(model.attach.is_none(), "Esc closed the view");
        assert_eq!(model.focus, focus, "Esc restores the focus");
        assert_eq!(
            model.shells_selected, shells_selected,
            "Esc restores the shell selection"
        );
        assert_eq!(model.shells_top, shells_top, "Esc restores Shells' window");
        assert_eq!(
            model.activity_top, activity_top,
            "Esc restores Activity's offset"
        );
        assert_eq!(model.selected, selected, "Esc restores the run selection");
        assert_eq!(model.tailing, tailing, "Esc restores Activity's tailing");
        terminal.draw(|f| draw(f, &model)).unwrap();
        let text: String = terminal
            .backend()
            .to_string()
            .lines()
            .map(|l| format!("{l}\n"))
            .collect();
        assert!(text.contains(" 1 Runs "), "the panels are back: {text}");
    }
}

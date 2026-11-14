//! Streaming response rendering for chat and ask flows.
//!
//! Owns `ask_with_session_ratatui`, the long-lived loop that consumes `Response`
//! events from the daemon and renders them via the ratatui inline-viewport
//! renderer (tokens, tool panels, approval prompts).

use anyhow::Result;
use tokio::io::BufReader;

use crate::cli::input::*;
use crate::cli::markdown::MarkdownRenderer;
use crate::ipc::{APPROVAL_TIMEOUT, Request, Response};

use super::approval::SessionApproval;
use super::interrupt::{InterruptAction, InterruptState};
use super::ipc_client::{connect, send_request};

/// Client-side silence bounds, derived from the daemon's
/// `KEEPALIVE_PERIOD_SECS` (15 s) with >= 6x margin: while a turn is in
/// flight the daemon sends *something* at least every 15 s, so 90 s of
/// total silence before the first content means the daemon is hung, not
/// slow. Phase 2 keeps the pre-existing 120 s.
const PHASE1_SILENCE_TIMEOUT_SECS: u64 = 90;
const PHASE2_SILENCE_TIMEOUT_SECS: u64 = 120;

/// Outcome of the inner streaming loop (daemon message or user interrupt).
#[derive(Debug)]
enum StreamOutcome {
    /// A daemon message arrived.
    Msg(Box<Response>),
    /// Spinner tick — caller should animate.
    Tick,
    /// First interrupt press — caller should show a warning.
    Warn,
    /// User aborted the turn.
    Interrupted,
    /// A resize or focus-gain arrived mid-stream — caller must re-anchor.
    Reanchor,
    /// Ctrl+O — caller opens the transcript viewer, then resumes the read.
    OpenViewer,
    /// Daemon error (EOF, parse failure, timeout).
    Error(String),
    /// Deadline expired before a daemon message (phase-accurate timeout).
    Deadline,
}

fn silence_budget(response_started: bool) -> std::time::Duration {
    std::time::Duration::from_secs(if response_started {
        PHASE2_SILENCE_TIMEOUT_SECS
    } else {
        PHASE1_SILENCE_TIMEOUT_SECS
    })
}

/// Footer shown on an elided inline tool-output panel.
fn output_footer(total: usize, shown: usize) -> String {
    format!("… {} more lines · ctrl+o", total - shown)
}

/// Fire an out-of-band cancel at the daemon on a fresh connection, so the
/// streaming connection's reader is never touched. Best-effort: on failure
/// (daemon gone, timeout) the daemon still ends the turn via EPIPE as today.
async fn send_cancel(session_id: &str) {
    let fut = async {
        let mut stream =
            tokio::net::UnixStream::connect(crate::config::default_socket_path()).await?;
        let mut data = serde_json::to_vec(&crate::ipc::Request::Cancel {
            session_id: session_id.to_string(),
        })?;
        data.push(b'\n');
        use tokio::io::AsyncWriteExt;
        stream.write_all(&data).await?;
        anyhow::Ok(())
    };
    if let Err(e) = tokio::time::timeout(std::time::Duration::from_secs(2), fut)
        .await
        .unwrap_or_else(|_| Err(anyhow::anyhow!("cancel send timed out")))
    {
        log::warn!("failed to deliver cancel: {e}");
    }
}

// ── AI conversation ─────────────────────────────────────────────────────────

pub(super) struct QueryArgs<'a> {
    pub(super) query: String,
    pub(super) prompt_override: Option<&'a str>,
}

pub(super) struct AskTmuxCtx<'a> {
    pub(super) session: Option<&'a str>,
    pub(super) pane: Option<&'a str>,
}

/// Context for the ratatui-path query function.
pub(super) struct RatatuiQueryCtx<'a> {
    pub(super) chat_width: Option<usize>,
    pub(super) renderer: &'a mut crate::cli::render_ratatui::RatatuiRendererStdout,
    pub(super) stdin: &'a AsyncStdin,
    pub(super) transcript: &'a mut crate::cli::transcript::Transcript,
    /// Where this turn's typed text accumulates (M32 #18); the caller reads
    /// it again after the turn to submit or pre-fill.
    pub(super) typeahead: &'a mut Typeahead,
    /// The OSC 7501 emitter the turn reports its state through (M36 21c).
    /// Shared for the client's lifetime; `ask_with_session_ratatui` only
    /// drives it.
    pub(super) status: &'a crate::cli::program_status_emit::Emitter,
    /// The status-bar model this turn updates in place (M37 02a): the same
    /// one the chat loop owns, so the bar stays coherent across the turn.
    pub(super) sb: &'a mut crate::cli::status_bar::StatusModel,
}

#[path = "stream_ask.rs"]
mod ask;
pub(super) use ask::ask_with_session_ratatui;
// `ask` reaches `super::approval::…` through this import.
use super::approval;

/// Read one newline-delimited `Response`, accumulating bytes into the
/// **caller-owned** `buf`.
///
/// `read_until` appends consumed bytes to `buf` *before* awaiting more, so if
/// this future is dropped mid-line (an interrupt key or spinner tick won the
/// `select!`), the partial bytes remain in `buf` and the next call continues
/// where it left off. This is what makes interrupting a streaming read
/// non-destructive (the bug-phase-11-1 / bug-phase-11-2 failure mode was a
/// dropped read that stranded bytes inside the future's own local buffer).
///
/// On a complete line, `buf` is cleared and the parsed `Response` returned.
async fn recv_line(
    rx: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
    buf: &mut Vec<u8>,
) -> anyhow::Result<Response> {
    use tokio::io::AsyncBufReadExt;
    let n = rx.read_until(b'\n', buf).await?;
    if n == 0 {
        anyhow::bail!("Daemon closed connection unexpectedly.");
    }
    let line = std::str::from_utf8(buf)?.trim();
    let response: Response = serde_json::from_str(line)?;
    buf.clear();
    Ok(response)
}

/// Race keyboard input against a daemon read whose partial state lives in the
/// caller-owned `buf` (see [`recv_line`]).
///
/// Returning `Warn` or `Tick` drops the in-flight `recv_line` future, but the
/// bytes it already read are preserved in `buf`, so the caller can re-enter
/// without losing a daemon message. Only `Msg`/`Error`/`Interrupted` end the
/// read for good.
///
/// `tick_interval` controls the spinner animation rate (80 ms for phase 1,
/// `Duration::MAX` to disable for phase 2). `overall_timeout` is an optional
/// total timeout for the daemon response (120 s for phase 2, `None` for phase 1).
#[allow(clippy::too_many_arguments)] // 8 params: the flow needs stdin, the daemon reader, its partial buffer, both per-turn states, the tick rate, the overall deadline and the resize signal
async fn select_stream(
    stdin: &AsyncStdin,
    rx: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
    buf: &mut Vec<u8>,
    interrupt_state: &mut InterruptState,
    typeahead: &mut Typeahead,
    tick_interval: std::time::Duration,
    overall_timeout: Option<std::time::Duration>,
    sigwinch: &mut tokio::signal::unix::Signal,
) -> StreamOutcome {
    let mut timeout_fut = overall_timeout.map(|d| Box::pin(tokio::time::sleep(d)));

    loop {
        if let Some(ref mut to) = timeout_fut {
            tokio::select! {
                key = read_key(stdin) => {
                    if let Some(key) = key {
                        if typeahead.feed(&key) {
                            continue;
                        }
                        if let Some(outcome) = key_outcome(&key) {
                            return outcome;
                        }
                        match interrupt_state.feed(&key) {
                            InterruptAction::Ignore => continue,
                            InterruptAction::Warn => return StreamOutcome::Warn,
                            InterruptAction::Abort => return StreamOutcome::Interrupted,
                        }
                    }
                    continue;
                }
                res = recv_line(rx, buf) => {
                    return match res {
                        Ok(response) => StreamOutcome::Msg(Box::new(response)),
                        Err(e) => StreamOutcome::Error(e.to_string()),
                    };
                }
                _ = to => {
                    return StreamOutcome::Deadline;
                }
                _ = tokio::time::sleep(tick_interval), if tick_interval != std::time::Duration::MAX => {
                    return StreamOutcome::Tick;
                }
                _ = sigwinch.recv() => {
                    return StreamOutcome::Reanchor;
                }
            }
        } else {
            tokio::select! {
                key = read_key(stdin) => {
                    if let Some(key) = key {
                        if typeahead.feed(&key) {
                            continue;
                        }
                        if let Some(outcome) = key_outcome(&key) {
                            return outcome;
                        }
                        match interrupt_state.feed(&key) {
                            InterruptAction::Ignore => continue,
                            InterruptAction::Warn => return StreamOutcome::Warn,
                            InterruptAction::Abort => return StreamOutcome::Interrupted,
                        }
                    }
                    continue;
                }
                res = recv_line(rx, buf) => {
                    return match res {
                        Ok(response) => StreamOutcome::Msg(Box::new(response)),
                        Err(e) => StreamOutcome::Error(e.to_string()),
                    };
                }
                _ = tokio::time::sleep(tick_interval), if tick_interval != std::time::Duration::MAX => {
                    return StreamOutcome::Tick;
                }
                _ = sigwinch.recv() => {
                    return StreamOutcome::Reanchor;
                }
            }
        }
    }
}

#[path = "stream_prompts.rs"]
mod prompts;
use prompts::*;

/// Text typed while a turn streams, kept for the turn's end (M32 #18).
///
/// `select_stream` feeds every key here before `key_outcome` and the
/// interrupt state; a key the typeahead consumed goes no further. Printable
/// characters and Backspace accumulate; the first Enter marks the kept text
/// as submitted and every later key of the turn is refused, so exactly one
/// message can be queued. Every other key (Ctrl-C's warn and abort, Ctrl-O,
/// focus events, a paste) is refused and follows today's path.
#[derive(Debug, Default, Clone)]
pub(super) struct Typeahead {
    text: String,
    enter: bool,
}

impl Typeahead {
    /// Feed one key. Returns `true` when the typeahead consumed it — the
    /// key then reaches neither `key_outcome` nor the interrupt state.
    pub(super) fn feed(&mut self, key: &Key) -> bool {
        if self.enter {
            return false;
        }
        match key {
            Key::Char(c) if *c != '\0' && *c != '\x1b' => {
                self.text.push(*c);
                true
            }
            Key::Backspace => {
                self.text.pop();
                true
            }
            Key::Enter => {
                self.enter = true;
                true
            }
            _ => false,
        }
    }

    /// The kept text, as it would be sent.
    pub(super) fn text(&self) -> &str {
        &self.text
    }

    /// Whether Enter was pressed mid-turn: the kept text is the next
    /// message, not a pre-fill.
    pub(super) fn enter_pressed(&self) -> bool {
        self.enter
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Typeahead ───────────────────────────────────────────────────────

    #[test]
    fn typeahead_keeps_text_and_one_enter() {
        let mut ta = Typeahead::default();
        // h, i, Backspace, o keep "ho" with no Enter.
        assert!(ta.feed(&Key::Char('h')), "h is consumed");
        assert!(ta.feed(&Key::Char('i')), "i is consumed");
        assert!(ta.feed(&Key::Backspace), "Backspace is consumed");
        assert!(ta.feed(&Key::Char('o')), "o is consumed");
        assert_eq!(ta.text(), "ho");
        assert!(!ta.enter_pressed(), "no Enter yet");

        // Enter is consumed once and marks the text submitted.
        assert!(ta.feed(&Key::Enter), "Enter is consumed");
        assert!(ta.enter_pressed());

        // After Enter, further characters are refused — one message only.
        assert!(!ta.feed(&Key::Char('x')), "a post-Enter char is refused");

        // Ctrl-C and focus events are never consumed: they keep today's path.
        assert!(!ta.feed(&Key::CtrlC), "Ctrl-C is never consumed");
        assert!(!ta.feed(&Key::FocusGained), "FocusGained is never consumed");
        assert_eq!(ta.text(), "ho", "refused keys change nothing");
    }

    // ── read_byte_by ────────────────────────────────────────────────────

    /// The deadline seam behind the approval reads (F7): a stdin that never
    /// sends a byte yields `None` once the deadline passes, and a byte that
    /// arrives in time passes through.
    #[tokio::test]
    async fn approval_read_times_out() {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(20);
        let never = std::future::pending::<Option<u8>>();
        assert_eq!(read_byte_by(deadline, never).await, None);
        let late = async { Some(b'y') };
        assert_eq!(
            read_byte_by(
                tokio::time::Instant::now() + std::time::Duration::from_secs(5),
                late
            )
            .await,
            Some(b'y')
        );
    }

    // ── parse_approval_decision ─────────────────────────────────────────

    #[test]
    fn parse_approval_decision_y_approves() {
        let (approved, is_session, msg) = parse_approval_response("y");
        assert!(approved);
        assert!(!is_session);
        assert!(msg.is_none());
    }

    #[test]
    fn parse_approval_decision_yes_approves() {
        let (approved, is_session, msg) = parse_approval_response("yes");
        assert!(approved);
        assert!(!is_session);
        assert!(msg.is_none());
    }

    #[test]
    fn parse_approval_decision_y_uppercase_approves() {
        let (approved, is_session, msg) = parse_approval_response("Y");
        assert!(approved);
        assert!(!is_session);
        assert!(msg.is_none());
    }

    #[test]
    fn parse_approval_decision_n_denies() {
        let (approved, is_session, msg) = parse_approval_response("n");
        assert!(!approved);
        assert!(!is_session);
        assert!(msg.is_none());
    }

    #[test]
    fn parse_approval_decision_empty_denies() {
        let (approved, is_session, msg) = parse_approval_response("");
        assert!(!approved);
        assert!(!is_session);
        assert!(msg.is_none());
    }

    #[test]
    fn a_root_edit_is_never_auto_approved() {
        let mut a = super::super::approval::SessionApproval {
            file_edits_all: true,
            ..super::super::approval::SessionApproval::default()
        };
        a.file_edits.insert("/etc/x".to_string());
        assert!(
            !edit_file_auto_approved(&a, "/etc/x", true, false),
            "the session-wide approval must not cover a root edit"
        );
        assert!(
            edit_file_auto_approved(&a, "/etc/x", false, false),
            "without the root flag the session approval covers the path as before"
        );
        assert!(
            !edit_file_auto_approved(&a, "/etc/x", false, true),
            "the session-wide approval must not cover a config edit either"
        );
    }

    #[test]
    fn a_root_edit_never_grants_a_session_approval() {
        assert!(
            !edit_file_grants_session(true, false, true, false),
            "an approved-for-session answer on a root edit approves only this edit"
        );
        assert!(
            !edit_file_grants_session(true, false, false, true),
            "an approved-for-session answer on a config edit approves only this edit"
        );
        assert!(
            edit_file_grants_session(true, false, false, false),
            "an ordinary edit still grants its session approval"
        );
    }

    #[test]
    fn config_edit_is_never_auto_approved() {
        let mut a = super::super::approval::SessionApproval {
            file_edits_all: true,
            ..Default::default()
        };
        a.file_edits.insert("/etc/x".to_string());
        // Session-wide approval, per-path grant, and both together: an edit
        // the daemon marks always-asked still prompts.
        assert!(
            !edit_file_auto_approved(&a, "/etc/x", false, true),
            "file_edits_all must not cover a config edit"
        );
        assert!(
            !edit_file_auto_approved(
                &{
                    let mut b = super::super::approval::SessionApproval::default();
                    b.file_edits.insert("/etc/x".to_string());
                    b
                },
                "/etc/x",
                false,
                true
            ),
            "the per-path session grant must not cover a config edit"
        );
        // And an approved answer on always-asked grants nothing for later asks.
        assert!(
            !edit_file_grants_session(true, false, false, true),
            "an approved config edit approves only this edit"
        );
    }

    #[test]
    fn parse_approval_decision_a_approves_session() {
        let (approved, is_session, msg) = parse_approval_response("a");
        assert!(approved);
        assert!(is_session);
        assert!(msg.is_none());
    }

    #[test]
    fn parse_approval_decision_typed_message_redirects() {
        let (approved, is_session, msg) = parse_approval_response("do X instead");
        assert!(!approved);
        assert!(!is_session);
        assert_eq!(msg, Some("do X instead".to_string()));
    }

    #[test]
    fn parse_approval_decision_typed_message_preserves_case() {
        let (approved, is_session, msg) = parse_approval_response("Fix the path please");
        assert!(!approved);
        assert!(!is_session);
        assert_eq!(msg, Some("Fix the path please".to_string()));
    }

    #[test]
    fn build_approval_prompt_session_with_redirect() {
        assert_eq!(
            build_approval_prompt("session", true),
            "  Approve? [Y]es  [A]pprove for session  [N]o  or type a message › "
        );
    }

    #[test]
    fn build_approval_prompt_sudo_session_with_redirect() {
        assert_eq!(
            build_approval_prompt("sudo session", true),
            "  Approve? [Y]es  [A]pprove for sudo session  [N]o  or type a message › "
        );
    }

    #[test]
    fn build_approval_prompt_session_without_redirect() {
        assert_eq!(
            build_approval_prompt("session", false),
            "  Approve? [Y]es  [A]pprove for session  [N]o  › "
        );
    }

    // ── silence_budget ─────────────────────────────────────────────────

    #[test]
    fn silence_budget_phase1_is_90s() {
        assert_eq!(
            silence_budget(false),
            std::time::Duration::from_secs(PHASE1_SILENCE_TIMEOUT_SECS)
        );
    }

    #[test]
    fn silence_budget_phase2_is_120s() {
        assert_eq!(
            silence_budget(true),
            std::time::Duration::from_secs(PHASE2_SILENCE_TIMEOUT_SECS)
        );
    }

    // ── output_footer ──────────────────────────────────────────────────

    #[test]
    fn output_footer_names_ctrl_o() {
        assert_eq!(output_footer(300, 9), "… 291 more lines · ctrl+o");
        assert_eq!(output_footer(10, 9), "… 1 more lines · ctrl+o");
    }

    // ── approval first-key (M36 phase-06) ──────────────────────────────

    /// Ctrl+O at an approval is never an answer: it views where viewing is
    /// offered (the write/edit prompts, via the step function's `View`)
    /// and is ignored elsewhere. Kept through phase-07's rewrite of the
    /// reader as the `approval_step` step function.
    #[test]
    fn approval_diff_ctrl_o_is_not_an_answer() {
        use super::prompts::{ApprovalStep, approval_step};
        let offered = |key: &Key| {
            let mut line = InputLine::new();
            approval_step(&mut line, key, true)
        };
        let not_offered = |key: &Key| {
            let mut line = InputLine::new();
            approval_step(&mut line, key, false)
        };
        assert_eq!(
            offered(&Key::CtrlO),
            ApprovalStep::View,
            "ctrl+o opens the viewer when viewing is offered"
        );
        assert_eq!(
            not_offered(&Key::CtrlO),
            ApprovalStep::Pending,
            "ctrl+o is ignored where viewing is not offered"
        );
        assert_eq!(
            offered(&Key::Enter),
            ApprovalStep::Done(String::new()),
            "Enter submits the empty line"
        );
        for (key, ch) in [
            (Key::Char('y'), 'y'),
            (Key::Char('Y'), 'Y'),
            (Key::Char('n'), 'n'),
            (Key::Char('a'), 'a'),
        ] {
            let mut line = InputLine::new();
            assert_eq!(
                approval_step(&mut line, &key, true),
                ApprovalStep::Pending,
                "{ch} edits the line and waits for Enter"
            );
            assert_eq!(line.as_str(), ch.to_string(), "{ch} landed in the line");
        }
        let mut line = InputLine::new();
        assert_eq!(
            approval_step(&mut line, &Key::Char('x'), true),
            ApprovalStep::Pending,
            "a letter starts a redirect, not an answer"
        );
    }

    // ── approval input (M36 phase-07, S2/S3) ───────────────────────────

    /// `a` no longer answers at once: every key edits the line and only
    /// Enter submits it, so `abort that` + Enter is a redirect — not an
    /// approval and not a session grant — and `y` needs its Enter.
    #[test]
    fn approval_input_letter_waits_for_enter() {
        use super::prompts::{ApprovalStep, approval_step};
        let mut line = InputLine::new();
        assert_eq!(
            approval_step(&mut line, &Key::Char('a'), false),
            ApprovalStep::Pending,
            "a alone is pending, not an answer"
        );
        for c in "bort that".chars() {
            assert_eq!(
                approval_step(&mut line, &Key::Char(c), false),
                ApprovalStep::Pending,
                "{c} edits the line"
            );
        }
        assert_eq!(
            approval_step(&mut line, &Key::Enter, false),
            ApprovalStep::Done("abort that".into()),
            "Enter submits the whole typed line"
        );
        let (approved, is_session, msg) = parse_approval_response("abort that");
        assert!(!approved, "a typed redirect never approves");
        assert!(!is_session, "a typed redirect never grants the session");
        assert_eq!(msg, Some("abort that".to_string()));

        let mut line = InputLine::new();
        approval_step(&mut line, &Key::Char('y'), false);
        assert_eq!(
            approval_step(&mut line, &Key::Enter, false),
            ApprovalStep::Done("y".into()),
            "y + Enter submits y"
        );
    }

    /// A bracketed paste is one value: it never submits even with a newline
    /// in it — the CR/LF become spaces — and Enter submits the flattened
    /// line.
    #[test]
    fn approval_input_paste_is_one_value() {
        use super::prompts::{ApprovalStep, approval_step};
        let mut line = InputLine::new();
        assert_eq!(
            approval_step(&mut line, &Key::Paste("yes\nplease".into()), false),
            ApprovalStep::Pending,
            "a paste never submits"
        );
        assert_eq!(
            line.as_str(),
            "yes please",
            "the paste's CR/LF became spaces"
        );
        assert_eq!(
            approval_step(&mut line, &Key::Enter, false),
            ApprovalStep::Done("yes please".into()),
            "Enter submits the pasted value as one"
        );
    }

    /// Focus events, arrow keys, a mouse report and a control byte never
    /// edit the approval line: it stays empty and pending, and y + Enter
    /// still submits y.
    #[test]
    fn approval_input_ignores_focus_and_arrows() {
        use super::prompts::{ApprovalStep, approval_step};
        let mut line = InputLine::new();
        for key in [
            Key::FocusGained,
            Key::FocusLost,
            Key::Up,
            Key::Left,
            Key::Mouse {
                button: 0,
                col: 3,
                row: 2,
                pressed: true,
            },
            Key::Char('\0'),
        ] {
            assert_eq!(
                approval_step(&mut line, &key, false),
                ApprovalStep::Pending,
                "{key:?} leaves the line pending"
            );
            assert!(line.is_empty(), "{key:?} never edits the line");
        }
        approval_step(&mut line, &Key::Char('y'), false);
        assert_eq!(
            approval_step(&mut line, &Key::Enter, false),
            ApprovalStep::Done("y".into()),
            "y + Enter submits y after the ignored keys"
        );
    }

    /// Ctrl+C and a bare Escape deny at once: each submits the empty line,
    /// even with text already on it.
    #[test]
    fn approval_input_ctrl_c_and_escape_deny() {
        use super::prompts::{ApprovalStep, approval_step};
        let mut line = InputLine::new();
        assert_eq!(
            approval_step(&mut line, &Key::CtrlC, false),
            ApprovalStep::Done(String::new()),
            "Ctrl+C submits the empty line"
        );
        let mut line = InputLine::new();
        approval_step(&mut line, &Key::Char('a'), false);
        approval_step(&mut line, &Key::Char('b'), false);
        assert_eq!(
            approval_step(&mut line, &Key::Char('\x1b'), false),
            ApprovalStep::Done(String::new()),
            "a bare Escape submits the empty line, discarding the text"
        );
    }

    /// A printable `Char` is inserted decoded: `c a f é` + Enter submits
    /// `café`, not the mangled bytes the old `b as char` read produced.
    #[test]
    fn approval_input_decodes_utf8() {
        use super::prompts::{ApprovalStep, approval_step};
        let mut line = InputLine::new();
        for c in ['c', 'a', 'f', 'é'] {
            assert_eq!(
                approval_step(&mut line, &Key::Char(c), false),
                ApprovalStep::Pending,
                "{c} edits the line"
            );
        }
        assert_eq!(line.as_str(), "café");
        assert_eq!(
            approval_step(&mut line, &Key::Enter, false),
            ApprovalStep::Done("café".into()),
            "Enter submits the decoded text"
        );
    }

    /// A pasted password is one value with its CR/LF removed: it stays
    /// pending and Enter submits it whole.
    #[test]
    fn approval_input_credential_paste_is_the_value() {
        use super::prompts::{ApprovalStep, credential_step};
        let mut value = String::new();
        assert_eq!(
            credential_step(&mut value, &Key::Paste("hunter2\n".into())),
            ApprovalStep::Pending,
            "a paste never submits"
        );
        assert_eq!(value, "hunter2", "the paste's trailing LF was removed");
        assert_eq!(
            credential_step(&mut value, &Key::Enter),
            ApprovalStep::Done("hunter2".into()),
            "Enter submits the whole pasted value"
        );
    }

    /// Focus events and arrows never edit the credential; `p ä ß` then
    /// Backspace submits `pä` — non-ASCII chars count one each.
    #[test]
    fn approval_input_credential_ignores_focus_and_arrows() {
        use super::prompts::{ApprovalStep, credential_step};
        let mut value = String::new();
        for key in [Key::FocusGained, Key::FocusLost, Key::Up, Key::Left] {
            assert_eq!(
                credential_step(&mut value, &key),
                ApprovalStep::Pending,
                "{key:?} is ignored"
            );
            assert!(value.is_empty(), "{key:?} never edits the value");
        }
        for c in ['p', 'ä', 'ß'] {
            credential_step(&mut value, &Key::Char(c));
        }
        assert_eq!(value, "päß", "each char arrives decoded");
        credential_step(&mut value, &Key::Backspace);
        assert_eq!(value, "pä", "Backspace removes one char");
        assert_eq!(
            credential_step(&mut value, &Key::Enter),
            ApprovalStep::Done("pä".into()),
            "Enter submits the edited value"
        );
    }

    /// Escape clears the credential and submits it empty.
    #[test]
    fn approval_input_credential_escape_clears() {
        use super::prompts::{ApprovalStep, credential_step};
        let mut value = String::new();
        for c in ['x', 'y'] {
            credential_step(&mut value, &Key::Char(c));
        }
        assert_eq!(value, "xy");
        assert_eq!(
            credential_step(&mut value, &Key::Char('\x1b')),
            ApprovalStep::Done(String::new()),
            "a bare Escape clears the value and submits it empty"
        );
        assert!(value.is_empty(), "the cleared value is not left behind");
    }
}

#[cfg(test)]
mod stream_seam_tests {
    use super::*;
    use std::os::unix::io::{AsRawFd, FromRawFd};
    use tokio::net::UnixStream;

    /// A pipe-backed `AsyncStdin` plus the write end (as a `File`) to feed bytes.
    fn make_pipe_stdin() -> (AsyncStdin, std::fs::File) {
        let mut fds: [libc::c_int; 2] = [-1, -1];
        let ret = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_NONBLOCK) };
        assert_eq!(ret, 0, "pipe2 failed: {}", std::io::Error::last_os_error());
        let stdin = AsyncStdin::from_raw_fd(fds[0]).expect("from_raw_fd");
        let write_file = unsafe { std::fs::File::from_raw_fd(fds[1]) };
        (stdin, write_file)
    }

    async fn write_bytes(file: &std::fs::File, bytes: &[u8]) {
        let fd = file.as_raw_fd();
        let mut remaining = bytes;
        while !remaining.is_empty() {
            let n = unsafe {
                libc::write(
                    fd,
                    remaining.as_ptr() as *const libc::c_void,
                    remaining.len(),
                )
            };
            if n > 0 {
                remaining = &remaining[n as usize..];
            } else {
                // A short write on this pipe can only mean EAGAIN; anything else is
                // a real bug and must fail loudly rather than spin forever.
                let err = std::io::Error::last_os_error();
                assert_eq!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock,
                    "write to test pipe failed: {err}"
                );
                tokio::task::yield_now().await;
            }
        }
    }

    // ── credential reader (M36 phase-07, bug-07-1) ─────────────────────

    /// The sudo prompt returns the value the user entered: the reader loop,
    /// not only `credential_step`, hands back what was typed — a bracketed
    /// paste of `hunter2` plus a trailing LF, then Enter, returns `hunter2`
    /// (before bug-07-1 it returned the emptied buffer), and `x y` then a
    /// bare Escape returns empty.
    #[tokio::test]
    async fn approval_input_credential_reader_returns_the_value() {
        let (stdin, wf) = make_pipe_stdin();
        let mut bytes: Vec<u8> = vec![0x1b, b'[', b'2', b'0', b'0', b'~'];
        bytes.extend_from_slice(b"hunter2\n");
        bytes.extend_from_slice(&[0x1b, b'[', b'2', b'0', b'1', b'~', b'\r']);
        write_bytes(&wf, &bytes).await;
        let redraws = std::cell::Cell::new(0u32);
        let value = read_credential_with(&stdin, |_| redraws.set(redraws.get() + 1))
            .await
            .expect("the reader returns on Enter");
        assert_eq!(value, "hunter2", "Enter returns the pasted value");
        assert_eq!(
            redraws.get(),
            2,
            "the reader redraws once on entry and once after the paste"
        );

        let (stdin, wf) = make_pipe_stdin();
        write_bytes(&wf, b"xy\x1b").await;
        let value = read_credential_with(&stdin, |_| {})
            .await
            .expect("the reader returns on Escape");
        assert_eq!(value, "", "a bare Escape clears and returns empty");
    }

    /// The regression guard for bug-phase-11-1 / bug-phase-11-2: a daemon read
    /// that is interrupted (dropped) mid-line must NOT lose the partial bytes —
    /// the next read completes the same message intact.
    #[tokio::test]
    async fn recv_line_preserves_partial_bytes_across_a_dropped_read() {
        let (client, server) = UnixStream::pair().unwrap();
        let (read_half, _write_half) = client.into_split();
        let mut rx = BufReader::new(read_half);
        let (_srv_r, mut srv_w) = server.into_split();

        // The full wire line is one serialized Response + '\n'.
        let wire = serde_json::to_string(&Response::Token("hello".to_string())).unwrap();
        let first_half = &wire[..wire.len() / 2];
        let second_half = &wire[wire.len() / 2..];

        let mut buf: Vec<u8> = Vec::new();

        // Send only the first half — no newline yet.
        use tokio::io::AsyncWriteExt;
        srv_w.write_all(first_half.as_bytes()).await.unwrap();

        // Drive recv_line until it has consumed the partial bytes, then drop it
        // by timing out — exactly what a Warn/Tick return does in select_stream.
        let dropped = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            recv_line(&mut rx, &mut buf),
        )
        .await;
        assert!(
            dropped.is_err(),
            "read should still be pending (no newline yet)"
        );
        assert!(
            !buf.is_empty(),
            "partial bytes must survive the dropped read, got empty buf"
        );

        // Now send the rest + newline; the next read must complete the message.
        srv_w.write_all(second_half.as_bytes()).await.unwrap();
        srv_w.write_all(b"\n").await.unwrap();

        let msg = recv_line(&mut rx, &mut buf).await.expect("recv_line ok");
        match msg {
            Response::Token(t) => assert_eq!(t, "hello", "message reassembled intact"),
            other => panic!("expected Token, got {other:?}"),
        }
        assert!(buf.is_empty(), "buf cleared after a complete line");
    }

    /// Text typed while the daemon is silent is kept in the typeahead and the
    /// turn ends on its Enter: `hi` + `\r` on the keyboard, then a full daemon
    /// line, gives `Msg` with `"hi"` kept and Enter marked.
    ///
    /// The daemon line is written 20 ms in, by a joined writer (the sequencer
    /// `read_byte_waits_out_a_drained_pipe` uses): `select!` polls its branches
    /// in random order, so a line pre-written beside the keys would race the
    /// key consumption. Until it lands the typeahead is the only ready branch,
    /// so the keys are consumed first, deterministically.
    #[tokio::test]
    async fn select_stream_keeps_typed_text_for_the_next_turn() {
        let (client, server) = UnixStream::pair().unwrap();
        let (read_half, _w) = client.into_split();
        let mut rx = BufReader::new(read_half);
        let (_srv_r, mut srv_w) = server.into_split();
        let mut buf: Vec<u8> = Vec::new();
        let mut state = InterruptState::new();
        let mut typeahead = Typeahead::default();

        // The user types `hi` then Enter while the spinner runs.
        let (stdin, wf) = make_pipe_stdin();
        write_bytes(&wf, b"hi\r").await;

        // Then the daemon delivers a full line.
        let writer = async {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let wire = serde_json::to_string(&Response::Token("hi".to_string())).unwrap();
            use tokio::io::AsyncWriteExt;
            srv_w.write_all(wire.as_bytes()).await.unwrap();
            srv_w.write_all(b"\n").await.unwrap();
        };

        let mut sigwinch =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change()).unwrap();

        let ((), outcome) = tokio::join!(
            writer,
            select_stream(
                &stdin,
                &mut rx,
                &mut buf,
                &mut state,
                &mut typeahead,
                std::time::Duration::MAX,
                Some(std::time::Duration::from_secs(120)),
                &mut sigwinch,
            )
        );

        assert!(matches!(outcome, StreamOutcome::Msg(_)), "got {outcome:?}");
        assert_eq!(typeahead.text(), "hi", "both letters were consumed");
        assert!(typeahead.enter_pressed(), "the \\r was consumed as Enter");
        assert!(buf.is_empty());
    }

    /// With the daemon idle (read pending) and an interrupt key queued,
    /// `select_stream` returns `Warn` on the first press without touching the
    /// stream — and the buffer is untouched.
    #[tokio::test]
    async fn select_stream_first_interrupt_press_warns() {
        let (client, _server) = UnixStream::pair().unwrap();
        let (read_half, _w) = client.into_split();
        let mut rx = BufReader::new(read_half);
        let mut buf: Vec<u8> = Vec::new();
        let mut state = InterruptState::new();
        let mut typeahead = Typeahead::default();

        let (stdin, wf) = make_pipe_stdin();
        write_bytes(&wf, &[0x03]).await; // Ctrl+C

        let mut sigwinch =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change()).unwrap();

        let outcome = select_stream(
            &stdin,
            &mut rx,
            &mut buf,
            &mut state,
            &mut typeahead,
            std::time::Duration::MAX, // no spinner tick
            Some(std::time::Duration::from_secs(120)),
            &mut sigwinch,
        )
        .await;

        assert!(matches!(outcome, StreamOutcome::Warn), "got {outcome:?}");
        assert!(state.is_armed());
        assert!(buf.is_empty());
    }

    /// With the keyboard idle and a full daemon line ready, `select_stream`
    /// returns the parsed `Msg`.
    #[tokio::test]
    async fn select_stream_delivers_a_full_daemon_message() {
        let (client, server) = UnixStream::pair().unwrap();
        let (read_half, _w) = client.into_split();
        let mut rx = BufReader::new(read_half);
        let (_srv_r, mut srv_w) = server.into_split();
        let mut buf: Vec<u8> = Vec::new();
        let mut state = InterruptState::new();

        let wire = serde_json::to_string(&Response::Token("hi".to_string())).unwrap();
        use tokio::io::AsyncWriteExt;
        srv_w.write_all(wire.as_bytes()).await.unwrap();
        srv_w.write_all(b"\n").await.unwrap();

        // stdin write end stays open with no bytes → read_key parks forever, so
        // only the daemon-read branch can fire.
        let (stdin, _wf) = make_pipe_stdin();

        let mut sigwinch =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change()).unwrap();

        let outcome = select_stream(
            &stdin,
            &mut rx,
            &mut buf,
            &mut state,
            &mut Typeahead::default(),
            std::time::Duration::MAX,
            Some(std::time::Duration::from_secs(120)),
            &mut sigwinch,
        )
        .await;

        match outcome {
            StreamOutcome::Msg(m) => match *m {
                Response::Token(t) => assert_eq!(t, "hi"),
                other => panic!("expected Token, got {other:?}"),
            },
            other => panic!("expected Msg, got {other:?}"),
        }
    }

    // ── tool_runtime_label ──────────────────────────────────────────────

    #[test]
    fn tool_runtime_label_formats_ok_and_err() {
        assert_eq!(tool_runtime_label(true, 1234), "✓ 1.2s");
        assert_eq!(tool_runtime_label(false, 450), "✗ 0.5s");
    }

    // ── key_outcome ─────────────────────────────────────────────────────

    #[test]
    fn stream_key_focus_gained_still_reanchors() {
        let result = key_outcome(&Key::FocusGained);
        assert!(matches!(result, Some(StreamOutcome::Reanchor)));
        assert!(key_outcome(&Key::Char('x')).is_none());
    }

    #[test]
    fn stream_key_ctrl_o_opens_viewer() {
        assert!(matches!(
            key_outcome(&Key::CtrlO),
            Some(StreamOutcome::OpenViewer)
        ));
        assert!(key_outcome(&Key::Char('q')).is_none());
    }

    // ── select_stream focus / sigwinch ──────────────────────────────────

    #[tokio::test]
    async fn select_stream_focus_gained_returns_reanchor() {
        let (client, _server) = UnixStream::pair().unwrap();
        let (read_half, _w) = client.into_split();
        let mut rx = BufReader::new(read_half);
        let mut buf: Vec<u8> = Vec::new();
        let mut state = InterruptState::new();

        let (stdin, wf) = make_pipe_stdin();
        // ESC [ I = focus gained
        write_bytes(&wf, b"\x1b[I").await;

        let mut sigwinch =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change()).unwrap();

        let outcome = select_stream(
            &stdin,
            &mut rx,
            &mut buf,
            &mut state,
            &mut Typeahead::default(),
            std::time::Duration::MAX,
            Some(std::time::Duration::from_secs(120)),
            &mut sigwinch,
        )
        .await;

        match outcome {
            StreamOutcome::Reanchor => {} // expected
            other => panic!("expected Reanchor, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn select_stream_sigwinch_returns_reanchor() {
        let (client, _server) = UnixStream::pair().unwrap();
        let (read_half, _w) = client.into_split();
        let mut rx = BufReader::new(read_half);
        let mut buf: Vec<u8> = Vec::new();
        let mut state = InterruptState::new();

        // stdin write end stays open with no bytes → read_key parks forever
        let (stdin, _wf) = make_pipe_stdin();

        let mut sigwinch =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change()).unwrap();

        // Spawn the select so it can receive the signal
        let select_handle = tokio::spawn(async move {
            select_stream(
                &stdin,
                &mut rx,
                &mut buf,
                &mut state,
                &mut Typeahead::default(),
                std::time::Duration::MAX,
                Some(std::time::Duration::from_secs(120)),
                &mut sigwinch,
            )
            .await
        });

        // Small delay to let the select settle
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Raise SIGWINCH to the current process
        unsafe { libc::raise(libc::SIGWINCH) };

        let outcome = select_handle.await.unwrap();
        match outcome {
            StreamOutcome::Reanchor => {} // expected
            other => panic!("expected Reanchor, got {other:?}"),
        }
    }
}

// ── tool-call approval panel (M21 phase-06a) ─────────────────────────────

#[cfg(test)]
mod tool_call_panel_tests {
    use super::*;
    use crate::daemon::utils::command_has_sudo;

    #[test]
    fn the_user_shell_panel_names_the_shell_and_is_always_asked() {
        let class = crate::ipc::Classification::UserShell { shell: "s7".into() };
        assert_eq!(
            tool_call_panel_body("uptime", false, None, &class, false),
            vec![
                "$ uptime".to_string(),
                "→ your shell s7 (opened with /shell)".to_string()
            ]
        );
        assert_eq!(
            approval_session_label(&class, false),
            "your shell s7 — always asked"
        );
    }

    #[test]
    fn the_local_command_panel_is_unchanged() {
        let body = tool_call_panel_body(
            "uptime",
            false,
            Some("%5"),
            &crate::ipc::Classification::Command,
            false,
        );
        assert_eq!(
            body,
            vec!["$ uptime".to_string(), "→ target: %5".to_string()],
            "a Command classification adds nothing to the pre-phase panel body"
        );
        assert_eq!(
            tool_call_panel_label(false),
            "terminal · visible to you",
            "the foreground label is the pre-phase wording"
        );
        assert_eq!(
            tool_call_panel_label(true),
            "daemon · runs silently",
            "the background label is the pre-phase wording"
        );
    }

    /// D6: the approval panel must say that a launch keeps the shell — the
    /// user sees the session open and must know nothing waits for it. The
    /// line sits right after the command, before any classification line.
    #[test]
    fn the_launch_panel_says_the_command_keeps_the_shell() {
        let class = crate::ipc::Classification::Command;
        let body = tool_call_panel_body("claude", false, None, &class, true);
        assert_eq!(
            body,
            vec![
                "$ claude".to_string(),
                "→ interactive: the command keeps the shell; nothing waits for it".to_string(),
            ],
            "a launch's body is the command then the keeps-the-shell line"
        );

        let body = tool_call_panel_body(
            "ssh zoidberg",
            false,
            None,
            &crate::ipc::Classification::RemoteShell {
                host: "zoidberg".into(),
            },
            true,
        );
        assert_eq!(
            body,
            vec![
                "$ ssh zoidberg".to_string(),
                "→ interactive: the command keeps the shell; nothing waits for it".to_string(),
                "→ host: zoidberg (configured)".to_string(),
            ],
            "the launch line comes before the classification line"
        );

        let body = tool_call_panel_body("uptime", false, None, &class, false);
        assert!(
            !body.iter().any(|l| l.contains("interactive")),
            "a non-launch's body has no interactive line: {body:?}"
        );
    }

    #[test]
    fn the_local_approval_flow_is_unchanged() {
        let approval = SessionApproval::from_config(&crate::config::ApprovalsConfig::default());
        let command = "uptime";
        let is_sudo = command_has_sudo(command);
        let class = crate::ipc::Classification::Command;

        assert!(
            approval.auto_approves(&class, is_sudo),
            "a non-sudo Command under the shipped default auto-approves as before this phase"
        );
        assert!(
            !approval.auto_approves(&class, true),
            "a sudo Command under the shipped default is still asked, as before this phase"
        );

        let body = tool_call_panel_body(command, false, Some("%5"), &class, false);
        assert_eq!(
            body,
            vec!["$ uptime".to_string(), "→ target: %5".to_string()],
            "the Command panel body is byte-identical to the pre-phase wording"
        );
        assert_eq!(
            approval_session_label(&class, false),
            "session",
            "the Command session label is the pre-phase wording"
        );
        assert_eq!(
            approval_session_label(&class, true),
            "sudo session",
            "the Command sudo session label is the pre-phase wording"
        );
    }

    #[test]
    fn a_remote_session_grant_records_against_the_host_not_the_daemon_flags() {
        let mut approval = SessionApproval::from_config(&crate::config::ApprovalsConfig::default());
        let class = crate::ipc::Classification::RemoteShell {
            host: "pinky".to_string(),
        };
        approval.record_session_approval(&class, false);
        assert!(
            approval.auto_approves(&class, false),
            "after the A answer, non-sudo on pinky auto-approves"
        );
        assert!(
            !approval.auto_approves(&crate::ipc::Classification::Command, true),
            "the remote grant must not widen the daemon-host sudo flag"
        );
        assert!(
            approval.host_approval_lines().len() == 1
                && approval.host_approval_lines()[0].contains("pinky"),
            "the per-host state holds exactly pinky"
        );
        assert_eq!(
            approval_session_label(&class, false),
            "pinky session",
            "the remote session label names the host"
        );
        assert_eq!(
            approval_session_label(
                &crate::ipc::Classification::NewHost {
                    host: "nope".to_string()
                },
                false
            ),
            "nope (unconfigured) — always asked",
            "the unconfigured label names the host and the always-asked property"
        );
        assert_eq!(
            approval_session_label(&crate::ipc::Classification::EscapeHatch, false),
            "escape hatch — always asked",
            "the escape-hatch label states the always-asked property"
        );
    }
}

//! OSC 7501 program-status emit (M36 phase 21c): the chat client reports its
//! own state to the user's terminal.
//!
//! One report per state change, only to a terminal, only over the injected
//! writer (stdout for real, a buffer in tests), only the root record
//! (`app=daemoneye`, no id, no `msg`, no `title`) so nothing from the
//! conversation reaches the terminal. Inside tmux every report rides the
//! passthrough envelope, and the client turns `allow-passthrough` on for its
//! own pane for its lifetime, unsetting it at exit. The clear-at-exit duty
//! hangs off `restore_terminal`, so the panic path clears too.

use std::cell::RefCell;
use std::io::{IsTerminal, Write};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

/// The `app` every report carries; the parser's grammar allows it.
const APP: &str = "daemoneye";
const OSC_START: &[u8] = b"\x1b]7501;";
const BEL: u8 = 0x07;
const TMUX_START: &[u8] = b"\x1bPtmux;";
const TMUX_END: &[u8] = b"\x1b\\";
const ESC: u8 = 0x1b;

/// Whether this process has written at least one report: the exit hook
/// clears only what was set (Behaviour 3).
static REPORTED: AtomicBool = AtomicBool::new(false);
/// The framing the reports used, so the exit clear matches them.
static TMUX_FRAMING: AtomicBool = AtomicBool::new(false);
/// Whether this process set `allow-passthrough` on its own pane: the exit
/// hook unsets exactly that, and nothing else.
static PANE_ARMED: AtomicBool = AtomicBool::new(false);
/// The pane the option was set on, recorded at arm time.
static PANE: OnceLock<String> = OnceLock::new();

/// The pairs of one report: the state (or `clear`) plus the root `app`.
/// The clear report is the one exception to the `app` pair: the parser
/// accepts exactly `state=clear[:id=…]` and drops whole anything else
/// (21a's clear clause), so clear carries no `app`.
fn state_pairs(value: &str) -> String {
    if value == "clear" {
        return format!("state={value}");
    }
    format!("state={value}:app={APP}")
}

/// The bare report: `ESC ] 7501 ; <pairs> BEL` (Behaviour 4).
pub fn bare_frame(value: &str) -> Vec<u8> {
    let pairs = state_pairs(value);
    let mut out = Vec::with_capacity(OSC_START.len() + pairs.len() + 1);
    out.extend_from_slice(OSC_START);
    out.extend_from_slice(pairs.as_bytes());
    out.push(BEL);
    out
}

/// The tmux passthrough envelope around a bare report (Behaviour 5): every
/// ESC inside is doubled, so tmux forwards the report instead of consuming it.
pub fn tmux_wrap(bare: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bare.len() * 2 + TMUX_START.len() + TMUX_END.len());
    out.extend_from_slice(TMUX_START);
    for &b in bare {
        if b == ESC {
            out.push(ESC);
        }
        out.push(b);
    }
    out.extend_from_slice(TMUX_END);
    out
}

/// One report, framed for the target: the envelope inside tmux, bare outside.
fn framed(value: &str, tmux: bool) -> Vec<u8> {
    let bare = bare_frame(value);
    if tmux { tmux_wrap(&bare) } else { bare }
}

/// Whether `$TMUX` puts this client inside tmux: set and non-empty, the same
/// rule `palette::detect_from_env` applies. Pure over the value so a test can
/// drive both branches.
fn tmux_passthrough(tmux_env: Option<&str>) -> bool {
    tmux_env.is_some_and(|v| !v.is_empty())
}

/// Which state a `Response` frame asks for, or `None` for a frame that
/// changes nothing: the prompt frames are `blocked`, the content frames
/// `working`, everything else silent. Pure over the frame so the tests
/// enumerate it (Behaviour 1).
pub fn frame_state(frame: &crate::ipc::Response) -> Option<crate::shell::program_status::State> {
    use crate::ipc::Response as R;
    use crate::shell::program_status::State;
    match frame {
        R::ToolCallPrompt { .. }
        | R::EditFilePrompt { .. }
        | R::ScriptWritePrompt { .. }
        | R::ScriptDeletePrompt { .. }
        | R::ScheduleWritePrompt { .. }
        | R::RunbookWritePrompt { .. }
        | R::RunbookDeletePrompt { .. }
        | R::CredentialPrompt { .. }
        | R::PaneSelectPrompt { .. } => Some(State::Blocked),
        R::Token(_) | R::ToolStarted { .. } | R::ToolResult { .. } => Some(State::Working),
        _ => None,
    }
}

/// The `blocked` kind a prompt frame waits under, for the frames
/// [`frame_state`] maps to blocked. Pure over the frame.
pub fn frame_kind(frame: &crate::ipc::Response) -> Option<crate::shell::program_status::Kind> {
    use crate::ipc::Response as R;
    use crate::shell::program_status::Kind;
    match frame {
        R::CredentialPrompt { .. } => Some(Kind::Auth),
        R::PaneSelectPrompt { .. } => Some(Kind::Question),
        R::ToolCallPrompt { .. }
        | R::EditFilePrompt { .. }
        | R::ScriptWritePrompt { .. }
        | R::ScriptDeletePrompt { .. }
        | R::ScheduleWritePrompt { .. }
        | R::RunbookWritePrompt { .. }
        | R::RunbookDeletePrompt { .. } => Some(Kind::Permission),
        _ => None,
    }
}

/// The per-report state behind a `RefCell` so the turn-end guard can hold a
/// shared borrow while the turn reports through `&self`.
struct Inner {
    last: Option<String>,
    reported: bool,
}

/// The chat client's status emitter. Every report goes to the one writer the
/// emitter was built over (bug-21c-1). A report is written only when the
/// state changed (Behaviour 2), only to a terminal (Behaviour 6), and never
/// fails the turn — a write error is ignored.
pub struct Emitter {
    writer: RefCell<Box<dyn Write + Send>>,
    inner: RefCell<Inner>,
    tmux: bool,
    terminal: bool,
}

impl Emitter {
    /// An emitter over an injected writer. `terminal` gates every write
    /// (Behaviour 6); `tmux` picks the framing (Behaviour 5). Tests inject a
    /// buffer they hold; production goes through [`Emitter::for_stdout`],
    /// which injects stdout.
    pub fn new(writer: Box<dyn Write + Send>, terminal: bool, tmux: bool) -> Self {
        Self {
            writer: RefCell::new(writer),
            inner: RefCell::new(Inner {
                last: None,
                reported: false,
            }),
            tmux,
            terminal,
        }
    }

    /// The production emitter: stdout, gated on stdout being a terminal and
    /// on `$TMUX`. Inside tmux this also arms the pane option — the client
    /// turns `allow-passthrough` on for its own pane, once per process, and
    /// `exit_clear` unsets it again.
    pub fn for_stdout() -> Self {
        let terminal = std::io::stdout().is_terminal();
        let in_tmux = tmux_passthrough(std::env::var("TMUX").ok().as_deref());
        if terminal && in_tmux {
            arm_pane_option();
        }
        Self::new(Box::new(std::io::stdout()), terminal, in_tmux)
    }

    /// The turn was sent: `working` (Behaviour 1).
    pub fn working(&self) {
        self.report("working");
    }

    /// An approval prompt waits: `blocked` with its kind.
    pub fn blocked(&self, kind: crate::shell::program_status::Kind) {
        self.report(&format!("blocked:kind={}", kind.as_str()));
    }

    /// The prompt's reply was sent — back to `working`, with no further
    /// frame needed from the caller (Behaviour 1).
    pub fn reply(&self) {
        self.working();
    }

    /// The turn ended on any path: `idle` (Behaviour 1).
    pub fn idle(&self) {
        self.report("idle");
    }

    /// The state one daemon frame asks for, or nothing for a silent frame.
    pub fn frame(&self, msg: &crate::ipc::Response) {
        match frame_state(msg) {
            Some(crate::shell::program_status::State::Blocked) => {
                let kind =
                    frame_kind(msg).unwrap_or(crate::shell::program_status::Kind::Permission);
                self.blocked(kind);
            }
            Some(_) => self.working(),
            None => {}
        }
    }

    /// Write `clear` if and only if this emitter reported something
    /// (Behaviour 3). The exit hook's clear goes through the same bytes.
    pub fn finish(&self) {
        if !self.inner.borrow().reported {
            return;
        }
        self.write(&framed("clear", self.tmux));
        let mut inner = self.inner.borrow_mut();
        inner.last = None;
        inner.reported = false;
    }

    /// A guard that reports `idle` when dropped: the turn ends on any path —
    /// done, error, cancelled — and the terminal must read `idle` again the
    /// moment it does, not only on the success path (bug-21c-2).
    pub fn idle_on_drop(&self) -> IdleOnDrop<'_> {
        IdleOnDrop { emitter: self }
    }

    fn report(&self, pairs: &str) {
        if !self.terminal {
            return;
        }
        let bytes = {
            let mut inner = self.inner.borrow_mut();
            if inner.last.as_deref() == Some(pairs) {
                return;
            }
            inner.last = Some(pairs.to_string());
            inner.reported = true;
            framed(pairs, self.tmux)
        };
        let _ = self.writer.borrow_mut().write_all(&bytes);
        let _ = self.writer.borrow_mut().flush();
        REPORTED.store(true, Ordering::SeqCst);
        TMUX_FRAMING.store(self.tmux, Ordering::SeqCst);
    }

    fn write(&self, bytes: &[u8]) {
        let _ = self.writer.borrow_mut().write_all(bytes);
        let _ = self.writer.borrow_mut().flush();
    }
}

/// The [`Emitter::idle_on_drop`] guard.
pub struct IdleOnDrop<'a> {
    emitter: &'a Emitter,
}

impl Drop for IdleOnDrop<'_> {
    fn drop(&mut self) {
        // Only when the turn changed something: a turn that failed before its
        // first report leaves the terminal exactly as it was — the same rule
        // `finish` applies to `clear` (Behaviour 3).
        if self.emitter.inner.borrow().last.is_some() {
            self.emitter.idle();
        }
    }
}

/// Set `allow-passthrough` on this client's own pane, once per process
/// (Behaviour 5). A failing call is ignored; nothing is armed, so the exit
/// hook will not try to unset what was never set.
fn arm_pane_option() {
    if PANE_ARMED.load(Ordering::SeqCst) {
        return;
    }
    let pane = std::env::var("TMUX_PANE").ok().filter(|p| !p.is_empty());
    let Some(pane) = pane else {
        return;
    };
    if crate::tmux::set_pane_passthrough(&pane, None, true) {
        PANE_ARMED.store(true, Ordering::SeqCst);
        let _ = PANE.set(pane);
    }
}

/// The exit hook `restore_terminal` calls: `clear` only if this process
/// reported (Behaviour 3), the pane option unset only if this process set it
/// (Behaviour 5). Both at most once; a second call writes nothing.
pub fn exit_clear() {
    if REPORTED.swap(false, Ordering::SeqCst) {
        let bytes = framed("clear", TMUX_FRAMING.load(Ordering::SeqCst));
        let mut out = std::io::stdout();
        let _ = out.write_all(&bytes);
        let _ = out.flush();
    }
    if PANE_ARMED.swap(false, Ordering::SeqCst)
        && let Some(pane) = PANE.get()
    {
        crate::tmux::unset_pane_passthrough(pane, None);
    }
}

#[cfg(test)]
#[path = "program_status_emit_tests.rs"]
mod tests;

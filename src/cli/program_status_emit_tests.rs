//! `program_status_emit`'s tests. In the same crate behind `#[cfg(test)]` via
//! `#[path]` (the `render_ratatui_tests.rs` shape), so they read the module's
//! private state directly.

use super::*;
use crate::shell::program_status::{Kind, State};
use std::io::Write;
use std::sync::{Arc, Mutex};

// ── The writer the test holds ────────────────────────────────────────────────

/// A writer the test keeps a handle to: every report the emitter writes lands
/// here and is read back through `snapshot()` — from the writer the test
/// holds, not from anything the emitter keeps (bug-21c-1).
#[derive(Clone, Default)]
struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl SharedBuf {
    fn snapshot(&self) -> Vec<u8> {
        self.0.lock().expect("test writer lock").clone()
    }
}

impl Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("test writer lock")
            .extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn emitter(terminal: bool, tmux: bool) -> (Emitter, SharedBuf) {
    let held = SharedBuf::default();
    let emit = Emitter::new(Box::new(held.clone()), terminal, tmux);
    (emit, held)
}

// ── Round trip through the host parser ───────────────────────────────────────

fn root_record(
    screen: &mut crate::shell::ShellScreen,
) -> Option<crate::shell::program_status::Record> {
    screen.programs().into_iter().find(|r| r.id.is_none())
}

#[test]
fn program_status_emit_each_state_round_trips_through_the_host_parser() {
    for (value, want_state, want_kind) in [
        ("working", State::Working, None),
        (
            "blocked:kind=permission",
            State::Blocked,
            Some(Kind::Permission),
        ),
        ("blocked:kind=auth", State::Blocked, Some(Kind::Auth)),
        (
            "blocked:kind=question",
            State::Blocked,
            Some(Kind::Question),
        ),
        ("idle", State::Idle, None),
    ] {
        let mut screen = crate::shell::ShellScreen::new(24, 80, 0);
        let frame = bare_frame(value);
        screen.feed(&frame);
        let record = root_record(&mut screen)
            .unwrap_or_else(|| panic!("the bare report `{value}` did not parse"));
        assert_eq!(record.state, want_state, "state of `{value}`");
        assert_eq!(record.kind, want_kind, "kind of `{value}`");
        assert_eq!(record.app.as_deref(), Some("daemoneye"), "app of `{value}`");
        assert_eq!(record.id, None, "`{value}` is the root record");
        assert!(
            record.title.is_none() && record.msg.is_none(),
            "`{value}` carries no text"
        );
    }

    // `clear` empties the table, after a report put one in it.
    let mut screen = crate::shell::ShellScreen::new(24, 80, 0);
    screen.feed(&bare_frame("working"));
    assert!(root_record(&mut screen).is_some(), "the report landed");
    screen.feed(&bare_frame("clear"));
    assert!(
        screen.programs().is_empty(),
        "`clear` removes the root record: {:?}",
        screen.programs()
    );
}

// ── Framing ──────────────────────────────────────────────────────────────────

#[test]
fn program_status_emit_tmux_wraps_and_doubles_escapes() {
    let bare = bare_frame("working");
    let wrapped = tmux_wrap(&bare);
    assert!(
        wrapped.starts_with(b"\x1bPtmux;"),
        "starts with the envelope: {wrapped:?}"
    );
    assert!(
        wrapped.ends_with(b"\x1b\\"),
        "ends with the envelope's ST: {wrapped:?}"
    );

    // Every ESC inside the envelope is doubled.
    let inside = &wrapped[b"\x1bPtmux;".len()..wrapped.len() - b"\x1b\\".len()];
    assert!(
        inside.iter().filter(|&&b| b == ESC).count() % 2 == 0,
        "every ESC inside is doubled: {inside:?}"
    );

    // Un-doubling gives exactly the bare report.
    let mut out = Vec::new();
    let mut i = 0;
    while i < inside.len() {
        if inside[i] == ESC && i + 1 < inside.len() && inside[i + 1] == ESC {
            out.push(ESC);
            i += 2;
        } else {
            out.push(inside[i]);
            i += 1;
        }
    }
    assert_eq!(out, bare, "un-doubling the inside gives the bare report");
}

// ── tmux env gating ──────────────────────────────────────────────────────────

#[test]
fn program_status_emit_empty_tmux_var_means_bare() {
    // The tmux decision is pure over `$TMUX`'s value: `Some("")` is as good
    // as `None` (palette.rs's rule).
    assert!(!tmux_passthrough(None), "no $TMUX: bare");
    assert!(!tmux_passthrough(Some("")), "empty $TMUX: bare");
    assert!(tmux_passthrough(Some("/tmp/tmux-0/x,1,0")), "set: wrapped");
}

// ── Pane-option argv (pure) ──────────────────────────────────────────────────

#[test]
fn program_status_emit_tmux_pane_option_argv() {
    let (start, exit) = crate::tmux::pane_option_argv(Some("%7"));
    assert_eq!(
        start,
        vec![
            "set".to_string(),
            "-p".to_string(),
            "-t".to_string(),
            "%7".to_string(),
            "allow-passthrough".to_string(),
            "on".to_string(),
        ],
        "the start call sets the pane option on"
    );
    assert_eq!(
        exit,
        vec![
            "set".to_string(),
            "-pu".to_string(),
            "-t".to_string(),
            "%7".to_string(),
            "allow-passthrough".to_string(),
        ],
        "the exit call unsets the pane option"
    );

    // Without `$TMUX_PANE` there is no call at all.
    assert_eq!(
        crate::tmux::pane_option_argv(None),
        (Vec::new(), Vec::new())
    );
}

// ── Live pane-option set and unset ───────────────────────────────────────────

/// A private tmux server on its own socket: every command names `-S <sock>`,
/// never the operator's server.
struct PrivateTmux {
    sock: std::path::PathBuf,
}

impl PrivateTmux {
    fn new(dir: &std::path::Path) -> Self {
        let sock = dir.join("s.sock");
        Self { sock }
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        let mut cmd = std::process::Command::new("tmux");
        cmd.arg("-S").arg(&self.sock).args(args);
        crate::process::bounded_output(&mut cmd).expect("run tmux on the private socket")
    }
}

impl Drop for PrivateTmux {
    fn drop(&mut self) {
        let _ = std::process::Command::new("tmux")
            .arg("-S")
            .arg(&self.sock)
            .args(["kill-server"])
            .output();
    }
}

#[test]
fn program_status_emit_tmux_pane_option_set_and_unset_live() {
    let dir = tempfile::tempdir().expect("temp dir under /tmp");
    let tmux = PrivateTmux::new(dir.path());

    // A detached session on a private server; the window's own value is the
    // default `off`.
    tmux.run(&["new-session", "-d", "-s", "t", "-x", "80", "-y", "24"]);
    let pane_out = tmux.run(&["display-message", "-p", "-t", "t:0.0", "#{pane_id}"]);
    let pane = String::from_utf8_lossy(&pane_out.stdout).trim().to_string();
    assert!(pane.starts_with('%'), "a pane id: {pane:?}");

    // `show -wv` prints nothing when the window holds no value of its own;
    // the effective value comes from `display-message` targeted at the pane.
    // Both are checked: the window's own value must stay unset (nothing was
    // ever written there) and the pane's effective value must track the
    // option.
    let pane_effective = |pane: &str| -> String {
        let out = tmux.run(&["display-message", "-p", "-t", pane, "#{allow-passthrough}"]);
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    let window_own_value = |target: &str| -> Option<String> {
        let out = tmux.run(&["show", "-wv", "-t", target, "allow-passthrough"]);
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!text.is_empty()).then_some(text)
    };
    let pane_value = |pane: &str| -> Option<String> {
        let out = tmux.run(&["show", "-pv", "-t", pane, "allow-passthrough"]);
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!text.is_empty()).then_some(text)
    };

    assert_eq!(
        pane_effective(&pane),
        "off",
        "the pane's effective value before anything"
    );
    assert_eq!(
        window_own_value("t:0"),
        None,
        "the window holds no value of its own"
    );
    assert_eq!(
        pane_value(&pane),
        None,
        "no pane value of its own before the start call"
    );

    let sock = Some(tmux.sock.as_path());
    assert!(
        crate::tmux::set_pane_passthrough(&pane, sock, true),
        "the start call succeeds against the private server"
    );
    assert_eq!(
        pane_value(&pane).as_deref(),
        Some("on"),
        "after the start call"
    );
    assert_eq!(
        pane_effective(&pane),
        "on",
        "the pane's effective value follows"
    );
    assert_eq!(
        window_own_value("t:0"),
        None,
        "the window's own value is untouched"
    );

    assert!(
        crate::tmux::unset_pane_passthrough(&pane, sock),
        "the exit call succeeds against the private server"
    );
    assert_eq!(
        pane_value(&pane),
        None,
        "after the exit call the pane's own value is gone again"
    );
    assert_eq!(
        pane_effective(&pane),
        "off",
        "the effective value is back to off"
    );
    assert_eq!(
        window_own_value("t:0"),
        None,
        "the window still holds no value"
    );
}

// ── Reports reach the injected writer (bug-21c-1) ────────────────────────────

#[test]
fn program_status_emit_reports_reach_the_injected_writer() {
    let (emit, held) = emitter(true, false);
    emit.working();
    emit.blocked(Kind::Permission);
    emit.idle();
    let text = String::from_utf8_lossy(&held.snapshot()).to_string();
    let reports: Vec<&str> = text.split("\x1b]7501;").skip(1).collect();
    assert_eq!(
        reports.len(),
        3,
        "three state changes, three reports on the writer the test holds: {text:?}"
    );
    assert_eq!(
        reports[0].trim_end_matches('\x07'),
        "state=working:app=daemoneye",
        "first report: {text:?}"
    );
    assert_eq!(
        reports[1].trim_end_matches('\x07'),
        "state=blocked:kind=permission:app=daemoneye",
        "second report: {text:?}"
    );
    assert_eq!(
        reports[2].trim_end_matches('\x07'),
        "state=idle:app=daemoneye",
        "third report: {text:?}"
    );
}

// ── No repeats ───────────────────────────────────────────────────────────────

#[test]
fn program_status_emit_repeats_write_nothing() {
    let (emit, held) = emitter(true, false);
    emit.working();
    emit.working();
    emit.working();
    let text = String::from_utf8_lossy(&held.snapshot()).to_string();
    assert_eq!(
        text.matches("\x1b]7501;").count(),
        1,
        "a stream of the same state writes one report: {text:?}"
    );
}

// ── Clear only after a report ────────────────────────────────────────────────

#[test]
fn program_status_emit_clear_only_after_a_report() {
    // One that reported writes exactly one `clear` on finish.
    let (emit, held) = emitter(true, false);
    emit.working();
    emit.finish();
    let text = String::from_utf8_lossy(&held.snapshot()).to_string();
    let reports: Vec<&str> = text.split("\x1b]7501;").skip(1).collect();
    assert_eq!(
        reports.len(),
        2,
        "working plus the exit clear, and nothing else: {text:?}"
    );
    assert!(
        reports[1].contains("state=clear"),
        "the second report is the clear: {text:?}"
    );

    // A silent emitter writes nothing on finish.
    let (emit, held) = emitter(true, false);
    emit.finish();
    assert!(
        held.snapshot().is_empty(),
        "never reported: finish writes nothing: {:?}",
        held.snapshot()
    );

    // Finish twice writes one clear in total.
    let (emit, held) = emitter(true, false);
    emit.working();
    emit.finish();
    emit.finish();
    let text = String::from_utf8_lossy(&held.snapshot()).to_string();
    assert_eq!(
        text.matches("\x1b]7501;").count(),
        2,
        "a second finish writes nothing: {text:?}"
    );
}

// ── Frame mapping ────────────────────────────────────────────────────────────

#[test]
fn program_status_emit_frame_mapping() {
    use crate::ipc::Response as R;
    // Every prompt frame named in Behaviour 1 maps to its `blocked` kind.
    let prompts: Vec<(R, Kind)> = vec![
        (
            R::ToolCallPrompt {
                id: String::new(),
                command: String::new(),
                background: false,
                target_pane: None,
                classification: crate::ipc::Classification::Command,
                interactive: false,
            },
            Kind::Permission,
        ),
        (
            R::EditFilePrompt {
                id: String::new(),
                path: String::new(),
                operation: "edit".to_string(),
                existing_content: None,
                new_content: None,
                dest_path: None,
                sudo: false,
                always_asked: false,
            },
            Kind::Permission,
        ),
        (
            R::ScriptDeletePrompt {
                id: String::new(),
                script_name: String::new(),
                agent: false,
            },
            Kind::Permission,
        ),
        (
            R::ScriptWritePrompt {
                id: String::new(),
                script_name: String::new(),
                content: String::new(),
                existing_content: None,
                agent: false,
            },
            Kind::Permission,
        ),
        (
            R::ScheduleWritePrompt {
                id: String::new(),
                name: String::new(),
                kind: String::new(),
                action: String::new(),
            },
            Kind::Permission,
        ),
        (
            R::RunbookWritePrompt {
                id: String::new(),
                runbook_name: String::new(),
                content: String::new(),
                existing_content: None,
            },
            Kind::Permission,
        ),
        (
            R::RunbookDeletePrompt {
                id: String::new(),
                runbook_name: String::new(),
                active_jobs: Vec::new(),
            },
            Kind::Permission,
        ),
        (
            R::CredentialPrompt {
                id: String::new(),
                prompt: String::new(),
            },
            Kind::Auth,
        ),
        (
            R::PaneSelectPrompt {
                id: String::new(),
                panes: Vec::new(),
            },
            Kind::Question,
        ),
    ];
    for (frame, want) in prompts {
        assert_eq!(
            frame_state(&frame),
            Some(State::Blocked),
            "{} maps to blocked",
            frame.kind()
        );
        assert_eq!(
            frame_kind(&frame),
            Some(want),
            "{} maps to blocked:kind={}",
            frame.kind(),
            want.as_str()
        );
    }

    // Working frames, and the no-op.
    assert_eq!(
        frame_state(&R::Token(String::new())),
        Some(State::Working),
        "a token means working"
    );
    assert_eq!(
        frame_state(&R::ToolStarted {
            id: String::new(),
            tool: String::new(),
            summary: String::new()
        }),
        Some(State::Working),
        "a tool start means working"
    );
    assert_eq!(
        frame_state(&R::ToolResult {
            tool_call_id: String::new(),
            output: String::new()
        }),
        Some(State::Working),
        "a tool result means working"
    );
    assert_eq!(
        frame_state(&R::KeepAlive),
        None,
        "a keepalive changes nothing"
    );
    assert_eq!(frame_kind(&R::KeepAlive), None, "a keepalive names no kind");
}

// ── Reply returns to working ─────────────────────────────────────────────────

#[test]
fn program_status_emit_reply_returns_to_working() {
    let (emit, held) = emitter(true, false);
    emit.blocked(Kind::Permission);
    emit.reply();
    // A repeat of the same blocked state after the reply writes again: the
    // emitter is at working, so blocked is a change.
    emit.blocked(Kind::Permission);
    let text = String::from_utf8_lossy(&held.snapshot()).to_string();
    let first = text
        .find("state=blocked")
        .expect("the blocked report is there");
    let second = text
        .find("state=working")
        .expect("the working report is there");
    assert!(
        first < second,
        "blocked comes before the return to working: {text:?}"
    );
    assert_eq!(
        text.matches("state=working").count(),
        1,
        "after the reply the emitter reports working with no further frame: {text:?}"
    );
}

// ── Only to a terminal ───────────────────────────────────────────────────────

#[test]
fn program_status_emit_not_a_terminal_writes_nothing() {
    // `terminal = false` is the gate the real emitter applies from
    // `std::io::stdout().is_terminal()`: a pipe gets no reports at all.
    let (emit, held) = emitter(false, false);
    emit.working();
    emit.blocked(Kind::Auth);
    emit.idle();
    emit.finish();
    assert!(
        held.snapshot().is_empty(),
        "a non-terminal target gets no reports at all: {:?}",
        held.snapshot()
    );
}

// ── idle on drop (bug-21c-2) ─────────────────────────────────────────────────

#[test]
fn program_status_emit_idle_on_drop_reports_idle() {
    let (emit, held) = emitter(true, false);
    emit.working();
    {
        let _guard = emit.idle_on_drop();
        // The guard is alive: the emitter is still working.
        assert!(
            String::from_utf8_lossy(&held.snapshot()).contains("state=working"),
            "the turn in flight reads working: {:?}",
            held.snapshot()
        );
    }
    let text = String::from_utf8_lossy(&held.snapshot()).to_string();
    assert!(
        text.ends_with("state=idle:app=daemoneye\x07"),
        "dropping the guard after working leaves the last report idle: {text:?}"
    );
    // A guard dropped without a report writes nothing: clear only what was
    // set, and idle-on-drop is no exception.
    let (emit, held) = emitter(true, false);
    {
        let _guard = emit.idle_on_drop();
    }
    assert!(
        held.snapshot().is_empty(),
        "no report yet: the dropped guard writes nothing: {:?}",
        held.snapshot()
    );
}

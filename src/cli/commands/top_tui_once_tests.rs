//! The `top --once` escaping test (M36 phase-09, finding S9): an Activity
//! row's model-authored `cmd` with a raw OSC 52 sequence must not survive
//! `render_once` + `escape_controls` as raw ESC/BEL bytes.

use crate::cli::commands::top::render_once;
use crate::cli::commands::top_tui::tests::{activity_row, snapshot_with};
use crate::cli::render::escape_controls;

/// The OSC 52 string a model-authored `cmd` carries (finding S9).
const OSC52: &str = "ls \x1b]52;c;aGk=\x07";

#[test]
fn terminal_restore_once_activity_row() {
    let snapshot = snapshot_with(
        vec![],
        vec![activity_row("09:00:01", "command_approval", OSC52)],
    );
    let frame = render_once(&snapshot, "");
    let escaped = escape_controls(&frame);
    assert!(
        !escaped.bytes().any(|b| b == 0x1b || b == 0x07),
        "no raw ESC or BEL byte in the --once frame: {escaped:?}"
    );
    assert!(
        escaped.contains("ls "),
        "the command head is still shown: {escaped:?}"
    );
}

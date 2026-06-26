// ── cli module tests ──────────────────────────────────────────────────────────

use crate::cli::render::*;
use crate::daemon::utils::command_has_sudo;

// ── command_has_sudo ──────────────────────────────────────────────────────────

#[test]
fn command_has_sudo_simple_cli() {
    assert!(command_has_sudo("sudo apt install vim"));
}

#[test]
fn command_has_sudo_in_pipeline_cli() {
    assert!(command_has_sudo("echo hi | sudo tee /etc/hosts"));
}

#[test]
fn command_has_sudo_after_semicolon_cli() {
    assert!(command_has_sudo("cd /tmp; sudo rm -rf foo"));
}

#[test]
fn command_has_sudo_false_positive_guard_cli() {
    // "sudoers" is not "sudo" — word-boundary must hold.
    assert!(!command_has_sudo("cat /etc/sudoers"));
}

#[test]
fn command_has_sudo_no_sudo_cli() {
    assert!(!command_has_sudo("ls -la /home"));
}

// ── visual_len ────────────────────────────────────────────────────────────────

#[test]
fn visual_len_plain_ascii() {
    assert_eq!(visual_len("hello"), 5);
}

#[test]
fn visual_len_empty_string() {
    assert_eq!(visual_len(""), 0);
}

#[test]
fn visual_len_strips_ansi_reset() {
    // "\x1b[0m" is an ANSI reset — it contributes 0 visual columns.
    assert_eq!(visual_len("\x1b[0mhello"), 5);
}

#[test]
fn visual_len_strips_ansi_colour() {
    assert_eq!(visual_len("\x1b[31mred\x1b[0m"), 3);
}

#[test]
fn visual_len_strips_bold() {
    assert_eq!(visual_len("\x1b[1mbold text\x1b[0m"), 9);
}

#[test]
fn visual_len_nested_escape_sequences() {
    // Two different ANSI sequences around some text.
    let s = "\x1b[1m\x1b[32mgreen bold\x1b[0m\x1b[0m";
    assert_eq!(visual_len(s), 10);
}

#[test]
fn visual_len_no_escape_inside_word() {
    // "DaemonEye" has no escapes — all 9 chars count.
    assert_eq!(visual_len("DaemonEye"), 9);
}

#[test]
fn wrap_line_hard_with_newlines() {
    use crate::cli::render::wrap_line_hard;
    let input = "line1\nline2";
    let wrapped = wrap_line_hard(input, 10);
    assert_eq!(wrapped, vec!["line1".to_string(), "line2".to_string()]);
}

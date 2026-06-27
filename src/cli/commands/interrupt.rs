//! Two-press interrupt state machine for streaming turns.
//!
//! First interrupt press arms a warning, second press within the same turn
//! aborts the stream. Completing a turn resets the state.

use crate::cli::input::Key;

/// The outcome of feeding an interrupt key to the state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptAction {
    /// Key was not an interrupt key — ignore it.
    Ignore,
    /// First interrupt press — show a warning, keep streaming.
    Warn,
    /// Second interrupt press — abort the turn.
    Abort,
}

/// State machine tracking interrupt presses within a single streaming turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterruptState {
    /// Whether the first interrupt has been pressed (armed).
    armed: bool,
}

impl InterruptState {
    /// Create a fresh state for a new turn.
    pub fn new() -> Self {
        Self { armed: false }
    }

    /// Feed a key into the state machine.
    ///
    /// Returns [`InterruptAction::Warn`] on the first interrupt key,
    /// [`InterruptAction::Abort`] on the second, and [`InterruptAction::Ignore`]
    /// for non-interrupt keys.
    pub fn feed(&mut self, key: &Key) -> InterruptAction {
        if !is_interrupt_key(key) {
            return InterruptAction::Ignore;
        }

        if self.armed {
            self.armed = false; // reset for safety
            InterruptAction::Abort
        } else {
            self.armed = true;
            InterruptAction::Warn
        }
    }

    /// Reset the state after a turn completes normally.
    pub fn reset(&mut self) {
        self.armed = false;
    }

    /// Whether a warning is currently shown (first press armed).
    pub fn is_armed(&self) -> bool {
        self.armed
    }
}

/// Check whether a key is one of the recognized interrupt keys (ESC or Ctrl+C).
pub fn is_interrupt_key(key: &Key) -> bool {
    matches!(key, Key::CtrlC | Key::Char('\x1b'))
}

impl Default for InterruptState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_interrupt_press_warns_keeps_streaming() {
        let mut state = InterruptState::new();
        let action = state.feed(&Key::CtrlC);
        assert_eq!(action, InterruptAction::Warn);
        assert!(state.is_armed());
    }

    #[test]
    fn first_interrupt_press_warns_on_esc() {
        let mut state = InterruptState::new();
        let action = state.feed(&Key::Char('\x1b'));
        assert_eq!(action, InterruptAction::Warn);
        assert!(state.is_armed());
    }

    #[test]
    fn second_interrupt_press_aborts() {
        let mut state = InterruptState::new();
        state.feed(&Key::CtrlC); // first press → warn
        let action = state.feed(&Key::Char('\x1b')); // second press (ESC) → abort
        assert_eq!(action, InterruptAction::Abort);
        assert!(!state.is_armed());
    }

    #[test]
    fn non_interrupt_key_is_ignored_while_streaming() {
        let mut state = InterruptState::new();
        let action = state.feed(&Key::Char('a'));
        assert_eq!(action, InterruptAction::Ignore);
        assert!(!state.is_armed());

        // After arming, non-interrupt keys still ignored
        state.feed(&Key::CtrlC);
        let action = state.feed(&Key::Enter);
        assert_eq!(action, InterruptAction::Ignore);
        assert!(state.is_armed()); // armed state preserved
    }

    #[test]
    fn armed_state_resets_between_turns() {
        let mut state = InterruptState::new();
        state.feed(&Key::CtrlC); // arm
        assert!(state.is_armed());

        // Turn completes — reset
        state.reset();
        assert!(!state.is_armed());

        // Next turn's first press warns again (does not abort)
        let action = state.feed(&Key::CtrlC);
        assert_eq!(action, InterruptAction::Warn);
    }

    #[test]
    fn is_interrupt_key_recognizes_esc_and_ctrl_c() {
        assert!(is_interrupt_key(&Key::CtrlC));
        assert!(is_interrupt_key(&Key::Char('\x1b')));
        assert!(!is_interrupt_key(&Key::Char('a')));
        assert!(!is_interrupt_key(&Key::Enter));
        assert!(!is_interrupt_key(&Key::Backspace));
    }
}

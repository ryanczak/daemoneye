//! The terminal-restore duty: a `TerminalGuard` that runs the restore action
//! at most once (explicit `restore()` or `Drop`, whichever comes first), the
//! process-wide "a terminal renderer is live" flag the guard sets and clears,
//! and the panic hook that restores only while a renderer is live.

use std::sync::atomic::{AtomicBool, Ordering};

/// Whether a real terminal renderer is currently alive. Set by
/// [`TerminalGuard::arm`] on the stdout renderer, cleared when the guard
/// restores. The panic hook reads it to decide whether to restore.
static TERMINAL_RENDERER_LIVE: AtomicBool = AtomicBool::new(false);

/// The panic hook's decision, as a pure function of the live flag: restore
/// only while a real terminal renderer is live. A process that never made a
/// terminal renderer — the daemon, `top --once` — gets no escape sequences.
pub fn panic_hook_restores(renderer_live: bool) -> bool {
    renderer_live
}

/// The real restore: disable focus reporting and bracketed paste, then let
/// ratatui leave raw mode (and any alternate screen). Errors are ignored —
/// a failing restore cannot be reported anywhere better than the terminal
/// it failed on.
pub fn restore_terminal() {
    use crossterm::event::{DisableBracketedPaste, DisableFocusChange};
    use crossterm::execute;
    // The chat client's OSC 7501 exit duty: `clear` only if this process
    // reported, the pane option unset only if it set one (M36 phase 21c).
    // Before the mode restores, so the clear lands on the screen the client
    // was drawing on.
    crate::cli::program_status_emit::exit_clear();
    let _ = execute!(std::io::stdout(), DisableFocusChange, DisableBracketedPaste);
    let _ = ratatui::try_restore();
}

static INSTALL_PANIC_RESTORE: std::sync::Once = std::sync::Once::new();

/// Install the panic hook that restores the terminal while a renderer is
/// live, exactly once per process. The previously installed hook runs after
/// the restore, so the panic message still prints.
pub fn install_panic_restore() {
    INSTALL_PANIC_RESTORE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if panic_hook_restores(TERMINAL_RENDERER_LIVE.load(Ordering::SeqCst)) {
                restore_terminal();
            }
            previous(info);
        }));
    });
}

/// Owns the terminal-restore duty for one terminal renderer: the restore
/// action runs at most once, whether it is triggered by an explicit
/// `restore()` or by `Drop`.
pub struct TerminalGuard {
    restore: fn(),
    armed: bool,
}

impl TerminalGuard {
    /// Arm a guard over `restore`. Called right after the renderer's
    /// `enable_raw_mode`, so a later failure in the constructor still
    /// restores through `Drop`.
    pub fn arm(restore: fn()) -> Self {
        TERMINAL_RENDERER_LIVE.store(true, Ordering::SeqCst);
        Self {
            restore,
            armed: true,
        }
    }

    /// Run the restore action once. A second call, or a following `Drop`,
    /// runs nothing.
    pub fn restore(&mut self) {
        if self.armed {
            self.armed = false;
            TERMINAL_RENDERER_LIVE.store(false, Ordering::SeqCst);
            (self.restore)();
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static DROP_RUNS: AtomicUsize = AtomicUsize::new(0);
    fn count_drop() {
        DROP_RUNS.fetch_add(1, Ordering::SeqCst);
    }

    static EXPLICIT_RUNS: AtomicUsize = AtomicUsize::new(0);
    fn count_explicit() {
        EXPLICIT_RUNS.fetch_add(1, Ordering::SeqCst);
    }

    #[test]
    fn terminal_restore_guard_drop_restores_once() {
        DROP_RUNS.store(0, Ordering::SeqCst);
        let guard = TerminalGuard::arm(count_drop);
        drop(guard);
        assert_eq!(
            DROP_RUNS.load(Ordering::SeqCst),
            1,
            "dropping an armed guard runs the restore action exactly once"
        );
    }

    #[test]
    fn terminal_restore_guard_explicit_then_drop_once() {
        EXPLICIT_RUNS.store(0, Ordering::SeqCst);
        let mut guard = TerminalGuard::arm(count_explicit);
        guard.restore();
        drop(guard);
        assert_eq!(
            EXPLICIT_RUNS.load(Ordering::SeqCst),
            1,
            "an explicit restore followed by a drop is exactly one call in total"
        );
    }

    #[test]
    fn terminal_restore_panic_decision() {
        assert!(
            panic_hook_restores(true),
            "a live renderer: the hook's decision restores"
        );
        assert!(
            !panic_hook_restores(false),
            "no live renderer: the hook's decision does nothing"
        );
    }
}

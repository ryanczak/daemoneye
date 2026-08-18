//! Cooperative cancellation for in-flight interactive turns.
//!
//! Built on `tokio::sync::watch` — a `CancelHandle` flips the signal and a
//! `CancelSignal` observes it. Ported from rexyMCP (`executor/src/agent/
//! cancel.rs`, MIT, same author).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use tokio::sync::watch;

use crate::util::UnpoisonExt;

static REGISTRY: OnceLock<Mutex<HashMap<String, CancelHandle>>> = OnceLock::new();

fn registry() -> &'static Mutex<HashMap<String, CancelHandle>> {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register a turn's cancel handle. Returns a guard that deregisters on
/// drop, so every exit path of the turn cleans up.
pub fn register_turn(session_id: &str) -> (TurnCancelGuard, CancelSignal) {
    let (handle, signal) = CancelSignal::new();
    registry()
        .lock()
        .unwrap_or_log()
        .insert(session_id.to_string(), handle);
    (
        TurnCancelGuard {
            session_id: session_id.to_string(),
        },
        signal,
    )
}

/// Flip the cancel signal for a session's in-flight turn, if any.
/// Returns whether a turn was found.
pub fn cancel_turn(session_id: &str) -> bool {
    match registry().lock().unwrap_or_log().get(session_id) {
        Some(handle) => {
            handle.cancel();
            true
        }
        None => false,
    }
}

pub struct TurnCancelGuard {
    session_id: String,
}

impl Drop for TurnCancelGuard {
    fn drop(&mut self) {
        registry().lock().unwrap_or_log().remove(&self.session_id);
    }
}

/// Handle that can flip the cancellation signal.
pub struct CancelHandle {
    tx: watch::Sender<bool>,
}

impl CancelHandle {
    /// Flip the signal. Ignores a send error from all-receivers-dropped.
    pub fn cancel(&self) {
        let _ = self.tx.send(true);
    }
}

/// Observable side of the cancellation signal.
#[derive(Clone)]
pub struct CancelSignal {
    rx: watch::Receiver<bool>,
}

impl CancelSignal {
    /// Create a fresh pair. The handle starts the signal at `false`.
    pub fn new() -> (CancelHandle, CancelSignal) {
        let (tx, rx) = watch::channel(false);
        (CancelHandle { tx }, CancelSignal { rx })
    }

    /// Check if the signal has been flipped.
    pub fn is_cancelled(&self) -> bool {
        *self.rx.borrow()
    }

    /// Resolve when the signal is flipped. If the sender is dropped before
    /// the flip, park forever.
    pub async fn cancelled(&mut self) {
        loop {
            if *self.rx.borrow() {
                return;
            }
            match self.rx.changed().await {
                Ok(_) => {}
                Err(_) => std::future::pending::<()>().await,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancel_flips_signal() {
        let (handle, mut signal) = CancelSignal::new();
        assert!(!signal.is_cancelled());
        handle.cancel();
        assert!(signal.is_cancelled());
        signal.cancelled().await;
    }

    #[tokio::test]
    async fn clone_observes_flip() {
        let (handle, signal) = CancelSignal::new();
        let mut clone = signal.clone();
        assert!(!clone.is_cancelled());
        handle.cancel();
        assert!(clone.is_cancelled());
        clone.cancelled().await;
    }

    #[tokio::test]
    async fn dropped_handle_does_not_cancel() {
        let (handle, mut signal) = CancelSignal::new();
        drop(handle);
        // With the sender dropped before the flip, `cancelled()` parks
        // forever; returning from this test proves it did not.
        let result =
            tokio::time::timeout(std::time::Duration::from_millis(50), signal.cancelled()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn register_cancel_roundtrip() {
        let session_id = "register-cancel-roundtrip";
        let (_guard, mut signal) = register_turn(session_id);
        assert!(cancel_turn(session_id));
        assert!(signal.is_cancelled());
        signal.cancelled().await;
    }

    #[tokio::test]
    async fn cancel_unknown_session_is_false() {
        let session_id = "no-such-session";
        assert!(!cancel_turn(session_id));
    }

    #[tokio::test]
    async fn guard_drop_deregisters() {
        let session_id = "guard-drop-deregisters";
        {
            let (_guard, _signal) = register_turn(session_id);
        }
        assert!(!cancel_turn(session_id));
    }
}

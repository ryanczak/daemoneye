use std::sync::{LockResult, MutexGuard, RwLockReadGuard, RwLockWriteGuard};

/// Extension trait for `LockResult` that logs an error when recovering from a
/// poisoned mutex/rwlock instead of silently swallowing the poison event.
pub trait UnpoisonExt<T> {
    fn unwrap_or_log(self) -> T;
}

impl<'a, T> UnpoisonExt<MutexGuard<'a, T>> for LockResult<MutexGuard<'a, T>> {
    fn unwrap_or_log(self) -> MutexGuard<'a, T> {
        self.unwrap_or_else(|e| {
            log::error!("Recovering from poisoned Mutex lock");
            e.into_inner()
        })
    }
}

impl<'a, T> UnpoisonExt<RwLockReadGuard<'a, T>> for LockResult<RwLockReadGuard<'a, T>> {
    fn unwrap_or_log(self) -> RwLockReadGuard<'a, T> {
        self.unwrap_or_else(|e| {
            log::error!("Recovering from poisoned RwLock (read) lock");
            e.into_inner()
        })
    }
}

impl<'a, T> UnpoisonExt<RwLockWriteGuard<'a, T>> for LockResult<RwLockWriteGuard<'a, T>> {
    fn unwrap_or_log(self) -> RwLockWriteGuard<'a, T> {
        self.unwrap_or_else(|e| {
            log::error!("Recovering from poisoned RwLock (write) lock");
            e.into_inner()
        })
    }
}

/// Truncate to the largest char boundary at or below `max_bytes`, so a
/// multibyte character straddling the cut can never panic a slice.
pub(crate) fn truncate_chars(s: &str, max_bytes: usize) -> &str {
    &s[..s.floor_char_boundary(max_bytes)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_boundary_webhook_fingerprint() {
        // B3: "abcdefgü" is 9 bytes; the old `[..len().min(8)]` byte slice cut
        // at byte 8 — the middle of 'ü' — and panicked. The char-safe cut at 8
        // returns the largest char boundary at or below 8: "abcdefg" (7 bytes).
        let fp = "abcdefgü";
        assert_eq!(fp.len(), 9);
        let cut = truncate_chars(fp, 8);
        assert_eq!(
            cut, "abcdefg",
            "cut lands on the char boundary at or below 8"
        );
        assert!(cut.len() <= 8);
        assert!(fp.starts_with(cut));

        // 15 ASCII bytes followed by 'ü' (2 bytes): the old `[..16]` panicked;
        // the char-safe cut returns the 15 ASCII bytes.
        let fp = format!("{}ü", "a".repeat(15));
        assert_eq!(fp.len(), 17);
        let cut = truncate_chars(&fp, 16);
        assert_eq!(cut, "a".repeat(15), "cut lands before the straddling 'ü'");
        assert!(cut.len() <= 16);
        assert!(fp.starts_with(cut));

        // ASCII unchanged byte for byte.
        let ascii = "0123456789abcdefghij";
        assert_eq!(truncate_chars(ascii, 16), &ascii[..16]);
    }

    #[test]
    fn char_boundary_frame_preview() {
        // B16: 119 ASCII bytes followed by 'é' and more text; the old
        // `[..len().min(120)]` panicked when the multibyte char straddled
        // byte 120. The char-safe cut returns the 119 ASCII bytes.
        let frame = format!("{}é{}", "x".repeat(119), "y".repeat(20));
        assert_eq!(frame.len(), 141);
        let cut = truncate_chars(&frame, 120);
        assert_eq!(cut, "x".repeat(119), "cut lands before the straddling 'é'");
        assert!(cut.len() <= 120);
        assert!(frame.starts_with(cut));
    }
}

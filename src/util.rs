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

/// Validate a name that is joined into a path under the config directory
/// (memory namespace, session id, prompt name, saved-session name).
/// Valid: non-empty, at most 128 bytes, every byte ASCII `A-Z`, `a-z`, `0-9`, `-` or `_`.
pub fn validate_path_name(kind: &str, name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err(format!("{kind} cannot be empty"));
    }
    if name.len() > 128 {
        return Err(format!(
            "{kind} is too long ({} bytes, maximum is 128)",
            name.len()
        ));
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(format!(
            "{kind} contains characters outside the allowed set A-Z, a-z, 0-9, '-' and '_'"
        ));
    }
    Ok(())
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

#[cfg(test)]
mod path_name_tests {
    use super::validate_path_name;

    #[test]
    fn path_name_validator_accepts_and_refuses() {
        for ok in [
            "global",
            "sre",
            "266f80f28f5db7e6",
            "3f2a9c1e-7b4d-4e8a-9f00-12ab34cd56ef",
            "my_prompt",
            "a".repeat(128).as_str(),
        ] {
            assert!(
                validate_path_name("memory namespace", ok).is_ok(),
                "expected '{ok}' to be accepted"
            );
        }
        for bad in [
            "",
            "..",
            ".",
            "../x",
            "a/b",
            "a.b",
            "a b",
            "a\0b",
            "é",
            &"a".repeat(129),
        ] {
            assert!(
                validate_path_name("memory namespace", bad).is_err(),
                "expected '{bad}' to be refused"
            );
        }
    }
}

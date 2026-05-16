//! G5: Memory review and confidence scoring (stub — not yet implemented).
//!
//! Returns default confidence until the review system is wired in.

use crate::memory::MemoryInfo;

/// Return effective confidence for a memory entry.
/// Defaults to 1.0 until the review system is implemented.
pub fn effective_confidence(_info: &MemoryInfo) -> f64 {
    1.0
}

//! The status bar's fitting half (M37 phase 02a): the `Part` shape, its
//! drop ranks, and the measure-and-drop join — split from `status_bar.rs`
//! to keep that file under the hygiene cap. The ranks order the whole bar;
//! `status_bar_line` in the parent drives them.

use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::gray;

/// The separator between segments: ` · ` at ≥ 36 columns, a single space
/// below.
pub(super) const WIDE_SEP: &str = " · ";
pub(super) const NARROW_SEP: &str = " ";
pub(super) const NARROW_BELOW: usize = 36;

/// One rendered piece of the bar: its spans, its columns, and where it sits
/// in the drop order. Ranks are unique; the lowest remaining rank drops
/// first when the line does not fit.
///
/// Tier 3 (the velocities, the alert name and age, the session, the model,
/// the grants detail) holds ranks 1–8, tier 2 (the counters) 9–12, the
/// grants core 13 — it drops after tier 2 and before tier 1 — and tier 1
/// (state, alerts, context, cost, turn) 14–18, the last resort on a very
/// narrow pane.
#[derive(Debug)]
pub(super) struct Part {
    pub spans: Vec<Span<'static>>,
    pub width: usize,
    pub rank: u16,
}

impl Part {
    pub(super) fn new(spans: Vec<Span<'static>>, rank: u16) -> Self {
        let width = spans.iter().map(Span::width).sum();
        Part { spans, width, rank }
    }
}

// Drop ranks, lowest drops first.
pub(super) const RANK_GROWTH: u16 = 1;
pub(super) const RANK_BURN: u16 = 2;
pub(super) const RANK_THROUGHPUT: u16 = 3;
pub(super) const RANK_ALERT_NAME: u16 = 4;
pub(super) const RANK_ALERT_AGE: u16 = 5;
pub(super) const RANK_SESSION: u16 = 6;
pub(super) const RANK_MODEL: u16 = 7;
pub(super) const RANK_GRANT_DETAIL: u16 = 8;
pub(super) const RANK_SHELLS: u16 = 9;
pub(super) const RANK_TOOLS: u16 = 10;
pub(super) const RANK_GHOSTS: u16 = 11;
pub(super) const RANK_BACKGROUND: u16 = 12;
pub(super) const RANK_GRANTS: u16 = 13;
pub(super) const RANK_TURN: u16 = 14;
pub(super) const RANK_COST: u16 = 15;
pub(super) const RANK_CONTEXT: u16 = 16;
pub(super) const RANK_ALERTS: u16 = 17;
pub(super) const RANK_STATE: u16 = 18;

/// Do all the slots plus the grants segment fit `width` at `sep` between
/// them? The segment is a whole slot's worth of parts: their widths plus
/// one space per internal join.
pub(super) fn line_fits(
    slots: &[Vec<Part>],
    hint_width: Option<usize>,
    sep: &str,
    width: usize,
) -> bool {
    let sep_w = UnicodeWidthStr::width(sep);
    let mut used = 0usize;
    let mut live_slots = 0usize;
    for slot in slots {
        if slot.is_empty() {
            continue;
        }
        live_slots += 1;
        used += slot.iter().map(|p| p.width).sum::<usize>() + slot.len().saturating_sub(1); // the spaces within the slot
    }
    if live_slots > 1 {
        used += sep_w * (live_slots - 1);
    }
    if let Some(hint) = hint_width {
        // Flush right with at least one space before it.
        used += 1 + hint;
    }
    used <= width
}

/// Join the slots into one `Line`, then the grants segment's parts joined by
/// single spaces, flush right when present.
pub(super) fn join_slots(
    slots: &[Vec<Part>],
    hint: &mut Option<Vec<Part>>,
    sep: &str,
    width: usize,
) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    let mut live_slot = false;
    for slot in slots {
        if slot.is_empty() {
            continue;
        }
        if live_slot {
            spans.push(Span::styled(sep.to_string(), gray()));
            used += UnicodeWidthStr::width(sep);
        }
        let mut live_part = false;
        for part in slot {
            if live_part {
                spans.push(Span::styled(" ".to_string(), gray()));
                used += 1;
            }
            spans.extend(part.spans.iter().cloned());
            used += part.width;
            live_part = true;
        }
        live_slot = true;
    }
    if let Some(parts) = hint {
        let hint_w = parts.iter().map(|p| p.width).sum::<usize>() + parts.len().saturating_sub(1);
        let gap = width.saturating_sub(used + hint_w);
        spans.push(Span::styled(" ".repeat(gap), gray()));
        let mut live_part = false;
        for part in parts.iter() {
            if live_part {
                spans.push(Span::styled(" ".to_string(), gray()));
            }
            spans.extend(part.spans.iter().cloned());
            live_part = true;
        }
    }
    Line::from(spans)
}

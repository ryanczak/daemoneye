//! The status bar's fitting half (M37 phase 02a): the drop ranks, and the
//! measure-and-drop join — split from `status_bar.rs` to keep that file under
//! the hygiene cap. The ranks order the whole bar; `status_bar_line` in the
//! parent drives them. Since phase 09 the parts are two-form (`Part` in
//! `status_words.rs`): a wide form with the item's word and a compact form
//! that is byte-for-byte the pre-words bar.

use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::gray;
use super::words::{TwoFormPart, counter_parts, word};
use crate::cli::status_bar::StatusModel;
use crate::cli::status_glyphs::GlyphSet;
/// The separator between segments: ` · ` at ≥ 36 columns, a single space
/// below.
pub(super) const WIDE_SEP: &str = " · ";
pub(super) const NARROW_SEP: &str = " ";
pub(super) const NARROW_BELOW: usize = 36;

// Drop ranks, lowest drops first.
pub(super) const RANK_GROWTH: u16 = 1;
pub(super) const RANK_BURN: u16 = 2;
pub(super) const RANK_THROUGHPUT: u16 = 3;
pub(super) const RANK_ALERT_NAME: u16 = 4;
pub(super) const RANK_ALERT_AGE: u16 = 5;
pub(super) const RANK_WORDS: u16 = 6;
pub(super) const RANK_SESSION: u16 = 7;
pub(super) const RANK_MODEL: u16 = 8;
pub(super) const RANK_GRANT_DETAIL: u16 = 9;
pub(super) const RANK_SHELLS: u16 = 10;
pub(super) const RANK_TOOLS: u16 = 11;
pub(super) const RANK_GHOSTS: u16 = 12;
pub(super) const RANK_BACKGROUND: u16 = 13;
pub(super) const RANK_GRANTS: u16 = 14;
pub(super) const RANK_TURN: u16 = 15;
pub(super) const RANK_COST: u16 = 16;
pub(super) const RANK_CONTEXT: u16 = 17;
pub(super) const RANK_ALERTS: u16 = 18;
pub(super) const RANK_STATE: u16 = 19;

/// The four counters (background, ghosts, tools, shells) as one slot: the
/// parts joined by single spaces inside it, ` · ` between slots. Each keeps
/// its own rank and drops alone; a counter at zero is still absent, and a
/// slot with no parts is skipped by `join_slots` as before.
pub(super) fn counters_slot(model: &StatusModel, g: &GlyphSet) -> Vec<TwoFormPart> {
    let mut parts = Vec::new();
    parts.extend(counter_parts(
        g.background,
        model.background,
        RANK_BACKGROUND,
        word::BACKGROUND,
        word::BACKGROUND,
    ));
    parts.extend(counter_parts(
        g.ghosts,
        model.ghosts,
        RANK_GHOSTS,
        word::GHOST,
        word::GHOSTS,
    ));
    parts.extend(counter_parts(
        g.tools,
        model.tools_total,
        RANK_TOOLS,
        word::TOOL,
        word::TOOLS,
    ));
    parts.extend(counter_parts(
        g.shells,
        model.shells,
        RANK_SHELLS,
        word::SHELL,
        word::SHELLS,
    ));
    parts
}

/// The rank drops, lowest first, re-measuring after each: when the whole
/// line (slots plus the grants segment) fits, the loop stops. The words rank
/// is not a removable part — reaching it sets `wide = false` (the words drop
/// all at once) instead of retaining parts, and every measurement before it
/// uses the wide form. The grants segment dies with its core part.
pub(super) fn drop_loop(
    slots: &mut [Vec<TwoFormPart>],
    grants: &mut Option<Vec<TwoFormPart>>,
    wide: &mut bool,
    sep: &str,
    width: usize,
) {
    let mut ranks: Vec<u16> = slots
        .iter()
        .flat_map(|slot| slot.iter().map(|p| p.rank))
        .chain(grants.iter().flatten().map(|p| p.rank))
        .chain(std::iter::once(RANK_WORDS))
        .collect();
    ranks.sort_unstable();
    ranks.dedup();
    for rank in ranks {
        if line_fits(slots, grants_width(grants, *wide), sep, *wide, width) {
            break;
        }
        if rank == RANK_WORDS {
            *wide = false;
            continue;
        }
        for slot in slots.iter_mut() {
            slot.retain(|p| p.rank != rank);
        }
        if let Some(parts) = grants.as_mut() {
            parts.retain(|p| p.rank != rank);
            if parts.iter().all(|p| p.rank != RANK_GRANTS) {
                *grants = None;
            }
        }
    }
}

/// The grants segment's columns in the form `wide` selects: its parts plus
/// their internal joins.
pub(super) fn grants_width(grants: &Option<Vec<TwoFormPart>>, wide: bool) -> Option<usize> {
    grants.as_ref().map(|parts| {
        parts.iter().map(|p| p.width(wide)).sum::<usize>() + parts.len().saturating_sub(1)
    })
}

/// Do all the slots plus the grants segment fit `width` at `sep` between
/// them, in the form `wide` selects? The segment is a whole slot's worth of
/// parts: their widths plus one space per internal join.
pub(super) fn line_fits(
    slots: &[Vec<TwoFormPart>],
    hint_width: Option<usize>,
    sep: &str,
    wide: bool,
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
        used += slot.iter().map(|p| p.width(wide)).sum::<usize>() + slot.len().saturating_sub(1); // the spaces within the slot
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

/// Join the slots into one `Line` in the form `wide` selects, then the grants
/// segment's parts joined by single spaces, flush right when present.
pub(super) fn join_slots(
    slots: &[Vec<TwoFormPart>],
    hint: &mut Option<Vec<TwoFormPart>>,
    sep: &str,
    wide: bool,
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
            let part_spans = part.spans(wide);
            spans.extend(part_spans.iter().cloned());
            used += part.width(wide);
            live_part = true;
        }
        live_slot = true;
    }
    if let Some(parts) = hint {
        let hint_w =
            parts.iter().map(|p| p.width(wide)).sum::<usize>() + parts.len().saturating_sub(1);
        let gap = width.saturating_sub(used + hint_w);
        spans.push(Span::styled(" ".repeat(gap), gray()));
        let mut live_part = false;
        for part in parts.iter() {
            if live_part {
                spans.push(Span::styled(" ".to_string(), gray()));
            }
            spans.extend(part.spans(wide).iter().cloned());
            live_part = true;
        }
    }
    Line::from(spans)
}

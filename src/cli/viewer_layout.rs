//! Transcript layout: blocks to screen rows, word wrap, scroll clamping and
//! mouse hit-testing.

use super::*;

/// Lay the transcript out at `width` columns: one `ViewRow` per screen row,
/// blocks separated by exactly one blank row (never after the last).
pub fn layout_blocks(blocks: &[Block], width: usize) -> Vec<ViewRow> {
    layout_blocks_with(blocks, width, &HashSet::new())
}

/// Lay out with a set of collapsed block indices. `layout_blocks` is this with
/// an empty set.
pub fn layout_blocks_with(
    blocks: &[Block],
    width: usize,
    collapsed: &HashSet<usize>,
) -> Vec<ViewRow> {
    let mut rows: Vec<ViewRow> = Vec::new();
    for (i, block) in blocks.iter().enumerate() {
        if i > 0 && !rows.is_empty() {
            rows.push(ViewRow {
                text: String::new(),
                kind: RowKind::Blank,
                block: i,
            });
        }
        if collapsed.contains(&i) {
            layout_block_collapsed(block, width, &mut rows, i);
        } else {
            layout_block(block, width, &mut rows, i);
        }
    }
    rows
}

fn layout_block(block: &Block, width: usize, rows: &mut Vec<ViewRow>, idx: usize) {
    match block {
        Block::UserTurn { label, text } => {
            rows.push(ViewRow {
                text: format!("▾ {label}"),
                kind: RowKind::Header,
                block: idx,
            });
            push_wrapped(rows, text, width, RowKind::User, idx);
        }
        Block::Assistant { text } => {
            push_wrapped(rows, text, width, RowKind::Assistant, idx);
        }
        Block::ToolPanel {
            tool,
            summary,
            label,
        } => {
            let header = match label {
                Some(l) => format!("▾ {tool} — {l}"),
                None => format!("▾ {tool}"),
            };
            rows.push(ViewRow {
                text: header,
                kind: RowKind::Header,
                block: idx,
            });
            push_wrapped(rows, summary, width, RowKind::Tool, idx);
        }
        Block::Output { full, .. } => {
            let n = full.lines().count();
            rows.push(ViewRow {
                text: format!("output ({n} lines)"),
                kind: RowKind::Header,
                block: idx,
            });
            // Never elide: showing what the inline panel hid is the point.
            // Machine output keeps the hard wrap — it must not be re-flowed.
            for line in full.lines() {
                push_wrapped_hard(rows, line, width, RowKind::Output, idx);
            }
        }
        Block::System { text } => {
            for (i, line) in text.lines().enumerate() {
                let prefix = if i == 0 { "⚙ " } else { "" };
                push_wrapped(
                    rows,
                    &format!("{prefix}{line}"),
                    width,
                    RowKind::System,
                    idx,
                );
            }
        }
    }
}

/// A block with no header of its own (Assistant, System) uses its first
/// laid-out row as the header row for the collapse suffix.
fn layout_block_collapsed(block: &Block, width: usize, rows: &mut Vec<ViewRow>, idx: usize) {
    let mut full: Vec<ViewRow> = Vec::new();
    layout_block(block, width, &mut full, idx);
    let n = full.len().saturating_sub(1);
    let text = match bare_header(block) {
        Some(bare) => format!("▸ {bare} [collapsed, {n} lines]"),
        None => {
            let first = full.first().map(|r| r.text.clone()).unwrap_or_default();
            format!("{first} [collapsed, {n} lines]")
        }
    };
    rows.push(ViewRow {
        text,
        kind: RowKind::Header,
        block: idx,
    });
}

fn bare_header(block: &Block) -> Option<String> {
    match block {
        Block::UserTurn { label, .. } => Some(label.clone()),
        Block::ToolPanel { tool, label, .. } => Some(match label {
            Some(l) => format!("{tool} — {l}"),
            None => tool.clone(),
        }),
        Block::Output { full, .. } => {
            let n = full.lines().count();
            Some(format!("output ({n} lines)"))
        }
        Block::Assistant { .. } | Block::System { .. } => None,
    }
}

fn push_wrapped(rows: &mut Vec<ViewRow>, text: &str, width: usize, kind: RowKind, idx: usize) {
    for line in wrap_words(text, width) {
        rows.push(ViewRow {
            text: line,
            kind,
            block: idx,
        });
    }
}

/// Row-kind-independent hard wrap, for machine output that must not be
/// re-flowed on word boundaries.
fn push_wrapped_hard(rows: &mut Vec<ViewRow>, text: &str, width: usize, kind: RowKind, idx: usize) {
    for line in crate::cli::render::wrap_line_hard(text, width) {
        rows.push(ViewRow {
            text: line,
            kind,
            block: idx,
        });
    }
}

/// Wrap a prose line on word boundaries: whole words are kept together and a
/// single token longer than the width still breaks rather than overflowing.
/// Leading/trailing whitespace per hard line is preserved so rejoining the
/// rows with single spaces round-trips the input.
pub(super) fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for hard_line in text.split('\n') {
        if hard_line.is_empty() {
            out.push(String::new());
            continue;
        }
        let leading_len = hard_line.len() - hard_line.trim_start().len();
        let trailing_len = hard_line.len() - hard_line.trim_end().len();
        let (leading, rest) = hard_line.split_at(leading_len);
        let (body, trailing) = rest.split_at(rest.len() - trailing_len);

        let mut line = String::with_capacity(width);
        let mut has_content = false;
        line.push_str(leading);
        if !leading.is_empty() && body.trim().is_empty() {
            has_content = true;
        }

        let flush = |line: &mut String, out: &mut Vec<String>, has_content: &mut bool| {
            if *has_content {
                out.push(std::mem::take(line));
            } else {
                out.push(String::new());
            }
            *line = String::with_capacity(width);
            *has_content = false;
        };

        let mut space_remaining: usize = width.saturating_sub(leading_len);

        for tok in body.split(' ') {
            if tok.is_empty() {
                continue;
            }
            if tok.len() > width {
                if has_content {
                    flush(&mut line, &mut out, &mut has_content);
                    space_remaining = width;
                }
                for chunk in tok.as_bytes().chunks(width) {
                    let chunk_txt = std::str::from_utf8(chunk).unwrap_or(tok);
                    if has_content {
                        flush(&mut line, &mut out, &mut has_content);
                        space_remaining = width;
                    }
                    line.push_str(chunk_txt);
                    has_content = true;
                    space_remaining -= chunk_txt.chars().count();
                }
                continue;
            }
            let token_len = tok.chars().count();
            let space_needed = if has_content { 1 } else { 0 };
            if space_remaining >= space_needed + token_len {
                if has_content {
                    line.push(' ');
                }
                line.push_str(tok);
                has_content = true;
                space_remaining -= space_needed + token_len;
            } else {
                flush(&mut line, &mut out, &mut has_content);
                space_remaining = width;
                line.push_str(tok);
                has_content = true;
                space_remaining -= token_len;
            }
        }
        if has_content || !trailing.is_empty() {
            line.push_str(trailing);
        }
        flush(&mut line, &mut out, &mut has_content);
    }
    out
}

/// Clamp a scroll offset so the last page never scrolls past the end.
pub fn clamp_scroll(scroll: usize, total: usize, height: usize) -> usize {
    let max = total.saturating_sub(height);
    scroll.min(max)
}

/// Which transcript row a mouse row lands on, or `None` for the status line
/// or out of range. `area_y` is the body's first screen row, `body_height`
/// its height, `scroll` the current offset, `total` the row count.
pub fn row_at(
    mouse_row: u16,
    area_y: u16,
    body_height: u16,
    scroll: usize,
    total: usize,
) -> Option<usize> {
    if mouse_row < area_y {
        return None;
    }
    if mouse_row >= area_y + body_height {
        return None;
    }
    let idx = scroll + (mouse_row - area_y) as usize;
    if idx >= total {
        return None;
    }
    Some(idx)
}

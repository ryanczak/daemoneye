//! Alt-screen transcript viewer: full-screen read-only pager over the
//! client-side transcript model. See `docs/design/transcript-view.md`.

use crate::cli::transcript::Block;
use ratatui::Frame;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use std::collections::HashSet;

/// What a rendered row is, so the draw pass can style it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Header,
    User,
    Assistant,
    Tool,
    Output,
    System,
    Blank,
}

/// One laid-out screen row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewRow {
    pub text: String,
    pub kind: RowKind,
    /// Index of the block this row came from. Blank separators carry the index
    /// of the block they precede.
    pub block: usize,
}

/// What a keypress means to the viewer. `searching` in `key_action` selects
/// between command mode and search-input mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewerAction {
    ScrollUp,
    ScrollDown,
    PageUp,
    PageDown,
    Top,
    Bottom,
    FocusNext,
    FocusPrev,
    ToggleCollapse,
    CollapseOutputs,
    ExpandAll,
    SearchOpen,
    SearchType(char),
    SearchBackspace,
    SearchCommit,
    SearchCancel,
    MatchNext,
    MatchPrev,
    Copy,
    Quit,
    Ignore,
}

/// Decode one key. `searching` is true while the search prompt is open.
pub fn key_action(key: &crate::cli::input::Key, searching: bool) -> ViewerAction {
    match (searching, key) {
        (true, crate::cli::input::Key::Char('\x1b')) => ViewerAction::SearchCancel,
        (true, crate::cli::input::Key::Enter) => ViewerAction::SearchCommit,
        (true, crate::cli::input::Key::Backspace) => ViewerAction::SearchBackspace,
        (true, crate::cli::input::Key::Char(c)) if !c.is_control() => ViewerAction::SearchType(*c),
        (_, crate::cli::input::Key::Up) => ViewerAction::ScrollUp,
        (_, crate::cli::input::Key::Down) => ViewerAction::ScrollDown,
        (_, crate::cli::input::Key::PageUp) => ViewerAction::PageUp,
        (_, crate::cli::input::Key::PageDown) => ViewerAction::PageDown,
        (_, crate::cli::input::Key::Home) => ViewerAction::Top,
        (_, crate::cli::input::Key::End) => ViewerAction::Bottom,
        (false, crate::cli::input::Key::Char(']')) => ViewerAction::FocusNext,
        (false, crate::cli::input::Key::Char('[')) => ViewerAction::FocusPrev,
        (false, crate::cli::input::Key::Enter) => ViewerAction::ToggleCollapse,
        (false, crate::cli::input::Key::Char('c')) => ViewerAction::CollapseOutputs,
        (false, crate::cli::input::Key::Char('a')) => ViewerAction::ExpandAll,
        (false, crate::cli::input::Key::Char('\x1b'))
        | (false, crate::cli::input::Key::Char('q'))
        | (false, crate::cli::input::Key::CtrlO) => ViewerAction::Quit,
        (false, crate::cli::input::Key::Char('/')) => ViewerAction::SearchOpen,
        (false, crate::cli::input::Key::Char('n')) => ViewerAction::MatchNext,
        (false, crate::cli::input::Key::Char('N')) => ViewerAction::MatchPrev,
        (false, crate::cli::input::Key::Char('y')) => ViewerAction::Copy,
        _ => ViewerAction::Ignore,
    }
}

/// The text a block yields when copied — its real content, with none of the
/// viewer's decoration and independent of whether it is collapsed.
pub fn copy_text(block: &crate::cli::transcript::Block) -> String {
    match block {
        crate::cli::transcript::Block::UserTurn { text, .. } => text.clone(),
        crate::cli::transcript::Block::Assistant { text } => text.clone(),
        crate::cli::transcript::Block::System { text } => text.clone(),
        crate::cli::transcript::Block::Output { full, .. } => full.clone(),
        crate::cli::transcript::Block::ToolPanel {
            tool,
            summary,
            label,
        } => match label {
            Some(l) => format!("{tool} — {l}\n{summary}"),
            None => format!("{tool}\n{summary}"),
        },
    }
}

/// Load `text` into a tmux buffer, and into the system clipboard where the
/// terminal supports it (`-w` uses OSC 52; tmux >= 3.2).
fn copy_to_tmux_buffer(text: &str) -> anyhow::Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = Command::new("tmux")
        .args(["load-buffer", "-w", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("tmux load-buffer: no stdin"))?
        .write_all(text.as_bytes())?;
    let status = child.wait()?;
    if !status.success() {
        anyhow::bail!("tmux load-buffer exited with {status}");
    }
    Ok(())
}

/// Row indices whose text contains `query`, case-insensitively.
/// An empty query matches nothing.
pub fn find_matches(rows: &[ViewRow], query: &str) -> Vec<usize> {
    if query.is_empty() {
        return Vec::new();
    }
    let needle = query.to_lowercase();
    rows.iter()
        .enumerate()
        .filter(|(_, r)| r.text.to_lowercase().contains(&needle))
        .map(|(i, _)| i)
        .collect()
}

/// Next match index, wrapping. `len == 0` yields 0.
pub fn next_match(cur: usize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    (cur + 1) % len
}

/// Previous match index, wrapping. `len == 0` yields 0.
pub fn prev_match(cur: usize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    (cur + len - 1) % len
}

/// Minimal scroll offset that keeps `row` visible in a `height`-row viewport.
/// Returns `scroll` unchanged when the row is already visible.
pub fn scroll_to_row(row: usize, scroll: usize, height: usize) -> usize {
    if row < scroll {
        return row;
    }
    if row >= scroll.saturating_add(height) {
        return row.saturating_add(1).saturating_sub(height);
    }
    scroll
}

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
        Block::Output { full, shown: _, .. } => {
            let n = full.lines().count();
            rows.push(ViewRow {
                text: format!("output ({n} lines)"),
                kind: RowKind::Header,
                block: idx,
            });
            // Never elide: showing what the inline panel hid is the point.
            for line in full.lines() {
                push_wrapped(rows, line, width, RowKind::Output, idx);
            }
        }
        Block::System { text } => {
            let mut first = true;
            for line in text.lines() {
                let prefix = if first { "⚙ " } else { "" };
                first = false;
                let wrapped = crate::cli::render::wrap_line_hard(line, width);
                if wrapped.is_empty() {
                    rows.push(ViewRow {
                        text: prefix.to_string(),
                        kind: RowKind::System,
                        block: idx,
                    });
                } else if let Some((first_line, rest)) = wrapped.split_first() {
                    rows.push(ViewRow {
                        text: format!("{prefix}{first_line}"),
                        kind: RowKind::System,
                        block: idx,
                    });
                    for wl in rest {
                        rows.push(ViewRow {
                            text: wl.clone(),
                            kind: RowKind::System,
                            block: idx,
                        });
                    }
                }
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
    for line in crate::cli::render::wrap_line_hard(text, width) {
        rows.push(ViewRow {
            text: line,
            kind,
            block: idx,
        });
    }
}

/// Clamp a scroll offset so the last page never scrolls past the end.
pub fn clamp_scroll(scroll: usize, total: usize, height: usize) -> usize {
    let max = total.saturating_sub(height);
    scroll.min(max)
}

/// What the search feature needs at draw time, bundled to keep
/// `render_transcript`'s parameter count inside clippy's arity limit.
pub struct SearchState<'a> {
    /// Whether the search prompt is open.
    pub active: bool,
    /// The typed (or committed) query.
    pub query: &'a str,
    /// View row indices whose text contains `query`.
    pub matches: &'a [usize],
    /// Index into `matches` of the current match.
    pub current: usize,
}

/// Render `rows` into a frame at a scroll offset. The bottom row is a status
/// line; the rows above it show `rows[scroll..]`. Never panics on an empty
/// row set or an out-of-range scroll.
pub fn render_transcript(
    f: &mut Frame,
    rows: &[ViewRow],
    scroll: usize,
    focus: usize,
    search: &SearchState,
    evicted: usize,
    note: Option<&str>,
) {
    let area = f.area();
    let body_height = area.height.saturating_sub(1);
    let scroll = clamp_scroll(scroll, rows.len(), body_height as usize);
    let palette = crate::cli::palette::Palette::from_env();
    let current_row = search.matches.get(search.current).copied();

    let mut lines: Vec<Line<'static>> = Vec::new();
    for (row_idx, row) in rows
        .iter()
        .enumerate()
        .skip(scroll)
        .take(body_height as usize)
    {
        let style = if Some(row_idx) == current_row {
            style_for_current(row.kind, palette)
        } else if search.matches.contains(&row_idx) {
            style_for_match(row.kind, palette)
        } else if row.block == focus {
            style_for_focused(row.kind, palette)
        } else {
            style_for(row.kind, palette)
        };
        lines.push(Line::from(Span::styled(row.text.clone(), style)));
    }

    let total = rows.len();
    let shown_from = if total == 0 { 0 } else { scroll + 1 };
    let shown_to = (scroll + body_height as usize).min(total);
    let mut status = format!("transcript — {shown_from}-{shown_to} of {total} lines");
    if evicted > 0 {
        status = format!("{evicted} older blocks evicted · {status}");
    }
    let k = if search.matches.is_empty() {
        0
    } else {
        search.current + 1
    };
    if search.active {
        status = format!(
            "{status} · /{} — {k}/{}",
            search.query,
            search.matches.len()
        );
    } else if !search.matches.is_empty() {
        status = format!(
            "{status} · {k}/{} for \"{}\"",
            search.matches.len(),
            search.query
        );
    }
    if let Some(note_text) = note {
        status = format!("{status} · {note_text}");
    }
    status.push_str(" · [ ] focus · enter collapse · c/a all");
    if !search.matches.is_empty() {
        status.push_str(" · / search · n/N next/prev");
    }
    status.push_str(" · ↑↓ PgUp/PgDn Home/End · esc to close");

    let status_line = Line::from(Span::styled(
        status,
        Style::default().add_modifier(Modifier::DIM),
    ));
    let para = Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false });
    f.render_widget(
        para,
        ratatui::layout::Rect::new(area.x, area.y, area.width, body_height),
    );
    f.render_widget(
        status_line,
        ratatui::layout::Rect::new(area.x, area.y + body_height, area.width, 1),
    );
}

/// Violet-tinted variant marking a row that contains a search match. Distinct
/// from the focused-underlined variant; `style_for_current` is stronger.
fn style_for_match(kind: RowKind, palette: crate::cli::palette::Palette) -> Style {
    style_for(kind, palette).add_modifier(Modifier::BOLD)
}

/// Violet-tinted variant marking the active search match. Strongest of the
/// match styles; wins over focus.
fn style_for_current(kind: RowKind, palette: crate::cli::palette::Palette) -> Style {
    style_for(kind, palette)
        .fg(Color::LightMagenta)
        .add_modifier(Modifier::BOLD)
        .add_modifier(Modifier::UNDERLINED)
}

fn style_for(kind: RowKind, palette: crate::cli::palette::Palette) -> Style {
    match kind {
        RowKind::Header => Style::default()
            .fg(palette.yellow())
            .add_modifier(Modifier::BOLD),
        RowKind::User => Style::default().fg(Color::Cyan),
        RowKind::Assistant => Style::default().fg(Color::White),
        RowKind::Tool => Style::default().fg(Color::Green),
        RowKind::Output => Style::default().fg(Color::Gray),
        RowKind::System => Style::default().fg(Color::Magenta),
        RowKind::Blank => Style::default(),
    }
}

/// Emphasised variant for the focused block's rows: same colour, underlined.
fn style_for_focused(kind: RowKind, palette: crate::cli::palette::Palette) -> Style {
    style_for(kind, palette).add_modifier(Modifier::UNDERLINED)
}

/// RAII guard for the alternate screen: `Drop` runs the teardown even when the
/// guarded scope exits early through `?`, so a viewer failure can never strand
/// the terminal on the alternate screen with the inline viewport unpinned.
/// The teardown action is injectable so tests can count how often it runs
/// without a real terminal.
struct AltScreenGuard<'a> {
    teardown: Box<dyn FnMut() + 'a>,
}

impl<'a> AltScreenGuard<'a> {
    fn new(teardown: impl FnMut() + 'a) -> Self {
        Self {
            teardown: Box::new(teardown),
        }
    }
}

impl Drop for AltScreenGuard<'_> {
    fn drop(&mut self) {
        (self.teardown)();
    }
}

/// Full-screen alternate-screen viewer loop.
///
/// Enters the alternate screen, runs its own key loop, and exits back to the
/// inline renderer via `reanchor()` — never tearing down raw mode, which the
/// chat session still needs.
pub async fn run_transcript_viewer(
    stdin: &crate::cli::input::AsyncStdin,
    sigwinch: &mut tokio::signal::unix::Signal,
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    transcript: &crate::cli::transcript::Transcript,
) -> anyhow::Result<()> {
    use crossterm::cursor::Show;
    use crossterm::execute;
    use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};

    execute!(std::io::stdout(), EnterAlternateScreen)?;

    // From here on the screen is owned by this guard: leaving it and re-pinning
    // the inline viewport runs from `Drop`, so `?`, `break` and `Ok(())` all
    // exit the same way. `let _ =` is required — a `Drop` cannot propagate.
    let _guard = AltScreenGuard::new(|| {
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
        let _ = execute!(std::io::stdout(), Show);
        renderer.reanchor();
    });

    viewer_loop(stdin, sigwinch, transcript).await?;
    Ok(())
}

/// Next focused block index, wrapping. `len == 0` yields 0.
pub fn focus_next(focus: usize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    (focus + 1) % len
}

/// Previous focused block index, wrapping. `len == 0` yields 0.
pub fn focus_prev(focus: usize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    (focus + len - 1) % len
}

/// The fallible body of the viewer. Lives separately from
/// `run_transcript_viewer` so the fullscreen `Terminal` is dropped (buffer
/// cleared) before the `AltScreenGuard` — which owns leaving the alternate
/// screen — runs its teardown after this returns, on every exit path.
async fn viewer_loop(
    stdin: &crate::cli::input::AsyncStdin,
    sigwinch: &mut tokio::signal::unix::Signal,
    transcript: &crate::cli::transcript::Transcript,
) -> anyhow::Result<()> {
    use ratatui::Terminal;
    use ratatui::backend::CrosstermBackend;

    let mut terminal = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;
    let blocks_len = transcript.blocks().len();
    let mut focus = blocks_len.saturating_sub(1);
    let mut collapsed: std::collections::HashSet<usize> = std::collections::HashSet::new();

    let mut searching = false;
    let mut query = String::new();
    let mut current: usize = 0;

    let mut scroll: usize;
    let mut matches: Vec<usize>;
    let mut note: Option<String> = None;
    {
        let size = terminal.size()?;
        let width = size.width as usize;
        let rows = layout_blocks(transcript.blocks(), width);
        let body_height = size.height.saturating_sub(1) as usize;
        scroll = clamp_scroll(usize::MAX, rows.len(), body_height);
        matches = find_matches(&rows, &query);
        terminal.draw(|f| {
            render_transcript(
                f,
                &rows,
                scroll,
                focus,
                &SearchState {
                    active: searching,
                    query: &query,
                    matches: &matches,
                    current,
                },
                transcript.evicted(),
                note.as_deref(),
            );
        })?;
    }

    loop {
        let key = tokio::select! {
            _ = sigwinch.recv() => {
                let size = terminal.size()?;
                let width = size.width as usize;
                let rows = layout_blocks(transcript.blocks(), width);
                let body_height = size.height.saturating_sub(1) as usize;
                scroll = clamp_scroll(scroll, rows.len(), body_height);
                matches = find_matches(&rows, &query);
                current = current.min(matches.len().saturating_sub(1));
                terminal.draw(|f| {
                    render_transcript(
                        f,
                        &rows,
                        scroll,
                        focus,
                        &SearchState {
                            active: searching,
                            query: &query,
                            matches: &matches,
                            current,
                        },
                        transcript.evicted(),
                        note.as_deref(),
                    );
                })?;
                continue;
            }
            key = crate::cli::input::read_key(stdin) => key,
        };
        let Some(key) = key else { break };
        let size = terminal.size()?;
        let width = size.width as usize;
        let rows = layout_blocks(transcript.blocks(), width);
        let body_height = size.height.saturating_sub(1) as usize;
        let mut focus_changed = false;
        let action = key_action(&key, searching);
        if !matches!(action, ViewerAction::Ignore) {
            note = None;
        }
        match action {
            ViewerAction::ScrollUp => {
                scroll = scroll.saturating_sub(1);
            }
            ViewerAction::ScrollDown => {
                scroll = scroll.saturating_add(1);
            }
            ViewerAction::PageUp => {
                scroll = scroll.saturating_sub(body_height.saturating_sub(1));
            }
            ViewerAction::PageDown => {
                scroll = scroll.saturating_add(body_height.saturating_sub(1));
            }
            ViewerAction::Top => scroll = 0,
            ViewerAction::Bottom => scroll = usize::MAX,
            ViewerAction::FocusNext => {
                focus = focus_next(focus, blocks_len);
                focus_changed = true;
            }
            ViewerAction::FocusPrev => {
                focus = focus_prev(focus, blocks_len);
                focus_changed = true;
            }
            ViewerAction::ToggleCollapse => {
                if collapsed.contains(&focus) {
                    collapsed.remove(&focus);
                } else {
                    collapsed.insert(focus);
                }
                matches = find_matches(&rows, &query);
                current = current.min(matches.len().saturating_sub(1));
            }
            ViewerAction::CollapseOutputs => {
                collapsed.clear();
                for (i, block) in transcript.blocks().iter().enumerate() {
                    if matches!(block, crate::cli::transcript::Block::Output { .. }) {
                        collapsed.insert(i);
                    }
                }
                matches = find_matches(&rows, &query);
                current = current.min(matches.len().saturating_sub(1));
            }
            ViewerAction::ExpandAll => {
                collapsed.clear();
                matches = find_matches(&rows, &query);
                current = current.min(matches.len().saturating_sub(1));
            }
            ViewerAction::Quit => break,
            ViewerAction::SearchOpen => {
                searching = true;
                query.clear();
                matches.clear();
                current = 0;
            }
            ViewerAction::SearchType(c) => {
                query.push(c);
                matches = find_matches(&rows, &query);
                current = 0;
                if let Some(&m) = matches.first() {
                    scroll = scroll_to_row(m, scroll, body_height);
                }
            }
            ViewerAction::SearchBackspace => {
                query.pop();
                matches = find_matches(&rows, &query);
                current = 0;
                if let Some(&m) = matches.first() {
                    scroll = scroll_to_row(m, scroll, body_height);
                }
            }
            ViewerAction::SearchCommit => {
                searching = false;
            }
            ViewerAction::SearchCancel => {
                searching = false;
                query.clear();
                matches.clear();
                current = 0;
            }
            ViewerAction::MatchNext => {
                if let Some(&m) = matches.get(current) {
                    current = next_match(current, matches.len());
                    scroll = scroll_to_row(m, scroll, body_height);
                }
            }
            ViewerAction::MatchPrev => {
                if let Some(&m) = matches.get(current) {
                    current = prev_match(current, matches.len());
                    scroll = scroll_to_row(m, scroll, body_height);
                }
            }
            ViewerAction::Copy => {
                if let Some(block) = transcript.blocks().get(focus) {
                    let text = copy_text(block);
                    match copy_to_tmux_buffer(&text) {
                        Ok(()) => {
                            note = Some(format!(
                                "copied {} lines to tmux buffer",
                                text.lines().count()
                            ))
                        }
                        Err(e) => note = Some(format!("copy failed: {e}")),
                    }
                }
            }
            ViewerAction::Ignore => {}
        }
        if focus_changed
            && let Some(row_idx) = rows
                .iter()
                .position(|r| r.block == focus && r.kind == crate::cli::viewer::RowKind::Header)
        {
            scroll = scroll_to_row(row_idx, scroll, body_height);
        }
        scroll = clamp_scroll(scroll, rows.len(), body_height);
        terminal.draw(|f| {
            render_transcript(
                f,
                &rows,
                scroll,
                focus,
                &SearchState {
                    active: searching,
                    query: &query,
                    matches: &matches,
                    current,
                },
                transcript.evicted(),
                note.as_deref(),
            );
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn output_block(full: &str, shown: usize) -> Block {
        Block::Output {
            tool_call_id: "toolu_abc".to_string(),
            full: full.to_string(),
            shown,
        }
    }

    #[test]
    fn layout_blocks_renders_full_output() {
        let mut full = String::new();
        for i in 0..300 {
            full.push_str(&format!("line {i}\n"));
        }
        let rows = layout_blocks(&[output_block(&full, 9)], 100);
        assert_eq!(rows[0].text, "output (300 lines)");
        assert_eq!(rows[0].kind, RowKind::Header);
        assert_eq!(rows.len(), 301);
        assert!(rows[1..].iter().all(|r| r.kind == RowKind::Output));
        assert_eq!(rows[300].text, "line 299");
    }

    #[test]
    fn layout_blocks_separates_blocks_with_one_blank() {
        let blocks = vec![
            Block::Assistant {
                text: "first".to_string(),
            },
            Block::Assistant {
                text: "second".to_string(),
            },
        ];
        let rows = layout_blocks(&blocks, 80);
        let blanks: Vec<&ViewRow> = rows.iter().filter(|r| r.kind == RowKind::Blank).collect();
        assert_eq!(blanks.len(), 1);
        assert_ne!(rows.last().unwrap().kind, RowKind::Blank);
    }

    #[test]
    fn layout_blocks_wraps_to_width() {
        let text = "x".repeat(100);
        let rows = layout_blocks(&[Block::Assistant { text: text.clone() }], 20);
        assert!(rows.iter().all(|r| r.text.chars().count() <= 20));
        let rejoined: String = rows.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(rejoined, text);
    }

    #[test]
    fn layout_blocks_empty_transcript_is_empty() {
        assert!(layout_blocks(&[], 80).is_empty());
    }

    #[test]
    fn clamp_scroll_pins_to_last_page() {
        assert_eq!(clamp_scroll(9999, 100, 10), 90);
        assert_eq!(clamp_scroll(50, 100, 10), 50);
    }

    #[test]
    fn clamp_scroll_zero_when_content_fits() {
        assert_eq!(clamp_scroll(5, 3, 10), 0);
    }

    #[test]
    fn render_transcript_draws_rows_into_backend() {
        let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();
        let rows = vec![
            ViewRow {
                text: "alpha".to_string(),
                kind: RowKind::Header,
                block: 0,
            },
            ViewRow {
                text: "beta".to_string(),
                kind: RowKind::Output,
                block: 0,
            },
            ViewRow {
                text: "gamma".to_string(),
                kind: RowKind::Output,
                block: 0,
            },
            ViewRow {
                text: "delta".to_string(),
                kind: RowKind::Output,
                block: 0,
            },
            ViewRow {
                text: "epsilon".to_string(),
                kind: RowKind::Output,
                block: 0,
            },
            ViewRow {
                text: "zeta".to_string(),
                kind: RowKind::Output,
                block: 0,
            },
            ViewRow {
                text: "eta".to_string(),
                kind: RowKind::Output,
                block: 0,
            },
        ];
        terminal
            .draw(|f| {
                render_transcript(
                    f,
                    &rows,
                    2,
                    0,
                    &SearchState {
                        active: false,
                        query: "",
                        matches: &[],
                        current: 0,
                    },
                    0,
                    None,
                )
            })
            .unwrap();
        let buf = terminal.backend().buffer();
        let all_text: String = buf
            .content
            .iter()
            .flat_map(|c| c.symbol().chars())
            .collect();
        assert!(all_text.contains("gamma"), "got: {all_text}");
        assert!(all_text.contains("of 7 lines"), "got: {all_text}");
        let bottom_row: String = buf
            .content
            .iter()
            .skip((7 * 40) as usize)
            .take(40)
            .flat_map(|c| c.symbol().chars())
            .collect();
        assert!(
            bottom_row.contains("of 7 lines"),
            "status must be on the bottom row, got: {bottom_row}"
        );
    }

    #[test]
    fn render_transcript_survives_scroll_past_end() {
        let mut terminal = Terminal::new(TestBackend::new(80, 8)).unwrap();
        let rows = vec![
            ViewRow {
                text: "one".to_string(),
                kind: RowKind::System,
                block: 0,
            },
            ViewRow {
                text: "two".to_string(),
                kind: RowKind::System,
                block: 0,
            },
        ];
        terminal
            .draw(|f| {
                render_transcript(
                    f,
                    &rows,
                    9999,
                    0,
                    &SearchState {
                        active: false,
                        query: "",
                        matches: &[],
                        current: 0,
                    },
                    1,
                    None,
                )
            })
            .unwrap();
        let buf = terminal.backend().buffer();
        let all_text: String = buf
            .content
            .iter()
            .flat_map(|c| c.symbol().chars())
            .collect();
        assert!(all_text.contains("evicted ·"), "got: {all_text}");
        assert!(all_text.contains("of 2 lines"), "got: {all_text}");
    }

    #[test]
    fn collapsed_output_lays_out_as_exactly_one_row() {
        let mut full = String::new();
        for i in 0..300 {
            full.push_str(&format!("line {i}\n"));
        }
        let before = layout_blocks(&[output_block(&full, 9)], 100).len();
        assert_eq!(before, 301);
        let collapsed = HashSet::from([0]);
        let rows = layout_blocks_with(&[output_block(&full, 9)], 100, &collapsed);
        let block_rows = rows.iter().filter(|r| r.block == 0).count();
        assert_eq!(
            block_rows, 1,
            "collapsed block must contribute exactly 1 row"
        );
        assert!(rows[0].text.contains("[collapsed, 300 lines]"));
    }

    #[test]
    fn expanded_layout_is_unchanged_by_the_new_path() {
        let blocks = vec![
            Block::UserTurn {
                label: "me".to_string(),
                text: "hello".to_string(),
            },
            output_block("lorem\nipsum\ndolor\nsit\namet\nset\num\n", 6),
            Block::Assistant {
                text: "world".to_string(),
            },
            Block::Assistant {
                text: "again".to_string(),
            },
            Block::System {
                text: "⚙ note\n⚙ more".to_string(),
            },
        ];
        let empty = HashSet::new();
        assert_eq!(
            layout_blocks(&blocks, 80),
            layout_blocks_with(&blocks, 80, &empty)
        );
    }

    #[test]
    fn collapse_toggle_is_involutive() {
        let blocks = vec![
            Block::UserTurn {
                label: "me".to_string(),
                text: "hello".to_string(),
            },
            output_block("line one\nline two\nline three\n", 2),
            Block::Assistant {
                text: "world".to_string(),
            },
        ];
        let original = layout_blocks(&blocks, 80);
        let collapsed_rows = layout_blocks_with(&blocks, 80, &HashSet::from([1]));
        assert_ne!(original, collapsed_rows);
        let restored = layout_blocks_with(&blocks, 80, &HashSet::new());
        assert_eq!(original, restored);
    }

    #[test]
    fn collapse_all_outputs_collapses_only_outputs() {
        let block = |i: usize| Block::System {
            text: format!("s{i}"),
        };
        let blocks = [
            output_block("a\nb\n", 1),
            block(0),
            output_block("c\nd\n", 1),
            block(1),
            block(2),
        ];
        assert_eq!(blocks.len(), 5);
        let outputs: Vec<bool> = blocks
            .iter()
            .map(|b| matches!(b, Block::Output { .. }))
            .collect();
        let mut collapsed = HashSet::new();
        for (i, b) in blocks.iter().enumerate() {
            if matches!(b, Block::Output { .. }) {
                collapsed.insert(i);
            }
        }
        assert_eq!(collapsed.len(), 2, "exactly 2 members");
        for &i in &collapsed {
            assert!(outputs[i], "index {i} must be an Output block");
        }
    }

    #[test]
    fn focus_next_wraps_at_last_block() {
        assert_eq!(focus_next(2, 3), 0);
        assert_eq!(focus_next(0, 3), 1);
        assert_eq!(focus_next(0, 0), 0);
    }

    #[test]
    fn focus_prev_wraps_at_first() {
        assert_eq!(focus_prev(0, 3), 2);
        assert_eq!(focus_prev(2, 3), 1);
        assert_eq!(focus_prev(0, 0), 0);
    }

    #[test]
    fn rows_carry_their_source_block_index() {
        let blocks = vec![
            Block::UserTurn {
                label: "me".to_string(),
                text: "hello".to_string(),
            },
            Block::Assistant {
                text: "world".to_string(),
            },
            output_block("lorem\nipsum\n", 1),
        ];
        let rows = layout_blocks(&blocks, 80);
        let mut expected = 0;
        for row in &rows {
            if row.kind == RowKind::Blank {
                expected += 1;
                continue;
            }
            assert_eq!(
                row.block, expected,
                "row {:?} must carry the index of the block it came from",
                row.text
            );
        }
    }

    #[test]
    fn render_transcript_marks_collapsed_and_focused() {
        let collapsed = HashSet::from([1]);
        let blocks = vec![
            Block::UserTurn {
                label: "me".to_string(),
                text: "hello".to_string(),
            },
            output_block("first output line\n", 1),
            Block::System {
                text: "done".to_string(),
            },
        ];
        let rows = layout_blocks_with(&blocks, 60, &collapsed);
        let mut terminal = Terminal::new(TestBackend::new(60, 10)).unwrap();
        terminal
            .draw(|f| {
                render_transcript(
                    f,
                    &rows,
                    0,
                    1,
                    &SearchState {
                        active: false,
                        query: "",
                        matches: &[],
                        current: 0,
                    },
                    0,
                    None,
                )
            })
            .unwrap();
        let buf = terminal.backend().buffer();
        let all_text: String = buf
            .content
            .iter()
            .flat_map(|c| c.symbol().chars())
            .collect();
        assert!(
            all_text.contains('▸'),
            "collapsed marker must draw: {all_text}"
        );
        assert!(all_text.contains("enter collapse"), "got: {all_text}");
    }

    #[test]
    fn key_action_typing_wins_over_commands_while_searching() {
        for ch in ['q', 'c', 'a', '[', ']', 'n'] {
            assert_eq!(
                key_action(&crate::cli::input::Key::Char(ch), true),
                ViewerAction::SearchType(ch),
                "typing {ch} while searching must type, not command"
            );
        }
        assert_eq!(
            key_action(&crate::cli::input::Key::Char('N'), true),
            ViewerAction::SearchType('N'),
            "typing N while searching must type"
        );
    }

    #[test]
    fn key_action_commands_apply_when_not_searching() {
        let cases = [
            ('q', ViewerAction::Quit),
            ('c', ViewerAction::CollapseOutputs),
            ('a', ViewerAction::ExpandAll),
            ('[', ViewerAction::FocusPrev),
            (']', ViewerAction::FocusNext),
            ('n', ViewerAction::MatchNext),
        ];
        for (ch, action) in cases {
            assert_eq!(
                key_action(&crate::cli::input::Key::Char(ch), false),
                action,
                "decoding {ch} when not searching"
            );
        }
        assert_eq!(
            key_action(&crate::cli::input::Key::Char('N'), false),
            ViewerAction::MatchPrev
        );
    }

    #[test]
    fn key_action_escape_cancels_search_but_quits_otherwise() {
        assert_eq!(
            key_action(&crate::cli::input::Key::Char('\x1b'), true),
            ViewerAction::SearchCancel
        );
        assert_eq!(
            key_action(&crate::cli::input::Key::Char('\x1b'), false),
            ViewerAction::Quit
        );
    }

    #[test]
    fn find_matches_empty_query_matches_nothing() {
        let rows = vec![
            ViewRow {
                text: "lorem ipsum".to_string(),
                kind: RowKind::Output,
                block: 0,
            },
            ViewRow {
                text: "dolor sit".to_string(),
                kind: RowKind::Output,
                block: 0,
            },
        ];
        assert_eq!(find_matches(&rows, "").len(), 0);
    }

    #[test]
    fn find_matches_is_case_insensitive() {
        let rows = vec![
            ViewRow {
                text: "Lorem ipsum".to_string(),
                kind: RowKind::Output,
                block: 0,
            },
            ViewRow {
                text: "lorem dolor".to_string(),
                kind: RowKind::Output,
                block: 0,
            },
            ViewRow {
                text: "no hit".to_string(),
                kind: RowKind::Output,
                block: 0,
            },
        ];
        let upper = find_matches(&rows, "LOREM");
        let lower = find_matches(&rows, "lorem");
        assert_eq!(upper, lower);
        assert!(!lower.is_empty(), "case-insensitive query must match");
    }

    #[test]
    fn find_matches_skips_collapsed_block_bodies() {
        let blocks = vec![
            Block::UserTurn {
                label: "me".to_string(),
                text: "subject header".to_string(),
            },
            output_block("needle inside body\nonly\n", 2),
        ];
        let collapsed = HashSet::from([1]);
        let collapsed_rows = layout_blocks_with(&blocks, 60, &collapsed);
        assert_eq!(
            find_matches(&collapsed_rows, "needle").len(),
            0,
            "collapsed body text must not be searchable"
        );
        let expanded_rows = layout_blocks_with(&blocks, 60, &HashSet::new());
        assert!(
            !find_matches(&expanded_rows, "needle").is_empty(),
            "the same query over the expanded layout must match"
        );
    }

    #[test]
    fn next_match_wraps() {
        assert_eq!(next_match(2, 3), 0);
        assert_eq!(next_match(0, 3), 1);
        assert_eq!(next_match(0, 0), 0);
    }

    #[test]
    fn prev_match_wraps() {
        assert_eq!(prev_match(0, 3), 2);
        assert_eq!(prev_match(2, 3), 1);
        assert_eq!(prev_match(0, 0), 0);
    }

    #[test]
    fn scroll_to_row_only_moves_when_offscreen() {
        assert_eq!(scroll_to_row(5, 0, 10), 0, "visible: unchanged");
        assert_eq!(scroll_to_row(2, 5, 10), 2, "above viewport: jump");
        assert_eq!(scroll_to_row(20, 0, 10), 11, "below viewport: jump");
        scroll_to_row(3, 0, 0);
    }

    #[test]
    fn render_transcript_shows_match_counter() {
        let mut terminal = Terminal::new(TestBackend::new(60, 10)).unwrap();
        let rows = vec![
            ViewRow {
                text: "apple pie".to_string(),
                kind: RowKind::Output,
                block: 0,
            },
            ViewRow {
                text: "banana split".to_string(),
                kind: RowKind::Output,
                block: 0,
            },
            ViewRow {
                text: "cherry tart".to_string(),
                kind: RowKind::Output,
                block: 0,
            },
        ];
        terminal
            .draw(|f| {
                render_transcript(
                    f,
                    &rows,
                    0,
                    0,
                    &SearchState {
                        active: false,
                        query: "e",
                        matches: &[0, 1, 2],
                        current: 0,
                    },
                    0,
                    None,
                )
            })
            .unwrap();
        let buf = terminal.backend().buffer();
        let bottom_row: String = buf
            .content
            .iter()
            .skip((9 * 60) as usize)
            .take(60)
            .flat_map(|c| c.symbol().chars())
            .collect();
        assert!(
            bottom_row.contains("1/3"),
            "match counter must be on the status row, got: {bottom_row}"
        );
    }

    #[test]
    fn alt_screen_guard_runs_teardown_on_drop() {
        use std::cell::Cell;
        use std::rc::Rc;

        let teardown_runs = Rc::new(Cell::new(0));
        let count = Rc::clone(&teardown_runs);
        {
            let guard = AltScreenGuard::new(move || {
                count.set(count.get() + 1);
            });
            drop(guard);
        }
        assert_eq!(teardown_runs.get(), 1, "teardown must run exactly once");
    }

    #[test]
    fn alt_screen_guard_runs_teardown_on_normal_exit() {
        use std::cell::Cell;
        use std::rc::Rc;

        let teardown_runs = Rc::new(Cell::new(0));
        let count = Rc::clone(&teardown_runs);
        {
            let _guard = AltScreenGuard::new(move || {
                count.set(count.get() + 1);
            });
        }
        assert_eq!(
            teardown_runs.get(),
            1,
            "a normally-returning guarded scope must run teardown exactly once"
        );
    }

    #[test]
    fn copy_text_copies_full_output_not_the_elided_view() {
        let mut full = String::new();
        for i in 0..300 {
            full.push_str(&format!("line {i}\n"));
        }
        let block = Block::Output {
            tool_call_id: "toolu_abc".to_string(),
            shown: 9,
            full: full.clone(),
        };
        let text = copy_text(&block);
        assert_eq!(text.lines().count(), 300);
        assert_eq!(text, full);
    }

    #[test]
    fn copy_text_of_collapsed_block_is_unchanged() {
        let full = "alpha\nbeta\ngamma\n";
        let block = Block::Output {
            tool_call_id: "toolu_x".to_string(),
            full: full.to_string(),
            shown: 3,
        };
        let collapsed = std::collections::HashSet::from([0usize]);
        let rows = layout_blocks_with(std::slice::from_ref(&block), 60, &collapsed);
        let _ = rows;
        assert_eq!(copy_text(&block), full);
    }

    #[test]
    fn copy_text_omits_viewer_decoration() {
        let sys = Block::System {
            text: "done".to_string(),
        };
        let copied_sys = copy_text(&sys);
        assert!(!copied_sys.starts_with('⚙'), "copied: {copied_sys}");
        let user = Block::UserTurn {
            label: "me".to_string(),
            text: "hello world".to_string(),
        };
        let copied_user = copy_text(&user);
        assert!(!copied_user.contains("me"), "copied: {copied_user}");
    }

    #[test]
    fn copy_text_tool_panel_composes_header_and_summary() {
        let labeled = Block::ToolPanel {
            tool: "cargo build".to_string(),
            summary: "compiling…".to_string(),
            label: Some("2.1s".to_string()),
        };
        assert_eq!(copy_text(&labeled), "cargo build — 2.1s\ncompiling…");
        let unlabeled = Block::ToolPanel {
            tool: "cargo build".to_string(),
            summary: "compiling…".to_string(),
            label: None,
        };
        assert_eq!(copy_text(&unlabeled), "cargo build\ncompiling…");
    }

    #[test]
    fn key_action_y_copies_only_when_not_searching() {
        assert_eq!(
            key_action(&crate::cli::input::Key::Char('y'), false),
            ViewerAction::Copy
        );
        assert_eq!(
            key_action(&crate::cli::input::Key::Char('y'), true),
            ViewerAction::SearchType('y')
        );
    }
}

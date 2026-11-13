//! The chat status bar's model and line (M37 phase 02a): one `StatusModel`
//! the chat loop updates in place, and one pure `status_bar_line` that turns
//! it into the glyph-led single line of M37 decision 3.
//!
//! Three glyph dialects share one slot table, chosen by `[chat] glyphs`;
//! `ascii` is forced when the locale is not UTF-8. Every emitted line is
//! measured with `unicode-width`, never with a char count, and nothing is
//! ever clipped mid-segment except the model name's own 12-column cap.

use std::time::{Duration, Instant};

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::cli::palette::Palette;
use crate::ipc::AlertBadge;
use crate::webhook::active_severity_rank;

/// Which chat state the bar's state slot shows. Set at the points the OSC
/// 7501 emitter is driven, not inferred a second time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatState {
    /// No turn is running.
    Idle,
    /// A turn is streaming.
    Working,
    /// An approval, credential or picker prompt waits.
    AwaitingApproval,
    /// The daemon has never answered or has stopped answering.
    DaemonDown,
    /// The presence connection dropped; a reconnect is under way.
    Reconnecting,
}

/// A glyph dialect's slot table: one field per glyph the bar can lead a
/// segment with. No glyph contains U+FE0F or U+FE0E.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlyphSet {
    pub idle: &'static str,
    pub working: &'static str,
    pub awaiting_approval: &'static str,
    pub daemon_down: &'static str,
    pub reconnecting: &'static str,
    /// The elapsed mark beside a working state's seconds; empty in a dialect
    /// that spells the seconds bare.
    pub elapsed: &'static str,
    pub alert_critical: &'static str,
    pub alert_warning: &'static str,
    pub alert_info: &'static str,
    pub alert_unknown: &'static str,
    /// The resolve flash shown for ten seconds after an alert leaves the
    /// snapshot.
    pub alert_resolved: &'static str,
    pub context_empty: &'static str,
    pub context_1: &'static str,
    pub context_2: &'static str,
    pub context_3: &'static str,
    pub context_full: &'static str,
    pub context_growth: &'static str,
    pub cost: &'static str,
    pub cost_untracked: &'static str,
    pub cost_burn: &'static str,
    pub turn: &'static str,
    pub throughput_fast: &'static str,
    pub throughput_slow: &'static str,
    pub background: &'static str,
    pub ghosts: &'static str,
    pub tools: &'static str,
    pub shells: &'static str,
    pub vendor_anthropic: &'static str,
    pub vendor_openai: &'static str,
    pub vendor_gemini: &'static str,
    /// The local (non-cloud) mark; a dialect without one shows the name bare.
    pub vendor_local: &'static str,
    pub session: &'static str,
    /// The dirty mark beside a saved session's name.
    pub session_dirty: &'static str,
    pub grants: &'static str,
}

/// The emoji dialect: the default.
pub static EMOJI: GlyphSet = GlyphSet {
    idle: "💤",
    working: "🧠",
    awaiting_approval: "✋",
    daemon_down: "💀",
    reconnecting: "📡",
    elapsed: "⏱",
    alert_critical: "🔥",
    alert_warning: "⚠",
    alert_info: "ℹ",
    alert_unknown: "❔",
    alert_resolved: "🧯",
    context_empty: "🌑",
    context_1: "🌒",
    context_2: "🌓",
    context_3: "🌔",
    context_full: "🌕",
    context_growth: "📈",
    cost: "💸",
    cost_untracked: "❔",
    cost_burn: "⛽",
    turn: "↻",
    throughput_fast: "🐇",
    throughput_slow: "🐢",
    background: "⏳",
    ghosts: "👻",
    tools: "🛠",
    shells: "🐚",
    vendor_anthropic: "⟡",
    vendor_openai: "◎",
    vendor_gemini: "♊",
    vendor_local: "🦙",
    session: "📌",
    session_dirty: "✎",
    grants: "⚡",
};

/// The symbols dialect.
pub static SYMBOLS: GlyphSet = GlyphSet {
    idle: "○",
    working: "◐",
    awaiting_approval: "⏸",
    daemon_down: "✖",
    reconnecting: "⟳",
    elapsed: "⏱",
    alert_critical: "●",
    alert_warning: "●",
    alert_info: "●",
    alert_unknown: "●",
    alert_resolved: "✓",
    context_empty: "◔",
    context_1: "◑",
    context_2: "◕",
    context_3: "●",
    context_full: "●",
    context_growth: "↗",
    cost: "$",
    cost_untracked: "?",
    cost_burn: "$/h",
    turn: "↻",
    throughput_fast: "🐇",
    throughput_slow: "🐢",
    background: "⧖",
    ghosts: "◆",
    tools: "⚒",
    shells: "⌨",
    vendor_anthropic: "⟡",
    vendor_openai: "◎",
    vendor_gemini: "♊",
    vendor_local: "",
    session: "▣",
    session_dirty: "✎",
    grants: "⚡",
};

/// The ascii dialect, forced when the locale is not UTF-8.
pub static ASCII: GlyphSet = GlyphSet {
    idle: "-",
    working: "*",
    awaiting_approval: "?",
    daemon_down: "X",
    reconnecting: "~",
    elapsed: "",
    alert_critical: "!!",
    alert_warning: "!",
    alert_info: "i",
    alert_unknown: "?",
    alert_resolved: "✓",
    context_empty: "ctx",
    context_1: "ctx",
    context_2: "ctx",
    context_3: "ctx",
    context_full: "ctx",
    context_growth: "",
    cost: "$",
    cost_untracked: "?",
    cost_burn: "$/h",
    turn: "t",
    throughput_fast: "t/s",
    throughput_slow: "t/s",
    background: "bg",
    ghosts: "gh",
    tools: "tl",
    shells: "sh",
    vendor_anthropic: "",
    vendor_openai: "",
    vendor_gemini: "",
    vendor_local: "",
    session: "@",
    session_dirty: "*",
    grants: "A:",
};

/// The dialect names `[chat] glyphs` accepts, in the order the load error
/// names them.
pub const GLYPH_DIALECTS: [&str; 3] = ["emoji", "symbols", "ascii"];

/// The set a config name resolves to. `None` for a name that is not one of
/// the three dialects — the config load turns that into an error.
pub fn glyph_set_by_name(name: &str) -> Option<&'static GlyphSet> {
    match name {
        "emoji" => Some(&EMOJI),
        "symbols" => Some(&SYMBOLS),
        "ascii" => Some(&ASCII),
        _ => None,
    }
}

/// The dialect's name, for doctor and the config error paths.
pub fn glyph_set_name(g: &GlyphSet) -> &'static str {
    if std::ptr::eq(g, &EMOJI) {
        "emoji"
    } else if std::ptr::eq(g, &SYMBOLS) {
        "symbols"
    } else {
        "ascii"
    }
}

/// Whether `LANG`, `LC_ALL` or `LC_CTYPE` names a UTF-8 locale (a
/// case-insensitive `utf-8`/`utf8` match). All three lacking one — unset
/// included — is not UTF-8.
pub fn locale_is_utf8(lang: Option<&str>, lc_all: Option<&str>, lc_ctype: Option<&str>) -> bool {
    fn utf8(v: &str) -> bool {
        let lower = v.to_ascii_lowercase();
        lower.contains("utf-8") || lower.contains("utf8")
    }
    [lang, lc_all, lc_ctype]
        .into_iter()
        .flatten()
        .any(|v| !v.is_empty() && utf8(v))
}

/// The set the session actually speaks: the configured dialect when the
/// locale is UTF-8, ASCII otherwise — the locale rule wins over the config.
pub fn glyph_set_for_locale(configured: &'static GlyphSet, utf8: bool) -> &'static GlyphSet {
    if utf8 { configured } else { &ASCII }
}

/// A one-line sample of `g`'s glyphs, for `doctor` — the user sees what their
/// terminal does with the set before they rely on it.
pub fn glyph_sample(g: &GlyphSet) -> String {
    [
        g.idle,
        g.working,
        g.awaiting_approval,
        g.daemon_down,
        g.reconnecting,
        g.alert_critical,
        g.alert_warning,
        g.alert_info,
        g.context_empty,
        g.context_full,
        g.cost,
        g.turn,
        g.background,
        g.ghosts,
        g.tools,
        g.shells,
        g.session,
    ]
    .join(" ")
}

/// How long a resolved alert's flash lives.
pub const RESOLVE_FLASH_SECS: Duration = Duration::from_secs(10);

/// Everything the status bar line reads. The chat loop owns one and updates
/// it in place; the line builder is pure over it.
#[derive(Debug, Clone)]
pub struct StatusModel {
    /// The session id the client is attached to.
    pub session_id: String,
    /// The approval hint, flush right.
    pub approval_hint: String,
    /// The model id the session runs, as the config names it.
    pub model: String,
    /// The chat's turn counter: 0 before the first turn, the resumed count
    /// after `--resume`.
    pub turn: usize,
    pub prompt_tokens: u32,
    pub context_window: u32,
    /// The prompt tokens of the previous completed turn; `None` before the
    /// first turn completes. The growth slot derives from it.
    pub previous_prompt_tokens: Option<u32>,
    /// The last finished turn's output tokens and duration, for throughput.
    /// The tokens stay zero until phase 02b feeds them.
    pub last_output_tokens: u32,
    pub last_turn_duration: Option<Duration>,
    /// The instant the current turn started; `None` when idle.
    pub turn_started_at: Option<Instant>,
    /// The session's monotonic start, for the burn rate's denominator.
    pub session_started_at: Instant,
    /// The session's start in unix seconds, the anchor the alert ages are
    /// measured from.
    pub session_start_unix: u64,
    /// Cumulative session cost in USD.
    pub cost_usd: f64,
    /// Whether any AI call in this session had Unknown pricing.
    pub has_untracked: bool,
    /// The chat state, shared with the OSC 7501 emitter's transitions.
    pub state: ChatState,
    /// The firing alerts the daemon pushed, in wire order.
    pub alerts: Vec<AlertBadge>,
    /// The alert a frame most recently removed, shown as the resolve flash
    /// for ten seconds.
    pub resolved_flash: Option<AlertBadge>,
    /// When the current flash started.
    pub resolve_flash_started_at: Option<Instant>,
    /// This session's running `wait: false` commands. Zero until phase 02b.
    pub background: u32,
    /// Active ghost runs. Zero until phase 02b.
    pub ghosts: u32,
    /// Silent tool calls this session.
    pub tools_total: u32,
    /// Open PTY shells this session. Zero until phase 02b.
    pub shells: u32,
    /// The saved session's name; `None` when unnamed.
    pub session_name: Option<String>,
    /// Whether the saved session has changes since its last save.
    pub session_dirty: bool,
    /// The dialect the bar speaks.
    pub glyphs: &'static GlyphSet,
}

impl Default for StatusModel {
    fn default() -> Self {
        StatusModel {
            session_id: String::new(),
            approval_hint: String::new(),
            model: String::new(),
            turn: 0,
            prompt_tokens: 0,
            context_window: 0,
            previous_prompt_tokens: None,
            last_output_tokens: 0,
            last_turn_duration: None,
            turn_started_at: None,
            session_started_at: Instant::now(),
            session_start_unix: unix_now(),
            cost_usd: 0.0,
            has_untracked: false,
            state: ChatState::Idle,
            alerts: Vec::new(),
            resolved_flash: None,
            resolve_flash_started_at: None,
            background: 0,
            ghosts: 0,
            tools_total: 0,
            shells: 0,
            session_name: None,
            session_dirty: false,
            glyphs: &EMOJI,
        }
    }
}

/// Unix seconds right now. The one wall-clock read the model makes, at
/// construction; every age it displays derives from this anchor.
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl StatusModel {
    /// Swap in a new alert snapshot. When a fingerprint the previous frame
    /// held is gone, the newest removed alert becomes the resolve flash: ten
    /// seconds of the resolved glyph and its name. A frame that removes
    /// nothing leaves an ongoing flash alone.
    pub fn set_alerts(&mut self, alerts: Vec<AlertBadge>, now: Instant) {
        let kept: std::collections::BTreeSet<&str> =
            alerts.iter().map(|a| a.fingerprint.as_str()).collect();
        let mut removed: Vec<AlertBadge> = self
            .alerts
            .iter()
            .filter(|a| !kept.contains(a.fingerprint.as_str()))
            .cloned()
            .collect();
        removed.sort_by_key(|a| std::cmp::Reverse(a.last_seen_unix));
        self.alerts = alerts;
        if let Some(newest) = removed.first() {
            self.resolved_flash = Some(newest.clone());
            self.resolve_flash_started_at = Some(now);
        }
    }

    /// Whether a resolve flash is still live at `now`.
    pub fn resolve_flash_live(&self, now: Instant) -> bool {
        self.resolve_flash_started_at
            .is_some_and(|t| now.duration_since(t) < RESOLVE_FLASH_SECS)
    }

    /// Whether the bar carries anything time-based, so the input loop keeps
    /// redrawing it once a second: a running turn, a firing alert, or a live
    /// resolve flash.
    pub fn is_time_based(&self, now: Instant) -> bool {
        self.state == ChatState::Working || !self.alerts.is_empty() || self.resolve_flash_live(now)
    }

    /// Fold the session's daemon liveness into the chat state. A liveness
    /// report only ever demotes an idle-looking state — it never overrides a
    /// running turn or a waiting prompt.
    pub fn set_daemon_up(&mut self, up: bool) {
        match (up, self.state) {
            (true, ChatState::DaemonDown | ChatState::Reconnecting) => {
                self.state = ChatState::Idle;
            }
            (false, ChatState::Idle) => self.state = ChatState::DaemonDown,
            _ => {}
        }
    }

    /// The unix seconds `now` corresponds to: the session's start anchor plus
    /// the monotonic time since it.
    fn now_unix(&self, now: Instant) -> u64 {
        let elapsed = now
            .saturating_duration_since(self.session_started_at)
            .as_secs();
        self.session_start_unix.saturating_add(elapsed)
    }
}

// ── The line ─────────────────────────────────────────────────────────────────

/// The separator between segments: ` · ` at ≥ 36 columns, a single space
/// below.
const WIDE_SEP: &str = " · ";
const NARROW_SEP: &str = " ";
const NARROW_BELOW: usize = 36;

/// One rendered piece of the bar: its spans, its columns, and where it sits
/// in the drop order. Ranks are unique; the lowest remaining rank drops
/// first when the line does not fit.
///
/// Tier 3 (the velocities, the alert name and age, the session, the model)
/// holds ranks 1–7, tier 2 (the counters) 8–11, the grant hint 12 — it drops
/// after tier 2 and before tier 1 — and tier 1 (state, alerts, context, cost,
/// turn) 13–17, the last resort on a very narrow pane.
#[derive(Debug)]
struct Part {
    spans: Vec<Span<'static>>,
    width: usize,
    rank: u16,
}

impl Part {
    fn new(spans: Vec<Span<'static>>, rank: u16) -> Self {
        let width = spans.iter().map(Span::width).sum();
        Part { spans, width, rank }
    }
}

// Drop ranks, lowest drops first.
const RANK_GROWTH: u16 = 1;
const RANK_BURN: u16 = 2;
const RANK_THROUGHPUT: u16 = 3;
const RANK_ALERT_NAME: u16 = 4;
const RANK_ALERT_AGE: u16 = 5;
const RANK_SESSION: u16 = 6;
const RANK_MODEL: u16 = 7;
const RANK_SHELLS: u16 = 8;
const RANK_TOOLS: u16 = 9;
const RANK_GHOSTS: u16 = 10;
const RANK_BACKGROUND: u16 = 11;
const RANK_HINT: u16 = 12;
const RANK_TURN: u16 = 13;
const RANK_COST: u16 = 14;
const RANK_CONTEXT: u16 = 15;
const RANK_ALERTS: u16 = 16;
const RANK_STATE: u16 = 17;

fn gray() -> Style {
    Style::default().fg(Color::Gray)
}

/// Build the status bar's single line from the model, `now`, the palette and
/// the pane width. Pure: no frame, no terminal, no clock read — every
/// instant is a field of the model or passed in.
///
/// The line is built at full detail, measured with `unicode-width`, and when
/// it does not fit the pane, parts drop one at a time in the rank order,
/// re-measuring each time. Nothing is clipped mid-segment except the model
/// name's own 12-column cap.
pub fn status_bar_line(
    model: &StatusModel,
    now: Instant,
    palette: &Palette,
    width: u16,
) -> Line<'static> {
    let width = width as usize;
    let g = model.glyphs;
    let sep = if width >= NARROW_BELOW {
        WIDE_SEP
    } else {
        NARROW_SEP
    };

    let mut slots: Vec<Vec<Part>> = vec![
        vec![state_part(model, now, g)],
        alert_parts(model, now, g, palette),
        context_parts(model, g, palette),
        cost_parts(model, now, g),
        turn_parts(model, g),
        counter_parts(g.background, model.background, RANK_BACKGROUND),
        counter_parts(g.ghosts, model.ghosts, RANK_GHOSTS),
        counter_parts(g.tools, model.tools_total, RANK_TOOLS),
        counter_parts(g.shells, model.shells, RANK_SHELLS),
        model_parts(model, g),
        session_parts(model, g),
    ];
    let mut hint = hint_part(model);

    // The ranks that exist right now, lowest first; each drop re-measures.
    let mut ranks: Vec<u16> = slots
        .iter()
        .flat_map(|slot| slot.iter().map(|p| p.rank))
        .collect();
    if hint.is_some() {
        ranks.push(RANK_HINT);
    }
    ranks.sort_unstable();
    ranks.dedup();
    for rank in ranks {
        if line_fits(&slots, hint.as_ref().map(|p| p.width), sep, width) {
            break;
        }
        if rank == RANK_HINT {
            hint = None;
        } else {
            for slot in &mut slots {
                slot.retain(|p| p.rank != rank);
            }
        }
    }
    join_slots(&slots, &mut hint, sep, width)
}

/// Do all the slots plus the hint fit `width` at `sep` between them?
fn line_fits(slots: &[Vec<Part>], hint_width: Option<usize>, sep: &str, width: usize) -> bool {
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

/// Join the slots into one `Line`, the hint flush right when present.
fn join_slots(
    slots: &[Vec<Part>],
    hint: &mut Option<Part>,
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
    if let Some(h) = hint {
        let gap = width.saturating_sub(used + h.width);
        spans.push(Span::styled(" ".repeat(gap), gray()));
        spans.extend(h.spans.iter().cloned());
    }
    Line::from(spans)
}

/// The state segment: the state glyph; while `Working`, the elapsed mark and
/// the seconds since the turn started; when down, the words `daemon down`.
fn state_part(model: &StatusModel, now: Instant, g: &GlyphSet) -> Part {
    let glyph = match model.state {
        ChatState::Idle => g.idle,
        ChatState::Working => g.working,
        ChatState::AwaitingApproval => g.awaiting_approval,
        ChatState::DaemonDown => g.daemon_down,
        ChatState::Reconnecting => g.reconnecting,
    };
    let mut text = glyph.to_string();
    if model.state == ChatState::Working {
        let secs = model
            .turn_started_at
            .map(|t| now.saturating_duration_since(t).as_secs())
            .unwrap_or(0);
        if g.elapsed.is_empty() {
            text.push_str(&format!("{secs}s"));
        } else {
            text.push_str(&format!(" {}{secs}s", g.elapsed));
        }
    } else if model.state == ChatState::DaemonDown {
        text.push_str(" daemon down");
    }
    Part::new(vec![Span::styled(text, gray())], RANK_STATE)
}

/// The alert slot: the worst severity's glyph and the count of all firing,
/// then — droppable — the name and age of the newest alert of that severity.
/// For ten seconds after a frame removes an alert, the slot instead shows the
/// resolved glyph and that alert's name. Hidden when nothing fires and no
/// flash is live.
fn alert_parts(model: &StatusModel, now: Instant, g: &GlyphSet, palette: &Palette) -> Vec<Part> {
    let severity_style = |rank: u8| match rank {
        3 => gray().fg(palette.red()),
        2 => gray().fg(palette.yellow()),
        _ => gray(),
    };
    let severity_glyph = |rank: u8| match rank {
        3 => g.alert_critical,
        2 => g.alert_warning,
        1 => g.alert_info,
        _ => g.alert_unknown,
    };
    let flash_live = model.resolve_flash_live(now);
    if flash_live && model.alerts.is_empty() {
        let Some(resolved) = &model.resolved_flash else {
            return Vec::new();
        };
        let rank = active_severity_rank(&resolved.severity);
        let mut parts = vec![Part::new(
            vec![Span::styled(
                g.alert_resolved.to_string(),
                severity_style(rank),
            )],
            RANK_ALERTS,
        )];
        if !resolved.name.is_empty() {
            parts.push(Part::new(
                vec![Span::styled(resolved.name.clone(), gray())],
                RANK_ALERT_NAME,
            ));
        }
        return parts;
    }
    if model.alerts.is_empty() {
        return Vec::new();
    }
    let worst_rank = model
        .alerts
        .iter()
        .map(|a| active_severity_rank(&a.severity))
        .max()
        .unwrap_or(0);
    let style = severity_style(worst_rank);
    let mut parts = vec![Part::new(
        vec![
            Span::styled(severity_glyph(worst_rank).to_string(), style),
            Span::styled(model.alerts.len().to_string(), style),
        ],
        RANK_ALERTS,
    )];
    let newest = model
        .alerts
        .iter()
        .filter(|a| active_severity_rank(&a.severity) == worst_rank)
        .max_by_key(|a| a.last_seen_unix);
    if let Some(newest) = newest {
        if !newest.name.is_empty() {
            parts.push(Part::new(
                vec![Span::styled(newest.name.clone(), gray())],
                RANK_ALERT_NAME,
            ));
        }
        let age = model.now_unix(now).saturating_sub(newest.last_seen_unix);
        parts.push(Part::new(
            vec![Span::styled(age_str(age), gray())],
            RANK_ALERT_AGE,
        ));
    }
    parts
}

/// Age as `45s`, `3m`, `2h`, `5d`.
fn age_str(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

/// The context slot: the fill glyph for `prompt_tokens / context_window` in
/// five steps, then the percentage — amber at ≥ 75 %, red at ≥ 90 % — then,
/// droppable, the growth since the previous turn's prompt, amber when that
/// is ≥ 10 % of the window. With no usage yet, the empty glyph and `—`.
fn context_parts(model: &StatusModel, g: &GlyphSet, palette: &Palette) -> Vec<Part> {
    let pct = if model.context_window == 0 {
        0
    } else {
        (f64::from(model.prompt_tokens) / f64::from(model.context_window) * 100.0).floor() as u32
    };
    let fill = if model.prompt_tokens == 0 {
        g.context_empty
    } else if pct < 20 {
        g.context_1
    } else if pct < 45 {
        g.context_2
    } else if pct < 90 {
        g.context_3
    } else {
        g.context_full
    };
    let color = if pct >= 90 {
        palette.red()
    } else if pct >= 75 {
        palette.yellow()
    } else {
        Color::Gray
    };
    let value = if model.prompt_tokens == 0 {
        "—".to_string()
    } else if model.context_window == 0 {
        compact_tokens(model.prompt_tokens)
    } else {
        format!("{pct}%")
    };
    let mut parts = vec![Part::new(
        vec![
            Span::styled(fill.to_string(), gray().fg(color)),
            Span::styled(value, gray().fg(color)),
        ],
        RANK_CONTEXT,
    )];
    if let Some(previous) = model.previous_prompt_tokens {
        let growth = model.prompt_tokens.saturating_sub(previous);
        if growth > 0 && !g.context_growth.is_empty() {
            let amber = model.context_window > 0
                && u64::from(growth) * 10 >= u64::from(model.context_window);
            let style = if amber {
                gray().fg(palette.yellow())
            } else {
                gray()
            };
            parts.push(Part::new(
                vec![
                    Span::styled(g.context_growth.to_string(), style),
                    Span::styled(compact_tokens(growth), style),
                ],
                RANK_GROWTH,
            ));
        }
    }
    parts
}

/// The cost slot: the cost glyph and `cost_usd` to two decimals, the unknown
/// glyph after it when any call had unknown pricing, then — droppable — the
/// burn rate over the session's hours.
fn cost_parts(model: &StatusModel, now: Instant, g: &GlyphSet) -> Vec<Part> {
    let mut cost_text = format!("{}{:.2}", g.cost, model.cost_usd);
    if model.has_untracked {
        cost_text.push_str(g.cost_untracked);
    }
    let mut parts = vec![Part::new(vec![Span::styled(cost_text, gray())], RANK_COST)];
    let hours = now
        .saturating_duration_since(model.session_started_at)
        .as_secs_f64()
        / 3600.0;
    if model.cost_usd > 0.0 && hours >= 0.01 && !g.cost_burn.is_empty() {
        parts.push(Part::new(
            vec![Span::styled(
                format!("{}{:.1}/h", g.cost_burn, model.cost_usd / hours),
                gray(),
            )],
            RANK_BURN,
        ));
    }
    parts
}

/// The turn slot: the turn glyph and the count, then — droppable, and only
/// once a turn has finished — the last turn's throughput.
fn turn_parts(model: &StatusModel, g: &GlyphSet) -> Vec<Part> {
    let mut parts = vec![Part::new(
        vec![
            Span::styled(g.turn.to_string(), gray()),
            Span::styled(model.turn.to_string(), gray()),
        ],
        RANK_TURN,
    )];
    if let (Some(duration), true) = (model.last_turn_duration, model.last_output_tokens > 0) {
        let secs = duration.as_secs_f64();
        if secs >= 0.5 {
            let tps = model.last_output_tokens as f64 / secs;
            let glyph = if tps >= 30.0 {
                g.throughput_fast
            } else {
                g.throughput_slow
            };
            parts.push(Part::new(
                vec![Span::styled(
                    format!("{}{}t/s", glyph, tps.round() as u64),
                    gray(),
                )],
                RANK_THROUGHPUT,
            ));
        }
    }
    parts
}

/// One counter slot: glyph and count, hidden at zero.
fn counter_parts(glyph: &str, count: u32, rank: u16) -> Vec<Part> {
    if count == 0 {
        return Vec::new();
    }
    vec![Part::new(
        vec![
            Span::styled(glyph.to_string(), gray()),
            Span::styled(count.to_string(), gray()),
        ],
        rank,
    )]
}

/// The model slot: the vendor mark and the short name.
fn model_parts(model: &StatusModel, g: &GlyphSet) -> Vec<Part> {
    let name = short_model_name(&model.model);
    if name.is_empty() {
        return Vec::new();
    }
    let mark = vendor_mark(&model.model, g);
    let text = if mark.is_empty() {
        name
    } else {
        format!("{mark} {name}")
    };
    vec![Part::new(vec![Span::styled(text, gray())], RANK_MODEL)]
}

/// The vendor mark for a model id: Anthropic, OpenAI, Gemini, otherwise the
/// local mark (which `SYMBOLS` and `ASCII` do not carry).
fn vendor_mark(model: &str, g: &GlyphSet) -> &'static str {
    let m = model.to_ascii_lowercase();
    if m.starts_with("claude") {
        g.vendor_anthropic
    } else if m.starts_with("gpt-") || m.starts_with("o1") || m.starts_with("o3") {
        g.vendor_openai
    } else if m.starts_with("gemini") {
        g.vendor_gemini
    } else {
        g.vendor_local
    }
}

/// Shorten a model id for the bar: a `vendor/` path and the vendor prefix
/// (`claude-`, `gpt-`, `gemini-`) removed, a trailing `-YYYYMMDD` snapshot
/// date removed, then clipped to 12 columns.
fn short_model_name(model: &str) -> String {
    let mut name = model.rsplit('/').next().unwrap_or(model).to_string();
    for prefix in ["claude-", "gpt-", "gemini-"] {
        if let Some(rest) = name.strip_prefix(prefix) {
            name = rest.to_string();
            break;
        }
    }
    let bytes = name.as_bytes();
    if name.len() > 9
        && bytes[name.len() - 9] == b'-'
        && bytes[name.len() - 8..].iter().all(u8::is_ascii_digit)
    {
        name.truncate(name.len() - 9);
    }
    clip_to_width(&name, 12)
}

/// The longest prefix of `s` whose display width is at most `max` columns. A
/// grapheme wider than the remaining budget ends the string before it.
fn clip_to_width(s: &str, max: usize) -> String {
    let mut kept = String::new();
    let mut used = 0usize;
    for ch in s.chars() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > max {
            break;
        }
        used += w;
        kept.push(ch);
    }
    kept
}

/// The session slot: hidden when unnamed; the session glyph, the dirty mark
/// when dirty, and the saved name.
fn session_parts(model: &StatusModel, g: &GlyphSet) -> Vec<Part> {
    let Some(name) = model.session_name.as_deref().filter(|n| !n.is_empty()) else {
        return Vec::new();
    };
    let mut text = g.session.to_string();
    if model.session_dirty && !g.session_dirty.is_empty() {
        text.push_str(g.session_dirty);
    }
    text.push(' ');
    text.push_str(name);
    vec![Part::new(vec![Span::styled(text, gray())], RANK_SESSION)]
}

/// The grant hint, flush right. The hint string carries its own `⚡` (or
/// `A:` in ascii) — the glyph dialect's `grants` slot is the prefix the
/// existing `SessionApproval::hint` already emits, so nothing is added here.
fn hint_part(model: &StatusModel) -> Option<Part> {
    if model.approval_hint.is_empty() {
        return None;
    }
    Some(Part::new(
        vec![Span::styled(model.approval_hint.clone(), gray())],
        RANK_HINT,
    ))
}

/// Compact token count: below 1 000 as is (`950`); below 10 000 with one
/// decimal (`4321` → `4.3k`); below 1 000 000 as whole thousands (`41234` →
/// `41k`); from 1 000 000 as millions with one decimal (`1048576` → `1.0M`).
pub fn compact_tokens(n: u32) -> String {
    if n < 1_000 {
        format!("{n}")
    } else if n < 10_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else if n < 1_000_000 {
        format!("{}k", n / 1_000)
    } else {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    }
}

#[cfg(test)]
#[path = "status_bar_tests.rs"]
mod tests;

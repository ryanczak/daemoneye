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

use crate::cli::palette::Palette;
use crate::cli::status_glyphs::{ASCII, EMOJI, GlyphSet, SYMBOLS};
use crate::ipc::AlertBadge;
use crate::shell::now_unix;
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

/// How long a resolved alert's flash lives.
pub const RESOLVE_FLASH_SECS: Duration = Duration::from_secs(10);

#[path = "status_grants.rs"]
mod grants;
use grants::grants_parts;
pub use grants::{CmdGrant, GrantSummary, Scope, grants_active_kinds, grants_lead_colour};

/// Everything the status bar line reads. The chat loop owns one and updates
/// it in place; the line builder is pure over it.
#[derive(Debug, Clone)]
pub struct StatusModel {
    /// The session id the client is attached to.
    pub session_id: String,
    /// The session's grants, as the grants segment reads them.
    pub grants: GrantSummary,
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
            grants: GrantSummary::default(),
            model: String::new(),
            turn: 0,
            prompt_tokens: 0,
            context_window: 0,
            previous_prompt_tokens: None,
            last_output_tokens: 0,
            last_turn_duration: None,
            turn_started_at: None,
            session_started_at: Instant::now(),
            session_start_unix: now_unix(),
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

    /// Apply a `SessionCounters` push (M37 02b) to the bar's counters: the
    /// background, ghost and shell slots. The draw hides zero counters; a
    /// nonzero frame shows them again.
    pub fn set_session_counters(&mut self, background: u32, ghosts: u32, shells: u32) {
        self.background = background;
        self.ghosts = ghosts;
        self.shells = shells;
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

    /// A turn was sent — the emitter's `working`. Starts the elapsed clock,
    /// remembers this prompt's tokens as the growth baseline, and marks a
    /// named session dirty.
    pub fn begin_turn(&mut self, now: Instant) {
        self.state = ChatState::Working;
        self.turn_started_at = Some(now);
        self.previous_prompt_tokens = Some(self.prompt_tokens);
        if self.session_name.is_some() {
            self.session_dirty = true;
        }
    }

    /// An approval prompt opened — the emitter's `blocked`.
    pub fn prompt_opened(&mut self) {
        self.state = ChatState::AwaitingApproval;
    }

    /// The prompt's reply was sent — the emitter's `reply`, back to
    /// `working`. The turn's start instant is left alone.
    pub fn prompt_answered(&mut self) {
        self.state = ChatState::Working;
    }

    /// The turn ended — the emitter's `idle`. `reached_daemon` is whether the
    /// ask got through: a failed ask ends the turn `DaemonDown`, which the
    /// next successful ask clears.
    pub fn end_turn(&mut self, now: Instant, reached_daemon: bool) {
        self.state = if reached_daemon {
            ChatState::Idle
        } else {
            ChatState::DaemonDown
        };
        self.last_turn_duration = self
            .turn_started_at
            .map(|t| now.saturating_duration_since(t));
        self.turn_started_at = None;
    }

    /// A text push from the parked presence task: "daemon connection lost…"
    /// means `Reconnecting`, and "reconnected" clears it (or a stale
    /// `DaemonDown`) to `Idle`. A running turn or open prompt is never
    /// overridden by the reconnect.
    pub fn presence_text(&mut self, msg: &str) {
        if msg.starts_with("daemon connection lost") {
            self.state = ChatState::Reconnecting;
        } else if msg == "reconnected" {
            self.set_daemon_up(true);
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
        counter_parts(
            g.background,
            model.background,
            RANK_BACKGROUND,
            word::BACKGROUND,
            word::BACKGROUND,
        ),
        counter_parts(
            g.ghosts,
            model.ghosts,
            RANK_GHOSTS,
            word::GHOST,
            word::GHOSTS,
        ),
        counter_parts(
            g.tools,
            model.tools_total,
            RANK_TOOLS,
            word::TOOL,
            word::TOOLS,
        ),
        counter_parts(
            g.shells,
            model.shells,
            RANK_SHELLS,
            word::SHELL,
            word::SHELLS,
        ),
        model_parts(model, g),
        session_parts(model, g),
    ];
    // The grants segment sits flush right, as `join_slots`'s hint path
    // places it — it is never a ` · ` slot.
    let grants_parts_vec = grants_parts(model, g, palette);
    let mut grants = if grants_parts_vec.is_empty() {
        None
    } else {
        Some(grants_parts_vec)
    };

    // Wide first: every part carries its word. If the whole line (slots plus
    // the grants segment) does not fit, every part drops to its compact form
    // at once — never a mixed bar — and stays there: the rank drops then run
    // over the compact parts only. The state and severity words are one-form,
    // so they survive at every width the drops leave them.
    let wide = line_fits(&slots, grants_width(&grants), sep, true, width);

    // The ranks that exist right now, lowest first; each drop re-measures.
    let mut ranks: Vec<u16> = slots
        .iter()
        .flat_map(|slot| slot.iter().map(|p| p.rank))
        .chain(grants.iter().flatten().map(|p| p.rank))
        .collect();
    ranks.sort_unstable();
    ranks.dedup();
    for rank in ranks {
        if line_fits(&slots, grants_width_compact(&grants), sep, wide, width) {
            break;
        }
        for slot in &mut slots {
            slot.retain(|p| p.rank != rank);
        }
        if let Some(parts) = grants.as_mut() {
            parts.retain(|p| p.rank != rank);
            if parts.iter().all(|p| p.rank != RANK_GRANTS) {
                grants = None;
            }
        }
    }
    join_slots(&slots, &mut grants, sep, wide, width)
}

/// The grants segment's columns: its parts plus their internal joins.
fn grants_width(grants: &Option<Vec<Part>>) -> Option<usize> {
    grants.as_ref().map(|parts| {
        parts.iter().map(|p| p.width(true)).sum::<usize>() + parts.len().saturating_sub(1)
    })
}

/// The columns the grants segment measures in the compact form, once the
/// words have dropped.
fn grants_width_compact(grants: &Option<Vec<Part>>) -> Option<usize> {
    grants.as_ref().map(|parts| {
        parts.iter().map(|p| p.width(false)).sum::<usize>() + parts.len().saturating_sub(1)
    })
}

/// The state segment: the state glyph and the state word, one form — the
/// word never drops. While `Working` the word is `thinking` and the seconds
/// since the turn started follow it (`thinking 12s` reads as elapsed, so the
/// `⏱` mark is gone); when down, `daemon down`.
fn state_part(model: &StatusModel, now: Instant, g: &GlyphSet) -> Part {
    let (glyph, text) = match model.state {
        ChatState::Idle => (g.idle, word::IDLE.to_string()),
        ChatState::Working => {
            let secs = model
                .turn_started_at
                .map(|t| now.saturating_duration_since(t).as_secs())
                .unwrap_or(0);
            (g.working, format!("{} {secs}s", word::THINKING))
        }
        ChatState::AwaitingApproval => (g.awaiting_approval, word::APPROVE.to_string()),
        ChatState::DaemonDown => (g.daemon_down, word::DAEMON_DOWN.to_string()),
        ChatState::Reconnecting => (g.reconnecting, word::RECONNECTING.to_string()),
    };
    Part::new(
        vec![Span::styled(format!("{glyph} {text}"), gray())],
        RANK_STATE,
    )
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
    let severity_word = |rank: u8| match rank {
        3 => word::CRITICAL,
        2 => word::WARNING,
        1 => word::INFO,
        _ => word::ALERT,
    };
    let flash_live = model.resolve_flash_live(now);
    if flash_live && model.alerts.is_empty() {
        let Some(resolved) = &model.resolved_flash else {
            return Vec::new();
        };
        let rank = active_severity_rank(&resolved.severity);
        // Wide: `🧯 resolved Watchdog`; compact: `🧯 Watchdog` (today).
        let wide = if resolved.name.is_empty() {
            vec![Span::styled(
                format!("{} {}", g.alert_resolved, word::RESOLVED),
                severity_style(rank),
            )]
        } else {
            vec![
                Span::styled(
                    format!("{} {}", g.alert_resolved, word::RESOLVED),
                    severity_style(rank),
                ),
                Span::styled(format!(" {}", resolved.name), gray()),
            ]
        };
        let compact = vec![Span::styled(
            g.alert_resolved.to_string(),
            severity_style(rank),
        )];
        let mut parts = vec![Part {
            wide,
            compact,
            rank: RANK_ALERTS,
        }];
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
    // The severity part: the word stays at every width, so the wide and
    // compact forms differ only in the count. Wide shows the worst
    // severity's own count only at two or more (`⚠ 2 warn`); the compact
    // form is today's bar — the glyph and the count of every firing alert.
    let worst_count = model
        .alerts
        .iter()
        .filter(|a| active_severity_rank(&a.severity) == worst_rank)
        .count();
    let wide = if worst_count >= 2 {
        vec![
            Span::styled(severity_glyph(worst_rank).to_string(), style),
            Span::styled(
                format!(" {worst_count} {}", severity_word(worst_rank)),
                style,
            ),
        ]
    } else {
        vec![
            Span::styled(severity_glyph(worst_rank).to_string(), style),
            Span::styled(format!(" {}", severity_word(worst_rank)), style),
        ]
    };
    let compact = vec![
        Span::styled(severity_glyph(worst_rank).to_string(), style),
        Span::styled(model.alerts.len().to_string(), style),
    ];
    let mut parts = vec![Part {
        wide,
        compact,
        rank: RANK_ALERTS,
    }];
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

pub(super) use words::{age_str, clip_to_width, compact_tokens};

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
    let mut parts = vec![part_two_form(
        gray().fg(color),
        fill,
        value.clone(),
        // The ascii dialect's glyph already spells `ctx`: the wide form is
        // the glyph alone followed by the value, so it never reads `ctx ctx`.
        if g.context_empty == "ctx" {
            None
        } else {
            Some((format!("{} {value}", word::CONTEXT), false))
        },
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
            let shown = compact_tokens(growth);
            parts.push(part_two_form(
                style,
                g.context_growth,
                shown.clone(),
                // The growth word is a sign, not a name: `📈 +3.1k` wide,
                // the bare value compact.
                if g.context_growth.is_empty() {
                    None
                } else {
                    Some((format!("+{shown}"), false))
                },
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
    let spend = format!("{:.2}", model.cost_usd);
    // The symbols and ascii glyphs already say `$`: their wide form is the
    // glyph followed by the value, never `$$`.
    let dollar_glyph = g.cost.contains('$');
    let cost = part_two_form(
        gray(),
        g.cost,
        format!(
            "{spend}{}",
            if model.has_untracked {
                g.cost_untracked
            } else {
                ""
            }
        ),
        if dollar_glyph {
            Some((spend.clone(), false))
        } else {
            Some((format!("${spend}"), false))
        },
        RANK_COST,
    );
    let mut parts = vec![cost];
    let hours = now
        .saturating_duration_since(model.session_started_at)
        .as_secs_f64()
        / 3600.0;
    if model.cost_usd > 0.0 && hours >= 0.01 && !g.cost_burn.is_empty() {
        let rate = format!("{:.1}/h", model.cost_usd / hours);
        let burn_glyph = g.cost_burn.contains('$');
        parts.push(part_two_form(
            gray(),
            g.cost_burn,
            rate.clone(),
            if burn_glyph {
                Some((format!("{rate} {}", word::BURN), false))
            } else {
                Some((format!("${rate} {}", word::BURN), false))
            },
            RANK_BURN,
        ));
    }
    parts
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

#[path = "status_bar_fit.rs"]
mod fit;
#[path = "status_words.rs"]
pub(crate) mod words;
use fit::{
    NARROW_BELOW, NARROW_SEP, RANK_ALERT_AGE, RANK_ALERT_NAME, RANK_ALERTS, RANK_BACKGROUND,
    RANK_BURN, RANK_CONTEXT, RANK_COST, RANK_GHOSTS, RANK_GRANTS, RANK_GROWTH, RANK_MODEL,
    RANK_SESSION, RANK_SHELLS, RANK_STATE, RANK_TOOLS, WIDE_SEP, join_slots, line_fits,
};
use words::{TwoFormPart as Part, counter_parts, turn_parts, two_form as part_two_form, word};

#[cfg(test)]
#[path = "status_bar_tests.rs"]
mod tests;

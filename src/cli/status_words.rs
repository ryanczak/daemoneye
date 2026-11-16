//! The status bar's word half (M37 phase 09, PE decision 7): every part
//! carries a wide form — its glyph plus a pithy word — and a compact form,
//! which is byte-for-byte what the bar rendered before this phase. When the
//! line does not fit, every part drops to its compact form at once, never a
//! mixed bar; the state and alert-severity words are exempt (one form).

use super::gray;
use ratatui::style::Style;
use ratatui::text::Span;
use unicode_width::UnicodeWidthStr;

/// The words the bar's wide forms speak. Same words in every dialect; the
/// glyphs differ per dialect, the words do not.
pub(super) mod word {
    pub const IDLE: &str = "idle";
    pub const THINKING: &str = "thinking";
    pub const APPROVE: &str = "approve?";
    pub const DAEMON_DOWN: &str = "daemon down";
    pub const RECONNECTING: &str = "reconnecting";

    pub const CRITICAL: &str = "crit";
    pub const WARNING: &str = "warn";
    pub const INFO: &str = "info";
    /// An unrankable severity's word.
    pub const ALERT: &str = "alert";
    pub const RESOLVED: &str = "resolved";

    pub const CONTEXT: &str = "ctx";
    pub const BURN: &str = "burn";
    pub const TURN: &str = "turn";
    pub const TOK_PER_S: &str = "tok/s";
    pub const MODEL: &str = "model:";

    pub const BACKGROUND: &str = "bg";
    pub const GHOST: &str = "ghost";
    pub const GHOSTS: &str = "ghosts";
    pub const TOOL: &str = "tool";
    pub const TOOLS: &str = "tools";
    pub const SHELL: &str = "shell";
    pub const SHELLS: &str = "shells";

    pub const SUDO: &str = "sudo";
    pub const SCRIPTS: &str = "scripts";
    pub const RUNBOOKS: &str = "runbooks";
    pub const FILES: &str = "files";
    pub const SCHEDULES: &str = "schedules";
    pub const AGENTS: &str = "agents";
    pub const DELETES: &str = "deletes";
    pub const HOST: &str = "host";
    pub const HOSTS: &str = "hosts";
    pub const ALL: &str = "all";
    pub const GRANTS: &str = "grants";

    /// `1` → the singular, anything else → the plural.
    pub fn plural(count: u32, one: &str, many: &str) -> String {
        if count == 1 {
            one.to_string()
        } else {
            many.to_string()
        }
    }
}

/// One rendered piece of the bar in its two forms. `wide` is the glyph plus
/// its word; `compact` is the glyph-and-number form. The two are equal for a
/// part with no word, and the exempt parts carry only one form.
#[derive(Debug, Clone)]
pub(super) struct TwoFormPart {
    pub wide: Vec<Span<'static>>,
    pub compact: Vec<Span<'static>>,
    pub rank: u16,
}

/// One glyph-value part in its two forms.
///
/// `compact` is the pre-words bar's byte-for-byte text (glyph then value, one
/// style). The wide form is `glyph ␣ word` — or `word` alone when `replace`
/// is set, the choice for a dialect whose glyph already spells the word
/// (`ctx`, `bg`, `$`, `$/h` …). `word = None` makes the two forms equal.
pub(super) fn two_form(
    style: Style,
    glyph: &str,
    compact_value: String,
    word: Option<(String, bool)>,
    rank: u16,
) -> TwoFormPart {
    let compact = vec![
        Span::styled(glyph.to_string(), style),
        Span::styled(compact_value, style),
    ];
    let wide = match &word {
        Some((w, true)) => vec![Span::styled(w.clone(), style)],
        Some((w, false)) => vec![
            Span::styled(format!("{glyph} "), style),
            Span::styled(w.clone(), style),
        ],
        None => compact.clone(),
    };
    TwoFormPart {
        wide,
        compact,
        rank,
    }
}

/// Is this glyph a letter abbreviation (the ascii dialect's `bg`, `tl`, `t`)
/// that the wide form's word replaces, rather than a mark the word follows?
pub(super) fn spells_its_word(glyph: &str) -> bool {
    !glyph.is_empty() && glyph.chars().all(|c| c.is_ascii_alphabetic())
}

/// A one-form part: the exempt state and severity words, and every part with
/// no word (the model, the session, the alert name and age). Dropping to
/// compact is a no-op — the pre-words builders' `Part::new(spans, rank)`.
pub(super) fn one_form(spans: Vec<Span<'static>>, rank: u16) -> TwoFormPart {
    TwoFormPart {
        wide: spans.clone(),
        compact: spans,
        rank,
    }
}

/// One counter slot: glyph and count, hidden at zero. The wide form is the
/// count, a space, then the pluralised word (`🐚 2 shells`). Where the glyph
/// already spells an abbreviation of the word (the ascii `bg`, `gh`, `tl`,
/// `sh`), the word replaces the glyph, so `bg 2 bg` never renders.
pub(super) fn counter_parts(
    glyph: &str,
    count: u32,
    rank: u16,
    word_one: &str,
    word_many: &str,
) -> Vec<TwoFormPart> {
    if count == 0 {
        return Vec::new();
    }
    let w = word::plural(count, word_one, word_many);
    // The ascii dialect's glyphs are letters (`bg`, `gh`, `tl`, `sh`): the
    // word replaces them in the wide form, so `7 tools`, never `tl 7 tools`.
    let spelled = spells_its_word(glyph);
    vec![two_form(
        gray(),
        glyph,
        count.to_string(),
        Some((format!("{count} {w}"), spelled)),
        rank,
    )]
}

/// The turn slot: the turn glyph and the count, then — droppable, and only
/// once a turn has finished — the last turn's throughput.
pub(super) fn turn_parts(
    model: &crate::cli::status_bar::StatusModel,
    g: &crate::cli::status_glyphs::GlyphSet,
) -> Vec<TwoFormPart> {
    // The ascii turn glyph is `t`: the word replaces it in the wide form,
    // so `turn 12`, never `t turn 12`.
    let turn = two_form(
        gray(),
        g.turn,
        model.turn.to_string(),
        Some((
            format!("{} {}", word::TURN, model.turn),
            spells_its_word(g.turn),
        )),
        super::fit::RANK_TURN,
    );
    let mut parts = vec![turn];
    if let (Some(duration), true) = (model.last_turn_duration, model.last_output_tokens > 0) {
        let secs = duration.as_secs_f64();
        if secs >= 0.5 {
            let tps = model.last_output_tokens as f64 / secs;
            let glyph = if tps >= 30.0 {
                g.throughput_fast
            } else {
                g.throughput_slow
            };
            let rate = tps.round() as u64;
            // The ascii throughput glyph is `t/s`: the rate joins it bare, so
            // `38t/s` compact and `38 tok/s` wide — never a doubled `t/s`.
            let spelled = glyph.contains("t/s");
            let glyph = if spelled { "" } else { glyph };
            parts.push(two_form(
                gray(),
                glyph,
                format!("{rate}t/s"),
                Some((format!("{rate} {}", word::TOK_PER_S), spelled)),
                super::fit::RANK_THROUGHPUT,
            ));
        }
    }
    parts
}

impl TwoFormPart {
    /// The legacy constructor: a part with no word, both forms equal.
    pub(super) fn new(spans: Vec<Span<'static>>, rank: u16) -> Self {
        one_form(spans, rank)
    }

    /// The active form's spans: wide until the words drop.
    pub(super) fn spans(&self, wide: bool) -> &[Span<'static>] {
        if wide { &self.wide } else { &self.compact }
    }

    /// The columns the active form measures.
    pub(super) fn width(&self, wide: bool) -> usize {
        form_width(self.spans(wide))
    }
}

/// The columns a form's spans measure.
pub(super) fn form_width(spans: &[Span<'static>]) -> usize {
    spans
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum()
}

/// Age as `45s`, `3m`, `2h`, `5d`.
pub(crate) fn age_str(secs: u64) -> String {
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

/// The longest prefix of `s` whose display width is at most `max` columns. A
/// grapheme wider than the remaining budget ends the string before it.
pub(crate) fn clip_to_width(s: &str, max: usize) -> String {
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

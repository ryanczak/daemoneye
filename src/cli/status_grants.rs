//! The grants segment's summary types and part builder: a child module of
//! `status_bar.rs`, re-exported from it so callers keep the
//! `crate::cli::status_bar::GrantSummary` path.

use ratatui::style::{Color, Style};

use super::fit::{RANK_GRANT_DETAIL, RANK_GRANTS};
use super::words::{TwoFormPart as Part, two_form as part_two_form, word};
use super::{StatusModel, gray};
use crate::cli::palette::Palette;
use crate::cli::status_glyphs::GlyphSet;

/// How the session's command grants stand. `Auto` is the shipped default and
/// carries no glyph; a gated session and a sudo grant both show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CmdGrant {
    /// Non-sudo commands prompt; no grant.
    Gated,
    /// Non-sudo commands auto-approve; the default state.
    #[default]
    Auto,
    /// Sudo commands auto-approve.
    Sudo,
    /// Both classes auto-approve.
    All,
}

/// How wide one class-wide write grant is: nothing, `n` named artifacts, or
/// the whole class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scope {
    /// No grant of this kind.
    #[default]
    None,
    /// `n` named artifacts approved for the session.
    Named(u32),
    /// The whole class pre-approved (`*_all` flag or an all-covering grant).
    All,
}

/// The session's grant state as the bar reads it: a snapshot, not a
/// pre-formatted string, so the segment speaks the bar's glyph dialect and
/// degrades by tier. Built by `SessionApproval::summary()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GrantSummary {
    /// The command grants (daemon-host classes).
    pub cmds: CmdGrant,
    /// Script writes.
    pub scripts: Scope,
    /// Runbook writes.
    pub runbooks: Scope,
    /// File edits.
    pub files: Scope,
    /// Agent config writes.
    pub agents: Scope,
    /// Schedule writes: boolean, no per-name state.
    pub schedules: bool,
    /// Every delete kind: boolean, no per-name state.
    pub deletes: bool,
    /// Distinct remote hosts granted commands or sudo commands.
    pub hosts: u32,
}

impl GrantSummary {
    /// Whether this is the shipped default state (`Auto` cmds, nothing
    /// granted): the segment hides entirely.
    pub fn is_default(&self) -> bool {
        self.cmds == CmdGrant::Auto
            && self.scripts == Scope::None
            && self.runbooks == Scope::None
            && self.files == Scope::None
            && self.agents == Scope::None
            && !self.schedules
            && !self.deletes
            && self.hosts == 0
    }

    /// The number of active grant kinds the core counts: gated or sudo/All
    /// cmds, each non-`None` scope, each boolean, and hosts when granted.
    pub fn active_kinds(&self) -> u32 {
        grants_active_kinds(self)
    }
}

/// The grants segment: hidden in the default state (`Auto` cmds, nothing
/// else); otherwise the lead glyph coloured by the riskiest grant, the word
/// `grants` and the count of active kinds (tier 1, the word exempt), plus —
/// droppable — one part per active kind, glyph then word then count (tier
/// 3). Flush right, as the hint was.
pub(super) fn grants_parts(model: &StatusModel, g: &GlyphSet, palette: &Palette) -> Vec<Part> {
    let s = model.grants;
    if s.is_default() {
        return Vec::new();
    }
    let colour = grants_lead_colour(&s, palette);
    let kinds = s.active_kinds();
    let lead_style = Style::default().fg(colour);
    let core = part_two_form(
        lead_style,
        g.grants,
        kinds.to_string(),
        Some((format!("{} {kinds}", word::GRANTS), false)),
        RANK_GRANTS,
    );
    let detail = |glyph: &str, wide_word: String, compact_text: String| {
        part_two_form(
            gray(),
            glyph,
            compact_text,
            Some((wide_word, false)),
            RANK_GRANT_DETAIL,
        )
    };
    let mut parts = vec![core];
    if s.cmds == CmdGrant::Gated {
        parts.push(detail(g.grant_gated, word::GATED.into(), String::new()));
    }
    if s.cmds == CmdGrant::Sudo || s.cmds == CmdGrant::All {
        parts.push(detail(g.grant_sudo, word::SUDO.into(), String::new()));
    }
    for (scope, glyph, kind) in [
        (s.scripts, g.grant_scripts, word::SCRIPTS),
        (s.runbooks, g.grant_runbooks, word::RUNBOOKS),
        (s.files, g.grant_files, word::FILES),
    ] {
        if scope != Scope::None {
            match scope {
                Scope::All => parts.push(detail(
                    glyph,
                    format!("{kind} {}", word::ALL),
                    g.grant_all.into(),
                )),
                Scope::Named(n) => parts.push(detail(glyph, format!("{kind} {n}"), n.to_string())),
                Scope::None => {}
            }
        }
    }
    if s.schedules {
        parts.push(detail(
            g.grant_schedules,
            word::SCHEDULES.into(),
            String::new(),
        ));
    }
    if s.agents != Scope::None {
        match s.agents {
            Scope::All => parts.push(detail(
                g.grant_agents,
                format!("{} {}", word::AGENTS, word::ALL),
                g.grant_all.into(),
            )),
            Scope::Named(n) => parts.push(detail(
                g.grant_agents,
                format!("{} {n}", word::AGENTS),
                n.to_string(),
            )),
            Scope::None => {}
        }
    }
    if s.deletes {
        parts.push(detail(g.grant_deletes, word::DELETES.into(), String::new()));
    }
    if s.hosts > 0 {
        parts.push(detail(
            g.grant_hosts,
            format!(
                "{} {}",
                word::plural(s.hosts, word::HOST, word::HOSTS),
                s.hosts
            ),
            s.hosts.to_string(),
        ));
    }
    parts
}

/// The lead colour the grants segment speaks for `s`, over `palette`: red
/// when the riskiest grant is sudo/All cmds, deletes or a whole-class scope,
/// amber when only per-name scopes or hosts are held, green when the session
/// is gated and nothing else. Public for the colour tests.
pub fn grants_lead_colour(s: &GrantSummary, palette: &Palette) -> Color {
    let any_named = |sc: Scope| matches!(sc, Scope::Named(_));
    let red = s.cmds == CmdGrant::Sudo
        || s.cmds == CmdGrant::All
        || s.deletes
        || s.scripts == Scope::All
        || s.runbooks == Scope::All
        || s.files == Scope::All
        || s.agents == Scope::All;
    let amber = !red
        && (any_named(s.scripts)
            || any_named(s.runbooks)
            || any_named(s.files)
            || any_named(s.agents)
            || s.hosts > 0);
    if red {
        palette.red()
    } else if amber {
        palette.yellow()
    } else {
        palette.green()
    }
}

/// The number of active grant kinds the core counts: gated or sudo/All cmds,
/// each non-`None` scope, each boolean, and hosts when granted.
pub fn grants_active_kinds(s: &GrantSummary) -> u32 {
    (s.cmds != CmdGrant::Auto) as u32
        + (s.scripts != Scope::None) as u32
        + (s.runbooks != Scope::None) as u32
        + (s.files != Scope::None) as u32
        + (s.agents != Scope::None) as u32
        + s.schedules as u32
        + s.deletes as u32
        + (s.hosts > 0) as u32
}

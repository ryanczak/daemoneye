//! Whole-line snapshot tests for the status bar (M37 phase 12). Each test
//! renders `status_bar_line` at fixed widths over a fixed model — a fixed
//! `now`, a fixed alert `first_seen`, the `deepseek-v4.1-flash` name so the
//! phase-11 clip shows — and asserts the exact string. Derived from the
//! code after the phase's fixes and read against the Goal before pinning:
//! the words show once the wide line fits with its lowest ranks gone; when
//! the words drop, every part dropped to make room for them comes back.

use std::time::{Duration, Instant};

use super::*;
use crate::cli::palette::{ColorDepth, Palette};
use crate::ipc::AlertBadge;

fn palette() -> Palette {
    Palette::for_depth(ColorDepth::Truecolor)
}

/// The model the snapshots render: a fixed anchor unix second and a fixed
/// `now` one hour later, so the session age and the alert age are stable at
/// every run. `now` is derived from the anchor, not the wall clock.
fn fixture() -> (StatusModel, Instant) {
    let anchor = 1_700_000_000u64;
    // Take the wall clock once, then hold the pair: the unix second and the
    // monotonic instant measured at the same breath. `now` is one hour past
    // the anchor on the unix scale, and the monotonic offset matches, so
    // `now_unix(now)` returns the anchor + 3600 exactly.
    let wall = crate::shell::now_unix();
    let tick = Instant::now();
    let now = if anchor + 3600 >= wall {
        tick + Duration::from_secs(anchor + 3600 - wall)
    } else {
        tick - Duration::from_secs(wall - anchor - 3600)
    };
    let m = StatusModel {
        session_id: "0123456789abcdef".to_string(),
        grants: GrantSummary {
            cmds: CmdGrant::All,
            scripts: Scope::Named(2),
            runbooks: Scope::All,
            files: Scope::All,
            agents: Scope::Named(1),
            schedules: true,
            deletes: true,
            hosts: 1,
        },
        model: "deepseek-v4.1-flash".to_string(),
        turn: 12,
        prompt_tokens: 41_234,
        context_window: 200_000,
        previous_prompt_tokens: Some(38_100),
        last_output_tokens: 1_140,
        last_turn_duration: Some(Duration::from_secs(30)),
        turn_started_at: None,
        session_started_at: unix_instant(anchor - 3600, wall, tick),
        session_start_unix: anchor - 3600,
        cost_usd: 0.42,
        has_untracked: true,
        state: ChatState::Idle,
        alerts: Vec::new(),
        resolved_flash: None,
        resolve_flash_started_at: None,
        background: 2,
        ghosts: 1,
        tools_total: 7,
        shells: 3,
        session_name: Some("incident-42".to_string()),
        session_dirty: false,
        glyphs: &EMOJI,
    };
    (m, now)
}

/// A monotonic instant whose unix time is `unix`, derived from the same
/// wall-clock breath: `(tick, wall)` means unix `wall`.
fn unix_instant(unix: u64, wall: u64, tick: Instant) -> Instant {
    if unix >= wall {
        tick + Duration::from_secs(unix - wall)
    } else {
        tick - Duration::from_secs(wall - unix)
    }
}

/// The full model at the fixture's `now`: the alert fired exactly at the
/// `now` unix second, so its age is `0s`.
fn full_model() -> (StatusModel, Instant) {
    let anchor = 1_700_000_000u64;
    let (mut m, now) = fixture();
    m.alerts = vec![AlertBadge {
        fingerprint: "fp-disk".to_string(),
        name: "DiskFull".to_string(),
        severity: "warning".to_string(),
        first_seen_unix: anchor + 3600,
        last_seen_unix: anchor + 3600,
    }];
    (m, now)
}

/// The PE's setup: an info alert, no grants, an unnamed session.
fn pe_model() -> (StatusModel, Instant) {
    let (mut m, now) = full_model();
    m.grants = GrantSummary::default();
    m.session_name = None;
    m.alerts = vec![AlertBadge {
        fingerprint: "fp-heart".to_string(),
        name: "heartbeat".to_string(),
        severity: "info".to_string(),
        first_seen_unix: 1_700_000_000 + 3600,
        last_seen_unix: 1_700_000_000 + 3600,
    }];
    (m, now)
}

fn line_at(m: &StatusModel, now: Instant, width: u16) -> String {
    status_bar_line(m, now, &palette(), width)
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect()
}

#[test]
fn snapshot_emoji_full_at_160_140_120_80_40() {
    let (m, now) = full_model();
    // 160: the wide form fits once the grants detail and the velocities are
    // gone; the alert name and age survive, and every word shows.
    assert_eq!(
        line_at(&m, now, 160),
        "💤 idle · ⚠ warn 0s · 🌓 ctx 20% · 💰 ?0.42 · 🔄 turn 12 · ⏳ 2 bg 👻 1 ghost 🔧 7 tools 🐚 3 shells · 🦙 model: deepseek-v4 · 📌 incident-42        🔒 grants 8"
    );
    // 140: the wide form does not fit, so the words drop — and the detail,
    // the velocities, the name and the age all come back in compact form.
    assert_eq!(
        line_at(&m, now, 140),
        "💤 idle · ⚠1 DiskFull 0s · 🌓20% 📈3.1k · 💰?0.42 ⛽0.2/h · 🔄12 🐇38t/s · ⏳2 👻1 🔧7 🐚3 · 🦙 deepseek-v4 · 📌 incident-42             🔒8"
    );
    assert_eq!(
        line_at(&m, now, 120),
        "💤 idle · ⚠1 DiskFull 0s · 🌓20% · 💰?0.42 · 🔄12 🐇38t/s · ⏳2 👻1 🔧7 🐚3 · 🦙 deepseek-v4 · 📌 incident-42        🔒8"
    );
    assert_eq!(
        line_at(&m, now, 80),
        "💤 idle · ⚠1 · 🌓20% · 💰?0.42 · 🔄12 · ⏳2 👻1 🔧7 🐚3 · 🦙 deepseek-v4     🔒8"
    );
    assert_eq!(
        line_at(&m, now, 40),
        "💤 idle · ⚠1 · 🌓20% · 💰?0.42 · 🔄12"
    );
}

#[test]
fn snapshot_emoji_working_approval_and_down_at_120() {
    let (mut m, now) = full_model();
    m.state = ChatState::Working;
    m.turn_started_at = Some(now - Duration::from_secs(12));
    assert_eq!(
        line_at(&m, now, 120),
        "🧠 thinking 12s · ⚠1 DiskFull 0s · 🌓20% · 💰?0.42 · 🔄12 · ⏳2 👻1 🔧7 🐚3 · 🦙 deepseek-v4 · 📌 incident-42        🔒8"
    );
    m.state = ChatState::AwaitingApproval;
    m.turn_started_at = None;
    assert_eq!(
        line_at(&m, now, 120),
        "✋ approve? · ⚠1 DiskFull 0s · 🌓20% · 💰?0.42 · 🔄12 🐇38t/s · ⏳2 👻1 🔧7 🐚3 · 🦙 deepseek-v4 · 📌 incident-42    🔒8"
    );
    m.state = ChatState::DaemonDown;
    assert_eq!(
        line_at(&m, now, 120),
        "💀 daemon down · ⚠1 DiskFull 0s · 🌓20% · 💰?0.42 · 🔄12 🐇38t/s · ⏳2 👻1 🔧7 🐚3 · 🦙 deepseek-v4 · 📌 incident-42 🔒8"
    );
}

/// The PE's own setup (info alert, no grants, unnamed session): the words
/// show at 160 and at 120; the compact band starts below 120.
#[test]
fn snapshot_emoji_info_alert_no_grants_unnamed_at_160_120_and_80() {
    let (m, now) = pe_model();
    assert_eq!(
        line_at(&m, now, 160),
        "💤 idle · ⚠ info heartbeat 0s · 🌓 ctx 20% · 💰 ?0.42 · 🔄 turn 12 🐇 38 tok/s · ⏳ 2 bg 👻 1 ghost 🔧 7 tools 🐚 3 shells · 🦙 model: deepseek-v4"
    );
    assert_eq!(
        line_at(&m, now, 120),
        "💤 idle · ⚠1 heartbeat 0s · 🌓20% 📈3.1k · 💰?0.42 ⛽0.2/h · 🔄12 🐇38t/s · ⏳2 👻1 🔧7 🐚3 · 🦙 deepseek-v4"
    );
    assert_eq!(
        line_at(&m, now, 80),
        "💤 idle · ⚠1 0s · 🌓20% · 💰?0.42 · 🔄12 · ⏳2 👻1 🔧7 🐚3 · 🦙 deepseek-v4"
    );
}

#[test]
fn snapshot_symbols_full_at_160_and_80() {
    let (mut m, now) = full_model();
    m.glyphs = &SYMBOLS;
    assert_eq!(
        line_at(&m, now, 160),
        "○ idle · ● warn DiskFull 0s · ◕ ctx 20% · ?0.42 · ↻ turn 12 » 38 tok/s · ⧖ 2 bg ◆ 1 ghost ⚒ 7 tools ⌨ 3 shells · model: deepseek-v4 · ▣ incident-42  ⚡ grants 8"
    );
    assert_eq!(
        line_at(&m, now, 80),
        "○ idle · ●1 · ◕20% · ?0.42 · ↻12 · ⧖2 ◆1 ⚒7 ⌨3 · deepseek-v4 · ▣ incident-42 ⚡8"
    );
}

#[test]
fn snapshot_ascii_full_at_160_and_80() {
    let (mut m, now) = full_model();
    m.glyphs = &ASCII;
    assert_eq!(
        line_at(&m, now, 160),
        "- idle · ! warn DiskFull 0s · ctx20% · ?0.42 $0.2/h burn · turn 12 38 tok/s · 2 bg 1 ghost 7 tools 3 shells · model: deepseek-v4 · @ incident-42      A grants 8"
    );
    assert_eq!(
        line_at(&m, now, 80),
        "- idle · !1 · ctx20% · ?0.42 · t12 · bg2 gh1 tl7 sh3 · deepseek-v4            A8"
    );
}

/// At 160 the full emoji bar has lost the grants detail, the velocities and
/// the alert name to the wide form, and keeps every word: the words outlive
/// those ranks, and the alert age outlives them too.
#[test]
fn snapshot_words_survive_the_grants_detail_and_velocities() {
    let (m, now) = full_model();
    let text = line_at(&m, now, 160);
    for gone in ["🔑", "📜", "📈", "⛽", "🐇"] {
        assert!(
            !text.contains(gone),
            "the grants detail and the velocities are gone: {gone} in {text:?}"
        );
    }
    for word in [
        "idle", "warn", "ctx", "turn", "bg", "ghost", "tools", "shells", "model:", "grants",
    ] {
        assert!(text.contains(word), "the word survives: {word} in {text:?}");
    }
    assert!(
        text.contains("warn 0s"),
        "the alert age outlives the velocities: {text:?}"
    );
}

//! Tests for the chat status bar's model and line. All pure: `now` and the
//! instants are built by hand, nothing reads the clock. The tests assert
//! what a line contains or omits at a width, and its measured width — never
//! a full string of the bar.

use std::time::{Duration, Instant};

use unicode_width::UnicodeWidthStr;

use super::*;
use crate::cli::palette::{ColorDepth, Palette};

fn palette() -> Palette {
    Palette::for_depth(ColorDepth::Truecolor)
}

/// A full model: working turn under way, alerts firing, every counter up.
fn full_model() -> StatusModel {
    let now = Instant::now();
    let mut m = StatusModel {
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
        model: "claude-sonnet-4-6".to_string(),
        turn: 12,
        prompt_tokens: 41_234,
        context_window: 200_000,
        previous_prompt_tokens: Some(38_100),
        last_output_tokens: 1_140,
        last_turn_duration: Some(Duration::from_secs(30)),
        turn_started_at: None,
        session_started_at: now - Duration::from_secs(3600),
        session_start_unix: 1_700_000_000 - 3600,
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
    m.alerts = vec![AlertBadge {
        fingerprint: "fp-disk".to_string(),
        name: "DiskFull".to_string(),
        severity: "warning".to_string(),
        first_seen_unix: 1_700_000_000,
        last_seen_unix: 1_700_000_000,
    }];
    m
}

fn line_at(m: &StatusModel, now: Instant, width: u16) -> String {
    status_bar_line(m, now, &palette(), width)
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect()
}

fn line_width(m: &StatusModel, now: Instant, width: u16) -> usize {
    status_bar_line(m, now, &palette(), width).width()
}

// ── Glyph hygiene ────────────────────────────────────────────────────────────

#[test]
fn glyph_sets_contain_no_variation_selectors() {
    let sets = [EMOJI, SYMBOLS, ASCII];
    for set in sets {
        for field in [
            set.idle,
            set.working,
            set.awaiting_approval,
            set.daemon_down,
            set.reconnecting,
            set.elapsed,
            set.alert_critical,
            set.alert_warning,
            set.alert_info,
            set.alert_unknown,
            set.alert_resolved,
            set.context_empty,
            set.context_1,
            set.context_2,
            set.context_3,
            set.context_full,
            set.context_growth,
            set.cost,
            set.cost_untracked,
            set.cost_burn,
            set.turn,
            set.throughput_fast,
            set.throughput_slow,
            set.background,
            set.ghosts,
            set.tools,
            set.shells,
            set.vendor_anthropic,
            set.vendor_openai,
            set.vendor_gemini,
            set.vendor_local,
            set.session,
            set.session_dirty,
            set.grants,
            set.grant_gated,
            set.grant_sudo,
            set.grant_scripts,
            set.grant_runbooks,
            set.grant_files,
            set.grant_schedules,
            set.grant_agents,
            set.grant_deletes,
            set.grant_hosts,
            set.grant_all,
        ] {
            assert!(
                !field.contains('\u{fe0f}') && !field.contains('\u{fe0e}'),
                "a glyph in the {set:?} set carries a variation selector: {field:?}"
            );
        }
    }
}

#[test]
fn glyph_set_by_name_resolves_the_three_dialects_only() {
    assert!(std::ptr::eq(glyph_set_by_name("emoji").unwrap(), &EMOJI));
    assert!(std::ptr::eq(
        glyph_set_by_name("symbols").unwrap(),
        &SYMBOLS
    ));
    assert!(std::ptr::eq(glyph_set_by_name("ascii").unwrap(), &ASCII));
    assert!(glyph_set_by_name("Emoji").is_none(), "case-sensitive");
    assert!(glyph_set_by_name("").is_none());
}

// ── Fitting ──────────────────────────────────────────────────────────────────

#[test]
fn status_line_fits_its_width_at_every_width_from_20_to_160() {
    let m = full_model();
    let now = Instant::now();
    for set in [&EMOJI, &SYMBOLS, &ASCII] {
        let mut m = m.clone();
        m.glyphs = set;
        for width in 20..=160 {
            let text = line_at(&m, now, width);
            let measured = line_width(&m, now, width);
            assert!(
                measured <= width as usize,
                "the {set:?} line at width {width} measures {measured}: {text:?}"
            );
        }
    }
}

// ── Slots ────────────────────────────────────────────────────────────────────

#[test]
fn status_line_hides_zero_counters_and_shows_nonzero_ones() {
    let now = Instant::now();
    let mut m = full_model();
    let text = line_at(&m, now, 160);
    for slot in ["⏳2", "👻1", "🛠7", "🐚3"] {
        assert!(text.contains(slot), "a live counter shows: {text:?}");
    }

    m.background = 0;
    m.ghosts = 0;
    m.tools_total = 0;
    m.shells = 0;
    let text = line_at(&m, now, 160);
    for slot in ["⏳", "👻", "🛠", "🐚"] {
        assert!(
            !text.contains(slot),
            "a zero counter is hidden entirely: {slot} in {text:?}"
        );
    }
}

#[test]
fn status_line_leads_with_the_worst_alert_and_counts_all() {
    // `now` is the session start, so the ages tick straight off the stamps.
    let now = Instant::now();
    let mut m = full_model();
    m.session_started_at = now;
    m.session_start_unix = 1_700_000_000;
    m.alerts = vec![
        AlertBadge {
            fingerprint: "fp-warn".to_string(),
            name: "DiskFull".to_string(),
            severity: "warning".to_string(),
            first_seen_unix: 1_699_999_940,
            last_seen_unix: 1_699_999_940,
        },
        AlertBadge {
            fingerprint: "fp-crit".to_string(),
            name: "DaemonGone".to_string(),
            severity: "critical".to_string(),
            first_seen_unix: 1_699_999_900,
            last_seen_unix: 1_699_999_900,
        },
        AlertBadge {
            fingerprint: "fp-info".to_string(),
            name: "Heartbeat".to_string(),
            severity: "info".to_string(),
            first_seen_unix: 1_699_999_950,
            last_seen_unix: 1_699_999_950,
        },
    ];
    let text = line_at(&m, now, 160);
    assert!(
        text.contains("🔥3"),
        "the worst severity's glyph and every count: {text:?}"
    );
    assert!(
        text.contains("DaemonGone"),
        "the name is the newest alert of the worst severity: {text:?}"
    );
    assert!(
        !text.contains("Heartbeat") && !text.contains("DiskFull"),
        "only the worst severity's newest alert is named: {text:?}"
    );
    // The worst-severity newest alert (DaemonGone, seen 100 s ago) shows a
    // minute age; 60 s rounds down to `1m`.
    assert!(
        text.contains("1m"),
        "the age ticks from last_seen: {text:?}"
    );

    // The leading slot comes before the context slot: alerts lead the line
    // after the state.
    let lead = text.find("🔥3").expect("the alert glyph is on the line");
    let ctx = text
        .find("🌑")
        .or_else(|| text.find("🌒"))
        .or_else(|| text.find("🌓"))
        .or_else(|| text.find("🌔"))
        .or_else(|| text.find("🌕"))
        .expect("the context glyph is on the line");
    assert!(lead < ctx, "alerts sit left of context: {text:?}");
}

#[test]
fn status_line_shows_the_resolve_flash_for_ten_seconds() {
    let now = Instant::now();
    let mut m = full_model();
    m.alerts = vec![AlertBadge {
        fingerprint: "fp-warn".to_string(),
        name: "DiskFull".to_string(),
        severity: "warning".to_string(),
        first_seen_unix: m.session_start_unix,
        last_seen_unix: m.session_start_unix,
    }];

    // The next frame drops the fingerprint: the flash starts.
    let t0 = now;
    m.set_alerts(Vec::new(), t0);
    assert!(m.resolve_flash_live(t0 + Duration::from_secs(9)));
    assert!(!m.resolve_flash_live(t0 + Duration::from_secs(11)));

    let at_9 = line_at(&m, t0 + Duration::from_secs(9), 160);
    assert!(
        at_9.contains("🧯"),
        "the resolved glyph shows during the flash: {at_9:?}"
    );
    assert!(
        at_9.contains("DiskFull"),
        "the resolved alert's name shows during the flash: {at_9:?}"
    );

    let at_11 = line_at(&m, t0 + Duration::from_secs(11), 160);
    assert!(
        !at_11.contains("🧯") && !at_11.contains("DiskFull"),
        "the flash is gone after ten seconds: {at_11:?}"
    );
}

#[test]
fn status_line_drops_tier_three_before_tier_two_before_the_grants_core() {
    let now = Instant::now();
    let mut m = full_model();
    m.turn_started_at = None;
    m.state = ChatState::Idle;

    // Tier 3 names the velocities, the alert name and age, the session, the
    // model and the grants detail. Tier 2 names the counters. Find the
    // narrowest width where every tier-3 item still shows, then step down
    // one column at a time: the first thing to go must be a tier-3 item.
    let wide = line_at(&m, now, 160);
    for item in [
        "📈",
        "⛽",
        "🐇",
        "DiskFull",
        "incident-42",
        "sonnet-4-6",
        "🔑",
        "📜",
        "📖",
        "📄",
        "🤖",
        "📅",
        "🗑",
        "🌐",
    ] {
        assert!(
            wide.contains(item),
            "tier 3 shows at 160: {item} in {wide:?}"
        );
    }
    for item in ["⏳2", "👻1", "🛠7", "🐚3"] {
        assert!(
            wide.contains(item),
            "tier 2 shows at 160: {item} in {wide:?}"
        );
    }
    assert!(
        wide.contains("⚡8"),
        "the grants core shows at 160: {wide:?}"
    );

    // Narrow until one tier-3 item drops; the tier-2 items must all survive.
    let mut width = 159usize;
    loop {
        let text = line_at(&m, now, width as u16);
        let missing: Vec<&str> = [
            "📈",
            "⛽",
            "🐇",
            "DiskFull",
            "incident-42",
            "sonnet-4-6",
            "🔑",
            "📜",
            "📖",
            "📄",
            "🤖",
            "📅",
            "🗑",
            "🌐",
        ]
        .into_iter()
        .filter(|i| !text.contains(i))
        .collect();
        if !missing.is_empty() {
            for item in ["⏳2", "👻1", "🛠7", "🐚3"] {
                assert!(
                    text.contains(item),
                    "tier 2 outlives the first tier-3 drop ({missing:?}): {item} gone from {text:?}"
                );
            }
            assert!(
                text.contains("⚡8"),
                "the grants core outlives the first tier-3 drop: {text:?}"
            );
            break;
        }
        width -= 1;
        assert!(width > 20, "no tier-3 item ever dropped: {text:?}");
    }

    // Narrow further until a tier-2 counter drops; the grants core must still
    // show.
    let mut width = width - 1;
    loop {
        let text = line_at(&m, now, width as u16);
        let missing: Vec<&str> = ["⏳2", "👻1", "🛠7", "🐚3"]
            .into_iter()
            .filter(|i| !text.contains(i))
            .collect();
        if !missing.is_empty() {
            assert!(
                text.contains("⚡8"),
                "the grants core drops after tier 2, not before: {text:?}"
            );
            break;
        }
        width -= 1;
        assert!(width > 10, "no tier-2 item ever dropped: {text:?}");
    }

    // Narrow further until the grants core drops; tier 1 must still show.
    let mut width = width - 1;
    loop {
        let text = line_at(&m, now, width as u16);
        if !text.contains("⚡8") {
            assert!(text.contains("💤"), "state is tier 1: {text:?}");
            assert!(text.contains("⚠1"), "alerts are tier 1: {text:?}");
            assert!(text.contains("🌓20%"), "context is tier 1: {text:?}");
            assert!(text.contains("💸0.42"), "cost is tier 1: {text:?}");
            assert!(text.contains("↻12"), "turn is tier 1: {text:?}");
            break;
        }
        width -= 1;
        assert!(width > 5, "the grants core never dropped: {text:?}");
    }
}

#[test]
fn status_line_context_fill_steps_and_budget_colours() {
    let now = Instant::now();
    let mut m = full_model();
    m.state = ChatState::Idle;
    m.alerts.clear();
    m.cost_usd = 0.0;

    // The five fill steps at a comfortable width, glyph only — the percentage
    // text is the same shape at every step.
    let mut fill_at = |tokens: u32| {
        m.prompt_tokens = tokens;
        line_at(&m, now, 80)
    };
    assert!(
        fill_at(10_000).contains("🌒"),
        "< 20 %: {}",
        fill_at(10_000)
    );
    assert!(
        fill_at(80_000).contains("🌓"),
        "< 45 %: {}",
        fill_at(80_000)
    );
    assert!(
        fill_at(130_000).contains("🌔"),
        "< 70 %: {}",
        fill_at(130_000)
    );
    assert!(
        fill_at(180_000).contains("🌕"),
        "≥ 90 %: {}",
        fill_at(180_000)
    );

    // The budget colours: the span holding the percentage is amber from 75 %
    // and red from 90 %, gray below.
    let mut pct_span_color = |tokens: u32| {
        m.prompt_tokens = tokens;
        let line = status_bar_line(&m, now, &palette(), 80);
        line.spans
            .iter()
            .find(|s| s.content.ends_with('%'))
            .map(|s| s.style.fg)
            .expect("a span holds the percentage")
    };
    let p = palette();
    assert_eq!(
        pct_span_color(100_000),
        Some(Color::Gray),
        "50 % stays gray"
    );
    assert_eq!(
        pct_span_color(150_000),
        Some(p.yellow()),
        "75 % turns amber"
    );
    assert_eq!(
        pct_span_color(178_000),
        Some(p.yellow()),
        "89 % stays amber"
    );
    assert_eq!(pct_span_color(180_000), Some(p.red()), "90 % turns red");

    // With no usage yet: the empty glyph and an em dash.
    m.prompt_tokens = 0;
    m.previous_prompt_tokens = None;
    let text = line_at(&m, now, 80);
    assert!(
        text.contains("🌑—"),
        "no usage shows the empty slot: {text:?}"
    );

    // Growth since the previous turn's prompt, amber at ≥ 10 % of the window.
    m.prompt_tokens = 41_234;
    m.previous_prompt_tokens = Some(38_100);
    let text = line_at(&m, now, 160);
    assert!(text.contains("📈"), "growth shows when room: {text:?}");
    assert!(
        text.contains("3.1k"),
        "the growth is compact tokens: {text:?}"
    );
    m.previous_prompt_tokens = Some(21_234);
    let amber_line = status_bar_line(&m, now, &palette(), 160);
    let growth_span = amber_line
        .spans
        .iter()
        .find(|s| s.content.contains("📈"))
        .expect("the growth span");
    assert_eq!(
        growth_span.style.fg,
        Some(p.yellow()),
        "≥ 10 % growth is amber: {growth_span:?}"
    );
}

#[test]
fn status_line_working_state_shows_elapsed_seconds() {
    let now = Instant::now();
    let mut m = full_model();
    m.state = ChatState::Working;
    m.turn_started_at = Some(now - Duration::from_secs(12));
    let text = line_at(&m, now, 160);
    assert!(
        text.contains("🧠"),
        "the working state's glyph leads: {text:?}"
    );
    assert!(
        text.contains("⏱12s"),
        "the elapsed mark and seconds since the turn started: {text:?}"
    );
    assert!(!text.contains("daemon down"), "not down: {text:?}");

    // The working slot drops the elapsed seconds under the ascii dialect's
    // bare spelling: the seconds still show.
    m.glyphs = &ASCII;
    let text = line_at(&m, now, 160);
    assert!(
        text.contains("12s"),
        "ascii spells the seconds bare: {text:?}"
    );
}

#[test]
fn status_line_daemon_down_shows_the_down_glyph_and_text() {
    let now = Instant::now();
    let mut m = full_model();
    m.state = ChatState::DaemonDown;
    let text = line_at(&m, now, 160);
    assert!(text.contains("💀"), "the down glyph: {text:?}");
    assert!(text.contains("daemon down"), "the words say why: {text:?}");
    assert!(
        !text.contains("🧠") && !text.contains("💤"),
        "the running/idle glyphs are gone: {text:?}"
    );

    m.glyphs = &ASCII;
    let text = line_at(&m, now, 80);
    assert!(text.contains("X"), "the ascii down glyph: {text:?}");
}

#[test]
fn status_line_omits_an_unnamed_session_and_shows_a_saved_one_dirty() {
    let now = Instant::now();
    let mut m = full_model();
    m.session_name = None;
    let text = line_at(&m, now, 160);
    assert!(!text.contains("📌"), "unnamed: no session slot: {text:?}");

    m.session_name = Some("incident-42".to_string());
    m.session_dirty = false;
    let text = line_at(&m, now, 160);
    assert!(
        text.contains("📌 incident-42"),
        "a saved name shows: {text:?}"
    );

    m.session_dirty = true;
    let text = line_at(&m, now, 160);
    assert!(
        text.contains("📌✎ incident-42"),
        "the dirty mark sits between glyph and name: {text:?}"
    );
}

#[test]
fn status_line_cost_shows_spend_untracked_and_burn() {
    let now = Instant::now();
    let mut m = full_model();
    m.turn_started_at = None;
    m.cost_usd = 0.42;
    m.has_untracked = true;
    // The session has run for an hour: burn 0.42/h.
    m.session_started_at = now - Duration::from_secs(3600);
    let text = line_at(&m, now, 160);
    assert!(
        text.contains("💸0.42"),
        "the spend to two decimals: {text:?}"
    );
    assert!(text.contains("❔"), "the unknown-pricing mark: {text:?}");
    assert!(text.contains("⛽"), "the burn glyph: {text:?}");
    assert!(text.contains("0.4/h"), "the burn rate: {text:?}");

    m.has_untracked = false;
    let text = line_at(&m, now, 160);
    assert!(
        !text.contains("❔"),
        "no unknown mark without untracked: {text:?}"
    );
}

#[test]
fn status_line_throughput_says_fast_or_slow() {
    let now = Instant::now();
    let mut m = full_model();
    m.turn_started_at = None;
    m.last_output_tokens = 1_140;
    m.last_turn_duration = Some(Duration::from_secs(30));
    let text = line_at(&m, now, 160);
    assert!(text.contains("🐇38t/s"), "30 t/s or more is fast: {text:?}");

    m.last_turn_duration = Some(Duration::from_secs(60));
    let text = line_at(&m, now, 160);
    assert!(text.contains("🐢19t/s"), "below 30 t/s is slow: {text:?}");
}

// ── Model naming ─────────────────────────────────────────────────────────────

#[test]
fn short_model_name_strips_vendor_and_date_and_picks_the_mark() {
    assert_eq!(short_model_name("claude-sonnet-4-6"), "sonnet-4-6");
    assert_eq!(short_model_name("claude-haiku-4-5-20251001"), "haiku-4-5");
    assert_eq!(short_model_name("gpt-4o"), "4o");
    assert_eq!(short_model_name("gemini-2.5-pro"), "2.5-pro");
    assert_eq!(short_model_name("openai/gpt-4o-mini"), "4o-mini");
    assert_eq!(short_model_name("ollama/llama3:8b"), "llama3:8b");
    assert_eq!(
        short_model_name("claude-sonnet-4-6").chars().count(),
        short_model_name("claude-sonnet-4-6").chars().count()
    );

    let m = |model: &str| vendor_mark(model, &EMOJI);
    assert_eq!(m("claude-sonnet-4-6"), "⟡");
    assert_eq!(m("gpt-4o"), "◎");
    assert_eq!(m("gemini-2.5-pro"), "♊");
    assert_eq!(m("ollama/llama3:8b"), "🦙");
    // SYMBOLS has no local mark; ASCII has no mark at all.
    assert_eq!(vendor_mark("ollama/llama3", &SYMBOLS), "");
    assert_eq!(vendor_mark("claude-sonnet-4-6", &ASCII), "");
}

// ── The locale rule ──────────────────────────────────────────────────────────

#[test]
fn glyphs_fall_back_to_ascii_without_a_utf8_locale() {
    assert!(locale_is_utf8(Some("en_US.UTF-8"), None, None));
    assert!(locale_is_utf8(None, Some("C.UTF-8"), None));
    assert!(locale_is_utf8(None, None, Some("POSIX.utf8")));
    assert!(locale_is_utf8(Some(""), None, Some("en_US.utf-8")));
    assert!(!locale_is_utf8(Some("en_US"), None, None));
    assert!(!locale_is_utf8(Some("C"), Some("C"), None));
    assert!(!locale_is_utf8(None, None, None));
    assert!(!locale_is_utf8(Some("en_US.ISO-8859-1"), None, None));

    let configured = &EMOJI;
    assert!(std::ptr::eq(glyph_set_for_locale(configured, true), &EMOJI));
    assert!(std::ptr::eq(
        glyph_set_for_locale(configured, false),
        &ASCII
    ));
    assert!(std::ptr::eq(glyph_set_for_locale(&ASCII, true), &ASCII));
}

// ── Ticking ──────────────────────────────────────────────────────────────────

#[test]
fn is_time_based_while_working_alerting_or_flashing() {
    let now = Instant::now();
    let mut m = full_model();
    m.alerts.clear();
    m.resolved_flash = None;
    m.resolve_flash_started_at = None;
    m.state = ChatState::Idle;
    assert!(!m.is_time_based(now), "a quiet bar needs no timer");
    m.state = ChatState::Working;
    assert!(m.is_time_based(now), "a running turn ticks");
    m.state = ChatState::Idle;
    m.alerts = vec![AlertBadge {
        fingerprint: "fp".to_string(),
        name: "a".to_string(),
        severity: "info".to_string(),
        first_seen_unix: 0,
        last_seen_unix: 0,
    }];
    assert!(m.is_time_based(now), "a firing alert ages, so it ticks");
    m.alerts.clear();
    m.resolved_flash = Some(AlertBadge {
        fingerprint: "fp".to_string(),
        name: "a".to_string(),
        severity: "info".to_string(),
        first_seen_unix: 0,
        last_seen_unix: 0,
    });
    m.resolve_flash_started_at = Some(now);
    assert!(m.is_time_based(now), "a live flash ticks");
    m.resolve_flash_started_at = Some(now - RESOLVE_FLASH_SECS);
    assert!(!m.is_time_based(now), "an expired flash stops the timer");
}

#[test]
fn set_alerts_keeps_a_flash_only_for_a_real_drop() {
    let now = Instant::now();
    let mut m = StatusModel::default();
    let alert = |fp: &str| AlertBadge {
        fingerprint: fp.to_string(),
        name: "DiskFull".to_string(),
        severity: "warning".to_string(),
        first_seen_unix: 0,
        last_seen_unix: 0,
    };
    m.set_alerts(vec![alert("fp-a")], now);
    assert!(
        m.resolved_flash.is_none(),
        "a first snapshot flashes nothing"
    );

    // A frame that removes nothing leaves an ongoing flash alone.
    m.set_alerts(vec![alert("fp-a"), alert("fp-b")], now);
    m.resolved_flash = Some(alert("fp-old"));
    m.resolve_flash_started_at = Some(now);
    m.set_alerts(vec![alert("fp-a"), alert("fp-b"), alert("fp-c")], now);
    assert_eq!(
        m.resolved_flash.as_ref().map(|a| a.fingerprint.as_str()),
        Some("fp-old"),
        "an additive frame does not restart the flash"
    );

    // A frame that drops one restarts it with the newest removed alert.
    let t1 = now + Duration::from_secs(3);
    m.set_alerts(vec![alert("fp-a")], t1);
    assert_eq!(
        m.resolved_flash.as_ref().map(|a| a.fingerprint.as_str()),
        Some("fp-b"),
        "the newest removed alert becomes the flash"
    );
    assert_eq!(m.resolve_flash_started_at, Some(t1));
}

// ── Width sanity for the separator rule ──────────────────────────────────────

#[test]
fn narrow_panes_use_single_space_separators() {
    let now = Instant::now();
    let m = full_model();
    let wide = line_at(&m, now, 40);
    let narrow = line_at(&m, now, 30);
    // Below 36 columns no ` · ` separator survives.
    assert!(
        !narrow.contains(" · "),
        "below 36 columns the separator is a single space: {narrow:?}"
    );
    let _ = wide;
}

#[test]
fn the_line_is_never_empty_on_a_wide_pane() {
    let now = Instant::now();
    let m = full_model();
    let text = line_at(&m, now, 160);
    assert!(UnicodeWidthStr::width(text.as_str()) > 10, "{text:?}");
}

#[test]
fn compact_tokens_scales() {
    assert_eq!(compact_tokens(950), "950");
    assert_eq!(compact_tokens(4_321), "4.3k");
    assert_eq!(compact_tokens(41_234), "41k");
    assert_eq!(compact_tokens(1_048_576), "1.0M");
}

// ── The emitter's state transitions (bug-02a-1) ──────────────────────────────
//
// The bar's state slot follows the OSC 7501 emitter: the loop sets `state`
// beside each emitter call. These tests pin the model methods behind those
// calls — the same functions production goes through — so removing a
// transition in the loop fails here.

#[test]
fn set_daemon_up_false_leaves_daemon_down_from_idle() {
    let mut m = StatusModel::default();
    assert_eq!(m.state, ChatState::Idle, "the model starts idle");
    m.set_daemon_up(false);
    assert_eq!(
        m.state,
        ChatState::DaemonDown,
        "a failed ask demotes an idle-looking state to DaemonDown"
    );
    m.set_daemon_up(true);
    assert_eq!(
        m.state,
        ChatState::Idle,
        "the next successful ask clears DaemonDown"
    );
}

#[test]
fn set_daemon_up_false_never_overrides_a_running_turn() {
    let now = Instant::now();
    let mut m = full_model();
    m.state = ChatState::Working;
    m.turn_started_at = Some(now - Duration::from_secs(3));
    m.set_daemon_up(false);
    assert_eq!(
        m.state,
        ChatState::Working,
        "a liveness report never overrides a running turn"
    );
    assert_eq!(
        m.turn_started_at,
        Some(now - Duration::from_secs(3)),
        "the turn start is untouched"
    );
}

#[test]
fn set_daemon_up_true_clears_reconnecting_to_idle() {
    let mut m = StatusModel {
        state: ChatState::Reconnecting,
        ..Default::default()
    };
    m.set_daemon_up(true);
    assert_eq!(
        m.state,
        ChatState::Idle,
        "liveness back clears Reconnecting to Idle"
    );
}

#[test]
fn the_prompt_reply_returns_the_bar_to_working_with_its_clock() {
    let start = Instant::now();
    let mut m = StatusModel::default();
    m.begin_turn(start);
    m.prompt_opened();
    assert_eq!(m.state, ChatState::AwaitingApproval);
    m.prompt_answered();
    assert_eq!(m.state, ChatState::Working, "the reply is back to working");
    assert_eq!(m.turn_started_at, Some(start), "the turn clock is kept");
    let line = line_at(&m, start + Duration::from_secs(4), 160);
    assert!(line.contains('4'), "the elapsed seconds show again: {line}");
}

#[test]
fn a_failed_turn_ends_daemon_down_and_the_next_success_clears_it() {
    let start = Instant::now();
    let mut m = StatusModel::default();
    m.begin_turn(start);
    m.end_turn(start + Duration::from_secs(2), false);
    assert_eq!(
        m.state,
        ChatState::DaemonDown,
        "a failed ask ends daemon-down"
    );
    assert_eq!(m.turn_started_at, None);
    assert_eq!(m.last_turn_duration, Some(Duration::from_secs(2)));
    m.begin_turn(start + Duration::from_secs(5));
    m.end_turn(start + Duration::from_secs(6), true);
    assert_eq!(m.state, ChatState::Idle, "a successful ask clears it");
}

#[test]
fn begin_turn_marks_a_named_session_dirty_and_keeps_the_growth_baseline() {
    let mut m = StatusModel {
        prompt_tokens: 1200,
        ..Default::default()
    };
    m.begin_turn(Instant::now());
    assert!(!m.session_dirty, "an unnamed session is never dirty");
    assert_eq!(m.previous_prompt_tokens, Some(1200));
    m.session_name = Some("ops".to_string());
    m.begin_turn(Instant::now());
    assert!(m.session_dirty, "a named session goes dirty on a turn");
}

#[test]
fn presence_text_reconnecting_then_reconnected_returns_to_idle() {
    let mut m = StatusModel::default();
    m.presence_text("daemon connection lost — reconnecting…");
    assert_eq!(m.state, ChatState::Reconnecting);
    m.presence_text("ALERT something else");
    assert_eq!(m.state, ChatState::Reconnecting, "other text leaves it");
    m.presence_text("reconnected");
    assert_eq!(m.state, ChatState::Idle, "the reconnect clears it");

    m.begin_turn(Instant::now());
    m.presence_text("reconnected");
    assert_eq!(
        m.state,
        ChatState::Working,
        "a running turn is not overridden"
    );
}

// ── Session counters (M37 phase 02b) ─────────────────────────────────────────

#[test]
fn status_model_applies_session_counters() {
    let now = Instant::now();
    let mut m = full_model();

    // A nonzero frame shows all three counters.
    m.set_session_counters(2, 1, 3);
    let text = line_at(&m, now, 160);
    for slot in ["⏳2", "👻1", "🐚3"] {
        assert!(
            text.contains(slot),
            "a live counter shows: {slot} in {text:?}"
        );
    }

    // A zero frame hides them entirely.
    m.set_session_counters(0, 0, 0);
    let text = line_at(&m, now, 160);
    for slot in ["⏳", "👻", "🐚"] {
        assert!(
            !text.contains(slot),
            "a zero counter is hidden entirely: {slot} in {text:?}"
        );
    }

    // A later nonzero frame shows them again.
    m.set_session_counters(1, 0, 2);
    let text = line_at(&m, now, 160);
    assert!(text.contains("⏳1"), "background shows again: {text:?}");
    assert!(
        !text.contains("👻"),
        "a zero ghost slot stays hidden: {text:?}"
    );
    assert!(text.contains("🐚2"), "shells show again: {text:?}");
}

// ── Grants segment (M37 02c) ─────────────────────────────────────────────────

#[test]
fn grants_hidden_in_the_default_state() {
    let now = Instant::now();
    let mut m = full_model();
    m.turn_started_at = None;
    m.state = ChatState::Idle;
    m.grants = GrantSummary::default();

    let text = line_at(&m, now, 160);
    for mark in ["⚡", "🔒", "🔑", "📜", "📖", "📄", "📅", "🤖", "🗑", "🌐"] {
        assert!(
            !text.contains(mark),
            "the default state hides the segment: {mark} in {text:?}"
        );
    }

    // Any one non-default field brings the segment back.
    m.grants.deletes = true;
    let text = line_at(&m, now, 160);
    assert!(
        text.contains("⚡1"),
        "deletes alone shows the core: {text:?}"
    );
    assert!(text.contains("🗑"), "the deletes detail shows: {text:?}");
}

#[test]
fn grants_lead_colour_follows_the_riskiest_grant() {
    let now = Instant::now();
    let p = palette();

    // Gated commands and nothing else: green.
    let gated = GrantSummary {
        cmds: CmdGrant::Gated,
        ..GrantSummary::default()
    };
    assert_eq!(
        grants_lead_colour(&gated, &p),
        p.green(),
        "gated-only is green"
    );

    // One named script: amber.
    let named = GrantSummary {
        cmds: CmdGrant::Auto,
        scripts: Scope::Named(1),
        ..GrantSummary::default()
    };
    assert_eq!(
        grants_lead_colour(&named, &p),
        p.yellow(),
        "a named script is amber"
    );

    // Files all: red.
    let files_all = GrantSummary {
        cmds: CmdGrant::Auto,
        files: Scope::All,
        ..GrantSummary::default()
    };
    assert_eq!(
        grants_lead_colour(&files_all, &p),
        p.red(),
        "files all is red"
    );

    // Deletes alone: red.
    let deletes = GrantSummary {
        deletes: true,
        ..GrantSummary::default()
    };
    assert_eq!(grants_lead_colour(&deletes, &p), p.red(), "deletes is red");

    // Sudo alone: red.
    let sudo = GrantSummary {
        cmds: CmdGrant::Sudo,
        ..GrantSummary::default()
    };
    assert_eq!(grants_lead_colour(&sudo, &p), p.red(), "sudo is red");

    // Hosts only: amber.
    let hosts = GrantSummary {
        hosts: 2,
        ..GrantSummary::default()
    };
    assert_eq!(
        grants_lead_colour(&hosts, &p),
        p.yellow(),
        "hosts only is amber"
    );

    // And the line the production path draws carries the same colour: the
    // lead span's style is the risk colour, the rest of the line stays gray.
    let mut m = full_model();
    m.turn_started_at = None;
    m.state = ChatState::Idle;
    m.alerts.clear();
    m.session_name = None;
    m.background = 0;
    m.ghosts = 0;
    m.tools_total = 0;
    m.shells = 0;
    m.grants = gated;
    let line = status_bar_line(&m, now, &p, 120);
    let lead = line
        .spans
        .iter()
        .find(|s| s.content.contains('⚡'))
        .expect("the lead glyph is in the line");
    assert_eq!(
        lead.style.fg,
        Some(p.green()),
        "the drawn lead is green: {lead:?}"
    );
}

#[test]
fn grants_count_active_kinds_and_show_detail_when_room() {
    let now = Instant::now();
    let mut m = full_model();
    m.turn_started_at = None;
    m.state = ChatState::Idle;
    m.alerts.clear();
    m.session_name = None;
    m.background = 0;
    m.ghosts = 0;
    m.tools_total = 0;
    m.shells = 0;
    m.grants = GrantSummary {
        cmds: CmdGrant::Sudo,
        scripts: Scope::Named(2),
        files: Scope::All,
        deletes: true,
        ..GrantSummary::default()
    };

    // Core: the lead glyph and the count of active kinds (sudo, scripts,
    // files all, deletes = 4).
    let text = line_at(&m, now, 160);
    assert!(text.contains("⚡4"), "four active kinds: {text:?}");

    // Detail at 160: each kind glyph, `∗` for the All scope, in the table's
    // order and after the lead.
    let lead_at = text.find('⚡').expect("the lead glyph shows");
    let mut last = lead_at;
    for part in ["🔑", "📜2", "📄∗", "🗑"] {
        let at = text
            .find(part)
            .unwrap_or_else(|| panic!("detail part {part} shows at 160: {text:?}"));
        assert!(at > last, "detail follows in order: {part} in {text:?}");
        last = at;
    }
}

#[test]
fn grants_detail_drops_before_the_core() {
    let now = Instant::now();
    let mut m = full_model();
    m.turn_started_at = None;
    m.state = ChatState::Idle;
    m.grants = GrantSummary {
        cmds: CmdGrant::Sudo,
        scripts: Scope::Named(2),
        files: Scope::All,
        deletes: true,
        ..GrantSummary::default()
    };

    // Narrow until the last detail part is gone: the core must still show.
    let mut width = 160usize;
    loop {
        let text = line_at(&m, now, width as u16);
        if !text.contains('🔑') {
            for part in ["📜", "📄", "🗑"] {
                assert!(
                    !text.contains(part),
                    "detail drops as one tier: {part} still in {text:?}"
                );
            }
            assert!(
                text.contains("⚡4"),
                "the core outlives the detail: {text:?}"
            );
            break;
        }
        width -= 1;
        assert!(width > 20, "no detail part ever dropped: {text:?}");
    }

    // Narrow further until the core drops. Tier 1 must survive it: the core
    // is the last slot below tier 1.
    let mut width = width - 1;
    loop {
        let text = line_at(&m, now, width as u16);
        if !text.contains("⚡4") {
            assert!(text.contains("↻12"), "tier 1 outlives the core: {text:?}");
            break;
        }
        width -= 1;
        assert!(width > 5, "the grants core never dropped: {text:?}");
    }
}

#[test]
fn grants_speak_each_dialect() {
    let now = Instant::now();
    let mut m = full_model();
    m.turn_started_at = None;
    m.state = ChatState::Idle;
    m.alerts.clear();
    m.session_name = None;
    m.background = 0;
    m.ghosts = 0;
    m.tools_total = 0;
    m.shells = 0;
    m.grants = GrantSummary {
        cmds: CmdGrant::Gated,
        ..GrantSummary::default()
    };

    m.glyphs = &ASCII;
    let text = line_at(&m, now, 120);
    assert!(
        !text.contains('⚡'),
        "ascii never speaks the emoji lead: {text:?}"
    );
    assert!(text.contains("A1"), "ascii lead with the count: {text:?}");
    assert!(text.contains('!'), "ascii gated mark: {text:?}");

    m.glyphs = &SYMBOLS;
    let text = line_at(&m, now, 120);
    assert!(
        text.contains("⚡1"),
        "symbols speak the same lead: {text:?}"
    );
    assert!(text.contains('⊘'), "symbols gated mark: {text:?}");
}

#[test]
fn grants_sit_flush_right() {
    let now = Instant::now();
    let mut m = full_model();
    m.turn_started_at = None;
    m.state = ChatState::Idle;
    m.grants = GrantSummary {
        cmds: CmdGrant::Gated,
        agents: Scope::Named(1),
        schedules: true,
        ..GrantSummary::default()
    };

    for width in [120u16, 160] {
        let line = status_bar_line(&m, now, &palette(), width);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(
            line.width(),
            width as usize,
            "the line is flush right at {width}: {text:?}"
        );
        let trimmed = text.trim_end();
        assert!(
            trimmed.ends_with("🤖1"),
            "the grants segment's last glyph ends the line at {width}: {text:?}"
        );
    }
}

#[test]
fn grants_detail_orders_schedules_before_agents() {
    let now = Instant::now();
    let mut m = full_model();
    m.turn_started_at = None;
    m.state = ChatState::Idle;
    m.grants = GrantSummary {
        cmds: CmdGrant::Auto,
        agents: Scope::Named(1),
        schedules: true,
        ..GrantSummary::default()
    };

    let text = line_at(&m, now, 160);
    let schedules = text.find('📅').expect("the schedules glyph shows");
    let agents = text.find('🤖').expect("the agents glyph shows");
    assert!(
        schedules < agents,
        "📅 comes before 🤖 in the detail: {text:?}"
    );
}

#[test]
fn no_status_line_mentions_ctrl_c() {
    let now = Instant::now();
    let mut m = full_model();
    m.turn_started_at = None;
    for state in [
        ChatState::Idle,
        ChatState::Working,
        ChatState::AwaitingApproval,
    ] {
        m.state = state;
        for width in [40u16, 60, 80, 120, 160] {
            let text = line_at(&m, now, width);
            assert!(
                !text.contains("Ctrl+C"),
                "no status line mentions Ctrl+C at {width}: {text:?}"
            );
        }
    }
}

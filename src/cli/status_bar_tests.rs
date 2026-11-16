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

    // Wide: count-then-word, the word replacing the glyph.
    let mut m = full_model();
    m.ghosts = 2;
    let text = line_at(&m, now, 400);
    assert!(
        text.contains("👻 2 ghosts"),
        "the wide ghost counter carries the word after the count: {text:?}"
    );
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
        "the worst severity's glyph and the count of every firing alert: {text:?}"
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

    // Wide (a model whose wide bar fits): the severity word beside the glyph.
    let mut m = m.clone();
    m.session_name = None;
    m.turn_started_at = None;
    m.grants = GrantSummary::default();
    let text = line_at(&m, now, 400);
    assert!(
        text.contains("🔥 crit") && text.contains("DaemonGone"),
        "the wide severity carries the word beside the glyph: {text:?}"
    );
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

    // Wide: the word `resolved` beside the glyph; compact drops it.
    let wide_flash = line_at(&m, t0 + Duration::from_secs(9), 400);
    assert!(
        wide_flash.contains("🧯 resolved DiskFull"),
        "the wide flash carries the word: {wide_flash:?}"
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
            assert!(text.contains("⚠"), "alerts are tier 1: {text:?}");
            assert!(text.contains("🌓20%"), "context is tier 1: {text:?}");
            assert!(text.contains("💰?0.42"), "cost is tier 1: {text:?}");
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
        text.contains("thinking 12s"),
        "the word and seconds since the turn started (no ⏱ mark): {text:?}"
    );
    assert!(!text.contains("daemon down"), "not down: {text:?}");
    assert!(!text.contains("⏱"), "the elapsed mark is gone: {text:?}");

    // The working slot keeps the word in the ascii dialect too.
    m.glyphs = &ASCII;
    let text = line_at(&m, now, 160);
    assert!(
        text.contains("thinking 12s"),
        "ascii spells the word and seconds bare: {text:?}"
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
        text.contains("💰?0.42"),
        "the spend to two decimals with the `?` sign: {text:?}"
    );
    assert!(!text.contains("❔"), "no `❔` suffix anywhere: {text:?}");
    assert!(text.contains("⛽"), "the burn glyph: {text:?}");
    assert!(text.contains("0.4/h"), "the burn rate: {text:?}");

    m.has_untracked = false;
    let text = line_at(&m, now, 160);
    assert!(
        text.contains("💰$0.42"),
        "known pricing keeps the `$` sign: {text:?}"
    );
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
    assert!(
        line.contains("thinking 4s"),
        "the elapsed seconds show again: {line}"
    );
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

    // Core: the lead glyph, the word and the count of active kinds (sudo,
    // scripts, files all, deletes = 4).
    let text = line_at(&m, now, 160);
    assert!(text.contains("⚡ grants 4"), "four active kinds: {text:?}");

    // Detail at 160: each kind glyph, its word, the count or `all`, in the
    // table's order and after the lead.
    let lead_at = text.find('⚡').expect("the lead glyph shows");
    let mut last = lead_at;
    for part in ["🔑 sudo", "📜 scripts 2", "📄 files all", "🗑 deletes"] {
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
            for part in ["📜", "📄", "🗑", "sudo", "scripts", "files", "deletes"] {
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
    assert!(
        text.contains("A grants 1"),
        "ascii lead with the count: {text:?}"
    );
    assert!(text.contains("! gated"), "ascii gated mark: {text:?}");

    m.glyphs = &SYMBOLS;
    let text = line_at(&m, now, 120);
    assert!(
        text.contains("⚡ grants 1"),
        "symbols speak the same lead: {text:?}"
    );
    assert!(text.contains("⊘ gated"), "symbols gated mark: {text:?}");
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

// ── Phase 09: the words (M37 decision 7) ─────────────────────────────────────

/// A busy model: the full model minus the grants segment, so the wide bar
/// carries only the task-3 table's items.
fn busy_model() -> StatusModel {
    let mut m = full_model();
    m.grants = GrantSummary::default();
    m
}

#[test]
fn wide_form_carries_the_words() {
    let now = Instant::now();
    let m = busy_model();
    let text = line_at(&m, now, 400);

    // State: the word beside the glyph; `thinking` names the seconds.
    assert!(text.contains("💤 idle"), "state idle: {text:?}");
    let mut working = m.clone();
    working.state = ChatState::Working;
    working.turn_started_at = Some(now - Duration::from_secs(12));
    assert!(
        line_at(&working, now, 400).contains("🧠 thinking 12s"),
        "state working with elapsed seconds: {text:?}"
    );
    let mut approving = m.clone();
    approving.state = ChatState::AwaitingApproval;
    assert!(
        line_at(&approving, now, 400).contains("✋ approve?"),
        "state awaiting approval: {text:?}"
    );
    let mut down = m.clone();
    down.state = ChatState::DaemonDown;
    assert!(
        line_at(&down, now, 400).contains("💀 daemon down"),
        "state daemon down: {text:?}"
    );
    let mut reconnecting = m.clone();
    reconnecting.state = ChatState::Reconnecting;
    assert!(
        line_at(&reconnecting, now, 400).contains("📡 reconnecting"),
        "state reconnecting: {text:?}"
    );

    // Alert: the severity word; the count at two or more of that severity.
    assert!(text.contains("⚠ warn"), "alert severity word: {text:?}");
    let mut two = m.clone();
    two.alerts.push(AlertBadge {
        fingerprint: "fp-warn-2".to_string(),
        name: "DiskLow".to_string(),
        severity: "warning".to_string(),
        first_seen_unix: 1_700_000_000,
        last_seen_unix: 1_700_000_000,
    });
    let two_text = line_at(&two, now, 400);
    assert!(
        two_text.contains("⚠ 2 warn"),
        "the count at two or more of the worst severity: {two_text:?}"
    );

    // Resolved flash: the word `resolved` beside the glyph.
    let mut flash = m.clone();
    flash.set_alerts(Vec::new(), now);
    assert!(
        line_at(&flash, now, 400).contains("🧯 resolved DiskFull"),
        "the resolved flash's word: {text:?}"
    );

    // Context, growth, cost, burn, turn, throughput.
    assert!(text.contains("🌓 ctx 20%"), "context word: {text:?}");
    assert!(text.contains("📈 +3.1k"), "growth word: {text:?}");
    assert!(text.contains("💰 ?0.42"), "cost word: {text:?}");
    assert!(text.contains("⛽ $0.4/h burn"), "burn word: {text:?}");
    assert!(text.contains("↻ turn 12"), "turn word: {text:?}");
    assert!(text.contains("🐇 38 tok/s"), "throughput word: {text:?}");

    // Counters: count-then-word.
    assert!(text.contains("⏳ 2 bg"), "background word: {text:?}");
    assert!(text.contains("👻 1 ghost"), "ghosts singular: {text:?}");
    assert!(text.contains("🛠 7 tools"), "tools: {text:?}");
    assert!(text.contains("🐚 3 shells"), "shells: {text:?}");

    // Grants (task 4): lead, per-kind words, `all`, hosts.
    let mut granted = m.clone();
    granted.grants = GrantSummary {
        cmds: CmdGrant::Gated,
        scripts: Scope::Named(2),
        runbooks: Scope::All,
        files: Scope::All,
        agents: Scope::Named(1),
        schedules: true,
        deletes: true,
        hosts: 2,
    };
    let g = line_at(&granted, now, 400);
    for part in [
        "⚡ grants 8",
        "🔒 gated",
        "📜 scripts 2",
        "📖 runbooks all",
        "📄 files all",
        "📅 schedules",
        "🤖 agents 1",
        "🗑 deletes",
        "🌐 2 hosts",
    ] {
        assert!(g.contains(part), "grants wide part {part}: {g:?}");
    }
    assert!(
        !g.contains("sudo"),
        "gated commands say gated, not sudo: {g:?}"
    );
    let mut sudo = granted.clone();
    sudo.grants.cmds = CmdGrant::Sudo;
    assert!(
        line_at(&sudo, now, 400).contains("🔑 sudo"),
        "sudo kind word: {g:?}"
    );
    let mut one_host = granted.clone();
    one_host.grants.hosts = 1;
    assert!(
        line_at(&one_host, now, 400).contains("🌐 1 host"),
        "hosts singular: {g:?}"
    );
}

#[test]
fn words_drop_all_at_once_at_their_rank() {
    let now = Instant::now();
    let m = busy_model();

    // The widest width where the wide bar does not fit (the words dropped):
    // scan down until a counter's word is gone, then check the one before.
    let mut width = 200usize;
    while width > 20 {
        let text = line_at(&m, now, width as u16);
        if !text.contains(" ghost") {
            break;
        }
        width -= 1;
    }
    assert!(width > 20, "no width under 200 drops the words");
    let text = line_at(&m, now, width as u16);
    for word in [
        "warn", "ctx", "burn", "turn", "tok/s", " ghost", " tool", " shell", " bg",
    ] {
        assert!(
            !text.contains(word),
            "no word survives the drop: {word} in {text:?}"
        );
    }
    for part in [
        "⏳2",
        "👻1",
        "🛠7",
        "🐚3",
        "🌓20%",
        "💰?0.42",
        "↻12",
        "🐇38t/s",
    ] {
        assert!(
            text.contains(part),
            "compact form intact: {part} in {text:?}"
        );
    }
    // One column wider and the words are all present: the step is all at once.
    let wide = line_at(&m, now, (width + 1) as u16);
    assert!(
        wide.contains("idle") && wide.contains(" bg") && wide.contains(" shell"),
        "one column wider every word is back: {wide:?}"
    );
    assert!(
        line_width(&m, now, (width + 1) as u16) <= width + 1,
        "the wide bar fits at width+1"
    );
}

#[test]
fn state_and_alert_words_survive_every_drop() {
    let now = Instant::now();
    let m = busy_model();
    let text = line_at(&m, now, 50);
    assert!(
        text.contains("daemon down") || text.contains("idle") || text.contains("warn"),
        "the state and severity words are on the line: {text:?}"
    );
    // The rank drops took the droppable parts: no name, no age, no session.
    assert!(
        !text.contains("DiskFull") && !text.contains("incident-42"),
        "the rank drops took name and session: {text:?}"
    );
    assert!(
        text.contains("idle") && text.contains("⚠"),
        "the state and severity words are still there: {text:?}"
    );
    assert!(!text.contains("⏱"), "the elapsed mark stays gone: {text:?}");
}

#[test]
fn an_idle_default_bar_has_no_grants_and_reads_idle() {
    let now = Instant::now();
    let m = StatusModel {
        model: "test-model".to_string(),
        ..Default::default()
    };
    let text = line_at(&m, now, 120);
    assert!(
        text.contains("💤 idle"),
        "the default bar reads idle: {text:?}"
    );
    assert!(
        !text.contains('⚡') && !text.contains("grants"),
        "no grants segment in the default state: {text:?}"
    );
}

#[test]
fn plural_forms_follow_the_count() {
    let now = Instant::now();

    // Shells.
    let mut m = busy_model();
    m.shells = 1;
    assert!(
        line_at(&m, now, 400).contains("🐚 1 shell"),
        "1 shell: {text}",
        text = line_at(&m, now, 400)
    );
    m.shells = 2;
    assert!(
        line_at(&m, now, 400).contains("🐚 2 shells"),
        "2 shells: {text}",
        text = line_at(&m, now, 400)
    );

    // Ghosts.
    m.ghosts = 1;
    assert!(
        line_at(&m, now, 400).contains("👻 1 ghost"),
        "1 ghost: {text}",
        text = line_at(&m, now, 400)
    );
    m.ghosts = 2;
    assert!(
        line_at(&m, now, 400).contains("👻 2 ghosts"),
        "2 ghosts: {text}",
        text = line_at(&m, now, 400)
    );

    // Tools.
    m.tools_total = 1;
    assert!(
        line_at(&m, now, 400).contains("🛠 1 tool"),
        "1 tool: {text}",
        text = line_at(&m, now, 400)
    );
    m.tools_total = 3;
    assert!(
        line_at(&m, now, 400).contains("🛠 3 tools"),
        "3 tools: {text}",
        text = line_at(&m, now, 400)
    );

    // Background: one word for every count.
    m.background = 2;
    assert!(
        line_at(&m, now, 400).contains("⏳ 2 bg"),
        "2 bg: {text}",
        text = line_at(&m, now, 400)
    );

    // Hosts in the grants segment.
    let mut g = busy_model();
    g.grants = GrantSummary {
        hosts: 1,
        ..GrantSummary::default()
    };
    assert!(
        line_at(&g, now, 400).contains("🌐 1 host"),
        "1 host: {text}",
        text = line_at(&g, now, 400)
    );
    g.grants.hosts = 2;
    assert!(
        line_at(&g, now, 400).contains("🌐 2 hosts"),
        "2 hosts: {text}",
        text = line_at(&g, now, 400)
    );

    // `scripts 2` stays a label then a count.
    g.grants = GrantSummary {
        cmds: CmdGrant::Auto,
        scripts: Scope::Named(2),
        ..GrantSummary::default()
    };
    let text = line_at(&g, now, 400);
    assert!(
        text.contains("📜 scripts 2"),
        "scripts stays label-then-count: {text:?}"
    );
    assert!(
        !text.contains("2 scripts"),
        "the count never leads a scope kind: {text:?}"
    );
}

#[test]
fn scope_all_reads_all_wide_and_star_compact() {
    let now = Instant::now();
    let mut m = busy_model();
    m.grants = GrantSummary {
        cmds: CmdGrant::Auto,
        files: Scope::All,
        ..GrantSummary::default()
    };

    let wide = line_at(&m, now, 400);
    assert!(
        wide.contains("📄 files all"),
        "the wide `all` word: {wide:?}"
    );
    assert!(!wide.contains('∗'), "no star in the wide form: {wide:?}");

    // Narrow until the words drop: the compact form is today's `∗`.
    let mut width = 200usize;
    loop {
        let text = line_at(&m, now, width as u16);
        if !text.contains("files") {
            assert!(text.contains("📄∗"), "the compact all mark: {text:?}");
            break;
        }
        width -= 1;
        assert!(width > 10, "the grants word never dropped: {text:?}");
    }
}

#[test]
fn the_symbols_and_ascii_dialects_use_the_same_words() {
    let now = Instant::now();
    let m = busy_model();

    for set in [&SYMBOLS, &ASCII] {
        let mut d = m.clone();
        d.glyphs = set;
        let text = line_at(&d, now, 400);
        for word in ["idle", "burn", "turn", "ghost", "shell", "bg", "model:"] {
            assert!(
                text.contains(word),
                "{set:?} speaks the same words: {word} in {text:?}"
            );
        }
        // No dialect doubles a space around the model word: the mark leads
        // it in the glyph dialects, and the word starts the ascii slot.
        assert!(
            !text.contains("  "),
            "{set:?} never doubles a space: {text:?}"
        );
        // The ascii cost never doubles the dollar.
        assert!(
            !text.contains("$$"),
            "{set:?} cost never doubles $: {text:?}"
        );
    }

    let mut a = m.clone();
    a.glyphs = &ASCII;
    let text = line_at(&a, now, 400);
    assert!(
        !text.contains("ctx ctx"),
        "ascii context never doubles: {text:?}"
    );
    assert!(
        !text.contains("bg 2 bg"),
        "ascii background never doubles: {text:?}"
    );
}

#[test]
fn no_dialect_doubles_a_word_or_a_dollar() {
    let now = Instant::now();
    let m = busy_model();
    for set in [&EMOJI, &SYMBOLS, &ASCII] {
        let mut d = m.clone();
        d.glyphs = set;
        let text = line_at(&d, now, 160);
        assert!(!text.contains("ctx ctx"), "{set:?}: {text:?}");
        assert!(!text.contains("bg 2 bg"), "{set:?}: {text:?}");
        assert!(!text.contains("$$"), "{set:?}: {text:?}");
        assert!(!text.contains("/h/h"), "{set:?}: {text:?}");
        assert!(!text.contains("t/s38"), "{set:?}: {text:?}");
        assert!(!text.contains("❔"), "{set:?}: {text:?}");
    }
}

#[test]
fn an_unrankable_severity_reads_alert() {
    let now = Instant::now();
    let mut m = busy_model();
    m.alerts = vec![AlertBadge {
        fingerprint: "fp-odd".to_string(),
        name: "OddSev".to_string(),
        severity: "catastrophic".to_string(),
        first_seen_unix: 1_700_000_000,
        last_seen_unix: 1_700_000_000,
    }];
    let text = line_at(&m, now, 400);
    assert!(
        text.contains("alert"),
        "an unrankable severity's word is `alert`: {text:?}"
    );
    assert!(
        text.contains("❔ alert"),
        "the unknown glyph leads it: {text:?}"
    );
}

#[test]
fn a_line_never_exceeds_the_pane_width() {
    let m = busy_model();
    let now = Instant::now();
    for set in [&EMOJI, &SYMBOLS, &ASCII] {
        let mut d = m.clone();
        d.glyphs = set;
        for width in 30..=200u16 {
            let measured = line_width(&d, now, width);
            assert!(
                measured <= width as usize,
                "{set:?} at {width} measures {measured}: {text}",
                text = line_at(&d, now, width)
            );
        }
    }
}

// ── Phase 10 (M37): the info and cost glyphs ────────────────────────────────

#[test]
fn an_info_alert_shows_a_yellow_warning_glyph() {
    let now = Instant::now();
    let mut m = full_model();
    m.turn_started_at = None;
    m.state = ChatState::Idle;
    m.alerts = vec![AlertBadge {
        fingerprint: "fp-info".to_string(),
        name: "Heartbeat".to_string(),
        severity: "info".to_string(),
        first_seen_unix: 1_700_000_000,
        last_seen_unix: 1_700_000_000,
    }];

    // Compact: the glyph leads, no `ℹ` anywhere.
    let p = palette();
    let line = status_bar_line(&m, now, &p, 160);
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(
        text.contains('⚠'),
        "the info alert leads with the warning glyph: {text:?}"
    );
    assert!(!text.contains('ℹ'), "no info glyph remains: {text:?}");

    // The glyph's span is palette yellow, as the warning rank's is.
    let glyph_span = line
        .spans
        .iter()
        .find(|s| s.content.contains('⚠'))
        .expect("the alert glyph span");
    assert_eq!(
        glyph_span.style.fg,
        Some(p.yellow()),
        "the info glyph is drawn in palette yellow: {glyph_span:?}"
    );

    // Wide: the word tells info and warning apart.
    let wide = line_at(&m, now, 400);
    assert!(
        wide.contains("⚠ info Heartbeat"),
        "the wide info alert names the word and the alert: {wide:?}"
    );
}

#[test]
fn the_cost_glyph_is_a_money_bag() {
    let now = Instant::now();
    let m = full_model();

    // Compact and wide: `💰`, never `💸`. Unknown pricing speaks `?` in the
    // sign's position and never appends a mark.
    let text = line_at(&m, now, 160);
    assert!(text.contains("💰?0.42"), "compact cost: {text:?}");
    assert!(!text.contains("💸"), "no flying money glyph: {text:?}");
    assert!(!text.contains("❔"), "no `❔` suffix: {text:?}");
    let wide = line_at(&m, now, 400);
    assert!(wide.contains("💰 ?0.42"), "wide cost: {wide:?}");
    assert!(!wide.contains("💸"), "no flying money glyph wide: {wide:?}");
    assert!(!wide.contains("❔"), "no `❔` suffix wide: {wide:?}");
}

#[test]
fn the_symbols_and_ascii_alert_glyphs_are_unchanged() {
    assert_eq!(SYMBOLS.alert_info, "●");
    assert_eq!(SYMBOLS.cost, "$");
    assert_eq!(ASCII.alert_info, "i");
    assert_eq!(ASCII.cost, "$");
    // And the warning/critical glyphs in the emoji dialect are untouched.
    assert_eq!(EMOJI.alert_critical, "🔥");
    assert_eq!(EMOJI.alert_warning, "⚠");
}

// ── Phase 11 (M37): the model slot's word and its clean clip ────────────────

#[test]
fn the_model_slot_carries_the_model_word() {
    let now = Instant::now();
    let mut m = full_model();
    m.session_name = None;
    m.model = "ollama/deepseek-v4.1-flash".to_string();

    // Wide: the word sits between the vendor mark and the name.
    let text = line_at(&m, now, 400);
    let mark_at = text.find("🦙").expect("the local vendor mark shows");
    let word_at = text
        .find("model: deepseek-v4")
        .expect("the model word shows");
    assert!(
        mark_at < word_at && text[mark_at..].starts_with("🦙 model: deepseek-v4"),
        "the word follows the vendor mark: {text:?}"
    );

    // Compact (the words have dropped): the glyph and the name, no word.
    let mut width = 200usize;
    while width > 20 {
        let text = line_at(&m, now, width as u16);
        if !text.contains("model:") {
            assert!(
                text.contains("🦙 deepseek-v4"),
                "the compact form is glyph and name: {text:?}"
            );
            break;
        }
        width -= 1;
    }
    assert!(width > 20, "the model word never dropped");
}

#[test]
fn the_model_name_is_clipped_at_a_token_boundary() {
    assert_eq!(short_model_name("deepseek-v4.1-flash"), "deepseek-v4");
    assert_eq!(short_model_name("qwen3.8-27b-instruct"), "qwen3.8-27b");
    assert_eq!(
        short_model_name("llama3.3-70b-instruct-q4_K_M"),
        "llama3.3-70b"
    );
    assert_eq!(short_model_name("abcdefghijklmnop"), "abcdefghijkl");
    assert_eq!(short_model_name("sonnet-4-6"), "sonnet-4-6");

    // Never end on a separator; a name of exactly 12 columns is untouched.
    for name in ["a.b.c.d.e.fgh", "x-y_z.w-v", "abcdefghijkl"] {
        let short = short_model_name(name);
        assert!(
            !short.ends_with(['-', '.', '_']),
            "the clip never ends on a separator: {name} → {short}"
        );
    }
    assert_eq!(short_model_name("abcdefghijkl"), "abcdefghijkl");
}

#[test]
fn the_ascii_dialect_starts_the_model_slot_with_the_word() {
    let now = Instant::now();
    let mut m = full_model();
    m.turn_started_at = None;
    m.state = ChatState::Idle;
    m.alerts.clear();
    m.session_name = None;
    m.glyphs = &ASCII;
    let text = line_at(&m, now, 400);
    let at = text
        .find("model: sonnet-4-6")
        .expect("the ascii model word shows");
    let prev = text[..at].chars().next_back();
    assert!(
        prev.is_none_or(|c| c == ' '),
        "one space between slots, never two, beside the word: {text:?}"
    );
    let after = &text[at + "model: sonnet-4-6".len()..];
    assert!(
        !after.trim_start().starts_with(' '),
        "no doubled space after the model slot: {text:?}"
    );
}

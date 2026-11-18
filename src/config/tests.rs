use super::seeds::SRE_PROMPT_TOML;
use super::*;

// ── Default values ───────────────────────────────────────────────────────

#[test]
fn default_config_has_default_model() {
    let cfg = Config::default();
    let entry = cfg.resolve_model(None);
    assert_eq!(entry.provider, "anthropic");
    assert_eq!(entry.model, "claude-sonnet-4-6");
}

#[test]
fn default_config_ai_prompt() {
    assert_eq!(Config::default().ai.prompt, "sre");
}

#[test]
fn default_config_environment() {
    assert_eq!(Config::default().context.environment, "personal");
}

#[test]
fn default_config_masking_empty() {
    assert!(Config::default().masking.extra_patterns.is_empty());
}

// ── TOML parsing ─────────────────────────────────────────────────────────

#[test]
fn parse_models_section() {
    let toml = r#"
            [models.default]
            provider = "openai"
            model    = "gpt-4o"

            [models.big]
            provider = "anthropic"
            model    = "claude-opus-4-6"

            [ai]
            prompt = "custom"
        "#;
    let cfg: Config = toml::from_str(toml).unwrap();
    let def = cfg.resolve_model(None);
    assert_eq!(def.provider, "openai");
    assert_eq!(def.model, "gpt-4o");
    let big = cfg.resolve_model(Some("big"));
    assert_eq!(big.model, "claude-opus-4-6");
    assert_eq!(cfg.ai.prompt, "custom");
}

#[test]
fn parse_context_section() {
    let toml = r#"
            [context]
            environment = "production"
        "#;
    let cfg: Config = toml::from_str(toml).unwrap();
    assert_eq!(cfg.context.environment, "production");
}

#[test]
fn parse_masking_section() {
    let toml = r#"
            [masking]
            extra_patterns = ["MYCO-[A-Z0-9]{8}", "sk_live_\\w+"]
        "#;
    let cfg: Config = toml::from_str(toml).unwrap();
    assert_eq!(cfg.masking.extra_patterns.len(), 2);
    assert_eq!(cfg.masking.extra_patterns[0], "MYCO-[A-Z0-9]{8}");
}

#[test]
fn missing_sections_fall_back_to_defaults() {
    let cfg: Config = toml::from_str("").unwrap();
    let entry = cfg.resolve_model(None);
    assert_eq!(entry.provider, "anthropic");
    assert_eq!(cfg.context.environment, "personal");
    assert!(cfg.masking.extra_patterns.is_empty());
}

#[test]
fn ai_timeout_keys_parse_and_default() {
    let toml = r#"
            [ai]
            first_token_timeout_secs = 10
        "#;
    let cfg: Config = toml::from_str(toml).unwrap();
    assert_eq!(cfg.ai.first_token_timeout_secs, 10);
    assert_eq!(cfg.ai.stream_idle_timeout_secs, 240);
    assert_eq!(cfg.ai.connect_timeout_secs, 30);

    let def = AiConfig::default();
    assert_eq!(def.first_token_timeout_secs, 600);
    assert_eq!(def.stream_idle_timeout_secs, 240);
    assert_eq!(def.connect_timeout_secs, 30);
}

#[test]
fn resolve_model_unknown_name_falls_back_to_default() {
    let cfg = Config::default();
    let entry = cfg.resolve_model(Some("nonexistent"));
    assert_eq!(entry.provider, "anthropic");
}

#[test]
fn available_models_returns_sorted_keys() {
    let toml = r#"
            [models.default]
            provider = "anthropic"
            model    = "claude-sonnet-4-6"
            [models.opus]
            provider = "anthropic"
            model    = "claude-opus-4-6"
            [models.local]
            provider = "ollama"
            model    = "llama3.2"
        "#;
    let cfg: Config = toml::from_str(toml).unwrap();
    let names = cfg.available_models();
    assert_eq!(names, vec!["default", "local", "opus"]);
}

// ── ModelEntry methods ───────────────────────────────────────────────────

#[test]
fn model_entry_context_window_claude() {
    let entry = ModelEntry {
        model: "claude-sonnet-4-6".to_string(),
        ..ModelEntry::default()
    };
    assert_eq!(entry.context_window(), 200_000);
}

#[test]
fn model_entry_context_window_override() {
    let entry = ModelEntry {
        context_window_tokens: Some(8192),
        ..ModelEntry::default()
    };
    assert_eq!(entry.context_window(), 8192);
}

// ── Builtin prompt ───────────────────────────────────────────────────────

#[test]
fn builtin_sre_prompt_parses() {
    let def = toml::from_str::<PromptDef>(SRE_PROMPT_TOML);
    assert!(def.is_ok(), "SRE_PROMPT_TOML must be valid TOML");
    let def = def.unwrap();
    assert!(!def.system.is_empty());
}

#[test]
fn builtin_minimal_prompt_is_nonempty() {
    let def = PromptDef::builtin_minimal();
    assert!(!def.system.is_empty());
}

// ── load_named_prompt fallback chain ─────────────────────────────────────

#[test]
fn load_sre_prompt_falls_back_to_builtin() {
    // "sre" should always succeed even without a file on disk (compiled-in fallback).
    let def = load_named_prompt("sre");
    assert!(!def.system.is_empty());
}

#[test]
fn load_unknown_prompt_returns_minimal() {
    let def = load_named_prompt("__nonexistent_prompt_xyz__");
    assert!(!def.system.is_empty());
}

// ── ApprovalsConfig ──────────────────────────────────────────────────────

#[test]
fn default_approvals_match_current_behavior() {
    let cfg = ApprovalsConfig::default();
    assert!(
        cfg.commands,
        "non-sudo commands must default to auto-approved"
    );
    assert!(!cfg.sudo);
    assert!(!cfg.scripts);
    assert!(!cfg.runbooks);
    assert!(!cfg.file_edits);
    assert!(!cfg.ghost_commands);
}

#[test]
fn approvals_config_parses_all_fields() {
    let toml = r#"
            [approvals]
            commands      = true
            sudo          = true
            scripts       = true
            runbooks      = true
            file_edits    = true
            ghost_commands = true
        "#;
    let cfg: Config = toml::from_str(toml).unwrap();
    assert!(cfg.approvals.commands);
    assert!(cfg.approvals.sudo);
    assert!(cfg.approvals.scripts);
    assert!(cfg.approvals.runbooks);
    assert!(cfg.approvals.file_edits);
    assert!(cfg.approvals.ghost_commands);
}

#[test]
fn missing_approvals_section_uses_defaults() {
    let cfg: Config = toml::from_str("").unwrap();
    assert!(cfg.approvals.commands);
    assert!(!cfg.approvals.sudo);
    assert!(!cfg.approvals.scripts);
    assert!(!cfg.approvals.runbooks);
    assert!(!cfg.approvals.file_edits);
    assert!(!cfg.approvals.ghost_commands);
}

#[test]
fn partial_approvals_section_fills_remaining_defaults() {
    let toml = r#"
            [approvals]
            sudo = true
        "#;
    let cfg: Config = toml::from_str(toml).unwrap();
    assert!(
        cfg.approvals.commands,
        "commands must still default to true"
    );
    assert!(cfg.approvals.sudo);
    assert!(!cfg.approvals.scripts);
    assert!(!cfg.approvals.ghost_commands);
}

// ── LimitsConfig ─────────────────────────────────────────────────────────

#[test]
fn default_limits_match_current_hardcoded_constants() {
    let limits = LimitsConfig::default();
    assert_eq!(
        limits.per_tool_batch, 100,
        "must match MAX_SAME_TOOL_BATCH in server.rs"
    );
    assert_eq!(
        limits.tool_result_chars, 16_000,
        "must match MAX_TOOL_RESULT_CHARS in server.rs"
    );
    assert_eq!(
        limits.total_tool_calls_per_turn, 0,
        "new field defaults to uncapped"
    );
    assert_eq!(limits.max_turns, 0, "new field defaults to uncapped");
    assert_eq!(
        limits.max_tool_calls_per_session, 0,
        "new field defaults to uncapped"
    );
    assert!(limits.per_tool.is_empty());
}

#[test]
fn missing_limits_section_uses_defaults() {
    let cfg: Config = toml::from_str("").unwrap();
    assert_eq!(cfg.limits.per_tool_batch, 100);
    assert_eq!(cfg.limits.tool_result_chars, 16_000);
    assert_eq!(cfg.limits.total_tool_calls_per_turn, 0);
    assert_eq!(cfg.limits.max_turns, 0);
    assert_eq!(cfg.limits.max_tool_calls_per_session, 0);
}

#[test]
fn limits_section_parses_all_fields() {
    let toml = r#"
            [limits]
            per_tool_batch            = 200
            total_tool_calls_per_turn = 50
            tool_result_chars         = 8000
            max_turns                 = 100
            max_tool_calls_per_session = 500

            [limits.per_tool]
            read_file         = 300
            search_repository = 25
        "#;
    let cfg: Config = toml::from_str(toml).unwrap();
    let l = &cfg.limits;
    assert_eq!(l.per_tool_batch, 200);
    assert_eq!(l.total_tool_calls_per_turn, 50);
    assert_eq!(l.tool_result_chars, 8000);
    assert_eq!(l.max_turns, 100);
    assert_eq!(l.max_tool_calls_per_session, 500);
    assert_eq!(l.per_tool.get("read_file").copied(), Some(300));
    assert_eq!(l.per_tool.get("search_repository").copied(), Some(25));
}

#[test]
fn partial_limits_section_fills_remaining_defaults() {
    let toml = r#"
            [limits]
            max_turns = 40
        "#;
    let cfg: Config = toml::from_str(toml).unwrap();
    assert_eq!(cfg.limits.max_turns, 40);
    assert_eq!(cfg.limits.per_tool_batch, 100, "should still default");
    assert_eq!(cfg.limits.tool_result_chars, 16_000, "should still default");
}

#[test]
fn limits_zero_means_uncapped() {
    let toml = r#"
            [limits]
            per_tool_batch    = 0
            tool_result_chars = 0
        "#;
    let cfg: Config = toml::from_str(toml).unwrap();
    assert!(LimitsConfig::cap_u32(cfg.limits.per_tool_batch).is_none());
    assert!(LimitsConfig::cap_usize(cfg.limits.tool_result_chars).is_none());
}

#[test]
fn turn_timeout_secs_parses_and_defaults_to_zero() {
    let toml = "[limits]\nturn_timeout_secs = 10\n";
    let cfg: Config = toml::from_str(toml).unwrap();
    assert_eq!(cfg.limits.turn_timeout_secs, 10);

    let cfg: Config = toml::from_str("").unwrap();
    assert_eq!(cfg.limits.turn_timeout_secs, 0);
}

#[test]
fn cap_u32_sentinel() {
    assert_eq!(LimitsConfig::cap_u32(0), None);
    assert_eq!(LimitsConfig::cap_u32(1), Some(1));
    assert_eq!(LimitsConfig::cap_u32(100), Some(100));
}

#[test]
fn cap_usize_sentinel() {
    assert_eq!(LimitsConfig::cap_usize(0), None);
    assert_eq!(LimitsConfig::cap_usize(1), Some(1));
    assert_eq!(LimitsConfig::cap_usize(80), Some(80));
}

#[test]
fn per_tool_cap_uses_override_over_global() {
    let mut limits = LimitsConfig::default(); // per_tool_batch = 100
    limits.per_tool.insert("read_file".to_string(), 200);
    assert_eq!(limits.per_tool_cap("read_file"), Some(200));
    assert_eq!(limits.per_tool_cap("search_repository"), Some(100)); // falls back to global
}

#[test]
fn per_tool_cap_zero_override_means_uncapped() {
    let mut limits = LimitsConfig::default();
    limits.per_tool.insert("read_file".to_string(), 0);
    assert_eq!(limits.per_tool_cap("read_file"), None);
}

#[test]
fn per_tool_cap_zero_global_means_all_uncapped() {
    let limits = LimitsConfig {
        per_tool_batch: 0,
        ..LimitsConfig::default()
    };
    assert_eq!(limits.per_tool_cap("read_file"), None);
    assert_eq!(limits.per_tool_cap("get_shell_context"), None);
}

#[test]
fn validate_approval_gated_per_tool_entry_does_not_panic() {
    // The validate() call should warn (via log::warn!) but never panic.
    // Verify the condition that triggers the warning: an approval-gated tool
    // appearing in per_tool. The warning is observable in daemon.log at runtime.
    let mut limits = LimitsConfig::default();
    limits
        .per_tool
        .insert("run_terminal_command".to_string(), 5);
    assert!(
        limits.per_tool.contains_key("run_terminal_command"),
        "precondition: entry must be present to trigger warning path"
    );
    limits.validate(); // must not panic
}

#[test]
fn compaction_config_defaults_and_validation() {
    // A partial [compaction] section: only compact_at_pct set; the rest
    // must default.
    let toml_src = r#"
            [models.default]
            provider = "anthropic"
            api_key  = "sk-ant-test"
            model    = "claude-sonnet-4-6"

            [compaction]
            compact_at_pct = 70
        "#;
    let cfg: Config = toml::from_str(toml_src).unwrap();
    assert_eq!(cfg.compaction.compact_at_pct, 70);
    assert_eq!(cfg.compaction.elide_at_pct, 50);
    assert_eq!(cfg.compaction.target_pct, 40);
    assert_eq!(cfg.compaction.emergency_pct, 85);

    // An invalid pair (target_pct >= compact_at_pct) must warn and fall back
    // to defaults for the pair so hysteresis is preserved.
    let mut bad = Config::default();
    bad.compaction.target_pct = 80;
    bad.compaction.compact_at_pct = 60;
    bad.validate_compaction();
    let d = CompactionConfig::default();
    assert_eq!(bad.compaction.target_pct, d.target_pct);
    assert_eq!(bad.compaction.compact_at_pct, d.compact_at_pct);
}

#[test]
fn config_migration_old_toml_without_limits_section_matches_constants() {
    // A config.toml that predates [limits] must parse cleanly and produce
    // exactly the same numeric constants that were previously hardcoded.
    let old_config = r#"
            [models.default]
            provider = "anthropic"
            api_key  = "sk-ant-test"
            model    = "claude-sonnet-4-6"
        "#;
    let cfg: Config = toml::from_str(old_config).unwrap();
    assert_eq!(
        cfg.limits.per_tool_batch, 100,
        "must match legacy MAX_SAME_TOOL_BATCH = 100"
    );
    assert_eq!(
        cfg.limits.tool_result_chars, 16_000,
        "must match legacy MAX_TOOL_RESULT_CHARS = 16_000"
    );
    assert_eq!(
        cfg.limits.total_tool_calls_per_turn, 0,
        "new — default uncapped"
    );
    assert_eq!(cfg.limits.max_turns, 0, "new — default uncapped");
    assert_eq!(
        cfg.limits.max_tool_calls_per_session, 0,
        "new — default uncapped"
    );
    assert!(
        cfg.limits.per_tool.is_empty(),
        "new — no overrides by default"
    );
}

// ── Pricing schema (Phase 1) ─────────────────────────────────────────────

#[test]
fn model_entry_with_explicit_pricing_overrides_defaults() {
    let entry = ModelEntry {
        provider: "anthropic".to_string(),
        model: "claude-sonnet-4-6".to_string(),
        input_cost_per_mtok: Some(5.0),
        output_cost_per_mtok: Some(20.0),
        cache_read_cost_per_mtok: Some(0.50),
        cache_write_cost_per_mtok: Some(6.25),
        ..ModelEntry::default()
    };
    let pricing = entry.pricing().expect("pricing must resolve");
    assert_eq!(pricing.input_per_mtok, 5.0);
    assert_eq!(pricing.output_per_mtok, 20.0);
    assert_eq!(pricing.cache_read_per_mtok, 0.50);
    assert_eq!(pricing.cache_write_per_mtok, 6.25);
    assert_eq!(pricing.source, PricingSource::UserConfig);
}

#[test]
fn model_entry_without_pricing_returns_none() {
    // No cost fields in config → Unknown pricing; cost accounting reports $0+.
    let entry = ModelEntry {
        provider: "anthropic".to_string(),
        model: "claude-sonnet-4-6".to_string(),
        ..ModelEntry::default()
    };
    assert!(entry.pricing().is_none());
}

#[test]
fn model_entry_no_cost_fields_returns_none() {
    // Any model without cost fields in config returns None regardless of name.
    for model in &["claude-unknown-future-model", "claude-sonnet-4-6", "gpt-4o"] {
        let entry = ModelEntry {
            provider: "anthropic".to_string(),
            model: model.to_string(),
            ..ModelEntry::default()
        };
        assert!(entry.pricing().is_none(), "expected None for {model}");
    }
}

#[test]
fn local_provider_pricing_is_zero() {
    for provider in &["ollama", "lmstudio"] {
        let entry = ModelEntry {
            provider: provider.to_string(),
            model: "some-local-model".to_string(),
            ..ModelEntry::default()
        };
        let pricing = entry.pricing().expect("local pricing must resolve");
        assert_eq!(pricing.input_per_mtok, 0.0);
        assert_eq!(pricing.output_per_mtok, 0.0);
        assert_eq!(pricing.cache_read_per_mtok, 0.0);
        assert_eq!(pricing.cache_write_per_mtok, 0.0);
        assert_eq!(pricing.source, PricingSource::Local);
    }
}

#[test]
fn local_provider_with_user_rates_uses_them() {
    for provider in &["ollama", "lmstudio"] {
        let entry = ModelEntry {
            provider: provider.to_string(),
            model: "some-local-model".to_string(),
            input_cost_per_mtok: Some(1.0),
            output_cost_per_mtok: Some(2.0),
            cache_read_cost_per_mtok: Some(0.1),
            cache_write_cost_per_mtok: Some(0.2),
            ..ModelEntry::default()
        };
        let pricing = entry.pricing().expect("user rates must resolve pricing");
        assert_eq!(pricing.input_per_mtok, 1.0, "{provider}: input rate");
        assert_eq!(pricing.output_per_mtok, 2.0, "{provider}: output rate");
        assert_eq!(
            pricing.cache_read_per_mtok, 0.1,
            "{provider}: cache-read rate"
        );
        assert_eq!(
            pricing.cache_write_per_mtok, 0.2,
            "{provider}: cache-write rate"
        );
        assert_eq!(pricing.source, PricingSource::UserConfig);
    }
}

#[test]
fn local_provider_partial_rates_price_unset_buckets_at_zero() {
    let entry = ModelEntry {
        provider: "ollama".to_string(),
        model: "some-local-model".to_string(),
        output_cost_per_mtok: Some(3.0),
        ..ModelEntry::default()
    };
    let pricing = entry.pricing().expect("one set rate must resolve pricing");
    assert_eq!(pricing.input_per_mtok, 0.0); // unset → zero
    assert_eq!(pricing.output_per_mtok, 3.0); // user value
    assert_eq!(pricing.cache_read_per_mtok, 0.0); // unset → zero
    assert_eq!(pricing.cache_write_per_mtok, 0.0); // unset → zero
    assert_eq!(pricing.source, PricingSource::UserConfig);
}

#[test]
fn pricing_partial_override_unset_fields_are_zero() {
    // User sets input rate only; unset fields default to 0.0.
    let entry = ModelEntry {
        provider: "anthropic".to_string(),
        model: "claude-sonnet-4-6".to_string(),
        input_cost_per_mtok: Some(4.0),
        ..ModelEntry::default()
    };
    let pricing = entry.pricing().expect("pricing must resolve");
    assert_eq!(pricing.input_per_mtok, 4.0); // user value
    assert_eq!(pricing.output_per_mtok, 0.0); // unset → zero
    assert_eq!(pricing.cache_read_per_mtok, 0.0); // unset → zero
    assert_eq!(pricing.cache_write_per_mtok, 0.0); // unset → zero
    assert_eq!(pricing.source, PricingSource::UserConfig);
}

#[test]
fn pricing_user_override_on_unknown_model_uses_zero_for_unset_rates() {
    // User sets input rate only on a model with no builtin entry.
    // Missing rates fall back to 0.0 (no builtin to merge with).
    let entry = ModelEntry {
        provider: "anthropic".to_string(),
        model: "claude-unknown-future-model".to_string(),
        input_cost_per_mtok: Some(6.0),
        ..ModelEntry::default()
    };
    let pricing = entry
        .pricing()
        .expect("pricing must resolve when user sets a rate");
    assert_eq!(pricing.input_per_mtok, 6.0); // user override
    assert_eq!(pricing.output_per_mtok, 0.0); // zero fallback (no builtin)
    assert_eq!(pricing.cache_read_per_mtok, 0.0); // zero fallback
    assert_eq!(pricing.cache_write_per_mtok, 0.0); // zero fallback
    assert_eq!(pricing.source, PricingSource::UserConfig);
}

// ── SandboxConfig ────────────────────────────────────────────────────────

#[test]
fn sandbox_defaults_are_disabled_and_non_root() {
    let sbx = SandboxConfig::default();
    assert!(!sbx.enabled);
    assert_eq!(sbx.runtime, "sbx");
    assert_eq!(sbx.run_as, "1000:1000");
    assert_eq!(sbx.workdir, "/de/work");
    assert!(!sbx.runs_as_container_root());
}

#[test]
fn sandbox_limits_defaults_match_documented_values() {
    let l = SandboxLimits::default();
    assert_eq!(l.memory, "1g");
    assert_eq!(l.pids, 256);
    assert_eq!(l.cpus, 2.0);
    assert_eq!(l.scratch, "2g");
}

#[test]
fn missing_sandbox_section_uses_defaults() {
    let toml_src = r#"
            [models.default]
            provider = "anthropic"
            api_key  = "sk-ant-test"
            model    = "claude-sonnet-4-6"
        "#;
    let cfg: Config = toml::from_str(toml_src).unwrap();
    let sbx = &cfg.sandbox;
    assert!(!sbx.enabled);
    assert_eq!(sbx.runtime, "sbx");
    assert_eq!(sbx.run_as, "1000:1000");
    assert_eq!(sbx.workdir, "/de/work");
    assert_eq!(sbx.limits.memory, "1g");
    assert_eq!(sbx.limits.pids, 256);
    assert_eq!(sbx.limits.cpus, 2.0);
    assert_eq!(sbx.limits.scratch, "2g");
    assert!(sbx.ghost_defaults.destroy_on_exit);
    assert_eq!(sbx.ghost_defaults.mount_scripts, "ro");
    assert_eq!(sbx.image, "daemoneye-agent-base");
    assert_eq!(sbx.docker_host, "unix:///run/user/1000/docker.sock");
}

#[test]
fn sandbox_section_parses_all_fields() {
    let toml_src = r#"
            [models.default]
            provider = "anthropic"
            api_key  = "sk-ant-test"
            model    = "claude-sonnet-4-6"

            [sandbox]
            enabled     = true
            runtime     = "podman"
            image       = "my-image"
            workdir     = "/de/work-custom"
            run_as      = "0:0"
            docker_host = "unix:///run/user/1000/docker.sock"

            [sandbox.limits]
            memory  = "2g"
            pids    = 512
            cpus    = 4.0
            scratch = "4g"

            [sandbox.ghost_defaults]
            destroy_on_exit = false
            mount_scripts   = "rw"
        "#;
    let cfg: Config = toml::from_str(toml_src).unwrap();
    let sbx = &cfg.sandbox;
    assert!(sbx.enabled);
    assert_eq!(sbx.runtime, "podman");
    assert_eq!(sbx.image, "my-image");
    assert_eq!(sbx.workdir, "/de/work-custom");
    assert_eq!(sbx.run_as, "0:0");
    assert_eq!(sbx.docker_host, "unix:///run/user/1000/docker.sock");
    assert_eq!(sbx.limits.memory, "2g");
    assert_eq!(sbx.limits.pids, 512);
    assert_eq!(sbx.limits.cpus, 4.0);
    assert_eq!(sbx.limits.scratch, "4g");
    assert!(!sbx.ghost_defaults.destroy_on_exit);
    assert_eq!(sbx.ghost_defaults.mount_scripts, "rw");
}

#[test]
fn partial_sandbox_section_fills_remaining_defaults() {
    let toml_src = r#"
            [models.default]
            provider = "anthropic"
            api_key  = "sk-ant-test"
            model    = "claude-sonnet-4-6"

            [sandbox]
            enabled = true
        "#;
    let cfg: Config = toml::from_str(toml_src).unwrap();
    let sbx = &cfg.sandbox;
    assert!(sbx.enabled);
    assert_eq!(sbx.run_as, "1000:1000");
    assert_eq!(sbx.runtime, "sbx");
    assert_eq!(sbx.limits.memory, "1g");
    assert_eq!(sbx.limits.pids, 256);
    assert_eq!(sbx.limits.cpus, 2.0);
    assert_eq!(sbx.limits.scratch, "2g");
    assert!(sbx.ghost_defaults.destroy_on_exit);
    assert_eq!(sbx.ghost_defaults.mount_scripts, "ro");
}

#[test]
fn sandbox_profile_table_parses_named_profiles() {
    let toml_src = r#"
            [models.default]
            provider = "anthropic"
            api_key  = "sk-ant-test"
            model    = "claude-sonnet-4-6"

            [sandbox.profile.researcher]
            network      = "proxy"
            proxy_allow  = ["crates.io"]
        "#;
    let cfg: Config = toml::from_str(toml_src).unwrap();
    let prof = cfg
        .sandbox
        .profile
        .get("researcher")
        .expect("profile present");
    assert_eq!(prof.network, "proxy");
    assert_eq!(prof.proxy_allow, vec!["crates.io"]);
    assert!(
        !cfg.sandbox.profile.contains_key("analyst"),
        "absent profile must not materialize"
    );
}

#[test]
fn sandbox_run_as_root_detection_pins_negative_cases() {
    let cases = [
        ("1000:1000", false),
        ("10:0", false),
        ("0:0", true),
        ("root:root", true),
        ("", true),
    ];
    for (run_as, expected) in cases {
        let sbx = SandboxConfig {
            run_as: run_as.to_string(),
            ..SandboxConfig::default()
        };
        assert_eq!(
            sbx.runs_as_container_root(),
            expected,
            "run_as = {run_as:?}"
        );
    }
}

#[test]
fn sandbox_validate_warns_and_never_panics() {
    let sbx = SandboxConfig {
        run_as: "0:0".to_string(),
        runtime: "podman".to_string(),
        profile: std::collections::HashMap::from([(
            "researcher".to_string(),
            SandboxProfile {
                network: "proxy".to_string(),
                proxy_allow: Vec::new(),
                proxy_deny: Vec::new(),
            },
        )]),
        ..SandboxConfig::default()
    };
    assert!(sbx.runs_as_container_root());
    assert_ne!(sbx.runtime, "docker");
    let prof = sbx.profile.get("researcher").unwrap();
    assert_eq!(prof.network, "proxy");
    assert!(prof.proxy_allow.is_empty());
    sbx.validate(); // must not panic
}

#[test]
fn sandbox_warnings_default_is_empty() {
    let sbx = SandboxConfig::default();
    assert_eq!(sbx.runtime, "sbx");
    assert!(sbx.warnings().is_empty());
}

#[test]
fn sandbox_warnings_accept_sbx_runtime() {
    let sbx = SandboxConfig {
        runtime: "sbx".to_string(),
        ..SandboxConfig::default()
    };
    assert!(
        sbx.warnings().is_empty(),
        "a default sbx config is valid; got {:?}",
        sbx.warnings()
    );
}

#[test]
fn sandbox_warnings_unknown_runtime_still_checks_profiles() {
    let sbx = SandboxConfig {
        runtime: "podman".to_string(),
        profile: std::collections::HashMap::from([(
            "bad".to_string(),
            SandboxProfile {
                network: "bogus".to_string(),
                proxy_allow: Vec::new(),
                proxy_deny: Vec::new(),
            },
        )]),
        ..SandboxConfig::default()
    };
    let warnings = sbx.warnings();
    assert_eq!(warnings.len(), 2, "got {warnings:#?}");
    assert!(warnings[0].contains("podman"), "got {warnings:#?}");
    assert!(warnings[1].contains("profile.bad"), "got {warnings:#?}");
}

#[test]
fn sandbox_warnings_reject_unknown_runtime() {
    let sbx = SandboxConfig {
        runtime: "podman".to_string(),
        run_as: "0:0".to_string(),
        image: "x".to_string(),
        profile: std::collections::HashMap::from([(
            "researcher".to_string(),
            SandboxProfile {
                network: "proxy".to_string(),
                proxy_allow: Vec::new(),
                proxy_deny: Vec::new(),
            },
        )]),
        ..SandboxConfig::default()
    };
    let warnings = sbx.warnings();
    assert_eq!(warnings.len(), 4, "got {warnings:#?}");
    assert!(warnings[0].contains("podman"), "got {warnings:#?}");
    assert!(warnings[0].contains("sbx"), "got {warnings:#?}");
    assert!(warnings[1].contains("image"), "got {warnings:#?}");
    assert!(warnings[2].contains("run_as"), "got {warnings:#?}");
    assert!(
        warnings[3].contains("profile.researcher"),
        "got {warnings:#?}"
    );
}

#[test]
fn sandbox_warnings_flag_docker_only_keys_under_sbx() {
    let sbx = SandboxConfig {
        runtime: "sbx".to_string(),
        image: "my-image".to_string(),
        run_as: "0:0".to_string(),
        limits: SandboxLimits {
            pids: 10,
            ..SandboxLimits::default()
        },
        ..SandboxConfig::default()
    };
    let warnings = sbx.warnings();
    assert_eq!(warnings.len(), 3, "got {warnings:#?}");
    assert!(warnings[0].contains("image"), "got {warnings:#?}");
    assert!(warnings[0].contains("has no effect"), "got {warnings:#?}");
    assert!(warnings[1].contains("run_as"), "got {warnings:#?}");
    assert!(warnings[1].contains("has no effect"), "got {warnings:#?}");
    assert!(warnings[2].contains("limits.pids"), "got {warnings:#?}");
    assert!(warnings[2].contains("has no effect"), "got {warnings:#?}");
    assert!(
        warnings.iter().all(|w| !w.contains("container root")),
        "sbx never warns on container root: {warnings:#?}"
    );
}

#[test]
fn sandbox_warnings_flag_a_docker_config_as_removed() {
    let sbx = SandboxConfig {
        runtime: "docker".to_string(),
        run_as: "0:0".to_string(),
        image: "my-image".to_string(),
        ..SandboxConfig::default()
    };
    let warnings = sbx.warnings();
    assert_eq!(warnings.len(), 3, "got {warnings:#?}");
    assert!(
        warnings[0].contains(r#"runtime = "docker" is ignored"#),
        "got {warnings:#?}"
    );
    assert!(warnings[0].contains("removed"), "got {warnings:#?}");
    assert!(warnings[1].contains("image"), "got {warnings:#?}");
    assert!(warnings[1].contains("has no effect"), "got {warnings:#?}");
    assert!(warnings[2].contains("run_as"), "got {warnings:#?}");
    assert!(warnings[2].contains("has no effect"), "got {warnings:#?}");
    assert!(
        warnings.iter().all(|w| !w.contains("container root")),
        "the docker container-root warning is gone: {warnings:#?}"
    );
}

#[test]
fn sandbox_warnings_never_flag_shared_keys_under_sbx() {
    let sbx = SandboxConfig {
        runtime: "sbx".to_string(),
        enabled: true,
        limits: SandboxLimits {
            memory: "4g".to_string(),
            cpus: 8.0,
            ..SandboxLimits::default()
        },
        ghost_defaults: SandboxGhostDefaults {
            destroy_on_exit: false,
            ..SandboxGhostDefaults::default()
        },
        ..SandboxConfig::default()
    };
    assert!(
        sbx.warnings().is_empty(),
        "shared keys must not warn under sbx: {:?}",
        sbx.warnings()
    );
}

#[test]
fn sandbox_warnings_sbx_proxy_profile_is_additive() {
    let sbx = SandboxConfig {
        runtime: "sbx".to_string(),
        profile: std::collections::HashMap::from([(
            "researcher".to_string(),
            SandboxProfile {
                network: "proxy".to_string(),
                proxy_allow: Vec::new(),
                proxy_deny: Vec::new(),
            },
        )]),
        ..SandboxConfig::default()
    };
    let warnings = sbx.warnings();
    assert_eq!(warnings.len(), 1, "got {warnings:#?}");
    assert!(
        warnings[0].contains("profile.researcher"),
        "got {warnings:#?}"
    );
    assert!(
        warnings[0].contains("global sbx network policy"),
        "got {warnings:#?}"
    );
    assert!(
        !warnings[0].contains("equivalent to"),
        "an empty proxy_allow is not equivalent to none under sbx: {warnings:#?}"
    );
}

#[test]
fn sandbox_warnings_list_profiles_in_name_order() {
    let sbx = SandboxConfig {
        runtime: "docker".to_string(),
        profile: std::collections::HashMap::from([
            (
                "zeta".to_string(),
                SandboxProfile {
                    network: "bogus".to_string(),
                    proxy_allow: Vec::new(),
                    proxy_deny: Vec::new(),
                },
            ),
            (
                "alpha".to_string(),
                SandboxProfile {
                    network: "bogus".to_string(),
                    proxy_allow: Vec::new(),
                    proxy_deny: Vec::new(),
                },
            ),
        ]),
        ..SandboxConfig::default()
    };
    let warnings = sbx.warnings();
    assert_eq!(warnings.len(), 3, "got {warnings:#?}");
    assert!(warnings[0].contains("runtime"), "got {warnings:#?}");
    assert!(warnings[1].contains("profile.alpha"), "got {warnings:#?}");
    assert!(warnings[2].contains("profile.zeta"), "got {warnings:#?}");
}

// ── ExecutionConfig and ShellsConfig (M20 phase-01) ─────────────────────

#[test]
fn execution_backend_defaults_to_pty() {
    let exec = ExecutionConfig::default();
    assert_eq!(exec.backend, "pty");
}

/// A config written before phase 05 still says `backend = "tmux"`. It
/// must load, keep the value for the startup warning, and validate
/// without panicking. Nothing reads the value for routing any more:
/// every command takes the PTY substrate.
#[test]
fn old_tmux_backend_config_still_loads() {
    let old: Config = toml::from_str("[execution]\nbackend = \"tmux\"\n")
        .expect("an old tmux-backend config must still parse");
    assert_eq!(old.execution.backend, "tmux");
    old.execution.validate();
    let src = include_str!("../daemon/executor/mod.rs");
    assert!(
        !src.contains("execution.backend"),
        "the executor must not branch on the backend value"
    );
}

#[test]
fn execution_config_parses_from_toml() {
    let with_pty: Config = toml::from_str("[execution]\nbackend = \"pty\"\n").unwrap();
    assert_eq!(with_pty.execution.backend, "pty");

    let empty: Config = toml::from_str("").unwrap();
    assert_eq!(empty.execution.backend, "pty");
}

#[test]
fn execution_validate_does_not_panic() {
    for backend in ["tmux", "pty", "nonsense"] {
        ExecutionConfig {
            backend: backend.to_string(),
        }
        .validate(); // must not panic
    }
}

#[test]
fn shells_config_defaults_to_40_by_120() {
    let empty: Config = toml::from_str("[shells]\n").unwrap();
    assert_eq!(empty.shells.rows, 40, "rows must default to 40");
    assert_eq!(empty.shells.cols, 120, "cols must default to 120");
    assert_eq!(empty.shells.size(), (40, 120));

    let sized: Config = toml::from_str("[shells]\nrows = 50\ncols = 200\n").unwrap();
    assert_eq!((sized.shells.rows, sized.shells.cols), (50, 200));

    let shells = ShellsConfig::default();
    assert_eq!(shells.size(), (40, 120), "default().size() is (40, 120)");
}

#[test]
fn shells_size_clamps_nonsense() {
    let zero: Config = toml::from_str("[shells]\nrows = 0\ncols = 0\n").unwrap();
    assert_eq!(zero.shells.size(), (10, 40), "0 clamps to the floor");
    let huge: Config = toml::from_str("[shells]\nrows = 9999\ncols = 9999\n").unwrap();
    assert_eq!(huge.shells.size(), (200, 500), "9999 clamps to the ceiling");
    let sane: Config = toml::from_str("[shells]\nrows = 50\ncols = 200\n").unwrap();
    assert_eq!(sane.shells.size(), (50, 200), "a sane size passes through");
}

#[test]
fn shells_config_defaults() {
    let shells = ShellsConfig::default();
    assert_eq!(shells.max_per_owner, 5);
    assert_eq!(shells.exited_retention_secs, 300);
    assert_eq!(shells.log_retention_days, 7);
    assert_eq!(shells.scrollback_lines, 5000);
}

#[test]
fn detach_close_secs_defaults_to_60_and_zero_is_allowed() {
    let no_shells_table: Config = toml::from_str("[models.default]\n").unwrap();
    assert_eq!(
        no_shells_table.shells.detach_close_secs, 60,
        "a config with no [shells] table defaults detach_close_secs to 60"
    );
    let zero: Config = toml::from_str("[shells]\ndetach_close_secs = 0\n").unwrap();
    assert_eq!(
        zero.shells.detach_close_secs, 0,
        "detach_close_secs = 0 parses as 0 (never close on detach)"
    );
    let set: Config = toml::from_str("[shells]\ndetach_close_secs = 42\n").unwrap();
    assert_eq!(
        set.shells.detach_close_secs, 42,
        "an explicit value passes through"
    );
}

#[test]
fn idle_reap_secs_defaults_to_3600_and_zero_is_allowed() {
    let no_shells_table: Config = toml::from_str("[models.default]\n").unwrap();
    assert_eq!(
        no_shells_table.shells.idle_reap_secs, 3600,
        "a config with no [shells] table defaults idle_reap_secs to 3600"
    );
    let zero: Config = toml::from_str("[shells]\nidle_reap_secs = 0\n").unwrap();
    assert_eq!(
        zero.shells.idle_reap_secs, 0,
        "idle_reap_secs = 0 parses as 0 (never reap)"
    );
    let set: Config = toml::from_str("[shells]\nidle_reap_secs = 42\n").unwrap();
    assert_eq!(
        set.shells.idle_reap_secs, 42,
        "an explicit value passes through"
    );
}

#[test]
fn shells_config_parses_from_toml() {
    let all: Config = toml::from_str(
        "[shells]\nmax_per_owner = 12\nexited_retention_secs = 999\n\
             log_retention_days = 30\nscrollback_lines = 8000\n",
    )
    .unwrap();
    assert_eq!(all.shells.max_per_owner, 12);
    assert_eq!(all.shells.exited_retention_secs, 999);
    assert_eq!(all.shells.log_retention_days, 30);
    assert_eq!(all.shells.scrollback_lines, 8000);

    let partial: Config = toml::from_str("[shells]\nmax_per_owner = 12\n").unwrap();
    assert_eq!(partial.shells.max_per_owner, 12);
    assert_eq!(
        partial.shells.exited_retention_secs, 300,
        "exited_retention_secs must still default"
    );
    assert_eq!(
        partial.shells.log_retention_days, 7,
        "log_retention_days must still default"
    );
    assert_eq!(
        partial.shells.scrollback_lines, 5000,
        "scrollback_lines must still default"
    );
}

#[test]
fn shell_paths_are_under_the_runtime_tree() {
    let _guard = crate::test_home_guard();
    let shells = shells_dir();
    let logs = shell_logs_dir();
    let base = config_dir();
    assert!(
        shells.ends_with("var/run/shells"),
        "shells_dir() = {shells:?}"
    );
    assert!(
        logs.ends_with("var/log/shells"),
        "shell_logs_dir() = {logs:?}"
    );
    assert!(
        shells.starts_with(&base),
        "{shells:?} must start with {base:?}"
    );
    assert!(logs.starts_with(&base), "{logs:?} must start with {base:?}");
}

// ── [hosts] ──────────────────────────────────────────────────────────────

#[test]
fn hosts_default_to_empty() {
    let cfg = Config::default();
    assert!(
        cfg.hosts.is_empty(),
        "no [hosts] section must mean no hosts, got {:?}",
        cfg.hosts
    );
    let parsed: Config = toml::from_str("[models.default]\nprovider = \"ollama\"\n").unwrap();
    assert!(
        parsed.hosts.is_empty(),
        "a config with no [hosts] section parses and stays empty"
    );
}

#[test]
fn hosts_parse_from_toml() {
    let cfg: Config = toml::from_str(
        "[hosts.web01]\n\
             ssh = \"matt@web01.lan\"\n\
             tags = [\"web\", \"prod\"]\n\
             \n\
             [hosts.db01]\n\
             ssh = \"db01\"\n\
             sudo_scripts = [\"pg-failover\"]\n",
    )
    .unwrap();
    assert_eq!(cfg.hosts.len(), 2);
    let web01 = cfg.hosts.get("web01").expect("web01 must parse");
    assert_eq!(web01.ssh, "matt@web01.lan");
    assert_eq!(web01.tags, vec!["web".to_string(), "prod".to_string()]);
    let db01 = cfg.hosts.get("db01").expect("db01 must parse");
    assert_eq!(db01.ssh, "db01");
    assert_eq!(db01.sudo_scripts, vec!["pg-failover".to_string()]);
    assert!(db01.tags.is_empty(), "unset tags must default to empty");
    assert_eq!(
        db01.sandbox_profile, None,
        "unset sandbox_profile must default to None"
    );
}

#[test]
fn validate_hosts_reports_empty_ssh_and_reserved_local() {
    let cfg: Config = toml::from_str(
        "[models.default]\n\
             provider = \"ollama\"\n\
             \n\
             [hosts.nokey]\n\
             \n\
             [hosts.local]\n\
             ssh = \"matt@localhost\"\n",
    )
    .unwrap();
    let problems = cfg.validate_hosts();
    assert_eq!(
        problems.len(),
        2,
        "exactly two problems expected (empty ssh, reserved local), got {problems:?}"
    );
    assert!(
        problems.iter().any(|p| p.contains("nokey")),
        "the empty-ssh problem must name its host: {problems:?}"
    );
    assert!(
        problems.iter().any(|p| p.contains("local")),
        "the reserved-key problem must name its host: {problems:?}"
    );
}

#[test]
fn validate_hosts_rejects_a_name_host_id_parse_rejects() {
    let cfg: Config = toml::from_str(
        "[models.default]\n\
             provider = \"ollama\"\n\
             \n\
             [hosts.\"../etc\"]\n\
             ssh = \"matt@escape\"\n",
    )
    .unwrap();
    let problems = cfg.validate_hosts();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].contains(".."),
        "the problem must echo parse's rejected name: {problems:?}"
    );
}

#[test]
fn validate_hosts_stays_silent_on_clean_entries() {
    let cfg: Config = toml::from_str(
        "[models.default]\n\
             provider = \"ollama\"\n\
             \n\
             [hosts.web01]\n\
             ssh = \"matt@web01.lan\"\n",
    )
    .unwrap();
    assert!(
        cfg.validate_hosts().is_empty(),
        "a clean entry yields no problems"
    );
}

// ── M37 phase 06: the self-host load warning ────────────────────────────────

/// One entry whose `ssh` names the daemon host produces exactly one problem,
/// naming the `[hosts.<name>]` key, the `ssh` value and the hostname; a
/// genuinely remote entry alongside it stays silent.
#[test]
fn self_host_problems_names_the_entry_that_is_this_machine() {
    let cfg: Config = toml::from_str(
        "[models.default]\n\
             provider = \"ollama\"\n\
             \n\
             [hosts.selfish]\n\
             ssh = \"pinky\"\n\
             \n\
             [hosts.remote]\n\
             ssh = \"zoidberg\"\n",
    )
    .unwrap();
    let problems = cfg.self_host_problems("pinky");
    assert_eq!(
        problems.len(),
        1,
        "exactly one self-host problem expected, got {problems:?}"
    );
    let p = &problems[0];
    assert!(p.contains("[hosts.selfish]"), "names the entry: {p}");
    assert!(p.contains("pinky"), "names the ssh value and the host: {p}");
    assert!(
        p.contains("will be refused"),
        "tells the operator the consequence: {p}"
    );
}

/// No entry naming the daemon host: no warning at all.
#[test]
fn self_host_problems_is_empty_when_no_entry_is_this_machine() {
    let cfg: Config = toml::from_str(
        "[models.default]\n\
             provider = \"ollama\"\n\
             \n\
             [hosts.web01]\n\
             ssh = \"matt@web01.lan\"\n\
             \n\
             [hosts.gpu]\n\
             ssh = \"zoidberg\"\n",
    )
    .unwrap();
    assert!(
        cfg.self_host_problems("pinky").is_empty(),
        "remote-only hosts yield no self-host problems"
    );
}

/// The full hostname and its first dot-separated label are both caught, any
/// case, and so are the loopback spellings — the same predicate the gate uses.
#[test]
fn self_host_problems_catches_the_label_and_loopback_spellings() {
    let cfg: Config = toml::from_str(
        "[hosts.a]\nssh = \"matt@PINKY.example.org\"\n\
             [hosts.b]\nssh = \"ssh://matt@pinky:2222\"\n\
             [hosts.c]\nssh = \"localhost\"\n\
             [hosts.d]\nssh = \"[::1]\"\n",
    )
    .unwrap();
    let problems = cfg.self_host_problems("pinky.example.org");
    assert_eq!(problems.len(), 4, "all four entries are self: {problems:?}");
    assert!(
        cfg.self_host_problems("zoidberg").len() == 2,
        "against another hostname only the loopback entries (c, d) remain: {:?}",
        cfg.self_host_problems("zoidberg")
    );
}

// ── M38 phase 07a: [[notifications.route]] ──────────────────────────────

/// The switch defaults on, from an empty file and from `Config::default()`.
#[test]
fn notifications_default_routes_is_on_by_default() {
    let parsed: Config = toml::from_str("").unwrap();
    assert!(
        parsed.notifications.default_routes,
        "an empty file parses default_routes to true"
    );
    assert!(
        Config::default().notifications.default_routes,
        "Config::default() has default_routes on"
    );
    assert!(
        parsed.notifications.route.is_empty(),
        "an empty file has no routes"
    );
}

/// Two `[[notifications.route]]` tables parse into filters carrying
/// min_severity, sources and names, with the right sinks.
#[test]
fn route_tables_parse_into_filters() {
    let toml = r#"
            [[notifications.route]]
            sink = "command"
            command = "echo hi"
            min_severity = "warning"
            sources = ["scheduler", "watchdog"]
            names = ["nightly-backup"]

            [[notifications.route]]
            sink = "history"
            sources = ["shell"]
        "#;
    let cfg: Config = toml::from_str(toml).unwrap();
    assert_eq!(cfg.notifications.route.len(), 2);

    let first = &cfg.notifications.route[0];
    assert_eq!(first.sink, crate::config::RouteSink::Command);
    assert_eq!(first.command.as_deref(), Some("echo hi"));
    let f = first.filter();
    assert_eq!(f.min_severity, Some(crate::bus::Severity::Warning));
    assert_eq!(
        f.sources,
        vec![crate::bus::Source::Scheduler, crate::bus::Source::Watchdog]
    );
    assert_eq!(f.names, vec!["nightly-backup".to_string()]);

    let second = &cfg.notifications.route[1];
    assert_eq!(second.sink, crate::config::RouteSink::History);
    assert_eq!(
        second.filter().sources,
        vec![crate::bus::Source::Shell],
        "the second route's filter carries its source"
    );
}

/// A typo'd key inside a route table is refused at parse, not ignored.
#[test]
fn route_with_an_unknown_key_is_refused() {
    let toml = r#"
            [[notifications.route]]
            sink = "history"
            severity = "warning"
        "#;
    let path = std::path::Path::new("config.toml");
    let err = parse_config_text(path, toml).expect_err("an unknown key must fail");
    let text = format!("{err:#}");
    assert!(
        text.contains("unknown field `severity`"),
        "the error names the unknown key: {text}"
    );
}

/// A config that still sets the removed `[webhook] severity_threshold` fails
/// to load, and the error shows the equivalent route table.
#[test]
fn removed_severity_threshold_is_a_load_error() {
    let toml = r#"
            [webhook]
            severity_threshold = "critical"
        "#;
    let path = std::path::Path::new("config.toml");
    let err = parse_config_text(path, toml).expect_err("a removed key must fail");
    let text = format!("{err:#}");
    assert!(
        text.contains("removed in 2.10.0"),
        "the error names the removal: {text}"
    );
    assert!(
        text.contains("[[notifications.route]]")
            && text.contains("sources = [\"webhook\"]")
            && text.contains("min_severity = \"critical\""),
        "the error shows the equivalent route: {text}"
    );
}

/// A config that still sets the removed `[notifications] on_alert` fails to
/// load. The error shows the two routes that reproduce it, with a literal
/// placeholder — never the configured command, which may carry a secret.
#[test]
fn removed_on_alert_is_a_load_error() {
    let toml = r#"
            [notifications]
            on_alert = "curl -H 'Authorization: Bearer token-s3cr3t' https://hooks.example"
        "#;
    let path = std::path::Path::new("config.toml");
    let err = parse_config_text(path, toml).expect_err("a removed key must fail");
    let text = format!("{err:#}");
    assert!(
        text.contains("removed in 2.10.0"),
        "the error names the removal: {text}"
    );
    assert!(
        text.contains("sink = \"command\"")
            && text.contains("[\"scheduler\", \"watchdog\"]")
            && text.contains("<your on_alert command>"),
        "the error shows the two reproducing routes: {text}"
    );
    assert!(
        !text.contains("token-s3cr3t"),
        "the configured command is never echoed: {text}"
    );
}

/// A config using only the surviving keys parses.
#[test]
fn config_without_removed_keys_still_loads() {
    let toml = r#"
            [webhook]
            enabled = true

            [notifications]
            default_routes = false

            [[notifications.route]]
            sink = "command"
            command = "echo hi"
            sources = ["watchdog"]
        "#;
    let path = std::path::Path::new("config.toml");
    let cfg = parse_config_text(path, toml).expect("the surviving keys parse");
    assert!(cfg.webhook.enabled);
    assert!(!cfg.notifications.default_routes);
    assert_eq!(cfg.notifications.route.len(), 1);
}

/// A `command` sink route with no command — missing and blank — is refused,
/// and the error names the route's position.
#[test]
fn command_route_without_a_command_is_refused() {
    let missing = r#"
            [[notifications.route]]
            sink = "command"
        "#;
    let path = std::path::Path::new("config.toml");
    let err = parse_config_text(path, missing).expect_err("a missing command must fail");
    assert!(
        err.to_string().contains("#1"),
        "the error names route #1: {err:#}"
    );

    let blank = r#"
            [[notifications.route]]
            sink = "command"
            command = "   "
        "#;
    let err = parse_config_text(path, blank).expect_err("a blank command must fail");
    assert!(
        err.to_string().contains("#1"),
        "the blank command also names route #1: {err:#}"
    );
}

/// A `command` on a `history` or `chat_notice` route is refused.
#[test]
fn command_on_a_non_command_route_is_refused() {
    let path = std::path::Path::new("config.toml");
    for sink in ["history", "chat_notice"] {
        let toml = format!(
            r#"
            [[notifications.route]]
            sink = "{sink}"
            command = "echo hi"
        "#
        );
        let err =
            parse_config_text(path, &toml).expect_err("a command on a non-command route must fail");
        assert!(
            err.to_string().contains("#1"),
            "the error names the route: {err:#}"
        );
    }
}

/// `sink = "analyze"` parses into `RouteSink::Analyze` and carries its
/// filter.
#[test]
fn analyze_route_parses() {
    let toml = r#"
            [[notifications.route]]
            sink = "analyze"
            names = ["HighCPU"]
        "#;
    let cfg: Config = toml::from_str(toml).unwrap();
    assert_eq!(cfg.notifications.route.len(), 1);
    assert_eq!(
        cfg.notifications.route[0].sink,
        crate::config::RouteSink::Analyze
    );
    assert_eq!(
        cfg.notifications.route[0].filter().names,
        vec!["HighCPU".to_string()]
    );
}

/// A `command` on an `analyze` route is refused, like on `history`.
#[test]
fn analyze_route_refuses_a_command() {
    let path = std::path::Path::new("config.toml");
    let toml = r#"
            [[notifications.route]]
            sink = "analyze"
            command = "echo hi"
        "#;
    let err = parse_config_text(path, toml).expect_err("a command on an analyze route must fail");
    assert!(
        err.to_string().contains("#1"),
        "the error names the route: {err:#}"
    );
}

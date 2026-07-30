//! Configuration: `~/.daemoneye/etc/config.toml` parsing, FHS path helpers,
//! and first-run asset seeding. Split across submodules in phase-09; the
//! public surface is re-exported here.

mod load;
mod path_audit;
mod seeds;
mod types;

pub use load::*;
pub use seeds::*;
pub use types::*;

#[cfg(test)]
mod tests {
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
        assert_eq!(limits.per_tool_cap("get_terminal_context"), None);
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
}

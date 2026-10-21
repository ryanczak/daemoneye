use crate::ai::types::TokenBreakdown;
use crate::config::Pricing;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Per-call cost breakdown in USD.
///
/// Each field represents the cost contribution of one token bucket
/// multiplied by its respective per-million-token rate.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Cost {
    /// Cost attributable to uncached input tokens.
    pub input_cost_usd: f64,
    /// Cost attributable to output (completion) tokens.
    pub output_cost_usd: f64,
    /// Cost attributable to cache-read tokens.
    pub cache_read_cost_usd: f64,
    /// Cost attributable to cache-write tokens.
    pub cache_write_cost_usd: f64,
    /// Sum of all four cost components.
    pub total_cost_usd: f64,
}

/// Compute the per-call cost from token counts and pricing rates.
///
/// Each bucket is billed independently:
/// `input_tokens * input_per_mtok / 1_000_000`, etc.
/// When `pricing.source == Unknown` all costs are zero.
pub fn compute_cost(tokens: &TokenBreakdown, pricing: &Pricing) -> Cost {
    let mtok = 1_000_000.0;
    let input_cost = (tokens.input_tokens as f64) * pricing.input_per_mtok / mtok;
    let output_cost = (tokens.output_tokens as f64) * pricing.output_per_mtok / mtok;
    let cache_read_cost = (tokens.cache_read_tokens as f64) * pricing.cache_read_per_mtok / mtok;
    let cache_write_cost = (tokens.cache_write_tokens as f64) * pricing.cache_write_per_mtok / mtok;
    let total = input_cost + output_cost + cache_read_cost + cache_write_cost;
    Cost {
        input_cost_usd: input_cost,
        output_cost_usd: output_cost,
        cache_read_cost_usd: cache_read_cost,
        cache_write_cost_usd: cache_write_cost,
        total_cost_usd: total,
    }
}

/// Attribution context for a single AI call.
///
/// Populated by the caller of `run_conversation_loop` and threaded through
/// to the cost record emitted at `AiEvent::Done`.
#[derive(Debug, Clone)]
pub struct CostAttribution {
    /// Agent identity: "chat" for interactive sessions, the agent name for
    /// `/agent switch` or named ghost shells, "ghost-anonymous" for unnamed ghosts.
    pub agent_name: String,
    /// True when this call belongs to a ghost shell session.
    pub is_ghost: bool,
    /// Job ID of the parent ghost that spawned this one (if any).
    pub parent_job_id: Option<String>,
}

impl CostAttribution {
    /// Build attribution from a ghost shell config.
    ///
    /// When `ghost_config` is `None` (should not happen in the ghost path),
    /// falls back to `"ghost-anonymous"` with no parent.
    pub fn from_ghost_config(ghost_config: Option<&crate::ipc::GhostConfig>) -> Self {
        let Some(gc) = ghost_config else {
            return CostAttribution {
                agent_name: "ghost-anonymous".to_string(),
                is_ghost: true,
                parent_job_id: None,
            };
        };
        CostAttribution {
            agent_name: gc
                .agent
                .clone()
                .unwrap_or_else(|| "ghost-anonymous".to_string()),
            is_ghost: true,
            parent_job_id: gc.parent_job_id.clone(),
        }
    }
}

/// Full cost record written to `events.jsonl` after each AI completion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostRecord {
    /// UTC timestamp of the AI completion.
    pub timestamp: DateTime<Utc>,
    /// Session identifier (ephemeral session ID or named session).
    pub session_id: String,
    /// Agent identity (see `CostAttribution::agent_name`).
    pub agent_name: String,
    /// True when this call belongs to a ghost shell.
    pub is_ghost: bool,
    /// Parent ghost job ID, if this is a spawned child ghost.
    pub parent_job_id: Option<String>,
    /// AI provider name (anthropic, openai, gemini, ollama, lmstudio).
    pub provider: String,
    /// Model identifier sent to the API.
    pub model: String,
    /// Four-bucket token breakdown from the AI response.
    pub tokens: TokenBreakdown,
    /// Computed cost in USD.
    pub cost: Cost,
    /// Where the pricing rates came from.
    pub pricing_source: crate::config::PricingSource,
    /// The run id whose loop made this call — the join to the run log.
    /// `""` on lines written before M30 phase 01.
    #[serde(default)]
    pub run_id: String,
}

/// Emit the one `ai_cost` event a completed model call produces.
///
/// The single cost site for every model call: resolves pricing the way the
/// ghost's cost site does (`model_entry.pricing()`, falling back to the
/// `Unknown` source at zero rates), builds the [`CostRecord`], appends it
/// with `log_event("ai_cost", ..)`, adds its total to the daily spend
/// ledger, and returns the record so the caller can keep feeding
/// `RunMetrics` and the session totals.
///
/// A serialization failure is logged and the `ai_cost` line skipped — never
/// a panic — but the ledger still counts the cost, so the daily cap's
/// picture of spend stays honest.
///
/// One model call's cost context: everything [`emit_cost`] needs beyond the
/// token breakdown. Built once per call by the caller.
pub struct CostCall<'a> {
    /// The resolved model entry whose rates price the call.
    pub model_entry: &'a crate::config::ModelEntry,
    /// The token buckets the call consumed.
    pub usage: &'a TokenBreakdown,
    /// Who made the call.
    pub attribution: &'a CostAttribution,
    /// The session id the call belongs to; `"-"` when there is none.
    pub session_id: &'a str,
    /// The run id of the loop that made the call; a side call passes
    /// `""` — but `RunId` cannot be empty, so the field is the raw string.
    pub run_id: &'a str,
    /// When the call completed.
    pub now: DateTime<Utc>,
}

pub fn emit_cost(call: CostCall<'_>) -> CostRecord {
    let pricing = call.model_entry.pricing().unwrap_or(Pricing {
        input_per_mtok: 0.0,
        output_per_mtok: 0.0,
        cache_read_per_mtok: 0.0,
        cache_write_per_mtok: 0.0,
        source: crate::config::PricingSource::Unknown,
    });
    let cost = compute_cost(call.usage, &pricing);
    let record = CostRecord {
        timestamp: call.now,
        session_id: call.session_id.to_string(),
        agent_name: call.attribution.agent_name.clone(),
        is_ghost: call.attribution.is_ghost,
        parent_job_id: call.attribution.parent_job_id.clone(),
        provider: call.model_entry.provider.clone(),
        model: call.model_entry.model.clone(),
        tokens: call.usage.clone(),
        cost: cost.clone(),
        pricing_source: pricing.source,
        run_id: call.run_id.to_string(),
    };
    // The write and the ledger add must land under the ledger's lock, with
    // any seed running *before* the write: the daily-cap gate reads
    // `spent_today()` between model calls, and a gate read landing between
    // a cold seed and the write would count the line, then the add would
    // count it again. `line_written` carries the write's outcome into the
    // ledger — a failed write still counts its cost, because the line the
    // seed would sum is not on disk.
    let line = match serde_json::to_value(&record) {
        Ok(value) => Some(value),
        Err(e) => {
            log::warn!(
                "ai_cost serialization failed for agent '{}' ({}): {} — the line is skipped, the cost still counts",
                call.attribution.agent_name,
                call.model_entry.model,
                e
            );
            None
        }
    };
    crate::daemon::spend::record_under_lock(line.is_some(), record.cost.total_cost_usd, || {
        if let Some(line) = &line {
            crate::daemon::utils::log_event("ai_cost", line.clone());
        }
    });
    record
}

/// Attribution for one of the five side model calls (briefing, digest,
/// auto-name, the two watchdogs): they belong to no chat session and no run
/// loop, so the record carries `session_id = "-"` and `run_id = ""`.
pub fn side_call_attribution(agent_name: &str) -> CostAttribution {
    CostAttribution {
        agent_name: agent_name.to_string(),
        is_ghost: false,
        parent_job_id: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PricingSource;

    fn pricing(input: f64, output: f64, cache_read: f64, cache_write: f64) -> Pricing {
        Pricing {
            input_per_mtok: input,
            output_per_mtok: output,
            cache_read_per_mtok: cache_read,
            cache_write_per_mtok: cache_write,
            source: PricingSource::UserConfig,
        }
    }

    #[test]
    fn compute_cost_zero_tokens_is_zero() {
        let tokens = TokenBreakdown::default();
        let p = pricing(3.0, 15.0, 0.30, 3.75);
        let cost = compute_cost(&tokens, &p);
        assert_eq!(cost.total_cost_usd, 0.0);
        assert_eq!(cost.input_cost_usd, 0.0);
        assert_eq!(cost.output_cost_usd, 0.0);
        assert_eq!(cost.cache_read_cost_usd, 0.0);
        assert_eq!(cost.cache_write_cost_usd, 0.0);
    }

    #[test]
    fn compute_cost_basic_input_output() {
        let tokens = TokenBreakdown {
            input_tokens: 1000,
            output_tokens: 500,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        };
        let p = pricing(3.0, 15.0, 0.30, 3.75);
        let cost = compute_cost(&tokens, &p);
        // input: 1000 * 3.0 / 1_000_000 = 0.003
        // output: 500 * 15.0 / 1_000_000 = 0.0075
        assert!((cost.input_cost_usd - 0.003).abs() < 1e-10);
        assert!((cost.output_cost_usd - 0.0075).abs() < 1e-10);
        assert!((cost.total_cost_usd - 0.0105).abs() < 1e-10);
    }

    #[test]
    fn compute_cost_with_cache_read_uses_cache_rate() {
        let tokens = TokenBreakdown {
            input_tokens: 500,
            output_tokens: 200,
            cache_read_tokens: 800,
            cache_write_tokens: 0,
        };
        let p = pricing(3.0, 15.0, 0.30, 3.75);
        let cost = compute_cost(&tokens, &p);
        // cache_read: 800 * 0.30 / 1_000_000 = 0.00024
        assert!((cost.cache_read_cost_usd - 0.00024).abs() < 1e-10);
    }

    #[test]
    fn compute_cost_with_cache_write_uses_cache_write_rate() {
        let tokens = TokenBreakdown {
            input_tokens: 500,
            output_tokens: 200,
            cache_read_tokens: 0,
            cache_write_tokens: 200,
        };
        let p = pricing(3.0, 15.0, 0.30, 3.75);
        let cost = compute_cost(&tokens, &p);
        // cache_write: 200 * 3.75 / 1_000_000 = 0.00075
        assert!((cost.cache_write_cost_usd - 0.00075).abs() < 1e-10);
    }

    #[test]
    fn compute_cost_anthropic_sonnet_4_6_sample_turn() {
        // Realistic Anthropic Sonnet 4.6 turn:
        // 12k input (uncached), 4k output, 8k cache read, 2k cache write
        let tokens = TokenBreakdown {
            input_tokens: 12_000,
            output_tokens: 4_000,
            cache_read_tokens: 8_000,
            cache_write_tokens: 2_000,
        };
        // Sonnet 4.6 rates: $3.00 input, $15.00 output, $0.30 cache read, $3.75 cache write
        let p = pricing(3.00, 15.00, 0.30, 3.75);
        let cost = compute_cost(&tokens, &p);
        // input: 12000 * 3.00 / 1e6 = 0.036
        // output: 4000 * 15.00 / 1e6 = 0.06
        // cache_read: 8000 * 0.30 / 1e6 = 0.0024
        // cache_write: 2000 * 3.75 / 1e6 = 0.0075
        // total: 0.036 + 0.06 + 0.0024 + 0.0075 = 0.1059
        assert!((cost.input_cost_usd - 0.036).abs() < 1e-10);
        assert!((cost.output_cost_usd - 0.06).abs() < 1e-10);
        assert!((cost.cache_read_cost_usd - 0.0024).abs() < 1e-10);
        assert!((cost.cache_write_cost_usd - 0.0075).abs() < 1e-10);
        assert!((cost.total_cost_usd - 0.1059).abs() < 1e-10);
    }

    #[test]
    fn unknown_pricing_emits_zero_cost_record_with_flag() {
        let tokens = TokenBreakdown {
            input_tokens: 1000,
            output_tokens: 500,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        };
        let p = Pricing {
            input_per_mtok: 0.0,
            output_per_mtok: 0.0,
            cache_read_per_mtok: 0.0,
            cache_write_per_mtok: 0.0,
            source: PricingSource::Unknown,
        };
        let cost = compute_cost(&tokens, &p);
        assert_eq!(cost.total_cost_usd, 0.0);
        assert_eq!(p.source, PricingSource::Unknown);
    }

    #[test]
    fn ghost_unnamed_attributes_to_ghost_anonymous() {
        let attribution = CostAttribution {
            agent_name: "ghost-anonymous".to_string(),
            is_ghost: true,
            parent_job_id: None,
        };
        assert_eq!(attribution.agent_name, "ghost-anonymous");
        assert!(attribution.is_ghost);
        assert!(attribution.parent_job_id.is_none());
    }

    #[test]
    fn ghost_named_attributes_to_agent_name() {
        let attribution = CostAttribution {
            agent_name: "architect".to_string(),
            is_ghost: true,
            parent_job_id: Some("ghost-parent-001".to_string()),
        };
        assert_eq!(attribution.agent_name, "architect");
        assert!(attribution.is_ghost);
        assert_eq!(
            attribution.parent_job_id,
            Some("ghost-parent-001".to_string())
        );
    }

    #[test]
    fn cost_record_serializes_to_events_jsonl_round_trip() {
        use crate::config::PricingSource;

        let record = CostRecord {
            timestamp: DateTime::parse_from_rfc3339("2026-05-16T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            session_id: "sess-abc123".to_string(),
            agent_name: "chat".to_string(),
            is_ghost: false,
            parent_job_id: None,
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            tokens: TokenBreakdown {
                input_tokens: 1000,
                output_tokens: 500,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
            },
            cost: Cost {
                input_cost_usd: 0.003,
                output_cost_usd: 0.0075,
                cache_read_cost_usd: 0.0,
                cache_write_cost_usd: 0.0,
                total_cost_usd: 0.0105,
            },
            pricing_source: PricingSource::UserConfig,
            run_id: "20261020153700-a1b2".to_string(),
        };

        let json = serde_json::to_string(&record).expect("serialize CostRecord");
        assert!(
            json.contains(r#""run_id":"20261020153700-a1b2""#),
            "the serialized line carries the run id: {json}"
        );
        let back: CostRecord = serde_json::from_str(&json).expect("deserialize CostRecord");
        assert_eq!(back.session_id, "sess-abc123");
        assert_eq!(back.agent_name, "chat");
        assert_eq!(back.provider, "anthropic");
        assert_eq!(back.model, "claude-sonnet-4-6");
        assert_eq!(back.tokens.input_tokens, 1000);
        assert!((back.cost.total_cost_usd - 0.0105).abs() < 1e-10);
        assert_eq!(back.pricing_source, PricingSource::UserConfig);
        assert_eq!(back.run_id, "20261020153700-a1b2", "the run id round-trips");
    }

    #[test]
    fn a_cost_record_carries_its_run_id() {
        let record = CostRecord {
            timestamp: Utc::now(),
            session_id: "sess-1".to_string(),
            agent_name: "chat".to_string(),
            is_ghost: false,
            parent_job_id: None,
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            tokens: TokenBreakdown::default(),
            cost: Cost::default(),
            pricing_source: PricingSource::UserConfig,
            run_id: "20261020153700-ffee".to_string(),
        };
        let value = serde_json::to_value(&record).expect("serialize CostRecord");
        assert_eq!(
            value["run_id"], "20261020153700-ffee",
            "the serialized JSON has run_id"
        );
    }

    #[test]
    fn a_cost_record_without_run_id_still_parses() {
        let older = r#"{
            "timestamp": "2026-05-16T12:00:00Z",
            "session_id": "sess-old",
            "agent_name": "chat",
            "is_ghost": false,
            "parent_job_id": null,
            "provider": "anthropic",
            "model": "claude-sonnet-4-6",
            "tokens": {"input_tokens": 1000, "output_tokens": 500,
                       "cache_read_tokens": 0, "cache_write_tokens": 0},
            "cost": {"input_cost_usd": 0.003, "output_cost_usd": 0.0075,
                     "cache_read_cost_usd": 0.0, "cache_write_cost_usd": 0.0,
                     "total_cost_usd": 0.0105},
            "pricing_source": "UserConfig"
        }"#;
        let record: CostRecord =
            serde_json::from_str(older).expect("an older line without run_id parses");
        assert_eq!(record.run_id, "", "an older line parses to an empty run_id");
    }
}

#[cfg(test)]
mod emit_tests {
    use super::*;
    use crate::daemon::spend;

    fn model_entry() -> crate::config::ModelEntry {
        crate::config::ModelEntry {
            provider: "anthropic".to_string(),
            api_key: String::new(),
            model: "claude-sonnet-4-6".to_string(),
            input_cost_per_mtok: Some(3.0),
            output_cost_per_mtok: Some(15.0),
            cache_read_cost_per_mtok: Some(0.3),
            cache_write_cost_per_mtok: Some(3.75),
            ..Default::default()
        }
    }

    #[test]
    fn emit_cost_logs_ai_cost_and_feeds_the_ledger() {
        let _guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };
        crate::ai::filter::init_masking(&[]);

        let before = spend::spent_today();
        let entry = model_entry();
        let usage = TokenBreakdown {
            input_tokens: 1000,
            output_tokens: 500,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        };
        // 1000 × 3.00 / 1M + 500 × 15.00 / 1M = 0.003 + 0.0075 = 0.0105
        let record = emit_cost(CostCall {
            model_entry: &entry,
            usage: &usage,
            attribution: &side_call_attribution("digest"),
            session_id: "-",
            run_id: "",
            now: Utc::now(),
        });
        let after = spend::spent_today();
        assert_eq!(record.agent_name, "digest", "the record names the agent");
        assert_eq!(record.session_id, "-", "a side call carries no session");
        assert_eq!(record.run_id, "", "a side call carries no run id");
        assert!(
            (record.cost.total_cost_usd - 0.0105).abs() < 1e-9,
            "the cost is computed from the model entry's pricing: {:?}",
            record.cost
        );
        assert!(
            (after - before - 0.0105).abs() < 1e-9,
            "the ledger grows by the call's cost: before={before} after={after}"
        );

        let path = crate::config::current_event_segment_path();
        let content = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!("the events segment at {} was written: {e}", path.display())
        });
        let lines: Vec<&str> = content
            .lines()
            .filter(|l| l.contains(r#""event":"ai_cost""#))
            .collect();
        assert_eq!(lines.len(), 1, "exactly one ai_cost line was appended");
        assert!(
            lines[0].contains(r#""agent_name":"digest""#),
            "the line names digest: {}",
            lines[0]
        );
        let logged: serde_json::Value =
            serde_json::from_str(lines[0]).expect("the logged line is valid JSON");
        assert!(
            (logged["cost"]["total_cost_usd"].as_f64().unwrap_or(0.0) - 0.0105).abs() < 1e-9,
            "the line carries the cost: {}",
            lines[0]
        );
    }
}

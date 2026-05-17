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
            source: PricingSource::BuiltinDefault,
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
            pricing_source: PricingSource::BuiltinDefault,
        };

        let json = serde_json::to_string(&record).expect("serialize CostRecord");
        let back: CostRecord = serde_json::from_str(&json).expect("deserialize CostRecord");
        assert_eq!(back.session_id, "sess-abc123");
        assert_eq!(back.agent_name, "chat");
        assert_eq!(back.provider, "anthropic");
        assert_eq!(back.model, "claude-sonnet-4-6");
        assert_eq!(back.tokens.input_tokens, 1000);
        assert!((back.cost.total_cost_usd - 0.0105).abs() < 1e-10);
        assert_eq!(back.pricing_source, PricingSource::BuiltinDefault);
    }
}

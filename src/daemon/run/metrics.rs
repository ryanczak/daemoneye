//! The per-run tally (M30 phase 01): what one run's chat or ghost loop did —
//! tool calls and their outcomes, parse failures, model calls, tokens and
//! spend. Pure data, no I/O; the run guard carries it so phase 02 can emit a
//! `run` event from it at run end.

use crate::ai::types::TokenBreakdown;

/// A per-run tally of what the run's loop did. Empty at `start`; the chat
/// loop, the ghost loop and the cost sites on each fill it as they go.
#[derive(Debug, Default, Clone)]
pub struct RunMetrics {
    /// Tool calls whose outcome came from `execute_tool_call` — calls the
    /// caps refused before dispatch are not counted.
    pub tool_calls: u32,
    /// Tool calls that did not fail.
    pub tool_successes: u32,
    /// Calls refused for malformed arguments (`PendingCall::InvalidArgs`).
    pub parse_failures: u32,
    /// Model calls completed with usage.
    pub model_calls: u32,
    /// Sums of each token bucket, saturating.
    pub tokens: TokenBreakdown,
    /// Sums of each call's total cost.
    pub cost_usd: f64,
    /// The most recent model call's provider.
    pub provider: Option<String>,
    /// The most recent model call's model.
    pub model: Option<String>,
}

impl RunMetrics {
    /// Record one completed model call: the count, every token bucket and
    /// the cost accumulate; `provider` and `model` are kept — the most
    /// recent call wins.
    pub fn record_model_call(
        &mut self,
        provider: impl Into<String>,
        model: impl Into<String>,
        tokens: &TokenBreakdown,
        cost_usd: f64,
    ) {
        self.model_calls = self.model_calls.saturating_add(1);
        self.tokens.input_tokens = self.tokens.input_tokens.saturating_add(tokens.input_tokens);
        self.tokens.output_tokens = self
            .tokens
            .output_tokens
            .saturating_add(tokens.output_tokens);
        self.tokens.cache_read_tokens = self
            .tokens
            .cache_read_tokens
            .saturating_add(tokens.cache_read_tokens);
        self.tokens.cache_write_tokens = self
            .tokens
            .cache_write_tokens
            .saturating_add(tokens.cache_write_tokens);
        self.cost_usd += cost_usd;
        self.provider = Some(provider.into());
        self.model = Some(model.into());
    }

    /// Successful tool calls over tool calls executed: `None` when no tool
    /// call was ever executed, so an empty run reads as "no rate" rather
    /// than a misleading 0.0 or NaN.
    pub fn tool_success_rate(&self) -> Option<f64> {
        if self.tool_calls == 0 {
            return None;
        }
        Some(self.tool_successes as f64 / self.tool_calls as f64)
    }

    /// Calls refused for malformed arguments over tool calls executed —
    /// the denominator is all tool calls, not just the parsed ones.
    pub fn parse_failure_rate(&self) -> Option<f64> {
        if self.tool_calls == 0 {
            return None;
        }
        Some(self.parse_failures as f64 / self.tool_calls as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(input: u32, output: u32, cache_read: u32, cache_write: u32) -> TokenBreakdown {
        TokenBreakdown {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: cache_read,
            cache_write_tokens: cache_write,
        }
    }

    #[test]
    fn empty_metrics_have_no_rates() {
        let m = RunMetrics::default();
        assert_eq!(m.tool_success_rate(), None, "no calls, no success rate");
        assert_eq!(
            m.parse_failure_rate(),
            None,
            "no calls, no parse-failure rate"
        );
        assert_eq!(
            m.tool_success_rate(),
            None,
            "an empty run must not read as a 0.0 rate"
        );
    }

    #[test]
    fn tool_success_rate_is_successes_over_calls() {
        let mut m = RunMetrics::default();
        for _ in 0..3 {
            m.tool_calls += 1;
            m.tool_successes += 1;
        }
        m.tool_calls += 1;
        assert_eq!(
            m.tool_success_rate(),
            Some(0.75),
            "3 successes over 4 calls is 0.75"
        );
    }

    #[test]
    fn parse_failure_rate_is_over_all_tool_calls() {
        let mut m = RunMetrics::default();
        for _ in 0..4 {
            m.tool_calls += 1;
        }
        m.parse_failures = 1;
        assert_eq!(
            m.parse_failure_rate(),
            Some(0.25),
            "1 parse failure over 4 executed calls is 0.25 — the denominator is all tool calls"
        );
    }

    #[test]
    fn model_calls_sum_tokens_and_cost() {
        let mut m = RunMetrics::default();
        m.record_model_call(
            "anthropic",
            "claude-sonnet-4-6",
            &tokens(100, 50, 10, 5),
            0.01,
        );
        m.record_model_call("gemini", "gemini-2.5-pro", &tokens(200, 60, 20, 6), 0.02);
        assert_eq!(m.model_calls, 2, "two calls, count two");
        assert_eq!(m.tokens.input_tokens, 300, "input buckets sum");
        assert_eq!(m.tokens.output_tokens, 110, "output buckets sum");
        assert_eq!(m.tokens.cache_read_tokens, 30, "cache-read buckets sum");
        assert_eq!(m.tokens.cache_write_tokens, 11, "cache-write buckets sum");
        assert!(
            (m.cost_usd - 0.03).abs() < 1e-10,
            "costs sum: {}",
            m.cost_usd
        );
        assert_eq!(
            m.model.as_deref(),
            Some("gemini-2.5-pro"),
            "the most recent call's model is kept"
        );
        assert_eq!(
            m.provider.as_deref(),
            Some("gemini"),
            "the most recent call's provider is kept"
        );
    }
}

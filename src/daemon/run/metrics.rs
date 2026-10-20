//! The per-run tally (M30 phase 01): what one run's chat or ghost loop did —
//! tool calls and their outcomes, parse failures, model calls, tokens and
//! spend. Pure data, no I/O; the run guard carries it so phase 02 can emit a
//! `run` event from it at run end.

use crate::ai::types::TokenBreakdown;

use super::Budget;

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
    /// Turns the loop ran — one per model request (chat) or per
    /// `TurnStarted` (ghost).
    pub turns: u32,
    /// Backend errors the loop saw: an `AiEvent::Error`, or the chat loop's
    /// give-up when the stream closed without completing after its retries.
    pub backend_errors: u32,
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
    /// Name the configured provider and model before the first model call,
    /// so a run whose first call fails still names its model.
    pub fn set_model(&mut self, provider: impl Into<String>, model: impl Into<String>) {
        self.provider = Some(provider.into());
        self.model = Some(model.into());
    }

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

    /// Record one turn of the run.
    pub fn record_turn(&mut self) {
        self.turns = self.turns.saturating_add(1);
    }

    /// Record one backend error.
    pub fn record_backend_error(&mut self) {
        self.backend_errors = self.backend_errors.saturating_add(1);
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

    /// Fold the tally into `budget`, so a run's `Budget` says what it spent:
    /// turns, tool calls, every token bucket, wall-clock seconds and USD as
    /// micro-USD, rounded. The limits `budget` already carries are kept —
    /// only the spent side is folded.
    pub fn fold_into_budget(&self, budget: &mut Budget, wall_clock_secs: u64) {
        for _ in 0..self.turns {
            budget.record_turn();
        }
        for _ in 0..self.tool_calls {
            budget.record_tool_call();
        }
        budget.record_tokens(self.tokens.total() as u64);
        budget.record_wall_clock_secs(wall_clock_secs);
        budget.record_usd_micros((self.cost_usd * 1_000_000.0).round() as u64);
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

    #[test]
    fn the_tally_folds_into_a_budget() {
        let mut m = RunMetrics::default();
        for _ in 0..3 {
            m.record_turn();
        }
        for _ in 0..4 {
            m.tool_calls += 1;
        }
        m.tokens = tokens(100, 50, 10, 5);
        m.cost_usd = 0.0125;
        let mut budget = Budget::from_limits(5, 10, 1_000_000, 600, 100_000);
        m.fold_into_budget(&mut budget, 42);
        assert_eq!(
            budget.spent(crate::daemon::run::BudgetDimension::Turns),
            3,
            "turns fold"
        );
        assert_eq!(
            budget.spent(crate::daemon::run::BudgetDimension::ToolCalls),
            4,
            "tool calls fold"
        );
        assert_eq!(
            budget.spent(crate::daemon::run::BudgetDimension::Tokens),
            165,
            "every token bucket folds"
        );
        assert_eq!(
            budget.spent(crate::daemon::run::BudgetDimension::WallClock),
            42,
            "wall-clock seconds fold"
        );
        assert_eq!(
            budget.spent(crate::daemon::run::BudgetDimension::Usd),
            12_500,
            "$0.0125 is 12 500 micro-USD, rounded"
        );
        assert_eq!(
            budget.remaining(crate::daemon::run::BudgetDimension::Usd),
            Some(87_500),
            "the limit side is kept, the spent side folded"
        );
    }

    #[test]
    fn a_tally_with_no_spend_folds_to_zeroes() {
        let mut budget = Budget::from_limits(2, 2, 2, 2, 2);
        RunMetrics::default().fold_into_budget(&mut budget, 0);
        for dim in crate::daemon::run::BUDGET_DIMENSION_ORDER {
            assert_eq!(
                budget.remaining(dim),
                Some(2),
                "an empty tally spends nothing in {dim:?}"
            );
        }
    }
}

//! The `Run` core types (M24 phase 01): identity, kind, state, budget, and
//! briefing bounds for the unified run/turn model (design § 2.3). Pure data —
//! no I/O, no clock, no callers yet (wiring is phases 02–08).

use anyhow::bail;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A canonical run id: exactly 16 lowercase hex characters — the same grammar
/// a session log filename carries, so `var/log/runs/<run-id>.jsonl` and
/// `var/log/sessions/<id>.jsonl` look alike on disk. The id becomes a path
/// segment, so `parse` is a security boundary: anything outside the grammar,
/// including path separators or traversal, is rejected.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct RunId(String);

/// The number of hex characters a run id carries; matches the session-id
/// grammar (`ipc_client::new_session_id`, 8 random bytes).
const RUN_ID_LEN: usize = 16;

impl RunId {
    fn is_valid(s: &str) -> bool {
        s.len() == RUN_ID_LEN
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }

    /// A freshly generated id: the low 64 bits of a random UUID rendered as
    /// 16 lowercase hex characters — the same span of characters a session
    /// id carries, drawn from the `uuid` crate's v4 generator.
    pub fn new() -> Self {
        let uuid = Uuid::new_v4();
        let bytes = uuid.as_bytes();
        let s: String = bytes[..8].iter().map(|b| format!("{b:02x}")).collect();
        RunId(s)
    }

    /// Validate against the grammar and build a `RunId`, or bail naming the
    /// offending value.
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        if !Self::is_valid(s) {
            bail!("Invalid run id: '{s}' — expected exactly {RUN_ID_LEN} lowercase hex characters");
        }
        Ok(RunId(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for RunId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for RunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What started this run. Serialised into the turn log (phase 02), so each
/// variant's wire name is spelled explicitly.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum RunKind {
    #[serde(rename = "Chat")]
    Chat { session: String },
    #[serde(rename = "Ghost")]
    Ghost { runbook: String, trigger: String },
    #[serde(rename = "Scheduled")]
    Scheduled { job: String },
    #[serde(rename = "Delegated")]
    Delegated { agent: String, parent: RunId },
}

impl RunKind {
    /// The short label for `daemoneye runs` listings and log lines.
    pub fn label(&self) -> &'static str {
        match self {
            RunKind::Chat { .. } => "chat",
            RunKind::Ghost { .. } => "ghost",
            RunKind::Scheduled { .. } => "scheduled",
            RunKind::Delegated { .. } => "delegated",
        }
    }
}

/// Bounded plain-data record of how a run ended (design § 2.3): goal,
/// criteria, working files, attempts, blocker, and budget left. The bounds
/// are the behavior — an over-long briefing cannot be built.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Briefing {
    /// The goal the run was given.
    pub goal: String,
    /// The condition that would make the run complete, phrased as checkable
    /// criteria.
    pub criteria: Option<String>,
    /// The files the run had in play, capped at [`MAX_BRIEFING_FILES`].
    pub working_files: Vec<String>,
    /// What the run tried, each attempt capped at
    /// [`MAX_BRIEFING_ATTEMPT_CHARS`].
    pub attempts: Vec<String>,
    /// What the run could not get past, when it did not complete.
    pub blocker: Option<String>,
    /// Which budget dimension ended the run, when one did.
    pub budget_left: Option<BudgetDimension>,
}

/// Working files a briefing may carry; extras are dropped at construction.
pub const MAX_BRIEFING_FILES: usize = 5;
/// Characters an attempt may carry; a longer attempt is truncated to this
/// many characters when the briefing is built.
pub const MAX_BRIEFING_ATTEMPT_CHARS: usize = 200;

impl Briefing {
    /// Build a briefing, enforcing the bounds: at most
    /// [`MAX_BRIEFING_FILES`] working files (extras dropped) and each attempt
    /// truncated to [`MAX_BRIEFING_ATTEMPT_CHARS`] characters.
    pub fn new(
        goal: impl Into<String>,
        criteria: Option<String>,
        working_files: Vec<String>,
        attempts: Vec<String>,
        blocker: Option<String>,
        budget_left: Option<BudgetDimension>,
    ) -> Self {
        Briefing {
            goal: goal.into(),
            criteria,
            working_files: working_files.into_iter().take(MAX_BRIEFING_FILES).collect(),
            attempts: attempts
                .into_iter()
                .map(|a| a.chars().take(MAX_BRIEFING_ATTEMPT_CHARS).collect())
                .collect(),
            blocker,
            budget_left,
        }
    }
}

/// Which budget dimension a run consumes.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetDimension {
    #[serde(rename = "Turns")]
    Turns,
    #[serde(rename = "ToolCalls")]
    ToolCalls,
    #[serde(rename = "Tokens")]
    Tokens,
    #[serde(rename = "WallClock")]
    WallClock,
    #[serde(rename = "Usd")]
    Usd,
}

/// Runtime consumption tracked against limits (design § 2.3), across turns,
/// tool calls, tokens, wall clock, and USD. Holds both the limits and what
/// has been spent; `0` in the limit means unlimited (the project's "set to 0
/// for no cap" idiom), so a dimension with `limit == 0` never reports
/// exhausted.
///
/// Elapsed wall-clock seconds are a parameter: the caller passes them in, so
/// this type stays free of a clock and tests stay deterministic.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    /// Turns permitted. `0` = unlimited.
    pub max_turns: u64,
    /// Tool calls permitted across the whole run. `0` = unlimited.
    pub max_tool_calls: u64,
    /// Model tokens permitted across the whole run. `0` = unlimited.
    pub max_tokens: u64,
    /// Wall-clock seconds permitted. `0` = unlimited.
    pub max_wall_clock_secs: u64,
    /// Micro-USD (millionths of a dollar) permitted, so the arithmetic stays
    /// integral. `0` = unlimited.
    pub max_usd_micros: u64,
    turns_spent: u64,
    tool_calls_spent: u64,
    tokens_spent: u64,
    wall_clock_secs_spent: u64,
    usd_micros_spent: u64,
}

/// The documented order dimensions are checked in — the first one that is out
/// is the one [`Budget::exhausted`] names.
pub const BUDGET_DIMENSION_ORDER: [BudgetDimension; 5] = [
    BudgetDimension::Turns,
    BudgetDimension::ToolCalls,
    BudgetDimension::Tokens,
    BudgetDimension::WallClock,
    BudgetDimension::Usd,
];

impl Budget {
    /// A budget with no limits in any dimension.
    pub fn unlimited() -> Self {
        Budget::from_limits(0, 0, 0, 0, 0)
    }

    /// A budget from five limits, in [`BUDGET_DIMENSION_ORDER`]. `0` on any
    /// dimension means unlimited. Spent amounts start at zero.
    pub fn from_limits(
        max_turns: u64,
        max_tool_calls: u64,
        max_tokens: u64,
        max_wall_clock_secs: u64,
        max_usd_micros: u64,
    ) -> Self {
        Budget {
            max_turns,
            max_tool_calls,
            max_tokens,
            max_wall_clock_secs,
            max_usd_micros,
            turns_spent: 0,
            tool_calls_spent: 0,
            tokens_spent: 0,
            wall_clock_secs_spent: 0,
            usd_micros_spent: 0,
        }
    }

    /// Record the consumption of one turn.
    pub fn record_turn(&mut self) {
        self.turns_spent = self.turns_spent.saturating_add(1);
    }

    /// Record the consumption of a tool call.
    pub fn record_tool_call(&mut self) {
        self.tool_calls_spent = self.tool_calls_spent.saturating_add(1);
    }

    /// Record token spend.
    pub fn record_tokens(&mut self, tokens: u64) {
        self.tokens_spent = self.tokens_spent.saturating_add(tokens);
    }

    /// Record wall-clock consumption. Elapsed seconds are supplied by the
    /// caller — no clock is read inside this type.
    pub fn record_wall_clock_secs(&mut self, secs: u64) {
        self.wall_clock_secs_spent = self.wall_clock_secs_spent.saturating_add(secs);
    }

    /// Record USD spend, in micro-USD.
    pub fn record_usd_micros(&mut self, micros: u64) {
        self.usd_micros_spent = self.usd_micros_spent.saturating_add(micros);
    }

    /// What remains in a dimension: `None` when unlimited (`limit == 0`),
    /// otherwise the limit minus what was spent, saturating at zero.
    pub fn remaining(&self, dim: BudgetDimension) -> Option<u64> {
        let (limit, spent) = self.limit_and_spent(dim);
        if limit == 0 {
            return None;
        }
        Some(limit.saturating_sub(spent))
    }

    /// The first dimension that is out, in [`BUDGET_DIMENSION_ORDER`], or
    /// `None` when every dimension still has room or is unlimited.
    pub fn exhausted(&self) -> Option<BudgetDimension> {
        BUDGET_DIMENSION_ORDER
            .iter()
            .copied()
            .find(|dim| self.is_exhausted(*dim))
    }

    fn is_exhausted(&self, dim: BudgetDimension) -> bool {
        self.remaining(dim) == Some(0)
    }

    fn limit_and_spent(&self, dim: BudgetDimension) -> (u64, u64) {
        match dim {
            BudgetDimension::Turns => (self.max_turns, self.turns_spent),
            BudgetDimension::ToolCalls => (self.max_tool_calls, self.tool_calls_spent),
            BudgetDimension::Tokens => (self.max_tokens, self.tokens_spent),
            BudgetDimension::WallClock => (self.max_wall_clock_secs, self.wall_clock_secs_spent),
            BudgetDimension::Usd => (self.max_usd_micros, self.usd_micros_spent),
        }
    }
}

/// How a run ended. Serialised into the turn log (phase 02) and into a
/// `RunResult` (phase 03); variant wire names are spelled explicitly.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum RunState {
    #[serde(rename = "Running")]
    Running,
    #[serde(rename = "Complete")]
    Complete,
    #[serde(rename = "HardFail")]
    HardFail(Briefing),
    #[serde(rename = "BudgetExceeded")]
    BudgetExceeded(Briefing),
    #[serde(rename = "Cancelled")]
    Cancelled { stage: String, turns: u32 },
}

impl RunState {
    /// Whether the run has finished. Only `Running` is not terminal.
    pub fn is_terminal(&self) -> bool {
        !matches!(self, RunState::Running)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_id_generates_a_valid_16_hex_id() {
        let id = RunId::new();
        assert_eq!(id.as_str().len(), 16, "id must be 16 chars: {id}");
        assert!(
            id.as_str()
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "id must be lowercase hex: {id}"
        );
        let parsed = RunId::parse(id.as_str()).map_err(|_| ());
        assert_eq!(
            parsed.map(|got| got.as_str().to_string()),
            Ok::<_, ()>(id.as_str().to_string())
        );
    }

    #[test]
    fn run_id_parse_rejects_path_traversal_and_separators() {
        for bad in [
            "../../etc/passwd",
            "a/b",
            "..\\windows",
            "2190b361c94e7a4\0",
        ] {
            assert!(RunId::parse(bad).is_err(), "must reject: {bad:?}");
        }
    }

    #[test]
    fn run_id_parse_rejects_wrong_length_case_and_non_hex() {
        assert!(RunId::parse("2190b361c94e7a4").is_err(), "15 chars");
        assert!(RunId::parse("2190b361c94e7a456").is_err(), "17 chars");
        assert!(
            RunId::parse("2190B361C94E7A45").is_err(),
            "uppercase letters are outside the grammar"
        );
        assert!(RunId::parse("").is_err(), "empty");
        assert!(
            RunId::parse("2190b361c94e7a4z").is_err(),
            "non-hex character"
        );
    }

    #[test]
    fn run_kind_labels_all_four_kinds() {
        let parent = RunId::parse("2190b361c94e7a45").expect("valid id");
        assert_eq!(
            RunKind::Chat {
                session: "s".into()
            }
            .label(),
            "chat"
        );
        assert_eq!(
            RunKind::Ghost {
                runbook: "r".into(),
                trigger: "t".into()
            }
            .label(),
            "ghost"
        );
        assert_eq!(RunKind::Scheduled { job: "j".into() }.label(), "scheduled");
        assert_eq!(
            RunKind::Delegated {
                agent: "a".into(),
                parent
            }
            .label(),
            "delegated"
        );
    }

    #[test]
    fn run_state_terminal_predicate_separates_running_from_finished() {
        let briefing = Briefing::new("g", None, vec![], vec![], None, None);
        assert!(!RunState::Running.is_terminal());
        assert!(RunState::Complete.is_terminal());
        assert!(RunState::HardFail(briefing.clone()).is_terminal());
        assert!(RunState::BudgetExceeded(briefing).is_terminal());
        assert!(
            RunState::Cancelled {
                stage: "model_call".into(),
                turns: 3
            }
            .is_terminal()
        );
    }

    #[test]
    fn budget_reports_the_first_exhausted_dimension() {
        let mut b = Budget::from_limits(2, 3, u64::MAX, 0, 0);
        b.record_turn();
        b.record_turn();
        // Turns is out; tool calls also out; tokens/wall-clock/USD unlimited.
        b.record_tool_call();
        b.record_tool_call();
        b.record_tool_call();
        assert_eq!(b.exhausted(), Some(BudgetDimension::Turns));
        assert_eq!(b.remaining(BudgetDimension::ToolCalls), Some(0));
    }

    #[test]
    fn budget_zero_limit_means_unlimited() {
        let mut b = Budget::from_limits(0, 0, 0, 0, 0);
        b.record_turn();
        b.record_tool_call();
        b.record_tokens(u64::MAX);
        b.record_wall_clock_secs(u64::MAX);
        b.record_usd_micros(u64::MAX);
        for dim in BUDGET_DIMENSION_ORDER {
            assert_eq!(
                b.remaining(dim),
                None,
                "limit 0 must be unlimited for {dim:?}"
            );
        }
        assert_eq!(b.exhausted(), None);
    }

    #[test]
    fn budget_remaining_saturates_at_zero() {
        let mut b = Budget::from_limits(1, 1, 100, 60, 5000);
        b.record_turn();
        b.record_turn();
        b.record_tool_call();
        b.record_tool_call();
        b.record_tokens(500);
        b.record_wall_clock_secs(120);
        b.record_usd_micros(9_999);
        assert_eq!(b.remaining(BudgetDimension::Turns), Some(0));
        assert_eq!(b.remaining(BudgetDimension::ToolCalls), Some(0));
        assert_eq!(b.remaining(BudgetDimension::Tokens), Some(0));
        assert_eq!(b.remaining(BudgetDimension::WallClock), Some(0));
        assert_eq!(b.remaining(BudgetDimension::Usd), Some(0));
    }

    #[test]
    fn briefing_truncates_working_files_to_five() {
        let files: Vec<String> = (0..6).map(|i| format!("file{i}.rs")).collect();
        let b = Briefing::new("goal", None, files, vec![], None, None);
        assert_eq!(b.working_files.len(), 5);
        assert_eq!(b.working_files[4], "file4.rs", "extras dropped, order kept");
    }

    #[test]
    fn briefing_truncates_each_attempt_to_200_chars() {
        let attempt = "x".repeat(500);
        let b = Briefing::new("goal", None, vec![], vec![attempt.clone()], None, None);
        assert_eq!(b.attempts, vec!["x".repeat(MAX_BRIEFING_ATTEMPT_CHARS)]);
        assert!(
            attempt.len() > MAX_BRIEFING_ATTEMPT_CHARS,
            "setup sanity: attempt was over-long"
        );
    }
}

mod log_read;
mod turn_log;
pub use log_read::{
    DEFAULT_RUN_LOG_LIMIT, MAX_RUN_LOG_LIMIT, clamp_run_log_limit, get_turn,
    run_log_input_rejected, run_log_search, run_log_tail,
};
pub use turn_log::{TurnLogEvent, TurnLogRecord, append_turn_event, read_turn_log, turn_log_file};

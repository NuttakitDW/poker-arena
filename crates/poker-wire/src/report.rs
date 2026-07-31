//! Machine-readable match documents.
//!
//! These are the JSON documents the arena CLI emits for programmatic
//! consumers — [`MatchReport`] once on stdout (`--output json`) and
//! [`ProgressReport`] as JSON lines on stderr during the match
//! (`--progress-json`). They are not bot messages; they are the contract
//! with whatever ranks, displays, or archives match results (a website, a
//! sweep script). Shapes are documented for non-Rust consumers in
//! `WIRE_PROTOCOL.md`; `schema_version` bumps on any breaking change.

use serde::{Deserialize, Serialize};

use crate::game::{BettingKind, Stakes};

/// Breaking-change counter for the report shapes.
pub const REPORT_SCHEMA_VERSION: u32 = 1;

/// The complete machine-readable result of one match.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchReport {
    pub schema_version: u32,
    pub game_id: String,
    /// The seed that reproduces this match exactly.
    pub seed: u64,
    pub dealing: String,
    pub decks: u64,
    pub hands: u64,
    pub seat_count: usize,
    pub starting_stack: u64,
    pub stakes: Stakes,
    pub betting: BettingKind,
    pub fault_policy: String,
    pub timeout_ms: Option<u64>,
    /// Bot name, when the match ended early by forfeit.
    pub forfeited_by: Option<String>,
    /// In the order bots were seated on the command line.
    pub bots: Vec<BotReport>,
}

/// One bot's results.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BotReport {
    pub name: String,
    pub hands: u64,
    pub total_chips: i64,
    /// Mean winnings per 100 hands, in **chips** — the canonical unit.
    /// Consumers normalize for display via `stakes`/`betting` (fixed
    /// limit: divide by the big bet; pot/no-limit: by the big blind).
    pub chips_per100_mean: f64,
    /// Two-sided 95% Student-t half-width of the mean, same scale;
    /// `null` with fewer than two observations.
    pub chips_per100_ci95: Option<f64>,
    /// Statistical observations behind the interval (hands in seeded mode,
    /// duplicate rotation-sets in duplicate mode).
    pub observations: u64,
    pub faults: u64,
    pub behavior: BehaviorReport,
}

/// Behavioral profile, all rates in `[0, 1]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BehaviorReport {
    pub vpip: f64,
    pub pfr: f64,
    /// Aggression factor (bets+raises)/calls; `null` when calls are zero
    /// but aggression isn't (JSON has no infinity).
    pub af: Option<f64>,
    pub wtsd: f64,
    pub wsd: f64,
    pub fold_rate: f64,
}

/// One interim-standings line: a live leaderboard snapshot with tightening
/// confidence intervals. Same field conventions as [`MatchReport`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgressReport {
    pub decks_done: u64,
    pub hands_done: u64,
    pub bots: Vec<BotProgress>,
}

/// One bot's interim standing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BotProgress {
    pub name: String,
    pub total_chips: i64,
    pub chips_per100_mean: f64,
    pub chips_per100_ci95: Option<f64>,
    pub observations: u64,
    pub faults: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_round_trip_through_json() {
        let report = MatchReport {
            schema_version: REPORT_SCHEMA_VERSION,
            game_id: "27td-fl".into(),
            seed: 9,
            dealing: "duplicate".into(),
            decks: 50,
            hands: 100,
            seat_count: 2,
            starting_stack: 10_000,
            stakes: Stakes::Blinds {
                small_blind: 50,
                big_blind: 100,
            },
            betting: BettingKind::FixedLimit { raise_cap: Some(4) },
            fault_policy: "check-fold".into(),
            timeout_ms: Some(1_000),
            forfeited_by: None,
            bots: vec![BotReport {
                name: "caller".into(),
                hands: 100,
                total_chips: 650,
                chips_per100_mean: 650.0,
                chips_per100_ci95: Some(8070.0),
                observations: 50,
                faults: 0,
                behavior: BehaviorReport {
                    vpip: 0.6,
                    pfr: 0.3,
                    af: None,
                    wtsd: 0.1,
                    wsd: 0.5,
                    fold_rate: 0.7,
                },
            }],
        };
        let json = serde_json::to_string(&report).unwrap();
        assert_eq!(serde_json::from_str::<MatchReport>(&json).unwrap(), report);

        let progress = ProgressReport {
            decks_done: 100,
            hands_done: 200,
            bots: vec![BotProgress {
                name: "caller".into(),
                total_chips: -4550,
                chips_per100_mean: -2275.0,
                chips_per100_ci95: Some(5310.0),
                observations: 100,
                faults: 0,
            }],
        };
        let json = serde_json::to_string(&progress).unwrap();
        assert_eq!(
            serde_json::from_str::<ProgressReport>(&json).unwrap(),
            progress
        );
    }
}

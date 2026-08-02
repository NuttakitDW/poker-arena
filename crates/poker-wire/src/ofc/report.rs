//! Machine-readable OFC match documents.
//!
//! Mirrors [`crate::report`] field-for-field where the same concept
//! applies, with points replacing chips: [`OfcMatchReport`] once on stdout
//! (`--output json`) and [`OfcProgressReport`] as JSON lines on stderr
//! during the match (`--progress-json`). Not bot messages; the contract
//! with whatever ranks, displays, or archives OFC results. OFC has no
//! dealing modes to record (no duplicate/deck-mirroring mode — fantasyland
//! state makes deck reuse incoherent) and no chip stakes, so those fields
//! from the betting report have no OFC counterpart.
//! [`OFC_REPORT_SCHEMA_VERSION`] bumps on any breaking change.

use serde::{Deserialize, Serialize};

/// Breaking-change counter for the OFC report shapes. Independent of
/// [`crate::report::REPORT_SCHEMA_VERSION`] — the two protocols version
/// separately.
pub const OFC_REPORT_SCHEMA_VERSION: u32 = 1;

/// The complete machine-readable result of one OFC match.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OfcMatchReport {
    pub schema_version: u32,
    /// Always `"ofc"` for this shape; lets a consumer dispatch between the
    /// two report schemas ([`OfcMatchReport`] and [`crate::report::MatchReport`])
    /// without a registry lookup.
    pub family: String,
    pub game_id: String,
    pub hands: u64,
    /// The seed that reproduces this match exactly.
    pub seed: u64,
    pub seat_count: usize,
    pub timeout_ms: Option<u64>,
    pub fault_policy: String,
    /// Bot name, when the match ended early by forfeit.
    pub forfeited_by: Option<String>,
    /// In the order bots were seated on the command line.
    pub bots: Vec<OfcBotReport>,
}

/// One bot's results.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OfcBotReport {
    pub name: String,
    /// Bot spec kind, e.g. `"builtin:greedy"` or `"cmd"`.
    pub kind: String,
    /// Total points won across the match, in **points** — the canonical
    /// unit (there are no chips to normalize by).
    pub points: i64,
    pub points_per_hand_mean: f64,
    /// Two-sided 95% Student-t half-width of the mean; `null` with fewer
    /// than two observations.
    pub points_per_hand_ci95: Option<f64>,
    pub hands: u64,
    pub fouls: u64,
    /// Hands played *in* fantasyland (entered or stayed).
    pub fantasylands: u64,
    /// Number of opponents scooped, summed across hands.
    pub scoops: u64,
    /// Total royalty points earned across the match.
    pub royalties: u64,
    pub faults: u64,
    /// Decisions this bot answered (`OfcBot::place` calls, `Ok` or `Err`
    /// alike). Wall-clock timing (`decision_ms_*`, below) is measured
    /// around the arena's call into the bot — for wire bots this includes
    /// transport round-trip, not pure think time. The `decision_ms_*`
    /// fields are the only part of a report that is not reproducible from
    /// `seed`: the same seed reproduces everything else in this document
    /// (this count included, given deterministic bots), but timing varies
    /// run to run. All five are `null` when the bot never decided.
    pub decisions: u64,
    /// Exact mean wall-clock ms per decision; `null` when the bot never
    /// decided.
    pub decision_ms_mean: Option<f64>,
    /// Approximate median wall-clock ms, from a log-scaled histogram (not
    /// the exact value — see `poker_arena::stat::DecisionStats::quantile`):
    /// relative error is bounded by that histogram's bucket ratio, about
    /// ±4.5%. `null` when the bot never decided.
    pub decision_ms_p50: Option<f64>,
    /// Approximate 90th-percentile wall-clock ms, same histogram and error
    /// bound as `decision_ms_p50`; `null` when the bot never decided.
    pub decision_ms_p90: Option<f64>,
    /// Approximate 99th-percentile wall-clock ms, same histogram and error
    /// bound as `decision_ms_p50`; `null` when the bot never decided.
    pub decision_ms_p99: Option<f64>,
    /// Exact max wall-clock ms across this bot's decisions; `null` when the
    /// bot never decided.
    pub decision_ms_max: Option<f64>,
}

/// One interim-standings line: a live leaderboard snapshot with tightening
/// confidence intervals. Same field conventions as [`OfcMatchReport`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OfcProgressReport {
    pub schema_version: u32,
    pub hands_done: u64,
    pub hands_total: u64,
    pub standings: Vec<OfcStanding>,
}

/// One bot's interim standing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OfcStanding {
    pub name: String,
    pub points: i64,
    pub points_per_hand_mean: f64,
    pub points_per_hand_ci95: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_match_report() -> OfcMatchReport {
        OfcMatchReport {
            schema_version: OFC_REPORT_SCHEMA_VERSION,
            family: "ofc".into(),
            game_id: "ofc-pineapple".into(),
            hands: 100,
            seed: 9,
            seat_count: 2,
            timeout_ms: Some(1_000),
            fault_policy: "substitute".into(),
            forfeited_by: None,
            bots: vec![OfcBotReport {
                name: "greedy".into(),
                kind: "builtin:greedy".into(),
                points: 65,
                points_per_hand_mean: 0.65,
                points_per_hand_ci95: Some(0.42),
                hands: 100,
                fouls: 8,
                fantasylands: 5,
                scoops: 3,
                royalties: 40,
                faults: 0,
                decisions: 500,
                decision_ms_mean: Some(0.6),
                decision_ms_p50: Some(0.5),
                decision_ms_p90: Some(1.1),
                decision_ms_p99: Some(4.8),
                decision_ms_max: Some(9.4),
            }],
        }
    }

    #[test]
    fn match_report_round_trips_through_json() {
        let report = sample_match_report();
        let json = serde_json::to_string(&report).unwrap();
        assert_eq!(
            serde_json::from_str::<OfcMatchReport>(&json).unwrap(),
            report
        );
    }

    #[test]
    fn progress_report_round_trips_through_json() {
        let progress = OfcProgressReport {
            schema_version: OFC_REPORT_SCHEMA_VERSION,
            hands_done: 40,
            hands_total: 100,
            standings: vec![OfcStanding {
                name: "greedy".into(),
                points: -12,
                points_per_hand_mean: -0.3,
                points_per_hand_ci95: Some(0.5),
            }],
        };
        let json = serde_json::to_string(&progress).unwrap();
        assert_eq!(
            serde_json::from_str::<OfcProgressReport>(&json).unwrap(),
            progress
        );
    }

    /// Pin the exact wire format for a progress-report line so a change to
    /// field names/order/case is caught by a test, not just by round-trip.
    #[test]
    fn progress_report_has_the_expected_exact_json() {
        let progress = OfcProgressReport {
            schema_version: 1,
            hands_done: 40,
            hands_total: 100,
            standings: vec![
                OfcStanding {
                    name: "greedy".into(),
                    points: 65,
                    points_per_hand_mean: 1.625,
                    points_per_hand_ci95: Some(0.42),
                },
                OfcStanding {
                    name: "filler".into(),
                    points: -65,
                    points_per_hand_mean: -1.625,
                    points_per_hand_ci95: None,
                },
            ],
        };
        assert_eq!(
            serde_json::to_string(&progress).unwrap(),
            r#"{"schema_version":1,"hands_done":40,"hands_total":100,"standings":[{"name":"greedy","points":65,"points_per_hand_mean":1.625,"points_per_hand_ci95":0.42},{"name":"filler","points":-65,"points_per_hand_mean":-1.625,"points_per_hand_ci95":null}]}"#
        );
    }
}

//! Machine-readable match results.
//!
//! [`MatchReport`] is the JSON document a website or script consumes
//! instead of scraping the CLI's human tables (`--output json`). It is a
//! self-contained summary: everything needed to rank bots, display
//! confidence, and reproduce the match (the seed) — while per-hand detail
//! stays in the hand log, which is already JSON lines.
//!
//! Field naming is snake_case, matching the wire protocol's conventions.
//! `schema_version` bumps on any breaking shape change.

use serde::Serialize;

use poker_wire::game::Stakes;

use crate::behavior::BehaviorStats;
use crate::config::{DealingMode, FaultPolicy, MatchConfig};
use crate::runner::{MatchResult, Progress};

/// Breaking-change counter for the report shape.
pub const REPORT_SCHEMA_VERSION: u32 = 1;

/// The complete machine-readable result of one match.
#[derive(Debug, Clone, Serialize)]
pub struct MatchReport {
    pub schema_version: u32,
    pub game_id: String,
    /// The seed that reproduces this match exactly.
    pub seed: u64,
    pub dealing: &'static str,
    pub decks: u64,
    pub hands: u64,
    pub seat_count: usize,
    pub starting_stack: u64,
    pub stakes: Stakes,
    pub betting: poker_wire::game::BettingKind,
    pub fault_policy: &'static str,
    pub timeout_ms: Option<u64>,
    /// The unit `rate_per100_*` is measured in: `"big-blind"` for blind
    /// games, `"small-bet"` for stud.
    pub rate_unit: &'static str,
    /// Bot name, when the match ended early by forfeit.
    pub forfeited_by: Option<String>,
    /// In the order bots were seated on the command line.
    pub bots: Vec<BotReport>,
}

/// One bot's results.
#[derive(Debug, Clone, Serialize)]
pub struct BotReport {
    pub name: String,
    pub hands: u64,
    pub total_chips: i64,
    /// Mean winnings per 100 hands, in `rate_unit`s.
    pub rate_per100_mean: f64,
    /// Two-sided 95% Student-t half-width of the mean, same scale;
    /// `null` with fewer than two observations.
    pub rate_per100_ci95: Option<f64>,
    /// Statistical observations behind the interval (hands in seeded mode,
    /// duplicate rotation-sets in duplicate mode).
    pub observations: u64,
    pub faults: u64,
    pub behavior: BehaviorReport,
}

/// Behavioral profile, all rates in `[0, 1]`.
#[derive(Debug, Clone, Serialize)]
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

impl MatchReport {
    pub fn new(config: &MatchConfig, seed: u64, result: &MatchResult) -> MatchReport {
        MatchReport {
            schema_version: REPORT_SCHEMA_VERSION,
            game_id: config.spec.id.to_string(),
            seed,
            dealing: match config.dealing {
                DealingMode::Seeded => "seeded",
                DealingMode::Duplicate => "duplicate",
            },
            decks: result.decks_played,
            hands: result.hands_played,
            seat_count: result.outcomes.len(),
            starting_stack: config.starting_stack,
            stakes: config.spec.stakes,
            betting: config.spec.betting,
            fault_policy: match config.fault_policy {
                FaultPolicy::CheckFold => "check-fold",
                FaultPolicy::Forfeit => "forfeit",
            },
            timeout_ms: config.timeout.map(|d| d.as_millis() as u64),
            rate_unit: match config.spec.stakes {
                Stakes::Blinds { .. } => "big-blind",
                Stakes::Stud { .. } => "small-bet",
            },
            forfeited_by: result.forfeited_by.map(|b| result.outcomes[b].name.clone()),
            bots: result
                .outcomes
                .iter()
                .map(|o| BotReport {
                    name: o.name.clone(),
                    hands: result.hands_played,
                    total_chips: o.total_net_chips,
                    rate_per100_mean: o.stats.mean() * 100.0,
                    rate_per100_ci95: o.stats.ci95_half_width().map(|hw| hw * 100.0),
                    observations: o.stats.count(),
                    faults: o.faults,
                    behavior: BehaviorReport::from(&o.behavior),
                })
                .collect(),
        }
    }
}

/// One interim-standings line for `--progress-json`: emitted as JSON lines
/// during a match so a consumer can render a live leaderboard with
/// tightening confidence intervals. Same field conventions as
/// [`MatchReport`].
#[derive(Debug, Clone, Serialize)]
pub struct ProgressReport {
    pub decks_done: u64,
    pub hands_done: u64,
    pub bots: Vec<BotProgress>,
}

/// One bot's interim standing.
#[derive(Debug, Clone, Serialize)]
pub struct BotProgress {
    pub name: String,
    pub total_chips: i64,
    pub rate_per100_mean: f64,
    pub rate_per100_ci95: Option<f64>,
    pub observations: u64,
    pub faults: u64,
}

impl ProgressReport {
    pub fn new(progress: &Progress<'_>) -> ProgressReport {
        ProgressReport {
            decks_done: progress.decks_done,
            hands_done: progress.hands_done,
            bots: progress
                .standings
                .iter()
                .map(|s| BotProgress {
                    name: s.name.clone(),
                    total_chips: s.total_chips,
                    rate_per100_mean: s.mean * 100.0,
                    rate_per100_ci95: s.ci95.map(|hw| hw * 100.0),
                    observations: s.observations,
                    faults: s.faults,
                })
                .collect(),
        }
    }
}

impl From<&BehaviorStats> for BehaviorReport {
    fn from(b: &BehaviorStats) -> BehaviorReport {
        let af = b.af();
        BehaviorReport {
            vpip: b.vpip(),
            pfr: b.pfr(),
            af: af.is_finite().then_some(af),
            wtsd: b.wtsd(),
            wsd: b.wsd(),
            fold_rate: b.fold_rate(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::Bot;
    use crate::builtin::{Caller, Random};
    use crate::runner::run_match;
    use poker_core::game::GameSpec;

    #[test]
    fn report_serializes_with_consistent_totals() {
        let config = MatchConfig {
            spec: GameSpec::holdem_nl(Stakes::Blinds {
                small_blind: 50,
                big_blind: 100,
            }),
            decks: 20,
            seed: 5,
            dealing: DealingMode::Duplicate,
            starting_stack: 10_000,
            fault_policy: FaultPolicy::CheckFold,
            timeout: None,
        };
        let mut bots: Vec<Box<dyn Bot>> = vec![
            Box::new(Caller::new("caller")),
            Box::new(Random::new("random", 3)),
        ];
        let result = run_match(&config, &mut bots, None, None).unwrap();
        let report = MatchReport::new(&config, config.seed, &result);

        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["game_id"], "holdem-nl");
        assert_eq!(json["seed"], 5);
        assert_eq!(json["dealing"], "duplicate");
        assert_eq!(json["hands"], 40);
        assert_eq!(json["rate_unit"], "big-blind");
        assert_eq!(json["stakes"]["kind"], "blinds");
        assert_eq!(json["betting"]["kind"], "no-limit");
        assert!(json["forfeited_by"].is_null());

        let bots = json["bots"].as_array().unwrap();
        assert_eq!(bots.len(), 2);
        let total: i64 = bots
            .iter()
            .map(|b| b["total_chips"].as_i64().unwrap())
            .sum();
        assert_eq!(total, 0, "reported totals must stay zero-sum");
        for b in bots {
            assert_eq!(b["observations"], 20, "duplicate: one obs per deck");
            assert_eq!(b["hands"], 40);
            assert!(b["rate_per100_ci95"].as_f64().unwrap() >= 0.0);
            let vpip = b["behavior"]["vpip"].as_f64().unwrap();
            assert!((0.0..=1.0).contains(&vpip));
        }
    }
}

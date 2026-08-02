//! Builders for the machine-readable match documents.
//!
//! The document *shapes* live in [`poker_wire::report`] (the consumer-facing
//! vocabulary, `Serialize` + `Deserialize`); this module knows how to fill
//! them from a finished [`MatchResult`] or an in-flight [`Progress`] tick.

pub use poker_wire::report::{
    BehaviorReport, BotProgress, BotReport, MatchReport, ProgressReport, REPORT_SCHEMA_VERSION,
};

use crate::behavior::BehaviorStats;
use crate::config::{DealingMode, FaultPolicy, MatchConfig};
use crate::runner::{MatchResult, Progress};

/// Build the final report for a completed match.
pub fn match_report(config: &MatchConfig, seed: u64, result: &MatchResult) -> MatchReport {
    MatchReport {
        schema_version: REPORT_SCHEMA_VERSION,
        family: "betting".to_string(),
        game_id: config.spec.id.to_string(),
        seed,
        dealing: match config.dealing {
            DealingMode::Seeded => "seeded",
            DealingMode::Duplicate => "duplicate",
        }
        .to_string(),
        decks: result.decks_played,
        hands: result.hands_played,
        seat_count: result.outcomes.len(),
        starting_stack: config.starting_stack,
        stakes: config.spec.stakes,
        betting: config.spec.betting,
        fault_policy: match config.fault_policy {
            FaultPolicy::Substitute => "substitute",
            FaultPolicy::Forfeit => "forfeit",
        }
        .to_string(),
        timeout_ms: config.timeout.map(|d| d.as_millis() as u64),
        forfeited_by: result.forfeited_by.map(|b| result.outcomes[b].name.clone()),
        bots: result
            .outcomes
            .iter()
            .map(|o| BotReport {
                name: o.name.clone(),
                hands: result.hands_played,
                total_chips: o.total_net_chips,
                chips_per100_mean: o.stats.mean() * 100.0,
                chips_per100_ci95: o.stats.ci95_half_width().map(|hw| hw * 100.0),
                observations: o.stats.count(),
                faults: o.faults,
                decisions: decision_timing(&o.decision_stats),
                behavior: behavior_report(&o.behavior),
            })
            .collect(),
    }
}

/// Build one interim-standings line from a [`Progress`] tick.
pub fn progress_report(progress: &Progress<'_>) -> ProgressReport {
    ProgressReport {
        decks_done: progress.decks_done,
        hands_done: progress.hands_done,
        bots: progress
            .standings
            .iter()
            .map(|s| BotProgress {
                name: s.name.clone(),
                total_chips: s.total_chips,
                chips_per100_mean: s.mean * 100.0,
                chips_per100_ci95: s.ci95.map(|hw| hw * 100.0),
                observations: s.observations,
                faults: s.faults,
            })
            .collect(),
    }
}

/// The wire timing block from an accumulated [`DecisionStats`]. Shared by
/// both report builders — the OFC builder uses it too.
pub(crate) fn decision_timing(
    stats: &crate::stat::DecisionStats,
) -> poker_wire::report::DecisionTiming {
    poker_wire::report::DecisionTiming {
        count: stats.count(),
        mean_ms: stats.mean_ms(),
        p50_ms: stats.quantile(0.5),
        p90_ms: stats.quantile(0.9),
        p99_ms: stats.quantile(0.99),
        max_ms: stats.max_ms(),
    }
}

fn behavior_report(b: &BehaviorStats) -> BehaviorReport {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::Bot;
    use crate::builtin::{Caller, Random};
    use crate::runner::run_match;
    use poker_core::game::{GameSpec, Stakes};

    #[test]
    fn report_serializes_with_consistent_totals() {
        let config = MatchConfig {
            spec: GameSpec::holdem_nl(Stakes::Blinds {
                small_blind: 50,
                big_blind: 100,
                ante: 0,
            }),
            decks: 20,
            seed: 5,
            dealing: DealingMode::Duplicate,
            starting_stack: 10_000,
            fault_policy: FaultPolicy::Substitute,
            timeout: None,
        };
        let mut bots: Vec<Box<dyn Bot>> = vec![
            Box::new(Caller::new("caller")),
            Box::new(Random::new("random", 3)),
        ];
        let result = run_match(&config, &mut bots, None, None).unwrap();
        let report = match_report(&config, config.seed, &result);

        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["family"], "betting");
        assert_eq!(json["game_id"], "holdem-nl");
        assert_eq!(json["seed"], 5);
        assert_eq!(json["dealing"], "duplicate");
        assert_eq!(json["hands"], 40);
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
            assert!(b["chips_per100_ci95"].as_f64().unwrap() >= 0.0);
            let vpip = b["behavior"]["vpip"].as_f64().unwrap();
            assert!((0.0..=1.0).contains(&vpip));
            assert!(b["decisions"]["count"].as_u64().unwrap() > 0);
            let mean = b["decisions"]["mean_ms"].as_f64().unwrap();
            let p50 = b["decisions"]["p50_ms"].as_f64().unwrap();
            let p90 = b["decisions"]["p90_ms"].as_f64().unwrap();
            let p99 = b["decisions"]["p99_ms"].as_f64().unwrap();
            let max = b["decisions"]["max_ms"].as_f64().unwrap();
            assert!(mean >= 0.0);
            assert!(p50 <= p90, "p50 {p50} > p90 {p90}");
            assert!(p90 <= p99, "p90 {p90} > p99 {p99}");
            assert!(p99 <= max, "p99 {p99} > max {max}");
        }
        // The wire shape is the parse contract: round-trip through it.
        let text = serde_json::to_string(&report).unwrap();
        let parsed: MatchReport = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed, report);
    }
}

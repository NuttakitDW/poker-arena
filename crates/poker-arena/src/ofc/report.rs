//! Builders for the machine-readable OFC match documents.
//!
//! The document *shapes* live in [`poker_wire::ofc::report`] (the
//! consumer-facing vocabulary, `Serialize` + `Deserialize`); this module
//! knows how to fill them from a finished [`OfcMatchResult`] or an in-flight
//! [`OfcProgress`] tick — the OFC mirror of [`crate::report`], with points
//! replacing chips.

pub use poker_wire::ofc::report::{
    OFC_REPORT_SCHEMA_VERSION, OfcBotReport, OfcMatchReport, OfcProgressReport, OfcStanding,
};

use crate::config::FaultPolicy;
use crate::ofc::runner::{OfcMatchConfig, OfcMatchResult, OfcProgress};

/// Build the final report for a completed match.
///
/// `kinds[b]` is the bot-spec string bot `b` was created from (e.g.
/// `"builtin:greedy"`, `"cmd"`) — the CLI owns that vocabulary, so it is
/// passed in rather than inferred; a bot with no entry reports an empty
/// kind.
pub fn ofc_match_report(
    config: &OfcMatchConfig,
    seed: u64,
    kinds: &[String],
    result: &OfcMatchResult,
) -> OfcMatchReport {
    OfcMatchReport {
        schema_version: OFC_REPORT_SCHEMA_VERSION,
        family: "ofc".to_string(),
        game_id: config.spec.id.to_string(),
        hands: result.hands_played,
        seed,
        seat_count: result.outcomes.len(),
        timeout_ms: config.timeout.map(|d| d.as_millis() as u64),
        fault_policy: match config.fault_policy {
            FaultPolicy::Substitute => "substitute",
            FaultPolicy::Forfeit => "forfeit",
        }
        .to_string(),
        forfeited_by: result
            .forfeited_by
            .map(|bot| result.outcomes[bot].name.clone()),
        bots: result
            .outcomes
            .iter()
            .enumerate()
            .map(|(bot, outcome)| OfcBotReport {
                name: outcome.name.clone(),
                kind: kinds.get(bot).cloned().unwrap_or_default(),
                points: outcome.total_points,
                points_per_hand_mean: outcome.stats.mean(),
                points_per_hand_ci95: outcome.stats.ci95_half_width(),
                hands: result.hands_played,
                fouls: outcome.fouls,
                fantasylands: outcome.fantasylands,
                scoops: outcome.scoops,
                royalties: outcome.royalties,
                faults: outcome.faults,
                decisions: outcome.decision_stats.count(),
                decision_ms_mean: outcome.decision_stats.mean_ms(),
                decision_ms_p50: outcome.decision_stats.quantile(0.5),
                decision_ms_p90: outcome.decision_stats.quantile(0.9),
                decision_ms_p99: outcome.decision_stats.quantile(0.99),
                decision_ms_max: outcome.decision_stats.max_ms(),
            })
            .collect(),
    }
}

/// Build one interim-standings line from an [`OfcProgress`] tick.
pub fn ofc_progress_report(progress: &OfcProgress<'_>) -> OfcProgressReport {
    OfcProgressReport {
        schema_version: OFC_REPORT_SCHEMA_VERSION,
        hands_done: progress.hands_done,
        hands_total: progress.hands_total,
        standings: progress
            .standings
            .iter()
            .map(|standing| OfcStanding {
                name: standing.name.clone(),
                points: standing.total_points,
                points_per_hand_mean: standing.mean,
                points_per_hand_ci95: standing.ci95,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ofc::bot::OfcBot;
    use crate::ofc::builtin::{OfcGreedy, OfcRandom};
    use crate::ofc::runner::run_ofc_match;
    use poker_core::ofc::OFC_PROGRESSIVE;

    #[test]
    fn report_serializes_with_consistent_totals() {
        let config = OfcMatchConfig {
            spec: OFC_PROGRESSIVE,
            hands: 40,
            seed: 5,
            fault_policy: FaultPolicy::Substitute,
            timeout: None,
        };
        let mut bots: Vec<Box<dyn OfcBot>> = vec![
            Box::new(OfcGreedy::new("greedy", OFC_PROGRESSIVE.middle)),
            Box::new(OfcRandom::new("random", 3)),
        ];
        let result = run_ofc_match(&config, &mut bots, &mut [], None).unwrap();
        let kinds = vec!["builtin:greedy".to_string(), "builtin:random".to_string()];
        let report = ofc_match_report(&config, config.seed, &kinds, &result);

        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["family"], "ofc");
        assert_eq!(json["game_id"], "ofc-progressive");
        assert_eq!(json["seed"], 5);
        assert_eq!(json["hands"], 40);
        assert_eq!(json["fault_policy"], "substitute");
        assert!(json["forfeited_by"].is_null());

        let bots = json["bots"].as_array().unwrap();
        assert_eq!(bots.len(), 2);
        let total: i64 = bots.iter().map(|b| b["points"].as_i64().unwrap()).sum();
        assert_eq!(total, 0, "reported totals must stay zero-sum");
        assert_eq!(bots[0]["kind"], "builtin:greedy");
        for entry in bots {
            assert_eq!(entry["hands"], 40);
            assert!(entry["points_per_hand_ci95"].as_f64().unwrap() >= 0.0);
            assert!(entry["decisions"].as_u64().unwrap() > 0);
            let mean = entry["decision_ms_mean"].as_f64().unwrap();
            let p50 = entry["decision_ms_p50"].as_f64().unwrap();
            let p90 = entry["decision_ms_p90"].as_f64().unwrap();
            let p99 = entry["decision_ms_p99"].as_f64().unwrap();
            let max = entry["decision_ms_max"].as_f64().unwrap();
            assert!(mean >= 0.0);
            assert!(p50 <= p90, "p50 {p50} > p90 {p90}");
            assert!(p90 <= p99, "p90 {p90} > p99 {p99}");
            assert!(p99 <= max, "p99 {p99} > max {max}");
        }

        // The wire shape is the parse contract: round-trip through it.
        let text = serde_json::to_string(&report).unwrap();
        let parsed: OfcMatchReport = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed, report);
    }

    #[test]
    fn progress_report_mirrors_the_tick_it_was_built_from() {
        let config = OfcMatchConfig {
            spec: OFC_PROGRESSIVE,
            hands: 6,
            seed: 9,
            fault_policy: FaultPolicy::Substitute,
            timeout: None,
        };
        let mut bots: Vec<Box<dyn OfcBot>> = vec![
            Box::new(OfcGreedy::new("greedy", OFC_PROGRESSIVE.middle)),
            Box::new(OfcRandom::new("random", 4)),
        ];
        let mut lines: Vec<OfcProgressReport> = Vec::new();
        let mut callback = |progress: &OfcProgress<'_>| lines.push(ofc_progress_report(progress));
        run_ofc_match(&config, &mut bots, &mut [], Some(&mut callback)).unwrap();

        assert_eq!(lines.len(), 6);
        let last = lines.last().unwrap();
        assert_eq!(last.schema_version, OFC_REPORT_SCHEMA_VERSION);
        assert_eq!(last.hands_done, 6);
        assert_eq!(last.hands_total, 6);
        assert_eq!(last.standings.len(), 2);
        assert_eq!(last.standings.iter().map(|s| s.points).sum::<i64>(), 0);
        assert_eq!(last.standings[0].name, "greedy");
    }
}

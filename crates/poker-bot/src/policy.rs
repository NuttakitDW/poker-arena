//! The equity-heuristic policy: one decision function for every betting
//! variant, driven by Monte-Carlo pot share against the legal-action menu
//! the arena offers.
//!
//! Value-oriented and bluff-free by design: raise with clear equity edges,
//! call on pot odds, fold the rest. Thresholds are expressed relative to
//! the "fair share" `1/players` so the same policy scales from heads-up to
//! full-ring. Draw decisions search every keep-subset by sampled equity;
//! the bring-in completes with an above-average start. Every action chosen
//! is drawn from the offered `decision`, never derived — the arena stays
//! legality-authoritative.

use poker_core::card::Card;
use poker_core::game::spec::GameSpec;
use poker_core::rng::Rng64;
use poker_wire::action::{Action, BetBounds};
use poker_wire::message::WireDecision;

use crate::equity::{equity, equity_with_replacement};
use crate::table::Table;

/// Rollouts per wager decision (halved under tight deadlines).
const WAGER_SAMPLES: u32 = 320;
/// Rollouts per candidate keep at a draw decision.
const DRAW_SAMPLES: u32 = 60;
/// Equity edge (over pot odds) required to put in more chips than a call.
const RAISE_EDGE: f64 = 1.35;
/// Equity edge for opening a bet when checking is free.
const BET_EDGE: f64 = 1.15;
/// Slack under exact pot odds a call tolerates (draw potential the
/// stand-pat rollout can't see).
const CALL_SLACK: f64 = 0.02;

pub struct Policy {
    spec: GameSpec,
    rng: Rng64,
}

impl Policy {
    pub fn new(spec: GameSpec, seed: u64) -> Policy {
        Policy {
            spec,
            rng: Rng64::from_seed_stream(seed, 0),
        }
    }

    /// Answer one `act`. `deadline_ms` shrinks the sample budget so slow
    /// hosts never flirt with the arena's timeout.
    pub fn decide(
        &mut self,
        decision: &WireDecision,
        table: &Table,
        deadline_ms: Option<u64>,
    ) -> Action {
        match decision {
            WireDecision::Wager {
                fold,
                check,
                call,
                bet,
                raise,
            } => self.wager(table, *fold, *check, *call, *bet, *raise, deadline_ms),
            WireDecision::Draw { max_discards } => Action::Discard {
                cards: self.choose_discards(table, *max_discards),
            },
            WireDecision::BringIn { complete, .. } => {
                let e = self.pot_share(table, deadline_ms);
                if e > 1.1 * self.fair_share(table) {
                    Action::Bet {
                        to: complete.min_to,
                    }
                } else {
                    Action::BringIn
                }
            }
        }
    }

    fn samples(&self, base: u32, deadline_ms: Option<u64>) -> u32 {
        match deadline_ms {
            Some(ms) if ms < 150 => base / 4,
            Some(ms) if ms < 500 => base / 2,
            _ => base,
        }
        .max(16)
    }

    fn pot_share(&mut self, table: &Table, deadline_ms: Option<u64>) -> f64 {
        let samples = self.samples(WAGER_SAMPLES, deadline_ms);
        equity(&self.spec, table, &mut self.rng, samples)
    }

    /// Pot share a hand with no edge would have.
    fn fair_share(&self, table: &Table) -> f64 {
        1.0 / (table.live_opponents() + 1) as f64
    }

    #[allow(clippy::too_many_arguments)]
    fn wager(
        &mut self,
        table: &Table,
        fold: bool,
        check: bool,
        call: Option<u64>,
        bet: Option<BetBounds>,
        raise: Option<BetBounds>,
        deadline_ms: Option<u64>,
    ) -> Action {
        let e = self.pot_share(table, deadline_ms);
        let fair = self.fair_share(table);
        let pot = table.pot();

        if let Some(call_amount) = call {
            // Facing a wager: price the call, escalate with a clear edge.
            let price = call_amount as f64 / (pot + call_amount) as f64;
            let strong = e > (RAISE_EDGE * fair).clamp(0.40, 0.80);
            if strong && let Some(bounds) = raise {
                return Action::Raise {
                    to: sized_to(bounds, pot, call_amount),
                };
            }
            if e + CALL_SLACK >= price.min(0.95) {
                return Action::Call;
            }
            if fold {
                return Action::Fold;
            }
            return Action::Call;
        }

        // Nothing to call: open with an edge, otherwise take the free card.
        let opens = e > (BET_EDGE * fair).clamp(0.35, 0.70);
        if opens && let Some(bounds) = bet {
            return Action::Bet {
                to: sized_to(bounds, pot, 0),
            };
        }
        if check {
            return Action::Check;
        }
        // No check and nothing to call shouldn't happen; fold if it can,
        // else the smallest legal bet keeps us conforming.
        if fold {
            return Action::Fold;
        }
        match bet.or(raise) {
            Some(bounds) => Action::Bet { to: bounds.min_to },
            None => Action::Check,
        }
    }

    /// Search every keep-subset (discard the rest) by sampled equity.
    /// At most 2^5 = 32 candidates; ties resolve to the first, which
    /// enumeration order makes the *smallest* discard.
    fn choose_discards(&mut self, table: &Table, max_discards: u8) -> Vec<Card> {
        let hand = table.hole.clone();
        let n = hand.len();
        let mut best: Option<(f64, Vec<Card>)> = None;
        for mask in 0u32..(1 << n) {
            let discard_count = mask.count_ones() as usize;
            if discard_count > usize::from(max_discards) {
                continue;
            }
            let keep: Vec<Card> = (0..n)
                .filter(|i| mask & (1 << i) == 0)
                .map(|i| hand[i])
                .collect();
            let e = equity_with_replacement(
                &self.spec,
                table,
                &keep,
                discard_count,
                &mut self.rng,
                DRAW_SAMPLES,
            );
            if best.as_ref().is_none_or(|(top, _)| e > *top) {
                let discards = (0..n)
                    .filter(|i| mask & (1 << i) != 0)
                    .map(|i| hand[i])
                    .collect();
                best = Some((e, discards));
            }
        }
        best.map(|(_, discards)| discards).unwrap_or_default()
    }
}

/// A pot-sized total for this street, clamped into the offered bounds.
/// `to` semantics are a *total* street commitment, so facing a wager the
/// target is the call price plus the pot after calling.
fn sized_to(bounds: BetBounds, pot: u64, call_amount: u64) -> u64 {
    let target = if call_amount > 0 {
        call_amount + pot + call_amount
    } else {
        (pot * 2) / 3
    };
    target.clamp(bounds.min_to, bounds.max_to)
}

#[cfg(test)]
mod tests {
    use super::*;
    use poker_core::card::parse_cards;
    use poker_wire::game::Stakes;

    const STAKES: Stakes = Stakes::Blinds {
        small_blind: 50,
        big_blind: 100,
        ante: 0,
    };

    fn table(hole: &str, board: &str, committed: &[u64]) -> Table {
        let mut t = Table::default();
        t.hand_start(0, committed.len());
        t.hole = parse_cards(hole).unwrap();
        t.board = parse_cards(board).unwrap();
        t.street = committed.to_vec();
        t
    }

    fn wager_facing(call: u64, raise: Option<BetBounds>) -> WireDecision {
        WireDecision::Wager {
            fold: true,
            check: false,
            call: Some(call),
            bet: None,
            raise,
        }
    }

    #[test]
    fn the_nuts_raise_when_raising_is_offered() {
        let mut policy = Policy::new(GameSpec::holdem_nl(STAKES), 1);
        let t = table("As Ks", "Qs Js Ts 2d 3d", &[100, 100]);
        let decision = wager_facing(
            100,
            Some(BetBounds {
                min_to: 200,
                max_to: 10_000,
            }),
        );
        let action = policy.decide(&decision, &t, None);
        assert!(
            matches!(action, Action::Raise { .. }),
            "royal flush must raise, got {action:?}"
        );
    }

    #[test]
    fn hopeless_hands_fold_to_a_big_bet() {
        let mut policy = Policy::new(GameSpec::holdem_nl(STAKES), 2);
        // Board pairs and flushes everywhere we don't have: 72o facing a
        // pot-sized river bet is far below the price.
        let t = table("7c 2d", "As Ks Qs Ah Kh", &[100, 100]);
        let decision = wager_facing(4_000, None);
        let action = policy.decide(&decision, &t, None);
        assert_eq!(action, Action::Fold);
    }

    #[test]
    fn free_checks_are_taken_with_nothing() {
        let mut policy = Policy::new(GameSpec::holdem_nl(STAKES), 3);
        let t = table("7c 2d", "As Ks Qs Ah Kh", &[100, 100]);
        let decision = WireDecision::Wager {
            fold: false,
            check: true,
            call: None,
            bet: Some(BetBounds {
                min_to: 100,
                max_to: 9_900,
            }),
            raise: None,
        };
        let action = policy.decide(&decision, &t, None);
        assert_eq!(action, Action::Check);
    }

    #[test]
    fn draw_decisions_discard_at_most_the_cap_and_only_held_cards() {
        let mut policy = Policy::new(GameSpec::td27_fl(STAKES), 4);
        let mut t = table("Kc Qd Jh Ts 2c", "", &[100, 100]);
        t.folded = vec![false, false];
        let action = policy.decide(&WireDecision::Draw { max_discards: 5 }, &t, Some(1_000));
        let Action::Discard { cards } = action else {
            panic!("draw decisions must answer with a discard");
        };
        assert!(cards.len() <= 5);
        let hand = parse_cards("Kc Qd Jh Ts 2c").unwrap();
        assert!(cards.iter().all(|card| hand.contains(card)));
        // Broadway cards are poison in 2-7; a sensible search ditches some.
        assert!(!cards.is_empty(), "standing pat with KQJT is wrong in 2-7");
    }

    #[test]
    fn sized_to_respects_bounds() {
        let bounds = BetBounds {
            min_to: 200,
            max_to: 500,
        };
        assert_eq!(sized_to(bounds, 10_000, 100), 500, "clamped to max");
        assert_eq!(sized_to(bounds, 0, 0), 200, "clamped to min");
        let single = BetBounds {
            min_to: 300,
            max_to: 300,
        };
        assert_eq!(sized_to(single, 1_000, 100), 300, "single legal total");
    }
}

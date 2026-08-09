//! Lossy abstraction: per-street equity buckets, a discrete action menu,
//! and the resulting abstract game-tree size.
//!
//! On top of the lossless suit isomorphism (`iso`), three lossy reductions
//! make every variant solvable within a fixed budget:
//!
//! 1. **Card buckets** — at each street, a hand context maps to one of a
//!    small number of buckets by its equity percentile (imperfect recall:
//!    the bucket depends only on the current street's context, not the path
//!    that reached it).
//! 2. **Action abstraction** — big-bet games offer two sizes (pot, all-in)
//!    with at most two wagers per street; fixed-limit games are already
//!    discrete (one size, engine-capped raises).
//! 3. **Draw abstraction** — a draw decision branches only on *how many*
//!    cards to discard; *which* cards is a deterministic keep-best rule.
//!
//! The tree size measured here is the **single-perspective abstract tree**:
//! the product over streets of `buckets × betting sequences × draw options`
//! — the tree one player's solver walks (chance nodes branch on that
//! player's own bucket transitions; opponent holdings surface in the leaf
//! utilities, as in external-sampling MCCFR). Every game's plan must fit in
//! [`TREE_BUDGET`] = 10^12 nodes; `tests` enforce it for the whole registry.

use poker_core::game::spec::{DealSpec, ForcedBets, GameSpec};
use poker_wire::game::BettingKind;

/// The solvability budget every game's abstract tree must fit inside.
pub const TREE_BUDGET: f64 = 1e12;

/// One street of the abstract game.
#[derive(Clone, Debug)]
pub struct StreetPlan {
    pub label: &'static str,
    /// Equity-percentile buckets for this street's card contexts.
    pub buckets: u64,
    /// Heads-up betting sequences this street's abstract menu allows
    /// (1 for a street with no betting round).
    pub bet_sequences: u64,
    /// Branches of the abstract draw decision (`max_discards + 1`;
    /// 1 for a non-draw street).
    pub draw_options: u64,
}

impl StreetPlan {
    fn nodes(&self) -> f64 {
        self.buckets as f64 * self.bet_sequences as f64 * self.draw_options as f64
    }
}

/// The abstraction plan for one game.
#[derive(Clone, Debug)]
pub struct GamePlan {
    pub game_id: &'static str,
    pub streets: Vec<StreetPlan>,
}

impl GamePlan {
    /// Nodes in the single-perspective abstract game tree.
    pub fn tree_size(&self) -> f64 {
        self.streets.iter().map(StreetPlan::nodes).product()
    }
}

/// Map an equity estimate in `[0, 1]` to a bucket in `0..buckets`.
pub fn bucket_of(equity: f64, buckets: u64) -> u64 {
    let clamped = equity.clamp(0.0, 1.0);
    ((clamped * buckets as f64) as u64).min(buckets - 1)
}

/// Count heads-up betting sequences for a street: `sizes` bet/raise sizes
/// on the menu, at most `cap` wagers (the opening bet counts as the first).
///
/// `opened_by_force` models a stud bring-in street: the round opens with a
/// forced choice (bring-in or complete — 2 ways, each a first wager)
/// instead of the usual check-or-bet.
fn bet_sequences(sizes: u64, cap: u32, opened_by_force: bool) -> u64 {
    // Sequences continuing after `wagers` wagers with a bet outstanding:
    // the facing player may fold, call, or (below the cap) raise.
    fn chain(wagers: u32, cap: u32, sizes: u64) -> u64 {
        let mut n = 2;
        if wagers < cap {
            n += sizes * chain(wagers + 1, cap, sizes);
        }
        n
    }
    if opened_by_force {
        return 2 * chain(1, cap, sizes);
    }
    // check-check, first player opens, or check then second player opens.
    1 + 2 * sizes * chain(1, cap, sizes)
}

/// The abstract action-menu parameters for a betting structure: bet/raise
/// sizes offered and the wager cap per street.
fn menu(betting: BettingKind) -> (u64, u32) {
    match betting {
        // The engine's cap is authoritative; None means uncapped, which the
        // abstraction still caps to keep the tree finite.
        BettingKind::FixedLimit { raise_cap } => (1, u32::from(raise_cap.unwrap_or(4))),
        // Two sizes (pot, all-in), two wagers: bet-raise or raise-reraise
        // ends the abstract street.
        BettingKind::NoLimit | BettingKind::PotLimit => (2, 2),
    }
}

/// Per-game bucket counts, indexed by street. Tuned so every plan fits
/// [`TREE_BUDGET`] with the menu above; the registry-wide test pins that.
///
/// First-street counts at or below the lossless class count stay exact
/// (hold'em's 169); everything else is an equity-percentile bucketing.
fn bucket_table(game_id: &str) -> Option<&'static [u64]> {
    Some(match game_id {
        "holdem-fl" => &[169, 40, 40, 40],
        "holdem-nl" => &[169, 24, 24, 24],
        "omaha-pl" | "omaha8-pl" | "bigo-pl" => &[100, 24, 24, 24],
        "omaha8-fl" => &[100, 40, 40, 40],
        // Street 0 is the bet-less down-card deal; its chance folds into
        // third street's bucket.
        "stud-fl" | "stud8-fl" | "razz-fl" => &[1, 30, 12, 12, 12, 12],
        "27td-fl" | "a5td-fl" | "badacey-fl" | "badeucy-fl" | "archie-fl" => &[50, 10, 10, 10],
        "badugi-fl" => &[50, 12, 12, 12],
        "5cd-nl" | "27sd-nl" => &[500, 2000],
        "drawmaha-fl" | "drawmaha-27-fl" | "drawmaha-dugi-fl" => &[30, 15, 12, 15, 15],
        _ => return None,
    })
}

/// Build the abstraction plan for a game.
pub fn plan(spec: &GameSpec) -> GamePlan {
    let (sizes, cap) = menu(spec.betting);
    let buckets = bucket_table(spec.id);
    let mut first_betting_street = true;
    let streets = spec
        .streets
        .iter()
        .enumerate()
        .map(|(index, street)| {
            let bet_sequences = match &street.betting {
                Some(_) => {
                    let forced = first_betting_street
                        && matches!(spec.forced_bets, ForcedBets::BringIn { .. });
                    first_betting_street = false;
                    bet_sequences(sizes, cap, forced)
                }
                None => 1,
            };
            let draw_options = match street.deal {
                DealSpec::Draw { max } => u64::from(max) + 1,
                _ => 1,
            };
            StreetPlan {
                label: street.label,
                buckets: buckets
                    .and_then(|table| table.get(index).copied())
                    .unwrap_or(1),
                bet_sequences,
                draw_options,
            }
        })
        .collect();
    GamePlan {
        game_id: spec.id,
        streets,
    }
}

/// Plans for every game in the registry.
pub fn all_plans() -> Vec<GamePlan> {
    use poker_wire::game::Stakes;
    let stakes = Stakes::Blinds {
        small_blind: 50,
        big_blind: 100,
        ante: 0,
    };
    GameSpec::known_ids()
        .iter()
        .map(|id| plan(&GameSpec::by_id(id, stakes).expect("registry id")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_limit_street_has_17_sequences() {
        // kk, plus 8 continuations after either player's opening bet.
        assert_eq!(bet_sequences(1, 4, false), 17);
    }

    #[test]
    fn big_bet_street_has_25_sequences() {
        assert_eq!(bet_sequences(2, 2, false), 25);
    }

    #[test]
    fn bring_in_street_has_16_sequences() {
        assert_eq!(bet_sequences(1, 4, true), 16);
    }

    #[test]
    fn buckets_partition_the_unit_interval() {
        assert_eq!(bucket_of(0.0, 10), 0);
        assert_eq!(bucket_of(0.05, 10), 0);
        assert_eq!(bucket_of(0.55, 10), 5);
        assert_eq!(bucket_of(1.0, 10), 9);
        assert_eq!(bucket_of(2.0, 10), 9, "clamped above");
        assert_eq!(bucket_of(-1.0, 10), 0, "clamped below");
    }

    #[test]
    fn every_game_has_an_explicit_bucket_table() {
        for id in GameSpec::known_ids() {
            assert!(bucket_table(id).is_some(), "{id} missing bucket table");
        }
    }

    #[test]
    fn bucket_tables_cover_every_bucketed_street() {
        // Each table must be exactly as long as the game's street list, so
        // a registry change can't silently leave streets unbucketed.
        for plan in all_plans() {
            let table = bucket_table(plan.game_id).unwrap();
            assert_eq!(
                table.len(),
                plan.streets.len(),
                "{}: bucket table length != street count",
                plan.game_id
            );
        }
    }

    #[test]
    fn every_game_fits_the_tree_budget() {
        for plan in all_plans() {
            let size = plan.tree_size();
            assert!(
                size <= TREE_BUDGET,
                "{}: abstract tree {size:.3e} exceeds 1e12",
                plan.game_id
            );
            assert!(
                size >= 1e6,
                "{}: abstract tree {size:.3e} is degenerately small",
                plan.game_id
            );
        }
    }
}

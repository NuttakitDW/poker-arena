//! Pot construction and awarding.
//!
//! Self-contained chip arithmetic: given what each seat put in and who
//! folded, build main/side pots; given showdown values, award them. The
//! state machine is the only intended caller, but the functions are public
//! for analysis tools and direct testing.

use super::action::{Chips, Seat};
use super::event::PotSide;
use crate::eval::HandValue;

/// One pot layer. `eligible` are the non-folded seats who contributed to
/// this layer (folded chips are in `amount` but folded seats are never
/// eligible to win).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pot {
    pub amount: Chips,
    pub eligible: Vec<Seat>,
}

/// Build pots from total per-seat contributions (over the whole hand) and
/// fold flags.
///
/// Contract:
/// - Layered by distinct contribution levels of *eligible* (non-folded)
///   seats, ascending: the main pot takes each seat's chips up to the lowest
///   all-in level, and so on. Pot index 0 is the main pot.
/// - Folded seats' chips join the layer(s) their contribution reaches, but
///   folded seats never appear in `eligible`.
/// - `sum(pots.amount) == sum(contributions)` always.
/// - A layer with exactly one eligible seat still forms a pot (the excess
///   wager is "returned" to that seat by winning it uncontested at award
///   time; the state machine may alternatively refund uncalled bets before
///   calling this — see `state.rs`, which refunds uncalled excess first so
///   logs match real-table behavior).
pub fn build_pots(contributions: &[Chips], folded: &[bool]) -> Vec<Pot> {
    debug_assert_eq!(
        contributions.len(),
        folded.len(),
        "contributions/folded length mismatch"
    );
    let n = contributions.len();

    // Distinct contribution levels of non-folded seats, ascending. These are
    // the only layer boundaries; folded seats never introduce a new level,
    // they just pour chips into whichever layers their contribution reaches.
    let mut levels: Vec<Chips> = (0..n)
        .filter(|&s| !folded[s])
        .map(|s| contributions[s])
        .collect();
    levels.sort_unstable();
    levels.dedup();

    if levels.is_empty() {
        // No non-folded seat at all: not a real hand state, but conserve
        // chips anyway rather than silently dropping them.
        let total: Chips = contributions.iter().sum();
        return if total > 0 {
            vec![Pot {
                amount: total,
                eligible: Vec::new(),
            }]
        } else {
            Vec::new()
        };
    }

    let mut pots = Vec::with_capacity(levels.len());
    let mut lower = 0;
    for (i, &level) in levels.iter().enumerate() {
        let is_top = i == levels.len() - 1;
        // Every layer but the top is capped at `level`; the top layer is
        // uncapped so that any contribution above the highest eligible
        // level (only possible from a folded seat that bet more than the
        // best remaining hand) still lands somewhere, preserving
        // sum(pots) == sum(contributions).
        let amount: Chips = (0..n)
            .map(|s| {
                let c = contributions[s].saturating_sub(lower);
                if is_top { c } else { c.min(level - lower) }
            })
            .sum();
        if amount > 0 {
            let eligible: Vec<Seat> = (0..n)
                .filter(|&s| !folded[s] && contributions[s] >= level)
                .collect();
            pots.push(Pot { amount, eligible });
        }
        lower = level;
    }
    pots
}

/// Showdown inputs for one seat still in at the end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShowdownEntry {
    pub seat: Seat,
    /// Qualifying high value, if any. Total evaluators (everything but
    /// archie's sixes-or-better) always produce `Some`.
    pub hi: Option<HandValue>,
    /// Qualifying low value, if the game has one and this hand qualifies.
    pub lo: Option<HandValue>,
}

/// One pot side handed to winners: `winners` lists each winning seat with
/// the exact chips it receives (odd chips already resolved).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PotAward {
    pub pot: u8,
    pub side: PotSide,
    pub winners: Vec<(Seat, Chips)>,
}

/// Award every pot.
///
/// Contract:
/// - `hands` contains exactly the non-folded seats. Seats missing from
///   `hands` never win anything.
/// - Per pot, the *candidates* are its eligible entries with a qualifying
///   value on that side: hi candidates have `hi.is_some()`, lo candidates
///   have `lo.is_some()` (and only exist when `has_low`). Total evaluators
///   always qualify, so a side is candidate-less only for a qualifier kind
///   nobody cleared (`EightOrBetterLow`, `SixesOrBetterHigh`).
/// - Both sides have candidates: the pot splits hi/lo with the *odd chip to
///   the hi side*; each side then resolves ties as below.
/// - Exactly one side has candidates: that side scoops the pot with
///   `PotSide::Whole` — the hi-only games' ordinary path, and equally the
///   omaha8/stud8 "no qualifying low" path.
/// - *Neither* side has candidates (possible only when both kinds are
///   qualifiers, i.e. archie): the whole pot splits evenly among **all**
///   eligible entries, remainders by `odd_chip_order` like any other split.
/// - Ties within a side split evenly; remainder chips (at most
///   `winners - 1`) go one each to the earliest tied winners in
///   `odd_chip_order`.
/// - `odd_chip_order`: all seats, in clockwise order starting left of the
///   button; used for every remainder distribution.
/// - Total awarded == total pot amounts, always, chip for chip.
pub fn award_pots(
    pots: &[Pot],
    hands: &[ShowdownEntry],
    has_low: bool,
    odd_chip_order: &[Seat],
) -> Vec<PotAward> {
    let mut awards = Vec::new();
    for (i, pot) in pots.iter().enumerate() {
        let pot_idx = i as u8;
        let eligible: Vec<&ShowdownEntry> = hands
            .iter()
            .filter(|h| pot.eligible.contains(&h.seat))
            .collect();
        assert!(
            !eligible.is_empty(),
            "pot {pot_idx} has no eligible hand at showdown (engine invariant violated)"
        );

        let hi_entries: Vec<(Seat, HandValue)> = eligible
            .iter()
            .filter_map(|h| h.hi.map(|hi| (h.seat, hi)))
            .collect();
        let lo_entries: Vec<(Seat, HandValue)> = if has_low {
            eligible
                .iter()
                .filter_map(|h| h.lo.map(|lo| (h.seat, lo)))
                .collect()
        } else {
            Vec::new()
        };

        let mut whole = |winners: Vec<Seat>| {
            awards.push(PotAward {
                pot: pot_idx,
                side: PotSide::Whole,
                winners: split_amount(pot.amount, &winners, odd_chip_order),
            });
        };

        match (hi_entries.is_empty(), lo_entries.is_empty()) {
            // Both sides contested: split with the odd chip going to hi.
            (false, false) => {
                let lo_half = pot.amount / 2;
                let hi_half = pot.amount - lo_half;
                awards.push(PotAward {
                    pot: pot_idx,
                    side: PotSide::Hi,
                    winners: split_amount(hi_half, &tie_winners(&hi_entries), odd_chip_order),
                });
                awards.push(PotAward {
                    pot: pot_idx,
                    side: PotSide::Lo,
                    winners: split_amount(lo_half, &tie_winners(&lo_entries), odd_chip_order),
                });
            }
            (false, true) => whole(tie_winners(&hi_entries)),
            (true, false) => whole(tie_winners(&lo_entries)),
            // Nobody cleared either qualifier: everyone still standing gets
            // their share back rather than the pot evaporating.
            (true, true) => whole(eligible.iter().map(|h| h.seat).collect()),
        }
    }
    awards
}

/// Seats holding the best (max) value among `entries`; ties included.
fn tie_winners(entries: &[(Seat, HandValue)]) -> Vec<Seat> {
    let best = entries
        .iter()
        .map(|&(_, v)| v)
        .max()
        .expect("tie_winners called with no entries");
    entries
        .iter()
        .filter(|&&(_, v)| v == best)
        .map(|&(s, _)| s)
        .collect()
}

/// Split `amount` evenly across `winners`; the remainder (< winners.len())
/// goes one chip each to the seats appearing earliest in `odd_chip_order`.
/// Result is sorted by seat for a deterministic, easily-asserted order.
fn split_amount(amount: Chips, winners: &[Seat], odd_chip_order: &[Seat]) -> Vec<(Seat, Chips)> {
    let count = winners.len() as Chips;
    let base = amount / count;
    let remainder = (amount % count) as usize;
    let bonus_seats: Vec<Seat> = odd_chip_order
        .iter()
        .copied()
        .filter(|s| winners.contains(s))
        .take(remainder)
        .collect();
    let mut result: Vec<(Seat, Chips)> = winners
        .iter()
        .map(|&s| {
            let amt = if bonus_seats.contains(&s) {
                base + 1
            } else {
                base
            };
            (s, amt)
        })
        .collect();
    result.sort_unstable_by_key(|&(s, _)| s);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pot(amount: Chips, eligible: &[Seat]) -> Pot {
        Pot {
            amount,
            eligible: eligible.to_vec(),
        }
    }

    fn hi(seat: Seat, v: u32) -> ShowdownEntry {
        ShowdownEntry {
            seat,
            hi: Some(HandValue(v)),
            lo: None,
        }
    }

    fn hilo(seat: Seat, hi_v: u32, lo_v: u32) -> ShowdownEntry {
        ShowdownEntry {
            seat,
            hi: Some(HandValue(hi_v)),
            lo: Some(HandValue(lo_v)),
        }
    }

    /// A hand that qualifies for neither side (archie's "no sixes, no
    /// eight-low" case).
    fn neither(seat: Seat) -> ShowdownEntry {
        ShowdownEntry {
            seat,
            hi: None,
            lo: None,
        }
    }

    /// A hand with a qualifying low but no qualifying high.
    fn lo_only(seat: Seat, lo_v: u32) -> ShowdownEntry {
        ShowdownEntry {
            seat,
            hi: None,
            lo: Some(HandValue(lo_v)),
        }
    }

    // ---- build_pots ----------------------------------------------------

    #[test]
    fn no_all_ins_single_pot() {
        let pots = build_pots(&[100, 100, 100], &[false, false, false]);
        assert_eq!(pots, vec![pot(300, &[0, 1, 2])]);
    }

    #[test]
    fn classic_three_way_ladder() {
        let pots = build_pots(&[100, 250, 600], &[false, false, false]);
        assert_eq!(
            pots,
            vec![pot(300, &[0, 1, 2]), pot(300, &[1, 2]), pot(350, &[2])]
        );
        assert_eq!(pots.iter().map(|p| p.amount).sum::<Chips>(), 950);
    }

    #[test]
    fn folded_chips_included_but_not_eligible() {
        // seat 2 folded after putting in 50; seats 0/1 both reached 100.
        let pots = build_pots(&[100, 100, 50], &[false, false, true]);
        assert_eq!(pots, vec![pot(250, &[0, 1])]);
    }

    #[test]
    fn folded_contribution_spans_multiple_layers() {
        // seat 3 folded with 300, straddling all three eligible levels.
        let pots = build_pots(&[100, 250, 600, 300], &[false, false, false, true]);
        assert_eq!(
            pots,
            vec![pot(400, &[0, 1, 2]), pot(450, &[1, 2]), pot(400, &[2]),]
        );
        let total: Chips = pots.iter().map(|p| p.amount).sum();
        assert_eq!(total, 100 + 250 + 600 + 300);
    }

    #[test]
    fn folded_contribution_above_top_level_still_conserved() {
        // seat 3 folded with 700, more than the best remaining hand (600).
        // The excess above 600 must still land in the top pot.
        let pots = build_pots(&[100, 250, 600, 700], &[false, false, false, true]);
        assert_eq!(
            pots,
            vec![pot(400, &[0, 1, 2]), pot(450, &[1, 2]), pot(800, &[2]),]
        );
        let total: Chips = pots.iter().map(|p| p.amount).sum();
        assert_eq!(total, 100 + 250 + 600 + 700);
    }

    #[test]
    fn zero_contribution_seat_excluded() {
        let pots = build_pots(&[0, 100, 100], &[false, false, false]);
        assert_eq!(pots, vec![pot(200, &[1, 2])]);
    }

    #[test]
    fn everyone_folded_except_one_seat() {
        let pots = build_pots(&[50, 30, 20], &[true, true, false]);
        assert_eq!(pots, vec![pot(100, &[2])]);
    }

    #[test]
    fn all_zero_contributions_yield_no_pots() {
        let pots = build_pots(&[0, 0, 0], &[false, false, false]);
        assert!(pots.is_empty());
    }

    #[test]
    fn one_seat_contributes_more_than_all_others() {
        let pots = build_pots(&[500, 100, 100], &[false, false, false]);
        assert_eq!(pots, vec![pot(300, &[0, 1, 2]), pot(400, &[0])]);
    }

    // ---- conservation property ------------------------------------------

    /// Enumerate every base-`values.len()` digit string of length `n`,
    /// calling `f` with the resulting index vector. Deterministic, no RNG.
    fn for_each_combo(n: usize, base: usize, mut f: impl FnMut(&[usize])) {
        let mut idx = vec![0usize; n];
        loop {
            f(&idx);
            let mut pos = 0;
            loop {
                if pos == n {
                    return;
                }
                idx[pos] += 1;
                if idx[pos] < base {
                    break;
                }
                idx[pos] = 0;
                pos += 1;
            }
        }
    }

    fn check_conservation(contributions: &[Chips], folded: &[bool]) {
        let pots = build_pots(contributions, folded);
        let total_in: Chips = contributions.iter().sum();
        let total_out: Chips = pots.iter().map(|p| p.amount).sum();
        assert_eq!(
            total_out, total_in,
            "contributions={contributions:?} folded={folded:?} pots={pots:?}"
        );
        for p in &pots {
            assert!(p.amount > 0, "zero-amount pot leaked through: {p:?}");
            for &s in &p.eligible {
                assert!(!folded[s], "folded seat {s} eligible in {p:?}");
            }
        }
        // Eligibility must be non-increasing (later pots' eligible sets are
        // subsets of earlier ones) since layers are ascending by level.
        for w in pots.windows(2) {
            for &s in &w[1].eligible {
                assert!(
                    w[0].eligible.contains(&s),
                    "seat {s} eligible for later pot but not earlier: {pots:?}"
                );
            }
        }
    }

    #[test]
    fn conservation_grid() {
        let values: [Chips; 5] = [0, 50, 100, 250, 600];

        for n in 2..=4usize {
            for_each_combo(n, values.len(), |contrib_idx| {
                let contributions: Vec<Chips> = contrib_idx.iter().map(|&i| values[i]).collect();
                for_each_combo(n, 2, |fold_idx| {
                    let folded: Vec<bool> = fold_idx.iter().map(|&i| i == 1).collect();
                    if folded.iter().all(|&f| f) {
                        return; // need at least one non-folded seat
                    }
                    check_conservation(&contributions, &folded);
                });
            });
        }

        // 5 seats: same check with a smaller value set to keep this fast.
        let small_values: [Chips; 3] = [0, 100, 600];
        for_each_combo(5, small_values.len(), |contrib_idx| {
            let contributions: Vec<Chips> = contrib_idx.iter().map(|&i| small_values[i]).collect();
            for_each_combo(5, 2, |fold_idx| {
                let folded: Vec<bool> = fold_idx.iter().map(|&i| i == 1).collect();
                if folded.iter().all(|&f| f) {
                    return;
                }
                check_conservation(&contributions, &folded);
            });
        });
    }

    // ---- award_pots ------------------------------------------------------

    #[test]
    fn single_winner() {
        let pots = vec![pot(300, &[0, 1, 2])];
        let hands = vec![hi(0, 10), hi(1, 30), hi(2, 20)];
        let awards = award_pots(&pots, &hands, false, &[0, 1, 2]);
        assert_eq!(
            awards,
            vec![PotAward {
                pot: 0,
                side: PotSide::Whole,
                winners: vec![(1, 300)],
            }]
        );
    }

    #[test]
    fn two_way_tie_even_split() {
        let pots = vec![pot(300, &[0, 1, 2])];
        let hands = vec![hi(0, 10), hi(1, 30), hi(2, 30)];
        let awards = award_pots(&pots, &hands, false, &[0, 1, 2]);
        assert_eq!(
            awards,
            vec![PotAward {
                pot: 0,
                side: PotSide::Whole,
                winners: vec![(1, 150), (2, 150)],
            }]
        );
    }

    #[test]
    fn three_way_tie_remainder_to_earliest_in_order() {
        // 3-way tie on 100 chips: base 33, remainder 1 -> first tied seat
        // in odd_chip_order gets the extra chip.
        let pots = vec![pot(100, &[0, 1, 2])];
        let hands = vec![hi(0, 10), hi(1, 10), hi(2, 10)];
        let awards = award_pots(&pots, &hands, false, &[2, 0, 1]);
        assert_eq!(
            awards,
            vec![PotAward {
                pot: 0,
                side: PotSide::Whole,
                winners: vec![(0, 33), (1, 33), (2, 34)],
            }]
        );

        // Remainder of 2 across 3 winners: the earliest two in
        // odd_chip_order get the extra chip.
        let pots = vec![pot(101, &[0, 1, 2])];
        let awards = award_pots(&pots, &hands, false, &[2, 0, 1]);
        assert_eq!(
            awards,
            vec![PotAward {
                pot: 0,
                side: PotSide::Whole,
                winners: vec![(0, 34), (1, 33), (2, 34)],
            }]
        );
    }

    #[test]
    fn side_pot_excludes_main_pot_winner() {
        // Seat 0 is short-stacked and wins the main pot outright, but is not
        // eligible for the side pot, which goes to the best of seats 1/2.
        let pots = vec![pot(150, &[0, 1, 2]), pot(200, &[1, 2])];
        let hands = vec![hi(0, 100), hi(1, 10), hi(2, 20)];
        let awards = award_pots(&pots, &hands, false, &[0, 1, 2]);
        assert_eq!(
            awards,
            vec![
                PotAward {
                    pot: 0,
                    side: PotSide::Whole,
                    winners: vec![(0, 150)],
                },
                PotAward {
                    pot: 1,
                    side: PotSide::Whole,
                    winners: vec![(2, 200)],
                },
            ]
        );
    }

    #[test]
    fn hilo_odd_amount_hi_gets_extra_chip() {
        let pots = vec![pot(101, &[0, 1])];
        let hands = vec![hilo(0, 100, 5), hilo(1, 50, 10)];
        let awards = award_pots(&pots, &hands, true, &[0, 1]);
        assert_eq!(
            awards,
            vec![
                PotAward {
                    pot: 0,
                    side: PotSide::Hi,
                    winners: vec![(0, 51)],
                },
                PotAward {
                    pot: 0,
                    side: PotSide::Lo,
                    winners: vec![(1, 50)],
                },
            ]
        );
    }

    #[test]
    fn hilo_no_qualifying_low_awards_whole() {
        let pots = vec![pot(100, &[0, 1])];
        let hands = vec![hi(0, 100), hi(1, 50)];
        let awards = award_pots(&pots, &hands, true, &[0, 1]);
        assert_eq!(
            awards,
            vec![PotAward {
                pot: 0,
                side: PotSide::Whole,
                winners: vec![(0, 100)],
            }]
        );
    }

    #[test]
    fn hilo_tied_lows_quarter_the_pot() {
        // 3 players, 100-chip pot: two share the low, one has the only hi.
        // hi half = 50 (all to seat 0); lo half = 50 split 25/25.
        let pots = vec![pot(100, &[0, 1, 2])];
        let hands = vec![hilo(0, 100, 1), hilo(1, 10, 5), hilo(2, 20, 5)];
        let awards = award_pots(&pots, &hands, true, &[0, 1, 2]);
        assert_eq!(
            awards,
            vec![
                PotAward {
                    pot: 0,
                    side: PotSide::Hi,
                    winners: vec![(0, 50)],
                },
                PotAward {
                    pot: 0,
                    side: PotSide::Lo,
                    winners: vec![(1, 25), (2, 25)],
                },
            ]
        );
        let total: Chips = awards
            .iter()
            .flat_map(|a| a.winners.iter().map(|&(_, c)| c))
            .sum();
        assert_eq!(total, 100);
    }

    #[test]
    fn multi_pot_hilo() {
        // Seat 0 is short-stacked with the only qualifying low, eligible
        // only for the main pot. Seats 1/2 are deeper, both eligible for
        // the side pot, and neither qualifies for low there, so the side
        // pot goes whole to the best hi.
        let pots = vec![pot(150, &[0, 1, 2]), pot(100, &[1, 2])];
        let hands = vec![hilo(0, 90, 30), hi(1, 100), hi(2, 50)];
        let awards = award_pots(&pots, &hands, true, &[0, 1, 2]);
        assert_eq!(
            awards,
            vec![
                PotAward {
                    pot: 0,
                    side: PotSide::Hi,
                    winners: vec![(1, 75)],
                },
                PotAward {
                    pot: 0,
                    side: PotSide::Lo,
                    winners: vec![(0, 75)],
                },
                PotAward {
                    pot: 1,
                    side: PotSide::Whole,
                    winners: vec![(1, 100)],
                },
            ]
        );
        let total: Chips = awards
            .iter()
            .flat_map(|a| a.winners.iter().map(|&(_, c)| c))
            .sum();
        assert_eq!(total, 250);
    }

    // ---- qualifier-only sides (archie) -----------------------------------

    #[test]
    fn only_the_low_qualifies_and_scoops_the_whole_pot() {
        let pots = vec![pot(100, &[0, 1, 2])];
        let hands = vec![neither(0), lo_only(1, 30), lo_only(2, 10)];
        let awards = award_pots(&pots, &hands, true, &[0, 1, 2]);
        assert_eq!(
            awards,
            vec![PotAward {
                pot: 0,
                side: PotSide::Whole,
                winners: vec![(1, 100)],
            }]
        );
    }

    #[test]
    fn only_the_high_qualifies_and_scoops_the_whole_pot() {
        let pots = vec![pot(100, &[0, 1])];
        let hands = vec![hi(0, 50), neither(1)];
        let awards = award_pots(&pots, &hands, true, &[0, 1]);
        assert_eq!(
            awards,
            vec![PotAward {
                pot: 0,
                side: PotSide::Whole,
                winners: vec![(0, 100)],
            }]
        );
    }

    #[test]
    fn neither_side_qualifies_splits_evenly_among_all_eligible() {
        // 100 chips, three qualifier-less hands: 33 each with the odd chip
        // to the earliest seat in `odd_chip_order`.
        let pots = vec![pot(100, &[0, 1, 2])];
        let hands = vec![neither(0), neither(1), neither(2)];
        let awards = award_pots(&pots, &hands, true, &[2, 0, 1]);
        assert_eq!(
            awards,
            vec![PotAward {
                pot: 0,
                side: PotSide::Whole,
                winners: vec![(0, 33), (1, 33), (2, 34)],
            }]
        );
        let total: Chips = awards
            .iter()
            .flat_map(|a| a.winners.iter().map(|&(_, c)| c))
            .sum();
        assert_eq!(total, 100);
    }

    #[test]
    fn qualifier_sides_are_decided_per_pot() {
        // Seat 0's qualifying high is locked into the main pot; the side pot
        // has no qualifier at all and splits evenly between seats 1 and 2.
        let pots = vec![pot(150, &[0, 1, 2]), pot(101, &[1, 2])];
        let hands = vec![hi(0, 50), neither(1), lo_only(2, 7)];
        let awards = award_pots(&pots, &hands, true, &[1, 2, 0]);
        assert_eq!(
            awards,
            vec![
                PotAward {
                    pot: 0,
                    side: PotSide::Hi,
                    winners: vec![(0, 75)],
                },
                PotAward {
                    pot: 0,
                    side: PotSide::Lo,
                    winners: vec![(2, 75)],
                },
                PotAward {
                    pot: 1,
                    side: PotSide::Whole,
                    winners: vec![(2, 101)],
                },
            ]
        );
        let total: Chips = awards
            .iter()
            .flat_map(|a| a.winners.iter().map(|&(_, c)| c))
            .sum();
        assert_eq!(total, 251);
    }

    #[test]
    fn no_low_game_with_no_hi_qualifier_still_conserves_chips() {
        // Defensively unreachable with the current specs (no hi-only game
        // uses a qualifier kind), but the even split must hold anyway.
        let pots = vec![pot(7, &[0, 1])];
        let hands = vec![neither(0), neither(1)];
        let awards = award_pots(&pots, &hands, false, &[1, 0]);
        assert_eq!(
            awards,
            vec![PotAward {
                pot: 0,
                side: PotSide::Whole,
                winners: vec![(0, 3), (1, 4)],
            }]
        );
    }

    #[test]
    #[should_panic(expected = "engine invariant")]
    fn empty_eligible_set_panics() {
        let pots = vec![pot(100, &[5])];
        let hands = vec![hi(0, 10)];
        award_pots(&pots, &hands, false, &[0]);
    }
}

//! 2-7 lowball lossless abstraction layers.
//!
//! Two reductions with different guarantees:
//!
//! 1. **Suit isomorphism** ([`crate::iso`]) is *strictly* lossless: the
//!    only automorphisms of a 2-7 game are the 4! suit permutations (any
//!    other card permutation changes some hand's straight/flush status),
//!    so merging suit-isomorphic states provably preserves every
//!    equilibrium. C(52,5) = 2,598,960 deals collapse to 134,459 classes.
//!
//! 2. **The 2-7 value class** ([`deuce_class`]): the evaluator reads only
//!    the rank multiset plus whether the hand is a flush, collapsing
//!    134,459 iso classes to 7,462 (6,175 rank multisets + 1,287 flush
//!    variants — only all-distinct-rank hands can be flushes). This is
//!    exactly lossless for *showdown value* — every hand in a class has
//!    the identical `deuce_to_seven_low` value — but merging *information
//!    sets* on it is epsilon-lossy: two same-class hands with different
//!    suit patterns block different opponent flushes (holding 2+2+1 of
//!    three suits leaves Σ C(13−aᵢ,5) = 3,003 opponent flush combos;
//!    2+1+1+1 leaves 2,838), a card-removal effect worth ~1e-4 of win
//!    probability. [`blocker_epsilon`] measures it exactly by full
//!    enumeration so the trade-off is a number, not a guess.

use std::collections::HashMap;
use std::sync::OnceLock;

use poker_core::card::Card;
use poker_core::eval::deuce_to_seven_low;

/// The 2-7 value class of a 5-card hand, packed into a `u32`: five 4-bit
/// rank indices sorted descending (a canonical rank-multiset encoding)
/// with bit 20 set when the hand is a flush. Hands share a class iff they
/// share a `deuce_to_seven_low` value.
pub fn deuce_class(cards: &[Card]) -> u32 {
    debug_assert_eq!(cards.len(), 5, "2-7 value classes are 5-card");
    let mut ranks: Vec<u8> = cards.iter().map(|card| card.rank().index()).collect();
    ranks.sort_unstable_by(|a, b| b.cmp(a));
    let mut key = 0u32;
    for rank in ranks {
        key = (key << 4) | u32::from(rank);
    }
    let flush = cards.iter().all(|card| card.suit() == cards[0].suit());
    if flush {
        key |= 1 << 20;
    }
    key
}

/// Every 5-card combination of the deck, streamed to `visit`.
pub fn for_each_hand(mut visit: impl FnMut(&[Card; 5])) {
    let deck: Vec<Card> = (0..52).filter_map(Card::from_index).collect();
    for a in 0..48 {
        for b in (a + 1)..49 {
            for c in (b + 1)..50 {
                for d in (c + 1)..51 {
                    for e in (d + 1)..52 {
                        visit(&[deck[a], deck[b], deck[c], deck[d], deck[e]]);
                    }
                }
            }
        }
    }
}

/// Exact P(win) + ½·P(tie) for `hand` at a 2-7 showdown against a uniform
/// random 5-card opponent hand from the remaining 47 cards, by full
/// enumeration of all C(47,5) = 1,533,939 opponent hands.
pub fn exact_win_probability(hand: &[Card; 5]) -> f64 {
    let ours = deuce_to_seven_low(hand);
    let rest: Vec<Card> = (0..52)
        .filter_map(Card::from_index)
        .filter(|card| !hand.contains(card))
        .collect();
    debug_assert_eq!(rest.len(), 47);

    let mut score = 0.0f64;
    let mut total = 0u64;
    for a in 0..43 {
        for b in (a + 1)..44 {
            for c in (b + 1)..45 {
                for d in (c + 1)..46 {
                    for e in (d + 1)..47 {
                        let theirs =
                            deuce_to_seven_low(&[rest[a], rest[b], rest[c], rest[d], rest[e]]);
                        if ours > theirs {
                            score += 1.0;
                        } else if ours == theirs {
                            score += 0.5;
                        }
                        total += 1;
                    }
                }
            }
        }
    }
    score / total as f64
}

/// One 2-7 value class in the exact equity table.
#[derive(Clone, Debug)]
pub struct ClassRow {
    /// The [`deuce_class`] key.
    pub class: u32,
    /// The frozen `deuce_to_seven_low` encoding shared by every hand in
    /// the class (greater = better).
    pub value: u32,
    /// Number of raw 5-card hands in the class.
    pub count: u32,
    /// Exact P(win) + ½·P(tie) against a hand drawn uniformly from all
    /// C(52,5) deals — the class's count-weighted percentile, computable
    /// in one enumeration pass because `deuce_to_seven_low` totally
    /// orders classes. Card removal is deliberately ignored: versus a
    /// removal-aware C(47,5) enumeration the value shifts by up to ~1.5%
    /// (a hand's own middling ranks block nearby opponent hands), but
    /// the shift is *systematic and order-preserving* — the table is a
    /// strictly monotone transform of true hand strength, which is
    /// exactly the property equity bucketing needs, with zero sampling
    /// noise (the Monte-Carlo path it replaces carried ±9% noise at 32
    /// rollouts).
    pub equity: f64,
}

/// The exact class-equity table: 7,462 rows, one per 2-7 value class.
///
/// Replaces Monte-Carlo rollouts for hand-strength queries in games whose
/// showdown is pure 2-7 (`27td-fl`, `27sd-nl`): a hand's equity becomes a
/// key computation plus a map lookup (~ns) instead of 32 rollouts (~µs),
/// and is exact rather than ±9% sampling noise.
pub struct EquityTable {
    by_class: HashMap<u32, f64>,
    rows: Vec<ClassRow>,
}

impl EquityTable {
    /// Build by full enumeration of all C(52,5) hands (a few seconds).
    pub fn build() -> EquityTable {
        let mut acc: HashMap<u32, (u32, u32)> = HashMap::new();
        for_each_hand(|hand| {
            let class = deuce_class(hand);
            let entry = acc
                .entry(class)
                .or_insert_with(|| (deuce_to_seven_low(hand).0, 0));
            entry.1 += 1;
        });

        let mut rows: Vec<ClassRow> = acc
            .into_iter()
            .map(|(class, (value, count))| ClassRow {
                class,
                value,
                count,
                equity: 0.0,
            })
            .collect();
        rows.sort_unstable_by_key(|row| row.value);

        let total: f64 = rows.iter().map(|row| f64::from(row.count)).sum();
        let mut beaten = 0.0f64;
        for row in &mut rows {
            let ties = f64::from(row.count);
            row.equity = (beaten + 0.5 * ties) / total;
            beaten += ties;
        }

        let by_class = rows.iter().map(|row| (row.class, row.equity)).collect();
        EquityTable { by_class, rows }
    }

    /// The process-wide table, built once on first use.
    pub fn shared() -> &'static EquityTable {
        static TABLE: OnceLock<EquityTable> = OnceLock::new();
        TABLE.get_or_init(EquityTable::build)
    }

    /// Exact equity of a 5-card hand vs a uniform random hand; `None` for
    /// hand sizes the table does not cover.
    pub fn equity(&self, cards: &[Card]) -> Option<f64> {
        if cards.len() != 5 {
            return None;
        }
        self.by_class.get(&deuce_class(cards)).copied()
    }

    /// All rows, ordered worst to best.
    pub fn rows(&self) -> &[ClassRow] {
        &self.rows
    }
}

/// The measured cost of merging information sets on [`deuce_class`]: for
/// each probe rank-multiset, compare the exact win probability of two
/// same-class hands with maximally different suit patterns and return the
/// per-pair deltas. The max over these is the blocker epsilon.
pub fn blocker_epsilon() -> Vec<(String, f64)> {
    use poker_core::card::parse_cards;
    // (clustered-suit variant, spread-suit variant) — same ranks, same
    // class, different card-removal footprints.
    let pairs = [
        ("smooth 7-low", "2c 3c 4d 5d 7h", "2c 3d 4h 5s 7c"),
        ("rough jack", "8c 9c Td Jd 2h", "8c 9d Th Js 2c"),
        ("paired", "2c 2d 4c 6c 8c", "2c 2d 4h 6s 8d"),
        ("king high", "Kc Qc 9d 5d 3h", "Kc Qd 9h 5s 3c"),
    ];
    pairs
        .iter()
        .map(|(label, clustered, spread)| {
            let a: [Card; 5] = parse_cards(clustered).unwrap().try_into().unwrap();
            let b: [Card; 5] = parse_cards(spread).unwrap().try_into().unwrap();
            debug_assert_eq!(deuce_class(&a), deuce_class(&b), "{label}: same class");
            let delta = (exact_win_probability(&a) - exact_win_probability(&b)).abs();
            (label.to_string(), delta)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::iso::canonical_key;
    use poker_core::card::parse_cards;
    use std::collections::HashSet;

    /// One pass over all C(52,5) hands pins every enumeration claim at
    /// once: the suit-iso class count, the 2-7 class count and its split,
    /// and that a class determines the 2-7 value exactly.
    #[test]
    fn full_enumeration_pins_the_lossless_counts() {
        let mut iso_classes = HashSet::new();
        let mut value_by_class: HashMap<u32, u32> = HashMap::new();
        let mut hands = 0u64;
        for_each_hand(|hand| {
            hands += 1;
            iso_classes.insert(canonical_key(&[hand]));
            let class = deuce_class(hand);
            let value = deuce_to_seven_low(hand).0;
            let seen = value_by_class.entry(class).or_insert(value);
            assert_eq!(*seen, value, "class must determine the 2-7 value");
        });
        assert_eq!(hands, 2_598_960, "C(52,5)");
        assert_eq!(iso_classes.len(), 134_459, "suit-isomorphic classes");
        assert_eq!(value_by_class.len(), 7_462, "2-7 value classes");

        let flushes = value_by_class
            .keys()
            .filter(|class| *class & (1 << 20) != 0)
            .count();
        assert_eq!(flushes, 1_287, "flushable = C(13,5) distinct-rank sets");
        assert_eq!(value_by_class.len() - flushes, 6_175, "rank multisets");
    }

    #[test]
    fn equity_table_is_complete_monotone_and_anchored() {
        let table = EquityTable::build();
        assert_eq!(table.rows().len(), 7_462);

        // Rows are value-sorted; equity must strictly increase with value
        // (every class has distinct value, so no plateaus).
        for pair in table.rows().windows(2) {
            assert!(pair[1].value > pair[0].value);
            assert!(pair[1].equity > pair[0].equity);
        }

        // Anchors: the nut low 7-5-4-3-2 offsuit sits at the top (short of
        // 1.0 only by half its own class's tie mass, 1020 hands ≈ 0.0002)
        // and the absolute worst 2-7 hand (a royal flush) at the bottom.
        let nuts = parse_cards("7c 5d 4h 3s 2c").unwrap();
        let top = table.equity(&nuts).unwrap();
        let bottom = table.rows()[0].equity;
        assert!(top > 0.9995, "nut low equity was {top}");
        assert!(bottom < 0.0005, "worst-hand equity was {bottom}");

        // Total probability mass: count-weighted mean equity is exactly ½.
        let total: f64 = table.rows().iter().map(|r| f64::from(r.count)).sum();
        let mean: f64 = table
            .rows()
            .iter()
            .map(|r| r.equity * f64::from(r.count))
            .sum::<f64>()
            / total;
        assert!((mean - 0.5).abs() < 1e-12, "mean equity was {mean}");
    }

    #[test]
    fn table_equity_tracks_removal_aware_enumeration_and_preserves_order() {
        // The table ignores card removal; exact_win_probability does not.
        // The deviation is systematic (a hand's own ranks block nearby
        // opponent hands — worst for middling holdings) and stays within
        // a couple of percent; crucially the *ordering* between probes
        // must agree exactly, because bucketing consumes order, not level.
        let table = EquityTable::build();
        let probes = ["2c 3d 4h 5s 7c", "8c 9d Th Js 2c", "Kc Qd 9h 5s 3c"];
        let mut pairs: Vec<(f64, f64)> = Vec::new();
        for probe in probes {
            let hand: [Card; 5] = parse_cards(probe).unwrap().try_into().unwrap();
            let exact = exact_win_probability(&hand);
            let looked_up = table.equity(&hand).unwrap();
            assert!(
                (exact - looked_up).abs() < 0.02,
                "{probe}: exact {exact} vs table {looked_up}"
            );
            pairs.push((exact, looked_up));
        }
        for window in pairs.windows(2) {
            let same_order = (window[0].0 > window[1].0) == (window[0].1 > window[1].1);
            assert!(same_order, "table must preserve the exact ordering");
        }
    }

    #[test]
    fn distinct_values_never_share_a_class() {
        // The converse direction of losslessness: 7,462 classes must carry
        // 7,462 distinct values (the evaluator distinguishes flushes from
        // their rank twins, so class -> value is injective).
        let mut values = HashSet::new();
        let mut classes = HashSet::new();
        for_each_hand(|hand| {
            classes.insert(deuce_class(hand));
            values.insert(deuce_to_seven_low(hand).0);
        });
        assert_eq!(values.len(), classes.len());
    }
}

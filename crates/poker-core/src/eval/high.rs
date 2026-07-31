//! Standard high-hand evaluation. Encoding contract in `eval/mod.rs` docs.

use super::{HandClass, HandValue};
use crate::card::{Card, Rank};

/// Best high hand from 5–7 cards. Panics if `cards.len() < 5` (engine bug,
/// not user input). Must accept duplicate-free input only; behavior with
/// duplicated cards is unspecified.
pub(super) fn evaluate(cards: &[Card]) -> HandValue {
    assert!(cards.len() >= 5, "high::evaluate requires at least 5 cards");
    five_card_combos(cards)
        .into_iter()
        .map(rank_five)
        .max()
        .expect("five_card_combos yields at least one combination")
}

/// Best high hand from 5–7 cards, but only if it clears archie's
/// "sixes or better" bar: any class above [`HandClass::OnePair`], or a pair
/// of Sixes or better. A no-pair hand never qualifies, however high.
///
/// Qualification is monotone in the encoding (class first, then the pair
/// rank in the top tiebreak slot), so testing the *best* hand is the same as
/// asking whether any five-card subset qualifies.
pub(super) fn sixes_or_better(cards: &[Card]) -> Option<HandValue> {
    let best = evaluate(cards);
    let class = best.high_class();
    let qualifies = class > HandClass::OnePair
        || (class == HandClass::OnePair && (best.0 >> 16) & 0xF >= Rank::Six.index() as u32);
    qualifies.then_some(best)
}

/// Rank exactly three cards (OFC's top row) in the same frozen encoding as
/// [`rank_five`], with the unused tiebreak nibbles zero. Only HighCard,
/// OnePair and Trips are reachable: a three-card row has no straights and no
/// flushes. Panics unless given exactly three cards (engine bug, not user
/// input).
///
/// The zero fill is load-bearing, not incidental. Zero is the lowest possible
/// nibble, so a five-card value of the same class whose leading tiebreaks
/// match a three-card value always compares greater or equal to it — and
/// equal exactly when the two rows tie through every rank the shorter hand
/// has. That makes OFC's top-vs-middle foul test a plain `HandValue`
/// comparison with the tie semantics OFC wants (equal rows do not foul),
/// with no separate cross-length comparison rule to keep in sync.
pub(super) fn three_card(cards: &[Card]) -> HandValue {
    assert_eq!(cards.len(), 3, "three_card requires exactly 3 cards");
    let mut ranks: [u8; 3] = [
        cards[0].rank().index(),
        cards[1].rank().index(),
        cards[2].rank().index(),
    ];
    ranks.sort_unstable_by(|a, b| b.cmp(a));

    let (class, tiebreak): (HandClass, [u8; 5]) = if ranks[0] == ranks[2] {
        (HandClass::Trips, [ranks[0], 0, 0, 0, 0])
    } else if ranks[0] == ranks[1] {
        (HandClass::OnePair, [ranks[0], ranks[2], 0, 0, 0])
    } else if ranks[1] == ranks[2] {
        (HandClass::OnePair, [ranks[1], ranks[0], 0, 0, 0])
    } else {
        (HandClass::HighCard, [ranks[0], ranks[1], ranks[2], 0, 0])
    };

    encode(class, tiebreak)
}

/// Rank exactly five cards. Exposed within the eval module for the lowball
/// evaluators (2-7 reuses the high ordering).
pub(super) fn rank_five(cards: [Card; 5]) -> HandValue {
    classify(cards, true)
}

/// Same as [`rank_five`] but without the ace-low "wheel" straight exception.
/// 2-7 lowball treats aces as always high, so `A-2-3-4-5` must classify as
/// an ace-high no-pair hand rather than a straight; this variant is what
/// makes that possible without duplicating the rest of the classification.
pub(super) fn rank_five_no_wheel(cards: [Card; 5]) -> HandValue {
    classify(cards, false)
}

/// Shared classifier behind [`rank_five`] and [`rank_five_no_wheel`].
fn classify(cards: [Card; 5], allow_wheel: bool) -> HandValue {
    let mut ranks: [u8; 5] = cards.map(|c| c.rank().index());
    ranks.sort_unstable_by(|a, b| b.cmp(a));

    let first_suit = cards[0].suit();
    let flush = cards.iter().all(|c| c.suit() == first_suit);

    let straight_high = straight_high_card(&ranks, allow_wheel);

    let mut counts = [0u8; 13];
    for &r in &ranks {
        counts[r as usize] += 1;
    }
    // (count, rank) groups, sorted so the biggest/highest group leads —
    // exactly the order every hand class below needs its tiebreaks in.
    let mut groups: Vec<(u8, u8)> = (0..13u8)
        .filter(|&r| counts[r as usize] > 0)
        .map(|r| (counts[r as usize], r))
        .collect();
    groups.sort_unstable_by(|a, b| b.cmp(a));

    let (class, tiebreak): (HandClass, [u8; 5]) = if let Some(high) = straight_high {
        if flush {
            (HandClass::StraightFlush, [high, 0, 0, 0, 0])
        } else {
            (HandClass::Straight, [high, 0, 0, 0, 0])
        }
    } else if flush {
        (HandClass::Flush, ranks)
    } else {
        match groups.as_slice() {
            [(4, q), (1, k)] => (HandClass::Quads, [*q, *k, 0, 0, 0]),
            [(3, t), (2, p)] => (HandClass::FullHouse, [*t, *p, 0, 0, 0]),
            [(3, t), (1, k1), (1, k2)] => (HandClass::Trips, [*t, *k1, *k2, 0, 0]),
            [(2, p1), (2, p2), (1, k)] => (HandClass::TwoPair, [*p1, *p2, *k, 0, 0]),
            [(2, p), (1, k1), (1, k2), (1, k3)] => (HandClass::OnePair, [*p, *k1, *k2, *k3, 0]),
            [(1, a), (1, b), (1, c), (1, d), (1, e)] => (HandClass::HighCard, [*a, *b, *c, *d, *e]),
            _ => unreachable!("five cards always produce one of the histograms above"),
        }
    };

    encode(class, tiebreak)
}

/// The straight's high rank, or `None` if the five ranks aren't a straight.
/// `ranks_desc` need not be sorted a particular way internally; only the
/// *set* of ranks matters. `allow_wheel` controls whether `A-2-3-4-5` counts
/// (true for high hands, false for 2-7 where aces are never low).
fn straight_high_card(ranks_desc: &[u8; 5], allow_wheel: bool) -> Option<u8> {
    let mut sorted = *ranks_desc;
    sorted.sort_unstable();
    for w in sorted.windows(2) {
        if w[0] == w[1] {
            return None;
        }
    }
    if allow_wheel && sorted == [0, 1, 2, 3, 12] {
        return Some(3); // wheel: high card is the Five
    }
    if sorted[4] - sorted[0] == 4 {
        Some(sorted[4])
    } else {
        None
    }
}

/// Pack a [`HandClass`] and up to five 4-bit tiebreak ranks (most
/// significant first, unused trailing slots zero) into the frozen encoding.
fn encode(class: HandClass, ranks: [u8; 5]) -> HandValue {
    let mut v = (class as u32) << 20;
    for (i, r) in ranks.iter().enumerate() {
        v |= (*r as u32) << (16 - 4 * i);
    }
    HandValue(v)
}

/// All `C(n, 5)` five-card combinations of `cards` (`n` in `5..=7` in
/// practice, but this works for any `n <= 32`).
fn five_card_combos(cards: &[Card]) -> Vec<[Card; 5]> {
    let n = cards.len();
    let mut out = Vec::with_capacity(21);
    for mask in 0u32..(1 << n) {
        if mask.count_ones() != 5 {
            continue;
        }
        let mut combo = [cards[0]; 5];
        let mut idx = 0;
        for (i, &c) in cards.iter().enumerate() {
            if mask & (1 << i) != 0 {
                combo[idx] = c;
                idx += 1;
            }
        }
        out.push(combo);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::parse_cards;
    use crate::eval::{EvalKind, HoleUsage, best_with_usage};

    fn five(s: &str) -> HandValue {
        let cards = parse_cards(s).unwrap();
        rank_five(cards.try_into().unwrap())
    }

    /// Gold-standard sweep: every one of the C(52,5) = 2,598,960 five-card
    /// hands, tallied by class. These are the textbook frequencies.
    #[test]
    fn exhaustive_5_card_frequencies() {
        let mut counts = [0u64; 9];
        for a in 0..52u8 {
            for b in (a + 1)..52u8 {
                for c in (b + 1)..52u8 {
                    for d in (c + 1)..52u8 {
                        for e in (d + 1)..52u8 {
                            let cards = [
                                Card::from_index(a).unwrap(),
                                Card::from_index(b).unwrap(),
                                Card::from_index(c).unwrap(),
                                Card::from_index(d).unwrap(),
                                Card::from_index(e).unwrap(),
                            ];
                            let class = rank_five(cards).high_class();
                            counts[class as usize] += 1;
                        }
                    }
                }
            }
        }

        assert_eq!(counts[HandClass::HighCard as usize], 1_302_540);
        assert_eq!(counts[HandClass::OnePair as usize], 1_098_240);
        assert_eq!(counts[HandClass::TwoPair as usize], 123_552);
        assert_eq!(counts[HandClass::Trips as usize], 54_912);
        assert_eq!(counts[HandClass::Straight as usize], 10_200);
        assert_eq!(counts[HandClass::Flush as usize], 5_108);
        assert_eq!(counts[HandClass::FullHouse as usize], 3_744);
        assert_eq!(counts[HandClass::Quads as usize], 624);
        assert_eq!(counts[HandClass::StraightFlush as usize], 40);
        assert_eq!(counts.iter().sum::<u64>(), 2_598_960);
    }

    #[test]
    fn class_ordering_is_strictly_increasing() {
        let straight_flush = five("6s 5s 4s 3s 2s");
        let quads = five("9c 9d 9h 9s 2c");
        let full_house = five("9c 9d 9h 2s 2c");
        let flush = five("As Ks 8s 6s 2s");
        let straight = five("9c 8d 7h 6s 5c");
        let trips = five("9c 9d 9h Kc 2c");
        let two_pair = five("9c 9d Kh Ks 2c");
        let one_pair = five("9c 9d Kh Qs 2c");
        let high_card = five("As Kd 8h 6s 2c");

        assert!(straight_flush > quads);
        assert!(quads > full_house);
        assert!(full_house > flush);
        assert!(flush > straight);
        assert!(straight > trips);
        assert!(trips > two_pair);
        assert!(two_pair > one_pair);
        assert!(one_pair > high_card);

        assert_eq!(straight_flush.high_class(), HandClass::StraightFlush);
        assert_eq!(quads.high_class(), HandClass::Quads);
        assert_eq!(full_house.high_class(), HandClass::FullHouse);
        assert_eq!(flush.high_class(), HandClass::Flush);
        assert_eq!(straight.high_class(), HandClass::Straight);
        assert_eq!(trips.high_class(), HandClass::Trips);
        assert_eq!(two_pair.high_class(), HandClass::TwoPair);
        assert_eq!(one_pair.high_class(), HandClass::OnePair);
        assert_eq!(high_card.high_class(), HandClass::HighCard);
    }

    #[test]
    fn kicker_ordering_within_class() {
        // One pair: kicker breaks the tie.
        assert!(five("Ac Ad Kc Qd Jh") > five("Ac Ad Kc Qd Th"));
        // Two pair: the higher pair matters more than the kicker.
        assert!(five("Kc Kd 2c 2d Ah") > five("Qc Qd Jc Jd Ah"));
        // Two pair: same pairs, kicker breaks the tie.
        assert!(five("Kc Kd 2c 2d Ah") > five("Kc Kd 2c 2d Qh"));
        // Trips: kickers break the tie.
        assert!(five("9c 9d 9h Ac 3c") > five("9c 9d 9h Kc Qc"));
        // High card: compare down the kicker chain.
        assert!(five("As Kd Qh 6s 2c") > five("As Kd Qh 5s 4c"));
        // Flush: ranks compare like high card (suits don't add a tiebreak).
        assert!(five("As Ks 8s 6s 3s") > five("As Ks 8s 6s 2s"));
    }

    #[test]
    fn wheel_straight_is_the_lowest_straight() {
        let wheel = five("5s 4d 3h 2c As");
        let six_high = five("6s 5d 4h 3c 2s");
        assert!(six_high > wheel);
        assert_eq!(wheel.high_class(), HandClass::Straight);
    }

    #[test]
    fn steel_wheel_is_the_lowest_straight_flush() {
        let steel_wheel = five("5s 4s 3s 2s As");
        let six_high_sf = five("6s 5s 4s 3s 2s");
        assert!(six_high_sf > steel_wheel);
        assert_eq!(steel_wheel.high_class(), HandClass::StraightFlush);
    }

    #[test]
    fn seven_card_evaluate_picks_the_best_five() {
        // Board plays: both hole cards are worse than the board's straight.
        let board_straight = evaluate(&parse_cards("Th 9c 8d 7h 6s 2c 3d").unwrap());
        assert_eq!(board_straight, five("Th 9c 8d 7h 6s"));
        assert_eq!(board_straight.high_class(), HandClass::Straight);

        // Adding a card that improves the hand (trips) is picked up.
        let with_trips = evaluate(&parse_cards("Ah Ac Ad Kd Qh 3c 2s").unwrap());
        assert_eq!(with_trips.high_class(), HandClass::Trips);
        assert_eq!(with_trips, five("Ah Ac Ad Kd Qh"));
    }

    #[test]
    fn adding_irrelevant_cards_never_lowers_value() {
        let base = five("Ac Ad Kc Qd Jh");
        let with_two_extra = evaluate(&parse_cards("Ac Ad Kc Qd Jh 2s 3h").unwrap());
        assert!(with_two_extra >= base);

        let base_flush = five("As Ks 8s 6s 2s");
        let with_extra = evaluate(&parse_cards("As Ks 8s 6s 2s 3d 4h").unwrap());
        assert!(with_extra >= base_flush);
    }

    // ---- sixes-or-better qualifier (archie) ----------------------------

    fn sob(s: &str) -> Option<HandValue> {
        sixes_or_better(&parse_cards(s).unwrap())
    }

    #[test]
    fn sixes_or_better_needs_at_least_a_pair_of_sixes() {
        assert_eq!(sob("6c 6d Kh Qs 2c"), Some(five("6c 6d Kh Qs 2c")));
        assert!(sob("5c 5d Kh Qs 2c").is_none());
        // The best possible pair of fives still misses; the worst pair of
        // sixes clears.
        assert!(sob("5c 5d Ah Ks Qc").is_none());
        assert!(sob("6c 6d 4h 3s 2c").is_some());
    }

    #[test]
    fn sixes_or_better_never_qualifies_a_no_pair_hand() {
        // Ace-high, king-high, an eight-high wheel-shaped no-pair hand:
        // HighCard is below the bar no matter what its kickers are.
        assert!(sob("Ac Kd Qh Js 9c").is_none());
        assert!(sob("8c 6d 4h 3s 2c").is_none());
    }

    #[test]
    fn sixes_or_better_qualifies_every_class_above_one_pair() {
        for hand in [
            "2c 2d 3h 3s Kc", // two pair, deuces and treys
            "2c 2d 2h Ks Qc", // trips
            "6s 5d 4h 3c 2s", // straight
            "Ks 8s 6s 4s 2s", // flush
            "2c 2d 2h 3s 3c", // full house
            "2c 2d 2h 2s 3c", // quads
            "6s 5s 4s 3s 2s", // straight flush
        ] {
            assert!(sob(hand).is_some(), "{hand} must qualify");
        }
    }

    #[test]
    fn sixes_or_better_qualifies_on_the_best_seven_card_hand() {
        // Seven cards whose best five is a pair of sevens; the qualifier
        // reads the best hand, not the first five cards.
        let cards = parse_cards("7c 7d 5h 4s 3c 2d 9h").unwrap();
        assert_eq!(sixes_or_better(&cards), Some(evaluate(&cards)));
        // No five-card subset here pairs anything at all.
        assert!(sixes_or_better(&parse_cards("Ac Kd Qh Js 9c 7d 5h").unwrap()).is_none());
    }

    /// Omaha's `ExactlyTwo` usage requires exactly two hole cards; a player
    /// with only one card of a suit can't complete a flush even if the
    /// board is drowning in that suit, because they'd need to borrow a
    /// fourth board card to do it.
    #[test]
    fn omaha_exactly_two_blocks_unreachable_flush() {
        let hole = parse_cards("9h 9c 2d 2c").unwrap();
        let board = parse_cards("Kh Qh Jh Th 3c").unwrap();

        let omaha = best_with_usage(EvalKind::High, HoleUsage::ExactlyTwo, &hole, &board).unwrap();
        assert_eq!(omaha.high_class(), HandClass::OnePair);
        assert_eq!(omaha, five("9h 9c Kh Qh Jh"));

        // Without the "exactly two hole cards" constraint the same nine
        // cards contain a straight flush (four board hearts plus the 9h) —
        // proof that ExactlyTwo, not the card pool itself, blocks it.
        let mut pool = hole.clone();
        pool.extend_from_slice(&board);
        assert_eq!(evaluate(&pool).high_class(), HandClass::StraightFlush);
    }
}

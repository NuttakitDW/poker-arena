//! Lowball evaluation: A-5 (California), 2-7 (Kansas City), and the
//! eight-or-better qualifier. Encoding contracts in `eval/mod.rs` docs.

use super::high;
use super::{HandClass, HandValue};
use crate::card::Card;

/// Best A-5 low (aces low, straights/flushes ignored) from 5–7 cards.
pub(super) fn ace_to_five(cards: &[Card]) -> HandValue {
    assert!(cards.len() >= 5, "ace_to_five requires at least 5 cards");
    HandValue(0x00FF_FFFF - best_low_badness(cards))
}

/// Best 2-7 low (aces high, straights/flushes count against you) from 5–7
/// cards: the inverse of the best (lowest) high-hand encoding.
pub(super) fn deuce_to_seven(cards: &[Card]) -> HandValue {
    assert!(cards.len() >= 5, "deuce_to_seven requires at least 5 cards");
    let min_high = five_card_combos(cards)
        .into_iter()
        .map(|combo| high::rank_five_no_wheel(combo).0)
        .min()
        .expect("five_card_combos yields at least one combination");
    HandValue(0x00FF_FFFF - min_high)
}

/// Best qualifying eight-or-better low (A-5 rules) from 5–7 cards, or
/// `None` if no 5-card subset has five distinct ranks all at Eight or below.
pub(super) fn eight_or_better(cards: &[Card]) -> Option<HandValue> {
    assert!(
        cards.len() >= 5,
        "eight_or_better requires at least 5 cards"
    );
    let badness = best_low_badness(cards);
    let class = (badness >> 20) & 0xF;
    let top_rank = (badness >> 16) & 0xF;
    if class == HandClass::HighCard as u32 && top_rank <= 7 {
        Some(HandValue(0x00FF_FFFF - badness))
    } else {
        None
    }
}

/// Ace-low rank: `Ace = 0, Two = 1 … King = 12`.
fn low_rank(c: Card) -> u8 {
    (c.rank().index() + 1) % 13
}

/// Lowest-badness 5-card subset, using the A-5 pair-structure classes
/// (never Straight/Flush/StraightFlush — those are irrelevant to A-5).
fn best_low_badness(cards: &[Card]) -> u32 {
    five_card_combos(cards)
        .into_iter()
        .map(low_badness)
        .min()
        .expect("five_card_combos yields at least one combination")
}

fn low_badness(cards: [Card; 5]) -> u32 {
    let mut ranks: [u8; 5] = cards.map(low_rank);
    ranks.sort_unstable_by(|a, b| b.cmp(a));

    let mut counts = [0u8; 13];
    for &r in &ranks {
        counts[r as usize] += 1;
    }
    let mut groups: Vec<(u8, u8)> = (0..13u8)
        .filter(|&r| counts[r as usize] > 0)
        .map(|r| (counts[r as usize], r))
        .collect();
    groups.sort_unstable_by(|a, b| b.cmp(a));

    let (class, tiebreak): (HandClass, [u8; 5]) = match groups.as_slice() {
        [(4, q), (1, k)] => (HandClass::Quads, [*q, *k, 0, 0, 0]),
        [(3, t), (2, p)] => (HandClass::FullHouse, [*t, *p, 0, 0, 0]),
        [(3, t), (1, k1), (1, k2)] => (HandClass::Trips, [*t, *k1, *k2, 0, 0]),
        [(2, p1), (2, p2), (1, k)] => (HandClass::TwoPair, [*p1, *p2, *k, 0, 0]),
        [(2, p), (1, k1), (1, k2), (1, k3)] => (HandClass::OnePair, [*p, *k1, *k2, *k3, 0]),
        [(1, a), (1, b), (1, c), (1, d), (1, e)] => (HandClass::HighCard, [*a, *b, *c, *d, *e]),
        _ => unreachable!("five cards always produce one of the histograms above"),
    };

    encode(class, tiebreak)
}

fn encode(class: HandClass, ranks: [u8; 5]) -> u32 {
    let mut v = (class as u32) << 20;
    for (i, r) in ranks.iter().enumerate() {
        v |= (*r as u32) << (16 - 4 * i);
    }
    v
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

    fn a5(s: &str) -> HandValue {
        ace_to_five(&parse_cards(s).unwrap())
    }
    fn d7(s: &str) -> HandValue {
        deuce_to_seven(&parse_cards(s).unwrap())
    }
    fn ob(s: &str) -> Option<HandValue> {
        eight_or_better(&parse_cards(s).unwrap())
    }

    #[test]
    fn ace_to_five_wheel_is_the_nuts_a2346_is_second() {
        let wheel = a5("5c 4d 3h 2s Ac");
        let second = a5("6c 4d 3h 2s Ac");
        let third = a5("6c 5d 3h 2s Ac");
        assert!(wheel > second);
        assert!(second > third);
    }

    #[test]
    fn ace_to_five_any_pair_is_worse_than_any_no_pair() {
        // Worst possible no-pair hand (K high).
        let worst_no_pair = a5("Kc Qd Jh Ts 9c");
        // Best possible one-pair hand (deuces, plus the lowest kickers).
        let best_pair = a5("2c 2d 3h 4s 5c");
        assert!(worst_no_pair > best_pair);
    }

    #[test]
    fn ace_to_five_treats_ace_as_low() {
        let with_ace = a5("Ac 2d 3h 4s 5c"); // wheel: ace plays as the lowest card
        let with_king = a5("Kc 2d 3h 4s 5c"); // king plays as the highest card
        assert!(with_ace > with_king);
    }

    #[test]
    fn ace_to_five_ignores_straights_and_flushes() {
        let wheel_suited = a5("5s 4s 3s 2s As");
        let wheel_offsuit = a5("5c 4d 3h 2s Ac");
        assert_eq!(wheel_suited, wheel_offsuit);

        // A straight-shaped hand (six-high) is judged purely by its ranks,
        // with no bonus or penalty for being sequential: six-high still
        // beats seven-high exactly as two ordinary no-pair hands would.
        let straight_shaped_six_high = a5("6c 5d 4h 3s 2c");
        let non_straight_seven_high = a5("7c 5d 4h 3s 2c");
        assert!(straight_shaped_six_high > non_straight_seven_high);

        // A flush-shaped hand (all clubs) is identical in value to the same
        // ranks dealt in mixed suits — suits play no role at all.
        let flush_shaped = a5("6c 5c 4c 3c 2c");
        let same_ranks_mixed_suits = a5("6c 5d 4h 3s 2c");
        assert_eq!(flush_shaped, same_ranks_mixed_suits);
    }

    #[test]
    fn deuce_to_seven_75432_is_the_nuts() {
        let nuts = d7("7c 5d 4h 3s 2c");
        assert!(nuts > d7("8c 5d 4h 3s 2c"));
        assert!(nuts > d7("7c 6d 4h 3s 2c"));
        // Suits don't matter for 2-7 badness beyond the flush check itself.
        assert_eq!(nuts, d7("7s 5c 4d 3h 2d"));
    }

    #[test]
    fn deuce_to_seven_23457_beats_23456_straight() {
        assert!(d7("2c 3d 4h 5s 7c") > d7("2c 3d 4h 5s 6c"));
    }

    #[test]
    fn deuce_to_seven_straight_or_flush_worse_than_any_nine_low() {
        let nine_low = d7("9c 7d 5h 4s 2c"); // no pair, no straight, no flush
        let straight = d7("6c 5d 4h 3s 2c"); // six-high straight
        let flush = d7("Kc 8c 6c 4c 2c"); // king-high flush, not sequential
        assert!(nine_low > straight);
        assert!(nine_low > flush);
    }

    #[test]
    fn deuce_to_seven_ace_is_always_high_never_completes_wheel() {
        // A-5432 is NOT a straight in 2-7: the ace is high, so this is just
        // an ace-high no-pair hand — one of the worst possible hands.
        let ace_attempt = d7("5c 4d 3h 2s Ac");
        let nine_low = d7("9c 7d 5h 4s 2c");
        let pair = d7("2c 2d 4h 5s 7c");
        assert!(nine_low > ace_attempt);
        assert!(ace_attempt > pair);
    }

    #[test]
    fn eight_or_better_qualification() {
        assert!(ob("8c 7d 6h 5s Ac").is_some());
        assert!(ob("9c 7d 6h 5s Ac").is_none());

        // Paired board (two 8s) but a qualifying subset still exists in 7 cards.
        let cards = parse_cards("8c 8d 7h 6s 5c 4d Ac").unwrap();
        assert!(eight_or_better(&cards).is_some());

        // A2345 is the best possible eight-or-better low.
        let nuts = eight_or_better(&parse_cards("Ac 2d 3h 4s 5c").unwrap()).unwrap();
        let eight_low = ob("8c 7d 6h 5s Ac").unwrap();
        assert!(nuts > eight_low);
    }
}

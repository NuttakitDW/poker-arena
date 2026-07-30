//! Badugi evaluation. Encoding contract in `eval/mod.rs` docs.

use super::HandValue;
use crate::card::Card;

/// Best badugi from up to 4 cards (the best subset with pairwise-distinct
/// ranks *and* suits; larger subsets always beat smaller; ties break low
/// with aces low).
pub(super) fn evaluate(cards: &[Card]) -> HandValue {
    assert!(!cards.is_empty(), "badugi requires at least one card");
    assert!(cards.len() <= 4, "badugi accepts at most four cards");
    let n = cards.len();

    let mut best: Option<u32> = None;
    for mask in 1u32..(1 << n) {
        let subset: Vec<Card> = (0..n)
            .filter(|i| mask & (1 << i) != 0)
            .map(|i| cards[i])
            .collect();
        if !is_valid_badugi(&subset) {
            continue;
        }
        let mut ranks: Vec<u8> = subset.iter().copied().map(low_rank).collect();
        ranks.sort_unstable_by(|a, b| b.cmp(a));
        let value = (subset.len() as u32) << 20 | (0xF_FFFF - pack_ranks_desc(&ranks));
        best = Some(best.map_or(value, |cur| cur.max(value)));
    }
    HandValue(best.expect("every single card is a valid one-card badugi subset"))
}

/// A subset is a valid badugi set iff every pair of cards has both a
/// distinct rank and a distinct suit.
fn is_valid_badugi(cards: &[Card]) -> bool {
    for i in 0..cards.len() {
        for j in (i + 1)..cards.len() {
            if cards[i].rank() == cards[j].rank() || cards[i].suit() == cards[j].suit() {
                return false;
            }
        }
    }
    true
}

/// Ace-low rank: `Ace = 0, Two = 1 … King = 12`.
fn low_rank(c: Card) -> u8 {
    (c.rank().index() + 1) % 13
}

/// Pack up to four ranks (already sorted descending) into the top four
/// 4-bit slots of a 20-bit field, leaving the trailing slot zero — mirrors
/// how unused tiebreak slots are always the least-significant ones.
fn pack_ranks_desc(ranks: &[u8]) -> u32 {
    let mut v = 0u32;
    for i in 0..4 {
        let r = ranks.get(i).copied().unwrap_or(0);
        v |= (r as u32) << (16 - 4 * i);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::parse_cards;

    fn badugi(s: &str) -> HandValue {
        evaluate(&parse_cards(s).unwrap())
    }

    #[test]
    fn four_card_badugi_beats_any_three_card() {
        let four = badugi("Ac 2d 3h 4s");
        // Duplicate rank (two Threes) forces a 3-card best subset.
        let three = badugi("Ac 2d 3h 3s");
        assert!(four > three);
        assert_eq!(four.0 >> 20, 4);
        assert_eq!(three.0 >> 20, 3);
    }

    #[test]
    fn a234_rainbow_is_the_nuts() {
        let nuts = badugi("Ac 2d 3h 4s");
        assert!(nuts > badugi("Ac 2d 3h 5s"));
        assert!(nuts > badugi("2c 3d 4h 5s"));
    }

    #[test]
    fn duplicate_suit_forces_subset_shrink_and_prefers_the_ace() {
        // Ac and 2c share a suit, so the best 4-set is invalid; between the
        // two valid 3-card subsets, the one keeping the ace is lower.
        let v = badugi("Ac 2c 3d 4h");
        let with_ace = evaluate(&parse_cards("Ac 3d 4h").unwrap());
        let without_ace = evaluate(&parse_cards("2c 3d 4h").unwrap());
        assert_eq!(v, with_ace);
        assert!(with_ace > without_ace);
    }

    #[test]
    fn duplicate_rank_forces_subset_shrink_example_from_spec() {
        // As Ad 2c 3h evaluates as a 3-card A23 (dropping one ace).
        let v = badugi("As Ad 2c 3h");
        let expected = evaluate(&parse_cards("As 2c 3h").unwrap());
        assert_eq!(v, expected);
        assert_eq!(v, evaluate(&parse_cards("Ad 2c 3h").unwrap()));
        assert_eq!(v.0 >> 20, 3);
    }

    #[test]
    fn single_card_is_always_valid() {
        let v = badugi("Kc");
        assert_eq!(v.0 >> 20, 1);
    }

    #[test]
    fn ties_break_low_ace_low() {
        let lower = badugi("Ac 2d 3h 4s");
        let higher = badugi("2c 3d 4h 5s");
        assert!(lower > higher);
    }
}

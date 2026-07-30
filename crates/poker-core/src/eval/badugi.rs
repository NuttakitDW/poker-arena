//! Badugi evaluation. Encoding contract in `eval/mod.rs` docs.

use super::HandValue;
use crate::card::Card;

/// Best badugi from up to 5 cards (the best subset with pairwise-distinct
/// ranks *and* suits; larger subsets always beat smaller; ties break low
/// with aces low). Five-card hands (badacey) simply enumerate one more
/// card's worth of subsets; a badugi set is never larger than four either
/// way, because only four suits exist.
pub(super) fn evaluate(cards: &[Card]) -> HandValue {
    best_badugi(cards, low_rank)
}

/// Best badugi with aces **high** (badeucy): identical algorithm and
/// encoding, ranked with [`Rank::index`] so the nuts are 5-4-3-2 rainbow and
/// any badugi containing an ace is crushed.
///
/// [`Rank::index`]: crate::card::Rank::index
pub(super) fn evaluate_ace_high(cards: &[Card]) -> HandValue {
    best_badugi(cards, high_rank)
}

/// Shared subset search behind both rank orders. `rank_of` maps a card to
/// the 0–12 "lower is better" rank used for the tiebreak packing; nothing
/// else about the two variants differs.
fn best_badugi(cards: &[Card], rank_of: fn(Card) -> u8) -> HandValue {
    assert!(!cards.is_empty(), "badugi requires at least one card");
    assert!(cards.len() <= 5, "badugi accepts at most five cards");
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
        let mut ranks: Vec<u8> = subset.iter().copied().map(rank_of).collect();
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

/// Ace-high rank: `Two = 0 … King = 11, Ace = 12` — [`Rank::index`] verbatim.
///
/// [`Rank::index`]: crate::card::Rank::index
fn high_rank(c: Card) -> u8 {
    c.rank().index()
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

    // ---- five-card inputs (badacey / badeucy) --------------------------

    fn ace_high(s: &str) -> HandValue {
        evaluate_ace_high(&parse_cards(s).unwrap())
    }

    #[test]
    fn five_card_hand_scores_its_best_four_card_badugi() {
        // As and 2s clash, so the best 4-set drops one of them; keeping the
        // ace (A-5-4-3) beats keeping the deuce (5-4-3-2) with aces low.
        let v = badugi("As 2s 3d 4h 5c");
        assert_eq!(v, badugi("As 3d 4h 5c"));
        assert!(v > badugi("2s 3d 4h 5c"));
        assert_eq!(v.0 >> 20, 4);
    }

    #[test]
    fn five_card_badugi_is_never_larger_than_four() {
        // Five cards cannot be rainbow: some pair of them always shares a
        // suit, so the subset length caps at four.
        let v = badugi("Ac 2d 3h 4s 5c");
        assert_eq!(v.0 >> 20, 4);
        assert_eq!(v, badugi("Ac 2d 3h 4s"));
    }

    #[test]
    fn five_card_hand_can_still_be_a_short_badugi() {
        // Four kings (every suit, one rank) plus a club: the only valid
        // two-card sets pair the nine with a non-club king.
        let v = badugi("Kc Kd Kh Ks 9c");
        assert_eq!(v.0 >> 20, 2);
        assert_eq!(v, badugi("Kd 9c"));
    }

    #[test]
    fn ace_high_nuts_is_5_4_3_2_rainbow() {
        let nuts = ace_high("5c 4d 3h 2s");
        assert!(nuts > ace_high("6c 4d 3h 2s"));
        assert!(nuts > ace_high("Ac 4d 3h 2s"));
        assert_eq!(nuts.0 >> 20, 4);
    }

    #[test]
    fn ace_high_crushes_any_badugi_holding_an_ace() {
        // The worst ace-free four-card badugi still beats the best one that
        // contains an ace.
        let worst_ace_free = ace_high("Kc Qd Jh Ts");
        let best_with_ace = ace_high("Ac 4d 3h 2s");
        assert!(worst_ace_free > best_with_ace);
    }

    #[test]
    fn ace_high_still_prefers_more_cards() {
        let four = ace_high("Ac 2d 3h 4s");
        let three = ace_high("5c 4d 3h 3s");
        assert!(four > three);
        assert_eq!(three.0 >> 20, 3);
    }

    #[test]
    fn ace_high_five_card_hand_drops_the_ace_when_it_can() {
        // Ac clashes with 5c; ace-high keeps the five instead of the ace,
        // the exact opposite of the ace-low choice on the same five cards.
        let v = ace_high("Ac 5c 4d 3h 2s");
        assert_eq!(v, ace_high("5c 4d 3h 2s"));
        assert!(v > ace_high("Ac 4d 3h 2s"));
    }
}

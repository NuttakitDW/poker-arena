//! Lossless card abstraction: suit isomorphism.
//!
//! Two hand contexts that differ only by a permutation of the four suits are
//! strategically identical in every game this arena runs (no variant ranks
//! suits). Canonicalizing under the full 4! = 24 suit permutations is
//! therefore *lossless*: e.g. the 1326 hold'em starting hands collapse to
//! the familiar 169 classes, and C(52,3) = 22100 flops to 1755.
//!
//! A context is an ordered list of card *groups*. Group boundaries carry
//! information (hole cards vs board vs a street's upcards), so they are
//! preserved; order *within* a group does not, so each group is sorted.
//! Canonical form: the suit permutation (applied to every card in every
//! group) whose flattened, per-group-sorted card sequence is
//! lexicographically smallest by [`Card::index`]. Brute force over 24
//! permutations — a handful of comparisons over at most a dozen cards, far
//! from any hot path, and obviously correct.

use poker_core::card::{Card, Suit};

/// All 24 permutations of the four suits, generated in a fixed order so the
/// canonical choice is deterministic.
fn suit_permutations() -> [[u8; 4]; 24] {
    let mut out = [[0u8; 4]; 24];
    let mut n = 0;
    for a in 0..4u8 {
        for b in 0..4u8 {
            if b == a {
                continue;
            }
            for c in 0..4u8 {
                if c == a || c == b {
                    continue;
                }
                let d = 6 - a - b - c;
                out[n] = [a, b, c, d];
                n += 1;
            }
        }
    }
    out
}

fn apply(perm: &[u8; 4], card: Card) -> Card {
    let suit =
        Suit::from_index(perm[card.suit().index() as usize]).expect("permutation entries are 0..4");
    Card::new(card.rank(), suit)
}

/// The canonical (suit-relabeled, per-group-sorted) form of `groups`.
///
/// Any two inputs that are suit permutations of each other — with the same
/// group structure and any within-group order — map to the same output.
pub fn canonical(groups: &[&[Card]]) -> Vec<Vec<Card>> {
    let mut best: Option<Vec<Vec<Card>>> = None;
    for perm in suit_permutations() {
        let mut mapped: Vec<Vec<Card>> = groups
            .iter()
            .map(|group| group.iter().map(|card| apply(&perm, *card)).collect())
            .collect();
        for group in &mut mapped {
            group.sort_unstable_by_key(|card| card.index());
        }
        if best.as_ref().is_none_or(|top| flat_less(&mapped, top)) {
            best = Some(mapped);
        }
    }
    best.unwrap_or_default()
}

/// Lexicographic comparison of two group lists by flattened card indices.
fn flat_less(a: &[Vec<Card>], b: &[Vec<Card>]) -> bool {
    let flatten = |groups: &[Vec<Card>]| -> Vec<u8> {
        groups
            .iter()
            .flat_map(|group| group.iter().map(|card| card.index()))
            .collect()
    };
    flatten(a) < flatten(b)
}

/// A canonical context packed into one `u128` key: 6 bits per card, a
/// sentinel (63) between groups. Holds up to 18 cards + separators — enough
/// for every context any variant here produces (drawmaha's 5 hole + 5 board
/// is the largest at 10 cards, 2 groups).
///
/// Keys are only comparable between contexts with the same group shape; the
/// intended use is as an information-set lookup key in a strategy table.
pub fn canonical_key(groups: &[&[Card]]) -> u128 {
    const SEPARATOR: u128 = 63;
    let mut key: u128 = 0;
    for (i, group) in canonical(groups).iter().enumerate() {
        if i > 0 {
            key = (key << 6) | SEPARATOR;
        }
        for card in group {
            key = (key << 6) | u128::from(card.index());
        }
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;
    use poker_core::card::parse_cards;
    use poker_core::rng::Rng64;
    use std::collections::HashSet;

    fn all_cards() -> Vec<Card> {
        (0..52).map(|i| Card::from_index(i).unwrap()).collect()
    }

    #[test]
    fn permutations_are_24_distinct() {
        let perms = suit_permutations();
        let set: HashSet<[u8; 4]> = perms.iter().copied().collect();
        assert_eq!(set.len(), 24);
    }

    #[test]
    fn holdem_starting_hands_collapse_to_169() {
        let cards = all_cards();
        let mut classes = HashSet::new();
        for i in 0..52 {
            for j in (i + 1)..52 {
                classes.insert(canonical_key(&[&[cards[i], cards[j]]]));
            }
        }
        assert_eq!(classes.len(), 169);
    }

    #[test]
    fn flops_collapse_to_1755() {
        let cards = all_cards();
        let mut classes = HashSet::new();
        for i in 0..52 {
            for j in (i + 1)..52 {
                for k in (j + 1)..52 {
                    classes.insert(canonical_key(&[&[cards[i], cards[j], cards[k]]]));
                }
            }
        }
        assert_eq!(classes.len(), 1755);
    }

    #[test]
    fn random_suit_permutations_share_a_canonical_form() {
        let mut rng = Rng64::from_seed_stream(7, 0);
        let perms = suit_permutations();
        for _ in 0..200 {
            let mut deck = all_cards();
            rng.shuffle(&mut deck);
            let hole = &deck[0..2];
            let board = &deck[2..7];
            let reference = canonical(&[hole, board]);

            let perm = perms[rng.below(24) as usize];
            let hole_mapped: Vec<Card> = hole.iter().map(|c| apply(&perm, *c)).collect();
            let board_mapped: Vec<Card> = board.iter().map(|c| apply(&perm, *c)).collect();
            assert_eq!(reference, canonical(&[&hole_mapped, &board_mapped]));
        }
    }

    #[test]
    fn group_boundaries_are_preserved_and_meaningful() {
        let hole = parse_cards("As Kd").unwrap();
        let board = parse_cards("2c").unwrap();
        let split = canonical_key(&[&hole, &board]);
        let merged = canonical_key(&[&[hole[0], hole[1], board[0]][..]]);
        assert_ne!(split, merged, "a separator must distinguish group shapes");
    }
}

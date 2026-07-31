//! Decks: the shuffled or scripted source cards are dealt from.
//!
//! Separate from `poker_wire::card` because a deck needs an RNG, which makes
//! it engine machinery rather than shared vocabulary — a bot reads `Card`s
//! off the wire and never shuffles anything.

use crate::rng::Rng64;
use poker_wire::card::Card;

/// A deck of cards. Cards are drawn from the *end* of the internal vector, so
/// a `Deck` built from an explicit card list deals in reverse list order —
/// use [`Deck::from_deal_order`] in tests to script exact deals.
#[derive(Clone, Debug)]
pub struct Deck {
    cards: Vec<Card>,
}

impl Deck {
    /// The full 52-card deck in canonical index order (2c, 2d, … As).
    pub fn standard() -> Deck {
        Deck {
            cards: (0..52)
                .map(|i| Card::from_index(i).expect("0..52 are all valid card indices"))
                .collect(),
        }
    }

    /// A standard deck shuffled with the given RNG (Fisher–Yates; consumes a
    /// deterministic number of RNG draws).
    pub fn shuffled(rng: &mut Rng64) -> Deck {
        let mut deck = Deck::standard();
        rng.shuffle(&mut deck.cards);
        deck
    }

    /// Build a deck that will deal the given cards in the given order.
    /// Intended for scripted tests. Panics on duplicates.
    pub fn from_deal_order(cards: &[Card]) -> Deck {
        let mut seen = [false; 52];
        for c in cards {
            assert!(!seen[c.index() as usize], "duplicate card {c} in deck");
            seen[c.index() as usize] = true;
        }
        Deck {
            cards: cards.iter().rev().copied().collect(),
        }
    }

    pub fn remaining(&self) -> usize {
        self.cards.len()
    }

    /// Draw the next card, or `None` if the deck is exhausted.
    pub fn draw(&mut self) -> Option<Card> {
        self.cards.pop()
    }

    /// Draw `n` cards in order. Returns `None` (drawing nothing) if fewer
    /// than `n` remain.
    pub fn draw_n(&mut self, n: usize) -> Option<Vec<Card>> {
        if self.cards.len() < n {
            return None;
        }
        Some((0..n).map(|_| self.cards.pop().unwrap()).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use poker_wire::card::parse_cards;

    #[test]
    fn scripted_deck_deals_in_order() {
        let cards = parse_cards("As Kd 2c").unwrap();
        let mut deck = Deck::from_deal_order(&cards);
        assert_eq!(deck.draw().unwrap().to_string(), "As");
        assert_eq!(deck.draw().unwrap().to_string(), "Kd");
        assert_eq!(deck.draw().unwrap().to_string(), "2c");
        assert_eq!(deck.draw(), None);
    }

    #[test]
    fn shuffled_deck_is_a_permutation_and_deterministic() {
        let mut rng = Rng64::from_seed_stream(42, 0);
        let mut deck = Deck::shuffled(&mut rng);
        let mut seen = [false; 52];
        while let Some(c) = deck.draw() {
            assert!(!seen[c.index() as usize]);
            seen[c.index() as usize] = true;
        }
        assert!(seen.iter().all(|&s| s));

        let mut a = Rng64::from_seed_stream(7, 3);
        let mut b = Rng64::from_seed_stream(7, 3);
        let da: Vec<_> = std::iter::from_fn(|| Deck::shuffled(&mut a).draw())
            .take(1)
            .collect();
        let db: Vec<_> = std::iter::from_fn(|| Deck::shuffled(&mut b).draw())
            .take(1)
            .collect();
        assert_eq!(da, db);
    }
}

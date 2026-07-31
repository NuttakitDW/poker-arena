//! The three-row board a seat fills over an OFC hand.
//!
//! Capacities are the same in every variant: top 3, middle 5, bottom 5,
//! thirteen cards in all. A board is *complete* when every row is full, which
//! is the only state scoring ever looks at — partial boards exist mid-hand
//! but are never evaluated by this crate.

use crate::card::Card;
use poker_wire::ofc::row::{Placement, Row};

/// One seat's board. Rows keep cards in the order they were placed; nothing
/// in the engine depends on that order, but it makes hand logs readable.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Board {
    pub top: Vec<Card>,
    pub middle: Vec<Card>,
    pub bottom: Vec<Card>,
}

impl Board {
    pub const TOP_CAPACITY: usize = 3;
    pub const MIDDLE_CAPACITY: usize = 5;
    pub const BOTTOM_CAPACITY: usize = 5;
    /// Cards in a complete board, and therefore the number of placements
    /// every variant's structure must add up to.
    pub const CAPACITY: usize = Self::TOP_CAPACITY + Self::MIDDLE_CAPACITY + Self::BOTTOM_CAPACITY;

    pub fn new() -> Board {
        Board::default()
    }

    pub const fn capacity(row: Row) -> usize {
        match row {
            Row::Top => Self::TOP_CAPACITY,
            Row::Middle => Self::MIDDLE_CAPACITY,
            Row::Bottom => Self::BOTTOM_CAPACITY,
        }
    }

    pub fn row(&self, row: Row) -> &[Card] {
        match row {
            Row::Top => &self.top,
            Row::Middle => &self.middle,
            Row::Bottom => &self.bottom,
        }
    }

    fn row_mut(&mut self, row: Row) -> &mut Vec<Card> {
        match row {
            Row::Top => &mut self.top,
            Row::Middle => &mut self.middle,
            Row::Bottom => &mut self.bottom,
        }
    }

    /// Slots still free in `row`.
    pub fn free(&self, row: Row) -> usize {
        Board::capacity(row) - self.row(row).len()
    }

    /// Add a card. Returns `false` — changing nothing — if the row is full;
    /// the state machine turns that into a placement error.
    pub fn push(&mut self, placement: Placement) -> bool {
        if self.free(placement.row) == 0 {
            return false;
        }
        self.row_mut(placement.row).push(placement.card);
        true
    }

    /// Cards placed so far, across all rows.
    pub fn placed(&self) -> usize {
        self.top.len() + self.middle.len() + self.bottom.len()
    }

    pub fn is_empty(&self) -> bool {
        self.placed() == 0
    }

    /// Every row full — the only state [`crate::ofc::score`] evaluates.
    pub fn is_complete(&self) -> bool {
        self.top.len() == Self::TOP_CAPACITY
            && self.middle.len() == Self::MIDDLE_CAPACITY
            && self.bottom.len() == Self::BOTTOM_CAPACITY
    }

    /// Every placed card, top row first.
    pub fn cards(&self) -> Vec<Card> {
        let mut all = Vec::with_capacity(self.placed());
        all.extend_from_slice(&self.top);
        all.extend_from_slice(&self.middle);
        all.extend_from_slice(&self.bottom);
        all
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::parse_cards;

    fn place(board: &mut Board, cards: &str, row: Row) -> bool {
        parse_cards(cards)
            .unwrap()
            .into_iter()
            .all(|card| board.push(Placement { card, row }))
    }

    #[test]
    fn rows_fill_to_capacity_and_then_refuse() {
        let mut board = Board::new();
        assert!(board.is_empty());
        assert!(place(&mut board, "As Ks Qs", Row::Top));
        assert_eq!(board.free(Row::Top), 0);
        assert!(!place(&mut board, "Js", Row::Top));
        assert_eq!(board.top.len(), 3);
    }

    #[test]
    fn completion_needs_all_thirteen_cards() {
        let mut board = Board::new();
        assert!(place(&mut board, "As Ks Qs", Row::Top));
        assert!(place(&mut board, "2c 3c 4c 5c 7c", Row::Middle));
        assert!(!board.is_complete());
        assert!(place(&mut board, "2d 3d 4d 5d 7d", Row::Bottom));
        assert!(board.is_complete());
        assert_eq!(board.placed(), Board::CAPACITY);
        assert_eq!(board.cards().len(), 13);
    }
}

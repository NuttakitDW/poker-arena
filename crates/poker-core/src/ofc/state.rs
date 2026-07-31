//! The per-hand OFC state machine.
//!
//! `OfcHandState` interprets an [`OfcSpec`] to run exactly one hand: dealing,
//! placement turns, showdown, settlement. It is pure and synchronous — no
//! I/O, no clocks, no bots. The arena layer owns all of those and simply
//! calls `to_act` / `request` / `apply` in a loop.
//!
//! # Rules contract (the implementation spec)
//!
//! ## Boards and rows
//! - Every seat fills a board of three rows: **top** (3 cards), **middle**
//!   (5), **bottom** (5) — thirteen placed cards. Placement is final: a card
//!   never moves once placed, and a row never takes a fourteenth card.
//! - Boards are open face. Every placement is public as it happens, *except*
//!   a seat playing the hand in fantasyland, whose board stays hidden until
//!   showdown. Discards are always private; their count is public.
//! - Rows are valued by row: bottom and (high) middle with `eval::high` on
//!   five cards, top with `eval::three_card_high` on three, an `ofc-27`
//!   middle with `eval::deuce_to_seven_low`. Greater is better in every case.
//!   Top and middle values share the high encoding, so comparing them is a
//!   plain `HandValue` comparison (see `eval`'s encoding contract).
//!
//! ## Variants
//! - `ofc`: deal 5, then 8 × (deal 1, place 1). 2–4 seats; 4 × 13 = 52.
//! - `ofc-pineapple`, `ofc-progressive`, `ofc-27`: deal 5, then
//!   4 × (deal 3, place 2, discard 1). 2–3 seats; 3 × 17 = 51.
//! - The deck is never reshuffled and, by that card math, never runs out. A
//!   fantasyland seat is dealt at most 17 cards, so it never widens the
//!   bound.
//!
//! ## Setup (`new`)
//! - Validates the seat count against the spec, that the per-seat fantasyland
//!   slice matches the seat count, and that each fantasyland count can both
//!   fill a board (≥ 13) and be dealt (≤ the spec's cards per seat).
//! - **Table order** is seat 1, 2, …, n−1, 0: the button is always seat 0 and
//!   goes last. *Every* per-seat iteration in a hand — fantasyland
//!   announcements, deals, placement turns, showdown, scores — walks table
//!   order. There are no other orders in this engine.
//! - Setup emits, in table order: `Fantasyland {seat, cards}` for each seat
//!   entering the hand in fantasyland, then one `Deal` per seat of its
//!   opening cards (its fantasyland count, or `initial_deal`). `new` returns
//!   those events and stops at the first pending decision.
//!
//! ## Placement turns
//! - Turn order for the hand is fixed at setup: first every fantasyland seat
//!   in table order (one turn each: place 13, discard the rest), then every
//!   non-fantasyland seat in table order for its opening deal (place all 5,
//!   discard none), then, for each round in turn, every non-fantasyland seat
//!   in table order.
//! - Rounds interleave deal and act **per seat**: a seat is dealt its
//!   `round_deal` cards (a `Deal` event) only after the previous seat has
//!   placed, and acts immediately. `apply` returns every event its placement
//!   caused, including the next seat's deal.
//! - Each turn ends with one `Place {seat, placements, discarded, count}`.
//!
//! ## Placement legality
//! - Placements are exactly `place` distinct cards drawn from the seat's
//!   *just-dealt* set; discards are exactly the remaining cards of that set.
//!   Order does not matter; a card used twice, or a card that was not dealt
//!   this turn, is illegal.
//! - Every placed card must go to a row with free capacity, counting the
//!   other placements in the same action.
//! - The totals guarantee a legal option always exists, so a rejected action
//!   is always the caller's error. The arena's answer is to substitute
//!   deterministically: sort the dealt cards ascending by `Card::index`, take
//!   the first `place`, drop each into bottom if it has space, else middle,
//!   else top, and discard the rest.
//!
//! ## Fouling
//! - High-middle variants: the hand fouls iff top > middle or middle >
//!   bottom. Equality never fouls.
//! - `ofc-27`: the hand fouls iff top > bottom, or the middle is not a
//!   qualifying 2-7 low — ten-low or better, meaning no pair, no straight, no
//!   flush and a high card of Ten or below (the worst qualifier is
//!   T-9-8-7-5). The middle has no ordering relationship with its neighbours.
//! - A fouled board earns no royalties and wins no rows.
//!
//! ## Showdown & settlement
//! - When the last turn is placed, every seat reveals in table order —
//!   `Showdown` carries the full board, all three row values (always
//!   computed, fouled hands included), the raw per-row royalties, the foul
//!   flag and the next hand's fantasyland count. For a fantasyland seat this
//!   is the first time other seats see its board. Then one `Score {seat,
//!   points}` per seat, again in table order.
//! - Scoring is **pairwise over every unordered pair of seats**. With neither
//!   fouled: +1 for each row won outright (a tied row pays nothing), +3 more
//!   for winning all three, plus the royalty difference — royalties always
//!   count, win or lose the row. With one fouled: the fouler pays 6 plus the
//!   opponent's royalties, and its own royalties are void; rows are not
//!   compared. With both fouled: nothing changes hands.
//! - A seat's net is the sum over its pairs. Nets sum to zero.
//!
//! ## Royalties
//! - Bottom, every variant: Straight 2, Flush 4, Full House 6, Quads 10,
//!   Straight Flush 15, Royal Flush 25.
//! - Middle, high variants: Trips 2, Straight 4, Flush 8, Full House 12,
//!   Quads 20, Straight Flush 30, Royal Flush 50.
//! - Middle, `ofc-27`: 9-low 1, 8-low 2, 7-low 4, the exact 7-5-4-3-2 8.
//! - Top, every variant: pairs from sixes (66 = 1 … AA = 9, i.e. the pair's
//!   `Rank::index` − 3) and trips from deuces (222 = 10 … AAA = 22, the trip
//!   rank's `Rank::index` + 10).
//! - A royal flush is a straight flush with an ace-high tiebreak; nothing
//!   below a pair of sixes pays on top.
//!
//! ## Fantasyland
//! - Fantasyland is a property of the *next* hand: a card count, dealt all at
//!   once, from which the seat places 13 and discards the rest with its board
//!   hidden until showdown. It never changes how many hands a match plays.
//! - **Entry** (only from a non-fantasyland, non-fouled hand):
//!   `ofc`/`ofc-pineapple` top QQ+ or any trips → 13/14 cards;
//!   `ofc-progressive` top QQ → 14, KK → 15, AA → 16, any top trips → 17;
//!   `ofc-27` top KK+ (pair or trips) or an exact 7-5-4-3-2 middle → 14, both
//!   at once → 15. A suited 7-5-4-3-2 is a flush, not a qualifying 2-7 low,
//!   and does not count.
//! - **Stay** (only from a fantasyland, non-fouled hand) grants the variant's
//!   base count — 13 for `ofc`, 14 otherwise: `ofc`/`ofc-pineapple`/
//!   `ofc-progressive` on top trips, middle full house or better, or bottom
//!   quads or better; `ofc-27` on top trips or bottom quads or better.
//! - A seat already in fantasyland can only stay; a seat outside it can only
//!   enter.
//!
//! ## Invariants (must be property-tested)
//! - Nets sum to zero at settlement.
//! - `apply` accepts exactly the actions this contract calls legal.
//! - Every hand terminates: the turn schedule is finite and fixed at setup.
//! - Card conservation: the union of every board and every discard pile is
//!   exactly the cards dealt, with no card appearing twice.
//! - Determinism: the same deck and the same placements produce a
//!   byte-identical event stream.

use crate::card::{Card, Deck};
use crate::ofc::board::Board;
use crate::ofc::score::{self, Evaluated};
use crate::ofc::spec::OfcSpec;
use poker_wire::ofc::event::OfcEvent;
use poker_wire::ofc::row::{OfcAction, Row};

pub use crate::ofc::score::OfcSettlement;

/// Errors constructing or driving an OFC hand.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OfcError {
    #[error("seat count {0} outside the spec's supported range")]
    BadSeatCount(usize),
    #[error("fantasyland slice has {got} entries but the hand has {seats} seats")]
    BadFantasylandLen { got: usize, seats: usize },
    #[error("seat {seat} fantasyland count {cards} is outside {min}..={max}")]
    BadFantasylandCount {
        seat: usize,
        cards: u8,
        min: u8,
        max: u8,
    },
    #[error("no placement decision is pending")]
    NoPendingDecision,
    #[error("seat {seat} must place {expected} cards, got {got}")]
    WrongPlacementCount {
        seat: usize,
        expected: u8,
        got: usize,
    },
    #[error("seat {seat} must discard {expected} cards, got {got}")]
    WrongDiscardCount {
        seat: usize,
        expected: u8,
        got: usize,
    },
    #[error("card {card} was not dealt to seat {seat} this turn")]
    CardNotDealt { seat: usize, card: Card },
    #[error("card {card} is used twice in seat {seat}'s action")]
    DuplicateCard { seat: usize, card: Card },
    #[error("seat {seat} cannot place {card} in the {row:?} row: it is full")]
    RowFull { seat: usize, card: Card, row: Row },
}

/// The decision the engine is waiting on: this turn's cards and how many of
/// them go on the board.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacementRequest {
    pub seat: usize,
    /// This turn's cards, in deal order.
    pub dealt: Vec<Card>,
    pub place: u8,
    pub discard: u8,
}

/// One scheduled placement turn. `deal` is the number of cards handed out
/// immediately before it (zero for the opening turns, whose cards were dealt
/// during setup).
#[derive(Copy, Clone, Debug)]
struct Turn {
    seat: usize,
    deal: u8,
    place: u8,
}

/// One OFC hand, from the opening deal to settlement.
#[derive(Clone, Debug)]
pub struct OfcHandState {
    spec: OfcSpec,
    seats: usize,
    hand_no: u64,
    deck: Deck,
    fantasyland: Vec<Option<u8>>,
    /// `fantasyland` as the redaction rules want it: which boards are hidden.
    hidden: Vec<bool>,
    boards: Vec<Board>,
    discarded: Vec<Vec<Card>>,
    /// Cards dealt to a seat and not yet placed or discarded.
    dealt: Vec<Vec<Card>>,
    turns: Vec<Turn>,
    next_turn: usize,
    pending: Option<PlacementRequest>,
    events: Vec<OfcEvent>,
    settlement: Option<OfcSettlement>,
}

impl OfcHandState {
    /// Set up a hand: validate, announce fantasyland, deal opening cards, and
    /// stop at the first placement decision. Returns the events that produced.
    pub fn new(
        spec: &OfcSpec,
        seats: usize,
        fantasyland: &[Option<u8>],
        hand_no: u64,
        deck: Deck,
    ) -> Result<(Self, Vec<OfcEvent>), OfcError> {
        if !spec.seats().contains(&seats) {
            return Err(OfcError::BadSeatCount(seats));
        }
        if fantasyland.len() != seats {
            return Err(OfcError::BadFantasylandLen {
                got: fantasyland.len(),
                seats,
            });
        }
        let (min, max) = (Board::CAPACITY as u8, spec.cards_per_seat());
        for (seat, count) in fantasyland.iter().enumerate() {
            if let Some(cards) = *count
                && !(min..=max).contains(&cards)
            {
                return Err(OfcError::BadFantasylandCount {
                    seat,
                    cards,
                    min,
                    max,
                });
            }
        }

        let mut state = OfcHandState {
            spec: *spec,
            seats,
            hand_no,
            deck,
            fantasyland: fantasyland.to_vec(),
            hidden: fantasyland.iter().map(Option::is_some).collect(),
            boards: vec![Board::new(); seats],
            discarded: vec![Vec::new(); seats],
            dealt: vec![Vec::new(); seats],
            turns: Vec::new(),
            next_turn: 0,
            pending: None,
            events: Vec::new(),
            settlement: None,
        };

        for seat in table_order(seats) {
            if let Some(cards) = state.fantasyland[seat] {
                state.emit(OfcEvent::Fantasyland { seat, cards });
            }
        }
        for seat in table_order(seats) {
            let count = state.fantasyland[seat].unwrap_or(spec.initial_deal);
            state.deal_to(seat, count);
        }

        state.turns = state.schedule();
        state.advance();
        let setup = state.events.clone();
        Ok((state, setup))
    }

    /// The seat whose placement the engine is waiting on, if any.
    pub fn to_act(&self) -> Option<usize> {
        self.pending.as_ref().map(|p| p.seat)
    }

    /// The pending decision, if any.
    pub fn request(&self) -> Option<PlacementRequest> {
        self.pending.clone()
    }

    /// Apply the acting seat's placement (`to_act` names it). Returns every
    /// event it caused: the `Place`, any deal that follows, and — when it was
    /// the hand's last turn — the showdown and score events.
    pub fn apply(&mut self, action: &OfcAction) -> Result<Vec<OfcEvent>, OfcError> {
        let request = self.pending.clone().ok_or(OfcError::NoPendingDecision)?;
        let seat = request.seat;

        if action.placements.len() != request.place as usize {
            return Err(OfcError::WrongPlacementCount {
                seat,
                expected: request.place,
                got: action.placements.len(),
            });
        }
        if action.discards.len() != request.discard as usize {
            return Err(OfcError::WrongDiscardCount {
                seat,
                expected: request.discard,
                got: action.discards.len(),
            });
        }

        // The dealt cards are distinct, so "already consumed" is exactly
        // "the action used this card twice".
        let mut unused = request.dealt.clone();
        let mut consume = |card: Card| match unused.iter().position(|c| *c == card) {
            Some(index) => {
                unused.swap_remove(index);
                Ok(())
            }
            None if request.dealt.contains(&card) => Err(OfcError::DuplicateCard { seat, card }),
            None => Err(OfcError::CardNotDealt { seat, card }),
        };

        let mut board = self.boards[seat].clone();
        for placement in &action.placements {
            consume(placement.card)?;
            if !board.push(*placement) {
                return Err(OfcError::RowFull {
                    seat,
                    card: placement.card,
                    row: placement.row,
                });
            }
        }
        for card in &action.discards {
            consume(*card)?;
        }
        debug_assert!(unused.is_empty(), "counts already forced a full account");

        self.boards[seat] = board;
        self.discarded[seat].extend_from_slice(&action.discards);
        self.dealt[seat].clear();
        self.pending = None;

        let start = self.events.len();
        self.emit(OfcEvent::Place {
            seat,
            placements: action.placements.clone(),
            discarded: action.discards.clone(),
            count: action.discards.len() as u8,
        });
        self.advance();
        Ok(self.events[start..].to_vec())
    }

    /// The settled hand's accounting, once every seat has placed.
    pub fn settlement(&self) -> Option<&OfcSettlement> {
        self.settlement.as_ref()
    }

    /// Every event emitted so far, unredacted.
    pub fn events(&self) -> &[OfcEvent] {
        &self.events
    }

    /// `ev` as observable by `viewer`, using this hand's fantasyland set —
    /// the engine is the only holder of the bit that decides whether a
    /// `Place` keeps its placements.
    pub fn redacted_for(&self, ev: &OfcEvent, viewer: usize) -> OfcEvent {
        ev.redacted_for(viewer, &self.hidden)
    }

    /// All boards, unredacted. The caller is responsible for hiding
    /// fantasyland boards from other seats (see [`Self::fantasyland`]).
    pub fn boards(&self) -> &[Board] {
        &self.boards
    }

    /// Per-seat fantasyland card counts *this* hand.
    pub fn fantasyland(&self) -> &[Option<u8>] {
        &self.fantasyland
    }

    /// Cards each seat has discarded so far.
    pub fn discarded(&self) -> &[Vec<Card>] {
        &self.discarded
    }

    pub fn spec(&self) -> &OfcSpec {
        &self.spec
    }

    pub fn hand_no(&self) -> u64 {
        self.hand_no
    }

    /// Fantasyland seats first (one turn each for the whole board), then the
    /// opening placement, then the rounds — every group in table order.
    fn schedule(&self) -> Vec<Turn> {
        let mut turns = Vec::new();
        for seat in table_order(self.seats) {
            if self.hidden[seat] {
                turns.push(Turn {
                    seat,
                    deal: 0,
                    place: Board::CAPACITY as u8,
                });
            }
        }
        for seat in table_order(self.seats) {
            if !self.hidden[seat] {
                turns.push(Turn {
                    seat,
                    deal: 0,
                    place: self.spec.initial_deal,
                });
            }
        }
        for _ in 0..self.spec.rounds {
            for seat in table_order(self.seats) {
                if !self.hidden[seat] {
                    turns.push(Turn {
                        seat,
                        deal: self.spec.round_deal,
                        place: self.spec.round_place,
                    });
                }
            }
        }
        turns
    }

    /// Move to the next turn — dealing its cards first when it has any — or
    /// settle the hand when the schedule is spent. Returns the events emitted.
    fn advance(&mut self) -> Vec<OfcEvent> {
        let start = self.events.len();
        match self.turns.get(self.next_turn).copied() {
            Some(turn) => {
                self.next_turn += 1;
                if turn.deal > 0 {
                    self.deal_to(turn.seat, turn.deal);
                }
                let dealt = self.dealt[turn.seat].clone();
                debug_assert!(dealt.len() >= turn.place as usize);
                self.pending = Some(PlacementRequest {
                    seat: turn.seat,
                    discard: dealt.len() as u8 - turn.place,
                    dealt,
                    place: turn.place,
                });
            }
            None => self.showdown(),
        }
        self.events[start..].to_vec()
    }

    /// Deal `count` cards to `seat` and announce them. The spec's card math
    /// guarantees the deck covers every deal of the hand.
    fn deal_to(&mut self, seat: usize, count: u8) {
        debug_assert!(
            self.deck.remaining() >= count as usize,
            "seat caps keep the deck from running out"
        );
        let cards = self
            .deck
            .draw_n(count as usize)
            .expect("seat caps keep the deck from running out");
        self.dealt[seat].extend_from_slice(&cards);
        self.emit(OfcEvent::Deal { seat, cards, count });
    }

    /// Reveal every board, settle, and close the hand.
    fn showdown(&mut self) {
        debug_assert!(self.boards.iter().all(Board::is_complete));

        let evals: Vec<Evaluated> = self
            .boards
            .iter()
            .map(|board| score::evaluate(&self.spec, board))
            .collect();
        let next_fantasyland: Vec<Option<u8>> = (0..self.seats)
            .map(|seat| {
                score::fantasyland(
                    &self.spec,
                    &self.boards[seat],
                    &evals[seat],
                    self.hidden[seat],
                )
            })
            .collect();
        let settlement = score::settle(&evals, next_fantasyland);

        for seat in table_order(self.seats) {
            let board = &self.boards[seat];
            let ev = &evals[seat];
            self.events.push(OfcEvent::Showdown {
                seat,
                top: board.top.clone(),
                middle: board.middle.clone(),
                bottom: board.bottom.clone(),
                top_value: ev.values.top,
                middle_value: ev.values.middle,
                bottom_value: ev.values.bottom,
                royalties: ev.royalties,
                fouled: ev.fouled,
                next_fantasyland: settlement.next_fantasyland[seat],
            });
        }
        for seat in table_order(self.seats) {
            self.events.push(OfcEvent::Score {
                seat,
                points: settlement.points[seat],
            });
        }
        self.settlement = Some(settlement);
    }

    fn emit(&mut self, ev: OfcEvent) {
        self.events.push(ev);
    }
}

/// Seat 1, 2, …, n−1, 0: the button (seat 0) always goes last. Every per-seat
/// iteration in a hand uses this order.
pub fn table_order(seats: usize) -> impl Iterator<Item = usize> {
    (1..seats).chain(core::iter::once(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_order_puts_the_button_last() {
        assert_eq!(table_order(2).collect::<Vec<_>>(), vec![1, 0]);
        assert_eq!(table_order(3).collect::<Vec<_>>(), vec![1, 2, 0]);
        assert_eq!(table_order(4).collect::<Vec<_>>(), vec![1, 2, 3, 0]);
    }
}

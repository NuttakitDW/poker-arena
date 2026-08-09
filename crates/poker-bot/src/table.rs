//! Betting-table state reconstructed from the event stream.
//!
//! The wire protocol deliberately sends no table state with `act`; the
//! event stream is the single source of truth. This tracker folds the
//! events into exactly what the policy needs: this bot's cards, the board,
//! everyone's visible upcards, per-seat commitments (the pot), folds, and
//! stacks.

use poker_core::card::Card;
use poker_wire::action::Action;
use poker_wire::event::Event;

/// Everything this bot knows about the hand in progress.
#[derive(Clone, Debug, Default)]
pub struct Table {
    /// This bot's seat for the current hand.
    pub seat: usize,
    /// This bot's private cards (draw results already applied).
    pub hole: Vec<Card>,
    /// Shared community cards.
    pub board: Vec<Card>,
    /// Per-seat face-up cards (stud).
    pub upcards: Vec<Vec<Card>>,
    /// Per-seat count of face-down cards (redacted deals still carry it).
    pub hidden_counts: Vec<usize>,
    /// Per-seat chips committed on completed streets, antes included.
    pub prev_streets: Vec<u64>,
    /// Per-seat commitment on the current street. Blinds and bring-ins
    /// count here (they are street commitments); `Acted.street_commit` is
    /// the absolute street total and overwrites it.
    pub street: Vec<u64>,
    /// Whether the hand's first `street-start` has passed. Blinds are
    /// posted *before* it and belong to the street it opens, so the first
    /// `street-start` must not fold them into `prev_streets`.
    seen_street_start: bool,
    /// Seats that have folded.
    pub folded: Vec<bool>,
    /// Stacks at hand start.
    pub stacks: Vec<u64>,
}

impl Table {
    /// Reset for a new hand; `seats` is the table size from `hello`.
    pub fn hand_start(&mut self, seat: usize, seats: usize) {
        self.seat = seat;
        self.hole.clear();
        self.board.clear();
        self.upcards = vec![Vec::new(); seats];
        self.hidden_counts = vec![0; seats];
        self.prev_streets = vec![0; seats];
        self.street = vec![0; seats];
        self.seen_street_start = false;
        self.folded = vec![false; seats];
        self.stacks = vec![0; seats];
    }

    /// Fold one observed event into the state.
    pub fn observe(&mut self, ev: &Event) {
        match ev {
            Event::HandStart { stacks, .. } => {
                self.stacks = stacks.clone();
            }
            Event::Post {
                seat, kind, amount, ..
            } => {
                // Antes join the pot but never count toward street
                // commitments; blinds and bring-ins do (the protocol's
                // street totals include them).
                if matches!(kind, poker_wire::event::PostKind::Ante) {
                    self.prev_streets[*seat] += amount;
                } else {
                    self.street[*seat] += amount;
                }
            }
            Event::StreetStart { .. } => {
                if self.seen_street_start {
                    for seat in 0..self.street.len() {
                        self.prev_streets[seat] += self.street[seat];
                        self.street[seat] = 0;
                    }
                } else {
                    self.seen_street_start = true;
                }
            }
            Event::DealHole { seat, cards, count } => {
                if *seat == self.seat {
                    self.hole.extend(cards.iter().copied());
                } else {
                    self.hidden_counts[*seat] += usize::from(*count);
                }
            }
            Event::DealCommunity { cards, .. } => {
                self.board.extend(cards.iter().copied());
            }
            Event::DealUp { seat, cards } => {
                self.upcards[*seat].extend(cards.iter().copied());
            }
            Event::Acted {
                seat,
                action,
                street_commit,
                ..
            } => {
                if matches!(action, Action::Fold) {
                    self.folded[*seat] = true;
                }
                // `street_commit` is the seat's absolute total for the
                // current street (blinds included); max() guards against
                // no-wager actions reporting a stale zero.
                self.street[*seat] = self.street[*seat].max(*street_commit);
            }
            Event::DrawResult {
                seat,
                discarded,
                drawn,
                ..
            } => {
                if *seat == self.seat {
                    self.hole.retain(|card| !discarded.contains(card));
                    self.hole.extend(drawn.iter().copied());
                }
            }
            _ => {}
        }
    }

    /// Chips in the pot right now.
    pub fn pot(&self) -> u64 {
        self.prev_streets.iter().sum::<u64>() + self.street.iter().sum::<u64>()
    }

    /// Opponent seats still contesting the pot.
    pub fn live_opponents(&self) -> usize {
        self.folded
            .iter()
            .enumerate()
            .filter(|(seat, folded)| *seat != self.seat && !**folded)
            .count()
    }

    /// This bot's commitment on the current street.
    pub fn my_street_commit(&self) -> u64 {
        self.street[self.seat]
    }

    /// Every card this bot can see (its own, the board, all upcards) —
    /// the dead cards for equity rollouts.
    pub fn visible_cards(&self) -> Vec<Card> {
        let mut cards = self.hole.clone();
        cards.extend(self.board.iter().copied());
        for seat_upcards in &self.upcards {
            cards.extend(seat_upcards.iter().copied());
        }
        cards
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use poker_core::card::parse_cards;
    use poker_wire::event::PostKind;

    fn deal(seat: usize, cards: &str, table: &mut Table) {
        let cards = parse_cards(cards).unwrap();
        let count = cards.len() as u8;
        table.observe(&Event::DealHole {
            seat,
            cards: if seat == table.seat {
                cards
            } else {
                Vec::new()
            },
            count,
        });
    }

    #[test]
    fn blinds_and_preflop_action_produce_the_right_pot() {
        let mut table = Table::default();
        table.hand_start(0, 2);
        table.observe(&Event::HandStart {
            hand_no: 1,
            button: 0,
            stacks: vec![10_000, 10_000],
        });
        table.observe(&Event::Post {
            seat: 0,
            kind: PostKind::SmallBlind,
            amount: 50,
            all_in: false,
        });
        table.observe(&Event::Post {
            seat: 1,
            kind: PostKind::BigBlind,
            amount: 100,
            all_in: false,
        });
        table.observe(&Event::StreetStart {
            street: 0,
            label: "preflop".into(),
        });
        deal(0, "As Kd", &mut table);
        deal(1, "2c 2d", &mut table);

        assert_eq!(table.pot(), 150);
        assert_eq!(table.hole, parse_cards("As Kd").unwrap());
        assert_eq!(table.hidden_counts[1], 2);

        // Button calls the blind: street_commit is the street total (100).
        table.observe(&Event::Acted {
            seat: 0,
            action: Action::Call,
            street_commit: 100,
            all_in: false,
        });
        assert_eq!(table.pot(), 200);
        assert_eq!(table.my_street_commit(), 100);

        // Big blind raises to 300 total on the street.
        table.observe(&Event::Acted {
            seat: 1,
            action: Action::Raise { to: 300 },
            street_commit: 300,
            all_in: false,
        });
        assert_eq!(table.pot(), 400);
    }

    #[test]
    fn street_start_rebases_commitments() {
        let mut table = Table::default();
        table.hand_start(1, 2);
        table.observe(&Event::Post {
            seat: 0,
            kind: PostKind::SmallBlind,
            amount: 50,
            all_in: false,
        });
        table.observe(&Event::Post {
            seat: 1,
            kind: PostKind::BigBlind,
            amount: 100,
            all_in: false,
        });
        table.observe(&Event::StreetStart {
            street: 0,
            label: "preflop".into(),
        });
        table.observe(&Event::Acted {
            seat: 0,
            action: Action::Call,
            street_commit: 100,
            all_in: false,
        });
        table.observe(&Event::StreetStart {
            street: 1,
            label: "flop".into(),
        });
        assert_eq!(table.my_street_commit(), 0);

        table.observe(&Event::Acted {
            seat: 1,
            action: Action::Bet { to: 200 },
            street_commit: 200,
            all_in: false,
        });
        assert_eq!(table.pot(), 400);
        assert_eq!(table.my_street_commit(), 200);
    }

    #[test]
    fn draw_results_replace_hole_cards_and_folds_are_tracked() {
        let mut table = Table::default();
        table.hand_start(0, 2);
        deal(0, "2c 3c 4c 5c Kd", &mut table);
        table.observe(&Event::DrawResult {
            seat: 0,
            discarded: parse_cards("Kd").unwrap(),
            drawn: parse_cards("6c").unwrap(),
            count: 1,
        });
        assert_eq!(table.hole, parse_cards("2c 3c 4c 5c 6c").unwrap());

        table.observe(&Event::Acted {
            seat: 1,
            action: Action::Fold,
            street_commit: 0,
            all_in: false,
        });
        assert!(table.folded[1]);
        assert_eq!(table.live_opponents(), 0);
    }
}

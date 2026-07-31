//! The OFC event stream: everything observable about a hand, in order.
//!
//! Same split as [`crate::event::Event`]: the engine emits *unredacted*
//! events; callers filter per observer with [`OfcEvent::redacted_for`]. The
//! one addition over the betting protocol is that redaction here also
//! depends on whether the placing seat is playing its hand face-down
//! (fantasyland) — an ordinary board is open-face and public even to
//! non-owners, so `Place` redaction needs that extra bit that `Event`'s
//! redaction never had to carry.

use crate::card::Card;
use crate::ofc::row::Placement;
use crate::value::HandValue;

/// Royalty points earned by each row, before fouling is applied (a fouled
/// hand's royalties are voided by scoring, not by this struct — the raw
/// per-row totals are always reported here).
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Royalties {
    pub top: u32,
    pub middle: u32,
    pub bottom: u32,
}

/// One observable occurrence in an OFC hand.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum OfcEvent {
    /// A seat enters the hand in fantasyland: `cards` is the count it will
    /// be dealt in the next `Deal` (13/14/15/16/17 depending on variant and
    /// entry condition).
    Fantasyland {
        seat: usize,
        cards: u8,
    },
    /// `cards` is private to `seat`; observers see `count` with empty cards.
    Deal {
        seat: usize,
        cards: Vec<Card>,
        count: u8,
    },
    /// A placement turn's result. `placements` is public — OFC boards are
    /// open-face — except when `seat` is playing this hand in fantasyland,
    /// in which case the board stays hidden from other seats until
    /// showdown. `discarded` is always private to `seat`; `count` (the
    /// discard count) is always public.
    Place {
        seat: usize,
        placements: Vec<Placement>,
        discarded: Vec<Card>,
        count: u8,
    },
    /// The full board revealed at showdown — for a fantasyland seat this is
    /// also the first time other seats see its board. Values are always
    /// computed, fouled hands included.
    Showdown {
        seat: usize,
        top: Vec<Card>,
        middle: Vec<Card>,
        bottom: Vec<Card>,
        top_value: HandValue,
        middle_value: HandValue,
        bottom_value: HandValue,
        royalties: Royalties,
        fouled: bool,
        /// Card count for the fantasyland hand this seat enters next, if
        /// any.
        next_fantasyland: Option<u8>,
    },
    Score {
        seat: usize,
        points: i64,
    },
    /// Catch-all for event types this build doesn't know about yet, so old
    /// bots don't fail hard against a newer arena. Never emitted by the
    /// engine — it only ever arrives from deserialization.
    #[serde(other)]
    Unknown,
}

impl OfcEvent {
    /// The event as observable by `viewer`. `fantasyland[seat]` says
    /// whether `seat` is playing this hand with its board hidden; the
    /// caller (the engine, which alone knows this) supplies it rather than
    /// this type carrying a private flag that could leak into
    /// serialization.
    ///
    /// `Deal` keeps cards only for `viewer == seat` (others get empty cards,
    /// count intact). `Place` keeps `discarded` only for `viewer == seat`
    /// (others get it emptied, count intact), and additionally empties
    /// `placements` for `viewer != seat` when `fantasyland[seat]`.
    /// `Showdown`/`Score`/`Fantasyland` pass through unchanged — nothing in
    /// them is ever private.
    pub fn redacted_for(&self, viewer: usize, fantasyland: &[bool]) -> OfcEvent {
        match self {
            OfcEvent::Deal { seat, count, .. } if viewer != *seat => OfcEvent::Deal {
                seat: *seat,
                cards: Vec::new(),
                count: *count,
            },
            OfcEvent::Place {
                seat,
                placements,
                count,
                ..
            } if viewer != *seat => OfcEvent::Place {
                seat: *seat,
                placements: if fantasyland.get(*seat).copied().unwrap_or(false) {
                    Vec::new()
                } else {
                    placements.clone()
                },
                discarded: Vec::new(),
                count: *count,
            },
            other => other.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{Rank, Suit};
    use crate::ofc::row::Row;

    fn c(rank: Rank, suit: Suit) -> Card {
        Card::new(rank, suit)
    }

    fn battery() -> Vec<OfcEvent> {
        vec![
            OfcEvent::Fantasyland { seat: 0, cards: 14 },
            OfcEvent::Deal {
                seat: 0,
                cards: vec![c(Rank::Ace, Suit::Spades), c(Rank::King, Suit::Diamonds)],
                count: 2,
            },
            OfcEvent::Deal {
                seat: 1,
                cards: Vec::new(),
                count: 2,
            },
            OfcEvent::Place {
                seat: 0,
                placements: vec![Placement {
                    card: c(Rank::Ace, Suit::Spades),
                    row: Row::Bottom,
                }],
                discarded: vec![c(Rank::Two, Suit::Clubs)],
                count: 1,
            },
            OfcEvent::Showdown {
                seat: 0,
                top: vec![c(Rank::Queen, Suit::Clubs), c(Rank::Queen, Suit::Diamonds)],
                middle: vec![],
                bottom: vec![],
                top_value: HandValue(100),
                middle_value: HandValue(0),
                bottom_value: HandValue(0),
                royalties: Royalties {
                    top: 1,
                    middle: 0,
                    bottom: 0,
                },
                fouled: false,
                next_fantasyland: Some(13),
            },
            OfcEvent::Showdown {
                seat: 1,
                top: vec![],
                middle: vec![],
                bottom: vec![],
                top_value: HandValue(0),
                middle_value: HandValue(0),
                bottom_value: HandValue(0),
                royalties: Royalties {
                    top: 0,
                    middle: 0,
                    bottom: 0,
                },
                fouled: true,
                next_fantasyland: None,
            },
            OfcEvent::Score {
                seat: 0,
                points: -6,
            },
            OfcEvent::Unknown,
        ]
    }

    #[test]
    fn event_round_trips_through_json() {
        for event in battery() {
            let text = serde_json::to_string(&event).unwrap();
            let back: OfcEvent = serde_json::from_str(&text).unwrap();
            assert_eq!(back, event);
        }
    }

    #[test]
    fn unknown_event_type_deserializes_to_unknown_variant() {
        let ev: OfcEvent = serde_json::from_str(r#"{"event":"some-future-event","x":1}"#).unwrap();
        assert_eq!(ev, OfcEvent::Unknown);
    }

    #[test]
    fn fantasyland_event_has_the_expected_exact_json() {
        let ev = OfcEvent::Fantasyland { seat: 2, cards: 14 };
        assert_eq!(
            serde_json::to_string(&ev).unwrap(),
            r#"{"event":"fantasyland","seat":2,"cards":14}"#
        );
    }

    #[test]
    fn deal_event_has_the_expected_exact_json_redacted_and_unredacted() {
        let unredacted = OfcEvent::Deal {
            seat: 0,
            cards: vec![c(Rank::Ace, Suit::Spades), c(Rank::King, Suit::Diamonds)],
            count: 2,
        };
        assert_eq!(
            serde_json::to_string(&unredacted).unwrap(),
            r#"{"event":"deal","seat":0,"cards":["As","Kd"],"count":2}"#
        );

        let redacted = unredacted.redacted_for(1, &[false, false]);
        assert_eq!(
            redacted,
            OfcEvent::Deal {
                seat: 0,
                cards: Vec::new(),
                count: 2,
            }
        );
        assert_eq!(
            serde_json::to_string(&redacted).unwrap(),
            r#"{"event":"deal","seat":0,"cards":[],"count":2}"#
        );

        // The dealt seat always sees its own cards, fantasyland or not.
        assert_eq!(unredacted.redacted_for(0, &[true, false]), unredacted);
    }

    #[test]
    fn place_event_has_the_expected_exact_json_normal_fantasyland_and_discard_redacted() {
        let normal = OfcEvent::Place {
            seat: 0,
            placements: vec![Placement {
                card: c(Rank::Ace, Suit::Spades),
                row: Row::Bottom,
            }],
            discarded: vec![c(Rank::Two, Suit::Clubs)],
            count: 1,
        };
        assert_eq!(
            serde_json::to_string(&normal).unwrap(),
            r#"{"event":"place","seat":0,"placements":[{"card":"As","row":"bottom"}],"discarded":["2c"],"count":1}"#
        );

        // Not in fantasyland: other seats keep the (public) placements, but
        // never the discards.
        let discard_redacted = normal.redacted_for(1, &[false, false]);
        assert_eq!(
            discard_redacted,
            OfcEvent::Place {
                seat: 0,
                placements: vec![Placement {
                    card: c(Rank::Ace, Suit::Spades),
                    row: Row::Bottom,
                }],
                discarded: Vec::new(),
                count: 1,
            }
        );
        assert_eq!(
            serde_json::to_string(&discard_redacted).unwrap(),
            r#"{"event":"place","seat":0,"placements":[{"card":"As","row":"bottom"}],"discarded":[],"count":1}"#
        );

        // In fantasyland: other seats lose the placements too.
        let fantasyland_redacted = normal.redacted_for(1, &[true, false]);
        assert_eq!(
            fantasyland_redacted,
            OfcEvent::Place {
                seat: 0,
                placements: Vec::new(),
                discarded: Vec::new(),
                count: 1,
            }
        );
        assert_eq!(
            serde_json::to_string(&fantasyland_redacted).unwrap(),
            r#"{"event":"place","seat":0,"placements":[],"discarded":[],"count":1}"#
        );

        // The placing seat always sees its own placements and discards.
        assert_eq!(normal.redacted_for(0, &[true, false]), normal);
    }

    #[test]
    fn showdown_event_has_the_expected_exact_json_with_and_without_next_fantasyland() {
        let with_next = OfcEvent::Showdown {
            seat: 0,
            top: vec![c(Rank::Queen, Suit::Clubs), c(Rank::Queen, Suit::Diamonds)],
            middle: vec![],
            bottom: vec![],
            top_value: HandValue(100),
            middle_value: HandValue(0),
            bottom_value: HandValue(0),
            royalties: Royalties {
                top: 1,
                middle: 0,
                bottom: 0,
            },
            fouled: false,
            next_fantasyland: Some(13),
        };
        assert_eq!(
            serde_json::to_string(&with_next).unwrap(),
            r#"{"event":"showdown","seat":0,"top":["Qc","Qd"],"middle":[],"bottom":[],"top_value":100,"middle_value":0,"bottom_value":0,"royalties":{"top":1,"middle":0,"bottom":0},"fouled":false,"next_fantasyland":13}"#
        );

        let without_next = OfcEvent::Showdown {
            seat: 0,
            top: vec![c(Rank::Queen, Suit::Clubs), c(Rank::Queen, Suit::Diamonds)],
            middle: vec![],
            bottom: vec![],
            top_value: HandValue(100),
            middle_value: HandValue(0),
            bottom_value: HandValue(0),
            royalties: Royalties {
                top: 1,
                middle: 0,
                bottom: 0,
            },
            fouled: false,
            next_fantasyland: None,
        };
        assert_eq!(
            serde_json::to_string(&without_next).unwrap(),
            r#"{"event":"showdown","seat":0,"top":["Qc","Qd"],"middle":[],"bottom":[],"top_value":100,"middle_value":0,"bottom_value":0,"royalties":{"top":1,"middle":0,"bottom":0},"fouled":false,"next_fantasyland":null}"#
        );
    }

    #[test]
    fn score_event_has_the_expected_exact_json() {
        let ev = OfcEvent::Score {
            seat: 1,
            points: -6,
        };
        assert_eq!(
            serde_json::to_string(&ev).unwrap(),
            r#"{"event":"score","seat":1,"points":-6}"#
        );
    }
}

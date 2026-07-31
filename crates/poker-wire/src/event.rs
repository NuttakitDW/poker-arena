//! The event stream: everything observable about a hand, in order.
//!
//! Events are the single source of truth — bots, hand logs, and statistics
//! all consume this stream. The engine emits *unredacted* events; callers
//! filter per observer with [`Event::redacted_for`].
//!
//! This is the one and only definition: the engine emits it and bots
//! deserialize it, so there is no mirror type to keep in sync and no way for
//! the log format and the wire format to drift apart.

use crate::action::{Action, Chips, Seat};
use crate::card::Card;
use crate::value::HandValue;

/// Forced-bet kinds.
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PostKind {
    Ante,
    SmallBlind,
    BigBlind,
    BringIn,
}

/// Which side of the pot an award came from.
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PotSide {
    /// Undivided pot (hi-only games, or hi-lo with no qualifying low).
    Whole,
    Hi,
    Lo,
}

/// One observable occurrence in a hand.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum Event {
    HandStart {
        hand_no: u64,
        button: Seat,
        stacks: Vec<Chips>,
    },
    Post {
        seat: Seat,
        kind: PostKind,
        amount: Chips,
        all_in: bool,
    },
    /// `cards` is private to `seat`; observers see `count` with empty cards.
    DealHole {
        seat: Seat,
        cards: Vec<Card>,
        count: u8,
    },
    StreetStart {
        street: u8,
        label: String,
    },
    DealCommunity {
        street: u8,
        cards: Vec<Card>,
    },
    /// Stud upcards (public).
    DealUp {
        seat: Seat,
        cards: Vec<Card>,
    },
    /// A voluntary action, exactly as validated by the engine.
    /// `street_commit` is the seat's total street commitment afterwards.
    Acted {
        seat: Seat,
        action: Action,
        street_commit: Chips,
        all_in: bool,
    },
    /// Draw-street result: discard count is public, `drawn` private.
    DrawResult {
        seat: Seat,
        discarded: u8,
        drawn: Vec<Card>,
    },
    /// A hand revealed at showdown (all non-folded hands are revealed; there
    /// is no mucking in an arena — information hiding between bots across
    /// hands is not a goal, honest stats are).
    ShowdownShow {
        seat: Seat,
        cards: Vec<Card>,
        hi: Option<HandValue>,
        lo: Option<HandValue>,
    },
    PotAwarded {
        /// Pot index: 0 = main pot, 1.. = side pots.
        pot: u8,
        side: PotSide,
        /// Winner(s) and the exact amount each received (odd chips resolved).
        winners: Vec<(Seat, Chips)>,
    },
    /// Terminal event; `nets[seat]` = winnings − contributions for the hand.
    HandEnd {
        nets: Vec<i64>,
    },
    /// Catch-all for event types this build doesn't know about yet, so old
    /// bots don't fail hard against a newer arena. Never emitted by the
    /// engine — it only ever arrives from deserialization.
    #[serde(other)]
    Unknown,
}

impl Event {
    /// The event as observable by `seat` (`None` for the table/log view).
    /// Private card contents are emptied for non-owners; counts remain.
    pub fn redacted_for(&self, observer: Option<Seat>) -> Event {
        match self {
            Event::DealHole { seat, cards, count } if observer != Some(*seat) => Event::DealHole {
                seat: *seat,
                cards: Vec::new(),
                count: *count,
            },
            Event::DrawResult {
                seat,
                discarded,
                drawn,
            } if observer != Some(*seat) => Event::DrawResult {
                seat: *seat,
                discarded: *discarded,
                drawn: Vec::new(),
            },
            other => other.clone(),
        }
    }
}

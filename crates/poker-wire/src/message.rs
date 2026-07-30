//! Wire message types.
//!
//! [`ArenaMsg`] (arena → bot) and [`BotMsg`] (bot → arena) are the two
//! top-level messages exchanged over a connection; both are internally
//! tagged on `"t"` with kebab-case variant names and carry an `Unknown`
//! catch-all for forward compatibility.
//!
//! [`WireEvent`] is a deserializable mirror of [`poker_core::game::Event`]
//! (core's is `Serialize`-only — see that type's docs) that serializes
//! byte-identically to it; see the `wire_event_fidelity` test below for the
//! contract this is held to.

use poker_core::card::Card;
use poker_core::eval::HandValue;
use poker_core::game::{Action, Event, LegalActions, Stakes};

/// Bot-facing description of the game being played. A deliberately small
/// subset of `poker_core::game::GameSpec`, which is `Serialize`-only by
/// design (its ids/labels are `&'static str` and its streets aren't
/// something bots need to reconstruct).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GameInfo {
    pub id: String,
    pub display_name: String,
    pub stakes: Stakes,
}

/// Messages sent from the arena to a bot.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "t", rename_all = "kebab-case")]
pub enum ArenaMsg {
    /// First message on a new connection: protocol and game parameters.
    Hello {
        proto: u32,
        game: GameInfo,
        seat_count: usize,
        starting_stack: u64,
        timeout_ms: Option<u64>,
    },
    /// A new hand is starting; `seat` is where this bot sits for this hand.
    /// The arena always seats the button at seat 0 and rotates *bots*
    /// between hands, so a seat number is also a position: seat 0 is the
    /// button, seat 1 the small blind, and so on. Everything else about the
    /// hand (stacks, the button, deals) arrives in the event stream.
    HandStart { hand_no: u64, seat: usize },
    /// An observable event, already redacted for this bot's seat.
    Event { hand_no: u64, ev: WireEvent },
    /// It is this bot's turn; reply with a `BotMsg::Action` conforming to
    /// `legal`. `deadline_ms` is echoed so bots can self-limit; the arena
    /// enforces the real deadline server-side regardless.
    ///
    /// Deliberately carries no table state: the event stream is the single
    /// source of truth (hole cards, board, upcards, stacks, pot, folds are
    /// all reconstructible from the events already delivered), and `legal`
    /// is here because legality must stay arena-authoritative — bots must
    /// never derive it themselves.
    Act {
        hand_no: u64,
        seat: usize,
        legal: LegalActions,
        deadline_ms: Option<u64>,
    },
    /// End-of-hand summary; `nets[seat]` is this bot's own result too.
    HandEnd { hand_no: u64, nets: Vec<i64> },
    /// The match is over; no further messages follow on this connection.
    MatchEnd {},
    /// Catch-all for message types this build doesn't know about yet, so
    /// old bots don't fail hard against a newer arena (and vice versa).
    #[serde(other)]
    Unknown,
}

/// Messages sent from a bot to the arena.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "t", rename_all = "kebab-case")]
pub enum BotMsg {
    /// First message on a new connection: identify to the arena.
    Join { name: String },
    /// Response to an `ArenaMsg::Act`.
    Action { action: Action },
    /// Catch-all for message types this build doesn't know about yet.
    #[serde(other)]
    Unknown,
}

/// Deserializable mirror of `poker_core::game::PostKind` (core's is
/// `Serialize`-only). Serializes identically: same kebab-case variant names.
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PostKind {
    Ante,
    SmallBlind,
    BigBlind,
    BringIn,
}

impl From<poker_core::game::PostKind> for PostKind {
    fn from(kind: poker_core::game::PostKind) -> PostKind {
        match kind {
            poker_core::game::PostKind::Ante => PostKind::Ante,
            poker_core::game::PostKind::SmallBlind => PostKind::SmallBlind,
            poker_core::game::PostKind::BigBlind => PostKind::BigBlind,
            poker_core::game::PostKind::BringIn => PostKind::BringIn,
        }
    }
}

/// Deserializable mirror of `poker_core::game::PotSide` (core's is
/// `Serialize`-only). Serializes identically: same kebab-case variant names.
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PotSide {
    Whole,
    Hi,
    Lo,
}

impl From<poker_core::game::PotSide> for PotSide {
    fn from(side: poker_core::game::PotSide) -> PotSide {
        match side {
            poker_core::game::PotSide::Whole => PotSide::Whole,
            poker_core::game::PotSide::Hi => PotSide::Hi,
            poker_core::game::PotSide::Lo => PotSide::Lo,
        }
    }
}

/// Deserializable mirror of `poker_core::game::Event`. Serializes
/// byte-identically to it (same tag `"event"`, same kebab-case variant and
/// field names/shapes); the only representational difference is
/// `label: String` where core has `&'static str`. `Card`, `Action`, and
/// `HandValue` are reused directly from `poker-core` since those already
/// round-trip through serde.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum WireEvent {
    HandStart {
        hand_no: u64,
        button: usize,
        stacks: Vec<u64>,
    },
    Post {
        seat: usize,
        kind: PostKind,
        amount: u64,
        all_in: bool,
    },
    /// `cards` is private to `seat`; observers see `count` with empty cards
    /// (mirrors `Event::redacted_for`).
    DealHole {
        seat: usize,
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
    DealUp {
        seat: usize,
        cards: Vec<Card>,
    },
    Acted {
        seat: usize,
        action: Action,
        street_commit: u64,
        all_in: bool,
    },
    DrawResult {
        seat: usize,
        discarded: u8,
        drawn: Vec<Card>,
    },
    ShowdownShow {
        seat: usize,
        cards: Vec<Card>,
        hi: Option<HandValue>,
        lo: Option<HandValue>,
    },
    PotAwarded {
        pot: u8,
        side: PotSide,
        winners: Vec<(usize, u64)>,
    },
    HandEnd {
        nets: Vec<i64>,
    },
    /// Catch-all for event types this build doesn't know about yet.
    #[serde(other)]
    Unknown,
}

impl From<&Event> for WireEvent {
    fn from(ev: &Event) -> WireEvent {
        match ev {
            Event::HandStart {
                hand_no,
                button,
                stacks,
            } => WireEvent::HandStart {
                hand_no: *hand_no,
                button: *button,
                stacks: stacks.clone(),
            },
            Event::Post {
                seat,
                kind,
                amount,
                all_in,
            } => WireEvent::Post {
                seat: *seat,
                kind: (*kind).into(),
                amount: *amount,
                all_in: *all_in,
            },
            Event::DealHole { seat, cards, count } => WireEvent::DealHole {
                seat: *seat,
                cards: cards.clone(),
                count: *count,
            },
            Event::StreetStart { street, label } => WireEvent::StreetStart {
                street: *street,
                label: (*label).to_string(),
            },
            Event::DealCommunity { street, cards } => WireEvent::DealCommunity {
                street: *street,
                cards: cards.clone(),
            },
            Event::DealUp { seat, cards } => WireEvent::DealUp {
                seat: *seat,
                cards: cards.clone(),
            },
            Event::Acted {
                seat,
                action,
                street_commit,
                all_in,
            } => WireEvent::Acted {
                seat: *seat,
                action: action.clone(),
                street_commit: *street_commit,
                all_in: *all_in,
            },
            Event::DrawResult {
                seat,
                discarded,
                drawn,
            } => WireEvent::DrawResult {
                seat: *seat,
                discarded: *discarded,
                drawn: drawn.clone(),
            },
            Event::ShowdownShow {
                seat,
                cards,
                hi,
                lo,
            } => WireEvent::ShowdownShow {
                seat: *seat,
                cards: cards.clone(),
                hi: *hi,
                lo: *lo,
            },
            Event::PotAwarded { pot, side, winners } => WireEvent::PotAwarded {
                pot: *pot,
                side: (*side).into(),
                winners: winners.clone(),
            },
            Event::HandEnd { nets } => WireEvent::HandEnd { nets: nets.clone() },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use poker_core::card::{Card as CoreCard, Rank, Suit};
    use poker_core::eval::HandValue as CoreHandValue;
    use poker_core::game::{
        Action as CoreAction, PostKind as CorePostKind, PotSide as CorePotSide,
    };

    fn c(rank: Rank, suit: Suit) -> CoreCard {
        CoreCard::new(rank, suit)
    }

    /// One `Event` of every variant, exercising the tricky corners called
    /// out in the spec: cards, a redacted `DealHole` (empty cards + count),
    /// `PotAwarded` with multiple winners, and a `HandEnd` with a negative
    /// net.
    fn battery() -> Vec<Event> {
        vec![
            Event::HandStart {
                hand_no: 7,
                button: 1,
                stacks: vec![10_000, 10_000],
            },
            Event::Post {
                seat: 0,
                kind: CorePostKind::SmallBlind,
                amount: 50,
                all_in: false,
            },
            Event::Post {
                seat: 1,
                kind: CorePostKind::BigBlind,
                amount: 100,
                all_in: true,
            },
            // Redacted: empty cards, count retained.
            Event::DealHole {
                seat: 0,
                cards: Vec::new(),
                count: 2,
            },
            // Unredacted: actual cards.
            Event::DealHole {
                seat: 1,
                cards: vec![c(Rank::Ace, Suit::Spades), c(Rank::King, Suit::Diamonds)],
                count: 2,
            },
            Event::StreetStart {
                street: 1,
                label: "flop",
            },
            Event::DealCommunity {
                street: 1,
                cards: vec![
                    c(Rank::Two, Suit::Clubs),
                    c(Rank::Seven, Suit::Hearts),
                    c(Rank::Nine, Suit::Spades),
                ],
            },
            Event::DealUp {
                seat: 0,
                cards: vec![c(Rank::Ten, Suit::Clubs)],
            },
            Event::Acted {
                seat: 0,
                action: CoreAction::Raise { to: 300 },
                street_commit: 300,
                all_in: false,
            },
            Event::Acted {
                seat: 1,
                action: CoreAction::Discard {
                    cards: vec![c(Rank::Two, Suit::Clubs)],
                },
                street_commit: 100,
                all_in: false,
            },
            Event::DrawResult {
                seat: 0,
                discarded: 1,
                drawn: vec![c(Rank::Ace, Suit::Hearts)],
            },
            Event::ShowdownShow {
                seat: 0,
                cards: vec![c(Rank::Ace, Suit::Spades), c(Rank::Ace, Suit::Diamonds)],
                hi: Some(CoreHandValue(12345)),
                lo: None,
            },
            Event::PotAwarded {
                pot: 0,
                side: CorePotSide::Whole,
                winners: vec![(0, 600)],
            },
            Event::PotAwarded {
                pot: 1,
                side: CorePotSide::Lo,
                winners: vec![(0, 50), (1, 50)],
            },
            Event::HandEnd {
                nets: vec![600, -600],
            },
        ]
    }

    #[test]
    fn wire_event_fidelity_matches_core_json_byte_for_byte() {
        for core_event in battery() {
            let core_json = serde_json::to_value(&core_event).unwrap();
            let wire_event = WireEvent::from(&core_event);
            let wire_json = serde_json::to_value(&wire_event).unwrap();
            assert_eq!(
                core_json, wire_json,
                "mismatch for {core_event:?}: core={core_json} wire={wire_json}"
            );
        }
    }

    #[test]
    fn wire_event_round_trips_through_json() {
        for core_event in battery() {
            let wire_event = WireEvent::from(&core_event);
            let text = serde_json::to_string(&wire_event).unwrap();
            let back: WireEvent = serde_json::from_str(&text).unwrap();
            assert_eq!(back, wire_event);
        }
    }

    fn sample_legal_actions() -> LegalActions {
        LegalActions {
            fold: true,
            check: false,
            call: Some(100),
            bet: None,
            raise: Some(poker_core::game::BetBounds {
                min_to: 200,
                max_to: 10_000,
            }),
            bring_in: None,
            draw: None,
        }
    }

    fn arena_msg_battery() -> Vec<ArenaMsg> {
        vec![
            ArenaMsg::Hello {
                proto: crate::PROTO_VERSION,
                game: GameInfo {
                    id: "holdem-nl".to_string(),
                    display_name: "No-Limit Texas Hold'em".to_string(),
                    stakes: Stakes::Blinds {
                        small_blind: 50,
                        big_blind: 100,
                    },
                },
                seat_count: 2,
                starting_stack: 10_000,
                timeout_ms: Some(5_000),
            },
            ArenaMsg::HandStart {
                hand_no: 1,
                seat: 0,
            },
            ArenaMsg::Event {
                hand_no: 1,
                ev: WireEvent::from(&Event::HandEnd {
                    nets: vec![100, -100],
                }),
            },
            ArenaMsg::Act {
                hand_no: 1,
                seat: 0,
                legal: sample_legal_actions(),
                deadline_ms: Some(5_000),
            },
            ArenaMsg::HandEnd {
                hand_no: 1,
                nets: vec![100, -100],
            },
            ArenaMsg::MatchEnd {},
        ]
    }

    fn bot_msg_battery() -> Vec<BotMsg> {
        vec![
            BotMsg::Join {
                name: "example-bot".to_string(),
            },
            BotMsg::Action {
                action: CoreAction::Call,
            },
            BotMsg::Action {
                action: CoreAction::Raise { to: 300 },
            },
        ]
    }

    #[test]
    fn arena_msg_round_trips() {
        for msg in arena_msg_battery() {
            let text = serde_json::to_string(&msg).unwrap();
            let back: ArenaMsg = serde_json::from_str(&text).unwrap();
            assert_eq!(back, msg);
        }
    }

    #[test]
    fn bot_msg_round_trips() {
        for msg in bot_msg_battery() {
            let text = serde_json::to_string(&msg).unwrap();
            let back: BotMsg = serde_json::from_str(&text).unwrap();
            assert_eq!(back, msg);
        }
    }

    /// Pin the exact wire format for two representative lines so a change to
    /// field names/order/case is caught by a test, not just by round-trip.
    #[test]
    fn hello_message_has_the_expected_exact_json() {
        let msg = ArenaMsg::Hello {
            proto: 1,
            game: GameInfo {
                id: "holdem-nl".to_string(),
                display_name: "No-Limit Texas Hold'em".to_string(),
                stakes: Stakes::Blinds {
                    small_blind: 50,
                    big_blind: 100,
                },
            },
            seat_count: 2,
            starting_stack: 10_000,
            timeout_ms: Some(5_000),
        };
        let text = serde_json::to_string(&msg).unwrap();
        assert_eq!(
            text,
            r#"{"t":"hello","proto":1,"game":{"id":"holdem-nl","display_name":"No-Limit Texas Hold'em","stakes":{"kind":"blinds","small_blind":50,"big_blind":100}},"seat_count":2,"starting_stack":10000,"timeout_ms":5000}"#
        );
    }

    #[test]
    fn action_message_has_the_expected_exact_json() {
        let msg = BotMsg::Action {
            action: CoreAction::Raise { to: 300 },
        };
        let text = serde_json::to_string(&msg).unwrap();
        assert_eq!(text, r#"{"t":"action","action":{"kind":"raise","to":300}}"#);
    }

    #[test]
    fn unknown_arena_msg_type_deserializes_to_unknown_variant() {
        let msg: ArenaMsg = serde_json::from_str(r#"{"t":"some-future-thing","x":1}"#).unwrap();
        assert_eq!(msg, ArenaMsg::Unknown);
    }

    #[test]
    fn unknown_bot_msg_type_deserializes_to_unknown_variant() {
        let msg: BotMsg = serde_json::from_str(r#"{"t":"some-future-thing","x":1}"#).unwrap();
        assert_eq!(msg, BotMsg::Unknown);
    }

    #[test]
    fn unknown_wire_event_type_deserializes_to_unknown_variant() {
        let ev: WireEvent = serde_json::from_str(r#"{"event":"some-future-event","x":1}"#).unwrap();
        assert_eq!(ev, WireEvent::Unknown);
    }

    #[test]
    fn unknown_fields_in_a_known_message_are_ignored() {
        let msg: BotMsg =
            serde_json::from_str(r#"{"t":"join","name":"bob","extra_field":123}"#).unwrap();
        assert_eq!(
            msg,
            BotMsg::Join {
                name: "bob".to_string()
            }
        );
    }
}

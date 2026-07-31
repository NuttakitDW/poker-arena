//! OFC message types.
//!
//! [`OfcArenaMsg`] (arena → bot) and [`OfcBotMsg`] (bot → arena) are the two
//! top-level messages exchanged over an OFC connection; both are internally
//! tagged on `"t"` with kebab-case variant names and carry an `Unknown`
//! catch-all for forward compatibility — the same shape as
//! [`crate::message::ArenaMsg`]/[`crate::message::BotMsg`], but this is a
//! separate protocol: OFC hands have no chips, no betting, no legal-actions
//! surface, only placement decisions.

use crate::ofc::event::OfcEvent;
use crate::ofc::row::OfcAction;

/// The decision offered to a bot at an `OfcArenaMsg::Act`. Only one shape
/// exists today — every OFC turn is a placement — but it is tagged like the
/// betting protocol's `WireDecision` so a future decision kind (e.g. a
/// fantasyland stay/drop choice, should one ever become a real decision
/// rather than automatic) is additive.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum OfcDecision {
    /// Place `place` of the just-dealt cards, discard `discard` of them.
    Place { place: u8, discard: u8 },
}

/// Messages sent from the arena to a bot.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "t", rename_all = "kebab-case")]
pub enum OfcArenaMsg {
    /// First message on a new connection: protocol and per-match parameters.
    Hello {
        proto: u32,
        /// Registry id, e.g. "ofc-pineapple". Bots are expected to know the
        /// variant's rules from its id; hello carries only the per-match
        /// parameters that cannot be derived from it.
        game_id: String,
        seat_count: usize,
        timeout_ms: Option<u64>,
    },
    /// Handshake acknowledgment: the arena-assigned name this bot competes
    /// under. Arrives only once every seat has connected — treat it as the
    /// authoritative identity in all match records.
    Joined { name: String },
    /// A new hand is starting; `seat` is where this bot sits for this hand.
    /// Bots rotate seats between hands; everything else about the hand
    /// (deals, placements, fantasyland) arrives in the event stream.
    HandStart { hand_no: u64, seat: usize },
    /// An observable event, already redacted for this bot's seat.
    Event { hand_no: u64, ev: OfcEvent },
    /// It is this bot's turn; reply with an `OfcBotMsg::Action` conforming
    /// to `decision`. `deadline_ms` is echoed so bots can self-limit; the
    /// arena enforces the real deadline server-side regardless.
    Act {
        hand_no: u64,
        seat: usize,
        decision: OfcDecision,
        deadline_ms: Option<u64>,
    },
    /// End-of-hand summary; `points[seat]` is this bot's own result too.
    HandEnd { hand_no: u64, points: Vec<i64> },
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
pub enum OfcBotMsg {
    /// First message on a new connection: "I'm ready". Carries nothing —
    /// bot identity is operator-assigned; the arena announces the assigned
    /// name in [`OfcArenaMsg::Joined`].
    Join {},
    /// Response to an `OfcArenaMsg::Act`.
    Action { action: OfcAction },
    /// Catch-all for message types this build doesn't know about yet.
    #[serde(other)]
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{Card, Rank, Suit};
    use crate::ofc::row::{Placement, Row};

    fn c(rank: Rank, suit: Suit) -> Card {
        Card::new(rank, suit)
    }

    fn arena_msg_battery() -> Vec<OfcArenaMsg> {
        vec![
            OfcArenaMsg::Hello {
                proto: crate::ofc::PROTO_VERSION,
                game_id: "ofc".to_string(),
                seat_count: 2,
                timeout_ms: Some(5_000),
            },
            OfcArenaMsg::Joined {
                name: "greedy".to_string(),
            },
            OfcArenaMsg::HandStart {
                hand_no: 1,
                seat: 0,
            },
            OfcArenaMsg::Event {
                hand_no: 1,
                ev: OfcEvent::Score { seat: 0, points: 6 },
            },
            OfcArenaMsg::Act {
                hand_no: 1,
                seat: 0,
                decision: OfcDecision::Place {
                    place: 1,
                    discard: 0,
                },
                deadline_ms: Some(5_000),
            },
            OfcArenaMsg::HandEnd {
                hand_no: 1,
                points: vec![6, -6],
            },
            OfcArenaMsg::MatchEnd {},
            OfcArenaMsg::Unknown,
        ]
    }

    fn bot_msg_battery() -> Vec<OfcBotMsg> {
        vec![
            OfcBotMsg::Join {},
            OfcBotMsg::Action {
                action: OfcAction {
                    placements: vec![Placement {
                        card: c(Rank::Ace, Suit::Spades),
                        row: Row::Bottom,
                    }],
                    discards: Vec::new(),
                },
            },
            OfcBotMsg::Unknown,
        ]
    }

    #[test]
    fn arena_msg_round_trips() {
        for msg in arena_msg_battery() {
            let text = serde_json::to_string(&msg).unwrap();
            let back: OfcArenaMsg = serde_json::from_str(&text).unwrap();
            assert_eq!(back, msg);
        }
    }

    #[test]
    fn bot_msg_round_trips() {
        for msg in bot_msg_battery() {
            let text = serde_json::to_string(&msg).unwrap();
            let back: OfcBotMsg = serde_json::from_str(&text).unwrap();
            assert_eq!(back, msg);
        }
    }

    #[test]
    fn decision_round_trips() {
        let decision = OfcDecision::Place {
            place: 2,
            discard: 1,
        };
        let text = serde_json::to_string(&decision).unwrap();
        let back: OfcDecision = serde_json::from_str(&text).unwrap();
        assert_eq!(back, decision);
    }

    /// Pin the exact wire format for representative lines so a change to
    /// field names/order/case is caught by a test, not just by round-trip.
    #[test]
    fn hello_message_has_the_expected_exact_json() {
        let msg = OfcArenaMsg::Hello {
            proto: 1,
            game_id: "ofc".to_string(),
            seat_count: 2,
            timeout_ms: Some(5_000),
        };
        let text = serde_json::to_string(&msg).unwrap();
        assert_eq!(
            text,
            r#"{"t":"hello","proto":1,"game_id":"ofc","seat_count":2,"timeout_ms":5000}"#
        );
    }

    #[test]
    fn act_message_with_place_decision_has_the_expected_exact_json() {
        let msg = OfcArenaMsg::Act {
            hand_no: 3,
            seat: 1,
            decision: OfcDecision::Place {
                place: 2,
                discard: 1,
            },
            deadline_ms: Some(5_000),
        };
        assert_eq!(
            serde_json::to_string(&msg).unwrap(),
            r#"{"t":"act","hand_no":3,"seat":1,"decision":{"kind":"place","place":2,"discard":1},"deadline_ms":5000}"#
        );
    }

    #[test]
    fn action_message_has_the_expected_exact_json() {
        let msg = OfcBotMsg::Action {
            action: OfcAction {
                placements: vec![Placement {
                    card: c(Rank::Ace, Suit::Spades),
                    row: Row::Bottom,
                }],
                discards: vec![c(Rank::Two, Suit::Clubs)],
            },
        };
        let text = serde_json::to_string(&msg).unwrap();
        assert_eq!(
            text,
            r#"{"t":"action","action":{"placements":[{"card":"As","row":"bottom"}],"discards":["2c"]}}"#
        );
    }

    #[test]
    fn unknown_arena_msg_type_deserializes_to_unknown_variant() {
        let msg: OfcArenaMsg = serde_json::from_str(r#"{"t":"some-future-thing","x":1}"#).unwrap();
        assert_eq!(msg, OfcArenaMsg::Unknown);
    }

    #[test]
    fn unknown_bot_msg_type_deserializes_to_unknown_variant() {
        let msg: OfcBotMsg = serde_json::from_str(r#"{"t":"some-future-thing","x":1}"#).unwrap();
        assert_eq!(msg, OfcBotMsg::Unknown);
    }

    #[test]
    fn unknown_fields_in_a_known_message_are_ignored() {
        let msg: OfcBotMsg = serde_json::from_str(r#"{"t":"join","extra_field":123}"#).unwrap();
        assert_eq!(msg, OfcBotMsg::Join {});
    }
}

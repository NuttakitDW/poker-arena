//! Open Face Chinese: four variants over one placement state machine.
//!
//! OFC is a different game from the betting variants in [`crate::game`] — no
//! chips, no blinds, no actions, no pots; seats place cards into three
//! fixed-capacity rows and settle in points, pairwise. It gets its own spec
//! type, its own engine and its own wire vocabulary rather than bending the
//! betting engine around a game that shares none of its concepts. The two are
//! frozen independently; nothing here changes the betting path.
//!
//! - [`spec`]: [`OfcSpec`] and the four-variant [`registry`].
//! - [`board`]: [`Board`], the three rows a seat fills.
//! - [`score`]: row values, royalties, fouling, fantasyland, settlement.
//! - [`state`]: [`OfcHandState`], the per-hand state machine, whose module
//!   doc is the authoritative OFC rules contract.
//!
//! The vocabulary the engine shares with bots — placements, actions, events,
//! messages, reports — is defined in [`poker_wire::ofc`] and re-exported here
//! so engine callers keep using `poker_core::ofc::…` paths throughout, the
//! same arrangement [`crate::game`] has with the betting vocabulary.

pub mod board;
pub mod score;
pub mod spec;
pub mod state;

/// The placement vocabulary, defined in [`poker_wire::ofc::row`].
pub mod row {
    pub use poker_wire::ofc::row::*;
}

/// The observable OFC event stream, defined in [`poker_wire::ofc::event`].
pub mod event {
    pub use poker_wire::ofc::event::*;
}

/// The OFC bot protocol messages, defined in [`poker_wire::ofc::message`].
pub mod message {
    pub use poker_wire::ofc::message::*;
}

/// The OFC report documents, defined in [`poker_wire::ofc::report`].
pub mod report {
    pub use poker_wire::ofc::report::*;
}

pub use poker_wire::ofc::{
    OFC_REPORT_SCHEMA_VERSION, OfcAction, OfcArenaMsg, OfcBotMsg, OfcBotReport, OfcDecision,
    OfcEvent, OfcMatchReport, OfcProgressReport, OfcStanding, PROTO_VERSION, Placement, Row,
    Royalties,
};

pub use board::Board;
pub use score::{Evaluated, OfcSettlement, RowValues};
pub use spec::{FantasylandRule, MiddleKind, OFC, OFC_27, OFC_PINEAPPLE, OFC_PROGRESSIVE, OfcSpec};
pub use state::{OfcError, OfcHandState, PlacementRequest, table_order};

/// Look up an OFC variant by registry id.
pub use spec::find;
/// Every OFC variant, in listing order.
pub use spec::registry;

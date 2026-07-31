//! Open Face Chinese (OFC): a separate wire protocol for the four OFC
//! variants, which place cards into fixed-capacity rows (top/middle/bottom)
//! and score in points rather than betting chips. It reuses this crate's
//! [`crate::Card`] and [`crate::HandValue`] but otherwise defines its own
//! messages, events, and decisions — an OFC hand has no chips, no actions,
//! no legal-actions surface, so overloading the betting protocol's types
//! would only blur two different games. `poker-wire`'s betting vocabulary
//! and this module are both frozen independently; nothing here changes it.
//!
//! - [`row`]: [`Row`], [`Placement`], [`OfcAction`] — the atoms of a
//!   placement decision.
//! - [`event`]: [`OfcEvent`], the single source of truth about an OFC hand,
//!   with the same emit-once/redact-per-observer split as
//!   [`crate::event::Event`] (see [`OfcEvent::redacted_for`]).
//! - [`message`]: [`OfcArenaMsg`] (arena → bot) and [`OfcBotMsg`] (bot →
//!   arena), tagged and unknown-tolerant like [`crate::message`].
//! - [`report`]: the JSON documents the OFC CLI emits for programmatic
//!   consumers, mirroring [`crate::report`] with points replacing chips.
//!
//! Framing is the same [`crate::framing`] JSON-lines codec, same
//! [`crate::MAX_LINE_BYTES`] cap. [`PROTO_VERSION`] is exchanged in
//! `OfcArenaMsg::Hello` and versions independently of the betting
//! protocol's [`crate::PROTO_VERSION`].

pub mod event;
pub mod message;
pub mod report;
pub mod row;

pub use event::{OfcEvent, Royalties};
pub use message::{OfcArenaMsg, OfcBotMsg, OfcDecision};
pub use report::{
    OFC_REPORT_SCHEMA_VERSION, OfcBotReport, OfcMatchReport, OfcProgressReport, OfcStanding,
};
pub use row::{OfcAction, Placement, Row};

/// OFC wire protocol version, exchanged in `OfcArenaMsg::Hello`.
pub const PROTO_VERSION: u32 = 1;

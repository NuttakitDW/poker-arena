//! # poker-wire
//!
//! The shared vocabulary of a poker match — cards, actions, events, stakes —
//! plus the versioned protocol messages and JSON-lines framing that carry
//! them. Dependency-light on purpose: a bot client links this crate and
//! nothing else, while the rules engine (`poker-core`) builds on top of the
//! very same definitions, so there is exactly one description of a card, an
//! action, or an event in the workspace.
//!
//! Transport-agnostic: everything works over any `std::io::Read`/`Write`;
//! sockets and subprocesses live in `poker-arena`.
//!
//! ## Vocabulary
//!
//! - [`card`]: [`Card`], [`Rank`], [`Suit`] — cards serialize as `"As"`,
//!   `"Td"`, `"2c"`.
//! - [`action`]: [`Action`] and the [`LegalActions`] that describe what may
//!   be done.
//! - [`event`]: [`Event`], the single source of truth about a hand. The
//!   engine emits it, logs record it, bots deserialize it.
//! - [`value`]: [`HandValue`] as it appears at showdown.
//! - [`game`]: [`Stakes`] and [`BettingKind`], the per-match parameters a
//!   bot cannot derive from a game id.
//!
//! ## Protocol summary
//!
//! - Framing: one compact JSON object per `\n`-terminated line (see
//!   [`framing`]), max [`MAX_LINE_BYTES`] per line.
//! - [`message::ArenaMsg`] flows arena → bot, [`message::BotMsg`] flows bot →
//!   arena. Both are tagged on `"t"` (kebab-case variants) and carry an
//!   `Unknown` catch-all so old and new builds can talk to each other;
//!   [`Event`] carries the same catch-all on `"event"`.
//! - [`PROTO_VERSION`] is exchanged in `ArenaMsg::Hello`; bump it on any
//!   breaking wire change.
//!
//! Full protocol documentation for bot authors (any language) lives in
//! `WIRE_PROTOCOL.md` at the workspace root.
//!
//! [`ofc`] is a second, independently versioned protocol for the OFC
//! (Open Face Chinese) variants: no chips, no betting, cards placed into
//! rows and scored in points.

pub mod action;
pub mod card;
pub mod event;
pub mod framing;
pub mod game;
pub mod message;
pub mod ofc;
pub mod report;
pub mod value;

pub use action::{Action, BetBounds, Chips, DrawBounds, LegalActions, Seat};
pub use card::{Card, ParseCardError, Rank, Suit, parse_cards};
pub use event::{Event, PostKind, PotSide};
pub use framing::{WireError, read_msg, write_msg};
pub use game::{BettingKind, Stakes};
pub use message::{ArenaMsg, BotMsg, WireDecision};
pub use value::{HandClass, HandValue};

/// Wire protocol version, exchanged in `ArenaMsg::Hello`.
pub const PROTO_VERSION: u32 = 1;

/// Maximum length, in bytes, of a single framed line (including the JSON
/// payload but not the trailing newline). Lines longer than this are
/// rejected by [`framing::read_msg`].
pub const MAX_LINE_BYTES: usize = 64 * 1024;

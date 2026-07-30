//! # poker-wire
//!
//! Versioned wire-protocol message definitions and JSON-lines framing for
//! poker-arena. Transport-agnostic: everything works over any
//! `std::io::Read`/`Write`; sockets and subprocesses live in `poker-arena`.
//!
//! ## Protocol summary
//!
//! - Framing: one compact JSON object per `\n`-terminated line (see
//!   [`framing`]), max [`MAX_LINE_BYTES`] per line.
//! - [`message::ArenaMsg`] flows arena → bot, [`message::BotMsg`] flows bot →
//!   arena. Both are tagged on `"t"` (kebab-case variants) and carry an
//!   `Unknown` catch-all so old and new builds can talk to each other.
//! - [`message::WireEvent`] mirrors `poker_core::game::Event` byte-for-byte
//!   and is additionally `Deserialize` (core's `Event` is `Serialize`-only).
//! - [`PROTO_VERSION`] is exchanged in `ArenaMsg::Hello`; bump it on any
//!   breaking wire change.
//!
//! Full protocol documentation for bot authors (any language) lives in
//! `docs/wire-protocol.md` at the workspace root.

pub mod framing;
pub mod message;

pub use framing::{WireError, read_msg, write_msg};
pub use message::{ArenaMsg, BotMsg, GameInfo, PostKind, PotSide, WireEvent};

/// Wire protocol version, exchanged in `ArenaMsg::Hello`.
pub const PROTO_VERSION: u32 = 1;

/// Maximum length, in bytes, of a single framed line (including the JSON
/// payload but not the trailing newline). Lines longer than this are
/// rejected by [`framing::read_msg`].
pub const MAX_LINE_BYTES: usize = 64 * 1024;

//! # poker-core
//!
//! Deterministic poker rules: hand evaluation, variant definitions, and the
//! per-hand state machine. Pure and I/O-free — reusable by arenas, solvers,
//! and analysis tools alike.
//!
//! The vocabulary this engine speaks — cards, actions, events, stakes — is
//! defined in `poker-wire` and re-exported here at its familiar paths, so
//! the types the engine computes with are literally the types that go on the
//! wire. The competition machinery (bots, match running, stats) lives in
//! `poker-arena`.

pub mod deck;
pub mod eval;
pub mod game;
pub mod rng;

/// Cards, ranks, and suits (defined in [`poker_wire::card`]), plus the
/// engine's own [`Deck`].
pub mod card {
    pub use crate::deck::Deck;
    pub use poker_wire::card::*;
}

pub use card::{Card, Deck, ParseCardError, Rank, Suit, parse_cards};
pub use rng::Rng64;

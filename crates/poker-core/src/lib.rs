//! # poker-core
//!
//! Deterministic poker rules: cards, hand evaluation, variant definitions,
//! and the per-hand state machine. Pure and I/O-free — reusable by arenas,
//! solvers, and analysis tools alike.
//!
//! The competition machinery (bots, wire protocol, match running, stats)
//! lives in the `poker-arena` and `poker-wire` crates.

pub mod card;
pub mod eval;
pub mod game;
pub mod rng;

pub use card::{Card, Deck, ParseCardError, Rank, Suit, parse_cards};
pub use rng::Rng64;

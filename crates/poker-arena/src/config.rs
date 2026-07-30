//! Match configuration.

use std::time::Duration;

use poker_core::game::{Chips, GameSpec};

/// How decks are dealt across the match.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DealingMode {
    /// One hand per deck; each deck draws a fresh seeded-random seating
    /// arrangement, so both position and neighbor order average out. Each
    /// hand's net is one observation per bot.
    Seeded,
    /// Duplicate dealing: each deck draws a seeded-random base arrangement
    /// and is replayed once per cyclic rotation of it (N rotations for N
    /// seats; heads-up = mirror pair), so every bot plays every seat with
    /// the same cards while neighbor arrangements vary deck to deck. A
    /// bot's *mean net per hand across the rotation set* is one
    /// observation — this is the variance killer.
    Duplicate,
}

/// What happens when a bot misbehaves (illegal action, timeout, disconnect,
/// crash, protocol garbage).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FaultPolicy {
    /// Substitute a check if free, else fold; continue the match. Faults are
    /// counted and reported.
    CheckFold,
    /// End the match immediately; the offender forfeits.
    Forfeit,
}

/// Full description of one match.
#[derive(Clone, Debug)]
pub struct MatchConfig {
    pub spec: GameSpec,
    /// Number of distinct decks. Total hands = `decks` (Seeded) or
    /// `decks × seats` (Duplicate).
    pub decks: u64,
    pub seed: u64,
    pub dealing: DealingMode,
    /// Starting stack, reset every hand (bot comparison measures per-hand
    /// EV, not bankroll trajectories). Typically 100 big blinds for big-bet
    /// games.
    pub starting_stack: Chips,
    pub fault_policy: FaultPolicy,
    /// Per-action deadline. Enforced as a hard deadline for wire bots;
    /// in-process bots run without deadline enforcement.
    pub timeout: Option<Duration>,
}

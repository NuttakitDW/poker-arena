//! Per-match game parameters carried in the hello handshake.
//!
//! Only the two things a bot cannot derive from a game id: what the seat
//! costs ([`Stakes`]) and how wagers are sized ([`BettingKind`]). The full
//! variant definition — streets, deals, showdown rules — is engine business
//! and lives in `poker_core::game::spec`.

use crate::action::Chips;

/// What a game costs to sit in. Two shapes because the families genuinely
/// differ: blind games post blinds; stud games post antes and a bring-in.
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Stakes {
    /// Blind games (hold'em, Omaha, draw). Fixed-limit variants use the
    /// standard convention small bet = big blind, big bet = 2 × big blind.
    Blinds {
        small_blind: Chips,
        big_blind: Chips,
    },
    /// Stud games: per-player ante, forced bring-in, and the two bet tiers.
    Stud {
        ante: Chips,
        bring_in: Chips,
        small_bet: Chips,
        big_bet: Chips,
    },
}

impl Stakes {
    /// The unit winnings are normalized in (bb/100): the big blind for
    /// blind games, the small bet for stud.
    pub fn rate_unit(&self) -> Chips {
        match self {
            Stakes::Blinds { big_blind, .. } => *big_blind,
            Stakes::Stud { small_bet, .. } => *small_bet,
        }
    }

    /// Small/big blind for blind games; a `Stud` stakes derives
    /// (small_bet / 2, small_bet) so blind-game constructors are total.
    pub fn blinds(&self) -> (Chips, Chips) {
        match self {
            Stakes::Blinds {
                small_blind,
                big_blind,
            } => (*small_blind, *big_blind),
            Stakes::Stud { small_bet, .. } => (small_bet / 2, *small_bet),
        }
    }

    /// (small_bet, big_bet) tier sizes: Blinds → (bb, 2*bb); Stud → explicit.
    pub fn tiers(&self) -> (Chips, Chips) {
        match self {
            Stakes::Blinds { big_blind, .. } => (*big_blind, *big_blind * 2),
            Stakes::Stud {
                small_bet, big_bet, ..
            } => (*small_bet, *big_bet),
        }
    }

    /// Normalize into stud numbers: Stud passes through; Blinds derives
    /// ante = bb/5 (min 1), bring_in = bb/2 (min 1), small_bet = bb,
    /// big_bet = 2*bb — exactly the current derivation.
    pub fn to_stud(&self) -> Stakes {
        match self {
            Stakes::Stud { .. } => *self,
            Stakes::Blinds { big_blind, .. } => Stakes::Stud {
                ante: (big_blind / 5).max(1),
                bring_in: (big_blind / 2).max(1),
                small_bet: *big_blind,
                big_bet: *big_blind * 2,
            },
        }
    }
}

/// The betting structure. Tagged on `kind`, so it reads on the wire as
/// `{"kind":"no-limit"}` | `{"kind":"pot-limit"}` |
/// `{"kind":"fixed-limit","raise_cap":4}` (`raise_cap` null = uncapped).
/// Bots need the cap to plan a street: without it, a fixed-limit bot cannot
/// tell how many more raises are legal.
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum BettingKind {
    /// Wagers are fixed at the street's tier size; raises are capped at
    /// `raise_cap` total wagers per round (the opening bet — or the big
    /// blind preflop — counts as the first). `None` = uncapped.
    FixedLimit {
        raise_cap: Option<u8>,
    },
    PotLimit,
    NoLimit,
}

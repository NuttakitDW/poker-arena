//! Data-driven variant definitions.
//!
//! A poker variant is *data*: a sequence of streets (each a deal plus an
//! optional betting round), a betting structure, forced bets, and a showdown
//! rule. The engine (`state.rs`) interprets specs; adding a variant means
//! writing a constructor here, not new engine code.

use core::ops::RangeInclusive;

use super::action::Chips;
use crate::eval::{EvalKind, HoleUsage};

/// Stakes for a game. For big-bet games (`NoLimit`/`PotLimit`) these are the
/// literal blinds. For fixed-limit games, convention: `small_bet == big_blind`
/// and `big_bet == 2 * big_blind` (so `Stakes { 50, 100 }` means a 100/200
/// limit game with a 50 small blind).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Stakes {
    pub small_blind: Chips,
    pub big_blind: Chips,
}

/// Forced bets posted before any cards are acted on.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub enum ForcedBets {
    /// Blind games. Heads-up: the button posts the small blind. `ante` is per
    /// player (0 for none).
    Blinds { ante: Chips },
    /// Stud games: everyone antes, lowest upcard posts the bring-in (M3).
    BringIn { ante: Chips, bring_in: Chips },
}

/// The betting structure.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
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

/// Fixed-limit bet sizing tier for a street (`Small` = big blind sized,
/// `Big` = 2× big blind). Ignored by big-bet games.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub enum BetTier {
    Small,
    Big,
}

/// Who opens a betting round.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub enum FirstToAct {
    /// First active seat left of the button (postflop convention; heads-up
    /// this is the big blind).
    LeftOfButton,
    /// First active seat after the last blind (preflop convention; heads-up
    /// this is the button/small blind). The big blind retains the option to
    /// raise when the pot is unraised back around.
    AfterBlinds,
    /// Stud: determined by visible upcards (M3).
    ByUpcards,
}

/// What is dealt at the start of a street.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub enum DealSpec {
    /// Nothing dealt (a pure betting round).
    None,
    /// `n` face-down cards to each active player.
    HolePrivate(u8),
    /// `n` shared community cards.
    Community(u8),
    /// `n` face-up cards to each active player (stud; M3).
    HoleUp(u8),
    /// A draw round: each active player may replace up to `max` cards (M3).
    Draw { max: u8 },
}

/// One street: a deal followed by an optional betting round.
/// (`Serialize` only: the `&'static str` label cannot be deserialized.)
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct StreetSpec {
    /// Human label used in events and logs ("preflop", "flop", …).
    pub label: &'static str,
    pub deal: DealSpec,
    pub betting: Option<BetRoundSpec>,
}

/// Betting round parameters for a street.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BetRoundSpec {
    pub tier: BetTier,
    pub first_to_act: FirstToAct,
}

/// How pots are contested at showdown.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub enum PotSplit {
    Hi(EvalKind),
    /// Split pot: half to `hi`, half to the best qualifying `lo`; if no hand
    /// qualifies for low, `hi` scoops. Odd chip goes to the hi side.
    HiLo {
        hi: EvalKind,
        lo: EvalKind,
    },
}

/// Showdown rules.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ShowdownSpec {
    pub pot_split: PotSplit,
    pub hole_usage: HoleUsage,
}

/// A complete variant definition.
/// (`Serialize` only: the `&'static str` ids cannot be deserialized.)
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct GameSpec {
    /// Registry identifier, e.g. `"holdem-nl"`.
    pub id: &'static str,
    pub display_name: &'static str,
    pub seats: RangeInclusive<u8>,
    pub stakes: Stakes,
    pub forced_bets: ForcedBets,
    pub betting: BettingKind,
    pub streets: Vec<StreetSpec>,
    pub showdown: ShowdownSpec,
}

impl GameSpec {
    /// No-limit Texas hold'em.
    pub fn holdem_nl(stakes: Stakes) -> GameSpec {
        GameSpec {
            id: "holdem-nl",
            display_name: "No-Limit Texas Hold'em",
            betting: BettingKind::NoLimit,
            ..Self::holdem_base(stakes)
        }
    }

    /// Fixed-limit Texas hold'em (1 bet + raises capped at 4 wagers/round).
    pub fn holdem_fl(stakes: Stakes) -> GameSpec {
        GameSpec {
            id: "holdem-fl",
            display_name: "Fixed-Limit Texas Hold'em",
            betting: BettingKind::FixedLimit { raise_cap: Some(4) },
            ..Self::holdem_base(stakes)
        }
    }

    fn holdem_base(stakes: Stakes) -> GameSpec {
        use BetTier::*;
        use FirstToAct::*;
        let street = |label, deal, tier, first_to_act| StreetSpec {
            label,
            deal,
            betting: Some(BetRoundSpec { tier, first_to_act }),
        };
        GameSpec {
            id: "holdem",
            display_name: "Texas Hold'em",
            seats: 2..=9,
            stakes,
            forced_bets: ForcedBets::Blinds { ante: 0 },
            betting: BettingKind::NoLimit,
            streets: vec![
                street("preflop", DealSpec::HolePrivate(2), Small, AfterBlinds),
                street("flop", DealSpec::Community(3), Small, LeftOfButton),
                street("turn", DealSpec::Community(1), Big, LeftOfButton),
                street("river", DealSpec::Community(1), Big, LeftOfButton),
            ],
            showdown: ShowdownSpec {
                pot_split: PotSplit::Hi(EvalKind::High),
                hole_usage: HoleUsage::Any,
            },
        }
    }

    /// Look up a variant by registry id.
    pub fn by_id(id: &str, stakes: Stakes) -> Option<GameSpec> {
        match id {
            "holdem-nl" => Some(Self::holdem_nl(stakes)),
            "holdem-fl" => Some(Self::holdem_fl(stakes)),
            _ => None,
        }
    }

    /// All registry ids, for CLI listings.
    pub fn known_ids() -> &'static [&'static str] {
        &["holdem-nl", "holdem-fl"]
    }

    /// Fixed-limit bet size for a tier under these stakes.
    pub fn tier_size(&self, tier: BetTier) -> Chips {
        match tier {
            BetTier::Small => self.stakes.big_blind,
            BetTier::Big => self.stakes.big_blind * 2,
        }
    }
}

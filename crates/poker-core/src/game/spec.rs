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
    /// Stud games: everyone antes, then the worst door card posts the
    /// bring-in. `low` flips the direction for razz-style games: when
    /// `false` (stud/stud8) the LOWEST upcard brings in and the best
    /// showing high hand leads later streets; when `true` (razz) the
    /// HIGHEST upcard brings in and the best showing *low* hand leads.
    /// Bring-in ties break by suit (clubs < diamonds < hearts < spades:
    /// lowest suit brings in for high games, highest for razz).
    BringIn {
        ante: Chips,
        bring_in: Chips,
        low: bool,
    },
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
    /// Stud: determined by visible upcards.
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
    /// `n` face-up cards to each active player (stud).
    HoleUp(u8),
    /// A draw round: each active player may replace up to `max` cards.
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

    /// Pot-limit Omaha (high only).
    pub fn omaha_pl(stakes: Stakes) -> GameSpec {
        GameSpec {
            id: "omaha-pl",
            display_name: "Pot-Limit Omaha",
            betting: BettingKind::PotLimit,
            ..Self::omaha_base(stakes)
        }
    }

    /// Pot-limit Omaha hi-lo, eight or better.
    pub fn omaha8_pl(stakes: Stakes) -> GameSpec {
        GameSpec {
            id: "omaha8-pl",
            display_name: "Pot-Limit Omaha Hi-Lo (8 or Better)",
            betting: BettingKind::PotLimit,
            showdown: Self::omaha8_showdown(),
            ..Self::omaha_base(stakes)
        }
    }

    /// Fixed-limit Omaha hi-lo, eight or better.
    pub fn omaha8_fl(stakes: Stakes) -> GameSpec {
        GameSpec {
            id: "omaha8-fl",
            display_name: "Fixed-Limit Omaha Hi-Lo (8 or Better)",
            betting: BettingKind::FixedLimit { raise_cap: Some(4) },
            showdown: Self::omaha8_showdown(),
            ..Self::omaha_base(stakes)
        }
    }

    /// Omaha differs from hold'em only in hole-card count and the
    /// exactly-two showdown constraint; streets and blinds are identical.
    fn omaha_base(stakes: Stakes) -> GameSpec {
        let mut spec = Self::holdem_base(stakes);
        spec.id = "omaha";
        spec.display_name = "Omaha";
        spec.streets[0].deal = DealSpec::HolePrivate(4);
        spec.showdown.hole_usage = HoleUsage::ExactlyTwo;
        spec
    }

    fn omaha8_showdown() -> ShowdownSpec {
        ShowdownSpec {
            pot_split: PotSplit::HiLo {
                hi: EvalKind::High,
                lo: EvalKind::EightOrBetterLow,
            },
            hole_usage: HoleUsage::ExactlyTwo,
        }
    }

    /// Look up a variant by registry id.
    pub fn by_id(id: &str, stakes: Stakes) -> Option<GameSpec> {
        match id {
            "holdem-nl" => Some(Self::holdem_nl(stakes)),
            "holdem-fl" => Some(Self::holdem_fl(stakes)),
            "omaha-pl" => Some(Self::omaha_pl(stakes)),
            "omaha8-pl" => Some(Self::omaha8_pl(stakes)),
            "omaha8-fl" => Some(Self::omaha8_fl(stakes)),
            "stud-fl" => Some(Self::stud_fl(stakes)),
            "stud8-fl" => Some(Self::stud8_fl(stakes)),
            "razz-fl" => Some(Self::razz_fl(stakes)),
            "27td-fl" => Some(Self::td27_fl(stakes)),
            "a5td-fl" => Some(Self::a5td_fl(stakes)),
            "badugi-fl" => Some(Self::badugi_fl(stakes)),
            "5cd-nl" => Some(Self::fcd_nl(stakes)),
            _ => None,
        }
    }

    /// All registry ids, for CLI listings.
    pub fn known_ids() -> &'static [&'static str] {
        &[
            "holdem-nl",
            "holdem-fl",
            "omaha-pl",
            "omaha8-pl",
            "omaha8-fl",
            "stud-fl",
            "stud8-fl",
            "razz-fl",
            "27td-fl",
            "a5td-fl",
            "badugi-fl",
            "5cd-nl",
        ]
    }

    /// Seven-card stud, fixed limit.
    pub fn stud_fl(stakes: Stakes) -> GameSpec {
        GameSpec {
            id: "stud-fl",
            display_name: "Seven-Card Stud",
            ..Self::stud_base(stakes, false)
        }
    }

    /// Seven-card stud hi-lo (eight or better), fixed limit.
    pub fn stud8_fl(stakes: Stakes) -> GameSpec {
        GameSpec {
            id: "stud8-fl",
            display_name: "Seven-Card Stud Hi-Lo (8 or Better)",
            showdown: ShowdownSpec {
                pot_split: PotSplit::HiLo {
                    hi: EvalKind::High,
                    lo: EvalKind::EightOrBetterLow,
                },
                hole_usage: HoleUsage::Any,
            },
            ..Self::stud_base(stakes, false)
        }
    }

    /// Razz (seven-card stud played for A-5 low), fixed limit.
    pub fn razz_fl(stakes: Stakes) -> GameSpec {
        GameSpec {
            id: "razz-fl",
            display_name: "Razz",
            showdown: ShowdownSpec {
                pot_split: PotSplit::Hi(EvalKind::AceToFiveLow),
                hole_usage: HoleUsage::Any,
            },
            ..Self::stud_base(stakes, true)
        }
    }

    /// Shared stud skeleton. Stakes follow the fixed-limit convention
    /// (small bet = big blind, big bet = 2×); derived forced bets: ante =
    /// small bet / 5, bring-in = small bet / 2 (each at least 1). Seats cap
    /// at 7 so a full run-out never exhausts the deck (7 × 7 = 49 ≤ 52);
    /// the 8-handed shared-community-card fallback is deliberately out of
    /// scope. Third street is two spec streets: a bet-less deal of the down
    /// cards, then the door card with the bring-in betting round.
    fn stud_base(stakes: Stakes, low: bool) -> GameSpec {
        use BetTier::*;
        let small_bet = stakes.big_blind;
        let street = |label, deal, tier| StreetSpec {
            label,
            deal,
            betting: Some(BetRoundSpec {
                tier,
                first_to_act: FirstToAct::ByUpcards,
            }),
        };
        GameSpec {
            id: "stud",
            display_name: "Seven-Card Stud",
            seats: 2..=7,
            stakes,
            forced_bets: ForcedBets::BringIn {
                ante: (small_bet / 5).max(1),
                bring_in: (small_bet / 2).max(1),
                low,
            },
            betting: BettingKind::FixedLimit { raise_cap: Some(4) },
            streets: vec![
                StreetSpec {
                    label: "deal",
                    deal: DealSpec::HolePrivate(2),
                    betting: None,
                },
                street("third", DealSpec::HoleUp(1), Small),
                street("fourth", DealSpec::HoleUp(1), Small),
                street("fifth", DealSpec::HoleUp(1), Big),
                street("sixth", DealSpec::HoleUp(1), Big),
                street("seventh", DealSpec::HolePrivate(1), Big),
            ],
            showdown: ShowdownSpec {
                pot_split: PotSplit::Hi(EvalKind::High),
                hole_usage: HoleUsage::Any,
            },
        }
    }

    /// 2-7 (Kansas City) triple draw, fixed limit.
    pub fn td27_fl(stakes: Stakes) -> GameSpec {
        GameSpec {
            id: "27td-fl",
            display_name: "2-7 Triple Draw",
            showdown: Self::draw_showdown(EvalKind::DeuceToSevenLow),
            ..Self::triple_draw_base(stakes, 5)
        }
    }

    /// A-5 (California) triple draw, fixed limit.
    pub fn a5td_fl(stakes: Stakes) -> GameSpec {
        GameSpec {
            id: "a5td-fl",
            display_name: "A-5 Triple Draw",
            showdown: Self::draw_showdown(EvalKind::AceToFiveLow),
            ..Self::triple_draw_base(stakes, 5)
        }
    }

    /// Badugi, fixed limit.
    pub fn badugi_fl(stakes: Stakes) -> GameSpec {
        GameSpec {
            id: "badugi-fl",
            display_name: "Badugi",
            showdown: Self::draw_showdown(EvalKind::Badugi),
            ..Self::triple_draw_base(stakes, 4)
        }
    }

    /// Five-card draw, no limit (single draw).
    pub fn fcd_nl(stakes: Stakes) -> GameSpec {
        use BetTier::*;
        use FirstToAct::*;
        let street = |label, deal, tier, first_to_act| StreetSpec {
            label,
            deal,
            betting: Some(BetRoundSpec { tier, first_to_act }),
        };
        GameSpec {
            id: "5cd-nl",
            display_name: "No-Limit Five-Card Draw",
            seats: 2..=6,
            stakes,
            forced_bets: ForcedBets::Blinds { ante: 0 },
            betting: BettingKind::NoLimit,
            streets: vec![
                street("predraw", DealSpec::HolePrivate(5), Small, AfterBlinds),
                street("draw", DealSpec::Draw { max: 5 }, Small, LeftOfButton),
            ],
            showdown: Self::draw_showdown(EvalKind::High),
        }
    }

    /// Triple-draw skeleton: blinds, three draws, small bets through the
    /// first draw round and big bets after. Seats cap at 6 (standard for
    /// draw games); heavy multiway drawing can still exhaust the deck, which
    /// the engine handles by reshuffling the discards.
    fn triple_draw_base(stakes: Stakes, hand_size: u8) -> GameSpec {
        use BetTier::*;
        use FirstToAct::*;
        let street = |label, deal, tier, first_to_act| StreetSpec {
            label,
            deal,
            betting: Some(BetRoundSpec { tier, first_to_act }),
        };
        let draw = DealSpec::Draw { max: hand_size };
        GameSpec {
            id: "triple-draw",
            display_name: "Triple Draw",
            seats: 2..=6,
            stakes,
            forced_bets: ForcedBets::Blinds { ante: 0 },
            betting: BettingKind::FixedLimit { raise_cap: Some(4) },
            streets: vec![
                street(
                    "predraw",
                    DealSpec::HolePrivate(hand_size),
                    Small,
                    AfterBlinds,
                ),
                street("draw1", draw.clone(), Small, LeftOfButton),
                street("draw2", draw.clone(), Big, LeftOfButton),
                street("draw3", draw, Big, LeftOfButton),
            ],
            showdown: Self::draw_showdown(EvalKind::High),
        }
    }

    fn draw_showdown(kind: EvalKind) -> ShowdownSpec {
        ShowdownSpec {
            pot_split: PotSplit::Hi(kind),
            hole_usage: HoleUsage::AllOwn,
        }
    }

    /// Fixed-limit bet size for a tier under these stakes.
    pub fn tier_size(&self, tier: BetTier) -> Chips {
        match tier {
            BetTier::Small => self.stakes.big_blind,
            BetTier::Big => self.stakes.big_blind * 2,
        }
    }
}

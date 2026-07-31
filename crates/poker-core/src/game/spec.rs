//! Data-driven variant definitions.
//!
//! A poker variant is *data*: a sequence of streets (each a deal plus an
//! optional betting round), a betting structure, forced bets, and a showdown
//! rule. The engine (`state.rs`) interprets specs; adding a variant means
//! writing a constructor here, not new engine code.

use core::ops::RangeInclusive;

use super::action::Chips;
use crate::eval::{EvalKind, HoleUsage};

/// The per-match parameters that also travel to bots on the wire; defined in
/// `poker-wire` and re-exported here so `game::spec::Stakes` keeps resolving.
pub use poker_wire::game::{BettingKind, Stakes};

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

/// One contested half of the pot: an evaluator plus its hole-usage rule.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ShowdownSide {
    pub kind: EvalKind,
    /// How this side combines hole cards with the board — per side, because
    /// drawmaha contests the omaha half `ExactlyTwo` and the in-hand half
    /// `AllOwn` from the same five cards.
    pub usage: HoleUsage,
}

/// Showdown rules: the hi side always exists; `lo` makes it a split game.
/// Each half goes to its best *qualifying* hand (total evaluators always
/// qualify). One qualifying side scoops; if neither side qualifies anywhere
/// (both kinds qualifiers, e.g. archie), the pot splits evenly among the
/// showdown players. Odd chip to the hi side of a split.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ShowdownSpec {
    pub hi: ShowdownSide,
    pub lo: Option<ShowdownSide>,
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
        let (small_blind, big_blind) = stakes.blinds();
        let street = |label, deal, tier, first_to_act| StreetSpec {
            label,
            deal,
            betting: Some(BetRoundSpec { tier, first_to_act }),
        };
        GameSpec {
            id: "holdem",
            display_name: "Texas Hold'em",
            seats: 2..=9,
            stakes: Stakes::Blinds {
                small_blind,
                big_blind,
            },
            forced_bets: ForcedBets::Blinds { ante: 0 },
            betting: BettingKind::NoLimit,
            streets: vec![
                street("preflop", DealSpec::HolePrivate(2), Small, AfterBlinds),
                street("flop", DealSpec::Community(3), Small, LeftOfButton),
                street("turn", DealSpec::Community(1), Big, LeftOfButton),
                street("river", DealSpec::Community(1), Big, LeftOfButton),
            ],
            showdown: ShowdownSpec {
                hi: ShowdownSide {
                    kind: EvalKind::High,
                    usage: HoleUsage::Any,
                },
                lo: None,
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
        Self::omaha_base_n(stakes, 4)
    }

    /// Omaha skeleton with `hole` hole cards (4 = Omaha, 5 = Big O).
    fn omaha_base_n(stakes: Stakes, hole: u8) -> GameSpec {
        let mut spec = Self::holdem_base(stakes);
        spec.id = "omaha";
        spec.display_name = "Omaha";
        spec.streets[0].deal = DealSpec::HolePrivate(hole);
        spec.showdown.hi.usage = HoleUsage::ExactlyTwo;
        if let Some(lo) = &mut spec.showdown.lo {
            lo.usage = HoleUsage::ExactlyTwo;
        }
        spec
    }

    /// Big O (five-card Omaha hi-lo eight-or-better), pot limit. Nine seats
    /// fit exactly: 5 × 9 + 5 = 50 ≤ 52.
    pub fn bigo_pl(stakes: Stakes) -> GameSpec {
        GameSpec {
            id: "bigo-pl",
            display_name: "Big O (Five-Card Omaha Hi-Lo)",
            betting: BettingKind::PotLimit,
            showdown: Self::omaha8_showdown(),
            ..Self::omaha_base_n(stakes, 5)
        }
    }

    /// Badacey: five-card triple draw, split between the best A-5 low (the
    /// hi slot — the five-card hand side takes the odd chip) and the best
    /// badugi, aces low in both halves. Both halves always exist, so the
    /// pot always splits.
    pub fn badacey_fl(stakes: Stakes) -> GameSpec {
        GameSpec {
            id: "badacey-fl",
            display_name: "Badacey",
            showdown: ShowdownSpec {
                hi: ShowdownSide {
                    kind: EvalKind::AceToFiveLow,
                    usage: HoleUsage::AllOwn,
                },
                lo: Some(ShowdownSide {
                    kind: EvalKind::Badugi,
                    usage: HoleUsage::AllOwn,
                }),
            },
            ..Self::triple_draw_base(stakes, 5)
        }
    }

    /// Badeucy: five-card triple draw, split between the best 2-7 low (the
    /// hi slot — the five-card hand side takes the odd chip) and the best
    /// ace-HIGH badugi; aces are high in both halves. Both halves always
    /// exist, so the pot always splits.
    pub fn badeucy_fl(stakes: Stakes) -> GameSpec {
        GameSpec {
            id: "badeucy-fl",
            display_name: "Badeucy",
            showdown: ShowdownSpec {
                hi: ShowdownSide {
                    kind: EvalKind::DeuceToSevenLow,
                    usage: HoleUsage::AllOwn,
                },
                lo: Some(ShowdownSide {
                    kind: EvalKind::BadugiAceHigh,
                    usage: HoleUsage::AllOwn,
                }),
            },
            ..Self::triple_draw_base(stakes, 5)
        }
    }

    /// Archie: five-card triple draw, split between high (sixes-or-better
    /// qualifier) and A-5 low (eight-or-better qualifier). One qualifying
    /// side scoops; if neither side qualifies anywhere, the pot splits
    /// evenly among the showdown players.
    pub fn archie_fl(stakes: Stakes) -> GameSpec {
        GameSpec {
            id: "archie-fl",
            display_name: "Archie",
            showdown: ShowdownSpec {
                hi: ShowdownSide {
                    kind: EvalKind::SixesOrBetterHigh,
                    usage: HoleUsage::AllOwn,
                },
                lo: Some(ShowdownSide {
                    kind: EvalKind::EightOrBetterLow,
                    usage: HoleUsage::AllOwn,
                }),
            },
            ..Self::triple_draw_base(stakes, 5)
        }
    }

    fn omaha8_showdown() -> ShowdownSpec {
        ShowdownSpec {
            hi: ShowdownSide {
                kind: EvalKind::High,
                usage: HoleUsage::ExactlyTwo,
            },
            lo: Some(ShowdownSide {
                kind: EvalKind::EightOrBetterLow,
                usage: HoleUsage::ExactlyTwo,
            }),
        }
    }

    /// Drawmaha: five hole cards over a hold'em-style board with one draw
    /// between the flop and turn betting rounds (the draw street itself has
    /// no betting). The pot splits between the omaha half (exactly two hole
    /// cards + three board, high) and the in-hand half — the whole five-card
    /// hand under the variant's evaluator. The in-hand half is the hi slot
    /// (it takes the odd chip); the omaha half is the lo slot. Both halves
    /// always exist, so the pot always splits.
    pub fn drawmaha_fl(stakes: Stakes) -> GameSpec {
        Self::drawmaha_base(stakes, "drawmaha-fl", "Drawmaha", EvalKind::High)
    }

    /// Drawmaha 2-7: the in-hand half is 2-7 lowball.
    pub fn drawmaha27_fl(stakes: Stakes) -> GameSpec {
        Self::drawmaha_base(
            stakes,
            "drawmaha-27-fl",
            "Drawmaha 2-7",
            EvalKind::DeuceToSevenLow,
        )
    }

    /// Drawmaha Dugi: the in-hand half is badugi (best four of five).
    pub fn drawmaha_dugi_fl(stakes: Stakes) -> GameSpec {
        Self::drawmaha_base(
            stakes,
            "drawmaha-dugi-fl",
            "Drawmaha Dugi",
            EvalKind::Badugi,
        )
    }

    fn drawmaha_base(
        stakes: Stakes,
        id: &'static str,
        display_name: &'static str,
        hand_half: EvalKind,
    ) -> GameSpec {
        use BetTier::*;
        use FirstToAct::*;
        let street = |label, deal, tier, first_to_act| StreetSpec {
            label,
            deal,
            betting: Some(BetRoundSpec { tier, first_to_act }),
        };
        GameSpec {
            id,
            display_name,
            seats: 2..=6,
            stakes,
            forced_bets: ForcedBets::Blinds { ante: 0 },
            betting: BettingKind::FixedLimit { raise_cap: Some(4) },
            streets: vec![
                street("preflop", DealSpec::HolePrivate(5), Small, AfterBlinds),
                street("flop", DealSpec::Community(3), Small, LeftOfButton),
                StreetSpec {
                    label: "draw",
                    deal: DealSpec::Draw { max: 5 },
                    betting: None,
                },
                street("turn", DealSpec::Community(1), Big, LeftOfButton),
                street("river", DealSpec::Community(1), Big, LeftOfButton),
            ],
            showdown: ShowdownSpec {
                hi: ShowdownSide {
                    kind: hand_half,
                    usage: HoleUsage::AllOwn,
                },
                lo: Some(ShowdownSide {
                    kind: EvalKind::High,
                    usage: HoleUsage::ExactlyTwo,
                }),
            },
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
            "badacey-fl" => Some(Self::badacey_fl(stakes)),
            "badeucy-fl" => Some(Self::badeucy_fl(stakes)),
            "archie-fl" => Some(Self::archie_fl(stakes)),
            "bigo-pl" => Some(Self::bigo_pl(stakes)),
            "drawmaha-fl" => Some(Self::drawmaha_fl(stakes)),
            "drawmaha-27-fl" => Some(Self::drawmaha27_fl(stakes)),
            "drawmaha-dugi-fl" => Some(Self::drawmaha_dugi_fl(stakes)),
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
            "badacey-fl",
            "badeucy-fl",
            "archie-fl",
            "bigo-pl",
            "drawmaha-fl",
            "drawmaha-27-fl",
            "drawmaha-dugi-fl",
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
                hi: ShowdownSide {
                    kind: EvalKind::High,
                    usage: HoleUsage::Any,
                },
                lo: Some(ShowdownSide {
                    kind: EvalKind::EightOrBetterLow,
                    usage: HoleUsage::Any,
                }),
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
                hi: ShowdownSide {
                    kind: EvalKind::AceToFiveLow,
                    usage: HoleUsage::Any,
                },
                lo: None,
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
        let stud_stakes = stakes.to_stud();
        let Stakes::Stud { ante, bring_in, .. } = stud_stakes else {
            unreachable!("to_stud() always returns Stakes::Stud")
        };
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
            stakes: stud_stakes,
            forced_bets: ForcedBets::BringIn {
                ante,
                bring_in,
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
                hi: ShowdownSide {
                    kind: EvalKind::High,
                    usage: HoleUsage::Any,
                },
                lo: None,
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
        let (small_blind, big_blind) = stakes.blinds();
        GameSpec {
            id: "5cd-nl",
            display_name: "No-Limit Five-Card Draw",
            seats: 2..=6,
            stakes: Stakes::Blinds {
                small_blind,
                big_blind,
            },
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
        let (small_blind, big_blind) = stakes.blinds();
        GameSpec {
            id: "triple-draw",
            display_name: "Triple Draw",
            seats: 2..=6,
            stakes: Stakes::Blinds {
                small_blind,
                big_blind,
            },
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
            hi: ShowdownSide {
                kind,
                usage: HoleUsage::AllOwn,
            },
            lo: None,
        }
    }

    /// The conventional win-rate unit for this game: fixed-limit games
    /// report in big bets ("BB/100"), pot-limit and no-limit in big blinds
    /// ("bb/100"). Returns the chip divisor and the human label.
    pub fn rate_unit(&self) -> (Chips, &'static str) {
        match self.betting {
            BettingKind::FixedLimit { .. } => (self.stakes.tiers().1, "BB/100"),
            BettingKind::PotLimit | BettingKind::NoLimit => (self.stakes.blinds().1, "bb/100"),
        }
    }

    /// Fixed-limit bet size for a tier under these stakes.
    pub fn tier_size(&self, tier: BetTier) -> Chips {
        let (small_bet, big_bet) = self.stakes.tiers();
        match tier {
            BetTier::Small => small_bet,
            BetTier::Big => big_bet,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLINDS: Stakes = Stakes::Blinds {
        small_blind: 50,
        big_blind: 100,
    };

    const STUD: Stakes = Stakes::Stud {
        ante: 10,
        bring_in: 30,
        small_bet: 100,
        big_bet: 200,
    };

    #[test]
    fn rate_unit_is_big_bets_for_fixed_limit_and_big_blinds_otherwise() {
        assert_eq!(GameSpec::holdem_nl(BLINDS).rate_unit(), (100, "bb/100"));
        assert_eq!(GameSpec::omaha_pl(BLINDS).rate_unit(), (100, "bb/100"));
        assert_eq!(GameSpec::holdem_fl(BLINDS).rate_unit(), (200, "BB/100"));
        assert_eq!(GameSpec::stud_fl(BLINDS).rate_unit(), (200, "BB/100"));
        assert_eq!(GameSpec::td27_fl(BLINDS).rate_unit(), (200, "BB/100"));
    }

    #[test]
    fn blinds_passes_through_for_blind_games() {
        assert_eq!(BLINDS.blinds(), (50, 100));
    }

    #[test]
    fn blinds_derives_from_stud_small_bet() {
        // small_bet / 2, small_bet
        assert_eq!(STUD.blinds(), (50, 100));
    }

    #[test]
    fn tiers_for_blinds_is_bb_and_2bb() {
        assert_eq!(BLINDS.tiers(), (100, 200));
    }

    #[test]
    fn tiers_for_stud_is_explicit_small_and_big_bet() {
        assert_eq!(STUD.tiers(), (100, 200));
    }

    #[test]
    fn to_stud_passes_through_stud_stakes_verbatim() {
        assert_eq!(STUD.to_stud(), STUD);
    }

    #[test]
    fn to_stud_derives_ante_bring_in_and_bet_tiers_from_blinds() {
        // ante = bb/5 (min 1), bring_in = bb/2 (min 1), small_bet = bb,
        // big_bet = 2*bb — the pre-refactor derivation.
        assert_eq!(
            BLINDS.to_stud(),
            Stakes::Stud {
                ante: 20,
                bring_in: 50,
                small_bet: 100,
                big_bet: 200,
            }
        );
    }

    #[test]
    fn to_stud_derivation_floors_ante_and_bring_in_at_one() {
        let tiny = Stakes::Blinds {
            small_blind: 1,
            big_blind: 2,
        };
        assert_eq!(
            tiny.to_stud(),
            Stakes::Stud {
                ante: 1,
                bring_in: 1,
                small_bet: 2,
                big_bet: 4,
            }
        );
    }

    #[test]
    fn stud_spec_built_from_blind_stakes_matches_the_pre_refactor_derivation() {
        let spec = GameSpec::stud_fl(BLINDS);
        assert_eq!(
            spec.stakes,
            Stakes::Stud {
                ante: 20,
                bring_in: 50,
                small_bet: 100,
                big_bet: 200,
            }
        );
        assert_eq!(
            spec.forced_bets,
            ForcedBets::BringIn {
                ante: 20,
                bring_in: 50,
                low: false,
            }
        );
        assert_eq!(spec.tier_size(BetTier::Small), 100);
        assert_eq!(spec.tier_size(BetTier::Big), 200);
    }

    #[test]
    fn stud_spec_built_from_explicit_stud_stakes_uses_them_verbatim() {
        let spec = GameSpec::stud_fl(STUD);
        assert_eq!(spec.stakes, STUD);
        assert_eq!(
            spec.forced_bets,
            ForcedBets::BringIn {
                ante: 10,
                bring_in: 30,
                low: false,
            }
        );
        assert_eq!(spec.tier_size(BetTier::Small), 100);
        assert_eq!(spec.tier_size(BetTier::Big), 200);
    }

    #[test]
    fn blind_spec_built_from_stud_stakes_derives_blinds() {
        let spec = GameSpec::holdem_nl(STUD);
        assert_eq!(
            spec.stakes,
            Stakes::Blinds {
                small_blind: 50,
                big_blind: 100,
            }
        );
    }

    #[test]
    fn split_pot_specs_pair_the_expected_evaluators() {
        let expected = [
            (
                GameSpec::badacey_fl(BLINDS),
                EvalKind::AceToFiveLow,
                EvalKind::Badugi,
            ),
            (
                GameSpec::badeucy_fl(BLINDS),
                EvalKind::DeuceToSevenLow,
                EvalKind::BadugiAceHigh,
            ),
            (
                GameSpec::archie_fl(BLINDS),
                EvalKind::SixesOrBetterHigh,
                EvalKind::EightOrBetterLow,
            ),
            (
                GameSpec::bigo_pl(BLINDS),
                EvalKind::High,
                EvalKind::EightOrBetterLow,
            ),
        ];
        for (spec, hi, lo) in expected {
            assert_eq!(spec.showdown.hi.kind, hi, "{}", spec.id);
            assert_eq!(spec.showdown.lo.map(|l| l.kind), Some(lo), "{}", spec.id);
        }
    }

    #[test]
    fn split_pot_draw_specs_are_five_card_triple_draws() {
        for spec in [
            GameSpec::badacey_fl(BLINDS),
            GameSpec::badeucy_fl(BLINDS),
            GameSpec::archie_fl(BLINDS),
        ] {
            assert_eq!(spec.showdown.hi.usage, HoleUsage::AllOwn, "{}", spec.id);
            assert_eq!(
                spec.showdown.lo.map(|l| l.usage),
                Some(HoleUsage::AllOwn),
                "{}",
                spec.id
            );
            assert_eq!(spec.seats, 2..=6, "{}", spec.id);
            assert_eq!(
                spec.streets[0].deal,
                DealSpec::HolePrivate(5),
                "{}",
                spec.id
            );
            assert_eq!(
                spec.streets
                    .iter()
                    .filter(|s| matches!(s.deal, DealSpec::Draw { max: 5 }))
                    .count(),
                3,
                "{} must have three draws",
                spec.id
            );
        }
    }

    #[test]
    fn bigo_deals_five_hole_cards_to_up_to_nine_seats() {
        let spec = GameSpec::bigo_pl(BLINDS);
        assert_eq!(spec.streets[0].deal, DealSpec::HolePrivate(5));
        assert_eq!(spec.showdown.hi.usage, HoleUsage::ExactlyTwo);
        assert_eq!(spec.betting, BettingKind::PotLimit);
        // 5 per seat plus a five-card board must fit in one deck.
        assert_eq!(*spec.seats.end() as usize * 5 + 5, 50);
    }

    #[test]
    fn every_new_id_round_trips_through_the_registry() {
        for id in ["badacey-fl", "badeucy-fl", "archie-fl", "bigo-pl"] {
            assert!(GameSpec::known_ids().contains(&id), "{id} not listed");
            let spec = GameSpec::by_id(id, BLINDS).unwrap_or_else(|| panic!("{id} unknown"));
            assert_eq!(spec.id, id);
        }
    }

    #[test]
    fn stakes_are_stored_normalized_by_family() {
        // A stud spec's stakes is always the Stud variant, no matter what
        // shape it was constructed with; a blind spec's is always Blinds.
        assert!(matches!(
            GameSpec::stud_fl(BLINDS).stakes,
            Stakes::Stud { .. }
        ));
        assert!(matches!(
            GameSpec::stud_fl(STUD).stakes,
            Stakes::Stud { .. }
        ));
        assert!(matches!(
            GameSpec::holdem_nl(BLINDS).stakes,
            Stakes::Blinds { .. }
        ));
        assert!(matches!(
            GameSpec::holdem_nl(STUD).stakes,
            Stakes::Blinds { .. }
        ));
    }
}

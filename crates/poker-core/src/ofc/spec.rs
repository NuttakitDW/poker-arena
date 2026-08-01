//! Data-driven OFC variant definitions.
//!
//! An OFC variant is *data*: how many cards open the hand, how many rounds of
//! deal-and-place follow, which evaluator contests the middle row, and which
//! fantasyland schedule applies. The state machine (`state.rs`) interprets
//! specs; every row capacity, foul rule and royalty table is shared by all
//! four variants and lives in `board.rs` / `score.rs`.
//!
//! Seat caps are card-exact and must stay that way: the deck is never
//! reshuffled, so `max_seats × cards_per_seat` may not exceed 52. Classic OFC
//! deals thirteen cards a seat (4 × 13 = 52); the pineapple family deals
//! seventeen (3 × 17 = 51). A fantasyland seat is dealt *fewer* cards than
//! the pineapple structure would give it (at most 17), so fantasyland never
//! widens that bound.

use core::ops::RangeInclusive;

/// Which evaluator the middle row is contested with.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub enum MiddleKind {
    /// Ordinary high hand; the board must run top ≤ middle ≤ bottom.
    High,
    /// 2-7 lowball with the ten-low qualifier. The middle has no ordering
    /// relationship with its neighbours at all: it fouls the hand by failing
    /// to qualify, never by being "too strong" or "too weak".
    DeuceToSeven,
}

/// Which fantasyland schedule a variant uses. Entry and stay conditions are
/// spelled out in `score.rs`; this only selects between them.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub enum FantasylandRule {
    /// Queens or better in the top row (pair or trips) grants a flat `cards`.
    /// `middle_stay` says whether a middle full house or better also repeats
    /// fantasyland: true for classic OFC, false for pineapple, whose stays
    /// are top trips / bottom quads+ only.
    Classic { cards: u8, middle_stay: bool },
    /// Top QQ → 14, KK → 15, AA → 16, any top trips → 17.
    Progressive,
    /// Top KK+ (pair or trips) or an exact 7-5-4-3-2 middle → 14; both → 15.
    DeuceMiddle,
}

/// A complete OFC variant definition.
/// (`Serialize` only: the `&'static str` ids cannot be deserialized.)
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct OfcSpec {
    /// Registry identifier, e.g. `"ofc-pineapple"`.
    pub id: &'static str,
    pub name: &'static str,
    pub min_seats: usize,
    pub max_seats: usize,
    /// Cards dealt to open the hand; all of them are placed.
    pub initial_deal: u8,
    /// Deal-and-place rounds after the opening placement.
    pub rounds: u8,
    /// Cards dealt to a seat at the start of each round.
    pub round_deal: u8,
    /// Cards placed each round; the rest of that round's deal is discarded.
    pub round_place: u8,
    pub middle: MiddleKind,
    pub fantasyland: FantasylandRule,
}

impl OfcSpec {
    /// Cards a seat places over the hand. Always 13 — the board's capacity —
    /// which is the invariant tying `initial_deal`, `rounds` and
    /// `round_place` together.
    pub const fn placements(&self) -> u8 {
        self.initial_deal + self.rounds * self.round_place
    }

    /// Cards a non-fantasyland seat is dealt over the hand (placements plus
    /// discards). Multiplied by `max_seats` this must not exceed 52.
    pub const fn cards_per_seat(&self) -> u8 {
        self.initial_deal + self.rounds * self.round_deal
    }

    /// The card count a fantasyland *stay* grants: 13 for classic OFC, 14 for
    /// every pineapple-structured variant.
    pub const fn fantasyland_base(&self) -> u8 {
        match self.fantasyland {
            FantasylandRule::Classic { cards, .. } => cards,
            FantasylandRule::Progressive | FantasylandRule::DeuceMiddle => 14,
        }
    }

    pub const fn seats(&self) -> RangeInclusive<usize> {
        self.min_seats..=self.max_seats
    }
}

/// Classic open face chinese: thirteen cards a seat, no discards.
pub const OFC: OfcSpec = OfcSpec {
    id: "ofc",
    name: "Open Face Chinese",
    min_seats: 2,
    max_seats: 4,
    initial_deal: 5,
    rounds: 8,
    round_deal: 1,
    round_place: 1,
    middle: MiddleKind::High,
    fantasyland: FantasylandRule::Classic {
        cards: 13,
        middle_stay: true,
    },
};

/// Pineapple OFC: three cards a round, place two and discard one.
pub const OFC_PINEAPPLE: OfcSpec = OfcSpec {
    id: "ofc-pineapple",
    name: "Pineapple OFC",
    min_seats: 2,
    max_seats: 3,
    initial_deal: 5,
    rounds: 4,
    round_deal: 3,
    round_place: 2,
    middle: MiddleKind::High,
    fantasyland: FantasylandRule::Classic {
        cards: 14,
        middle_stay: false,
    },
};

/// Pineapple OFC with the progressive fantasyland schedule.
pub const OFC_PROGRESSIVE: OfcSpec = OfcSpec {
    id: "ofc-progressive",
    name: "Progressive Pineapple OFC",
    min_seats: 2,
    max_seats: 3,
    initial_deal: 5,
    rounds: 4,
    round_deal: 3,
    round_place: 2,
    middle: MiddleKind::High,
    fantasyland: FantasylandRule::Progressive,
};

/// Pineapple OFC whose middle row is a 2-7 lowball hand.
pub const OFC_27: OfcSpec = OfcSpec {
    id: "ofc-27",
    name: "2-7 Pineapple OFC",
    min_seats: 2,
    max_seats: 3,
    initial_deal: 5,
    rounds: 4,
    round_deal: 3,
    round_place: 2,
    middle: MiddleKind::DeuceToSeven,
    fantasyland: FantasylandRule::DeuceMiddle,
};

const REGISTRY: [OfcSpec; 4] = [OFC, OFC_PINEAPPLE, OFC_PROGRESSIVE, OFC_27];

/// Every OFC variant, in listing order.
pub fn registry() -> &'static [OfcSpec] {
    &REGISTRY
}

/// Look up an OFC variant by registry id.
pub fn find(id: &str) -> Option<&'static OfcSpec> {
    REGISTRY.iter().find(|spec| spec.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_places_exactly_a_full_board() {
        for spec in registry() {
            assert_eq!(spec.placements(), 13, "{}", spec.id);
        }
    }

    #[test]
    fn seat_caps_fit_in_one_deck() {
        for spec in registry() {
            let needed = spec.max_seats * spec.cards_per_seat() as usize;
            assert!(needed <= 52, "{} needs {needed} cards", spec.id);
        }
        assert_eq!(OFC.max_seats * OFC.cards_per_seat() as usize, 52);
        assert_eq!(
            OFC_PINEAPPLE.max_seats * OFC_PINEAPPLE.cards_per_seat() as usize,
            51
        );
    }

    #[test]
    fn every_id_round_trips_through_the_registry() {
        for spec in registry() {
            assert_eq!(find(spec.id), Some(spec));
        }
        assert_eq!(find("holdem-nl"), None);
    }

    #[test]
    fn fantasyland_base_is_thirteen_only_for_classic_ofc() {
        assert_eq!(OFC.fantasyland_base(), 13);
        assert_eq!(OFC_PINEAPPLE.fantasyland_base(), 14);
        assert_eq!(OFC_PROGRESSIVE.fantasyland_base(), 14);
        assert_eq!(OFC_27.fantasyland_base(), 14);
    }
}

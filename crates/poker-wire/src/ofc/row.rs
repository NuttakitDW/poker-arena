//! The atoms of an OFC placement decision: which row a card goes to, and the
//! bundle of placements + discards a bot replies with.

use crate::card::Card;

/// A row of an OFC board. Capacities (top 3, middle 5, bottom 5) are engine
/// business, not wire vocabulary.
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Row {
    Top,
    Middle,
    Bottom,
}

/// One card assigned to one row, e.g. `{"card":"As","row":"bottom"}`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Placement {
    pub card: Card,
    pub row: Row,
}

/// A bot's reply to a placement decision: every dealt card is accounted for
/// as exactly one placement or exactly one discard (the arena validates
/// this; a bot that gets it wrong faults).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OfcAction {
    pub placements: Vec<Placement>,
    pub discards: Vec<Card>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{Rank, Suit};

    fn c(rank: Rank, suit: Suit) -> Card {
        Card::new(rank, suit)
    }

    #[test]
    fn row_round_trips_through_json() {
        for row in [Row::Top, Row::Middle, Row::Bottom] {
            let text = serde_json::to_string(&row).unwrap();
            let back: Row = serde_json::from_str(&text).unwrap();
            assert_eq!(back, row);
        }
    }

    #[test]
    fn row_has_the_expected_kebab_case_json() {
        assert_eq!(serde_json::to_string(&Row::Top).unwrap(), r#""top""#);
        assert_eq!(serde_json::to_string(&Row::Middle).unwrap(), r#""middle""#);
        assert_eq!(serde_json::to_string(&Row::Bottom).unwrap(), r#""bottom""#);
    }

    #[test]
    fn placement_has_the_expected_exact_json() {
        let placement = Placement {
            card: c(Rank::Ace, Suit::Spades),
            row: Row::Bottom,
        };
        assert_eq!(
            serde_json::to_string(&placement).unwrap(),
            r#"{"card":"As","row":"bottom"}"#
        );
        let back: Placement = serde_json::from_str(r#"{"card":"As","row":"bottom"}"#).unwrap();
        assert_eq!(back, placement);
    }

    #[test]
    fn ofc_action_round_trips_through_json() {
        let action = OfcAction {
            placements: vec![
                Placement {
                    card: c(Rank::Ace, Suit::Spades),
                    row: Row::Bottom,
                },
                Placement {
                    card: c(Rank::King, Suit::Diamonds),
                    row: Row::Top,
                },
            ],
            discards: vec![c(Rank::Two, Suit::Clubs)],
        };
        let text = serde_json::to_string(&action).unwrap();
        let back: OfcAction = serde_json::from_str(&text).unwrap();
        assert_eq!(back, action);
    }
}

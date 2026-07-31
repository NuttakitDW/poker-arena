//! Cards, ranks, and suits.
//!
//! `Card` is a compact wrapper over `0..52` (`rank * 4 + suit`). The canonical
//! text form is two characters, rank then suit: `"As"`, `"Td"`, `"2c"`, and
//! that is exactly how a card appears on the wire.
//!
//! (`Deck` is not here: shuffling needs an RNG, which is engine business —
//! it lives in `poker_core::deck`.)

use core::fmt;
use core::str::FromStr;

/// Card rank, ordered `Two < Three < … < Ace`.
///
/// Ordering is the *high-hand* convention; lowball orderings are the
/// evaluators' business, not `Rank`'s.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Rank {
    Two = 0,
    Three,
    Four,
    Five,
    Six,
    Seven,
    Eight,
    Nine,
    Ten,
    Jack,
    Queen,
    King,
    Ace,
}

impl Rank {
    pub const ALL: [Rank; 13] = [
        Rank::Two,
        Rank::Three,
        Rank::Four,
        Rank::Five,
        Rank::Six,
        Rank::Seven,
        Rank::Eight,
        Rank::Nine,
        Rank::Ten,
        Rank::Jack,
        Rank::Queen,
        Rank::King,
        Rank::Ace,
    ];

    const CHARS: [char; 13] = [
        '2', '3', '4', '5', '6', '7', '8', '9', 'T', 'J', 'Q', 'K', 'A',
    ];

    /// Index in `0..13` (`Two = 0`, `Ace = 12`).
    #[inline]
    pub const fn index(self) -> u8 {
        self as u8
    }

    pub const fn from_index(index: u8) -> Option<Rank> {
        if index < 13 {
            Some(Self::ALL[index as usize])
        } else {
            None
        }
    }

    pub const fn as_char(self) -> char {
        Self::CHARS[self as usize]
    }

    pub fn from_char(c: char) -> Option<Rank> {
        let c = c.to_ascii_uppercase();
        Self::CHARS
            .iter()
            .position(|&r| r == c)
            .map(|i| Self::ALL[i])
    }
}

/// Card suit. Suits are never ranked in poker; the ordering here exists only
/// to give `Card` a stable total order.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Suit {
    Clubs = 0,
    Diamonds,
    Hearts,
    Spades,
}

impl Suit {
    pub const ALL: [Suit; 4] = [Suit::Clubs, Suit::Diamonds, Suit::Hearts, Suit::Spades];

    const CHARS: [char; 4] = ['c', 'd', 'h', 's'];

    #[inline]
    pub const fn index(self) -> u8 {
        self as u8
    }

    pub const fn from_index(index: u8) -> Option<Suit> {
        if index < 4 {
            Some(Self::ALL[index as usize])
        } else {
            None
        }
    }

    pub const fn as_char(self) -> char {
        Self::CHARS[self as usize]
    }

    pub fn from_char(c: char) -> Option<Suit> {
        let c = c.to_ascii_lowercase();
        Self::CHARS
            .iter()
            .position(|&s| s == c)
            .map(|i| Self::ALL[i])
    }
}

/// A playing card, stored as `rank * 4 + suit` in `0..52`.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Card(u8);

impl Card {
    pub const fn new(rank: Rank, suit: Suit) -> Card {
        Card(rank as u8 * 4 + suit as u8)
    }

    /// Index in `0..52`.
    #[inline]
    pub const fn index(self) -> u8 {
        self.0
    }

    pub const fn from_index(index: u8) -> Option<Card> {
        if index < 52 { Some(Card(index)) } else { None }
    }

    #[inline]
    pub const fn rank(self) -> Rank {
        Rank::ALL[(self.0 / 4) as usize]
    }

    #[inline]
    pub const fn suit(self) -> Suit {
        Suit::ALL[(self.0 % 4) as usize]
    }
}

impl fmt::Display for Card {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.rank().as_char(), self.suit().as_char())
    }
}

impl fmt::Debug for Card {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// Error parsing a card from text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid card {0:?}: expected rank+suit like \"As\" or \"td\"")]
pub struct ParseCardError(pub String);

impl FromStr for Card {
    type Err = ParseCardError;

    fn from_str(s: &str) -> Result<Card, ParseCardError> {
        let mut chars = s.chars();
        let (Some(r), Some(u), None) = (chars.next(), chars.next(), chars.next()) else {
            return Err(ParseCardError(s.to_string()));
        };
        match (Rank::from_char(r), Suit::from_char(u)) {
            (Some(rank), Some(suit)) => Ok(Card::new(rank, suit)),
            _ => Err(ParseCardError(s.to_string())),
        }
    }
}

impl serde::Serialize for Card {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for Card {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Card, D::Error> {
        let s = <&str as serde::Deserialize>::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Parse a whitespace- or comma-separated card list like `"As Kd"` / `"As,Kd"`.
pub fn parse_cards(s: &str) -> Result<Vec<Card>, ParseCardError> {
    s.split(|c: char| c.is_whitespace() || c == ',')
        .filter(|t| !t.is_empty())
        .map(str::parse)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_roundtrip_all_52() {
        for i in 0..52u8 {
            let card = Card::from_index(i).unwrap();
            let text = card.to_string();
            assert_eq!(text.parse::<Card>().unwrap(), card);
            assert_eq!(card.index(), i);
            assert_eq!(Card::new(card.rank(), card.suit()), card);
        }
    }

    #[test]
    fn parse_is_case_insensitive_and_strict() {
        assert_eq!(
            "as".parse::<Card>().unwrap(),
            Card::new(Rank::Ace, Suit::Spades)
        );
        assert_eq!(
            "TD".parse::<Card>().unwrap(),
            Card::new(Rank::Ten, Suit::Diamonds)
        );
        assert!("A".parse::<Card>().is_err());
        assert!("Asx".parse::<Card>().is_err());
        assert!("1s".parse::<Card>().is_err());
        assert!("".parse::<Card>().is_err());
    }
}

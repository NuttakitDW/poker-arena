//! Player actions and the structured description of what is currently legal.

use crate::card::Card;

/// Chip amounts. Wagers and stacks are unsigned; per-hand *net* results are
/// `i64` (see `Settlement`).
pub type Chips = u64;

/// Seat index at the table, `0..seat_count`. Seat 0 is arbitrary; the button
/// position is carried per hand.
pub type Seat = usize;

/// An action a player takes when it is their turn.
///
/// **`to` semantics**: `Bet` and `Raise` carry the player's *total* street
/// commitment after the action ("raise to"), not the increment. This removes
/// all ambiguity about blinds and partial calls.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Action {
    Fold,
    Check,
    /// Match the current wager (amount is implied; engine computes it).
    Call,
    /// Open the betting this street to a total of `to`.
    Bet {
        to: Chips,
    },
    /// Raise the current wager to a total street commitment of `to`.
    Raise {
        to: Chips,
    },
    /// Stud: post the forced bring-in.
    BringIn,
    /// Draw streets: discard these cards and draw replacements.
    /// Discarding zero cards is "standing pat".
    Discard {
        cards: Vec<Card>,
    },
}

/// What the seat to act may legally do right now.
///
/// Exactly one "family" applies per decision point: on betting streets the
/// fold/check/call/bet/raise fields; on draw streets only `draw`; at a stud
/// bring-in decision only `bring_in` (+ the completion options it allows).
#[derive(Clone, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct LegalActions {
    /// Folding is offered only when facing chips to call (open-folding when a
    /// check is free is disallowed — it is never correct and almost always a
    /// bot bug worth surfacing).
    pub fold: bool,
    pub check: bool,
    /// Additional chips required to call. May be less than the nominal price
    /// when it puts the caller all-in (`call == remaining stack`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call: Option<Chips>,
    /// Available when nothing has been wagered this street.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bet: Option<BetBounds>,
    /// Available when facing a wager (includes preflop raises over the blind).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raise: Option<BetBounds>,
    /// Stud bring-in amount.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bring_in: Option<Chips>,
    /// Draw-street bounds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draw: Option<DrawBounds>,
}

/// Inclusive bounds on `Bet`/`Raise` **`to`-totals** for the street.
///
/// Fixed-limit: `min_to == max_to`. No-limit/pot-limit: `max_to` is the
/// actor's all-in total; `min_to` is the smallest legal full raise, except
/// when the all-in total is below the full-raise minimum, in which case
/// `min_to == max_to` (the short all-in is the only wager available).
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BetBounds {
    pub min_to: Chips,
    pub max_to: Chips,
}

/// Bounds for a draw decision.
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DrawBounds {
    pub max_discards: u8,
}

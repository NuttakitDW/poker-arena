//! The per-hand state machine.
//!
//! `HandState` interprets a [`GameSpec`] to run exactly one hand: forced
//! bets, dealing, betting rounds, showdown, settlement. It is pure and
//! synchronous — no I/O, no clocks, no bots. The arena layer owns all of
//! those and simply calls `to_act` / `legal_actions` / `apply` in a loop.
//!
//! # Rules contract (the implementation spec)
//!
//! ## Setup (`new`)
//! - Validates seat count against the spec, stacks all positive, and (M1)
//!   rejects specs using M3 features (`BringIn`, `HoleUp`, `Draw`,
//!   `ByUpcards`) with `HandError::Unsupported`.
//! - Emits `HandStart`, posts forced bets (heads-up: the **button posts the
//!   small blind** and acts first preflop), deals street 0, emits
//!   `StreetStart`/`DealHole` events, and opens street 0's betting round.
//! - A blind that covers a stack posts all-in for less (`Post.all_in`).
//!
//! ## Betting rounds
//! - State per round: each seat's street commitment; `current_to` (highest
//!   commitment, = big blind preflop before any raise); the size of the last
//!   full raise; who still must act. A round ends when every non-folded,
//!   non-all-in seat has either matched `current_to` or checked, *and* has
//!   acted since the last full wager — with the classic exception that the
//!   big blind gets its option preflop (may raise an unraised pot even
//!   though its commitment already matches).
//! - Action order: [`FirstToAct::AfterBlinds`] (street 0 of blind games) —
//!   first seat after the big blind, heads-up the button; otherwise
//!   [`FirstToAct::LeftOfButton`] — first non-folded seat left of button.
//!   Folded and all-in seats are skipped.
//! - **Fold** is legal only when facing chips to call. **Check** only when
//!   not. **Call** always available facing a wager (all-in for less when
//!   short). `Bet {to}` opens a street with no wager; `Raise {to}` increases
//!   an existing one; `to` is the seat's *total* street commitment and must
//!   lie in the offered [`BetBounds`].
//! - **No-limit**: opening bet minimum = big blind; minimum raise increment
//!   = size of the last full bet/raise this street (initially the big
//!   blind); maximum = actor's all-in total. A short all-in below the full
//!   minimum is legal (bounds collapse to the all-in) but does **not**
//!   reopen the action: seats that already acted at the prior price may only
//!   call or fold when action returns to them, and the min-raise base for
//!   later full raises is unchanged by the short wager.
//! - **Pot-limit**: same as no-limit except the maximum. Implement exactly
//!   as `max_to = to_call_total + pot_after_call`, where `to_call_total` is
//!   the actor's street commitment after a hypothetical call and
//!   `pot_after_call = pot_total_before_action + to_call_amount` (the
//!   classic "call, then raise the size of the pot"). Clamp to all-in.
//! - **Fixed-limit**: wager sizes fixed at `spec.tier_size(street.tier)`;
//!   `min_to == max_to == current_to + tier` (or `tier` for the opening
//!   bet). The round's wager count is capped per
//!   `BettingKind::FixedLimit { raise_cap }`: the opening bet counts as
//!   wager 1, and preflop the big blind itself counts as wager 1. At the
//!   cap, only call/fold are offered. A short all-in "raise" is allowed
//!   below tier size when it is the actor's whole stack; it counts toward
//!   the cap only if ≥ half the tier (half-bet rule) — otherwise treated as
//!   a call-and-more that does not reopen action.
//!
//! ## Street advancement & hand end
//! - `apply` auto-advances: when a betting round completes, deal the next
//!   street (events in order: `StreetStart`, deal event) and open its
//!   betting round. If all but one seat folds, the hand ends immediately:
//!   refund the uncalled excess of the last wager to its owner (no event;
//!   reflected in nets), award the pot without showdown (`PotAwarded` with
//!   `PotSide::Whole`, no `ShowdownShow`), emit `HandEnd`.
//! - When at most one non-all-in seat remains with a live wager matched
//!   (betting cannot continue), remaining streets are dealt out with no
//!   betting rounds ("run-out") straight to showdown.
//! - Uncalled excess: whenever a street's highest commitment exceeds the
//!   second-highest among non-folded seats at the moment betting on that
//!   street ends (all-in situations), the difference is returned to the
//!   over-committed seat before pot construction.
//!
//! ## Showdown & settlement
//! - Every non-folded seat reveals (`ShowdownShow`, engine-computed values
//!   via `eval::best_with_usage` per `spec.showdown`). Order of reveal:
//!   odd-chip order (left of button first) — arena bots learn everything
//!   either way; there is no strategic mucking.
//! - Pots built by `pot::build_pots` from full-hand contributions, awarded
//!   by `pot::award_pots` (odd-chip rules documented there). One
//!   `PotAwarded` event per pot side.
//! - `HandEnd { nets }` closes the hand: `nets[s] = won − contributed`,
//!   `sum(nets) == 0` (chip conservation — property-tested).
//!
//! ## Invariants (must be property-tested)
//! - Chip conservation at `HandEnd`.
//! - `apply(a)` succeeds iff `a` conforms to the last `legal_actions()`.
//! - Every hand terminates (fold-out, or showdown after the last street).
//! - Stacks never go negative; commitments never exceed starting stacks.

use super::action::{Action, Chips, LegalActions, Seat};
use super::event::Event;
use super::pot::PotAward;
use super::spec::GameSpec;
use crate::card::{Card, Deck};

/// Errors constructing a hand.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HandError {
    #[error("seat count {0} outside the spec's supported range")]
    BadSeatCount(usize),
    #[error("all stacks must be positive")]
    BadStacks,
    #[error("deck exhausted while dealing")]
    DeckExhausted,
    #[error("spec feature not yet supported: {0}")]
    Unsupported(&'static str),
}

/// Errors applying an action.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ActionError {
    #[error("hand is over; no actions accepted")]
    HandOver,
    #[error("action {action:?} is not legal now: {reason}")]
    Illegal {
        action: Action,
        reason: &'static str,
    },
}

/// Final accounting for a completed hand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settlement {
    /// `nets[seat] = chips won − chips contributed`; sums to zero.
    pub nets: Vec<i64>,
    pub awards: Vec<PotAward>,
    /// Seats that reached showdown (empty on a fold-out).
    pub showdown_seats: Vec<Seat>,
}

/// State of one hand in progress. See module docs for the full rules
/// contract. Construction deals street 0; drive it with `to_act` /
/// `legal_actions` / `apply` until `is_over`.
pub struct HandState {
    // Delegated implementation chooses internal representation, subject to
    // the public API below and the module-level contract.
    _todo: core::marker::PhantomData<()>,
}

impl HandState {
    /// Start a hand. `button` indexes into `stacks` (len = seat count).
    /// Deals from `deck`; the deck must hold enough cards for a full
    /// run-out. Returns the state plus all events emitted so far
    /// (hand start, posts, street 0 deal).
    pub fn new(
        spec: &GameSpec,
        stacks: &[Chips],
        button: Seat,
        hand_no: u64,
        deck: Deck,
    ) -> Result<(HandState, Vec<Event>), HandError> {
        todo!("delegated implementation")
    }

    /// The seat that must act, or `None` when the hand is over.
    pub fn to_act(&self) -> Option<Seat> {
        todo!("delegated implementation")
    }

    /// Legal actions for the seat to act; `None` when the hand is over.
    pub fn legal_actions(&self) -> Option<LegalActions> {
        todo!("delegated implementation")
    }

    /// Apply an action for the seat returned by `to_act`, returning every
    /// event that resulted (the action itself, street advances, deals,
    /// showdown, settlement…). Illegal actions leave state untouched.
    pub fn apply(&mut self, action: Action) -> Result<Vec<Event>, ActionError> {
        todo!("delegated implementation")
    }

    pub fn is_over(&self) -> bool {
        todo!("delegated implementation")
    }

    /// Settlement, once `is_over`.
    pub fn settlement(&self) -> Option<&Settlement> {
        todo!("delegated implementation")
    }

    // --- Read-only views (for arena/bot consumption and logging) ---

    /// Community cards dealt so far.
    pub fn board(&self) -> &[Card] {
        todo!("delegated implementation")
    }

    /// Hole cards of a seat (unredacted — callers redact via events).
    pub fn hole_cards(&self, seat: Seat) -> &[Card] {
        todo!("delegated implementation")
    }

    /// Current street index and label.
    pub fn street(&self) -> (u8, &'static str) {
        todo!("delegated implementation")
    }

    /// Remaining stack per seat (starting stack − contributions so far).
    pub fn stacks(&self) -> &[Chips] {
        todo!("delegated implementation")
    }

    /// Each seat's commitment on the current street.
    pub fn street_commits(&self) -> &[Chips] {
        todo!("delegated implementation")
    }

    /// Total chips in the pot (all streets, including current commitments).
    pub fn pot_total(&self) -> Chips {
        todo!("delegated implementation")
    }

    pub fn folded(&self) -> &[bool] {
        todo!("delegated implementation")
    }

    pub fn all_in(&self) -> &[bool] {
        todo!("delegated implementation")
    }

    /// Full unredacted event history since hand start.
    pub fn events(&self) -> &[Event] {
        todo!("delegated implementation")
    }
}

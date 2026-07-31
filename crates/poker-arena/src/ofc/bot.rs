//! The OFC bot interface.
//!
//! [`OfcBot`] is the single abstraction every OFC competitor implements.
//! In-process bots implement it directly; remote bots are wrapped by
//! [`crate::ofc::remote::OfcWireBot`], which implements the same trait, so
//! the runner never distinguishes.
//!
//! Bots are driven per hand: `hand_start`, a stream of seat-redacted
//! `event`s, a `place` call whenever it is their turn, and `hand_end`.
//! Between hands, seats rotate — `seat` in [`OfcHandStart`] tells the bot
//! where it sits *this hand*, and `fantasyland` whether it is playing this
//! one face-down.

use poker_core::card::Card;
use poker_core::ofc::{Board, OfcAction, OfcEvent};

use crate::bot::BotFault;

/// Per-hand context given to each bot before any events.
#[derive(Clone, Debug)]
pub struct OfcHandStart {
    pub hand_no: u64,
    /// This bot's seat for this hand.
    pub seat: usize,
    /// Cards this bot is dealt at once, board hidden until showdown, when it
    /// plays this hand in fantasyland; `None` for an ordinary hand.
    pub fantasyland: Option<u8>,
}

/// Everything a bot may know at a placement decision. Self-contained so bots
/// can be stateless; stateful bots may also fold in the event stream they've
/// observed.
#[derive(Clone, Debug)]
pub struct OfcActionRequest<'a> {
    pub hand_no: u64,
    /// The acting bot's seat.
    pub seat: usize,
    /// This turn's cards, in deal order. Exactly `place + discard` of them.
    pub dealt: &'a [Card],
    pub place: u8,
    pub discard: u8,
    /// Boards as visible to this seat: its own board as it really stands,
    /// every other seat's as the table sees it — which for a seat playing in
    /// fantasyland is an empty board until showdown.
    pub boards: &'a [Board],
    /// Per-seat fantasyland card counts *this* hand.
    pub fantasyland: &'a [Option<u8>],
}

/// End-of-hand summary delivered to each bot.
#[derive(Clone, Debug)]
pub struct OfcHandEnd {
    pub hand_no: u64,
    /// Net points by seat (this bot's is `points[seat]` from its
    /// [`OfcHandStart`]). Sums to zero.
    pub points: Vec<i64>,
}

/// An OFC bot. Implementations must be `Send` (matches may run off-thread).
///
/// `place` must return an action conforming to `req`: exactly `req.place`
/// distinct cards from `req.dealt` assigned to rows with free capacity, and
/// exactly `req.discard` of the rest. A non-conforming action or an `Err` is
/// a fault handled by the arena's fault policy (it is never silently
/// patched). In-process bots normally always return `Ok`; `Err` exists for
/// transport-backed bots (timeouts, disconnects).
pub trait OfcBot: Send {
    /// Stable display name (used in reports; uniqueness enforced by the CLI).
    fn name(&self) -> &str;

    fn hand_start(&mut self, _info: &OfcHandStart) {}

    /// An observable event, already redacted for this bot's seat.
    fn event(&mut self, _event: &OfcEvent) {}

    fn place(&mut self, req: &OfcActionRequest<'_>) -> Result<OfcAction, BotFault>;

    fn hand_end(&mut self, _result: &OfcHandEnd) {}
}

//! The bot interface.
//!
//! `Bot` is the single abstraction every competitor implements. In-process
//! bots implement it directly; remote bots are wrapped by a wire adapter
//! that implements the same trait (M2), so the runner never distinguishes.
//!
//! Bots are driven per hand: `hand_start`, a stream of seat-redacted
//! `event`s, an `act` call whenever it is their turn, and `hand_end`.
//! Between hands, seats may be permuted for variance reduction — `seat` in
//! [`HandStart`] tells the bot where it sits *this hand*.

use poker_core::card::Card;
use poker_core::game::{Action, Chips, Event, LegalActions, Seat};

/// Per-hand context given to each bot before any events.
#[derive(Clone, Debug)]
pub struct HandStart {
    pub hand_no: u64,
    /// This bot's seat for this hand.
    pub seat: Seat,
    pub button: Seat,
    pub seat_count: usize,
    /// Starting stacks by seat.
    pub stacks: Vec<Chips>,
}

/// Everything a bot may know at a decision point. Self-contained so bots can
/// be stateless; stateful bots may also fold in the event stream they've
/// observed.
#[derive(Clone, Debug)]
pub struct ActionRequest<'a> {
    pub hand_no: u64,
    /// The acting bot's seat.
    pub seat: Seat,
    pub button: Seat,
    pub street: u8,
    pub street_label: &'static str,
    pub hole: &'a [Card],
    pub board: &'a [Card],
    /// Remaining stacks by seat.
    pub stacks: &'a [Chips],
    /// Current-street commitments by seat.
    pub street_commits: &'a [Chips],
    pub pot_total: Chips,
    pub folded: &'a [bool],
    pub legal: &'a LegalActions,
}

/// End-of-hand summary delivered to each bot.
#[derive(Clone, Debug)]
pub struct HandEnd {
    pub hand_no: u64,
    /// Net result by seat (this bot's is `nets[seat]` from its `HandStart`).
    pub nets: Vec<i64>,
}

/// A failure to produce an action at all — the transport-level counterpart
/// of returning an illegal action. Both are "faults" to the arena and are
/// handled by the configured [`crate::config::FaultPolicy`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BotFault {
    /// No answer within the configured deadline.
    Timeout,
    /// The bot's transport is gone (socket closed, process exited).
    Disconnected,
    /// The bot answered with something that isn't a well-formed action.
    Protocol(String),
}

/// A poker bot. Implementations must be `Send` (matches may run off-thread).
///
/// `act` must return an action conforming to `req.legal`; a non-conforming
/// action or an `Err` is a fault handled by the arena's fault policy (it is
/// never silently patched). In-process bots normally always return `Ok`;
/// `Err` exists for transport-backed bots (timeouts, disconnects).
pub trait Bot: Send {
    /// Stable display name (used in reports; uniqueness enforced by the CLI).
    fn name(&self) -> &str;

    fn hand_start(&mut self, _info: &HandStart) {}

    /// An observable event, already redacted for this bot's seat.
    fn event(&mut self, _event: &Event) {}

    fn act(&mut self, req: &ActionRequest<'_>) -> Result<Action, BotFault>;

    fn hand_end(&mut self, _result: &HandEnd) {}
}

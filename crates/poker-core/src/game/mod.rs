//! The rules engine: variant specs, actions, events, pots, and the per-hand
//! state machine.
//!
//! Actions, events, and the per-match stakes/betting parameters are defined
//! in `poker-wire` — they are the vocabulary the engine shares with bots —
//! and are re-exported here (including as [`action`] and [`event`] modules)
//! so engine callers keep using `poker_core::game::…` paths throughout.

pub mod pot;
pub mod spec;
pub mod state;

/// Player actions and legality, defined in [`poker_wire::action`].
pub mod action {
    pub use poker_wire::action::*;
}

/// The observable event stream, defined in [`poker_wire::event`].
pub mod event {
    pub use poker_wire::event::*;
}

pub use poker_wire::action::{Action, BetBounds, Chips, DrawBounds, LegalActions, Seat};
pub use poker_wire::event::{Event, PostKind, PotSide};
pub use poker_wire::game::{BettingKind, Stakes};

pub use pot::{Pot, PotAward, ShowdownEntry};
pub use spec::{
    BetRoundSpec, BetTier, DealSpec, FirstToAct, ForcedBets, GameSpec, ShowdownSide, ShowdownSpec,
    StreetSpec,
};
pub use state::{ActionError, HandError, HandState, Settlement};

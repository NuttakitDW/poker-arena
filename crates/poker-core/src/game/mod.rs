//! The rules engine: variant specs, actions, events, pots, and the per-hand
//! state machine.

pub mod action;
pub mod event;
pub mod pot;
pub mod spec;
pub mod state;

pub use action::{Action, BetBounds, Chips, DrawBounds, LegalActions, Seat};
pub use event::{Event, PostKind, PotSide};
pub use pot::{Pot, PotAward, ShowdownEntry};
pub use spec::{
    BetRoundSpec, BetTier, BettingKind, DealSpec, FirstToAct, ForcedBets, GameSpec, PotSplit,
    ShowdownSpec, Stakes, StreetSpec,
};
pub use state::{ActionError, HandError, HandState, Settlement};

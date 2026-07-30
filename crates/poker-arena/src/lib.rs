//! # poker-arena
//!
//! The competition layer: bot interface, match orchestration, variance
//! reduction, fault policies, statistics, and hand-history logging.

pub mod behavior;
pub mod bot;
pub mod builtin;
pub mod config;
pub mod log;
pub mod remote;
pub mod runner;
pub mod stat;

pub use behavior::BehaviorStats;
pub use bot::{ActionRequest, Bot, BotFault, HandEnd, HandStart};
pub use config::{DealingMode, FaultPolicy, MatchConfig};
pub use log::{EventSink, JsonLog};
pub use remote::{WireBot, WireBotError};
pub use runner::{BotOutcome, MatchError, MatchResult, Progress, run_match};

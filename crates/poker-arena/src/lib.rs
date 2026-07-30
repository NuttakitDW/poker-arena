//! # poker-arena
//!
//! The competition layer: bot interface, match orchestration, variance
//! reduction, fault policies, statistics, and hand-history logging.

pub mod bot;
pub mod builtin;
pub mod config;
pub mod stat;

pub use bot::{ActionRequest, Bot, HandEnd, HandStart};
pub use config::{DealingMode, FaultPolicy, MatchConfig};

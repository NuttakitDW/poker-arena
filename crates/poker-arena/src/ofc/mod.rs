//! The OFC competition layer.
//!
//! Everything [`crate`] provides for the betting games, provided again for
//! Open Face Chinese: a bot interface, baseline bots, a wire adapter, a match
//! runner, hand-history logging, and report builders. OFC hands have no
//! chips, no betting and no legal-actions surface — they place cards into
//! rows and settle in points — so the two paths share their mechanics
//! ([`crate::stat::RateStats`], [`crate::transport::LineTransport`],
//! [`crate::bot::BotFault`], [`crate::remote::WireBotError`]) but not their
//! vocabulary.
//!
//! - [`bot`]: [`OfcBot`], the trait every OFC competitor implements.
//! - [`builtin`]: the baseline bots — [`OfcFiller`], [`OfcRandom`],
//!   [`OfcGreedy`].
//! - [`remote`]: [`OfcWireBot`], the arena side of the OFC wire protocol.
//! - [`runner`]: [`run_ofc_match`], the match orchestration.
//! - [`log`]: [`OfcEventSink`] and its two implementations.
//! - [`report`]: builders for the [`poker_wire::ofc::report`] documents.

pub mod bot;
pub mod builtin;
pub mod log;
pub mod remote;
pub mod report;
pub mod runner;

pub use bot::{OfcActionRequest, OfcBot, OfcHandEnd, OfcHandStart};
pub use builtin::{OfcFiller, OfcGreedy, OfcRandom};
pub use log::{OfcEventSink, OfcHandMeta, OfcJsonLog, OfcLogSelection, OfcSelectiveLog};
pub use remote::OfcWireBot;
pub use report::{ofc_match_report, ofc_progress_report};
pub use runner::{
    OfcBotOutcome, OfcMatchConfig, OfcMatchError, OfcMatchResult, OfcProgress, OfcStanding,
    run_ofc_match,
};

//! poker-bot: one wire bot for every poker-arena game.
//!
//! Three layers, mirroring how a solver-backed bot is built:
//!
//! - **Lossless abstraction** ([`iso`]): suit-isomorphism canonicalization
//!   and information-set keys.
//! - **Lossy abstraction** ([`abstraction`]): per-street equity buckets, a
//!   discrete action menu, and the per-game abstract tree size, budgeted at
//!   ≤ 10^12 nodes so every variant stays solvable.
//! - **Play** ([`table`], [`equity`], [`policy`], [`betting`], [`ofc`]):
//!   the v1 strategy — Monte-Carlo pot-share equity against the arena's
//!   legal-action menu for the twenty betting games, and the arena's
//!   foul-avoiding greedy for the four OFC games. A trained strategy over
//!   the abstraction layers plugs in behind the same [`policy`] surface
//!   later without touching the transport.

pub mod abstraction;
pub mod betting;
pub mod blueprint;
pub mod cfr;
pub mod equity;
pub mod iso;
pub mod ofc;
pub mod policy;
pub mod sim;
pub mod table;

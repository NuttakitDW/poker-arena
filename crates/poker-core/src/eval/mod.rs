//! Hand evaluation.
//!
//! Every evaluator returns a [`HandValue`] where **greater is better for the
//! pot being contested** — low evaluators invert internally so that callers
//! compare uniformly with `>`. Values are only comparable when produced by
//! the same [`EvalKind`]; comparing across kinds is a logic error (not
//! detectable at runtime by design — `HandValue` stays a thin `u32`).
//!
//! ## Encodings (frozen; tests may snapshot exact values)
//!
//! *High* (`EvalKind::High`): bits `[20..24)` hold [`HandClass`] (0–8), bits
//! `[0..20)` hold five 4-bit tiebreak ranks, most significant first, using
//! [`Rank::index`] (`Two = 0 … Ace = 12`). Tiebreaks per class:
//! - HighCard/Flush: the five ranks descending.
//! - OnePair: pair rank, then three kickers descending.
//! - TwoPair: high pair, low pair, kicker (remaining slots 0).
//! - Trips: trip rank, two kickers. Quads: quad rank, kicker.
//! - FullHouse: trip rank, pair rank.
//! - Straight/StraightFlush: the straight's high card only (wheel A-5 has
//!   high card `Five`).
//!
//! *A-5 low* (`AceToFiveLow`): aces low, straights/flushes ignored. Compute
//! "badness" as five 4-bit low-ranks (`Ace = 0, Two = 1 … King = 12`) of the
//! best (lowest) five cards, highest low-rank most significant; pairs etc.
//! add a class penalty exactly like the high encoding (pairing always makes
//! a low worse). Return `HandValue(0x00FF_FFFF - badness)`.
//!
//! *2-7 low* (`DeuceToSevenLow`): hands ordered exactly opposite the high
//! ordering (straights and flushes count against you, aces always high).
//! Return `HandValue(0x00FF_FFFF - high_encoding)`.
//!
//! *8-or-better* (`EightOrBetterLow`): A-5 low, but qualifies only with five
//! distinct ranks all at low-rank ≤ 7 (i.e. Eight or lower, aces low).
//! Non-qualifying hands return `None`.
//!
//! *Badugi*: from ≤5 cards, the best "badugi subset" has distinct ranks and
//! distinct suits; more cards beat fewer; ties break low (aces low).
//! Encoding: `(subset_len << 20) | (0xF_FFFF - packed low-ranks descending)`.
//! A badugi subset never exceeds four cards (there are only four suits), so
//! the fifth card of a badacey hand only ever widens the search.
//!
//! *Badugi, aces high* (`BadugiAceHigh`, badeucy): the same subset rule and
//! the same encoding shape, ranked with [`Rank::index`] (`Two = 0 …
//! Ace = 12`) instead of the ace-low mapping. The nut badugi is therefore
//! 5-4-3-2 rainbow, and any badugi holding an ace loses to every ace-free
//! badugi of the same length.
//!
//! *Sixes-or-better high* (`SixesOrBetterHigh`, archie): the ordinary high
//! encoding, returned only when the best five-card hand qualifies —
//! `high_class() > OnePair`, or `OnePair` with the pair rank at Six or
//! above. No-pair hands never qualify, however high; non-qualifying hands
//! return `None`. Qualification is monotone in the encoding, so the best
//! hand qualifies exactly when some subset does.
//!
//! ## N-card inputs
//!
//! `high`, `ace_to_five_low`, `deuce_to_seven_low`, `eight_or_better`, and
//! `sixes_or_better` accept 5–7 cards and evaluate the best 5-card subset
//! (C(7,5)=21 brute force — plenty fast for arena use). `badugi` and
//! `badugi_ace_high` accept 1–5 cards (4 for badugi proper, 5 for the
//! badacey/badeucy split games).
//!
//! [`Rank::index`]: crate::card::Rank::index

mod badugi;
mod high;
mod low;

use crate::card::Card;

/// Hand strength as it appears at showdown; defined in `poker-wire` (bots
/// read these values off the event stream), while every *encoding* below and
/// the evaluators that produce them are this module's business.
pub use poker_wire::value::{HandClass, HandValue};

/// Which evaluator to run; variants reference these in their showdown specs.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub enum EvalKind {
    High,
    AceToFiveLow,
    DeuceToSevenLow,
    /// A-5 low with the eight-or-better qualifier; may not qualify.
    EightOrBetterLow,
    Badugi,
    /// Badugi with aces HIGH (badeucy): the nut badugi is 5-4-3-2 rainbow,
    /// and an ace is the worst card. Same encoding shape as `Badugi` with
    /// the ace-high rank order.
    BadugiAceHigh,
    /// High hands with the "sixes or better" qualifier (archie): qualifies
    /// iff the best high hand is at least a pair of sixes (class above
    /// OnePair, or OnePair with pair rank ≥ Six); may not qualify.
    SixesOrBetterHigh,
}

/// Best high hand from 5–7 cards.
pub fn high(cards: &[Card]) -> HandValue {
    high::evaluate(cards)
}

/// Best A-5 lowball hand (aces low, straights/flushes ignored) from 5–7 cards.
pub fn ace_to_five_low(cards: &[Card]) -> HandValue {
    low::ace_to_five(cards)
}

/// Best 2-7 lowball hand (aces high, straights/flushes count) from 5–7 cards.
pub fn deuce_to_seven_low(cards: &[Card]) -> HandValue {
    low::deuce_to_seven(cards)
}

/// Best qualifying eight-or-better low from 5–7 cards, if any.
pub fn eight_or_better(cards: &[Card]) -> Option<HandValue> {
    low::eight_or_better(cards)
}

/// Best badugi from up to 5 cards (badacey evaluates the best 4-of-5).
pub fn badugi(cards: &[Card]) -> HandValue {
    badugi::evaluate(cards)
}

/// Best ace-high badugi (badeucy ranking) from up to 5 cards.
pub fn badugi_ace_high(cards: &[Card]) -> HandValue {
    badugi::evaluate_ace_high(cards)
}

/// Best qualifying sixes-or-better high hand from 5–7 cards, if any.
pub fn sixes_or_better(cards: &[Card]) -> Option<HandValue> {
    high::sixes_or_better(cards)
}

/// Dispatch by kind. `None` means "does not qualify" and can only occur for
/// kinds with qualifiers (`EightOrBetterLow`, `SixesOrBetterHigh`).
pub fn evaluate(kind: EvalKind, cards: &[Card]) -> Option<HandValue> {
    match kind {
        EvalKind::High => Some(high(cards)),
        EvalKind::AceToFiveLow => Some(ace_to_five_low(cards)),
        EvalKind::DeuceToSevenLow => Some(deuce_to_seven_low(cards)),
        EvalKind::EightOrBetterLow => eight_or_better(cards),
        EvalKind::Badugi => Some(badugi(cards)),
        EvalKind::BadugiAceHigh => Some(badugi_ace_high(cards)),
        EvalKind::SixesOrBetterHigh => sixes_or_better(cards),
    }
}

/// How hole cards may combine with the board at showdown.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub enum HoleUsage {
    /// Best hand from any mix of hole and board cards (hold'em, stud).
    Any,
    /// Exactly two hole cards and exactly three board cards (Omaha).
    ExactlyTwo,
    /// The hand is the player's own cards; the board is unused (draw games).
    AllOwn,
}

/// Best value achievable from `hole` + `board` under a usage constraint.
///
/// For `ExactlyTwo`, enumerates C(hole,2) × C(board,3) candidate hands and
/// takes the max; for a qualifier kind the result is `None` only when *no*
/// candidate qualifies.
pub fn best_with_usage(
    kind: EvalKind,
    usage: HoleUsage,
    hole: &[Card],
    board: &[Card],
) -> Option<HandValue> {
    match usage {
        HoleUsage::AllOwn => evaluate(kind, hole),
        HoleUsage::Any => {
            let mut cards = hole.to_vec();
            cards.extend_from_slice(board);
            evaluate(kind, &cards)
        }
        HoleUsage::ExactlyTwo => {
            let mut best: Option<HandValue> = None;
            for i in 0..hole.len() {
                for j in i + 1..hole.len() {
                    for a in 0..board.len() {
                        for b in a + 1..board.len() {
                            for c in b + 1..board.len() {
                                let hand = [hole[i], hole[j], board[a], board[b], board[c]];
                                if let Some(v) = evaluate(kind, &hand) {
                                    best = Some(best.map_or(v, |cur| cur.max(v)));
                                }
                            }
                        }
                    }
                }
            }
            best
        }
    }
}

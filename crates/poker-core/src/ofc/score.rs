//! Row evaluation, royalties, fouling, fantasyland qualification and the
//! pairwise settlement.
//!
//! Everything here reads a *complete* board (13 cards) and nothing else: OFC
//! scoring has no history, no position and no chips. The normative statement
//! of these rules is the module doc of `state.rs`; this module is where they
//! are computed.

use crate::card::Card;
use crate::eval::{HandClass, HandValue, deuce_to_seven_low, high, three_card_high};
use crate::ofc::board::Board;
use crate::ofc::spec::{FantasylandRule, MiddleKind, OfcSpec};
use poker_wire::card::Rank;
use poker_wire::ofc::Royalties;

/// The three row values of one complete board, each from its row's
/// evaluator: top from `three_card_high`, bottom from `high`, middle from
/// `high` or `deuce_to_seven_low` per the variant. Greater is better within
/// a row; values are only ever compared against the same row of another
/// board (plus top-vs-middle for the foul test, which the shared high
/// encoding makes meaningful).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RowValues {
    pub top: HandValue,
    pub middle: HandValue,
    pub bottom: HandValue,
}

/// Everything settlement needs to know about one finished board.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Evaluated {
    pub values: RowValues,
    /// Raw per-row royalties, before fouling voids them. A fouled hand's
    /// royalties are dropped by [`settle`], not here.
    pub royalties: Royalties,
    pub fouled: bool,
}

impl Evaluated {
    /// Royalty points this board actually earns: zero when it fouled.
    pub fn royalty_total(&self) -> u32 {
        if self.fouled {
            0
        } else {
            self.royalties.top + self.royalties.middle + self.royalties.bottom
        }
    }
}

/// Per-seat outcome of a settled hand. `points` sums to zero — the whole
/// point of pairwise scoring is that it only moves points between seats.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OfcSettlement {
    /// Net points, summing to zero.
    pub points: Vec<i64>,
    pub fouled: Vec<bool>,
    /// Royalty points earned (zero for a fouled seat).
    pub royalties: Vec<u32>,
    /// Opponents this seat scooped: all three rows won outright, or the
    /// opponent fouled while this seat did not — the two cases that pay the
    /// same six points plus royalties.
    pub scoops: Vec<u32>,
    /// Card count for the fantasyland hand each seat enters next, if any.
    pub next_fantasyland: Vec<Option<u8>>,
}

/// Evaluate a complete board under `spec`: row values, raw royalties, and
/// whether it fouled. Panics on an incomplete board (an engine bug — hands
/// only settle once every seat has placed thirteen cards).
pub fn evaluate(spec: &OfcSpec, board: &Board) -> Evaluated {
    assert!(
        board.is_complete(),
        "only a complete board can be evaluated"
    );

    let top = three_card_high(&board.top);
    let bottom = high(&board.bottom);
    let middle = match spec.middle {
        MiddleKind::High => high(&board.middle),
        MiddleKind::DeuceToSeven => deuce_to_seven_low(&board.middle),
    };
    let values = RowValues {
        top,
        middle,
        bottom,
    };

    // High-middle variants require top ≤ middle ≤ bottom; the 2-7 middle has
    // no ordering relationship with its neighbours and fouls only by failing
    // its own qualifier, leaving top vs bottom as the ordering to keep.
    let fouled = match spec.middle {
        MiddleKind::High => top > middle || middle > bottom,
        MiddleKind::DeuceToSeven => top > bottom || !deuce_qualifies(&board.middle),
    };

    Evaluated {
        values,
        royalties: Royalties {
            top: top_royalty(&board.top),
            middle: middle_royalty(spec.middle, &board.middle),
            bottom: bottom_royalty(&board.bottom),
        },
        fouled,
    }
}

/// Top-row royalties: pairs from sixes (66 = 1 … AA = 9), trips from deuces
/// (222 = 10 … AAA = 22). Nothing below a pair of sixes pays.
pub fn top_royalty(top: &[Card]) -> u32 {
    let value = three_card_high(top);
    let rank = leading_rank(value);
    match value.high_class() {
        HandClass::Trips => rank as u32 + 10,
        HandClass::OnePair if rank >= Rank::Six.index() => rank as u32 - 3,
        _ => 0,
    }
}

/// Middle-row royalties, per the variant's middle evaluator.
pub fn middle_royalty(kind: MiddleKind, middle: &[Card]) -> u32 {
    match kind {
        MiddleKind::High => {
            let value = high(middle);
            match value.high_class() {
                HandClass::Trips => 2,
                HandClass::Straight => 4,
                HandClass::Flush => 8,
                HandClass::FullHouse => 12,
                HandClass::Quads => 20,
                HandClass::StraightFlush if is_royal(value) => 50,
                HandClass::StraightFlush => 30,
                _ => 0,
            }
        }
        MiddleKind::DeuceToSeven => deuce_middle_royalty(middle),
    }
}

/// Bottom-row royalties: the same ladder as the high middle, halved-ish —
/// the bottom row is the easiest to fill, so it pays least.
pub fn bottom_royalty(bottom: &[Card]) -> u32 {
    let value = high(bottom);
    match value.high_class() {
        HandClass::Straight => 2,
        HandClass::Flush => 4,
        HandClass::FullHouse => 6,
        HandClass::Quads => 10,
        HandClass::StraightFlush if is_royal(value) => 25,
        HandClass::StraightFlush => 15,
        _ => 0,
    }
}

/// Does a 2-7 middle qualify? Ten-low or better: no pair, no straight, no
/// flush, high card at Ten or below (so the worst qualifier is T-9-8-7-5).
/// Aces are always high in 2-7, so A-5-4-3-2 is an ace-high hand and misses.
pub fn deuce_qualifies(middle: &[Card]) -> bool {
    let high_form = deuce_high_form(middle);
    high_form.high_class() == HandClass::HighCard && leading_rank(high_form) <= Rank::Ten.index()
}

/// 2-7 middle royalties: 9-low 1, 8-low 2, 7-low 4, and the exact 7-5-4-3-2
/// wheel 8. Non-qualifying middles pay nothing (they have already fouled the
/// hand).
pub fn deuce_middle_royalty(middle: &[Card]) -> u32 {
    if !deuce_qualifies(middle) {
        return 0;
    }
    if is_deuce_wheel(middle) {
        return 8;
    }
    match leading_rank(deuce_high_form(middle)) {
        r if r == Rank::Nine.index() => 1,
        r if r == Rank::Eight.index() => 2,
        r if r == Rank::Seven.index() => 4,
        _ => 0,
    }
}

/// Exactly 7-5-4-3-2 and not a flush. A suited 7-5-4-3-2 is a flush, which is
/// not a qualifying 2-7 low at all — so it neither pays the wheel royalty nor
/// opens fantasyland.
pub fn is_deuce_wheel(middle: &[Card]) -> bool {
    let mut ranks: Vec<u8> = middle.iter().map(|c| c.rank().index()).collect();
    ranks.sort_unstable();
    ranks
        == [
            Rank::Two.index(),
            Rank::Three.index(),
            Rank::Four.index(),
            Rank::Five.index(),
            Rank::Seven.index(),
        ]
        && !is_flush(middle)
}

/// The card count `board`'s owner enters the next hand's fantasyland with.
///
/// A fouled board never qualifies. A seat *already* in fantasyland can only
/// stay (the entry conditions do not apply to it); every other seat can only
/// enter (there is nothing to stay in).
pub fn fantasyland(
    spec: &OfcSpec,
    board: &Board,
    ev: &Evaluated,
    in_fantasyland: bool,
) -> Option<u8> {
    if ev.fouled {
        return None;
    }
    if in_fantasyland {
        stays(spec, ev).then(|| spec.fantasyland_base())
    } else {
        enters(spec, board, ev)
    }
}

/// Entry from a non-fantasyland hand.
fn enters(spec: &OfcSpec, board: &Board, ev: &Evaluated) -> Option<u8> {
    let class = ev.values.top.high_class();
    let rank = leading_rank(ev.values.top);
    let trips = class == HandClass::Trips;
    let pair_at_least = |r: Rank| class == HandClass::OnePair && rank >= r.index();

    match spec.fantasyland {
        FantasylandRule::Classic { cards, .. } => {
            (trips || pair_at_least(Rank::Queen)).then_some(cards)
        }
        FantasylandRule::Progressive => {
            if trips {
                Some(17)
            } else if pair_at_least(Rank::Ace) {
                Some(16)
            } else if pair_at_least(Rank::King) {
                Some(15)
            } else if pair_at_least(Rank::Queen) {
                Some(14)
            } else {
                None
            }
        }
        FantasylandRule::DeuceMiddle => {
            let top_ok = trips || pair_at_least(Rank::King);
            let middle_ok = is_deuce_wheel(&board.middle);
            match (top_ok, middle_ok) {
                (true, true) => Some(15),
                (true, false) | (false, true) => Some(14),
                (false, false) => None,
            }
        }
    }
}

/// Stay from a fantasyland hand. The reward is always the variant's base
/// count, however strong the qualifying row.
///
/// Top trips or bottom quads+ stay in every variant; a middle full house or
/// better additionally stays only where `Classic { middle_stay: true }` says
/// so (classic OFC). The pineapple family — plain, progressive and 2-7 —
/// stays on the top and bottom conditions alone (and a 2-7 middle is a
/// lowball hand with no full house to reach for anyway).
fn stays(spec: &OfcSpec, ev: &Evaluated) -> bool {
    let top_trips = ev.values.top.high_class() == HandClass::Trips;
    let bottom_quads = ev.values.bottom.high_class() >= HandClass::Quads;
    let middle_full_house = match spec.fantasyland {
        // Only read the middle as a high hand where the rule wants it: a 2-7
        // middle's value is the lowball encoding, meaningless to high_class.
        FantasylandRule::Classic {
            middle_stay: true, ..
        } => ev.values.middle.high_class() >= HandClass::FullHouse,
        _ => false,
    };
    top_trips || bottom_quads || middle_full_house
}

/// Settle a hand: score every unordered pair of seats and sum the results.
///
/// Between two unfouled boards each row won outright pays one point, ties pay
/// nothing, winning all three rows pays three more, and the royalty
/// difference is added on top — royalties count whether or not their row won.
/// A seat that fouled pays six plus its opponent's royalties and collects
/// nothing of its own; two fouled seats exchange nothing.
pub fn settle(evals: &[Evaluated], next_fantasyland: Vec<Option<u8>>) -> OfcSettlement {
    let seats = evals.len();
    debug_assert_eq!(next_fantasyland.len(), seats);

    let royalties: Vec<u32> = evals.iter().map(Evaluated::royalty_total).collect();
    let mut points = vec![0i64; seats];
    let mut scoops = vec![0u32; seats];

    for a in 0..seats {
        for b in (a + 1)..seats {
            let delta = match (evals[a].fouled, evals[b].fouled) {
                (true, true) => 0,
                (true, false) => {
                    scoops[b] += 1;
                    -6 - royalties[b] as i64
                }
                (false, true) => {
                    scoops[a] += 1;
                    6 + royalties[a] as i64
                }
                (false, false) => {
                    let rows = row_margin(&evals[a].values, &evals[b].values);
                    let scoop = match rows {
                        3 => {
                            scoops[a] += 1;
                            3
                        }
                        -3 => {
                            scoops[b] += 1;
                            -3
                        }
                        _ => 0,
                    };
                    rows + scoop + royalties[a] as i64 - royalties[b] as i64
                }
            };
            points[a] += delta;
            points[b] -= delta;
        }
    }

    debug_assert_eq!(
        points.iter().sum::<i64>(),
        0,
        "pairwise scoring must be zero-sum"
    );

    OfcSettlement {
        points,
        fouled: evals.iter().map(|e| e.fouled).collect(),
        royalties,
        scoops,
        next_fantasyland,
    }
}

/// Rows won minus rows lost, in `-3..=3`.
fn row_margin(a: &RowValues, b: &RowValues) -> i64 {
    let row = |x: HandValue, y: HandValue| match x.cmp(&y) {
        core::cmp::Ordering::Greater => 1,
        core::cmp::Ordering::Less => -1,
        core::cmp::Ordering::Equal => 0,
    };
    row(a.top, b.top) + row(a.middle, b.middle) + row(a.bottom, b.bottom)
}

/// The most significant tiebreak nibble: the pair/trip/straight-high rank for
/// the classes that have one, otherwise the hand's highest card.
fn leading_rank(value: HandValue) -> u8 {
    ((value.0 >> 16) & 0xF) as u8
}

/// A royal flush is just a straight flush whose high card is the ace.
fn is_royal(value: HandValue) -> bool {
    value.high_class() == HandClass::StraightFlush && leading_rank(value) == Rank::Ace.index()
}

/// The 2-7 value re-expressed as the high encoding it inverts, so the
/// qualifier and the royalty ladder can read its class and ranks directly.
fn deuce_high_form(middle: &[Card]) -> HandValue {
    HandValue(0x00FF_FFFF - deuce_to_seven_low(middle).0)
}

fn is_flush(cards: &[Card]) -> bool {
    cards
        .first()
        .is_some_and(|first| cards.iter().all(|c| c.suit() == first.suit()))
}

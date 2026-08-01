//! The OFC rules engine: the three-card evaluator, royalty schedules,
//! fouling, pairwise settlement, fantasyland, and full scripted hands of all
//! four variants.
//!
//! Scripted hands name each seat's finished board and its discards; [`Script`]
//! lays those cards out in the order the variant's schedule deals them,
//! [`deck_for`] interleaves the seats, and [`play`] answers every request by
//! looking each dealt card up in the seat's plan. That keeps the fixtures
//! readable as *boards*, which is how OFC rules are actually stated.

use std::collections::{HashMap, HashSet};

use poker_core::card::{Card, Deck, parse_cards};
use poker_core::eval::{HandValue, high, three_card_high};
use poker_core::ofc::score::{
    self, Evaluated, OfcSettlement, RowValues, bottom_royalty, deuce_middle_royalty,
    deuce_qualifies, middle_royalty, top_royalty,
};
use poker_core::ofc::spec::{MiddleKind, OFC, OFC_27, OFC_PINEAPPLE, OFC_PROGRESSIVE, OfcSpec};
use poker_core::ofc::state::{OfcError, OfcHandState, PlacementRequest, table_order};
use poker_core::ofc::{Board, OfcAction, OfcEvent, Placement, Row, Royalties};
use poker_core::rng::Rng64;

// ---- helpers -----------------------------------------------------------

fn cards(s: &str) -> Vec<Card> {
    parse_cards(s).unwrap()
}

fn top3(s: &str) -> HandValue {
    three_card_high(&cards(s))
}

fn five(s: &str) -> HandValue {
    high(&cards(s))
}

/// One seat's scripted hand: the finished board plus the cards it discards,
/// in the order the discards are dealt.
#[derive(Clone)]
struct Script {
    top: Vec<Card>,
    middle: Vec<Card>,
    bottom: Vec<Card>,
    discards: Vec<Card>,
}

fn script(top: &str, middle: &str, bottom: &str, discards: &str) -> Script {
    let s = Script {
        top: cards(top),
        middle: cards(middle),
        bottom: cards(bottom),
        discards: cards(discards),
    };
    assert_eq!(s.top.len(), Board::TOP_CAPACITY);
    assert_eq!(s.middle.len(), Board::MIDDLE_CAPACITY);
    assert_eq!(s.bottom.len(), Board::BOTTOM_CAPACITY);
    s
}

impl Script {
    /// Card → row for every card that reaches the board; anything else the
    /// seat is dealt is a discard.
    fn plan(&self) -> HashMap<Card, Row> {
        let mut plan = HashMap::new();
        for (row, cards) in [
            (Row::Top, &self.top),
            (Row::Middle, &self.middle),
            (Row::Bottom, &self.bottom),
        ] {
            for card in cards {
                plan.insert(*card, row);
            }
        }
        plan
    }

    /// The seat's cards in the order it must be dealt them: the opening batch
    /// is all board cards, and every later round is `round_place` board cards
    /// followed by that round's discards. A fantasyland seat takes everything
    /// in one batch.
    fn layout(&self, spec: &OfcSpec, fantasyland: bool) -> Vec<Card> {
        let board: Vec<Card> = self
            .top
            .iter()
            .chain(&self.middle)
            .chain(&self.bottom)
            .copied()
            .collect();
        if fantasyland {
            return board.iter().chain(&self.discards).copied().collect();
        }
        let per_round = (spec.round_deal - spec.round_place) as usize;
        assert_eq!(self.discards.len(), per_round * spec.rounds as usize);

        let mut out = board[..spec.initial_deal as usize].to_vec();
        let mut placed = spec.initial_deal as usize;
        for round in 0..spec.rounds as usize {
            out.extend_from_slice(&board[placed..placed + spec.round_place as usize]);
            placed += spec.round_place as usize;
            out.extend_from_slice(&self.discards[round * per_round..(round + 1) * per_round]);
        }
        out
    }
}

/// Lay the seats' cards out in the order the engine draws them: the opening
/// deals in table order, then each round's deals in table order, skipping
/// fantasyland seats (dealt everything up front).
fn deck_for(spec: &OfcSpec, scripts: &[Script], fantasyland: &[Option<u8>]) -> Deck {
    let seats = scripts.len();
    let layouts: Vec<Vec<Card>> = scripts
        .iter()
        .enumerate()
        .map(|(seat, s)| s.layout(spec, fantasyland[seat].is_some()))
        .collect();
    let mut cursor = vec![0usize; seats];
    let mut order = Vec::new();

    for seat in table_order(seats) {
        let count = fantasyland[seat].unwrap_or(spec.initial_deal) as usize;
        order.extend_from_slice(&layouts[seat][cursor[seat]..cursor[seat] + count]);
        cursor[seat] += count;
    }
    for _ in 0..spec.rounds {
        for seat in table_order(seats) {
            if fantasyland[seat].is_some() {
                continue;
            }
            let count = spec.round_deal as usize;
            order.extend_from_slice(&layouts[seat][cursor[seat]..cursor[seat] + count]);
            cursor[seat] += count;
        }
    }
    for (seat, layout) in layouts.iter().enumerate() {
        assert_eq!(
            cursor[seat],
            layout.len(),
            "seat {seat} layout not consumed"
        );
    }
    Deck::from_deal_order(&order)
}

fn planned_action(plan: &HashMap<Card, Row>, request: &PlacementRequest) -> OfcAction {
    let mut placements = Vec::new();
    let mut discards = Vec::new();
    for card in &request.dealt {
        match plan.get(card) {
            Some(row) => placements.push(Placement {
                card: *card,
                row: *row,
            }),
            None => discards.push(*card),
        }
    }
    OfcAction {
        placements,
        discards,
    }
}

/// Run a scripted hand to settlement.
fn play(spec: &OfcSpec, scripts: &[Script], fantasyland: &[Option<u8>]) -> OfcHandState {
    let deck = deck_for(spec, scripts, fantasyland);
    let plans: Vec<HashMap<Card, Row>> = scripts.iter().map(Script::plan).collect();
    let (mut state, _) =
        OfcHandState::new(spec, scripts.len(), fantasyland, 1, deck).expect("valid setup");
    while let Some(request) = state.request() {
        let action = planned_action(&plans[request.seat], &request);
        state
            .apply(&action)
            .unwrap_or_else(|e| panic!("scripted action rejected: {e}"));
    }
    state
}

fn settlement(state: &OfcHandState) -> &OfcSettlement {
    state.settlement().expect("hand must be settled")
}

/// A compact, order-sensitive summary of an event, for stream assertions.
fn tag(ev: &OfcEvent) -> String {
    match ev {
        OfcEvent::Fantasyland { seat, cards } => format!("fantasyland {seat} {cards}"),
        OfcEvent::Deal { seat, cards, count } => format!("deal {seat} {} {count}", cards.len()),
        OfcEvent::Place {
            seat,
            placements,
            discarded,
            count,
        } => format!(
            "place {seat} {} {} {count}",
            placements.len(),
            discarded.len()
        ),
        OfcEvent::Showdown { seat, fouled, .. } => format!("showdown {seat} {fouled}"),
        OfcEvent::Score { seat, points } => format!("score {seat} {points}"),
        OfcEvent::Unknown => "unknown".to_string(),
    }
}

fn tags(state: &OfcHandState) -> Vec<String> {
    state.events().iter().map(tag).collect()
}

/// The deterministic filler: sort the dealt cards ascending by index, take the
/// first `place`, drop each into bottom if it has space, else middle, else
/// top. This is the arena's fault substitution, used here to drive sweeps.
fn filler(board: &Board, request: &PlacementRequest) -> OfcAction {
    let mut dealt = request.dealt.clone();
    dealt.sort_by_key(|c| c.index());
    let mut free = [
        board.free(Row::Bottom),
        board.free(Row::Middle),
        board.free(Row::Top),
    ];
    let mut placements = Vec::new();
    for card in dealt.iter().take(request.place as usize) {
        let row = if free[0] > 0 {
            free[0] -= 1;
            Row::Bottom
        } else if free[1] > 0 {
            free[1] -= 1;
            Row::Middle
        } else {
            free[2] -= 1;
            Row::Top
        };
        placements.push(Placement { card: *card, row });
    }
    OfcAction {
        placements,
        discards: dealt[request.place as usize..].to_vec(),
    }
}

/// A uniformly random legal placement.
fn random_action(board: &Board, request: &PlacementRequest, rng: &mut Rng64) -> OfcAction {
    let mut dealt = request.dealt.clone();
    rng.shuffle(&mut dealt);
    let mut free = [
        board.free(Row::Top),
        board.free(Row::Middle),
        board.free(Row::Bottom),
    ];
    let mut placements = Vec::new();
    for card in dealt.iter().take(request.place as usize) {
        let open: Vec<usize> = (0..3).filter(|i| free[*i] > 0).collect();
        let pick = open[rng.below(open.len() as u64) as usize];
        free[pick] -= 1;
        placements.push(Placement {
            card: *card,
            row: [Row::Top, Row::Middle, Row::Bottom][pick],
        });
    }
    OfcAction {
        placements,
        discards: dealt[request.place as usize..].to_vec(),
    }
}

fn evaluated(
    top: HandValue,
    middle: HandValue,
    bottom: HandValue,
    royalties: (u32, u32, u32),
    fouled: bool,
) -> Evaluated {
    Evaluated {
        values: RowValues {
            top,
            middle,
            bottom,
        },
        royalties: Royalties {
            top: royalties.0,
            middle: royalties.1,
            bottom: royalties.2,
        },
        fouled,
    }
}

fn settle(evals: &[Evaluated]) -> OfcSettlement {
    score::settle(evals, vec![None; evals.len()])
}

fn board_of(top: &str, middle: &str, bottom: &str) -> Board {
    Board {
        top: cards(top),
        middle: cards(middle),
        bottom: cards(bottom),
    }
}

// ---- three_card_high ---------------------------------------------------

#[test]
fn three_card_classes_are_strictly_ordered() {
    assert!(top3("9c 9d 9h") > top3("9c 9d Ah"));
    assert!(top3("9c 9d Ah") > top3("Ac Kd 9h"));
    // No straights and no flushes on a three-card row.
    assert_eq!(top3("Ac Kc Qc"), top3("Ac Kd Qh"));
    assert_eq!(top3("5c 4d 3h"), top3("5c 4c 3c"));
    assert!(top3("Ac Kd Qh") > top3("5c 4d 3h"));
}

#[test]
fn three_card_tiebreaks_run_down_the_ranks() {
    assert!(top3("Ac Ad Kh") > top3("Ac Ad Qh"));
    assert!(top3("Ac Ad 2h") > top3("Kc Kd Ah"));
    assert!(top3("9c 9d 9h") > top3("8c 8d 8h"));
    assert!(top3("Ac Kd 7h") > top3("Ac Kd 6h"));
    assert!(top3("Ac Qd Jh") > top3("Kc Qd Jh"));
    // The pair is found wherever it sits among the three cards.
    assert_eq!(top3("Ah 6s 6h"), top3("6s 6h Ah"));
}

#[test]
fn three_card_encodings_are_pinned() {
    // class << 20 | five 4-bit tiebreak nibbles, most significant first, the
    // unused ones zero.
    assert_eq!(top3("As Kd 7c"), HandValue(0x000C_B500));
    assert_eq!(top3("6s 6h Ad"), HandValue(0x0014_C000));
    assert_eq!(top3("2c 2d 2h"), HandValue(0x0030_0000));
    assert_eq!(top3("Ac Ad 2c"), HandValue(0x001C_0000));
    assert_eq!(top3("Ac Ad Ah"), HandValue(0x003C_0000));
}

#[test]
fn a_five_card_hand_never_loses_to_the_three_card_hand_it_contains() {
    // The zero fill makes top-vs-middle a plain comparison: equal ranks
    // through the top row's three cards means the middle holds, which does not
    // foul.
    assert!(top3("6s 6h Ad") < five("6c 6d Ac 3s 2h"));
    assert!(top3("Ac Kd Qh") < five("Ac Kd Qh 2s 3c"));
    assert!(top3("2c 2d 3h") < five("2c 2d 3h 4s 5c"));
    // …and a genuinely stronger top does beat the middle.
    assert!(top3("Ac Ad Ah") > five("Kc Kd Ks 2s 3c"));
    assert!(top3("Ac Ad 2h") > five("Kc Kd 9s 4s 3c"));
}

#[test]
#[should_panic(expected = "exactly 3 cards")]
fn three_card_high_rejects_the_wrong_card_count() {
    three_card_high(&cards("Ac Kd Qh Js"));
}

// ---- royalty schedules -------------------------------------------------

#[test]
fn top_royalties_run_from_sixes_to_trip_aces() {
    assert_eq!(top_royalty(&cards("Ac Kd Qh")), 0);
    assert_eq!(top_royalty(&cards("5c 5d Ah")), 0);
    for (hand, points) in [
        ("6c 6d Ah", 1),
        ("7c 7d Ah", 2),
        ("8c 8d Ah", 3),
        ("9c 9d Ah", 4),
        ("Tc Td Ah", 5),
        ("Jc Jd Ah", 6),
        ("Qc Qd Ah", 7),
        ("Kc Kd Ah", 8),
        ("Ac Ad Kh", 9),
    ] {
        assert_eq!(top_royalty(&cards(hand)), points, "{hand}");
    }
    for (hand, points) in [
        ("2c 2d 2h", 10),
        ("3c 3d 3h", 11),
        ("4c 4d 4h", 12),
        ("5c 5d 5h", 13),
        ("6c 6d 6h", 14),
        ("7c 7d 7h", 15),
        ("8c 8d 8h", 16),
        ("9c 9d 9h", 17),
        ("Tc Td Th", 18),
        ("Jc Jd Jh", 19),
        ("Qc Qd Qh", 20),
        ("Kc Kd Kh", 21),
        ("Ac Ad Ah", 22),
    ] {
        assert_eq!(top_royalty(&cards(hand)), points, "{hand}");
    }
}

#[test]
fn high_middle_royalties_pay_from_trips_up() {
    for (hand, points) in [
        ("Ac Kd Qh 9s 2c", 0),
        ("Ac Ad Qh 9s 2c", 0),
        ("Ac Ad Kh Ks 2c", 0),
        ("9c 9d 9h Ks 2c", 2),
        ("9c 8d 7h 6s 5c", 4),
        ("Ac Kc 9c 6c 2c", 8),
        ("9c 9d 9h Ks Kc", 12),
        ("9c 9d 9h 9s Kc", 20),
        ("9c 8c 7c 6c 5c", 30),
        ("Ac Kc Qc Jc Tc", 50),
    ] {
        assert_eq!(
            middle_royalty(MiddleKind::High, &cards(hand)),
            points,
            "{hand}"
        );
    }
    // The wheel straight flush is not royal: its high card is the five.
    assert_eq!(
        middle_royalty(MiddleKind::High, &cards("5c 4c 3c 2c Ac")),
        30
    );
}

#[test]
fn bottom_royalties_pay_from_straights_up() {
    for (hand, points) in [
        ("Ac Kd Qh 9s 2c", 0),
        ("Ac Ad Kh Ks 2c", 0),
        ("9c 9d 9h Ks 2c", 0),
        ("9c 8d 7h 6s 5c", 2),
        ("Ac Kc 9c 6c 2c", 4),
        ("9c 9d 9h Ks Kc", 6),
        ("9c 9d 9h 9s Kc", 10),
        ("9c 8c 7c 6c 5c", 15),
        ("Ac Kc Qc Jc Tc", 25),
    ] {
        assert_eq!(bottom_royalty(&cards(hand)), points, "{hand}");
    }
    assert_eq!(bottom_royalty(&cards("5c 4c 3c 2c Ac")), 15);
}

#[test]
fn the_deuce_to_seven_middle_qualifies_at_ten_low() {
    assert!(deuce_qualifies(&cards("Th 9d 8s 7c 5h"))); // the worst qualifier
    assert!(deuce_qualifies(&cards("7h 5d 4s 3c 2h"))); // the best
    assert!(!deuce_qualifies(&cards("Jh 9d 8s 7c 5h"))); // jack high
    assert!(!deuce_qualifies(&cards("Ah 5d 4s 3c 2h"))); // aces are high
    assert!(!deuce_qualifies(&cards("Th 9d 9s 7c 5h"))); // a pair
    assert!(!deuce_qualifies(&cards("9h 8h 7h 5h 4h"))); // a flush
    assert!(!deuce_qualifies(&cards("9h 8d 7s 6c 5h"))); // a straight
}

#[test]
fn deuce_to_seven_middle_royalties_climb_to_the_wheel() {
    for (hand, points) in [
        ("Th 9d 8s 7c 5h", 0),
        ("9h 8d 7s 5c 4h", 1),
        ("8h 7d 5s 4c 3h", 2),
        ("7h 6d 5s 4c 2h", 4),
        ("7h 5d 4s 3c 2h", 8),
        ("Jh 9d 8s 7c 5h", 0), // does not qualify
    ] {
        assert_eq!(deuce_middle_royalty(&cards(hand)), points, "{hand}");
        assert_eq!(
            middle_royalty(MiddleKind::DeuceToSeven, &cards(hand)),
            points,
            "{hand}"
        );
    }
    // A suited 7-5-4-3-2 is a flush, so it is not a 2-7 low at all.
    assert_eq!(deuce_middle_royalty(&cards("7h 5h 4h 3h 2h")), 0);
    assert!(!deuce_qualifies(&cards("7h 5h 4h 3h 2h")));
}

// ---- fouling -----------------------------------------------------------

#[test]
fn high_variants_foul_on_an_out_of_order_board() {
    let ok = score::evaluate(
        &OFC,
        &board_of("2c 3d 4h", "9c 9d 5s 6h 7c", "Ac Ad Ah 2s 3s"),
    );
    assert!(!ok.fouled);

    let top_over_middle = score::evaluate(
        &OFC,
        &board_of("Ac Ad Ah", "9c 9d 5s 6h 7c", "Kc Kd Ks 2s 3s"),
    );
    assert!(top_over_middle.fouled);

    let middle_over_bottom = score::evaluate(
        &OFC,
        &board_of("2c 3d 4h", "Ac Ad Ah 5s 6s", "Kc Kd Ks 7s 8s"),
    );
    assert!(middle_over_bottom.fouled);
}

#[test]
fn equal_rows_do_not_foul() {
    // Top and middle tie through the top's three ranks; the middle's extra
    // cards can only raise it, never lower it.
    let ev = score::evaluate(
        &OFC,
        &board_of("6s 6h Ad", "6c 6d Ac 3s 2h", "Kc Kd Ks 4c 4d"),
    );
    assert!(!ev.fouled);
}

#[test]
fn the_deuce_middle_fouls_only_by_failing_its_qualifier_or_top_over_bottom() {
    // A middle that would be "too strong" for a high variant is fine here.
    let ok = score::evaluate(
        &OFC_27,
        &board_of("2c 3d 4h", "Th 9d 8s 7c 5h", "Ac Ad Ah As Ks"),
    );
    assert!(!ok.fouled);

    let unqualified = score::evaluate(
        &OFC_27,
        &board_of("2c 3d 4h", "Jh 9d 8s 7c 5h", "Ac Ad Ah As Ks"),
    );
    assert!(unqualified.fouled);

    let top_over_bottom = score::evaluate(
        &OFC_27,
        &board_of("Ac Ad Ah", "Th 9d 8s 7c 5h", "Kc Qd Js 4c 2d"),
    );
    assert!(top_over_bottom.fouled);
}

// ---- pairwise scoring --------------------------------------------------

#[test]
fn a_split_board_pays_only_the_royalty_difference() {
    let a = evaluated(
        top3("Ac Kd Qh"),
        five("2c 2d 3h 3s 4c"),
        five("9c 9d 9h 9s Kc"),
        (0, 0, 10),
        false,
    );
    let b = evaluated(
        top3("Ac Kd Qh"),
        five("5c 5d 6h 6s 4c"),
        five("8c 8d 8h 8s Kc"),
        (0, 0, 10),
        false,
    );
    // The top ties, `a` takes the bottom, `b` takes the middle, and the
    // royalties cancel.
    let settled = settle(&[a, b]);
    assert_eq!(settled.points, vec![0, 0]);
    assert_eq!(settled.scoops, vec![0, 0]);
}

#[test]
fn winning_every_row_pays_three_rows_plus_the_scoop() {
    let a = evaluated(
        top3("Ac Ad Kh"),
        five("9c 9d 9h Ks 2c"),
        five("9c 9d 9h 9s Kc"),
        (0, 0, 0),
        false,
    );
    let b = evaluated(
        top3("2c 3d 4h"),
        five("Ac Kd Qh 9s 2c"),
        five("Ac Kd Qh 9s 2c"),
        (0, 0, 0),
        false,
    );
    let settled = settle(&[a, b]);
    assert_eq!(settled.points, vec![6, -6]);
    assert_eq!(settled.scoops, vec![1, 0]);
}

#[test]
fn royalties_net_in_both_directions() {
    let strong_rows = evaluated(
        top3("Ac Ad Kh"),
        five("9c 9d 9h Ks 2c"),
        five("9c 9d 9h 9s Kc"),
        (0, 0, 0),
        false,
    );
    let strong_royalties = evaluated(
        top3("2c 3d 4h"),
        five("Ac Kd Qh 9s 2c"),
        five("Ac Kd Qh 9s 2c"),
        (0, 0, 20),
        false,
    );
    // Six points of rows against twenty points of royalties.
    let settled = settle(&[strong_rows, strong_royalties]);
    assert_eq!(settled.points, vec![-14, 14]);
    // The scoop still happened, even though its owner lost on points.
    assert_eq!(settled.scoops, vec![1, 0]);

    let mirrored = settle(&[strong_royalties, strong_rows]);
    assert_eq!(mirrored.points, vec![14, -14]);
    assert_eq!(mirrored.scoops, vec![0, 1]);
}

#[test]
fn a_fouled_hand_pays_six_plus_the_opponents_royalties_and_keeps_none() {
    let fouler = evaluated(
        top3("Ac Ad Ah"),
        five("9c 9d 9h 9s Kc"),
        five("Ac Kd Qh 9s 2c"),
        (22, 20, 0),
        true,
    );
    let clean = evaluated(
        top3("2c 3d 4h"),
        five("Ac Kd Qh 9s 2c"),
        five("9c 8d 7h 6s 5c"),
        (0, 0, 2),
        false,
    );
    let settled = settle(&[fouler, clean]);
    assert_eq!(settled.points, vec![-8, 8]);
    assert_eq!(settled.royalties, vec![0, 2]);
    assert_eq!(settled.fouled, vec![true, false]);
    assert_eq!(settled.scoops, vec![0, 1]);
}

#[test]
fn two_fouled_hands_exchange_nothing() {
    let a = evaluated(
        top3("Ac Ad Ah"),
        five("9c 9d 9h 9s Kc"),
        five("Ac Kd Qh 9s 2c"),
        (22, 20, 0),
        true,
    );
    let b = evaluated(
        top3("Kc Kd Kh"),
        five("8c 8d 8h 8s Kc"),
        five("Ac Kd Qh 9s 3c"),
        (21, 20, 0),
        true,
    );
    let settled = settle(&[a, b]);
    assert_eq!(settled.points, vec![0, 0]);
    assert_eq!(settled.scoops, vec![0, 0]);
}

#[test]
fn three_and_four_handed_nets_sum_to_zero() {
    let strong = evaluated(
        top3("Ac Ad Kh"),
        five("9c 9d 9h 9s Kc"),
        five("Ac Kc Qc Jc Tc"),
        (9, 20, 25),
        false,
    );
    let middling = evaluated(
        top3("8c 8d Kh"),
        five("9c 9d 9h Ks 2c"),
        five("9c 8d 7h 6s 5c"),
        (3, 2, 2),
        false,
    );
    let weak = evaluated(
        top3("2c 3d 4h"),
        five("Ac Kd Qh 9s 2c"),
        five("Ac Kd Qh 9s 3c"),
        (0, 0, 0),
        false,
    );
    let fouled = evaluated(
        top3("Ac Ad Ah"),
        five("2c 3d 4h 5s 7c"),
        five("2c 3d 4h 5s 8c"),
        (22, 0, 0),
        true,
    );

    let three = settle(&[strong, middling, weak]);
    assert_eq!(three.points, vec![113, -40, -73]);
    assert_eq!(three.points.iter().sum::<i64>(), 0);
    assert_eq!(three.scoops, vec![2, 1, 0]);

    let four = settle(&[strong, middling, weak, fouled]);
    assert_eq!(four.points.iter().sum::<i64>(), 0);
    assert_eq!(four.fouled, vec![false, false, false, true]);
    assert_eq!(four.royalties, vec![54, 7, 0, 0]);
    // The fouler pays six plus royalties to each of the other three.
    assert_eq!(four.points[3], -(6 + 54) - (6 + 7) - 6);
    assert_eq!(four.scoops, vec![3, 2, 1, 0]);
}

// ---- scripted hands: ofc ----------------------------------------------

fn ofc_clean_scripts() -> Vec<Script> {
    vec![
        // Seat 0: fives up top, trip tens, quad jacks.
        script("5c 5d 6c", "Ts Td Th 8c 7c", "Js Jd Jh Jc 9s", ""),
        // Seat 1: kings up top, trip nines, quad aces.
        script("Kc Kd 2c", "9c 9d 9h 4s 3s", "Ac Ad Ah As 5s", ""),
    ]
}

#[test]
fn a_clean_ofc_hand_emits_the_documented_event_stream_and_nets() {
    let state = play(&OFC, &ofc_clean_scripts(), &[None, None]);

    // Table order is 1 then 0 throughout: the opening deals, the opening
    // placements, then eight rounds of deal-then-place per seat, then the
    // showdowns and the scores.
    let mut expected = vec![
        "deal 1 5 5".to_string(),
        "deal 0 5 5".to_string(),
        "place 1 5 0 0".to_string(),
        "place 0 5 0 0".to_string(),
    ];
    for _ in 0..8 {
        expected.push("deal 1 1 1".to_string());
        expected.push("place 1 1 0 0".to_string());
        expected.push("deal 0 1 1".to_string());
        expected.push("place 0 1 0 0".to_string());
    }
    expected.extend([
        "showdown 1 false".to_string(),
        "showdown 0 false".to_string(),
        "score 1 9".to_string(),
        "score 0 -9".to_string(),
    ]);
    assert_eq!(tags(&state), expected);

    let settled = settlement(&state);
    // Seat 1 takes the bottom (quad aces) and the top (kings), seat 0 takes
    // the middle (trip tens): one row net to seat 1, no scoop, plus the
    // royalty difference 20 − 12.
    assert_eq!(settled.points, vec![-9, 9]);
    assert_eq!(settled.royalties, vec![12, 20]);
    assert_eq!(settled.scoops, vec![0, 0]);
    assert_eq!(settled.fouled, vec![false, false]);
    // Seat 1's kings up top open classic OFC's thirteen-card fantasyland.
    assert_eq!(settled.next_fantasyland, vec![None, Some(13)]);
}

#[test]
fn showdown_events_carry_the_row_values_and_raw_royalties() {
    let state = play(&OFC, &ofc_clean_scripts(), &[None, None]);
    let showdown = state
        .events()
        .iter()
        .find(|ev| matches!(ev, OfcEvent::Showdown { seat: 1, .. }))
        .expect("seat 1 shows down");
    match showdown {
        OfcEvent::Showdown {
            top_value,
            middle_value,
            bottom_value,
            royalties,
            fouled,
            ..
        } => {
            assert_eq!(*top_value, top3("Kc Kd 2c"));
            assert_eq!(*middle_value, five("9c 9d 9h 4s 3s"));
            assert_eq!(*bottom_value, five("Ac Ad Ah As 5s"));
            assert_eq!(
                *royalties,
                Royalties {
                    top: 8,
                    middle: 2,
                    bottom: 10
                }
            );
            assert!(!fouled);
        }
        _ => unreachable!(),
    }
}

#[test]
fn an_ofc_foul_costs_six_plus_the_opponents_royalties() {
    let scripts = vec![
        script("Qc Qd 3s", "8c 8d 8h 6s 5s", "Ts Td Th Tc 4s", ""),
        // Trip aces over a seven-high middle: fouled, and its own 22 + 10 in
        // royalties is void.
        script("Ac Ad Ah", "2c 3c 4c 5c 7d", "Ks Kd Kh Kc 9s", ""),
    ];
    let state = play(&OFC, &scripts, &[None, None]);
    let settled = settlement(&state);
    assert_eq!(settled.fouled, vec![false, true]);
    assert_eq!(settled.royalties, vec![19, 0]);
    assert_eq!(settled.points, vec![25, -25]);
    assert_eq!(settled.scoops, vec![1, 0]);
    // The fouler earns nothing, fantasyland included; its opponent's queens
    // still do.
    assert_eq!(settled.next_fantasyland, vec![Some(13), None]);
}

// ---- scripted hands: pineapple ----------------------------------------

fn pineapple_scripts() -> Vec<Script> {
    vec![
        script(
            "2c 3s 4s",
            "5s 6c 7d 8d Th",
            "Kc Kd Ks Kh 6d",
            "2d 7h 8h 9h",
        ),
        // Queens up top: fantasyland next hand.
        script(
            "Qc Qd 2h",
            "9c 9d 9s 4c 3c",
            "Ac Ad Ah As 5c",
            "7s 8s Ts Js",
        ),
    ]
}

#[test]
fn a_pineapple_hand_discards_one_a_round_and_queens_up_earn_fantasyland() {
    let state = play(&OFC_PINEAPPLE, &pineapple_scripts(), &[None, None]);

    let mut expected = vec![
        "deal 1 5 5".to_string(),
        "deal 0 5 5".to_string(),
        "place 1 5 0 0".to_string(),
        "place 0 5 0 0".to_string(),
    ];
    for _ in 0..4 {
        expected.push("deal 1 3 3".to_string());
        expected.push("place 1 2 1 1".to_string());
        expected.push("deal 0 3 3".to_string());
        expected.push("place 0 2 1 1".to_string());
    }
    expected.extend([
        "showdown 1 false".to_string(),
        "showdown 0 false".to_string(),
        "score 1 15".to_string(),
        "score 0 -15".to_string(),
    ]);
    assert_eq!(tags(&state), expected);

    let settled = settlement(&state);
    // Seat 1 scoops (six) but seat 0's quad kings claw back nine of the
    // royalty difference.
    assert_eq!(settled.points, vec![-15, 15]);
    assert_eq!(settled.scoops, vec![0, 1]);
    assert_eq!(settled.royalties, vec![10, 19]);
    assert_eq!(settled.next_fantasyland, vec![None, Some(14)]);
}

#[test]
fn discards_are_private_but_open_face_placements_are_not() {
    let state = play(&OFC_PINEAPPLE, &pineapple_scripts(), &[None, None]);
    let place = state
        .events()
        .iter()
        .find(|ev| {
            matches!(
                ev,
                OfcEvent::Place {
                    seat: 1,
                    count: 1,
                    ..
                }
            )
        })
        .expect("seat 1 discards in every round");

    assert_eq!(&state.redacted_for(place, 1), place);
    match state.redacted_for(place, 0) {
        OfcEvent::Place {
            placements,
            discarded,
            count,
            ..
        } => {
            assert_eq!(placements.len(), 2, "boards are open face");
            assert!(discarded.is_empty(), "discards are private");
            assert_eq!(count, 1, "the discard count is public");
        }
        _ => unreachable!(),
    }

    // The deals themselves stay private to their seat.
    let deal = state
        .events()
        .iter()
        .find(|ev| matches!(ev, OfcEvent::Deal { seat: 1, .. }))
        .unwrap();
    match state.redacted_for(deal, 0) {
        OfcEvent::Deal { cards, count, .. } => {
            assert!(cards.is_empty());
            assert_eq!(count, 5);
        }
        _ => unreachable!(),
    }
}

// ---- scripted hands: fantasyland --------------------------------------

/// Seat 1 plays a fantasyland hand; seat 0 plays the ordinary structure.
fn fantasyland_scripts(seat1: Script, seat0_discards: &str) -> Vec<Script> {
    vec![
        script(
            "2d 3d 4d",
            "5s 6s 8d Ts Js",
            "Kh Ks Qh Qs Qd",
            seat0_discards,
        ),
        seat1,
    ]
}

/// Trip sevens up top: a stay under every schedule.
fn fantasyland_stay_hand(discards: &str) -> Script {
    script("7c 7d 7h", "9c 9d 9h 9s Kc", "Ac Ad Ah As Kd", discards)
}

#[test]
fn a_fantasyland_board_stays_hidden_until_showdown_and_can_stay() {
    let scripts = fantasyland_scripts(fantasyland_stay_hand("2c"), "3c 4c 5c 6c");
    let state = play(&OFC_PINEAPPLE, &scripts, &[None, Some(14)]);

    // The fantasyland seat is announced, dealt all fourteen cards at once, and
    // takes its single turn before the open-face seat starts.
    let stream = tags(&state);
    let opening = vec![
        "fantasyland 1 14".to_string(),
        "deal 1 14 14".to_string(),
        "deal 0 5 5".to_string(),
        "place 1 13 1 1".to_string(),
        "place 0 5 0 0".to_string(),
    ];
    assert_eq!(&stream[..5], opening.as_slice());

    let place = state
        .events()
        .iter()
        .find(|ev| matches!(ev, OfcEvent::Place { seat: 1, .. }))
        .unwrap();
    match state.redacted_for(place, 0) {
        OfcEvent::Place {
            placements,
            discarded,
            count,
            ..
        } => {
            assert!(placements.is_empty(), "a fantasyland board is hidden");
            assert!(discarded.is_empty());
            assert_eq!(count, 1);
        }
        _ => unreachable!(),
    }
    assert_eq!(&state.redacted_for(place, 1), place);

    // Showdown is the reveal: it passes through redaction untouched.
    let showdown = state
        .events()
        .iter()
        .find(|ev| matches!(ev, OfcEvent::Showdown { seat: 1, .. }))
        .unwrap();
    assert_eq!(&state.redacted_for(showdown, 0), showdown);
    match showdown {
        OfcEvent::Showdown {
            top,
            next_fantasyland,
            ..
        } => {
            assert_eq!(top, &cards("7c 7d 7h"));
            assert_eq!(*next_fantasyland, Some(14));
        }
        _ => unreachable!(),
    }

    let settled = settlement(&state);
    assert_eq!(settled.next_fantasyland, vec![None, Some(14)]);
    assert_eq!(settled.royalties, vec![6, 45]);
    assert_eq!(settled.points, vec![-45, 45]);
    assert_eq!(settled.scoops, vec![0, 1]);
}

#[test]
fn a_fantasyland_hand_without_a_stay_drops_back_out() {
    // Kings up top would open fantasyland from the outside, but a seat already
    // in it needs trips, a full house in the middle, or quads at the bottom.
    let dropout = script("Kc Kd 2h", "9c 9d 9h 4h 3h", "Ac Ad Ah 5h 6h", "2c");
    let state = play(
        &OFC_PINEAPPLE,
        &fantasyland_scripts(dropout, "3c 4c 5c 6c"),
        &[None, Some(14)],
    );
    let settled = settlement(&state);
    assert_eq!(settled.fouled, vec![false, false]);
    assert_eq!(settled.next_fantasyland, vec![None, None]);
}

#[test]
fn classic_ofc_fantasyland_deals_exactly_thirteen() {
    let scripts = vec![
        script("2d 3d 4d", "5s 6s 8d Ts Js", "Kh Ks Qh Qs Qd", ""),
        fantasyland_stay_hand(""),
    ];
    let state = play(&OFC, &scripts, &[None, Some(13)]);
    let opening = vec![
        "fantasyland 1 13".to_string(),
        "deal 1 13 13".to_string(),
        "deal 0 5 5".to_string(),
        "place 1 13 0 0".to_string(),
        "place 0 5 0 0".to_string(),
    ];
    assert_eq!(&tags(&state)[..5], opening.as_slice());
    // Classic OFC's fantasyland is thirteen cards, so a stay grants thirteen.
    assert_eq!(settlement(&state).next_fantasyland, vec![None, Some(13)]);
}

// ---- scripted hands: progressive --------------------------------------

#[test]
fn progressive_fantasyland_scales_with_the_top_row() {
    let scripts = vec![
        // Trip eights up top: seventeen cards.
        script(
            "8c 8d 8h",
            "Qc Qd Qs Qh 4c",
            "Ts 9s 8s 7s 6s",
            "4d 4h 4s 5c",
        ),
        // Aces up top: sixteen.
        script(
            "Ac Ad 2c",
            "Jc Jd Js Jh 3c",
            "Kc Kd Ks Kh 3d",
            "2d 2h 2s 3h",
        ),
    ];
    let state = play(&OFC_PROGRESSIVE, &scripts, &[None, None]);
    let settled = settlement(&state);
    assert_eq!(settled.fouled, vec![false, false]);
    assert_eq!(settled.next_fantasyland, vec![Some(17), Some(16)]);
    assert_eq!(settled.royalties, vec![51, 39]);
    assert_eq!(settled.points, vec![18, -18]);
    assert_eq!(settled.scoops, vec![1, 0]);
}

/// Seat 0 for the progressive top-row fixtures: no queens, kings or aces of
/// its own to collide with the seat under test.
fn progressive_neutral_seat() -> Script {
    script(
        "2d 3d 4d",
        "5s 6s 8d Ts Js",
        "Kh Ks Qh Qs 3s",
        "3c 4c 5c 6c",
    )
}

#[test]
fn progressive_queens_and_kings_earn_fourteen_and_fifteen() {
    let queens = vec![
        progressive_neutral_seat(),
        script(
            "Qc Qd 2h",
            "9c 9d 9h 9s Kc",
            "Ac Ad Ah As Kd",
            "2c 7c 8c 5h",
        ),
    ];
    let state = play(&OFC_PROGRESSIVE, &queens, &[None, None]);
    assert_eq!(
        settlement(&state).next_fantasyland,
        vec![None, Some(14)],
        "queens pay fourteen"
    );

    let kings = vec![
        progressive_neutral_seat(),
        script(
            "Kc Kd 2h",
            "9c 9d 9h 9s Qc",
            "Ac Ad Ah As Jc",
            "2c 7c 8c 5h",
        ),
    ];
    let state = play(&OFC_PROGRESSIVE, &kings, &[None, None]);
    assert_eq!(
        settlement(&state).next_fantasyland,
        vec![None, Some(15)],
        "kings pay fifteen"
    );
}

#[test]
fn a_progressive_stay_grants_the_base_fourteen() {
    let scripts = fantasyland_scripts(fantasyland_stay_hand("2c 3h 4h 5h"), "3c 4c 5c 6c");
    let state = play(&OFC_PROGRESSIVE, &scripts, &[None, Some(17)]);
    // Trip sevens stay — and a stay is always the base count, never the
    // seventeen that trips would have earned from outside fantasyland.
    assert_eq!(settlement(&state).next_fantasyland, vec![None, Some(14)]);
}

// ---- scripted hands: ofc-27 -------------------------------------------

/// Seat 0 for the 2-7 fixtures: a nine-low middle worth one royalty point.
fn deuce_open_seat() -> Script {
    script(
        "2s 3h 4s",
        "9d 8s 6h 4d 3d",
        "Kh Ks Qh Qs Qd",
        "5h 5s 7h 7s",
    )
}

#[test]
fn an_unqualified_deuce_middle_fouls_the_hand() {
    let scripts = vec![
        script(
            "2d 3d 4d",
            "Th 9h 8s 7h 5h",
            "Kd Ks Kh Qd Qs",
            "2h 2s 3h 3s",
        ),
        // A jack-high middle misses the ten-low qualifier.
        script(
            "2c 3c 4c",
            "Jc 9d 8h 7s 5c",
            "Ac Ad Ah As Kc",
            "6c 6d 6h 6s",
        ),
    ];
    let state = play(&OFC_27, &scripts, &[None, None]);
    let settled = settlement(&state);
    assert_eq!(settled.fouled, vec![false, true]);
    assert_eq!(settled.royalties, vec![6, 0]);
    assert_eq!(settled.points, vec![12, -12]);
    assert_eq!(settled.next_fantasyland, vec![None, None]);
}

#[test]
fn a_wheel_middle_and_kings_up_top_earn_fifteen() {
    let scripts = vec![
        deuce_open_seat(),
        // Kings up top *and* the 7-5-4-3-2 middle: both conditions at once.
        script(
            "Kc Kd 2h",
            "7c 5d 4h 3s 2c",
            "Ac Ad Ah As 9s",
            "6c 6d 6s 8c",
        ),
    ];
    let state = play(&OFC_27, &scripts, &[None, None]);
    let settled = settlement(&state);
    assert_eq!(settled.fouled, vec![false, false]);
    // Seat 0 earns one for its nine-low middle; seat 1 earns eight for the
    // wheel on top of its kings and quad aces.
    assert_eq!(settled.royalties, vec![7, 26]);
    assert_eq!(settled.next_fantasyland, vec![None, Some(15)]);
    assert_eq!(settled.points, vec![-25, 25]);
    assert_eq!(settled.scoops, vec![0, 1]);
}

#[test]
fn a_wheel_middle_alone_earns_fourteen() {
    let scripts = vec![
        deuce_open_seat(),
        // Nines up top: only the wheel qualifies.
        script(
            "9c 9h 2h",
            "7c 5d 4h 3s 2c",
            "Ac Ad Ah As 9s",
            "6c 6d 6s 8c",
        ),
    ];
    let state = play(&OFC_27, &scripts, &[None, None]);
    assert_eq!(settlement(&state).next_fantasyland, vec![None, Some(14)]);
}

#[test]
fn deuce_kings_alone_earn_fourteen() {
    let scripts = vec![
        deuce_open_seat(),
        // Kings up top with an ordinary eight-low middle, worth two.
        script(
            "Kc Kd 2h",
            "8h 7c 5c 4c 3c",
            "Ac Ad Ah As 9s",
            "6c 6d 6s 8c",
        ),
    ];
    let state = play(&OFC_27, &scripts, &[None, None]);
    let settled = settlement(&state);
    assert_eq!(settled.fouled, vec![false, false]);
    assert_eq!(settled.royalties, vec![7, 20]);
    assert_eq!(settled.next_fantasyland, vec![None, Some(14)]);
}

#[test]
fn a_deuce_fantasyland_stays_on_trips_or_bottom_quads_only() {
    // Trip jacks up top: a stay.
    let stay = script("Jc Jd Jh", "Th 9c 8c 6c 4c", "Ac Ad Ah As Kd", "2c");
    let state = play(&OFC_27, &[deuce_open_seat(), stay], &[None, Some(14)]);
    assert_eq!(settlement(&state).next_fantasyland, vec![None, Some(14)]);

    // Kings up top and no quads at the bottom: an entry condition, but never
    // a stay.
    let no_stay = script("Kc Kd 2h", "Th 9c 8c 6c 4c", "Ac Ad Ah Jd Js", "2c");
    let state = play(&OFC_27, &[deuce_open_seat(), no_stay], &[None, Some(14)]);
    let settled = settlement(&state);
    assert_eq!(settled.fouled, vec![false, false]);
    assert_eq!(settled.next_fantasyland, vec![None, None]);
}

// ---- placement legality ------------------------------------------------

fn open_hand(spec: &OfcSpec) -> OfcHandState {
    let deck = Deck::from_deal_order(
        &(0..52u8)
            .rev()
            .map(|i| Card::from_index(i).unwrap())
            .collect::<Vec<_>>(),
    );
    OfcHandState::new(spec, 2, &[None, None], 1, deck)
        .unwrap()
        .0
}

fn outsider(dealt: &[Card]) -> Card {
    (0..52u8)
        .map(|i| Card::from_index(i).unwrap())
        .find(|c| !dealt.contains(c))
        .unwrap()
}

#[test]
fn a_placement_must_account_for_exactly_the_cards_just_dealt() {
    let mut state = open_hand(&OFC_PINEAPPLE);
    let request = state.request().unwrap();
    let legal = filler(&state.boards()[request.seat], &request);

    let mut short = legal.clone();
    short.placements.pop();
    assert!(matches!(
        state.apply(&short),
        Err(OfcError::WrongPlacementCount { .. })
    ));

    let mut duplicated = legal.clone();
    duplicated.placements[1].card = duplicated.placements[0].card;
    assert!(matches!(
        state.apply(&duplicated),
        Err(OfcError::DuplicateCard { .. })
    ));

    let mut foreign = legal.clone();
    foreign.placements[0].card = outsider(&request.dealt);
    assert!(matches!(
        state.apply(&foreign),
        Err(OfcError::CardNotDealt { .. })
    ));

    let mut overfull = legal.clone();
    for placement in &mut overfull.placements {
        placement.row = Row::Top;
    }
    assert!(matches!(
        state.apply(&overfull),
        Err(OfcError::RowFull { row: Row::Top, .. })
    ));

    // None of the rejections changed anything.
    assert_eq!(state.request(), Some(request));
    assert!(state.boards()[0].is_empty() && state.boards()[1].is_empty());
    assert!(state.apply(&legal).is_ok());
}

#[test]
fn discards_must_be_exactly_the_unplaced_cards() {
    let mut state = open_hand(&OFC_PINEAPPLE);
    // Past the two opening turns, which have no discards.
    for _ in 0..2 {
        let request = state.request().unwrap();
        let action = filler(&state.boards()[request.seat], &request);
        state.apply(&action).unwrap();
    }
    let request = state.request().unwrap();
    assert_eq!((request.place, request.discard), (2, 1));

    let legal = filler(&state.boards()[request.seat], &request);
    let mut no_discard = legal.clone();
    no_discard.discards.clear();
    assert!(matches!(
        state.apply(&no_discard),
        Err(OfcError::WrongDiscardCount { .. })
    ));

    let mut discards_a_placed_card = legal.clone();
    discards_a_placed_card.discards[0] = legal.placements[0].card;
    assert!(matches!(
        state.apply(&discards_a_placed_card),
        Err(OfcError::DuplicateCard { .. })
    ));

    assert!(state.apply(&legal).is_ok());
}

#[test]
fn a_settled_hand_accepts_nothing_more() {
    let mut state = play(&OFC, &ofc_clean_scripts(), &[None, None]);
    assert_eq!(state.to_act(), None);
    assert_eq!(state.request(), None);
    assert!(matches!(
        state.apply(&OfcAction {
            placements: Vec::new(),
            discards: Vec::new(),
        }),
        Err(OfcError::NoPendingDecision)
    ));
}

#[test]
fn setup_validates_seats_and_fantasyland_counts() {
    let deck = Deck::standard();
    assert!(matches!(
        OfcHandState::new(&OFC_PINEAPPLE, 4, &[None; 4], 1, deck.clone()),
        Err(OfcError::BadSeatCount(4))
    ));
    assert!(matches!(
        OfcHandState::new(&OFC, 1, &[None], 1, deck.clone()),
        Err(OfcError::BadSeatCount(1))
    ));
    assert!(matches!(
        OfcHandState::new(&OFC, 2, &[None], 1, deck.clone()),
        Err(OfcError::BadFantasylandLen { got: 1, seats: 2 })
    ));
    assert!(matches!(
        OfcHandState::new(&OFC, 2, &[Some(12), None], 1, deck.clone()),
        Err(OfcError::BadFantasylandCount { cards: 12, .. })
    ));
    // Classic OFC deals thirteen cards a seat, so seventeen cannot be dealt.
    assert!(matches!(
        OfcHandState::new(&OFC, 2, &[Some(17), None], 1, deck.clone()),
        Err(OfcError::BadFantasylandCount { cards: 17, .. })
    ));
    assert!(OfcHandState::new(&OFC_PINEAPPLE, 3, &[None, Some(17), None], 1, deck).is_ok());
}

// ---- property sweeps ---------------------------------------------------

/// Play a whole hand with the filler, from a shuffled deck.
fn sweep_hand(spec: &OfcSpec, seats: usize, fantasyland: &[Option<u8>], seed: u64) -> OfcHandState {
    let mut rng = Rng64::from_seed_stream(seed, 0);
    let deck = Deck::shuffled(&mut rng);
    let (mut state, _) = OfcHandState::new(spec, seats, fantasyland, seed, deck).unwrap();
    let mut turns = 0;
    while let Some(request) = state.request() {
        let action = filler(&state.boards()[request.seat], &request);
        state.apply(&action).expect("the filler is always legal");
        turns += 1;
        assert!(turns < 100, "hand did not terminate");
    }
    state
}

/// Every seat count and fantasyland shape a variant admits.
fn sweep_configs(spec: &OfcSpec) -> Vec<(usize, Vec<Option<u8>>)> {
    let mut out = Vec::new();
    for seats in spec.seats() {
        out.push((seats, vec![None; seats]));
        let mut one = vec![None; seats];
        one[0] = Some(spec.cards_per_seat().clamp(13, 14));
        out.push((seats, one));
        out.push((seats, vec![Some(13); seats]));
    }
    out
}

#[test]
fn every_variant_terminates_and_settles_zero_sum() {
    for spec in poker_core::ofc::registry() {
        for (seats, fantasyland) in sweep_configs(spec) {
            for seed in 0..12u64 {
                let state = sweep_hand(spec, seats, &fantasyland, seed);
                let settled = settlement(&state);
                assert_eq!(
                    settled.points.iter().sum::<i64>(),
                    0,
                    "{} seats={seats} seed={seed}",
                    spec.id
                );
                assert_eq!(settled.points.len(), seats);
                assert!(state.boards().iter().all(Board::is_complete));
                assert!(matches!(
                    state.events().last(),
                    Some(OfcEvent::Score { .. })
                ));
            }
        }
    }
}

#[test]
fn dealt_cards_are_conserved_across_boards_and_discards() {
    for spec in poker_core::ofc::registry() {
        for (seats, fantasyland) in sweep_configs(spec) {
            for seed in 0..8u64 {
                let state = sweep_hand(spec, seats, &fantasyland, seed);

                let mut dealt: Vec<Vec<Card>> = vec![Vec::new(); seats];
                for ev in state.events() {
                    if let OfcEvent::Deal { seat, cards, count } = ev {
                        assert_eq!(cards.len(), *count as usize);
                        dealt[*seat].extend_from_slice(cards);
                    }
                }

                let mut all = HashSet::new();
                for seat in 0..seats {
                    let expected = fantasyland[seat].unwrap_or(spec.cards_per_seat()) as usize;
                    assert_eq!(dealt[seat].len(), expected, "{} seat {seat}", spec.id);

                    let mut held = state.boards()[seat].cards();
                    held.extend_from_slice(&state.discarded()[seat]);
                    assert_eq!(held.len(), expected);

                    let mut lhs = dealt[seat].clone();
                    let mut rhs = held.clone();
                    lhs.sort_by_key(|c| c.index());
                    rhs.sort_by_key(|c| c.index());
                    assert_eq!(lhs, rhs, "{} seat {seat} lost or invented a card", spec.id);

                    for card in held {
                        assert!(all.insert(card), "{card} appeared twice");
                    }
                }
            }
        }
    }
}

#[test]
fn random_legal_placements_are_always_accepted() {
    for spec in poker_core::ofc::registry() {
        for seed in 0..24u64 {
            let mut rng = Rng64::from_seed_stream(seed, 7);
            let seats = spec.min_seats;
            let deck = Deck::shuffled(&mut rng);
            let (mut state, _) =
                OfcHandState::new(spec, seats, &vec![None; seats], seed, deck).unwrap();
            while let Some(request) = state.request() {
                let action = random_action(&state.boards()[request.seat], &request, &mut rng);
                state
                    .apply(&action)
                    .unwrap_or_else(|e| panic!("{} rejected a legal action: {e}", spec.id));
            }
            assert!(state.settlement().is_some());
        }
    }
}

#[test]
fn mutated_placements_are_always_rejected() {
    for spec in poker_core::ofc::registry() {
        for seed in 0..8u64 {
            let mut rng = Rng64::from_seed_stream(seed, 11);
            let seats = spec.min_seats;
            let deck = Deck::shuffled(&mut rng);
            let (mut state, _) =
                OfcHandState::new(spec, seats, &vec![None; seats], seed, deck).unwrap();
            while let Some(request) = state.request() {
                let legal = random_action(&state.boards()[request.seat], &request, &mut rng);

                let mut short = legal.clone();
                short.placements.pop();
                assert!(state.apply(&short).is_err());

                let mut foreign = legal.clone();
                foreign.placements[0].card = outsider(&request.dealt);
                assert!(state.apply(&foreign).is_err());

                if legal.placements.len() >= 2 {
                    let mut duplicated = legal.clone();
                    duplicated.placements[1].card = duplicated.placements[0].card;
                    assert!(state.apply(&duplicated).is_err());
                }

                if !legal.discards.is_empty() {
                    let mut clash = legal.clone();
                    clash.discards[0] = legal.placements[0].card;
                    assert!(state.apply(&clash).is_err());
                }

                state.apply(&legal).expect("the legal action still stands");
            }
        }
    }
}

#[test]
fn the_same_deck_and_placements_produce_identical_event_bytes() {
    let bytes = |state: &OfcHandState| {
        state
            .events()
            .iter()
            .map(|ev| serde_json::to_string(ev).unwrap())
            .collect::<Vec<_>>()
            .join("\n")
    };
    for spec in poker_core::ofc::registry() {
        for (seats, fantasyland) in sweep_configs(spec) {
            for seed in 0..4u64 {
                let first = sweep_hand(spec, seats, &fantasyland, seed);
                let second = sweep_hand(spec, seats, &fantasyland, seed);
                assert_eq!(bytes(&first), bytes(&second), "{}", spec.id);
                assert_eq!(
                    settlement(&first).points,
                    settlement(&second).points,
                    "{}",
                    spec.id
                );
            }
        }
    }
}

#[test]
fn every_per_seat_iteration_walks_table_order() {
    for spec in poker_core::ofc::registry() {
        for seats in spec.seats() {
            let state = sweep_hand(spec, seats, &vec![None; seats], 3);
            let order: Vec<usize> = table_order(seats).collect();

            let of = |pick: fn(&OfcEvent) -> Option<usize>| -> Vec<usize> {
                state.events().iter().filter_map(pick).collect()
            };
            let showdowns = of(|ev| match ev {
                OfcEvent::Showdown { seat, .. } => Some(*seat),
                _ => None,
            });
            let scores = of(|ev| match ev {
                OfcEvent::Score { seat, .. } => Some(*seat),
                _ => None,
            });
            let places = of(|ev| match ev {
                OfcEvent::Place { seat, .. } => Some(*seat),
                _ => None,
            });

            assert_eq!(showdowns, order);
            assert_eq!(scores, order);
            for chunk in places.chunks(seats) {
                assert_eq!(chunk, order.as_slice());
            }
        }
    }
}

// ---- fantasyland stay schedules ----

#[test]
fn a_middle_full_house_stays_only_in_classic_ofc() {
    // Middle full house, top safely below it, bottom above it but short of
    // quads — so the middle is the only possible stay condition.
    let board = board_of("Kc Qd 2h", "8c 8d 8h 2c 2d", "Ac Ad Ah Kd Ks");
    for (spec, expected) in [
        (&OFC, Some(13)),
        (&OFC_PINEAPPLE, None),
        (&OFC_PROGRESSIVE, None),
    ] {
        let ev = score::evaluate(spec, &board);
        assert!(!ev.fouled, "{}: fixture must not foul", spec.id);
        assert_eq!(
            score::fantasyland(spec, &board, &ev, true),
            expected,
            "{}: middle full house stay",
            spec.id
        );
        // The same boards stay everywhere once the bottom reaches quads.
        let quads = board_of("Kc Qd 2h", "8c 8d 8h 2c 2d", "Ac Ad Ah As Ks");
        let ev = score::evaluate(spec, &quads);
        assert_eq!(
            score::fantasyland(spec, &quads, &ev, true),
            Some(spec.fantasyland_base()),
            "{}: bottom quads stay",
            spec.id
        );
    }
}

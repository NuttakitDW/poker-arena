//! Scenario tests for the Omaha family of variants (`omaha-pl`, `omaha8-pl`,
//! `omaha8-fl`): the `ExactlyTwo` hole-usage constraint, hi-lo settlement,
//! and pot-limit sizing with four hole cards.
//!
//! Decks are scripted with [`Deck::from_deal_order`], exactly as in
//! `hand.rs`: the engine deals hole cards one full batch per seat (in seat
//! order starting left of the button), then one community batch per street.
//! `deck_for` builds decks in exactly that order; its `holes` slice is
//! indexed by *seat number* (`holes[0]` is seat 0's cards), not deal order —
//! the function itself reorders for dealing.

use poker_core::card::{Deck, parse_cards};
use poker_core::eval::{self, HandClass};
use poker_core::game::action::{Action, BetBounds, Chips, Seat};
use poker_core::game::event::{Event, PotSide};
use poker_core::game::spec::{GameSpec, Stakes};
use poker_core::game::state::{ActionError, HandState};

const STAKES: Stakes = Stakes {
    small_blind: 50,
    big_blind: 100,
};

fn omaha_pl_spec() -> GameSpec {
    GameSpec::omaha_pl(STAKES)
}

fn omaha8_pl_spec() -> GameSpec {
    GameSpec::omaha8_pl(STAKES)
}

fn omaha8_fl_spec() -> GameSpec {
    GameSpec::omaha8_fl(STAKES)
}

/// Deck dealing `holes[seat]` to each seat and then `board`, in engine
/// order. `holes` is indexed by seat number; the function reorders for
/// dealing (left of button first).
fn deck_for(button: Seat, holes: &[&str], board: &str) -> Deck {
    let n = holes.len();
    let mut cards = Vec::new();
    for i in 1..=n {
        cards.extend(parse_cards(holes[(button + i) % n]).unwrap());
    }
    cards.extend(parse_cards(board).unwrap());
    Deck::from_deal_order(&cards)
}

fn cards(s: &str) -> Vec<poker_core::card::Card> {
    parse_cards(s).unwrap()
}

fn awarded(pot: u8, winners: &[(Seat, Chips)]) -> Event {
    Event::PotAwarded {
        pot,
        side: PotSide::Whole,
        winners: winners.to_vec(),
    }
}

fn awarded_side(pot: u8, side: PotSide, winners: &[(Seat, Chips)]) -> Event {
    Event::PotAwarded {
        pot,
        side,
        winners: winners.to_vec(),
    }
}

/// Apply a script of actions, asserting each is accepted.
fn play(hand: &mut HandState, actions: &[Action]) -> Vec<Event> {
    let mut all = Vec::new();
    for a in actions {
        all.extend(
            hand.apply(a.clone())
                .unwrap_or_else(|e| panic!("action {a:?} rejected: {e}")),
        );
    }
    all
}

fn assert_conserved(hand: &HandState) {
    let settle = hand.settlement().expect("hand must be settled");
    assert_eq!(settle.nets.iter().sum::<i64>(), 0, "nets must sum to zero");
    assert!(matches!(hand.events().last(), Some(Event::HandEnd { .. })));
    if let Some(Event::HandEnd { nets }) = hand.events().last() {
        assert_eq!(nets, &settle.nets, "HandEnd nets must match settlement");
    }
}

/// Every `ShowdownShow` hi value, keyed by seat.
fn hi_values(events: &[Event]) -> Vec<(Seat, eval::HandValue)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::ShowdownShow {
                seat, hi: Some(v), ..
            } => Some((*seat, *v)),
            _ => None,
        })
        .collect()
}

// --- 1. ExactlyTwo blocks a board flush a single hole spade can't join ---

#[test]
fn exactly_two_blocks_board_flushes() {
    // Board carries four spades. Seat 0 holds only the ace of spades among
    // its four hole cards (plus a strong pocket-aces-and-kings pair); seat 1
    // holds two low spades. Under `HoleUsage::Any`, seat 0's lone spade plus
    // four board spades would make an ace-high flush and seat 0 would win —
    // but Omaha's `ExactlyTwo` rule forbids using only one hole card for a
    // flush, so seat 0 can never complete one, and seat 1's much smaller
    // flush wins outright despite seat 0 holding the higher spade.
    let holes = ["As Ac Kd Kc", "4s 6s 2d 3c"];
    let deck = deck_for(0, &holes, "2s 5s 8s Js 3d");
    let (mut hand, _) = HandState::new(&omaha_pl_spec(), &[10_000, 10_000], 0, 1, deck).unwrap();

    play(&mut hand, &[Action::Call, Action::Check]); // preflop
    play(&mut hand, &[Action::Check, Action::Check]); // flop
    play(&mut hand, &[Action::Check, Action::Check]); // turn
    let end = play(&mut hand, &[Action::Check, Action::Check]); // river

    let hi = hi_values(&end);
    assert_eq!(
        hi.iter().find(|(s, _)| *s == 0).unwrap().1.high_class(),
        HandClass::OnePair,
        "seat 0 can only pair aces; it never sees a flush"
    );
    assert_eq!(
        hi.iter().find(|(s, _)| *s == 1).unwrap().1.high_class(),
        HandClass::Flush
    );

    let awards: Vec<&Event> = end
        .iter()
        .filter(|e| matches!(e, Event::PotAwarded { .. }))
        .collect();
    assert_eq!(awards, vec![&awarded(0, &[(1, 200)])]);
    assert_eq!(hand.settlement().unwrap().nets, vec![-100, 100]);
    assert_conserved(&hand);
}

// --- 2. ExactlyTwo must search all C(4,2) hole combinations -------------

#[test]
fn exactly_two_uses_best_two_of_four() {
    // Seat 0 holds a pocket pair of nines plus two irrelevant blanks; only
    // the pair combines with the board's nine to make trips. Seat 1 holds
    // two unpaired high cards that both pair with the board for two pair.
    // The evaluator must search every hole pairing (not just "the first
    // two cards" or "all four") to find seat 0's trips.
    let holes = ["9h 9d 2c 3d", "Kc Qc 2h 3h"];
    let deck = deck_for(0, &holes, "9c Kd Qh 4s 5c");
    let (mut hand, _) = HandState::new(&omaha_pl_spec(), &[10_000, 10_000], 0, 1, deck).unwrap();

    play(&mut hand, &[Action::Call, Action::Check]);
    play(&mut hand, &[Action::Check, Action::Check]);
    play(&mut hand, &[Action::Check, Action::Check]);
    let end = play(&mut hand, &[Action::Check, Action::Check]);

    let hi = hi_values(&end);
    let hi0 = hi.iter().find(|(s, _)| *s == 0).unwrap().1;
    let hi1 = hi.iter().find(|(s, _)| *s == 1).unwrap().1;

    // Cross-check against the evaluator directly on the exact five cards
    // each seat's best `ExactlyTwo` selection must use.
    let expected0 = eval::high(&cards("9h 9d 9c Kd Qh"));
    let expected1 = eval::high(&cards("Kc Qc Kd Qh 9c"));
    assert_eq!(
        hi0, expected0,
        "must find trips via the 9h/9d hole pair, not the 2c/3d blanks"
    );
    assert_eq!(hi1, expected1);
    assert_eq!(hi0.high_class(), HandClass::Trips);
    assert_eq!(hi1.high_class(), HandClass::TwoPair);
    assert!(hi0 > hi1);

    assert_eq!(hand.settlement().unwrap().nets, vec![100, -100]);
    assert_conserved(&hand);
}

// --- 3. The board can never play alone -----------------------------------

#[test]
fn board_never_plays_alone() {
    // The board itself is a complete broadway straight (T-J-Q-K-A). Seat 0
    // holds only 2233 — under `HoleUsage::Any` (impossible in real Omaha,
    // but the point of this test) that seat could just "play the board"
    // and tie the straight; `ExactlyTwo` forbids using fewer than two hole
    // cards, so seat 0 is stuck with one pair. Seat 1 holds a pair of
    // straight-completing broadway cards (Ts, Ks) that combine with three
    // board cards to make the identical straight legitimately, and wins
    // outright — a single winner, not a chop with the board.
    let holes = ["2c 2d 3c 3d", "Ts Ks 4h 5d"];
    let deck = deck_for(0, &holes, "Th Jd Qc Kh As");
    let (mut hand, _) = HandState::new(&omaha_pl_spec(), &[10_000, 10_000], 0, 1, deck).unwrap();

    play(&mut hand, &[Action::Call, Action::Check]);
    play(&mut hand, &[Action::Check, Action::Check]);
    play(&mut hand, &[Action::Check, Action::Check]);
    let end = play(&mut hand, &[Action::Check, Action::Check]);

    let hi = hi_values(&end);
    assert_eq!(
        hi.iter().find(|(s, _)| *s == 0).unwrap().1.high_class(),
        HandClass::OnePair,
        "seat 0 cannot play the board's straight through ExactlyTwo"
    );
    assert_eq!(
        hi.iter().find(|(s, _)| *s == 1).unwrap().1.high_class(),
        HandClass::Straight
    );

    let awards: Vec<&Event> = end
        .iter()
        .filter(|e| matches!(e, Event::PotAwarded { .. }))
        .collect();
    // A single winner: the non-split outcome, not a board-tie chop.
    assert_eq!(awards, vec![&awarded(0, &[(1, 200)])]);
    assert_eq!(hand.settlement().unwrap().nets, vec![-100, 100]);
    assert_conserved(&hand);
}

// --- 4. Omaha8: scoop + quartered low ------------------------------------

#[test]
fn omaha8_split_and_quarter() {
    // Three-handed omaha8-fl. Seats 0 and 1 both hold the A-2 "wheel"
    // cards; combined with the board's only three low-eligible cards
    // (3-4-8) they make the identical A-2-3-4-8 low, so the low half of the
    // pot ties and splits between them. Seat 0's other two hole cards (JJ)
    // beat seat 1's (99) and seat 2's (TT) for hi, so seat 0 scoops the hi
    // half outright *and* collects half the low half — the classic
    // "quarter": seat 1 put in a full share but recovers only 25% of the
    // pot back.
    let holes = [
        "As 2s Jc Jd", // seat 0: scoops hi, ties low
        "Ah 2h 9c 9d", // seat 1: ties low, quartered
        "Th Tc 9h 9s", // seat 2: no qualifying low, worse hi
    ];
    let deck = deck_for(0, &holes, "3c 4d 8h Kc Qd");
    let (mut hand, _) =
        HandState::new(&omaha8_fl_spec(), &[10_000, 10_000, 10_000], 0, 1, deck).unwrap();

    play(&mut hand, &[Action::Call, Action::Call, Action::Check]); // preflop
    play(&mut hand, &[Action::Check, Action::Check, Action::Check]); // flop
    play(&mut hand, &[Action::Check, Action::Check, Action::Check]); // turn
    let end = play(&mut hand, &[Action::Check, Action::Check, Action::Check]); // river

    assert_eq!(hand.pot_total(), 300);

    // Hi order is seat 0 (pair of jacks) > seat 2 (pair of tens) > seat 1
    // (pair of nines); seat 0's pair is still the best of the three, which
    // is all the scoop needs.
    let hi = hi_values(&end);
    assert_eq!(
        hi.iter().find(|(s, _)| *s == 0).unwrap().1.high_class(),
        HandClass::OnePair
    );
    assert!(
        hi.iter().find(|(s, _)| *s == 0).unwrap().1 > hi.iter().find(|(s, _)| *s == 2).unwrap().1
    );
    assert!(
        hi.iter().find(|(s, _)| *s == 2).unwrap().1 > hi.iter().find(|(s, _)| *s == 1).unwrap().1
    );

    let awards: Vec<&Event> = end
        .iter()
        .filter(|e| matches!(e, Event::PotAwarded { .. }))
        .collect();
    assert_eq!(
        awards,
        vec![
            &awarded_side(0, PotSide::Hi, &[(0, 150)]),
            &awarded_side(0, PotSide::Lo, &[(0, 75), (1, 75)]),
        ]
    );
    assert_eq!(hand.settlement().unwrap().nets, vec![125, -25, -100]);
    assert_conserved(&hand);
}

// --- 5. Omaha8: no qualifier scoops ---------------------------------------

#[test]
fn omaha8_no_qualifier_scoops() {
    // The board carries only two low-eligible cards (5, 7); any three-card
    // board subset therefore includes at least one card ranked above eight,
    // so no seat can ever complete a qualifying eight-or-better low
    // regardless of hole cards. The pot must go whole to the best hi hand.
    let holes = ["Ah 2d 9c 9s", "3h 4c Td Ts"];
    let deck = deck_for(0, &holes, "5c 7d Kc Qh Jd");
    let (mut hand, _) = HandState::new(&omaha8_pl_spec(), &[10_000, 10_000], 0, 1, deck).unwrap();

    play(&mut hand, &[Action::Call, Action::Check]);
    play(&mut hand, &[Action::Check, Action::Check]);
    play(&mut hand, &[Action::Check, Action::Check]);
    let end = play(&mut hand, &[Action::Check, Action::Check]);

    // Nobody shows a low.
    assert!(
        end.iter()
            .all(|e| !matches!(e, Event::ShowdownShow { lo: Some(_), .. }))
    );

    let awards: Vec<&Event> = end
        .iter()
        .filter(|e| matches!(e, Event::PotAwarded { .. }))
        .collect();
    assert_eq!(awards, vec![&awarded(0, &[(1, 200)])]); // seat 1's pair of tens beats seat 0's nines
    assert_eq!(hand.settlement().unwrap().nets, vec![-100, 100]);
    assert_conserved(&hand);
}

// --- 6. Pot-limit bounds with four hole cards -----------------------------

#[test]
fn pot_limit_bounds_with_four_hole_cards() {
    // Heads-up omaha-pl, sb 50 / bb 100. Preflop, before the button (seat
    // 0) acts: to_call = 50, to_call_total = 100, pot_before = 150,
    // pot_after_call = 200, max_to = to_call_total + pot_after_call = 300.
    let (mut hand, _) = HandState::new(
        &omaha_pl_spec(),
        &[100_000, 100_000],
        0,
        1,
        Deck::standard(),
    )
    .unwrap();

    let la = hand.legal_actions().unwrap();
    assert_eq!(la.call, Some(50));
    assert_eq!(
        la.raise,
        Some(BetBounds {
            min_to: 200,
            max_to: 300
        })
    );
    assert!(matches!(
        hand.apply(Action::Raise { to: 301 }),
        Err(ActionError::Illegal { .. })
    ));

    // Raise to the full pot (300); the pot is now 400.
    hand.apply(Action::Raise { to: 300 }).unwrap();
    assert_eq!(hand.pot_total(), 400);

    // Seat 1: to_call = 200, to_call_total = 300, pot_before = 400,
    // pot_after_call = 600, max_to = 300 + 600 = 900.
    let la = hand.legal_actions().unwrap();
    assert_eq!(la.call, Some(200));
    assert_eq!(
        la.raise,
        Some(BetBounds {
            min_to: 500,
            max_to: 900
        })
    );

    // Seat 1 raises to the new pot cap (900); the pot is now 1200.
    hand.apply(Action::Raise { to: 900 }).unwrap();
    assert_eq!(hand.pot_total(), 1_200);

    // Seat 0: to_call = 600, to_call_total = 900, pot_before = 1200,
    // pot_after_call = 1800, max_to = 900 + 1800 = 2700 — the cap keeps
    // growing with the pot on every street.
    let la = hand.legal_actions().unwrap();
    assert_eq!(la.call, Some(600));
    assert_eq!(
        la.raise,
        Some(BetBounds {
            min_to: 1_500,
            max_to: 2_700
        })
    );
    assert!(matches!(
        hand.apply(Action::Raise { to: 2_701 }),
        Err(ActionError::Illegal { .. })
    ));
}

// --- 7. Deck sizing at 9 seats ---------------------------------------------

#[test]
fn deck_sizing() {
    // Omaha deals 4 hole cards per seat: 9 seats need 4*9 + 5 = 41 cards,
    // comfortably within a standard 52-card deck. `cards_needed` is a
    // private helper in `state.rs`; this exercises the same bound
    // indirectly by actually completing a 9-seat hand with a standard
    // (unscripted) deck — `HandState::new` would reject an undersized deck
    // with `HandError::DeckExhausted` before any of this runs.
    let stacks = vec![10_000; 9];
    let (mut hand, _) = HandState::new(&omaha_pl_spec(), &stacks, 0, 1, Deck::standard()).unwrap();

    while hand.to_act().is_some() {
        let la = hand.legal_actions().unwrap();
        let action = if la.check {
            Action::Check
        } else {
            Action::Call
        };
        hand.apply(action).unwrap();
    }

    assert!(hand.is_over());
    assert_eq!(hand.settlement().unwrap().showdown_seats.len(), 9);
    assert_conserved(&hand);
}

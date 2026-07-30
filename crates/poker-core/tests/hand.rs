//! Scenario tests for the hand state machine.
//!
//! Decks are scripted with [`Deck::from_deal_order`]. The engine deals hole
//! cards one full batch per seat, in seat order starting left of the button,
//! then one community batch per street — `deck_for` builds decks in exactly
//! that order.

use poker_core::card::{Deck, parse_cards};
use poker_core::eval::{EvalKind, HoleUsage};
use poker_core::game::action::{Action, BetBounds, Chips, LegalActions, Seat};
use poker_core::game::event::{Event, PostKind, PotSide};
use poker_core::game::spec::{
    BettingKind, DealSpec, FirstToAct, ForcedBets, GameSpec, ShowdownSide, ShowdownSpec, Stakes,
};
use poker_core::game::state::{ActionError, HandState};
use poker_core::rng::Rng64;

fn test_rng() -> Rng64 {
    Rng64::from_seed_stream(0, 0)
}

const STAKES: Stakes = Stakes::Blinds {
    small_blind: 50,
    big_blind: 100,
};

fn nl(seats: u8) -> GameSpec {
    let mut spec = GameSpec::holdem_nl(STAKES);
    spec.seats = 2..=seats.max(2);
    spec
}

fn fl(seats: u8) -> GameSpec {
    let mut spec = GameSpec::holdem_fl(STAKES);
    spec.seats = 2..=seats.max(2);
    spec
}

/// Deck dealing `holes[seat]` to each seat and then `board`, in engine order.
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

fn acted(seat: Seat, action: Action, street_commit: Chips, all_in: bool) -> Event {
    Event::Acted {
        seat,
        action,
        street_commit,
        all_in,
    }
}

fn awarded(pot: u8, winners: &[(Seat, Chips)]) -> Event {
    Event::PotAwarded {
        pot,
        side: PotSide::Whole,
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

// --- 1. Heads-up no-limit basics ---------------------------------------

#[test]
fn heads_up_limp_check_down_to_showdown() {
    let holes = ["Ah Ad", "Kh Kd"];
    let deck = deck_for(0, &holes, "2c 7d 9s Ts 3h");
    let (mut hand, setup) =
        HandState::new(&nl(2), &[10_000, 10_000], 0, 7, deck, test_rng()).unwrap();

    assert_eq!(
        setup,
        vec![
            Event::HandStart {
                hand_no: 7,
                button: 0,
                stacks: vec![10_000, 10_000],
            },
            Event::Post {
                seat: 0,
                kind: PostKind::SmallBlind,
                amount: 50,
                all_in: false,
            },
            Event::Post {
                seat: 1,
                kind: PostKind::BigBlind,
                amount: 100,
                all_in: false,
            },
            Event::StreetStart {
                street: 0,
                label: "preflop",
            },
            Event::DealHole {
                seat: 1,
                cards: cards("Kh Kd"),
                count: 2,
            },
            Event::DealHole {
                seat: 0,
                cards: cards("Ah Ad"),
                count: 2,
            },
        ]
    );
    // Heads-up: the button posts the small blind and acts first preflop.
    assert_eq!(hand.to_act(), Some(0));

    let pre = play(&mut hand, &[Action::Call, Action::Check]);
    assert_eq!(
        pre,
        vec![
            acted(0, Action::Call, 100, false),
            acted(1, Action::Check, 100, false),
            Event::StreetStart {
                street: 1,
                label: "flop",
            },
            Event::DealCommunity {
                street: 1,
                cards: cards("2c 7d 9s"),
            },
        ]
    );
    // Postflop the big blind acts first.
    assert_eq!(hand.to_act(), Some(1));

    play(&mut hand, &[Action::Check, Action::Check]);
    assert_eq!(hand.to_act(), Some(1));
    assert_eq!(hand.board(), &cards("2c 7d 9s Ts")[..]);
    play(&mut hand, &[Action::Check, Action::Check]);
    assert_eq!(hand.street(), (3, "river"));

    let last = play(&mut hand, &[Action::Check, Action::Check]);
    assert!(hand.is_over());
    // Showdown reveals in odd-chip order: left of the button first.
    assert!(matches!(last[2], Event::ShowdownShow { seat: 1, .. }));
    assert!(matches!(last[3], Event::ShowdownShow { seat: 0, .. }));
    assert_eq!(last[4], awarded(0, &[(0, 200)]));
    assert_eq!(
        last[5],
        Event::HandEnd {
            nets: vec![100, -100]
        }
    );
    assert_eq!(hand.settlement().unwrap().showdown_seats, vec![1, 0]);
    assert_conserved(&hand);
}

#[test]
fn heads_up_raise_fold_refunds_uncalled_bet() {
    let deck = deck_for(0, &["Ah Ad", "Kh Kd"], "2c 7d 9s Ts 3h");
    let (mut hand, _) = HandState::new(&nl(2), &[10_000, 10_000], 0, 1, deck, test_rng()).unwrap();

    let ev = play(&mut hand, &[Action::Raise { to: 300 }, Action::Fold]);
    assert_eq!(
        ev,
        vec![
            acted(0, Action::Raise { to: 300 }, 300, false),
            acted(1, Action::Fold, 100, false),
            // Uncalled 200 refunded silently; only the matched 100 each is
            // in the pot, and there is no showdown.
            awarded(0, &[(0, 200)]),
            Event::HandEnd {
                nets: vec![100, -100]
            },
        ]
    );
    assert!(hand.settlement().unwrap().showdown_seats.is_empty());
    assert_eq!(hand.stacks(), &[9_900, 9_900]);
    assert_conserved(&hand);
}

#[test]
fn heads_up_check_raise_line() {
    let deck = deck_for(0, &["Ah Ad", "Kh Kd"], "2c 7d 9s Ts 3h");
    let (mut hand, _) = HandState::new(&nl(2), &[10_000, 10_000], 0, 1, deck, test_rng()).unwrap();
    play(&mut hand, &[Action::Call, Action::Check]);

    // Flop: BB checks, button bets, BB check-raises, button calls.
    assert_eq!(hand.to_act(), Some(1));
    play(&mut hand, &[Action::Check]);
    assert_eq!(
        hand.legal_actions().unwrap().bet,
        Some(BetBounds {
            min_to: 100,
            max_to: 9_900
        })
    );
    play(&mut hand, &[Action::Bet { to: 200 }]);
    let la = hand.legal_actions().unwrap();
    assert_eq!(la.call, Some(200));
    assert_eq!(
        la.raise,
        Some(BetBounds {
            min_to: 400,
            max_to: 9_900
        })
    );
    play(&mut hand, &[Action::Raise { to: 600 }, Action::Call]);
    assert_eq!(hand.street(), (2, "turn"));
    assert_eq!(hand.pot_total(), 1_400);
    assert_eq!(hand.stacks(), &[9_300, 9_300]);
}

// --- 2. All-in preflop runs the board out -------------------------------

#[test]
fn heads_up_all_in_preflop_runs_out_immediately() {
    let deck = deck_for(0, &["Ah Ad", "Kh Kd"], "2c 7d 9s Ts 3h");
    let (mut hand, _) = HandState::new(&nl(2), &[1_000, 1_000], 0, 1, deck, test_rng()).unwrap();

    let ev = play(&mut hand, &[Action::Raise { to: 1_000 }, Action::Call]);
    assert_eq!(ev[0], acted(0, Action::Raise { to: 1_000 }, 1_000, true));
    assert_eq!(ev[1], acted(1, Action::Call, 1_000, true));
    // Remaining streets deal out with no betting rounds in between.
    assert_eq!(
        &ev[2..8],
        &[
            Event::StreetStart {
                street: 1,
                label: "flop"
            },
            Event::DealCommunity {
                street: 1,
                cards: cards("2c 7d 9s")
            },
            Event::StreetStart {
                street: 2,
                label: "turn"
            },
            Event::DealCommunity {
                street: 2,
                cards: cards("Ts")
            },
            Event::StreetStart {
                street: 3,
                label: "river"
            },
            Event::DealCommunity {
                street: 3,
                cards: cards("3h")
            },
        ]
    );
    assert_eq!(ev[10], awarded(0, &[(0, 2_000)]));
    assert_eq!(
        ev[11],
        Event::HandEnd {
            nets: vec![1_000, -1_000]
        }
    );
    assert_eq!(hand.to_act(), None);
    assert_conserved(&hand);
}

// --- 3. Short all-in does not reopen the action -------------------------

/// Seats: 0 = button/UTG (3-handed), 1 = small blind, 2 = big blind.
fn three_handed(stacks: &[Chips; 3]) -> HandState {
    let deck = deck_for(0, &["Ah Ad", "Kh Kd", "Qh Qd"], "2c 7d 9s Ts 3h");
    HandState::new(&nl(3), stacks, 0, 1, deck, test_rng())
        .unwrap()
        .0
}

#[test]
fn short_all_in_does_not_reopen_the_action() {
    // Seat 1 can reach only 400 total; seat 0 opened to 300 with a full
    // raise increment of 200, so seat 1's all-in is 100 short.
    let mut hand = three_handed(&[10_000, 400, 10_000]);
    play(&mut hand, &[Action::Raise { to: 300 }]);
    assert_eq!(hand.to_act(), Some(1));
    assert_eq!(
        hand.legal_actions().unwrap().raise,
        Some(BetBounds {
            min_to: 400,
            max_to: 400
        }),
        "a short all-in collapses the bounds"
    );
    play(&mut hand, &[Action::Raise { to: 400 }, Action::Call]);

    // Back to seat 0, who already acted at 300: call or fold only.
    assert_eq!(hand.to_act(), Some(0));
    let la = hand.legal_actions().unwrap();
    assert_eq!(
        la,
        LegalActions {
            fold: true,
            check: false,
            call: Some(100),
            bet: None,
            raise: None,
            bring_in: None,
            draw: None,
        }
    );
    assert!(matches!(
        hand.apply(Action::Raise { to: 600 }),
        Err(ActionError::Illegal { .. })
    ));
}

#[test]
fn full_raise_all_in_reopens_the_action() {
    // Same line, but seat 1 can reach 500: a full 200 increment, so the
    // action reopens and seat 0 may raise again.
    let mut hand = three_handed(&[10_000, 500, 10_000]);
    play(
        &mut hand,
        &[
            Action::Raise { to: 300 },
            Action::Raise { to: 500 },
            Action::Call,
        ],
    );
    assert_eq!(hand.to_act(), Some(0));
    assert_eq!(
        hand.legal_actions().unwrap().raise,
        Some(BetBounds {
            min_to: 700,
            max_to: 10_000
        })
    );
}

#[test]
fn short_all_in_leaves_the_min_raise_base_alone() {
    // Seat 1's 100-chip increment must not become the new raise base for
    // seat 2, who has not acted yet: min raise-to stays current_to + 200.
    let mut hand = three_handed(&[10_000, 400, 10_000]);
    play(
        &mut hand,
        &[Action::Raise { to: 300 }, Action::Raise { to: 400 }],
    );
    assert_eq!(hand.to_act(), Some(2));
    assert_eq!(
        hand.legal_actions().unwrap().raise,
        Some(BetBounds {
            min_to: 600,
            max_to: 10_000
        })
    );
}

// --- 3b. Cumulative reopening (TDA rule) --------------------------------

/// Four-handed table for the reopening scenarios. Postflop order is seat 1,
/// 2, 3, 0: the short stacks sit in the middle, so the seats that already
/// acted meet their all-ins on the way back around. Hand strength descends
/// seat 2 (aces) > seat 3 (kings) > seat 1 (queens) > seat 0 (jacks) on a
/// board that never plays.
fn four_handed(spec: &GameSpec, stacks: &[Chips; 4]) -> HandState {
    let holes = ["Jh Jd", "Qh Qd", "Ah As", "Kh Kd"];
    let deck = deck_for(0, &holes, "2c 7d 9s Ts 3h");
    HandState::new(spec, stacks, 0, 1, deck, test_rng())
        .unwrap()
        .0
}

/// Everyone in for the big blind, leaving the flop four-handed (seat 3 acts
/// first preflop, the big blind closes with its option).
fn limp_to_the_flop(hand: &mut HandState) {
    play(
        hand,
        &[Action::Call, Action::Call, Action::Call, Action::Check],
    );
}

#[test]
fn cumulative_short_all_ins_reopen_the_action() {
    // Seats 2 and 3 reach the flop with exactly 1_700 and 2_000 behind.
    let mut hand = four_handed(&nl(4), &[10_100, 10_100, 1_800, 2_100]);
    limp_to_the_flop(&mut hand);
    assert_eq!(hand.street(), (1, "flop"));
    assert_eq!(hand.stacks(), &[10_000, 10_000, 1_700, 2_000]);

    // Bet 500, two calls, raise to 1_200: the last full raise is 700.
    play(
        &mut hand,
        &[
            Action::Bet { to: 500 },
            Action::Call,
            Action::Call,
            Action::Raise { to: 1_200 },
        ],
    );
    // Seat 1 calls the raise, so from here on it has acted at a price of
    // 1_200 — a single short all-in must not give it the action back.
    assert_eq!(hand.to_act(), Some(1));
    play(&mut hand, &[Action::Call]);

    // Seat 2 shoves 1_700: 500 short of the 700 needed for a full raise.
    assert_eq!(
        hand.legal_actions().unwrap().raise,
        Some(BetBounds {
            min_to: 1_700,
            max_to: 1_700
        })
    );
    play(&mut hand, &[Action::Raise { to: 1_700 }]);
    // Seat 3 shoves 2_000: 300 more, also short on its own.
    assert_eq!(hand.to_act(), Some(3));
    assert_eq!(
        hand.legal_actions().unwrap().raise,
        Some(BetBounds {
            min_to: 2_000,
            max_to: 2_000
        })
    );
    play(&mut hand, &[Action::Raise { to: 2_000 }]);

    // Seat 0 raised to 1_200 and now faces 2_000: 800 >= 700, so the two
    // short all-ins together reopen the action, minimum 2_000 + 700.
    let reopened = Some(BetBounds {
        min_to: 2_700,
        max_to: 10_000,
    });
    assert_eq!(hand.to_act(), Some(0));
    assert_eq!(hand.legal_actions().unwrap().raise, reopened);
    play(&mut hand, &[Action::Call]);

    // Seat 1 called at 1_200 and is reopened by the same 800.
    assert_eq!(hand.to_act(), Some(1));
    assert_eq!(hand.legal_actions().unwrap().raise, reopened);
    play(&mut hand, &[Action::Raise { to: 2_700 }, Action::Call]);

    assert_eq!(hand.street(), (2, "turn"));
    play(&mut hand, &[Action::Check, Action::Check]);
    let end = play(&mut hand, &[Action::Check, Action::Check]);

    // Contributions 2_800 / 2_800 / 1_800 / 2_100 layer into three pots:
    // everyone up to 1_800, seats 0/1/3 up to 2_100, seats 0/1 above it.
    let awards: Vec<&Event> = end
        .iter()
        .filter(|e| matches!(e, Event::PotAwarded { .. }))
        .collect();
    assert_eq!(
        awards,
        vec![
            &awarded(0, &[(2, 7_200)]),
            &awarded(1, &[(3, 900)]),
            &awarded(2, &[(1, 1_400)]),
        ]
    );
    assert_eq!(
        hand.settlement().unwrap().nets,
        vec![-2_800, -1_400, 5_400, -1_200]
    );
    assert_conserved(&hand);
}

#[test]
fn single_short_all_in_still_does_not_reopen() {
    // Same line, but only one seat shoves short: 1_700 − 1_200 = 500 < 700,
    // so nobody who already acted gets the action back.
    let mut hand = four_handed(&nl(4), &[10_100, 10_100, 1_800, 2_100]);
    limp_to_the_flop(&mut hand);
    play(
        &mut hand,
        &[
            Action::Bet { to: 500 },
            Action::Call,
            Action::Call,
            Action::Raise { to: 1_200 },
            Action::Call,                // seat 1, now acted at 1_200
            Action::Raise { to: 1_700 }, // seat 2, all-in and short
            Action::Call,                // seat 3
        ],
    );

    for seat in [0, 1] {
        assert_eq!(hand.to_act(), Some(seat));
        assert_eq!(
            hand.legal_actions().unwrap(),
            LegalActions {
                fold: true,
                check: false,
                call: Some(500),
                bet: None,
                raise: None,
                bring_in: None,
                draw: None,
            }
        );
        assert!(matches!(
            hand.apply(Action::Raise { to: 2_400 }),
            Err(ActionError::Illegal { .. })
        ));
        play(&mut hand, &[Action::Call]);
    }
    assert_eq!(hand.street(), (2, "turn"));
}

#[test]
fn two_shorts_below_threshold_do_not_reopen() {
    // Shoves of 1_400 and 1_600 add up to 400 over the 1_200 price — still
    // short of the 700 full raise, so the cumulative rule stays silent.
    let mut hand = four_handed(&nl(4), &[10_100, 10_100, 1_500, 1_700]);
    limp_to_the_flop(&mut hand);
    play(
        &mut hand,
        &[
            Action::Bet { to: 500 },
            Action::Call,
            Action::Call,
            Action::Raise { to: 1_200 },
            Action::Call,                // seat 1, acted at 1_200
            Action::Raise { to: 1_400 }, // seat 2, all-in
            Action::Raise { to: 1_600 }, // seat 3, all-in
        ],
    );

    for seat in [0, 1] {
        assert_eq!(hand.to_act(), Some(seat));
        let la = hand.legal_actions().unwrap();
        assert_eq!(la.raise, None, "400 < 700 is not a reopening");
        assert_eq!(la.call, Some(400));
        play(&mut hand, &[Action::Call]);
    }
    assert_eq!(hand.street(), (2, "turn"));
}

// --- 4. Min-raise laddering ---------------------------------------------

#[test]
fn min_raise_ladders_and_rejects_undersized_raises() {
    let mut hand = three_handed(&[10_000, 10_000, 10_000]);
    assert_eq!(
        hand.legal_actions().unwrap().raise,
        Some(BetBounds {
            min_to: 200,
            max_to: 10_000
        })
    );
    // Below the minimum: rejected, and nothing observable changes.
    let events_before = hand.events().len();
    let pot_before = hand.pot_total();
    assert!(matches!(
        hand.apply(Action::Raise { to: 199 }),
        Err(ActionError::Illegal { .. })
    ));
    assert_eq!(hand.to_act(), Some(0));
    assert_eq!(hand.events().len(), events_before);
    assert_eq!(hand.pot_total(), pot_before);

    play(&mut hand, &[Action::Raise { to: 350 }]);
    // Last full raise was 250, so the ladder moves to 600.
    assert_eq!(
        hand.legal_actions().unwrap().raise,
        Some(BetBounds {
            min_to: 600,
            max_to: 10_000
        })
    );
    assert!(matches!(
        hand.apply(Action::Raise { to: 599 }),
        Err(ActionError::Illegal { .. })
    ));
    play(&mut hand, &[Action::Raise { to: 900 }]);
    assert_eq!(
        hand.legal_actions().unwrap().raise,
        Some(BetBounds {
            min_to: 1_450,
            max_to: 10_000
        })
    );
    // The all-in maximum is always accepted.
    play(&mut hand, &[Action::Raise { to: 10_000 }]);
    assert!(hand.all_in()[2]);
}

#[test]
fn postflop_opening_bet_minimum_is_the_big_blind() {
    let mut hand = three_handed(&[10_000, 10_000, 10_000]);
    play(&mut hand, &[Action::Call, Action::Call, Action::Check]);
    assert_eq!(hand.street(), (1, "flop"));
    assert_eq!(hand.to_act(), Some(1));
    let la = hand.legal_actions().unwrap();
    assert!(la.check && !la.fold && la.call.is_none() && la.raise.is_none());
    assert_eq!(
        la.bet,
        Some(BetBounds {
            min_to: 100,
            max_to: 9_900
        })
    );
    assert!(matches!(
        hand.apply(Action::Bet { to: 99 }),
        Err(ActionError::Illegal { .. })
    ));
}

// --- 5. Pot-limit maximum ------------------------------------------------

fn pot_limit(seats: u8) -> GameSpec {
    let mut spec = nl(seats);
    spec.betting = BettingKind::PotLimit;
    spec.id = "holdem-pl";
    spec
}

#[test]
fn pot_limit_maximum_follows_call_then_pot_formula() {
    let deck = deck_for(0, &["Ah Ad", "Kh Kd", "Qh Qd"], "2c 7d 9s Ts 3h");
    let (mut hand, _) = HandState::new(
        &pot_limit(3),
        &[10_000, 10_000, 10_000],
        0,
        1,
        deck,
        test_rng(),
    )
    .unwrap();

    // Pot 150, seat 0 calls 100 -> max raise-to = 100 + (150 + 100) = 350.
    assert_eq!(hand.pot_total(), 150);
    assert_eq!(
        hand.legal_actions().unwrap().raise,
        Some(BetBounds {
            min_to: 200,
            max_to: 350
        })
    );
    assert!(matches!(
        hand.apply(Action::Raise { to: 351 }),
        Err(ActionError::Illegal { .. })
    ));
    play(&mut hand, &[Action::Raise { to: 350 }]);

    // Pot 500, seat 1 calls 300 -> max = 350 + (500 + 300) = 1150.
    assert_eq!(hand.pot_total(), 500);
    assert_eq!(
        hand.legal_actions().unwrap().raise,
        Some(BetBounds {
            min_to: 600,
            max_to: 1_150
        })
    );
    play(&mut hand, &[Action::Call, Action::Call]);

    // Flop, pot 1050: an opening pot-sized bet is exactly the pot.
    assert_eq!(hand.street(), (1, "flop"));
    assert_eq!(
        hand.legal_actions().unwrap().bet,
        Some(BetBounds {
            min_to: 100,
            max_to: 1_050
        })
    );
}

#[test]
fn pot_limit_maximum_clamps_to_a_short_stack() {
    let deck = deck_for(0, &["Ah Ad", "Kh Kd", "Qh Qd"], "2c 7d 9s Ts 3h");
    // Seat 0 can only reach 250, below the 350 pot maximum.
    let (hand, _) = HandState::new(
        &pot_limit(3),
        &[250, 10_000, 10_000],
        0,
        1,
        deck,
        test_rng(),
    )
    .unwrap();
    assert_eq!(
        hand.legal_actions().unwrap().raise,
        Some(BetBounds {
            min_to: 200,
            max_to: 250
        })
    );
}

// --- 6. Fixed-limit hold'em ---------------------------------------------

#[test]
fn fixed_limit_tier_sizes_and_raise_cap() {
    let deck = deck_for(0, &["Ah Ad", "Kh Kd", "Qh Qd"], "2c 7d 9s Ts 3h");
    let (mut hand, _) =
        HandState::new(&fl(3), &[10_000, 10_000, 10_000], 0, 1, deck, test_rng()).unwrap();

    // Preflop tier = big blind; the blind itself is wager 1.
    assert_eq!(hand.legal_actions().unwrap().raise, fixed(200));
    assert!(matches!(
        hand.apply(Action::Raise { to: 250 }),
        Err(ActionError::Illegal { .. })
    ));
    play(&mut hand, &[Action::Raise { to: 200 }]); // wager 2
    assert_eq!(hand.legal_actions().unwrap().raise, fixed(300));
    play(&mut hand, &[Action::Raise { to: 300 }]); // wager 3
    play(&mut hand, &[Action::Raise { to: 400 }]); // wager 4 = cap

    // At the cap only call/fold remain.
    let la = hand.legal_actions().unwrap();
    assert_eq!(la.raise, None);
    assert_eq!(la.call, Some(200));
    assert!(la.fold);
    assert!(matches!(
        hand.apply(Action::Raise { to: 500 }),
        Err(ActionError::Illegal { .. })
    ));
    play(&mut hand, &[Action::Call, Action::Call]);

    // Flop uses the small tier, turn and river the big tier.
    assert_eq!(hand.street(), (1, "flop"));
    assert_eq!(hand.legal_actions().unwrap().bet, fixed(100));
    play(&mut hand, &[Action::Check, Action::Check, Action::Check]);
    assert_eq!(hand.street(), (2, "turn"));
    assert_eq!(hand.legal_actions().unwrap().bet, fixed(200));
    play(&mut hand, &[Action::Bet { to: 200 }]);
    assert_eq!(hand.legal_actions().unwrap().raise, fixed(400));
    play(&mut hand, &[Action::Call, Action::Call]);
    assert_eq!(hand.street(), (3, "river"));
    assert_eq!(hand.legal_actions().unwrap().bet, fixed(200));
}

#[test]
fn fixed_limit_big_blind_option() {
    let deck = deck_for(0, &["Ah Ad", "Kh Kd", "Qh Qd"], "2c 7d 9s Ts 3h");
    let (mut hand, _) =
        HandState::new(&fl(3), &[10_000, 10_000, 10_000], 0, 1, deck, test_rng()).unwrap();
    play(&mut hand, &[Action::Call, Action::Call]);
    assert_eq!(hand.to_act(), Some(2));
    let la = hand.legal_actions().unwrap();
    assert!(la.check && !la.fold && la.call.is_none());
    assert_eq!(
        la.raise,
        Some(BetBounds {
            min_to: 200,
            max_to: 200
        })
    );
    play(&mut hand, &[Action::Raise { to: 200 }]);
    assert_eq!(hand.to_act(), Some(0));
    assert_eq!(hand.legal_actions().unwrap().call, Some(100));
}

#[test]
fn fixed_limit_short_all_in_below_half_a_bet_does_not_count_toward_the_cap() {
    // Seat 0 can only add 40 to the 100 price — under half the 100 tier, so
    // it is a call-and-more and the cap is still at wager 1.
    let deck = deck_for(0, &["Ah Ad", "Kh Kd", "Qh Qd"], "2c 7d 9s Ts 3h");
    let (mut hand, _) =
        HandState::new(&fl(3), &[140, 10_000, 10_000], 0, 1, deck, test_rng()).unwrap();
    assert_eq!(
        hand.legal_actions().unwrap().raise,
        Some(BetBounds {
            min_to: 140,
            max_to: 140
        })
    );
    play(&mut hand, &[Action::Raise { to: 140 }]);
    assert!(hand.all_in()[0]);
    assert_eq!(
        hand.legal_actions().unwrap().raise,
        Some(BetBounds {
            min_to: 240,
            max_to: 240
        })
    );
    // Wagers 2, 3, 4 — the cap is only reached after three more raises.
    play(
        &mut hand,
        &[
            Action::Raise { to: 240 },
            Action::Raise { to: 340 },
            Action::Raise { to: 440 },
        ],
    );
    let la = hand.legal_actions().unwrap();
    assert_eq!(la.raise, None);
    assert_eq!(la.call, Some(100));
}

#[test]
fn fixed_limit_short_all_in_of_half_a_bet_counts_toward_the_cap() {
    // Seat 0 adds 60 to the 100 price — at least half the 100 tier, so it
    // is a full wager and consumes a cap slot.
    let deck = deck_for(0, &["Ah Ad", "Kh Kd", "Qh Qd"], "2c 7d 9s Ts 3h");
    let (mut hand, _) =
        HandState::new(&fl(3), &[160, 10_000, 10_000], 0, 1, deck, test_rng()).unwrap();
    play(
        &mut hand,
        &[
            Action::Raise { to: 160 }, // wager 2
            Action::Raise { to: 260 }, // wager 3
            Action::Raise { to: 360 }, // wager 4 = cap
        ],
    );
    let la = hand.legal_actions().unwrap();
    assert_eq!(la.raise, None);
    assert_eq!(la.call, Some(100));
}

/// Fixed-limit `to`-bounds always collapse to a single amount.
fn fixed(to: Chips) -> Option<BetBounds> {
    Some(BetBounds {
        min_to: to,
        max_to: to,
    })
}

#[test]
fn fl_sub_half_short_does_not_reopen() {
    // Flop tier 100. Seat 3 arrives with 130 behind and shoves it: 30 is
    // under half a bet, so it consumes no cap slot and reopens nobody.
    let mut hand = four_handed(&fl(4), &[10_100, 10_100, 10_100, 230]);
    limp_to_the_flop(&mut hand);
    play(&mut hand, &[Action::Bet { to: 100 }, Action::Call]);
    assert_eq!(hand.legal_actions().unwrap().raise, fixed(130));
    play(&mut hand, &[Action::Raise { to: 130 }]);

    // Seat 0 has not acted this street: it raises a full bet over the price
    // actually showing.
    assert_eq!(hand.to_act(), Some(0));
    assert_eq!(hand.legal_actions().unwrap().raise, fixed(230));
    play(&mut hand, &[Action::Call]);

    // Seat 1 bet 100 and faces 130: call the extra 30 or fold, nothing else.
    assert_eq!(hand.to_act(), Some(1));
    assert_eq!(
        hand.legal_actions().unwrap(),
        LegalActions {
            fold: true,
            check: false,
            call: Some(30),
            bet: None,
            raise: None,
            bring_in: None,
            draw: None,
        }
    );
    assert!(matches!(
        hand.apply(Action::Raise { to: 230 }),
        Err(ActionError::Illegal { .. })
    ));
}

#[test]
fn fl_half_or_more_short_reopens_with_additive_raise() {
    // Seat 3 shoves 170 over the 100 bet: 70 is at least half the 100 tier,
    // so it is a raise — cap slot 2, and the action reopens at 170 + 100.
    let mut hand = four_handed(&fl(4), &[10_100, 10_100, 10_100, 270]);
    limp_to_the_flop(&mut hand);
    play(
        &mut hand,
        &[
            Action::Bet { to: 100 }, // wager 1
            Action::Call,
            Action::Raise { to: 170 }, // wager 2, all-in and short
        ],
    );
    assert_eq!(hand.legal_actions().unwrap().raise, fixed(270));
    play(&mut hand, &[Action::Call]);

    // Seat 1 bet 100 and is reopened by the half-bet all-in.
    assert_eq!(hand.to_act(), Some(1));
    assert_eq!(hand.legal_actions().unwrap().raise, fixed(270));
    play(&mut hand, &[Action::Raise { to: 270 }]); // wager 3
    assert_eq!(hand.legal_actions().unwrap().raise, fixed(370));
    play(&mut hand, &[Action::Raise { to: 370 }]); // wager 4 = cap

    let la = hand.legal_actions().unwrap();
    assert_eq!(la.raise, None, "four wagers is the cap");
    assert_eq!(la.call, Some(200));
}

#[test]
fn fl_cumulative_half_reopen() {
    // Two all-ins of 30 and 25 over the 100 bet: neither is half a bet, so
    // neither consumes a slot or reopens on its own — but together the price
    // has risen 55 since seat 1 acted, which is half a bet or more.
    let mut hand = four_handed(&fl(4), &[255, 10_100, 10_100, 230]);
    limp_to_the_flop(&mut hand);
    play(
        &mut hand,
        &[
            Action::Bet { to: 100 }, // wager 1
            Action::Call,
            Action::Raise { to: 130 }, // seat 3, all-in, no slot
        ],
    );
    assert_eq!(hand.to_act(), Some(0));
    assert_eq!(hand.legal_actions().unwrap().raise, fixed(155));
    play(&mut hand, &[Action::Raise { to: 155 }]); // seat 0, all-in, no slot

    assert_eq!(hand.to_act(), Some(1));
    assert_eq!(hand.legal_actions().unwrap().raise, fixed(255));
    play(&mut hand, &[Action::Raise { to: 255 }]); // wager 2
    assert_eq!(hand.legal_actions().unwrap().raise, fixed(355));
    play(&mut hand, &[Action::Raise { to: 355 }]); // wager 3
    assert_eq!(hand.legal_actions().unwrap().raise, fixed(455));
    play(&mut hand, &[Action::Raise { to: 455 }]); // wager 4 = cap

    // Exactly four wagers were counted: the two short all-ins consumed none.
    let la = hand.legal_actions().unwrap();
    assert_eq!(la.raise, None);
    assert_eq!(la.call, Some(100));
}

#[test]
fn fl_capped_means_call_or_fold_even_when_reopened() {
    // Three-handed: bet, raise, raise, raise fills the cap. Every seat still
    // to act has seen the price climb a full bet or more since it last
    // acted — the reopening rules never outrank the cap. (A seat whose
    // `acted` flag is still set cannot even exist here: the wager that fills
    // the last slot clears every flag, and at the cap nothing may move the
    // price again.)
    let deck = deck_for(0, &["Ah Ad", "Kh Kd", "Qh Qd"], "2c 7d 9s Ts 3h");
    let (mut hand, _) = HandState::new(&fl(3), &[10_000; 3], 0, 1, deck, test_rng()).unwrap();
    play(&mut hand, &[Action::Call, Action::Call, Action::Check]);
    assert_eq!(hand.street(), (1, "flop"));
    play(
        &mut hand,
        &[
            Action::Bet { to: 100 },   // wager 1
            Action::Raise { to: 200 }, // wager 2
            Action::Raise { to: 300 }, // wager 3
            Action::Raise { to: 400 }, // wager 4 = cap
        ],
    );

    for seat in [2, 0] {
        assert_eq!(hand.to_act(), Some(seat));
        let la = hand.legal_actions().unwrap();
        assert_eq!(la.raise, None);
        assert!(la.fold);
        assert!(matches!(
            hand.apply(Action::Raise { to: 500 }),
            Err(ActionError::Illegal { .. })
        ));
        play(&mut hand, &[Action::Call]);
    }
    assert_eq!(hand.street(), (2, "turn"));
}

#[test]
fn fixed_limit_heads_up_end_to_end() {
    let deck = deck_for(0, &["Ah Ad", "Kh Kd"], "2c 7d 9s Ts 3h");
    let (mut hand, _) = HandState::new(&fl(2), &[10_000, 10_000], 0, 1, deck, test_rng()).unwrap();
    play(&mut hand, &[Action::Call, Action::Check]);
    // Flop 100-sized: bet, raise, call.
    play(
        &mut hand,
        &[
            Action::Bet { to: 100 },
            Action::Raise { to: 200 },
            Action::Call,
        ],
    );
    assert_eq!(hand.street(), (2, "turn"));
    play(
        &mut hand,
        &[Action::Check, Action::Bet { to: 200 }, Action::Call],
    );
    assert_eq!(hand.street(), (3, "river"));
    play(&mut hand, &[Action::Check, Action::Check]);

    assert!(hand.is_over());
    let settle = hand.settlement().unwrap();
    assert_eq!(settle.showdown_seats, vec![1, 0]);
    assert_eq!(settle.nets, vec![500, -500]);
    assert_conserved(&hand);
}

// --- 7. Multiway side pots and odd chips --------------------------------

#[test]
fn four_handed_two_level_side_pots_with_a_folder() {
    // Seats: 0 button, 1 SB, 2 BB, 3 UTG (first to act).
    // Seat 1 tops out at 300, seat 2 at 600; seat 0 folds having put in 200.
    let holes = ["2c 2d", "Ah As", "Kh Ks", "Qh Qs"];
    let deck = deck_for(0, &holes, "7c 8d 9h Jd 4s");
    let (mut hand, _) =
        HandState::new(&nl(4), &[10_000, 300, 600, 10_000], 0, 1, deck, test_rng()).unwrap();

    assert_eq!(hand.to_act(), Some(3));
    let ev = play(
        &mut hand,
        &[
            Action::Raise { to: 200 }, // seat 3
            Action::Call,              // seat 0
            Action::Raise { to: 300 }, // seat 1, all-in
            Action::Raise { to: 600 }, // seat 2, all-in
            Action::Call,              // seat 3
            Action::Fold,              // seat 0
        ],
    );

    // Main pot: 300 from each of seats 1/2/3 plus seat 0's 200.
    // Side pot: 300 more from each of seats 2/3.
    let awards: Vec<&Event> = ev
        .iter()
        .filter(|e| matches!(e, Event::PotAwarded { .. }))
        .collect();
    assert_eq!(
        awards,
        vec![&awarded(0, &[(1, 1_100)]), &awarded(1, &[(2, 600)]),]
    );
    assert_eq!(
        hand.settlement().unwrap().nets,
        vec![-200, 800, 0, -600],
        "aces win the main pot, kings the side pot"
    );
    assert_eq!(hand.settlement().unwrap().showdown_seats, vec![1, 2, 3]);
    assert_conserved(&hand);
}

#[test]
fn split_pot_odd_chip_goes_left_of_the_button() {
    // Board plays for everyone; seats 0 and 2 chop an odd 275-chip pot.
    let stakes = Stakes::Blinds {
        small_blind: 25,
        big_blind: 50,
    };
    let mut spec = GameSpec::holdem_nl(stakes);
    spec.seats = 2..=9;
    let holes = ["2c 3c", "8d 9d", "4h 5h"];
    let deck = deck_for(0, &holes, "As Ks Qs Js Ts");
    let (mut hand, _) =
        HandState::new(&spec, &[1_000, 1_000, 1_000], 0, 1, deck, test_rng()).unwrap();

    play(
        &mut hand,
        &[Action::Raise { to: 125 }, Action::Fold, Action::Call],
    );
    assert_eq!(hand.pot_total(), 275);
    assert_eq!(hand.to_act(), Some(2), "folded small blind is skipped");
    play(&mut hand, &[Action::Check, Action::Check]);
    play(&mut hand, &[Action::Check, Action::Check]);
    let ev = play(&mut hand, &[Action::Check, Action::Check]);
    let award = ev
        .iter()
        .find(|e| matches!(e, Event::PotAwarded { .. }))
        .unwrap();
    // Odd-chip order is [1, 2, 0]; seat 2 is the earliest tied winner.
    assert_eq!(award, &awarded(0, &[(0, 137), (2, 138)]));
    assert_conserved(&hand);
}

// --- 8. Fold-out ---------------------------------------------------------

#[test]
fn three_handed_fold_out_collects_the_blinds() {
    let mut hand = three_handed(&[10_000, 10_000, 10_000]);
    let ev = play(
        &mut hand,
        &[Action::Raise { to: 300 }, Action::Fold, Action::Fold],
    );
    assert_eq!(
        ev,
        vec![
            acted(0, Action::Raise { to: 300 }, 300, false),
            acted(1, Action::Fold, 50, false),
            acted(2, Action::Fold, 100, false),
            awarded(0, &[(0, 250)]),
            Event::HandEnd {
                nets: vec![150, -50, -100],
            },
        ]
    );
    assert!(hand.settlement().unwrap().showdown_seats.is_empty());
    assert!(
        !hand
            .events()
            .iter()
            .any(|e| matches!(e, Event::ShowdownShow { .. }))
    );
    assert_conserved(&hand);
}

// --- 9. Blind all-in edges ----------------------------------------------

#[test]
fn short_big_blind_does_not_lower_the_price() {
    // Seat 2 posts 30 of the 100 big blind, all-in. Everyone else still
    // faces the nominal 100.
    let mut hand = three_handed(&[10_000, 10_000, 30]);
    assert!(hand.all_in()[2]);
    assert_eq!(hand.street_commits(), &[0, 50, 30]);
    assert_eq!(hand.to_act(), Some(0));
    let la = hand.legal_actions().unwrap();
    assert_eq!(la.call, Some(100));
    assert_eq!(
        la.raise,
        Some(BetBounds {
            min_to: 200,
            max_to: 10_000
        })
    );
    play(&mut hand, &[Action::Call, Action::Call, Action::Check]);
    assert_eq!(hand.pot_total(), 230);
    assert_eq!(hand.street(), (1, "flop"));
}

#[test]
fn both_blinds_all_in_from_posting_runs_out_from_new() {
    let deck = deck_for(0, &["Ah Ad", "Kh Kd"], "2c 7d 9s Ts 3h");
    let (hand, ev) = HandState::new(&nl(2), &[40, 30], 0, 1, deck, test_rng()).unwrap();

    assert_eq!(hand.to_act(), None);
    assert!(hand.is_over());
    // Seat 0's uncalled 10 came back before the pot was built.
    assert_eq!(hand.settlement().unwrap().nets, vec![30, -30]);
    assert_eq!(
        ev[1],
        Event::Post {
            seat: 0,
            kind: PostKind::SmallBlind,
            amount: 40,
            all_in: true,
        }
    );
    assert_eq!(
        ev[2],
        Event::Post {
            seat: 1,
            kind: PostKind::BigBlind,
            amount: 30,
            all_in: true,
        }
    );
    assert!(matches!(ev.last(), Some(Event::HandEnd { .. })));
    assert_eq!(
        ev.iter()
            .filter(|e| matches!(e, Event::DealCommunity { .. }))
            .count(),
        3
    );
    assert_conserved(&hand);
}

#[test]
fn antes_are_pot_chips_but_not_street_commitment() {
    let mut spec = nl(3);
    spec.forced_bets = ForcedBets::Blinds { ante: 10 };
    let deck = deck_for(0, &["Ah Ad", "Kh Kd", "Qh Qd"], "2c 7d 9s Ts 3h");
    let (mut hand, _) =
        HandState::new(&spec, &[10_000, 10_000, 10_000], 0, 1, deck, test_rng()).unwrap();
    assert_eq!(hand.pot_total(), 180);
    assert_eq!(hand.street_commits(), &[0, 50, 100]);
    assert_eq!(hand.legal_actions().unwrap().call, Some(100));
    play(
        &mut hand,
        &[Action::Raise { to: 300 }, Action::Fold, Action::Fold],
    );
    assert_eq!(hand.settlement().unwrap().nets, vec![170, -60, -110]);
    assert_conserved(&hand);
}

// --- 10. Illegal-action battery -----------------------------------------

#[test]
fn illegal_actions_are_rejected() {
    let mut hand = three_handed(&[10_000, 10_000, 10_000]);

    // Facing the big blind: no check, no bet.
    assert!(matches!(
        hand.apply(Action::Check),
        Err(ActionError::Illegal { .. })
    ));
    assert!(matches!(
        hand.apply(Action::Bet { to: 300 }),
        Err(ActionError::Illegal { .. })
    ));
    assert!(matches!(
        hand.apply(Action::BringIn),
        Err(ActionError::Illegal { .. })
    ));
    assert!(matches!(
        hand.apply(Action::Discard { cards: Vec::new() }),
        Err(ActionError::Illegal { .. })
    ));

    play(&mut hand, &[Action::Call, Action::Call]);
    // Big blind's option: nothing to call, so no fold and no call.
    assert_eq!(hand.to_act(), Some(2));
    assert!(matches!(
        hand.apply(Action::Fold),
        Err(ActionError::Illegal { .. })
    ));
    assert!(matches!(
        hand.apply(Action::Call),
        Err(ActionError::Illegal { .. })
    ));

    play(&mut hand, &[Action::Check]);
    // Flop with no wager yet: raising is not a thing.
    assert_eq!(hand.street(), (1, "flop"));
    assert!(matches!(
        hand.apply(Action::Raise { to: 200 }),
        Err(ActionError::Illegal { .. })
    ));

    play(&mut hand, &[Action::Bet { to: 200 }]);
    assert!(matches!(
        hand.apply(Action::Bet { to: 400 }),
        Err(ActionError::Illegal { .. })
    ));
}

#[test]
fn acting_after_the_hand_is_over_reports_hand_over() {
    let mut hand = three_handed(&[10_000, 10_000, 10_000]);
    play(
        &mut hand,
        &[Action::Raise { to: 300 }, Action::Fold, Action::Fold],
    );
    assert!(hand.is_over());
    assert_eq!(hand.legal_actions(), None);
    assert_eq!(hand.apply(Action::Check), Err(ActionError::HandOver));
    assert_eq!(hand.apply(Action::Fold), Err(ActionError::HandOver));
}

#[test]
fn hi_lo_split_pot_awards_both_sides() {
    // A hold'em skeleton with an eight-or-better low side, to exercise the
    // HiLo settlement path end to end.
    let mut spec = nl(2);
    spec.showdown = ShowdownSpec {
        hi: ShowdownSide {
            kind: EvalKind::High,
            usage: HoleUsage::Any,
        },
        lo: Some(ShowdownSide {
            kind: EvalKind::EightOrBetterLow,
            usage: HoleUsage::Any,
        }),
    };
    let deck = deck_for(0, &["Kh Kd", "Ac 2c"], "3d 4h 8s Ks 9c");
    let (mut hand, _) = HandState::new(&spec, &[1_000, 1_000], 0, 1, deck, test_rng()).unwrap();
    play(&mut hand, &[Action::Call, Action::Check]);
    play(&mut hand, &[Action::Check, Action::Check]);
    play(&mut hand, &[Action::Check, Action::Check]);
    let end = play(&mut hand, &[Action::Check, Action::Check]);

    let awards: Vec<&Event> = end
        .iter()
        .filter(|e| matches!(e, Event::PotAwarded { .. }))
        .collect();
    assert_eq!(
        awards,
        vec![
            &Event::PotAwarded {
                pot: 0,
                side: PotSide::Hi,
                winners: vec![(0, 100)],
            },
            &Event::PotAwarded {
                pot: 0,
                side: PotSide::Lo,
                winners: vec![(1, 100)],
            },
        ]
    );
    assert_conserved(&hand);
}

// --- 11. Randomized property sweep --------------------------------------

fn random_action(la: &LegalActions, rng: &mut Rng64) -> Action {
    let mut choices: Vec<u8> = Vec::new();
    if la.fold {
        choices.push(0);
    }
    if la.check {
        choices.push(1);
    }
    if la.call.is_some() {
        choices.push(2);
    }
    if la.bet.is_some() {
        choices.push(3);
    }
    if la.raise.is_some() {
        choices.push(4);
    }
    let pick = choices[rng.below(choices.len() as u64) as usize];
    let sized = |b: BetBounds, rng: &mut Rng64| b.min_to + rng.below(b.max_to - b.min_to + 1);
    match pick {
        0 => Action::Fold,
        1 => Action::Check,
        2 => Action::Call,
        3 => Action::Bet {
            to: sized(la.bet.unwrap(), rng),
        },
        _ => Action::Raise {
            to: sized(la.raise.unwrap(), rng),
        },
    }
}

/// Play one hand with a random-legal bot; returns the full event stream.
fn random_hand(spec: &GameSpec, stacks: &[Chips], button: Seat, seed: u64) -> Vec<Event> {
    let mut rng = Rng64::from_seed_stream(seed, 0);
    let deck = Deck::shuffled(&mut rng);
    let (mut hand, mut events) =
        HandState::new(spec, stacks, button, seed, deck, test_rng()).unwrap();

    let mut steps = 0;
    while let Some(la) = hand.legal_actions() {
        assert!(hand.to_act().is_some());
        assert!(
            la.fold || la.check || la.call.is_some(),
            "a seat to act always has at least one passive option: {la:?}"
        );
        assert_rejects_everything_illegal(&hand, &la);
        let action = random_action(&la, &mut rng);
        events.extend(hand.apply(action).unwrap());
        steps += 1;
        assert!(steps < 20_000, "hand failed to terminate");
    }

    assert!(hand.is_over());
    let settle = hand.settlement().unwrap();
    assert_eq!(settle.nets.iter().sum::<i64>(), 0);
    for (seat, &net) in settle.nets.iter().enumerate() {
        assert!(
            net >= -(stacks[seat] as i64),
            "seat {seat} lost more than its stack"
        );
        assert!(
            (stacks[seat] as i64 + net) >= 0,
            "seat {seat} ended with a negative stack"
        );
    }
    let awarded: Chips = settle
        .awards
        .iter()
        .flat_map(|a| a.winners.iter().map(|&(_, c)| c))
        .sum();
    assert!(settle.awards.iter().all(|a| !a.winners.is_empty()));
    let contributed: i64 = settle.nets.iter().filter(|&&n| n < 0).map(|&n| -n).sum();
    let won: i64 = settle.nets.iter().filter(|&&n| n > 0).sum();
    assert_eq!(contributed, won);
    assert!(awarded > 0);
    assert!(matches!(events.last(), Some(Event::HandEnd { .. })));
    assert_eq!(events, hand.events(), "returned events must match history");
    events
}

/// `apply` on a throwaway clone, so probes never disturb the real hand.
trait Probe {
    fn clone_probe(&self, action: Action) -> Result<Vec<Event>, ActionError>;
}

impl Probe for HandState {
    fn clone_probe(&self, action: Action) -> Result<Vec<Event>, ActionError> {
        let mut copy = self.clone();
        copy.apply(action)
    }
}

/// `apply` must succeed exactly on what `legal_actions` offered.
fn assert_rejects_everything_illegal(hand: &HandState, la: &LegalActions) {
    let mut probes: Vec<Action> = vec![Action::BringIn, Action::Discard { cards: Vec::new() }];
    if !la.fold {
        probes.push(Action::Fold);
    }
    if !la.check {
        probes.push(Action::Check);
    }
    if la.call.is_none() {
        probes.push(Action::Call);
    }
    match la.bet {
        None => probes.push(Action::Bet { to: 1 }),
        Some(b) => {
            probes.push(Action::Bet { to: b.max_to + 1 });
            if b.min_to > 0 {
                probes.push(Action::Bet { to: b.min_to - 1 });
            }
        }
    }
    match la.raise {
        None => probes.push(Action::Raise {
            to: u32::MAX as Chips,
        }),
        Some(b) => {
            probes.push(Action::Raise { to: b.max_to + 1 });
            if b.min_to > 0 {
                probes.push(Action::Raise { to: b.min_to - 1 });
            }
        }
    }
    for action in probes {
        assert!(
            matches!(
                hand.clone_probe(action.clone()),
                Err(ActionError::Illegal { .. })
            ),
            "{action:?} should have been rejected given {la:?}"
        );
    }
}

#[test]
fn random_hands_conserve_chips_and_terminate() {
    let (mut hands, mut showdowns, mut fold_outs, mut side_pots) = (0, 0, 0, 0);
    for seats in 2..=6u8 {
        for depth in [3u64, 10, 100, 250] {
            for (i, spec) in [nl(seats), fl(seats)].iter().enumerate() {
                for seed in 0..10u64 {
                    let stacks = vec![depth * STAKES.blinds().1; seats as usize];
                    let button = (seed as usize) % seats as usize;
                    let key = seed * 1000 + depth * 7 + seats as u64 + i as u64 * 131;
                    let events = random_hand(spec, &stacks, button, key);
                    hands += 1;
                    if events
                        .iter()
                        .any(|e| matches!(e, Event::ShowdownShow { .. }))
                    {
                        showdowns += 1;
                    } else {
                        fold_outs += 1;
                    }
                    if events
                        .iter()
                        .any(|e| matches!(e, Event::PotAwarded { pot, .. } if *pot > 0))
                    {
                        side_pots += 1;
                    }
                }
            }
        }
    }
    assert!(hands >= 300, "only ran {hands} hands");
    // Guard against a vacuous sweep that only ever walks one code path.
    assert!(showdowns > 0 && fold_outs > 0, "{showdowns}/{fold_outs}");
    // Equal starting stacks can never ladder into a side pot; uneven ones
    // are swept by `random_hands_with_mixed_stacks_build_side_pots`.
    assert_eq!(side_pots, 0);
}

#[test]
fn random_hands_are_deterministic() {
    for seats in 2..=6u8 {
        for spec in [nl(seats), fl(seats)] {
            for seed in 0..5u64 {
                let stacks = vec![100 * STAKES.blinds().1; seats as usize];
                let a = random_hand(&spec, &stacks, 0, seed);
                let b = random_hand(&spec, &stacks, 0, seed);
                assert_eq!(a, b, "same seed must replay identically");
            }
        }
    }
}

#[test]
fn random_hands_with_mixed_stacks_build_side_pots() {
    // Uneven stacks make all-ins and side pots common.
    let mut side_pots = 0;
    for seed in 0..60u64 {
        let stacks = vec![150, 400, 1_000, 2_500, 10_000];
        let events = random_hand(&nl(5), &stacks, (seed % 5) as usize, 900_000 + seed);
        if events
            .iter()
            .any(|e| matches!(e, Event::PotAwarded { pot, .. } if *pot > 0))
        {
            side_pots += 1;
        }
    }
    assert!(side_pots > 0, "no side pots ever formed");
}

#[test]
fn deal_spec_none_street_is_supported() {
    // A pure betting street with nothing dealt must still run.
    let mut spec = nl(2);
    spec.streets[3].deal = DealSpec::None;
    let deck = deck_for(0, &["Ah Ad", "Kh Kd"], "2c 7d 9s Ts");
    let (mut hand, _) = HandState::new(&spec, &[1_000, 1_000], 0, 1, deck, test_rng()).unwrap();
    play(&mut hand, &[Action::Call, Action::Check]);
    play(&mut hand, &[Action::Check, Action::Check]);
    play(&mut hand, &[Action::Check, Action::Check]);
    assert_eq!(hand.street(), (3, "river"));
    assert_eq!(hand.board().len(), 4);
    play(&mut hand, &[Action::Check, Action::Check]);
    assert_conserved(&hand);
}

#[test]
fn left_of_button_ordering_skips_folded_seats() {
    let mut spec = nl(4);
    spec.streets[0].betting.as_mut().unwrap().first_to_act = FirstToAct::AfterBlinds;
    let holes = ["2c 2d", "Ah As", "Kh Ks", "Qh Qs"];
    let deck = deck_for(0, &holes, "7c 8d 9h Jd 4s");
    let (mut hand, _) = HandState::new(&spec, &[1_000; 4], 0, 1, deck, test_rng()).unwrap();
    // Seat 3 opens; seat 1 (the small blind) folds.
    play(
        &mut hand,
        &[Action::Call, Action::Call, Action::Fold, Action::Check],
    );
    assert_eq!(hand.street(), (1, "flop"));
    // Flop action starts left of the button, skipping the folded seat 1.
    assert_eq!(hand.to_act(), Some(2));
}

//! Scenario tests for the three drawmaha variants: `drawmaha-fl`,
//! `drawmaha-27-fl` and `drawmaha-dugi-fl`.
//!
//! Drawmaha is five-card Omaha with one draw between the flop and turn
//! (`preflop`, `flop`, `draw` — no betting — `turn`, `river`) and a split
//! pot: the omaha half (`ExactlyTwo` hole usage, high) against the in-hand
//! half (`AllOwn`, the variant's evaluator over the whole five-card hand,
//! board ignored). Both halves are total evaluators, so the pot always
//! splits unless one seat scoops both.
//!
//! Decks are scripted with [`Deck::from_deal_order`], exactly as in
//! `draw.rs` and `split.rs`: hole cards one full batch per seat (starting
//! left of the button), then one community batch per street, with draw
//! replacements consumed in the order the draw phase draws them.

use std::collections::HashSet;

use poker_core::card::{Card, Deck, parse_cards};
use poker_core::eval::{self, EvalKind, HandClass, HoleUsage};
use poker_core::game::action::{Action, BetBounds, Chips, DrawBounds, LegalActions, Seat};
use poker_core::game::event::{Event, PotSide};
use poker_core::game::spec::{GameSpec, Stakes};
use poker_core::game::state::HandState;
use poker_core::rng::Rng64;

fn test_rng() -> Rng64 {
    Rng64::from_seed_stream(0, 0)
}

const STAKES: Stakes = Stakes::Blinds {
    small_blind: 50,
    big_blind: 100,
    ante: 0,
};

fn cards(s: &str) -> Vec<Card> {
    parse_cards(s).unwrap()
}

/// Deck dealing `holes[seat]` to each seat in engine order (five cards
/// each), then `rest` — community cards and draw replacements, in the
/// order the engine consumes them.
fn deck_for(button: Seat, holes: &[&str], rest: &str) -> Deck {
    let n = holes.len();
    let mut deal = Vec::new();
    for i in 1..=n {
        deal.extend(cards(holes[(button + i) % n]));
    }
    deal.extend(cards(rest));
    Deck::from_deal_order(&deal)
}

fn start(spec: &GameSpec, stacks: &[Chips], holes: &[&str], rest: &str) -> HandState {
    HandState::new(spec, stacks, 0, 1, deck_for(0, holes, rest), test_rng())
        .unwrap()
        .0
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

/// Drive checks and calls until a draw phase begins (or the hand ends).
/// Leaves `to_act` at the first drawer so the caller can script the draw
/// phase by hand.
fn check_or_call(hand: &mut HandState) {
    while let Some(la) = hand.legal_actions() {
        if la.draw.is_some() {
            return;
        }
        let action = if la.check {
            Action::Check
        } else if la.call.is_some() {
            Action::Call
        } else {
            panic!("no passive action available: {la:?}");
        };
        hand.apply(action).unwrap();
    }
}

/// Run the rest of the hand passively: stand pat on the draw, check when
/// possible, call when facing a wager. Ends at showdown.
fn check_down(hand: &mut HandState) {
    let mut steps = 0;
    while let Some(la) = hand.legal_actions() {
        let action = if la.draw.is_some() {
            Action::Discard { cards: Vec::new() }
        } else if la.check {
            Action::Check
        } else if la.call.is_some() {
            Action::Call
        } else {
            panic!("no passive action available: {la:?}");
        };
        play(hand, &[action]);
        steps += 1;
        assert!(steps < 200, "hand failed to terminate");
    }
}

fn assert_conserved(hand: &HandState) {
    let settle = hand.settlement().expect("hand must be settled");
    assert_eq!(settle.nets.iter().sum::<i64>(), 0, "nets must sum to zero");
    assert!(matches!(hand.events().last(), Some(Event::HandEnd { .. })));
    let awarded: Chips = settle
        .awards
        .iter()
        .flat_map(|a| a.winners.iter().map(|&(_, c)| c))
        .sum();
    assert_eq!(awarded, hand.pot_total(), "awards must exhaust the pot");
}

/// No card may sit in two live hands, or in a live hand and the board, at
/// once.
fn assert_distinct_live_cards(hand: &HandState) {
    let mut seen: HashSet<Card> = HashSet::new();
    for &card in hand.board() {
        assert!(seen.insert(card), "{card} appears twice on the board");
    }
    for event in hand.events() {
        if let Event::ShowdownShow { seat, cards, .. } = event {
            for &card in cards {
                assert!(seen.insert(card), "{card} shown twice (seat {seat})");
            }
        }
    }
}

fn awards(hand: &HandState) -> Vec<Event> {
    hand.events()
        .iter()
        .filter(|e| matches!(e, Event::PotAwarded { .. }))
        .cloned()
        .collect()
}

fn awarded(pot: u8, side: PotSide, winners: &[(Seat, Chips)]) -> Event {
    Event::PotAwarded {
        pot,
        side,
        winners: winners.to_vec(),
    }
}

/// The (hi, lo) values the engine published for a seat at showdown.
fn shown(hand: &HandState, seat: Seat) -> (Option<eval::HandValue>, Option<eval::HandValue>) {
    hand.events()
        .iter()
        .find_map(|e| match e {
            Event::ShowdownShow {
                seat: s, hi, lo, ..
            } if *s == seat => Some((*hi, *lo)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("seat {seat} never reached showdown"))
}

fn drawn_in(events: &[Event]) -> Vec<Card> {
    events
        .iter()
        .find_map(|e| match e {
            Event::DrawResult { drawn, .. } => Some(drawn.clone()),
            _ => None,
        })
        .expect("a discard always produces a DrawResult")
}

// --- 1. The draw sits between flop and turn, with no betting --------------

#[test]
fn draw_sits_between_flop_and_turn_with_no_betting() {
    let holes = ["2c 3d 4h 5s 7c", "Ah Kh Qh Jh Th"];
    let mut hand = start(
        &GameSpec::drawmaha_fl(STAKES),
        &[10_000; 2],
        &holes,
        "9c 8d 7h 6c 5d",
    );

    // Preflop: button/SB calls, BB checks its option.
    play(&mut hand, &[Action::Call, Action::Check]);

    // Flop: left of button checks, then the button checks. The second
    // check's batch is exactly [Acted, Acted, StreetStart("draw")] — no
    // deal event, since a draw street deals nothing up front.
    let flop_ev = play(&mut hand, &[Action::Check, Action::Check]);
    assert_eq!(
        flop_ev,
        vec![
            Event::Acted {
                seat: 1,
                action: Action::Check,
                street_commit: 0,
                all_in: false,
            },
            Event::Acted {
                seat: 0,
                action: Action::Check,
                street_commit: 0,
                all_in: false,
            },
            Event::StreetStart {
                street: 2,
                label: "draw".to_string(),
            },
        ]
    );

    assert_eq!(hand.street(), (2, "draw"));
    assert_eq!(hand.to_act(), Some(1), "left of button draws first");
    assert_eq!(
        hand.legal_actions().unwrap(),
        LegalActions {
            draw: Some(DrawBounds { max_discards: 5 }),
            ..LegalActions::default()
        },
        "a draw phase offers nothing but Discard"
    );

    let ev1 = play(&mut hand, &[Action::Discard { cards: Vec::new() }]);
    assert_eq!(
        ev1,
        vec![Event::DrawResult {
            seat: 1,
            discarded: 0,
            drawn: Vec::new(),
        }],
        "no betting event sits between the two draws"
    );

    assert_eq!(hand.to_act(), Some(0));
    assert_eq!(
        hand.legal_actions().unwrap(),
        LegalActions {
            draw: Some(DrawBounds { max_discards: 5 }),
            ..LegalActions::default()
        }
    );

    // The last drawer's batch carries straight on into the turn deal: the
    // draw street opens no betting round of its own.
    let ev2 = play(&mut hand, &[Action::Discard { cards: Vec::new() }]);
    assert!(
        !ev2.iter().any(|e| matches!(e, Event::Acted { .. })),
        "the draw phase never contains an Acted event: {ev2:?}"
    );
    assert_eq!(
        ev2[0],
        Event::DrawResult {
            seat: 0,
            discarded: 0,
            drawn: Vec::new(),
        }
    );
    assert_eq!(
        ev2[1],
        Event::StreetStart {
            street: 3,
            label: "turn".to_string(),
        }
    );
    assert!(matches!(ev2[2], Event::DealCommunity { street: 3, .. }));
    assert_eq!(ev2.len(), 3);

    assert_eq!(hand.street(), (3, "turn"));
    let la = hand.legal_actions().unwrap();
    assert!(la.draw.is_none(), "the draw is over; turn bets normally");
    assert_eq!(
        la.bet,
        Some(BetBounds {
            min_to: 200,
            max_to: 200,
        }),
        "turn is a big-bet street"
    );
}

// --- 2. Replacements come off the deck ahead of the turn card -------------

#[test]
fn draw_replacements_come_after_board_cards_in_deck_order() {
    let holes = ["2c 3d 4h 5s 7c", "As Ks Qs Js 9s"];
    let mut hand = start(
        &GameSpec::drawmaha_fl(STAKES),
        &[10_000; 2],
        &holes,
        "Th 9h 8h 2s 3s 4c 5c 6c",
    );

    play(&mut hand, &[Action::Call, Action::Check]);
    play(&mut hand, &[Action::Check, Action::Check]);

    // Seat 1 draws first and takes the next two cards off the deck.
    let ev = play(
        &mut hand,
        &[Action::Discard {
            cards: cards("As Ks"),
        }],
    );
    assert_eq!(drawn_in(&ev), cards("2s 3s"));
    assert_eq!(hand.hole_cards(1), &cards("Qs Js 9s 2s 3s")[..]);

    // Seat 0 then draws the card after that, and the same batch carries the
    // turn deal straight from the same deck.
    let ev = play(&mut hand, &[Action::Discard { cards: cards("2c") }]);
    assert_eq!(drawn_in(&ev), cards("4c"));
    assert_eq!(hand.hole_cards(0), &cards("3d 4h 5s 7c 4c")[..]);
    assert_eq!(
        ev[1],
        Event::StreetStart {
            street: 3,
            label: "turn".to_string(),
        }
    );
    assert_eq!(
        ev[2],
        Event::DealCommunity {
            street: 3,
            cards: cards("5c"),
        },
        "the turn card is whatever follows the replacements in deck order"
    );

    play(&mut hand, &[Action::Check, Action::Check]);
    assert_eq!(
        hand.events()[hand.events().len() - 1],
        Event::DealCommunity {
            street: 4,
            cards: cards("6c"),
        }
    );
    play(&mut hand, &[Action::Check, Action::Check]);

    let shown: Vec<Vec<Card>> = hand
        .events()
        .iter()
        .filter_map(|e| match e {
            Event::ShowdownShow { cards, .. } => Some(cards.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        shown,
        vec![cards("Qs Js 9s 2s 3s"), cards("3d 4h 5s 7c 4c")],
        "exact final hands at showdown, seat 1 (left of button) first"
    );
    assert_eq!(hand.board(), &cards("Th 9h 8h 5c 6c")[..]);
    assert_distinct_live_cards(&hand);
    assert_conserved(&hand);
}

// --- 3. The omaha half is exactly two hole cards ---------------------------

#[test]
fn omaha_half_uses_exactly_two() {
    // Seat 0 holds exactly one heart among five; the board is a four-flush.
    // The omaha half (ExactlyTwo) can only ever pull in one hole heart plus
    // three board hearts — four, one short of a flush — while the in-hand
    // half sees all five hole cards, board ignored.
    let holes = ["Ah 4d 2c 5s 9c", "Kd Qc Jh Ts 9d"];
    let board = "Kh Qh 8h 3h 7s";
    let mut hand = start(&GameSpec::drawmaha_fl(STAKES), &[10_000; 2], &holes, board);
    check_down(&mut hand);

    let (hi0, lo0) = shown(&hand, 0);
    assert_eq!(lo0, Some(eval::high(&cards("Ah 9c Kh Qh 8h"))));
    assert_eq!(
        lo0.unwrap().high_class(),
        HandClass::HighCard,
        "one hole heart plus three board hearts is one short of a flush"
    );

    // Confirm it's the ExactlyTwo rule and not the card pool that blocks
    // the flush: the same nine cards, used freely, do make one.
    let mut pool = cards("Ah 4d 2c 5s 9c");
    pool.extend(cards(board));
    assert_eq!(eval::high(&pool).high_class(), HandClass::Flush);
    assert_eq!(
        eval::best_with_usage(
            EvalKind::High,
            HoleUsage::ExactlyTwo,
            &cards("Ah 4d 2c 5s 9c"),
            &cards(board),
        ),
        lo0
    );

    assert_eq!(
        hi0,
        Some(eval::high(&cards("Ah 4d 2c 5s 9c"))),
        "the in-hand half uses all five hole cards and ignores the board"
    );
    assert_eq!(hi0.unwrap().high_class(), HandClass::HighCard);

    assert_distinct_live_cards(&hand);
    assert_conserved(&hand);
}

// --- 4. The in-hand half ignores the board ---------------------------------

#[test]
fn in_hand_half_ignores_the_board() {
    // Seat 0: bare quad deuces (huge in-hand, weak omaha — only a
    // low pair reaches the board). Seat 1: reversed — Ah/Kh plus three
    // board hearts makes a flush for omaha, but the in-hand five-card hand
    // is just ace-high.
    let holes = ["2c 2d 2h 2s 9c", "Ah Kh 5s 6d 7c"];
    let board = "Qh Jh 8h 4c 3d";
    let mut hand = start(&GameSpec::drawmaha_fl(STAKES), &[10_000; 2], &holes, board);
    check_down(&mut hand);

    let (hi0, lo0) = shown(&hand, 0);
    let (hi1, lo1) = shown(&hand, 1);

    assert_eq!(
        lo0,
        eval::best_with_usage(
            EvalKind::High,
            HoleUsage::ExactlyTwo,
            &cards("2c 2d 2h 2s 9c"),
            &cards(board)
        )
    );
    assert_eq!(
        lo1,
        eval::best_with_usage(
            EvalKind::High,
            HoleUsage::ExactlyTwo,
            &cards("Ah Kh 5s 6d 7c"),
            &cards(board)
        )
    );
    assert!(
        lo1 > lo0,
        "seat 1's board-connected flush beats seat 0's bare pair of deuces"
    );

    assert_eq!(hi0, Some(eval::high(&cards("2c 2d 2h 2s 9c"))));
    assert_eq!(hi1, Some(eval::high(&cards("Ah Kh 5s 6d 7c"))));
    assert_eq!(hi0.unwrap().high_class(), HandClass::Quads);
    assert_eq!(hi1.unwrap().high_class(), HandClass::HighCard);
    assert!(
        hi0 > hi1,
        "seat 0's quads beat seat 1's ace-high once the board is ignored"
    );

    assert_eq!(
        awards(&hand),
        vec![
            awarded(0, PotSide::Hi, &[(0, 100)]),
            awarded(0, PotSide::Lo, &[(1, 100)]),
        ]
    );
    assert_eq!(hand.settlement().unwrap().nets, vec![0, 0]);
    assert_distinct_live_cards(&hand);
    assert_conserved(&hand);
}

// --- 5. drawmaha-27: the in-hand half is 2-7 lowball -----------------------

#[test]
fn drawmaha27_in_hand_half_is_deuce_seven() {
    let holes = ["7d 5c 4h 3s Kc", "Ks Kd 9h 8s 6c"];
    let mut hand = start(
        &GameSpec::drawmaha27_fl(STAKES),
        &[10_000; 2],
        &holes,
        "Tc Jd Qs 2c 5h 6h",
    );

    play(&mut hand, &[Action::Call, Action::Check]);
    play(&mut hand, &[Action::Check, Action::Check]);

    // Seat 1 (left of button) draws first and stands pat; seat 0 breaks
    // its king for the 2-7 nuts.
    play(&mut hand, &[Action::Discard { cards: Vec::new() }]);
    let ev = play(&mut hand, &[Action::Discard { cards: cards("Kc") }]);
    assert_eq!(drawn_in(&ev), cards("2c"));

    play(&mut hand, &[Action::Check, Action::Check]);
    play(&mut hand, &[Action::Check, Action::Check]);

    assert_eq!(hand.hole_cards(0), &cards("7d 5c 4h 3s 2c")[..]);
    assert_eq!(hand.hole_cards(1), &cards("Ks Kd 9h 8s 6c")[..]);

    let (hi0, _) = shown(&hand, 0);
    let (hi1, _) = shown(&hand, 1);
    assert_eq!(
        hi0,
        Some(eval::deuce_to_seven_low(&cards("7d 5c 4h 3s 2c")))
    );
    assert_eq!(
        hi1,
        Some(eval::deuce_to_seven_low(&cards("Ks Kd 9h 8s 6c")))
    );
    assert!(hi0 > hi1, "7-5-4-3-2 beats a pair of kings at 2-7");

    // A-5-4-3-2 is merely ace-high at 2-7 (aces are always high, so it is
    // not a "wheel"); the 7-5-4-3-2 nuts crush it.
    assert!(
        eval::deuce_to_seven_low(&cards("7d 5c 4h 3s 2c"))
            > eval::deuce_to_seven_low(&cards("Ac 5h 4d 3c 2d")),
        "A-5-4-3-2 is ace-high at 2-7, not a wheel"
    );

    assert!(
        awards(&hand).contains(&awarded(0, PotSide::Hi, &[(0, 100)])),
        "seat 0's 2-7 nuts must take the hand (hi) half: {:?}",
        awards(&hand)
    );
    assert_distinct_live_cards(&hand);
    assert_conserved(&hand);
}

// --- 6. drawmaha-dugi: the in-hand half is badugi ---------------------------

#[test]
fn drawmaha_dugi_in_hand_half_is_badugi() {
    // As and 2s clash: dropping the deuce (A-3-4-5) beats dropping the ace
    // with aces low, exactly as in badacey. Four kings plus a club forces
    // seat 1's badugi down to two cards.
    let holes = ["As 2s 3d 4h 5c", "Kc Kd Kh Ks 9c"];
    let mut hand = start(
        &GameSpec::drawmaha_dugi_fl(STAKES),
        &[10_000; 2],
        &holes,
        "Th Jc Qd 8s 7h",
    );
    check_down(&mut hand);

    let (hi0, _) = shown(&hand, 0);
    let (hi1, _) = shown(&hand, 1);

    assert_eq!(
        hi0,
        Some(eval::badugi(&cards("As 3d 4h 5c"))),
        "the best four-of-five badugi drops the deuce, not the ace"
    );
    assert_ne!(hi0, Some(eval::badugi(&cards("2s 3d 4h 5c"))));
    assert_eq!(hi0.unwrap().0 >> 20, 4, "a four-card badugi");

    assert_eq!(
        hi1,
        Some(eval::badugi(&cards("Kd 9c"))),
        "four same-rank kings force a two-card badugi"
    );
    assert_eq!(
        hi1.unwrap().0 >> 20,
        2,
        "a rainbow-poor hand, shorter badugi"
    );

    assert!(hi0 > hi1, "the four-card badugi beats the two-card one");
    assert_distinct_live_cards(&hand);
    assert_conserved(&hand);
}

// --- 7. Scoop vs. split, and the odd chip on an odd pot --------------------

#[test]
fn both_halves_always_split_or_one_seat_scoops() {
    // Scoop: seat 0's own pocket pair of aces (plus trip deuces for a full
    // house in-hand) beats seat 1 on both halves. The board (9-T-J-Q-K, all
    // distinct ranks, no two of one suit) neither pairs nor completes a
    // straight for either seat: seat 0's ranks (A, 2) and seat 1's (5, 6, 7)
    // both sit well clear of the board's run.
    let holes = ["Ah Ac 2s 2d 2h", "5c 6d 5h 6s 7c"];
    let board = "9d Tc Jh Qs Kc";
    let mut hand = start(&GameSpec::drawmaha_fl(STAKES), &[10_000; 2], &holes, board);
    check_down(&mut hand);

    let (hi0, lo0) = shown(&hand, 0);
    let (hi1, lo1) = shown(&hand, 1);
    assert_eq!(
        lo0,
        Some(eval::high(&cards("Ah Ac Kc Qs Jh"))),
        "seat 0's pocket aces plus the board's three best kickers"
    );
    assert_eq!(hi0, Some(eval::high(&cards("Ah Ac 2s 2d 2h"))));
    assert_eq!(hi0.unwrap().high_class(), HandClass::FullHouse);
    assert!(
        lo0 > lo1,
        "seat 0's ace pair beats anything seat 1 can make"
    );
    assert!(
        hi0 > hi1,
        "seat 0's full house beats anything seat 1 can make"
    );

    assert_eq!(
        awards(&hand),
        vec![
            awarded(0, PotSide::Hi, &[(0, 100)]),
            awarded(0, PotSide::Lo, &[(0, 100)]),
        ],
        "one seat wins both halves: two PotAwarded events, same winner"
    );
    assert_eq!(hand.settlement().unwrap().nets, vec![100, -100]);
    assert_conserved(&hand);

    // Split with an odd pot: three seats calling an odd big blind down to
    // showdown make an odd-chip pot; the in-hand (hi) half gets the extra
    // chip. Seat 0/1 reuse the reversed-strength pair from test 4; seat 2 is
    // a spare seat, weaker than both on either half.
    const ODD_STAKES: Stakes = Stakes::Blinds {
        small_blind: 51,
        big_blind: 101,
        ante: 0,
    };
    let holes = ["2c 2d 2h 2s 9c", "Ah Kh 5s 6d 7c", "7d 8d Tc Jc Qc"];
    let board = "Qh Jh 8h 4c 3d";
    let mut hand = start(
        &GameSpec::drawmaha_fl(ODD_STAKES),
        &[10_000; 3],
        &holes,
        board,
    );
    check_down(&mut hand);

    assert_eq!(hand.pot_total(), 303, "an odd pot");
    assert_eq!(
        awards(&hand),
        vec![
            awarded(0, PotSide::Hi, &[(0, 152)]),
            awarded(0, PotSide::Lo, &[(1, 151)]),
        ],
        "the odd chip (303 / 2 = 151 r 1) goes to the in-hand (hi) half"
    );
    assert_eq!(hand.settlement().unwrap().nets, vec![51, 50, -101]);
    assert_distinct_live_cards(&hand);
    assert_conserved(&hand);
}

// --- 8. Standing pat and discarding all five both work ---------------------

#[test]
fn stand_pat_and_full_five_discard_both_work() {
    let holes = ["2c 3d 4h 5s 7c", "Ah Kh Qh Jh Th"];
    let mut hand = start(
        &GameSpec::drawmaha_fl(STAKES),
        &[10_000; 2],
        &holes,
        "9c 8d 6s 9d Tc Jc Qc Kc 9s 8s",
    );

    check_or_call(&mut hand);
    assert_eq!(hand.street(), (2, "draw"));
    assert_eq!(hand.to_act(), Some(1));

    play(&mut hand, &[Action::Discard { cards: Vec::new() }]);
    assert_eq!(
        hand.hole_cards(1),
        &cards("Ah Kh Qh Jh Th")[..],
        "standing pat leaves the hand untouched"
    );

    assert_eq!(hand.to_act(), Some(0));
    let whole_hand = hand.hole_cards(0).to_vec();
    assert_eq!(whole_hand.len(), 5);
    let ev = play(
        &mut hand,
        &[Action::Discard {
            cards: whole_hand.clone(),
        }],
    );
    assert_eq!(drawn_in(&ev), cards("9d Tc Jc Qc Kc"));
    assert_eq!(hand.hole_cards(0), &cards("9d Tc Jc Qc Kc")[..]);

    check_down(&mut hand);

    let shown: Vec<Vec<Card>> = hand
        .events()
        .iter()
        .filter_map(|e| match e {
            Event::ShowdownShow { cards, .. } => Some(cards.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        shown,
        vec![cards("Ah Kh Qh Jh Th"), cards("9d Tc Jc Qc Kc")],
        "showdown reflects the pat hand and the fully replaced hand"
    );
    assert_distinct_live_cards(&hand);
    assert_conserved(&hand);
}

// --- 9. Six-handed drains the deck and reshuffles the discards -------------

#[test]
fn six_handed_completes_from_one_deck_with_reshuffle_if_needed() {
    // 6 seats x 5 hole + 5 board = 35 cards dealt before the draw; 6 x 5 = 30
    // replacement requests against a 52-card deck (19 left after the flop)
    // can only be filled by recycling the discard pile.
    let spec = GameSpec::drawmaha_fl(STAKES);
    let (mut hand, _) =
        HandState::new(&spec, &[10_000; 6], 0, 1, Deck::standard(), test_rng()).unwrap();

    check_or_call(&mut hand);
    assert_eq!(hand.street(), (2, "draw"));

    let mut log = Vec::new();
    while hand.legal_actions().is_some_and(|la| la.draw.is_some()) {
        let seat = hand.to_act().expect("legal_actions implies a seat");
        let discarded = hand.hole_cards(seat).to_vec();
        assert_eq!(discarded.len(), 5, "every seat discards its whole hand");
        let ev = hand
            .apply(Action::Discard {
                cards: discarded.clone(),
            })
            .expect("discarding the whole hand is within max");
        log.push((seat, discarded, drawn_in(&ev)));
    }

    assert_eq!(log.len(), 6, "all six seats drew");
    let total: usize = log.iter().map(|(_, _, drawn)| drawn.len()).sum();
    assert_eq!(total, 30, "6 seats x 5 cards, filled from a 52-card deck");
    for (seat, discarded, drawn) in &log {
        assert_eq!(drawn.len(), discarded.len(), "seat {seat} short-changed");
        for card in drawn {
            assert!(
                !discarded.contains(card),
                "seat {seat} drew its own just-discarded card back"
            );
        }
    }

    check_down(&mut hand);

    assert!(hand.is_over());
    assert_eq!(hand.settlement().unwrap().showdown_seats.len(), 6);
    assert_distinct_live_cards(&hand);
    assert_conserved(&hand);
}

// --- 10. Randomized sweep over all three variants --------------------------

fn random_action(hand: &HandState, la: &LegalActions, rng: &mut Rng64) -> Action {
    if let Some(bounds) = la.draw {
        let seat = hand.to_act().expect("legal_actions implies a seat");
        let mut pool = hand.hole_cards(seat).to_vec();
        rng.shuffle(&mut pool);
        let max = (bounds.max_discards as usize).min(pool.len());
        pool.truncate(rng.below(max as u64 + 1) as usize);
        return Action::Discard { cards: pool };
    }
    let mut choices: Vec<Action> = Vec::new();
    if la.fold {
        choices.push(Action::Fold);
    }
    if la.check {
        choices.push(Action::Check);
    }
    if la.call.is_some() {
        choices.push(Action::Call);
    }
    let sized = |b: BetBounds, rng: &mut Rng64| b.min_to + rng.below(b.max_to - b.min_to + 1);
    if let Some(b) = la.bet {
        choices.push(Action::Bet { to: sized(b, rng) });
    }
    if let Some(b) = la.raise {
        choices.push(Action::Raise { to: sized(b, rng) });
    }
    choices.swap_remove(rng.below(choices.len() as u64) as usize)
}

/// Play one hand with a random-legal driver, asserting the invariants that
/// must hold for every drawmaha variant.
fn random_hand(spec: &GameSpec, stacks: &[Chips], button: Seat, seed: u64) -> Vec<Event> {
    let mut rng = Rng64::from_seed_stream(seed, 3);
    let deck = Deck::shuffled(&mut rng);
    let (mut hand, mut events) = HandState::new(
        spec,
        stacks,
        button,
        seed,
        deck,
        Rng64::from_seed_stream(seed, 77),
    )
    .unwrap();

    let mut steps = 0;
    while let Some(la) = hand.legal_actions() {
        assert!(
            la.fold || la.check || la.call.is_some() || la.draw.is_some(),
            "a seat to act always has something passive to do: {la:?}"
        );
        events.extend(hand.apply(random_action(&hand, &la, &mut rng)).unwrap());
        steps += 1;
        assert!(steps < 1_000, "hand failed to terminate");
    }

    let settle = hand.settlement().unwrap();
    assert_eq!(settle.nets.iter().sum::<i64>(), 0, "nets must sum to zero");
    let awarded: Chips = settle
        .awards
        .iter()
        .flat_map(|a| a.winners.iter().map(|&(_, c)| c))
        .sum();
    assert_eq!(awarded, hand.pot_total(), "awards must exhaust the pot");
    for (seat, &net) in settle.nets.iter().enumerate() {
        assert!(
            stacks[seat] as i64 + net >= 0,
            "seat {seat} ended with a negative stack"
        );
        assert!(net >= -(stacks[seat] as i64));
    }
    assert_distinct_live_cards(&hand);
    assert_eq!(events, hand.events(), "returned events must match history");
    events
}

fn drawmaha_specs() -> [GameSpec; 3] {
    [
        GameSpec::drawmaha_fl(STAKES),
        GameSpec::drawmaha27_fl(STAKES),
        GameSpec::drawmaha_dugi_fl(STAKES),
    ]
}

#[test]
fn random_drawmaha_hands_hold_every_invariant() {
    let (mut hi_lo_splits, mut fold_outs) = (0, 0);
    for spec in &drawmaha_specs() {
        for seats in 2..=6usize {
            for seed in 0..30u64 {
                let stacks = vec![60 * STAKES.blinds().1; seats];
                let key = seed * 977 + seats as u64;
                let events = random_hand(spec, &stacks, seed as usize % seats, key);
                if !events
                    .iter()
                    .any(|e| matches!(e, Event::ShowdownShow { .. }))
                {
                    fold_outs += 1;
                    continue;
                }
                if events.iter().any(|e| {
                    matches!(
                        e,
                        Event::PotAwarded {
                            side: PotSide::Hi,
                            ..
                        }
                    )
                }) {
                    hi_lo_splits += 1;
                }
            }
        }
    }
    assert!(fold_outs > 0, "no hand ever folded out");
    assert!(hi_lo_splits > 0, "no pot ever split hi/lo");

    // Determinism: replaying the same seed must reproduce the exact events.
    for spec in &drawmaha_specs() {
        for seats in 2..=6usize {
            for seed in 0..5u64 {
                let stacks = vec![60 * STAKES.blinds().1; seats];
                let a = random_hand(spec, &stacks, 0, seed);
                let b = random_hand(spec, &stacks, 0, seed);
                assert_eq!(a, b, "same seed must replay identically");
            }
        }
    }
}

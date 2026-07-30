//! Scenario tests for the draw family: discard phases, replacements, deck
//! exhaustion and draw-game showdowns.
//!
//! Decks are scripted with [`Deck::from_deal_order`]. `draw_deck` lays out
//! one full hand per seat in seat order starting left of the button, followed
//! by the replacement cards in the order the draw phases consume them.

use std::collections::HashSet;

use poker_core::card::{Card, Deck, parse_cards};
use poker_core::game::action::{Action, BetBounds, Chips, DrawBounds, LegalActions, Seat};
use poker_core::game::event::Event;
use poker_core::game::spec::{GameSpec, Stakes};
use poker_core::game::state::{ActionError, HandState};
use poker_core::rng::Rng64;

fn test_rng() -> Rng64 {
    Rng64::from_seed_stream(0, 0)
}

const STAKES: Stakes = Stakes::Blinds {
    small_blind: 50,
    big_blind: 100,
};

fn cards(s: &str) -> Vec<Card> {
    parse_cards(s).unwrap()
}

fn draw_deck(button: Seat, hands: &[&str], rest: &str) -> Deck {
    let n = hands.len();
    let mut deal = Vec::new();
    for i in 1..=n {
        deal.extend(cards(hands[(button + i) % n]));
    }
    deal.extend(cards(rest));
    Deck::from_deal_order(&deal)
}

fn start(spec: &GameSpec, stacks: &[Chips], hands: &[&str], rest: &str) -> HandState {
    HandState::new(spec, stacks, 0, 1, draw_deck(0, hands, rest), test_rng())
        .unwrap()
        .0
}

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

fn pat(hand: &mut HandState, times: usize) {
    for _ in 0..times {
        hand.apply(Action::Discard { cards: Vec::new() })
            .expect("standing pat is always legal in a draw phase");
    }
}

fn fixed(to: Chips) -> Option<BetBounds> {
    Some(BetBounds {
        min_to: to,
        max_to: to,
    })
}

fn assert_conserved(hand: &HandState) {
    let settle = hand.settlement().expect("hand must be settled");
    assert_eq!(settle.nets.iter().sum::<i64>(), 0, "nets must sum to zero");
    assert!(matches!(hand.events().last(), Some(Event::HandEnd { .. })));
}

/// No card may sit in two live hands at once.
fn assert_distinct_live_cards(hand: &HandState) {
    let mut seen: HashSet<Card> = HashSet::new();
    for event in hand.events() {
        if let Event::ShowdownShow { seat, cards, .. } = event {
            for &card in cards {
                assert!(seen.insert(card), "{card} shown twice (seat {seat})");
            }
        }
    }
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

// --- 1. Draw phase order and legality -----------------------------------

#[test]
fn every_live_seat_draws_once_starting_left_of_the_button() {
    let hands = ["Ah Kh Qh Jh Th", "2c 3d 4h 5s 7c", "8c 9d Tc Jd Qc"];
    let mut hand = start(
        &GameSpec::td27_fl(STAKES),
        &[10_000; 3],
        &hands,
        "2h 3h 4s 5d 6c 7d 8h 9s",
    );
    play(&mut hand, &[Action::Call, Action::Call, Action::Check]);

    // The draw phase runs before the street's betting round.
    assert_eq!(hand.street(), (1, "draw1"));
    let mut order = Vec::new();
    for _ in 0..3 {
        let seat = hand.to_act().expect("three seats still owe a draw");
        assert_eq!(
            hand.legal_actions().unwrap(),
            LegalActions {
                fold: false,
                check: false,
                call: None,
                bet: None,
                raise: None,
                bring_in: None,
                draw: Some(DrawBounds { max_discards: 5 }),
            },
            "a draw phase offers nothing but `Discard`"
        );
        order.push(seat);
        pat(&mut hand, 1);
    }
    assert_eq!(order, vec![1, 2, 0]);

    // Once everyone has drawn, the betting round opens as usual.
    assert_eq!(hand.street(), (1, "draw1"));
    assert_eq!(hand.to_act(), Some(1));
    assert_eq!(hand.legal_actions().unwrap().bet, fixed(100));
}

#[test]
fn other_action_families_are_rejected_during_a_draw() {
    let hands = ["Ah Kh Qh Jh Th", "2c 3d 4h 5s 7c"];
    let mut hand = start(&GameSpec::td27_fl(STAKES), &[10_000; 2], &hands, "2h 3h 4s");
    play(&mut hand, &[Action::Call, Action::Check]);
    for illegal in [
        Action::Fold,
        Action::Check,
        Action::Call,
        Action::Bet { to: 100 },
        Action::Raise { to: 200 },
        Action::BringIn,
    ] {
        assert!(
            matches!(
                hand.apply(illegal.clone()),
                Err(ActionError::Illegal { .. })
            ),
            "{illegal:?} must be rejected during a draw phase"
        );
    }
}

// --- 2. Discard validation ------------------------------------------------

#[test]
fn discards_must_be_distinct_cards_the_seat_holds() {
    let hands = ["Ah Kh Qh Jh Th", "2c 3d 4h 5s 7c"];
    let mut hand = start(
        &GameSpec::td27_fl(STAKES),
        &[10_000; 2],
        &hands,
        "2h 3h 4s 5d 6c",
    );
    play(&mut hand, &[Action::Call, Action::Check]);
    assert_eq!(hand.to_act(), Some(1));

    let before_hole = hand.hole_cards(1).to_vec();
    let before_events = hand.events().len();
    for (illegal, why) in [
        (cards("As"), "a card the seat does not hold"),
        (cards("2c 2c"), "the same card twice"),
        (
            cards("2c 3d 4h 5s 7c Ah"),
            "more cards than the draw allows",
        ),
    ] {
        assert!(
            matches!(
                hand.apply(Action::Discard {
                    cards: illegal.clone()
                }),
                Err(ActionError::Illegal { .. })
            ),
            "discarding {illegal:?} ({why}) must be rejected"
        );
        assert_eq!(hand.hole_cards(1), &before_hole[..], "hand was mutated");
        assert_eq!(hand.events().len(), before_events, "events were emitted");
        assert_eq!(hand.to_act(), Some(1));
    }

    // Standing pat is always available and consumes the seat's turn.
    let ev = play(&mut hand, &[Action::Discard { cards: Vec::new() }]);
    assert_eq!(
        ev[0],
        Event::DrawResult {
            seat: 1,
            discarded: 0,
            drawn: Vec::new(),
        }
    );
    assert_eq!(hand.hole_cards(1), &before_hole[..]);
    assert_eq!(hand.to_act(), Some(0));
}

// --- 3. Replacements ------------------------------------------------------

#[test]
fn replacements_come_off_the_top_of_the_deck() {
    let hands = ["Ah Kh Qh Jh Th", "2c 3d 4h 5s 7c"];
    let mut hand = start(
        &GameSpec::td27_fl(STAKES),
        &[10_000; 2],
        &hands,
        "9d 8s 6h 6d",
    );
    play(&mut hand, &[Action::Call, Action::Check]);

    // Seat 1 draws first and takes the next two cards off the deck.
    let ev = play(
        &mut hand,
        &[Action::Discard {
            cards: cards("2c 3d"),
        }],
    );
    assert_eq!(drawn_in(&ev), cards("9d 8s"));
    assert_eq!(hand.hole_cards(1), &cards("4h 5s 7c 9d 8s")[..]);

    // Seat 0 then gets the two after those.
    let ev = play(
        &mut hand,
        &[Action::Discard {
            cards: cards("Ah Kh"),
        }],
    );
    assert_eq!(drawn_in(&ev), cards("6h 6d"));
    assert_eq!(hand.hole_cards(0), &cards("Qh Jh Th 6h 6d")[..]);

    // ... and the final hands are what shows down.
    play(&mut hand, &[Action::Check, Action::Check]);
    pat(&mut hand, 2);
    play(&mut hand, &[Action::Check, Action::Check]);
    pat(&mut hand, 2);
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
        vec![cards("4h 5s 7c 9d 8s"), cards("Qh Jh Th 6h 6d")]
    );
    assert_conserved(&hand);
}

// --- 4. Run-outs still draw ----------------------------------------------

#[test]
fn all_in_seats_still_draw_during_a_run_out() {
    // Both seats are all-in before the first draw. All three draw phases
    // still run — only the betting rounds are skipped.
    let hands = ["7d 5c 4h 3s 2c", "Ah Kh Qh Jh Th"];
    let mut hand = start(&GameSpec::td27_fl(STAKES), &[300; 2], &hands, "9c 8d 7h");
    play(
        &mut hand,
        &[
            Action::Raise { to: 200 },
            Action::Raise { to: 300 },
            Action::Call,
        ],
    );
    assert!(hand.all_in()[0] && hand.all_in()[1]);
    assert_eq!(hand.street(), (1, "draw1"));
    assert_eq!(hand.to_act(), Some(1), "all-in seats still draw");

    for replacement in ["9c", "8d", "7h"] {
        let discard = hand.hole_cards(1).last().copied().unwrap();
        let ev = play(
            &mut hand,
            &[Action::Discard {
                cards: vec![discard],
            }],
        );
        assert_eq!(drawn_in(&ev), cards(replacement));
        pat(&mut hand, 1);
    }

    assert!(hand.is_over());
    assert_eq!(
        hand.events()
            .iter()
            .filter(|e| matches!(e, Event::DrawResult { .. }))
            .count(),
        6,
        "three draw phases still ran for both seats"
    );
    assert_eq!(
        hand.hole_cards(1),
        &cards("Ah Kh Qh Jh 7h")[..],
        "the run-out drew seat 1 into a flush"
    );
    assert_eq!(
        hand.settlement().unwrap().nets,
        vec![300, -300],
        "7-5-4-3-2 beats a flush at 2-7"
    );
    assert_distinct_live_cards(&hand);
    assert_conserved(&hand);
}

#[test]
fn draw_specs_are_sized_for_the_initial_deal_only() {
    // Six-handed 2-7 could consume 120 cards in the worst case, which no
    // deck can cover; the reshuffle is what makes it playable, so `new` must
    // only insist on the 30 cards the opening deal needs.
    let spec = GameSpec::td27_fl(STAKES);
    let exact: Vec<Card> = (0..30).map(|i| Card::from_index(i).unwrap()).collect();
    assert!(
        HandState::new(
            &spec,
            &[10_000; 6],
            0,
            1,
            Deck::from_deal_order(&exact),
            test_rng()
        )
        .is_ok()
    );
    assert_eq!(
        HandState::new(
            &spec,
            &[10_000; 6],
            0,
            1,
            Deck::from_deal_order(&exact[..29]),
            test_rng()
        )
        .err(),
        Some(poker_core::game::state::HandError::DeckExhausted)
    );
}

// --- 5. Full hands --------------------------------------------------------

#[test]
fn triple_draw_runs_three_draws_and_switches_tiers() {
    let hands = ["7d 5c 4h 3s 2c", "Ks Kd Kc Kh Qs"];
    let mut hand = start(
        &GameSpec::td27_fl(STAKES),
        &[10_000; 2],
        &hands,
        "9d 8s 7s 6d",
    );
    // Predraw is a small-bet street.
    assert_eq!(hand.legal_actions().unwrap().raise, fixed(200));
    play(&mut hand, &[Action::Call, Action::Check]);

    // Draw 1: seat 1 breaks the quads, seat 0 stands pat on 7-5-4-3-2.
    play(
        &mut hand,
        &[Action::Discard {
            cards: cards("Kc Kh"),
        }],
    );
    pat(&mut hand, 1);
    assert_eq!(hand.hole_cards(1), &cards("Ks Kd Qs 9d 8s")[..]);
    assert_eq!(hand.legal_actions().unwrap().bet, fixed(100), "small bet");
    play(&mut hand, &[Action::Check, Action::Check]);

    // Draw 2 onwards are big-bet streets.
    play(
        &mut hand,
        &[Action::Discard {
            cards: cards("Ks Kd"),
        }],
    );
    pat(&mut hand, 1);
    assert_eq!(hand.hole_cards(1), &cards("Qs 9d 8s 7s 6d")[..]);
    assert_eq!(hand.legal_actions().unwrap().bet, fixed(200), "big bet");
    play(&mut hand, &[Action::Check, Action::Check]);

    pat(&mut hand, 2);
    assert_eq!(hand.street(), (3, "draw3"));
    assert_eq!(hand.legal_actions().unwrap().bet, fixed(200), "big bet");
    play(&mut hand, &[Action::Check, Action::Check]);

    assert!(hand.is_over());
    assert_eq!(
        hand.settlement().unwrap().nets,
        vec![100, -100],
        "7-5-4-3-2 is the 2-7 nuts"
    );
    assert_eq!(
        hand.events()
            .iter()
            .filter(|e| matches!(e, Event::DrawResult { .. }))
            .count(),
        6,
        "two seats × three draw phases"
    );
    assert_distinct_live_cards(&hand);
    assert_conserved(&hand);
}

#[test]
fn badugi_four_card_hand_beats_a_three_card_finish() {
    let hands = ["As 2h 3d 4c", "Kd Kh Qs Jc"];
    let mut hand = start(&GameSpec::badugi_fl(STAKES), &[10_000; 2], &hands, "Qh 2s");
    assert_eq!(
        hand.legal_actions().unwrap().raise,
        fixed(200),
        "badugi deals four cards but bets like any limit game"
    );
    play(&mut hand, &[Action::Call, Action::Check]);

    // Seat 1 keeps drawing into a paired-suit hand.
    for discard in ["Kh", "Qh"] {
        assert_eq!(
            hand.legal_actions().unwrap().draw,
            Some(DrawBounds { max_discards: 4 })
        );
        play(
            &mut hand,
            &[Action::Discard {
                cards: cards(discard),
            }],
        );
        pat(&mut hand, 1);
        play(&mut hand, &[Action::Check, Action::Check]);
    }
    pat(&mut hand, 2);
    play(&mut hand, &[Action::Check, Action::Check]);

    // Seat 1 holds Kd Qs Jc 2s: two spades, so only a three-card badugi.
    assert_eq!(hand.hole_cards(1), &cards("Kd Qs Jc 2s")[..]);
    assert_eq!(
        hand.settlement().unwrap().nets,
        vec![100, -100],
        "A-2-3-4 rainbow beats any three-card badugi"
    );
    assert_conserved(&hand);
}

#[test]
fn five_card_draw_supports_a_check_raise_after_the_draw() {
    let hands = ["As Ks Qs Js Ts", "2c 3d 4h 5s 7c"];
    let mut hand = start(&GameSpec::fcd_nl(STAKES), &[10_000; 2], &hands, "8d 9h");
    play(&mut hand, &[Action::Call, Action::Check]);

    play(
        &mut hand,
        &[Action::Discard {
            cards: cards("5s 7c"),
        }],
    );
    pat(&mut hand, 1);

    // Post-draw: seat 1 checks, seat 0 bets, seat 1 check-raises.
    assert_eq!(hand.to_act(), Some(1));
    play(&mut hand, &[Action::Check]);
    assert_eq!(
        hand.legal_actions().unwrap().bet,
        Some(BetBounds {
            min_to: 100,
            max_to: 9_900
        })
    );
    play(&mut hand, &[Action::Bet { to: 300 }]);
    let la = hand.legal_actions().unwrap();
    assert_eq!(la.call, Some(300));
    assert_eq!(
        la.raise,
        Some(BetBounds {
            min_to: 600,
            max_to: 9_900
        })
    );
    play(&mut hand, &[Action::Raise { to: 900 }, Action::Call]);

    assert!(hand.is_over());
    assert_eq!(hand.settlement().unwrap().nets, vec![1_000, -1_000]);
    assert_conserved(&hand);
}

// --- 6. Deck exhaustion ---------------------------------------------------

/// Every seat still owing a draw discards its whole hand; returns
/// `(seat, discarded, drawn)` per draw.
fn draw_everything(hand: &mut HandState) -> Vec<(Seat, Vec<Card>, Vec<Card>)> {
    let mut log = Vec::new();
    while hand.legal_actions().is_some_and(|la| la.draw.is_some()) {
        let seat = hand.to_act().expect("legal_actions implies a seat");
        let discarded = hand.hole_cards(seat).to_vec();
        let ev = hand
            .apply(Action::Discard {
                cards: discarded.clone(),
            })
            .expect("discarding the whole hand is within `max`");
        log.push((seat, discarded, drawn_in(&ev)));
    }
    log
}

fn check_around(hand: &mut HandState) {
    while hand
        .legal_actions()
        .is_some_and(|la| la.check && la.draw.is_none())
    {
        hand.apply(Action::Check).unwrap();
    }
}

#[test]
fn deck_exhaustion_reshuffles_the_discard_pile() {
    // Six seats drawing five cards three times need 90 replacements from a
    // deck that has 22 left after the deal: the pile must recycle.
    let spec = GameSpec::td27_fl(STAKES);
    let (mut hand, _) =
        HandState::new(&spec, &[10_000; 6], 0, 1, Deck::standard(), test_rng()).unwrap();
    play(
        &mut hand,
        &[
            Action::Call,
            Action::Call,
            Action::Call,
            Action::Call,
            Action::Call,
            Action::Check,
        ],
    );

    let mut log = Vec::new();
    for _ in 0..3 {
        log.extend(draw_everything(&mut hand));
        check_around(&mut hand);
    }

    assert!(hand.is_over(), "the hand must complete");
    assert_eq!(log.len(), 18, "six seats × three draw phases");
    let total: usize = log.iter().map(|(_, _, drawn)| drawn.len()).sum();
    assert_eq!(
        total, 90,
        "every request was filled from a 52-card deck, so the pile recycled"
    );
    assert_eq!(
        hand.settlement().unwrap().showdown_seats.len(),
        6,
        "all six hands must still be distinct and live at showdown"
    );
    for (seat, discarded, drawn) in &log {
        assert_eq!(
            drawn.len(),
            discarded.len(),
            "seat {seat} was short-changed"
        );
        for card in drawn {
            assert!(
                !discarded.contains(card),
                "seat {seat} drew {card} straight back out of its own discards"
            );
        }
    }
    assert_distinct_live_cards(&hand);
    assert_conserved(&hand);
}

// --- 7. Redaction ---------------------------------------------------------

#[test]
fn draw_results_keep_the_drawn_cards_private() {
    let hands = ["Ah Kh Qh Jh Th", "2c 3d 4h 5s 7c"];
    let mut hand = start(&GameSpec::td27_fl(STAKES), &[10_000; 2], &hands, "9d 8s 6h");
    play(&mut hand, &[Action::Call, Action::Check]);
    let ev = play(
        &mut hand,
        &[Action::Discard {
            cards: cards("2c 3d"),
        }],
    );
    let result = &ev[0];
    assert_eq!(
        result,
        &Event::DrawResult {
            seat: 1,
            discarded: 2,
            drawn: cards("9d 8s"),
        }
    );
    assert_eq!(result.redacted_for(Some(1)), *result, "the owner sees all");
    for observer in [Some(0), None] {
        assert_eq!(
            result.redacted_for(observer),
            Event::DrawResult {
                seat: 1,
                discarded: 2,
                drawn: Vec::new(),
            },
            "the discard count stays public, the replacements do not"
        );
    }
}

// --- 8. Randomized sweep --------------------------------------------------

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
/// must hold for every draw game.
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
        let action = random_action(&hand, &la, &mut rng);
        events.extend(hand.apply(action).unwrap());
        steps += 1;
        assert!(steps < 1_000, "hand failed to terminate");
    }

    let settle = hand.settlement().unwrap();
    assert_eq!(settle.nets.iter().sum::<i64>(), 0, "nets must sum to zero");
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

#[test]
fn random_draw_hands_hold_every_invariant() {
    let specs = [
        GameSpec::td27_fl(STAKES),
        GameSpec::badugi_fl(STAKES),
        GameSpec::fcd_nl(STAKES),
    ];
    let (mut showdowns, mut fold_outs, mut draws) = (0, 0, 0);
    for spec in &specs {
        for seats in 2..=6usize {
            for depth in [5u64, 60] {
                for seed in 0..50u64 {
                    let stacks = vec![depth * STAKES.blinds().1; seats];
                    let key = seed * 977 + depth * 13 + seats as u64;
                    let events = random_hand(spec, &stacks, seed as usize % seats, key);
                    if events
                        .iter()
                        .any(|e| matches!(e, Event::ShowdownShow { .. }))
                    {
                        showdowns += 1;
                    } else {
                        fold_outs += 1;
                    }
                    draws += events
                        .iter()
                        .filter(
                            |e| matches!(e, Event::DrawResult { discarded, .. } if *discarded > 0),
                        )
                        .count();
                }
            }
        }
    }
    assert!(showdowns > 0 && fold_outs > 0, "{showdowns}/{fold_outs}");
    assert!(draws > 1_000, "only {draws} cards-changing draws happened");
}

#[test]
fn random_draw_hands_are_deterministic() {
    for spec in [
        GameSpec::td27_fl(STAKES),
        GameSpec::badugi_fl(STAKES),
        GameSpec::fcd_nl(STAKES),
    ] {
        for seats in 2..=6usize {
            for seed in 0..5u64 {
                let stacks = vec![60 * STAKES.blinds().1; seats];
                let a = random_hand(&spec, &stacks, 0, seed);
                let b = random_hand(&spec, &stacks, 0, seed);
                assert_eq!(a, b, "same seed must replay identically");
            }
        }
    }
}

/// Folded hands join the muck: under deck exhaustion their cards are
/// reshuffled back into circulation and can legally reappear in live hands.
/// Deterministic for the fixed seed, so the intersection assertion is
/// stable; folded seats never reach showdown, so no distinctness invariant
/// is affected.
#[test]
fn folded_hands_are_reshuffled_into_the_draw_pile() {
    let spec = GameSpec::td27_fl(STAKES);
    let (mut hand, setup) =
        HandState::new(&spec, &[100_000; 6], 0, 1, Deck::standard(), test_rng()).unwrap();

    let folder: Seat = 3; // first to act preflop with the button at 0
    let mucked: HashSet<Card> = setup
        .iter()
        .find_map(|e| match e {
            Event::DealHole { seat, cards, .. } if *seat == folder => Some(cards.clone()),
            _ => None,
        })
        .unwrap()
        .into_iter()
        .collect();

    let mut drawn_after_fold: HashSet<Card> = HashSet::new();
    let mut record = |events: &[Event]| {
        for e in events {
            if let Event::DrawResult { drawn, .. } = e {
                drawn_after_fold.extend(drawn.iter().copied());
            }
        }
    };

    while !hand.is_over() {
        let seat = hand.to_act().unwrap();
        let legal = hand.legal_actions().unwrap();
        let action = if legal.draw.is_some() {
            // Everyone draws the maximum every round to force exhaustion.
            Action::Discard {
                cards: hand.hole_cards(seat).to_vec(),
            }
        } else if seat == folder && legal.fold {
            Action::Fold
        } else if legal.check {
            Action::Check
        } else {
            Action::Call
        };
        let events = hand.apply(action).unwrap();
        record(&events);
    }

    let recycled: Vec<&Card> = mucked
        .iter()
        .filter(|c| drawn_after_fold.contains(c))
        .collect();
    assert!(
        !recycled.is_empty(),
        "none of the folded hand {mucked:?} re-entered play; drawn set had {} cards",
        drawn_after_fold.len()
    );
    let nets = hand.settlement().unwrap().nets.clone();
    assert_eq!(nets.iter().sum::<i64>(), 0);
}

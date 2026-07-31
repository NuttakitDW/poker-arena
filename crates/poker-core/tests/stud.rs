//! Scenario tests for the stud family: antes, bring-in, upcard action order
//! and seven-card showdowns.
//!
//! Decks are scripted with [`Deck::from_deal_order`]. A stud hand is dealt in
//! six batches: two down cards per seat, then one up card per seat on each of
//! third through sixth, then one down card per seat on seventh — every batch
//! in seat order starting left of the button. `stud_deck` lays cards out in
//! exactly that order from per-seat seven-card strings
//! (`"down down third fourth fifth sixth down"`).

use poker_core::card::{Card, Deck, parse_cards};
use poker_core::game::action::{Action, BetBounds, Chips, LegalActions, Seat};
use poker_core::game::event::{Event, PostKind};
use poker_core::game::spec::{GameSpec, Stakes};
use poker_core::game::state::{ActionError, HandState};
use poker_core::rng::Rng64;

fn test_rng() -> Rng64 {
    Rng64::from_seed_stream(0, 0)
}

/// Small bet 100, big bet 200; the stud constructors derive ante 20 and
/// bring-in 50 from these.
const STAKES: Stakes = Stakes::Blinds {
    small_blind: 50,
    big_blind: 100,
    ante: 0,
};

const ANTE: Chips = 20;
const BRING_IN: Chips = 50;
const SMALL_BET: Chips = 100;
const BIG_BET: Chips = 200;

fn cards(s: &str) -> Vec<Card> {
    parse_cards(s).unwrap()
}

/// Deck dealing `hands[seat]` in engine order. Every hand must list all seven
/// cards; folded seats simply stop consuming, which shifts later batches, so
/// tests that fold do not assert on cards dealt afterwards.
fn stud_deck(button: Seat, hands: &[&str]) -> Deck {
    let n = hands.len();
    let parsed: Vec<Vec<Card>> = hands.iter().map(|h| cards(h)).collect();
    assert!(
        parsed.iter().all(|h| h.len() == 7),
        "stud hands are 7 cards"
    );
    // Deal order: seats clockwise from the button's left, one batch per
    // street rather than one card at a time.
    let clockwise: Vec<&Vec<Card>> = (1..=n).map(|i| &parsed[(button + i) % n]).collect();
    let mut deal = Vec::new();
    for hand in &clockwise {
        deal.extend_from_slice(&hand[..2]);
    }
    for slot in 2..7 {
        deal.extend(clockwise.iter().map(|hand| hand[slot]));
    }
    Deck::from_deal_order(&deal)
}

fn start(spec: &GameSpec, stacks: &[Chips], button: Seat, hands: &[&str]) -> HandState {
    let deck = stud_deck(button, hands);
    HandState::new(spec, stacks, button, 1, deck, test_rng())
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

/// Everyone calls the bring-in, closing third street.
fn call_around(hand: &mut HandState, n: usize) {
    for _ in 0..n {
        hand.apply(Action::Call).expect("call must be legal");
    }
}

// --- 1. Bring-in selection ----------------------------------------------

#[test]
fn lowest_door_card_brings_in() {
    // Doors: seat 0 = 7h, seat 1 = 2d, seat 2 = Kc.
    let hands = [
        "Ah Ad 7h 9c Jd 4s 6h",
        "Kh Kd 2d Tc Qd 5s 8h",
        "Qh Qs Kc 3c 9d 2s 7s",
    ];
    let hand = start(&GameSpec::stud_fl(STAKES), &[1_000; 3], 0, &hands);
    assert_eq!(hand.street(), (1, "third"));
    assert_eq!(hand.to_act(), Some(1));
    assert_eq!(hand.legal_actions().unwrap().bring_in, Some(BRING_IN));
}

#[test]
fn bring_in_ties_break_by_suit() {
    // Two deuces show; clubs is the lower suit, so seat 2 posts.
    let hands = [
        "Ah Ad 7h 9c Jd 4s 6h",
        "Kh Kd 2d Tc Qd 5s 8h",
        "Qh Qs 2c 3c 9d 2s 7s",
    ];
    let hand = start(&GameSpec::stud_fl(STAKES), &[1_000; 3], 0, &hands);
    assert_eq!(hand.to_act(), Some(2));
}

#[test]
fn razz_brings_in_on_the_highest_door_card() {
    // Doors: Kc, Kd, 7h — razz takes the max by card index, so the king of
    // diamonds (higher suit) posts.
    let hands = [
        "Ah Ad Kc 9c Jd 4s 6h",
        "3h 3d Kd Tc Qd 5s 8h",
        "Qh Qs 7h 3c 9d 2s 7s",
    ];
    let hand = start(&GameSpec::razz_fl(STAKES), &[1_000; 3], 0, &hands);
    assert_eq!(hand.to_act(), Some(1));
    assert_eq!(hand.legal_actions().unwrap().bring_in, Some(BRING_IN));
}

// --- 2. The bring-in decision -------------------------------------------

#[test]
fn bring_in_decision_offers_only_posting_or_completing() {
    let hands = [
        "Ah Ad 7h 9c Jd 4s 6h",
        "Kh Kd 2d Tc Qd 5s 8h",
        "Qh Qs Kc 3c 9d 2s 7s",
    ];
    let mut hand = start(&GameSpec::stud_fl(STAKES), &[1_000; 3], 0, &hands);
    assert_eq!(
        hand.legal_actions().unwrap(),
        LegalActions {
            fold: false,
            check: false,
            call: None,
            bet: fixed(SMALL_BET),
            raise: None,
            bring_in: Some(BRING_IN),
            draw: None,
        }
    );
    for illegal in [
        Action::Fold,
        Action::Check,
        Action::Call,
        Action::Raise { to: SMALL_BET },
        Action::Bet { to: BRING_IN },
        Action::Bet { to: SMALL_BET + 1 },
        Action::Discard { cards: Vec::new() },
    ] {
        assert!(
            matches!(
                hand.apply(illegal.clone()),
                Err(ActionError::Illegal { .. })
            ),
            "{illegal:?} must be rejected at the bring-in"
        );
    }
    // Rejections left the decision exactly where it was.
    assert_eq!(hand.to_act(), Some(1));
    assert_eq!(hand.pot_total(), 3 * ANTE);
}

// --- 3. Completion and the raise cap ------------------------------------

#[test]
fn completion_is_the_first_wager_and_three_raises_reach_the_cap() {
    let hands = [
        "Ah Ad 7h 9c Jd 4s 6h",
        "Kh Kd 2d Tc Qd 5s 8h",
        "Qh Qs Kc 3c 9d 2s 7s",
    ];
    let mut hand = start(&GameSpec::stud_fl(STAKES), &[10_000; 3], 0, &hands);
    play(&mut hand, &[Action::BringIn]);
    assert_eq!(hand.street_commits(), &[0, BRING_IN, 0]);
    assert_eq!(hand.to_act(), Some(2));

    // Facing the bring-in, a raise *completes* to the small bet rather than
    // adding a full bet on top of it.
    let la = hand.legal_actions().unwrap();
    assert_eq!(la.call, Some(BRING_IN));
    assert_eq!(la.raise, fixed(SMALL_BET));
    assert!(matches!(
        hand.apply(Action::Raise {
            to: SMALL_BET + BRING_IN
        }),
        Err(ActionError::Illegal { .. })
    ));

    play(&mut hand, &[Action::Raise { to: SMALL_BET }]); // wager 1
    assert_eq!(hand.legal_actions().unwrap().raise, fixed(200));
    play(&mut hand, &[Action::Raise { to: 200 }]); // wager 2
    play(&mut hand, &[Action::Raise { to: 300 }]); // wager 3
    play(&mut hand, &[Action::Raise { to: 400 }]); // wager 4 = cap

    let la = hand.legal_actions().unwrap();
    assert_eq!(la.raise, None);
    assert_eq!(la.call, Some(200));
    assert!(la.fold);
    assert!(matches!(
        hand.apply(Action::Raise { to: 500 }),
        Err(ActionError::Illegal { .. })
    ));
}

#[test]
fn bring_in_seat_may_complete_directly() {
    let hands = [
        "Ah Ad 7h 9c Jd 4s 6h",
        "Kh Kd 2d Tc Qd 5s 8h",
        "Qh Qs Kc 3c 9d 2s 7s",
    ];
    let mut hand = start(&GameSpec::stud_fl(STAKES), &[10_000; 3], 0, &hands);
    let ev = play(&mut hand, &[Action::Bet { to: SMALL_BET }]);
    assert_eq!(
        ev[0],
        Event::Acted {
            seat: 1,
            action: Action::Bet { to: SMALL_BET },
            street_commit: SMALL_BET,
            all_in: false,
        }
    );
    // The completion is wager 1, so the next raise is a full bet on top.
    assert_eq!(hand.to_act(), Some(2));
    assert_eq!(hand.legal_actions().unwrap().raise, fixed(2 * SMALL_BET));
}

// --- 4. Short all-in bring-in -------------------------------------------

#[test]
fn short_all_in_bring_in_keeps_the_nominal_price() {
    // Seat 1 has 10 chips left after the ante — a third of the bring-in.
    let hands = [
        "Ah Ad 7h 9c Jd 4s 6h",
        "Kh Kd 2d Tc Qd 5s 8h",
        "Qh Qs Kc 3c 9d 2s 7s",
    ];
    let mut hand = start(&GameSpec::stud_fl(STAKES), &[10_000, 30, 10_000], 0, &hands);
    assert_eq!(hand.to_act(), Some(1));
    let la = hand.legal_actions().unwrap();
    assert_eq!(la.bring_in, Some(10));
    assert_eq!(la.bet, fixed(10), "the completion is capped by the stack");

    play(&mut hand, &[Action::BringIn]);
    assert!(hand.all_in()[1]);
    assert_eq!(hand.street_commits(), &[0, 10, 0]);
    // Everyone else still owes the full nominal bring-in.
    let la = hand.legal_actions().unwrap();
    assert_eq!(la.call, Some(BRING_IN));
    assert_eq!(la.raise, fixed(SMALL_BET));
}

// --- 5. Upcard action order ---------------------------------------------

/// Seat 0 shows a pair of sevens by fourth street; seat 1 a pair of aces by
/// fifth. Seat 2 never shows better than queen high.
const UPCARD_HANDS: [&str; 3] = [
    "3h 3s 7h 7d 2c 8s 9s",
    "5c 5d Ah Kd Ad 4h 6h",
    "9c 9d Qs Jc Tc 2d 3d",
];

#[test]
fn upcard_leader_opens_and_changes_street_to_street() {
    let mut hand = start(&GameSpec::stud_fl(STAKES), &[10_000; 3], 0, &UPCARD_HANDS);
    // Seat 0 has the seven of hearts showing, the lowest door card.
    assert_eq!(hand.to_act(), Some(0));
    play(&mut hand, &[Action::BringIn]);
    call_around(&mut hand, 2);

    // Fourth street: a pair of sevens showing beats ace-king.
    assert_eq!(hand.street(), (2, "fourth"));
    assert_eq!(hand.to_act(), Some(0));
    assert_eq!(hand.upcards()[0], cards("7h 7d"));
    assert_eq!(hand.upcards()[1], cards("Ah Kd"));
    play(&mut hand, &[Action::Check, Action::Check, Action::Check]);

    // Fifth street: seat 1 pairs aces and takes the lead.
    assert_eq!(hand.street(), (3, "fifth"));
    assert_eq!(hand.to_act(), Some(1));
    play(&mut hand, &[Action::Check, Action::Check, Action::Check]);

    // Sixth street: aces still lead.
    assert_eq!(hand.street(), (4, "sixth"));
    assert_eq!(hand.to_act(), Some(1));
}

#[test]
fn upcard_ties_break_left_of_the_button() {
    // Seats 0 and 2 both show ace-king. Seat 2 comes first clockwise from
    // the button, so it leads — seat 1, which is the left-of-button seat,
    // does not.
    let hands = [
        "3h 3s As Kc 5c 6s 7s",
        "9c 9d 2c 4d Tc Jd Qs",
        "5d 5h Ah Kd 9h Th 2h",
    ];
    let mut hand = start(&GameSpec::stud_fl(STAKES), &[10_000; 3], 0, &hands);
    assert_eq!(hand.to_act(), Some(1), "the deuce of clubs brings in");
    play(&mut hand, &[Action::BringIn]);
    call_around(&mut hand, 2);
    assert_eq!(hand.street(), (2, "fourth"));
    assert_eq!(hand.upcards()[0], cards("As Kc"));
    assert_eq!(hand.upcards()[2], cards("Ah Kd"));
    assert_eq!(hand.to_act(), Some(2));
}

#[test]
fn razz_gives_the_lead_to_the_lowest_board() {
    let hands = [
        "9h 9s 2c 3d 4c 5s 7s",
        "Kc Kd Qh Jd Th 8h 6h",
        "Tc Td 8c 9c Jc Qc 6c",
    ];
    let mut hand = start(&GameSpec::razz_fl(STAKES), &[10_000; 3], 0, &hands);
    // Razz: the highest door card brings in.
    assert_eq!(hand.to_act(), Some(1));
    play(&mut hand, &[Action::BringIn]);
    play(&mut hand, &[Action::Call, Action::Call]);
    // Fourth street: 3-2 showing is the best low board.
    assert_eq!(hand.street(), (2, "fourth"));
    assert_eq!(hand.to_act(), Some(0));
}

#[test]
fn an_all_in_leader_passes_the_open_clockwise() {
    // Seat 2 pairs aces on fifth street while all-in. The open therefore
    // goes to seat 0 — clockwise from the leader, *not* left of the button.
    let hands = [
        "3h 3s 2c 4d 5c 6s 7s",
        "9c 9d 8c 9h Tc Jd Qs",
        "5d 5h 3d Ah Ad Kh 2h",
    ];
    let mut hand = start(
        &GameSpec::stud_fl(STAKES),
        &[10_000, 10_000, 100],
        0,
        &hands,
    );
    assert_eq!(hand.to_act(), Some(0), "the deuce of clubs brings in");
    play(&mut hand, &[Action::BringIn]);
    call_around(&mut hand, 2);

    // Fourth street: ace-high showing leads, and seat 2 has 30 chips left.
    assert_eq!(hand.street(), (2, "fourth"));
    assert_eq!(hand.to_act(), Some(2));
    play(
        &mut hand,
        &[
            Action::Check,
            Action::Check,
            Action::Bet { to: SMALL_BET },
            Action::Call, // seat 2, all-in for 30
            Action::Call,
        ],
    );
    assert!(hand.all_in()[2]);

    assert_eq!(hand.street(), (3, "fifth"));
    assert_eq!(hand.upcards()[2], cards("3d Ah Ad"), "aces show best");
    assert_eq!(hand.to_act(), Some(0));
}

// --- 6. Bet tiers --------------------------------------------------------

#[test]
fn small_bets_through_fourth_then_big_bets() {
    let mut hand = start(&GameSpec::stud_fl(STAKES), &[10_000; 3], 0, &UPCARD_HANDS);
    play(&mut hand, &[Action::BringIn]);
    assert_eq!(hand.legal_actions().unwrap().raise, fixed(SMALL_BET));
    call_around(&mut hand, 2);

    assert_eq!(hand.street(), (2, "fourth"));
    assert_eq!(hand.legal_actions().unwrap().bet, fixed(SMALL_BET));
    play(&mut hand, &[Action::Check, Action::Check, Action::Check]);

    assert_eq!(hand.street(), (3, "fifth"));
    assert_eq!(hand.legal_actions().unwrap().bet, fixed(BIG_BET));
    play(&mut hand, &[Action::Bet { to: BIG_BET }]);
    assert_eq!(hand.legal_actions().unwrap().raise, fixed(2 * BIG_BET));
    play(&mut hand, &[Action::Call, Action::Call]);

    assert_eq!(hand.street(), (4, "sixth"));
    assert_eq!(hand.legal_actions().unwrap().bet, fixed(BIG_BET));
    play(&mut hand, &[Action::Check, Action::Check, Action::Check]);

    assert_eq!(hand.street(), (5, "seventh"));
    assert_eq!(hand.legal_actions().unwrap().bet, fixed(BIG_BET));
}

// --- 7. Full hands -------------------------------------------------------

/// Seat 0 makes aces, seat 1 kings; neither board pairs or flushes.
const HEADS_UP_HANDS: [&str; 2] = ["As Ad 7h 9c Jd 2s 4h", "Ks Kd 8h Tc Qd 3s 5h"];

#[test]
fn heads_up_seven_streets_to_showdown() {
    let mut hand = start(
        &GameSpec::stud_fl(STAKES),
        &[10_000, 10_000],
        0,
        &HEADS_UP_HANDS,
    );
    assert_eq!(hand.to_act(), Some(0), "the seven of hearts brings in");
    play(&mut hand, &[Action::BringIn, Action::Call]);
    for street in 2..=5 {
        assert_eq!(hand.street().0, street);
        play(&mut hand, &[Action::Check, Action::Check]);
    }

    assert!(hand.is_over());
    let shows: Vec<&Event> = hand
        .events()
        .iter()
        .filter(|e| matches!(e, Event::ShowdownShow { .. }))
        .collect();
    assert_eq!(
        shows,
        vec![
            // Down cards first (deal order), then the four up cards.
            &Event::ShowdownShow {
                seat: 1,
                cards: cards("Ks Kd 5h 8h Tc Qd 3s"),
                hi: poker_core::eval::evaluate(
                    poker_core::eval::EvalKind::High,
                    &cards("Ks Kd 5h 8h Tc Qd 3s")
                ),
                lo: None,
            },
            &Event::ShowdownShow {
                seat: 0,
                cards: cards("As Ad 4h 7h 9c Jd 2s"),
                hi: poker_core::eval::evaluate(
                    poker_core::eval::EvalKind::High,
                    &cards("As Ad 4h 7h 9c Jd 2s")
                ),
                lo: None,
            },
        ]
    );
    assert!(shows.iter().all(|e| match e {
        Event::ShowdownShow { cards, .. } => cards.len() == 7,
        _ => false,
    }));
    // Antes 40 plus the bring-in called for 50 each.
    assert_eq!(hand.settlement().unwrap().nets, vec![70, -70]);
    assert_eq!(
        hand.events()
            .iter()
            .filter(|e| matches!(e, Event::DealUp { .. }))
            .count(),
        8,
        "four up cards per seat, all public"
    );
    assert_conserved(&hand);
}

#[test]
fn stud8_splits_between_a_high_and_a_qualifying_low() {
    let hands = ["Ks Kd Kh 9c Jd 2s 4h", "Ac 2c 3d 4d 8h 9s Kc"];
    let mut hand = start(&GameSpec::stud8_fl(STAKES), &[10_000, 10_000], 0, &hands);
    assert_eq!(hand.to_act(), Some(1), "the three of diamonds brings in");
    play(&mut hand, &[Action::BringIn, Action::Call]);
    for _ in 0..4 {
        play(&mut hand, &[Action::Check, Action::Check]);
    }
    let awards = &hand.settlement().unwrap().awards;
    assert_eq!(
        awards
            .iter()
            .map(|a| (a.side, a.winners.clone()))
            .collect::<Vec<_>>(),
        vec![
            (poker_core::game::event::PotSide::Hi, vec![(0, 70)]),
            (poker_core::game::event::PotSide::Lo, vec![(1, 70)]),
        ],
        "trip kings scoop the high, the eight-low takes the other half"
    );
    assert_conserved(&hand);
}

#[test]
fn razz_awards_the_best_ace_to_five_low() {
    let hands = ["Ac 2d 3h 4s 5c 9d Kh", "2h 3c 4d 5h 7s 8c Kd"];
    let mut hand = start(&GameSpec::razz_fl(STAKES), &[10_000, 10_000], 0, &hands);
    // Razz brings in on the highest door: 4d beats 3h.
    assert_eq!(hand.to_act(), Some(1));
    play(&mut hand, &[Action::BringIn, Action::Call]);
    for _ in 0..4 {
        play(&mut hand, &[Action::Check, Action::Check]);
    }
    assert_eq!(
        hand.settlement().unwrap().nets,
        vec![70, -70],
        "the wheel beats seven-five"
    );
    assert_conserved(&hand);
}

// --- 8. Ante accounting --------------------------------------------------

#[test]
fn antes_are_pot_chips_and_an_ante_only_all_in_still_shows_down() {
    // Seat 1 owns exactly one ante, so it is all-in before a card is dealt
    // and never gets a bring-in decision — but it plays for the main pot.
    let hands = [
        "Kh Kd 7h 9c Jd 4s 6h",
        "Ah Ad 2d Tc Qd 5s As",
        "Qh Qs Kc 3c 9d 2s 7s",
    ];
    let mut hand = start(
        &GameSpec::stud_fl(STAKES),
        &[10_000, ANTE, 10_000],
        0,
        &hands,
    );
    assert!(hand.all_in()[1]);
    assert_eq!(hand.pot_total(), 3 * ANTE);
    assert_eq!(
        hand.street_commits(),
        &[0, 0, 0],
        "antes never count as street commitment"
    );
    // The lowest *actionable* door card posts; seat 1 cannot act at all.
    assert_eq!(hand.to_act(), Some(0));
    play(&mut hand, &[Action::BringIn, Action::Call]);
    for _ in 0..4 {
        play(&mut hand, &[Action::Check, Action::Check]);
    }

    let settle = hand.settlement().unwrap();
    assert_eq!(settle.showdown_seats, vec![1, 2, 0]);
    // Trip aces take the 60-chip main pot; the 100-chip side pot goes to the
    // better of the two remaining hands.
    assert_eq!(settle.awards[0].winners, vec![(1, 3 * ANTE)]);
    assert_eq!(settle.nets[1], 2 * ANTE as i64);
    assert_conserved(&hand);
}

#[test]
fn stud_keeps_the_strict_upfront_deck_check() {
    // Seven seats × seven cards is exactly what a stud run-out consumes, and
    // unlike draw games there is no discard pile to fall back on.
    let spec = GameSpec::stud_fl(STAKES);
    let full: Vec<Card> = (0..52).map(|i| Card::from_index(i).unwrap()).collect();
    assert!(
        HandState::new(
            &spec,
            &[1_000; 7],
            0,
            1,
            Deck::from_deal_order(&full[..49]),
            test_rng()
        )
        .is_ok()
    );
    assert_eq!(
        HandState::new(
            &spec,
            &[1_000; 7],
            0,
            1,
            Deck::from_deal_order(&full[..48]),
            test_rng()
        )
        .err(),
        Some(poker_core::game::state::HandError::DeckExhausted)
    );
}

// --- 9. Event-stream snapshot -------------------------------------------

fn kind_of(event: &Event) -> String {
    match event {
        Event::HandStart { .. } => "hand-start".into(),
        Event::Post { seat, kind, .. } => format!("post:{seat}:{kind:?}"),
        Event::DealHole { seat, count, .. } => format!("deal-hole:{seat}:{count}"),
        Event::StreetStart { street, label } => format!("street:{street}:{label}"),
        Event::DealCommunity { .. } => "deal-community".into(),
        Event::DealUp { seat, cards } => format!("deal-up:{seat}:{}", cards.len()),
        Event::Acted { seat, action, .. } => format!("acted:{seat}:{}", action_tag(action)),
        Event::DrawResult {
            seat, discarded, ..
        } => format!("draw:{seat}:{discarded}"),
        Event::ShowdownShow { seat, cards, .. } => format!("show:{seat}:{}", cards.len()),
        Event::PotAwarded { pot, .. } => format!("award:{pot}"),
        Event::HandEnd { .. } => "hand-end".into(),
        // Deserialization-only forward-compat variant; the engine never
        // emits it, so reaching it here is a bug worth failing loudly on.
        Event::Unknown => unreachable!("the engine never emits Event::Unknown"),
    }
}

fn action_tag(action: &Action) -> &'static str {
    match action {
        Action::Fold => "fold",
        Action::Check => "check",
        Action::Call => "call",
        Action::Bet { .. } => "bet",
        Action::Raise { .. } => "raise",
        Action::BringIn => "bring-in",
        Action::Discard { .. } => "discard",
    }
}

#[test]
fn scripted_hand_emits_a_stable_event_sequence() {
    let mut hand = start(
        &GameSpec::stud_fl(STAKES),
        &[10_000, 10_000],
        0,
        &HEADS_UP_HANDS,
    );
    play(&mut hand, &[Action::BringIn, Action::Call]);
    for _ in 0..4 {
        play(&mut hand, &[Action::Check, Action::Check]);
    }
    let stream: Vec<String> = hand.events().iter().map(kind_of).collect();
    assert_eq!(
        stream,
        vec![
            "hand-start",
            "post:1:Ante",
            "post:0:Ante",
            "street:0:deal",
            "deal-hole:1:2",
            "deal-hole:0:2",
            "street:1:third",
            "deal-up:1:1",
            "deal-up:0:1",
            "acted:0:bring-in",
            "acted:1:call",
            "street:2:fourth",
            "deal-up:1:1",
            "deal-up:0:1",
            "acted:1:check",
            "acted:0:check",
            "street:3:fifth",
            "deal-up:1:1",
            "deal-up:0:1",
            "acted:1:check",
            "acted:0:check",
            "street:4:sixth",
            "deal-up:1:1",
            "deal-up:0:1",
            "acted:1:check",
            "acted:0:check",
            "street:5:seventh",
            "deal-hole:1:1",
            "deal-hole:0:1",
            "acted:1:check",
            "acted:0:check",
            "show:1:7",
            "show:0:7",
            "award:0",
            "hand-end",
        ]
    );
    assert_eq!(
        hand.events()[1],
        Event::Post {
            seat: 1,
            kind: PostKind::Ante,
            amount: ANTE,
            all_in: false,
        }
    );
}

// --- 10. Randomized sweep ------------------------------------------------

fn random_action(la: &LegalActions, rng: &mut Rng64) -> Action {
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
    if let Some(b) = la.bet {
        choices.push(Action::Bet { to: b.min_to });
    }
    if let Some(b) = la.raise {
        choices.push(Action::Raise { to: b.max_to });
    }
    if la.bring_in.is_some() {
        choices.push(Action::BringIn);
    }
    choices.swap_remove(rng.below(choices.len() as u64) as usize)
}

#[test]
fn random_stud_hands_conserve_chips_and_terminate() {
    let mut showdowns = 0;
    let mut fold_outs = 0;
    for spec in [
        GameSpec::stud_fl(STAKES),
        GameSpec::stud8_fl(STAKES),
        GameSpec::razz_fl(STAKES),
    ] {
        for seats in 2..=7usize {
            for seed in 0..12u64 {
                let mut rng = Rng64::from_seed_stream(seed * 31 + seats as u64, 5);
                let deck = Deck::shuffled(&mut rng);
                let stacks = vec![if seed % 2 == 0 { 400 } else { 5_000 }; seats];
                let (mut hand, _) = HandState::new(
                    &spec,
                    &stacks,
                    seed as usize % seats,
                    seed,
                    deck,
                    test_rng(),
                )
                .unwrap();
                let mut steps = 0;
                while let Some(la) = hand.legal_actions() {
                    hand.apply(random_action(&la, &mut rng)).unwrap();
                    steps += 1;
                    assert!(steps < 500, "hand failed to terminate");
                }
                let settle = hand.settlement().unwrap();
                assert_eq!(settle.nets.iter().sum::<i64>(), 0);
                for (seat, &net) in settle.nets.iter().enumerate() {
                    assert!(stacks[seat] as i64 + net >= 0, "seat {seat} went negative");
                }
                if settle.showdown_seats.is_empty() {
                    fold_outs += 1;
                } else {
                    showdowns += 1;
                    for event in hand.events() {
                        if let Event::ShowdownShow { cards, .. } = event {
                            assert_eq!(cards.len(), 7, "stud shows down seven cards");
                        }
                    }
                }
            }
        }
    }
    assert!(showdowns > 0 && fold_outs > 0, "{showdowns}/{fold_outs}");
}

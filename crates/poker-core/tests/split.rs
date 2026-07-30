//! Scenario tests for the four split-pot variants: `badacey-fl`,
//! `badeucy-fl` and `archie-fl` (five-card triple draw) plus `bigo-pl`
//! (five-card Omaha hi-lo).
//!
//! Decks are scripted with [`Deck::from_deal_order`], exactly as in
//! `draw.rs` and `omaha.rs`: the engine deals hole cards one full batch per
//! seat, in seat order starting left of the button, then one community batch
//! per street. `deck_for`'s `holes` slice is indexed by *seat number*
//! (`holes[0]` is seat 0's cards); the function reorders for dealing.
//!
//! The four games between them cover every branch of the award rule:
//! badacey/badeucy always split (both evaluators are total), archie can
//! scoop from either side or split evenly when nobody qualifies, and big O
//! is the hi-lo community game with five hole cards.

use std::collections::HashSet;

use poker_core::card::{Card, Deck, parse_cards};
use poker_core::eval::{self, HandClass, HandValue, HoleUsage};
use poker_core::game::action::{Action, BetBounds, Chips, LegalActions, Seat};
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
};

fn cards(s: &str) -> Vec<Card> {
    parse_cards(s).unwrap()
}

/// Deck dealing `holes[seat]` to each seat in engine order, then `rest`
/// (community cards, or draw replacements in the order the draw phases
/// consume them).
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

/// Run the rest of the hand passively: stand pat on every draw, check when
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
    // Every chip that went into a pot comes back out of one: the awards add
    // up to the pot total the state machine built them from.
    let awarded: Chips = settle
        .awards
        .iter()
        .flat_map(|a| a.winners.iter().map(|&(_, c)| c))
        .sum();
    assert_eq!(awarded, hand.pot_total(), "awards must exhaust the pot");
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
fn shown(hand: &HandState, seat: Seat) -> (Option<HandValue>, Option<HandValue>) {
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

// --- 1. badacey: both halves always exist ---------------------------------

#[test]
fn badacey_pot_always_splits() {
    // Seat 1 holds the nut badugi (A-2-3-4 rainbow) but a king-high A-5
    // low; seat 0 is four-flushed into a two-card badugi but holds the
    // wheel. Neither evaluator can fail to qualify, so the pot must split.
    let holes = ["Ad 2c 3c 4c 5c", "Ac 2d 3h 4s Kc"];
    let mut hand = start(&GameSpec::badacey_fl(STAKES), &[10_000; 2], &holes, "");
    check_down(&mut hand);

    let (hi0, lo0) = shown(&hand, 0);
    let (hi1, lo1) = shown(&hand, 1);
    assert_eq!(hi1, Some(eval::badugi(&cards("Ac 2d 3h 4s"))));
    assert_eq!(hi0.unwrap().0 >> 20, 2, "seat 0 has a two-card badugi only");
    assert_eq!(lo0, Some(eval::ace_to_five_low(&cards("Ad 2c 3c 4c 5c"))));
    assert!(hi1 > hi0, "nut badugi wins the hi half");
    assert!(lo0 > lo1, "the wheel wins the lo half");

    assert_eq!(
        awards(&hand),
        vec![
            awarded(0, PotSide::Hi, &[(1, 100)]),
            awarded(0, PotSide::Lo, &[(0, 100)]),
        ]
    );
    assert_eq!(hand.settlement().unwrap().nets, vec![0, 0]);
    assert_distinct_live_cards(&hand);
    assert_conserved(&hand);
}

#[test]
fn badacey_one_seat_can_scoop_both_halves() {
    // Same split machinery, but seat 0 wins both sides: two awards still
    // land, both on the same seat.
    let holes = ["Ac 2d 3h 4s 9c", "Kc Qd Jh Ts 9s"];
    let mut hand = start(&GameSpec::badacey_fl(STAKES), &[10_000; 2], &holes, "");
    check_down(&mut hand);

    assert_eq!(
        awards(&hand),
        vec![
            awarded(0, PotSide::Hi, &[(0, 100)]),
            awarded(0, PotSide::Lo, &[(0, 100)]),
        ]
    );
    assert_eq!(hand.settlement().unwrap().nets, vec![100, -100]);
    assert_conserved(&hand);
}

// --- 2. badacey: the badugi is the best four of five ----------------------

#[test]
fn badacey_badugi_uses_best_four_of_five() {
    // As and 2s clash. Dropping the deuce (A-5-4-3) beats dropping the ace
    // (5-4-3-2) with aces low, so the engine must pick the right subset.
    let holes = ["As 2s 3d 4h 5c", "Kc Kd Kh Ks 9c"];
    let mut hand = start(&GameSpec::badacey_fl(STAKES), &[10_000; 2], &holes, "");
    check_down(&mut hand);

    let (hi0, _) = shown(&hand, 0);
    assert_eq!(
        hi0,
        Some(eval::badugi(&cards("As 3d 4h 5c"))),
        "the best badugi drops the deuce, not the ace"
    );
    assert_ne!(hi0, Some(eval::badugi(&cards("2s 3d 4h 5c"))));
    assert_eq!(hi0.unwrap().0 >> 20, 4, "four-card badugi");

    // Four kings plus a club: only a nine-plus-king two-card badugi exists.
    let (hi1, _) = shown(&hand, 1);
    assert_eq!(hi1.unwrap().0 >> 20, 2);
    assert_eq!(hi1, Some(eval::badugi(&cards("Kd 9c"))));

    // Seat 0 also holds the wheel, so it scoops both halves.
    assert_eq!(hand.settlement().unwrap().nets, vec![100, -100]);
    assert_conserved(&hand);
}

// --- 3. badeucy: aces are high in both halves -----------------------------

#[test]
fn badeucy_aces_are_high_in_both_halves() {
    // Seat 0: A-2-3-4 rainbow badugi (its only four-card badugi, since the
    //         fifth card duplicates a suit) and A-5-4-3-2 for the 2-7 side.
    // Seat 1: 5-4-3-2 rainbow — the ace-high nuts — but a pair for the low.
    // Seat 2: only a three-card badugi, but a T-9-8-7-5 for the low.
    let holes = ["As 2c 3d 4h 5h", "2h 3c 4s 5d 5c", "Th 9s 8c 7h 5s"];
    let mut hand = start(&GameSpec::badeucy_fl(STAKES), &[10_000; 3], &holes, "");
    check_down(&mut hand);

    let (hi0, lo0) = shown(&hand, 0);
    let (hi1, lo1) = shown(&hand, 1);
    let (hi2, lo2) = shown(&hand, 2);

    assert_eq!(hi0, Some(eval::badugi_ace_high(&cards("As 2c 3d 4h"))));
    assert_eq!(hi1, Some(eval::badugi_ace_high(&cards("2h 3c 4s 5d"))));
    assert_eq!(hi0.unwrap().0 >> 20, 4);
    assert_eq!(hi2.unwrap().0 >> 20, 3, "only three suits in seat 2's hand");
    assert!(
        hi1 > hi0,
        "5-4-3-2 rainbow beats A-2-3-4 rainbow when aces are high"
    );
    assert!(hi0 > hi2, "a four-card badugi still beats a three-card one");

    assert_eq!(
        lo2,
        Some(eval::deuce_to_seven_low(&cards("Th 9s 8c 7h 5s")))
    );
    assert!(
        lo2 > lo0,
        "a ten-low beats A-5-4-3-2, which is merely ace-high at 2-7"
    );
    assert!(lo0 > lo1, "any no-pair hand beats a pair of fives");

    // 300 chips: 150 to the badugi (seat 1), 150 to the 2-7 low (seat 2).
    assert_eq!(
        awards(&hand),
        vec![
            awarded(0, PotSide::Hi, &[(1, 150)]),
            awarded(0, PotSide::Lo, &[(2, 150)]),
        ]
    );
    assert_eq!(hand.settlement().unwrap().nets, vec![-100, 50, 50]);
    assert_distinct_live_cards(&hand);
    assert_conserved(&hand);
}

// --- 4. archie: one qualifying side scoops --------------------------------

#[test]
fn archie_high_qualifier_scoops_when_no_low_qualifies() {
    // Seat 0's pair of sixes is exactly on the qualifying bar; nobody has an
    // eight-or-better low, so the hi side takes the whole pot.
    let holes = ["6c 6d Kh Qs 9c", "Kd Qc Jh 9s 2d"];
    let mut hand = start(&GameSpec::archie_fl(STAKES), &[10_000; 2], &holes, "");
    check_down(&mut hand);

    let (hi0, lo0) = shown(&hand, 0);
    let (hi1, lo1) = shown(&hand, 1);
    assert_eq!(hi0, Some(eval::high(&cards("6c 6d Kh Qs 9c"))));
    assert_eq!(
        hi1, None,
        "king-high never qualifies for the sixes-or-better"
    );
    assert_eq!((lo0, lo1), (None, None));

    assert_eq!(awards(&hand), vec![awarded(0, PotSide::Whole, &[(0, 200)])]);
    assert_eq!(hand.settlement().unwrap().nets, vec![100, -100]);
    assert_conserved(&hand);
}

#[test]
fn archie_low_qualifier_scoops_when_no_high_qualifies() {
    // The mirror: seat 0's 8-6-4-3-2 qualifies for low but is only an
    // eight-high no-pair hand, which never qualifies for high.
    let holes = ["8c 6d 4h 3s 2c", "Kd Qh Js 9c 7d"];
    let mut hand = start(&GameSpec::archie_fl(STAKES), &[10_000; 2], &holes, "");
    check_down(&mut hand);

    let (hi0, lo0) = shown(&hand, 0);
    let (hi1, lo1) = shown(&hand, 1);
    assert_eq!((hi0, hi1), (None, None), "no pair of sixes anywhere");
    assert_eq!(
        lo0,
        Some(eval::eight_or_better(&cards("8c 6d 4h 3s 2c")).unwrap())
    );
    assert_eq!(lo1, None);

    assert_eq!(awards(&hand), vec![awarded(0, PotSide::Whole, &[(0, 200)])]);
    assert_eq!(hand.settlement().unwrap().nets, vec![100, -100]);
    assert_conserved(&hand);
}

// --- 5. archie: nobody qualifies ------------------------------------------

#[test]
fn archie_neither_qualifies_splits_evenly() {
    // Pairs of fives and fours are below the sixes bar, king-high is a
    // no-pair hand, and no hand has five distinct cards at eight or lower.
    // Seat 1 folds its small blind, leaving a 350-chip pot three ways.
    let holes = [
        "5c 5d Kh Qs 9c",
        "2h 2s 3c 3d 4h",
        "4c 4d Jh Ts 9d",
        "Kc Qd Jc 9s 7c",
    ];
    let mut hand = start(&GameSpec::archie_fl(STAKES), &[10_000; 4], &holes, "");
    play(
        &mut hand,
        &[Action::Call, Action::Call, Action::Fold, Action::Check],
    );
    check_down(&mut hand);

    for seat in [0, 2, 3] {
        assert_eq!(shown(&hand, seat), (None, None), "seat {seat} qualifies");
    }

    // 350 / 3 = 116 with two chips over; `odd_chip_order` is [1, 2, 3, 0],
    // so seats 2 and 3 take the extras.
    assert_eq!(
        awards(&hand),
        vec![awarded(0, PotSide::Whole, &[(0, 116), (2, 117), (3, 117)])]
    );
    assert_eq!(hand.settlement().unwrap().nets, vec![16, -50, 17, 17]);
    assert_distinct_live_cards(&hand);
    assert_conserved(&hand);
}

// --- 6. archie: both sides qualify ----------------------------------------

#[test]
fn archie_both_qualify_splits() {
    // Seat 1 is all-in for 25 on the blind, so there are two pots. Trips
    // take every hi side, the 7-5-4-3-A takes every lo side, and the odd
    // chip of the 75-chip main pot goes to hi.
    let holes = ["8c 8d 8h Ks 2c", "6c 6d Qh Js 9c", "7c 5d 4h 3s Ac"];
    let mut hand = start(
        &GameSpec::archie_fl(STAKES),
        &[10_000, 25, 10_000],
        &holes,
        "",
    );
    check_down(&mut hand);

    let (hi0, lo0) = shown(&hand, 0);
    let (hi1, lo1) = shown(&hand, 1);
    let (hi2, lo2) = shown(&hand, 2);
    assert_eq!(hi0.unwrap().high_class(), HandClass::Trips);
    assert_eq!(hi1.unwrap().high_class(), HandClass::OnePair);
    assert!(hi0 > hi1, "trips beat the qualifying pair of sixes");
    assert_eq!(
        hi2, None,
        "a seven-high no-pair hand never qualifies for hi"
    );
    assert_eq!((lo0, lo1), (None, None));
    assert_eq!(
        lo2,
        Some(eval::eight_or_better(&cards("7c 5d 4h 3s Ac")).unwrap())
    );

    assert_eq!(
        awards(&hand),
        vec![
            awarded(0, PotSide::Hi, &[(0, 38)]),
            awarded(0, PotSide::Lo, &[(2, 37)]),
            awarded(1, PotSide::Hi, &[(0, 75)]),
            awarded(1, PotSide::Lo, &[(2, 75)]),
        ]
    );
    assert_eq!(hand.settlement().unwrap().nets, vec![13, -25, 12]);
    assert_distinct_live_cards(&hand);
    assert_conserved(&hand);
}

// --- 7. big O: exactly two of five hole cards -----------------------------

#[test]
fn bigo_exactly_two_with_five_hole_cards() {
    // Four hearts on the board. Seat 0 holds exactly one of them, so its
    // flush needs four board cards — one too many for `ExactlyTwo`. Seat 2
    // holds two hearts and makes the real flush. Seat 1 folds its blind.
    let holes = ["Ah 4d 2c 5s 9c", "9d 9s 4c 4h 5d", "Th 2h Jc 6d 6c"];
    let board = "Kh Qh 8h 3h 7s";
    let mut hand = start(&GameSpec::bigo_pl(STAKES), &[10_000; 3], &holes, board);
    play(&mut hand, &[Action::Call, Action::Fold, Action::Check]);
    check_down(&mut hand);

    let (hi0, lo0) = shown(&hand, 0);
    let (hi2, lo2) = shown(&hand, 2);

    // Seat 0: the best two hole cards plus three board cards, no flush.
    assert_eq!(hi0, Some(eval::high(&cards("Ah 9c Kh Qh 8h"))));
    assert_eq!(hi0.unwrap().high_class(), HandClass::HighCard);
    let mut pool = cards("Ah 4d 2c 5s 9c");
    pool.extend(cards(board));
    assert_eq!(
        eval::high(&pool).high_class(),
        HandClass::Flush,
        "the ExactlyTwo rule, not the card pool, is what blocks seat 0's flush"
    );
    assert_eq!(
        eval::best_with_usage(
            eval::EvalKind::High,
            HoleUsage::ExactlyTwo,
            &cards("Ah 4d 2c 5s 9c"),
            &cards(board)
        ),
        hi0
    );

    // Seat 2: two hole hearts plus the three best board hearts.
    assert_eq!(hi2, Some(eval::high(&cards("Th 2h Kh Qh 8h"))));
    assert_eq!(hi2.unwrap().high_class(), HandClass::Flush);
    assert!(hi2 > hi0);

    // Both lows qualify off the same three board cards; seat 0's A-2 beats
    // seat 2's 2-6.
    assert_eq!(
        lo0,
        Some(eval::eight_or_better(&cards("Ah 2c 8h 3h 7s")).unwrap())
    );
    assert_eq!(
        lo2,
        Some(eval::eight_or_better(&cards("2h 6d 8h 3h 7s")).unwrap())
    );
    assert!(lo0 > lo2);

    // 250 chips (seat 1's dead blind included): 125 each way.
    assert_eq!(
        awards(&hand),
        vec![
            awarded(0, PotSide::Hi, &[(2, 125)]),
            awarded(0, PotSide::Lo, &[(0, 125)]),
        ]
    );
    assert_eq!(hand.settlement().unwrap().nets, vec![25, -50, 25]);
    assert_distinct_live_cards(&hand);
    assert_conserved(&hand);
}

// --- 8. big O: nine seats fit in one deck ---------------------------------

#[test]
fn bigo_nine_handed_deals_from_one_deck() {
    // 9 × 5 hole cards + a five-card board = 50 of 52.
    let spec = GameSpec::bigo_pl(STAKES);
    let mut rng = Rng64::from_seed_stream(7, 7);
    let deck = Deck::shuffled(&mut rng);
    let (mut hand, _) =
        HandState::new(&spec, &[10_000; 9], 0, 1, deck, test_rng()).expect("50 <= 52");
    check_down(&mut hand);

    let settle = hand.settlement().unwrap();
    assert_eq!(settle.showdown_seats.len(), 9, "everyone checks it down");
    assert_eq!(hand.board().len(), 5);
    assert_distinct_live_cards(&hand);
    assert_conserved(&hand);

    // One card short of the 50 the run-out needs is rejected up front.
    let exact: Vec<Card> = (0..49).map(|i| Card::from_index(i).unwrap()).collect();
    assert_eq!(
        HandState::new(
            &spec,
            &[10_000; 9],
            0,
            1,
            Deck::from_deal_order(&exact),
            test_rng()
        )
        .err(),
        Some(poker_core::game::state::HandError::DeckExhausted)
    );
}

// --- 9. randomized sweep over the three new draw games --------------------

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
/// must hold for every split-pot draw game.
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

fn split_draw_specs() -> [GameSpec; 3] {
    [
        GameSpec::badacey_fl(STAKES),
        GameSpec::badeucy_fl(STAKES),
        GameSpec::archie_fl(STAKES),
    ]
}

#[test]
fn random_split_draw_hands_hold_every_invariant() {
    let (mut hi_lo_splits, mut wholes, mut fold_outs) = (0, 0, 0);
    for spec in &split_draw_specs() {
        for seats in 2..=6usize {
            for depth in [5u64, 60] {
                for seed in 0..40u64 {
                    let stacks = vec![depth * STAKES.blinds().1; seats];
                    let key = seed * 977 + depth * 13 + seats as u64;
                    let events = random_hand(spec, &stacks, seed as usize % seats, key);
                    if !events
                        .iter()
                        .any(|e| matches!(e, Event::ShowdownShow { .. }))
                    {
                        fold_outs += 1;
                        continue;
                    }
                    for e in &events {
                        match e {
                            Event::PotAwarded {
                                side: PotSide::Hi, ..
                            } => hi_lo_splits += 1,
                            Event::PotAwarded {
                                side: PotSide::Whole,
                                ..
                            } => wholes += 1,
                            _ => {}
                        }
                    }
                }
            }
        }
    }
    assert!(fold_outs > 0, "no hand ever folded out");
    assert!(hi_lo_splits > 0, "no pot ever split hi/lo");
    // Archie is the only one of the three that can award a whole pot at
    // showdown (one side qualifying, or neither).
    assert!(wholes > 0, "archie never scooped or split evenly");
}

#[test]
fn random_split_draw_hands_are_deterministic() {
    for spec in &split_draw_specs() {
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

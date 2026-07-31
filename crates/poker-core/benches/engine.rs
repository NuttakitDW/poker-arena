//! Full-hand `HandState` cost, documented for solver consumers.
//!
//! Three fixed scenarios driven by a deterministic check-when-free-else-call
//! policy (never bets, never raises, never folds — there is always a free
//! check or a call on offer under this policy, so the hand always reaches
//! showdown or a stand pat). Decks are cloned per iteration from a template
//! shuffled once with a fixed seed, so every iteration deals identical cards
//! — required for stable numbers, since a fresh shuffle would vary how many
//! streets/draws actually run. Returned events are collected into a `Vec`
//! and `black_box`ed, mirroring the allocation `run_match` does per hand.
//!
//! ## Baseline (Apple Silicon, 2026-07)
//!
//! | benchmark | time |
//! |---|---|
//! | `holdem_nl_heads_up_hand` | ~6.5 µs |
//! | `holdem_fl_six_max_hand` | ~16 µs |
//! | `triple_draw_hand` | ~4.6 µs |
//!
//! Informational only — a review-time drift check, not a CI gate. Re-run
//! `cargo bench -p poker-core` after touching `game/state.rs` and compare.

use criterion::{Criterion, criterion_group, criterion_main};
use poker_core::card::Deck;
use poker_core::game::{Action, Chips, Event, GameSpec, HandState, LegalActions, Stakes};
use poker_core::rng::Rng64;
use std::hint::black_box;

const STAKES: Stakes = Stakes::Blinds {
    small_blind: 50,
    big_blind: 100,
    ante: 0,
};

/// Check when free, else call; on a draw street, discard the entire hand
/// (maximum replacement work). Never bets, raises, or folds.
fn policy_action(hand: &HandState, la: &LegalActions) -> Action {
    if la.draw.is_some() {
        let seat = hand
            .to_act()
            .expect("a draw decision implies a seat to act");
        Action::Discard {
            cards: hand.hole_cards(seat).to_vec(),
        }
    } else if la.check {
        Action::Check
    } else if la.call.is_some() {
        Action::Call
    } else {
        unreachable!("check/call/discard covers every decision this policy meets")
    }
}

/// Deal a hand from `deck` and drive it to completion with `policy_action`,
/// collecting every event exactly as `run_match` does per hand.
fn play_hand(spec: &GameSpec, stacks: &[Chips], deck: Deck) -> Vec<Event> {
    let (mut hand, mut events) =
        HandState::new(spec, stacks, 0, 0, deck, Rng64::from_seed_stream(1, 1))
            .expect("fixed bench scenarios are valid hands");
    while let Some(la) = hand.legal_actions() {
        let action = policy_action(&hand, &la);
        events.extend(hand.apply(action).expect("policy only plays legal actions"));
    }
    events
}

fn holdem_nl_heads_up_hand(c: &mut Criterion) {
    let spec = GameSpec::holdem_nl(STAKES);
    let template = Deck::shuffled(&mut Rng64::from_seed_stream(42, 0));
    let stacks = [10_000u64, 10_000];
    c.bench_function("holdem_nl_heads_up_hand", |b| {
        b.iter(|| black_box(play_hand(&spec, &stacks, template.clone())))
    });
}

fn holdem_fl_six_max_hand(c: &mut Criterion) {
    let spec = GameSpec::holdem_fl(STAKES);
    let template = Deck::shuffled(&mut Rng64::from_seed_stream(42, 0));
    let stacks = [10_000u64; 6];
    c.bench_function("holdem_fl_six_max_hand", |b| {
        b.iter(|| black_box(play_hand(&spec, &stacks, template.clone())))
    });
}

/// Heads-up 2-7 triple draw: every draw phase discards all 5 cards, the
/// maximum replacement work the deal-exhaustion/reshuffle path can face.
fn triple_draw_hand(c: &mut Criterion) {
    let spec = GameSpec::td27_fl(STAKES);
    let template = Deck::shuffled(&mut Rng64::from_seed_stream(7, 0));
    let stacks = [10_000u64, 10_000];
    c.bench_function("triple_draw_hand", |b| {
        b.iter(|| black_box(play_hand(&spec, &stacks, template.clone())))
    });
}

criterion_group!(
    benches,
    holdem_nl_heads_up_hand,
    holdem_fl_six_max_hand,
    triple_draw_hand
);
criterion_main!(benches);

//! Evaluator cost, documented for solver consumers.
//!
//! Six fixed, representative inputs — no fuzzing, no sweep over card space
//! (the C(52,5) frequency sweep lives in the test suite, not here). Inputs
//! are built once outside the timed loop; `black_box` wraps both the input
//! references and the returned `HandValue`(s) so the optimizer cannot fold
//! the call away.
//!
//! ## Baseline (Apple Silicon, 2026-07)
//!
//! | benchmark | time |
//! |---|---|
//! | `rank_five` | ~97 ns |
//! | `high_best_5_of_7` | ~2.1 µs |
//! | `omaha_exactly_two` | ~9.0 µs |
//! | `bigo_exactly_two_hilo` | ~40 µs |
//! | `badugi_of_five` | ~1.7 µs |
//! | `eight_or_better_7` | ~2.1 µs |
//!
//! These numbers are informational — a reviewer's tripwire for "did this
//! change get 10x slower," not a gate. Nothing in CI enforces them; re-run
//! `cargo bench -p poker-core` and eyeball the diff when touching `eval/`.

use criterion::{Criterion, criterion_group, criterion_main};
use poker_core::card::{Card, parse_cards};
use poker_core::eval::{self, EvalKind, HoleUsage};
use std::hint::black_box;

fn cards(s: &str) -> Vec<Card> {
    parse_cards(s).unwrap()
}

/// One 5-card classify: two pair, aces over kings.
fn rank_five(c: &mut Criterion) {
    let hand = cards("Ah Ad Kh Kd 2c");
    c.bench_function("rank_five", |b| {
        b.iter(|| black_box(eval::high(black_box(&hand))))
    });
}

/// Best high hand from a typical 7-card hold'em showdown (C(7,5)=21 combos).
fn high_best_5_of_7(c: &mut Criterion) {
    let hand = cards("Ah Kh Qh Jd Th 9c 2d");
    c.bench_function("high_best_5_of_7", |b| {
        b.iter(|| black_box(eval::high(black_box(&hand))))
    });
}

/// Omaha's exactly-two-of-four-hole rule: C(4,2)*C(5,3)=60 candidate hands.
fn omaha_exactly_two(c: &mut Criterion) {
    let hole = cards("Ah Kh Qd Jd");
    let board = cards("Th 9h 8d 3c 2s");
    c.bench_function("omaha_exactly_two", |b| {
        b.iter(|| {
            black_box(eval::best_with_usage(
                EvalKind::High,
                HoleUsage::ExactlyTwo,
                black_box(&hole),
                black_box(&board),
            ))
        })
    });
}

/// Worst case: Big O hi-lo showdown, 5 hole + 5 board. Each side runs
/// `best_with_usage(ExactlyTwo)` independently — C(5,2)*C(5,3)=100 candidate
/// hands per side, 200 evaluations total, exactly what a bigo side-pot
/// showdown does per contesting hand.
fn bigo_exactly_two_hilo(c: &mut Criterion) {
    let hole = cards("Ah 2h 3d 4c 5s");
    let board = cards("6h 7d 8c 9s Th");
    c.bench_function("bigo_exactly_two_hilo", |b| {
        b.iter(|| {
            let hi = eval::best_with_usage(
                EvalKind::High,
                HoleUsage::ExactlyTwo,
                black_box(&hole),
                black_box(&board),
            );
            let lo = eval::best_with_usage(
                EvalKind::EightOrBetterLow,
                HoleUsage::ExactlyTwo,
                black_box(&hole),
                black_box(&board),
            );
            black_box((hi, lo))
        })
    });
}

/// Badacey-shaped input: 5 cards, best 4-of-5 badugi subset search.
fn badugi_of_five(c: &mut Criterion) {
    let hand = cards("Ah 2d 3c 4s 5h");
    c.bench_function("badugi_of_five", |b| {
        b.iter(|| black_box(eval::badugi(black_box(&hand))))
    });
}

/// Eight-or-better qualifier check over a typical 7-card hand.
fn eight_or_better_7(c: &mut Criterion) {
    let hand = cards("Ah 2d 3c 4s 5h 9c Tc");
    c.bench_function("eight_or_better_7", |b| {
        b.iter(|| black_box(eval::eight_or_better(black_box(&hand))))
    });
}

criterion_group!(
    benches,
    rank_five,
    high_best_5_of_7,
    omaha_exactly_two,
    bigo_exactly_two_hilo,
    badugi_of_five,
    eight_or_better_7
);
criterion_main!(benches);

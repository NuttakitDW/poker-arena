//! Match-runner throughput: `run_match` end to end, Caller vs Caller.
//!
//! Caller is deterministic (no RNG, never bets, never folds), so all
//! variance across iterations comes from engine + runner overhead — deck
//! shuffling, event redaction/delivery, stat/behavior bookkeeping — not from
//! bot decision cost. This is the regression tripwire for match throughput:
//! a real slowdown in the runner or the engine's hot path shows up here.
//! Not a substitute for the evaluator/engine microbenchmarks in
//! `poker-core`, which isolate where time actually goes.
//!
//! Bots are rebuilt inside `iter()` (cheap relative to a match) so no state
//! leaks across iterations. `Throughput::Elements` is set to the hands
//! played per iteration so the report reads directly in hands/sec.
//!
//! ## Baseline (Apple Silicon, 2026-07)
//!
//! | benchmark | hands/iter | throughput |
//! |---|---|---|
//! | `heads_up_duplicate_100_decks` | 200 | ~PLACEHOLDER hands/s |
//! | `six_max_duplicate_20_decks` | 120 | ~PLACEHOLDER hands/s |
//!
//! Informational only — a review-time drift check, not a CI gate. Re-run
//! `cargo bench -p poker-arena` after touching `runner.rs` or the engine's
//! hot path and compare against this table.

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use poker_arena::builtin::Caller;
use poker_arena::{Bot, DealingMode, FaultPolicy, MatchConfig, run_match};
use poker_core::game::{GameSpec, Stakes};
use std::hint::black_box;

const STAKES: Stakes = Stakes::Blinds {
    small_blind: 50,
    big_blind: 100,
};

fn nl_config(decks: u64) -> MatchConfig {
    MatchConfig {
        spec: GameSpec::holdem_nl(STAKES),
        decks,
        seed: 42,
        dealing: DealingMode::Duplicate,
        starting_stack: 100 * 100,
        fault_policy: FaultPolicy::CheckFold,
        timeout: None,
    }
}

/// Heads-up, 100 decks duplicate-dealt (2 rotations/deck) = 200 hands/iter.
fn heads_up_duplicate_100_decks(c: &mut Criterion) {
    let config = nl_config(100);
    let mut group = c.benchmark_group("heads_up_duplicate_100_decks");
    group.throughput(Throughput::Elements(200));
    group.bench_function("heads_up_duplicate_100_decks", |b| {
        b.iter(|| {
            let mut bots: Vec<Box<dyn Bot>> = vec![
                Box::new(Caller::new("caller-0")),
                Box::new(Caller::new("caller-1")),
            ];
            black_box(run_match(&config, &mut bots, None, None).unwrap())
        })
    });
    group.finish();
}

/// Six-max, 20 decks duplicate-dealt (6 rotations/deck) = 120 hands/iter.
fn six_max_duplicate_20_decks(c: &mut Criterion) {
    let config = nl_config(20);
    let mut group = c.benchmark_group("six_max_duplicate_20_decks");
    group.throughput(Throughput::Elements(120));
    group.bench_function("six_max_duplicate_20_decks", |b| {
        b.iter(|| {
            let mut bots: Vec<Box<dyn Bot>> = (0..6)
                .map(|i| Box::new(Caller::new(format!("caller-{i}"))) as Box<dyn Bot>)
                .collect();
            black_box(run_match(&config, &mut bots, None, None).unwrap())
        })
    });
    group.finish();
}

criterion_group!(
    benches,
    heads_up_duplicate_100_decks,
    six_max_duplicate_20_decks
);
criterion_main!(benches);

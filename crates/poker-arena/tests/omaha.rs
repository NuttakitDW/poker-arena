//! Runner smoke tests for the Omaha family of variants: full matches driven
//! end to end through [`run_match`], checking the arena-level invariants
//! (zero-sum totals, determinism, no faults) rather than hand-by-hand rules
//! (that's `poker-core/tests/omaha.rs`'s job).

use std::time::Duration;

use poker_arena::builtin::{Caller, Folder, Random, Shover};
use poker_arena::config::{DealingMode, FaultPolicy, MatchConfig};
use poker_arena::log::JsonLog;
use poker_arena::runner::run_match;
use poker_arena::{Bot, EventSink};
use poker_core::game::{GameSpec, Stakes};

const STAKES: Stakes = Stakes {
    small_blind: 50,
    big_blind: 100,
};

fn config(spec: GameSpec, decks: u64, seed: u64, dealing: DealingMode) -> MatchConfig {
    MatchConfig {
        spec,
        decks,
        seed,
        dealing,
        starting_stack: 100 * STAKES.big_blind,
        fault_policy: FaultPolicy::CheckFold,
        timeout: Some(Duration::from_secs(1)),
    }
}

// --- 1. omaha-pl duplicate heads-up ---------------------------------------

#[test]
fn omaha_pl_duplicate_match_is_zero_sum_and_deterministic() {
    let run = || {
        let cfg = config(
            GameSpec::omaha_pl(STAKES),
            200,
            2_026,
            DealingMode::Duplicate,
        );
        let mut bots: Vec<Box<dyn Bot>> = vec![
            Box::new(Caller::new("caller")),
            Box::new(Random::new("random", 3)),
        ];
        run_match(&cfg, &mut bots, None, None).unwrap()
    };

    let a = run();
    let b = run();

    // Duplicate heads-up: every deck is replayed once per seat rotation
    // (2 rotations), so 200 decks -> 400 hands.
    assert_eq!(a.hands_played, 400);
    assert_eq!(a.decks_played, 200);
    assert_eq!(a.forfeited_by, None);

    let sum: i64 = a.outcomes.iter().map(|o| o.total_net_chips).sum();
    assert_eq!(sum, 0, "arena chips are conserved across the whole match");

    // Same config, same seed -> byte-for-byte identical outcome.
    assert_eq!(a.hands_played, b.hands_played);
    assert_eq!(a.decks_played, b.decks_played);
    assert_eq!(a.forfeited_by, b.forfeited_by);
    for (oa, ob) in a.outcomes.iter().zip(&b.outcomes) {
        assert_eq!(oa.name, ob.name);
        assert_eq!(oa.total_net_chips, ob.total_net_chips);
        assert_eq!(oa.faults, ob.faults);
        assert_eq!(oa.stats.count(), ob.stats.count());
        assert!((oa.stats.mean() - ob.stats.mean()).abs() < 1e-12);
    }
}

// --- 2. omaha8-fl multiway ------------------------------------------------

#[test]
fn omaha8_fl_multiway_zero_sum() {
    let cfg = config(GameSpec::omaha8_fl(STAKES), 150, 8_811, DealingMode::Seeded);
    let mut bots: Vec<Box<dyn Bot>> = vec![
        Box::new(Caller::new("caller")),
        Box::new(Random::new("random", 1)),
        Box::new(Shover::new("shover")),
        Box::new(Folder::new("folder")),
    ];
    let result = run_match(&cfg, &mut bots, None, None).unwrap();

    assert_eq!(result.forfeited_by, None);
    assert_eq!(result.hands_played, 150);
    assert!(
        result.outcomes.iter().all(|o| o.faults == 0),
        "no bot here ever produces an illegal action: {:?}",
        result.outcomes.iter().map(|o| o.faults).collect::<Vec<_>>()
    );

    let sum: i64 = result.outcomes.iter().map(|o| o.total_net_chips).sum();
    assert_eq!(sum, 0);
}

// --- 3. omaha8-fl produces split (hi/lo) pots -----------------------------

#[test]
fn omaha8_produces_split_pots() {
    let cfg = config(GameSpec::omaha8_fl(STAKES), 300, 4_040, DealingMode::Seeded);
    // Callers never fold, so every hand runs to showdown, maximizing the
    // chance of observing a qualifying low over enough hands.
    let mut bots: Vec<Box<dyn Bot>> = vec![
        Box::new(Caller::new("a")),
        Box::new(Caller::new("b")),
        Box::new(Caller::new("c")),
        Box::new(Caller::new("d")),
    ];

    let mut buf: Vec<u8> = Vec::new();
    {
        let mut sink = JsonLog::new(&mut buf);
        let dyn_sink: &mut dyn EventSink = &mut sink;
        run_match(&cfg, &mut bots, Some(dyn_sink), None).unwrap();
    }

    let text = String::from_utf8(buf).expect("log must be valid UTF-8");
    let split_sides: Vec<String> = text
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|v| v["ev"]["event"] == "pot-awarded")
        .filter_map(|v| v["ev"]["side"].as_str().map(str::to_string))
        .filter(|side| side == "hi" || side == "lo")
        .collect();

    assert!(
        !split_sides.is_empty(),
        "expected at least one hi/lo split PotAwarded event over 300 hands"
    );
    // Every split pot must award both a "hi" and a "lo" side.
    let hi_count = split_sides.iter().filter(|s| s.as_str() == "hi").count();
    let lo_count = split_sides.iter().filter(|s| s.as_str() == "lo").count();
    assert_eq!(hi_count, lo_count, "hi/lo sides must come in pairs");
}

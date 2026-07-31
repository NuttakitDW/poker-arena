//! Runner-level smoke tests for the stud, draw, split-pot and drawmaha
//! families: full matches driven end to end through [`run_match`] (and, for
//! one case, over the wire protocol against the real `wire-caller` binary),
//! checking arena-level invariants — completion, zero-sum totals, zero
//! faults, determinism — for every stud/draw/split/drawmaha registry id.
//! Hand-by-hand rules for these families are `poker-core/tests/stud.rs`,
//! `draw.rs`, `split.rs` and `drawmaha.rs`'s job.

use std::time::Duration;

use poker_arena::Bot;
use poker_arena::builtin::{Caller, Random, Shover};
use poker_arena::config::{DealingMode, FaultPolicy, MatchConfig};
use poker_arena::remote::WireBot;
use poker_arena::runner::run_match;
use poker_core::game::{GameSpec, Stakes};
use poker_wire::message::ArenaMsg;

const STAKES: Stakes = Stakes::Blinds {
    small_blind: 50,
    big_blind: 100,
    ante: 0,
};

/// Every stud/draw registry id: the three stud variants, the three draw variants,
/// and five-card draw.
const STUD_DRAW_IDS: [&str; 8] = [
    "stud-fl",
    "stud8-fl",
    "razz-fl",
    "27td-fl",
    "a5td-fl",
    "badugi-fl",
    "5cd-nl",
    "27sd-nl",
];

/// The four split-pot ids: three five-card triple-draw split games plus
/// five-card Omaha hi-lo.
const SPLIT_IDS: [&str; 4] = ["badacey-fl", "badeucy-fl", "archie-fl", "bigo-pl"];

/// The three drawmaha ids: five-card Omaha with a single mid-hand draw,
/// split hi (omaha) / lo (in-hand, one per evaluator) between the flop and
/// turn. Hand-by-hand rules are `poker-core/tests/drawmaha.rs`'s job.
const DRAWMAHA_IDS: [&str; 3] = ["drawmaha-fl", "drawmaha-27-fl", "drawmaha-dugi-fl"];

fn config(spec: GameSpec, decks: u64, seed: u64, dealing: DealingMode) -> MatchConfig {
    MatchConfig {
        spec,
        decks,
        seed,
        dealing,
        starting_stack: 100 * STAKES.blinds().1,
        fault_policy: FaultPolicy::CheckFold,
        timeout: Some(Duration::from_secs(1)),
    }
}

/// Three seats fits every stud/draw spec's seat range (all support at least
/// `2..=6`), and exercises Caller/Random/Shover together: between them they
/// hit every stud/draw decision family (bring-in vs. completion, discard vs. stand
/// pat, and ordinary betting).
fn three_bots(seed: u64) -> Vec<Box<dyn Bot>> {
    vec![
        Box::new(Caller::new("caller")),
        Box::new(Random::new("random", seed)),
        Box::new(Shover::new("shover")),
    ]
}

// --- 1. every stud/draw id: a run_match smoke ------------------------------------

#[test]
fn every_m3_registry_id_completes_cleanly() {
    let decks = 80;
    for &id in &STUD_DRAW_IDS {
        let spec = GameSpec::by_id(id, STAKES).unwrap_or_else(|| panic!("unknown id {id}"));
        let cfg = config(spec, decks, 1_234, DealingMode::Seeded);
        let mut bots = three_bots(5);
        let result = run_match(&cfg, &mut bots, None, None)
            .unwrap_or_else(|e| panic!("{id} failed to run: {e}"));

        assert_eq!(result.forfeited_by, None, "{id} forfeited");
        assert_eq!(result.hands_played, decks, "{id} hand count");
        assert!(
            result.outcomes.iter().all(|o| o.faults == 0),
            "{id}: unexpected faults {:?}",
            result
                .outcomes
                .iter()
                .map(|o| (o.name.as_str(), o.faults))
                .collect::<Vec<_>>()
        );
        let sum: i64 = result.outcomes.iter().map(|o| o.total_net_chips).sum();
        assert_eq!(sum, 0, "{id} not zero-sum");
    }
}

/// The same mixed lineup at an arbitrary seat count: Caller, Shover, and
/// Random fillers, which between them cover every decision family.
fn mixed_bots(n: usize, seed: u64) -> Vec<Box<dyn Bot>> {
    let mut bots: Vec<Box<dyn Bot>> = vec![
        Box::new(Caller::new("caller")),
        Box::new(Shover::new("shover")),
    ];
    for i in bots.len()..n {
        bots.push(Box::new(Random::new(format!("random{i}"), seed + i as u64)));
    }
    bots
}

fn assert_clean(id: &str, result: &poker_arena::runner::MatchResult, decks: u64) {
    assert_eq!(result.forfeited_by, None, "{id} forfeited");
    assert_eq!(result.hands_played, decks, "{id} hand count");
    assert!(
        result.outcomes.iter().all(|o| o.faults == 0),
        "{id}: unexpected faults {:?}",
        result
            .outcomes
            .iter()
            .map(|o| (o.name.as_str(), o.faults))
            .collect::<Vec<_>>()
    );
    let sum: i64 = result.outcomes.iter().map(|o| o.total_net_chips).sum();
    assert_eq!(sum, 0, "{id} not zero-sum");
}

// --- 1b. every split-pot id: a run_match smoke ----------------------------

#[test]
fn every_split_registry_id_completes_cleanly() {
    let decks = 80;
    for &id in &SPLIT_IDS {
        let spec = GameSpec::by_id(id, STAKES).unwrap_or_else(|| panic!("unknown id {id}"));
        let cfg = config(spec, decks, 1_234, DealingMode::Seeded);
        let mut bots = three_bots(5);
        let result = run_match(&cfg, &mut bots, None, None)
            .unwrap_or_else(|e| panic!("{id} failed to run: {e}"));
        assert_clean(id, &result, decks);
    }
}

/// Big O seats up to nine (5 × 9 + 5 = 50 cards); six-handed exercises the
/// pot-limit sizing and the hi-lo settlement with a full-ish table, which
/// the three-handed sweep above never reaches.
#[test]
fn bigo_pl_six_handed_completes_cleanly() {
    let decks = 60;
    let cfg = config(GameSpec::bigo_pl(STAKES), decks, 4_242, DealingMode::Seeded);
    let mut bots = mixed_bots(6, 17);
    let result = run_match(&cfg, &mut bots, None, None).expect("bigo-pl six-handed");
    assert_clean("bigo-pl", &result, decks);
    assert_eq!(result.outcomes.len(), 6);
}

// --- 1c. every drawmaha id: a run_match smoke ------------------------------

#[test]
fn every_drawmaha_registry_id_completes_cleanly() {
    let decks = 80;
    for &id in &DRAWMAHA_IDS {
        let spec = GameSpec::by_id(id, STAKES).unwrap_or_else(|| panic!("unknown id {id}"));
        let cfg = config(spec, decks, 1_234, DealingMode::Seeded);
        let mut bots = three_bots(5);
        let result = run_match(&cfg, &mut bots, None, None)
            .unwrap_or_else(|e| panic!("{id} failed to run: {e}"));
        assert_clean(id, &result, decks);
    }
}

/// Five seats fits every drawmaha spec (2..=6) and exercises the draw phase
/// (and its deck-exhaustion reshuffle path) with a fuller table than the
/// three-handed sweep above.
#[test]
fn drawmaha_fl_five_handed_completes_cleanly() {
    let decks = 60;
    let cfg = config(
        GameSpec::drawmaha_fl(STAKES),
        decks,
        9_001,
        DealingMode::Seeded,
    );
    let mut bots = mixed_bots(5, 29);
    let result = run_match(&cfg, &mut bots, None, None).expect("drawmaha-fl five-handed");
    assert_clean("drawmaha-fl", &result, decks);
    assert_eq!(result.outcomes.len(), 5);
}

// --- 2. determinism replay: one stud family, one draw family, drawmaha ----

#[test]
fn badacey_fl_replays_deterministically() {
    let spec = GameSpec::badacey_fl(STAKES);
    let run = || {
        let cfg = config(spec.clone(), 60, 777, DealingMode::Duplicate);
        let mut bots = three_bots(23);
        run_match(&cfg, &mut bots, None, None).expect("badacey-fl")
    };

    let a = run();
    let b = run();

    assert_eq!(a.hands_played, b.hands_played);
    assert_eq!(a.decks_played, b.decks_played);
    assert_eq!(a.forfeited_by, b.forfeited_by);
    for (oa, ob) in a.outcomes.iter().zip(&b.outcomes) {
        assert_eq!(oa.name, ob.name);
        assert_eq!(oa.total_net_chips, ob.total_net_chips, "{}", oa.name);
        assert_eq!(oa.faults, ob.faults, "{}", oa.name);
        assert_eq!(oa.stats.count(), ob.stats.count(), "{}", oa.name);
        assert!(
            (oa.stats.mean() - ob.stats.mean()).abs() < 1e-12,
            "{} mean drifted between replays",
            oa.name
        );
    }
}

#[test]
fn drawmaha_dugi_fl_replays_deterministically() {
    let spec = GameSpec::drawmaha_dugi_fl(STAKES);
    let run = || {
        let cfg = config(spec.clone(), 60, 555, DealingMode::Duplicate);
        let mut bots = three_bots(31);
        run_match(&cfg, &mut bots, None, None).expect("drawmaha-dugi-fl")
    };

    let a = run();
    let b = run();

    assert_eq!(a.hands_played, b.hands_played);
    assert_eq!(a.decks_played, b.decks_played);
    assert_eq!(a.forfeited_by, b.forfeited_by);
    for (oa, ob) in a.outcomes.iter().zip(&b.outcomes) {
        assert_eq!(oa.name, ob.name);
        assert_eq!(oa.total_net_chips, ob.total_net_chips, "{}", oa.name);
        assert_eq!(oa.faults, ob.faults, "{}", oa.name);
        assert_eq!(oa.stats.count(), ob.stats.count(), "{}", oa.name);
        assert!(
            (oa.stats.mean() - ob.stats.mean()).abs() < 1e-12,
            "{} mean drifted between replays",
            oa.name
        );
    }
}

#[test]
fn stud_fl_and_27td_fl_replay_deterministically() {
    for id in ["stud-fl", "27td-fl"] {
        let spec = GameSpec::by_id(id, STAKES).unwrap();
        let run = || {
            let cfg = config(spec.clone(), 60, 999, DealingMode::Duplicate);
            let mut bots = three_bots(11);
            run_match(&cfg, &mut bots, None, None).unwrap_or_else(|e| panic!("{id}: {e}"))
        };

        let a = run();
        let b = run();

        assert_eq!(a.hands_played, b.hands_played, "{id}");
        assert_eq!(a.decks_played, b.decks_played, "{id}");
        assert_eq!(a.forfeited_by, b.forfeited_by, "{id}");
        for (oa, ob) in a.outcomes.iter().zip(&b.outcomes) {
            assert_eq!(oa.name, ob.name, "{id}");
            assert_eq!(oa.total_net_chips, ob.total_net_chips, "{id}: {}", oa.name);
            assert_eq!(oa.faults, ob.faults, "{id}: {}", oa.name);
            assert_eq!(oa.stats.count(), ob.stats.count(), "{id}: {}", oa.name);
            assert!(
                (oa.stats.mean() - ob.stats.mean()).abs() < 1e-12,
                "{id}: {} mean drifted between replays",
                oa.name
            );
        }
    }
}

// --- 3. duplicate-mode observation math: stud8-fl heads-up ----------------

#[test]
fn stud8_fl_duplicate_heads_up_observation_count() {
    let decks = 40;
    let cfg = config(
        GameSpec::stud8_fl(STAKES),
        decks,
        55,
        DealingMode::Duplicate,
    );
    let mut bots: Vec<Box<dyn Bot>> = vec![
        Box::new(Caller::new("caller")),
        Box::new(Shover::new("shover")),
    ];
    let result = run_match(&cfg, &mut bots, None, None).unwrap();

    assert_eq!(result.forfeited_by, None);
    // Heads-up duplicate: every deck is replayed once per seat rotation (2
    // rotations), so `decks` decks -> `decks * 2` hands but one folded-in
    // observation per deck per bot.
    assert_eq!(result.hands_played, decks * 2);
    assert_eq!(result.decks_played, decks);
    for o in &result.outcomes {
        assert_eq!(o.stats.count(), decks, "{}", o.name);
    }
    let sum: i64 = result.outcomes.iter().map(|o| o.total_net_chips).sum();
    assert_eq!(sum, 0);
}

// --- 4. wire-level: stud-fl and 27td-fl over the real wire-caller ---------

fn hello_for(spec: &GameSpec, starting_stack: u64, timeout_ms: Option<u64>) -> ArenaMsg {
    ArenaMsg::Hello {
        proto: poker_wire::PROTO_VERSION,
        game_id: spec.id.to_string(),
        stakes: spec.stakes,
        betting: spec.betting,
        seat_count: 2,
        starting_stack,
        timeout_ms,
    }
}

/// End to end over the wire: `wire-caller` (check/call, now also handling
/// draw and bring-in decisions) against `builtin:caller`'s in-process
/// equivalent. This is the one test that exercises the wire `Act` message's
/// `upcards` field and the draw/bring-in action flow through a real
/// subprocess round trip, not just in-process.
#[test]
fn wire_caller_plays_stud_and_draw_games_over_the_wire() {
    for id in ["stud-fl", "27td-fl"] {
        let spec = GameSpec::by_id(id, STAKES).unwrap();
        let decks = 20;
        let starting_stack = 100 * STAKES.blinds().1;
        let mut cfg = config(spec.clone(), decks, 321, DealingMode::Duplicate);
        cfg.timeout = Some(Duration::from_secs(5));
        cfg.starting_stack = starting_stack;

        let mut wire = WireBot::spawn_cmd(
            env!("CARGO_BIN_EXE_wire-caller"),
            hello_for(&spec, starting_stack, Some(5_000)),
            Duration::from_secs(10),
        )
        .unwrap_or_else(|e| panic!("{id}: spawn wire-caller failed: {e}"));
        wire.set_timeout(cfg.timeout);

        let mut bots: Vec<Box<dyn Bot>> = vec![Box::new(wire), Box::new(Caller::new("caller"))];
        let result = run_match(&cfg, &mut bots, None, None)
            .unwrap_or_else(|e| panic!("{id}: match failed: {e}"));

        assert_eq!(result.forfeited_by, None, "{id} forfeited");
        assert_eq!(result.hands_played, decks * 2, "{id} hand count");
        assert!(
            result.outcomes.iter().all(|o| o.faults == 0),
            "{id}: unexpected faults {:?}",
            result
                .outcomes
                .iter()
                .map(|o| (o.name.as_str(), o.faults))
                .collect::<Vec<_>>()
        );
        let sum: i64 = result.outcomes.iter().map(|o| o.total_net_chips).sum();
        assert_eq!(sum, 0, "{id} not zero-sum");
    }
}

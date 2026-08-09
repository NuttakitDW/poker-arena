//! End-to-end: the `poker-bot` binary plays a real match of **every** game
//! in both registries over the stdio transport, and zero faults is the bar.

use std::time::Duration;

use poker_arena::builtin::Random;
use poker_arena::config::{DealingMode, FaultPolicy, MatchConfig};
use poker_arena::ofc::{OfcMatchConfig, OfcRandom, OfcWireBot, run_ofc_match};
use poker_arena::remote::WireBot;
use poker_arena::runner::run_match;
use poker_core::game::GameSpec;
use poker_wire::game::Stakes;
use poker_wire::message::ArenaMsg;
use poker_wire::ofc::message::OfcArenaMsg;

const STAKES: Stakes = Stakes::Blinds {
    small_blind: 50,
    big_blind: 100,
    ante: 0,
};
const STACK: u64 = 10_000;
const TIMEOUT: Duration = Duration::from_secs(10);

fn betting_hello(spec: &GameSpec, timeout: Duration) -> ArenaMsg {
    ArenaMsg::Hello {
        proto: poker_wire::PROTO_VERSION,
        game_id: spec.id.to_string(),
        stakes: spec.stakes,
        betting: spec.betting,
        seat_count: 2,
        starting_stack: STACK,
        timeout_ms: Some(timeout.as_millis() as u64),
    }
}

/// A short heads-up match of `game_id` against `builtin:random`; the wire
/// bot must answer every decision without a single fault.
fn betting_game_is_fault_free(game_id: &str) {
    let spec = GameSpec::by_id(game_id, STAKES).expect("registry id");
    let hello = betting_hello(&spec, TIMEOUT);
    let mut wire = WireBot::spawn_cmd(env!("CARGO_BIN_EXE_poker-bot"), hello, TIMEOUT)
        .unwrap_or_else(|e| panic!("{game_id}: spawn poker-bot: {e}"));
    wire.set_name("poker-bot");
    wire.set_timeout(Some(TIMEOUT));

    let config = MatchConfig {
        spec,
        decks: 10,
        seed: 42,
        dealing: DealingMode::Seeded,
        starting_stack: STACK,
        fault_policy: FaultPolicy::Substitute,
        timeout: Some(TIMEOUT),
    };
    let mut bots: Vec<Box<dyn poker_arena::bot::Bot>> =
        vec![Box::new(wire), Box::new(Random::new("random", 7))];
    let result = run_match(&config, &mut bots, None, None)
        .unwrap_or_else(|e| panic!("{game_id}: match failed: {e}"));

    assert_eq!(result.outcomes[0].faults, 0, "{game_id}: poker-bot faulted");
    assert!(result.hands_played > 0, "{game_id}: no hands played");
}

/// A short OFC match of `game_id` against `builtin:random`, same bar.
fn ofc_game_is_fault_free(game_id: &str) {
    let spec = *poker_core::ofc::find(game_id).expect("registry id");
    let hello = OfcArenaMsg::Hello {
        proto: poker_wire::ofc::PROTO_VERSION,
        game_id: game_id.to_string(),
        seat_count: 2,
        timeout_ms: Some(TIMEOUT.as_millis() as u64),
    };
    let mut wire = OfcWireBot::spawn_cmd(env!("CARGO_BIN_EXE_poker-bot"), hello, TIMEOUT)
        .unwrap_or_else(|e| panic!("{game_id}: spawn poker-bot: {e}"));
    wire.set_name("poker-bot");
    wire.set_timeout(Some(TIMEOUT));

    let config = OfcMatchConfig {
        spec,
        hands: 20,
        seed: 42,
        fault_policy: FaultPolicy::Substitute,
        timeout: Some(TIMEOUT),
    };
    let mut bots: Vec<Box<dyn poker_arena::ofc::OfcBot>> =
        vec![Box::new(wire), Box::new(OfcRandom::new("random", 7))];
    let result = run_ofc_match(&config, &mut bots, &mut [], None)
        .unwrap_or_else(|e| panic!("{game_id}: match failed: {e}"));

    assert_eq!(result.outcomes[0].faults, 0, "{game_id}: poker-bot faulted");
    assert_eq!(result.hands_played, 20, "{game_id}: short match");
}

#[test]
fn every_betting_game_plays_fault_free() {
    for game_id in GameSpec::known_ids() {
        betting_game_is_fault_free(game_id);
    }
}

#[test]
fn every_ofc_game_plays_fault_free() {
    for game_id in ["ofc", "ofc-pineapple", "ofc-progressive", "ofc-27"] {
        ofc_game_is_fault_free(game_id);
    }
}

#[test]
fn the_bot_beats_random_at_holdem() {
    // Not a statistical proof, just a smoke check with a healthy margin:
    // over 200 seeded hands the equity bot should be comfortably ahead of
    // uniform-random.
    let spec = GameSpec::by_id("holdem-nl", STAKES).expect("registry id");
    let hello = betting_hello(&spec, TIMEOUT);
    let mut wire =
        WireBot::spawn_cmd(env!("CARGO_BIN_EXE_poker-bot"), hello, TIMEOUT).expect("spawn");
    wire.set_name("poker-bot");
    wire.set_timeout(Some(TIMEOUT));

    let config = MatchConfig {
        spec,
        decks: 200,
        seed: 9,
        dealing: DealingMode::Seeded,
        starting_stack: STACK,
        fault_policy: FaultPolicy::Substitute,
        timeout: Some(TIMEOUT),
    };
    let mut bots: Vec<Box<dyn poker_arena::bot::Bot>> =
        vec![Box::new(wire), Box::new(Random::new("random", 3))];
    let result = run_match(&config, &mut bots, None, None).expect("match runs");

    assert_eq!(result.outcomes[0].faults, 0);
    assert!(
        result.outcomes[0].total_net_chips > 0,
        "poker-bot lost to random over 200 hands: {}",
        result.outcomes[0].total_net_chips
    );
}

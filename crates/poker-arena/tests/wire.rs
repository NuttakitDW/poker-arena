//! Integration tests for the wire-bot transport adapter.
//!
//! Two flavors: scripted TCP peers (a thread that speaks the protocol by
//! hand, so faults can be provoked exactly) and end-to-end matches against
//! the real `wire-caller` binary over both transports.

use std::io::{BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use poker_arena::bot::{ActionRequest, Bot, BotFault};
use poker_arena::builtin::Random;
use poker_arena::config::{DealingMode, FaultPolicy, MatchConfig};
use poker_arena::remote::WireBot;
use poker_arena::runner::{MatchResult, run_match};
use poker_core::card::Card;
use poker_core::game::{Action, BetBounds, Chips, GameSpec, LegalActions, Stakes};
use poker_wire::framing::{read_msg, write_msg};
use poker_wire::message::{ArenaMsg, BotMsg, GameInfo};

const STAKES: Stakes = Stakes::Blinds {
    small_blind: 50,
    big_blind: 100,
};

fn hello(timeout_ms: Option<u64>) -> ArenaMsg {
    ArenaMsg::Hello {
        proto: poker_wire::PROTO_VERSION,
        game: GameInfo {
            id: "holdem-nl".to_string(),
            display_name: "No-Limit Texas Hold'em".to_string(),
            stakes: STAKES,
        },
        seat_count: 2,
        starting_stack: 10_000,
        timeout_ms,
    }
}

/// Owned backing data for an [`ActionRequest`] (which borrows everything).
struct Scenario {
    hole: Vec<Card>,
    board: Vec<Card>,
    upcards: Vec<Vec<Card>>,
    stacks: Vec<Chips>,
    street_commits: Vec<Chips>,
    folded: Vec<bool>,
    legal: LegalActions,
}

impl Scenario {
    fn new() -> Self {
        Self {
            hole: Vec::new(),
            board: Vec::new(),
            upcards: vec![Vec::new(), Vec::new()],
            stacks: vec![10_000, 10_000],
            street_commits: vec![100, 200],
            folded: vec![false, false],
            legal: LegalActions {
                fold: true,
                check: false,
                call: Some(100),
                bet: None,
                raise: Some(BetBounds {
                    min_to: 300,
                    max_to: 10_000,
                }),
                bring_in: None,
                draw: None,
            },
        }
    }

    fn request(&self) -> ActionRequest<'_> {
        ActionRequest {
            hand_no: 1,
            seat: 0,
            button: 1,
            street: 0,
            street_label: "preflop",
            hole: &self.hole,
            board: &self.board,
            upcards: &self.upcards,
            stacks: &self.stacks,
            street_commits: &self.street_commits,
            pot_total: 300,
            folded: &self.folded,
            legal: &self.legal,
        }
    }
}

/// Bind an ephemeral port, run `script` as the connecting peer, and hand the
/// (already bound) listener to the adapter — no bind/connect race.
fn with_scripted_peer<F>(script: F, handshake: Duration) -> (WireBot, JoinHandle<()>)
where
    F: FnOnce(&mut BufReader<TcpStream>, &mut TcpStream) + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().unwrap().port();
    let peer = thread::spawn(move || {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to arena");
        stream.set_nodelay(true).unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        script(&mut reader, &mut stream);
    });
    let bot = WireBot::listen_tcp_on(listener, hello(Some(50)), handshake).expect("handshake");
    (bot, peer)
}

/// Read arena messages until the next `act` (the scripted peers ignore
/// hand-start/event/hand-end, exactly like a minimal real bot).
fn read_until_act(reader: &mut BufReader<TcpStream>) -> bool {
    loop {
        match read_msg::<_, ArenaMsg>(reader) {
            Ok(ArenaMsg::Act { .. }) => return true,
            Ok(_) => continue,
            Err(_) => return false,
        }
    }
}

// ---- scripted-peer tests ----

#[test]
fn handshake_and_single_action() {
    let (mut bot, peer) = with_scripted_peer(
        |reader, writer| {
            let _: ArenaMsg = read_msg(reader).expect("hello");
            write_msg(
                writer,
                &BotMsg::Join {
                    name: "scripted".to_string(),
                },
            )
            .unwrap();
            assert!(read_until_act(reader));
            write_msg(
                writer,
                &BotMsg::Action {
                    action: Action::Call,
                },
            )
            .unwrap();
        },
        Duration::from_secs(5),
    );

    assert_eq!(bot.name(), "scripted");
    bot.set_timeout(Some(Duration::from_secs(5)));
    let scenario = Scenario::new();
    assert_eq!(bot.act(&scenario.request()), Ok(Action::Call));
    drop(bot);
    peer.join().unwrap();
}

#[test]
fn act_timeout_yields_timeout_fault() {
    // Signals that the peer's *late* answer to request #1 has been written,
    // so the test knows the stale-drain has something to drain.
    let (notify_tx, notify_rx) = std::sync::mpsc::channel::<()>();

    let (mut bot, peer) = with_scripted_peer(
        move |reader, writer| {
            let _: ArenaMsg = read_msg(reader).expect("hello");
            write_msg(
                writer,
                &BotMsg::Join {
                    name: "slowpoke".to_string(),
                },
            )
            .unwrap();

            // Request #1: answer far too late, with a distinctive action.
            assert!(read_until_act(reader));
            thread::sleep(Duration::from_millis(150));
            write_msg(
                writer,
                &BotMsg::Action {
                    action: Action::Fold,
                },
            )
            .unwrap();
            notify_tx.send(()).unwrap();

            // Request #2: answer promptly, with a *different* action.
            assert!(read_until_act(reader));
            write_msg(
                writer,
                &BotMsg::Action {
                    action: Action::Call,
                },
            )
            .unwrap();
        },
        Duration::from_secs(5),
    );

    let scenario = Scenario::new();
    bot.set_timeout(Some(Duration::from_millis(50)));
    assert_eq!(bot.act(&scenario.request()), Err(BotFault::Timeout));

    // Wait for the stale answer to be written, then give it time to land in
    // the adapter's channel before the next `act` drains it.
    notify_rx.recv().unwrap();
    thread::sleep(Duration::from_millis(250));

    // The connection is still usable, and the stale Fold must not be
    // mistaken for the answer to this request.
    bot.set_timeout(Some(Duration::from_secs(5)));
    assert_eq!(bot.act(&scenario.request()), Ok(Action::Call));
    drop(bot);
    peer.join().unwrap();
}

#[test]
fn disconnect_yields_disconnected() {
    let (mut bot, peer) = with_scripted_peer(
        |reader, writer| {
            let _: ArenaMsg = read_msg(reader).expect("hello");
            write_msg(
                writer,
                &BotMsg::Join {
                    name: "quitter".to_string(),
                },
            )
            .unwrap();
            // Return: the stream drops and the connection closes.
        },
        Duration::from_secs(5),
    );
    peer.join().unwrap();

    bot.set_timeout(Some(Duration::from_secs(5)));
    let scenario = Scenario::new();
    let started = Instant::now();
    assert_eq!(bot.act(&scenario.request()), Err(BotFault::Disconnected));
    assert_eq!(bot.act(&scenario.request()), Err(BotFault::Disconnected));
    // Neither call may sit out the deadline; a dead transport is known dead.
    assert!(started.elapsed() < Duration::from_secs(4));
}

#[test]
fn garbage_yields_protocol_fault() {
    let (mut bot, peer) = with_scripted_peer(
        |reader, writer| {
            let _: ArenaMsg = read_msg(reader).expect("hello");
            write_msg(
                writer,
                &BotMsg::Join {
                    name: "garbler".to_string(),
                },
            )
            .unwrap();
            assert!(read_until_act(reader));
            writer.write_all(b"this is not json\n").unwrap();
            writer.flush().unwrap();
        },
        Duration::from_secs(5),
    );

    bot.set_timeout(Some(Duration::from_secs(5)));
    let scenario = Scenario::new();
    match bot.act(&scenario.request()) {
        Err(BotFault::Protocol(msg)) => assert!(msg.contains("not json"), "unexpected: {msg}"),
        other => panic!("expected a protocol fault, got {other:?}"),
    }
    drop(bot);
    peer.join().unwrap();
}

// ---- end-to-end matches against the real `wire-caller` binary ----

fn nl_config(decks: u64, timeout: Option<Duration>, fault_policy: FaultPolicy) -> MatchConfig {
    MatchConfig {
        spec: GameSpec::holdem_nl(STAKES),
        decks,
        seed: 7,
        dealing: DealingMode::Duplicate,
        starting_stack: 10_000,
        fault_policy,
        timeout,
    }
}

/// The wire bot is bot index 0 in every end-to-end match below.
const WIRE: usize = 0;

fn assert_clean_heads_up(result: &MatchResult, decks: u64) {
    assert_eq!(result.forfeited_by, None);
    assert_eq!(result.hands_played, decks * 2);
    assert_eq!(result.outcomes[WIRE].faults, 0);
    assert_eq!(
        result.outcomes[0].total_net_chips + result.outcomes[1].total_net_chips,
        0
    );
}

#[test]
fn subprocess_end_to_end_match() {
    let decks = 30;
    let run = || {
        let config = nl_config(decks, Some(Duration::from_secs(5)), FaultPolicy::CheckFold);
        let mut wire = WireBot::spawn_cmd(
            env!("CARGO_BIN_EXE_wire-caller"),
            hello(Some(5_000)),
            Duration::from_secs(10),
        )
        .expect("spawn wire-caller");
        assert_eq!(wire.name(), "wire-caller");
        wire.set_timeout(config.timeout);

        let mut bots: Vec<Box<dyn Bot>> = vec![Box::new(wire), Box::new(Random::new("random", 5))];
        run_match(&config, &mut bots, None, None).expect("match runs")
    };

    let a = run();
    assert_clean_heads_up(&a, decks);

    // Determinism survives a real subprocess round trip.
    let b = run();
    assert_clean_heads_up(&b, decks);
    for (oa, ob) in a.outcomes.iter().zip(&b.outcomes) {
        assert_eq!(oa.name, ob.name);
        assert_eq!(oa.total_net_chips, ob.total_net_chips);
    }
}

#[test]
fn tcp_end_to_end_match() {
    let decks = 30;
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().unwrap().port();

    let mut child = Command::new(env!("CARGO_BIN_EXE_wire-caller"))
        .arg("--tcp")
        .arg(format!("127.0.0.1:{port}"))
        .arg("--name")
        .arg("tcp-caller")
        .spawn()
        .expect("spawn wire-caller");

    let config = nl_config(decks, Some(Duration::from_secs(5)), FaultPolicy::CheckFold);
    let mut wire = WireBot::listen_tcp_on(listener, hello(Some(5_000)), Duration::from_secs(10))
        .expect("handshake over tcp");
    assert_eq!(wire.name(), "tcp-caller");
    wire.set_timeout(config.timeout);

    let mut bots: Vec<Box<dyn Bot>> = vec![Box::new(wire), Box::new(Random::new("random", 5))];
    let result = run_match(&config, &mut bots, None, None).expect("match runs");
    assert_clean_heads_up(&result, decks);

    // Nothing owns this child, so the test reaps it (the `match-end` sent
    // when the bot drops normally makes it exit on its own).
    drop(bots);
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn slow_bot_forfeits_under_forfeit_policy() {
    let config = nl_config(30, Some(Duration::from_millis(50)), FaultPolicy::Forfeit);
    let command = format!("{} --sleep-ms 300", env!("CARGO_BIN_EXE_wire-caller"));
    let mut wire = WireBot::spawn_cmd(&command, hello(Some(50)), Duration::from_secs(10))
        .expect("spawn wire-caller");
    wire.set_timeout(config.timeout);

    let mut bots: Vec<Box<dyn Bot>> = vec![Box::new(wire), Box::new(Random::new("random", 5))];
    let started = Instant::now();
    let result = run_match(&config, &mut bots, None, None).expect("match runs");

    assert_eq!(result.forfeited_by, Some(WIRE));
    assert!(result.hands_played < 30);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "forfeit took {:?}",
        started.elapsed()
    );
}

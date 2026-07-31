//! Integration tests for the OFC arena: the runner over every variant, the
//! determinism promise, fantasyland carrying with the bot rather than the
//! seat, the greedy bot's foul-avoidance bar, fault handling, and the wire
//! adapter.

use std::collections::BTreeMap;
use std::io::{BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use poker_arena::bot::BotFault;
use poker_arena::ofc::bot::{OfcActionRequest, OfcBot};
use poker_arena::ofc::builtin::{OfcFiller, OfcGreedy, OfcRandom};
use poker_arena::ofc::log::{OfcEventSink, OfcHandMeta, OfcJsonLog};
use poker_arena::ofc::remote::OfcWireBot;
use poker_arena::ofc::runner::{OfcFaultPolicy, OfcMatchConfig, OfcMatchResult, run_ofc_match};
use poker_core::card::Card;
use poker_core::ofc::{
    Board, OfcAction, OfcArenaMsg, OfcBotMsg, OfcEvent, OfcSpec, Placement, Row, registry,
};
use poker_wire::framing::{read_msg, write_msg};

fn config(spec: OfcSpec, hands: u64, seed: u64, policy: OfcFaultPolicy) -> OfcMatchConfig {
    OfcMatchConfig {
        spec,
        hands,
        seed,
        fault_policy: policy,
        timeout: None,
    }
}

/// A three-bot field for the pineapple family, extended to four for classic
/// OFC (whose seat cap is four).
fn field(spec: &OfcSpec, seats: usize) -> Vec<Box<dyn OfcBot>> {
    let mut bots: Vec<Box<dyn OfcBot>> = vec![
        Box::new(OfcGreedy::new("greedy", spec.middle)),
        Box::new(OfcRandom::new("random", 5)),
        Box::new(OfcFiller::new("filler")),
        Box::new(OfcRandom::new("random-2", 9)),
    ];
    bots.truncate(seats);
    bots
}

// ---- runner smoke ----

#[test]
fn every_variant_plays_a_clean_match() {
    for spec in registry() {
        let seats = spec.max_seats;
        let hands = 60;
        let mut bots = field(spec, seats);
        let result = run_ofc_match(
            &config(*spec, hands, 17, OfcFaultPolicy::Substitute),
            &mut bots,
            &mut [],
            None,
        )
        .unwrap_or_else(|err| panic!("{} failed: {err}", spec.id));

        assert_eq!(result.hands_played, hands, "{}", spec.id);
        assert_eq!(result.forfeited_by, None, "{}", spec.id);
        assert_eq!(result.outcomes.len(), seats, "{}", spec.id);

        let total: i64 = result.outcomes.iter().map(|o| o.total_points).sum();
        assert_eq!(total, 0, "{}: points only move between bots", spec.id);

        for outcome in &result.outcomes {
            assert_eq!(outcome.stats.count(), hands, "{}", spec.id);
            assert_eq!(outcome.faults, 0, "{} builtins never fault", spec.id);
            assert!(outcome.fouls <= hands, "{}", spec.id);
            assert!(outcome.fantasylands <= hands, "{}", spec.id);
            assert!(outcome.stats.ci95_half_width().is_some(), "{}", spec.id);
        }
        // Over sixty hands with a greedy bot in the field, somebody earns
        // royalties — the statistic must actually be wired up.
        assert!(
            result.outcomes.iter().any(|o| o.royalties > 0),
            "{}: no royalties recorded",
            spec.id
        );
    }
}

// ---- determinism ----

#[test]
fn the_same_seed_reproduces_the_match_and_its_log_bytes() {
    let spec = poker_core::ofc::OFC_PINEAPPLE;
    let run = || {
        let mut buf: Vec<u8> = Vec::new();
        let result = {
            let mut log = OfcJsonLog::new(&mut buf);
            let mut sinks: Vec<&mut dyn OfcEventSink> = vec![&mut log];
            let mut bots = field(&spec, 3);
            run_ofc_match(
                &config(spec, 40, 2024, OfcFaultPolicy::Substitute),
                &mut bots,
                &mut sinks,
                None,
            )
            .unwrap()
        };
        (summarize(&result), buf)
    };

    let (first, first_log) = run();
    let (second, second_log) = run();
    assert_eq!(first, second, "same seed, same match");
    assert_eq!(first_log, second_log, "same seed, byte-identical log");
    assert!(!first_log.is_empty());
}

/// Every observable field of a match result, in a comparable form
/// (`RateStats` is an accumulator, not a value type, so its mean and count
/// stand in for it).
type Summary = Vec<(String, i64, u64, f64, u64, u64, u64, u64, u64)>;

fn summarize(result: &OfcMatchResult) -> (u64, Option<usize>, Summary) {
    (
        result.hands_played,
        result.forfeited_by,
        result
            .outcomes
            .iter()
            .map(|o| {
                (
                    o.name.clone(),
                    o.total_points,
                    o.stats.count(),
                    o.stats.mean(),
                    o.fouls,
                    o.fantasylands,
                    o.scoops,
                    o.royalties,
                    o.faults,
                )
            })
            .collect(),
    )
}

// ---- fantasyland carries with the bot ----

/// What one hand said about fantasyland, keyed by bot name so seat rotation
/// cannot smear the answer: who *played* the hand in fantasyland, and who
/// was granted one for the next hand.
#[derive(Default)]
struct HandRecord {
    played_in: BTreeMap<String, u8>,
    granted: BTreeMap<String, u8>,
}

#[derive(Default)]
struct FantasylandProbe {
    seats: Vec<String>,
    hands: Vec<HandRecord>,
}

impl OfcEventSink for FantasylandProbe {
    fn hand_start(&mut self, _hand_no: u64, seats: &[String]) {
        self.seats = seats.to_vec();
        self.hands.push(HandRecord::default());
    }

    fn event(&mut self, ev: &OfcEvent) {
        let record = self.hands.last_mut().expect("a hand is open");
        match ev {
            OfcEvent::Fantasyland { seat, cards } => {
                record.played_in.insert(self.seats[*seat].clone(), *cards);
            }
            OfcEvent::Showdown {
                seat,
                next_fantasyland: Some(cards),
                ..
            } => {
                record.granted.insert(self.seats[*seat].clone(), *cards);
            }
            _ => {}
        }
    }

    fn hand_end(&mut self, _meta: &OfcHandMeta) {}
}

#[test]
fn a_bot_granted_fantasyland_plays_the_next_hand_in_it() {
    let spec = poker_core::ofc::OFC_PROGRESSIVE;
    let mut probe = FantasylandProbe::default();
    {
        let mut sinks: Vec<&mut dyn OfcEventSink> = vec![&mut probe];
        let mut bots = field(&spec, 3);
        run_ofc_match(
            &config(spec, 150, 4242, OfcFaultPolicy::Substitute),
            &mut bots,
            &mut sinks,
            None,
        )
        .unwrap();
    }

    let grants: usize = probe.hands.iter().map(|h| h.granted.len()).sum();
    assert!(grants > 0, "no fantasyland was earned in 150 hands");

    for (index, pair) in probe.hands.windows(2).enumerate() {
        assert_eq!(
            pair[0].granted,
            pair[1].played_in,
            "hand {index}'s grants must be exactly hand {}'s fantasyland seats",
            index + 1
        );
    }
    // The last hand's grants are simply never played (the match ends), which
    // is the fixed-hand-count rule: fantasyland never adds a hand.
}

// ---- the greedy bot's foul-avoidance bar ----

/// Counts fouls per *bot name*, reading the showdown events and the seat
/// names each hand opens with — so seat rotation never misattributes one.
#[derive(Default)]
struct FoulProbe {
    seats: Vec<String>,
    fouls: BTreeMap<String, u64>,
    hands: u64,
}

impl OfcEventSink for FoulProbe {
    fn hand_start(&mut self, _hand_no: u64, seats: &[String]) {
        self.seats = seats.to_vec();
    }

    fn event(&mut self, ev: &OfcEvent) {
        if let OfcEvent::Showdown {
            seat, fouled: true, ..
        } = ev
        {
            *self.fouls.entry(self.seats[*seat].clone()).or_default() += 1;
        }
    }

    fn hand_end(&mut self, _meta: &OfcHandMeta) {
        self.hands += 1;
    }
}

#[test]
fn greedy_fouls_rarely_and_beats_random_over_three_hundred_hands() {
    let spec = poker_core::ofc::OFC;
    let hands = 300;
    let mut probe = FoulProbe::default();
    let result = {
        let mut sinks: Vec<&mut dyn OfcEventSink> = vec![&mut probe];
        let mut bots: Vec<Box<dyn OfcBot>> = vec![
            Box::new(OfcGreedy::new("greedy", spec.middle)),
            Box::new(OfcRandom::new("random", 1)),
        ];
        run_ofc_match(
            &config(spec, hands, 31337, OfcFaultPolicy::Substitute),
            &mut bots,
            &mut sinks,
            None,
        )
        .unwrap()
    };

    let greedy_fouls = *probe.fouls.get("greedy").unwrap_or(&0);
    let random_fouls = *probe.fouls.get("random").unwrap_or(&0);
    let greedy_points = result.outcomes[0].total_points;
    println!(
        "greedy: {greedy_fouls} fouls / {hands} hands ({:.1}%), {greedy_points} points; \
         random: {random_fouls} fouls ({:.1}%)",
        100.0 * greedy_fouls as f64 / hands as f64,
        100.0 * random_fouls as f64 / hands as f64,
    );

    // The runner's own foul statistic and the event stream must agree.
    assert_eq!(result.outcomes[0].fouls, greedy_fouls);
    assert_eq!(result.outcomes[1].fouls, random_fouls);

    assert!(
        greedy_fouls * 5 < hands,
        "greedy fouled {greedy_fouls} of {hands} hands (bar: under 20%)"
    );
    assert!(
        greedy_fouls * 2 < random_fouls,
        "greedy fouled {greedy_fouls} times, random {random_fouls} (bar: under half)"
    );
    assert!(
        greedy_points > 0,
        "greedy finished {greedy_points} points against random"
    );
}

// ---- fault handling ----

/// Answers every request with a placement that can never be legal: no cards
/// at all, whatever was asked for.
struct Garbage {
    name: String,
}

impl OfcBot for Garbage {
    fn name(&self) -> &str {
        &self.name
    }

    fn place(&mut self, _req: &OfcActionRequest<'_>) -> Result<OfcAction, BotFault> {
        Ok(OfcAction {
            placements: Vec::new(),
            discards: Vec::new(),
        })
    }
}

#[test]
fn substitution_keeps_a_faulting_match_running_and_zero_sum() {
    let spec = poker_core::ofc::OFC_27;
    let hands = 20;
    let mut probe = FoulProbe::default();
    let result = {
        let mut sinks: Vec<&mut dyn OfcEventSink> = vec![&mut probe];
        let mut bots: Vec<Box<dyn OfcBot>> = vec![
            Box::new(Garbage {
                name: "garbage".into(),
            }),
            Box::new(OfcGreedy::new("greedy", spec.middle)),
        ];
        run_ofc_match(
            &config(spec, hands, 8, OfcFaultPolicy::Substitute),
            &mut bots,
            &mut sinks,
            None,
        )
        .unwrap()
    };

    assert_eq!(result.forfeited_by, None);
    assert_eq!(result.hands_played, hands);
    assert_eq!(probe.hands, hands, "every hand still settled");
    // Five turns a hand, every one of them faulted.
    assert_eq!(result.outcomes[0].faults, hands * 5);
    assert_eq!(result.outcomes[1].faults, 0);
    let total: i64 = result.outcomes.iter().map(|o| o.total_points).sum();
    assert_eq!(total, 0);
}

#[test]
fn forfeit_policy_ends_the_match_at_the_first_fault() {
    let spec = poker_core::ofc::OFC;
    let mut bots: Vec<Box<dyn OfcBot>> = vec![
        Box::new(OfcGreedy::new("greedy", spec.middle)),
        Box::new(Garbage {
            name: "garbage".into(),
        }),
    ];
    let result = run_ofc_match(
        &config(spec, 20, 8, OfcFaultPolicy::Forfeit),
        &mut bots,
        &mut [],
        None,
    )
    .unwrap();

    assert_eq!(result.forfeited_by, Some(1));
    assert_eq!(result.outcomes[1].name, "garbage");
    assert_eq!(result.outcomes[1].faults, 1);
    assert_eq!(result.hands_played, 0, "the first hand never settled");
}

// ---- the wire adapter ----

fn hello(timeout_ms: Option<u64>) -> OfcArenaMsg {
    OfcArenaMsg::Hello {
        proto: poker_wire::ofc::PROTO_VERSION,
        game_id: "ofc".to_string(),
        seat_count: 2,
        timeout_ms,
    }
}

/// Owned backing data for an [`OfcActionRequest`] (which borrows everything).
struct Scenario {
    dealt: Vec<Card>,
    boards: Vec<Board>,
    fantasyland: Vec<Option<u8>>,
}

impl Scenario {
    fn new() -> Self {
        Self {
            dealt: poker_core::card::parse_cards("As").unwrap(),
            boards: vec![Board::new(), Board::new()],
            fantasyland: vec![None, None],
        }
    }

    fn request(&self) -> OfcActionRequest<'_> {
        OfcActionRequest {
            hand_no: 1,
            seat: 0,
            dealt: &self.dealt,
            place: 1,
            discard: 0,
            boards: &self.boards,
            fantasyland: &self.fantasyland,
        }
    }
}

fn ace_on_the_bottom() -> OfcAction {
    OfcAction {
        placements: vec![Placement {
            card: poker_core::card::parse_cards("As").unwrap()[0],
            row: Row::Bottom,
        }],
        discards: Vec::new(),
    }
}

/// Bind an ephemeral port, run `script` as the connecting peer, and hand the
/// (already bound) listener to the adapter — no bind/connect race.
fn with_scripted_peer<F>(script: F, handshake: Duration) -> (OfcWireBot, JoinHandle<()>)
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
    let bot = OfcWireBot::listen_tcp_on(listener, hello(Some(50)), handshake).expect("handshake");
    (bot, peer)
}

/// Read arena messages until the next `act` (the scripted peers ignore
/// hand-start/event/hand-end, exactly like a minimal real bot).
fn read_until_act(reader: &mut BufReader<TcpStream>) -> bool {
    loop {
        match read_msg::<_, OfcArenaMsg>(reader) {
            Ok(OfcArenaMsg::Act { .. }) => return true,
            Ok(_) => continue,
            Err(_) => return false,
        }
    }
}

#[test]
fn wire_handshake_and_single_placement() {
    let (mut bot, peer) = with_scripted_peer(
        |reader, writer| {
            let _: OfcArenaMsg = read_msg(reader).expect("hello");
            write_msg(writer, &OfcBotMsg::Join {}).unwrap();
            assert!(read_until_act(reader));
            write_msg(
                writer,
                &OfcBotMsg::Action {
                    action: ace_on_the_bottom(),
                },
            )
            .unwrap();
        },
        Duration::from_secs(5),
    );

    // Identity is operator-assigned; until then the placeholder holds.
    assert_eq!(bot.name(), "ofc-wire-bot");
    bot.set_timeout(Some(Duration::from_secs(5)));
    let scenario = Scenario::new();
    assert_eq!(bot.place(&scenario.request()), Ok(ace_on_the_bottom()));
    drop(bot);
    peer.join().unwrap();
}

#[test]
fn wire_act_carries_the_place_and_discard_counts() {
    let (mut bot, peer) = with_scripted_peer(
        |reader, writer| {
            let _: OfcArenaMsg = read_msg(reader).expect("hello");
            write_msg(writer, &OfcBotMsg::Join {}).unwrap();
            loop {
                match read_msg::<_, OfcArenaMsg>(reader) {
                    Ok(OfcArenaMsg::Act { decision, .. }) => {
                        assert_eq!(
                            decision,
                            poker_core::ofc::OfcDecision::Place {
                                place: 1,
                                discard: 0
                            }
                        );
                        break;
                    }
                    Ok(_) => continue,
                    Err(err) => panic!("peer read failed: {err}"),
                }
            }
            write_msg(
                writer,
                &OfcBotMsg::Action {
                    action: ace_on_the_bottom(),
                },
            )
            .unwrap();
        },
        Duration::from_secs(5),
    );

    bot.set_timeout(Some(Duration::from_secs(5)));
    let scenario = Scenario::new();
    assert!(bot.place(&scenario.request()).is_ok());
    drop(bot);
    peer.join().unwrap();
}

#[test]
fn wire_timeout_yields_a_timeout_fault_without_desyncing() {
    // Signals that the peer's *late* answer to request #1 has been written,
    // so the test knows the stale-drain has something to drain.
    let (notify_tx, notify_rx) = std::sync::mpsc::channel::<()>();

    let (mut bot, peer) = with_scripted_peer(
        move |reader, writer| {
            let _: OfcArenaMsg = read_msg(reader).expect("hello");
            write_msg(writer, &OfcBotMsg::Join {}).unwrap();

            // Request #1: answer far too late.
            assert!(read_until_act(reader));
            thread::sleep(Duration::from_millis(150));
            write_msg(
                writer,
                &OfcBotMsg::Action {
                    action: OfcAction {
                        placements: Vec::new(),
                        discards: Vec::new(),
                    },
                },
            )
            .unwrap();
            notify_tx.send(()).unwrap();

            // Request #2: answer promptly, with a *different* action.
            assert!(read_until_act(reader));
            write_msg(
                writer,
                &OfcBotMsg::Action {
                    action: ace_on_the_bottom(),
                },
            )
            .unwrap();
        },
        Duration::from_secs(5),
    );

    let scenario = Scenario::new();
    bot.set_timeout(Some(Duration::from_millis(50)));
    assert_eq!(bot.place(&scenario.request()), Err(BotFault::Timeout));

    notify_rx.recv().unwrap();
    thread::sleep(Duration::from_millis(250));

    // The connection is still usable, and the stale empty action must not be
    // mistaken for the answer to this request.
    bot.set_timeout(Some(Duration::from_secs(5)));
    assert_eq!(bot.place(&scenario.request()), Ok(ace_on_the_bottom()));
    drop(bot);
    peer.join().unwrap();
}

#[test]
fn wire_disconnect_yields_disconnected() {
    let (mut bot, peer) = with_scripted_peer(
        |reader, writer| {
            let _: OfcArenaMsg = read_msg(reader).expect("hello");
            write_msg(writer, &OfcBotMsg::Join {}).unwrap();
            // Return: the stream drops and the connection closes.
        },
        Duration::from_secs(5),
    );
    peer.join().unwrap();

    bot.set_timeout(Some(Duration::from_secs(5)));
    let scenario = Scenario::new();
    let started = Instant::now();
    assert_eq!(bot.place(&scenario.request()), Err(BotFault::Disconnected));
    assert_eq!(bot.place(&scenario.request()), Err(BotFault::Disconnected));
    // Neither call may sit out the deadline; a dead transport is known dead.
    assert!(started.elapsed() < Duration::from_secs(4));
}

#[test]
fn wire_garbage_yields_a_protocol_fault() {
    let (mut bot, peer) = with_scripted_peer(
        |reader, writer| {
            let _: OfcArenaMsg = read_msg(reader).expect("hello");
            write_msg(writer, &OfcBotMsg::Join {}).unwrap();
            assert!(read_until_act(reader));
            writer.write_all(b"this is not json\n").unwrap();
            writer.flush().unwrap();
        },
        Duration::from_secs(5),
    );

    bot.set_timeout(Some(Duration::from_secs(5)));
    let scenario = Scenario::new();
    match bot.place(&scenario.request()) {
        Err(BotFault::Protocol(msg)) => assert!(msg.contains("not json"), "unexpected: {msg}"),
        other => panic!("expected a protocol fault, got {other:?}"),
    }
    drop(bot);
    peer.join().unwrap();
}

/// After the field-wide name assignment, the bot receives a `joined` ack
/// with its final (possibly disambiguated) name.
#[test]
fn wire_joined_ack_carries_the_assigned_name() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    let peer = thread::spawn(move || {
        let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut writer = stream;
        let _hello: OfcArenaMsg = read_msg(&mut reader).unwrap();
        write_msg(&mut writer, &OfcBotMsg::Join {}).unwrap();
        // The ack arrives after the arena finishes seating everyone.
        match read_msg::<_, OfcArenaMsg>(&mut reader).unwrap() {
            OfcArenaMsg::Joined { name } => name,
            other => panic!("expected joined ack, got {other:?}"),
        }
    });

    let mut bot =
        OfcWireBot::listen_tcp_on(listener, hello(Some(5_000)), Duration::from_secs(5)).unwrap();
    assert_eq!(bot.name(), "ofc-wire-bot");
    bot.set_name("greedy-2");
    assert_eq!(bot.name(), "greedy-2");

    assert_eq!(peer.join().unwrap(), "greedy-2");
}

/// A wire bot playing a whole match against an in-process opponent: the
/// runner cannot tell the difference, and the transport survives every hand.
#[test]
fn wire_bot_plays_a_full_match_over_a_socket() {
    let spec = poker_core::ofc::OFC;
    let hands = 10;
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().unwrap().port();

    // A filler bot on the far end of a socket, driven purely off the wire:
    // it tracks its own deals and board from the event stream.
    let peer = thread::spawn(move || {
        let stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to arena");
        stream.set_nodelay(true).unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut writer = stream;
        write_msg(&mut writer, &OfcBotMsg::Join {}).unwrap();

        let mut seat = 0usize;
        let mut board = Board::new();
        let mut dealt: Vec<Card> = Vec::new();
        while let Ok(msg) = read_msg::<_, OfcArenaMsg>(&mut reader) {
            match msg {
                OfcArenaMsg::HandStart { seat: mine, .. } => {
                    seat = mine;
                    board = Board::new();
                    dealt.clear();
                }
                OfcArenaMsg::Event { ev, .. } => match ev {
                    OfcEvent::Deal { seat: s, cards, .. } if s == seat => dealt.extend(cards),
                    OfcEvent::Place {
                        seat: s,
                        placements,
                        ..
                    } if s == seat => {
                        for placement in placements {
                            board.push(placement);
                        }
                        dealt.clear();
                    }
                    _ => {}
                },
                OfcArenaMsg::Act { decision, .. } => {
                    let poker_core::ofc::OfcDecision::Place { place, .. } = decision;
                    // The filler rule, computed on the bot's side.
                    let mut sorted = dealt.clone();
                    sorted.sort_unstable_by_key(|card| card.index());
                    let discards = sorted.split_off(place as usize);
                    let mut free = [
                        board.free(Row::Top),
                        board.free(Row::Middle),
                        board.free(Row::Bottom),
                    ];
                    let rows = [Row::Top, Row::Middle, Row::Bottom];
                    let placements = sorted
                        .into_iter()
                        .map(|card| {
                            let index = if free[2] > 0 {
                                2
                            } else if free[1] > 0 {
                                1
                            } else {
                                0
                            };
                            free[index] -= 1;
                            Placement {
                                card,
                                row: rows[index],
                            }
                        })
                        .collect();
                    write_msg(
                        &mut writer,
                        &OfcBotMsg::Action {
                            action: OfcAction {
                                placements,
                                discards,
                            },
                        },
                    )
                    .unwrap();
                }
                OfcArenaMsg::MatchEnd {} => break,
                _ => {}
            }
        }
    });

    let mut wire = OfcWireBot::listen_tcp_on(listener, hello(Some(5_000)), Duration::from_secs(10))
        .expect("handshake over tcp");
    wire.set_name("wire-filler");
    wire.set_timeout(Some(Duration::from_secs(5)));

    let mut bots: Vec<Box<dyn OfcBot>> = vec![
        Box::new(wire),
        Box::new(OfcGreedy::new("greedy", spec.middle)),
    ];
    let result = run_ofc_match(
        &config(spec, hands, 77, OfcFaultPolicy::Substitute),
        &mut bots,
        &mut [],
        None,
    )
    .expect("match runs");

    assert_eq!(result.hands_played, hands);
    assert_eq!(result.forfeited_by, None);
    assert_eq!(
        result.outcomes[0].faults, 0,
        "a correct wire bot never faults"
    );
    assert_eq!(
        result.outcomes[0].total_points + result.outcomes[1].total_points,
        0
    );

    drop(bots);
    peer.join().unwrap();
}

// ---- reference wire bots, end-to-end over a real subprocess ----

fn ofc_pineapple_hello(timeout_ms: Option<u64>) -> OfcArenaMsg {
    OfcArenaMsg::Hello {
        proto: poker_wire::ofc::PROTO_VERSION,
        game_id: "ofc-pineapple".to_string(),
        seat_count: 2,
        timeout_ms,
    }
}

/// `wire-placer` (the Rust reference bot, `crates/poker-arena/src/bin/`)
/// against `greedy` over a spawned-subprocess stdio transport: the bar is
/// zero faults over a full match, with points still zero-sum.
#[test]
fn wire_placer_reference_bot_plays_a_clean_match_via_subprocess() {
    let spec = poker_core::ofc::OFC_PINEAPPLE;
    let hands = 20;
    let mut wire = OfcWireBot::spawn_cmd(
        env!("CARGO_BIN_EXE_wire-placer"),
        ofc_pineapple_hello(Some(5_000)),
        Duration::from_secs(10),
    )
    .expect("spawn wire-placer");
    wire.set_name("wire-placer");
    wire.set_timeout(Some(Duration::from_secs(5)));

    let mut bots: Vec<Box<dyn OfcBot>> = vec![
        Box::new(wire),
        Box::new(OfcGreedy::new("greedy", spec.middle)),
    ];
    let result = run_ofc_match(
        &config(spec, hands, 99, OfcFaultPolicy::Substitute),
        &mut bots,
        &mut [],
        None,
    )
    .expect("match runs");

    assert_eq!(result.hands_played, hands);
    assert_eq!(result.forfeited_by, None);
    assert_eq!(
        result.outcomes[0].faults, 0,
        "the reference bot never faults"
    );
    assert_eq!(
        result.outcomes[0].total_points + result.outcomes[1].total_points,
        0
    );
}

/// `examples/ofc_bot.py` (the dependency-free Python reference client)
/// against `greedy`, shelled out unconditionally exactly like the wire-bot
/// tests above shell out to a compiled binary: the repo's bar is zero
/// faults with python3 present, not a skip when it's missing.
#[test]
fn python_reference_bot_plays_a_clean_match_via_subprocess() {
    let spec = poker_core::ofc::OFC_PINEAPPLE;
    let hands = 20;
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/ofc_bot.py");
    let command = format!("python3 {script}");
    let mut wire = OfcWireBot::spawn_cmd(
        &command,
        ofc_pineapple_hello(Some(5_000)),
        Duration::from_secs(10),
    )
    .expect("spawn python reference bot");
    wire.set_name("python-bot");
    wire.set_timeout(Some(Duration::from_secs(5)));

    let mut bots: Vec<Box<dyn OfcBot>> = vec![
        Box::new(wire),
        Box::new(OfcGreedy::new("greedy", spec.middle)),
    ];
    let result = run_ofc_match(
        &config(spec, hands, 100, OfcFaultPolicy::Substitute),
        &mut bots,
        &mut [],
        None,
    )
    .expect("match runs");

    assert_eq!(result.hands_played, hands);
    assert_eq!(result.forfeited_by, None);
    assert_eq!(
        result.outcomes[0].faults, 0,
        "the reference bot never faults"
    );
    assert_eq!(
        result.outcomes[0].total_points + result.outcomes[1].total_points,
        0
    );
}

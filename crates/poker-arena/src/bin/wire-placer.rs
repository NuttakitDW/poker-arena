//! Reference OFC wire bot: the filler strategy, spoken over the wire
//! protocol. Doubles as the fixture the arena's OFC wire tests play against.
//!
//! Usage: `wire-placer [--tcp HOST:PORT] [--sleep-ms N]`. Identity is
//! operator-assigned (`--bot name@spec` on the arena side); this bot
//! carries none of its own.
//! Default transport is stdio (arena → stdin, bot → stdout); `--sleep-ms`
//! stalls before every placement, which is how tests provoke timeouts.
//!
//! The strategy is the contract's filler rule, computed client-side from the
//! event stream alone (the wire protocol never sends board state directly,
//! only placements): sort this turn's dealt cards ascending by
//! [`Card::index`], then drop each into bottom if it has room, else middle,
//! else top, tracking its own board across turns so it always knows which
//! rows still have space.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::process::ExitCode;
use std::time::Duration;

use poker_core::card::Card;
use poker_core::ofc::{
    Board, OfcAction, OfcArenaMsg, OfcBotMsg, OfcDecision, OfcEvent, Placement, Row,
};
use poker_wire::framing::{WireError, read_msg, write_msg};

/// The row order the filler rule fills in: bottom first, then middle, then
/// top.
const ROWS: [Row; 3] = [Row::Bottom, Row::Middle, Row::Top];

fn main() -> ExitCode {
    let mut sleep_ms = 0u64;
    let mut tcp: Option<String> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--tcp" => tcp = Some(next_value(&mut args, "--tcp")),
            "--sleep-ms" => {
                sleep_ms = next_value(&mut args, "--sleep-ms")
                    .parse()
                    .unwrap_or_else(|_| fail("--sleep-ms expects a number"));
            }
            other => fail(&format!("unknown argument {other:?}")),
        }
    }

    let outcome = match &tcp {
        Some(addr) => match TcpStream::connect(addr) {
            Ok(stream) => {
                let _ = stream.set_nodelay(true);
                match stream.try_clone() {
                    Ok(reader) => play(BufReader::new(reader), stream, sleep_ms),
                    Err(e) => Err(e.to_string()),
                }
            }
            Err(e) => Err(format!("could not connect to {addr}: {e}")),
        },
        None => play(
            BufReader::new(std::io::stdin()),
            std::io::stdout(),
            sleep_ms,
        ),
    };

    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("wire-placer: {msg}");
            ExitCode::from(1)
        }
    }
}

/// The little state this bot tracks off the event stream: which seat it is
/// this hand, its board so far (to know each row's free capacity), and the
/// cards dealt to it that haven't been placed yet.
#[derive(Default)]
struct Table {
    seat: usize,
    board: Board,
    dealt: Vec<Card>,
}

impl Table {
    fn hand_start(&mut self, seat: usize) {
        self.seat = seat;
        self.board = Board::new();
        self.dealt.clear();
    }

    fn observe(&mut self, ev: &OfcEvent) {
        match ev {
            OfcEvent::Deal { seat, cards, .. } if *seat == self.seat => {
                self.dealt.extend(cards.iter().copied());
            }
            OfcEvent::Place {
                seat, placements, ..
            } if *seat == self.seat => {
                for placement in placements {
                    self.board.push(*placement);
                }
                self.dealt.clear();
            }
            _ => {}
        }
    }

    /// The filler rule: lowest cards first, placed bottom-first as rows have
    /// room, the rest discarded.
    fn filler_action(&self, place: u8) -> OfcAction {
        let mut sorted = self.dealt.clone();
        sorted.sort_unstable_by_key(|card| card.index());
        let discards = sorted.split_off(place as usize);

        let mut free = [
            self.board.free(Row::Bottom),
            self.board.free(Row::Middle),
            self.board.free(Row::Top),
        ];
        let placements = sorted
            .into_iter()
            .map(|card| {
                let index = free
                    .iter()
                    .position(|&n| n > 0)
                    .expect("the spec's card math guarantees room for every placement");
                free[index] -= 1;
                Placement {
                    card,
                    row: ROWS[index],
                }
            })
            .collect();

        OfcAction {
            placements,
            discards,
        }
    }
}

/// Read arena messages until the match ends or the stream closes.
fn play<R: BufRead, W: Write>(mut reader: R, mut writer: W, sleep_ms: u64) -> Result<(), String> {
    let mut table = Table::default();
    loop {
        let msg = match read_msg::<_, OfcArenaMsg>(&mut reader) {
            Ok(msg) => msg,
            Err(WireError::Closed) => return Ok(()),
            Err(e) => return Err(e.to_string()),
        };
        match msg {
            OfcArenaMsg::Hello { .. } => {
                write_msg(&mut writer, &OfcBotMsg::Join {}).map_err(|e| e.to_string())?;
            }
            OfcArenaMsg::HandStart { seat, .. } => table.hand_start(seat),
            OfcArenaMsg::Event { ev, .. } => table.observe(&ev),
            OfcArenaMsg::Act {
                decision: OfcDecision::Place { place, .. },
                ..
            } => {
                if sleep_ms > 0 {
                    std::thread::sleep(Duration::from_millis(sleep_ms));
                }
                let action = table.filler_action(place);
                write_msg(&mut writer, &OfcBotMsg::Action { action }).map_err(|e| e.to_string())?;
            }
            OfcArenaMsg::MatchEnd {} => return Ok(()),
            // joined ack / hand-end / anything newer: nothing to say.
            _ => continue,
        }
    }
}

fn next_value(args: &mut impl Iterator<Item = String>, flag: &str) -> String {
    args.next()
        .unwrap_or_else(|| fail(&format!("{flag} expects a value")))
}

fn fail(msg: &str) -> ! {
    eprintln!("wire-placer: {msg}");
    std::process::exit(1)
}

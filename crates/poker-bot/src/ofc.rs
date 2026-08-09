//! The OFC-protocol session loop: board tracking off the event stream and
//! the arena's foul-avoiding greedy placement strategy per `act`.
//!
//! Strategy is [`poker_arena::ofc::OfcGreedy`] — the same one-ply,
//! royalty-aware, foul-avoiding search the arena ships as `builtin:greedy`
//! — so this wire bot and the strongest builtin can never drift apart.

use std::io::{BufRead, Write};

use poker_arena::ofc::{OfcActionRequest, OfcBot, OfcGreedy};
use poker_core::card::Card;
use poker_core::ofc::{Board, MiddleKind, OfcArenaMsg, OfcBotMsg, OfcDecision, OfcEvent};
use poker_wire::framing::{WireError, read_msg, write_msg};

/// Table state reconstructed from the OFC event stream.
#[derive(Default)]
struct OfcTable {
    hand_no: u64,
    seat: usize,
    boards: Vec<Board>,
    fantasyland: Vec<Option<u8>>,
    dealt: Vec<Card>,
}

impl OfcTable {
    fn hand_start(&mut self, hand_no: u64, seat: usize, seats: usize) {
        self.hand_no = hand_no;
        self.seat = seat;
        self.boards = vec![Board::new(); seats];
        self.fantasyland = vec![None; seats];
        self.dealt.clear();
    }

    fn observe(&mut self, ev: &OfcEvent) {
        match ev {
            OfcEvent::Fantasyland { seat, cards } => {
                self.fantasyland[*seat] = Some(*cards);
            }
            OfcEvent::Deal { seat, cards, .. } if *seat == self.seat => {
                self.dealt.extend(cards.iter().copied());
            }
            OfcEvent::Place {
                seat, placements, ..
            } => {
                // A fantasyland opponent's placements arrive empty (hidden
                // until showdown); its board simply stays unknown, which is
                // exactly what this seat may know.
                for placement in placements {
                    self.boards[*seat].push(*placement);
                }
                if *seat == self.seat {
                    self.dealt.clear();
                }
            }
            _ => {}
        }
    }
}

/// Run an OFC session whose `hello` has already been read.
pub fn run<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    hello: OfcArenaMsg,
) -> Result<(), String> {
    let OfcArenaMsg::Hello {
        game_id,
        seat_count,
        ..
    } = hello
    else {
        return Err("expected a hello as the first message".into());
    };

    // The middle-row evaluator is the one rules fact the bot must know from
    // the game id: 2-7 low for ofc-27, high everywhere else.
    let middle = if game_id == "ofc-27" {
        MiddleKind::DeuceToSeven
    } else {
        MiddleKind::High
    };
    let mut greedy = OfcGreedy::new("poker-bot", middle);
    write_msg(writer, &OfcBotMsg::Join {}).map_err(|e| e.to_string())?;

    let mut table = OfcTable::default();
    loop {
        let msg = match read_msg::<_, OfcArenaMsg>(reader) {
            Ok(msg) => msg,
            Err(WireError::Closed) => return Ok(()),
            Err(e) => return Err(e.to_string()),
        };
        match msg {
            OfcArenaMsg::HandStart { hand_no, seat } => {
                table.hand_start(hand_no, seat, seat_count);
            }
            OfcArenaMsg::Event { ev, .. } => table.observe(&ev),
            OfcArenaMsg::Act {
                decision: OfcDecision::Place { place, discard },
                seat,
                hand_no,
                ..
            } => {
                let request = OfcActionRequest {
                    hand_no,
                    seat,
                    dealt: &table.dealt,
                    place,
                    discard,
                    boards: &table.boards,
                    fantasyland: &table.fantasyland,
                };
                let action = greedy
                    .place(&request)
                    .map_err(|fault| format!("greedy faulted: {fault:?}"))?;
                write_msg(writer, &OfcBotMsg::Action { action }).map_err(|e| e.to_string())?;
            }
            OfcArenaMsg::MatchEnd {} => return Ok(()),
            // hello (repeated), joined, hand-end, unknown: nothing to do.
            _ => {}
        }
    }
}

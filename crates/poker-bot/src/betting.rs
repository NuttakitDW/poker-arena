//! The betting-protocol session loop: `hello`/`join`, event ingestion into
//! [`Table`], and one [`Policy`] decision per `act`.

use std::io::{BufRead, Write};

use poker_core::game::spec::GameSpec;
use poker_wire::action::Action;
use poker_wire::framing::{WireError, read_msg, write_msg};
use poker_wire::message::{ArenaMsg, BotMsg, WireDecision};

use crate::policy::Policy;
use crate::table::Table;

/// What answers decisions: the equity policy when the game id is known,
/// otherwise a check/call floor so a newer arena still gets legal answers.
enum Brain {
    Policy(Box<Policy>),
    Caller,
}

impl Brain {
    fn decide(&mut self, decision: &WireDecision, table: &Table, deadline: Option<u64>) -> Action {
        match self {
            Brain::Policy(policy) => policy.decide(decision, table, deadline),
            Brain::Caller => caller_action(decision),
        }
    }
}

/// The spec-appendix floor strategy: check, else call, else fold; stand
/// pat at draws; post the bring-in.
fn caller_action(decision: &WireDecision) -> Action {
    match decision {
        WireDecision::Wager { check, call, .. } => {
            if *check {
                Action::Check
            } else if call.is_some() {
                Action::Call
            } else {
                Action::Fold
            }
        }
        WireDecision::Draw { .. } => Action::Discard { cards: Vec::new() },
        WireDecision::BringIn { .. } => Action::BringIn,
    }
}

/// Run a betting session whose `hello` has already been read.
pub fn run<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    hello: ArenaMsg,
    seed: u64,
) -> Result<(), String> {
    let ArenaMsg::Hello {
        game_id,
        stakes,
        seat_count,
        ..
    } = hello
    else {
        return Err("expected a hello as the first message".into());
    };

    let mut brain = match GameSpec::by_id(&game_id, stakes) {
        Some(spec) => Brain::Policy(Box::new(Policy::new(spec, seed))),
        None => {
            eprintln!("poker-bot: unknown game {game_id:?}; playing the check/call floor");
            Brain::Caller
        }
    };
    write_msg(writer, &BotMsg::Join {}).map_err(|e| e.to_string())?;

    let mut table = Table::default();
    loop {
        let msg = match read_msg::<_, ArenaMsg>(reader) {
            Ok(msg) => msg,
            Err(WireError::Closed) => return Ok(()),
            Err(e) => return Err(e.to_string()),
        };
        match msg {
            ArenaMsg::HandStart { seat, .. } => table.hand_start(seat, seat_count),
            ArenaMsg::Event { ev, .. } => table.observe(&ev),
            ArenaMsg::Act {
                decision,
                deadline_ms,
                ..
            } => {
                let action = brain.decide(&decision, &table, deadline_ms);
                write_msg(writer, &BotMsg::Action { action }).map_err(|e| e.to_string())?;
            }
            ArenaMsg::MatchEnd {} => return Ok(()),
            // hello (repeated), joined, hand-end, unknown: nothing to do.
            _ => {}
        }
    }
}

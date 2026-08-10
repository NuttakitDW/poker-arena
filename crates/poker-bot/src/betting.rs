//! The betting-protocol session loop: `hello`/`join`, event ingestion into
//! [`Table`], and one [`Policy`] decision per `act`.

use std::io::{BufRead, Write};
use std::path::Path;

use poker_core::game::spec::GameSpec;
use poker_wire::action::Action;
use poker_wire::framing::{WireError, read_msg, write_msg};
use poker_wire::game::BettingKind;
use poker_wire::message::{ArenaMsg, BotMsg, WireDecision};

use crate::blueprint::Blueprint;
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

/// Run a betting session whose `hello` has already been read. When
/// `blueprints` holds a trained strategy for this game, it drives play;
/// otherwise (and on any unseen infoset) the equity heuristic does.
///
/// A blueprint is only used when its validation stamp says it beats the
/// fallback ([`Blueprint::trusted`]); `trust_unvalidated` bypasses that
/// gate — it exists for the trainer's own validation matches, where the
/// candidate must play *before* it has a stamp.
pub fn run<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    hello: ArenaMsg,
    seed: u64,
    blueprints: Option<&Path>,
    trust_unvalidated: bool,
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

    let mut big_bet = false;
    let mut brain = match GameSpec::by_id(&game_id, stakes) {
        Some(spec) => {
            big_bet = !matches!(spec.betting, BettingKind::FixedLimit { .. });
            let trained = blueprints
                .map(|dir| dir.join(Blueprint::file_name(&game_id)))
                .filter(|path| path.exists())
                .and_then(|path| match Blueprint::load(&path) {
                    Ok(blueprint) if trust_unvalidated || blueprint.trusted() => {
                        eprintln!(
                            "poker-bot: loaded blueprint {} ({} infosets, {} iterations, \
                             validated edge {:?})",
                            path.display(),
                            blueprint.strategy.len(),
                            blueprint.iterations,
                            blueprint.validated_edge
                        );
                        Some(blueprint)
                    }
                    Ok(blueprint) => {
                        eprintln!(
                            "poker-bot: blueprint {} not trusted (validated edge {:?}); \
                             using the equity heuristic — train longer to activate it",
                            path.display(),
                            blueprint.validated_edge
                        );
                        None
                    }
                    Err(e) => {
                        eprintln!("poker-bot: failed to load {}: {e}", path.display());
                        None
                    }
                });
            Brain::Policy(Box::new(match trained {
                Some(blueprint) => Policy::with_blueprint(spec, seed, blueprint),
                None => Policy::new(spec, seed),
            }))
        }
        None => {
            eprintln!("poker-bot: unknown game {game_id:?}; playing the check/call floor");
            Brain::Caller
        }
    };
    write_msg(writer, &BotMsg::Join {}).map_err(|e| e.to_string())?;

    let mut table = Table::default();
    table.big_bet = big_bet;
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
            ArenaMsg::MatchEnd {} => {
                if let Brain::Policy(policy) = &brain {
                    let (hits, fallbacks) = policy.coverage();
                    if hits + fallbacks > 0 {
                        eprintln!(
                            "poker-bot: blueprint answered {hits} decisions, fallback {fallbacks}"
                        );
                    }
                }
                return Ok(());
            }
            // hello (repeated), joined, hand-end, unknown: nothing to do.
            _ => {}
        }
    }
}

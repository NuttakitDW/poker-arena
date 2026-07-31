//! OFC match orchestration.
//!
//! [`run_ofc_match`] plays a whole match: it shuffles a deck per hand,
//! rotates bots through seats for positional fairness, carries fantasyland
//! with the *bot* rather than the seat, drives each hand through
//! [`poker_core::ofc::OfcHandState`], applies the configured fault policy to
//! non-conforming placements, and folds per-hand results into the statistics
//! each bot is judged on. The mechanics documented on [`run_ofc_match`] are
//! normative, not illustrative — they are the statistics-bearing part of the
//! OFC arena.

use std::time::Duration;

use poker_core::card::Deck;
use poker_core::ofc::{Board, OfcError, OfcEvent, OfcHandState, OfcSpec};
use poker_core::rng::Rng64;

use crate::ofc::bot::{OfcActionRequest, OfcBot, OfcHandEnd, OfcHandStart};
use crate::ofc::builtin::filler_action;
use crate::ofc::log::{OfcEventSink, OfcHandMeta};
use crate::stat::RateStats;

/// What happens when a bot misbehaves (illegal placement, timeout,
/// disconnect, crash, protocol garbage).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum OfcFaultPolicy {
    /// Substitute the deterministic filler placement (lowest cards first,
    /// lowest rows first — the rule the OFC contract names) and continue the
    /// match. Faults are counted and reported.
    Substitute,
    /// End the match immediately; the offender forfeits.
    Forfeit,
}

/// Full description of one OFC match.
#[derive(Clone, Debug)]
pub struct OfcMatchConfig {
    pub spec: OfcSpec,
    /// Hands played. Fixed: fantasyland changes how a hand is dealt, never
    /// how many hands there are.
    pub hands: u64,
    pub seed: u64,
    pub fault_policy: OfcFaultPolicy,
    /// Per-action deadline. Enforced as a hard deadline for wire bots;
    /// in-process bots run without deadline enforcement.
    pub timeout: Option<Duration>,
}

/// Errors that prevent a match from running at all (as opposed to in-hand
/// bot misbehavior, which is handled by [`OfcFaultPolicy`]).
#[derive(Debug, thiserror::Error)]
pub enum OfcMatchError {
    #[error("{bots} bots given, but {game} supports {min}..={max} seats")]
    SeatCount {
        bots: usize,
        game: &'static str,
        min: usize,
        max: usize,
    },
    #[error("match config has no hands to play (hands == 0)")]
    Empty,
    /// Should not happen for a valid [`OfcMatchConfig`] (the seat count is
    /// already validated above and fantasyland counts come from the engine's
    /// own settlements); surfaced rather than panicking so a degenerate
    /// config fails cleanly instead of taking the process down.
    #[error("hand setup failed: {0}")]
    Hand(#[from] OfcError),
}

/// One bot's accumulated results over the match.
pub struct OfcBotOutcome {
    pub name: String,
    /// Sum of this bot's per-hand net points over every hand played. Points
    /// are the canonical unit — there are no chips to normalize by.
    pub total_points: i64,
    /// One observation per hand, in points.
    pub stats: RateStats,
    pub fouls: u64,
    /// Hands played *in* fantasyland (entered or stayed).
    pub fantasylands: u64,
    /// Opponents scooped, summed across hands.
    pub scoops: u64,
    /// Royalty points earned across the match (a fouled hand earns none).
    pub royalties: u64,
    pub faults: u64,
}

/// Outcome of a full match.
pub struct OfcMatchResult {
    /// Indexed by bot index (the order bots were passed to
    /// [`run_ofc_match`]).
    pub outcomes: Vec<OfcBotOutcome>,
    pub hands_played: u64,
    /// `Some(bot_index)` if the match ended early by forfeit.
    pub forfeited_by: Option<usize>,
}

/// Reported to an optional progress callback after each hand.
pub struct OfcProgress<'a> {
    pub hands_done: u64,
    pub hands_total: u64,
    /// Interim per-bot standings, in bot argument order.
    pub standings: &'a [OfcStanding],
}

/// One bot's interim standing at a progress tick.
#[derive(Debug, Clone)]
pub struct OfcStanding {
    pub name: String,
    pub total_points: i64,
    /// Mean points per hand so far.
    pub mean: f64,
    /// 95% Student-t half-width of the mean; `None` under two observations.
    pub ci95: Option<f64>,
    pub observations: u64,
    pub faults: u64,
}

/// Play a full match between `bots`.
///
/// # Mechanics
///
/// Let `n = bots.len()`, validated against the variant's seat range. For
/// `hand_no` in `0..config.hands`, a deck is shuffled from
/// `Rng64::from_seed_stream(config.seed, hand_no)` — one distinct stream per
/// hand, never reshuffled within it — and bot `b` sits at seat `(b +
/// hand_no) % n`. The button is always seat 0, so rotating bots is what
/// evens out position. There is no duplicate/deck-mirroring mode: fantasyland
/// carries state from one hand into the next, which makes replaying a deck
/// under a different seating incoherent.
///
/// Fantasyland is tracked per *bot* and translated to per-seat counts for
/// [`OfcHandState::new`]; the settlement's `next_fantasyland` is translated
/// back the same way, so a bot that earns fantasyland plays the next hand in
/// it wherever the rotation seats it.
///
/// Each hand is driven via `request` / `place` / `apply` in a loop; bots
/// receive seat-redacted events and a per-seat view of the boards (a
/// fantasyland seat's board reads empty to everyone else until showdown),
/// while every `sink` receives the unredacted stream. A placement rejected by
/// `apply`, and a bot that fails to answer at all, are the same thing: a
/// fault, counted against that bot and handled per `config.fault_policy` —
/// [`OfcFaultPolicy::Substitute`] plays the contract's filler placement and
/// the match continues, [`OfcFaultPolicy::Forfeit`] ends the match
/// immediately.
///
/// Each bot's [`RateStats`] takes one observation per hand: its net points.
///
/// `config.timeout` applies to wire bots (the CLI passes it to
/// [`crate::ofc::OfcWireBot`]); the runner itself imposes no deadline on
/// in-process bots.
pub fn run_ofc_match(
    config: &OfcMatchConfig,
    bots: &mut [Box<dyn OfcBot>],
    sinks: &mut [&mut dyn OfcEventSink],
    mut on_progress: Option<&mut dyn FnMut(&OfcProgress<'_>)>,
) -> Result<OfcMatchResult, OfcMatchError> {
    let n = bots.len();
    if !config.spec.seats().contains(&n) {
        return Err(OfcMatchError::SeatCount {
            bots: n,
            game: config.spec.name,
            min: config.spec.min_seats,
            max: config.spec.max_seats,
        });
    }
    if config.hands == 0 {
        return Err(OfcMatchError::Empty);
    }

    let mut totals = vec![0i64; n];
    let mut stats: Vec<RateStats> = vec![RateStats::new(); n];
    let mut faults = vec![0u64; n];
    let mut fouls = vec![0u64; n];
    let mut fantasylands = vec![0u64; n];
    let mut scoops = vec![0u64; n];
    let mut royalties = vec![0u64; n];

    // Per-bot fantasyland card count entering the next hand.
    let mut fantasyland: Vec<Option<u8>> = vec![None; n];
    let mut hands_played: u64 = 0;
    let mut forfeited_by: Option<usize> = None;

    for hand_no in 0..config.hands {
        for (bot, count) in fantasyland.iter().enumerate() {
            if count.is_some() {
                fantasylands[bot] += 1;
            }
        }

        let outcome = play_hand(config, bots, sinks, &mut faults, hand_no, &fantasyland)?;

        if let Some(offender) = outcome.forfeited_by {
            forfeited_by = Some(offender);
            break;
        }

        debug_assert_eq!(
            outcome.points.iter().sum::<i64>(),
            0,
            "a settled OFC hand only moves points between seats"
        );
        hands_played += 1;
        for bot in 0..n {
            totals[bot] += outcome.points[bot];
            stats[bot].push(outcome.points[bot] as f64);
            fouls[bot] += u64::from(outcome.fouled[bot]);
            scoops[bot] += u64::from(outcome.scoops[bot]);
            royalties[bot] += u64::from(outcome.royalties[bot]);
        }
        fantasyland = outcome.next_fantasyland;

        if let Some(callback) = on_progress.as_deref_mut() {
            // Built lazily: matches without a callback pay nothing.
            let standings: Vec<OfcStanding> = (0..n)
                .map(|bot| OfcStanding {
                    name: bots[bot].name().to_string(),
                    total_points: totals[bot],
                    mean: stats[bot].mean(),
                    ci95: stats[bot].ci95_half_width(),
                    observations: stats[bot].count(),
                    faults: faults[bot],
                })
                .collect();
            callback(&OfcProgress {
                hands_done: hands_played,
                hands_total: config.hands,
                standings: &standings,
            });
        }
    }

    for sink in sinks.iter_mut() {
        sink.finish();
    }

    let outcomes = (0..n)
        .map(|bot| OfcBotOutcome {
            name: bots[bot].name().to_string(),
            total_points: totals[bot],
            stats: stats[bot].clone(),
            fouls: fouls[bot],
            fantasylands: fantasylands[bot],
            scoops: scoops[bot],
            royalties: royalties[bot],
            faults: faults[bot],
        })
        .collect();

    Ok(OfcMatchResult {
        outcomes,
        hands_played,
        forfeited_by,
    })
}

/// Result of driving one hand to completion (or to a forfeit). Every vector
/// is indexed by *bot*, not seat.
struct HandOutcome {
    points: Vec<i64>,
    fouled: Vec<bool>,
    royalties: Vec<u32>,
    scoops: Vec<u32>,
    next_fantasyland: Vec<Option<u8>>,
    forfeited_by: Option<usize>,
}

impl HandOutcome {
    /// A forfeited hand never settled: nothing is scored, and no bot carries
    /// fantasyland out of it.
    fn forfeit(n: usize, offender: usize) -> HandOutcome {
        HandOutcome {
            points: vec![0; n],
            fouled: vec![false; n],
            royalties: vec![0; n],
            scoops: vec![0; n],
            next_fantasyland: vec![None; n],
            forfeited_by: Some(offender),
        }
    }
}

/// The seat bot `bot` occupies in hand `hand_no`.
fn seat_of_bot(hand_no: u64, bot: usize, n: usize) -> usize {
    (bot + (hand_no % n as u64) as usize) % n
}

/// The bot sitting at `seat` in hand `hand_no` — the inverse of
/// [`seat_of_bot`].
fn bot_of_seat(hand_no: u64, seat: usize, n: usize) -> usize {
    (seat + n - (hand_no % n as u64) as usize) % n
}

/// Drive one hand from `OfcHandState::new` to settlement (or forfeit),
/// delivering redacted events to bots and the unredacted stream to `sinks`.
fn play_hand(
    config: &OfcMatchConfig,
    bots: &mut [Box<dyn OfcBot>],
    sinks: &mut [&mut dyn OfcEventSink],
    faults: &mut [u64],
    hand_no: u64,
    fantasyland: &[Option<u8>],
) -> Result<HandOutcome, OfcMatchError> {
    let n = bots.len();
    let mut by_seat: Vec<Option<u8>> = vec![None; n];
    for (bot, count) in fantasyland.iter().enumerate() {
        by_seat[seat_of_bot(hand_no, bot, n)] = *count;
    }

    let deck = Deck::shuffled(&mut Rng64::from_seed_stream(config.seed, hand_no));
    let (mut state, setup) = OfcHandState::new(&config.spec, n, &by_seat, hand_no, deck)?;

    if !sinks.is_empty() {
        // `seats[seat]` = the bot name sitting there this hand; only worth
        // building when a sink is actually watching.
        let seats: Vec<String> = (0..n)
            .map(|seat| bots[bot_of_seat(hand_no, seat, n)].name().to_string())
            .collect();
        for sink in sinks.iter_mut() {
            sink.hand_start(hand_no, &seats);
        }
    }
    for (bot, entry) in bots.iter_mut().enumerate() {
        entry.hand_start(&OfcHandStart {
            hand_no,
            seat: seat_of_bot(hand_no, bot, n),
            fantasyland: fantasyland[bot],
        });
    }
    deliver_events(&setup, &state, hand_no, bots, sinks);

    // Seats whose bot faulted this hand, whether or not the policy
    // substituted a placement and let the hand continue.
    let mut faulted_seats: Vec<usize> = Vec::new();
    while let Some(request) = state.request() {
        let seat = request.seat;
        let bot = bot_of_seat(hand_no, seat, n);
        let boards = visible_boards(&state, seat);
        let answer = bots[bot].place(&OfcActionRequest {
            hand_no,
            seat,
            dealt: &request.dealt,
            place: request.place,
            discard: request.discard,
            boards: &boards,
            fantasyland: state.fantasyland(),
        });

        // A transport fault (Err) and a placement the engine rejects are the
        // same thing to the arena: a fault, handled per policy.
        let applied = answer
            .map_err(|_| ())
            .and_then(|action| state.apply(&action).map_err(|_| ()));
        let events = match applied {
            Ok(events) => events,
            Err(()) => {
                faults[bot] += 1;
                if !faulted_seats.contains(&seat) {
                    faulted_seats.push(seat);
                }
                match config.fault_policy {
                    OfcFaultPolicy::Substitute => {
                        let board = state.boards()[seat].clone();
                        let substitute = filler_action(&request.dealt, request.place, &board);
                        state
                            .apply(&substitute)
                            .expect("the filler placement is legal by construction")
                    }
                    OfcFaultPolicy::Forfeit => {
                        // The evidence hand is closed out even though it
                        // never settled: a buffered sink needs the boundary
                        // to decide whether to keep it.
                        faulted_seats.sort_unstable();
                        for sink in sinks.iter_mut() {
                            sink.hand_end(&OfcHandMeta {
                                points: vec![0; n],
                                faulted: faulted_seats.clone(),
                                forfeited: true,
                            });
                        }
                        return Ok(HandOutcome::forfeit(n, bot));
                    }
                }
            }
        };
        deliver_events(&events, &state, hand_no, bots, sinks);
    }

    let settlement = state
        .settlement()
        .expect("the loop above exits only when every seat has placed");

    let mut outcome = HandOutcome {
        points: vec![0; n],
        fouled: vec![false; n],
        royalties: vec![0; n],
        scoops: vec![0; n],
        next_fantasyland: vec![None; n],
        forfeited_by: None,
    };
    for bot in 0..n {
        let seat = seat_of_bot(hand_no, bot, n);
        outcome.points[bot] = settlement.points[seat];
        outcome.fouled[bot] = settlement.fouled[seat];
        outcome.royalties[bot] = settlement.royalties[seat];
        outcome.scoops[bot] = settlement.scoops[seat];
        outcome.next_fantasyland[bot] = settlement.next_fantasyland[seat];
    }

    for entry in bots.iter_mut() {
        entry.hand_end(&OfcHandEnd {
            hand_no,
            points: settlement.points.clone(),
        });
    }
    faulted_seats.sort_unstable();
    for sink in sinks.iter_mut() {
        sink.hand_end(&OfcHandMeta {
            points: settlement.points.clone(),
            faulted: faulted_seats.clone(),
            forfeited: false,
        });
    }

    Ok(outcome)
}

/// The boards as `viewer` may see them: its own as it really stands, every
/// other seat's as the table sees it — empty while that seat plays the hand
/// in fantasyland.
fn visible_boards(state: &OfcHandState, viewer: usize) -> Vec<Board> {
    state
        .boards()
        .iter()
        .enumerate()
        .map(|(seat, board)| {
            if seat == viewer || state.fantasyland()[seat].is_none() {
                board.clone()
            } else {
                Board::new()
            }
        })
        .collect()
}

/// Forward `events` to every bot (seat-redacted) and to every sink
/// (unredacted).
fn deliver_events(
    events: &[OfcEvent],
    state: &OfcHandState,
    hand_no: u64,
    bots: &mut [Box<dyn OfcBot>],
    sinks: &mut [&mut dyn OfcEventSink],
) {
    let n = bots.len();
    for event in events {
        for (bot, entry) in bots.iter_mut().enumerate() {
            entry.event(&state.redacted_for(event, seat_of_bot(hand_no, bot, n)));
        }
        for sink in sinks.iter_mut() {
            sink.event(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ofc::builtin::{OfcFiller, OfcGreedy, OfcRandom};
    use poker_core::ofc::{OFC, OFC_PINEAPPLE};

    fn config(spec: OfcSpec, hands: u64, seed: u64, policy: OfcFaultPolicy) -> OfcMatchConfig {
        OfcMatchConfig {
            spec,
            hands,
            seed,
            fault_policy: policy,
            timeout: None,
        }
    }

    #[test]
    fn seat_mapping_is_a_bijection_every_hand() {
        for n in 2..=4usize {
            for hand_no in 0..10u64 {
                let seats: Vec<usize> = (0..n).map(|bot| seat_of_bot(hand_no, bot, n)).collect();
                let mut sorted = seats.clone();
                sorted.sort_unstable();
                assert_eq!(sorted, (0..n).collect::<Vec<_>>());
                for (bot, seat) in seats.iter().enumerate() {
                    assert_eq!(bot_of_seat(hand_no, *seat, n), bot);
                }
            }
        }
    }

    #[test]
    fn every_bot_visits_every_seat_over_a_rotation() {
        let hands = 12;
        let mut bots: Vec<Box<dyn OfcBot>> = vec![
            Box::new(OfcGreedy::new("greedy", OFC.middle)),
            Box::new(OfcRandom::new("random", 3)),
            Box::new(OfcFiller::new("filler")),
        ];
        let result = run_ofc_match(
            &config(OFC, hands, 11, OfcFaultPolicy::Substitute),
            &mut bots,
            &mut [],
            None,
        )
        .unwrap();

        assert_eq!(result.hands_played, hands);
        let total: i64 = result.outcomes.iter().map(|o| o.total_points).sum();
        assert_eq!(total, 0, "points only move between bots");
    }

    #[test]
    fn rejects_bot_count_outside_the_seat_range() {
        let mut bots: Vec<Box<dyn OfcBot>> = vec![Box::new(OfcFiller::new("solo"))];
        assert!(matches!(
            run_ofc_match(
                &config(OFC, 5, 1, OfcFaultPolicy::Substitute),
                &mut bots,
                &mut [],
                None
            ),
            Err(OfcMatchError::SeatCount { bots: 1, .. })
        ));
    }

    #[test]
    fn rejects_empty_match() {
        let mut bots: Vec<Box<dyn OfcBot>> =
            vec![Box::new(OfcFiller::new("a")), Box::new(OfcFiller::new("b"))];
        assert!(matches!(
            run_ofc_match(
                &config(OFC_PINEAPPLE, 0, 1, OfcFaultPolicy::Substitute),
                &mut bots,
                &mut [],
                None
            ),
            Err(OfcMatchError::Empty)
        ));
    }

    #[test]
    fn progress_standings_stay_zero_sum_at_every_tick() {
        let hands = 8;
        let mut bots: Vec<Box<dyn OfcBot>> = vec![
            Box::new(OfcGreedy::new("greedy", OFC_PINEAPPLE.middle)),
            Box::new(OfcRandom::new("random", 2)),
        ];
        let mut ticks: Vec<(u64, i64, u64)> = Vec::new();
        let mut callback = |progress: &OfcProgress<'_>| {
            let total: i64 = progress.standings.iter().map(|s| s.total_points).sum();
            ticks.push((
                progress.hands_done,
                total,
                progress.standings[0].observations,
            ));
        };
        run_ofc_match(
            &config(OFC_PINEAPPLE, hands, 5, OfcFaultPolicy::Substitute),
            &mut bots,
            &mut [],
            Some(&mut callback),
        )
        .unwrap();

        assert_eq!(ticks.len(), hands as usize, "one tick per hand");
        for (done, total, observations) in &ticks {
            assert_eq!(*total, 0);
            assert_eq!(observations, done, "one observation per completed hand");
        }
    }
}

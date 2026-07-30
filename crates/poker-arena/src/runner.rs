//! Match orchestration.
//!
//! [`run_match`] plays a whole match: it shuffles decks, rotates bots
//! through seats for positional fairness, drives each hand through
//! [`poker_core::game::HandState`], applies the configured fault policy to
//! non-conforming bot actions, and folds per-hand results into the
//! statistics each bot is judged on. See the mechanics documented on
//! [`run_match`] itself for the exact algorithm — it is the
//! statistics-bearing part of the arena, so the algorithm is normative, not
//! just illustrative.

use poker_core::card::Deck;
use poker_core::game::{Action, Event, HandError, HandState};
use poker_core::rng::Rng64;

use crate::bot::{ActionRequest, Bot, HandEnd, HandStart};
use crate::config::{DealingMode, FaultPolicy, MatchConfig};
use crate::log::EventSink;
use crate::stat::RateStats;

/// Reported to an optional progress callback after each deck finishes.
pub struct Progress {
    pub decks_done: u64,
    pub hands_done: u64,
}

/// Errors that prevent a match from running at all (as opposed to in-hand
/// bot misbehavior, which is handled by [`crate::config::FaultPolicy`]).
#[derive(Debug, thiserror::Error)]
pub enum MatchError {
    #[error("{bots} bots given, but {game} supports {min}..={max} seats")]
    SeatCount {
        bots: usize,
        game: &'static str,
        min: u8,
        max: u8,
    },
    #[error("match config has no decks to play (decks == 0)")]
    Empty,
    /// Should not happen for a valid `MatchConfig` (seat count and stacks
    /// are already validated above); surfaced rather than panicking so a
    /// degenerate config fails cleanly instead of taking the process down.
    #[error("hand setup failed: {0}")]
    Hand(#[from] HandError),
}

/// One bot's accumulated results over the match.
pub struct BotOutcome {
    pub name: String,
    /// Sum of this bot's actual per-hand net chips over every hand played
    /// (not the variance-reduced observations used for `stats`).
    pub total_net_chips: i64,
    /// Observations in big blinds *per hand* (Seeded), or big blinds per
    /// hand *averaged over a duplicate rotation set* (Duplicate) — see the
    /// module docs on [`crate::config::DealingMode`].
    pub stats: RateStats,
    pub faults: u64,
}

/// Outcome of a full match.
pub struct MatchResult {
    /// Indexed by bot index (the order bots were passed to `run_match`).
    pub outcomes: Vec<BotOutcome>,
    pub decks_played: u64,
    pub hands_played: u64,
    /// `Some(bot_index)` if the match ended early by forfeit.
    pub forfeited_by: Option<usize>,
}

/// Play a full match between `bots`.
///
/// # Mechanics
///
/// Let `n = bots.len()`. Stacks reset to `config.starting_stack` every hand;
/// the button is always seat 0 — positional fairness comes from rotating
/// *bots* through seats, not moving the button.
///
/// For `deck_no` in `0..config.decks`, a deck is shuffled from
/// `Rng64::from_seed_stream(config.seed, deck_no)`. It is replayed once per
/// rotation: [`DealingMode::Seeded`] plays a single rotation
/// `r = deck_no % n`; [`DealingMode::Duplicate`] plays every rotation
/// `0..n`. For rotation `r`, bot `b` sits at seat `(b + r) % n`.
///
/// Each hand is driven via `HandState::new` / `to_act` / `legal_actions` /
/// `apply` in a loop; bots receive seat-redacted events, the optional `sink`
/// receives the unredacted stream. A bot action rejected by `apply` costs
/// that bot a fault and is handled per `config.fault_policy`:
/// [`FaultPolicy::CheckFold`] substitutes a check (if free) or a fold, and
/// the match continues; [`FaultPolicy::Forfeit`] ends the match immediately.
///
/// Observations pushed to each bot's [`RateStats`] are in big blinds *per
/// hand*: Seeded pushes one observation per hand; Duplicate pushes one
/// observation per deck, equal to the bot's mean net across that deck's
/// rotation set (this averaging is the variance reduction duplicate dealing
/// exists for). A forfeit mid-deck under Duplicate discards that deck's
/// partial observations.
///
/// `config.timeout` is unused in M1 (bots run in-process with no deadline
/// enforcement).
pub fn run_match(
    config: &MatchConfig,
    bots: &mut [Box<dyn Bot>],
    mut sink: Option<&mut dyn EventSink>,
    mut on_progress: Option<&mut dyn FnMut(Progress)>,
) -> Result<MatchResult, MatchError> {
    // Threaded through `play_hand`/`deliver_events` as `&mut Option<&mut dyn
    // EventSink>` rather than reborrowed by value each call: reborrowing a
    // `Option<&mut dyn Trait>` by value across a helper-function boundary
    // inside a loop over-extends the borrow in the eyes of the borrow
    // checker (the elided trait-object lifetime gets unified with the
    // whole-function lifetime). A `&mut` to the `Option` itself reborrows
    // trivially instead.
    let n = bots.len();
    let (min, max) = (*config.spec.seats.start(), *config.spec.seats.end());
    if n < min as usize || n > max as usize {
        return Err(MatchError::SeatCount {
            bots: n,
            game: config.spec.display_name,
            min,
            max,
        });
    }
    if config.decks == 0 {
        return Err(MatchError::Empty);
    }

    let big_blind = config.spec.stakes.big_blind.max(1) as f64;

    let mut totals = vec![0i64; n];
    let mut stats: Vec<RateStats> = vec![RateStats::new(); n];
    let mut faults = vec![0u64; n];

    let mut hand_no: u64 = 0;
    let mut hands_played: u64 = 0;
    let mut decks_done: u64 = 0;
    let mut forfeited_by: Option<usize> = None;

    'decks: for deck_no in 0..config.decks {
        let deck = Deck::shuffled(&mut Rng64::from_seed_stream(config.seed, deck_no));
        let rotations: Vec<usize> = match config.dealing {
            DealingMode::Seeded => vec![(deck_no % n as u64) as usize],
            DealingMode::Duplicate => (0..n).collect(),
        };

        // Per-bot net sum across this deck's rotation set; folded into
        // `stats` as a single observation once the whole set completes (or
        // discarded if a forfeit interrupts it).
        let mut deck_net_sum = vec![0i64; n];

        for r in rotations {
            let outcome = play_hand(
                config,
                bots,
                &mut sink,
                &mut faults,
                hand_no,
                deck.clone(),
                r,
            )?;

            if let Some(offender) = outcome.forfeited_by {
                forfeited_by = Some(offender);
                break 'decks;
            }

            hand_no += 1;
            hands_played += 1;
            for b in 0..n {
                totals[b] += outcome.nets[b];
                deck_net_sum[b] += outcome.nets[b];
            }
            if config.dealing == DealingMode::Seeded {
                for (bot_stats, &net) in stats.iter_mut().zip(&outcome.nets) {
                    bot_stats.push(net as f64 / big_blind);
                }
            }
        }

        if config.dealing == DealingMode::Duplicate {
            for (bot_stats, &sum) in stats.iter_mut().zip(&deck_net_sum) {
                bot_stats.push((sum as f64 / n as f64) / big_blind);
            }
        }

        decks_done += 1;
        if let Some(cb) = on_progress.as_deref_mut() {
            cb(Progress {
                decks_done,
                hands_done: hands_played,
            });
        }
    }

    let outcomes = (0..n)
        .map(|b| BotOutcome {
            name: bots[b].name().to_string(),
            total_net_chips: totals[b],
            stats: stats[b].clone(),
            faults: faults[b],
        })
        .collect();

    Ok(MatchResult {
        outcomes,
        decks_played: decks_done,
        hands_played,
        forfeited_by,
    })
}

/// Result of driving one hand to completion (or to a forfeit).
struct HandOutcome {
    /// Net chips per *bot index*; meaningless (left zeroed) when
    /// `forfeited_by.is_some()`.
    nets: Vec<i64>,
    forfeited_by: Option<usize>,
}

/// Seat occupied by `bot` under rotation `r` (`n` seats total).
fn seat_of_bot(n: usize, r: usize, bot: usize) -> usize {
    (bot + r) % n
}

/// Bot occupying `seat` under rotation `r` — the inverse of `seat_of_bot`.
fn bot_of_seat(n: usize, r: usize, seat: usize) -> usize {
    (seat + n - r % n) % n
}

/// Drive one hand from `HandState::new` to settlement (or forfeit),
/// delivering redacted events to bots and the unredacted stream to `sink`.
fn play_hand(
    config: &MatchConfig,
    bots: &mut [Box<dyn Bot>],
    sink: &mut Option<&mut dyn EventSink>,
    faults: &mut [u64],
    hand_no: u64,
    deck: Deck,
    r: usize,
) -> Result<HandOutcome, MatchError> {
    let n = bots.len();
    let stacks = vec![config.starting_stack; n];
    let (mut state, ev0) = HandState::new(&config.spec, &stacks, 0, hand_no, deck)?;

    if let Some(s) = sink {
        s.hand_start(hand_no);
    }
    for (b, bot) in bots.iter_mut().enumerate() {
        bot.hand_start(&HandStart {
            hand_no,
            seat: seat_of_bot(n, r, b),
            button: 0,
            seat_count: n,
            stacks: stacks.clone(),
        });
    }
    deliver_events(&ev0, n, r, bots, sink);

    let mut forfeited_by = None;
    while let Some(seat) = state.to_act() {
        let bot = bot_of_seat(n, r, seat);
        let legal = state
            .legal_actions()
            .expect("to_act() implies legal_actions()");
        let (street, street_label) = state.street();
        let req = ActionRequest {
            hand_no,
            seat,
            button: 0,
            street,
            street_label,
            hole: state.hole_cards(seat),
            board: state.board(),
            stacks: state.stacks(),
            street_commits: state.street_commits(),
            pot_total: state.pot_total(),
            folded: state.folded(),
            legal: &legal,
        };
        let action = bots[bot].act(&req);

        let events = match state.apply(action) {
            Ok(events) => events,
            Err(_) => {
                faults[bot] += 1;
                match config.fault_policy {
                    FaultPolicy::CheckFold => {
                        let substitute = if legal.check {
                            Action::Check
                        } else {
                            Action::Fold
                        };
                        state.apply(substitute).expect(
                            "check xor fold is always legal (owed is either 0 or >0): engine invariant",
                        )
                    }
                    FaultPolicy::Forfeit => {
                        forfeited_by = Some(bot);
                        break;
                    }
                }
            }
        };
        deliver_events(&events, n, r, bots, sink);
    }

    if let Some(offender) = forfeited_by {
        return Ok(HandOutcome {
            nets: vec![0; n],
            forfeited_by: Some(offender),
        });
    }

    let settlement = state
        .settlement()
        .expect("the loop above exits only when to_act() is None, i.e. is_over()");
    let mut nets = vec![0i64; n];
    for seat in 0..n {
        nets[bot_of_seat(n, r, seat)] = settlement.nets[seat];
    }

    for bot in bots.iter_mut() {
        bot.hand_end(&HandEnd {
            hand_no,
            nets: settlement.nets.clone(),
        });
    }
    if let Some(s) = sink {
        s.hand_end();
    }

    Ok(HandOutcome {
        nets,
        forfeited_by: None,
    })
}

/// Forward `events` to every bot (seat-redacted) and to `sink` (unredacted).
fn deliver_events(
    events: &[Event],
    n: usize,
    r: usize,
    bots: &mut [Box<dyn Bot>],
    sink: &mut Option<&mut dyn EventSink>,
) {
    for e in events {
        for (b, bot) in bots.iter_mut().enumerate() {
            bot.event(&e.redacted_for(Some(seat_of_bot(n, r, b))));
        }
        if let Some(s) = sink {
            s.event(e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::{Caller, Folder, Random, Shover};
    use poker_core::game::{GameSpec, LegalActions, Stakes};
    use std::time::Duration;

    fn nl_config(
        decks: u64,
        seed: u64,
        dealing: DealingMode,
        fault_policy: FaultPolicy,
    ) -> MatchConfig {
        MatchConfig {
            spec: GameSpec::holdem_nl(Stakes {
                small_blind: 50,
                big_blind: 100,
            }),
            decks,
            seed,
            dealing,
            starting_stack: 100 * 100,
            fault_policy,
            timeout: Some(Duration::from_secs(1)),
        }
    }

    fn boxed(bots: Vec<Box<dyn Bot>>) -> Vec<Box<dyn Bot>> {
        bots
    }

    // ---- (a) determinism ----

    #[test]
    fn identical_configs_produce_identical_outcomes() {
        let run = || {
            let config = nl_config(50, 99, DealingMode::Duplicate, FaultPolicy::CheckFold);
            let mut bots = boxed(vec![
                Box::new(Caller::new("caller")),
                Box::new(Random::new("random", 7)),
            ]);
            run_match(&config, &mut bots, None, None).unwrap()
        };

        let a = run();
        let b = run();

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

    // ---- (b) duplicate observation math ----

    #[test]
    fn duplicate_heads_up_observation_math() {
        let decks = 25;
        let config = nl_config(decks, 3, DealingMode::Duplicate, FaultPolicy::CheckFold);
        let mut bots = boxed(vec![
            Box::new(Caller::new("caller")),
            Box::new(Shover::new("shover")),
        ]);
        let result = run_match(&config, &mut bots, None, None).unwrap();

        assert_eq!(result.hands_played, decks * 2);
        assert_eq!(result.decks_played, decks);
        for o in &result.outcomes {
            assert_eq!(o.stats.count(), decks);
        }
        assert_eq!(
            result.outcomes[0].total_net_chips + result.outcomes[1].total_net_chips,
            0
        );
    }

    // ---- (c) zero-sum ----

    #[test]
    fn total_net_chips_are_always_zero_sum() {
        let dealings = [DealingMode::Seeded, DealingMode::Duplicate];
        for dealing in dealings {
            for n in [3, 4] {
                let config = nl_config(30, 11, dealing, FaultPolicy::CheckFold);
                let mut bots: Vec<Box<dyn Bot>> = vec![
                    Box::new(Folder::new("folder")),
                    Box::new(Caller::new("caller")),
                    Box::new(Shover::new("shover")),
                    Box::new(Random::new("random", 5)),
                ];
                bots.truncate(n);
                let result = run_match(&config, &mut bots, None, None).unwrap();
                let sum: i64 = result.outcomes.iter().map(|o| o.total_net_chips).sum();
                assert_eq!(sum, 0, "dealing={dealing:?} n={n}");
            }
        }
    }

    // ---- (d) fault handling ----

    /// Always returns an illegal action: raises to an absurd total that is
    /// never within the offered bounds (and never legal when only fold/call
    /// is offered either, since it ignores `legal` entirely).
    struct AlwaysIllegal {
        name: String,
    }

    impl Bot for AlwaysIllegal {
        fn name(&self) -> &str {
            &self.name
        }

        fn act(&mut self, _req: &ActionRequest<'_>) -> Action {
            Action::Raise { to: u64::MAX / 2 }
        }
    }

    #[test]
    fn check_fold_policy_faults_every_decision_and_stays_zero_sum() {
        let config = nl_config(20, 21, DealingMode::Seeded, FaultPolicy::CheckFold);
        let mut bots: Vec<Box<dyn Bot>> = vec![
            Box::new(AlwaysIllegal {
                name: "illegal".into(),
            }),
            Box::new(Caller::new("caller")),
        ];
        let result = run_match(&config, &mut bots, None, None).unwrap();

        assert_eq!(result.forfeited_by, None);
        assert_eq!(result.hands_played, 20);
        // Every decision the illegal bot faces is faulted; it always has at
        // least the preflop decision, so it must have faulted at least once
        // per hand it was dealt into (it's in every hand of a 2-bot match).
        assert!(result.outcomes[0].faults >= 20);
        let sum: i64 = result.outcomes.iter().map(|o| o.total_net_chips).sum();
        assert_eq!(sum, 0);
    }

    #[test]
    fn forfeit_policy_stops_the_match_early() {
        let config = nl_config(20, 21, DealingMode::Seeded, FaultPolicy::Forfeit);
        let mut bots: Vec<Box<dyn Bot>> = vec![
            Box::new(AlwaysIllegal {
                name: "illegal".into(),
            }),
            Box::new(Caller::new("caller")),
        ];
        let result = run_match(&config, &mut bots, None, None).unwrap();

        assert!(result.forfeited_by.is_some());
        // The offending bot is whichever one held the illegal strategy —
        // seat rotation may put it in either bot slot, but only one bot
        // implementation ever misbehaves.
        let offender = result.forfeited_by.unwrap();
        assert_eq!(result.outcomes[offender].name, "illegal");
        assert!(result.hands_played < 20);
    }

    // ---- (e) seeded rotation ----

    struct SeatProbe {
        name: String,
        seats_seen: std::sync::Arc<std::sync::Mutex<Vec<usize>>>,
    }

    impl Bot for SeatProbe {
        fn name(&self) -> &str {
            &self.name
        }
        fn hand_start(&mut self, info: &HandStart) {
            self.seats_seen.lock().unwrap().push(info.seat);
        }
        fn act(&mut self, req: &ActionRequest<'_>) -> Action {
            if req.legal.check {
                Action::Check
            } else {
                Action::Fold
            }
        }
    }

    #[test]
    fn seeded_rotation_actually_rotates_seats() {
        let seats_seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let config = nl_config(4, 0, DealingMode::Seeded, FaultPolicy::CheckFold);
        let mut bots: Vec<Box<dyn Bot>> = vec![
            Box::new(SeatProbe {
                name: "probe".into(),
                seats_seen: seats_seen.clone(),
            }),
            Box::new(Folder::new("folder")),
        ];
        let result = run_match(&config, &mut bots, None, None).unwrap();
        assert_eq!(result.hands_played, 4);

        let seen = seats_seen.lock().unwrap().clone();
        assert_eq!(seen, vec![0, 1, 0, 1]);
    }

    // ---- error paths ----

    #[test]
    fn rejects_bot_count_outside_seat_range() {
        let config = nl_config(1, 1, DealingMode::Seeded, FaultPolicy::CheckFold);
        let mut bots: Vec<Box<dyn Bot>> = vec![Box::new(Caller::new("solo"))];
        assert!(matches!(
            run_match(&config, &mut bots, None, None),
            Err(MatchError::SeatCount { bots: 1, .. })
        ));
    }

    #[test]
    fn rejects_empty_match() {
        let config = nl_config(0, 1, DealingMode::Seeded, FaultPolicy::CheckFold);
        let mut bots: Vec<Box<dyn Bot>> =
            vec![Box::new(Caller::new("a")), Box::new(Caller::new("b"))];
        assert!(matches!(
            run_match(&config, &mut bots, None, None),
            Err(MatchError::Empty)
        ));
    }

    #[test]
    fn legal_actions_smoke() {
        // Sanity check that `LegalActions` is reachable the way tests above
        // rely on (`req.legal.check`), guarding against an accidental
        // re-export break.
        let la = LegalActions::default();
        assert!(!la.check);
    }
}

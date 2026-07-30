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

/// Seed salt separating reshuffle RNG streams from deck-shuffle streams.
const RESHUFFLE_SALT: u64 = 0x5245_5348_5546_4C31;

/// Seed salt separating per-deck seating-arrangement streams from both of
/// the above.
const SEATING_SALT: u64 = 0x5345_4154_494E_4731;

use crate::behavior::BehaviorStats;
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
    /// Observations in [`Stakes::rate_unit`](poker_core::game::Stakes::rate_unit)
    /// units *per hand* (Seeded), or that same unit *averaged over a
    /// duplicate rotation set* (Duplicate) — see the module docs on
    /// [`crate::config::DealingMode`]. The unit is the big blind for blind
    /// games (hold'em, Omaha, draw) and the small bet for stud games —
    /// so cross-family comparisons of this field are not apples-to-apples.
    pub stats: RateStats,
    pub faults: u64,
    /// This bot's behavioral profile (VPIP/PFR/AF/WTSD/WSD/fold rate)
    /// accumulated over every completed hand of the match.
    pub behavior: BehaviorStats,
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
/// `config.timeout` applies to wire bots (the CLI passes it to `WireBot`);
/// the runner itself imposes no deadline on in-process bots.
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

    let rate_unit = config.spec.stakes.rate_unit().max(1) as f64;

    let mut totals = vec![0i64; n];
    let mut stats: Vec<RateStats> = vec![RateStats::new(); n];
    let mut faults = vec![0u64; n];
    let mut behavior: Vec<BehaviorStats> = vec![BehaviorStats::new(); n];

    let mut hand_no: u64 = 0;
    let mut hands_played: u64 = 0;
    let mut decks_done: u64 = 0;
    let mut forfeited_by: Option<usize> = None;

    'decks: for deck_no in 0..config.decks {
        let deck = Deck::shuffled(&mut Rng64::from_seed_stream(config.seed, deck_no));
        let seating = Seating::for_deck(config.seed, deck_no, n);
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
                &seating,
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
                behavior[b].record_hand(
                    &outcome.events,
                    seating.seat_of_bot(r, b),
                    outcome.nets[b],
                );
            }
            if config.dealing == DealingMode::Seeded {
                for (bot_stats, &net) in stats.iter_mut().zip(&outcome.nets) {
                    bot_stats.push(net as f64 / rate_unit);
                }
            }
        }

        if config.dealing == DealingMode::Duplicate {
            for (bot_stats, &sum) in stats.iter_mut().zip(&deck_net_sum) {
                bot_stats.push((sum as f64 / n as f64) / rate_unit);
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
            behavior: behavior[b].clone(),
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
    /// The hand's full unredacted event stream, in order; empty when
    /// `forfeited_by.is_some()` (a forfeited hand has no settled behavior to
    /// record). Owned here rather than cloned again for behavioral
    /// accumulation — it's built by moving the same `Vec<Event>`s already
    /// produced for `deliver_events`, not by copying them a second time.
    events: Vec<Event>,
}

/// One deck's bot↔seat mapping: a random base arrangement (drawn per deck)
/// composed with a cyclic rotation.
///
/// Cyclic rotation alone gives every bot every *position* on the same
/// cards, but preserves the circular order of bots — who acts after whom
/// never changes, so neighbor effects (e.g. sitting behind the maniac)
/// would never average out in multiway play. Randomizing the base
/// arrangement per deck keeps positional fairness exact within a deck
/// while neighbor arrangements average out across decks. Heads-up this
/// reduces to plain mirror pairs.
struct Seating {
    /// `base[bot]` = the bot's seat under rotation 0.
    base: Vec<usize>,
    /// `inverse[seat]` = the bot at that seat under rotation 0.
    inverse: Vec<usize>,
}

impl Seating {
    fn for_deck(seed: u64, deck_no: u64, n: usize) -> Seating {
        let mut base: Vec<usize> = (0..n).collect();
        Rng64::from_seed_stream(seed ^ SEATING_SALT, deck_no).shuffle(&mut base);
        let mut inverse = vec![0; n];
        for (bot, &seat) in base.iter().enumerate() {
            inverse[seat] = bot;
        }
        Seating { base, inverse }
    }

    fn seat_of_bot(&self, r: usize, bot: usize) -> usize {
        let n = self.base.len();
        (self.base[bot] + r) % n
    }

    fn bot_of_seat(&self, r: usize, seat: usize) -> usize {
        let n = self.base.len();
        self.inverse[(seat + n - r % n) % n]
    }
}

/// Drive one hand from `HandState::new` to settlement (or forfeit),
/// delivering redacted events to bots and the unredacted stream to `sink`.
#[allow(clippy::too_many_arguments)]
fn play_hand(
    config: &MatchConfig,
    bots: &mut [Box<dyn Bot>],
    sink: &mut Option<&mut dyn EventSink>,
    faults: &mut [u64],
    hand_no: u64,
    deck: Deck,
    seating: &Seating,
    r: usize,
) -> Result<HandOutcome, MatchError> {
    let n = bots.len();
    let stacks = vec![config.starting_stack; n];
    // Draw-street reshuffles get their own deterministic stream, salted so
    // it never collides with the deck-shuffle streams.
    let reshuffle_rng = Rng64::from_seed_stream(config.seed ^ RESHUFFLE_SALT, hand_no);
    let (mut state, ev0) = HandState::new(&config.spec, &stacks, 0, hand_no, deck, reshuffle_rng)?;
    // Accumulates the hand's unredacted stream for `HandOutcome::events`,
    // built by moving each batch already produced for `deliver_events`
    // rather than cloning it a second time.
    let mut hand_events: Vec<Event> = Vec::new();

    if let Some(s) = sink {
        s.hand_start(hand_no);
    }
    for (b, bot) in bots.iter_mut().enumerate() {
        bot.hand_start(&HandStart {
            hand_no,
            seat: seating.seat_of_bot(r, b),
            button: 0,
            seat_count: n,
            stacks: stacks.clone(),
        });
    }
    deliver_events(&ev0, seating, r, bots, sink);
    hand_events.extend(ev0);

    let mut forfeited_by = None;
    while let Some(seat) = state.to_act() {
        let bot = seating.bot_of_seat(r, seat);
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
            upcards: state.upcards(),
            stacks: state.stacks(),
            street_commits: state.street_commits(),
            pot_total: state.pot_total(),
            folded: state.folded(),
            legal: &legal,
        };
        let action = bots[bot].act(&req);

        // A transport fault (Err) and an action the engine rejects are the
        // same thing to the arena: a fault, handled per policy.
        let applied = action
            .map_err(|_| ())
            .and_then(|a| state.apply(a).map_err(|_| ()));
        let events = match applied {
            Ok(events) => events,
            Err(()) => {
                faults[bot] += 1;
                match config.fault_policy {
                    FaultPolicy::CheckFold => {
                        // The minimal legal action for the decision family in
                        // play: draw phases only accept a discard (stand pat),
                        // a bring-in decision only accepts bring-in/complete,
                        // and betting decisions offer exactly one of
                        // check/fold.
                        let substitute = if legal.draw.is_some() {
                            Action::Discard { cards: Vec::new() }
                        } else if legal.bring_in.is_some() {
                            Action::BringIn
                        } else if legal.check {
                            Action::Check
                        } else {
                            Action::Fold
                        };
                        state.apply(substitute).expect(
                            "each decision family has a minimal legal action: engine invariant",
                        )
                    }
                    FaultPolicy::Forfeit => {
                        forfeited_by = Some(bot);
                        break;
                    }
                }
            }
        };
        deliver_events(&events, seating, r, bots, sink);
        hand_events.extend(events);
    }

    if let Some(offender) = forfeited_by {
        return Ok(HandOutcome {
            nets: vec![0; n],
            forfeited_by: Some(offender),
            events: Vec::new(),
        });
    }

    let settlement = state
        .settlement()
        .expect("the loop above exits only when to_act() is None, i.e. is_over()");
    let mut nets = vec![0i64; n];
    for seat in 0..n {
        nets[seating.bot_of_seat(r, seat)] = settlement.nets[seat];
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
        events: hand_events,
    })
}

/// Forward `events` to every bot (seat-redacted) and to `sink` (unredacted).
fn deliver_events(
    events: &[Event],
    seating: &Seating,
    r: usize,
    bots: &mut [Box<dyn Bot>],
    sink: &mut Option<&mut dyn EventSink>,
) {
    for e in events {
        for (b, bot) in bots.iter_mut().enumerate() {
            bot.event(&e.redacted_for(Some(seating.seat_of_bot(r, b))));
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
            spec: GameSpec::holdem_nl(Stakes::Blinds {
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

        fn act(&mut self, _req: &ActionRequest<'_>) -> Result<Action, crate::bot::BotFault> {
            Ok(Action::Raise { to: u64::MAX / 2 })
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
        fn act(&mut self, req: &ActionRequest<'_>) -> Result<Action, crate::bot::BotFault> {
            Ok(if req.legal.check {
                Action::Check
            } else {
                Action::Fold
            })
        }
    }

    #[test]
    fn seeded_seating_covers_every_seat_and_is_not_purely_cyclic() {
        let seats_seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let decks = 24;
        let config = nl_config(decks, 0, DealingMode::Seeded, FaultPolicy::CheckFold);
        let mut bots: Vec<Box<dyn Bot>> = vec![
            Box::new(SeatProbe {
                name: "probe".into(),
                seats_seen: seats_seen.clone(),
            }),
            Box::new(Folder::new("folder")),
            Box::new(Caller::new("caller")),
            Box::new(Folder::new("folder2")),
        ];
        let result = run_match(&config, &mut bots, None, None).unwrap();
        assert_eq!(result.hands_played, decks);

        let seen = seats_seen.lock().unwrap().clone();
        // Positional fairness: the probe plays every seat within the match.
        for seat in 0..4 {
            assert!(seen.contains(&seat), "probe never sat at seat {seat}");
        }
        // Arrangement randomization: a pure cyclic scheme would produce
        // (seen[0] + d) % 4 forever; the per-deck random arrangement must
        // break that pattern (deterministic for this seed).
        let cyclic: Vec<usize> = (0..seen.len()).map(|d| (seen[0] + d) % 4).collect();
        assert_ne!(seen, cyclic, "seating degenerated to a pure cycle");
    }

    // ---- (f) behavior stats ----

    #[test]
    fn behavior_stats_reflect_bot_strategies_over_a_match() {
        // Caller never folds and never bets/raises; Folder folds whenever it
        // faces a wager and never calls one. Heads-up under Duplicate
        // dealing seats each bot as both SB and BB across the match, so
        // Folder is forced to fold-from-the-blind roughly half the time
        // (facing the SB->BB gap) while Caller, seated the other half,
        // simply calls it — neither bot ever bets, so every hand checks down
        // once blinds are matched.
        let decks = 5;
        let config = nl_config(decks, 42, DealingMode::Duplicate, FaultPolicy::CheckFold);
        let mut bots: Vec<Box<dyn Bot>> = vec![
            Box::new(Caller::new("caller")),
            Box::new(Folder::new("folder")),
        ];
        let result = run_match(&config, &mut bots, None, None).unwrap();

        assert_eq!(result.hands_played, decks * 2);
        for o in &result.outcomes {
            assert_eq!(o.behavior.hands(), result.hands_played);
        }

        let caller = &result.outcomes[0];
        let folder = &result.outcomes[1];
        assert_eq!(caller.name, "caller");
        assert_eq!(folder.name, "folder");

        assert_eq!(caller.behavior.fold_rate(), 0.0, "caller never folds");
        assert_eq!(
            folder.behavior.vpip(),
            0.0,
            "folder never voluntarily pays in — it checks or folds"
        );
        assert!(
            folder.behavior.fold_rate() > 0.0,
            "folder must fold from the blinds at least once"
        );
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

    /// A faulting bot at a bring-in or draw decision must be substituted with
    /// that family's minimal legal action (bring-in / stand pat), never
    /// panic the arena: check and fold are both illegal at those decisions.
    #[test]
    fn check_fold_policy_survives_bring_in_and_draw_decisions() {
        for id in ["stud-fl", "27td-fl"] {
            let spec = GameSpec::by_id(
                id,
                Stakes::Blinds {
                    small_blind: 50,
                    big_blind: 100,
                },
            )
            .unwrap();
            let config = MatchConfig {
                spec,
                decks: 10,
                seed: 3,
                dealing: DealingMode::Seeded,
                starting_stack: 10_000,
                fault_policy: FaultPolicy::CheckFold,
                timeout: None,
            };
            let mut bots: Vec<Box<dyn Bot>> = vec![
                Box::new(AlwaysIllegal {
                    name: "illegal".into(),
                }),
                Box::new(Caller::new("caller")),
            ];
            let result = run_match(&config, &mut bots, None, None).unwrap();
            assert_eq!(result.hands_played, 10, "{id}");
            assert!(result.outcomes[0].faults > 0, "{id}");
            let total: i64 = result.outcomes.iter().map(|o| o.total_net_chips).sum();
            assert_eq!(total, 0, "{id}");
        }
    }
}

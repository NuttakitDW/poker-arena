//! Baseline bots: fixed strategies used as opponents/benchmarks and to
//! exercise the arena's action-handling paths (all-ins, random legal
//! coverage, etc).

use poker_core::card::Card;
use poker_core::game::{Action, BetBounds};
use poker_core::rng::Rng64;

use crate::bot::{ActionRequest, Bot, BotFault};

/// Checks when free, folds when facing a bet. At a stud bring-in decision it
/// posts the bring-in; on a draw street it stands pat.
pub struct Folder {
    name: String,
}

impl Folder {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

impl Bot for Folder {
    fn name(&self) -> &str {
        &self.name
    }

    fn act(&mut self, req: &ActionRequest<'_>) -> Result<Action, BotFault> {
        let legal = req.legal;
        if legal.draw.is_some() {
            return Ok(Action::Discard { cards: Vec::new() });
        }
        if legal.bring_in.is_some() {
            return Ok(Action::BringIn);
        }
        Ok(if legal.check {
            Action::Check
        } else {
            Action::Fold
        })
    }
}

/// Checks when free, calls when facing a bet. Never folds, never raises. At
/// a stud bring-in decision it posts the bring-in; on a draw street it
/// stands pat.
pub struct Caller {
    name: String,
}

impl Caller {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

impl Bot for Caller {
    fn name(&self) -> &str {
        &self.name
    }

    fn act(&mut self, req: &ActionRequest<'_>) -> Result<Action, BotFault> {
        let legal = req.legal;
        if legal.draw.is_some() {
            return Ok(Action::Discard { cards: Vec::new() });
        }
        if legal.bring_in.is_some() {
            return Ok(Action::BringIn);
        }
        Ok(if legal.check {
            Action::Check
        } else {
            Action::Call
        })
    }
}

/// Bets/raises to the maximum whenever offered, else calls, else checks.
/// Exercises all-in paths. At a stud bring-in decision it completes to the
/// small bet; on a draw street it stands pat.
pub struct Shover {
    name: String,
}

impl Shover {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

impl Bot for Shover {
    fn name(&self) -> &str {
        &self.name
    }

    fn act(&mut self, req: &ActionRequest<'_>) -> Result<Action, BotFault> {
        let legal = req.legal;
        if legal.draw.is_some() {
            return Ok(Action::Discard { cards: Vec::new() });
        }
        if legal.bring_in.is_some() {
            let bounds = legal
                .bet
                .expect("a bring-in decision always offers the completion bet");
            return Ok(Action::Bet { to: bounds.max_to });
        }
        // Betting streets offer at most one of `bet`/`raise` per decision
        // (never/facing-a-wager are mutually exclusive), so checking both is
        // just defensive.
        Ok(if let Some(bounds) = legal.raise {
            Action::Raise { to: bounds.max_to }
        } else if let Some(bounds) = legal.bet {
            Action::Bet { to: bounds.max_to }
        } else if legal.call.is_some() {
            Action::Call
        } else {
            Action::Check
        })
    }
}

/// Uniformly picks among the action families offered this decision (check,
/// call, fold, bet, raise), and for bet/raise picks a uniform `to` in
/// `[min_to, max_to]`. At a stud bring-in decision it picks uniformly
/// between posting the bring-in and completing to the small bet. On a draw
/// street it discards a uniform-random `k`-subset (`k` uniform in
/// `0..=max_discards`) of its hole cards. Deterministic for a given seed.
pub struct Random {
    name: String,
    rng: Rng64,
}

impl Random {
    pub fn new(name: impl Into<String>, seed: u64) -> Self {
        Self {
            name: name.into(),
            rng: Rng64::from_seed_stream(seed, 0),
        }
    }
}

/// One offered action family, carrying whatever data is needed to build the
/// concrete `Action` once chosen.
enum Choice {
    Check,
    Call,
    Fold,
    Bet(BetBounds),
    Raise(BetBounds),
}

impl Bot for Random {
    fn name(&self) -> &str {
        &self.name
    }

    fn act(&mut self, req: &ActionRequest<'_>) -> Result<Action, BotFault> {
        let legal = req.legal;
        if let Some(bounds) = legal.draw {
            return Ok(Action::Discard {
                cards: self.random_discards(req.hole, bounds.max_discards),
            });
        }
        if legal.bring_in.is_some() {
            let bounds = legal
                .bet
                .expect("a bring-in decision always offers the completion bet");
            return Ok(if self.rng.below(2) == 0 {
                Action::BringIn
            } else {
                Action::Bet { to: bounds.max_to }
            });
        }

        let mut choices = Vec::with_capacity(5);
        if legal.check {
            choices.push(Choice::Check);
        }
        if legal.call.is_some() {
            choices.push(Choice::Call);
        }
        if legal.fold {
            choices.push(Choice::Fold);
        }
        if let Some(bounds) = legal.bet {
            choices.push(Choice::Bet(bounds));
        }
        if let Some(bounds) = legal.raise {
            choices.push(Choice::Raise(bounds));
        }

        // Contract: `legal` always offers at least one family.
        let idx = self.rng.below(choices.len() as u64) as usize;
        Ok(match choices.swap_remove(idx) {
            Choice::Check => Action::Check,
            Choice::Call => Action::Call,
            Choice::Fold => Action::Fold,
            Choice::Bet(bounds) => Action::Bet {
                to: self.uniform_to(bounds),
            },
            Choice::Raise(bounds) => Action::Raise {
                to: self.uniform_to(bounds),
            },
        })
    }
}

impl Random {
    /// Uniform `to` in `[min_to, max_to]`, inclusive on both ends.
    fn uniform_to(&mut self, bounds: BetBounds) -> u64 {
        let span = bounds.max_to - bounds.min_to + 1;
        self.rng.below(span) + bounds.min_to
    }

    /// A uniform-random `k`-subset of `hole` (order irrelevant), with `k`
    /// itself uniform in `0..=max_discards`.
    fn random_discards(&mut self, hole: &[Card], max_discards: u8) -> Vec<Card> {
        let k = self.rng.below(max_discards as u64 + 1) as usize;
        let mut shuffled = hole.to_vec();
        self.rng.shuffle(&mut shuffled);
        shuffled.truncate(k);
        shuffled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use poker_core::card::{Card, Rank, Suit};
    use poker_core::game::{Chips, DrawBounds, LegalActions};

    /// Build a minimal, owned `ActionRequest`-equivalent scenario: since
    /// `ActionRequest` borrows, tests construct the owned backing data and
    /// the request separately, in the same scope.
    struct Scenario {
        hole: Vec<poker_core::card::Card>,
        board: Vec<poker_core::card::Card>,
        upcards: Vec<Vec<poker_core::card::Card>>,
        stacks: Vec<Chips>,
        street_commits: Vec<Chips>,
        folded: Vec<bool>,
        legal: LegalActions,
    }

    impl Scenario {
        fn new(legal: LegalActions) -> Self {
            Self {
                hole: Vec::new(),
                board: Vec::new(),
                upcards: vec![Vec::new(), Vec::new()],
                stacks: vec![1000, 1000],
                street_commits: vec![0, 0],
                folded: vec![false, false],
                legal,
            }
        }

        fn request(&self) -> ActionRequest<'_> {
            ActionRequest {
                hand_no: 1,
                seat: 0,
                button: 0,
                street: 0,
                street_label: "preflop",
                hole: &self.hole,
                board: &self.board,
                upcards: &self.upcards,
                stacks: &self.stacks,
                street_commits: &self.street_commits,
                pot_total: 0,
                folded: &self.folded,
                legal: &self.legal,
            }
        }
    }

    fn facing_bet() -> LegalActions {
        LegalActions {
            fold: true,
            check: false,
            call: Some(50),
            bet: None,
            raise: Some(BetBounds {
                min_to: 100,
                max_to: 1000,
            }),
            bring_in: None,
            draw: None,
        }
    }

    fn free_check() -> LegalActions {
        LegalActions {
            fold: false,
            check: true,
            call: None,
            bet: Some(BetBounds {
                min_to: 20,
                max_to: 1000,
            }),
            raise: None,
            bring_in: None,
            draw: None,
        }
    }

    fn bet_available_only() -> LegalActions {
        LegalActions {
            fold: false,
            check: true,
            call: None,
            bet: Some(BetBounds {
                min_to: 20,
                max_to: 500,
            }),
            raise: None,
            bring_in: None,
            draw: None,
        }
    }

    fn raise_capped() -> LegalActions {
        // Short stack: the only legal raise is a short all-in, min == max.
        LegalActions {
            fold: true,
            check: false,
            call: Some(30),
            bet: None,
            raise: Some(BetBounds {
                min_to: 45,
                max_to: 45,
            }),
            bring_in: None,
            draw: None,
        }
    }

    /// A draw decision offering up to `max` discards and nothing else.
    fn draw_only(max: u8) -> LegalActions {
        LegalActions {
            fold: false,
            check: false,
            call: None,
            bet: None,
            raise: None,
            bring_in: None,
            draw: Some(DrawBounds { max_discards: max }),
        }
    }

    /// A stud bring-in decision: post `bring_in`, or complete directly to
    /// `completion` (a fixed-limit bet, `min_to == max_to`).
    fn bring_in_offered(bring_in: Chips, completion: Chips) -> LegalActions {
        LegalActions {
            fold: false,
            check: false,
            call: None,
            bet: Some(BetBounds {
                min_to: completion,
                max_to: completion,
            }),
            raise: None,
            bring_in: Some(bring_in),
            draw: None,
        }
    }

    // ---- Folder ----

    #[test]
    fn folder_checks_when_free() {
        let scenario = Scenario::new(free_check());
        let mut bot = Folder::new("folder");
        assert_eq!(bot.act(&scenario.request()).unwrap(), Action::Check);
    }

    #[test]
    fn folder_folds_when_facing_bet() {
        let scenario = Scenario::new(facing_bet());
        let mut bot = Folder::new("folder");
        assert_eq!(bot.act(&scenario.request()).unwrap(), Action::Fold);
    }

    #[test]
    fn folder_stands_pat_on_draw() {
        let scenario = Scenario::new(draw_only(3));
        let mut bot = Folder::new("folder");
        assert_eq!(
            bot.act(&scenario.request()).unwrap(),
            Action::Discard { cards: Vec::new() }
        );
    }

    #[test]
    fn folder_posts_bring_in() {
        let scenario = Scenario::new(bring_in_offered(5, 10));
        let mut bot = Folder::new("folder");
        assert_eq!(bot.act(&scenario.request()).unwrap(), Action::BringIn);
    }

    // ---- Caller ----

    #[test]
    fn caller_checks_when_free() {
        let scenario = Scenario::new(free_check());
        let mut bot = Caller::new("caller");
        assert_eq!(bot.act(&scenario.request()).unwrap(), Action::Check);
    }

    #[test]
    fn caller_calls_when_facing_bet() {
        let scenario = Scenario::new(facing_bet());
        let mut bot = Caller::new("caller");
        assert_eq!(bot.act(&scenario.request()).unwrap(), Action::Call);
    }

    #[test]
    fn caller_stands_pat_on_draw() {
        let scenario = Scenario::new(draw_only(5));
        let mut bot = Caller::new("caller");
        assert_eq!(
            bot.act(&scenario.request()).unwrap(),
            Action::Discard { cards: Vec::new() }
        );
    }

    #[test]
    fn caller_posts_bring_in() {
        let scenario = Scenario::new(bring_in_offered(5, 10));
        let mut bot = Caller::new("caller");
        assert_eq!(bot.act(&scenario.request()).unwrap(), Action::BringIn);
    }

    // ---- Shover ----

    #[test]
    fn shover_raises_to_max_when_facing_bet() {
        let scenario = Scenario::new(facing_bet());
        let mut bot = Shover::new("shover");
        assert_eq!(
            bot.act(&scenario.request()).unwrap(),
            Action::Raise { to: 1000 }
        );
    }

    #[test]
    fn shover_bets_to_max_when_bet_available() {
        let scenario = Scenario::new(bet_available_only());
        let mut bot = Shover::new("shover");
        assert_eq!(
            bot.act(&scenario.request()).unwrap(),
            Action::Bet { to: 500 }
        );
    }

    #[test]
    fn shover_handles_min_eq_max_short_allin_raise() {
        let scenario = Scenario::new(raise_capped());
        let mut bot = Shover::new("shover");
        assert_eq!(
            bot.act(&scenario.request()).unwrap(),
            Action::Raise { to: 45 }
        );
    }

    #[test]
    fn shover_calls_when_no_bet_or_raise_offered() {
        let legal = LegalActions {
            fold: true,
            check: false,
            call: Some(20),
            bet: None,
            raise: None,
            bring_in: None,
            draw: None,
        };
        let scenario = Scenario::new(legal);
        let mut bot = Shover::new("shover");
        assert_eq!(bot.act(&scenario.request()).unwrap(), Action::Call);
    }

    #[test]
    fn shover_checks_when_nothing_else_offered() {
        let legal = LegalActions {
            fold: false,
            check: true,
            call: None,
            bet: None,
            raise: None,
            bring_in: None,
            draw: None,
        };
        let scenario = Scenario::new(legal);
        let mut bot = Shover::new("shover");
        assert_eq!(bot.act(&scenario.request()).unwrap(), Action::Check);
    }

    #[test]
    fn shover_stands_pat_on_draw() {
        let scenario = Scenario::new(draw_only(4));
        let mut bot = Shover::new("shover");
        assert_eq!(
            bot.act(&scenario.request()).unwrap(),
            Action::Discard { cards: Vec::new() }
        );
    }

    #[test]
    fn shover_completes_the_bring_in() {
        let scenario = Scenario::new(bring_in_offered(5, 10));
        let mut bot = Shover::new("shover");
        assert_eq!(
            bot.act(&scenario.request()).unwrap(),
            Action::Bet { to: 10 }
        );
    }

    // ---- Random ----

    fn full_menu() -> LegalActions {
        LegalActions {
            fold: true,
            check: false,
            call: Some(50),
            bet: None,
            raise: Some(BetBounds {
                min_to: 100,
                max_to: 1000,
            }),
            bring_in: None,
            draw: None,
        }
    }

    #[test]
    fn random_is_deterministic_for_same_seed() {
        let scenarios: Vec<Scenario> = (0..100)
            .map(|i| {
                let legal = match i % 4 {
                    0 => facing_bet(),
                    1 => free_check(),
                    2 => bet_available_only(),
                    _ => raise_capped(),
                };
                Scenario::new(legal)
            })
            .collect();

        let run = || {
            let mut bot = Random::new("random", 42);
            scenarios
                .iter()
                .map(|s| bot.act(&s.request()).unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn random_stays_in_bounds_and_covers_families() {
        let mut bot = Random::new("random", 7);
        let scenario = Scenario::new(full_menu());
        let mut saw_check = false;
        let mut saw_call = false;
        let mut saw_fold = false;
        let mut saw_raise = false;

        for _ in 0..2000 {
            match bot.act(&scenario.request()).unwrap() {
                Action::Check => saw_check = true,
                Action::Call => saw_call = true,
                Action::Fold => saw_fold = true,
                Action::Raise { to } => {
                    saw_raise = true;
                    assert!((100..=1000).contains(&to), "raise `to` out of bounds: {to}");
                }
                other => panic!("unexpected action from full menu: {other:?}"),
            }
        }

        // `full_menu` offers fold/call/raise only (no free check).
        assert!(!saw_check);
        assert!(saw_call);
        assert!(saw_fold);
        assert!(saw_raise);
    }

    #[test]
    fn random_bet_to_stays_in_bounds() {
        let mut bot = Random::new("random", 99);
        let scenario = Scenario::new(bet_available_only());
        for _ in 0..500 {
            match bot.act(&scenario.request()).unwrap() {
                Action::Check => {}
                Action::Bet { to } => assert!((20..=500).contains(&to)),
                other => panic!("unexpected action: {other:?}"),
            }
        }
    }

    #[test]
    fn random_handles_min_eq_max_bounds() {
        let mut bot = Random::new("random", 3);
        let scenario = Scenario::new(raise_capped());
        for _ in 0..200 {
            match bot.act(&scenario.request()).unwrap() {
                Action::Fold | Action::Call => {}
                Action::Raise { to } => assert_eq!(to, 45),
                other => panic!("unexpected action: {other:?}"),
            }
        }
    }

    fn five_cards() -> Vec<Card> {
        vec![
            Card::new(Rank::Two, Suit::Clubs),
            Card::new(Rank::Five, Suit::Diamonds),
            Card::new(Rank::Nine, Suit::Hearts),
            Card::new(Rank::Jack, Suit::Spades),
            Card::new(Rank::Ace, Suit::Clubs),
        ]
    }

    #[test]
    fn random_draw_discards_a_distinct_subset_of_its_hole_within_bounds() {
        let mut bot = Random::new("random", 11);
        let mut scenario = Scenario::new(draw_only(3));
        scenario.hole = five_cards();
        let mut saw_zero = false;
        let mut saw_nonzero = false;

        for _ in 0..500 {
            match bot.act(&scenario.request()).unwrap() {
                Action::Discard { cards } => {
                    assert!(cards.len() <= 3, "discarded more than max_discards");
                    let mut seen = Vec::new();
                    for c in &cards {
                        assert!(!seen.contains(c), "duplicate card discarded: {c:?}");
                        assert!(scenario.hole.contains(c), "discarded a card not held");
                        seen.push(*c);
                    }
                    if cards.is_empty() {
                        saw_zero = true;
                    } else {
                        saw_nonzero = true;
                    }
                }
                other => panic!("unexpected action from a draw decision: {other:?}"),
            }
        }
        assert!(saw_zero, "never stood pat over 500 draws");
        assert!(saw_nonzero, "never discarded anything over 500 draws");
    }

    #[test]
    fn random_draw_is_deterministic_for_same_seed() {
        let mut scenario = Scenario::new(draw_only(5));
        scenario.hole = five_cards();
        let run = || {
            let mut bot = Random::new("random", 22);
            (0..50)
                .map(|_| bot.act(&scenario.request()).unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn random_bring_in_covers_both_choices_and_stays_in_bounds() {
        let mut bot = Random::new("random", 4);
        let scenario = Scenario::new(bring_in_offered(5, 10));
        let mut saw_bring_in = false;
        let mut saw_completion = false;

        for _ in 0..200 {
            match bot.act(&scenario.request()).unwrap() {
                Action::BringIn => saw_bring_in = true,
                Action::Bet { to } => {
                    assert_eq!(to, 10);
                    saw_completion = true;
                }
                other => panic!("unexpected action from a bring-in decision: {other:?}"),
            }
        }
        assert!(saw_bring_in);
        assert!(saw_completion);
    }
}

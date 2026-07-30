//! Baseline bots: fixed strategies used as opponents/benchmarks and to
//! exercise the arena's action-handling paths (all-ins, random legal
//! coverage, etc).

use poker_core::game::{Action, BetBounds};
use poker_core::rng::Rng64;

use crate::bot::{ActionRequest, Bot};

/// Checks when free, folds when facing a bet.
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

    fn act(&mut self, req: &ActionRequest<'_>) -> Action {
        if req.legal.check {
            Action::Check
        } else {
            Action::Fold
        }
    }
}

/// Checks when free, calls when facing a bet. Never folds, never raises.
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

    fn act(&mut self, req: &ActionRequest<'_>) -> Action {
        if req.legal.check {
            Action::Check
        } else {
            Action::Call
        }
    }
}

/// Bets/raises to the maximum whenever offered, else calls, else checks.
/// Exercises all-in paths.
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

    fn act(&mut self, req: &ActionRequest<'_>) -> Action {
        // Betting streets offer at most one of `bet`/`raise` per decision
        // (never/facing-a-wager are mutually exclusive), so checking both is
        // just defensive.
        if let Some(bounds) = req.legal.raise {
            Action::Raise { to: bounds.max_to }
        } else if let Some(bounds) = req.legal.bet {
            Action::Bet { to: bounds.max_to }
        } else if req.legal.call.is_some() {
            Action::Call
        } else {
            Action::Check
        }
    }
}

/// Uniformly picks among the action families offered this decision (check,
/// call, fold, bet, raise), and for bet/raise picks a uniform `to` in
/// `[min_to, max_to]`. Deterministic for a given seed. Never constructs
/// `Discard`/`BringIn` — it only plays the betting-street families.
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

    fn act(&mut self, req: &ActionRequest<'_>) -> Action {
        let legal = req.legal;
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
        match choices.swap_remove(idx) {
            Choice::Check => Action::Check,
            Choice::Call => Action::Call,
            Choice::Fold => Action::Fold,
            Choice::Bet(bounds) => Action::Bet {
                to: self.uniform_to(bounds),
            },
            Choice::Raise(bounds) => Action::Raise {
                to: self.uniform_to(bounds),
            },
        }
    }
}

impl Random {
    /// Uniform `to` in `[min_to, max_to]`, inclusive on both ends.
    fn uniform_to(&mut self, bounds: BetBounds) -> u64 {
        let span = bounds.max_to - bounds.min_to + 1;
        self.rng.below(span) + bounds.min_to
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use poker_core::game::{Chips, LegalActions};

    /// Build a minimal, owned `ActionRequest`-equivalent scenario: since
    /// `ActionRequest` borrows, tests construct the owned backing data and
    /// the request separately, in the same scope.
    struct Scenario {
        hole: Vec<poker_core::card::Card>,
        board: Vec<poker_core::card::Card>,
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

    // ---- Folder ----

    #[test]
    fn folder_checks_when_free() {
        let scenario = Scenario::new(free_check());
        let mut bot = Folder::new("folder");
        assert_eq!(bot.act(&scenario.request()), Action::Check);
    }

    #[test]
    fn folder_folds_when_facing_bet() {
        let scenario = Scenario::new(facing_bet());
        let mut bot = Folder::new("folder");
        assert_eq!(bot.act(&scenario.request()), Action::Fold);
    }

    // ---- Caller ----

    #[test]
    fn caller_checks_when_free() {
        let scenario = Scenario::new(free_check());
        let mut bot = Caller::new("caller");
        assert_eq!(bot.act(&scenario.request()), Action::Check);
    }

    #[test]
    fn caller_calls_when_facing_bet() {
        let scenario = Scenario::new(facing_bet());
        let mut bot = Caller::new("caller");
        assert_eq!(bot.act(&scenario.request()), Action::Call);
    }

    // ---- Shover ----

    #[test]
    fn shover_raises_to_max_when_facing_bet() {
        let scenario = Scenario::new(facing_bet());
        let mut bot = Shover::new("shover");
        assert_eq!(bot.act(&scenario.request()), Action::Raise { to: 1000 });
    }

    #[test]
    fn shover_bets_to_max_when_bet_available() {
        let scenario = Scenario::new(bet_available_only());
        let mut bot = Shover::new("shover");
        assert_eq!(bot.act(&scenario.request()), Action::Bet { to: 500 });
    }

    #[test]
    fn shover_handles_min_eq_max_short_allin_raise() {
        let scenario = Scenario::new(raise_capped());
        let mut bot = Shover::new("shover");
        assert_eq!(bot.act(&scenario.request()), Action::Raise { to: 45 });
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
        assert_eq!(bot.act(&scenario.request()), Action::Call);
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
        assert_eq!(bot.act(&scenario.request()), Action::Check);
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
                .map(|s| bot.act(&s.request()))
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
            match bot.act(&scenario.request()) {
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
            match bot.act(&scenario.request()) {
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
            match bot.act(&scenario.request()) {
                Action::Fold | Action::Call => {}
                Action::Raise { to } => assert_eq!(to, 45),
                other => panic!("unexpected action: {other:?}"),
            }
        }
    }
}

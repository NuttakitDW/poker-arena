//! External-sampling MCCFR over the abstract game in [`crate::sim`].
//!
//! Standard shape: each iteration deals a real hand, then walks the tree
//! once per traverser. At the traverser's nodes every abstract action is
//! explored and regrets update with regret-matching⁺ (negative regrets
//! clip to zero — faster convergence, same guarantees); at the opponent's
//! nodes one action is sampled from the current strategy and the average
//! strategy accumulates. Infosets are addressed exactly as the player
//! addresses them at match time ([`crate::blueprint::infoset_key`]), so a
//! trained table drops straight into the live policy.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use poker_core::game::spec::GameSpec;
use poker_core::rng::Rng64;

use crate::blueprint::{Blueprint, Bucketer, infoset_key};
use crate::sim::{Sim, State, new_hand};
use crate::table::Table;

/// Minimum accumulated average-strategy weight (≈ opponent-node visits) an
/// infoset needs before it is worth saving.
const MIN_STRATEGY_WEIGHT: f64 = 20.0;

pub struct Trainer {
    sim: Sim,
    bucketer: Bucketer,
    regrets: HashMap<String, Vec<f64>>,
    strategy_sum: HashMap<String, Vec<f64>>,
    rng: Rng64,
    seed: u64,
    pub iterations: u64,
}

/// The persisted trainer state: everything needed to continue a run in a
/// later process. The RNG is not stored — resuming re-streams it from
/// `(seed, iterations)`, which keeps chunked runs deterministic without
/// replaying the original sample path.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct TrainerState {
    pub game_id: String,
    pub iterations: u64,
    pub regrets: HashMap<String, Vec<f64>>,
    pub strategy_sum: HashMap<String, Vec<f64>>,
}

impl Trainer {
    pub fn new(spec: GameSpec, stack: u64, seed: u64) -> Trainer {
        let bucketer = Bucketer::new(&spec);
        Trainer {
            sim: Sim::new(spec, stack),
            bucketer,
            regrets: HashMap::new(),
            strategy_sum: HashMap::new(),
            rng: Rng64::from_seed_stream(seed, 0),
            seed,
            iterations: 0,
        }
    }

    /// Save the accumulated regrets and average strategy for a later
    /// [`Trainer::load_state`].
    pub fn save_state(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let state = TrainerState {
            game_id: self.sim.spec.id.to_string(),
            iterations: self.iterations,
            regrets: self.regrets.clone(),
            strategy_sum: self.strategy_sum.clone(),
        };
        let file = std::fs::File::create(path)?;
        serde_json::to_writer(std::io::BufWriter::new(file), &state).map_err(std::io::Error::other)
    }

    /// Resume from a state saved by [`Trainer::save_state`]. Fails if the
    /// state belongs to a different game.
    pub fn load_state(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        let file = std::fs::File::open(path)?;
        let state: TrainerState = serde_json::from_reader(std::io::BufReader::new(file))
            .map_err(std::io::Error::other)?;
        if state.game_id != self.sim.spec.id {
            return Err(std::io::Error::other(format!(
                "state is for {}, trainer is for {}",
                state.game_id, self.sim.spec.id
            )));
        }
        self.iterations = state.iterations;
        self.regrets = state.regrets;
        self.strategy_sum = state.strategy_sum;
        // A fresh stream per resume keeps chunks deterministic yet
        // non-repeating: stream index = iterations already done.
        self.rng = Rng64::from_seed_stream(self.seed, self.iterations);
        Ok(())
    }

    /// Run until `budget` elapses; returns iterations completed in this run.
    pub fn run_for(&mut self, budget: Duration) -> u64 {
        let start = Instant::now();
        let before = self.iterations;
        while start.elapsed() < budget {
            self.iterate();
        }
        self.iterations - before
    }

    /// Run exactly `n` iterations (tests and calibration).
    pub fn run_iterations(&mut self, n: u64) {
        for _ in 0..n {
            self.iterate();
        }
    }

    fn iterate(&mut self) {
        for traverser in 0..2 {
            let state = new_hand(&self.sim, &mut self.rng);
            self.traverse(&state, traverser);
        }
        self.iterations += 1;
    }

    /// Counterfactual value of `state` for `traverser`, updating regrets
    /// and the average strategy along the way.
    fn traverse(&mut self, state: &State, traverser: usize) -> f64 {
        if state.is_terminal() {
            let u0 = state.utility(&self.sim);
            return if traverser == 0 { u0 } else { -u0 };
        }

        let actor = state.actor();
        let actions = state.actions(&self.sim);
        if actions.len() == 1 {
            return self.traverse(&state.apply(&self.sim, actions[0]), traverser);
        }

        let key = self.key_for(state, actor, actions.len());
        let sigma = {
            let regrets = self
                .regrets
                .entry(key.clone())
                .or_insert_with(|| vec![0.0; actions.len()]);
            regret_matching(regrets)
        };

        if actor == traverser {
            let mut values = Vec::with_capacity(actions.len());
            for action in &actions {
                values.push(self.traverse(&state.apply(&self.sim, *action), traverser));
            }
            let node_value: f64 = sigma.iter().zip(&values).map(|(s, v)| s * v).sum();
            let regrets = self.regrets.get_mut(&key).expect("inserted above");
            for (index, value) in values.iter().enumerate() {
                // Regret-matching⁺: clip at zero.
                regrets[index] = (regrets[index] + value - node_value).max(0.0);
            }
            node_value
        } else {
            let sum = self
                .strategy_sum
                .entry(key)
                .or_insert_with(|| vec![0.0; actions.len()]);
            for (slot, probability) in sum.iter_mut().zip(&sigma) {
                *slot += probability;
            }
            let choice = sample(&sigma, &mut self.rng);
            self.traverse(&state.apply(&self.sim, actions[choice]), traverser)
        }
    }

    /// The player-identical infoset address for `actor` at this state.
    fn key_for(&mut self, state: &State, actor: usize, menu_len: usize) -> String {
        let street = state.street();
        // A minimal Table view of what `actor` can see; equity bucketing
        // reads exactly these fields.
        let mut table = Table::default();
        table.hand_start(actor, 2);
        table.hole = state.hole[actor].clone();
        table.upcards = state.up.to_vec();
        table.board = state.board.clone();
        table.folded = vec![false, false];
        let bucket = self
            .bucketer
            .bucket(&self.sim.spec, &table, street, &mut self.rng);
        infoset_key(street, state.kind(&self.sim), bucket, &state.path, menu_len)
    }

    /// The normalized average strategy. Infosets visited fewer than
    /// [`MIN_STRATEGY_WEIGHT`] times are dropped: their averages are still
    /// mostly regret-matching noise, and the player's equity fallback is a
    /// better answer than an untrained shrug.
    pub fn blueprint(&self) -> Blueprint {
        let mut strategy = std::collections::BTreeMap::new();
        for (key, sum) in &self.strategy_sum {
            let total: f64 = sum.iter().sum();
            if total < MIN_STRATEGY_WEIGHT {
                continue;
            }
            strategy.insert(
                key.clone(),
                sum.iter().map(|s| (s / total) as f32).collect(),
            );
        }
        Blueprint {
            game_id: self.sim.spec.id.to_string(),
            iterations: self.iterations,
            validated_edge: None,
            validated_ci: None,
            strategy,
        }
    }

    pub fn infosets(&self) -> usize {
        self.regrets.len()
    }
}

/// Regret matching: play in proportion to positive regret, uniform when
/// there is none.
fn regret_matching(regrets: &[f64]) -> Vec<f64> {
    let positive: f64 = regrets.iter().filter(|r| **r > 0.0).sum();
    if positive <= f64::EPSILON {
        return vec![1.0 / regrets.len() as f64; regrets.len()];
    }
    regrets
        .iter()
        .map(|r| if *r > 0.0 { r / positive } else { 0.0 })
        .collect()
}

/// Sample an index from a probability vector.
fn sample(sigma: &[f64], rng: &mut Rng64) -> usize {
    let roll = rng.next_u64() as f64 / u64::MAX as f64;
    let mut cumulative = 0.0;
    for (index, probability) in sigma.iter().enumerate() {
        cumulative += probability;
        if roll < cumulative {
            return index;
        }
    }
    sigma.len() - 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use poker_wire::game::Stakes;

    const STAKES: Stakes = Stakes::Blinds {
        small_blind: 50,
        big_blind: 100,
        ante: 0,
    };

    #[test]
    fn regret_matching_normalizes_and_ignores_negatives() {
        let sigma = regret_matching(&[3.0, 1.0, 0.0]);
        assert!((sigma[0] - 0.75).abs() < 1e-12);
        assert!((sigma[1] - 0.25).abs() < 1e-12);
        assert_eq!(sigma[2], 0.0);

        let uniform = regret_matching(&[0.0, 0.0]);
        assert_eq!(uniform, vec![0.5, 0.5]);
    }

    #[test]
    fn a_short_run_produces_a_normalized_blueprint() {
        // holdem-fl has few enough preflop buckets (169) that a modest run
        // pushes popular infosets past the save threshold.
        let spec = GameSpec::by_id("holdem-fl", STAKES).unwrap();
        let mut trainer = Trainer::new(spec, 10_000, 7);
        trainer.run_iterations(2_000);
        let blueprint = trainer.blueprint();
        assert!(!blueprint.strategy.is_empty(), "no infosets survived");
        for (key, probs) in &blueprint.strategy {
            let total: f32 = probs.iter().sum();
            assert!(
                (total - 1.0).abs() < 1e-3,
                "{key}: probabilities sum to {total}"
            );
        }
    }

    #[test]
    fn training_is_deterministic_for_a_seed() {
        let run = || {
            let spec = GameSpec::by_id("holdem-fl", STAKES).unwrap();
            let mut trainer = Trainer::new(spec, 10_000, 3);
            trainer.run_iterations(20);
            trainer.blueprint().strategy
        };
        assert_eq!(run(), run());
    }
}

//! Trained-strategy storage and the infoset addressing shared by the
//! trainer and the player.
//!
//! An information set is addressed by `street|kind|bucket|path`:
//! the street index, the decision kind (`w`ager / `d`raw / bring-`i`n),
//! the actor's equity bucket for that street, and the street's abstract
//! action path so far. The trainer writes average strategies under these
//! keys; the player rebuilds the identical key from its [`Table`] view and
//! looks the strategy up. A miss (unexplored infoset, or a real-game line
//! the abstraction cannot express) simply falls back to the equity
//! heuristic — the blueprint is an upgrade path, never a requirement.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use poker_core::game::spec::GameSpec;
use poker_core::rng::Rng64;

use crate::abstraction::{GamePlan, bucket_of, plan};
use crate::equity::equity;
use crate::iso::canonical_key;
use crate::sim::Kind;
use crate::table::Table;

/// Rollouts behind each bucket assignment. Small on purpose: the iso-keyed
/// cache means each distinct context is estimated once, and bucket edges
/// only need the estimate to land in the right ~decile. This is also the
/// trainer's throughput ceiling — fresh contexts dominate iteration cost.
const BUCKET_SAMPLES: u32 = 32;

/// The infoset address for one decision point. `menu_len` — the number of
/// abstract actions on offer — is part of the address: the same
/// street/bucket/path can surface different menus when a short stack caps
/// the wager list, and regret vectors must never change length under one
/// key.
pub fn infoset_key(street: usize, kind: Kind, bucket: u64, path: &str, menu_len: usize) -> String {
    format!("{street}|{}|{bucket}|{path}|{menu_len}", kind.letter())
}

/// Maps a hand context to its street's equity bucket.
///
/// Two paths: games whose showdown is pure 2-7 on the player's own five
/// cards (`27td-fl`, `27sd-nl`) read the exact class-equity table — a
/// lookup, no sampling, no cache needed; everything else estimates by
/// Monte-Carlo rollouts, cached by the lossless iso key so every
/// isomorphic context is estimated exactly once.
pub struct Bucketer {
    plan: GamePlan,
    cache: HashMap<(usize, u128), f64>,
    /// Exact-table fast path is sound for this game.
    deuce_exact: bool,
}

/// The exact table applies when the *entire* showdown is 2-7 on the
/// player's own cards: one hi side, `DeuceToSevenLow`, `AllOwn`. Split
/// games with a 2-7 half (badeucy, drawmaha-27) still need rollouts for
/// the other half.
fn deuce_exact_applies(spec: &GameSpec) -> bool {
    use poker_core::eval::{EvalKind, HoleUsage};
    spec.showdown.lo.is_none()
        && spec.showdown.hi.kind == EvalKind::DeuceToSevenLow
        && spec.showdown.hi.usage == HoleUsage::AllOwn
}

impl Bucketer {
    pub fn new(spec: &GameSpec) -> Bucketer {
        Bucketer {
            plan: plan(spec),
            cache: HashMap::new(),
            deuce_exact: deuce_exact_applies(spec),
        }
    }

    /// The equity bucket for this bot's context in `table` at `street`.
    pub fn bucket(
        &mut self,
        spec: &GameSpec,
        table: &Table,
        street: usize,
        rng: &mut Rng64,
    ) -> u64 {
        let buckets = self
            .plan
            .streets
            .get(street)
            .map_or(1, |s| s.buckets)
            .max(1);
        if buckets == 1 {
            return 0;
        }
        // Fast path: exact class equity by table lookup (see EquityTable).
        if self.deuce_exact
            && let Some(exact) = crate::deuce::EquityTable::shared().equity(&table.hole)
        {
            return bucket_of(exact, buckets);
        }
        // Context: my private cards, my upcards, everyone else's visible
        // upcards (dead cards shift equity), the board.
        let mut opp_up: Vec<poker_core::card::Card> = Vec::new();
        for (seat, up) in table.upcards.iter().enumerate() {
            if seat != table.seat {
                opp_up.extend(up.iter().copied());
            }
        }
        let my_up: &[poker_core::card::Card] =
            table.upcards.get(table.seat).map_or(&[], Vec::as_slice);
        let key = (
            street,
            canonical_key(&[&table.hole, my_up, &opp_up, &table.board]),
        );
        let e = match self.cache.get(&key) {
            Some(cached) => *cached,
            None => {
                let e = equity(spec, table, rng, BUCKET_SAMPLES);
                self.cache.insert(key, e);
                e
            }
        };
        bucket_of(e, buckets)
    }

    pub fn plan(&self) -> &GamePlan {
        &self.plan
    }
}

/// A trained average strategy for one game.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Blueprint {
    pub game_id: String,
    /// MCCFR iterations (per traverser) behind this strategy.
    pub iterations: u64,
    /// Measured edge of this blueprint over the equity fallback, in the
    /// game's rate unit per 100 hands (bb/100 or BB/100), from the
    /// trainer's validation match. `None` = never validated.
    #[serde(default)]
    pub validated_edge: Option<f64>,
    /// The 95% confidence half-width of that edge, same unit. A blueprint
    /// is only trusted when `edge − ci ≥ 0` — the validation must show a
    /// *statistically significant* win over the fallback, because a noisy
    /// positive mean in a big-bet game promotes noise, and the fallback it
    /// would replace is already a competent player.
    #[serde(default)]
    pub validated_ci: Option<f64>,
    /// Infoset key → action probabilities, aligned with the abstract
    /// action list the simulator offers at that decision point.
    pub strategy: BTreeMap<String, Vec<f32>>,
}

impl Blueprint {
    /// Whether the player should prefer this strategy over its fallback:
    /// a validated, statistically significant non-negative edge.
    pub fn trusted(&self) -> bool {
        match (self.validated_edge, self.validated_ci) {
            (Some(edge), Some(ci)) => edge - ci >= 0.0,
            _ => false,
        }
    }
}

impl Blueprint {
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::fs::File::create(path)?;
        serde_json::to_writer(std::io::BufWriter::new(file), self).map_err(std::io::Error::other)
    }

    pub fn load(path: &Path) -> std::io::Result<Blueprint> {
        let file = std::fs::File::open(path)?;
        serde_json::from_reader(std::io::BufReader::new(file)).map_err(std::io::Error::other)
    }

    /// The conventional file name inside a blueprint directory.
    pub fn file_name(game_id: &str) -> String {
        format!("{game_id}.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use poker_core::card::parse_cards;
    use poker_wire::game::Stakes;

    const STAKES: Stakes = Stakes::Blinds {
        small_blind: 50,
        big_blind: 100,
        ante: 0,
    };

    #[test]
    fn keys_are_stable_and_distinct() {
        assert_eq!(infoset_key(1, Kind::Wager, 12, "cb", 3), "1|w|12|cb|3");
        assert_ne!(
            infoset_key(1, Kind::Wager, 12, "", 2),
            infoset_key(1, Kind::Draw, 12, "", 2)
        );
        assert_ne!(
            infoset_key(1, Kind::Wager, 12, "", 2),
            infoset_key(1, Kind::Wager, 12, "", 3),
            "menu length is part of the address"
        );
    }

    #[test]
    fn bucketer_caches_by_isomorphism() {
        let spec = GameSpec::by_id("holdem-nl", STAKES).unwrap();
        let mut bucketer = Bucketer::new(&spec);
        let mut rng = Rng64::from_seed_stream(1, 0);

        let mut table = Table::default();
        table.hand_start(0, 2);
        table.folded = vec![false, false];
        table.hole = parse_cards("As Kd").unwrap();
        let a = bucketer.bucket(&spec, &table, 0, &mut rng);

        // Same hand under a suit permutation must hit the cache and agree.
        table.hole = parse_cards("Ah Kc").unwrap();
        let b = bucketer.bucket(&spec, &table, 0, &mut rng);
        assert_eq!(a, b);
        assert_eq!(bucketer.cache.len(), 1, "one iso class, one estimate");
    }

    #[test]
    fn blueprints_round_trip_through_disk() {
        let mut strategy = BTreeMap::new();
        strategy.insert("0|w|5|".to_string(), vec![0.25, 0.75]);
        let blueprint = Blueprint {
            game_id: "holdem-nl".to_string(),
            iterations: 123,
            validated_edge: Some(12.5),
            validated_ci: Some(4.0),
            strategy,
        };
        let dir = std::env::temp_dir().join("poker-bot-test-blueprints");
        let path = dir.join(Blueprint::file_name("holdem-nl"));
        blueprint.save(&path).unwrap();
        let back = Blueprint::load(&path).unwrap();
        assert_eq!(back.game_id, "holdem-nl");
        assert_eq!(back.iterations, 123);
        assert_eq!(back.validated_edge, Some(12.5));
        assert!(back.trusted());
        assert_eq!(back.strategy.len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }
}

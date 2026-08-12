//! The **true public game tree** of heads-up fixed-limit 2-7 triple draw.
//!
//! This module is the contract the corrected infoset addressing must
//! honor: a real 27td-fl player's information is their five cards plus
//! the *entire* public history — every betting action from hand start and
//! both players' discard counts in every draw round, in order. Any
//! information set the solver uses must be a merging of true-tree nodes
//! that agree on a *class* of this public history; abstraction may
//! coarsen how finely history is seen (and may remove actions), but must
//! never truncate the tree itself. (The first-generation key reset the
//! path every street — a different, smaller game, and the reason its
//! blueprint lost to history-aware opponents. See KEY_DECISIONS.)
//!
//! Everything here is exact and enumerated: two betting-street templates
//! (preflop opens facing the blind with the blind counting as the first
//! of four capped wagers; postdraw streets open check-or-bet), draw
//! rounds where each player discards 0–5, and dynamic-programming totals
//! over the seven-stage sequence
//!
//! ```text
//! BET(preflop) → DRAW → BET → DRAW → BET → DRAW → BET(showdown)
//! ```
//!
//! The same machinery computes, for a family of *abstraction lenses*
//! (history-class functions applied on top of the true tree), how many
//! history classes and information sets each lens yields — the numbers
//! that decide whether a lens is solvable with meaningful visitation.

use std::fmt::Write as _;

/// One complete betting sequence within a single street.
#[derive(Clone, Debug)]
pub struct Outcome {
    /// Action letters from the street's first actor: `k` check, `b` bet,
    /// `c` call, `r` raise, `f` fold. Preflop uses `c`/`r` facing the
    /// blind and `k` for the big blind's option.
    pub seq: String,
    /// Ends the hand (fold) rather than continuing to the next stage.
    pub fold: bool,
    /// Total wagers on the street when it closed (blind included
    /// preflop; 0 = checked through).
    pub wagers: u8,
}

/// A betting street's full shape: every decision prefix and outcome.
#[derive(Clone, Debug)]
pub struct StreetTemplate {
    /// Seat index (0 = button) acting first.
    pub first_actor: usize,
    /// Every prefix at which some player must act, in discovery order;
    /// `""` is the street's opening decision.
    pub prefixes: Vec<String>,
    pub outcomes: Vec<Outcome>,
}

impl StreetTemplate {
    pub fn continuing(&self) -> usize {
        self.outcomes.iter().filter(|o| !o.fold).count()
    }

    pub fn folds(&self) -> usize {
        self.outcomes.len() - self.continuing()
    }
}

/// The engine's raise cap: four wagers per street, the preflop blind
/// counting as the first.
const WAGER_CAP: u8 = 4;
/// Discard options per player per draw round in the true game (0..=5).
pub const TRUE_DRAW_OPTIONS: usize = 6;

/// The preflop street: heads-up, the button (seat 0) acts first facing
/// the big blind, which counts as wager one. The big blind holds the
/// option: after a plain call it may check or raise.
pub fn preflop_template() -> StreetTemplate {
    let mut template = StreetTemplate {
        first_actor: 0,
        prefixes: Vec::new(),
        outcomes: Vec::new(),
    };
    // state: (seq, wagers, option_pending)
    fn walk(template: &mut StreetTemplate, seq: String, wagers: u8, option: bool) {
        template.prefixes.push(seq.clone());
        if option {
            // Big blind's option: check closes the street, raise reopens.
            close(template, format!("{seq}k"), wagers);
            if wagers < WAGER_CAP {
                walk(template, format!("{seq}r"), wagers + 1, false);
            }
            return;
        }
        // Facing a live wager: fold, call, or raise.
        fold(template, format!("{seq}f"), wagers);
        if seq.is_empty() {
            // Button's plain preflop call leaves the big blind an option.
            walk(template, format!("{seq}c"), wagers, true);
        } else {
            close(template, format!("{seq}c"), wagers);
        }
        if wagers < WAGER_CAP {
            walk(template, format!("{seq}r"), wagers + 1, false);
        }
    }
    fn close(template: &mut StreetTemplate, seq: String, wagers: u8) {
        template.outcomes.push(Outcome {
            seq,
            fold: false,
            wagers,
        });
    }
    fn fold(template: &mut StreetTemplate, seq: String, wagers: u8) {
        template.outcomes.push(Outcome {
            seq,
            fold: true,
            wagers,
        });
    }
    walk(&mut template, String::new(), 1, false);
    template
}

/// A postdraw street: first actor is seat 1 (left of the button); opens
/// check-or-bet, the opening bet counting as wager one.
pub fn postdraw_template() -> StreetTemplate {
    let mut template = StreetTemplate {
        first_actor: 1,
        prefixes: Vec::new(),
        outcomes: Vec::new(),
    };
    fn walk(template: &mut StreetTemplate, seq: String, wagers: u8, checked: bool) {
        template.prefixes.push(seq.clone());
        if wagers == 0 {
            if checked {
                template.outcomes.push(Outcome {
                    seq: format!("{seq}k"),
                    fold: false,
                    wagers: 0,
                });
            } else {
                walk(template, format!("{seq}k"), 0, true);
            }
            walk(template, format!("{seq}b"), 1, checked);
            return;
        }
        template.outcomes.push(Outcome {
            seq: format!("{seq}f"),
            fold: true,
            wagers,
        });
        template.outcomes.push(Outcome {
            seq: format!("{seq}c"),
            fold: false,
            wagers,
        });
        if wagers < WAGER_CAP {
            walk(template, format!("{seq}r"), wagers + 1, checked);
        }
    }
    walk(&mut template, String::new(), 0, false);
    template
}

/// An abstraction lens: how much of the public history a solver's key
/// retains. `TrueTree` retains everything (no merging at all).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Lens {
    /// Full history, draws 0–5: the game itself.
    TrueTree,
    /// Full history, draw menu trimmed to 0–3 (action removal only).
    TrimDraws,
    /// Trimmed draws; past streets' betting collapsed to a per-street
    /// wager tier (0–1 / 2 / 3–4 wagers). Draw-count history stays exact.
    TrimPlusPotTiers,
    /// Trimmed draws; betting collapsed to one cumulative pot tier; draw
    /// history collapsed to the *last* round's counts plus each player's
    /// ever-stood-pat flag.
    ShortMemory,
}

impl Lens {
    pub fn all() -> [Lens; 4] {
        [
            Lens::TrueTree,
            Lens::TrimDraws,
            Lens::TrimPlusPotTiers,
            Lens::ShortMemory,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            Lens::TrueTree => "true tree",
            Lens::TrimDraws => "trim draws 0-3",
            Lens::TrimPlusPotTiers => "trim + pot tiers",
            Lens::ShortMemory => "short draw memory",
        }
    }

    fn draw_options(self) -> u64 {
        match self {
            Lens::TrueTree => 6,
            _ => 4,
        }
    }

    /// History classes a completed betting street contributes.
    fn street_classes(self, template: &StreetTemplate) -> u64 {
        match self {
            Lens::TrueTree | Lens::TrimDraws => template.continuing() as u64,
            // 3 tiers: unraised (0-1 wagers), raised (2), heavy (3-4).
            Lens::TrimPlusPotTiers => 3,
            // Cumulative pot tier only: handled at the stage level.
            Lens::ShortMemory => 3,
        }
    }

    /// History classes a completed draw round contributes (both seats).
    fn draw_classes(self, _round: usize) -> u64 {
        match self {
            Lens::TrueTree => 36,
            Lens::TrimDraws | Lens::TrimPlusPotTiers => 16,
            // Last round only: 16 combos x 4 ever-pat flag pairs, but the
            // *contribution per round* is replacement, not product; the
            // stage math special-cases this lens.
            Lens::ShortMemory => 16,
        }
    }
}

/// One stage's exact statistics under a lens.
#[derive(Clone, Debug)]
pub struct StageStats {
    pub name: &'static str,
    /// Distinct public-history classes entering the stage.
    pub classes_in: u64,
    /// Public decision points (classes x acting prefixes/draw nodes).
    pub decision_points: u64,
    /// Fold leaves this stage produces (betting stages only).
    pub fold_leaves: u64,
    /// History classes continuing to the next stage.
    pub classes_out: u64,
}

/// Full-tree statistics under a lens.
#[derive(Clone, Debug)]
pub struct TreeStats {
    pub lens: Lens,
    pub stages: Vec<StageStats>,
    pub showdown_classes: u64,
    pub total_decision_points: u64,
    /// Information sets with `buckets` private classes per public point.
    pub infosets: u64,
}

/// Exact leaf count of the true tree (folds at every street plus
/// showdown lines), by the same DP the stats use — but over raw
/// sequences, never classes.
pub fn true_tree_leaves() -> u64 {
    let pre = preflop_template();
    let post = postdraw_template();
    let mut lines = 1u64;
    let mut leaves = 0u64;
    for street in 0..4 {
        let template = if street == 0 { &pre } else { &post };
        leaves += lines * template.folds() as u64;
        let continuing = lines * template.continuing() as u64;
        if street < 3 {
            lines = continuing * 36;
        } else {
            leaves += continuing; // showdown lines
        }
    }
    leaves
}

/// Per-round draw decision nodes per entering class: seat 1 decides once,
/// then seat 0 decides knowing seat 1's count.
fn draw_nodes_per_class(options: u64) -> u64 {
    1 + options
}

/// Compute exact stage statistics for `lens` with `buckets` private
/// classes per decision point.
pub fn stats(lens: Lens, buckets: u64) -> TreeStats {
    let pre = preflop_template();
    let post = postdraw_template();
    let stage_names = [
        "bet: predraw",
        "draw 1",
        "bet: post-draw-1",
        "draw 2",
        "bet: post-draw-2",
        "draw 3",
        "bet: final",
    ];

    let mut stages = Vec::new();
    let mut classes = 1u64;
    let mut decisions = 0u64;
    let mut showdown = 0u64;
    let mut betting_street = 0usize;
    // ShortMemory: history = pot tier (3) x last-draw combo x pat flags,
    // so classes are *replaced* each stage rather than multiplied.
    for (stage, name) in stage_names.iter().enumerate() {
        if stage % 2 == 0 {
            // Betting stage.
            let template = if betting_street == 0 { &pre } else { &post };
            let points = classes * template.prefixes.len() as u64;
            let folds = classes * template.folds() as u64;
            let out = match lens {
                Lens::ShortMemory => {
                    // Pot tier replaces betting history entirely.
                    if betting_street == 3 {
                        0
                    } else {
                        3 * draw_memory_classes(lens, betting_street)
                    }
                }
                _ => classes * lens.street_classes(template),
            };
            stages.push(StageStats {
                name,
                classes_in: classes,
                decision_points: points,
                fold_leaves: folds,
                classes_out: if betting_street == 3 { 0 } else { out },
            });
            decisions += points;
            if betting_street == 3 {
                showdown = match lens {
                    Lens::ShortMemory => classes,
                    _ => classes * lens.street_classes(template),
                };
            }
            classes = out;
            betting_street += 1;
        } else {
            // Draw stage.
            let options = lens.draw_options();
            let points = classes * draw_nodes_per_class(options);
            let out = match lens {
                Lens::ShortMemory => classes, // combos folded into next bet stage's replacement
                _ => classes * lens.draw_classes(stage / 2),
            };
            stages.push(StageStats {
                name,
                classes_in: classes,
                decision_points: points,
                fold_leaves: 0,
                classes_out: out,
            });
            decisions += points;
            classes = out;
        }
    }

    TreeStats {
        lens,
        stages,
        showdown_classes: showdown,
        total_decision_points: decisions,
        infosets: decisions * buckets,
    }
}

/// ShortMemory's between-street history state: last-round draw combo
/// (16) x ever-pat flags (4) — before the first draw, 1.
fn draw_memory_classes(lens: Lens, betting_street: usize) -> u64 {
    debug_assert_eq!(lens, Lens::ShortMemory);
    if betting_street == 0 { 1 } else { 64 }
}

/// The explorer's data document: both street templates, per-lens stage
/// stats at the reference bucket count, and the true-tree totals.
pub fn explorer_json(buckets: u64) -> String {
    let mut out = String::with_capacity(1 << 16);
    let template_json = |template: &StreetTemplate| {
        let prefixes: Vec<String> = template
            .prefixes
            .iter()
            .map(|p| format!("\"{p}\""))
            .collect();
        let outcomes: Vec<String> = template
            .outcomes
            .iter()
            .map(|o| {
                format!(
                    "{{\"seq\":\"{}\",\"fold\":{},\"wagers\":{}}}",
                    o.seq, o.fold, o.wagers
                )
            })
            .collect();
        format!(
            "{{\"first_actor\":{},\"prefixes\":[{}],\"outcomes\":[{}]}}",
            template.first_actor,
            prefixes.join(","),
            outcomes.join(",")
        )
    };
    let _ = write!(
        out,
        "{{\"preflop\":{},\"postdraw\":{},\"true_leaves\":{},\"buckets\":{},\"lenses\":[",
        template_json(&preflop_template()),
        template_json(&postdraw_template()),
        true_tree_leaves(),
        buckets
    );
    for (index, lens) in Lens::all().into_iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        let stats = stats(lens, buckets);
        let stages: Vec<String> = stats
            .stages
            .iter()
            .map(|s| {
                format!(
                    "{{\"name\":\"{}\",\"classes_in\":{},\"decisions\":{},\"folds\":{},\"classes_out\":{}}}",
                    s.name, s.classes_in, s.decision_points, s.fold_leaves, s.classes_out
                )
            })
            .collect();
        let _ = write!(
            out,
            "{{\"label\":\"{}\",\"draw_options\":{},\"stages\":[{}],\"showdown_classes\":{},\"decisions\":{},\"infosets\":{}}}",
            lens.label(),
            lens.draw_options(),
            stages.join(","),
            stats.showdown_classes,
            stats.total_decision_points,
            stats.infosets
        );
    }
    out.push_str("]}");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preflop_template_matches_hand_derivation() {
        let template = preflop_template();
        // Button first: f | c(option: k | r-chain) | r-chain; blind is
        // wager 1 of 4. Hand count: 14 outcomes, 7 folds, 7 continuing.
        assert_eq!(template.outcomes.len(), 14);
        assert_eq!(template.folds(), 7);
        assert_eq!(template.continuing(), 7);
        assert_eq!(template.prefixes.len(), 8);
        assert_eq!(template.first_actor, 0);
        // The blind counts toward the cap: at most three raises on top.
        let max_wagers = template.outcomes.iter().map(|o| o.wagers).max().unwrap();
        assert_eq!(max_wagers, WAGER_CAP);
        assert!(template.outcomes.iter().any(|o| o.seq == "ck"));
        assert!(template.outcomes.iter().any(|o| o.seq == "f"));
    }

    #[test]
    fn postdraw_template_matches_hand_derivation() {
        let template = postdraw_template();
        // kk | (k)b then f/c/r chain to cap: 17 outcomes, 8 folds.
        assert_eq!(template.outcomes.len(), 17);
        assert_eq!(template.folds(), 8);
        assert_eq!(template.continuing(), 9);
        assert_eq!(template.prefixes.len(), 10);
        assert_eq!(template.first_actor, 1);
        assert!(template.outcomes.iter().any(|o| o.seq == "kk"));
    }

    #[test]
    fn true_tree_totals_follow_the_dp() {
        // lines: 1 -> 7 continue x36 -> 252x36? No: verify against an
        // independent recomputation right here.
        let pre = preflop_template();
        let post = postdraw_template();
        let mut lines = 1u64;
        let mut expected = 0u64;
        for street in 0..4u32 {
            let (folds, continuing) = if street == 0 {
                (pre.folds() as u64, pre.continuing() as u64)
            } else {
                (post.folds() as u64, post.continuing() as u64)
            };
            expected += lines * folds;
            let live = lines * continuing;
            if street < 3 {
                lines = live * 36;
            } else {
                expected += live;
            }
        }
        assert_eq!(true_tree_leaves(), expected);
        // And the closed form: 7 + 7*36*8 + 7*36*9*36*8 + 7*36*9*36*9*36*17
        // (final street: 8 folds + 9 showdowns = 17 ends per line).
        let closed = 7 + 7 * 36 * 8 + 7 * 36 * 9 * 36 * 8 + 7 * 36 * 9 * 36 * 9 * 36 * 17u64;
        assert_eq!(true_tree_leaves(), closed);
    }

    #[test]
    fn lens_stats_shrink_monotonically() {
        let sizes: Vec<u64> = Lens::all()
            .into_iter()
            .map(|lens| stats(lens, 50).total_decision_points)
            .collect();
        for pair in sizes.windows(2) {
            assert!(
                pair[1] < pair[0],
                "each lens must be strictly smaller: {sizes:?}"
            );
        }
        // The short-memory lens must land in solvable territory: under a
        // million public decision points at 50 buckets.
        let short = stats(Lens::ShortMemory, 50);
        assert!(
            short.infosets < 5_000_000,
            "short-memory infosets: {}",
            short.infosets
        );
    }

    #[test]
    fn explorer_json_is_valid_json() {
        let doc = explorer_json(50);
        let parsed: serde_json::Value = serde_json::from_str(&doc).unwrap();
        assert_eq!(parsed["lenses"].as_array().unwrap().len(), 4);
        assert_eq!(parsed["true_leaves"].as_u64().unwrap(), 450_372_391);
    }
}

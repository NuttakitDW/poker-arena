//! Monte-Carlo pot-share equity, driven entirely by the game's
//! [`GameSpec`] showdown rules — one estimator covers all twenty variants.
//!
//! A rollout completes the hand from what this bot can see: unknown
//! opponent cards, the rest of the board, and this bot's own missing cards
//! are dealt uniformly from the unseen deck, then the showdown is scored
//! exactly as the engine would (hi/lo sides, qualifiers, scoops, even
//! split when nothing qualifies). The estimate is the average share of the
//! pot, in `[0, 1]`.
//!
//! Draw streets are rolled out **stand-pat** (nobody draws again). That
//! undervalues live draws; the draw policy compensates by sampling its own
//! replacements explicitly via [`equity_with_replacement`].

use poker_core::card::Card;
use poker_core::eval::best_with_usage;
use poker_core::game::spec::{DealSpec, GameSpec, ShowdownSpec};
use poker_core::rng::Rng64;

use crate::table::Table;

/// Final card counts a full hand reaches: per-player cards (hole + up) and
/// board cards, straight from the street list.
pub fn final_counts(spec: &GameSpec) -> (usize, usize) {
    let mut per_player = 0usize;
    let mut board = 0usize;
    for street in &spec.streets {
        match street.deal {
            DealSpec::HolePrivate(n) | DealSpec::HoleUp(n) => per_player += n as usize,
            DealSpec::Community(n) => board += n as usize,
            DealSpec::None | DealSpec::Draw { .. } => {}
        }
    }
    (per_player, board)
}

/// This bot's expected pot share against every live opponent, by `samples`
/// rollouts. `replace` optionally swaps this bot's hand for `keep` plus
/// `draw` fresh cards per rollout (the draw-decision evaluator).
fn rollout_share(
    spec: &GameSpec,
    table: &Table,
    replace: Option<(&[Card], usize)>,
    rng: &mut Rng64,
    samples: u32,
) -> f64 {
    let (per_player, board_final) = final_counts(spec);
    let dead = table.visible_cards();
    let unseen: Vec<Card> = (0..52)
        .filter_map(Card::from_index)
        .filter(|card| !dead.contains(card))
        .collect();

    let my_up = &table.upcards[table.seat];
    let opponents: Vec<usize> = (0..table.folded.len())
        .filter(|seat| *seat != table.seat && !table.folded[*seat])
        .collect();

    let mut total = 0.0;
    let mut scratch = unseen.clone();
    for _ in 0..samples.max(1) {
        rng.shuffle(&mut scratch);
        let mut next = 0usize;
        let mut take = |n: usize, from: &[Card]| -> Vec<Card> {
            let cards = from[next..next + n].to_vec();
            next += n;
            cards
        };

        // This bot's final hand: known cards plus whatever is still to come
        // (for a draw decision, the kept cards plus fresh replacements).
        let mine: Vec<Card> = match replace {
            Some((keep, draw)) => {
                let mut hand = keep.to_vec();
                hand.extend(take(draw, &scratch));
                hand
            }
            None => {
                let mut hand = table.hole.clone();
                hand.extend(my_up.iter().copied());
                let missing = per_player.saturating_sub(hand.len());
                hand.extend(take(missing, &scratch));
                hand
            }
        };

        // Board runout.
        let mut board = table.board.clone();
        board.extend(take(board_final.saturating_sub(board.len()), &scratch));

        // Live opponents: visible upcards plus hidden cards from the deck.
        let opp_hands: Vec<Vec<Card>> = opponents
            .iter()
            .map(|seat| {
                let mut hand = table.upcards[*seat].clone();
                let missing = per_player.saturating_sub(hand.len());
                hand.extend(take(missing, &scratch));
                hand
            })
            .collect();

        total += pot_share(&spec.showdown, &mine, &opp_hands, &board);
    }
    total / f64::from(samples.max(1))
}

/// Expected pot share with the hand as dealt.
pub fn equity(spec: &GameSpec, table: &Table, rng: &mut Rng64, samples: u32) -> f64 {
    rollout_share(spec, table, None, rng, samples)
}

/// Expected pot share after discarding down to `keep` and drawing
/// `draw` replacements.
pub fn equity_with_replacement(
    spec: &GameSpec,
    table: &Table,
    keep: &[Card],
    draw: usize,
    rng: &mut Rng64,
    samples: u32,
) -> f64 {
    rollout_share(spec, table, Some((keep, draw)), rng, samples)
}

/// Score one completed showdown for the first hand (`mine`) against the
/// rest, mirroring the engine's settlement: each side of the pot goes to
/// its best qualifying hand(s); one qualifying side scoops; if neither side
/// qualifies anywhere, the pot splits evenly among everyone shown.
/// Public because the trainer settles its abstract hands with the same rule.
pub fn pot_share(
    showdown: &ShowdownSpec,
    mine: &[Card],
    opponents: &[Vec<Card>],
    board: &[Card],
) -> f64 {
    let players = 1 + opponents.len();
    let hi = side_values(showdown, true, mine, opponents, board);
    match &showdown.lo {
        None => win_share(&hi).unwrap_or(0.0),
        Some(_) => {
            let lo = side_values(showdown, false, mine, opponents, board);
            match (win_share(&hi), win_share(&lo)) {
                (Some(hi_share), Some(lo_share)) => 0.5 * hi_share + 0.5 * lo_share,
                (Some(hi_share), None) => hi_share,
                (None, Some(lo_share)) => lo_share,
                (None, None) => 1.0 / players as f64,
            }
        }
    }
}

/// Everyone's value for one side of the pot (index 0 = this bot). `None` =
/// does not qualify.
fn side_values(
    showdown: &ShowdownSpec,
    hi: bool,
    mine: &[Card],
    opponents: &[Vec<Card>],
    board: &[Card],
) -> Vec<Option<poker_core::eval::HandValue>> {
    let side = if hi {
        showdown.hi
    } else {
        showdown.lo.expect("caller checked lo exists")
    };
    std::iter::once(mine)
        .chain(opponents.iter().map(Vec::as_slice))
        .map(|hand| best_with_usage(side.kind, side.usage, hand, board))
        .collect()
}

/// This bot's share of one side: `None` when nobody qualifies (side is not
/// awarded), else `1/winners` if this bot ties the best value, else 0.
fn win_share(values: &[Option<poker_core::eval::HandValue>]) -> Option<f64> {
    let best = values.iter().flatten().max()?;
    let winners = values.iter().flatten().filter(|v| *v == best).count();
    match values[0] {
        Some(v) if v == *best => Some(1.0 / winners as f64),
        _ => Some(0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use poker_core::card::parse_cards;
    use poker_core::game::spec::GameSpec;
    use poker_wire::game::Stakes;

    const STAKES: Stakes = Stakes::Blinds {
        small_blind: 50,
        big_blind: 100,
        ante: 0,
    };

    fn holdem_table(hole: &str, board: &str) -> Table {
        let mut table = Table::default();
        table.hand_start(0, 2);
        table.hole = parse_cards(hole).unwrap();
        table.board = parse_cards(board).unwrap();
        table.upcards = vec![Vec::new(), Vec::new()];
        table.folded = vec![false, false];
        table
    }

    #[test]
    fn final_counts_match_the_specs() {
        assert_eq!(final_counts(&GameSpec::holdem_nl(STAKES)), (2, 5));
        assert_eq!(final_counts(&GameSpec::omaha_pl(STAKES)), (4, 5));
        assert_eq!(final_counts(&GameSpec::stud_fl(STAKES)), (7, 0));
        assert_eq!(final_counts(&GameSpec::td27_fl(STAKES)), (5, 0));
        assert_eq!(final_counts(&GameSpec::drawmaha_fl(STAKES)), (5, 5));
    }

    #[test]
    fn aces_beat_a_random_hand_most_of_the_time() {
        let spec = GameSpec::holdem_nl(STAKES);
        let table = holdem_table("As Ad", "");
        let mut rng = Rng64::from_seed_stream(1, 0);
        let e = equity(&spec, &table, &mut rng, 400);
        assert!(e > 0.75, "AA preflop equity was {e}");
    }

    #[test]
    fn seven_deuce_loses_to_a_random_hand_more_often_than_not() {
        let spec = GameSpec::holdem_nl(STAKES);
        let table = holdem_table("7c 2d", "");
        let mut rng = Rng64::from_seed_stream(2, 0);
        let e = equity(&spec, &table, &mut rng, 400);
        assert!(e < 0.45, "72o preflop equity was {e}");
    }

    #[test]
    fn the_nuts_on_the_river_have_equity_one() {
        let spec = GameSpec::holdem_nl(STAKES);
        let table = holdem_table("As Ks", "Qs Js Ts 2d 3d");
        let mut rng = Rng64::from_seed_stream(3, 0);
        let e = equity(&spec, &table, &mut rng, 100);
        assert!(e > 0.99, "royal flush equity was {e}");
    }

    #[test]
    fn drawing_away_trips_beats_standing_pat_in_lowball() {
        // 2-7: pat trip kings is nearly hopeless; breaking them to draw
        // three at a ten-deuce start must be clearly better.
        let spec = GameSpec::td27_fl(STAKES);
        let mut table = Table::default();
        table.hand_start(0, 2);
        table.hole = parse_cards("Kc Kd Kh Ts 2c").unwrap();
        table.upcards = vec![Vec::new(), Vec::new()];
        table.folded = vec![false, false];

        let mut rng = Rng64::from_seed_stream(4, 0);
        let keep = parse_cards("Ts 2c").unwrap();
        let draw_three = equity_with_replacement(&spec, &table, &keep, 3, &mut rng, 300);
        let pat = equity(&spec, &table, &mut rng, 300);
        assert!(
            draw_three > pat + 0.1,
            "drawing three ({draw_three}) should crush pat trips ({pat})"
        );
    }

    #[test]
    fn split_pot_shares_sum_sensibly() {
        // Omaha8: a hand that scoops both sides gets share 1.
        let spec = GameSpec::omaha8_pl(STAKES);
        let showdown = &spec.showdown;
        let mine = parse_cards("As 2s 3d 4d").unwrap();
        let opp = vec![parse_cards("Kc Kd Qc Jd").unwrap()];
        let board = parse_cards("5s 6s 7s 8c 9c").unwrap();
        // Mine: straight flush hi + 6-low; opp: no low, straight at best.
        let share = pot_share(showdown, &mine, &opp, &board);
        assert!(share > 0.99, "scoop share was {share}");
    }
}

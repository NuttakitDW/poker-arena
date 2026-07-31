//! Baseline OFC bots: a deterministic floor, a uniform-random opponent, and
//! the greedy sparring partner matches are normally judged against.

use poker_core::card::Card;
use poker_core::eval::{HandClass, HandValue, deuce_to_seven_low, high, three_card_high};
use poker_core::ofc::score::{bottom_royalty, deuce_qualifies, middle_royalty, top_royalty};
use poker_core::ofc::{Board, MiddleKind, OfcAction, Placement, Row};
use poker_core::rng::Rng64;

use crate::bot::BotFault;
use crate::ofc::bot::{OfcActionRequest, OfcBot};

/// The one row order used everywhere in this module: the declaration order
/// of [`Row`]. Candidate enumeration and every per-row array are indexed by
/// it, which is what makes the greedy bot's tie-breaks reproducible.
const ROWS: [Row; 3] = [Row::Top, Row::Middle, Row::Bottom];

fn row_index(row: Row) -> usize {
    match row {
        Row::Top => 0,
        Row::Middle => 1,
        Row::Bottom => 2,
    }
}

/// The arena's deterministic answer to a faulted placement, and
/// [`OfcFiller`]'s entire strategy: sort the dealt cards ascending by
/// [`Card::index`], take the first `place`, drop each into bottom if it has
/// space, else middle, else top, and discard the rest.
///
/// The runner substitutes with this exact function, so "what a fault costs"
/// and "what the floor bot plays" can never drift apart.
pub(crate) fn filler_action(dealt: &[Card], place: u8, board: &Board) -> OfcAction {
    let mut sorted = dealt.to_vec();
    sorted.sort_unstable_by_key(|card| card.index());

    let mut free = [
        board.free(Row::Top),
        board.free(Row::Middle),
        board.free(Row::Bottom),
    ];
    debug_assert!(
        free.iter().sum::<usize>() >= place as usize,
        "the spec's card math guarantees room for every placement"
    );

    let discards = sorted.split_off(place as usize);
    let placements = sorted
        .into_iter()
        .map(|card| {
            let index = if free[2] > 0 {
                2
            } else if free[1] > 0 {
                1
            } else {
                0
            };
            free[index] = free[index].saturating_sub(1);
            Placement {
                card,
                row: ROWS[index],
            }
        })
        .collect();

    OfcAction {
        placements,
        discards,
    }
}

/// Plays the fault-substitution rule as a strategy: lowest cards first, low
/// rows first. The deterministic floor every other bot must beat.
pub struct OfcFiller {
    name: String,
}

impl OfcFiller {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

impl OfcBot for OfcFiller {
    fn name(&self) -> &str {
        &self.name
    }

    fn place(&mut self, req: &OfcActionRequest<'_>) -> Result<OfcAction, BotFault> {
        Ok(filler_action(req.dealt, req.place, &req.boards[req.seat]))
    }
}

/// Places a uniform-random legal subset into uniform-random rows with free
/// capacity: the dealt cards are shuffled and split into the placed set and
/// the discards, then each placed card draws a row among those still open.
/// Deterministic for a given seed.
pub struct OfcRandom {
    name: String,
    rng: Rng64,
}

impl OfcRandom {
    pub fn new(name: impl Into<String>, seed: u64) -> Self {
        Self {
            name: name.into(),
            rng: Rng64::from_seed_stream(seed, 0),
        }
    }
}

impl OfcBot for OfcRandom {
    fn name(&self) -> &str {
        &self.name
    }

    fn place(&mut self, req: &OfcActionRequest<'_>) -> Result<OfcAction, BotFault> {
        let mut cards = req.dealt.to_vec();
        self.rng.shuffle(&mut cards);
        let discards = cards.split_off(req.place as usize);

        let board = &req.boards[req.seat];
        let mut free = [
            board.free(Row::Top),
            board.free(Row::Middle),
            board.free(Row::Bottom),
        ];
        let mut placements = Vec::with_capacity(cards.len());
        for card in cards {
            let open: Vec<usize> = (0..3).filter(|index| free[*index] > 0).collect();
            debug_assert!(
                !open.is_empty(),
                "the spec's card math guarantees room for every placement"
            );
            let pick = open[self.rng.below(open.len() as u64) as usize];
            free[pick] -= 1;
            placements.push(Placement {
                card,
                row: ROWS[pick],
            });
        }

        Ok(OfcAction {
            placements,
            discards,
        })
    }
}

/// A one-ply greedy bot that actively avoids fouling. Deterministic: it
/// holds no RNG, and equal-scoring candidates resolve to the first one in
/// the canonical enumeration order (see [`candidates`]).
///
/// The middle row's evaluator is a constructor parameter rather than
/// something read off a request, because a bot knows which game it entered:
/// one instance plays one variant for a whole match, and the placement
/// request deliberately describes only the decision, not the rules.
pub struct OfcGreedy {
    name: String,
    middle: MiddleKind,
}

impl OfcGreedy {
    pub fn new(name: impl Into<String>, middle: MiddleKind) -> Self {
        Self {
            name: name.into(),
            middle,
        }
    }
}

impl OfcBot for OfcGreedy {
    fn name(&self) -> &str {
        &self.name
    }

    fn place(&mut self, req: &OfcActionRequest<'_>) -> Result<OfcAction, BotFault> {
        let board = &req.boards[req.seat];
        // A whole board in one decision is the fantasyland turn, where exact
        // greedy is both affordable and (for high middles) foul-proof.
        if req.place as usize == Board::CAPACITY {
            return Ok(full_board(self.middle, req.dealt));
        }
        Ok(best_candidate(
            self.middle,
            req.dealt,
            req.place as usize,
            board,
        ))
    }
}

// ---- exact greedy for a whole board ----

/// Fill a whole board at once: bottom takes the best `high` five-subset of
/// the dealt cards, middle the best five-subset of what remains (`high`, or
/// `deuce_to_seven_low` for a 2-7 middle), top the best `three_card_high`
/// three-subset of what remains after that, and the leftovers are discarded.
///
/// For a high middle this provably cannot foul: middle is chosen from a
/// subset of the pool bottom was chosen from, so `high(middle) <=
/// high(bottom)`; and the top three plus any two other cards form a
/// five-subset of that same remainder whose `high` value is at least
/// `three_card_high(top)` (the top encoding is the high encoding with its
/// unused nibbles zero), so `three_card_high(top) <= high(middle)`. The same
/// argument gives `three_card_high(top) <= high(bottom)`, which is the only
/// ordering a 2-7 board must keep — there its middle can still fail the
/// qualifier, which no arrangement of a given thirteen cards may be able to
/// avoid.
fn full_board(middle: MiddleKind, dealt: &[Card]) -> OfcAction {
    let mut pool = dealt.to_vec();
    pool.sort_unstable_by_key(|card| card.index());

    let (bottom, rest) = best_subset(&pool, Board::BOTTOM_CAPACITY, high);
    let (mid, rest) = best_subset(&rest, Board::MIDDLE_CAPACITY, |cards| match middle {
        MiddleKind::High => high(cards),
        MiddleKind::DeuceToSeven => deuce_to_seven_low(cards),
    });
    let (top, discards) = best_subset(&rest, Board::TOP_CAPACITY, three_card_high);

    let mut placements = Vec::with_capacity(Board::CAPACITY);
    for (row, cards) in [
        (Row::Top, &top),
        (Row::Middle, &mid),
        (Row::Bottom, &bottom),
    ] {
        placements.extend(cards.iter().map(|card| Placement { card: *card, row }));
    }
    OfcAction {
        placements,
        discards,
    }
}

/// The `k`-subset of `pool` maximizing `score`, plus everything it left
/// behind. Subsets are enumerated in lexicographic index order and the first
/// maximum wins, so the choice is deterministic.
fn best_subset(
    pool: &[Card],
    k: usize,
    score: impl Fn(&[Card]) -> HandValue,
) -> (Vec<Card>, Vec<Card>) {
    let mut best: Option<(HandValue, Vec<usize>)> = None;
    let mut cards = Vec::with_capacity(k);
    for subset in combinations(pool.len(), k) {
        cards.clear();
        cards.extend(subset.iter().map(|index| pool[*index]));
        let value = score(&cards);
        if best.as_ref().is_none_or(|(top, _)| value > *top) {
            best = Some((value, subset));
        }
    }

    // Every caller passes a pool of at least `k` cards (the fantasyland deal
    // is 13..=17 and the three rows take 13 of it).
    let chosen = best.expect("a k-subset exists").1;
    let taken: Vec<Card> = chosen.iter().map(|index| pool[*index]).collect();
    let rest: Vec<Card> = (0..pool.len())
        .filter(|index| !chosen.contains(index))
        .map(|index| pool[index])
        .collect();
    (taken, rest)
}

/// Every `k`-subset of `0..n` as ascending index vectors, in lexicographic
/// order.
fn combinations(n: usize, k: usize) -> Vec<Vec<usize>> {
    if k > n {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut pick: Vec<usize> = (0..k).collect();
    loop {
        out.push(pick.clone());
        // Advance the rightmost index that still has room, then repack
        // everything to its right immediately after it.
        let Some(position) = (0..k).rev().find(|i| pick[*i] < n - k + i) else {
            return out;
        };
        pick[position] += 1;
        for i in (position + 1)..k {
            pick[i] = pick[i - 1] + 1;
        }
    }
}

// ---- one-ply greedy for an incremental turn ----

/// Weight per row for the rank-shaping term, indexed like [`ROWS`]: big
/// cards belong low, so bottom pays most and top nothing.
const RANK_WEIGHT: [f64; 3] = [0.0, 16.0, 32.0];

/// A foul that has already happened costs more than any board can be worth.
const CERTAIN_FOUL: f64 = 1e12;
/// A made-so-far ordering violation that a later card could still repair.
///
/// This has to outrank raw row strength, not merely tax it: one class step
/// in the frozen high encoding is `1 << 20` ≈ 1.05e6, so any smaller penalty
/// is bought off by a single class jump — filling the middle to two pair
/// over a one-pair bottom, say — and the board fouls three cards later.
/// A weighted board sum cannot exceed `3.15 × 2^24` ≈ 5.3e7, so `1e8`
/// dominates every strength swing there is, which makes "keep the rows in
/// order" a hard preference and row strength the tie-break underneath it.
const SOFT_INVERSION: f64 = 1e8;
/// Royalties are worth far more than raw row strength, but never worth a
/// foul.
const ROYALTY: f64 = 1e4;

/// The best-scoring legal option for this turn.
fn best_candidate(middle: MiddleKind, dealt: &[Card], place: usize, board: &Board) -> OfcAction {
    let mut best: Option<(f64, OfcAction)> = None;
    for action in candidates(dealt, place, board) {
        let value = score(middle, board, &action);
        if best.as_ref().is_none_or(|(top, _)| value > *top) {
            best = Some((value, action));
        }
    }
    best.expect("the spec's card math guarantees a legal option")
        .1
}

/// Every legal option for this turn, in the canonical order that decides
/// ties: the dealt cards sorted ascending by [`Card::index`], their
/// `place`-subsets in lexicographic order, and for each subset the row
/// assignments as a base-3 odometer over [`ROWS`] whose *last* placed card
/// varies fastest. The first maximum wins, so the same decision always
/// produces the same action.
fn candidates(dealt: &[Card], place: usize, board: &Board) -> Vec<OfcAction> {
    let mut pool = dealt.to_vec();
    pool.sort_unstable_by_key(|card| card.index());
    let free = [
        board.free(Row::Top),
        board.free(Row::Middle),
        board.free(Row::Bottom),
    ];

    let mut out = Vec::new();
    for subset in combinations(pool.len(), place) {
        let discards: Vec<Card> = (0..pool.len())
            .filter(|index| !subset.contains(index))
            .map(|index| pool[index])
            .collect();

        let mut assignment = vec![0usize; place];
        loop {
            let mut used = [0usize; 3];
            for row in &assignment {
                used[*row] += 1;
            }
            if (0..3).all(|row| used[row] <= free[row]) {
                out.push(OfcAction {
                    placements: subset
                        .iter()
                        .zip(&assignment)
                        .map(|(index, row)| Placement {
                            card: pool[*index],
                            row: ROWS[*row],
                        })
                        .collect(),
                    discards: discards.clone(),
                });
            }

            let Some(digit) = (0..place).rev().find(|i| assignment[*i] < 2) else {
                break;
            };
            assignment[digit] += 1;
            for slot in assignment.iter_mut().skip(digit + 1) {
                *slot = 0;
            }
        }
    }
    out
}

/// Score the position `action` would leave behind. Row strength is the
/// dominant term only once fouling is out of the way: a foul that has
/// already happened is worth `-1e12`, an ordering a later card could still
/// repair `-1e6`, a royalty locked in by completing a row `+1e4`, and the
/// rank-shaping tail breaks ties toward big cards in low rows.
fn score(middle: MiddleKind, before: &Board, action: &OfcAction) -> f64 {
    let mut after = before.clone();
    for placement in &action.placements {
        let pushed = after.push(*placement);
        debug_assert!(pushed, "candidates only offers rows with free capacity");
    }

    let mut total = made(middle, Row::Bottom, &after) * 1.0
        + made(middle, Row::Middle, &after) * 1.05
        + made(middle, Row::Top, &after) * 1.1;

    let (certain, soft) = foul_terms(middle, &after);
    total -= CERTAIN_FOUL * certain + SOFT_INVERSION * soft;

    for row in ROWS {
        let capacity = Board::capacity(row);
        if before.row(row).len() < capacity && after.row(row).len() == capacity {
            let cards = after.row(row);
            total += ROYALTY
                * f64::from(match row {
                    Row::Top => top_royalty(cards),
                    Row::Middle => middle_royalty(middle, cards),
                    Row::Bottom => bottom_royalty(cards),
                });
        }
    }

    for placement in &action.placements {
        total += f64::from(placement.card.rank().index()) * RANK_WEIGHT[row_index(placement.row)];
    }
    total
}

/// How good a row is *so far*: a full row through its real evaluator, a
/// partial one through the pairs-only partial encoding below.
fn made(middle: MiddleKind, row: Row, board: &Board) -> f64 {
    f64::from(row_value(middle, row, board.row(row)).0)
}

/// A row's value, on whatever scale is meaningful for its state. Full rows
/// are directly comparable across boards; partial rows only ever meet other
/// values from this same function, and only within one candidate's score.
fn row_value(middle: MiddleKind, row: Row, cards: &[Card]) -> HandValue {
    if cards.len() < Board::capacity(row) {
        return partial_high(cards);
    }
    match row {
        Row::Top => three_card_high(cards),
        Row::Middle if middle == MiddleKind::DeuceToSeven => deuce_to_seven_low(cards),
        Row::Middle | Row::Bottom => high(cards),
    }
}

/// The high encoding of an incomplete row, reading pairing only: a partial
/// row has no straights and no flushes to speak of, and treating a
/// three-card "flush draw" as a flush would wildly overrate it. Rank
/// multiplicities give the class; the group ranks fill the tiebreak nibbles
/// most significant first, exactly as the five-card encoding orders them,
/// and the unused nibbles stay zero so a row's value only ever grows as it
/// fills.
fn partial_high(cards: &[Card]) -> HandValue {
    let mut counts = [0u8; 13];
    for card in cards {
        counts[card.rank().index() as usize] += 1;
    }
    // Rank-descending first, then a stable sort by count: groups come out
    // ordered by count desc, rank desc — the high encoding's order.
    let mut groups: Vec<(u8, u8)> = (0..13u8)
        .rev()
        .filter(|rank| counts[*rank as usize] > 0)
        .map(|rank| (counts[rank as usize], rank))
        .collect();
    groups.sort_by_key(|group| std::cmp::Reverse(group.0));

    let top_count = groups.first().map_or(0, |group| group.0);
    let paired_twice = groups.len() >= 2 && groups[1].0 >= 2;
    let class = if top_count >= 4 {
        HandClass::Quads
    } else if top_count == 3 {
        HandClass::Trips
    } else if top_count == 2 && paired_twice {
        HandClass::TwoPair
    } else if top_count == 2 {
        HandClass::OnePair
    } else {
        HandClass::HighCard
    };

    let mut bits = (class as u32) << 20;
    for (slot, (_, rank)) in groups.iter().take(5).enumerate() {
        bits |= u32::from(*rank) << (16 - 4 * slot);
    }
    HandValue(bits)
}

/// Can this partial 2-7 middle still become a ten-low? Only the two
/// irreversible ways to lose the qualifier are checked — a pair, or a card
/// above Ten; a partial row's straight and flush risks depend on cards not
/// dealt yet and are left to the full-row test.
fn can_still_qualify(middle: &[Card]) -> bool {
    let mut seen = [false; 13];
    for card in middle {
        let rank = card.rank().index() as usize;
        if seen[rank] || rank > poker_core::card::Rank::Ten.index() as usize {
            return false;
        }
        seen[rank] = true;
    }
    true
}

/// Ordering violations on the board, split into the ones that are already
/// final (both rows full, or a full 2-7 middle that misses the qualifier)
/// and the ones a later card could still repair.
fn foul_terms(middle: MiddleKind, board: &Board) -> (f64, f64) {
    let pairs: &[(Row, Row)] = match middle {
        // Top ≤ middle ≤ bottom.
        MiddleKind::High => &[(Row::Top, Row::Middle), (Row::Middle, Row::Bottom)],
        // The 2-7 middle has no ordering relationship with its neighbours.
        MiddleKind::DeuceToSeven => &[(Row::Top, Row::Bottom)],
    };

    let mut certain = 0.0;
    let mut soft = 0.0;
    for (upper, lower) in pairs {
        if row_value(middle, *upper, board.row(*upper))
            > row_value(middle, *lower, board.row(*lower))
        {
            let both_full = board.row(*upper).len() == Board::capacity(*upper)
                && board.row(*lower).len() == Board::capacity(*lower);
            if both_full {
                certain += 1.0;
            } else {
                soft += 1.0;
            }
        }
    }
    if middle == MiddleKind::DeuceToSeven {
        if board.middle.len() == Board::MIDDLE_CAPACITY {
            if !deuce_qualifies(&board.middle) {
                certain += 1.0;
            }
        } else if !can_still_qualify(&board.middle) {
            // The qualifier is the 2-7 middle's whole ordering requirement,
            // so a partial middle that has already lost it is that
            // variant's soft inversion — without this the bot gets no
            // signal at all until the row is full and the foul is locked in.
            soft += 1.0;
        }
    }
    (certain, soft)
}

#[cfg(test)]
mod tests {
    use super::*;
    use poker_core::card::parse_cards;

    /// Owned backing data for an [`OfcActionRequest`] (which borrows).
    struct Scenario {
        dealt: Vec<Card>,
        place: u8,
        boards: Vec<Board>,
        fantasyland: Vec<Option<u8>>,
    }

    impl Scenario {
        fn new(dealt: &str, place: u8, board: Board) -> Self {
            Self {
                dealt: parse_cards(dealt).unwrap(),
                place,
                boards: vec![board, Board::new()],
                fantasyland: vec![None, None],
            }
        }

        fn request(&self) -> OfcActionRequest<'_> {
            OfcActionRequest {
                hand_no: 1,
                seat: 0,
                dealt: &self.dealt,
                place: self.place,
                discard: self.dealt.len() as u8 - self.place,
                boards: &self.boards,
                fantasyland: &self.fantasyland,
            }
        }
    }

    fn board(top: &str, middle: &str, bottom: &str) -> Board {
        Board {
            top: parse_cards(top).unwrap(),
            middle: parse_cards(middle).unwrap(),
            bottom: parse_cards(bottom).unwrap(),
        }
    }

    fn row_of(action: &OfcAction, card: &str) -> Row {
        let card = parse_cards(card).unwrap()[0];
        action
            .placements
            .iter()
            .find(|placement| placement.card == card)
            .unwrap_or_else(|| panic!("{card} was not placed"))
            .row
    }

    // ---- filler ----

    #[test]
    fn filler_takes_the_lowest_cards_and_fills_from_the_bottom() {
        let scenario = Scenario::new("As 2c Kd 3h", 2, Board::new());
        let mut bot = OfcFiller::new("filler");
        let action = bot.place(&scenario.request()).unwrap();

        assert_eq!(action.placements.len(), 2);
        assert_eq!(action.discards.len(), 2);
        assert_eq!(row_of(&action, "2c"), Row::Bottom);
        assert_eq!(row_of(&action, "3h"), Row::Bottom);
        assert_eq!(action.discards, parse_cards("Kd As").unwrap());
    }

    #[test]
    fn filler_spills_into_middle_then_top_as_rows_fill() {
        let full_bottom = board("", "2d 3d 4d 5d", "2c 3c 4c 5c 7c");
        let scenario = Scenario::new("As Kd", 2, full_bottom);
        let mut bot = OfcFiller::new("filler");
        let action = bot.place(&scenario.request()).unwrap();

        assert_eq!(row_of(&action, "Kd"), Row::Middle, "middle had one slot");
        assert_eq!(row_of(&action, "As"), Row::Top);
    }

    // ---- random ----

    #[test]
    fn random_is_deterministic_for_the_same_seed_and_stays_legal() {
        let scenario = Scenario::new("As 2c Kd 3h 9s", 2, board("Qs", "2d 3d", "2c 3c 4c"));
        let run = || {
            let mut bot = OfcRandom::new("random", 42);
            (0..50)
                .map(|_| bot.place(&scenario.request()).unwrap())
                .collect::<Vec<_>>()
        };
        let actions = run();
        assert_eq!(actions, run());

        for action in &actions {
            assert_eq!(action.placements.len(), 2);
            assert_eq!(action.discards.len(), 3);
            let mut seen: Vec<Card> = action.placements.iter().map(|p| p.card).collect();
            seen.extend(&action.discards);
            seen.sort_unstable_by_key(|card| card.index());
            let mut expected = scenario.dealt.clone();
            expected.sort_unstable_by_key(|card| card.index());
            assert_eq!(seen, expected, "every dealt card must be accounted for");
        }
    }

    #[test]
    fn random_never_places_into_a_full_row() {
        let scenario = Scenario::new("As Kd", 2, board("Qs Qd Qc", "2d 3d 4d 5d 7d", ""));
        let mut bot = OfcRandom::new("random", 7);
        for _ in 0..100 {
            let action = bot.place(&scenario.request()).unwrap();
            assert!(
                action.placements.iter().all(|p| p.row == Row::Bottom),
                "only the bottom row had capacity"
            );
        }
    }

    // ---- greedy ----

    #[test]
    fn partial_encoding_grows_monotonically_and_ranks_pairs_over_high_cards() {
        let ace_high = partial_high(&parse_cards("As Kd").unwrap());
        let pair = partial_high(&parse_cards("2s 2d").unwrap());
        assert!(pair > ace_high, "a pair outranks any unpaired partial");

        let one = partial_high(&parse_cards("As").unwrap());
        let two = partial_high(&parse_cards("As 3d").unwrap());
        assert!(two > one, "zero-filled nibbles only ever grow");

        assert_eq!(
            partial_high(&parse_cards("As Ks Qs").unwrap()),
            three_card_high(&parse_cards("As Ks Qs").unwrap()),
            "a three-card row's partial and full encodings agree"
        );
    }

    #[test]
    fn a_partial_two_seven_middle_dies_on_a_pair_or_a_high_card() {
        assert!(can_still_qualify(&parse_cards("2c 5d 7h").unwrap()));
        assert!(can_still_qualify(&parse_cards("Tc 9d").unwrap()));
        assert!(can_still_qualify(&[]));
        assert!(!can_still_qualify(&parse_cards("2c 2d").unwrap()));
        assert!(!can_still_qualify(&parse_cards("2c Jd").unwrap()));
    }

    #[test]
    fn greedy_keeps_a_two_seven_middle_alive() {
        // The jack can never be part of a ten-low, and the deuce would pair
        // the middle's existing deuce; only the eight leaves the row able to
        // qualify, and the middle is the one row with space.
        let two_seven = board("Ah Kh Qh", "2c 5d 7h", "Ac Ad As Kc Kd");
        let scenario = Scenario::new("Jd 2s 8h", 1, two_seven);
        let mut bot = OfcGreedy::new("greedy", MiddleKind::DeuceToSeven);
        let action = bot.place(&scenario.request()).unwrap();

        assert_eq!(action.placements.len(), 1);
        assert_eq!(action.placements[0].row, Row::Middle);
        assert_eq!(action.placements[0].card, parse_cards("8h").unwrap()[0]);
    }

    #[test]
    fn greedy_refuses_a_placement_that_fouls_on_the_spot() {
        // Bottom is a full pair of deuces; the middle needs its last card.
        // The ten pairs the middle into tens-up over the bottom's deuces — a
        // foul that is already final — while the queen keeps the board legal
        // even though it leaves the weaker middle.
        let nearly_done = board("3s 4s 5s", "7d 8d 9d Ts", "2c 2d 4c 5c 7c");
        let scenario = Scenario::new("Th Qh", 1, nearly_done);
        let mut bot = OfcGreedy::new("greedy", MiddleKind::High);
        let action = bot.place(&scenario.request()).unwrap();

        assert_eq!(action.placements.len(), 1);
        assert_eq!(action.placements[0].row, Row::Middle, "only row with room");
        assert_eq!(
            action.placements[0].card,
            parse_cards("Qh").unwrap()[0],
            "the ten fouls the board immediately"
        );
    }

    #[test]
    fn greedy_pushes_big_cards_toward_the_bottom() {
        let scenario = Scenario::new("As 2c", 2, Board::new());
        let mut bot = OfcGreedy::new("greedy", MiddleKind::High);
        let action = bot.place(&scenario.request()).unwrap();
        assert_eq!(row_of(&action, "As"), Row::Bottom);
    }

    #[test]
    fn greedy_is_deterministic() {
        let scenario = Scenario::new("As 2c Kd", 2, board("", "2d", "3c 4c"));
        let run = || {
            let mut bot = OfcGreedy::new("greedy", MiddleKind::High);
            (0..20)
                .map(|_| bot.place(&scenario.request()).unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn greedy_fills_a_fantasyland_board_without_fouling() {
        // A fourteen-card fantasyland deal: exact greedy must return a
        // complete, legal, non-fouling board and one discard.
        let scenario = Scenario::new(
            "As Ks Qs Js Ts 9h 9d 9c 8h 7h 6h 5h 4h 3h",
            Board::CAPACITY as u8,
            Board::new(),
        );
        let mut bot = OfcGreedy::new("greedy", MiddleKind::High);
        let action = bot.place(&scenario.request()).unwrap();

        assert_eq!(action.placements.len(), Board::CAPACITY);
        assert_eq!(action.discards.len(), 1);

        let mut built = Board::new();
        for placement in &action.placements {
            assert!(built.push(*placement));
        }
        assert!(built.is_complete());
        let evaluated = poker_core::ofc::score::evaluate(&poker_core::ofc::OFC_PINEAPPLE, &built);
        assert!(!evaluated.fouled, "exact greedy never fouls a high board");
    }

    #[test]
    fn combinations_are_lexicographic_and_complete() {
        assert_eq!(
            combinations(4, 2),
            vec![
                vec![0, 1],
                vec![0, 2],
                vec![0, 3],
                vec![1, 2],
                vec![1, 3],
                vec![2, 3]
            ]
        );
        assert_eq!(combinations(3, 3), vec![vec![0, 1, 2]]);
        assert!(combinations(2, 3).is_empty());
    }
}

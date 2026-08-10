//! The abstract heads-up game the trainer solves.
//!
//! Real cards, real showdown settlement, abstract *actions*: betting uses
//! the lossy menu from [`crate::abstraction`] (fixed limit: the one legal
//! size, engine-capped raises; big-bet: pot or all-in, two wagers per
//! street), and a draw decision is only a discard *count* — the cards are
//! chosen by a deterministic worst-first rule so count is the only thing
//! CFR has to learn. Fidelity notes (deliberate simplifications, all on
//! the abstraction side, never in settlement):
//!
//! - Stud act order on later streets uses a partial-upcard comparator with
//!   seat-order tie-breaks (the arena breaks bring-in ties by suit; post
//!   bring-in ordering ties are rare and worth no tree complexity).
//! - The abstract path records both players' draw counts (public in the
//!   real game) and one letter per betting action; check and call share a
//!   letter because the menu merges them.

use poker_core::card::Card;
use poker_core::deck::Deck;
use poker_core::eval::EvalKind;
use poker_core::game::spec::{
    BetRoundSpec, DealSpec, FirstToAct, ForcedBets, GameSpec, StreetSpec,
};
use poker_core::rng::Rng64;
use poker_wire::game::BettingKind;

use crate::equity::pot_share;

/// One abstract action.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Abs {
    Fold,
    /// Check when nothing is outstanding, call otherwise: one passive slot.
    CheckCall,
    /// The street's fixed-limit size (open or raise).
    BetFixed,
    /// A pot-sized bet or raise (big-bet games).
    BetPot,
    /// Shove (big-bet games).
    AllIn,
    /// Post the stud bring-in.
    BringIn,
    /// Discard exactly `n` cards this draw turn.
    Draw(u8),
}

impl Abs {
    /// The path letter appended when this action is taken.
    pub fn letter(self) -> char {
        match self {
            Abs::Fold => 'f',
            Abs::CheckCall => 'c',
            Abs::BetFixed => 'b',
            Abs::BetPot => 'p',
            Abs::AllIn => 'a',
            Abs::BringIn => 'i',
            Abs::Draw(_) => 'g',
        }
    }
}

/// Decision kinds, the first component of an infoset key.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Wager,
    Draw,
    BringIn,
}

impl Kind {
    pub fn letter(self) -> char {
        match self {
            Kind::Wager => 'w',
            Kind::Draw => 'd',
            Kind::BringIn => 'i',
        }
    }
}

/// Static per-game context for simulation.
pub struct Sim {
    pub spec: GameSpec,
    pub stack: u64,
    /// Max wagers per street: the engine cap for fixed limit, 2 for the
    /// big-bet abstraction.
    pub wager_cap: u32,
    small_blind: u64,
    big_blind: u64,
}

impl Sim {
    pub fn new(spec: GameSpec, stack: u64) -> Sim {
        let wager_cap = match spec.betting {
            BettingKind::FixedLimit { raise_cap } => u32::from(raise_cap.unwrap_or(4)),
            BettingKind::NoLimit | BettingKind::PotLimit => 2,
        };
        let (small_blind, big_blind) = spec.stakes.blinds();
        Sim {
            spec,
            stack,
            wager_cap,
            small_blind,
            big_blind,
        }
    }

    fn street(&self, index: usize) -> &StreetSpec {
        &self.spec.streets[index]
    }

    fn tier_size(&self, betting: &BetRoundSpec) -> u64 {
        self.spec.tier_size(betting.tier)
    }
}

/// Where a hand stands between decisions.
#[derive(Clone, Debug, PartialEq)]
enum Phase {
    /// Perform street `street`'s deal next.
    Deal,
    /// Draw decisions this street, in seat order; front is next to act.
    Draws(Vec<usize>),
    /// A betting round in progress.
    Betting,
    /// Hand over (fold or showdown).
    Over,
}

/// A full game state. Cloned when the traverser branches.
#[derive(Clone, Debug)]
pub struct State {
    /// The whole shuffled deck for this hand; `cursor` marks what's used.
    deck: Vec<Card>,
    cursor: usize,
    street: usize,
    phase: Phase,
    pub hole: [Vec<Card>; 2],
    pub up: [Vec<Card>; 2],
    pub board: Vec<Card>,
    /// Whole-hand commitment per seat (blinds/antes included).
    commit: [u64; 2],
    /// Current-street commitment per seat.
    sc: [u64; 2],
    wagers: u32,
    to_act: usize,
    /// Seats that have acted since the last wager this street.
    acted: [bool; 2],
    /// This street's abstract path (draw counts + betting letters).
    pub path: String,
    folded: Option<usize>,
    /// Betting closed for the rest of the hand (someone is all-in).
    all_in_locked: bool,
}

impl State {
    pub fn street(&self) -> usize {
        self.street
    }

    pub fn is_terminal(&self) -> bool {
        self.phase == Phase::Over
    }

    /// Seat to act at the current decision point.
    pub fn actor(&self) -> usize {
        match &self.phase {
            Phase::Draws(pending) => pending[0],
            _ => self.to_act,
        }
    }

    pub fn kind(&self, sim: &Sim) -> Kind {
        match &self.phase {
            Phase::Draws(_) => Kind::Draw,
            _ => {
                if self.is_bring_in_decision(sim) {
                    Kind::BringIn
                } else {
                    Kind::Wager
                }
            }
        }
    }

    /// The pending forced-open decision on a stud bring-in street: nobody
    /// has wagered yet and the street's round opens by upcards.
    fn is_bring_in_decision(&self, sim: &Sim) -> bool {
        matches!(sim.spec.forced_bets, ForcedBets::BringIn { .. })
            && self.wagers == 0
            && first_betting_street(&sim.spec) == Some(self.street)
    }

    /// Terminal utility for seat 0, in chips.
    pub fn utility(&self, sim: &Sim) -> f64 {
        debug_assert!(self.is_terminal());
        let pot = (self.commit[0] + self.commit[1]) as f64;
        match self.folded {
            Some(0) => -(self.commit[0] as f64),
            Some(1) => self.commit[1] as f64,
            Some(_) => unreachable!("two seats"),
            None => {
                let mine: Vec<Card> = self.hole[0].iter().chain(&self.up[0]).copied().collect();
                let theirs: Vec<Card> = self.hole[1].iter().chain(&self.up[1]).copied().collect();
                let share = pot_share(&sim.spec.showdown, &mine, &[theirs], &self.board);
                share * pot - self.commit[0] as f64
            }
        }
    }

    /// The abstract actions available at the current decision point.
    pub fn actions(&self, sim: &Sim) -> Vec<Abs> {
        match &self.phase {
            Phase::Draws(_) => {
                let DealSpec::Draw { max } = sim.street(self.street).deal else {
                    unreachable!("draw phase only on draw streets");
                };
                let hand = self.hole[self.actor()].len().min(usize::from(max)) as u8;
                (0..=hand).map(Abs::Draw).collect()
            }
            Phase::Betting => {
                if self.kind(sim) == Kind::BringIn {
                    return vec![Abs::BringIn, Abs::BetFixed];
                }
                let actor = self.actor();
                let outstanding = self.sc[1 - actor] > self.sc[actor];
                let mut acts = Vec::with_capacity(4);
                if outstanding {
                    acts.push(Abs::Fold);
                }
                acts.push(Abs::CheckCall);
                let can_wager = !self.all_in_locked
                    && self.wagers < sim.wager_cap
                    && self.commit[actor] + self.call_amount(actor) < sim.stack;
                if can_wager {
                    match sim.spec.betting {
                        BettingKind::FixedLimit { .. } => acts.push(Abs::BetFixed),
                        BettingKind::NoLimit | BettingKind::PotLimit => {
                            acts.push(Abs::BetPot);
                            acts.push(Abs::AllIn);
                        }
                    }
                }
                acts
            }
            _ => unreachable!("no decisions outside draws/betting"),
        }
    }

    fn call_amount(&self, actor: usize) -> u64 {
        self.sc[1 - actor].saturating_sub(self.sc[actor])
    }

    /// Commit `actor` up to `target` street chips, clamped by stack.
    fn commit_to(&mut self, sim: &Sim, actor: usize, target: u64) {
        let target = target.min(self.sc[actor] + (sim.stack - self.commit[actor]));
        let add = target.saturating_sub(self.sc[actor]);
        self.sc[actor] += add;
        self.commit[actor] += add;
        if self.commit[actor] >= sim.stack {
            self.all_in_locked = true;
        }
    }

    /// Apply `action` and advance to the next decision point or terminal.
    pub fn apply(&self, sim: &Sim, action: Abs) -> State {
        let mut next = self.clone();
        next.apply_in_place(sim, action);
        next.advance(sim);
        next
    }

    fn apply_in_place(&mut self, sim: &Sim, action: Abs) {
        match action {
            Abs::Draw(n) => {
                let Phase::Draws(pending) = &mut self.phase else {
                    unreachable!("draw action outside draw phase");
                };
                let seat = pending.remove(0);
                let discards = ranked_discards(sim.spec.showdown.hi.kind, &self.hole[seat]);
                for card in discards.iter().take(usize::from(n)) {
                    let position = self.hole[seat]
                        .iter()
                        .position(|held| held == card)
                        .expect("ranked discards come from the hand");
                    self.hole[seat].remove(position);
                }
                for _ in 0..n {
                    self.hole[seat].push(self.deck[self.cursor]);
                    self.cursor += 1;
                }
                self.path.push('g');
                self.path.push(char::from(b'0' + n));
            }
            Abs::Fold => {
                self.folded = Some(self.actor());
                self.phase = Phase::Over;
            }
            Abs::CheckCall => {
                let actor = self.actor();
                let target = self.sc[1 - actor];
                self.commit_to(sim, actor, target);
                self.acted[actor] = true;
                self.path.push('c');
                self.to_act = 1 - actor;
            }
            Abs::BringIn => {
                let actor = self.actor();
                let ForcedBets::BringIn { bring_in, .. } = sim.spec.forced_bets else {
                    unreachable!("bring-in only in stud games");
                };
                self.commit_to(sim, actor, bring_in);
                self.wagers = 1;
                self.acted = [false, false];
                self.acted[actor] = true;
                self.path.push('i');
                self.to_act = 1 - actor;
            }
            Abs::BetFixed | Abs::BetPot | Abs::AllIn => {
                let actor = self.actor();
                let target = self.wager_target(sim, actor, action);
                self.commit_to(sim, actor, target);
                self.wagers += 1;
                self.acted = [false, false];
                self.acted[actor] = true;
                self.path.push(action.letter());
                self.to_act = 1 - actor;
            }
        }
    }

    /// The street total a wager action commits its actor to.
    fn wager_target(&self, sim: &Sim, actor: usize, action: Abs) -> u64 {
        let outstanding = self.sc[1 - actor].max(self.sc[actor]);
        match action {
            Abs::BetFixed => {
                let tier = self
                    .betting_spec(sim)
                    .map_or(sim.big_blind, |round| sim.tier_size(&round));
                if self.is_bring_in_decision(sim) || outstanding == 0 {
                    // Open (or complete the bring-in street straight) to
                    // one full bet.
                    tier.max(outstanding)
                } else {
                    outstanding + tier
                }
            }
            Abs::BetPot => {
                let call = self.call_amount(actor);
                let pot = self.commit[0] + self.commit[1];
                let to = self.sc[actor] + call + (pot + call);
                to.max(self.sc[actor] + call + sim.big_blind)
            }
            Abs::AllIn => self.sc[actor] + (sim.stack - self.commit[actor]),
            _ => unreachable!("not a wager"),
        }
    }

    fn betting_spec(&self, sim: &Sim) -> Option<BetRoundSpec> {
        sim.street(self.street).betting
    }

    /// March through deals, draw-phase setup, betting-round closure, and
    /// street changes until the next decision point or the end of the hand.
    fn advance(&mut self, sim: &Sim) {
        loop {
            match &self.phase {
                Phase::Over => return,
                Phase::Draws(pending) => {
                    if !pending.is_empty() {
                        return; // a draw decision is due
                    }
                    if self.open_betting(sim) {
                        return;
                    }
                    continue;
                }
                Phase::Betting => {
                    // The street closes once both have acted since the last
                    // wager and commitments match — or can't match because a
                    // short all-in capped one side.
                    let both_acted = self.acted[0] && self.acted[1];
                    let matched = self.sc[0] == self.sc[1];
                    let short_all_in = self.all_in_locked && self.commit.contains(&sim.stack);
                    if both_acted && (matched || short_all_in) {
                        self.next_street(sim);
                        continue;
                    }
                    if self.all_in_locked && self.call_amount(self.to_act) == 0 {
                        // Nothing to decide: auto-check through.
                        self.acted[self.to_act] = true;
                        self.to_act = 1 - self.to_act;
                        continue;
                    }
                    return; // a betting decision is due
                }
                Phase::Deal => {
                    let street = sim.street(self.street);
                    match street.deal {
                        DealSpec::None => {}
                        DealSpec::HolePrivate(n) => {
                            for seat in 0..2 {
                                for _ in 0..n {
                                    self.hole[seat].push(self.deck[self.cursor]);
                                    self.cursor += 1;
                                }
                            }
                        }
                        DealSpec::HoleUp(n) => {
                            for seat in 0..2 {
                                for _ in 0..n {
                                    self.up[seat].push(self.deck[self.cursor]);
                                    self.cursor += 1;
                                }
                            }
                        }
                        DealSpec::Community(n) => {
                            for _ in 0..n {
                                self.board.push(self.deck[self.cursor]);
                                self.cursor += 1;
                            }
                        }
                        DealSpec::Draw { .. } => {
                            // Seat order starting left of the button.
                            self.phase = Phase::Draws(vec![1, 0]);
                            continue;
                        }
                    }
                    if self.open_betting(sim) {
                        return;
                    }
                }
            }
        }
    }

    /// Open this street's betting round if it has one and betting is still
    /// live; returns true when a decision is now due. Otherwise moves on.
    fn open_betting(&mut self, sim: &Sim) -> bool {
        let Some(round) = self.betting_spec(sim) else {
            self.next_street(sim);
            return false;
        };
        if self.all_in_locked {
            self.next_street(sim);
            return false;
        }
        self.phase = Phase::Betting;
        self.acted = [false, false];
        // Street 0 keeps the blinds as live street commitments (and the
        // blind as the first wager); every later street starts clean.
        if self.street != 0 {
            self.sc = [0, 0];
            self.wagers = 0;
        }
        self.to_act = match round.first_to_act {
            FirstToAct::AfterBlinds => 0, // heads-up: the button/small blind
            FirstToAct::LeftOfButton => 1,
            FirstToAct::ByUpcards => {
                if self.is_bring_in_decision(sim) {
                    bring_in_seat(&sim.spec, &self.up)
                } else {
                    lead_seat(&sim.spec, &self.up)
                }
            }
        };
        true
    }

    fn next_street(&mut self, sim: &Sim) {
        // Fold the street into history and move on.
        self.street += 1;
        self.path.clear();
        if self.street >= sim.spec.streets.len() {
            self.phase = Phase::Over;
        } else {
            self.phase = Phase::Deal;
        }
    }
}

/// The first street with a betting round (the bring-in street for stud).
fn first_betting_street(spec: &GameSpec) -> Option<usize> {
    spec.streets.iter().position(|s| s.betting.is_some())
}

/// Deal a fresh hand: shuffle, post forced bets, advance to the first
/// decision.
pub fn new_hand(sim: &Sim, rng: &mut Rng64) -> State {
    let mut deck_cards = Vec::with_capacity(52);
    let mut deck = Deck::shuffled(rng);
    while let Some(card) = deck.draw() {
        deck_cards.push(card);
    }

    let mut state = State {
        deck: deck_cards,
        cursor: 0,
        street: 0,
        phase: Phase::Deal,
        hole: [Vec::new(), Vec::new()],
        up: [Vec::new(), Vec::new()],
        board: Vec::new(),
        commit: [0, 0],
        sc: [0, 0],
        wagers: 0,
        to_act: 0,
        acted: [false, false],
        path: String::new(),
        folded: None,
        all_in_locked: false,
    };

    match sim.spec.forced_bets {
        ForcedBets::Blinds { ante } => {
            for seat in 0..2 {
                state.commit[seat] += ante;
            }
            // Heads-up: the button (seat 0) posts the small blind.
            state.commit_to(sim, 0, sim.small_blind);
            state.commit_to(sim, 1, sim.big_blind);
            state.wagers = 1;
        }
        ForcedBets::BringIn { ante, .. } => {
            for seat in 0..2 {
                state.commit[seat] += ante;
            }
        }
    }

    state.advance(sim);
    state
}

/// Cards of `hand` ordered worst-first for the game's primary evaluator, so
/// "discard n" always means the n least useful cards.
///
/// Heuristic, not exact: multiples are gold in high games and poison in
/// lowball, badugi wants its best rainbow subset, and everything else sorts
/// by rank in the direction the evaluator prefers.
pub fn ranked_discards(kind: EvalKind, hand: &[Card]) -> Vec<Card> {
    let mut counts = [0u8; 13];
    for card in hand {
        counts[card.rank().index() as usize] += 1;
    }
    let mut cards = hand.to_vec();
    match kind {
        EvalKind::High | EvalKind::SixesOrBetterHigh => {
            // Worst = low singleton; keep pairs and high cards.
            cards.sort_by_key(|card| {
                let rank = card.rank().index();
                (counts[rank as usize], rank)
            });
        }
        EvalKind::DeuceToSevenLow => {
            // Worst = duplicated rank, then the highest cards (ace worst).
            cards.sort_by_key(|card| {
                let rank = card.rank().index();
                (
                    u8::from(counts[rank as usize] < 2),
                    13u8.saturating_sub(rank),
                )
            });
        }
        EvalKind::AceToFiveLow | EvalKind::EightOrBetterLow => {
            // Ace plays low: map ace to 0, then as 2-7.
            let low_rank = |card: &Card| -> u8 {
                let rank = card.rank().index();
                if rank == 12 { 0 } else { rank + 1 }
            };
            cards.sort_by_key(|card| {
                (
                    u8::from(counts[card.rank().index() as usize] < 2),
                    13u8.saturating_sub(low_rank(card)),
                )
            });
        }
        EvalKind::Badugi | EvalKind::BadugiAceHigh => {
            // Worst = a card duplicating the suit or rank of a better card.
            // Greedy: repeatedly keep the card that extends rank+suit
            // coverage with the lowest rank (ace low for plain badugi).
            let ace_low = kind == EvalKind::Badugi;
            let value = |card: &Card| -> u8 {
                let rank = card.rank().index();
                if ace_low && rank == 12 {
                    0
                } else {
                    rank + u8::from(ace_low)
                }
            };
            let mut keep: Vec<Card> = Vec::new();
            let mut pool = cards.clone();
            pool.sort_by_key(value);
            for card in &pool {
                let clashes = keep
                    .iter()
                    .any(|kept| kept.suit() == card.suit() || kept.rank() == card.rank());
                if !clashes {
                    keep.push(*card);
                }
            }
            cards.sort_by_key(|card| {
                let kept = keep.contains(card);
                (u8::from(kept), value(card).wrapping_neg())
            });
        }
    }
    cards
}

/// Which seat posts the stud bring-in: the worst door card — lowest rank
/// for high games (suit clubs<…<spades breaking ties low), highest for
/// razz-style `low` games.
fn bring_in_seat(spec: &GameSpec, up: &[Vec<Card>; 2]) -> usize {
    let ForcedBets::BringIn { low, .. } = spec.forced_bets else {
        unreachable!("bring-in seat only queried for stud");
    };
    let door = |seat: usize| {
        let card = up[seat][0];
        (card.rank().index(), card.suit().index())
    };
    let zero_worse = if low {
        door(0) > door(1)
    } else {
        door(0) < door(1)
    };
    if zero_worse { 0 } else { 1 }
}

/// Which seat leads a later stud street: best showing partial high (or
/// best showing low for razz); ties go to seat 0.
fn lead_seat(spec: &GameSpec, up: &[Vec<Card>; 2]) -> usize {
    let ForcedBets::BringIn { low, .. } = spec.forced_bets else {
        return 1;
    };
    let score = |seat: usize| -> (u8, Vec<u8>) {
        let mut counts = [0u8; 13];
        for card in &up[seat] {
            counts[card.rank().index() as usize] += 1;
        }
        let mut groups: Vec<(u8, u8)> = (0..13u8)
            .rev()
            .filter(|rank| counts[*rank as usize] > 0)
            .map(|rank| (counts[rank as usize], rank))
            .collect();
        groups.sort_by_key(|group| std::cmp::Reverse(group.0));
        let class = groups.first().map_or(0, |g| g.0);
        let ranks: Vec<u8> = groups.iter().map(|g| g.1).collect();
        (class, ranks)
    };
    let (a, b) = (score(0), score(1));
    let zero_leads = if low {
        // Razz: fewest pairs, then lowest showing (ace low ignored here —
        // a coarse but stable ordering).
        a <= b
    } else {
        a >= b
    };
    if zero_leads { 0 } else { 1 }
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

    fn sim(id: &str) -> Sim {
        Sim::new(GameSpec::by_id(id, STAKES).unwrap(), 10_000)
    }

    /// Play a full random hand with uniform action choice; the walk must
    /// terminate with a zero-sum settlement and legal bookkeeping.
    fn random_playout(sim: &Sim, rng: &mut Rng64) -> f64 {
        let mut state = new_hand(sim, rng);
        let mut steps = 0;
        while !state.is_terminal() {
            let actions = state.actions(sim);
            assert!(!actions.is_empty(), "decision point with no actions");
            let pick = actions[rng.below(actions.len() as u64) as usize];
            state = state.apply(sim, pick);
            steps += 1;
            assert!(steps < 200, "hand did not terminate");
        }
        state.utility(sim)
    }

    #[test]
    fn every_game_plays_random_hands_to_completion() {
        for id in GameSpec::known_ids() {
            let sim = sim(id);
            let mut rng = Rng64::from_seed_stream(11, 0);
            let mut total = 0.0;
            for _ in 0..200 {
                total += random_playout(&sim, &mut rng);
            }
            // Utility is seat-0 net chips; over random-vs-random play the
            // mean must stay small relative to the stakes involved.
            let mean = total / 200.0;
            assert!(mean.abs() < 2_000.0, "{id}: suspicious mean utility {mean}");
        }
    }

    #[test]
    fn preflop_starts_with_blinds_and_button_to_act() {
        let sim = sim("holdem-nl");
        let mut rng = Rng64::from_seed_stream(1, 0);
        let state = new_hand(&sim, &mut rng);
        assert_eq!(state.actor(), 0, "heads-up button acts first preflop");
        assert_eq!(state.kind(&sim), Kind::Wager);
        let actions = state.actions(&sim);
        assert!(actions.contains(&Abs::Fold));
        assert!(actions.contains(&Abs::CheckCall));
        assert!(actions.contains(&Abs::BetPot));
        assert!(actions.contains(&Abs::AllIn));
    }

    #[test]
    fn fixed_limit_offers_one_size_and_respects_the_cap() {
        let sim = sim("holdem-fl");
        let mut rng = Rng64::from_seed_stream(2, 0);
        let mut state = new_hand(&sim, &mut rng);
        // Raise the maximum number of times; the menu must run dry.
        let mut raises = 0;
        loop {
            let actions = state.actions(&sim);
            if let Some(bet) = actions.iter().find(|a| matches!(a, Abs::BetFixed)) {
                state = state.apply(&sim, *bet);
                raises += 1;
                assert!(raises <= 4, "cap of 4 wagers exceeded");
            } else {
                break;
            }
        }
        assert_eq!(raises, 3, "blind counts as the first of 4 wagers");
    }

    #[test]
    fn stud_hands_open_with_a_bring_in_decision() {
        let sim = sim("stud-fl");
        let mut rng = Rng64::from_seed_stream(3, 0);
        let state = new_hand(&sim, &mut rng);
        assert_eq!(state.kind(&sim), Kind::BringIn);
        assert_eq!(state.actions(&sim), vec![Abs::BringIn, Abs::BetFixed]);
    }

    #[test]
    fn draw_games_ask_for_counts_in_seat_order() {
        let sim = sim("27td-fl");
        let mut rng = Rng64::from_seed_stream(4, 0);
        let mut state = new_hand(&sim, &mut rng);
        // Call/check through predraw betting to reach the draw phase.
        let mut steps = 0;
        while state.kind(&sim) != Kind::Draw {
            state = state.apply(&sim, Abs::CheckCall);
            steps += 1;
            assert!(steps < 10, "never reached a draw phase");
        }
        assert_eq!(state.actor(), 1, "left of the button draws first");
        assert_eq!(state.actions(&sim).len(), 6, "0..=5 discards");
        let hand_before: Vec<Card> = state.hole[1].clone();
        let after = state.apply(&sim, Abs::Draw(2));
        assert_eq!(after.hole[1].len(), 5);
        assert_ne!(after.hole[1], hand_before);
        assert!(after.path.contains("g2"));
    }

    #[test]
    fn ranked_discards_keep_pairs_in_high_and_shed_them_in_lowball() {
        let hand = parse_cards("Kc Kd 7h 4s 2c").unwrap();
        let high = ranked_discards(EvalKind::High, &hand);
        assert_eq!(
            high[0],
            parse_cards("2c").unwrap()[0],
            "low singleton first"
        );
        assert!(
            high[3..].iter().all(|c| c.rank().index() == 11),
            "kings kept last"
        );

        let low = ranked_discards(EvalKind::DeuceToSevenLow, &hand);
        assert_eq!(
            low[0].rank().index(),
            11,
            "a duplicated king goes first in 2-7"
        );

        let badugi = ranked_discards(EvalKind::Badugi, &hand);
        // Kd clashes with Kc by rank; one king must be among the first out.
        assert!(badugi[..2].iter().any(|c| c.rank().index() == 11));
    }

    #[test]
    fn utilities_are_zero_sum_on_folds() {
        let sim = sim("holdem-nl");
        let mut rng = Rng64::from_seed_stream(5, 0);
        let state = new_hand(&sim, &mut rng);
        let folded = state.apply(&sim, Abs::Fold);
        assert!(folded.is_terminal());
        assert_eq!(folded.utility(&sim), -50.0, "button folds the small blind");
    }
}

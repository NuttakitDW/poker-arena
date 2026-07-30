//! The per-hand state machine.
//!
//! `HandState` interprets a [`GameSpec`] to run exactly one hand: forced
//! bets, dealing, betting rounds, showdown, settlement. It is pure and
//! synchronous — no I/O, no clocks, no bots. The arena layer owns all of
//! those and simply calls `to_act` / `legal_actions` / `apply` in a loop.
//!
//! # Rules contract (the implementation spec)
//!
//! ## Setup (`new`)
//! - Validates seat count against the spec, stacks all positive, and (M1)
//!   rejects specs using M3 features (`BringIn`, `HoleUp`, `Draw`,
//!   `ByUpcards`) with `HandError::Unsupported`.
//! - Emits `HandStart`, posts forced bets (heads-up: the **button posts the
//!   small blind** and acts first preflop), deals street 0, emits
//!   `StreetStart`/`DealHole` events, and opens street 0's betting round.
//! - A blind that covers a stack posts all-in for less (`Post.all_in`).
//!
//! ## Betting rounds
//! - State per round: each seat's street commitment; `current_to` (highest
//!   commitment, = big blind preflop before any raise); the size of the last
//!   full raise; who still must act. A round ends when every non-folded,
//!   non-all-in seat has either matched `current_to` or checked, *and* has
//!   acted since the last full wager — with the classic exception that the
//!   big blind gets its option preflop (may raise an unraised pot even
//!   though its commitment already matches).
//! - Action order: [`FirstToAct::AfterBlinds`] (street 0 of blind games) —
//!   first seat after the big blind, heads-up the button; otherwise
//!   [`FirstToAct::LeftOfButton`] — first non-folded seat left of button.
//!   Folded and all-in seats are skipped.
//! - **Fold** is legal only when facing chips to call. **Check** only when
//!   not. **Call** always available facing a wager (all-in for less when
//!   short). `Bet {to}` opens a street with no wager; `Raise {to}` increases
//!   an existing one; `to` is the seat's *total* street commitment and must
//!   lie in the offered [`BetBounds`].
//! - **No-limit**: opening bet minimum = big blind; minimum raise increment
//!   = size of the last full bet/raise this street (initially the big
//!   blind); maximum = actor's all-in total. A short all-in below the full
//!   minimum is legal (bounds collapse to the all-in) but does **not**
//!   reopen the action: seats that already acted at the prior price may only
//!   call or fold when action returns to them, and the min-raise base for
//!   later full raises is unchanged by the short wager.
//! - **Pot-limit**: same as no-limit except the maximum. Implement exactly
//!   as `max_to = to_call_total + pot_after_call`, where `to_call_total` is
//!   the actor's street commitment after a hypothetical call and
//!   `pot_after_call = pot_total_before_action + to_call_amount` (the
//!   classic "call, then raise the size of the pot"). Clamp to all-in.
//! - **Fixed-limit**: wager sizes fixed at `spec.tier_size(street.tier)`;
//!   `min_to == max_to == current_to + tier` (or `tier` for the opening
//!   bet). The round's wager count is capped per
//!   `BettingKind::FixedLimit { raise_cap }`: the opening bet counts as
//!   wager 1, and preflop the big blind itself counts as wager 1. At the
//!   cap, only call/fold are offered. A short all-in "raise" is allowed
//!   below tier size when it is the actor's whole stack; it counts toward
//!   the cap only if ≥ half the tier (half-bet rule) — otherwise treated as
//!   a call-and-more that does not reopen action.
//!
//! ## Street advancement & hand end
//! - `apply` auto-advances: when a betting round completes, deal the next
//!   street (events in order: `StreetStart`, deal event) and open its
//!   betting round. If all but one seat folds, the hand ends immediately:
//!   refund the uncalled excess of the last wager to its owner (no event;
//!   reflected in nets), award the pot without showdown (`PotAwarded` with
//!   `PotSide::Whole`, no `ShowdownShow`), emit `HandEnd`.
//! - When at most one non-all-in seat remains with a live wager matched
//!   (betting cannot continue), remaining streets are dealt out with no
//!   betting rounds ("run-out") straight to showdown.
//! - Uncalled excess: whenever a street's highest commitment exceeds the
//!   second-highest among non-folded seats at the moment betting on that
//!   street ends (all-in situations), the difference is returned to the
//!   over-committed seat before pot construction.
//!
//! ## Showdown & settlement
//! - Every non-folded seat reveals (`ShowdownShow`, engine-computed values
//!   via `eval::best_with_usage` per `spec.showdown`). Order of reveal:
//!   odd-chip order (left of button first) — arena bots learn everything
//!   either way; there is no strategic mucking.
//! - Pots built by `pot::build_pots` from full-hand contributions, awarded
//!   by `pot::award_pots` (odd-chip rules documented there). One
//!   `PotAwarded` event per pot side.
//! - `HandEnd { nets }` closes the hand: `nets[s] = won − contributed`,
//!   `sum(nets) == 0` (chip conservation — property-tested).
//!
//! ## Invariants (must be property-tested)
//! - Chip conservation at `HandEnd`.
//! - `apply(a)` succeeds iff `a` conforms to the last `legal_actions()`.
//! - Every hand terminates (fold-out, or showdown after the last street).
//! - Stacks never go negative; commitments never exceed starting stacks.

use super::action::{Action, BetBounds, Chips, LegalActions, Seat};
use super::event::{Event, PostKind, PotSide};
use super::pot::{PotAward, ShowdownEntry, award_pots, build_pots};
use super::spec::{BettingKind, DealSpec, FirstToAct, ForcedBets, GameSpec, PotSplit};
use crate::card::{Card, Deck};
use crate::eval::{HandValue, best_with_usage};

/// Errors constructing a hand.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HandError {
    #[error("seat count {0} outside the spec's supported range")]
    BadSeatCount(usize),
    #[error("all stacks must be positive")]
    BadStacks,
    #[error("deck exhausted while dealing")]
    DeckExhausted,
    #[error("spec feature not yet supported: {0}")]
    Unsupported(&'static str),
}

/// Errors applying an action.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ActionError {
    #[error("hand is over; no actions accepted")]
    HandOver,
    #[error("action {action:?} is not legal now: {reason}")]
    Illegal {
        action: Action,
        reason: &'static str,
    },
}

/// Final accounting for a completed hand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settlement {
    /// `nets[seat] = chips won − chips contributed`; sums to zero.
    pub nets: Vec<i64>,
    pub awards: Vec<PotAward>,
    /// Seats that reached showdown (empty on a fold-out).
    pub showdown_seats: Vec<Seat>,
}

/// State of one hand in progress. See module docs for the full rules
/// contract. Construction deals street 0; drive it with `to_act` /
/// `legal_actions` / `apply` until `is_over`.
#[derive(Clone, Debug)]
pub struct HandState {
    spec: GameSpec,
    button: Seat,
    seats: usize,
    deck: Deck,
    hole: Vec<Vec<Card>>,
    board: Vec<Card>,
    /// Starting stack − contributions so far (refunds add back).
    stacks: Vec<Chips>,
    /// Everything a seat has put in this hand, antes included.
    contrib: Vec<Chips>,
    /// Current-street commitment; antes are deliberately excluded so they
    /// never affect call/raise arithmetic.
    commit: Vec<Chips>,
    folded: Vec<bool>,
    all_in: Vec<bool>,
    /// Has this seat acted since the last wager that reopened the action?
    /// Cleared for everyone else by a *full* bet/raise; a short all-in
    /// leaves it set, which is exactly the no-reopen rule.
    acted: Vec<bool>,
    street: usize,
    current_to: Chips,
    /// Size of the last full bet/raise this street; also the minimum
    /// opening bet while `current_to == 0`.
    last_raise: Chips,
    /// Fixed-limit wager count for the current round (cap bookkeeping).
    wagers: u8,
    to_act: Option<Seat>,
    over: bool,
    settlement: Option<Settlement>,
    events: Vec<Event>,
}

impl HandState {
    /// Start a hand. `button` indexes into `stacks` (len = seat count).
    /// Deals from `deck`; the deck must hold enough cards for a full
    /// run-out. Returns the state plus all events emitted so far
    /// (hand start, posts, street 0 deal).
    pub fn new(
        spec: &GameSpec,
        stacks: &[Chips],
        button: Seat,
        hand_no: u64,
        deck: Deck,
    ) -> Result<(HandState, Vec<Event>), HandError> {
        let seats = stacks.len();
        if seats < *spec.seats.start() as usize || seats > *spec.seats.end() as usize {
            return Err(HandError::BadSeatCount(seats));
        }
        assert!(button < seats, "button seat {button} out of range");
        if stacks.contains(&0) {
            return Err(HandError::BadStacks);
        }
        check_supported(spec)?;
        // Every later deal must succeed, so size the deck up front: `apply`
        // has no way to report a deck error.
        if deck.remaining() < cards_needed(spec, seats) {
            return Err(HandError::DeckExhausted);
        }

        let mut state = HandState {
            spec: spec.clone(),
            button,
            seats,
            deck,
            hole: vec![Vec::new(); seats],
            board: Vec::new(),
            stacks: stacks.to_vec(),
            contrib: vec![0; seats],
            commit: vec![0; seats],
            folded: vec![false; seats],
            all_in: vec![false; seats],
            acted: vec![false; seats],
            street: 0,
            current_to: 0,
            last_raise: spec.stakes.big_blind.max(1),
            wagers: 0,
            to_act: None,
            over: false,
            settlement: None,
            events: Vec::new(),
        };

        let mut ev = vec![Event::HandStart {
            hand_no,
            button,
            stacks: stacks.to_vec(),
        }];
        state.post_forced(&mut ev);
        state.deal_street(&mut ev);
        if !state.open_round() {
            state.advance(&mut ev);
        }
        state.events.extend_from_slice(&ev);
        Ok((state, ev))
    }

    /// The seat that must act, or `None` when the hand is over.
    pub fn to_act(&self) -> Option<Seat> {
        self.to_act
    }

    /// Legal actions for the seat to act; `None` when the hand is over.
    pub fn legal_actions(&self) -> Option<LegalActions> {
        let seat = self.to_act?;
        let commit = self.commit[seat];
        let stack = self.stacks[seat];
        let all_in_to = commit + stack;
        let owed = self.current_to.saturating_sub(commit);

        let mut la = LegalActions {
            fold: owed > 0,
            check: owed == 0,
            call: (owed > 0).then(|| owed.min(stack)),
            ..LegalActions::default()
        };

        // Wagering only makes sense while some other seat can still respond.
        let contested = (0..self.seats).any(|s| s != seat && !self.folded[s] && !self.all_in[s]);
        if !contested || all_in_to <= self.current_to {
            return Some(la);
        }

        let opening = self.current_to == 0;
        // A short all-in that did not reopen the action leaves `acted` set,
        // which is precisely who may no longer raise.
        if !opening && self.acted[seat] {
            return Some(la);
        }

        let tier = self.spec.tier_size(
            self.spec.streets[self.street]
                .betting
                .expect("betting round open")
                .tier,
        );
        let bounds = match self.spec.betting {
            BettingKind::FixedLimit { raise_cap } => {
                if raise_cap.is_some_and(|cap| self.wagers >= cap) {
                    None
                } else {
                    let to = if opening {
                        tier.min(all_in_to)
                    } else {
                        (self.current_to + tier).min(all_in_to)
                    };
                    Some(BetBounds {
                        min_to: to,
                        max_to: to,
                    })
                }
            }
            BettingKind::NoLimit | BettingKind::PotLimit => {
                let max_to = match self.spec.betting {
                    BettingKind::PotLimit => self.pot_limit_max(seat).min(all_in_to),
                    _ => all_in_to,
                };
                let min_to = if opening {
                    self.last_raise
                } else {
                    self.current_to + self.last_raise
                };
                Some(BetBounds {
                    min_to: min_to.min(max_to),
                    max_to,
                })
            }
        };

        if let Some(b) = bounds
            && b.max_to > self.current_to
        {
            if opening {
                la.bet = Some(b);
            } else {
                la.raise = Some(b);
            }
        }
        Some(la)
    }

    /// Apply an action for the seat returned by `to_act`, returning every
    /// event that resulted (the action itself, street advances, deals,
    /// showdown, settlement…). Illegal actions leave state untouched.
    pub fn apply(&mut self, action: Action) -> Result<Vec<Event>, ActionError> {
        let Some(la) = self.legal_actions() else {
            return Err(ActionError::HandOver);
        };
        if let Err(reason) = conforms(&action, &la) {
            return Err(ActionError::Illegal { action, reason });
        }
        // Everything below this point is infallible: no partial mutation.
        let seat = self.to_act.expect("legal_actions implies a seat to act");
        let mut ev = Vec::new();

        match action {
            Action::Fold => self.folded[seat] = true,
            Action::Check => {}
            Action::Call => {
                let pay = (self.current_to - self.commit[seat]).min(self.stacks[seat]);
                self.pay(seat, pay);
            }
            Action::Bet { to } | Action::Raise { to } => {
                self.pay(seat, to - self.commit[seat]);
                let inc = to - self.current_to;
                self.current_to = to;
                if self.is_full_wager(inc) {
                    self.wagers += 1;
                    if !matches!(self.spec.betting, BettingKind::FixedLimit { .. }) {
                        self.last_raise = inc;
                    }
                    for s in 0..self.seats {
                        if s != seat {
                            self.acted[s] = false;
                        }
                    }
                }
            }
            Action::BringIn | Action::Discard { .. } => unreachable!("rejected by `conforms`"),
        }
        self.acted[seat] = true;
        ev.push(Event::Acted {
            seat,
            action,
            street_commit: self.commit[seat],
            all_in: self.all_in[seat],
        });

        if self.live_count() <= 1 || self.round_complete() {
            self.advance(&mut ev);
        } else {
            self.to_act = Some(self.next_to_act(seat));
        }
        self.events.extend_from_slice(&ev);
        Ok(ev)
    }

    pub fn is_over(&self) -> bool {
        self.over
    }

    /// Settlement, once `is_over`.
    pub fn settlement(&self) -> Option<&Settlement> {
        self.settlement.as_ref()
    }

    // --- Read-only views (for arena/bot consumption and logging) ---

    /// Community cards dealt so far.
    pub fn board(&self) -> &[Card] {
        &self.board
    }

    /// Hole cards of a seat (unredacted — callers redact via events).
    pub fn hole_cards(&self, seat: Seat) -> &[Card] {
        &self.hole[seat]
    }

    /// Current street index and label.
    pub fn street(&self) -> (u8, &'static str) {
        (self.street as u8, self.spec.streets[self.street].label)
    }

    /// Remaining stack per seat (starting stack − contributions so far).
    pub fn stacks(&self) -> &[Chips] {
        &self.stacks
    }

    /// Each seat's commitment on the current street.
    pub fn street_commits(&self) -> &[Chips] {
        &self.commit
    }

    /// Total chips in the pot (all streets, including current commitments).
    pub fn pot_total(&self) -> Chips {
        self.contrib.iter().sum()
    }

    pub fn folded(&self) -> &[bool] {
        &self.folded
    }

    pub fn all_in(&self) -> &[bool] {
        &self.all_in
    }

    /// Full unredacted event history since hand start.
    pub fn events(&self) -> &[Event] {
        &self.events
    }

    // --- Forced bets & dealing ------------------------------------------

    fn post_forced(&mut self, ev: &mut Vec<Event>) {
        let ForcedBets::Blinds { ante } = self.spec.forced_bets else {
            unreachable!("non-blind forced bets rejected in new()")
        };
        if ante > 0 {
            for i in 1..=self.seats {
                let seat = (self.button + i) % self.seats;
                let amount = ante.min(self.stacks[seat]);
                if amount == 0 {
                    continue;
                }
                self.stacks[seat] -= amount;
                self.contrib[seat] += amount;
                self.all_in[seat] = self.stacks[seat] == 0;
                ev.push(Event::Post {
                    seat,
                    kind: PostKind::Ante,
                    amount,
                    all_in: self.all_in[seat],
                });
            }
        }

        let (sb_seat, bb_seat) = self.blind_seats();
        let stakes = self.spec.stakes;
        for (seat, kind, nominal) in [
            (sb_seat, PostKind::SmallBlind, stakes.small_blind),
            (bb_seat, PostKind::BigBlind, stakes.big_blind),
        ] {
            let amount = nominal.min(self.stacks[seat]);
            if amount == 0 {
                continue;
            }
            self.pay(seat, amount);
            ev.push(Event::Post {
                seat,
                kind,
                amount,
                all_in: self.all_in[seat],
            });
        }

        // A short all-in blind never lowers the price for anyone else.
        self.current_to = stakes.big_blind;
        self.last_raise = stakes.big_blind.max(1);
        if matches!(self.spec.betting, BettingKind::FixedLimit { .. }) {
            self.wagers = 1;
        }
    }

    /// Hole cards are dealt one full batch per seat (not round-robin),
    /// in seat order starting left of the button; community cards are one
    /// batch per street. Scripted tests depend on this order.
    fn deal_street(&mut self, ev: &mut Vec<Event>) {
        let idx = self.street;
        let label = self.spec.streets[idx].label;
        let deal = self.spec.streets[idx].deal.clone();
        ev.push(Event::StreetStart {
            street: idx as u8,
            label,
        });
        match deal {
            DealSpec::None => {}
            DealSpec::HolePrivate(count) => {
                for i in 1..=self.seats {
                    let seat = (self.button + i) % self.seats;
                    if self.folded[seat] {
                        continue;
                    }
                    let cards = self
                        .deck
                        .draw_n(count as usize)
                        .expect("deck sized in new()");
                    self.hole[seat].extend_from_slice(&cards);
                    ev.push(Event::DealHole { seat, cards, count });
                }
            }
            DealSpec::Community(count) => {
                let cards = self
                    .deck
                    .draw_n(count as usize)
                    .expect("deck sized in new()");
                self.board.extend_from_slice(&cards);
                ev.push(Event::DealCommunity {
                    street: idx as u8,
                    cards,
                });
            }
            DealSpec::HoleUp(_) | DealSpec::Draw { .. } => {
                unreachable!("M3 deal specs rejected in new()")
            }
        }
    }

    // --- Round bookkeeping ----------------------------------------------

    fn blind_seats(&self) -> (Seat, Seat) {
        if self.seats == 2 {
            (self.button, (self.button + 1) % self.seats)
        } else {
            (
                (self.button + 1) % self.seats,
                (self.button + 2) % self.seats,
            )
        }
    }

    fn pay(&mut self, seat: Seat, amount: Chips) {
        self.stacks[seat] -= amount;
        self.contrib[seat] += amount;
        self.commit[seat] += amount;
        if self.stacks[seat] == 0 {
            self.all_in[seat] = true;
        }
    }

    /// Does a wager of increment `inc` reopen the action (and count toward
    /// the fixed-limit cap)?
    fn is_full_wager(&self, inc: Chips) -> bool {
        match self.spec.betting {
            BettingKind::FixedLimit { .. } => {
                let tier = self.spec.tier_size(
                    self.spec.streets[self.street]
                        .betting
                        .expect("betting round open")
                        .tier,
                );
                inc * 2 >= tier
            }
            _ => inc >= self.last_raise,
        }
    }

    /// Pot-limit ceiling: call first, then raise the size of the resulting
    /// pot.
    fn pot_limit_max(&self, seat: Seat) -> Chips {
        let to_call = self.current_to.saturating_sub(self.commit[seat]);
        self.current_to + self.pot_total() + to_call
    }

    fn live_count(&self) -> usize {
        self.folded.iter().filter(|&&f| !f).count()
    }

    fn active_count(&self) -> usize {
        (0..self.seats)
            .filter(|&s| !self.folded[s] && !self.all_in[s])
            .count()
    }

    fn round_complete(&self) -> bool {
        (0..self.seats).all(|s| {
            self.folded[s] || self.all_in[s] || (self.acted[s] && self.commit[s] == self.current_to)
        })
    }

    fn next_to_act(&self, from: Seat) -> Seat {
        for i in 1..=self.seats {
            let seat = (from + i) % self.seats;
            if !self.folded[seat]
                && !self.all_in[seat]
                && (!self.acted[seat] || self.commit[seat] != self.current_to)
            {
                return seat;
            }
        }
        unreachable!("round_complete() was false, so someone still owes action")
    }

    fn reset_round(&mut self) {
        for s in 0..self.seats {
            self.commit[s] = 0;
            self.acted[s] = false;
        }
        self.current_to = 0;
        self.last_raise = self.spec.stakes.big_blind.max(1);
        self.wagers = 0;
    }

    /// Sets `to_act` for the current street; `false` when no betting round
    /// runs (no spec'd round, or fewer than two seats can act).
    fn open_round(&mut self) -> bool {
        self.to_act = None;
        let Some(round) = self.spec.streets[self.street].betting else {
            return false;
        };
        if self.active_count() < 2 {
            return false;
        }
        let start = match round.first_to_act {
            FirstToAct::AfterBlinds => (self.blind_seats().1 + 1) % self.seats,
            FirstToAct::LeftOfButton => (self.button + 1) % self.seats,
            FirstToAct::ByUpcards => unreachable!("rejected in new()"),
        };
        for i in 0..self.seats {
            let seat = (start + i) % self.seats;
            if !self.folded[seat] && !self.all_in[seat] {
                self.to_act = Some(seat);
                return true;
            }
        }
        false
    }

    // --- Street advancement, showdown, settlement -------------------------

    /// Called whenever a betting round has finished (or could not open):
    /// refunds, then either ends the hand or runs out further streets.
    fn advance(&mut self, ev: &mut Vec<Event>) {
        loop {
            self.refund_uncalled();
            if self.live_count() <= 1 {
                self.finish_foldout(ev);
                return;
            }
            if self.street + 1 >= self.spec.streets.len() {
                self.finish_showdown(ev);
                return;
            }
            self.street += 1;
            self.reset_round();
            self.deal_street(ev);
            if self.open_round() {
                return;
            }
        }
    }

    /// Return the part of the top street commitment nobody could match.
    fn refund_uncalled(&mut self) {
        let Some(top) = (0..self.seats)
            .filter(|&s| !self.folded[s])
            .max_by_key(|&s| self.commit[s])
        else {
            return;
        };
        // Folded seats' chips count as "called" up to what they put in.
        let matched = (0..self.seats)
            .filter(|&s| s != top)
            .map(|s| self.commit[s])
            .max()
            .unwrap_or(0);
        if self.commit[top] <= matched {
            return;
        }
        let refund = self.commit[top] - matched;
        self.commit[top] -= refund;
        self.contrib[top] -= refund;
        self.stacks[top] += refund;
        if self.stacks[top] > 0 {
            self.all_in[top] = false;
        }
    }

    fn odd_chip_order(&self) -> Vec<Seat> {
        (1..=self.seats)
            .map(|i| (self.button + i) % self.seats)
            .collect()
    }

    fn finish_foldout(&mut self, ev: &mut Vec<Event>) {
        let winner = (0..self.seats)
            .find(|&s| !self.folded[s])
            .expect("a hand always has a last seat standing");
        let awards = build_pots(&self.contrib, &self.folded)
            .into_iter()
            .enumerate()
            .map(|(i, p)| PotAward {
                pot: i as u8,
                side: PotSide::Whole,
                winners: vec![(winner, p.amount)],
            })
            .collect();
        self.finish(ev, awards, Vec::new());
    }

    fn finish_showdown(&mut self, ev: &mut Vec<Event>) {
        let (hi_kind, lo_kind) = match self.spec.showdown.pot_split {
            PotSplit::Hi(hi) => (hi, None),
            PotSplit::HiLo { hi, lo } => (hi, Some(lo)),
        };
        let usage = self.spec.showdown.hole_usage;
        let order = self.odd_chip_order();
        let mut entries: Vec<ShowdownEntry> = Vec::new();
        let mut showdown_seats: Vec<Seat> = Vec::new();
        for &seat in &order {
            if self.folded[seat] {
                continue;
            }
            let hole = &self.hole[seat];
            let hi = best_with_usage(hi_kind, usage, hole, &self.board);
            let lo = lo_kind.and_then(|k| best_with_usage(k, usage, hole, &self.board));
            ev.push(Event::ShowdownShow {
                seat,
                cards: hole.clone(),
                hi,
                lo,
            });
            entries.push(ShowdownEntry {
                seat,
                hi: hi.unwrap_or(HandValue(0)),
                lo,
            });
            showdown_seats.push(seat);
        }
        let pots = build_pots(&self.contrib, &self.folded);
        let has_low = lo_kind.is_some();
        let awards = award_pots(&pots, &entries, has_low, &order);
        self.finish(ev, awards, showdown_seats);
    }

    fn finish(&mut self, ev: &mut Vec<Event>, awards: Vec<PotAward>, showdown_seats: Vec<Seat>) {
        let mut won = vec![0 as Chips; self.seats];
        for award in &awards {
            for &(seat, amount) in &award.winners {
                won[seat] += amount;
            }
            ev.push(Event::PotAwarded {
                pot: award.pot,
                side: award.side,
                winners: award.winners.clone(),
            });
        }
        let nets: Vec<i64> = (0..self.seats)
            .map(|s| won[s] as i64 - self.contrib[s] as i64)
            .collect();
        ev.push(Event::HandEnd { nets: nets.clone() });
        self.to_act = None;
        self.over = true;
        self.settlement = Some(Settlement {
            nets,
            awards,
            showdown_seats,
        });
    }
}

/// M1 supports blind games with private-hole/community deals only.
fn check_supported(spec: &GameSpec) -> Result<(), HandError> {
    if matches!(spec.forced_bets, ForcedBets::BringIn { .. }) {
        return Err(HandError::Unsupported("bring-in forced bets"));
    }
    for street in &spec.streets {
        match street.deal {
            DealSpec::HoleUp(_) => return Err(HandError::Unsupported("face-up hole cards")),
            DealSpec::Draw { .. } => return Err(HandError::Unsupported("draw streets")),
            _ => {}
        }
        if let Some(round) = street.betting
            && round.first_to_act == FirstToAct::ByUpcards
        {
            return Err(HandError::Unsupported("upcard-determined action order"));
        }
    }
    Ok(())
}

/// Upper bound on cards consumed by a full run-out.
fn cards_needed(spec: &GameSpec, seats: usize) -> usize {
    spec.streets
        .iter()
        .map(|street| match street.deal {
            DealSpec::None => 0,
            DealSpec::HolePrivate(n) | DealSpec::HoleUp(n) => n as usize * seats,
            DealSpec::Community(n) => n as usize,
            DealSpec::Draw { max } => max as usize * seats,
        })
        .sum()
}

/// `apply` accepts exactly what the last `legal_actions()` offered.
fn conforms(action: &Action, la: &LegalActions) -> Result<(), &'static str> {
    let in_bounds = |to: &Chips, b: Option<BetBounds>, err| match b {
        Some(b) if (b.min_to..=b.max_to).contains(to) => Ok(()),
        Some(_) => Err("wager size outside the legal bounds"),
        None => Err(err),
    };
    match action {
        Action::Fold if la.fold => Ok(()),
        Action::Fold => Err("folding is only legal when facing a wager"),
        Action::Check if la.check => Ok(()),
        Action::Check => Err("cannot check while facing a wager"),
        Action::Call if la.call.is_some() => Ok(()),
        Action::Call => Err("there is nothing to call"),
        Action::Bet { to } => in_bounds(to, la.bet, "betting is closed; a wager is already live"),
        Action::Raise { to } => in_bounds(to, la.raise, "raising is not available"),
        Action::BringIn => Err("this game has no bring-in"),
        Action::Discard { .. } => Err("no draw round is in progress"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::spec::Stakes;

    fn nl(seats: u8) -> GameSpec {
        let mut spec = GameSpec::holdem_nl(Stakes {
            small_blind: 1,
            big_blind: 2,
        });
        spec.seats = 2..=seats.max(2);
        spec
    }

    fn start(spec: &GameSpec, stacks: &[Chips]) -> HandState {
        HandState::new(spec, stacks, 0, 1, Deck::standard())
            .unwrap()
            .0
    }

    #[test]
    fn rejects_unsupported_specs() {
        let mut spec = nl(9);
        spec.forced_bets = ForcedBets::BringIn {
            ante: 1,
            bring_in: 2,
        };
        assert_eq!(
            HandState::new(&spec, &[100, 100], 0, 1, Deck::standard()).err(),
            Some(HandError::Unsupported("bring-in forced bets"))
        );

        let mut spec = nl(9);
        spec.streets[0].deal = DealSpec::HoleUp(2);
        assert!(matches!(
            HandState::new(&spec, &[100, 100], 0, 1, Deck::standard()).err(),
            Some(HandError::Unsupported(_))
        ));

        let mut spec = nl(9);
        spec.streets[1].betting.as_mut().unwrap().first_to_act = FirstToAct::ByUpcards;
        assert!(matches!(
            HandState::new(&spec, &[100, 100], 0, 1, Deck::standard()).err(),
            Some(HandError::Unsupported(_))
        ));
    }

    #[test]
    fn rejects_bad_setup() {
        let spec = nl(9);
        assert_eq!(
            HandState::new(&spec, &[100], 0, 1, Deck::standard()).err(),
            Some(HandError::BadSeatCount(1))
        );
        assert_eq!(
            HandState::new(&spec, &[100, 0], 0, 1, Deck::standard()).err(),
            Some(HandError::BadStacks)
        );
        let short = Deck::from_deal_order(&crate::card::parse_cards("As Ks Qs Js").unwrap());
        assert_eq!(
            HandState::new(&spec, &[100, 100], 0, 1, short).err(),
            Some(HandError::DeckExhausted)
        );
    }

    #[test]
    fn cards_needed_covers_holdem_runout() {
        let spec = nl(9);
        assert_eq!(cards_needed(&spec, 6), 6 * 2 + 5);
    }

    #[test]
    fn heads_up_blinds_and_first_action() {
        let hand = start(&nl(9), &[100, 100]);
        // Button (seat 0) posts the small blind and acts first preflop.
        assert_eq!(hand.commit, vec![1, 2]);
        assert_eq!(hand.to_act(), Some(0));
        assert_eq!(hand.current_to, 2);
    }

    #[test]
    fn multiway_blinds_and_first_action() {
        let hand = start(&nl(9), &[100, 100, 100, 100]);
        assert_eq!(hand.commit, vec![0, 1, 2, 0]);
        assert_eq!(hand.to_act(), Some(3));
    }

    #[test]
    fn antes_do_not_count_toward_street_commitment() {
        let mut spec = nl(9);
        spec.forced_bets = ForcedBets::Blinds { ante: 5 };
        let hand = start(&spec, &[100, 100, 100]);
        assert_eq!(hand.contrib, vec![5, 6, 7]);
        assert_eq!(hand.commit, vec![0, 1, 2]);
        assert_eq!(hand.pot_total(), 18);
        // Price is still the big blind, not the blind + ante.
        assert_eq!(hand.legal_actions().unwrap().call, Some(2));
    }

    #[test]
    fn no_limit_min_raise_ladder() {
        let mut hand = start(&nl(9), &[200, 200, 200]);
        // Preflop: min raise-to is 2 * BB, max is the all-in total.
        let la = hand.legal_actions().unwrap();
        assert_eq!(
            la.raise,
            Some(BetBounds {
                min_to: 4,
                max_to: 200
            })
        );
        hand.apply(Action::Raise { to: 10 }).unwrap();
        // Last full raise was 8, so the next min raise-to is 18.
        assert_eq!(
            hand.legal_actions().unwrap().raise,
            Some(BetBounds {
                min_to: 18,
                max_to: 200
            })
        );
    }

    #[test]
    fn pot_limit_opening_raise_formula() {
        let mut spec = nl(9);
        spec.betting = BettingKind::PotLimit;
        let hand = start(&spec, &[200, 200, 200]);
        // Pot 3, calling 2 -> max raise-to = 2 + (3 + 2) = 7.
        assert_eq!(
            hand.legal_actions().unwrap().raise,
            Some(BetBounds {
                min_to: 4,
                max_to: 7
            })
        );
    }

    #[test]
    fn illegal_action_leaves_state_untouched() {
        let mut hand = start(&nl(9), &[200, 200, 200]);
        let before = hand.events().len();
        assert!(matches!(
            hand.apply(Action::Raise { to: 3 }),
            Err(ActionError::Illegal { .. })
        ));
        assert_eq!(hand.to_act(), Some(0));
        assert_eq!(hand.events().len(), before);
        assert_eq!(hand.pot_total(), 3);
    }
}

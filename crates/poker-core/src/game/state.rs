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
//! - Validates seat count against the spec and that stacks are all positive.
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
//!   minimum is legal (bounds collapse to the all-in) and never changes the
//!   min-raise base. **Reopening (TDA cumulative rule)**: track the table
//!   price at each seat's last action (`acted_to`); when action returns to
//!   a seat that already acted, raising is offered iff
//!   `current_to − acted_to[seat] ≥ last full raise size` — one short
//!   all-in does not reopen, but several that *cumulatively* amount to a
//!   full raise do. (Example: bet 500, raise to 1200, all-in 1700, all-in
//!   2000 → the original raiser faces 2000−1200 = 800 ≥ 700 and may
//!   re-raise, minimum to 2700.)
//! - **Pot-limit**: same as no-limit except the maximum. Implement exactly
//!   as `max_to = to_call_total + pot_after_call`, where `to_call_total` is
//!   the actor's street commitment after a hypothetical call and
//!   `pot_after_call = pot_total_before_action + to_call_amount` (the
//!   classic "call, then raise the size of the pot"). Clamp to all-in.
//! - **Fixed-limit** is *additive*: with street tier `T`, a raise is always
//!   **to `current_to + T`** — one full bet on top of the price actually
//!   showing, all-in amounts included — except while no full wager has been
//!   made this street (`wagers == 0`: a stud bring-in is pending, or the
//!   street's only wager so far was a sub-half short all-in), where the
//!   wager *completes* to `T` flat. Clamped to the actor's all-in, so
//!   `min_to == max_to` always.
//!   **Half-bet rule**: a short all-in whose increment is at least half of
//!   `T` (integer form `2·inc ≥ T`) is a raise — it consumes a cap slot and
//!   reopens the action; below half it is a call-plus-extra that consumes
//!   no slot and reopens nothing. **Reopening** for a seat that already
//!   acted follows the same cumulative rule as no-limit, with threshold
//!   `T/2`: raising is offered iff `2·(current_to − acted_to[seat]) ≥ T`,
//!   so two short all-ins that together add half a bet reopen a seat that
//!   neither would alone. Such a raise is an ordinary wager to
//!   `current_to + T` and consumes its own cap slot.
//!   The cap (`BettingKind::FixedLimit { raise_cap }`) counts wagers:
//!   preflop the big blind is the first, postflop the opening bet is; the
//!   stud bring-in is never one, and a short all-in counts only when the
//!   half-bet rule makes it a raise. At `wagers >= cap` only call/fold are
//!   offered — no exceptions, reopened or not.
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
//! ## Stud games (`ForcedBets::BringIn`)
//! - Setup: every seat antes (all-in capped; antes count toward
//!   contributions, never street commitment), then streets are dealt/opened
//!   in spec order. Bet-less streets (`betting: None`) just deal.
//!   `DealSpec::HoleUp` deals face-up cards: tracked separately from down
//!   cards, emitted as public `DealUp` events.
//! - **Bring-in decision** (first street with a betting round): the worst
//!   door card posts. Because `Card::index = rank*4 + suit` with suit order
//!   clubs < diamonds < hearts < spades, the bring-in seat is simply
//!   min-by-`Card::index` of the door cards for high games (`low: false`)
//!   and max-by-index for razz (`low: true`) — rank first, suit breaking
//!   ties, exactly the standard rule. That seat must act with
//!   `LegalActions { bring_in: Some(min(bring_in, stack)), bet: Some(small
//!   bet bounds, all-in capped), .. }` — no fold/check/call.
//!   `Action::BringIn` posts the bring-in (emitted as a normal `Acted`
//!   event); `Action::Bet { to: small_bet }` "completes" directly.
//! - **Cap accounting**: stud rounds start at `wagers = 0` (the blind-game
//!   "big blind counts as wager 1" rule does NOT apply). A bring-in does
//!   not count toward the cap; a completion (whether the bring-in seat's
//!   direct `Bet` or a later seat's `Raise { to: small_bet }`) is wager 1.
//!   The existing fixed-limit half-bet rule composes: completion over a
//!   half-bet bring-in is `inc == tier/2`, which `is_full_wager` already
//!   accepts. Raises then step by the street tier as usual.
//! - **`FirstToAct::ByUpcards`** (all post-bring-in streets): the best
//!   *showing* hand acts first. Visible strength uses upcard ranks only
//!   (no suits): group ranks — quads > trips > two pair > pair > high
//!   cards — tiebreak by ranks descending; for `low: true` the best A-5
//!   low showing leads (pairs hurt). Ties break by seat order starting
//!   left of the button. If the leader cannot act (all-in), the first
//!   actionable seat clockwise from the leader opens.
//! - Showdown: evaluate down + up cards together (7 cards) via
//!   `best_with_usage(kind, HoleUsage::Any, all_seven, &[])`.
//!   `ShowdownShow.cards` carries all seven.
//! - Stud specs cap seats at 7 so the deck always suffices; the upfront
//!   `cards_needed` check stays.
//!
//! ## Draw streets (`DealSpec::Draw { max }`)
//! - A draw street runs a **draw phase** before its betting round. Every
//!   non-folded seat — including all-in seats — acts exactly once, in seat
//!   order starting left of the button: `LegalActions { draw:
//!   Some(DrawBounds { max_discards: max }), .. }` only (no other family).
//!   `Action::Discard { cards }` must reference distinct cards the seat
//!   actually holds, at most `max`; empty = stand pat. Replacements are
//!   dealt immediately (before the next seat draws): the seat's hand loses
//!   the discards and gains the drawn cards; emit
//!   `DrawResult { seat, discarded, drawn }` (drawn is private).
//! - **Deck exhaustion**: if the deck holds fewer cards than a replacement
//!   request, deal what remains, then reshuffle the muck — every card
//!   discarded earlier this hand plus every folded hand, *excluding* the
//!   drawing seat's own just-discarded cards — into the deck with
//!   `self.rng` and continue.
//!   Only if that still cannot cover the request (pathological) may the
//!   seat's own discards be included. No dedicated event — determinism
//!   comes from the seeded RNG.
//! - After the draw phase, the street's betting round opens as usual. In
//!   run-out situations (fewer than two seats can bet), remaining draw
//!   phases STILL run — all-in players draw to their final hands — while
//!   the betting rounds are skipped.
//! - Draw-game showdown uses `HoleUsage::AllOwn` on the final hand.
//!
//! ## Invariants (must be property-tested)
//! - Chip conservation at `HandEnd`.
//! - `apply(a)` succeeds iff `a` conforms to the last `legal_actions()`.
//! - Every hand terminates (fold-out, or showdown after the last street).
//! - Stacks never go negative; commitments never exceed starting stacks.
//! - No card is ever simultaneously in two live hands / the board (reshuffle
//!   only recycles dead discards).

use super::action::{Action, BetBounds, Chips, DrawBounds, LegalActions, Seat};
use super::event::{Event, PostKind, PotSide};
use super::pot::{PotAward, ShowdownEntry, award_pots, build_pots};
use super::spec::{BettingKind, DealSpec, FirstToAct, ForcedBets, GameSpec};
use crate::card::{Card, Deck};
use crate::eval::best_with_usage;
use crate::rng::Rng64;

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

/// What kind of decision the current street is asking for. Draw streets run
/// a discard phase before their betting round; everything else — including
/// the stud bring-in decision, which is a betting action — is `Betting`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Phase {
    Betting,
    /// `to_act` is the next seat to discard, not to wager.
    Drawing,
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
    /// Used only when a draw street exhausts the deck and the discard pile
    /// must be reshuffled; deterministic because callers derive it from the
    /// match seed.
    rng: Rng64,
    /// Down cards. For stud this is *only* the face-down cards; showdown
    /// concatenates `up`.
    hole: Vec<Vec<Card>>,
    /// Face-up cards per seat (stud); empty for every other family.
    up: Vec<Vec<Card>>,
    /// Cards thrown away on draw streets, recycled when the deck runs dry.
    discards: Vec<Card>,
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
    /// The price (`current_to`) as of each seat's last action this street.
    /// Drives the cumulative reopening rule: the action reopens for a seat
    /// once the price has risen by a full wager's worth since it acted, so
    /// several short all-ins that add up to a full raise reopen it even
    /// though none of them cleared `acted`.
    acted_to: Vec<Chips>,
    street: usize,
    current_to: Chips,
    /// Size of the last full bet/raise this street; also the minimum
    /// opening bet while `current_to == 0`.
    last_raise: Chips,
    /// Fixed-limit wager count for the current round (cap bookkeeping). Also
    /// doubles as "has a full wager been made this street", which is what
    /// distinguishes a pending completion from an ordinary raise.
    wagers: u8,
    phase: Phase,
    /// The seat to act owes the bring-in decision (post or complete).
    bring_in_pending: bool,
    /// Set once the bring-in street has opened, so later stud streets use the
    /// upcard order instead.
    bring_in_done: bool,
    to_act: Option<Seat>,
    over: bool,
    settlement: Option<Settlement>,
    events: Vec<Event>,
}

impl HandState {
    /// Start a hand. `button` indexes into `stacks` (len = seat count).
    /// Deals from `deck`; the deck must hold enough cards for a full
    /// run-out (draw games may exhaust it mid-hand, in which case the
    /// discards are reshuffled with `rng`). Returns the state plus all
    /// events emitted so far (hand start, posts, street 0 deal).
    pub fn new(
        spec: &GameSpec,
        stacks: &[Chips],
        button: Seat,
        hand_no: u64,
        deck: Deck,
        rng: Rng64,
    ) -> Result<(HandState, Vec<Event>), HandError> {
        let seats = stacks.len();
        if seats < *spec.seats.start() as usize || seats > *spec.seats.end() as usize {
            return Err(HandError::BadSeatCount(seats));
        }
        assert!(button < seats, "button seat {button} out of range");
        if stacks.contains(&0) {
            return Err(HandError::BadStacks);
        }
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
            rng,
            hole: vec![Vec::new(); seats],
            up: vec![Vec::new(); seats],
            discards: Vec::new(),
            board: Vec::new(),
            stacks: stacks.to_vec(),
            contrib: vec![0; seats],
            commit: vec![0; seats],
            folded: vec![false; seats],
            all_in: vec![false; seats],
            acted: vec![false; seats],
            acted_to: vec![0; seats],
            street: 0,
            current_to: 0,
            last_raise: spec.stakes.blinds().1.max(1),
            wagers: 0,
            phase: Phase::Betting,
            bring_in_pending: false,
            bring_in_done: false,
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
        if !(state.open_draw() || state.open_round()) {
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
        match self.phase {
            Phase::Drawing => {
                let DealSpec::Draw { max } = self.spec.streets[self.street].deal else {
                    unreachable!("a draw phase only opens on a draw street")
                };
                Some(LegalActions {
                    draw: Some(DrawBounds { max_discards: max }),
                    ..LegalActions::default()
                })
            }
            Phase::Betting if self.bring_in_pending => Some(self.bring_in_actions(seat)),
            Phase::Betting => Some(self.betting_actions(seat)),
        }
    }

    /// The bring-in seat may only post or complete — no fold, check or call.
    fn bring_in_actions(&self, seat: Seat) -> LegalActions {
        let ForcedBets::BringIn { bring_in, .. } = self.spec.forced_bets else {
            unreachable!("a bring-in decision implies bring-in forced bets")
        };
        let stack = self.stacks[seat];
        let completion = self.street_tier().min(stack);
        LegalActions {
            bring_in: Some(bring_in.min(stack)),
            bet: Some(BetBounds {
                min_to: completion,
                max_to: completion,
            }),
            ..LegalActions::default()
        }
    }

    fn betting_actions(&self, seat: Seat) -> LegalActions {
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
            return la;
        }

        let opening = self.current_to == 0;
        // A short all-in that did not reopen the action leaves `acted` set;
        // such a seat may raise again only once the price has climbed a full
        // wager's worth since it last acted (the cumulative rule).
        if !opening && self.acted[seat] && !self.reopened_for(seat) {
            return la;
        }

        let tier = self.street_tier();
        let bounds = match self.spec.betting {
            BettingKind::FixedLimit { raise_cap } => {
                if raise_cap.is_some_and(|cap| self.wagers >= cap) {
                    None
                } else {
                    // `wagers == 0` with chips already out means no full
                    // wager has been made yet — a bring-in, or an all-in
                    // below half a bet. Either way the price is *completed*
                    // to the tier rather than raised by it.
                    let to = if opening || self.wagers == 0 {
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
        la
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
        let seat = self.to_act.expect("legal_actions implies a seat to act");
        if self.phase == Phase::Drawing {
            let Action::Discard { cards } = action else {
                unreachable!("the draw phase offers nothing but `Discard`")
            };
            return self.apply_discard(seat, cards);
        }
        // Everything below this point is infallible: no partial mutation.
        let mut ev = Vec::new();

        match action {
            Action::Fold => {
                self.folded[seat] = true;
                // Folded hands join the muck: in draw games the reshuffle
                // pile is the whole muck (discards + folded hands), matching
                // real-table practice and deepening the pile against
                // exhaustion. Folded seats never reach showdown, so nothing
                // downstream reads their hole cards.
                let mut mucked = core::mem::take(&mut self.hole[seat]);
                self.discards.append(&mut mucked);
            }
            Action::Check => {}
            Action::Call => {
                let pay = (self.current_to - self.commit[seat]).min(self.stacks[seat]);
                self.pay(seat, pay);
            }
            Action::BringIn => {
                let ForcedBets::BringIn { bring_in, .. } = self.spec.forced_bets else {
                    unreachable!("a bring-in decision implies bring-in forced bets")
                };
                self.pay(seat, bring_in.min(self.stacks[seat]));
                // Nominal, not what was paid: a short all-in bring-in never
                // lowers the price for anyone else. The bring-in is not a
                // wager, so `wagers` stays 0 and the cap is untouched.
                self.current_to = bring_in;
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
            Action::Discard { .. } => unreachable!("rejected by `conforms`"),
        }
        self.bring_in_pending = false;
        self.acted[seat] = true;
        self.acted_to[seat] = self.current_to;
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

    /// Hole cards of a seat (unredacted — callers redact via events). For
    /// stud these are the face-down cards only; see [`HandState::upcards`].
    pub fn hole_cards(&self, seat: Seat) -> &[Card] {
        &self.hole[seat]
    }

    /// Face-up cards per seat (empty vecs for games without upcards).
    pub fn upcards(&self) -> &[Vec<Card>] {
        &self.up
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
        let ante = match self.spec.forced_bets {
            ForcedBets::Blinds { ante } | ForcedBets::BringIn { ante, .. } => ante,
        };
        if ante > 0 {
            for i in 1..=self.seats {
                let seat = (self.button + i) % self.seats;
                let amount = ante.min(self.stacks[seat]);
                if amount == 0 {
                    continue;
                }
                // Antes buy pot equity, never street commitment: `commit` is
                // deliberately untouched so call/raise arithmetic ignores them.
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
        if matches!(self.spec.forced_bets, ForcedBets::BringIn { .. }) {
            // Stud rounds open with no wager and no cap consumed; the
            // bring-in itself is posted as an action, not here.
            return;
        }

        let (sb_seat, bb_seat) = self.blind_seats();
        let (small_blind, big_blind) = self.spec.stakes.blinds();
        for (seat, kind, nominal) in [
            (sb_seat, PostKind::SmallBlind, small_blind),
            (bb_seat, PostKind::BigBlind, big_blind),
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
        self.current_to = big_blind;
        self.last_raise = big_blind.max(1);
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
            label: label.to_string(),
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
            DealSpec::HoleUp(count) => {
                for i in 1..=self.seats {
                    let seat = (self.button + i) % self.seats;
                    if self.folded[seat] {
                        continue;
                    }
                    let cards = self
                        .deck
                        .draw_n(count as usize)
                        .expect("deck sized in new()");
                    self.up[seat].extend_from_slice(&cards);
                    ev.push(Event::DealUp { seat, cards });
                }
            }
            // Draw streets deal nothing up front: replacements are dealt one
            // seat at a time during the draw phase (`open_draw`).
            DealSpec::Draw { .. } => {}
        }
    }

    // --- Draw phases ------------------------------------------------------

    /// Opens the draw phase of a draw street. Every non-folded seat draws
    /// exactly once, all-in seats included, starting left of the button.
    fn open_draw(&mut self) -> bool {
        self.phase = Phase::Betting;
        if !matches!(self.spec.streets[self.street].deal, DealSpec::Draw { .. }) {
            return false;
        }
        let Some(first) = self.odd_chip_order().into_iter().find(|&s| !self.folded[s]) else {
            return false;
        };
        self.phase = Phase::Drawing;
        self.to_act = Some(first);
        true
    }

    /// Next seat owing a draw, or `None` when the phase is done. Nobody can
    /// fold during a draw phase, so this order is stable while it runs.
    fn next_drawer(&self, from: Seat) -> Option<Seat> {
        let order: Vec<Seat> = self
            .odd_chip_order()
            .into_iter()
            .filter(|&s| !self.folded[s])
            .collect();
        let pos = order
            .iter()
            .position(|&s| s == from)
            .expect("the drawing seat is never folded");
        order.get(pos + 1).copied()
    }

    fn check_discard(&self, seat: Seat, cards: &[Card]) -> Result<(), &'static str> {
        for (i, card) in cards.iter().enumerate() {
            if cards[..i].contains(card) {
                return Err("the same card cannot be discarded twice");
            }
            if !self.hole[seat].contains(card) {
                return Err("cannot discard a card the seat does not hold");
            }
        }
        Ok(())
    }

    fn apply_discard(&mut self, seat: Seat, cards: Vec<Card>) -> Result<Vec<Event>, ActionError> {
        if let Err(reason) = self.check_discard(seat, &cards) {
            return Err(ActionError::Illegal {
                action: Action::Discard { cards },
                reason,
            });
        }
        // Everything below this point is infallible: no partial mutation.
        for card in &cards {
            let at = self.hole[seat]
                .iter()
                .position(|held| held == card)
                .expect("validated above");
            self.hole[seat].remove(at);
        }
        let drawn = self.draw_replacements(&cards);
        self.hole[seat].extend_from_slice(&drawn);

        let mut ev = vec![Event::DrawResult {
            seat,
            count: cards.len() as u8,
            discarded: cards.clone(),
            drawn,
        }];
        match self.next_drawer(seat) {
            Some(next) => self.to_act = Some(next),
            None => {
                self.phase = Phase::Betting;
                if !self.open_round() {
                    self.advance(&mut ev);
                }
            }
        }
        self.events.extend_from_slice(&ev);
        Ok(ev)
    }

    /// Deal one replacement per discarded card, then retire the discards.
    /// When the deck runs dry mid-request the pile of *earlier* discards is
    /// reshuffled back in, so the drawing seat can never receive a card it
    /// just threw away — unless nothing else is left anywhere, which no real
    /// spec can reach (cards in play + deck + discards is always 52).
    fn draw_replacements(&mut self, discarded: &[Card]) -> Vec<Card> {
        let mut drawn = Vec::with_capacity(discarded.len());
        let mut recycled_own = false;
        while drawn.len() < discarded.len() {
            if let Some(card) = self.deck.draw() {
                drawn.push(card);
                continue;
            }
            if self.discards.is_empty() {
                if recycled_own {
                    break;
                }
                self.discards.extend_from_slice(discarded);
                recycled_own = true;
            }
            let mut pile = core::mem::take(&mut self.discards);
            self.rng.shuffle(&mut pile);
            self.deck = Deck::from_deal_order(&pile);
        }
        if !recycled_own {
            self.discards.extend_from_slice(discarded);
        }
        drawn
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

    /// Fixed-limit wager size for the street currently open for betting.
    fn street_tier(&self) -> Chips {
        self.spec.tier_size(
            self.spec.streets[self.street]
                .betting
                .expect("betting round open")
                .tier,
        )
    }

    /// Does a wager of increment `inc` reopen the action (and count toward
    /// the fixed-limit cap)? The half-bet rule is what makes a stud
    /// completion over a bring-in count as the round's first wager.
    fn is_full_wager(&self, inc: Chips) -> bool {
        match self.spec.betting {
            BettingKind::FixedLimit { .. } => inc * 2 >= self.street_tier(),
            _ => inc >= self.last_raise,
        }
    }

    /// Cumulative reopening: a seat that already acted regains the right to
    /// raise once the price has risen by a full wager's worth since it last
    /// acted. One short all-in never clears that bar (it is short precisely
    /// because it falls under it), but several together can.
    fn reopened_for(&self, seat: Seat) -> bool {
        self.is_full_wager(self.current_to - self.acted_to[seat])
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
            self.acted_to[s] = 0;
        }
        self.current_to = 0;
        self.last_raise = self.spec.stakes.blinds().1.max(1);
        self.wagers = 0;
        self.phase = Phase::Betting;
        self.bring_in_pending = false;
    }

    /// Sets `to_act` for the current street; `false` when no betting round
    /// runs (no spec'd round, or fewer than two seats can act).
    fn open_round(&mut self) -> bool {
        self.to_act = None;
        self.bring_in_pending = false;
        let Some(round) = self.spec.streets[self.street].betting else {
            return false;
        };
        if self.active_count() < 2 {
            return false;
        }
        if !self.bring_in_done && matches!(self.spec.forced_bets, ForcedBets::BringIn { .. }) {
            self.bring_in_done = true;
            self.bring_in_pending = true;
            self.to_act = Some(self.bring_in_seat());
            return true;
        }
        let start = match round.first_to_act {
            FirstToAct::AfterBlinds => (self.blind_seats().1 + 1) % self.seats,
            FirstToAct::LeftOfButton => (self.button + 1) % self.seats,
            // The leader may be all-in; the loop below then hands the open to
            // the first actionable seat clockwise from it.
            FirstToAct::ByUpcards => self.upcard_leader(),
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

    /// Worst door card posts the bring-in: min by `Card::index` (rank first,
    /// suit breaking ties) for high games, max for razz. Seats already all-in
    /// from the ante have no decision to make, so they are not candidates.
    fn bring_in_seat(&self) -> Seat {
        let ForcedBets::BringIn { low, .. } = self.spec.forced_bets else {
            unreachable!("a bring-in street implies bring-in forced bets")
        };
        let door = |&s: &Seat| {
            self.up[s]
                .last()
                .expect("the door card is dealt before the bring-in round opens")
                .index()
        };
        let candidates = (0..self.seats).filter(|&s| !self.folded[s] && !self.all_in[s]);
        if low {
            candidates.max_by_key(door)
        } else {
            candidates.min_by_key(door)
        }
        .expect("active_count() >= 2 was checked")
    }

    /// Seat with the best *showing* hand, ties broken by seat order from
    /// left of the button. All-in seats still count as leaders.
    fn upcard_leader(&self) -> Seat {
        let low = matches!(self.spec.forced_bets, ForcedBets::BringIn { low: true, .. });
        let mut best: Option<(Vec<u8>, Seat)> = None;
        for seat in self.odd_chip_order() {
            if self.folded[seat] {
                continue;
            }
            let key = visible_key(&self.up[seat], low);
            let better = match &best {
                None => true,
                Some((leader, _)) if low => key < *leader,
                Some((leader, _)) => key > *leader,
            };
            if better {
                best = Some((key, seat));
            }
        }
        best.map(|(_, seat)| seat)
            .unwrap_or((self.button + 1) % self.seats)
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
            // Draw phases run even in a run-out: all-in seats still draw to
            // their final hands, only the betting round is skipped.
            if self.open_draw() || self.open_round() {
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
        let hi_side = self.spec.showdown.hi;
        let lo_side = self.spec.showdown.lo;
        let order = self.odd_chip_order();
        let mut entries: Vec<ShowdownEntry> = Vec::new();
        let mut showdown_seats: Vec<Seat> = Vec::new();
        for &seat in &order {
            if self.folded[seat] {
                continue;
            }
            // Stud shows down all seven: the up cards are part of the hand,
            // not of a board. Every other family has an empty `up`.
            let mut hole = self.hole[seat].clone();
            hole.extend_from_slice(&self.up[seat]);
            let hi = best_with_usage(hi_side.kind, hi_side.usage, &hole, &self.board);
            let lo =
                lo_side.and_then(|side| best_with_usage(side.kind, side.usage, &hole, &self.board));
            ev.push(Event::ShowdownShow {
                seat,
                cards: hole,
                hi,
                lo,
            });
            entries.push(ShowdownEntry { seat, hi, lo });
            showdown_seats.push(seat);
        }
        let pots = build_pots(&self.contrib, &self.folded);
        let has_low = lo_side.is_some();
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

/// Visible-strength key for a stud upcard set: rank groups only, suits
/// ignored. Byte 0 is the class (0 high card, 1 pair, 2 two pair, 3 trips,
/// 4 quads), then the distinct ranks by group size and rank, most
/// significant first. Compare with `>` for high games and `<` for razz —
/// pairing raises the class, which is exactly what makes a pair bad for a
/// low hand. `low` switches to ace-low rank indices.
fn visible_key(up: &[Card], low: bool) -> Vec<u8> {
    let mut counts = [0u8; 13];
    for card in up {
        let rank = card.rank().index();
        counts[if low {
            ((rank + 1) % 13) as usize
        } else {
            rank as usize
        }] += 1;
    }
    let mut groups: Vec<(u8, u8)> = (0..13u8)
        .filter(|&r| counts[r as usize] > 0)
        .map(|r| (counts[r as usize], r))
        .collect();
    groups.sort_unstable_by(|a, b| b.cmp(a));
    let pairs = groups.iter().filter(|&&(n, _)| n == 2).count();
    let class = match groups.first().map(|&(n, _)| n) {
        Some(4) => 4,
        Some(3) => 3,
        Some(2) if pairs >= 2 => 2,
        Some(2) => 1,
        _ => 0,
    };
    let mut key = vec![class];
    key.extend(groups.into_iter().map(|(_, rank)| rank));
    key
}

/// Cards the deck must hold up front. Draw streets are excluded: their
/// replacements come out of the discard pile once the deck runs dry, so a
/// worst-case bound would reject perfectly playable games.
fn cards_needed(spec: &GameSpec, seats: usize) -> usize {
    spec.streets
        .iter()
        .map(|street| match street.deal {
            DealSpec::None | DealSpec::Draw { .. } => 0,
            DealSpec::HolePrivate(n) | DealSpec::HoleUp(n) => n as usize * seats,
            DealSpec::Community(n) => n as usize,
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
        Action::BringIn if la.bring_in.is_some() => Ok(()),
        Action::BringIn => Err("no bring-in is owed"),
        Action::Discard { cards } => match la.draw {
            Some(b) if cards.len() <= b.max_discards as usize => Ok(()),
            Some(_) => Err("more cards discarded than the draw allows"),
            None => Err("no draw round is in progress"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::spec::Stakes;

    fn test_rng() -> Rng64 {
        Rng64::from_seed_stream(0, 0)
    }

    fn nl(seats: u8) -> GameSpec {
        let mut spec = GameSpec::holdem_nl(Stakes::Blinds {
            small_blind: 1,
            big_blind: 2,
            ante: 0,
        });
        spec.seats = 2..=seats.max(2);
        spec
    }

    fn start(spec: &GameSpec, stacks: &[Chips]) -> HandState {
        HandState::new(spec, stacks, 0, 1, Deck::standard(), test_rng())
            .unwrap()
            .0
    }

    #[test]
    fn every_registered_spec_starts_a_hand() {
        // The `Unsupported` construction gate is gone: stud and draw specs must all
        // reach a first decision (or settle outright) without erroring.
        for id in GameSpec::known_ids() {
            let spec = GameSpec::by_id(
                id,
                Stakes::Blinds {
                    small_blind: 1,
                    big_blind: 2,
                    ante: 0,
                },
            )
            .unwrap();
            let seats = *spec.seats.start() as usize;
            let stacks = vec![500 as Chips; seats];
            let mut rng = Rng64::from_seed_stream(9, 9);
            let deck = Deck::shuffled(&mut rng);
            let (hand, ev) = HandState::new(&spec, &stacks, 0, 1, deck, test_rng())
                .unwrap_or_else(|e| panic!("{id} failed to start: {e}"));
            assert!(hand.to_act().is_some(), "{id} produced no decision");
            assert!(matches!(ev.first(), Some(Event::HandStart { .. })));
        }
    }

    #[test]
    fn rejects_bad_setup() {
        let spec = nl(9);
        assert_eq!(
            HandState::new(&spec, &[100], 0, 1, Deck::standard(), test_rng()).err(),
            Some(HandError::BadSeatCount(1))
        );
        assert_eq!(
            HandState::new(&spec, &[100, 0], 0, 1, Deck::standard(), test_rng()).err(),
            Some(HandError::BadStacks)
        );
        let short = Deck::from_deal_order(&crate::card::parse_cards("As Ks Qs Js").unwrap());
        assert_eq!(
            HandState::new(&spec, &[100, 100], 0, 1, short, test_rng()).err(),
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
    fn acted_to_records_the_price_at_each_seats_last_action() {
        let mut hand = start(&nl(9), &[200, 9, 12, 200]);
        // Seat 3 opens to 6 (a full raise of 4 over the blind), seat 0 calls.
        hand.apply(Action::Raise { to: 6 }).unwrap();
        hand.apply(Action::Call).unwrap();
        assert_eq!(hand.acted_to, vec![6, 0, 0, 6]);

        // Two all-ins of 3 each: neither is a full raise on its own, so
        // neither clears `acted`, but together they add up to one.
        hand.apply(Action::Raise { to: 9 }).unwrap();
        hand.apply(Action::Raise { to: 12 }).unwrap();
        assert_eq!(hand.acted_to, vec![6, 9, 12, 6]);
        assert!(hand.acted[3] && hand.reopened_for(3));
        assert_eq!(
            hand.legal_actions().unwrap().raise,
            Some(BetBounds {
                min_to: 16,
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

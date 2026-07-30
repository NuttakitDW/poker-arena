//! Behavioral statistics: VPIP, PFR, aggression factor, and showdown rates.
//!
//! Unlike [`crate::stat::RateStats`] (which tracks *winnings*), this module
//! tracks *how* a bot plays, folded into one accumulator per bot over a
//! match. [`BehaviorStats::record_hand`] consumes a single hand's unredacted
//! event stream — the same stream an [`crate::log::EventSink`] would see —
//! plus the seat this bot occupied and its net result for the hand.

use poker_core::game::{Action, Event, Seat};

/// Per-bot behavioral profile accumulated over a match.
///
/// All rate accessors return `0.0` on an empty accumulator (no hands
/// recorded yet) rather than `NaN`, so a freshly-built `BehaviorStats` prints
/// cleanly.
#[derive(Clone, Debug, Default)]
pub struct BehaviorStats {
    hands: u64,
    vpip_hands: u64,
    pfr_hands: u64,
    /// Cumulative Bet+Raise actions, every street, every hand.
    aggressive_actions: u64,
    /// Cumulative Call actions, every street, every hand.
    call_actions: u64,
    showdown_hands: u64,
    showdown_wins: u64,
    fold_hands: u64,
}

impl BehaviorStats {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold one hand's unredacted event stream into the accumulator.
    ///
    /// `seat` is the seat this bot occupied *for this hand* (seats rotate
    /// hand to hand — see [`crate::runner::run_match`]); `net` is this bot's
    /// net chip result for the hand (used for `wsd`).
    ///
    /// "Street 0" (the first betting round) is derived from the stream
    /// itself, not hardcoded: it's whichever street was in effect (per the
    /// most recent [`Event::StreetStart`]) when the hand's first
    /// [`Event::Acted`] occurred. This matters for stud, whose opening deal
    /// carries no betting round — the first `Acted` (the forced bring-in)
    /// lands on a later street index.
    pub fn record_hand(&mut self, events: &[Event], seat: Seat, net: i64) {
        self.hands += 1;

        let mut current_street: Option<u8> = None;
        let mut first_acted_street: Option<u8> = None;
        let mut voluntary_street0 = false;
        let mut pfr_street0 = false;
        let mut aggressive = 0u64;
        let mut calls = 0u64;
        let mut folded = false;
        let mut showdown = false;

        for event in events {
            match event {
                Event::StreetStart { street, .. } => current_street = Some(*street),
                Event::Acted {
                    seat: actor,
                    action,
                    ..
                } => {
                    if first_acted_street.is_none() {
                        first_acted_street = current_street;
                    }
                    if *actor != seat {
                        continue;
                    }
                    let is_street0 =
                        current_street.is_some() && current_street == first_acted_street;
                    match action {
                        Action::Call => {
                            calls += 1;
                            voluntary_street0 |= is_street0;
                        }
                        Action::Bet { .. } | Action::Raise { .. } => {
                            aggressive += 1;
                            voluntary_street0 |= is_street0;
                            pfr_street0 |= is_street0;
                        }
                        Action::Fold => folded = true,
                        // Forced (BringIn) and neither-call-nor-aggression
                        // (Check, Discard) — none of these count as
                        // voluntary or aggressive action.
                        Action::Check | Action::BringIn | Action::Discard { .. } => {}
                    }
                }
                Event::ShowdownShow { seat: s, .. } if *s == seat => showdown = true,
                _ => {}
            }
        }

        self.vpip_hands += voluntary_street0 as u64;
        self.pfr_hands += pfr_street0 as u64;
        self.aggressive_actions += aggressive;
        self.call_actions += calls;
        if folded {
            self.fold_hands += 1;
        }
        if showdown {
            self.showdown_hands += 1;
            if net > 0 {
                self.showdown_wins += 1;
            }
        }
    }

    /// Number of hands folded in.
    pub fn hands(&self) -> u64 {
        self.hands
    }

    /// Fraction of hands with a voluntary Call/Bet/Raise on street 0.
    pub fn vpip(&self) -> f64 {
        ratio(self.vpip_hands, self.hands)
    }

    /// Fraction of hands with a street-0 Bet or Raise.
    pub fn pfr(&self) -> f64 {
        ratio(self.pfr_hands, self.hands)
    }

    /// Aggression factor: (Bet+Raise) / Call across every street of every
    /// hand. `f64::INFINITY` when there are aggressive actions but no calls;
    /// `0.0` when there are neither.
    pub fn af(&self) -> f64 {
        if self.call_actions == 0 {
            if self.aggressive_actions == 0 {
                0.0
            } else {
                f64::INFINITY
            }
        } else {
            self.aggressive_actions as f64 / self.call_actions as f64
        }
    }

    /// Fraction of hands that reached showdown (this seat appeared in a
    /// [`Event::ShowdownShow`]).
    pub fn wtsd(&self) -> f64 {
        ratio(self.showdown_hands, self.hands)
    }

    /// Fraction of showdown hands won (net > 0). `0.0` when there were no
    /// showdowns.
    pub fn wsd(&self) -> f64 {
        ratio(self.showdown_wins, self.showdown_hands)
    }

    /// Fraction of hands folded.
    pub fn fold_rate(&self) -> f64 {
        ratio(self.fold_hands, self.hands)
    }
}

/// `num / den` as a fraction, `0.0` when `den == 0` rather than `NaN`.
fn ratio(num: u64, den: u64) -> f64 {
    if den == 0 {
        0.0
    } else {
        num as f64 / den as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn street_start(street: u8) -> Event {
        Event::StreetStart {
            street,
            label: "test",
        }
    }

    fn acted(seat: Seat, action: Action) -> Event {
        Event::Acted {
            seat,
            action,
            street_commit: 0,
            all_in: false,
        }
    }

    fn showdown(seat: Seat) -> Event {
        Event::ShowdownShow {
            seat,
            cards: Vec::new(),
            hi: None,
            lo: None,
        }
    }

    // ---- VPIP ----

    #[test]
    fn vpip_counts_call_but_not_blind_posts() {
        // Posts are separate events and never examined; the hero's own call
        // on street 0 is what should trip vpip.
        let events = vec![
            Event::Post {
                seat: 0,
                kind: poker_core::game::PostKind::SmallBlind,
                amount: 50,
                all_in: false,
            },
            Event::Post {
                seat: 1,
                kind: poker_core::game::PostKind::BigBlind,
                amount: 100,
                all_in: false,
            },
            street_start(0),
            acted(0, Action::Call),
            acted(1, Action::Check),
        ];
        let mut bs = BehaviorStats::new();
        bs.record_hand(&events, 0, -50);
        assert_eq!(bs.hands(), 1);
        assert_eq!(bs.vpip(), 1.0);
        assert_eq!(bs.pfr(), 0.0);
    }

    #[test]
    fn blind_check_through_hand_has_zero_vpip() {
        let events = vec![
            Event::Post {
                seat: 0,
                kind: poker_core::game::PostKind::SmallBlind,
                amount: 50,
                all_in: false,
            },
            Event::Post {
                seat: 1,
                kind: poker_core::game::PostKind::BigBlind,
                amount: 100,
                all_in: false,
            },
            street_start(0),
            acted(0, Action::Call),
            acted(1, Action::Check),
        ];
        let mut bs = BehaviorStats::new();
        // Seat 1 only checked — never a Call/Bet/Raise.
        bs.record_hand(&events, 1, 50);
        assert_eq!(bs.vpip(), 0.0);
        assert_eq!(bs.fold_rate(), 0.0);
    }

    #[test]
    fn raise_counts_as_both_vpip_and_pfr() {
        let events = vec![
            street_start(0),
            acted(0, Action::Raise { to: 300 }),
            acted(1, Action::Fold),
        ];
        let mut bs = BehaviorStats::new();
        bs.record_hand(&events, 0, 150);
        assert_eq!(bs.vpip(), 1.0);
        assert_eq!(bs.pfr(), 1.0);
    }

    #[test]
    fn bring_in_excluded_from_vpip() {
        // Stud: street 0 is the antes/upcard deal (no Acted), street 1 opens
        // with the forced bring-in.
        let events = vec![
            street_start(0),
            street_start(1),
            acted(0, Action::BringIn),
            acted(1, Action::Call),
        ];
        let mut bs = BehaviorStats::new();
        bs.record_hand(&events, 0, -10);
        assert_eq!(bs.vpip(), 0.0, "a forced bring-in must not count as vpip");
        assert_eq!(bs.pfr(), 0.0);

        let mut bs1 = BehaviorStats::new();
        bs1.record_hand(&events, 1, 10);
        assert_eq!(
            bs1.vpip(),
            1.0,
            "a voluntary call on the same street still counts"
        );
    }

    #[test]
    fn stud_first_betting_street_is_detected_not_hardcoded() {
        // StreetStart(0) with no Acted at all (deal-only street), then
        // StreetStart(1) carries the first Acted — first_acted_street must
        // resolve to 1, not 0. A raise on street 1 must count as pfr.
        let events = vec![
            street_start(0),
            street_start(1),
            acted(0, Action::BringIn),
            acted(1, Action::Raise { to: 40 }),
        ];
        let mut bs = BehaviorStats::new();
        bs.record_hand(&events, 1, 20);
        assert_eq!(bs.vpip(), 1.0);
        assert_eq!(bs.pfr(), 1.0);

        // A hypothetical raise landing on a later street (say 2) must NOT
        // count toward pfr, only toward af.
        let events2 = vec![
            street_start(0),
            street_start(1),
            acted(0, Action::BringIn),
            acted(1, Action::Call),
            street_start(2),
            acted(0, Action::Raise { to: 80 }),
        ];
        let mut bs2 = BehaviorStats::new();
        bs2.record_hand(&events2, 0, -40);
        assert_eq!(bs2.vpip(), 0.0, "bring-in alone is not voluntary");
        assert_eq!(bs2.pfr(), 0.0, "the raise happened after street 0");
    }

    // ---- AF ----

    #[test]
    fn af_math_including_inf_and_zero() {
        let mut bs = BehaviorStats::new();
        bs.record_hand(
            &[
                street_start(0),
                acted(0, Action::Bet { to: 100 }),
                acted(1, Action::Call),
                street_start(1),
                acted(0, Action::Raise { to: 300 }),
                acted(1, Action::Call),
            ],
            0,
            200,
        );
        // 2 aggressive actions, 0 calls by seat 0.
        assert_eq!(bs.af(), f64::INFINITY);

        let mut bs_caller = BehaviorStats::new();
        bs_caller.record_hand(
            &[
                street_start(0),
                acted(0, Action::Bet { to: 100 }),
                acted(1, Action::Call),
            ],
            1,
            -100,
        );
        // seat 1: 0 aggressive, 1 call -> af = 0/1 = 0.0
        assert_eq!(bs_caller.af(), 0.0);

        let mut bs_neither = BehaviorStats::new();
        bs_neither.record_hand(&[street_start(0), acted(0, Action::Check)], 1, 0);
        // seat 1 does nothing at all -> 0/0 -> 0.0
        assert_eq!(bs_neither.af(), 0.0);

        let mut bs_mixed = BehaviorStats::new();
        bs_mixed.record_hand(
            &[
                street_start(0),
                acted(0, Action::Bet { to: 100 }),
                acted(1, Action::Call),
                street_start(1),
                acted(0, Action::Check),
                acted(1, Action::Bet { to: 50 }),
                acted(0, Action::Call),
            ],
            0,
            -50,
        );
        // seat 0: aggressive=1 (street0 bet), calls=1 (street1 call) -> 1/1
        assert_eq!(bs_mixed.af(), 1.0);
    }

    // ---- WTSD / WSD ----

    #[test]
    fn wtsd_and_wsd_track_showdown_appearance_and_net_sign() {
        let mut bs = BehaviorStats::new();
        // Hand 1: seat 0 reaches showdown and wins.
        bs.record_hand(
            &[street_start(0), acted(0, Action::Check), showdown(0)],
            0,
            100,
        );
        // Hand 2: seat 0 reaches showdown and loses.
        bs.record_hand(
            &[street_start(0), acted(0, Action::Check), showdown(0)],
            0,
            -100,
        );
        // Hand 3: seat 0 folds, never reaches showdown.
        bs.record_hand(&[street_start(0), acted(0, Action::Fold)], 0, -50);

        assert_eq!(bs.hands(), 3);
        assert_eq!(bs.wtsd(), 2.0 / 3.0);
        assert_eq!(bs.wsd(), 0.5);
        assert_eq!(bs.fold_rate(), 1.0 / 3.0);
    }

    #[test]
    fn wsd_is_zero_with_no_showdowns() {
        let mut bs = BehaviorStats::new();
        bs.record_hand(&[street_start(0), acted(0, Action::Fold)], 0, -50);
        assert_eq!(bs.wtsd(), 0.0);
        assert_eq!(bs.wsd(), 0.0);
    }

    // ---- fold_rate ----

    #[test]
    fn fold_rate_counts_any_street_fold() {
        let mut bs = BehaviorStats::new();
        bs.record_hand(
            &[
                street_start(0),
                acted(0, Action::Call),
                street_start(1),
                acted(0, Action::Fold),
            ],
            0,
            -50,
        );
        assert_eq!(bs.fold_rate(), 1.0);
        assert_eq!(bs.vpip(), 1.0, "the street-0 call still counts");
    }

    // ---- empty accumulator ----

    #[test]
    fn empty_accumulator_reports_zero_not_nan() {
        let bs = BehaviorStats::new();
        assert_eq!(bs.hands(), 0);
        assert_eq!(bs.vpip(), 0.0);
        assert_eq!(bs.pfr(), 0.0);
        assert_eq!(bs.af(), 0.0);
        assert_eq!(bs.wtsd(), 0.0);
        assert_eq!(bs.wsd(), 0.0);
        assert_eq!(bs.fold_rate(), 0.0);
    }
}

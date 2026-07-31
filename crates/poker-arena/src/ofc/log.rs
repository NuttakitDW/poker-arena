//! OFC hand-history logging.
//!
//! The mirror of [`crate::log`] for the OFC event stream: the runner streams
//! the *unredacted* events to an [`OfcEventSink`], bracketed by
//! [`OfcEventSink::hand_start`] / [`OfcEventSink::hand_end`] per hand, with
//! [`OfcEventSink::finish`] called once at the very end. [`OfcJsonLog`]
//! writes every hand; [`OfcSelectiveLog`] keeps a chosen subset (the first N
//! hands, the biggest point swings, fault evidence) and writes them once the
//! match is over.
//!
//! An OFC match has no deck grouping — one hand, one deck, no duplicate
//! rotations — so headers carry no deck number and sampling is a plain
//! first-N rather than whole rotation sets.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};
use std::io::Write;

use poker_core::ofc::OfcEvent;

/// Hand-boundary metadata the event stream deliberately does not carry.
#[derive(Debug, Clone)]
pub struct OfcHandMeta {
    /// Net points by seat, summing to zero. All zeroes for a forfeited hand,
    /// which never settled.
    pub points: Vec<i64>,
    /// Seats whose bot faulted during this hand, ascending.
    pub faulted: Vec<usize>,
    /// The hand was cut short by a forfeit (match ended here).
    pub forfeited: bool,
}

impl OfcHandMeta {
    /// The hand's swing: the largest absolute per-seat result. The OFC
    /// counterpart of pot size as a "was this hand interesting" measure.
    pub fn swing(&self) -> u64 {
        self.points
            .iter()
            .map(|points| points.unsigned_abs())
            .max()
            .unwrap_or(0)
    }
}

/// Receives the unredacted OFC event stream as a match plays out.
pub trait OfcEventSink {
    /// Opens a hand boundary; `hand_no` matches the events that follow until
    /// the next `hand_end`. `seats[s]` is the bot name sitting at seat `s`
    /// for this hand.
    fn hand_start(&mut self, hand_no: u64, seats: &[String]);

    /// One engine event, in the order the engine produced it.
    fn event(&mut self, ev: &OfcEvent);

    /// Closes the current hand boundary.
    fn hand_end(&mut self, meta: &OfcHandMeta);

    /// Called once when the match is over; buffered sinks write here.
    fn finish(&mut self) {}
}

/// One logged line: the hand it belongs to, plus the engine event.
#[derive(serde::Serialize)]
struct LogLine<'a> {
    hand: u64,
    ev: &'a OfcEvent,
}

/// The header line opening a hand in [`OfcJsonLog`]: `{"hand":N,
/// "seats":[...]}`.
#[derive(serde::Serialize)]
struct HandHeader<'a> {
    hand: u64,
    seats: &'a [String],
}

/// The header line opening a hand in [`OfcSelectiveLog`]'s output: like
/// [`HandHeader`], plus the reasons the hand was kept.
#[derive(serde::Serialize)]
struct SelectiveHandHeader<'a> {
    hand: u64,
    seats: &'a [String],
    kept: Vec<&'static str>,
}

/// Trailing summary line, wrapped under a `"log_summary"` key.
#[derive(serde::Serialize)]
struct LogSummaryLine<T> {
    log_summary: T,
}

/// [`OfcJsonLog`]'s summary: every hand is kept, so the two counts match.
#[derive(serde::Serialize)]
struct FullLogSummary {
    hands_seen: u64,
    hands_kept: u64,
}

/// [`OfcSelectiveLog`]'s summary: echoes the selection knobs alongside
/// counts.
#[derive(serde::Serialize)]
struct SelectiveLogSummary {
    hands_seen: u64,
    hands_kept: u64,
    sample_first_hands: Option<u64>,
    top_swings: Option<usize>,
    fault_hands_kept: u64,
}

/// Writes one JSON object per line: a `{"hand":N,"seats":[...]}` header at
/// the start of each hand, then one `{"hand":N,"ev":<OfcEvent>}` line per
/// event, and finally — once, from [`OfcEventSink::finish`] — a trailing
/// `{"log_summary":{"hands_seen":H,"hands_kept":H}}` line (every hand is
/// kept in full-log mode, so the two counts always match).
///
/// `hand_end` flushes, so a completed hand is always fully on disk even if
/// the process is killed mid-match.
pub struct OfcJsonLog<W: Write> {
    out: W,
    hand_no: u64,
    hands_seen: u64,
}

impl<W: Write> OfcJsonLog<W> {
    pub fn new(out: W) -> Self {
        Self {
            out,
            hand_no: 0,
            hands_seen: 0,
        }
    }
}

impl<W: Write> OfcEventSink for OfcJsonLog<W> {
    fn hand_start(&mut self, hand_no: u64, seats: &[String]) {
        self.hand_no = hand_no;
        let header = HandHeader {
            hand: hand_no,
            seats,
        };
        // See `event` below: a serialization/write failure here has no
        // useful recovery, so the line is dropped rather than aborting the
        // match.
        if let Ok(json) = serde_json::to_string(&header) {
            let _ = writeln!(self.out, "{json}");
        }
    }

    fn event(&mut self, ev: &OfcEvent) {
        let line = LogLine {
            hand: self.hand_no,
            ev,
        };
        // A serialization or write failure here would mean a broken pipe or
        // a full disk; there is no useful recovery for a hand-history sink,
        // so drop the line rather than aborting the match it is watching.
        if let Ok(json) = serde_json::to_string(&line) {
            let _ = writeln!(self.out, "{json}");
        }
    }

    fn hand_end(&mut self, _meta: &OfcHandMeta) {
        self.hands_seen += 1;
        let _ = self.out.flush();
    }

    fn finish(&mut self) {
        let summary = LogSummaryLine {
            log_summary: FullLogSummary {
                hands_seen: self.hands_seen,
                hands_kept: self.hands_seen,
            },
        };
        if let Ok(json) = serde_json::to_string(&summary) {
            let _ = writeln!(self.out, "{json}");
        }
        let _ = self.out.flush();
    }
}

/// Which hands [`OfcSelectiveLog`] keeps.
pub struct OfcLogSelection {
    /// Keep the first N hands. `None` = off; `Some(0)` is invalid (validated
    /// by the CLI, not here).
    pub sample_first_hands: Option<u64>,
    /// Keep the K hands with the largest swing — the biggest absolute
    /// per-seat result (global top K, single-pass min-heap).
    pub top_swings: Option<usize>,
    /// Keep the first K hands in which any bot faulted. Forfeit hands are
    /// always kept regardless of this cap.
    pub fault_hands: u64,
}

/// One hand buffered between `hand_start` and `hand_end`: header info plus
/// the unredacted event stream. Held until the keep/drop decision is made,
/// and — for hands that are kept — until [`OfcSelectiveLog::finish`] writes
/// it out; dropped hands free their buffer immediately.
#[derive(Clone)]
struct BufferedHand {
    hand_no: u64,
    seats: Vec<String>,
    events: Vec<OfcEvent>,
}

/// One candidate in the top-swings min-heap: ordered so the heap's max
/// (`peek`) is always the weakest kept entry — smallest swing, ties broken
/// in favor of evicting the *larger* hand number (so lower hand numbers win
/// ties, deterministically).
struct TopEntry {
    swing: u64,
    hand: BufferedHand,
}

impl PartialEq for TopEntry {
    fn eq(&self, other: &Self) -> bool {
        self.swing == other.swing && self.hand.hand_no == other.hand.hand_no
    }
}

impl Eq for TopEntry {}

impl Ord for TopEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reversed on swing: the *smaller* swing ranks higher (is more
        // eligible for eviction). Tied swings break on hand_no descending,
        // so the higher hand_no — the one we'd rather evict — ranks higher.
        other
            .swing
            .cmp(&self.swing)
            .then(self.hand.hand_no.cmp(&other.hand.hand_no))
    }
}

impl PartialOrd for TopEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Keeps only a chosen subset of hands: the first N, the biggest swings, and
/// fault evidence. Nothing is written until [`OfcEventSink::finish`] — the
/// full match must be seen before "biggest swings" and "first K faults" can
/// be decided — at which point every kept hand is written in ascending
/// hand_no order as a header line
/// (`{"hand":N,"seats":[...],"kept":[...]}`) followed by its event lines,
/// then a trailing `{"log_summary":{...}}` line.
///
/// Memory stays O(sample-window + top_swings + fault_hands): every hand is
/// buffered only between its own `hand_start`/`hand_end`, dropped
/// immediately unless it is kept for a reason or is a top-swings candidate.
pub struct OfcSelectiveLog<W: Write> {
    out: W,
    selection: OfcLogSelection,
    hands_seen: u64,
    fault_hands_kept: u64,
    /// Hands kept by the first-N sample so far.
    sample_hands_kept: u64,
    current: Option<BufferedHand>,
    /// hand_no -> (buffered hand, reasons it was kept). A `BTreeMap` so
    /// `finish` can iterate in ascending hand_no order for free.
    kept: BTreeMap<u64, (BufferedHand, BTreeSet<&'static str>)>,
    /// Single-pass top-K min-heap over swing, independent of `kept` until
    /// merged in `finish`.
    top_heap: BinaryHeap<TopEntry>,
}

impl<W: Write> OfcSelectiveLog<W> {
    pub fn new(out: W, selection: OfcLogSelection) -> Self {
        Self {
            out,
            selection,
            hands_seen: 0,
            fault_hands_kept: 0,
            sample_hands_kept: 0,
            current: None,
            kept: BTreeMap::new(),
            top_heap: BinaryHeap::new(),
        }
    }

    /// Considers `hand` (whose swing is `swing`) for the top-K set, evicting
    /// the current weakest member if `hand` beats it.
    fn consider_top_swing(&mut self, k: usize, swing: u64, hand: BufferedHand) {
        if k == 0 {
            return;
        }
        let entry = TopEntry { swing, hand };
        if self.top_heap.len() < k {
            self.top_heap.push(entry);
        } else if self.top_heap.peek().is_some_and(|worst| entry < *worst) {
            self.top_heap.pop();
            self.top_heap.push(entry);
        }
    }
}

impl<W: Write> OfcEventSink for OfcSelectiveLog<W> {
    fn hand_start(&mut self, hand_no: u64, seats: &[String]) {
        self.current = Some(BufferedHand {
            hand_no,
            seats: seats.to_vec(),
            events: Vec::new(),
        });
    }

    fn event(&mut self, ev: &OfcEvent) {
        if let Some(hand) = &mut self.current {
            hand.events.push(ev.clone());
        }
    }

    fn hand_end(&mut self, meta: &OfcHandMeta) {
        self.hands_seen += 1;
        let Some(hand) = self.current.take() else {
            return;
        };
        let hand_no = hand.hand_no;

        let mut reasons: BTreeSet<&'static str> = BTreeSet::new();
        if let Some(n) = self.selection.sample_first_hands
            && self.sample_hands_kept < n
        {
            self.sample_hands_kept += 1;
            reasons.insert("sample");
        }
        if !meta.faulted.is_empty() && self.fault_hands_kept < self.selection.fault_hands {
            reasons.insert("fault");
            self.fault_hands_kept += 1;
        }
        if meta.forfeited {
            reasons.insert("forfeit");
        }

        // Every hand is a top-swings candidate, even one already kept for
        // another reason — so a kept hand needs a clone to feed the heap
        // (the heap owns its own buffer, merged back in `finish`). A hand
        // kept for no other reason is moved into the heap consideration
        // directly, and dropped there and then if it doesn't make the cut.
        let top_candidate = if reasons.is_empty() {
            Some(hand)
        } else {
            let clone = hand.clone();
            self.kept.insert(hand_no, (hand, reasons));
            Some(clone)
        };

        if let (Some(k), Some(candidate)) = (self.selection.top_swings, top_candidate) {
            self.consider_top_swing(k, meta.swing(), candidate);
        }
    }

    fn finish(&mut self) {
        for entry in self.top_heap.drain() {
            let TopEntry { hand, .. } = entry;
            match self.kept.get_mut(&hand.hand_no) {
                Some((_, reasons)) => {
                    reasons.insert("top-swing");
                }
                None => {
                    let mut reasons = BTreeSet::new();
                    reasons.insert("top-swing");
                    self.kept.insert(hand.hand_no, (hand, reasons));
                }
            }
        }

        for (hand, reasons) in self.kept.values() {
            let header = SelectiveHandHeader {
                hand: hand.hand_no,
                seats: &hand.seats,
                kept: reasons.iter().copied().collect(),
            };
            if let Ok(json) = serde_json::to_string(&header) {
                let _ = writeln!(self.out, "{json}");
            }
            for ev in &hand.events {
                let line = LogLine {
                    hand: hand.hand_no,
                    ev,
                };
                if let Ok(json) = serde_json::to_string(&line) {
                    let _ = writeln!(self.out, "{json}");
                }
            }
        }

        let summary = LogSummaryLine {
            log_summary: SelectiveLogSummary {
                hands_seen: self.hands_seen,
                hands_kept: self.kept.len() as u64,
                sample_first_hands: self.selection.sample_first_hands,
                top_swings: self.selection.top_swings,
                fault_hands_kept: self.fault_hands_kept,
            },
        };
        if let Ok(json) = serde_json::to_string(&summary) {
            let _ = writeln!(self.out, "{json}");
        }
        let _ = self.out.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn meta(points: Vec<i64>, faulted: Vec<usize>, forfeited: bool) -> OfcHandMeta {
        OfcHandMeta {
            points,
            faulted,
            forfeited,
        }
    }

    fn seats() -> Vec<String> {
        vec!["a".to_string(), "b".to_string()]
    }

    fn parse_lines(buf: Vec<u8>) -> Vec<Value> {
        let text = String::from_utf8(buf).unwrap();
        text.lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn push_hand<W: Write>(
        log: &mut OfcSelectiveLog<W>,
        hand_no: u64,
        swing: i64,
        faulted: bool,
        forfeited: bool,
    ) {
        log.hand_start(hand_no, &seats());
        log.event(&OfcEvent::Score {
            seat: 0,
            points: swing,
        });
        log.hand_end(&meta(
            vec![swing, -swing],
            if faulted { vec![0] } else { Vec::new() },
            forfeited,
        ));
    }

    #[test]
    fn full_log_writes_headers_events_and_summary() {
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut log = OfcJsonLog::new(&mut buf);
            log.hand_start(0, &seats());
            log.event(&OfcEvent::Fantasyland { seat: 1, cards: 14 });
            log.event(&OfcEvent::Score { seat: 0, points: 6 });
            log.hand_end(&meta(vec![6, -6], Vec::new(), false));
            log.hand_start(1, &seats());
            log.event(&OfcEvent::Score {
                seat: 0,
                points: -6,
            });
            log.hand_end(&meta(vec![-6, 6], Vec::new(), false));
            log.finish();
        }

        let parsed = parse_lines(buf);
        assert_eq!(parsed.len(), 6, "2 headers + 3 events + summary");
        assert_eq!(parsed[0]["hand"], 0);
        assert_eq!(parsed[0]["seats"], serde_json::json!(["a", "b"]));
        assert!(parsed[0].get("deck").is_none(), "OFC has no deck concept");
        assert_eq!(parsed[1]["ev"]["event"], "fantasyland");
        assert_eq!(parsed[3]["hand"], 1);
        assert_eq!(parsed[5]["log_summary"]["hands_seen"], 2);
        assert_eq!(parsed[5]["log_summary"]["hands_kept"], 2);
    }

    #[test]
    fn nothing_written_until_finish() {
        let mut buf: Vec<u8> = Vec::new();
        let mut log = OfcSelectiveLog::new(
            &mut buf,
            OfcLogSelection {
                sample_first_hands: Some(u64::MAX),
                top_swings: None,
                fault_hands: 0,
            },
        );
        push_hand(&mut log, 0, 3, false, false);
        assert!(buf.is_empty(), "nothing should be written pre-finish");
    }

    #[test]
    fn sample_keeps_exactly_the_first_n_hands() {
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut log = OfcSelectiveLog::new(
                &mut buf,
                OfcLogSelection {
                    sample_first_hands: Some(2),
                    top_swings: None,
                    fault_hands: 0,
                },
            );
            for hand_no in 0..5 {
                push_hand(&mut log, hand_no, 1, false, false);
            }
            log.finish();
        }
        let parsed = parse_lines(buf);
        let headers: Vec<&Value> = parsed.iter().filter(|v| v.get("kept").is_some()).collect();
        assert_eq!(headers.len(), 2);
        assert_eq!(headers[0]["hand"], 0);
        assert_eq!(headers[1]["hand"], 1);
        for header in &headers {
            assert_eq!(header["kept"], serde_json::json!(["sample"]));
        }
    }

    #[test]
    fn top_swings_keeps_the_k_biggest_with_deterministic_tie_break() {
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut log = OfcSelectiveLog::new(
                &mut buf,
                OfcLogSelection {
                    sample_first_hands: None,
                    top_swings: Some(2),
                    fault_hands: 0,
                },
            );
            // Swings 3, 30, -30, 20 at hands 0..3: the two 30s win, and the
            // tie between hands 1 and 2 goes to the lower hand number.
            push_hand(&mut log, 0, 3, false, false);
            push_hand(&mut log, 1, 30, false, false);
            push_hand(&mut log, 2, -30, false, false);
            push_hand(&mut log, 3, 20, false, false);
            log.finish();
        }
        let parsed = parse_lines(buf);
        let headers: Vec<&Value> = parsed.iter().filter(|v| v.get("kept").is_some()).collect();
        assert_eq!(headers.len(), 2);
        assert_eq!(headers[0]["hand"], 1);
        assert_eq!(headers[1]["hand"], 2);
        for header in &headers {
            assert_eq!(header["kept"], serde_json::json!(["top-swing"]));
        }
        assert_eq!(
            parsed.last().unwrap()["log_summary"]["hands_seen"],
            4,
            "every hand is still counted"
        );
    }

    #[test]
    fn fault_cap_keeps_only_the_first_k_and_forfeit_is_always_kept() {
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut log = OfcSelectiveLog::new(
                &mut buf,
                OfcLogSelection {
                    sample_first_hands: None,
                    top_swings: None,
                    fault_hands: 1,
                },
            );
            push_hand(&mut log, 0, 1, true, false);
            push_hand(&mut log, 1, 1, true, false);
            push_hand(&mut log, 2, 1, true, true);
            log.finish();
        }
        let parsed = parse_lines(buf);
        let headers: Vec<&Value> = parsed.iter().filter(|v| v.get("kept").is_some()).collect();
        assert_eq!(headers.len(), 2, "one fault hand plus the forfeit");
        assert_eq!(headers[0]["kept"], serde_json::json!(["fault"]));
        assert_eq!(headers[1]["kept"], serde_json::json!(["forfeit"]));
        assert_eq!(parsed.last().unwrap()["log_summary"]["fault_hands_kept"], 1);
    }

    #[test]
    fn union_of_reasons_is_tagged_on_one_header() {
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut log = OfcSelectiveLog::new(
                &mut buf,
                OfcLogSelection {
                    sample_first_hands: Some(u64::MAX),
                    top_swings: Some(1),
                    fault_hands: 0,
                },
            );
            push_hand(&mut log, 0, 5, false, false);
            push_hand(&mut log, 1, 1, false, false);
            log.finish();
        }
        let parsed = parse_lines(buf);
        let headers: Vec<&Value> = parsed.iter().filter(|v| v.get("kept").is_some()).collect();
        assert_eq!(headers.len(), 2);
        assert_eq!(
            headers[0]["kept"],
            serde_json::json!(["sample", "top-swing"])
        );
        assert_eq!(headers[1]["kept"], serde_json::json!(["sample"]));
    }
}

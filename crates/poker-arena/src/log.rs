//! Hand-history logging.
//!
//! The runner streams the *unredacted* event sequence to an [`EventSink`] as
//! a match plays out, bracketed by [`EventSink::hand_start`] /
//! [`EventSink::hand_end`] per hand, with [`EventSink::finish`] called once
//! at the very end. [`JsonLog`] is the reference implementation: one JSON
//! object per line, suitable for `tail -f` or line-by-line replay tooling.
//! [`SelectiveLog`] keeps only a chosen subset of hands (sampled rotation
//! sets, the biggest pots, fault evidence) and writes them once the match is
//! over — useful when logging every hand of a long match is too much data.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};
use std::io::Write;

use poker_core::game::Event;

/// Hand-boundary metadata the event stream deliberately does not carry.
#[derive(Debug, Clone)]
pub struct HandMeta {
    /// Total chips in the pot at settlement (post-refund).
    pub pot_total: u64,
    /// Any bot faulted during this hand.
    pub faulted: bool,
    /// The hand was cut short by a forfeit (match ended here).
    pub forfeited: bool,
}

/// Receives the unredacted event stream as a match plays out.
pub trait EventSink {
    /// Opens a hand boundary; `hand_no` matches the events that follow until
    /// the next `hand_end`. `deck_no` groups duplicate rotations of the
    /// same deck together; `seats[s]` is the bot name sitting at seat `s`
    /// for this hand.
    fn hand_start(&mut self, hand_no: u64, deck_no: u64, seats: &[String]);

    /// One engine event, in the order the engine produced it.
    fn event(&mut self, ev: &Event);

    /// Closes the current hand boundary.
    fn hand_end(&mut self, meta: &HandMeta);

    /// Called once when the match is over; buffered sinks write here.
    fn finish(&mut self) {}
}

/// One logged line: the hand it belongs to, plus the engine event.
#[derive(serde::Serialize)]
struct LogLine<'a> {
    hand: u64,
    ev: &'a Event,
}

/// The header line opening a hand in [`JsonLog`]: `{"hand":N,"deck":D,
/// "seats":[...]}`.
#[derive(serde::Serialize)]
struct HandHeader<'a> {
    hand: u64,
    deck: u64,
    seats: &'a [String],
}

/// The header line opening a hand in [`SelectiveLog`]'s output: like
/// [`HandHeader`], plus the reasons the hand was kept.
#[derive(serde::Serialize)]
struct SelectiveHandHeader<'a> {
    hand: u64,
    deck: u64,
    seats: &'a [String],
    kept: Vec<&'static str>,
}

/// Trailing summary line, wrapped under a `"log_summary"` key.
#[derive(serde::Serialize)]
struct LogSummaryLine<T> {
    log_summary: T,
}

/// [`JsonLog`]'s summary: every hand is kept, so the two counts match.
#[derive(serde::Serialize)]
struct FullLogSummary {
    hands_seen: u64,
    hands_kept: u64,
}

/// [`SelectiveLog`]'s summary: echoes the selection knobs alongside counts.
#[derive(serde::Serialize)]
struct SelectiveLogSummary {
    hands_seen: u64,
    hands_kept: u64,
    sample_first_hands: Option<u64>,
    top_pots: Option<usize>,
    fault_hands_kept: u64,
}

/// Writes one JSON object per line: a `{"hand":N,"deck":D,"seats":[...]}`
/// header at the start of each hand, then one `{"hand":N,"ev":<Event>}` line
/// per event, and finally — once, from [`EventSink::finish`] — a trailing
/// `{"log_summary":{"hands_seen":H,"hands_kept":H}}` line (every hand is
/// kept in full-log mode, so the two counts always match).
///
/// `hand_end` flushes, so a completed hand is always fully on disk even if
/// the process is killed mid-match.
pub struct JsonLog<W: Write> {
    out: W,
    hand_no: u64,
    hands_seen: u64,
}

impl<W: Write> JsonLog<W> {
    pub fn new(out: W) -> Self {
        Self {
            out,
            hand_no: 0,
            hands_seen: 0,
        }
    }
}

impl<W: Write> EventSink for JsonLog<W> {
    fn hand_start(&mut self, hand_no: u64, deck_no: u64, seats: &[String]) {
        self.hand_no = hand_no;
        let header = HandHeader {
            hand: hand_no,
            deck: deck_no,
            seats,
        };
        // See `event` below: a serialization/write failure here has no
        // useful recovery, so the line is dropped rather than aborting the
        // match.
        if let Ok(json) = serde_json::to_string(&header) {
            let _ = writeln!(self.out, "{json}");
        }
    }

    fn event(&mut self, ev: &Event) {
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

    fn hand_end(&mut self, _meta: &HandMeta) {
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

/// Which hands [`SelectiveLog`] keeps.
pub struct LogSelection {
    /// Keep the first N hands, extended to whole decks so a duplicate
    /// rotation set (mirror pair) is never split: decks keep being sampled
    /// until at least N hands are kept. `None` = off; `Some(0)` is invalid
    /// (validated by the CLI, not here).
    pub sample_first_hands: Option<u64>,
    /// Keep the K biggest-pot hands (global top K, single-pass min-heap).
    pub top_pots: Option<usize>,
    /// Keep the first K hands in which any bot faulted. Forfeit hands are
    /// always kept regardless of this cap.
    pub fault_hands: u64,
}

/// One hand buffered between `hand_start` and `hand_end`: header info plus
/// the unredacted event stream. Held until the keep/drop decision is made,
/// and — for hands that are kept — until [`SelectiveLog::finish`] writes it
/// out; dropped hands free their buffer immediately.
#[derive(Clone)]
struct BufferedHand {
    hand_no: u64,
    deck_no: u64,
    seats: Vec<String>,
    events: Vec<Event>,
}

/// One candidate in the top-pots min-heap: ordered so the heap's max
/// (`peek`) is always the weakest kept entry — smallest pot, ties broken in
/// favor of evicting the *larger* hand number (so lower hand numbers win
/// ties, deterministically).
struct TopEntry {
    pot_total: u64,
    hand: BufferedHand,
}

impl PartialEq for TopEntry {
    fn eq(&self, other: &Self) -> bool {
        self.pot_total == other.pot_total && self.hand.hand_no == other.hand.hand_no
    }
}

impl Eq for TopEntry {}

impl Ord for TopEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reversed on pot_total: the *smaller* pot ranks higher (is more
        // eligible for eviction). Tied pots break on hand_no descending,
        // so the higher hand_no — the one we'd rather evict — ranks higher.
        other
            .pot_total
            .cmp(&self.pot_total)
            .then(self.hand.hand_no.cmp(&other.hand.hand_no))
    }
}

impl PartialOrd for TopEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Keeps only a chosen subset of hands: sampled rotation sets, the biggest
/// pots, and fault evidence. Nothing is written until [`EventSink::finish`]
/// — the full match must be seen before "biggest pots" and "first K faults"
/// can be decided — at which point every kept hand is written as a header
/// line (`{"hand":N,"deck":D,"seats":[...],"kept":[...]}`) followed by its
/// event lines, then a trailing `{"log_summary":{...}}` line.
///
/// Memory stays O(sample-window + top_pots + fault_hands): every hand is
/// buffered only between its own `hand_start`/`hand_end`, dropped
/// immediately unless it is kept for a reason or is a top-pots candidate.
pub struct SelectiveLog<W: Write> {
    out: W,
    selection: LogSelection,
    hands_seen: u64,
    fault_hands_kept: u64,
    /// Hands kept by the first-N sample so far.
    sample_hands_kept: u64,
    /// The deck currently being sampled: its remaining rotations are kept
    /// even once the N-hand target is reached, so a set is never split.
    sampling_deck: Option<u64>,
    current: Option<BufferedHand>,
    /// hand_no -> (buffered hand, reasons it was kept). A `BTreeMap` so
    /// `finish` can iterate in ascending hand_no order for free.
    kept: BTreeMap<u64, (BufferedHand, BTreeSet<&'static str>)>,
    /// Single-pass top-K min-heap over pot_total, independent of `kept`
    /// until merged in `finish`.
    top_heap: BinaryHeap<TopEntry>,
}

impl<W: Write> SelectiveLog<W> {
    pub fn new(out: W, selection: LogSelection) -> Self {
        Self {
            out,
            selection,
            hands_seen: 0,
            fault_hands_kept: 0,
            sample_hands_kept: 0,
            sampling_deck: None,
            current: None,
            kept: BTreeMap::new(),
            top_heap: BinaryHeap::new(),
        }
    }

    /// Considers `hand` (whose pot is `pot_total`) for the top-K set,
    /// evicting the current weakest member if `hand` beats it.
    fn consider_top_pot(&mut self, k: usize, pot_total: u64, hand: BufferedHand) {
        if k == 0 {
            return;
        }
        let entry = TopEntry { pot_total, hand };
        if self.top_heap.len() < k {
            self.top_heap.push(entry);
        } else if self.top_heap.peek().is_some_and(|worst| entry < *worst) {
            self.top_heap.pop();
            self.top_heap.push(entry);
        }
    }
}

impl<W: Write> EventSink for SelectiveLog<W> {
    fn hand_start(&mut self, hand_no: u64, deck_no: u64, seats: &[String]) {
        self.current = Some(BufferedHand {
            hand_no,
            deck_no,
            seats: seats.to_vec(),
            events: Vec::new(),
        });
    }

    fn event(&mut self, ev: &Event) {
        if let Some(hand) = &mut self.current {
            hand.events.push(ev.clone());
        }
    }

    fn hand_end(&mut self, meta: &HandMeta) {
        self.hands_seen += 1;
        let Some(hand) = self.current.take() else {
            return;
        };
        let hand_no = hand.hand_no;

        let mut reasons: BTreeSet<&'static str> = BTreeSet::new();
        if let Some(n) = self.selection.sample_first_hands {
            // First-N sampling, but never splitting a rotation set: a deck
            // whose first hand was sampled keeps its remaining rotations.
            let continue_deck = self.sampling_deck == Some(hand.deck_no);
            if continue_deck || self.sample_hands_kept < n {
                self.sampling_deck = Some(hand.deck_no);
                self.sample_hands_kept += 1;
                reasons.insert("sample");
            }
        }
        if meta.faulted && self.fault_hands_kept < self.selection.fault_hands {
            reasons.insert("fault");
            self.fault_hands_kept += 1;
        }
        if meta.forfeited {
            reasons.insert("forfeit");
        }

        // Every hand is a top-pots candidate, even one already kept for
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

        if let (Some(k), Some(candidate)) = (self.selection.top_pots, top_candidate) {
            self.consider_top_pot(k, meta.pot_total, candidate);
        }
    }

    fn finish(&mut self) {
        for entry in self.top_heap.drain() {
            let TopEntry { hand, .. } = entry;
            match self.kept.get_mut(&hand.hand_no) {
                Some((_, reasons)) => {
                    reasons.insert("top-pot");
                }
                None => {
                    let mut reasons = BTreeSet::new();
                    reasons.insert("top-pot");
                    self.kept.insert(hand.hand_no, (hand, reasons));
                }
            }
        }

        for (hand, reasons) in self.kept.values() {
            let header = SelectiveHandHeader {
                hand: hand.hand_no,
                deck: hand.deck_no,
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
                top_pots: self.selection.top_pots,
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
    use poker_core::game::PostKind;
    use serde_json::Value;

    fn meta(pot_total: u64) -> HandMeta {
        HandMeta {
            pot_total,
            faulted: false,
            forfeited: false,
        }
    }

    #[test]
    fn writes_headers_events_and_summary_with_correct_hand_numbers() {
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut log = JsonLog::new(&mut buf);
            log.hand_start(0, 0, &["a".to_string(), "b".to_string()]);
            log.event(&Event::HandStart {
                hand_no: 0,
                button: 0,
                stacks: vec![100, 100],
            });
            log.event(&Event::Post {
                seat: 0,
                kind: PostKind::SmallBlind,
                amount: 1,
                all_in: false,
            });
            log.hand_end(&meta(2));

            log.hand_start(1, 0, &["b".to_string(), "a".to_string()]);
            log.event(&Event::HandEnd { nets: vec![1, -1] });
            log.hand_end(&meta(2));
            log.finish();
        }

        let text = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        // header, 2 events, header, 1 event, summary
        assert_eq!(lines.len(), 6);

        let parsed: Vec<Value> = lines
            .iter()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(parsed[0]["hand"], 0);
        assert_eq!(parsed[0]["deck"], 0);
        assert_eq!(parsed[0]["seats"], serde_json::json!(["a", "b"]));
        assert_eq!(parsed[1]["hand"], 0);
        assert_eq!(parsed[1]["ev"]["event"], "hand-start");
        assert_eq!(parsed[2]["hand"], 0);
        assert_eq!(parsed[2]["ev"]["event"], "post");
        assert_eq!(parsed[3]["hand"], 1);
        assert_eq!(parsed[3]["seats"], serde_json::json!(["b", "a"]));
        assert_eq!(parsed[4]["hand"], 1);
        assert_eq!(parsed[4]["ev"]["event"], "hand-end");
        assert_eq!(parsed[5]["log_summary"]["hands_seen"], 2);
        assert_eq!(parsed[5]["log_summary"]["hands_kept"], 2);
    }

    #[test]
    fn hand_end_flushes() {
        // A `Vec<u8>`-backed writer is always "flushed" trivially; this test
        // exists to document the contract and catch an accidental no-op
        // `flush` regression via a wrapper that only exposes data after
        // `flush`.
        struct FlushGate {
            staged: Vec<u8>,
            visible: Vec<u8>,
        }
        impl Write for FlushGate {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.staged.extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                self.visible.append(&mut self.staged);
                Ok(())
            }
        }

        let gate = FlushGate {
            staged: Vec::new(),
            visible: Vec::new(),
        };
        let mut log = JsonLog::new(gate);
        log.hand_start(0, 0, &["a".to_string(), "b".to_string()]);
        log.event(&Event::HandEnd { nets: vec![0, 0] });
        assert!(
            log.out.visible.is_empty(),
            "should not be visible pre-flush"
        );
        log.hand_end(&meta(0));
        assert!(!log.out.visible.is_empty(), "hand_end must flush");
    }

    // ---- SelectiveLog unit tests (mechanics in isolation; full-match
    // integration tests live in runner.rs) ----

    fn push_hand<W: Write>(
        log: &mut SelectiveLog<W>,
        hand_no: u64,
        deck_no: u64,
        pot_total: u64,
        faulted: bool,
        forfeited: bool,
    ) {
        log.hand_start(hand_no, deck_no, &["a".to_string(), "b".to_string()]);
        log.event(&Event::HandStart {
            hand_no,
            button: 0,
            stacks: vec![100, 100],
        });
        log.hand_end(&HandMeta {
            pot_total,
            faulted,
            forfeited,
        });
    }

    fn parse_lines(buf: Vec<u8>) -> Vec<Value> {
        let text = String::from_utf8(buf).unwrap();
        text.lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn nothing_written_until_finish() {
        let mut buf: Vec<u8> = Vec::new();
        let mut log = SelectiveLog::new(
            &mut buf,
            LogSelection {
                sample_first_hands: Some(u64::MAX),
                top_pots: None,
                fault_hands: 0,
            },
        );
        push_hand(&mut log, 0, 0, 10, false, false);
        assert!(buf.is_empty(), "nothing should be written pre-finish");
    }

    #[test]
    fn top_pots_keeps_the_k_biggest_with_deterministic_tie_break() {
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut log = SelectiveLog::new(
                &mut buf,
                LogSelection {
                    sample_first_hands: None,
                    top_pots: Some(2),
                    fault_hands: 0,
                },
            );
            // Pots: 10, 30, 30, 20 at hand_nos 0..3. Top-2 by pot desc,
            // hand_no asc tie-break: hand 1 (30) then hand 2 (30, tied but
            // higher hand_no loses the tie).
            push_hand(&mut log, 0, 0, 10, false, false);
            push_hand(&mut log, 1, 0, 30, false, false);
            push_hand(&mut log, 2, 0, 30, false, false);
            push_hand(&mut log, 3, 0, 20, false, false);
            log.finish();
        }
        let parsed = parse_lines(buf);
        let headers: Vec<&Value> = parsed.iter().filter(|v| v.get("kept").is_some()).collect();
        assert_eq!(headers.len(), 2);
        assert_eq!(headers[0]["hand"], 1);
        assert_eq!(headers[1]["hand"], 2);
        for h in &headers {
            assert_eq!(h["kept"], serde_json::json!(["top-pot"]));
        }
        let summary = parsed.last().unwrap();
        assert_eq!(summary["log_summary"]["hands_seen"], 4);
        assert_eq!(summary["log_summary"]["hands_kept"], 2);
    }

    #[test]
    fn fault_cap_keeps_only_the_first_k_fault_hands() {
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut log = SelectiveLog::new(
                &mut buf,
                LogSelection {
                    sample_first_hands: None,
                    top_pots: None,
                    fault_hands: 2,
                },
            );
            push_hand(&mut log, 0, 0, 1, true, false);
            push_hand(&mut log, 1, 0, 1, true, false);
            push_hand(&mut log, 2, 0, 1, true, false);
            log.finish();
        }
        let parsed = parse_lines(buf);
        let headers: Vec<&Value> = parsed.iter().filter(|v| v.get("kept").is_some()).collect();
        assert_eq!(headers.len(), 2);
        assert_eq!(headers[0]["hand"], 0);
        assert_eq!(headers[1]["hand"], 1);
        let summary = parsed.last().unwrap();
        assert_eq!(summary["log_summary"]["fault_hands_kept"], 2);
    }

    #[test]
    fn forfeit_always_kept_regardless_of_fault_cap() {
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut log = SelectiveLog::new(
                &mut buf,
                LogSelection {
                    sample_first_hands: None,
                    top_pots: None,
                    fault_hands: 0,
                },
            );
            push_hand(&mut log, 0, 0, 1, true, true);
            log.finish();
        }
        let parsed = parse_lines(buf);
        let headers: Vec<&Value> = parsed.iter().filter(|v| v.get("kept").is_some()).collect();
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0]["kept"], serde_json::json!(["forfeit"]));
    }

    #[test]
    fn union_of_reasons_is_tagged_on_one_header() {
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut log = SelectiveLog::new(
                &mut buf,
                LogSelection {
                    sample_first_hands: Some(u64::MAX),
                    top_pots: Some(1),
                    fault_hands: 0,
                },
            );
            push_hand(&mut log, 0, 0, 5, false, false);
            push_hand(&mut log, 1, 0, 1, false, false);
            log.finish();
        }
        let parsed = parse_lines(buf);
        let headers: Vec<&Value> = parsed.iter().filter(|v| v.get("kept").is_some()).collect();
        assert_eq!(headers.len(), 2, "sample keeps both hands");
        let hand0 = headers.iter().find(|h| h["hand"] == 0).unwrap();
        assert_eq!(hand0["kept"], serde_json::json!(["sample", "top-pot"]));
        let hand1 = headers.iter().find(|h| h["hand"] == 1).unwrap();
        assert_eq!(hand1["kept"], serde_json::json!(["sample"]));
    }
}

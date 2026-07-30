//! Hand-history logging.
//!
//! The runner streams the *unredacted* event sequence to an [`EventSink`] as
//! a match plays out, bracketed by [`EventSink::hand_start`] /
//! [`EventSink::hand_end`] per hand. [`JsonLog`] is the reference
//! implementation: one JSON object per line, suitable for `tail -f` or
//! line-by-line replay tooling.

use std::io::Write;

use poker_core::game::Event;

/// Receives the unredacted event stream as a match plays out.
pub trait EventSink {
    /// Opens a hand boundary; `hand_no` matches the events that follow until
    /// the next `hand_end`.
    fn hand_start(&mut self, hand_no: u64);

    /// One engine event, in the order the engine produced it.
    fn event(&mut self, ev: &Event);

    /// Closes the current hand boundary.
    fn hand_end(&mut self);
}

/// Writes one JSON object per line: `{"hand": N, "ev": <Event>}`.
///
/// `hand_end` flushes, so a completed hand is always fully on disk even if
/// the process is killed mid-match.
pub struct JsonLog<W: Write> {
    out: W,
    hand_no: u64,
}

impl<W: Write> JsonLog<W> {
    pub fn new(out: W) -> Self {
        Self { out, hand_no: 0 }
    }
}

/// One logged line: the hand it belongs to, plus the engine event.
#[derive(serde::Serialize)]
struct LogLine<'a> {
    hand: u64,
    ev: &'a Event,
}

impl<W: Write> EventSink for JsonLog<W> {
    fn hand_start(&mut self, hand_no: u64) {
        self.hand_no = hand_no;
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

    fn hand_end(&mut self) {
        let _ = self.out.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use poker_core::game::PostKind;
    use serde_json::Value;

    #[test]
    fn writes_one_valid_json_object_per_line_with_correct_hand_numbers() {
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut log = JsonLog::new(&mut buf);
            log.hand_start(0);
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
            log.hand_end();

            log.hand_start(1);
            log.event(&Event::HandEnd { nets: vec![1, -1] });
            log.hand_end();
        }

        let text = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);

        let parsed: Vec<Value> = lines
            .iter()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(parsed[0]["hand"], 0);
        assert_eq!(parsed[0]["ev"]["event"], "hand-start");
        assert_eq!(parsed[1]["hand"], 0);
        assert_eq!(parsed[1]["ev"]["event"], "post");
        assert_eq!(parsed[2]["hand"], 1);
        assert_eq!(parsed[2]["ev"]["event"], "hand-end");
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
        log.hand_start(0);
        log.event(&Event::HandEnd { nets: vec![0, 0] });
        assert!(
            log.out.visible.is_empty(),
            "should not be visible pre-flush"
        );
        log.hand_end();
        assert!(!log.out.visible.is_empty(), "hand_end must flush");
    }
}

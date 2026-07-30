//! JSON Lines framing: one `\n`-terminated JSON object per message, no
//! pretty printing. Transport-agnostic — works over any
//! `std::io::Write`/`BufRead`, so callers can wire this to a TCP socket or a
//! subprocess's stdio.

use std::io::{BufRead, Write};

use crate::MAX_LINE_BYTES;

/// Framing- and codec-level failures.
#[derive(Debug, thiserror::Error)]
pub enum WireError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// The peer closed the connection (clean EOF between messages).
    #[error("connection closed")]
    Closed,
    /// A line exceeded `MAX_LINE_BYTES` before a newline was found. The
    /// offending line is *not* buffered in full (see [`read_msg`]), so a
    /// hostile peer can't force unbounded memory use.
    #[error("line exceeds the {limit}-byte limit")]
    TooLong { limit: usize },
    /// A line parsed as neither valid UTF-8/JSON nor the target type.
    #[error("malformed message: {source} (line: {line:?})")]
    Parse {
        line: String,
        source: serde_json::Error,
    },
}

/// Number of characters of an unparseable line to keep in [`WireError::Parse`]
/// for readability.
const PARSE_ERROR_LINE_CHARS: usize = 200;

/// Serialize `msg` as one compact JSON line (`\n`-terminated) and flush.
pub fn write_msg<W: Write, T: serde::Serialize>(w: &mut W, msg: &T) -> Result<(), WireError> {
    // Serialization of these DTOs cannot fail in practice (no maps with
    // non-string keys, no fallible custom Serialize impls); surface a
    // failure as an I/O error rather than growing WireError's surface.
    let mut buf = serde_json::to_vec(msg)
        .map_err(|e| WireError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))?;
    buf.push(b'\n');
    w.write_all(&buf)?;
    w.flush()?;
    Ok(())
}

/// Read and deserialize the next non-empty line.
///
/// EOF with nothing read returns [`WireError::Closed`]. Lines are capped at
/// `MAX_LINE_BYTES` using a `Take`-limited reader per attempt, so a peer that
/// never sends a newline can force at most ~`MAX_LINE_BYTES` of buffering
/// before we bail with [`WireError::TooLong`], not unbounded memory growth.
/// Empty lines (bare `\n`) are skipped.
pub fn read_msg<R: BufRead, T: serde::de::DeserializeOwned>(r: &mut R) -> Result<T, WireError> {
    loop {
        let mut buf = Vec::new();
        let cap = MAX_LINE_BYTES as u64 + 1;
        // UFCS, explicitly instantiated at `&mut R` and fed a fresh reborrow:
        // plain method-call syntax lets resolution deref past `&mut R` to
        // `R` and try to move it out from behind the reference.
        let mut limited = <&mut R as std::io::Read>::take(&mut *r, cap);
        let n = limited.read_until(b'\n', &mut buf)?;
        if n == 0 {
            return Err(WireError::Closed);
        }

        let ends_with_newline = buf.last() == Some(&b'\n');
        if !ends_with_newline && buf.len() as u64 >= cap {
            // Hit the take() cap without finding a newline: the line is too
            // long. The rest of it is still sitting unread on `r`, but we've
            // already refused to buffer past `cap` bytes.
            return Err(WireError::TooLong {
                limit: MAX_LINE_BYTES,
            });
        }
        if ends_with_newline {
            buf.pop();
            if buf.last() == Some(&b'\r') {
                buf.pop();
            }
        }

        if buf.is_empty() {
            continue;
        }
        if buf.len() > MAX_LINE_BYTES {
            return Err(WireError::TooLong {
                limit: MAX_LINE_BYTES,
            });
        }

        return serde_json::from_slice(&buf).map_err(|source| WireError::Parse {
            line: truncate_for_display(&String::from_utf8_lossy(&buf), PARSE_ERROR_LINE_CHARS),
            source,
        });
    }
}

/// Truncate `s` to at most `max_chars` chars (UTF-8-safe), appending `...`
/// when truncated.
fn truncate_for_display(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars).collect();
    out.push_str("...");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Sample {
        n: u32,
        s: String,
    }

    #[test]
    fn write_then_read_round_trips_over_a_vec() {
        let mut buf: Vec<u8> = Vec::new();
        let a = Sample {
            n: 1,
            s: "hello".to_string(),
        };
        let b = Sample {
            n: 2,
            s: "world".to_string(),
        };
        write_msg(&mut buf, &a).unwrap();
        write_msg(&mut buf, &b).unwrap();

        let mut cursor = Cursor::new(buf);
        let got_a: Sample = read_msg(&mut cursor).unwrap();
        let got_b: Sample = read_msg(&mut cursor).unwrap();
        assert_eq!(got_a, a);
        assert_eq!(got_b, b);
    }

    #[test]
    fn write_msg_produces_one_compact_newline_terminated_line() {
        let mut buf: Vec<u8> = Vec::new();
        write_msg(
            &mut buf,
            &Sample {
                n: 7,
                s: "x".to_string(),
            },
        )
        .unwrap();
        assert_eq!(buf, b"{\"n\":7,\"s\":\"x\"}\n");
    }

    #[test]
    fn eof_with_nothing_pending_is_closed() {
        let mut cursor = Cursor::new(Vec::<u8>::new());
        let err = read_msg::<_, Sample>(&mut cursor).unwrap_err();
        assert!(matches!(err, WireError::Closed));
    }

    #[test]
    fn oversized_line_is_too_long_and_does_not_buffer_it_all() {
        // One line far larger than MAX_LINE_BYTES, no other lines after it.
        let huge = "a".repeat(MAX_LINE_BYTES * 4);
        let mut cursor = Cursor::new(huge.into_bytes());
        let err = read_msg::<_, Sample>(&mut cursor).unwrap_err();
        match err {
            WireError::TooLong { limit } => assert_eq!(limit, MAX_LINE_BYTES),
            other => panic!("expected TooLong, got {other:?}"),
        }
    }

    #[test]
    fn line_at_exactly_the_limit_is_accepted() {
        // MAX_LINE_BYTES of JSON string content plus quotes/newline is over
        // the limit including the JSON structure, so build a string whose
        // *line* (JSON text) is exactly MAX_LINE_BYTES bytes.
        let overhead = r#"{"n":0,"s":""}"#.len();
        let pad = MAX_LINE_BYTES - overhead;
        let value = Sample {
            n: 0,
            s: "a".repeat(pad),
        };
        let text = serde_json::to_string(&value).unwrap();
        assert_eq!(text.len(), MAX_LINE_BYTES);

        let mut buf = text.into_bytes();
        buf.push(b'\n');
        let mut cursor = Cursor::new(buf);
        let got: Sample = read_msg(&mut cursor).unwrap();
        assert_eq!(got, value);
    }

    #[test]
    fn malformed_json_is_a_parse_error_with_truncated_line() {
        let mut buf = "not json at all\n".as_bytes().to_vec();
        let mut cursor = Cursor::new(std::mem::take(&mut buf));
        let err = read_msg::<_, Sample>(&mut cursor).unwrap_err();
        match err {
            WireError::Parse { line, .. } => assert_eq!(line, "not json at all"),
            other => panic!("expected Parse, got {other:?}"),
        }
    }

    #[test]
    fn malformed_json_line_is_truncated_for_display() {
        let long_garbage = "x".repeat(1000);
        let mut input = long_garbage.clone();
        input.push('\n');
        let mut cursor = Cursor::new(input.into_bytes());
        let err = read_msg::<_, Sample>(&mut cursor).unwrap_err();
        match err {
            WireError::Parse { line, .. } => {
                assert!(line.len() < long_garbage.len());
                assert!(line.ends_with("..."));
            }
            other => panic!("expected Parse, got {other:?}"),
        }
    }

    #[test]
    fn empty_lines_are_skipped() {
        let a = Sample {
            n: 1,
            s: "a".to_string(),
        };
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(b"\n\n");
        buf.extend_from_slice(serde_json::to_string(&a).unwrap().as_bytes());
        buf.push(b'\n');
        buf.extend_from_slice(b"\n");

        let mut cursor = Cursor::new(buf);
        let got: Sample = read_msg(&mut cursor).unwrap();
        assert_eq!(got, a);
        // Nothing left but skippable blank content -> Closed, not a message.
        let err = read_msg::<_, Sample>(&mut cursor).unwrap_err();
        assert!(matches!(err, WireError::Closed));
    }
}

//! Pure SSE (Server-Sent Events) parser for the helene transport layer.
//!
//! Extracts SSE parsing from helene's HTTP transport into a standalone,
//! synchronous parser. No async, no I/O, no allocator tricks — just
//! line-oriented parsing with bounded accumulation.
//!
//! # Size limits
//!
//! The parser enforces `max_event_data_bytes` during accumulation, not
//! just at event completion. This prevents OOM from malicious servers
//! that send unlimited `data:` lines before a blank line boundary.
//!
//! # Edition
//!
//! Edition 2021 — Kani requires this (helene main uses 2024).

/// A parsed SSE event.
///
/// Accumulated from `event:`, `data:`, and `signature:` fields
/// between blank-line boundaries in the SSE stream.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SseEvent {
    /// Event type (e.g., `"message"`). `None` defaults to `"message"`
    /// per the SSE spec.
    pub event_type: Option<String>,
    /// Accumulated data payload. Multiple `data:` fields are joined
    /// with `'\n'` separators per SSE spec.
    pub data: String,
    /// Optional signature field (MCP HMAC extension).
    pub signature: Option<String>,
}

/// Error from SSE parsing when size limits are exceeded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SseParseError {
    /// Accumulated event data would exceed the configured limit.
    ///
    /// The parser state is NOT reset on error — call
    /// [`SseParser::reset`] to discard the current event and
    /// continue parsing from the next blank-line boundary.
    EventDataTooLarge {
        /// Bytes that would have been accumulated.
        would_accumulate: usize,
        /// Configured limit.
        limit: usize,
    },
}

impl core::fmt::Display for SseParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SseParseError::EventDataTooLarge {
                would_accumulate,
                limit,
            } => write!(
                f,
                "SSE event data would reach {would_accumulate} bytes, exceeding {limit} byte limit"
            ),
        }
    }
}

/// Incremental SSE line parser with bounded accumulation.
///
/// Feed lines (without trailing newlines) via [`feed_line`](Self::feed_line).
/// A blank line signals the end of an event; if the event has data, it is
/// returned.
///
/// # Bounded accumulation
///
/// Size limits are enforced during data accumulation — each `data:` field
/// is checked BEFORE it is appended. This prevents OOM from a malicious
/// server sending unlimited `data:` lines without a blank-line boundary.
///
/// The previous design only checked `max_event_data_bytes` at event
/// completion (on blank lines), allowing unbounded accumulation between
/// boundaries.
pub struct SseParser {
    current: SseEvent,
    has_data: bool,
    max_event_data_bytes: usize,
}

impl SseParser {
    /// Create a new parser with the given maximum event data size.
    ///
    /// `max_event_data_bytes` limits the total accumulated bytes across
    /// all `data:` fields in a single event. Set to `usize::MAX` to
    /// disable the limit (not recommended for untrusted input).
    pub fn new(max_event_data_bytes: usize) -> Self {
        Self {
            current: SseEvent::default(),
            has_data: false,
            max_event_data_bytes,
        }
    }

    /// Feed a single line (without trailing newline).
    ///
    /// Returns `Ok(Some(event))` when a blank line completes an event
    /// that has accumulated data. Returns `Ok(None)` for non-completing
    /// lines (data fields, event type declarations, comments, unknown
    /// fields, and blank lines with no pending data).
    ///
    /// # Errors
    ///
    /// Returns [`SseParseError::EventDataTooLarge`] if accepting a
    /// `data:` field would push accumulated data past
    /// `max_event_data_bytes`. The data is NOT appended, and the
    /// parser state is left dirty — call [`reset`](Self::reset)
    /// to discard the in-progress event.
    pub fn feed_line(&mut self, line: &str) -> Result<Option<SseEvent>, SseParseError> {
        if line.is_empty() {
            // Blank line = event boundary.
            if self.has_data {
                let event = core::mem::take(&mut self.current);
                self.has_data = false;
                return Ok(Some(event));
            }
            return Ok(None);
        }

        // Comment lines start with ':'
        if line.starts_with(':') {
            return Ok(None);
        }

        let (field, value) = match line.find(':') {
            Some(pos) => {
                let value = &line[pos + 1..];
                // Strip single leading space per SSE spec.
                let value = value.strip_prefix(' ').unwrap_or(value);
                (&line[..pos], value)
            }
            None => (line, ""),
        };

        match field {
            "event" => self.current.event_type = Some(value.to_owned()),
            "data" => {
                // Check size BEFORE accumulating — the fix for the OOM
                // vector identified by Kani verification. Previously,
                // the size check only fired at event completion (on
                // blank lines), allowing unbounded accumulation between
                // boundaries.
                let new_len = if self.has_data {
                    self.current.data.len() + 1 + value.len() // +1 for '\n'
                } else {
                    value.len()
                };
                if new_len > self.max_event_data_bytes {
                    return Err(SseParseError::EventDataTooLarge {
                        would_accumulate: new_len,
                        limit: self.max_event_data_bytes,
                    });
                }
                if self.has_data {
                    self.current.data.push('\n');
                }
                self.current.data.push_str(value);
                self.has_data = true;
            }
            "signature" => self.current.signature = Some(value.to_owned()),
            _ => {} // Ignore unknown fields per spec.
        }

        Ok(None)
    }

    /// Reset the parser, discarding any in-progress event.
    ///
    /// Use after an [`SseParseError`] to resume parsing from the
    /// next event boundary.
    pub fn reset(&mut self) {
        self.current = SseEvent::default();
        self.has_data = false;
    }

    /// Returns the byte count currently accumulated in the
    /// in-progress event's data field.
    ///
    /// Useful for diagnostics and verification harnesses.
    pub fn accumulated_data_len(&self) -> usize {
        self.current.data.len()
    }
}

// ── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // Use a generous limit for tests that don't exercise bounds.
    const BIG: usize = 4 * 1024 * 1024;

    #[test]
    fn simple_message() {
        let mut parser = SseParser::new(BIG);
        assert!(matches!(parser.feed_line("data: hello"), Ok(None)));
        let event = parser.feed_line("").unwrap().unwrap();
        assert_eq!(event.data, "hello");
        assert!(event.event_type.is_none());
    }

    #[test]
    fn named_event() {
        let mut parser = SseParser::new(BIG);
        parser.feed_line("event: message").unwrap();
        parser.feed_line("data: {\"test\": true}").unwrap();
        let event = parser.feed_line("").unwrap().unwrap();
        assert_eq!(event.event_type.as_deref(), Some("message"));
        assert_eq!(event.data, "{\"test\": true}");
    }

    #[test]
    fn multiline_data() {
        let mut parser = SseParser::new(BIG);
        parser.feed_line("data: line1").unwrap();
        parser.feed_line("data: line2").unwrap();
        parser.feed_line("data: line3").unwrap();
        let event = parser.feed_line("").unwrap().unwrap();
        assert_eq!(event.data, "line1\nline2\nline3");
    }

    #[test]
    fn comment_ignored() {
        let mut parser = SseParser::new(BIG);
        parser.feed_line(": this is a comment").unwrap();
        parser.feed_line("data: actual data").unwrap();
        let event = parser.feed_line("").unwrap().unwrap();
        assert_eq!(event.data, "actual data");
    }

    #[test]
    fn empty_blank_lines_ignored() {
        let mut parser = SseParser::new(BIG);
        assert!(parser.feed_line("").unwrap().is_none());
        assert!(parser.feed_line("").unwrap().is_none());
    }

    #[test]
    fn signature_field() {
        let mut parser = SseParser::new(BIG);
        parser.feed_line("data: payload").unwrap();
        parser.feed_line("signature: abc123").unwrap();
        let event = parser.feed_line("").unwrap().unwrap();
        assert_eq!(event.data, "payload");
        assert_eq!(event.signature.as_deref(), Some("abc123"));
    }

    #[test]
    fn unknown_field_ignored() {
        let mut parser = SseParser::new(BIG);
        parser.feed_line("custom: ignored").unwrap();
        parser.feed_line("data: kept").unwrap();
        let event = parser.feed_line("").unwrap().unwrap();
        assert_eq!(event.data, "kept");
    }

    #[test]
    fn data_no_space_after_colon() {
        let mut parser = SseParser::new(BIG);
        parser.feed_line("data:nospace").unwrap();
        let event = parser.feed_line("").unwrap().unwrap();
        assert_eq!(event.data, "nospace");
    }

    #[test]
    fn consecutive_events() {
        let mut parser = SseParser::new(BIG);
        parser.feed_line("data: first").unwrap();
        let e1 = parser.feed_line("").unwrap().unwrap();
        assert_eq!(e1.data, "first");

        parser.feed_line("data: second").unwrap();
        let e2 = parser.feed_line("").unwrap().unwrap();
        assert_eq!(e2.data, "second");
    }

    // ── Bounds enforcement tests ───────────────────────────────

    #[test]
    fn data_within_limit_accepted() {
        let mut parser = SseParser::new(10);
        parser.feed_line("data: hello").unwrap(); // 5 bytes, under 10
        let event = parser.feed_line("").unwrap().unwrap();
        assert_eq!(event.data, "hello");
    }

    #[test]
    fn data_at_limit_accepted() {
        let mut parser = SseParser::new(5);
        parser.feed_line("data: hello").unwrap(); // exactly 5 bytes
        let event = parser.feed_line("").unwrap().unwrap();
        assert_eq!(event.data, "hello");
    }

    #[test]
    fn data_over_limit_rejected() {
        let mut parser = SseParser::new(4);
        let err = parser.feed_line("data: hello").unwrap_err(); // 5 > 4
        assert_eq!(
            err,
            SseParseError::EventDataTooLarge {
                would_accumulate: 5,
                limit: 4,
            }
        );
    }

    #[test]
    fn multiline_accumulation_over_limit_rejected() {
        let mut parser = SseParser::new(8);
        parser.feed_line("data: abcd").unwrap(); // 4 bytes
                                                 // Next line would be 4 + 1('\n') + 4 = 9 > 8
        let err = parser.feed_line("data: efgh").unwrap_err();
        assert_eq!(
            err,
            SseParseError::EventDataTooLarge {
                would_accumulate: 9,
                limit: 8,
            }
        );
    }

    #[test]
    fn reset_after_error_allows_new_event() {
        let mut parser = SseParser::new(4);
        let _ = parser.feed_line("data: toolong"); // error
        parser.reset();
        // Skip to next blank line to complete any partial state
        parser.feed_line("data: ok").unwrap(); // 2 bytes, fine
        let event = parser.feed_line("").unwrap().unwrap();
        assert_eq!(event.data, "ok");
    }

    #[test]
    fn accumulated_data_len_tracks_correctly() {
        let mut parser = SseParser::new(100);
        assert_eq!(parser.accumulated_data_len(), 0);
        parser.feed_line("data: abc").unwrap(); // 3 bytes
        assert_eq!(parser.accumulated_data_len(), 3);
        parser.feed_line("data: de").unwrap(); // +1('\n') + 2 = 6
        assert_eq!(parser.accumulated_data_len(), 6);
        let _ = parser.feed_line(""); // event completed, resets
        assert_eq!(parser.accumulated_data_len(), 0);
    }

    #[test]
    fn field_only_line_no_colon() {
        let mut parser = SseParser::new(BIG);
        // A line like "data" with no colon — value is ""
        parser.feed_line("data").unwrap();
        let event = parser.feed_line("").unwrap().unwrap();
        assert_eq!(event.data, "");
    }
}

// ── Kani proof harnesses ───────────────────────────────────────
//
// These verify the REAL parser code, not a model.
// Run: cargo kani --harness <name>

#[cfg(kani)]
mod proofs {
    use super::*;

    // Pre-built lines of known sizes for tractable CBMC exploration.
    // Avoids heap-heavy arbitrary string construction.
    const D1: &str = "data: a";
    const D2: &str = "data: ab";
    const D4: &str = "data: abcd";
    const D8: &str = "data: abcdefgh";
    const BLANK: &str = "";
    const COMMENT: &str = ": keepalive";
    const EVENT: &str = "event: message";
    const SIG: &str = "signature: deadbeef";

    /// Helper: choose a line from the pre-built set and return
    /// its data-field length (0 if not a data line).
    fn choose_line(choice: u8) -> (&'static str, usize) {
        match choice % 8 {
            0 => (D1, 1),
            1 => (D2, 2),
            2 => (D4, 4),
            3 => (D8, 8),
            4 => (BLANK, 0),
            5 => (COMMENT, 0),
            6 => (EVENT, 0),
            _ => (SIG, 0),
        }
    }

    // ════════════════════════════════════════════════════════════
    // INVARIANT #1: Accumulation bounds at ingestion
    // ════════════════════════════════════════════════════════════

    /// The parser's accumulated data never exceeds max_event_data_bytes
    /// after any successful feed_line call.
    ///
    /// This is the core safety property: bounded accumulation prevents
    /// OOM from malicious input.
    ///
    /// Expected: VERIFICATION SUCCESSFUL
    #[kani::proof]
    #[kani::unwind(7)]
    fn accumulation_always_bounded() {
        let max_data: usize = kani::any();
        kani::assume(max_data > 0 && max_data <= 16);

        let mut parser = SseParser::new(max_data);

        // Feed up to 5 lines
        let n: usize = kani::any();
        kani::assume(n <= 5);

        for _ in 0..n {
            let choice: u8 = kani::any();
            let (line, _) = choose_line(choice);

            match parser.feed_line(line) {
                Ok(_) => {
                    // After any successful call, invariant holds
                    assert!(
                        parser.accumulated_data_len() <= max_data,
                        "invariant violated: {} > {}",
                        parser.accumulated_data_len(),
                        max_data,
                    );
                }
                Err(SseParseError::EventDataTooLarge { .. }) => {
                    // Error returned BEFORE accumulation — data
                    // was NOT appended, so the invariant trivially
                    // holds from the previous state.
                }
            }
        }
    }

    // ════════════════════════════════════════════════════════════
    // INVARIANT #2: No unbounded allocation
    // ════════════════════════════════════════════════════════════

    /// Even when errors occur and the parser is reset, accumulated
    /// data never exceeds the limit.
    ///
    /// Expected: VERIFICATION SUCCESSFUL
    #[kani::proof]
    #[kani::unwind(7)]
    fn reset_and_continue_stays_bounded() {
        let max_data: usize = kani::any();
        kani::assume(max_data > 0 && max_data <= 16);

        let mut parser = SseParser::new(max_data);

        let n: usize = kani::any();
        kani::assume(n <= 5);

        for _ in 0..n {
            let choice: u8 = kani::any();
            let (line, _) = choose_line(choice);

            match parser.feed_line(line) {
                Ok(_) => {}
                Err(_) => {
                    // Reset and continue — simulates error recovery
                    parser.reset();
                }
            }

            assert!(
                parser.accumulated_data_len() <= max_data,
                "invariant violated after reset path",
            );
        }
    }

    // ════════════════════════════════════════════════════════════
    // STATE MACHINE: Event completion resets accumulator
    // ════════════════════════════════════════════════════════════

    /// After a blank line completes an event, accumulated_data_len
    /// is zero.
    ///
    /// Expected: VERIFICATION SUCCESSFUL
    #[kani::proof]
    #[kani::unwind(5)]
    fn blank_line_resets_accumulator() {
        let max_data: usize = kani::any();
        kani::assume(max_data >= 8 && max_data <= 16);

        let mut parser = SseParser::new(max_data);

        // Accumulate some data
        let n: usize = kani::any();
        kani::assume(n > 0 && n <= 3);

        for _ in 0..n {
            let _ = parser.feed_line(D1); // 1 byte each
        }

        // Complete the event
        let result = parser.feed_line(BLANK);
        assert!(result.is_ok());

        if result.unwrap().is_some() {
            // Event was completed — accumulator must be zero
            assert_eq!(
                parser.accumulated_data_len(),
                0,
                "accumulator not reset after event completion",
            );
        }
    }

    // ════════════════════════════════════════════════════════════
    // PROPERTY: Rejection is pre-emptive, not post-hoc
    // ════════════════════════════════════════════════════════════

    /// When feed_line returns an error, the data was NOT appended.
    /// The accumulated length is unchanged from before the call.
    ///
    /// Expected: VERIFICATION SUCCESSFUL
    #[kani::proof]
    fn rejection_does_not_grow_buffer() {
        let max_data: usize = kani::any();
        kani::assume(max_data > 0 && max_data <= 8);

        let mut parser = SseParser::new(max_data);

        // Feed one line that fits
        let _ = parser.feed_line(D1);
        let len_before = parser.accumulated_data_len();

        // Feed a line that might not fit
        let choice: u8 = kani::any();
        let (line, _) = choose_line(choice);

        if parser.feed_line(line).is_err() {
            // Error path: nothing was appended
            assert_eq!(
                parser.accumulated_data_len(),
                len_before,
                "error path modified accumulator",
            );
        }
    }
}

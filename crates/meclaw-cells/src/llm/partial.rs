//! GH #1410: the sentence partials of the `partial` lane.
//!
//! Measured before: the first reaction of a text turn came with the whole
//! answer body -- the chat wire collected its stream into one body, the
//! Responses wire read the whole SSE body before parsing it, and the Messages
//! wire did not stream at all. With `partial_unit: "sentence"` the wires hand
//! every visible text delta to a [`PartialTap`] while they read, and the tap
//! emits one `route: "partial"` message at each sentence boundary: the text
//! from the start of the answer up to that boundary, cumulative. The answer
//! itself leaves the cell as before, after the last partial.
//!
//! The rules (the lane contract, `docs/cell-types.md` § `llm`):
//!
//! * only visible text counts -- tool-call arguments and thinking never reach
//!   the tap, so no partial is ever cut inside them;
//! * a boundary is `.`, `!`, `?` or `…` (closing quotes and brackets may
//!   follow), then whitespace; the end of the text counts only where a tool
//!   call follows it, never at the end of the stream (the rest comes with the
//!   answer); a `.` right after a run of digits or a one-letter token is no
//!   boundary (review M4: an ordinal like `3.` before a month, the parts of
//!   `e. g.` -- a structural rule, no word list);
//! * leading whitespace neither counts nor goes out (review M5): the harmony
//!   recovery trims the answer, and a partial stays its prefix;
//! * one partial per boundary at most, never per token, and a partial that
//!   adds fewer than `partial_min_chars` characters waits for the next one;
//! * a text the cell's own sanitizer would rewrite (a harmony token, a
//!   provider annotation in the Private Use Area) stops the partials of that
//!   call: a partial must stay a prefix of the answer, and the answer is the
//!   sanitized text.

use meclaw_core::serde_json::{Value, json};
use meclaw_core::{CellOutput, OutputSink, Path, Uuid};

/// The hop keys a partial carries, with their JSON type. Kept apart from
/// [`crate::llm::output::HOP_KEYS`]: only a cell with `partial_unit` on writes
/// them, so only such a cell has to declare them (the template sweep
/// `gh1410_a_partial_llm_cell_declares_the_partial_lane`).
pub const PARTIAL_HOP_KEYS: &[(&str, &str)] = &[
    ("route", "string"),
    ("seq", "number"),
    ("final", "boolean"),
    ("partial_of", "string"),
];

/// The `route` value of a partial.
pub const PARTIAL_ROUTE: &str = "partial";

/// Sentence terminators.
fn is_terminator(c: char) -> bool {
    matches!(c, '.' | '!' | '?' | '…')
}

/// What may close a sentence after its terminator (`"Yes."` -> `."`).
fn is_closer(c: char) -> bool {
    matches!(c, '"' | '\'' | '”' | '’' | '»' | '›' | ')' | ']' | '}')
}

/// The text a sanitizer downstream would rewrite: a harmony token or a
/// Private-Use-Area codepoint (provider annotations, `sanitize.rs`).
fn would_be_rewritten(text: &str) -> bool {
    text.contains("<|") || text.chars().any(|c| ('\u{E000}'..='\u{F8FF}').contains(&c))
}

/// The pure accumulator: visible text in, the cumulative prefix to emit out.
#[derive(Debug)]
pub(crate) struct Sentences {
    text: String,
    /// Byte length of the last emitted prefix.
    emitted: usize,
    min_chars: usize,
    halted: bool,
}

impl Sentences {
    pub(crate) fn new(min_chars: u32) -> Self {
        Self {
            text: String::new(),
            emitted: 0,
            min_chars: min_chars as usize,
            halted: false,
        }
    }

    /// Feed one visible text delta. `Some` is the new cumulative prefix when
    /// the delta completed a boundary that adds at least `min_chars`.
    pub(crate) fn push(&mut self, delta: &str) -> Option<String> {
        if self.halted {
            return None;
        }
        // Review M5: nothing before the first non-whitespace character
        // counts or goes out (`read_harmony` trims the answer it recovers).
        let delta = if self.text.is_empty() {
            delta.trim_start()
        } else {
            delta
        };
        if delta.is_empty() {
            return None;
        }
        // Review M7: only the new delta is scanned, with the last character
        // before it for a `<|` split over two deltas -- not the whole text
        // on every delta (quadratic over the answer).
        let scan = self.text.char_indices().next_back().map_or(0, |(i, _)| i);
        self.text.push_str(delta);
        if would_be_rewritten(&self.text[scan..]) {
            self.halted = true;
            return None;
        }
        self.cut(false)
    }

    /// The visible text ends here for now (a tool call starts): a sentence
    /// that ends exactly at the end counts as complete.
    pub(crate) fn close(&mut self) -> Option<String> {
        if self.halted {
            return None;
        }
        self.cut(true)
    }

    /// A continuation (`length`) goes on from `so_far`, the parsed text of
    /// the answer up to the cut. Partials go on only when every partial so
    /// far is still its prefix.
    pub(crate) fn rebase(&mut self, so_far: &str) {
        // Review M5: the partials count from the first non-whitespace
        // character, so the text so far does too.
        let so_far = so_far.trim_start();
        if so_far.starts_with(&self.text[..self.emitted]) && !would_be_rewritten(so_far) {
            self.text = so_far.to_string();
        } else {
            self.halted = true;
        }
    }

    fn cut(&mut self, at_end: bool) -> Option<String> {
        let end = last_boundary(&self.text, self.emitted, at_end)?;
        if self.text[self.emitted..end].chars().count() < self.min_chars {
            return None;
        }
        self.emitted = end;
        Some(self.text[..end].to_string())
    }
}

/// The byte index just after the last sentence boundary of `text` at or
/// after `from`: a terminator, its closers, then whitespace -- or the end of
/// the text when `at_end`.
fn last_boundary(text: &str, from: usize, at_end: bool) -> Option<usize> {
    let tail = &text[from..];
    let mut found = None;
    let mut chars = tail.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if !is_terminator(c) {
            continue;
        }
        // Review M4: an ordinal (`3.` before a month) or the parts of
        // `e. g.` -- a `.` after a run of digits or after a one-letter token
        // ends no sentence.
        if c == '.' && is_ordinal_or_initial(&text[..from + i]) {
            continue;
        }
        let mut end = i + c.len_utf8();
        while let Some(&(j, n)) = chars.peek() {
            if is_terminator(n) || is_closer(n) {
                end = j + n.len_utf8();
                chars.next();
            } else {
                break;
            }
        }
        match chars.peek() {
            Some(&(_, n)) if n.is_whitespace() => found = Some(from + end),
            None if at_end => found = Some(from + end),
            _ => {}
        }
    }
    found
}

/// Whether the token right before a `.` (the alphanumeric run that ends
/// `before`) is a run of digits or one letter.
fn is_ordinal_or_initial(before: &str) -> bool {
    let token: Vec<char> = before
        .chars()
        .rev()
        .take_while(|c| c.is_alphanumeric())
        .collect();
    !token.is_empty()
        && (token.iter().all(char::is_ascii_digit)
            || (token.len() == 1 && token[0].is_alphabetic()))
}

/// What the partials of one call cost, for the latency line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PartialStats {
    /// Milliseconds from `handle()` entry to the first delta of the model.
    pub first_token_ms: Option<u64>,
    /// Milliseconds from `handle()` entry to the first partial.
    pub first_partial_ms: Option<u64>,
    /// How many partials left the cell.
    pub partials: u64,
}

/// The tap the wires feed while they read: it owns the accumulator and emits
/// each partial through the sink of the message being answered.
pub(crate) struct PartialTap<'a> {
    sink: &'a OutputSink,
    target: Path,
    partial_of: Uuid,
    sentences: Sentences,
    start: tokio::time::Instant,
    stats: PartialStats,
}

impl<'a> PartialTap<'a> {
    /// A tap for the answer to message `partial_of`, emitting to `target`;
    /// `start` is the `handle()` entry the latency figures count from.
    pub(crate) fn new(
        sink: &'a OutputSink,
        target: Path,
        partial_of: Uuid,
        min_chars: u32,
        start: tokio::time::Instant,
    ) -> Self {
        Self {
            sink,
            target,
            partial_of,
            sentences: Sentences::new(min_chars),
            start,
            stats: PartialStats::default(),
        }
    }

    fn ms(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// Any delta of the model arrived (text, thinking or a tool call).
    pub(crate) fn token(&mut self) {
        if self.stats.first_token_ms.is_none() {
            self.stats.first_token_ms = Some(self.ms());
        }
    }

    /// A visible text delta.
    pub(crate) async fn text(&mut self, delta: &str) {
        self.token();
        if let Some(text) = self.sentences.push(delta) {
            self.emit(text).await;
        }
    }

    /// The visible text stops for a tool call.
    pub(crate) async fn close_text(&mut self) {
        if let Some(text) = self.sentences.close() {
            self.emit(text).await;
        }
    }

    /// A `length` continuation goes on from `so_far`.
    pub(crate) fn rebase(&mut self, so_far: &str) {
        self.sentences.rebase(so_far);
    }

    pub(crate) fn stats(&self) -> PartialStats {
        self.stats
    }

    async fn emit(&mut self, text: String) {
        self.stats.partials += 1;
        if self.stats.first_partial_ms.is_none() {
            self.stats.first_partial_ms = Some(self.ms());
        }
        let content = partial_body(self.stats.partials, &self.partial_of, &text);
        // A closed sink means the colony is going down; the answer path
        // finds the same and has nothing to do either.
        let _ = self
            .sink
            .push(CellOutput {
                target: self.target.clone(),
                content,
            })
            .await;
    }
}

/// The body of partial number `seq`: the lane header and one assistant text
/// message. No `finish_reason`, no usage -- an edge on `finish_reason` never
/// fires on a partial.
pub(crate) fn partial_body(seq: u64, partial_of: &Uuid, text: &str) -> Value {
    json!({
        "header": {
            "route": PARTIAL_ROUTE,
            "seq": seq,
            "final": false,
            "partial_of": partial_of.to_string(),
        },
        "messages": [{"origin": "assistant", "type": "text", "text": text}],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(min: u32, deltas: &[&str]) -> Vec<String> {
        let mut s = Sentences::new(min);
        deltas.iter().filter_map(|d| s.push(d)).collect()
    }

    /// `PARTIAL_HOP_KEYS` is what a partial writes, no more and no less, in
    /// the declared types (the template sweep reads the constant).
    #[test]
    fn a_partial_writes_exactly_the_partial_hop_keys() {
        let body = partial_body(3, &Uuid::now_v7(), "Hello.");
        let header = body["header"].as_object().unwrap();
        assert_eq!(header.len(), PARTIAL_HOP_KEYS.len(), "{body}");
        for (key, ty) in PARTIAL_HOP_KEYS {
            let v = &header[*key];
            let fits = match *ty {
                "string" => v.is_string(),
                "number" => v.is_number(),
                "boolean" => v.is_boolean(),
                other => panic!("{key}: unknown type {other}"),
            };
            assert!(fits, "{key} is declared {ty} but written as {v}");
        }
        assert_eq!(body["header"]["route"], PARTIAL_ROUTE);
        assert_eq!(body["header"]["final"], false);
    }

    #[test]
    fn a_boundary_needs_whitespace_after_it() {
        assert_eq!(
            feed(1, &["Sure, gladly.", " Let me", " look."]),
            vec!["Sure, gladly."]
        );
    }

    #[test]
    fn a_number_and_a_url_are_no_boundary() {
        assert!(feed(1, &["It is 3.5 degrees on example.com today"]).is_empty());
    }

    #[test]
    fn closers_stay_with_their_sentence() {
        assert_eq!(
            feed(1, &["He said \"Yes.\" Then"]),
            vec!["He said \"Yes.\""]
        );
        assert_eq!(feed(1, &["Really?! Yes"]), vec!["Really?!"]);
    }

    #[test]
    fn one_delta_with_two_sentences_is_one_partial() {
        assert_eq!(
            feed(1, &["One is here. Two is here. Three"]),
            vec!["One is here. Two is here."]
        );
    }

    #[test]
    fn a_short_sentence_joins_the_next() {
        assert_eq!(
            feed(12, &["Yes. ", "Let me look it up. ", "Good"]),
            vec!["Yes. Let me look it up."]
        );
    }

    #[test]
    fn the_end_counts_only_when_closed() {
        let mut s = Sentences::new(1);
        assert_eq!(s.push("Let me look it up."), None);
        assert_eq!(s.close(), Some("Let me look it up.".to_string()));
        assert_eq!(s.close(), None, "nothing new, nothing twice");
    }

    #[test]
    fn a_harmony_token_stops_the_partials() {
        assert!(feed(1, &["<|channel|>x. More. "]).is_empty());
        let mut s = Sentences::new(1);
        assert!(s.push("Good. ").is_some());
        assert!(s.push("\u{E200}cite\u{E201} Onwards. ").is_none());
        assert!(s.push("And more. ").is_none());
    }

    /// Review M1: an abbreviation without whitespace inside is no boundary.
    #[test]
    fn an_abbreviation_without_a_space_is_no_boundary() {
        assert!(feed(1, &["Take z.B.x and go"]).is_empty());
    }

    /// Review M4: a `.` right after a run of digits (an ordinal before a
    /// month) or after a one-letter token (the parts of `e. g.`) is no
    /// boundary -- a structural rule, no word list; a longer token (`Dr.`)
    /// still is one. English carriers for the two cases of the review (the
    /// export bans foreign-language test literals).
    #[test]
    fn an_ordinal_and_a_one_letter_token_are_no_boundary() {
        assert!(feed(1, &["We meet on the 3. November"]).is_empty());
        assert!(feed(1, &["Take e. g. the bus"]).is_empty());
        assert_eq!(
            feed(1, &["We meet on the 3. November. Then"]),
            vec!["We meet on the 3. November."]
        );
        assert_eq!(feed(1, &["Ask the Dr. about it"]), vec!["Ask the Dr."]);
    }

    /// Review M5: leading whitespace neither counts nor goes out -- a
    /// partial stays a prefix of an answer the harmony recovery trimmed.
    #[test]
    fn leading_whitespace_is_neither_counted_nor_emitted() {
        assert_eq!(feed(1, &["\n ", " Hello. ", "More"]), vec!["Hello."]);
        assert_eq!(feed(6, &["\n\n\n\n\nHi. ", "More"]), Vec::<String>::new());
        let mut s = Sentences::new(1);
        assert_eq!(s.push("  One. Tw"), Some("One.".to_string()));
        s.rebase("  One. Two");
        assert_eq!(s.push(" three. "), Some("One. Two three.".to_string()));
    }

    /// Review M7: the rewrite check reads only the new delta, with one
    /// character of overlap -- a `<|` split over two deltas still stops.
    #[test]
    fn a_harmony_token_split_over_two_deltas_still_stops() {
        let mut s = Sentences::new(1);
        assert!(s.push("Good. Then <").is_some());
        assert!(s.push("|channel|> Onwards. ").is_none());
        assert!(s.push("And more. ").is_none());
    }

    #[test]
    fn a_rebase_keeps_the_prefix_or_stops() {
        let mut s = Sentences::new(1);
        assert_eq!(s.push("One. Tw"), Some("One.".to_string()));
        s.rebase("One. Two");
        assert_eq!(s.push(" three. "), Some("One. Two three.".to_string()));
        let mut t = Sentences::new(1);
        assert!(t.push("One. ").is_some());
        t.rebase("Other");
        assert!(t.push(" onwards. ").is_none());
    }
}

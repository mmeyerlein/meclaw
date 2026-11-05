//! GH #1411 (D53 X2): the first sentence of an answer, sent before the rest.
//!
//! An `llm` cell with `partial_unit: "sentence"` emits its answer on the
//! `partial` lane while it is still being written: one emission per sentence
//! boundary, each carrying the text from the start of the answer up to that
//! boundary (cumulative, always a prefix of the final answer). A Telegram
//! connector with `partial_delivery: "first_sentence"` turns that into
//! exactly one early message per turn:
//!
//! - the FIRST `partial` of a turn goes out as a message at once;
//! - every later `partial` of the same turn is ignored -- a chat message cannot
//!   be extended without `editMessageText`, and a message per sentence would be
//!   more messages than the turn has without partials;
//! - the next answer of the chat sends only what the early message did not
//!   show: the answer minus the sent prefix, nothing when that is empty. An
//!   answer that does not begin with the prefix (a quick bridge sentence was
//!   shown, then the brain answers) goes out whole;
//! - an INTERIM answer (`hop.interim == "1"`, the dispatcher's bridge sentence
//!   beside a tool call) is cut the same way but does not end the turn: the
//!   final answer still follows, and the partials of the model that writes it
//!   must not open a second early message;
//! - an error after partials (`hop.finish_reason == "error"` or an
//!   `hop.error_code`) appends nothing of the unseen rest and leaves the
//!   early message recorded: the error may be the other branch's, whose
//!   answer still sends only the rest (OR-D53.X2.12).
//!
//! The invariant: at most two messages per turn from the partial path (the
//! early message and the rest), and no sentence twice.
//!
//! **Correlation (OR-D53.X2.1).** The state is keyed by `chat_id` alone and
//! ends with the chat's next final answer. A new user message does not end it
//! (OR-D53.X2.11): the old turn's answer may still come after it and must send
//! only its rest; while the entry stands no second early message opens; an
//! answer that never comes leaves the entry to the next final answer, which
//! does not begin with the shown text and goes whole. `turn_id` is not read:
//! it is minted by the channel, the collector and the member on different legs
//! (`tests/724_the_turn_id_a_channel_stamps_reaches_the_answer.rs`), so a
//! partial of the model and the answer of the collector need not carry the
//! same value; and `partial_of` names the model's input message, which the
//! answer at this connector does not carry. At most one entry per chat,
//! removed with the next final answer, so the map never grows over turns.

use meclaw_core::JsonValue;
use std::collections::HashMap;

/// `params.partial_delivery` of a Telegram connector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PartialDelivery {
    /// `"off"` (default): a `partial` is dropped silently (OR-D53.X2.3: the
    /// member wires the lane always, a sentence each would be a message
    /// each); everything else is byte-identical to before.
    #[default]
    Off,
    /// `"first_sentence"`: the first partial of a turn goes out at once, the
    /// final answer sends only the rest.
    FirstSentence,
}

impl PartialDelivery {
    /// Reads `partial_delivery` out of a params object. Absent → `Off`; any
    /// other value than the two names is refused by name.
    pub fn parse(obj: &serde_json::Map<String, JsonValue>) -> Result<Self, String> {
        match obj.get("partial_delivery") {
            None => Ok(Self::Off),
            Some(v) => match v.as_str() {
                Some("off") => Ok(Self::Off),
                Some("first_sentence") => Ok(Self::FirstSentence),
                _ => Err(format!(
                    "partial_delivery: \"off\" or \"first_sentence\", got {v}"
                )),
            },
        }
    }
}

/// The early message of each chat whose turn is still open: the text it
/// showed, which the next answer is cut by.
#[derive(Debug, Default)]
pub struct PartialTurns {
    shown: HashMap<i64, String>,
}

impl PartialTurns {
    /// Whether the chat's turn has not shown an early message yet.
    pub fn is_first(&self, chat_id: i64) -> bool {
        !self.shown.contains_key(&chat_id)
    }

    /// The early message of `chat_id` went out with `text`.
    pub fn record(&mut self, chat_id: i64, text: &str) {
        self.shown.insert(chat_id, text.to_string());
    }

    /// What an answer of `chat_id` still has to send. Without an early message
    /// that is the whole answer; with one, the answer minus the shown prefix
    /// (leading whitespace dropped, `""` = send nothing) -- or the whole answer
    /// when it does not begin with that prefix. Both sides are compared
    /// without leading/trailing whitespace of the prefix. A final answer ends the turn;
    /// an interim one keeps it open.
    pub fn answer<'a>(&mut self, chat_id: i64, text: &'a str, interim: bool) -> &'a str {
        let Some(shown) = self.shown.get(&chat_id) else {
            return text;
        };
        let out = match text.trim_start().strip_prefix(shown.trim()) {
            Some(rest) => rest.trim_start(),
            None => text,
        };
        if !interim {
            self.shown.remove(&chat_id);
        }
        out
    }

    /// Number of chats with an open early message (tests: the map is bounded).
    pub fn len(&self) -> usize {
        self.shown.len()
    }

    /// Whether no chat has an open early message.
    pub fn is_empty(&self) -> bool {
        self.shown.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_param_reads_two_names_and_refuses_the_rest() {
        let parse = |v: JsonValue| PartialDelivery::parse(v.as_object().unwrap());
        assert_eq!(parse(json!({})), Ok(PartialDelivery::Off));
        assert_eq!(
            parse(json!({"partial_delivery": "off"})),
            Ok(PartialDelivery::Off)
        );
        assert_eq!(
            parse(json!({"partial_delivery": "first_sentence"})),
            Ok(PartialDelivery::FirstSentence)
        );
        assert!(parse(json!({"partial_delivery": "sentence"})).is_err());
        assert!(parse(json!({"partial_delivery": true})).is_err());
    }

    #[test]
    fn the_rest_is_the_answer_minus_what_was_shown() {
        let mut t = PartialTurns::default();
        t.record(1, "Yes, sure. ");
        assert_eq!(
            t.answer(1, "Yes, sure. It is 12 degrees.", false),
            "It is 12 degrees."
        );
        assert!(t.is_empty(), "a final answer ends the turn");
        assert_eq!(
            t.answer(1, "New.", false),
            "New.",
            "no early message: whole"
        );
    }

    #[test]
    fn an_interim_keeps_the_turn_and_a_foreign_answer_goes_whole() {
        let mut t = PartialTurns::default();
        t.record(7, "Moment.");
        assert_eq!(t.answer(7, "Moment.", true), "");
        assert!(!t.is_first(7), "the interim did not end the turn");
        assert_eq!(t.answer(7, "It is 12 degrees.", false), "It is 12 degrees.");
        assert!(t.is_first(7));
    }

    #[test]
    fn whitespace_around_the_shown_prefix_does_not_repeat_it() {
        let mut t = PartialTurns::default();
        t.record(1, "Yes, sure.");
        assert_eq!(
            t.answer(1, "\n Yes, sure. It is 12 degrees.\n", false),
            "It is 12 degrees.\n"
        );
        t.record(2, "\nYes, sure. ");
        assert_eq!(
            t.answer(2, "Yes, sure. It is 12 degrees.", false),
            "It is 12 degrees."
        );
    }
}

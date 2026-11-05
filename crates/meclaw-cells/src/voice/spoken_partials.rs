//! What a cascade call has already said of an answer that is still being
//! written (GH #1411, `speak_partials: "1"`).
//!
//! The `llm` cell can hand out the text of an answer sentence by sentence on
//! its `partial` lane (lane contract X1): every partial carries the answer
//! **cumulated** from its start up to the last sentence boundary, each one is a
//! prefix of the next and of the finished answer, and all partials of a call
//! leave the cell before its finished answer or its error. A cascade call that
//! speaks them hands the first sentence to the synthesis while the model is
//! still writing the second; this type is the bookkeeping that keeps a sentence
//! from being said twice.
//!
//! **Correlation is by prefix, not by an id.** The finished answer is unchanged
//! by the lane (it carries no `partial_of`), and `turn_id` is stamped by
//! different cells on different paths (`tests/724_…`), so the only thing a
//! partial and its answer reliably share is the text itself. An open answer is
//! the text spoken so far; a partial that starts with it says the rest, an
//! answer that starts with it is spoken from where the partials stopped, and an
//! answer that starts with none of them — the quick cell's interim sentence,
//! followed by the brain's answer — is a different answer and is spoken whole.
//! Both sides are compared with leading and trailing whitespace trimmed and
//! nothing else changed: one path may hand the text over trimmed, the other
//! not, and the same sentence must still be recognised.
//!
//! **Bounded.** At most [`OPEN_ANSWERS_KEPT`] answers are open per call: two
//! are the real case (the interim sentence and the brain's answer may overlap,
//! because the partial edges go straight to the edge while the interim goes
//! through splitter, dispatcher and collector), the rest is slack for an answer
//! whose finished form never arrived. The oldest goes first.
//!
//! **Turn boundary.** Every open answer ends with the caller's next finished
//! turn ([`SpokenPartials::turn_ended`]): it belongs to a turn the caller has
//! moved on from, and kept it would read a later bridge sentence with the same
//! words — or a later answer that starts with them — as its own replay and
//! swallow it. The whole state lives on the call's `SessionState` and dies
//! with the connection.
//!
//! **Interruption.** [`SpokenPartials::cut`] is called where the cell's
//! existing barge-in fires; an answer the caller talked over keeps receiving
//! its partials (so its later text still matches) but says nothing more, and
//! its finished form is dropped — exactly what happened to a whole answer whose
//! synthesis was cancelled before this lane existed. Its stream may well run
//! on past the caller's turn; text cannot tell its next partial from a new
//! answer then, its `hop.partial_of` can (the id of the `llm` call, lane
//! contract X1). A cut answer keeps that id across the turn boundary — at most
//! [`OPEN_ANSWERS_KEPT`] of them, the oldest goes first — and a later partial
//! carrying it stays silent; its text takes no part in the prefix match any
//! more, so it never swallows another answer's sentence. The finished form
//! carries no `partial_of` and is read as the answers before this lane read
//! it (#1411 Fix-Runde 2).
//!
//! **Errors.** An error names no branch and touches no open answer: the
//! branch that failed sends nothing after it, the other one goes on to its
//! end, and whatever is still open ends with the caller's next turn.

/// How many answers one call keeps open at most.
pub const OPEN_ANSWERS_KEPT: usize = 4;

/// One answer this call has started to say.
#[derive(Debug, Clone)]
struct OpenAnswer {
    /// The text received so far (the last partial, whitespace trimmed at
    /// both ends), before `speak_plain`.
    text: String,
    /// Whether the caller interrupted it.
    cut: bool,
    /// The `hop.partial_of` of its partials, when they carried one.
    partial_of: Option<String>,
}

/// The open answers of one cascade call. Empty unless `speak_partials` is on.
#[derive(Debug, Clone, Default)]
pub struct SpokenPartials {
    open: Vec<OpenAnswer>,
    /// The `partial_of` of answers the caller interrupted in a turn that has
    /// ended, oldest first: their later partials stay silent.
    cut_calls: Vec<String>,
}

impl SpokenPartials {
    /// The open answer `text` continues: the longest open text it starts with.
    fn continued_by(&self, text: &str) -> Option<usize> {
        self.open
            .iter()
            .enumerate()
            .filter(|(_, a)| text.starts_with(a.text.as_str()))
            .max_by_key(|(_, a)| a.text.len())
            .map(|(i, _)| i)
    }

    /// A partial arrived, `partial_of` its `hop.partial_of`. Returns what is
    /// new in it and still to be said, or `None` when there is nothing to say
    /// (no growth, a replay of an older partial, or an answer the caller
    /// interrupted — in this turn or one that has ended).
    pub fn partial(&mut self, text: &str, partial_of: Option<&str>) -> Option<String> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        if partial_of.is_some_and(|id| self.cut_calls.iter().any(|c| c == id)) {
            return None;
        }
        if let Some(i) = self.continued_by(text) {
            let answer = &mut self.open[i];
            let grown = text[answer.text.len()..].trim_start().to_string();
            answer.text = text.to_string();
            if let Some(id) = partial_of {
                answer.partial_of = Some(id.to_string());
            }
            return (!answer.cut && !grown.trim().is_empty()).then_some(grown);
        }
        // An older partial of an answer that has already grown past it: a
        // replay, said already.
        if self.open.iter().any(|a| a.text.starts_with(text)) {
            return None;
        }
        if self.open.len() >= OPEN_ANSWERS_KEPT {
            self.open.remove(0);
        }
        self.open.push(OpenAnswer {
            text: text.to_string(),
            cut: false,
            partial_of: partial_of.map(str::to_string),
        });
        Some(text.to_string())
    }

    /// A finished answer (or an interim sentence) arrived. Returns what is
    /// left to say: the text after the spoken prefix, the whole text when no
    /// open answer is its prefix, `None` when nothing is left or the caller
    /// interrupted it. The answer it closes is no longer open.
    pub fn answer(&mut self, text: &str) -> Option<String> {
        let trimmed = text.trim();
        let Some(i) = self.continued_by(trimmed) else {
            return Some(text.to_string());
        };
        let answer = self.open.remove(i);
        if answer.cut {
            return None;
        }
        let rest = trimmed[answer.text.len()..].trim_start();
        (!rest.trim().is_empty()).then(|| rest.to_string())
    }

    /// The caller interrupted: every answer open now says nothing more.
    pub fn cut(&mut self) {
        for answer in &mut self.open {
            answer.cut = true;
        }
    }

    /// An error arrived for this call. No open answer is touched: the error
    /// names no branch, the branch that failed sends nothing after it, and
    /// the other one goes on to its end — neither said twice nor cut short.
    /// Returns whether anything was open, i.e. whether partials were already
    /// spoken.
    pub fn fail(&self) -> bool {
        !self.open.is_empty()
    }

    /// The caller finished a turn: every open answer ends here. A cut one
    /// leaves its `partial_of` behind, so the rest of its stream stays silent.
    pub fn turn_ended(&mut self) {
        for answer in self.open.drain(..) {
            let Some(id) = answer.partial_of.filter(|_| answer.cut) else {
                continue;
            };
            if self.cut_calls.contains(&id) {
                continue;
            }
            if self.cut_calls.len() >= OPEN_ANSWERS_KEPT {
                self.cut_calls.remove(0);
            }
            self.cut_calls.push(id);
        }
    }

    /// How many answers are open.
    pub fn open_count(&self) -> usize {
        self.open.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn growth_is_said_once_and_the_rest_after_the_answer() {
        let mut s = SpokenPartials::default();
        assert_eq!(
            s.partial("Hello everyone.", None).as_deref(),
            Some("Hello everyone.")
        );
        assert_eq!(
            s.partial("Hello everyone.", None).as_deref(),
            None,
            "no growth"
        );
        assert_eq!(
            s.partial("Hello everyone. The weather is fine.", None)
                .as_deref(),
            Some("The weather is fine.")
        );
        assert_eq!(
            s.partial("Hello everyone.", None).as_deref(),
            None,
            "a replay"
        );
        assert_eq!(
            s.answer("Hello everyone. The weather is fine. See you soon")
                .as_deref(),
            Some("See you soon")
        );
        assert_eq!(s.open_count(), 0, "the answer closed it");
    }

    #[test]
    fn a_foreign_answer_is_said_whole_and_an_interim_not_again() {
        let mut s = SpokenPartials::default();
        s.partial("Hold on, I will check.", None);
        s.partial("Tomorrow will be sunny.", None);
        assert_eq!(s.answer("Hold on, I will check.").as_deref(), None);
        assert_eq!(
            s.answer("Tomorrow will be sunny. At twenty degrees.")
                .as_deref(),
            Some("At twenty degrees.")
        );
        assert_eq!(
            s.answer("Something else.").as_deref(),
            Some("Something else.")
        );
    }

    #[test]
    fn a_cut_answer_says_nothing_more_and_the_state_is_bounded() {
        let mut s = SpokenPartials::default();
        s.partial("Eins.", None);
        s.cut();
        assert_eq!(s.partial("Eins. Zwei.", None).as_deref(), None);
        assert_eq!(s.answer("Eins. Zwei. Drei.").as_deref(), None);
        for n in 0..10 {
            s.partial(&format!("Antwort {n}."), None);
        }
        assert_eq!(s.open_count(), OPEN_ANSWERS_KEPT);
        assert!(s.fail());
        assert_eq!(
            s.answer("Antwort 9. Zehn.").as_deref(),
            Some("Zehn."),
            "an error cuts nothing"
        );
        s.turn_ended();
        assert!(!s.fail());
        assert_eq!(
            s.partial(" Eins. ", None).as_deref(),
            Some("Eins."),
            "a new turn"
        );
    }

    #[test]
    fn a_cut_answer_keeps_its_call_id_across_the_turn_and_nothing_else() {
        let mut s = SpokenPartials::default();
        s.partial("Moment.", Some("a"));
        s.cut();
        s.turn_ended();
        assert_eq!(s.open_count(), 0);
        assert_eq!(s.partial("Moment. Zwei.", Some("a")).as_deref(), None);
        assert_eq!(s.partial("Moment.", Some("b")).as_deref(), Some("Moment."));
        assert_eq!(s.answer("Moment. Drei.").as_deref(), Some("Drei."));
        for n in 0..10 {
            s.partial(&format!("Satz {n}."), Some(&format!("c{n}")));
            s.cut();
            s.turn_ended();
        }
        assert_eq!(s.cut_calls.len(), OPEN_ANSWERS_KEPT);
        assert_eq!(
            s.partial("Moment. Vier.", Some("a")).as_deref(),
            Some("Moment. Vier.")
        );
    }
}

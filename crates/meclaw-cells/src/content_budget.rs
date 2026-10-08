//! R-IG-1 (GH #1085): how much of a model's window one piece of content may
//! take, and how a cut says so.
//!
//! Three places may bound content on its way to or from a model: the catalog
//! row (`input_soft` / `input_hard`, pushed into the `llm` cell as package
//! keys), the curator, and the carrier. A producer -- a tool cell, a cell that
//! builds a request -- holds no number of its own. It takes a share of the
//! window the catalog row states ([`budget_chars`]) and cuts visibly
//! ([`clip`], [`clip_bytes`]) with the one mark every cell and template uses:
//!
//! `...[cut: <shown> of <total> <unit> shown; <hint>]`
//!
//! Without a known window a producer delivers whole. The only bound left then
//! is the carrier ([`CARRIER_MAX_BYTES`]), and the carrier refuses with the
//! size and the bound; it never cuts.
//!
//! The two functions exist once more in Python, for the templates, as the
//! block `content-budget v1`; both sides compute the same numbers.

use meclaw_core::Headers;
use meclaw_core::serde_json::{Map, Value};
use std::borrow::Cow;

/// Characters (or bytes) counted as one token of the receiving model.
///
/// Deliberately low: a token is about four bytes of English, and three
/// counts JSON, German and code as well, so a budget never runs past the
/// window. The `llm` cell's window estimate counts the same three bytes
/// (`llm::window`), and so does the Python block of the templates.
pub const CHARS_PER_TOKEN: u64 = 3;

/// The share of the window one tool result may take (R-IG-1 share table:
/// "one tool result to the model" = 10 %).
pub const SHARE_TOOL_RESULT: f64 = 0.10;

/// The carrier ceiling for one piece of content entering the colony from
/// outside, in bytes: what a single colony message may carry in from an
/// external source at all.
///
/// Derived from the carrier, not chosen for the content: it is the largest
/// HTTP body any mount of the colony may be declared to take
/// (`proxy::webhook::params::MAX_BODY_KB_LIMIT`, 4096 KiB). A fetched body, a
/// search response or a command's output enters the colony the same way --
/// one external body into one message -- so it gets the same ceiling. At
/// [`CHARS_PER_TOKEN`] that is about 1.4 million tokens, above the window of
/// every model a catalog row names today, so nothing a model could take is
/// refused by it. Above it the content is refused with its size, never cut.
pub const CARRIER_MAX_BYTES: usize =
    (crate::proxy::webhook::params::MAX_BODY_KB_LIMIT as usize) * 1024;

/// Characters a producer may hand a model whose usable window is
/// `input_soft` tokens (the catalog row, cut by the curator), for a section
/// that takes `share` of that window. `None` when no window is known
/// (`input_soft` 0): the producer then delivers its content whole.
///
/// Same arithmetic as the Python `budget_chars` of the templates:
/// `int(soft * share * CHARS_PER_TOKEN)`. The same number serves as a byte
/// budget ([`clip_bytes`]): the window estimate counts bytes at the same rate.
pub fn budget_chars(input_soft: u64, share: f64) -> Option<usize> {
    if input_soft == 0 {
        return None;
    }
    // `as usize` saturates: a negative or NaN share gives 0, as `int()` of
    // a non-positive product does in Python (clamped by `clip`'s max(0, ..)).
    Some((input_soft as f64 * share * CHARS_PER_TOKEN as f64) as usize)
}

/// The cut mark: what was shown, of how much, in which unit, and how to get
/// the rest (`hint`: a tool and its argument, or `budget of the window`).
pub fn cut_mark(shown: usize, total: usize, unit: &str, hint: &str) -> String {
    format!("...[cut: {shown} of {total} {unit} shown; {hint}]")
}

/// `text` whole when it fits `limit` characters (`None`: no limit); else its
/// first `limit` characters and a mark naming what was shown, the total and
/// how to get the rest. Characters are Unicode scalar values, as Python's
/// `len()` counts them.
pub fn clip<'a>(text: &'a str, limit: Option<usize>, hint: &str) -> Cow<'a, str> {
    let Some(limit) = limit else {
        return Cow::Borrowed(text);
    };
    let total = text.chars().count();
    if total <= limit {
        return Cow::Borrowed(text);
    }
    let end = text
        .char_indices()
        .nth(limit)
        .map_or(text.len(), |(i, _)| i);
    Cow::Owned(format!(
        "{}{}",
        &text[..end],
        cut_mark(limit, total, "chars", hint)
    ))
}

/// The byte variant of [`clip`]: `text` whole when it fits `limit` bytes;
/// else at most `limit` bytes of its head, cut back to a char boundary, and
/// the mark with `unit = bytes` (shown = the bytes actually kept).
pub fn clip_bytes<'a>(text: &'a str, limit: Option<usize>, hint: &str) -> Cow<'a, str> {
    let Some(limit) = limit else {
        return Cow::Borrowed(text);
    };
    if text.len() <= limit {
        return Cow::Borrowed(text);
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Cow::Owned(format!(
        "{}{}",
        &text[..end],
        cut_mark(end, text.len(), "bytes", hint)
    ))
}

/// The window of the model a message is for, in tokens, when the message
/// says so: `input_soft` in the persistent `context` (the curator puts it on
/// every request to a producer; an edge may set it, OR-IG-8) or in the `hop`.
/// The `llm` cell stamps its catalog row on every answer, a tool-call bundle
/// included, but a hop lives for one emission: the stamp reaches a tool
/// directly behind the `llm` cell, or behind a dispatcher that hands it on to
/// each call it emits (`dispatcher@1.2.3` does, GH #1085); any other cell in
/// between ends it. Both present: the smaller one -- the content must fit the
/// tighter window. `None`: unknown, and the producer delivers whole.
pub fn input_soft_of(headers: &Headers) -> Option<u64> {
    let read = |m: &Map<String, Value>| {
        m.get("input_soft")
            .and_then(|v| {
                v.as_u64().or_else(|| {
                    v.as_f64()
                        .filter(|f| f.is_finite() && *f >= 1.0)
                        .map(|f| f as u64)
                })
            })
            .filter(|n| *n > 0)
    };
    match (read(&headers.context), read(&headers.hop)) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// A tool result over the carrier ceiling: refused whole (`too_long`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarrierRefusal {
    /// The size of the content that would have been sent, in bytes.
    pub bytes: usize,
    /// The ceiling it is over ([`CARRIER_MAX_BYTES`]).
    pub max: usize,
}

impl CarrierRefusal {
    /// The refusal text: `too_long: <n> > <max> bytes` and why.
    pub fn detail(&self) -> String {
        format!(
            "too_long: {} > {} bytes; the result is over the carrier ceiling and no window \
             budget came with the call to cut it to, so it is refused whole",
            self.bytes, self.max
        )
    }
}

/// What a tool cell hands back as its result text (R-IG-1): cut to `limit`
/// when one is known -- the cell's explicit `max_bytes` first, else the
/// tool-result share of the window the message names ([`input_soft_of`]) --
/// and whole otherwise. `Ok((text, cut))`; `Err` when the text that would be
/// sent is over the carrier ceiling.
pub fn tool_result_text(
    text: String,
    max_bytes: Option<usize>,
    headers: &Headers,
    hint: &str,
) -> Result<(String, bool), CarrierRefusal> {
    let limit = max_bytes
        .or_else(|| input_soft_of(headers).and_then(|soft| budget_chars(soft, SHARE_TOOL_RESULT)));
    let (out, cut) = match clip_bytes(&text, limit, hint) {
        Cow::Borrowed(_) => (text, false),
        Cow::Owned(s) => (s, true),
    };
    if out.len() > CARRIER_MAX_BYTES {
        return Err(CarrierRefusal {
            bytes: out.len(),
            max: CARRIER_MAX_BYTES,
        });
    }
    Ok((out, cut))
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    fn headers(context: Value, hop: Value) -> Headers {
        Headers::from_parts(
            context.as_object().cloned().unwrap_or_default(),
            hop.as_object().cloned().unwrap_or_default(),
        )
    }

    #[test]
    fn budget_chars_is_share_of_the_window_at_three_chars_a_token() {
        assert_eq!(budget_chars(120_000, 0.25), Some(90_000));
        assert_eq!(budget_chars(100_000, SHARE_TOOL_RESULT), Some(30_000));
        assert_eq!(budget_chars(1, 0.5), Some(1));
    }

    #[test]
    fn budget_chars_without_a_window_is_none() {
        assert_eq!(budget_chars(0, 0.5), None);
    }

    #[test]
    fn clip_leaves_text_that_fits_alone() {
        assert!(matches!(
            clip("hello", Some(5), "h"),
            Cow::Borrowed("hello")
        ));
        assert!(matches!(clip("hello", None, "h"), Cow::Borrowed("hello")));
        let big = "x".repeat(1_000_000);
        assert_eq!(clip(&big, None, "h").len(), 1_000_000);
    }

    #[test]
    fn clip_marks_the_cut_with_shown_total_and_hint() {
        let out = clip("abcdefghij", Some(4), "read with offset=4");
        assert_eq!(out, "abcd...[cut: 4 of 10 chars shown; read with offset=4]");
    }

    #[test]
    fn clip_counts_characters_not_bytes() {
        // Five two-byte characters: a char limit of 3 keeps three of them.
        let out = clip("αβγδε", Some(3), "budget of the window");
        assert_eq!(out, "αβγ...[cut: 3 of 5 chars shown; budget of the window]");
    }

    #[test]
    fn clip_bytes_cuts_on_a_char_boundary_and_names_bytes() {
        // "ααα" is six bytes; a 3-byte limit falls inside the second char.
        let out = clip_bytes("ααα", Some(3), "budget of the window");
        assert_eq!(out, "α...[cut: 2 of 6 bytes shown; budget of the window]");
        assert!(matches!(clip_bytes("ααα", Some(6), "h"), Cow::Borrowed(_)));
        assert!(matches!(clip_bytes("ααα", None, "h"), Cow::Borrowed(_)));
    }

    #[test]
    fn input_soft_comes_from_context_or_hop_and_the_smaller_wins() {
        assert_eq!(input_soft_of(&Headers::new()), None);
        assert_eq!(
            input_soft_of(&headers(json!({"input_soft": 8000}), json!({}))),
            Some(8000)
        );
        assert_eq!(
            input_soft_of(&headers(json!({}), json!({"input_soft": 9000}))),
            Some(9000)
        );
        assert_eq!(
            input_soft_of(&headers(
                json!({"input_soft": 8000}),
                json!({"input_soft": 6000})
            )),
            Some(6000)
        );
        assert_eq!(
            input_soft_of(&headers(
                json!({"input_soft": 0}),
                json!({"input_soft": "x"})
            )),
            None
        );
        assert_eq!(
            input_soft_of(&headers(json!({"input_soft": 4000.0}), json!({}))),
            Some(4000)
        );
    }

    /// The lock of the removed 256 KiB tool cap: content over the old value
    /// comes whole when no budget is known.
    #[test]
    fn a_tool_result_over_the_old_cap_comes_whole_without_a_budget() {
        let text = "y".repeat(300 * 1024);
        let (out, cut) = tool_result_text(text.clone(), None, &Headers::new(), "h").unwrap();
        assert!(!cut);
        assert_eq!(out, text);
    }

    #[test]
    fn a_tool_result_over_the_budget_comes_marked_with_its_total() {
        // input_soft 10 000 tokens -> 10 % -> 3 000 bytes.
        let h = headers(json!({}), json!({"input_soft": 10_000}));
        let text = "z".repeat(5_000);
        let (out, cut) = tool_result_text(text, None, &h, "budget of the window").unwrap();
        assert!(cut);
        assert!(out.starts_with(&"z".repeat(3_000)));
        assert!(
            out.ends_with("...[cut: 3000 of 5000 bytes shown; budget of the window]"),
            "{}",
            &out[out.len() - 80..]
        );
    }

    #[test]
    fn an_explicit_max_bytes_wins_over_the_budget() {
        let h = headers(json!({}), json!({"input_soft": 10_000}));
        let (out, cut) = tool_result_text("q".repeat(5_000), Some(100), &h, "h").unwrap();
        assert!(cut);
        assert!(out.contains("[cut: 100 of 5000 bytes shown; h]"));
    }

    #[test]
    fn a_tool_result_over_the_carrier_is_refused_with_its_size() {
        let text = "w".repeat(CARRIER_MAX_BYTES + 1);
        let err = tool_result_text(text, None, &Headers::new(), "h").unwrap_err();
        assert_eq!(err.bytes, CARRIER_MAX_BYTES + 1);
        assert_eq!(err.max, CARRIER_MAX_BYTES);
        assert!(
            err.detail().starts_with(&format!(
                "too_long: {} > {} bytes",
                CARRIER_MAX_BYTES + 1,
                CARRIER_MAX_BYTES
            )),
            "{}",
            err.detail()
        );
    }

    #[test]
    fn the_carrier_ceiling_is_the_colony_ingress_bound() {
        assert_eq!(CARRIER_MAX_BYTES, 4096 * 1024);
    }
}

//! Response sanitation at the translate boundary: provider-internal annotation
//! markers never reach UBF (GH #569).
//!
//! Some providers decorate answer text with inline citation markers built from
//! Private-Use-Area codepoints. The shape observed on a live turn (hexdump of
//! the reported text): `U+E200` `cite` `U+E202` `turn0search0` `U+E201`. The PUA
//! codepoints render as invisible glyphs or boxes, the enclosed `cite` /
//! `turnNsearchM` tokens render as literal junk, and the whole span refers to
//! the provider's own tool-round numbering — it means nothing outside the
//! provider. No provider-internal token may reach a person, so the response
//! translation drops these spans before the text enters an assistant turn.

use std::ops::RangeInclusive;

/// The Unicode Basic Multilingual Plane's Private Use Area.
const PRIVATE_USE_AREA: RangeInclusive<char> = '\u{E000}'..='\u{F8FF}';

/// Opens an annotation span.
const MARKER_START: char = '\u{E200}';

/// Closes an annotation span.
const MARKER_END: char = '\u{E201}';

/// True for a codepoint in the Private Use Area.
fn is_private_use(c: char) -> bool {
    PRIVATE_USE_AREA.contains(&c)
}

/// Strip provider-internal annotation markers from response text (GH #569).
///
/// Rules, in the order the scan applies them:
///
/// * A span from [`MARKER_START`] up to and including the next [`MARKER_END`]
///   falls **with its content** — the enclosed `cite` / `turnNsearchM` tokens
///   and any separator codepoint (`U+E202`) inside it.
/// * **Truncation:** a [`MARKER_START`] whose closing codepoint never arrives
///   (a cut-off response) takes **the rest of the text** with it. Everything
///   after an opened marker is marker content by construction — there is no
///   answer text behind it to save, and keeping it would leak exactly the
///   literal `citeturn0search0` junk this function exists to remove.
/// * Any other Private-Use-Area codepoint is a stray and falls **on its own**;
///   the text around it is untouched.
/// * Everything else is returned byte-identical: no trimming, no whitespace
///   normalisation. A space in front of a dropped marker stays — that is honest
///   and visibly harmless.
///
/// **Limit.** No Private-Use-Area codepoint of any provider survives this, but
/// only the observed `U+E200`/`U+E201` pair carries its enclosed text away with
/// it. A different provider's marker protocol would leave its ASCII payload
/// behind until its opener pair is measured and added — a heuristic that eats
/// everything between any two PUA codepoints would swallow real answer text.
pub(crate) fn strip_provider_annotations(text: &str) -> String {
    if !text.contains(is_private_use) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == MARKER_START {
            // Consume the span, content included, up to and including its
            // closing codepoint. If none arrives, the iterator runs dry and the
            // rest of the text falls with the marker.
            for inner in chars.by_ref() {
                if inner == MARKER_END {
                    break;
                }
            }
        } else if !is_private_use(c) {
            out.push(c);
        }
        // A stray Private-Use-Area codepoint outside a span falls on its own.
    }
    out
}

#[cfg(test)]
mod tests {
    use super::strip_provider_annotations;

    #[test]
    fn a_pua_wrapped_citation_marker_is_dropped_whole() {
        let raw = "It may rain this evening. \u{E200}cite\u{E202}turn0search0\u{E201}";
        assert_eq!(
            strip_provider_annotations(raw),
            "It may rain this evening. "
        );
    }

    #[test]
    fn text_without_markers_is_returned_byte_identical() {
        let raw = "plain text, naïve café, 日本語, and a lone \\ backslash";
        assert_eq!(strip_provider_annotations(raw), raw);
    }

    #[test]
    fn two_markers_and_a_stray_codepoint_all_fall() {
        let raw = "a\u{E200}cite\u{E202}turn0search0\u{E201}b\u{E200}cite\u{E202}turn1search3\u{E201}c\u{E203}";
        assert_eq!(strip_provider_annotations(raw), "abc");
    }

    #[test]
    fn an_unclosed_marker_takes_the_rest_of_the_text() {
        // Truncated response: the closing codepoint never arrives. Everything
        // behind the opener is marker content, so it falls with it.
        let raw = "It rains. \u{E200}cite\u{E202}turn0sea";
        assert_eq!(strip_provider_annotations(raw), "It rains. ");
    }

    #[test]
    fn a_marker_in_the_middle_leaves_both_sides_untouched() {
        let raw = "before \u{E200}cite\u{E202}turn0search0\u{E201} after";
        assert_eq!(strip_provider_annotations(raw), "before  after");
    }
}

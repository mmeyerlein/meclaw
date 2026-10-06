//! The permessage-deflate offer and its answer (RFC 7692 § 5, § 7.1).
//!
//! The server takes the FIRST acceptable `permessage-deflate` offer and answers
//! with one fixed configuration:
//!
//! ```text
//! permessage-deflate; server_no_context_takeover; client_no_context_takeover
//! ```
//!
//! - `server_no_context_takeover`: each compressed message is a deflate stream
//!   of its own. Only frames above [`super::DEFLATE_MIN_BYTES`] are compressed
//!   at all — join chunks and tranches and whole-page resyncs, which share little with the
//!   frame before — so a sliding window kept across messages would buy little
//!   and cost a compressor's state for the life of every socket. A server may
//!   always add this parameter (§ 7.1.1.1).
//! - `client_no_context_takeover`: the server inflates each client message on
//!   its own, too. A server may add it even when the offer lacks it
//!   (§ 7.1.1.2), and the browser then complies.
//!
//! An offer is declined when it asks for something this side cannot honour:
//! `server_max_window_bits` below 15 (the deflate backend in the tree writes
//! with a 32 KiB window and cannot be narrowed), an unknown parameter, or a
//! parameter given twice (§ 7.1 "MUST decline"). `client_max_window_bits`
//! with or without a value is fine: an inflater with the full window reads any
//! narrower one.

/// The answer to an accepted offer, as it goes into `Sec-WebSocket-Extensions`.
pub(crate) const ANSWER: &str =
    "permessage-deflate; server_no_context_takeover; client_no_context_takeover";

/// The answer to an offer that named `server_max_window_bits=15`: the
/// parameter must come back (RFC 7692 § 7.1.2.1; review M-3).
pub(crate) const ANSWER_WINDOW_15: &str = "permessage-deflate; server_no_context_takeover; \
     client_no_context_takeover; server_max_window_bits=15";

/// The answer to the first acceptable permessage-deflate offer among the
/// `Sec-WebSocket-Extensions` values; `None` when there is none.
pub(crate) fn answer<'a>(values: impl IntoIterator<Item = &'a str>) -> Option<&'static str> {
    values
        .into_iter()
        .flat_map(|v| v.split(','))
        .find_map(acceptable)
}

#[cfg(test)]
fn accepts<'a>(values: impl IntoIterator<Item = &'a str>) -> bool {
    answer(values).is_some()
}

/// One offer: `name; param; param=value`; its answer when acceptable.
fn acceptable(offer: &str) -> Option<&'static str> {
    let mut parts = offer.split(';').map(str::trim);
    if !parts
        .next()
        .is_some_and(|name| name.eq_ignore_ascii_case("permessage-deflate"))
    {
        return None;
    }
    let mut seen: Vec<String> = Vec::new();
    for param in parts {
        if param.is_empty() {
            continue;
        }
        let (name, value) = match param.split_once('=') {
            Some((n, v)) => (
                n.trim().to_ascii_lowercase(),
                Some(v.trim().trim_matches('"')),
            ),
            None => (param.to_ascii_lowercase(), None),
        };
        if seen.contains(&name) {
            return None;
        }
        let ok = match name.as_str() {
            "server_no_context_takeover" | "client_no_context_takeover" => value.is_none(),
            "client_max_window_bits" => value.is_none_or(valid_bits),
            // Only the full window: the compressor here cannot narrow it.
            "server_max_window_bits" => value == Some("15"),
            _ => false,
        };
        if !ok {
            return None;
        }
        seen.push(name);
    }
    Some(if seen.iter().any(|n| n == "server_max_window_bits") {
        ANSWER_WINDOW_15
    } else {
        ANSWER
    })
}

fn valid_bits(v: &str) -> bool {
    v.parse::<u8>().is_ok_and(|b| (8..=15).contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_browsers_offer_is_accepted() {
        // Chrome and Firefox.
        assert!(accepts(["permessage-deflate; client_max_window_bits"]));
        // Safari, and the bare form.
        assert!(accepts(["permessage-deflate"]));
        assert!(accepts([
            "permessage-deflate; server_no_context_takeover; client_no_context_takeover"
        ]));
        assert!(accepts(["permessage-deflate; client_max_window_bits=10"]));
    }

    #[test]
    fn no_offer_is_no_extension() {
        assert!(!accepts([]));
        assert!(!accepts(["x-webkit-deflate-frame"]));
    }

    #[test]
    fn an_offer_this_side_cannot_honour_is_declined() {
        assert!(!accepts(["permessage-deflate; server_max_window_bits=10"]));
        assert!(!accepts(["permessage-deflate; unknown_param"]));
        assert!(!accepts([
            "permessage-deflate; client_max_window_bits; client_max_window_bits"
        ]));
        assert!(!accepts(["permessage-deflate; client_max_window_bits=99"]));
    }

    #[test]
    fn a_window_of_15_is_named_in_the_answer() {
        assert_eq!(
            answer(["permessage-deflate; server_max_window_bits=15"]),
            Some(ANSWER_WINDOW_15)
        );
        assert!(ANSWER_WINDOW_15.ends_with("; server_max_window_bits=15"));
        assert_eq!(
            answer(["permessage-deflate; client_max_window_bits"]),
            Some(ANSWER)
        );
    }

    #[test]
    fn offers_over_two_header_lines_and_in_any_case() {
        assert_eq!(
            answer([
                "x-webkit-deflate-frame",
                "Permessage-Deflate; Client_Max_Window_Bits"
            ]),
            Some(ANSWER)
        );
        assert_eq!(
            answer(["PERMESSAGE-DEFLATE; SERVER_MAX_WINDOW_BITS=15"]),
            Some(ANSWER_WINDOW_15)
        );
        assert_eq!(
            answer(["permessage-deflate; server_max_window_bits=14", "foo"]),
            None
        );
    }

    #[test]
    fn a_later_acceptable_offer_is_taken() {
        // § 5.1: offers are listed in preference order; the first one this
        // side can honour wins.
        assert!(accepts([
            "permessage-deflate; server_max_window_bits=9, permessage-deflate"
        ]));
        assert!(accepts([
            "foo",
            "permessage-deflate; client_max_window_bits"
        ]));
    }
}

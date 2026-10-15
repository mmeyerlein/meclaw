//! P12 S4/S5: Slack Socket Mode frame parsing and the ack frame.
//!
//! Every fixture in this file is hand-written from the wire pin (docs.slack.dev,
//! retrieved 2026-08-09), NOT copy-pasted from the docs: two of the published
//! examples are syntactically invalid JSON (the `hello` example is missing the
//! colon after `"started"`, several examples carry trailing commas). Those are
//! documentation defects, not wire format, so the fixtures below are the
//! corrected form. A parser that only ever saw the broken examples would be
//! written against a format Slack never sends.

use meclaw_cells::proxy::slack::wire::{SlackFrame, ack_frame, parse_frame};

/// The first frame after the WebSocket opens. `app_id` is load-bearing: it is
/// the only place the connection tells us which app we are, and loop rule R3
/// compares it against the `api_app_id` of inbound events.
#[test]
fn hello_frame_yields_app_id() {
    let raw = r#"{
        "type": "hello",
        "connection_info": { "app_id": "A1234" },
        "num_connections": 1,
        "debug_info": {
            "host": "applink-111",
            "started": "2020-10-11 12:12:12.120",
            "build_number": 54,
            "approximate_connection_time": 3600
        }
    }"#;
    match parse_frame(raw).expect("hello must parse") {
        SlackFrame::Hello { app_id, .. } => assert_eq!(app_id.as_deref(), Some("A1234")),
        other => panic!("expected Hello, got {other:?}"),
    }
}

/// A `hello` without `connection_info` must still parse. The docs do not
/// promise the field on every variant, and a missing app id is a degraded R3
/// (rules R1/R2/R4 still fire), not a connection we should tear down.
#[test]
fn hello_without_connection_info_parses_with_none_app_id() {
    let raw = r#"{"type":"hello","num_connections":1}"#;
    match parse_frame(raw).expect("bare hello must parse") {
        SlackFrame::Hello { app_id, .. } => assert!(app_id.is_none()),
        other => panic!("expected Hello, got {other:?}"),
    }
}

#[test]
fn events_api_envelope_yields_id_and_payload() {
    let raw = r#"{
        "type": "events_api",
        "envelope_id": "57d0e5a4-1b6a-4f9c-8f1d-2c3b4a5d6e7f",
        "accepts_response_payload": false,
        "payload": {
            "type": "event_callback",
            "api_app_id": "A1234",
            "team_id": "T0001",
            "event": { "type": "app_mention", "channel": "C123", "text": "hi", "ts": "1.1" }
        }
    }"#;
    match parse_frame(raw).expect("envelope must parse") {
        SlackFrame::EventsApi {
            envelope_id,
            payload,
            accepts_response_payload,
            ..
        } => {
            assert_eq!(envelope_id, "57d0e5a4-1b6a-4f9c-8f1d-2c3b4a5d6e7f");
            assert!(!accepts_response_payload);
            assert_eq!(
                payload.get("api_app_id").and_then(|v| v.as_str()),
                Some("A1234")
            );
        }
        other => panic!("expected EventsApi, got {other:?}"),
    }
}

/// `retry_attempt`/`retry_reason` are NOT in the official Socket Mode docs —
/// they exist only in the Python SDK's reader. So they must be optional and
/// absent in the normal case; a parser that required them would drop every
/// ordinary event.
#[test]
fn retry_fields_absent_by_default_and_read_when_present() {
    let plain = r#"{"type":"events_api","envelope_id":"e1","payload":{}}"#;
    match parse_frame(plain).expect("parse") {
        SlackFrame::EventsApi {
            retry_attempt,
            retry_reason,
            ..
        } => {
            assert!(retry_attempt.is_none());
            assert!(retry_reason.is_none());
        }
        other => panic!("expected EventsApi, got {other:?}"),
    }

    let retried = r#"{"type":"events_api","envelope_id":"e2","payload":{},
                      "retry_attempt":2,"retry_reason":"http_timeout"}"#;
    match parse_frame(retried).expect("parse") {
        SlackFrame::EventsApi {
            retry_attempt,
            retry_reason,
            ..
        } => {
            assert_eq!(retry_attempt, Some(2));
            assert_eq!(retry_reason.as_deref(), Some("http_timeout"));
        }
        other => panic!("expected EventsApi, got {other:?}"),
    }
}

/// All three documented reasons parse. The reason is carried as a String, not
/// matched into a closed enum: the docs publish no exhaustive list, so an
/// unknown reason must stay a normal disconnect rather than a parse error.
#[test]
fn all_documented_disconnect_reasons_parse() {
    for reason in ["link_disabled", "warning", "refresh_requested"] {
        let raw = format!(
            r#"{{"type":"disconnect","reason":"{reason}","debug_info":{{"host":"wss-111.slack.com"}}}}"#
        );
        match parse_frame(&raw).expect("disconnect must parse") {
            SlackFrame::Disconnect { reason: got } => assert_eq!(got, reason),
            other => panic!("expected Disconnect, got {other:?}"),
        }
    }
}

#[test]
fn undocumented_disconnect_reason_is_not_an_error() {
    let raw = r#"{"type":"disconnect","reason":"some_future_reason"}"#;
    match parse_frame(raw).expect("unknown reason must still parse") {
        SlackFrame::Disconnect { reason } => assert_eq!(reason, "some_future_reason"),
        other => panic!("expected Disconnect, got {other:?}"),
    }
}

/// An unknown frame type is NOT a failure. It becomes `Other`, carrying any
/// envelope id so the caller can still acknowledge it — silence toward Slack
/// would trigger redelivery of a frame we will never understand.
#[test]
fn unknown_frame_type_is_other_and_keeps_envelope_id() {
    let raw = r#"{"type":"slash_commands","envelope_id":"env-9","payload":{}}"#;
    match parse_frame(raw).expect("unknown type must parse") {
        SlackFrame::Other {
            envelope_id,
            frame_type,
        } => {
            assert_eq!(envelope_id.as_deref(), Some("env-9"));
            assert_eq!(frame_type, "slash_commands");
        }
        other => panic!("expected Other, got {other:?}"),
    }
}

/// Malformed input returns Err. It must never panic: this parser runs inside
/// the I/O task, and a panic there takes the whole cell down over one bad byte
/// on the socket.
#[test]
fn malformed_frames_are_errors_not_panics() {
    // The literal broken example from the Slack docs: missing colon after
    // "started". Proof that we treat it as malformed rather than as format.
    let broken_doc_example = r#"{"type":"hello","debug_info":{"started" "2020-10-11"}}"#;
    assert!(parse_frame(broken_doc_example).is_err());
    assert!(parse_frame("not json at all").is_err());
    assert!(parse_frame("[1,2,3]").is_err());
    assert!(parse_frame(r#"{"no_type":true}"#).is_err());
}

/// The ack is the entire contract for "I got it": envelope id, nothing else.
#[test]
fn ack_frame_is_exactly_the_envelope_id() {
    let raw = ack_frame("env-42");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("ack must be valid json");
    assert_eq!(
        v.get("envelope_id").and_then(|x| x.as_str()),
        Some("env-42")
    );
    let obj = v.as_object().expect("ack is an object");
    assert_eq!(obj.len(), 1, "ack carries no extra fields: {raw}");
}

/// Round-trip through the parser proves the ack we emit is the shape the pin
/// describes, not merely a string we believe in.
#[test]
fn ack_frame_escapes_exotic_envelope_ids() {
    let raw = ack_frame(r#"weird"id\with-escapes"#);
    let v: serde_json::Value = serde_json::from_str(&raw).expect("ack must stay valid json");
    assert_eq!(
        v.get("envelope_id").and_then(|x| x.as_str()),
        Some(r#"weird"id\with-escapes"#)
    );
}

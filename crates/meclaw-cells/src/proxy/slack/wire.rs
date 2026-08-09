//! P12 S4/S5: the Socket Mode wire — frame parsing and the ack frame.
//!
//! This module is pure: it turns bytes into values and values into bytes, with
//! no I/O and no state. That is what lets the whole frame vocabulary be proven
//! by unit tests before a socket exists anywhere in the tree.
//!
//! Two deliberate looseness decisions, both taken from the wire pin:
//!
//! * `disconnect.reason` is carried as a `String`, not a closed enum. Slack
//!   documents three reasons but publishes no exhaustive list, so an unknown
//!   reason must remain an ordinary disconnect instead of becoming a parse
//!   error on a live connection.
//! * An unknown frame `type` becomes [`SlackFrame::Other`] rather than an
//!   error, and keeps its envelope id so the caller can still acknowledge it.
//!   Staying silent toward Slack would make it redeliver a frame we are never
//!   going to understand.
//!
//! Timestamps (`ts`, `event_ts`, `thread_ts`) are kept as strings everywhere.
//! They look numeric and are not: `ts` carries a dot, `event_ts` frequently
//! does not, and parsing either through a float silently destroys the
//! microsecond digits that make a Slack message addressable.

use serde_json::{Value as JsonValue, json};

/// One decoded Socket Mode frame.
#[derive(Debug, Clone)]
pub enum SlackFrame {
    /// First frame after the socket opens. `app_id` identifies which app this
    /// connection belongs to and is the input to loop rule R3.
    Hello {
        /// Our own app id, when the server sent `connection_info`.
        app_id: Option<String>,
        /// How many connections this app currently holds (max 10).
        num_connections: Option<i64>,
    },
    /// An Events API delivery. Must be acknowledged by envelope id.
    EventsApi {
        /// Acknowledgement key — the only field the ack carries.
        envelope_id: String,
        /// The `event_callback` wrapper.
        payload: JsonValue,
        /// Whether Slack will accept a response payload alongside the ack.
        accepts_response_payload: bool,
        /// Redelivery counter. Undocumented in the Socket Mode reference and
        /// absent on normal deliveries — never require it.
        retry_attempt: Option<i64>,
        /// Redelivery reason. Free-form; must not be matched against a fixed
        /// set of strings.
        retry_reason: Option<String>,
    },
    /// Slack is closing this connection; the caller reconnects.
    Disconnect {
        /// Documented values are `link_disabled`, `warning`, `refresh_requested`.
        reason: String,
    },
    /// A frame type outside this cell's scope. Ignored, but still ackable.
    Other {
        /// Present for envelope-carrying frame types.
        envelope_id: Option<String>,
        /// The raw `type` value, kept for logging.
        frame_type: String,
    },
}

/// Decodes one Socket Mode frame.
///
/// Returns `Err` on malformed input and never panics: this runs inside the I/O
/// task, where a panic over one bad byte on the socket would take down the
/// whole cell.
pub fn parse_frame(raw: &str) -> Result<SlackFrame, String> {
    let v: JsonValue =
        serde_json::from_str(raw).map_err(|e| format!("frame: invalid json: {e}"))?;
    let obj = v.as_object().ok_or("frame: must be a json object")?;
    let frame_type = obj
        .get("type")
        .and_then(|t| t.as_str())
        .ok_or("frame: missing `type`")?;

    match frame_type {
        "hello" => Ok(SlackFrame::Hello {
            app_id: obj
                .get("connection_info")
                .and_then(|c| c.get("app_id"))
                .and_then(|a| a.as_str())
                .map(String::from),
            num_connections: obj.get("num_connections").and_then(|n| n.as_i64()),
        }),
        "events_api" => Ok(SlackFrame::EventsApi {
            envelope_id: obj
                .get("envelope_id")
                .and_then(|e| e.as_str())
                .ok_or("events_api: missing `envelope_id`")?
                .to_string(),
            payload: obj.get("payload").cloned().unwrap_or(JsonValue::Null),
            accepts_response_payload: obj
                .get("accepts_response_payload")
                .and_then(|b| b.as_bool())
                .unwrap_or(false),
            retry_attempt: obj.get("retry_attempt").and_then(|r| r.as_i64()),
            retry_reason: obj
                .get("retry_reason")
                .and_then(|r| r.as_str())
                .map(String::from),
        }),
        "disconnect" => Ok(SlackFrame::Disconnect {
            reason: obj
                .get("reason")
                .and_then(|r| r.as_str())
                .unwrap_or("unspecified")
                .to_string(),
        }),
        other => Ok(SlackFrame::Other {
            envelope_id: obj
                .get("envelope_id")
                .and_then(|e| e.as_str())
                .map(String::from),
            frame_type: other.to_string(),
        }),
    }
}

/// Builds the acknowledgement frame. The envelope id is the entire contract;
/// serialisation (not string formatting) does the escaping.
pub fn ack_frame(envelope_id: &str) -> String {
    json!({ "envelope_id": envelope_id }).to_string()
}

/// Which kind of user-visible event arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlackEventKind {
    /// `app_mention` — Slack routes these only to the app that was mentioned,
    /// which is what makes multi-bot addressing Slack's job rather than ours.
    Mention,
    /// `message` in any conversation type.
    Message,
}

/// A user-authored event, flattened to what the cell needs.
///
/// Every timestamp stays a `String`. They are addresses, not quantities.
#[derive(Debug, Clone)]
pub struct SlackUserEvent {
    /// Mention or plain message.
    pub kind: SlackEventKind,
    /// Conversation id (`C…`/`D…`/`G…`).
    pub channel: String,
    /// `channel`/`im`/`group`/`mpim`. The only way to tell an mpim from a
    /// private group.
    pub channel_type: Option<String>,
    /// Author. Absent on some bot-authored events.
    pub user: Option<String>,
    /// Raw message text, with mentions still in `<@U…>` form.
    pub text: String,
    /// This message's own timestamp — also its address within the channel.
    pub ts: String,
    /// Set when the message belongs to a thread. Equal to `ts` on the parent.
    pub thread_ts: Option<String>,
    /// Event timestamp; frequently formatted without a dot.
    pub event_ts: Option<String>,
}

/// Extracts a user event from an `event_callback` payload.
///
/// Returns `None` for anything without user-authored text (reactions, joins,
/// rate-limit notices), which the caller acknowledges and ignores.
pub fn parse_user_event(payload: &JsonValue) -> Option<SlackUserEvent> {
    let event = payload.get("event")?;
    let kind = match event.get("type").and_then(|t| t.as_str())? {
        "app_mention" => SlackEventKind::Mention,
        "message" => SlackEventKind::Message,
        _ => return None,
    };
    Some(SlackUserEvent {
        kind,
        channel: event.get("channel").and_then(|c| c.as_str())?.to_string(),
        channel_type: event
            .get("channel_type")
            .and_then(|c| c.as_str())
            .map(String::from),
        user: event.get("user").and_then(|u| u.as_str()).map(String::from),
        text: event.get("text").and_then(|t| t.as_str())?.to_string(),
        ts: event.get("ts").and_then(|t| t.as_str())?.to_string(),
        thread_ts: event
            .get("thread_ts")
            .and_then(|t| t.as_str())
            .map(String::from),
        event_ts: event
            .get("event_ts")
            .and_then(|t| t.as_str())
            .map(String::from),
    })
}

/// Why an event was recognised as our own traffic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopDrop {
    /// R1 — `bot_id` present. The authoritative marker.
    BotId,
    /// R2 — `subtype == "bot_message"`. Classic-app behaviour; not reliable on
    /// its own, kept as a second line of defence.
    BotMessageSubtype,
    /// R3 — the sending app is us.
    OwnAppId,
    /// R4 — the author is our own bot user.
    OwnUserId,
}

/// Stateless bot-loop guard (rules R1–R4). `Some(_)` means "our own traffic,
/// do not process" — the caller still acknowledges the envelope, because
/// ignoring an event is not the same as failing to receive it.
///
/// R3 deliberately reads `event.app_id`, the app that SENT the message, and
/// never the payload's `api_app_id`. The latter names the RECEIVING app and
/// therefore equals `own_app_id` on every inbound event; reading it here would
/// drop all traffic and leave a bot that is silent on a healthy socket.
pub fn loop_drop_reason(
    payload: &JsonValue,
    own_app_id: Option<&str>,
    bot_user_id: Option<&str>,
) -> Option<LoopDrop> {
    let event = payload.get("event")?;
    if event.get("bot_id").is_some() {
        return Some(LoopDrop::BotId);
    }
    if event.get("subtype").and_then(|s| s.as_str()) == Some("bot_message") {
        return Some(LoopDrop::BotMessageSubtype);
    }
    let sender_app_id = event.get("app_id").and_then(|a| a.as_str());
    if let (Some(own), Some(sender)) = (own_app_id, sender_app_id)
        && own == sender
    {
        return Some(LoopDrop::OwnAppId);
    }
    let author = event.get("user").and_then(|u| u.as_str());
    if let (Some(self_id), Some(author)) = (bot_user_id, author)
        && self_id == author
    {
        return Some(LoopDrop::OwnUserId);
    }
    None
}

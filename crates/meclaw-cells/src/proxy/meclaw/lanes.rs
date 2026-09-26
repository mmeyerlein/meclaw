//! The projection: what the lane names crosses; everything else is refused.
//!
//! A body field the lane does not name is a refusal, never a silent strip —
//! stripping would deliver a message its writer never sent. `context` is the
//! one place that filters silently: it is routing metadata, not content.

use serde_json::{Map, Value};

use super::params::{Lane, Lanes};
use super::wire;

/// Why a crossing did not happen: one of the eleven `error_code`s plus a detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// One of the eleven codes in [`wire`].
    pub error_code: &'static str,
    /// The human-readable detail, naming what was refused.
    pub detail: String,
}

impl Refusal {
    /// Builds a refusal from a code and a detail.
    pub fn new(error_code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            error_code,
            detail: detail.into(),
        }
    }
}

/// Which half of the declaration a lane is looked up in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Lanes the peer may send in.
    Accepts,
    /// Lanes this colony may send out.
    Emits,
}

/// The lane named `route` in `direction`, or `None`. `lane_undeclared` is the
/// caller's to raise: only it knows whether it answers a frame or judges an
/// outgoing message.
pub fn find_lane<'a>(lanes: &'a Lanes, direction: Direction, route: &str) -> Option<&'a Lane> {
    let list = match direction {
        Direction::Accepts => &lanes.accepts,
        Direction::Emits => &lanes.emits,
    };
    list.iter().find(|l| l.route == route)
}

/// Projects a body onto the lane. Checks the whole body first and builds only
/// then, so nothing ever crosses half.
pub fn project_body(lane: &Lane, body: &Value) -> Result<Value, Refusal> {
    let Some(obj) = body.as_object() else {
        return Err(Refusal::new(
            wire::LANE_BODY_UNSUPPORTED,
            "a whole-body blob does not cross a colony boundary; the body must be inline JSON",
        ));
    };
    // Before the allow-list and regardless of it: a declaration may not unlock an
    // attachment (R-J6). Blob stores are separate and stay separate.
    if obj.contains_key("attachments") {
        return Err(Refusal::new(
            wire::LANE_BODY_UNSUPPORTED,
            "attachments[] does not cross a colony boundary, and naming it in `fields` does \
             not change that",
        ));
    }
    // GH #853: a `params` slot switches the model of an `llm` cell it reaches.
    // A peer is not the operator, so no declaration can let one cross — the
    // same "before the allow-list and regardless of it" as `attachments`.
    if obj.contains_key("params") {
        return Err(Refusal::new(
            wire::LANE_BODY_UNSUPPORTED,
            "a params slot does not cross a colony boundary: it reconfigures the cell it \
             reaches, and only the colony's own operator may do that; naming it in `fields` \
             does not change that",
        ));
    }
    let mut out = Map::new();
    for (key, value) in obj {
        if key == "messages" {
            out.insert(key.clone(), project_turns(lane, value)?);
            continue;
        }
        if !lane.fields.iter().any(|f| f == key) {
            return Err(Refusal::new(
                wire::LANE_FIELD_DENIED,
                format!(
                    "the lane {:?} does not name the body field {key:?}; it names [{}]",
                    lane.route,
                    lane.fields.join(", ")
                ),
            ));
        }
        out.insert(key.clone(), value.clone());
    }
    Ok(Value::Object(out))
}

/// `messages[]`, turn by turn: each turn field is named one by one as
/// `messages[].<field>` or not at all (OR-Peer.L1a.3 — a bare `"messages"`
/// allows nothing, because free prose about a person is what the boundary is
/// there to hold back).
fn project_turns(lane: &Lane, messages: &Value) -> Result<Value, Refusal> {
    let Some(turns) = messages.as_array() else {
        return Err(Refusal::new(
            wire::LANE_BODY_UNSUPPORTED,
            "messages must be an inline array of turn objects to cross a colony boundary",
        ));
    };
    // GH #839: before the allow-list and regardless of it, like `attachments`
    // in `project_body`. A `text_id`/`messages_id` is resolved against the
    // RECEIVING colony's blob store at its delivery boundary
    // (`meclaw-colony/src/cell_task.rs`, `resolve_blob_for_delivery`), so a
    // peer that could send one would read that store: measured in
    // `meclaw-cli/tests/gh839_a_foreign_pointer_is_never_resolved.rs`, where
    // the frame crossed as soon as the lane named the key. The scan runs over
    // every turn first, so a pointer anywhere wins over an allow-list refusal
    // in an earlier turn. Outgoing it is inert (the local delivery resolved
    // already) and holds for symmetry.
    if turns.iter().any(|t| {
        t.as_object()
            .is_some_and(|o| o.contains_key("text_id") || o.contains_key("messages_id"))
    }) {
        return Err(Refusal::new(
            wire::LANE_BODY_UNSUPPORTED,
            "a blob reference from another colony's store is never resolved here, and naming \
             it in `fields` does not change that",
        ));
    }
    let mut out = Vec::with_capacity(turns.len());
    for turn in turns {
        let Some(obj) = turn.as_object() else {
            return Err(Refusal::new(
                wire::LANE_BODY_UNSUPPORTED,
                "a turn in messages[] must be an inline object to cross a colony boundary",
            ));
        };
        let mut kept = Map::new();
        for (k, v) in obj {
            let dotted = format!("messages[].{k}");
            if !lane.fields.contains(&dotted) {
                return Err(Refusal::new(
                    wire::LANE_FIELD_DENIED,
                    format!(
                        "the lane {:?} does not name the turn field {dotted:?}; a turn field is \
                         named one by one or not at all",
                        lane.route
                    ),
                ));
            }
            kept.insert(k.clone(), v.clone());
        }
        out.push(Value::Object(kept));
    }
    Ok(Value::Array(out))
}

/// The hop key that carries the origins the sender claimed (GH #847).
pub const PEER_ORIGINS: &str = "peer_origins";
/// The hop key that carries the speaker fields the sender set itself (GH #847).
pub const PEER_SPEAKERS: &str = "peer_speakers";

/// The body schema's `maxLength` of a turn's `speaker` (`ubf-body.json`).
const SPEAKER_MAX_CHARS: usize = 120;

/// The body schema's form of a `speaker_ref`: 8 or 12 lowercase hex digits.
fn is_speaker_ref(r: &str) -> bool {
    matches!(r.len(), 8 | 12) && r.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The closed set of `origin` values a turn may carry (`ubf-body.json`,
/// `$defs.TurnObject.origin`); only a claim from this set is stamped.
const TURN_ORIGINS: [&str; 5] = ["user", "assistant", "tool", "system", "peer"];

/// Stamps every turn of a projected body `origin: "peer"` — GH #847, R-SN-3.
///
/// Runs after [`project_body`] and before the deliverability check. Before
/// this, the sender's `origin` crossed unchanged: the other side could claim
/// `user` and arrive as this agent's own person, or `assistant` and be filed
/// as its own answer. Now what the sender said moves out of the turns into the
/// returned hop keys, in turn order:
///
/// - [`PEER_ORIGINS`]: the claimed `origin` of each turn;
/// - [`PEER_SPEAKERS`]: `{"speaker", "speaker_ref"}` as the sender set them on
///   a turn (a missing half is `null`), or `null` for a turn that carried
///   neither. Both fields leave the turn: they are set on this side only, from
///   a checked identity (OR-SN-33), never taken from the other side's word.
///
/// Only a claim of the closed set is stamped ([`TURN_ORIGINS`]). A turn
/// without an `origin`, or with one that is none of the five values (`"bogus"`,
/// `null`, an object, a number, a megabyte of text), is left exactly as it
/// came, so the deliverability check after this refuses the frame
/// `invalid_frame` as it did before GH #847 (rev-L1 I-1: stamping any value
/// laundered it into a valid `peer` turn and put it into `hop.peer_origins`,
/// the field an application reads "the other side's person" from, R-SN-3).
/// The stamp never makes a turn out of something that was none, so a lane
/// that does not name `messages[].origin` stays undeliverable. A body without
/// a `messages` array carries no turns and gets no hop keys.
pub fn restamp_as_peer(mut body: Value) -> (Value, Map<String, Value>) {
    let mut hop = Map::new();
    let Some(turns) = body.get_mut("messages").and_then(Value::as_array_mut) else {
        return (body, hop);
    };
    let mut origins = Vec::with_capacity(turns.len());
    let mut speakers = Vec::with_capacity(turns.len());
    for turn in turns.iter_mut() {
        let Some(obj) = turn.as_object_mut() else {
            continue;
        };
        let Some(claimed) = obj
            .get("origin")
            .and_then(Value::as_str)
            .filter(|o| TURN_ORIGINS.contains(o))
            .map(str::to_string)
        else {
            continue;
        };
        obj.insert("origin".to_string(), Value::String("peer".to_string()));
        origins.push(Value::String(claimed));
        // Only the schema's form reaches the hop (review of L1, R1-2): a name
        // is a string of at most 120 characters, a reference 8 or 12
        // lowercase hex digits; anything else is `null`, so an application
        // reading `hop.peer_speakers` never meets an object, a number or a
        // megabyte of text the other side put on a turn.
        let speaker = obj.remove("speaker").map(|v| match v {
            Value::String(s) if s.chars().count() <= SPEAKER_MAX_CHARS => Value::String(s),
            _ => Value::Null,
        });
        let speaker_ref = obj.remove("speaker_ref").map(|v| match v {
            Value::String(s) if is_speaker_ref(&s) => Value::String(s),
            _ => Value::Null,
        });
        speakers.push(if speaker.is_none() && speaker_ref.is_none() {
            Value::Null
        } else {
            let mut s = Map::new();
            s.insert("speaker".to_string(), speaker.unwrap_or(Value::Null));
            s.insert(
                "speaker_ref".to_string(),
                speaker_ref.unwrap_or(Value::Null),
            );
            Value::Object(s)
        });
    }
    hop.insert(PEER_ORIGINS.to_string(), Value::Array(origins));
    hop.insert(PEER_SPEAKERS.to_string(), Value::Array(speakers));
    (body, hop)
}

/// Projects `context` onto the lane's `context` list, silently: an unnamed key
/// simply does not cross.
pub fn project_context(lane: &Lane, context: &Map<String, Value>) -> Map<String, Value> {
    context
        .iter()
        .filter(|(k, _)| lane.context.iter().any(|c| c == *k))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Review of L1, R1-3: the closed set of origins is a copy of the body
    /// schema's enum (`TurnObject.origin`); it must not drift from it, or a
    /// new origin would be refused at the mount while the schema allows it.
    #[test]
    fn the_turn_origins_are_the_schema_enum() {
        let schema: Value = serde_json::from_str(include_str!(
            "../../../../meclaw-core/schemas/ubf-body.json"
        ))
        .expect("the body schema parses");
        let mut from_schema: Vec<String> =
            schema["$defs"]["TurnObject"]["properties"]["origin"]["enum"]
                .as_array()
                .expect("TurnObject.origin is an enum")
                .iter()
                .map(|v| v.as_str().expect("a string").to_string())
                .collect();
        from_schema.sort();
        let mut ours: Vec<String> = TURN_ORIGINS.iter().map(|s| s.to_string()).collect();
        ours.sort();
        assert_eq!(ours, from_schema);
    }

    /// Review of L1, R1-2: `hop.peer_speakers` carries only values in the
    /// schema's form -- a `speaker` string of at most 120 characters, a
    /// `speaker_ref` of 8 or 12 lowercase hex digits -- and `null` for
    /// anything else, so what an application reads there is never an object,
    /// a number or a megabyte of text the other side put on a turn.
    #[test]
    fn peer_speakers_carry_only_the_schema_form() {
        let long = "x".repeat(121);
        let body = json!({"messages": [
            {"origin": "user", "type": "text", "text": "a",
             "speaker": {"nested": true}, "speaker_ref": "3A47FE3E"},
            {"origin": "user", "type": "text", "text": "b", "speaker": long, "speaker_ref": 7},
            {"origin": "user", "type": "text", "text": "c", "speaker": "Jonas",
             "speaker_ref": "3a47fe3e0b1c"}
        ]});
        let (_, hop) = restamp_as_peer(body);
        assert_eq!(
            hop[PEER_SPEAKERS],
            json!([
                {"speaker": null, "speaker_ref": null},
                {"speaker": null, "speaker_ref": null},
                {"speaker": "Jonas", "speaker_ref": "3a47fe3e0b1c"}
            ])
        );
    }
}

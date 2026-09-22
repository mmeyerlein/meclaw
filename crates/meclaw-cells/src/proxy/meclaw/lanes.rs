//! The projection: what the lane names crosses; everything else is refused.
//!
//! A body field the lane does not name is a refusal, never a silent strip —
//! stripping would deliver a message its writer never sent. `context` is the
//! one place that filters silently: it is routing metadata, not content.

use serde_json::{Map, Value};

use super::params::{Lane, Lanes};
use super::wire;

/// Why a crossing did not happen: one of the nine `error_code`s plus a detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// One of the nine codes in [`wire`].
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

/// Projects `context` onto the lane's `context` list, silently: an unnamed key
/// simply does not cross.
pub fn project_context(lane: &Lane, context: &Map<String, Value>) -> Map<String, Value> {
    context
        .iter()
        .filter(|(k, _)| lane.context.iter().any(|c| c == *k))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

//! D58 live (GH #1482, plan `u-live.md` contract points 1, 2) -- the live
//! handle is set only for a session of its own.
//!
//! WHY this file exists: everywhere before, "a running session" was the
//! conjunction `engine` "duplex" AND `call_id` set (the lead, the policy's
//! fact from a tool and its wait announcement). The voice cell stamps
//! `call_id` on every message it sends and `engine` only when it runs duplex
//! (`crates/meclaw-cells/src/voice/cell.rs`, `stamp_engine`). An edge that
//! set `live := call_id` would make a cascade turn -- a call without a model
//! session of its own -- live. So the edges that translate the voice's keys
//! into the handle set it to the value of `call_id` only when `engine` is
//! "duplex", and leave it empty otherwise.
//!
//! Reads the edges of `templates/member` and `templates/talky` as they ship
//! and evaluates them with the colony's own CEL (`support/curator_live.rs`
//! `through`); no copy of an edge.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_live.rs"]
#[allow(unused_imports)]
use crate::curator_live; // harness_fold: mod curator_live;

use curator_hive::*;
use curator_live::*;
use meclaw_core::serde_json::{Value, json};

const CALL: &str = "call-9";

/// The voice's head on `route`, with `engine` as given (None = not stamped).
fn voice_head(route: &str, engine: Option<&str>) -> Value {
    let mut hop = json!({"route": route, "call_id": CALL, "session_id": VOICE_SESSION,
                         "turn_id": "t1", "delegation_id": "item_d1", "reason": "start",
                         "renewal_n": 1});
    if let Some(e) = engine {
        hop["engine"] = json!(e);
    }
    hop
}

/// Where the translation into the handle happens: (template, from, route,
/// destination).
const INWARD: [(&str, &str, &str, &str); 5] = [
    ("member", "./channels", "turn", "./firewall"),
    ("member", "./channels", "delegation", "./assistants"),
    ("member", "./channels", "renewed", "./assistants"),
    ("talky", ".", "in_renewed", "./curator"),
    ("talky", ".", "in_seed_ask", "./curator"),
];

#[test]
fn gh_d58_live_the_handle_is_set_only_for_a_session_of_its_own() {
    if !shipped() {
        return;
    }
    for (template, from, route, to) in INWARD {
        let ctx = json!({"channel": "voice", "audience_set": ROUND_E});

        // A session of its own: the handle carries the call's value.
        let own = reaching(
            template,
            from,
            to,
            ctx.clone(),
            voice_head(route, Some("duplex")),
        );
        assert!(
            !own.is_empty(),
            "templates/{template}: no edge takes `{route}` from {from} to {to}"
        );
        for p in &own {
            assert_eq!(
                named(&p.context, "live").as_deref(),
                Some(CALL),
                "templates/{template} edge {}: a duplex `{route}` carries the live handle \
                 in its context: {:?}",
                p.edge,
                p.context
            );
        }

        // A call without a session of its own: no handle, whatever else.
        for engine in [None, Some(""), Some("cascade")] {
            for p in reaching(template, from, to, ctx.clone(), voice_head(route, engine)) {
                for (side, m) in [("context", &p.context), ("hop", &p.hop)] {
                    assert!(
                        named(m, "live").is_none(),
                        "templates/{template} edge {}: `{route}` with engine {engine:?} is not \
                         live, its {side} carries a handle: {m:?}",
                        p.edge
                    );
                }
            }
        }
    }
}

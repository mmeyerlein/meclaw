//! D58 live (GH #1482, plan `u-live.md` contract points 2, 3) -- the edges
//! translate both ways.
//!
//! WHY this file exists: the voice cell is a cell type of the voice domain
//! and keeps `call_id`, `renewal_n` and `engine`; the curator knows only the
//! handle `live` and its own lane names. Between the two stand the edges,
//! and they translate in both directions: into the handle where the voice's
//! messages enter the curator, back to `call_id` where the curator's answers
//! reach the voice. Four kinds of edge carry it:
//!
//! - into the curator: `talky` takes the voice's lanes to `./curator`, the
//!   end of a session under the curator's own name (`in_call_end` becomes
//!   `in_live_end` there; the session keeper keeps its lane `in_call_end`);
//! - in the curator hive: the answers of `./handover` (`sidecar`),
//!   `./history` (`seed`, `live_facts`) and `./policy` (`live_feed`) leave
//!   with the handle in the context -- never a `call_id`; the end of a
//!   session comes in on `in_live_end`;
//! - back to the voice: `member` turns a `sidecar` for the voice into
//!   `in_advise` with the call's id, `talky` hands `seed`, `live_facts` and
//!   `live_feed` out with it.
//!
//! Reads the edges of `templates/member`, `templates/talky` and
//! `templates/curator` as they ship and evaluates them with the colony's own
//! CEL (`support/curator_live.rs` `through`); no copy of an edge. The
//! condition that sets the handle only for a duplex session is the pin
//! `gh_d58_live_the_handle_is_set_only_for_a_session_of_its_own`.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_live.rs"]
#[allow(unused_imports)]
use crate::curator_live; // harness_fold: mod curator_live;

use curator_hive::*;
use curator_live::*;
use meclaw_core::serde_json::{Map, Value, json};

const CALL: &str = "call-9";

/// The call's id a message carries for the voice: hop first, then context
/// (the voice cell reads its address from either).
fn call_of(p: &Passed) -> Option<String> {
    named(&p.hop, "call_id").or_else(|| named(&p.context, "call_id"))
}

fn duplex_head(route: &str) -> Value {
    json!({"route": route, "call_id": CALL, "engine": "duplex",
           "session_id": VOICE_SESSION, "reason": "start", "renewal_n": 1,
           "final": true})
}

fn no_call(m: &Map<String, Value>) -> bool {
    named(m, "call_id").is_none()
}

#[test]
fn gh_d58_live_the_edges_translate_both_ways() {
    if !shipped() {
        return;
    }
    let ctx = json!({"channel": "voice", "audience_set": ROUND_E});

    // 1. Into the curator (talky): the voice's lanes arrive with the handle,
    //    the end of a session under the curator's own name.
    for (lane, at_curator) in [
        ("in_seed_ask", "in_seed_ask"),
        ("in_renewed", "in_renewed"),
        ("in_call_end", "in_live_end"),
    ] {
        let got = reaching("talky", ".", "./curator", ctx.clone(), duplex_head(lane));
        assert_eq!(
            got.len(),
            1,
            "templates/talky: `{lane}` reaches ./curator once: {got:?}"
        );
        let p = &got[0];
        assert_eq!(
            p.hop.get("route"),
            Some(&json!(at_curator)),
            "templates/talky edge {}: `{lane}` arrives at the curator as `{at_curator}`",
            p.edge
        );
        assert_eq!(
            named(&p.context, "live").as_deref(),
            Some(CALL),
            "templates/talky edge {}: `{lane}` carries the live handle: {:?}",
            p.edge,
            p.context
        );
    }
    let keeper = reaching(
        "talky",
        ".",
        "./session-keeper",
        ctx.clone(),
        duplex_head("in_call_end"),
    );
    assert_eq!(
        keeper.len(),
        1,
        "the session keeper hears the end: {keeper:?}"
    );
    assert_eq!(
        keeper[0].hop.get("route"),
        Some(&json!("in_call_end")),
        "the session keeper keeps its lane's name"
    );

    // 2. The curator hive: its lane for the end of a session, and its
    //    answers leave with the handle, never with a call.
    let end = reaching(
        "curator",
        ".",
        "./history",
        json!({"live": CALL, "audience_set": ROUND_E}),
        json!({"route": "in_live_end", "live": CALL}),
    );
    assert_eq!(
        end.len(),
        1,
        "templates/curator: `in_live_end` reaches ./history"
    );
    for (cell, route) in [
        ("./handover", "sidecar"),
        ("./history", "seed"),
        ("./history", "live_facts"),
        ("./policy", "live_feed"),
    ] {
        let out = reaching(
            "curator",
            cell,
            ".",
            json!({"audience_set": ROUND_E}),
            json!({"route": route, "section": "context", "live": CALL,
                   "session_id": VOICE_SESSION}),
        );
        assert_eq!(
            out.len(),
            1,
            "templates/curator: `{route}` of {cell} leaves the hive once: {out:?}"
        );
        let p = &out[0];
        assert_eq!(
            named(&p.context, "live").as_deref(),
            Some(CALL),
            "templates/curator edge {}: `{route}` of {cell} leaves with the handle: {:?}",
            p.edge,
            p.context
        );
        assert!(
            no_call(&p.context) && no_call(&p.hop),
            "templates/curator edge {}: `{route}` of {cell} names no call: {:?} {:?}",
            p.edge,
            p.context,
            p.hop
        );
    }

    // 3. Back to the voice: the call's id again, from the handle.
    let advise = reaching(
        "member",
        "./assistants",
        "./channels",
        json!({"channel": "voice", "channel_node": "voice", "live": CALL,
               "audience_set": ROUND_E}),
        json!({"route": "sidecar", "section": "context", "live": CALL}),
    );
    assert_eq!(
        advise.len(),
        1,
        "templates/member: the sidecar reaches the voice: {advise:?}"
    );
    assert_eq!(advise[0].hop.get("route"), Some(&json!("in_advise")));
    assert_eq!(
        call_of(&advise[0]).as_deref(),
        Some(CALL),
        "templates/member edge {}: `in_advise` carries the call's id from the handle: {:?} {:?}",
        advise[0].edge,
        advise[0].hop,
        advise[0].context
    );
    for route in ["seed", "live_facts", "live_feed"] {
        let out = reaching(
            "talky",
            "./curator",
            ".",
            json!({"live": CALL, "audience_set": ROUND_E}),
            json!({"route": route, "live": CALL, "session_id": VOICE_SESSION}),
        );
        assert_eq!(
            out.len(),
            1,
            "templates/talky: `{route}` leaves the composite once: {out:?}"
        );
        assert_eq!(
            call_of(&out[0]).as_deref(),
            Some(CALL),
            "templates/talky edge {}: `{route}` carries the call's id from the handle: {:?} {:?}",
            out[0].edge,
            out[0].hop,
            out[0].context
        );
    }
}

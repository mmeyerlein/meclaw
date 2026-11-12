//! #1404 -- a partial's stamp names its call.
//!
//! WHY this file exists: the curator interrupts a crisis on the first partial
//! (`interrupt_on_delta`) and sends the advise to the call it belongs to, so
//! the `delta_stamp` must carry the call. The voice partial brings `call_id`
//! on its hop; `./route` builds every header anew, so it rides along on each
//! hop of a partial (the wall ask, the decide call) and stands on the
//! `delta_stamp` header -- with `session_id` where the hop has one. A turn
//! (no delta) keeps today's bytes, and a partial without a call carries none.

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::{lane, now_ms, run_cell};
use serde_json::{Value, json};

const ROUTE: &str = "templates/talky/route/config.json";

fn ctx(delta: bool) -> Value {
    let mut c = json!({"turn_id": "c-1", "channel": "voice:1"});
    if delta {
        c["delta"] = json!("1");
    }
    c
}

fn call() -> Value {
    json!({"call_id": "call-7", "session_id": "call-7"})
}

fn said() -> Value {
    json!([{"origin": "user", "type": "text", "text": "I do not want to go on."}])
}

// Asked an hour ahead: no start time of the cell's process makes it late.
fn in_time() -> String {
    (now_ms() + 3_600_000).to_string()
}

fn on<'a>(out: &'a [Value], route: &str) -> &'a Value {
    out.iter()
        .find(|m| m["header"]["route"] == route)
        .unwrap_or_else(|| panic!("no {route} in {out:?}"))
}

fn decision(ctx: Value, hop: Value) -> Value {
    let mut h = json!({"finish_reason": "stop", "asked_at": in_time(),
                       "state": "I do not want to go on."});
    for (k, v) in hop.as_object().expect("hop") {
        h[k] = v.clone();
    }
    let mut doc = lane("in_decision", h, ctx, json!([]));
    doc["decision"] = json!({"answers": {
        "path": {"choice": "talk", "p": 0.9},
        "intent": {"choice": "statement", "p": 0.8}
    }, "missing": [], "model": "d"});
    doc
}

#[test]
fn a_delta_stamp_of_a_call_carries_its_call_id() {
    // The partial's decide call carries the call out and back ...
    let out = run_cell(ROUTE, &[], lane("in_turn", call(), ctx(true), said())).0;
    let decide = on(&out, "decide");
    assert_eq!(decide["header"]["call_id"], "call-7", "{out:?}");
    assert_eq!(decide["header"]["session_id"], "call-7", "{out:?}");
    // ... the wall ask too, and the decide call after it.
    let over = [("context_turns", json!(3))];
    let out = run_cell(ROUTE, &over, lane("in_turn", call(), ctx(true), said())).0;
    assert_eq!(on(&out, "view_ask")["header"]["call_id"], "call-7");
    let mut hop = call();
    hop["state"] = json!("I do not want to go on.");
    hop["asked_at"] = json!(in_time());
    let back = lane("in_view", hop, ctx(true), json!([]));
    let out = run_cell(ROUTE, &over, back).0;
    assert_eq!(on(&out, "decide")["header"]["call_id"], "call-7");

    // The stamp of the partial names the call.
    let out = run_cell(ROUTE, &[], decision(ctx(true), call())).0;
    let d = on(&out, "delta_stamp");
    assert_eq!(d["header"]["call_id"], "call-7", "{out:?}");
    assert_eq!(d["header"]["session_id"], "call-7", "{out:?}");

    // A partial without a call carries none; a turn keeps today's header.
    let out = run_cell(ROUTE, &[], decision(ctx(true), json!({}))).0;
    assert!(
        on(&out, "delta_stamp")["header"].get("call_id").is_none(),
        "{out:?}"
    );
    let out = run_cell(ROUTE, &[], lane("in_turn", call(), ctx(false), said())).0;
    assert!(
        on(&out, "decide")["header"].get("call_id").is_none(),
        "{out:?}"
    );
}

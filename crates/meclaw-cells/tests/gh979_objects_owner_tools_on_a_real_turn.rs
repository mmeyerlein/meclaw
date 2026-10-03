//! GH #979 — the object hive's owner tools work on a REAL turn of the bound
//! member, and only there.
//!
//! `object_set` (`why`, `state`) and `object_confirm` are written only for
//! the owner of the turn: `context.speaker`, a `member:` reference standing in
//! the round (`templates/objects` README). Until GH #949 nothing stamped it on
//! a real turn, and every test of the owner rule set it by hand; downstream,
//! on a real Telegram turn, the rule therefore named nobody (GH #979). Here the
//! speaker is whatever the road made of the sender: the recipe's bound
//! ingress, the shipped firewall, the member's shipped edges to the tool and
//! back. The answer is read where the brain gets it.
//!
//! The objects store is born with one candidate in the member's round.

#[path = "support/speaker_road.rs"]
mod speaker_road;

use meclaw_core::serde_json::{Value, json};
use speaker_road::*;

async fn call(road: &mut Road, sender: i64, tool: &str, args: Value) -> (String, Value) {
    let t = road
        .say(
            "own",
            Some(OWN_CHAT),
            Some(json!(sender)),
            None,
            &format!("TOOL {tool} {args}"),
        )
        .await;
    let got = next(&mut road.ear, tool).await;
    assert_eq!(got.trace_id, t, "the answer of another turn arrived");
    (hop(&got, "refused").unwrap_or_default(), answer_of(&got))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_owner_tools_answer_the_bound_member_and_refuse_everybody_else() {
    if !shipped() {
        return;
    }
    let mut road = Road::boot().await;
    let confirm = json!({"id": OBJECT});
    let why = json!({"id": OBJECT, "slot": "why", "value": "for the commute"});

    // Somebody else in the member's bound chat: the round is the member's, the
    // speaker is nobody -- the owner tools stay closed and nothing is written.
    let (refused, v) = call(&mut road, STRANGER, "object_confirm", confirm.clone()).await;
    assert_eq!(refused, "owner_only", "{v}");
    assert_eq!(v["error"]["code"], json!("owner_only"), "{v}");
    let (refused, v) = call(&mut road, STRANGER, "object_set", why.clone()).await;
    assert_eq!(refused, "owner_only", "{v}");

    // The member, in the member's own chat: the same two calls are written.
    let (refused, v) = call(&mut road, OWN_CHAT, "object_confirm", confirm).await;
    assert_eq!(refused, "", "{v}");
    assert_eq!(v["ok"], json!(true), "{v}");
    assert_eq!(
        v["state"],
        json!("active"),
        "the candidate is confirmed: {v}"
    );
    let (refused, v) = call(&mut road, OWN_CHAT, "object_set", why).await;
    assert_eq!(refused, "", "{v}");
    assert_eq!(v["ok"], json!(true), "{v}");

    road.shutdown().await;
}

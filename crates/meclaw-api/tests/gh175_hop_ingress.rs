//! GH #175: `POST /messages` can seed the FIRST hop, opt-in, via a `hop`
//! sibling of `body`/`headers`.
//!
//! Since the hive boundary rule (`docs/meclaw-overview.md` § Die Hive-Grenze) a
//! hive distributes internally with `{"from": "."}` edges conditioned on
//! `hop.route`. The ingress put every inbound header into `context` and started
//! `hop` empty, so a message posted straight at a hive path matched no door and
//! dead-lettered with `hive_no_route` — the only way to open a door from
//! outside was to post at an interior cell, i.e. to break the rule the door
//! exists for.
//!
//! The two-compartment model stays exactly as it was: `headers` is `context`,
//! `hop` is `hop`, and neither infers the other. What the caller gains is a way
//! to SAY which compartment they mean.
//!
//! **The forgery question.** A seeded hop must not reach further than a
//! `modifier.set_hop` reaches. A modifier writes header compartments and
//! nothing else (§ Edge model: "edges operate strictly on the header layer"),
//! with the single sanctioned exception `restore_ttl` — which is a modifier
//! FIELD, not a hop key, and therefore not expressible in a compartment map at
//! all. `envelope_is_out_of_reach` pins that: the envelope names a caller might
//! try to smuggle through the compartment stay inert data, exactly as a
//! `set_hop` would leave them.
//!
//! The routing half — a message whose `hop` matches a hive's `{"from": "."}`
//! edge is transited into the hive — is already pinned by
//! `meclaw-colony/tests/phase_13_5_hive_transit_demo.rs`. What was missing, and
//! is pinned here, is that the ingress can produce such a message at all.

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use meclaw_colony::ColonyMsg;
use std::sync::Arc;
use tower::ServiceExt;

mod common;

/// Router over a RAW mpsc receiver: a 422 must leave the inbox untouched, and
/// an accepted message can be inspected as the envelope the colony would see.
fn raw_colony_app() -> (
    axum::Router,
    tokio::sync::mpsc::Receiver<ColonyMsg>,
    tempfile::TempDir,
) {
    let (inbox_tx, inbox_rx) = tokio::sync::mpsc::channel::<ColonyMsg>(8);
    let api_colony = Arc::new(meclaw_api::ColonyHandle {
        inbox: inbox_tx,
        templates_root: std::path::PathBuf::new(),
    });
    let (blob_store, blob_td) = common::test_blob_store();
    let app = meclaw_api::router::build_router(
        api_colony,
        blob_store,
        9,
        meclaw_api::router::SurfaceState::disabled(),
    );
    (app, inbox_rx, blob_td)
}

/// A valid UBF request at a hive path, with whatever extra fields the case needs.
fn hive_request(extra: &[(&str, serde_json::Value)]) -> serde_json::Value {
    let mut req = serde_json::json!({
        "target": "/talky",
        "body": {"messages": [{"origin": "user", "type": "text", "text": "ping"}]}
    });
    for (k, v) in extra {
        req.as_object_mut()
            .unwrap()
            .insert((*k).to_string(), v.clone());
    }
    req
}

async fn post_json(app: axum::Router, body: &serde_json::Value) -> (StatusCode, serde_json::Value) {
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/messages")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

/// Take the single routed message off the raw inbox.
fn routed(inbox_rx: &mut tokio::sync::mpsc::Receiver<ColonyMsg>) -> meclaw_core::Message {
    match inbox_rx.try_recv().expect("handler must send Route") {
        ColonyMsg::Route { msg, .. } => msg,
        _ => panic!("expected ColonyMsg::Route"),
    }
}

/// The lane the caller asserts arrives in the `hop` compartment — the door a
/// hive conditions on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn seeded_hop_reaches_the_hop_compartment() {
    let (app, mut inbox_rx, _td) = raw_colony_app();
    let req = hive_request(&[("hop", serde_json::json!({"route": "in_turn"}))]);
    let (status, body) = post_json(app, &req).await;
    assert_eq!(status, StatusCode::ACCEPTED, "got {body}");

    let msg = routed(&mut inbox_rx);
    assert_eq!(
        msg.headers.hop.get("route").and_then(|v| v.as_str()),
        Some("in_turn"),
        "the asserted lane must land in `hop`, not in `context`"
    );
    assert!(
        msg.headers.context.is_empty(),
        "a seeded hop must not bleed into the persistent compartment, got {:?}",
        msg.headers.context
    );
}

/// Opt-in: without the field the ingress behaves exactly as before — a source
/// message with an empty hop.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn absent_or_null_hop_stays_empty() {
    for extra in [
        vec![],
        vec![("hop", serde_json::Value::Null)],
        vec![("headers", serde_json::json!({"trace": "abc"}))],
    ] {
        let (app, mut inbox_rx, _td) = raw_colony_app();
        let (status, body) = post_json(app, &hive_request(&extra)).await;
        assert_eq!(status, StatusCode::ACCEPTED, "got {body}");
        let msg = routed(&mut inbox_rx);
        assert!(
            msg.headers.hop.is_empty(),
            "no `hop` field means no asserted lane; got {:?}",
            msg.headers.hop
        );
    }
}

/// Same answer `headers` and `ttl` give: a mistyped compartment is a loud 422,
/// never a silent `{}`. A caller who believes they asserted a lane and got a
/// 202 would debug a `hive_no_route` that the ingress caused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn non_object_hop_is_422() {
    for bad in [
        serde_json::json!("in_turn"),
        serde_json::json!(7),
        serde_json::json!(true),
        serde_json::json!([{"route": "in_turn"}]),
    ] {
        let (app, mut inbox_rx, _td) = raw_colony_app();
        let req = hive_request(&[("hop", bad.clone())]);
        let (status, body) = post_json(app, &req).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "hop={bad} must be rejected, got {body}"
        );
        assert_eq!(
            body["error"], "invalid_hop",
            "hop={bad} must carry the invalid_hop token, got {body}"
        );
        assert!(
            inbox_rx.try_recv().is_err(),
            "hop={bad}: nothing may reach the colony inbox on a 422"
        );
    }
}

/// The two compartments are addressed separately and stay separate, even when
/// they carry the same key — the whole point of naming the compartment.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn headers_and_hop_do_not_cross() {
    let (app, mut inbox_rx, _td) = raw_colony_app();
    let req = hive_request(&[
        ("headers", serde_json::json!({"route": "persistent"})),
        ("hop", serde_json::json!({"route": "in_turn"})),
    ]);
    let (status, body) = post_json(app, &req).await;
    assert_eq!(status, StatusCode::ACCEPTED, "got {body}");

    let msg = routed(&mut inbox_rx);
    assert_eq!(
        msg.headers.context.get("route").and_then(|v| v.as_str()),
        Some("persistent"),
        "`headers` remains the context compartment"
    );
    assert_eq!(
        msg.headers.hop.get("route").and_then(|v| v.as_str()),
        Some("in_turn"),
        "`hop` remains the hop compartment"
    );
}

/// The forgery pin. A modifier reaches the header compartments and nothing
/// else; `restore_ttl` is a modifier FIELD, not a hop key, so no compartment
/// map can express it. Envelope names inside `hop` are therefore inert data —
/// the ttl stays the one the request asked for, and the target stays the one
/// the request addressed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn envelope_is_out_of_reach() {
    let (app, mut inbox_rx, _td) = raw_colony_app();
    let req = hive_request(&[
        ("ttl", serde_json::json!(5)),
        (
            "hop",
            serde_json::json!({
                "restore_ttl": true,
                "ttl": 4096,
                "target": "/elsewhere",
                "reply_to": "/elsewhere",
                "trace_id": "00000000-0000-0000-0000-000000000000",
            }),
        ),
    ]);
    let (status, body) = post_json(app, &req).await;
    assert_eq!(status, StatusCode::ACCEPTED, "got {body}");

    let msg = routed(&mut inbox_rx);
    assert_eq!(
        msg.ttl, 5,
        "the envelope ttl is the request's, not the hop's"
    );
    assert_eq!(
        msg.target.as_str(),
        "/talky",
        "the envelope target is the request's, not the hop's"
    );
    assert_eq!(msg.reply_to, None, "a source message has no reply anchor");
    assert_eq!(
        msg.parent_message_id, None,
        "a source message has no parent"
    );
    assert_eq!(
        msg.headers.hop.get("restore_ttl"),
        Some(&serde_json::json!(true)),
        "the key survives as plain hop data — exactly what set_hop would leave"
    );
}

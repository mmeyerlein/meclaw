//! GH #159 — the return path, driven directly.
//!
//! The dispatcher is fed a fabricated egress channel rather than a colony: every
//! case below is then deterministic, with no wall-clock ordering and no probe that
//! sends a message in order to observe one. The colony half of the same path is
//! proven in `crates/meclaw-colony/tests/gh159_egress_policy.rs`.

use meclaw_api::surface::render::{Dispatcher, EGRESS_MARK, REQUEST_ID, RenderError, SURFACE_PATH};

/// Failure-marker budget for a render that MUST complete (30 s convention,
/// `AGENTS.md` § Coding-Standards).
///
/// It was 5 s and went red once under `cargo`-parallel load — `Err(Timeout)` on a
/// round trip that takes microseconds when it is not competing for a core. That is
/// the exact class the convention exists for: a marker is not a measurement, and
/// nothing here asserts anything about latency. The two deliberately TIGHT budgets
/// in this file (50 ms, 200 ms) are semantic discriminators — they are what the
/// timeout tests are testing — and they stay where they are.
const RENDER_BUDGET: Duration = Duration::from_secs(30);
use meclaw_colony::ColonyMsg;
use meclaw_core::{Body, Message, MessageBuilder, Path};
use std::time::Duration;
use tokio::sync::mpsc;

/// A dispatcher plus the two ends a test needs: what the colony would have
/// received, and the door replies come back through.
struct Rig {
    dispatcher: std::sync::Arc<Dispatcher>,
    colony_rx: mpsc::Receiver<ColonyMsg>,
    egress_tx: mpsc::Sender<Message>,
}

fn rig() -> Rig {
    let (colony_tx, colony_rx) = mpsc::channel::<ColonyMsg>(64);
    let (egress_tx, egress_rx) = mpsc::channel::<Message>(64);
    let (dispatcher, _join) =
        Dispatcher::new(colony_tx, egress_rx, meclaw_core::MESSAGE_DEFAULT_TTL);
    Rig {
        dispatcher,
        colony_rx,
        egress_tx,
    }
}

/// The request id the dispatcher stamped, read off the message it injected.
fn injected_request_id(msg: &ColonyMsg) -> String {
    let ColonyMsg::Route { msg, .. } = msg else {
        panic!("expected a Route");
    };
    msg.headers
        .context
        .get(REQUEST_ID)
        .and_then(|v| v.as_str())
        .expect("a request id must be stamped")
        .to_string()
}

/// A cell's answer: HTML, or an error, for a given request and surface.
fn reply(id: &str, surface: &str, slot: meclaw_core::serde_json::Value) -> Message {
    let mut ctx = meclaw_core::serde_json::Map::new();
    ctx.insert(
        EGRESS_MARK.to_string(),
        meclaw_core::serde_json::json!(true),
    );
    ctx.insert(REQUEST_ID.to_string(), meclaw_core::serde_json::json!(id));
    ctx.insert(
        SURFACE_PATH.to_string(),
        meclaw_core::serde_json::json!(surface),
    );
    MessageBuilder::new(Path::new("/"))
        .context(ctx)
        .body(Body::Inline(
            meclaw_core::serde_json::json!({ "surface": slot }),
        ))
        .build()
}

fn html(text: &str) -> meclaw_core::serde_json::Value {
    meclaw_core::serde_json::json!({ "html": text })
}

/// The request carries the three context keys and nothing about what is drawn:
/// the event is passed through verbatim under the body's `surface` slot.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_request_is_stamped_and_the_event_is_passed_through_verbatim() {
    let mut r = rig();
    let d = r.dispatcher.clone();
    let handle = tokio::spawn(async move {
        d.render(
            "/org/acme/canvy/render",
            meclaw_core::serde_json::json!({ "event": "made:up", "value": { "x": 1 } }),
            RENDER_BUDGET,
        )
        .await
    });

    let sent = r
        .colony_rx
        .recv()
        .await
        .expect("a message must be injected");
    let ColonyMsg::Route { msg, .. } = &sent else {
        panic!("expected Route")
    };
    assert_eq!(msg.target.as_str(), "/org/acme/canvy/render");
    assert!(msg.headers.context.contains_key(EGRESS_MARK));
    assert_eq!(
        msg.headers
            .context
            .get(SURFACE_PATH)
            .and_then(|v| v.as_str()),
        Some("/org/acme/canvy/render")
    );
    let Body::Inline(body) = &msg.body else {
        panic!("inline")
    };
    assert_eq!(
        body["surface"]["event"], "made:up",
        "the HTTP layer must not interpret or rewrite an event"
    );
    assert_eq!(body["surface"]["value"]["x"], 1);

    let id = injected_request_id(&sent);
    r.egress_tx
        .send(reply(&id, "/org/acme/canvy/render", html("<svg/>")))
        .await
        .unwrap();
    assert_eq!(handle.await.unwrap(), Ok("<svg/>".to_string()));
}

/// A reply nobody asked for must not disturb a pending request.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unknown_request_id_is_dropped() {
    let mut r = rig();
    let d = r.dispatcher.clone();
    let handle = tokio::spawn(async move {
        d.render("/s", meclaw_core::serde_json::json!({}), RENDER_BUDGET)
            .await
    });
    let sent = r.colony_rx.recv().await.unwrap();
    let id = injected_request_id(&sent);

    r.egress_tx
        .send(reply("nobody-asked", "/s", html("<stray/>")))
        .await
        .unwrap();
    r.egress_tx
        .send(reply(&id, "/s", html("<mine/>")))
        .await
        .unwrap();

    assert_eq!(
        handle.await.unwrap(),
        Ok("<mine/>".to_string()),
        "the stray reply must not have resolved the pending request"
    );
}

/// **The test that earns its keep.** Two concurrent renders, resolved in REVERSE
/// order of request: a single-slot correlation bug passes every other case in this
/// file and fails only here.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_concurrent_renders_get_their_own_answers() {
    let mut r = rig();
    let d1 = r.dispatcher.clone();
    let d2 = r.dispatcher.clone();

    let first = tokio::spawn(async move {
        d1.render("/a", meclaw_core::serde_json::json!({}), RENDER_BUDGET)
            .await
    });
    let id1 = injected_request_id(&r.colony_rx.recv().await.unwrap());

    let second = tokio::spawn(async move {
        d2.render("/b", meclaw_core::serde_json::json!({}), RENDER_BUDGET)
            .await
    });
    let id2 = injected_request_id(&r.colony_rx.recv().await.unwrap());
    assert_ne!(id1, id2, "two requests must get two ids");

    // Reverse order on purpose.
    r.egress_tx
        .send(reply(&id2, "/b", html("<b/>")))
        .await
        .unwrap();
    r.egress_tx
        .send(reply(&id1, "/a", html("<a/>")))
        .await
        .unwrap();

    assert_eq!(first.await.unwrap(), Ok("<a/>".to_string()));
    assert_eq!(second.await.unwrap(), Ok("<b/>".to_string()));
}

/// A reply that never comes is a timeout, and the waiter is gone afterwards — so
/// a late reply cannot resolve a future request.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_render_that_is_never_answered_times_out_and_unregisters() {
    let mut r = rig();
    let d = r.dispatcher.clone();
    let handle = tokio::spawn(async move {
        d.render(
            "/s",
            meclaw_core::serde_json::json!({}),
            Duration::from_millis(50),
        )
        .await
    });
    let id = injected_request_id(&r.colony_rx.recv().await.unwrap());
    assert_eq!(handle.await.unwrap(), Err(RenderError::Timeout));

    // The late reply arrives. It must not be able to resolve the NEXT request,
    // which is what unregistering on timeout buys.
    r.egress_tx
        .send(reply(&id, "/s", html("<late/>")))
        .await
        .unwrap();

    let d = r.dispatcher.clone();
    let next = tokio::spawn(async move {
        d.render(
            "/s",
            meclaw_core::serde_json::json!({}),
            Duration::from_millis(200),
        )
        .await
    });
    let _ = r.colony_rx.recv().await.unwrap();
    assert_eq!(
        next.await.unwrap(),
        Err(RenderError::Timeout),
        "a late reply must not resolve a later request"
    );
}

/// The cell reported a problem. The browser gets that sentence, not a blank page.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cell_error_comes_back_as_the_cells_own_words() {
    let mut r = rig();
    let d = r.dispatcher.clone();
    let handle = tokio::spawn(async move {
        d.render("/s", meclaw_core::serde_json::json!({}), RENDER_BUDGET)
            .await
    });
    let id = injected_request_id(&r.colony_rx.recv().await.unwrap());
    r.egress_tx
        .send(reply(
            &id,
            "/s",
            meclaw_core::serde_json::json!({ "error": "the store answered nothing" }),
        ))
        .await
        .unwrap();
    assert_eq!(
        handle.await.unwrap(),
        Err(RenderError::CellError("the store answered nothing".into()))
    );
}

/// An answer that is neither html nor error is malformed. Half a canvas is harder
/// to diagnose than a visible failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reply_that_is_neither_html_nor_error_is_malformed() {
    let mut r = rig();
    let d = r.dispatcher.clone();
    let handle = tokio::spawn(async move {
        d.render("/s", meclaw_core::serde_json::json!({}), RENDER_BUDGET)
            .await
    });
    let id = injected_request_id(&r.colony_rx.recv().await.unwrap());
    r.egress_tx
        .send(reply(
            &id,
            "/s",
            meclaw_core::serde_json::json!({ "picture": "yes" }),
        ))
        .await
        .unwrap();
    assert!(matches!(
        handle.await.unwrap(),
        Err(RenderError::Malformed(_))
    ));
}

/// The cache holds the last render and is REPLACED by a newer one, so it can never
/// be older than the last change. That is what makes a second viewer of an
/// unchanged surface cost zero cell calls.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_cache_holds_the_newest_render_and_nothing_older() {
    let mut r = rig();
    assert_eq!(r.dispatcher.cached("/s").await, None, "empty to begin with");

    for (n, markup) in [("first", "<one/>"), ("second", "<two/>")] {
        let d = r.dispatcher.clone();
        let expect = markup.to_string();
        let handle = tokio::spawn(async move {
            d.render("/s", meclaw_core::serde_json::json!({}), RENDER_BUDGET)
                .await
        });
        let id = injected_request_id(&r.colony_rx.recv().await.unwrap());
        r.egress_tx
            .send(reply(&id, "/s", html(markup)))
            .await
            .unwrap();
        assert_eq!(handle.await.unwrap(), Ok(expect.clone()), "{n} render");
        assert_eq!(
            r.dispatcher.cached("/s").await,
            Some(expect),
            "the cache must hold the {n} render"
        );
    }
    assert_eq!(
        r.dispatcher.cached("/other").await,
        None,
        "a surface's cache is its own"
    );
}

/// A render whose reply arrives after the browser left still fills the cache: that
/// render was paid for, and the next viewer should not pay again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reply_with_nobody_waiting_still_fills_the_cache() {
    let mut r = rig();
    let d = r.dispatcher.clone();
    let handle = tokio::spawn(async move {
        d.render(
            "/s",
            meclaw_core::serde_json::json!({}),
            Duration::from_millis(50),
        )
        .await
    });
    let id = injected_request_id(&r.colony_rx.recv().await.unwrap());
    assert_eq!(handle.await.unwrap(), Err(RenderError::Timeout));

    r.egress_tx
        .send(reply(&id, "/s", html("<arrived-late/>")))
        .await
        .unwrap();
    for _ in 0..100 {
        if r.dispatcher.cached("/s").await.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        r.dispatcher.cached("/s").await,
        Some("<arrived-late/>".to_string())
    );
}

/// A colony that is gone is `NoColony`, not a hang.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dropped_colony_inbox_is_reported_not_awaited() {
    let (colony_tx, colony_rx) = mpsc::channel::<ColonyMsg>(1);
    let (_egress_tx, egress_rx) = mpsc::channel::<Message>(1);
    let (d, _join) = Dispatcher::new(colony_tx, egress_rx, meclaw_core::MESSAGE_DEFAULT_TTL);
    drop(colony_rx);
    assert_eq!(
        d.render(
            "/s",
            meclaw_core::serde_json::json!({}),
            Duration::from_secs(30)
        )
        .await,
        Err(RenderError::NoColony)
    );
}

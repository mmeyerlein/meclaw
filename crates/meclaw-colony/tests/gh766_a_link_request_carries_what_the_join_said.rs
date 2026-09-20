//! GH #766 (wave G, T1): a link request carries what the join said.
//!
//! A `voice:` join is fully described by four named fields, so the request that
//! reaches a mount had four of them and nothing else. A `page:` join carries a
//! viewport instead — a shape the door has no business reading, because the
//! cell behind the mount is the only thing that knows what a viewport means.
//! So the request gets ONE generic slot beside the four: whatever the join
//! payload said, minus the `mount` that chose the door (OR-G2, OR-G32).

use meclaw_colony::surfaces::{BoxFuture, LINK_QUEUE};
use meclaw_colony::{
    Link, LinkFrame, LinkOpener, LinkRefused, LinkRequest, SurfaceEntry, SurfaceRegistry,
};
use meclaw_core::Path;
use meclaw_core::serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

/// A mount that answers every link and hands the request it was given back out.
struct Echo {
    seen: std::sync::Mutex<Option<oneshot::Sender<LinkRequest>>>,
}

impl LinkOpener for Echo {
    fn open(&self, req: LinkRequest) -> BoxFuture<'static, Result<Link, LinkRefused>> {
        // Taken before the future, so nothing borrows `self` across the
        // `'static` boundary the trait asks for.
        let told = self.seen.lock().expect("no panic held this lock").take();
        Box::pin(async move {
            if let Some(tx) = told {
                let _ = tx.send(req);
            }
            let (to_cell, _to_cell_rx) = mpsc::channel::<LinkFrame>(LINK_QUEUE);
            let (_from_cell_tx, from_cell) = mpsc::channel::<LinkFrame>(LINK_QUEUE);
            Ok(Link { to_cell, from_cell })
        })
    }
}

/// Register an echo mount and return the request the next link carries to it.
async fn what_the_mount_sees(req: LinkRequest) -> LinkRequest {
    let surfaces = Arc::new(SurfaceRegistry::new());
    let (tx, rx) = oneshot::channel();
    let entry = SurfaceEntry {
        kind: "browser",
        cell_path: Path::new("/echo"),
        links: Some(Arc::new(Echo {
            seen: std::sync::Mutex::new(Some(tx)),
        })),
    };
    let (_handoff, _registration) = surfaces
        .register("browser", entry)
        .await
        .expect("the mount is free");
    surfaces
        .open_link("browser", req)
        .await
        .expect("the mount is there")
        .expect("the echo admits everyone");
    rx.await.expect("the mount saw the request")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_request_carries_the_payload_the_door_did_not_read() {
    let payload = json!({"viewport": {"width": 960, "height": 600, "dpr": 2.0, "mobile": false}});
    let seen = what_the_mount_sees(LinkRequest {
        session: Some("card-7".to_string()),
        params: payload.clone(),
        ..LinkRequest::default()
    })
    .await;
    assert_eq!(seen.session.as_deref(), Some("card-7"));
    assert_eq!(
        seen.params, payload,
        "the payload the door did not read reaches the cell verbatim"
    );
    assert_eq!(
        seen.params.get("viewport").and_then(|v| v.get("width")),
        Some(&json!(960)),
        "and one level deep, not two (OR-G32)"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_request_that_says_nothing_extra_carries_null() {
    let seen = what_the_mount_sees(LinkRequest {
        session: Some("call-1".to_string()),
        mode: Some("push_to_talk".to_string()),
        sample_rate: Some(16_000),
        encoding: Some("pcm_s16le".to_string()),
        params: Value::Null,
    })
    .await;
    assert_eq!(
        seen.params,
        Value::Null,
        "absence is null, not an empty object"
    );
    assert_eq!(
        LinkRequest::default().params,
        Value::Null,
        "and the default says the same"
    );
    assert_eq!(seen.mode.as_deref(), Some("push_to_talk"));
    assert_eq!(seen.sample_rate, Some(16_000));
    assert_eq!(seen.encoding.as_deref(), Some("pcm_s16le"));
}

//! Issue #7: `/health` exposes the per-I/O-task liveness marks.
//!
//! The heartbeat proves the colony select-loop and nothing else, so a proxy
//! whose long poll hung looked exactly like a proxy with nothing to do:
//! `active`, 200 on `/health`, empty log. These tests pin the read side — that
//! `/health` reports, per long-running cell, how long ago its I/O task last
//! completed a successful external round trip, and that the endpoint keeps its
//! status-code semantics (always 200) whatever the colony is doing.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use meclaw_api::ColonyHandle;
use meclaw_api::router::build_router;
use meclaw_colony::ColonyMsg;
use meclaw_colony::api_dto::{IoLivenessDto, ReadLivenessReply};
use std::sync::Arc;
use tower::ServiceExt;

mod common;

/// Builds a router plus a stand-in colony inbox the test drives by hand. The
/// returned `TempDir` must outlive the request (it holds the blob store).
fn router_with_inbox() -> (
    axum::Router,
    tokio::sync::mpsc::Receiver<ColonyMsg>,
    tempfile::TempDir,
) {
    let (inbox, rx) = tokio::sync::mpsc::channel::<ColonyMsg>(8);
    let colony = Arc::new(ColonyHandle {
        inbox,
        templates_root: std::path::PathBuf::new(),
    });
    let (blob_store, blob_td) = common::test_blob_store();
    (
        build_router(
            colony,
            blob_store,
            meclaw_core::MESSAGE_DEFAULT_TTL,
            meclaw_api::router::SurfaceState::disabled(),
        ),
        rx,
        blob_td,
    )
}

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// The core of issue #7: a stalled I/O task must be visible from outside. Two
/// cells, one marking freshly and one whose last successful round trip is long
/// past — `/health` must tell them apart.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn health_reports_the_age_of_each_io_tasks_last_round_trip() {
    let (app, mut rx, _blob_td) = router_with_inbox();

    let colony = tokio::spawn(async move {
        match rx.recv().await.expect("health must ask the colony") {
            ColonyMsg::ReadLiveness { ack } => {
                let _ = ack.send(ReadLivenessReply {
                    entries: vec![
                        IoLivenessDto {
                            path: "/main/hung".into(),
                            last_success_secs: Some(942),
                        },
                        IoLivenessDto {
                            path: "/main/live".into(),
                            last_success_secs: Some(1),
                        },
                        IoLivenessDto {
                            path: "/main/never".into(),
                            last_success_secs: None,
                        },
                    ],
                });
            }
            _ => panic!("/health must read liveness, not something else"),
        }
    });

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    colony.await.unwrap();

    assert_eq!(json["status"], "ok");
    let cells = json["io_liveness"].as_array().expect("io_liveness array");
    assert_eq!(cells.len(), 3);
    assert_eq!(cells[0]["path"], "/main/hung");
    assert_eq!(cells[0]["last_success_secs"], 942);
    assert_eq!(cells[1]["path"], "/main/live");
    assert_eq!(cells[1]["last_success_secs"], 1);
    assert_eq!(cells[2]["path"], "/main/never");
    assert!(
        cells[2]["last_success_secs"].is_null(),
        "an I/O task without a round trip yet must report null, not 0"
    );
}

/// A colony that never answers is exactly the situation `/health` must survive:
/// the endpoint keeps its status-code semantics (200, as before this change) and
/// says that the marks are unavailable instead of hanging or inventing them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn health_stays_200_and_reports_null_when_the_colony_does_not_answer() {
    let (app, rx, _blob_td) = router_with_inbox();
    // Hold the receiver without ever replying: the ack is dropped when the
    // colony would have answered.
    drop(rx);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "/health keeps answering 200 — the marks add visibility, not a verdict"
    );
    let json = body_json(resp).await;
    assert_eq!(json["status"], "ok");
    assert!(
        json["io_liveness"].is_null(),
        "unreadable marks must be null, never an empty list that reads as 'no cells'"
    );
}

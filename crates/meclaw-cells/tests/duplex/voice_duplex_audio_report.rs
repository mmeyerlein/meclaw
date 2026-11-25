//! #1637 -- how a duplex call's audio went, as the cell says it on the
//! `audio_report` lane.
//!
//! **The promise.** With `emit_audio_report` set, the end of a duplex call is
//! one emission on its own lane, after the call's `final` usage: the fields of
//! the report stand in the body beside an empty `messages`, and no word of the
//! call is among them. Off by default. What the fields hold is pinned on the
//! virtual clock in `src/voice/connection.rs`; here it is the lane and its shape.

// harness_fold: #[path = "support/duplex_cell.rs"]
#[allow(unused_imports)]
use crate::duplex_cell; // harness_fold: mod duplex_cell;

use duplex_cell::{Live, boot_fake, duplex_params, emission_route, header, marker};
use meclaw_cells::VoiceCellFactory;
use meclaw_cells::voice::contract::DuplexEvent;
use meclaw_colony::{CellFactory, ContractView, SpawnedCellKind, SurfaceRegistry};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{CellEmission, Path};
use meclaw_testing::voice_client::VoiceClient;
use meclaw_testing::{surface_listener, wait};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Every emission up to and including the first one `last` picks.
async fn until(live: &mut Live, last: impl Fn(&Value) -> bool) -> Vec<Value> {
    let mut seen: Vec<Value> = Vec::new();
    loop {
        let e = tokio::time::timeout(marker(), live.emissions.recv())
            .await
            .unwrap_or_else(|_| panic!("HANG: saw {seen:?}"))
            .expect("the handler is running");
        let done = last(&e.content);
        seen.push(e.content);
        if done {
            return seen;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_call_ends_with_one_audio_report_on_its_lane() {
    let params = duplex_params(json!({
        "default_mode": "auto", "emit_usage": true, "emit_audio_report": true,
    }));
    let mut live = boot_fake(params).await;
    let (client, _hello) = live.connect("session=report-1&mode=auto").await;
    let mut session = live.session().await;
    // Out on the error lane: the session's start was read before it.
    session
        .say(DuplexEvent::Warning {
            detail: "marker".to_string(),
            provider_code: None,
            client_event_id: None,
        })
        .await;
    until(&mut live, |e| emission_route(e) == Some("error")).await;
    drop(client);
    session.finish();

    let seen = until(&mut live, |e| emission_route(e) == Some("audio_report")).await;
    let before = seen[..seen.len() - 1].iter();
    assert!(
        before
            .filter(|e| emission_route(e) == Some("usage"))
            .any(|e| header(e, "final") == Some(&json!(true))),
        "behind the final usage: {seen:?}"
    );
    let report = seen.last().expect("just read");
    for key in ["session_id", "call_id"] {
        assert_eq!(header(report, key), Some(&json!("report-1")), "{report}");
    }
    assert_eq!(header(report, "engine"), Some(&json!("duplex")), "{report}");
    let body = report.as_object().expect("a body");
    let mut keys: Vec<&str> = body.keys().map(String::as_str).collect();
    keys.retain(|key| *key != "header");
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "chunks",
            "ended_epoch_ms",
            "forward_wait_max_ms",
            "gaps",
            "gaps_beyond_cap",
            "held_max_ms",
            "late",
            "late_max_ms",
            "lateness",
            "lead_ms",
            "messages",
            "model",
            "provider_send_wait_max_ms",
            "send_wait_max_ms",
            "started_epoch_ms",
        ]
    );
    assert_eq!(body["messages"], json!([]), "no word of the call");
    assert_eq!(body["lateness"], json!([0, 0, 0, 0, 0]));
    assert_eq!(body["model"], Value::Null, "the loopback has no model");

    // The next call reports for itself, and this one does not again.
    let (client, _hello) = live.connect("session=report-2&mode=auto").await;
    let mut session = live.session().await;
    session
        .say(DuplexEvent::Warning {
            detail: "marker".to_string(),
            provider_code: None,
            client_event_id: None,
        })
        .await;
    drop(client);
    session.finish();
    let seen = until(&mut live, |e| emission_route(e) == Some("audio_report")).await;
    let last = seen.last().expect("just read");
    assert_eq!(header(last, "call_id"), Some(&json!("report-2")), "{last}");
}

/// The `final` usage of a second call is the marker: whatever the first call
/// emitted at its end is in the channel before it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_report_lane_is_silent_unless_asked() {
    let params = duplex_params(json!({"default_mode": "auto", "emit_usage": true}));
    let mut live = boot_fake(params).await;
    let mut seen = Vec::new();
    for call in ["quiet-1", "quiet-2"] {
        let (client, _hello) = live.connect(&format!("session={call}&mode=auto")).await;
        let mut session = live.session().await;
        drop(client);
        session.finish();
        seen.extend(
            until(&mut live, |e| {
                emission_route(e) == Some("usage")
                    && header(e, "call_id") == Some(&json!(call))
                    && header(e, "final") == Some(&json!(true))
            })
            .await,
        );
    }
    assert!(
        seen.iter()
            .all(|e| emission_route(e) != Some("audio_report")),
        "off by default: {seen:?}"
    );
}

/// The factory hands both params to the half that runs the call: a cell spawned
/// with them reports, and the report names the lead it ran with.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_spawned_cell_reports_with_the_lead_it_was_born_with() {
    let dir = tempfile::tempdir().expect("tempdir");
    let surfaces = Arc::new(SurfaceRegistry::new());
    let (out_tx, mut emissions) = mpsc::channel::<CellEmission>(512);
    let (inbox_tx, _inbox) = mpsc::channel(8);
    let spawned = Arc::new(VoiceCellFactory::new(Arc::clone(&surfaces)))
        .spawn_cell(
            Path::new("/voice"),
            json!({
                "mount": "voice",
                "duplex": {"provider": "echo"},
                "emit_audio_report": true,
                "audio_out_lead_ms": 30,
            }),
            out_tx,
            dir.path().to_path_buf(),
            ContractView::default(),
            inbox_tx,
            None,
            -1,
            None,
            None,
            64,
        )
        .expect("a voice cell spawns");
    let SpawnedCellKind::Active {
        sender: _mailbox, ..
    } = spawned
    else {
        panic!("a voice cell is an eager, long-running kind");
    };
    let (addr, _listener) = surface_listener(Arc::clone(&surfaces)).await;
    wait::until("the cell registers its mount", || {
        let surfaces = Arc::clone(&surfaces);
        async move { !surfaces.table().await.is_empty() }
    })
    .await;
    let url = format!("ws://{addr}/voice/ws?session=born-1&mode=auto");
    let (mut client, _hello) = VoiceClient::connect(&url).await.expect("a socket");
    // 20 ms at the loopback's rate, there and back: the session carried audio.
    client.send_audio(&[0; 960]).await.expect("a frame");
    loop {
        let frame = client.next_frame(marker()).await.expect("the echo comes");
        if frame.as_audio().is_some() {
            break;
        }
    }
    client.close().await.expect("the client hangs up");
    let report = loop {
        let e = tokio::time::timeout(marker(), emissions.recv())
            .await
            .expect("HANG: no report")
            .expect("the cell runs");
        if emission_route(&e.content) == Some("audio_report") {
            break e.content;
        }
    };
    assert_eq!(report["lead_ms"], json!(30), "{report}");
}

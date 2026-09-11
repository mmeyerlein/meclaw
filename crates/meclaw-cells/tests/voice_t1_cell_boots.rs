//! Wave voice-cell, strand t1: the cell exists, refuses loudly, and mounts.
//!
//! Two claims, and they are deliberately on different sides of the dual task.
//!
//! The first needs no I/O at all: a message a `voice` cell cannot say out loud
//! must come back as exactly one error emission with a code from the closed
//! list. The failure mode this pins is the quiet one — an assistant turn that
//! never reaches a caller and never reaches a log, so the operator's only
//! evidence is a silence they cannot tell from a slow model. The `slack` cell's
//! handler tests are the shape.
//!
//! The second is the whole cell: spawned through its factory, the listener has
//! to be up. It was `#[ignore]`d while `run_io` was a stub that parked forever
//! (A1′) and bound nothing; strand t4 replaced it, so this is now the first
//! proof that the two halves boot as one cell.

use meclaw_cells::VoiceCellFactory;
use meclaw_cells::voice::cell::VoiceCell;
use meclaw_cells::voice::contract::ProviderTimeouts;
use meclaw_cells::voice::io::VoiceIo;
use meclaw_cells::voice::params::VoiceParams;
use meclaw_cells::voice::providers::build_stt;
use meclaw_cells::voice::wire::Mode;
use meclaw_colony::{CellFactory, ContractView, DbConn, LongRunningCell, SpawnedCellKind};
use meclaw_core::serde_json::json;
use meclaw_core::{Body, CellEmission, MessageBuilder, OriginSink, OutputSink, Path, Uuid};
use meclaw_testing::surface_listener;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

const CELL_PATH: &str = "/main/members/tester/channels/voice";

/// The mount the fixtures of this file register. Since `voice@2.0.0` it is the
/// cell's only door.
const MOUNT: &str = "voice";

/// An echo-provider cell with no colony around it.
///
/// `emit_partials` is ordered explicitly: it is off by default (R-V8'), and
/// the tests below are about what reaches the lane, so they have to start from
/// a cell whose lane is on.
fn echo_cell(path: &Path) -> VoiceCell {
    let raw = json!({"mount": MOUNT, "stt": {"provider": "echo"}, "emit_partials": true});
    let params = VoiceParams::parse(&raw).expect("the echo config parses");
    let stt = build_stt(&params.stt, ProviderTimeouts::default()).expect("the echo provider");
    let (events_tx, _events_rx) = mpsc::channel(8);
    let io = VoiceIo::new(
        params.mount.clone(),
        stt,
        None,
        Mode::Auto,
        Duration::from_millis(params.external_timeout_ms),
        Duration::from_millis(params.provider_idle_timeout_ms),
        events_tx,
    );
    VoiceCell::new(path.clone(), io, &params, &raw)
}

fn db() -> DbConn {
    let conn = rusqlite::Connection::open_in_memory().expect("db");
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS params (
             key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at INTEGER NOT NULL);",
    )
    .expect("the overlay table");
    DbConn::wrap(conn, None)
}

fn error_code(em: &CellEmission) -> Option<String> {
    em.content
        .get("header")
        .and_then(|h| h.get("error_code"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// A body with no assistant turn is answered, not dropped.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_body_without_an_assistant_turn_is_refused_by_name() {
    let path = Path::new(CELL_PATH);
    let mut cell = echo_cell(&path);
    let mut db = db();
    let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        path.clone(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        64,
        meclaw_core::Headers::new(),
        None,
    );
    let (reconfig_tx, _reconfig_rx) = mpsc::channel(8);

    let msg = MessageBuilder::new(path.clone())
        .body(Body::Inline(json!({"messages": []})))
        .build();
    cell.handle(msg, &sink, &mut db, &reconfig_tx).await;

    let mut emissions = Vec::new();
    while let Ok(em) = rx.try_recv() {
        emissions.push(em);
    }
    assert_eq!(emissions.len(), 1, "exactly one answer, never silence");
    assert_eq!(
        error_code(&emissions[0]).as_deref(),
        Some("invalid_body"),
        "the code is from the closed list: {:?}",
        emissions[0].content
    );
}

/// The whole cell, through its factory: the mount has to be reachable.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_voice_cell_spawned_by_its_factory_serves_its_mount() {
    let td = tempfile::tempdir().expect("tempdir");
    let surfaces = Arc::new(meclaw_colony::SurfaceRegistry::new());
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(64);
    let (inbox_tx, _inbox_rx) = mpsc::channel(8);

    let spawned = Arc::new(VoiceCellFactory::new(Arc::clone(&surfaces)))
        .spawn_cell(
            Path::new(CELL_PATH),
            json!({"mount": MOUNT, "stt": {"provider": "echo"}}),
            out_tx,
            td.path().to_path_buf(),
            ContractView::default(),
            inbox_tx,
            None,
            -1,
            None,
            None,
            64,
        )
        .expect("a voice cell spawns");
    let SpawnedCellKind::Active { join, .. } = spawned else {
        panic!("a voice cell is an eager, long-running kind");
    };

    let (addr, listener) = surface_listener(Arc::clone(&surfaces)).await;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        // The helper answers `404 not found` for a segment nobody mounted, so
        // the discriminator is the STATUS and not the absence of an answer.
        let answered = reqwest::get(format!("http://{addr}/{MOUNT}/info"))
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false);
        if answered {
            break;
        }
        assert!(Instant::now() < deadline, "the cell never took its mount");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    listener.abort();
    join.abort();
}

/// GH #601: a client that stopped taking frames leaves a reason on the `error`
/// lane, not only a log line.
///
/// The I/O half decides by count — `DISPATCH_QUEUE` commands queued behind an
/// already full connection channel — and the ruling of 2026-09-09 is that the
/// decision is a message. Exactly one, carrying the call it ended under both
/// names and the number of queued commands that went with it, so a colony can
/// tell a caller who hung up from a client that stopped reading.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_that_stopped_reading_leaves_its_reason_on_the_error_lane() {
    use meclaw_cells::voice::cell::VoiceEvent;

    let path = Path::new(CELL_PATH);
    let mut cell = echo_cell(&path);
    let mut db = db();
    let (tx, mut rx) = mpsc::channel::<CellEmission>(32);
    let sink = OriginSink::new(tx, path.clone(), 64);

    cell.handle_event(
        VoiceEvent::Connected {
            session_id: "call-1".to_string(),
            mode: Mode::Auto,
        },
        &sink,
        &mut db,
    )
    .await;
    cell.handle_event(
        VoiceEvent::ClientTooSlow {
            session_id: "call-1".to_string(),
            dropped: 65,
        },
        &sink,
        &mut db,
    )
    .await;
    cell.handle_event(
        VoiceEvent::Disconnected {
            session_id: "call-1".to_string(),
        },
        &sink,
        &mut db,
    )
    .await;

    let mut emissions = Vec::new();
    while let Ok(em) = rx.try_recv() {
        emissions.push(em);
    }
    let reported: Vec<&CellEmission> = emissions
        .iter()
        .filter(|em| error_code(em).as_deref() == Some("client_too_slow"))
        .collect();
    assert_eq!(
        reported.len(),
        1,
        "one give-up, one message: {:?}",
        emissions.iter().map(error_code).collect::<Vec<_>>()
    );
    let header = reported[0]
        .content
        .get("header")
        .expect("every emission carries a header");
    assert_eq!(
        header.get("route").and_then(|v| v.as_str()),
        Some("error"),
        "the give-up rides the error lane: {header:?}"
    );
    assert_eq!(
        (
            header.get("session_id").and_then(|v| v.as_str()),
            header.get("call_id").and_then(|v| v.as_str())
        ),
        (Some("call-1"), Some("call-1")),
        "R-V15: the report names the call, under both names a colony wires against: {header:?}"
    );
    assert_eq!(
        header.get("dropped_frames").and_then(|v| v.as_u64()),
        Some(65),
        "the hop carries how many queued commands went with the session: {header:?}"
    );
    let detail = reported[0]
        .content
        .get("meta")
        .and_then(|m| m.get("detail"))
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    assert!(
        detail.contains("65"),
        "and the detail says the number out loud, for a reader who never parses a hop: \
         {detail:?}"
    );
}

/// R-V15: every emission that belongs to a connection names its session.
///
/// Telephony is why this is a rule and not a nicety — a colony that answers the
/// wrong caller is worse than one that answers nobody, and `session_id` on the
/// hop is the only thing that keeps two live calls apart. The two refusals that
/// may go without it are the ones that have no session to name: a mailbox
/// message with an unreadable body, and one that never said which call it meant.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_connection_bound_emission_carries_session_id() {
    use meclaw_cells::voice::cell::VoiceEvent;
    use meclaw_cells::voice::contract::SttEvent;
    use meclaw_cells::voice::wire::SpeakEndReason;

    let path = Path::new(CELL_PATH);
    let mut cell = echo_cell(&path);
    let mut db = db();
    let (tx, mut rx) = mpsc::channel::<CellEmission>(32);
    let sink = OriginSink::new(tx, path.clone(), 64);

    cell.handle_event(
        VoiceEvent::Connected {
            session_id: "call-1".to_string(),
            mode: Mode::Auto,
        },
        &sink,
        &mut db,
    )
    .await;

    let events = vec![
        VoiceEvent::Stt {
            session_id: "call-1".to_string(),
            event: SttEvent::Partial {
                text: "hallo".to_string(),
                eager: false,
            },
        },
        VoiceEvent::Stt {
            session_id: "call-1".to_string(),
            event: SttEvent::EndOfTurn {
                text: "hallo".to_string(),
            },
        },
        VoiceEvent::Stt {
            session_id: "call-1".to_string(),
            event: SttEvent::Failed {
                detail: "socket closed".to_string(),
            },
        },
        VoiceEvent::SpeakEnded {
            session_id: "call-1".to_string(),
            speak_id: "sp1".to_string(),
            reason: SpeakEndReason::Failed,
            detail: Some("provider said no".to_string()),
        },
        VoiceEvent::BadAudioFrame {
            session_id: "call-1".to_string(),
            len: 7,
            count: 3,
        },
    ];
    for event in events {
        cell.handle_event(event, &sink, &mut db).await;
    }

    let mut emissions = Vec::new();
    while let Ok(em) = rx.try_recv() {
        emissions.push(em);
    }
    assert!(
        emissions.len() >= 5,
        "partial, turn, stt_failed, speak_failed and bad_audio_frame all emit: {}",
        emissions.len()
    );
    for em in &emissions {
        let session = em
            .content
            .get("header")
            .and_then(|h| h.get("session_id"))
            .and_then(|v| v.as_str());
        assert_eq!(
            session,
            Some("call-1"),
            "an emission without its session cannot be routed back to its caller: {:?}",
            em.content
        );
    }

    // R-V6': the mis-framed buffer is reported with its count, and the call is
    // still there afterwards — a dropped frame does not end a conversation.
    let bad = emissions
        .iter()
        .find(|em| {
            em.content
                .get("header")
                .and_then(|h| h.get("error_code"))
                .and_then(|v| v.as_str())
                == Some("bad_audio_frame")
        })
        .expect("the bad frame reached the error lane");
    assert_eq!(
        bad.content
            .get("header")
            .and_then(|h| h.get("bad_frames"))
            .and_then(|v| v.as_u64()),
        Some(3),
        "the hop carries how many times it has happened: {:?}",
        bad.content
    );

    cell.handle_event(
        VoiceEvent::Stt {
            session_id: "call-1".to_string(),
            event: SttEvent::EndOfTurn {
                text: "it goes on".to_string(),
            },
        },
        &sink,
        &mut db,
    )
    .await;
    let mut after = Vec::new();
    while let Ok(em) = rx.try_recv() {
        after.push(em);
    }
    assert!(
        after.iter().any(|em| em
            .content
            .get("header")
            .and_then(|h| h.get("route"))
            .and_then(|v| v.as_str())
            == Some("turn")),
        "the session survived the bad frame and still produces turns: {after:?}"
    );
}

/// SHOULD 3: an accepted flag update reaches the calls that are already up.
///
/// The failure mode is the half-applied setting: `barge_in: false` accepted,
/// remembered, reported — and the one caller who happened to be mid-sentence
/// still gets cut off, which is exactly the caller the operator was turning it
/// off for.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_flag_update_reaches_open_sessions() {
    use meclaw_cells::voice::cell::{VoiceEvent, VoiceReconfig};
    use meclaw_cells::voice::contract::SttEvent;

    let path = Path::new(CELL_PATH);
    let mut cell = echo_cell(&path);
    // Drain the command seam so a full channel cannot be mistaken for a
    // behaviour change.
    let mut io = cell.split_io().expect("the i/o half");
    let mut from_handler = io.from_handler.take().expect("the command seam");
    let (seen_tx, mut seen_rx) = mpsc::channel::<&'static str>(16);
    tokio::spawn(async move {
        while let Some(cmd) = from_handler.recv().await {
            let name = match cmd {
                VoiceReconfig::CancelSpeak { .. } => "cancel",
                _ => "other",
            };
            let _ = seen_tx.send(name).await;
        }
    });

    let mut db = db();
    let (tx, mut rx) = mpsc::channel::<CellEmission>(16);
    let out = OutputSink::new(
        tx.clone(),
        path.clone(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        64,
        meclaw_core::Headers::new(),
        None,
    );
    let origin = OriginSink::new(tx, path.clone(), 64);
    let (reconfig_tx, _reconfig_rx) = mpsc::channel(8);

    cell.handle_event(
        VoiceEvent::Connected {
            session_id: "call-1".to_string(),
            mode: Mode::Auto,
        },
        &origin,
        &mut db,
    )
    .await;

    // Give the call something to interrupt: without a running synthesis,
    // `SpeechStarted` is a no-op whatever `barge_in` says, and the test would
    // pass on a cell that ignores the update entirely.
    let mut ctx = meclaw_core::serde_json::Map::new();
    ctx.insert("session_id".to_string(), json!("call-1"));
    let speak = MessageBuilder::new(path.clone())
        .context(ctx)
        .body(Body::Inline(json!({
            "messages": [{"origin": "assistant", "type": "text", "text": "guten tag"}]
        })))
        .build();
    cell.handle(speak, &out, &mut db, &reconfig_tx).await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(30), seen_rx.recv())
            .await
            .expect("the speak order reached the i/o half within 30s"),
        Some("other"),
        "the speak order reached the i/o half"
    );

    // Positive control, BEFORE the update: with `barge_in` still on, speech
    // does cut the synthesis short. Without this the negative assertion below
    // would also pass on a cell that never cancels anything at all — and the
    // control has to run on this session and at this moment, because the
    // update reaches every open session and a session opened after it starts
    // from the new value.
    cell.handle_event(
        VoiceEvent::Stt {
            session_id: "call-1".to_string(),
            event: SttEvent::SpeechStarted,
        },
        &origin,
        &mut db,
    )
    .await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(30), seen_rx.recv())
            .await
            .expect("barge_in:true cancels within 30s"),
        Some("cancel"),
        "barge_in is on: speech interrupts the synthesis"
    );

    // Turn both flags off while the call is live.
    let msg = MessageBuilder::new(path.clone())
        .body(Body::Inline(
            json!({"params": {"barge_in": false, "emit_partials": false}}),
        ))
        .build();
    cell.handle(msg, &out, &mut db, &reconfig_tx).await;
    if let Ok(em) = rx.try_recv() {
        panic!("an accepted params update is silent, got {:?}", em.content);
    }
    // A cancel does not end the synthesis by itself — only a `SpeakEnded` from
    // the I/O half does — so the session is still speaking and the negative
    // case below is testing what it says it is.

    // `emit_partials: false` — the client still sees the mirror, the topology
    // does not see the lane.
    cell.handle_event(
        VoiceEvent::Stt {
            session_id: "call-1".to_string(),
            event: SttEvent::Partial {
                text: "hallo".to_string(),
                eager: false,
            },
        },
        &origin,
        &mut db,
    )
    .await;
    let mut emissions = Vec::new();
    while let Ok(em) = rx.try_recv() {
        emissions.push(em);
    }
    assert!(
        emissions.is_empty(),
        "emit_partials:false must stop the lane for a call that was already up: {emissions:?}"
    );

    // The interim still travels to the client — `emit_partials` stops the lane,
    // never the mirror — so the frame shows up on the command seam. Take it off
    // before asserting silence, and assert it while we are here.
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(30), seen_rx.recv())
            .await
            .expect("the client mirror arrives within 30s"),
        Some("other"),
        "emit_partials:false stops the lane, not the frame the client sees"
    );

    // `barge_in: false` — speech no longer cuts a synthesis short.
    cell.handle_event(
        VoiceEvent::Stt {
            session_id: "call-1".to_string(),
            event: SttEvent::SpeechStarted,
        },
        &origin,
        &mut db,
    )
    .await;
    // A deadline, not a `try_recv`: the command travels through a channel and
    // a task, so an immediate poll wins the scheduler hop and a cell that DID
    // cancel would still look quiet. Half a second is long enough for the hop
    // the positive control above just demonstrated.
    assert!(
        tokio::time::timeout(Duration::from_millis(500), seen_rx.recv())
            .await
            .is_err(),
        "barge_in:false must reach the open call, not only the next one"
    );
}

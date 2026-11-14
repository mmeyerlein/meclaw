//! GH #1511 -- the duplex connection and `gpt_live` must
//! never wait on each other.
//!
//! The connection hands guidance to the session with `control_tx.send(..).await`
//! and reads the session's events and audio in the same loop, so while it
//! waits for room on the guidance channel it reads nothing. `gpt_live` handed
//! what the model said to the connection with `events.send(..).await` /
//! `audio_out.send(..).await` and read no guidance while it waited. Both
//! channels full at once -- a burst of guidance while the model talks -- and
//! each side waited for the other for ever: no timeout covers either wait,
//! because neither is an operation on the socket.
//!
//! The pin fills the session's outlet (nobody reads it), proves by the I/O
//! liveness marks that the session holds the next frame of the model in hand,
//! and only then hands over more guidance than the guidance channel holds.
//! The session has to keep taking guidance, put every item on the wire, and
//! still deliver everything the model said, in order, once the outlet is read
//! again: backpressure on the model, never on the colony's guidance.

use meclaw_cells::voice::contract::{
    AppendKind, DuplexControl, DuplexError, DuplexEvent, DuplexProvider, DuplexSession,
    ProviderTimeouts,
};
use meclaw_cells::voice::params::GptLiveParams;
use meclaw_cells::voice::providers::gpt_live::GptLiveDuplex;
use meclaw_colony::{ColonyMsg, IoLivenessMark};
use meclaw_core::Path;
use meclaw_testing::deadline::HANG;
use meclaw_testing::mock_gpt_live::{LiveAction, LiveScript, MockGptLive};
use serde_json::json;
use std::time::Duration;
use tokio::sync::mpsc;

/// The guidance handed over while the outlet is full: three items through a
/// channel of one, so at least one `send` can only finish when the session
/// takes an item off the channel.
const GUIDANCE: [&str; 3] = ["guidance-0", "guidance-1", "guidance-2"];

/// Marks before the flood: the handshake and `session.started`.
const OPENING_MARKS: usize = 2;

/// Which of the session's two outlets the model floods.
#[derive(Clone, Copy)]
enum Outlet {
    Events,
    Audio,
}

fn params(base_url: &str) -> GptLiveParams {
    serde_json::from_value(json!({
        "api_key": "test-key",
        "base_url": base_url,
        "instructions": "Be Egon.",
        // No keepalive inside the test: its pong is a mark too, and the count
        // below has to mean "session frames read".
        "keepalive_ms": 600_000,
    }))
    .expect("params parse")
}

/// Waits for the `n`-th successful I/O mark.
async fn marks_reach(marks_rx: &mut mpsc::Receiver<ColonyMsg>, n: usize) {
    let mut seen = 0;
    while seen < n {
        match tokio::time::timeout(HANG, marks_rx.recv()).await {
            Err(_) => panic!("HANG: no I/O mark {}/{n} within {HANG:?}", seen + 1),
            Ok(None) => panic!("the session ended before mark {}/{n}", seen + 1),
            Ok(Some(ColonyMsg::IoLiveness { at: Some(_), .. })) => seen += 1,
            Ok(Some(_)) => {}
        }
    }
}

async fn next_event(events_rx: &mut mpsc::Receiver<DuplexEvent>, what: &str) -> DuplexEvent {
    tokio::time::timeout(HANG, events_rx.recv())
        .await
        .unwrap_or_else(|_| panic!("HANG: no `{what}` within {HANG:?}"))
        .unwrap_or_else(|| panic!("the session ended before `{what}`"))
}

async fn runs_while_the_outlet_is_full(outlet: Outlet) {
    // The model says three things at once, then waits for the guidance.
    let mut actions = match outlet {
        Outlet::Events => (1..=3)
            .map(|s| LiveAction::Usage {
                seconds: f64::from(s),
                ratio: None,
            })
            .collect::<Vec<_>>(),
        Outlet::Audio => (1..=3u8)
            .map(|b| LiveAction::SendAudio(vec![b; 640]))
            .collect(),
    };
    actions.extend(GUIDANCE.iter().map(|g| LiveAction::ExpectAppend {
        kind: "commentary",
        contains: g,
    }));
    actions.push(LiveAction::Delay(Duration::from_secs(600)));
    let mock = MockGptLive::start(LiveScript {
        session_id: "sess_outlet_full".to_string(),
        actions,
        ..LiveScript::default()
    })
    .await
    .expect("bind fake");

    let (_audio_in_tx, audio_in_rx) = mpsc::channel(32);
    let (audio_out_tx, mut audio_out_rx) = mpsc::channel::<Vec<u8>>(match outlet {
        Outlet::Audio => 1,
        Outlet::Events => 32,
    });
    let (events_tx, mut events_rx) = mpsc::channel::<DuplexEvent>(match outlet {
        Outlet::Events => 1,
        Outlet::Audio => 32,
    });
    let (control_tx, control_rx) = mpsc::channel::<DuplexControl>(1);
    let (marks_tx, mut marks_rx) = mpsc::channel::<ColonyMsg>(64);
    let provider = GptLiveDuplex::new(params(&mock.base_url())).with_timeouts(ProviderTimeouts {
        external: Duration::from_secs(30),
        idle: Duration::from_secs(600),
    });
    let join = tokio::spawn(provider.run_session(
        provider.format(),
        DuplexSession {
            audio_in: audio_in_rx,
            audio_out: audio_out_tx,
            events: events_tx,
            control: control_rx,
        },
        IoLivenessMark::new(Path::new("/voice"), Some(marks_tx)),
    ));

    assert!(
        matches!(
            next_event(&mut events_rx, "Started").await,
            DuplexEvent::Started { .. }
        ),
        "the session opened"
    );
    // The first frame of the model fills the outlet of one, the second is in
    // the session's hand: read off the socket (marked) with nowhere to go.
    marks_reach(&mut marks_rx, OPENING_MARKS + 2).await;

    for (n, g) in GUIDANCE.iter().enumerate() {
        tokio::time::timeout(
            HANG,
            control_tx.send(DuplexControl::Append {
                kind: AppendKind::Commentary,
                event_id: format!("ev_{n}"),
                delegation_id: None,
                content: (*g).to_string(),
            }),
        )
        .await
        .unwrap_or_else(|_| {
            panic!(
                "HANG: guidance {n} not taken within {HANG:?} while the model's outlet \
                 was full -- the session waits on the connection that waits on it"
            )
        })
        .expect("the session was still taking guidance");
    }

    // Now the outlet is read again: everything the model said arrives, in
    // order, and every guidance item reached the wire and was acknowledged.
    match outlet {
        Outlet::Events => {
            for want in 1..=3 {
                match next_event(&mut events_rx, "Usage").await {
                    DuplexEvent::Usage { seconds, .. } => assert_eq!(
                        seconds,
                        f64::from(want),
                        "what the model said arrives in its order, nothing dropped"
                    ),
                    other => panic!("expected usage {want}, got {other:?}"),
                }
            }
        }
        Outlet::Audio => {
            for want in 1..=3u8 {
                let chunk = tokio::time::timeout(HANG, audio_out_rx.recv())
                    .await
                    .unwrap_or_else(|_| panic!("HANG: no audio chunk {want} within {HANG:?}"))
                    .expect("the session ended before its audio");
                assert!(
                    chunk.iter().all(|b| *b == want),
                    "the model's audio arrives in its order, nothing dropped"
                );
            }
        }
    }
    // The fake answers each expected append once it is on the wire (its
    // acknowledgement names the latest append, so the order is read off the
    // wire below, not off the acknowledgements).
    for n in 0..GUIDANCE.len() {
        match next_event(&mut events_rx, "Appended").await {
            DuplexEvent::Appended { .. } => {}
            other => panic!("expected the acknowledgement of guidance {n}, got {other:?}"),
        }
    }
    let on_the_wire: Vec<String> = mock
        .client_events()
        .await
        .iter()
        .filter(|e| e.get("type").and_then(|t| t.as_str()) == Some("session.commentary.append"))
        .filter_map(|e| {
            e.get("content")
                .and_then(|c| c.as_str())
                .map(str::to_string)
        })
        .collect();
    assert_eq!(
        on_the_wire, GUIDANCE,
        "every guidance item handed over while the outlet was full reached the wire, in order"
    );

    control_tx
        .send(DuplexControl::Close)
        .await
        .expect("the session was still taking guidance");
    loop {
        if let DuplexEvent::Closed { .. } = next_event(&mut events_rx, "Closed").await {
            break;
        }
    }
    let verdict: Result<(), DuplexError> = tokio::time::timeout(HANG, join)
        .await
        .expect("HANG: the session did not end after its close")
        .expect("the session task did not panic");
    assert!(
        verdict.is_ok(),
        "a requested close is not a failure: {verdict:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_weblive_gpt_live_takes_guidance_while_its_event_outlet_is_full() {
    runs_while_the_outlet_is_full(Outlet::Events).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_weblive_gpt_live_takes_guidance_while_its_audio_outlet_is_full() {
    runs_while_the_outlet_is_full(Outlet::Audio).await;
}

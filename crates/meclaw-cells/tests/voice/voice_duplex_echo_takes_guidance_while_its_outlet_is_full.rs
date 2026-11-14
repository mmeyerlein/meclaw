//! GH #1511 -- the duplex connection and the `echo` loopback
//! must never wait on each other.
//!
//! The same cycle closed in `gpt_live`: the connection hands
//! guidance to the session with `control_tx.send(..).await` and reads the
//! session's events and audio in the same loop, so while it waits for room on
//! the guidance channel it reads nothing. The loopback answered every append
//! with `events.send(..).await` and echoed every frame with
//! `audio_out.send(..).await`, and read no guidance while it waited. Both
//! channels full at once and each side waited for the other for ever.
//!
//! Two pins, one per outlet. The event outlet: an append is answered with two
//! events, so a channel of one is full after the first answer, and more
//! guidance than the guidance channel holds can only be taken by a session
//! that keeps reading it. The audio outlet: the first echoed frame fills it,
//! the second is proven (by the I/O liveness marks) to be in the session's
//! hand, and only then the guidance comes. Either way the session has to take
//! every item and, once the outlet is read again, deliver everything it owes,
//! in order, nothing dropped -- a loopback that dropped would measure the drop
//! rather than the wire.

use meclaw_cells::voice::contract::{
    AppendKind, DuplexControl, DuplexError, DuplexEvent, DuplexProvider, DuplexSession, Speaker,
};
use meclaw_cells::voice::providers::duplex_echo::EchoDuplex;
use meclaw_colony::{ColonyMsg, IoLivenessMark};
use meclaw_core::Path;
use meclaw_testing::deadline::HANG;
use tokio::sync::mpsc;

/// The guidance handed over while an outlet is full: more items than the
/// guidance channel of one holds, so at least one `send` can only finish when
/// the session takes an item off the channel.
const GUIDANCE: [&str; 4] = ["guidance-0", "guidance-1", "guidance-2", "guidance-3"];

/// Which of the session's two outlets nobody reads.
#[derive(Clone, Copy)]
enum Outlet {
    Events,
    Audio,
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

async fn next_frame(audio_out_rx: &mut mpsc::Receiver<Vec<u8>>, want: u8) {
    let frame = tokio::time::timeout(HANG, audio_out_rx.recv())
        .await
        .unwrap_or_else(|_| panic!("HANG: no echoed frame {want} within {HANG:?}"))
        .expect("the session ended before its echo");
    assert!(
        frame.iter().all(|b| *b == want),
        "the echo arrives in the order the audio came in, nothing dropped"
    );
}

async fn runs_while_the_outlet_is_full(outlet: Outlet) {
    let (audio_in_tx, audio_in_rx) = mpsc::channel::<Vec<u8>>(32);
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
    let provider = EchoDuplex::new(16_000);
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

    if let Outlet::Audio = outlet {
        // The first echo fills the outlet of one, the second frame is in the
        // session's hand: read off `audio_in` (marked) with nowhere to go.
        for b in 1..=2u8 {
            audio_in_tx.send(vec![b; 640]).await.expect("session alive");
        }
        marks_reach(&mut marks_rx, 2).await;
    }

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
                "HANG: guidance {n} not taken within {HANG:?} while the loopback's outlet \
                 was full -- the session waits on the connection that waits on it"
            )
        })
        .expect("the session was still taking guidance");
    }

    // The outlet is read again: every frame comes back, and the audio is read
    // again once the held frame is out.
    if let Outlet::Audio = outlet {
        next_frame(&mut audio_out_rx, 1).await;
        next_frame(&mut audio_out_rx, 2).await;
        audio_in_tx.send(vec![3; 640]).await.expect("session alive");
        next_frame(&mut audio_out_rx, 3).await;
    }

    // Every append answered and mirrored, in the order it was handed over.
    for (n, g) in GUIDANCE.iter().enumerate() {
        match next_event(&mut events_rx, "Appended").await {
            DuplexEvent::Appended { event_id, .. } => assert_eq!(
                event_id,
                format!("ev_{n}"),
                "the answers come in the order the guidance was handed over"
            ),
            other => panic!("expected the answer to guidance {n}, got {other:?}"),
        }
        match next_event(&mut events_rx, "Transcript").await {
            DuplexEvent::Transcript {
                speaker: Speaker::Assistant,
                delta,
                ..
            } => assert_eq!(delta, *g, "the mirror of guidance {n}, nothing dropped"),
            other => panic!("expected the mirror of guidance {n}, got {other:?}"),
        }
    }

    control_tx
        .send(DuplexControl::Close)
        .await
        .expect("the session was still taking guidance");
    match next_event(&mut events_rx, "Closed").await {
        DuplexEvent::Closed { reason, .. } => assert_eq!(reason, "close_requested"),
        other => panic!("expected the close, got {other:?}"),
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
async fn gh_d54g_weblive_echo_takes_guidance_while_its_event_outlet_is_full() {
    runs_while_the_outlet_is_full(Outlet::Events).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_weblive_echo_takes_guidance_while_its_audio_outlet_is_full() {
    runs_while_the_outlet_is_full(Outlet::Audio).await;
}

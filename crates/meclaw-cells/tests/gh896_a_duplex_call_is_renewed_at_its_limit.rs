//! GH #896 — a duplex call outlives the session limit of its live model.
//!
//! **The promise.** A hosted live model ends a session at a limit of its own.
//! With `renew_after_ms` set, the `voice` connection opens the call's NEXT
//! provider session while the current one is still running — same call, same
//! `call_id`, same connection — announces it on the `renewed` lane, hands the
//! colony's handover block to the new session BEFORE any audio of the caller
//! reaches it, and makes it the session the call runs on at the first quiet
//! moment of the line after that — never while the model speaks, at the
//! latest `spoken_cap_ms` after the block (review I-2). The old session loses
//! its way in, finishes its sentence and closes; no frame of the caller's is
//! lost on the way, and no frame of two models shares the line. Without the
//! block the new session is due at `renew_grace_ms`. A renewal that is gone
//! before it takes over leaves the call on the session it runs on (review
//! I-3). It is a renewal and not a reconnect: a session that ends on its own
//! still ends the call, as it did before (OR-KY-G7).
//!
//! **How it is measured.** At the seam, not inside the connection: a fake
//! provider (`tests/support/duplex_cell.rs`) numbers its sessions and writes
//! every control frame and every audio frame it is told into a journal, tagged
//! with the session that received it and in the order that session received
//! it. The handler's own lanes are read off its emissions, the client's off a
//! real socket. Timing only appears as a discriminator where the promise is
//! about time, and every such number says in place why it cannot be met by
//! accident.

#[path = "support/duplex_cell.rs"]
mod duplex_cell;

use duplex_cell::{
    FRAME_BYTES, Live, Seen, advise_msg, boot_fake, boot_renewing, boot_renewing_failing,
    boot_renewing_timed, duplex_params, emission_route, handover_msg, header, message_text,
};
use meclaw_cells::voice::contract::{AppendKind, DuplexControl, DuplexEvent, Speaker};
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::voice_client::Frame;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// The failure marker of this file: generous, never a discriminator.
const MARKER: Duration = Duration::from_secs(30);

/// A renewal opens this long after a session took charge of the call. Short,
/// so a test does not wait for it; long enough that the first session has
/// clearly been in charge before the second one exists.
const RENEW_AFTER_MS: u64 = 300;

/// A grace no test in this file reaches, where the handover is what the test
/// is about: the takeover must come from the block, never from the clock.
const NO_GRACE_MS: u64 = 20_000;

/// The block a colony writes in answer to the `renewed` lane.
const HANDOVER: &str = "Handover: the caller asked for the opening hours of the library; \
                        the answer was nine to six.";

/// One 20 ms frame whose every byte is `n`, so a frame names itself.
fn frame(n: u8) -> Vec<u8> {
    vec![n; FRAME_BYTES]
}

/// A duplex cell in `auto`, the mode telephony runs.
fn params() -> Value {
    duplex_params(json!({"default_mode": "auto"}))
}

/// Milliseconds since the Unix epoch, on the test's side.
fn epoch_ms() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after 1970")
            .as_millis(),
    )
    .expect("fits")
}

/// The next `n` audio frames any session of the fake was fed, with the session
/// each one reached.
async fn tagged_audio(live: &mut Live, n: usize) -> Vec<(usize, Vec<u8>)> {
    let mut out: Vec<(usize, Vec<u8>)> = Vec::with_capacity(n);
    while out.len() < n {
        let got = tokio::time::timeout(MARKER, live.tagged_audio.recv())
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "only {} of {n} frames reached a session within the failure marker; \
                     sessions: {:?}",
                    out.len(),
                    out.iter().map(|(i, _)| *i).collect::<Vec<_>>()
                )
            })
            .expect("the fake is running");
        out.push(got);
    }
    out
}

/// The next control frame any session was told, with the session it reached.
async fn tagged_control(live: &mut Live) -> (usize, DuplexControl) {
    tokio::time::timeout(MARKER, live.tagged_controls.recv())
        .await
        .expect("a control frame reaches a session within the failure marker")
        .expect("the fake is running")
}

/// The next journal entry of session `index`, skipping every other session's.
async fn next_seen_at(live: &mut Live, index: usize) -> Seen {
    let deadline = Instant::now() + MARKER;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let (at, seen) = tokio::time::timeout(left, live.journal.recv())
            .await
            .unwrap_or_else(|_| panic!("session {index} was told nothing within the marker"))
            .expect("the fake is running");
        if at == index {
            return seen;
        }
    }
}

/// Journal entries until `n` of them were audio, and the sessions they reached.
async fn journal_audio(live: &mut Live, n: usize) -> Vec<(usize, Vec<u8>)> {
    let mut out = Vec::new();
    while out.len() < n {
        let (at, seen) = tokio::time::timeout(MARKER, live.journal.recv())
            .await
            .expect("the journal moves within the failure marker")
            .expect("the fake is running");
        if let Seen::Audio(bytes) = seen {
            out.push((at, bytes));
        }
    }
    out
}

/// A second session is opened `renew_after_ms` into the call, on the same
/// connection, through `run_renewed_session` — and the client is told nothing:
/// no close, no second `hello`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn renew_after_ms_opens_a_second_session_in_the_same_call() {
    let mut live = boot_renewing(params(), RENEW_AFTER_MS, NO_GRACE_MS).await;
    let (mut client, hello) = live.connect("session=renew-1&mode=auto").await;
    let greeted = Instant::now();
    assert_eq!(hello["duplex"], true, "got {hello}");

    let first = live.session().await;
    assert_eq!(
        (first.index, first.renewed),
        (1, false),
        "the call's first session is an ordinary one"
    );
    let second = live.session().await;
    let waited = greeted.elapsed();
    assert_eq!(
        (second.index, second.renewed),
        (2, true),
        "the second session is opened as a RENEWAL, which a greeting provider does not greet"
    );
    // The renewal clock starts when the first session is started, which is
    // after the `hello` was sent and so after it could be read here. What can
    // make the gap look shorter is only the loopback delivering the `hello`,
    // which is far below the 100 ms of slack allowed for it; a renewal opened
    // at once would be seen within a few milliseconds.
    assert!(
        waited >= Duration::from_millis(RENEW_AFTER_MS - 100),
        "the renewal opened {waited:?} after the hello, before renew_after_ms"
    );
    assert_eq!(live.starts.load(Ordering::SeqCst), 2);
    assert_eq!(live.renewed_starts.load(Ordering::SeqCst), 1);

    // The same connection, and it still carries the call: the caller's audio
    // reaches the session in charge, which is still the first one — the
    // renewal has not taken over (no handover, and a grace this test never
    // reaches).
    client
        .send_audio(&frame(1))
        .await
        .expect("the socket is open");
    let got = tagged_audio(&mut live, 1).await;
    assert_eq!(got, vec![(1, frame(1))]);
    let frames = client.drain_for(Duration::from_millis(200)).await;
    assert!(
        frames.iter().all(|(_, f)| f.as_close().is_none()),
        "a renewal closes nothing: {frames:?}"
    );
    assert!(
        !frames.iter().any(|(_, f)| f.is_type("hello")),
        "a renewal is not a new connection: {frames:?}"
    );
}

/// The `renewed` lane names the call under both keys, counts the renewals of
/// the call, and stamps the moment in epoch milliseconds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn renewed_route_carries_call_id_and_count() {
    // A short grace so the second renewal follows the first inside the test.
    let mut live = boot_renewing(params(), RENEW_AFTER_MS, 200).await;
    let before = epoch_ms();
    let (_client, _hello) = live.connect("session=renew-2&mode=auto").await;

    let first = live.emission("renewed").await;
    let after = epoch_ms();
    let hop = first.get("header").cloned().unwrap_or_default();
    assert_eq!(hop["route"], json!("renewed"), "{first}");
    assert_eq!(hop["call_id"], json!("renew-2"), "{first}");
    assert_eq!(hop["session_id"], json!("renew-2"), "{first}");
    assert_eq!(hop["platform"], json!("voice"), "{first}");
    assert_eq!(hop["engine"], json!("duplex"), "{first}");
    assert_eq!(hop["mode"], json!("auto"), "{first}");
    assert_eq!(
        hop["renewal_n"],
        json!(1),
        "a number, counting from 1: {first}"
    );
    let at = hop["renewed_at"]
        .as_u64()
        .unwrap_or_else(|| panic!("renewed_at is a number: {first}"));
    assert!(
        (before..=after).contains(&at),
        "renewed_at is epoch milliseconds of the moment the renewal started, between \
         {before} and {after}; got {at}"
    );
    assert_eq!(
        first["messages"],
        json!([]),
        "no words on this lane: {first}"
    );

    let second = live.emission("renewed").await;
    assert_eq!(header(&second, "renewal_n"), Some(&json!(2)), "{second}");
    assert_eq!(header(&second, "call_id"), Some(&json!("renew-2")));
    assert!(live.renewed_starts.load(Ordering::SeqCst) >= 2);
}

/// The lock sentence of the plan: the handover reaches the new session BEFORE
/// the caller does, and audio sent before the takeover reached the old one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_renewed_session_hears_its_handover_before_the_caller() {
    let mut live = boot_renewing(params(), RENEW_AFTER_MS, NO_GRACE_MS).await;
    let (mut client, _hello) = live.connect("session=renew-3&mode=auto").await;
    let _first = live.session().await;

    for n in 1..=5 {
        client
            .send_audio(&frame(n))
            .await
            .expect("the socket is open");
    }
    let before = journal_audio(&mut live, 5).await;
    assert!(
        before.iter().all(|(at, _)| *at == 1),
        "before any renewal the caller's audio reaches session 1: {:?}",
        before.iter().map(|(i, _)| *i).collect::<Vec<_>>()
    );

    let renewed = live.emission("renewed").await;
    assert_eq!(header(&renewed, "renewal_n"), Some(&json!(1)));
    let second = live.session().await;
    assert_eq!(second.index, 2);

    // Opened and started, not in charge: the caller still talks to session 1.
    for n in 6..=8 {
        client
            .send_audio(&frame(n))
            .await
            .expect("the socket is open");
    }
    let waiting = journal_audio(&mut live, 3).await;
    assert_eq!(
        waiting,
        vec![(1, frame(6)), (1, frame(7)), (1, frame(8))],
        "a renewal hears nothing of the caller before it takes over"
    );
    while let Ok((at, seen)) = live.journal.try_recv() {
        assert_ne!(at, 2, "session 2 was told {seen:?} before its handover");
    }

    live.send(handover_msg("renew-3", HANDOVER, 1)).await;
    match next_seen_at(&mut live, 2).await {
        Seen::Control(DuplexControl::Append {
            kind,
            delegation_id,
            content,
            ..
        }) => {
            assert_eq!(kind, AppendKind::Thinking, "a handover is known, not said");
            assert_eq!(delegation_id, None);
            assert_eq!(content, HANDOVER);
        }
        other => {
            panic!("the FIRST thing the renewed session is told is its handover; got {other:?}")
        }
    }

    // And only now the caller.
    for n in 9..=11 {
        client
            .send_audio(&frame(n))
            .await
            .expect("the socket is open");
    }
    let after = journal_audio(&mut live, 3).await;
    assert_eq!(
        after,
        vec![(2, frame(9)), (2, frame(10)), (2, frame(11))],
        "after the takeover the caller's audio reaches the new session, in order"
    );
}

/// After the takeover the old session's ear is shut — its `audio_in` ends and
/// it finalises — every frame the caller sent reached exactly one session, in
/// order, and what the old model still says reaches the caller.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_old_session_closes_after_the_takeover() {
    let mut live = boot_renewing(params(), RENEW_AFTER_MS, NO_GRACE_MS).await;
    let (mut client, _hello) = live.connect("session=renew-4&mode=auto").await;
    let mut first = live.session().await;

    let mut sent: Vec<Vec<u8>> = Vec::new();
    for n in 1..=10 {
        client
            .send_audio(&frame(n))
            .await
            .expect("the socket is open");
        sent.push(frame(n));
    }
    let mut seen = tagged_audio(&mut live, 10).await;

    let _renewed = live.emission("renewed").await;
    let _second = live.session().await;
    // Sent while the renewal waits, and waited for AT session 1 before the
    // handover goes out: the socket and the mailbox are two roads, and the
    // claim is about audio that reached the connection before the takeover.
    for n in 11..=20 {
        client
            .send_audio(&frame(n))
            .await
            .expect("the socket is open");
        sent.push(frame(n));
    }
    seen.extend(tagged_audio(&mut live, 10).await);

    live.send(handover_msg("renew-4", HANDOVER, 1)).await;
    let ended = tokio::time::timeout(MARKER, live.ended.recv())
        .await
        .expect("the old session's audio ends within the failure marker")
        .expect("the fake is running");
    assert_eq!(ended, 1, "it is the OLD session whose ear is shut");

    for n in 21..=30 {
        client
            .send_audio(&frame(n))
            .await
            .expect("the socket is open");
        sent.push(frame(n));
    }
    seen.extend(tagged_audio(&mut live, 10).await);

    let at_first: Vec<Vec<u8>> = seen
        .iter()
        .filter(|(i, _)| *i == 1)
        .map(|(_, b)| b.clone())
        .collect();
    let at_second: Vec<Vec<u8>> = seen
        .iter()
        .filter(|(i, _)| *i == 2)
        .map(|(_, b)| b.clone())
        .collect();
    assert_eq!(
        at_first.len() + at_second.len(),
        sent.len(),
        "every frame reached exactly one session"
    );
    assert_eq!(
        at_first,
        sent[..20].to_vec(),
        "session 1: frames 1..=20, in order"
    );
    assert_eq!(
        at_second,
        sent[20..].to_vec(),
        "session 2: frames 21..=30, in order"
    );

    // The old model finishing its sentence after the takeover: the caller
    // hears it.
    let last_words = vec![0x5A; FRAME_BYTES];
    first
        .audio_out
        .send(last_words.clone())
        .await
        .expect("the connection still reads the retiring session's audio");
    let heard = client
        .collect_until(|f| f.as_audio() == Some(last_words.as_slice()), MARKER)
        .await;
    assert!(
        heard
            .iter()
            .any(|(_, f)| f.as_audio() == Some(last_words.as_slice())),
        "the retiring session's audio reaches the client: {} frames, none of them it",
        heard.len()
    );

    // Let it finalise. The call goes on: nothing closes the client.
    first.finish();
    let frames = client.drain_for(Duration::from_millis(300)).await;
    assert!(
        frames.iter().all(|(_, f)| f.as_close().is_none()),
        "the old session closing is not the call closing: {frames:?}"
    );
    assert!(
        live.ended.try_recv().is_err(),
        "the new session's ear is still open"
    );
}

/// Without a handover the renewal takes over at its grace — not at once, and
/// not never — and the first thing the new session is told is the caller.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_a_block_the_renewed_session_takes_over_at_its_grace() {
    /// The grace under test.
    const GRACE_MS: u64 = 800;
    // A renewal every second rather than every 300 ms, so the NEXT one cannot
    // take over inside the window this test reads: that one is at the earliest
    // RENEW + GRACE after this takeover, 1 800 ms, against a probe 700 ms in.
    let mut live = boot_renewing(params(), 1_000, GRACE_MS).await;
    let (mut client, _hello) = live.connect("session=renew-5&mode=auto").await;
    let _first = live.session().await;

    let _renewed = live.emission("renewed").await;
    // The grace was armed when the renewal started, which is BEFORE the lane
    // reached this test: the takeover is due at the latest GRACE_MS from here.
    let seen_at = Instant::now();
    let _second = live.session().await;

    // Inside the grace. The frame has the grace minus two local hops to reach
    // the connection — milliseconds against 800.
    client
        .send_audio(&frame(1))
        .await
        .expect("the socket is open");
    assert_eq!(
        tagged_audio(&mut live, 1).await,
        vec![(1, frame(1))],
        "inside the grace the caller still talks to the session in charge"
    );

    // Past it, by 700 ms.
    tokio::time::sleep_until((seen_at + Duration::from_millis(GRACE_MS + 700)).into()).await;
    client
        .send_audio(&frame(2))
        .await
        .expect("the socket is open");
    assert_eq!(
        tagged_audio(&mut live, 1).await,
        vec![(2, frame(2))],
        "past the grace the renewal has taken over without a block"
    );
    assert_eq!(
        next_seen_at(&mut live, 2).await,
        Seen::Audio(frame(2)),
        "no handover was ever appended: the first thing session 2 heard was the caller"
    );
}

/// A session that ends on its own ends the call, exactly as before this
/// strand: no renewal is started to stand in for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dropped_socket_still_ends_the_call() {
    /// Far enough out that the first session is certainly gone before it.
    const RENEW_MS: u64 = 1_000;
    let mut live = boot_renewing_failing(params(), RENEW_MS, NO_GRACE_MS).await;
    let (mut client, _hello) = live.connect("session=renew-6&mode=auto").await;
    let greeted = Instant::now();

    let error = live.emission("error").await;
    assert_eq!(
        header(&error, "error_code"),
        Some(&json!("duplex_failed")),
        "{error}"
    );
    let closed = client
        .collect_until(|f| f.as_close().is_some(), MARKER)
        .await;
    let code = closed
        .iter()
        .find_map(|(_, f)| f.as_close())
        .expect("the client is told the call is over");
    assert_eq!(code, 1011, "got {closed:?}");

    // Past the moment the call would have renewed, with room: nothing opened.
    tokio::time::sleep_until((greeted + Duration::from_millis(RENEW_MS + 300)).into()).await;
    assert_eq!(
        live.starts.load(Ordering::SeqCst),
        1,
        "the session was the conversation, and it is over"
    );
    assert_eq!(live.renewed_starts.load(Ordering::SeqCst), 0);
}

/// The same with a renewal already started and waiting: the session in charge
/// drops its socket, and the call ends — the waiting renewal does not step in.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dropped_socket_ends_the_call_even_with_a_renewal_waiting() {
    let mut live = boot_renewing(params(), RENEW_AFTER_MS, NO_GRACE_MS).await;
    let (mut client, _hello) = live.connect("session=renew-7&mode=auto").await;
    let first = live.session().await;
    let _renewed = live.emission("renewed").await;
    let _second = live.session().await;

    first.hang_up();
    let closed = client
        .collect_until(|f| f.as_close().is_some(), MARKER)
        .await;
    let code = closed
        .iter()
        .find_map(|(_, f)| f.as_close())
        .expect("the call ends although a renewal was waiting to take over");
    assert_eq!(
        code, 1000,
        "a socket that tore is an ordinary end with a warning, as on the real adapter: \
         {closed:?}"
    );
    let warning = live.emission("error").await;
    assert_eq!(
        header(&warning, "error_code"),
        Some(&json!("duplex_warning")),
        "{warning}"
    );
    assert_eq!(
        live.starts.load(Ordering::SeqCst),
        2,
        "the first session and the renewal, and nothing after them"
    );
}

/// With the shipped default nothing renews: one session, and no `renewed`
/// emission. The window is 1.5 s; the default itself is pinned by
/// `params::tests::a_renewal_is_off_unless_a_host_sets_it`, so what this adds
/// is the path — a cell built without the knob gets the default and runs on it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn renew_after_ms_zero_changes_nothing() {
    let mut live = boot_fake(params()).await;
    let (_client, _hello) = live.connect("session=renew-8&mode=auto").await;
    let _first = live.session().await;

    let until = Instant::now() + Duration::from_millis(1_500);
    let mut routes = Vec::new();
    loop {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        match tokio::time::timeout(left, live.emissions.recv()).await {
            Ok(Some(e)) => routes.push(emission_route(&e.content).unwrap_or_default().to_string()),
            Ok(None) | Err(_) => break,
        }
    }
    assert!(
        !routes.iter().any(|r| r == "renewed"),
        "nothing renews by default: {routes:?}"
    );
    assert_eq!(live.starts.load(Ordering::SeqCst), 1);
    assert_eq!(live.renewed_starts.load(Ordering::SeqCst), 0);
}

/// An answer to a delegation the OLD session opened is spoken on the new one
/// without its handle — the new model never saw it — while a delegation the
/// new session opened keeps its handle.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_answer_for_a_delegation_of_the_old_session_is_spoken_without_its_handle() {
    let mut live = boot_renewing(params(), RENEW_AFTER_MS, NO_GRACE_MS).await;
    let (_client, _hello) = live.connect("session=renew-9&mode=auto").await;
    let first = live.session().await;
    first
        .say(DuplexEvent::DelegationCreated {
            delegation_id: "dlg-old".to_string(),
            offset_ms: 100,
        })
        .await;
    let delegation = live.emission("delegation").await;
    assert_eq!(
        header(&delegation, "delegation_id"),
        Some(&json!("dlg-old"))
    );

    let _renewed = live.emission("renewed").await;
    let second = live.session().await;
    live.send(handover_msg("renew-9", HANDOVER, 1)).await;
    match tagged_control(&mut live).await {
        (2, DuplexControl::Append { kind, .. }) => assert_eq!(kind, AppendKind::Thinking),
        other => panic!("the handover reaches session 2 first; got {other:?}"),
    }

    live.send(advise_msg(
        "renew-9",
        "fact",
        "It is sunny.",
        Some("dlg-old"),
    ))
    .await;
    match tagged_control(&mut live).await {
        (
            2,
            DuplexControl::Append {
                kind,
                delegation_id,
                content,
                ..
            },
        ) => {
            assert_eq!(kind, AppendKind::Commentary);
            assert_eq!(
                delegation_id, None,
                "the handle belongs to a session that is gone; the answer is a late fact"
            );
            assert_eq!(content, "It is sunny.");
        }
        other => panic!("the answer reaches the session in charge; got {other:?}"),
    }

    second
        .say(DuplexEvent::DelegationCreated {
            delegation_id: "dlg-new".to_string(),
            offset_ms: 50,
        })
        .await;
    let delegation = live.emission("delegation").await;
    assert_eq!(
        header(&delegation, "delegation_id"),
        Some(&json!("dlg-new"))
    );
    live.send(advise_msg(
        "renew-9",
        "fact",
        "It is raining.",
        Some("dlg-new"),
    ))
    .await;
    match tagged_control(&mut live).await {
        (2, DuplexControl::Append { delegation_id, .. }) => assert_eq!(
            delegation_id.as_deref(),
            Some("dlg-new"),
            "a handle of the session in charge is kept"
        ),
        other => panic!("the answer reaches the session in charge; got {other:?}"),
    }
}

/// The new session counts its clock from zero; the call's clock does not. A
/// turn the caller says on the new session lands AFTER everything said on the
/// old one.
///
/// The discriminator: the old session stamps its sentence at 5 000..5 400 ms,
/// the new one stamps the next at 100..600 ms of ITS clock. Unmoved, the new
/// turn happens at 600 -- five seconds before the old one; moved onto the
/// call's timeline it happens after 5 400, where the clock stood when the new
/// session took over. The takeover waits for a quiet line (review I-2), so the
/// caller's sentence on the old session is over before the new one hears
/// anything: one sentence is never split between two sessions.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_renewed_timeline_continues_where_the_old_one_stood() {
    const QUIET_MS: u64 = 300;
    let mut live =
        boot_renewing_timed(params(), RENEW_AFTER_MS, NO_GRACE_MS, QUIET_MS, 8_000, 100).await;
    let (_client, _hello) = live.connect("session=renew-10&mode=auto").await;
    let first = live.session().await;
    let _renewed = live.emission("renewed").await;
    let second = live.session().await;

    first
        .say(DuplexEvent::Transcript {
            speaker: Speaker::User,
            delta: "Hello there.".to_string(),
            start_ms: 5_000,
            end_ms: 5_400,
        })
        .await;
    let partial = live.emission("partial").await;
    assert_eq!(message_text(&partial, 0), "Hello there.");
    // The old sentence's turn closes first. On ONE timeline a caller who goes
    // on 400 ms later is still in the same turn (`turn_gap_ms` 1 000), and the
    // quiet moment the takeover waits for is shorter than that -- measured
    // on the first run of this lock: "Hello there." and "Next question" came
    // out as one turn, and the turn this test waits for never did.
    let first_turn = live.emission("turn").await;
    assert_eq!(message_text(&first_turn, 0), "Hello there.");

    live.send(handover_msg("renew-10", HANDOVER, 1)).await;
    match tagged_control(&mut live).await {
        (2, DuplexControl::Append { .. }) => {}
        other => panic!("the handover reaches session 2 first; got {other:?}"),
    }
    // The takeover: the old session's ear is shut, once the line was quiet.
    let ended = tokio::time::timeout(MARKER, live.ended.recv())
        .await
        .expect("the renewal takes over within the failure marker")
        .expect("the fake is running");
    assert_eq!(ended, 1);

    second
        .say(DuplexEvent::Transcript {
            speaker: Speaker::User,
            delta: "Next question".to_string(),
            start_ms: 100,
            end_ms: 600,
        })
        .await;
    let turn = loop {
        let turn = live.emission("turn").await;
        if message_text(&turn, 0) == "Next question" {
            break turn;
        }
    };
    let happened_at = header(&turn, "happened_at")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| panic!("happened_at is a number: {turn}"));
    assert!(
        happened_at > 5_400,
        "the new session's words are placed AFTER the old one's on the call's clock, \
         got {happened_at}"
    );
}

/// The first byte of every audio frame the client heard, in order: a chunk of
/// the fake is one byte value throughout, so the byte names the model.
fn heard_bytes(frames: &[(Instant, Frame)]) -> Vec<u8> {
    frames
        .iter()
        .filter_map(|(_, f)| f.as_audio().and_then(|a| a.first().copied()))
        .collect()
}

/// How many bytes of audio the client heard from the model whose byte is `b`.
fn heard_len(frames: &[(Instant, Frame)], b: u8) -> usize {
    frames
        .iter()
        .filter_map(|(_, f)| f.as_audio())
        .filter(|a| a.first() == Some(&b))
        .map(<[u8]>::len)
        .sum()
}

/// Review I-2, the lock sentence: a renewal while the model speaks. The block
/// reaches the new session at once, but the CALL stays with the old one until
/// the line is quiet -- the caller's frames reach the session that is speaking
/// to them -- and at the client no frame of the new model ever comes before a
/// frame of the old one's: two voices are never interleaved on one line.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_renewal_while_the_model_speaks_takes_over_in_the_quiet() {
    const QUIET_MS: u64 = 300;
    const OLD: u8 = 0x11;
    const NEW: u8 = 0x22;
    let mut live = boot_renewing_timed(
        params(),
        RENEW_AFTER_MS,
        NO_GRACE_MS,
        QUIET_MS,
        8_000,
        1_000,
    )
    .await;
    let (mut client, _hello) = live.connect("session=renew-11&mode=auto").await;
    let first = live.session().await;
    let _renewed = live.emission("renewed").await;
    let second = live.session().await;

    // The old model speaks: a chunk every 40 ms for 1.2 s.
    let voice = first.audio_out.clone();
    let speaking = tokio::spawn(async move {
        for _ in 0..30 {
            voice
                .send(vec![OLD; FRAME_BYTES])
                .await
                .expect("the connection reads the model's audio");
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    live.send(handover_msg("renew-11", HANDOVER, 1)).await;
    match next_seen_at(&mut live, 2).await {
        Seen::Control(DuplexControl::Append { content, .. }) => assert_eq!(content, HANDOVER),
        other => panic!("the block reaches the renewal at once; got {other:?}"),
    }
    // Mid-speech: the caller still talks to the session that is speaking.
    client
        .send_audio(&frame(1))
        .await
        .expect("the socket is open");
    assert_eq!(
        tagged_audio(&mut live, 1).await,
        vec![(1, frame(1))],
        "no takeover while the model speaks"
    );
    speaking.await.expect("the old model finished its sentence");
    let stopped = Instant::now();

    // Quiet for QUIET_MS: the takeover. The next frame is the new session's.
    // 300 ms of slack on top is far more than two local hops and far less
    // than any stretch in which the fake says anything.
    tokio::time::sleep_until((stopped + Duration::from_millis(QUIET_MS + 300)).into()).await;
    client
        .send_audio(&frame(2))
        .await
        .expect("the socket is open");
    assert_eq!(
        tagged_audio(&mut live, 1).await,
        vec![(2, frame(2))],
        "after the quiet the renewal has taken over"
    );
    second
        .audio_out
        .send(vec![NEW; FRAME_BYTES])
        .await
        .expect("the connection reads the new model's audio");
    let heard = client
        .collect_until(
            |f| f.as_audio().is_some_and(|a| a.first() == Some(&NEW)),
            MARKER,
        )
        .await;
    let order = heard_bytes(&heard);
    let first_new = order
        .iter()
        .position(|b| *b == NEW)
        .expect("the new model is heard");
    assert!(
        order[first_new..].iter().all(|b| *b != OLD),
        "no frame of the old model after the new one spoke: {order:?}"
    );
    assert_eq!(
        heard_len(&heard, OLD),
        30 * FRAME_BYTES,
        "the old model's sentence was heard whole, before the new one: {order:?}"
    );
}

/// Review I-2, the cap: a model that never stops talking still hands the call
/// over, `spoken_cap_ms` after the block -- and once the new model speaks, the
/// tail of the old one is dropped, so the caller never hears both at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_model_that_never_stops_is_taken_over_at_the_cap_and_never_heard_twice() {
    const CAP_MS: u64 = 800;
    const OLD: u8 = 0x33;
    const NEW: u8 = 0x44;
    let mut live =
        boot_renewing_timed(params(), RENEW_AFTER_MS, NO_GRACE_MS, 300, CAP_MS, 1_000).await;
    let (mut client, _hello) = live.connect("session=renew-12&mode=auto").await;
    let first = live.session().await;
    let _renewed = live.emission("renewed").await;
    let second = live.session().await;

    // The old model speaks for 3 s without a pause -- long past the cap.
    let voice = first.audio_out.clone();
    let speaking = tokio::spawn(async move {
        for _ in 0..75 {
            if voice.send(vec![OLD; FRAME_BYTES]).await.is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    live.send(handover_msg("renew-12", HANDOVER, 1)).await;
    let briefed = Instant::now();
    match next_seen_at(&mut live, 2).await {
        Seen::Control(DuplexControl::Append { .. }) => {}
        other => panic!("the block reaches the renewal at once; got {other:?}"),
    }
    client
        .send_audio(&frame(1))
        .await
        .expect("the socket is open");
    assert_eq!(
        tagged_audio(&mut live, 1).await,
        vec![(1, frame(1))],
        "inside the cap, the call stays with the model that speaks"
    );
    // Past the cap, by 500 ms.
    tokio::time::sleep_until((briefed + Duration::from_millis(CAP_MS + 500)).into()).await;
    client
        .send_audio(&frame(2))
        .await
        .expect("the socket is open");
    assert_eq!(
        tagged_audio(&mut live, 1).await,
        vec![(2, frame(2))],
        "past the cap the renewal has taken over, speech or not"
    );
    // The new model answers while the old one is still streaming its tail.
    for _ in 0..5 {
        second
            .audio_out
            .send(vec![NEW; FRAME_BYTES])
            .await
            .expect("the connection reads the new model's audio");
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    let heard = client.drain_for(Duration::from_millis(600)).await;
    let order = heard_bytes(&heard);
    let first_new = order
        .iter()
        .position(|b| *b == NEW)
        .unwrap_or_else(|| panic!("the new model is heard: {order:?}"));
    assert!(
        order[first_new..].iter().all(|b| *b != OLD),
        "once the new model speaks, the old one's tail is dropped: {order:?}"
    );
    speaking.abort();
}

/// Review I-3 (and M-8): a renewal whose provider is gone before it takes
/// over -- here gone while the connection cannot yet know it from its events
/// -- takes nothing over. The call stays on the session it runs on, the colony
/// is told on the warning lane, and the block meant for the renewal is not
/// read to the session in charge, which was in the conversation all along.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_renewal_gone_before_its_takeover_leaves_the_call_on_its_session() {
    let mut live = boot_renewing(params(), RENEW_AFTER_MS, NO_GRACE_MS).await;
    let (mut client, _hello) = live.connect("session=renew-13&mode=auto").await;
    let _first = live.session().await;
    let _renewed = live.emission("renewed").await;
    let mut second = live.session().await;
    // Its provider task ends; this handle keeps its event channel open.
    second.stop().await;

    live.send(handover_msg("renew-13", HANDOVER, 1)).await;
    let warning = live.emission("error").await;
    assert_eq!(
        header(&warning, "error_code"),
        Some(&json!("duplex_warning")),
        "{warning}"
    );
    client
        .send_audio(&frame(1))
        .await
        .expect("the socket is open");
    assert_eq!(
        tagged_audio(&mut live, 1).await,
        vec![(1, frame(1))],
        "the call goes on on the session it ran on"
    );
    // A second copy of the block, now that the renewal is known to be gone.
    live.send(handover_msg("renew-13", HANDOVER, 1)).await;
    client
        .send_audio(&frame(2))
        .await
        .expect("the socket is open");
    assert_eq!(tagged_audio(&mut live, 1).await, vec![(1, frame(2))]);
    while let Ok((at, seen)) = live.journal.try_recv() {
        assert!(
            !matches!(seen, Seen::Control(DuplexControl::Append { .. })),
            "session {at} was read a handover meant for a renewal that is gone: {seen:?}"
        );
    }
    let frames = client.drain_for(Duration::from_millis(300)).await;
    assert!(
        frames.iter().all(|(_, f)| f.as_close().is_none()),
        "a failed renewal ends nothing: {frames:?}"
    );
    drop(second);
}

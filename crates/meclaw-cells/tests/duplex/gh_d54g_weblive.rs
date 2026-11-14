//! d54g weblive -- the page speaks through the live model the way the phone
//! does, and the key is the one door to the microphone.
//!
//! Measured on a lab colony with the display page in a real browser against
//! the live model (2026-11-13): every hold reached the session and got an
//! answer, the page sent no frame outside a hold, and a hold into the model's
//! speech did not stop it -- the key only opened the model's ear. The live
//! model has no event that commits the caller's audio or cancels its speech
//! (its primary socket knows `mute`/`unmute` and nothing between): the end of
//! an utterance is the mute, and stopping the model is the playback on this
//! side of the provider.
//!
//! The locks, all on the page's own door (`Live::link`, the in-process link a
//! display socket topic opens):
//!
//! - **The key is the door.** In `hold` the connection drops a binary frame
//!   that arrives outside a hold, also while the model speaks; it neither
//!   forwards it nor makes silence of it.
//! - **A release ends the utterance and keeps the last word.** The mute goes to
//!   the model only behind the last frame the caller sent before the release:
//!   the model's socket reads its control before its audio, so a mute sent
//!   while frames still queue would cut the end of the sentence off.
//! - **A hold while the model speaks stops it.** The open spoken section ends
//!   as `cancelled`, and the model's audio leaves the connection as silence of
//!   the same length until the key comes up again.
//!
//! Topology tests: markers and orders, no time windows. The one number with a
//! time in it, `spoken_quiet_ms`, is set far beyond any wait of these tests,
//! so "the model is speaking" never depends on how fast the machine is.

// harness_fold: #[path = "support/duplex_cell.rs"]
#[allow(unused_imports)]
use crate::duplex_cell; // harness_fold: mod duplex_cell;

use duplex_cell::{
    DEAD_LETTER_INBOX, FRAME_BYTES, Seen, TICK_MS, advise_msg, boot_fake, boot_fake_clocked,
    boot_fake_timed, boot_page, duplex_params, header, speak_msg,
};
use meclaw_cells::voice::contract::{DuplexControl, DuplexEvent, Speaker};
use meclaw_colony::mutation::subtree::{CellNode, check_override_params, parse_subtree};
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry, scan_templates_dir};
use meclaw_colony::{ColonyMsg, DeadLetterReason};
use meclaw_core::Body;
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::deadline;

/// The name a page session carries where a call carries its `call_id`: the
/// connection's name and the session's number, counting from 1 (OR-54g-21 e).
fn page_session(connection: &str, n: u32) -> String {
    format!("{connection}-{n}")
}

/// The params of a duplex cell whose connections start in `hold`.
fn hold_params() -> Value {
    duplex_params(json!({"default_mode": "hold"}))
}

/// One 20 ms frame of the caller, every byte `n`: frames tell themselves apart
/// by their content.
fn caller_frame(n: u8) -> Vec<u8> {
    vec![n; FRAME_BYTES]
}

/// One 20 ms frame of the model that is audibly speaking: a square wave at
/// about -8 dBFS, far above any gate level.
fn loud() -> Vec<u8> {
    let mut out = Vec::with_capacity(FRAME_BYTES);
    for i in 0..FRAME_BYTES / 2 {
        let sample: i16 = if i % 2 == 0 { 12_000 } else { -12_000 };
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

/// A `spoken_quiet_ms` (and section cap) no wait of these tests comes near:
/// the model counts as speaking from its last audible frame on, whatever the
/// load of the machine.
const FAR_MS: u64 = 600_000;

/// The life of a page's early session in the pin of review M-3 (virtual time).
const PREWARM_MS: u64 = 2_000;

/// A hold feeds the model's ear, and the release shuts it: the mute is the
/// provider's own word for "the caller is done".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_weblive_a_hold_feeds_the_session_and_a_release_ends_the_utterance() {
    let mut live = boot_fake(hold_params()).await;
    let (client, hello) = live.link("weblive-a", "hold").await;
    assert_eq!(hello["mode"], json!("hold"), "got {hello}");

    client.hold().await;
    let _session = live.session().await;
    assert_eq!(
        live.control().await,
        DuplexControl::Unmute,
        "the first hold opens the page's session with the model's ear open"
    );
    for n in 1..=5u8 {
        client.send_audio(caller_frame(n)).await;
    }
    for n in 1..=5u8 {
        assert_eq!(
            live.audio.recv().await.expect("the provider runs"),
            caller_frame(n),
            "what is said while the key is down reaches the model, in order"
        );
    }

    client.release().await;
    assert_eq!(
        live.control().await,
        DuplexControl::Mute,
        "the release is the end of the utterance, on the provider's own means"
    );
}

/// Outside a hold no frame of the microphone reaches the model -- before the
/// first hold, between two holds, and while the model speaks.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_weblive_outside_a_hold_no_microphone_audio_reaches_the_provider() {
    let mut live = boot_fake(hold_params()).await;
    let (mut client, _hello) = live.link("weblive-b", "hold").await;

    // Before any hold: there is no session yet, and the frame goes nowhere.
    client.send_audio(caller_frame(1)).await;
    client.hold().await;
    let session = live.session().await;
    assert_eq!(live.control().await, DuplexControl::Unmute);
    client.send_audio(caller_frame(3)).await;
    assert_eq!(
        live.audio.recv().await.expect("the provider runs"),
        caller_frame(3),
        "the first frame the model hears is the first one inside the hold: the \
         one sent before it went nowhere (one queue, one order)"
    );
    client.release().await;
    assert_eq!(live.control().await, DuplexControl::Mute);

    // While the model speaks, without a hold.
    session
        .audio_out
        .send(loud())
        .await
        .expect("the connection reads the model");
    assert_eq!(
        client.next_audio().await,
        loud(),
        "the page hears the model"
    );
    client.send_audio(caller_frame(2)).await;
    client.hold().await;
    assert_eq!(live.control().await, DuplexControl::Unmute);
    client.send_audio(caller_frame(6)).await;
    assert_eq!(
        live.audio.recv().await.expect("the provider runs"),
        caller_frame(6),
        "the frame sent while the model spoke, outside a hold, went nowhere"
    );
    client.release().await;
    assert_eq!(live.control().await, DuplexControl::Mute);
    // Between two holds.
    client.send_audio(caller_frame(4)).await;
    client.hold().await;
    assert_eq!(live.control().await, DuplexControl::Unmute);
    client.send_audio(caller_frame(5)).await;
    assert_eq!(
        live.audio.recv().await.expect("the provider runs"),
        caller_frame(5),
        "the frame sent between the two holds went nowhere"
    );
}

/// A hold into the model's speech stops it: the open section ends as
/// `cancelled`, and what the model goes on saying leaves as silence until the
/// key comes up. Then the model is heard again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_weblive_a_hold_while_the_model_speaks_stops_it() {
    let mut live = boot_fake_timed(hold_params(), FAR_MS, FAR_MS).await;
    let (mut client, _hello) = live.link("weblive-c", "hold").await;
    // The first hold opens the page's session; the release ends the
    // utterance.
    client.hold().await;
    let session = live.session().await;
    assert_eq!(live.control().await, DuplexControl::Unmute);
    client.release().await;
    assert_eq!(live.control().await, DuplexControl::Mute);

    // A spoken section of the handler's, and the model speaking it -- for
    // the page's session, the call the colony answers.
    live.send(speak_msg(
        &page_session("weblive-c", 1),
        "The appointment is on Thursday.",
    ))
    .await;
    match live.control().await {
        DuplexControl::Append { .. } => {}
        other => panic!("expected the append of the section, got {other:?}"),
    }
    let start = client.frame_of_type("speak_start").await;
    session.audio_out.send(loud()).await.expect("model audio");
    assert_eq!(client.next_audio().await, loud(), "the model is heard");

    client.hold().await;
    assert_eq!(live.control().await, DuplexControl::Unmute);
    session.audio_out.send(loud()).await.expect("model audio");
    let hushed = client.next_audio().await;
    assert_eq!(
        hushed.len(),
        FRAME_BYTES,
        "silence of the same length, not a gap"
    );
    assert!(
        hushed.iter().all(|b| *b == 0),
        "the model's speech after the hold leaves as silence"
    );
    let end = live.emission("speak_end").await;
    assert_eq!(
        header(&end, "reason"),
        Some(&json!("cancelled")),
        "the open section ends as cancelled: {end}"
    );
    assert_eq!(
        header(&end, "speak_id"),
        start.get("speak_id"),
        "the same section"
    );

    client.release().await;
    assert_eq!(live.control().await, DuplexControl::Mute);
    session.audio_out.send(loud()).await.expect("model audio");
    assert_eq!(
        client.next_audio().await,
        loud(),
        "after the release the model is heard again: its answer"
    );

    // The model's own speech, no section open: the key stops it the same way.
    client.hold().await;
    assert_eq!(live.control().await, DuplexControl::Unmute);
    session.audio_out.send(loud()).await.expect("model audio");
    let hushed = client.next_audio().await;
    assert!(
        hushed.iter().all(|b| *b == 0),
        "a hold into the model's own answer stops it as well"
    );
}

/// The release keeps the last word: every frame sent before the release
/// reaches the model before the mute does.
///
/// The fake's own queue is one item long and the test does not read it, so
/// the provider is parked on the second frame and the rest wait in the
/// connection's queue -- the moment of a model behind the caller. A bad frame
/// after the release is the marker that the connection has read the release
/// (its answer is an `error` frame); only then the test lets the provider
/// read on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_weblive_a_release_keeps_the_last_word() {
    let mut live = boot_fake_clocked(hold_params(), TICK_MS, 1).await;
    let (mut client, _hello) = live.link("weblive-d", "hold").await;
    client.hold().await;
    let _session = live.session().await;
    assert_eq!(live.control().await, DuplexControl::Unmute);

    const FRAMES: u8 = 10;
    for n in 1..=FRAMES {
        client.send_audio(caller_frame(n)).await;
    }
    client.release().await;
    client.send_audio(vec![0u8; 3]).await;
    let marker = client.frame_of_type("error").await;
    assert_eq!(marker["code"], json!("bad_audio_frame"), "got {marker}");

    for n in 1..=FRAMES {
        assert_eq!(
            live.audio.recv().await.expect("the provider runs"),
            caller_frame(n)
        );
    }
    assert_eq!(live.control().await, DuplexControl::Mute);

    // The provider's own order of what it was told after the hold.
    let mut told: Vec<Seen> = Vec::new();
    while let Ok((_, seen)) = live.journal.try_recv() {
        told.push(seen);
    }
    let unmute = told
        .iter()
        .position(|s| *s == Seen::Control(DuplexControl::Unmute))
        .expect("the hold reached the model");
    let after: Vec<&Seen> = told[unmute + 1..].iter().collect();
    let mute = after
        .iter()
        .position(|s| **s == Seen::Control(DuplexControl::Mute))
        .expect("the release reached the model");
    let heard_before = after[..mute]
        .iter()
        .filter(|s| matches!(s, Seen::Audio(_)))
        .count();
    assert_eq!(
        heard_before,
        usize::from(FRAMES),
        "every frame of the hold reaches the model before the mute; the order \
         the model was told: {:?}",
        after
            .iter()
            .map(|s| match s {
                Seen::Audio(b) => format!("audio({})", b.first().copied().unwrap_or(0)),
                Seen::Control(c) => format!("{c:?}"),
            })
            .collect::<Vec<_>>()
    );
}

// -- d54g weblive, contract 4, 7 and 8: a page has sessions as a phone has
// calls. The provider session opens with the first hold, not when the page
// hangs on; it ends after `hold_idle_ms` without a hold, without audible
// speech of the model and without an open delegation, and the page keeps its
// connection: the next hold opens a new session under a new name. Each is
// reported like a call (`seed_ask` at its start, `usage` with `final` at its
// end) and booked on its own.

/// The idle end of the tests below, on the paused clock. Under the hang mark
/// of the kit (30 s): a wait for the end must not run into it.
const IDLE_MS: u64 = 5_000;

/// The params of a page cell that reports its usage.
fn page_params() -> Value {
    duplex_params(json!({"default_mode": "hold", "emit_usage": true}))
}

/// The next emission on `route` whose hop says `key == want`, skipping the
/// others on the same route.
async fn emission_where(
    live: &mut duplex_cell::Live,
    route: &str,
    key: &str,
    want: &Value,
) -> Value {
    loop {
        let e = live.emission(route).await;
        if header(&e, key) == Some(want) {
            return e;
        }
    }
}

/// The index of the next session whose way in closed, under the hang mark.
async fn next_end(live: &mut duplex_cell::Live) -> usize {
    tokio::time::timeout(deadline::scaled(deadline::HANG), live.ended.recv())
        .await
        .expect("HANG: no session of the page ended")
        .expect("the provider is running")
}

/// The index of the next session that ended and the time from the call to
/// its end on the PAUSED clock (`tokio::time::pause()` right after the boot,
/// as `voice_duplex_a_speak_ends_when_the_model_goes_quiet::quiet_section`):
/// the boot binds a socket and waits for the mount, so the test cannot carry
/// `start_paused = true`; everything after it is timers and channels, and the
/// duration is virtual time, exact, never the machine's load.
async fn next_end_on_the_paused_clock(
    live: &mut duplex_cell::Live,
) -> (usize, std::time::Duration) {
    let from = tokio::time::Instant::now();
    let index = next_end(live).await;
    (index, from.elapsed())
}

/// The first hold opens the page's session -- a page that is merely open
/// opens none, so nothing greets it (contract 8) -- and the session ends once
/// the page was idle for `hold_idle_ms`: counted from the last thing that
/// happened on it, here the model's audible answer after the release.
#[tokio::test(flavor = "current_thread")]
async fn gh_d54g_weblive_a_session_opens_with_the_first_hold_and_closes_when_idle() {
    let mut live = boot_page(hold_params(), IDLE_MS, 0, 0).await;
    tokio::time::pause();
    let (mut client, _hello) = live.link("weblive-e", "hold").await;
    // A bad frame is answered by the connection: the marker that it has read
    // past its `hello`.
    client.send_audio(vec![0u8; 3]).await;
    let marker = client.frame_of_type("error").await;
    assert_eq!(marker["code"], json!("bad_audio_frame"), "got {marker}");
    assert_eq!(
        live.starts.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a page that is merely open has no session: nothing runs, nothing greets"
    );

    client.hold().await;
    let session = live.session().await;
    assert_eq!(session.index, 1);
    assert_eq!(live.control().await, DuplexControl::Unmute);
    client.send_audio(caller_frame(1)).await;
    assert_eq!(
        live.audio.recv().await.expect("the provider runs"),
        caller_frame(1)
    );
    client.release().await;
    assert_eq!(live.control().await, DuplexControl::Mute);

    // The model answers half an idle period later: its audible speech is
    // something happening on the page, and the idle period starts again.
    tokio::time::advance(std::time::Duration::from_millis(IDLE_MS / 2)).await;
    session.audio_out.send(loud()).await.expect("model audio");
    assert_eq!(
        client.next_audio().await,
        loud(),
        "the page hears the model"
    );
    let (ended, idle) = next_end_on_the_paused_clock(&mut live).await;
    assert_eq!(ended, 1, "the page's session ended");
    assert!(
        idle >= std::time::Duration::from_millis(IDLE_MS),
        "the session ends no sooner than `hold_idle_ms` after the model's last \
         audible audio: ended {} ms after it",
        idle.as_millis()
    );
    assert!(
        idle <= std::time::Duration::from_millis(IDLE_MS + TICK_MS),
        "the session ends at `hold_idle_ms`, not later: ended {} ms after it",
        idle.as_millis()
    );
    assert_eq!(
        live.starts.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the end opened nothing new"
    );
}

/// A page session is reported with the sendings of a call: `seed_ask` at its
/// start and `usage` with `final` at its end, both under the session's own
/// name in the place of `call_id`, with `engine` `duplex`.
#[tokio::test(flavor = "current_thread")]
async fn gh_d54g_weblive_a_page_session_reports_its_start_and_its_end_like_a_call() {
    let mut live = boot_page(page_params(), IDLE_MS, 0, 1_000).await;
    tokio::time::pause();
    let (client, _hello) = live.link("weblive-f", "hold").await;
    let name = json!(page_session("weblive-f", 1));

    client.hold().await;
    let _session = live.session().await;
    let seed = live.emission("seed_ask").await;
    assert_eq!(header(&seed, "call_id"), Some(&name), "{seed}");
    assert_eq!(header(&seed, "engine"), Some(&json!("duplex")), "{seed}");
    assert_eq!(live.control().await, DuplexControl::Unmute);
    client.release().await;
    assert_eq!(live.control().await, DuplexControl::Mute);

    let usage = emission_where(&mut live, "usage", "final", &json!(true)).await;
    assert_eq!(header(&usage, "call_id"), Some(&name), "{usage}");
    assert_eq!(header(&usage, "engine"), Some(&json!("duplex")), "{usage}");
    assert_eq!(next_end(&mut live).await, 1, "the provider session is over");
}

/// A hold after the end of a session opens a new one, on the same
/// connection, under the next name -- and the caller's audio reaches the new
/// session, never the one that ended.
#[tokio::test(flavor = "current_thread")]
async fn gh_d54g_weblive_a_hold_after_the_end_opens_a_new_session() {
    let mut live = boot_page(page_params(), IDLE_MS, 0, 1_000).await;
    tokio::time::pause();
    let (client, _hello) = live.link("weblive-h", "hold").await;

    client.hold().await;
    let _first = live.session().await;
    let seed = live.emission("seed_ask").await;
    assert_eq!(
        header(&seed, "call_id"),
        Some(&json!(page_session("weblive-h", 1)))
    );
    client.release().await;
    emission_where(
        &mut live,
        "usage",
        "call_id",
        &json!(page_session("weblive-h", 1)),
    )
    .await;
    assert_eq!(next_end(&mut live).await, 1);

    client.hold().await;
    let second = live.session().await;
    assert_eq!(
        second.index, 2,
        "the hold after the end opens a new session"
    );
    let seed = live.emission("seed_ask").await;
    assert_eq!(
        header(&seed, "call_id"),
        Some(&json!(page_session("weblive-h", 2))),
        "the new session has a name of its own: {seed}"
    );
    client.send_audio(caller_frame(9)).await;
    let (index, frame) =
        tokio::time::timeout(deadline::scaled(deadline::HANG), live.tagged_audio.recv())
            .await
            .expect("HANG: no audio reached a session")
            .expect("the provider is running");
    assert_eq!(
        (index, frame),
        (2, caller_frame(9)),
        "the caller's audio reaches the new session"
    );
}

/// The seconds of a page session are booked as a call's are
/// (`voice/call_usage.rs`): the provider's meter goes out on `usage` under
/// the session's name, and every session of the page is a ledger of its own.
#[tokio::test(flavor = "current_thread")]
async fn gh_d54g_weblive_the_seconds_of_a_page_session_are_booked() {
    let mut live = boot_page(page_params(), IDLE_MS, 0, 0).await;
    tokio::time::pause();
    let (client, _hello) = live.link("weblive-i", "hold").await;

    client.hold().await;
    let first = live.session().await;
    first
        .say(DuplexEvent::Usage {
            seconds: 42.0,
            usage_ratio: None,
        })
        .await;
    let meter = emission_where(
        &mut live,
        "usage",
        "call_id",
        &json!(page_session("weblive-i", 1)),
    )
    .await;
    assert_eq!(header(&meter, "seconds"), Some(&json!(42.0)), "{meter}");
    assert_eq!(header(&meter, "final"), Some(&json!(false)), "{meter}");
    client.release().await;
    let last = emission_where(&mut live, "usage", "final", &json!(true)).await;
    assert_eq!(
        header(&last, "call_id"),
        Some(&json!(page_session("weblive-i", 1))),
        "{last}"
    );
    assert_eq!(next_end(&mut live).await, 1);

    client.hold().await;
    let second = live.session().await;
    second
        .say(DuplexEvent::Usage {
            seconds: 5.0,
            usage_ratio: None,
        })
        .await;
    let meter = emission_where(
        &mut live,
        "usage",
        "call_id",
        &json!(page_session("weblive-i", 2)),
    )
    .await;
    assert_eq!(
        header(&meter, "seconds"),
        Some(&json!(5.0)),
        "the second session is booked on its own, not on top of the first: {meter}"
    );
}

/// The page says early that the key is about to go down (`ready`: the pointer
/// on the key, the microphone granted). Where `prewarm_ttl_ms` is set, the
/// connection opens the session then, the way a ringing line does, and the
/// first hold adopts it: no second session, and the hold's audio reaches the
/// early one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_weblive_an_early_word_of_the_page_warms_its_session() {
    let mut live = boot_page(hold_params(), 0, FAR_MS, 0).await;
    let (client, _hello) = live.link("weblive-j", "hold").await;

    client.ready().await;
    let early = live.session().await;
    assert_eq!(early.index, 1, "the early word opens the session");

    client.hold().await;
    assert_eq!(live.control().await, DuplexControl::Unmute);
    client.send_audio(caller_frame(7)).await;
    // While parked, the early session is fed 20 ms frames of digital silence,
    // the line a quiet caller gives (`park_pump`, GH #1055 R1, S0a), until the
    // hold adopts it. So the first frame that reaches the session can be that
    // silence; what the pin judges is the first frame of the page. Every frame
    // before it must be exactly the park's silence on the same session --
    // nothing else, and on no other session.
    let silence = vec![0u8; FRAME_BYTES];
    let mut parked_silence = 0usize;
    let (index, frame) = loop {
        let (index, frame) =
            tokio::time::timeout(deadline::scaled(deadline::HANG), live.tagged_audio.recv())
                .await
                .expect("HANG: no audio of the page reached a session")
                .expect("the provider is running");
        if frame != silence {
            break (index, frame);
        }
        assert_eq!(
            index, 1,
            "the park's silence feeds the early session and no other"
        );
        parked_silence += 1;
    };
    assert_eq!(
        (index, frame),
        (1, caller_frame(7)),
        "the hold adopts the early session: the page's first frame reaches it \
         ({parked_silence} frames of the park's silence before it)"
    );
    assert_eq!(
        live.starts.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "no second session opens for the hold"
    );
}

/// A page in `hold` with no session open that switches to `auto` gets its
/// session at once (OR-d54g.weblive.7): `auto` is an open ear by definition,
/// so the switch is acknowledged, the page's first session opens without a
/// hold, and what the microphone says reaches it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_weblive_a_switch_to_auto_opens_the_page_s_session_at_once() {
    let mut live = boot_fake(hold_params()).await;
    let (mut client, _hello) = live.link("weblive-k", "hold").await;
    assert_eq!(
        live.starts.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a page in `hold` that is merely open has no session"
    );

    client
        .send_text(json!({"type": "mode", "mode": "auto"}))
        .await;
    let ack = client.frame_of_type("mode").await;
    assert_eq!(
        ack["mode"],
        json!("auto"),
        "the switch is acknowledged: {ack}"
    );
    let session = live.session().await;
    assert_eq!(
        session.index, 1,
        "the switch to `auto` opens the page's first session, without a hold"
    );

    client.send_audio(caller_frame(3)).await;
    let (index, frame) =
        tokio::time::timeout(deadline::scaled(deadline::HANG), live.tagged_audio.recv())
            .await
            .expect("HANG: no audio of the page reached a session")
            .expect("the provider is running");
    assert_eq!(
        (index, frame),
        (1, caller_frame(3)),
        "in `auto` the microphone reaches the session the switch opened"
    );
    assert_eq!(
        live.starts.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the switch opens exactly one session"
    );
}

/// A switch of mode inside a page's session holds after it (review M-1 of
/// d54g weblive): the page switched to `auto` (its session opens at once),
/// back to `hold` inside the session (acknowledged), and the session ended
/// for being idle. The cell acknowledged `hold`, so the page's next hold
/// opens its next session -- it is not refused with `wrong_mode` for the
/// mode from before the session.
///
/// Time logic on the paused clock, as the other idle pins of this file: the
/// idle end is a tokio timer; nothing here is a time assertion.
#[tokio::test(flavor = "current_thread")]
async fn gh_d54g_weblive_a_mode_switched_in_a_session_holds_after_it() {
    let mut live = boot_page(page_params(), IDLE_MS, 0, 0).await;
    tokio::time::pause();
    let (mut client, _hello) = live.link("weblive-m", "hold").await;

    client
        .send_text(json!({"type": "mode", "mode": "auto"}))
        .await;
    assert_eq!(client.frame_of_type("mode").await["mode"], json!("auto"));
    let first = live.session().await;
    assert_eq!(
        first.index, 1,
        "the switch to `auto` opens the first session"
    );

    client
        .send_text(json!({"type": "mode", "mode": "hold"}))
        .await;
    let ack = client.frame_of_type("mode").await;
    assert_eq!(
        ack["mode"],
        json!("hold"),
        "the switch back is acknowledged: {ack}"
    );
    emission_where(
        &mut live,
        "usage",
        "call_id",
        &json!(page_session("weblive-m", 1)),
    )
    .await;
    assert_eq!(
        next_end(&mut live).await,
        1,
        "the session ended for being idle"
    );

    client.hold().await;
    let second = live.session().await;
    assert_eq!(
        second.index, 2,
        "the next hold opens the next session in the mode the cell acknowledged"
    );
    drop(first);
}

/// An append for a page with no session open is not dropped in silence
/// (review M-2 of d54g weblive): there is no model to take it, and the handler
/// says so the way it says the model never got an append --
/// `advise_append_refused` on the error lane, under the name it was sent to.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_weblive_an_append_for_a_page_without_a_session_is_refused() {
    let mut live = boot_fake(hold_params()).await;
    let (_client, _hello) = live.link("weblive-q", "hold").await;
    live.send(advise_msg("weblive-q", "context", "A note.", None))
        .await;
    let refused = emission_where(
        &mut live,
        "error",
        "error_code",
        &json!("advise_append_refused"),
    )
    .await;
    assert_eq!(
        header(&refused, "call_id"),
        Some(&json!("weblive-q")),
        "{refused}"
    );
    assert_eq!(
        live.starts.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the append opens no session"
    );
}

/// An early session of a page that ran out spends its name (review M-3 of
/// d54g weblive): it was reported like a ring nobody answered -- `seed_ask`
/// under `<name>-1`, no `usage` -- so the hold after it opens its session
/// under the next number, with a `seed_ask` of its own, never a second one
/// under the same name.
///
/// Time logic on the paused clock: the early session's end is the tokio
/// timer of `prewarm_ttl_ms`; nothing here is a time assertion.
#[tokio::test(flavor = "current_thread")]
async fn gh_d54g_weblive_a_page_counts_past_an_early_session_that_ran_out() {
    let mut live = boot_page(page_params(), 0, PREWARM_MS, 1_000).await;
    tokio::time::pause();
    let (client, _hello) = live.link("weblive-p", "hold").await;

    client.ready().await;
    let early = live.session().await;
    assert_eq!(early.index, 1, "the early word opens a session");
    let seed = live.emission("seed_ask").await;
    assert_eq!(
        header(&seed, "call_id"),
        Some(&json!(page_session("weblive-p", 1)))
    );
    assert_eq!(
        next_end(&mut live).await,
        1,
        "the early session nobody held ran out"
    );

    client.hold().await;
    let second = live.session().await;
    assert_eq!(second.index, 2, "the hold opens a session of its own");
    let seed = live.emission("seed_ask").await;
    assert_eq!(
        header(&seed, "call_id"),
        Some(&json!(page_session("weblive-p", 2))),
        "the session after an early one that ran out takes the next number: {seed}"
    );
    drop(early);
}

/// A call ends when the cell gives up delivering to it (review I-1 of d54g
/// weblive): the client stopped reading, the commands for its session ran
/// over (`client_too_slow`), and the session was taken out of the registry.
/// The connection then holds no way in for its commands any more, so it ends
/// its session as it always did -- the provider session closes and the
/// `usage` with `final` goes out -- instead of running on with the
/// provider's clock under a session the handler already let go of.
///
/// Topology: markers and orders, no time window. The client parks the
/// connection by not reading its link while the model talks; the count, not
/// a clock, gives the delivery up (`external_timeout_ms` is set far out of
/// reach); then the client reads again -- still on the line, its own half of
/// the link open, so a hang-up cannot be what ends the call -- and the
/// connection must end.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_weblive_a_call_ends_when_its_delivery_gives_up() {
    let mut live = boot_fake(duplex_params(json!({
        "emit_usage": true,
        "external_timeout_ms": FAR_MS,
    })))
    .await;
    let tag = "weblive-gone";
    let (client, hello) = live.link(tag, "auto").await;
    assert_eq!(hello["mode"], json!("auto"), "a call: {hello}");
    let session = live.session().await;

    // The model talks and the client does not read: the link fills and the
    // connection parks on it.
    let audio_out = session.audio_out.clone();
    let talk = tokio::spawn(async move {
        for _ in 0..100_000u32 {
            if audio_out.send(loud()).await.is_err() {
                break;
            }
        }
    });
    // Commands for the call pile up behind the parked connection until the
    // delivery gives up by count.
    let mailbox = live.mailbox.clone();
    let flood_tag = tag.to_string();
    let flood = tokio::spawn(async move {
        for n in 0..100_000u32 {
            let advise = advise_msg(&flood_tag, "context", &format!("Note {n}."), None);
            if mailbox.send(advise).await.is_err() {
                break;
            }
        }
    });
    let given_up =
        emission_where(&mut live, "error", "error_code", &json!("client_too_slow")).await;
    assert_eq!(
        header(&given_up, "call_id"),
        Some(&json!(tag)),
        "{given_up}"
    );
    flood.abort();
    talk.abort();

    // The client reads again and stays on the line: the connection runs on
    // and must end the call by itself.
    let read = client.drain();
    let usage = emission_where(&mut live, "usage", "final", &json!(true)).await;
    assert_eq!(header(&usage, "call_id"), Some(&json!(tag)), "{usage}");
    tokio::time::timeout(deadline::scaled(deadline::HANG), read)
        .await
        .expect("HANG: the connection held the link after its delivery gave up")
        .expect("the reader ran");
    drop(session);
}

/// A call whose delivery gave up hands every late command to the colony as a
/// `session_ended` dead letter and goes on answering -- also when there are
/// more of them than the colony inbox holds at once (d54g weblive, fix
/// round 5). Measured: the pin above hung 1 in 100 runs under load with the
/// handler parked in ONE `handle()`; the late commands of the flood went into
/// a dead-letter inbox of 64 the fixture never read, and the 65th send waited
/// for ever. The colony reads its inbox for as long as it runs, so the fixture
/// reads it as the colony does (`duplex_cell::drain_dead_letters`).
///
/// Topology: markers and orders, no time window. The call ends as in the pin
/// above; the client's reader finishing is the fence: the connection drops
/// its half of the link only after it reported the session gone, and the
/// handler takes events before its mailbox, so every command sent after that
/// is late. Twice the inbox plus one late commands, then one advise for a
/// session nobody holds: its refusal is the handler still answering, and
/// every late command is in the dead letters by its own text.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_weblive_a_call_given_up_dead_letters_every_late_command() {
    let mut live = boot_fake(duplex_params(json!({
        "emit_usage": true,
        "external_timeout_ms": FAR_MS,
    })))
    .await;
    let tag = "weblive-late";
    let (client, _hello) = live.link(tag, "auto").await;
    let session = live.session().await;

    let audio_out = session.audio_out.clone();
    let talk = tokio::spawn(async move {
        for _ in 0..100_000u32 {
            if audio_out.send(loud()).await.is_err() {
                break;
            }
        }
    });
    let mailbox = live.mailbox.clone();
    let flood_tag = tag.to_string();
    let flood = tokio::spawn(async move {
        for n in 0..100_000u32 {
            let advise = advise_msg(&flood_tag, "context", &format!("Note {n}."), None);
            if mailbox.send(advise).await.is_err() {
                break;
            }
        }
    });
    emission_where(&mut live, "error", "error_code", &json!("client_too_slow")).await;
    flood.abort();
    talk.abort();
    let read = client.drain();
    let usage = emission_where(&mut live, "usage", "final", &json!(true)).await;
    assert_eq!(header(&usage, "call_id"), Some(&json!(tag)), "{usage}");
    tokio::time::timeout(deadline::scaled(deadline::HANG), read)
        .await
        .expect("HANG: the connection held the link after its delivery gave up")
        .expect("the reader ran");

    let late = 2 * DEAD_LETTER_INBOX + 1;
    let nobody = "weblive-nobody";
    deadline::hang(
        "the handler takes every late command and answers the next one",
        async {
            for n in 0..late {
                live.send(advise_msg(tag, "context", &format!("Late {n}."), None))
                    .await;
            }
            live.send(advise_msg(nobody, "context", "Anyone?", None))
                .await;
        },
    )
    .await;
    let refused = emission_where(&mut live, "error", "call_id", &json!(nobody)).await;
    assert_eq!(
        header(&refused, "error_code"),
        Some(&json!("unknown_session")),
        "{refused}"
    );

    let mut missing: std::collections::BTreeSet<String> =
        (0..late).map(|n| format!("Late {n}.")).collect();
    deadline::hang("every late command is a session_ended dead letter", async {
        while !missing.is_empty() {
            match live.dead_letters.recv().await {
                Some(ColonyMsg::DeadLetterMessage { message, reason }) => {
                    assert_eq!(reason, DeadLetterReason::SessionEnded, "{message:?}");
                    let text = match &message.body {
                        Body::Inline(body) => duplex_cell::message_text(body, 0),
                        other => panic!("a late advise is inline: {other:?}"),
                    };
                    missing.remove(&text);
                }
                Some(_) => {}
                None => panic!("the dead-letter inbox closed"),
            }
        }
    })
    .await;
    drop(session);
}

/// The appends a call hands its model before the flood of the pins above:
/// one more than the fake's control outlet held before d54g weblive fix
/// round 6 (a bounded channel of 256 that these pins never read).
const PAST_THE_OUTLET: usize = 257;

/// A call whose delivery gave up ends also after it handed its model more
/// appends than any test reads (GH #1511). Measured: with the
/// dead letters read, the set of these pins still hung 1-4 in 40
/// runs under load, the handler free and the connection silent after the
/// client read again; the fake model had passed every control on into an
/// outlet of 256 nobody read, parked on the 257th, stopped reading its
/// control channel, and the connection waited in its next `control_tx.send`
/// for ever. A real provider writes its controls to its socket under the
/// operation timeout (`gpt_live::send_frame`) and ends the session when that
/// fails, so the fake passes them on without a bound.
///
/// Topology: markers and orders, no time window. First the call hands its
/// model `PAST_THE_OUTLET` appends, one at a time, each read off the fake's
/// unbounded tap before the next goes out (the model reads them all; nobody
/// reads the outlet). Then the delivery gives up as in the pins above and the
/// client reads again: the commands still queued for the connection are
/// appends, and the connection must take them and end the call.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_weblive_a_call_given_up_ends_past_the_models_control_outlet() {
    let mut live = boot_fake(duplex_params(json!({
        "emit_usage": true,
        "external_timeout_ms": FAR_MS,
    })))
    .await;
    let tag = "weblive-outlet";
    let (client, _hello) = live.link(tag, "auto").await;
    let session = live.session().await;

    for n in 0..PAST_THE_OUTLET {
        let text = format!("Before {n}.");
        live.send(advise_msg(tag, "context", &text, None)).await;
        deadline::hang("the model is handed the append of the advise", async {
            loop {
                match live.tagged_controls.recv().await {
                    Some((_, DuplexControl::Append { content, .. })) if content.contains(&text) => {
                        break;
                    }
                    Some(_) => {}
                    None => panic!("the fake is gone"),
                }
            }
        })
        .await;
    }

    let audio_out = session.audio_out.clone();
    let talk = tokio::spawn(async move {
        for _ in 0..100_000u32 {
            if audio_out.send(loud()).await.is_err() {
                break;
            }
        }
    });
    let mailbox = live.mailbox.clone();
    let flood_tag = tag.to_string();
    let flood = tokio::spawn(async move {
        for n in 0..100_000u32 {
            let advise = advise_msg(&flood_tag, "context", &format!("Note {n}."), None);
            if mailbox.send(advise).await.is_err() {
                break;
            }
        }
    });
    emission_where(&mut live, "error", "error_code", &json!("client_too_slow")).await;
    flood.abort();
    talk.abort();
    let read = client.drain();
    let usage = emission_where(&mut live, "usage", "final", &json!(true)).await;
    assert_eq!(header(&usage, "call_id"), Some(&json!(tag)), "{usage}");
    tokio::time::timeout(deadline::scaled(deadline::HANG), read)
        .await
        .expect("HANG: the connection held the link after its delivery gave up")
        .expect("the reader ran");
    drop(session);
}

/// A page's session ends when the cell gives up delivering to it
/// (OR-d54g.weblive.20): the page's client stopped reading, the
/// commands for its session ran over (`client_too_slow` under the session's
/// name), and the session was taken out of the registry. The page's own name
/// stays registered, and it must not keep that session's way in open: the
/// provider session closes and the `usage` with `final` goes out under the
/// session's name -- as a call ends, not only after `hold_idle_ms` (`0` here,
/// never) or a hang-up.
///
/// Topology: markers and orders, no time window. As the call's pin above: the
/// count, not a clock, gives the delivery up (`external_timeout_ms` far out of
/// reach), and the client reads again with its own half of the link open.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_weblive_a_page_session_ends_when_its_delivery_gives_up() {
    let mut live = boot_fake(duplex_params(json!({
        "default_mode": "hold",
        "emit_usage": true,
        "external_timeout_ms": FAR_MS,
    })))
    .await;
    let tag = "weblive-page-gone";
    let (client, hello) = live.link(tag, "hold").await;
    assert_eq!(hello["mode"], json!("hold"), "a page: {hello}");
    client.hold().await;
    let session = live.session().await;
    assert_eq!(session.index, 1, "the hold opens the page's first session");
    let name = page_session(tag, 1);

    // The model talks and the client does not read: the link fills and the
    // connection parks on it.
    let audio_out = session.audio_out.clone();
    let talk = tokio::spawn(async move {
        for _ in 0..100_000u32 {
            if audio_out.send(loud()).await.is_err() {
                break;
            }
        }
    });
    // Commands for the page's session pile up behind the parked connection
    // until the delivery gives up by count.
    let mailbox = live.mailbox.clone();
    let flood_name = name.clone();
    let flood = tokio::spawn(async move {
        for n in 0..100_000u32 {
            let advise = advise_msg(&flood_name, "context", &format!("Note {n}."), None);
            if mailbox.send(advise).await.is_err() {
                break;
            }
        }
    });
    let given_up =
        emission_where(&mut live, "error", "error_code", &json!("client_too_slow")).await;
    assert_eq!(
        header(&given_up, "call_id"),
        Some(&json!(name)),
        "{given_up}"
    );
    flood.abort();
    talk.abort();

    // The client reads again and stays on the line: the connection must end
    // the session by itself.
    let _read = client.drain();
    let usage = emission_where(&mut live, "usage", "final", &json!(true)).await;
    assert_eq!(header(&usage, "call_id"), Some(&json!(name)), "{usage}");
    assert_eq!(
        next_end(&mut live).await,
        1,
        "the provider session of the page closed"
    );
    drop(session);
}

/// The root cell of the shipped `voice` template, or `None` where the
/// template did not travel (R2b / GH #49).
fn shipped_voice_cell() -> Option<CellNode> {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let voice = root.join("templates/voice");
    if !voice.join("config.json").is_file() {
        return None;
    }
    let scanned = scan_templates_dir(&root.join("templates")).unwrap_or_default();
    let registry = TemplatesRegistry::from_entries(
        scanned
            .into_iter()
            .map(|s| TemplateEntry {
                template_id: format!("scan-{}", s.name),
                name: s.name,
                version: s.version,
                filesystem_path: s.filesystem_path,
            })
            .collect(),
    );
    let parsed = parse_subtree(&voice, &registry)
        .unwrap_or_else(|e| panic!("the shipped template 'voice' parses: {e:?}"));
    parsed.cells.into_iter().find(|c| c.rel_path.is_empty())
}

/// What one origin said in a `turn` emission, all of its turns joined.
fn said_by(emission: &Value, origin: &str) -> String {
    emission
        .get("messages")
        .and_then(Value::as_array)
        .map(|all| {
            all.iter()
                .filter(|m| m.get("origin").and_then(Value::as_str) == Some(origin))
                .filter_map(|m| m.get("text").and_then(Value::as_str))
                .collect::<String>()
        })
        .unwrap_or_default()
}

/// Contract 5: both sides of a page's conversation leave the cell as a
/// call's do. The person holds and speaks, lets go, and the model answers:
/// ONE `turn` carries the person's words as `user` and the model's as
/// `assistant`, under the page session's name in the place of `call_id`,
/// numbered within that session, with `engine` `duplex` -- the shape a phone
/// call's turn has (`gh_d54g_callseam`). The page itself hears what the model
/// says as it says it (`spoken`) and the turn when it closes; it needs no
/// branch of its own for either, the chat application draws the line.
#[tokio::test(flavor = "current_thread")]
async fn gh_d54g_weblive_both_sides_turns_leave_as_on_the_phone() {
    const ASKED: &str = "What is on today?";
    const ANSWER: &str = "A walk at three.";
    let mut live = boot_page(page_params(), IDLE_MS, 0, 0).await;
    tokio::time::pause();
    let (mut client, _hello) = live.link("weblive-j", "hold").await;
    let name = page_session("weblive-j", 1);

    client.hold().await;
    let session = live.session().await;
    assert_eq!(live.control().await, DuplexControl::Unmute);
    session
        .say(DuplexEvent::Transcript {
            speaker: Speaker::User,
            delta: ASKED.to_string(),
            start_ms: 0,
            end_ms: 900,
        })
        .await;
    client.release().await;
    assert_eq!(live.control().await, DuplexControl::Mute);
    session
        .say(DuplexEvent::Transcript {
            speaker: Speaker::Assistant,
            delta: ANSWER.to_string(),
            start_ms: 1_000,
            end_ms: 2_400,
        })
        .await;

    let turn = emission_where(&mut live, "turn", "call_id", &json!(name)).await;
    assert_eq!(said_by(&turn, "user"), ASKED, "the person's side: {turn}");
    assert_eq!(
        said_by(&turn, "assistant"),
        ANSWER,
        "the model's side: {turn}"
    );
    assert_eq!(header(&turn, "engine"), Some(&json!("duplex")), "{turn}");
    assert_eq!(
        header(&turn, "turn_id"),
        Some(&json!(format!("{name}#0"))),
        "the first turn of the page session: {turn}"
    );

    let spoken = client.frame_of_type("spoken").await;
    assert_eq!(
        spoken["text"],
        json!(ANSWER),
        "the page hears the model's words: {spoken}"
    );
    let closed = client.frame_of_type("turn").await;
    assert_eq!(
        closed["turn_id"],
        json!(format!("{name}#0")),
        "the page hears the turn close: {closed}"
    );
}

/// A set can switch the new param (OR-54g-21 h): `hold_idle_ms` in the
/// `duplex` block of an instance's override passes the door on the shipped
/// template, and the parse reads it. Its shipped value is `0`, which never
/// ends a session before its connection.
#[test]
fn gh_d54g_weblive_a_set_override_of_the_new_params_passes_the_door() {
    let block = |extra: Value| {
        let mut b = json!({
            "provider": "gpt_live",
            "api_key": "test-key",
            "instructions": "A test line."
        });
        if let (Some(obj), Some(more)) = (b.as_object_mut(), extra.as_object()) {
            for (k, v) in more {
                obj.insert(k.clone(), v.clone());
            }
        }
        b
    };
    let over = json!({"default_mode": "hold", "duplex": block(json!({"hold_idle_ms": 60_000}))});
    if let Some(cell) = shipped_voice_cell()
        && let Err(why) = check_override_params(&cell, None, "voice", &over)
    {
        panic!("a set's `duplex.hold_idle_ms` on 'voice' must pass the door: {why:?}");
    }
    let idle_of = |raw: Value| {
        let parsed = meclaw_cells::voice::params::VoiceParams::parse(&duplex_params(raw))
            .expect("the params parse");
        match parsed.duplex {
            Some(meclaw_cells::voice::params::DuplexParams::GptLive(g)) => g.hold_idle_ms,
            other => panic!("a gpt_live block, got {other:?}"),
        }
    };
    assert_eq!(idle_of(over), 60_000, "the parse reads the override");
    assert_eq!(
        idle_of(json!({"duplex": block(json!({}))})),
        0,
        "shipped: a session lives as long as its connection"
    );
}

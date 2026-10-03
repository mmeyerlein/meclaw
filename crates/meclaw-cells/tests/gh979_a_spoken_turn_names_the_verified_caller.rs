//! GH #979 (OR-NL.I.2, option A) — **the words of a call name the caller the
//! switch verified, and nobody else.**
//!
//! Measured before a line was written (report I, `OR-NL.I.2`): the `voice`
//! cell knows a socket and a session id — the call's UUID — and never a
//! person; `voice/turns.rs`, `connection.rs` and `params.rs` carried no sender
//! at all, so every spoken turn of a telephone call reached the member's
//! ingress without `hop.user_id`, and the ingress named nobody. The caller the
//! switch verified was known one cell over, in the signalling half.
//!
//! The road built: the signalling half hands that caller to the media half on
//! the internal `in_session` lane, and the media half stamps it as
//! `hop.user_id` (the sender) and `hop.verified_user` (the proof the ingress
//! reads, OR-NL-164) on every `turn` and `delegation` of that session. The hive's
//! half of it is locked in `gh620_a_second_call_is_arbitrated.rs` (§ GH #979:
//! a verified call is named to the media half, a refused one and a call only
//! the `callers` table knows are not). This file locks the cell's half on a
//! REAL `voice` cell, and then reads the cell's own turn at the receiver that
//! decides: the member, behind the documented telephone ingress and the
//! shipped firewall (`support/speaker_road.rs`).
//!
//! Every ordering is an event, never a clock: the cell answers `in_session`
//! with nothing, so a nameless `in_session` behind it — refused by name,
//! `missing_session` — is the sentinel that the one in front was taken.

#[path = "support/duplex_cell.rs"]
mod duplex_cell;
#[path = "support/speaker_road.rs"]
mod speaker_road;

use duplex_cell::{FakeSession, Live, boot_fake, duplex_params, header};
use meclaw_cells::voice::contract::{DuplexEvent, Speaker};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use speaker_road::{LINE_USER, Road, SPEAKER, ctx, names_nobody, next, shipped};

/// The caller the switch put through, as the signalling half hands it on.
const CALLER: &str = LINE_USER;

/// One `in_session`, as the signalling half writes it: the session on the
/// hop, the sender beside it. `user` `None` leaves the key out.
fn in_session(session: Option<&str>, user: Option<&str>) -> Message {
    let mut hop = Map::new();
    hop.insert("route".into(), json!("in_session"));
    if let Some(s) = session {
        hop.insert("session_id".into(), json!(s));
        hop.insert("call_id".into(), json!(s));
    }
    if let Some(u) = user {
        hop.insert("user_id".into(), json!(u));
    }
    MessageBuilder::new(Path::new("/voice"))
        .hop(hop)
        .body(Body::Inline(json!({"messages": []})))
        .build()
}

/// Hand the cell a lane and wait until it has been taken: the nameless
/// `in_session` behind it is refused by name, and the mailbox is a queue.
async fn hand(live: &mut Live, msg: Message) {
    live.send(msg).await;
    live.send(in_session(None, Some("nobody-in-particular")))
        .await;
    let refusal = live.emission("error").await;
    assert_eq!(
        header(&refusal, "error_code").and_then(Value::as_str),
        Some("missing_session"),
        "the sentinel is refused by name: {refusal}"
    );
}

/// One finished turn of the call `call`: two fragments a full gap apart on
/// the model's clock, the second closing the first. A turn of ANOTHER call --
/// a session clock may close an open turn on a silent line -- is skipped.
async fn a_turn(live: &mut Live, call: &str, session: &FakeSession, words: &str, at: u64) -> Value {
    for (start, delta) in [
        (at, words.to_string()),
        (at + 5_000, "and then".to_string()),
    ] {
        session
            .say(DuplexEvent::Transcript {
                speaker: Speaker::User,
                delta,
                start_ms: start,
                end_ms: start + 500,
            })
            .await;
    }
    loop {
        let turn = live.emission("turn").await;
        if header(&turn, "call_id").and_then(Value::as_str) == Some(call) {
            return turn;
        }
    }
}

/// The proof beside the sender: the key the member's ingress reads.
fn proof(emission: &Value) -> Option<String> {
    header(emission, "verified_user")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn sender(emission: &Value) -> Option<String> {
    header(emission, "user_id")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The cell's half: a named session stamps its caller on every turn and on a
/// delegation; a session nobody named, in the same cell at the same time,
/// carries no `user_id` at all; the name may arrive before the connection; an
/// empty name withdraws it.
///
/// Red before GH #979: no `turn` of a `voice` cell ever carried `user_id`, and
/// the cell answered `in_session` like an `in_speak` with no assistant text.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_named_session_carries_its_caller_and_an_unnamed_one_carries_nobody() {
    let mut live = boot_fake(duplex_params(json!({"default_mode": "auto"}))).await;

    // The signalling half hears the call from the switch while the switch is
    // still opening the audio fork: the name can arrive FIRST.
    hand(&mut live, in_session(Some("call-a"), Some(CALLER))).await;
    let (_a, _hello) = live.connect("session=call-a&mode=auto").await;
    let session_a = live.session().await;
    let (_b, _hello) = live.connect("session=call-b&mode=auto").await;
    let session_b = live.session().await;

    let turn = a_turn(&mut live, "call-a", &session_a, "it's me", 0).await;
    assert_eq!(
        sender(&turn).as_deref(),
        Some(CALLER),
        "the named session's turn carries its caller: {turn}"
    );
    assert_eq!(
        proof(&turn).as_deref(),
        Some(CALLER),
        "and the proof beside it, which is what the ingress reads: {turn}"
    );

    let turn = a_turn(&mut live, "call-b", &session_b, "who am I", 0).await;
    assert_eq!(
        sender(&turn),
        None,
        "a session nobody named carries no sender -- the key is absent: {turn}"
    );
    assert_eq!(proof(&turn), None, "and no proof: {turn}");

    // A delegation carries the caller's own words as well.
    session_a
        .say(DuplexEvent::DelegationCreated {
            delegation_id: "dlg-1".to_string(),
            offset_ms: 0,
        })
        .await;
    let delegation = live.emission("delegation").await;
    assert_eq!(sender(&delegation).as_deref(), Some(CALLER), "{delegation}");

    // An empty name withdraws it: fail-closed.
    hand(&mut live, in_session(Some("call-a"), Some(""))).await;
    let turn = a_turn(&mut live, "call-a", &session_a, "still me", 20_000).await;
    assert_eq!(sender(&turn), None, "a withdrawn name is gone: {turn}");
    assert_eq!(proof(&turn), None, "with its proof: {turn}");
}

/// The receiver: the cell's OWN turn, put through the documented telephone
/// ingress and the shipped firewall, arrives at the member named -- and the
/// turn of a session nobody named arrives naming nobody.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_member_hears_the_verified_caller_by_name() {
    if !shipped() {
        return;
    }
    let mut live = boot_fake(duplex_params(json!({"default_mode": "auto"}))).await;
    hand(&mut live, in_session(Some("call-a"), Some(CALLER))).await;
    let (_a, _hello) = live.connect("session=call-a&mode=auto").await;
    let session_a = live.session().await;
    let (_b, _hello) = live.connect("session=call-b&mode=auto").await;
    let session_b = live.session().await;
    let named = a_turn(&mut live, "call-a", &session_a, "it's me", 0).await;
    let unnamed = a_turn(&mut live, "call-b", &session_b, "it's me too", 0).await;

    let mut road = Road::boot().await;
    for (turn, named_by) in [(&named, Some(SPEAKER)), (&unnamed, None)] {
        let trace = road
            .say(
                "phone",
                None,
                sender(turn).map(Value::from),
                proof(turn).map(|p| json!({"verified_user": p})),
                "a spoken turn",
            )
            .await;
        let got = next(&mut road.turns, "the spoken turn").await;
        assert_eq!(got.trace_id, trace, "another turn arrived first");
        match named_by {
            Some(speaker) => assert_eq!(
                ctx(&got, "speaker").as_deref(),
                Some(speaker),
                "the verified caller's words name the member: {:?}",
                got.headers.context
            ),
            None => assert!(
                names_nobody(&got),
                "the words of a session nobody named name nobody: {:?}",
                got.headers.context
            ),
        }
    }
    road.shutdown().await;
}

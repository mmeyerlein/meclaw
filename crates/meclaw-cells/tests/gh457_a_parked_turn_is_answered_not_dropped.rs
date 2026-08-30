//! GH #457 — the turn that asks for the credential is the turn that gets it.
//!
//! WHAT THIS FILE IS
//! =================
//! An `llm` cell that spends a grant holds its bearer credential in RAM and
//! nowhere else, so every wake starts with an empty pocket. Under GH #421 the
//! first inference after every wake was spent on a `credential_pending` refusal
//! that nobody redelivered: a chat user's first message after a sleep was gone,
//! and what they saw was silence.
//!
//! The fix is to PARK rather than drop. The four claims, one test each:
//!
//! | claim | test |
//! |---|---|
//! | the triggering turn is answered, not refused | [`a_the_first_turn_after_a_spawn_is_answered_once_the_box_arrives`] |
//! | a vault that never answers costs receipts, not messages | [`b_a_vault_that_never_delivers_gives_every_parked_turn_its_receipt`] |
//! | the bound is a bound, and the overflow is loud | [`c_the_overflow_turn_is_refused_at_once_and_the_bound_is_still_served`] |
//! | the batch is answered in arrival order | [`d_the_parked_turns_reach_the_provider_in_the_order_they_arrived`] |
//!
//! WHY THE CELL IS DRIVEN DIRECTLY
//! ==============================
//! The vault half of this round is already pinned end to end by
//! `gh421_no_plaintext_on_the_wire` and `gh452_the_vault_pilot_grows_a_granted_credential`
//! — a real broker, a real sealed box, a real provider. What is NOT reachable
//! through a colony is the *timing*: whether the box arrives before or after the
//! deadline, and how many turns are in flight when it does, are exactly the
//! variables this issue is about, and a topology decides them for you.
//!
//! So these tests hold the two ends of the round themselves. They read the
//! recipient key out of the cell's own `credential_request` emission — the same
//! public half the broker reads — and seal a fixture to it. Everything else is
//! the shipped cell: `LlmCell::handle`, the real `OutputSink`, a real `cell.db`,
//! and a real HTTP round trip against a mock provider.

use meclaw_cells::llm::LlmCell;
use meclaw_cells::{LlmParams, sealed};
use meclaw_colony::StatefulCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, Headers, Message, MessageBuilder, OutputSink, Path, Uuid};
use meclaw_testing::mock_http::{MockResponse, start_mock_server_capturing};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// The credential the "vault" delivers. A fixture, not a key — the shape is
/// deliberately unmistakable if it ever shows up where it must not.
const SECRET: &str = "sk-test-not-a-key-gh457";

// ───────────────────────────────────────────────────────────────── the harness

/// The cell under test: a grant, no key of its own, and the two GH #457 knobs
/// spelled out so no test depends on a default it did not choose.
fn cell(base_url: &str, wait_ms: u64, wait_max: usize) -> LlmCell {
    let raw = json!({
        "provider": "openai",
        "model": "gpt-4o-mini",
        "api_key": "",
        "credential_grant_id": "grant:gh457",
        "base_url": base_url,
        "external_timeout_ms": 5_000u64,
        "credential_wait_ms": wait_ms,
        "credential_wait_max": wait_max,
    });
    LlmCell::new(
        LlmParams::parse(&raw).expect("params"),
        reqwest::Client::builder().build().expect("http client"),
    )
}

/// A sink per turn, all feeding one receiver.
///
/// Per turn on purpose: the sink is what stamps `parent_message_id` on an
/// emission, so a receipt that came back on the wrong turn's sink is visible
/// here as a wrong parent — which is how "nothing is silently lost" is measured
/// below rather than assumed.
fn sink_for(tx: &mpsc::Sender<CellEmission>, parent: Uuid) -> OutputSink {
    OutputSink::new(
        tx.clone(),
        Path::new("/llm"),
        parent,
        Uuid::now_v7(),
        32,
        Headers::new(),
        None,
    )
}

fn user_turn(text: &str) -> Message {
    MessageBuilder::new(Path::new("/llm"))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .build()
}

fn chat_answer() -> MockResponse {
    MockResponse::ok_json(
        json!({
            "id": "chatcmpl-1", "object": "chat.completion", "created": 1,
            "model": "gpt-4o-mini",
            "choices": [{"index": 0, "finish_reason": "stop",
                         "message": {"role": "assistant", "content": "pong"}}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        })
        .to_string()
        .as_bytes(),
    )
}

/// A live `cell.db`, the only kind `handle()` accepts.
fn db(td: &tempfile::TempDir) -> meclaw_colony::DbConn {
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db"))
        .expect("cell.db");
    meclaw_colony::DbConn::wrap(conn, None)
}

/// Everything the cell has emitted so far, without waiting for it to be done.
fn drain(rx: &mut mpsc::Receiver<CellEmission>) -> Vec<Value> {
    let mut out = Vec::new();
    while let Ok(em) = rx.try_recv() {
        out.push(json!({
            "parent": em.parent_message_id.map(|u| u.simple().to_string()),
            "content": em.content,
        }));
    }
    out
}

fn error_codes(seen: &[Value]) -> Vec<String> {
    seen.iter()
        .filter_map(|e| e["content"]["header"]["error_code"].as_str())
        .map(str::to_string)
        .collect()
}

/// The `credential_request` the cell emitted, as a sealed box addressed to the
/// recipient key it minted. This is the vault's half of the round, performed by
/// the test — with the same public key and the same crypto the broker uses.
fn seal_for(seen: &[Value]) -> Value {
    let ask = seen
        .iter()
        .find(|e| e["content"]["header"]["route"] == "credential_request")
        .unwrap_or_else(|| panic!("the cell never asked for its credential: {seen:?}"));
    let args: Value = meclaw_core::serde_json::from_str(
        ask["content"]["messages"][0]["text"]
            .as_str()
            .expect("the request carries its args as text"),
    )
    .expect("the args are JSON");
    let recipient = args["payload"]["recipient_key"]
        .as_str()
        .expect("recipient_key");
    sealed::seal_to(recipient, SECRET.as_bytes())
        .expect("seal")
        .to_json()
}

fn delivery(sealed_box: Value) -> Message {
    MessageBuilder::new(Path::new("/llm"))
        .body(Body::Inline(json!({"sealed": sealed_box})))
        .build()
}

/// The user text of the LAST turn in a captured provider request — the one this
/// call was made for.
fn asked_about(req: &meclaw_testing::mock_http::CapturedRequest) -> String {
    let body: Value = meclaw_core::serde_json::from_slice(&req.body).expect("request body is JSON");
    let msgs = body["messages"].as_array().expect("messages[]");
    msgs.last()
        .and_then(|m| m["content"].as_str())
        .unwrap_or_default()
        .to_string()
}

async fn captured(
    c: &Arc<tokio::sync::Mutex<Vec<meclaw_testing::mock_http::CapturedRequest>>>,
) -> Vec<meclaw_testing::mock_http::CapturedRequest> {
    c.lock().await.clone()
}

// ═════════════════════════════════════════════════════════════════════════ pins

/// (a) The claim the issue is titled after.
///
/// One turn arrives at a cell that holds nothing. It is not refused: the cell
/// asks the vault, holds the turn, and when the box comes back the SAME turn
/// reaches the provider — with the credential that was sealed to it. No
/// `credential_pending` anywhere in the round.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_the_first_turn_after_a_spawn_is_answered_once_the_box_arrives() {
    let (addr, _server, capture) = start_mock_server_capturing(vec![chat_answer()]).await;
    let td = tempfile::TempDir::new().unwrap();
    let mut db = db(&td);
    let mut cell = cell(&format!("http://{addr}/v1"), 30_000, 16);
    let (tx, mut rx) = mpsc::channel(64);

    let first = Uuid::now_v7();
    cell.handle(user_turn("ping"), &sink_for(&tx, first), &mut db)
        .await;

    let asked = drain(&mut rx);
    assert!(
        error_codes(&asked).is_empty(),
        "the triggering turn was refused instead of parked: {asked:?}"
    );
    assert!(
        captured(&capture).await.is_empty(),
        "a cell without a credential called the provider anyway"
    );

    // The vault answers.
    let box_json = seal_for(&asked);
    cell.handle(delivery(box_json), &sink_for(&tx, Uuid::now_v7()), &mut db)
        .await;

    let after = drain(&mut rx);
    assert!(
        error_codes(&after).is_empty(),
        "the released turn was not answered cleanly: {after:?}"
    );
    let answer = after
        .iter()
        .find(|e| e["content"]["header"]["finish_reason"] == "stop")
        .unwrap_or_else(|| panic!("the parked turn was never answered: {after:?}"));
    assert_eq!(
        answer["parent"].as_str(),
        Some(first.simple().to_string().as_str()),
        "the answer was filed under the wrong turn: {after:?}"
    );

    // And it is the vault's value that reached the wire — the cell's own config
    // carries an empty key, so no other path could have produced this header.
    let seen = captured(&capture).await;
    assert_eq!(seen.len(), 1, "expected exactly one provider call");
    assert_eq!(
        seen[0].headers.get("authorization").map(String::as_str),
        Some(format!("Bearer {SECRET}").as_str()),
        "the bearer on the wire is not the sealed one: {:?}",
        seen[0].headers
    );
    assert_eq!(asked_about(&seen[0]), "ping");
}

/// (b) The failure case, and the promise that it is still a case with receipts.
///
/// The vault is never played: no box ever comes back. Every turn that was
/// parked gets its own `credential_pending`, on its own sink — which is what
/// "nothing goes silently missing" means when it is measured rather than
/// asserted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn b_a_vault_that_never_delivers_gives_every_parked_turn_its_receipt() {
    let (addr, _server, capture) = start_mock_server_capturing(vec![chat_answer()]).await;
    let td = tempfile::TempDir::new().unwrap();
    let mut db = db(&td);
    let mut cell = cell(&format!("http://{addr}/v1"), 400, 16);
    let (tx, mut rx) = mpsc::channel(64);

    let parents: Vec<Uuid> = (0..3).map(|_| Uuid::now_v7()).collect();
    for (i, parent) in parents.iter().enumerate() {
        cell.handle(
            user_turn(&format!("turn {i}")),
            &sink_for(&tx, *parent),
            &mut db,
        )
        .await;
    }

    // The deadline is the cell's only signal here: a broker refusal travels the
    // topology's error lane and never comes back to the asking cell, so there is
    // no message to wait for. Generous marker (CLAUDE.md § Coding-Standards)
    // against a loaded host; the discriminator is the count, not the clock.
    let mut seen = Vec::new();
    for _ in 0..120 {
        seen.extend(drain(&mut rx));
        if error_codes(&seen).len() >= 3 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    assert_eq!(
        error_codes(&seen),
        vec![
            "credential_pending".to_string(),
            "credential_pending".to_string(),
            "credential_pending".to_string()
        ],
        "one receipt per parked turn, and nothing else: {seen:?}"
    );
    let receipted: Vec<Option<&str>> = seen
        .iter()
        .filter(|e| e["content"]["header"]["error_code"] == "credential_pending")
        .map(|e| e["parent"].as_str())
        .collect();
    let expected: Vec<Option<String>> = parents
        .iter()
        .map(|p| Some(p.simple().to_string()))
        .collect();
    assert_eq!(
        receipted,
        expected
            .iter()
            .map(|o| o.as_deref())
            .collect::<Vec<Option<&str>>>(),
        "the receipts are not the parked turns', in order: {seen:?}"
    );
    assert_eq!(
        seen.iter()
            .filter(|e| e["content"]["header"]["route"] == "credential_request")
            .count(),
        1,
        "three turns must cost ONE vault request, not three: {seen:?}"
    );
    assert!(
        captured(&capture).await.is_empty(),
        "a cell without a credential called the provider anyway"
    );
}

/// (c) The bound. Two slots, three turns: the third is refused on the spot —
/// not after the deadline, which is a minute away — and the two that fit are
/// still served when the box arrives.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c_the_overflow_turn_is_refused_at_once_and_the_bound_is_still_served() {
    let (addr, _server, capture) =
        start_mock_server_capturing(vec![chat_answer(), chat_answer()]).await;
    let td = tempfile::TempDir::new().unwrap();
    let mut db = db(&td);
    let mut cell = cell(&format!("http://{addr}/v1"), 60_000, 2);
    let (tx, mut rx) = mpsc::channel(64);

    let overflow = Uuid::now_v7();
    cell.handle(user_turn("first"), &sink_for(&tx, Uuid::now_v7()), &mut db)
        .await;
    cell.handle(user_turn("second"), &sink_for(&tx, Uuid::now_v7()), &mut db)
        .await;
    cell.handle(user_turn("third"), &sink_for(&tx, overflow), &mut db)
        .await;

    let early = drain(&mut rx);
    assert_eq!(
        error_codes(&early),
        vec!["credential_pending".to_string()],
        "exactly the overflow turn is refused, and it is refused now: {early:?}"
    );
    let receipt = early
        .iter()
        .find(|e| e["content"]["header"]["error_code"] == "credential_pending")
        .expect("checked above");
    assert_eq!(
        receipt["parent"].as_str(),
        Some(overflow.simple().to_string().as_str()),
        "the wrong turn was refused: {early:?}"
    );

    let box_json = seal_for(&early);
    cell.handle(delivery(box_json), &sink_for(&tx, Uuid::now_v7()), &mut db)
        .await;

    let after = drain(&mut rx);
    assert!(
        error_codes(&after).is_empty(),
        "a turn inside the bound was refused after all: {after:?}"
    );
    let texts: Vec<String> = captured(&capture).await.iter().map(asked_about).collect();
    assert_eq!(
        texts,
        vec!["first".to_string(), "second".to_string()],
        "the two turns that fit did not both reach the provider"
    );
}

/// (d) Order. A conversation is a sequence, so a buffer that reordered it would
/// be worse than the drop it replaced.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn d_the_parked_turns_reach_the_provider_in_the_order_they_arrived() {
    let (addr, _server, capture) = start_mock_server_capturing(vec![
        chat_answer(),
        chat_answer(),
        chat_answer(),
        chat_answer(),
    ])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    let mut db = db(&td);
    let mut cell = cell(&format!("http://{addr}/v1"), 60_000, 16);
    let (tx, mut rx) = mpsc::channel(64);

    let wanted = ["alpha", "bravo", "charlie", "delta"];
    for text in wanted {
        cell.handle(user_turn(text), &sink_for(&tx, Uuid::now_v7()), &mut db)
            .await;
    }
    let asked = drain(&mut rx);
    let box_json = seal_for(&asked);
    cell.handle(delivery(box_json), &sink_for(&tx, Uuid::now_v7()), &mut db)
        .await;

    let texts: Vec<String> = captured(&capture).await.iter().map(asked_about).collect();
    assert_eq!(
        texts,
        wanted
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<String>>(),
        "the parked batch was reordered"
    );
    let after = drain(&mut rx);
    assert!(
        error_codes(&after).is_empty(),
        "a released turn was refused: {after:?}"
    );
    assert_eq!(
        after
            .iter()
            .filter(|e| e["content"]["header"]["finish_reason"] == "stop")
            .count(),
        4,
        "every parked turn must be answered exactly once: {after:?}"
    );
}

//! GH #1092 — a broker DENIAL of a credential round reaches the cell that
//! asked, and the cell answers its parked turns at once.
//!
//! Measured in 0.62.0: in every colony without a vault deposit the shell's
//! translator (`/os/llm-registry/translate`, grant
//! `grant:openrouter@colony-os/llm-registry-translate`) asked for its key at
//! boot, the access hive answered `ack {outcome: denied, reason_code:
//! vault_locked}`, and no edge carried that answer home: it died at
//! `/os/access` as `hive_no_route` (8 of 150 orga lab runs red on exactly
//! this), while the translator's turn waited out its `wait_ms`. The shell now
//! routes every ack of its own grants to the requester (`meclaw-os`), and the
//! shared grant round takes a denial (`CredentialSlots::accept_denial`).
//!
//! | claim | test |
//! |---|---|
//! | the llm cell answers its parked turn when the broker denies, and asks again on the next | [`gh1092_the_llm_cell_answers_its_parked_turn_when_the_broker_denies`] |
//! | the shell routes the denial of each of its grants to the requester | [`gh1092_the_shell_routes_every_ack_of_its_grants_to_the_requester`] |
//! | the shared round hands a denied round's items back at once and asks again | [`gh1092_a_denial_refuses_the_round_at_once`] |
//! | a chat turn, a box, another grant's denial or a superseded round's denial is not this round's denial | [`gh1092_only_a_denial_of_this_round_is_taken`] |
//!
//! The colony-level lock is in `gh974_a_person_round_leaves_no_dead_letter.rs`
//! (`a_shell_with_no_vault_deposit_leaves_only_declared_dead_letters`).

use meclaw_cells::LlmParams;
use meclaw_cells::credential::{CredentialSlots, ExpiryFn, ParkOutcome, SlotState};
use meclaw_cells::llm::LlmCell;
use meclaw_colony::StatefulCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, Headers, Message, MessageBuilder, OutputSink, Path, Uuid};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

const GRANT: &str = "grant:openrouter@colony-os/llm-registry-translate";

fn cell(raw: Value) -> LlmCell {
    LlmCell::new(
        LlmParams::parse(&raw).expect("params"),
        reqwest::Client::builder().build().expect("http client"),
    )
}

fn sink_for(tx: &mpsc::Sender<CellEmission>) -> OutputSink {
    OutputSink::new(
        tx.clone(),
        Path::new("/llm"),
        Uuid::now_v7(),
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

fn cell_db(td: &tempfile::TempDir) -> meclaw_colony::DbConn {
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db"))
        .expect("cell.db");
    meclaw_colony::DbConn::wrap(conn, None)
}

fn drain(rx: &mut mpsc::Receiver<CellEmission>) -> Vec<Value> {
    let mut out = Vec::new();
    while let Ok(em) = rx.try_recv() {
        out.push(em.content);
    }
    out
}

fn requests(seen: &[Value]) -> Vec<&Value> {
    seen.iter()
        .filter(|e| e["header"]["route"] == "credential_request")
        .collect()
}

/// The access hive's `refuse` answer, as the shell's `in_sealed` edge
/// delivers it: one `tool_result` naming the request's call id, no `sealed`.
fn denial(grant: &str, call_id: &str, reason: &str) -> Message {
    MessageBuilder::new(Path::new("/llm"))
        .body(Body::Inline(json!({"messages": [{
            "origin": "tool", "type": "tool_result", "id": call_id,
            "text": json!({"outcome": "denied", "reason_code": reason,
                           "grant_id": grant}).to_string()}]})))
        .build()
}

#[tokio::test]
async fn gh1092_the_llm_cell_answers_its_parked_turn_when_the_broker_denies() {
    let td = tempfile::TempDir::new().unwrap();
    let mut db = cell_db(&td);
    let mut c = cell(json!({
        "provider": "openai", "model": "gpt-4o-mini", "api_key": "",
        "credential_grant_id": GRANT,
        "base_url": "http://127.0.0.1:9/v1", "external_timeout_ms": 5_000u64,
        // Far beyond the test: a receipt seen below was handed out by the
        // denial, never by the round's deadline.
        "credential_wait_ms": 600_000u64, "credential_wait_max": 16usize,
    }));
    let (tx, mut rx) = mpsc::channel(64);

    c.handle(user_turn("ping"), &sink_for(&tx), &mut db).await;
    let asked = drain(&mut rx);
    let [request] = requests(&asked)[..] else {
        panic!("exactly one credential request: {asked:?}");
    };
    let call_id = request["messages"][0]["id"]
        .as_str()
        .expect("the request's call id")
        .to_string();

    c.handle(
        denial(GRANT, &call_id, "vault_locked"),
        &sink_for(&tx),
        &mut db,
    )
    .await;
    let answered = drain(&mut rx);
    let codes: Vec<&str> = answered
        .iter()
        .map(|e| e["header"]["error_code"].as_str().unwrap_or("<none>"))
        .collect();
    assert_eq!(
        codes,
        ["credential_pending"],
        "the denial answers the parked turn with its receipt, and nothing else \
         (no reject of the denial, no second request): {answered:?}"
    );

    // The round is over: the next turn opens a new one and asks again.
    c.handle(user_turn("ping again"), &sink_for(&tx), &mut db)
        .await;
    let again = drain(&mut rx);
    assert_eq!(
        requests(&again).len(),
        1,
        "the next turn asks again: {again:?}"
    );
    assert_eq!(again.len(), 1, "and does nothing else: {again:?}");
}

/// The shell's own grants: every `ack` that names one goes to the requester
/// and to nobody else -- the out-edge `./access -> .` excludes them, so an
/// `in_sealed` edge that took only `vault.deliver` left the denial with no
/// route at all.
#[test]
fn gh1092_the_shell_routes_every_ack_of_its_grants_to_the_requester() {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates/meclaw-os/config.json");
    let Ok(raw) = std::fs::read_to_string(&p) else {
        return; // GH #49: the template library does not travel with every tree
    };
    let cfg: Value = meclaw_core::serde_json::from_str(&raw).expect("meclaw-os config");
    let edges = cfg["params"]["graph"]["edges"].as_array().expect("edges");
    for (grant, requester) in [
        ("grant:openrouter@colony-os/argus-judge", "./argus/judge"),
        (GRANT, "./llm-registry/translate"),
    ] {
        let home: Vec<&str> = edges
            .iter()
            .filter(|e| e["from"] == "./access" && e["to"] == requester)
            .filter_map(|e| e["condition"].as_str())
            .collect();
        let [cond] = home[..] else {
            panic!("exactly one edge ./access -> {requester}: {home:?}");
        };
        assert!(
            cond.contains(&format!("hop.grant_id == '{grant}'")) && cond.contains("'ack'"),
            "the edge home names the grant and the ack: {cond}"
        );
        assert!(
            !cond.contains("vault.deliver"),
            "the edge home carries every ack of {grant} -- the box AND the denial: {cond}"
        );
    }
}

/// The items a warden handed to `on_expired`. A std mutex in a TEST collector
/// is fine; the no-lock rule is about cell state.
type Expired = Arc<Mutex<Vec<u32>>>;

fn slots() -> (CredentialSlots<u32>, Expired) {
    let seen: Expired = Arc::new(Mutex::new(Vec::new()));
    let s = Arc::clone(&seen);
    let on_expired: ExpiryFn<u32> = Arc::new(move |batch, _| {
        let s = Arc::clone(&s);
        Box::pin(async move {
            s.lock().unwrap().extend(batch);
        })
    });
    // A deadline far beyond the test: whatever comes back came from the denial.
    (CredentialSlots::new(600_000, 16, on_expired), seen)
}

fn denial_body(grant: &str, call_id: &str, reason: &str) -> Value {
    json!({"messages": [{"origin": "tool", "type": "tool_result", "id": call_id,
        "text": json!({"outcome": "denied", "reason_code": reason,
                       "grant_id": grant}).to_string()}]})
}

fn call_id_of(request: &Value) -> String {
    request["messages"][0]["id"]
        .as_str()
        .expect("call id")
        .to_string()
}

#[tokio::test]
async fn gh1092_a_denial_refuses_the_round_at_once() {
    let (mut s, expired) = slots();
    assert!(matches!(s.park(GRANT, 1), ParkOutcome::Asked));
    assert!(matches!(s.park(GRANT, 2), ParkOutcome::Parked));
    let call_id = call_id_of(&s.request(GRANT).expect("request"));

    let denied = s
        .accept_denial(&denial_body(GRANT, &call_id, "vault_locked"))
        .await
        .expect("the denial of the round in flight is taken");
    assert_eq!(denied.grant, GRANT);
    assert_eq!(denied.reason_code, "vault_locked");
    assert_eq!(denied.refused, vec![1, 2], "every parked item, in order");
    assert_eq!(s.state(GRANT), SlotState::Empty, "no request in flight");
    assert!(
        expired.lock().unwrap().is_empty(),
        "the warden handed nothing out"
    );

    // The next item opens a new round and asks again.
    assert!(matches!(s.park(GRANT, 3), ParkOutcome::Asked));
}

#[tokio::test]
async fn gh1092_only_a_denial_of_this_round_is_taken() {
    let (mut s, _) = slots();
    assert!(matches!(s.park(GRANT, 1), ParkOutcome::Asked));
    let first = call_id_of(&s.request(GRANT).expect("request"));

    // Not a denial at all: handled by the cell like any other message.
    let turn = json!({"messages": [{"origin": "user", "type": "text", "text": "hi"}]});
    assert!(s.accept_denial(&turn).await.is_none(), "a chat turn");
    let ok = json!({"messages": [{"origin": "tool", "type": "tool_result", "id": first,
        "text": json!({"outcome": "ok", "grant_id": GRANT}).to_string()}]});
    assert!(s.accept_denial(&ok).await.is_none(), "an ok ack");
    let mut boxed = denial_body(GRANT, &first, "vault_locked");
    boxed["sealed"] = json!({});
    assert!(s.accept_denial(&boxed).await.is_none(), "a body with a box");
    let mut two = denial_body(GRANT, &first, "vault_locked");
    two["messages"]
        .as_array_mut()
        .unwrap()
        .push(json!({"origin": "user", "type": "text", "text": "and more"}));
    assert!(
        s.accept_denial(&two).await.is_none(),
        "a conversation that carries a denial among other turns"
    );
    assert!(
        s.accept_denial(&denial_body("grant:someone-else", &first, "vault_locked"))
            .await
            .is_none(),
        "the denial of a grant this cell does not hold"
    );
    assert_eq!(s.state(GRANT), SlotState::Asked, "the round is untouched");

    // A superseded request's denial refuses nothing: the round in flight
    // still has its own answer coming.
    let second = call_id_of(&s.request(GRANT).expect("re-ask"));
    let late = s
        .accept_denial(&denial_body(GRANT, &first, "vault_locked"))
        .await
        .expect("taken -- it is this slot's broker answer");
    assert!(
        late.refused.is_empty(),
        "nothing refused: {:?}",
        late.refused
    );
    assert_eq!(s.state(GRANT), SlotState::Asked, "the newer request stands");
    let now = s
        .accept_denial(&denial_body(GRANT, &second, "vault_locked"))
        .await
        .expect("the newer request's denial");
    assert_eq!(now.refused, vec![1], "it ends the round");
}

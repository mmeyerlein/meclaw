//! GH #1252 — a late sealed box of a replaced instance does not consume the
//! open round's key.
//!
//! Measured in the orga lab: an instance replaced by `replace_nodes` while its
//! vault round was in flight; the old round's box reached the new instance
//! after the new instance had asked for the same grant. The box carried a call
//! id the new instance never issued (and that was not among its superseded
//! ones), so `accept_sealed` opened it with the new round's key: `DidNotOpen`,
//! key consumed, and the new round's own box then arrived for a slot with no
//! key — three rounds, about 40 s, until the cell had its credential.
//!
//! Second pin (finding KV2-N1): a turn parked while the warden of an expired
//! round is still inside `on_expired` went down with that round's receiver.

use meclaw_cells::credential::{CredentialSlots, ExpiryFn, ParkOutcome, Refusal, SlotState};
use meclaw_cells::sealed::{RecipientKeypair, seal_to};
use meclaw_core::serde_json::{Value, json};
use std::sync::Arc;

fn slots() -> CredentialSlots<u32> {
    let on_expired: ExpiryFn<u32> = Arc::new(|_, _| Box::pin(async {}));
    CredentialSlots::new(30_000, 16, on_expired)
}

fn recipient_of(request: &Value) -> String {
    let args: Value =
        meclaw_core::serde_json::from_str(request["messages"][0]["text"].as_str().unwrap())
            .unwrap();
    args["payload"]["recipient_key"]
        .as_str()
        .unwrap()
        .to_string()
}

fn call_id_of(request: &Value) -> String {
    request["messages"][0]["id"].as_str().unwrap().to_string()
}

/// The delivery as `access/invoke` shapes it: the broker's ack plus the box.
fn delivery(grant: &str, call_id: &str, recipient: &str, secret: &str) -> Value {
    json!({
        "messages": [{"origin": "tool", "type": "tool_result", "id": call_id,
                      "text": json!({"outcome": "ok", "grant_id": grant,
                                     "operation": "vault.deliver"}).to_string()}],
        "sealed": seal_to(recipient, secret.as_bytes()).unwrap().to_json(),
    })
}

#[tokio::test]
async fn gh1252_a_foreign_box_does_not_consume_the_open_rounds_key() {
    let mut s = slots();
    assert!(matches!(s.park("g1", 0), ParkOutcome::Asked));
    let req = s.request("g1").unwrap();
    // The replaced instance's round: its own key pair, a call id this cell
    // never issued, the same grant.
    let replaced = RecipientKeypair::generate().unwrap();
    let late = delivery(
        "g1",
        "call-of-the-replaced-instance",
        &replaced.public_hex(),
        "stub-secret-old",
    );
    match s.accept_sealed(&late).await {
        Err(r @ Refusal::Unsolicited) => {
            assert!(r.into_refused().is_empty(), "nothing of the round refused");
        }
        Err(r) => panic!("a foreign box is refused as unasked, got {:?}", r.detail()),
        Ok(_) => panic!("a foreign box must not open"),
    }
    assert_eq!(s.state("g1"), SlotState::Asked, "the round stays open");
    let own = delivery(
        "g1",
        &call_id_of(&req),
        &recipient_of(&req),
        "stub-secret-new",
    );
    let Ok(got) = s.accept_sealed(&own).await else {
        panic!("the round's own box opens with the round's key");
    };
    assert_eq!(got.released, vec![0], "the parked item is released");
    assert_eq!(s.secret("g1"), Some("stub-secret-new"));
}

/// KV2-N1 (#1252): a turn that parks while the warden is still answering its
/// expired round (inside `on_expired`) must not be dropped with that round —
/// it belongs to the next round, whose warden answers it in turn.
#[tokio::test(start_paused = true)]
async fn gh1252_a_turn_parked_while_the_warden_answers_is_not_lost() {
    let (batches_tx, mut batches) = tokio::sync::mpsc::unbounded_channel::<Vec<u32>>();
    let go = Arc::new(tokio::sync::Notify::new());
    let gate = Arc::clone(&go);
    let on_expired: ExpiryFn<u32> = Arc::new(move |held, _| {
        let (tx, gate) = (batches_tx.clone(), Arc::clone(&gate));
        Box::pin(async move {
            let _ = tx.send(held);
            // Held open on an event: the round is expired, its warden has not
            // returned yet (in production: the receipts' pending pushes).
            gate.notified().await;
        })
    });
    let mut s = CredentialSlots::new(1_000, 16, on_expired);
    assert!(matches!(s.park("g1", 1), ParkOutcome::Asked));
    assert_eq!(
        batches.recv().await,
        Some(vec![1]),
        "the first round expires"
    );
    let into_the_dying_round = matches!(s.park("g1", 2), ParkOutcome::Parked);
    go.notify_one();
    let next = tokio::time::timeout(meclaw_testing::deadline::HANG, batches.recv()).await;
    assert_eq!(
        next.ok().flatten(),
        Some(vec![2]),
        "a turn parked inside on_expired got no receipt \
         (parked into the dying round: {into_the_dying_round})"
    );
}

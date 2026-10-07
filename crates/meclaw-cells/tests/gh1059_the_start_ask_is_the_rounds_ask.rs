//! GH #1059 (review V1, I1): a long-running cell asks in `on_start`, before
//! anything is parked. An item parked while that question is in flight joins
//! its round — no second request, and the first box opens and releases it.
//! Before the fix `park` saw no round, answered `Asked`, the caller asked again
//! and the `on_start` box came back `Late` and was discarded: one superfluous
//! round per cold start, in exactly the proxy/voice normal case.

use meclaw_cells::credential::{CredentialSlots, ExpiryFn, ParkOutcome, SlotState};
use meclaw_cells::sealed::seal_to;
use meclaw_core::serde_json::{Value, json};
use std::sync::Arc;

fn slots(wait_ms: u64) -> CredentialSlots<u32> {
    let on_expired: ExpiryFn<u32> = Arc::new(|_, _| Box::pin(async {}));
    CredentialSlots::new(wait_ms, 16, on_expired)
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

fn delivery(grant: &str, request: &Value, secret: &str) -> Value {
    json!({
        "messages": [{"origin": "tool", "type": "tool_result",
                      "id": request["messages"][0]["id"],
                      "text": json!({"outcome": "ok", "grant_id": grant,
                                     "operation": "vault.deliver"}).to_string()}],
        "sealed": seal_to(&recipient_of(request), secret.as_bytes()).unwrap().to_json(),
    })
}

#[tokio::test]
async fn gh1059_a_park_after_the_start_ask_joins_its_round() {
    let mut s = slots(10_000);
    let asked = s.request_all(["g1"]);
    let start_request = asked[0].1.as_ref().unwrap().clone();
    assert!(
        matches!(s.park("g1", 7), ParkOutcome::Parked),
        "the on_start question is this round's question — no second request"
    );
    assert!(
        matches!(s.park("g1", 8), ParkOutcome::Parked),
        "and every further item joins the same round"
    );
    let Ok(got) = s
        .accept_sealed(&delivery("g1", &start_request, "stub-secret-1"))
        .await
    else {
        panic!("the box of the on_start question opens");
    };
    assert_eq!(got.released, vec![7, 8]);
    assert_eq!(s.state("g1"), SlotState::Open);
}

#[tokio::test]
async fn gh1059_a_park_after_an_answered_round_still_asks() {
    // The flag is the start ask's alone: once its box opened and the secret is
    // forgotten (a provider said 401), the next item opens a fresh round.
    let mut s = slots(10_000);
    let r = s.request("g1").unwrap();
    assert!(
        s.accept_sealed(&delivery("g1", &r, "stub-secret-1"))
            .await
            .is_ok()
    );
    s.forget("g1");
    assert!(matches!(s.park("g1", 1), ParkOutcome::Asked));
}

#[tokio::test]
async fn gh1059_the_secret_travels_sealed() {
    // Review V1, M2: a long-running cell hands its secret to the I/O half; the
    // handle it gets is a `Secret` (Debug `<sealed>`), not a bare `String`.
    let mut s = slots(10_000);
    let r = s.request("g1").unwrap();
    assert!(
        s.accept_sealed(&delivery("g1", &r, "stub-secret-2"))
            .await
            .is_ok()
    );
    let handle = s.secret_handle("g1").expect("the box opened");
    assert_eq!(format!("{handle:?}"), "<sealed>");
    assert_eq!(handle.clone().expose(), "stub-secret-2");
    assert!(s.secret_handle("g2").is_none());
}

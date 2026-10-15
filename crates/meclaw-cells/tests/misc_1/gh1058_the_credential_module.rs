//! GH #1058 — the grant round of a cell, once, for every cell type.
//!
//! `meclaw_cells::credential` is what `llm` did alone since GH #421/#457 —
//! ask once, park, release on the sealed box, refuse on a bad box — lifted out
//! so `web_search`, `code`, `proxy` and `voice` spend grants through the same
//! code. These are its locks (T1–T6 of the plan), driven through the public API
//! only: the same surface the other cell types build on, so this file doubles as
//! the usage example.
//!
//! The "vault" here is the test: it reads the recipient key out of the request
//! and seals a stub value to it with the same crypto the vault uses.
//!
//! | claim | test |
//! |---|---|
//! | T1 one request for many parked items, all released by the box | [`gh1058_a_slot_asks_once_for_many_parked_items`] |
//! | T2 two grants are two slots | [`gh1058_two_grants_of_one_cell_are_two_slots`] |
//! | T3 a box that does not open refuses every parked item | [`gh1058_a_box_that_does_not_open_refuses_every_parked_item`] |
//! | T4 a box nobody asked for is refused, nothing opened | [`gh1058_an_unsolicited_box_is_refused`] |
//! | T5 the deadline answers every parked item | [`gh1058_the_wait_bound_answers_every_parked_item`] |
//! | T6 the secret never prints | [`gh1058_the_secret_never_prints`] |
//! | a late box of a superseded round leaves the new round alone | [`gh1058_a_late_box_of_a_superseded_round_does_not_refuse_the_new_one`] |
//! | the bound is a bound | [`gh1058_the_overflow_is_refused_at_once`] |
//! | OR-VG-4 bearer precedence | [`gh1058_bearer_precedence`] |

use meclaw_cells::credential::{
    CredentialParams, CredentialSlots, ExpiryCause, ExpiryFn, ParkOutcome, Refusal, SlotState,
    bearer, literal_is_ignored,
};
use meclaw_cells::sealed::{RecipientKeypair, seal_to};
use meclaw_core::serde_json::{Value, json};
use std::sync::{Arc, Mutex};

/// What the warden handed to `on_expired`: (item, wait_ms). A std mutex in a
/// TEST collector is fine; the no-lock rule is about cell state.
type Seen = Arc<Mutex<Vec<(u32, u64)>>>;

fn slots(wait_ms: u64, wait_max: usize) -> (CredentialSlots<u32>, Seen) {
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let s = Arc::clone(&seen);
    let on_expired: ExpiryFn<u32> = Arc::new(move |batch, cause| {
        let s = Arc::clone(&s);
        let wait = match cause {
            ExpiryCause::Deadline { wait_ms } => wait_ms,
            ExpiryCause::CellGone => 0,
        };
        Box::pin(async move {
            for item in batch {
                s.lock().unwrap().push((item, wait));
            }
        })
    });
    (CredentialSlots::new(wait_ms, wait_max, on_expired), seen)
}

/// The recipient key a request carries — the broker's half of the round.
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

/// T1.
#[tokio::test]
async fn gh1058_a_slot_asks_once_for_many_parked_items() {
    let (mut s, _) = slots(30_000, 16);
    let mut asked = 0;
    for i in 0..5 {
        match s.park("g1", i) {
            ParkOutcome::Asked => asked += 1,
            ParkOutcome::Parked => {}
            ParkOutcome::Refused(_) => panic!("item {i} refused under the bound"),
        }
    }
    assert_eq!(asked, 1, "five items, one round, one request");
    let req = s.request("g1").unwrap();
    assert_eq!(req["header"]["route"], "credential_request");
    assert_eq!(req["header"]["grant_id"], "g1");
    let args: Value =
        meclaw_core::serde_json::from_str(req["messages"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(args["operation"], "vault.deliver");
    assert_eq!(args["grant_id"], "g1");
    let d = delivery(
        "g1",
        &call_id_of(&req),
        &recipient_of(&req),
        "stub-secret-1",
    );
    let Ok(got) = s.accept_sealed(&d).await else {
        panic!("the box opens");
    };
    assert_eq!(got.grant, "g1");
    assert_eq!(got.released, vec![0, 1, 2, 3, 4], "all five, arrival order");
    assert_eq!(s.secret("g1"), Some("stub-secret-1"));
    assert_eq!(s.state("g1"), SlotState::Open);
}

/// T2.
#[tokio::test]
async fn gh1058_two_grants_of_one_cell_are_two_slots() {
    let (mut s, _) = slots(30_000, 16);
    assert!(matches!(s.park("g1", 1), ParkOutcome::Asked));
    assert!(
        matches!(s.park("g2", 2), ParkOutcome::Asked),
        "own slot, own round"
    );
    let r1 = s.request("g1").unwrap();
    let r2 = s.request("g2").unwrap();
    let d2 = delivery("g2", &call_id_of(&r2), &recipient_of(&r2), "stub-secret-2");
    let Ok(got) = s.accept_sealed(&d2).await else {
        panic!("g2's box opens");
    };
    assert_eq!((got.grant.as_str(), got.released), ("g2", vec![2]));
    assert_eq!(s.secret("g2"), Some("stub-secret-2"));
    assert_eq!(s.secret("g1"), None, "slot 1 still waits");
    assert_eq!(s.state("g1"), SlotState::Asked);
    let d1 = delivery("g1", &call_id_of(&r1), &recipient_of(&r1), "stub-secret-1");
    let Ok(got) = s.accept_sealed(&d1).await else {
        panic!("g1's box opens");
    };
    assert_eq!((got.grant.as_str(), got.released), ("g1", vec![1]));
}

/// T3.
#[tokio::test]
async fn gh1058_a_box_that_does_not_open_refuses_every_parked_item() {
    let (mut s, _) = slots(30_000, 16);
    for i in 0..3 {
        let _ = s.park("g1", i);
    }
    let req = s.request("g1").unwrap();
    let stranger = RecipientKeypair::generate().unwrap();
    let d = delivery(
        "g1",
        &call_id_of(&req),
        &stranger.public_hex(),
        "stub-secret-1",
    );
    match s.accept_sealed(&d).await {
        Err(r @ Refusal::DidNotOpen { .. }) => {
            assert!(
                r.detail()
                    .unwrap()
                    .starts_with("the sealed box did not open")
            );
            assert_eq!(r.into_refused(), vec![0, 1, 2], "every parked item refused");
        }
        _ => panic!("a box for another recipient must not open"),
    }
    assert_eq!(s.state("g1"), SlotState::Empty, "the slot is empty again");
    assert_eq!(s.secret("g1"), None);
    assert!(
        matches!(s.park("g1", 9), ParkOutcome::Asked),
        "the next item asks anew"
    );
}

/// T4.
#[tokio::test]
async fn gh1058_an_unsolicited_box_is_refused() {
    let (mut s, _) = slots(30_000, 16);
    let anyone = RecipientKeypair::generate().unwrap();
    let d = delivery("g1", "call-x", &anyone.public_hex(), "stub-secret-1");
    match s.accept_sealed(&d).await {
        Err(r @ Refusal::Unsolicited) => assert_eq!(
            r.detail().unwrap(),
            "a sealed box arrived that this cell never asked for"
        ),
        _ => panic!("a box nobody asked for is refused"),
    }
    assert_eq!(s.secret("g1"), None, "nothing opened");
    // A slot whose request was already answered is not asking any more.
    let _ = s.park("g1", 1);
    let req = s.request("g1").unwrap();
    let d = delivery(
        "g1",
        &call_id_of(&req),
        &recipient_of(&req),
        "stub-secret-1",
    );
    assert!(s.accept_sealed(&d).await.is_ok());
    let again = delivery(
        "g1",
        &call_id_of(&req),
        &recipient_of(&req),
        "stub-secret-2",
    );
    assert!(matches!(
        s.accept_sealed(&again).await,
        Err(Refusal::Unsolicited)
    ));
    assert_eq!(
        s.secret("g1"),
        Some("stub-secret-1"),
        "the second box changed nothing"
    );
}

/// T5.
#[tokio::test]
async fn gh1058_the_wait_bound_answers_every_parked_item() {
    let (mut s, seen) = slots(50, 16);
    for i in 0..3 {
        let _ = s.park("g1", i);
    }
    assert!(matches!(s.park("g2", 7), ParkOutcome::Asked));
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let mut got = seen.lock().unwrap().clone();
    got.sort_unstable();
    assert_eq!(
        got,
        vec![(0, 50), (1, 50), (2, 50), (7, 50)],
        "every parked item of every slot reached the callback once"
    );
    // An expired round is no round: the next item opens a fresh one.
    assert!(matches!(s.park("g1", 3), ParkOutcome::Asked));
}

/// T6.
#[tokio::test]
async fn gh1058_the_secret_never_prints() {
    let (mut s, _) = slots(30_000, 16);
    let _ = s.park("g1", 0);
    let req = s.request("g1").unwrap();
    let d = delivery(
        "g1",
        &call_id_of(&req),
        &recipient_of(&req),
        "stub-secret-6",
    );
    assert!(s.accept_sealed(&d).await.is_ok());
    let printed = format!("{s:?}");
    assert!(!printed.contains("stub-secret-6"), "{printed}");
    assert!(printed.contains("<sealed>"), "{printed}");
}

/// The round matching of the plan (§ 2): a box of an expired, re-asked round
/// is discarded — it does not refuse the round that replaced it.
#[tokio::test]
async fn gh1058_a_late_box_of_a_superseded_round_does_not_refuse_the_new_one() {
    let (mut s, _) = slots(50, 16);
    let _ = s.park("g1", 0);
    let old = s.request("g1").unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        matches!(s.park("g1", 1), ParkOutcome::Asked),
        "deadline passed, re-ask"
    );
    let new = s.request("g1").unwrap();
    let late = delivery(
        "g1",
        &call_id_of(&old),
        &recipient_of(&old),
        "stub-secret-old",
    );
    match s.accept_sealed(&late).await {
        Err(r @ Refusal::Late { .. }) => assert!(r.detail().is_none()),
        _ => panic!("the late box is discarded"),
    }
    assert_eq!(
        s.state("g1"),
        SlotState::Asked,
        "the new round is untouched"
    );
    let d = delivery(
        "g1",
        &call_id_of(&new),
        &recipient_of(&new),
        "stub-secret-new",
    );
    let Ok(got) = s.accept_sealed(&d).await else {
        panic!("the new box opens");
    };
    assert_eq!(got.released, vec![1]);
    assert_eq!(s.secret("g1"), Some("stub-secret-new"));
}

#[tokio::test]
async fn gh1058_the_overflow_is_refused_at_once() {
    let (mut s, _) = slots(30_000, 2);
    assert!(matches!(s.park("g1", 0), ParkOutcome::Asked));
    assert!(matches!(s.park("g1", 1), ParkOutcome::Parked));
    match s.park("g1", 2) {
        ParkOutcome::Refused(item) => assert_eq!(*item, 2),
        _ => panic!("the third item does not fit a bound of two"),
    }
}

#[test]
fn gh1058_bearer_precedence() {
    // Grant set: only the delivered value, the literal never (OR-VG-4).
    assert_eq!(bearer(Some("g"), None, Some("stub-literal")), None);
    assert_eq!(
        bearer(Some("g"), Some("stub-v"), Some("stub-literal")),
        Some("stub-v")
    );
    assert_eq!(bearer(Some("g"), Some(""), Some("stub-literal")), None);
    // No grant (or an empty one): the transition literal.
    assert_eq!(
        bearer(None, None, Some("stub-literal")),
        Some("stub-literal")
    );
    assert_eq!(
        bearer(Some(""), None, Some("stub-literal")),
        Some("stub-literal")
    );
    assert_eq!(bearer(None, None, Some("")), None);
    assert!(literal_is_ignored(Some("g"), Some("k")));
    assert!(!literal_is_ignored(Some("g"), Some("")));
    assert!(!literal_is_ignored(None, Some("k")));
}

#[test]
fn gh1058_the_params_block_reads_the_llm_names_and_defaults() {
    let p: CredentialParams = meclaw_core::serde_json::from_value(json!({})).unwrap();
    assert_eq!(p, CredentialParams::default());
    assert_eq!((p.credential_wait_max, p.credential_wait_ms), (16, 10_000));
    let p: CredentialParams =
        meclaw_core::serde_json::from_value(json!({"credential_grant_id": ""})).unwrap();
    assert_eq!(p.grant(), None, "the empty string is not a grant");
}

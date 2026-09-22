//! GH #617 — a cell that is the birth point of a message it did NOT originate
//! carries the trace and the budget instead of minting them, and only if it declared so.

use meclaw_core::serde_json::json;
use meclaw_core::{CellEmission, CellOutput, IngressEmitError, OriginSink, Path, Uuid};
use tokio::sync::mpsc;

fn out() -> CellOutput {
    CellOutput {
        target: Path::new("/sink"),
        content: json!({"messages": []}),
    }
}

#[tokio::test]
async fn an_ingress_emission_carries_the_trace_and_the_budget_it_was_handed() {
    let (tx, mut rx) = mpsc::channel::<CellEmission>(4);
    let sink = OriginSink::new(tx, Path::new("/peer"), 64).with_ingress();
    let trace = Uuid::now_v7();
    sink.ingress()
        .expect("a declared sink hands out the handle")
        .emit(out(), trace, 41)
        .await
        .expect("the emission is sent");
    let em = rx.recv().await.expect("one emission");
    assert_eq!(em.trace_id, trace, "one conversation stays one trace");
    assert_eq!(
        em.input_ttl, 41,
        "the budget comes from the wire, not the seed (64)"
    );
    assert_eq!(em.parent_message_id, None, "nothing was consumed here");
    assert!(em.input_headers.context.is_empty() && em.input_headers.hop.is_empty());
}

#[tokio::test]
async fn a_carried_ttl_of_zero_is_refused_before_the_emission() {
    let (tx, mut rx) = mpsc::channel::<CellEmission>(4);
    let sink = OriginSink::new(tx, Path::new("/peer"), 64).with_ingress();
    let err = sink
        .ingress()
        .expect("handle")
        .emit(out(), Uuid::now_v7(), 0)
        .await
        .expect_err("a carried 0 must not buy a hop");
    assert!(matches!(err, IngressEmitError::NoBudget), "got {err:?}");
    assert!(
        rx.try_recv().is_err(),
        "refused BEFORE the emission: nothing reaches the colony"
    );
}

#[tokio::test]
async fn a_sink_without_the_declaration_has_no_ingress_handle() {
    let (tx, _rx) = mpsc::channel::<CellEmission>(4);
    let plain = OriginSink::new(tx.clone(), Path::new("/timer"), 64);
    assert!(plain.ingress().is_none(), "no declaration, no handle");
    let declared = OriginSink::new(tx, Path::new("/peer"), 64).with_ingress();
    assert!(
        declared.ingress().is_some(),
        "the declaration is the gate, and it opens"
    );
}

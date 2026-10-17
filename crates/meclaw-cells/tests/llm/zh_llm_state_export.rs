//! T6-05 (cell boundary, inventory Nr 2/3): an `llm` cell hands its state over
//! by MESSAGE, on its own connection.
//!
//! Before, a model swap's twin got the original's `system` table and running
//! budget because the colony opened the original's `cell.db` (`carry_system`,
//! `running_budget` in `mutation/twin.rs`) — the last substrate reads of a
//! running cell's database. The replacement starts here: the original answers
//! a `handover` export with ONE message that carries its `system` table and
//! its `params` overlay, each part in the document form of the `transfer`
//! slot (`meclaw-cell-export/1`), addressed only to the asker (`reply_to`,
//! direct), never along the cell's edges — the parts are personal.
//!
//! | claim | test |
//! |---|---|
//! | the export carries `system` + `params` as transfer documents, to `reply_to` only, in the import form | [`the_llm_hands_its_system_table_and_overlay_over_in_one_message`] |

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_colony::StatefulCell;
use meclaw_colony::db_transfer::{TRANSFER_FORMAT, TransferOutcome, import_document};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, Headers, MessageBuilder, OutputSink, Path, Uuid};
use tokio::sync::mpsc;

fn db_at(td: &tempfile::TempDir, name: &str) -> meclaw_colony::DbConn {
    let conn =
        meclaw_colony::persist::open_or_create_cell_db(&td.path().join(name)).expect("cell.db");
    meclaw_colony::DbConn::wrap(conn, None)
}

fn to_me(body: Value) -> meclaw_core::Message {
    MessageBuilder::new(Path::new("/brain"))
        .body(Body::Inline(body))
        .build()
}

#[tokio::test]
async fn the_llm_hands_its_system_table_and_overlay_over_in_one_message() {
    let td = tempfile::TempDir::new().unwrap();
    let mut db = db_at(&td, "cell.db");
    let mut cell = LlmCell::new(
        LlmParams::parse(&json!({
            "provider": "openai", "model": "gpt-4o", "api_key": "k",
            "base_url": "http://127.0.0.1:1/v1",
        }))
        .unwrap(),
        reqwest::Client::builder().build().unwrap(),
    );
    let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new("/brain"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        Headers::new(),
        None,
    );

    // The state a run grows: a system leaf written by message, a budget push.
    cell.handle(
        to_me(json!({"system": {"identity": {"text": "I am Sam."}}})),
        &sink,
        &mut db,
    )
    .await;
    cell.handle(
        to_me(json!({"params": {"reasoning_effort": "high"}})),
        &sink,
        &mut db,
    )
    .await;
    assert!(
        rx.try_recv().is_err(),
        "both writes are answered with silence"
    );

    let ask = MessageBuilder::new(Path::new("/brain"))
        .reply_to(Path::new("/twin"))
        .body(Body::Inline(json!({"handover": {"operation": "export"}})))
        .build();
    cell.handle(ask, &sink, &mut db).await;

    let em = rx.try_recv().expect("the export is answered");
    assert!(rx.try_recv().is_err(), "with exactly one message");
    assert_eq!(em.target, Path::new("/twin"), "addressed to the asker");
    assert!(
        em.direct_reply,
        "delivered to reply_to, never along the edges"
    );
    let handover = &em.content["handover"];
    assert_eq!(handover["operation"], "import", "the answer IS the import");
    let parts = handover["parts"].as_array().expect("parts");
    let part = |table: &str| {
        parts
            .iter()
            .find(|p| p["table"] == table)
            .unwrap_or_else(|| panic!("no {table} part: {parts:?}"))
            .clone()
    };
    let system = part("system");
    let params = part("params");
    for p in [&system, &params] {
        assert_eq!(p["format"], TRANSFER_FORMAT, "the transfer document form");
    }
    assert_eq!(system["rows"][0]["slot_path"], "identity");
    assert_eq!(params["key"], json!(["key"]));
    assert_eq!(params["rows"][0]["key"], "reasoning_effort");
    assert_eq!(params["rows"][0]["value"], "\"high\"");

    // The system part is a transfer document the substrate's import takes.
    let mut target = db_at(&td, "other.db");
    let args = system.as_object().unwrap().clone();
    let outcome = target.call(move |c| import_document(c, &args)).await;
    assert!(
        matches!(
            outcome,
            Ok(TransferOutcome::Done {
                rows_affected: 1,
                ..
            })
        ),
        "{outcome:?}"
    );
}

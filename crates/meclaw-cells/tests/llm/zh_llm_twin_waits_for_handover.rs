//! T6-06 (cell boundary, inventory Nr 2/3): a twin PARKS its turns until the
//! original's handover is imported, the way a cell that spends a grant parks
//! them until its sealed credential arrives.
//!
//! A twin born with `handover_wait_ms` holds every inference turn back: a
//! turn it ran first would reach the model without the original's identity,
//! tools and budget — the 06.10. lab measurement behind GH #1034 (0/52 tools,
//! a judge answering like a chat assistant). The handover arrives as ONE
//! message (T6-05's export answer, forwarded as is) and is written on the
//! twin's own connection, the original winning over the twin's seed (a seed
//! slot the original overwrote at run time must not fall back to the seed
//! value); the parked turns then run in arrival order. When no
//! handover comes, `handover_wait_ms` is the operation timeout A of that wait:
//! the twin wakes itself, says so on a WARN line and runs on without the
//! inheritance.
//!
//! The twin here spends a grant and holds no credential, so a released turn's
//! first visible step is its `credential_request` — no provider, no socket,
//! and the virtual clock stays exact (AGENTS rule 11).
//!
//! | claim | test |
//! |---|---|
//! | a turn waits for the handover, then runs on the inherited state | [`a_twin_parks_its_turn_until_the_handover_is_imported`] |
//! | without a handover the turn runs after exactly `handover_wait_ms`, without the inheritance | [`a_twin_runs_on_without_inheritance_after_its_wait`] |
//! | a refused budget still ends the wait in `cell.db`: after a wake a second import is refused | [`a_refused_budget_still_ends_the_wait_for_good`] |
//! | a cell that does not wait refuses an import (`invalid_input`) and changes no slot | [`a_cell_that_does_not_wait_refuses_an_import_and_writes_nothing`] |
//! | one `system` row that is no JSON object refuses the whole import; the next turn runs | [`a_malformed_system_row_refuses_the_whole_import`] |
//!
//! The import writes `system` past the SystemGate (GH #118) and the budget
//! past the params-slot rules, so only a twin that waits takes one; and
//! every row is read back as a JSON leaf object on each later turn, so one
//! row that is none would break the cell for good (re-check H2, I1/I2).

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_colony::{DbConn, StatefulCell};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, Headers, Message, MessageBuilder, OutputSink, Path, Uuid};
use meclaw_testing::deadline;
use tokio::sync::mpsc;

const WAIT_MS: u64 = 5_000;

fn cell(raw: Value) -> LlmCell {
    LlmCell::new(
        LlmParams::parse(&raw).expect("params"),
        reqwest::Client::builder().build().expect("http client"),
    )
}

fn twin_birth() -> Value {
    json!({
        "provider": "openai", "model": "twin-model", "api_key": "",
        "credential_grant_id": "grant:twin", "base_url": "http://127.0.0.1:1/v1",
        "handover_wait_ms": WAIT_MS,
    })
}

fn twin() -> LlmCell {
    cell(twin_birth())
}

/// The twin as the factory births it on a wake: its birth params with the
/// `cell.db` overlay replayed over them.
async fn twin_after_a_wake(db: &mut DbConn) -> LlmCell {
    db.call(|c| {
        LlmCell::restored(
            c,
            &twin_birth(),
            reqwest::Client::builder().build().unwrap(),
        )
        .expect("restored")
    })
    .await
}

fn db_at(td: &tempfile::TempDir, name: &str) -> DbConn {
    let conn =
        meclaw_colony::persist::open_or_create_cell_db(&td.path().join(name)).expect("cell.db");
    DbConn::wrap(conn, None)
}

fn sink(at: &str, tx: &mpsc::Sender<CellEmission>) -> OutputSink {
    OutputSink::new(
        tx.clone(),
        Path::new(at),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        Headers::new(),
        None,
    )
}

fn to(at: &str, body: Value) -> Message {
    MessageBuilder::new(Path::new(at))
        .body(Body::Inline(body))
        .build()
}

fn user_turn() -> Message {
    to(
        "/twin",
        json!({"messages": [{"origin": "user", "type": "text", "text": "ping"}]}),
    )
}

fn drain(rx: &mut mpsc::Receiver<CellEmission>) -> Vec<Value> {
    let mut out = Vec::new();
    while let Ok(em) = rx.try_recv() {
        out.push(em.content);
    }
    out
}

fn asked_for_its_credential(seen: &[Value]) -> bool {
    seen.len() == 1 && seen[0]["header"]["route"] == "credential_request"
}

fn refused(seen: &[Value]) -> bool {
    seen.len() == 1 && seen[0]["header"]["error_code"] == "invalid_input"
}

async fn seed_identity(db: &mut DbConn) {
    db.call(|c| {
        c.execute(
            "INSERT INTO system (slot_path, value, updated_at) \
             VALUES ('identity', '{\"text\":\"seed\"}', 0)",
            [],
        )
        .unwrap()
    })
    .await;
}

async fn rows(db: &mut DbConn, sql: &'static str) -> Vec<(String, String)> {
    db.call(move |c| {
        let mut stmt = c.prepare(sql).unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    })
    .await
}

/// The original's handover: T6-05's export answer, taken as it came.
async fn handover_of_an_original(td: &tempfile::TempDir) -> Value {
    let mut db = db_at(td, "original.db");
    let mut original = cell(json!({
        "provider": "openai", "model": "gpt-4o", "api_key": "k",
        "base_url": "http://127.0.0.1:1/v1",
    }));
    let (tx, mut rx) = mpsc::channel(8);
    let sink = sink("/original", &tx);
    for body in [
        json!({"system": {"identity": {"text": "I am Sam."}}}),
        json!({"params": {"reasoning_effort": "high"}}),
    ] {
        original.handle(to("/original", body), &sink, &mut db).await;
    }
    let ask = MessageBuilder::new(Path::new("/original"))
        .reply_to(Path::new("/twin"))
        .body(Body::Inline(json!({"handover": {"operation": "export"}})))
        .build();
    original.handle(ask, &sink, &mut db).await;
    rx.try_recv().expect("the export answer").content
}

#[tokio::test(start_paused = true)]
async fn a_twin_parks_its_turn_until_the_handover_is_imported() {
    let td = tempfile::TempDir::new().unwrap();
    let handover = handover_of_an_original(&td).await;
    let mut db = db_at(&td, "twin.db");
    // The twin's own seed holds the slot the original overwrote at run time.
    seed_identity(&mut db).await;
    let mut twin = twin();
    let (tx, mut rx) = mpsc::channel(16);
    let sink = sink("/twin", &tx);

    twin.handle(user_turn(), &sink, &mut db).await;
    let parked = drain(&mut rx);
    assert!(parked.is_empty(), "the turn is parked, not run: {parked:?}");

    twin.handle(to("/twin", handover), &sink, &mut db).await;
    let after = drain(&mut rx);
    assert!(
        asked_for_its_credential(&after),
        "the import released the turn, which ran on: {after:?}"
    );
    assert_eq!(
        rows(&mut db, "SELECT slot_path, value FROM system").await,
        vec![("identity".into(), r#"{"text":"I am Sam."}"#.into())],
        "the original's row wins over the twin's seed, on the twin's own connection"
    );
    assert_eq!(
        rows(&mut db, "SELECT key, value FROM params ORDER BY key").await,
        vec![
            ("handover_wait_ms".into(), "0".into()),
            ("reasoning_effort".into(), "\"high\"".into()),
        ],
        "the inherited budget, and the twin waits no more after a wake"
    );
}

#[tokio::test(start_paused = true)]
async fn a_twin_runs_on_without_inheritance_after_its_wait() {
    let td = tempfile::TempDir::new().unwrap();
    let mut db = db_at(&td, "twin.db");
    let mut twin = twin();
    let (tx, mut rx) = mpsc::channel(16);
    let sink = sink("/twin", &tx);
    let t0 = tokio::time::Instant::now();

    twin.handle(user_turn(), &sink, &mut db).await;
    assert!(drain(&mut rx).is_empty(), "the turn is parked");

    let wake = deadline::hang_with("the twin's own wake", rx.recv(), Vec::<Value>::new)
        .await
        .expect("the sink stays open");
    assert_eq!(
        t0.elapsed(),
        std::time::Duration::from_millis(WAIT_MS),
        "the wait is the operation timeout A, exactly"
    );
    assert_eq!(wake.target, Path::new("/twin"), "the twin wakes itself");
    assert!(wake.direct_reply, "past every edge");

    twin.handle(to("/twin", wake.content), &sink, &mut db).await;
    let after = drain(&mut rx);
    assert!(
        asked_for_its_credential(&after),
        "the turn ran on: {after:?}"
    );
    assert!(
        rows(&mut db, "SELECT slot_path, value FROM system")
            .await
            .is_empty(),
        "without the inheritance"
    );
    assert_eq!(
        rows(&mut db, "SELECT key, value FROM params").await,
        vec![("handover_wait_ms".into(), "0".into())],
        "the expiry alone ends the wait for good, so a wake waits no more (review N2)"
    );
}

#[tokio::test(start_paused = true)]
async fn a_refused_budget_still_ends_the_wait_for_good() {
    let td = tempfile::TempDir::new().unwrap();
    let mut handover = handover_of_an_original(&td).await;
    // A budget the twin cannot run on: `max_tokens` is no count.
    handover["handover"]["parts"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|p| p["table"] == "params")
        .expect("the params part")["rows"]
        .as_array_mut()
        .unwrap()
        .push(json!({"key": "max_tokens", "value": "-1", "updated_at": 0}));
    let mut db = db_at(&td, "twin.db");
    let mut twin = twin();
    let (tx, mut rx) = mpsc::channel(16);
    let sink = sink("/twin", &tx);

    twin.handle(user_turn(), &sink, &mut db).await;
    assert!(drain(&mut rx).is_empty(), "the turn is parked");
    twin.handle(to("/twin", handover.clone()), &sink, &mut db)
        .await;
    let after = drain(&mut rx);
    assert!(
        asked_for_its_credential(&after),
        "the import released the turn, which ran on: {after:?}"
    );

    // A wake rebuilds the cell from `cell.db`: it must not wait again, or a
    // second import would write `system` past the SystemGate.
    let mut woken = twin_after_a_wake(&mut db).await;
    woken.handle(to("/twin", handover), &sink, &mut db).await;
    let seen = drain(&mut rx);
    assert!(
        refused(&seen),
        "a second import after a wake (review N1): {seen:?}"
    );
    assert_eq!(
        rows(&mut db, "SELECT key, value FROM params").await,
        vec![("handover_wait_ms".into(), "0".into())],
        "the end of the wait alone, without the refused budget"
    );
}

#[tokio::test]
async fn a_cell_that_does_not_wait_refuses_an_import_and_writes_nothing() {
    let td = tempfile::TempDir::new().unwrap();
    let handover = handover_of_an_original(&td).await;
    let mut db = db_at(&td, "running.db");
    seed_identity(&mut db).await;
    // A running cell, born without `handover_wait_ms`: it waits for nothing.
    let mut running = cell(json!({
        "provider": "openai", "model": "running-model", "api_key": "",
        "base_url": "http://127.0.0.1:1/v1",
    }));
    let (tx, mut rx) = mpsc::channel(16);
    let sink = sink("/running", &tx);

    running
        .handle(to("/running", handover), &sink, &mut db)
        .await;
    let seen = drain(&mut rx);
    assert!(refused(&seen), "an import it does not wait for: {seen:?}");
    assert_eq!(
        rows(&mut db, "SELECT slot_path, value FROM system").await,
        vec![("identity".into(), r#"{"text":"seed"}"#.into())],
        "no system slot changes"
    );
    assert!(
        rows(&mut db, "SELECT key, value FROM params")
            .await
            .is_empty(),
        "no budget is taken either"
    );
}

#[tokio::test(start_paused = true)]
async fn a_malformed_system_row_refuses_the_whole_import() {
    let td = tempfile::TempDir::new().unwrap();
    let handover = handover_of_an_original(&td).await;
    let mut db = db_at(&td, "twin.db");
    let mut twin = twin();
    let (tx, mut rx) = mpsc::channel(16);
    let sink = sink("/twin", &tx);

    twin.handle(user_turn(), &sink, &mut db).await;
    assert!(drain(&mut rx).is_empty(), "the turn is parked");

    // No JSON at all, and JSON that is no leaf object: either row, read back
    // on the next turn, would break the cell's system tree for good.
    for broken in ["x", "\"x\""] {
        let mut bad = handover.clone();
        bad["handover"]["parts"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|p| p["table"] == "system")
            .expect("the system part")["rows"]
            .as_array_mut()
            .unwrap()
            .push(json!({"slot_path": "broken", "value": broken, "updated_at": 0}));
        twin.handle(to("/twin", bad), &sink, &mut db).await;
        let seen = drain(&mut rx);
        assert!(refused(&seen), "a malformed row ({broken}): {seen:?}");
    }
    assert!(
        rows(&mut db, "SELECT slot_path, value FROM system")
            .await
            .is_empty(),
        "nothing written, not even the well-formed identity row"
    );
    assert!(
        rows(&mut db, "SELECT key, value FROM params")
            .await
            .is_empty(),
        "no budget and no end of the wait either"
    );

    // The twin keeps waiting; its wake releases the turn, which runs.
    let wake = deadline::hang_with("the twin's own wake", rx.recv(), Vec::<Value>::new)
        .await
        .expect("the sink stays open");
    twin.handle(to("/twin", wake.content), &sink, &mut db).await;
    let after = drain(&mut rx);
    assert!(
        asked_for_its_credential(&after),
        "the next turn runs: {after:?}"
    );
}

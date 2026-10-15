//! GH #314 — the vault's database does not travel (`contract.transfer: "none"`).
//!
//! The `transfer` body slot is answered by the SUBSTRATE, above every cell type
//! and before `handle()` — so the vault's two-caller ACL, which lives in
//! `handle()`, never sees a transfer. Nothing in the vault's own code could
//! refuse one. What travelled: `vault_secrets` (ciphertext, but `name`,
//! `version`, `status` and `created_at` in plaintext — the complete inventory),
//! `vault_meta` (the salt) and `vault_audit` (the complete call history).
//!
//! The ruling on #314 is a per-type opt-out the cell DECLARES: a contract that
//! says `transfer: "none"` makes its database exempt from the slot, export and
//! import alike, and the refusal carries `error_code: "transfer_exempt"`.
//!
//! This file drives the real seam — a real `VaultCell` on a real `cell.db`,
//! spawned through the substrate's own `build_stateful_task_with_peace`, fed a
//! real `transfer` slot through its mailbox. Both halves are pinned:
//!
//! * a cell whose bounds say `All` (the default, and what an absent declaration
//!   means) transfers exactly as before — that is the negative pin, and it is
//!   what the vault did before this ruling;
//! * a cell whose bounds say `None` is refused, on export and on import, and the
//!   import writes nothing.

use meclaw_cells::vault::store as vault_store;
use meclaw_cells::vault::{VaultCell, VaultParams};
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_colony::{DbConn, build_stateful_task_with_peace};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, Headers, Message, MessageBuilder, OutputSink, Path, Uuid};
use tokio::sync::mpsc;

const VAULT_PATH: &str = "/main/access/vault";
const BROKER: &str = "/main/access/broker";
const CALLER: &str = "/main/access/invoke";
const PASSPHRASE: &str = "a passphrase nobody guesses";

/// A colony that answers exactly one question: who is wired to `of`
/// (GH #160 — the vault holds a declared `NeighbourhoodView`, it does not read
/// `colony.db`). Same fixture as `gh151_the_vault_has_no_way_out`.
fn colony_answering(edges: Vec<(String, String)>) -> mpsc::Sender<meclaw_colony::ColonyMsg> {
    let (tx, mut rx) = mpsc::channel::<meclaw_colony::ColonyMsg>(16);
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let meclaw_colony::ColonyMsg::ReadInboundEdges { of, ack } = msg {
                let mut inbound: Vec<Path> = edges
                    .iter()
                    .filter(|(_, to)| to.as_str() == of.as_str())
                    .map(|(from, _)| Path::new(from))
                    .collect();
                inbound.sort_by(|a, b| a.as_str().cmp(b.as_str()));
                let _ = ack.send(inbound);
            }
        }
    });
    tx
}

/// An unlocked vault holding one real secret, on a real `cell.db`.
async fn vault_holding_a_secret() -> (tempfile::TempDir, VaultCell, DbConn, std::path::PathBuf) {
    let root = tempfile::TempDir::new().unwrap();
    let cell_dir = root.path().join("main").join("access").join("vault");
    std::fs::create_dir_all(&cell_dir).unwrap();
    let db_path = cell_dir.join("cell.db");
    let conn = meclaw_colony::persist::open_or_create_cell_db(&db_path).unwrap();
    vault_store::apply_ddl(&conn).unwrap();

    let colony = colony_answering(vec![
        (BROKER.to_string(), VAULT_PATH.to_string()),
        (VAULT_PATH.to_string(), BROKER.to_string()),
    ]);
    let view = meclaw_colony::NeighbourhoodView::new(Path::new(VAULT_PATH), colony);
    let params = VaultParams::parse(&json!({"broker": BROKER})).unwrap();
    let mut cell = VaultCell::new(params, Some(view));
    let mut db = DbConn::wrap(conn, None);

    direct(
        &mut cell,
        &mut db,
        json!({"op": "unlock", "passphrase": PASSPHRASE}),
    )
    .await;
    direct(
        &mut cell,
        &mut db,
        json!({"op": "put", "name": "telegram_token", "secret": "SUPER-SECRET-TOKEN"}),
    )
    .await;

    (root, cell, db, db_path)
}

/// One user-channel call straight into `handle()` (a source message — no edge
/// can produce one). Used only to fill the vault before the seam is exercised.
async fn direct(cell: &mut VaultCell, db: &mut DbConn, args: Value) {
    let (otx, mut orx) = mpsc::channel(8);
    let sink = OutputSink::new(
        otx,
        Path::new(VAULT_PATH),
        Uuid::now_v7(),
        Uuid::now_v7(),
        64,
        Headers::new(),
        None,
    );
    let msg = MessageBuilder::new(Path::new(VAULT_PATH))
        .body(Body::Inline(json!({"messages":[{
            "origin":"assistant","type":"tool_call","id":"call_1",
            "text": args.to_string()}]})))
        .build();
    cell.handle(msg, &sink, db).await;
    drop(sink);
    let content = orx.recv().await.expect("the vault always answers").content;
    assert!(
        content["header"].get("error_code").is_none(),
        "fixture call {args} must succeed: {content}"
    );
}

/// Send one `transfer` slot through the substrate's own spawn path and return
/// the reply the substrate emits.
///
/// This is the seam under test: `build_stateful_task_with_peace` →
/// `cell_task_stateful` → `db_transfer::handle_transfer_slot`, which runs
/// BEFORE `handle()` and therefore above the vault's own ACL.
async fn transfer(
    cell: VaultCell,
    db: DbConn,
    bounds: meclaw_core::TransferBounds,
    slot: Value,
) -> Value {
    let (mb_tx, mb_rx) = mpsc::channel::<Message>(8);
    let (otx, mut orx) = mpsc::channel::<CellEmission>(8);
    let (inbox_tx, _inbox_rx) = mpsc::channel::<meclaw_colony::ColonyMsg>(8);

    let (join, _peace_rx, _stop_tx, _death_ack_rx, _backstop_rx) = build_stateful_task_with_peace(
        Path::new(VAULT_PATH),
        mb_rx,
        otx,
        inbox_tx,
        None, // idle_timeout
        None, // message_timeout
        0,    // cell_timeout
        cell,
        db,
        None, // blob_store
        None, // consumes
        bounds,
    );

    let msg = MessageBuilder::new(Path::new(VAULT_PATH))
        .body(Body::Inline(json!({ "transfer": slot })))
        .reply_to(Path::new(CALLER))
        .build();
    mb_tx.send(msg).await.expect("mailbox open");

    let reply = tokio::time::timeout(std::time::Duration::from_secs(30), orx.recv())
        .await
        .expect("the substrate must answer a transfer slot within 30s")
        .expect("the substrate must answer a transfer slot")
        .content;

    // Close the mailbox and let the task finish so `cell.db` is closed before
    // the caller reads the file.
    drop(mb_tx);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), join).await;
    reply
}

/// The reply's `tool_result` payload, parsed back from its text.
fn payload(reply: &Value) -> Value {
    let text = reply["messages"][0]["text"].as_str().unwrap_or_default();
    meclaw_core::serde_json::from_str(text).unwrap_or(Value::String(text.to_string()))
}

// ---------------------------------------------------------------------------
// The negative pin: without the declaration, nothing changed.
// ---------------------------------------------------------------------------

/// A cell that declares nothing behaves exactly as it did before #314 — the
/// substrate hands out the inventory of its `cell.db`. This is the state the
/// audit found the vault in, and it stays the default for every cell type:
/// the opt-out is a declaration, never an inference about a type's name.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_the_declaration_a_vault_still_gives_up_its_inventory() {
    let (_root, cell, db, _p) = vault_holding_a_secret().await;
    let reply = transfer(
        cell,
        db,
        meclaw_core::TransferBounds::default(),
        json!({"operation": "export"}),
    )
    .await;

    assert!(
        reply["header"].get("error_code").is_none(),
        "an undeclared cell is not bounded by this ruling: {reply}"
    );
    let tables = payload(&reply)["tables"].clone();
    assert_eq!(
        tables,
        json!(["system", "vault_audit", "vault_meta", "vault_secrets"]),
        "this is what travelled before the declaration existed"
    );
}

// ---------------------------------------------------------------------------
// The ruling: the declared cell is exempt, in both directions.
// ---------------------------------------------------------------------------

/// Export half. The refusal is the whole point: no inventory, no rows, no
/// table names — the caller learns that the cell is exempt and nothing else.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_exempt_vault_refuses_an_export_and_names_nothing() {
    let (_root, cell, db, _p) = vault_holding_a_secret().await;
    let reply = transfer(
        cell,
        db,
        meclaw_core::TransferBounds::exempt(),
        json!({"operation": "export"}),
    )
    .await;

    assert_eq!(reply["header"]["error_code"], "transfer_exempt");
    assert_eq!(reply["header"]["rows_affected"], 0);
    let whole = reply.to_string();
    for leak in [
        "vault_secrets",
        "vault_meta",
        "vault_audit",
        "telegram_token",
    ] {
        assert!(
            !whole.contains(leak),
            "a refusal that names {leak} is an inventory: {reply}"
        );
    }
}

/// Export half, addressed at one named table — the caller does not have to
/// guess, and guessing right must not help either.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_exempt_vault_refuses_a_named_table_too() {
    let (_root, cell, db, _p) = vault_holding_a_secret().await;
    let reply = transfer(
        cell,
        db,
        meclaw_core::TransferBounds::exempt(),
        json!({"operation": "export", "table": "vault_secrets"}),
    )
    .await;

    assert_eq!(reply["header"]["error_code"], "transfer_exempt");
    assert!(
        !reply.to_string().contains("telegram_token"),
        "the inventory must not leak through a named export: {reply}"
    );
}

/// Import half — "a store that cannot leave also cannot be overwritten through
/// the same seam" (the ruling). Proven on the database, not just in the reply:
/// the smuggled row is not there afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_exempt_vault_refuses_an_import_and_writes_nothing() {
    let (_root, cell, db, db_path) = vault_holding_a_secret().await;
    let reply = transfer(
        cell,
        db,
        meclaw_core::TransferBounds::exempt(),
        json!({"operation": "import", "table": "vault_audit",
               "schema": {"at": "int", "op": "text", "actor": "text",
                          "name": "text", "outcome": "text", "reason": "text"},
               "rows": [{"at": 1, "op": "use", "actor": "/nobody",
                         "name": "telegram_token", "outcome": "ok", "reason": ""}]}),
    )
    .await;

    assert_eq!(reply["header"]["error_code"], "transfer_exempt");
    assert_eq!(reply["header"]["rows_affected"], 0);

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let smuggled: i64 = conn
        .query_row(
            "SELECT count(*) FROM vault_audit WHERE actor = '/nobody'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(smuggled, 0, "a refused import must write nothing");
}

// ---------------------------------------------------------------------------
// The declaration itself, as shipped.
// ---------------------------------------------------------------------------

/// The templates that ship a vault declare the exemption. Without this the
/// mechanism exists and nobody uses it — and the cell type's promise
/// (`cell-types.md` § vault) is only true for a hand-edited instance.
///
/// **Conservative by construction, in exactly one half.** `vault` is a public
/// template and is in every checkout, so its declaration is judged
/// unconditionally — an absent `templates/vault/config.json` is a defect here,
/// not a subset. `access` is PRIVATE and does not travel with the published
/// tree, so its half carries the per-file `path.exists()` guard the rest of the
/// suite uses for non-travelling templates (`access_template.rs`): where the
/// file is absent the half is skipped rather than failing on a dead
/// `templates/` reference. The skip is named on stderr and the judged halves
/// are counted against a floor, so "nothing wrong" and "nothing looked at" do
/// not read as the same green.
#[test]
fn every_shipped_vault_declares_the_exemption() {
    /// Public template — present in every tree, judged unconditionally.
    const ALWAYS: &str = "vault/config.json";
    /// Private template — absent from the published subset, guarded.
    const WHERE_IT_SHIPS: &str = "access/vault/config.json";

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("templates");
    let mut judged = 0usize;
    for rel in [ALWAYS, WHERE_IT_SHIPS] {
        let path = root.join(rel);
        if rel == WHERE_IT_SHIPS && !path.exists() {
            eprintln!(
                "[every_shipped_vault_declares_the_exemption] SKIPPED {rel}: `access` is a \
                 private template and does not ship in this tree — 1 of 2 halves judged"
            );
            continue;
        }
        let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel}: {e}"));
        let cfg: Value = meclaw_core::serde_json::from_str(&raw).unwrap();
        assert_eq!(
            cfg["contract"]["transfer"], "none",
            "{rel} must declare its database exempt from the transfer slot"
        );
        judged += 1;
    }
    assert!(
        judged >= 1,
        "not one shipped vault declaration was read — `{ALWAYS}` is public and is in every \
         tree, so a zero count means this check lost its subject, not that the library lost \
         the template"
    );
}

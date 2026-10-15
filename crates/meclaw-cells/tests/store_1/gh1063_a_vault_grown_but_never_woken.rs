//! GH #1063 — the cold deposit: a vault takes its first secret before the
//! colony ever boots.
//!
//! A vault grown by a manifest has a cell directory and a `config.json`, but
//! no `cell.db` until the cell first wakes — and the user channel
//! (`meclaw --vault-add`) refused a vault without one ("no vault at …"). The
//! way round it was to boot once, or to create an empty `cell.db` by hand,
//! which the next boot refused (`CorruptCellDb … no such table: meta`). Now the
//! user channel lays the database down itself — with the schema the cell lays
//! down at its first wake, never a second one — and only where the directory
//! is unmistakably a vault cell.
//!
//! | claim | test |
//! |---|---|
//! | a grown, never-woken vault takes its first secret; a non-vault path still creates nothing | [`gh1063_a_vault_grown_but_never_woken_takes_its_first_secret`] |
//! | the colony boots over that database, and the vault delivers the secret | [`gh1063_the_colony_boots_after_a_cold_deposit_and_delivers_it`] |
//!
//! The lease half (no cold deposit while a colony holds the root) is the
//! CLI's and is pinned in `meclaw-cli/tests/gh1063_*`.

use meclaw_cells::sealed::{RecipientKeypair, SealedBox};
use meclaw_cells::vault::user_channel;
use meclaw_cells::vault::{VaultCell, VaultCellFactory, VaultParams};
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_colony::{CellFactory, CellFactoryRegistry, DbConn, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, Headers, Message, MessageBuilder, OutputSink, Path, Uuid};
use meclaw_testing::ColonyHandle;
use std::sync::Arc;
use tokio::sync::mpsc;

const VAULT_PATH: &str = "/main/vault";
const BROKER: &str = "/main/invoke";
const PASSPHRASE: &[u8] = b"stub-secret-passphrase-1063";
const SECRET: &[u8] = b"stub-secret-1063";

/// A tree with a vault that was grown and never woken: hive + vault config, no
/// `cell.db` anywhere.
fn grown_tree() -> tempfile::TempDir {
    let td = tempfile::TempDir::new().unwrap();
    let main = td.path().join("main");
    std::fs::create_dir_all(main.join("vault")).unwrap();
    std::fs::write(
        main.join("config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    // The shipped vault cell, as a manifest grows it — a complete contract,
    // because the boot below validates it like any other cell.
    let mut cfg: Value =
        meclaw_core::serde_json::from_str(&std::fs::read_to_string(shipped_vault()).unwrap())
            .unwrap();
    cfg["params"]["broker"] = json!(BROKER);
    std::fs::write(main.join("vault").join("config.json"), cfg.to_string()).unwrap();
    td
}

fn shipped_vault() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates/access/vault/config.json")
}

/// Presence guard: the fixture is the shipped template.
fn shipped() -> bool {
    shipped_vault().is_file()
}

#[test]
fn gh1063_a_vault_grown_but_never_woken_takes_its_first_secret() {
    if !shipped() {
        return;
    }
    let td = grown_tree();
    let db = td.path().join("main/vault/cell.db");
    assert!(!db.exists(), "the fixture is a vault that never woke");

    let version = user_channel::add(td.path(), VAULT_PATH, "openrouter", SECRET, PASSPHRASE)
        .expect("the first secret lands in a vault that never woke");
    assert_eq!(version, 1);
    assert!(db.is_file(), "the deposit laid the database down");
    let held = user_channel::status(td.path(), VAULT_PATH).unwrap();
    assert_eq!(held, vec![("openrouter".to_string(), 1)]);

    // A path that is not a vault cell still creates nothing: a typo must not
    // leave an empty vault behind that looks like it worked.
    std::fs::create_dir_all(td.path().join("main/notavault")).unwrap();
    std::fs::write(
        td.path().join("main/notavault/config.json"),
        r#"{"cell":{"type":"store"}}"#,
    )
    .unwrap();
    for path in ["/main/notavault", "/main/nothing-here"] {
        let err = user_channel::add(td.path(), path, "x", SECRET, PASSPHRASE).unwrap_err();
        assert!(err.contains("no vault at"), "{path}: {err}");
    }
    assert!(!td.path().join("main/notavault/cell.db").exists());
    assert!(!td.path().join("main/nothing-here").exists());
}

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

fn call(sender: Option<&str>, args: Value) -> Message {
    let body = json!({"messages":[{
        "origin":"assistant","type":"tool_call","text": args.to_string(),"id":"call_1"
    }]});
    let b = MessageBuilder::new(Path::new(VAULT_PATH)).body(Body::Inline(body));
    match sender {
        Some(s) => b.reply_to(Path::new(s)).build(),
        None => b.build(),
    }
}

async fn run(cell: &mut VaultCell, db: &mut DbConn, msg: Message) -> Value {
    let (otx, mut orx) = mpsc::channel::<CellEmission>(8);
    let sink = OutputSink::new(
        otx,
        Path::new(VAULT_PATH),
        Uuid::now_v7(),
        Uuid::now_v7(),
        64,
        Headers::new(),
        None,
    );
    cell.handle(msg, &sink, db).await;
    drop(sink);
    orx.recv().await.map(|em| em.content).expect("an answer")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1063_the_colony_boots_after_a_cold_deposit_and_delivers_it() {
    if !shipped() {
        return;
    }
    let td = grown_tree();
    user_channel::add(td.path(), VAULT_PATH, "openrouter", SECRET, PASSPHRASE)
        .expect("cold deposit");

    // The boot accepts the database the deposit laid down — the hand-made
    // empty file is exactly what it used to refuse here (`CorruptCellDb`).
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![(
            "vault".to_string(),
            Arc::new(VaultCellFactory) as Arc<dyn CellFactory>,
        )]
    };
    let h = ColonyHandle::new_with_factories_at(&td, factories());
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    let booted = bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .map(|_| ())
        .map_err(|e| format!("{e:?}"));
    assert!(booted.is_ok(), "the colony boots: {booted:?}");
    drop(h);

    // And the vault on that database delivers the deposit, sealed, to its
    // broker — the same cell code, the same `cell.db`.
    let conn =
        meclaw_colony::persist::open_or_create_cell_db(&td.path().join("main/vault/cell.db"))
            .unwrap();
    meclaw_cells::vault::store::apply_ddl(&conn).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let view = meclaw_colony::NeighbourhoodView::new(
        Path::new(VAULT_PATH),
        colony_answering(vec![
            (BROKER.to_string(), VAULT_PATH.to_string()),
            (VAULT_PATH.to_string(), BROKER.to_string()),
        ]),
    );
    let mut cell = VaultCell::new(
        VaultParams::parse(&json!({"broker": BROKER})).unwrap(),
        Some(view),
    );
    let passphrase = String::from_utf8(PASSPHRASE.to_vec()).unwrap();
    let opened = run(
        &mut cell,
        &mut db,
        call(None, json!({"op": "unlock", "passphrase": passphrase})),
    )
    .await;
    assert!(opened["header"]["error_code"].is_null(), "{opened}");
    let me = RecipientKeypair::generate().unwrap();
    let content = run(
        &mut cell,
        &mut db,
        call(
            Some(BROKER),
            json!({"op": "deliver", "name": "openrouter", "grant_id": "g-1063",
                   "recipient_key": me.public_hex()}),
        ),
    )
    .await;
    assert!(content["header"]["error_code"].is_null(), "{content}");
    let text = content["messages"][0]["text"].as_str().unwrap();
    let result: Value = meclaw_core::serde_json::from_str(text).unwrap();
    let sealed = SealedBox::from_json(&result["sealed"]).unwrap();
    assert_eq!(me.open(&sealed).unwrap(), SECRET.to_vec());
}

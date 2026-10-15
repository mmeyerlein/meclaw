//! GH #1058 (OR-VG-6) — a vault opens itself from its configured key source.
//!
//! A vault inside a sealed access hive is reachable by its broker alone, and
//! `unlock` is user-channel-only, so a woken vault stays locked unless it can
//! open itself. Until now only `params.unlock_env` (a passphrase in the
//! process environment) did that. The systemd credential is the better home
//! for the one secret that opens all others — `LoadCredentialEncrypted=` keeps
//! it off the disk in the clear — and the vault already READ it
//! (`key_source: "systemd-cred"`, `$CREDENTIALS_DIRECTORY/<credential_name>`);
//! what it lacked was the self-unlock from it.
//!
//! Now an EXPLICIT `key_source: "systemd-cred"` or `"plainfile"` opens the
//! vault on the first operation that needs a key. `auto` does not: "a woken
//! vault is locked" stays the default, and an operator opts in by naming the
//! source.
//!
//! The fixture helpers are a deliberate copy of `gh421_sealed_delivery.rs`'s;
//! the files pin different promises and must be able to move independently.

use meclaw_cells::sealed::{RecipientKeypair, SealedBox};
use meclaw_cells::vault::store as vault_store;
use meclaw_cells::vault::{VaultCell, VaultParams};
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, Headers, Message, MessageBuilder, OutputSink, Path, Uuid};
use std::os::unix::fs::PermissionsExt;
use tokio::sync::mpsc;

const VAULT_PATH: &str = "/main/access/vault";
const BROKER: &str = "/main/access/broker";
const PASSPHRASE: &str = "stub-secret-passphrase-1058";
const SECRET: &str = "stub-secret-8";

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

/// A vault on a real `cell.db` with the sealed topology (broker in, vault out)
/// and the given params on top of `broker`.
fn vault(params: Value) -> (tempfile::TempDir, VaultCell, DbConn) {
    let root = tempfile::TempDir::new().unwrap();
    let cell_dir = root.path().join("main").join("access").join("vault");
    std::fs::create_dir_all(&cell_dir).unwrap();
    let conn = meclaw_colony::persist::open_or_create_cell_db(&cell_dir.join("cell.db")).unwrap();
    vault_store::apply_ddl(&conn).unwrap();
    let colony = colony_answering(vec![
        (BROKER.to_string(), VAULT_PATH.to_string()),
        (VAULT_PATH.to_string(), BROKER.to_string()),
    ]);
    let view = meclaw_colony::NeighbourhoodView::new(Path::new(VAULT_PATH), colony);
    let mut raw = json!({"broker": BROKER});
    for (k, v) in params.as_object().unwrap() {
        raw[k] = v.clone();
    }
    let params = VaultParams::parse(&raw).unwrap();
    (
        root,
        VaultCell::new(params, Some(view)),
        DbConn::wrap(conn, None),
    )
}

fn sink_pair() -> (OutputSink, mpsc::Receiver<CellEmission>) {
    let (otx, orx) = mpsc::channel(8);
    let sink = OutputSink::new(
        otx,
        Path::new(VAULT_PATH),
        Uuid::now_v7(),
        Uuid::now_v7(),
        64,
        Headers::new(),
        None,
    );
    (sink, orx)
}

/// `sender = None` is the user channel.
fn call(sender: Option<&str>, args: Value) -> Message {
    let body = json!({"messages":[{
        "origin":"assistant","type":"tool_call",
        "text": args.to_string(),
        "id":"call_1"
    }]});
    let b = MessageBuilder::new(Path::new(VAULT_PATH)).body(Body::Inline(body));
    match sender {
        Some(s) => b.reply_to(Path::new(s)).build(),
        None => b.build(),
    }
}

async fn run(cell: &mut VaultCell, db: &mut DbConn, msg: Message) -> Value {
    let (sink, mut orx) = sink_pair();
    cell.handle(msg, &sink, db).await;
    drop(sink);
    orx.recv()
        .await
        .map(|em| em.content)
        .expect("the vault always answers")
}

fn result(content: &Value) -> Value {
    let text = content["messages"][0]["text"]
        .as_str()
        .expect("result text");
    meclaw_core::serde_json::from_str(text).unwrap_or(Value::String(text.to_string()))
}

/// Fill the vault over the user channel and lock it again — the state of a
/// vault that was filled once and has just woken up.
async fn fill_and_lock(cell: &mut VaultCell, db: &mut DbConn) {
    let r = run(
        cell,
        db,
        call(None, json!({"op": "unlock", "passphrase": PASSPHRASE})),
    )
    .await;
    assert!(r["header"]["error_code"].is_null(), "unlock: {r}");
    let r = run(
        cell,
        db,
        call(
            None,
            json!({"op": "put", "name": "openrouter", "secret": SECRET}),
        ),
    )
    .await;
    assert!(r["header"]["error_code"].is_null(), "put: {r}");
    run(cell, db, call(None, json!({"op": "lock"}))).await;
}

/// The broker asks for a delivery, sealed to a fresh recipient.
async fn deliver(cell: &mut VaultCell, db: &mut DbConn) -> (Value, RecipientKeypair) {
    let me = RecipientKeypair::generate().expect("keypair");
    let content = run(
        cell,
        db,
        call(
            Some(BROKER),
            json!({"op": "deliver", "name": "openrouter", "grant_id": "g-1058",
                   "recipient_key": me.public_hex()}),
        ),
    )
    .await;
    (content, me)
}

/// A key file the way systemd hands one out: readable by the owner only (the
/// vault refuses a loose one, like ssh), with a trailing newline from `echo`.
fn key_file(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, format!("{PASSPHRASE}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    path
}

/// T8. One test, three cases in sequence, because all three set the same
/// process-wide variable: `set_var` is `unsafe` in edition 2024 because a
/// concurrent `getenv` in another thread is a data race. It is sound here
/// because nextest runs every test in its own process, and inside this one
/// test the variable is written only between operations — no vault is reading
/// it while it changes (each `run` has returned before the next write).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gh1058_the_vault_unlocks_from_a_systemd_credential() {
    let creds = tempfile::TempDir::new().unwrap();
    key_file(creds.path(), "vault_key");

    // (a) An explicit systemd-cred source opens the woken vault on the first
    // delivery — no unlock message, no passphrase on any wire.
    unsafe { std::env::set_var("CREDENTIALS_DIRECTORY", creds.path()) };
    let (_r, mut cell, mut db) = vault(json!({"key_source": "systemd-cred"}));
    fill_and_lock(&mut cell, &mut db).await;
    let (content, me) = deliver(&mut cell, &mut db).await;
    assert!(
        content["header"]["error_code"].is_null(),
        "the vault opened itself from the credential: {content}"
    );
    let sealed = SealedBox::from_json(&result(&content)["sealed"]).expect("a box came back");
    assert_eq!(me.open(&sealed).expect("open"), SECRET.as_bytes().to_vec());
    assert!(
        !content.to_string().contains(SECRET),
        "nothing in the clear"
    );
    assert!(
        !content.to_string().contains(PASSPHRASE),
        "nor the passphrase"
    );

    // (b) The credential is missing: the vault stays locked and says why, by
    // name — an operator who chose this source wants to know it is empty.
    let empty = tempfile::TempDir::new().unwrap();
    unsafe { std::env::set_var("CREDENTIALS_DIRECTORY", empty.path()) };
    let (_r, mut cell, mut db) = vault(json!({"key_source": "systemd-cred"}));
    fill_and_lock(&mut cell, &mut db).await;
    let (content, _) = deliver(&mut cell, &mut db).await;
    assert_eq!(
        content["header"]["error_code"], "invalid_input",
        "{content}"
    );
    let text = content["messages"][0]["text"].as_str().unwrap_or_default();
    assert!(
        text.contains("systemd-cred") && text.contains("vault_key"),
        "the refusal names the source and the credential: {text}"
    );
    assert!(!text.contains(PASSPHRASE), "{text}");
    let status = run(&mut cell, &mut db, call(None, json!({"op": "status"}))).await;
    assert_eq!(result(&status)["locked"], true, "still locked: {status}");

    // (c) `auto` does not open itself, even with a credential right there:
    // "a woken vault is locked" stays the default.
    unsafe { std::env::set_var("CREDENTIALS_DIRECTORY", creds.path()) };
    let (_r, mut cell, mut db) = vault(json!({"key_source": "auto"}));
    fill_and_lock(&mut cell, &mut db).await;
    let (content, _) = deliver(&mut cell, &mut db).await;
    assert_eq!(content["header"]["error_code"], "vault_locked", "{content}");

    unsafe { std::env::remove_var("CREDENTIALS_DIRECTORY") };
}

/// The start.sh path (R-VG-7 b): an explicit `plainfile` source opens the
/// vault the same way, from a 0600 file beside the colony.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gh1058_the_vault_unlocks_from_a_plain_key_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = key_file(dir.path(), "vault-pass.txt");
    let (_r, mut cell, mut db) = vault(json!({
        "key_source": "plainfile", "key_file": path.to_string_lossy()
    }));
    fill_and_lock(&mut cell, &mut db).await;
    let (content, me) = deliver(&mut cell, &mut db).await;
    assert!(content["header"]["error_code"].is_null(), "{content}");
    let sealed = SealedBox::from_json(&result(&content)["sealed"]).expect("a box came back");
    assert_eq!(me.open(&sealed).expect("open"), SECRET.as_bytes().to_vec());
}

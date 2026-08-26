//! The vault's eighth operation, driven through its real route surface (GH #421).
//!
//! `deliver` is the sanctioned extension of a sealed cell type (sanctioned ruling
//! R3): the one operation whose answer contains the credential, and it contains
//! it under a key only the requester holds. What this file pins is that the
//! answer never carries a plaintext, that the ACL treats a delivery as a SPEND,
//! and that a vault whose broker is not wired in refuses to unlock at all.
//!
//! The fixture helpers below are a deliberate copy of the ones in
//! `gh151_the_vault_has_no_way_out.rs`. The two files pin different promises
//! and must be able to move independently.

use meclaw_cells::sealed::{RecipientKeypair, SealedBox};
use meclaw_cells::vault::store as vault_store;
use meclaw_cells::vault::{VaultCell, VaultParams};
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, Headers, Message, MessageBuilder, OutputSink, Path, Uuid};
use tokio::sync::mpsc;

const VAULT_PATH: &str = "/main/access/vault";
const BROKER: &str = "/main/access/broker";
const PASSPHRASE: &str = "a passphrase nobody guesses";

/// A colony that answers exactly one question: who is wired to `of`.
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

/// A vault on a real `cell.db`, holding a neighbourhood view onto an edge table.
async fn vault_with_sealed_topology(
    inbound: &[(&str, &str)],
) -> (tempfile::TempDir, VaultCell, DbConn) {
    let root = tempfile::TempDir::new().unwrap();
    let cell_dir = root.path().join("main").join("access").join("vault");
    std::fs::create_dir_all(&cell_dir).unwrap();
    let conn = meclaw_colony::persist::open_or_create_cell_db(&cell_dir.join("cell.db")).unwrap();
    vault_store::apply_ddl(&conn).unwrap();

    let colony = colony_answering(
        inbound
            .iter()
            .map(|(f, t)| ((*f).to_string(), (*t).to_string()))
            .collect(),
    );
    let view = meclaw_colony::NeighbourhoodView::new(Path::new(VAULT_PATH), colony);

    let params = VaultParams::parse(&json!({"broker": BROKER})).unwrap();
    (
        root,
        VaultCell::new(params, Some(view)),
        DbConn::wrap(conn, None),
    )
}

/// The default topology: broker in, vault out. Sealed and correct.
async fn sealed_vault() -> (tempfile::TempDir, VaultCell, DbConn) {
    vault_with_sealed_topology(&[(BROKER, VAULT_PATH), (VAULT_PATH, BROKER)]).await
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

/// `sender = None` is the user channel — a source message, which no edge can
/// produce because the colony stamps `reply_to` on everything a cell emits.
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

/// The `tool_result` payload, parsed back from its text.
fn result(content: &Value) -> Value {
    let text = content["messages"][0]["text"]
        .as_str()
        .expect("result text");
    serde_json::from_str(text).unwrap_or(Value::String(text.to_string()))
}

async fn unlock(cell: &mut VaultCell, db: &mut DbConn) -> Value {
    run(
        cell,
        db,
        call(None, json!({"op": "unlock", "passphrase": PASSPHRASE})),
    )
    .await
}

async fn put(cell: &mut VaultCell, db: &mut DbConn, name: &str, secret: &str) -> Value {
    run(
        cell,
        db,
        call(None, json!({"op": "put", "name": name, "secret": secret})),
    )
    .await
}

// ---------------------------------------------------------------------------
// The claim
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_broker_gets_the_credential_sealed_and_the_wire_never_carries_it() {
    let (_root, mut cell, mut db) = sealed_vault().await;
    unlock(&mut cell, &mut db).await;
    put(&mut cell, &mut db, "openrouter", "SUPER-SECRET-TOKEN").await;

    let me = RecipientKeypair::generate().expect("keypair");
    let content = run(
        &mut cell,
        &mut db,
        call(
            Some(BROKER),
            json!({
                "op": "deliver", "name": "openrouter", "grant_id": "g-1",
                "recipient_key": me.public_hex(),
            }),
        ),
    )
    .await;

    // Nothing on the wire, header included, is the secret.
    assert!(
        !content.to_string().contains("SUPER-SECRET-TOKEN"),
        "{content}"
    );

    let r = result(&content);
    assert_eq!(r["name"], "openrouter");
    assert_eq!(r["version"], 1);
    assert_eq!(r["grant_id"], "g-1");
    let sealed = SealedBox::from_json(&r["sealed"]).expect("a sealed box came back");
    assert_eq!(
        me.open(&sealed).expect("open"),
        b"SUPER-SECRET-TOKEN".to_vec()
    );
}

// ---------------------------------------------------------------------------
// The refusals — a delivery is a SPEND and is guarded like one
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_the_broker_may_ask_for_a_delivery() {
    let (_root, mut cell, mut db) = sealed_vault().await;
    unlock(&mut cell, &mut db).await;
    put(&mut cell, &mut db, "openrouter", "SUPER-SECRET-TOKEN").await;
    let me = RecipientKeypair::generate().expect("keypair");
    let ask = json!({"op":"deliver","name":"openrouter","grant_id":"g",
                     "recipient_key": me.public_hex()});

    for sender in [None, Some("/main/egon/brain")] {
        let content = run(&mut cell, &mut db, call(sender, ask.clone())).await;
        assert_eq!(
            content["header"]["error_code"], "access_denied",
            "a delivery is a SPEND — only the broker spends: {content}"
        );
        assert!(
            !content.to_string().contains("SUPER-SECRET-TOKEN"),
            "{content}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_locked_vault_delivers_nothing() {
    let (_root, mut cell, mut db) = sealed_vault().await;
    unlock(&mut cell, &mut db).await;
    put(&mut cell, &mut db, "openrouter", "SUPER-SECRET-TOKEN").await;
    run(&mut cell, &mut db, call(None, json!({"op":"lock"}))).await;

    let me = RecipientKeypair::generate().expect("keypair");
    let content = run(
        &mut cell,
        &mut db,
        call(
            Some(BROKER),
            json!({"op":"deliver","name":"openrouter","grant_id":"g",
                   "recipient_key": me.public_hex()}),
        ),
    )
    .await;
    assert_eq!(content["header"]["error_code"], "vault_locked");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_revoked_or_unknown_name_is_unknown_secret_and_a_bad_key_is_invalid_input() {
    let (_root, mut cell, mut db) = sealed_vault().await;
    unlock(&mut cell, &mut db).await;
    put(&mut cell, &mut db, "openrouter", "SUPER-SECRET-TOKEN").await;
    let me = RecipientKeypair::generate().expect("keypair");

    let unknown = run(
        &mut cell,
        &mut db,
        call(
            Some(BROKER),
            json!({"op":"deliver","name":"nope","grant_id":"g",
                   "recipient_key": me.public_hex()}),
        ),
    )
    .await;
    assert_eq!(unknown["header"]["error_code"], "unknown_secret");

    // The bad keys are asked BEFORE the revoke, deliberately: `deliver` looks
    // the secret up before it seals, exactly like `use` does, so a bad key on a
    // name that no longer resolves would report the missing name and say
    // nothing about the key. Asking against a live secret is what isolates the
    // property this loop is here for. An empty string never reaches the store
    // at all — `arg()` treats empty as absent.
    for bad in ["", "zz", "abcd"] {
        let content = run(
            &mut cell,
            &mut db,
            call(
                Some(BROKER),
                json!({"op":"deliver","name":"openrouter","grant_id":"g",
                       "recipient_key": bad}),
            ),
        )
        .await;
        assert_eq!(
            content["header"]["error_code"], "invalid_input",
            "key {bad:?}: {content}"
        );
        assert!(
            !content.to_string().contains("SUPER-SECRET-TOKEN"),
            "a refused key must not leak the value it was refused for: {content}"
        );
    }

    run(
        &mut cell,
        &mut db,
        call(None, json!({"op":"revoke","name":"openrouter"})),
    )
    .await;
    let revoked = run(
        &mut cell,
        &mut db,
        call(
            Some(BROKER),
            json!({"op":"deliver","name":"openrouter","grant_id":"g",
                   "recipient_key": me.public_hex()}),
        ),
    )
    .await;
    assert_eq!(revoked["header"]["error_code"], "unknown_secret");
}

/// A delivery and a refusal both have to be answerable from rows: "why did the
/// vault not send" is an operator question, and the audit trail is where it is
/// answered. The trail names the SECRET, never a value.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_delivery_and_every_refusal_lands_in_the_audit_trail() {
    let (_root, mut cell, mut db) = sealed_vault().await;
    unlock(&mut cell, &mut db).await;
    put(&mut cell, &mut db, "openrouter", "SUPER-SECRET-TOKEN").await;
    let me = RecipientKeypair::generate().expect("keypair");
    let ask = json!({"op":"deliver","name":"openrouter","grant_id":"g-7",
                     "recipient_key": me.public_hex()});

    run(&mut cell, &mut db, call(Some(BROKER), ask.clone())).await;
    run(&mut cell, &mut db, call(Some("/main/egon/brain"), ask)).await;

    let rows: Vec<(String, String, Option<String>, String)> = db
        .call(|c| {
            let mut s = c
                .prepare(
                    "SELECT op, actor, name, outcome FROM vault_audit \
                     WHERE op = 'deliver' ORDER BY id",
                )
                .unwrap();
            s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                .unwrap()
                .map(Result::unwrap)
                .collect::<Vec<_>>()
        })
        .await;
    assert_eq!(rows.len(), 2, "both the delivery and the refusal are rows");
    assert_eq!(
        rows[0],
        (
            "deliver".into(),
            "broker".into(),
            Some("openrouter".into()),
            "ok".into()
        )
    );
    assert_eq!(rows[1].1, "/main/egon/brain");
    assert_eq!(rows[1].3, "refused");
}

/// GH #421 / R3 regression lock for the stricter attestation: no broker edge,
/// no authenticity source, no key. This is the behaviour-changing half of the
/// sanctioned extension — before GH #421 a vault nobody could reach unlocked
/// happily, which was correct only while no edge reached the vault at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_vault_nobody_can_reach_stays_locked() {
    let (_root, mut cell, mut db) = vault_with_sealed_topology(&[]).await;
    let content = unlock(&mut cell, &mut db).await;
    assert_eq!(content["header"]["error_code"], "attestation_failed");
    assert!(
        content["messages"][0]["text"]
            .as_str()
            .unwrap()
            .contains("broker_unwired"),
        "{content}"
    );
    let status = result(&run(&mut cell, &mut db, call(None, json!({"op":"status"}))).await);
    assert_eq!(status["locked"], true);
}

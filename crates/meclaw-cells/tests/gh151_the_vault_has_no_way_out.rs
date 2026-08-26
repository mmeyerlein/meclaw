//! The vault, driven through its real route surface (GitHub #151).
//!
//! The claim this file has to hold up is narrow and absolute: **there is no
//! operation that returns a secret**. Everything else the vault does — the
//! sender ACL, the unlock attestation, the audit trail — exists to keep that
//! claim from being routed around, and each of those has its own test below.
//!
//! What is deliberately NOT claimed here: that a compromised process cannot
//! read the key out of memory while the vault is unlocked. It can. The designed
//! answer to that is placement (own process, own user), which is a deployment
//! property and changes no edge — see `templates/access/README.md`.

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
///
/// GH #160: the vault no longer reads `colony.db` — it holds a declared,
/// self-scoped `NeighbourhoodView` and asks the authority. So the fixture is an
/// edge table plus a responder, and the responder filters by `to` exactly as the
/// colony does, which keeps the "an outbound edge is not a neighbour" property
/// under test rather than assumed. The task lives as long as the sender does.
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

/// A vault on a real `cell.db`, holding a neighbourhood view onto an edge table
/// that says only the broker is wired to it — the sealed contract the unlock
/// attests against.
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
async fn there_is_no_operation_that_hands_a_secret_back() {
    let (_root, mut cell, mut db) = sealed_vault().await;
    unlock(&mut cell, &mut db).await;
    put(&mut cell, &mut db, "tg", "SUPER-SECRET-TOKEN").await;

    // Every name somebody might reach for, from both callers.
    for op in ["get", "read", "export", "reveal", "show", "select", "dump"] {
        for sender in [None, Some(BROKER)] {
            let content = run(
                &mut cell,
                &mut db,
                call(sender, json!({"op": op, "name": "tg"})),
            )
            .await;
            let code = content["header"]["error_code"].as_str().unwrap_or("");
            assert!(
                code == "unknown_op" || code == "access_denied",
                "op {op:?} from {sender:?} must be refused, got {content}"
            );
            assert!(
                !content.to_string().contains("SUPER-SECRET-TOKEN"),
                "op {op:?} leaked the secret: {content}"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_secret_can_be_used_without_ever_being_returned() {
    let (_root, mut cell, mut db) = sealed_vault().await;
    unlock(&mut cell, &mut db).await;
    put(&mut cell, &mut db, "tg", "SUPER-SECRET-TOKEN").await;

    let content = run(
        &mut cell,
        &mut db,
        call(
            Some(BROKER),
            json!({"op":"use","use":"sign","name":"tg","payload":"nonce-42","grant_id":"g-1"}),
        ),
    )
    .await;
    let r = result(&content);
    assert_eq!(r["name"], "tg");
    assert_eq!(r["grant_id"], "g-1");
    let sig = r["signature"].as_str().expect("a signature came back");
    assert_eq!(sig.len(), 64, "HMAC-SHA256, hex");
    assert!(
        !content.to_string().contains("SUPER-SECRET-TOKEN"),
        "the payload must carry the result of the secret, never the secret"
    );

    // Deterministic: the same payload under the same secret signs the same.
    let again = run(
        &mut cell,
        &mut db,
        call(
            Some(BROKER),
            json!({"op":"use","use":"sign","name":"tg","payload":"nonce-42","grant_id":"g-2"}),
        ),
    )
    .await;
    assert_eq!(result(&again)["signature"], sig);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_reports_metadata_and_nothing_else() {
    let (_root, mut cell, mut db) = sealed_vault().await;
    let locked = result(&run(&mut cell, &mut db, call(None, json!({"op":"status"}))).await);
    assert_eq!(locked["locked"], true);
    assert_eq!(locked["key_id"], Value::Null, "a locked vault holds no key");

    unlock(&mut cell, &mut db).await;
    put(&mut cell, &mut db, "tg", "SUPER-SECRET-TOKEN").await;

    let open = result(&run(&mut cell, &mut db, call(None, json!({"op":"status"}))).await);
    assert_eq!(open["locked"], false);
    assert!(open["key_id"].is_string(), "the fingerprint, not the key");
    assert_eq!(open["secrets"][0]["name"], "tg");
    assert_eq!(open["secrets"][0]["version"], 1);
    assert!(!open.to_string().contains("SUPER-SECRET-TOKEN"));
    assert!(!open.to_string().contains(PASSPHRASE));
}

// ---------------------------------------------------------------------------
// Who may do what
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_secret_enters_over_the_user_channel_and_nowhere_else() {
    let (_root, mut cell, mut db) = sealed_vault().await;
    unlock(&mut cell, &mut db).await;

    // The broker is the most privileged sender there is, and it still cannot
    // plant a credential — otherwise an agent that captured the broker could
    // swap the vault's contents for its own.
    let refused = run(
        &mut cell,
        &mut db,
        call(
            Some(BROKER),
            json!({"op":"put","name":"tg","secret":"planted"}),
        ),
    )
    .await;
    assert_eq!(refused["header"]["error_code"], "access_denied");

    let stored: i64 = db
        .call(|c| {
            c.query_row("SELECT COUNT(*) FROM vault_secrets", [], |r| r.get(0))
                .unwrap()
        })
        .await;
    assert_eq!(stored, 0, "nothing was written");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn any_other_sender_is_refused_before_the_operation_is_even_considered() {
    let (_root, mut cell, mut db) = sealed_vault().await;
    unlock(&mut cell, &mut db).await;
    put(&mut cell, &mut db, "tg", "SUPER-SECRET-TOKEN").await;

    for op in ["status", "use", "put", "unlock", "lock", "revoke", "rotate"] {
        let content = run(
            &mut cell,
            &mut db,
            call(
                Some("/main/egon/brain"),
                json!({"op":op,"name":"tg","secret":"x","payload":"p","grant_id":"g"}),
            ),
        )
        .await;
        assert_eq!(
            content["header"]["error_code"], "access_denied",
            "a foreign sender must not reach {op}: {content}"
        );
    }

    // …and the vault still holds what it held.
    let open = result(&run(&mut cell, &mut db, call(None, json!({"op":"status"}))).await);
    assert_eq!(open["secrets"][0]["name"], "tg");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_refusal_lands_in_the_audit_trail() {
    let (_root, mut cell, mut db) = sealed_vault().await;
    run(
        &mut cell,
        &mut db,
        call(
            Some("/main/egon/brain"),
            json!({"op":"use","name":"tg","payload":"p","grant_id":"g"}),
        ),
    )
    .await;

    let rows: Vec<(String, String, String, Option<String>)> = db
        .call(|c| {
            let mut s = c
                .prepare("SELECT op, actor, outcome, reason FROM vault_audit ORDER BY id")
                .unwrap();
            s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                .unwrap()
                .map(Result::unwrap)
                .collect::<Vec<_>>()
        })
        .await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "use");
    assert_eq!(rows[0].1, "/main/egon/brain");
    assert_eq!(rows[0].2, "refused");
    assert_eq!(rows[0].3.as_deref(), Some("sender_not_broker"));
}

// ---------------------------------------------------------------------------
// Locked means locked
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_locked_vault_signs_nothing() {
    let (_root, mut cell, mut db) = sealed_vault().await;
    unlock(&mut cell, &mut db).await;
    put(&mut cell, &mut db, "tg", "SUPER-SECRET-TOKEN").await;
    run(&mut cell, &mut db, call(None, json!({"op":"lock"}))).await;

    let content = run(
        &mut cell,
        &mut db,
        call(
            Some(BROKER),
            json!({"op":"use","use":"sign","name":"tg","payload":"p","grant_id":"g"}),
        ),
    )
    .await;
    assert_eq!(content["header"]["error_code"], "vault_locked");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wrong_passphrase_is_refused_at_unlock_rather_than_at_first_use() {
    let (_root, mut cell, mut db) = sealed_vault().await;
    unlock(&mut cell, &mut db).await;
    put(&mut cell, &mut db, "tg", "SUPER-SECRET-TOKEN").await;
    run(&mut cell, &mut db, call(None, json!({"op":"lock"}))).await;

    let content = run(
        &mut cell,
        &mut db,
        call(None, json!({"op":"unlock","passphrase":"not it"})),
    )
    .await;
    assert_eq!(content["header"]["error_code"], "vault_error");
    assert!(
        content["messages"][0]["text"]
            .as_str()
            .unwrap()
            .contains("does not open this vault")
    );
    let after = result(&run(&mut cell, &mut db, call(None, json!({"op":"status"}))).await);
    assert_eq!(after["locked"], true, "a failed unlock leaves it locked");
}

// ---------------------------------------------------------------------------
// The attestation — the reason a reboot cannot launder the gate
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_edge_that_no_mutation_would_have_allowed_keeps_the_vault_locked() {
    // The birth topology is exempt from the port boundary by design. So the
    // attack is: rewrite the tree on disk, let the next boot wire an agent
    // straight into the vault, and the gate has been laundered through a
    // reboot. It has not — because the key never arrives.
    let (_root, mut cell, mut db) =
        vault_with_sealed_topology(&[(BROKER, VAULT_PATH), ("/main/egon/brain", VAULT_PATH)]).await;

    let content = unlock(&mut cell, &mut db).await;
    assert_eq!(content["header"]["error_code"], "attestation_failed");
    let text = content["messages"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("/main/egon/brain"),
        "names the intruder: {text}"
    );

    let status = result(&run(&mut cell, &mut db, call(None, json!({"op":"status"}))).await);
    assert_eq!(status["locked"], true);

    // And nothing can be stored into it either.
    let put_content = put(&mut cell, &mut db, "tg", "SUPER-SECRET-TOKEN").await;
    assert_eq!(put_content["header"]["error_code"], "vault_locked");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_undeclared_vault_fails_closed() {
    // No capability at all: the cell's contract does not declare
    // `consumes.topology.inbound_edges`, so it cannot learn who is wired to it
    // and therefore never takes the key. Before GH #160 the same case was "no
    // colony.db above the cell"; the verdict is identical, which is the point.
    let root = tempfile::TempDir::new().unwrap();
    let cell_dir = root.path().join("vault");
    std::fs::create_dir_all(&cell_dir).unwrap();
    let conn = meclaw_colony::persist::open_or_create_cell_db(&cell_dir.join("cell.db")).unwrap();
    vault_store::apply_ddl(&conn).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let mut cell = VaultCell::new(
        VaultParams::parse(&json!({"broker": BROKER})).unwrap(),
        None,
    );

    let content = unlock(&mut cell, &mut db).await;
    assert_eq!(content["header"]["error_code"], "attestation_failed");
}

/// And a colony that is there but does not answer is the same verdict: the ask
/// is bounded by the vault's own operation timeout, and an unanswered
/// neighbourhood is treated exactly like a wrong one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_silent_colony_fails_closed() {
    let root = tempfile::TempDir::new().unwrap();
    let cell_dir = root.path().join("vault");
    std::fs::create_dir_all(&cell_dir).unwrap();
    let conn = meclaw_colony::persist::open_or_create_cell_db(&cell_dir.join("cell.db")).unwrap();
    vault_store::apply_ddl(&conn).unwrap();
    let mut db = DbConn::wrap(conn, None);
    // A channel nobody reads: the send succeeds, the answer never comes.
    let (colony, _held_open) = mpsc::channel::<meclaw_colony::ColonyMsg>(4);
    let mut cell = VaultCell::new(
        VaultParams::parse(&json!({"broker": BROKER, "external_timeout_ms": 200})).unwrap(),
        Some(meclaw_colony::NeighbourhoodView::new(
            Path::new(VAULT_PATH),
            colony,
        )),
    );

    let content = unlock(&mut cell, &mut db).await;
    assert_eq!(content["header"]["error_code"], "attestation_failed");
    let text = content["messages"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("unverifiable"),
        "the reason must say it could not verify, not that it was wrong: {text}"
    );
}

// ---------------------------------------------------------------------------
// No-delete, the way the rest of the substrate means it
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rotation_appends_and_use_reaches_the_newest_version() {
    let (_root, mut cell, mut db) = sealed_vault().await;
    unlock(&mut cell, &mut db).await;
    put(&mut cell, &mut db, "tg", "first").await;
    let rotated = result(
        &run(
            &mut cell,
            &mut db,
            call(None, json!({"op":"rotate","name":"tg","secret":"second"})),
        )
        .await,
    );
    assert_eq!(rotated["version"], 2);

    let used = result(
        &run(
            &mut cell,
            &mut db,
            call(
                Some(BROKER),
                json!({"op":"use","use":"sign","name":"tg","payload":"p","grant_id":"g"}),
            ),
        )
        .await,
    );
    assert_eq!(used["version"], 2, "use reaches the newest active version");

    let kept: i64 = db
        .call(|c| {
            c.query_row("SELECT COUNT(*) FROM vault_secrets", [], |r| r.get(0))
                .unwrap()
        })
        .await;
    assert_eq!(kept, 2, "the superseded version stays on disk");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_revoked_secret_cannot_be_used_and_says_so_by_name() {
    let (_root, mut cell, mut db) = sealed_vault().await;
    unlock(&mut cell, &mut db).await;
    put(&mut cell, &mut db, "tg", "SUPER-SECRET-TOKEN").await;
    let revoked = result(
        &run(
            &mut cell,
            &mut db,
            call(Some(BROKER), json!({"op":"revoke","name":"tg"})),
        )
        .await,
    );
    assert_eq!(revoked["revoked_versions"], 1);

    let content = run(
        &mut cell,
        &mut db,
        call(
            Some(BROKER),
            json!({"op":"use","use":"sign","name":"tg","payload":"p","grant_id":"g"}),
        ),
    )
    .await;
    assert_eq!(content["header"]["error_code"], "unknown_secret");
}

// ---------------------------------------------------------------------------
// The wire contract
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_use_without_a_grant_id_is_refused() {
    let (_root, mut cell, mut db) = sealed_vault().await;
    unlock(&mut cell, &mut db).await;
    put(&mut cell, &mut db, "tg", "SUPER-SECRET-TOKEN").await;

    let content = run(
        &mut cell,
        &mut db,
        call(
            Some(BROKER),
            json!({"op":"use","use":"sign","name":"tg","payload":"p"}),
        ),
    )
    .await;
    assert_eq!(content["header"]["error_code"], "invalid_input");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_use_action_is_refused_rather_than_falling_back_to_something() {
    let (_root, mut cell, mut db) = sealed_vault().await;
    unlock(&mut cell, &mut db).await;
    put(&mut cell, &mut db, "tg", "SUPER-SECRET-TOKEN").await;

    let content = run(
        &mut cell,
        &mut db,
        call(
            Some(BROKER),
            json!({"op":"use","use":"return_it","name":"tg","payload":"p","grant_id":"g"}),
        ),
    )
    .await;
    assert_eq!(content["header"]["error_code"], "vault_error");
    assert!(!content.to_string().contains("SUPER-SECRET-TOKEN"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_malformed_body_is_a_named_error_and_not_a_panic() {
    let (_root, mut cell, mut db) = sealed_vault().await;
    let msg = MessageBuilder::new(Path::new(VAULT_PATH))
        .body(Body::Inline(json!({"messages": []})))
        .build();
    let content = run(&mut cell, &mut db, msg).await;
    assert_eq!(content["header"]["error_code"], "invalid_input");
}

// ---------------------------------------------------------------------------
// The circle: in through the user channel, used by the broker, never returned
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_secret_deposited_offline_is_usable_by_the_running_vault() {
    // This is the whole filling workflow end to end: the operator stores a
    // credential with the daemon not even running (`meclaw --vault-add`), and
    // the vault cell picks it up on its next unlock. No message carried it.
    let (root, mut cell, mut db) = sealed_vault().await;

    let version = meclaw_cells::vault::user_channel::add(
        root.path(),
        VAULT_PATH,
        "tg",
        b"SUPER-SECRET-TOKEN",
        PASSPHRASE.as_bytes(),
    )
    .expect("the user channel stores it");
    assert_eq!(version, 1);

    // Nothing about it is visible as a message — the only record is the vault's
    // own audit row, with the user channel as the actor.
    let actor: String = db
        .call(|c| {
            c.query_row(
                "SELECT actor FROM vault_audit ORDER BY id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap()
        })
        .await;
    assert_eq!(actor, "user-channel");

    unlock(&mut cell, &mut db).await;
    let used = result(
        &run(
            &mut cell,
            &mut db,
            call(
                Some(BROKER),
                json!({"op":"use","use":"sign","name":"tg","payload":"p","grant_id":"g"}),
            ),
        )
        .await,
    );
    assert_eq!(used["version"], 1);
    assert!(used["signature"].is_string());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_user_channel_and_the_cell_agree_on_the_key() {
    // Both halves derive from the same salt in the same database, so a secret
    // stored by one is opened by the other. If that ever drifts, this is the
    // test that catches it — silently unopenable secrets are the worst failure
    // this design can have.
    let (root, mut cell, mut db) = sealed_vault().await;
    unlock(&mut cell, &mut db).await;
    put(&mut cell, &mut db, "by_cell", "value-a").await;

    meclaw_cells::vault::user_channel::add(
        root.path(),
        VAULT_PATH,
        "by_channel",
        b"value-b",
        PASSPHRASE.as_bytes(),
    )
    .expect("the same passphrase opens the vault the cell created");

    for name in ["by_cell", "by_channel"] {
        let content = run(
            &mut cell,
            &mut db,
            call(
                Some(BROKER),
                json!({"op":"use","use":"sign","name":name,"payload":"p","grant_id":"g"}),
            ),
        )
        .await;
        assert!(
            content["header"]["error_code"].is_null(),
            "{name} could not be used: {content}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_user_channel_refuses_a_passphrase_that_does_not_open_the_vault() {
    let (root, mut cell, mut db) = sealed_vault().await;
    unlock(&mut cell, &mut db).await;
    put(&mut cell, &mut db, "tg", "SUPER-SECRET-TOKEN").await;

    let err = meclaw_cells::vault::user_channel::add(
        root.path(),
        VAULT_PATH,
        "second",
        b"x",
        b"a different passphrase",
    )
    .unwrap_err();
    assert!(err.contains("does not open this vault"), "{err}");

    // The vault still holds exactly what it held.
    let status = result(&run(&mut cell, &mut db, call(None, json!({"op":"status"}))).await);
    assert_eq!(status["secrets"].as_array().unwrap().len(), 1);
}

// ---------------------------------------------------------------------------
// Injection at unlock — the one path a plaintext secret takes out of the cell
// ---------------------------------------------------------------------------

/// A vault that hands `tg` to a co-located connector at unlock.
async fn vault_with_injection() -> (tempfile::TempDir, VaultCell, DbConn) {
    let (root, _cell, db) = sealed_vault().await;
    let params = VaultParams::parse(&json!({
        "broker": BROKER,
        "inject_map": {"tg": {"to": "./connector", "key": "bot_token"}}
    }))
    .unwrap();
    let colony = colony_answering(vec![(BROKER.to_string(), VAULT_PATH.to_string())]);
    let view = meclaw_colony::NeighbourhoodView::new(Path::new(VAULT_PATH), colony);
    (root, VaultCell::new(params, Some(view)), db)
}

/// Every emission of one handle() call, in order.
async fn run_all(cell: &mut VaultCell, db: &mut DbConn, msg: Message) -> Vec<CellEmission> {
    let (sink, mut orx) = sink_pair();
    cell.handle(msg, &sink, db).await;
    drop(sink);
    let mut out = Vec::new();
    while let Some(em) = orx.recv().await {
        out.push(em);
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_secret_reaches_its_connector_at_unlock_and_the_requester_only_hears_that_it_did() {
    let (_root, mut cell, mut db) = vault_with_injection().await;
    unlock(&mut cell, &mut db).await;
    put(&mut cell, &mut db, "tg", "SUPER-SECRET-TOKEN").await;
    run(&mut cell, &mut db, call(None, json!({"op": "lock"}))).await;

    let emissions = run_all(
        &mut cell,
        &mut db,
        call(None, json!({"op": "unlock", "passphrase": PASSPHRASE})),
    )
    .await;

    // One params update to the connector, one answer to the requester.
    let injected = emissions
        .iter()
        .find(|em| em.target.as_str() == "/main/access/connector")
        .expect("the connector is handed its credential");
    assert_eq!(
        injected.content["params"]["bot_token"],
        "SUPER-SECRET-TOKEN"
    );
    assert!(
        injected.content.get("messages").is_none(),
        "a params update carries no turn, so the receiving cell answers with silence"
    );

    let answer = emissions
        .iter()
        .find(|em| em.target.as_str() != "/main/access/connector")
        .expect("the requester is answered");
    let r = result(&answer.content);
    assert_eq!(r["locked"], false);
    assert_eq!(r["injected"][0]["name"], "tg");
    assert_eq!(r["injected"][0]["to"], "/main/access/connector");
    assert_eq!(r["injected"][0]["key"], "bot_token");
    assert!(
        !answer.content.to_string().contains("SUPER-SECRET-TOKEN"),
        "the requester learns THAT it went, never WHAT went: {}",
        answer.content
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_secret_is_skipped_rather_than_failing_the_unlock() {
    // A vault that refuses to open because one of five credentials has not been
    // deposited yet is a vault nobody can commission.
    let (_root, mut cell, mut db) = vault_with_injection().await;
    let emissions = run_all(
        &mut cell,
        &mut db,
        call(None, json!({"op": "unlock", "passphrase": PASSPHRASE})),
    )
    .await;
    assert_eq!(emissions.len(), 1, "nothing was injected");
    let r = result(&emissions[0].content);
    assert_eq!(r["locked"], false, "and the vault is open regardless");
    assert_eq!(r["injected"].as_array().unwrap().len(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_message_can_choose_where_a_secret_goes() {
    // The map is configuration. A caller naming its own target changes nothing,
    // because the field is never read — the same reasoning that makes `reply_to`
    // the only identity this cell trusts.
    let (_root, mut cell, mut db) = vault_with_injection().await;
    unlock(&mut cell, &mut db).await;
    put(&mut cell, &mut db, "tg", "SUPER-SECRET-TOKEN").await;
    run(&mut cell, &mut db, call(None, json!({"op": "lock"}))).await;

    let emissions = run_all(
        &mut cell,
        &mut db,
        call(
            None,
            json!({"op": "unlock", "passphrase": PASSPHRASE,
                   "inject_map": {"tg": {"to": "/main/egon/brain", "key": "x"}},
                   "to": "/main/egon/brain"}),
        ),
    )
    .await;
    assert!(
        !emissions
            .iter()
            .any(|em| em.target.as_str() == "/main/egon/brain"),
        "a body may not steer an injection"
    );
    assert!(
        emissions
            .iter()
            .any(|em| em.target.as_str() == "/main/access/connector"),
        "and the configured target still gets it"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_injection_target_must_be_a_path_and_a_key() {
    for bad in [
        json!({"tg": {"to": "connector", "key": "bot_token"}}),
        json!({"tg": {"to": "./connector"}}),
        json!({"tg": {"key": "bot_token"}}),
        json!({"tg": "connector"}),
    ] {
        assert!(
            VaultParams::parse(&json!({"broker": BROKER, "inject_map": bad})).is_err(),
            "a malformed injection must fail at validation, not at unlock"
        );
    }
}

// ---------------------------------------------------------------------------
// Regression lock — the sanctioned extension (R3) may ADD, never move
// ---------------------------------------------------------------------------

/// GH #421: `deliver` joined the route surface by sanctioned ruling R3 (2026-08-26). That is an
/// addition to a sealed cell type, so what was there before is pinned here by
/// behaviour rather than by trust: each of the seven original operations keeps
/// its caller, its answer shape and its error code.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_seven_original_operations_behave_exactly_as_before() {
    let (_root, mut cell, mut db) = sealed_vault().await;

    // status on a locked vault
    let s = result(&run(&mut cell, &mut db, call(None, json!({"op":"status"}))).await);
    assert_eq!(s["locked"], true);
    assert_eq!(s["key_id"], Value::Null);

    // unlock, put, rotate, status, use, revoke, lock — in that order
    assert_eq!(result(&unlock(&mut cell, &mut db).await)["locked"], false);
    assert_eq!(
        result(&put(&mut cell, &mut db, "tg", "SUPER-SECRET-TOKEN").await)["version"],
        1
    );
    assert_eq!(
        result(
            &run(
                &mut cell,
                &mut db,
                call(
                    None,
                    json!({"op":"rotate","name":"tg","secret":"SUPER-SECRET-TOKEN-2"})
                )
            )
            .await
        )["version"],
        2
    );
    let used = result(
        &run(
            &mut cell,
            &mut db,
            call(
                Some(BROKER),
                json!({"op":"use","use":"sign","name":"tg","payload":"p","grant_id":"g"}),
            ),
        )
        .await,
    );
    assert_eq!(used["signature"].as_str().map(str::len), Some(64));
    assert_eq!(
        result(
            &run(
                &mut cell,
                &mut db,
                call(None, json!({"op":"revoke","name":"tg"}))
            )
            .await
        )["revoked_versions"],
        2
    );
    let after_lock = run(&mut cell, &mut db, call(None, json!({"op":"lock"}))).await;
    assert_eq!(result(&after_lock)["locked"], true);
}

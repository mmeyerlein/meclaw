//! The core pin of ruling R3 / GH #421: a full credential round leaves no
//! plaintext behind.
//!
//! One round means all three halves of the promise at once — a cell that has no
//! credential ASKS for one, the vault DELIVERS it sealed to a key that exists
//! only in the asking task's RAM, and the next message USES it against a real
//! provider. What this file measures afterwards is not an assertion about a
//! single message: it is a sweep over everything the colony wrote down. Every
//! `message_log` row (headers AND body) and every blob on disk is searched for
//! the secret, byte for byte.
//!
//! A pin that only ever says "not found" can also be searching in the wrong
//! place, so the second test is the counter-proof: the same tree, the same
//! sweep, but the secret arrives through the vault's plaintext filling path
//! (`vault.put` over the user channel) instead of being seeded into its
//! `cell.db` — and then the grep has to HIT. The two tests are a pair, and the
//! first is worth nothing without the second.
//!
//! # Why this topology is flat (GH #427)
//!
//! The shipped `access@2` hive keeps its vault as an INTERIOR cell, and a vault
//! inside a sealed hive can never be unlocked: `unlock` is user-channel-only in
//! the vault's ACL, the user channel is a source message (no `reply_to`), and
//! anything that arrives over an edge carries a `reply_to` by construction —
//! so the one caller allowed to unlock is the one caller that cannot get in.
//! That is measured, not assumed, and it is not worked around here.
//!
//! This test therefore wires the SAME cells with the SAME shipped scripts and
//! configs into a FLAT tree, where `/vault` is reachable by a source message.
//! What that costs is the hive boundary: nothing here proves that the broker's
//! lanes are sealed, that `./vault` is unaddressable from outside the scope, or
//! that a mutation reaching inside is refused. All of that is pinned in
//! `crates/meclaw-cells/tests/access_template.rs`, on the shipped hive. What is
//! pinned HERE is the one thing that topology cannot show: what the round
//! leaves in the log.

use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::llm::LlmCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mock_http::{MockResponse, start_mock_server_capturing};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// The value that must never surface. Distinctive enough that a substring hit
/// anywhere in the colony's writings is unambiguous.
const SECRET: &str = "sk-or-v1-NOBODY-MAY-EVER-SEE-THIS";
/// The vault passphrase, armed on disk so the unlock message carries none.
const PASSPHRASE: &str = "a passphrase nobody guesses";
/// The name the grant binds the delivery to (R-AC-2 for the vault).
const CRED_REF: &str = "cred:openrouter:primary";
/// The grant handle. It has to be a literal in `./brain`'s config because
/// `credential_grant_id` is IMMUTABLE — no message may repoint it, so no
/// message can mint it either, and it must exist before the boot.
const GRANT_ID: &str = "grant:e2e-1";
/// The broker, absolute. `params.broker` is compared against the sender path
/// the colony stamps, so an absolute literal takes the resolution out of play.
const BROKER: &str = "/invoke";

// ─────────────────────────────────────────────────────────── the shipped tree

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

/// The three shipped cells this test reuses verbatim. The list is the guard AND
/// the inventory: a file that disappears makes the tests skip rather than pass.
const ACCESS_FILES: &[&str] = &[
    "invoke/config.json",
    "store/config.json",
    "vault/config.json",
];

/// The template root, or `None` where it does not ship. `access@1` is PRIVATE —
/// it is not in `PUBLIC_TEMPLATES`, so in the public clone these tests exit
/// cleanly instead of failing on a dead `templates/` reference (the documented
/// R2b exception form, GH #49).
fn shipped_access() -> Option<std::path::PathBuf> {
    let root = templates_root().join("access");
    for rel in ACCESS_FILES {
        if !root.join(rel).exists() {
            return None;
        }
    }
    Some(root)
}

fn read_json(p: &std::path::Path) -> Value {
    meclaw_core::serde_json::from_str(&std::fs::read_to_string(p).unwrap())
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(v).unwrap()).unwrap();
}

fn patch(root: &std::path::Path, rel: &str, f: impl FnOnce(&mut Value)) {
    let p = root.join(rel);
    let mut v = read_json(&p);
    f(&mut v);
    std::fs::write(&p, meclaw_core::serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

// ─────────────────────────────────────────────────────── the test-only cells

/// The probe. It reaches straight into `./store` — the operator's gesture, and
/// the test's own read channel. Everything it is handed becomes the store
/// operation verbatim.
const PROBE: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
raw = str(msgs[-1].get("text", "{}")) if msgs else "{}"
sys.stdout.write(json.dumps({
    "header": {"route": "pstore"},
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "p1",
                  "text": raw}]}))
"#;

fn code_cell(script: &str, routes: &[&str]) -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": script, "external_timeout_ms": 15000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {
                "body": {"messages": {"type": "array", "required": true}},
                "hop": {"route": {"type": "string", "values": routes, "required": false}}
            },
            "consumes": {"body": {"messages": {"type": "array", "required": false}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in around the shipped access cells.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// The credential-spending model. No static key at all (`api_key: ""` is not a
/// bearer, GH #271), so the only way it can ever call a provider is the sealed
/// lane — which is what makes the mock server's `Authorization` header a proof
/// rather than a coincidence.
fn brain_config(base_url: &str) -> Value {
    json!({
        "cell": {"type": "llm"},
        "params": {
            "provider": "openai",
            "model": "gpt-4o",
            "api_key": "",
            "base_url": base_url,
            "credential_grant_id": GRANT_ID,
            "external_timeout_ms": 5000,
            "max_tokens": 64
        },
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "emits": {
                "body": {
                    "messages": {"type": "array", "required": false},
                    "meta": {"type": "object", "required": false}
                },
                "hop": {
                    // The one route this cell ever mints. Declared as an enum so
                    // a second, undeclared lane out of a model holding a live
                    // credential would be dropped rather than routed.
                    "route": {"type": "string", "required": false,
                              "values": ["credential_request"]},
                    "grant_id": {"type": "string", "required": false},
                    "error_code": {"type": "string", "required": false},
                    "finish_reason": {"type": "string", "required": false}
                }
            },
            "consumes": {
                "body": {
                    "messages": {"type": "array", "required": false},
                    "sealed": {"type": "object", "required": false}
                }
            },
            "capabilities": ["network:llm", "db:own"]
        },
        "description": {
            "purpose": "A model whose bearer credential arrives sealed from the vault.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

// ───────────────────────────────────────────────────────────────── the topology

/// The flat tree (see the module docs for why it is flat). Every lane the
/// shipped hive draws inside itself is drawn here between siblings, with the
/// same conditions and the same context promotions — the scripts cannot tell
/// the difference, which is the point.
fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        // ── the ask: R-AC-1 lives on THIS edge. The requester is the edge's
        //    word, so a model cannot spend somebody else's grant by claiming
        //    to be them in a body. ──
        {"from": "./brain", "to": "./invoke",
         "condition": "has(hop.route) && hop.route == 'credential_request'",
         "modifier": {"set_context": {"requester": "'agent:brain'"}}},
        // ── the store round trip, which is `invoke`'s whole memory ──
        {"from": "./invoke", "to": "./store",
         "condition": "has(hop.route) && hop.route == 'astore'",
         "modifier": {"set_context": {"access_origin": "'invoke'",
                                      "ac_phase": "hop.phase",
                                      "ac_carry": "hop.carry"}}},
        {"from": "./store", "to": "./invoke",
         "condition": "context.access_origin == 'invoke'"},
        // ── the vault lane ──
        {"from": "./invoke", "to": "./vault",
         "condition": "has(hop.route) && hop.route == 'avault'",
         "modifier": {"set_context": {"access_origin": "'invoke'",
                                      "access_lane": "'vault'",
                                      "ac_phase": "hop.phase",
                                      "ac_carry": "hop.carry"}}},
        {"from": "./vault", "to": "./invoke",
         "condition": "context.access_origin == 'invoke' && context.access_lane == 'vault'"},
        // ── what the vault answers a SOURCE message (the unlock, and the
        //    deprecated injection). Nothing promoted a context onto those, which
        //    is exactly what tells them apart from a brokered round. ──
        {"from": "./vault", "to": "/sink",
         "condition": "!has(context.access_origin)"},
        // ── the sealed ack goes back to the cell that asked; every other ack
        //    leaves for the observer ──
        {"from": "./invoke", "to": "./brain",
         "condition": "has(hop.route) && hop.route == 'ack' && has(hop.operation) \
                       && hop.operation == 'vault.deliver'"},
        {"from": "./invoke", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'ack' && (!has(hop.operation) \
                       || hop.operation != 'vault.deliver')"},
        {"from": "./invoke", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'error'"},
        // ── everything the model says that is NOT a credential request ──
        {"from": "./brain", "to": "/sink",
         "condition": "!has(hop.route) || hop.route != 'credential_request'"},
        // ── the test's own read channel ──
        {"from": "./probe", "to": "./store",
         "condition": "has(hop.route) && hop.route == 'pstore'",
         "modifier": {"set_context": {"access_origin": "'probe'"}}},
        {"from": "./store", "to": "/sink",
         "condition": "context.access_origin == 'probe'"}
    ]}}})
}

/// Build the tree. Both tests run the identical topology — what differs is only
/// how the secret gets into the vault, which is the whole point of the pair.
fn build_tree(td: &tempfile::TempDir, access: &std::path::Path, base_url: &str) {
    let root = td.path();
    // The shipped `invoke` script late-binds `${ACCESS_USAGE_ROWS:-500}`, so the
    // tree needs an env source for the substitution to run against.
    std::fs::write(root.join(".env"), "ACCESS_USAGE_ROWS=500\n").unwrap();
    // A deliberately tiny inline bound. At the 64 KiB default nothing in this
    // round is big enough to offload, and the blob half of the sweep would
    // measure an empty directory. At 256 bytes most of the round — the grant
    // row, the carry, the sealed box, the provider answer — lands as a FILE, so
    // "no plaintext" has to hold on disk as well as in the log.
    std::fs::write(
        root.join("colony.json"),
        r#"{"blob_inline_max_bytes": 256}"#,
    )
    .unwrap();
    write(root, "main/config.json", &main_config());
    // The three shipped cells, copied verbatim — script, contract and all. A
    // re-typed script would pin this test's idea of the broker instead of the
    // broker.
    write(
        root,
        "main/invoke/config.json",
        &read_json(&access.join("invoke/config.json")),
    );
    write(
        root,
        "main/store/config.json",
        &read_json(&access.join("store/config.json")),
    );
    let mut vault = read_json(&access.join("vault/config.json"));
    vault["params"]["broker"] = json!(BROKER);
    // Only the broker edge points at the vault, so nothing else has to be
    // declared here — an undeclared inbound edge would keep the vault locked.
    vault["params"]["sealed_neighbors"] = json!([]);
    vault["params"]["inject_map"] = json!({});
    write(root, "main/vault/config.json", &vault);
    write(
        root,
        "main/probe/config.json",
        &code_cell(PROBE, &["pstore"]),
    );
    write(root, "main/brain/config.json", &brain_config(base_url));
}

/// Fill the vault the way `meclaw --vault-add` does: straight into its own
/// `cell.db`, with no colony running. This is not a shortcut around the test —
/// it is the production filling path, and using a message instead would put the
/// secret into the very log this file is about.
fn seed_vault_secret(td: &tempfile::TempDir, name: &str, value: &str, passphrase: &str) {
    use meclaw_cells::vault::crypto::MasterKey;
    use meclaw_cells::vault::store as vs;
    let dir = td.path().join("main/vault");
    std::fs::create_dir_all(&dir).unwrap();
    let conn = meclaw_colony::persist::open_or_create_cell_db(&dir.join("cell.db")).unwrap();
    vs::apply_ddl(&conn).unwrap();
    let salt = vs::salt_or_create(&conn).unwrap();
    let key = MasterKey::derive(passphrase.as_bytes(), &salt).unwrap();
    let (nonce, ct) = key.seal(value.as_bytes()).unwrap();
    vs::put(&conn, name, &nonce, &ct, &vs::now_iso()).unwrap();
}

/// `key_source: "plainfile"` — the passphrase comes off disk, so the unlock
/// message carries none. Without it the passphrase would be a second plaintext
/// in the log, and this file would be measuring its own fixture.
fn arm_plainfile_key(td: &tempfile::TempDir, passphrase: &str) {
    let keyfile = td.path().join("vault.key");
    std::fs::write(&keyfile, passphrase).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&keyfile, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    patch(td.path(), "main/vault/config.json", |v| {
        v["params"]["key_source"] = json!("plainfile");
        v["params"]["key_file"] = json!(keyfile.to_string_lossy());
    });
}

async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<Message>) {
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            ("store".to_string(), Arc::new(StoreCellFactory)),
            ("llm".to_string(), Arc::new(LlmCellFactory)),
            (
                "vault".to_string(),
                Arc::new(meclaw_cells::vault::VaultCellFactory),
            ),
        ]
    };
    // A real blob store, so the offload path is LIVE: a body that outgrows the
    // inline bound becomes a file under `blobs/`, which is precisely the second
    // place a plaintext could come to rest. A colony without one would make the
    // blob half of the pin vacuous.
    let h = ColonyHandle::new_with_blobs_at(td, factories());
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(64);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");
    (h, sink_rx)
}

// ───────────────────────────────────────────────────────────── message helpers

fn to(cell: &str, text: &str) -> Message {
    MessageBuilder::new(Path::new(cell))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .ttl(400)
        .build()
}

fn send_json(cell: &str, v: &Value) -> Message {
    to(cell, &meclaw_core::serde_json::to_string(v).unwrap())
}

fn body_of(m: &Message) -> &Value {
    match &m.body {
        Body::Inline(v) => v,
        Body::Blob(_) => panic!("inline expected"),
    }
}

fn hop_of(m: &Message, key: &str) -> String {
    m.headers
        .hop
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn turn_text(m: &Message) -> String {
    body_of(m)["messages"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The JSON payload of the one turn a store or a port answer carries.
fn turn_json(m: &Message) -> Value {
    meclaw_core::serde_json::from_str(&turn_text(m)).unwrap_or(Value::Null)
}

async fn recv_bounded(rx: &mut mpsc::Receiver<Message>) -> Option<Message> {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
}

/// The next message the sink collects that satisfies `pred`, skipping the rest.
/// `what` only names the wait in the panic, so a hang is diagnosable.
async fn recv_where(
    rx: &mut mpsc::Receiver<Message>,
    what: &str,
    pred: impl Fn(&Message) -> bool,
) -> Message {
    let mut seen: Vec<String> = Vec::new();
    for _ in 0..24 {
        let m = recv_bounded(rx).await.unwrap_or_else(|| {
            panic!("nothing more arrived while waiting for {what}; saw {seen:?}")
        });
        if pred(&m) {
            return m;
        }
        seen.push(format!(
            "{} {}",
            meclaw_core::serde_json::to_string(&m.headers.hop).unwrap_or_default(),
            turn_text(&m)
        ));
    }
    panic!("{what} never arrived; saw {seen:?}");
}

/// A store op through the probe, returned as its rows. A store answer is the
/// one thing on this sink that echoes `hop.operation`, which is what makes the
/// filter unambiguous while a credential round is in flight.
async fn probe(h: &ColonyHandle, rx: &mut mpsc::Receiver<Message>, op: Value) -> Value {
    h.send(send_json("/probe", &op)).await;
    let m = recv_where(rx, "a store answer", |m| !hop_of(m, "operation").is_empty()).await;
    turn_json(&m)
}

/// The `unlock` gesture: a SOURCE message, which is the only caller the vault's
/// ACL lets deposit or unlock. No `reply_to` — that is what makes it one.
fn unlock_message() -> Message {
    MessageBuilder::new(Path::new("/vault"))
        .body(Body::Inline(json!({"messages": [{
            "origin": "assistant", "type": "tool_call", "id": "u1",
            "text": "{\"op\":\"unlock\"}"
        }]})))
        .ttl(400)
        .build()
}

/// A timestamp in the format the shipped `invoke` script compares as a STRING
/// (`%Y-%m-%dT%H:%M:%S.%fZ`, six fractional digits). A different width would
/// make `now() >= expires_at` a lexicographic accident.
fn stamp(offset_secs: i64) -> String {
    (chrono::Utc::now() + chrono::Duration::seconds(offset_secs))
        .format("%Y-%m-%dT%H:%M:%S%.6fZ")
        .to_string()
}

/// The grant, written straight into the store rather than asked for through
/// `./policy`. The handle has to exist BEFORE the boot (`credential_grant_id`
/// is immutable, so it is a literal in the config), and the granting half is
/// pinned in `access_template.rs` — this file is about the delivery.
async fn plant_grant(h: &ColonyHandle, rx: &mut mpsc::Receiver<Message>) {
    probe(
        h,
        rx,
        json!({"operation": "insert", "table": "grants", "row": {
            "grant_id": GRANT_ID, "requester": "agent:brain",
            "capability": "credential.read", "subject": "member:example",
            "scope": {"actions": ["vault.deliver"]},
            "cred_ref": CRED_REF, "purpose": "authenticate",
            "issued_at": stamp(0), "expires_at": stamp(3600),
            "rule_id": "r-cred", "constraints": {}}}),
    )
    .await;
    probe(
        h,
        rx,
        json!({"operation": "insert", "table": "grant_events", "row": {
            "id": "ev0000000001", "grant_id": GRANT_ID, "event": "granted",
            "at": stamp(0), "actor": "test", "reason_code": "", "detail": {}}}),
    )
    .await;
}

/// One chat-completions answer, enough for the model to finish a turn.
fn chat_answer() -> MockResponse {
    MockResponse::ok_json(
        json!({
            "id": "chatcmpl-1",
            "object": "chat.completion",
            "created": 1,
            "model": "gpt-4o",
            "choices": [{"index": 0, "finish_reason": "stop",
                         "message": {"role": "assistant", "content": "pong"}}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        })
        .to_string()
        .as_bytes(),
    )
}

// ────────────────────────────────────────────────────────────────── the sweep

/// What one sweep found. Both tests run this exact function, which is what
/// makes the counter-proof a control rather than a second opinion.
struct Sweep {
    /// How many `message_log` rows were searched.
    log_rows: usize,
    /// How many blob files were searched.
    blobs: usize,
    /// One locator per place the secret was found, empty when it was nowhere.
    hits: Vec<String>,
}

/// Search everything the colony wrote down for [`SECRET`]: every `message_log`
/// row — headers AND body, because a leak is a leak wherever it rides — and
/// every file under `blobs/`, since a body that outgrew the inline bound lives
/// there and nowhere else.
fn sweep(td: &tempfile::TempDir) -> Sweep {
    let conn = rusqlite::Connection::open(td.path().join("colony.db")).expect("open colony.db");
    let mut st = conn
        .prepare("SELECT headers, body_payload FROM message_log")
        .expect("message_log");
    let rows: Vec<(String, Option<String>)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("query")
        .map(Result::unwrap)
        .collect();

    let mut hits = Vec::new();
    for (i, (headers, body)) in rows.iter().enumerate() {
        if headers.contains(SECRET) {
            hits.push(format!("message_log row {i} HEADERS: {headers}"));
        }
        let body = body.as_deref().unwrap_or_default();
        if body.contains(SECRET) {
            hits.push(format!("message_log row {i} BODY: {body}"));
        }
    }

    let mut blobs = 0usize;
    fn walk(dir: &std::path::Path, blobs: &mut usize, hits: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                walk(&p, blobs, hits);
            } else if let Ok(bytes) = std::fs::read(&p) {
                *blobs += 1;
                if bytes.windows(SECRET.len()).any(|w| w == SECRET.as_bytes()) {
                    hits.push(format!("blob {}", p.display()));
                }
            }
        }
    }
    walk(&td.path().join("blobs"), &mut blobs, &mut hits);

    Sweep {
        log_rows: rows.len(),
        blobs,
        hits,
    }
}

// ═══════════════════════════════════════════════════════════════════════ pins

/// The claim, end to end: ask, sealed delivery, use — and nothing of the secret
/// anywhere the colony writes.
///
/// The order of the assertions is the order of the evidence. First the round has
/// to actually happen (a green sweep over a round that never ran is a green
/// sweep over nothing): the model refuses its first turn with
/// `credential_pending`, the broker books a `vault.deliver` spend, and the mock
/// provider sees `Authorization: Bearer <the seeded secret>` — which no code
/// path could produce unless the box really opened in the model's RAM. Only
/// then is the log swept.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_full_credential_round_leaves_no_plaintext_in_the_log_or_in_a_blob() {
    let Some(access) = shipped_access() else {
        return;
    };
    let (addr, _server, captured) =
        start_mock_server_capturing(vec![chat_answer(), chat_answer(), chat_answer()]).await;

    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &access, &format!("http://{addr}/v1"));
    seed_vault_secret(&td, CRED_REF, SECRET, PASSPHRASE);
    arm_plainfile_key(&td, PASSPHRASE);
    let (h, mut rx) = boot(&td).await;

    // 1. Unlock, over the user channel. A brokered edge could not do this, and
    //    that asymmetry is the vault's filling discipline.
    h.send(unlock_message()).await;
    let opened = recv_where(&mut rx, "the unlock answer", |m| {
        turn_json(m).get("locked").is_some()
    })
    .await;
    assert_eq!(
        turn_json(&opened)["locked"],
        json!(false),
        "the vault refused to unlock, so nothing below would prove anything: {}",
        turn_text(&opened)
    );

    // 2. The grant the model's config already names.
    plant_grant(&h, &mut rx).await;

    // 3. First turn: no credential in RAM, so the model asks and refuses this
    //    one message. It must NOT have called the provider.
    h.send(to("/brain", "ping")).await;
    let pending = recv_where(&mut rx, "the credential_pending refusal", |m| {
        hop_of(m, "error_code") == "credential_pending"
    })
    .await;
    assert_eq!(
        hop_of(&pending, "finish_reason"),
        "error",
        "a refused turn is an error turn: {:?}",
        pending.headers.hop
    );
    assert!(
        captured.lock().await.is_empty(),
        "a cell without a credential called the provider anyway"
    );

    // 4. The sealed round runs beside us. Its positive receipt is the booked
    //    spend — `invoke` writes the usage row only after the box came back.
    let mut booked = false;
    for _ in 0..40 {
        let usage = probe(
            &h,
            &mut rx,
            json!({"operation": "select", "table": "usage",
                   "columns": ["grant_id", "operation", "outcome"],
                   "where": {"grant_id": GRANT_ID}, "limit": 5}),
        )
        .await;
        if let Some(row) = usage.as_array().and_then(|a| a.first()) {
            assert_eq!(row["operation"], "vault.deliver", "usage: {usage}");
            assert_eq!(
                row["outcome"], "ok",
                "the delivery was refused, so the round never completed: {usage}"
            );
            booked = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(
        booked,
        "no vault.deliver spend was ever booked for {GRANT_ID}"
    );

    // 5. Second turn: the credential is in RAM now, so the provider is called.
    h.send(to("/brain", "ping again")).await;
    let answer = recv_where(&mut rx, "the model's answer", |m| {
        hop_of(m, "finish_reason") == "stop"
    })
    .await;
    assert_eq!(
        body_of(&answer)["messages"][0]["text"],
        "pong",
        "the mock's answer came back: {}",
        turn_text(&answer)
    );

    // 6. THE proof that the RAM credential is the real one. Nothing else in this
    //    colony knows the value — the config carries an empty key.
    let seen = captured.lock().await.clone();
    let last = seen.last().expect("the provider was called");
    assert_eq!(
        last.headers.get("authorization").map(String::as_str),
        Some(format!("Bearer {SECRET}").as_str()),
        "the seeded secret never reached the wire: {:?}",
        last.headers
    );

    h.shutdown().await;

    // 7. And now the sweep. Both non-vacuity guards come first: a "nothing
    //    found" over an empty log or an empty blob directory is not a finding.
    let found = sweep(&td);
    assert!(
        found.log_rows > 10,
        "the message log holds {} rows — a pin over an empty log measures nothing",
        found.log_rows
    );
    assert!(
        found.blobs > 0,
        "no body was ever offloaded, so the blob half of this sweep searched an \
         empty directory — lower colony.json blob_inline_max_bytes until it does"
    );
    assert!(
        found.hits.is_empty(),
        "the credential is on record after all: {:?}",
        found.hits
    );

    eprintln!(
        "gh421 sweep: {} message_log rows and {} blobs checked, no plaintext",
        found.log_rows, found.blobs
    );
}

/// The counter-proof. A pin that only ever reports "not found" can also be
/// looking in the wrong column, at the wrong needle, or at a log nothing was
/// ever written to. So the same sweep is pointed at a round that DOES put a
/// plaintext credential on the wire, and it has to HIT.
///
/// The vehicle is `vault.put` over the user channel — the vault's documented
/// filling path, and the one place where a secret still travels in the clear
/// because it has nowhere else to come from. That makes this the sharpest
/// possible control: it is the exact reason the test above fills the vault
/// through its `cell.db` (`seed_vault_secret`) instead of by sending a message.
/// Swap that fixture for a `put` and the pin next door goes red — which is the
/// property a control case exists to demonstrate.
///
/// **Why not `params.inject_map`**, the deprecated injection-at-unlock that
/// advertises itself as THE plaintext path: measured here, it never reaches
/// `message_log` at all. Its emission is a `params`-only body, the UBF schema
/// requires one of `system`/`messages`/`attachments`, and the colony's
/// debug-build validation dead-letters the emission (`InvalidUbfBody`) before a
/// log row is written. A counter-proof hung on it would have been green by
/// accident and blind by construction. (That the deprecated path is broken in a
/// debug build is a finding of its own, not a thing this file repairs.)
///
/// If this test ever goes green by NOT finding the secret, the pin next door has
/// lost its teeth and both belong looked at together. And on the day the `put`
/// lane stops carrying plaintext too, this test is not deleted — it is re-hung
/// on whatever visible plaintext path exists then, because the question it
/// answers ("does the sweep work at all") outlives any single mechanism.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_grep_would_find_a_plaintext_if_one_were_there() {
    let Some(access) = shipped_access() else {
        return;
    };
    let (addr, _server, _captured) = start_mock_server_capturing(vec![chat_answer()]).await;

    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &access, &format!("http://{addr}/v1"));
    arm_plainfile_key(&td, PASSPHRASE);
    let (h, mut rx) = boot(&td).await;

    h.send(unlock_message()).await;
    let opened = recv_where(&mut rx, "the unlock answer", |m| {
        turn_json(m).get("locked").is_some()
    })
    .await;
    assert_eq!(
        turn_json(&opened)["locked"],
        json!(false),
        "{}",
        turn_text(&opened)
    );

    // The deposit — a source message whose body IS the credential.
    h.send(
        MessageBuilder::new(Path::new("/vault"))
            .body(Body::Inline(json!({"messages": [{
                "origin": "assistant", "type": "tool_call", "id": "p1",
                "text": json!({"op": "put", "name": CRED_REF, "secret": SECRET}).to_string()
            }]})))
            .ttl(400)
            .build(),
    )
    .await;
    let stored = recv_where(&mut rx, "the put answer", |m| {
        turn_json(m).get("version").is_some()
    })
    .await;
    assert_eq!(
        turn_json(&stored)["name"],
        CRED_REF,
        "the deposit was refused, so this test proves nothing: {}",
        turn_text(&stored)
    );

    h.shutdown().await;

    let found = sweep(&td);
    assert!(
        !found.hits.is_empty(),
        "the same sweep that guards the sealed round found nothing after a \
         plaintext credential demonstrably travelled — the pin next door is \
         blind, not clean ({} log rows and {} blobs swept)",
        found.log_rows,
        found.blobs
    );

    eprintln!(
        "gh421 counter-proof: {} hit(s) across {} message_log rows and {} blobs",
        found.hits.len(),
        found.log_rows,
        found.blobs
    );
}

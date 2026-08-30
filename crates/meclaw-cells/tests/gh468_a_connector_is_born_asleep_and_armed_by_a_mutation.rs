//! GH #468 — a Telegram connector grown into a live colony without taking the
//! upstream away from the poller that is still running, and armed afterwards by
//! a mutation rather than by an editor.
//!
//! # The case
//!
//! A bot token permits exactly ONE `getUpdates` consumer. Growing a channel the
//! ordinary way starts its long poll at birth, so the new connector and the old
//! one steal each other's updates from the first second. The switchover script
//! of the last rebuild solved that by editing `config.json` **in the target
//! tree** — the one build form this project does not allow: that file is a
//! bootstrap imprint written once, at instantiation, and nothing reads an edit
//! of it until the next spawn.
//!
//! The canonical form is two mutations through the door, and both of them are
//! written out in `templates/telegram-connector/README.md`. This file does not
//! paraphrase them: it READS them out of the README and applies them. A recipe
//! that stops working is then red here, which is what
//! `docs/development-rules.md` § 2d asks of a behavioural promise on a public
//! template surface — grep the sentence AND assert the mechanism.
//!
//! # The four measurements
//!
//! * **`the_readme_parked_manifest_grows_a_node_that_does_not_poll`** — the
//!   parked manifest verbatim: it commits, the node is registered, and the
//!   registry says `inactive`.
//! * **`birth_inactive_alone_keeps_the_poller_off_a_reachable_upstream`** — the
//!   same manifest with its `base_url` pointed at a fake that WOULD answer.
//!   Nothing arrives there. The placeholders are a seatbelt; `birth` is the
//!   mechanism, and this is what separates the two.
//! * **`without_the_birth_declaration_the_same_manifest_polls_at_once`** — the
//!   control. The same tree, the same fake, `birth` removed: the fake is hit.
//!   An empty result and a forgotten call must never look alike
//!   (`docs/development-rules.md` § 2c), and without this test the one above
//!   would be green even if the connector had never been grown at all.
//! * **`the_readme_arming_manifest_swings_the_edges_and_the_first_update_lands`**
//!   — the whole story: parked against a reachable fake (silent), then armed
//!   with the README's `swap_nodes`, and now the poll arrives carrying the REAL
//!   token out of the colony's `.env`, and the update it returns reaches the
//!   sink as a user turn.
//!
//! # Why the arming manifest is patched in one field
//!
//! The README's `swap_nodes` sets no `base_url`, because a production arming
//! does not: the template default is `https://api.telegram.org`. A hermetic test
//! cannot reach it, so the manifest is read verbatim and its `with.params` gains
//! exactly one key, the fake's address. Everything the test is about — the
//! operation, the match, the fresh name, the `${VAR}` token — is the README's.

use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Message, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mock_http::{CapturedRequest, MockResponse, start_mock_server_capturing};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc, oneshot};

/// The token the test colony's `.env` carries. A placeholder, and it is the
/// whole point that it is one: what the test proves is that the ARMED node
/// substitutes `${TELEGRAM_BOT_TOKEN}` out of the colony's own environment,
/// never that any particular value is right.
const ENV_TOKEN: &str = "test-bot-token-468";

/// The parked node's placeholder token, as the README writes it.
const PARKED_TOKEN: &str = "parked";

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// GH #49: a tree that did not ship the template is SKIPPED, never judged.
fn shipped() -> bool {
    repo("templates/telegram-connector/config.json").is_file()
        && repo("templates/telegram-connector/README.md").is_file()
}

// ───────────────────────────────────────────────────────────────────────────
// Reading the two manifests out of the README
// ───────────────────────────────────────────────────────────────────────────

/// Every fenced ```json block of the connector README that parses as one JSON
/// object. The README carries other fences (an edge list, a fragment of an
/// `add_nodes` entry) that are not standalone documents; those simply do not
/// parse and are skipped, so the selection below cannot accidentally pick one.
fn readme_json_blocks() -> Vec<Value> {
    let text = std::fs::read_to_string(repo("templates/telegram-connector/README.md"))
        .expect("the connector README is readable");
    let mut out = Vec::new();
    let mut rest = text.as_str();
    while let Some(open) = rest.find("```json\n") {
        let after = &rest[open + "```json\n".len()..];
        let Some(close) = after.find("\n```") else {
            break;
        };
        if let Ok(v) = meclaw_core::serde_json::from_str::<Value>(&after[..close]) {
            out.push(v);
        }
        rest = &after[close..];
    }
    out
}

/// The README's parked manifest: the one `add_nodes` entry declaring
/// `birth: "inactive"`.
fn readme_parked_manifest() -> Value {
    readme_json_blocks()
        .into_iter()
        .find(|v| v["diff"]["add_nodes"][0]["birth"] == "inactive")
        .expect(
            "templates/telegram-connector/README.md must carry a manifest whose \
             add_nodes entry declares birth: \"inactive\" — that recipe IS the \
             canonical form GH #468 rules, and a README without it is the drift \
             this test exists to catch",
        )
}

/// The README's arming manifest: the one carrying a `swap_nodes`.
fn readme_arming_manifest() -> Value {
    readme_json_blocks()
        .into_iter()
        .find(|v| v["diff"]["swap_nodes"].is_array())
        .expect(
            "templates/telegram-connector/README.md must carry a manifest that \
             arms the parked node with `swap_nodes` — arming in place is \
             impossible (bot_token is immutable, config.json is never \
             rewritten), so the swap is the mechanism and the README owes it",
        )
}

// ───────────────────────────────────────────────────────────────────────────
// The colony
// ───────────────────────────────────────────────────────────────────────────

/// A root that holds nothing but a scope marker, the ONE template under test,
/// and an `.env` whose only value is a placeholder.
fn build_root(root: &std::path::Path) {
    std::fs::write(
        root.join("colony.json"),
        b"{\n  \"schema_version\": 1,\n  \"message_default_ttl\": 64\n}\n",
    )
    .unwrap();
    let main = root.join("main");
    std::fs::create_dir_all(&main).unwrap();
    std::fs::write(
        main.join("config.json"),
        br#"{"cell": {"type": "hive"}, "params": {"graph": {"edges": []}}}"#,
    )
    .unwrap();
    let tpl = root.join("templates").join("telegram-connector");
    std::fs::create_dir_all(&tpl).unwrap();
    for f in ["config.json", "template.json", "README.md"] {
        std::fs::copy(repo("templates/telegram-connector").join(f), tpl.join(f)).unwrap();
    }
    std::fs::write(
        root.join(".env"),
        format!("TELEGRAM_BOT_TOKEN={ENV_TOKEN}\n"),
    )
    .unwrap();
}

/// Boot the colony with the REAL `proxy` factory — an inert stand-in would make
/// every measurement here vacuous, because "did not poll" is exactly what an
/// inert cell does.
async fn boot(td: &tempfile::TempDir) -> ColonyHandle {
    build_root(td.path());
    let factories: Vec<(String, Arc<dyn CellFactory>)> = vec![(
        "proxy".to_string(),
        Arc::new(meclaw_cells::proxy::factory::ProxyCellFactory) as Arc<dyn CellFactory>,
    )];
    let h = ColonyHandle::new_with_factories_at(td, factories.clone());
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("a root holding one hive and nothing else must boot");
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: td.path().join("templates"),
            ack: ack_tx,
        })
        .await
        .expect("rescan");
    ack_rx
        .await
        .expect("rescan ack")
        .expect("the one-template registry must scan");
    h
}

async fn mutate(h: &ColonyHandle, payload: Value) -> MutationOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("send mutation");
    tokio::time::timeout(Duration::from_secs(30), ack_rx)
        .await
        .expect("mutation ack timed out")
        .expect("mutation ack")
}

fn committed(outcome: &MutationOutcome, what: &str) {
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "{what} must commit, and a refusal here is the finding rather than a \
         test bug: {outcome:?}"
    );
}

/// The registry row for `path`, or `None`.
async fn registry_row(
    h: &ColonyHandle,
    path: &str,
) -> Option<meclaw_colony::api_dto::RegistryEntryDto> {
    let (ack_tx, ack_rx) = oneshot::channel::<ReadRegistryReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: None,
            path_prefix: None,
            cell_type: None,
            active: None,
            limit: 500,
            ack: ack_tx,
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(30), ack_rx)
        .await
        .expect("ReadRegistry ack timed out")
        .unwrap()
        .entries
        .into_iter()
        .find(|e| e.path == path)
}

/// A fake Telegram that always answers, so "nothing arrived" can only mean
/// "nobody asked". The first poll carries one update; every later one is an
/// empty result (the mock repeats its last canned response forever).
async fn fake_telegram() -> (String, Arc<Mutex<Vec<CapturedRequest>>>) {
    let (addr, _join, cap) = start_mock_server_capturing(vec![
        MockResponse::ok_json(
            br#"{"ok":true,"result":[
                {"update_id":42,"message":{"message_id":7,"chat":{"id":100},
                 "from":{"id":200},"text":"armed"}}
            ]}"#,
        ),
        MockResponse::ok_json(br#"{"ok":true,"result":[]}"#),
    ])
    .await;
    // The join handle is deliberately dropped: the server task lives for the
    // duration of the tokio runtime, which is the test.
    (format!("http://{addr}"), cap)
}

/// Long enough for a poller that exists to have asked several times (the first
/// poll fires with zero backoff), short enough to keep the suite quick.
const SETTLE: Duration = Duration::from_secs(3);

// ───────────────────────────────────────────────────────────────────────────
// The measurements
// ───────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_readme_parked_manifest_grows_a_node_that_does_not_poll() {
    if !shipped() {
        return;
    }
    let td = tempfile::TempDir::new().unwrap();
    let h = boot(&td).await;
    let (sink_tx, _sink_rx) = mpsc::channel::<Message>(32);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    let manifest = readme_parked_manifest();
    // The placeholder token is part of the promise: a parked node must not hold
    // the real credential, because holding it is what arming means.
    assert_eq!(
        manifest["diff"]["add_nodes"][0]["override_params"]["bot_token"],
        json!(PARKED_TOKEN),
        "the README's parked node must carry a literal placeholder token, never \
         a ${{VAR}} that would resolve to the real one"
    );
    let outcome = mutate(&h, manifest).await;
    committed(&outcome, "the README's parked manifest");

    let row = registry_row(&h, "/telegram").await.expect(
        "a node born inactive is REGISTERED and addressable — that is \
                 the difference between parking it and not growing it",
    );
    assert!(
        !row.active,
        "the parked node must be persisted inactive, or the next reboot starts \
         the poller nobody asked for: {row:?}"
    );

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn birth_inactive_alone_keeps_the_poller_off_a_reachable_upstream() {
    if !shipped() {
        return;
    }
    let td = tempfile::TempDir::new().unwrap();
    let h = boot(&td).await;
    let (sink_tx, _sink_rx) = mpsc::channel::<Message>(32);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    let (base_url, captured) = fake_telegram().await;

    // The README parks against a closed port. Here the address is LIVE, so the
    // only thing that can keep the fake quiet is `birth: "inactive"` itself.
    let mut manifest = readme_parked_manifest();
    manifest["diff"]["add_nodes"][0]["override_params"]["base_url"] = json!(base_url);
    committed(
        &mutate(&h, manifest).await,
        "the parked manifest pointed at a reachable upstream",
    );

    tokio::time::sleep(SETTLE).await;
    let seen = captured.lock().await.len();
    assert_eq!(
        seen, 0,
        "a node born inactive builds NO task, so the long poll never opens — \
         the upstream is reachable here and must still see nothing. {seen} \
         request(s) arrived, which means the connector took the token away from \
         whoever else is holding it."
    );

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_the_birth_declaration_the_same_manifest_polls_at_once() {
    if !shipped() {
        return;
    }
    let td = tempfile::TempDir::new().unwrap();
    let h = boot(&td).await;
    let (sink_tx, _sink_rx) = mpsc::channel::<Message>(32);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    let (base_url, captured) = fake_telegram().await;

    // The control for the test above. Same tree, same fake, same manifest —
    // minus the one declaration. If this is quiet too, the silence up there
    // proves nothing about `birth`.
    let mut manifest = readme_parked_manifest();
    manifest["diff"]["add_nodes"][0]["override_params"]["base_url"] = json!(base_url);
    manifest["diff"]["add_nodes"][0]
        .as_object_mut()
        .unwrap()
        .remove("birth");
    committed(&mutate(&h, manifest).await, "the un-parked manifest");

    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        if !captured.lock().await.is_empty() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "without `birth: \"inactive\"` the connector opens its long poll at \
             birth — that is the behaviour the declaration exists to suppress, \
             and if it does not happen here the parked test above is measuring \
             nothing"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_readme_arming_manifest_swings_the_edges_and_the_first_update_lands() {
    if !shipped() {
        return;
    }
    let td = tempfile::TempDir::new().unwrap();
    let h = boot(&td).await;
    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(32);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    let (base_url, captured) = fake_telegram().await;

    // 1. Parked, against a live address: the wiring is laid down and dark.
    let mut parked = readme_parked_manifest();
    parked["diff"]["add_nodes"][0]["override_params"]["base_url"] = json!(base_url);
    committed(&mutate(&h, parked).await, "the parked manifest");
    tokio::time::sleep(SETTLE).await;
    assert!(
        captured.lock().await.is_empty(),
        "the parked half must be silent before the arming half means anything"
    );

    // 2. Armed. The manifest is the README's; only the address it does not name
    //    is filled in, because a hermetic test cannot reach api.telegram.org.
    let mut arm = readme_arming_manifest();
    arm["diff"]["swap_nodes"][0]["with"]["params"]["base_url"] = json!(base_url);
    committed(&mutate(&h, arm).await, "the README's arming manifest");

    // 3. The turn the fake returns has to arrive as a user-origin message —
    //    through the edges the PARKED mutation wrote, swung onto the armed node
    //    by the swap. Nothing re-wired them by hand.
    let msg = tokio::time::timeout(Duration::from_secs(30), sink_rx.recv())
        .await
        .expect(
            "no turn reached the sink: either the swap did not start the poller, \
             or it did not carry the parked node's edges across",
        )
        .expect("sink channel closed");

    match &msg.body {
        meclaw_core::Body::Inline(v) => {
            let turns = v["messages"].as_array().expect("messages[]");
            assert_eq!(turns.len(), 1, "one inbound message is one user turn");
            assert_eq!(turns[0]["origin"], json!("user"));
            assert_eq!(turns[0]["text"], json!("armed"));
        }
        other => panic!("expected an inline body, got {other:?}"),
    }
    assert_eq!(
        msg.headers.hop.get("chat_id").and_then(|v| v.as_i64()),
        Some(100),
        "the chat the turn came from travels on the hop"
    );

    // 4. And the poll carried the REAL token — substituted out of the colony's
    //    `.env` at instantiation, never written into the mutation body. This is
    //    what arming MEANS; a swap that kept the placeholder would have reached
    //    the fake just as well and proved nothing.
    let paths: Vec<String> = captured
        .lock()
        .await
        .iter()
        .map(|r| r.path.clone())
        .collect();
    assert!(
        paths
            .iter()
            .any(|p| p.contains(&format!("/bot{ENV_TOKEN}/getUpdates"))),
        "the armed connector must poll with the token its ${{TELEGRAM_BOT_TOKEN}} \
         resolved to; requests seen: {paths:?}"
    );
    assert!(
        !paths.iter().any(|p| p.contains(PARKED_TOKEN)),
        "the placeholder must never reach the upstream: {paths:?}"
    );

    // 5. The parked node survives, disconnected — no-delete, and swinging the
    //    edges back is an ordinary mutation.
    let old = registry_row(&h, "/telegram")
        .await
        .expect("a swapped-out node keeps its registry row");
    assert!(
        !old.active,
        "the predecessor is preserved DISCONNECTED, not deleted and not left \
         running: {old:?}"
    );

    h.shutdown().await;
}

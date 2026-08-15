//! hard-shell -- `examples/hard-shell` really refuses the metadata endpoint.
//!
//! The example makes a claim about the DEFAULT: a colony that was given no
//! security configuration at all still refuses to fetch `169.254.169.254`, the
//! address every cloud provider answers credentials on. Not because a rule was
//! written for it -- the seed contains no rule, no allow list, no policy file --
//! but because the cell ships that way and an opt-out has to be typed.
//!
//! So this file takes the shipped seed and the shipped `grow.json` verbatim --
//! no inlined copy, no paraphrase -- boots the one, applies the other, and
//! sends the attack through the front door:
//!
//!   POST /messages -> door -> web_fetch(169.254.169.254) -> DENIED -> terminal
//!
//! No network is needed and none is used: the deny fires on the ADDRESS, before
//! any connect, so the run is offline by construction and costs nothing.
//!
//! The other two moments of the example -- the root lease and the orphan
//! journal -- are process-level facts about the daemon rather than properties
//! of this tree, and they are pinned where they live:
//! `crates/meclaw-cli/tests/gh121_root_lease.rs` boots two real daemons on one
//! root, and `crates/meclaw-cells/tests/gh116_orphan_reap.rs` hard-kills a real
//! daemon and watches the next boot reap its child. The README walks a reader
//! through both by hand.

use meclaw_cells::WebFetchCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, ColonyMsg, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

// ─────────────────────────────────────────────────────── the shipped example

fn repo_path(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn example_path(rel: &str) -> std::path::PathBuf {
    repo_path("examples/hard-shell").join(rel)
}

/// The templates `grow.json` names, each next to the path this test really
/// reads. Both ship publicly, so the file runs in the open clone exactly as it
/// does here -- and the paths are spelled out rather than formatted, so the
/// export's R2b check can read the names off them (GH #9).
const GROWN_FROM: [(&str, &str); 2] = [
    ("door", "templates/door"),
    ("terminal", "templates/terminal"),
];

/// One cell from `door@1`, one checked-in `web_fetch`, one from `terminal@1`.
const CELLS_AFTER_GROW: usize = 3;

/// The address of the story. Every major cloud answers instance credentials
/// here, over plain HTTP, to anything that asks from inside the machine.
const METADATA: &str = "http://169.254.169.254/latest/meta-data/iam/security-credentials/";

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

// ═══════════════════════════════════════ 1. the seed configures NO security

/// The premise, measured rather than asserted in prose: the whole seed is three
/// files, and not one of them says anything about what may be reached. If a
/// future edit adds an allow list here, the example stops proving "by default"
/// and this test goes red first.
#[test]
fn the_seed_carries_no_security_configuration_at_all() {
    let mut files: Vec<String> = Vec::new();
    fn walk(dir: &std::path::Path, base: &std::path::Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).expect("seed dir").flatten() {
            let p = entry.path();
            if p.is_dir() {
                walk(&p, base, out);
            } else {
                out.push(
                    p.strip_prefix(base)
                        .expect("under seed")
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    let seed = example_path("seed");
    walk(&seed, &seed, &mut files);
    files.sort();
    assert_eq!(
        files,
        vec![
            "colony.json".to_string(),
            "main/config.json".to_string(),
            "main/probe/config.json".to_string(),
        ],
        "the seed grew a file -- if it is a policy file, the example is no longer about defaults"
    );

    let probe = read_json(&seed.join("main/probe/config.json"));
    assert_eq!(probe["cell"]["type"], json!("web_fetch"));
    let params = probe["params"].as_object().expect("params");
    assert!(
        !params.contains_key("allow_private_networks"),
        "the example opted OUT of the deny, so it proves nothing about the default: {params:?}"
    );
    // The knob that would open it is a real knob -- naming it here is what
    // makes the absence above a statement instead of an oversight.
    assert!(
        params.contains_key("external_timeout_ms"),
        "the cell is configured, just not about reachability: {params:?}"
    );
}

/// `grow.json` names only templates that ship publicly -- otherwise the example
/// is a promise the open clone cannot keep. And it wires the deny lane on the
/// error CODE rather than on the message text: a reader who copies this gets a
/// branch that survives a reworded refusal.
#[test]
fn grow_json_ships_and_routes_the_refusal_on_its_code() {
    let grow = read_json(&example_path("grow.json"));
    let named: Vec<&str> = grow["diff"]["add_nodes"]
        .as_array()
        .expect("add_nodes")
        .iter()
        .map(|n| n["template"].as_str().expect("template"))
        .collect();
    let expected: Vec<&str> = GROWN_FROM.iter().map(|(name, _)| *name).collect();
    assert_eq!(named, expected, "grow.json grew a template: {named:?}");
    for (name, dir) in GROWN_FROM {
        assert!(
            repo_path(dir).join("template.json").is_file(),
            "{name}@1 is missing from the tree this test runs in"
        );
    }

    let edges = grow["diff"]["add_edges"].as_array().expect("add_edges");
    assert!(
        edges.iter().any(|e| e["condition"]
            .as_str()
            .is_some_and(|c| c.contains("hop.error_code == 'target_blocked'"))
            && e["modifier"]["set_hop"]["route"] == json!("'denied'")),
        "the deny has no lane of its own, so nobody downstream can see it: {edges:?}"
    );
}

// ══════════════════════════════════════════════════ 2. the colony under test

fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![
        (
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        ),
        ("web_fetch".to_string(), Arc::new(WebFetchCellFactory)),
    ]
}

fn build_root(td: &tempfile::TempDir) {
    let root = td.path();
    copy_tree(&example_path("seed"), root);
    for (name, dir) in GROWN_FROM {
        copy_tree(&repo_path(dir), &root.join("templates").join(name));
    }
}

async fn boot(td: &tempfile::TempDir) -> ColonyHandle {
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the seed must boot");
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: td.path().join("templates"),
            ack: ack_tx,
        })
        .await
        .expect("rescan");
    ack_rx.await.expect("rescan ack");
    h
}

async fn grow(h: &ColonyHandle) -> meclaw_colony::mutation::MutationOutcome {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload: read_json(&example_path("grow.json")),
            reply_to: None,
            trace_id: meclaw_core::Uuid::now_v7(),
            parent_message_id: meclaw_core::Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("send mutation");
    ack_rx.await.expect("mutation ack")
}

async fn registry_paths(h: &ColonyHandle) -> Vec<String> {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: None,
            path_prefix: None,
            cell_type: None,
            active: None,
            limit: 1000,
            ack: ack_tx,
        })
        .await
        .expect("read registry");
    let mut v: Vec<String> = ack_rx
        .await
        .expect("registry ack")
        .entries
        .into_iter()
        .map(|e| e.path)
        .collect();
    v.sort();
    v
}

/// What a model's tool call looks like once it has crossed the door: a
/// `tool_call` turn whose text is the JSON arguments.
fn fetch(url: &str) -> Message {
    MessageBuilder::new(Path::new("/surface"))
        .body(Body::Inline(json!({"messages": [{
            "origin": "assistant", "type": "tool_call", "id": "c1",
            "text": json!({"url": url}).to_string()
        }]})))
        .ttl(200)
        .build()
}

fn text_of(m: &Message) -> String {
    match &m.body {
        Body::Inline(v) => v["messages"][0]["text"].as_str().unwrap_or("").to_string(),
        Body::Blob(_) => String::new(),
    }
}

fn hop_str(m: &Message, key: &str) -> String {
    m.headers
        .hop
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

async fn recv(rx: &mut mpsc::Receiver<Message>) -> Message {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .expect("30s: nothing arrived at the terminal")
        .expect("the terminal lane went quiet")
}

// ══════════════════════════════════ 3. the refusal, through the whole colony

/// The moment the example is named after: an unconfigured colony refuses the
/// cloud metadata endpoint, says which range it belongs to, and puts the
/// refusal on a lane of its own so the tree above can see it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_metadata_endpoint_is_refused_by_a_colony_that_was_told_nothing() {
    let td = tempfile::TempDir::new().unwrap();
    build_root(&td);
    let h = boot(&td).await;

    let outcome = grow(&h).await;
    assert!(
        matches!(
            outcome,
            meclaw_colony::mutation::MutationOutcome::Committed { .. }
        ),
        "grow.json was not committed: {outcome:?}"
    );
    let after = registry_paths(&h).await;
    assert_eq!(
        after,
        vec![
            "/probe".to_string(),
            "/sink".to_string(),
            "/surface".to_string()
        ],
        "the tree is not the three cells the README draws"
    );
    assert_eq!(after.len(), CELLS_AFTER_GROW);

    // The probe watches the terminal's lane; the example sends every outcome
    // to a terminal, and a terminal that swallows leaves nothing to assert.
    let (tap_tx, mut tap_rx) = mpsc::channel::<Message>(16);
    h.spawn(Path::new("/tap"), move || CaptureCell::new(tap_tx.clone()))
        .await;
    h.add_edge(
        meclaw_core::Uuid::now_v7(),
        Path::new("/probe"),
        Path::new("/tap"),
    )
    .await;

    h.send(fetch(METADATA)).await;
    let denied = recv(&mut tap_rx).await;

    assert_eq!(
        hop_str(&denied, "finish_reason"),
        "error",
        "the refusal did not arrive as an error: {:?}",
        denied.headers.hop
    );
    assert_eq!(
        hop_str(&denied, "error_code"),
        "target_blocked",
        "some OTHER failure happened -- this test would then prove nothing: {:?}",
        denied.headers.hop
    );
    assert_eq!(
        denied.headers.hop.get("http_status"),
        None,
        "an http_status means a connect happened, and the whole claim is that none did"
    );

    // The refusal NAMES the reason. A deny that only says "no" leaves an
    // operator guessing whether it was policy, DNS or a dead network.
    let said = text_of(&denied);
    assert!(
        said.contains("169.254.169.254"),
        "the refusal does not name the target: {said}"
    );
    assert!(
        said.contains("link-local 169.254.0.0/16 (cloud metadata)"),
        "the refusal does not name the RANGE it belongs to: {said}"
    );

    // Nothing was dropped on the way: the deny has a lane, and the lane has an
    // address. This is the second half of "nothing leaks" -- a refusal that
    // dead-letters is a refusal nobody sees.
    let dlq: i64 = rusqlite::Connection::open(td.path().join("colony.db"))
        .expect("colony.db")
        .query_row("SELECT COUNT(*) FROM dead_letters", [], |r| r.get(0))
        .expect("dead_letters count");
    assert_eq!(dlq, 0, "the refusal had nowhere to go");

    h.shutdown().await;
}

/// The neighbours, in one run: every address the machine and the datacentre
/// answer on is refused by the same untouched colony, each naming its own
/// range. One deny is an anecdote; the matrix is the shell.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_colony_refuses_the_whole_inside_of_the_network() {
    let td = tempfile::TempDir::new().unwrap();
    build_root(&td);
    let h = boot(&td).await;
    grow(&h).await;

    let (tap_tx, mut tap_rx) = mpsc::channel::<Message>(16);
    h.spawn(Path::new("/tap"), move || CaptureCell::new(tap_tx.clone()))
        .await;
    h.add_edge(
        meclaw_core::Uuid::now_v7(),
        Path::new("/probe"),
        Path::new("/tap"),
    )
    .await;

    for (url, reason) in [
        ("http://127.0.0.1:8080/admin", "loopback 127.0.0.0/8"),
        ("http://10.0.0.5/internal", "private RFC 1918 10.0.0.0/8"),
        ("http://192.168.1.1/", "private RFC 1918 192.168.0.0/16"),
        ("http://[::1]:9200/_cluster/health", "loopback ::1"),
    ] {
        h.send(fetch(url)).await;
        let m = recv(&mut tap_rx).await;
        assert_eq!(
            hop_str(&m, "error_code"),
            "target_blocked",
            "{url} was not refused: {:?}",
            m.headers.hop
        );
        let said = text_of(&m);
        assert!(
            said.contains(reason),
            "{url} refused without naming {reason}: {said}"
        );
    }

    h.shutdown().await;
}

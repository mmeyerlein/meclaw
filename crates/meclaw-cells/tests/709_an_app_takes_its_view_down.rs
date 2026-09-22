//! GH #709 — an app takes its own view back down, and the lane reaches the screen.
//!
//! A view outlives the turn that produced it, so ending one has to be a message rather
//! than an absence: `display` has accepted `in_withdraw` since it shipped, and its
//! template says why — *"a `ttl_ms` is the timer version of the same wish and does not
//! replace it"*. What was missing was the road. Measured on the e24 graph (Welle H,
//! befund 03): `./apps -> ./channels` carried `view` and nothing else, and the down-edge
//! onto a screen re-stamped `view` alone. An app could ask; nothing carried the asking.
//!
//! It stopped being academic with three views per app. `ambient` writes a clock, a
//! weather and a timer as three views of its own, and the timer is gone when it has rung;
//! `voice2vision` writes one view per card, and a card is replaced. Waiting out a `ttl_ms`
//! is not the same statement as *this is over* — the first leaves the window standing and
//! fading, the second takes it down.
//!
//! **What moves, and what does not.** `member@1.8.0` widens ONE edge: the app rim carries
//! `withdraw` beside `view`, in the same edge rather than in a twin, because the two are
//! one lane pair of one producer and a second edge would be a second thing to keep in
//! step. The down-edge onto the screen is the INSTANTIATING mutation's, as it has always
//! been (`Edge.to` is a static path and a template cannot name a screen it has not met),
//! so this file draws it the way the member's README writes it out and measures that the
//! pair travels end to end.
//!
//! Guarded like every other template-reading test (GH #49).

use meclaw_cells::code::CodeCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped() -> Option<std::path::PathBuf> {
    let p = repo("templates/member");
    p.join("config.json").is_file().then_some(p)
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{} does not parse: {e}", p.display()))
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().expect("a parent directory")).expect("create the directory");
    std::fs::write(
        p,
        meclaw_core::serde_json::to_string_pretty(v).expect("serialise"),
    )
    .expect("write");
}

fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).expect("create the directory");
    for entry in std::fs::read_dir(src).expect("the template directory is readable") {
        let entry = entry.expect("directory entry");
        let from = entry.path();
        if from.is_dir() {
            copy_cells(&from, &dst.join(entry.file_name()));
        } else if entry.file_name() == "config.json" {
            std::fs::copy(&from, dst.join("config.json")).expect("copy the config");
        }
    }
}

// ══════════════════════════════════════════════════════ 1. the widened edge

#[test]
fn the_app_rim_carries_the_withdrawal_beside_the_view() {
    let Some(root) = shipped() else { return };
    let cfg = read_json(&root.join("config.json"));
    let edges = cfg["params"]["graph"]["edges"]
        .as_array()
        .expect("the member ships a graph");

    let rim: Vec<&Value> = edges
        .iter()
        .filter(|e| e["from"] == json!("./apps") && e["to"] == json!("./channels"))
        .collect();
    assert_eq!(
        rim.len(),
        1,
        "ONE edge carries what an app draws out of the apps container. A twin for the \
         withdrawal would be a second thing to keep in step with the first, and the two \
         are one lane pair of one producer: {rim:#?}"
    );
    let cond = rim[0]["condition"].as_str().unwrap_or_default();
    for lane in ["view", "withdraw"] {
        assert!(
            cond.contains(&format!("hop.route == '{lane}'")),
            "the app rim does not carry `{lane}`. A view outlives its turn, so ending one \
             is a message: {cond}"
        );
    }
    assert!(
        cond.contains("context.channel_node") && cond.contains("!= ''"),
        "both lanes still need a screen named on the context, or the container has \
         nowhere to send them: {cond}"
    );

    let meta = read_json(&root.join("template.json"));
    assert_eq!(
        meta["version"], "1.9.0",
        "a lane an app can use and could not before is the second digit \
         (docs/development-rules.md § 4). The number is the LEVEL's, not this \
         lane's: it moved on again with member@1.9.0, which wired the channel \
         whose model answers on its own timeline. What this file guards is the \
         edge below, and that edge has not moved since 1.8.0"
    );
}

// ═════════════════════════════════════════════════════════ 2. end to end

const INERT: &str = r#"
import sys, json
json.load(sys.stdin)
sys.stdout.write(json.dumps([]))
"#;

/// The app: it puts a view up on `in_tick` and takes it down on `in_drop`.
const APP: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
if str(hop.get("route") or "") == "in_drop":
    # A view that is over. `withdraw` says so; a ttl_ms would only let it fade.
    sys.stdout.write(json.dumps({
        "header": {"route": "withdraw"}, "messages": [], "view_id": "timer"}))
else:
    sys.stdout.write(json.dumps({
        "header": {"route": "view"}, "messages": [], "view_id": "timer",
        "kind": "component", "content": {"title": "tea"}}))
"#;

/// The screen, doubled at the lane this file is about: it reports WHICH lane it was
/// handed and for which view. In production the round ends at a screen -- a browser sees
/// it and the colony does not -- so a test needs a witness, and `error_code` on the hop
/// is the one a shipped display never writes there.
const SCREEN: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hdr = doc["envelope"].get("header") or {}
hop = hdr.get("hop") or {}
sys.stdout.write(json.dumps({
    "header": {"error_code": "seen",
               "seen_route": str(hop.get("route") or ""),
               "seen_view": str(doc["body"].get("view_id") or "")},
    "messages": []}))
"#;

const DRIVER: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
sys.stdout.write(json.dumps({
    "header": {"route": str(hop.get("lane") or "in_tick")}, "messages": []}))
"#;

fn double(script: &str, purpose: &str) -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": script, "external_timeout_ms": 10000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "emits": {"body": {"messages": {"type": "array", "required": false}}},
            "consumes": {"body": {"messages": {"type": "array", "required": false}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": purpose,
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

const SCREEN_NAME: &str = "display-main";
const APP_NAME: &str = "ambient";

/// The edges an instantiating mutation draws, written the way
/// `templates/member/README.md` writes them out.
///
/// The down-edge takes ONE ternary rather than two edges: `view` and `withdraw` are the
/// same producer's pair, and the screen's own names for them differ by one word.
fn instance_edges() -> Vec<Value> {
    vec![
        json!({
            "from": format!("./apps/{APP_NAME}"), "to": "./apps",
            "condition": "has(hop.route) && (hop.route == 'view' || hop.route == 'withdraw')",
            "modifier": {"set_context": {"channel_node": format!("'{SCREEN_NAME}'"),
                                         "channel": format!("'{SCREEN_NAME}'")}}
        }),
        json!({
            "from": "./channels", "to": format!("./channels/{SCREEN_NAME}"),
            "condition": format!(
                "has(hop.route) && (hop.route == 'view' || hop.route == 'withdraw') && \
                 has(context.channel_node) && context.channel_node == '{SCREEN_NAME}'"),
            "modifier": {"set_hop": {
                "route": "hop.route == 'withdraw' ? 'in_withdraw' : 'in_view'"
            }}
        }),
        // TEST-ONLY: a witness lane out of the screen. A shipped display emits `event`
        // and `receipt` and puts no `error_code` on the hop, so nothing real matches it.
        json!({
            "from": format!("./channels/{SCREEN_NAME}"), "to": "./channels",
            "condition": "has(hop.error_code)",
            "modifier": {"set_hop": {"route": "'error'"}}
        }),
    ]
}

fn main_config() -> Value {
    let mut edges = vec![json!({
        "from": "./driver", "to": format!("./person/apps/{APP_NAME}"),
        "condition": "has(hop.route) && (hop.route == 'in_tick' || hop.route == 'in_drop')"
    })];
    for lane in ["error", "answer", "ack", "reject"] {
        edges.push(json!({"from": "./person", "to": "/sink",
                          "condition": format!("has(hop.route) && hop.route == '{lane}'")}));
    }
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}})
}

/// Build the tree. `down_edge` says whether the INSTANTIATING mutation's half is drawn
/// — the half `builder@1.11.0` renders and this template cannot, because `Edge.to` is a
/// static path and a member does not know its screen's name.
fn build_tree(td: &tempfile::TempDir, member: &std::path::Path, down_edge: bool) {
    let root = td.path();
    write(root, "main/config.json", &main_config());
    write(
        root,
        "main/driver/config.json",
        &double(DRIVER, "Test driver: puts one message on a named lane."),
    );
    copy_cells(member, &root.join("main/person"));
    for holder in [
        "access",
        "affinity",
        "memory-hive",
        "assistants",
        "firewall",
    ] {
        write(
            root,
            &format!("main/person/{holder}/config.json"),
            &double(INERT, "Inert double for a holder this round never reaches."),
        );
    }
    write(
        root,
        &format!("main/person/apps/{APP_NAME}/config.json"),
        &double(APP, "Test double for an app that holds several views."),
    );
    write(
        root,
        &format!("main/person/channels/{SCREEN_NAME}/config.json"),
        &double(SCREEN, "Test double for the person's screen."),
    );

    let cfg_path = root.join("main/person/config.json");
    let mut cfg = read_json(&cfg_path);
    let edges = cfg["params"]["graph"]["edges"]
        .as_array_mut()
        .expect("the member ships a graph");
    let mut instance = instance_edges();
    if !down_edge {
        // The member's own half alone: the app rim and the witness lane stay, the
        // mutation's down-edge does not.
        instance.remove(1);
    }
    edges.extend(instance);
    std::fs::write(
        &cfg_path,
        meclaw_core::serde_json::to_string_pretty(&cfg).expect("serialise"),
    )
    .expect("write the member config");
    std::fs::write(root.join(".env"), "").expect("write an empty .env");
}

async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<Message>) {
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![(
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        )]
    };
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(32);
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
        .expect("the shipped member must boot");
    (h, sink_rx)
}

fn inject(lane: &str) -> Message {
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("lane".into(), json!(lane));
    MessageBuilder::new(Path::new("/driver"))
        .body(Body::Inline(json!({"messages": []})))
        .hop(hop)
        .ttl(200)
        .build()
}

fn hop_of(m: &Message, key: &str) -> String {
    m.headers
        .hop
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

async fn recv_bounded(rx: &mut mpsc::Receiver<Message>) -> Option<Message> {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
}

/// `(error_code, resolved_target)` of every dead letter.
fn dead_letters(root: &std::path::Path) -> Vec<(String, String)> {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare("SELECT error_code, resolved_target FROM dead_letters")
        .expect("dead_letters");
    st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .expect("query")
        .filter_map(Result::ok)
        .collect()
}

/// `(to_path, headers)` of every logged message.
fn log_rows(root: &std::path::Path) -> Vec<(String, String)> {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare("SELECT to_path, headers FROM message_log")
        .expect("message_log");
    st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .expect("query")
        .filter_map(Result::ok)
        .collect()
}

/// Poll the message log until `pred` holds, or give up. Thirty seconds is the
/// failure-marker convention of this repo, not a timing discriminator.
async fn wait_for_log(
    root: &std::path::Path,
    pred: impl Fn(&(String, String)) -> bool,
) -> Vec<(String, String)> {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let rows = log_rows(root);
        if rows.iter().any(&pred) || std::time::Instant::now() > deadline {
            return rows;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn skip() -> bool {
    if shipped().is_none() {
        eprintln!("member did not travel into this tree -- skipped (GH #49)");
        return true;
    }
    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("no python3 -- skipped");
        return true;
    }
    false
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_view_goes_up_and_comes_down_again_on_the_same_road() {
    if skip() {
        return;
    }
    let member = shipped().expect("guarded above");
    let td = tempfile::tempdir().expect("a temporary directory");
    build_tree(&td, &member, true);
    let (h, mut rx) = boot(&td).await;

    h.send(inject("in_tick")).await;
    let up = recv_bounded(&mut rx)
        .await
        .expect("the view has to reach the screen -- the double answers");
    assert_eq!(hop_of(&up, "seen_route"), "in_view");
    assert_eq!(hop_of(&up, "seen_view"), "timer");

    h.send(inject("in_drop")).await;
    let down = recv_bounded(&mut rx)
        .await
        .expect("the withdrawal has to reach the screen too -- that is the whole task");
    assert_eq!(
        hop_of(&down, "seen_route"),
        "in_withdraw",
        "the withdrawal reached the screen as something else, or not at all. Until \
         member@1.8.0 the app rim carried `view` alone, so an app with three views had \
         no way to end one but to wait out a ttl_ms -- which leaves the window standing \
         and fading, a different statement from `this is over`"
    );
    assert_eq!(hop_of(&down, "seen_view"), "timer");
    h.shutdown().await;
}

/// **The member's own half, on its own.** `./apps -> ./channels` is what `member@1.8.0`
/// moved; the down-edge onto the screen is the instantiating mutation's, as its `view`
/// twin always was.
///
/// This is the shape of the risk the wave has to hand on, measured rather than described:
/// with the member half in place and the mutation's half missing, the withdrawal REACHES
/// `./channels` — so the level carries it — and then stops there as a dead letter. That
/// is what a screen grown before `builder@1.11.0` does, once per withdrawal, and it is
/// why `templates/member/README.md` says the edge has to be redrawn rather than leaving
/// the reader to find it in the DLQ.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_member_half_carries_the_withdrawal_to_the_container_and_no_further() {
    if skip() {
        return;
    }
    let member = shipped().expect("guarded above");
    let td = tempfile::tempdir().expect("a temporary directory");
    build_tree(&td, &member, false);
    let (h, mut rx) = boot(&td).await;

    h.send(inject("in_drop")).await;

    // Delivered AT the container: that half is the member's and it is what 1.8.0 added.
    let rows = wait_for_log(td.path(), |(to, headers)| {
        to.ends_with("/person/channels") && headers.contains("\"withdraw\"")
    })
    .await;
    assert!(
        rows.iter()
            .any(|(to, headers)| to.ends_with("/person/channels")
                && headers.contains("\"withdraw\"")),
        "`./apps -> ./channels` has to carry the withdrawal as far as the container -- \
         that is the half `member@1.8.0` moved, and it is the half this template can \
         have. Log:\n{rows:#?}"
    );

    // And no further, because nothing drew the mutation's half.
    let stray = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await;
    assert!(
        stray.is_err(),
        "nothing reaches the screen without the mutation's down-edge: {stray:?}"
    );
    let dlq = dead_letters(td.path());
    assert!(
        dlq.iter()
            .any(|(code, target)| code == "hive_no_route" && target.ends_with("/person/channels")),
        "and the withdrawal ends as `hive_no_route` AT the container, once per withdrawal. \
         This is exactly what a screen grown before `builder@1.11.0` does, and \
         `templates/member/README.md` says so where a wirer will read it: {dlq:?}"
    );
    h.shutdown().await;
}

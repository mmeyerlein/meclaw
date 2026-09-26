//! GH #845 / #847, the colony case -- two turns of one session reach the
//! provider with the same prefix, and the other side's words arrive framed with
//! the reference affinity computed.
//!
//! Measured AT THE RECEIVER: the request bodies a mock provider recorded. Ruling
//! R-SN-1 names the acceptance point in so many words -- "two turns of the same
//! session -> identical prefix (prefix-cache hit)" -- and no script pin can see
//! it, because the prefix is built by three cells in a row: the collector
//! (`tool_scope`, `system.roster`, `system.instructions.peer`, the peer row), the
//! `llm` cell (the filter, the frame, the system message) and the store between
//! turns (the speaker written back onto the first turn's row).
//!
//! What is booted: the SHIPPED `member`, `affinity` (store and seeds) and one
//! generation of the shipped `assistant` whose spoken surface is the shipped
//! `talky` with the shipped `collector` and a REAL `llm` brain pointed at the
//! mock. The container edges are the recipe's golden
//! (`examples/organism/grow-assistant.json`). The brief road is the real one, so
//! the reference in the frame is the one affinity's brief answered with as `who`
//! -- the lock that `who` reaches the collector (M1 review B-9).
//!
//! Pinned:
//!
//! 1. both requests carry `tools` without the denied `x`, byte-identical;
//! 2. the system message is byte-identical, and it names the participant
//!    (`<ref> = North (north)`) and states the peer rule;
//! 3. the peer turn goes out as role `user` framed `[peer <ref> · North]`, the
//!    reference being `sha256("north")[:8]` -- affinity's, not typed anywhere;
//! 4. request 1's conversation is a prefix of request 2's: the system message
//!    and the first peer turn, byte for byte. The per-turn evidence (the brief
//!    pair) stands at the END of each request by design, after the window.
//!
//! Guarded like every template-reading test (GH #49).

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use mock_openai::{MockOpenAI, canned_chat_completion};
use std::sync::Arc;
use std::time::Duration;

const WHO: &str = "alpha";
const COUNTERPART: &str = "peer:north";
/// `participant_ref("north")` -- the first 8 hex of sha256 of the identity
/// (`templates/affinity/README.md` § Who is speaking). Computed by affinity at
/// run time; written here only to compare against.
const NORTH_REF: &str = "dc365e79";
const SESSION: &str = "s-alpha-talky";

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped() -> bool {
    ["member", "assistant", "talky", "collector", "affinity"]
        .iter()
        .all(|t| repo(&format!("templates/{t}/config.json")).is_file())
        && repo("examples/organism/grow-assistant.json").is_file()
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{} does not parse: {e}", p.display()))
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().expect("a parent")).expect("mkdir");
    std::fs::write(
        p,
        meclaw_core::serde_json::to_string_pretty(v).expect("serialise"),
    )
    .expect("write");
}

fn patch(root: &std::path::Path, rel: &str, f: impl FnOnce(&mut Value)) {
    let p = root.join(rel);
    let mut v = read_json(&p);
    f(&mut v);
    std::fs::write(&p, meclaw_core::serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).expect("mkdir");
    for entry in std::fs::read_dir(src).expect("readable") {
        let entry = entry.expect("entry");
        let from = entry.path();
        let name = entry.file_name();
        if from.is_dir() {
            copy_cells(&from, &dst.join(name));
        } else if name == "config.json"
            || src.file_name().is_some_and(|d| d == "seed")
                && std::path::Path::new(&name)
                    .extension()
                    .is_some_and(|e| e == "jsonl")
        {
            std::fs::copy(&from, dst.join(name)).expect("copy");
        }
    }
}

fn append_rows(root: &std::path::Path, rel: &str, rows: &[Value]) {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(root.join(rel))
        .unwrap_or_else(|e| panic!("{rel}: {e}"));
    for r in rows {
        writeln!(f, "{r}").expect("append a seed row");
    }
}

const INERT: &str = r#"
import sys, json
json.load(sys.stdin)
sys.stdout.write(json.dumps([]))
"#;

/// Emits exactly the `{"header": ..., "messages": ...}` it is handed; the
/// context is the injected message's own and rides on.
const DRIVER: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
sys.stdout.write(json.dumps(json.loads(d["messages"][0]["text"])))
"#;

fn double(script: &str, purpose: &str) -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": script,
                   "external_timeout_ms": 15000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {"body": {"messages": {"type": "array", "required": false}}},
            "consumes": {"body": {"messages": {"type": "array", "required": false}}},
            "capabilities": ["shell:exec"]
        },
        "description": {"purpose": purpose, "use_when": "Test fixture only.",
                        "not_in_scope": "Not a template."}
    })
}

/// Two declarations the collector hands the brain as its menu. `x` is the one
/// the channel denies.
fn menu() -> String {
    let tool = |name: &str| {
        json!({"type": "function", "function": {
            "name": name, "description": format!("tool {name}"),
            "parameters": {"type": "object", "properties": {}}}})
    };
    json!([tool("x"), tool("y")]).to_string()
}

fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        {"from": "./person", "to": "/sink",
         "condition": "!has(hop.route) || !hop.route.startsWith('in_')"},
        {"from": "./driver", "to": format!("./person/assistants/{WHO}/talky/collector"),
         "condition": "has(hop.route) && hop.route == 'turn'",
         "modifier": {"set_hop": {"route": "'in_turn'"}}}
    ]}}})
}

fn build_tree(td: &tempfile::TempDir, base_url: &str) {
    let root = td.path();
    // The test writes its own environment file at run time: a placeholder key
    // for the one real `llm` cell, which talks to the mock and nowhere else.
    std::fs::write(root.join(".env"), "OPENROUTER_API_KEY=test-key\n").expect("write the env file");
    write(root, "main/config.json", &main_config());
    write(
        root,
        "main/driver/config.json",
        &double(DRIVER, "Test driver: emits what the test hands it."),
    );
    copy_cells(&repo("templates/member"), &root.join("main/person"));
    for holder in ["access", "memory-hive", "firewall"] {
        write(
            root,
            &format!("main/person/{holder}/config.json"),
            &double(INERT, "Inert double for a holder this road never reaches."),
        );
    }
    std::fs::remove_dir_all(root.join("main/person/affinity")).expect("drop the ref marker");
    copy_cells(
        &repo("templates/affinity"),
        &root.join("main/person/affinity"),
    );
    patch(root, "main/person/affinity/clock/config.json", |v| {
        v["params"]["schedules"][0]["schedule_id"] = json!("01916f00-0000-7000-8000-000000000845");
        v["params"]["schedules"][0]["cron"] = json!("0 0 4 * * *");
    });
    let seed = "main/person/affinity/store/seed";
    append_rows(
        root,
        &format!("{seed}/entities.jsonl"),
        &[
            json!({"entity_id": COUNTERPART, "kind": "peer", "display_name": "North",
                 "canonical_name": "north", "owner_member": "member:alex",
                 "aieos": {"identity": {"names": {"first": "North"}}},
                 "aieos_version": "1.1.0",
                 "mx": {"peer": {"name": "north", "url": "https://north.example/peer"}},
                 "status": "active", "supersedes": "", "source": "curated",
                 "confidence": 90, "recorded_at": "2026-01-01T00:00:00Z"}),
        ],
    );
    append_rows(
        root,
        &format!("{seed}/disclosure.jsonl"),
        &[json!({"audience": format!("agent:{WHO}"),
                 "audience_set": [format!("agent:{WHO}"), COUNTERPART],
                 "decided_at": "2026-01-01T00:00:00Z", "entity_id": COUNTERPART,
                 "field_path": "aieos.identity.names", "id": "disc:north-0",
                 "mode": "share"})],
    );

    let base = format!("main/person/assistants/{WHO}");
    copy_cells(&repo("templates/assistant"), &root.join(&base));
    let talky = format!("{base}/talky");
    std::fs::remove_dir_all(root.join(&talky)).expect("drop the ref marker");
    copy_cells(&repo("templates/talky"), &root.join(&talky));
    let collector = format!("{talky}/collector");
    std::fs::remove_dir_all(root.join(&collector)).expect("drop the ref marker");
    copy_cells(&repo("templates/collector"), &root.join(&collector));
    patch(root, &format!("{collector}/assemble/config.json"), |v| {
        v["params"]["brief_slots"] = json!(["peer", "channel"]);
        v["params"]["memory_tier"] = json!("");
        v["params"]["turn_write"] = json!("0");
        v["params"]["tool_menu"] = json!(menu());
    });
    patch(root, &format!("{talky}/brain/config.json"), |v| {
        v["params"]["base_url"] = json!(base_url);
        v["params"]["model"] = json!("gpt-4o-mock");
        v["params"]["api_key"] = json!("sk-test");
    });
    let mut inerts = vec![
        format!("{talky}/session-keeper"),
        format!("{talky}/dispatcher"),
        format!("{base}/talky-chat"),
        format!("{base}/cogny"),
        format!("{base}/tools"),
    ];
    for inert in inerts.drain(..) {
        let _ = std::fs::remove_dir_all(root.join(&inert));
        write(
            root,
            &format!("{inert}/config.json"),
            &double(INERT, "Inert double for a node this road never reaches."),
        );
    }
    let ex = read_json(&repo("examples/organism/grow-assistant.json"));
    let raw = meclaw_core::serde_json::to_string(&ex["diff"]["add_edges"]).expect("edges");
    let edges: Vec<Value> =
        meclaw_core::serde_json::from_str(&raw.replace("scribe", WHO)).expect("edges parse");
    patch(root, "main/person/assistants/config.json", |v| {
        v["params"]["graph"] = json!({"edges": edges});
    });
}

async fn boot(
    td: &tempfile::TempDir,
) -> (
    ColonyHandle,
    tokio::sync::mpsc::Receiver<meclaw_core::Message>,
) {
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            ("store".to_string(), Arc::new(StoreCellFactory)),
            ("timer".to_string(), Arc::new(TimerCellFactory)),
            ("llm".to_string(), Arc::new(LlmCellFactory)),
        ]
    };
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (tx, rx) = tokio::sync::mpsc::channel(256);
    h.spawn(Path::new("/sink"), move || CaptureCell::new(tx.clone()))
        .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the shipped member, affinity, talky and collector must boot");
    (h, rx)
}

/// A turn at the collector's door, with the stamps the channel, the member and
/// the session keeper set: the channel's scope (`tools_deny`), the counterpart
/// and the round that holds it, the session.
fn peer_turn(turn_id: &str, text: &str) -> meclaw_core::Message {
    let ctx = json!({"session_id": SESSION, "turn_id": turn_id, "assistant": WHO,
                     "channel": "peer-channel:north", "counterpart": COUNTERPART,
                     "audience_set": format!("[\"agent:{WHO}\",\"{COUNTERPART}\"]"),
                     "tools_deny": ["x"]});
    let spec = json!({"header": {"route": "turn", "turn_id": turn_id},
                      "messages": [{"origin": "peer", "type": "text", "text": text}]});
    let ctx: Map<String, Value> = ctx.as_object().cloned().expect("ctx");
    MessageBuilder::new(Path::new("/driver"))
        .body(Body::Inline(json!({"messages": [
            {"origin": "user", "type": "text", "text": spec.to_string()}]})))
        .context(ctx)
        .ttl(400)
        .build()
}

async fn requests(mock: &MockOpenAI, n: usize) -> Vec<Value> {
    for _ in 0..600 {
        let reqs = mock.recorded_requests().await;
        if reqs.len() >= n {
            return reqs.into_iter().map(|r| r.body).collect();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the provider was called fewer than {n} time(s) within 60 s");
}

fn names(tools: &Value) -> Vec<String> {
    tools
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|t| t["function"]["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_turns_of_one_session_reach_the_provider_with_one_prefix() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let mock = MockOpenAI::start(vec![
        canned_chat_completion("hello North", "stop"),
        canned_chat_completion("again, North", "stop"),
    ])
    .await;
    let td = tempfile::tempdir().expect("tempdir");
    build_tree(&td, &mock.base_url);
    // The sink's receiver is held for the whole run: what leaves the member
    // (the answers) is drained, not refused.
    let (h, _sink) = boot(&td).await;

    h.send(peer_turn("peer-channel#1", "hello from the north"))
        .await;
    let first = requests(&mock, 1).await;
    // The second turn only after the first reached the provider: its window is
    // then read after the first turn's fan-in wrote the speaker onto its row.
    tokio::time::sleep(Duration::from_millis(500)).await;
    h.send(peer_turn("peer-channel#2", "and again")).await;
    let reqs = requests(&mock, 2).await;
    let (r1, r2) = (&first[0], &reqs[1]);

    // 1. The channel's scope, applied at the provider's door, the menu kept in
    //    order -- and byte-identical between the turns.
    assert_eq!(names(&r1["tools"]), vec!["y"], "{r1}");
    assert_eq!(
        r1["tools"].to_string(),
        r2["tools"].to_string(),
        "the same scope yields the same `tools` bytes"
    );

    // 2. The system message: byte-identical, with the legend and the rule.
    let m1 = r1["messages"].as_array().expect("messages");
    let m2 = r2["messages"].as_array().expect("messages");
    assert_eq!(m1[0]["role"], "system", "{r1}");
    assert_eq!(
        m1[0].to_string(),
        m2[0].to_string(),
        "the system prompt does not move between two turns of the same speakers"
    );
    let system = m1[0]["content"].as_str().expect("system text");
    assert!(
        system.contains(&format!(
            "Participants of this channel:\n- {NORTH_REF} = North (north)"
        )),
        "the legend names the reference affinity's `who` carried: {system}"
    );
    assert!(
        system.contains("is someone else's words") && system.contains("never your person"),
        "the fixed peer rule: {system}"
    );

    // 3. The peer turn, framed on the wire from the turn fields.
    assert_eq!(m1[1]["role"], "user", "{r1}");
    assert_eq!(
        m1[1]["content"],
        format!("[peer {NORTH_REF} \u{b7} North]\nhello from the north"),
        "the frame carries the reference and the name affinity answered with"
    );

    // 4. Request 1's conversation is the prefix of request 2's.
    assert_eq!(
        m1[..2].iter().map(Value::to_string).collect::<Vec<_>>(),
        m2[..2].iter().map(Value::to_string).collect::<Vec<_>>(),
        "system + the first peer turn, byte for byte: the provider's prefix cache hits"
    );
    assert_eq!(
        m2[2]["content"],
        format!("[peer {NORTH_REF} \u{b7} North]\nand again"),
        "{r2}"
    );
    h.shutdown().await;
}

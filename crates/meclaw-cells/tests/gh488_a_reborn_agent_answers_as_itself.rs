//! GH #488 — the END-TO-END half: an agent that was exported out of one colony
//! comes up in the next one knowing who it is and how it answers.
//!
//! The measured defect was a dead end with two ends. `identity.soul` and
//! `instructions.reply` sat as `system.*` rows in a brain's own `cell.db`;
//! a brain has no `porter`, so nothing carried them out, and no template seeds
//! them, so nothing put them back. A member-level export therefore carried
//! everything a person had ever said and nothing the agent was.
//!
//! The way out is not a porter for the brain. It is a different HOME: the
//! durable original is the agent's own record in `affinity` — the reserved
//! `mx.brain` subtree of its `entities` row — and the brain's `system.*` is a
//! delivered copy. That single move makes both ends work with lanes that
//! already ship:
//!
//! * OUT, because `entities.mx` is a `json` column in `affinity`'s porter
//!   schema mirror. The record is in the export document by construction.
//! * BACK, because `subscribers` travels too and the porter blanks
//!   `pack_hash`/`sent_at` on import — so the reborn colony's very first push
//!   tick sees a subscription that has never been delivered to and fires.
//!
//! This file runs both ends in one go:
//!
//!   colony A   shipped affinity + shipped talky, one active self-subscription
//!              -> the brain's `cell.db` holds `identity.soul` and
//!                 `instructions.reply`
//!   export     `in_export` into the affinity hive, nine parts out on `dump`
//!   colony B   the same two templates with EMPTY affinity seeds — anonymous,
//!              exactly as a freshly grown one is — fed the nine parts on
//!              `in_import`
//!   proof      B's brain holds the same two slot paths with BYTE-IDENTICAL
//!              values, and a turn taken in B carries both of them into the
//!              system prompt the provider is handed.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use mock_openai::{MockOpenAI, canned_chat_completion};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

// ───────────────────────────────────────────────────────────── the shipped tree

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

/// Every file this run needs out of the two templates. A missing one makes the
/// test skip rather than fail (R2b / GH #49 — `affinity` is not public).
const AFFINITY_FILES: &[&str] = &[
    "config.json",
    "store/config.json",
    "brief/config.json",
    "gate/config.json",
    "push/config.json",
    "clock/config.json",
    "porter/config.json",
    "store/seed/entities.jsonl",
    "store/seed/relations.jsonl",
    "store/seed/trust.jsonl",
    "store/seed/disclosure.jsonl",
    "store/seed/subscribers.jsonl",
];

fn shipped(name: &str, files: &[&str]) -> Option<std::path::PathBuf> {
    let root = templates_root().join(name);
    files
        .iter()
        .all(|rel| root.join(rel).exists())
        .then_some(root)
}

/// The shipped template, copied the way instantiation copies it: `config.json`
/// files, the seed tables next to them, and a `ref` resolved to the tree it
/// names (GH #277).
fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    let src = &resolve_template_ref(src);
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
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
            std::fs::copy(&from, dst.join(name)).unwrap();
        }
    }
}

fn resolve_template_ref(dir: &std::path::Path) -> std::path::PathBuf {
    let mut dir = dir.to_path_buf();
    for _ in 0..8 {
        let Ok(raw) = std::fs::read_to_string(dir.join("config.json")) else {
            return dir;
        };
        let Ok(v) = meclaw_core::serde_json::from_str::<Value>(&raw) else {
            return dir;
        };
        if v["cell"]["type"] != "ref" {
            return dir;
        }
        let reference = v["cell"]["template"]
            .as_str()
            .expect("a ref cell names a template");
        dir = templates_root().join(reference.split('@').next().unwrap_or_default());
    }
    panic!("template ref chain does not terminate at {}", dir.display());
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(v).unwrap()).unwrap();
}

fn patch(root: &std::path::Path, rel: &str, f: impl FnOnce(&mut Value)) {
    let p = root.join(rel);
    let mut v: Value = meclaw_core::serde_json::from_str(&std::fs::read_to_string(&p).unwrap())
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    f(&mut v);
    std::fs::write(&p, meclaw_core::serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

/// The data rows of one shipped seed table (the `schema` header dropped).
fn seed_rows(rel: &str) -> Vec<Value> {
    std::fs::read_to_string(templates_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel}: {e}"))
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| meclaw_core::serde_json::from_str::<Value>(l).expect("a seed line is JSON"))
        .filter(|v| v.get("schema").is_none())
        .collect()
}

/// A seed table cut back to its `schema` header — which is what a freshly grown
/// colony's affinity looks like where the curated record would be. The header
/// stays, because it is what declares the columns to the store.
fn blank_seed(root: &std::path::Path, rel: &str) {
    let p = root.join(rel);
    let raw = std::fs::read_to_string(&p).unwrap();
    let header = raw.lines().next().expect("a seed file has a header row");
    std::fs::write(&p, format!("{header}\n")).unwrap();
}

// ───────────────────────────────────────────────────────────────── the colony

/// Where the subscriber lives. `bootstrap_from_filesystem` roots the tree at
/// `main/`, so the hive `main/talky` answers to `/talky`.
const TALKY: &str = "/talky";
const CLOCK_ID: &str = "01916f00-0000-7000-8000-000000000488";
const KEEPER_ID: &str = "0190a3f2-0000-7000-8000-000000000488";
const NEVER: &str = "0 0 0 1 1 *";

/// The port wiring around the two hives. The identity door is verbatim the one
/// `templates/talky/README.md` prints; the rest is drains, so nothing this test
/// does dead-letters unseen.
fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        // ── THE DOOR IN THE WALL, exactly as documented ──
        {"from": "./affinity", "to": "./talky",
         "condition": "has(hop.route) && hop.route == 'answer' && hop.subscriber == '/talky'",
         "modifier": {"set_hop": {"route": "'in_pack'"}}},
        {"from": "./talky", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'pack_ack'"},
        // ── the transfer lane's own output, and its refusal ──
        {"from": "./affinity", "to": "/sink",
         "condition": "has(hop.route) && (hop.route == 'dump' || hop.route == 'reject')"},
        {"from": "./affinity", "to": "/park",
         "condition": "has(hop.route) && (hop.route == 'error' || hop.route == 'ack' \
          || (hop.route == 'answer' && hop.subscriber != '/talky'))"},
        // ── a turn, and every exit it can leave by ──
        {"from": "./talky", "to": "/park",
         "condition": "has(hop.route) && hop.route != 'pack_ack'"}
    ]}}})
}

fn build_tree(
    td: &tempfile::TempDir,
    affinity: &std::path::Path,
    talky: &std::path::Path,
    base_url: &str,
    anonymous: bool,
    subscription: Option<&Value>,
) {
    let root = td.path();
    // Two seconds, so several push ticks fit inside the test's own budget. The
    // cron comes out of the `.env` through the shipped `${AFFINITY_PUSH_CRON:-…}`
    // default, so late binding is under test too.
    std::fs::write(
        root.join(".env"),
        "AFFINITY_PUSH_CRON=*/2 * * * * *\nOPENROUTER_API_KEY=test-key\nKEEPER_IDLE_MS=0\n",
    )
    .unwrap();
    write(root, "main/config.json", &main_config());
    copy_cells(affinity, &root.join("main/affinity"));
    copy_cells(talky, &root.join("main/talky"));
    patch(root, "main/affinity/clock/config.json", |v| {
        v["params"]["schedules"][0]["schedule_id"] = json!(CLOCK_ID);
    });
    patch(root, "main/talky/session-keeper/night/config.json", |v| {
        v["params"]["schedules"][0]["schedule_id"] = json!(KEEPER_ID);
        v["params"]["schedules"][0]["cron"] = json!(NEVER);
    });
    // GH #464 — the menu tick would ask a tools hive this colony does not have.
    patch(root, "main/talky/collector/menu-clock/config.json", |v| {
        v["params"]["schedules"][0]["schedule_id"] = json!(KEEPER_ID);
        v["params"]["schedules"][0]["cron"] = json!(NEVER);
    });
    patch(root, "main/talky/brain/config.json", |v| {
        v["params"]["base_url"] = json!(base_url);
        v["params"]["model"] = json!("gpt-4o-mock");
    });

    if anonymous {
        // What a grown colony's affinity holds: the tables, and nothing in
        // them. Measured on a real rebuild — zero subscriber rows, zero
        // records, a brain with two tool schemas and no identity at all.
        for t in [
            "entities",
            "relations",
            "trust",
            "disclosure",
            "subscribers",
        ] {
            blank_seed(root, &format!("main/affinity/store/seed/{t}.jsonl"));
        }
    }
    if let Some(row) = subscription {
        let p = root.join("main/affinity/store/seed/subscribers.jsonl");
        let raw = std::fs::read_to_string(&p).unwrap();
        let header = raw.lines().next().unwrap().to_string();
        std::fs::write(
            &p,
            format!(
                "{header}\n{}\n",
                meclaw_core::serde_json::to_string(row).unwrap()
            ),
        )
        .unwrap();
    }
}

async fn boot(
    td: &tempfile::TempDir,
) -> (
    ColonyHandle,
    mpsc::Receiver<Message>,
    mpsc::Receiver<Message>,
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
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(256);
    let (park_tx, park_rx) = mpsc::channel::<Message>(256);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    h.spawn(Path::new("/park"), move || {
        CaptureCell::new(park_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");
    (h, sink_rx, park_rx)
}

fn hop_of(m: &Message, key: &str) -> String {
    m.headers
        .hop
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn first_text(m: &Message) -> String {
    match &m.body {
        Body::Inline(v) => v["messages"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        Body::Blob(_) => String::new(),
    }
}

/// The next message on `rx` whose `hop.route` matches. 30s is the failure
/// marker convention; several two-second push ticks fit inside it.
async fn recv_route(rx: &mut mpsc::Receiver<Message>, route: &str) -> Message {
    let mut seen: Vec<String> = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let Ok(Some(m)) = tokio::time::timeout(left, rx.recv()).await else {
            panic!("no `{route}` arrived within 30s; saw {seen:?}");
        };
        if hop_of(&m, "route") == route {
            return m;
        }
        seen.push(format!(
            "{}/{}",
            hop_of(&m, "route"),
            hop_of(&m, "dump_kind")
        ));
    }
}

/// The subscriber's OWN durable state: the `system` table of the talky brain's
/// `cell.db`. Nothing else in either colony can write a row into it.
fn brain_slots(td: &tempfile::TempDir) -> Vec<(String, String)> {
    let p = td.path().join("main/talky/brain/cell.db");
    if !p.exists() {
        return Vec::new();
    }
    let Ok(conn) = rusqlite::Connection::open(&p) else {
        return Vec::new();
    };
    let Ok(mut stmt) = conn.prepare("SELECT slot_path, value FROM system ORDER BY slot_path")
    else {
        return Vec::new();
    };
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)));
    match rows {
        Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

async fn wait_for_slots(td: &tempfile::TempDir, want: &[&str]) -> Vec<(String, String)> {
    let mut slots = Vec::new();
    for _ in 0..1500 {
        slots = brain_slots(td);
        if want.iter().all(|w| slots.iter().any(|(p, _)| p == w)) {
            return slots;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    slots
}

fn slot(slots: &[(String, String)], path: &str) -> String {
    slots
        .iter()
        .find(|(p, _)| p == path)
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| panic!("the brain holds no `{path}`; it holds {:?}", names(slots)))
}

fn names(slots: &[(String, String)]) -> Vec<String> {
    slots.iter().map(|(p, _)| p.clone()).collect()
}

// ═══════════════════════════════════════════════════════════════════════ the run

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_exported_agent_comes_back_knowing_who_it_is() {
    let (Some(affinity), Some(talky)) = (
        shipped("affinity", AFFINITY_FILES),
        shipped("talky", &["config.json", "brain/config.json"]),
    ) else {
        return;
    };

    // What the shipped record says the agent is. Read out of the seed, because
    // a copy in this file could agree with itself while disagreeing with what
    // ships — and because the two strings below are the whole subject of the
    // byte comparison further down.
    let agent = seed_rows("affinity/store/seed/entities.jsonl")
        .into_iter()
        .find(|r| r["kind"] == "agent")
        .expect("the shipped affinity seeds an agent record");
    let entity_id = agent["entity_id"].as_str().unwrap().to_string();
    let soul = agent["mx"]["brain"]["identity"]["soul"]
        .as_str()
        .expect("the seeded agent record carries mx.brain.identity.soul")
        .to_string();
    let reply = agent["mx"]["brain"]["instructions"]["reply"]
        .as_str()
        .expect("the seeded agent record carries mx.brain.instructions.reply")
        .to_string();
    // The value an `llm` cell writes into its `system` table for such a leaf:
    // the UBF container, serialised compactly. This is the shape the byte
    // comparison is ABOUT — the same one a long-lived deployment was measured
    // holding, which is what makes a transfer of it a transfer and not a
    // re-rendering.
    let want_soul = meclaw_core::serde_json::to_string(&json!({"text": soul})).unwrap();
    let want_reply = meclaw_core::serde_json::to_string(&json!({"text": reply})).unwrap();

    // The subscription: the agent, to ITSELF, for its own durable brain state.
    // `pack_hash` empty means never delivered, which is what makes the first
    // tick a change.
    let subscription = json!({
        "id": "sub:gh488-self", "cell_path": TALKY, "subject": entity_id,
        "audience": format!("agent:{}", entity_id.trim_start_matches("entity:")),
        "channel": "*", "slots": ["brain"], "pack_hash": "", "status": "active",
        "sent_at": ""});

    // ── colony A ────────────────────────────────────────────────────────────
    let a_mock = MockOpenAI::start(vec![]).await;
    let a_td = tempfile::TempDir::new().unwrap();
    build_tree(
        &a_td,
        &affinity,
        &talky,
        &a_mock.base_url,
        false,
        Some(&subscription),
    );
    let (a, mut a_sink, _a_park) = boot(&a_td).await;

    let receipt = recv_route(&mut a_sink, "pack_ack").await;
    assert_eq!(
        hop_of(&receipt, "error_code"),
        "",
        "the pack the shipped record rendered must be ACCEPTED by the door — a \
         family outside `PACK_SLOTS` refuses the whole pack: {:?}",
        receipt.headers.hop
    );

    let a_slots = wait_for_slots(&a_td, &["identity.soul", "instructions.reply"]).await;
    assert_eq!(
        slot(&a_slots, "identity.soul"),
        want_soul,
        "the agent's soul must stand in its brain's own cell.db under the slot \
         path the record names. Slots: {:?}",
        names(&a_slots)
    );
    assert_eq!(
        slot(&a_slots, "instructions.reply"),
        want_reply,
        "and its reply instructions beside it — the half GH #458 had closed the \
         lane to. Slots: {:?}",
        names(&a_slots)
    );

    // ── the export: one word, one walk, nine parts ──────────────────────────
    let mut hop = Map::new();
    hop.insert("route".to_string(), json!("in_export"));
    a.send(
        MessageBuilder::new(Path::new("/affinity"))
            .hop(hop)
            .body(Body::Inline(json!({"messages": []})))
            .ttl(400)
            .build(),
    )
    .await;

    let mut parts: Vec<(i64, String)> = Vec::new();
    loop {
        let m = recv_route(&mut a_sink, "dump").await;
        assert_ne!(
            hop_of(&m, "dump_kind"),
            "",
            "a dump names what it is: {:?}",
            m.headers.hop
        );
        let idx = m
            .headers
            .hop
            .get("export_part")
            .and_then(|v| {
                v.as_i64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            })
            .unwrap_or_default();
        let last = hop_of(&m, "export_final") == "1";
        parts.push((idx, first_text(&m)));
        if last {
            break;
        }
    }
    parts.sort_by_key(|(i, _)| *i);
    assert!(
        parts.len() >= 9,
        "the affinity walk writes one part per transferable table; it wrote {}",
        parts.len()
    );
    let carried: Value = meclaw_core::serde_json::from_str(
        &parts
            .iter()
            .find(|(_, p)| p.contains("\"entities\""))
            .unwrap()
            .1,
    )
    .expect("a part is JSON");
    assert!(
        carried["rows"].to_string().contains(&soul),
        "the agent's identity must be INSIDE the export document — that is the \
         whole reason `mx` was chosen as its home. The entities part carried: {}",
        &carried["rows"].to_string()[..carried["rows"].to_string().len().min(400)]
    );
    a.shutdown().await;

    // ── colony B: it never heard any of this ────────────────────────────────
    let b_mock = MockOpenAI::start(vec![canned_chat_completion("understood", "stop")]).await;
    let b_td = tempfile::TempDir::new().unwrap();
    build_tree(&b_td, &affinity, &talky, &b_mock.base_url, true, None);
    let (b, mut b_sink, _b_park) = boot(&b_td).await;

    // Anonymous, and measurably so: nothing has been pushed, because there is
    // no subscription and no record to push.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let before = brain_slots(&b_td);
    assert!(
        !before
            .iter()
            .any(|(p, _)| p.starts_with("identity") || p.starts_with("instructions")),
        "a grown colony's brain holds no identity of its own — that is the \
         defect this issue is about. It held {:?}",
        names(&before)
    );

    // ── the import: the same nine parts, in the order they were written ─────
    for (idx, part) in &parts {
        let mut hop = Map::new();
        hop.insert("route".to_string(), json!("in_import"));
        b.send(
            MessageBuilder::new(Path::new("/affinity"))
                .hop(hop)
                .body(Body::Inline(json!({"messages": [
                    {"origin": "assistant", "type": "text", "text": part}]})))
                .ttl(400)
                .build(),
        )
        .await;
        let receipt = recv_route(&mut b_sink, "dump").await;
        assert_eq!(
            hop_of(&receipt, "dump_kind"),
            "import_receipt",
            "part {idx} was refused rather than applied: {:?}",
            receipt.headers.hop
        );
    }

    // ── and the first identity pack of a colony that never delivered one ────
    let receipt = recv_route(&mut b_sink, "pack_ack").await;
    assert_eq!(
        hop_of(&receipt, "error_code"),
        "",
        "the first pack after an import must be ACCEPTED: {:?}",
        receipt.headers.hop
    );
    let b_slots = wait_for_slots(&b_td, &["identity.soul", "instructions.reply"]).await;
    assert_eq!(
        slot(&b_slots, "identity.soul"),
        want_soul,
        "BYTE-IDENTICAL, or the transfer re-rendered the agent instead of \
         moving it. Slots: {:?}",
        names(&b_slots)
    );
    assert_eq!(
        slot(&b_slots, "instructions.reply"),
        want_reply,
        "BYTE-IDENTICAL, and this is the half no lane carried at all before \
         GH #488. Slots: {:?}",
        names(&b_slots)
    );

    // ── the point of all of it: a turn answers as the agent ─────────────────
    let mut ctx = Map::new();
    ctx.insert("channel".to_string(), json!("test"));
    ctx.insert("audience_set".to_string(), json!("[\"member:alex\"]"));
    ctx.insert("session_id".to_string(), json!("s-gh488"));
    let mut hop = Map::new();
    hop.insert("route".to_string(), json!("in_turn"));
    b.send(
        MessageBuilder::new(Path::new("/talky/session-keeper"))
            .hop(hop)
            .context(ctx)
            .body(Body::Inline(json!({"messages": [
                {"origin": "user", "type": "text", "text": "who are you?"}]})))
            .ttl(400)
            .build(),
    )
    .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut prompt = String::new();
    while tokio::time::Instant::now() < deadline {
        for req in b_mock.recorded_requests().await {
            let sys: String = req
                .body
                .get("messages")
                .and_then(|m| m.as_array())
                .map(|ms| {
                    ms.iter()
                        .filter(|m| m["role"] == "system")
                        .map(|m| m["content"].as_str().unwrap_or_default().to_string())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            if !sys.is_empty() {
                prompt = sys;
            }
        }
        if prompt.contains(&soul) && prompt.contains(&reply) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        prompt.contains(&soul),
        "the turn's system prompt must carry the imported soul — a slot in a \
         cell.db that never reaches the wire has moved nothing. Prompt: {prompt:?}"
    );
    assert!(
        prompt.contains(&reply),
        "and the imported reply instructions. Prompt: {prompt:?}"
    );

    b.shutdown().await;
}

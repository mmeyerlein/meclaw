//! GH #872 — a member reborn from a SEED delivers its first identity pack.
//!
//! GH #488 gave an agent's identity a durable home (`mx.brain` on its own
//! `affinity` record) and a way back in: the affinity porter blanks
//! `subscribers.pack_hash`/`sent_at` on `in_import`, so the reborn colony's
//! first push tick sees a subscription that was never delivered to and fires.
//! `gh488_a_reborn_agent_answers_as_itself.rs` pins that half.
//!
//! The other way in is the BIRTH seed: `examples/memory-import/build_import.py`
//! writes the export tables under `affinity/store/seed/` of a derived member.
//! Until #872 it placed them verbatim, so the reborn store carried the hash the
//! SOURCE had delivered, `./push` computed the same hash over the same record,
//! and stayed silent for ever. Measured on a rebuilt deployment: 2 486 push
//! ticks, 0 `in_pack`, 0 `pack_ack`, three brains without a single
//! `identity.*` slot -- the agent answered as the vendor's default assistant.
//!
//! This file runs the whole path once:
//!
//!   colony A   shipped affinity + shipped talky, one active self-subscription
//!              with an EMPTY hash -> the real `./push` delivers once and, on
//!              the clean receipt (GH #877), writes the hash it computed; then
//!              `in_export` writes the seed set
//!   tool       `build_import.py` turns that directory into one manifest; the
//!              manifest's `affinity/store/seed/` files are the seed under test
//!   colony B   the same two templates, its affinity store seeded with exactly
//!              those files -- the birth a derived member gets
//!   proof      one `in_pack` reaches the brain's rim carrying the record's two
//!              slots, the rim holds them (since GH #889 in its curator's
//!              ledger, which hands them to the brain with the next call), and
//!              the store wrote a new `sent_at`
//!
//! The hash is never re-implemented here (OR-FD-I-Test): it is the one the
//! shipped `./push` wrote in colony A, carried by a real export.
//!
//! Colony B is the GH #488 island rather than a whole grown member: what the
//! defect is about is the seed rows the tool places and the shipped
//! push/brief/store reading them. A grown member adds an assistant generation,
//! an org shell and three `llm` brains, none of which the seed path touches.
//!
//! Guarded like every template-reading test (GH #49): a tree that does not carry
//! the templates or the example is skipped, never judged.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, from_str, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use mock_openai::MockOpenAI;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// CONTRIBUTING.md failure-marker convention; many two-second ticks fit in it.
const RECV_TIMEOUT: Duration = Duration::from_secs(30);

/// Where the subscriber lives. `bootstrap_from_filesystem` roots the tree at
/// `main/`, so the hive `main/talky` answers to `/talky`.
const TALKY: &str = "/talky";
const SUB_ID: &str = "sub:gh872-self";
const CLOCK_ID: &str = "01916f00-0000-7000-8000-000000000872";
const KEEPER_ID: &str = "0190a3f2-0000-7000-8000-000000000872";
const NEVER: &str = "0 0 0 1 1 *";

// ───────────────────────────────────────────────────────────── the shipped tree

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn templates_root() -> std::path::PathBuf {
    repo("templates")
}

/// Everything this run reads. `examples/` travels with the export, but the
/// guard stays: the test needs a Python interpreter and the library beside it.
fn shipped() -> bool {
    [
        "templates/member/config.json",
        "templates/affinity/config.json",
        "templates/affinity/store/config.json",
        "templates/affinity/push/config.json",
        "templates/affinity/clock/config.json",
        "templates/affinity/porter/config.json",
        "templates/affinity/store/seed/entities.jsonl",
        "templates/affinity/store/seed/disclosure.jsonl",
        "templates/affinity/store/seed/subscribers.jsonl",
        "templates/talky/config.json",
        "templates/talky/brain/config.json",
        "examples/memory-import/build_import.py",
    ]
    .iter()
    .all(|rel| repo(rel).is_file())
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
        let Ok(v) = from_str::<Value>(&raw) else {
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
    let mut v: Value = from_str(&std::fs::read_to_string(&p).unwrap())
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    f(&mut v);
    std::fs::write(&p, meclaw_core::serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

/// The data rows of one seed file (the `schema` header dropped).
fn data_rows(body: &str) -> Vec<Value> {
    body.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| from_str::<Value>(l).expect("a seed line is JSON"))
        .filter(|v| v.get("schema").is_none())
        .collect()
}

// ───────────────────────────────────────────────────────────────── the colony

/// The identity door, verbatim the one `templates/talky/README.md` prints plus
/// the promotion of the row and the hash the pack names (GH #877), a TAP on the
/// same condition (an edge table fans out: every matching edge delivers), the
/// receipt's way home in the form the member draws it -- a pack is booked only
/// when its receipt comes back clean -- and drains for everything else.
fn main_config() -> Value {
    let door = "has(hop.route) && hop.route == 'answer' && hop.subscriber == '/talky'";
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        {"from": "./affinity", "to": "./talky", "condition": door,
         "modifier": {"set_hop": {"route": "'in_pack'"},
                      "set_context": {
                          "pack_sub": "has(hop.pack_sub) ? hop.pack_sub : ''",
                          "pack_hash": "has(hop.pack_hash) ? hop.pack_hash : ''"}}},
        {"from": "./affinity", "to": "/tap", "condition": door,
         "modifier": {"set_hop": {"route": "'in_pack'"}}},
        {"from": "./talky", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'pack_ack'"},
        {"from": "./talky", "to": "./affinity",
         "condition": "has(hop.route) && hop.route == 'pack_ack'",
         "modifier": {"set_hop": {"route": "'in_pack_ack'"},
                      "set_context": {
                          "pack_sub": "has(context.pack_sub) ? context.pack_sub : ''",
                          "pack_hash": "has(context.pack_hash) ? context.pack_hash : ''"}}},
        {"from": "./affinity", "to": "/sink",
         "condition": "has(hop.route) && (hop.route == 'export_done' || hop.route == 'dump' \
          || hop.route == 'reject')"},
        {"from": "./affinity", "to": "/park",
         "condition": "has(hop.route) && (hop.route == 'error' || hop.route == 'ack' \
          || (hop.route == 'answer' && hop.subscriber != '/talky'))"},
        {"from": "./talky", "to": "/park",
         "condition": "has(hop.route) && hop.route != 'pack_ack'"}
    ]}}})
}

/// The tree both colonies share. `seed` decides what the affinity store is
/// born with: `None` keeps the shipped record (colony A, with `subscription`
/// as its only subscriber row), `Some(files)` replaces every shipped seed table
/// with exactly those files (colony B -- the seed `build_import.py` placed).
fn build_tree(
    td: &tempfile::TempDir,
    base_url: &str,
    subscription: Option<&Value>,
    seed: Option<&Map<String, Value>>,
    fence: &std::path::Path,
) {
    let root = td.path();
    std::fs::write(root.join(".env"), "OPENROUTER_API_KEY=test-key\n").unwrap();
    write(root, "main/config.json", &main_config());
    copy_cells(
        &templates_root().join("affinity"),
        &root.join("main/affinity"),
    );
    copy_cells(&templates_root().join("talky"), &root.join("main/talky"));
    patch(root, "main/affinity/clock/config.json", |v| {
        v["params"]["schedules"][0]["schedule_id"] = json!(CLOCK_ID);
        // A literal of `./clock`'s own params since GH #138; two seconds so
        // several push ticks fire inside the failure marker.
        v["params"]["schedules"][0]["cron"] = json!("*/2 * * * * *");
    });
    patch(root, "main/talky/session-keeper/night/config.json", |v| {
        v["params"]["schedules"][0]["schedule_id"] = json!(KEEPER_ID);
        v["params"]["schedules"][0]["cron"] = json!(NEVER);
    });
    patch(root, "main/talky/brain/config.json", |v| {
        v["params"]["base_url"] = json!(base_url);
        v["params"]["model"] = json!("gpt-4o-mock");
    });
    // GH #889: the curator in front of the brain carries an `llm` cell of its
    // own; it is never expected to talk here, and it may only ever reach the mock.
    patch(root, "main/talky/curator/summarizer/config.json", |v| {
        v["params"]["base_url"] = json!(base_url);
        v["params"]["model"] = json!("gpt-4o-mock");
    });
    let fence_s = fence.to_str().expect("a utf-8 fence").to_string();
    patch(root, "main/affinity/store/config.json", |v| {
        v["params"]["transfer"]["base_path"] = json!(fence_s);
    });

    let seed_dir = root.join("main/affinity/store/seed");
    if let Some(row) = subscription {
        let p = seed_dir.join("subscribers.jsonl");
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
    if let Some(files) = seed {
        for entry in std::fs::read_dir(&seed_dir).unwrap() {
            let p = entry.unwrap().path();
            if p.extension().is_some_and(|e| e == "jsonl") {
                std::fs::remove_file(p).unwrap();
            }
        }
        for (name, body) in files {
            std::fs::write(
                seed_dir.join(name),
                body.as_str().expect("a manifest file is text"),
            )
            .unwrap();
        }
    }
}

struct Colony {
    h: ColonyHandle,
    sink: mpsc::Receiver<Message>,
    tap: mpsc::Receiver<Message>,
}

async fn boot(td: &tempfile::TempDir) -> Colony {
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
    let (sink_tx, sink) = mpsc::channel::<Message>(256);
    let (tap_tx, tap) = mpsc::channel::<Message>(256);
    let (park_tx, mut park) = mpsc::channel::<Message>(256);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    h.spawn(Path::new("/tap"), move || CaptureCell::new(tap_tx.clone()))
        .await;
    h.spawn(Path::new("/park"), move || {
        CaptureCell::new(park_tx.clone())
    })
    .await;
    // The park only has to swallow; a full channel must never stall a cell.
    tokio::spawn(async move { while park.recv().await.is_some() {} });
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");
    Colony { h, sink, tap }
}

fn hop_of(m: &Message, key: &str) -> String {
    m.headers
        .hop
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// The next message on `rx` whose `hop.route` matches, or `None` after `within`.
async fn next_route(
    rx: &mut mpsc::Receiver<Message>,
    route: &str,
    within: Duration,
) -> Option<Message> {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Some(m)) if hop_of(&m, "route") == route => return Some(m),
            Ok(Some(_)) => continue,
            _ => return None,
        }
    }
}

/// One cell's `cell.db`, read from outside and read-only -- an observation of
/// the result, never a re-implementation of the mechanism.
fn rows(db: &std::path::Path, sql: &str) -> Vec<Vec<String>> {
    if !db.is_file() {
        return Vec::new();
    }
    let Ok(conn) =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return Vec::new();
    };
    let Ok(mut st) = conn.prepare(sql) else {
        return Vec::new();
    };
    let n = st.column_count();
    st.query_map([], |r| {
        Ok((0..n)
            .map(|i| {
                r.get::<_, Option<String>>(i)
                    .unwrap_or_default()
                    .unwrap_or_default()
            })
            .collect::<Vec<String>>())
    })
    .map(|it| it.filter_map(Result::ok).collect())
    .unwrap_or_default()
}

/// `(pack_hash, sent_at)` of the self-subscription in a colony's affinity store.
fn subscription_state(td: &tempfile::TempDir) -> Option<(String, String)> {
    rows(
        &td.path().join("main/affinity/store/cell.db"),
        &format!("SELECT pack_hash, sent_at FROM subscribers WHERE id = '{SUB_ID}'"),
    )
    .into_iter()
    .next()
    .map(|r| (r[0].clone(), r[1].clone()))
}

/// The receiver: the pack-owned slots of the talky curator's ledger, as
/// `(path, body)` pairs.
///
/// GH #889: this was the brain's own `system` table until the pack moved to the
/// curator. `./curator` holds an accepted pack in its ledger — table `slots`,
/// owner `pack` (`curator@1.0.0`) — and hands it to the brain as a `$replace`
/// root with the NEXT call; the body is read from `blocks` by the slot's hash.
fn ledger_slots(td: &tempfile::TempDir) -> Vec<(String, String)> {
    rows(
        &td.path().join("main/talky/curator/ledger/cell.db"),
        "SELECT s.path, COALESCE(b.body, '') FROM slots s \
         LEFT JOIN blocks b ON b.hash = s.hash \
         WHERE s.owner = 'pack' ORDER BY s.path",
    )
    .into_iter()
    .map(|r| (r[0].clone(), r[1].clone()))
    .collect()
}

/// A ledger path belongs to a family when it IS the family or lies under it.
/// The ledger may hold a family whole (`identity`) or leaf by leaf
/// (`identity.soul`); what is pinned is the family and its text.
fn in_family(path: &str, family: &str) -> bool {
    path == family
        || path
            .strip_prefix(family)
            .is_some_and(|rest| rest.starts_with('.') || rest.starts_with('/'))
}

/// Run the example's own tool -- what a reader runs, so what is under test.
fn build_manifest(export_dir: &std::path::Path, scratch: &std::path::Path) -> Value {
    let out = std::process::Command::new("python3")
        .arg(repo("examples/memory-import/build_import.py"))
        .arg("--export")
        .arg(export_dir)
        .arg("--templates")
        .arg(templates_root())
        .arg("--scope")
        .arg("/os/orgs/t")
        .arg("--name")
        .arg("m")
        .arg("--after-boot")
        .arg(scratch.join("after-boot.json"))
        .output()
        .expect("python3");
    assert!(
        out.status.success(),
        "build_import.py failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    from_str(&String::from_utf8_lossy(&out.stdout)).expect("the tool prints one manifest")
}

// ═══════════════════════════════════════════════════════════════════════ the run

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_born_from_an_export_seed_delivers_its_first_identity_pack() {
    if !shipped() {
        return;
    }

    // What the shipped record says the agent is, read from the seed so a copy
    // in this file cannot agree with itself while disagreeing with what ships.
    let agent = data_rows(
        &std::fs::read_to_string(templates_root().join("affinity/store/seed/entities.jsonl"))
            .unwrap(),
    )
    .into_iter()
    .find(|r| r["kind"] == "agent")
    .expect("the shipped affinity seeds an agent record");
    let entity_id = agent["entity_id"].as_str().unwrap().to_string();
    let soul = agent["mx"]["brain"]["identity"]["soul"]
        .as_str()
        .expect("the agent record carries mx.brain.identity.soul")
        .to_string();
    let reply = agent["mx"]["brain"]["instructions"]["reply"]
        .as_str()
        .expect("the agent record carries mx.brain.instructions.reply")
        .to_string();
    assert!(
        soul.len() >= 30 && reply.len() >= 30,
        "two real texts, not stubs"
    );

    let subscription = json!({
        "id": SUB_ID, "cell_path": TALKY, "subject": entity_id,
        "audience": format!("agent:{}", entity_id.trim_start_matches("entity:")),
        "channel": "*", "slots": ["brain"], "pack_hash": "", "status": "active",
        "sent_at": ""});

    // ── colony A: the real `./push` delivers once and writes its own hash ──
    let fence_td = tempfile::TempDir::new().unwrap();
    let a_mock = MockOpenAI::start(vec![]).await;
    let a_td = tempfile::TempDir::new().unwrap();
    build_tree(
        &a_td,
        &a_mock.base_url,
        Some(&subscription),
        None,
        fence_td.path(),
    );
    let mut a = boot(&a_td).await;
    let ack = next_route(&mut a.sink, "pack_ack", RECV_TIMEOUT)
        .await
        .expect("colony A never delivered its first pack -- the fixture itself is broken");
    assert_eq!(hop_of(&ack, "error_code"), "", "{:?}", ack.headers.hop);

    let deadline = tokio::time::Instant::now() + RECV_TIMEOUT;
    let delivered = loop {
        if let Some((hash, sent)) = subscription_state(&a_td)
            && !hash.is_empty()
            && !sent.is_empty()
        {
            break (hash, sent);
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "colony A's push never wrote its pack_hash back"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    // ── the export: the store writes its own seed set (GH #555) ─────────────
    let mut hop = Map::new();
    hop.insert("route".to_string(), json!("in_export"));
    a.h.send(
        MessageBuilder::new(Path::new("/affinity"))
            .hop(hop)
            .body(Body::Inline(json!({"messages": []})))
            .ttl(400)
            .build(),
    )
    .await;
    let done = next_route(&mut a.sink, "export_done", RECV_TIMEOUT)
        .await
        .expect("the affinity export never finished");
    assert_eq!(hop_of(&done, "export_hive"), "affinity");
    a.h.shutdown().await;

    let seed = fence_td.path().join(hop_of(&done, "seed_dir"));
    assert!(seed.join("export_final.json").is_file());
    // `<export dir>/affinity/seed` -- the tool is pointed at the directory that
    // HOLDS the per-hive directories.
    let export_dir = seed
        .parent()
        .and_then(|p| p.parent())
        .unwrap()
        .to_path_buf();
    let exported_subscribers =
        std::fs::read_to_string(seed.join("subscribers.jsonl")).expect("subscribers.jsonl");
    let exported_row = data_rows(&exported_subscribers)
        .into_iter()
        .find(|r| r["id"] == SUB_ID)
        .expect("the self-subscription travels in the export");
    assert_eq!(
        (
            exported_row["pack_hash"].as_str().unwrap_or_default(),
            exported_row["sent_at"].as_str().unwrap_or_default()
        ),
        (delivered.0.as_str(), delivered.1.as_str()),
        "the export carries what the source DELIVERED -- the state a rebirth \
         inherits (OR-FD-I-Test: the hash comes from a real push run)"
    );

    // ── the tool: one manifest, and the seed it places ──────────────────────
    let scratch = tempfile::TempDir::new().unwrap();
    let manifest = build_manifest(&export_dir, scratch.path());
    let files = manifest["manifest"][0]["diff"]["add_templates"][0]["files"]
        .as_object()
        .expect("the manifest carries the derived template's files");
    let placed: Map<String, Value> = files
        .iter()
        .filter_map(|(k, v)| {
            k.strip_prefix("affinity/store/seed/")
                .map(|name| (name.to_string(), v.clone()))
        })
        .collect();
    let placed_subscribers = placed["subscribers.jsonl"]
        .as_str()
        .expect("the tool placed subscribers.jsonl")
        .to_string();

    // Every assert below is collected rather than fired at once, so the red
    // run names each claim that is broken, not only the first.
    let mut broken: Vec<String> = Vec::new();

    // Assert 3 -- cheap, no colony: the delivery trace is reset, the decision
    // travels, the header is the export's byte for byte.
    assert_eq!(
        placed_subscribers.lines().next(),
        exported_subscribers.lines().next(),
        "the schema header must stay byte-identical"
    );
    let placed_row = data_rows(&placed_subscribers)
        .into_iter()
        .find(|r| r["id"] == SUB_ID)
        .expect("the tool dropped the self-subscription");
    if placed_row["pack_hash"] != "" || placed_row["sent_at"] != "" {
        broken.push(format!(
            "assert 3: the placed subscriber row keeps the source's delivery trace \
             (pack_hash {:?}, sent_at {:?}) -- build_import.py must reset both, as the \
             porter does on in_import",
            placed_row["pack_hash"], placed_row["sent_at"]
        ));
    }
    for key in [
        "status",
        "slots",
        "cell_path",
        "channel",
        "subject",
        "audience",
    ] {
        assert_eq!(
            placed_row[key], exported_row[key],
            "`{key}` is the subscribe DECISION and travels untouched"
        );
    }

    // ── colony B: born from exactly that seed ───────────────────────────────
    let b_mock = MockOpenAI::start(vec![]).await;
    let b_td = tempfile::TempDir::new().unwrap();
    build_tree(
        &b_td,
        &b_mock.base_url,
        None,
        Some(&placed),
        fence_td.path(),
    );
    let mut b = boot(&b_td).await;

    // Assert 1 -- at the receiver's rim: exactly one pack, carrying the record.
    match next_route(&mut b.tap, "in_pack", RECV_TIMEOUT).await {
        None => broken.push(
            "assert 1: no `in_pack` reached the brain's rim within 30s -- the reborn \
             store compares against a hash it never sent and stays silent"
                .to_string(),
        ),
        Some(pack) => {
            let Body::Inline(body) = &pack.body else {
                panic!("an identity pack is inline");
            };
            assert_eq!(
                body["system"]["identity"]["soul"]["text"].as_str(),
                Some(soul.as_str()),
                "the pack must carry the exported soul: {body}"
            );
            assert_eq!(
                body["system"]["instructions"]["reply"]["text"].as_str(),
                Some(reply.as_str()),
                "and the exported reply instructions: {body}"
            );
            // Two more ticks at least: an unchanged record is not re-sent.
            if let Some(second) = next_route(&mut b.tap, "in_pack", Duration::from_secs(5)).await {
                broken.push(format!(
                    "assert 1: a second `in_pack` over an unchanged record: {:?}",
                    second.headers.hop
                ));
            }
            let ack = next_route(&mut b.sink, "pack_ack", RECV_TIMEOUT)
                .await
                .expect("the rim never acknowledged the pack");
            assert_eq!(hop_of(&ack, "error_code"), "", "{:?}", ack.headers.hop);
            // GH #889: the receiver is the curator's ledger now; the brain gets
            // the two slots with its next call, and no call happens here.
            let deadline = tokio::time::Instant::now() + RECV_TIMEOUT;
            loop {
                let slots = ledger_slots(&b_td);
                let holds = |family: &str, text: &str| {
                    slots
                        .iter()
                        .any(|(p, v)| in_family(p, family) && v.contains(text))
                };
                if holds("identity", soul.as_str()) && holds("instructions", reply.as_str()) {
                    break;
                }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "the curator's ledger holds no identity after the ack: {:?}",
                    slots.iter().map(|(p, _)| p).collect::<Vec<_>>()
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }

    // Assert 2 -- in the store's own cell.db: the reborn member delivered.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut now_sent = String::new();
    while tokio::time::Instant::now() < deadline {
        now_sent = subscription_state(&b_td).map(|s| s.1).unwrap_or_default();
        if !now_sent.is_empty() && now_sent != delivered.1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    if now_sent.is_empty() || now_sent == delivered.1 {
        broken.push(format!(
            "assert 2: subscribers.sent_at in the reborn store is {now_sent:?}, the \
             exported value was {:?} -- nothing was delivered",
            delivered.1
        ));
    }

    b.h.shutdown().await;
    assert!(
        broken.is_empty(),
        "GH #872 -- a seeded rebirth must push its first pack:\n{}",
        broken.join("\n")
    );
}

/// Fix strand F of GH #874 (review minor I-M1 of #872): the reset split the
/// seed file with Python's `splitlines()`, which also breaks at U+2028/U+2029,
/// U+0085 and `\x1c`-`\x1e` -- characters `serde_json` writes raw inside a
/// string. A subscriber row carrying one in a value was cut mid-JSON and the
/// tool died on its own export. JSONL is split at `\n` only.
#[test]
fn the_seed_reset_splits_rows_only_at_a_newline() {
    if !shipped() {
        return;
    }
    let row = json!({"id": SUB_ID, "subject": "a\u{2028}b\u{2029}c\u{85}d\u{1c}e",
        "pack_hash": "h-source", "sent_at": "2026-09-01T00:00:00Z"});
    let body = format!(
        "{{\"schema\": \"subscribers\"}}\n{}\n",
        meclaw_core::serde_json::to_string(&row).unwrap()
    );
    let program = concat!(
        "import sys, json, importlib.util\n",
        "spec = importlib.util.spec_from_file_location('build_import', sys.argv[1])\n",
        "tool = importlib.util.module_from_spec(spec)\n",
        "spec.loader.exec_module(tool)\n",
        "sys.stdout.write(tool.reset_on_seed('affinity', 'subscribers.jsonl', ",
        "sys.stdin.read()))\n"
    );
    let mut child = std::process::Command::new("python3")
        .arg("-c")
        .arg(program)
        .arg(repo("examples/memory-import/build_import.py"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("python3");
    {
        use std::io::Write;
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(body.as_bytes())
            .expect("write the seed body");
    }
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "the reset died on a row with a unicode line separator in a value: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let placed = String::from_utf8(out.stdout).expect("utf-8");
    assert_eq!(
        placed.lines().next(),
        Some("{\"schema\": \"subscribers\"}"),
        "the header stays byte for byte: {placed:?}"
    );
    let rows = data_rows(&placed);
    assert_eq!(rows.len(), 1, "one row in, one row out: {placed:?}");
    assert_eq!(
        rows[0]["subject"], row["subject"],
        "the value travels whole"
    );
    assert_eq!(rows[0]["pack_hash"], "", "{placed:?}");
    assert_eq!(rows[0]["sent_at"], "", "{placed:?}");
}

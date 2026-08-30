//! meclaw-os -- the drain adapter in a running colony (GitHub #101, R-MD-2).
//!
//! The script pins live in `memory_drain.rs`. This file asks the question the
//! issue is actually about, and it asks it of a COLONY that contains the
//! SHIPPED memory-hive writer and the shipped hive's `episodes` surface --
//! wired with the hive's OWN `writer -> store` edge. If the episodes land, they
//! land because the adapter speaks the hive's real port, not because a stand-in
//! was taught to accept it.
//!
//! The three pieces come out of `tests/fixtures/memory_drain_colony/` and not
//! out of `templates/memory-hive/` (GH #125). The memory hive is
//! private, so a test that read it at runtime could not travel into the public
//! clone and sat on the export blocklist instead -- the colony gates of a
//! public adapter went unrun by everyone but us. What travels now is the
//! minimum this test needs: the writer byte for byte, its edge byte for byte,
//! and the store PROJECTED onto the one table the drain contract documents.
//! The private `x2_hive_fixture_drift.rs` holds all three against the living
//! template, so the snapshot cannot rot in silence and cannot quietly grow.
//!
//! Two gates, and they are the reason the template exists (R-MD-2):
//!
//! 1. COUNT GATE (lossless) -- `hop.turn_count` of the batch equals the number
//!    of new `episodes` rows of that session. Both sides are measured: the
//!    count off the real batch message, the rows out of the store's own
//!    `cell.db`.
//! 2. IDEMPOTENCE GATE -- the same batch delivered twice leaves the row count
//!    where it was. Proven against a POSITIVE receipt: every ledger reply is
//!    also captured, so the duplicate's `select` is visible, and the row count
//!    is read AFTER it -- no quiet window, no sleep, no ordering luck.
//!
//! Free by construction: the write path is synchronous and model-free by spec,
//! so there is no llm cell in these trees at all and nothing is spent.

use std::time::Duration;

#[path = "support_14b.rs"]
mod support;

use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use support::{boot, recv_bounded};
use tokio::sync::mpsc;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The pinned hive surface (GH #125). Drift lock: `x2_hive_fixture_drift.rs`.
fn fixture(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/memory_drain_colony")
        .join(name)
}

/// The shipped template, copied cell by cell: only `config.json` files travel,
/// so the tree under test IS the template and nothing else.
fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        if from.is_dir() {
            copy_cells(&from, &dst.join(entry.file_name()));
        } else if entry.file_name() == "config.json" {
            std::fs::copy(&from, dst.join("config.json")).unwrap();
        }
    }
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(v).unwrap()).unwrap();
}

/// The memory hive, reduced to the two cells the WRITE path consists of, and
/// wired with the hive's own edge between them. Nothing here is retyped: the
/// writer is the shipped writer, the edge is the shipped edge, and the store
/// carries the shipped `episodes` schema and the shipped store contract. What
/// the projection leaves out plays no part in this write path -- the drain
/// writes turns into `episodes` and reads nothing back.
fn memory_write_path(root: &std::path::Path) {
    let edges: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(fixture("hive_writer_store_edge.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        edges.as_array().expect("edge array").len(),
        1,
        "the write path inside the hive is ONE edge: writer -> store"
    );
    write(
        root,
        "main/memory/config.json",
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}}),
    );
    for (cell, snapshot) in [
        ("writer", "hive_writer_config.json"),
        ("store", "hive_store_config.json"),
    ] {
        let dst = root.join(format!("main/memory/{cell}"));
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::copy(fixture(snapshot), dst.join("config.json")).unwrap();
    }
}

/// Stands in for the collector's close lane: turns a harness message into the
/// close BATCH form (C3 receipt) -- the whole day in `messages[]`, `rounds`
/// next to it, session and counts on the hop.
const PROBE: &str = r#"
import sys, json
doc = json.load(sys.stdin)
d = doc["body"]
envelope = doc["envelope"]
ctx = (envelope.get("header") or {}).get("context") or {}
msgs = d.get("messages", [])
sys.stdout.write(json.dumps({
    "header": {"route": "write", "session_id": str(ctx.get("session_id", "s1")),
               "turn_count": str(len(msgs)), "round_count": "0"},
    "messages": msgs, "rounds": []}))
"#;

/// The drain a parent tree owes the two lanes this test does not measure: the
/// hive's extraction queue and the store's episode echo (in a real hive both go
/// to extract-glue). Terminal on purpose, so the DLQ assertion keeps meaning.
const VOID: &str = r#"
import sys, json
json.load(sys.stdin)
sys.stdout.write(json.dumps([]))
"#;

fn code_cell(script: &str, hop: Value) -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": script, "external_timeout_ms": 10000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {
                "body": {"messages": {"type": "array", "required": true},
                         "rounds": {"type": "array", "required": false}},
                "hop": hop
            },
            "consumes": {"body": {"messages": {"type": "array", "required": true}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in around the memory-drain colony test.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// The wiring the README documents, plus three observation lanes that belong to
/// the TEST, not to the template: the batch copy into `/sink` (so the count
/// gate can read `hop.turn_count` off the real message), every ledger reply
/// into `/park` (the positive receipt the idempotence gate is measured
/// against), and a terminal drain for the two hive lanes downstream of the
/// writer.
fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        // Both ends name the drain HIVE, never the cell inside it (overview
        // § Die Hive-Grenze): the template declares `. -> ./drain` on an in_
        // lane and `./drain -> .` on `episode`, and reaching past those doors
        // would deliver the episode twice — once to the writer and once to a
        // hive path this graph has no exit for.
        {"from": "./probe", "to": "./drain",
         "condition": "has(hop.route) && hop.route == 'write'",
         "modifier": {"set_hop": {"route": "'in_batch'"},
                      "set_context": {"session_id": "hop.session_id"}}},
        // The port edge promotes exactly the four hop keys the template
        // documents, and NOT the provenance (#269). The participant set and
        // the room are keys of the round, declared at the door a turn enters
        // by -- here, the ingress context of the message itself, standing in
        // for the ingress door of a channel generation (ADR-0002 E8). They
        // ride in `context` from there through the drain, through its ledger
        // round trip and into the writer without anything on this path
        // copying, defaulting or widening them. A literal stamped HERE would
        // hide exactly the defect #269 is about: it would make the gates pass
        // over a drain that had thrown the real audience away.
        {"from": "./drain", "to": "./memory/writer",
         "condition": "has(hop.route) && hop.route == 'episode'",
         "modifier": {"set_context": {"session_id": "hop.session_id",
                                      "turn_id": "hop.turn_id",
                                      "happened_at": "hop.happened_at"}}},
        // The drain's own refusal lane (#269). Drained here so a batch this
        // adapter would not take is READ rather than dead-lettered -- which is
        // also what makes the DLQ assertions of the gates below mean something.
        {"from": "./drain", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'reject'"},
        {"from": "./probe", "to": "/sink"},
        // The ledger receipt is the exception this test needs and the boundary
        // rule tolerates: it is not the hive's contract but a probe INTO it, the
        // positive receipt the idempotence gate is measured against. A test that
        // may only see a hive's doors cannot tell "skipped" from "never ran".
        {"from": "./drain/ledger", "to": "/park"},
        {"from": "./memory/writer", "to": "./void",
         "condition": "has(hop.route) && hop.route == 'enqueue'"},
        // GH #519: the writer also hands the turn's text to the embedding lane.
        // This colony has no embedder -- it is the WRITE path under test -- so
        // the lane is drained into the void beside the extraction queue. Both
        // are the same kind of exception: a route the shipped writer emits and
        // this fixture deliberately does not run.
        {"from": "./memory/writer", "to": "./void",
         "condition": "has(hop.route) && hop.route == 'embed'"},
        {"from": "./memory/store", "to": "./void",
         "condition": "context.store_origin == 'episode'"}
    ]}}})
}

fn build_tree(td: &tempfile::TempDir) {
    let root = td.path();
    std::fs::write(root.join(".env"), "").unwrap();
    write(root, "main/config.json", &main_config());
    write(
        root,
        "main/probe/config.json",
        &code_cell(
            PROBE,
            json!({"route": {"type": "string", "values": ["write"], "required": true},
                   "session_id": {"type": "string", "required": false},
                   "turn_count": {"type": "string", "required": false},
                   "round_count": {"type": "string", "required": false}}),
        ),
    );
    write(root, "main/void/config.json", &code_cell(VOID, json!({})));
    copy_cells(&repo("templates/memory-drain"), &root.join("main/drain"));
    memory_write_path(root);
}

/// The round these sessions were held in: one person and one agent, which is
/// exactly the cast of `DAY`. Never `["*"]` -- a universal set would make every
/// gate below pass against an adapter that had lost the real one.
const AUDIENCE: &str = r#"["member:alex","agent:scribe"]"#;
/// The one room they were held in, recorded explicitly. It is never parsed out
/// of the `session_id` prefix: that prefix is a session-keeper convention and
/// no promise to the memory.
const ROOM: &str = "c-drain";

/// A closed session, arriving with the provenance of the round it happened in.
///
/// The keys sit in the ingress `context` because that is where a real colony
/// puts them -- the door of a channel declares the participant set once and the
/// connector promotes the room (ADR-0002 E8; since GH #303 a channel is a node
/// in a `channels` container rather than a `channel` hive, and since GH #454
/// that container stands in the MEMBER, not in one generation of its agent) --
/// and from there they are simply carried, hop by hop, all the way in.
fn day(session: &str, turns: &[(&str, &str)]) -> Message {
    day_with_provenance(
        session,
        turns,
        &[("audience_set", AUDIENCE), ("channel", ROOM)],
    )
}

/// The same day, with the provenance spelled out (or left out) key by key.
fn day_with_provenance(
    session: &str,
    turns: &[(&str, &str)],
    provenance: &[(&str, &str)],
) -> Message {
    let mut ctx = meclaw_core::serde_json::Map::new();
    ctx.insert("session_id".into(), json!(session));
    for (k, v) in provenance {
        ctx.insert((*k).into(), json!(v));
    }
    let msgs: Vec<Value> = turns
        .iter()
        .map(|(origin, text)| json!({"origin": origin, "type": "text", "text": text}))
        .collect();
    MessageBuilder::new(Path::new("/probe"))
        .body(Body::Inline(json!({"messages": msgs})))
        .context(ctx)
        .ttl(64)
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

/// One episodes row, as the hive's writer left it.
#[derive(Debug, PartialEq, Eq)]
struct Episode {
    turn_id: String,
    sender: String,
    content: String,
}

/// The session's episodes, ordered by the index the adapter minted into the
/// `turn_id`. The physical insert order belongs to the hive (six episodes leave
/// in ONE multi-send and the store commits them as they arrive); what the
/// ADAPTER guarantees is that turn i of the day carries index i, and that is
/// what is read back here.
fn episodes(db: &std::path::Path, session: &str) -> Vec<Episode> {
    let Ok(conn) = rusqlite::Connection::open(db) else {
        return vec![];
    };
    let Ok(mut stmt) =
        conn.prepare("SELECT turn_id, sender, content FROM episodes WHERE session_id = ?1")
    else {
        return vec![];
    };
    let rows = stmt
        .query_map([session], |r| {
            Ok(Episode {
                turn_id: r.get(0)?,
                sender: r.get(1)?,
                content: r.get(2)?,
            })
        })
        .expect("query episodes");
    let mut out: Vec<Episode> = rows.map(|r| r.expect("row")).collect();
    out.sort_by_key(|e| index_of(&e.turn_id));
    out
}

/// `"<session>#<index>"` -> index. A row whose id does not carry one sorts
/// last, loudly (that is a bug, and it must not hide between the others).
fn index_of(turn_id: &str) -> i64 {
    turn_id
        .rsplit_once('#')
        .and_then(|(_, i)| i.parse::<i64>().ok())
        .unwrap_or(i64::MAX)
}

/// Polls the hive store's own `cell.db` until the session has `n` episodes.
/// 30 s failure marker, robust under cargo parallel load.
fn await_episodes(db: &std::path::Path, session: &str, n: usize) -> Vec<Episode> {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let rows = episodes(db, session);
        if rows.len() >= n {
            return rows;
        }
        if std::time::Instant::now() > deadline {
            panic!(
                "session {session:?} has {} of {n} episodes after 30s",
                rows.len()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Waits for the ledger replies of ONE drain chain, in the order the chain
/// produces them, and returns the operations it saw. A chain that drains
/// something is `insert` (park) / `select` (probe) / `insert` (mark); a chain
/// that has nothing to drain stops after the `select`. This is the positive
/// receipt the idempotence gate needs: the duplicate's probe was ANSWERED, and
/// only then is the row count read.
async fn chain_ops(rx: &mut mpsc::Receiver<Message>, n: usize) -> Vec<String> {
    let mut ops = Vec::new();
    while ops.len() < n {
        let m = recv_bounded(rx)
            .await
            .unwrap_or_else(|| panic!("only {} of {n} ledger replies arrived: {ops:?}", ops.len()));
        ops.push(hop_of(&m, "operation"));
    }
    ops
}

/// The adapter's own ledger, row by row: `(kind, drained_upto)`. Empty means
/// the adapter consumed nothing at all -- neither a parked day nor a mark.
fn ledger_rows(db: &std::path::Path) -> Vec<(String, i64)> {
    let Ok(conn) = rusqlite::Connection::open(db) else {
        return vec![];
    };
    let Ok(mut stmt) = conn.prepare("SELECT kind, drained_upto FROM drain_log") else {
        return vec![];
    };
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("query drain_log")
        .map(|r| r.expect("row"))
        .collect()
}

fn dlq_count(root: &std::path::Path) -> i64 {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    conn.query_row("SELECT COUNT(*) FROM dead_letters", [], |r| r.get(0))
        .expect("dead_letters count")
}

const DAY: [(&str, &str); 6] = [
    ("user", "my editor is helix"),
    ("assistant", "noted"),
    ("user", "and i cook keto"),
    ("assistant", "noted that too"),
    ("user", "what did i say first?"),
    ("assistant", "that your editor is helix"),
];

/// The provenance of a session's episodes, as the hive's writer left it:
/// `(turn_id, audience_set, channel)`, ordered by the index in the turn id.
fn provenance(db: &std::path::Path, session: &str) -> Vec<(String, String, String)> {
    let Ok(conn) = rusqlite::Connection::open(db) else {
        return vec![];
    };
    let Ok(mut stmt) = conn.prepare(
        "SELECT turn_id, COALESCE(audience_set, ''), COALESCE(channel, '') \
         FROM episodes WHERE session_id = ?1",
    ) else {
        return vec![];
    };
    let mut out: Vec<(String, String, String)> = stmt
        .query_map([session], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .expect("query provenance")
        .map(|r| r.expect("row"))
        .collect();
    out.sort_by_key(|(turn_id, _, _)| index_of(turn_id));
    out
}

/// The next message on `/sink` whose `hop.route` is `route`, skipping the
/// batch copies the observation lane also carries. 30 s failure marker.
async fn next_on_route(rx: &mut mpsc::Receiver<Message>, route: &str) -> Message {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        let m = tokio::time::timeout(left, rx.recv())
            .await
            .unwrap_or_else(|_| panic!("no message on route {route:?} within 30s"))
            .expect("the sink is still open");
        if hop_of(&m, "route") == route {
            return m;
        }
    }
}

// ============================================================== AUDIENCE GATE

/// GH #269, the positive half: an episode goes through the SHIPPED adapter into
/// the SHIPPED hive writer and the row lands **with** the participant set and
/// the room it was declared under -- byte for byte the set the door named, not
/// a wider one and not an empty one.
///
/// Nothing on the path between the door and the row promotes them: the port
/// edge carries only `session_id`, `turn_id` and `happened_at`. So this pins
/// the property the adapter really owes -- that it does not drop, rewrite or
/// widen a provenance it was handed, across a ledger round trip that leaves and
/// re-enters the cell twice (ADR-0002 E12).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_episode_lands_with_the_participant_set_it_was_spoken_to() {
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td);
    let (h, _sink_rx, _park_rx) = boot(&td).await;
    let db = td.path().join("main/memory/store/cell.db");

    h.send(day("s1", &DAY)).await;
    await_episodes(&db, "s1", DAY.len());

    // The writer normalises a participant set the way affinity does
    // (deduplicated, blanks dropped, sorted) and re-serialises it, so the
    // stored form is the sorted one -- the SAME two participants, in the
    // canonical order the rule compares them in.
    let stored = r#"["agent:scribe", "member:alex"]"#;
    let rows = provenance(&db, "s1");
    assert_eq!(rows.len(), DAY.len());
    for (i, (turn_id, audience_set, channel)) in rows.iter().enumerate() {
        assert_eq!(turn_id, &format!("s1#{i}"));
        assert_eq!(
            audience_set, stored,
            "episode {i} carries the round it was spoken to, not a wider one"
        );
        assert_eq!(channel, ROOM, "episode {i} carries the room it was said in");
    }
    assert_eq!(dlq_count(td.path()), 0, "nothing dead-letters on the way");
}

/// GH #269, the half that matters: a batch whose participant set is not
/// knowable is REFUSED at the adapter's own door -- and refused means nothing
/// was consumed. No episode leaves, no ledger row is written, and the reason
/// names the missing key.
///
/// This test can never go green through a default. A default would put a row in
/// `episodes` (the first assertion) and a mark in the ledger (the third), and a
/// `["*"]` default would additionally fail the explicit check below that no row
/// anywhere claims a universal audience.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_batch_without_a_participant_set_is_refused_and_not_consumed() {
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td);
    let (h, mut sink_rx, _park_rx) = boot(&td).await;
    let db = td.path().join("main/memory/store/cell.db");
    let ledger = td.path().join("main/drain/ledger/cell.db");

    h.send(day_with_provenance("s1", &DAY, &[("channel", ROOM)]))
        .await;

    let reject = next_on_route(&mut sink_rx, "reject").await;
    assert_eq!(
        hop_of(&reject, "reject_reason"),
        "missing_audience",
        "the refusal names the key that is missing, in the hive's own vocabulary"
    );
    assert_eq!(
        hop_of(&reject, "session_id"),
        "s1",
        "and it names the session, so the wiring that is wrong can be found"
    );

    assert!(
        episodes(&db, "s1").is_empty(),
        "an untagged episode is not written -- not with a guess, not with a blank"
    );
    assert!(
        !provenance(&db, "s1")
            .iter()
            .any(|(_, audience, _)| audience.contains('*')),
        "and above all not with a universal one: `*` is the value nothing can take back"
    );
    assert!(
        ledger_rows(&ledger).is_empty(),
        "the batch was not consumed either: no parked day, no mark. \
         A refusal that ate the batch would lose the day for good"
    );
}

/// The same door, the other key (contract ruling R3: a missing room is
/// `missing_channel` on every path, never `missing_audience`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_batch_without_a_room_is_refused_and_not_consumed() {
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td);
    let (h, mut sink_rx, _park_rx) = boot(&td).await;
    let db = td.path().join("main/memory/store/cell.db");
    let ledger = td.path().join("main/drain/ledger/cell.db");

    h.send(day_with_provenance(
        "s1",
        &DAY,
        &[("audience_set", AUDIENCE)],
    ))
    .await;

    let reject = next_on_route(&mut sink_rx, "reject").await;
    assert_eq!(hop_of(&reject, "reject_reason"), "missing_channel");
    assert!(episodes(&db, "s1").is_empty());
    assert!(ledger_rows(&ledger).is_empty());
}

/// The reason the refusal belongs at THIS door and not one hop later: a batch
/// the adapter refused is still deliverable. Fix the wiring, hand the same day
/// over again, and all of it lands.
///
/// Refused downstream instead, the day would be gone: the adapter marks a
/// session drained in the same multi-send that emits its episodes, so a hive
/// that refuses them leaves a mark behind that no later delivery can get past.
/// That is the silent half of #269, and this is the test that holds it shut.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_day_still_lands_whole_once_the_wiring_carries_its_audience() {
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td);
    let (h, mut sink_rx, _park_rx) = boot(&td).await;
    let db = td.path().join("main/memory/store/cell.db");

    // First delivery: the door named no participant set.
    h.send(day_with_provenance("s1", &DAY, &[("channel", ROOM)]))
        .await;
    let refused = next_on_route(&mut sink_rx, "reject").await;
    assert_eq!(hop_of(&refused, "reject_reason"), "missing_audience");
    assert!(episodes(&db, "s1").is_empty());

    // Second delivery: the same day, over an edge that carries the round.
    h.send(day("s1", &DAY)).await;
    let rows = await_episodes(&db, "s1", DAY.len());

    assert_eq!(
        rows.len(),
        DAY.len(),
        "every turn of the refused day is still there to be drained"
    );
    for (i, (origin, text)) in DAY.iter().enumerate() {
        assert_eq!(rows[i].turn_id, format!("s1#{i}"));
        assert_eq!(rows[i].sender, *origin);
        assert_eq!(rows[i].content, *text);
    }
    assert!(
        provenance(&db, "s1")
            .iter()
            .all(|(_, audience, channel)| audience.contains("member:alex") && channel == ROOM),
        "and it lands under the round that was finally declared"
    );
}

// ================================================================ COUNT GATE

/// R-MD-2 no. 1, end to end: a closed session with `turn_count` turns leaves
/// exactly `turn_count` new rows in the hive's `episodes` table -- every turn
/// whole, the sender kept, under the id minted from the session and the index.
/// Nothing is judged, merged, capped or dropped between the batch and the store.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_closed_session_leaves_exactly_turn_count_episodes_in_the_hive() {
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td);
    let (h, mut sink_rx, _park_rx) = boot(&td).await;
    let db = td.path().join("main/memory/store/cell.db");

    h.send(day("s1", &DAY)).await;

    // The left-hand side of the gate, read off the real batch message.
    let batch = recv_bounded(&mut sink_rx).await.expect("the batch");
    let turn_count: usize = hop_of(&batch, "turn_count").parse().expect("turn_count");
    assert_eq!(turn_count, DAY.len());

    // The right-hand side, read out of the hive store's own cell.db.
    let rows = await_episodes(&db, "s1", turn_count);
    assert_eq!(rows.len(), turn_count, "turn_count = new episodes rows");
    for (i, (origin, text)) in DAY.iter().enumerate() {
        assert_eq!(
            rows[i],
            Episode {
                turn_id: format!("s1#{i}"),
                sender: origin.to_string(),
                content: text.to_string(),
            },
            "episode {i} is the {i}th turn of the day, whole"
        );
    }
    assert_eq!(dlq_count(td.path()), 0, "nothing dead-letters on the way");
}

// ========================================================== IDEMPOTENCE GATE

/// R-MD-2 no. 2, end to end. Three deliveries, each awaited on its own receipt:
/// the day, the SAME day again, and the day grown by one exchange. The
/// duplicate's chain ends after its `select` -- that select is captured, so the
/// row count that follows it is measured after the duplicate had its chance,
/// not after a sleep.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_batch_twice_moves_no_row() {
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td);
    let (h, _sink_rx, mut park_rx) = boot(&td).await;
    let db = td.path().join("main/memory/store/cell.db");

    h.send(day("s1", &DAY)).await;
    assert_eq!(
        chain_ops(&mut park_rx, 3).await,
        vec!["insert", "select", "insert"],
        "the first chain parks, probes and marks"
    );
    await_episodes(&db, "s1", DAY.len());

    // The duplicate: byte for byte the batch that just went through.
    h.send(day("s1", &DAY)).await;
    assert_eq!(
        chain_ops(&mut park_rx, 2).await,
        vec!["insert", "select"],
        "the duplicate parks and probes -- and stops there, with no mark, \
         because it has nothing to mark"
    );
    assert_eq!(
        episodes(&db, "s1").len(),
        DAY.len(),
        "the row count did not move: a skip, not a second insert"
    );

    // And the session that GREW is drained from where it stopped.
    let mut grown = DAY.to_vec();
    grown.push(("user", "and my kids?"));
    grown.push(("assistant", "mika and noa"));
    h.send(day("s1", &grown)).await;
    let rows = await_episodes(&db, "s1", grown.len());

    assert_eq!(rows.len(), grown.len(), "6 + 0 + 2 = 8");
    let ids: Vec<&str> = rows.iter().map(|r| r.turn_id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "s1#0", "s1#1", "s1#2", "s1#3", "s1#4", "s1#5", "s1#6", "s1#7"
        ],
        "every turn appears exactly once, under its deterministic id"
    );
    assert_eq!(rows[6].content, "and my kids?");
    assert_eq!(rows[7].content, "mika and noa");
    assert_eq!(dlq_count(td.path()), 0, "nothing dead-letters on the way");
}

/// Two sessions do not see each other's marks: the ledger is read per session,
/// so a drained `s1` never suppresses `s2` and never re-numbers it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_sessions_are_drained_independently() {
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td);
    let (h, _sink_rx, mut park_rx) = boot(&td).await;
    let db = td.path().join("main/memory/store/cell.db");

    h.send(day("s1", &DAY)).await;
    chain_ops(&mut park_rx, 3).await;
    await_episodes(&db, "s1", DAY.len());

    h.send(day("s2", &DAY[..2])).await;
    chain_ops(&mut park_rx, 3).await;
    let second = await_episodes(&db, "s2", 2);

    assert_eq!(second.len(), 2);
    assert_eq!(second[0].turn_id, "s2#0");
    assert_eq!(second[1].turn_id, "s2#1");
    assert_eq!(episodes(&db, "s1").len(), DAY.len(), "s1 is untouched");
    assert_eq!(dlq_count(td.path()), 0);
}

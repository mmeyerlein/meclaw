//! P4 integration tests: the graph leg (`traverse`) and the vector leg
//! (`similar`) of the store cell — memory-spec A.2.4 / A.2.5.
//!
//! Everything here runs against real rows. Guards are proven by counts, cycle
//! elimination by path contents, and the injection matrix by a positive receipt
//! (the fixture tables are still there, unchanged), never by "an error appeared".

use meclaw_cells::store::ops::dispatch;
use meclaw_cells::store::query::hamming::register;
use meclaw_core::serde_json::{Value, json};

// ---------------------------------------------------------------- helpers

/// Pack bytes into the encoding the memory hive's `embed` cell produces:
/// standard base64 of the packed sign bits (BIN_VERSION v1).
fn b64(bytes: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let idx = [(n >> 18) & 63, (n >> 12) & 63, (n >> 6) & 63, n & 63];
        for (i, x) in idx.iter().enumerate() {
            if i <= chunk.len() {
                out.push(A[*x as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// An `embeddings` table shaped exactly like the memory-hive template's
/// (`builder/templates/memory-hive/store/config.json`), holding one generation
/// of 2-byte vectors plus the NULL backfill queue.
fn embeddings_fixture() -> rusqlite::Connection {
    let c = rusqlite::Connection::open_in_memory().unwrap();
    register(&c).unwrap();
    c.execute(
        "CREATE TABLE embeddings (id TEXT, owner_table TEXT, owner_id TEXT, model_id TEXT, \
         dim INTEGER, binarization_version TEXT, blob TEXT, status TEXT, created_at TEXT)",
        [],
    )
    .unwrap();
    // query vector is 0x00 0x00; distances: near 1 bit, mid 3 bits, far 9 bits
    insert_embedding(&c, "r-near", "facts", Some(&[0b0000_0001, 0x00]));
    insert_embedding(&c, "r-mid", "facts", Some(&[0b0000_0111, 0x00]));
    insert_embedding(&c, "r-far", "facts", Some(&[0b1111_1111, 0b0000_0001]));
    // the backfill queue of a store whose embed lane was down (memory-spec B.1.1)
    insert_embedding(&c, "q-1", "facts", None);
    insert_embedding(&c, "q-2", "episodes", None);
    c
}

/// Insert one `embeddings` row of the active generation, or a queued row with
/// no vector at all (`blob NULL`, `status "queued"`).
fn insert_embedding(c: &rusqlite::Connection, id: &str, owner_table: &str, blob: Option<&[u8]>) {
    c.execute(
        "INSERT INTO embeddings (id, owner_table, owner_id, model_id, dim, \
         binarization_version, blob, status, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        rusqlite::params![
            id,
            owner_table,
            format!("owner-{id}"),
            if blob.is_some() { "qwen3-emb-0.6b" } else { "" },
            if blob.is_some() { 16 } else { 0 },
            if blob.is_some() { "v1" } else { "" },
            blob.map(b64),
            if blob.is_some() { "ready" } else { "queued" },
            "2026-08-08T00:00:00Z",
        ],
    )
    .unwrap();
}

// ------------------------------------------- live-cell helpers (lifecycle)

type Mailbox = tokio::sync::mpsc::Sender<meclaw_core::Message>;
type Emissions = tokio::sync::mpsc::Receiver<meclaw_core::CellEmission>;

/// Spawn a store cell through its factory and wake it — the production path,
/// including the connection setup that has to register `hamming`. Returns the
/// mailbox, the emission stream and the colony inbox receiver (kept alive so
/// the cell's colony channel does not close under it).
fn wake_store(
    cell_dir: &std::path::Path,
    params: &Value,
) -> (
    Mailbox,
    Emissions,
    tokio::sync::mpsc::Receiver<meclaw_colony::ColonyMsg>,
) {
    use meclaw_colony::{CellFactory, SpawnedCellKind};
    let (otx, orx) = tokio::sync::mpsc::channel(32);
    let (itx, irx) = tokio::sync::mpsc::channel(32);
    let spawned = std::sync::Arc::new(meclaw_cells::store::StoreCellFactory)
        .spawn_cell(
            meclaw_core::Path::new("/store"),
            params.clone(),
            otx,
            cell_dir.to_path_buf(),
            meclaw_colony::ContractView::default(),
            itx,
            None,
            0,
            None,
            None,
            32,
        )
        .unwrap();
    let SpawnedCellKind::Dormant {
        sender,
        receiver,
        wake,
        ..
    } = spawned
    else {
        unreachable!("the store factory is lazy");
    };
    wake(receiver);
    (sender, orx, irx)
}

/// Drive the crash-restart path (`RespawnFn`) against an existing `cell.db`.
fn respawn_store(
    cell_dir: &std::path::Path,
    params: &Value,
) -> (
    Mailbox,
    Emissions,
    tokio::sync::mpsc::Receiver<meclaw_colony::ColonyMsg>,
) {
    use meclaw_colony::{CellFactory, SpawnedCellKind};
    let (otx, orx) = tokio::sync::mpsc::channel(32);
    let (itx, irx) = tokio::sync::mpsc::channel(32);
    let spawned = std::sync::Arc::new(meclaw_cells::store::StoreCellFactory)
        .spawn_cell(
            meclaw_core::Path::new("/store"),
            params.clone(),
            otx,
            cell_dir.to_path_buf(),
            meclaw_colony::ContractView::default(),
            itx,
            None,
            0,
            None,
            None,
            32,
        )
        .unwrap();
    let SpawnedCellKind::Dormant { respawn, .. } = spawned else {
        unreachable!("the store factory is lazy");
    };
    let (sender, _join, _peace, _backstop) = respawn();
    (sender, orx, irx)
}

/// Send one store op as a `tool_call` turn to a live cell.
async fn send(mailbox: &Mailbox, args: &Value) {
    let body = json!({"messages":[{"origin":"assistant","type":"tool_call",
                                   "text": args.to_string(), "id":"call-1"}]});
    let msg = meclaw_core::MessageBuilder::new(meclaw_core::Path::new("/store"))
        .body(meclaw_core::Body::Inline(body))
        .reply_to(meclaw_core::Path::new("/sink"))
        .build();
    mailbox.send(msg).await.unwrap();
}

/// Fire one `similar` against a live cell and return the ranked ids.
async fn similar_ids(mailbox: &Mailbox, emissions: &mut Emissions) -> Vec<String> {
    send(
        mailbox,
        &json!({"operation":"similar","table":"embeddings","columns":["id"],
                "vector_column":"blob","vector": b64(&[0x00, 0x00])}),
    )
    .await;
    let em = emissions.recv().await.unwrap();
    assert!(
        em.content["header"].get("error_code").is_none(),
        "similar failed on a live cell: {:?}",
        em.content
    );
    let text = em.content["messages"][0]["text"].as_str().unwrap();
    let payload: Value = meclaw_core::serde_json::from_str(text).unwrap();
    ids(&payload, "id")
}

fn ids(out: &Value, key: &str) -> Vec<String> {
    out.as_array()
        .unwrap()
        .iter()
        .map(|r| r[key].as_str().unwrap().to_string())
        .collect()
}

// --------------------------------------------------------------- traverse

/// An `entity_edges` table shaped like the memory-hive template's, plus a
/// helper to add edges.
fn edges_fixture() -> rusqlite::Connection {
    let c = rusqlite::Connection::open_in_memory().unwrap();
    c.execute(
        "CREATE TABLE entity_edges (id TEXT, src_entity TEXT, dst_entity TEXT, edge_kind TEXT, \
         weight INTEGER, episode_id TEXT, valid_from TEXT, valid_until TEXT)",
        [],
    )
    .unwrap();
    c
}

fn edge(c: &rusqlite::Connection, src: &str, dst: &str, kind: &str, weight: i64) {
    edge_until(c, src, dst, kind, weight, None);
}

fn edge_until(
    c: &rusqlite::Connection,
    src: &str,
    dst: &str,
    kind: &str,
    weight: i64,
    valid_until: Option<&str>,
) {
    c.execute(
        "INSERT INTO entity_edges (id, src_entity, dst_entity, edge_kind, weight, episode_id, \
         valid_from, valid_until) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        rusqlite::params![
            format!("{src}->{dst}"),
            src,
            dst,
            kind,
            weight,
            format!("ep-{src}"),
            "2026-01-01T00:00:00Z",
            valid_until
        ],
    )
    .unwrap();
}

/// Convenience: run a traverse and return its payload object.
fn traverse(c: &rusqlite::Connection, extra: Value) -> Value {
    let mut args = json!({"operation":"traverse","table":"entity_edges",
                          "src":"src_entity","dst":"dst_entity"});
    for (k, v) in extra.as_object().unwrap() {
        args[k] = v.clone();
    }
    let out = dispatch(c, &args).unwrap();
    assert_eq!(out.error_code, None, "{:?}", out.error_text);
    out.payload
}

/// Find the (single) path ending in `node`. Tests address paths by their end
/// node, never by position within a depth — that order is SQLite's, not ours.
fn find_path<'a>(p: &'a Value, node: &str) -> &'a Value {
    p["paths"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["node"] == node)
        .unwrap_or_else(|| panic!("no path to {node} in {:?}", p["paths"]))
}

fn path_of(p: &Value, i: usize) -> Vec<String> {
    p["paths"][i]["path"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect()
}

/// The graph leg returns PATHS, not reachability: node, depth, the nodes walked
/// through, the last edge's attributes and the accumulated weight — everything a
/// `code` cell needs to score (memory-spec A.2.4).
#[test]
fn traverse_returns_paths_with_depth_edge_attributes_and_accumulated_weight() {
    let c = edges_fixture();
    edge(&c, "a", "b", "entity", 2);
    edge(&c, "b", "c", "entity", 3);
    edge(&c, "a", "c", "causal", 1);

    let p = traverse(
        &c,
        json!({"start":"a","kind":"edge_kind","weight":"weight",
               "columns":["episode_id"],"max_depth":2}),
    );
    assert_eq!(p["truncated"], false);
    assert_eq!(p["max_depth"], 2);
    assert_eq!(p["max_nodes"], 200);
    let paths = p["paths"].as_array().unwrap();
    assert_eq!(paths.len(), 3, "a->b, a->c, a->b->c");

    // breadth first: the two one-hop paths come before the two-hop path. The
    // order among equal depths belongs to SQLite and is not asserted.
    let depths: Vec<i64> = paths.iter().map(|r| r["depth"].as_i64().unwrap()).collect();
    assert_eq!(depths, vec![1, 1, 2]);

    let b = find_path(&p, "b");
    assert_eq!(b["depth"], 1);
    assert_eq!(b["weight_sum"], 2);
    assert_eq!(b["edge"]["kind"], "entity");
    assert_eq!(b["edge"]["weight"], 2);
    assert_eq!(b["edge"]["episode_id"], "ep-a");

    // Both routes to c survive — the direct causal edge (weight 1) and the
    // two-hop entity route (weight 2+3). A global visited set would have dropped
    // one of them; path-local cycle elimination keeps both (plan R2).
    let to_c: Vec<&Value> = paths.iter().filter(|r| r["node"] == "c").collect();
    assert_eq!(to_c.len(), 2, "both routes to c are reported");
    let direct = to_c.iter().find(|r| r["depth"] == 1).unwrap();
    assert_eq!(direct["edge"]["kind"], "causal");
    assert_eq!(direct["weight_sum"], 1);
    let via_b = to_c.iter().find(|r| r["depth"] == 2).unwrap();
    assert_eq!(via_b["weight_sum"], 5, "2 + 3 accumulated over the path");
    assert_eq!(
        via_b["path"].as_array().unwrap(),
        &vec![json!("a"), json!("b"), json!("c")]
    );
}

/// Without the optional roles a path carries neither weight nor edge object —
/// nothing is invented.
#[test]
fn traverse_omits_weight_and_edge_when_no_roles_are_declared() {
    let c = edges_fixture();
    edge(&c, "a", "b", "entity", 2);
    let p = traverse(&c, json!({"start":"a"}));
    let row = &p["paths"][0];
    assert_eq!(row["node"], "b");
    assert!(row.get("weight_sum").is_none(), "no weight role, no sum");
    assert!(row.get("edge").is_none(), "no attributes requested");
}

/// The start node itself is not a path — it has no edge and no weight, and the
/// caller already knows it.
#[test]
fn traverse_does_not_report_the_start_node() {
    let c = edges_fixture();
    edge(&c, "a", "b", "entity", 1);
    let p = traverse(&c, json!({"start":"a"}));
    assert_eq!(p["paths"].as_array().unwrap().len(), 1);
    assert_eq!(p["paths"][0]["node"], "b");
}

/// `where` filters the EDGE rows of the recursive step — the operator set of P3
/// applies unchanged.
#[test]
fn traverse_filters_edges_by_where() {
    let c = edges_fixture();
    edge(&c, "a", "b", "entity", 1);
    edge(&c, "a", "x", "semantic", 1);
    edge_until(&c, "a", "old", "entity", 1, Some("2026-02-01T00:00:00Z"));

    let p = traverse(
        &c,
        json!({"start":"a","where":{"edge_kind":{"in":["entity","causal"]},
                                    "valid_until":{"is_null":true}}}),
    );
    let nodes: Vec<String> = p["paths"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["node"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        nodes,
        vec!["b"],
        "semantic edge and expired edge are filtered"
    );
}

/// Several start nodes in one walk — each anchor is a bound value.
#[test]
fn traverse_accepts_several_start_nodes() {
    let c = edges_fixture();
    edge(&c, "a", "b", "entity", 1);
    edge(&c, "x", "y", "entity", 1);
    let p = traverse(&c, json!({"start":["a","x"],"max_depth":1}));
    assert_eq!(p["paths"].as_array().unwrap().len(), 2);
}

// ------------------------------------------------------- traverse: guards

/// `max_depth` alone: a chain of four nodes answers with exactly as many hops
/// as the caller allowed — the guard limits, and it limits precisely there.
#[test]
fn max_depth_bounds_the_walk_hop_by_hop() {
    let c = edges_fixture();
    edge(&c, "a", "b", "entity", 1);
    edge(&c, "b", "c", "entity", 1);
    edge(&c, "c", "d", "entity", 1);
    edge(&c, "d", "e", "entity", 1);
    for (depth, want) in [(1, 1), (2, 2), (3, 3), (4, 4), (5, 4)] {
        let p = traverse(&c, json!({"start":"a","max_depth":depth}));
        assert_eq!(
            p["paths"].as_array().unwrap().len(),
            want,
            "max_depth {depth} must yield {want} paths"
        );
        assert_eq!(p["truncated"], false);
    }
}

/// `max_nodes` alone: a hub with 50 outgoing edges is cut to the cap, and the
/// cut is REPORTED. Silent truncation is the failure mode this flag exists for.
#[test]
fn max_nodes_caps_the_fanout_and_reports_the_cut() {
    let c = edges_fixture();
    for i in 0..50 {
        edge(&c, "hub", &format!("n{i}"), "entity", 1);
    }
    let capped = traverse(&c, json!({"start":"hub","max_depth":1,"max_nodes":10}));
    assert_eq!(capped["paths"].as_array().unwrap().len(), 10);
    assert_eq!(capped["truncated"], true, "the cut must be visible");
    assert_eq!(capped["max_nodes"], 10, "the guard is echoed back");

    let full = traverse(&c, json!({"start":"hub","max_depth":1}));
    assert_eq!(full["paths"].as_array().unwrap().len(), 50);
    assert_eq!(full["truncated"], false, "no cut, no flag");
}

/// A cyclic graph terminates, and no path visits a node twice. The same node
/// MAY appear on different paths — that is the point of path-local cycle
/// elimination (plan R2): a weaker one-hop path must not hide a stronger
/// two-hop one.
#[test]
fn cycles_terminate_without_repeating_a_node_within_one_path() {
    let c = edges_fixture();
    edge(&c, "a", "b", "entity", 1);
    edge(&c, "b", "c", "entity", 1);
    edge(&c, "c", "a", "entity", 1); // closes the cycle
    edge(&c, "b", "a", "entity", 1); // and a shorter one back

    let p = traverse(&c, json!({"start":"a","max_depth":5}));
    let paths = p["paths"].as_array().unwrap();
    assert!(!paths.is_empty());
    assert_eq!(
        p["truncated"], false,
        "the walk ended on its own, not on the cap"
    );
    for (i, _) in paths.iter().enumerate() {
        let nodes = path_of(&p, i);
        let mut seen = std::collections::HashSet::new();
        for n in &nodes {
            assert!(seen.insert(n.clone()), "node {n} repeats in path {nodes:?}");
        }
    }
    // c is reachable both as a->b->c — and only that way here; b appears once
    // per distinct path, which is what keeps alternative routes visible.
    let reached: Vec<String> = paths
        .iter()
        .map(|r| r["node"].as_str().unwrap().to_string())
        .collect();
    assert!(reached.contains(&"c".to_string()));
}

/// Both guards under load: a complete graph of eight nodes has more paths than
/// any cap allows, and every one of them would loop without cycle elimination.
/// Terminating AND cutting AND reporting is the whole contract in one test.
#[test]
fn cycle_and_high_fanout_together_stay_inside_both_guards() {
    let c = edges_fixture();
    let nodes: Vec<String> = (0..8).map(|i| format!("n{i}")).collect();
    for a in &nodes {
        for b in &nodes {
            if a != b {
                edge(&c, a, b, "entity", 1);
            }
        }
    }
    let p = traverse(
        &c,
        json!({"start":"n0","max_depth":5,"max_nodes":100,"weight":"weight"}),
    );
    let paths = p["paths"].as_array().unwrap();
    assert_eq!(paths.len(), 100, "cut exactly at the cap");
    assert_eq!(p["truncated"], true);
    for (i, row) in paths.iter().enumerate() {
        let nodes = path_of(&p, i);
        assert!(nodes.len() <= 6, "at most max_depth+1 nodes: {nodes:?}");
        let unique: std::collections::HashSet<&String> = nodes.iter().collect();
        assert_eq!(unique.len(), nodes.len(), "no repeat inside {nodes:?}");
        assert_eq!(
            row["weight_sum"].as_i64().unwrap(),
            row["depth"].as_i64().unwrap(),
            "weight 1 per hop accumulates to the depth"
        );
    }
}

// -------------------------------------------------- traverse: query timeout

/// A dense graph whose traversal is far too large to finish inside the
/// operation timeout: 12 nodes, all connected, depth 5 — the recursion has to
/// produce 5000 paths before the cap stops it.
fn dense_graph(c: &rusqlite::Connection, n: usize) {
    for a in 0..n {
        for b in 0..n {
            if a != b {
                edge(c, &format!("n{a}"), &format!("n{b}"), "entity", 1);
            }
        }
    }
}

/// `query_timeout_ms` (concept A) must reach INTO a running recursive CTE, not
/// just around it — the `InterruptHandle` is the only thing that can cancel a
/// walk that is already inside SQLite.
///
/// Timing discriminator: 1 ms against a walk measured at ~64 ms on this machine
/// (the generous-budget run below reports its own `duration_ms`). The margin is
/// ~60x and one-sided — the walk can only ever get slower under load, never fast
/// enough to beat a 1 ms budget, so a false pass is not reachable.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn query_timeout_interrupts_a_running_recursive_cte() {
    let td = tempfile::TempDir::new().unwrap();
    let cell_dir = td.path().to_path_buf();
    let schema = json!({"entity_edges":{"id":"text","src_entity":"text","dst_entity":"text",
                                        "edge_kind":"text","weight":"int","episode_id":"text",
                                        "valid_from":"text","valid_until":"text"}});

    // Fill the cell.db before the cell wakes on it.
    {
        let c = meclaw_colony::persist::open_or_create_cell_db(&cell_dir.join("cell.db")).unwrap();
        let mut schema_map = std::collections::BTreeMap::new();
        schema_map.insert(
            "entity_edges".to_string(),
            schema["entity_edges"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
                .collect::<std::collections::BTreeMap<_, _>>(),
        );
        meclaw_cells::store::ddl::apply_schema_ddl(&c, &schema_map).unwrap();
        dense_graph(&c, 12);
    }

    let walk = json!({"operation":"traverse","table":"entity_edges",
                      "src":"src_entity","dst":"dst_entity","start":"n0",
                      "max_depth":5,"max_nodes":5000});

    // 1. with a 1 ms budget the walk is cut short — as an error message, not a
    //    tool_result, exactly like every other query timeout.
    let (sender, mut orx, _keep) =
        wake_store(&cell_dir, &json!({"schema": schema, "query_timeout_ms": 1}));
    send(&sender, &walk).await;
    let em = orx.recv().await.unwrap();
    assert_eq!(em.content["header"]["error_code"], "query_timeout");
    assert_eq!(em.content["header"]["finish_reason"], "error");
    drop(sender);

    // 2. positive counter-receipt: with a generous budget the same walk answers.
    let (sender2, mut orx2, _keep2) = wake_store(
        &cell_dir,
        &json!({"schema": schema, "query_timeout_ms": 30000}),
    );
    send(&sender2, &walk).await;
    let em2 = orx2.recv().await.unwrap();
    assert!(
        em2.content["header"].get("error_code").is_none(),
        "the same walk must succeed with a generous timeout: {:?}",
        em2.content["header"]
    );
    assert_eq!(em2.content["header"]["operation"], "traverse");
    assert_eq!(em2.content["header"]["rows_affected"], 5000);
}

// ------------------------------------------------ demo: memory-hive schema

/// Build a `cell.db` from the memory hive template's OWN `params.schema` — the
/// file is read, not transcribed, so a schema change in the template surfaces
/// here instead of silently drifting apart from the ops.
fn memory_hive_db() -> rusqlite::Connection {
    let cfg = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../builder/templates/memory-hive/store/config.json");
    let raw: Value =
        meclaw_core::serde_json::from_str(&std::fs::read_to_string(&cfg).expect("template config"))
            .expect("template config is JSON");
    let schema: std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>> =
        raw["params"]["schema"]
            .as_object()
            .expect("params.schema")
            .iter()
            .map(|(t, cols)| {
                (
                    t.clone(),
                    cols.as_object()
                        .unwrap()
                        .iter()
                        .map(|(c, ty)| (c.clone(), ty.as_str().unwrap().to_string()))
                        .collect(),
                )
            })
            .collect();
    let c = rusqlite::Connection::open_in_memory().unwrap();
    register(&c).unwrap();
    meclaw_cells::store::ddl::apply_schema_ddl(&c, &schema).unwrap();
    c
}

/// Insert through the op layer, not through raw SQL — the demo exercises the
/// same path a `code` cell would take.
fn op_insert(c: &rusqlite::Connection, table: &str, row: Value) {
    let out = dispatch(c, &json!({"operation":"insert","table":table,"row":row})).unwrap();
    assert_eq!(out.error_code, None, "{:?}", out.error_text);
    assert_eq!(out.rows_affected, 1);
}

/// The graph leg on the real `entity_edges` schema, over a graph that contains a
/// cycle, an expired edge and a second edge kind: terminates, returns correct
/// paths with accumulated weight, honours the temporal filter, and cuts on the
/// guards.
#[test]
fn demo_traverse_over_entity_edges_with_a_cycle() {
    let c = memory_hive_db();
    let edge_row = |id: &str, src: &str, dst: &str, kind: &str, w: i64, until: Value| {
        op_insert(
            &c,
            "entity_edges",
            json!({"id":id,"src_entity":src,"dst_entity":dst,"edge_kind":kind,"weight":w,
                   "episode_id":format!("ep-{id}"),"valid_from":"2026-01-01T00:00:00Z",
                   "valid_until":until}),
        );
    };
    edge_row("e1", "node-a", "node-b", "entity", 5, Value::Null);
    edge_row("e2", "node-b", "node-c", "entity", 2, Value::Null);
    edge_row("e3", "node-c", "node-a", "entity", 1, Value::Null); // closes the cycle
    edge_row("e4", "node-a", "node-d", "causal", 3, Value::Null);
    edge_row(
        "e5",
        "node-a",
        "node-e",
        "entity",
        9,
        json!("2026-02-01T00:00:00Z"),
    ); // expired

    // 1. live edges only, both kinds, depth 3 — the cycle must not spin.
    let p = traverse_on(
        &c,
        json!({"start":"node-a","kind":"edge_kind","weight":"weight",
               "columns":["episode_id"],"max_depth":3,
               "where":{"valid_until":{"is_null":true}}}),
    );
    assert_eq!(
        p["truncated"], false,
        "the walk ended on its own, not on a cap"
    );
    let reached: std::collections::BTreeSet<String> = p["paths"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["node"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        reached,
        ["node-c", "node-d", "node-b"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        "every live edge is followed; the cycle adds no new node"
    );
    assert!(
        !reached.contains("node-e"),
        "the expired edge must be filtered out by the temporal predicate"
    );
    // Depth is non-decreasing (the CTE walks breadth first); the order WITHIN a
    // depth is SQLite's and deliberately not part of the contract — ranking is
    // the caller's job (memory-spec A.2.4), so the test does not pin it.
    let depths: Vec<i64> = p["paths"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["depth"].as_i64().unwrap())
        .collect();
    assert!(
        depths.windows(2).all(|w| w[0] <= w[1]),
        "breadth first: {depths:?}"
    );

    // The two-hop path carries its accumulated weight and its last edge.
    let node_c = find_path(&p, "node-c");
    assert_eq!(node_c["depth"], 2);
    assert_eq!(
        node_c["weight_sum"], 7,
        "5 + 2 along node-a->node-b->node-c"
    );
    assert_eq!(node_c["edge"]["kind"], "entity");
    assert_eq!(node_c["edge"]["episode_id"], "ep-e2");

    // The cycle-closing edge node-c->node-a is pruned: node-a is already on that
    // path (as its start), and a path never visits a node twice (plan R2).
    for i in 0..p["paths"].as_array().unwrap().len() {
        let nodes = path_of(&p, i);
        let unique: std::collections::HashSet<&String> = nodes.iter().collect();
        assert_eq!(unique.len(), nodes.len(), "node repeats in {nodes:?}");
    }
    assert!(
        p["paths"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["depth"].as_i64().unwrap() <= 2),
        "depth 3 would require re-entering node-a — correctly pruned"
    );

    // 2. the caller can narrow to one edge kind — P3 operators, unchanged.
    let causal = traverse_on(
        &c,
        json!({"start":"node-a","where":{"edge_kind":"causal","valid_until":{"is_null":true}}}),
    );
    assert_eq!(causal["paths"].as_array().unwrap().len(), 1);
    assert_eq!(causal["paths"][0]["node"], "node-d");

    // 3. guards bite on this graph too.
    let capped = traverse_on(&c, json!({"start":"node-a","max_depth":3,"max_nodes":2}));
    assert_eq!(capped["paths"].as_array().unwrap().len(), 2);
    assert_eq!(capped["truncated"], true);
    let shallow = traverse_on(&c, json!({"start":"node-a","max_depth":1}));
    assert_eq!(
        shallow["paths"].as_array().unwrap().len(),
        3,
        "three live 1-hop edges"
    );
}

/// Same walk, but with the table name filled in from the memory-hive schema.
fn traverse_on(c: &rusqlite::Connection, extra: Value) -> Value {
    let mut args = json!({"operation":"traverse","table":"entity_edges",
                          "src":"src_entity","dst":"dst_entity"});
    for (k, v) in extra.as_object().unwrap() {
        args[k] = v.clone();
    }
    let out = dispatch(c, &args).unwrap();
    assert_eq!(out.error_code, None, "{:?}", out.error_text);
    out.payload
}

/// The vector leg on the real `embeddings` schema, with two embedding
/// generations side by side plus the backfill queue: the caller's `model_id`
/// filter is what makes the query well-defined, and leaving it out is loud.
#[test]
fn demo_similar_over_embeddings_with_two_generations() {
    let c = memory_hive_db();
    let emb = |id: &str, model: &str, dim: i64, blob: Option<Vec<u8>>, status: &str| {
        op_insert(
            &c,
            "embeddings",
            json!({"id":id,"owner_table":"facts","owner_id":format!("f-{id}"),
                   "model_id":model,"dim":dim,"binarization_version":"v1",
                   "blob": blob.map(|b| b64(&b)),"status":status,
                   "created_at":"2026-08-08T00:00:00Z"}),
        );
    };
    // active generation: 2-byte vectors, known distances to 0x0000
    emb(
        "a1",
        "qwen3-emb-0.6b",
        16,
        Some(vec![0b0000_0001, 0x00]),
        "ready",
    );
    emb(
        "a2",
        "qwen3-emb-0.6b",
        16,
        Some(vec![0b0000_0111, 0x00]),
        "ready",
    );
    emb(
        "a3",
        "qwen3-emb-0.6b",
        16,
        Some(vec![0xFF, 0b0000_0001]),
        "ready",
    );
    // previous generation: 4-byte vectors, still on disk (no-delete)
    emb(
        "o1",
        "legacy-emb",
        32,
        Some(vec![0x00, 0x00, 0x00, 0x00]),
        "ready",
    );
    // backfill queue: written, not embedded yet
    emb("q1", "", 0, None, "queued");

    let query = b64(&[0x00, 0x00]);

    // 1. with the generation filter: a clean, ordered answer.
    let out = dispatch(
        &c,
        &json!({"operation":"similar","table":"embeddings","columns":["id","owner_id"],
                "vector_column":"blob","vector": query,
                "where":{"model_id":"qwen3-emb-0.6b","status":"ready","owner_table":"facts"},
                "limit":2}),
    )
    .unwrap();
    assert_eq!(out.error_code, None, "{:?}", out.error_text);
    assert_eq!(ids(&out.payload, "id"), vec!["a1", "a2"]);
    assert_eq!(out.payload[0]["owner_id"], "f-a1");
    assert_eq!(out.payload[0]["distance"], 1);
    assert_eq!(out.payload[1]["distance"], 3);

    // 2. without it: the generation mix is a loud error, not a wrong ranking.
    let mixed = dispatch(
        &c,
        &json!({"operation":"similar","table":"embeddings","columns":["id"],
                "vector_column":"blob","vector": query,"where":{"status":"ready"}}),
    )
    .unwrap();
    assert_eq!(mixed.error_code, Some("sql_error"));
    assert!(
        mixed
            .error_text
            .as_deref()
            .unwrap_or("")
            .contains("length mismatch"),
        "got {:?}",
        mixed.error_text
    );

    // 3. the queue never shows up, with or without a status filter (R9).
    let all_active = dispatch(
        &c,
        &json!({"operation":"similar","table":"embeddings","columns":["id"],
                "vector_column":"blob","vector": query,
                "where":{"model_id":"qwen3-emb-0.6b"}}),
    )
    .unwrap();
    assert_eq!(all_active.error_code, None, "{:?}", all_active.error_text);
    assert_eq!(ids(&all_active.payload, "id"), vec!["a1", "a2", "a3"]);
}

// ------------------------------------------------------- injection matrix

/// Two tables with known content plus the graph/vector tables the new ops read.
/// `keep` is the canary: it must exist, untouched, after every hostile payload.
fn injection_fixture() -> rusqlite::Connection {
    let c = rusqlite::Connection::open_in_memory().unwrap();
    register(&c).unwrap();
    c.execute_batch(
        "CREATE TABLE entity_edges (src_entity TEXT, dst_entity TEXT, edge_kind TEXT, \
                                    weight INTEGER, episode_id TEXT);\
         CREATE TABLE embeddings (id TEXT, model_id TEXT, blob TEXT);\
         CREATE TABLE keep (id INTEGER);\
         INSERT INTO entity_edges VALUES ('a','b','entity',1,'ep-1'),('b','c','entity',1,'ep-2');\
         INSERT INTO embeddings VALUES ('e1','m1','AAAA'),('e2','m1','AAAB');\
         INSERT INTO keep VALUES (1),(2);",
    )
    .unwrap();
    c
}

/// Positive receipt: all three tables still exist with their exact contents and
/// nothing extra was created. Never "an error appeared".
fn assert_intact(c: &rusqlite::Connection) {
    let tables: i64 = c
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tables, 3, "no table created or dropped");
    for (t, n) in [("entity_edges", 2), ("embeddings", 2), ("keep", 2)] {
        let got: i64 = c
            .query_row(&format!("SELECT count(*) FROM \"{t}\""), [], |r| r.get(0))
            .unwrap();
        assert_eq!(got, n, "{t} rows untouched");
    }
    let ids: String = c
        .query_row(
            "SELECT group_concat(id) FROM (SELECT id FROM embeddings ORDER BY id)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(ids, "e1,e2", "no value mutated");
}

/// Every new argument surface of P4 gets SQL-shaped caller text: the column
/// roles, the projection, the guards, the start values, the query vector.
/// Requirement per case: a clean reject (hard `Err` on the invalid_input path or
/// an outcome carrying an error code) and an untouched database.
#[test]
fn injection_matrix_over_the_new_arg_surfaces_rejects_cleanly() {
    let c = injection_fixture();
    let evil = "\"; DROP TABLE keep; --";
    let cases = vec![
        // traverse: table + every column role
        json!({"operation":"traverse","table":format!("entity_edges{evil}"),
               "src":"src_entity","dst":"dst_entity","start":"a"}),
        json!({"operation":"traverse","table":"entity_edges",
               "src":format!("src_entity{evil}"),"dst":"dst_entity","start":"a"}),
        json!({"operation":"traverse","table":"entity_edges",
               "src":"src_entity","dst":format!("dst_entity{evil}"),"start":"a"}),
        json!({"operation":"traverse","table":"entity_edges","src":"src_entity",
               "dst":"dst_entity","kind":format!("edge_kind{evil}"),"start":"a"}),
        json!({"operation":"traverse","table":"entity_edges","src":"src_entity",
               "dst":"dst_entity","weight":format!("weight{evil}"),"start":"a"}),
        json!({"operation":"traverse","table":"entity_edges","src":"src_entity",
               "dst":"dst_entity","columns":[format!("episode_id{evil}")],"start":"a"}),
        // traverse: guards and filters
        json!({"operation":"traverse","table":"entity_edges","src":"src_entity",
               "dst":"dst_entity","start":"a","max_depth":"3; DROP TABLE keep"}),
        json!({"operation":"traverse","table":"entity_edges","src":"src_entity",
               "dst":"dst_entity","start":"a","max_nodes":"10; DROP TABLE keep"}),
        json!({"operation":"traverse","table":"entity_edges","src":"src_entity",
               "dst":"dst_entity","start":"a","where":{format!("edge_kind{evil}"):"entity"}}),
        json!({"operation":"traverse","table":"entity_edges","src":"src_entity",
               "dst":"dst_entity","start":"a","where":{"edge_kind":{format!("eq{evil}"):"x"}}}),
        // similar: table, vector column, projection, vector, order and limit
        json!({"operation":"similar","table":format!("embeddings{evil}"),"columns":["id"],
               "vector_column":"blob","vector":"AAAA"}),
        json!({"operation":"similar","table":"embeddings","columns":["id"],
               "vector_column":format!("blob{evil}"),"vector":"AAAA"}),
        json!({"operation":"similar","table":"embeddings","columns":[format!("id{evil}")],
               "vector_column":"blob","vector":"AAAA"}),
        json!({"operation":"similar","table":"embeddings","columns":["id"],
               "vector_column":"blob","vector":format!("AAAA{evil}")}),
        json!({"operation":"similar","table":"embeddings","columns":["id"],
               "vector_column":"blob","vector":"AAAA","limit":"1; DROP TABLE keep"}),
        json!({"operation":"similar","table":"embeddings","columns":["id"],
               "vector_column":"blob","vector":"AAAA",
               "order_by":[{"col":format!("id{evil}")}]}),
        json!({"operation":"similar","table":"embeddings","columns":["id"],
               "vector_column":"blob","vector":"AAAA",
               "order_by":[{"col":"id","dir":format!("asc{evil}")}]}),
    ];
    for case in cases {
        match dispatch(&c, &case) {
            Err(_) => {}
            Ok(o) => assert!(o.error_code.is_some(), "must not succeed: {case}"),
        }
        assert_intact(&c);
    }
}

/// Sharper than the matrix: the identifier cases must be stopped by the
/// **catalog**, i.e. answer `unknown_column` — not by whatever SQLite happens to
/// make of a mangled statement.
///
/// Why this test exists (P3 lesson): a mutation probe that lets the catalog pass
/// caller text through unchecked leaves the matrix above fully green, because
/// rusqlite rejects the resulting multi-statement on its own. "Nothing got
/// through" is the security claim; "the catalog stopped it" is the design claim,
/// and only this test pins the second one.
#[test]
fn new_role_arguments_are_stopped_by_the_catalog_not_by_sqlite() {
    let c = injection_fixture();
    let evil = "\"; DROP TABLE keep; --";
    for role in ["src", "dst", "kind", "weight"] {
        let mut args = json!({"operation":"traverse","table":"entity_edges",
                              "src":"src_entity","dst":"dst_entity","start":"a"});
        args[role] = json!(format!("src_entity{evil}"));
        let out = dispatch(&c, &args).unwrap();
        assert_eq!(out.error_code, Some("unknown_column"), "role {role}");
    }
    let out = dispatch(
        &c,
        &json!({"operation":"traverse","table":"entity_edges","src":"src_entity",
                "dst":"dst_entity","start":"a","columns":[format!("episode_id{evil}")]}),
    )
    .unwrap();
    assert_eq!(
        out.error_code,
        Some("unknown_column"),
        "traverse projection"
    );

    let out = dispatch(
        &c,
        &json!({"operation":"similar","table":"embeddings","columns":["id"],
                "vector_column":format!("blob{evil}"),"vector":"AAAA"}),
    )
    .unwrap();
    assert_eq!(
        out.error_code,
        Some("unknown_column"),
        "similar vector_column"
    );

    for (op, args) in [
        (
            "traverse",
            json!({"operation":"traverse","table":format!("entity_edges{evil}"),
                   "src":"src_entity","dst":"dst_entity","start":"a"}),
        ),
        (
            "similar",
            json!({"operation":"similar","table":format!("embeddings{evil}"),
                   "columns":["id"],"vector_column":"blob","vector":"AAAA"}),
        ),
    ] {
        let out = dispatch(&c, &args).unwrap();
        assert_eq!(out.error_code, Some("unknown_table"), "{op} table");
    }
}

/// SQL-shaped *values* are inert: they are bound, so they simply match nothing.
#[test]
fn sql_shaped_start_nodes_and_vectors_are_bound_not_interpreted() {
    let c = injection_fixture();
    let p = dispatch(
        &c,
        &json!({"operation":"traverse","table":"entity_edges","src":"src_entity",
                "dst":"dst_entity","start":["a'; DROP TABLE keep; --"]}),
    )
    .unwrap();
    assert_eq!(p.error_code, None);
    assert_eq!(
        p.payload["paths"].as_array().unwrap().len(),
        0,
        "no such node"
    );
    assert_intact(&c);
}

// ---------------------------------------------------------------- similar

/// The vector leg itself: ranking is ascending hamming distance, and the
/// distance travels back with every row so a `code` cell can fuse legs.
#[test]
fn similar_ranks_ascending_by_hamming_distance() {
    let c = embeddings_fixture();
    let out = dispatch(
        &c,
        &json!({"operation":"similar","table":"embeddings","columns":["id"],
                "vector_column":"blob","vector": b64(&[0x00, 0x00]),
                "where":{"status":"ready"}}),
    )
    .unwrap();
    assert_eq!(out.error_code, None, "{:?}", out.error_text);
    assert_eq!(ids(&out.payload, "id"), vec!["r-near", "r-mid", "r-far"]);
    let d: Vec<i64> = out
        .payload
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["distance"].as_i64().unwrap())
        .collect();
    assert_eq!(d, vec![1, 3, 9], "distance is reported per row");
}

/// `where` filters BEFORE the ranking: the nearest vector of another owner
/// table must not appear at all, not even at the bottom.
#[test]
fn similar_applies_where_before_ranking() {
    let c = embeddings_fixture();
    c.execute(
        "INSERT INTO embeddings (id, owner_table, owner_id, model_id, dim, \
         binarization_version, blob, status, created_at) \
         VALUES ('r-other','episodes','e9','qwen3-emb-0.6b',16,'v1',?1,'ready','t')",
        rusqlite::params![b64(&[0x00, 0x00])],
    )
    .unwrap();
    let out = dispatch(
        &c,
        &json!({"operation":"similar","table":"embeddings","columns":["id"],
                "vector_column":"blob","vector": b64(&[0x00, 0x00]),
                "where":{"status":"ready","owner_table":"facts"}}),
    )
    .unwrap();
    assert_eq!(out.error_code, None, "{:?}", out.error_text);
    assert_eq!(
        ids(&out.payload, "id"),
        vec!["r-near", "r-mid", "r-far"],
        "the distance-0 row of another owner_table was filtered out, not ranked"
    );
}

/// Plan R9 (ratification requirement), pinned: a store whose embed lane was
/// down holds a backfill queue (`blob NULL`) next to finished rows. `similar`
/// must ignore the queue — neither fail on it nor rank NULLs to the top, which
/// is where `ORDER BY … ASC` would otherwise put them.
#[test]
fn similar_ignores_the_backfill_queue_instead_of_failing_or_ranking_nulls() {
    let c = embeddings_fixture();
    let out = dispatch(
        &c,
        &json!({"operation":"similar","table":"embeddings","columns":["id","status"],
                "vector_column":"blob","vector": b64(&[0x00, 0x00])}),
    )
    .unwrap();
    assert_eq!(out.error_code, None, "{:?}", out.error_text);
    assert_eq!(
        ids(&out.payload, "id"),
        vec!["r-near", "r-mid", "r-far"],
        "no queued row, and the order is the distance order"
    );
    assert_eq!(out.rows_affected, 3);
}

/// `limit` cuts AFTER the ranking, not before.
#[test]
fn similar_limits_after_ranking() {
    let c = embeddings_fixture();
    let out = dispatch(
        &c,
        &json!({"operation":"similar","table":"embeddings","columns":["id"],
                "vector_column":"blob","vector": b64(&[0x00, 0x00]),
                "where":{"status":"ready"},"limit":2}),
    )
    .unwrap();
    assert_eq!(ids(&out.payload, "id"), vec!["r-near", "r-mid"]);
}

/// Memory-spec B.1.1: comparisons never cross embedding generations. The op
/// does not enforce that rule — it makes the breach loud (plan R8) instead of
/// returning a plausible, wrong ranking.
#[test]
fn similar_on_mixed_generations_fails_loudly() {
    let c = embeddings_fixture();
    c.execute(
        "INSERT INTO embeddings (id, owner_table, owner_id, model_id, dim, \
         binarization_version, blob, status, created_at) \
         VALUES ('g2','facts','f9','other-model',32,'v1',?1,'ready','t')",
        rusqlite::params![b64(&[0x00, 0x00, 0x00, 0x00])],
    )
    .unwrap();
    let out = dispatch(
        &c,
        &json!({"operation":"similar","table":"embeddings","columns":["id"],
                "vector_column":"blob","vector": b64(&[0x00, 0x00]),
                "where":{"status":"ready"}}),
    )
    .unwrap();
    assert_eq!(out.error_code, Some("sql_error"));
    assert!(
        out.error_text
            .as_deref()
            .unwrap_or("")
            .contains("length mismatch"),
        "the error must name the mismatch: {:?}",
        out.error_text
    );

    // …and with the caller's model filter in place, the same table answers.
    let ok = dispatch(
        &c,
        &json!({"operation":"similar","table":"embeddings","columns":["id"],
                "vector_column":"blob","vector": b64(&[0x00, 0x00]),
                "where":{"status":"ready","model_id":"qwen3-emb-0.6b"}}),
    )
    .unwrap();
    assert_eq!(ok.error_code, None, "{:?}", ok.error_text);
    assert_eq!(ids(&ok.payload, "id"), vec!["r-near", "r-mid", "r-far"]);
}

/// The `hamming` function lives on the connection, not on the cell — so it has
/// to be registered wherever a store connection is born. This drives all three
/// paths a live cell actually takes (first wake on a fresh `cell.db`, a later
/// wake on the existing file, and a crash respawn) and demands a *ranked result*
/// each time, not merely the absence of an error.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hamming_survives_wake_rewake_and_respawn() {
    let td = tempfile::TempDir::new().unwrap();
    let cell_dir = td.path().to_path_buf();
    let params = json!({"schema":{"embeddings":{"id":"text","blob":"text"}}});

    // ---- 1. first wake: fresh cell.db, rows written through the cell ----
    let (sender, mut orx, _keep) = wake_store(&cell_dir, &params);
    for (id, vec) in [("v-near", vec![0x01, 0x00]), ("v-far", vec![0xFF, 0xFF])] {
        send(
            &sender,
            &json!({"operation":"insert","table":"embeddings",
                              "row":{"id":id,"blob": b64(&vec)}}),
        )
        .await;
        let em = orx.recv().await.unwrap();
        assert_eq!(em.content["header"]["rows_affected"], 1);
    }
    assert_eq!(
        similar_ids(&sender, &mut orx).await,
        vec!["v-near", "v-far"]
    );
    drop(sender);

    // ---- 2. re-wake on the SAME cell.db (a cold cell woken again) ----
    let (sender2, mut orx2, _keep2) = wake_store(&cell_dir, &params);
    assert_eq!(
        similar_ids(&sender2, &mut orx2).await,
        vec!["v-near", "v-far"],
        "a re-woken cell must still know hamming"
    );
    drop(sender2);

    // ---- 3. respawn (the crash-restart path) ----
    let (sender3, mut orx3, _keep3) = respawn_store(&cell_dir, &params);
    assert_eq!(
        similar_ids(&sender3, &mut orx3).await,
        vec!["v-near", "v-far"],
        "a respawned cell must still know hamming"
    );
}

/// Without a registered `hamming`, the op fails as a named SQL error — the
/// shape a forgotten connection path would produce.
#[test]
fn similar_without_registration_is_a_named_sql_error() {
    let c = rusqlite::Connection::open_in_memory().unwrap();
    c.execute("CREATE TABLE embeddings (id TEXT, blob TEXT)", [])
        .unwrap();
    let out = dispatch(
        &c,
        &json!({"operation":"similar","table":"embeddings","columns":["id"],
                "vector_column":"blob","vector":"AAAA"}),
    )
    .unwrap();
    assert_eq!(out.error_code, Some("sql_error"));
    assert!(
        out.error_text
            .as_deref()
            .unwrap_or("")
            .contains("no such function"),
        "got {:?}",
        out.error_text
    );
}

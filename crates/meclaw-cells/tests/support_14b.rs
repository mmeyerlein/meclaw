//! Phase-14-B shared test support: tree copy + base_url patch, boot with llm+code+store,
//! Live-Graph-Query (ColonyMsg::ReadGraph), zero-dep .dot-Emit, message_log-body_kind-Probe.
//! `#[path = "support_14b.rs"] mod support;` from both 14-B test files.
#![allow(dead_code)]

#[path = "topology_svg.rs"]
mod topology_svg;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::api_dto::{GraphEdgeDto, GraphNodeDto, ReadGraphReply};
use meclaw_colony::{CellFactory, CellFactoryRegistry, ColonyMsg, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json, to_string_pretty};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// Repo-root-relative path to a checked-in example tree.
pub fn example_dir(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

/// Copies `src` recursively to `dst` (files/directories only; no runtime artefacts
/// in the source tree), then patches `params.base_url` to `base_url` in EVERY `llm`
/// config.
pub fn copy_tree_patch_base_url(src: &std::path::Path, dst: &std::path::Path, base_url: &str) {
    copy_dir_recursive(src, dst);
    patch_llm_base_url(dst, base_url);
}

/// Recursively copies `src` into `dst`, skipping `.dot` and `.svg` files
/// (those are generated topology artefacts, not boot input).
pub fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        // Copy the topology tree only; SVG/DOT are artefacts, not boot input.
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.ends_with(".dot") || name.ends_with(".svg") {
            continue;
        }
        if from.is_dir() {
            copy_dir_recursive(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

fn patch_llm_base_url(dir: &std::path::Path, base_url: &str) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let p = entry.unwrap().path();
        if p.is_dir() {
            patch_llm_base_url(&p, base_url);
        } else if p.file_name().is_some_and(|n| n == "config.json") {
            let txt = std::fs::read_to_string(&p).unwrap();
            let mut v: Value = meclaw_core::serde_json::from_str(&txt).unwrap();
            if v["cell"]["type"] == "llm" {
                v["params"]["base_url"] = Value::String(base_url.to_string());
                std::fs::write(&p, to_string_pretty(&v).unwrap()).unwrap();
            }
        }
    }
}

/// Shared boot boilerplate behind [`boot`] and [`boot_with_blobs`]: takes the
/// ALREADY constructed `ColonyHandle` (the difference between the two boots is
/// exactly its construction — `new_with_factories_at` vs. `new_with_blobs_at`),
/// spawns the /sink + /park CaptureCells BEFORE bootstrap, fills the registry
/// (llm+code+store) and runs `bootstrap_from_filesystem` over the (already patched)
/// TempDir tree. Returns (ColonyHandle, sink_rx, park_rx).
async fn boot_inner(
    td: &tempfile::TempDir,
    h: ColonyHandle,
) -> (
    ColonyHandle,
    mpsc::Receiver<Message>,
    mpsc::Receiver<Message>,
) {
    let llm_f: Arc<dyn CellFactory> = Arc::new(LlmCellFactory);
    let code_f: Arc<dyn CellFactory> = Arc::new(CodeCellFactory);
    let store_f: Arc<dyn CellFactory> = Arc::new(StoreCellFactory);
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(64);
    let (park_tx, park_rx) = mpsc::channel::<Message>(64);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    h.spawn(Path::new("/park"), move || {
        CaptureCell::new(park_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    registry.insert("llm".to_string(), llm_f);
    registry.insert("code".to_string(), code_f);
    registry.insert("store".to_string(), store_f);
    // GH #464: `collector` ships a `menu-clock`, so every tree that carries the
    // shipped hive carries a `timer` cell -- a boot without the factory is an
    // `UnknownCellType`, not a quiet skip.
    registry.insert(
        "timer".to_string(),
        Arc::new(TimerCellFactory) as Arc<dyn CellFactory>,
    );
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");
    (h, sink_rx, park_rx)
}

/// Boot-Boilerplate: Factories (llm+code+store), /sink + /park CaptureCells VOR Bootstrap,
/// bootstrap_from_filesystem over the (already patched) TempDir tree.
/// Returns (ColonyHandle, sink_rx, park_rx).
pub async fn boot(
    td: &tempfile::TempDir,
) -> (
    ColonyHandle,
    mpsc::Receiver<Message>,
    mpsc::Receiver<Message>,
) {
    let h = ColonyHandle::new_with_factories_at(
        td,
        vec![
            (
                "llm".to_string(),
                Arc::new(LlmCellFactory) as Arc<dyn CellFactory>,
            ),
            ("code".to_string(), Arc::new(CodeCellFactory)),
            ("store".to_string(), Arc::new(StoreCellFactory)),
            ("timer".to_string(), Arc::new(TimerCellFactory)),
        ],
    );
    boot_inner(td, h).await
}

/// Like [`boot`], but wires a real `DiskBlobStore` at `<td>/blobs`
/// (`new_with_blobs_at`) so the A8 auto-offload producer hook
/// (`offload_oversized`) is live. Needed for whole-body blob offload tests; the
/// no-blob `boot` path would never produce a `Body::Blob`.
pub async fn boot_with_blobs(
    td: &tempfile::TempDir,
) -> (
    ColonyHandle,
    mpsc::Receiver<Message>,
    mpsc::Receiver<Message>,
) {
    let h = ColonyHandle::new_with_blobs_at(
        td,
        vec![
            (
                "llm".to_string(),
                Arc::new(LlmCellFactory) as Arc<dyn CellFactory>,
            ),
            ("code".to_string(), Arc::new(CodeCellFactory)),
            ("store".to_string(), Arc::new(StoreCellFactory)),
            ("timer".to_string(), Arc::new(TimerCellFactory)),
        ],
    );
    boot_inner(td, h).await
}

/// Minimal boot: only `code` factory + /sink CaptureCell + bootstrap over `dir`.
/// The registry is constructed twice: `ColonyHandle::new_with_factories_at` consumes
/// the factory list; `bootstrap_from_filesystem` needs a separate `&CellFactoryRegistry`.
/// This is API-mandated, not a smell.
pub async fn boot_code_only(td: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<Message>) {
    let h = ColonyHandle::new_with_factories_at(
        td,
        vec![(
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        )],
    );
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(16);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    registry.insert("code".to_string(), Arc::new(CodeCellFactory));
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");
    (h, sink_rx)
}

/// Bounded receipt (30 s, robust against cargo's parallel load).
pub async fn recv_bounded(rx: &mut mpsc::Receiver<Message>) -> Option<Message> {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
}

/// Live-Graph-Query: ColonyMsg::ReadGraph je Scope, gemergt (Nodes+Edges dedupe by Path/id).
pub async fn live_graph(
    h: &ColonyHandle,
    scopes: &[&str],
) -> (Vec<GraphNodeDto>, Vec<GraphEdgeDto>) {
    let mut nodes: Vec<GraphNodeDto> = Vec::new();
    let mut edges: Vec<GraphEdgeDto> = Vec::new();
    for s in scopes {
        let (tx, rx) = oneshot::channel::<ReadGraphReply>();
        h.runtime()
            .inbox_tx
            .send(ColonyMsg::ReadGraph {
                scope: Path::new(s),
                ack: tx,
            })
            .await
            .unwrap();
        let reply = rx.await.unwrap();
        for n in reply.nodes {
            if !nodes.iter().any(|x| x.path == n.path) {
                nodes.push(n);
            }
        }
        for e in reply.edges {
            if !edges.iter().any(|x| x.id == e.id) {
                edges.push(e);
            }
        }
    }
    (nodes, edges)
}

/// Konvertiert die Live-Graph-DTOs (`/colony/graph`-Read) in die `LiveGraph`-Form
/// of the shared zero-dep renderer (`topology_svg`). Cells = registry nodes;
/// hives = edge endpoints without a registry node (scope markers, not actors). A
/// pure data-mapping step, no render logic (that lives in the renderer).
fn live_graph_from_dtos(nodes: &[GraphNodeDto], edges: &[GraphEdgeDto]) -> topology_svg::LiveGraph {
    let cells: std::collections::BTreeSet<String> = nodes.iter().map(|n| n.path.clone()).collect();
    // Die `/colony/graph`-Read-Replies liefern Edges in Registry-Iterations-
    // order (not deterministic across processes). The renderer derives the node
    // arrangement from the order in which edges appear → without sorting, the
    // committed SVG would be run-dependent. Sort deterministically by
    // (from, to, condition) here — a pure test-helper step, the renderer stays
    // untouched. This makes the committed `graph.svg` byte-stable.
    let mut render_edges: Vec<topology_svg::LiveEdge> = edges
        .iter()
        .map(|e| topology_svg::LiveEdge {
            from: e.from.clone(),
            to: e.to.clone(),
            condition: e.condition.clone(),
        })
        .collect();
    render_edges
        .sort_by(|a, b| (&a.from, &a.to, &a.condition).cmp(&(&b.from, &b.to, &b.condition)));
    let mut hives: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for e in &render_edges {
        for p in [&e.from, &e.to] {
            if !cells.contains(p) {
                hives.insert(p.clone());
            }
        }
    }
    topology_svg::LiveGraph {
        edges: render_edges,
        hives: hives.into_iter().collect(),
        cells: cells.into_iter().collect(),
    }
}

/// Writes `graph.svg` + `graph.dot` into the checked-in example folder, both from
/// the LIVE booted graph via the shared zero-dep renderer
/// (`render_topology_svg`/`render_topology_dot`) — the same mechanism as
/// `tests/fixtures/14a-tool-loop/topology.svg`. ONLY under the env gate
/// `MECLAW_EMIT_DOT=1` (otherwise a no-op; CI and normal runs are read-only).
pub fn emit_dot_if_requested(example_name: &str, nodes: &[GraphNodeDto], edges: &[GraphEdgeDto]) {
    if std::env::var("MECLAW_EMIT_DOT").as_deref() == Ok("1") {
        let g = live_graph_from_dtos(nodes, edges);
        let dir = example_dir(example_name);
        std::fs::write(dir.join("graph.svg"), topology_svg::render_topology_svg(&g)).unwrap();
        std::fs::write(dir.join("graph.dot"), topology_svg::render_topology_dot(&g)).unwrap();
    }
}

/// Renders a DOT string from live-graph DTOs (zero-dep, identical to
/// `emit_dot_if_requested`). For tests that want to assert on the generated DOT.
pub fn render_dot(nodes: &[GraphNodeDto], edges: &[GraphEdgeDto]) -> String {
    topology_svg::render_topology_dot(&live_graph_from_dtos(nodes, edges))
}

/// Renders an SVG string from live-graph DTOs (zero-dep, identical to
/// `emit_dot_if_requested`). For tests that want to assert on the generated SVG.
pub fn render_svg(nodes: &[GraphNodeDto], edges: &[GraphEdgeDto]) -> String {
    topology_svg::render_topology_svg(&live_graph_from_dtos(nodes, edges))
}

/// Polls message_log until a row for `to_path` exists; returns its `body_kind`
/// ("inline"|"blob"). Pattern: pre14_a1_per_form_blob_resolution.rs::await_body_kind.
pub async fn await_body_kind(db_dir: &std::path::Path, to_path: &str) -> String {
    let db = db_dir.join("colony.db");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(conn) = rusqlite::Connection::open(&db) {
            // Target the offloading row directly: across ≥2 iterations there are several
            // `to_path`=/llm rows; the last one (the stop final) is small/inline. We want
            // the offloading row → AND body_kind='blob'. If it exists, A8 is proven.
            // message_log's destination column is `to_path` (schema.rs), not `target`.
            let row: Result<String, _> = conn.query_row(
                "SELECT body_kind FROM message_log WHERE to_path = ?1 AND body_kind = 'blob' LIMIT 1",
                [to_path],
                |r| r.get(0),
            );
            if let Ok(k) = row {
                return k;
            }
        }
        if std::time::Instant::now() > deadline {
            panic!("no message_log blob row for to_path {to_path} within 30s");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Returns all blob BODY contents (parsed) from `<td>/blobs/` (recursively, only
/// `*.json`). The A8 offload writes `<uuid>.json` (body) AND `<uuid>.json.meta.json`
/// (sidecar) per blob — both match the `*.json` glob. Sidecars (`*.meta.json`) are
/// filtered out so that only real blob bodies come back.
pub fn read_blob_bodies(td: &std::path::Path) -> Vec<Value> {
    let mut out = Vec::new();
    let dir = td.join("blobs");
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with(".meta.json"))
            {
                continue;
            }
            if p.extension().is_some_and(|x| x == "json")
                && let Ok(txt) = std::fs::read_to_string(&p)
                && let Ok(v) = meclaw_core::serde_json::from_str::<Value>(&txt)
            {
                out.push(v);
            }
        }
    }
    out
}

/// Source probe: one user turn + a tool schema under system.tools.*, with header.turn_id set.
pub fn user_probe(turn_id: &str) -> Message {
    let tool_schema = json!({
        "type": "function",
        "function": {"name": "calc", "description": "calc", "parameters": {"type": "object", "properties": {}}}
    });
    let tool_schema_str = meclaw_core::serde_json::to_string(&tool_schema).unwrap();
    let mut headers = meclaw_core::serde_json::Map::new();
    headers.insert("turn_id".into(), json!(turn_id));
    MessageBuilder::new(Path::new("/capture"))
        .body(Body::Inline(json!({
            "system": {"tools": {"calc": {"text": tool_schema_str}}},
            "messages": [{"origin": "user", "type": "text", "text": "rechne 2+3"}]
        })))
        .context(headers)
        .ttl(64)
        .build()
}

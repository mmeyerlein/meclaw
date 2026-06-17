//! Phase-14-B Shared Test-Support: Tree-Copy+base_url-Patch, Boot mit llm+code+store,
//! Live-Graph-Query (ColonyMsg::ReadGraph), zero-dep .dot-Emit, message_log-body_kind-Probe.
//! `#[path = "support_14b.rs"] mod support;` aus beiden 14-B-Testfiles.
#![allow(dead_code)]

#[path = "topology_svg.rs"]
mod topology_svg;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_colony::api_dto::{GraphEdgeDto, GraphNodeDto, ReadGraphReply};
use meclaw_colony::{CellFactory, CellFactoryRegistry, ColonyMsg, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json, to_string_pretty};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// Repo-root-relativer Pfad zu einem eingecheckten Beispiel-Baum.
pub fn example_dir(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

/// Kopiert `src` rekursiv nach `dst` (nur Dateien/Verzeichnisse; keine Laufzeit-Artefakte
/// im Quellbaum), patcht danach in JEDER `llm`-config die `params.base_url` auf `base_url`.
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
        // Nur den Topologie-Baum kopieren; SVG/DOT sind Artefakte, nicht Boot-Input.
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

/// Geteilte Boot-Boilerplate hinter [`boot`] und [`boot_with_blobs`]: nimmt den
/// BEREITS konstruierten `ColonyHandle` (der Unterschied der beiden Boots ist genau
/// dessen Konstruktion — `new_with_factories_at` vs. `new_with_blobs_at`), spawnt
/// /sink + /park CaptureCells VOR Bootstrap, füllt die Registry (llm+code+store) und
/// läuft `bootstrap_from_filesystem` über den (bereits gepatchten) TempDir-Baum.
/// Gibt (ColonyHandle, sink_rx, park_rx) zurück.
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
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");
    (h, sink_rx, park_rx)
}

/// Boot-Boilerplate: Factories (llm+code+store), /sink + /park CaptureCells VOR Bootstrap,
/// bootstrap_from_filesystem über den (bereits gepatchten) TempDir-Baum.
/// Gibt (ColonyHandle, sink_rx, park_rx) zurück.
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
        ],
    );
    boot_inner(td, h).await
}

/// Wie [`boot`], aber wired einen echten `DiskBlobStore` unter `<td>/blobs` ein
/// (`new_with_blobs_at`), sodass der A8-Auto-Offload-Producer-Hook
/// (`offload_oversized`) live ist. Nötig für Ganzkörper-Blob-Offload-Tests; der
/// no-blob-`boot`-Pfad würde nie eine `Body::Blob` erzeugen.
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

/// Bounded Receipt (30 s, robust gegen cargo-parallel-Last).
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
/// des shared zero-dep Renderers (`topology_svg`). Cells = Registry-Nodes;
/// Hives = Edge-Endpunkte ohne Registry-Node (Scope-Marker, kein Aktor). Reiner
/// Daten-Mapping-Schritt, keine Render-Logik (die lebt im Renderer).
fn live_graph_from_dtos(nodes: &[GraphNodeDto], edges: &[GraphEdgeDto]) -> topology_svg::LiveGraph {
    let cells: std::collections::BTreeSet<String> = nodes.iter().map(|n| n.path.clone()).collect();
    // Die `/colony/graph`-Read-Replies liefern Edges in Registry-Iterations-
    // Reihenfolge (nicht deterministisch über Prozesse). Der Renderer leitet die
    // Knoten-Anordnung aus der Edge-Auftauch-Reihenfolge ab → ohne Sortierung
    // wäre die committete SVG lauf-abhängig. Hier deterministisch nach
    // (from, to, condition) sortieren — reiner Test-Helper-Schritt, der Renderer
    // bleibt unberührt. Macht die committete `graph.svg` byte-stabil.
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

/// Schreibt `graph.svg` + `graph.dot` in den eingecheckten Beispiel-Ordner, beide
/// aus dem LIVE gebooteten Graph via den shared zero-dep Renderer
/// (`render_topology_svg`/`render_topology_dot`) — identischer Mechanismus wie
/// `tests/fixtures/14a-tool-loop/topology.svg`. NUR unter Env-Gate `MECLAW_EMIT_DOT=1`
/// (sonst no-op, CI/normale Läufe sind read-only).
pub fn emit_dot_if_requested(example_name: &str, nodes: &[GraphNodeDto], edges: &[GraphEdgeDto]) {
    if std::env::var("MECLAW_EMIT_DOT").as_deref() == Ok("1") {
        let g = live_graph_from_dtos(nodes, edges);
        let dir = example_dir(example_name);
        std::fs::write(dir.join("graph.svg"), topology_svg::render_topology_svg(&g)).unwrap();
        std::fs::write(dir.join("graph.dot"), topology_svg::render_topology_dot(&g)).unwrap();
    }
}

/// Rendert einen DOT-String aus Live-Graph-DTOs (zero-dep, identisch zu `emit_dot_if_requested`).
/// Für Tests, die den generierten DOT direkt assertieren wollen.
pub fn render_dot(nodes: &[GraphNodeDto], edges: &[GraphEdgeDto]) -> String {
    topology_svg::render_topology_dot(&live_graph_from_dtos(nodes, edges))
}

/// Rendert einen SVG-String aus Live-Graph-DTOs (zero-dep, identisch zu `emit_dot_if_requested`).
/// Für Tests, die das generierte SVG direkt assertieren wollen.
pub fn render_svg(nodes: &[GraphNodeDto], edges: &[GraphEdgeDto]) -> String {
    topology_svg::render_topology_svg(&live_graph_from_dtos(nodes, edges))
}

/// Poll message_log, bis eine Row für `to_path` existiert; gib deren `body_kind`
/// ("inline"|"blob") zurück. Muster: pre14_a1_per_form_blob_resolution.rs::await_body_kind.
pub async fn await_body_kind(db_dir: &std::path::Path, to_path: &str) -> String {
    let db = db_dir.join("colony.db");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(conn) = rusqlite::Connection::open(&db) {
            // Direkt auf die offloadende Row zielen: über ≥2 Iterationen gibt es mehrere
            // `to_path`=/llm-Rows; die letzte (stop-Final) ist klein/inline. Wir wollen die
            // offloadende Row → AND body_kind='blob'. Existiert sie, ist A8 bewiesen.
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

/// Liefert alle Blob-BODY-Inhalte (geparst) aus `<td>/blobs/` (rekursiv, nur `*.json`).
/// Der A8-Offload schreibt pro Blob `<uuid>.json` (Body) UND `<uuid>.json.meta.json`
/// (Sidecar) — beide matchen den `*.json`-Glob. Sidecars (`*.meta.json`) werden
/// herausgefiltert, sodass nur echte Blob-Bodies zurückkommen.
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

/// Quell-Probe: ein user-Turn + tool-Schema unter system.tools.*, header.turn_id gesetzt.
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

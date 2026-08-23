//! Workstream B (14-A addendum): one topology picture per test, rendered from the
//! LIVE booted graph (`/colony/graph` per scope, merged). The source is exactly
//! what the substrate REALLY loaded — not the filesystem tree on paper.
//!
//! Zero tooling: `dot` is not guaranteed to exist in the environment → we render a
//! hand-rolled SVG (zero-dep) and ADDITIONALLY emit a `.dot` file (zero-dep text)
//! so the SVG can be reproduced later via `dot -Tsvg`. NO cargo dep.
//!
//! Render-Konventionen: Hive = Transit-Form (Raute, gestrichelt), Cell = Box,
//! Edge-Label = CEL-`condition`, `/sink` als Test-Sonde markiert (gepunktet).

#![allow(dead_code)]

use meclaw_core::serde_json::json;
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use std::collections::{BTreeSet, HashSet};
use std::time::Duration;
use tokio::sync::mpsc;

/// One edge in the live graph: resolved `from`/`to` paths + the CEL `condition` source.
#[derive(Clone)]
pub struct LiveEdge {
    pub from: String,
    pub to: String,
    pub condition: Option<String>,
}

/// The merged live graph: edges + classified nodes.
/// `cells` = registry nodes (real actors); `hives` = edge endpoints that are NOT
/// registry nodes (hive scope markers are not actors → not in the registry, they
/// only appear as transit endpoints).
pub struct LiveGraph {
    pub edges: Vec<LiveEdge>,
    pub hives: Vec<String>,
    pub cells: Vec<String>,
}

/// Reads `/colony/graph` for every scope and merges (dedup by edge `id`).
/// `graph_rx` is the rx of the `/graphsink` CaptureCell (boot registers it as the
/// `reply_to` target). Source = the live booted graph; if an expected edge does not
/// come back, the calling assert fails = a FINDING (the read does not load the
/// real gebooteten Stand).
pub async fn read_live_graph(
    h: &ColonyHandle,
    graph_rx: &mut mpsc::Receiver<Message>,
    scopes: &[&str],
) -> LiveGraph {
    let mut edges: Vec<LiveEdge> = Vec::new();
    let mut seen_edge_ids: HashSet<String> = HashSet::new();
    let mut cells: BTreeSet<String> = BTreeSet::new();

    for scope in scopes {
        let msg = MessageBuilder::new(Path::new("/colony/graph"))
            .body(Body::Inline(json!({ "query": { "scope": scope } })))
            .reply_to(Path::new("/graphsink"))
            .build();
        h.send(msg).await;
        let reply = tokio::time::timeout(Duration::from_secs(30), graph_rx.recv())
            .await
            .ok()
            .flatten()
            .expect("/colony/graph must reply to /graphsink");
        let body = match &reply.body {
            Body::Inline(v) => v.clone(),
            Body::Blob(_) => panic!("inline graph reply expected"),
        };
        let g = &body["graph"];
        if let Some(nodes) = g["nodes"].as_array() {
            for n in nodes {
                if let Some(p) = n["path"].as_str() {
                    // `/graphsink` is this helper's own graph-read probe (edgeless,
                    // not part of the demo topology) → do not render it.
                    if p == "/graphsink" {
                        continue;
                    }
                    cells.insert(p.to_string());
                }
            }
        }
        if let Some(es) = g["edges"].as_array() {
            for e in es {
                let id = e["id"].as_str().unwrap_or("").to_string();
                if !seen_edge_ids.insert(id) {
                    continue;
                }
                edges.push(LiveEdge {
                    from: e["from"].as_str().unwrap_or("").to_string(),
                    to: e["to"].as_str().unwrap_or("").to_string(),
                    condition: e["condition"].as_str().map(|s| s.to_string()),
                });
            }
        }
    }

    // Hives = edge endpoints without a registry node (scope markers, not actors).
    let mut hives: BTreeSet<String> = BTreeSet::new();
    for e in &edges {
        for p in [&e.from, &e.to] {
            if !cells.contains(p) {
                hives.insert(p.clone());
            }
        }
    }

    LiveGraph {
        edges,
        hives: hives.into_iter().collect(),
        cells: cells.into_iter().collect(),
    }
}

/// Deterministic node order: edge sources in order of appearance, then the
/// remaining nodes sorted. Stable across runs (no randomness, no timestamp).
fn ordered_nodes(g: &LiveGraph) -> Vec<String> {
    let mut order: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let push = |p: &str, order: &mut Vec<String>, seen: &mut HashSet<String>| {
        if seen.insert(p.to_string()) {
            order.push(p.to_string());
        }
    };
    for e in &g.edges {
        push(&e.from, &mut order, &mut seen);
        push(&e.to, &mut order, &mut seen);
    }
    let mut rest: Vec<String> = g
        .cells
        .iter()
        .chain(g.hives.iter())
        .filter(|p| !seen.contains(*p))
        .cloned()
        .collect();
    rest.sort();
    order.extend(rest);
    order
}

fn is_hive(g: &LiveGraph, p: &str) -> bool {
    g.hives.iter().any(|h| h == p)
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\'', "&apos;")
}

/// Hand-rolled, deterministic SVG of the live graph (zero-dep).
/// Hive = Raute (gestrichelt), Cell = Box, `/sink` gepunktet, Edge-Label =
/// CEL-`condition`.
pub fn render_topology_svg(g: &LiveGraph) -> String {
    let nodes = ordered_nodes(g);
    let row_h = 90;
    let box_w = 220;
    let box_h = 44;
    let x = 60;
    let width = x + box_w + 360; // room for edge labels on the right
    let height = 60 + nodes.len() as i32 * row_h;
    let cy = |i: usize| -> i32 { 40 + i as i32 * row_h + box_h / 2 };
    let idx = |p: &str| -> Option<usize> { nodes.iter().position(|n| n == p) };

    let mut s = String::new();
    s.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" font-family=\"monospace\" font-size=\"13\">\n"
    ));
    s.push_str("<marker id=\"arrow\" markerWidth=\"10\" markerHeight=\"10\" refX=\"8\" refY=\"3\" orient=\"auto\"><path d=\"M0,0 L8,3 L0,6 Z\" fill=\"#444\"/></marker>\n");

    // Edges first (below the nodes).
    for e in &g.edges {
        let (fi, ti) = match (idx(&e.from), idx(&e.to)) {
            (Some(a), Some(b)) => (a, b),
            _ => continue,
        };
        let y1 = cy(fi);
        let y2 = cy(ti);
        let ex = x + box_w + 30;
        // Orthogonal path: out to the right, vertical, back into the target node.
        s.push_str(&format!(
            "<path d=\"M{} {} H{} V{} H{}\" fill=\"none\" stroke=\"#444\" stroke-width=\"1.5\" marker-end=\"url(#arrow)\"/>\n",
            x + box_w, y1, ex, y2, x + box_w + 6
        ));
        if let Some(c) = &e.condition {
            s.push_str(&format!(
                "<text x=\"{}\" y=\"{}\" fill=\"#a00\">{}</text>\n",
                ex + 6,
                (y1 + y2) / 2 - 4,
                xml_escape(c)
            ));
        }
    }

    // Nodes.
    for (i, p) in nodes.iter().enumerate() {
        let y = 40 + i as i32 * row_h;
        if is_hive(g, p) {
            // Raute, gestrichelt (Transit-Form).
            let mx = x + box_w / 2;
            let my = y + box_h / 2;
            s.push_str(&format!(
                "<polygon points=\"{mx},{} {},{my} {mx},{} {},{my}\" fill=\"#eef\" stroke=\"#338\" stroke-width=\"1.5\" stroke-dasharray=\"5,3\"/>\n",
                y - 6, x + box_w + 8, y + box_h + 6, x - 8
            ));
            s.push_str(&format!(
                "<text x=\"{mx}\" y=\"{}\" text-anchor=\"middle\" fill=\"#338\">{} (hive)</text>\n",
                my + 4,
                xml_escape(p)
            ));
        } else {
            let probe = p == "/sink";
            let dash = if probe {
                " stroke-dasharray=\"2,2\""
            } else {
                ""
            };
            let fill = if probe { "#efe" } else { "#fff" };
            s.push_str(&format!(
                "<rect x=\"{x}\" y=\"{y}\" width=\"{box_w}\" height=\"{box_h}\" rx=\"4\" fill=\"{fill}\" stroke=\"#333\" stroke-width=\"1.5\"{dash}/>\n"
            ));
            let label = if probe {
                format!("{p} (test probe)")
            } else {
                p.to_string()
            };
            s.push_str(&format!(
                "<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" fill=\"#111\">{}</text>\n",
                x + box_w / 2,
                y + box_h / 2 + 4,
                xml_escape(&label)
            ));
        }
    }

    s.push_str("</svg>\n");
    s
}

/// Accompanying Graphviz DOT (zero-dep text) — where `dot` is available, the SVG
/// can be reproduced via `dot -Tsvg <fn>.dot`.
pub fn render_topology_dot(g: &LiveGraph) -> String {
    let mut s = String::from("digraph topology {\n  rankdir=TB;\n");
    for p in &g.cells {
        let shape = if p == "/sink" {
            "box, style=dotted"
        } else {
            "box"
        };
        s.push_str(&format!("  {:?} [shape={}];\n", p, shape));
    }
    for p in &g.hives {
        s.push_str(&format!(
            "  {:?} [shape=diamond, style=dashed, label={:?}];\n",
            p,
            format!("{p} (hive)")
        ));
    }
    for e in &g.edges {
        match &e.condition {
            Some(c) => s.push_str(&format!("  {:?} -> {:?} [label={:?}];\n", e.from, e.to, c)),
            None => s.push_str(&format!("  {:?} -> {:?};\n", e.from, e.to)),
        }
    }
    s.push_str("}\n");
    s
}

/// Writes the topology SVG of the LIVE booted graph to
/// `target/phase-14a-topologies/<test_fn>.svg` (+ begleitendes `.dot`).
/// Scopes: `/` and `/tool-loop` (merged). An incomplete read is exposed by the
/// `read_live_graph` path or the calling assert = a FINDING.
pub async fn write_topology_diagram(
    h: &ColonyHandle,
    graph_rx: &mut mpsc::Receiver<Message>,
    test_fn: &str,
) -> LiveGraph {
    let g = read_live_graph(h, graph_rx, &["/", "/tool-loop"]).await;
    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/phase-14a-topologies");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{test_fn}.svg")), render_topology_svg(&g)).unwrap();
    std::fs::write(dir.join(format!("{test_fn}.dot")), render_topology_dot(&g)).unwrap();
    g
}

/// Mirrors the canonical 14a topology diagram into the checked-in example folder
/// `tests/fixtures/14a-tool-loop/` — ONLY under the env gate `MECLAW_EMIT_DOT=1`
/// (otherwise a no-op; the same mechanism as `emit_dot_if_requested` of the
/// 14b/c/d examples in `support_14b.rs`).
/// The canonical producer is `tool_loop_end_to_end_reaches_collector` — exactly ONE
/// test writes the example artefact, no hand copies from
/// `target/phase-14a-topologies/` mehr.
///
/// Edges are sorted deterministically by `(from, to, condition)` as in
/// `support_14b::live_graph_from_dtos`: `/colony/graph` returns them in registry
/// iteration order (not deterministic across processes), and the renderer derives
/// the node arrangement from the edge order — without sorting, the committed
/// artefact would be run-dependent.
pub fn emit_14a_example_diagram_if_requested(g: &LiveGraph) {
    if std::env::var("MECLAW_EMIT_DOT").as_deref() == Ok("1") {
        let mut edges = g.edges.clone();
        edges.sort_by(|a, b| (&a.from, &a.to, &a.condition).cmp(&(&b.from, &b.to, &b.condition)));
        let sorted = LiveGraph {
            edges,
            hives: g.hives.clone(),
            cells: g.cells.clone(),
        };
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/14a-tool-loop");
        std::fs::write(dir.join("topology.svg"), render_topology_svg(&sorted)).unwrap();
        std::fs::write(dir.join("topology.dot"), render_topology_dot(&sorted)).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svg_render_contains_nodes_and_edge_labels() {
        let g = LiveGraph {
            edges: vec![LiveEdge {
                from: "/llm".into(),
                to: "/tool-loop".into(),
                condition: Some("headers.finish_reason == 'tool_calls'".into()),
            }],
            hives: vec!["/tool-loop".into()],
            cells: vec!["/llm".into()],
        };
        let svg = render_topology_svg(&g);
        assert!(svg.contains("<svg"), "must be an SVG document");
        assert!(svg.contains("/llm"), "cell node label present");
        assert!(svg.contains("/tool-loop"), "hive node label present");
        assert!(svg.contains("(hive)"), "hive marked as transit form");
        assert!(
            svg.contains("tool_calls"),
            "edge label = CEL condition present"
        );
    }

    #[test]
    fn dot_render_marks_hive_and_sink() {
        let g = LiveGraph {
            edges: vec![LiveEdge {
                from: "/collector".into(),
                to: "/sink".into(),
                condition: None,
            }],
            hives: vec!["/tool-loop".into()],
            cells: vec!["/collector".into(), "/sink".into()],
        };
        let dot = render_topology_dot(&g);
        assert!(dot.contains("digraph topology"));
        assert!(dot.contains("shape=diamond"), "hive as diamond");
        assert!(dot.contains("style=dotted"), "/sink probe dotted");
    }
}

//! GH #1002: the city both lock files of the issue run against -- the plan's
//! lab (560 flat objects plus 30 containers of 128, a 250 KB stylesheet with
//! near-random data URIs) seeded into a `web` cell and served on one mount.
//!
//! A module of its own because the Slow 3G lock is a browser lock and lives
//! in a file of its own (`gate_plan.py` `BROWSER_LOCKS` is file-based), and
//! both files need the same city.

#![allow(dead_code)]

use meclaw_cells::web::WebCellFactory;
use meclaw_colony::{CellFactory, ContractView, SpawnedCellKind, SurfaceRegistry};
use meclaw_core::CellEmission;
use meclaw_core::Path;
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{surface_listener, wait_for_mount};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::sync::mpsc;

pub const MOUNT: &str = "city";

/// The default chunk limit (`join_chunk_bytes` absent): 96 KB.
pub const CHUNK: usize = 96 * 1024;

/// What a push frame adds around its payload: `[join_ref, null, topic,
/// "diff", …]`. Measured at about 80 bytes with this fixture's topic; 1 KB is
/// the "+ frame" of the plan with room to spare.
pub const ENVELOPE: usize = 1024;

// ---------------------------------------------------------------- fixture --

/// One JSONL seed file: a schema header and rows.
pub fn jsonl(dir: &std::path::Path, name: &str, schema: Value, rows: &[Value]) {
    let mut text = format!("{}\n", json!({ "schema": schema }));
    for row in rows {
        text.push_str(&row.to_string());
        text.push('\n');
    }
    std::fs::write(dir.join(name), text).expect("write seed file");
}

/// A stylesheet of `bytes` bytes that compresses the way a real one with
/// embedded images does: rule text around base64 runs that are close to
/// random. A run of one repeated letter would compress 100:1 and make the
/// gzip numbers a fiction.
pub fn stylesheet(bytes: usize) -> String {
    const B64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes + 256);
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut rule = 0usize;
    while out.len() < bytes {
        out.push_str(&format!(
            ".house-{rule}{{background:url(data:image/png;base64,"
        ));
        for _ in 0..600 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            out.push(B64[(state % 64) as usize] as char);
        }
        out.push_str(");width:32px;height:32px}\n");
        rule += 1;
    }
    out.truncate(bytes);
    out
}

/// The city: a root with `figs` flat children and `containers` children of
/// `kids` each, plus three assets -- the stylesheet, a small text file and a
/// large PNG (which must never be compressed).
///
/// One object renders to about 150 bytes, which is what the measured city
/// carried (666 KB / ~4 400 objects).
pub fn seed_city(dir: &std::path::Path, figs: usize, containers: usize, kids: usize, css: usize) {
    let seed = dir.join("seed");
    std::fs::create_dir_all(&seed).expect("seed dir");
    jsonl(
        &seed,
        "components.jsonl",
        json!({"name":"text","template":"text","prop_schema":"text","editable":"text","layer":"text"}),
        &[
            json!({"name":"city","template":"<main class=\"city\"><link rel=\"stylesheet\" href=\"city.css\">{{children}}</main>","prop_schema":"{}","editable":"[]","layer":"content"}),
            json!({"name":"fig","template":"<div class=\"fig\" data-k=\"{{k}}\" style=\"left:{{x}}px;top:{{y}}px;--hue:{{hue}}\"><b>{{label}}</b><i>{{role}}</i></div>","prop_schema":"{\"k\":\"text\",\"x\":\"text\",\"y\":\"text\",\"hue\":\"text\",\"label\":\"text\",\"role\":\"text\"}","editable":"[]","layer":"content"}),
            json!({"name":"chunk","template":"<section class=\"chunk\" data-c=\"{{c}}\">{{children}}</section>","prop_schema":"{\"c\":\"text\"}","editable":"[]","layer":"content"}),
        ],
    );
    let fig = |id: String, parent: &str, ord: usize| {
        json!({
            "id": id.clone(), "parent": parent, "component": "fig", "ord": ord,
            "props": json!({
                "k": id, "x": (ord * 37) % 4000, "y": (ord * 53) % 3000,
                "hue": ord % 360, "label": format!("resident {ord:05} of the old quarter"),
                "role": "walker"
            }).to_string()
        })
    };
    let mut objects =
        vec![json!({"id":"root","parent":null,"component":"city","ord":0,"props":"{}"})];
    // Containers first, then the flat figures: the LAST root child is a figure,
    // which is the slot the write-during-join lock aims at.
    for c in 0..containers {
        let cid = format!("chunk-{c}");
        objects.push(
            json!({"id": cid.clone(), "parent":"root","component":"chunk","ord": c,
                            "props": json!({"c": c}).to_string()}),
        );
        for k in 0..kids {
            objects.push(fig(format!("chunk-{c}/o-{k}"), &cid, k));
        }
    }
    for i in 0..figs {
        objects.push(fig(format!("fig-{i}"), "root", containers + i));
    }
    jsonl(
        &seed,
        "objects.jsonl",
        json!({"id":"text","parent":"text","component":"text","ord":"int","props":"text"}),
        &objects,
    );
    jsonl(
        &seed,
        "pages.jsonl",
        json!({"route":"text","root":"text","title":"text"}),
        &[json!({"route":"/","root":"root","title":"City"})],
    );
    // A PNG large enough to cross the compression threshold: binary types are
    // never compressed, whatever their size.
    let png: String = "\u{89}PNG"
        .chars()
        .chain(std::iter::repeat_n('x', 40 * 1024))
        .collect();
    jsonl(
        &seed,
        "assets.jsonl",
        json!({"path":"text","content_type":"text","body":"blob"}),
        &[
            json!({"path":"/city.css","content_type":"text/css; charset=utf-8","body": stylesheet(css)}),
            json!({"path":"/small.js","content_type":"application/javascript","body": "console.log(1);\n".repeat(100)}),
            json!({"path":"/house.png","content_type":"image/png","body": png}),
        ],
    );
}

/// The same seed, materialised in memory: what the whole tree and the whole
/// page are, independent of anything the listener does.
pub fn whole_of(dir: &std::path::Path) -> (Value, String) {
    let conn = rusqlite::Connection::open_in_memory().expect("memory db");
    meclaw_cells::web::db::setup_web_schema(&conn).expect("schema");
    meclaw_cells::web::seed::load_seed_if_present(&conn, dir).expect("seed");
    let page = meclaw_cells::web::render::materialize(&conn, "/").expect("materialize");
    (page.packed_tree(), page.rendered_body())
}

/// A running cell with everything that must stay alive to keep it running.
pub struct Live {
    pub port: u16,
    pub sender: mpsc::Sender<meclaw_core::Message>,
    pub join: tokio::task::JoinHandle<()>,
    _listener: tokio::task::JoinHandle<()>,
    _stop: tokio::sync::oneshot::Sender<()>,
    _drain: tokio::task::JoinHandle<()>,
}

impl Live {
    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}/{MOUNT}{path}", self.port)
    }
}

pub async fn start(dir: &std::path::Path, extra: Value) -> Live {
    let surfaces = Arc::new(SurfaceRegistry::new());
    let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(256);
    let (inbox_tx, _inbox_rx) = mpsc::channel(8);
    let mut params = json!({ "mount": MOUNT });
    if let (Some(p), Some(e)) = (params.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            p.insert(k.clone(), v.clone());
        }
    }
    let spawned = Arc::new(WebCellFactory::new(Arc::clone(&surfaces)))
        .spawn_cell(
            Path::new("/web"),
            params,
            out_tx,
            dir.to_path_buf(),
            ContractView::default(),
            inbox_tx,
            None,
            -1,
            None,
            None,
            64,
        )
        .expect("spawn");
    let SpawnedCellKind::Active {
        join,
        sender,
        stop_tx,
        ..
    } = spawned
    else {
        panic!("web cells spawn Active");
    };
    wait_for_mount(&surfaces, MOUNT).await;
    let (addr, listener) = surface_listener(Arc::clone(&surfaces)).await;
    let live = Live {
        port: addr.port(),
        sender,
        join,
        _listener: listener,
        _stop: stop_tx,
        // Every reply the cell sends goes somewhere: an undrained channel
        // fills after a few dozen writes and parks the handler, which is a
        // test that measures its own plumbing (seen: the 33rd of 40 writes).
        _drain: tokio::spawn(async move { while out_rx.recv().await.is_some() {} }),
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(r) = reqwest::get(live.url("/")).await
            && r.status().is_success()
        {
            break;
        }
        assert!(Instant::now() < deadline, "the cell never served its page");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    live
}

/// The city of the plan's lab: 560 + 30 × 128 objects, a 250 KB stylesheet.
pub async fn city(extra: Value) -> (TempDir, Live) {
    let td = TempDir::new().expect("tempdir");
    let dir = td.path().join("web");
    std::fs::create_dir_all(&dir).expect("cell dir");
    seed_city(&dir, 560, 30, 128, 250 * 1024);
    let live = start(&dir, extra).await;
    (td, live)
}

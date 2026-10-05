//! GH #1002: the first picture of a `web` page is small, and the second one is
//! nearly free.
//!
//! Measured on a 2D city served by this cell (about 4 400 objects on one
//! route): the first load moved 1.30 MB -- the page HTML carried the whole tree
//! (636 KB), the join reply carried it again in ONE WebSocket frame (666 KB),
//! and the stylesheet and client came on top -- and the second load moved the
//! same bytes again, because nothing carried a validator. A phone on a slow
//! link never connected at all: the client gives a join 10 s, and 0.69 MB in
//! one frame does not arrive in 10 s below roughly 70 KB/s.
//!
//! The locks here pin the four answers: validators on everything the cell
//! serves (`ETag`, `304`, `immutable` for a stamped client), a gzip variant
//! computed once when a large text file is loaded (never per request), a join
//! that arrives in pieces no larger than `join_chunk_bytes`, and a join
//! timeout the operator can raise.
//!
//! # Why a helper of its own and not the shared fixture
//!
//! The shared `tests/support/web_fixture.rs` is built by a parallel strand of
//! the same wave; until it lands this file seeds its own city
//! (`support/gh1002_city.rs`, OR-H4-15). The
//! shape follows the same numbers -- flat root children plus 30 containers of
//! 128 children -- so moving onto the fixture later changes no assertion.

use futures_util::{SinkExt, StreamExt};
use meclaw_cells::web::WebCellFactory;
use meclaw_colony::{CellFactory, SurfaceRegistry};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, MessageBuilder, Path};
use meclaw_testing::{surface_listener, wait_for_mount};
use std::io::Read;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio_tungstenite::tungstenite::Message as WsMessage;

#[path = "support/gh1002_city.rs"]
mod city_fixture;
use city_fixture::{CHUNK, ENVELOPE, Live, MOUNT, city, seed_city, start, whole_of};

/// One GET answer, as the wire had it: status, headers, the body bytes as
/// transferred (the client here decodes nothing).
struct Got {
    status: u16,
    headers: reqwest::header::HeaderMap,
    body: Vec<u8>,
}

impl Got {
    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    }
}

async fn get(url: &str, headers: &[(&str, &str)]) -> Got {
    let client = reqwest::Client::new();
    let mut req = client.get(url);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let resp = req.send().await.expect("get");
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let body = resp.bytes().await.expect("body").to_vec();
    Got {
        status,
        headers,
        body,
    }
}

fn gunzip(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(bytes)
        .read_to_end(&mut out)
        .expect("a gzip body decodes");
    out
}

/// Every `src="…/@client/…"` the page names, as written.
fn client_srcs(page: &str) -> Vec<String> {
    page.split("src=\"")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .filter(|src| src.contains("/@client/"))
        .map(str::to_string)
        .collect()
}

fn token_in(page: &str) -> String {
    let marker = "data-phx-session=\"";
    let start = page.find(marker).expect("the shell carries a token") + marker.len();
    let end = start + page[start..].find('"').expect("quoted");
    page[start..end].to_string()
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Open a socket and send the join the client sends.
async fn join(live: &Live) -> Ws {
    let page = String::from_utf8(get(&live.url("/"), &[]).await.body).expect("utf8");
    let token = token_in(&page);
    let topic = format!("lv:{}", meclaw_surface::session::container_id("/web"));
    let (mut ws, _) = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{}/{MOUNT}/live/websocket",
        live.port
    ))
    .await
    .expect("socket");
    let frame = json!(["1", "1", topic, "phx_join", {"session": token, "url": live.url("/")}]);
    ws.send(WsMessage::Text(frame.to_string().into()))
        .await
        .expect("send join");
    ws
}

/// The next text frame, or `None` after `quiet` without one.
async fn next_frame(ws: &mut Ws, quiet: Duration) -> Option<String> {
    loop {
        match tokio::time::timeout(quiet, ws.next()).await {
            Err(_) => return None,
            Ok(Some(Ok(WsMessage::Text(t)))) => return Some(t.to_string()),
            Ok(Some(Ok(_))) => continue,
            Ok(other) => panic!("the socket ended: {other:?}"),
        }
    }
}

/// The numeric slot keys of a packed tree.
fn slot_keys(tree: &Map<String, Value>) -> Vec<String> {
    tree.keys()
        .filter(|k| k.parse::<usize>().is_ok())
        .cloned()
        .collect()
}

/// Everything a join delivers: the reply, every later frame, and the tree
/// merged the way the client merges it. Reads until no slot of the reply is
/// still empty, then for `settle` more.
struct Joined {
    frames: Vec<String>,
    /// The tree as the client holds it: every frame resolved, then merged.
    tree: Value,
    /// The join reply's `rendered`, as it came.
    reply: Value,
    /// Every diff payload after it, as it came.
    diffs: Vec<Value>,
}

/// A frame's payload as the client holds it once it rendered it: every
/// numeric `"s"` replaced by the statics it names in THAT frame's `"p"`, and
/// the table gone. The client deletes the table after rendering a frame
/// (GH #1001), so a part naming statics another frame's table holds is a part
/// it cannot build -- `resolved` panics on one.
fn resolved(frame: &Value) -> Value {
    fn walk(v: &Value, p: &Value) -> Value {
        match v {
            Value::Object(m) => Value::Object(
                m.iter()
                    .filter(|(k, _)| k.as_str() != "p")
                    .map(|(k, x)| {
                        let y = match (k.as_str(), x) {
                            ("s", Value::Number(n)) => {
                                let t = p[n.to_string()].clone();
                                assert!(t.is_array(), "statics {n} are not in this frame's p");
                                t
                            }
                            ("s", other) => other.clone(),
                            _ => walk(x, p),
                        };
                        (k.clone(), y)
                    })
                    .collect(),
            ),
            other => other.clone(),
        }
    }
    walk(frame, &frame["p"])
}

/// The client's merge: a part with statics replaces, one without merges into
/// the part it has, key by key (`Rendered.mergeDiff`).
fn merge_into(target: &mut Value, diff: &Value) {
    match (target.as_object_mut(), diff.as_object()) {
        (Some(t), Some(d)) if !d.contains_key("s") => {
            for (k, v) in d {
                match t.get_mut(k) {
                    Some(old) if old.is_object() => merge_into(old, v),
                    _ => {
                        t.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        _ => *target = diff.clone(),
    }
}

/// GH #1013: the HTML of the last root child the client holds. The root's
/// children are one keyed list (`"0"` → `"k"`, counted by `"kc"`), so the
/// last one is entry `kc - 1` — and the page's last root child arrives with
/// the last join piece, which is what the write-during-the-chunks lock reads.
fn last_entry_html(tree: &Value) -> String {
    let list = &tree["0"]["k"];
    match list["kc"].as_u64() {
        Some(kc) if kc > 0 => {
            meclaw_cells::web::render::wire_html(&list[(kc - 1).to_string()]["0"], &Value::Null)
        }
        _ => String::new(),
    }
}

async fn join_all(live: &Live, settle: Duration) -> Joined {
    let mut ws = join(live).await;
    let first = next_frame(&mut ws, Duration::from_secs(30))
        .await
        .expect("a join reply");
    let reply: Value = meclaw_core::serde_json::from_str(&first).expect("json");
    assert_eq!(
        reply[4]["status"],
        json!("ok"),
        "join reply: {}",
        &first[..first.len().min(300)]
    );
    let raw = reply[4]["response"]["rendered"].clone();
    let mut tree = resolved(&raw);
    let mut diffs = Vec::new();
    let mut frames = vec![first];
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let open = tree
            .as_object()
            .map(|t| slot_keys(t).iter().any(|k| t[k] == json!("")))
            .unwrap_or(false);
        if !open {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the join never filled its empty slots"
        );
        if let Some(f) = next_frame(&mut ws, Duration::from_secs(10)).await {
            diffs.extend(merge(&mut tree, &f));
            frames.push(f);
        }
    }
    while let Some(f) = next_frame(&mut ws, settle).await {
        diffs.extend(merge(&mut tree, &f));
        frames.push(f);
    }
    Joined {
        frames,
        tree,
        reply: raw,
        diffs,
    }
}

/// Merge one pushed frame into a resolved tree the way the client does
/// (resolve against the frame's own `"p"`, then merge); its raw payload if it
/// was a diff.
fn merge(tree: &mut Value, frame: &str) -> Option<Value> {
    let f: Value = meclaw_core::serde_json::from_str(frame).expect("json");
    if f[3] != json!("diff") {
        return None;
    }
    merge_into(tree, &resolved(&f[4]));
    Some(f[4].clone())
}

/// One `object.update` through the cell's mailbox, as an app sends it.
async fn update(live: &Live, id: &str, props: Value) {
    let leg = json!({
        "origin": "assistant", "type": "tool_call", "id": "c0",
        "text": json!({"op": "object.update", "id": id, "props": props}).to_string(),
    });
    let msg = MessageBuilder::new(Path::new("/web"))
        .reply_to(Path::new("/caller"))
        .body(Body::Inline(json!({ "messages": [leg] })))
        .build();
    live.sender.send(msg).await.expect("mailbox");
}

// ------------------------------------------------------------ validators --

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1002_an_asset_answers_304_to_its_own_etag() {
    let (_td, live) = city(json!({})).await;
    let first = get(&live.url("/city.css"), &[]).await;
    assert_eq!(first.status, 200);
    let etag = first.header("etag").expect("an asset carries an ETag");
    assert!(
        etag.starts_with('"') && etag.ends_with('"'),
        "a strong validator: {etag}"
    );

    let again = get(&live.url("/city.css"), &[("if-none-match", &etag)]).await;
    assert_eq!(again.status, 304, "the browser's own copy is current");
    assert!(again.body.is_empty(), "a 304 has no body");
    assert_eq!(
        again.header("etag").as_deref(),
        Some(etag.as_str()),
        "and names what it confirms"
    );
    live.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1002_a_changed_asset_gets_a_new_etag() {
    let (_a_td, a) = city(json!({})).await;
    let td = TempDir::new().expect("tempdir");
    let dir = td.path().join("web");
    std::fs::create_dir_all(&dir).expect("cell dir");
    // One byte of stylesheet more: a different seed, a different file.
    seed_city(&dir, 4, 1, 2, 250 * 1024 + 1);
    let b = start(&dir, json!({})).await;

    let ta = get(&a.url("/city.css"), &[])
        .await
        .header("etag")
        .expect("etag a");
    let tb = get(&b.url("/city.css"), &[])
        .await
        .header("etag")
        .expect("etag b");
    assert_ne!(ta, tb, "another file is another validator");
    let cross = get(&b.url("/city.css"), &[("if-none-match", &ta)]).await;
    assert_eq!(
        cross.status, 200,
        "an old copy is not confirmed as the new file"
    );
    a.join.abort();
    b.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1002_a_versioned_client_bundle_is_immutable() {
    let (_td, live) = city(json!({})).await;
    let page = String::from_utf8(get(&live.url("/"), &[]).await.body).expect("utf8");
    let srcs = client_srcs(&page);
    assert_eq!(
        srcs.len(),
        3,
        "the shell names three client files: {srcs:?}"
    );
    for src in &srcs {
        assert!(
            src.contains("?v="),
            "every client file the shell names is stamped: {src}"
        );
        let url = format!("http://127.0.0.1:{}{src}", live.port);
        let got = get(&url, &[]).await;
        assert_eq!(got.status, 200, "{src}");
        assert_eq!(
            got.header("cache-control").as_deref(),
            Some("public, max-age=31536000, immutable"),
            "a stamped file never changes under its URL: {src}"
        );
    }
    // Without the stamp -- the microphone worklet is loaded that way -- the
    // file is revalidated, and a current copy is a 304.
    let bare = live.url("/@client/boot.js");
    let got = get(&bare, &[]).await;
    assert_eq!(got.header("cache-control").as_deref(), Some("no-cache"));
    let etag = got
        .header("etag")
        .expect("an unstamped client file carries an ETag");
    assert_eq!(get(&bare, &[("if-none-match", &etag)]).await.status, 304);
    // A stamp that is not this binary's is not a promise this binary can keep.
    let stale = get(&format!("{bare}?v=000000000000"), &[]).await;
    assert_eq!(stale.header("cache-control").as_deref(), Some("no-cache"));
    live.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1002_a_large_text_asset_is_served_precompressed() {
    let (td, live) = city(json!({})).await;
    let css = std::fs::read_to_string(td.path().join("web/seed/assets.jsonl")).expect("seed");
    let row: Value =
        meclaw_core::serde_json::from_str(css.lines().nth(1).expect("row")).expect("json");
    let raw = row["body"].as_str().expect("body").as_bytes().to_vec();

    let before = meclaw_cells::web::assets::compressions();
    let mut sizes = Vec::new();
    for _ in 0..3 {
        let got = get(
            &live.url("/city.css"),
            &[("accept-encoding", "gzip, deflate, br")],
        )
        .await;
        assert_eq!(got.status, 200);
        assert_eq!(got.header("content-encoding").as_deref(), Some("gzip"));
        assert_eq!(got.header("vary").as_deref(), Some("Accept-Encoding"));
        assert!(
            got.header("etag").expect("etag").ends_with("-gz\""),
            "the variant has its own validator"
        );
        assert_eq!(gunzip(&got.body), raw, "the variant is the file");
        sizes.push(got.body.len());
    }
    assert_eq!(
        meclaw_cells::web::assets::compressions(),
        before,
        "compression happens when the snapshot is loaded, never per request"
    );
    println!("NOTE css raw={} gz={}", raw.len(), sizes[0]);
    // The client bundle the same way.
    let lv = live.url("/@client/phoenix_live_view.min.js");
    let got = get(&lv, &[("accept-encoding", "gzip")]).await;
    assert_eq!(got.header("content-encoding").as_deref(), Some("gzip"));
    assert_eq!(
        gunzip(&got.body),
        get(&lv, &[]).await.body,
        "the compressed client is the client"
    );
    assert_eq!(meclaw_cells::web::assets::compressions(), before);
    live.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1002_a_small_or_binary_asset_is_never_compressed() {
    let (_td, live) = city(json!({})).await;
    for path in ["/small.js", "/house.png"] {
        let got = get(&live.url(path), &[("accept-encoding", "gzip")]).await;
        assert_eq!(got.status, 200, "{path}");
        assert_eq!(
            got.header("content-encoding"),
            None,
            "{path} is served as stored"
        );
    }
    // Without the header the large stylesheet is raw, and gzip at q=0 is a no.
    for ae in [None, Some("gzip;q=0"), Some("identity")] {
        let headers: Vec<(&str, &str)> =
            ae.map(|v| vec![("accept-encoding", v)]).unwrap_or_default();
        let got = get(&live.url("/city.css"), &headers).await;
        assert_eq!(
            got.header("content-encoding"),
            None,
            "accept-encoding {ae:?}"
        );
        assert!(got.body.starts_with(b".house-0{"), "raw bytes for {ae:?}");
    }
    live.join.abort();
}

/// T6: a page carries one load's
/// session token, whose nonce is the `session_id` of every event the load
/// sends. Its validator covers that token, so the previous load's `ETag`
/// never earns a `304` that would hand the old nonce back -- and `private`
/// keeps a shared cache from handing it to anyone else. Red before: the
/// same conditional GET answered `304` under `no-cache`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1002_a_page_is_private_and_never_304_with_an_old_token() {
    let (_td, live) = city(json!({})).await;
    let first = get(&live.url("/"), &[]).await;
    assert_eq!(
        first.header("cache-control").as_deref(),
        Some("private, no-cache")
    );
    let etag = first.header("etag").expect("a page carries an ETag");
    let again = get(&live.url("/"), &[("if-none-match", &etag)]).await;
    assert_eq!(
        again.status, 200,
        "an old load's validator is never a 304: its token is that load's"
    );
    let (one, two) = (
        token_in(&String::from_utf8(first.body.clone()).expect("utf8")),
        token_in(&String::from_utf8(again.body.clone()).expect("utf8")),
    );
    assert_ne!(one, two, "every load carries a token of its own");
    assert_ne!(
        again.header("etag").expect("etag"),
        etag,
        "and the validator names the body it came with"
    );
    live.join.abort();
}

// ---------------------------------------------------------- the join -----

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1002_no_join_frame_exceeds_the_chunk_limit() {
    let (td, live) = city(json!({})).await;
    let (whole, _) = whole_of(&td.path().join("web"));
    let joined = join_all(&live, Duration::from_millis(300)).await;
    let largest = joined.frames.iter().map(String::len).max().unwrap_or(0);
    // GH #1013: the join is cut between entries of the root's keyed list,
    // and an entry (a root child, with any list below it, GH #1009) travels
    // whole; the largest entry is the most one piece must carry alone.
    let largest_slot = whole["0"]["k"]
        .as_object()
        .expect("the root list")
        .iter()
        .filter(|(k, _)| k.parse::<usize>().is_ok())
        .map(|(_, v)| v.to_string().len())
        .max()
        .unwrap_or(0);
    println!("NOTE largest root entry={largest_slot} chunk={CHUNK}");
    let total: usize = joined.frames.iter().map(String::len).sum();
    println!(
        "NOTE join frames={} largest={} total={} whole_tree={}",
        joined.frames.len(),
        largest,
        total,
        whole.to_string().len()
    );
    for (i, f) in joined.frames.iter().enumerate() {
        assert!(
            f.len() <= CHUNK + ENVELOPE,
            "frame {i} carries {} bytes, over the {CHUNK}-byte limit",
            f.len()
        );
    }
    assert_eq!(
        joined.tree,
        resolved(&whole),
        "the pieces add up to the whole tree"
    );
    live.join.abort();
}

/// The vendored client bundle.
fn client_js() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../meclaw-surface/src/client/phoenix_live_view.min.js")
}

/// What the vendored client builds from a join and its diffs (the GH #1001
/// driver), or `None` when this host has no node.
fn client_builds(join: &Value, diffs: &[Value]) -> Option<String> {
    let td = TempDir::new().expect("tempdir");
    let steps = td.path().join("steps.json");
    std::fs::write(&steps, json!({"join": join, "diffs": diffs}).to_string()).expect("steps");
    let out = std::process::Command::new("node")
        .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/lv_client.mjs"))
        .arg(client_js())
        .arg(&steps)
        .output()
        .ok()?;
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if out.status.code() == Some(3) {
        println!("{stderr}");
        return None;
    }
    assert!(out.status.success(), "the client driver failed: {stderr}");
    let v: Value = meclaw_core::serde_json::from_slice(&out.stdout).expect("driver JSON");
    Some(v["full"].as_str().expect("full").to_string())
}

/// The markup with the client's own `data-phx-id` attributes taken out.
fn without_client_ids(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(at) = rest.find(" data-phx-id=\"") {
        out.push_str(&rest[..at]);
        let tail = &rest[at + " data-phx-id=\"".len()..];
        let end = tail.find('"').expect("closing quote");
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

/// The cut join through the vendored client: the reply and every piece, each
/// with the `"p"` of its own frame, build the whole page. A piece naming
/// statics only the head's table held fails in the client's render -- the
/// client deletes a frame's table once it rendered that frame.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1002_the_client_builds_the_page_from_the_cut_join() {
    let (td, live) = city(json!({})).await;
    let (_, body) = whole_of(&td.path().join("web"));
    let joined = join_all(&live, Duration::from_millis(300)).await;
    assert!(joined.diffs.len() > 1, "the city's join is cut");
    for (i, d) in joined.diffs.iter().enumerate() {
        let names_statics = d.to_string().contains("\"s\":");
        assert!(
            !names_statics || d.get("p").is_some(),
            "piece {i} names shared statics but carries no table"
        );
    }
    // GH #1009: the city's chunks hold their children as keyed
    // comprehensions, and a cut keeps a list whole inside its root slot.
    assert!(
        std::iter::once(&joined.reply)
            .chain(&joined.diffs)
            .any(|f| f.to_string().contains("\"kc\":")),
        "the join carries the children as keyed lists"
    );
    let Some(full) = client_builds(&joined.reply, &joined.diffs) else {
        live.join.abort();
        return;
    };
    let built = without_client_ids(&full);
    if built != body {
        let at = built
            .bytes()
            .zip(body.bytes())
            .position(|(a, b)| a != b)
            .unwrap_or(built.len().min(body.len()));
        let from = at.saturating_sub(120);
        panic!(
            "the client built a different page at byte {at}:\nclient: {}\nserved: {}",
            &built[from..(at + 120).min(built.len())],
            &body[from..(at + 120).min(body.len())]
        );
    }
    live.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1002_a_write_during_the_chunks_is_not_lost_or_early() {
    let (_td, live) = city(json!({})).await;
    let live = Arc::new(live);
    // The last root child sits in the last piece. Twelve writes run while the
    // join is answered; whatever the interleaving, the viewer must never see
    // the label go backwards (a piece cut from an older snapshot landing
    // after a newer diff) and must end on the last write. Twelve, not more:
    // every write re-renders the whole city (≈ 0.6 s each in a debug build,
    // the cost W1 of this wave removes), and the lock is about order, not
    // throughput.
    let target = "fig-559";
    const WRITES: u32 = 12;
    let writer = {
        let live = Arc::clone(&live);
        tokio::spawn(async move {
            for n in 0..WRITES {
                update(&live, target, json!({"label": format!("w{n:03}")})).await;
                tokio::time::sleep(Duration::from_millis(3)).await;
            }
        })
    };
    let mut ws = join(&live).await;
    let first = next_frame(&mut ws, Duration::from_secs(30))
        .await
        .expect("reply");
    let reply: Value = meclaw_core::serde_json::from_str(&first).expect("json");
    let mut tree = resolved(&reply[4]["response"]["rendered"]);
    let seen = |html: &str| -> Option<u32> {
        html.split("<b>w")
            .nth(1)
            .and_then(|r| r.get(..3))
            .and_then(|n| n.parse().ok())
    };
    let mut last: Option<u32> = seen(&last_entry_html(&tree));
    writer.await.expect("writer");
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut frames = 0usize;
    while last != Some(WRITES - 1) {
        if Instant::now() >= deadline {
            // Which side lost it: a fresh viewer reads what the cell holds.
            let fresh = join_all(&live, Duration::from_millis(200)).await;
            let held = seen(&last_entry_html(&fresh.tree));
            panic!(
                "the last write never reached the viewer (at {last:?} after {frames} frames); \
                 a fresh join reads {held:?}"
            );
        }
        let Some(f) = next_frame(&mut ws, Duration::from_secs(5)).await else {
            continue;
        };
        frames += 1;
        merge(&mut tree, &f);
        if let Some(n) = seen(&last_entry_html(&tree)) {
            if let Some(prev) = last {
                assert!(n >= prev, "the viewer went back from w{prev:03} to w{n:03}");
            }
            last = Some(n);
        }
    }
    live.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1002_a_tree_under_the_limit_joins_byte_identical() {
    // A display-sized page: a few slots, far below the limit.
    let td = TempDir::new().expect("tempdir");
    let dir = td.path().join("web");
    std::fs::create_dir_all(&dir).expect("cell dir");
    seed_city(&dir, 12, 2, 3, 2 * 1024);
    let (whole, body) = whole_of(&dir);
    let live = start(&dir, json!({})).await;

    let page = String::from_utf8(get(&live.url("/"), &[]).await.body).expect("utf8");
    assert!(
        page.contains(&body),
        "the page carries the whole body, as before"
    );

    let mut ws = join(&live).await;
    let first = next_frame(&mut ws, Duration::from_secs(30))
        .await
        .expect("reply");
    let reply: Value = meclaw_core::serde_json::from_str(&first).expect("json");
    assert_eq!(
        meclaw_core::serde_json::to_string(&reply[4]["response"]["rendered"]).expect("json"),
        meclaw_core::serde_json::to_string(&whole).expect("json"),
        "the reply is the packed tree, byte for byte"
    );
    assert_eq!(
        next_frame(&mut ws, Duration::from_millis(500)).await,
        None,
        "and nothing follows it"
    );
    live.join.abort();
}

// ------------------------------------------------- the display, whole --

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).expect("dir");
    for entry in std::fs::read_dir(src).expect("read_dir") {
        let entry = entry.expect("entry");
        let (from, to) = (entry.path(), dst.join(entry.file_name()));
        if from.is_dir() {
            copy_tree(&from, &to);
        } else {
            std::fs::copy(&from, &to).expect("copy");
        }
    }
}

/// The colony message `kind` sends, and its acknowledgement.
async fn colony_call<T>(
    h: &meclaw_testing::ColonyHandle,
    msg: impl FnOnce(tokio::sync::oneshot::Sender<T>) -> meclaw_colony::ColonyMsg,
) -> T {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx.send(msg(ack_tx)).await.expect("colony inbox");
    ack_rx.await.expect("colony ack")
}

/// The text of `page` between the container's opening tag and its close:
/// exactly the body the shell embedded.
fn container_body(page: &str) -> String {
    let open = "data-phx-static=\"\">\n";
    let start = page.find(open).expect("the container") + open.len();
    let end = start + page[start..].find("\n</div>\n<script").expect("its end");
    page[start..end].to_string()
}

fn container_id(page: &str) -> String {
    let marker = "<div id=\"";
    let start = page.find(marker).expect("the container") + marker.len();
    let end = start + page[start..].find('"').expect("quoted");
    page[start..end].to_string()
}

/// A packed tree as the HTML the client builds from it: statics and slots
/// interleaved.
fn rendered(tree: &Value) -> String {
    meclaw_cells::web::render::wire_html(&resolved(tree), &Value::Null)
}

/// T12 on the largest shipped display: the GH #553 example,
/// booted as it ships. Its page and its join are what they were before
/// GH #1002 -- the page carries every slot, the join is the whole tree in one
/// reply -- and its slot HTML stays below half of the cut threshold, the
/// headroom `CUT_ABOVE_BYTES` is measured with. Red before as GH #553
/// twice: the GET was cut at 48 KB (107 192 bytes, the picture missing).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1002_the_display_example_is_served_whole() {
    use meclaw_cells::code::CodeCellFactory;
    use meclaw_cells::store::StoreCellFactory;
    use meclaw_cells::timer::TimerCellFactory;
    use meclaw_colony::{CellFactoryRegistry, ColonyMsg, bootstrap_from_filesystem};

    let ex = repo("examples/display-colony-view");
    if !ex.join("grow.json").is_file() || !repo("templates/colony-view").is_dir() {
        return;
    }
    let td = TempDir::new().expect("tempdir");
    let root = td.path();
    copy_tree(&ex.join("seed"), root);
    for name in [
        "display",
        "colony-view",
        "terminal",
        "web",
        "canvy",
        "clock",
    ] {
        let src = repo(&format!("templates/{name}"));
        if src.is_dir() {
            copy_tree(&src, &root.join("templates").join(name));
        }
    }
    std::fs::write(root.join(".env"), "OPENROUTER_API_KEY=test-key\n").expect("env");
    let grow = read_json(&ex.join("grow.json"));
    let mount = grow["manifest"][0]["diff"]["add_nodes"]
        .as_array()
        .expect("add_nodes")
        .iter()
        .find(|n| n["name"] == json!("display"))
        .and_then(|n| n["override_params"]["web"]["mount"].as_str())
        .expect("the shipped example names the screen's mount")
        .to_string();

    let surfaces = Arc::new(SurfaceRegistry::new());
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            ("store".to_string(), Arc::new(StoreCellFactory)),
            ("timer".to_string(), Arc::new(TimerCellFactory)),
            (
                "web".to_string(),
                Arc::new(WebCellFactory::new(Arc::clone(&surfaces))),
            ),
        ]
    };
    let h = meclaw_testing::ColonyHandle::new_with_factories_at(&td, factories());
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(root, &registry, &h.runtime())
        .await
        .expect("the seed of examples/display-colony-view boots");
    let templates_root = root.join("templates");
    colony_call(&h, |ack| ColonyMsg::RescanTemplates {
        templates_root,
        ack,
    })
    .await
    .expect("rescan");
    for payload in grow["manifest"].as_array().expect("manifest").clone() {
        let outcome = colony_call(&h, |ack| ColonyMsg::Mutation {
            payload,
            reply_to: None,
            trace_id: meclaw_core::Uuid::now_v7(),
            parent_message_id: meclaw_core::Uuid::now_v7(),
            ack,
        })
        .await;
        assert!(
            matches!(
                outcome,
                meclaw_colony::mutation::MutationOutcome::Committed { .. }
            ),
            "precondition: the shipped declaration commits; got {outcome:?}"
        );
    }

    wait_for_mount(&surfaces, &mount).await;
    let (addr, _listener) = surface_listener(Arc::clone(&surfaces)).await;
    let url = format!("http://{addr}/{mount}/");
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let got = get(&url, &[]).await;
        if got.status == 200 && String::from_utf8_lossy(&got.body).contains("colony-view") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the screen never served the topology picture: status {}, {} bytes",
            got.status,
            got.body.len()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // A page and a join back to back; a write in between (the example has a
    // clock) changes both, so a mismatch is retried and only a lasting one
    // fails.
    let mut last = String::new();
    for _ in 0..5 {
        let page = String::from_utf8(get(&url, &[]).await.body).expect("utf8");
        let topic = format!("lv:{}", container_id(&page));
        let (mut ws, _) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/{mount}/live/websocket"))
                .await
                .expect("socket");
        let frame = json!(["1", "1", topic, "phx_join", {"session": token_in(&page), "url": url}]);
        ws.send(WsMessage::Text(frame.to_string().into()))
            .await
            .expect("send join");
        let first = next_frame(&mut ws, Duration::from_secs(30))
            .await
            .expect("a join reply");
        let reply: Value = meclaw_core::serde_json::from_str(&first).expect("json");
        assert_eq!(reply[4]["status"], json!("ok"), "join reply");
        let tree = reply[4]["response"]["rendered"].clone();
        let slots = tree.as_object().map(slot_keys).unwrap_or_default();
        let slot_bytes: usize = slots
            .iter()
            .map(|k| meclaw_cells::web::render::wire_html(&tree[k], &tree["p"]).len())
            .sum();
        println!(
            "NOTE display page={} slots={} slot_bytes={} join_reply={} cut_above={} join_chunk={}",
            page.len(),
            slots.len(),
            slot_bytes,
            first.len(),
            meclaw_cells::web::render::CUT_ABOVE_BYTES,
            CHUNK
        );
        assert!(
            slots.iter().all(|k| tree[k] != json!("")),
            "the display's join is the whole tree in its reply"
        );
        assert!(
            2 * slot_bytes <= meclaw_cells::web::render::CUT_ABOVE_BYTES,
            "the display's {slot_bytes} bytes of slot HTML leave the cut threshold \
             less than a factor of 2 (OR-H4-7)"
        );
        let body = container_body(&page);
        if body == rendered(&tree) {
            return;
        }
        last = format!(
            "page body {} bytes, join tree {} bytes",
            body.len(),
            rendered(&tree).len()
        );
    }
    panic!("the display's page never carried the join's whole body: {last}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1002_the_join_timeout_reaches_the_client() {
    let td = TempDir::new().expect("tempdir");
    let dir = td.path().join("web");
    std::fs::create_dir_all(&dir).expect("cell dir");
    seed_city(&dir, 4, 1, 2, 1024);
    let live = start(&dir, json!({"join_timeout_ms": 30000})).await;
    let page = String::from_utf8(get(&live.url("/"), &[]).await.body).expect("utf8");
    assert!(
        page.contains("<meta name=\"meclaw-join-timeout\" content=\"30000\">"),
        "the shell states the timeout"
    );
    let boot = String::from_utf8(get(&live.url("/@client/boot.js"), &[]).await.body).expect("utf8");
    assert!(
        boot.contains("meclaw-join-timeout") && boot.contains("timeout"),
        "and the boot hands it to the socket"
    );
    live.join.abort();

    // Absent, the shell says nothing and the client keeps its own 10 s.
    let td2 = TempDir::new().expect("tempdir");
    let dir2 = td2.path().join("web");
    std::fs::create_dir_all(&dir2).expect("cell dir");
    seed_city(&dir2, 4, 1, 2, 1024);
    let plain = start(&dir2, json!({})).await;
    let page = String::from_utf8(get(&plain.url("/"), &[]).await.body).expect("utf8");
    assert!(!page.contains("meclaw-join-timeout"), "no param, no meta");
    plain.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1002_join_params_outside_their_range_are_refused() {
    let td = TempDir::new().expect("tempdir");
    for bad in [
        json!({"mount": MOUNT, "join_chunk_bytes": 1000}),
        json!({"mount": MOUNT, "join_chunk_bytes": 8 * 1024 * 1024}),
        json!({"mount": MOUNT, "join_timeout_ms": 1000}),
        json!({"mount": MOUNT, "join_timeout_ms": "30000"}),
    ] {
        assert!(
            meclaw_cells::web::WebParams::parse(&bad).is_err(),
            "{bad} must be refused"
        );
    }
    let _ = td;
}

// -------------------------------------------------------------- the lab --

/// Every byte one first and one second visit move, by part.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1002_first_load_bytes_cold_and_warm() {
    let (_td, live) = city(json!({})).await;
    let gz = [("accept-encoding", "gzip, deflate, br")];

    // Cold: no validator anywhere.
    let page = get(&live.url("/"), &gz).await;
    let page_text = String::from_utf8(page.body.clone()).expect("utf8");
    let mut client_cold = 0usize;
    let mut client_tags = Vec::new();
    for src in client_srcs(&page_text) {
        let got = get(&format!("http://127.0.0.1:{}{src}", live.port), &gz).await;
        client_cold += got.body.len();
        client_tags.push((src, got.header("etag"), got.header("cache-control")));
    }
    let css = get(&live.url("/city.css"), &gz).await;
    let joined = join_all(&live, Duration::from_millis(300)).await;
    let join_bytes: usize = joined.frames.iter().map(String::len).sum();
    let cold_http = page.body.len() + client_cold + css.body.len();

    // Warm: the browser holds the first visit's validators. A stamped client
    // file is `immutable` -- the browser does not ask at all.
    let page_tag = page.header("etag").unwrap_or_default();
    let page2 = get(&live.url("/"), &[gz[0], ("if-none-match", &page_tag)]).await;
    let mut client_warm = 0usize;
    for (src, tag, cc) in &client_tags {
        if cc.as_deref().is_some_and(|c| c.contains("immutable")) {
            continue;
        }
        let tag = tag.clone().unwrap_or_default();
        let got = get(
            &format!("http://127.0.0.1:{}{src}", live.port),
            &[gz[0], ("if-none-match", &tag)],
        )
        .await;
        client_warm += got.body.len();
    }
    let css_tag = css.header("etag").unwrap_or_default();
    let css2 = get(
        &live.url("/city.css"),
        &[gz[0], ("if-none-match", &css_tag)],
    )
    .await;
    assert_eq!(page2.status, 200, "a page is never a 304 (T6)");
    let warm_http = page2.body.len() + client_warm + css2.body.len();

    println!(
        "NOTE cold page={} client={} css={} join={} -> http={} total={}",
        page.body.len(),
        client_cold,
        css.body.len(),
        join_bytes,
        cold_http,
        cold_http + join_bytes
    );
    println!(
        "NOTE warm page={} client={} css={} join={} -> http={} total={}",
        page2.body.len(),
        client_warm,
        css2.body.len(),
        join_bytes,
        warm_http,
        warm_http + join_bytes
    );
    // What this strand controls: everything that is not the socket. Measured
    // cold http ≈ 260 KB (page 48 KB + client + stylesheet gzip); warm, the
    // page alone (36 KB: it is never a 304, T6) while every file is a 304 or
    // immutable; the limits leave a factor of 2 (OR-H4-7). The join itself is
    // the tree, once, uncompressed -- the socket has no permessage-deflate
    // (O4, not this wave); its number is a NOTE and the city run carries the
    // end number.
    assert!(cold_http <= 520 * 1024, "cold http {cold_http} bytes");
    let warm_files = warm_http - page2.body.len();
    assert!(warm_files <= 16 * 1024, "warm files {warm_files} bytes");
    assert!(
        page2.body.len() <= 2 * 48 * 1024,
        "warm, the page is its first 48 KB again"
    );
    assert!(
        page.body.len() <= 2 * 48 * 1024,
        "the page carries at most its first 48 KB of body"
    );
    live.join.abort();
}

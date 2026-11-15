//! GH #1541: a page read the way a browser reads it -- the GET, then the
//! LiveView join on the page's own socket, its head and its pieces merged
//! the way the client merges them.
//!
//! Since GH #1002 a page with more than `CUT_ABOVE_BYTES` of slot HTML is
//! cut: the GET embeds only its first slots (`PAGE_INLINE_BYTES`) and the
//! join carries the rest in pieces. A display page is no exception (ruling
//! OR-54g-142, one cut for every page): the colony-view picture of a colony
//! with the full talky and its tools crossed 320 KiB, the GET of the screen
//! lost the picture's tail, and a test that read only the GET waited for a
//! cell the browser showed. A test that asks what a page SHOWS reads it here,
//! so its answer does not depend on the size of the picture.
//!
//! The end of the join is an event, not a pause: the socket answers a
//! heartbeat sent right behind the join only after the join wrote its last
//! piece (`socket.rs`: the pieces go out before the task reads its queue
//! again), so every frame before that reply is part of the picture.

#![allow(dead_code)]

use futures_util::{SinkExt, StreamExt};
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::deadline;
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// One read of a page.
#[derive(Debug)]
pub struct Seen {
    /// The GET's status.
    pub status: u16,
    /// The GET's body as served -- for a page above the cut, its first slots.
    pub get: String,
    /// What the browser shows: the joined tree as HTML for a `200`, the GET
    /// body for any other status.
    pub html: String,
    /// Frames the join took: 1 for a page joined whole, more for a cut one.
    pub frames: usize,
}

impl Seen {
    /// `(status, GET bytes, shown bytes, join frames)`, for a hang's last state.
    pub fn state(&self) -> String {
        format!(
            "(status {}, GET {} bytes, shown {} bytes, {} join frame(s))",
            self.status,
            self.get.len(),
            self.html.len(),
            self.frames
        )
    }
}

/// Reads `http://127.0.0.1:<port>/<mount>/` the way a browser does. `None`
/// while the listener or the socket does not answer, or the join is refused;
/// the hang mark bounds every step, so a hang comes back `None` and the wait
/// around it names it.
pub async fn read_page(port: u16, mount: &str) -> Option<Seen> {
    let url = format!("http://127.0.0.1:{port}/{mount}/");
    let client = reqwest::Client::builder()
        .timeout(deadline::scaled(deadline::HANG))
        .build()
        .ok()?;
    let r = client.get(&url).send().await.ok()?;
    let status = r.status().as_u16();
    let get = r.text().await.ok()?;
    if status != 200 {
        return Some(Seen {
            status,
            html: get.clone(),
            get,
            frames: 0,
        });
    }
    let joined = tokio::time::timeout(
        deadline::scaled(deadline::HANG),
        join(port, mount, &url, &get),
    )
    .await
    .ok()??;
    Some(Seen {
        status,
        get,
        html: meclaw_cells::web::render::wire_html(&joined.0, &Value::Null),
        frames: joined.1,
    })
}

/// The join of the page `get` served: the tree as the client holds it once
/// the join is through, and the number of frames it took.
async fn join(port: u16, mount: &str, url: &str, get: &str) -> Option<(Value, usize)> {
    let topic = format!("lv:{}", attr_after(get, "<div id=\"")?);
    let token = attr_after(get, "data-phx-session=\"")?;
    let (mut ws, _) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/{mount}/live/websocket"))
            .await
            .ok()?;
    let join = json!(["1", "1", topic, "phx_join", {"session": token, "url": url}]);
    let beat = json!([null, "2", "phoenix", "heartbeat", {}]);
    ws.send(WsMessage::Text(join.to_string().into()))
        .await
        .ok()?;
    ws.send(WsMessage::Text(beat.to_string().into()))
        .await
        .ok()?;

    let mut tree: Option<Value> = None;
    let mut frames = 0usize;
    loop {
        let text = match ws.next().await? {
            Ok(WsMessage::Text(t)) => t.to_string(),
            Ok(_) => continue,
            Err(_) => return None,
        };
        let f: Value = meclaw_core::serde_json::from_str(&text).ok()?;
        match (f[2].as_str(), f[3].as_str()) {
            (Some("phoenix"), Some("phx_reply")) => break,
            (_, Some("phx_reply")) if f[1] == json!("1") => {
                if f[4]["status"] != json!("ok") {
                    return None;
                }
                tree = Some(resolved(&f[4]["response"]["rendered"]));
                frames += 1;
            }
            (_, Some("diff")) => {
                merge_into(tree.as_mut()?, &resolved(&f[4]));
                frames += 1;
            }
            _ => {}
        }
    }
    let _ = ws.close(None).await;
    Some((tree?, frames))
}

/// The quoted value after `marker` in `page`.
fn attr_after(page: &str, marker: &str) -> Option<String> {
    let start = page.find(marker)? + marker.len();
    let end = start + page[start..].find('"')?;
    Some(page[start..end].to_string())
}

/// A frame's payload as the client holds it once it rendered it: every
/// numeric `"s"` replaced by the statics it names in THAT frame's `"p"`
/// (GH #1001: the client deletes the table after rendering the frame).
fn resolved(frame: &Value) -> Value {
    fn walk(v: &Value, p: &Value) -> Value {
        match v {
            Value::Object(m) => Value::Object(
                m.iter()
                    .filter(|(k, _)| k.as_str() != "p")
                    .map(|(k, x)| {
                        let y = match (k.as_str(), x) {
                            ("s", Value::Number(n)) => p[n.to_string()].clone(),
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
/// the part it has, key by key (`Rendered.mergeDiff`; a keyed list's entries
/// and its `"kc"` the same way, GH #1013).
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

//! The Phoenix channels protocol, five frames wide.
//!
//! # The whole contract
//!
//! Wire format is **vsn 2.0.0**: a JSON 5-tuple `[join_ref, ref, topic, event,
//! payload]`. A reply reuses `join_ref` and `ref`.
//!
//! | in | out |
//! |---|---|
//! | `heartbeat` on topic `phoenix` | `{status:"ok", response:{}}` |
//! | `phx_join` on `lv:<container-id>` | `{status:"ok", response:{rendered:<tree>, liveview_version:…}}` |
//! | `event` | `{status:"ok", response:{diff:<tree>}}` |
//! | anything else | `{status:"ok", response:{}}` |
//!
//! Verified against the real client in Chromium by the spike in
//! `projeks/MeClaw/meclaw-next/spike-liveview-client/` (9/9 Playwright).
//!
//! # The packed tree needs no template compiler
//!
//! `s` is an array of n+1 literal strings and the integer keys are the n dynamics;
//! a dynamic may itself be a nested tree. So
//!
//! ```json
//! {"s": ["<div id=\"surface\">", "</div>"], "0": "…the cell's whole HTML…"}
//! ```
//!
//! is legal, and gives full morphdom patching, the whole `phx-*` binding surface
//! and hooks. Cutting that HTML into per-object slots later is **not a protocol
//! change** — the same tree with more slots, after which a moved node sends
//! `{"0":{"1":{"0":"translate(240px,140px)"}}}` and nothing else. That is #159's
//! deferred diff protocol arriving for free, and it is deliberately not built now.
//!
//! # What this module refuses to understand
//!
//! An event name. `node:moved` means nothing here: the name and value go to the
//! cell verbatim. The moment this layer interpreted one, the binary would know what
//! is being drawn, and the architecture ruling says it must not.

use super::render::{Dispatcher, RenderError};
use super::session;
use axum::extract::ws::{Message as WsMessage, WebSocket};
use meclaw_core::serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

/// Reported on every join. Must match the vendored bundle in
/// `client/VERSIONS.md`. A mismatch is only a `console.warn` in the client, which
/// is exactly why it is a constant next to a documented rule and a test rather than
/// something a watchdog would catch.
pub const LIVEVIEW_VERSION: &str = "1.2.9";

/// How long a join or an event may wait for its cell.
///
/// Two `code` cell calls are ~34 ms measured, so this is three orders of magnitude
/// of headroom: it is a backstop against a wedged cell, not a latency budget.
const RENDER_TIMEOUT: Duration = Duration::from_secs(15);

/// One connection, one task.
pub struct Connection {
    /// The surface this socket was opened on, absolute.
    cell_path: String,
    dispatcher: Arc<Dispatcher>,
    /// Set by a successful `phx_join`.
    joined: Option<String>,
}

impl Connection {
    pub fn new(cell_path: String, dispatcher: Arc<Dispatcher>) -> Self {
        Self {
            cell_path,
            dispatcher,
            joined: None,
        }
    }

    /// Drive one socket until it closes.
    pub async fn run(mut self, mut ws: WebSocket) {
        while let Some(Ok(frame)) = ws.recv().await {
            let text = match frame {
                WsMessage::Text(t) => t,
                WsMessage::Close(_) => return,
                // Ping/Pong are axum's to answer; binary frames are not part of
                // vsn 2.0.0 and are ignored rather than treated as a protocol error.
                _ => continue,
            };
            let Some(reply) = self.handle_text(&text).await else {
                // Unparseable frame: close rather than guess. A client that speaks
                // another protocol should learn that immediately.
                let _ = ws.send(WsMessage::Close(None)).await;
                return;
            };
            if let Some(out) = reply
                && ws.send(WsMessage::Text(out)).await.is_err()
            {
                return;
            }
        }
    }

    /// `None` → the frame was not a vsn 2.0.0 tuple and the connection should
    /// close. `Some(None)` → nothing to send. `Some(Some(s))` → send `s`.
    async fn handle_text(&mut self, text: &str) -> Option<Option<String>> {
        let parsed: Value = meclaw_core::serde_json::from_str(text).ok()?;
        let tuple = parsed.as_array()?;
        if tuple.len() != 5 {
            return None;
        }
        let join_ref = tuple[0].clone();
        let msg_ref = tuple[1].clone();
        let topic = tuple[2].as_str()?.to_string();
        let event = tuple[3].as_str()?.to_string();
        let payload = tuple[4].clone();

        let response = match (topic.as_str(), event.as_str()) {
            ("phoenix", "heartbeat") => json!({}),
            (_, "phx_join") => match self.join(&topic, &payload).await {
                Ok(tree) => json!({ "rendered": tree, "liveview_version": LIVEVIEW_VERSION }),
                Err(reply) => {
                    return Some(Some(error_reply(&join_ref, &msg_ref, &topic, reply)));
                }
            },
            (_, "event") => match self.event(&payload).await {
                Ok(tree) => json!({ "diff": tree }),
                Err(reply) => {
                    return Some(Some(error_reply(&join_ref, &msg_ref, &topic, reply)));
                }
            },
            // live_patch, cids_destroyed, phx_leave, allow_upload, … An empty ok
            // keeps the connection up, which is what the client expects.
            _ => json!({}),
        };
        Some(Some(ok_reply(&join_ref, &msg_ref, &topic, response)))
    }

    /// A join: check the token, then render (or take the cache).
    async fn join(&mut self, topic: &str, payload: &Value) -> Result<Value, String> {
        let expected = format!("lv:{}", session::container_id(&self.cell_path));
        if topic != expected {
            return Err("this socket does not serve that container".into());
        }
        let token = payload.get("session").and_then(Value::as_str).unwrap_or("");
        if !session::names(token, &self.cell_path) {
            // The security case, and the reason we mint the token at all.
            tracing::warn!(
                surface = %self.cell_path,
                "join refused: the session token names another surface"
            );
            return Err("the session token does not name this surface".into());
        }
        self.joined = Some(topic.to_string());

        if let Some(html) = self.dispatcher.cached(&self.cell_path).await {
            return Ok(tree(&html));
        }
        self.ask(json!({ "event": "surface:join", "value": {} }))
            .await
    }

    /// An event: pass it to the cell verbatim.
    async fn event(&mut self, payload: &Value) -> Result<Value, String> {
        if self.joined.is_none() {
            return Err("event before join".into());
        }
        let name = payload.get("event").and_then(Value::as_str).unwrap_or("");
        let value = payload.get("value").cloned().unwrap_or(json!({}));
        self.ask(json!({ "event": name, "value": value })).await
    }

    /// One render, turned into a packed tree.
    async fn ask(&self, request: Value) -> Result<Value, String> {
        match self
            .dispatcher
            .render(&self.cell_path, request, RENDER_TIMEOUT)
            .await
        {
            Ok(html) => Ok(tree(&html)),
            Err(RenderError::CellError(m)) => Err(m),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// The one-slot packed tree. One static pair, one dynamic.
fn tree(html: &str) -> Value {
    json!({ "s": ["<div id=\"surface-root\">", "</div>"], "0": html })
}

fn ok_reply(join_ref: &Value, msg_ref: &Value, topic: &str, response: Value) -> String {
    meclaw_core::serde_json::to_string(&json!([
        join_ref,
        msg_ref,
        topic,
        "phx_reply",
        { "status": "ok", "response": response }
    ]))
    .unwrap_or_default()
}

fn error_reply(join_ref: &Value, msg_ref: &Value, topic: &str, reason: String) -> String {
    meclaw_core::serde_json::to_string(&json!([
        join_ref,
        msg_ref,
        topic,
        "phx_reply",
        { "status": "error", "response": { "reason": reason } }
    ]))
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_packed_tree_is_a_legal_one_slot_tree() {
        let t = tree("<svg/>");
        assert_eq!(t["s"].as_array().unwrap().len(), 2, "n+1 statics for n=1");
        assert_eq!(t["0"], "<svg/>");
    }

    #[test]
    fn a_reply_is_a_five_tuple_reusing_both_refs() {
        let s = ok_reply(&json!("1"), &json!("7"), "lv:x", json!({}));
        let v: Value = meclaw_core::serde_json::from_str(&s).unwrap();
        let a = v.as_array().unwrap();
        assert_eq!(a.len(), 5);
        assert_eq!(a[0], "1");
        assert_eq!(a[1], "7");
        assert_eq!(a[2], "lv:x");
        assert_eq!(a[3], "phx_reply");
        assert_eq!(a[4]["status"], "ok");
    }

    #[test]
    fn an_error_reply_carries_the_reason_the_cell_gave() {
        let s = error_reply(&json!("1"), &json!("2"), "lv:x", "no store".into());
        let v: Value = meclaw_core::serde_json::from_str(&s).unwrap();
        assert_eq!(v[4]["status"], "error");
        assert_eq!(v[4]["response"]["reason"], "no store");
    }

    // ---- the frame layer, driven without a browser ---------------------------
    //
    // The browser half is already proven against real Chromium by the spike
    // (`meclaw-next/spike-liveview-client/`, 9/9 Playwright). What is ours to check
    // is that the right frame produces the right reply and that the wrong one is
    // refused, and `handle_text` is separate from `run` precisely so that can be
    // done deterministically.

    const CELL: &str = "/org/acme/canvy/render";

    /// A connection whose dispatcher has a dead colony: every render fails fast, so
    /// cases that must be refused BEFORE a render are unambiguous.
    fn conn_without_a_colony() -> Connection {
        let (colony_tx, colony_rx) = tokio::sync::mpsc::channel(1);
        let (_egress_tx, egress_rx) = tokio::sync::mpsc::channel(1);
        let (dispatcher, _join) = super::super::render::Dispatcher::new(
            colony_tx,
            egress_rx,
            meclaw_core::MESSAGE_DEFAULT_TTL,
        );
        drop(colony_rx);
        Connection::new(CELL.to_string(), dispatcher)
    }

    fn frame(topic: &str, event: &str, payload: Value) -> String {
        meclaw_core::serde_json::to_string(&json!(["1", "2", topic, event, payload])).unwrap()
    }

    fn parse(reply: Option<Option<String>>) -> Value {
        let text = reply
            .expect("the connection must stay open")
            .expect("a reply must be sent");
        meclaw_core::serde_json::from_str(&text).unwrap()
    }

    #[tokio::test]
    async fn a_heartbeat_is_answered_ok() {
        let mut c = conn_without_a_colony();
        let v = parse(
            c.handle_text(&frame("phoenix", "heartbeat", json!({})))
                .await,
        );
        assert_eq!(v[2], "phoenix");
        assert_eq!(v[4]["status"], "ok");
    }

    /// **The security case, and the reason we mint the token at all.** A token from
    /// another surface must not open this one — checked before any render.
    #[tokio::test]
    async fn a_join_whose_token_names_another_surface_is_refused() {
        let mut c = conn_without_a_colony();
        let topic = format!("lv:{}", session::container_id(CELL));
        let foreign = session::mint("/org/acme/vault");
        let v = parse(
            c.handle_text(&frame(&topic, "phx_join", json!({ "session": foreign })))
                .await,
        );
        assert_eq!(v[4]["status"], "error");
        assert!(
            v[4]["response"]["reason"]
                .as_str()
                .unwrap()
                .contains("token"),
            "{v}"
        );
    }

    #[tokio::test]
    async fn a_join_on_another_container_is_refused() {
        let mut c = conn_without_a_colony();
        let token = session::mint(CELL);
        let v = parse(
            c.handle_text(&frame(
                "lv:somebody-else",
                "phx_join",
                json!({ "session": token }),
            ))
            .await,
        );
        assert_eq!(v[4]["status"], "error");
    }

    /// An event before a join is refused rather than served: the token check lives
    /// in the join, so an ungated event would be a way around it.
    #[tokio::test]
    async fn an_event_before_a_join_is_refused() {
        let mut c = conn_without_a_colony();
        let topic = format!("lv:{}", session::container_id(CELL));
        let v = parse(
            c.handle_text(&frame(&topic, "event", json!({ "event": "node:moved" })))
                .await,
        );
        assert_eq!(v[4]["status"], "error");
        assert!(
            v[4]["response"]["reason"]
                .as_str()
                .unwrap()
                .contains("join")
        );
    }

    /// Everything the client pushes that is not one of the three gets an empty ok
    /// and keeps the connection up. That list grows with the client, so the default
    /// must be tolerant rather than enumerated.
    #[tokio::test]
    async fn an_unknown_event_gets_an_empty_ok_and_stays_connected() {
        let mut c = conn_without_a_colony();
        let topic = format!("lv:{}", session::container_id(CELL));
        for event in ["live_patch", "cids_destroyed", "phx_leave", "allow_upload"] {
            let v = parse(c.handle_text(&frame(&topic, event, json!({}))).await);
            assert_eq!(v[4]["status"], "ok", "{event}");
            assert_eq!(v[4]["response"], json!({}), "{event}");
        }
    }

    /// A frame that is not a vsn 2.0.0 tuple closes the connection instead of being
    /// guessed at. A client speaking another protocol should learn that at once.
    #[tokio::test]
    async fn a_malformed_frame_closes_the_connection_without_a_panic() {
        let mut c = conn_without_a_colony();
        for bad in [
            "not json at all",
            "{}",
            "[]",
            "[1,2,3]",
            "[\"1\",\"2\",\"t\",\"e\"]",
            "[\"1\",\"2\",\"t\",\"e\",{},\"extra\"]",
            "[\"1\",\"2\",7,\"e\",{}]",
            "[\"1\",\"2\",\"t\",7,{}]",
        ] {
            assert!(
                c.handle_text(bad).await.is_none(),
                "{bad:?} must close the connection"
            );
        }
    }
}

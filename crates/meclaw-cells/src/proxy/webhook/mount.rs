//! The webhook mount: one POST, verified, answered `202`, emitted once.
//!
//! The colony's one listener hands a connection whose first path segment is
//! this mount's name to this cell (`crate::handed::serve_handed`), exactly as
//! for the `meclaw` platform (`crate::proxy::meclaw::mount`). The router below
//! answers only `/<mount>/` and `/<mount>/<rest>`; every other path on the
//! handed connection is a `404` of this router and never reaches a foreign
//! mount, because the listener has already decided whose connection it is.
//!
//! Order per request (README § 2.1): size (`413`, from the body limit, which
//! stops reading at the limit), then verification over the raw body (`401` and
//! a `refused` receipt, nothing emitted), then `202` — after the arrival has
//! been handed to the handler half, which emits it. A method other than POST is
//! `405` from the router and reads no body. Nothing waits for the emission to
//! be routed: a webhook is one-way, and a sender that wants its delivery
//! repeated repeats it itself.

use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path as UrlPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use meclaw_colony::SurfaceRegistry;
use serde_json::{Map, Value, json};
use std::sync::Arc;
use tokio::sync::mpsc;

use super::params::{VerifyKind, WebhookParams};
use super::verify::{Unverified, verify};
use crate::mount_guard::MountGuard;

/// What the mount half needs: its name, its proof rule, which headers travel,
/// the mount table and the way to the handler.
#[derive(Clone)]
pub struct WebhookIo {
    pub(crate) mount: String,
    pub(crate) kind: VerifyKind,
    /// The resolved secret. Held in memory only; never logged, never echoed.
    pub(crate) secret: Arc<str>,
    /// The proof header, lower-case; `""` with kind `none`.
    pub(crate) header: String,
    pub(crate) prefix: String,
    pub(crate) headers_allow: Arc<Vec<String>>,
    pub(crate) max_body_bytes: usize,
    pub(crate) cell_path: String,
    pub(crate) surfaces: Arc<SurfaceRegistry>,
    /// Set by `run_io`, because the channel exists only there.
    pub(crate) events_tx: Option<mpsc::Sender<HookEvent>>,
}

impl std::fmt::Debug for WebhookIo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebhookIo")
            .field("mount", &self.mount)
            .field("kind", &self.kind)
            .field("header", &self.header)
            .field("cell_path", &self.cell_path)
            .finish_non_exhaustive()
    }
}

impl WebhookIo {
    /// The mount half of the cell at `cell_path`, registering on `surfaces`.
    pub fn new(p: &WebhookParams, cell_path: &str, surfaces: Arc<SurfaceRegistry>) -> Self {
        Self {
            mount: p.mount.clone(),
            kind: p.kind(),
            secret: Arc::from(p.secret()),
            header: p
                .verify
                .header
                .as_deref()
                .unwrap_or("")
                .to_ascii_lowercase(),
            prefix: p.verify.prefix.clone().unwrap_or_default(),
            headers_allow: Arc::new(p.headers_allow.clone()),
            max_body_bytes: p.max_body_bytes(),
            cell_path: cell_path.to_string(),
            surfaces,
            events_tx: None,
        }
    }

    /// Whether a proof is required but there is no secret to check it with
    /// (the variable is bound blank). Such a mount answers every POST `503`.
    pub fn secret_missing(&self) -> bool {
        self.kind != VerifyKind::None && self.secret.is_empty()
    }
}

/// What the mount tells the handler half. The verdict has already fallen.
#[derive(Debug)]
pub enum HookEvent {
    /// A verified request: the UBF body to emit on the lane.
    Arrived(Value),
    /// A request that did not prove itself; nothing of it is emitted.
    Refused(Unverified),
    /// The mount requires a proof and holds no secret; reported once per life.
    SecretMissing,
    /// The name could not be taken; the cell stays up and serves nobody.
    MountFailed(String),
}

/// Nothing travels on the reconfig channel: every key of a `webhook` proxy is
/// immutable, so the channel closing is the whole signal.
#[derive(Debug)]
pub enum HookReconfig {}

/// The router a handed connection is served with: POST on `/<mount>/` and
/// below, nothing else. `post` answers every other method `405` itself.
pub(crate) fn mounted_router(io: WebhookIo) -> Router {
    let mount = io.mount.clone();
    let limit = io.max_body_bytes;
    Router::new()
        .route(&format!("/{mount}/"), axum::routing::post(post_root))
        .route(&format!("/{mount}/*rest"), axum::routing::post(post_rest))
        .layer(DefaultBodyLimit::max(limit))
        .with_state(io)
}

async fn post_root(State(io): State<WebhookIo>, headers: HeaderMap, body: Bytes) -> Response {
    receive(&io, "", &headers, &body).await
}

async fn post_rest(
    State(io): State<WebhookIo>,
    UrlPath(rest): UrlPath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    receive(&io, &rest, &headers, &body).await
}

/// One request, after the body limit held: verify, hand over, answer.
async fn receive(io: &WebhookIo, path: &str, headers: &HeaderMap, raw: &[u8]) -> Response {
    let Some(events_tx) = io.events_tx.clone() else {
        // Only reachable if a router were built outside `run_io`.
        return (StatusCode::SERVICE_UNAVAILABLE, "no handler\n").into_response();
    };
    if io.secret_missing() {
        // Never open: a blank secret would otherwise let anyone sign.
        return (StatusCode::SERVICE_UNAVAILABLE, "secret missing\n").into_response();
    }
    let proof = if io.header.is_empty() {
        None
    } else {
        headers.get(io.header.as_str()).map(|v| v.as_bytes())
    };
    if let Err(u) = verify(io.kind, &io.secret, proof, &io.prefix, raw) {
        // The receipt is booked; the answer does not wait for it to matter.
        let _ = events_tx.send(HookEvent::Refused(u)).await;
        return (StatusCode::UNAUTHORIZED, "unverified\n").into_response();
    }
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
        .unwrap_or_default();
    let body = build_body(
        raw,
        &content_type,
        filter_headers(headers, &io.headers_allow),
        path,
    );
    // The one wait: the bounded hand-over to the handler half. An arrival the
    // handler cannot take is not acknowledged, so the sender repeats it.
    if events_tx.send(HookEvent::Arrived(body)).await.is_err() {
        return (StatusCode::SERVICE_UNAVAILABLE, "the cell is going away\n").into_response();
    }
    (StatusCode::ACCEPTED, "accepted\n").into_response()
}

/// The allowed request headers, names lower-case, compared without regard to
/// case. A header sent more than once is joined with `", "` (RFC 9110 § 5.3);
/// an allowed header the request lacks is absent, not empty.
pub fn filter_headers(all: &HeaderMap, allow: &[String]) -> Map<String, Value> {
    let mut out = Map::new();
    for name in allow {
        let lower = name.to_ascii_lowercase();
        let values: Vec<String> = all
            .get_all(lower.as_str())
            .iter()
            .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
            .collect();
        if !values.is_empty() {
            out.insert(lower, Value::String(values.join(", ")));
        }
    }
    out
}

/// The UBF body of one arrival: `messages: []` — foreign content never
/// becomes a conversation turn — plus the slot `webhook`. The raw body is
/// `raw` when it is UTF-8 and `raw_base64` otherwise; `json` is present only
/// for `application/json` whose body parses. An unparsable JSON body is not
/// an error: `raw` still carries it.
pub fn build_body(
    raw: &[u8],
    content_type: &str,
    headers: Map<String, Value>,
    path: &str,
) -> Value {
    let mut hook = Map::new();
    hook.insert("path".into(), Value::String(path.to_string()));
    hook.insert(
        "content_type".into(),
        Value::String(content_type.to_string()),
    );
    hook.insert("headers".into(), Value::Object(headers));
    match std::str::from_utf8(raw) {
        Ok(s) => {
            hook.insert("raw".into(), Value::String(s.to_string()));
        }
        Err(_) => {
            hook.insert(
                "raw_base64".into(),
                Value::String(crate::file::base64_encode(raw)),
            );
        }
    }
    if is_json(content_type)
        && let Ok(v) = serde_json::from_slice::<Value>(raw)
    {
        hook.insert("json".into(), v);
    }
    json!({ "messages": [], "webhook": Value::Object(hook) })
}

/// `application/json`, parameters (`; charset=utf-8`) and case aside.
fn is_json(content_type: &str) -> bool {
    content_type
        .split(';')
        .next()
        .map(str::trim)
        .is_some_and(|t| t.eq_ignore_ascii_case("application/json"))
}

/// Register the mount, serve every handed connection, and stay up for the
/// cell's whole life — the `meclaw` platform's `run_io`
/// (`crate::proxy::meclaw::io::run_io`), with one addition: a mount that needs
/// a secret and holds none reports it once, then answers every POST `503`.
pub async fn run_io(
    io: WebhookIo,
    events_tx: mpsc::Sender<HookEvent>,
    mut reconfig_rx: mpsc::Receiver<HookReconfig>,
) {
    let mut io = io;
    io.events_tx = Some(events_tx.clone());
    let surfaces = Arc::clone(&io.surfaces);

    if io.secret_missing() {
        let _ = events_tx.send(HookEvent::SecretMissing).await;
    }

    let mut registration: Option<MountGuard> = None;
    let entry = meclaw_colony::SurfaceEntry {
        kind: "proxy",
        cell_path: meclaw_core::Path::new(&io.cell_path),
        // A webhook mount answers POSTs, never a topic link.
        links: None,
    };
    let handoff = match surfaces.register(&io.mount, entry).await {
        Ok((rx, held)) => {
            registration = Some(MountGuard::new(&surfaces, &io.mount, held));
            Some(rx)
        }
        Err(e) => {
            let _ = events_tx.send(HookEvent::MountFailed(e.to_string())).await;
            None
        }
    };

    let mut serving = tokio::task::JoinSet::new();
    if let Some(mut rx) = handoff {
        let io = io.clone();
        serving.spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    handed = rx.recv() => match handed {
                        Some(handed) => {
                            connections.spawn(crate::handed::serve_handed(
                                handed.stream,
                                mounted_router(io.clone()),
                            ));
                        }
                        // A respawn registered over this entry.
                        None => break,
                    },
                    Some(_) = connections.join_next(), if !connections.is_empty() => {}
                }
            }
        });
    }

    // Only the handler going away ends this half. `HookReconfig` has no
    // values, so the only thing `recv` can return is `None`.
    if let Some(never) = reconfig_rx.recv().await {
        match never {}
    }
    drop(serving);
    drop(registration);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(
                axum::http::HeaderName::from_bytes(k.as_bytes()).expect("name"),
                v.parse().expect("value"),
            );
        }
        h
    }

    fn allow(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn filter_headers_table() {
        let all = headers(&[
            ("X-Probe-Event", "push"),
            ("x-probe-delivery", "d-1"),
            ("X-Probe-Other", "leak"),
            ("X-Multi", "a"),
            ("X-Multi", "b"),
        ]);
        let cases: Vec<(Vec<String>, Value)> = vec![
            (allow(&[]), json!({})),
            (
                allow(&["X-Probe-Event", "X-PROBE-DELIVERY"]),
                json!({"x-probe-event": "push", "x-probe-delivery": "d-1"}),
            ),
            (allow(&["x-multi"]), json!({"x-multi": "a, b"})),
            (allow(&["X-Absent"]), json!({})),
        ];
        for (allow, want) in cases {
            assert_eq!(
                Value::Object(filter_headers(&all, &allow)),
                want,
                "{allow:?}"
            );
        }
    }

    #[test]
    fn build_body_table() {
        let h = || {
            let mut m = Map::new();
            m.insert("x-probe-event".into(), json!("push"));
            m
        };
        let cases: Vec<(&[u8], &str, &str, Value)> = vec![
            (
                &br#"{"a":1}"#[..],
                "application/json",
                "",
                json!({"messages": [], "webhook": {"path": "", "content_type": "application/json",
                    "headers": {"x-probe-event": "push"}, "raw": "{\"a\":1}", "json": {"a": 1}}}),
            ),
            (
                &br#"{"a":1}"#[..],
                "Application/JSON; charset=utf-8",
                "sub/path",
                json!({"messages": [], "webhook": {"path": "sub/path",
                    "content_type": "Application/JSON; charset=utf-8",
                    "headers": {"x-probe-event": "push"}, "raw": "{\"a\":1}", "json": {"a": 1}}}),
            ),
            (
                &b"{not json"[..],
                "application/json",
                "",
                json!({"messages": [], "webhook": {"path": "", "content_type": "application/json",
                    "headers": {"x-probe-event": "push"}, "raw": "{not json"}}),
            ),
            (
                &br#"{"a":1}"#[..],
                "text/plain",
                "",
                json!({"messages": [], "webhook": {"path": "", "content_type": "text/plain",
                    "headers": {"x-probe-event": "push"}, "raw": "{\"a\":1}"}}),
            ),
            (
                &[0xffu8, 0x00, 0x41][..],
                "application/octet-stream",
                "",
                json!({"messages": [], "webhook": {"path": "",
                    "content_type": "application/octet-stream",
                    "headers": {"x-probe-event": "push"}, "raw_base64": "/wBB"}}),
            ),
            (
                &b""[..],
                "",
                "",
                json!({"messages": [], "webhook": {"path": "", "content_type": "",
                    "headers": {"x-probe-event": "push"}, "raw": ""}}),
            ),
        ];
        for (raw, ct, path, want) in cases {
            let got = build_body(raw, ct, h(), path);
            assert_eq!(got, want, "{ct} {raw:?}");
            // Every body the mount builds is one the colony delivers.
            meclaw_core::validate_ubf_body(&got).expect("a UBF body");
        }
    }

    /// The reason the body carries no turn: a turn with a foreign origin or
    /// an extra field is no UBF body, and the colony would refuse it.
    #[test]
    fn a_turn_would_not_have_been_a_valid_body() {
        let bad = json!({"messages": [{"origin": "webhook", "type": "text", "text": "x"}]});
        assert!(meclaw_core::validate_ubf_body(&bad).is_err());
    }

    fn io(kind: &str, secret: &str) -> WebhookIo {
        let mut v = json!({"platform": "webhook", "mount": "probe-hook", "route": "probe",
            "emit_to": "/", "verify": {"kind": kind, "because": "a test"}});
        if kind != "none" {
            v["verify"] = json!({"kind": kind, "secret": secret, "header": "X-Probe-Token"});
        }
        let p = WebhookParams::parse(&v).expect("params");
        WebhookIo::new(&p, "/hook", Arc::new(SurfaceRegistry::new()))
    }

    #[test]
    fn a_blank_secret_is_missing_only_where_a_proof_is_required() {
        assert!(io("token", "").secret_missing());
        assert!(io("hmac_sha256", "").secret_missing());
        assert!(!io("token", "t").secret_missing());
        assert!(!io("none", "").secret_missing());
        assert_eq!(io("token", "t").header, "x-probe-token");
        let d = format!("{:?}", io("token", "tok-921-dbg"));
        assert!(!d.contains("tok-921-dbg"), "{d}");
    }
}

//! `GET /colony/surfaces` — which name reaches which surface cell, and the same
//! table in the form a reverse proxy reads.
//!
//! A surface cell is reached under a declared mount name on the colony's one
//! listener (ADR-0031). The names live in the process's mount table, and this is
//! the door that reads it: the plain shape for a person or a script, the
//! `?format=traefik` shape for a proxy that polls. The cell path stays inside
//! the colony (ruling O-639-5) — the mount is the name a client is meant to
//! know, and the tree path is the hive boundary this endpoint exists to keep
//! invisible.
//!
//! A row is a mount and the cell type behind it, and that is the whole row: no
//! surface cell owns a port any more, so there is no second address to publish
//! beside the listener's (the wave of #653; ADR-0031 supersedes ADR-0014).
//!
//! # The Traefik document, and what it was checked against
//!
//! Two pages of the Traefik reference, read on 2026-09-09:
//!
//! - <https://doc.traefik.io/traefik/reference/install-configuration/providers/others/http/>
//!   — "The HTTP provider allows you to provide your dynamic configuration via
//!   an HTTP(S) endpoint, and uses the same configuration as the File Provider
//!   in YAML or JSON format." `pollInterval` (default `5s`) is the provider's
//!   own knob, so this endpoint carries no interval of its own.
//! - <https://doc.traefik.io/traefik/reference/routing-configuration/http/routing/rules-and-priority/>
//!   — a router takes a `rule` and a `service`, and "To set the value of a rule,
//!   use backticks ` or escaped double-quotes \". Single quotes ' are not
//!   accepted since the values are Go's String Literals." Hence the backticks
//!   around the path in [`traefik_rule`]. `PathPrefix` is the prefix match:
//!   "PathPrefix: /products would match /products but also /products/shoes",
//!   and the path is forwarded as-is, which is what the mounted cell's own
//!   router expects (it serves `/<mount>/…`).
//! - <https://doc.traefik.io/traefik/reference/routing-configuration/http/load-balancing/service/>
//!   — a service is `loadBalancer.servers`, each entry naming a `url`.
//!
//! The reference and the design agree, so the shape below is the spec's shape:
//! one router per mount, one service for the listener, under the top-level
//! `http` key of a dynamic configuration.

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use meclaw_colony::{MountRow, SurfaceRegistry};
use serde::Deserialize;
use serde_json::{Value, json};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

/// The one rendering this endpoint offers besides its own JSON.
pub const TRAEFIK_FORMAT: &str = "traefik";

/// Top-level key of a Traefik dynamic configuration.
pub const TRAEFIK_ROOT: &str = "http";

/// Prefix of the router name a mount gets, so two colonies behind one proxy do
/// not collide on a bare mount name.
pub const TRAEFIK_ROUTER_PREFIX: &str = "meclaw-";

/// The one service every router of this colony points at: the one listener.
pub const TRAEFIK_SERVICE: &str = "meclaw";

/// The address an unspecified bind is published as. A proxy cannot dial
/// `0.0.0.0`, and the only address that is certainly the same host is loopback.
const PUBLISHED_LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// Query of `GET /colony/surfaces`.
#[derive(Debug, Deserialize)]
pub struct SurfacesParams {
    /// `traefik` renders the table as a Traefik dynamic configuration. Absent is
    /// the plain table; anything else is refused.
    pub format: Option<String>,
}

/// The Traefik rule that sends `/<mount>/…` to this colony.
fn traefik_rule(mount: &str) -> String {
    format!("PathPrefix(`/{mount}`)")
}

/// Where a proxy reaches this colony, as a URL, or `None` when nothing can be
/// named.
///
/// The request's `Host` header wins: a poller reached this endpoint on an
/// address that works, and that is the address its own service entry has to
/// carry. Without one the bound listener is used, with an unspecified bind
/// (`0.0.0.0`, `[::]`) published as loopback. Neither available leaves the
/// service without servers rather than inventing one.
fn service_url(host: Option<&str>, listener: Option<SocketAddr>) -> Option<String> {
    if let Some(host) = host.filter(|h| !h.is_empty()) {
        return Some(format!("http://{host}"));
    }
    let addr = listener?;
    if addr.ip().is_unspecified() {
        return Some(format!(
            "http://{}",
            SocketAddr::new(PUBLISHED_LOOPBACK, addr.port())
        ));
    }
    Some(format!("http://{addr}"))
}

/// The mount table as a Traefik dynamic configuration: one router per mount, one
/// service for the listener.
fn traefik_document(rows: &[MountRow], service_url: Option<String>) -> Value {
    let mut routers = serde_json::Map::new();
    for row in rows {
        routers.insert(
            format!("{TRAEFIK_ROUTER_PREFIX}{}", row.mount),
            json!({ "rule": traefik_rule(&row.mount), "service": TRAEFIK_SERVICE }),
        );
    }
    let servers: Vec<Value> = service_url
        .map(|url| vec![json!({ "url": url })])
        .unwrap_or_default();
    let mut http = serde_json::Map::new();
    http.insert("routers".into(), Value::Object(routers));
    http.insert(
        "services".into(),
        json!({ TRAEFIK_SERVICE: { "loadBalancer": { "servers": servers } } }),
    );
    let mut doc = serde_json::Map::new();
    doc.insert(TRAEFIK_ROOT.into(), Value::Object(http));
    Value::Object(doc)
}

/// `GET /colony/surfaces` — the mount table, plain or as a Traefik document.
///
/// A read of the process's mount table, not a message: there is no colony
/// round trip on this path, which is also why it answers while the colony is
/// draining.
pub async fn get_surfaces(
    State(surfaces): State<Arc<SurfaceRegistry>>,
    headers: HeaderMap,
    Query(params): Query<SurfacesParams>,
) -> impl IntoResponse {
    let rows = surfaces.table().await;
    let listener = surfaces.listener();
    match params.format.as_deref() {
        None => (
            StatusCode::OK,
            Json(json!({
                "listener": listener.map(|a| a.to_string()),
                "surfaces": rows,
            })),
        ),
        Some(TRAEFIK_FORMAT) => {
            let host = headers
                .get(axum::http::header::HOST)
                .and_then(|v| v.to_str().ok());
            (
                StatusCode::OK,
                Json(traefik_document(&rows, service_url(host, listener))),
            )
        }
        Some(_) => (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "bad_query",
                "detail": format!("format must be {TRAEFIK_FORMAT}"),
            })),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(mount: &str) -> MountRow {
        MountRow {
            mount: mount.to_string(),
            kind: "voice",
        }
    }

    #[test]
    fn the_host_header_wins_over_the_bound_listener() {
        let listener: SocketAddr = "127.0.0.1:7777".parse().expect("addr");
        assert_eq!(
            service_url(Some("meclaw.example:8080"), Some(listener)).as_deref(),
            Some("http://meclaw.example:8080")
        );
        // An empty header is no header: a proxy that sends one is not naming an
        // address, and `http://` is not a URL a service can be dialled on.
        assert_eq!(
            service_url(Some(""), Some(listener)).as_deref(),
            Some("http://127.0.0.1:7777")
        );
    }

    #[test]
    fn an_unspecified_bind_is_published_as_loopback() {
        for bind in ["0.0.0.0:7777", "[::]:7777"] {
            let addr: SocketAddr = bind.parse().expect("addr");
            assert_eq!(
                service_url(None, Some(addr)).as_deref(),
                Some("http://127.0.0.1:7777"),
                "{bind} is not an address a proxy can dial"
            );
        }
    }

    #[test]
    fn no_host_and_no_listener_leaves_the_service_without_servers() {
        assert_eq!(service_url(None, None), None);
        let doc = traefik_document(&[row("voice")], None);
        assert_eq!(
            doc["http"]["services"]["meclaw"]["loadBalancer"]["servers"],
            json!([]),
            "an unreachable colony names no server rather than a wrong one"
        );
        assert_eq!(
            doc["http"]["routers"]["meclaw-voice"]["rule"],
            "PathPrefix(`/voice`)"
        );
    }

    #[test]
    fn every_mount_is_a_router_of_its_own_on_the_one_service() {
        let doc = traefik_document(
            &[row("voice"), row("kiosk")],
            Some("http://127.0.0.1:7777".into()),
        );
        for mount in ["voice", "kiosk"] {
            let router = &doc["http"]["routers"][format!("meclaw-{mount}")];
            assert_eq!(router["rule"], format!("PathPrefix(`/{mount}`)"));
            assert_eq!(router["service"], "meclaw");
        }
        assert_eq!(
            doc["http"]["services"]["meclaw"]["loadBalancer"]["servers"][0]["url"],
            "http://127.0.0.1:7777"
        );
    }
}

//! The four axum handlers behind `/surface/*rest`.
//!
//! One wildcard route, one parser, four answers. Everything a caller must not be
//! able to distinguish collapses to the same 404 here: "no such cell", "that cell
//! declares no surface", "that path is not addressable". The one exception is a
//! **broken** declaration, which is reported with the typo named — it is the
//! operator's own mistake, and hiding it costs them an afternoon.

use super::{Target, assets, bundle, page, parse_target, socket};
use crate::router::SurfaceState;
use axum::extract::{Path as AxumPath, State, WebSocketUpgrade};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use meclaw_colony::surface::{LocateError, Located, locate};

/// `GET /surface/*rest` — the page, an asset, a bundle, or the socket.
///
/// One handler for all four because axum allows one route per wildcard pattern and
/// the transport lives under the same prefix by design. The upgrade is an
/// `Option` extractor: present exactly when the client asked for a websocket, so
/// the socket arm cannot be reached by an ordinary GET and an ordinary GET cannot
/// accidentally be upgraded.
pub async fn get_surface(
    State(state): State<SurfaceState>,
    AxumPath(rest): AxumPath<String>,
    upgrade: Option<WebSocketUpgrade>,
) -> Response {
    let Some(target) = parse_target(rest.trim_start_matches('/')) else {
        return miss();
    };
    match target {
        Target::Client { file } => match bundle(&file) {
            Some((ctype, body)) => {
                ([(header::CONTENT_TYPE, ctype)], body.to_string()).into_response()
            }
            None => miss(),
        },
        Target::Page { cell } => match resolve(&state, &cell) {
            Ok(located) => (
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                page::dead_render(&located, &cell),
            )
                .into_response(),
            Err(r) => *r,
        },
        Target::Asset { cell, file } => match resolve(&state, &cell) {
            Ok(located) => match assets::read_asset(&located, &file) {
                Some((ctype, bytes)) => ([(header::CONTENT_TYPE, ctype)], bytes).into_response(),
                None => miss(),
            },
            Err(r) => *r,
        },
        Target::Socket { cell } => {
            // Resolve BEFORE the upgrade, so an undeclared cell has no transport
            // either: the 404 rule holds on every route, not just the page.
            let located = match resolve(&state, &cell) {
                Ok(l) => l,
                Err(r) => return *r,
            };
            match upgrade {
                Some(up) => {
                    let dispatcher = state.dispatcher.clone();
                    let cell_path = located.cell_path.clone();
                    up.on_upgrade(move |ws| socket::Connection::new(cell_path, dispatcher).run(ws))
                }
                // The path is right, the request is not. 400, not 404.
                None => (
                    StatusCode::BAD_REQUEST,
                    "this path is a websocket endpoint\n",
                )
                    .into_response(),
            }
        }
    }
}

/// Resolve a URL cell path, mapping every miss onto the same 404.
fn resolve(state: &SurfaceState, cell: &str) -> Result<Located, Box<Response>> {
    match locate(&state.colony_root, cell) {
        Ok(l) => Ok(l),
        Err(LocateError::Malformed(m)) => Err(Box::new(
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("this surface's declaration is broken: {m}\n"),
            )
                .into_response(),
        )),
        // NotFound and NoSurface are ONE answer on purpose.
        Err(_) => Err(Box::new(miss())),
    }
}

/// The one negative answer. 404, never 403: a surface nobody declared should not
/// confirm that it exists.
fn miss() -> Response {
    (StatusCode::NOT_FOUND, "no such surface\n").into_response()
}

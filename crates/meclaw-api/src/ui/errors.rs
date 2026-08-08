//! HTML error pages for `/ui/*`. Spec clarification: reads have **no 422** —
//! `422 Unprocessable Entity` is semantically POST-mutation-only. UI validation
//! errors on read endpoints (e.g. an invalid UUID format) map onto 400
//! `Bad Request` + an HTML page.
//!
//! 500 for server-side problems (colony inbox down, ack dropped) is the generic
//! "something went wrong", without showing the operator stack traces.

use crate::ui::layout;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use maud::html;

/// Render `400 Bad Request` with a detail string.
pub(crate) fn render_400(detail: &str) -> Response {
    let content = html! {
        p class="error" { "400 Bad Request" }
        p { (detail) }
        p { a href="/ui/" { "Zurück zum Dashboard" } }
    };
    (
        StatusCode::BAD_REQUEST,
        Html(layout("400 Bad Request", content).into_string()),
    )
        .into_response()
}

/// Render `500 Internal Server Error` with a detail string.
pub(crate) fn render_500(detail: &str) -> Response {
    let content = html! {
        p class="error" { "500 Internal Server Error" }
        p { (detail) }
        p { a href="/ui/" { "Zurück zum Dashboard" } }
    };
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Html(layout("500 Internal Server Error", content).into_string()),
    )
        .into_response()
}

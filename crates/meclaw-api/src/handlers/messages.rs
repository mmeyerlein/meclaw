//! POST /messages — fire-and-forget message injection. Spec l.1644/l.1656/l.1658.
//!
//! Dual content-type handling (Phase 12-X T18):
//!   * `application/json`: classic JSON `{target, body, headers?}` -> inline UBF body.
//!   * `multipart/form-data`: `target` text field + 1..n file fields; each file
//!     streams into `DiskBlobStore`, the resulting `BlobRef`s land in the
//!     `attachments[]` slot of the synthesized UBF body. The JSON path stays
//!     untouched, no phase jumping (no Body::Blob, no inline cap).
//!
//! The HTTP layer builds a `Message` with an arbitrary `target` and sends it via
//! `ColonyMsg::Route` through the same routing path as an internal message.
//! Response 202 + `{message_id}` (multipart additionally `attachments: [BlobRef]`);
//! any cell reply travels through the routing cascade, NOT back over HTTP.
//! `sender_path` is `/` (root), analogous to external/test senders (see the
//! `ColonyMsg::Route` doc comment).
//!
//! `trace_id` and `message_id` are identical to the generated `Uuid::now_v7`;
//! source messages have `parent_message_id = None` and `reply_to = None`
//! (spec § Message model).

use crate::AppState;
use axum::Json;
use axum::extract::{FromRequest, Multipart, State};
use axum::http::{HeaderMap, StatusCode, header::CONTENT_TYPE};
use meclaw_colony::ColonyMsg;
use meclaw_core::{BlobRef, Body, MessageBuilder, Path, Uuid};
use serde::Deserialize;
use serde_json::{Map, Value};

/// Request body for the JSON `POST /messages`. `target` is mandatory, `body` is
/// an arbitrary JSON value (UBF validation happens downstream in routing),
/// `headers` is optional and defaults to an empty object.
///
/// TTL slice (2026-06-11): `ttl` is the optional per-initial-message override
/// (spec § Message model, TTL semantics). Absent/`null` → colony.json
/// `message_default_ttl` (AppState). Declared as a `Value` so that EVERY
/// non-positive-integer value (0, negative, float, string, > u32::MAX) is
/// answered with 422 `invalid_ttl` instead of a generic 400 type error.
#[derive(Debug, Deserialize)]
pub struct MessageRequest {
    pub target: String,
    pub body: Value,
    #[serde(default)]
    pub headers: Value,
    #[serde(default)]
    pub ttl: Option<Value>,
}

/// Validate the optional request `ttl`: absent/`null` → `None` (caller falls
/// back to the colony default); a positive integer in `1..=u32::MAX` → that
/// value; everything else → `Err` with the rejection detail (422 `invalid_ttl`).
fn validate_request_ttl(ttl: &Option<Value>) -> Result<Option<u32>, String> {
    match ttl {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => match n.as_u64() {
            Some(x) if (1..=u32::MAX as u64).contains(&x) => Ok(Some(x as u32)),
            _ => Err(format!(
                "ttl must be a positive integer in 1..={}, got {n}",
                u32::MAX
            )),
        },
        Some(other) => Err(format!("ttl must be a positive integer, got {other}")),
    }
}

/// `POST /messages` — fire-and-forget. Returns 202 + `{message_id}` (JSON path)
/// or 202 + `{message_id, attachments}` (multipart path).
///
/// When the inbox is down: 503 with `{error}`. Every other failure mode
/// (unresolved target, TTL expired, InvalidUbfBody) lands in the DLQ and is by
/// definition invisible from the HTTP side — the fire-and-forget semantics only
/// guarantee "the message was submitted to the routing queue".
///
/// Branching on `Content-Type` happens up here rather than in the router,
/// because axum has no native content-type-based dispatch.
pub async fn post_messages(
    State(state): State<AppState>,
    req: axum::extract::Request,
) -> (StatusCode, Json<Value>) {
    let content_type = req
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    if content_type.starts_with("multipart/form-data") {
        let multipart = match Multipart::from_request(req, &state).await {
            Ok(m) => m,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": "multipart_parse",
                        "detail": format!("{e}"),
                    })),
                );
            }
        };
        post_messages_multipart(state, multipart).await
    } else {
        // Default to JSON. Use the same axum extractor semantics as before by
        // converting the request body to bytes and deserializing manually —
        // mirrors what `Json<MessageRequest>` would do under the hood, but
        // keeps the response shape (4xx with our `{error,detail}`) consistent.
        let headers = req.headers().clone();
        let body = req.into_body();
        let bytes = match axum::body::to_bytes(body, 10 * 1024 * 1024).await {
            Ok(b) => b,
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": "body_read",
                        "detail": "body too large or stream error",
                    })),
                );
            }
        };
        post_messages_json(state, headers, &bytes).await
    }
}

/// JSON path: parse `MessageRequest`, build an inline-body message, fire and forget.
async fn post_messages_json(
    state: AppState,
    headers: HeaderMap,
    bytes: &[u8],
) -> (StatusCode, Json<Value>) {
    // Mirror axum::Json's Content-Type check so we return our `{error,detail}`
    // shape rather than axum's plain-text 415.
    let ct = headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !ct.is_empty() && !ct.starts_with("application/json") {
        return (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Json(serde_json::json!({
                "error": "unsupported_media_type",
                "detail": format!("expected application/json, got {ct}"),
            })),
        );
    }
    let req: MessageRequest = match serde_json::from_slice(bytes) {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "invalid_json",
                    "detail": format!("{e}"),
                })),
            );
        }
    };
    // Edge validation (always-on, release too): the HTTP ingress is a trust
    // boundary, so every source body is validated against the UBF schema BEFORE
    // it enters the routing path. Reject with 422 rather than letting a
    // malformed body reach a cell. Spec § Schema validation (edge validation
    // always-on).
    if let Err(reason) = meclaw_core::validate_ubf_body(&req.body) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "invalid_ubf_body",
                "detail": reason,
            })),
        );
    }
    // TTL slice (2026-06-11): validate the optional per-message ttl BEFORE
    // routing — only positive integers pass; everything else is 422.
    let ttl = match validate_request_ttl(&req.ttl) {
        Ok(t) => t.unwrap_or(state.message_default_ttl),
        Err(detail) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": "invalid_ttl",
                    "detail": detail,
                })),
            );
        }
    };
    let target = Path::new(&req.target);
    let req_headers = match req.headers {
        Value::Object(map) => map,
        Value::Null => Map::new(),
        _ => Map::new(),
    };
    // Source message from the HTTP ingress: no previous hop. The inbound headers
    // go into the persistent `context` compartment (correlation / long-lived);
    // `hop` starts empty (slice 2, two-compartment model).
    let msg = MessageBuilder::new(target)
        .context(req_headers)
        .ttl(ttl)
        .body(Body::Inline(req.body))
        .build();
    let message_id: Uuid = msg.id;
    let sender_path = Path::new("/");
    if state
        .colony
        .inbox
        .send(ColonyMsg::Route { sender_path, msg })
        .await
        .is_err()
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "colony unavailable" })),
        );
    }
    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "message_id": message_id.to_string() })),
    )
}

/// Multipart path: stream each file field into DiskBlobStore, collect BlobRefs,
/// build the UBF body with an `attachments[]` slot.
///
/// No phase jumping: no inline-cap logic (phase 13), no Body::Blob (T18 scope).
/// Files are pulled fully into RAM via `Field::bytes()` — acceptable for phase
/// 12; a phase-13+ streaming path needs `tokio_util::StreamReader`.
async fn post_messages_multipart(
    state: AppState,
    mut multipart: Multipart,
) -> (StatusCode, Json<Value>) {
    let mut target: Option<String> = None;
    let mut attachments: Vec<BlobRef> = Vec::new();

    loop {
        let field = match multipart.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": "multipart_field",
                        "detail": format!("{e}"),
                    })),
                );
            }
        };
        let name = field.name().unwrap_or("").to_string();
        if name == "target" {
            match field.text().await {
                Ok(t) => target = Some(t),
                Err(e) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({
                            "error": "multipart_field",
                            "detail": format!("target read: {e}"),
                        })),
                    );
                }
            }
        } else {
            // File field — read into memory, then stream into the blob store.
            let filename = field.file_name().map(String::from);
            let mime = field
                .content_type()
                .map(String::from)
                .unwrap_or_else(|| "application/octet-stream".into());
            let bytes = match field.bytes().await {
                Ok(b) => b,
                Err(e) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({
                            "error": "multipart_field",
                            "detail": format!("file body read: {e}"),
                        })),
                    );
                }
            };
            let reader = std::io::Cursor::new(bytes);
            match state
                .blob_store
                .write_streaming(reader, &mime, filename.as_deref())
                .await
            {
                Ok(blob_ref) => attachments.push(blob_ref),
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "error": "blob_write",
                            "detail": format!("{e}"),
                        })),
                    );
                }
            }
        }
    }

    let target_str = match target {
        Some(t) => t,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "missing_field",
                    "detail": "multipart body must include a `target` text field",
                })),
            );
        }
    };

    // Build the UBF body with the `attachments[]` slot only. Per no-phase-jumping:
    // attachments[] lives alongside whatever the downstream cell expects;
    // we do NOT synthesize a `messages[]` slot here (the client owns that
    // for JSON; multipart is for uploads, not conversation turns).
    let attachments_json: Vec<Value> = attachments
        .iter()
        .map(|a| serde_json::to_value(a).expect("BlobRef serializes"))
        .collect();
    let body_json = serde_json::json!({ "attachments": attachments_json });

    // Edge validation (always-on, release too): same trust-boundary discipline
    // as the JSON path. The synthesized attachments-only body is a valid UBF
    // shape (anyOf attachments-branch). Reject with 422 on any violation BEFORE
    // routing. Spec § Schema validation (edge validation always-on).
    //
    // Defense in depth, constructionally unreachable (DECISION A / finding 2):
    // there is NO client-authored body on this path — the client uploads files
    // and the substrate SYNTHESIZES `body_json` from `BlobRef`s above, each of
    // which serializes to a schema-valid `Attachment` (and the file-less case is
    // `{"attachments":[]}`, still a valid anyOf branch). So this 422 branch
    // cannot fire from any real multipart request; it is kept as a guard so a
    // future change to the synthesis cannot silently emit an invalid body. A
    // client-reachable 422 only arises on the JSON path. NOT dead code / NOT a
    // missing validation — see docs/meclaw-overview.md § Schema validation.
    if let Err(reason) = meclaw_core::validate_ubf_body(&body_json) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "invalid_ubf_body",
                "detail": reason,
            })),
        );
    }

    // TTL slice (2026-06-11): the multipart path has no per-message override
    // surface (no `ttl` form field — uploads, not conversation turns); the
    // colony.json default applies.
    let msg = MessageBuilder::new(Path::new(&target_str))
        .ttl(state.message_default_ttl)
        .body(Body::Inline(body_json))
        .build();
    let message_id: Uuid = msg.id;
    let sender_path = Path::new("/");
    if state
        .colony
        .inbox
        .send(ColonyMsg::Route { sender_path, msg })
        .await
        .is_err()
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "colony unavailable" })),
        );
    }

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "message_id": message_id.to_string(),
            "attachments": attachments_json,
        })),
    )
}

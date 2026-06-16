//! POST /messages — fire-and-forget message injection. Spec Z.1644/Z.1656/Z.1658.
//!
//! Dual content-type handling (Phase 12-X T18):
//!   * `application/json`: classic JSON `{target, body, headers?}` -> inline UBF body.
//!   * `multipart/form-data`: `target` text field + 1..n file fields; each file
//!     streams into `DiskBlobStore`, the resulting `BlobRef`s land in the
//!     `attachments[]` slot of the synthesized UBF body. JSON-Pfad bleibt
//!     anti-Vorgriff-konform unangetastet (kein Body::Blob, kein Inline-Cap).
//!
//! HTTP-Layer baut eine `Message` mit beliebigem `target` und schickt sie via
//! `ColonyMsg::Route` durch denselben Routing-Pfad wie eine interne Message.
//! Antwort 202 + `{message_id}` (multipart zusaetzlich `attachments: [BlobRef]`);
//! eine etwaige Cell-Antwort laeuft ueber die Routing-Cascade, NICHT ueber HTTP
//! zurueck. `sender_path` ist `/` (Root) analog zu externen/Test-Sendern (siehe
//! `ColonyMsg::Route` Doc-Comment).
//!
//! `trace_id` und `message_id` sind identisch zur generierten `Uuid::now_v7`;
//! Source-Messages haben `parent_message_id = None` und `reply_to = None`
//! (Spec § Message-Modell).

use crate::AppState;
use axum::Json;
use axum::extract::{FromRequest, Multipart, State};
use axum::http::{HeaderMap, StatusCode, header::CONTENT_TYPE};
use meclaw_colony::ColonyMsg;
use meclaw_core::{BlobRef, Body, MessageBuilder, Path, Uuid};
use serde::Deserialize;
use serde_json::{Map, Value};

/// Request-Body fuer JSON-`POST /messages`. `target` ist Pflicht, `body` ist
/// arbitraerer JSON-Wert (UBF-Validierung passiert downstream im Routing),
/// `headers` ist optional und defaultet auf leeres Objekt.
///
/// TTL slice (2026-06-11): `ttl` ist das optionale per-Initial-Message-Override
/// (spec § Message-Modell, TTL-Semantik). Absent/`null` → colony.json
/// `message_default_ttl` (AppState). Als `Value` deklariert, damit JEDER
/// Nicht-positive-Integer-Wert (0, negativ, float, string, > u32::MAX) als 422
/// `invalid_ttl` beantwortet wird statt als generischer 400-Typ-Fehler.
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

/// `POST /messages` — fire-and-forget. Returnt 202 + `{message_id}`
/// (JSON-Pfad) oder 202 + `{message_id, attachments}` (multipart-Pfad).
///
/// Bei Inbox-Down: 503 mit `{error}`. Alle anderen Failure-Modes (unresolved
/// Target, TTL-Expired, InvalidUbfBody) landen im DLQ und sind aus HTTP-Sicht
/// per Definition unsichtbar — die fire-and-forget-Semantik garantiert nur
/// "Message wurde in die Routing-Queue eingereicht".
///
/// Branching auf `Content-Type` passiert hier oben statt im Router, weil axum
/// keine native Content-Type-basierte Dispatch hat.
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

/// JSON-Pfad: parse `MessageRequest`, build inline-body Message, fire+forget.
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
    // malformed body reach a cell. Spec § Schema-Validierung (Rand-Validierung
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
    // Source-Message vom HTTP-Ingress: kein vorheriger Hop. Die eingehenden
    // Header gehen ins persistente `context`-Fach (Korrelation/Langlebiges);
    // `hop` startet leer (Slice 2 Zwei-Fächer-Modell).
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

/// Multipart-Pfad: stream each file field into DiskBlobStore, collect BlobRefs,
/// build UBF body with `attachments[]` slot.
///
/// Anti-Vorgriff: keine Inline-Cap-Logik (Phase 13), kein Body::Blob (T18 scope).
/// Files werden via `Field::bytes()` voll in den RAM gezogen — fuer Phase 12
/// akzeptabel; Phase-13+ Streaming-Pfad braucht `tokio_util::StreamReader`.
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

    // Build the UBF body with `attachments[]` slot only. Per Anti-Vorgriff:
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
    // routing. Spec § Schema-Validierung (Rand-Validierung always-on).
    //
    // Defense-in-depth, constructionally unreachable (DECISION A / Befund-2):
    // there is NO client-authored body on this path — the client uploads files
    // and the substrate SYNTHESIZES `body_json` from `BlobRef`s above, each of
    // which serializes to a schema-valid `Attachment` (and the file-less case is
    // `{"attachments":[]}`, still a valid anyOf branch). So this 422 branch
    // cannot fire from any real multipart request; it is kept as a guard so a
    // future change to the synthesis cannot silently emit an invalid body. A
    // client-reachable 422 only arises on the JSON path. NOT dead code / NOT a
    // missing validation — see docs/meclaw-overview.md § Schema-Validierung.
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

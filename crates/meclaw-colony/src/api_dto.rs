//! Phase 12-B: DTOs for the typed `ColonyMsg::Read*` Inbox-API.
//!
//! Each `Read*` variant carries an `ack: oneshot::Sender<Read*Reply>`. The HTTP-handlers
//! in Slice 12-B Task 8 will marshal these replies to JSON, so every DTO derives
//! `serde::Serialize + serde::Deserialize`.
//!
//! **Why mirror-DTOs and not the in-memory structs?** `meclaw_core::Path` and
//! `meclaw_core::Uuid` are not `Serialize` in this workspace (no `serde` feature on
//! the upstream crate). The DTOs convert to plain `String`s at the Inbox-arm boundary,
//! keeping the upstream types untouched.

use serde::{Deserialize, Serialize};

/// Reply for [`crate::ColonyMsg::ReadRegistry`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadRegistryReply {
    /// Filtered + capped snapshot of the Colony registry.
    pub entries: Vec<RegistryEntryDto>,
}

/// Per-cell registry projection (path + cell_id + cell_type only — no `ActorHandle`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryEntryDto {
    /// Registry-key string, e.g. `"/foo/bar"`.
    pub path: String,
    /// Cell-ID as UUID-string (assigned at Register-time).
    pub cell_id: String,
    /// Cell-type string from `config.json` (e.g. `"echo"`, `"llm"`).
    pub cell_type: String,
    /// Phase-13 lifecycle status: "Awake" | "Asleep" | "NotYetSpawned".
    pub lifecycle_status: String,
    /// Phase-13.5 Lifecycle-3b: edge-derived activity (persisted in
    /// `colony.db.registry.status`). `true` == active, mirrors
    /// `RegistryEntry.active`; orthogonal to `lifecycle_status`.
    pub active: bool,
    /// Paket 6 / P7: `true` once the cell exhausted its restart limit and is
    /// `failed` (persisted in `colony.db.registry.status` as `"failed"`). Mirrors
    /// `RegistryEntry.failed`; distinguishes a failed cell (`active:false,
    /// failed:true`) from a merely disconnected one (`active:false, failed:false`).
    pub failed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_dto_has_lifecycle_status_field() {
        let _: fn(&RegistryEntryDto) -> &String = |d| &d.lifecycle_status;
    }

    /// Phase-13.5-lifecycle-3b T8.1: `RegistryEntryDto` carries `active: bool`
    /// (projected from `RegistryEntry.active` in `handle_read_registry`).
    #[test]
    fn registry_dto_has_active_field() {
        let _: fn(&RegistryEntryDto) -> &bool = |d| &d.active;
    }
}

/// Reply for [`crate::ColonyMsg::ReadDeadLetters`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadDeadLettersReply {
    /// Filtered + capped snapshot. Pure read — does NOT drain (use
    /// `DrainDeadLetters` for the DELETE-path).
    pub entries: Vec<DeadLetterDto>,
}

/// Reply for [`crate::ColonyMsg::ReadTrace`].
///
/// `entries` are rows from the `colony.db::message_log` table, sorted by
/// `created_at` ASC. Subject to the limit hard-cap of 1000.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadTraceReply {
    /// Filtered + capped audit-rows.
    pub entries: Vec<MessageLogDto>,
}

/// DTO mirror of [`crate::persist::writer::MessageLogRow`] for HTTP-marshal.
///
/// The `headers_json` column is included raw; the HTTP-handler in Task 8 may
/// parse `error_code`/`reply_to` out of it as needed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageLogDto {
    /// Message-ID (UUID v7 as string).
    pub id: String,
    /// Trace-root-ID (constant across the trace chain).
    pub trace_id: String,
    /// Parent message-ID; None for source-messages.
    pub parent_message_id: Option<String>,
    /// Correlation-ID for request/response pairing.
    pub correlation_id: Option<String>,
    /// Post-decrement TTL at the hop.
    pub ttl: i64,
    /// Sender path; `"@external"` sentinel for source-messages.
    pub from_path: String,
    /// Resolved receiver path.
    pub to_path: String,
    /// Reply-target (cell address for error replies).
    pub reply_to: Option<String>,
    /// Headers as raw JSON string (HTTP-handler may parse `error_code` etc.).
    pub headers_json: String,
    /// Body variant: `"inline"` or `"blob"`.
    pub body_kind: String,
    /// Body payload: JSON if inline, UUID-string if blob.
    pub body_payload: Option<String>,
    /// Unix seconds at row creation.
    pub created_at: i64,
}

/// Reply for [`crate::ColonyMsg::ReadGraph`].
///
/// `graph_version` is tracked at the Colony-Inbox level; for now (Phase 12-B)
/// it is **always `0`**. Real graph-version bumping (per registry/edge mutation)
/// is a Phase-14 backlog item — clients using this for pull-cache-invalidation
/// must treat `0` as "no caching" until the bump is wired.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadGraphReply {
    /// The scope-prefix the reply was filtered by, echoed for client caching.
    pub scope: String,
    /// Monotonic graph-version. **Currently always `0`** — Phase-14 backlog.
    pub graph_version: u64,
    /// Cells whose path falls inside the scope-prefix (Registry-projection).
    pub nodes: Vec<GraphNodeDto>,
    /// Edges where BOTH endpoints fall inside the scope-prefix.
    pub edges: Vec<GraphEdgeDto>,
}

/// Per-node graph projection (mirror of `RegistryEntryDto` plus possible
/// per-node graph metadata in future phases).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphNodeDto {
    /// Cell-path string.
    pub path: String,
    /// Cell-type string from `config.json`.
    pub cell_type: String,
}

/// Per-edge graph projection from the in-memory `EdgeTable`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdgeDto {
    /// Edge-ID (UUID v7 as string).
    pub id: String,
    /// Edge source path string.
    pub from: String,
    /// Edge target path string.
    pub to: String,
    /// Phase 13.5-A1 (F6-PIN): Source string of the edge's `condition` CEL
    /// expression, exposed for `remove_edges`/`swap_nodes` match-pattern
    /// string-equality comparisons. `None` = edge has no condition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    /// Phase 13.5-A1 (F6-PIN): Source `ModifierSpec` of the edge's `modifier`,
    /// rendered as a JSON value for serde-roundtrip string-equality. `None`
    /// = edge has no modifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modifier: Option<meclaw_core::JsonValue>,
}

/// Reply for [`crate::ColonyMsg::ReadMutationsAudit`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadMutationsAuditReply {
    /// Filtered + capped audit-rows from `colony.db::mutation_log`,
    /// sorted by `created_at` ASC.
    pub entries: Vec<MutationLogDto>,
}

/// DTO mirror of [`crate::persist::colony_db::MutationLogRow`].
/// Same fields, plain types — added `Serialize`/`Deserialize` for HTTP-marshal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutationLogDto {
    /// Mutation-ID (UUID v7 as string).
    pub id: String,
    /// Scope-path string.
    pub scope: String,
    /// Original payload as JSON-string.
    pub payload_json: String,
    /// Status: `"in_flight"` | `"committed"` | `"failed"` | `"rejected"`.
    pub status: String,
    /// Optional failure-reason. For `status="failed"`: the crash reason; for
    /// `status="rejected"` (A6): the human-readable reject reason.
    pub failure_reason: Option<String>,
    /// Unix-seconds at row creation.
    pub created_at: i64,
    /// Unix-seconds at commit/fail (NULL while in-flight).
    pub committed_at: Option<i64>,
    /// Phase-16 W3 (A6): `error_code` of a validate-stage reject row
    /// (only set when `status="rejected"`).
    pub error_code: Option<String>,
    /// Phase-16 W3 (A6): trace-id of the rejected mutation request
    /// (only set when `status="rejected"`).
    pub trace_id: Option<String>,
}

/// Reply for [`crate::ColonyMsg::ReadTemplates`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadTemplatesReply {
    /// Filtered + capped snapshot from the `templates` table.
    pub entries: Vec<TemplateEntryDto>,
}

/// Per-template projection from the `colony.db::templates` table.
///
/// **Known limitation**: `cell_type` filter on `ReadTemplates` is currently no-op.
/// The Spec `/colony/templates?type=` filter would require parsing each template's
/// `config.json::type`-field, which lives in the filesystem (not in colony.db).
/// Phase-14 backlog: either cache `cell_type` in the `templates` table at scan
/// time, or do an on-the-fly FS-read in the HTTP-handler.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateEntryDto {
    /// Template-ID (UUID v7 as string).
    pub template_id: String,
    /// Template name (from `template.json::name`).
    pub name: String,
    /// Optional semantic-version string.
    pub version: Option<String>,
    /// Absolute filesystem path of the template directory.
    pub filesystem_path: String,
    /// Optional author string from `template.json::author`.
    pub author: Option<String>,
}

/// DTO mirror of [`crate::DeadLetter`]; the in-memory struct holds a `Message`
/// envelope which is not `Serialize`. Only the audit-relevant fields are
/// exposed here.
///
/// Phase-16 W2 (Ruling A2): the DLQ is **self-locating** — every entry carries
/// `trace_id` + `created_at` on top of the sender + dying-edge path fields, and
/// the `since` filter on `ReadDeadLetters` now actually filters. Both are read
/// off the dead-lettered message envelope (`message.trace_id` / `message.created_at`,
/// stamped by `MessageBuilder` at message birth) — no separate timestamp on the
/// in-memory `DeadLetter`. Phase-16 W6d (Ruling A6): DLQ durability (reboot-
/// survival) is now **realized** — the DLQ persists in `colony.db` (`dead_letters`
/// table). This DTO is the HTTP read projection; the persisted row additionally
/// carries the full `message_json` envelope so the drain reconstructs the verbatim
/// `DeadLetter`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadLetterDto {
    /// Sender path string (the cell/hive whose emission could not be routed).
    pub sender_path: String,
    /// Original target before any resolution (start of the dying edge).
    pub original_target: String,
    /// Resolved target the router last tried (end of the dying edge).
    pub resolved_target: String,
    /// Canonical error_code string (per `DeadLetterReason::as_code`).
    pub error_code: String,
    /// Trace id of the dead-lettered message, for cross-referencing `/colony/trace`.
    pub trace_id: String,
    /// Unix-seconds timestamp of the dead-lettered message (powers `?since=`).
    pub created_at: i64,
    /// P1 (message browser): id of the dead-lettered message, parsed out of the
    /// persisted `message_json` envelope. `None` for rows written before this
    /// field existed, or whose envelope carries no `id` — consumers fall back to
    /// the trace-level link rather than treating it as an error.
    #[serde(default)]
    pub message_id: Option<String>,
}

/// Filter for [`crate::ColonyMsg::ReadMessages`] — the P1 message browser.
///
/// All fields are optional and AND-combined. The dispatch helper splits them into
/// two groups: **indexed** predicates (`trace_id`, `parent_message_id`,
/// `to_path_prefix`, `since`/`until`, `before`) narrow the row set through an
/// existing `message_log` index, **residual** predicates (`correlation_id`,
/// `from_path_prefix`, `body_kind`) are applied afterwards, inside the window the
/// indexed pass produced. `message_log` has no index on `from_path`, `body_kind`
/// or `correlation_id` (`persist::schema` DDL) — hence the window, and hence
/// [`ReadMessagesReply::scan_truncated`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MessageLogFilter {
    /// Exact message id — single-row lookup via PRIMARY KEY. Wins over every
    /// other field when set.
    pub id: Option<String>,
    /// Exact `trace_id` (indexed: `idx_msglog_trace`).
    pub trace_id: Option<String>,
    /// Exact `parent_message_id` (indexed: `idx_msglog_parent`).
    pub parent_message_id: Option<String>,
    /// Exact `correlation_id` — residual predicate, no index.
    pub correlation_id: Option<String>,
    /// `to_path` prefix, range-encoded (indexed: `idx_msglog_to`).
    pub to_path_prefix: Option<String>,
    /// `from_path` prefix, range-encoded — residual predicate, no index.
    pub from_path_prefix: Option<String>,
    /// Exact `body_kind` (`"inline"` | `"blob"`) — residual predicate, no index.
    pub body_kind: Option<String>,
    /// Lower bound on `created_at` (Unix seconds, inclusive).
    pub since: Option<i64>,
    /// Upper bound on `created_at` (Unix seconds, inclusive).
    pub until: Option<i64>,
    /// Keyset cursor: return only rows strictly older than `(created_at, id)`.
    pub before: Option<MessageLogCursor>,
    /// Rows returned. Clamped to `1..=1000` by the dispatch helper.
    pub limit: usize,
    /// Hard ceiling on rows READ before residual filtering. Clamped to
    /// `1..=50_000` by the dispatch helper; the HTTP layer defaults it to 5000.
    pub scan_budget: usize,
}

/// Keyset-pagination cursor: the `(created_at, id)` pair of the last row on a page.
///
/// `created_at` has second granularity, so ties are common; `id` (UUIDv7 string)
/// is the stable tie-breaker. Keyset paging keeps every page index-driven —
/// `OFFSET` would make the scan grow with the page number.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageLogCursor {
    /// `created_at` of the last row on the previous page.
    pub created_at: i64,
    /// `id` of the last row on the previous page.
    pub id: String,
}

/// Reply for [`crate::ColonyMsg::ReadMessages`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadMessagesReply {
    /// Rows, newest first (`created_at DESC, id DESC`).
    pub entries: Vec<MessageLogDto>,
    /// Cursor for the next (older) page; `None` when the page was not full.
    pub next: Option<MessageLogCursor>,
    /// The scan budget actually applied (post-clamp) — the UI reports it verbatim
    /// when `scan_truncated` is set.
    pub scan_budget: usize,
    /// `true` when the scan budget was exhausted, i.e. the residual predicates
    /// (`from_path_prefix`, `body_kind`, `correlation_id`) only saw that window.
    /// Callers MUST surface this; silent truncation reads as "nothing matched".
    pub scan_truncated: bool,
}

#[cfg(test)]
mod message_log_dto_tests {
    use super::*;

    #[test]
    fn message_log_filter_defaults_are_empty() {
        let f = MessageLogFilter::default();
        assert!(f.id.is_none());
        assert!(f.trace_id.is_none());
        assert!(f.before.is_none());
        assert_eq!(f.limit, 0, "limit is set explicitly by the caller");
        assert_eq!(
            f.scan_budget, 0,
            "scan_budget is set explicitly by the caller"
        );
    }

    #[test]
    fn read_messages_reply_roundtrips_through_json() {
        let reply = ReadMessagesReply {
            entries: Vec::new(),
            next: Some(MessageLogCursor {
                created_at: 42,
                id: "abc".into(),
            }),
            scan_budget: 5000,
            scan_truncated: true,
        };
        let s = serde_json::to_string(&reply).expect("serialize");
        let back: ReadMessagesReply = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(back.next.expect("cursor").created_at, 42);
        assert!(back.scan_truncated);
        assert_eq!(back.scan_budget, 5000);
    }
}

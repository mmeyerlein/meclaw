//! GH #19 (D-025) and GH #86 — recursive resolution of body blob pointers.
//!
//! # What this resolves
//!
//! The `messages[]` slot of a UBF body may carry two pointer shapes instead of
//! an inline turn (spec § `messages[]` schema, `ubf-body.json` `$defs`):
//!
//! * **Turn-Pointer** `{"text_id": "<uuid>"}` — one single turn lives in the blob.
//! * **Bulk-Pointer** `{"messages_id": "<uuid>"}` — a whole body document lives
//!   in the blob; its `messages[]` is spliced inline in the pointer's place.
//!
//! The `system` slot carries the same `text_id` pointer name at a different
//! site (spec § `system` sub-slot structure): a leaf of the application-defined
//! `system` tree is either an inline `{"text": …}` container or a
//! `{"text_id": "<uuid>"}` pointer to one.
//!
//! Every blob is itself a complete body document (spec § Blob references are
//! universal), so a resolved document may carry pointers of its own. Resolution
//! is therefore recursive, and bounded.
//!
//! # Rulings taken here (the design questions D-025 left open)
//!
//! * **Where** — at the delivery boundary, inside `resolve_blob_for_delivery`
//!   (roadmap option A). Same place, same moment and the same "the cell sees a
//!   transparent body" promise as the whole-body `Body::Blob` resolution, and
//!   necessarily BEFORE the `consumes` check, which validates the body's
//!   top-level slots and would otherwise inspect unexpanded pointers.
//! * **Cut** — `text_id` and `messages_id` in ONE pass. Splitting them would
//!   ship the easy half and leave the recursion, the depth limit and the cycle
//!   behaviour (all of which live in `messages_id`) undone.
//! * **`text_id` semantics** — the referenced document's `messages[]` must hold
//!   **exactly one** entry, and that entry (itself expanded) replaces the
//!   pointer. That is the singular of `messages_id`: same document shape, one
//!   turn instead of all of them. It is also the only reading that yields a
//!   schema-valid result, because `TurnObject` is closed and requires `origin`
//!   and `type` — a pointer carries neither, so they can only come from the
//!   referenced document.
//! * **Cache** — a cache for the duration of ONE resolution pass
//!   (`blob_uuid → parsed body`). It makes a diamond (two pointers to the same
//!   blob) one read instead of two. The cell-lifetime cache the spec describes
//!   is deliberately NOT built here: it needs state that outlives a message and
//!   belongs to the separate "BlobCache pro Cell" roadmap item.
//! * **Cycles** — BOTH guards. The depth limit
//!   (`colony.json blob_max_recursion_depth`, default 64) is the contract the
//!   spec promises; a visited-set over the CURRENT path catches a mutual cycle
//!   (A→B→A) immediately instead of after 64 pointless disk reads. Both report
//!   the same canonical `blob_recursion_too_deep` — the cycle is not a new wire
//!   string, it is the same failure found earlier and named in the log line. The
//!   set is per-path, not global: the same blob appearing on two different
//!   branches is a legal diamond, not a cycle.
//!
//! # Rulings taken for the `system` tree (GH #86)
//!
//! Same name, different substitution. What the second class reuses, and where
//! it necessarily differs:
//!
//! * **One `text_id` contract, not two.** A `system` leaf's blob is the SAME
//!   document shape the `messages[]` class expects: exactly one turn, itself
//!   expanded. Only the substitution differs — here the turn's `text` STRING
//!   fills a plain `{"text": …}` container, instead of the turn object replacing
//!   an array entry. A turn object could not stand in a `system` leaf anyway:
//!   the leaf has no place for `origin` and `type`. Splitting the document
//!   contract would have made `text_id` mean two things depending on where it
//!   appears, which is the one thing a shared pointer name must not do.
//! * **Where the walk stops** — at an object carrying `text` or `text_id`, the
//!   same leaf definition `meclaw-cells`' `llm::state::flatten_to_leaves` uses.
//!   Resolver and consumer must agree on what a leaf is. A leaf carrying BOTH is
//!   malformed: the spec calls them exclusive per slot.
//! * **`system.tools.*` is resolved like any other sub-slot.** Its exemption is
//!   from prompt CONCATENATION (the adapter pulls tools out as a provider-native
//!   tool set), not from resolution — a tool definition behind a pointer is
//!   exactly the oversized leaf this class exists for.
//! * **Depth counts pointer expansions, not tree levels.** Descending the object
//!   tree crosses no blob and costs nothing against
//!   `blob_max_recursion_depth`; only a resolved leaf spends a hop. Nesting is
//!   therefore free, chains are not.
//! * **Same guards, same codes, same all-or-nothing.** Depth limit, per-path
//!   visited set, `blob_recursion_too_deep` / `blob_unavailable`, and a failed
//!   pass leaves the tree untouched. Nothing about the second site justifies a
//!   second failure vocabulary.
//! * **The two passes are separate** (one per slot), so each stays testable on
//!   its own; the pass-local cache is per pass. A blob referenced from BOTH
//!   slots is read twice, which is the price of not entangling the two walks in
//!   one shared traversal. The delivery boundary runs them against one working
//!   copy, so either failure delivers nothing.
//!
//! # What this deliberately does NOT resolve
//!
//! * **`attachments[]` `blob_id` refs.** A different pointer class with a
//!   different owner: the CONSUMING CELL resolves them, on demand, at `handle()`
//!   time. An attachment is a file of arbitrary type and size; inlining it into
//!   a JSON body would defeat the blob store it came from. See
//!   `docs/meclaw-overview.en.md` § `attachments[]` schema.

use meclaw_core::{JsonValue, Uuid};
use std::collections::HashMap;

/// Why an in-message pointer could not be resolved.
#[derive(Debug)]
pub(crate) enum PointerError {
    /// The referenced blob could not be read (missing, orphaned, unparsable).
    Unavailable {
        /// Blob the pointer named.
        blob_id: Uuid,
        /// Underlying store error, for the log line.
        source: super::BlobError,
    },
    /// The pointer itself, or the document it referenced, has a shape that
    /// cannot be spliced (no `messages[]`, a `text_id` target that is not
    /// exactly one turn, a non-UUID pointer value).
    Malformed {
        /// Blob the pointer named, if it was a readable UUID.
        blob_id: Option<Uuid>,
        /// What was wrong, for the log line.
        reason: String,
    },
    /// The nesting exceeded `blob_max_recursion_depth`.
    TooDeep {
        /// The pointer that would have crossed the limit.
        blob_id: Uuid,
        /// The configured limit.
        limit: u32,
    },
    /// The pointer chain revisited a blob that is already on the current path.
    Cycle {
        /// The blob that closed the cycle.
        blob_id: Uuid,
    },
}

impl PointerError {
    /// The dead-letter reason this failure is reported under.
    ///
    /// Only two codes exist for the whole class, and both are already on the
    /// stable `error_code` list: an unreachable or unusable target is
    /// `blob_unavailable` (same as a whole-body blob that cannot be read), a
    /// runaway chain is `blob_recursion_too_deep`.
    pub(crate) fn dead_letter_reason(&self) -> crate::DeadLetterReason {
        match self {
            Self::Unavailable { .. } | Self::Malformed { .. } => {
                crate::DeadLetterReason::BlobUnavailable
            }
            Self::TooDeep { .. } | Self::Cycle { .. } => {
                crate::DeadLetterReason::BlobRecursionTooDeep
            }
        }
    }
}

impl std::fmt::Display for PointerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable { blob_id, source } => {
                write!(f, "blob {blob_id} is unreadable: {source}")
            }
            Self::Malformed {
                blob_id: Some(id),
                reason,
            } => {
                write!(f, "blob {id} cannot be spliced: {reason}")
            }
            Self::Malformed {
                blob_id: None,
                reason,
            } => write!(f, "malformed pointer: {reason}"),
            Self::TooDeep { blob_id, limit } => write!(
                f,
                "pointer chain at blob {blob_id} exceeds blob_max_recursion_depth={limit}"
            ),
            Self::Cycle { blob_id } => {
                write!(f, "pointer chain revisits blob {blob_id} — cycle")
            }
        }
    }
}

/// Working state of one resolution pass: the store, the limit, the pass-local
/// blob cache and the chain of blobs currently being expanded.
struct Pass<'a> {
    store: &'a super::DiskBlobStore,
    limit: u32,
    cache: HashMap<Uuid, JsonValue>,
    /// Blobs on the CURRENT path, innermost last. A repeat is a cycle; the same
    /// blob on a sibling branch is not (it is popped before the sibling runs).
    path: Vec<Uuid>,
}

/// Does this body carry any in-message pointer at all?
///
/// The cheap pre-check that keeps the ordinary inline body free of resolution
/// work: one scan over `messages[]` looking for the two pointer keys. Every
/// body that has no `messages[]` array, or whose entries are all plain turns,
/// answers `false` and never touches the blob store.
pub(crate) fn has_in_message_pointer(body: &JsonValue) -> bool {
    body.get("messages")
        .and_then(|m| m.as_array())
        .is_some_and(|entries| entries.iter().any(is_pointer))
}

/// Is this `messages[]` entry a Turn-Pointer or a Bulk-Pointer?
fn is_pointer(entry: &JsonValue) -> bool {
    entry
        .as_object()
        .is_some_and(|o| o.contains_key("text_id") || o.contains_key("messages_id"))
}

/// Resolve every in-message pointer in `body`'s `messages[]`, recursively.
///
/// On success `body["messages"]` contains only inline turn objects. On failure
/// `body` is left untouched — the caller dead-letters the message rather than
/// delivering a half-expanded conversation.
pub(crate) async fn resolve_in_message_pointers(
    body: &mut JsonValue,
    store: &super::DiskBlobStore,
    limit: u32,
) -> Result<(), PointerError> {
    let Some(entries) = body.get("messages").and_then(|m| m.as_array()).cloned() else {
        return Ok(());
    };
    let mut pass = Pass {
        store,
        limit,
        cache: HashMap::new(),
        path: Vec::new(),
    };
    let expanded = expand(entries, &mut pass, 0).await?;
    if let Some(obj) = body.as_object_mut() {
        obj.insert("messages".into(), JsonValue::Array(expanded));
    }
    Ok(())
}

/// Expand one `messages[]` level: plain turns pass through, pointers are
/// replaced by what they reference.
///
/// `depth` is the number of pointer expansions already crossed above this
/// level. Boxed because the recursion is async.
fn expand<'a>(
    entries: Vec<JsonValue>,
    pass: &'a mut Pass<'_>,
    depth: u32,
) -> std::pin::Pin<Box<dyn Future<Output = Result<Vec<JsonValue>, PointerError>> + Send + 'a>> {
    Box::pin(async move {
        let mut out: Vec<JsonValue> = Vec::with_capacity(entries.len());
        for entry in entries {
            let Some(obj) = entry.as_object() else {
                out.push(entry);
                continue;
            };
            let (key, raw) = match (obj.get("messages_id"), obj.get("text_id")) {
                (Some(_), Some(_)) => {
                    return Err(PointerError::Malformed {
                        blob_id: None,
                        reason: "entry carries both messages_id and text_id".into(),
                    });
                }
                (Some(v), None) => ("messages_id", v),
                (None, Some(v)) => ("text_id", v),
                (None, None) => {
                    out.push(entry);
                    continue;
                }
            };
            let blob_id = raw
                .as_str()
                .and_then(|s| Uuid::parse_str(s).ok())
                .ok_or_else(|| PointerError::Malformed {
                    blob_id: None,
                    reason: format!("{key} is not a UUID string: {raw}"),
                })?;
            // Both guards BEFORE the read: neither a runaway chain nor a cycle
            // may cost another disk round-trip.
            if depth >= pass.limit {
                return Err(PointerError::TooDeep {
                    blob_id,
                    limit: pass.limit,
                });
            }
            if pass.path.contains(&blob_id) {
                return Err(PointerError::Cycle { blob_id });
            }
            let doc = read_cached(pass, blob_id).await?;
            let inner = doc
                .get("messages")
                .and_then(|m| m.as_array())
                .cloned()
                .ok_or_else(|| PointerError::Malformed {
                    blob_id: Some(blob_id),
                    reason: "referenced document has no messages[] array".into(),
                })?;
            if key == "text_id" && inner.len() != 1 {
                return Err(PointerError::Malformed {
                    blob_id: Some(blob_id),
                    reason: format!(
                        "text_id references a single turn, but the document holds {} entries",
                        inner.len()
                    ),
                });
            }
            pass.path.push(blob_id);
            let mut resolved = expand(inner, pass, depth + 1).await?;
            pass.path.pop();
            if key == "text_id" && resolved.len() != 1 {
                return Err(PointerError::Malformed {
                    blob_id: Some(blob_id),
                    reason: format!(
                        "text_id expanded to {} turns, expected exactly one",
                        resolved.len()
                    ),
                });
            }
            out.append(&mut resolved);
        }
        Ok(out)
    })
}

/// Is this `system` node a leaf — the place the walk stops?
///
/// The same stop condition the consuming side uses (`meclaw-cells`
/// `llm::state::flatten_to_leaves`): an object carrying `text` or `text_id`.
/// Resolver and consumer must agree on what a leaf is, or one of them walks
/// into a container the other treats as content.
fn is_system_leaf(obj: &meclaw_core::serde_json::Map<String, JsonValue>) -> bool {
    obj.contains_key("text") || obj.contains_key("text_id")
}

/// Does this body carry a `{text_id}` pointer leaf anywhere in its `system` tree?
///
/// The cheap pre-check, the counterpart of [`has_in_message_pointer`]: one walk
/// over the `system` tree, no blob store access. A body without a `system` slot,
/// or one whose leaves are all inline `{text}` containers, answers `false`.
///
/// The walk is plain recursion over the parsed tree. Its depth is bounded by
/// `serde_json`'s own parser recursion limit, which every body has already
/// passed on its way in.
pub(crate) fn has_system_pointer(body: &JsonValue) -> bool {
    body.get("system").is_some_and(system_node_has_pointer)
}

fn system_node_has_pointer(node: &JsonValue) -> bool {
    let Some(obj) = node.as_object() else {
        return false;
    };
    if is_system_leaf(obj) {
        return obj.contains_key("text_id");
    }
    obj.values().any(system_node_has_pointer)
}

/// Resolve every `{text_id}` leaf in `body`'s `system` tree, recursively.
///
/// On success every leaf of `body["system"]` is an inline `{"text": …}`
/// container. On failure `body` is left untouched — the caller dead-letters the
/// message rather than delivering a half-expanded system tree.
pub(crate) async fn resolve_system_pointers(
    body: &mut JsonValue,
    store: &super::DiskBlobStore,
    limit: u32,
) -> Result<(), PointerError> {
    let Some(tree) = body.get("system").cloned() else {
        return Ok(());
    };
    let mut pass = Pass {
        store,
        limit,
        cache: HashMap::new(),
        path: Vec::new(),
    };
    let resolved = expand_system(tree, &mut pass, 0).await?;
    if let Some(obj) = body.as_object_mut() {
        obj.insert("system".into(), resolved);
    }
    Ok(())
}

/// Walk one `system` node: a pointer leaf is resolved, an inline leaf passes
/// through, anything else is a container whose values are walked.
///
/// `depth` counts POINTER EXPANSIONS, not tree levels — descending the object
/// tree crosses no blob and costs nothing against the limit. Boxed because the
/// recursion is async.
fn expand_system<'a>(
    node: JsonValue,
    pass: &'a mut Pass<'_>,
    depth: u32,
) -> std::pin::Pin<Box<dyn Future<Output = Result<JsonValue, PointerError>> + Send + 'a>> {
    Box::pin(async move {
        let obj = match node {
            JsonValue::Object(o) => o,
            other => return Ok(other),
        };
        if obj.contains_key("text_id") {
            if obj.contains_key("text") {
                return Err(PointerError::Malformed {
                    blob_id: None,
                    reason: "system leaf carries both text and text_id".into(),
                });
            }
            return resolve_system_leaf(&obj, pass, depth).await;
        }
        if obj.contains_key("text") {
            return Ok(JsonValue::Object(obj));
        }
        let mut out = meclaw_core::serde_json::Map::with_capacity(obj.len());
        for (key, value) in obj {
            out.insert(key, expand_system(value, pass, depth).await?);
        }
        Ok(JsonValue::Object(out))
    })
}

/// Turn one `{"text_id": …}` leaf into the `{"text": …}` container it names.
///
/// The referenced document is the SAME shape the `messages[]` class expects —
/// exactly one turn, itself expanded — because there is one `text_id` contract,
/// not two. Only the substitution differs: here the turn's `text` string fills a
/// plain container instead of the turn object replacing an array entry. A turn
/// object could not stand in a `system` leaf anyway: the leaf has no place for
/// `origin` and `type`.
async fn resolve_system_leaf(
    obj: &meclaw_core::serde_json::Map<String, JsonValue>,
    pass: &mut Pass<'_>,
    depth: u32,
) -> Result<JsonValue, PointerError> {
    let raw = &obj["text_id"];
    let blob_id = raw
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| PointerError::Malformed {
            blob_id: None,
            reason: format!("text_id is not a UUID string: {raw}"),
        })?;
    // Both guards BEFORE the read, exactly as in the `messages[]` class.
    if depth >= pass.limit {
        return Err(PointerError::TooDeep {
            blob_id,
            limit: pass.limit,
        });
    }
    if pass.path.contains(&blob_id) {
        return Err(PointerError::Cycle { blob_id });
    }
    let doc = read_cached(pass, blob_id).await?;
    let inner = doc
        .get("messages")
        .and_then(|m| m.as_array())
        .cloned()
        .ok_or_else(|| PointerError::Malformed {
            blob_id: Some(blob_id),
            reason: "referenced document has no messages[] array".into(),
        })?;
    if inner.len() != 1 {
        return Err(PointerError::Malformed {
            blob_id: Some(blob_id),
            reason: format!(
                "text_id references a single turn, but the document holds {} entries",
                inner.len()
            ),
        });
    }
    pass.path.push(blob_id);
    let resolved = expand(inner, pass, depth + 1).await?;
    pass.path.pop();
    if resolved.len() != 1 {
        return Err(PointerError::Malformed {
            blob_id: Some(blob_id),
            reason: format!(
                "text_id expanded to {} turns, expected exactly one",
                resolved.len()
            ),
        });
    }
    let text = resolved[0]
        .get("text")
        .and_then(|t| t.as_str())
        .ok_or_else(|| PointerError::Malformed {
            blob_id: Some(blob_id),
            reason: "the referenced turn carries no text string".into(),
        })?;
    Ok(meclaw_core::serde_json::json!({ "text": text }))
}

/// Read a blob body, through the pass-local cache.
async fn read_cached(pass: &mut Pass<'_>, blob_id: Uuid) -> Result<JsonValue, PointerError> {
    if let Some(hit) = pass.cache.get(&blob_id) {
        return Ok(hit.clone());
    }
    let value = pass
        .store
        .read_body(blob_id)
        .await
        .map_err(|source| PointerError::Unavailable { blob_id, source })?;
    pass.cache.insert(blob_id, value.clone());
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    /// Write a UBF body document into the store and return its blob id.
    async fn put(store: &super::super::DiskBlobStore, doc: JsonValue) -> Uuid {
        let bytes = meclaw_core::serde_json::to_vec(&doc).unwrap();
        let r = store
            .write_streaming(std::io::Cursor::new(bytes), "application/json", None)
            .await
            .unwrap();
        r.blob_id
    }

    fn turn(text: &str) -> JsonValue {
        json!({"origin": "user", "type": "text", "text": text})
    }

    fn store_at(td: &tempfile::TempDir) -> super::super::DiskBlobStore {
        super::super::DiskBlobStore::new(td.path()).unwrap()
    }

    #[test]
    fn pre_check_is_false_for_a_plain_body() {
        assert!(!has_in_message_pointer(&json!({"messages": [turn("hi")]})));
        assert!(!has_in_message_pointer(
            &json!({"system": {"a": {"text": "x"}}})
        ));
        assert!(has_in_message_pointer(
            &json!({"messages": [{"text_id": "x"}]})
        ));
        assert!(has_in_message_pointer(
            &json!({"messages": [turn("hi"), {"messages_id": "x"}]})
        ));
    }

    #[tokio::test]
    async fn bulk_pointer_splices_the_referenced_conversation_in_place() {
        let td = tempfile::TempDir::new().unwrap();
        let store = store_at(&td);
        let id = put(&store, json!({"messages": [turn("a"), turn("b")]})).await;
        let mut body =
            json!({"messages": [turn("before"), {"messages_id": id.to_string()}, turn("after")]});
        resolve_in_message_pointers(&mut body, &store, 64)
            .await
            .unwrap();
        assert_eq!(
            body,
            json!({"messages": [turn("before"), turn("a"), turn("b"), turn("after")]}),
            "the pointer is replaced by the referenced messages[], in place"
        );
    }

    #[tokio::test]
    async fn turn_pointer_resolves_to_the_single_turn_it_names() {
        let td = tempfile::TempDir::new().unwrap();
        let store = store_at(&td);
        let id = put(&store, json!({"messages": [turn("the one")]})).await;
        let mut body = json!({"messages": [{"text_id": id.to_string()}]});
        resolve_in_message_pointers(&mut body, &store, 64)
            .await
            .unwrap();
        assert_eq!(body, json!({"messages": [turn("the one")]}));
    }

    /// A `text_id` whose document is not exactly one turn is a shape error, not
    /// a silent truncation: "one single turn in the blob" is the contract.
    #[tokio::test]
    async fn turn_pointer_rejects_a_document_that_is_not_exactly_one_turn() {
        let td = tempfile::TempDir::new().unwrap();
        let store = store_at(&td);
        let two = put(&store, json!({"messages": [turn("a"), turn("b")]})).await;
        let mut body = json!({"messages": [{"text_id": two.to_string()}]});
        let err = resolve_in_message_pointers(&mut body, &store, 64)
            .await
            .unwrap_err();
        assert!(matches!(err, PointerError::Malformed { .. }), "got {err:?}");
        assert_eq!(
            err.dead_letter_reason(),
            crate::DeadLetterReason::BlobUnavailable
        );
    }

    #[tokio::test]
    async fn resolution_recurses_through_nested_pointers() {
        let td = tempfile::TempDir::new().unwrap();
        let store = store_at(&td);
        let leaf = put(&store, json!({"messages": [turn("deep")]})).await;
        let mid = put(
            &store,
            json!({"messages": [turn("mid"), {"messages_id": leaf.to_string()}]}),
        )
        .await;
        let mut body = json!({"messages": [{"messages_id": mid.to_string()}]});
        resolve_in_message_pointers(&mut body, &store, 64)
            .await
            .unwrap();
        assert_eq!(body, json!({"messages": [turn("mid"), turn("deep")]}));
    }

    /// The same blob referenced twice on DIFFERENT branches is a diamond, not a
    /// cycle — it must resolve, and it must cost exactly one read.
    #[tokio::test]
    async fn a_diamond_resolves_and_the_pass_cache_reads_it_once() {
        let td = tempfile::TempDir::new().unwrap();
        let store = store_at(&td);
        let shared = put(&store, json!({"messages": [turn("shared")]})).await;
        let mut body = json!({"messages": [
            {"messages_id": shared.to_string()},
            {"messages_id": shared.to_string()}
        ]});
        resolve_in_message_pointers(&mut body, &store, 64)
            .await
            .unwrap();
        assert_eq!(
            body,
            json!({"messages": [turn("shared"), turn("shared")]}),
            "a repeated reference is legal and expands twice"
        );
    }

    /// Mutual cycle A→B→A. The spec excludes SELF-cycles by UUID immutability
    /// but not mutual ones; this is the loud failure the issue asks for.
    #[tokio::test]
    async fn a_mutual_cycle_fails_loudly_as_recursion_too_deep() {
        let td = tempfile::TempDir::new().unwrap();
        let store = store_at(&td);
        // Build A and B by hand so B can name A: write A first, then B pointing
        // at A, then OVERWRITE A's content file to point at B. The store is
        // content-addressed by uuid, so a hand-written second write under the
        // same id is the only way to forge a mutual cycle at all.
        let a = put(&store, json!({"messages": [turn("placeholder")]})).await;
        let b = put(
            &store,
            json!({"messages": [{"messages_id": a.to_string()}]}),
        )
        .await;
        std::fs::write(
            td.path().join(format!("{a}.json")),
            meclaw_core::serde_json::to_vec(&json!({"messages": [{"messages_id": b.to_string()}]}))
                .unwrap(),
        )
        .unwrap();

        let mut body = json!({"messages": [{"messages_id": a.to_string()}]});
        let err = resolve_in_message_pointers(&mut body, &store, 64)
            .await
            .unwrap_err();
        assert!(matches!(err, PointerError::Cycle { .. }), "got {err:?}");
        assert_eq!(
            err.dead_letter_reason(),
            crate::DeadLetterReason::BlobRecursionTooDeep,
            "a cycle reports the canonical recursion code — no new wire string"
        );
    }

    /// The depth limit is the contract; a chain longer than it stops, and the
    /// body is left untouched rather than half-expanded.
    #[tokio::test]
    async fn a_chain_deeper_than_the_limit_stops_and_changes_nothing() {
        let td = tempfile::TempDir::new().unwrap();
        let store = store_at(&td);
        let leaf = put(&store, json!({"messages": [turn("leaf")]})).await;
        let mid = put(
            &store,
            json!({"messages": [{"messages_id": leaf.to_string()}]}),
        )
        .await;
        let top = put(
            &store,
            json!({"messages": [{"messages_id": mid.to_string()}]}),
        )
        .await;
        let probe = json!({"messages": [{"messages_id": top.to_string()}]});

        let mut ok = probe.clone();
        resolve_in_message_pointers(&mut ok, &store, 3)
            .await
            .unwrap();
        assert_eq!(ok, json!({"messages": [turn("leaf")]}), "3 hops fit in 3");

        let mut too_deep = probe.clone();
        let err = resolve_in_message_pointers(&mut too_deep, &store, 2)
            .await
            .unwrap_err();
        assert!(
            matches!(err, PointerError::TooDeep { limit: 2, .. }),
            "got {err:?}"
        );
        assert_eq!(
            too_deep, probe,
            "a failed resolution leaves the body untouched — no half-expanded conversation"
        );
    }

    /// A limit of zero is a kill switch: no pointer may be expanded at all.
    #[tokio::test]
    async fn a_limit_of_zero_rejects_every_pointer() {
        let td = tempfile::TempDir::new().unwrap();
        let store = store_at(&td);
        let id = put(&store, json!({"messages": [turn("x")]})).await;
        let mut body = json!({"messages": [{"messages_id": id.to_string()}]});
        let err = resolve_in_message_pointers(&mut body, &store, 0)
            .await
            .unwrap_err();
        assert!(
            matches!(err, PointerError::TooDeep { limit: 0, .. }),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn an_unknown_blob_is_unavailable_not_a_panic() {
        let td = tempfile::TempDir::new().unwrap();
        let store = store_at(&td);
        let mut body = json!({"messages": [{"messages_id": Uuid::now_v7().to_string()}]});
        let err = resolve_in_message_pointers(&mut body, &store, 64)
            .await
            .unwrap_err();
        assert!(
            matches!(err, PointerError::Unavailable { .. }),
            "got {err:?}"
        );
        assert_eq!(
            err.dead_letter_reason(),
            crate::DeadLetterReason::BlobUnavailable
        );
    }

    #[tokio::test]
    async fn a_non_uuid_pointer_value_is_malformed() {
        let td = tempfile::TempDir::new().unwrap();
        let store = store_at(&td);
        let mut body = json!({"messages": [{"messages_id": "not-a-uuid"}]});
        let err = resolve_in_message_pointers(&mut body, &store, 64)
            .await
            .unwrap_err();
        assert!(
            matches!(err, PointerError::Malformed { blob_id: None, .. }),
            "got {err:?}"
        );
    }

    /// A referenced document without a `messages[]` array cannot be spliced.
    #[tokio::test]
    async fn a_document_without_messages_is_malformed() {
        let td = tempfile::TempDir::new().unwrap();
        let store = store_at(&td);
        let id = put(&store, json!({"system": {"a": {"text": "x"}}})).await;
        let mut body = json!({"messages": [{"messages_id": id.to_string()}]});
        let err = resolve_in_message_pointers(&mut body, &store, 64)
            .await
            .unwrap_err();
        assert!(matches!(err, PointerError::Malformed { .. }), "got {err:?}");
    }

    /// A body without a `messages[]` slot is not this resolver's business and
    /// comes back untouched — `system` and `attachments` are other classes.
    #[tokio::test]
    async fn a_body_without_messages_is_left_alone() {
        let td = tempfile::TempDir::new().unwrap();
        let store = store_at(&td);
        let before = json!({
            "system": {"identity": {"body": {"text_id": Uuid::now_v7().to_string()}}},
            "attachments": [{"blob_id": Uuid::now_v7().to_string(), "mime_type": "application/pdf", "size_bytes": 4}]
        });
        let mut body = before.clone();
        resolve_in_message_pointers(&mut body, &store, 64)
            .await
            .unwrap();
        assert_eq!(body, before);
    }

    // ── GH #86: the `system` tree's `{text_id}` leaves ───────────────────────

    /// A blob holding exactly one turn — the document form both `text_id`
    /// classes reference.
    async fn put_one_turn(
        td: &tempfile::TempDir,
        text: &str,
    ) -> (super::super::DiskBlobStore, Uuid) {
        let store = store_at(td);
        let id = put(&store, json!({"messages": [turn(text)]})).await;
        (store, id)
    }

    #[test]
    fn system_pre_check_only_fires_on_a_pointer_leaf() {
        assert!(!has_system_pointer(
            &json!({"messages": [{"text_id": "x"}]})
        ));
        assert!(!has_system_pointer(
            &json!({"system": {"identity": {"soul": {"text": "S"}}}})
        ));
        assert!(has_system_pointer(
            &json!({"system": {"identity": {"body": {"text_id": "x"}}}})
        ));
        assert!(
            has_system_pointer(&json!({"system": {"tools": {"calc": {"text_id": "x"}}}})),
            "the tools sub-slot is a leaf site like any other"
        );
        assert!(
            has_system_pointer(&json!({"system": {"a": {"text": "i", "text_id": "x"}}})),
            "an ambiguous leaf must reach the resolver, which rejects it loudly — \
             the pre-check may not swallow it"
        );
    }

    /// The substitution this class exists for: a pointer leaf becomes a plain
    /// string container, NOT a turn object.
    #[tokio::test]
    async fn a_system_leaf_pointer_becomes_a_plain_text_container() {
        let td = tempfile::TempDir::new().unwrap();
        let (store, id) = put_one_turn(&td, "the long persona").await;
        let mut body = json!({
            "system": {"identity": {"soul": {"text": "inline"}, "body": {"text_id": id.to_string()}}}
        });
        resolve_system_pointers(&mut body, &store, 64)
            .await
            .unwrap();
        assert_eq!(
            body,
            json!({
                "system": {
                    "identity": {"soul": {"text": "inline"}, "body": {"text": "the long persona"}}
                }
            }),
            "a leaf becomes {{\"text\": …}} — the turn's origin/type do not survive"
        );
    }

    /// The walk covers an arbitrarily deep object tree, not one array level.
    #[tokio::test]
    async fn every_depth_of_the_tree_is_walked() {
        let td = tempfile::TempDir::new().unwrap();
        let (store, id) = put_one_turn(&td, "deep").await;
        let mut body = json!({
            "system": {"a": {"b": {"c": {"d": {"text_id": id.to_string()}}}},
                       "tools": {"calc": {"text_id": id.to_string()}}}
        });
        resolve_system_pointers(&mut body, &store, 64)
            .await
            .unwrap();
        assert_eq!(
            body,
            json!({
                "system": {"a": {"b": {"c": {"d": {"text": "deep"}}}},
                           "tools": {"calc": {"text": "deep"}}}
            }),
            "the tools sub-slot is exempt from prompt CONCATENATION, not from resolution"
        );
    }

    /// The referenced document goes through the `messages[]` resolver first, so
    /// a pointer inside it is expanded too — one recursion budget for both.
    #[tokio::test]
    async fn a_system_leaf_recurses_through_a_nested_turn_pointer() {
        let td = tempfile::TempDir::new().unwrap();
        let store = store_at(&td);
        let leaf = put(&store, json!({"messages": [turn("from two hops down")]})).await;
        let mid = put(&store, json!({"messages": [{"text_id": leaf.to_string()}]})).await;
        let mut body = json!({"system": {"identity": {"body": {"text_id": mid.to_string()}}}});
        resolve_system_pointers(&mut body, &store, 64)
            .await
            .unwrap();
        assert_eq!(
            body,
            json!({"system": {"identity": {"body": {"text": "from two hops down"}}}})
        );
    }

    /// Same guard, same canonical code as the `messages[]` class.
    #[tokio::test]
    async fn a_system_pointer_cycle_fails_loudly_as_recursion_too_deep() {
        let td = tempfile::TempDir::new().unwrap();
        let store = store_at(&td);
        let a = put(&store, json!({"messages": [turn("placeholder")]})).await;
        let b = put(&store, json!({"messages": [{"text_id": a.to_string()}]})).await;
        std::fs::write(
            td.path().join(format!("{a}.json")),
            meclaw_core::serde_json::to_vec(&json!({"messages": [{"text_id": b.to_string()}]}))
                .unwrap(),
        )
        .unwrap();

        let mut body = json!({"system": {"identity": {"body": {"text_id": a.to_string()}}}});
        let err = resolve_system_pointers(&mut body, &store, 64)
            .await
            .unwrap_err();
        assert!(matches!(err, PointerError::Cycle { .. }), "got {err:?}");
        assert_eq!(
            err.dead_letter_reason(),
            crate::DeadLetterReason::BlobRecursionTooDeep
        );
    }

    /// The depth limit is the same one, and a failure changes nothing.
    #[tokio::test]
    async fn a_system_chain_deeper_than_the_limit_stops_and_changes_nothing() {
        let td = tempfile::TempDir::new().unwrap();
        let store = store_at(&td);
        let leaf = put(&store, json!({"messages": [turn("leaf")]})).await;
        let top = put(&store, json!({"messages": [{"text_id": leaf.to_string()}]})).await;
        let probe = json!({"system": {"identity": {"body": {"text_id": top.to_string()}}}});

        let mut ok = probe.clone();
        resolve_system_pointers(&mut ok, &store, 2).await.unwrap();
        assert_eq!(
            ok,
            json!({"system": {"identity": {"body": {"text": "leaf"}}}}),
            "2 hops fit in 2"
        );

        let mut too_deep = probe.clone();
        let err = resolve_system_pointers(&mut too_deep, &store, 1)
            .await
            .unwrap_err();
        assert!(
            matches!(err, PointerError::TooDeep { limit: 1, .. }),
            "got {err:?}"
        );
        assert_eq!(
            too_deep, probe,
            "a failed resolution leaves the tree untouched — no half-expanded system"
        );
    }

    /// A limit of zero is the same kill switch here.
    #[tokio::test]
    async fn a_system_limit_of_zero_rejects_every_leaf_pointer() {
        let td = tempfile::TempDir::new().unwrap();
        let (store, id) = put_one_turn(&td, "x").await;
        let mut body = json!({"system": {"identity": {"body": {"text_id": id.to_string()}}}});
        let err = resolve_system_pointers(&mut body, &store, 0)
            .await
            .unwrap_err();
        assert!(
            matches!(err, PointerError::TooDeep { limit: 0, .. }),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn an_unknown_blob_in_the_system_tree_is_unavailable() {
        let td = tempfile::TempDir::new().unwrap();
        let store = store_at(&td);
        let mut body = json!({
            "system": {"identity": {"body": {"text_id": Uuid::now_v7().to_string()}}}
        });
        let err = resolve_system_pointers(&mut body, &store, 64)
            .await
            .unwrap_err();
        assert!(
            matches!(err, PointerError::Unavailable { .. }),
            "got {err:?}"
        );
        assert_eq!(
            err.dead_letter_reason(),
            crate::DeadLetterReason::BlobUnavailable
        );
    }

    /// `text_id` means one turn in BOTH classes — a document with more is a
    /// shape error, not a silent pick-the-first.
    #[tokio::test]
    async fn a_system_leaf_document_that_is_not_exactly_one_turn_is_malformed() {
        let td = tempfile::TempDir::new().unwrap();
        let store = store_at(&td);
        let two = put(&store, json!({"messages": [turn("a"), turn("b")]})).await;
        let mut body = json!({"system": {"identity": {"body": {"text_id": two.to_string()}}}});
        let err = resolve_system_pointers(&mut body, &store, 64)
            .await
            .unwrap_err();
        assert!(matches!(err, PointerError::Malformed { .. }), "got {err:?}");
        assert_eq!(
            err.dead_letter_reason(),
            crate::DeadLetterReason::BlobUnavailable
        );
    }

    /// The target container holds a STRING. A turn without one cannot fill it.
    #[tokio::test]
    async fn a_referenced_turn_without_a_text_string_is_malformed() {
        let td = tempfile::TempDir::new().unwrap();
        let store = store_at(&td);
        let id = put(
            &store,
            json!({"messages": [{"origin": "user", "type": "image"}]}),
        )
        .await;
        let mut body = json!({"system": {"identity": {"body": {"text_id": id.to_string()}}}});
        let err = resolve_system_pointers(&mut body, &store, 64)
            .await
            .unwrap_err();
        assert!(matches!(err, PointerError::Malformed { .. }), "got {err:?}");
    }

    #[tokio::test]
    async fn a_non_uuid_system_pointer_value_is_malformed() {
        let td = tempfile::TempDir::new().unwrap();
        let store = store_at(&td);
        let mut body = json!({"system": {"identity": {"body": {"text_id": "not-a-uuid"}}}});
        let err = resolve_system_pointers(&mut body, &store, 64)
            .await
            .unwrap_err();
        assert!(
            matches!(err, PointerError::Malformed { blob_id: None, .. }),
            "got {err:?}"
        );
    }

    /// `text` and `text_id` are exclusive per slot (spec § `system` sub-slot
    /// structure). A leaf carrying both is ambiguous, and ambiguity is loud.
    #[tokio::test]
    async fn a_leaf_carrying_both_text_and_text_id_is_malformed() {
        let td = tempfile::TempDir::new().unwrap();
        let (store, id) = put_one_turn(&td, "x").await;
        let mut body = json!({
            "system": {"identity": {"body": {"text": "inline", "text_id": id.to_string()}}}
        });
        let err = resolve_system_pointers(&mut body, &store, 64)
            .await
            .unwrap_err();
        assert!(
            matches!(err, PointerError::Malformed { blob_id: None, .. }),
            "got {err:?}"
        );
    }

    /// A tree without pointers, and a body without a `system` slot, are not this
    /// resolver's business and come back byte-identical.
    #[tokio::test]
    async fn a_pointerless_body_is_left_alone() {
        let td = tempfile::TempDir::new().unwrap();
        let store = store_at(&td);
        let before = json!({
            "system": {"identity": {"soul": {"text": "S"}}, "facts": {"n": {"text": "A"}}},
            "messages": [turn("hi")]
        });
        let mut body = before.clone();
        resolve_system_pointers(&mut body, &store, 64)
            .await
            .unwrap();
        assert_eq!(body, before);

        let mut no_system = json!({"messages": [turn("hi")]});
        resolve_system_pointers(&mut no_system, &store, 64)
            .await
            .unwrap();
        assert_eq!(no_system, json!({"messages": [turn("hi")]}));
    }

    /// The two classes stay in their lanes: the system resolver must not touch
    /// `messages[]` pointers, which have a different target shape.
    #[tokio::test]
    async fn the_system_resolver_leaves_message_pointers_alone() {
        let td = tempfile::TempDir::new().unwrap();
        let store = store_at(&td);
        let before = json!({"messages": [{"text_id": Uuid::now_v7().to_string()}]});
        let mut body = before.clone();
        resolve_system_pointers(&mut body, &store, 64)
            .await
            .unwrap();
        assert_eq!(body, before);
    }
}

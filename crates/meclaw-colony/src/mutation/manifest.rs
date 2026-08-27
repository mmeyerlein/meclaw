//! GH #422 — the manifest: a second, additive body form for
//! `/colony/mutations`.
//!
//! A manifest is an ORDERED LIST of ordinary mutation bodies in ONE body. The
//! colony rolls it off itself: entry by entry, in order, through the very
//! `handle_mutation` a single body takes, stopping at the first refusal and
//! answering with ONE receipt — "k applied, entry k+1 refused with
//! `error_code`, the rest untouched". There is deliberately NO ROLLBACK: what
//! committed stays committed, and the receipt says exactly where to resume.
//!
//! ```json
//! { "manifest": [
//!     { "scope": "/",   "diff": { "add_nodes": [ … ] } },
//!     { "scope": "/os", "diff": { "add_edges": [ … ] } }
//! ] }
//! ```
//!
//! **Every entry is byte-for-byte one single-form body.** No `kind`, no `id`,
//! no manifest-wide `ctx` — each entry brings its own, because two places for
//! one substitution is one place too many. Manifest v1 is MUTATIONS ONLY: a
//! message entry has no meaning for a receipt whose word is "applied", and
//! `/colony/mutations` is the mutation door, not a general traffic inlet. A
//! later `kind` discriminator whose absence means `"mutation"` would be purely
//! additive.
//!
//! **The single form does not move.** Detection is one question — does the body
//! carry a top-level `manifest` key — and a `None` answer means "this is the
//! single form, do not touch it". Pinned by
//! `tests/gh422_the_single_mutation_body_does_not_move.rs`.

use super::MutationOutcome;
use meclaw_core::JsonValue;

/// A well-formed manifest body: the entries, in the order they were written.
#[derive(Debug, Clone)]
pub struct ManifestBody {
    entries: Vec<JsonValue>,
}

impl ManifestBody {
    /// Is this body a manifest, and if so, is it well formed?
    ///
    /// The double wrapper is deliberate and load-bearing:
    ///
    /// * `None` — the body carries no top-level `manifest` key. It is the
    ///   SINGLE form and takes byte-for-byte the path it has always taken. No
    ///   other key discriminates, not even an unknown one.
    /// * `Some(Err(_))` — the body meant to be a manifest and is broken. It is
    ///   refused as a manifest (`error_code: "schema"`), never silently retried
    ///   as a single mutation.
    /// * `Some(Ok(_))` — a manifest to roll off.
    pub fn detect(body: &JsonValue) -> Option<Result<Self, ManifestError>> {
        let raw = body.get("manifest")?;
        Some(Self::parse(body, raw))
    }

    fn parse(body: &JsonValue, raw: &JsonValue) -> Result<Self, ManifestError> {
        // A body cannot be both forms: `manifest` beside `diff`/`scope` is an
        // author who wrote two intentions into one document, and guessing which
        // one wins is how a mutation lands somewhere nobody asked for.
        if body.get("diff").is_some() || body.get("scope").is_some() {
            return Err(ManifestError::BothForms);
        }
        let Some(items) = raw.as_array() else {
            return Err(ManifestError::NotAnArray);
        };
        if items.is_empty() {
            return Err(ManifestError::Empty);
        }
        for (i, entry) in items.iter().enumerate() {
            if !entry.is_object() {
                // 1-based: an operator counts entries, not indices.
                return Err(ManifestError::EntryNotAnObject { position: i + 1 });
            }
        }
        Ok(Self {
            entries: items.clone(),
        })
    }

    /// The entries, in manifest order. Each is one single-form mutation body.
    pub fn entries(&self) -> &[JsonValue] {
        &self.entries
    }
}

/// Why a body that meant to be a manifest is not one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    /// `manifest` is present but not an array.
    NotAnArray,
    /// `manifest` is an empty array.
    Empty,
    /// The 1-based entry at this position is not an object.
    EntryNotAnObject {
        /// 1-based position of the offending entry.
        position: usize,
    },
    /// The body carries `manifest` AND a single-form key (`diff` / `scope`).
    BothForms,
}

impl ManifestError {
    /// The `error_code` a refused manifest body carries.
    ///
    /// ALWAYS `"schema"`, and never a new string: `error_code` is a documented
    /// public contract surface (README § Stability), and a broken body form is
    /// precisely what `schema` already means. A manifest that cannot be read is
    /// not a new class of failure, it is the oldest one.
    pub fn error_code(&self) -> &'static str {
        "schema"
    }
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAnArray => write!(f, "`manifest` must be an array of mutation bodies"),
            Self::Empty => write!(
                f,
                "an empty manifest applies nothing; omit it or send a single mutation"
            ),
            Self::EntryNotAnObject { position } => {
                write!(f, "manifest entry {position} is not an object")
            }
            Self::BothForms => write!(
                f,
                "a body is either a single mutation or a manifest, not both"
            ),
        }
    }
}

impl std::error::Error for ManifestError {}

/// The verdict of one manifest roll-off.
///
/// There is no rollback: `applied` mutations are committed and stay committed,
/// whatever position `failed_at` names. That is the point of the form — the
/// receipt says where it stopped so the rest can be sent again.
#[derive(Debug, Clone)]
pub enum ManifestOutcome {
    /// Every entry committed. `ids` holds one mutation id per entry, in order.
    Committed {
        /// One mutation id per entry, in manifest order.
        ids: Vec<String>,
    },
    /// Entry `failed_at` (1-based) was refused.
    Rejected {
        /// The mutation ids of the entries BEFORE the refused one, in order.
        ids: Vec<String>,
        /// 1-based position of the refused entry.
        failed_at: usize,
        /// The refused entry's own mutation id, if it got one.
        id: Option<String>,
        /// The refusing entry's `error_code`, verbatim.
        error_code: String,
        /// The refusing entry's `details`, verbatim.
        details: String,
        /// How many entries were never looked at.
        remaining: usize,
    },
}

/// GH #422 — the verdict of ONE knock at `/colony/mutations`, whichever body
/// form knocked.
///
/// The door takes two forms and therefore answers with two kinds of verdict.
/// This is the type the door hands back — to the HTTP handler, to `--apply`,
/// and to the EDA reply builder — so that the discrimination happens exactly
/// once, inside the colony, and every caller reads the same answer.
///
/// Why a new inbox variant rather than lifting `ColonyMsg::Mutation`'s ack: the
/// single form must stay provably unmoved (R5, pinned by
/// `tests/gh422_the_single_mutation_body_does_not_move.rs`), and ~140 call
/// sites take that variant with a `MutationOutcome` ack. Widening it would
/// have touched every one of them to prove nothing. `ColonyMsg::Mutation` stays
/// exactly what it was — "I know this is one mutation" — and
/// `ColonyMsg::MutationDoor` is the door that does not know yet.
#[derive(Debug, Clone)]
pub enum MutationDoorOutcome {
    /// The body was a single mutation and took the path it always took.
    Single(MutationOutcome),
    /// The body was a manifest and was rolled off.
    Manifest(ManifestOutcome),
    /// The body meant to be a manifest and could not be read at all.
    MalformedManifest(ManifestError),
}

impl MutationDoorOutcome {
    /// Did everything this body asked for commit?
    ///
    /// The one question the HTTP mapping asks: `true` → 200, `false` → 422 —
    /// the same two codes the single form has always used.
    pub fn is_committed(&self) -> bool {
        match self {
            Self::Single(MutationOutcome::Committed { .. })
            | Self::Manifest(ManifestOutcome::Committed { .. }) => true,
            Self::Single(MutationOutcome::Rejected { .. })
            | Self::Manifest(ManifestOutcome::Rejected { .. })
            | Self::MalformedManifest(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    #[test]
    fn a_body_without_the_manifest_key_is_not_a_manifest() {
        let v = json!({ "scope": "/", "diff": {} });
        assert!(ManifestBody::detect(&v).is_none());
    }

    #[test]
    fn an_unknown_top_level_key_does_not_discriminate() {
        let v = json!({ "scope": "/", "diff": {}, "comment": "x" });
        assert!(ManifestBody::detect(&v).is_none());
    }

    #[test]
    fn a_manifest_body_yields_its_entries_in_order() {
        let v = json!({ "manifest": [ { "scope": "/a" }, { "scope": "/b" } ] });
        let m = ManifestBody::detect(&v)
            .expect("manifest")
            .expect("well-formed");
        assert_eq!(m.entries().len(), 2);
        assert_eq!(m.entries()[0]["scope"], "/a");
        assert_eq!(m.entries()[1]["scope"], "/b");
    }

    #[test]
    fn a_manifest_that_is_not_an_array_is_refused_by_name() {
        let v = json!({ "manifest": { "scope": "/a" } });
        let e = ManifestBody::detect(&v).expect("manifest").unwrap_err();
        assert_eq!(e, ManifestError::NotAnArray);
        assert_eq!(
            e.to_string(),
            "`manifest` must be an array of mutation bodies"
        );
    }

    #[test]
    fn an_empty_manifest_is_refused_by_name() {
        let v = json!({ "manifest": [] });
        let e = ManifestBody::detect(&v).expect("manifest").unwrap_err();
        assert_eq!(e, ManifestError::Empty);
        assert_eq!(
            e.to_string(),
            "an empty manifest applies nothing; omit it or send a single mutation"
        );
    }

    #[test]
    fn an_entry_that_is_not_an_object_is_named_by_its_one_based_position() {
        let v = json!({ "manifest": [ { "scope": "/a" }, 7 ] });
        let e = ManifestBody::detect(&v).expect("manifest").unwrap_err();
        assert_eq!(e, ManifestError::EntryNotAnObject { position: 2 });
        assert_eq!(e.to_string(), "manifest entry 2 is not an object");
    }

    #[test]
    fn a_body_that_is_both_forms_is_refused_by_name() {
        let v = json!({ "manifest": [ { "scope": "/a" } ], "diff": {} });
        let e = ManifestBody::detect(&v).expect("manifest").unwrap_err();
        assert_eq!(e, ManifestError::BothForms);
        assert_eq!(
            e.to_string(),
            "a body is either a single mutation or a manifest, not both"
        );
    }

    #[test]
    fn every_manifest_refusal_carries_the_schema_error_code() {
        for e in [
            ManifestError::NotAnArray,
            ManifestError::Empty,
            ManifestError::EntryNotAnObject { position: 1 },
            ManifestError::BothForms,
        ] {
            assert_eq!(e.error_code(), "schema", "{e}");
        }
    }
}

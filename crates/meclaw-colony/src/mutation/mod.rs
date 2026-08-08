//! Mutation-Apply-Pipeline (Phase 6).
//!
//! Substitute → Validate → durable in_flight → stage + rename → spawn → edges → durable committed.
//! Detail: see `plans/archive/phase-6-mutations.md` Entscheidung 8.

pub mod apply;
pub mod header_views;
pub mod hook;
pub mod recovery;
pub mod rename;
pub mod stage;
pub mod substitute;
pub mod subtree;
pub(crate) mod swap;
pub mod validate;

use meclaw_core::Path;

/// Build an EDA error-reply message (Phase 6 T13).
///
/// Sent via `route_with_log` to `reply_to` from the Mutation arm when
/// substitute or validate fail. The body follows the universal UBF schema
/// with `system.header.finish_reason = "error"` and the spec-stable
/// `error_code` token.
pub fn build_error_reply(
    mutation_id: &str,
    error_code: &str,
    details: &str,
    target: Path,
    trace_id: meclaw_core::Uuid,
    parent_message_id: meclaw_core::Uuid,
) -> meclaw_core::Message {
    use meclaw_core::{Body, MessageBuilder};
    let body_json = meclaw_core::serde_json::json!({
        "system": {
            "header": {
                "finish_reason": "error",
                "error_code": error_code,
                "mutation_id": mutation_id,
            }
        },
        "messages": [{
            "role": "system",
            "content": [{"type": "text", "text": details}]
        }]
    });
    MessageBuilder::new(target)
        .trace_id(trace_id)
        .parent_message_id(parent_message_id)
        .body(Body::Inline(body_json))
        .build()
}

/// Phase-6: combine a scope-prefix (absolute path) with a relative name into
/// an absolute Path. Uses the substrate's canonical [`meclaw_core::Path::resolve`]
/// resolution — the SAME normalisation the filesystem boot applies to hive-graph
/// edges — so a mutation endpoint resolves identically whether it boots or is
/// added at runtime.
///
/// Spec § Mutation-Format: scope is an absolute path prefix; names in the diff
/// are relative to it. Used by T15 (stage), T19 (remove), T20 (swap), T21 (edges).
///
/// Befund 6: `Path::resolve` normalises `./name` → `<scope>/name` (and collapses
/// `//`); the former raw string-join produced `<scope>/./name`, a path the
/// routing layer would never match — a `./`-prefixed `add_edges` endpoint
/// (the canonical mutation form per overview § Variable substitution) committed
/// a dead edge. Validate mirrors this by stripping `./` before its short-name
/// membership test.
pub fn resolve_scoped_path(scope: &str, name: &str) -> meclaw_core::Path {
    meclaw_core::Path::resolve(&meclaw_core::Path::new(scope), name)
}

/// Errors that abort a mutation. Each variant maps to a spec `error_code` token
/// (see `docs/meclaw-overview.md` § Mutation-Format → Validierung).
#[derive(Debug, Clone, PartialEq)]
pub enum MutationError {
    Schema(String),
    MatchNoHit(String),
    NamingCollision(String),
    Cycle(String),
    EdgeSchema(String),
    TemplateMissing(String),
    EnvVarMissing(String),
    /// A `${...}` substitution token uses an operator form that meclaw does not
    /// support (e.g. `${VAR:=x}`, `${VAR-x}`, `${VAR:+x}`, `${VAR:?msg}`). Only
    /// the plain `${VAR}` and POSIX default `${VAR:-fallback}` forms are valid
    /// for env tokens (spec § Variable substitution → `${ENV_VAR}` from `.env`).
    /// Carries the offending token inner string. Never silently passed through.
    UnsupportedSubstitution(String),
    CtxKeyMissing(String),
    ScopeOutOfBounds {
        path: Path,
    },
    /// Unknown cell type encountered during mutation validation.
    UnknownCellType(String),
    /// `add_nodes` targets an existing path whose cell is currently `Awake`
    /// (running task). Resume/Reconnect cannot race-free hand over the live
    /// `cell.db` of a running cell (Phase-13.5 Lifecycle-3a, Auflage A2).
    /// Carries the target path string.
    ResumeRequiresStoppedCell(String),
    /// Resume (single-cell OR subtree per-node) targets an existing path whose
    /// on-disk `cell.type` does NOT match the template being resumed. Resume
    /// preserves identity and `cell.db`; a type change at the same path would be
    /// a silent reinterpretation of persisted state, so it is a loud,
    /// pre-destructive reject (F2-Ruling, Paket 5). Carries the target path
    /// string.
    ResumeTypeMismatch(String),
    /// Hardening Slice 4 (Task 4.2): a staged NON-hive `config.json` does not
    /// declare the builder-mandatory contract presence keys `version` /
    /// `settings` / `consumes`, or a key has the wrong JSON type (config.md
    /// § contract, Enforcement-Stufen). Raised pre-destructively during
    /// staging (the `.staging` dir is discarded, live tree untouched). Carries
    /// the offending config's staging path plus the presence-check reason.
    ContractIncomplete(String),
    /// Deep-Audit F2: an atomic rename sequence failed AFTER its first committed
    /// `rename(2)` — earlier renames already stand in the live tree (audit-model,
    /// no rollback). This is NOT a clean pre-destructive reject. The call-site
    /// (`colony::handle_mutation`) strict-fails (panic) on this variant BEFORE any
    /// EDA reply, so it never reaches a `reply_to`; `error_code()` carries a
    /// defensive, never-reached fallback to keep the spec error_code enum
    /// (overview Z.293) unchanged. Carries the failing rename's diagnostic string.
    LiveTreeMutated(String),
}

impl MutationError {
    /// Spec-stable token used in EDA error replies (see `error_code`-Enum in spec).
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::Schema(_) => "schema",
            Self::MatchNoHit(_) => "match_no_hit",
            Self::NamingCollision(_) => "naming_collision",
            Self::Cycle(_) => "cycle",
            Self::EdgeSchema(_) => "edge_schema",
            Self::TemplateMissing(_) => "template_missing",
            Self::EnvVarMissing(_) => "env_var_missing",
            Self::UnsupportedSubstitution(_) => "unsupported_substitution",
            Self::CtxKeyMissing(_) => "ctx_key_missing",
            Self::ScopeOutOfBounds { .. } => "scope_out_of_bounds",
            Self::UnknownCellType(_) => "unknown_cell_type",
            Self::ResumeRequiresStoppedCell(_) => "resume_requires_stopped_cell",
            Self::ResumeTypeMismatch(_) => "resume_type_mismatch",
            Self::ContractIncomplete(_) => "contract_incomplete",
            // Never reached: the call-site panics (strict-fail) before any EDA
            // reply. Defensive fallback to "schema" so the spec error_code enum
            // (overview Z.293) stays unchanged — LiveTreeMutated is a strict-fail
            // signal, not an over-the-wire reject code.
            Self::LiveTreeMutated(_) => "schema",
        }
    }
}

/// Returns true iff an existing node at a resume target is type-compatible with
/// the template being resumed: exact `cell.type` equality.
///
/// This single equality also subsumes the hive-vs-cell class distinction: a
/// hive's `type` is the literal string `"hive"`, while a cell's `type` is its
/// cell-type token. So a hive resumed by a cell template (or vice versa) is
/// already caught here — the strings differ. Used by both the single-cell
/// resume path (`colony::handle_mutation` Step 1a) and the subtree per-node
/// resume path (T12); there is exactly ONE compatibility rule, expressed here.
pub fn resume_type_compatible(existing_type: &str, template_type: &str) -> bool {
    existing_type == template_type
}

/// Result of `handle_mutation`: returned to the caller via the `ColonyMsg::Mutation.ack` oneshot.
#[derive(Debug, Clone)]
pub enum MutationOutcome {
    Committed {
        id: String,
    },
    Rejected {
        id: Option<String>,
        error_code: String,
        details: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_maps_each_variant_to_spec_token() {
        assert_eq!(MutationError::Schema("x".into()).error_code(), "schema");
        assert_eq!(
            MutationError::TemplateMissing("x".into()).error_code(),
            "template_missing"
        );
        assert_eq!(
            MutationError::EnvVarMissing("x".into()).error_code(),
            "env_var_missing"
        );
        assert_eq!(MutationError::Cycle("x".into()).error_code(), "cycle");
        assert_eq!(
            MutationError::MatchNoHit("x".into()).error_code(),
            "match_no_hit"
        );
        assert_eq!(
            MutationError::NamingCollision("x".into()).error_code(),
            "naming_collision"
        );
        assert_eq!(
            MutationError::EdgeSchema("x".into()).error_code(),
            "edge_schema"
        );
        assert_eq!(
            MutationError::CtxKeyMissing("x".into()).error_code(),
            "ctx_key_missing"
        );
        assert_eq!(
            MutationError::ContractIncomplete("x".into()).error_code(),
            "contract_incomplete"
        );
    }

    #[test]
    fn unsupported_substitution_maps_to_error_code() {
        let err = MutationError::UnsupportedSubstitution("VAR:=x".into());
        assert_eq!(err.error_code(), "unsupported_substitution");
    }

    #[test]
    fn unknown_cell_type_maps_to_error_code() {
        let err = MutationError::UnknownCellType("foo".into());
        assert_eq!(err.error_code(), "unknown_cell_type");
    }

    #[test]
    fn resume_requires_stopped_cell_maps_to_error_code() {
        let err = MutationError::ResumeRequiresStoppedCell("/a".into());
        assert_eq!(err.error_code(), "resume_requires_stopped_cell");
    }

    #[test]
    fn resume_type_mismatch_maps_to_error_code() {
        let err = MutationError::ResumeTypeMismatch("/a".into());
        assert_eq!(err.error_code(), "resume_type_mismatch");
    }

    #[test]
    fn resume_type_compatible_equal_types_true() {
        assert!(resume_type_compatible("echo", "echo"));
    }

    #[test]
    fn resume_type_compatible_different_cell_types_false() {
        assert!(!resume_type_compatible("echo", "persist_mock"));
    }

    #[test]
    fn resume_type_compatible_hive_vs_cell_false() {
        // A hive's type is the literal "hive"; a cell's is its cell-type.
        assert!(!resume_type_compatible("hive", "echo"));
        assert!(!resume_type_compatible("echo", "hive"));
    }

    #[test]
    fn resolve_scoped_path_joins_scope_and_name() {
        assert_eq!(
            resolve_scoped_path("/main", "worker").as_str(),
            "/main/worker"
        );
    }

    #[test]
    fn resolve_scoped_path_handles_root_scope() {
        assert_eq!(resolve_scoped_path("/", "x").as_str(), "/x");
    }

    #[test]
    fn resolve_scoped_path_strips_trailing_slash() {
        assert_eq!(resolve_scoped_path("/main/", "x").as_str(), "/main/x");
    }
}

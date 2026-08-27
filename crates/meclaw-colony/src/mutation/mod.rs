//! Mutation-Apply-Pipeline (Phase 6).
//!
//! Substitute → Validate → durable in_flight → stage + rename → spawn → edges → durable committed.
//! Detail: see `plans/archive/phase-6-mutations.md` Entscheidung 8.

pub mod apply;
pub mod header_views;
pub mod hive_contract;
pub mod hook;
pub mod manifest;
pub mod port_boundary;
pub mod recovery;
pub mod rejection;
pub(crate) mod relocate;
pub mod rename;
pub mod required_drains;
pub mod stage;
pub mod substitute;
pub mod subtree;
pub(crate) mod swap;
pub mod validate;

pub use manifest::{ManifestBody, ManifestError, ManifestOutcome, MutationDoorOutcome};

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

/// GH #163 — the absolute endpoints a mutation may address from inside a
/// subtree: the colony's own read-only topology and ledger endpoints.
///
/// Containment (`validate::validate_scope_containment`,
/// `subtree::resolve_internal_edges`) exists so that a mutation cannot wire into
/// a cell it has no authority over. `/colony/graph` is not a cell — it is a
/// virtual read endpoint of the authority itself, dispatched before `apply_edges`
/// ever runs, and answering it hands out topology, which is not secret: it is the
/// *sanctioned* way to learn topology, because § Database isolation forbids
/// reading `colony.db`. Denying the lane to every mutation did not protect
/// anything; it only meant a cell that needs the graph had to be born with the
/// lane, or somebody would go read the database instead.
///
/// Deliberately an enumerated list and not a `/colony/*` prefix: `/colony/mutations`
/// is authority *transfer*, `/colony/trace` and `/colony/dead_letters` hand out
/// other cells' message content. Widening this list is a decision with its own
/// argument, not a convenience.
///
/// GH #267 — `/colony/ledger` carries that argument and is therefore the second
/// entry: it answers **counts**, never rows and never header contents. That puts
/// it in the same class as the topology endpoint — an aggregate view of the
/// colony's own bookkeeping that reveals no cell's message content — and
/// explicitly not in the class of `/colony/trace`, which hands out exactly that
/// content. A cell that needs to know how much moved may ask; it still may not
/// learn what moved.
pub const MUTATION_DRAWABLE_VIRTUAL_ENDPOINTS: &[&str] = &["/colony/graph", "/colony/ledger"];

/// Whether `endpoint` is an absolute virtual endpoint a mutation may draw an
/// edge **to** (never `from` — a virtual endpoint emits nothing on its own).
///
/// See [`MUTATION_DRAWABLE_VIRTUAL_ENDPOINTS`].
pub fn is_mutation_drawable_virtual_target(endpoint: &str) -> bool {
    MUTATION_DRAWABLE_VIRTUAL_ENDPOINTS.contains(&endpoint)
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
    /// GH #277: a template `ref` chain closes a ring — a template that is
    /// already on the resolution stack is entered a second time. Because the
    /// stack itself is the guard, composition needs no depth cap: a ring is
    /// caught at its first repetition, and a finite registry cannot produce an
    /// unbounded ring-free chain. Pre-destructive (raised while parsing, before
    /// any staging). Carries the ring rendered as `a@1.0.0 -> b@1.0.0 -> a@1.0.0`.
    TemplateRefCycle(String),
    EnvVarMissing(String),
    /// A `${...}` substitution token uses an operator form that meclaw does not
    /// support (e.g. `${VAR:=x}`, `${VAR-x}`, `${VAR:+x}`, `${VAR:?msg}`). Only
    /// the plain `${VAR}` and POSIX default `${VAR:-fallback}` forms are valid
    /// for env tokens (spec § Variable substitution → `${ENV_VAR}` from `.env`).
    /// Carries the offending token inner string. Never silently passed through.
    UnsupportedSubstitution(String),
    CtxKeyMissing(String),
    /// GH #292: an `add_nodes` names a template that declares a key
    /// (`requires.ctx` / `requires.env`) the mutation does not supply — or one
    /// of the templates it reaches through a `ref` does. Raised before staging,
    /// so nothing is copied, renamed or registered; the message names the
    /// template, the class, the key and the template's own `because`.
    RequirementMissing(String),
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
    /// GH #404: the `params` block a staged cell would carry does not
    /// deserialize for the cell type it names — the same question
    /// `CellFactory::validate_params` answers for every cell at boot
    /// (`plan_bootstrap`), asked at the moment the params are written instead
    /// of six hours later.
    ///
    /// Before this variant existed, the two paths that put a cell into a colony
    /// disagreed: instantiation accepted what the boot refuses, so a template
    /// defect committed cleanly, the cell never did its job, and the colony
    /// refused to start at the next deploy, crash or host reboot — in front of
    /// whoever restarted it rather than whoever grew it (GH #401 was one
    /// instance of the class).
    ///
    /// Raised pre-destructively during staging, like [`Self::ContractIncomplete`]
    /// beside it: the `.staging` dir is discarded and the live tree is
    /// unchanged. Carries the staged config path plus the factory's own reason,
    /// verbatim — the refusal says what the boot would have said.
    InvalidParams(String),
    /// GH #133: an `add_edges` endpoint reaches INTO a hive that declared its
    /// ports (`params.ports`, opt-in) while the edge's other endpoint lies
    /// OUTSIDE that hive — a deep endpoint past the port, which would bypass
    /// the hive's contracts (filters, gates, audit). Raised pre-destructively
    /// during validation: nothing is staged, spawned or wired, the colony state
    /// is byte-identical afterwards. Carries the human-readable constellation
    /// (hive path, offending endpoint, declared ports).
    HivePortBoundary(String),
    /// GH #147: a hive port that declared a paired drain (`params.required_drains`)
    /// is wired from outside, and nothing routes the drain's hop out of the hive.
    /// The classic shape is an ingress whose refusals leave on a reject egress
    /// that nobody consumes — the refusal is then a dead end and the caller never
    /// learns the work was not done. Raised pre-destructively, like the port
    /// boundary. Carries the constellation plus the hive's own reason string.
    RequiredDrainMissing(String),
    /// GH #173: a hive declared its interface as lanes (`params.contract`) and
    /// something contradicts it. Two shapes: an `add_edges` entry stamps a
    /// `hop.route` the hive does not accept, or the hive's own graph no longer
    /// carries a lane it promises (an accepted lane with no door behind it, an
    /// emitted lane with no exit out through the hive path). Raised
    /// pre-destructively, like the port boundary. Carries the constellation
    /// plus the hive's own reason string.
    HiveContract(String),
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
            Self::TemplateRefCycle(_) => "template_ref_cycle",
            Self::EnvVarMissing(_) => "env_var_missing",
            Self::UnsupportedSubstitution(_) => "unsupported_substitution",
            Self::CtxKeyMissing(_) => "ctx_key_missing",
            Self::RequirementMissing(_) => "requirement_missing",
            Self::ScopeOutOfBounds { .. } => "scope_out_of_bounds",
            Self::UnknownCellType(_) => "unknown_cell_type",
            Self::ResumeRequiresStoppedCell(_) => "resume_requires_stopped_cell",
            Self::ResumeTypeMismatch(_) => "resume_type_mismatch",
            Self::ContractIncomplete(_) => "contract_incomplete",
            Self::InvalidParams(_) => "invalid_params",
            Self::HivePortBoundary(_) => "hive_port_boundary",
            Self::RequiredDrainMissing(_) => "required_drain_missing",
            Self::HiveContract(_) => "hive_contract",
            // Never reached: the call-site panics (strict-fail) before any EDA
            // reply. Defensive fallback to "schema" so the spec error_code enum
            // (overview Z.293) stays unchanged — LiveTreeMutated is a strict-fail
            // signal, not an over-the-wire reject code.
            Self::LiveTreeMutated(_) => "schema",
        }
    }

    /// The human-readable payload string this error carries.
    ///
    /// Exhaustive on purpose — no `_` arm — so a new variant is a compile error
    /// here rather than a silently empty message. Lives next to
    /// [`MutationError::error_code`] because the two answer the same question
    /// about the same value ("what does this refusal say, and under which
    /// token"), and a second copy in
    /// [`crate::mutation::rejection`] had already started to be one: the
    /// rendered refusal and the `Result`-form reply must never disagree about
    /// what an error says (GH #293, W3 T21).
    pub fn message(&self) -> String {
        match self {
            Self::Schema(s)
            | Self::MatchNoHit(s)
            | Self::NamingCollision(s)
            | Self::Cycle(s)
            | Self::EdgeSchema(s)
            | Self::TemplateMissing(s)
            | Self::TemplateRefCycle(s)
            | Self::EnvVarMissing(s)
            | Self::UnsupportedSubstitution(s)
            | Self::CtxKeyMissing(s)
            | Self::RequirementMissing(s)
            | Self::UnknownCellType(s)
            | Self::ResumeRequiresStoppedCell(s)
            | Self::ResumeTypeMismatch(s)
            | Self::ContractIncomplete(s)
            | Self::InvalidParams(s)
            | Self::HivePortBoundary(s)
            | Self::RequiredDrainMissing(s)
            | Self::HiveContract(s)
            | Self::LiveTreeMutated(s) => s.clone(),
            Self::ScopeOutOfBounds { path } => path.as_str().to_string(),
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
        /// GH #293 — the structured form of the same refusal: every violation
        /// the refusing stage produced, in the order the stage produced them.
        ///
        /// ADDITIVE. `error_code` and `details` keep saying exactly what they
        /// said (`details` being the rendered form of these entries when the
        /// refusal came out of the collecting pipeline), so a reader that only
        /// knows the two string fields is unaffected. Empty for the refusals
        /// that do not run through the pipeline — the apply-stage and runtime
        /// ones, which judge what happens when a diff is applied rather than
        /// what the diff says.
        violations: Vec<rejection::Violation>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GH #254 — the SET claim behind the per-variant assertions below: the
    /// spec says `error_code` **is an enum**, and this pins the whole of it.
    ///
    /// Three producers together make up that enum, which is exactly why no
    /// single existing test could carry the row:
    ///
    /// * `MutationError::error_code` — the validation refusals;
    /// * two constants in `colony.rs` — `term_timeout` and
    ///   `stop_wiring_unavailable` come from the lifecycle path, not from a
    ///   `MutationError`, and are pinned individually there;
    /// * `subtree_resume_unsupported` — **reserved**: listed in the spec, with
    ///   no producer in the tree. It is named here so the gap is a recorded
    ///   decision rather than a hole somebody rediscovers.
    ///
    /// The `match` in [`MutationError::error_code`] has no `_` arm, so a new
    /// variant already fails to compile there; what this adds is the other
    /// direction — a token in the spec list that nothing produces, or a code
    /// produced that the spec never promised.
    #[test]
    fn the_error_code_enum_is_exactly_what_the_spec_lists() {
        // Every variant, constructed. No `_` arm exists in `error_code`, so this
        // list going stale shows up as a missing token below rather than never.
        let produced: Vec<&'static str> = vec![
            MutationError::Schema("x".into()).error_code(),
            MutationError::MatchNoHit("x".into()).error_code(),
            MutationError::NamingCollision("x".into()).error_code(),
            MutationError::Cycle("x".into()).error_code(),
            MutationError::EdgeSchema("x".into()).error_code(),
            MutationError::TemplateMissing("x".into()).error_code(),
            MutationError::TemplateRefCycle("x".into()).error_code(),
            MutationError::EnvVarMissing("x".into()).error_code(),
            MutationError::UnsupportedSubstitution("x".into()).error_code(),
            MutationError::CtxKeyMissing("x".into()).error_code(),
            MutationError::RequirementMissing("x".into()).error_code(),
            MutationError::ScopeOutOfBounds {
                path: Path::new("/b"),
            }
            .error_code(),
            MutationError::UnknownCellType("x".into()).error_code(),
            MutationError::ResumeRequiresStoppedCell("x".into()).error_code(),
            MutationError::ResumeTypeMismatch("x".into()).error_code(),
            MutationError::ContractIncomplete("x".into()).error_code(),
            MutationError::InvalidParams("x".into()).error_code(),
            MutationError::HivePortBoundary("x".into()).error_code(),
            MutationError::RequiredDrainMissing("x".into()).error_code(),
            MutationError::HiveContract("x".into()).error_code(),
            // Deliberately folded onto `schema`: a strict-fail signal, never an
            // over-the-wire reject code (see the comment at `error_code`).
            MutationError::LiveTreeMutated("x".into()).error_code(),
        ];

        // The two the lifecycle path emits, pinned individually in `colony.rs`.
        let lifecycle = ["term_timeout", "stop_wiring_unavailable"];
        // Listed by the spec, produced by nothing. Named, not silently missing.
        let reserved = ["subtree_resume_unsupported"];

        let mut have: std::collections::BTreeSet<&str> = produced.iter().copied().collect();
        have.extend(lifecycle);
        have.extend(reserved);

        let spec: std::collections::BTreeSet<&str> = [
            "schema",
            "match_no_hit",
            "naming_collision",
            "cycle",
            "edge_schema",
            "template_missing",
            "env_var_missing",
            "unsupported_substitution",
            "ctx_key_missing",
            "scope_out_of_bounds",
            "unknown_cell_type",
            "stop_wiring_unavailable",
            "term_timeout",
            "resume_requires_stopped_cell",
            "subtree_resume_unsupported",
            "resume_type_mismatch",
            "contract_incomplete",
            "invalid_params",
            "hive_port_boundary",
            "hive_contract",
            "required_drain_missing",
            "template_ref_cycle",
            "requirement_missing",
        ]
        .into_iter()
        .collect();

        let missing: Vec<_> = spec.difference(&have).collect();
        assert!(
            missing.is_empty(),
            "the spec promises error codes nothing in the tree produces: \
             {missing:?} -- either build them or retract them from \
             `docs/meclaw-overview.md` § Validation (a promise without a \
             producer is the GH #254 class)"
        );
        let extra: Vec<_> = have.difference(&spec).collect();
        assert!(
            extra.is_empty(),
            "the tree produces error codes the spec does not list: {extra:?} -- \
             `error_code` is documented as an ENUM, so a caller matching on it \
             would meet a token the contract never named"
        );
        assert_eq!(spec.len(), 23, "the documented enum is 23 tokens wide");
    }

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
        assert_eq!(
            MutationError::TemplateRefCycle("x".into()).error_code(),
            "template_ref_cycle"
        );
        assert_eq!(
            MutationError::RequirementMissing("x".into()).error_code(),
            "requirement_missing"
        );
    }

    #[test]
    fn hive_port_boundary_maps_to_error_code() {
        assert_eq!(
            MutationError::HivePortBoundary("x".into()).error_code(),
            "hive_port_boundary"
        );
    }

    #[test]
    fn hive_contract_maps_to_error_code() {
        assert_eq!(
            MutationError::HiveContract("x".into()).error_code(),
            "hive_contract"
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

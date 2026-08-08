//! Dead-letter record used by Colony to surface unroutable messages.
//!
//! Phase-2 scope: Colony stores these in a bounded `VecDeque` (Ring-Buffer,
//! drop-oldest on overflow). Inspection happens via the Phase-2 test hook
//! `ColonyMsg::DrainDeadLetters`; the spec-symmetric `/colony/dead_letters` +
//! `reply_to` roundtrip arrives in Phase 3 (UBF header) / Phase 12 (HTTP API).

use meclaw_core::{Message, Path};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeadLetterReason {
    /// Resolved target is not registered (no entry in Colony's HashMap).
    UnresolvedPath,
    /// The Hive path was reachable and resolved, but no out-edge of the Hive
    /// matched the message. Deliberately distinct from `UnresolvedPath` (the
    /// Hive itself was found; the graph simply had no onward route).
    /// Spec ref: docs/meclaw-overview.md l.553 — special case, no out-edge →
    /// `DeadLetterReason::HiveNoRoute`, canonical string `hive_no_route`.
    HiveNoRoute,
    /// TTL hit zero before a registered target accepted the message.
    /// Per spec § Behavior on routing errors.
    TtlExpired,
    /// Target is a `/colony/<x>` endpoint that exists in the spec but is not
    /// yet implemented in the current phase (e.g., `/colony/templates` in Phase 2).
    ColonyEndpointUnimplemented,
    /// Target is `/colony` without a subpath (spec § /colony als virtueller Endpunkt
    /// Z. 407: not addressable).
    ColonyEndpointInvalid,
    /// Cell emitted a body that fails UBF JSON-Schema validation.
    /// Set in debug builds only; the variant exists in release builds
    /// for API stability but is never constructed there.
    InvalidUbfBody,
    /// Target path exists (cell or hive) but is disconnected/inactive — the
    /// edge-derived activity recompute deactivated it. Phase-13.5 Lifecycle-3b:
    /// the unread mailbox remainder of a disconnected cell is drained here, and
    /// (later) messages routed to an inactive target land here too. Spec ref:
    /// docs/meclaw-overview.md Z.593/595, canonical string `cell_inactive`.
    CellInactive,
    /// A `Body::Blob` referenced a blob that could not be resolved at the cell
    /// delivery boundary (blob/sidecar missing — reader-contract Z.1362). The
    /// message is dead-lettered instead of handing the cell an unreadable UUID.
    /// Phase-13.5 A8. Canonical string `blob_unavailable` — a new entry on the
    /// stable error_code list (spec Z.593; doc-sync is backlog).
    BlobUnavailable,
    /// A message failed the substrate-side required-`consumes` check at the
    /// cell-delivery boundary — the cell was not invoked (config.md § consumes;
    /// reply-path uses the SAME canonical token). Canonical string
    /// `consumes_violation`.
    ConsumesViolation,
    /// A NON-`code` cell emission violated its `contract.emits` at the central
    /// emits check in the colony's outputs arm (flag-gated via
    /// `resolve_validate_emits`; `code` validates in-cell, always-on). The
    /// emission was dropped; with an `input_reply_to` an error reply is routed
    /// instead and no dead letter is recorded. Canonical string
    /// `contract_violation` (same token as the code-cell reply,
    /// cell-types.md Z.264).
    ContractViolation,
    /// A cell emission matched no out-edge of the emitting cell — the
    /// Cell-emission analogue to `hive_no_route` (Phase-16 W2 / Ruling A1).
    /// The implicit identity-decision (deliver to the cell's own emission
    /// target when no edge matched) is removed; default routing is a settable
    /// catch-all out-edge. Canonical string `no_route` — a new entry on the
    /// stable error_code list (spec § Behavior on routing errors).
    NoRoute,
}

impl DeadLetterReason {
    /// Canonical string per spec § Behavior on routing errors (Cascade),
    /// Absatz "Kanonische error_code-Strings". Stable API contract — new
    /// reasons may be added, existing strings must not change.
    pub fn as_code(&self) -> &'static str {
        match self {
            Self::UnresolvedPath => "unresolved_path",
            Self::HiveNoRoute => "hive_no_route",
            Self::TtlExpired => "ttl_expired",
            Self::ColonyEndpointUnimplemented => "colony_endpoint_unimplemented",
            Self::ColonyEndpointInvalid => "colony_endpoint_invalid",
            Self::InvalidUbfBody => "invalid_ubf_body",
            Self::CellInactive => "cell_inactive",
            Self::BlobUnavailable => "blob_unavailable",
            Self::ConsumesViolation => "consumes_violation",
            Self::ContractViolation => "contract_violation",
            Self::NoRoute => "no_route",
        }
    }

    /// Inverse of [`Self::as_code`] — reconstruct the reason from its canonical
    /// string. Phase-16 W6d (A6): the persistent `dead_letters` table stores the
    /// `error_code` string; the DLQ-drain reconstructs the full `DeadLetter` from
    /// the DB and needs the enum back. `None` for an unknown code (forward-compat:
    /// a row written by a newer schema with a reason this build doesn't know).
    pub fn from_code(code: &str) -> Option<Self> {
        Some(match code {
            "unresolved_path" => Self::UnresolvedPath,
            "hive_no_route" => Self::HiveNoRoute,
            "ttl_expired" => Self::TtlExpired,
            "colony_endpoint_unimplemented" => Self::ColonyEndpointUnimplemented,
            "colony_endpoint_invalid" => Self::ColonyEndpointInvalid,
            "invalid_ubf_body" => Self::InvalidUbfBody,
            "cell_inactive" => Self::CellInactive,
            "blob_unavailable" => Self::BlobUnavailable,
            "consumes_violation" => Self::ConsumesViolation,
            "contract_violation" => Self::ContractViolation,
            "no_route" => Self::NoRoute,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct DeadLetter {
    pub sender_path: Path,
    pub original_target: Path,
    pub resolved_target: Path,
    pub message: Message,
    pub reason: DeadLetterReason,
}

#[cfg(test)]
mod tests_3a {
    use super::*;

    #[test]
    fn ttl_expired_variant_exists() {
        let r = DeadLetterReason::TtlExpired;
        assert_eq!(r, DeadLetterReason::TtlExpired);
    }

    #[test]
    fn as_code_returns_canonical_strings() {
        assert_eq!(
            DeadLetterReason::UnresolvedPath.as_code(),
            "unresolved_path"
        );
        assert_eq!(DeadLetterReason::TtlExpired.as_code(), "ttl_expired");
        assert_eq!(
            DeadLetterReason::ColonyEndpointUnimplemented.as_code(),
            "colony_endpoint_unimplemented"
        );
        assert_eq!(
            DeadLetterReason::ColonyEndpointInvalid.as_code(),
            "colony_endpoint_invalid"
        );
    }
}

#[cfg(test)]
mod tests_13_5 {
    use super::*;

    #[test]
    fn hive_no_route_as_code() {
        assert_eq!(DeadLetterReason::HiveNoRoute.as_code(), "hive_no_route");
    }
}

#[cfg(test)]
mod tests_3b {
    use super::*;

    #[test]
    fn invalid_ubf_body_variant_exists() {
        let r = DeadLetterReason::InvalidUbfBody;
        assert_eq!(r, DeadLetterReason::InvalidUbfBody);
    }

    #[test]
    fn invalid_ubf_body_as_code_returns_canonical_string() {
        assert_eq!(
            DeadLetterReason::InvalidUbfBody.as_code(),
            "invalid_ubf_body"
        );
    }

    /// Phase-13.5 Lifecycle-3b Task 4: the `cell_inactive` reason pins the
    /// canonical error_code string (spec Z.593/595, stable API contract).
    #[test]
    fn cell_inactive_as_code_returns_canonical_string() {
        assert_eq!(DeadLetterReason::CellInactive.as_code(), "cell_inactive");
    }

    #[test]
    fn consumes_violation_as_code() {
        assert_eq!(
            DeadLetterReason::ConsumesViolation.as_code(),
            "consumes_violation"
        );
    }

    /// Hardening Slice 3 (Task 3.2): the `contract_violation` reason pins the
    /// canonical error_code string (same token as the code-cell reply,
    /// cell-types.md Z.264 — stable API contract).
    #[test]
    fn contract_violation_as_code() {
        assert_eq!(
            DeadLetterReason::ContractViolation.as_code(),
            "contract_violation"
        );
    }

    /// Phase-16 W2 (A1): the `no_route` reason pins the canonical error_code
    /// string — Cell-emission analogue to `hive_no_route` when no out-edge
    /// matches (stable API contract).
    #[test]
    fn no_route_as_code() {
        assert_eq!(DeadLetterReason::NoRoute.as_code(), "no_route");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::MessageBuilder;

    #[test]
    fn dead_letter_round_trips_fields() {
        let dl = DeadLetter {
            sender_path: Path::new("/"),
            original_target: Path::new("/missing"),
            resolved_target: Path::new("/missing"),
            message: MessageBuilder::new(Path::new("/missing")).build(),
            reason: DeadLetterReason::UnresolvedPath,
        };
        assert_eq!(dl.resolved_target.as_str(), "/missing");
        assert_eq!(dl.reason, DeadLetterReason::UnresolvedPath);
    }
}

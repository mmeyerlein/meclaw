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
    /// GH #19 (D-025): an in-message pointer chain (`messages_id`/`text_id`
    /// inside `messages[]`) either exceeded `colony.json
    /// blob_max_recursion_depth` or revisited a blob already on its own path
    /// (a mutual cycle A→B→A, which UUID immutability does NOT exclude). Both
    /// are the same class of failure — a chain that does not terminate — and
    /// both report the canonical string the spec has always promised for it.
    /// The distinction between "too deep" and "cyclic" lives in the log line,
    /// not on the wire. Canonical string `blob_recursion_too_deep`
    /// (docs/meclaw-overview.md § Behavior on routing errors).
    BlobRecursionTooDeep,
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
    /// GH #285: a message reached a hive's DECLARED SLOT while nothing was bound
    /// behind it, and the hive declared `"unbound": "error"` — an unfinished
    /// topology, said in the hive's own words. Deliberately distinct from
    /// `unresolved_path`: that address is not unknown, it is announced and
    /// empty, and only the declaration can tell the two apart. The counterpart
    /// `"unbound": "drop"` produces NO dead letter at all. The entry's
    /// `resolved_target` is the slot address, which names the hive path and the
    /// slot name. Canonical string `slot_unbound`.
    SlotUnbound,
    /// GH #285 (W4 T12): a message reached a hive's declared slot with
    /// `"unbound": "park"` while that slot's queue already held
    /// `colony.json slot_park_max` messages. The NEWEST arrival is the one
    /// refused — the earliest context is what a later reader cannot
    /// reconstruct, so it is what the bound protects. The entry's
    /// `resolved_target` is the slot address, exactly as for `slot_unbound`.
    /// Canonical string `slot_park_overflow`.
    SlotParkOverflow,
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
            Self::BlobRecursionTooDeep => "blob_recursion_too_deep",
            Self::ConsumesViolation => "consumes_violation",
            Self::ContractViolation => "contract_violation",
            Self::NoRoute => "no_route",
            Self::SlotUnbound => "slot_unbound",
            Self::SlotParkOverflow => "slot_park_overflow",
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
            "blob_recursion_too_deep" => Self::BlobRecursionTooDeep,
            "consumes_violation" => Self::ConsumesViolation,
            "contract_violation" => Self::ContractViolation,
            "no_route" => Self::NoRoute,
            "slot_unbound" => Self::SlotUnbound,
            "slot_park_overflow" => Self::SlotParkOverflow,
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

    /// GH #254 — the SET claim, which the per-variant assertions below cannot
    /// make: `error_code` on a dead letter is a **canonical, closed** vocabulary.
    ///
    /// The three halves of that sentence, each asserted rather than assumed:
    ///
    /// 1. **Closed.** The array is built from a `match` with **no `_` arm**, so a
    ///    new variant added to `DeadLetterReason` does not slip past this test —
    ///    it stops compiling here, which is the only mechanism that survives a
    ///    contributor who never reads this file.
    /// 2. **Canonical.** Every code round-trips through `from_code`, so the
    ///    string a dead letter carries is the string that reconstructs it.
    /// 3. **Unambiguous.** No two variants share a code — a duplicate would make
    ///    the round trip lossy for one of them while every per-variant assertion
    ///    still passed.
    ///
    /// The count is stated explicitly so a *removal* is as loud as an addition:
    /// the exhaustive match alone would happily shrink.
    #[test]
    fn every_reason_maps_to_a_canonical_code_and_back() {
        use DeadLetterReason::*;
        // No `_` arm, deliberately: adding a variant must break this line.
        let all: Vec<DeadLetterReason> = vec![
            UnresolvedPath,
            HiveNoRoute,
            TtlExpired,
            ColonyEndpointUnimplemented,
            ColonyEndpointInvalid,
            InvalidUbfBody,
            CellInactive,
            BlobUnavailable,
            BlobRecursionTooDeep,
            ConsumesViolation,
            ContractViolation,
            NoRoute,
            SlotUnbound,
            SlotParkOverflow,
        ];
        // The compile-time half of "closed": this match names every variant and
        // has no catch-all, so a new one is a compiler error in this function.
        fn exhaustive(r: &DeadLetterReason) -> &'static str {
            match r {
                DeadLetterReason::UnresolvedPath => "unresolved_path",
                DeadLetterReason::HiveNoRoute => "hive_no_route",
                DeadLetterReason::TtlExpired => "ttl_expired",
                DeadLetterReason::ColonyEndpointUnimplemented => "colony_endpoint_unimplemented",
                DeadLetterReason::ColonyEndpointInvalid => "colony_endpoint_invalid",
                DeadLetterReason::InvalidUbfBody => "invalid_ubf_body",
                DeadLetterReason::CellInactive => "cell_inactive",
                DeadLetterReason::BlobUnavailable => "blob_unavailable",
                DeadLetterReason::BlobRecursionTooDeep => "blob_recursion_too_deep",
                DeadLetterReason::ConsumesViolation => "consumes_violation",
                DeadLetterReason::ContractViolation => "contract_violation",
                DeadLetterReason::NoRoute => "no_route",
                DeadLetterReason::SlotUnbound => "slot_unbound",
                DeadLetterReason::SlotParkOverflow => "slot_park_overflow",
            }
        }

        assert_eq!(
            all.len(),
            14,
            "the canonical set is 14 codes; a variant was added or removed \
             without moving this count, and the spec list has to move with it"
        );

        let mut seen: std::collections::BTreeSet<&'static str> = Default::default();
        for r in &all {
            let code = r.as_code();
            assert_eq!(
                code,
                exhaustive(r),
                "`as_code` disagrees with the exhaustive map for {r:?}"
            );
            assert!(
                seen.insert(code),
                "two reasons share the canonical code {code:?} — the round trip \
                 below cannot be right for both, and a reader of the dead-letter \
                 queue cannot tell them apart"
            );
            assert_eq!(
                DeadLetterReason::from_code(code),
                Some(r.clone()),
                "{r:?} does not survive as_code -> from_code; the persisted \
                 `error_code` is what the DLQ drain reconstructs from"
            );
        }
        assert_eq!(seen.len(), all.len(), "every code is distinct");

        // And the inverse is closed too: a string nobody emits is not a reason.
        assert_eq!(DeadLetterReason::from_code("not_a_reason"), None);
        assert_eq!(DeadLetterReason::from_code(""), None);
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

    /// GH #285 (W4 T11): the `slot_unbound` reason pins the canonical
    /// error_code string, and the round trip through the persisted
    /// `dead_letters` table — a reason the DLQ drain cannot reconstruct is a
    /// reason that vanishes on the way back out.
    #[test]
    fn slot_unbound_as_code_round_trips() {
        assert_eq!(DeadLetterReason::SlotUnbound.as_code(), "slot_unbound");
        assert_eq!(
            DeadLetterReason::from_code("slot_unbound"),
            Some(DeadLetterReason::SlotUnbound)
        );
    }

    /// GH #285 (W4 T12): the same contract for the bound of a `park` queue —
    /// canonical string plus the round trip the DLQ drain depends on.
    #[test]
    fn slot_park_overflow_as_code_round_trips() {
        assert_eq!(
            DeadLetterReason::SlotParkOverflow.as_code(),
            "slot_park_overflow"
        );
        assert_eq!(
            DeadLetterReason::from_code("slot_park_overflow"),
            Some(DeadLetterReason::SlotParkOverflow)
        );
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

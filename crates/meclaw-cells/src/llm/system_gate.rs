//! GH #118 — the write gate in front of the persistent `system` tree.
//!
//! The `system` tree is long-term state in the `llm` cell's `cell.db`, rebuilt
//! into the prompt on every `handle()`, and it also carries the tool menu
//! (`system.tools.*`). Before this gate, ANY cell with an edge to an `llm` cell
//! could set any slot, unlimited in size and slot count, durably across
//! restarts — whoever could route to the cell owned its prompt and its tools.
//!
//! The gate is a **message-path** gate. Two independent halves:
//!
//! * **Bounds — always on.** A leaf larger than `system_max_leaf_bytes` or a
//!   write that would push the tree past `system_max_slots` distinct slots is a
//!   loud reject, never a truncation and never a silent drop. This is the safe
//!   default: it needs no declaration and holds for every cell.
//! * **Allowlist — opt-in.** `system_writable` pins which subtrees a MESSAGE
//!   may write at all. Unset (the default) means "no allowlist configured" and
//!   every slot path stays writable — the operator's direct `@external` system
//!   update and every in-topology writer keep working byte-identically to
//!   before this gate existed. A cell that declares the list accepts writes
//!   only under the declared prefixes.
//!
//! All three knobs are **immutable params** (`params::IMMUTABLE_PARAM_KEYS`): a
//! message must never be able to widen the gate it is about to pass.
//!
//! What the gate deliberately does NOT do: gate on the SENDER. A cell knows no
//! topology (`meclaw-overview.md` § Cell-Modell — only message + params), and
//! the operator lane arrives as `@external` with no stable cell identity behind
//! it. The declaration is therefore about slot paths, not about sources.
//!
//! The `seed/system.jsonl` loader does NOT pass this gate: a seed file is
//! configuration on the same trust tier as `config.json` (the operator authors
//! both, side by side in the cell directory) — the same tier split the
//! params-update reject already draws, where `config.json` may set `api_key`
//! and a message may not. Gating the seed against a declaration written by the
//! same hand would be circular, and it would lock a pinned cell out of its own
//! identity at boot. The intended shape is exactly the opposite: seed the
//! identity at boot, then pin the message-writable surface to the slots that
//! must stay live (e.g. `handover`, `tools`).

use meclaw_core::serde_json::Value;

/// Default cap on the number of distinct slots in the persistent `system` tree.
///
/// Generous enough for a real tool menu (one slot per MCP tool) plus the
/// identity/instruction/handover/memory families, tight enough that an unbounded
/// writer hits a wall instead of growing the prompt forever.
pub(crate) const DEFAULT_SYSTEM_MAX_SLOTS: usize = 256;

/// Default cap on the serialized size of ONE system leaf, in bytes.
///
/// 64 KiB is far above every legitimate leaf in the reference templates (a
/// handover summary, a tool schema, a persona) and far below a size that would
/// blow up the prompt on its own.
pub(crate) const DEFAULT_SYSTEM_MAX_LEAF_BYTES: usize = 65_536;

/// Why a write into the persistent `system` tree was refused (GH #118).
///
/// Every variant names the offending slot; none of them ever carries the leaf
/// CONTENT (the same secret-hygiene rule the params-update reject follows —
/// a system leaf is prompt material and may hold private context).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GateReject {
    /// The cell declares `system_writable` and the slot is not under any
    /// declared prefix.
    NotWritable {
        /// Dotted slot path of the refused write.
        slot: String,
        /// The declared prefixes, for the reject detail.
        declared: Vec<String>,
    },
    /// The serialized leaf is larger than `system_max_leaf_bytes`.
    LeafTooLarge {
        /// Dotted slot path of the refused write.
        slot: String,
        /// Serialized size of the offered leaf, in bytes.
        bytes: usize,
        /// The configured cap.
        max: usize,
    },
    /// The write would push the tree past `system_max_slots` distinct slots.
    TooManySlots {
        /// How many distinct slots the tree would hold after the write.
        would_be: usize,
        /// The configured cap.
        max: usize,
    },
}

impl GateReject {
    /// Human-readable reject detail for `meta.error.detail`. Names the slot and
    /// the rule, never a leaf value.
    pub(crate) fn detail(&self) -> String {
        match self {
            GateReject::NotWritable { slot, declared } => {
                let list = declared
                    .iter()
                    .map(|p| format!("'{p}'"))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "system slot '{slot}': not writable by message (GH #118). This cell declares \
                     `system_writable` = [{list}]; a system update may only touch those subtrees. \
                     Nothing was written"
                )
            }
            GateReject::LeafTooLarge { slot, bytes, max } => format!(
                "system slot '{slot}': leaf is {bytes} bytes, over the `system_max_leaf_bytes` \
                 limit of {max} (GH #118). The write is refused whole — a system leaf is never \
                 truncated, because a half prompt is worse than none. Nothing was written"
            ),
            GateReject::TooManySlots { would_be, max } => format!(
                "system tree would hold {would_be} slots, over the `system_max_slots` limit of \
                 {max} (GH #118). The persistent system tree is rebuilt into every prompt; an \
                 unbounded slot count is an unbounded prompt. Nothing was written"
            ),
        }
    }

    /// Short, value-free reason tag for the `WARN` log line.
    pub(crate) fn reason(&self) -> &'static str {
        match self {
            GateReject::NotWritable { .. } => "not_writable",
            GateReject::LeafTooLarge { .. } => "leaf_too_large",
            GateReject::TooManySlots { .. } => "too_many_slots",
        }
    }

    /// The offending slot path, or `None` for a tree-wide reject.
    pub(crate) fn slot(&self) -> Option<&str> {
        match self {
            GateReject::NotWritable { slot, .. } | GateReject::LeafTooLarge { slot, .. } => {
                Some(slot)
            }
            GateReject::TooManySlots { .. } => None,
        }
    }
}

/// The resolved write policy for one `llm` cell's persistent `system` tree.
///
/// Built from the cell's params once per call (three field copies — cheaper
/// than threading `&LlmParams` through the DB closure).
#[derive(Debug, Clone)]
pub(crate) struct SystemGate {
    /// Cap on distinct slots in the tree.
    max_slots: usize,
    /// Cap on the serialized size of one leaf.
    max_leaf_bytes: usize,
    /// Declared writable prefixes. Empty = no allowlist configured.
    writable: Vec<String>,
}

impl Default for SystemGate {
    /// The default gate: bounds on, no allowlist. This is what a cell without
    /// any `system_*` param gets, and it accepts every write the cell accepted
    /// before GH #118 existed (up to the bounds).
    fn default() -> Self {
        Self {
            max_slots: DEFAULT_SYSTEM_MAX_SLOTS,
            max_leaf_bytes: DEFAULT_SYSTEM_MAX_LEAF_BYTES,
            writable: Vec::new(),
        }
    }
}

impl SystemGate {
    /// Build the gate from the cell's params.
    pub(crate) fn from_params(p: &crate::llm::params::LlmParams) -> Self {
        Self {
            max_slots: p.system_max_slots,
            max_leaf_bytes: p.system_max_leaf_bytes,
            writable: p.system_writable.clone(),
        }
    }

    /// Test-only constructor so sibling modules can pin the gate's edges
    /// without routing through `LlmParams`.
    #[cfg(test)]
    pub(crate) fn for_test(max_slots: usize, max_leaf_bytes: usize, writable: &[&str]) -> Self {
        Self {
            max_slots,
            max_leaf_bytes,
            writable: writable.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Is this slot path writable by a message?
    ///
    /// No declaration → yes. With a declaration, a prefix matches only on a
    /// SEGMENT boundary: `"identity"` covers `identity` and `identity.soul`,
    /// but never `identityx`.
    fn is_writable(&self, slot: &str) -> bool {
        if self.writable.is_empty() {
            return true;
        }
        self.writable
            .iter()
            .any(|p| slot == p || slot.starts_with(&format!("{p}.")))
    }

    /// Check every offered leaf against the allowlist and the per-leaf size cap.
    ///
    /// Pure — no database. All-or-nothing: the FIRST offending leaf (slots
    /// sorted, so the verdict is deterministic) refuses the whole batch, exactly
    /// like the params-update reject. The caller must not have written anything
    /// yet when this returns `Err`.
    pub(crate) fn check_leaves(&self, leaves: &[(String, Value)]) -> Result<(), GateReject> {
        let mut sorted: Vec<&(String, Value)> = leaves.iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        for (slot, leaf) in sorted {
            if !self.is_writable(slot) {
                return Err(GateReject::NotWritable {
                    slot: slot.clone(),
                    declared: self.writable.clone(),
                });
            }
            let bytes = meclaw_core::serde_json::to_string(leaf)
                .map(|s| s.len())
                .unwrap_or(usize::MAX);
            if bytes > self.max_leaf_bytes {
                return Err(GateReject::LeafTooLarge {
                    slot: slot.clone(),
                    bytes,
                    max: self.max_leaf_bytes,
                });
            }
        }
        Ok(())
    }

    /// Check the slot budget: how many distinct slots the tree would hold once
    /// `novel` previously-unseen slots are added to `existing` rows.
    ///
    /// Overwriting a slot that is already there does not grow the tree and is
    /// therefore always within budget — a cell parked at the limit can still
    /// refresh its handover, it just cannot open new subtrees.
    pub(crate) fn check_slot_budget(
        &self,
        existing: usize,
        novel: usize,
    ) -> Result<(), GateReject> {
        let would_be = existing + novel;
        if would_be > self.max_slots {
            return Err(GateReject::TooManySlots {
                would_be,
                max: self.max_slots,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    fn gate_with(writable: &[&str]) -> SystemGate {
        SystemGate {
            writable: writable.iter().map(|s| s.to_string()).collect(),
            ..SystemGate::default()
        }
    }

    /// The safe default is "bounded", not "closed": a cell that declares
    /// nothing keeps accepting every slot path it accepted before GH #118 —
    /// this is what keeps the operator's direct `@external` system update and
    /// every in-topology writer working.
    #[test]
    fn the_default_gate_accepts_any_slot_path() {
        let g = SystemGate::default();
        let leaves = vec![
            ("identity".to_string(), json!({"text": "I am Egon."})),
            ("handover".to_string(), json!({"text": "yesterday"})),
            ("tools.calc".to_string(), json!({"text": "{}"})),
            ("whatever.deep.slot".to_string(), json!({"text": "x"})),
        ];
        g.check_leaves(&leaves).unwrap();
    }

    #[test]
    fn a_declared_prefix_accepts_the_exact_slot_and_everything_under_it() {
        let g = gate_with(&["handover", "tools"]);
        g.check_leaves(&[
            ("handover".to_string(), json!({"text": "a"})),
            ("handover.summary".to_string(), json!({"text": "b"})),
            ("tools.calc".to_string(), json!({"text": "{}"})),
        ])
        .unwrap();
    }

    /// The gate's core case: a source that is not part of the declared surface
    /// cannot rewrite the cell's identity any more.
    #[test]
    fn a_slot_outside_the_declaration_is_refused_by_name() {
        let g = gate_with(&["handover"]);
        let err = g
            .check_leaves(&[(
                "identity".to_string(),
                json!({"text": "I am someone else."}),
            )])
            .unwrap_err();
        assert_eq!(err.reason(), "not_writable");
        assert_eq!(err.slot(), Some("identity"));
        let d = err.detail();
        assert!(d.contains("'identity'"), "must name the slot: {d}");
        assert!(d.contains("system_writable"), "must name the rule: {d}");
        assert!(d.contains("GH #118"), "must name the issue: {d}");
        assert!(
            !d.contains("I am someone else."),
            "the detail must never echo the leaf content: {d}"
        );
    }

    /// Prefix matching is on segment boundaries — otherwise declaring
    /// `identity` would silently open `identity_backdoor` too.
    #[test]
    fn a_declared_prefix_does_not_match_mid_segment() {
        let g = gate_with(&["identity"]);
        let err = g
            .check_leaves(&[("identityx".to_string(), json!({"text": "x"}))])
            .unwrap_err();
        assert_eq!(err.slot(), Some("identityx"));
    }

    /// One bad leaf refuses the whole batch — no partial write, mirroring the
    /// params-update all-or-nothing rule.
    #[test]
    fn one_refused_leaf_refuses_the_whole_batch() {
        let g = gate_with(&["handover"]);
        let err = g
            .check_leaves(&[
                ("handover".to_string(), json!({"text": "fine"})),
                ("identity".to_string(), json!({"text": "not fine"})),
            ])
            .unwrap_err();
        assert_eq!(err.slot(), Some("identity"));
    }

    #[test]
    fn a_leaf_over_the_size_cap_is_refused_whole_not_truncated() {
        let g = SystemGate {
            max_leaf_bytes: 64,
            ..SystemGate::default()
        };
        let big = "x".repeat(200);
        let err = g
            .check_leaves(&[("identity".to_string(), json!({"text": big}))])
            .unwrap_err();
        assert_eq!(err.reason(), "leaf_too_large");
        let d = err.detail();
        assert!(d.contains("'identity'"), "must name the slot: {d}");
        assert!(d.contains("64"), "must name the limit: {d}");
        assert!(!d.contains("xxxxxxxx"), "must not echo the content: {d}");
    }

    #[test]
    fn a_leaf_at_the_size_cap_still_passes() {
        let g = SystemGate {
            max_leaf_bytes: 64,
            ..SystemGate::default()
        };
        // `{"text":"…"}` = 11 bytes of frame around the payload.
        let payload = "y".repeat(64 - 11);
        let leaf = json!({"text": payload});
        assert_eq!(meclaw_core::serde_json::to_string(&leaf).unwrap().len(), 64);
        g.check_leaves(&[("identity".to_string(), leaf)]).unwrap();
    }

    #[test]
    fn the_slot_budget_refuses_a_tree_that_would_grow_past_the_cap() {
        let g = SystemGate {
            max_slots: 3,
            ..SystemGate::default()
        };
        g.check_slot_budget(2, 1).unwrap();
        let err = g.check_slot_budget(3, 1).unwrap_err();
        assert_eq!(err.reason(), "too_many_slots");
        assert_eq!(err.slot(), None);
        let d = err.detail();
        assert!(d.contains('4'), "must name the resulting count: {d}");
        assert!(d.contains("system_max_slots"), "must name the rule: {d}");
    }

    /// Overwriting an existing slot never grows the tree: a cell parked at the
    /// limit can still refresh its handover.
    #[test]
    fn overwriting_an_existing_slot_stays_within_budget() {
        let g = SystemGate {
            max_slots: 3,
            ..SystemGate::default()
        };
        g.check_slot_budget(3, 0).unwrap();
    }
}

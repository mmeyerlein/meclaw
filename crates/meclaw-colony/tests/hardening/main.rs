//! Test harness `hardening` of `meclaw-colony`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

pub mod hardening_central_emits;
pub mod hardening_consumes_ingress;
pub mod hardening_contract_presence;
pub mod hardening_header_locality;
pub mod hardening_subtree_lazy_wake;
pub mod hardening_subtree_override_params_reject;
pub mod pre14_a1_per_form_blob_resolution;
pub mod pre14_b4_scope_boundary;
pub mod pre14_b6_deny_unknown_fields;
pub mod substrate_fix_add_edges_diff_nodes;
pub mod substrate_fix_mid_rename_strict_fail;
pub mod substrate_fix_scope_name_traversal;
pub mod substrate_fix_staging_atomicity;

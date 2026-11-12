//! Test harness `curator_proof` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

#[path = "../support/curator_hive.rs"]
pub mod curator_hive;
#[path = "../support/curator_lead.rs"]
pub mod curator_lead;
#[path = "../support/curator_target.rs"]
pub mod curator_target;
pub mod gh_d58_proof_a_fifth_model_cell_needs_no_change_in_the_curator;

//! Test harness `curator_lead` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

#[path = "../support/curator_hive.rs"]
pub mod curator_hive;
#[path = "../support/curator_lead.rs"]
pub mod curator_lead;
#[path = "../support/curator_target.rs"]
pub mod curator_target;
pub mod gh_d58_lead_a_composition_without_a_lead_runs_on_the_target_it_names;
pub mod gh_d58_lead_a_fresh_lead_leaves_the_target_to_the_curate;
pub mod gh_d58_lead_a_rebuild_without_a_lead_reaches_the_ledger;
pub mod gh_d58_lead_a_set_override_of_the_lead_params_passes_the_door;
pub mod gh_d58_lead_a_wired_lead_names_the_target_of_every_call;
pub mod gh_d58_lead_every_shipped_composition_with_a_lead_has_its_five_lanes;
pub mod gh_d58_lead_the_ask_leaves_with_the_ledger_reads;
pub mod gh_d58_lead_the_curator_holds_no_model_choice;
pub mod gh_d58_lead_without_a_lead_no_event_and_no_note_leaves;

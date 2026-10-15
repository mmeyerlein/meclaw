//! Test harness `store_1` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

pub mod access_template;
pub mod fitness_store;
pub mod gh1058_the_vault_unlocks_from_a_systemd_credential;
pub mod gh1060_code_on_a_grant;
pub mod gh1061_a_granted_embedder_keeps_its_budget_under_load;
pub mod gh1063_a_vault_grown_but_never_woken;
pub mod gh1094_every_granted_cell_may_ask_for_its_credential;
pub mod gh132_store_internal_write_surface;
pub mod gh138_argus_access_receptionist_params;
pub mod gh151_the_vault_has_no_way_out;
pub mod gh253_store_transfer;
pub mod gh280_the_bundle_carries_the_provenance_the_store_holds;
pub mod gh295_store_fan_out_bundle;
pub mod gh314_a_vault_does_not_travel;
pub mod gh331_store_error_paths_stamp_operation;
pub mod gh343_a_store_refusal_is_not_a_result;
pub mod gh452_the_vault_pilot_grows_a_granted_credential;
pub mod gh471_the_porters_mirror_their_stores;
pub mod gh483_a_store_publishes_what_seed_rows_must_satisfy;
pub mod gh555_a_store_writes_its_own_seed_set;
pub mod gh857_an_unchanged_store_buys_no_dream;
pub mod gh915_a_store_filters_on_a_json_path;
pub mod gh932_a_store_selects_only_what_the_round_may_see;
pub mod gh982_the_presenter_decider_runs_on_a_grant;
pub mod harness_task_store;
#[path = "../support/gh1060.rs"]
pub mod support;

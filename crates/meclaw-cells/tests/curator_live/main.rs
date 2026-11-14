//! Test harness `curator_live` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

#[path = "../support/assemble_cell.rs"]
pub mod assemble_cell;
#[path = "../support/curator_feed.rs"]
pub mod curator_feed;
#[path = "../support/curator_hive.rs"]
pub mod curator_hive;
#[path = "../support/curator_live.rs"]
pub mod curator_live;
pub mod gh_d54g_ack_listen;
pub mod gh_d54g_kprofil_window;
pub mod gh_d54g_kprofil_window_handover_voice;
pub mod gh_d54g_kprofil_window_history_kind;
pub mod gh_d54g_kprofil_window_transcript_voice;
pub mod gh_d58_live_a_delegation_parks_under_the_live_handle_or_the_session;
pub mod gh_d58_live_a_set_override_of_the_new_params_passes_the_door;
pub mod gh_d58_live_a_turn_with_a_live_handle_gets_start_window_and_feed;
pub mod gh_d58_live_a_turn_without_one_gets_neither;
pub mod gh_d58_live_no_curator_and_no_lead_script_reads_a_call_key_or_an_engine;
pub mod gh_d58_live_the_edges_translate_both_ways;
pub mod gh_d58_live_the_handle_is_set_only_for_a_session_of_its_own;
pub mod gh_d58_live_the_start_window_budget_is_a_parameter;
pub mod gh_d58_live_the_topic_ages_against_the_opening_of_the_live_session;

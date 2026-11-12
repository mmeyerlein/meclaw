//! Test harness `curator_judge` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

#[path = "../support/curator_hive.rs"]
pub mod curator_hive;
#[path = "../support/curator_live.rs"]
pub mod curator_live;
#[path = "../support/curator_target.rs"]
pub mod curator_target;
pub mod gh_d58_judge_a_domain_fills_its_leaf_of_the_targets_section;
pub mod gh_d58_judge_a_partial_turn_reads_across_sessions;
pub mod gh_d58_judge_a_second_composition_orders_a_view_under_another_name;
pub mod gh_d58_judge_a_set_override_of_the_new_params_passes_the_door;
pub mod gh_d58_judge_a_start_window_line_keeps_its_place_without_a_kind;
pub mod gh_d58_judge_a_structured_contribution_arrives_unread_and_whole;
pub mod gh_d58_judge_a_view_comes_back_as_system_and_messages;
pub mod gh_d58_judge_a_view_costs_no_more_messages_than_the_turns_ask_did;
pub mod gh_d58_judge_a_view_reads_and_leaves_no_trace_in_the_ledger;
pub mod gh_d58_judge_mutant_a_name_inside_a_phase_or_lane_trips_the_lock;
pub mod gh_d58_judge_parts_are_one_text;
pub mod gh_d58_judge_parts_in_a_curate_cost_no_message_of_their_own;
pub mod gh_d58_judge_the_feed_and_the_live_facts_name_what_the_seed_names;
#[path = "../support/history_judge.rs"]
pub mod history_judge;
#[path = "../support/history_view.rs"]
pub mod history_view;

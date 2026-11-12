//! Test harness `curator_moveout` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

#[path = "../support/curator_feed.rs"]
pub mod curator_feed;
#[path = "../support/curator_hive.rs"]
pub mod curator_hive;
#[path = "../support/curator_moveout.rs"]
pub mod curator_moveout;
pub mod gh_d58_moveout_a_conditional_feed_leaves_with_the_round;
pub mod gh_d58_moveout_a_held_round_counts_only_when_it_is_sent;
pub mod gh_d58_moveout_a_set_override_of_the_moved_params_passes_the_door;
pub mod gh_d58_moveout_a_side_call_goes_out_when_its_condition_holds;
pub mod gh_d58_moveout_a_side_call_marked_first_leaves_before_the_main_call;
pub mod gh_d58_moveout_a_side_call_never_ends_the_turn;
pub mod gh_d58_moveout_a_side_call_stays_in_when_it_does_not;
pub mod gh_d58_moveout_the_kind_of_a_call_travels_in_a_head_field;
pub mod gh_d58_moveout_the_lead_is_told_every_waiting_round_and_books_it;
pub mod gh_d58_moveout_the_lead_orders_the_interrupt_once;
pub mod gh_d58_moveout_the_policy_fills_the_tools_the_cut_left_open;
pub mod gh_d58_moveout_the_side_text_is_a_system_leaf_of_the_target;

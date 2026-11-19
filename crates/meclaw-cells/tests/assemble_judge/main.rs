//! Test harness `assemble_judge` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

#[path = "../support/assemble_cell.rs"]
pub mod assemble_cell;
#[path = "../support/curator_hive.rs"]
pub mod curator_hive;
pub mod gh1556_the_route_speaks_the_decisions_form;
pub mod gh1558_the_route_falls_back_on_an_uncalibrated_decider;
pub mod gh1562_a_late_stamp_keeps_its_risk;
pub mod gh1562_a_late_unsure_verdict_is_not_sharpened;
pub mod gh1562_a_stamp_after_the_fire_acts_on_nothing;
pub mod gh1564_a_woken_turn_answers_on_its_channel;
pub mod gh1564_the_wake_ends_the_wait_for_the_stamp;
pub mod gh_d54g_listennote_seam_carries_the_choices;
pub mod gh_d54g_stamp_route;
pub mod gh_d54g_stamp_route_an_owed_family_stands_on_the_stamp;
pub mod gh_d583_kern_route_a_late_stamp_carries_its_yes_values;
pub mod gh_d583_kern_route_error_and_unsure_carry_none;
pub mod gh_d583_kern_route_family_threshold_bars_the_families_alone;
pub mod gh_d583_raten_route_the_decide_window_counts_an_image_at_the_row;
pub mod gh_d58_judge_a_refused_view_keeps_the_judges_text;
pub mod gh_d58_judge_off_sends_no_ask;
pub mod gh_d58_judge_the_state_route_sends_to_decide_is_byte_identical;
#[path = "../support/history_judge.rs"]
pub mod history_judge;
#[path = "../support/history_view.rs"]
pub mod history_view;
#[path = "../mock_openai.rs"]
pub mod mock_openai;
#[path = "../support/organism_assistant.rs"]
pub mod organism_assistant;

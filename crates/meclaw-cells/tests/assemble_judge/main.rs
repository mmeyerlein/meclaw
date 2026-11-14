//! Test harness `assemble_judge` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

#[path = "../support/assemble_cell.rs"]
pub mod assemble_cell;
#[path = "../support/curator_hive.rs"]
pub mod curator_hive;
pub mod gh_d54g_stamp_route;
pub mod gh_d58_judge_a_refused_view_keeps_the_judges_text;
pub mod gh_d58_judge_off_sends_no_ask;
pub mod gh_d58_judge_the_state_route_sends_to_decide_is_byte_identical;
#[path = "../support/history_judge.rs"]
pub mod history_judge;
#[path = "../support/history_view.rs"]
pub mod history_view;

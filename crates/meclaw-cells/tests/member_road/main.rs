//! Test harness `member_road` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

#[path = "../support/gh929_file_road.rs"]
pub mod file_road;
pub mod gh1225_every_fact_of_an_agent_round_keeps_its_source;
pub mod gh1276_a_member_turn_passes_no_cold_code_cell;
pub mod gh929_every_budget_segment_fits_its_reserve;
pub mod gh946_a_round_change_close_fits_its_budget;
pub mod gh953_a_round_change_closes_after_the_last_answer;
pub mod gh954_two_first_turns_open_one_generation;
pub mod gh974_a_person_round_leaves_no_dead_letter;
pub mod gh981_a_run_reaches_the_brain_and_ends_at_the_app;
pub mod gh981_a_run_reads_the_library_in_the_members_round;
pub mod gh985_a_screen_grows_into_a_running_member;
pub mod gh995_every_run_tool_result_names_its_tool;
#[path = "../mock_openai.rs"]
pub mod mock_openai;
#[path = "../support/round_race.rs"]
pub mod race;
#[path = "../support/gh929_member_road.rs"]
pub mod road;
#[path = "../support/gh1276_runner_modes.rs"]
pub mod runner_modes;
#[path = "../support/gh929_tool_roads.rs"]
pub mod tool_roads;

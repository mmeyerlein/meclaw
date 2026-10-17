//! Test harness `round_1` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

pub mod fitness_bash;
pub mod fitness_code;
pub mod fitness_edit;
pub mod fitness_file;
pub mod fitness_harness;
pub mod gh1059_the_start_ask_is_the_rounds_ask;
pub mod gh1092_a_denied_credential_round_is_answered_at_once;
pub mod gh1252_a_foreign_box_leaves_the_round_alone;
pub mod gh342_the_shipped_judge_tool_reaches_the_wire;
pub mod gh372_an_async_only_round_still_answers;
pub mod gh425_the_build_tool_closes_its_own_round;
pub mod gh477_the_second_round_still_knows_the_question;
pub mod gh485_a_capped_build_says_it_ran_out_of_rounds;
pub mod gh501_a_repair_does_not_restart_the_round_budget;
pub mod gh517_a_grown_channel_is_told_the_round_it_speaks_in;
#[path = "../mock_openai.rs"]
pub mod mock_openai;
#[path = "../round_2/support_fitness.rs"]
pub mod support;

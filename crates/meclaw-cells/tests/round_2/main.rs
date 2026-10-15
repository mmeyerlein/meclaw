//! Test harness `round_2` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

pub mod gh679_the_judge_speaks_only_on_a_content_change;
pub mod gh694_the_judge_decides_only_what_is_large;
pub mod gh702_the_judge_sees_no_geometry;
pub mod gh856_judges_default_to_a_listed_model;
pub mod gh906_a_repo_round_trips_through_the_hive;
pub mod gh925_history_serves_only_what_the_round_was_present_for;
pub mod gh927_argus_judges_a_curator_metric;
pub mod gh930_a_steward_judge_cannot_order_a_revert;
pub mod gh933_the_close_pass_groups_by_round;
pub mod gh933_the_close_pass_never_crosses_a_round;
pub mod gh940_a_foreign_chat_never_speaks_in_the_round;
pub mod gh963_a_real_tool_round_reaches_the_presenter;
pub mod gh965_a_person_appears_by_the_names_released_to_the_round;
pub mod gh965_a_read_without_a_round_is_refused;
pub mod gh967_a_round_literal_takes_only_checked_names;
pub mod gh972_the_turn_round_reaches_the_curator;
pub mod gh976_a_digest_is_kept_with_its_round_and_shown_only_to_it;
pub mod gh976_a_result_is_kept_with_its_round;
#[path = "../gh927/harness.rs"]
pub mod harness;
#[path = "../gh927/meter.rs"]
pub mod meter;
#[path = "../mock_openai.rs"]
pub mod mock_openai;
#[path = "../gh927/mutator.rs"]
pub mod mutator;
#[path = "../support/mod.rs"]
pub mod support;
pub mod support_fitness;
pub mod w3_judge_closures;
pub mod w5_judged_cardinality;

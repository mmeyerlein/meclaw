//! Test harness `curator` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

pub mod curator_cells;
#[path = "../support/curator_hive.rs"]
pub mod curator_hive;
pub mod curator_policy;
pub mod curator_push;
pub mod gh1038_the_curator_knows_what_its_window_costs;
pub mod gh1040_the_curator_hands_the_package_to_memory;
pub mod gh1044_the_tool_recall_is_sized_by_the_model_package;
pub mod gh1079_sidecar_quotes_are_checked_word_for_word;
pub mod gh1085_the_curator_alone_cuts;
pub mod gh892_the_window_follows_its_role_and_the_models_trim;
pub mod gh896_a_session_starts_with_its_handover;
pub mod gh901_a_broken_file_is_never_written;
pub mod gh901_the_hook_register_parses_three_languages;
pub mod gh904_the_curator_clock_keeps_one_row;
pub mod gh916_a_pin_source_is_replaced;
pub mod gh926_a_curator_counts_its_own_ledger;
pub mod gh932_the_curator_pages_only_over_visible_rows;
pub mod gh935_the_identity_budget_splits_the_pack;
pub mod gh943_a_plan_per_round;
pub mod gh949_a_candidate_never_widens_its_round;
pub mod gh949_a_trigger_matches_on_word_boundaries;
pub mod gh949_an_app_reads_its_round_from_the_ledger;
pub mod gh949_the_push_carries_covered_candidates_only;
pub mod gh949_things_leave_as_thing_seen;
pub mod gh955_the_loser_of_two_first_claims_backs_off;
pub mod gh972_a_pin_carries_the_round_its_row_proves;
pub mod gh972_two_groups_of_one_source_replace_in_one_message;
#[path = "../mock_openai.rs"]
pub mod mock_openai;
#[path = "../support/organism_assistant.rs"]
pub mod organism_assistant;

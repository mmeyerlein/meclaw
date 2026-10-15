//! Test harness `builder` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

pub mod builder_bounds_agree_with_their_settings;
pub mod builder_dispatch_is_the_shipped_one;
pub mod builder_eyes_address_only_reads;
pub mod builder_eyes_read_the_colony;
pub mod builder_lib_adapts_both_ways;
pub mod builder_loop_bounds_are_declared;
pub mod builder_tool_vocabulary_is_closed;
pub mod builder_transcript_holds_the_round;
pub mod builder_unknown_tool_answers;
pub mod builder_weave_closes_the_round;
pub mod builder_weave_counts_the_rounds_it_closed;
pub mod builder_weave_rebuilds_a_wire_legal_thread;
pub mod gh425_the_builder_cannot_reach_the_mutation_door;
pub mod gh425_the_builder_classifies_positively;
pub mod gh440_the_builder_may_declare_a_template;
pub mod gh466_the_recipe_enum_matches_the_builder;
pub mod gh469_an_operator_drives_the_builder;

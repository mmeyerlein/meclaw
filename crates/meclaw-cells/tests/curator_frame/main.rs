//! Test harness `curator_frame` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

#[path = "../support/assemble_cell.rs"]
pub mod assemble_cell;
#[path = "../support/curator_hive.rs"]
pub mod curator_hive;
#[path = "../support/curator_target.rs"]
pub mod curator_target;
pub mod gh_d54g_stamp_notes;
pub mod gh_d58_frame_a_class_note_follows_the_stamp_class;
pub mod gh_d58_frame_a_class_with_pair_proposes_question_and_answer;
pub mod gh_d58_frame_a_class_without_tools_gets_none;
pub mod gh_d58_frame_a_set_override_of_class_notes_passes_the_door;
pub mod gh_d58_frame_a_target_that_holds_its_tree_gets_the_frame_last_and_frozen_per_turn;
pub mod gh_d58_frame_a_target_without_it_gets_none;
pub mod gh_d58_frame_any_target_with_frame_gets_time_and_place;
pub mod gh_d58_frame_every_shipped_composition_sets_one_place_at_both_cells;
pub mod gh_d58_frame_no_class_entry_means_no_note_and_no_pair;
pub mod gh_d58_frame_the_pair_of_todays_class_is_byte_identical;

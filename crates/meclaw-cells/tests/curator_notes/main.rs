//! Test harness `curator_notes` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

#[path = "../support/curator_hive.rs"]
pub mod curator_hive;
pub mod gh_d58_note_a_discarded_call_uses_up_nothing;
pub mod gh_d58_note_a_kept_speculation_receipts_what_it_showed;
pub mod gh_d58_note_a_once_note_left_out_for_space_stays_open;
pub mod gh_d58_note_a_once_note_never_rides_a_live_feed;
pub mod gh_d58_note_a_once_note_reaches_the_next_start_window_and_its_renewals;
pub mod gh_d58_note_a_once_note_stands_in_every_call_of_one_turn_and_never_again;
pub mod gh_d58_note_a_released_fallback_receipts_with_its_sends;
pub mod gh_d58_note_a_set_override_of_note_once_passes_the_door;
pub mod gh_d58_note_a_shown_note_with_a_receipt_answers_at_the_rim;
pub mod gh_d58_note_a_turn_note_behaves_as_before;
pub mod gh_d58_note_a_withdrawn_note_is_neither_shown_nor_receipted;
pub mod gh_d58_note_off_is_byte_identical;
pub mod gh_d58_note_two_writers_use_a_note_up_once;
pub mod gh_d58_note_withdrawing_a_used_note_changes_nothing;
#[path = "../support/note_once.rs"]
pub mod note_once;

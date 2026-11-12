//! Test harness `curator_errands` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

#[path = "../support/curator_bank.rs"]
pub mod curator_bank;
#[path = "../support/curator_errand.rs"]
pub mod curator_errand;
#[path = "../support/curator_hive.rs"]
pub mod curator_hive;
#[path = "../support/curator_profiles.rs"]
pub mod curator_profiles;
pub mod gh_d58_errand_a_failed_errand_leaves_its_outcome_in_the_window;
pub mod gh_d58_errand_a_late_answer_still_arrives;
pub mod gh_d58_errand_a_park_nobody_reads_again_is_swept_with_its_outcome;
pub mod gh_d58_errand_a_set_override_of_errand_outcome_passes_the_door;
pub mod gh_d58_errand_a_swept_park_has_its_outcome_written;
pub mod gh_d58_errand_an_answered_errand_leaves_no_outcome_line;
pub mod gh_d58_errand_an_errand_past_its_deadline_leaves_its_outcome;
pub mod gh_d58_errand_off_is_byte_identical;
pub mod gh_d58_errand_the_outcome_is_nobodys_words;
pub mod gh_d58_errand_the_outcome_is_written_once;
pub mod gh_d58_errand_the_outcome_survives_a_rebuild_unshrunk;

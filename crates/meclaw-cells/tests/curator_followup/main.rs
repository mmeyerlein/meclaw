//! Test harness `curator_followup` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

#[path = "../support/assistant_followup.rs"]
pub mod assistant_followup;
#[path = "../support/curator_hive.rs"]
pub mod curator_hive;
#[path = "../support/curator_target.rs"]
pub mod curator_target;
pub mod gh_d58_followup_a_call_during_an_open_text_session_knows_it_once;
pub mod gh_d58_followup_a_class_on_a_fact_passes_through_unnamed;
pub mod gh_d58_followup_a_concern_heard_without_a_session_reaches_the_core_at_once;
pub mod gh_d58_followup_a_fact_below_the_confidence_sends_nothing;
pub mod gh_d58_followup_an_open_concern_reaches_the_next_conversation_once;
pub mod gh_d58_followup_off_sends_nothing;
pub mod gh_d58_followup_the_curator_knows_no_wellbeing;
pub mod gh_d58_followup_the_first_receipt_closes_it_and_withdraws_the_copies;
pub mod gh_d58_followup_the_hearing_session_never_sees_its_concern;
pub mod gh_d58_followup_the_reasoning_core_gets_it_only_when_the_hearing_session_ends;
pub mod gh_d58_followup_the_same_words_in_a_new_session_replace_a_hanging_hold;

//! Test harness `duplex` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

#[path = "../support/duplex_cell.rs"]
pub mod duplex_cell;
pub mod gh1016_nothing_is_said_after_the_call_ended;
pub mod gh1055_a_ringing_call_finds_its_model_ready;
pub mod gh1276_a_speaker_turn_passes_no_cold_code_cell;
pub mod gh728_a_late_answer_after_a_turn_change_is_not_spoken;
pub mod gh896_a_duplex_call_is_renewed_at_its_limit;
pub mod gh979_a_spoken_turn_names_the_verified_caller;
pub mod gh979_an_app_cannot_set_the_speaker;
pub mod gh979_no_speaker_without_proof;
pub mod gh979_objects_owner_tools_on_a_real_turn;
pub mod gh979_speaker_stamp_from_bound_channel;
#[path = "../support/gh1276_runner_modes.rs"]
pub mod runner_modes;
#[path = "../support/speaker_road.rs"]
pub mod speaker_road;
pub mod voice_duplex_a_barge_in_cancels_the_open_speak;
pub mod voice_duplex_a_cascade_session_refuses_in_advise;
pub mod voice_duplex_a_correction_becomes_an_instruction;
pub mod voice_duplex_a_fact_becomes_commentary;
pub mod voice_duplex_a_hold_unmutes_and_a_release_mutes;
pub mod voice_duplex_a_section_travels_in_the_payload;
pub mod voice_duplex_a_speak_ends_when_the_model_goes_quiet;
pub mod voice_duplex_a_stale_delegation_is_closed_once;
pub mod voice_duplex_a_tick_closes_a_turn_on_a_silent_line;
pub mod voice_duplex_a_waiting_fragment_beats_a_due_tick;
pub mod voice_duplex_audio_passes_one_to_one;
pub mod voice_duplex_the_clock_runs_on_the_model_timeline;
pub mod voice_duplex_the_close_grace_is_the_operators_number;
pub mod voice_duplex_the_delegation_carries_the_open_turn;
pub mod voice_duplex_the_provider_gone_is_the_call_over;

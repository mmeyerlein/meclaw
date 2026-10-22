//! Test harness `curator_1` of `meclaw-cells`: one binary, one module per old file.
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
pub mod gh1089_kp_the_push_asks_with_names_not_nouns;
pub mod gh1225_a_day_longer_than_the_window_still_gets_cut;
pub mod gh1225_a_memory_lookup_is_not_said;
pub mod gh1225_a_parked_row_is_swept_on_a_rebuild;
pub mod gh1225_a_policy_row_is_replaced_in_one_step;
pub mod gh1225_a_quote_is_checked_at_its_own_source;
pub mod gh1225_a_refused_read_is_never_an_empty_window;
pub mod gh1225_a_replay_channel_stamps_turn_time;
pub mod gh1225_a_replayed_week_ages_by_turn_time;
pub mod gh1225_a_slot_path_holds_one_row;
pub mod gh1225_a_wall_row_without_its_block_is_cut_and_marked;
pub mod gh1225_duplicate_slots_count_once;
pub mod gh1225_every_sidecar_fact_is_checked_at_its_own_source;
pub mod gh1225_kc_a_a_refused_read_of_the_claims_keeps_the_episode;
pub mod gh1225_kc_a_a_stamped_ledger_reads_no_heal;
pub mod gh1225_kc_k9_the_readme_names_every_knob_and_nothing_dead;
pub mod gh1225_kc_p_every_hash_leg_refused_twice_is_one_cut;
pub mod gh1225_kc_w_a_refused_step_b_keeps_the_plan_of_the_parked_turn;
pub mod gh1225_kh_a_handover_row_is_replaced_in_one_step;
pub mod gh1225_kh_a_refused_note_write_is_written_by_the_next_close;
pub mod gh1225_kh_a_refused_read_of_handover_is_said_never_empty;
pub mod gh1225_kh_a_stale_parked_note_is_swept_once;
pub mod gh1225_kh_a_topic_outlives_sixty_quiet_turns;
pub mod gh1225_kh_history_reads_the_whole_wall;
pub mod gh1225_kh_the_note_a_close_parks_is_swept_and_said;
pub mod gh1225_kh_the_note_of_the_last_session_is_found_behind_other_handovers;
pub mod gh1225_kh_writer_a_late_wall_row_waits_for_the_next_close;
pub mod gh1225_kh_writer_a_refused_close_read_is_no_empty_session;
pub mod gh1225_kh_writer_a_written_store_error_mark_is_not_said_refused;
pub mod gh1225_kp_fix1_names_and_the_ledgers_word;
pub mod gh1225_kp_the_push_holds_what_the_ledger_holds;
pub mod gh1225_kr_a_failed_read_is_never_an_answer;
pub mod gh1225_kr_a_refused_read_of_intake_writes_nothing_blind;
pub mod gh1225_kr_a_refused_write_is_written_by_the_next_message;
pub mod gh1225_kr_a_refused_write_never_reissues_an_episode_id;
// `warm_python` is also a private module inside `curator_hive`; folding puts both in one crate
#[allow(clippy::duplicate_mod)]
#[path = "../support/warm_python.rs"]
pub mod warm_python;

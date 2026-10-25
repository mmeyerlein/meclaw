//! Test harness `memory` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

pub mod gh1019_a_schedule_fires_now_by_name;
pub mod gh1039_a_peer_is_known_by_name_and_a_forget_is_honoured;
pub mod gh1089_a_name_is_no_common_noun;
pub mod gh1225_a_topic_taken_up_is_visible_in_the_same_session;
pub mod gh1249_recur_finds_routines_and_sends_prewarm_candidates;
pub mod gh1292_a_fenced_verdict_is_read_like_a_bare_one;
pub mod gh1292_an_episode_written_before_the_credential_gets_its_vector;
pub mod gh1309_a_conversation_is_read_whole;
pub mod gh948_a_subject_query_obeys_the_round;
pub mod gh948_a_taken_alias_is_not_rebound;
pub mod gh948_an_alias_from_outside_canonicalizes_facts;
pub mod gh948_bounds_are_refused;
pub mod gh948_the_alias_check_uses_the_stores_normal_form;
pub mod gh973_dream_glue_never_rebinds_an_in_alias_binding;
pub mod gh973_dream_glue_orders_a_verdicts_chains;
#[path = "../support/memory_hive_subject.rs"]
pub mod support;

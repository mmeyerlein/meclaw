//! Test harness `watcher` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

pub mod gh1493_the_argus_door_reads_cycles_by_role_and_goal;
pub mod gh_d38_w1_a_stats_question_reaches_the_generation_curator;
pub mod gh_d38_w1_cold_pct_and_rebuild_cost_are_role_metrics;
pub mod gh_d38_w1_the_calendar_starts_the_first_cycle;
pub mod gh_d58_form_the_argus_reads_prefix_breaks_and_window_lost;
#[path = "../gh927/harness.rs"]
pub mod harness;
#[path = "../mock_openai.rs"]
pub mod mock_openai;

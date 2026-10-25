//! Test harness `misc` of `meclaw`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

pub mod cli;
pub mod gh121_root_lease;
pub mod gh1310_simple_agent_block_validates_strict;
pub mod gh423_apply_one_shot;
pub mod gh424_validate_lists_growths;
pub mod gh440_apply_registers_a_template;
pub mod gh47_a_batch_pipe_keeps_its_answers;
pub mod gh47_a_drain_is_not_a_wedge;
pub mod gh47_a_trip_does_not_drain;
pub mod gh586_validate_checks_ref_marker_overrides;
pub mod gh623_ask_prints_the_answer;
pub mod gh623_wrapper_binary_runs_ask;
pub mod gh640_ask_waits_out_a_sibling_dead_letter;
pub mod gh662_the_journal_is_the_default_sink;
pub mod gh811_a_colony_never_writes_into_a_library_it_does_not_own;
pub mod gh811_the_boot_keeps_what_the_colony_stands_on;
pub mod gh826_the_env_report_names_the_unused_and_the_missing_key;
pub mod gh97_sandbox_probe;
pub mod stdio_json_format;
pub mod u6_tracing_subscriber;

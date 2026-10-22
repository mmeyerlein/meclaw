//! Test harness `code` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

pub mod code_failure_modes;
pub mod gh1197_the_warm_code_cells_answer_like_cold;
pub mod gh1268_a_parked_sandbox_path_does_not_block_the_boot;
pub mod gh1276_an_idle_code_runner_child_stops_and_the_next_message_boots_a_new_one;
pub mod gh1276_every_code_cell_has_its_runner_mode;
pub mod gh1276_every_warm_code_cell_answers_like_cold;
pub mod gh144_embed_sandbox_egress;
pub mod gh309_coder_pipeline_verdict_parse;
pub mod gh343_the_small_cells_read_the_error_code_too;
pub mod gh446_model_authored_code_needs_a_capability;
pub mod gh673_a_reactivated_code_cell_can_be_swapped_away_again;
pub mod gh766_the_sandbox_is_said_out_loud;
pub mod gh907_a_code_cell_hands_attachments_to_its_script;
pub mod harness_adapter;
pub mod harness_cancel;
pub mod harness_cell_lifecycle;
pub mod harness_command_parse;
pub mod harness_emit;
pub mod harness_factory;
pub mod harness_fixture_dialect;
pub mod harness_io_loop;
pub mod harness_params;
#[path = "../support/gh1276_runner_modes.rs"]
pub mod runner_modes;
pub mod sandbox_harness_child;
pub mod sandbox_isolation;
pub mod sandbox_profile_parse;
pub mod w12_code_params_on_stdin;
#[path = "../support/warm_equivalence.rs"]
pub mod warm_equivalence;

//! Test harness `misc` of `meclaw-colony`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

pub mod an_unknown_diff_key_is_refused;
pub mod apps_rim_a_required_lane_is_wired_at_birth;
pub mod bootstrap_crash_recovery;
pub mod colony_hot_path_panic_free;
pub mod long_running_on_start_before_mailbox;
pub mod p1_read_messages_inbox;
pub mod r12_add_edges_subscope_matrix;
pub mod read_echo_tag;
pub mod w13_memory_hive_is_sealed;
pub mod writer_shutdown_with_a_full_queue_persists_every_op;

//! Test harness `paket` of `meclaw-colony`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

pub mod paket_1_env_substitution_mutation;
pub mod paket_1_mailbox_default_and_override;
pub mod paket_1_mailbox_size_behavioral;
pub mod paket_1_mailbox_size_mutation;
pub mod paket_1_message_timeout_boot_warn;
pub mod paket_2_swap;
pub mod paket_3_b_restart_backstop;
pub mod paket_3_c1_activity_aware_spawn;
pub mod paket_3_message_timeout_off;
pub mod paket_4_backpressure_disconnect;
pub mod paket_4_survivor_heal;
pub mod paket_6_failed_persist;
pub mod paket_6_recovery;

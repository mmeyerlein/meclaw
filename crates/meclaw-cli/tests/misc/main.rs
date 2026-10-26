//! Test harness `misc` of `meclaw-cli`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

pub mod gh1063_a_cold_deposit_is_refused_while_a_colony_holds_the_lease;
pub mod gh1326_validate_replays_apply_params;
pub mod gh17_timer_trigger_http;
pub mod gh325_the_registry_spawns_what_the_catalogue_lists;
pub mod gh423_apply_flag_surface;
pub mod gh617_a_lane_crosses_a_colony_boundary;
pub mod gh644_one_port_for_api_and_voice;
pub mod gh839_a_foreign_pointer_is_never_resolved;
pub mod gh921_a_signed_push_arrives_once;
pub mod import_export_matrix;
pub mod paket_1_validate_mailbox_size_zero;
pub mod phase_12_a_flags;
pub mod phase_12_a_http_lifecycle;
pub mod phase_12_a_validate;
pub mod phase_12_validate_hardening;
pub mod phase_13_5_a7_a8_validate_colony_json;
pub mod phase_13_5_lifecycle_3a_validate_regression;
pub mod phase_16_w1a_validate_strict;
pub mod phase_16_w1b_validate_unregistered;
pub mod proxy_hung_poll_health_liveness;
pub mod rescan_flag_parse;
pub mod slack_is_not_a_cell_type;
pub mod stdio_root_guard;
pub mod store_seed_header_validate;
pub mod u11_validate_file_base_path;
pub mod u7_env_flag;
pub mod u9_headless;
#[path = "../support/vault_grant.rs"]
pub mod vault_grant;
pub mod watchdog_trip_exit_code;

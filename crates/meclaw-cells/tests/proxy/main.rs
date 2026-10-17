//! Test harness `proxy` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

#[path = "../support/gh1059_grants.rs"]
pub mod gh1059_grants;
pub mod gh1059_slack_grant;
pub mod gh1059_telegram_grant;
pub mod gh1105_proxy_meclaw_client_secret_grant;
pub mod gh677_the_proxy_example_strips_the_prefix;
pub mod gh907_a_pdf_from_telegram_is_read_by_its_address;
pub mod proxy_db_round_trip;
pub mod proxy_factory_platform_dispatch;
pub mod proxy_factory_resume;
pub mod proxy_handle_event_state;
pub mod proxy_handle_inbound;
pub mod proxy_io_hang_deadline;
pub mod proxy_io_loop;
pub mod proxy_meclaw_lanes;
pub mod proxy_meclaw_mount;
pub mod proxy_meclaw_params;
pub mod proxy_meclaw_wire;
pub mod proxy_params_message;
pub mod proxy_params_parse;
pub mod proxy_platform_seam;
pub mod proxy_shutdown_sanity;
pub mod proxy_telegram_mock;
pub mod proxy_w7_tripwire;
pub mod slack_cell_handler;
pub mod slack_client_hang_deadline;
pub mod slack_idle_deadline;
pub mod slack_loop_guard;
pub mod slack_multi_bot;
pub mod slack_params_parse;
pub mod slack_reconnect;
pub mod slack_socket_roundtrip;
pub mod slack_wire_parse;

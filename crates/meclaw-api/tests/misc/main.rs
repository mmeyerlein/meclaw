//! Test harness `misc` of `meclaw-api`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

#[path = "../common/mod.rs"]
pub mod common;
pub mod gh175_hop_ingress;
pub mod gh383_api_is_pre_surface;
pub mod gh422_post_manifest;
pub mod gh644_colony_surfaces;
pub mod gh683_resolve_blob_is_capped;
pub mod gh867_ui_pages_carry_no_inline_script;
pub mod health_io_liveness;
pub mod health_only;
pub mod p1_messages_api;
pub mod p1_ui_message_detail;
pub mod p1_ui_messages;
pub mod phase_12_b_events_501;
pub mod phase_12_b_limit_clamp;
pub mod phase_12_b_messages_post;
pub mod phase_12_b_mutations_post;
pub mod phase_12_b_query_400;
pub mod phase_12_b_routes;
pub mod phase_12_d_dashboard;
pub mod phase_12_d_ui_dead_letters;
pub mod phase_12_d_ui_errors;
pub mod phase_12_d_ui_graph;
pub mod phase_12_d_ui_registry;
pub mod phase_12_d_ui_templates;
pub mod phase_12_d_ui_trace;
pub mod phase_12_x_multipart_upload;
pub mod phase_12_x_router_sig;
pub mod pre14_b5_ingress_422;
pub mod ttl_ingress;
pub mod w13_headers_ingress;

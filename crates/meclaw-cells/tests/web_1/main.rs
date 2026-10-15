//! Test harness `web_1` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

pub mod canvy2_client_geometry;
pub mod canvy2_drag_roundtrip;
pub mod canvy2_pipeline;
pub mod canvy2_position_migration;
pub mod fitness_web_fetch;
pub mod fitness_web_search;
pub mod gh1001_web_child_delta;
pub mod gh1001_web_render_scope;
pub mod gh1003_a_screen_class_is_a_route;
pub mod gh1004_viewport_event;
pub mod gh1005_a_bundle_may_answer_only_its_refusals;
pub mod gh1006_a_slow_viewer_is_reported;
pub mod gh1009_web_an_update_keeps_the_node;
pub mod gh1024_the_socket_speaks_deflate;
#[path = "../round_2/support_fitness.rs"]
pub mod support;
pub mod web_fetch_ssrf;
#[path = "../support/web_fixture.rs"]
pub mod web_fixture;

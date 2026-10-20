//! Test harness `web_2` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

#[path = "../support/gh1059_grants.rs"]
pub mod gh1059_grants;
pub mod gh1060_web_search_on_a_grant;
pub mod gh1106_a_webhook_takes_its_secret_from_a_grant;
pub mod gh1260_a_folded_web_call_reaches_its_runner;
pub mod gh163_web_cell_into_a_running_colony;
pub mod gh660_two_web_cells_one_mount_the_second_is_refused;
#[path = "../mock_openai.rs"]
pub mod mock_openai;
#[path = "../support/gh1060.rs"]
pub mod support;
pub mod web_cell_assets;
pub mod web_cell_define_and_pages;
pub mod web_cell_editable_events;
pub mod web_cell_liveview_join;
pub mod web_cell_object_crud;
pub mod web_cell_renders;
pub mod web_cell_schema_and_seed;
pub mod web_cell_semantic_events;
pub mod web_cell_serves;
pub mod web_template;
pub mod web_template_vision_set;

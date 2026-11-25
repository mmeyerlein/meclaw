//! Test harness `timer` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

#[path = "../support/composite_colony.rs"]
pub mod composite_colony;
#[path = "../support/curator_hive.rs"]
pub mod curator_hive;
pub mod fitness_timer;
pub mod gh1353_a_params_push_is_guarded_and_answered;
pub mod gh1353_the_params_road_from_the_shell_to_the_cell;
pub mod gh401_shipped_timer_schedules_deserialize;
pub mod gh695_the_seconds_run_and_the_timer_is_heard;
pub mod gh81_timer_tool_lane;
#[path = "../mock_openai.rs"]
pub mod mock_openai;
pub mod timer_fire_emission;
pub mod timer_frames_smoke;
pub mod timer_handle_add;
pub mod timer_handle_event_state;
pub mod timer_handle_modify_remove;
pub mod timer_io_loop;
pub mod timer_params_message;
pub mod timer_shutdown_sanity;
pub mod zh_corrupt_timer_cell_db_boots_failed;
pub mod zh_sleeping_timer_corrupt_cell_db_wakes_failed;

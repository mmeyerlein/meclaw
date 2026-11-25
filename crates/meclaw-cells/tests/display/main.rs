//! Test harness `display` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

#[path = "../support/display_colony.rs"]
pub mod display_colony;
pub mod gh1210_a_gesture_reaches_the_member_exactly_once;
pub mod gh1590_a_born_screen_serves_its_pages;
pub mod gh1597_a_member_without_a_screen_leaves_no_dead_letter;
pub mod gh1598_a_member_without_an_app_leaves_no_dead_letter;

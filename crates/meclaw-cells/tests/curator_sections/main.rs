//! Test harness `curator_sections` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

#[path = "../support/curator_hive.rs"]
pub mod curator_hive;
pub mod gh_d58_sections_a_cap_share_cuts_the_payload_to_the_window;
pub mod gh_d58_sections_a_mark_is_booked_as_registered;
pub mod gh_d58_sections_a_registered_section_leaves_on_its_lane;
pub mod gh_d58_sections_a_set_override_of_sections_passes_the_door;
pub mod gh_d58_sections_an_unregistered_section_is_dropped_and_named;
pub mod gh_d58_sections_the_curator_offers_only_its_own_menu_entries;
pub mod gh_d58_sections_the_receiving_hive_offers_and_checks_things;
#[path = "../support/objects_hive.rs"]
pub mod objects_hive;

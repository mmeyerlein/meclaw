//! Test harness `curator_feed` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

#[path = "../support/curator_feed.rs"]
pub mod curator_feed;
#[path = "../support/curator_hive.rs"]
pub mod curator_hive;
#[path = "../support/curator_live.rs"]
pub mod curator_live;
pub mod gh_d58_feed_a_fed_note_is_not_announced_again_by_the_next_renewal;
pub mod gh_d58_feed_a_lost_claim_stays_silent;
pub mod gh_d58_feed_a_renewal_keeps_its_moment_and_gains_a_mark;
pub mod gh_d58_feed_a_set_override_of_feed_in_passes_the_door;
pub mod gh_d58_feed_an_order_with_once_is_sent_once_per_turn;
pub mod gh_d58_feed_an_order_without_a_live_handle_is_booked_not_sent;
pub mod gh_d58_feed_an_ordered_feed_leaves_at_once_without_a_curate;
pub mod gh_d58_feed_at_turn_end_is_passed_on_untouched;
pub mod gh_d58_feed_off_takes_no_order;
pub mod gh_d58_feed_the_curators_own_feeds_take_the_same_road;
pub mod gh_d58_feed_the_mark_carries_what_the_orderer_gave;
#[path = "../support/note_once.rs"]
pub mod note_once;

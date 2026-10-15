//! Test harness `graph_space_2` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

pub mod gh950_every_library_tool_maps_to_one_op;
pub mod gh950_find_ranks_and_filters;
pub mod gh950_the_catalog_follows_its_sources;
pub mod gh951_a_document_edge_resolves_in_the_graph;
pub mod gh951_a_removal_breaks_only_extracted_edges;
pub mod gh951_a_thing_becomes_a_row;
pub mod gh951_an_alias_pushes_the_brief;
pub mod gh951_an_empty_round_sees_no_round;
pub mod gh951_graph_answers_keep_their_contract;
pub mod gh951_graph_state_is_order_free;
pub mod gh951_one_spelling_one_key;
pub mod gh951_the_brief_is_deterministic_and_round_bound;
pub mod gh951_why_is_the_owners;
#[path = "../support/librarian_hive.rs"]
pub mod librarian;
#[path = "../mock_openai.rs"]
pub mod mock_openai;
#[path = "../support/objects_hive.rs"]
pub mod objects_hive;
#[path = "../support/graph_space_pure.rs"]
pub mod pure;
#[path = "../support/gh929_member_road.rs"]
pub mod road;
#[path = "../support/graph_space_colony.rs"]
pub mod space;

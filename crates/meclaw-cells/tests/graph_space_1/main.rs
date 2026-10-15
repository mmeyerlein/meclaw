//! Test harness `graph_space_1` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

pub mod gh1085_the_producers_deliver_whole_or_by_budget;
pub mod gh905_a_scan_finds_what_a_tool_wrote;
pub mod gh945_a_rename_breaks_and_reports_the_edge;
pub mod gh945_a_source_change_fits_its_budget;
pub mod gh945_a_stale_pull_never_overwrites;
pub mod gh945_similar_edges_are_inferred;
pub mod gh945_the_graph_answers_without_waking_the_files;
pub mod gh950_a_model_finds_and_reads_a_function;
pub mod gh950_another_members_files_never_appear;
#[path = "../support/librarian_member.rs"]
pub mod librarian;
#[path = "../mock_openai.rs"]
pub mod mock_openai;
#[path = "../support/objects_hive.rs"]
pub mod objects_hive;
#[path = "../support/projection_hive.rs"]
pub mod projection_hive;
#[path = "../support/graph_space_pure.rs"]
pub mod pure;
#[path = "../support/graph_space_colony.rs"]
pub mod space;

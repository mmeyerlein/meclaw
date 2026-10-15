//! Test harness `misc_9` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

#[path = "../mock_openai.rs"]
pub mod mock_openai;
pub mod w4_extract_replaces;
pub mod w6_claim_aliases;
pub mod w9a_per_turn_colony;
pub mod x2_hive_fixture_drift;

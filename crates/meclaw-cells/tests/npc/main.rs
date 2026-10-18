//! Test harness `npc` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

#[path = "../mock_openai.rs"]
pub mod mock_openai;
pub mod npc_template;

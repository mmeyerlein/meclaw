//! Test harness `figure` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

pub mod figure_template;
pub mod figure_turns;
#[path = "../mock_openai.rs"]
pub mod mock_openai;

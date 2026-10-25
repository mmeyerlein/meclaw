//! Test harness `simple_agent` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

#[path = "../mock_openai.rs"]
pub mod mock_openai;
pub mod simple_agent_2_colony;
pub mod simple_agent_2_door;
pub mod simple_agent_2_tools;
pub mod simple_agent_loop;
pub mod simple_agent_template;

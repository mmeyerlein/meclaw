//! Test harness `phase` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

#[path = "../mock_openai.rs"]
pub mod mock_openai;
pub mod p15_dream_glue;
pub mod p15_recall_chain;
pub mod p15_recall_fusion_budget;
pub mod p15_recall_window;
pub mod phase_14a_tool_loop;
pub mod phase_14b_termination_guard;
pub mod phase_14c_lexical_rag;
pub mod phase_14d_multi_agent;
pub mod phase_16_w2d_llm_no_route;
pub mod phase_8_cell;
#[path = "../support_14b.rs"]
pub mod support;
// `topology_svg` is also a private module inside `support`; folding puts both in one crate
#[allow(clippy::duplicate_mod)]
#[path = "../topology_svg.rs"]
pub mod topology_svg;

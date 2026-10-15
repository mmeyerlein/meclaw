//! Test harness `llm` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

pub mod a_shipped_llm_backstop_outlasts_its_own_call;
pub mod gh1058_the_llm_cell_on_the_shared_module;
pub mod gh271_an_empty_llm_key_is_not_a_bearer;
pub mod gh47_an_async_llm_cell_survives_the_eof;
pub mod gh845_the_llm_narrows_tools_per_request;
pub mod gh858_every_llm_cell_states_its_need;
pub mod gh87_llm_attachment_consumption;
pub mod gh886_every_llm_cell_declares_hop_model;
pub mod gh890_every_llm_cell_declares_what_output_writes;
pub mod gh94_responses_attachments;
pub mod llm_attribution_headers;
pub mod llm_chat_completions_wire_regression;
pub mod llm_in_body_error;
pub mod llm_latency_instrumentation;
pub mod llm_params_message;
pub mod llm_reasoning_passthrough;
pub mod llm_registry_template;
pub mod llm_responses_smoke;
pub mod llm_seed;
pub mod llm_subscription_lane;
pub mod llm_token_broker;
#[path = "../misc_8/mock_oauth.rs"]
pub mod mock_oauth;
#[path = "../mock_openai.rs"]
pub mod mock_openai;
pub mod mock_responses;

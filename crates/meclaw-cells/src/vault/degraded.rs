//! The vault that has no usable `cell.db`.
//!
//! Same reasoning as the store's degraded cell, with one difference that
//! matters here: a vault whose storage is unusable must look *locked*, not
//! broken in some undefined way. Everything it answers says so. There is no
//! in-memory substitute — a vault that accepts a secret into a database that
//! vanishes at the next wake is worse than one that refuses.

use meclaw_colony::StatelessCell;
use meclaw_core::serde_json::json;
use meclaw_core::{Body, CellOutput, Message, OutputSink};

/// A vault cell without storage. Answers every message with `vault_error`.
pub struct DegradedVaultCell {
    reason: String,
}

impl DegradedVaultCell {
    /// Build it from the wake-time failure reason.
    #[doc(hidden)]
    pub fn new(reason: String) -> Self {
        Self { reason }
    }
}

fn tool_call_id(msg: &Message) -> String {
    let Body::Inline(v) = &msg.body else {
        return String::new();
    };
    v.get("messages")
        .and_then(|m| m.as_array())
        .and_then(|turns| turns.first())
        .and_then(|turn| turn.get("id"))
        .and_then(|id| id.as_str())
        .unwrap_or_default()
        .to_string()
}

#[allow(clippy::manual_async_fn)]
impl StatelessCell for DegradedVaultCell {
    fn handle<'a>(
        &'a self,
        msg: Message,
        sink: &'a OutputSink,
    ) -> impl std::future::Future<Output = ()> + Send + 'a {
        async move {
            let target = msg.reply_to.clone().unwrap_or_else(|| msg.target.clone());
            let body = json!({
                "header": {
                    "finish_reason": "error",
                    "error_code": "vault_error",
                    "duration_ms": 0,
                },
                "messages": [{
                    "origin": "tool",
                    "type": "tool_result",
                    "text": format!(
                        "vault: unusable and therefore LOCKED — {}. No secret can be stored or \
                         used until the cell has its database back.",
                        self.reason
                    ),
                    "id": tool_call_id(&msg),
                }]
            });
            let _ = sink
                .push(CellOutput {
                    target,
                    content: body,
                })
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tool_call_id_travels_into_the_error_so_a_loop_can_correlate_it() {
        let msg = Message {
            id: uuid_v7(),
            trace_id: uuid_v7(),
            parent_message_id: None,
            correlation_id: None,
            target: meclaw_core::Path::new("/main/access/vault"),
            reply_to: None,
            ttl: 10,
            headers: meclaw_core::Headers::new(),
            body: Body::Inline(json!({
                "messages": [{"origin":"tool","type":"tool_call","text":"{}","id":"c1"}]
            })),
            created_at: 0,
        };
        assert_eq!(tool_call_id(&msg), "c1");
    }

    #[test]
    fn a_body_without_an_id_yields_an_empty_one_rather_than_panicking() {
        let msg = Message {
            id: uuid_v7(),
            trace_id: uuid_v7(),
            parent_message_id: None,
            correlation_id: None,
            target: meclaw_core::Path::new("/main/access/vault"),
            reply_to: None,
            ttl: 10,
            headers: meclaw_core::Headers::new(),
            body: Body::Inline(json!({"messages": []})),
            created_at: 0,
        };
        assert_eq!(tool_call_id(&msg), "");
    }

    fn uuid_v7() -> meclaw_core::Uuid {
        meclaw_core::Uuid::now_v7()
    }
}

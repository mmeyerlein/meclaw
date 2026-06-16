//! Thin convenience wrapper around `meclaw_core::MessageBuilder` for tests.
//!
//! Adds ergonomic setters: `.target("/a")` (str instead of Path), `.body_json(...)`,
//! `.with_inline_content(json!(...))`, `.with_inline_messages(vec![...])`,
//! `.with_header(key, value)`. Production code should use
//! `meclaw_core::MessageBuilder` directly.

use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder as CoreBuilder, Path};

pub struct MessageBuilder {
    inner: CoreBuilder,
    pending_headers: Map<String, Value>,
}

impl MessageBuilder {
    /// Create with a target path (str-convenient).
    pub fn new(target: &str) -> Self {
        Self {
            inner: CoreBuilder::new(Path::new(target)),
            pending_headers: Map::new(),
        }
    }

    /// Set body = `Body::Inline(value)`.
    pub fn with_inline_content(mut self, value: Value) -> Self {
        self.inner = self.inner.body(Body::Inline(value));
        self
    }

    /// Set body = `Body::Inline({"messages": msgs})` (UBF-style).
    pub fn with_inline_messages(mut self, msgs: Vec<Value>) -> Self {
        self.inner = self.inner.body(Body::Inline(json!({"messages": msgs})));
        self
    }

    /// Add a single header key/value pair to the persistent `context`
    /// compartment of the envelope.
    ///
    /// Slice 2: `with_header` defaults to the `context` compartment (its only
    /// existing call sites set correlation keys like `session_id`). A `hop`
    /// counterpart is added when a test needs it.
    ///
    /// Multiple calls accumulate; later calls with the same key overwrite that key.
    pub fn with_header(mut self, key: &str, value: Value) -> Self {
        self.pending_headers.insert(key.to_string(), value);
        self
    }

    /// Set ttl (test-driven cascade exercises).
    pub fn with_ttl(mut self, ttl: u32) -> Self {
        self.inner = self.inner.ttl(ttl);
        self
    }

    /// Set reply_to (test-driven cascade exercises).
    pub fn with_reply_to(mut self, path: &str) -> Self {
        self.inner = self.inner.reply_to(Path::new(path));
        self
    }

    pub fn build(self) -> Message {
        let mut inner = self.inner;
        if !self.pending_headers.is_empty() {
            inner = inner.context(self.pending_headers);
        }
        inner.build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    #[test]
    fn builder_constructs_message_with_inline_content() {
        let m = MessageBuilder::new("/a/b")
            .with_inline_content(json!({"x": 1}))
            .build();
        assert_eq!(m.target.as_str(), "/a/b");
        assert!(matches!(&m.body, Body::Inline(v) if v == &json!({"x": 1})));
    }

    #[test]
    fn builder_with_reply_to_and_ttl() {
        let m = MessageBuilder::new("/a")
            .with_reply_to("/back")
            .with_ttl(3)
            .build();
        assert_eq!(m.reply_to.as_ref().map(|p| p.as_str()), Some("/back"));
        assert_eq!(m.ttl, 3);
    }

    #[test]
    fn builder_with_inline_messages_produces_ubf_body() {
        let m = MessageBuilder::new("/a")
            .with_inline_messages(vec![
                json!({"origin": "user", "type": "text", "text": "hi"}),
            ])
            .build();
        let body = match m.body {
            Body::Inline(v) => v,
            _ => panic!("expected Inline body"),
        };
        assert_eq!(body["messages"][0]["origin"], json!("user"));
    }

    #[test]
    fn builder_with_header_sets_envelope_header() {
        let m = MessageBuilder::new("/a")
            .with_header("session_id", json!("s1"))
            .build();
        assert_eq!(m.headers.context.get("session_id"), Some(&json!("s1")));
    }
}

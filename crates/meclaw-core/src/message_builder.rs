//! Production-code message builder. Sets id/trace_id/created_at automatically;
//! target is required; ttl defaults to MESSAGE_DEFAULT_TTL; rest optional.
//!
//! `meclaw-testing::MessageBuilder` is a thin convenience wrapper around this.

use crate::body::Body;
use crate::headers::Headers;
use crate::message::{MESSAGE_DEFAULT_TTL, Message, now_unix_seconds};
use crate::path::Path;
use serde_json::{Map, Value};
use uuid::Uuid;

pub struct MessageBuilder {
    target: Path,
    trace_id: Option<Uuid>,
    parent_message_id: Option<Uuid>,
    correlation_id: Option<Uuid>,
    reply_to: Option<Path>,
    ttl: u32,
    headers: Headers,
    body: Body,
}

impl MessageBuilder {
    pub fn new(target: Path) -> Self {
        Self {
            target,
            trace_id: None,
            parent_message_id: None,
            correlation_id: None,
            reply_to: None,
            ttl: MESSAGE_DEFAULT_TTL,
            headers: Headers::new(),
            body: Body::Inline(Value::Null),
        }
    }

    pub fn trace_id(mut self, id: Uuid) -> Self {
        self.trace_id = Some(id);
        self
    }

    pub fn parent_message_id(mut self, id: Uuid) -> Self {
        self.parent_message_id = Some(id);
        self
    }

    /// Set `parent_message_id` from an explicit `Option<Uuid>`. Used by
    /// callers that forward a possibly-`None` parent (e.g. cells emitting
    /// origin messages via `OriginSink`). The existing
    /// `parent_message_id(Uuid)` setter remains for callers with a concrete
    /// parent id.
    pub fn parent_message_id_opt(mut self, id: Option<Uuid>) -> Self {
        self.parent_message_id = id;
        self
    }

    pub fn correlation_id(mut self, id: Uuid) -> Self {
        self.correlation_id = Some(id);
        self
    }

    /// Set `correlation_id` from an explicit `Option<Uuid>`. Used by callers
    /// that forward a possibly-`None` correlation_id unchanged, e.g. transparent
    /// hive-transit routing. The existing `correlation_id(Uuid)` setter remains
    /// for callers with a concrete id.
    pub fn correlation_id_opt(mut self, id: Option<Uuid>) -> Self {
        self.correlation_id = id;
        self
    }

    pub fn reply_to(mut self, path: Path) -> Self {
        self.reply_to = Some(path);
        self
    }

    /// Set `reply_to` from an explicit `Option<Path>`. Used by callers that
    /// forward a possibly-`None` reply_to unchanged, e.g. transparent
    /// hive-transit routing. The existing `reply_to(Path)` setter remains
    /// for callers with a concrete path.
    pub fn reply_to_opt(mut self, path: Option<Path>) -> Self {
        self.reply_to = path;
        self
    }

    pub fn ttl(mut self, ttl: u32) -> Self {
        self.ttl = ttl;
        self
    }

    pub fn headers(mut self, headers: Headers) -> Self {
        self.headers = headers;
        self
    }

    /// Set only the persistent `context` compartment.
    pub fn context(mut self, context: Map<String, Value>) -> Self {
        self.headers.context = context;
        self
    }

    /// Set only the single-hop `hop` compartment.
    pub fn hop(mut self, hop: Map<String, Value>) -> Self {
        self.headers.hop = hop;
        self
    }

    pub fn body(mut self, body: Body) -> Self {
        self.body = body;
        self
    }

    pub fn build(self) -> Message {
        let id = Uuid::now_v7();
        let trace_id = self.trace_id.unwrap_or(id);
        Message {
            id,
            trace_id,
            parent_message_id: self.parent_message_id,
            correlation_id: self.correlation_id,
            target: self.target,
            reply_to: self.reply_to,
            ttl: self.ttl,
            headers: self.headers,
            body: self.body,
            created_at: now_unix_seconds(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_defaults_trace_id_to_id_when_unset() {
        let m = MessageBuilder::new(Path::new("/a")).build();
        assert_eq!(m.id, m.trace_id, "source message: trace_id defaults to id");
        assert_eq!(m.ttl, MESSAGE_DEFAULT_TTL);
        assert_eq!(m.parent_message_id, None);
        assert_eq!(m.reply_to, None);
    }

    #[test]
    fn build_copies_trace_id_when_set() {
        let parent_trace = Uuid::now_v7();
        let m = MessageBuilder::new(Path::new("/a"))
            .trace_id(parent_trace)
            .parent_message_id(Uuid::now_v7())
            .build();
        assert_eq!(
            m.trace_id, parent_trace,
            "follow-up message keeps parent trace_id"
        );
        assert_ne!(m.id, parent_trace, "new id is generated");
    }

    #[test]
    fn build_respects_ttl_override() {
        let m = MessageBuilder::new(Path::new("/a")).ttl(1).build();
        assert_eq!(m.ttl, 1);
    }

    #[test]
    fn parent_message_id_opt_accepts_some() {
        let parent = Uuid::now_v7();
        let m = MessageBuilder::new(Path::new("/a"))
            .parent_message_id_opt(Some(parent))
            .build();
        assert_eq!(m.parent_message_id, Some(parent));
    }

    #[test]
    fn parent_message_id_opt_accepts_none() {
        let m = MessageBuilder::new(Path::new("/a"))
            .parent_message_id_opt(None)
            .build();
        assert_eq!(m.parent_message_id, None);
    }

    #[test]
    fn reply_to_opt_some_sets_field() {
        let m = MessageBuilder::new(Path::new("/a"))
            .reply_to_opt(Some(Path::new("/b")))
            .build();
        assert_eq!(m.reply_to, Some(Path::new("/b")));
    }

    #[test]
    fn reply_to_opt_none_leaves_none() {
        let m = MessageBuilder::new(Path::new("/a"))
            .reply_to_opt(None)
            .build();
        assert_eq!(m.reply_to, None);
    }

    #[test]
    fn correlation_id_opt_some_sets_field() {
        let id = Uuid::now_v7();
        let m = MessageBuilder::new(Path::new("/a"))
            .correlation_id_opt(Some(id))
            .build();
        assert_eq!(m.correlation_id, Some(id));
    }

    #[test]
    fn correlation_id_opt_none_leaves_none() {
        let m = MessageBuilder::new(Path::new("/a"))
            .correlation_id_opt(None)
            .build();
        assert_eq!(m.correlation_id, None);
    }
}

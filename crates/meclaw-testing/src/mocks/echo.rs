//! Echo mock: stream-fortpflanzend per spec § "Cell-Bauarten bzgl. messages[]".
//! Reads inbound `messages[]` (if present), appends an assistant-text-turn,
//! optionally emits a `content.header`-block via `.with_emitted_header(k, v)`.

use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Cell, CellOutput, Message, OutputSink, Path};
use tokio::sync::mpsc;

pub struct EchoMockCell {
    own_path: Path,
    echo_to: Option<Path>,
    tap_to: Option<mpsc::Sender<Path>>,
    emitted_headers: Map<String, Value>,
}

impl EchoMockCell {
    /// Create a new echo cell at `own_path`.
    pub fn new(own_path: Path) -> Self {
        Self {
            own_path,
            echo_to: None,
            tap_to: None,
            emitted_headers: Map::new(),
        }
    }

    /// Forward output to `target`.
    pub fn echo_to(mut self, target: Path) -> Self {
        self.echo_to = Some(target);
        self
    }

    /// Tap own path into an observation channel for test assertions.
    pub fn tap_to(mut self, tap: mpsc::Sender<Path>) -> Self {
        self.tap_to = Some(tap);
        self
    }

    /// Accumulate a `content.header` entry. If at least one entry is set,
    /// the emitted content will contain a `"header"` object; otherwise the
    /// `"header"` key is omitted entirely (spec: no empty header block).
    pub fn with_emitted_header(mut self, key: &str, value: Value) -> Self {
        self.emitted_headers.insert(key.to_string(), value);
        self
    }
}

impl Cell for EchoMockCell {
    #[allow(clippy::manual_async_fn)]
    fn handle(
        &mut self,
        msg: Message,
        sink: &OutputSink,
    ) -> impl std::future::Future<Output = ()> + Send {
        let own_path = self.own_path.clone();
        let echo_to = self.echo_to.clone();
        let tap_to = self.tap_to.clone();
        let emitted_headers = self.emitted_headers.clone();
        let sink = sink.clone();
        async move {
            if let Some(tap) = &tap_to {
                let _ = tap.send(own_path.clone()).await;
            }
            let Some(target) = echo_to else { return };

            // Robust against non-UBF inputs (source messages with Null body).
            let input_messages: Vec<Value> = match &msg.body {
                Body::Inline(Value::Object(map)) => match map.get("messages") {
                    Some(Value::Array(arr)) => arr.clone(),
                    _ => Vec::new(),
                },
                _ => Vec::new(),
            };
            let mut out_messages = input_messages;
            out_messages.push(json!({
                "origin": "assistant",
                "type": "text",
                "text": format!("echo from {}", own_path.as_str()),
            }));

            let mut content = Map::new();
            content.insert("messages".into(), Value::Array(out_messages));
            if !emitted_headers.is_empty() {
                content.insert("header".into(), Value::Object(emitted_headers));
            }

            let _ = sink
                .push(CellOutput {
                    target,
                    content: Value::Object(content),
                })
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;
    use meclaw_core::{CellEmission, OutputSink, Path};
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn echo_taps_own_path_and_forwards() {
        use meclaw_core::{MessageBuilder, Uuid};
        let (tap_tx, mut tap_rx) = mpsc::channel(4);
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(4);
        let sink = OutputSink::new(
            out_tx,
            Path::new("/a"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            10,
            meclaw_core::Headers::new(),
            None,
        );
        let mut cell = EchoMockCell::new(Path::new("/a"))
            .echo_to(Path::new("/b"))
            .tap_to(tap_tx);
        let msg = MessageBuilder::new(Path::new("/a")).build();
        cell.handle(msg, &sink).await;
        assert_eq!(tap_rx.recv().await.unwrap().as_str(), "/a");
        let em = out_rx.recv().await.unwrap();
        assert_eq!(em.sender_path.as_str(), "/a");
        assert_eq!(em.target.as_str(), "/b");
        let msgs = em.content["messages"]
            .as_array()
            .expect("UBF messages array");
        assert_eq!(msgs.len(), 1, "no input messages → only own assistant-turn");
    }

    #[tokio::test]
    async fn echo_terminal_does_not_emit() {
        use meclaw_core::{MessageBuilder, Uuid};

        let (tap_tx, mut tap_rx) = mpsc::channel(4);
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(4);
        let sink = OutputSink::new(
            out_tx,
            Path::new("/sink"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            10,
            meclaw_core::Headers::new(),
            None,
        );
        let mut cell = EchoMockCell::new(Path::new("/sink")).tap_to(tap_tx);
        let msg = MessageBuilder::new(Path::new("/sink")).build();
        cell.handle(msg, &sink).await;
        assert_eq!(tap_rx.recv().await.unwrap().as_str(), "/sink");
        assert!(out_rx.try_recv().is_err(), "terminal echo must not emit");
    }

    #[tokio::test]
    async fn echo_streams_input_messages_and_appends_assistant_turn() {
        use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};

        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(4);
        let sink = OutputSink::new(
            out_tx,
            Path::new("/a"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            10,
            meclaw_core::Headers::new(),
            None,
        );
        let mut cell = EchoMockCell::new(Path::new("/a"))
            .echo_to(Path::new("/b"))
            .with_emitted_header("forwarded_by", json!("/a"));
        let input = MessageBuilder::new(Path::new("/a"))
            .body(Body::Inline(json!({
                "messages": [{"origin": "user", "type": "text", "text": "hi"}]
            })))
            .build();
        cell.handle(input, &sink).await;

        let em = out_rx.recv().await.unwrap();
        assert_eq!(em.target.as_str(), "/b");
        assert_eq!(em.content["header"]["forwarded_by"], json!("/a"));
        let msgs = em.content["messages"].as_array().expect("messages array");
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["origin"], json!("user"));
        assert_eq!(msgs[1]["origin"], json!("assistant"));
        assert_eq!(msgs[1]["type"], json!("text"));
    }

    #[tokio::test]
    async fn echo_without_emitted_header_omits_header_block() {
        use meclaw_core::{CellEmission, MessageBuilder, OutputSink, Path, Uuid};
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(4);
        let sink = OutputSink::new(
            out_tx,
            Path::new("/a"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            10,
            meclaw_core::Headers::new(),
            None,
        );
        let mut cell = EchoMockCell::new(Path::new("/a")).echo_to(Path::new("/b"));
        let input = MessageBuilder::new(Path::new("/a")).build();
        cell.handle(input, &sink).await;
        let em = out_rx.recv().await.unwrap();
        assert!(
            em.content.get("header").is_none(),
            "no setter call → no header block"
        );
    }
}

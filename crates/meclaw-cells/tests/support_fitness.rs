//! Track T (#104) — shared support for the per-tool fitness batteries.
//!
//! The rig spawns ONE tool cell through its production `CellFactory` (the same
//! construction the bootstrap runs) and talks to it over its real mailbox.
//! Emissions come back as raw `CellEmission`s, so a battery can pin the
//! `header` section exactly as the cell wrote it — before any edge or
//! delivery-stage rewriting.
//!
//! Pattern source: `sandbox_isolation.rs::run_bash` (factory spawn) and
//! `store_integration.rs` (tool_call round trips). No ad-hoc infrastructure:
//! everything here delegates to `meclaw-colony` factory machinery and
//! `meclaw-core` builders.
#![allow(dead_code)]

use meclaw_colony::{CellFactory, SpawnedCellKind};
use meclaw_core::serde_json::Value;
use meclaw_core::{Body, CellEmission, Message, MessageBuilder, Path};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// One spawned tool cell under test: its mailbox, its emission stream, and the
/// colony-inbox receiver kept alive so substrate sends cannot error.
pub struct ToolRig {
    pub tx: mpsc::Sender<Message>,
    pub rx: mpsc::Receiver<CellEmission>,
    _colony_inbox: mpsc::Receiver<meclaw_colony::ColonyMsg>,
    path: &'static str,
}

impl ToolRig {
    /// Spawns `factory` at `path` with `params` through the production
    /// `spawn_cell` path. Panics on spawn errors — a battery must not start
    /// against a half-built cell.
    pub fn spawn(factory: Arc<dyn CellFactory>, path: &'static str, params: Value) -> Self {
        let (otx, orx) = mpsc::channel::<CellEmission>(64);
        let (itx, irx) = mpsc::channel::<meclaw_colony::ColonyMsg>(64);
        let spawned = factory
            .spawn_cell(
                Path::new(path),
                params,
                otx,
                std::path::PathBuf::new(),
                meclaw_colony::ContractView::default(),
                itx,
                None,
                0,
                None,
                None,
                64,
            )
            .expect("fitness rig: spawn_cell must succeed");
        let tx = match spawned {
            SpawnedCellKind::Active { sender, .. } => sender,
            SpawnedCellKind::Dormant { .. } => {
                panic!("fitness rig: expected an Active (stateless) cell")
            }
        };
        Self {
            tx,
            rx: orx,
            _colony_inbox: irx,
            path,
        }
    }

    /// Sends one `tool_call` turn whose `text` is the serialized `args` and
    /// awaits the single emission (30 s failure marker, cargo-load-robust).
    pub async fn call(&mut self, args: Value, id: &str) -> CellEmission {
        self.send_raw(tool_call_msg(self.path, &args.to_string(), id))
            .await
    }

    /// Sends one `tool_call` turn with a RAW (possibly non-JSON) `text`.
    pub async fn call_raw_text(&mut self, text: &str, id: &str) -> CellEmission {
        self.send_raw(tool_call_msg(self.path, text, id)).await
    }

    async fn send_raw(&mut self, msg: Message) -> CellEmission {
        self.tx.send(msg).await.expect("cell mailbox open");
        tokio::time::timeout(Duration::from_secs(30), self.rx.recv())
            .await
            .expect("no emission within 30s")
            .expect("emission channel closed")
    }
}

/// A `tool_call` probe in the shared tool-cell convention (`store`/`bash`):
/// one turn, args as `text`, correlation `id`, `reply_to` = `/sink`.
pub fn tool_call_msg(target: &str, text: &str, id: &str) -> Message {
    MessageBuilder::new(Path::new(target))
        .reply_to(Path::new("/sink"))
        .body(Body::Inline(meclaw_core::serde_json::json!({
            "messages": [{
                "origin": "assistant", "type": "tool_call",
                "text": text, "id": id
            }]
        })))
        .build()
}

/// The `text` of the first turn of an emission.
pub fn text_of(em: &CellEmission) -> String {
    em.content["messages"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// A `header` value of an emission (the cell's own hop output).
pub fn header_of<'a>(em: &'a CellEmission, key: &str) -> &'a Value {
    &em.content["header"][key]
}

/// Asserts the emission is a NORMAL tool_result: no `finish_reason: "error"`,
/// no `error_code`, and the turn carries the correlation id.
pub fn assert_normal_result(em: &CellEmission, id: &str) {
    assert!(
        header_of(em, "error_code").is_null(),
        "expected a normal tool_result, got error_code {:?} (text: {:?})",
        header_of(em, "error_code"),
        text_of(em)
    );
    assert_ne!(
        header_of(em, "finish_reason").as_str(),
        Some("error"),
        "expected a normal tool_result, got finish_reason error (text: {:?})",
        text_of(em)
    );
    assert_eq!(
        em.content["messages"][0]["id"].as_str(),
        Some(id),
        "the tool_call id must survive into the tool_result"
    );
    assert_eq!(em.content["messages"][0]["type"], "tool_result");
    assert_eq!(em.content["messages"][0]["origin"], "tool");
}

/// Asserts the emission is a typed ERROR with exactly this `error_code`.
pub fn assert_error(em: &CellEmission, code: &str) {
    assert_eq!(
        header_of(em, "finish_reason").as_str(),
        Some("error"),
        "expected finish_reason error, got {:?} (text: {:?})",
        header_of(em, "finish_reason"),
        text_of(em)
    );
    assert_eq!(
        header_of(em, "error_code").as_str(),
        Some(code),
        "expected error_code {code}, got {:?} (text: {:?})",
        header_of(em, "error_code"),
        text_of(em)
    );
}

//! `meclaw-core`: actor-substrate primitives.
//!
//! Internal crate — the public contract is the HTTP API and the template DSL;
//! no SemVer guarantee on Rust items. See README.md § Stability.

pub mod blob;
mod body;
mod cell;
mod contract;
mod handle;
mod headers;
mod message;
mod message_builder;
pub mod origin_sink;
mod output;
mod path;
mod schema;

pub use blob::BlobRef;
pub use body::Body;
pub use cell::Cell;
pub use contract::{
    CompiledConsumes, CompiledEmits, ConsumeSpec, ConsumesBlock, EmitsBlock, IngressBlock,
    SettingSpec, TransferBounds, TransferPolicy, WriteSurface, validate_consumes, validate_emits,
};
pub use handle::ActorHandle;
pub use headers::Headers;
pub use message::MESSAGE_DEFAULT_TTL;
pub use message::Message;
pub use message_builder::MessageBuilder;
pub use origin_sink::OriginSink;
pub use output::{CellEmission, CellOutput, OutputSink};
pub use path::Path;
pub use schema::{init_validator, validate_ubf_body};
pub use serde_json;
pub use serde_json::Value as JsonValue;
pub use uuid::Uuid;

#[cfg(test)]
mod cargo_deps_smoke {
    #[test]
    fn uuid_v7_is_available() {
        let _id: uuid::Uuid = uuid::Uuid::now_v7();
    }

    #[test]
    fn serde_json_is_available() {
        let _v: serde_json::Value = serde_json::Value::Null;
    }
}

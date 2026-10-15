//! GH #1176 — a slot rebuild that cannot read a hive's seal has failed.
//!
//! The delivery filter is rebuilt from the hives' `config.json` after every
//! topology change. A config that does not parse (or a read that errors, EIO)
//! used to count as "this hive declares nothing": the rebuild succeeded and
//! the boundary opened without that hive's seal, so an outside message reached
//! an address the hive never declared. Now such a rebuild counts as failed,
//! like one that timed out: every hive stays sealed with no declared address
//! (`hive_boundary`) and the log says `ingress closed: slot rebuild could not
//! read a hive config` (ERROR) once. When the file is readable again, the next
//! pass rebuilds, the log says `ingress reopened: slot rebuild succeeded`
//! (WARN), and the declared port is an address again.
//!
//! A missing `config.json` is not a read error: such a scope declares
//! nothing, as before. The test records the log from a process-wide
//! subscriber (nextest runs each test in its own process).

use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::json;
use meclaw_core::{Body, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// Failure marker (AGENTS.md: 30 s convention).
const MARKER: Duration = Duration::from_secs(30);

/// How long a refused message is given to arrive anyway.
const NOT_DELIVERED: Duration = Duration::from_millis(1_500);

const CLOSED: &str = "ingress closed: slot rebuild could not read a hive config";
const REOPENED: &str = "ingress reopened: slot rebuild succeeded";

/// `/vault` sealed, its one cell `secret` declared as a port.
const VAULT_CONFIG: &str =
    r#"{"cell":{"type":"hive"},"params":{"ports":["secret"],"graph":{"edges":[]}}}"#;

/// How many lines at `level` say `line` about this test's colony (the
/// recorder is process-wide and the tests of the binary run concurrently).
fn count(root: &std::path::Path, level: tracing::Level, line: &str) -> usize {
    let root = format!("root={}", root.display());
    let message = format!("message={line}");
    meclaw_testing::log_recorder::global()
        .lines(level, &[&root, &message])
        .len()
}

fn echo_factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "echo".to_string(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    )]
}

fn echo_registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    r.insert(
        "echo".into(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    );
    r
}

fn from_outside(target: &str) -> meclaw_core::Message {
    MessageBuilder::new(Path::new(target))
        .body(Body::Inline(
            json!({"messages":[{"origin":"user","type":"text","text":"ping"}]}),
        ))
        .ttl(16)
        .build()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unparsable_hive_config_keeps_outside_ingress_refused_until_it_reads_again() {
    let _ = meclaw_testing::log_recorder::global();
    let td = tempfile::TempDir::new().unwrap();
    std::fs::write(td.path().join("colony.json"), "{}").unwrap();
    std::fs::create_dir_all(td.path().join("main")).unwrap();
    std::fs::write(
        td.path().join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    let (tx, mut secret_rx) = mpsc::channel(8);
    h.spawn(Path::new("/vault/secret"), move || {
        CaptureCell::new(tx.clone())
    })
    .await;
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("boot");
    let _ = h.drain_dead_letters().await;

    // The new hive `/vault`: its config is cut off mid-write and does not
    // parse. Whatever it seals, the colony cannot know.
    let dir = td.path().join("main/vault");
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = dir.join("config.json");
    std::fs::write(&cfg, &VAULT_CONFIG[..30]).unwrap();
    h.add_hive_scope(Path::new("/vault")).await;

    h.send(from_outside("/vault/inner")).await;
    assert!(
        tokio::time::timeout(NOT_DELIVERED, secret_rx.recv())
            .await
            .is_err(),
        "an unreadable seal must not open the boundary"
    );
    let dlq = tokio::time::timeout(MARKER, h.drain_dead_letters())
        .await
        .expect("failure marker: the colony answers the drain");
    let codes: Vec<&str> = dlq.iter().map(|d| d.reason.as_code()).collect();
    let refused = dlq
        .iter()
        .find(|d| d.reason.as_code() == "hive_boundary")
        .unwrap_or_else(|| {
            panic!(
                "a rebuild that could not read `/vault`'s seal must leave the boundary \
                 closed: the outside message to `/vault/inner` was not refused at the \
                 boundary, dead letters {codes:?}"
            )
        });
    assert_eq!(refused.resolved_target, Path::new("/vault/inner"));
    assert_eq!(
        count(td.path(), tracing::Level::ERROR, CLOSED),
        1,
        "closing logs `{CLOSED}` once"
    );

    // The file is whole again; the next pass reads it and the declared port is
    // an address again.
    std::fs::write(&cfg, VAULT_CONFIG).unwrap();
    let _ = tokio::time::timeout(MARKER, h.drain_dead_letters())
        .await
        .expect("failure marker: the colony answers the drain");
    h.send(from_outside("/vault/secret")).await;
    let got = tokio::time::timeout(MARKER, secret_rx.recv())
        .await
        .expect("after a good rebuild the declared port is an address again")
        .expect("channel closed");
    assert_eq!(got.target, Path::new("/vault/secret"));
    assert_eq!(
        count(td.path(), tracing::Level::WARN, REOPENED),
        1,
        "the good rebuild logs `{REOPENED}` once"
    );
    h.shutdown().await;
}

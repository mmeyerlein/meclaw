//! Phase-1 demo binary.
//!
//! Holds a phase-1 topology (2 echos chained + 1 flaky cell) running so an
//! operator can attach `tokio-console` and inspect the actor substrate.
//!
//! Run with:
//!     cargo run --example phase_1_demo -p meclaw-cli
//!
//! In another terminal:
//!     tokio-console
//!
//! Press Ctrl-C in the demo terminal to exit cleanly.

use meclaw_testing::MessageBuilder;
use meclaw_testing::topologies::phase_1::build_phase_1_topology;

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> anyhow::Result<()> {
    let log_path = std::path::PathBuf::from("./target/phase_1_demo.log.jsonl");
    let _guard = meclaw_cli::setup_subscriber(&log_path, "info", None, false, 6669)?;

    tracing::info!(event = "phase_1_demo_starting");
    let mut topo = build_phase_1_topology().await;
    tracing::info!(event = "phase_1_demo_topology_ready");

    // One sample round-trip so the demo isn't completely silent.
    topo.colony
        .send(MessageBuilder::new("/echo-a").build())
        .await;
    while let Ok(p) =
        tokio::time::timeout(std::time::Duration::from_millis(250), topo.tap_rx.recv()).await
    {
        let Some(p) = p else { break };
        tracing::info!(event = "phase_1_demo_tap", path = %p.as_str());
    }

    eprintln!("Phase 1 demo running. Logs: {}", log_path.display());
    eprintln!("Attach tokio-console in another terminal. Ctrl-C to exit.");
    tokio::signal::ctrl_c().await?;
    tracing::info!(event = "phase_1_demo_shutdown_requested");
    topo.colony.shutdown().await;
    tracing::info!(event = "phase_1_demo_shutdown_complete");
    Ok(())
}

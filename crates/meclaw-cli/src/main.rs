use clap::Parser;
use meclaw_cli::Cli;

/// The workspace's `meclaw` binary. The published one lives in the `meclaw`
/// package and is the same three lines: both hand the parsed `Cli` to
/// [`meclaw_cli::entrypoint`], which owns the mode order, the subscriber and
/// the `ask` dispatch. Two copies of that logic drifted once (GH #623 review);
/// there is one copy now, and it is not here.
#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> anyhow::Result<()> {
    meclaw_cli::entrypoint(Cli::parse()).await
}

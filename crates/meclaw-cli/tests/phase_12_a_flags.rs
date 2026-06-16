//! Phase-12-A TDD-Anker: Top-Level-Flags (CLAUDE.md R9 — keine Subcommands).
//! --validate hat Vorrang, einzeilige stderr-Notiz wenn --api/--daemon
//! danebenstehen (kein Error). --daemon ohne --api ist erlaubt.

use clap::Parser;
use meclaw_cli::Cli;
use std::net::SocketAddr;

#[test]
fn parses_api_daemon_validate_blobs_flags() {
    let cli = Cli::parse_from([
        "meclaw",
        "--api",
        "127.0.0.1:7777",
        "--daemon",
        "--blobs",
        "/tmp/b",
    ]);
    assert_eq!(
        cli.api,
        Some("127.0.0.1:7777".parse::<SocketAddr>().unwrap())
    );
    assert!(cli.daemon);
    assert_eq!(cli.blobs, Some(std::path::PathBuf::from("/tmp/b")));
    assert!(!cli.validate);
}

#[test]
fn validate_without_api_or_daemon_parses_clean() {
    let cli = Cli::parse_from(["meclaw", "--validate"]);
    assert!(cli.validate);
    assert_eq!(cli.api, None);
    assert!(!cli.daemon);
}

#[test]
fn daemon_without_api_is_allowed() {
    let cli = Cli::parse_from(["meclaw", "--daemon"]);
    assert!(cli.daemon);
    assert_eq!(cli.api, None);
}

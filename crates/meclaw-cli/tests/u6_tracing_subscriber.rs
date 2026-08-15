//! U6 (roadmap 2026-06-11): the production binary must initialize the tracing
//! subscriber — `--log` materializes `log.jsonl` and it fills with the boot
//! events. Runs the REAL binary (`CARGO_BIN_EXE_meclaw`) in a child process:
//! `setup_subscriber` sets the process-global subscriber, so in-process tests
//! cannot exercise this path twice.
//!
//! Secret hygiene rider (same boot): the tree's cell params carry a
//! `${U6_SECRET}` whose substituted value must NEVER appear in `log.jsonl`
//! (boot substitution is in-memory; the log records events, not configs).

use std::process::{Child, Command};
use std::time::{Duration, Instant};

const SENTINEL: &str = "U6-SENTINEL-c3f9a";

/// Kill-on-drop guard so a failing assert never leaks the child colony.
struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn boot_with_log_flag_creates_and_fills_log_jsonl_without_secrets() {
    let td = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(td.path().join("demo/a")).unwrap();
    std::fs::write(
        td.path().join("demo/config.json"),
        br#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.path().join("demo/a/config.json"),
        br#"{"cell":{"type":"bash"},"params":{"marker":"${U6_SECRET}"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
    std::fs::write(td.path().join(".env"), format!("U6_SECRET={SENTINEL}\n")).unwrap();
    let log_path = td.path().join("log.jsonl");

    // `--api` is required for a real boot: without it `run()` returns in
    // Direct-Mode before the colony spawns (lib.rs `let Some(bind_addr)`).
    // Port 0 = ephemeral, no fixture port collisions under parallel cargo.
    let child = Command::new(env!("CARGO_BIN_EXE_meclaw"))
        .arg("--root")
        .arg(td.path())
        .arg("--log")
        .arg(&log_path)
        .arg("--api")
        .arg("127.0.0.1:0")
        .arg("--daemon")
        .spawn()
        .unwrap();
    let _guard = ChildGuard(child);

    // Failure-marker timeout: generous 30s convention (robust under cargo
    // parallel load); the boot info event lands far sooner on a healthy path.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if std::fs::metadata(&log_path)
            .map(|m| m.len() > 0)
            .unwrap_or(false)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "U6: log.jsonl was not created/filled within 30s — tracing subscriber not wired"
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    // Let the non-blocking appender flush any trailing boot events, then pin
    // both halves: events present, substituted secret value absent.
    std::thread::sleep(Duration::from_millis(1000));
    let content = std::fs::read_to_string(&log_path).unwrap();
    assert!(
        content.contains("filesystem bootstrap applied"),
        "U6: boot info event must be in log.jsonl; got: {}",
        &content[..content.len().min(500)]
    );
    assert!(
        !content.contains(SENTINEL),
        "U6 secret hygiene: substituted ${{U6_SECRET}} value leaked into log.jsonl"
    );
}

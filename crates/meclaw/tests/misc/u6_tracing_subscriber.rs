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
const BOOT_LINE: &str = "filesystem bootstrap applied";
/// Logged by the colony loop on SIGTERM, after the boot apply spawned the cells.
const SHUTDOWN_LINE: &str = "shutdown requested";

/// The log as it stands once `needle` is in it, or at `limit`.
/// GH #1072: the first cut waited for a non-empty file and then a fixed
/// second; on a loaded 12-thread lane (07.10.) the boot line was not there
/// after that second. Waiting for the line itself is load-independent; the
/// whole file comes back either way, for the secret check and the message.
fn wait_for_line(path: &std::path::Path, needle: &str, limit: Duration) -> String {
    let deadline = Instant::now() + limit;
    loop {
        let content = std::fs::read_to_string(path).unwrap_or_default();
        if content.contains(needle) || Instant::now() >= deadline {
            return content;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// GH #1072 lock: the boot line may land well after the first line under a
/// loaded lane (measured 07.10.: not there one second after the file filled,
/// 12-thread lane with neighbour load). A writer that delays the boot line by
/// 2.5 s still has it found; a line that never comes returns the whole file.
#[test]
fn gh1072_the_wait_finds_a_boot_line_that_comes_late() {
    let td = tempfile::TempDir::new().unwrap();
    let path = td.path().join("log.jsonl");
    let p = path.clone();
    let writer = std::thread::spawn(move || {
        use std::io::Write;
        let mut f = std::fs::File::create(&p).unwrap();
        writeln!(f, "{{\"msg\":\"first line\"}}").unwrap();
        f.flush().unwrap();
        std::thread::sleep(Duration::from_millis(2500));
        writeln!(f, "{{\"msg\":\"{BOOT_LINE}\"}}").unwrap();
        f.flush().unwrap();
    });
    let got = wait_for_line(&path, BOOT_LINE, Duration::from_secs(30));
    writer.join().unwrap();
    assert!(
        got.contains(BOOT_LINE),
        "the wait returned before the late boot line: {got:?}"
    );
    let missing = wait_for_line(&path, "never written", Duration::from_millis(300));
    assert!(
        missing.contains("first line") && missing.contains(BOOT_LINE),
        "a line that never comes must hand back the whole file for the message: {missing:?}"
    );
}

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
    let mut guard = ChildGuard(child);

    // Failure-marker timeout: generous 30s convention (robust under cargo
    // parallel load); the boot info event lands far sooner on a healthy path.
    let content = wait_for_line(&log_path, BOOT_LINE, Duration::from_secs(30));
    assert!(
        content.contains(BOOT_LINE),
        "U6: boot info event must be in log.jsonl within 30s; got: {}",
        content.get(..content.len().min(2000)).unwrap_or(&content)
    );
    // Review VF2: the boot line lands before the cells spawn, so a secret
    // check that ended there saw no line of the spawn. Stop the colony
    // (SIGTERM -> drain) and wait, bounded, for it to exit and for the drain
    // line -- a line after the spawn -- so the check reads the whole log.
    let pid = guard.0.id().to_string();
    let _ = Command::new("kill").args(["-TERM", &pid]).status();
    let deadline = Instant::now() + Duration::from_secs(30);
    while guard.0.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    let content = wait_for_line(&log_path, SHUTDOWN_LINE, Duration::from_secs(30));
    assert!(
        content.contains(SHUTDOWN_LINE),
        "U6: the drain line after the spawn must be in log.jsonl within 30s; got: {}",
        content
            .get(content.len().saturating_sub(2000)..)
            .unwrap_or(&content)
    );
    assert!(
        !content.contains(SENTINEL),
        "U6 secret hygiene: substituted ${{U6_SECRET}} value leaked into log.jsonl"
    );
}

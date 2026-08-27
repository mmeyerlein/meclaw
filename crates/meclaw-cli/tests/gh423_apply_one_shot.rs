//! GH #423 — `--apply` is a one-shot with an exit-code contract, and against a
//! held root the lease refuses it.
//!
//! Every proof here uses a REAL process. The claims are about what the binary
//! does — whether it exits, with which code, and whether it is still running
//! afterwards — and none of that can be observed from inside the library.
//!
//! Two disciplines, both from the GH #121 lease suite: every child is owned by
//! a guard so a failing assertion cannot leak a daemon, and nothing is awaited
//! without a deadline — a regression that leaves a process running would
//! otherwise wedge the run instead of naming itself. And never `pkill`: a
//! foreign colony may run under the same binary name on this host, so a child
//! is only ever stopped through the pid this test remembered.

use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Generous failure marker (30 s convention): a one-shot finishes in well under
/// a second; the deadline only fences a hang.
const DEADLINE: Duration = Duration::from_secs(60);

/// A child process this test owns. Dropping it ends the process.
struct Proc(Option<std::process::Child>);

impl Proc {
    fn pid(&self) -> u32 {
        self.0.as_ref().expect("child is still owned").id()
    }

    fn exited(&mut self) -> Option<std::process::ExitStatus> {
        self.0.as_mut()?.try_wait().ok().flatten()
    }

    /// SIGTERM through `kill(1)` — by the remembered pid, NEVER by name.
    fn sigterm(&self) {
        let _ = Command::new("kill")
            .arg("-TERM")
            .arg(self.pid().to_string())
            .status();
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn copy_tree(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

/// Did the example and the library travel with this tree (GH #49)?
fn shipped() -> bool {
    repo("examples/organism/grow.manifest.json").is_file()
        && repo("templates/meclaw-os/template.json").is_file()
}

/// A root holding the organism seed, the real library and a placeholder `.env`.
fn build_root(root: &Path) {
    copy_tree(&repo("examples/organism/seed"), root);
    copy_tree(&repo("templates"), &root.join("templates"));
    std::fs::write(
        root.join(".env"),
        "OPENROUTER_API_KEY=test-key\n\
         MODEL_BRAIN=gpt-4o-mock\n\
         MODEL_CORE=gpt-4o-mock\n\
         MODEL_CORE_FAST=gpt-4o-mock-fast\n\
         MODEL_CLOSER=gpt-4o-mock\n\
         MODEL_DIALECTIC=gpt-4o-mock\n\
         MODEL_DREAMER=gpt-4o-mock\n\
         TELEGRAM_BOT_TOKEN=test-token\n\
         TELEGRAM_BOT_TOKEN_2=test-token-2\n\
         TELEGRAM_ALLOWED_USER_ID=0\n\
         EXAMPLE_CHAT_TOKEN=test-chat-token\n\
         KEEPER_NIGHT_CRON=0 0 0 1 1 *\n",
    )
    .unwrap();
}

/// The shipped manifest, optionally bent — written into `root` so the shipped
/// file itself is never touched.
fn manifest_at(root: &Path, bend: impl FnOnce(&mut serde_json::Value)) -> std::path::PathBuf {
    let raw = std::fs::read_to_string(repo("examples/organism/grow.manifest.json")).unwrap();
    let mut v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    bend(&mut v);
    let p = root.join("apply.manifest.json");
    std::fs::write(&p, serde_json::to_string_pretty(&v).unwrap()).unwrap();
    p
}

/// Run `meclaw --root ROOT <args>` to completion and return status + streams.
fn run(root: &Path, args: &[&std::ffi::OsStr], stdin: Option<&str>) -> (bool, String, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_meclaw"));
    cmd.arg("--root")
        .arg(root)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
    let mut child = cmd.spawn().expect("spawn meclaw");
    if let Some(s) = stdin {
        child
            .stdin
            .as_mut()
            .expect("stdin piped")
            .write_all(s.as_bytes())
            .expect("write stdin");
    }
    drop(child.stdin.take());

    let deadline = Instant::now() + DEADLINE;
    loop {
        if child.try_wait().ok().flatten().is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "REGRESSION: `--apply` did not finish within {DEADLINE:?} — the \
             one-shot is not shutting down"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    let out = child.wait_with_output().expect("wait");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn osargs<'a>(args: &'a [&'a str]) -> Vec<&'a std::ffi::OsStr> {
    args.iter().map(|s| s.as_ref()).collect()
}

// ──────────────────────────────────────────────────────────────────────────────
// the one-shot
// ──────────────────────────────────────────────────────────────────────────────

/// Boot, apply, print the receipt, shut down, exit 0.
#[test]
fn a_one_shot_apply_exits_zero_and_shuts_down() {
    if !shipped() {
        eprintln!("skipped: examples/organism or the library did not ship (GH #49)");
        return;
    }
    let td = tempfile::TempDir::new().unwrap();
    build_root(td.path());
    let m = manifest_at(td.path(), |_| {});
    let (ok, stdout, stderr) = run(td.path(), &osargs(&["--apply", m.to_str().unwrap()]), None);
    assert!(ok, "exit 0 on a committed manifest; stderr: {stderr}");
    assert!(
        stdout.contains("applied 5 of 5"),
        "the receipt is on stdout: {stdout:?} / {stderr:?}"
    );
}

/// A refused manifest exits non-zero and says where it stopped.
#[test]
fn a_one_shot_apply_that_is_refused_exits_non_zero() {
    if !shipped() {
        eprintln!("skipped: examples/organism or the library did not ship (GH #49)");
        return;
    }
    let td = tempfile::TempDir::new().unwrap();
    build_root(td.path());
    let m = manifest_at(td.path(), |v| {
        v["manifest"][2]["diff"]["add_nodes"][0]["template"] = serde_json::json!("membre@9.9.9");
    });
    let (ok, stdout, stderr) = run(td.path(), &osargs(&["--apply", m.to_str().unwrap()]), None);
    assert!(!ok, "a refused manifest must not exit 0; stdout: {stdout}");
    assert!(
        stderr.contains("entry 3 was refused"),
        "the refusal names the position, on stderr: {stderr}"
    );
    assert!(
        stderr.contains("to resume:"),
        "…and how to pick it up again: {stderr}"
    );
}

/// `--apply -` reads the manifest from stdin.
#[test]
fn an_apply_from_stdin_reads_the_manifest() {
    if !shipped() {
        eprintln!("skipped: examples/organism or the library did not ship (GH #49)");
        return;
    }
    let td = tempfile::TempDir::new().unwrap();
    build_root(td.path());
    let raw = std::fs::read_to_string(repo("examples/organism/grow.manifest.json")).unwrap();
    let (ok, stdout, stderr) = run(td.path(), &osargs(&["--apply", "-"]), Some(&raw));
    assert!(ok, "exit 0; stderr: {stderr}");
    assert!(stdout.contains("applied 5 of 5"), "{stdout:?} / {stderr:?}");
}

/// A `--apply` naming a file that is not there refuses by name, before booting
/// anything interesting.
#[test]
fn an_apply_naming_a_missing_file_refuses_by_name() {
    let td = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(td.path().join("main")).unwrap();
    std::fs::write(
        td.path().join("main/config.json"),
        br#"{"cell":{"type":"hive"}}"#,
    )
    .unwrap();
    let (ok, _stdout, stderr) = run(td.path(), &osargs(&["--apply", "/nope/nope.json"]), None);
    assert!(!ok);
    assert!(stderr.contains("no such file"), "{stderr}");
    assert!(stderr.contains("/nope/nope.json"), "{stderr}");
}

// ──────────────────────────────────────────────────────────────────────────────
// the two modes that keep running
// ──────────────────────────────────────────────────────────────────────────────

/// `--daemon --apply <broken>` prints the refusal and KEEPS RUNNING.
///
/// The colony stands, the mutation does not — that is the audit semantics of
/// every mutation, and a manifest is a list of them. The refusal is a verdict
/// about a body, not about the colony.
#[test]
fn a_daemon_apply_keeps_running_after_a_refusal() {
    if !shipped() {
        eprintln!("skipped: examples/organism or the library did not ship (GH #49)");
        return;
    }
    let td = tempfile::TempDir::new().unwrap();
    build_root(td.path());
    let m = manifest_at(td.path(), |v| {
        v["manifest"][2]["diff"]["add_nodes"][0]["template"] = serde_json::json!("membre@9.9.9");
    });

    let mut proc = Proc(Some(
        Command::new(env!("CARGO_BIN_EXE_meclaw"))
            .arg("--root")
            .arg(td.path())
            .arg("--daemon")
            .arg("--apply")
            .arg(&m)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn"),
    ));

    // The receipt is written after the boot; give it a real window, then check
    // the process is STILL there.
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if log_contains(td.path(), "filesystem bootstrap applied") {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        proc.exited().is_none(),
        "a refused manifest must not end the daemon"
    );

    // Stop it by the pid we remembered — never by name.
    proc.sigterm();
    let deadline = Instant::now() + DEADLINE;
    while proc.exited().is_none() {
        assert!(Instant::now() < deadline, "the daemon did not stop");
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// `--apply` against a root a daemon already holds is refused by the LEASE.
///
/// Orchestrator ruling O5: this is not a gap, it is the HTTP door doing its
/// job. Against a colony that is already running one mutates through
/// `POST /colony/mutations` — and since R5 the same manifest body travels
/// there, so it is one `curl` instead of five.
#[test]
fn an_apply_against_a_held_root_refuses_with_the_lease_message() {
    let td = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(td.path().join("main")).unwrap();
    std::fs::write(
        td.path().join("main/config.json"),
        br#"{"cell":{"type":"hive"}}"#,
    )
    .unwrap();
    let m = td.path().join("m.json");
    std::fs::write(&m, r#"{"manifest":[{"scope":"/","diff":{}}]}"#).unwrap();

    let mut daemon = Proc(Some(
        Command::new(env!("CARGO_BIN_EXE_meclaw"))
            .arg("--root")
            .arg(td.path())
            .arg("--daemon")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn daemon"),
    ));
    let holder_pid = daemon.pid();
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if log_contains(td.path(), "filesystem bootstrap applied") {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }

    let (ok, _stdout, stderr) = run(td.path(), &osargs(&["--apply", m.to_str().unwrap()]), None);
    assert!(!ok, "the second process must not boot on a held root");
    assert!(
        stderr.contains(&holder_pid.to_string()),
        "the refusal names the holding pid {holder_pid}: {stderr}"
    );

    daemon.sigterm();
    let deadline = Instant::now() + DEADLINE;
    while daemon.exited().is_none() {
        assert!(Instant::now() < deadline, "the daemon did not stop");
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Whether the JSONL log carries `needle` yet.
///
/// The boot's own log line is the marker that the daemon is past the bootstrap
/// and into its shutdown select — the same device the GH #121 lease suite uses,
/// and the guarantee a SIGTERM needs.
fn log_contains(root: &Path, needle: &str) -> bool {
    std::fs::read_to_string(root.join("log.jsonl"))
        .map(|s| s.contains(needle))
        .unwrap_or(false)
}

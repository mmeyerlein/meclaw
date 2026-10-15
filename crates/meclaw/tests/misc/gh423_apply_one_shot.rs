//! GH #423 — `--apply` is a one-shot with an exit-code contract, and against a
//! held root the lease refuses it.
//!
//! Every proof here uses a REAL process. The claims are about what the binary
//! does — whether it exits, with which code, and whether it is still running
//! afterwards — and none of that can be observed from inside the library.
//!
//! Test format (`docs/development-rules.md` § 11): topology class, a child
//! process per test. The end of a child is an event: `tokio::process` awaits
//! it (SIGCHLD), and `wait_with_output` drains stdout and stderr while it
//! waits, so a child that writes more than a pipe holds can never wedge on a
//! full pipe the test only reads after the exit. An apply -- boot of a fresh
//! binary with the shipped library, the apply, the shutdown -- waits under its
//! own hang mark (`deadline::APPLY_BOOT_HANG`, whose doc carries the
//! measurement): `BOOT_HANG` (60 s) sits below an apply observed on the
//! weakest lane. A daemon's end after its SIGTERM waits under `BOOT_HANG`.
//! A mark that fires names the hang and the tail of the child's own log,
//! never a verdict on speed.
//!
//! Every child carries `kill_on_drop`: a failing assertion or a hang mark
//! cannot leak a daemon. And never `pkill`: a foreign colony may run under the
//! same binary name on this host, so a child is only ever signalled through
//! the pid this test remembered.

use meclaw_testing::{deadline, wait};
use std::path::Path;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};
use tokio::process::{Child, Command};

/// `meclaw --root ROOT <args>` as a child this test owns: killed when dropped.
fn meclaw(root: &Path, args: &[&std::ffi::OsStr]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_meclaw"));
    cmd.arg("--root").arg(root).args(args).kill_on_drop(true);
    cmd
}

/// SIGTERM through `kill(1)` — by the remembered pid, NEVER by name.
fn sigterm(child: &Child) {
    let pid = child.id().expect("the child has not been reaped yet");
    let sent = std::process::Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status()
        .expect("kill(1) runs");
    assert!(sent.success(), "SIGTERM to pid {pid} was not delivered");
}

/// The last lines of the colony's own log under `root` — the state a hang
/// mark names, so a wedged boot or shutdown says where it stood.
fn log_tail(root: &Path) -> String {
    let log = std::fs::read_to_string(root.join("log.jsonl")).unwrap_or_default();
    let lines: Vec<&str> = log.lines().collect();
    let from = lines.len().saturating_sub(5);
    format!("log.jsonl tail: {:#?}", &lines[from..])
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

/// Waits (boot mark) until the daemon under `root` is past its bootstrap.
async fn past_the_bootstrap(root: &Path) {
    wait::boot_until_with(
        "the daemon logs `filesystem bootstrap applied`",
        || std::future::ready(log_contains(root, "filesystem bootstrap applied")),
        || log_tail(root),
    )
    .await;
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
    // The keeper's nightly close sweep, pushed to a date this run cannot reach.
    // It was a `KEEPER_NIGHT_CRON` line in the `.env` below until GH #138: the
    // schedule is a LITERAL of `session-keeper/night`'s own params now, so such
    // a line is read by nothing at all -- the sweep would fire into this run and
    // nobody would say so. The library copy is this tree's own, so writing the
    // key into it is what an `override_params` entry does to a staged config
    // (`crates/meclaw-cells/tests/misc_2/gh138_keeper_summarizer_dispatcher_params.rs`
    // is the proof that the timer plans on what it finds there).
    meclaw_testing::quiet_keeper_night(&root.join("templates/session-keeper"));
    std::fs::write(
        root.join(".env"),
        "OPENROUTER_API_KEY=test-key\n\
         MODEL_BRAIN=gpt-4o-mock\n\
         MODEL_CORE=gpt-4o-mock\n\
         MODEL_CORE_FAST=gpt-4o-mock-fast\n\
         MODEL_SURFACE=gpt-4o-mock-surface\n\
         MODEL_CLOSER=gpt-4o-mock\n\
         MODEL_DIALECTIC=gpt-4o-mock\nMODEL_FILE_SPACE=gpt-4o-mock\n\
         MODEL_DREAMER=gpt-4o-mock\n\
         TELEGRAM_BOT_TOKEN=test-token\n\
         TELEGRAM_BOT_TOKEN_2=test-token-2\n\
         TELEGRAM_ALLOWED_USER_ID=0\n\
         EXAMPLE_CHAT_TOKEN=test-chat-token\n",
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

/// Run `meclaw --root ROOT <args>` to its end and return status + streams.
///
/// The manifest goes in on stdin while the child runs and its streams are read
/// while it runs (`wait_with_output`), both under the one apply mark; the child
/// is dropped -- and so killed -- if the mark fires.
async fn run(
    root: &Path,
    args: &[&std::ffi::OsStr],
    stdin: Option<&str>,
) -> (bool, String, String) {
    let mut child = meclaw(root, args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .spawn()
        .expect("spawn meclaw");
    let pipe = child.stdin.take();
    let input = stdin.unwrap_or_default().as_bytes().to_vec();
    let feed = async move {
        if let Some(mut pipe) = pipe {
            pipe.write_all(&input).await?;
            pipe.shutdown().await?;
        }
        Ok::<(), std::io::Error>(())
    };
    let (fed, out) = deadline::apply_boot_hang_with(
        "`--apply` ends: the one-shot shuts down and exits",
        async { tokio::join!(feed, child.wait_with_output()) },
        || log_tail(root),
    )
    .await;
    fed.expect("write the manifest to stdin");
    let out = out.expect("the child is waitable");
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
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_one_shot_apply_exits_zero_and_shuts_down() {
    if !shipped() {
        eprintln!("skipped: examples/organism or the library did not ship (GH #49)");
        return;
    }
    let td = tempfile::TempDir::new().unwrap();
    build_root(td.path());
    let m = manifest_at(td.path(), |_| {});
    let (ok, stdout, stderr) =
        run(td.path(), &osargs(&["--apply", m.to_str().unwrap()]), None).await;
    assert!(ok, "exit 0 on a committed manifest; stderr: {stderr}");
    assert!(
        stdout.contains("applied 5 of 5"),
        "the receipt is on stdout: {stdout:?} / {stderr:?}"
    );
}

/// A refused manifest exits non-zero and says where it stopped.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_one_shot_apply_that_is_refused_exits_non_zero() {
    if !shipped() {
        eprintln!("skipped: examples/organism or the library did not ship (GH #49)");
        return;
    }
    let td = tempfile::TempDir::new().unwrap();
    build_root(td.path());
    let m = manifest_at(td.path(), |v| {
        v["manifest"][2]["diff"]["add_nodes"][0]["template"] = serde_json::json!("membre@9.9.9");
    });
    let (ok, stdout, stderr) =
        run(td.path(), &osargs(&["--apply", m.to_str().unwrap()]), None).await;
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
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_apply_from_stdin_reads_the_manifest() {
    if !shipped() {
        eprintln!("skipped: examples/organism or the library did not ship (GH #49)");
        return;
    }
    let td = tempfile::TempDir::new().unwrap();
    build_root(td.path());
    let raw = std::fs::read_to_string(repo("examples/organism/grow.manifest.json")).unwrap();
    let (ok, stdout, stderr) = run(td.path(), &osargs(&["--apply", "-"]), Some(&raw)).await;
    assert!(ok, "exit 0; stderr: {stderr}");
    assert!(stdout.contains("applied 5 of 5"), "{stdout:?} / {stderr:?}");
}

/// A `--apply` naming a file that is not there refuses by name, before booting
/// anything interesting.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_apply_naming_a_missing_file_refuses_by_name() {
    let td = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(td.path().join("main")).unwrap();
    std::fs::write(
        td.path().join("main/config.json"),
        br#"{"cell":{"type":"hive"}}"#,
    )
    .unwrap();
    let (ok, _stdout, stderr) =
        run(td.path(), &osargs(&["--apply", "/nope/nope.json"]), None).await;
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
///
/// No window is waited out. The refusal on stderr is the event; the daemon is
/// still there right after it (a zero-wait check), it ends on THIS test's
/// SIGTERM with exit 0, and the one-shot's verdict -- the line a daemon that
/// ended on the refusal would leave, with a non-zero exit -- never appears.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_daemon_apply_keeps_running_after_a_refusal() {
    if !shipped() {
        eprintln!("skipped: examples/organism or the library did not ship (GH #49)");
        return;
    }
    let td = tempfile::TempDir::new().unwrap();
    build_root(td.path());
    let m = manifest_at(td.path(), |v| {
        v["manifest"][2]["diff"]["add_nodes"][0]["template"] = serde_json::json!("membre@9.9.9");
    });

    let args = [
        std::ffi::OsStr::new("--daemon"),
        std::ffi::OsStr::new("--apply"),
        m.as_os_str(),
    ];
    let mut daemon = meclaw(td.path(), &args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    // stderr is read while the daemon runs, line by line, into a channel.
    let stderr = daemon.stderr.take().expect("stderr piped");
    let (line_tx, mut lines) = tokio::sync::mpsc::unbounded_channel::<String>();
    let reader = tokio::spawn(async move {
        let mut stderr = tokio::io::BufReader::new(stderr).lines();
        while let Ok(Some(line)) = stderr.next_line().await {
            if line_tx.send(line).is_err() {
                break;
            }
        }
    });

    let mut seen = Vec::new();
    deadline::apply_boot_hang_with(
        "the daemon reports the refused entry on stderr",
        async {
            while let Some(line) = lines.recv().await {
                let refused = line.contains("entry 3 was refused");
                seen.push(line);
                if refused {
                    return;
                }
            }
            panic!("stderr closed before the refusal -- the daemon ended: {seen:#?}");
        },
        || log_tail(td.path()),
    )
    .await;
    assert!(
        daemon.try_wait().expect("try_wait").is_none(),
        "a refused manifest must not end the daemon; stderr: {seen:#?}"
    );

    // Stop it by the pid we remembered — never by name.
    sigterm(&daemon);
    let status = deadline::boot_hang_with("the daemon ends on its SIGTERM", daemon.wait(), || {
        log_tail(td.path())
    })
    .await
    .expect("the daemon is waitable");
    // stderr closes with the process: the rest of it, read to its end.
    deadline::hang("the daemon's stderr reaches its end", reader)
        .await
        .expect("the stderr reader must not panic");
    while let Ok(line) = lines.try_recv() {
        seen.push(line);
    }
    assert!(
        !seen.iter().any(|l| l.contains("the manifest was refused")),
        "the daemon took the one-shot's way out on a refusal: {seen:#?}"
    );
    assert!(
        status.success(),
        "a daemon stopped by SIGTERM exits 0, was {status:?}; stderr: {seen:#?}"
    );
}

/// `--apply` against a root a daemon already holds is refused by the LEASE.
///
/// Orchestrator ruling O5: this is not a gap, it is the HTTP door doing its
/// job. Against a colony that is already running one mutates through
/// `POST /colony/mutations` — and since R5 the same manifest body travels
/// there, so it is one `curl` instead of five.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_apply_against_a_held_root_refuses_with_the_lease_message() {
    let td = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(td.path().join("main")).unwrap();
    std::fs::write(
        td.path().join("main/config.json"),
        br#"{"cell":{"type":"hive"}}"#,
    )
    .unwrap();
    let m = td.path().join("m.json");
    std::fs::write(&m, r#"{"manifest":[{"scope":"/","diff":{}}]}"#).unwrap();

    let mut daemon = meclaw(td.path(), &osargs(&["--daemon"]))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn daemon");
    let holder_pid = daemon.id().expect("the daemon runs");
    past_the_bootstrap(td.path()).await;

    let (ok, _stdout, stderr) =
        run(td.path(), &osargs(&["--apply", m.to_str().unwrap()]), None).await;
    assert!(!ok, "the second process must not boot on a held root");
    assert!(
        stderr.contains(&holder_pid.to_string()),
        "the refusal names the holding pid {holder_pid}: {stderr}"
    );

    sigterm(&daemon);
    let status = deadline::boot_hang_with("the daemon ends on its SIGTERM", daemon.wait(), || {
        log_tail(td.path())
    })
    .await
    .expect("the daemon is waitable");
    // The claim here is the lease's refusal; how the holder ends is the
    // refusal test's claim above, so its exit code is only reported.
    eprintln!("the holding daemon ended with {status:?}");
}

//! GH #662 — a colony writes to stderr by default, and keeps `log.jsonl`
//! beside it. Two sinks, two flags, one subscriber.
//!
//! Every proof here runs the REAL binary (`CARGO_BIN_EXE_meclaw`) in a child
//! process: `setup_subscriber` installs the process-global subscriber, so the
//! claim "this reaches stderr" cannot be observed from inside the library.
//! stderr is redirected into a file rather than a pipe — a pipe nobody drains
//! blocks the child once the kernel buffer is full, and the file can be polled
//! with the same deadline as `log.jsonl`.
//!
//! Discipline from the GH #121 lease suite: every child is owned by a guard so
//! a failing assertion cannot leak a daemon, nothing is awaited without a
//! deadline, and a child is only ever stopped through the pid this test
//! remembered — never by name, because a foreign colony may run under the same
//! binary name on this host.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Generous failure marker (30 s convention): the boot event lands far sooner
/// on a healthy path; the deadline only fences a hang.
const DEADLINE: Duration = Duration::from_secs(30);

/// The one line both sinks must be able to carry.
const BOOT_LINE: &str = "filesystem bootstrap applied";

/// The value a `${GH662_MARKER}` param resolves to. It must appear in no sink.
const SENTINEL: &str = "GH662-SENTINEL-b41d7";

/// A child process this test owns. Dropping it ends the process.
struct Proc(Option<Child>);

impl Drop for Proc {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// The smallest colony that really boots: one hive, one cell under it. The
/// cell's `marker` param is the test's handle on substitution.
fn colony_at(root: &Path, marker: &str) {
    std::fs::create_dir_all(root.join("demo/a")).unwrap();
    std::fs::write(
        root.join("demo/config.json"),
        br#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("demo/a/config.json"),
        format!(
            r#"{{"cell":{{"type":"bash"}},"params":{{"marker":"{marker}"}},"contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#
        ),
    )
    .unwrap();
}

/// Start the real binary on `root`, stderr into `err_path`.
///
/// `--api` is required for a real boot: without it `run()` returns in
/// Direct-Mode before the colony spawns. Port 0 = ephemeral, so parallel
/// cargo runs cannot collide. `RUST_LOG` is removed unless a caller puts it
/// back — a variable in the runner's environment would otherwise decide what
/// these tests read.
fn spawn(root: &Path, err_path: &Path, extra: &[&str], env: &[(&str, &str)]) -> Proc {
    let err_file = std::fs::File::create(err_path).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_meclaw"));
    cmd.arg("--root")
        .arg(root)
        .arg("--api")
        .arg("127.0.0.1:0")
        .arg("--daemon")
        .args(extra)
        .env_remove("RUST_LOG")
        .stdout(Stdio::null())
        .stderr(Stdio::from(err_file));
    for (k, v) in env {
        cmd.env(k, v);
    }
    Proc(Some(cmd.spawn().unwrap()))
}

/// A plain colony under `dir/root`, booted with `extra`.
fn boot(dir: &Path, extra: &[&str]) -> (Proc, PathBuf) {
    boot_env(dir, extra, &[])
}

/// The same, with `env` in the child's environment.
fn boot_env(dir: &Path, extra: &[&str], env: &[(&str, &str)]) -> (Proc, PathBuf) {
    let root = dir.join("root");
    colony_at(&root, "plain");
    let err_path = dir.join("stderr.txt");
    let proc = spawn(&root, &err_path, extra, env);
    (proc, err_path)
}

/// The first `n` characters of `s`, for an assertion message. A byte slice
/// would panic instead of reporting when the cut lands inside a UTF-8
/// character.
fn head(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// Poll `path` until it holds `needle`, or fail at the deadline.
fn wait_for(path: &Path, needle: &str, what: &str) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        if std::fs::read_to_string(path)
            .map(|c| c.contains(needle))
            .unwrap_or(false)
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "GH #662: {what} did not carry {needle:?} within 30s; got: {}",
            std::fs::read_to_string(path).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn a_boot_line_reaches_stderr_without_any_flag() {
    let td = tempfile::TempDir::new().unwrap();
    let (_proc, err_path) = boot(td.path(), &[]);
    wait_for(&err_path, BOOT_LINE, "stderr");

    // This stream went into a file, so nothing in it is for a human at a
    // console: colour codes would prefix every line with an escape sequence,
    // and a `grep '^2026'` over the collected log would find nothing.
    let stderr = std::fs::read_to_string(&err_path).unwrap();
    assert!(
        !stderr.contains('\u{1b}'),
        "GH #662: a redirected stream must carry no escape sequences; got: {}",
        head(&stderr, 200)
    );
}

#[test]
fn the_file_is_still_there_beside_it() {
    let td = tempfile::TempDir::new().unwrap();
    let (_proc, _err_path) = boot(td.path(), &[]);
    wait_for(&td.path().join("root/log.jsonl"), BOOT_LINE, "log.jsonl");
}

#[test]
fn log_stderr_off_leaves_only_the_file() {
    let td = tempfile::TempDir::new().unwrap();
    let (_proc, err_path) = boot(td.path(), &["--log-stderr", "off"]);
    wait_for(&td.path().join("root/log.jsonl"), BOOT_LINE, "log.jsonl");
    let stderr = std::fs::read_to_string(&err_path).unwrap();
    assert!(
        stderr.is_empty(),
        "GH #662: --log-stderr off must leave stderr untouched; got: {stderr}"
    );
}

#[test]
fn log_file_off_writes_no_file() {
    let td = tempfile::TempDir::new().unwrap();
    // A path whose parent does not exist: `off` must not even reach the
    // `create_dir_all` that `auto` would do.
    let log_path = td.path().join("nested/deep/log.jsonl");
    let (_proc, err_path) = boot(
        td.path(),
        &["--log-file", "off", "--log", log_path.to_str().unwrap()],
    );
    wait_for(&err_path, BOOT_LINE, "stderr");
    assert!(
        !log_path.exists(),
        "GH #662: --log-file off must write no file; {} is there",
        log_path.display()
    );
    assert!(
        !td.path().join("nested").exists(),
        "GH #662: --log-file off must create no directory for a file it does not open"
    );
    assert!(
        !td.path().join("root/log.jsonl").exists(),
        "GH #662: --log-file off must not fall back to the default path either"
    );
}

/// Two boots, and the two filters trade places between them. `debug` would be
/// the showier level, but a boot of this colony emits no event below `INFO` at
/// all, so `INFO` against `warn` is what discriminates here.
///
/// Both directions are needed. In the first the file is silent, which would
/// also be true of a build carrying no file layer at all; the second makes the
/// file the loud half and stderr the silent one, so each sink proves once that
/// it is installed and once that it obeys its own filter.
#[test]
fn rust_log_moves_the_journal_and_leaves_the_file() {
    // The variable above the flag: stderr speaks, the file keeps `warn`.
    let loud_journal = tempfile::TempDir::new().unwrap();
    let (_proc, err_path) = boot_env(
        loud_journal.path(),
        &["--log-level", "warn"],
        &[("RUST_LOG", "info")],
    );
    wait_for(&err_path, BOOT_LINE, "stderr");
    std::thread::sleep(Duration::from_millis(1000));
    let log_path = loud_journal.path().join("root/log.jsonl");
    assert!(
        log_path.exists(),
        "GH #662: the file layer must still be installed, silent or not"
    );
    let file = std::fs::read_to_string(&log_path).unwrap();
    assert!(
        !file.contains(r#""level":"INFO""#),
        "GH #662: RUST_LOG must not reach the file layer; got: {}",
        head(&file, 500)
    );

    // The variable below the flag: now the file speaks and stderr keeps
    // `warn`. A runner that has the variable in its environment cannot
    // redirect what the proof scripts of GH #121, #423 and U6 read.
    let loud_file = tempfile::TempDir::new().unwrap();
    let (_proc, err_path) = boot_env(
        loud_file.path(),
        &["--log-level", "info"],
        &[("RUST_LOG", "warn")],
    );
    wait_for(
        &loud_file.path().join("root/log.jsonl"),
        BOOT_LINE,
        "log.jsonl",
    );
    std::thread::sleep(Duration::from_millis(1000));
    let stderr = std::fs::read_to_string(&err_path).unwrap();
    assert!(
        !stderr.contains(BOOT_LINE),
        "GH #662: RUST_LOG must hold the stderr layer at `warn`; got: {}",
        head(&stderr, 500)
    );
}

/// An exported-but-empty `RUST_LOG` is a wrapper-script accident, and reading
/// it as a filter would leave the default sink silent with nobody having asked
/// for that.
#[test]
fn an_empty_rust_log_does_not_silence_the_journal() {
    let td = tempfile::TempDir::new().unwrap();
    let (_proc, err_path) = boot_env(td.path(), &[], &[("RUST_LOG", "")]);
    wait_for(&err_path, BOOT_LINE, "stderr");
}

#[test]
fn a_substituted_secret_leaks_into_neither_sink() {
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path().join("root");
    colony_at(&root, "${GH662_MARKER}");
    // The value is this test's own, written into this test's throwaway root.
    std::fs::write(root.join(".env"), format!("GH662_MARKER={SENTINEL}\n")).unwrap();
    let err_path = td.path().join("stderr.txt");
    let _proc = spawn(&root, &err_path, &[], &[]);
    wait_for(&err_path, BOOT_LINE, "stderr");
    wait_for(&root.join("log.jsonl"), BOOT_LINE, "log.jsonl");

    // Let the non-blocking appender flush its trailing boot events.
    std::thread::sleep(Duration::from_millis(1000));
    for (what, path) in [("stderr", err_path), ("log.jsonl", root.join("log.jsonl"))] {
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            !content.contains(SENTINEL),
            "GH #662: the substituted value leaked into {what}"
        );
    }
}

/// A typo in an ambient `RUST_LOG` must not decide whether the colony boots.
/// The broken directive is dropped and the rest of the expression still
/// applies, so `info` here still puts the boot line on stderr.
#[test]
fn a_broken_rust_log_directive_does_not_stop_the_boot() {
    let td = tempfile::TempDir::new().unwrap();
    let (_proc, err_path) = boot_env(td.path(), &[], &[("RUST_LOG", "info,nonsense=notalevel")]);
    wait_for(&err_path, BOOT_LINE, "stderr");
}

/// An expression in which nothing parses falls back to the `ERROR` directive —
/// the same answer `EnvFilter::from_default_env` gives. The colony still boots,
/// stderr stays quiet below `ERROR`, and the file keeps `--log-level`.
#[test]
fn a_wholly_unparsable_rust_log_falls_back_to_the_error_default() {
    let td = tempfile::TempDir::new().unwrap();
    let (_proc, err_path) = boot_env(td.path(), &[], &[("RUST_LOG", "nonsense=notalevel")]);
    // The colony is alive: its file half says so.
    wait_for(&td.path().join("root/log.jsonl"), BOOT_LINE, "log.jsonl");
    std::thread::sleep(Duration::from_millis(1000));
    let stderr = std::fs::read_to_string(&err_path).unwrap();
    assert!(
        !stderr.contains(BOOT_LINE),
        "GH #662: an unusable RUST_LOG must leave the ERROR default in place; got: {}",
        head(&stderr, 500)
    );
}

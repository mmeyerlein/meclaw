//! GH #826 — `meclaw --env-report` names the unused and the missing key.
//!
//! The `.env` of a colony root is a substitution source and nothing else: the
//! boot reads it, replaces every `${KEY}` in every `config.json` it walks, and
//! never exports it into the process environment. So a key nobody substitutes
//! is dead weight nobody sees, and a `${KEY}` whose key is gone is a boot
//! refusal waiting for the next restart. The report names both — names only,
//! never a value — and it answers for a colony that is RUNNING, which is the
//! case it was measured on: `--validate` takes the root lease and is refused
//! there.
//!
//! The fixture `.env` is written by this test at run time, into a temporary
//! root, with values that exist nowhere else — so "no value is printed" is a
//! plain substring check over stdout and stderr.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use meclaw_cli::lease::{HOLDER_FILE, LEASE_DIR, ProcStatus, process_status};

const USED_VALUE: &str = "v-used-3f9a";
const UNUSED_VALUE: &str = "v-unused-8c21";

const HIVE: &str = r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#;

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().expect("a file has a parent")).expect("mkdir");
    std::fs::write(p, body).expect("write fixture");
}

/// The fixture root: one hive, one `echo` cell with every token form the
/// substituter knows — a plain key the `.env` carries, a plain key it does
/// not, a key with a default, and an escaped token that is no key at all.
fn fixture() -> tempfile::TempDir {
    let td = tempfile::tempdir().expect("tempdir");
    let root = td.path();
    write(root, "main/config.json", HIVE);
    write(
        root,
        "main/a/config.json",
        r#"{"cell":{"type":"echo"},
            "params":{"emitted_target":"/sink",
                      "marker":"${USED_KEY}",
                      "note":"${MISSING_KEY}",
                      "fallback":"${DEFAULTED_KEY:-x}",
                      "literal":"$${ESCAPED_KEY}"},
            "contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    // Written by the test, at run time, into a temporary root (GH #826 § 2.2.4).
    std::fs::write(
        root.join(".env"),
        format!("USED_KEY={USED_VALUE}\nUNUSED_KEY={UNUSED_VALUE}\n"),
    )
    .expect("write fixture env file");
    td
}

fn env_report(root: &Path) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_meclaw"))
        .arg("--root")
        .arg(root)
        .arg("--env-report")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run meclaw --env-report");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// The findings of a report, as `(kind, name)` — the two-space-indented lines
/// under the header.
fn findings(stdout: &str) -> BTreeSet<(String, String)> {
    stdout
        .lines()
        .filter(|l| l.starts_with("  "))
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            match (it.next(), it.next()) {
                (Some(kind @ ("unused" | "missing")), Some(name)) => {
                    Some((kind.to_string(), name.to_string()))
                }
                _ => None,
            }
        })
        .collect()
}

fn expected() -> BTreeSet<(String, String)> {
    [("unused", "UNUSED_KEY"), ("missing", "MISSING_KEY")]
        .into_iter()
        .map(|(k, n)| (k.to_string(), n.to_string()))
        .collect()
}

/// Every file under `root`, with its size — what "wrote nothing" is measured on.
fn listing(root: &Path) -> BTreeSet<(PathBuf, u64)> {
    let mut out = BTreeSet::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).expect("read_dir").flatten() {
            let p = e.path();
            if p.is_dir() {
                out.insert((p.strip_prefix(root).expect("under root").to_path_buf(), 0));
                stack.push(p);
            } else {
                let len = e.metadata().map(|m| m.len()).unwrap_or(0);
                out.insert((p.strip_prefix(root).expect("under root").to_path_buf(), len));
            }
        }
    }
    out
}

#[test]
fn the_report_names_exactly_the_unused_and_the_missing_key() {
    let td = fixture();
    let (code, stdout, stderr) = env_report(td.path());
    assert_eq!(
        code, 0,
        "a report with findings is an answer, not a failure (OR-T17): stdout={stdout} stderr={stderr}"
    );
    assert!(
        stdout.starts_with("env report: 2 keys in "),
        "the header names the key count and the file: {stdout}"
    );
    assert_eq!(
        findings(&stdout),
        expected(),
        "exactly one unused and one missing key — a used key, a defaulted one and an escaped \
         token are none of the two: {stdout}"
    );
    for quiet in ["USED_KEY ", "DEFAULTED_KEY", "ESCAPED_KEY"] {
        assert!(
            !stdout
                .lines()
                .skip(1)
                .any(|l| l.split_whitespace().nth(1) == Some(quiet.trim())),
            "{quiet} is not a finding: {stdout}"
        );
    }
    assert!(
        stdout.contains("main/a"),
        "a missing key names the cell directory that asks for it: {stdout}"
    );
}

#[test]
fn no_value_of_the_env_file_is_printed() {
    let td = fixture();
    let (_, stdout, stderr) = env_report(td.path());
    for value in [USED_VALUE, UNUSED_VALUE] {
        assert!(
            !stdout.contains(value) && !stderr.contains(value),
            "the report names keys, never a value — {value} leaked: stdout={stdout} stderr={stderr}"
        );
    }
}

#[test]
fn the_report_answers_on_a_held_root() {
    let td = fixture();
    // The lease a running colony holds: a live pid (this test process) with its
    // real start id, planted the way `gh121_root_lease.rs` plants one.
    let pid = std::process::id();
    let start_id = match process_status(pid) {
        ProcStatus::Running { start_id, .. } => start_id,
        other => panic!("this process must be running, got {other:?}"),
    };
    let dir = td.path().join(LEASE_DIR);
    std::fs::create_dir_all(&dir).expect("mkdir lease");
    std::fs::write(
        dir.join(HOLDER_FILE),
        serde_json::json!({ "pid": pid, "start_id": start_id, "acquired_at_unix": 0 }).to_string(),
    )
    .expect("plant lease");

    // Control: the lease really is held — `--validate` is refused on this root.
    let validate = Command::new(env!("CARGO_BIN_EXE_meclaw"))
        .arg("--root")
        .arg(td.path())
        .arg("--validate")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run meclaw --validate");
    assert!(
        !validate.status.success(),
        "the control: --validate must be refused on a held root, or this test proves nothing"
    );

    let (code, stdout, stderr) = env_report(td.path());
    assert_eq!(
        code, 0,
        "the report must answer for a running colony: stdout={stdout} stderr={stderr}"
    );
    assert_eq!(findings(&stdout), expected(), "{stdout}");
}

#[test]
fn the_report_writes_nothing_into_the_root() {
    let td = fixture();
    let before = listing(td.path());
    let (code, stdout, stderr) = env_report(td.path());
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    let after = listing(td.path());
    assert_eq!(
        before, after,
        "no lease, no colony.db, no log.jsonl, no templates row — the report reads files and \
         nothing else"
    );
}

#[test]
fn a_marker_counts_the_keys_of_the_template_it_grows() {
    let td = tempfile::tempdir().expect("tempdir");
    let root = td.path();
    write(
        root,
        "templates/leaf/template.json",
        r#"{"name":"leaf","version":"1.0.0"}"#,
    );
    write(
        root,
        "templates/leaf/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        root,
        "templates/leaf/inner/config.json",
        r#"{"cell":{"type":"code"},
            "params":{"script_inline":"print('${TEMPLATE_KEY}')",
                      "token":"${TEMPLATE_MISSING}"}}"#,
    );
    write(root, "main/config.json", HIVE);
    write(
        root,
        "main/os/config.json",
        r#"{"cell":{"type":"ref","template":"leaf@1.0.0"}}"#,
    );
    std::fs::write(root.join(".env"), "TEMPLATE_KEY=v-template-51d0\n")
        .expect("write fixture env file");

    let (code, stdout, stderr) = env_report(root);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    let expect: BTreeSet<(String, String)> =
        [("missing".to_string(), "TEMPLATE_MISSING".to_string())].into();
    assert_eq!(
        findings(&stdout),
        expect,
        "a marker's keys are the keys of the template it grows — the whole config, \
         `script_inline` included: {stdout}"
    );
    assert!(!stdout.contains("v-template-51d0"));
}

/// T7 review M1: a `.env` line that is no `KEY=` (a continuation line of a
/// multi-line value, say) splits at its first `=` like any other, and what
/// stands left of it is a fragment of a VALUE. The report prints only names
/// shaped like one and counts the rest.
#[test]
fn a_line_that_is_no_key_is_counted_and_never_printed() {
    let td = fixture();
    std::fs::write(
        td.path().join(".env"),
        format!("USED_KEY={USED_VALUE}\nUNUSED_KEY={UNUSED_VALUE}\nMIIBx+q9/fragment=tail\n"),
    )
    .expect("write fixture env file");
    let (code, stdout, stderr) = env_report(td.path());
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(
        !stdout.contains("MIIBx") && !stderr.contains("MIIBx"),
        "a value fragment was printed as a key: stdout={stdout} stderr={stderr}"
    );
    assert!(
        stdout.contains("1 line"),
        "the report says a line was not read as a key: {stdout}"
    );
    assert_eq!(findings(&stdout), expected(), "{stdout}");
}

/// T7 review M2: an unterminated `${` makes the report fail — and the failure
/// names the file, the JSON pointer and the position, never the string, which
/// may be a whole `script_inline`.
#[test]
fn an_unterminated_token_names_file_and_position_not_the_text() {
    let td = fixture();
    write(
        td.path(),
        "main/b/config.json",
        r#"{"cell":{"type":"echo"},
            "params":{"emitted_target":"/sink",
                      "script":"text-that-stays-private-4b1d ${OPEN_KEY"},
            "contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    let (code, stdout, stderr) = env_report(td.path());
    assert_ne!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(
        !stderr.contains("text-that-stays-private") && !stdout.contains("text-that-stays-private"),
        "the whole string was printed: stderr={stderr}"
    );
    assert!(
        stderr.contains("config.json") && stderr.contains("/params/script"),
        "the failure names the file and the pointer: {stderr}"
    );
}

//! GH #1060 (Vault-Grants, strand V3): the `code` cell spends a credential
//! grant and hands the opened secret to ITS script, per call, as one
//! environment entry — never in argv, the stdin document, a file, the config
//! or a log line.
//!
//! Locks T4–T9 and the `code` half of T10 of the plan
//! (`plan-parts/V3-search-code.md` § 4). Driven through the real factory and
//! the stateless dispatcher; T6 drives the warm harness itself, because the
//! property is the harness's (one child serves job after job). Secrets are stub
//! values (`stub-secret-<n>`).

#[path = "support/gh1060.rs"]
mod support;

use meclaw_cells::code::CodeCellFactory;
use meclaw_core::serde_json::{self, Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use support::{Running, Stub, error_code, is_credential_request, recipient_of, sealed_box, spawn};

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A script that answers with what it can see of the credential: the LENGTH
/// of the entry (never the value), plus argv, `/proc/self/cmdline` and its
/// stdin, so a lock can check that the value is in none of them.
fn probe_script(env_name: &str) -> String {
    format!(
        r#"
import json, os, sys
raw = sys.stdin.read()
v = os.environ.get("{env_name}")
try:
    cmd = open("/proc/self/cmdline", "rb").read().decode("utf-8", "replace")
except OSError:
    cmd = ""
sys.stderr.write("probe ran\n")
print(json.dumps({{"header": {{}}, "messages": [{{"origin": "tool", "type": "tool_result", "id": "",
    "text": json.dumps({{"len": -1 if v is None else len(v), "argv": sys.argv, "cmdline": cmd, "stdin": raw}})}}]}}))
"#
    )
}

fn seen_by_script(reply: &Value) -> Value {
    let text = reply["messages"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("the script answered: {reply}"));
    serde_json::from_str(text).expect("the probe's text is JSON")
}

/// Ask, deliver, and return the script's answer to the one parked call.
async fn one_granted_call(cell: &mut Running, secret: &str) -> Value {
    cell.send(json!({"messages": [{"origin": "user", "type": "text", "text": "go"}]}))
        .await;
    let request = cell.next().await;
    assert!(is_credential_request(&request), "{request}");
    cell.send(sealed_box(&recipient_of(&request), secret)).await;
    let reply = cell.next().await;
    assert_eq!(error_code(&reply), None, "the script ran: {reply}");
    reply
}

/// T4: cold runner — the script sees the granted credential under the
/// configured entry name, and its length matches.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1060_a_code_call_sees_its_credential_as_an_env_entry_cold() {
    let mut cell = spawn(
        Arc::new(CodeCellFactory),
        json!({"runner": "python3", "script_inline": probe_script("EMBED_KEY"),
               "credential_grant_id": "g-code", "credential_env": "EMBED_KEY"}),
    );
    let reply = one_granted_call(&mut cell, "stub-secret-4").await;
    assert_eq!(seen_by_script(&reply)["len"], "stub-secret-4".len());
    assert!(!reply.to_string().contains("stub-secret-4"));
}

/// T5: warm runner — the same, through the resident harness (job frame).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1060_a_code_call_sees_its_credential_as_an_env_entry_warm() {
    let mut cell = spawn(
        Arc::new(CodeCellFactory),
        json!({"runner": "python3", "script_inline": probe_script("MECLAW_CREDENTIAL"),
               "runner_mode": "warm", "max_concurrency": 1,
               "credential_grant_id": "g-code"}),
    );
    let reply = one_granted_call(&mut cell, "stub-secret-5").await;
    let seen = seen_by_script(&reply);
    assert_eq!(seen["len"], "stub-secret-5".len());
    assert!(
        seen["stdin"]
            .as_str()
            .unwrap_or_default()
            .starts_with("{\"body\":"),
        "the body reads the plain document, not the frame: {seen}"
    );
    assert!(!reply.to_string().contains("stub-secret-5"));
}

/// Drive the shipped harness by hand: boot it with `script`, feed `lines`,
/// return one answer frame per line.
fn drive_harness(script: &str, persistent: bool, lines: &[String]) -> Vec<Value> {
    use std::io::{BufRead, BufReader, Write};
    let harness = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/code/harness.py"),
    )
    .expect("harness.py");
    let mut child = std::process::Command::new("python3")
        .arg("-c")
        .arg(harness)
        .env_remove("MECLAW_CREDENTIAL")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("python3");
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));
    writeln!(
        stdin,
        "{}",
        json!({"script": script, "persistent": persistent})
    )
    .unwrap();
    let mut frames = Vec::new();
    for line in lines {
        writeln!(stdin, "{line}").unwrap();
        stdin.flush().unwrap();
        let mut answer = String::new();
        stdout.read_line(&mut answer).expect("an answer frame");
        frames.push(serde_json::from_str(&answer).expect("frame JSON"));
    }
    drop(stdin);
    let _ = child.wait();
    frames
}

/// The job frame as `code::harness::job_frame` writes it (the format is the
/// harness's wire, so the lock spells it out).
fn job_frame(document: &str, name: &str, value: &str) -> String {
    format!(
        "{{\"meclaw_job\":1,\"env\":{},\"document\":{}}}",
        json!({ name: value }),
        Value::String(document.to_string())
    )
}

/// T6: one warm child serves job after job. The entry of a granted job is gone
/// for the next job without one — also when the granted job raised, and also
/// in the persistent (resident) namespace.
#[test]
fn gh1060_the_next_job_does_not_see_it() {
    let script = r#"
import json, os, sys
d = json.load(sys.stdin)
v = os.environ.get("MECLAW_CREDENTIAL")
print(json.dumps({"len": -1 if v is None else len(v)}))
if d.get("raise"):
    raise RuntimeError("boom")
"#;
    for persistent in [false, true] {
        let frames = drive_harness(
            script,
            persistent,
            &[
                job_frame(r#"{"raise":false}"#, "MECLAW_CREDENTIAL", "stub-secret-6"),
                r#"{"raise":false}"#.to_string(),
                job_frame(r#"{"raise":true}"#, "MECLAW_CREDENTIAL", "stub-secret-6"),
                r#"{"raise":false}"#.to_string(),
            ],
        );
        let lens: Vec<Value> = frames
            .iter()
            .map(|f| {
                let out: Value = serde_json::from_str(f["stdout"].as_str().unwrap_or("").trim())
                    .unwrap_or(Value::Null);
                out["len"].clone()
            })
            .collect();
        assert_eq!(
            lens,
            vec![json!(13), json!(-1), json!(13), json!(-1)],
            "persistent={persistent}: the entry exists for its own job only"
        );
        assert_eq!(frames[2]["exit_code"], 1, "the third job raised");
        for f in &frames {
            assert!(
                !f.to_string().contains("stub-secret-6"),
                "no frame, traceback included, carries the value: {f}"
            );
        }
    }
}

// ───────────────────────────────────────────────────────────── log capture

/// Every tracing event of the process, all field values rendered. A std mutex
/// in a TEST collector; the no-lock rule is about cell state.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<String>>>);

struct Render(String);

impl tracing::field::Visit for Render {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.push_str(&format!(" {}={value:?}", field.name()));
    }
}

impl tracing::Subscriber for Capture {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut r = Render(event.metadata().target().to_string());
        event.record(&mut r);
        self.0.lock().unwrap().push(r.0);
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

/// T7: the value is in no argv (also not on the GH #349 temp-file path of an
/// oversized inline script), not in the stdin document, and in no log line —
/// while the script does see it. The probe writes to stderr on purpose: that
/// stderr is logged (`code script wrote to stderr`), so the log path runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1060_no_credential_reaches_argv_stdin_or_the_log() {
    let capture = Capture::default();
    // Process-wide (nextest runs one test per process): the cell's events come
    // from the runtime's worker threads, not from this one.
    let _ = tracing::subscriber::set_global_default(capture.clone());

    let small = probe_script("MECLAW_CREDENTIAL");
    let oversized = format!("{small}\n#{}\n", "x".repeat(140 * 1024));
    for (n, script) in [(1, small), (2, oversized)] {
        let secret = format!("stub-secret-7{n}");
        let mut cell = spawn(
            Arc::new(CodeCellFactory),
            json!({"runner": "python3", "script_inline": script,
                   "credential_grant_id": "g-code"}),
        );
        let reply = one_granted_call(&mut cell, &secret).await;
        let seen = seen_by_script(&reply);
        assert_eq!(seen["len"], secret.len(), "variant {n}: the script saw it");
        for part in ["argv", "cmdline", "stdin"] {
            assert!(
                !seen[part].to_string().contains(&secret),
                "variant {n}: the value is in the script's {part}"
            );
        }
        assert!(!reply.to_string().contains(&secret));
    }
    let lines = capture.0.lock().unwrap().clone();
    assert!(
        lines.iter().any(|l| l.contains("probe ran")),
        "the stderr log line was written (the scan below has something to scan)"
    );
    for l in &lines {
        assert!(
            !l.contains("stub-secret-7"),
            "a log line carries the value: {l}"
        );
    }
}

// ─────────────────────────────────────────────────────────── the embedders

/// `${NAME:-default}` → default, `${NAME}` → "", except the names in `set`.
fn resolve(text: &str, set: &[(&str, &str)]) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find("${") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        let j = after.find('}').expect("closed token");
        let token = &after[..j];
        let (name, default) = token.split_once(":-").unwrap_or((token, ""));
        match set.iter().find(|(n, _)| *n == name) {
            Some((_, v)) => out.push_str(v),
            None => out.push_str(default),
        }
        rest = &after[j + 1..];
    }
    out.push_str(rest);
    out
}

fn shipped_embed_params(rel: &str, endpoint: &str) -> Value {
    let raw = std::fs::read_to_string(repo_root().join(rel)).expect("the shipped embed config");
    let resolved = resolve(&raw, &[("MEMORY_EMBED_ENDPOINT", endpoint)]);
    let config: Value = serde_json::from_str(&resolved).expect("config JSON");
    let mut params = config["params"].clone();
    params["credential_grant_id"] = json!("g-embed");
    params
}

fn embedding_reply() -> String {
    let v: Vec<f64> = (0..1024)
        .map(|i| if i % 2 == 0 { 0.5 } else { -0.5 })
        .collect();
    json!({"data": [{"index": 0, "embedding": v}], "usage": {"prompt_tokens": 1}}).to_string()
}

/// T8: both shipped embedders authorize with the granted key — the
/// file-space one sent NO Authorization before (its key was a param the stdin
/// filter withholds), the memory-hive one had it in its script text.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1060_the_embed_script_authorizes_with_the_granted_key() {
    let cases = [
        (
            "templates/memory-hive/embed/config.json",
            json!({"query": {"text": "what does the user eat", "recall_id": "r1"}}),
        ),
        (
            "templates/file-space/embed/config.json",
            json!({"texts": ["# Title\nintro"]}),
        ),
    ];
    for (rel, args) in cases {
        let stub = Stub::start(Duration::ZERO, embedding_reply());
        let mut cell = spawn(
            Arc::new(CodeCellFactory),
            shipped_embed_params(rel, &stub.url()),
        );
        cell.send(support::tool_call(args, "call-embed")).await;
        let request = cell.next().await;
        assert!(is_credential_request(&request), "{rel}: {request}");
        cell.send(sealed_box(&recipient_of(&request), "stub-secret-8"))
            .await;
        let reply = cell.next().await;
        assert_eq!(error_code(&reply), None, "{rel}: {reply}");
        assert_eq!(
            stub.auth(),
            vec![Some("Bearer stub-secret-8".to_string())],
            "{rel}: the endpoint saw the granted key"
        );
    }
}

// ───────────────────────────────────────────────────────────── concurrency

fn calling_script(url: &str) -> String {
    format!(
        r#"
import json, sys, urllib.request
sys.stdin.read()
urllib.request.urlopen("{url}", timeout=20).read()
print(json.dumps({{"header": {{}}, "messages": [{{"origin": "tool", "type": "tool_result", "id": "", "text": "ok"}}]}}))
"#
    )
}

async fn run_calls(cell: &mut Running, calls: usize, grant: bool) {
    for i in 0..calls {
        cell.send(
            json!({"messages": [{"origin": "user", "type": "text", "text": format!("{i}")}]}),
        )
        .await;
    }
    if grant {
        let request = cell.next().await;
        assert!(is_credential_request(&request), "{request}");
        cell.send(sealed_box(&recipient_of(&request), "stub-secret-9"))
            .await;
    }
    for _ in 0..calls {
        let reply = cell.next().await;
        assert_eq!(error_code(&reply), None, "{reply}");
    }
}

/// T9: six calls park; when the box opens the round, they run — but never
/// more than `max_concurrency` (2) at once. A parked call does not count, and a
/// released one does not bypass the bound.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1060_code_parks_within_its_concurrency_bound() {
    let stub = Stub::start(Duration::from_millis(300), "{}".into());
    let mut cell = spawn(
        Arc::new(CodeCellFactory),
        json!({"runner": "python3", "script_inline": calling_script(&stub.url()),
               "max_concurrency": 2, "credential_grant_id": "g-code"}),
    );
    run_calls(&mut cell, 6, true).await;
    assert_eq!(stub.requests(), 6, "every parked call ran");
    assert!(
        stub.max_in_flight() <= 2,
        "released calls kept the bound of 2 (max in flight {})",
        stub.max_in_flight()
    );
}

/// T10 (`code` half): four calls with `max_concurrency` 4 overlap — with and
/// without a grant. A grant must not make the embedders serial.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1060_parallel_calls_stay_parallel_code() {
    for grant in [false, true] {
        let stub = Stub::start(Duration::from_millis(500), "{}".into());
        let mut params = json!({"runner": "python3", "script_inline": calling_script(&stub.url()),
                                "max_concurrency": 4});
        if grant {
            params["credential_grant_id"] = json!("g-code");
        }
        let mut cell = spawn(Arc::new(CodeCellFactory), params);
        run_calls(&mut cell, 4, grant).await;
        assert_eq!(stub.requests(), 4);
        assert!(
            stub.max_in_flight() >= 2,
            "grant={grant}: the four calls overlapped (max in flight {})",
            stub.max_in_flight()
        );
    }
}

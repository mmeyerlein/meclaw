//! GH #1061 (#801), ruling OR-VG.V4.2 (b2) — the organism example starts, and
//! its member gets the key.
//!
//! `scripts/start.sh MECLAW_EXAMPLE=organism` boots `examples/organism/seed-ref`
//! with no key at all, the reader grows the levels of
//! `docs/getting-started.en.md`, and `<colony>/deposit-key.sh` hands the
//! member's brains their key: stop the daemon by its pid, seal the key into
//! alex's OWN vault with `meclaw --vault-add` (stdin, plainfile passphrase),
//! start the daemon again. This lock runs that road with the REAL binary:
//!
//! 1. the seed-ref root boots (its `ref` marker grows the `meclaw-os` shell);
//! 2. the SHIPPED `grow-door.json`, `grow-org.json`, `grow-member.json` and
//!    `grow-assistant.json` are posted to `POST /colony/mutations`, verbatim —
//!    the member's vault opens itself from `${MECLAW_VAULT_KEY_FILE:-}`, the
//!    assistant is the credentialled generation (grants + v-lanes, no key);
//! 3. the daemon is stopped by its pid, `stub-secret-<n>` goes in on stdin;
//! 4. the daemon starts again, one turn goes to `/door`, and the stub provider
//!    sees `Authorization: Bearer stub-secret-<n>`.
//!
//! WHERE THE STUB IS. The brains' `base_url` is a literal of their templates,
//! not a `${…}` token, so the colony's library COPY is pointed at the stub: every
//! `https://openrouter.ai/api/v1` in it is rewritten to the loopback address.
//! The declarations themselves are posted byte for byte as shipped. The model
//! tokens come from the colony's `.env`, exactly the set `start.sh` writes.
//!
//! No stub value may appear in the daemons' stdout/stderr, `{root}/log.jsonl`,
//! the deposit's own output or the colony's `message_log`.
//!
//! Guarded like every template-reading test (GH #49): a tree without the
//! shipped example and library, or a host without `python3`, is skipped.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// Failure marker (30 s convention), never a timing claim.
const MARKER: Duration = Duration::from_secs(30);
/// The first boot grows the whole `meclaw-os` shell out of the seed's ref
/// marker; twice the marker, still a failure marker and not a timing claim.
const BOOT_MARKER: Duration = Duration::from_secs(60);
const PASSPHRASE: &str = "stub-secret-passphrase-organism";
/// The deposited provider key: what the stub has to see, and nothing else.
const SECRET: &str = "stub-secret-1";
/// The name `scripts/start.sh` deposits under and the grants name.
const CRED_REF: &str = "cred:openrouter";
/// alex's own vault, WITH the root cell directory (`--vault` talks to disk).
const MEMBER_VAULT: &str = "/main/os/orgs/acme/members/alex/access/vault";
/// The literal every shipped `llm` cell (and the embedder's default) calls.
const PROVIDER: &str = "https://openrouter.ai/api/v1";

/// The four declarations, in the order `docs/getting-started.en.md` (and
/// `start.sh` for the door) applies them.
const DECLARATIONS: [&str; 4] = [
    "examples/organism/grow-door.json",
    "examples/organism/grow-org.json",
    "examples/organism/grow-member.json",
    "examples/organism/grow-assistant.json",
];

// ─────────────────────────────────────────────────────────────── the stub

/// A loopback provider on std threads that answers every request with one
/// canned chat completion and records every `Authorization` header.
struct Stub {
    addr: SocketAddr,
    auth: Arc<Mutex<Vec<String>>>,
    hits: Arc<Mutex<usize>>,
}

impl Stub {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the stub");
        let addr = listener.local_addr().expect("stub address");
        let auth = Arc::new(Mutex::new(Vec::new()));
        let hits = Arc::new(Mutex::new(0usize));
        let (a, h) = (Arc::clone(&auth), Arc::clone(&hits));
        let body = json!({
            "id": "chatcmpl-1", "object": "chat.completion", "created": 1,
            "model": "stub-model",
            "choices": [{"index": 0, "finish_reason": "stop",
                         "message": {"role": "assistant", "content": "pong"}}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        })
        .to_string();
        let body = Arc::new(body);
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(conn) = conn else { continue };
                let (a, h, body) = (Arc::clone(&a), Arc::clone(&h), Arc::clone(&body));
                std::thread::spawn(move || serve(conn, &a, &h, &body));
            }
        });
        Self { addr, auth, hits }
    }

    fn authorizations(&self) -> Vec<String> {
        self.auth.lock().unwrap().clone()
    }

    fn hits(&self) -> usize {
        *self.hits.lock().unwrap()
    }
}

fn serve(mut conn: TcpStream, auth: &Mutex<Vec<String>>, hits: &Mutex<usize>, body: &str) {
    let _ = conn.set_read_timeout(Some(Duration::from_secs(10)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        match conn.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut headers = BTreeMap::new();
    for line in head.lines().skip(1) {
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let length: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    while buf.len() < head_end + length {
        match conn.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    *hits.lock().unwrap() += 1;
    if let Some(a) = headers.get("authorization") {
        auth.lock().unwrap().push(a.clone());
    }
    let reply = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = conn.write_all(reply.as_bytes());
}

// ─────────────────────────────────────────────────────────── the material

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped() -> bool {
    let files = [
        "examples/organism/seed-ref/colony.json",
        "examples/organism/seed-ref/main/os/config.json",
        "templates/meclaw-os/template.json",
        "templates/member/config.json",
        "templates/assistant/config.json",
        "templates/access/vault/config.json",
    ];
    files.iter().all(|f| repo(f).is_file())
        && DECLARATIONS.iter().all(|f| repo(f).is_file())
        && Command::new("python3")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
}

/// Copy a tree; with `point_at`, every `.json` file has the provider literal
/// rewritten to the stub (the library copy, never the repository).
fn copy_tree(src: &Path, dst: &Path, point_at: Option<&str>) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dst.join(entry.file_name());
        let from = entry.path();
        if from.is_dir() {
            copy_tree(&from, &to, point_at);
        } else if let (Some(url), Some("json")) =
            (point_at, from.extension().and_then(|e| e.to_str()))
        {
            let text = std::fs::read_to_string(&from).unwrap();
            std::fs::write(&to, text.replace(PROVIDER, url)).unwrap();
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

// ───────────────────────────────────────────────────────────── the process

struct Owned(Option<Child>);

impl Drop for Owned {
    fn drop(&mut self) {
        if let Some(mut c) = self.0.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// `meclaw --root <root>` with nothing of the secret class in its environment.
fn meclaw(root: &Path, home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_meclaw"));
    cmd.env_clear()
        .env(
            "PATH",
            std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
        )
        .env("HOME", home)
        .env("TMPDIR", std::env::temp_dir())
        .current_dir(root)
        .arg("--root")
        .arg(root);
    cmd
}

/// A one-shot run with `stdin`, its success and its combined output.
fn run(mut cmd: Command, stdin: &str) -> (bool, String) {
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn meclaw");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let mut owned = Owned(Some(child));
    let deadline = Instant::now() + MARKER;
    while owned
        .0
        .as_mut()
        .unwrap()
        .try_wait()
        .ok()
        .flatten()
        .is_none()
    {
        assert!(Instant::now() < deadline, "meclaw did not finish: {cmd:?}");
        std::thread::sleep(Duration::from_millis(25));
    }
    let out = owned.0.take().unwrap().wait_with_output().unwrap();
    (
        out.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

/// The daemon, the way `start.sh` and `deposit-key.sh` start it.
fn start_daemon(root: &Path, home: &Path, templates: &Path, api: SocketAddr, log: &Path) -> Owned {
    let out = std::fs::File::create(log.with_extension("out")).unwrap();
    let err = std::fs::File::create(log.with_extension("err")).unwrap();
    let mut cmd = meclaw(root, home);
    cmd.arg("--templates")
        .arg(templates)
        .arg("--daemon")
        .arg("--api")
        .arg(api.to_string())
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err);
    Owned(Some(cmd.spawn().expect("spawn the daemon")))
}

/// Stop by PID (SIGTERM), never by name, and wait for the exit.
async fn stop_by_pid(owned: &mut Owned) {
    let pid = owned.0.as_ref().expect("a running daemon").id();
    let _ = Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status();
    let deadline = Instant::now() + MARKER;
    while owned
        .0
        .as_mut()
        .unwrap()
        .try_wait()
        .ok()
        .flatten()
        .is_none()
    {
        assert!(
            Instant::now() < deadline,
            "the daemon (pid {pid}) did not stop"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    owned.0.take();
}

async fn until(what: &str, marker: Duration, mut f: impl AsyncFnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + marker;
    while !f().await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{what} never happened"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// The last 40 lines of a log file (empty when it is missing).
fn tail(path: &Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(40)..].join("\n")
}

async fn healthy(api: SocketAddr) -> bool {
    reqwest::get(format!("http://{api}/health"))
        .await
        .is_ok_and(|r| r.status().is_success())
}

async fn grow(api: SocketAddr, rel: &str) {
    let body = std::fs::read_to_string(repo(rel)).unwrap();
    let reply = reqwest::Client::new()
        .post(format!("http://{api}/colony/mutations"))
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .unwrap_or_else(|e| panic!("POST /colony/mutations {rel}: {e}"))
        .text()
        .await
        .unwrap();
    let v: Value = serde_json::from_str(&reply).unwrap_or(Value::Null);
    assert_eq!(
        v["mutation"]["outcome"],
        json!("committed"),
        "{rel} is not committed: {reply}"
    );
}

fn message_log(root: &Path) -> Vec<String> {
    let Ok(conn) = rusqlite::Connection::open(root.join("colony.db")) else {
        return Vec::new();
    };
    let Ok(mut st) = conn.prepare("SELECT headers, COALESCE(body_payload, '') FROM message_log")
    else {
        return Vec::new();
    };
    st.query_map([], |r| {
        Ok(format!(
            "{} {}",
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?
        ))
    })
    .expect("query")
    .filter_map(Result::ok)
    .collect()
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

// ─────────────────────────────────────────────────────────────── the claim

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1061_the_organism_example_starts_and_its_member_gets_the_key() {
    if !shipped() {
        eprintln!("skipped: examples/organism, the template library or python3 is absent (GH #49)");
        return;
    }
    let colonies = tempfile::TempDir::new().unwrap();
    let root = colonies.path().join("organism");
    let library = tempfile::TempDir::new().unwrap();
    let logs = tempfile::TempDir::new().unwrap();
    let home = tempfile::TempDir::new().unwrap();
    let stub = Stub::start();
    let stub_url = format!("http://{}/v1", stub.addr);

    // The colony as start.sh lays it down: the seed-ref copy, a library, a
    // 0600 passphrase file NEXT TO the root, and an `.env` of non-secret
    // tokens only -- the key file's path and the model every MODEL_* reads.
    copy_tree(&repo("examples/organism/seed-ref"), &root, None);
    copy_tree(&repo("templates"), library.path(), Some(&stub_url));
    let key_file = colonies.path().join("organism.vault-key");
    std::fs::write(&key_file, format!("{PASSPHRASE}\n")).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let mut dotenv = format!("MECLAW_VAULT_KEY_FILE={}\n", key_file.display());
    for token in [
        "MODEL_BRAIN",
        "MODEL_CORE",
        "MODEL_CORE_FAST",
        "MODEL_SURFACE",
        "MODEL_CLOSER",
        "MODEL_DIALECTIC",
        "MODEL_DREAMER",
        "MODEL_FILE_SPACE",
    ] {
        dotenv.push_str(&format!("{token}=stub-model\n"));
    }
    std::fs::write(root.join(".env"), dotenv).unwrap();

    let api: SocketAddr = format!("127.0.0.1:{}", free_port()).parse().unwrap();
    let mut transcript = String::new();

    // 1. The first boot grows the shell; 2. the four declarations, verbatim.
    let mut daemon = start_daemon(
        &root,
        home.path(),
        library.path(),
        api,
        &logs.path().join("first"),
    );
    let first_log = logs.path().join("first");
    let booted = tokio::time::timeout(BOOT_MARKER, async {
        while !healthy(api).await {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    // The daemon's own words, so a red boot names its reason (the first red
    // run of this lock said only "never happened").
    assert!(
        booted.is_ok(),
        "the first boot never answered /health; daemon stderr tail:\n{}",
        tail(&first_log.with_extension("err"))
    );
    for rel in DECLARATIONS {
        grow(api, rel).await;
    }

    // 3. Stopped by its pid; the key goes in on stdin, never in argv.
    stop_by_pid(&mut daemon).await;
    let mut add = meclaw(&root, home.path());
    add.args(["--vault", MEMBER_VAULT, "--vault-add", CRED_REF])
        .args(["--vault-key-source", "plainfile", "--vault-key-file"])
        .arg(&key_file);
    let (ok, text) = tokio::task::spawn_blocking(move || run(add, SECRET))
        .await
        .expect("the deposit thread");
    assert!(
        ok,
        "--vault-add {CRED_REF} into {MEMBER_VAULT} stores: {text}"
    );
    transcript.push_str(&text);

    // 4. The same daemon again, and one turn through the front door.
    let mut daemon = start_daemon(
        &root,
        home.path(),
        library.path(),
        api,
        &logs.path().join("second"),
    );
    until("the restart answers /health", BOOT_MARKER, async || {
        healthy(api).await
    })
    .await;
    let before = stub.hits();
    let turn = json!({
        "target": "/door",
        "headers": {"channel": "gh1061-organism"},
        "body": {"messages": [{"origin": "user", "type": "text", "text": "ping"}]}
    });
    let client = reqwest::Client::new();
    until("POST /messages to /door is accepted", MARKER, async || {
        client
            .post(format!("http://{api}/messages"))
            .json(&turn)
            .send()
            .await
            .is_ok_and(|r| r.status() == reqwest::StatusCode::ACCEPTED)
    })
    .await;
    let want = format!("Bearer {SECRET}");
    until(
        "the stub provider sees the deposited bearer",
        MARKER,
        async || stub.authorizations().contains(&want),
    )
    .await;
    assert!(stub.hits() > before, "the turn reached the provider");
    let foreign: Vec<String> = stub
        .authorizations()
        .into_iter()
        .filter(|a| *a != want && a.trim() != "Bearer")
        .collect();
    assert!(
        foreign.is_empty(),
        "every bearer the provider saw is the deposited one: {foreign:?}"
    );

    // The value is in no log this test can read.
    stop_by_pid(&mut daemon).await;
    for pass in ["first", "second"] {
        for ext in ["out", "err"] {
            let p = logs.path().join(pass).with_extension(ext);
            transcript.push_str(&std::fs::read_to_string(p).unwrap_or_default());
        }
    }
    transcript.push_str(&std::fs::read_to_string(root.join("log.jsonl")).unwrap_or_default());
    let rows = message_log(&root);
    assert!(!rows.is_empty(), "the colony logged its traffic");
    assert!(!transcript.contains(SECRET), "the key is in a process log");
    let hits: Vec<&String> = rows.iter().filter(|r| r.contains(SECRET)).collect();
    assert!(hits.is_empty(), "the key is on record: {hits:?}");
    assert!(
        !transcript.contains(PASSPHRASE),
        "the passphrase is in a log"
    );
}

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
//! 3. two turns go to `/door` before the key, the daemon is stopped by its pid
//!    while they are still at work, `stub-secret-<n>` goes in on stdin;
//! 4. the daemon starts again and replays what the stop left open, and the
//!    first turn's OWN trace (the `message_id` its `POST /messages` got back)
//!    holds a brain's answer to a stub request that carried
//!    `Authorization: Bearer stub-secret-<n>` -- and so does the second's.
//!
//! WHERE THE STUB IS. The brains' `base_url` is a literal of their templates,
//! not a `${…}` token, so the colony's library COPY is pointed at the stub: every
//! `https://openrouter.ai/api/v1` in it is rewritten to the loopback address.
//! The declarations themselves are posted byte for byte as shipped. The model
//! tokens come from the colony's `.env`, exactly the set `start.sh` writes.
//!
//! No stub value may appear in the daemons' stdout/stderr, `{root}/log.jsonl`,
//! the deposit's own output or the colony's message log (`GET /colony/messages`,
//! read by a third boot after the orderly stop, so the log is whole).
//!
//! Guarded like every template-reading test (GH #49): a tree without the
//! shipped example and library, or a host without `python3`, is skipped.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use meclaw_testing::{deadline, wait};
use serde_json::{Value, json};
use tokio::sync::watch;

const PASSPHRASE: &str = "stub-secret-passphrase-organism";
/// The deposited provider key: what the stub has to see, and nothing else.
const SECRET: &str = "stub-secret-1";
/// The name `scripts/start.sh` deposits under and the grants name.
const CRED_REF: &str = "cred:openrouter";
/// alex's own vault, WITH the root cell directory (`--vault` talks to disk).
const MEMBER_VAULT: &str = "/main/os/orgs/acme/members/alex/access/vault";
/// The literal every shipped `llm` cell (and the embedder's default) calls.
const PROVIDER: &str = "https://openrouter.ai/api/v1";
/// The turn the verdict binds to: it goes in before the key and is replayed.
const FIRST_TURN: &str = "hello before the key";
/// A second turn before the key: the stop leaves it open too, so the second
/// boot replays more than one delivery (re-check fix2 M1 b).
const SECOND_TURN: &str = "a second turn before the key";

/// The four declarations, in the order `docs/getting-started.en.md` (and
/// `start.sh` for the door) applies them.
const DECLARATIONS: [&str; 4] = [
    "examples/organism/grow-door.json",
    "examples/organism/grow-org.json",
    "examples/organism/grow-member.json",
    "examples/organism/grow-assistant.json",
];

// ─────────────────────────────────────────────────────────────── the stub

/// One request the stub answered (GH #1216): its path, its `Authorization`
/// header and the texts of its body's user messages, in order (diagnosis
/// only). Its index in the stub's record is the `n` of its reply marks.
#[derive(Clone, Debug)]
struct Seen {
    path: String,
    auth: Option<String>,
    users: Vec<String>,
}

impl Seen {
    /// The text of the last user message ("" for none).
    fn last_user(&self) -> &str {
        self.users.last().map(String::as_str).unwrap_or_default()
    }

    /// A brain request with `bearer`: a chat completion, not an embedding.
    fn keyed_chat(&self, bearer: &str) -> bool {
        self.path.ends_with("/chat/completions") && self.auth.as_deref() == Some(bearer)
    }
}

/// The reply id of the stub's answer to its request `n`.
fn response_id(n: usize) -> String {
    format!("chatcmpl-{n}")
}

/// The answer text of the stub's reply to its request `n`.
fn answer(n: usize) -> String {
    format!("pong-{n}")
}

/// The stub's reply to its request `n`: one chat completion whose id and
/// answer both name `n` (re-check fix3 M1 b).
fn reply_body(n: usize) -> String {
    json!({
        "id": response_id(n), "object": "chat.completion", "created": 1,
        "model": "stub-model",
        "choices": [{"index": 0, "finish_reason": "stop",
                     "message": {"role": "assistant", "content": answer(n)}}],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
    })
    .to_string()
}

/// The body of a message-log row as `GET /colony/messages` serves it: the
/// inline payload, or the resolved blob (`resolve_blob=true`). A blob row
/// without its body (`blob_error`, or not resolved) is no body and fails here
/// rather than reading as `null` (re-check tv-br M6 M1, TV-A3 fail-closed).
fn row_body(row: &Value) -> Value {
    let parse = |s: &str| serde_json::from_str(s).unwrap_or(Value::Null);
    match row["body_kind"].as_str() {
        Some("inline") => row["body_payload"].as_str().map_or(Value::Null, parse),
        Some("blob") => match &row["blob_body"] {
            Value::Null => panic!(
                "a blob row without its body (blob_error={}) is no body: {row}",
                row["blob_error"]
            ),
            Value::String(s) => parse(s),
            v => v.clone(),
        },
        other => panic!("a message-log row of an unknown body_kind {other:?}: {row}"),
    }
}

/// The stub request a row is the brain's answer to: an `llm` output names the
/// provider reply it carries in `meta.response_id` and carries the reply as
/// its last message (`llm/output.rs` `emit_assistant_turn`). Both must name
/// the same `n`; the wall's history inside a body has no reply id of its own.
fn answered_request(row: &Value) -> Option<usize> {
    let body = row_body(row);
    let n: usize = body["meta"]["response_id"]
        .as_str()?
        .strip_prefix("chatcmpl-")?
        .parse()
        .ok()?;
    let last = body["messages"].as_array()?.last()?;
    (last["text"].as_str()? == answer(n)).then_some(n)
}

/// Whether the trace `trace` holds a brain's answer to a stub chat request
/// that carried `bearer` (re-check fix3 M1). Bauer H
/// measured both turns' keyed requests byte for byte equal -- one wall, one
/// window -- so no body tells the first turn's own request from the second's
/// or from a doubled one; counting them proves nothing. The trace does: the
/// turn's `POST /messages` named it (`message_id` = `trace_id` of a source
/// message, `meclaw-api` `handlers/messages.rs`), the brain's output row
/// carries it, and that row names the one stub request it answers. A row of
/// another trace, the history of the wall, an embedding or a request without
/// the key never count.
fn answered_with_key(seen: &[Seen], rows: &[Value], trace: &str, bearer: &str) -> bool {
    !keyed_answers(seen, rows, trace, bearer).is_empty()
}

/// The stub requests `n` with `bearer` whose brain answers are rows of
/// `trace` (the set [`answered_with_key`] asks to be non-empty).
fn keyed_answers(seen: &[Seen], rows: &[Value], trace: &str, bearer: &str) -> BTreeSet<usize> {
    rows.iter()
        .filter(|r| r["trace_id"].as_str() == Some(trace))
        .filter_map(answered_request)
        .filter(|&n| seen.get(n).is_some_and(|s| s.keyed_chat(bearer)))
        .collect()
}

/// Both turns are answered with the key by two DIFFERENT stub requests
/// (re-check legacy-b fix4 A-H1): each trace holds a keyed answer and no
/// request `n` counts for both -- one stub answer is never the answer of two
/// traces. Real runs cannot share one (an output inherits the trace of the
/// delivery in hand, `llm` emits one output per `handle`, there is no answer
/// cache), so the condition costs nothing and makes "both turns" two requests.
fn two_turns_answered(first: &BTreeSet<usize>, second: &BTreeSet<usize>) -> bool {
    !first.is_empty() && !second.is_empty() && first.is_disjoint(second)
}

/// A trace for a hang: per row (newest first) sender, receiver and the stub
/// request it answers, if any (last 40).
fn trace_state(rows: &[Value]) -> Vec<String> {
    let mut lines = vec![format!("trace: {} rows", rows.len())];
    lines.extend(rows.iter().take(40).map(|r| {
        format!(
            "{} -> {} answers={:?}",
            r["from_path"].as_str().unwrap_or("?"),
            r["to_path"].as_str().unwrap_or("?"),
            answered_request(r)
        )
    }));
    lines
}

/// The texts of the `role: user` messages of a chat body (string content or
/// the `text` of its parts), in order; empty for any other body.
fn user_texts(body: &[u8]) -> Vec<String> {
    let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let Some(messages) = v["messages"].as_array() else {
        return Vec::new();
    };
    messages
        .iter()
        .filter(|m| m["role"] == "user")
        .map(|m| match &m["content"] {
            Value::String(s) => s.clone(),
            Value::Array(parts) => parts
                .iter()
                .filter_map(|p| p["text"].as_str())
                .collect::<Vec<_>>()
                .join(" "),
            _ => String::new(),
        })
        .collect()
}

/// A loopback provider on std threads that records every request as a
/// [`Seen`] and answers request `n` with [`reply_body`]`(n)`.
struct Stub {
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
    hold: Arc<Hold>,
    /// Bumped on every park and every answered request: the event every wait
    /// on the stub awaits (TV-S1 rank 1, re-check fix2 H4), never a poll.
    events: Arc<watch::Sender<u64>>,
}

/// GH #1216: while held, the stub parks every request it reads and answers it
/// only at the next release. A parked call keeps the delivery that made it
/// open, so the test decides by an event, not by a clock, that a stop leaves
/// work for the next boot to replay and that this replay is still running
/// when that boot answers /health.
#[derive(Default)]
struct Hold {
    state: Mutex<HoldState>,
    released: Condvar,
}

#[derive(Default)]
struct HoldState {
    on: bool,
    generation: u64,
    parked: usize,
}

impl Hold {
    /// Parks the calling request while held. The event goes out with the
    /// state lock released: a waiter reads that state under the channel's
    /// lock, so holding both here could deadlock.
    fn park(&self, events: &watch::Sender<u64>) {
        let generation = {
            let mut s = self.state.lock().unwrap();
            if !s.on {
                return;
            }
            s.parked += 1;
            s.generation
        };
        events.send_modify(|n| *n += 1);
        let mut s = self.state.lock().unwrap();
        while s.on && s.generation == generation {
            s = self.released.wait(s).unwrap();
        }
    }
}

impl Stub {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the stub");
        let addr = listener.local_addr().expect("stub address");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let hold = Arc::new(Hold::default());
        let events = Arc::new(watch::Sender::new(0u64));
        let (a, p, e) = (Arc::clone(&seen), Arc::clone(&hold), Arc::clone(&events));
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(conn) = conn else { continue };
                let (a, p, e) = (Arc::clone(&a), Arc::clone(&p), Arc::clone(&e));
                std::thread::spawn(move || serve(conn, &a, &p, &e));
            }
        });
        Self {
            addr,
            seen,
            hold,
            events,
        }
    }

    /// From now on every request is parked until [`Stub::release`].
    fn hold(&self) {
        self.hold.state.lock().unwrap().on = true;
    }

    /// Answers every parked request (a dead daemon's ones fail on write).
    fn release(&self) {
        let mut s = self.hold.state.lock().unwrap();
        s.on = false;
        s.generation += 1;
        self.hold.released.notify_all();
    }

    /// How many requests the stub has parked so far.
    fn parked(&self) -> usize {
        self.hold.state.lock().unwrap().parked
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    fn authorizations(&self) -> Vec<String> {
        self.seen().into_iter().filter_map(|s| s.auth).collect()
    }

    /// Resolves once `pred` holds; re-checked on every stub event, and once
    /// right away (the subscription comes first, so no event is lost).
    async fn event(&self, mut pred: impl FnMut(&Stub) -> bool) {
        let mut rx = self.events.subscribe();
        let _ = rx
            .wait_for(|_| pred(self))
            .await
            .expect("the stub's event channel lives as long as the stub");
    }

    /// The stub's last 40 requests, for a hang: index, path, whether the
    /// bearer is `bearer` (never the value), and the last user message.
    fn state(&self, bearer: &str) -> Vec<String> {
        let seen = self.seen();
        let mut lines = vec![format!(
            "stub: {} requests, {} parked",
            seen.len(),
            self.parked()
        )];
        lines.extend(seen.iter().enumerate().rev().take(40).map(|(n, s)| {
            format!(
                "#{n} {} deposited-bearer={} last-user={:?}",
                s.path,
                s.auth.as_deref() == Some(bearer),
                s.last_user().chars().take(80).collect::<String>()
            )
        }));
        lines
    }
}

fn serve(mut conn: TcpStream, seen: &Mutex<Vec<Seen>>, hold: &Hold, events: &watch::Sender<u64>) {
    // Re-check fix3 H2: the hang mark, not a literal of its own.
    let _ = conn.set_read_timeout(Some(deadline::scaled(deadline::HANG)));
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
    let path = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or_default()
        .to_string();
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
    let users = user_texts(&buf[head_end..buf.len().min(head_end + length)]);
    hold.park(events);
    let n = {
        let mut seen = seen.lock().unwrap();
        seen.push(Seen {
            path,
            auth: headers.get("authorization").cloned(),
            users,
        });
        seen.len() - 1
    };
    events.send_modify(|n| *n += 1);
    let body = reply_body(n);
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

/// A one-shot run with `stdin`, its success and its combined output. The exit
/// is awaited under the hang mark (TV-S3), no sleep loop (re-check fix2 M2).
async fn run(mut cmd: Command, stdin: &str) -> (bool, String) {
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
    let pid = child.id();
    let mut owned = Owned(Some(child));
    let running = owned.0.as_mut().unwrap();
    // Re-check fix3 H1: a hang names the process state. Under the boot mark
    // (GH #1216, tv-br M10k): the call is a whole run of the binary, and the
    // sibling's `--apply` run took 37.5 s under an 80 % CPU quota.
    wait::boot_until_with(
        &format!("meclaw finishes: {cmd:?}"),
        || std::future::ready(running.try_wait().ok().flatten().is_some()),
        || proc_state(pid),
    )
    .await;
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

/// A running daemon and the stem of its `<log>.out`/`<log>.err`, as every
/// wait on it sees it (tv-br M10g, TV-Z3): its exit status the moment it has
/// one (`try_wait`), and its own last words.
struct Daemon {
    owned: RefCell<Owned>,
    pid: u32,
    log: PathBuf,
}

impl Daemon {
    /// The daemon's exit status, once it has exited (`try_wait` reaps it).
    fn exit(&self) -> Option<std::process::ExitStatus> {
        self.owned
            .borrow_mut()
            .0
            .as_mut()
            .and_then(|c| c.try_wait().ok().flatten())
    }

    /// Fail at once when the daemon has exited (tv-br M10g): an exited daemon
    /// answers nothing, and waiting out a hang mark on it hid WHY -- the M11
    /// reds read only `ConnectionRefused` after 30 s. An exit is an event,
    /// not a time verdict, so it ends the wait the moment it is seen.
    fn alive(&self, what: &str) {
        if self.exit().is_some() {
            panic!(
                "the daemon exited while waiting for: {what}; last state: {:#?}",
                self.state()
            );
        }
    }

    /// The daemon's last observed state for a red wait (GH #1209, #1216,
    /// tv-br M10g): running or its exit status, its lines about credential
    /// rounds, refusals, the watchdog and every WARN/ERROR (`<log>.err` and
    /// `<log>.out`, last 40), then the last 15 lines of `<log>.err` as they
    /// are -- an exit's own reason need not match a filter.
    fn state(&self) -> Vec<String> {
        let head = match self.exit() {
            Some(status) => format!("daemon pid {} EXITED: {status}", self.pid),
            None => format!("daemon {}", proc_state(self.pid)),
        };
        let mut state = vec![head];
        state.extend(daemon_lines(&self.log));
        let err = tail(&self.log.with_extension("err"));
        state.push("-- stderr tail --".into());
        state.extend(err[err.len().saturating_sub(15)..].iter().cloned());
        state
    }
}

/// The daemon, the way `start.sh` and `deposit-key.sh` start it.
fn start_daemon(root: &Path, home: &Path, templates: &Path, api: SocketAddr, log: &Path) -> Daemon {
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
    let child = cmd.spawn().expect("spawn the daemon");
    Daemon {
        pid: child.id(),
        owned: RefCell::new(Owned(Some(child))),
        log: log.to_path_buf(),
    }
}

/// Stop by PID (SIGTERM), never by name, and await the exit under the hang
/// mark (TV-S3, re-check fix2 M2); a hang names the daemon's last state
/// (re-check fix3 H1).
async fn stop_by_pid(daemon: &Daemon) {
    let pid = daemon.pid;
    let _ = Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status();
    wait::until_with(
        &format!("the daemon (pid {pid}) stops on SIGTERM"),
        || std::future::ready(daemon.exit().is_some()),
        || daemon.state(),
    )
    .await;
    daemon.owned.borrow_mut().0.take();
}

/// A process's state from `/proc` (the child is reaped only at drop, so an
/// exited one shows as a zombie).
fn proc_state(pid: u32) -> String {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
    let st = stat
        .rsplit_once(") ")
        .and_then(|(_, rest)| rest.split(' ').next())
        .unwrap_or("gone");
    let alive = if st == "Z" || st == "gone" {
        "EXITED"
    } else {
        "alive"
    };
    format!("pid {pid} state {st} ({alive})")
}

/// A daemon's telling lines for a hang (GH #1209, #1216): its lines about
/// credential rounds, refusals, the watchdog and every WARN/ERROR
/// (`<log>.err` and `<log>.out`, last 40).
fn daemon_lines(log: &Path) -> Vec<String> {
    let mut lines = Vec::new();
    for ext in ["err", "out"] {
        let text = std::fs::read_to_string(log.with_extension(ext)).unwrap_or_default();
        lines.extend(
            text.lines()
                .filter(|l| {
                    [
                        "credential",
                        "vault",
                        "grant",
                        "sealed",
                        "denied",
                        "attest",
                        "WARN",
                        "ERROR",
                        "watchdog",
                        "panic",
                        "replaying",
                    ]
                    .iter()
                    .any(|k| l.contains(k))
                })
                .map(|l| l.chars().take(400).collect::<String>()),
        );
    }
    lines.split_off(lines.len().saturating_sub(40))
}

/// The last 40 lines of a log file (empty when it is missing).
fn tail(path: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    lines[lines.len().saturating_sub(40)..].to_vec()
}

/// One user turn to `/door`, the way the front door takes it; returns the
/// `message_id` of the 202, which is the turn's `trace_id` (a source message,
/// `meclaw-api` `handlers/messages.rs`): the identity a verdict binds to
/// (re-check fix3 M1 a). A hang names the last refusal (re-check fix3 H1) and
/// the daemon's state; an exited daemon ends the wait at once with its exit
/// status and stderr (tv-br M10g: the M11 reds said only `ConnectionRefused`).
async fn post_door(api: SocketAddr, text: &str, daemon: &Daemon) -> String {
    let turn = json!({
        "target": "/door",
        "headers": {"channel": "gh1061-organism"},
        "body": {"messages": [{"origin": "user", "type": "text", "text": text}]}
    });
    let client = reqwest::Client::new();
    let (client, turn) = (&client, &turn);
    let last = RefCell::new(String::from("no attempt yet"));
    let id = RefCell::new(None);
    let (last_seen, accepted) = (&last, &id);
    let what = format!("POST /messages {text:?} to /door is accepted");
    wait::until_with(
        &what,
        || {
            daemon.alive(&what);
            async move {
                match client
                    .post(format!("http://{api}/messages"))
                    .json(turn)
                    .send()
                    .await
                {
                    Ok(r) if r.status() == reqwest::StatusCode::ACCEPTED => {
                        let v: Value = r.json().await.unwrap_or(Value::Null);
                        let m = v["message_id"]
                            .as_str()
                            .unwrap_or_else(|| panic!("a 202 names its message_id: {v}"));
                        *accepted.borrow_mut() = Some(m.to_string());
                        true
                    }
                    Ok(r) => {
                        *last_seen.borrow_mut() = format!("status {}", r.status());
                        false
                    }
                    Err(e) => {
                        *last_seen.borrow_mut() = format!("{e:?}");
                        false
                    }
                }
            }
        },
        || {
            let mut state = vec![format!("last refusal: {}", last.borrow())];
            state.extend(daemon.state());
            state
        },
    )
    .await;
    id.into_inner().expect("an accepted turn has its id")
}

async fn healthy(api: SocketAddr) -> bool {
    reqwest::get(format!("http://{api}/health"))
        .await
        .is_ok_and(|r| r.status().is_success())
}

/// One declaration through the front door. A refused connection names the
/// daemon's own last words (GH #1216: four reds read only "error sending
/// request", with nothing to say why the daemon stopped answering) and, since
/// tv-br M10g, its exit status.
async fn grow(api: SocketAddr, rel: &str, daemon: &Daemon) {
    let body = std::fs::read_to_string(repo(rel)).unwrap();
    let reply = reqwest::Client::new()
        .post(format!("http://{api}/colony/mutations"))
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .unwrap_or_else(|e| {
            panic!(
                "POST /colony/mutations {rel}: {e:?}; last state: {:#?}",
                daemon.state()
            )
        })
        .text()
        .await
        .unwrap_or_else(|e| {
            panic!(
                "POST /colony/mutations {rel}: no reply body: {e:?}; last state: {:#?}",
                daemon.state()
            )
        });
    let v: Value = serde_json::from_str(&reply).unwrap_or(Value::Null);
    assert_eq!(
        v["mutation"]["outcome"],
        json!("committed"),
        "{rel} is not committed: {reply}"
    );
}

/// The second boot's replay as its own WARN line names it: `count=<n>` of
/// `replaying deliveries …`, once the line is in the daemon's stderr.
fn replay_count(log: &Path) -> Option<usize> {
    let text = std::fs::read_to_string(log.with_extension("err")).ok()?;
    let line = text.lines().find(|l| l.contains("replaying deliveries"))?;
    let rest = &line[line.find("count=")? + "count=".len()..];
    rest.split(|c: char| !c.is_ascii_digit())
        .next()
        .and_then(|n| n.parse().ok())
}

/// Rows per page of the message-log reads (re-check legacy-b fix4 M1, TV-A3):
/// `GET /colony/messages` resolves at most `RESOLVE_BLOB_MAX` = 25 blob bodies
/// per page (`meclaw-api` `handlers/message_log.rs`) and keeps only the blob id
/// of every further one, so a page of 1000 rows left an absence check blind
/// from its 26th blob on. 25 rows hold at most 25 blobs; [`assert_whole_page`]
/// asserts it anyway, should the cap ever change.
const LOG_PAGE: usize = 25;

/// The server's ceiling on `?scan_budget` (`MAX_SCAN_BUDGET`): with it a page
/// reports `scan_truncated` only past 50 000 rows of its window.
const LOG_SCAN_BUDGET: usize = 50_000;

/// A page of `GET /colony/messages` is whole, or the test fails here
/// (re-check legacy-b fix4 M1, tv-br M6 M1, TV-A3 fail-closed): the page says
/// whether it is whole (both flags are booleans -- a missing flag is no
/// `false`), the server resolved every blob body it was asked for
/// (`blob_resolution_truncated == false`), the scan saw its whole window
/// (`scan_truncated == false`), and no blob row of the page lacks its body: a
/// read the server could not do (`blob_error`, `handlers/message_log.rs`
/// `attach_blob_body`) leaves the flags `false` and the row without a body.
fn assert_whole_page(page: &Value) {
    let flags = format!(
        "blob_resolution_truncated={} scan_truncated={} scan_budget={} rows={}",
        page["blob_resolution_truncated"],
        page["scan_truncated"],
        page["scan_budget"],
        page["messages"].as_array().map_or(0, Vec::len)
    );
    assert!(
        page["blob_resolution_truncated"].is_boolean() && page["scan_truncated"].is_boolean(),
        "a message-log page does not say whether it is whole (no truncation flags), so it is \
         no record of the log: {flags}"
    );
    let unread: Vec<String> = page["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| {
            r["body_kind"] == "blob" && (r["blob_body"].is_null() || r.get("blob_error").is_some())
        })
        .map(|r| format!("{} blob_error={}", r["body_payload"], r["blob_error"]))
        .collect();
    assert!(
        page["blob_resolution_truncated"] == Value::Bool(false),
        "a message-log page left blob bodies unresolved (the server resolves 25 per page), \
         so the rows read are not the bodies logged: {flags}"
    );
    assert!(
        page["scan_truncated"] == Value::Bool(false),
        "a message-log page scanned only part of its window, so the rows read are not the \
         whole log: {flags}"
    );
    assert!(
        unread.is_empty(),
        "a message-log page holds blob rows whose body was not read, so the rows read are not \
         the bodies logged: {unread:?} ({flags})"
    );
}

/// The cursor of the page after `page`, or `None` at the log's end (re-check
/// tv-br M6 M1, TV-A3 fail-closed): the server names `next` only on a full page
/// (`colony_dispatch.rs` `handle_read_messages`), so `next == null` is the
/// end. A page without a `next` field, or one that names a next page without
/// a usable cursor (or on no rows), says nothing about the end and fails here
/// -- it never ends the walk as if the log were read.
fn next_cursor(page: &Value) -> Option<String> {
    let next = page
        .get("next")
        .unwrap_or_else(|| panic!("a message-log page does not say where the log goes on: {page}"));
    if next.is_null() {
        return None;
    }
    let rows = page["messages"].as_array().map_or(0, Vec::len);
    match (next["created_at"].as_i64(), next["id"].as_str()) {
        (Some(at), Some(id)) if rows > 0 => Some(format!("&before_created_at={at}&before_id={id}")),
        _ => panic!(
            "a message-log page names a next page without a usable cursor ({rows} rows, \
             next={next}), so its end is not the log's end"
        ),
    }
}

/// The colony's own lines for a message-log read it failed: on an SQL or
/// `spawn_blocking` error `handle_read_messages` logs one of them and answers
/// 200 with an empty page, no `next` and `scan_truncated == false`
/// (`colony_dispatch.rs`), which no page flag tells from the log's end.
const LOG_READ_ERRORS: [&str; 2] = [
    "ReadMessages SQL failed",
    "ReadMessages spawn_blocking failed",
];

/// Re-check tv-br M8 A-M2, orchestrator ruling OR-T10-7: the product source
/// [`LOG_READ_ERRORS`] is bound to, read at compile time -- a renamed line in
/// the colony turns this file red instead of letting a failed read pass as the
/// log's end.
const COLONY_DISPATCH_SRC: &str = include_str!("../../meclaw-colony/src/colony_dispatch.rs");

/// The `tracing::error!` messages of `handle_read_messages` in `src` and the
/// number of its empty fallbacks `(Vec::new(), 0)`.
fn read_error_lines_of(src: &str) -> (Vec<String>, usize) {
    let body = src
        .split_once("pub async fn handle_read_messages(")
        .map(|(_, rest)| rest.split_once("\n}\n").map_or(rest, |(b, _)| b))
        .expect("colony_dispatch.rs defines `pub async fn handle_read_messages(`");
    let messages = body
        .split("tracing::error!(")
        .skip(1)
        .map(|call| {
            let call = call.split_once(");").map_or(call, |(c, _)| c);
            let end = call.rfind('"').expect("a tracing::error! with a message");
            let start = call[..end].rfind('"').expect("a quoted message") + 1;
            call[start..end].to_string()
        })
        .collect();
    (messages, body.matches("(Vec::new(), 0)").count())
}

/// [`LOG_READ_ERRORS`] are exactly the error lines of `handle_read_messages`
/// in `src`, one per empty fallback: a renamed line, a new line or a new
/// silent fallback is a drift of the oracle (OR-T10-7).
fn assert_log_read_errors_bound(src: &str) {
    let (messages, fallbacks) = read_error_lines_of(src);
    assert_eq!(
        messages, LOG_READ_ERRORS,
        "the oracle's log-read errors are not the colony's own lines \
         (colony_dispatch.rs handle_read_messages)"
    );
    assert_eq!(
        fallbacks,
        LOG_READ_ERRORS.len(),
        "handle_read_messages answers an empty page on a path the oracle has no line for"
    );
}

/// Re-check tv-br M6 M1 (b), orchestrator ruling OR-T10-3: a failed read is
/// closed on the test side, the HTTP contract stays -- the stderr of the boot
/// that served the log reads holds none of [`LOG_READ_ERRORS`], or the rows
/// read are not the whole log.
fn assert_no_log_read_error(stderr: &str) {
    assert_log_read_errors_bound(COLONY_DISPATCH_SRC);
    let failed: Vec<&str> = stderr
        .lines()
        .filter(|l| LOG_READ_ERRORS.iter().any(|e| l.contains(e)))
        .collect();
    assert!(
        failed.is_empty(),
        "the colony failed a message-log read and served it as an empty page (200, no next), \
         so the rows read are not the whole log: {failed:?}"
    );
}

/// The message a check panics with, or `None` when it passes (TV-Q4: a pin
/// names the reason it expects, not only that something panicked).
fn refusal(check: impl FnOnce() + std::panic::UnwindSafe) -> Option<String> {
    std::panic::catch_unwind(check).err().map(|p| {
        p.downcast_ref::<String>()
            .cloned()
            .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_string()))
            .unwrap_or_else(|| "a panic without a message".into())
    })
}

/// `refused` is a refusal whose message holds `reason` (TV-Q4).
fn expect_refusal(refused: Option<String>, reason: &str, case: &str) {
    let msg = refused.unwrap_or_else(|| panic!("{case}: the oracle took it as whole"));
    assert!(
        msg.contains(reason),
        "{case}: refused, but not for `{reason}`: {msg}"
    );
}

/// Every row of the colony's message log as the colony serves it, newest first
/// (TV-A3, re-check fix2 M3): `GET /colony/messages`, the HTTP twin of
/// `ReadMessages`, paged by its keyset cursor, every page whole
/// ([`assert_whole_page`]: all blob bodies resolved, the whole window scanned); with
/// `filter` (`&trace_id=<id>`) the rows of one trace. No database file is
/// opened. A failed request is the caller's to report with the daemon's state
/// (tv-br M10g): `Err` names it, it panics no more without one.
async fn message_log(api: SocketAddr, filter: &str) -> Result<Vec<Value>, String> {
    let client = reqwest::Client::new();
    let mut rows = Vec::new();
    let mut cursor = String::new();
    loop {
        let page: Value = client
            .get(format!(
                "http://{api}/colony/messages?limit={LOG_PAGE}&scan_budget={LOG_SCAN_BUDGET}\
                 &resolve_blob=true{filter}{cursor}"
            ))
            .send()
            .await
            .map_err(|e| format!("GET /colony/messages: {e:?}"))?
            .json()
            .await
            .map_err(|e| format!("GET /colony/messages: no message page: {e:?}"))?;
        assert_whole_page(&page);
        let messages = page["messages"].as_array().cloned().unwrap_or_default();
        rows.extend(messages.iter().cloned());
        match next_cursor(&page) {
            Some(next) => cursor = next,
            None => return Ok(rows),
        }
    }
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

    // GH #1220: a port from the test band, not a dropped `bind(…:0)` -- while the
    // daemon (re)starts, the kernel gave such a port to another test's mock,
    // and this test's /health and /messages requests landed there.
    let api: SocketAddr = format!("127.0.0.1:{}", meclaw_testing::ports::free_port())
        .parse()
        .unwrap();
    let mut transcript = String::new();

    // 1. The first boot grows the shell; 2. the four declarations, verbatim.
    let first = logs.path().join("first");
    let daemon = start_daemon(&root, home.path(), library.path(), api, &first);
    // The daemon's own words, so a red boot names its reason (the first red
    // run of this lock said only "never happened").
    wait::boot_until_with(
        "the first boot answers /health",
        || {
            daemon.alive("the first boot answers /health");
            healthy(api)
        },
        || daemon.state(),
    )
    .await;
    for rel in DECLARATIONS {
        grow(api, rel, &daemon).await;
    }
    // GH #1216 (root 2): the deposit stops a colony that is still working --
    // the growth starts on_start rounds, intake and embeds, and the second
    // boot replays what the stop left open (35 deliveries in the red run of
    // 12.10.). That stop in the middle of the work stays part of this road and
    // is made certain here: the stub parks every provider call, two turns go
    // in before the key, and the stop comes once a call is parked. Neither
    // turn can finish while every provider call is parked, so the stop leaves
    // both open (the shutdown drain cuts the parked handler), and the second
    // boot replays at least two deliveries (re-check fix2 M1 b).
    let parked = stub.parked();
    stub.hold();
    let first_turn = post_door(api, FIRST_TURN, &daemon).await;
    let second_turn = post_door(api, SECOND_TURN, &daemon).await;
    deadline::boot_hang_with(
        "a provider call of the first boot is parked",
        stub.event(|s| s.parked() > parked),
        || daemon.state(),
    )
    .await;

    // 3. Stopped by its pid; the key goes in on stdin, never in argv.
    stop_by_pid(&daemon).await;
    stub.release();
    let mut add = meclaw(&root, home.path());
    add.args(["--vault", MEMBER_VAULT, "--vault-add", CRED_REF])
        .args(["--vault-key-source", "plainfile", "--vault-key-file"])
        .arg(&key_file);
    let (ok, text) = run(add, SECRET).await;
    assert!(
        ok,
        "--vault-add {CRED_REF} into {MEMBER_VAULT} stores: {text}"
    );
    transcript.push_str(&text);

    // 4. The same daemon again. Every provider call parks again, so the
    // replay is certainly still in front of the brains when this boot answers
    // /health.
    stub.hold();
    let second = logs.path().join("second");
    let daemon = start_daemon(&root, home.path(), library.path(), api, &second);
    wait::boot_until_with(
        "the restart answers /health",
        || {
            daemon.alive("the restart answers /health");
            healthy(api)
        },
        || daemon.state(),
    )
    .await;
    // The stop left work open, and this boot replays it; how much is the
    // protocol line below (the count says nothing about WHICH deliveries --
    // the two traces below do, re-check fix3 M1 d).
    let mut replayed = None;
    wait::boot_until_with(
        "the second boot replays what the stop left open",
        || {
            daemon.alive("the second boot replays what the stop left open");
            replayed = replay_count(&second);
            std::future::ready(replayed.is_some())
        },
        || daemon.state(),
    )
    .await;
    let replayed = replayed.unwrap_or_default();

    // Re-check fix3 M1: the verdict is a row of the
    // FIRST TURN's own trace -- the id its POST got back -- that is a brain's
    // answer to a stub request with the deposited bearer, not a count or a
    // body of indistinguishable requests. The second turn's trace is held to
    // the same, so "both turns stay open over the stop" is bound to the two
    // traces, not to a replay count. FIFO replay puts the turns behind exactly
    // the work the stop left open -- the reader's road after
    // `deposit-key.sh` -- so the wait is the replay hang mark (REPLAY_HANG,
    // >= 3 x the P95 of the protocol line below under a CPU quota, GH #1216).
    let want = format!("Bearer {SECRET}");
    stub.release();
    let t0 = Instant::now();
    let mut answers = Vec::new();
    for (turn, trace) in [(FIRST_TURN, &first_turn), (SECOND_TURN, &second_turn)] {
        let filter = format!("&trace_id={trace}");
        let last = RefCell::new(Vec::new());
        let read_failed = RefCell::new(None);
        let (filter, rows_seen, failed, stub, want) = (&filter, &last, &read_failed, &stub, &want);
        let what = format!(
            "the replayed turn {turn:?} (trace {trace}) holds a brain's answer to a request \
             with the deposited bearer"
        );
        wait::replay_until_with(
            &what,
            || {
                daemon.alive(&what);
                async move {
                    match message_log(api, filter).await {
                        Ok(rows) => {
                            let held = answered_with_key(&stub.seen(), &rows, trace, want);
                            *rows_seen.borrow_mut() = rows;
                            *failed.borrow_mut() = None;
                            held
                        }
                        Err(e) => {
                            *failed.borrow_mut() = Some(e);
                            false
                        }
                    }
                }
            },
            || {
                let mut state = stub.state(want);
                state.extend(trace_state(&rows_seen.borrow()));
                state.push(format!("last log read failed: {:?}", read_failed.borrow()));
                state.extend(daemon.state());
                state
            },
        )
        .await;
        answers.push(keyed_answers(&stub.seen(), &last.borrow(), trace, want));
    }
    // Re-check legacy-b fix4 A-H1: "both turns" are two stub requests, never
    // one answer counted for both traces.
    assert!(
        two_turns_answered(&answers[0], &answers[1]),
        "the two turns are answered with the key by different stub requests: first turn \
         {:?}, second turn {:?}",
        answers[0],
        answers[1]
    );
    eprintln!(
        "gh1061 organism: the second boot replayed {replayed} deliveries; the first \
         turn's brain request carried the key {} ms after the release",
        t0.elapsed().as_millis()
    );

    // A fresh turn after the replay: protocol only, no verdict hangs on it
    // (re-check fix2 M1 c) -- whether a keyed brain answered its trace before
    // the stop, read from the whole log below.
    let ping = post_door(api, "ping", &daemon).await;
    let foreign: Vec<String> = stub
        .authorizations()
        .into_iter()
        .filter(|a| *a != want && a.trim() != "Bearer")
        .collect();
    assert!(
        foreign.is_empty(),
        "every bearer the provider saw is the deposited one: {foreign:?}"
    );

    // The value is in no log this test can read. Re-check fix3 M2: the
    // message log is read AFTER the orderly stop -- which empties the log
    // writer -- by a third boot over `GET /colony/messages`, so it is whole
    // (the basis read it after the stop too; a read before the stop missed
    // the rows still in the writer's queue).
    stop_by_pid(&daemon).await;
    // Re-check tv-br M8 A-M1 (OR-T10-3): the second boot served the per-trace
    // reads the two-turns verdict stands on (`keyed_answers`, `is_disjoint`) --
    // a read it failed is an empty page, so its stderr holds no failed read.
    assert_no_log_read_error(
        &std::fs::read_to_string(second.with_extension("err")).expect("the second boot's stderr"),
    );
    let third = logs.path().join("third");
    let reader = start_daemon(&root, home.path(), library.path(), api, &third);
    wait::boot_until_with(
        "the third boot answers /health",
        || {
            reader.alive("the third boot answers /health");
            healthy(api)
        },
        || reader.state(),
    )
    .await;
    let rows = deadline::hang_with(
        "GET /colony/messages serves the whole log",
        message_log(api, ""),
        || reader.state(),
    )
    .await
    .unwrap_or_else(|e| panic!("{e}; last state: {:#?}", reader.state()));
    stop_by_pid(&reader).await;
    assert_no_log_read_error(
        &std::fs::read_to_string(third.with_extension("err")).expect("the third boot's stderr"),
    );
    eprintln!(
        "gh1061 organism: protocol: the ping's trace holds a keyed brain answer: {}",
        answered_with_key(&stub.seen(), &rows, &ping, &want)
    );
    // GH #1216: the second boot's broker denials, for the record of a green
    // run (the shell's own interior vault never opens, so its translator's
    // round is denied `vault_locked` in every run; GH #1092).
    let denials: Vec<String> = std::fs::read_to_string(second.with_extension("err"))
        .unwrap_or_default()
        .lines()
        .filter(|l| l.contains("denied"))
        .map(|l| l.chars().take(200).collect())
        .collect();
    eprintln!("gh1061 organism: second boot denials: {denials:?}");
    // tv-br M10h: every watchdog trip of the three boots, fatal or not, for
    // the record of a green run -- a `slow_work_item` here is the near miss of
    // the M11 red (TV-F2: the trip's evidence, not a guess).
    for log in [&first, &second, &third] {
        let trips: Vec<String> = std::fs::read_to_string(log.with_extension("err"))
            .unwrap_or_default()
            .lines()
            .filter(|l| l.contains("heartbeat lost"))
            .map(|l| l.chars().take(400).collect())
            .collect();
        eprintln!(
            "gh1061 organism: watchdog trips of {}: {trips:?}",
            log.display()
        );
    }
    for log in [&first, &second, &third] {
        for ext in ["out", "err"] {
            transcript
                .push_str(&std::fs::read_to_string(log.with_extension(ext)).unwrap_or_default());
        }
    }
    transcript.push_str(&std::fs::read_to_string(root.join("log.jsonl")).unwrap_or_default());
    assert!(!rows.is_empty(), "the colony logged its traffic");
    assert!(!transcript.contains(SECRET), "the key is in a process log");
    let hits: Vec<String> = rows
        .iter()
        .map(Value::to_string)
        .filter(|r| r.contains(SECRET))
        .collect();
    assert!(hits.is_empty(), "the key is on record: {hits:?}");
    assert!(
        !transcript.contains(PASSPHRASE),
        "the passphrase is in a log"
    );
}

// ──────────────────────────────────────────────────────── the verdict's pins

/// One request to the stub with an optional bearer: a chat completion whose
/// LAST user message is `text` (after `history`, if any), or an embedding.
/// Returns the `n` the stub answered it with (its reply id names it).
async fn call(stub: &Stub, path: &str, bearer: Option<&str>, history: &str, text: &str) -> usize {
    let body = if path.ends_with("/embeddings") {
        json!({"model": "stub-model", "input": text})
    } else {
        let mut messages = vec![json!({"role": "system", "content": "a brain"})];
        if !history.is_empty() {
            messages.push(json!({"role": "user", "content": history}));
            messages.push(json!({"role": "assistant", "content": "pong"}));
        }
        messages.push(json!({"role": "user", "content": [{"type": "text", "text": text}]}));
        json!({"model": "stub-model", "messages": messages})
    };
    let mut req = reqwest::Client::new()
        .post(format!("http://{}{path}", stub.addr))
        .json(&body);
    if let Some(b) = bearer {
        req = req.header("Authorization", b);
    }
    let reply = deadline::hang("the stub answers", req.send())
        .await
        .expect("the stub answers");
    assert!(reply.status().is_success(), "the stub answers 200");
    let v: Value = reply.json().await.expect("a chat completion");
    v["id"]
        .as_str()
        .and_then(|id| id.strip_prefix("chatcmpl-"))
        .and_then(|n| n.parse().ok())
        .expect("the reply id names the request")
}

/// A message-log row of `trace` with an inline `body`, as
/// `GET /colony/messages` serves it.
fn row(trace: &str, body: Value) -> Value {
    json!({"trace_id": trace, "from_path": "/x", "to_path": "/y",
           "body_kind": "inline", "body_payload": body.to_string()})
}

/// A brain's output row in `trace` answering stub request `n`
/// (`llm/output.rs` `emit_assistant_turn`: the reply, then `meta`).
fn brain_row(trace: &str, n: usize) -> Value {
    row(
        trace,
        json!({"messages": [{"origin": "assistant", "type": "text", "text": answer(n)}],
               "meta": {"provider": "openrouter", "response_id": response_id(n)}}),
    )
}

const TRACE_FIRST: &str = "0199c0de-0000-7000-8000-000000000001";
const TRACE_SECOND: &str = "0199c0de-0000-7000-8000-000000000002";
const TRACE_OTHER: &str = "0199c0de-0000-7000-8000-000000000003";

/// Re-check fix2 M1 (a, d), fix3 M1: the verdict binds to the first turn's
/// OWN trace. A keyed answer in another delivery's trace (the old "any
/// bearer" event was green on it), the first turn's text in another trace's
/// request, an answer to its embedding call or to its brain call without the
/// key, the wall's history (a reply text without a reply id) or a reply id
/// whose answer is not the stub's -- none of these satisfies it; only a
/// brain's answer to a keyed chat request in that trace does, inline or as a
/// resolved blob.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1061_organism_the_verdict_binds_to_the_first_turns_own_request() {
    let stub = Stub::start();
    let want = format!("Bearer {SECRET}");
    let chat = "/v1/chat/completions";
    let embed = "/v1/embeddings";
    let mut rows = Vec::new();
    let held = |rows: &[Value]| answered_with_key(&stub.seen(), rows, TRACE_FIRST, &want);

    let growth = call(&stub, chat, Some(&want), "", "a replayed growth round").await;
    rows.push(brain_row(TRACE_OTHER, growth));
    assert!(
        !held(&rows),
        "a keyed answer in another delivery's trace is no verdict on the first turn"
    );
    let later = call(&stub, chat, Some(&want), FIRST_TURN, SECOND_TURN).await;
    rows.push(brain_row(TRACE_SECOND, later));
    assert!(
        !held(&rows),
        "the first turn in another trace's request is not its own"
    );
    let embedding = call(&stub, embed, Some(&want), "", FIRST_TURN).await;
    rows.push(brain_row(TRACE_FIRST, embedding));
    assert!(
        !held(&rows),
        "an answer to the first turn's embedding call is no brain request"
    );
    let unkeyed = call(&stub, chat, None, "", FIRST_TURN).await;
    let empty = call(&stub, chat, Some("Bearer"), "", FIRST_TURN).await;
    rows.push(brain_row(TRACE_FIRST, unkeyed));
    rows.push(brain_row(TRACE_FIRST, empty));
    assert!(
        !held(&rows),
        "an answer to the first turn's brain request without the key is no verdict"
    );
    rows.push(row(
        TRACE_FIRST,
        json!({"messages": [
            {"origin": "assistant", "type": "text", "text": answer(later)},
            {"origin": "user", "type": "text", "text": FIRST_TURN}]}),
    ));
    rows.push(row(
        TRACE_FIRST,
        json!({"messages": [{"origin": "assistant", "type": "text", "text": "pong"}],
               "meta": {"response_id": response_id(later)}}),
    ));
    assert!(
        !held(&rows),
        "the wall's history or a reply id without the stub's answer is no answer"
    );
    let own = call(&stub, chat, Some(&want), "", FIRST_TURN).await;
    let mut blob = brain_row(TRACE_FIRST, own);
    blob["body_kind"] = json!("blob");
    blob["blob_body"] = serde_json::from_str(blob["body_payload"].as_str().unwrap()).unwrap();
    blob["body_payload"] = json!("0199c0de-0000-7000-8000-0000000000b1");
    rows.push(blob);
    assert!(
        held(&rows),
        "a brain's answer to a keyed chat request in the first turn's trace is the verdict"
    );
}

/// Re-check fix3 M1 (e), the counterexample: the first turn is lost over the
/// stop and the second turn's request goes out twice (a doubled delivery or
/// the `llm` retry ladder) -- two keyed requests, byte for byte the window of
/// both turns, `[first, second]`, whose answers land in the second turn's
/// trace only. That is NO verdict on the first turn (the counting predicate of
/// Bauer H took it for one); a keyed answer in the first turn's own trace is.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1061_organism_a_doubled_second_turn_is_no_verdict_on_the_first() {
    let stub = Stub::start();
    let want = format!("Bearer {SECRET}");
    let chat = "/v1/chat/completions";
    let first = format!("[#2b20375b5e36] {FIRST_TURN}");
    let second = format!("[#5e09bfe2e1ce] {SECOND_TURN}");

    let a = call(&stub, chat, Some(&want), &first, &second).await;
    let b = call(&stub, chat, Some(&want), &first, &second).await;
    let mut rows = vec![brain_row(TRACE_SECOND, a), brain_row(TRACE_SECOND, b)];
    assert!(
        !answered_with_key(&stub.seen(), &rows, TRACE_FIRST, &want),
        "two keyed requests on one wall answered in the second turn's trace hold \
         nothing of the first turn"
    );
    assert!(
        answered_with_key(&stub.seen(), &rows, TRACE_SECOND, &want),
        "they are the second turn's answers"
    );
    // Re-check legacy-b fix4 A-H1: the control is a request of its own; the
    // second turn's answer `a` in the first trace counts for that trace, but
    // never for both turns at once.
    let mut shared = rows.clone();
    shared.push(brain_row(TRACE_FIRST, a));
    let of = |rows: &[Value], trace| keyed_answers(&stub.seen(), rows, trace, &want);
    assert!(
        !two_turns_answered(&of(&shared, TRACE_FIRST), &of(&shared, TRACE_SECOND)),
        "one stub answer in both traces is not both turns answered"
    );
    let own = call(&stub, chat, Some(&want), &first, &second).await;
    rows.push(brain_row(TRACE_FIRST, own));
    assert!(
        answered_with_key(&stub.seen(), &rows, TRACE_FIRST, &want),
        "a keyed answer in the first turn's own trace is the verdict"
    );
    assert!(
        two_turns_answered(&of(&rows, TRACE_FIRST), &of(&rows, TRACE_SECOND)),
        "two keyed requests, one answered in each trace, are both turns answered"
    );
}

/// A message-log page of `blobs` blob rows as `GET /colony/messages` serves it
/// (`handlers/message_log.rs`): the first 25 with their body resolved, every
/// further one with its blob id only, and the page's flag says so.
fn page_of_blobs(blobs: usize) -> Value {
    let messages: Vec<Value> = (0..blobs)
        .map(|i| {
            let mut row = json!({"body_kind": "blob", "body_payload": format!("blob-{i}")});
            if i < 25 {
                row["blob_body"] = json!({"messages": [{"text": format!("body {i}")}]});
            }
            row
        })
        .collect();
    json!({"messages": messages, "next": null, "scan_budget": LOG_SCAN_BUDGET,
           "scan_truncated": false, "blob_resolution_truncated": blobs > 25})
}

/// Re-check legacy-b fix4 M1, tv-br M6 M2 (TV-Q4): the page check of the
/// message-log oracle holds for a page whose every blob body is resolved and
/// refuses, each for its own reason, a page from its 26th blob on, a
/// truncated scan and a page without its flags.
#[test]
fn gh1061_organism_the_log_oracle_refuses_a_page_past_its_25th_blob() {
    let refused = |page: Value| refusal(move || assert_whole_page(&page));
    assert_eq!(
        refused(page_of_blobs(25)),
        None,
        "25 resolved blob bodies are a whole page"
    );
    expect_refusal(
        refused(page_of_blobs(26)),
        "left blob bodies unresolved",
        "the 26th blob of a page is unresolved",
    );
    let mut scanned = page_of_blobs(3);
    scanned["scan_truncated"] = json!(true);
    expect_refusal(
        refused(scanned),
        "scanned only part of its window",
        "a truncated scan",
    );
    expect_refusal(
        refused(json!({"messages": []})),
        "does not say whether it is whole",
        "a page without its flags",
    );
}

/// Re-check tv-br M6 M1 (a): a blob row whose body the server could not read
/// (`blob_error`: `missing_blob_id`, `malformed_blob_id`, `blob_unreadable`,
/// `handlers/message_log.rs` `attach_blob_body`) or did not resolve carries
/// no body, and the page flags stay `false` -- the oracle refuses the page.
#[test]
fn gh1061_organism_the_log_oracle_refuses_a_blob_row_without_its_body() {
    for error in ["missing_blob_id", "malformed_blob_id", "blob_unreadable"] {
        let mut page = page_of_blobs(3);
        page["messages"][1]
            .as_object_mut()
            .expect("a row")
            .remove("blob_body");
        page["messages"][1]["blob_error"] = json!(error);
        expect_refusal(
            refusal(move || assert_whole_page(&page)),
            "whose body was not read",
            error,
        );
    }
    let mut unresolved = page_of_blobs(3);
    unresolved["messages"][2]
        .as_object_mut()
        .expect("a row")
        .remove("blob_body");
    expect_refusal(
        refusal(move || assert_whole_page(&unresolved)),
        "whose body was not read",
        "a blob row without body and without error",
    );
}

/// Re-check tv-br M6 M1: the walk over the log ends only at `next == null`;
/// a page that names a next page it gives no cursor for, or a page without a
/// `next` field, is no end of the log.
#[test]
fn gh1061_organism_the_log_oracle_ends_only_at_a_null_next() {
    let mut full = page_of_blobs(3);
    full["next"] = json!({"created_at": 7, "id": "row-3"});
    assert_eq!(
        next_cursor(&full).as_deref(),
        Some("&before_created_at=7&before_id=row-3"),
        "a full page names its successor"
    );
    assert_eq!(
        next_cursor(&page_of_blobs(3)),
        None,
        "next == null is the end"
    );
    let mut broken = page_of_blobs(3);
    broken["next"] = json!({"created_at": 7});
    expect_refusal(
        refusal(move || drop(next_cursor(&broken))),
        "without a usable cursor",
        "a next page without an id",
    );
    let mut empty = json!({"messages": [], "next": {"created_at": 7, "id": "row-3"}});
    empty["scan_truncated"] = json!(false);
    expect_refusal(
        refusal(move || drop(next_cursor(&empty))),
        "without a usable cursor",
        "a next page named on no rows",
    );
    let mut silent = page_of_blobs(3);
    silent.as_object_mut().expect("a page").remove("next");
    expect_refusal(
        refusal(move || drop(next_cursor(&silent))),
        "does not say where the log goes on",
        "a page without a next field",
    );
}

/// Re-check tv-br M6 M1 (b), OR-T10-3: a read the colony failed is a 200 with
/// an empty page and no `next` -- the oracle refuses it by the colony's own
/// error line on the reading boot's stderr; other lines pass.
#[test]
fn gh1061_organism_the_log_oracle_refuses_a_log_read_the_colony_failed() {
    assert_eq!(
        refusal(|| assert_no_log_read_error(
            "2026-10-12T10:00:00Z  WARN meclaw_colony: replaying deliveries n=1\n"
        )),
        None,
        "a stderr without a failed read passes"
    );
    for line in LOG_READ_ERRORS {
        let stderr = format!(
            "2026-10-12T10:00:00Z ERROR meclaw_colony::colony_dispatch: {line} error=SqliteFailure\n"
        );
        expect_refusal(
            refusal(move || assert_no_log_read_error(&stderr)),
            "failed a message-log read",
            line,
        );
    }
}

/// Re-check tv-br M8 A-M2, OR-T10-7: the oracle's log-read errors are the
/// colony's own lines -- the shipped source passes; a renamed line and a new
/// silent fallback in a copy of it are refused.
#[test]
fn gh1061_organism_the_log_read_errors_are_the_colony_s_own_lines() {
    assert_eq!(
        refusal(|| assert_log_read_errors_bound(COLONY_DISPATCH_SRC)),
        None,
        "the shipped colony_dispatch.rs is the oracle's source"
    );
    let renamed = COLONY_DISPATCH_SRC.replacen(
        "\"ReadMessages SQL failed\"",
        "\"ReadMessages query failed\"",
        1,
    );
    expect_refusal(
        refusal(move || assert_log_read_errors_bound(&renamed)),
        "not the colony's own lines",
        "a renamed error line",
    );
    let silent = COLONY_DISPATCH_SRC.replacen(
        "    let next = if entries.len() == limit {",
        "    let (entries, scanned) = if scanned > 0 { (entries, scanned) } else { (Vec::new(), 0) };\n    let next = if entries.len() == limit {",
        1,
    );
    assert_ne!(silent, COLONY_DISPATCH_SRC, "the mutation applies");
    expect_refusal(
        refusal(move || assert_log_read_errors_bound(&silent)),
        "a path the oracle has no line for",
        "a silent empty fallback",
    );
}

/// Re-check tv-br M6 M1 (a): a blob row whose body was not read is no body --
/// `row_body` refuses it instead of reading it as `null`, the same for a row
/// of a kind it does not know; an inline row and a resolved blob row read.
#[test]
fn gh1061_organism_a_blob_row_without_its_body_is_no_body() {
    let inline = row(TRACE_FIRST, json!({"messages": []}));
    assert_eq!(row_body(&inline), json!({"messages": []}), "an inline row");
    let resolved = json!({"body_kind": "blob", "body_payload": "blob-1",
                          "blob_body": {"messages": []}});
    assert_eq!(
        row_body(&resolved),
        json!({"messages": []}),
        "a resolved blob row"
    );
    let unread = json!({"body_kind": "blob", "body_payload": "blob-1",
                        "blob_error": "blob_unreadable"});
    expect_refusal(
        refusal(move || drop(row_body(&unread))),
        "without its body",
        "a blob row with blob_error",
    );
    let unknown = json!({"body_kind": "carrier-pigeon", "body_payload": "{}"});
    expect_refusal(
        refusal(move || drop(row_body(&unknown))),
        "of an unknown body_kind",
        "a row of an unknown kind",
    );
}

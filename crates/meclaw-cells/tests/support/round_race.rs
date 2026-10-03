//! The member road of GH #929 (`support/gh929_member_road.rs`), set up for the
//! races of a round change (GH #953, #954): a brain that answers every turn
//! with a reply NAMED after the turn, so a close batch can be searched for one
//! turn's answer, a closer that files nothing, and the readers of the
//! colony's own `message_log` and of the keeper's and the curator's stores.
//!
//! The including test file declares, at its root,
//! `#[path = "mock_openai.rs"] mod mock_openai;` and
//! `#[path = "support/gh929_member_road.rs"] mod road;`.
#![allow(dead_code)]

use crate::mock_openai::{MockOpenAI, canned_chat_completion};
use crate::road;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, MESSAGE_DEFAULT_TTL, Message, MessageBuilder, Path};
use meclaw_testing::mock_http::{
    CapturedRequest, RequestValidator, start_mock_server_capturing_with_validator,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The bound on a close pass or a store row, a failure marker and not a
/// timing: the pass runs behind the turn, through the curator, the memory and
/// the closer.
pub const PASS_DEADLINE: Duration = Duration::from_secs(90);

/// The generation's talky, its keeper's store and its curator's cells.
pub const TALKY: &str = "/assistants/scribe/talky";
pub const WRITER: &str = "/assistants/scribe/talky/curator/writer";
pub const LEDGER: &str = "/assistants/scribe/talky/curator/ledger";
pub const INTAKE: &str = "/assistants/scribe/talky/curator/intake";

/// The prefix a turn's words carry its tag under, and the prefix of the reply
/// the brain names after it.
pub const SAY: &str = "SAY-";
pub const REPLY: &str = "REPLY-";

pub fn say(tag: &str) -> String {
    format!("{SAY}{tag} is said here.")
}

pub fn reply(tag: &str) -> String {
    format!("{REPLY}{tag} is the answer.")
}

fn content_of(m: &Value) -> String {
    match &m["content"] {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// The tag of the LAST `SAY-` in the last user message of a brain request:
/// the turn this call answers (the window before it may name older turns).
fn tag_of(body: &Value) -> Option<String> {
    let text = body["messages"]
        .as_array()?
        .iter()
        .rev()
        .find(|m| m["role"] == "user")
        .map(content_of)?;
    let at = text.rfind(SAY)? + SAY.len();
    let tag: String = text[at..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    (!tag.is_empty()).then_some(tag)
}

/// The brain: every request is answered with [`reply`] of the turn it
/// carries, and its whole body is kept under that tag (the window a turn was
/// answered with, GH #954).
pub struct Brain {
    pub base_url: String,
    pub seen: Arc<Mutex<HashMap<String, String>>>,
}

pub async fn start_brain() -> Brain {
    let seen: Arc<Mutex<HashMap<String, String>>> = Arc::default();
    let keep = seen.clone();
    let validator: RequestValidator = Arc::new(move |req: &CapturedRequest| {
        let body: Value = meclaw_core::serde_json::from_slice(&req.body).ok()?;
        let tag = tag_of(&body)?;
        keep.lock()
            .unwrap()
            .insert(tag.clone(), String::from_utf8_lossy(&req.body).to_string());
        Some(canned_chat_completion(&reply(&tag), "stop"))
    });
    let (addr, _join, _captured) = start_mock_server_capturing_with_validator(
        vec![canned_chat_completion(
            "REPLY-untagged is the answer.",
            "stop",
        )],
        Some(validator),
    )
    .await;
    Brain {
        base_url: format!("http://{addr}"),
        seen,
    }
}

/// The road booted with the naming brain, a closer that files nothing and the
/// background stub for every other `llm` cell.
pub async fn boot_road() -> (
    tempfile::TempDir,
    meclaw_testing::ColonyHandle,
    road::Ports,
    Brain,
) {
    let brain = start_brain().await;
    let (td, h, ports) = boot_road_with(&brain.base_url).await;
    (td, h, ports, brain)
}

/// The road booted with the brain at `brain_url` (the naming [`Brain`] or the
/// [`HeldBrain`]), a closer that files nothing and the background stub for
/// every other `llm` cell.
pub async fn boot_road_with(
    brain_url: &str,
) -> (tempfile::TempDir, meclaw_testing::ColonyHandle, road::Ports) {
    let nothing = json!({"nothing_to_add": true, "add": [], "sharpen": [], "correct": [],
                         "close_topics": []});
    let closer =
        MockOpenAI::start(vec![canned_chat_completion(&nothing.to_string(), "stop")]).await;
    let background =
        MockOpenAI::start(vec![canned_chat_completion(road::BACKGROUND_REPLY, "stop")]).await;
    let brain_cell = format!("{}/talky/brain", road::GENERATION.trim_start_matches('/'));
    let stubs = road::Stubs {
        scripted: HashMap::from([
            (brain_cell, brain_url.to_string()),
            ("memory-hive/closer".to_string(), closer.base_url.clone()),
        ]),
        background: background.base_url.clone(),
    };
    let td = tempfile::TempDir::new().expect("tempdir");
    let pointed = road::build_member(&td, &stubs);
    for cell in stubs.scripted.keys() {
        assert!(
            pointed.contains(cell),
            "{cell} is not an llm cell of the road: {pointed:?}"
        );
    }
    let (h, ports) = road::boot(&td).await;
    // The stubs live as long as the servers' tasks; the handles may go.
    std::mem::forget(closer);
    std::mem::forget(background);
    (td, h, ports)
}

/// The brain of GH #953's late-answer lock: it answers like [`Brain`] (a
/// reply named after the turn), except that a request under a tag the test
/// asked to [`HeldBrain::hold`] is NOT answered until the test calls
/// [`HeldBrain::release`].
///
/// WHY a held request and not a delay: the lock is about the ORDER of two
/// events -- the seal of a generation and the last answer of the turn before
/// it -- and a delay is a window the scheduler may or may not respect. The
/// held request is answered after an event the test has OBSERVED (the sealed
/// row), so "the answer came after the seal" is true by construction in every
/// run, not with some probability (Review I-3, OR-NL-163).
///
/// It is built on `meclaw_testing::mock_http::start_mock_server_held`, which
/// parks every connection on a oneshot; the synchronous `RequestValidator`
/// could not wait without blocking a runtime worker. A dispatcher task answers
/// every request that is not held at once.
pub struct HeldBrain {
    pub base_url: String,
    pub seen: Arc<Mutex<HashMap<String, String>>>,
    hold: Arc<Mutex<std::collections::HashSet<String>>>,
    held: Arc<Mutex<HashMap<String, meclaw_testing::mock_http::HeldRequest>>>,
    arrivals: tokio::sync::watch::Sender<usize>,
}

pub async fn start_held_brain() -> HeldBrain {
    let (addr, _join, mut rx) = meclaw_testing::mock_http::start_mock_server_held()
        .await
        .expect("the held brain binds");
    let seen: Arc<Mutex<HashMap<String, String>>> = Arc::default();
    let hold: Arc<Mutex<std::collections::HashSet<String>>> = Arc::default();
    let held: Arc<Mutex<HashMap<String, meclaw_testing::mock_http::HeldRequest>>> = Arc::default();
    let (arrivals, _) = tokio::sync::watch::channel(0usize);
    let (seen_t, hold_t, held_t, arrivals_t) =
        (seen.clone(), hold.clone(), held.clone(), arrivals.clone());
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            let tag = tag_of(&req.json());
            let Some(tag) = tag else {
                req.release(canned_chat_completion(
                    "REPLY-untagged is the answer.",
                    "stop",
                ));
                continue;
            };
            seen_t.lock().unwrap().insert(
                tag.clone(),
                String::from_utf8_lossy(&req.request.body).to_string(),
            );
            if hold_t.lock().unwrap().remove(&tag) {
                held_t.lock().unwrap().insert(tag, req);
                arrivals_t.send_modify(|n| *n += 1);
            } else {
                req.release(canned_chat_completion(&reply(&tag), "stop"));
            }
        }
    });
    HeldBrain {
        base_url: format!("http://{addr}"),
        seen,
        hold,
        held,
        arrivals,
    }
}

impl HeldBrain {
    /// The next brain request under `tag` is held (once; a second request
    /// under the same tag is answered at once).
    pub fn hold(&self, tag: &str) {
        self.hold.lock().unwrap().insert(tag.to_string());
    }

    /// Until the brain was asked under `tag` and holds that request -- the
    /// event "the turn reached the brain and its answer is not out".
    pub async fn until_asked(&self, tag: &str) {
        let mut arrivals = self.arrivals.subscribe();
        let deadline = Instant::now() + PASS_DEADLINE;
        loop {
            if self.held.lock().unwrap().contains_key(tag) {
                return;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(
                tokio::time::timeout(left, arrivals.changed()).await.is_ok(),
                "the brain was not asked under {tag} within {PASS_DEADLINE:?}"
            );
        }
    }

    /// Answer the held request under `tag` with [`reply`] of `tag`, now.
    pub fn release(&self, tag: &str) {
        let req = self
            .held
            .lock()
            .unwrap()
            .remove(tag)
            .unwrap_or_else(|| panic!("no held brain request under {tag}"));
        req.release(canned_chat_completion(&reply(tag), "stop"));
    }
}

/// A person's words at the container's door on `channel`, in `round`.
pub fn turn(channel: &str, tag: &str, round: &str) -> Message {
    MessageBuilder::new(Path::new("/assistants"))
        .hop(road::as_map(&json!({"route": "in_turn"})))
        .context(road::as_map(
            &json!({"assistant": "scribe", "channel": channel,
                                       "audience_set": round}),
        ))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": say(tag)}]}),
        ))
        .ttl(MESSAGE_DEFAULT_TTL - road::ABOVE_THE_CONTAINER)
        .build()
}

/// [`turn`] with the id a channel door stamps on every turn it accepts
/// (`context.turn_id`, display-hive.md 8.2): `turn-<tag>`. The keeper records
/// it as the turn its generation owes an answer to (GH #953), so a lock about
/// the last answer of a generation needs a turn that carries one -- a turn
/// without an id owes nothing, and its generation closes as it did before.
pub fn turn_with_id(channel: &str, tag: &str, round: &str) -> Message {
    let mut m = turn(channel, tag, round);
    m.headers
        .context
        .insert("turn_id".into(), json!(turn_id(tag)));
    m
}

/// The channel turn id [`turn_with_id`] stamps for `tag`.
pub fn turn_id(tag: &str) -> String {
    format!("turn-{tag}")
}

pub fn text_of(m: &Message) -> String {
    match &m.body {
        Body::Inline(v) => meclaw_core::serde_json::to_string(v).unwrap_or_default(),
        _ => String::new(),
    }
}

/// The next answer that leaves the generation.
pub async fn answer(ports: &mut road::Ports, what: &str) -> Message {
    tokio::time::timeout(road::DEADLINE, ports.sink.recv())
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| panic!("{what}: no answer left the generation"))
}

/// Until the close pass of every session of `sessions` reported at the park
/// (in any order; every other parked message is passed over).
pub async fn close_reports(ports: &mut road::Ports, root: &std::path::Path, sessions: &[String]) {
    let mut left: std::collections::HashSet<&str> = sessions.iter().map(String::as_str).collect();
    let deadline = Instant::now() + PASS_DEADLINE;
    while !left.is_empty() {
        let wait = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(wait, ports.park.recv()).await {
            Ok(Some(m)) => {
                if m.headers.hop.get("route").and_then(Value::as_str) == Some("close_report")
                    && let Some(sid) = m.headers.context.get("session_id").and_then(Value::as_str)
                {
                    left.remove(sid);
                }
            }
            _ => panic!(
                "no close_report of {left:?} within {PASS_DEADLINE:?}; dead letters: {:?}",
                road::dead_letters(root)
            ),
        }
    }
}

/// The rows of `sql` over a cell's own `cell.db`, every column as text.
/// Read-only and never created here (the `gh893` lesson).
pub fn rows(db: &std::path::Path, sql: &str) -> Vec<Vec<String>> {
    if !db.is_file() {
        return Vec::new();
    }
    let Ok(conn) =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return Vec::new();
    };
    let Ok(mut st) = conn.prepare(sql) else {
        return Vec::new();
    };
    let n = st.column_count();
    st.query_map([], |r| {
        Ok((0..n)
            .map(|i| match r.get_ref(i) {
                Ok(rusqlite::types::ValueRef::Text(t)) => String::from_utf8_lossy(t).to_string(),
                Ok(rusqlite::types::ValueRef::Integer(n)) => n.to_string(),
                _ => String::new(),
            })
            .collect::<Vec<String>>())
    })
    .map(|it| it.filter_map(Result::ok).collect())
    .unwrap_or_default()
}

/// Until `sql` returns at least `n` rows in `db`, or the deadline says what
/// did not happen.
pub async fn until_rows(db: &std::path::Path, sql: &str, n: usize, what: &str) -> Vec<Vec<String>> {
    let deadline = Instant::now() + PASS_DEADLINE;
    loop {
        let got = rows(db, sql);
        if got.len() >= n {
            return got;
        }
        assert!(
            Instant::now() < deadline,
            "{what}: fewer than {n} row(s) of `{sql}` within {PASS_DEADLINE:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub fn sessions_db(root: &std::path::Path) -> std::path::PathBuf {
    root.join("main/assistants/scribe/talky/session-keeper/sessions/cell.db")
}

/// `(session_id, audience_set, closed)` of every generation of `channel`,
/// oldest first.
pub fn generations(root: &std::path::Path, channel: &str) -> Vec<(String, String, String)> {
    rows(
        &sessions_db(root),
        &format!(
            "SELECT session_id, COALESCE(audience_set, ''), closed FROM sessions \
             WHERE channel = '{channel}' ORDER BY opened_at"
        ),
    )
    .into_iter()
    .map(|r| (r[0].clone(), r[1].clone(), r[2].clone()))
    .collect()
}

/// One delivery of the run's `message_log`, with its place in the log.
#[derive(Clone, Debug)]
pub struct Logged {
    pub at: i64,
    pub from: String,
    pub to: String,
    pub hop: Value,
    pub context: Value,
    pub body: String,
}

/// Every delivery from `from` to `to` (`None` = any), in log order. Read after
/// `shutdown`, like `road::deliveries`.
pub fn logged(root: &std::path::Path, from: &str, to: Option<&str>) -> Vec<Logged> {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare(
            "SELECT rowid, from_path, to_path, headers, COALESCE(body_payload, '') \
             FROM message_log WHERE from_path = ?1 ORDER BY rowid",
        )
        .expect("message_log");
    st.query_map([from], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
        ))
    })
    .expect("query")
    .filter_map(Result::ok)
    .filter(|(_, _, t, _, _)| to.is_none_or(|to| t == to))
    .map(|(at, from, to, headers, body)| {
        let h: Value = meclaw_core::serde_json::from_str(&headers).unwrap_or(Value::Null);
        Logged {
            at,
            from,
            to,
            hop: h["hop"].clone(),
            context: h["context"].clone(),
            body,
        }
    })
    .collect()
}

/// The `write` batches the curator's writer handed out, per session.
pub fn writes(root: &std::path::Path) -> HashMap<String, Vec<Logged>> {
    let mut out: HashMap<String, Vec<Logged>> = HashMap::new();
    for l in logged(root, WRITER, None) {
        if l.hop["route"] == "write" {
            let sid = l.hop["session_id"].as_str().unwrap_or_default().to_string();
            out.entry(sid).or_default().push(l);
        }
    }
    out
}

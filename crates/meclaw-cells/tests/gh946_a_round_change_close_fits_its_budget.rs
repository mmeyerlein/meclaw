//! GH #946 -- a close that a round change triggers fits its budget.
//!
//! WHY this file exists: since GH #940 (ADR-0002 E8) a turn whose round differs
//! from the open generation of its channel SEALS that generation, and the
//! session keeper's `close` for it leaves MID-TURN: it is emitted on the turn's
//! own chain, after the generation's door (`. -> ./talky`) and the keeper's
//! stamp/store round trips have already spent part of the turn's budget. The
//! night's close is a fresh root; this one is not. The talky's edge
//! `./session-keeper -> ./curator` (`close` -> `in_close`) is therefore a door
//! (`restore_ttl`, the seam row in `gh929_every_restoring_edge_sits_on_a_seam.rs`),
//! and this file holds the road behind it to the reserve of
//! `gh929_every_budget_segment_fits_its_reserve.rs`:
//!
//! (a) the `close` of the sealed generation reaches the curator entry on the
//!     turn's own parent chain (it IS mid-turn) and arrives with the colony
//!     budget less the one decision that crossed the door;
//! (b) from there the close pass of its one round runs to its last fact -- the
//!     curator's `write` batch, the member's memory, the closer, the fact in
//!     the store -- without a `ttl_expired`, and the deepest delivery it causes
//!     stays within `MESSAGE_DEFAULT_TTL - RESERVE` routing decisions; every
//!     run prints `gh946 close lane: start=<ttl> end=<ttl> used=<n>`;
//! (c) the turn that changed the round reaches the brain exactly ONCE: the
//!     curator's handover for the new generation and the close of the old one
//!     run beside it, and neither sends it a second time.
//!
//! The road is the member road of GH #929 (`support/gh929_member_road.rs`):
//! the shipped generation under its container, the member's memory, every
//! `llm` cell on a local stub -- the brain on a scripted one, the memory's
//! closer on one that answers each prompt with a fact per marked turn, every
//! other on the background stub. Guarded like every template-reading test
//! (GH #49).

#[path = "mock_openai.rs"]
mod mock_openai;
#[path = "support/gh929_member_road.rs"]
mod road;

use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, MESSAGE_DEFAULT_TTL, Message, MessageBuilder, Path};
use meclaw_testing::mock_http::{
    CapturedRequest, MockResponse, RequestValidator, start_mock_server_capturing_with_validator,
};
use mock_openai::{MockOpenAI, canned_chat_completion};
use road::{Delivery, GENERATION, Stubs, segment_before};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// What a segment keeps back from the colony budget (OR-BD-11), the number of
/// `gh929_every_budget_segment_fits_its_reserve.rs`.
const RESERVE: u32 = 16;

/// The most routing decisions one segment may spend.
const SEGMENT_MAX: i64 = (MESSAGE_DEFAULT_TTL - RESERVE) as i64;

/// What a restoring seam hands the delivery right behind it: the colony
/// budget less the one routing decision that crossed the seam.
const BEHIND_A_SEAM: i64 = MESSAGE_DEFAULT_TTL as i64 - 1;

/// The bound on the close pass, a failure marker and not a timing: it runs
/// behind the turn, through the curator, the memory and the closer.
const PASS_DEADLINE: Duration = Duration::from_secs(90);

const CHANNEL: &str = "talky:946";

/// The round the generation is opened in, and the round that ends it.
const ROUND_A: &str = road::AUDIENCE;
const ROUND_B: &str = r#"["member:owner","agent:scribe","member:guest"]"#;

/// The marker of the one turn the closer files a fact for; nothing else in the
/// tree says it.
const MARKER: &str = "MARK-946";
const SAID_A: &str = "MARK-946: the blue lantern hangs by the north door.";
const REPLY_A: &str = "Noted: the lantern by the north door.";
const SAID_B: &str = "A new voice joins the room.";
const REPLY_B: &str = "Welcome to the room.";
const PREDICATE: &str = "hangs_at";

// ─────────────────────────────────────────────────────────────── the stubs

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

/// The JSON document of the closer's user message (tolerant of text around
/// it).
fn prompt_doc(text: &str) -> Value {
    if let Ok(v) = meclaw_core::serde_json::from_str::<Value>(text) {
        return v;
    }
    match (text.find('{'), text.rfind('}')) {
        (Some(a), Some(b)) if a < b => {
            meclaw_core::serde_json::from_str(&text[a..=b]).unwrap_or(Value::Null)
        }
        _ => Value::Null,
    }
}

/// The stub closer's verdict: an `add` per marked turn of the prompt, filed
/// under that turn; nothing else.
fn closer_verdict(req: &CapturedRequest) -> Option<MockResponse> {
    let body: Value = meclaw_core::serde_json::from_slice(&req.body).ok()?;
    let text = body["messages"]
        .as_array()
        .and_then(|ms| ms.iter().rev().find(|m| m["role"] == "user"))
        .map(content_of)
        .unwrap_or_default();
    let doc = prompt_doc(&text);
    let add: Vec<Value> = doc["turns"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|t| t["text"].as_str().is_some_and(|x| x.contains(MARKER)))
        .map(|t| {
            json!({"episode_id": t["episode_id"].clone(),
                   "subject": format!("{MARKER} lantern"),
                   "predicate": PREDICATE, "claim": MARKER,
                   "fact_kind": "world", "confidence": 80})
        })
        .collect();
    let verdict = json!({"nothing_to_add": add.is_empty(), "add": add, "sharpen": [],
                         "correct": [], "close_topics": []});
    Some(canned_chat_completion(&verdict.to_string(), "stop"))
}

/// The closer's own stub; every request it sees is answered by
/// [`closer_verdict`].
async fn start_closer() -> String {
    let validator: RequestValidator = Arc::new(closer_verdict);
    let nothing = json!({"nothing_to_add": true, "add": [], "sharpen": [], "correct": [],
                         "close_topics": []});
    let (addr, _join, _captured) = start_mock_server_capturing_with_validator(
        vec![canned_chat_completion(&nothing.to_string(), "stop")],
        Some(validator),
    )
    .await;
    format!("http://{addr}")
}

// ─────────────────────────────────────────────────────────────── the turns

/// A person's words at the container's door, in `round`, with the budget the
/// ingress leaves after the member's own two routing decisions.
fn turn(text: &str, round: &str) -> Message {
    MessageBuilder::new(Path::new("/assistants"))
        .hop(road::as_map(&json!({"route": "in_turn"})))
        .context(road::as_map(
            &json!({"assistant": "scribe", "channel": CHANNEL,
                                      "audience_set": round}),
        ))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .ttl(MESSAGE_DEFAULT_TTL - road::ABOVE_THE_CONTAINER)
        .build()
}

async fn answer(ports: &mut road::Ports, what: &str) -> Message {
    tokio::time::timeout(road::DEADLINE, ports.sink.recv())
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| panic!("{what}: no answer left the generation"))
}

// ─────────────────────────────────────────────────────────────── reading

/// The rows of `sql` over a cell's own `cell.db`, every column as text.
/// Read-only and never created here (the `gh893` lesson: a poll that creates
/// the file before the store woke turns a fresh birth into a resumed one).
/// The participant set of an `audience_set` value, whatever its spelling.
fn participants(round: &str) -> std::collections::BTreeSet<String> {
    meclaw_core::serde_json::from_str::<Vec<String>>(round)
        .unwrap_or_else(|e| panic!("not a round: {round:?} ({e})"))
        .into_iter()
        .collect()
}

fn rows(db: &std::path::Path, sql: &str) -> Vec<Vec<String>> {
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
    let out: Vec<Vec<String>> = st
        .query_map([], |r| {
            Ok((0..n)
                .map(|i| {
                    r.get::<_, Option<String>>(i)
                        .ok()
                        .flatten()
                        .unwrap_or_default()
                })
                .collect::<Vec<String>>())
        })
        .map(|it| it.filter_map(Result::ok).collect())
        .unwrap_or_default();
    out
}

/// Until `sql` returns at least `n` rows in `db`, or the deadline says what
/// did not happen.
async fn until_rows(db: &std::path::Path, sql: &str, n: usize, what: &str) -> Vec<Vec<String>> {
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

/// The parent chain of `d`, `d` itself excluded, as delivery ids.
fn ancestors(log: &[Delivery], d: &Delivery) -> HashSet<String> {
    let by_id: HashMap<&str, &Delivery> = log.iter().map(|x| (x.id.as_str(), x)).collect();
    let mut out = HashSet::new();
    let mut at = d;
    while let Some(p) = at.parent.as_deref().and_then(|p| by_id.get(p)).copied() {
        if !out.insert(p.id.clone()) {
            break;
        }
        at = p;
    }
    out
}

/// Every delivery caused by `root`, `root` itself excluded.
fn descendants<'a>(log: &'a [Delivery], root: &Delivery) -> Vec<&'a Delivery> {
    let mut children: HashMap<&str, Vec<&Delivery>> = HashMap::new();
    for d in log {
        if let Some(p) = d.parent.as_deref() {
            children.entry(p).or_default().push(d);
        }
    }
    let mut out = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    let mut todo = vec![root.id.as_str()];
    while let Some(id) = todo.pop() {
        for c in children.get(id).cloned().unwrap_or_default() {
            if seen.insert(c.id.as_str()) {
                out.push(c);
                todo.push(c.id.as_str());
            }
        }
    }
    out
}

// ═══════════════════════════════════════════════════════════════════ lock

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_round_change_close_fits_its_budget() {
    if !road::shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let brain_cell = format!("{}/talky/brain", GENERATION.trim_start_matches('/'));
    let brain = MockOpenAI::start(vec![
        canned_chat_completion(REPLY_A, "stop"),
        canned_chat_completion(REPLY_B, "stop"),
    ])
    .await;
    let background =
        MockOpenAI::start(vec![canned_chat_completion(road::BACKGROUND_REPLY, "stop")]).await;
    let closer = start_closer().await;
    let stubs = Stubs {
        scripted: HashMap::from([
            (brain_cell.clone(), brain.base_url.clone()),
            ("memory-hive/closer".to_string(), closer),
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
    let (h, mut ports) = road::boot(&td).await;
    let root = td.path();
    let memory = root.join("main/memory-hive/store/cell.db");
    let sessions = root.join("main/assistants/scribe/talky/session-keeper/sessions/cell.db");
    let ledger = root.join("main/assistants/scribe/talky/curator/ledger/cell.db");

    // Turn one opens the generation of round A. Its words stand in the
    // member's memory and its answer on the curator's wall before the round
    // changes: the close reads both at the moment it is sent.
    h.send(turn(SAID_A, ROUND_A)).await;
    answer(&mut ports, "turn one").await;
    until_rows(
        &memory,
        &format!("SELECT id FROM episodes WHERE content LIKE '%{MARKER}%'"),
        1,
        "turn one's words reach the memory",
    )
    .await;
    until_rows(
        &ledger,
        &format!(
            "SELECT w.hash FROM wall w JOIN blocks b ON b.hash = w.hash \
             WHERE b.body LIKE '%{REPLY_A}%'"
        ),
        1,
        "turn one's answer stands on the wall",
    )
    .await;

    // Turn two changes the round: the generation of round A is sealed on its
    // chain, and the close pass runs behind the curator's door.
    h.send(turn(SAID_B, ROUND_B)).await;
    answer(&mut ports, "turn two").await;
    let deadline = Instant::now() + PASS_DEADLINE;
    let report = loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, ports.park.recv()).await {
            Ok(Some(m))
                if m.headers.hop.get("route").and_then(Value::as_str) == Some("close_report") =>
            {
                break m;
            }
            Ok(Some(_)) => {}
            _ => panic!(
                "no close_report within {PASS_DEADLINE:?}; dead letters: {:?}",
                road::dead_letters(root)
            ),
        }
    };
    let sealed = until_rows(
        &sessions,
        "SELECT session_id, COALESCE(audience_set, '') FROM sessions WHERE closed = 1",
        1,
        "the round change seals the generation of round A",
    )
    .await;
    let sid_a = sealed[0][0].clone();
    until_rows(
        &memory,
        &format!("SELECT id FROM facts WHERE session_id = '{sid_a}' AND claim LIKE '%{MARKER}%'"),
        1,
        "the close pass of round A writes its fact",
    )
    .await;
    // The close leaves mid-turn, with round B's turn in flight behind the same
    // door: every fact of the sealed generation is written under round A, the
    // round it was opened in -- never under round B (GH #940, E8).
    let fact_rounds = rows(
        &memory,
        &format!("SELECT COALESCE(audience_set, '') FROM facts WHERE session_id = '{sid_a}'"),
    );
    let open = rows(
        &sessions,
        "SELECT session_id, COALESCE(audience_set, '') FROM sessions WHERE closed = 0",
    );
    let brain_calls = brain.recorded_requests().await.len();
    h.shutdown().await;
    let log = road::deliveries(root);
    let dead = road::dead_letters(root);

    // The keeper: one generation sealed under round A, one open of round B.
    assert_eq!(
        sealed.len(),
        1,
        "exactly one generation is sealed: {sealed:?}"
    );
    assert_eq!(sealed[0][1], ROUND_A, "{sealed:?}");
    assert!(!fact_rounds.is_empty(), "no fact of {sid_a}");
    for r in &fact_rounds {
        assert_eq!(
            participants(&r[0]),
            participants(ROUND_A),
            "a fact of the generation of round A carries another round: {fact_rounds:?}"
        );
    }
    assert_eq!(
        open.iter().map(|r| r[1].as_str()).collect::<Vec<_>>(),
        vec![ROUND_B],
        "turn two runs in a generation of its own round: {open:?}"
    );
    assert_eq!(
        report
            .headers
            .context
            .get("session_id")
            .and_then(Value::as_str),
        Some(sid_a.as_str()),
        "the report is the close pass of the sealed generation"
    );

    // (b) no delivery of the run died of its budget.
    let expired: Vec<&(String, String, String)> =
        dead.iter().filter(|(_, _, c)| c == "ttl_expired").collect();
    assert!(expired.is_empty(), "the run has ttl deaths: {expired:?}");

    // (a) the close is mid-turn and crosses a restoring door.
    let door = format!("{GENERATION}/talky");
    let curator = format!("{door}/curator");
    let in_turn: Vec<&Delivery> = log
        .iter()
        .filter(|d| d.to == door && d.route == "in_turn")
        .collect();
    assert_eq!(in_turn.len(), 2, "two turns crossed the generation's door");
    let closes: Vec<&Delivery> = log
        .iter()
        .filter(|d| d.to == curator && d.route == "in_close")
        .collect();
    assert_eq!(closes.len(), 1, "one close reached the curator: {closes:?}");
    let close = closes[0];
    assert!(
        ancestors(&log, close).contains(&in_turn[1].id),
        "the close rides turn two's own chain:\n{}",
        road::chain_of(&log, close, 80).join("\n")
    );
    if let Some(before) = segment_before(&log, close, |d| d.id == in_turn[1].id) {
        eprintln!("{}", before.say("gh946 turn door -> close door"));
    }
    assert_eq!(
        close.ttl,
        BEHIND_A_SEAM,
        "the close door restores: the close arrives with the colony budget less one \
         decision, not with the rest of the turn:\n{}",
        road::chain_of(&log, close, 80).join("\n")
    );

    // (b) the close pass, measured at the deepest delivery it caused.
    let caused = descendants(&log, close);
    assert!(
        caused.iter().any(|d| d.to == "/memory-hive/closer"),
        "the close pass reached the memory's closer"
    );
    let deepest = caused
        .iter()
        .min_by_key(|d| d.ttl)
        .copied()
        .expect("the close caused deliveries");
    let used = close.ttl - deepest.ttl;
    eprintln!(
        "gh946 close lane: start={} end={} used={used}",
        close.ttl, deepest.ttl
    );
    assert!(
        used <= SEGMENT_MAX,
        "the close lane spends {used} of {SEGMENT_MAX} routing decisions (reserve \
         {RESERVE}):\n{}",
        road::chain_of(&log, deepest, 80).join("\n")
    );

    // (c) each turn reached the brain once; the close never did.
    let brain_path = format!("{door}/brain");
    let at_brain: Vec<&Delivery> = log.iter().filter(|d| d.to == brain_path).collect();
    assert_eq!(
        (at_brain.len(), brain_calls),
        (2, 2),
        "one brain call per turn, none twice: {at_brain:?}"
    );
    let of_turn_two = at_brain
        .iter()
        .filter(|d| ancestors(&log, d).contains(&in_turn[1].id))
        .count();
    assert_eq!(
        of_turn_two, 1,
        "the turn that changed the round reaches the brain exactly once"
    );
    assert!(
        at_brain
            .iter()
            .all(|d| !ancestors(&log, d).contains(&close.id)),
        "the close of the sealed generation sent something to the brain"
    );
}

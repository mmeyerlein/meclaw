//! GH #852 — the shipped high-frequency sinks and gates run `warm`.
//!
//! `terminal` sits at the end of every lane it is wired on, `daily-digest/void`
//! swallows every inbound chat of a push-only bot, and `affinity/gate` sits
//! under every door of a member. All three ran `cold`: a fresh interpreter per
//! message and, under the default-deny profile every instantiated `code` cell
//! gets (`network: deny`), a fresh user and network namespace as well. A burst
//! of messages was a burst of namespace setups, and the mailbox ran full while
//! they ran.
//!
//! What is pinned here, each at the receiver:
//!
//! 1. **the declarations** — the three shipped configs say `runner_mode: "warm"`,
//!    their scripts read stdin as text only (the warm harness hands them an
//!    `io.StringIO`, which has no `buffer` and no `fileno()`) and declare no
//!    `global` state, and a cell that says nothing still runs `cold`;
//! 2. **the reason** — a warm sink behind the default-deny profile answers a
//!    flood of 1 000 messages through a mailbox of 64 message for message,
//!    from the children of its pool and from nothing else; 100 messages start
//!    100 interpreters `cold` and at most one per slot `warm`. How fast it keeps
//!    up (200 messages a second without one `Full`, the cold/warm ratio) is
//!    measured in the perf stage, never judged here (rule 11);
//! 3. **the move changes nothing but cost** — the shipped gate answers the
//!    same message stream with the same emissions under `warm` as under `cold`.

use meclaw_cells::code::{CodeCellFactory, CodeParams, RunnerMode};
use meclaw_colony::{CellFactory, SpawnedCellKind};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, CellEmission, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::EmissionsExt;
use meclaw_testing::deadline;
use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;
use tokio::sync::mpsc;

/// The three cells #852 moved, as they ship.
const WARM_CELLS: [&str; 3] = [
    "templates/terminal/config.json",
    "templates/daily-digest/void/config.json",
    "templates/affinity/gate/config.json",
];

fn shipped(rel: &str) -> Value {
    let raw = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(rel),
    )
    .unwrap_or_else(|e| panic!("{rel}: {e}"));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// The block a cell instantiated from a template gets when it declares none
/// (`meclaw-colony` `default_sandbox_block`, GH #85).
fn default_deny() -> Value {
    json!({"trust": "restricted", "network": "deny", "filesystem": {"runtime": true}})
}

type Cell = (
    mpsc::Sender<Message>,
    mpsc::Receiver<CellEmission>,
    tempfile::TempDir,
);

fn spawn(params: Value, mailbox: usize, multi_send: bool) -> Cell {
    let (otx, orx) = mpsc::channel(4096);
    let td = tempfile::TempDir::new().expect("tempdir");
    let (itx, _irx) = mpsc::channel(8);
    let spawned = Arc::new(CodeCellFactory)
        .spawn_cell(
            Path::new("/sink"),
            params,
            otx,
            td.path().to_path_buf(),
            meclaw_colony::ContractView {
                multi_send_capable: multi_send,
                ..Default::default()
            },
            itx,
            None,
            0,
            None,
            None,
            mailbox,
        )
        .expect("the params spawn");
    match spawned {
        SpawnedCellKind::Active { sender, .. } => (sender, orx, td),
        SpawnedCellKind::Dormant { .. } => unreachable!("code spawns Active"),
    }
}

fn msg(body: Value) -> Message {
    MessageBuilder::new(Path::new("/sink"))
        .body(Body::Inline(body))
        .reply_to(Path::new("/drain"))
        .build()
}

// ── 1. the declarations ──────────────────────────────────────────────────

#[test]
fn the_three_hot_cells_ship_warm_and_are_safe_to() {
    for rel in WARM_CELLS {
        let cfg = shipped(rel);
        let params = &cfg["params"];
        assert_eq!(
            params["runner_mode"],
            json!("warm"),
            "{rel}: a high-frequency sink or gate runs warm (GH #852)"
        );
        let parsed = CodeParams::parse(params).unwrap_or_else(|e| panic!("{rel}: {e}"));
        assert_eq!(parsed.runner_mode, RunnerMode::Warm, "{rel}");
        let script = params["script_inline"]
            .as_str()
            .unwrap_or_else(|| panic!("{rel}: an inline script"));
        // The warm harness gives the body an `io.StringIO` as stdin and a fresh
        // namespace per message. A script reading bytes off the real fd, or
        // keeping module-level state on purpose, would notice both.
        for forbidden in [
            "stdin.buffer",
            "fileno(",
            "os.read(0",
            "\nglobal ",
            " global ",
        ] {
            assert!(
                !script.contains(forbidden),
                "{rel}: `{forbidden}` does not survive the warm harness"
            );
        }
    }
}

#[test]
fn a_cell_that_says_nothing_still_runs_cold() {
    let parsed =
        CodeParams::parse(&json!({"runner": "python3", "script_inline": "pass"})).expect("parses");
    assert_eq!(
        parsed.runner_mode,
        RunnerMode::Cold,
        "cold stays the default"
    );
}

// ── 2. the reason ────────────────────────────────────────────────────────

/// A sink that reads its document and answers with one text naming the
/// interpreter that ran it and the message it ran: `<pid>:<n>`. The pid is the
/// event the flood is judged on -- a warm cell answers from its pool, a cold
/// cell starts one interpreter (and, behind the default-deny profile, one user
/// and network namespace) per message.
fn pid_sink_params(mode: &str, sandbox: Option<Value>) -> Value {
    let mut p = json!({
        "runner": "python3",
        "script_inline": "import json, os, sys\n\
            d = json.load(sys.stdin)\n\
            n = d['body'].get('n', -1)\n\
            sys.stdout.write(json.dumps({'messages': [{'origin': 'assistant', 'type': 'text', \
            'text': '%d:%d' % (os.getpid(), n)}]}))\n",
        "external_timeout_ms": meclaw_testing::deadline::product_wait_ms(),
        "runner_mode": mode,
    });
    if let Some(s) = sandbox {
        p["sandbox"] = s;
    }
    p
}

/// Read the output channel until every message of `ids` is marked consumed
/// (GH #1015: a delivery's mark rides behind every emission of that
/// delivery), and return the answers in between. No count is guessed and no
/// window waited out: when the last mark arrived, every answer is in.
async fn drain(
    orx: &mut mpsc::Receiver<CellEmission>,
    ids: &[Uuid],
    what: &str,
) -> Vec<CellEmission> {
    let mut open: HashSet<Uuid> = ids.iter().copied().collect();
    let mut answers = Vec::new();
    deadline::hang(what, async {
        while !open.is_empty() {
            let em = orx
                .recv()
                .await
                .unwrap_or_else(|| panic!("{what}: the output channel closed"));
            match em.consumed_mark_id() {
                Some(id) => {
                    open.remove(&id);
                }
                None => answers.push(em),
            }
        }
    })
    .await;
    answers
}

/// The `<pid>:<n>` of every answer of [`pid_sink_params`].
fn pids_and_ns(answers: &[CellEmission]) -> (BTreeSet<i64>, Vec<i64>) {
    let mut pids = BTreeSet::new();
    let mut ns = Vec::with_capacity(answers.len());
    for em in answers {
        let text = em.content["messages"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("an answer without its text: {}", em.content));
        let (pid, n) = text
            .split_once(':')
            .unwrap_or_else(|| panic!("not `<pid>:<n>`: {text}"));
        pids.insert(pid.parse().expect("a numeric pid"));
        ns.push(n.parse().expect("a numeric n"));
    }
    ns.sort_unstable();
    (pids, ns)
}

/// Send `n` messages (numbered) into `tx` with backpressure -- a full mailbox
/// makes the sender wait, it never drops -- while a second task drains the
/// answers; return those answers.
async fn flood(
    tx: mpsc::Sender<Message>,
    mut orx: mpsc::Receiver<CellEmission>,
    n: usize,
    what: &'static str,
) -> Vec<CellEmission> {
    let msgs: Vec<Message> = (0..n)
        .map(|i| msg(json!({"messages": [], "n": i})))
        .collect();
    let ids: Vec<Uuid> = msgs.iter().map(|m| m.id).collect();
    let reader = tokio::spawn(async move { drain(&mut orx, &ids, what).await });
    for m in msgs {
        tx.send(m).await.expect("the sink's mailbox stays open");
    }
    deadline::hang("the reader of the flood", reader)
        .await
        .expect("the reader task")
}

/// GH #852, the reason, as events: a flood of 1 000 messages into a warm sink
/// behind the default-deny profile, through a mailbox of 64, is answered
/// message for message -- every message once, none lost to backpressure -- by
/// the children of its pool and by nothing else. A cold run would have started
/// 1 000 interpreters and 1 000 namespace pairs; the warm one starts at most
/// one per slot. How fast it keeps up is a number for the perf stage
/// (`tests/gh852_perf_a_warm_sink_keeps_up_with_a_flood.rs`), never a verdict here.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_warm_sink_keeps_up_with_a_flood() {
    // A kernel without Landlock cannot apply the restricted profile, and a
    // restricted cell never falls back to unsandboxed; skip visibly rather
    // than go red (the convention of `sandbox_isolation.rs`).
    if meclaw_cells::sandbox::landlock_abi().is_none() {
        eprintln!("SKIP: no Landlock on this kernel");
        return;
    }
    const FLOOD: usize = 1_000;
    let params = pid_sink_params("warm", Some(default_deny()));
    let pool = CodeParams::parse(&params)
        .expect("parses")
        .effective_max_concurrency();
    let (tx, orx, _td) = spawn(params, 64, false);
    let answers = flood(
        tx,
        orx,
        FLOOD,
        "every message of the flood is marked consumed",
    )
    .await;
    let (pids, ns) = pids_and_ns(&answers);
    assert_eq!(
        ns,
        (0..FLOOD as i64).collect::<Vec<_>>(),
        "every message of the flood is answered exactly once, at the receiver"
    );
    assert!(
        pids.len() <= pool,
        "a warm sink answers a flood from its pool of {pool}, not one interpreter per message: \
         {} interpreters for {FLOOD} messages",
        pids.len()
    );
}

/// The cost of #852 as events: 100 messages start 100 interpreters `cold` and
/// at most one per pool slot `warm`. The time the two take is a number for the
/// perf stage (`tests/gh852_perf_a_warm_sink_keeps_up_with_a_flood.rs`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hundred_messages_start_a_hundred_interpreters_cold_and_a_pool_warm() {
    const N: usize = 100;
    let mut seen = Vec::new();
    for mode in ["cold", "warm"] {
        let params = pid_sink_params(mode, None);
        let (tx, orx, _td) = spawn(params, 2 * N, false);
        let answers = flood(tx, orx, N, "the hundred messages are marked consumed").await;
        let (pids, ns) = pids_and_ns(&answers);
        assert_eq!(
            ns,
            (0..N as i64).collect::<Vec<_>>(),
            "{mode}: every message answered once"
        );
        seen.push(pids.len());
    }
    let pool = CodeParams::parse(&pid_sink_params("warm", None))
        .expect("parses")
        .effective_max_concurrency();
    assert_eq!(seen[0], N, "cold: one interpreter per message");
    assert!(
        seen[1] <= pool,
        "warm: at most one interpreter per slot of {pool}, got {}",
        seen[1]
    );
}

// ── 3. the move changes nothing but cost ─────────────────────────────────

/// One proposal as the gate receives it: a tool call from the member's side,
/// with the actor on the edge.
fn proposal(args: Value) -> Message {
    let mut ctx = Map::new();
    ctx.insert("actor".into(), json!("member:alex"));
    MessageBuilder::new(Path::new("/sink"))
        .context(ctx)
        .body(Body::Inline(json!({"messages": [{
            "origin": "assistant", "type": "tool_call", "id": "c1",
            "text": args.to_string(),
        }]})))
        .reply_to(Path::new("/drain"))
        .build()
}

/// A store answer as it comes back to the gate: `hop.operation`, and an error
/// code when the write failed.
fn store_answer(error_code: Option<&str>) -> Message {
    let mut hop = Map::new();
    hop.insert("operation".into(), json!("insert"));
    if let Some(code) = error_code {
        hop.insert("error_code".into(), json!(code));
    }
    MessageBuilder::new(Path::new("/sink"))
        .hop(hop)
        .body(Body::Inline(json!({"messages": []})))
        .reply_to(Path::new("/drain"))
        .build()
}

/// A stream that crosses every branch shape of the gate — accepted writes, a
/// refusal, a silent echo that leaves through `sys.exit(0)`, a store error,
/// unparsable arguments — and how many emissions it produces in total.
fn gate_stream() -> (Vec<Message>, usize) {
    let aieos = json!({
        "standard": {"protocol": "AIEOS", "version": "1.1.0"},
        "metadata": {"instance_id": "i-1"},
        "identity": {"names": {"first": "Ada"}}
    });
    let stream = vec![
        // 2 store ops + audit + ack + the push nudge (GH #1096)
        proposal(
            json!({"op": "upsert_entity", "kind": "person", "display_name": "Ada", "aieos": aieos}),
        ),
        // audit + ack (refused)
        proposal(
            json!({"op": "upsert_entity", "kind": "person", "display_name": "Ada", "aieos": {"bogus": 1}}),
        ),
        // nothing: the echo of a write that worked
        store_answer(None),
        // one error
        store_answer(Some("query_failed")),
        // 1 store op + audit + ack + the push nudge (GH #1096)
        proposal(
            json!({"op": "set_trust", "entity_id": "entity:1", "audience": "*", "level": "known"}),
        ),
        // audit + ack (refused)
        {
            let mut ctx = Map::new();
            ctx.insert("actor".into(), json!("member:alex"));
            MessageBuilder::new(Path::new("/sink"))
                .context(ctx)
                .body(Body::Inline(json!({"messages": [{
                    "origin": "assistant", "type": "tool_call", "id": "c2", "text": "not json"
                }]})))
                .reply_to(Path::new("/drain"))
                .build()
        },
        // audit + ack (refused: no subscriber on the edge)
        proposal(json!({"op": "subscribe", "subject": "entity:1"})),
        // 1 store op + audit + ack + the push nudge (GH #1096)
        proposal(
            json!({"op": "propose", "source_ref": "turn:1", "entity_ref": "entity:1",
                        "field_path": "aieos.identity", "audience": "agent:x"}),
        ),
    ];
    (stream, 5 + 2 + 1 + 4 + 2 + 2 + 4)
}

/// One emission with everything random (ids, timestamps) taken out: the lane,
/// the verdict, and per turn what it asks the store to do or what it answers.
fn shape(em: &CellEmission) -> String {
    let h = &em.content["header"];
    let mut parts = vec![format!(
        "{}|{}|{}",
        h["route"].as_str().unwrap_or(""),
        h["outcome"].as_str().unwrap_or(""),
        h["reason_code"].as_str().unwrap_or("")
    )];
    for m in em.content["messages"].as_array().into_iter().flatten() {
        let text: Value = m["text"]
            .as_str()
            .and_then(|t| meclaw_core::serde_json::from_str(t).ok())
            .unwrap_or(Value::Null);
        parts.push(format!(
            "{}:{}:{}:{}:{}",
            m["type"].as_str().unwrap_or(""),
            text["operation"].as_str().unwrap_or(""),
            text["table"].as_str().unwrap_or(""),
            text["outcome"].as_str().unwrap_or(""),
            text["reason_code"].as_str().unwrap_or("")
        ));
    }
    parts.join(" ")
}

async fn gate_answers(mode: &str) -> Vec<String> {
    let mut params = shipped("templates/affinity/gate/config.json")["params"].clone();
    params["runner_mode"] = json!(mode);
    params["external_timeout_ms"] = json!(meclaw_testing::deadline::product_wait_ms());
    let (tx, mut orx, _td) = spawn(params, 64, true);
    let (stream, expected) = gate_stream();
    let ids: Vec<Uuid> = stream.iter().map(|m| m.id).collect();
    for m in stream {
        tx.send(m).await.expect("send");
    }
    // Every message of the stream is marked consumed, and every emission of
    // a delivery rides in front of its mark: the answers read until the last
    // mark are all there are. The count is the claim, not a stop condition.
    let got = drain(&mut orx, &ids, "the gate stream is marked consumed").await;
    assert_eq!(
        got.len(),
        expected,
        "{mode}: the stream answers with exactly {expected} emissions"
    );
    // And nothing stands behind the last mark (zero wait, one hop).
    if let Ok(extra) = orx.try_recv_answer() {
        panic!(
            "{mode}: an emission behind the last consume mark: {}",
            extra.content
        );
    }
    for em in &got {
        assert!(
            em.content["header"]["exit_code"] == json!(0),
            "{mode}: the gate never fails a message: {}",
            em.content
        );
    }
    let mut shapes: Vec<String> = got.iter().map(shape).collect();
    shapes.sort();
    shapes
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_shipped_gate_answers_the_same_warm_as_cold() {
    let cold = gate_answers("cold").await;
    let warm = gate_answers("warm").await;
    assert_eq!(warm, cold, "warm changes latency, never an answer");
}

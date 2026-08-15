//! Wave 13 -- the extraction lane stops paying twice for the same turn
//! (GitHub #72).
//!
//! The batch lane's recovery sweep (`mem_phase = 'flush'` with
//! `flush_reclaim = '1'`) exists for chains that died: a TTL expiry or a restart
//! leaves rows stuck in `claimed`, and somebody has to hand them back. It used
//! to hand back EVERY claimed row, on the argument that the
//! `(episode_id, claim_hash)` dedup makes a re-extraction write nothing new.
//!
//! That argument is about correctness and the cost is not. Measured under
//! sustained ingest: 5 859 batched items for 3 839 ingested turns (1.53x), with
//! the sweep firing whenever the queue had not moved for 40 seconds -- a
//! threshold one full extraction cycle now crosses routinely. So LIVE batches
//! were taken back and extracted a second time, and roughly a third of the
//! extraction bill bought nothing.
//!
//! The fix is to give the claim an expiry. The claim stamps `claimed_at`, and
//! the sweep only reaches claims older than `MEMORY_BATCH_CLAIM_LEASE_MIN` --
//! the discriminator between "nothing is happening" and "something slow is
//! happening" that the raw sweep did not have.
//!
//! Everything here runs the REAL `params.script_inline` of `extract-glue`
//! against a small in-test queue, so nothing costs anything and no model is
//! called.

use std::collections::BTreeMap;
use std::io::Write;
use std::process::{Command, Stdio};

const GLUE_CONFIG: &str = "../../templates/memory-hive/extract-glue/config.json";

/// `${VAR:-default}` becomes the default, a bare `${VAR}` becomes the empty
/// string -- the same substitution the colony performs when it instantiates the
/// template.
fn resolve_vars(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    let mut rest = script;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail
            .find('}')
            .expect("unterminated ${...} in script_inline");
        if let Some((_, default)) = tail[..end].split_once(":-") {
            out.push_str(default);
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

fn glue_script() -> String {
    let raw = std::fs::read_to_string(GLUE_CONFIG).expect("extract-glue config");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("config json");
    resolve_vars(
        v["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
    )
}

/// Run the real script with a real stdin document and return the emitted
/// messages.
fn emit(script: &str, doc: serde_json::Value) -> Vec<serde_json::Value> {
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(&meclaw_testing::code_stdin_bytes(&doc))
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "extract-glue exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

fn args_of(msg: &serde_json::Value) -> serde_json::Value {
    let text = msg["messages"][0]["text"].as_str().expect("op text");
    serde_json::from_str(text).expect("op args")
}

/// The single store op of an emission, with the phase the edge would promote.
fn only_op(msgs: &[serde_json::Value]) -> Option<(String, serde_json::Value)> {
    let msg = msgs.iter().find(|m| m["header"]["route"] == "xstore")?;
    Some((
        msg["header"]["phase"].as_str().unwrap_or("").to_string(),
        args_of(msg),
    ))
}

/// A document as the colony delivers it to the lane: the phase in the context,
/// the store's answer in the hop.
fn doc(
    phase: &str,
    operation: &str,
    rows_affected: i64,
    payload: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"mem_phase": phase},
            "hop": {"operation": operation, "rows_affected": rows_affected}
        },
        "messages": [{"origin": "user", "type": "text", "text": payload.to_string()}]
    })
}

// ---------------------------------------------------------------- tiny queue
//
// Just enough of a store to drive the lane's state machine: the three ops the
// extraction queue uses (insert, select, guarded update) over one table. The
// point is not to reimplement the store -- it is to make "how many items did
// this lane claim for N turns" a number a test can read.

type Row = BTreeMap<String, serde_json::Value>;

#[derive(Default)]
struct Queue {
    rows: Vec<Row>,
}

impl Queue {
    /// `where` semantics of the store, restricted to what this lane emits:
    /// scalar equality, `{"in": [...]}` and `{"lt": "..."}`.
    fn matches(row: &Row, filters: &serde_json::Value) -> bool {
        let Some(map) = filters.as_object() else {
            return true;
        };
        map.iter().all(|(col, want)| {
            let have = row.get(col).cloned().unwrap_or(serde_json::Value::Null);
            match want {
                serde_json::Value::Object(o) if o.contains_key("in") => {
                    o["in"].as_array().expect("in list").contains(&have)
                }
                serde_json::Value::Object(o) if o.contains_key("lt") => {
                    let bound = o["lt"].as_str().expect("lt is a string bound");
                    have.as_str().is_some_and(|h| h < bound)
                }
                other => *other == have,
            }
        })
    }

    fn apply(&mut self, op: &serde_json::Value) -> (i64, Vec<Row>) {
        match op["operation"].as_str().expect("operation") {
            "insert" => {
                let row: Row = serde_json::from_value(op["row"].clone()).expect("row object");
                self.rows.push(row);
                (1, vec![])
            }
            "select" => {
                let hits: Vec<Row> = self
                    .rows
                    .iter()
                    .filter(|r| Self::matches(r, &op["where"]))
                    .cloned()
                    .collect();
                (hits.len() as i64, hits)
            }
            "update" => {
                let set = op["set"].as_object().expect("set object");
                let mut n = 0;
                for row in self.rows.iter_mut() {
                    if Self::matches(row, &op["where"]) {
                        for (k, v) in set {
                            row.insert(k.clone(), v.clone());
                        }
                        n += 1;
                    }
                }
                (n, vec![])
            }
            other => panic!("the extraction queue never emits {other}"),
        }
    }

    fn episode_of(&self, id: &str) -> String {
        self.rows
            .iter()
            .find(|r| r["id"] == id)
            .and_then(|r| r["episode_id"].as_str())
            .unwrap_or_else(|| panic!("the claim names a row that is not in the queue: {id}"))
            .to_string()
    }

    fn with_status(&self, status: &str) -> Vec<&Row> {
        self.rows.iter().filter(|r| r["status"] == status).collect()
    }
}

/// Drive one lane step: hand the script a phase plus the store's answer, apply
/// whatever op comes back, and return the op together with the phase it named.
fn step(
    script: &str,
    q: &mut Queue,
    phase: &str,
    operation: &str,
    rows_affected: i64,
    payload: serde_json::Value,
) -> Option<(String, serde_json::Value, i64, Vec<Row>)> {
    let msgs = emit(script, doc(phase, operation, rows_affected, payload));
    let (next_phase, op) = only_op(&msgs)?;
    let (n, rows) = q.apply(&op);
    Some((next_phase, op, n, rows))
}

/// A store result set as the lane reads it: the rows are the payload, plain.
fn queue_payload(rows: &[Row]) -> serde_json::Value {
    serde_json::json!(rows)
}

/// One ingested turn: enqueue it, then run the gate. Returns the EPISODES the
/// gate claimed, which is empty whenever the gate parked. Episodes rather than
/// row ids on purpose -- what the extractor is paid for is a turn, and the
/// question #72 asks is how many times one turn is handed to it.
fn ingest_turn(script: &str, q: &mut Queue, episode: &str, tokens: i64) -> Vec<String> {
    let item = serde_json::json!({
        "episode_id": episode, "session_id": "s1", "sender": "user",
        "content": "a turn", "token_est": tokens
    });
    let Some((phase, _op, _n, _rows)) = step(script, q, "enqueue", "", 0, item) else {
        return vec![];
    };
    assert_eq!(
        phase, "gate",
        "the write lane enqueues and then opens the gate"
    );

    // gate -> the pending read
    let Some((phase, _op, n, rows)) = step(script, q, "gate", "insert", 1, serde_json::json!({}))
    else {
        return vec![];
    };
    assert_eq!(phase, "gate-eval");

    // gate-eval -> either a park (no op) or the guarded claim
    match step(script, q, "gate-eval", "select", n, queue_payload(&rows)) {
        None => vec![],
        Some((_phase, op, _n, _rows)) => op["where"]["id"]["in"]
            .as_array()
            .expect("the claim names its ids")
            .iter()
            .map(|v| q.episode_of(v.as_str().expect("id")))
            .collect(),
    }
}

/// The stall detector's sweep: "nothing has moved, hand the dead chains back".
/// Returns how many rows it actually took back.
fn recovery_sweep(script: &str, q: &mut Queue) -> i64 {
    let msgs = emit(
        script,
        serde_json::json!({
            "header": {
                "context": {"mem_phase": "flush", "flush_reclaim": "1"},
                "hop": {}
            },
            "messages": [{"origin": "user", "type": "text", "text": "{}"}]
        }),
    );
    let (phase, op) = only_op(&msgs).expect("the sweep emits its reclaim");
    assert_eq!(phase, "reclaimed");
    assert_eq!(op["table"], "pending_extraction");
    let (n, _) = q.apply(&op);
    n
}

// -------------------------------------------------------------------- tests

#[test]
fn a_claim_stamps_the_instant_it_was_taken() {
    // Without this column the sweep has nothing to reason about: every claimed
    // row looks the same whether its worker started a second ago or died an
    // hour ago, so the sweep has to assume the worst about all of them.
    let script = glue_script();
    let mut q = Queue::default();
    let claimed = ingest_turn(&script, &mut q, "e1", 999);
    assert_eq!(
        claimed.len(),
        1,
        "one turn over the token gate is one claim"
    );

    let row = q.with_status("claimed")[0].clone();
    let stamp = row["claimed_at"]
        .as_str()
        .expect("the claim carries its instant");
    assert!(
        stamp.starts_with("20") && stamp.ends_with('Z'),
        "claimed_at is an ISO instant, got {stamp:?}"
    );
}

#[test]
fn the_recovery_sweep_leaves_a_live_claim_alone() {
    // The whole of #72 in one assertion. The batch was claimed a moment ago and
    // its extractor call is still in flight; the stall detector fires because
    // the queue has not moved. The sweep must find nothing to do.
    let script = glue_script();
    let mut q = Queue::default();
    let claimed = ingest_turn(&script, &mut q, "e1", 999);
    assert_eq!(claimed.len(), 1);

    let taken_back = recovery_sweep(&script, &mut q);
    assert_eq!(
        taken_back, 0,
        "a claim younger than the lease is a worker that is still alive"
    );
    assert_eq!(
        q.with_status("claimed").len(),
        1,
        "the batch still owns its row"
    );
}

#[test]
fn the_recovery_sweep_still_frees_a_dead_chain() {
    // The lease narrows the sweep; it does not disable it. A chain that died
    // leaves its claim behind, the claim ages past the lease, and the next
    // sweep hands the row back -- which is the reason the sweep exists.
    let script = glue_script();
    let mut q = Queue::default();
    ingest_turn(&script, &mut q, "e1", 999);

    // The worker died an hour ago: age the claim past any sane lease.
    for row in q.rows.iter_mut() {
        row.insert(
            "claimed_at".into(),
            serde_json::json!("2020-01-01T00:00:00.000000Z"),
        );
    }

    let taken_back = recovery_sweep(&script, &mut q);
    assert_eq!(taken_back, 1, "an expired claim is a dead chain");
    let freed = &q.rows[0];
    assert_eq!(freed["status"], "pending", "the row is back in the queue");
    assert_eq!(freed["batch_id"], "", "and it belongs to no batch");
}

#[test]
fn sustained_ingest_claims_each_turn_exactly_once() {
    // The measurement the issue is named after, in miniature: turns arrive
    // continuously, every batch takes longer than the stall detector's patience,
    // and the sweep fires between turns. The number to watch is batched items
    // per turn -- 1.53 was the defect, 1.0 is the contract.
    //
    // Nothing here waits on wall time: each claim is stamped as the script runs,
    // so every claim in this test is young, which is exactly the condition the
    // old sweep could not recognise.
    let script = glue_script();
    let mut q = Queue::default();

    const TURNS: usize = 12;
    let mut claims: BTreeMap<String, usize> = BTreeMap::new();
    for i in 0..TURNS {
        // Each turn is over the token gate on its own, so every turn opens a
        // batch: the worst case for a sweep that cannot tell live from dead.
        for episode in ingest_turn(&script, &mut q, &format!("e{i}"), 999) {
            *claims.entry(episode).or_default() += 1;
        }
        // The extractor is slow, the queue looks frozen, the harness sweeps.
        recovery_sweep(&script, &mut q);
    }

    let batched_items: usize = claims.values().sum();
    let repeats: Vec<_> = claims.iter().filter(|(_, n)| **n > 1).collect();
    assert!(
        repeats.is_empty(),
        "these turns were handed to the extractor more than once: {repeats:?}"
    );
    assert_eq!(
        claims.len(),
        TURNS,
        "every ingested turn reaches the extractor exactly once; claimed: {claims:?}"
    );
    assert_eq!(
        batched_items, TURNS,
        "{batched_items} batched items for {TURNS} turns -- the lane is paying for turns twice"
    );
}

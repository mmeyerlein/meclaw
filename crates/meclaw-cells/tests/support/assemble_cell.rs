//! What every GH #728 lock needs from `collector/assemble`: the SHIPPED script, run
//! once per message the way a `code` cell runs it, with the shipped params merged
//! under the case's overrides, and the few readers that turn an emission back into
//! the store ops and hop keys a lock measures.
//!
//! One file instead of one copy per lock, the precedent `support/duplex_cell.rs`
//! set. The script goes to python3 on stdin, never in argv (GH #279: a single argv
//! string is capped at 128 KiB and the assembler is past it).
#![allow(dead_code)]

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

pub const ASSEMBLE: &str = "templates/collector/assemble/config.json";
pub const DISPATCHER: &str = "templates/dispatcher/config.json";
pub const ASSISTANT: &str = "templates/assistant/config.json";
pub const TALKY_REF: &str = "templates/assistant/talky/config.json";
pub const TALKY_CHAT_REF: &str = "templates/assistant/talky-chat/config.json";

/// The session every fixture below speaks in.
pub const SESSION: &str = "s1";

pub fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

pub fn config_of(rel: &str) -> Value {
    let p = repo(rel);
    let raw = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// The shipped params of a code cell, minus its source, with `over` merged on top.
/// A knob the config does not carry is a failure, so a typo cannot pass as a
/// silently ignored override.
pub fn params_of(rel: &str, over: &[(&str, Value)]) -> Value {
    let mut p = config_of(rel)["params"]
        .as_object()
        .cloned()
        .expect("params object");
    p.remove("script_inline");
    for (k, v) in over {
        assert!(p.contains_key(*k), "{rel}: no such param: {k}");
        p.insert((*k).to_string(), v.clone());
    }
    Value::Object(p)
}

/// Run a shipped code cell over one message. Returns the emissions and stderr.
pub fn run_cell(rel: &str, over: &[(&str, Value)], doc: Value) -> (Vec<Value>, String) {
    let (stdout, stderr) = run_cell_stdout(rel, over, doc);
    let v: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("{rel}: output is not JSON ({e}): {stdout}"));
    let msgs = match v {
        Value::Array(a) => a,
        one => vec![one],
    };
    (msgs, stderr)
}

/// Run a shipped code cell over one message and hand back its stdout as written,
/// for a golden that compares bytes (key order, one emission or a list) rather
/// than parsed values (GH #1304).
pub fn run_cell_stdout(rel: &str, over: &[(&str, Value)], doc: Value) -> (String, String) {
    let script = config_of(rel)["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string();
    let mut doc = doc;
    doc["params"] = params_of(rel, over);
    let stdin_doc = meclaw_testing::code_stdin(&doc).to_string();
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        serde_json::to_string(&script).unwrap(),
        serde_json::to_string(&stdin_doc).unwrap(),
    );
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    let out = child.wait_with_output().expect("wait");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "{rel} exited non-zero: {stderr}");
    (String::from_utf8_lossy(&out.stdout).to_string(), stderr)
}

pub fn assemble(over: &[(&str, Value)], doc: Value) -> Vec<Value> {
    run_cell(ASSEMBLE, over, doc).0
}

/// D58 GH #1480: a judge context of before (`{turns, topic, screen: {topics},
/// hints?, now: {iso, weekday, timezone}, place}`) as the curator's view for
/// talky's target `decide`, `(messages, system)`: the turns as `messages`
/// (`origin` = the role), the leaf `topic` when there is one, the leaf
/// `frame` when there is a moment (one `<name>: <value>` per line,
/// OR-d58-2.judge.8), the screen topics as the leaf of the source `presenter`
/// and the hints as the leaf of `affinity` (the route's `view_sources`). The
/// curator keeps no screen state (master's history answered `state: []` on
/// every read), so a view carries none.
pub fn talky_view_of(judge: &Value) -> (Value, Value) {
    let messages: Vec<Value> = judge["turns"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|t| json!({"origin": t["role"], "type": "text", "text": t["text"]}))
        .collect();
    let word = |v: &Value| v.as_str().unwrap_or("").to_string();
    let now = &judge["now"];
    let mut system = json!({"decide": {}});
    if !word(&now["iso"]).is_empty() {
        system["frame"] = json!({"text": format!(
            "now: {}\nweekday: {}\ntimezone: {}\nplace: {}",
            word(&now["iso"]),
            word(&now["weekday"]),
            word(&now["timezone"]),
            word(&judge["place"])
        )});
    }
    if !word(&judge["topic"]).is_empty() {
        system["topic"] = json!({"text": judge["topic"]});
    }
    if judge["screen"]["topics"].is_array() {
        system["decide"]["presenter"] = json!({"text": judge["screen"]["topics"].to_string()});
    }
    if judge["hints"].is_array() {
        system["decide"]["affinity"] = json!({"text": judge["hints"].to_string()});
    }
    (Value::Array(messages), system)
}

/// A lane arrival as the hive's door hands it to `./assemble`.
pub fn lane(route: &str, hop: Value, ctx: Value, messages: Value) -> Value {
    let mut h = json!({"route": route});
    for (k, v) in hop.as_object().expect("hop object") {
        h[k] = v.clone();
    }
    let mut c = json!({"session_id": SESSION});
    for (k, v) in ctx.as_object().expect("ctx object") {
        c[k] = v.clone();
    }
    json!({"header": {"context": c, "hop": h}, "messages": messages})
}

/// A store BUNDLE reply as `./window` hands it back: the phase and the round key
/// in context (the cstore edge promoted both), one leg per `(tool_call_id, rows)`.
pub fn bundle_reply(phase: &str, turn_id: &str, legs: &[(&str, Value)]) -> Value {
    json!({
        "header": {"context": {"session_id": SESSION, "turn_id": turn_id, "iter": "0",
                               "col_phase": phase, "store_origin": "collector"},
                   "hop": {"operation": "bundle", "rows_affected": 1, "bundle_errors": 0}},
        "messages": legs.iter().map(|(id, rows)| json!(
            {"origin": "tool", "type": "tool_result", "id": id, "text": rows.to_string()}))
            .collect::<Vec<_>>(),
        "results": legs.iter().map(|(id, _)| json!(
            {"tool_call_id": id, "operation": "select", "rows_affected": 1,
             "duration_ms": 0})).collect::<Vec<_>>()
    })
}

/// Every `(tool_call_id, store args)` of one `cstore` emission, in order.
pub fn calls_of(msg: &Value) -> Vec<(String, Value)> {
    msg["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|m| {
            let id = m["id"].as_str().unwrap_or_default().to_string();
            let args: Value =
                serde_json::from_str(m["text"].as_str().expect("op text")).expect("op json");
            (id, args)
        })
        .collect()
}

/// The store args under one `tool_call_id` of a `cstore` emission.
pub fn call(msg: &Value, id: &str) -> Value {
    calls_of(msg)
        .into_iter()
        .find(|(cid, _)| cid == id)
        .map(|(_, a)| a)
        .unwrap_or_else(|| panic!("no call `{id}` in {msg}"))
}

/// The one emission on `route`.
pub fn on_route<'a>(out: &'a [Value], route: &str) -> &'a Value {
    out.iter()
        .find(|m| m["header"]["route"].as_str() == Some(route))
        .unwrap_or_else(|| panic!("nothing on route `{route}`: {out:?}"))
}

/// The one `cstore` emission of this phase.
pub fn in_phase<'a>(out: &'a [Value], phase: &str) -> &'a Value {
    out.iter()
        .find(|m| {
            m["header"]["route"].as_str() == Some("cstore")
                && m["header"]["phase"].as_str() == Some(phase)
        })
        .unwrap_or_else(|| panic!("no cstore in phase `{phase}`: {out:?}"))
}

pub fn hop_str(msg: &Value, key: &str) -> String {
    msg["header"][key]
        .as_str()
        .unwrap_or_else(|| panic!("the header carries no `{key}`: {msg}"))
        .to_string()
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis() as i64
}

/// An advisor's answer coming home on `in_advice`, under its consult id.
pub fn advice(consult: &str, text: &str) -> Value {
    lane(
        "in_advice",
        json!({"turn_id": "cogny-round-1"}),
        json!({"consult_id": consult, "turn_id": "cogny-round-1"}),
        json!([{"origin": "assistant", "type": "text", "text": text}]),
    )
}

/// Stage B of an advice: the lookup came back. `departs` is what the depart select
/// found; `rid` and `text` are the advice's own row, read back.
pub fn advice_looked(prov: &str, departs: Value, rid: &str, text: &str, consult: &str) -> Value {
    bundle_reply(
        "advice-look",
        prov,
        &[
            ("c-look-depart", departs),
            ("c-look-turn", Value::Null),
            (
                "c-look-self",
                json!([{"id": rid, "content": text, "consult_id": consult}]),
            ),
        ],
    )
}

/// The brain's answer reaching the collector under a round key.
pub fn answer_in(round_key: &str, text: &str) -> Value {
    lane(
        "in_answer",
        json!({}),
        json!({"turn_id": round_key, "iter": "1"}),
        json!([{"origin": "assistant", "type": "text", "text": text}]),
    )
}

/// Run an advice through both stages and return the round key it opened.
pub fn open_advice(consult: &str, departs: Value) -> (String, Vec<Value>) {
    let a = assemble(&[], advice(consult, "berlin: 21C"));
    let look = in_phase(&a, "advice-look");
    let prov = hop_str(look, "turn_id");
    let row = call(look, "c-look-turn");
    let rid = row["row"]["id"].as_str().expect("row id").to_string();
    let b = assemble(
        &[],
        advice_looked(&prov, departs, &rid, "berlin: 21C", consult),
    );
    let key = hop_str(in_phase(&b, "turn-open"), "turn_id");
    (key, b)
}

// oneanswer

/// The store `./window` as far as the rows of a round go, kept in the test
/// (D54g oneanswer, GH #1505). The ops `collector/assemble` sends on `cstore`
/// are run here in the order they leave and answered in the two shapes
/// `./window` answers in: one call as a single-op reply, two or more as a
/// bundle. A pin that drives the cell through it measures what leaves on
/// `answer` and what the store holds, not which phase or call id carried it.
/// `order_by` is not read: rows come back in the order they were written.
pub struct WindowStore {
    tables: std::collections::BTreeMap<String, Vec<Value>>,
    /// Every op the cell sent, in order, as `(table, operation, args)`.
    pub ops: Vec<(String, String, Value)>,
    /// The call ids this store refuses (`refusing`): such an op runs not at
    /// all and answers `sql_error`, the others of its bundle run as usual.
    refused: std::collections::BTreeSet<String>,
}

fn cell_matches(have: &Value, want: &Value) -> bool {
    let Some(cond) = want.as_object() else {
        return have == want;
    };
    // `or_null` wraps one comparison: a missing value matches too, as the
    // store renders it (`IS NULL OR …`).
    if let Some(inner) = cond.get("or_null") {
        return have.is_null() || cell_matches(have, inner);
    }
    cond.iter().all(|(op, v)| {
        let ord = match (have.as_str(), v.as_str()) {
            (Some(a), Some(b)) => a.cmp(b),
            _ => have
                .as_f64()
                .unwrap_or(0.0)
                .total_cmp(&v.as_f64().unwrap_or(0.0)),
        };
        match op.as_str() {
            "eq" => have == v,
            "neq" => have != v,
            "in" => v.as_array().is_some_and(|a| a.contains(have)),
            "lt" => ord.is_lt(),
            "lte" => ord.is_le(),
            "gt" => ord.is_gt(),
            "gte" => ord.is_ge(),
            other => panic!("the test store knows no comparison `{other}`: {want}"),
        }
    })
}

fn row_matches(row: &Value, filter: &Value) -> bool {
    filter.as_object().is_none_or(|w| {
        w.iter()
            .all(|(k, want)| cell_matches(row.get(k).unwrap_or(&Value::Null), want))
    })
}

impl WindowStore {
    /// A store that holds `rows` in `table` and nothing else.
    pub fn holding(table: &str, rows: Vec<Value>) -> Self {
        let mut tables = std::collections::BTreeMap::new();
        tables.insert(table.to_string(), rows);
        Self {
            tables,
            ops: Vec::new(),
            refused: std::collections::BTreeSet::new(),
        }
    }

    /// The same store, refusing every op sent under the call id `id` with
    /// `sql_error` -- the store's way of failing one leg of a bundle while the
    /// legs around it run (review M-3 of D54g oneanswer, error injection).
    #[must_use]
    pub fn refusing(mut self, id: &str) -> Self {
        self.refused.insert(id.to_string());
        self
    }

    /// Answer the ops under call id `id` again, after `refusing`: one leg
    /// refused once while a later op under the same id runs.
    pub fn heal(&mut self, id: &str) {
        self.refused.remove(id);
    }

    /// The rows of one table as the store holds them now.
    pub fn rows(&self, table: &str) -> Vec<Value> {
        self.tables.get(table).cloned().unwrap_or_default()
    }

    /// Run one op; returns `(operation, rows read, rows affected)`.
    fn run(&mut self, args: &Value) -> (String, Value, usize) {
        let table = args["table"]
            .as_str()
            .unwrap_or_else(|| panic!("a store op names no table: {args}"))
            .to_string();
        let operation = args["operation"]
            .as_str()
            .unwrap_or_else(|| panic!("a store op names no operation: {args}"))
            .to_string();
        self.ops
            .push((table.clone(), operation.clone(), args.clone()));
        let rows = self.tables.entry(table).or_default();
        let (read, affected) = match operation.as_str() {
            "select" => {
                let mut found: Vec<Value> = rows
                    .iter()
                    .filter(|r| row_matches(r, &args["where"]))
                    .cloned()
                    .collect();
                if let Some(n) = args["limit"].as_u64() {
                    found.truncate(usize::try_from(n).unwrap_or(usize::MAX));
                }
                if let Some(cols) = args["columns"].as_array() {
                    found = found
                        .iter()
                        .map(|r| {
                            let mut o = serde_json::Map::new();
                            for c in cols.iter().filter_map(Value::as_str) {
                                o.insert(c.to_string(), r.get(c).cloned().unwrap_or(Value::Null));
                            }
                            Value::Object(o)
                        })
                        .collect();
                }
                let n = found.len();
                (Value::Array(found), n)
            }
            "insert" => {
                if args["if_absent"].is_object()
                    && rows.iter().any(|r| row_matches(r, &args["if_absent"]))
                {
                    (json!([]), 0)
                } else {
                    if args["replace"].is_object() {
                        rows.retain(|r| !row_matches(r, &args["replace"]));
                    }
                    rows.push(args["row"].clone());
                    (json!([]), 1)
                }
            }
            "delete" => {
                let before = rows.len();
                rows.retain(|r| !row_matches(r, &args["where"]));
                (json!([]), before - rows.len())
            }
            "update" => {
                let mut n = 0;
                for r in rows.iter_mut() {
                    if !row_matches(r, &args["where"]) {
                        continue;
                    }
                    if let Some(set) = args["set"].as_object() {
                        for (k, v) in set {
                            r[k.as_str()] = v.clone();
                        }
                    }
                    n += 1;
                }
                (json!([]), n)
            }
            other => panic!("the test store knows no operation `{other}`: {args}"),
        };
        (operation, read, affected)
    }

    /// Run the ops of one `cstore` emission and build the reply `./window`
    /// hands back: the context of the message the cell was handling, with the
    /// keys the hive's `cstore` edge promotes from the hop (`col_phase`,
    /// `turn_id`, `session_id`, `iter`, `col_gate`, `store_origin`).
    pub fn reply(&mut self, cstore: &Value, ctx: &Value) -> Value {
        let head = &cstore["header"];
        let mut context = ctx.clone();
        for (to, from) in [
            ("col_phase", "phase"),
            ("turn_id", "turn_id"),
            ("session_id", "session_id"),
            ("iter", "iter"),
            ("col_gate", "gate"),
        ] {
            context[to] = head[from].clone();
        }
        context["store_origin"] = json!("collector");
        let mut legs: Vec<(String, String, Value, usize, bool)> = Vec::new();
        for (id, args) in calls_of(cstore) {
            if self.refused.contains(&id) {
                let operation = args["operation"].as_str().unwrap_or_default().to_string();
                legs.push((id, operation, json!({"code": "sql_error"}), 0, true));
                continue;
            }
            let (operation, read, affected) = self.run(&args);
            legs.push((id, operation, read, affected, false));
        }
        let messages: Vec<Value> = legs
            .iter()
            .map(|(id, _, read, _, _)| {
                json!({"origin": "tool", "type": "tool_result", "id": id,
                       "text": read.to_string()})
            })
            .collect();
        if let [(_, operation, _, affected, refused)] = legs.as_slice() {
            let mut hop = json!({"operation": operation, "rows_affected": affected});
            if *refused {
                hop["error_code"] = json!("sql_error");
            }
            return json!({
                "header": {"context": context.clone(), "hop": hop},
                "messages": messages
            });
        }
        let results: Vec<Value> = legs
            .iter()
            .map(|(id, operation, _, affected, refused)| {
                let mut r = json!({"tool_call_id": id, "operation": operation,
                                   "rows_affected": affected, "duration_ms": 0});
                if *refused {
                    r["error_code"] = json!("sql_error");
                }
                r
            })
            .collect();
        let errors = legs.iter().filter(|leg| leg.4).count();
        json!({
            "header": {"context": context,
                       "hop": {"operation": "bundle", "rows_affected": legs.len(),
                               "bundle_errors": errors}},
            "messages": messages,
            "results": results
        })
    }
}

/// Feed one message to the shipped `collector/assemble` and then every reply
/// of the store it leads to, until the cell sends no store op any more.
/// Returns what left on every other route, in order. The step cap is a hang
/// mark, not a verdict about the cell.
pub fn drive(store: &mut WindowStore, doc: Value) -> Vec<Value> {
    drive_with(&[], store, doc)
}

/// `drive` with the case's params laid over the shipped ones.
pub fn drive_with(over: &[(&str, Value)], store: &mut WindowStore, doc: Value) -> Vec<Value> {
    let mut left = Vec::new();
    let mut queue = std::collections::VecDeque::from([doc]);
    let mut steps = 0;
    while let Some(msg) = queue.pop_front() {
        steps += 1;
        assert!(steps <= 64, "the cell and its store never came to rest");
        let ctx = msg["header"]["context"].clone();
        for e in assemble(over, msg) {
            if e["header"]["route"].as_str() == Some("cstore") {
                queue.push_back(store.reply(&e, &ctx));
            } else {
                left.push(e);
            }
        }
    }
    left
}

/// A store that holds the opening row of round `turn_id` and nothing else.
/// Since GH #1505 every interim leaves through `bridge-gate`, which lets it
/// out only while its round is open.
pub fn open_round_of(turn_id: &str) -> WindowStore {
    WindowStore::holding(
        "round",
        vec![json!({"turn_id": turn_id, "session_id": SESSION, "iter": 0,
                    "role": "leg-window", "turn": "{}", "fired": 1, "recorded_at": ""})],
    )
}

/// What leaves when `doc` meets a store that holds nothing. Since GH #1505 no
/// answer leaves `collector/assemble` in the call that brought it: it goes
/// through `answer-gate` and leaves on the store's reply.
pub fn through_store(doc: Value) -> Vec<Value> {
    drive(&mut WindowStore::holding("round", Vec::new()), doc)
}

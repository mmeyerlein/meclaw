//! GH #888 -- the curator hive, cell by cell, against a ledger that is a real store.
//!
//! `curator@1.0.0` owns the window of one model and keeps a ledger of every
//! call. What is pinned here is the contract other strands write against
//! (plan K § 1) and the behaviour behind it (§ 2), one group per task of the
//! plan: the shape, the intake, the window and the call record, the tap and the
//! clock, the rebuild, the writer, the pack.
//!
//! Nothing is mocked that the substrate would do: the three shipped
//! `script_inline` programs run under python3 against real stdin documents; the
//! edges are the shipped `params.graph` of the hive, evaluated by the colony's
//! own CEL (`meclaw_colony::cel_eval`); the ledger is an in-memory SQLite with
//! the shipped schema and seed, and every store operation runs through the
//! store cell's own dispatcher (`meclaw_cells::store::ops::dispatch`) and comes
//! back in its own reply shape (`store::output`). Only the clock and the
//! summarizer are recorders the test answers by hand -- a strike and a summary
//! are the two things a test has to be able to hold back.
//!
//! The colony case -- the same hive booted, a real `llm` cell and a stub
//! provider -- is `gh888_the_ledger_rebuilds_what_the_provider_received.rs`.

use meclaw_colony::cel_eval::{
    CompiledCondition, CompiledModifier, apply_modifier, evaluate_condition, parse_condition,
    parse_modifier,
};
use meclaw_colony::config::ModifierSpec;
use meclaw_core::Headers;
use meclaw_core::serde_json::{self as sj, Map, Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// R2b / GH #49: a tree without the template skips.
fn shipped() -> bool {
    repo("templates/curator/config.json").is_file()
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    sj::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn cell_config(name: &str) -> Value {
    read_json(&repo(&format!("templates/curator/{name}/config.json")))
}

fn script_of(name: &str) -> String {
    cell_config(name)["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string()
}

/// Run a program under python3, the program itself on stdin (a single argv
/// string is capped at 128 KiB; same harness as `collector_window.rs`).
fn run_python(script: &str, stdin_doc: &str) -> std::process::Output {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        sj::to_string(script).unwrap(),
        sj::to_string(stdin_doc).unwrap(),
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
    child.wait_with_output().expect("wait")
}

#[derive(Clone, Debug, Default)]
struct Msg {
    context: Map<String, Value>,
    hop: Map<String, Value>,
    body: Map<String, Value>,
    reply_to: String,
}

impl Msg {
    fn route(&self) -> &str {
        self.hop.get("route").and_then(Value::as_str).unwrap_or("")
    }
    fn messages(&self) -> Vec<Value> {
        self.body
            .get("messages")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }
}

fn obj(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

struct Edge {
    from: String,
    to: String,
    condition: Option<CompiledCondition>,
    modifier: Option<CompiledModifier>,
}

/// The curator hive in one process: its cells, its edges, its ledger.
struct Hive {
    edges: Vec<Edge>,
    cells: BTreeMap<String, Value>,
    db: rusqlite::Connection,
    out: Vec<Msg>,
    clock: Vec<Msg>,
    summ: VecDeque<Msg>,
    stderr: Vec<String>,
}

impl Hive {
    fn new() -> Self {
        Self::with(&[])
    }

    /// The shipped hive with `(cell, param, value)` overrides.
    fn with(over: &[(&str, &str, Value)]) -> Self {
        let hive = read_json(&repo("templates/curator/config.json"));
        let mut edges = Vec::new();
        for e in hive["params"]["graph"]["edges"].as_array().expect("edges") {
            let condition = e["condition"]
                .as_str()
                .map(|c| parse_condition(c).unwrap_or_else(|x| panic!("{c}: {x}")));
            let modifier = if e["modifier"].is_object() {
                let spec: ModifierSpec =
                    sj::from_value(e["modifier"].clone()).expect("a modifier spec");
                Some(parse_modifier(&spec).expect("a modifier"))
            } else {
                None
            };
            edges.push(Edge {
                from: e["from"].as_str().unwrap().to_string(),
                to: e["to"].as_str().unwrap().to_string(),
                condition,
                modifier,
            });
        }
        let mut cells = BTreeMap::new();
        for name in [
            "intake",
            "policy",
            "writer",
            "ledger",
            "summarizer",
            "clock",
            // GH #896: `./policy` hands the first call of a session it did not
            // serve last to `./handover`, so every round passes it once.
            "handover",
        ] {
            cells.insert(name.to_string(), cell_config(name));
        }
        for (cell, key, value) in over {
            let params = cells
                .get_mut(*cell)
                .and_then(|c| c["params"].as_object_mut())
                .expect("a cell with params");
            assert!(params.contains_key(*key), "no such param: {cell}.{key}");
            params.insert((*key).to_string(), value.clone());
        }
        let db = rusqlite::Connection::open_in_memory().expect("sqlite");
        let schema: BTreeMap<String, BTreeMap<String, String>> =
            sj::from_value(cells["ledger"]["params"]["schema"].clone()).expect("schema");
        meclaw_cells::store::ddl::apply_schema_ddl(&db, &schema).expect("ddl");
        meclaw_cells::store::seed::load_seed_if_present(
            &db,
            &repo("templates/curator/ledger"),
            &schema,
        )
        .expect("seed");
        Self {
            edges,
            cells,
            db,
            out: Vec::new(),
            clock: Vec::new(),
            summ: VecDeque::new(),
            stderr: Vec::new(),
        }
    }

    fn run_cell(&mut self, name: &str, msg: &Msg) -> Vec<Msg> {
        let cfg = &self.cells[name];
        let script = cfg["params"]["script_inline"].as_str().expect("script");
        let mut params = obj(cfg["params"].clone());
        params.remove("script_inline");
        let doc = json!({
            "envelope": {"header": {"context": msg.context, "hop": msg.hop},
                         "target": format!("/x/curator/{name}"),
                         "reply_to": msg.reply_to},
            "body": msg.body,
            "params": params,
        });
        let out = run_python(script, &doc.to_string());
        let err = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(out.status.success(), "{name} exited non-zero: {err}");
        if !err.trim().is_empty() {
            self.stderr.push(format!("{name}: {err}"));
        }
        let emitted: Value = sj::from_slice(&out.stdout).unwrap_or_else(|e| {
            panic!(
                "{name}: output is not JSON ({e}): {}",
                String::from_utf8_lossy(&out.stdout)
            )
        });
        let list = match emitted {
            Value::Array(a) => a,
            other => vec![other],
        };
        list.into_iter()
            .map(|m| {
                let mut body = obj(m);
                let hop = body
                    .remove("header")
                    .and_then(|h| h.as_object().cloned())
                    .unwrap_or_default();
                Msg {
                    context: msg.context.clone(),
                    hop,
                    body,
                    reply_to: String::new(),
                }
            })
            .collect()
    }

    /// One store message answered the way the store cell answers it.
    fn store(&mut self, msg: &Msg) -> Msg {
        use meclaw_cells::store::ops::dispatch;
        use meclaw_cells::store::output::{BundleLeg, build_bundle_result, build_tool_result};
        let calls: Vec<Value> = msg
            .messages()
            .into_iter()
            .filter(|m| m["type"] == "tool_call")
            .collect();
        let (body, hop) = if calls.len() == 1 {
            let c = &calls[0];
            let args: Value = sj::from_str(c["text"].as_str().unwrap_or("")).expect("op json");
            match dispatch(&self.db, &args) {
                Ok(outcome) => {
                    build_tool_result(&outcome, c["id"].as_str().unwrap_or("").to_string(), 0)
                }
                Err(e) => (
                    json!({"messages": [{"origin": "tool", "type": "tool_result",
                                         "text": e, "id": ""}]}),
                    obj(
                        json!({"operation": "error", "rows_affected": 0, "duration_ms": 0,
                               "finish_reason": "error", "error_code": "invalid_input"}),
                    ),
                ),
            }
        } else {
            let legs: Vec<BundleLeg> = calls
                .iter()
                .map(|c| {
                    let id = c["id"].as_str().unwrap_or("").to_string();
                    let args: Value =
                        sj::from_str(c["text"].as_str().unwrap_or("")).expect("op json");
                    match dispatch(&self.db, &args) {
                        Ok(outcome) => BundleLeg::from_outcome(&outcome, id, 0),
                        Err(e) => BundleLeg::refusal(
                            args["operation"].as_str().unwrap_or("error"),
                            id,
                            0,
                            "invalid_input",
                            e,
                        ),
                    }
                })
                .collect();
            build_bundle_result(&legs, 0)
        };
        for r in body["results"].as_array().cloned().unwrap_or_default() {
            if let Some(code) = r["error_code"].as_str() {
                panic!("a ledger op failed ({code}): {r} in {:?}", msg.body);
            }
        }
        if let Some(code) = hop.get("error_code").and_then(Value::as_str) {
            panic!("a ledger op failed ({code}): {body} in {:?}", msg.body);
        }
        Msg {
            context: msg.context.clone(),
            hop,
            body: obj(body),
            reply_to: String::new(),
        }
    }

    fn route(&self, from: &str, msg: &Msg) -> Vec<(String, Msg)> {
        let mut out = Vec::new();
        for e in self.edges.iter().filter(|e| e.from == from) {
            if let Some(c) = &e.condition
                && !matches!(evaluate_condition(c, &msg.context, &msg.hop), Ok(true))
            {
                continue;
            }
            let mut m = msg.clone();
            if let Some(modif) = &e.modifier {
                let h = Headers::from_parts(msg.context.clone(), msg.hop.clone());
                match apply_modifier(modif, &h) {
                    Ok(h) => {
                        m.context = h.context;
                        m.hop = h.hop;
                    }
                    Err(_) => continue,
                }
            }
            out.push((e.to.clone(), m));
        }
        out
    }

    fn pump(&mut self, from: &str, msg: Msg) {
        let mut queue = VecDeque::from([(from.to_string(), msg)]);
        let mut steps = 0;
        while let Some((from, msg)) = queue.pop_front() {
            steps += 1;
            assert!(steps < 4000, "the hive does not come to rest");
            for (to, m) in self.route(&from, &msg) {
                match to.as_str() {
                    "." => self.out.push(m),
                    "./ledger" => {
                        let answer = self.store(&m);
                        queue.push_back(("./ledger".to_string(), answer));
                    }
                    "./clock" => self.clock.push(m),
                    "./summarizer" => self.summ.push_back(m),
                    cell => {
                        let name = cell.trim_start_matches("./").to_string();
                        for o in self.run_cell(&name, &m) {
                            queue.push_back((cell.to_string(), o));
                        }
                    }
                }
            }
        }
    }

    /// A message arriving on the hive path on lane `route`.
    fn lane(&mut self, route: &str, context: Value, hop: Value, body: Value) {
        self.lane_from(route, context, hop, body, "");
    }

    fn lane_from(&mut self, route: &str, context: Value, hop: Value, body: Value, reply_to: &str) {
        let mut hop = obj(hop);
        hop.insert("route".into(), json!(route));
        let msg = Msg {
            context: obj(context),
            hop,
            body: obj(body),
            reply_to: reply_to.to_string(),
        };
        self.pump(".", msg);
    }

    /// The strike of an order the clock holds, as the timer emits it.
    fn fire(&mut self, order: &Msg) {
        let msg = Msg {
            context: Map::new(),
            hop: obj(json!({
                "event_id": "0190a3f2-0000-7000-8000-000000000888",
                "schedule_id": order.body["schedule_id"],
                "schedule_name": order.body["schedule_name"],
                "scheduled_at": order.body["at"], "fired_at": order.body["at"]})),
            body: obj(order.body["emit_body"].clone()),
            reply_to: String::new(),
        };
        self.pump("./clock", msg);
    }

    /// The summarizer's answer to the oldest request it holds.
    fn answer(&mut self, text: &str, finish: &str) -> Msg {
        let req = self.summ.pop_front().expect("a summarizer request");
        let msg = Msg {
            context: req.context.clone(),
            hop: obj(json!({"finish_reason": finish, "model": "summary-model"})),
            body: obj(json!({"messages": [{"origin": "assistant", "type": "text", "text": text}]})),
            reply_to: String::new(),
        };
        self.pump("./summarizer", msg);
        req
    }

    fn rows(&self, sql: &str) -> Vec<Vec<Value>> {
        use rusqlite::types::ValueRef;
        let mut st = self.db.prepare(sql).expect(sql);
        let n = st.column_count();
        st.query_map([], |r| {
            Ok((0..n)
                .map(|i| match r.get_ref(i).unwrap() {
                    ValueRef::Null => Value::Null,
                    ValueRef::Integer(x) => json!(x),
                    ValueRef::Real(x) => json!(x),
                    ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                    ValueRef::Blob(_) => Value::Null,
                })
                .collect())
        })
        .expect(sql)
        .map(Result::unwrap)
        .collect()
    }

    fn state(&self, key: &str) -> String {
        self.rows(&format!("SELECT value FROM state WHERE key = '{key}'"))
            .first()
            .and_then(|r| r[0].as_str().map(str::to_string))
            .unwrap_or_default()
    }

    fn routed(&self, route: &str) -> Vec<Msg> {
        self.out
            .iter()
            .filter(|m| m.route() == route)
            .cloned()
            .collect()
    }

    /// One round on `in_curate`; returns the call that left for the model.
    fn curate(&mut self, session: &str, turn: &str, iter: u32, round: Value, system: Value) -> Msg {
        self.out.clear();
        let hop = json!({"session_id": session, "turn_id": turn, "iter": iter.to_string(),
                         "phase": ""});
        let ctx = json!({"session_id": session, "turn_id": turn, "iter": iter.to_string(),
                         "channel": "test", "audience_set": "[\"member:test\"]"});
        let mut body = json!({"messages": round});
        if system.is_object() {
            body["system"] = system;
        }
        self.lane("in_curate", ctx, hop, body);
        let calls = self.routed("brain");
        assert_eq!(calls.len(), 1, "one round, one call: {:?}", self.out);
        calls[0].clone()
    }

    /// The model's output on the tap, with the context the parent edge promotes.
    fn tap(&mut self, call: &Msg, finish: &str, extra: Value, messages: Value) {
        let mut ctx = call.context.clone();
        for k in ["curator_call", "turn_id", "session_id", "iter"] {
            ctx.insert(k.into(), call.hop[k].clone());
        }
        let mut hop = obj(json!({"finish_reason": finish, "model": "test-model"}));
        for (k, v) in obj(extra) {
            hop.insert(k, v);
        }
        self.lane(
            "in_llm",
            Value::Object(ctx),
            Value::Object(hop),
            json!({"messages": messages}),
        );
    }
}

fn user(text: &str) -> Value {
    json!({"origin": "user", "type": "text", "text": text})
}

fn said(text: &str) -> Value {
    json!({"origin": "assistant", "type": "text", "text": text})
}

fn tool_call(id: &str, name: &str) -> Value {
    json!({"origin": "assistant", "type": "tool_call", "id": id,
           "text": json!({"name": name, "arguments": "{}"}).to_string()})
}

fn tool_result(id: &str, text: &str) -> Value {
    json!({"origin": "tool", "type": "tool_result", "id": id, "text": text})
}

fn mode(text: &str) -> Value {
    json!({"instructions": {"mode": {"text": text}}})
}

fn sha256_hex(s: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn texts(call: &Msg) -> Vec<String> {
    call.messages()
        .iter()
        .map(|m| m["text"].as_str().unwrap_or("").to_string())
        .collect()
}

/// A whole turn: the round in, the final answer back on the tap.
fn turn(h: &mut Hive, session: &str, turn_id: &str, ask: &str, reply: &str, extra: Value) -> Msg {
    let call = h.curate(session, turn_id, 0, json!([user(ask)]), mode("Be brief."));
    h.tap(&call, "stop", extra, json!([said(reply)]));
    call
}

fn clock_orders(h: &Hive) -> Vec<(String, String)> {
    h.clock
        .iter()
        .map(|m| {
            (
                m.body["op"].as_str().unwrap_or("").to_string(),
                m.body["schedule_id"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

fn last_add(h: &Hive) -> Msg {
    h.clock
        .iter()
        .rev()
        .find(|m| m.body["op"] == "add")
        .cloned()
        .expect("an add order")
}

// ============================================================ 1. the shape

#[test]
fn curator_template_shape() {
    if !shipped() {
        return;
    }
    let t = read_json(&repo("templates/curator/template.json"));
    assert_eq!(t["name"], "curator");
    assert_eq!(t["version"], "1.1.1");
    let hive = read_json(&repo("templates/curator/config.json"));
    assert_eq!(hive["cell"]["type"], "hive");
    assert_eq!(hive["params"]["ports"], json!([]), "sealed");
    let lanes = |k: &str| -> Vec<String> {
        hive["params"]["contract"][k]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["route"].as_str().unwrap().to_string())
            .collect()
    };
    // GH #892 added the sections, the pin door and the menu question, and
    // the two routes that answer them.
    assert_eq!(
        lanes("accepts"),
        [
            "in_curate",
            "in_slots",
            "in_llm",
            "in_pack",
            "in_close",
            "in_model",
            "in_section",
            "in_pin",
            "in_schemas",
            "in_history_call",
            "in_recall_ask",
            "in_gap_bundle",
            "in_renewed"
        ]
    );
    assert_eq!(
        lanes("emits"),
        [
            "brain",
            "turn_write",
            "write",
            "pack_ack",
            "model_refused",
            "sidecar",
            "tool_schemas",
            "tool_result",
            "recall"
        ]
    );
    let types: Vec<(&str, &str)> = vec![
        ("intake", "code"),
        ("policy", "code"),
        ("writer", "code"),
        ("ledger", "store"),
        ("summarizer", "llm"),
        ("clock", "timer"),
        ("schemas", "code"),
        ("history", "code"),
        ("push", "code"),
        ("handover", "code"),
    ];
    for (cell, ty) in &types {
        let cfg = cell_config(cell);
        assert_eq!(cfg["cell"]["type"], *ty, "{cell}");
        assert!(
            cfg["contract"]["emits"]["hop"].is_object(),
            "{cell} declares what it emits"
        );
    }
    let ledger = cell_config("ledger");
    let schema = &ledger["params"]["schema"];
    let expect = json!({
        "blocks": ["hash", "kind", "chars", "body", "first_seen"],
        "wall": ["seq", "session_id", "turn_id", "iter", "kind", "hash", "nth", "final",
                 "episode_idx", "at"],
        "calls": ["call_id", "session_id", "turn_id", "iter", "trigger", "started_at", "model",
                  "tokens_prompt", "tokens_completion", "tokens_cached", "tokens_cache_write",
                  "cost", "cache_expires_at", "system_hash", "actions"],
        "call_blocks": ["call_id", "pos", "hash"],
        "summaries": ["id", "covers_to_seq", "hash", "sources", "model", "at"],
        "slots": ["path", "hash", "owner", "at"],
        "state": ["key", "value"],
        "marks": ["seq", "session_id", "turn_id", "kind", "value", "at"],
        "pins": ["hash", "source", "until", "at"],
    });
    // GH #892: `marks` and `pins` joined the seven of 1.0.0.
    assert_eq!(schema.as_object().unwrap().len(), 9, "nine tables");
    // The llm cell stamps `cost` as a fraction; the store knows `int`, `text`
    // and `json`, and only `json` says what the column holds (review M4).
    assert_eq!(schema["calls"]["cost"], "json", "calls.cost");
    for (table, cols) in expect.as_object().unwrap() {
        let mut want: Vec<&str> = cols
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap())
            .collect();
        let mut have: Vec<&str> = schema[table]
            .as_object()
            .unwrap_or_else(|| panic!("{table}"))
            .keys()
            .map(String::as_str)
            .collect();
        want.sort();
        have.sort();
        assert_eq!(have, want, "columns of {table}");
    }
    // The pointers `state` is born with.
    let seed = std::fs::read_to_string(repo("templates/curator/ledger/seed/state.jsonl")).unwrap();
    for key in [
        "last_call",
        "armed_call",
        "system_hash_sent",
        "rebuild_running",
        "actions_pending",
    ] {
        assert!(
            seed.contains(&format!("\"key\": \"{key}\"")),
            "state seed lacks {key}"
        );
    }
    // OR-KX-G8 / G11: the summarizer declares every hop key the llm cell
    // writes, states a need and is born on the context's model.
    let summ = cell_config("summarizer");
    assert_eq!(summ["params"]["model"], "${ctx.model}");
    assert!(
        !summ["params"]["requirement"]
            .as_str()
            .unwrap_or("")
            .is_empty()
    );
    let src = std::fs::read_to_string(repo("crates/meclaw-cells/src/llm/output.rs")).unwrap();
    let live = src.split("#[cfg(test)]").next().unwrap();
    let mut keys = vec![
        "refused_subscriber".to_string(),
        "refused_model".to_string(),
    ];
    for chunk in live.split("header.insert(").skip(1) {
        let lit = chunk.split('"').nth(1).expect("a key literal");
        keys.push(lit.to_string());
    }
    for k in &keys {
        assert!(
            summ["contract"]["emits"]["hop"][k].is_object(),
            "the summarizer does not declare hop.{k}, which the llm cell writes"
        );
    }
}

/// The fence rule and the hash rule are one text in every script that uses
/// them: two readings of where a block stands would be two windows.
#[test]
fn the_shared_helpers_are_one_text() {
    if !shipped() {
        return;
    }
    let def = |script: &str, name: &str| -> String {
        let at = script
            .find(&format!("\ndef {name}("))
            .unwrap_or_else(|| panic!("no {name}"));
        let rest = &script[at + 1..];
        let end = rest[1..].find("\ndef ").map_or(rest.len(), |e| e + 1);
        rest[..end].trim_end().to_string()
    };
    let (i, p, w) = (
        script_of("intake"),
        script_of("policy"),
        script_of("writer"),
    );
    for name in ["block_span", "stripped_text"] {
        assert_eq!(def(&i, name), def(&p, name), "{name}: intake vs policy");
        assert_eq!(def(&i, name), def(&w, name), "{name}: intake vs writer");
    }
    for name in ["canonical", "block_hash"] {
        assert_eq!(def(&i, name), def(&p, name), "{name}: intake vs policy");
    }
}

// =========================================================== 2. the intake

#[test]
fn curate_writes_each_block_once() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    // Two identical user texts in one round: one hash, two copies (nth 0, 1).
    let round = json!([
        user("ok"),
        tool_call("c1", "look"),
        tool_result("c1", "seen"),
        user("ok")
    ]);
    for _ in 0..3 {
        h.curate("s1", "t1", 0, round.clone(), mode("Be brief."));
    }
    let wall = h.rows("SELECT kind, nth FROM wall ORDER BY seq");
    assert_eq!(
        wall,
        vec![
            vec![json!("user"), json!(0)],
            vec![json!("tool_call"), json!(0)],
            vec![json!("tool_result"), json!(0)],
            vec![json!("user"), json!(1)]
        ],
        "the same round three times is its rows once"
    );
    let dup = h.rows("SELECT hash, COUNT(*) FROM blocks GROUP BY hash HAVING COUNT(*) > 1");
    assert!(dup.is_empty(), "a block is stored once per hash: {dup:?}");
    // The next iteration only appends what is new.
    let round2 = json!([
        user("ok"),
        tool_call("c1", "look"),
        tool_result("c1", "seen"),
        user("ok"),
        tool_call("c2", "look"),
        tool_result("c2", "seen")
    ]);
    h.curate("s1", "t1", 1, round2, mode("Be brief."));
    assert_eq!(h.rows("SELECT COUNT(*) FROM wall")[0][0], json!(6));
}

#[test]
fn hash_is_canonical() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let a = json!({"origin": "user", "type": "text", "text": "hi \u{e4}"});
    let b: Value = sj::from_str(r#"{"text": "hi ä", "type": "text", "origin": "user"}"#).unwrap();
    h.curate("s1", "t1", 0, json!([a]), Value::Null);
    h.curate("s1", "t2", 0, json!([b]), Value::Null);
    let blocks = h.rows("SELECT hash, body FROM blocks WHERE kind = 'user'");
    assert_eq!(
        blocks.len(),
        1,
        "key order does not make a second block: {blocks:?}"
    );
    let canonical = "{\"origin\":\"user\",\"text\":\"hi \u{e4}\",\"type\":\"text\"}";
    assert_eq!(
        blocks[0][1],
        json!(canonical),
        "keys sorted, UTF-8, no whitespace"
    );
    assert_eq!(
        blocks[0][0],
        json!(sha256_hex(canonical)),
        "sha256 of the canonical JSON"
    );
}

#[test]
fn assistant_final_only_from_the_tap() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    // Prose beside a tool call arrives with the round: an assistant row, not final.
    let call = h.curate(
        "s1",
        "t1",
        1,
        json!([
            user("go"),
            said("let me look"),
            tool_call("c1", "look"),
            tool_result("c1", "x")
        ]),
        Value::Null,
    );
    h.tap(
        &call,
        "tool_calls",
        json!({}),
        json!([tool_call("c9", "look")]),
    );
    assert_eq!(
        h.rows("SELECT final FROM wall WHERE kind = 'assistant'"),
        vec![vec![json!(0)]],
        "a tool-call output of the tap writes no row; round prose is never final"
    );
    let call = h.curate(
        "s1",
        "t1",
        2,
        json!([
            user("go"),
            said("let me look"),
            tool_call("c1", "look"),
            tool_result("c1", "x")
        ]),
        Value::Null,
    );
    h.tap(&call, "stop", json!({}), json!([said("done")]));
    assert_eq!(
        h.rows("SELECT final, episode_idx FROM wall WHERE kind = 'assistant' ORDER BY seq"),
        vec![vec![json!(0), Value::Null], vec![json!(1), json!(1)]],
        "the final answer enters from the tap, as an episode"
    );
}

#[test]
fn two_equal_results_under_two_ids_are_two_blocks() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let call = h.curate(
        "s1",
        "t1",
        1,
        json!([
            user("twice"),
            tool_call("a", "look"),
            tool_call("b", "look"),
            tool_result("a", "same"),
            tool_result("b", "same")
        ]),
        Value::Null,
    );
    assert_eq!(
        h.rows("SELECT COUNT(*) FROM wall WHERE kind = 'tool_result'")[0][0],
        json!(2)
    );
    let ids: Vec<Value> = call
        .messages()
        .iter()
        .filter(|m| m["type"] == "tool_result")
        .map(|m| m["id"].clone())
        .collect();
    assert_eq!(
        ids,
        vec![json!("a"), json!("b")],
        "both results reach the model"
    );
}

/// GH #871 I-1 and the interim of an ended round: the sentence a model says
/// beside a tool call enters the wall off the tap -- not final, no episode --
/// with the contract's nothing-form after it when it carries no block, so the
/// next window shows it with a precedent; the call itself stays the round's.
#[test]
fn the_sentence_beside_a_call_carries_the_nothing_block() {
    if !shipped() {
        return;
    }
    const NB: &str =
        r#"{"memory":{"nothing_new":true,"facts":[],"topic":{"movement":"continue"}}}"#;
    let consult = || tool_call("k1", "consult_cogny");
    let mut h = Hive::with(&[("intake", "nothing_block", json!(NB))]);
    let call = h.curate("s1", "t1", 0, json!([user("why?")]), mode("Be brief."));
    h.out.clear();
    h.tap(
        &call,
        "tool_calls",
        json!({}),
        json!([consult(), said("one moment, i am asking")]),
    );
    assert!(
        h.routed("turn_write").is_empty(),
        "an interim is no episode"
    );
    assert_eq!(
        h.rows("SELECT final, episode_idx FROM wall WHERE kind = 'assistant'"),
        vec![vec![json!(0), Value::Null]]
    );
    let next = h.curate(
        "s1",
        "t2",
        0,
        json!([user("are you still there?")]),
        mode("Be brief."),
    );
    let shown = format!("one moment, i am asking\n\n```sidecar\n{NB}\n```");
    assert_eq!(
        texts(&next),
        vec!["why?", shown.as_str(), "are you still there?"],
        "the interim stands in the next window, with the nothing-form"
    );
    assert!(
        next.messages().iter().all(|m| m["type"] != "tool_call"),
        "the ended round's call is not re-opened"
    );
    // A sentence with a block of its own gets no second one.
    let mut h = Hive::with(&[("intake", "nothing_block", json!(NB))]);
    let call = h.curate("s1", "t1", 0, json!([user("why?")]), mode("Be brief."));
    let own = "asking.\n\n```sidecar\n{\"memory\": {}}\n```";
    h.tap(
        &call,
        "tool_calls",
        json!({}),
        json!([consult(), said(own)]),
    );
    let next = h.curate("s1", "t2", 0, json!([user("x")]), mode("Be brief."));
    assert_eq!(texts(&next), vec!["why?", own, "x"]);
    // Off by default: a model behind a curator need not speak a sidecar.
    let mut h = Hive::new();
    let call = h.curate("s1", "t1", 0, json!([user("why?")]), mode("Be brief."));
    h.tap(
        &call,
        "tool_calls",
        json!({}),
        json!([consult(), said("one moment")]),
    );
    let next = h.curate("s1", "t2", 0, json!([user("x")]), mode("Be brief."));
    assert_eq!(texts(&next), vec!["why?", "one moment", "x"]);
}

/// A duplex turn arrives whole (R-25-9): the caller's words and the model's
/// answer in one round, without a tool call. The answer half never passes the
/// tap -- that model is not behind this hive -- so it is final and an episode
/// off the round; an empty answer half writes nothing. The evidence pairs the
/// collector adds to every round (`memory_recall`, `affinity_brief`) are no
/// tool call of the model's: beside them the answer half stays final.
#[test]
fn a_duplex_pair_is_two_episodes() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    h.curate(
        "s1",
        "call-1#4",
        0,
        json!([user("how is the weather?"), said("sunny, sixteen degrees")]),
        Value::Null,
    );
    let eps: Vec<(Value, Value)> = h
        .routed("turn_write")
        .iter()
        .map(|m| (m.hop["turn_id"].clone(), m.messages()[0].clone()))
        .collect();
    assert_eq!(
        eps,
        vec![
            (json!("s1#0"), user("how is the weather?")),
            (json!("s1#1"), said("sunny, sixteen degrees"))
        ]
    );
    assert_eq!(
        h.rows("SELECT kind, final FROM wall ORDER BY seq"),
        vec![
            vec![json!("user"), json!(0)],
            vec![json!("assistant"), json!(1)]
        ]
    );
    h.curate(
        "s1",
        "call-1#5",
        0,
        json!([user("and tomorrow?"), said("")]),
        Value::Null,
    );
    assert_eq!(h.rows("SELECT COUNT(*) FROM wall")[0][0], json!(3));
    // The shipped surface: talky sets `memory_tier`, so the collector hangs a
    // recall pair (and with a counterpart a brief pair) on every round, a
    // duplex pair included (review I-1).
    h.curate(
        "s1",
        "call-1#6",
        0,
        json!([
            user("and the day after?"),
            said("rain, I am afraid"),
            tool_call("r1", "memory_recall"),
            tool_result("r1", "nothing stored"),
            tool_call("b1", "affinity_brief"),
            tool_result("b1", "a regular caller")
        ]),
        Value::Null,
    );
    let eps: Vec<(Value, Value)> = h
        .routed("turn_write")
        .iter()
        .map(|m| (m.hop["turn_id"].clone(), m.messages()[0].clone()))
        .collect();
    assert_eq!(
        eps,
        vec![
            (json!("s1#3"), user("and the day after?")),
            (json!("s1#4"), said("rain, I am afraid"))
        ],
        "an evidence pair is no tool call: the answer half is an episode"
    );
    assert_eq!(
        h.rows("SELECT kind, final FROM wall WHERE turn_id = 'call-1#6' ORDER BY seq"),
        vec![
            vec![json!("user"), json!(0)],
            vec![json!("assistant"), json!(1)],
            vec![json!("recall"), json!(0)],
            vec![json!("recall"), json!(0)],
            vec![json!("brief"), json!(0)],
            vec![json!("brief"), json!(0)]
        ]
    );
}

/// The collector's own slots (`in_slots`: the menu, the sidecar contract) cause
/// no call; they ride with the next call whose system changed, a `$replace` in
/// them dropping the collector's leaves the new tree no longer names.
#[test]
fn slots_from_the_collector_ride_the_next_call() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    h.lane(
        "in_slots",
        json!({}),
        json!({"menu_count": "2"}),
        json!({"system": {"tools": {"$replace": true, "x": {"text": "{\"a\":1}"},
                                    "y": {"text": "{\"b\":1}"}},
                          "instructions": {"sidecar": {"text": "Fence it."}}}}),
    );
    assert!(h.routed("brain").is_empty(), "slots are no call");
    let call = h.curate("s1", "t1", 0, json!([user("q")]), mode("Be brief."));
    assert_eq!(
        call.body["system"],
        json!({"instructions": {"$replace": true, "mode": {"text": "Be brief."},
                                "sidecar": {"text": "Fence it."}},
               "tools": {"$replace": true, "x": {"text": "{\"a\":1}"},
                         "y": {"text": "{\"b\":1}"}}})
    );
    h.lane(
        "in_slots",
        json!({}),
        json!({}),
        json!({"system": {"tools": {"$replace": true, "x": {"text": "{\"a\":1}"}}}}),
    );
    let call = h.curate("s1", "t2", 0, json!([user("q2")]), mode("Be brief."));
    assert_eq!(
        call.body["system"],
        json!({"tools": {"$replace": true, "x": {"text": "{\"a\":1}"}}}),
        "only the family that moved"
    );
    assert_eq!(
        h.rows("SELECT path FROM slots WHERE path LIKE 'tools.%'"),
        vec![vec![json!("tools.x")]]
    );
}

// ============================================= 3. the window and the record

#[test]
fn window_is_summary_then_wall_then_round() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "keep_recent", json!(1))]);
    turn(&mut h, "s1", "t1", "first", "one", json!({}));
    turn(
        &mut h,
        "s1",
        "t2",
        "second",
        "two",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
    );
    h.fire(&last_add(&h));
    h.answer("The person asked first; the answer was one.", "stop");
    let call = h.curate("s1", "t3", 0, json!([user("third")]), mode("Be brief."));
    let summary = call.body["system"]["history"]["summary"]["text"]
        .as_str()
        .expect("the summary rides as system.history.summary");
    assert!(
        summary.starts_with("[earlier exchange, summarised -- what was said, not what is true;")
    );
    assert!(summary.ends_with("The person asked first; the answer was one."));
    assert_eq!(
        texts(&call),
        vec!["second", "two", "third"],
        "after the summary: the kept wall, then the running round"
    );
}

#[test]
fn system_leaves_only_when_changed() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let c1 = turn(&mut h, "s1", "t1", "a", "b", json!({}));
    assert_eq!(
        c1.body["system"],
        json!({"instructions": {"$replace": true, "mode": {"text": "Be brief."}}})
    );
    let c2 = turn(&mut h, "s1", "t2", "c", "d", json!({}));
    assert!(
        c2.body.get("system").is_none(),
        "nothing changed, nothing sent: {:?}",
        c2.body
    );
    h.lane(
        "in_pack",
        json!({}),
        json!({}),
        json!({"system": {"identity": {"text": "You are T."}}}),
    );
    let c3 = turn(&mut h, "s1", "t3", "e", "f", json!({}));
    assert_eq!(
        c3.body["system"],
        json!({"identity": {"$replace": true, "text": "You are T."}}),
        "only the family that changed, as one $replace root"
    );
}

#[test]
fn collector_slot_wins_at_its_leaf() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    h.curate("s1", "t1", 0, json!([user("a")]), mode("collector says"));
    h.lane(
        "in_pack",
        json!({}),
        json!({}),
        json!({"system": {"instructions": {"mode": {"text": "pack says"},
                                           "reply": {"text": "reply kindly"}}}}),
    );
    let call = h.curate("s1", "t2", 0, json!([user("b")]), mode("collector says"));
    assert_eq!(
        call.body["system"],
        json!({"instructions": {"$replace": true, "mode": {"text": "collector says"},
                                "reply": {"text": "reply kindly"}}}),
        "the pack's leaf beside the collector's, the collector's at its own path"
    );
    assert_eq!(
        h.rows("SELECT owner FROM slots WHERE path = 'instructions.mode'"),
        vec![vec![json!("collector")]]
    );
}

#[test]
fn earlier_answer_keeps_its_block_up_to_the_cap() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "sidecar_max_chars", json!(40))]);
    let small = "short.\n\n```sidecar\n{\"a\": 1}\n```";
    let big = format!(
        "long.\n\n```sidecar\n{{\"a\": \"{}\"}}\n```",
        "x".repeat(60)
    );
    turn(&mut h, "s1", "t1", "q1", small, json!({}));
    turn(&mut h, "s1", "t2", "q2", &big, json!({}));
    let call = h.curate("s1", "t3", 0, json!([user("q3")]), mode("Be brief."));
    assert_eq!(texts(&call), vec!["q1", small, "q2", "long.", "q3"]);
    // What the call record names, the ledger holds -- the bare answer too.
    let missing = h.rows(
        "SELECT cb.hash FROM call_blocks cb LEFT JOIN blocks b ON b.hash = cb.hash \
         WHERE b.hash IS NULL",
    );
    assert!(
        missing.is_empty(),
        "every listed block is in the ledger: {missing:?}"
    );
}

#[test]
fn call_blocks_list_what_left() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    h.lane(
        "in_pack",
        json!({}),
        json!({}),
        json!({"system": {"identity": {"text": "You are T."}}}),
    );
    turn(&mut h, "s1", "t1", "hello", "hi", json!({}));
    let call = h.curate(
        "s1",
        "t2",
        1,
        json!([
            user("look"),
            tool_call("c1", "look"),
            tool_result("c1", "seen")
        ]),
        mode("Be brief."),
    );
    let id = call.hop["curator_call"].as_str().unwrap().to_string();
    let listed = h.rows(&format!(
        "SELECT b.kind, b.body FROM call_blocks cb JOIN blocks b ON b.hash = cb.hash \
         WHERE cb.call_id = '{id}' ORDER BY cb.pos"
    ));
    let (sys, msgs): (Vec<_>, Vec<_>) = listed.iter().partition(|r| r[0] == "system");
    let sys: Vec<Value> = sys
        .iter()
        .map(|r| sj::from_str(r[1].as_str().unwrap()).unwrap())
        .collect();
    assert_eq!(
        sys,
        vec![
            json!({"path": "identity", "text": "You are T."}),
            json!({"path": "instructions.mode", "text": "Be brief."})
        ],
        "every system leaf the model holds, first"
    );
    assert!(
        listed.iter().take(2).all(|r| r[0] == "system"),
        "system blocks come first"
    );
    let msgs: Vec<Value> = msgs
        .iter()
        .map(|r| sj::from_str(r[1].as_str().unwrap()).unwrap())
        .collect();
    assert_eq!(msgs, call.messages(), "then exactly the messages that left");
    let row = h.rows(&format!(
        "SELECT session_id, turn_id, iter, trigger FROM calls WHERE call_id = '{id}'"
    ));
    assert_eq!(
        row,
        vec![vec![json!("s1"), json!("t2"), json!(1), json!("curate")]]
    );
}

/// The loader of `workshop/evals/p5-longmemeval/annotate.py`
/// (`collector_scope`): imports, defs and upper-case constants of the policy
/// script, in file order. Its input is read first; after that stdin is empty,
/// so a definition that read it would fail the load.
const AST_LOADER: &str = r#"
import ast, io, json, sys
inp = json.load(sys.stdin)
sys.stdin = io.StringIO("")
src, params = inp["src"], inp["params"]
keep = [n for n in ast.parse(src).body
        if isinstance(n, (ast.Import, ast.ImportFrom, ast.FunctionDef))
        or (isinstance(n, ast.Assign)
            and all(isinstance(t, ast.Name) and t.id.isupper() for t in n.targets))]
scope = {"P": params, "doc": {"params": params, "body": {}, "envelope": {}}}
for n in keep:
    exec(compile(ast.Module(body=[n], type_ignores=[]), "policy", "exec"), scope)
big = "done.\n\n```sidecar\n" + "y" * 80 + "\n```"
rows = [
    {"seq": 3, "session_id": "s", "turn_id": "t2", "kind": "tool_call", "final": 0,
     "element": {"origin": "assistant", "type": "tool_call", "id": "orphan", "text": "{}"}},
    {"seq": 1, "session_id": "s", "turn_id": "t1", "kind": "user", "final": 0,
     "element": {"origin": "user", "type": "text", "text": "q"}},
    {"seq": 2, "session_id": "s", "turn_id": "t1", "kind": "assistant", "final": 1,
     "body": json.dumps({"origin": "assistant", "type": "text", "text": big})},
]
a = scope["history_window"](rows, {"sidecar_max_chars": 20})
b = scope["history_window"](list(reversed(rows)), {"sidecar_max_chars": 20})
print(json.dumps({"a": a, "same": a == b,
                  "knobs": [scope[k] for k in ("KEEP_RECENT", "COMPRESS_AT",
                                               "SUMMARY_CHARS", "SIDECAR_MAX_CHARS")]}))
"#;

#[test]
fn history_window_is_pure_and_ast_loadable() {
    if !shipped() {
        return;
    }
    let mut params = obj(cell_config("policy")["params"].clone());
    params.remove("script_inline");
    let doc = json!({"src": script_of("policy"), "params": params});
    let out = run_python(AST_LOADER, &doc.to_string());
    assert!(
        out.status.success(),
        "the pure half does not load: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let got: Value = sj::from_slice(&out.stdout).expect("json");
    assert_eq!(
        got["a"],
        json!([{"origin": "user", "type": "text", "text": "q"},
               {"origin": "assistant", "type": "text", "text": "done."}]),
        "wall order, the orphan call out, the over-cap block cut (#871)"
    );
    assert_eq!(got["same"], json!(true), "the input order does not matter");
    assert_eq!(
        got["knobs"],
        json!([12, 0.5, 4000, 6000]),
        "the shipped defaults"
    );
}

// ======================================================= 4. the tap and the clock

#[test]
fn tap_fills_usage_and_expiry() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let call = turn(
        &mut h,
        "s1",
        "t1",
        "q",
        "a",
        json!({"tokens_prompt": 120, "tokens_completion": 7, "tokens_cached": 100,
               "tokens_cache_write": 20, "cost": 0.25,
               "cache_expires_at": "2099-01-01T00:00:00Z"}),
    );
    let id = call.hop["curator_call"].as_str().unwrap().to_string();
    assert_eq!(
        h.rows(&format!(
            "SELECT model, tokens_prompt, tokens_completion, tokens_cached, tokens_cache_write, \
             cost, cache_expires_at FROM calls WHERE call_id = '{id}'"
        )),
        vec![vec![
            json!("test-model"),
            json!(120),
            json!(7),
            json!(100),
            json!(20),
            json!("0.25"),
            json!("2099-01-01T00:00:00Z")
        ]]
    );
    assert_eq!(h.state("last_call"), id);
}

/// The clock stands on the newest call, under ONE id (GH #904, PP-7): every
/// tap re-arms the same order (`add` with `rearm`) instead of removing the
/// last call's order and adding one under a fresh id -- that grew the timer's
/// `schedules` by a row per call.
#[test]
fn tap_arms_the_clock_under_one_id() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    turn(
        &mut h,
        "s1",
        "t1",
        "q1",
        "a1",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
    );
    let first = clock_orders(&h);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].0, "add");
    turn(
        &mut h,
        "s1",
        "t2",
        "q2",
        "a2",
        json!({"cache_expires_at": "2099-01-01T00:05:00Z"}),
    );
    let all = clock_orders(&h);
    assert_eq!(
        all,
        vec![first[0].clone(), first[0].clone()],
        "two adds under one id, no remove"
    );
    assert!(
        h.clock.iter().all(|m| m.body["rearm"] == json!(true)),
        "every order re-arms"
    );
    let add = last_add(&h);
    assert_eq!(add.body["at"], "2099-01-01T00:05:00Z");
    assert_eq!(add.body["schedule_name"], "cache");
    assert_eq!(
        add.body["emit_body"]["curator_call"],
        json!(h.state("last_call"))
    );
}

/// An order that fired is done at the timer (`completed`): the next tap
/// orders no `remove` for it -- that answered `schedule_not_found` (review
/// M-2) -- but re-arms the same id (GH #904). A stale strike leaves the newer
/// order armed.
#[test]
fn a_fired_order_is_not_removed() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    turn(
        &mut h,
        "s1",
        "t1",
        "q1",
        "a1",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
    );
    let first = last_add(&h);
    h.fire(&first);
    assert_eq!(h.state("armed_call"), "", "the fired order is disarmed");
    turn(
        &mut h,
        "s1",
        "t2",
        "q2",
        "a2",
        json!({"cache_expires_at": "2099-01-01T00:05:00Z"}),
    );
    let orders = clock_orders(&h);
    assert_eq!(
        orders.iter().map(|(op, _)| op.as_str()).collect::<Vec<_>>(),
        vec!["add", "add"],
        "no remove for an order that already fired: {orders:?}"
    );
    assert_eq!(orders[0].1, orders[1].1, "re-armed under the same id");
    let second = last_add(&h);
    assert_eq!(second.body["rearm"], json!(true));
    let armed = h.state("armed_call");
    assert_eq!(json!(armed), second.body["emit_body"]["curator_call"]);
    // A strike for the first call now (the race: it fired while the second tap
    // armed) must not disarm the second order.
    h.fire(&first);
    assert_eq!(h.state("armed_call"), armed);
}

#[test]
fn a_stale_fire_is_dropped() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "keep_recent", json!(0))]);
    turn(
        &mut h,
        "s1",
        "t1",
        "q1",
        "a1",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
    );
    let stale = last_add(&h);
    turn(
        &mut h,
        "s1",
        "t2",
        "q2",
        "a2",
        json!({"cache_expires_at": "2099-01-01T00:05:00Z"}),
    );
    h.fire(&stale);
    assert!(
        h.summ.is_empty(),
        "a strike for a call that is not the newest is dropped"
    );
    assert!(
        h.stderr.iter().any(|l| l.contains("stale")),
        "{:?}",
        h.stderr
    );
}

#[test]
fn at_in_past_rebuilds_now() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "keep_recent", json!(1))]);
    turn(&mut h, "s1", "t1", "q1", "a1", json!({}));
    let before = chrono::Utc::now();
    turn(
        &mut h,
        "s1",
        "t2",
        "q2",
        "a2",
        json!({"cache_expires_at": "2000-01-01T00:00:00Z"}),
    );
    let after = chrono::Utc::now();
    let add = last_add(&h);
    let at = add.body["at"].as_str().unwrap().to_string();
    let t = chrono::DateTime::parse_from_rfc3339(&at)
        .expect("rfc3339")
        .with_timezone(&chrono::Utc);
    // The earliest second the timer accepts: at least a second after the tap,
    // at most that second rounded up.
    assert!(
        t >= before + chrono::Duration::seconds(1) && t <= after + chrono::Duration::seconds(2),
        "the earliest second: {at} (tap between {before} and {after})"
    );
    assert_eq!(add.body["emit_body"]["reason"], "cache");
    h.fire(&add);
    assert_eq!(h.summ.len(), 1, "a cold cache is rebuilt");
}

#[test]
fn compress_at_crossed_rebuilds() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[
        ("policy", "keep_recent", json!(1)),
        ("policy", "context_window", json!(1000)),
    ]);
    turn(
        &mut h,
        "s1",
        "t1",
        "q1",
        "a1",
        json!({"tokens_prompt": 400}),
    );
    assert!(
        h.clock.is_empty(),
        "under the line, no stamp: nothing ordered"
    );
    turn(
        &mut h,
        "s1",
        "t2",
        "q2",
        "a2",
        json!({"tokens_prompt": 500}),
    );
    let add = last_add(&h);
    assert_eq!(add.body["emit_body"]["reason"], "compress");
    h.fire(&add);
    assert_eq!(h.summ.len(), 1);
    // The window the model reports wins over the knob.
    let mut h = Hive::with(&[("policy", "context_window", json!(1000))]);
    turn(
        &mut h,
        "s1",
        "t1",
        "q1",
        "a1",
        json!({"tokens_prompt": 600, "context_window": 100000}),
    );
    assert!(h.clock.is_empty(), "600 of 100000 is under compress_at");
}

// ============================================================ 5. the rebuild

#[test]
fn rebuild_summarises_between_cover_and_keep_recent() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "keep_recent", json!(1))]);
    turn(&mut h, "s1", "t1", "q1", "a1", json!({}));
    turn(&mut h, "s1", "t2", "q2", "a2", json!({}));
    turn(
        &mut h,
        "s1",
        "t3",
        "q3",
        "a3",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
    );
    h.fire(&last_add(&h));
    let req = h.answer("S1: q1 a1 q2 a2.", "stop");
    let transcript = req.messages()[0]["text"].as_str().unwrap().to_string();
    assert_eq!(
        transcript,
        "user: q1\nassistant: a1\nuser: q2\nassistant: a2"
    );
    let cover = h.rows("SELECT MAX(seq) FROM wall WHERE turn_id = 't2'")[0][0].clone();
    assert_eq!(h.rows("SELECT covers_to_seq FROM summaries")[0][0], cover);
    // The next rebuild starts from the summary and moves the cover on.
    turn(
        &mut h,
        "s1",
        "t4",
        "q4",
        "a4",
        json!({"cache_expires_at": "2099-01-01T00:01:00Z"}),
    );
    h.fire(&last_add(&h));
    let req = h.answer("S2.", "stop");
    let transcript = req.messages()[0]["text"].as_str().unwrap().to_string();
    assert_eq!(
        transcript,
        "Summary of the exchange before this part:\nS1: q1 a1 q2 a2.\n\nuser: q3\nassistant: a3"
    );
    // R-KX-2: the text the summarizer's model reads assumes no conversation
    // and no assistant -- any `llm` cell may sit behind the hive.
    let prompt = req.body["system"]["instructions"]["text"]
        .as_str()
        .expect("the summarizer's instructions")
        .to_lowercase();
    for word in ["conversation", "assistant"] {
        assert!(!prompt.contains(word), "{word:?} in {prompt:?}");
    }
    let cover = h.rows("SELECT MAX(seq) FROM wall WHERE turn_id = 't3'")[0][0].clone();
    assert_eq!(
        h.rows("SELECT covers_to_seq FROM summaries ORDER BY covers_to_seq DESC LIMIT 1")[0][0],
        cover
    );
    let call = h.curate("s1", "t5", 0, json!([user("q5")]), mode("Be brief."));
    assert_eq!(texts(&call), vec!["q4", "a4", "q5"]);
    let actions: Value = sj::from_str(
        h.rows(&format!(
            "SELECT actions FROM calls WHERE call_id = '{}'",
            call.hop["curator_call"].as_str().unwrap()
        ))[0][0]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let a: Vec<String> = actions
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_string())
        .collect();
    assert!(a.contains(&"rebuild:cache".to_string()), "{a:?}");
    assert!(a.iter().any(|x| x.starts_with("summary:")), "{a:?}");
}

#[test]
fn summary_block_names_its_sources() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "keep_recent", json!(1))]);
    turn(&mut h, "s1", "t1", "q1", "a1", json!({}));
    turn(
        &mut h,
        "s1",
        "t2",
        "q2",
        "a2",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
    );
    h.fire(&last_add(&h));
    h.answer("What was said.", "stop");
    let row = h.rows("SELECT hash, sources, model FROM summaries");
    assert_eq!(row.len(), 1);
    let sources: Value = sj::from_str(row[0][1].as_str().unwrap()).unwrap();
    let want: Vec<Value> = h
        .rows("SELECT hash FROM wall WHERE turn_id = 't1' ORDER BY seq")
        .into_iter()
        .map(|r| r[0].clone())
        .collect();
    assert_eq!(
        sources,
        Value::Array(want),
        "the hashes it covers, in wall order"
    );
    assert_eq!(row[0][2], json!("summary-model"));
    let block = h.rows(&format!(
        "SELECT kind, body FROM blocks WHERE hash = '{}'",
        row[0][0].as_str().unwrap()
    ));
    assert_eq!(block[0][0], json!("summary"));
    assert_eq!(
        sj::from_str::<Value>(block[0][1].as_str().unwrap()).unwrap(),
        json!({"type": "summary", "text": "What was said."})
    );
    assert_eq!(
        h.rows("SELECT owner FROM slots WHERE path = 'history.summary'"),
        vec![vec![json!("curator")]]
    );
}

#[test]
fn a_second_trigger_during_a_rebuild_merges() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "keep_recent", json!(1))]);
    turn(&mut h, "s1", "t1", "q1", "a1", json!({}));
    turn(
        &mut h,
        "s1",
        "t2",
        "q2",
        "a2",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
    );
    let order = last_add(&h);
    h.fire(&order);
    // A pack while the rebuild runs (review focus K-2) changes nothing of it.
    h.lane(
        "in_pack",
        json!({}),
        json!({}),
        json!({"system": {"identity": {"text": "I."}}}),
    );
    h.fire(&order);
    assert_eq!(
        h.summ.len(),
        1,
        "one rebuild at a time: the second trigger merges"
    );
    h.answer("Merged.", "stop");
    assert_eq!(h.rows("SELECT COUNT(*) FROM summaries")[0][0], json!(1));
    assert_eq!(h.state("rebuild_running"), "", "the lock is released");
    assert_eq!(
        h.rows("SELECT path FROM slots ORDER BY path"),
        vec![
            vec![json!("history.summary")],
            vec![json!("identity")],
            vec![json!("instructions.mode")]
        ]
    );
}

#[test]
fn a_failed_summary_keeps_the_old_window() {
    if !shipped() {
        return;
    }
    for (text, finish) in [("", "error"), ("cut", "length"), ("   ", "stop")] {
        let mut h = Hive::with(&[("policy", "keep_recent", json!(1))]);
        turn(&mut h, "s1", "t1", "q1", "a1", json!({}));
        turn(
            &mut h,
            "s1",
            "t2",
            "q2",
            "a2",
            json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
        );
        h.fire(&last_add(&h));
        h.answer(text, finish);
        assert_eq!(
            h.rows("SELECT COUNT(*) FROM summaries")[0][0],
            json!(0),
            "{finish}"
        );
        assert_eq!(h.state("rebuild_running"), "");
        let call = h.curate("s1", "t3", 0, json!([user("q3")]), mode("Be brief."));
        assert_eq!(texts(&call), vec!["q1", "a1", "q2", "a2", "q3"], "{finish}");
        let actions = h.rows(&format!(
            "SELECT actions FROM calls WHERE call_id = '{}'",
            call.hop["curator_call"].as_str().unwrap()
        ));
        assert!(
            actions[0][0].as_str().unwrap().contains("rebuild_failed"),
            "{actions:?}"
        );
        assert!(
            h.stderr.iter().any(|l| l.contains("old window kept")),
            "{:?}",
            h.stderr
        );
    }
    // Longer than summary_chars is refused hard (review focus K-5).
    let mut h = Hive::with(&[
        ("policy", "keep_recent", json!(1)),
        ("policy", "summary_chars", json!(10)),
    ]);
    turn(&mut h, "s1", "t1", "q1", "a1", json!({}));
    turn(
        &mut h,
        "s1",
        "t2",
        "q2",
        "a2",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
    );
    h.fire(&last_add(&h));
    let req = h.answer("far more than ten characters", "stop");
    assert!(
        req.body["system"]["instructions"]["text"]
            .as_str()
            .unwrap()
            .contains("at most 10 characters")
    );
    assert_eq!(h.rows("SELECT COUNT(*) FROM summaries")[0][0], json!(0));
}

// ============================================================= 6. the writer

#[test]
fn each_participant_turn_writes_one_episode() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let mut episodes = Vec::new();
    h.curate("s1", "t1", 0, json!([user("hello")]), Value::Null);
    episodes.extend(h.routed("turn_write"));
    let call2 = h.curate(
        "s1",
        "t1",
        1,
        json!([
            user("hello"),
            tool_call("c1", "look"),
            tool_result("c1", "x")
        ]),
        Value::Null,
    );
    episodes.extend(h.routed("turn_write"));
    h.out.clear();
    h.tap(
        &call2,
        "stop",
        json!({}),
        json!([said("hi\n\n```sidecar\n{\"memory\": {}}\n```")]),
    );
    episodes.extend(h.routed("turn_write"));
    let advice = "[advice from your reasoning core, consult k1]\nthink twice";
    h.curate("s1", "k1", 0, json!([user(advice)]), Value::Null);
    episodes.extend(h.routed("turn_write"));
    let peer = json!({"origin": "peer", "type": "text", "text": "from afar",
                      "speaker": "North", "speaker_ref": "dc365e79"});
    h.curate("s1", "t2", 0, json!([peer]), Value::Null);
    episodes.extend(h.routed("turn_write"));
    let got: Vec<(Value, Value)> = episodes
        .iter()
        .map(|m| (m.hop["turn_id"].clone(), m.messages()[0].clone()))
        .collect();
    assert_eq!(
        got,
        vec![
            (
                json!("s1#0"),
                json!({"origin": "user", "type": "text", "text": "hello"})
            ),
            (
                json!("s1#1"),
                json!({"origin": "assistant", "type": "text", "text": "hi"})
            ),
            (
                json!("s1#2"),
                json!({"origin": "peer", "type": "text", "text": "from afar",
                                   "speaker": "North", "speaker_ref": "dc365e79"})
            ),
        ],
        "the person, the model's answer without its block, the peer -- once each"
    );
    let keys: Vec<&str> = episodes[0].hop.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "happened_at",
            "iter",
            "phase",
            "route",
            "session_id",
            "turn_id",
            "turn_index"
        ],
        "the collector's per-turn contract"
    );
    assert!(
        episodes
            .iter()
            .all(|m| !m.context.contains_key("cur_origin")),
        "no marker leaves"
    );
}

#[test]
fn turn_write_off_writes_nothing() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("writer", "turn_write", json!("0"))]);
    let call = h.curate("s1", "t1", 0, json!([user("hello")]), Value::Null);
    assert!(h.routed("turn_write").is_empty());
    h.tap(&call, "stop", json!({}), json!([said("hi")]));
    assert!(h.routed("turn_write").is_empty());
    assert_eq!(
        h.rows("SELECT COUNT(*) FROM wall")[0][0],
        json!(2),
        "the wall is written anyway"
    );
}

#[test]
fn close_batches_the_session_in_the_collector_contract() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let call = h.curate(
        "s1",
        "t1",
        1,
        json!([user("go"), tool_call("c1", "look"), tool_result("c1", "x")]),
        Value::Null,
    );
    h.tap(
        &call,
        "stop",
        json!({}),
        json!([said("done\n\n```sidecar\n{}\n```")]),
    );
    h.curate("s2", "t9", 0, json!([user("elsewhere")]), Value::Null);
    h.out.clear();
    h.lane(
        "in_close",
        json!({"session_id": "s1"}),
        json!({}),
        json!({"messages": []}),
    );
    let w = h.routed("write");
    assert_eq!(w.len(), 1);
    let keys: Vec<&str> = w[0].hop.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "iter",
            "phase",
            "round_count",
            "route",
            "session_id",
            "turn_count",
            "turn_id"
        ],
        "the collector's close header"
    );
    let mut body: Vec<&str> = w[0].body.keys().map(String::as_str).collect();
    body.sort();
    assert_eq!(body, ["messages", "rounds"]);
    assert_eq!(w[0].hop["turn_id"], "close-s1");
    assert_eq!(w[0].hop["turn_count"], "2");
    assert_eq!(w[0].hop["round_count"], "2");
    assert_eq!(
        w[0].messages(),
        vec![user("go"), said("done")],
        "participant turns in order, the answer without its block"
    );
    // A session that left nothing closes empty and fails nothing (review focus K-4).
    h.out.clear();
    h.lane(
        "in_close",
        json!({"session_id": "nobody"}),
        json!({}),
        json!({"messages": []}),
    );
    let w = h.routed("write");
    assert_eq!(w.len(), 1);
    assert_eq!(w[0].hop["turn_count"], "0");
    assert_eq!(w[0].messages(), Vec::<Value>::new());
}

// =============================================================== 7. the pack

#[test]
fn a_valid_pack_is_acked_empty_and_reaches_the_next_call() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    turn(&mut h, "s1", "t1", "q", "a", json!({}));
    h.out.clear();
    h.lane_from(
        "in_pack",
        json!({}),
        json!({}),
        json!({"system": {"identity": {"text": "You are T."},
                          "persona": {"voice": {"text": "calm"}}}}),
        "/x/affinity/push",
    );
    let ack = h.routed("pack_ack");
    assert_eq!(ack.len(), 1);
    assert_eq!(ack[0].hop["error_code"], "");
    assert_eq!(ack[0].hop["pack_owner"], "/x/affinity/push");
    assert_eq!(ack[0].hop["pack_slots"], "identity,persona");
    assert_eq!(ack[0].hop["pack_unknown"], "");
    let call = h.curate("s1", "t2", 0, json!([user("q2")]), mode("Be brief."));
    assert_eq!(
        call.body["system"],
        json!({"identity": {"$replace": true, "text": "You are T."},
               "persona": {"$replace": true, "voice": {"text": "calm"}}})
    );
}

#[test]
fn an_unknown_family_is_slot_unknown() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    h.lane(
        "in_pack",
        json!({}),
        json!({}),
        json!({"system": {"identity": {"text": "I."}, "tools": {"x": {"text": "{}"}}}}),
    );
    let ack = h.routed("pack_ack");
    assert_eq!(ack[0].hop["error_code"], "slot_unknown");
    assert_eq!(ack[0].hop["pack_unknown"], "tools");
    h.out.clear();
    h.lane("in_pack", json!({}), json!({}), json!({"system": {}}));
    assert_eq!(h.routed("pack_ack")[0].hop["error_code"], "pack_empty");
    assert_eq!(
        h.rows("SELECT COUNT(*) FROM slots")[0][0],
        json!(0),
        "all or nothing"
    );
}

#[test]
fn pack_ack_carries_the_context_through() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    h.lane(
        "in_pack",
        json!({"pack_sub": "sub:/x/talky|entity:t", "pack_hash": "abc123"}),
        json!({}),
        json!({"system": {"identity": {"text": "I."}}}),
    );
    let ack = h.routed("pack_ack");
    assert_eq!(ack[0].context["pack_sub"], "sub:/x/talky|entity:t");
    assert_eq!(ack[0].context["pack_hash"], "abc123");
    for k in ["cur_origin", "cur_phase", "cur_call", "cur_reason"] {
        assert!(!ack[0].context.contains_key(k), "{k} leaves the hive");
    }
}

// ================================================= R-KX-2: any llm cell, any caller

/// The contract assumes no conversation: a caller that names no session and no
/// turn -- a summarizer, a judge, any `llm` cell -- still gets its call, as one
/// session (`default`) and one round of its own.
#[test]
fn a_round_without_ids_is_a_round_of_its_own() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    h.lane(
        "in_curate",
        json!({}),
        json!({}),
        json!({"messages": [user("condense this")]}),
    );
    let calls = h.routed("brain");
    assert_eq!(calls.len(), 1, "{:?}", h.out);
    let hop = &calls[0].hop;
    assert_eq!(hop["session_id"], "default");
    assert_eq!(
        hop["turn_id"], hop["curator_call"],
        "the round is keyed by its call"
    );
    assert_eq!(texts(&calls[0]), vec!["condense this"]);
    assert_eq!(
        h.rows("SELECT session_id, kind FROM wall"),
        vec![vec![json!("default"), json!("user")]]
    );
    // The answer comes back with nothing but `context.curator_call` -- the
    // caller's edge promoted no session and no turn. Same rule as on the way
    // out: session `default`, the round keyed by its call (review M2).
    let call = hop["curator_call"].as_str().unwrap().to_string();
    h.lane(
        "in_llm",
        json!({"curator_call": call}),
        json!({"finish_reason": "stop", "model": "test-model"}),
        json!({"messages": [said("condensed")]}),
    );
    assert_eq!(
        h.rows("SELECT session_id, turn_id, kind, final FROM wall ORDER BY seq"),
        vec![
            vec![json!("default"), json!(call), json!("user"), json!(0)],
            vec![json!("default"), json!(call), json!("assistant"), json!(1)],
        ]
    );
    assert_eq!(h.state("last_call"), call);
}

// ========================================================== review focus K-3

#[test]
fn a_tap_without_a_call_is_dropped_and_said() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    h.lane(
        "in_llm",
        json!({"session_id": "s1", "turn_id": "t1"}),
        json!({"finish_reason": "stop"}),
        json!({"messages": [said("from somebody else")]}),
    );
    assert_eq!(h.rows("SELECT COUNT(*) FROM wall")[0][0], json!(0));
    assert!(h.out.is_empty());
    assert!(
        h.stderr.iter().any(|l| l.contains("curator_call")),
        "{:?}",
        h.stderr
    );
}

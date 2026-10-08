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

// The round key and the summary slot of a round (GH #943) are the shared
// harness's; only those two are used, qualified, beside this file's own hive.
#[path = "support/curator_hive.rs"]
mod curator_hive;

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
        // GH #915/#943: the named indexes between schema and seed, as the
        // store builds them (the unique `state.key` makes a round's first row
        // a compare-and-set).
        let ledger = meclaw_cells::store::StoreParams::parse(&cells["ledger"]["params"])
            .expect("the ledger params parse");
        meclaw_cells::store::ddl::apply_index_ddl(&db, &ledger.indexes).expect("index ddl");
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
        // GH #925: every lane message of a colony carries the audience of its
        // round. A test that names none gets the standard round; one that sets
        // the key to `null` keeps it null -- a round that declares nothing.
        let mut context = obj(context);
        context
            .entry("audience_set")
            .or_insert_with(|| json!(ROUND_E));
        let msg = Msg {
            context,
            hop,
            body: obj(body),
            reply_to: reply_to.to_string(),
        };
        self.pump(".", msg);
    }

    /// The strike of an order the clock holds, as the timer emits it: a
    /// fresh root without a context, the row's `emit_headers` as hop keys and
    /// the timer's own headers over them (`timer::cell::build_fire_content`).
    /// The row under one id is the order armed LAST (`rearm` replaces it), so
    /// its `emit_headers` ride -- the round of the newest call (GH #925,
    /// OR-BD.A.6); an order the clock never saw brings its own.
    fn fire(&mut self, order: &Msg) {
        let id = order.body.get("schedule_id");
        let armed = self
            .clock
            .iter()
            .rev()
            .find(|m| m.body.get("op") == Some(&json!("add")) && m.body.get("schedule_id") == id)
            .unwrap_or(order);
        let mut hop = obj(armed.body.get("emit_headers").cloned().unwrap_or_default());
        for (k, v) in obj(json!({
            "event_id": "0190a3f2-0000-7000-8000-000000000888",
            "schedule_id": order.body["schedule_id"],
            "schedule_name": order.body["schedule_name"],
            "scheduled_at": order.body["at"], "fired_at": order.body["at"]}))
        {
            hop.insert(k, v);
        }
        let msg = Msg {
            context: Map::new(),
            hop,
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

    /// The state row `key` of the round `round` (GH #943).
    fn state_in(&self, key: &str, round: &str) -> String {
        self.state(&format!("{key}:{}", curator_hive::round_key(round)))
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
        self.curate_as(json!(ROUND_E), session, turn, iter, round, system)
    }

    /// The same round under an audience of its own (GH #925): the context
    /// value as the colony carries it, a JSON array in a TEXT.
    fn curate_as(
        &mut self,
        audience: Value,
        session: &str,
        turn: &str,
        iter: u32,
        round: Value,
        system: Value,
    ) -> Msg {
        self.out.clear();
        let hop = json!({"session_id": session, "turn_id": turn, "iter": iter.to_string(),
                         "phase": ""});
        let ctx = json!({"session_id": session, "turn_id": turn, "iter": iter.to_string(),
                         "channel": "test", "audience_set": audience});
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

/// The standard round of this file (GH #925): the audience a lane message
/// carries when a test names none, as the TEXT the colony carries it in.
const ROUND_E: &str = "[\"member:e\"]";

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

/// `system` with the collector's menu beside it (`system.tools`, one leaf per
/// tool under `$replace`, as collector `tools_slot` writes it): a `tool_error`
/// names only a tool the menu holds (OR-BD-92).
fn with_menu(mut system: Value, names: &[&str]) -> Value {
    let mut tools = json!({"$replace": true});
    for n in names {
        tools[*n] = json!({"text": json!({"name": n}).to_string()});
    }
    system["tools"] = tools;
    system
}

fn sha256_hex(s: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `hop.turn_id` of an episode as `./writer` stamps it (GH #932,
/// OR-S3.K.1): `<session>#<tag>-<index>`, `tag` the first 8 hex of sha256
/// over the canonical round, the index counted per (session, round) from 0.
/// WHY: the old `<session>#<index>` counted over every round of a session,
/// so its gaps told a round how many turns it did not see.
fn episode_turn_id(session: &str, round: &str, index: u32) -> String {
    format!("{session}#{}-{index}", &sha256_hex(round)[..8])
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
    assert_eq!(t["version"], "1.11.3");
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
    // the two routes that answer them; GH #926 the stats question and its
    // answer; GH #949 the candidate door, the ledger read for an app and the
    // three routes that answer them.
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
            "in_renewed",
            "in_stats",
            "in_candidate",
            "in_read",
            // GH #1061 (#801): the summarizer's sealed key, answered by the
            // member's broker over a v-lane.
            "in_sealed"
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
            "recall",
            "stats",
            "candidate_ack",
            "thing_seen",
            "read",
            "credential_request",
            // GH #1079: one per item of a section that failed its word-for-word check.
            "reject"
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
        ("stats", "code"),
        ("reader", "code"),
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
    // GH #925: every row a round causes carries its audience (`audience_set`).
    let expect = json!({
        "blocks": ["hash", "kind", "chars", "body", "first_seen"],
        "wall": ["seq", "session_id", "turn_id", "iter", "kind", "hash", "nth", "final",
                 "episode_idx", "at", "audience_set"],
        "calls": ["call_id", "session_id", "turn_id", "iter", "trigger", "started_at", "model",
                  "tokens_prompt", "tokens_completion", "tokens_cached", "tokens_cache_write",
                  "cost", "cache_expires_at", "system_hash", "actions", "audience_set"],
        "call_blocks": ["call_id", "pos", "hash"],
        "summaries": ["id", "covers_to_seq", "hash", "sources", "model", "at", "audience_set"],
        "slots": ["path", "hash", "owner", "at"],
        "state": ["key", "value"],
        "marks": ["seq", "session_id", "turn_id", "kind", "value", "at", "audience_set"],
        "pins": ["hash", "source", "until", "at", "audience_set"],
        "candidates": ["source", "cand_id", "hash", "triggers", "until", "once", "used_at",
                       "last_seq", "priority", "audience_set", "at"],
    });
    // GH #892: `marks` and `pins` joined the seven of 1.0.0; GH #949
    // `candidates`.
    assert_eq!(schema.as_object().unwrap().len(), 10, "ten tables");
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
    // The pointers `state` is born with. Since GH #943 the round-scoped
    // pointers are `<key>:<round_key>` rows the policy inserts on first use,
    // so the seed carries only the two an update leg needs (GH #949, C1
    // finding W M-7) and never a bare pre-#943 key.
    let seed = std::fs::read_to_string(repo("templates/curator/ledger/seed/state.jsonl")).unwrap();
    for key in ["system_hash_sent", "context_window"] {
        assert!(
            seed.contains(&format!("\"key\": \"{key}\"")),
            "state seed lacks {key}"
        );
    }
    for key in [
        "last_call",
        "armed_call",
        "rebuild_running",
        "actions_pending",
        "window_plan",
    ] {
        assert!(
            !seed.contains(&format!("\"key\": \"{key}\"")),
            "state seed still carries the bare {key}"
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
            // GH #932: the turn id carries the round's tag; every round here
            // is the standard one, so the index still counts 0, 1, ...
            (
                json!(episode_turn_id("s1", ROUND_E, 0)),
                user("how is the weather?")
            ),
            (
                json!(episode_turn_id("s1", ROUND_E, 1)),
                said("sunny, sixteen degrees")
            )
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
            (
                json!(episode_turn_id("s1", ROUND_E, 3)),
                user("and the day after?")
            ),
            (
                json!(episode_turn_id("s1", ROUND_E, 4)),
                said("rain, I am afraid")
            )
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
/// them dropping the collector's leaves the new tree no longer names. Beside
/// them every call carries whole the two families gated by the round and the
/// families of the session's own slots (`roster`, `consult`, `instructions`),
/// here empty but for `instructions` (GH #925).
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
                         "y": {"text": "{\"b\":1}"}},
               "history": {"$replace": true}, "pinned": {"$replace": true},
               "roster": {"$replace": true}, "consult": {"$replace": true}})
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
        json!({"tools": {"$replace": true, "x": {"text": "{\"a\":1}"}},
               "history": {"$replace": true}, "pinned": {"$replace": true},
               "instructions": {"$replace": true, "mode": {"text": "Be brief."},
                                "sidecar": {"text": "Fence it."}},
               "roster": {"$replace": true}, "consult": {"$replace": true}}),
        "only the family that moved, beside the gated and the session families"
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

/// A family of slots is sent only when it changed. The two families a round
/// gates (`history`, `pinned`) are the exception: every call carries them
/// whole, empty when the round sees nothing -- the model holds ONE system
/// tree, and a call that trusted the diff of an overlapping call of another
/// round would keep that round's summary leaf (GH #925, review I-2). So are
/// the families that hold a session's slots (`roster`, `consult`, and
/// `instructions` for its `peer` leaf), for the same reason one session
/// further (review I-4). The same text is the same prompt, so the provider's
/// cache keeps.
#[test]
fn system_leaves_only_when_changed() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let every_call = json!({"instructions": {"$replace": true, "mode": {"text": "Be brief."}},
                            "history": {"$replace": true}, "pinned": {"$replace": true},
                            "roster": {"$replace": true}, "consult": {"$replace": true}});
    let c1 = turn(&mut h, "s1", "t1", "a", "b", json!({}));
    assert_eq!(c1.body["system"], every_call);
    let c2 = turn(&mut h, "s1", "t2", "c", "d", json!({}));
    assert_eq!(
        c2.body["system"], every_call,
        "nothing changed, nothing sent but the gated and the session families: {:?}",
        c2.body
    );
    h.lane(
        "in_pack",
        json!({}),
        json!({}),
        json!({"system": {"identity": {"text": "You are T."}}}),
    );
    let c3 = turn(&mut h, "s1", "t3", "e", "f", json!({}));
    let mut want = every_call.clone();
    want["identity"] = json!({"$replace": true, "text": "You are T."});
    assert_eq!(
        c3.body["system"], want,
        "only the family that changed, as one $replace root"
    );
}

/// The collector wins at its own leaf: a pack leaf on a path the collector
/// holds (here the sidecar contract of its menu) is not written. The advise
/// mode is no leaf of the hive at all (review R2-I-1): it is the session's,
/// rides with the call, and a pack that names it writes nothing.
#[test]
fn collector_slot_wins_at_its_leaf() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    h.lane(
        "in_slots",
        json!({}),
        json!({}),
        json!({"system": {"instructions": {"sidecar": {"text": "collector says"}}}}),
    );
    h.curate(
        "s1",
        "t1",
        0,
        json!([user("a")]),
        mode("the session's mode"),
    );
    h.lane(
        "in_pack",
        json!({}),
        json!({}),
        json!({"system": {"instructions": {"sidecar": {"text": "pack says"},
                                           "mode": {"text": "pack says"},
                                           "reply": {"text": "reply kindly"}}}}),
    );
    let call = h.curate(
        "s1",
        "t2",
        0,
        json!([user("b")]),
        mode("the session's mode"),
    );
    assert_eq!(
        call.body["system"],
        json!({"instructions": {"$replace": true, "mode": {"text": "the session's mode"},
                                "reply": {"text": "reply kindly"},
                                "sidecar": {"text": "collector says"}},
               "history": {"$replace": true}, "pinned": {"$replace": true},
               "roster": {"$replace": true}, "consult": {"$replace": true}}),
        "the pack's leaf beside the collector's, the collector's at its own path, the \
         call's own mode"
    );
    assert_eq!(
        h.rows("SELECT owner FROM slots WHERE path = 'instructions.sidecar'"),
        vec![vec![json!("collector")]]
    );
    assert!(
        h.rows("SELECT owner FROM slots WHERE path = 'instructions.mode'")
            .is_empty(),
        "the mode is the session's, never the hive's"
    );
}

/// GH #871, R-IG-1 (GH #1085): an earlier answer keeps its block up to its
/// share of the usable window -- a tenth of `input_soft` (100 tokens here, 30
/// characters) -- and over it shows the mark where the block stood, with the
/// block's length; the call's hop names the cut.
#[test]
fn earlier_answer_keeps_its_block_up_to_its_share() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let small = "short.\n\n```sidecar\n{\"a\": 1}\n```";
    let big = format!(
        "long.\n\n```sidecar\n{{\"a\": \"{}\"}}\n```",
        "x".repeat(60)
    );
    let size = big.len() - big.find("```sidecar").expect("a block");
    turn(&mut h, "s1", "t1", "q1", small, json!({"input_soft": 100}));
    turn(&mut h, "s1", "t2", "q2", &big, json!({"input_soft": 100}));
    let call = h.curate("s1", "t3", 0, json!([user("q3")]), mode("Be brief."));
    let dropped = format!("long.\n\n...[dropped: sidecar ({size} chars) over budget]");
    assert_eq!(
        texts(&call),
        vec!["q1", small, "q2", dropped.as_str(), "q3"]
    );
    let cuts = call.hop["cuts"].as_array().expect("hop.cuts names the cut");
    assert_eq!(cuts.len(), 1, "{cuts:?}");
    assert_eq!(cuts[0]["what"], "sidecar");
    assert_eq!(cuts[0]["total"], json!(size));
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
a = scope["history_window"](rows, {"input_soft": 60})
b = scope["history_window"](list(reversed(rows)), {"input_soft": 60})
whole = scope["history_window"](rows, {})
print(json.dumps({"a": a, "same": a == b, "whole": whole,
                  "knobs": [scope[k] for k in ("KEEP_RECENT", "COMPRESS_AT",
                                               "LEAF_MAX_BYTES", "SIDECAR_SHARE")]}))
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
               {"origin": "assistant", "type": "text",
                "text": "done.\n\n...[dropped: sidecar (95 chars) over budget]"}]),
        "wall order, the orphan call out, the block over its share said (#871, R-IG-1)"
    );
    assert_eq!(got["same"], json!(true), "the input order does not matter");
    assert_eq!(
        got["whole"][1]["text"].as_str().map(|t| t.ends_with("```")),
        Some(true),
        "without a window the block goes whole (R-IG-1): {}",
        got["whole"]
    );
    assert_eq!(
        got["knobs"],
        json!([12, 0.5, 4194304, 0.1]),
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
    assert_eq!(h.state_in("last_call", ROUND_E), id);
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
        json!(h.state_in("last_call", ROUND_E))
    );
    // GH #925 (OR-BD.A.6): the strike is a fresh root without a context, so
    // the order carries the call's round as a header of the strike -- the
    // rebuild it starts has a round and may make a summary.
    assert_eq!(add.body["emit_headers"], json!({"audience_set": ROUND_E}));
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
    assert_eq!(
        h.state_in("armed_call", ROUND_E),
        "",
        "the fired order is disarmed"
    );
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
    let armed = h.state_in("armed_call", ROUND_E);
    assert_eq!(json!(armed), second.body["emit_body"]["curator_call"]);
    // A strike for the first call now (the race: it fired while the second tap
    // armed) must not disarm the second order.
    h.fire(&first);
    assert_eq!(h.state_in("armed_call", ROUND_E), armed);
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
        h.rows(&format!(
            "SELECT owner FROM slots WHERE path = '{}'",
            curator_hive::summary_slot(ROUND_E)
        )),
        vec![vec![json!("curator")]],
        "the summary slot of the round (GH #943)"
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
        "one rebuild of a round at a time: the second trigger merges"
    );
    h.answer("Merged.", "stop");
    assert_eq!(h.rows("SELECT COUNT(*) FROM summaries")[0][0], json!(1));
    assert_eq!(
        h.state_in("rebuild_running", ROUND_E),
        "",
        "the lock is released"
    );
    assert_eq!(
        h.rows("SELECT path FROM slots ORDER BY path"),
        vec![
            vec![json!(curator_hive::summary_slot(ROUND_E))],
            vec![json!("identity")]
        ],
        "the advise mode rides with each call, never in `slots` (review R2-I-1)"
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
        assert_eq!(h.state_in("rebuild_running", ROUND_E), "");
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
    // A summary has no typed length (OR-IG-5, GH #1085); only one whose leaf
    // would not fit the carrier -- one system leaf, 4 MiB -- is refused,
    // with its size and the bound (review focus K-5, R-IG-2).
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
    h.answer(
        &"x".repeat(meclaw_cells::content_budget::CARRIER_MAX_BYTES),
        "stop",
    );
    assert_eq!(h.rows("SELECT COUNT(*) FROM summaries")[0][0], json!(0));
    assert!(
        h.stderr
            .iter()
            .any(|l| l.contains("too_long: ") && l.contains("> 4194304 bytes")),
        "{:?}",
        h.stderr
    );
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
                // GH #932: `<session>#<tag>-<index>`, one round throughout.
                json!(episode_turn_id("s1", ROUND_E, 0)),
                json!({"origin": "user", "type": "text", "text": "hello"})
            ),
            (
                json!(episode_turn_id("s1", ROUND_E, 1)),
                json!({"origin": "assistant", "type": "text", "text": "hi"})
            ),
            (
                json!(episode_turn_id("s1", ROUND_E, 2)),
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

/// The close batch is a reader of the ledger like the window (GH #925,
/// review I-3): it hands on only the rows whose audience holds the close
/// round, `context.audience_set` of `in_close` -- the round the session
/// keeper took from the turn that opened the generation. One session with
/// rows under {e,a} and {e,b} (a tool round in each, review R2-I-5: the
/// raw rows are gated like the said ones) and no round at all:
/// before the gate a close in {e,a} handed on all six participant turns
/// (`turn_count` 6 in the fix round's driver), so the memory gave {e,a} what
/// only {e,b} heard and what no round declared. A close without a round
/// reads nothing and leaves an empty batch.
#[test]
fn a_close_hands_on_only_the_rows_its_round_may_see() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    // The close round as a caller may write it: unsorted, with spaces.
    let ea = json!("[ \"member:e\", \"member:a\" ]");
    let call = h.curate_as(
        ea.clone(),
        "s1",
        "t1",
        0,
        json!([
            user("ea: the gate opens with quince"),
            tool_call("c1", "look"),
            tool_result("c1", "ea: under the step")
        ]),
        Value::Null,
    );
    h.tap(
        &call,
        "stop",
        json!({}),
        json!([said("ea: noted\n\n```sidecar\n{}\n```")]),
    );
    let call = h.curate_as(
        json!("[\"member:e\",\"member:b\"]"),
        "s1",
        "t2",
        0,
        json!([
            user("eb: the key is behind the kettle"),
            tool_call("c2", "peek"),
            tool_result("c2", "eb: behind the kettle, wrapped")
        ]),
        Value::Null,
    );
    h.tap(&call, "stop", json!({}), json!([said("eb: noted")]));
    let call = h.curate_as(
        Value::Null,
        "s1",
        "t3",
        0,
        json!([user("none: the lantern hangs under the stairs")]),
        Value::Null,
    );
    h.tap(&call, "stop", json!({}), json!([said("none: noted")]));
    assert_eq!(
        h.rows(
            "SELECT DISTINCT audience_set FROM wall WHERE session_id = 's1' \
             ORDER BY audience_set"
        ),
        // PP-BD-12 (GH #932): the round-less round writes `[]`, not NULL --
        // NULL is left to rows from before the rule -- and `[]` sorts last.
        vec![
            vec![json!(ROUND_AE)],
            vec![json!("[\"member:b\",\"member:e\"]")],
            vec![json!("[]")]
        ],
        "the session holds rows of {{e,a}}, of {{e,b}} and of no round"
    );

    h.out.clear();
    h.lane(
        "in_close",
        json!({"session_id": "s1", "audience_set": ea}),
        json!({}),
        json!({"messages": []}),
    );
    let w = h.routed("write");
    assert_eq!(w.len(), 1, "one close, one batch");
    assert_eq!(
        w[0].messages(),
        vec![user("ea: the gate opens with quince"), said("ea: noted")],
        "only the turns of {{e,a}}, the answer without its block"
    );
    let rounds: Vec<Value> = w[0].body["rounds"]
        .as_array()
        .expect("rounds")
        .iter()
        .map(|r| r["turn"].clone())
        .collect();
    assert_eq!(
        rounds,
        vec![
            tool_call("c1", "look"),
            tool_result("c1", "ea: under the step")
        ],
        "only the tool round of {{e,a}}, not {{e,b}}'s beside it"
    );
    assert_eq!(w[0].hop["turn_count"], "2", "counts what was handed on");
    assert_eq!(w[0].hop["round_count"], "2", "counts what was handed on");
    let batch = sj::to_string(&w[0].body).expect("serialise");
    assert!(
        !batch.contains("eb:") && !batch.contains("none:"),
        "no word of {{e,b}} or of the round-less round: {batch}"
    );

    // A close without a round: nothing passes, the batch is empty.
    h.out.clear();
    h.lane(
        "in_close",
        json!({"session_id": "s1", "audience_set": null}),
        json!({}),
        json!({"messages": []}),
    );
    let w = h.routed("write");
    assert_eq!(w.len(), 1, "a close without a round still closes");
    assert_eq!(w[0].hop["turn_count"], "0");
    assert_eq!(w[0].hop["round_count"], "0");
    assert_eq!(w[0].messages(), Vec::<Value>::new());
    assert_eq!(w[0].body["rounds"], json!([]));
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
               "persona": {"$replace": true, "voice": {"text": "calm"}},
               "history": {"$replace": true}, "pinned": {"$replace": true},
               "instructions": {"$replace": true, "mode": {"text": "Be brief."}},
               "roster": {"$replace": true}, "consult": {"$replace": true}})
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
    assert_eq!(h.state_in("last_call", ROUND_E), call);
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

// ======================================== GH #925: the audience of the round

/// A refusal of a `history_*` tool: its code in `error` (curator/history).
const HISTORY_REFUSAL: &str =
    r#"{"detail":"no block has this id","error":"not_found","tool":"history_read"}"#;
/// A refusal of a file-space tool: `ok` false, the code in `error.code`.
const FILE_REFUSAL: &str =
    r#"{"error":{"code":"no_such_file","message":"nothing at /a"},"ok":false,"op":"read"}"#;
/// A final answer whose block rewrites a standing instruction.
const CORRECTED: &str =
    "Done.\n\n```sidecar\n{\"correction\": \"Confirm the date before booking.\"}\n```";
/// A round of two members, as its context carries it (unsorted, the way a
/// caller may write it) and as a row keeps it (canonical).
const ROUND_EA: &str = "[\"member:e\",\"member:a\"]";
const ROUND_AE: &str = "[\"member:a\",\"member:e\"]";

/// A section of the model's block on `in_section`, the way the splitter
/// hands it with the answer's context.
fn section(h: &mut Hive, audience: Value, turn_id: &str, name: &str, payload: Value) {
    h.lane(
        "in_section",
        json!({"session_id": "s1", "turn_id": turn_id, "audience_set": audience}),
        json!({"section": name}),
        json!({"messages": [], "section": name, "payload": payload}),
    );
}

/// A gap's find on the internal lane `in_addendum`, the way `./push` hands it.
fn addendum(h: &mut Hive, audience: Value, turn_id: &str, pair: Value) {
    let msg = Msg {
        context: obj(json!({"session_id": "s1", "turn_id": turn_id, "audience_set": audience})),
        hop: obj(json!({"route": "in_addendum", "session_id": "s1", "turn_id": turn_id})),
        body: obj(json!({"messages": pair})),
        reply_to: String::new(),
    };
    h.pump("./push", msg);
}

/// One pin of the hive `probe` through the pin door, under `audience`.
fn pin(h: &mut Hive, audience: Value, text: &str) {
    h.lane(
        "in_pin",
        json!({"audience_set": audience}),
        json!({}),
        json!({"pins": [{"text": text, "source": "probe"}]}),
    );
}

/// Every row a round causes carries the round's audience, in its canonical
/// form (sorted, no duplicates, no whitespace): the wall rows of the round, of
/// the model's answer and of a gap's find, and every mark -- one out of a
/// failed tool, one out of the answer's block, one out of a section.
#[test]
fn the_round_is_written_on_every_row_it_causes() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let call = h.curate_as(
        json!(ROUND_EA),
        "s1",
        "t1",
        0,
        json!([
            user("look it up"),
            tool_call("c1", "history_read"),
            tool_result("c1", HISTORY_REFUSAL)
        ]),
        with_menu(mode("Be brief."), &["history_read"]),
    );
    h.tap(&call, "stop", json!({}), json!([said(CORRECTED)]));
    section(
        &mut h,
        json!(ROUND_EA),
        "t1",
        "window",
        json!({"pin": "#0123456789ab"}),
    );
    addendum(
        &mut h,
        json!(ROUND_EA),
        "t1",
        json!([
            tool_call("r1", "memory_recall"),
            tool_result("r1", "found it")
        ]),
    );
    assert_eq!(
        h.rows("SELECT kind FROM wall ORDER BY seq"),
        [
            "user",
            "tool_call",
            "tool_result",
            "assistant",
            "recall",
            "recall"
        ]
        .iter()
        .map(|k| vec![json!(k)])
        .collect::<Vec<_>>(),
        "{:?}",
        h.stderr
    );
    assert_eq!(
        h.rows("SELECT DISTINCT audience_set FROM wall"),
        vec![vec![json!(ROUND_AE)]]
    );
    assert_eq!(
        h.rows("SELECT kind, value FROM marks ORDER BY kind"),
        vec![
            vec![json!("correction"), json!("")],
            vec![json!("pin"), json!("0123456789ab")],
            vec![json!("tool_error"), json!("history_read:not_found")]
        ]
    );
    assert_eq!(
        h.rows("SELECT DISTINCT audience_set FROM marks"),
        vec![vec![json!(ROUND_AE)]]
    );
}

/// A round that declares no audience is written all the same, its rows with
/// `audience_set` `[]` (PP-BD-12, GH #932): nothing refuses the round, the
/// rows declare no round, so only a round-less read of their own session
/// finds them (`round_where`) and no reader hands them to another round.
/// NULL stays the mark of a row from before the rule (OR-BD-5) -- before
/// GH #932 these rows were NULL, which made them indistinguishable from such
/// legacy rows and invisible to the store filter. Its call is marked once
/// `missing_audience` by `./policy` (OR-BD-4), that mark `[]` like every row
/// of the round.
#[test]
fn a_round_without_an_audience_writes_the_empty_set() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    h.lane(
        "in_curate",
        json!({"session_id": "s1", "turn_id": "t1", "iter": "0", "audience_set": null}),
        json!({"session_id": "s1", "turn_id": "t1", "iter": "0", "phase": ""}),
        json!({"messages": [
            user("look it up"),
            tool_call("c1", "history_read"),
            tool_result("c1", HISTORY_REFUSAL)
        ]}),
    );
    section(
        &mut h,
        Value::Null,
        "t1",
        "window",
        json!({"release": "#0123456789ab"}),
    );
    assert_eq!(
        h.rows("SELECT kind, audience_set FROM wall ORDER BY seq"),
        vec![
            vec![json!("user"), json!("[]")],
            vec![json!("tool_call"), json!("[]")],
            vec![json!("tool_result"), json!("[]")]
        ]
    );
    assert_eq!(
        h.rows("SELECT kind, audience_set FROM marks ORDER BY kind"),
        vec![
            vec![json!("missing_audience"), json!("[]")],
            vec![json!("release"), json!("[]")],
            vec![json!("tool_error"), json!("[]")]
        ]
    );
}

/// A pin without a round is refused whole, in the form of every refusal of
/// the pin door: nothing parked, nothing read, nothing written, nothing out.
#[test]
fn a_pin_without_a_round_is_refused() {
    if !shipped() {
        return;
    }
    for audience in [Value::Null, json!(""), json!("member:e")] {
        let mut h = Hive::new();
        pin(&mut h, audience.clone(), "keep this");
        for table in ["pins", "blocks", "state WHERE key LIKE 'pending:%'"] {
            assert_eq!(
                h.rows(&format!("SELECT COUNT(*) FROM {table}"))[0][0],
                json!(0),
                "{audience}: {table}"
            );
        }
        assert!(h.out.is_empty(), "{audience}: {:?}", h.out);
        assert!(
            h.stderr
                .iter()
                .any(|l| l.contains("in_pin refused: missing_audience")),
            "{audience}: {:?}",
            h.stderr
        );
    }
}

/// A pin carries the round it came in. The same text pinned again under
/// another round takes that round (OR-BD.A.3: the latest pin decides) and
/// stays one row.
#[test]
fn a_pin_carries_its_round_and_the_latest_pin_decides() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    pin(&mut h, json!(ROUND_EA), "keep this");
    assert_eq!(
        h.rows("SELECT audience_set, until FROM pins"),
        vec![vec![json!(ROUND_AE), json!("")]]
    );
    pin(&mut h, json!("[\"member:b\"]"), "keep this");
    assert_eq!(
        h.rows("SELECT audience_set, until FROM pins"),
        vec![vec![json!("[\"member:b\"]"), json!("")]],
        "{:?}",
        h.stderr
    );
}

/// A tool's refusal in its own words: a sentence with a path, no code.
const FREE_TEXT_REFUSAL: &str = r#"{"error":"cannot read /srv/data/notes.txt: permission denied"}"#;

/// A tool result that reports a failure leaves one `tool_error` mark,
/// `<tool>:<code>`, per result: the wall's own key makes it once, so a round
/// sent again or grown by an iteration marks nothing twice. The tool's name
/// comes from the call of the same id (`unknown` without one), the code from
/// the result (`error` without one). A clean result marks nothing, and nor
/// does an evidence pair -- it is no tool result of the model's. The value
/// reaches a judge as a sample (GH #925, OR-BD-6: never text), so a name
/// only enters it in code form (`[a-z0-9_.-]{1,64}`) and a code only in the
/// narrow form `[a-z][a-z_]{0,31}` (review m-5: one word with a dot is a file
/// name, `report.txt` is no code; review R2-I-2: a word with a digit or a `-`
/// is a password, a card number, a name -- `hunter2`, `4111111111111111`,
/// `secret-name` are none): a sentence or such a word is `error`, a name
/// that is none is `unknown`. A name enters only when the menu holds the tool
/// (OR-BD-92: a model can make one up out of the person's words in the form
/// of a name -- `lookup_alice`, `lookup.alice-smith` reached a judge); one
/// it does not hold is `unknown` too. An `error` that says there is none
/// (`none`, `null`, `false`, any case) is no failure.
#[test]
fn a_failed_tool_result_is_one_tool_error_mark() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    // (call id, the tool it calls -- "" for no call --, the result, the mark).
    let table: [(&str, &str, &str, Option<&str>); 22] = [
        (
            "c1",
            "history_read",
            HISTORY_REFUSAL,
            Some("history_read:not_found"),
        ),
        (
            "c2",
            "file_read",
            FILE_REFUSAL,
            Some("file_read:no_such_file"),
        ),
        (
            "c3",
            "file_read",
            r#"{"ok":true,"op":"read","text":"fine"}"#,
            None,
        ),
        ("r1", "memory_recall", r#"{"error":"store_refused"}"#, None),
        ("x9", "", r#"{"ok":false}"#, Some("unknown:error")),
        (
            "c5",
            "web_fetch",
            FREE_TEXT_REFUSAL,
            Some("web_fetch:error"),
        ),
        (
            "c6",
            "file_edit",
            r#"{"error_code":"No Such File"}"#,
            Some("file_edit:error"),
        ),
        (
            "c7",
            "web_search",
            r#"{"error_code":"rate_limited"}"#,
            Some("web_search:rate_limited"),
        ),
        (
            "c8",
            "Fetch Page",
            r#"{"error":"timeout"}"#,
            Some("unknown:timeout"),
        ),
        ("c9", "file_write", r#"{"error":"none"}"#, None),
        (
            "c10",
            "file_write",
            r#"{"error":"Null","op":"write"}"#,
            None,
        ),
        ("c11", "file_write", r#"{"error":" FALSE "}"#, None),
        // Review m-5: a file name is no code; the tool's dotted name keeps
        // its form, and a dotted code falls to the next place in the body.
        (
            "c12",
            "file.read",
            r#"{"error":"report.txt"}"#,
            Some("file.read:error"),
        ),
        (
            "c13",
            "file_read",
            r#"{"ok":false,"error":{"code":"io.denied"}}"#,
            Some("file_read:error"),
        ),
        (
            "c14",
            "file_read",
            r#"{"error":"v1.2","error_code":"denied"}"#,
            Some("file_read:denied"),
        ),
        // Review R2-I-2: a digit, a `-`, a long word -- never a code.
        (
            "c15",
            "login",
            r#"{"ok":false,"error":{"code":"hunter2"}}"#,
            Some("login:error"),
        ),
        (
            "c16",
            "pay",
            r#"{"error":"4111111111111111"}"#,
            Some("pay:error"),
        ),
        (
            "c17",
            "file_read",
            r#"{"error":"secret-name"}"#,
            Some("file_read:error"),
        ),
        (
            "c18",
            "web_fetch",
            r#"{"error_code":"http_404"}"#,
            Some("web_fetch:error"),
        ),
        (
            "c19",
            "web_search",
            r#"{"error_code":"a_code_longer_than_thirty_two_chars"}"#,
            Some("web_search:error"),
        ),
        // OR-BD-92: a name in the form of a name that no menu holds.
        (
            "c20",
            "lookup_alice",
            r#"{"error":"not_found"}"#,
            Some("unknown:not_found"),
        ),
        (
            "c21",
            "lookup.alice-smith",
            r#"{"error":"not_found"}"#,
            Some("unknown:not_found"),
        ),
    ];
    // The menu: every tool of the table but the two made-up names.
    let offered = with_menu(
        mode("Be brief."),
        &[
            "history_read",
            "history_search",
            "file_read",
            "file.read",
            "file_edit",
            "file_write",
            "memory_recall",
            "web_fetch",
            "web_search",
            "Fetch Page",
            "login",
            "pay",
        ],
    );
    let mut round = vec![user("read a few things")];
    round.extend(
        table
            .iter()
            .filter(|(_, name, _, _)| !name.is_empty())
            .map(|(id, name, _, _)| tool_call(id, name)),
    );
    round.extend(table.iter().map(|(id, _, text, _)| tool_result(id, text)));
    let marks = |h: &Hive| {
        h.rows(
            "SELECT value, session_id, turn_id, audience_set FROM marks \
             WHERE kind = 'tool_error' ORDER BY value",
        )
    };
    let mut values: Vec<&str> = table.iter().filter_map(|(_, _, _, mark)| *mark).collect();
    values.sort_unstable();
    let want: Vec<Vec<Value>> = values
        .iter()
        .map(|v| vec![json!(v), json!("s1"), json!("t1"), json!(ROUND_E)])
        .collect();
    h.curate("s1", "t1", 1, json!(round), offered.clone());
    assert_eq!(marks(&h), want, "{:?}", h.stderr);
    h.curate("s1", "t1", 1, json!(round), offered.clone());
    assert_eq!(marks(&h), want, "the same round twice marks nothing twice");
    let mut grown = round.clone();
    grown.push(tool_call("c4", "history_search"));
    grown.push(tool_result("c4", r#"{"hits":[],"tool":"history_search"}"#));
    h.curate("s1", "t1", 2, json!(grown), mode("Be brief."));
    assert_eq!(marks(&h), want, "a clean result marks nothing");
    assert_eq!(
        h.rows("SELECT COUNT(*) FROM marks")[0][0],
        json!(values.len())
    );
    assert!(
        h.rows("SELECT at FROM marks")
            .iter()
            .all(|r| !r[0].as_str().unwrap_or("").is_empty()),
        "every mark says when"
    );
}

/// The model's own output leaves two marks off the tap: `ask` when it calls
/// the ask-back tool, `correction` when the block of its final answer
/// rewrites a standing instruction -- each once per round (OR-BD.A.4), so the
/// same output delivered twice marks nothing twice. Any other tool, the block
/// beside a call, a section without words and the legacy single-section
/// fence mark nothing.
#[test]
fn an_ask_and_a_correction_are_marked_once_off_the_tap() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let call = h.curate(
        "s1",
        "t1",
        0,
        json!([user("book a table")]),
        mode("Be brief."),
    );
    for _ in 0..2 {
        h.tap(
            &call,
            "tool_calls",
            json!({}),
            json!([tool_call("q1", "ask_requester")]),
        );
    }
    let call = h.curate(
        "s1",
        "t2",
        0,
        json!([user("for two, at eight")]),
        mode("Be brief."),
    );
    for _ in 0..2 {
        h.tap(&call, "stop", json!({}), json!([said(CORRECTED)]));
    }
    let marks = |h: &Hive| {
        h.rows("SELECT kind, value, session_id, turn_id, audience_set FROM marks ORDER BY seq")
    };
    let want = vec![
        vec![
            json!("ask"),
            json!("ask_requester"),
            json!("s1"),
            json!("t1"),
            json!(ROUND_E),
        ],
        vec![
            json!("correction"),
            json!(""),
            json!("s1"),
            json!("t2"),
            json!(ROUND_E),
        ],
    ];
    assert_eq!(marks(&h), want, "{:?}", h.stderr);
    let call = h.curate("s1", "t3", 0, json!([user("why?")]), mode("Be brief."));
    h.tap(
        &call,
        "tool_calls",
        json!({}),
        json!([tool_call("k1", "consult_cogny"), said(CORRECTED)]),
    );
    let call = h.curate("s1", "t4", 0, json!([user("and now?")]), mode("Be brief."));
    h.tap(
        &call,
        "stop",
        json!({}),
        json!([said(
            "Fine.\n\n```sidecar\n{\"memory\": {\"nothing_new\": true}, \"correction\": \" \"}\n```"
        )]),
    );
    let call = h.curate("s1", "t5", 0, json!([user("and then?")]), mode("Be brief."));
    h.tap(
        &call,
        "stop",
        json!({}),
        json!([said("Old.\n\n```memory\n{\"correction\": \"x\"}\n```")]),
    );
    assert_eq!(
        marks(&h),
        want,
        "no other tool, no block beside a call, no empty section, no legacy fence"
    );
}

// ----------------------------------------------------- GH #1018 replay time

/// GH #1018: one turn and its answer on `channel`, with `happened_at` in the
/// context where the HTTP door puts a request header. Returns the `happened_at`
/// of every `turn_write` the round caused -- the value the member edge promotes
/// to the memory hive's `context.happened_at`, the episode's time.
fn replayed_times(h: &mut Hive, channel: &str, happened_at: &str) -> Vec<String> {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": "t1", "iter": "0", "phase": ""});
    let ctx = json!({"session_id": "s1", "turn_id": "t1", "iter": "0", "channel": channel,
                     "audience_set": ROUND_E, "happened_at": happened_at});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("we adopted a dog last week")]}),
    );
    let calls = h.routed("brain");
    assert_eq!(calls.len(), 1, "one round, one call: {:?}", h.out);
    let mut episodes = h.routed("turn_write");
    h.out.clear();
    h.tap(&calls[0], "stop", json!({}), json!([said("a dog!")]));
    episodes.extend(h.routed("turn_write"));
    assert_eq!(episodes.len(), 2, "the person's turn and the answer");
    episodes
        .iter()
        .map(|m| {
            m.hop["happened_at"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

const REPLAYED: &str = "2020-01-02T03:04:05Z";
const REPLAYED_WALL: &str = "2020-01-02T03:04:05.000000Z";

#[test]
fn gh1018_a_replay_channel_keeps_the_time_of_its_turn() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("intake", "replay_channel_prefix", json!("replay-"))]);
    assert_eq!(
        replayed_times(&mut h, "replay-c1-s1", REPLAYED),
        [REPLAYED_WALL, REPLAYED_WALL],
        "the turn and its answer carry the time the turn was said"
    );
    // The wall keeps the ingest clock: it is the curator's own ledger.
    let walls = h.rows("SELECT DISTINCT at FROM wall");
    assert!(
        walls.iter().all(|r| r[0] != json!(REPLAYED_WALL)),
        "{walls:?}"
    );
    // The rest of the way is shipped already and pinned here so it stays: the
    // member edge promotes the writer's hop time to the memory hive's context,
    // which the memory hive's writer stores as the episode's `happened_at`.
    let member = read_json(&repo("templates/member/config.json"));
    let edge = member["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .iter()
        .find(|e| {
            e["from"] == "./assistants"
                && e["to"] == "./memory-hive"
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains("'turn_write'"))
        })
        .expect("the member's episode edge");
    assert_eq!(
        edge["modifier"]["set_context"]["happened_at"],
        json!("has(hop.happened_at) ? hop.happened_at : ''")
    );
}

#[test]
fn gh1018_off_by_default_and_never_on_a_live_channel() {
    if !shipped() {
        return;
    }
    assert_eq!(
        cell_config("intake")["params"]["replay_channel_prefix"],
        json!(""),
        "the switch ships off"
    );
    for (prefix, channel, at) in [
        ("", "replay-c1-s1", REPLAYED),           // switch off
        ("replay-", "telegram-42", REPLAYED),     // a live channel
        ("replay-", "replay-c1-s1", "last week"), // an unreadable time
    ] {
        let mut h = Hive::with(&[("intake", "replay_channel_prefix", json!(prefix))]);
        let times = replayed_times(&mut h, channel, at);
        assert!(
            times
                .iter()
                .all(|t| !t.is_empty() && !t.starts_with("2020-")),
            "prefix {prefix:?}, channel {channel:?}, time {at:?}: the ingest clock, got {times:?}"
        );
    }
}

// ================================================ GH #1079: the word-for-word check

/// A section of the model's block the parent's splitter stamped with a
/// `sidecar_verify` rule (GH #1079), on `in_section` with the answer's context
/// -- the person's episode of the turn included, as the talky's brain edge
/// promotes it -- and, when a field names `answer`, the answer text beside the
/// body as `verify_answer`. Returns what left the hive: the sections on
/// `sidecar` and the refusals on `reject`.
fn checked_section(
    h: &mut Hive,
    call: &Msg,
    name: &str,
    rule: &str,
    payload: Value,
    answer: Option<&str>,
) -> (Vec<Msg>, Vec<Msg>) {
    let mut ctx = call.context.clone();
    for k in ["curator_call", "turn_id", "session_id", "iter"] {
        ctx.insert(k.into(), call.hop[k].clone());
    }
    ctx.insert(
        "episode_turn_id".into(),
        call.hop
            .get("episode_turn_id")
            .cloned()
            .unwrap_or_else(|| json!("")),
    );
    let mut body = json!({"messages": [], "section": name, "payload": payload});
    if let Some(a) = answer {
        body["verify_answer"] = json!(a);
    }
    h.out.clear();
    h.lane(
        "in_section",
        Value::Object(ctx),
        json!({"section": name, "turn_id": call.hop["turn_id"], "sidecar_verify": rule}),
        body,
    );
    (h.routed("sidecar"), h.routed("reject"))
}

/// The one section a checked section leaves as: the splitter's hop without the
/// rule, the splitter's body without the answer text.
fn the_checked_section(sides: &[Msg], name: &str) -> Value {
    assert_eq!(sides.len(), 1, "a checked section leaves once: {sides:?}");
    let s = &sides[0];
    assert_eq!(s.hop["route"], "sidecar");
    assert_eq!(s.hop["section"], name);
    assert!(
        !s.hop.contains_key("sidecar_verify"),
        "the rule stays inside the composite: {:?}",
        s.hop
    );
    assert!(
        !s.body.contains_key("verify_answer"),
        "the answer text never leaves on a section: {:?}",
        s.body
    );
    s.body["payload"].clone()
}

fn reasons(rejects: &[Msg]) -> Vec<(String, String)> {
    rejects
        .iter()
        .map(|m| {
            (
                m.hop["field"].as_str().unwrap_or("").to_string(),
                m.hop["reject_reason"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

#[test]
fn sidecar_verify_drops_item_whose_quote_is_not_in_the_turn() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let call = h.curate(
        "s1",
        "t1",
        0,
        json!([user("I miss Werner's patient ways. Yes.")]),
        mode("Be brief."),
    );
    let (sides, rejects) = checked_section(
        &mut h,
        &call,
        "needs",
        r#"{"quote":"turn"}"#,
        json!({"needs": [
            {"need": "connection", "quote": "miss Werner's patient ways"},
            {"need": "solitude", "quote": "I like being alone"},
            // A paraphrase is no quote: no fuzzy match counts as a source.
            {"need": "patience", "quote": "miss Werner's patience"},
            // Inside a word is no word boundary.
            {"need": "cut", "quote": "iss Werner's patient"},
            // Under three words a quote counts only as a whole turn, and this
            // one is part of one.
            {"need": "short", "quote": "patient ways"}
        ]}),
        None,
    );
    let payload = the_checked_section(&sides, "needs");
    assert_eq!(
        payload,
        json!({"needs": [{"need": "connection", "quote": "miss Werner's patient ways",
                          "verified": ["quote"]}]}),
        "only the item whose quote the person said stays"
    );
    assert_eq!(
        reasons(&rejects),
        vec![("quote".to_string(), "quote_not_in_source".to_string()); 4],
        "one refusal per dropped item: {rejects:?}"
    );
}

#[test]
fn sidecar_verify_keeps_verbatim_quote_after_normal_form() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let call = h.curate(
        "s1",
        "t1",
        0,
        json!([
            user("I told her: \u{201c}Gardening keeps me  SANE\u{201d} \u{2013} every single day."),
            user("Yes.")
        ]),
        mode("Be brief."),
    );
    let (sides, rejects) = checked_section(
        &mut h,
        &call,
        "needs",
        r#"{"quote":"turn"}"#,
        json!({"needs": [
            // Case, straight quotes for curly ones, a plain dash, one space.
            {"need": "a", "quote": "\"gardening keeps me sane\" - every single day"},
            // Punctuation at the edge of a quote is no part of it.
            {"need": "b", "quote": "...Gardening keeps me sane!"},
            // Whitespace of any width is one space.
            {"need": "c", "quote": "keeps   me\n sane"},
            // A short quote that IS a whole turn.
            {"need": "d", "quote": "yes"}
        ]}),
        None,
    );
    let payload = the_checked_section(&sides, "needs");
    let kept: Vec<&str> = payload["needs"]
        .as_array()
        .expect("the array")
        .iter()
        .map(|i| i["need"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(kept, ["a", "b", "c", "d"], "{payload}");
    assert!(
        payload["needs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["verified"] == json!(["quote"])),
        "{payload}"
    );
    assert!(rejects.is_empty(), "{rejects:?}");
}

/// The apostrophes people type beside the straight one fold like the curly
/// ones: the acute accent (U+00B4) many European keyboards produce for one, the
/// modifier letter, the backtick and the single guillemets. A copy that
/// straightens them is still a copy, in either direction.
#[test]
fn sidecar_verify_folds_every_apostrophe_form() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let call = h.curate(
        "s1",
        "t1",
        0,
        json!([
            user("How\u{00b4}s it going today?"),
            user("I can\u{02bc}t sleep without the radio."),
            user("We called it the \u{2039}quiet room\u{203a} back then."),
            user("Don`t forget the garden gate."),
            user("It's my brother's birthday on Sunday.")
        ]),
        mode("Be brief."),
    );
    let (sides, rejects) = checked_section(
        &mut h,
        &call,
        "notes",
        r#"{"quote":"turn"}"#,
        json!({"notes": [
            {"note": "a", "quote": "how's it going today"},
            {"note": "b", "quote": "I can't sleep without the radio"},
            {"note": "c", "quote": "the 'quiet room' back then"},
            {"note": "d", "quote": "don't forget the garden gate"},
            {"note": "e", "quote": "it\u{00b4}s my brother\u{02bc}s birthday"}
        ]}),
        None,
    );
    let payload = the_checked_section(&sides, "notes");
    let kept: Vec<&str> = payload["notes"]
        .as_array()
        .expect("the array")
        .iter()
        .map(|i| i["note"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(kept, ["a", "b", "c", "d", "e"], "{payload} {rejects:?}");
    assert!(rejects.is_empty(), "{rejects:?}");
}

/// A typo in a rule is said, never swallowed: a `mode` the cell does not know
/// checks as `drop` and says so, and an `items` that names no array of the
/// section checks nothing -- the section goes on with no item stamped
/// `verified`, and stderr names the array it looked for.
#[test]
fn sidecar_verify_a_rule_with_a_typo_is_said() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let call = h.curate(
        "s1",
        "t1",
        0,
        json!([user("I miss Werner's patient ways.")]),
        mode("Be brief."),
    );
    let (sides, rejects) = checked_section(
        &mut h,
        &call,
        "notes",
        r#"{"mode":"flag","quote":"turn"}"#,
        json!({"notes": [{"note": "a", "quote": "I like being alone"}]}),
        None,
    );
    let payload = the_checked_section(&sides, "notes");
    assert_eq!(
        payload,
        json!({"notes": []}),
        "an unknown mode checks as drop"
    );
    assert_eq!(rejects.len(), 1, "{rejects:?}");
    assert!(
        h.stderr
            .iter()
            .any(|e| e.contains("mode") && e.contains("flag")),
        "the unknown mode is said: {:?}",
        h.stderr
    );

    let (sides, rejects) = checked_section(
        &mut h,
        &call,
        "notes",
        r#"{"items":"note","quote":"turn"}"#,
        json!({"notes": [{"note": "a", "quote": "I like being alone"}]}),
        None,
    );
    let payload = the_checked_section(&sides, "notes");
    assert!(
        payload["notes"][0].get("verified").is_none(),
        "nothing was checked, so nothing is stamped: {payload}"
    );
    assert!(rejects.is_empty(), "{rejects:?}");
    assert!(
        h.stderr
            .iter()
            .any(|e| e.contains("items") && e.contains("note")),
        "the array it looked for is said: {:?}",
        h.stderr
    );
}

#[test]
fn sidecar_verify_reason_quote_missing() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let call = h.curate(
        "s1",
        "t1",
        0,
        json!([user("I miss Werner's patient ways.")]),
        mode("Be brief."),
    );
    let (sides, rejects) = checked_section(
        &mut h,
        &call,
        "needs",
        r#"{"quote":"turn"}"#,
        json!({"needs": [
            {"need": "no field"},
            {"need": "empty", "quote": ""},
            {"need": "not text", "quote": 42},
            {"need": "only marks", "quote": " ... "},
            "not an object"
        ]}),
        None,
    );
    // Every item fell, and the section still leaves: "annotated, nothing
    // proven" is not "not annotated".
    assert_eq!(the_checked_section(&sides, "needs"), json!({"needs": []}));
    assert_eq!(
        reasons(&rejects),
        vec![("quote".to_string(), "quote_missing".to_string()); 5],
        "{rejects:?}"
    );
}

#[test]
fn sidecar_verify_turn_plus_recall_accepts_memory_text() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let call = h.curate(
        "s1",
        "t1",
        0,
        json!([
            user("What do you remember about my balcony?"),
            tool_call("r1", "memory_recall"),
            tool_result(
                "r1",
                "The person grows tomatoes on the balcony every summer."
            )
        ]),
        mode("Be brief."),
    );
    let claims = json!({"claims": [
        {"claim": "grows tomatoes", "quote": "grows tomatoes on the balcony"},
        {"claim": "asked", "quote": "remember about my balcony"}
    ]});
    let (sides, rejects) = checked_section(
        &mut h,
        &call,
        "claims",
        r#"{"quote":"turn+recall"}"#,
        claims.clone(),
        None,
    );
    let payload = the_checked_section(&sides, "claims");
    assert_eq!(
        payload["claims"].as_array().map(Vec::len),
        Some(2),
        "the memory's words count beside the person's: {payload} {rejects:?}"
    );
    assert!(rejects.is_empty(), "{rejects:?}");
    // The same quote under `turn`: the memory's words are no turn.
    let (sides, rejects) =
        checked_section(&mut h, &call, "claims", r#"{"quote":"turn"}"#, claims, None);
    let payload = the_checked_section(&sides, "claims");
    assert_eq!(
        payload["claims"],
        json!([{"claim": "asked", "quote": "remember about my balcony", "verified": ["quote"]}])
    );
    assert_eq!(
        reasons(&rejects),
        vec![("quote".to_string(), "quote_not_in_source".to_string())]
    );
}

#[test]
fn sidecar_verify_answer_source_checks_answer_text() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let call = h.curate(
        "s1",
        "t1",
        0,
        json!([user("Can you book us a table?")]),
        mode("Be brief."),
    );
    let (sides, rejects) = checked_section(
        &mut h,
        &call,
        "claims",
        r#"{"said":"answer"}"#,
        json!({"claims": [
            {"said": "booked the table for seven"},
            {"said": "cancelled the trip to Rome"},
            // The person's words are no answer.
            {"said": "book us a table"}
        ]}),
        Some("Done: I booked the table for seven at the corner place."),
    );
    let payload = the_checked_section(&sides, "claims");
    assert_eq!(
        payload,
        json!({"claims": [{"said": "booked the table for seven", "verified": ["said"]}]})
    );
    assert_eq!(
        reasons(&rejects),
        vec![("said".to_string(), "quote_not_in_source".to_string()); 2]
    );
}

#[test]
fn sidecar_verify_mark_mode_keeps_item_with_failed_and_still_rejects() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let call = h.curate(
        "s1",
        "t1",
        0,
        json!([user("I miss Werner's patient ways.")]),
        mode("Be brief."),
    );
    let (sides, rejects) = checked_section(
        &mut h,
        &call,
        "claims",
        r#"{"items":"claims","mode":"mark","quote":"turn","said":"answer"}"#,
        json!({"note": ["not the array"], "claims": [
            {"quote": "miss Werner's patient ways", "said": "Werner was patient",
             "verified": ["forged by the model"]},
            {"quote": "Werner hated waiting", "said": "something I never said"}
        ]}),
        Some("You miss how patient Werner was. Werner was patient with everyone."),
    );
    let payload = the_checked_section(&sides, "claims");
    assert_eq!(
        payload["note"],
        json!(["not the array"]),
        "`items` names the array"
    );
    assert_eq!(
        payload["claims"],
        json!([
            {"quote": "miss Werner's patient ways", "said": "Werner was patient",
             "verified": ["quote", "said"]},
            {"quote": "Werner hated waiting", "said": "something I never said",
             "verified": [],
             "failed": [{"field": "quote", "reason": "quote_not_in_source"},
                        {"field": "said", "reason": "quote_not_in_source"}]}
        ]),
        "the failed item stays, marked; a model's own `verified` is no proof"
    );
    assert_eq!(
        reasons(&rejects),
        vec![("quote".to_string(), "quote_not_in_source".to_string())],
        "the refusal goes out all the same, once per item, its first failed field"
    );
}

#[test]
fn sidecar_verify_peer_turn_counts_as_source() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let peer = json!({"origin": "peer", "type": "text",
                      "text": "I will bring the cake on Sunday.",
                      "speaker": "North", "speaker_ref": "dc365e79"});
    let call = h.curate(
        "s1",
        "t1",
        0,
        json!([user("Who brings dessert this time?"), peer]),
        mode("Be brief."),
    );
    let (sides, rejects) = checked_section(
        &mut h,
        &call,
        "claims",
        r#"{"quote":"turn"}"#,
        json!({"claims": [
            {"quote": "bring the cake on Sunday"},
            {"quote": "who brings dessert"},
            {"quote": "bring the pie on Sunday"}
        ]}),
        None,
    );
    let payload = the_checked_section(&sides, "claims");
    assert_eq!(
        payload["claims"],
        json!([{"quote": "bring the cake on Sunday", "verified": ["quote"]},
               {"quote": "who brings dessert", "verified": ["quote"]}]),
        "the round answers both: the person and the peer"
    );
    assert_eq!(
        reasons(&rejects),
        vec![("quote".to_string(), "quote_not_in_source".to_string())]
    );
}

//! The curator hive in one process, for the locks of wave A2 (GH #892 and the
//! strands that build on it). The same harness `curator_cells.rs` carries for
//! A1, written once here for every new lock instead of once per file (the
//! precedent `support/assemble_cell.rs` set): the shipped `script_inline`
//! programs run under python3 against real stdin documents, the edges are the
//! shipped `params.graph` of the hive evaluated by the colony's own CEL, the
//! ledger is an in-memory SQLite with the shipped schema and seed, and every
//! store operation runs through the store cell's own dispatcher and comes back
//! in its own reply shape. Only the clock and the summarizer are recorders the
//! test answers by hand.
#![allow(dead_code)]

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

pub fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// R2b / GH #49: a tree without the template skips.
pub fn shipped() -> bool {
    repo("templates/curator/config.json").is_file()
}

pub fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    sj::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

pub fn cell_config(name: &str) -> Value {
    read_json(&repo(&format!("templates/curator/{name}/config.json")))
}

pub fn script_of(name: &str) -> String {
    cell_config(name)["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string()
}

/// Run a program under python3, the program itself on stdin (a single argv
/// string is capped at 128 KiB).
pub fn run_python(script: &str, stdin_doc: &str) -> std::process::Output {
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

/// The loader the evaluation harnesses use (`annotate.py` `collector_scope`):
/// imports, defs and upper-case constants of the policy script, in file order,
/// under `params`; then `probe` -- a python expression over that scope -- is
/// printed as JSON. Its input is read first; after that stdin is empty, so a
/// definition that read it would fail the load.
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
scope["ARGS"] = inp.get("args")
print(json.dumps(eval(inp["probe"], scope)))
"#;

/// The pure half of the shipped policy, loaded with the shipped params
/// overlaid by `over`, and `probe` evaluated in it. Returns the value and what
/// the load said on stderr.
pub fn policy_scope(over: Value, probe: &str, args: Value) -> (Value, String) {
    let mut params = cell_config("policy")["params"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    params.remove("script_inline");
    for (k, v) in over.as_object().cloned().unwrap_or_default() {
        params.insert(k, v);
    }
    let doc = json!({"src": script_of("policy"), "params": params, "probe": probe,
                     "args": args});
    let out = run_python(AST_LOADER, &doc.to_string());
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "the pure half does not load: {err}");
    let got: Value = sj::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("not JSON ({e}): {}", String::from_utf8_lossy(&out.stdout)));
    (got, err)
}

#[derive(Clone, Debug, Default)]
pub struct Msg {
    pub context: Map<String, Value>,
    pub hop: Map<String, Value>,
    pub body: Map<String, Value>,
    pub reply_to: String,
}

impl Msg {
    pub fn route(&self) -> &str {
        self.hop.get("route").and_then(Value::as_str).unwrap_or("")
    }
    pub fn messages(&self) -> Vec<Value> {
        self.body
            .get("messages")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }
}

pub fn obj(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

/// The standard round of this harness (GH #925): the audience a lane message
/// carries when a test names none, and the audience of a row a test sows by
/// hand -- as the TEXT the colony carries it in.
pub const ROUND_E: &str = "[\"member:e\"]";

struct Edge {
    from: String,
    to: String,
    condition: Option<CompiledCondition>,
    modifier: Option<CompiledModifier>,
}

/// The cells of the shipped hive, `schemas` included since GH #892, `push`
/// since GH #895, `handover` since GH #896 (`./policy` hands the first
/// call of every new session through it) and `stats` since GH #926 (the
/// ledger's counts on `in_stats`).
pub const CELLS: [&str; 10] = [
    "intake",
    "policy",
    "writer",
    "ledger",
    "summarizer",
    "clock",
    "schemas",
    "push",
    "handover",
    "stats",
];

/// The curator hive in one process: its cells, its edges, its ledger.
pub struct Hive {
    edges: Vec<Edge>,
    cells: BTreeMap<String, Value>,
    pub db: rusqlite::Connection,
    pub out: Vec<Msg>,
    pub clock: Vec<Msg>,
    pub summ: VecDeque<Msg>,
    pub stderr: Vec<String>,
    /// Every ledger operation the hive ran, with the cell that sent it (the
    /// edge's `from`, never the cell the answer is addressed to): who writes
    /// which table is measured here, at the store, not read off a script.
    pub ledger_ops: Vec<(String, Value)>,
    /// A test that breaks the ledger on purpose sets this (GH #926,
    /// `store_error`): the store's refusal then travels on like any
    /// answer. Unset, a refused ledger op fails the test where it happens.
    pub ledger_may_refuse: bool,
}

impl Hive {
    pub fn new() -> Self {
        Self::with(&[])
    }

    /// The shipped hive with `(cell, param, value)` overrides.
    pub fn with(over: &[(&str, &str, Value)]) -> Self {
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
        for name in CELLS {
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
            ledger_ops: Vec::new(),
            ledger_may_refuse: false,
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
            if let Some(code) = r["error_code"].as_str()
                && !self.ledger_may_refuse
            {
                panic!("a ledger op failed ({code}): {r} in {:?}", msg.body);
            }
        }
        if let Some(code) = hop.get("error_code").and_then(Value::as_str)
            && !self.ledger_may_refuse
        {
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
                        let sender = from.trim_start_matches("./").to_string();
                        for c in m.messages().iter().filter(|c| c["type"] == "tool_call") {
                            let args = sj::from_str(c["text"].as_str().unwrap_or(""))
                                .unwrap_or(Value::Null);
                            self.ledger_ops.push((sender.clone(), args));
                        }
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
    pub fn lane(&mut self, route: &str, context: Value, hop: Value, body: Value) {
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
            reply_to: String::new(),
        };
        self.pump(".", msg);
    }

    /// The strike of an order the clock holds, as the timer emits it: a
    /// fresh root without a context, the row's `emit_headers` as hop keys and
    /// the timer's own headers over them (`timer::cell::build_fire_content`).
    /// The row under one id is the order armed LAST (`rearm` replaces it), so
    /// its `emit_headers` ride -- the round of the newest call (GH #925,
    /// OR-BD.A.6); an order the clock never saw brings its own.
    pub fn fire(&mut self, order: &Msg) {
        let id = order.body.get("schedule_id");
        let armed = self
            .clock
            .iter()
            .rev()
            .find(|m| m.body.get("op") == Some(&json!("add")) && m.body.get("schedule_id") == id)
            .unwrap_or(order);
        let mut hop = obj(armed.body.get("emit_headers").cloned().unwrap_or_default());
        for (k, v) in obj(json!({
            "event_id": "0190a3f2-0000-7000-8000-000000000892",
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
    pub fn answer(&mut self, text: &str, finish: &str) -> Msg {
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

    pub fn rows(&self, sql: &str) -> Vec<Vec<Value>> {
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

    pub fn state(&self, key: &str) -> String {
        self.rows(&format!("SELECT value FROM state WHERE key = '{key}'"))
            .first()
            .and_then(|r| r[0].as_str().map(str::to_string))
            .unwrap_or_default()
    }

    /// The current plan of the window (`state.window_plan`), `{}` before the
    /// first rebuild.
    pub fn plan(&self) -> Value {
        sj::from_str(&self.state("window_plan")).unwrap_or_else(|_| json!({}))
    }

    pub fn routed(&self, route: &str) -> Vec<Msg> {
        self.out
            .iter()
            .filter(|m| m.route() == route)
            .cloned()
            .collect()
    }

    /// One round on `in_curate`; returns the call that left for the model.
    pub fn curate(
        &mut self,
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
                         "channel": "test", "audience_set": ROUND_E});
        let mut body = json!({"messages": round});
        if system.is_object() {
            body["system"] = system;
        }
        self.lane("in_curate", ctx, hop, body);
        let calls = self.routed("brain");
        assert_eq!(
            calls.len(),
            1,
            "one round, one call: {:?} {:?}",
            self.out,
            self.stderr
        );
        calls[0].clone()
    }

    /// The model's output on the tap, with the context the parent edge promotes.
    pub fn tap(&mut self, call: &Msg, finish: &str, extra: Value, messages: Value) {
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

    /// A section of the model's block, as the parent's splitter hands it on:
    /// the answer's context, `hop.section`, the splitter's body.
    pub fn section(&mut self, call: &Msg, section: &str, payload: Value) {
        let mut ctx = call.context.clone();
        for k in ["curator_call", "turn_id", "session_id", "iter"] {
            ctx.insert(k.into(), call.hop[k].clone());
        }
        self.lane(
            "in_section",
            Value::Object(ctx),
            json!({"section": section}),
            json!({"messages": [], "section": section, "payload": payload}),
        );
    }

    /// A wall row written straight into the ledger, the way `./intake` writes
    /// one, at a stamp of the test's choosing -- a wall of yesterday cannot be
    /// grown through the lanes today -- under the standard round. Returns
    /// `(seq, hash)`.
    pub fn row(
        &mut self,
        at: chrono::DateTime<chrono::Utc>,
        session: &str,
        turn: &str,
        kind: &str,
        el: &Value,
        final_: i64,
    ) -> (i64, String) {
        self.row_under(Some(ROUND_E), at, session, turn, kind, el, final_)
    }

    /// [`Hive::row`] under the audience `aud` (GH #925): a JSON text, or
    /// `None` for a row from before the rule (column NULL).
    #[allow(clippy::too_many_arguments)]
    pub fn row_under(
        &mut self,
        aud: Option<&str>,
        at: chrono::DateTime<chrono::Utc>,
        session: &str,
        turn: &str,
        kind: &str,
        el: &Value,
        final_: i64,
    ) -> (i64, String) {
        let body = canonical(el);
        let hash = sha256_hex(&body);
        let seq = at.timestamp_micros();
        let stamp = at.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string();
        let chars = el["text"].as_str().unwrap_or("").chars().count() as i64;
        let have: i64 = self
            .db
            .query_row(
                "SELECT COUNT(*) FROM blocks WHERE hash = ?1",
                [&hash],
                |r| r.get(0),
            )
            .unwrap();
        if have == 0 {
            self.db
                .execute(
                    "INSERT INTO blocks (hash, kind, chars, body, first_seen) \
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![hash, kind, chars, body, stamp],
                )
                .unwrap();
        }
        self.db
            .execute(
                "INSERT INTO wall (seq, session_id, turn_id, iter, kind, hash, nth, final, \
                 episode_idx, at, audience_set) \
                 VALUES (?1, ?2, ?3, 0, ?4, ?5, 0, ?6, NULL, ?7, ?8)",
                rusqlite::params![seq, session, turn, kind, hash, final_, stamp, aud],
            )
            .unwrap();
        (seq, hash)
    }

    /// A `marks` row written straight into the ledger at a stamp of the
    /// test's choosing, under the standard round (GH #925).
    pub fn mark(
        &mut self,
        at: chrono::DateTime<chrono::Utc>,
        session: &str,
        turn: &str,
        kind: &str,
        value: &str,
    ) {
        self.db
            .execute(
                "INSERT INTO marks (seq, session_id, turn_id, kind, value, at, audience_set) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    at.timestamp_micros(),
                    session,
                    turn,
                    kind,
                    value,
                    at.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string(),
                    ROUND_E
                ],
            )
            .unwrap();
    }
}

/// The one serialisation a block is hashed in: keys sorted, UTF-8, no
/// whitespace -- `serde_json` without `preserve_order` (the workspace enables
/// it nowhere) writes an object's keys in order and nothing between them.
pub fn canonical(v: &Value) -> String {
    sj::to_string(v).expect("serialise")
}

pub fn sha256_hex(s: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The short id of an element: the first 12 hex digits of its block hash.
pub fn short_id(el: &Value) -> String {
    sha256_hex(&canonical(el))[..12].to_string()
}

pub fn user(text: &str) -> Value {
    json!({"origin": "user", "type": "text", "text": text})
}

pub fn said(text: &str) -> Value {
    json!({"origin": "assistant", "type": "text", "text": text})
}

pub fn tool_call(id: &str, name: &str) -> Value {
    json!({"origin": "assistant", "type": "tool_call", "id": id,
           "text": json!({"name": name, "arguments": "{}"}).to_string()})
}

pub fn tool_result(id: &str, text: &str) -> Value {
    json!({"origin": "tool", "type": "tool_result", "id": id, "text": text})
}

pub fn mode(text: &str) -> Value {
    json!({"instructions": {"mode": {"text": text}}})
}

pub fn texts(call: &Msg) -> Vec<String> {
    call.messages()
        .iter()
        .map(|m| m["text"].as_str().unwrap_or("").to_string())
        .collect()
}

/// A whole turn: the round in, the final answer back on the tap.
pub fn turn(
    h: &mut Hive,
    session: &str,
    turn_id: &str,
    ask: &str,
    reply: &str,
    extra: Value,
) -> Msg {
    let call = h.curate(session, turn_id, 0, json!([user(ask)]), mode("Be brief."));
    h.tap(&call, "stop", extra, json!([said(reply)]));
    call
}

pub fn last_add(h: &Hive) -> Msg {
    h.clock
        .iter()
        .rev()
        .find(|m| m.body["op"] == "add")
        .cloned()
        .expect("an add order")
}

/// What the ledger says call `id` carried, messages only, in `call_blocks`
/// order: the rebuild of the request (gh888) at the level of the elements.
pub fn listed_messages(h: &Hive, call: &Msg) -> Vec<Value> {
    let id = call.hop["curator_call"].as_str().expect("a call id");
    h.rows(&format!(
        "SELECT b.kind, b.body FROM call_blocks cb JOIN blocks b ON b.hash = cb.hash \
         WHERE cb.call_id = '{id}' ORDER BY cb.pos"
    ))
    .into_iter()
    .filter(|r| r[0] != "system")
    .map(|r| sj::from_str(r[1].as_str().unwrap()).unwrap())
    .collect()
}

/// A stamp `days` UTC calendar days before now, at `hour`:00.
pub fn days_ago(days: i64, hour: u32) -> chrono::DateTime<chrono::Utc> {
    use chrono::TimeZone;
    let d = (chrono::Utc::now() - chrono::Duration::days(days)).date_naive();
    chrono::Utc.from_utc_datetime(&d.and_hms_opt(hour, 0, 0).unwrap())
}

/// Seconds until the next midnight UTC.
pub fn secs_to_midnight() -> i64 {
    use chrono::Timelike;
    86_400 - i64::from(chrono::Utc::now().num_seconds_from_midnight())
}

/// A test that places rows by calendar day must not straddle midnight UTC:
/// the plan's day is read when the rebuild runs. Within `margin_s` of
/// midnight, wait until it has passed -- a wait for the clock, not for a
/// result, and at most `margin_s` + 1 seconds once a day.
pub fn clear_of_midnight(margin_s: i64) {
    let left = secs_to_midnight();
    if left <= margin_s {
        std::thread::sleep(std::time::Duration::from_secs((left + 1) as u64));
    }
}

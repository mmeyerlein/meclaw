//! An objects hive in one process, for the locks of GH #951. The pattern of
//! `support/file_space_hive.rs`: the shipped `script_inline` programs of
//! `templates/objects/*` run under python3 against real stdin documents, the
//! edges are the shipped `params.graph` of the hive evaluated by the colony's
//! own CEL, and the store is an in-memory SQLite with the shipped schema AND
//! indexes (the unique (id, rev) index is the gate's compare-and-set), every
//! store operation answered by the store cell's own dispatcher in its own
//! reply shape.
//!
//! Unit-harness duty of the wave: every message a code cell emits is held to
//! that cell's `contract.emits` -- each `hop` and body key it sets is declared
//! there with a matching type, every required key is present, and a key with
//! `values` (the route above all) carries one of them. A cell that emits a
//! route or a key it does not declare fails here, not in a colony.
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

pub const TEMPLATE: &str = "templates/objects";

/// The context keys the hive sets for itself; none may leave it (gh494).
pub const INTERNAL: [&str; 3] = ["cur_origin", "cur_phase", "cur_call"];

pub fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// R2b / GH #49: a tree without the template skips.
pub fn shipped() -> bool {
    repo(&format!("{TEMPLATE}/config.json")).is_file()
}

pub fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    sj::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

pub fn hive_config() -> Value {
    read_json(&repo(&format!("{TEMPLATE}/config.json")))
}

pub fn cell_config(name: &str) -> Value {
    read_json(&repo(&format!("{TEMPLATE}/{name}/config.json")))
}

pub fn script_of(name: &str) -> String {
    cell_config(name)["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string()
}

/// The text of `script_of(cell)` between `# --8<-- <name>` and
/// `# --8<-- end <name>`: a block a code cell carries word for word beside
/// another one, because code cells share no library.
pub fn block(cell: &str, name: &str) -> String {
    let s = script_of(cell);
    let open = format!("# --8<-- {name}\n");
    let close = format!("# --8<-- end {name}");
    let a = s
        .find(&open)
        .unwrap_or_else(|| panic!("{cell}: no block {name}"));
    let b = s[a..]
        .find(&close)
        .unwrap_or_else(|| panic!("{cell}: block {name} is not closed"));
    s[a..a + b].to_string()
}

/// Every cell of the shipped hive: `(name, cell.type)`, in name order.
pub fn cells() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(repo(TEMPLATE)).expect("the template directory") {
        let p = e.expect("an entry").path();
        let cfg = p.join("config.json");
        if p.is_dir() && cfg.is_file() {
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            let ty = read_json(&cfg)["cell"]["type"]
                .as_str()
                .unwrap_or("")
                .to_string();
            out.push((name, ty));
        }
    }
    out.sort();
    out
}

/// Run a program under python3, the program itself on stdin.
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

/// The loader of the pure half of a script (`file_space_hive.rs`): imports,
/// defs and upper-case constants, in file order, under `params`; then `probe`
/// -- a python expression over that scope, `ARGS` the test's argument -- is
/// printed as JSON.
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
    exec(compile(ast.Module(body=[n], type_ignores=[]), "cell", "exec"), scope)
scope["ARGS"] = inp.get("args")
print(json.dumps(eval(inp["probe"], scope)))
"#;

/// The pure half of the shipped script of `cell`, and `probe` evaluated in it.
pub fn pure(cell: &str, probe: &str, args: Value) -> Value {
    let mut params = cell_config(cell)["params"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    params.remove("script_inline");
    let doc = json!({"src": script_of(cell), "params": params, "probe": probe, "args": args});
    let out = run_python(AST_LOADER, &doc.to_string());
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        out.status.success(),
        "{cell}: the pure half does not load: {err}"
    );
    sj::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{cell}: not JSON ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

#[derive(Clone, Debug, Default)]
pub struct Msg {
    pub context: Map<String, Value>,
    pub hop: Map<String, Value>,
    pub body: Map<String, Value>,
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
    /// The body without its `messages`, as one JSON object.
    pub fn fields(&self) -> Value {
        let mut b = self.body.clone();
        b.remove("messages");
        Value::Object(b)
    }
}

pub fn obj(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

fn type_holds(ty: &str, v: &Value) -> bool {
    match ty {
        "string" => v.is_string(),
        "number" | "integer" => v.is_number(),
        "boolean" => v.is_boolean(),
        "array" => v.is_array(),
        "object" => v.is_object(),
        _ => true,
    }
}

/// One emission of `cell` held to its `contract.emits` (the wave's
/// unit-harness duty): every key declared, of its type, within its `values`,
/// every required key present -- for the hop and for the body alike.
pub fn check_emission(
    cell: &str,
    emits: &Value,
    hop: &Map<String, Value>,
    body: &Map<String, Value>,
) {
    for (part, got) in [("hop", hop), ("body", body)] {
        let decl = emits[part].as_object().cloned().unwrap_or_default();
        for (k, spec) in &decl {
            if spec["required"] == json!(true) {
                assert!(
                    got.contains_key(k),
                    "{cell}: {part}.{k} is required by contract.emits and missing: {got:?}"
                );
            }
        }
        for (k, v) in got {
            let spec = decl.get(k).unwrap_or_else(|| {
                panic!("{cell}: emits {part}.{k} = {v}, which contract.emits does not declare")
            });
            let ty = spec["type"].as_str().unwrap_or("");
            assert!(
                type_holds(ty, v),
                "{cell}: {part}.{k} = {v} is not of the declared type {ty}"
            );
            if let Some(values) = spec["values"].as_array() {
                assert!(
                    values.contains(v),
                    "{cell}: {part}.{k} = {v} is not one of the declared values {values:?}"
                );
            }
        }
    }
}

struct Edge {
    from: String,
    to: String,
    condition: Option<CompiledCondition>,
    modifier: Option<CompiledModifier>,
}

/// One objects hive: its cells, its edges, its store.
pub struct Hive {
    edges: Vec<Edge>,
    cells: BTreeMap<String, Value>,
    pub db: rusqlite::Connection,
    /// Everything that left through the hive path, oldest first.
    pub out: Vec<Msg>,
    pub stderr: Vec<String>,
    /// Every store operation, with the cell that sent it.
    pub store_ops: Vec<(String, Value)>,
    /// Every store operation that failed, with its code.
    pub store_errors: Vec<String>,
    /// Every message a code cell emitted: `(cell, message)`, oldest first.
    pub emitted: Vec<(String, Msg)>,
    seq: u64,
}

impl Hive {
    pub fn new() -> Self {
        Self::with(&[])
    }

    /// The shipped hive with `(cell, param, value)` overrides.
    pub fn with(over: &[(&str, &str, Value)]) -> Self {
        let hive = hive_config();
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
        for (name, _) in self::cells() {
            cells.insert(name.clone(), cell_config(&name));
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
        meclaw_cells::store::query::install_connection_extensions(&db).expect("extensions");
        let schema: BTreeMap<String, BTreeMap<String, String>> =
            sj::from_value(cells["store"]["params"]["schema"].clone()).expect("schema");
        meclaw_cells::store::ddl::apply_schema_ddl(&db, &schema).expect("ddl");
        let mut indexes = BTreeMap::new();
        for (name, spec) in cells["store"]["params"]["indexes"]
            .as_object()
            .expect("indexes")
        {
            indexes.insert(
                name.clone(),
                meclaw_cells::store::IndexSpec {
                    table: spec["table"].as_str().expect("table").to_string(),
                    on: spec["on"]
                        .as_array()
                        .expect("on")
                        .iter()
                        .map(|k| k.as_str().expect("a key").to_string())
                        .collect(),
                    unique: spec["unique"] == json!(true),
                },
            );
        }
        meclaw_cells::store::ddl::apply_index_ddl(&db, &indexes).expect("index ddl");
        Self {
            edges,
            cells,
            db,
            out: Vec::new(),
            stderr: Vec::new(),
            store_ops: Vec::new(),
            store_errors: Vec::new(),
            emitted: Vec::new(),
            seq: 0,
        }
    }

    fn cell_type(&self, name: &str) -> String {
        self.cells
            .get(name)
            .and_then(|c| c["cell"]["type"].as_str())
            .unwrap_or("")
            .to_string()
    }

    fn run_cell(&mut self, name: &str, msg: &Msg) -> Vec<Msg> {
        let cfg = &self.cells[name];
        let script = cfg["params"]["script_inline"].as_str().expect("script");
        let emits = cfg["contract"]["emits"].clone();
        let mut params = obj(cfg["params"].clone());
        params.remove("script_inline");
        let doc = json!({
            "envelope": {"header": {"context": msg.context, "hop": msg.hop},
                         "target": format!("/m/objects/{name}"),
                         "reply_to": ""},
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
        let mut msgs = Vec::new();
        for m in list {
            let mut body = obj(m);
            let hop = body
                .remove("header")
                .and_then(|h| h.as_object().cloned())
                .unwrap_or_default();
            // A real colony's debug build dead-letters a body that is no UBF
            // body (`invalid_ubf_body`); the harness holds every emission to
            // the same schema, and to the cell's own contract.
            if let Err(e) = meclaw_core::validate_ubf_body(&Value::Object(body.clone())) {
                panic!("{name} emitted no UBF body ({e}): {}", Value::Object(body));
            }
            check_emission(name, &emits, &hop, &body);
            let m = Msg {
                context: msg.context.clone(),
                hop,
                body,
            };
            self.emitted.push((name.to_string(), m.clone()));
            msgs.push(m);
        }
        msgs
    }

    /// One store message answered the way the store cell answers it.
    fn store(&mut self, sender: &str, msg: &Msg) -> Msg {
        use meclaw_cells::store::ops::dispatch;
        use meclaw_cells::store::output::{BundleLeg, build_bundle_result, build_tool_result};
        let calls: Vec<Value> = msg
            .messages()
            .into_iter()
            .filter(|m| m["type"] == "tool_call")
            .collect();
        for c in &calls {
            let args = sj::from_str(c["text"].as_str().unwrap_or("")).unwrap_or(Value::Null);
            self.store_ops.push((sender.to_string(), args));
        }
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
                self.store_errors.push(format!("{sender}: {code}: {r}"));
            }
        }
        if let Some(code) = hop.get("error_code").and_then(Value::as_str) {
            self.store_errors.push(format!("{sender}: {code}: {body}"));
        }
        Msg {
            context: msg.context.clone(),
            hop,
            body: obj(body),
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

    /// Carry `msg`, emitted by `from`, through the hive until it comes to rest.
    pub fn pump(&mut self, from: &str, msg: Msg) {
        let mut queue = VecDeque::from(vec![(from.to_string(), msg)]);
        let mut steps = 0;
        while let Some((from, msg)) = queue.pop_front() {
            steps += 1;
            assert!(steps < 2000, "the hive does not come to rest");
            for (to, m) in self.route(&from, &msg) {
                if to == "." {
                    for k in INTERNAL {
                        assert!(
                            !m.context.contains_key(k),
                            "`{k}` leaves the hive on {} (gh494): {:?}",
                            m.route(),
                            m.context
                        );
                    }
                    self.out.push(m);
                    continue;
                }
                let name = to.trim_start_matches("./").to_string();
                match self.cell_type(&name).as_str() {
                    "store" => {
                        let sender = from.trim_start_matches("./").to_string();
                        let answer = self.store(&sender, &m);
                        queue.push_back((to.clone(), answer));
                    }
                    "code" => {
                        for o in self.run_cell(&name, &m) {
                            queue.push_back((to.clone(), o));
                        }
                    }
                    other => panic!("an edge onto {to} of type {other:?}"),
                }
            }
        }
    }

    /// A message arriving on the hive path on lane `route`.
    pub fn lane(&mut self, route: &str, context: Value, hop: Value, body: Value) {
        let mut hop = obj(hop);
        hop.insert("route".into(), json!(route));
        let mut body = obj(body);
        body.entry("messages").or_insert(json!([]));
        self.pump(
            ".",
            Msg {
                context: obj(context),
                hop,
                body,
            },
        );
    }

    /// A sighting of `names` in turn `turn`, the round in
    /// `context.audience_set` (`None`: no round at all).
    pub fn thing_seen(&mut self, round: Option<&str>, turn: &str, names: &[&str]) {
        let ctx = match round {
            Some(r) => json!({"audience_set": r}),
            None => json!({}),
        };
        let items: Vec<Value> = names.iter().map(|n| json!({"name": n})).collect();
        self.lane(
            "thing_seen",
            ctx,
            json!({}),
            json!({"items": items, "turn_id": turn, "session_id": "s1"}),
        );
    }

    /// The context of a tool call as the member's tool edge leaves it: the
    /// round in `audience_now` and `audience_set`, the speaker when there is one.
    pub fn tool_context(round: Option<&str>, speaker: Option<&str>) -> Value {
        let mut ctx = json!({"tool_caller": "talky"});
        if let Some(r) = round {
            ctx["audience_now"] = json!(r);
            ctx["audience_set"] = json!(r);
        }
        if let Some(s) = speaker {
            ctx["speaker"] = json!(s);
        }
        ctx
    }

    /// One tool call under `context`; the ONE `tool_result` that left the hive
    /// for it: (hop, the result text, byte for byte).
    pub fn tool_raw(&mut self, name: &str, args: Value, context: Value) -> (Value, String) {
        self.seq += 1;
        let id = format!("c{}", self.seq);
        let before = self.out.len();
        self.lane(
            "in_tool",
            context.clone(),
            json!({"tool_name": name, "tool_call_id": id}),
            json!({"messages": [{"origin": "assistant", "type": "tool_call", "id": id,
                                 "text": args.to_string()}]}),
        );
        let mine: Vec<Msg> = self.out[before..]
            .iter()
            .filter(|m| m.route() == "tool_result")
            .cloned()
            .collect();
        assert_eq!(
            mine.len(),
            1,
            "{name}: exactly one tool_result, got {mine:?}; stderr {:?}",
            self.stderr
        );
        let m = &mine[0];
        assert_eq!(
            m.hop["tool_call_id"],
            json!(id),
            "{name}: under the call's id"
        );
        assert_eq!(
            m.context.get("tool_caller"),
            context.get("tool_caller"),
            "the assistant's stamp survives the hive"
        );
        let msgs = m.messages();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["id"], json!(id));
        let mut hop = m.hop.clone();
        hop.remove("tool_call_id");
        (
            Value::Object(hop),
            msgs[0]["text"].as_str().expect("a text").to_string(),
        )
    }

    /// [`Hive::tool_raw`] with the result parsed.
    pub fn tool_ctx(&mut self, name: &str, args: Value, context: Value) -> (Value, Value) {
        let (hop, text) = self.tool_raw(name, args, context);
        let v: Value = sj::from_str(&text).expect("the result is JSON");
        (hop, v)
    }

    /// One tool call in `round` by `speaker`: (hop, the parsed result).
    pub fn tool(
        &mut self,
        name: &str,
        args: Value,
        round: Option<&str>,
        speaker: Option<&str>,
    ) -> (Value, Value) {
        self.tool_ctx(name, args, Self::tool_context(round, speaker))
    }

    /// A graph space's pull of `op` for `source@version` on `in_read`; the ONE
    /// `answer` that left for it: (hop, body without `messages`).
    pub fn read(&mut self, op: &str, source: &str, version: &str) -> (Value, Value) {
        let op_id = format!("gs:{op}:{source}:{version}");
        let before = self.out.len();
        self.lane(
            "in_read",
            json!({}),
            json!({"op": op, "op_id": op_id, "source": source}),
            json!({"op": op, "file": format!("{source}@{version}"), "source": source,
                   "version": version, "args": {"version": version, "limit": 100}}),
        );
        let mine: Vec<Msg> = self.out[before..].to_vec();
        assert_eq!(mine.len(), 1, "{op}: exactly one answer, got {mine:?}");
        let m = &mine[0];
        assert_eq!(m.route(), "answer");
        assert_eq!(m.hop["op"], json!(op), "the answer mirrors op");
        assert_eq!(m.hop["op_id"], json!(op_id), "the answer mirrors op_id");
        assert_eq!(m.hop["source"], json!(source), "the answer mirrors source");
        (Value::Object(m.hop.clone()), m.fields())
    }

    pub fn routed(&self, route: &str) -> Vec<Msg> {
        self.out
            .iter()
            .filter(|m| m.route() == route)
            .cloned()
            .collect()
    }

    /// The rows of `sql`, each as an object keyed by column name.
    pub fn query(&self, sql: &str) -> Vec<Value> {
        use rusqlite::types::ValueRef;
        let mut st = self.db.prepare(sql).expect(sql);
        let names: Vec<String> = st.column_names().iter().map(|s| s.to_string()).collect();
        st.query_map([], |r| {
            let mut o = Map::new();
            for (i, n) in names.iter().enumerate() {
                let v = match r.get_ref(i).unwrap() {
                    ValueRef::Null => Value::Null,
                    ValueRef::Integer(x) => json!(x),
                    ValueRef::Real(x) => json!(x),
                    ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                    ValueRef::Blob(_) => Value::Null,
                };
                o.insert(n.clone(), v);
            }
            Ok(Value::Object(o))
        })
        .expect(sql)
        .map(Result::unwrap)
        .collect()
    }

    pub fn count(&self, table: &str) -> i64 {
        self.db
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .expect("count")
    }

    /// The head (highest `rev`) of every object, oldest first.
    pub fn heads(&self) -> Vec<Value> {
        self.query(
            "SELECT * FROM objects o WHERE rev = (SELECT MAX(rev) FROM objects x \
             WHERE x.id = o.id) ORDER BY valid_since, id",
        )
    }

    pub fn head(&self, id: &str) -> Value {
        self.heads()
            .into_iter()
            .find(|r| r["id"] == json!(id))
            .unwrap_or_else(|| panic!("no row {id}"))
    }

    /// The one head whose round is `round`.
    pub fn head_in(&self, round: &str) -> Value {
        let mine: Vec<Value> = self
            .heads()
            .into_iter()
            .filter(|r| r["audience_set"] == json!(round))
            .collect();
        assert_eq!(mine.len(), 1, "one row in {round}: {mine:?}");
        mine[0].clone()
    }
}

/// A JSON column of a row, parsed.
pub fn col(row: &Value, name: &str) -> Value {
    sj::from_str(row[name].as_str().unwrap_or("null")).unwrap_or(Value::Null)
}

/// `ob-<12 hex>`.
pub fn is_object_id(s: &str) -> bool {
    s.len() == 15
        && s.starts_with("ob-")
        && s[3..]
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// A memory answer about `subject` in the shape memory's `in_query {subject}`
/// answers with (GH #948): the facts as `candidates[]` of the bundle JSON in
/// `system.memory.bundle.text`, in the order memory hands them -- newest
/// first. A fact here carries no predicate (recall leaves out every key it
/// does not know); `facts_bundle` builds the JSON for a probe of the parser.
pub fn facts_answer(subject: &str, texts: &[&str]) -> Value {
    json!({
        "system": {"memory": {"bundle": {"text": facts_bundle(subject, texts).to_string()}}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "recall",
                      "text": format!("WHAT THIS MEMORY HOLDS ABOUT {subject}")}]
    })
}

/// The bundle JSON of `facts_answer`, as a value.
pub fn facts_bundle(subject: &str, texts: &[&str]) -> Value {
    let list: Vec<Value> = texts
        .iter()
        .map(|t| json!({"kind": "fact", "text": t, "subject": subject}))
        .collect();
    json!({"subject": subject, "as_of": "2026-10-02T10:00:00.000000Z",
           "answers": if list.is_empty() { "none" } else { "direct" },
           "candidates": list, "complete": true})
}

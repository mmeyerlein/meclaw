//! A file space in one process, for the locks of wave File Hive B1 (GH #899
//! and the strands that build on it: #900, #901, #902, #903). Written once
//! here, the precedent `support/curator_hive.rs` set: the shipped
//! `script_inline` programs of `templates/file-space/*` run under python3
//! against real stdin documents, the edges are the shipped `params.graph` of
//! the hive evaluated by the colony's own CEL, the store is an in-memory
//! SQLite with the shipped schema, and every store operation runs through the
//! store cell's own dispatcher and comes back in its own reply shape. An
//! `llm` cell is a recorder the test answers by hand.
//!
//! The cells are read off the template directory, not listed here: a strand
//! that adds its cell (`write`, `guard`, `embed`, `summarizer`) is run by this
//! harness without touching it. A new kind of seed is an orchestrator ruling,
//! not an edit by a strand.
//!
//! Seeding writes rows straight into the store, the way the cells would
//! leave them (README § 2.3 of the wave). Blocks are cut wherever the test
//! says (OR-FH-S1): reading never depends on the cut, and the cut rule of
//! § 2.4 belongs to `./write` alone.
//!
//! Blobs (GH #907): `Space::blobs` stands in for the colony's blob store. A
//! code cell whose contract declares `consumes.body.attachments` gets, on its
//! stdin document only, every object entry of `body.attachments` plus
//! `data_b64` or `error {code, message}` -- what the colony's
//! `AttachmentReader` hands a code cell. `Space::sent` records every message
//! the space delivers, so a lock can scan hops, contexts and store operations.
//!
//! The routing budget (GH #929, GH #947 review I-1): every delivery spends one
//! routing decision of the message's ttl, a `restore_ttl` edge hands the
//! colony default back first (`restore_edge_ttl`, `colony.rs`), a cell's
//! emission and the store's answer carry the ttl they were handled under, and
//! an `llm` answer the one its request arrived with. A message on a lane
//! arrives behind the space's door. Only recorded, never enforced: a lock
//! reads `worst_segment` and `ttl_dead`.
#![allow(dead_code)]

use meclaw_colony::cel_eval::{
    CompiledCondition, CompiledModifier, apply_modifier, evaluate_condition, parse_condition,
    parse_modifier,
};
use meclaw_colony::config::ModifierSpec;
use meclaw_core::Headers;
use meclaw_core::serde_json::{self as sj, Map, Value, json};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub const TEMPLATE: &str = "templates/file-space";

/// The colony's routing budget (`colony.json message_default_ttl`).
pub const TTL: i64 = meclaw_core::MESSAGE_DEFAULT_TTL as i64;

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

/// Every cell of the shipped space: `(name, cell.type)`, in name order.
pub fn cells() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let dir = repo(TEMPLATE);
    for e in std::fs::read_dir(&dir).expect("the template directory") {
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

/// The code cells of the space whose script defines `def <func>(`.
pub fn cells_defining(func: &str) -> Vec<String> {
    let needle = format!("\ndef {func}(");
    cells()
        .into_iter()
        .filter(|(_, ty)| ty == "code")
        .filter(|(n, _)| script_of(n).contains(&needle))
        .map(|(n, _)| n)
        .collect()
}

#[path = "warm_python.rs"]
mod warm_python;

/// Run a program under python3: in the substrate's warm harness, one child
/// per script (GH #1048, `warm_python.rs` says why and what it measured); a
/// document that is not one line runs cold, the program itself on stdin (a
/// single argv string is capped at 128 KiB).
pub fn run_python(script: &str, stdin_doc: &str) -> std::process::Output {
    if let Some(out) = warm_python::run(script, stdin_doc) {
        return out;
    }
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

/// The loader of the pure half of a script (`curator_hive.rs` `policy_scope`):
/// imports, defs and upper-case constants, in file order, under `params`;
/// then `probe` -- a python expression over that scope, `ARGS` the test's
/// argument -- is printed as JSON.
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
}

pub fn obj(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

struct Edge {
    from: String,
    to: String,
    condition: Option<CompiledCondition>,
    modifier: Option<CompiledModifier>,
    /// `modifier.restore_ttl`: the follow-up gets the colony budget back.
    restore: bool,
}

/// The one read of `node_runs` across files: `./read`'s `near` scan, which
/// compares the file vector of every head and nothing else (GH #944) -- a
/// `select` of `file, fvec` where `fvec <> ''`. Every other read of the table
/// names its file.
pub fn is_near_scan(sender: &str, a: &Value) -> bool {
    sender == "read"
        && a["operation"] == "select"
        && a["table"] == "node_runs"
        && a["columns"] == json!(["file", "fvec"])
        && a["where"] == json!({"fvec": {"neq": ""}})
}

/// One file space: its cells, its edges, its store.
pub struct Space {
    /// The hive path the space stands under (only the cells' envelopes see it).
    pub path: String,
    edges: Vec<Edge>,
    cells: BTreeMap<String, Value>,
    pub db: rusqlite::Connection,
    /// Everything that left through the hive path.
    pub out: Vec<Msg>,
    /// What reached an `llm` cell: `(cell, message)`, oldest first.
    pub llm: VecDeque<(String, Msg)>,
    pub stderr: Vec<String>,
    /// Every store operation, with the cell that sent it (the edge's `from`).
    pub store_ops: Vec<(String, Value)>,
    /// Every store operation that failed, with its code.
    pub store_errors: Vec<String>,
    /// One-shot SQL run on the store right before `cell` next receives a
    /// message on lane `route` -- a writer outside the space, arriving
    /// between two phases (GH #907: a path taken after it was searched).
    pub before: Option<(String, String, String)>,
    /// The colony's blob store: blob id -> bytes (GH #907).
    pub blobs: HashMap<String, Vec<u8>>,
    /// Every message the space delivered, oldest first:
    /// `{from, to, route, header: {context, hop}, body}` (GH #907).
    pub sent: Vec<Value>,
    /// The most routing decisions one delivery had spent since its last seam
    /// (a restoring edge, or the lane it came in on), and where:
    /// `(used, "from -> to route")`.
    pub worst_segment: (i64, String),
    /// Every delivery a colony would have dead-lettered as `ttl_expired`.
    pub ttl_dead: Vec<String>,
    /// The ttl each message in [`Space::llm`] arrived with, in its order.
    llm_ttl: VecDeque<i64>,
    seq: u64,
}

impl Space {
    pub fn new() -> Self {
        Self::with("/x/files", &[])
    }

    /// The shipped space under `path`.
    pub fn at(path: &str) -> Self {
        Self::with(path, &[])
    }

    /// The shipped space under `path` with `(cell, param, value)` overrides.
    pub fn with(path: &str, over: &[(&str, &str, Value)]) -> Self {
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
                restore: e["modifier"]["restore_ttl"] == json!(true),
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
        // As the store factory does for every connection: `hamming` (`similar`),
        // `meclaw_norm` and the FTS tokenizer (GH #903 ranks by `similar`).
        meclaw_cells::store::query::install_connection_extensions(&db).expect("extensions");
        let schema: BTreeMap<String, BTreeMap<String, String>> =
            sj::from_value(cells["store"]["params"]["schema"].clone()).expect("schema");
        meclaw_cells::store::ddl::apply_schema_ddl(&db, &schema).expect("ddl");
        Self {
            path: path.to_string(),
            edges,
            cells,
            db,
            out: Vec::new(),
            llm: VecDeque::new(),
            stderr: Vec::new(),
            store_ops: Vec::new(),
            store_errors: Vec::new(),
            before: None,
            blobs: HashMap::new(),
            sent: Vec::new(),
            worst_segment: (0, String::new()),
            ttl_dead: Vec::new(),
            llm_ttl: VecDeque::new(),
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
        let mut params = obj(cfg["params"].clone());
        params.remove("script_inline");
        let reads = cfg["contract"]["consumes"]["body"]
            .get("attachments")
            .is_some();
        let body = if reads {
            self.with_blobs(&msg.body)
        } else {
            msg.body.clone()
        };
        let doc = json!({
            "envelope": {"header": {"context": msg.context, "hop": msg.hop},
                         "target": format!("{}/{name}", self.path),
                         "reply_to": ""},
            "body": body,
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
                // A real colony's debug build dead-letters a body that is no
                // UBF body (`invalid_ubf_body`, `colony.rs` emission check):
                // the first colony lock of the file space (gh908) lost the
                // `in_check` of `write` there, with no answer. The harness
                // holds every emission to the same schema.
                let mut ubf = body.clone();
                ubf.remove("header");
                if let Err(e) = meclaw_core::validate_ubf_body(&Value::Object(ubf.clone())) {
                    panic!("{name} emitted no UBF body ({e}): {}", Value::Object(ubf));
                }
                let hop = body
                    .remove("header")
                    .and_then(|h| h.as_object().cloned())
                    .unwrap_or_default();
                Msg {
                    context: msg.context.clone(),
                    hop,
                    body,
                }
            })
            .collect()
    }

    /// `body` with every object entry of `attachments` read from
    /// [`Space::blobs`]: `data_b64` (standard base64, padded), or `error`
    /// `bad_ref` (no `blob_id`, or not a UUID) / `not_found`. The stdin
    /// document only; the message itself stays as it was.
    fn with_blobs(&self, body: &Map<String, Value>) -> Map<String, Value> {
        let mut out = body.clone();
        let Some(list) = body.get("attachments").and_then(Value::as_array) else {
            return out;
        };
        let read: Vec<Value> = list
            .iter()
            .map(|a| {
                let Some(entry) = a.as_object() else {
                    return a.clone();
                };
                let mut e = entry.clone();
                let id = entry.get("blob_id").and_then(Value::as_str).unwrap_or("");
                if meclaw_core::Uuid::parse_str(id).is_err() {
                    e.insert(
                        "error".into(),
                        json!({"code": "bad_ref", "message": "blob_id is missing or not a UUID"}),
                    );
                } else if let Some(bytes) = self.blobs.get(id) {
                    e.insert("data_b64".into(), json!(b64(bytes)));
                } else {
                    e.insert(
                        "error".into(),
                        json!({"code": "not_found", "message": format!("no blob {id}")}),
                    );
                }
                Value::Object(e)
            })
            .collect();
        out.insert("attachments".into(), Value::Array(read));
        out
    }

    /// One store message answered the way the store cell answers it. A failed
    /// operation is answered, not panicked on, and recorded in `store_errors`.
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

    fn route(&self, from: &str, msg: &Msg) -> Vec<(String, Msg, bool)> {
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
            out.push((e.to.clone(), m, e.restore));
        }
        out
    }

    /// Carry `msg`, emitted by `from`, through the hive until it comes to rest.
    pub fn pump(&mut self, from: &str, msg: Msg) {
        self.pump_all(vec![(from.to_string(), msg)]);
    }

    /// Carry several messages at once: all of them are queued before the
    /// first is handled, as a cell's mailbox holds a batch (GH #907).
    pub fn pump_all(&mut self, msgs: Vec<(String, Msg)>) {
        self.carry(msgs.into_iter().map(|(f, m)| (f, m, TTL)).collect());
    }

    /// [`Space::pump_all`] with the ttl each message holds as it is emitted.
    fn carry(&mut self, msgs: Vec<(String, Msg, i64)>) {
        let mut queue = VecDeque::from(msgs);
        let mut steps = 0;
        while let Some((from, msg, ttl)) = queue.pop_front() {
            steps += 1;
            assert!(steps < 4000, "the space does not come to rest");
            for (to, m, restore) in self.route(&from, &msg) {
                // As the colony routes (GH #82, GH #929): a restoring edge
                // lifts the budget to the default, the delivery spends one.
                let held = if restore { ttl.max(TTL) } else { ttl };
                let at = format!("{from} -> {to} {}", m.route());
                if held <= 0 {
                    self.ttl_dead.push(at.clone());
                }
                let arrived = held - 1;
                if TTL - arrived > self.worst_segment.0 {
                    self.worst_segment = (TTL - arrived, at);
                }
                self.sent.push(json!({
                    "from": from,
                    "to": to,
                    "route": m.route(),
                    "header": {"context": m.context, "hop": m.hop},
                    "body": m.body,
                }));
                if to == "." {
                    self.out.push(m);
                    continue;
                }
                let name = to.trim_start_matches("./").to_string();
                match self.cell_type(&name).as_str() {
                    "store" => {
                        let sender = from.trim_start_matches("./").to_string();
                        let answer = self.store(&sender, &m);
                        queue.push_back((to.clone(), answer, arrived));
                    }
                    "code" => {
                        let hit = matches!(&self.before,
                            Some((c, r, _)) if *c == name && r == m.route());
                        if hit {
                            let (_, _, sql) = self.before.take().unwrap();
                            self.db.execute_batch(&sql).expect("the one-shot SQL");
                        }
                        for o in self.run_cell(&name, &m) {
                            queue.push_back((to.clone(), o, arrived));
                        }
                    }
                    "llm" => {
                        self.llm.push_back((name, m));
                        self.llm_ttl.push_back(arrived);
                    }
                    other => panic!("an edge onto {to} of type {other:?}"),
                }
            }
        }
    }

    /// A message arriving on the hive path on lane `route`.
    pub fn lane(&mut self, route: &str, context: Value, hop: Value, body: Value) {
        let msg = Self::on_lane(route, context, hop, body);
        self.pump(".", msg);
    }

    /// The message [`Space::lane`] carries, for [`Space::pump_all`].
    pub fn on_lane(route: &str, context: Value, hop: Value, body: Value) -> Msg {
        let mut hop = obj(hop);
        hop.insert("route".into(), json!(route));
        Msg {
            context: obj(context),
            hop,
            body: obj(body),
        }
    }

    /// An `llm` cell's answer to the oldest message it holds, as the cell
    /// emits it. Returns the message it answered.
    pub fn llm_answer(&mut self, text: &str, finish: &str) -> Msg {
        let (cell, req) = self.llm.pop_front().expect("an llm request");
        let ttl = self.llm_ttl.pop_front().unwrap_or(TTL);
        let msg = Msg {
            context: req.context.clone(),
            hop: obj(json!({"finish_reason": finish, "model": "stub-model"})),
            body: obj(json!({"messages": [{"origin": "assistant", "type": "text", "text": text}]})),
        };
        self.carry(vec![(format!("./{cell}"), msg, ttl)]);
        req
    }

    pub fn next_op_id(&mut self) -> String {
        self.seq += 1;
        format!("q{}", self.seq)
    }

    /// One request on `lane`; returns the ONE answer that left through the
    /// hive path for it (its body, `messages` removed), its hop checked.
    pub fn request(
        &mut self,
        lane: &str,
        op: &str,
        file: Option<&str>,
        args: Value,
        hop_extra: Value,
    ) -> Value {
        let op_id = self.next_op_id();
        let mut hop = json!({"op": op, "op_id": op_id});
        for (k, v) in obj(hop_extra) {
            hop[k] = v;
        }
        let mut body = json!({"op": op, "args": args});
        if let Some(f) = file {
            body["file"] = json!(f);
        }
        let caller = hop.get("caller").cloned().unwrap_or(json!(""));
        let before = self.out.len();
        self.lane(lane, json!({}), hop, body);
        let mine: Vec<Msg> = self.out[before..]
            .iter()
            .filter(|m| m.route() == "answer" && m.hop.get("op_id") == Some(&json!(op_id)))
            .cloned()
            .collect();
        assert_eq!(
            mine.len(),
            1,
            "{op} on {lane}: exactly one answer, got {mine:?}; stderr {:?}",
            self.stderr
        );
        let m = &mine[0];
        assert_eq!(m.hop.get("op"), Some(&json!(op)), "the answer mirrors op");
        assert_eq!(
            m.hop.get("caller").cloned().unwrap_or(json!("")),
            caller,
            "the answer mirrors caller"
        );
        let mut b = m.body.clone();
        b.remove("messages");
        assert_eq!(
            b.get("op_id"),
            Some(&json!(op_id)),
            "the body mirrors op_id"
        );
        Value::Object(b)
    }

    /// A read on `in_read`.
    pub fn read(&mut self, op: &str, file: &str, args: Value) -> Value {
        self.request("in_read", op, Some(file), args, json!({}))
    }

    /// A read on `in_read` that names no file (`list`, `find`).
    pub fn read_space(&mut self, op: &str, args: Value) -> Value {
        self.request("in_read", op, None, args, json!({}))
    }

    pub fn routed(&self, route: &str) -> Vec<Msg> {
        self.out
            .iter()
            .filter(|m| m.route() == route)
            .cloned()
            .collect()
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

    /// The tables of the shipped schema that carry a `file` column.
    pub fn file_tables(&self) -> Vec<String> {
        let schema = &self.cells["store"]["params"]["schema"];
        let mut out: Vec<String> = schema
            .as_object()
            .unwrap()
            .iter()
            .filter(|(_, cols)| cols.get("file").is_some())
            .map(|(t, _)| t.clone())
            .collect();
        out.sort();
        out
    }

    /// R-FH-1 Auflage 2 measured at the store: every select, update and
    /// delete on a table with `file` names `file` in its `where`, and every
    /// select carries a `limit`. Three reads are exempt: `files` (path lookup,
    /// `list`, `find`), `ws_files` by `ws` (the files a workspace touched,
    /// OR-FH-68) and the one `near` scan of `node_runs` (GH #944, see
    /// [`is_near_scan`]) -- an update or delete on any of them still names its
    /// file. The breaches, as text.
    pub fn unscoped(&self) -> Vec<String> {
        let with_file = self.file_tables();
        let mut out = Vec::new();
        for (sender, a) in &self.store_ops {
            let op = a["operation"].as_str().unwrap_or("");
            let table = a["table"].as_str().unwrap_or("");
            if op == "select" && a.get("limit").is_none() {
                out.push(format!("{sender}: select without limit: {a}"));
            }
            let read = matches!(op, "select" | "search" | "similar");
            // Auflage 2 exempts three READS only (review S I-2): `files` for the
            // path lookup, `list` and `find`, `ws_files` by `ws`, and the
            // `near` scan of `node_runs` (GH #944) -- that scan alone, not
            // every read of the table (C1 finding, review of GH #944 M-4). An
            // update or delete on any of them still names its file.
            let exempt = read
                && (table == "files"
                    || is_near_scan(sender, a)
                    || (table == "ws_files" && a["where"].get("ws").is_some()));
            if (read || matches!(op, "update" | "delete"))
                && !exempt
                && with_file.iter().any(|t| t == table)
                && a["where"].get("file").is_none()
            {
                out.push(format!("{sender}: {op} on {table} without file: {a}"));
            }
        }
        out
    }

    // ------------------------------------------------------------- seeding

    fn exec<P: rusqlite::Params>(&self, sql: &str, params: P) {
        self.db
            .execute(sql, params)
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
    }

    /// A `files` row with no head yet (born nowhere until a head or a
    /// workspace names a version).
    pub fn seed_file(&mut self, file: &str, path: &str, kind: &str, mime: &str) {
        self.exec(
            "INSERT INTO files (file, path, kind, mime, head, head_seq, lock, tomb, oneline, \
             bytes, lines, born_at) VALUES (?1, ?2, ?3, ?4, '', 0, '', '', '', 0, 0, ?5)",
            rusqlite::params![&file, &path, &kind, &mime, &"2026-09-29T00:00:00.000000Z"],
        );
    }

    /// A version of `file` out of `bytes`, cut into blocks after every length
    /// of `cuts` (cycled; empty = one block). Blocks are `utf8` when the piece
    /// is UTF-8 without NUL, else `b64`. Returns the full version (sha256 hex).
    /// Moves no head.
    pub fn seed_version(
        &mut self,
        file: &str,
        bytes: &[u8],
        cuts: &[usize],
        parent: &str,
    ) -> String {
        let mut hashes = Vec::new();
        let mut at = 0usize;
        let mut k = 0usize;
        while at < bytes.len() {
            let len = if cuts.is_empty() {
                bytes.len()
            } else {
                cuts[k % cuts.len()].max(1)
            };
            k += 1;
            let end = (at + len).min(bytes.len());
            let piece = &bytes[at..end];
            at = end;
            let h = sha256_hex(piece);
            let (enc, body) = match std::str::from_utf8(piece) {
                Ok(s) if !s.contains('\0') => ("utf8", s.to_string()),
                _ => ("b64", b64(piece)),
            };
            let have: i64 = self
                .db
                .query_row(
                    "SELECT COUNT(*) FROM blocks WHERE file = ?1 AND hash = ?2",
                    [file, h.as_str()],
                    |r| r.get(0),
                )
                .unwrap();
            if have == 0 {
                self.exec(
                    "INSERT INTO blocks (file, hash, enc, size, body, first_seen) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![
                        &file,
                        &h,
                        &enc,
                        &(piece.len() as i64),
                        &body,
                        &"2026-09-29T00:00:00.000000Z"
                    ],
                );
            }
            hashes.push(h);
        }
        let version = sha256_hex(bytes);
        let lines = line_count(bytes);
        let blocks = sj::to_string(&hashes).unwrap();
        let have: i64 = self
            .db
            .query_row(
                "SELECT COUNT(*) FROM versions WHERE file = ?1 AND version = ?2",
                [file, version.as_str()],
                |r| r.get(0),
            )
            .unwrap();
        if have == 0 {
            self.exec(
                "INSERT INTO versions (file, version, blocks, bytes, lines, parent, force, \
                 made_by, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, '', 'seed', ?7)",
                rusqlite::params![
                    &file,
                    &version,
                    &blocks,
                    &(bytes.len() as i64),
                    &lines,
                    &parent,
                    &"2026-09-29T00:00:00.000000Z"
                ],
            );
        }
        version
    }

    /// One `line` row of the main line.
    #[allow(clippy::too_many_arguments)]
    pub fn seed_line(
        &mut self,
        file: &str,
        seq: i64,
        version: &str,
        prev: &str,
        op: &str,
        note: &str,
        ws: &str,
        commit: &str,
    ) {
        let at = stamp(seq);
        self.exec(
            "INSERT INTO line (file, seq, version, prev, op, note, ws, \"commit\", at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![&file, &seq, &version, &prev, &op, &note, &ws, &commit, &at],
        );
    }

    /// Move the head of `file` to `version` at `seq`, with its `line` row and
    /// the `files` figures the writer keeps.
    pub fn seed_head(&mut self, file: &str, version: &str, seq: i64, op: &str) {
        let prev = self.rows(&format!("SELECT head FROM files WHERE file = '{file}'"))[0][0]
            .as_str()
            .unwrap_or("")
            .to_string();
        self.seed_line(file, seq, version, &prev, op, "", "", "");
        let (bytes, lines) = {
            let r = self.rows(&format!(
                "SELECT bytes, lines FROM versions WHERE file = '{file}' AND version = '{version}'"
            ));
            (r[0][0].as_i64().unwrap_or(0), r[0][1].as_i64().unwrap_or(0))
        };
        self.exec(
            "UPDATE files SET head = ?1, head_seq = ?2, bytes = ?3, lines = ?4 WHERE file = ?5",
            rusqlite::params![&version, &seq, &bytes, &lines, &file],
        );
    }

    /// A text file whose main line went through `texts` in order (seq 1000,
    /// 2000, ...), each cut into blocks by `cuts`. Returns the versions.
    pub fn seed_text(
        &mut self,
        file: &str,
        path: &str,
        texts: &[&str],
        cuts: &[usize],
    ) -> Vec<String> {
        self.seed_file(file, path, "text", "text/plain");
        let mut out: Vec<String> = Vec::new();
        for (i, t) in texts.iter().enumerate() {
            let parent = out.last().cloned().unwrap_or_default();
            let v = self.seed_version(file, t.as_bytes(), cuts, &parent);
            let op = if i == 0 { "create" } else { "overwrite" };
            self.seed_head(file, &v, 1000 * (i as i64 + 1), op);
            out.push(v);
        }
        out
    }

    pub fn seed_derived(&mut self, file: &str, version: &str, part: i64, text: &str) {
        self.exec(
            "INSERT INTO derived (file, version, kind, part, body) VALUES (?1, ?2, 'text', ?3, ?4)",
            rusqlite::params![&file, &version, &part, &text],
        );
    }

    pub fn seed_snap(&mut self, file: &str, name: &str, version: &str) {
        self.exec(
            "INSERT INTO snaps (file, name, version, at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![&file, &name, &version, &"2026-09-29T00:00:00.000000Z"],
        );
    }

    pub fn seed_summary(&mut self, file: &str, version: &str, level: &str, text: &str) {
        self.exec(
            "INSERT INTO summaries (file, version, level, text, model, at) \
             VALUES (?1, ?2, ?3, ?4, 'stub-model', ?5)",
            rusqlite::params![
                &file,
                &version,
                &level,
                &text,
                &"2026-09-29T00:00:00.000000Z"
            ],
        );
    }

    /// An open workspace `ws` named `name` whose base is the main line at `base_seq`.
    pub fn seed_ws(&mut self, ws: &str, name: &str, root: &str, base_seq: i64) {
        self.exec(
            "INSERT INTO ws (ws, name, root, base_seq, state, opened_at, closed_at, \"commit\") \
             VALUES (?1, ?2, ?3, ?4, 'open', ?5, '', '')",
            rusqlite::params![&ws, &name, &root, &base_seq, &stamp(base_seq)],
        );
    }

    pub fn seed_ws_file(&mut self, ws: &str, file: &str, base: &str, working: &str, state: &str) {
        self.exec(
            "INSERT INTO ws_files (ws, file, base, working, state, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![&ws, &file, &base, &working, &state, &"2026-09-29T00:00:00.000000Z"],
        );
    }

    /// A `commits` row; `plan` the JSON `[{file, from, to}]`.
    pub fn seed_commit(
        &mut self,
        commit: &str,
        ws: &str,
        state: &str,
        plan: Value,
        deadline: &str,
    ) {
        let plan = sj::to_string(&plan).unwrap();
        self.exec(
            "INSERT INTO commits (\"commit\", ws, state, plan, note, at, deadline) \
             VALUES (?1, ?2, ?3, ?4, '', ?5, ?6)",
            rusqlite::params![
                &commit,
                &ws,
                &state,
                &plan,
                &"2026-09-29T00:00:00.000000Z",
                &deadline
            ],
        );
    }

    pub fn seed_lock(&mut self, file: &str, commit: &str) {
        self.exec(
            "UPDATE files SET lock = ?1 WHERE file = ?2",
            rusqlite::params![&commit, &file],
        );
    }

    pub fn seed_tomb(&mut self, file: &str) {
        self.exec(
            "UPDATE files SET tomb = ?1 WHERE file = ?2",
            rusqlite::params![&"2026-09-29T00:00:00.000000Z", &file],
        );
    }

    /// Every row of `file` in every table with a `file` column, copied into
    /// `other` -- a move to another space's store (ADR 0047).
    pub fn copy_file_to(&self, file: &str, other: &Space) {
        for t in self.file_tables() {
            let cols: Vec<String> = {
                let st = self
                    .db
                    .prepare(&format!("SELECT * FROM \"{t}\" LIMIT 0"))
                    .unwrap();
                st.column_names().iter().map(|c| c.to_string()).collect()
            };
            let list = cols
                .iter()
                .map(|c| format!("\"{c}\""))
                .collect::<Vec<_>>()
                .join(", ");
            let marks = (1..=cols.len())
                .map(|i| format!("?{i}"))
                .collect::<Vec<_>>()
                .join(", ");
            let mut st = self
                .db
                .prepare(&format!("SELECT {list} FROM \"{t}\" WHERE file = ?1"))
                .unwrap();
            let rows: Vec<Vec<rusqlite::types::Value>> = st
                .query_map([file], |r| {
                    Ok((0..cols.len())
                        .map(|i| r.get::<_, rusqlite::types::Value>(i).unwrap())
                        .collect())
                })
                .unwrap()
                .map(Result::unwrap)
                .collect();
            for row in rows {
                other
                    .db
                    .execute(
                        &format!("INSERT INTO \"{t}\" ({list}) VALUES ({marks})"),
                        rusqlite::params_from_iter(row.iter()),
                    )
                    .unwrap();
            }
        }
    }
}

impl Default for Space {
    fn default() -> Self {
        Self::new()
    }
}

pub fn sha256_hex(b: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

/// Standard base64 with padding (the crate has no base64 dependency, and a
/// test adds none -- AGENTS.md rule 6).
pub fn b64(b: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in b.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                out.push(A[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Lines as the space counts them: `\n` ends a line, a last line without
/// one counts.
pub fn line_count(b: &[u8]) -> i64 {
    let n = b.iter().filter(|c| **c == b'\n').count() as i64;
    if !b.is_empty() && *b.last().unwrap() != b'\n' {
        n + 1
    } else {
        n
    }
}

/// `h4` of one line, computed here and not by a script.
pub fn h4(line: &str) -> String {
    sha256_hex(line.as_bytes())[..4].to_string()
}

/// A UTC stamp for a `seq` (microseconds since the epoch).
pub fn stamp(seq: i64) -> String {
    chrono::DateTime::from_timestamp_micros(seq)
        .expect("a stamp")
        .format("%Y-%m-%dT%H:%M:%S%.6fZ")
        .to_string()
}

/// Every string anywhere in `v`.
pub fn strings(v: &Value) -> Vec<String> {
    match v {
        Value::String(s) => vec![s.clone()],
        Value::Array(a) => a.iter().flat_map(strings).collect(),
        Value::Object(o) => o
            .iter()
            .flat_map(|(k, x)| std::iter::once(k.clone()).chain(strings(x)))
            .collect(),
        _ => Vec::new(),
    }
}

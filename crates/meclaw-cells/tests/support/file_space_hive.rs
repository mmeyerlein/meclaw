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

pub const TEMPLATE: &str = "templates/file-space";

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
        let doc = json!({
            "envelope": {"header": {"context": msg.context, "hop": msg.hop},
                         "target": format!("{}/{name}", self.path),
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
                }
            })
            .collect()
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
        let mut queue = VecDeque::from([(from.to_string(), msg)]);
        let mut steps = 0;
        while let Some((from, msg)) = queue.pop_front() {
            steps += 1;
            assert!(steps < 4000, "the space does not come to rest");
            for (to, m) in self.route(&from, &msg) {
                if to == "." {
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
                    "llm" => self.llm.push_back((name, m)),
                    other => panic!("an edge onto {to} of type {other:?}"),
                }
            }
        }
    }

    /// A message arriving on the hive path on lane `route`.
    pub fn lane(&mut self, route: &str, context: Value, hop: Value, body: Value) {
        let mut hop = obj(hop);
        hop.insert("route".into(), json!(route));
        let msg = Msg {
            context: obj(context),
            hop,
            body: obj(body),
        };
        self.pump(".", msg);
    }

    /// An `llm` cell's answer to the oldest message it holds, as the cell
    /// emits it. Returns the message it answered.
    pub fn llm_answer(&mut self, text: &str, finish: &str) -> Msg {
        let (cell, req) = self.llm.pop_front().expect("an llm request");
        let msg = Msg {
            context: req.context.clone(),
            hop: obj(json!({"finish_reason": finish, "model": "stub-model"})),
            body: obj(json!({"messages": [{"origin": "assistant", "type": "text", "text": text}]})),
        };
        self.pump(&format!("./{cell}"), msg);
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
    /// select carries a `limit`. Two reads are exempt: `files` (path lookup,
    /// `list`, `find`) and `ws_files` by `ws` (the files a workspace touched,
    /// OR-FH-68) -- an update or delete on `ws_files` still names its file.
    /// The breaches, as text.
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
            // Auflage 2 exempts two READS only (review S I-2): `files` for the
            // path lookup, `list` and `find`, and `ws_files` by `ws`. An update
            // or delete on either still names its file.
            let exempt = read
                && (table == "files" || (table == "ws_files" && a["where"].get("ws").is_some()));
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

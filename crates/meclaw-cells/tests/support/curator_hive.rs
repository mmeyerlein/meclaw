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

/// [`AST_LOADER`] for a cell whose upper-case names also read the running
/// message (`./history`: `SESSION`, `ROUND`, ...): an assignment that cannot
/// be evaluated without a message is left out of the scope; imports and defs
/// load as there.
const AST_LOADER_LENIENT: &str = r#"
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
    try:
        exec(compile(ast.Module(body=[n], type_ignores=[]), "cell", "exec"), scope)
    except Exception:
        if not isinstance(n, ast.Assign):
            raise
scope["ARGS"] = inp.get("args")
print(json.dumps(eval(inp["probe"], scope)))
"#;

/// The pure half of the shipped cell `cell` (GH #1436: the one function both
/// `./history` and `./policy` age a topic with), loaded with the shipped
/// params overlaid by `over`, and `probe` evaluated in it.
pub fn cell_scope(cell: &str, over: Value, probe: &str, args: Value) -> (Value, String) {
    let mut params = cell_config(cell)["params"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    params.remove("script_inline");
    for (k, v) in over.as_object().cloned().unwrap_or_default() {
        params.insert(k, v);
    }
    let doc = json!({"src": script_of(cell), "params": params, "probe": probe,
                     "args": args});
    let out = run_python(AST_LOADER_LENIENT, &doc.to_string());
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        out.status.success(),
        "{cell}: the pure half does not load: {err}"
    );
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
/// ledger's counts on `in_stats`), `history` since GH #1279 (the judge
/// context, the seed of a live call and its seed lines), `reader` since
/// GH #1448 (the audience-gated read of the plan snapshots).
pub const CELLS: [&str; 12] = [
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
    "history",
    "reader",
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
    /// The store refuses -- every leg `store_error` -- the next ledger
    /// message whose `cur_phase` is this (GH #955: a bundle of a running
    /// rebuild that the store does not take). Taken when it strikes; set
    /// `ledger_may_refuse` with it.
    pub refuse_phase: Option<String>,
    /// Before the store answers the next ledger message whose `cur_phase`
    /// is the first, it runs the SQL of the second on the ledger: another
    /// chain's write landing between two steps of this one (GH #955 review
    /// M-1, a claim taken over while a refused bundle is on its way back).
    /// Taken when it strikes, before `refuse_phase` is looked at.
    pub sql_at_phase: Option<(String, String)>,
    /// GH #1371: before the store answers the next ledger message whose
    /// `cur_phase` is this, the lane message runs to rest on its own: another
    /// delivery of the hive that comes in and finishes between two steps of
    /// a chain (a turn's normal curate while its spec curate is in
    /// `./handover`). Taken when it strikes; set with [`Hive::lane_at_phase`].
    lane_at: Option<(String, Msg)>,
    /// GH #1225: the cells of [`RESIDENT_CELLS`] run as the colony runs them,
    /// `runner_mode: resident` -- one child per hive whose globals live from
    /// message to message (`_MEM` of `./intake`), never the thread's warm
    /// child that starts every message empty.
    resident: BTreeMap<String, warm_python::Resident>,
    /// The runs of each resident cell so far, by the `cur_phase` of the
    /// message it got (`""` for a lane).
    runs: BTreeMap<(String, String), usize>,
    /// GH #1225: `(cell, cur_phase, n)` -- the child of the resident `cell`
    /// is killed and booted again right before its `n`-th run (1-based) on a
    /// message of that phase: a deploy between two steps of a chain. Taken
    /// when it strikes.
    pub restart_at: Option<(String, String, usize)>,
    /// GH #1225: `(leg, error_code)` -- the store refuses every leg of the next
    /// ledger message whose tool_call id is `leg` (or starts with it, when it
    /// ends in `*`) with that code, and runs the other legs: one read or
    /// write of a bundle failing. An entry is taken in the message it struck;
    /// set `ledger_may_refuse` with it.
    pub refuse_legs: Vec<(String, String)>,
    /// GH #1260 (fix round 1): every emission of a hive cell is checked
    /// against that cell's own `contract.emits`, the way the colony's
    /// `validate_emits` checks it before routing -- a violation is
    /// dead-lettered there, so it is recorded here in `emit_violations`
    /// (`<cell>: <reason>`). Opt-in: the bridge of the tool round carried
    /// `hop.bridge = "tool"` outside the declared values, and no test saw it.
    pub check_emits: bool,
    pub emit_violations: Vec<String>,
    /// GH #1446 (D38 C0): the corpus clock, see [`Hive::set_clock`].
    clock_us: Option<i64>,
    runs_since_clock: i64,
    /// GH #1447 (D38 C1): the store of `./lead` (`./lead_state`), its own
    /// in-memory SQLite with the shipped schema and seed of that cell --
    /// never the ledger. `None` in a tree without the lead cell.
    pub lead_db: Option<rusqlite::Connection>,
    /// GH #1447: every operation `./lead_state` ran, with the cell that sent
    /// it -- measured at the store, like `ledger_ops`.
    pub lead_ops: Vec<(String, Value)>,
    /// GH #1447: `refuse_legs` for `./lead_state` -- `(leg, error_code)`, the
    /// store of the lead refuses that leg of the next message carrying it
    /// and runs the others. Taken when it strikes.
    pub lead_refuse_legs: Vec<(String, String)>,
    /// GH #1447 (the join of `win`): `(from, cur_phase)` -- every message
    /// routed from `from` to `./policy` (and, when the phase is set, only one
    /// whose context `cur_phase` is it) is held in `held` instead of
    /// delivered, until [`Hive::release_held`]. Two arrival orders of the two
    /// halves of a call (ledger legs, lead verdict) are made this way.
    pub hold: Option<(String, Option<String>)>,
    pub held: Vec<Msg>,
}

/// GH #1447 (D38 C1): the cells of the lead, loaded when the tree ships them.
pub const LEAD_CELLS: [&str; 2] = ["lead", "lead_state"];

/// GH #1447: whether the tree ships the lead cell of the curator.
pub fn lead_shipped() -> bool {
    repo("templates/curator/lead/config.json").is_file()
}

/// GH #1446 (D38 C0): the microseconds one cell run moves the corpus clock --
/// room for the readings of one run, the millisecond of a step unchanged
/// for up to 125 runs.
pub const CLOCK_STEP_US: i64 = 8;

/// GH #1225: the cells the hive runs `resident`, the way they ship.
/// `push` since strand KP: its RAM is a cache of in-flight writes only (A1).
pub const RESIDENT_CELLS: [&str; 3] = ["intake", "push", "policy"];

// Folded, this module is `pub` at a harness root, so clippy::new_without_default
// applies to `Hive::new` (W5 clippy).
impl Default for Hive {
    fn default() -> Self {
        Self::new()
    }
}

impl Hive {
    pub fn new() -> Self {
        Self::with(&[])
    }

    /// The shipped hive with `(cell, param, value)` overrides.
    pub fn with(over: &[(&str, &str, Value)]) -> Self {
        Self::with_prelude(over, "")
    }

    /// GH #1446 (D38 C0): [`Hive::with`], every cell's program run behind
    /// `prelude` -- python the test puts in front of each shipped script
    /// (the corpus's own clock). The shipped text follows unchanged; an empty
    /// prelude is [`Hive::with`].
    pub fn with_prelude(over: &[(&str, &str, Value)], prelude: &str) -> Self {
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
        if lead_shipped() {
            for name in LEAD_CELLS {
                cells.insert(name.to_string(), cell_config(name));
            }
        }
        for (cell, key, value) in over {
            let params = cells
                .get_mut(*cell)
                .and_then(|c| c["params"].as_object_mut())
                .expect("a cell with params");
            assert!(params.contains_key(*key), "no such param: {cell}.{key}");
            params.insert((*key).to_string(), value.clone());
        }
        if !prelude.is_empty() {
            for cfg in cells.values_mut() {
                if let Some(s) = cfg["params"]["script_inline"].as_str() {
                    let joined = format!("{prelude}\n{s}");
                    cfg["params"]["script_inline"] = json!(joined);
                }
            }
        }
        let db = rusqlite::Connection::open_in_memory().expect("sqlite");
        // GH #1225: the store's own reading of the ledger params -- a column
        // in the object form (`{type, default}`, `wall.turn_at`) carries its
        // SQL DEFAULT exactly as the store cell builds it.
        let ledger = meclaw_cells::store::StoreParams::parse(&cells["ledger"]["params"])
            .expect("the ledger params parse");
        meclaw_cells::store::ddl::apply_declared_schema_ddl(&db, &ledger).expect("ddl");
        // GH #915/#943: the named indexes right after the schema and before the
        // seed, in the store's own order (`store::factory`: schema -> indexes
        // -> seed). The unique index on `state.key` is what turns the insert of
        // a round's first row into a compare-and-set.
        meclaw_cells::store::ddl::apply_index_ddl(&db, &ledger.indexes).expect("index ddl");
        meclaw_cells::store::seed::load_seed_if_present(
            &db,
            &repo("templates/curator/ledger"),
            &ledger.schema,
        )
        .expect("seed");
        // GH #1447: the lead's own store, built like the ledger (schema ->
        // indexes -> seed) from the params of `./lead_state`.
        let lead_db = cells.get("lead_state").map(|cfg| {
            let p = meclaw_cells::store::StoreParams::parse(&cfg["params"])
                .expect("the lead_state params parse");
            let db = rusqlite::Connection::open_in_memory().expect("sqlite");
            meclaw_cells::store::ddl::apply_declared_schema_ddl(&db, &p).expect("lead ddl");
            meclaw_cells::store::ddl::apply_index_ddl(&db, &p.indexes).expect("lead index ddl");
            meclaw_cells::store::seed::load_seed_if_present(
                &db,
                &repo("templates/curator/lead_state"),
                &p.schema,
            )
            .expect("lead seed");
            db
        });
        let mut resident = BTreeMap::new();
        for name in RESIDENT_CELLS {
            let cfg = &cells[name];
            if cfg["params"]["runner_mode"] == "resident" {
                let script = cfg["params"]["script_inline"].as_str().expect("script");
                resident.insert(name.to_string(), warm_python::Resident::boot(script));
            }
        }
        Self {
            edges,
            cells,
            db,
            resident,
            runs: BTreeMap::new(),
            restart_at: None,
            refuse_legs: Vec::new(),
            out: Vec::new(),
            clock: Vec::new(),
            summ: VecDeque::new(),
            stderr: Vec::new(),
            ledger_ops: Vec::new(),
            ledger_may_refuse: false,
            refuse_phase: None,
            sql_at_phase: None,
            lane_at: None,
            check_emits: false,
            emit_violations: Vec::new(),
            clock_us: None,
            runs_since_clock: 0,
            lead_db,
            lead_ops: Vec::new(),
            lead_refuse_legs: Vec::new(),
            hold: None,
            held: Vec::new(),
        }
    }

    /// GH #1446 (D38 C0): the corpus clock at `us` microseconds since the
    /// epoch. From here on every cell run's stdin document starts with the
    /// key `c0_clock_us` -- this instant plus `CLOCK_STEP_US` per run since
    /// the clock was set -- which the corpus prelude reads.
    pub fn set_clock(&mut self, us: i64) {
        self.clock_us = Some(us);
        self.runs_since_clock = 0;
    }

    /// GH #1225: the child of the resident `cell` killed and booted again --
    /// its RAM is gone, the ledger is what it was.
    pub fn restart(&mut self, cell: &str) {
        self.resident
            .get_mut(cell)
            .unwrap_or_else(|| panic!("{cell} does not run resident"))
            .restart();
    }

    /// OR-d38-3.o9.4: `cell` runs without a child of its own from here on --
    /// every message a fresh process, nothing kept between two of them (as a
    /// resident child restarted after each message).
    pub fn run_cold(&mut self, cell: &str) {
        self.resident.remove(cell);
    }

    fn run_cell(&mut self, name: &str, msg: &Msg) -> Vec<Msg> {
        let cfg = &self.cells[name];
        let mut params = obj(cfg["params"].clone());
        params.remove("script_inline");
        let doc = json!({
            "envelope": {"header": {"context": msg.context, "hop": msg.hop},
                         "target": format!("/x/curator/{name}"),
                         "reply_to": msg.reply_to},
            "body": msg.body,
            "params": params,
        })
        .to_string();
        let doc = match self.clock_us {
            Some(us) => {
                let at = us + self.runs_since_clock * CLOCK_STEP_US;
                self.runs_since_clock += 1;
                format!("{{\"c0_clock_us\":{at},{}", &doc[1..])
            }
            None => doc,
        };
        let mut warm = None;
        if self.resident.contains_key(name) {
            let phase = msg
                .context
                .get("cur_phase")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let n = self
                .runs
                .entry((name.to_string(), phase.clone()))
                .or_insert(0);
            *n += 1;
            let n = *n;
            if self
                .restart_at
                .as_ref()
                .is_some_and(|(c, p, at)| c == name && *p == phase && *at == n)
            {
                self.restart_at = None;
                self.restart(name);
            }
            warm = self.resident.get_mut(name).and_then(|r| r.run(&doc));
        }
        let out = match warm {
            Some(out) => out,
            None => run_python(
                self.cells[name]["params"]["script_inline"]
                    .as_str()
                    .expect("script"),
                &doc,
            ),
        };
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
        // GH #1447: the code cell refuses an array its contract does not
        // declare (`multi_send_not_declared`, docs/cell-types.md § code) --
        // the lead's first form wrote one and dead-lettered in a real colony
        // while this harness took it. Like the cell (`code/cell.rs`,
        // `parsed.len() > 1`), an array of at most one message is one send
        // (`curator/stats` answers so).
        assert!(
            emitted.as_array().is_none_or(|a| a.len() <= 1)
                || self.cells[name]["contract"]["multi_send_capable"] == json!(true),
            "{name} writes a JSON array without contract.multi_send_capable \
             (multi_send_not_declared)"
        );
        let list = match emitted {
            Value::Array(a) => a,
            other => vec![other],
        };
        if self.check_emits {
            self.check_emits_of(name, &list);
        }
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

    /// The colony's emission check on one cell's output (`check_emits`).
    fn check_emits_of(&mut self, name: &str, list: &[Value]) {
        let emits = &self.cells[name]["contract"]["emits"];
        if emits.is_null() {
            return;
        }
        let block: meclaw_core::EmitsBlock = sj::from_value(emits.clone())
            .unwrap_or_else(|e| panic!("{name}: contract.emits does not parse: {e}"));
        let compiled = meclaw_core::CompiledEmits::compile(&block)
            .unwrap_or_else(|e| panic!("{name}: contract.emits does not compile: {e}"));
        for m in list {
            if let Err(e) = meclaw_core::validate_emits(m, &compiled) {
                self.emit_violations.push(format!("{name}: {e}"));
            }
        }
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
        let phase = msg.context.get("cur_phase").and_then(Value::as_str);
        if self
            .sql_at_phase
            .as_ref()
            .is_some_and(|(at, _)| Some(at.as_str()) == phase)
            && let Some((_, sql)) = self.sql_at_phase.take()
        {
            self.db
                .execute_batch(&sql)
                .expect("the SQL of `sql_at_phase` runs");
        }
        if self.refuse_phase.is_some() && self.refuse_phase.as_deref() == phase {
            self.refuse_phase = None;
            let legs: Vec<BundleLeg> = calls
                .iter()
                .map(|c| {
                    let args: Value =
                        sj::from_str(c["text"].as_str().unwrap_or("")).unwrap_or(Value::Null);
                    BundleLeg::refusal(
                        args["operation"].as_str().unwrap_or("error"),
                        c["id"].as_str().unwrap_or("").to_string(),
                        0,
                        "store_error",
                        "refused by the test".to_string(),
                    )
                })
                .collect();
            let (body, hop) = build_bundle_result(&legs, 0);
            return Msg {
                context: msg.context.clone(),
                hop,
                body: obj(body),
                reply_to: String::new(),
            };
        }
        let struck: Vec<(String, String)> = self
            .refuse_legs
            .iter()
            .filter(|(leg, _)| calls.iter().any(|c| leg_matches(leg, &c["id"])))
            .cloned()
            .collect();
        self.refuse_legs.retain(|e| !struck.contains(e));
        let refused_as = |id: &Value| {
            struck
                .iter()
                .find(|(leg, _)| leg_matches(leg, id))
                .map(|(_, code)| code.clone())
        };
        let (body, hop) = if !struck.is_empty() {
            // GH #1225: the struck legs refused, every other one run -- in
            // the bundle form, so each leg answers under its own id.
            let legs: Vec<BundleLeg> = calls
                .iter()
                .map(|c| {
                    let id = c["id"].as_str().unwrap_or("").to_string();
                    let args: Value =
                        sj::from_str(c["text"].as_str().unwrap_or("")).expect("op json");
                    if let Some(code) = refused_as(&c["id"]) {
                        return BundleLeg::refusal(
                            args["operation"].as_str().unwrap_or("error"),
                            id,
                            0,
                            &code,
                            "refused by the test".to_string(),
                        );
                    }
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
        } else if calls.len() == 1 {
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
        self.pump_many(vec![(from.to_string(), msg)]);
    }

    /// Several messages at once, run to rest through ONE FIFO queue: each
    /// step of each chain is taken in turn, so two chains interleave step by
    /// step the way two deliveries in flight do (GH #955).
    fn pump_many(&mut self, starts: Vec<(String, Msg)>) {
        let mut queue = VecDeque::from(starts);
        let mut steps = 0;
        while let Some((from, msg)) = queue.pop_front() {
            steps += 1;
            assert!(steps < 4000, "the hive does not come to rest");
            for (to, m) in self.route(&from, &msg) {
                match to.as_str() {
                    "." => self.out.push(m),
                    "./ledger" => {
                        let phase = m.context.get("cur_phase").and_then(Value::as_str);
                        if self
                            .lane_at
                            .as_ref()
                            .is_some_and(|(at, _)| Some(at.as_str()) == phase)
                            && let Some((_, lane)) = self.lane_at.take()
                        {
                            self.pump(".", lane);
                        }
                        let sender = from.trim_start_matches("./").to_string();
                        for c in m.messages().iter().filter(|c| c["type"] == "tool_call") {
                            let args = sj::from_str(c["text"].as_str().unwrap_or(""))
                                .unwrap_or(Value::Null);
                            self.ledger_ops.push((sender.clone(), args));
                        }
                        let answer = self.store(&m);
                        queue.push_back(("./ledger".to_string(), answer));
                    }
                    "./policy"
                        if self.hold.as_ref().is_some_and(|(f, p)| {
                            *f == from
                                && p.as_deref().is_none_or(|p| {
                                    m.context.get("cur_phase").and_then(Value::as_str) == Some(p)
                                })
                        }) =>
                    {
                        self.held.push(m)
                    }
                    "./lead_state" if self.lead_db.is_some() => {
                        let sender = from.trim_start_matches("./").to_string();
                        for c in m.messages().iter().filter(|c| c["type"] == "tool_call") {
                            let args = sj::from_str(c["text"].as_str().unwrap_or(""))
                                .unwrap_or(Value::Null);
                            self.lead_ops.push((sender.clone(), args));
                        }
                        let answer = store_plain(
                            self.lead_db.as_ref().expect("lead db"),
                            &m,
                            &mut self.lead_refuse_legs,
                        );
                        queue.push_back(("./lead_state".to_string(), answer));
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
        let msg = Self::lane_msg(route, context, hop, body);
        self.pump(".", msg);
    }

    /// GH #1371: a message on lane `route` that arrives right before the
    /// store answers the next ledger message of `phase` and runs to rest
    /// there (see `lane_at`).
    pub fn lane_at_phase(
        &mut self,
        phase: &str,
        route: &str,
        context: Value,
        hop: Value,
        body: Value,
    ) {
        self.lane_at = Some((phase.to_string(), Self::lane_msg(route, context, hop, body)));
    }

    /// Several messages arriving on the hive path at once, `(route, context,
    /// hop, body)` each, run to rest through ONE FIFO queue (GH #1225): two
    /// slot writes in flight both read before either writes, as two
    /// deliveries do.
    pub fn lanes_together(&mut self, lanes: Vec<(&str, Value, Value, Value)>) {
        let starts = lanes
            .into_iter()
            .map(|(route, context, hop, body)| {
                (".".to_string(), Self::lane_msg(route, context, hop, body))
            })
            .collect();
        self.pump_many(starts);
    }

    fn lane_msg(route: &str, context: Value, hop: Value, body: Value) -> Msg {
        let mut hop = obj(hop);
        hop.insert("route".into(), json!(route));
        // GH #925: every lane message of a colony carries the audience of its
        // round. A test that names none gets the standard round; one that sets
        // the key to `null` keeps it null -- a round that declares nothing.
        let mut context = obj(context);
        context
            .entry("audience_set")
            .or_insert_with(|| json!(ROUND_E));
        Msg {
            context,
            hop,
            body: obj(body),
            reply_to: String::new(),
        }
    }

    /// The strike of an order the clock holds, as the timer emits it: a
    /// fresh root without a context, the row's `emit_headers` as hop keys and
    /// the timer's own headers over them (`timer::cell::build_fire_content`).
    /// The row under one id is the order armed LAST (`rearm` replaces it), so
    /// its `emit_headers` ride -- the round of the newest call (GH #925,
    /// OR-BD.A.6); an order the clock never saw brings its own.
    pub fn fire(&mut self, order: &Msg) {
        let msg = self.strike_of(order);
        self.pump("./clock", msg);
    }

    fn strike_of(&self, order: &Msg) -> Msg {
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
        Msg {
            context: Map::new(),
            hop,
            body: obj(order.body["emit_body"].clone()),
            reply_to: String::new(),
        }
    }

    /// Several strikes at the same instant (GH #955): each as [`Hive::fire`]
    /// builds it, all of them through one queue, so their steps interleave.
    pub fn fire_together(&mut self, orders: &[Msg]) {
        let starts = orders
            .iter()
            .map(|o| ("./clock".to_string(), self.strike_of(o)))
            .collect();
        self.pump_many(starts);
    }

    /// The summarizer's answer to the oldest request it holds.
    pub fn answer(&mut self, text: &str, finish: &str) -> Msg {
        self.answer_with(text, finish, json!({}))
    }

    /// The summarizer's answer with `extra` hop keys -- the llm cell's stamp
    /// of its own package (`input_soft`, GH #1037) among them.
    pub fn answer_with(&mut self, text: &str, finish: &str, extra: Value) -> Msg {
        let req = self.summ.pop_front().expect("a summarizer request");
        let mut hop = obj(json!({"finish_reason": finish, "model": "summary-model"}));
        for (k, v) in obj(extra) {
            hop.insert(k, v);
        }
        let msg = Msg {
            context: req.context.clone(),
            hop,
            body: obj(json!({"messages": [{"origin": "assistant", "type": "text", "text": text}]})),
            reply_to: String::new(),
        };
        self.pump("./summarizer", msg);
        req
    }

    /// The summarizer's refusal of the oldest request it holds as over its
    /// window, in the llm cell's own form (`llm/window.rs`, R-HK-15/16): no
    /// model call, `invalid_input`, `meta.error.kind` `input_over_hard` with
    /// the bound it refused at, the request's messages handed back.
    pub fn refuse_window(&mut self, input_hard: u64) -> Msg {
        let req = self.summ.pop_front().expect("a summarizer request");
        let hop = obj(
            json!({"finish_reason": "error", "error_code": "invalid_input",
                             "latency_ms": 0}),
        );
        let msg = Msg {
            context: req.context.clone(),
            hop,
            body: obj(json!({
                "messages": req.messages(),
                "meta": {"provider": "openai", "latency_ms": 0, "started_at": 0,
                         "error": {"source": "window",
                                   "detail": "window: prompt over input_hard; no model call",
                                   "kind": "input_over_hard",
                                   "input_estimate": input_hard + 1,
                                   "input_hard": input_hard}}})),
            reply_to: String::new(),
        };
        self.pump("./summarizer", msg);
        req
    }

    /// GH #1447: the held messages delivered to `./policy`, in the order they
    /// were held, and the hive run to rest; the hold is lifted.
    pub fn release_held(&mut self) {
        self.hold = None;
        let held = std::mem::take(&mut self.held);
        for m in held {
            let outs = self.run_cell("policy", &m);
            let starts = outs
                .into_iter()
                .map(|o| ("./policy".to_string(), o))
                .collect();
            self.pump_many(starts);
        }
    }

    /// GH #1447: rows of the lead's own store (`./lead_state`).
    pub fn lead_rows(&self, sql: &str) -> Vec<Vec<Value>> {
        rows_of(self.lead_db.as_ref().expect("the tree ships the lead"), sql)
    }

    /// GH #1447: the state row `key` of the lead's own store.
    pub fn lead_state(&self, key: &str) -> String {
        self.lead_rows(&format!("SELECT value FROM state WHERE key = '{key}'"))
            .first()
            .and_then(|r| r[0].as_str().map(str::to_string))
            .unwrap_or_default()
    }

    pub fn rows(&self, sql: &str) -> Vec<Vec<Value>> {
        rows_of(&self.db, sql)
    }

    pub fn state(&self, key: &str) -> String {
        self.rows(&format!("SELECT value FROM state WHERE key = '{key}'"))
            .first()
            .and_then(|r| r[0].as_str().map(str::to_string))
            .unwrap_or_default()
    }

    /// The state row `key` of the round `round` (GH #943): the policy keeps
    /// a round's rows under `<key>:<round_key(round)>`.
    pub fn state_in(&self, key: &str, round: &str) -> String {
        self.state(&format!("{key}:{}", round_key(round)))
    }

    /// The plan of the window of the round `round` (`state.window_plan:<rk>`,
    /// GH #943), `{}` before the round's first rebuild.
    pub fn plan_in(&self, round: &str) -> Value {
        sj::from_str(&self.state_in("window_plan", round)).unwrap_or_else(|_| json!({}))
    }

    /// GH #1448: what the rebuilds of the round `round` made of its plan --
    /// [`Hive::plan_in`] without the blocks the answers write ahead
    /// (`blocks`, `blocks_to`); `{}` before the round's first rebuild.
    pub fn rebuild_in(&self, round: &str) -> Value {
        let mut plan = self.plan_in(round);
        if let Some(m) = plan.as_object_mut() {
            m.remove("blocks");
            m.remove("blocks_to");
        }
        plan
    }

    /// The plan after the LAST rebuild, of whichever round it ran in: the
    /// plan of the round with the newest entry in the registry
    /// `state.window_plans` (`{rk: last_used_ms}`, GH #943; ties go to the
    /// smaller key), `{}` before the first rebuild. A test that runs one
    /// round keeps the meaning it had when the hive held one plan; a test
    /// that runs several names the round with [`Hive::plan_in`].
    pub fn plan(&self) -> Value {
        let reg: Map<String, Value> = sj::from_str(&self.state("window_plans")).unwrap_or_default();
        let newest = reg
            .iter()
            .filter_map(|(rk, t)| t.as_f64().map(|t| (rk, t)))
            .fold(None::<(&String, f64)>, |best, (rk, t)| match best {
                Some((_, b)) if b >= t => best,
                _ => Some((rk, t)),
            });
        match newest {
            Some((rk, _)) => sj::from_str(&self.state(&format!("window_plan:{rk}")))
                .unwrap_or_else(|_| json!({})),
            None => json!({}),
        }
    }

    /// The newest `add` the clock received for the round `round` (GH #943:
    /// one order per round). Matched by the round key of the round the order
    /// carries in `emit_headers.audience_set` (canonical, `""` for a call
    /// without a round, which keys like `[]`) -- the key the schedule id is
    /// made of, without recomputing the policy's uuid5.
    pub fn clock_order_for(&self, round: &str) -> Option<Msg> {
        let want = round_key(round);
        self.clock
            .iter()
            .rev()
            .find(|m| {
                m.body.get("op") == Some(&json!("add"))
                    && m.body
                        .get("emit_headers")
                        .and_then(|h| h.get("audience_set"))
                        .and_then(Value::as_str)
                        .is_some_and(|a| round_key(a) == want)
            })
            .cloned()
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
        // talky's `./curator -> ./quick` edge lifts `hop.interim` too
        // (GH #1260); a call without it carries none.
        if let Some(interim) = call.hop.get("interim") {
            ctx.insert("interim".into(), interim.clone());
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

/// GH #1225: a `refuse_legs` entry against a tool_call id -- equal, or a
/// prefix when the entry ends in `*`.
/// The rows of `sql` on `db`, every column as JSON.
fn rows_of(db: &rusqlite::Connection, sql: &str) -> Vec<Vec<Value>> {
    use rusqlite::types::ValueRef;
    let mut st = db.prepare(sql).expect(sql);
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

/// GH #1447: one message of a store cell other than the ledger (`./lead_state`)
/// through the store's own dispatcher, answered in its own reply shape --
/// the ledger path without its refusal hooks. A refused op fails the test.
fn store_plain(db: &rusqlite::Connection, msg: &Msg, refuse: &mut Vec<(String, String)>) -> Msg {
    use meclaw_cells::store::ops::dispatch;
    use meclaw_cells::store::output::{BundleLeg, build_bundle_result, build_tool_result};
    let calls: Vec<Value> = msg
        .messages()
        .into_iter()
        .filter(|m| m["type"] == "tool_call")
        .collect();
    let struck: Vec<(String, String)> = refuse
        .iter()
        .filter(|(leg, _)| calls.iter().any(|c| leg_matches(leg, &c["id"])))
        .cloned()
        .collect();
    refuse.retain(|e| !struck.contains(e));
    let (body, hop) = if !struck.is_empty() {
        let legs: Vec<BundleLeg> = calls
            .iter()
            .map(|c| {
                let id = c["id"].as_str().unwrap_or("").to_string();
                let args: Value = sj::from_str(c["text"].as_str().unwrap_or("")).expect("op json");
                let op = args["operation"].as_str().unwrap_or("error");
                match struck.iter().find(|(leg, _)| leg_matches(leg, &c["id"])) {
                    Some((_, code)) => {
                        BundleLeg::refusal(op, id, 0, code, "refused by the test".to_string())
                    }
                    None => BundleLeg::from_outcome(
                        &dispatch(db, &args)
                            .unwrap_or_else(|e| panic!("a lead_state op failed: {e} in {args}")),
                        id,
                        0,
                    ),
                }
            })
            .collect();
        build_bundle_result(&legs, 0)
    } else if calls.len() == 1 {
        let c = &calls[0];
        let args: Value = sj::from_str(c["text"].as_str().unwrap_or("")).expect("op json");
        let outcome =
            dispatch(db, &args).unwrap_or_else(|e| panic!("a lead_state op failed: {e} in {args}"));
        build_tool_result(&outcome, c["id"].as_str().unwrap_or("").to_string(), 0)
    } else {
        let legs: Vec<BundleLeg> = calls
            .iter()
            .map(|c| {
                let id = c["id"].as_str().unwrap_or("").to_string();
                let args: Value = sj::from_str(c["text"].as_str().unwrap_or("")).expect("op json");
                let outcome = dispatch(db, &args)
                    .unwrap_or_else(|e| panic!("a lead_state op failed: {e} in {args}"));
                BundleLeg::from_outcome(&outcome, id, 0)
            })
            .collect();
        build_bundle_result(&legs, 0)
    };
    Msg {
        context: msg.context.clone(),
        hop,
        body: obj(body),
        reply_to: String::new(),
    }
}

fn leg_matches(leg: &str, id: &Value) -> bool {
    let id = id.as_str().unwrap_or("");
    match leg.strip_suffix('*') {
        Some(prefix) => id.starts_with(prefix),
        None => id == leg,
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

/// A round in its canonical form, as the policy's `audience_of` writes it: a
/// JSON array of strings, sorted, without duplicates or empty strings, no
/// whitespace, non-ASCII escaped as `\uXXXX` (python's `json.dumps`
/// default). `None` when the text declares no round (empty, not JSON, not an
/// array). Elements that are not strings are written as JSON text (python
/// would take `str(x)`; no test sends one).
pub fn round_canon(round: &str) -> Option<String> {
    let v: Value = sj::from_str(round.trim()).ok()?;
    let mut items: Vec<String> = v
        .as_array()?
        .iter()
        .map(|x| match x {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        })
        .filter(|s| !s.is_empty())
        .collect();
    items.sort();
    items.dedup();
    let text = sj::to_string(&items).expect("serialise");
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            let mut buf = [0u16; 2];
            for unit in c.encode_utf16(&mut buf) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    Some(out)
}

/// The key a round's state rows and its summary slot carry (GH #943): the
/// first 12 hex digits of the sha256 of the canonical round; a round that
/// declares nothing has the key of `[]`.
pub fn round_key(round: &str) -> String {
    let canon = round_canon(round).unwrap_or_else(|| "[]".to_string());
    sha256_hex(&canon)[..12].to_string()
}

/// The `slots` row of the summary leaf of the round `round` (GH #943): the
/// leaf is `history.summary` in the system tree, its row is per round.
pub fn summary_slot(round: &str) -> String {
    format!("history.summary:{}", round_key(round))
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

/// How far the curator tests reach behind the clock: the furthest row any of
/// them places is `now - 60 s` (`consult_keeps_the_task_raw`,
/// `three_sessions`, the rebuild wall). Every backdate goes through
/// [`backdated`], which holds it to this bound, so [`clear_of_midnight`]
/// always waits for the furthest row (GH #1062).
pub const LOOKBACK_S: i64 = 60;

/// `now - back_s`: the base a test places its rows from. A test that calls
/// [`clear_of_midnight`] reaches at most [`LOOKBACK_S`] behind the clock, so
/// a larger backdate is refused here instead of landing on the day before
/// after a midnight wait (GH #1062: the wait ended one second past midnight
/// and a 30 s backdate put the rows at 23:59:31 of the previous day).
pub fn backdated(back_s: i64) -> chrono::DateTime<chrono::Utc> {
    assert!(
        (0..=LOOKBACK_S).contains(&back_s),
        "a backdate of {back_s} s lies outside the {LOOKBACK_S} s clear_of_midnight waits for"
    );
    chrono::Utc::now() - chrono::Duration::seconds(back_s)
}

/// How long a curator test may run after its wait. Alone they need a few
/// seconds, at full-suite load about ten (gate of 2026-10-05: 77-91 s each,
/// an ~80 s wait included); 20 s keeps the longest wait, [`LOOKBACK_S`] +
/// this, inside a third of the nextest budget of 8 x 30 s (GH #1046).
pub const RUN_AHEAD_S: i64 = 20;

/// Seconds to wait at `since_midnight` seconds past midnight UTC so that a
/// test reaching `back_s` behind the clock and running `ahead_s` past it stays
/// on one calendar day (GH #1033): close before midnight, past it AND past the
/// lookback; just after it, past the lookback. At most `back_s + ahead_s`.
pub fn midnight_wait_s(since_midnight: i64, back_s: i64, ahead_s: i64) -> i64 {
    let to_midnight = 86_400 - since_midnight;
    if to_midnight <= ahead_s {
        to_midnight + back_s
    } else if since_midnight < back_s {
        back_s - since_midnight
    } else {
        0
    }
}

/// A test that places rows by calendar day must not straddle midnight UTC:
/// the plan's day is read when the rebuild runs, and the rows lie up to
/// [`LOOKBACK_S`] behind the clock. Near midnight, wait until the whole span
/// fits one day -- a wait for the clock, not for a result, at most
/// [`LOOKBACK_S`] + [`RUN_AHEAD_S`] seconds once a day. GH #1033: it used to
/// wait only until one second past midnight, which put the lookback on the
/// day before.
pub fn clear_of_midnight() {
    use chrono::Timelike;
    let since = i64::from(chrono::Utc::now().num_seconds_from_midnight());
    let wait = midnight_wait_s(since, LOOKBACK_S, RUN_AHEAD_S);
    if wait > 0 {
        std::thread::sleep(std::time::Duration::from_secs(wait as u64));
    }
}

/// GH #1448 (D38 C2, R-38-4 Nr. 2): the keys every building block of the
/// window plan carries -- `{id, kind, prio, origin, tokens, audience,
/// valid_until}`, `origin` as `{source, seq}` or `{source, hash}`.
pub const BLOCK_KEYS: [&str; 7] = [
    "id",
    "kind",
    "prio",
    "origin",
    "tokens",
    "audience",
    "valid_until",
];

/// GH #1448: the id of the building block of `kind` whose origin is the
/// wall element `el` -- the kind and the element's short id
/// (`words:<12 hex>`, `answer:<12 hex>`, `recall:<12 hex>`).
pub fn block_id(kind: &str, el: &Value) -> String {
    format!("{kind}:{}", short_id(el))
}

/// GH #1448: the building blocks of the plan of the round `round`
/// (`state.window_plan:<rk>`, the one plan form, `blocks`), empty when the
/// round has no plan or the plan none.
pub fn plan_blocks(h: &Hive, round: &str) -> Vec<Value> {
    h.plan_in(round)["blocks"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// GH #1448: the ids of `blocks`, in their order.
pub fn block_ids(blocks: &[Value]) -> Vec<String> {
    blocks
        .iter()
        .map(|b| b["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// GH #1448 (R-38-4 Nr. 5): the plan snapshot the ledger keeps for the call
/// `call` (table `snapshots`, column `body`: `[{id, prio, tokens}, ...]`),
/// `None` when the ledger has no such table or no row of the call.
pub fn snapshot_of(h: &Hive, call: &Msg) -> Option<Vec<Value>> {
    let has = h.rows("SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'snapshots'");
    if has.is_empty() {
        return None;
    }
    let id = call.hop["curator_call"].as_str().unwrap_or_default();
    let rows = h.rows(&format!(
        "SELECT body FROM snapshots WHERE call_id = '{id}'"
    ));
    let body = rows.first()?.first()?.as_str()?.to_string();
    sj::from_str::<Value>(&body).ok()?.as_array().cloned()
}

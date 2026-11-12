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
//!
//! D58 lead (GH #1474): the lead is a hive of its own (`templates/lead`)
//! beside the curator. The harness runs two hives and the bridge between
//! them: the curator's edges, the lead's edges, and the edges between
//! `./curator` and `./lead` that `templates/talky/config.json` ships -- taken
//! from that file, never written here, so a pin through the harness checks
//! the shipped wiring. A message the curator hands to its parent (`.`) meets
//! the bridge first; what the bridge takes goes into the lead hive and never
//! into `out`, everything else leaves as before. The lead is wired as talky
//! ships it (`policy.lead_wired` of talky's curator marker); a hive whose
//! override switches it off has no lead hive at all.
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
    // GH #1474: `lead` is the cell of the lead hive, every other one a cell
    // of the curator.
    let cfg = if cell == "lead" {
        lead_config("lead")
    } else {
        cell_config(cell)
    };
    let src = cfg["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string();
    let mut params = cfg["params"].as_object().cloned().unwrap_or_default();
    params.remove("script_inline");
    for (k, v) in over.as_object().cloned().unwrap_or_default() {
        params.insert(k, v);
    }
    let doc = json!({"src": src, "params": params, "probe": probe, "args": args});
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

/// D58 target (contract point 8, OR-d58-2.target.39): the way a call left
/// on before every model call took one route, read off its target under
/// [`LEAD_WIRING`] -- `quick` for the fast voice and its spec and side
/// cells, `brain` for the brain and for a call that names no target (the
/// way of a curate without a stamp), the errand's class for an errand.
/// [`Hive::routed`] finds a call by either name.
pub fn way_of(m: &Msg) -> String {
    let field = |k: &str| {
        m.hop
            .get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    match m.route() {
        "model_call" => match field("target").as_str() {
            "quick" | "quick_spec" | "quick_bridge" => "quick".to_string(),
            "" => "brain".to_string(),
            t => t.to_string(),
        },
        "errand" => field("consult_class"),
        r => r.to_string(),
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

/// One shipped edge, its condition and modifier compiled by the colony's CEL.
fn compile_edge(e: &Value) -> Edge {
    let condition = e["condition"]
        .as_str()
        .map(|c| parse_condition(c).unwrap_or_else(|x| panic!("{c}: {x}")));
    let modifier = if e["modifier"].is_object() {
        let spec: ModifierSpec = sj::from_value(e["modifier"].clone()).expect("a modifier spec");
        Some(parse_modifier(&spec).expect("a modifier"))
    } else {
        None
    };
    Edge {
        from: e["from"].as_str().unwrap().to_string(),
        to: e["to"].as_str().unwrap().to_string(),
        condition,
        modifier,
    }
}

/// The shipped edges (`params.graph.edges`) of the hive config `rel` that
/// `keep` selects, compiled.
fn shipped_edges(rel: &str, keep: impl Fn(&str, &str) -> bool) -> Vec<Edge> {
    read_json(&repo(rel))["params"]["graph"]["edges"]
        .as_array()
        .unwrap_or_else(|| panic!("{rel}: params.graph.edges"))
        .iter()
        .filter(|e| {
            keep(
                e["from"].as_str().unwrap_or(""),
                e["to"].as_str().unwrap_or(""),
            )
        })
        .map(compile_edge)
        .collect()
}

/// Which of the three edge lists a routing step reads: the curator hive's,
/// the lead hive's, or the composite's bridge between the two.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Scope {
    Curator,
    Lead,
    Bridge,
}

/// D58 lead (GH #1474): the composite whose shipped edges wire a lead beside
/// its curator -- the harness takes the bridge and the knob `lead_wired`
/// from it.
pub const LEAD_COMPOSITE: &str = "templates/talky";

/// The places of the two hives inside [`LEAD_COMPOSITE`].
pub const CURATOR_AT: &str = "./curator";
pub const LEAD_AT: &str = "./lead";

/// The template of the lead, a hive of its own.
pub const LEAD_HIVE: &str = "templates/lead";

/// The lane the lead's verdict comes home on, set by the composite's bridge.
pub const VERDICT_LANE: &str = "in_lead";

/// The value of `policy.lead_wired` the composite's curator marker ships
/// (`override_params.policy.lead_wired`), `None` when it sets none.
pub fn lead_wired_shipped() -> Option<Value> {
    let marker = repo(&format!("{LEAD_COMPOSITE}/curator/config.json"));
    if !marker.is_file() {
        return None;
    }
    read_json(&marker)["override_params"]["policy"]
        .get("lead_wired")
        .cloned()
}

/// One switch the way the policy reads it (`switch_on` in `curator/policy`):
/// JSON true, "1", "true" or "on" is on, anything else off.
pub fn switch_on(v: &Value) -> bool {
    let s = match v {
        Value::String(s) => s.trim().to_lowercase(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        _ => return false,
    };
    matches!(s.as_str(), "1" | "true" | "on")
}

/// The config of a cell of the lead hive (`lead` or `state`).
pub fn lead_config(name: &str) -> Value {
    read_json(&repo(&format!("{LEAD_HIVE}/{name}/config.json")))
}

/// The program of the lead cell.
pub fn lead_script() -> String {
    lead_config("lead")["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string()
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
    /// GH #1474: the lead hive's own edges (`templates/lead/config.json`),
    /// empty when no lead is wired.
    lead_edges: Vec<Edge>,
    /// GH #1474: the composite's edges between `./curator` and `./lead`
    /// ([`LEAD_COMPOSITE`]), empty when no lead is wired.
    bridge: Vec<Edge>,
    /// GH #1474: whether a lead hive stands beside the curator (the final
    /// value of `policy.lead_wired`, read as the policy reads it).
    pub wired: bool,
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
    /// GH #1463 (fix round 2): the second entry is the cell the message
    /// comes from -- `.` for a lane, `./summarizer` for a summary that comes
    /// home there ([`Hive::summary_at_phase`]).
    lane_at: Option<(String, String, Msg)>,
    /// GH #1471: every message the store answered, in order -- the cell that
    /// sent it and its `cur_phase`. One entry per message, however many
    /// operations it bundles: the steps a call takes at the store are
    /// counted here.
    pub ledger_steps: Vec<(String, String)>,
    /// D58 feed (GH #1481, review m2): for each entry of `ledger_ops`, the
    /// index in `ledger_steps` of the message that carried it -- two
    /// operations ride one message to the store exactly when their indices
    /// are equal; neighbours in `ledger_ops` may be two messages.
    pub ledger_op_step: Vec<usize>,
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
    /// GH #1447 (D38 C1): the store of the lead (`./state` of the lead hive
    /// since GH #1474), its own in-memory SQLite with the shipped schema and
    /// seed of that cell -- never the ledger. `None` when no lead is wired.
    pub lead_db: Option<rusqlite::Connection>,
    /// GH #1447: every operation the lead's store ran, with the cell that
    /// sent it -- measured at the store, like `ledger_ops`.
    pub lead_ops: Vec<(String, Value)>,
    /// GH #1447: `refuse_legs` for the lead's store -- `(leg, error_code)`,
    /// the store of the lead refuses that leg of the next message carrying it
    /// and runs the others. Taken when it strikes.
    pub lead_refuse_legs: Vec<(String, String)>,
    /// GH #1447 (the join of `win`): `(from, cur_phase)` -- every message
    /// routed inside the curator hive from `from` to `./policy` (and, when
    /// the phase is set, only one whose context `cur_phase` is it) is held in
    /// `held` instead of delivered, until [`Hive::release_held`]. Two arrival
    /// orders of the two halves of a call (ledger legs, lead verdict) are
    /// made this way, with [`Hive::hold_lane`] for the verdict.
    pub hold: Option<(String, Option<String>)>,
    /// GH #1474: every message delivered to `./policy` on this lane
    /// (`hop.route`) is held in `held` the same way -- the lead's verdict,
    /// which reaches the curator over its rim on [`VERDICT_LANE`] now that
    /// the lead is a hive of its own (no inner edge to hold it at).
    pub hold_lane: Option<String>,
    pub held: Vec<Msg>,
    /// D58 target (contract point 13, R-58-15): the policy knobs the test
    /// set, in order, so [`Hive::report_profile`] can move a window value
    /// the test set on the instance into the profile (a set knob beat the
    /// preset of the former role; a profile value beats the knob).
    policy_set: Vec<(String, Value)>,
    /// D58 target (contract point 13): the target that reported its profile
    /// through [`Hive::report_profile`]; a curate on a lane that names no
    /// target names this one, as the curate edge of a composite does.
    pub target: Option<String>,
}

/// D58 target (contract point 13): the model cell a test's curates go to
/// once it reported its profile ([`Hive::report_profile`]); the name the
/// lead's `path_routes` give the path `talk` ([`LEAD_WIRING`]).
pub const MAIN_TARGET: &str = "brain";

/// D58 target: the window values of a profile (the knobs of the former
/// role table, `SHIPPED` of the policy).
pub const WINDOW_KEYS: [&str; 18] = [
    "compress_at",
    "rebuild_to",
    "keep_recent",
    "keep_recent_tokens",
    "horizon",
    "tiers",
    "keep_rounds",
    "gc",
    "gc_scaffold",
    "stub_tools_after",
    "broadcast_mode",
    "recall_push",
    "recall_budget",
    "short_ids",
    "identity_budget",
    "window_target",
    "notes_share",
    "state_lines",
];

/// `patch` merged into `base`: an object key by key (recursively), any
/// other value replaces.
pub fn merge_into(base: &mut Value, patch: &Value) {
    match (base.as_object_mut(), patch.as_object()) {
        (Some(b), Some(p)) => {
            for (k, v) in p {
                match b.get_mut(k) {
                    Some(old) if old.is_object() && v.is_object() => merge_into(old, v),
                    _ => {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        _ => *base = patch.clone(),
    }
}

/// D58 target (contract point 3, Nachtrag 11.11. Nr. 1): the values a
/// call's cut reads, which a target beside the tree-holding one carries.
pub const CALL_KEYS: [&str; 5] = [
    "window_target",
    "state_lines",
    "short_ids",
    "identity_budget",
    "broadcast_mode",
];

/// GH #1447 (D38 C1): the cells of the lead -- since GH #1474 the cells of
/// the lead hive (`templates/lead`): the lead and its store.
pub const LEAD_CELLS: [&str; 2] = ["lead", "state"];

/// D58 target (OR-d58-2.target.39): the lead's knobs as talky's composite
/// sets them -- the targets and errand classes the shipped lead named
/// before it named none. D58 moveout (GH #1476): `bridge_wording`, the form
/// of the side call beside each errand class.
pub const LEAD_WIRING: [(&str, &str); 8] = [
    (
        "path_routes",
        "talk:brain,simple:quick,complex:consult,research:research",
    ),
    ("departures", "consult:10000,research:120000:240000"),
    ("fallback", "quick:brain"),
    ("spec_target", "quick_spec"),
    ("bridge_target", "*:quick,spec:quick_bridge"),
    ("bridge_wording", "consult:soon,research:later"),
    ("errand_tool", "consult_cogny"),
    ("errand_family", "consult"),
];

/// D58 target (contract point 11, OR-d58-2.target.43): the intake's knob as
/// talky's composite sets it -- the errand tool the lead names as well.
pub const INTAKE_WIRING: [(&str, &str); 1] = [("errand_tool", "consult_cogny")];

/// GH #1447: whether the tree ships the lead -- since GH #1474 the template
/// `lead` with its cell `lead`.
pub fn lead_shipped() -> bool {
    repo(&format!("{LEAD_HIVE}/lead/config.json")).is_file()
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
        let edges = shipped_edges("templates/curator/config.json", |_, _| true);
        let mut cells = BTreeMap::new();
        for name in CELLS {
            cells.insert(name.to_string(), cell_config(name));
        }
        // GH #1474: the knob that says a lead is wired, as the composite that
        // wires one ships it (talky: on). A test's own `policy.lead_wired`
        // wins; off, the hive has no lead hive and no bridge.
        if let (Some(v), Some(params)) = (
            lead_wired_shipped(),
            cells
                .get_mut("policy")
                .and_then(|c| c["params"].as_object_mut()),
        ) && params.contains_key("lead_wired")
        {
            params.insert("lead_wired".to_string(), v);
        }
        let wired_value = over
            .iter()
            .rev()
            .find(|(c, k, _)| *c == "policy" && *k == "lead_wired")
            .map(|(_, _, v)| v.clone())
            .unwrap_or_else(|| cells["policy"]["params"]["lead_wired"].clone());
        let wired = switch_on(&wired_value) && lead_shipped();
        let (lead_edges, bridge) = if wired {
            cells.insert("lead".to_string(), lead_config("lead"));
            let lead_edges = shipped_edges(&format!("{LEAD_HIVE}/config.json"), |_, _| true);
            let bridge = shipped_edges(&format!("{LEAD_COMPOSITE}/config.json"), |f, t| {
                (f == CURATOR_AT && t == LEAD_AT) || (f == LEAD_AT && t == CURATOR_AT)
            });
            for (f, t) in [(CURATOR_AT, LEAD_AT), (LEAD_AT, CURATOR_AT)] {
                assert!(
                    bridge.iter().any(|e| e.from == f && e.to == t),
                    "{LEAD_COMPOSITE} wires a lead (lead_wired on) but ships no edge {f} -> {t}"
                );
            }
            (lead_edges, bridge)
        } else {
            (Vec::new(), Vec::new())
        };
        // D58 target (OR-d58-2.target.39): the lead ships without targets;
        // the harness wires it the way talky's composite does, so a test
        // that stamps `simple` still meets the fast target. A test's own
        // override wins.
        if let Some(params) = cells
            .get_mut("lead")
            .and_then(|c| c["params"].as_object_mut())
        {
            for (key, value) in LEAD_WIRING {
                if params.contains_key(key) {
                    params.insert(key.to_string(), json!(value));
                }
            }
        }
        if let Some(params) = cells
            .get_mut("intake")
            .and_then(|c| c["params"].as_object_mut())
        {
            for (key, value) in INTAKE_WIRING {
                if params.contains_key(key) {
                    params.insert(key.to_string(), json!(value));
                }
            }
        }
        for (cell, key, value) in over {
            let params = cells
                .get_mut(*cell)
                .and_then(|c| c["params"].as_object_mut())
                .unwrap_or_else(|| {
                    panic!(
                        "no cell with params: {cell} (the lead stands only where \
                         policy.lead_wired is on, GH #1474)"
                    )
                });
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
        // indexes -> seed) from the params of the lead hive's `./state`
        // (GH #1474), when a lead is wired.
        let lead_db = wired.then(|| {
            let p = meclaw_cells::store::StoreParams::parse(&lead_config("state")["params"])
                .expect("the params of the lead's store parse");
            let db = rusqlite::Connection::open_in_memory().expect("sqlite");
            meclaw_cells::store::ddl::apply_declared_schema_ddl(&db, &p).expect("lead ddl");
            meclaw_cells::store::ddl::apply_index_ddl(&db, &p.indexes).expect("lead index ddl");
            meclaw_cells::store::seed::load_seed_if_present(
                &db,
                &repo(&format!("{LEAD_HIVE}/state")),
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
            lead_edges,
            bridge,
            wired,
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
            ledger_steps: Vec::new(),
            ledger_op_step: Vec::new(),
            check_emits: false,
            emit_violations: Vec::new(),
            clock_us: None,
            runs_since_clock: 0,
            lead_db,
            lead_ops: Vec::new(),
            lead_refuse_legs: Vec::new(),
            hold: None,
            hold_lane: None,
            held: Vec::new(),
            policy_set: over
                .iter()
                .filter(|(cell, _, _)| *cell == "policy")
                .map(|(_, key, value)| ((*key).to_string(), value.clone()))
                .collect(),
            target: None,
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
        // GH #1474: the lead is a cell of its own hive.
        let hive = if name == "lead" { "lead" } else { "curator" };
        let doc = json!({
            "envelope": {"header": {"context": msg.context, "hop": msg.hop},
                         "target": format!("/x/{hive}/{name}"),
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

    fn route(&self, scope: Scope, from: &str, msg: &Msg) -> Vec<(String, Msg)> {
        let edges = match scope {
            Scope::Curator => &self.edges,
            Scope::Lead => &self.lead_edges,
            Scope::Bridge => &self.bridge,
        };
        let mut out = Vec::new();
        for e in edges.iter().filter(|e| e.from == from) {
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
        let mut queue: VecDeque<(Scope, String, Msg)> = starts
            .into_iter()
            .map(|(from, msg)| (Scope::Curator, from, msg))
            .collect();
        let mut steps = 0;
        while let Some((scope, from, msg)) = queue.pop_front() {
            steps += 1;
            assert!(steps < 4000, "the hive does not come to rest");
            if scope == Scope::Lead {
                self.step_in_lead(&from, &msg, &mut queue);
                continue;
            }
            for (to, m) in self.route(scope, &from, &msg) {
                match to.as_str() {
                    // GH #1474: what leaves the curator meets the composite's
                    // bridge first; what it takes enters the lead hive at its
                    // rim and is no output of the curator.
                    "." if self.wired => {
                        let bridged = self.route(Scope::Bridge, CURATOR_AT, &m);
                        if bridged.is_empty() {
                            self.out.push(m);
                        }
                        for (to, b) in bridged {
                            assert_eq!(to, LEAD_AT, "the bridge leads into the lead");
                            queue.push_back((Scope::Lead, ".".to_string(), b));
                        }
                    }
                    "." => self.out.push(m),
                    "./ledger" => {
                        let phase = m.context.get("cur_phase").and_then(Value::as_str);
                        if self
                            .lane_at
                            .as_ref()
                            .is_some_and(|(at, _, _)| Some(at.as_str()) == phase)
                            && let Some((_, origin, lane)) = self.lane_at.take()
                        {
                            self.pump(&origin, lane);
                        }
                        let sender = from.trim_start_matches("./").to_string();
                        self.ledger_steps
                            .push((sender.clone(), phase.unwrap_or("").to_string()));
                        for c in m.messages().iter().filter(|c| c["type"] == "tool_call") {
                            let args = sj::from_str(c["text"].as_str().unwrap_or(""))
                                .unwrap_or(Value::Null);
                            self.ledger_ops.push((sender.clone(), args));
                            self.ledger_op_step.push(self.ledger_steps.len() - 1);
                        }
                        let answer = self.store(&m);
                        queue.push_back((Scope::Curator, "./ledger".to_string(), answer));
                    }
                    "./policy"
                        if self.hold.as_ref().is_some_and(|(f, p)| {
                            *f == from
                                && p.as_deref().is_none_or(|p| {
                                    m.context.get("cur_phase").and_then(Value::as_str) == Some(p)
                                })
                        }) || self
                            .hold_lane
                            .as_deref()
                            .is_some_and(|lane| m.route() == lane) =>
                    {
                        self.held.push(m)
                    }
                    "./clock" => self.clock.push(m),
                    "./summarizer" => self.summ.push_back(m),
                    cell => {
                        let name = cell.trim_start_matches("./").to_string();
                        for o in self.run_cell(&name, &m) {
                            queue.push_back((Scope::Curator, cell.to_string(), o));
                        }
                    }
                }
            }
        }
    }

    /// GH #1474: one routing step inside the lead hive, from `from` (`.` is
    /// its rim, a message the bridge brought in): its edges into the lead or
    /// its store, and out over its rim, where the composite's bridge carries
    /// the verdict home to the curator's rim. A message the bridge does not
    /// take would be a dead letter of the composite; it fails the test.
    fn step_in_lead(&mut self, from: &str, msg: &Msg, queue: &mut VecDeque<(Scope, String, Msg)>) {
        for (to, m) in self.route(Scope::Lead, from, msg) {
            match to.as_str() {
                "." => {
                    let bridged = self.route(Scope::Bridge, LEAD_AT, &m);
                    assert!(
                        !bridged.is_empty(),
                        "the lead hive hands {:?} to {LEAD_COMPOSITE}, and no edge \
                         {LEAD_AT} -> {CURATOR_AT} takes it (a dead letter)",
                        m.route()
                    );
                    for (to, b) in bridged {
                        assert_eq!(to, CURATOR_AT, "the bridge leads into the curator");
                        queue.push_back((Scope::Curator, ".".to_string(), b));
                    }
                }
                "./state" => {
                    let sender = from.trim_start_matches("./").to_string();
                    for c in m.messages().iter().filter(|c| c["type"] == "tool_call") {
                        let args =
                            sj::from_str(c["text"].as_str().unwrap_or("")).unwrap_or(Value::Null);
                        self.lead_ops.push((sender.clone(), args));
                    }
                    let answer = store_plain(
                        self.lead_db.as_ref().expect("lead db"),
                        &m,
                        &mut self.lead_refuse_legs,
                    );
                    queue.push_back((Scope::Lead, "./state".to_string(), answer));
                }
                "./lead" => {
                    for o in self.run_cell("lead", &m) {
                        queue.push_back((Scope::Lead, "./lead".to_string(), o));
                    }
                }
                other => panic!("the lead hive routes to {other}, a cell it does not have"),
            }
        }
    }

    /// A message arriving on the hive path on lane `route`.
    pub fn lane(&mut self, route: &str, context: Value, hop: Value, body: Value) {
        let msg = self.named_lane_msg(route, context, hop, body);
        self.pump(".", msg);
    }

    /// D58 target (contract point 13, R-58-15): the model cell
    /// [`MAIN_TARGET`] reports its package on `in_catalog` -- a row naming
    /// only its model (the window stays the one the answers report) and
    /// `profile` beside it (`support/curator_profiles.rs` holds the five
    /// value sets of the former roles). A window value the test set as a
    /// policy knob moves into the profile: under a role the set knob beat
    /// the role's preset, and a profile value beats the knob (contract point
    /// 3), so the window stays the one of before. From here on a curate that
    /// names no target names this one.
    pub fn report_profile(&mut self, profile: Value) {
        let profile = self.moved_into(profile, &WINDOW_KEYS);
        self.report_package(MAIN_TARGET, profile, json!([]));
        self.target = Some(MAIN_TARGET.to_string());
    }

    /// D58 target (contract point 13, OR-d58-2.target.60 c): every model
    /// cell of the shipped composite `templates/<comp>` reports its package
    /// with the profile and the order of the system part its template
    /// writes, as the registry's push after the birth does -- the curator of
    /// that composite, the way the corpus runs it. The window values the
    /// test set as policy knobs move in: every one into the profile of the
    /// cell that holds its tree (the main target), the values a call's cut
    /// reads ([`CALL_KEYS`]) also into the others. From here on a curate
    /// that names no target names the main one.
    pub fn report_composite(&mut self, comp: &str) {
        self.report_composite_with(comp, &[]);
    }

    /// [`Hive::report_composite`] with `patches` -- `(cell, fields)` merged
    /// into that cell's profile (an object into an object, key by key):
    /// what a test sets on one model cell of the composite, e.g. the
    /// `instructions.only` or the `allow` list that replaced a knob of the
    /// curator (OR-d58-2.target.62).
    pub fn report_composite_with(&mut self, comp: &str, patches: &[(&str, Value)]) {
        let dir = repo(&format!("templates/{comp}"));
        let mut cells: Vec<String> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
            .filter_map(|e| e.ok())
            .filter(|e| e.path().join("config.json").is_file())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        cells.sort();
        let mut main = None;
        for cell in cells {
            let params =
                read_json(&repo(&format!("templates/{comp}/{cell}/config.json")))["params"].clone();
            let mut profile = params["window_profile"].clone();
            if !profile.as_object().is_some_and(|p| !p.is_empty()) {
                continue;
            }
            for (_, patch) in patches.iter().filter(|(c, _)| *c == cell) {
                merge_into(&mut profile, patch);
            }
            let holds = profile["system"] == json!("delta");
            let profile = self.moved_into(profile, if holds { &WINDOW_KEYS } else { &CALL_KEYS });
            self.report_package(&cell, profile, params["system_order"].clone());
            if holds {
                assert!(main.is_none(), "{comp}: one cell holds the tree");
                main = Some(cell);
            }
        }
        self.target = Some(main.unwrap_or_else(|| panic!("{comp}: no cell holds the tree")));
    }

    /// D58 target (contract point 13): the model cell `cell` of the shipped
    /// composite `templates/<comp>` reports its package with the catalogue
    /// row `row` -- a cell's package line carries row and profile together,
    /// so the profile and the order its template writes ride along (the
    /// test's window knobs moved in as in [`Hive::report_composite`]).
    pub fn report_row(&mut self, comp: &str, cell: &str, row: Value) {
        let params =
            read_json(&repo(&format!("templates/{comp}/{cell}/config.json")))["params"].clone();
        let profile = params["window_profile"].clone();
        let holds = profile["system"] == json!("delta");
        let profile = self.moved_into(profile, if holds { &WINDOW_KEYS } else { &CALL_KEYS });
        self.lane(
            "in_catalog",
            json!({}),
            json!({"package_of": format!("/{comp}/{cell}")}),
            json!({"messages": [], "package": row, "window_profile": profile,
                   "system_order": params["system_order"].clone()}),
        );
    }

    /// `profile` with every policy knob of `keys` the test set (and did not
    /// leave blank) written over it.
    fn moved_into(&self, profile: Value, keys: &[&str]) -> Value {
        let mut profile = profile;
        if let Some(p) = profile.as_object_mut() {
            for (key, value) in &self.policy_set {
                let unset = value.is_null() || value.as_str().is_some_and(|v| v.trim().is_empty());
                if keys.contains(&key.as_str()) && !unset {
                    p.insert(key.clone(), value.clone());
                }
            }
        }
        profile
    }

    /// The package line of the model cell `cell` of the composite `/comp`.
    fn report_package(&mut self, cell: &str, profile: Value, system_order: Value) {
        self.lane(
            "in_catalog",
            json!({}),
            json!({"package_of": format!("/comp/{cell}")}),
            json!({"messages": [], "package": {"model": format!("test/{cell}")},
                   "window_profile": profile, "system_order": system_order}),
        );
    }

    /// [`Hive::report_profile`] on a hive just built: `Hive::with(..)
    /// .reporting(profile("talky"))` is the hive whose main target runs the
    /// example set `talky`.
    pub fn reporting(mut self, profile: Value) -> Self {
        self.report_profile(profile);
        self
    }

    /// [`Hive::report_composite`] on a hive just built.
    pub fn reporting_composite(mut self, comp: &str) -> Self {
        self.report_composite(comp);
        self
    }

    /// [`Hive::report_composite_with`] on a hive just built.
    pub fn reporting_composite_with(mut self, comp: &str, patches: &[(&str, Value)]) -> Self {
        self.report_composite_with(comp, patches);
        self
    }

    /// D58 target (contract point 1): the model cell [`MAIN_TARGET`]
    /// reports its package without a profile, and every curate that names
    /// no target names it -- a bare curator whose composite names the
    /// target of its curates (no `target_unknown` mark), cut as before on
    /// the shipped fields and the instance's values.
    pub fn name_main_target(&mut self) {
        self.report_package(MAIN_TARGET, json!({}), json!([]));
        self.target = Some(MAIN_TARGET.to_string());
    }

    /// [`Hive::lane_msg`], and a curate that names no target names the one
    /// that reported its profile ([`Hive::report_profile`]).
    fn named_lane_msg(&self, route: &str, context: Value, hop: Value, body: Value) -> Msg {
        let mut msg = Self::lane_msg(route, context, hop, body);
        if let Some(t) = &self.target
            && route == "in_curate"
            && !msg.hop.contains_key("target")
        {
            msg.hop.insert("target".into(), json!(t));
        }
        msg
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
        self.lane_at = Some((
            phase.to_string(),
            ".".to_string(),
            self.named_lane_msg(route, context, hop, body),
        ));
    }

    /// Several messages arriving on the hive path at once, `(route, context,
    /// hop, body)` each, run to rest through ONE FIFO queue (GH #1225): two
    /// slot writes in flight both read before either writes, as two
    /// deliveries do.
    pub fn lanes_together(&mut self, lanes: Vec<(&str, Value, Value, Value)>) {
        let starts = lanes
            .into_iter()
            .map(|(route, context, hop, body)| {
                (
                    ".".to_string(),
                    self.named_lane_msg(route, context, hop, body),
                )
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

    /// GH #1463 (fix round 2): the summarizer's answer to the oldest request
    /// it holds arrives right before the store answers the next ledger
    /// message of `phase` and runs to rest there (see `lane_at`): the plan
    /// of a rebuild written between two steps of a call that is being built.
    pub fn summary_at_phase(&mut self, phase: &str, text: &str) {
        let req = self.summ.pop_front().expect("a summarizer request");
        let msg = Msg {
            context: req.context.clone(),
            hop: obj(json!({"finish_reason": "stop", "model": "summary-model"})),
            body: obj(json!({"messages": [{"origin": "assistant", "type": "text", "text": text}]})),
            reply_to: String::new(),
        };
        self.lane_at = Some((phase.to_string(), "./summarizer".to_string(), msg));
    }

    /// Whether a message set with [`Hive::summary_at_phase`] or
    /// [`Hive::lane_at_phase`] still waits for its phase.
    pub fn waits_at_phase(&self) -> bool {
        self.lane_at.is_some()
    }

    /// GH #1471: every parked row that names the time it was parked at
    /// (`pending:*` with `parked_at`) is `days` days old from here on -- the
    /// age the sweep of parked rows judges by, without waiting for it.
    /// Returns the rows it aged, `(key, value before)`.
    pub fn age_parked_rows(&mut self, days: i64) -> Vec<(String, String)> {
        let old = days_ago(days, 12)
            .format("%Y-%m-%dT%H:%M:%S%.6fZ")
            .to_string();
        let mut aged = Vec::new();
        for r in self.rows("SELECT key, value FROM state WHERE key LIKE 'pending:%'") {
            let (key, value) = (
                r[0].as_str().unwrap_or("").to_string(),
                r[1].as_str().unwrap_or("").to_string(),
            );
            let Ok(Value::Object(mut v)) = sj::from_str::<Value>(&value) else {
                continue;
            };
            if !v.contains_key("parked_at") {
                continue;
            }
            v.insert("parked_at".into(), json!(old));
            self.db
                .execute(
                    "UPDATE state SET value = ?1 WHERE key = ?2",
                    (canonical(&Value::Object(v)), key.clone()),
                )
                .expect("the parked row");
            aged.push((key, value));
        }
        aged
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
        self.hold_lane = None;
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

    /// GH #1447: rows of the lead's own store (`./state` of the lead hive).
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
            .filter(|m| m.route() == route || way_of(m) == route)
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
        self.tap_body(call, finish, extra, json!({"messages": messages}));
    }

    /// The same with the answer's whole body, for what the model cell says
    /// beside its messages (`meta`): an edge sets and deletes header keys
    /// only, so the body reaches the tap as the model cell wrote it.
    pub fn tap_body(&mut self, call: &Msg, finish: &str, extra: Value, body: Value) {
        let mut ctx = call.context.clone();
        for k in ["curator_call", "turn_id", "session_id", "iter"] {
            ctx.insert(k.into(), call.hop[k].clone());
        }
        // talky's `./curator -> ./quick` edge lifts `hop.interim` too
        // (GH #1260), and a side call's `hop.round_open` "1" onto it (D58
        // moveout, contract point 4); a call without either carries none.
        if call.hop.get("round_open").is_some_and(|v| *v == "1") {
            ctx.insert("interim".into(), json!("1"));
        } else if let Some(interim) = call.hop.get("interim") {
            ctx.insert("interim".into(), interim.clone());
        }
        // D58 moveout (contract point 3): the edge lifts the kind of a call
        // (`hop.call_kind`) into the context, where the curator reads it.
        if let Some(kind) = call.hop.get("call_kind") {
            ctx.insert("call_kind".into(), kind.clone());
        }
        let mut hop = obj(json!({"finish_reason": finish, "model": "test-model"}));
        for (k, v) in obj(extra) {
            hop.insert(k, v);
        }
        self.lane("in_llm", Value::Object(ctx), Value::Object(hop), body);
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

/// GH #1462 (D58 snap): the list `sent` the ledger keeps for the call `call`
/// (table `snapshots`, column `sent`: `[{hash, kind, form, bytes}, ...]`, one
/// entry per element in the order it was sent). `None` when the table has no
/// such column, the call no row, or the row no list (`snapshot_window` off).
pub fn sent_of(h: &Hive, call: &Msg) -> Option<Vec<Value>> {
    let has = h.rows("SELECT name FROM pragma_table_info('snapshots') WHERE name = 'sent'");
    if has.is_empty() {
        return None;
    }
    let id = call.hop["curator_call"].as_str().unwrap_or_default();
    let rows = h.rows(&format!(
        "SELECT sent FROM snapshots WHERE call_id = '{id}'"
    ));
    let text = rows.first()?.first()?.as_str()?.to_string();
    let list = sj::from_str::<Value>(&text).ok()?.as_array().cloned()?;
    if list.is_empty() { None } else { Some(list) }
}

/// GH #1462: the forms an entry of `sent` may name.
pub const SENT_FORMS: [&str; 6] = ["raw", "stub", "released", "expired", "cut", "built"];

/// GH #1462: a window put together again from a `sent` list and the blocks
/// of the ledger alone -- nothing of the call itself is read.
#[derive(Debug, Default)]
pub struct SentWindow {
    /// The leaves of the system part, `(path, text)`, sorted by path: the
    /// whole tree the model holds for the call.
    pub leaves: Vec<(String, String)>,
    /// The leaves among them whose entry says `carried`: the ones this call
    /// carried to the model cell, sorted by path.
    pub carried: Vec<(String, String)>,
    /// The messages, in the order sent.
    pub messages: Vec<Value>,
    /// The tool menu (the names the call may use), when the list holds one.
    pub tools: Option<Value>,
    /// The pieces listed beside the element that carries them (the state
    /// line, a note): `(kind, form, text)`, in their order.
    pub pieces: Vec<(String, String, String)>,
    /// GH #1463: the elements the call left out and says so -- an entry with
    /// `why` (the reason) and the form `cut`: `(kind, why, element)`, in
    /// their order. They are no part of the window sent.
    pub left_out: Vec<(String, String, Value)>,
}

/// GH #1462: [`SentWindow`] of `sent`. Every entry carries exactly the keys
/// `hash`, `kind`, `form` and `bytes`, an entry of the system part `carried`
/// (a bool) beside them and no other entry, an element the call left out
/// `why` (GH #1463, see [`SentWindow::left_out`]); `hash` names a block the ledger
/// holds, the sha256 of its body, and `bytes` is the length of that body in
/// UTF-8. What a block is decides its body: an element with an `origin` is a
/// message, one with a `path` a leaf of the system part, a list the tool
/// menu, anything else a piece of the element that follows it.
pub fn sent_window(h: &Hive, sent: &[Value]) -> SentWindow {
    let mut w = SentWindow::default();
    for e in sent {
        let keys: Vec<&str> = e
            .as_object()
            .map(|m| m.keys().map(String::as_str).collect())
            .unwrap_or_default();
        assert_eq!(
            keys.iter()
                .filter(|k| **k != "carried" && **k != "why")
                .copied()
                .collect::<Vec<_>>(),
            ["bytes", "form", "hash", "kind"],
            "the keys of {e}"
        );
        let hash = e["hash"].as_str().unwrap_or_default();
        let kind = e["kind"].as_str().unwrap_or_default().to_string();
        let form = e["form"].as_str().unwrap_or_default().to_string();
        assert!(!kind.is_empty(), "an entry names its kind: {e}");
        assert!(SENT_FORMS.contains(&form.as_str()), "a known form: {e}");
        assert!(
            hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()),
            "a block hash: {e}"
        );
        let rows = h.rows(&format!("SELECT body FROM blocks WHERE hash = '{hash}'"));
        let body = rows
            .first()
            .and_then(|r| r[0].as_str())
            .unwrap_or_else(|| panic!("the ledger holds the block of {e}"))
            .to_string();
        assert_eq!(
            sha256_hex(&body),
            hash,
            "the block is content-addressed: {e}"
        );
        assert_eq!(
            e["bytes"].as_u64(),
            Some(body.len() as u64),
            "`bytes` is the length of the block: {e}"
        );
        let el: Value = sj::from_str(&body).unwrap_or_else(|x| panic!("{e}: {x}"));
        let is_leaf = !el.is_array() && el.get("origin").is_none() && el.get("path").is_some();
        assert_eq!(
            e.get("carried").is_some_and(Value::is_boolean),
            is_leaf,
            "`carried` stands at every entry of the system part and at no other: {e}"
        );
        if let Some(why) = e.get("why") {
            // GH #1463: left out by a decision of the curator. The entry
            // names the block of the wall and the reason; nothing was sent.
            assert!(
                why.as_str().is_some_and(|s| !s.is_empty()),
                "`why` names the reason: {e}"
            );
            assert_eq!(form, "cut", "an element left out has the form `cut`: {e}");
            assert!(
                el.get("origin").is_some(),
                "only a message is left out with a reason: {e}"
            );
            w.left_out
                .push((kind, why.as_str().unwrap_or_default().to_string(), el));
            continue;
        }
        if el.is_array() {
            assert!(w.tools.is_none(), "one tool menu: {sent:?}");
            w.tools = Some(el);
        } else if el.get("origin").is_some() {
            w.messages.push(el);
        } else if el.get("path").is_some() {
            let leaf = (
                el["path"].as_str().unwrap_or_default().to_string(),
                el["text"].as_str().unwrap_or_default().to_string(),
            );
            if e["carried"] == Value::Bool(true) {
                w.carried.push(leaf.clone());
            }
            w.leaves.push(leaf);
        } else {
            w.pieces.push((
                kind,
                form,
                el["text"].as_str().unwrap_or_default().to_string(),
            ));
        }
    }
    w.leaves.sort();
    w.carried.sort();
    w
}

/// GH #1462: the leaves of a `system` tree as a call carries it, `(path,
/// text)` sorted by path -- a family root is one `$replace` node, a leaf is
/// `{"text": ...}` at its path (the root's own text at the family name).
pub fn system_leaves(system: &Value) -> Vec<(String, String)> {
    fn walk(path: &str, node: &Value, out: &mut Vec<(String, String)>) {
        for (k, v) in node.as_object().cloned().unwrap_or_default() {
            if k == "$replace" {
                continue;
            }
            if k == "text" && v.is_string() {
                out.push((path.to_string(), v.as_str().unwrap_or_default().to_string()));
            } else if v.is_object() {
                let below = if path.is_empty() {
                    k
                } else {
                    format!("{path}.{k}")
                };
                walk(&below, &v, out);
            }
        }
    }
    let mut out = Vec::new();
    walk("", system, &mut out);
    out.sort();
    out
}

/// GH #1462: what `call_blocks` says the call `call` carried, as `(leaves,
/// messages)` in the shapes of [`SentWindow`] -- the record of before the
/// `sent` list, for a call that has one (a bridge has none).
pub fn listed_window(h: &Hive, call: &Msg) -> (Vec<(String, String)>, Vec<Value>) {
    let id = call.hop["curator_call"].as_str().expect("a call id");
    let (mut leaves, mut messages) = (Vec::new(), Vec::new());
    for r in h.rows(&format!(
        "SELECT b.body FROM call_blocks cb JOIN blocks b ON b.hash = cb.hash \
         WHERE cb.call_id = '{id}' ORDER BY cb.pos"
    )) {
        let el: Value = sj::from_str(r[0].as_str().unwrap_or("null")).unwrap_or(Value::Null);
        if el.get("origin").is_some() {
            messages.push(el);
        } else if el.get("path").is_some() {
            leaves.push((
                el["path"].as_str().unwrap_or_default().to_string(),
                el["text"].as_str().unwrap_or_default().to_string(),
            ));
        }
    }
    leaves.sort();
    (leaves, messages)
}

/// GH #1462: every call the hive sent to a model since `out` was cleared --
/// a message with a `curator_call` -- the call itself first, then its side
/// call (`call_kind` "side") when it has one. Whatever way the call took.
pub fn calls_out(h: &Hive) -> (Msg, Option<Msg>) {
    let is_bridge = |m: &Msg| m.hop.get("call_kind").is_some_and(|v| *v == "side");
    let all: Vec<Msg> = h
        .out
        .iter()
        .filter(|m| {
            m.hop.get("curator_call").is_some_and(Value::is_string)
                && m.body.contains_key("messages")
        })
        .cloned()
        .collect();
    let call = all
        .iter()
        .find(|m| !is_bridge(m))
        .cloned()
        .unwrap_or_else(|| panic!("a call: {:?} {:?}", h.out, h.stderr));
    (call, all.iter().find(|m| is_bridge(m)).cloned())
}

/// GH #1463 (D58 form): the stamp of an answering model with a package and a
/// cache that stays warm for the whole test -- no limit orders a rebuild,
/// and the policy writes its `curate` mark (it does so only with limits).
pub fn warm_usage() -> Value {
    json!({"tokens_prompt": 2000, "tokens_cached": 1000,
           "input_soft": 5_000_000, "input_hard": 10_000_000,
           "cost_in": 10, "cost_cached_in": 0,
           "cache_expires_at": "2099-01-01T00:00:00Z"})
}

/// GH #1471: the state of the plan the call `call` was cut by, as its own
/// snapshot row names it (`cut.plan_epoch`, written with `snapshot_window`
/// on); `""` for a call without such a row.
pub fn cut_epoch(h: &Hive, call: &Msg) -> String {
    let id = call.hop["curator_call"].as_str().unwrap_or_default();
    h.rows(&format!("SELECT cut FROM snapshots WHERE call_id = '{id}'"))
        .first()
        .and_then(|r| r[0].as_str().map(str::to_string))
        .and_then(|text| sj::from_str::<Value>(&text).ok())
        .and_then(|cut| cut["plan_epoch"].as_str().map(str::to_string))
        .unwrap_or_default()
}

/// GH #1471: the state of the plan that stands for the audience `round`
/// right now, in the form [`cut_epoch`] reads off a call: the stamp of the
/// rebuild that made it and its cover, `0:0` before the first rebuild.
pub fn standing_epoch(h: &Hive, round: &str) -> String {
    let plan = h.plan_in(round);
    format!(
        "{}:{}",
        plan["as_of_ms"].as_i64().unwrap_or(0),
        plan["cover"].as_i64().unwrap_or(0)
    )
}

/// GH #1471: the row the turn `turn` of `session` keeps in the audience
/// `round` -- what its first call was cut by (`plan`, `cover`, `leaves`,
/// `legend`) --, `null` while no such row stands.
pub fn turn_cut_row(h: &Hive, round: &str, session: &str, turn: &str) -> Value {
    let key = format!("pending:turn_cut:{}:{session}:{turn}", round_key(round));
    sj::from_str(&h.state(&key)).unwrap_or(Value::Null)
}

/// GH #1463: the window of the call `call` as its snapshot row lists it
/// (`sent`), put together from the ledger alone.
pub fn window_of(h: &Hive, call: &Msg) -> SentWindow {
    let sent = sent_of(h, call).unwrap_or_else(|| {
        panic!(
            "the snapshot row of call {} carries `sent`",
            call.hop["curator_call"]
        )
    });
    sent_window(h, &sent)
}

/// GH #1463: one call of a conversation the form pins drive -- the call, the
/// turn it belongs to, and how many of its messages stand before its running
/// round (the place the model cell's second cache mark stands at).
#[derive(Debug, Clone)]
pub struct Sent {
    pub call: Msg,
    pub turn: String,
    pub stable: usize,
}

/// GH #1463: the prefix rule of the form between two consecutive calls `a`
/// and `b` of one target, read off their snapshot rows. The system part is
/// the same tree, on every entry (never only the carried ones). Within a
/// round `b` begins with the whole window of `a`; across the round boundary
/// with the messages of `a` before its running round. `Err` says where the
/// rule breaks.
pub fn begins_with(h: &Hive, a: &Sent, b: &Sent) -> Result<(), String> {
    let (wa, wb) = (window_of(h, &a.call), window_of(h, &b.call));
    if wa.leaves != wb.leaves {
        return Err(format!(
            "the system part moved between {} and {}: {:?} / {:?}",
            a.turn, b.turn, wa.leaves, wb.leaves
        ));
    }
    let n = if a.turn == b.turn {
        wa.messages.len()
    } else {
        a.stable
    };
    for i in 0..n {
        if wb.messages.get(i) != Some(&wa.messages[i]) {
            return Err(format!(
                "{} -> {}: message {i} of {n} differs: {} / {}",
                a.turn,
                b.turn,
                wa.messages[i],
                wb.messages.get(i).cloned().unwrap_or(Value::Null)
            ));
        }
    }
    Ok(())
}

/// GH #1463: a conversation on one session of the round [`ROUND_E`], every
/// call recorded for [`begins_with`].
pub struct Talk {
    pub h: Hive,
    pub sent: Vec<Sent>,
    /// The breaks of the prefix rule, judged as each call left (the ledger
    /// keeps a round's newest snapshot rows only).
    pub broken: Vec<String>,
    /// How many elements the calls left out with a reason, all told.
    pub left_out: usize,
}

impl Talk {
    pub fn new(over: &[(&str, &str, Value)]) -> Self {
        Self {
            h: Hive::with(over),
            sent: Vec::new(),
            broken: Vec::new(),
            left_out: 0,
        }
    }

    /// D58 target (contract point 13): [`Hive::report_profile`] on the
    /// conversation's hive before its first turn.
    pub fn reporting(mut self, profile: Value) -> Self {
        self.h.report_profile(profile);
        self
    }

    /// [`Hive::report_composite`] before the first turn.
    pub fn reporting_composite(mut self, comp: &str) -> Self {
        self.h.report_composite(comp);
        self
    }

    /// GH #1471: a conversation with the form on (`recall_in_wall`,
    /// `snapshot_window`) and `keep_recent` 2, `n` turns told: turn `t<i>`
    /// says "The person tells item <i>." and is answered "Noted item <i>.".
    /// After six of them a rebuild covers the oldest.
    pub fn told(n: usize) -> Self {
        let mut t = Self::new(&[
            ("policy", "recall_in_wall", json!("1")),
            ("policy", "snapshot_window", json!("1")),
            ("policy", "keep_recent", json!(2)),
        ]);
        for i in 1..=n {
            t.turn(
                &format!("t{i}"),
                &format!("The person tells item {i}."),
                &format!("Noted item {i}."),
            );
        }
        t
    }

    /// GH #1471: how far the plan that stands covers the wall; 0 before the
    /// first rebuild that covers anything.
    pub fn cover(&self) -> i64 {
        self.h.rebuild_in(ROUND_E)["cover"].as_i64().unwrap_or(0)
    }

    /// GH #1471: the clock strikes with the newest order it holds; a
    /// summarizer that is asked answers `summary` at once.
    pub fn strike(&mut self, summary: &str) {
        let order = self.h.clock_order_for(ROUND_E).unwrap_or_else(|| {
            panic!(
                "check of the setup: an order at the clock: {:?}",
                self.h.clock
            )
        });
        self.h.fire(&order);
        if !self.h.summ.is_empty() {
            self.h.answer(summary, "stop");
        }
    }

    /// Call `iter` of the turn `turn` with the elements `round`; the first
    /// call of a turn brings the person's words (and what came with them).
    pub fn call(&mut self, turn: &str, iter: u32, round: Value) -> Msg {
        let brought = round.as_array().map(Vec::len).unwrap_or(0);
        let call = self.h.curate("s1", turn, iter, round, mode("Be brief."));
        let stable = match self.sent.last() {
            Some(last) if last.turn == turn => last.stable,
            _ => call.messages().len().saturating_sub(brought),
        };
        self.sent.push(Sent {
            call: call.clone(),
            turn: turn.to_string(),
            stable,
        });
        // Without the snapshot of the window there is nothing to judge.
        if let Some(sent) = sent_of(&self.h, &call) {
            self.left_out += sent_window(&self.h, &sent).left_out.len();
            if let [.., a, b] = self.sent.as_slice()
                && let Err(why) = begins_with(&self.h, a, b)
            {
                self.broken.push(why);
            }
        }
        call
    }

    /// The model's answer to `call`: `finish` and its elements.
    pub fn answer(&mut self, call: &Msg, finish: &str, messages: Value) {
        self.h.tap(call, finish, warm_usage(), messages);
    }

    /// A whole turn without a tool: the words, the final answer.
    pub fn turn(&mut self, turn: &str, words: &str, reply: &str) -> Msg {
        let c = self.call(turn, 0, json!([user(words)]));
        self.answer(&c, "stop", json!([said(reply)]));
        c
    }

    /// Every break of the prefix rule between consecutive calls, in order.
    pub fn breaks(&self) -> Vec<String> {
        self.broken.clone()
    }

    /// The figure `window` of the hive's own `stats` door over the last day
    /// and the next.
    pub fn window_figure(&mut self) -> Value {
        let now = chrono::Utc::now();
        let day = chrono::Duration::days(1);
        self.h.out.clear();
        self.h.lane(
            "in_stats",
            json!({}),
            json!({"stats_tag": "form:1",
                   "stats_from": (now - day).format("%Y-%m-%dT%H:%M:%SZ").to_string(),
                   "stats_to": (now + day).format("%Y-%m-%dT%H:%M:%SZ").to_string(),
                   "stats_kinds": "[\"window\"]"}),
            json!({"messages": []}),
        );
        assert_eq!(self.h.out.len(), 1, "one answer: {:?}", self.h.out);
        let m = &self.h.out[0];
        assert_eq!(m.hop["error_code"], "", "{:?}", self.h.stderr);
        m.body["stats"]["counts"]["window"].clone()
    }

    /// Sabotage: the plan of the round forgets what it decided -- no block
    /// says `decided`, no element is `left_out`. Every call then checks every
    /// recall block again, the cut per call of before the form.
    pub fn forget_decisions(&self) {
        let mut plan = self.h.plan_in(ROUND_E);
        let Some(m) = plan.as_object_mut() else {
            return;
        };
        m.remove("left_out");
        if let Some(blocks) = m.get_mut("blocks").and_then(Value::as_array_mut) {
            for b in blocks {
                if let Some(b) = b.as_object_mut() {
                    b.remove("decided");
                }
            }
        }
        self.write_plan(&plan);
    }

    /// GH #1471: `plan` as the plan of the round -- and as the plan of every
    /// turn of the round that keeps its cut right now: a call in the middle
    /// of a turn is cut by the row of its turn, not by the plan that stands,
    /// so a sabotage of the plan alone would reach the next turn only.
    fn write_plan(&self, plan: &Value) {
        let key = format!("window_plan:{}", round_key(ROUND_E));
        self.h
            .db
            .execute(
                "UPDATE state SET value = ?1 WHERE key = ?2",
                (canonical(plan), key),
            )
            .expect("the plan row");
        let like = format!("pending:turn_cut:{}:%", round_key(ROUND_E));
        for r in self.h.rows(&format!(
            "SELECT key, value FROM state WHERE key LIKE '{like}'"
        )) {
            let (Some(key), Some(text)) = (r[0].as_str(), r[1].as_str()) else {
                continue;
            };
            let Ok(Value::Object(mut row)) = sj::from_str::<Value>(text) else {
                continue;
            };
            row.insert("plan".into(), json!(canonical(plan)));
            self.h
                .db
                .execute(
                    "UPDATE state SET value = ?1 WHERE key = ?2",
                    (canonical(&Value::Object(row)), key),
                )
                .expect("the row of the turn");
        }
    }

    /// Sabotage: the plan of the round lists the wall element `el` as left
    /// out -- the next call sends its window without it, although it stands
    /// in the raw part and no summary holds it.
    pub fn drop_from_the_window(&self, el: &Value) {
        let mut plan = self.h.plan_in(ROUND_E);
        let hash = sha256_hex(&canonical(el));
        let m = plan.as_object_mut().expect("a plan");
        let mut out = m
            .get("left_out")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        out.push(json!(hash));
        m.insert("left_out".into(), Value::Array(out));
        self.write_plan(&plan);
    }
}

//! D58 followup (GH #1479): the open concern of a person moves out of the
//! curator into the hive `assistant`, the companion domain of core.
//!
//! WHY this file exists: until this strand the curator kept a rule about a
//! person's wellbeing (`wellbeing_line` in `./policy` and `./history`, a line
//! `Wellbeing: {...}` at the head of the start window, a `wellbeing` mark that
//! closed it). The curator keeps the window of every model and knows no
//! domain (R-58-9 no. 3); the rule now lives in a cell of `assistant` that
//! hears the person's facts where they already cross that hive -- the
//! section `memory` leaves each curator as `sidecar` (assistant edges 18, 21
//! and 44 before this strand) -- keeps the day's newest open concern in its
//! own store, and hands it to the curators as a note of scope `once` with a
//! `receipt` (strand note, GH #1477). The first receipt closes it; the cell
//! withdraws the other copies.
//!
//! Two halves, both on the manifests as they ship:
//! - the hive `assistant` in one process: its shipped edges evaluated by the
//!   colony's own CEL, the new cell's `script_inline` under python3 -- COLD on
//!   every message, so whatever it keeps between two messages lives in its
//!   own store -- and that store an in-memory SQLite with the store cell's
//!   shipped schema, answered by the store's own dispatcher. What the hive
//!   sends into a child composite is recorded in `into`, what leaves its rim
//!   in `out`; a message to any other cell fails the test (the cell reads no
//!   foreign database and no ledger of a curator -- it hears messages);
//! - the curator of a composite (`support/curator_hive.rs`), reached through
//!   that composite's shipped edges in both directions.
//!
//! The names this file fixes (OR-d58-2.followup.1 to .3 in the strand's
//! report) are the contract Bauer 3 builds against.
#![allow(dead_code)]

use crate::curator_hive::*;
use meclaw_colony::cel_eval::{
    CompiledCondition, CompiledModifier, apply_modifier, evaluate_condition, parse_condition,
    parse_modifier,
};
use meclaw_colony::config::ModifierSpec;
use meclaw_core::Headers;
use meclaw_core::serde_json::{self as sj, Map, Value, json};
use std::collections::VecDeque;

/// The companion domain of core.
pub const ASSISTANT: &str = "templates/assistant";

/// The new cell and its own store, children of `assistant`
/// (OR-d58-2.followup.1: neutral names, the store beside the cell like
/// `lead`/`state` of the lead hive).
pub const CELL: &str = "followup";
pub const CELL_AT: &str = "./followup";
pub const STORE: &str = "followup_store";
pub const STORE_AT: &str = "./followup_store";

/// The children of `assistant` with a curator: every one of them gets the
/// note, and the cell hears the receipt of every one of them.
pub const COMPOSITES: [&str; 3] = ["./talky", "./talky-chat", "./cogny"];

/// The knobs of the cell (OR-d58-2.followup.2): on/off (shipped "0"), the
/// least confidence of a fact, the text form of the note, the instance's
/// time zone and the cell's test clock.
pub const KNOB_ON: &str = "enabled";
pub const KNOB_MIN: &str = "min_confidence";
pub const KNOB_TEXT: &str = "note_text";
pub const KNOB_ZONE: &str = "timezone";
pub const KNOB_CLOCK: &str = "now_utc";

/// The class the memory-hive's sidecar schema offers on a fact
/// (`templates/memory-hive/schemas`, GH #1456): the assistant's business,
/// never the curator's.
pub const CLASS: &str = "wellbeing";

/// The test day of the pins and an instant in it.
pub const DAY: &str = "2026-11-08";
pub const NOON: &str = "2026-11-08T11:20:00Z";
pub const NEXT_DAY_NOON: &str = "2026-11-09T11:20:00Z";

/// The knobs of the note strand on the curator (GH #1477, `u-note.md`
/// point 8): `once` and `receipt` only with "1".
pub const NOTE_KNOB: &str = "note_once";
pub const NOTE_KNOB_CELLS: [&str; 3] = ["intake", "policy", "history"];

/// The head keys of a receipt (OR-d58-2.note.1).
pub const RECEIPT_KEYS: [&str; 7] = [
    "note_source",
    "note_id",
    "receipt",
    "session_id",
    "turn_id",
    "target",
    "at",
];

struct Edge {
    from: String,
    to: String,
    condition: Option<CompiledCondition>,
    modifier: Option<CompiledModifier>,
}

fn compile(e: &Value) -> Edge {
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
        from: e["from"].as_str().unwrap_or("").to_string(),
        to: e["to"].as_str().unwrap_or("").to_string(),
        condition,
        modifier,
    }
}

/// The shipped edges of a hive config, raw.
pub fn edges_of(rel: &str) -> Vec<Value> {
    read_json(&repo(rel))["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("{rel}: params.graph.edges"))
}

fn compiled(rel: &str) -> Vec<Edge> {
    edges_of(rel).iter().map(compile).collect()
}

/// Every edge of `edges` from `from` that takes `msg`, with its target and
/// the message as the edge's modifier leaves it.
fn step(edges: &[Edge], from: &str, msg: &Msg) -> Vec<(String, Msg)> {
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

/// The config of a child of `assistant` (`followup`, `followup_store`).
pub fn child_config(name: &str) -> Value {
    let p = repo(&format!("{ASSISTANT}/{name}/config.json"));
    assert!(
        p.is_file(),
        "{ASSISTANT}/{name}: the cell is not built (GH #1479, the strand followup)"
    );
    read_json(&p)
}

/// The template a child composite of `assistant` refers to, without its
/// version (`talky@6.19.0` -> `templates/talky`): the tree's current one.
pub fn composite_template(child: &str) -> String {
    let name = child.trim_start_matches("./");
    let marker = read_json(&repo(&format!("{ASSISTANT}/{name}/config.json")));
    let t = marker["cell"]["template"]
        .as_str()
        .unwrap_or_else(|| panic!("{ASSISTANT}/{name}: a ref"));
    format!("templates/{}", t.split('@').next().unwrap_or(t))
}

/// The hive `assistant` with its new cell in one process.
pub struct Assistant {
    edges: Vec<Edge>,
    cell: Value,
    pub db: rusqlite::Connection,
    /// Every store operation the cell sent, in order.
    pub ops: Vec<Value>,
    /// What the hive sent into a child composite: `(child, message)`.
    pub into: Vec<(String, Msg)>,
    /// What left the hive at its rim.
    pub out: Vec<Msg>,
    pub stderr: Vec<String>,
    /// Emissions the colony's `validate_emits` would dead-letter.
    pub emit_violations: Vec<String>,
}

impl Assistant {
    /// As shipped, `over` set on the cell's params (every key must exist).
    pub fn with(over: &[(&str, Value)]) -> Self {
        let mut cell = child_config(CELL);
        assert_eq!(cell["cell"]["type"], json!("code"), "{CELL} is a code cell");
        {
            let params = cell["params"]
                .as_object_mut()
                .unwrap_or_else(|| panic!("{CELL}: params"));
            for (k, v) in over {
                assert!(params.contains_key(*k), "no such param: {CELL}.{k}");
                params.insert((*k).to_string(), v.clone());
            }
        }
        let store = child_config(STORE);
        assert_eq!(store["cell"]["type"], json!("store"), "{STORE} is a store");
        let p = meclaw_cells::store::StoreParams::parse(&store["params"])
            .expect("the params of the cell's store parse");
        let db = rusqlite::Connection::open_in_memory().expect("sqlite");
        meclaw_cells::store::ddl::apply_declared_schema_ddl(&db, &p).expect("ddl");
        meclaw_cells::store::ddl::apply_index_ddl(&db, &p.indexes).expect("index ddl");
        meclaw_cells::store::seed::load_seed_if_present(
            &db,
            &repo(&format!("{ASSISTANT}/{STORE}")),
            &p.schema,
        )
        .expect("seed");
        Self {
            edges: compiled(&format!("{ASSISTANT}/config.json")),
            cell,
            db,
            ops: Vec::new(),
            into: Vec::new(),
            out: Vec::new(),
            stderr: Vec::new(),
            emit_violations: Vec::new(),
        }
    }

    /// The cell switched on, its clock at `now`.
    pub fn on_at(now: &str) -> Self {
        Self::with(&[(KNOB_ON, json!("1")), (KNOB_CLOCK, json!(now))])
    }

    /// A message leaving the child composite `child` at its rim, run to rest
    /// inside `assistant`.
    pub fn leave(&mut self, child: &str, msg: Msg) {
        let mut queue: VecDeque<(String, Msg)> = VecDeque::from([(child.to_string(), msg)]);
        let mut steps = 0;
        while let Some((from, msg)) = queue.pop_front() {
            steps += 1;
            assert!(steps < 400, "the hive does not come to rest");
            for (to, m) in step(&self.edges, &from, &msg) {
                match to.as_str() {
                    "." => self.out.push(m),
                    STORE_AT => {
                        for c in m.messages().iter().filter(|c| c["type"] == "tool_call") {
                            self.ops.push(
                                sj::from_str(c["text"].as_str().unwrap_or(""))
                                    .unwrap_or(Value::Null),
                            );
                        }
                        let answer = self.store(&m);
                        queue.push_back((STORE_AT.to_string(), answer));
                    }
                    CELL_AT => {
                        for o in self.run_cell(&m) {
                            queue.push_back((CELL_AT.to_string(), o));
                        }
                    }
                    c if COMPOSITES.contains(&c) || c == "./tools" => {
                        self.into.push((c.to_string(), m))
                    }
                    other => panic!("assistant routes to {other}, a child it does not have"),
                }
            }
        }
    }

    /// The notes the hive sent into `child`, every note of every message.
    pub fn notes_into(&self, child: &str) -> Vec<Value> {
        self.into
            .iter()
            .filter(|(c, m)| c == child && m.route() == "in_note")
            .flat_map(|(_, m)| {
                m.body
                    .get("notes")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
            })
            .collect()
    }

    /// The messages the hive sent into `child` on `in_note`.
    pub fn note_messages_into(&self, child: &str) -> Vec<Msg> {
        self.into
            .iter()
            .filter(|(c, m)| c == child && m.route() == "in_note")
            .map(|(_, m)| m.clone())
            .collect()
    }

    /// Forget what was sent so far.
    pub fn clear(&mut self) {
        self.into.clear();
        self.out.clear();
    }

    fn run_cell(&mut self, msg: &Msg) -> Vec<Msg> {
        let mut params = obj(self.cell["params"].clone());
        params.remove("script_inline");
        let doc = json!({
            "envelope": {"header": {"context": msg.context, "hop": msg.hop},
                         "target": format!("/x/assistant/{CELL}"),
                         "reply_to": msg.reply_to},
            "body": msg.body,
            "params": params,
        })
        .to_string();
        let script = self.cell["params"]["script_inline"]
            .as_str()
            .expect("script_inline")
            .to_string();
        let out = run_python(&script, &doc);
        let err = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(out.status.success(), "{CELL} exited non-zero: {err}");
        if !err.trim().is_empty() {
            self.stderr.push(err);
        }
        let emitted: Value = sj::from_slice(&out.stdout).unwrap_or_else(|e| {
            panic!(
                "{CELL}: output is not JSON ({e}): {}",
                String::from_utf8_lossy(&out.stdout)
            )
        });
        assert!(
            emitted.as_array().is_none_or(|a| a.len() <= 1)
                || self.cell["contract"]["multi_send_capable"] == json!(true),
            "{CELL} writes a JSON array without contract.multi_send_capable"
        );
        let list = match emitted {
            Value::Array(a) => a,
            other => vec![other],
        };
        let emits = &self.cell["contract"]["emits"];
        if !emits.is_null() {
            let block: meclaw_core::EmitsBlock = sj::from_value(emits.clone())
                .unwrap_or_else(|e| panic!("{CELL}: contract.emits does not parse: {e}"));
            let compiled = meclaw_core::CompiledEmits::compile(&block)
                .unwrap_or_else(|e| panic!("{CELL}: contract.emits does not compile: {e}"));
            for m in &list {
                if let Err(e) = meclaw_core::validate_emits(m, &compiled) {
                    self.emit_violations.push(format!("{CELL}: {e}"));
                }
            }
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

    /// One store message answered the way the store cell answers it; a
    /// refused operation fails the test.
    fn store(&self, msg: &Msg) -> Msg {
        use meclaw_cells::store::ops::dispatch;
        use meclaw_cells::store::output::{BundleLeg, build_bundle_result, build_tool_result};
        let calls: Vec<Value> = msg
            .messages()
            .into_iter()
            .filter(|m| m["type"] == "tool_call")
            .collect();
        let run = |c: &Value| {
            let args: Value = sj::from_str(c["text"].as_str().unwrap_or("")).expect("op json");
            dispatch(&self.db, &args)
                .unwrap_or_else(|e| panic!("an op of {STORE} failed: {e} in {args}"))
        };
        let (body, hop) = if calls.len() == 1 {
            build_tool_result(
                &run(&calls[0]),
                calls[0]["id"].as_str().unwrap_or("").to_string(),
                0,
            )
        } else {
            let legs: Vec<BundleLeg> = calls
                .iter()
                .map(|c| {
                    BundleLeg::from_outcome(&run(c), c["id"].as_str().unwrap_or("").to_string(), 0)
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
}

/// One fact item as the memory-hive's sidecar schema has the model write it.
pub fn fact(subject: &str, predicate: &str, claim: &str, conf: Option<f64>, class: bool) -> Value {
    let mut v = json!({"subject": subject, "predicate": predicate, "claim": claim,
                       "quote": claim, "source": "person", "stance": "asserted",
                       "fact_kind": "experience", "valid_from": DAY});
    if let Some(c) = conf {
        v["confidence"] = json!(c);
    }
    if class {
        v[CLASS] = json!(true);
    }
    v
}

/// The section `memory` as a composite hands it out at its rim (`sidecar`,
/// `hop.section` "memory", the splitter's body), in the standard round.
pub fn memory_out(session: &str, turn: &str, facts: Value) -> Msg {
    Msg {
        context: obj(json!({"session_id": session, "turn_id": turn,
                            "audience_set": ROUND_E, "channel": "test"})),
        hop: obj(json!({"route": "sidecar", "section": "memory",
                        "session_id": session, "turn_id": turn})),
        body: obj(json!({"messages": [], "section": "memory",
                         "payload": {"nothing_new": false, "facts": facts,
                                     "topic": {"movement": "continue"}}})),
        reply_to: String::new(),
    }
}

/// The end of the session `session` as the curator of a composite reports it
/// (`./writer`'s close pass: one `write` per close, hop `turn_id`
/// `close-<session>` and `session_id`, the generation's round and channel in
/// the context), carried through the composite's shipped edge to its rim:
/// what `assistant` hears from `child` when one of its sessions ends. The
/// batch is empty here -- the cell reads the head, never the words.
pub fn session_end(child: &str, session: &str) -> Msg {
    let m = Msg {
        context: obj(json!({"session_id": session, "audience_set": ROUND_E,
                            "channel": "test"})),
        hop: obj(
            json!({"route": "write", "phase": "", "turn_id": format!("close-{session}"),
                        "session_id": session, "iter": "0", "turn_count": "0",
                        "round_count": "0"}),
        ),
        body: obj(json!({"messages": [], "rounds": []})),
        reply_to: String::new(),
    };
    let at_rim = out_of(child, &m);
    assert_eq!(at_rim.len(), 1, "{child} hands `write` out once");
    at_rim[0].clone()
}

/// `session` of `child` ends, and `assistant` hears it.
pub fn session_ends(a: &mut Assistant, child: &str, session: &str) {
    let m = session_end(child, session);
    a.leave(child, m);
}

/// A curator as a composite with notes wires it: the note strand's knob on
/// its three cells, a share of the window for notes, the section `memory`
/// registered on `sidecar` the way talky's curator marker registers it, and
/// `extra` over it. Without the note strand in the tree the knob is missing
/// and this fails.
pub fn curator(extra: &[(&str, &str, Value)]) -> Hive {
    let mut over: Vec<(&str, &str, Value)> = vec![
        // OR-d58-2.followup.4: a `once` note ends with the person's day
        // (`until`), and the seed holds it against `history.now_utc` -- the
        // lab curator lives on the lab day of the cell (`NOON`), never on
        // the machine's (`extra` may move it, as the day-boundary pin does).
        ("history", "now_utc", json!(NOON)),
        ("policy", "notes_share", json!(0.02)),
        (
            "intake",
            "sections",
            json!({"memory": {"lane": "sidecar", "mark": "topic"}}),
        ),
    ];
    for c in NOTE_KNOB_CELLS {
        over.push((c, NOTE_KNOB, json!("1")));
    }
    over.extend(extra.iter().cloned());
    Hive::with(&over)
}

/// What `assistant` sent into `child` on `in_note`, carried through that
/// composite's shipped edges to its curator and delivered there.
pub fn deliver_notes(a: &Assistant, child: &str, h: &mut Hive) -> usize {
    let template = composite_template(child);
    let edges = compiled(&format!("{template}/config.json"));
    let mut n = 0;
    for m in a.note_messages_into(child) {
        for (to, inner) in step(&edges, ".", &m) {
            if to != "./curator" {
                continue;
            }
            n += 1;
            h.lane(
                "in_note",
                Value::Object(inner.context.clone()),
                Value::Object(inner.hop.clone()),
                Value::Object(inner.body.clone()),
            );
        }
    }
    n
}

/// A message leaving the curator of `child` (`./curator -> .`), carried
/// through the composite's shipped edges to its rim: what reaches
/// `assistant` from that child.
pub fn out_of(child: &str, m: &Msg) -> Vec<Msg> {
    let template = composite_template(child);
    let edges = compiled(&format!("{template}/config.json"));
    step(&edges, "./curator", m)
        .into_iter()
        .filter(|(to, _)| to == ".")
        .map(|(_, m)| m)
        .collect()
}

/// The receipts a curator sent since `out` was last cleared, home to
/// `assistant` through the composite `child`. Returns how many arrived.
pub fn receipts_home(a: &mut Assistant, child: &str, h: &Hive) -> usize {
    let mut n = 0;
    for r in h.routed("note_receipt") {
        for m in out_of(child, &r) {
            n += 1;
            a.leave(child, m);
        }
    }
    n
}

/// The start window of `call` (`reason` start or renew) asked of the
/// curator's `./history`, as the voice asks it under the handle `live`;
/// returns the seed. The receipts it sent stay in `h.out`.
pub fn seed(h: &mut Hive, call: &str, reason: &str, renewal_n: u64) -> Msg {
    h.out.clear();
    h.lane(
        "in_seed_ask",
        json!({"live": call}),
        json!({"live": call, "session_id": call, "reason": reason, "renewal_n": renewal_n}),
        json!({"messages": []}),
    );
    let seeds = h.routed("seed");
    assert_eq!(seeds.len(), 1, "one seed: {:?} {:?}", h.out, h.stderr);
    seeds[0].clone()
}

/// How many messages of `m` carry `text`.
pub fn count_in(m: &Msg, text: &str) -> usize {
    texts(m).iter().filter(|t| t.contains(text)).count()
}

/// A head field as text, "" when absent.
pub fn field(m: &Msg, k: &str) -> String {
    m.hop
        .get(k)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// The notes with text (not a withdrawal) among `notes`.
pub fn live_notes(notes: &[Value]) -> Vec<Value> {
    notes
        .iter()
        .filter(|n| n["text"].as_str().is_some_and(|t| !t.trim().is_empty()))
        .cloned()
        .collect()
}

/// The withdrawals (text empty) among `notes`.
pub fn withdrawals(notes: &[Value]) -> Vec<Value> {
    notes
        .iter()
        .filter(|n| n["text"].as_str().is_some_and(|t| t.trim().is_empty()))
        .cloned()
        .collect()
}

/// One turn of `session` in the curator, answered: the call it sent.
pub fn turn_in(h: &mut Hive, session: &str, turn: &str, words: &str) -> Msg {
    turn_in_on(h, None, session, turn, words)
}

/// The same, the turn come in by the channel node `node`
/// (`context.channel_node`, the key `assistant` splits its surfaces by:
/// `chat` reaches `./talky-chat`, anything else `./talky`); `None` = a turn
/// without one, as `turn_in` sends it.
pub fn turn_in_on(h: &mut Hive, node: Option<&str>, session: &str, turn: &str, words: &str) -> Msg {
    h.out.clear();
    let hop = json!({"session_id": session, "turn_id": turn, "iter": "0", "phase": "",
                     "path": "talk", "needs": "", "families": ""});
    let mut ctx = json!({"session_id": session, "turn_id": turn, "iter": "0",
                         "channel": "test", "audience_set": ROUND_E, "path": "talk"});
    if let Some(n) = node {
        ctx["channel_node"] = json!(n);
    }
    h.lane("in_curate", ctx, hop, json!({"messages": [user(words)]}));
    let calls = h.routed("brain");
    assert_eq!(calls.len(), 1, "one call: {:?} {:?}", h.out, h.stderr);
    calls[0].clone()
}

/// A closed turn of the running session `live` (its handle in hop and
/// context): the call, answered; `h.out` holds what the answer sent.
pub fn closed_turn_of(h: &mut Hive, live: &str, turn: &str) {
    h.out.clear();
    let hop = json!({"session_id": live, "turn_id": turn, "iter": "0", "phase": "",
                     "path": "talk", "needs": "", "families": "", "live": live});
    let ctx = json!({"session_id": live, "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": "talk",
                     "live": live});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("and then?")]}),
    );
    let calls = h.routed("brain");
    assert_eq!(calls.len(), 1, "{:?} {:?}", h.out, h.stderr);
    h.out.clear();
    h.tap(&calls[0], "stop", json!({}), json!([said("fine")]));
}

/// The section `memory` the curator of a composite hands out after one
/// answered turn whose block carried `facts`: the curator's own forward
/// (`./intake`, registered section), through the composite's edge to its
/// rim -- the message `assistant` hears from that child.
pub fn memory_from_curator(h: &mut Hive, child: &str, turn: &str, facts: Value) -> Msg {
    memory_from_curator_on(h, child, None, "s1", turn, facts)
}

/// The same for a turn of `session` come in by the channel node `node`
/// (see `turn_in_on`): the section leaves with that turn's context.
pub fn memory_from_curator_on(
    h: &mut Hive,
    child: &str,
    node: Option<&str>,
    session: &str,
    turn: &str,
    facts: Value,
) -> Msg {
    let call = turn_in_on(h, node, session, turn, "how are you?");
    h.out.clear();
    h.tap(&call, "stop", json!({}), json!([said("I am listening.")]));
    h.out.clear();
    h.section(
        &call,
        "memory",
        json!({"nothing_new": false, "facts": facts, "topic": {"movement": "continue"}}),
    );
    let fwd = h.routed("sidecar");
    assert_eq!(
        fwd.len(),
        1,
        "the curator forwards the section once: {:?} {:?}",
        h.out,
        h.stderr
    );
    let at_rim = out_of(child, &fwd[0]);
    assert_eq!(at_rim.len(), 1, "{child} hands the section out once");
    at_rim[0].clone()
}

/// The `in_note` messages' notes as `(id, text)`.
pub fn ids_and_texts(notes: &[Value]) -> Vec<(String, String)> {
    notes
        .iter()
        .map(|n| {
            (
                n["id"].as_str().unwrap_or("").to_string(),
                n["text"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

/// Params as a map, for `Assistant::with` callers that build them.
pub fn knobs(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect()
}

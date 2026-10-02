//! GH #893 -- the model reads its own wall: `history_search`, `history_read`
//! and `history_outline`, served by `curator/history` out of the hive's ledger.
//!
//! Same harness as `curator_cells.rs`: the shipped `script_inline` programs run
//! under python3 against real stdin documents, the edges are the shipped
//! `params.graph` of the hive evaluated by the colony's own CEL, and the ledger
//! is an in-memory SQLite with the shipped schema whose every operation runs
//! through the store cell's own dispatcher. The wall is SOWN by hand -- blocks
//! and rows exactly as `./intake` writes them (canonical JSON, sha256, `seq`
//! in microseconds, `at` in UTC) -- so a test can put a row at the time and in
//! the session it needs.
//!
//! The colony case (a booted talky and cogny, a stub brain calling the tool) is
//! `gh893_a_model_reads_its_own_wall.rs`.

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
    /// The rows every `select` of the ledger handed back, by table (`blocks`
    /// reads that carry `body` count apart as `blocks:body`): how much of the
    /// wall a call read, which is what a bounded read is measured by.
    reads: BTreeMap<String, usize>,
    /// GH #925: the audience every wall row and mark this test sows carries
    /// (`audience_set`, stored as given) -- `ROUND_E` unless the test sets
    /// another; `None` sows a row from before the rule.
    audience: Option<String>,
    /// GH #925: the round a `history_in` call is served in
    /// (`context.audience_set`, a TEXT as the colony carries it) -- `ROUND_E`
    /// unless the test sets another; `Value::Null` declares none.
    round: Value,
    /// GH #925 (OR-BD-74): the store messages the ledger answered -- one
    /// ledger round trip each, two routing decisions on the chain of a
    /// history call (GH #929 S3).
    trips: usize,
}

/// GH #925: the one round the harness gives what it sows and what it asks
/// unless a test names another, so every test written before the gate reads
/// the wall it sowed.
const ROUND_E: &str = r#"["member:e"]"#;

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
            "schemas",
            "history",
            // GH #896: `./policy` hands the first call of every new session
            // through `./handover`, so a hive without it stops there.
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
            reads: BTreeMap::new(),
            audience: Some(ROUND_E.to_string()),
            round: json!(ROUND_E),
            trips: 0,
        }
    }

    fn count_read(&mut self, args: &Value, outcome: &meclaw_cells::store::ops::OpOutcome) {
        if args["operation"] != "select" {
            return;
        }
        let table = args["table"].as_str().unwrap_or("").to_string();
        let body = args["columns"]
            .as_array()
            .is_some_and(|c| c.iter().any(|x| x == "body"));
        let key = if body { format!("{table}:body") } else { table };
        let n = outcome.payload.as_array().map_or(0, Vec::len);
        *self.reads.entry(key).or_default() += n;
    }

    fn read_of(&self, key: &str) -> usize {
        self.reads.get(key).copied().unwrap_or(0)
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
        self.trips += 1;
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
                    self.count_read(&args, &outcome);
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
            let mut legs: Vec<BundleLeg> = Vec::new();
            for c in &calls {
                let id = c["id"].as_str().unwrap_or("").to_string();
                let args: Value = sj::from_str(c["text"].as_str().unwrap_or("")).expect("op json");
                legs.push(match dispatch(&self.db, &args) {
                    Ok(outcome) => {
                        self.count_read(&args, &outcome);
                        BundleLeg::from_outcome(&outcome, id, 0)
                    }
                    Err(e) => BundleLeg::refusal(
                        args["operation"].as_str().unwrap_or("error"),
                        id,
                        0,
                        "invalid_input",
                        e,
                    ),
                });
            }
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
        // GH #925: a message the colony hands in carries its round; a test
        // that names none gets the harness's, one that names `null` keeps it.
        let mut context = obj(context);
        if !context.contains_key("audience_set") {
            context.insert("audience_set".into(), json!(ROUND_E));
        }
        let msg = Msg {
            context,
            hop,
            body: obj(body),
            reply_to: reply_to.to_string(),
        };
        self.pump(".", msg);
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

    fn routed(&self, route: &str) -> Vec<Msg> {
        self.out
            .iter()
            .filter(|m| m.route() == route)
            .cloned()
            .collect()
    }
}

fn user(text: &str) -> Value {
    json!({"origin": "user", "type": "text", "text": text})
}

fn said(text: &str) -> Value {
    json!({"origin": "assistant", "type": "text", "text": text})
}

fn sha256_hex(s: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The canonical JSON `./intake` hashes and stores: keys sorted (this crate's
/// `serde_json` keeps a sorted map), UTF-8, no whitespace.
fn canonical(el: &Value) -> String {
    sj::to_string(el).expect("serialise")
}

/// `2026-09-<day>T<hour>:00:00.000000Z`, the wall's own time format.
fn at(day: u32, hour: u32) -> String {
    format!("2026-09-{day:02}T{hour:02}:00:00.000000Z")
}

const CALL: &str = "call-893";

impl Hive {
    /// One element sown onto the wall the way `./intake` writes it: its block
    /// once under the sha256 of its canonical JSON, its row with the given
    /// `seq`, session, turn, kind and time. Returns the hash.
    fn sow(
        &mut self,
        seq: i64,
        session: &str,
        turn: &str,
        kind: &str,
        el: &Value,
        when: &str,
    ) -> String {
        let hash = sha256_hex(&canonical(el));
        self.sow_as(&hash, seq, session, turn, kind, el, when);
        hash
    }

    /// The same under a hash the test chooses (two blocks sharing a prefix
    /// cannot be found by hashing, but the ledger stores what it is given).
    #[allow(clippy::too_many_arguments)]
    fn sow_as(
        &mut self,
        hash: &str,
        seq: i64,
        session: &str,
        turn: &str,
        kind: &str,
        el: &Value,
        when: &str,
    ) {
        let body = canonical(el);
        let chars = el["text"].as_str().map_or(0, |t| t.chars().count()) as i64;
        self.db
            .execute(
                "INSERT INTO blocks (hash, kind, chars, body, first_seen) \
                 SELECT ?1, ?2, ?3, ?4, ?5 WHERE NOT EXISTS (SELECT 1 FROM blocks WHERE hash = ?1)",
                rusqlite::params![hash, kind, chars, body, when],
            )
            .expect("a block");
        // GH #925: the row carries the audience of the round that sowed it
        // (`self.audience`; NULL for a row from before the rule).
        self.db
            .execute(
                "INSERT INTO wall (seq, session_id, turn_id, iter, kind, hash, nth, final, \
                 episode_idx, at, audience_set) \
                 VALUES (?1, ?2, ?3, 0, ?4, ?5, 0, 1, NULL, ?6, ?7)",
                rusqlite::params![seq, session, turn, kind, hash, when, self.audience],
            )
            .expect("a wall row");
    }

    /// One `marks` row (README § 2, written by T's TRIM and topic ingress),
    /// with the audience of the round that wrote it (GH #925, `self.audience`).
    fn mark(&mut self, seq: i64, session: &str, turn: &str, kind: &str, value: &str) {
        self.db
            .execute(
                "INSERT INTO marks (seq, session_id, turn_id, kind, value, at, audience_set) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![seq, session, turn, kind, value, at(28, 12), self.audience],
            )
            .expect("a mark (the `marks` table is the ledger's since GH #892)");
    }
}

impl Hive {
    /// One call on the hive's `in_history_call` lane, in the shape the parent's
    /// dispatcher edge hands it in, during turn `t-now` of session `s-now`.
    /// Returns the one answer that left the hive and its text, parsed.
    fn history(&mut self, name: &str, args: Value) -> (Msg, Value) {
        self.history_in("s-now", "t-now", name, args)
    }

    fn history_in(&mut self, session: &str, turn: &str, name: &str, args: Value) -> (Msg, Value) {
        self.out.clear();
        // GH #925: the round the call is served in (`self.round`, null = none).
        let ctx = json!({"session_id": session, "turn_id": turn, "iter": "1",
                         "curator_call": "cc-893", "audience_set": self.round.clone()});
        let hop = json!({"tool_name": name, "tool_call_id": CALL});
        let body = json!({"messages": [{"origin": "assistant", "type": "tool_call", "id": CALL,
                                        "text": args.to_string()}]});
        self.lane("in_history_call", ctx, hop, body);
        let answers = self.routed("tool_result");
        assert_eq!(answers.len(), 1, "one call, one answer: {:?}", self.out);
        assert_eq!(
            self.out.len(),
            1,
            "nothing else leaves the hive: {:?}",
            self.out
        );
        let m = answers[0].clone();
        let turns = m.messages();
        assert_eq!(turns.len(), 1, "one tool_result turn: {turns:?}");
        assert_eq!(turns[0]["type"], "tool_result");
        assert_eq!(turns[0]["id"], CALL, "filed under the call id it answers");
        assert_eq!(m.hop["tool_call_id"], CALL);
        let payload: Value = sj::from_str(turns[0]["text"].as_str().expect("a text"))
            .expect("the answer is one JSON object");
        assert_eq!(payload["tool"], name);
        (m, payload)
    }
}

fn error_of(m: &Msg) -> String {
    m.hop["error_code"].as_str().unwrap_or("").to_string()
}

fn ids(payload: &Value, key: &str) -> Vec<String> {
    payload[key]
        .as_array()
        .unwrap_or_else(|| panic!("`{key}` is a list: {payload}"))
        .iter()
        .map(|h| h["id"].as_str().unwrap_or("").to_string())
        .collect()
}

fn short(hash: &str) -> String {
    format!("#{}", &hash[..12])
}

/// The wall most tests read: two sessions about a lighthouse keeper, one of
/// them with a tool round.
struct Sown {
    ada: String,
    noted: String,
    again: String,
    call: String,
    result: String,
    answer: String,
}

fn sown(h: &mut Hive) -> Sown {
    let ada = h.sow(
        1_000,
        "s1",
        "t1",
        "user",
        &user("The lighthouse keeper is called Ada Quill."),
        &at(20, 10),
    );
    let noted = h.sow(
        1_001,
        "s1",
        "t1",
        "assistant",
        &said("Noted: Ada Quill keeps the lighthouse."),
        &at(20, 10),
    );
    let again = h.sow(
        2_000,
        "s2",
        "t2",
        "user",
        &user("Tell me about the LIGHTHOUSE   keeper again."),
        &at(25, 9),
    );
    let args = json!({"q": "lighthouse keeper Ada"}).to_string();
    let call = h.sow(
        2_001,
        "s2",
        "t2",
        "tool_call",
        &json!({"origin": "assistant", "type": "tool_call", "id": "c-7",
                             "text": json!({"name": "web_search", "arguments": args}).to_string()}),
        &at(25, 9),
    );
    let result = h.sow(
        2_002,
        "s2",
        "t2",
        "tool_result",
        &json!({"origin": "tool", "type": "tool_result", "id": "c-7",
                               "text": "no public record of a keeper named Ada"}),
        &at(25, 9),
    );
    let answer = h.sow(
        2_003,
        "s2",
        "t3",
        "assistant",
        &said("Ada Quill, as you told me on the twentieth."),
        &at(25, 10),
    );
    Sown {
        ada,
        noted,
        again,
        call,
        result,
        answer,
    }
}

// ============================================================ 1. the search

#[test]
fn phrase_finds_across_sessions_newest_first() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let w = sown(&mut h);
    let (m, p) = h.history("history_search", json!({"query": "lighthouse keeper"}));
    assert_eq!(error_of(&m), "", "{p}");
    // The default mode is `phrase`: the words in order, any case, any spacing
    // -- so the shouted question of session s2 is a hit, and so is the query
    // inside the tool call's arguments.
    assert_eq!(p["mode"], "phrase");
    assert_eq!(
        ids(&p, "hits"),
        vec![short(&w.call), short(&w.again), short(&w.ada)],
        "every match of both sessions, newest first: {p}"
    );
    let first = &p["hits"][0];
    assert_eq!(first["seq"], 2_001);
    assert_eq!(first["session_id"], "s2");
    assert_eq!(first["turn_id"], "t2");
    assert_eq!(first["kind"], "tool_call");
    assert_eq!(first["at"], at(25, 9));
    assert!(
        p["hits"][1]["excerpt"]
            .as_str()
            .unwrap()
            .contains("LIGHTHOUSE   keeper"),
        "{p}"
    );
    assert_eq!(p["total_hits"], 3);
    assert_eq!(p["truncated_scan"], false);
    // The answer goes back into the round it came from: the turn's context
    // rides on, the hive's own bookkeeping does not.
    assert_eq!(m.context["turn_id"], "t-now", "{:?}", m.context);
    assert_eq!(m.context["session_id"], "s-now");
    for k in ["cur_origin", "cur_phase", "cur_call", "cur_reason"] {
        assert!(
            !m.context.contains_key(k),
            "`{k}` left the hive: {:?}",
            m.context
        );
    }
    assert!(h.stderr.is_empty(), "{:?}", h.stderr);
}

#[test]
fn exact_is_case_sensitive() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let w = sown(&mut h);
    let (_, upper) = h.history(
        "history_search",
        json!({"query": "LIGHTHOUSE", "mode": "exact"}),
    );
    assert_eq!(ids(&upper, "hits"), vec![short(&w.again)], "{upper}");
    let (_, lower) = h.history(
        "history_search",
        json!({"query": "lighthouse", "mode": "exact"}),
    );
    assert_eq!(
        ids(&lower, "hits"),
        vec![short(&w.call), short(&w.noted), short(&w.ada)],
        "{lower}"
    );
}

#[test]
fn regex_with_a_bad_pattern_is_bad_pattern() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let w = sown(&mut h);
    let (m, p) = h.history(
        "history_search",
        json!({"query": "(unclosed", "mode": "regex"}),
    );
    assert_eq!(error_of(&m), "bad_pattern", "{p}");
    assert_eq!(
        p["error"], "bad_pattern",
        "the code stands in the text the model reads"
    );
    let long = "a".repeat(201);
    let (m, p) = h.history("history_search", json!({"query": long, "mode": "regex"}));
    assert_eq!(
        error_of(&m),
        "bad_pattern",
        "a pattern over 200 characters: {p}"
    );
    // A pattern that compiles is searched with it.
    let (m, p) = h.history(
        "history_search",
        json!({"query": r"Ada\s+Q\w+", "mode": "regex"}),
    );
    assert_eq!(error_of(&m), "", "{p}");
    assert_eq!(
        ids(&p, "hits"),
        vec![short(&w.answer), short(&w.noted), short(&w.ada)],
        "{p}"
    );
    let (m, _) = h.history("history_search", json!({"query": "x", "mode": "fuzzy"}));
    assert_eq!(error_of(&m), "bad_arguments", "a mode that does not exist");
}

#[test]
fn since_until_and_kinds_filter() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let w = sown(&mut h);
    let (_, p) = h.history(
        "history_search",
        json!({"query": "Ada", "since": "2026-09-21"}),
    );
    assert_eq!(
        ids(&p, "hits"),
        vec![short(&w.answer), short(&w.result), short(&w.call)],
        "only session s2 lies after the 21st: {p}"
    );
    // A bare date as the upper bound is the whole day.
    let (_, p) = h.history(
        "history_search",
        json!({"query": "Ada", "until": "2026-09-20"}),
    );
    assert_eq!(ids(&p, "hits"), vec![short(&w.noted), short(&w.ada)], "{p}");
    let (_, p) = h.history(
        "history_search",
        json!({"query": "Ada", "since": "2026-09-25T09:30:00+00:00",
               "until": "2026-09-25T12:00:00Z"}),
    );
    assert_eq!(
        ids(&p, "hits"),
        vec![short(&w.answer)],
        "both bounds, with offsets: {p}"
    );
    let (_, p) = h.history(
        "history_search",
        json!({"query": "Ada", "kinds": ["assistant"]}),
    );
    assert_eq!(
        ids(&p, "hits"),
        vec![short(&w.answer), short(&w.noted)],
        "{p}"
    );
    let (_, p) = h.history(
        "history_search",
        json!({"query": "Ada", "kinds": ["tool_call", "tool_result"]}),
    );
    assert_eq!(
        ids(&p, "hits"),
        vec![short(&w.result), short(&w.call)],
        "{p}"
    );
    // Review focus (3): `since` after `until` is refused, not answered empty.
    let (m, _) = h.history(
        "history_search",
        json!({"query": "Ada", "since": "2026-09-26", "until": "2026-09-21"}),
    );
    assert_eq!(error_of(&m), "bad_range");
    let (m, _) = h.history(
        "history_search",
        json!({"query": "Ada", "since": "last tuesday"}),
    );
    assert_eq!(error_of(&m), "bad_arguments");
    let (m, _) = h.history(
        "history_search",
        json!({"query": "Ada", "kinds": ["system"]}),
    );
    assert_eq!(
        error_of(&m),
        "bad_arguments",
        "system blocks are never on the wall"
    );
}

#[test]
fn limit_and_context_bound_the_answer() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let w = sown(&mut h);
    let (_, p) = h.history("history_search", json!({"query": "Ada", "limit": 2}));
    assert_eq!(
        ids(&p, "hits"),
        vec![short(&w.answer), short(&w.result)],
        "{p}"
    );
    assert_eq!(
        p["total_hits"], 5,
        "the hits past the limit are counted, not shown"
    );
    let (_, p) = h.history("history_search", json!({"query": "Ada", "limit": 99}));
    assert_eq!(p["limit"], 20, "the limit is at most 20");
    let (_, p) = h.history("history_search", json!({"query": "Ada"}));
    assert_eq!(p["limit"], 8, "and 8 when none is named");
    assert!(
        p["hits"][0].get("before").is_none(),
        "no neighbours by default: {p}"
    );

    // One neighbour on each side, in wall order, inside the hit's own session.
    let (_, p) = h.history(
        "history_search",
        json!({"query": "Ada", "limit": 2, "context": 1}),
    );
    let newest = &p["hits"][0];
    assert_eq!(ids(newest, "before"), vec![short(&w.result)], "{p}");
    assert_eq!(ids(newest, "after"), Vec::<String>::new(), "{p}");
    let second = &p["hits"][1];
    assert_eq!(ids(second, "before"), vec![short(&w.call)], "{p}");
    assert_eq!(ids(second, "after"), vec![short(&w.answer)], "{p}");
    assert_eq!(second["after"][0]["kind"], "assistant");
    assert_eq!(second["after"][0]["turn_id"], "t3");
    // At most two: a wider context is two.
    let (_, p) = h.history(
        "history_search",
        json!({"query": "Ada", "limit": 1, "context": 9}),
    );
    assert_eq!(
        ids(&p["hits"][0], "before"),
        vec![short(&w.call), short(&w.result)],
        "{p}"
    );
    // The first row of session s2 has no neighbour in s1 before it.
    let (_, p) = h.history(
        "history_search",
        json!({"query": "tell me about", "context": 2}),
    );
    assert_eq!(ids(&p, "hits"), vec![short(&w.again)], "{p}");
    assert_eq!(ids(&p["hits"][0], "before"), Vec::<String>::new(), "{p}");
    assert_eq!(
        ids(&p["hits"][0], "after"),
        vec![short(&w.call), short(&w.result)],
        "{p}"
    );
}

#[test]
fn a_scan_over_budget_says_truncated_scan() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("history", "scan_budget", json!(3))]);
    let mut needles = Vec::new();
    for i in 1..=5 {
        needles.push(h.sow(
            i,
            "s1",
            &format!("t{i}"),
            "user",
            &user(&format!("needle number {i}")),
            &at(20, i as u32),
        ));
    }
    let (m, p) = h.history("history_search", json!({"query": "needle"}));
    assert_eq!(
        error_of(&m),
        "",
        "a cut scan is an answer, not a refusal: {p}"
    );
    assert_eq!(p["truncated_scan"], true, "{p}");
    assert_eq!(p["cut_by"], "scan_budget", "{p}");
    assert_eq!(p["scanned"], 3);
    assert_eq!(
        ids(&p, "hits"),
        vec![short(&needles[4]), short(&needles[3]), short(&needles[2])],
        "the hits up to the cut, newest first: {p}"
    );
    let mut h = Hive::with(&[("history", "scan_budget", json!(5))]);
    for i in 1..=5 {
        h.sow(
            i,
            "s1",
            &format!("t{i}"),
            "user",
            &user(&format!("needle number {i}")),
            &at(20, i as u32),
        );
    }
    let (_, p) = h.history("history_search", json!({"query": "needle"}));
    assert_eq!(
        p["truncated_scan"], false,
        "exactly the budget is no cut: {p}"
    );
    assert_eq!(p["total_hits"], 5);
}

/// Bounded reads (GH #893): a pattern that backtracks without end is answered in
/// time, under its call id, with the hits up to the moment the time budget
/// ran out -- never a runner timeout (`script_timeout` carries no route, the
/// call would stay unanswered until the collector gives up on the round).
#[test]
fn a_catastrophic_pattern_is_answered_in_time() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("history", "time_budget_ms", json!(1000))]);
    // `(a+)+$` against forty `a` and a `b` does not finish in CPython.
    h.sow(
        1,
        "s1",
        "t1",
        "user",
        &user(&format!("{}b", "a".repeat(40))),
        &at(20, 1),
    );
    let quick = h.sow(2, "s1", "t2", "user", &user("aaaa"), &at(20, 2));
    let t = std::time::Instant::now();
    let (m, p) = h.history(
        "history_search",
        json!({"query": "(a+)+$", "mode": "regex"}),
    );
    let took = t.elapsed();
    assert_eq!(error_of(&m), "", "a cut search is an answer: {p}");
    assert_eq!(
        ids(&p, "hits"),
        vec![short(&quick)],
        "the hits before the cut: {p}"
    );
    assert_eq!(p["truncated_scan"], true, "{p}");
    assert_eq!(p["cut_by"], "time_budget", "{p}");
    let runner = cell_config("history")["params"]["external_timeout_ms"]
        .as_u64()
        .expect("the runner's timeout");
    assert!(
        took.as_millis() < u128::from(runner),
        "answered in {took:?}; the runner kills at {runner} ms"
    );
}

/// Bounded reads (GH #893): a search reads the wall page by page, newest first,
/// and stops at the page that fills its `limit` -- the bodies of a whole wall
/// are never fetched to show eight hits of its newest turns. The hits it
/// counts are those of the pages it read (`stopped_at_limit` says so), and a
/// hit's neighbours come from the wall, not from the page it stood on.
#[test]
fn a_search_reads_page_by_page_and_stops_at_its_limit() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("history", "page_rows", json!(2))]);
    let n: Vec<String> = (1..=7)
        .map(|i| {
            h.sow(
                i,
                "s1",
                &format!("t{i}"),
                "user",
                &user(&format!("needle number {i}")),
                &at(20, i as u32),
            )
        })
        .collect();
    let (m, p) = h.history("history_search", json!({"query": "needle", "limit": 2}));
    assert_eq!(error_of(&m), "", "{p}");
    assert_eq!(ids(&p, "hits"), vec![short(&n[6]), short(&n[5])], "{p}");
    assert_eq!(p["stopped_at_limit"], true, "{p}");
    assert_eq!(p["total_hits"], 2, "counted in the pages read: {p}");
    assert_eq!(p["scanned"], 2);
    assert_eq!(p["truncated_scan"], false);
    assert_eq!(
        h.read_of("blocks:body"),
        2,
        "the bodies of one page, not of the wall: {:?}",
        h.reads
    );
    h.reads.clear();
    let (_, p) = h.history("history_search", json!({"query": "needle", "limit": 3}));
    assert_eq!(
        ids(&p, "hits"),
        vec![short(&n[6]), short(&n[5]), short(&n[4])],
        "{p}"
    );
    assert_eq!(p["total_hits"], 4, "the second page is counted whole: {p}");
    assert_eq!(p["stopped_at_limit"], true);
    assert_eq!(h.read_of("blocks:body"), 4, "{:?}", h.reads);
    // No limit reached: every page, and nothing stopped early.
    let (_, p) = h.history("history_search", json!({"query": "needle"}));
    assert_eq!(p["total_hits"], 7, "{p}");
    assert_eq!(p["scanned"], 7);
    assert_eq!(p["stopped_at_limit"], false);
    assert_eq!(p["truncated_scan"], false);
    // A neighbour across the edge of the page the hit stood on.
    let (_, p) = h.history(
        "history_search",
        json!({"query": "needle", "limit": 2, "context": 1}),
    );
    assert_eq!(ids(&p["hits"][1], "before"), vec![short(&n[4])], "{p}");
    assert_eq!(ids(&p["hits"][1], "after"), vec![short(&n[6])], "{p}");
    assert!(h.stderr.is_empty(), "{:?}", h.stderr);
}

/// The block kind `view` (GH #892): a window form is a
/// block of the ledger but never a row of the wall, and the search reads the
/// wall first -- so a `view` block is no hit and no id `history_read` knows.
#[test]
fn a_view_block_is_never_a_hit() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let w = sown(&mut h);
    let view = json!({"origin": "user", "type": "text",
                      "text": "[#0123456789ab] The lighthouse keeper is called Ada Quill."});
    let view_hash = sha256_hex(&canonical(&view));
    h.db.execute(
        "INSERT INTO blocks (hash, kind, chars, body, first_seen) VALUES (?1, 'view', 60, ?2, ?3)",
        rusqlite::params![view_hash, canonical(&view), at(28, 9)],
    )
    .expect("a view block");
    let (_, p) = h.history("history_search", json!({"query": "keeper is called"}));
    assert_eq!(ids(&p, "hits"), vec![short(&w.ada)], "{p}");
    let (m, _) = h.history("history_read", json!({"id": short(&view_hash)}));
    assert_eq!(error_of(&m), "not_found");
}

// ============================================================== 2. the read

#[test]
fn read_by_short_id_is_byte_identical() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let w = sown(&mut h);
    let stored = h.rows(&format!("SELECT body FROM blocks WHERE hash = '{}'", w.ada));
    let stored = stored[0][0].as_str().expect("a body").to_string();
    // Every spelling of the id a model can copy: as the window shows it, with
    // the brackets, bare, and the full hash.
    for id in [
        short(&w.ada),
        format!("[{}]", short(&w.ada)),
        w.ada[..12].to_string(),
        w.ada.clone(),
    ] {
        let (m, p) = h.history("history_read", json!({"id": id}));
        assert_eq!(error_of(&m), "", "{id}: {p}");
        let blocks = p["blocks"].as_array().expect("blocks");
        assert_eq!(blocks.len(), 1, "{p}");
        let b = &blocks[0];
        assert_eq!(
            b["block"],
            user("The lighthouse keeper is called Ada Quill.")
        );
        assert_eq!(
            canonical(&b["block"]),
            stored,
            "byte for byte what the ledger holds"
        );
        assert_eq!(b["id"], short(&w.ada));
        assert_eq!(b["hash"], w.ada.as_str());
        assert_eq!(b["seq"], 1_000);
        assert_eq!(b["session_id"], "s1");
        assert_eq!(b["turn_id"], "t1");
        assert_eq!(b["kind"], "user");
        assert_eq!(b["released"], false);
        assert_eq!(p["size"], stored.len());
    }
    // Nothing was released, so nothing was marked.
    assert!(h.rows("SELECT * FROM marks").is_empty());
}

#[test]
fn read_a_released_block_whole_and_mark_reread() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let w = sown(&mut h);
    // TRIM released the answer of session s1 by the id the window showed --
    // `./intake` writes the 12 hex digits without `#` (OR-KY.T.4).
    h.mark(5, "s1", "t1", "release", &w.noted[..12]);
    // The question of s1 was released and pinned again later: the later word
    // wins (OR-KY.T.13), it is no released block.
    h.mark(6, "s1", "t1", "release", &w.ada[..12]);
    h.mark(7, "s1", "t1", "pin", &w.ada[..12]);
    let (m, p) = h.history_in(
        "s-now",
        "t-ada",
        "history_read",
        json!({"id": short(&w.ada)}),
    );
    assert_eq!(error_of(&m), "", "{p}");
    assert_eq!(
        p["blocks"][0]["released"], false,
        "pinned after its release: {p}"
    );
    assert!(
        h.rows("SELECT * FROM marks WHERE kind = 'reread'")
            .is_empty(),
        "reading a block that is not released is no re-read"
    );
    let (m, p) = h.history("history_read", json!({"id": short(&w.noted)}));
    assert_eq!(error_of(&m), "", "{p}");
    assert_eq!(
        p["blocks"][0]["block"],
        said("Noted: Ada Quill keeps the lighthouse."),
        "a released block is read whole -- that is the way back"
    );
    assert_eq!(p["blocks"][0]["released"], true);
    let reread = h.rows("SELECT session_id, turn_id, value FROM marks WHERE kind = 'reread'");
    assert_eq!(
        reread,
        vec![vec![json!("s-now"), json!("t-now"), json!(w.noted.clone())]],
        "one `reread` mark, in the turn that read it, naming the block"
    );
    // Read before written (OR-KY-61): the same turn reading it again adds none.
    h.history("history_read", json!({"id": w.noted.clone()}));
    assert_eq!(h.rows("SELECT * FROM marks WHERE kind = 'reread'").len(), 1);
    // A later turn that reads it again is a second re-read.
    h.history_in(
        "s-now",
        "t-next",
        "history_read",
        json!({"id": short(&w.noted)}),
    );
    assert_eq!(h.rows("SELECT * FROM marks WHERE kind = 'reread'").len(), 2);
    // A turn range that holds it counts as a re-read too; one that does not, not.
    h.history_in(
        "s-now",
        "t-range",
        "history_read",
        json!({"from_turn": "t1"}),
    );
    assert_eq!(h.rows("SELECT * FROM marks WHERE kind = 'reread'").len(), 3);
    h.history_in(
        "s-now",
        "t-other",
        "history_read",
        json!({"from_turn": "t2"}),
    );
    assert_eq!(h.rows("SELECT * FROM marks WHERE kind = 'reread'").len(), 3);
    assert!(h.stderr.is_empty(), "{:?}", h.stderr);
}

#[test]
fn read_a_turn_range_in_wall_order() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let w = sown(&mut h);
    let (m, p) = h.history("history_read", json!({"from_turn": "t2", "to_turn": "t2"}));
    assert_eq!(error_of(&m), "", "{p}");
    assert_eq!(
        ids(&p, "blocks"),
        vec![short(&w.again), short(&w.call), short(&w.result)],
        "{p}"
    );
    let kinds: Vec<&str> = p["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["user", "tool_call", "tool_result"]);
    assert_eq!(
        p["blocks"][2]["block"]["text"], "no public record of a keeper named Ada",
        "whole blocks, not excerpts"
    );
    // One bound names one turn.
    let (_, one) = h.history("history_read", json!({"to_turn": "t2"}));
    assert_eq!(one["blocks"], p["blocks"]);
    // Across two sessions, in the order the wall has them.
    let (_, p) = h.history("history_read", json!({"from_turn": "t1", "to_turn": "t3"}));
    assert_eq!(
        ids(&p, "blocks"),
        vec![
            short(&w.ada),
            short(&w.noted),
            short(&w.again),
            short(&w.call),
            short(&w.result),
            short(&w.answer)
        ],
        "{p}"
    );
    let (m, _) = h.history("history_read", json!({"from_turn": "t3", "to_turn": "t1"}));
    assert_eq!(error_of(&m), "bad_range", "a range that runs backwards");
    let (m, p) = h.history("history_read", json!({"from_turn": "t9"}));
    assert_eq!(error_of(&m), "not_found", "{p}");
    let (m, _) = h.history(
        "history_read",
        json!({"id": short(&w.ada), "from_turn": "t1"}),
    );
    assert_eq!(error_of(&m), "bad_arguments", "an id or a range, not both");
    let (m, _) = h.history("history_read", json!({}));
    assert_eq!(error_of(&m), "bad_arguments", "neither");
}

#[test]
fn read_over_budget_is_too_large_never_cut() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("history", "read_budget", json!(50))]);
    let w = sown(&mut h);
    let body = canonical(&user("The lighthouse keeper is called Ada Quill."));
    assert!(
        body.len() > 50,
        "the block is over the budget this test sets"
    );
    let (m, p) = h.history("history_read", json!({"id": short(&w.ada)}));
    assert_eq!(error_of(&m), "too_large", "{p}");
    assert_eq!(
        p["size"],
        body.len(),
        "the refusal says how large the read is"
    );
    assert_eq!(p["budget"], 50);
    assert!(
        p.get("blocks").is_none(),
        "nothing of the block is delivered: {p}"
    );
    let (m, p) = h.history("history_read", json!({"from_turn": "t2"}));
    assert_eq!(error_of(&m), "too_large", "{p}");
    assert_eq!(
        p["block_count"], 3,
        "the range names how many blocks it holds: {p}"
    );
    // Within the budget the same read is whole.
    let mut h = Hive::with(&[("history", "read_budget", json!(body.len()))]);
    let w = sown(&mut h);
    let (m, p) = h.history("history_read", json!({"id": short(&w.ada)}));
    assert_eq!(error_of(&m), "", "{p}");
    assert_eq!(canonical(&p["blocks"][0]["block"]), body);
}

#[test]
fn a_foreign_id_is_not_found() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    sown(&mut h);
    // A block another model's ledger holds: this wall never carried it.
    let foreign = sha256_hex(&canonical(&user("what the reasoning core was told")));
    let (m, p) = h.history("history_read", json!({"id": short(&foreign)}));
    assert_eq!(error_of(&m), "not_found", "{p}");
    // A hash the ledger holds but no wall row names (a system leaf, a summary)
    // is no block of the model's turns either.
    let leaf = json!({"origin": "system", "type": "text", "text": "Be brief."});
    let leaf_hash = sha256_hex(&canonical(&leaf));
    h.db.execute(
        "INSERT INTO blocks (hash, kind, chars, body, first_seen) VALUES (?1, 'system', 9, ?2, ?3)",
        rusqlite::params![leaf_hash, canonical(&leaf), at(20, 9)],
    )
    .expect("a system block");
    let (m, _) = h.history("history_read", json!({"id": short(&leaf_hash)}));
    assert_eq!(error_of(&m), "not_found");
    let (m, _) = h.history("history_read", json!({"id": "#nothex!"}));
    assert_eq!(error_of(&m), "bad_arguments", "an id is 12 hex digits");
}

/// Bounded reads (GH #893): a range read is bounded on the wall -- it reads up to
/// the last row of its last turn, not to the wall's end -- and a range whose
/// rows cannot fit is refused before a single body is fetched: more rows than
/// `scan_budget`, or raw text over ten read budgets. An outline reads the
/// newest `scan_budget` rows and says when there were more.
#[test]
fn a_range_and_an_outline_read_a_bounded_wall() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("history", "scan_budget", json!(5))]);
    let a = h.sow(1, "s1", "t1", "user", &user("the first turn"), &at(20, 1));
    let b = h.sow(2, "s1", "t1", "assistant", &said("its answer"), &at(20, 1));
    for i in 3..=40_u32 {
        h.sow(
            i64::from(i),
            "s1",
            &format!("t{i}"),
            "user",
            &user(&format!("a later turn, number {i}")),
            &at(21, i / 2),
        );
    }
    let (m, p) = h.history("history_read", json!({"from_turn": "t1"}));
    assert_eq!(error_of(&m), "", "{p}");
    assert_eq!(ids(&p, "blocks"), vec![short(&a), short(&b)], "{p}");
    assert!(
        h.read_of("wall") <= 10,
        "read up to its last turn, not to the wall's end: {:?}",
        h.reads
    );
    assert_eq!(h.read_of("blocks:body"), 2, "{:?}", h.reads);
    h.reads.clear();
    let (m, p) = h.history("history_read", json!({"from_turn": "t1", "to_turn": "t40"}));
    assert_eq!(error_of(&m), "too_large", "{p}");
    assert_eq!(p["block_count_at_least"], 6, "{p}");
    assert_eq!(h.read_of("blocks:body"), 0, "refused unread: {:?}", h.reads);
    h.reads.clear();
    let (m, p) = h.history("history_outline", json!({}));
    assert_eq!(error_of(&m), "", "{p}");
    assert_eq!(p["truncated_scan"], true, "{p}");
    assert_eq!(p["sessions"][0]["first_turn"], "t36", "{p}");
    assert_eq!(p["sessions"][0]["turns"], 5, "{p}");
    assert!(h.read_of("wall") <= 6, "{:?}", h.reads);
    // Raw text over ten read budgets is refused before the bodies are read.
    let mut h = Hive::with(&[("history", "read_budget", json!(10))]);
    sown(&mut h);
    let (m, p) = h.history("history_read", json!({"from_turn": "t2"}));
    assert_eq!(error_of(&m), "too_large", "{p}");
    assert!(p["raw_size"].as_u64().unwrap_or(0) > 100, "{p}");
    assert_eq!(h.read_of("blocks:body"), 0, "{:?}", h.reads);
}

/// GH #893: the model's own history calls and their answers are on the wall
/// like every round the curator writes, but they are no hits of a search,
/// no neighbours and no blocks of a range read -- a second search would
/// otherwise find the first one's excerpts before the sources, and a range
/// would carry earlier reads over again. Read by its id, each is whole.
#[test]
fn the_models_own_history_calls_are_no_hits_and_no_range() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let w = sown(&mut h);
    let args = json!({"query": "Ada Quill"}).to_string();
    let call = h.sow(
        2_004,
        "s2",
        "t3",
        "tool_call",
        &json!({"origin": "assistant", "type": "tool_call", "id": "c-h1",
                "text": json!({"name": "history_search", "arguments": args}).to_string()}),
        &at(25, 10),
    );
    let said_before = json!({"hits": [{"excerpt": "The lighthouse keeper is called Ada Quill."}],
                             "tool": "history_search"})
    .to_string();
    let echo = h.sow(
        2_005,
        "s2",
        "t3",
        "tool_result",
        &json!({"origin": "tool", "type": "tool_result", "id": "c-h1", "text": said_before}),
        &at(25, 10),
    );
    // An answer whose call is not beside it is known by its own text.
    let stray = h.sow(
        2_006,
        "s2",
        "t3",
        "tool_result",
        &json!({"origin": "tool", "type": "tool_result", "id": "c-h0",
                "text": json!({"blocks": [{"block": {"text": "Ada Quill"}}],
                               "tool": "history_read"}).to_string()}),
        &at(25, 10),
    );
    let (_, p) = h.history("history_search", json!({"query": "Ada Quill"}));
    assert_eq!(
        ids(&p, "hits"),
        vec![short(&w.answer), short(&w.noted), short(&w.ada)],
        "the sources, not the earlier answers: {p}"
    );
    let (_, p) = h.history(
        "history_search",
        json!({"query": "history_", "mode": "exact"}),
    );
    assert_eq!(p["hits"], json!([]), "{p}");
    let (_, p) = h.history(
        "history_search",
        json!({"query": "Ada", "kinds": ["tool_call", "tool_result"]}),
    );
    assert_eq!(
        ids(&p, "hits"),
        vec![short(&w.result), short(&w.call)],
        "{p}"
    );
    let (_, p) = h.history(
        "history_search",
        json!({"query": "as you told me", "context": 2}),
    );
    assert_eq!(ids(&p["hits"][0], "after"), Vec::<String>::new(), "{p}");
    let (m, p) = h.history("history_read", json!({"from_turn": "t3"}));
    assert_eq!(error_of(&m), "", "{p}");
    assert_eq!(ids(&p, "blocks"), vec![short(&w.answer)], "{p}");
    assert_eq!(p["skipped_history"], 3, "{p}");
    for block in [&call, &echo, &stray] {
        let (m, p) = h.history("history_read", json!({"id": short(block)}));
        assert_eq!(error_of(&m), "", "{p}");
        assert_eq!(ids(&p, "blocks"), vec![short(block)], "{p}");
    }
}

/// Review focus (1): 12 hex digits practically never collide, and when they do
/// the read says so instead of picking one.
#[test]
fn an_id_that_names_two_blocks_is_ambiguous() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let a = format!("abcdef012345{}", "0".repeat(52));
    let b = format!("abcdef012345{}", "1".repeat(52));
    h.sow_as(&a, 1, "s1", "t1", "user", &user("first"), &at(20, 1));
    h.sow_as(&b, 2, "s1", "t1", "user", &user("second"), &at(20, 1));
    let (m, p) = h.history("history_read", json!({"id": "#abcdef012345"}));
    assert_eq!(error_of(&m), "ambiguous", "{p}");
    assert_eq!(p["candidates"].as_array().map(Vec::len), Some(2), "{p}");
    let (m, p) = h.history("history_read", json!({"id": "#abcdef0123451111"}));
    assert_eq!(error_of(&m), "", "a longer prefix settles it: {p}");
    assert_eq!(p["blocks"][0]["block"], user("second"));
}

/// Review focus (2): an empty wall answers, it does not fail.
#[test]
fn an_empty_wall_is_an_empty_answer() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let (m, p) = h.history("history_search", json!({"query": "anything"}));
    assert_eq!(error_of(&m), "", "{p}");
    assert_eq!(p["hits"], json!([]));
    assert_eq!(p["scanned"], 0);
    let (m, p) = h.history("history_outline", json!({}));
    assert_eq!(error_of(&m), "", "{p}");
    assert_eq!(p["sessions"], json!([]));
    let (m, _) = h.history("history_read", json!({"id": "#0123456789ab"}));
    assert_eq!(error_of(&m), "not_found");
}

/// Review focus (4): the turn that asks is on the wall already (`./intake`
/// writes the round before the call leaves), and a search finds it.
#[test]
fn a_search_finds_the_running_turn() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    sown(&mut h);
    let now = h.sow(
        9_000,
        "s-now",
        "t-now",
        "user",
        &user("what did I say about the lighthouse keeper?"),
        &at(28, 11),
    );
    let (_, p) = h.history(
        "history_search",
        json!({"query": "lighthouse keeper", "limit": 1}),
    );
    assert_eq!(ids(&p, "hits"), vec![short(&now)], "{p}");
    assert_eq!(p["hits"][0]["turn_id"], "t-now");
}

/// Review focus (5): Unicode in a pattern and in a phrase.
#[test]
fn unicode_in_a_regex_and_in_a_phrase() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let greet = h.sow(
        1,
        "s1",
        "t1",
        "user",
        &user("Grüße aus Köln, Straße 7 — bis bald"),
        &at(20, 1),
    );
    let (m, p) = h.history(
        "history_search",
        json!({"query": "Stra(ss|ß)e \\d", "mode": "regex"}),
    );
    assert_eq!(error_of(&m), "", "{p}");
    assert_eq!(ids(&p, "hits"), vec![short(&greet)], "{p}");
    assert!(
        p["hits"][0]["excerpt"]
            .as_str()
            .unwrap()
            .contains("Straße 7 — bis bald"),
        "{p}"
    );
    let (_, p) = h.history("history_search", json!({"query": "KÖLN"}));
    assert_eq!(
        ids(&p, "hits"),
        vec![short(&greet)],
        "a phrase ignores case beyond ASCII: {p}"
    );
    let (_, p) = h.history("history_search", json!({"query": "KÖLN", "mode": "exact"}));
    assert_eq!(p["hits"], json!([]), "{p}");
}

// =========================================================== 3. the outline

#[test]
fn outline_lists_sessions_with_turn_ranges() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    sown(&mut h);
    let (m, p) = h.history("history_outline", json!({}));
    assert_eq!(error_of(&m), "", "{p}");
    assert_eq!(
        p["sessions"],
        json!([
            {"session_id": "s1", "first_turn": "t1", "last_turn": "t1", "turns": 1,
             "first_at": at(20, 10), "last_at": at(20, 10), "topics": []},
            {"session_id": "s2", "first_turn": "t2", "last_turn": "t3", "turns": 2,
             "first_at": at(25, 9), "last_at": at(25, 10), "topics": []}
        ]),
        "oldest first, without marks no topics (OR-KY-H2): {p}"
    );
    assert_eq!(p["omitted_sessions"], 0);
    let (_, p) = h.history("history_outline", json!({"since": "2026-09-21"}));
    let names: Vec<&str> = p["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["session_id"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["s2"], "{p}");
    let (m, _) = h.history("history_outline", json!({"since": "soon"}));
    assert_eq!(error_of(&m), "bad_arguments");
}

/// A `topic` mark as OR-KY-71 has `./intake` write it: canonical JSON of the
/// movement and the name the `memory` section carried (keys sorted, no
/// whitespace; `name` empty when the section named none).
fn topic(movement: &str, name: &str) -> String {
    let v = json!({"movement": movement, "name": name});
    let s = canonical(&v);
    assert_eq!(
        s,
        format!(r#"{{"movement":"{movement}","name":"{name}"}}"#),
        "the canonical form"
    );
    s
}

/// OR-KY-71: the open topic is the name of the newest mark with a name and no
/// later `end`; a nameless `continue` or `end` belongs to it -- across
/// sessions, too. A topic that began before a session (or before anything the
/// wall holds) shows `from_turn` null; only the entry of the topic still open
/// at the end is `open`.
#[test]
fn outline_names_topics_from_marks() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    sown(&mut h);
    h.sow(
        3_000,
        "s3",
        "t5",
        "user",
        &user("And the ferry, is it running again?"),
        &at(26, 9),
    );
    // s1: a nameless start with nothing open opens a topic without a name;
    // then a thread that began before anything this wall holds is closed.
    h.mark(5, "s1", "t1", "topic", &topic("start", ""));
    h.mark(10, "s1", "t1", "topic", &topic("end", "the weather"));
    // s2: named start, the nameless continue and end belong to it.
    h.mark(
        20,
        "s2",
        "t2",
        "topic",
        &topic("start", "the lighthouse keeper"),
    );
    h.mark(21, "s2", "t3", "topic", &topic("continue", ""));
    h.mark(22, "s2", "t3", "topic", &topic("end", ""));
    h.mark(23, "s2", "t3", "topic", &topic("start", "the ferry"));
    // Not the form OR-KY-71 writes: read as nothing.
    h.mark(24, "s2", "t3", "topic", "start");
    // s3: the ferry goes on in the next session, nameless and then by name.
    h.mark(25, "s3", "t5", "topic", &topic("continue", ""));
    h.mark(26, "s3", "t5", "topic", &topic("continue", "the ferry"));
    let (m, p) = h.history("history_outline", json!({}));
    assert_eq!(error_of(&m), "", "{p}");
    assert_eq!(
        p["sessions"][0]["topics"],
        json!([
            {"name": null, "from_turn": "t1", "to_turn": "t1", "open": false},
            {"name": "the weather", "from_turn": null, "to_turn": "t1", "open": false}
        ]),
        "{p}"
    );
    assert_eq!(
        p["sessions"][1]["topics"],
        json!([
            {"name": "the lighthouse keeper", "from_turn": "t2", "to_turn": "t3", "open": false},
            {"name": "the ferry", "from_turn": "t3", "to_turn": "t3", "open": false}
        ]),
        "{p}"
    );
    assert_eq!(
        p["sessions"][2]["topics"],
        json!([{"name": "the ferry", "from_turn": null, "to_turn": "t5", "open": true}]),
        "{p}"
    );
}

/// The writer side of OR-KY-71, through the hive's own door: the `memory`
/// section the parent's splitter hands in on `in_section` leaves its `topic`
/// mark in the canonical form, and the outline names it.
#[test]
fn a_topic_the_memory_section_names_reaches_the_outline() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    sown(&mut h);
    h.lane(
        "in_section",
        json!({"session_id": "s2", "turn_id": "t2", "iter": "1", "curator_call": "cc-t"}),
        json!({"section": "memory"}),
        json!({"messages": [], "section": "memory",
               "payload": {"topic": {"movement": "start", "name": "the ferry"}}}),
    );
    let marks = h.rows("SELECT session_id, turn_id, value FROM marks WHERE kind = 'topic'");
    assert_eq!(
        marks,
        vec![vec![
            json!("s2"),
            json!("t2"),
            json!(topic("start", "the ferry"))
        ]],
        "one topic mark, canonical JSON with the name (OR-KY-71)"
    );
    let (_, p) = h.history("history_outline", json!({}));
    assert_eq!(
        p["sessions"][1]["topics"],
        json!([{"name": "the ferry", "from_turn": "t2", "to_turn": "t2", "open": true}]),
        "{p}"
    );
}

// ================================================= 4. the lane and the offer

/// A call the cell cannot serve is still answered under its id, so the round
/// it belongs to is complete and the model reads why.
#[test]
fn a_call_the_hive_cannot_serve_is_answered_not_dropped() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let (m, p) = h.history("history_rewrite", json!({"id": "#0123456789ab"}));
    assert_eq!(error_of(&m), "unknown_tool", "{p}");
    let (m, _) = h.history("history_read", json!("not an object"));
    assert_eq!(error_of(&m), "bad_arguments");
    let (m, _) = h.history("history_search", json!({"query": "   "}));
    assert_eq!(error_of(&m), "bad_arguments", "an empty query");
    let (m, _) = h.history("history_search", json!({"query": "x", "limit": "many"}));
    assert_eq!(error_of(&m), "bad_arguments");
}

#[test]
fn the_hive_takes_the_call_and_hands_back_the_result() {
    if !shipped() {
        return;
    }
    let hive = read_json(&repo("templates/curator/config.json"));
    let routes = |k: &str| -> Vec<String> {
        hive["params"]["contract"][k]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["route"].as_str().unwrap().to_string())
            .collect()
    };
    assert!(routes("accepts").contains(&"in_history_call".to_string()));
    assert!(routes("emits").contains(&"tool_result".to_string()));
    let edges = hive["params"]["graph"]["edges"].as_array().unwrap();
    let edge = |from: &str, to: &str| {
        edges
            .iter()
            .find(|e| e["from"] == from && e["to"] == to)
            .unwrap_or_else(|| panic!("no edge {from} -> {to}"))
            .clone()
    };
    assert!(
        edge(".", "./history")["condition"]
            .as_str()
            .unwrap()
            .contains("'in_history_call'")
    );
    let out = edge("./history", ".");
    assert!(out["condition"].as_str().unwrap().contains("'tool_result'"));
    assert_eq!(
        out["modifier"]["delete_context"],
        json!(["cur_origin", "cur_phase", "cur_call", "cur_reason"]),
        "the hive's own bookkeeping never leaves it"
    );
    assert_eq!(
        edge("./ledger", "./history")["condition"],
        "has(context.cur_origin) && context.cur_origin == 'curator-history'"
    );
    let cell = cell_config("history");
    assert_eq!(cell["cell"]["type"], "code");
    for (knob, default) in [
        ("read_budget", 40_000),
        ("scan_budget", 5_000),
        ("time_budget_ms", 3_000),
        // 400 since OR-BD-78: the longest search inside the GH #929 reserve.
        ("page_rows", 400),
    ] {
        assert_eq!(cell["params"][knob], default, "{knob}");
        assert_eq!(
            cell["contract"]["settings"][knob]["default"], default,
            "{knob}"
        );
        let literal = format!("_int(\"{knob}\", {default})");
        assert!(
            script_of("history").contains(&literal),
            "{knob}: the script's default is {literal}"
        );
    }
    assert!(
        cell["params"]["time_budget_ms"].as_u64() < cell["params"]["external_timeout_ms"].as_u64(),
        "a search answers before the runner would kill it"
    );
}

/// A top-level literal of a shipped script, read with `ast` and never run.
fn literal(script: &str, name: &str) -> Value {
    let program = format!(
        concat!(
            "import ast, json, sys\n",
            "tree = ast.parse(sys.stdin.read())\n",
            "for node in tree.body:\n",
            "    if isinstance(node, ast.Assign) and any(getattr(t, 'id', '') == {name:?} for t in node.targets):\n",
            "        print(json.dumps(ast.literal_eval(node.value)))\n",
            "        break\n"
        ),
        name = name
    );
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(program)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("python3");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(script.as_bytes())
        .expect("write the script");
    let out = child.wait_with_output().expect("wait");
    sj::from_slice(&out.stdout).unwrap_or_else(|e| panic!("`{name}` is no top-level literal: {e}"))
}

/// The schemas the model is offered (`HISTORY_SCHEMAS` in `./schemas`,
/// spliced into `CURATOR_OFFER`, OR-KY-G1/OR-KY.T.1) and the arguments
/// `./history` reads are one set of names: a property the schema promises and
/// the cell ignores is a promise nobody keeps.
#[test]
fn the_offer_names_what_the_cell_reads() {
    if !shipped() {
        return;
    }
    let offered = literal(&script_of("schemas"), "HISTORY_SCHEMAS");
    let read = literal(&script_of("history"), "ARGS");
    let mut names = Vec::new();
    for schema in offered.as_array().expect("a list of schemas") {
        let name = schema["name"].as_str().expect("a name").to_string();
        let mut props: Vec<String> = schema["parameters"]["properties"]
            .as_object()
            .expect("properties")
            .keys()
            .cloned()
            .collect();
        props.sort();
        let mut args: Vec<String> = read[&name]
            .as_array()
            .unwrap_or_else(|| panic!("{name}: the cell reads no such tool"))
            .iter()
            .map(|a| a.as_str().unwrap().to_string())
            .collect();
        args.sort();
        assert_eq!(props, args, "{name}: offered vs read");
        assert!(
            !schema["description"].as_str().unwrap_or("").is_empty(),
            "{name}"
        );
        names.push(name);
    }
    names.sort();
    assert_eq!(
        names,
        vec!["history_outline", "history_read", "history_search"]
    );
    assert_eq!(read.as_object().unwrap().len(), 3);
}

/// The curator answers the collector's menu question with the three tools
/// (OR-KY-G1): `./schemas` serves them by name and for `*`, and names what it
/// does not serve as unknown -- measured at the hive's `tool_schemas` exit, the
/// way the parent's collector receives it (`in_menu`).
#[test]
fn the_menu_answer_offers_the_history_tools() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let names = |m: &Msg| -> Vec<String> {
        let mut n: Vec<String> = m.body["schemas"]
            .as_array()
            .expect("schemas")
            .iter()
            .map(|s| s["name"].as_str().unwrap_or("").to_string())
            .collect();
        n.sort();
        n
    };
    h.lane(
        "in_schemas",
        json!({}),
        json!({}),
        json!({"messages": [], "tools": ["history_search", "history_read",
                                         "history_outline", "web_search"]}),
    );
    let out = h.routed("tool_schemas");
    assert_eq!(out.len(), 1, "{:?}", h.out);
    assert_eq!(
        names(&out[0]),
        vec!["history_outline", "history_read", "history_search"],
        "{:?}",
        out[0].body
    );
    assert_eq!(out[0].body["unknown"], json!(["web_search"]));
    for s in out[0].body["schemas"].as_array().unwrap() {
        assert!(
            s["parameters"]["properties"].is_object(),
            "a provider-neutral schema: {s}"
        );
    }
    h.out.clear();
    h.lane(
        "in_schemas",
        json!({}),
        json!({}),
        json!({"messages": [], "tools": ["*"]}),
    );
    let all = h.routed("tool_schemas");
    assert_eq!(
        names(&all[0]),
        vec!["history_outline", "history_read", "history_search"],
        "`*` is everything the curator serves"
    );
}

/// GH #845 and OR-KY-72: a channel's tool scope is a whitelist, and
/// the curator hands it on to the brain as the channel stamped it -- it never
/// adds its own tools to a present `allow` half. A channel that narrows a
/// model (a peer, a room) must not gain word-for-word access to the whole wall
/// by the curator's say; a channel that wants the history tools names them.
#[test]
fn the_curator_hands_a_channel_scope_on_unchanged() {
    if !shipped() {
        return;
    }
    let scoped = |scope: Value| -> Value {
        let mut h = Hive::new();
        h.out.clear();
        let hop = json!({"session_id": "s1", "turn_id": "t1", "iter": "0", "phase": ""});
        let ctx = json!({"session_id": "s1", "turn_id": "t1", "iter": "0"});
        let mut body = json!({"messages": [user("hello")]});
        if !scope.is_null() {
            body["tool_scope"] = scope;
        }
        h.lane("in_curate", ctx, hop, body);
        let calls = h.routed("brain");
        assert_eq!(calls.len(), 1, "{:?}", h.out);
        calls[0]
            .body
            .get("tool_scope")
            .cloned()
            .unwrap_or(Value::Null)
    };
    assert_eq!(
        scoped(json!({"allow": ["web_search"]})),
        json!({"allow": ["web_search"]}),
        "an allow list stays the channel's own: nothing of the curator's is added"
    );
    assert_eq!(
        scoped(json!({"allow": ["web_search", "history_read"], "deny": ["web_fetch"]})),
        json!({"allow": ["web_search", "history_read"], "deny": ["web_fetch"]}),
        "a channel that wants a history tool names it"
    );
    assert_eq!(
        scoped(json!({"deny": ["web_fetch"]})),
        json!({"deny": ["web_fetch"]}),
        "no allow half: every tool stays, the history tools with them"
    );
    assert_eq!(scoped(Value::Null), Value::Null, "no scope, no scope");
}

// ====================================== 4b. what a round may read of the wall

/// A round as the colony stamps it (GH #925): a JSON array of `member:<name>`,
/// carried as TEXT.
fn round(names: &[&str]) -> String {
    let members: Vec<String> = names.iter().map(|n| format!("member:{n}")).collect();
    sj::to_string(&members).expect("a JSON array")
}

/// The text of the one answer, as the model reads it.
fn text_of(m: &Msg) -> String {
    m.messages()[0]["text"]
        .as_str()
        .expect("a text")
        .to_string()
}

/// GH #925: a row reaches a round iff its audience holds the round -- what was
/// said before {e, a, b} in another session is found, read and shown with its
/// neighbours by a round of {e, a}.
#[test]
fn a_round_finds_what_a_wider_audience_heard_in_another_session() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    h.audience = Some(round(&["e", "a", "b"]));
    let wide = h.sow(
        1_000,
        "s1",
        "t1",
        "user",
        &user("The ferry leaves at nine."),
        &at(20, 10),
    );
    let reply = h.sow(
        1_001,
        "s1",
        "t1",
        "assistant",
        &said("Nine it is."),
        &at(20, 10),
    );
    h.round = json!(round(&["e", "a"]));
    let (m, p) = h.history(
        "history_search",
        json!({"query": "ferry leaves", "context": 1}),
    );
    assert_eq!(error_of(&m), "", "{p}");
    assert_eq!(ids(&p, "hits"), vec![short(&wide)], "{p}");
    assert_eq!(p["hits"][0]["session_id"], "s1", "another session: {p}");
    assert_eq!(ids(&p["hits"][0], "after"), vec![short(&reply)], "{p}");
    let (m, p) = h.history("history_read", json!({"id": short(&wide)}));
    assert_eq!(error_of(&m), "", "{p}");
    assert_eq!(p["blocks"][0]["block"], user("The ferry leaves at nine."));
    let (_, p) = h.history("history_read", json!({"from_turn": "t1"}));
    assert_eq!(ids(&p, "blocks"), vec![short(&wide), short(&reply)], "{p}");
    let (_, p) = h.history("history_outline", json!({}));
    assert_eq!(p["sessions"][0]["session_id"], "s1", "{p}");
    // The answer goes back into the round it was asked in.
    assert_eq!(m.context["audience_set"], json!(round(&["e", "a"])));
}

/// GH #925: a round WIDER than a row's audience finds nothing of it -- what
/// {e, a, b} or {e, a} heard is not said before {e, a, b, c}. The outline
/// names neither their sessions nor their topics, and a hidden row or topic
/// mark inside a session the round may see moves no turn and names no topic.
#[test]
fn a_wider_round_finds_nothing_said_before_fewer() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    h.audience = Some(round(&["e", "a", "b"]));
    h.sow(
        1_000,
        "s-three",
        "t1",
        "user",
        &user("The code word is heron."),
        &at(20, 10),
    );
    h.mark(
        10,
        "s-three",
        "t1",
        "topic",
        &topic("start", "the code word"),
    );
    h.audience = Some(round(&["e", "a"]));
    h.sow(
        2_000,
        "s-two",
        "t2",
        "user",
        &user("The code word is still heron."),
        &at(21, 10),
    );
    h.mark(
        20,
        "s-two",
        "t2",
        "topic",
        &topic("start", "the second code"),
    );
    h.audience = Some(round(&["e", "a", "b", "c"]));
    let open = h.sow(
        3_000,
        "s-four",
        "t3",
        "user",
        &user("Hello to all four of us, heron fans."),
        &at(22, 10),
    );
    h.mark(30, "s-four", "t3", "topic", &topic("start", "the greeting"));
    // Inside the session the round may see: a later turn and a topic that
    // only {e, a} heard.
    h.audience = Some(round(&["e", "a"]));
    h.sow(
        3_001,
        "s-four",
        "t4",
        "user",
        &user("Just between the two of us: heron."),
        &at(22, 11),
    );
    h.mark(
        31,
        "s-four",
        "t4",
        "topic",
        &topic("start", "the private plan"),
    );
    h.round = json!(round(&["e", "a", "b", "c"]));
    let (m, p) = h.history("history_search", json!({"query": "heron"}));
    assert_eq!(error_of(&m), "", "{p}");
    assert_eq!(ids(&p, "hits"), vec![short(&open)], "{p}");
    assert_eq!(p["total_hits"], 1, "a hidden hit is not counted: {p}");
    assert_eq!(p["scanned"], 1, "a hidden row is not counted: {p}");
    let (m, p) = h.history("history_outline", json!({}));
    assert_eq!(error_of(&m), "", "{p}");
    assert_eq!(
        p["sessions"],
        json!([{"session_id": "s-four", "first_turn": "t3", "last_turn": "t3", "turns": 1,
                "first_at": at(22, 10), "last_at": at(22, 10),
                "topics": [{"name": "the greeting", "from_turn": "t3", "to_turn": "t3",
                            "open": true}]}]),
        "{p}"
    );
    assert_eq!(p["omitted_sessions"], 0, "{p}");
    let text = text_of(&m);
    for trace in ["s-three", "s-two", "code", "private", "t4"] {
        assert!(!text.contains(trace), "`{trace}` in the outline: {text}");
    }
}

/// GH #925: a sibling round -- {e, b} beside {e, a} -- neither finds nor reads
/// the other's row. By its id, its turn or a range that reaches into it, the
/// answer is `not_found` word for word as for an id or a turn this wall never
/// carried, and an id that names a hidden block beside a visible one is no
/// ambiguity: a hidden row leaves no id and no count behind.
#[test]
fn a_sibling_round_neither_finds_nor_reads_a_row() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    h.audience = Some(round(&["e", "a"]));
    let secret = h.sow(
        1_000,
        "s1",
        "t-secret",
        "user",
        &user("The spare key is under the blue pot."),
        &at(20, 10),
    );
    h.audience = Some(round(&["e", "b"]));
    let own = h.sow(
        2_000,
        "s2",
        "t2",
        "user",
        &user("Where is the spare key?"),
        &at(21, 10),
    );
    h.round = json!(round(&["e", "b"]));
    let (m, p) = h.history("history_search", json!({"query": "spare key"}));
    assert_eq!(error_of(&m), "", "{p}");
    assert_eq!(ids(&p, "hits"), vec![short(&own)], "{p}");
    assert_eq!(p["total_hits"], 1, "{p}");
    assert_eq!(p["scanned"], 1, "{p}");
    assert!(!text_of(&m).contains("blue pot"), "{p}");

    let foreign = sha256_hex(&canonical(&user("never said on this wall")));
    let (m, as_foreign) = h.history("history_read", json!({"id": short(&foreign)}));
    assert_eq!(error_of(&m), "not_found", "{as_foreign}");
    let as_foreign = as_foreign
        .to_string()
        .replace(&foreign[..12], &secret[..12]);
    for id in [short(&secret), secret.clone()] {
        let (m, p) = h.history("history_read", json!({"id": id}));
        assert_eq!(error_of(&m), "not_found", "{id}: {p}");
        assert_eq!(
            p.to_string(),
            as_foreign,
            "the same words as an id no wall carries"
        );
    }

    let (_, unknown) = h.history("history_read", json!({"from_turn": "t-unknown"}));
    let unknown = unknown.to_string().replace("t-unknown", "t-secret");
    for range in [
        json!({"from_turn": "t-secret"}),
        json!({"from_turn": "t-secret", "to_turn": "t2"}),
    ] {
        let (m, p) = h.history("history_read", range.clone());
        assert_eq!(error_of(&m), "not_found", "{range}: {p}");
        assert_eq!(
            p.to_string(),
            unknown,
            "{range}: a turn this wall never had"
        );
    }

    let (m, p) = h.history("history_outline", json!({}));
    assert_eq!(p["sessions"].as_array().map(Vec::len), Some(1), "{p}");
    assert_eq!(p["sessions"][0]["session_id"], "s2", "{p}");
    assert!(!text_of(&m).contains("t-secret"), "{p}");

    // Two blocks share a prefix, one of them hidden: the prefix names the one
    // this round may see, and the hidden one is no candidate.
    let a = format!("abcdef012345{}", "0".repeat(52));
    let b = format!("abcdef012345{}", "1".repeat(52));
    h.audience = Some(round(&["e", "a"]));
    h.sow_as(&a, 3_000, "s1", "t5", "user", &user("first"), &at(22, 1));
    h.audience = Some(round(&["e", "b"]));
    h.sow_as(&b, 3_001, "s2", "t6", "user", &user("second"), &at(22, 2));
    let (m, p) = h.history("history_read", json!({"id": "#abcdef012345"}));
    assert_eq!(error_of(&m), "", "no ambiguity: {p}");
    assert_eq!(p["blocks"][0]["block"], user("second"), "{p}");
    assert!(p.get("candidates").is_none(), "{p}");
    // One block on the wall twice, once hidden: read where the round may see
    // it, and counted once.
    let twice = user("said twice");
    h.audience = Some(round(&["e", "a"]));
    let hash = h.sow(4_000, "s1", "t7", "user", &twice, &at(23, 1));
    h.audience = Some(round(&["e", "b"]));
    h.sow(4_001, "s2", "t8", "user", &twice, &at(23, 2));
    let (m, p) = h.history("history_read", json!({"id": short(&hash)}));
    assert_eq!(error_of(&m), "", "{p}");
    assert_eq!(p["blocks"][0]["session_id"], "s2", "{p}");
    assert_eq!(p["blocks"][0]["seq"], 4_001, "{p}");
    assert_eq!(p["blocks"][0]["appearances"], 1, "{p}");
}

/// GH #925: a call whose round declares no audience -- none at all, empty
/// text, or text that is no JSON array -- is refused with `missing_audience`
/// before a single ledger read, for every tool. The round is named before the
/// arguments: no argument the model could fix would make the call readable.
#[test]
fn a_call_without_a_round_is_refused_before_a_read() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let w = sown(&mut h);
    for no_round in [
        Value::Null,
        json!(""),
        json!("member:e"),
        json!(r#"{"member": "e"}"#),
    ] {
        h.round = no_round.clone();
        for (tool, args) in [
            ("history_search", json!({"query": "Ada"})),
            ("history_read", json!({"id": short(&w.ada)})),
            ("history_read", json!({"from_turn": "t1"})),
            ("history_outline", json!({})),
            ("history_search", json!({"query": "   "})),
        ] {
            h.reads.clear();
            let (m, p) = h.history(tool, args.clone());
            assert_eq!(
                error_of(&m),
                "missing_audience",
                "{tool} {args} in {no_round}: {p}"
            );
            assert_eq!(p["error"], "missing_audience", "{p}");
            assert!(
                p["detail"]
                    .as_str()
                    .is_some_and(|d| d.contains("audience_set")),
                "the detail names what is missing: {p}"
            );
            // `count_read` files every select, an empty one too.
            assert!(
                h.reads.is_empty(),
                "{tool} in {no_round}: not one ledger read: {:?}",
                h.reads
            );
        }
    }
    h.round = Value::Null;
    let (m, _) = h.history("history_rewrite", json!({}));
    assert_eq!(
        error_of(&m),
        "unknown_tool",
        "a tool this hive never serves"
    );
    assert!(h.rows("SELECT * FROM marks").is_empty());
}

/// GH #925 (OR-BD-5) and GH #932: a row from before the rule carries no
/// audience and reaches no round, not even the declared empty one. That empty
/// round used to reach every row WITH an audience, in any session -- a `[]`
/// call saw what {e} said elsewhere. Since GH #932 the store reads it like a
/// round-less call (`round_where`, `covers` refuses an empty round): only the
/// rows of the running session that declare no round either (`[]`, PP-BD-12)
/// or name `*`. A row a declared round said, a round-less row of another
/// session and a NULL row of its own are all not found, word for word as an
/// id or a turn the wall never carried.
#[test]
fn the_declared_empty_round_reads_only_its_sessions_round_less_rows() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    // `history` asks in session `s-now`.
    h.audience = None;
    let old = h.sow(
        1_000,
        "s-now",
        "t-old",
        "user",
        &user("An old remark about the garden."),
        &at(20, 10),
    );
    h.audience = Some("[]".to_string());
    let new = h.sow(
        2_000,
        "s-now",
        "t-new",
        "user",
        &user("A new remark about the garden."),
        &at(21, 10),
    );
    h.audience = Some(round(&["e"]));
    let heard = h.sow(
        3_000,
        "s-now",
        "t-heard",
        "user",
        &user("A remark about the garden only e heard."),
        &at(22, 10),
    );
    h.audience = Some("[]".to_string());
    let elsewhere = h.sow(
        4_000,
        "s-else",
        "t-else",
        "user",
        &user("A round-less remark about the garden elsewhere."),
        &at(23, 10),
    );
    h.audience = None;
    h.mark(
        5,
        "s-now",
        "t-new",
        "topic",
        &topic("start", "the old plan"),
    );
    h.round = json!("[]");
    let (m, p) = h.history("history_search", json!({"query": "garden"}));
    assert_eq!(error_of(&m), "", "the empty round is declared: {p}");
    assert_eq!(ids(&p, "hits"), vec![short(&new)], "{p}");
    assert_eq!(p["total_hits"], 1, "{p}");
    assert_eq!(p["scanned"], 1, "a row it may not see is not read: {p}");
    let foreign = sha256_hex(&canonical(&user("never said on this wall")));
    let (_, unknown) = h.history("history_read", json!({"id": short(&foreign)}));
    for hidden in [&old, &heard, &elsewhere] {
        let (m, p) = h.history("history_read", json!({"id": short(hidden)}));
        assert_eq!(error_of(&m), "not_found", "{p}");
        assert_eq!(
            p.to_string(),
            unknown.to_string().replace(&foreign[..12], &hidden[..12]),
            "the same words as an id no wall carries"
        );
    }
    let (_, unknown) = h.history("history_read", json!({"from_turn": "t-unknown"}));
    for turn in ["t-old", "t-heard", "t-else"] {
        let (m, p) = h.history("history_read", json!({"from_turn": turn}));
        assert_eq!(error_of(&m), "not_found", "{turn}: {p}");
        assert_eq!(
            p.to_string(),
            unknown.to_string().replace("t-unknown", turn),
            "{turn}: a turn this wall never had"
        );
    }
    let (m, p) = h.history("history_read", json!({"from_turn": "t-new"}));
    assert_eq!(error_of(&m), "", "{p}");
    assert_eq!(ids(&p, "blocks"), vec![short(&new)], "{p}");
    let (m, p) = h.history("history_outline", json!({}));
    assert_eq!(
        p["sessions"],
        json!([{"session_id": "s-now", "first_turn": "t-new", "last_turn": "t-new",
                "turns": 1, "first_at": at(21, 10), "last_at": at(21, 10), "topics": []}]),
        "{p}"
    );
    let text = text_of(&m);
    for trace in ["old", "heard", "s-else", "t-else"] {
        assert!(!text.contains(trace), "`{trace}` in the outline: {p}");
    }
}

/// GH #925: a neighbour the round may not see is no neighbour -- the nearest
/// row it may see stands in its place, and not a word of the hidden one is in
/// the answer.
#[test]
fn a_neighbour_of_another_round_is_left_out() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let first = h.sow(1, "s1", "t1", "user", &user("first words"), &at(20, 1));
    h.audience = Some(round(&["a"]));
    let aside = h.sow(
        2,
        "s1",
        "t2",
        "user",
        &user("an aside for someone else"),
        &at(20, 2),
    );
    h.audience = Some(ROUND_E.to_string());
    let hit = h.sow(
        3,
        "s1",
        "t3",
        "user",
        &user("the needle itself"),
        &at(20, 3),
    );
    h.audience = None;
    let old = h.sow(
        4,
        "s1",
        "t4",
        "assistant",
        &said("an old reply"),
        &at(20, 4),
    );
    h.audience = Some(round(&["e", "a"]));
    let last = h.sow(5, "s1", "t5", "assistant", &said("last words"), &at(20, 5));
    for width in [1, 2] {
        let (m, p) = h.history(
            "history_search",
            json!({"query": "needle", "context": width}),
        );
        assert_eq!(error_of(&m), "", "{p}");
        assert_eq!(ids(&p, "hits"), vec![short(&hit)], "{p}");
        assert_eq!(ids(&p["hits"][0], "before"), vec![short(&first)], "{p}");
        assert_eq!(ids(&p["hits"][0], "after"), vec![short(&last)], "{p}");
        let text = text_of(&m);
        for trace in ["aside", "old reply", "t2", "t4", &aside[..12], &old[..12]] {
            assert!(!text.contains(trace), "`{trace}`: {text}");
        }
    }
}

/// GH #925: a turn range reads the rows the round may see and skips the rest
/// without a gap -- no block, no size, no count of the hidden ones. A bound
/// whose turn holds only hidden rows is a turn this wall never had.
#[test]
fn a_range_skips_the_rows_it_may_not_show() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let ask = user("what is the plan?");
    let q = h.sow(1, "s1", "t1", "user", &ask, &at(20, 1));
    h.audience = Some(round(&["b"]));
    h.sow(
        2,
        "s1",
        "t1",
        "assistant",
        &said("the plan for b only"),
        &at(20, 1),
    );
    h.audience = Some(ROUND_E.to_string());
    let all = said("the plan for all");
    let a = h.sow(3, "s1", "t1", "assistant", &all, &at(20, 1));
    h.audience = Some(round(&["b"]));
    h.sow(4, "s1", "t2", "user", &user("b's own turn"), &at(20, 2));
    h.audience = Some(ROUND_E.to_string());
    let and_later = user("and later");
    let later = h.sow(5, "s1", "t3", "user", &and_later, &at(20, 3));
    let (m, p) = h.history("history_read", json!({"from_turn": "t1", "to_turn": "t3"}));
    assert_eq!(error_of(&m), "", "{p}");
    assert_eq!(
        ids(&p, "blocks"),
        vec![short(&q), short(&a), short(&later)],
        "{p}"
    );
    assert_eq!(p["skipped_history"], 0, "{p}");
    assert_eq!(
        p["size"],
        canonical(&ask).len() + canonical(&all).len() + canonical(&and_later).len(),
        "the size of what it shows: {p}"
    );
    assert!(!text_of(&m).contains("b only"), "{p}");
    let (_, unknown) = h.history("history_read", json!({"from_turn": "t9"}));
    for range in [
        json!({"from_turn": "t2"}),
        json!({"from_turn": "t1", "to_turn": "t2"}),
    ] {
        let (m, p) = h.history("history_read", range.clone());
        assert_eq!(error_of(&m), "not_found", "{range}: {p}");
        assert_eq!(
            p.to_string(),
            unknown.to_string().replace("t9", "t2"),
            "{range}"
        );
    }
}

/// GH #925: the `reread` mark a read writes carries the round that read
/// (`audience_of(context.audience_set)`), and a `release` mark another round
/// wrote is not this round's to see -- its block is read as not released and
/// marks no re-read.
#[test]
fn a_reread_mark_carries_the_round() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    h.audience = Some(round(&["e", "a", "b"]));
    let kept = h.sow(
        1_000,
        "s1",
        "t1",
        "user",
        &user("Ada Quill keeps the lighthouse."),
        &at(20, 10),
    );
    let noted = h.sow(1_001, "s1", "t1", "assistant", &said("Noted."), &at(20, 10));
    h.mark(5, "s1", "t1", "release", &kept[..12]);
    h.audience = Some(round(&["b"]));
    h.mark(6, "s1", "t1", "release", &noted[..12]);
    // The round arrives as the colony writes it, unsorted and with spaces.
    h.round = json!(r#"["member:e", "member:a"]"#);
    let (m, p) = h.history("history_read", json!({"id": short(&kept)}));
    assert_eq!(error_of(&m), "", "{p}");
    assert_eq!(p["blocks"][0]["released"], true, "{p}");
    assert_eq!(
        h.rows("SELECT value, audience_set FROM marks WHERE kind = 'reread'"),
        vec![vec![
            json!(kept.clone()),
            json!(r#"["member:a","member:e"]"#)
        ]],
        "one `reread` mark, in the round's canonical audience"
    );
    let (m, p) = h.history("history_read", json!({"id": short(&noted)}));
    assert_eq!(error_of(&m), "", "{p}");
    assert_eq!(
        p["blocks"][0]["released"], false,
        "a release another round wrote: {p}"
    );
    assert_eq!(
        h.rows("SELECT * FROM marks WHERE kind = 'reread'").len(),
        1,
        "and no re-read of it"
    );
}

/// GH #931: `released` -- and the `reread` mark a read writes -- is decided
/// by the `release`/`pin` marks the round may see. The read took the newest
/// `scan_budget` such marks of the whole ledger and gated them after, so marks
/// of another round pushed the round's own out: under `scan_budget` 10 a block
/// released by a visible mark read as not released from 10 hidden newer marks
/// on (9 left it right), and a range of five released blocks lost them one by
/// one (`-RRRR` at 6, `-----` and no `reread` mark from 10 on). Now a read
/// takes the marks of its own blocks only, the newest `scan_budget` of them the
/// round may see. Since GH #932 the store hands back no hidden mark at all
/// (`round_where`, `covers`): however many marks of another round stand on the
/// read's blocks, the answer is the one without them -- the cut form a read
/// took once hidden pins of its own block filled the row bound (19 and more
/// under `scan_budget` 10) no longer occurs, and the `reread` mark is written.
#[test]
fn released_is_decided_by_the_marks_the_round_may_see() {
    if !shipped() {
        return;
    }
    // One block `x`, released by a visible mark; `k` hidden marks newer than
    // it, of other blocks (`same` false) or of `x` itself (pins, `same` true).
    let by_id = |k: i64, same: bool| -> (String, Value, usize) {
        let mut h = Hive::with(&[("history", "scan_budget", json!(10))]);
        h.audience = Some(ROUND_E.to_string());
        let x = h.sow(100, "s1", "t1", "user", &user("block x"), &at(20, 1));
        h.mark(1, "s1", "t1", "release", &x[..12]);
        h.audience = Some(round(&["a"]));
        for j in 0..k {
            if same {
                h.mark(2 + j, "s-x", "tx", "pin", &x[..12]);
            } else {
                h.mark(
                    2 + j,
                    "s-x",
                    "tx",
                    "release",
                    &format!("{:012x}", 0xabc000 + j),
                );
            }
        }
        h.audience = Some(ROUND_E.to_string());
        let (code, p, _) = asked(&mut h, "history_read", json!({"id": short(&x)}));
        let rereads = h.rows("SELECT * FROM marks WHERE kind = 'reread'").len();
        (code, p, rereads)
    };
    let (code, plain, rereads) = by_id(0, false);
    assert_eq!(code, "", "{plain}");
    assert_eq!(plain["blocks"][0]["released"], true, "{plain}");
    assert_eq!(rereads, 1);
    for k in [9, 10, 25, 100] {
        assert_eq!(
            by_id(k, false),
            (String::new(), plain.clone(), 1),
            "{k} hidden marks of other blocks moved the read"
        );
    }
    // Hidden marks of the block itself: a hidden pin is no later word. GH
    // #932: from 19 on (past the row bound of twice `scan_budget`) the read
    // answered in the cut form and wrote no `reread` mark; the store reads
    // none of them now, so every count answers as the ledger without them.
    for k in [10, 18, 19, 25, 40] {
        assert_eq!(
            by_id(k, true),
            (String::new(), plain.clone(), 1),
            "{k} hidden pins of the block moved the read"
        );
    }

    // A range of five blocks, each released by a visible mark, under `k`
    // hidden newer marks of other blocks.
    let range = |k: i64| -> (Value, usize) {
        let mut h = Hive::with(&[("history", "scan_budget", json!(10))]);
        h.audience = Some(ROUND_E.to_string());
        for i in 1..=5_i64 {
            let x = h.sow(
                100 * i,
                "s1",
                &format!("t{i}"),
                "user",
                &user(&format!("block x{i}")),
                &at(20, i as u32),
            );
            h.mark(10 * i, "s1", &format!("t{i}"), "release", &x[..12]);
        }
        h.audience = Some(round(&["a"]));
        for j in 0..k {
            h.mark(
                1_000 + j,
                "s-x",
                "tx",
                "release",
                &format!("{:012x}", 0xabc000 + j),
            );
        }
        h.audience = Some(ROUND_E.to_string());
        let (code, p, _) = asked(
            &mut h,
            "history_read",
            json!({"from_turn": "t1", "to_turn": "t5"}),
        );
        assert_eq!(code, "", "{p}");
        (p, h.rows("SELECT * FROM marks WHERE kind = 'reread'").len())
    };
    let (plain, rereads) = range(0);
    assert_eq!(
        plain["blocks"]
            .as_array()
            .expect("blocks")
            .iter()
            .map(|b| b["released"].clone())
            .collect::<Vec<_>>(),
        vec![json!(true); 5],
        "{plain}"
    );
    assert_eq!(rereads, 5);
    for k in [6, 10, 30] {
        assert_eq!(
            range(k),
            (plain.clone(), 5),
            "{k} hidden marks moved the range"
        );
    }

    // Blocks no longer compete for one window: a visibly released block stays
    // released under more than `scan_budget` newer visible marks of others.
    let mut h = Hive::with(&[("history", "scan_budget", json!(10))]);
    h.audience = Some(ROUND_E.to_string());
    let x = h.sow(100, "s1", "t1", "user", &user("block x"), &at(20, 1));
    h.mark(1, "s1", "t1", "release", &x[..12]);
    for j in 0..30_i64 {
        h.mark(
            2 + j,
            "s1",
            "t1",
            "release",
            &format!("{:012x}", 0xdef000 + j),
        );
    }
    let (code, p, _) = asked(&mut h, "history_read", json!({"id": short(&x)}));
    assert_eq!(code, "", "{p}");
    assert_eq!(p["blocks"][0]["released"], true, "{p}");
}

/// GH #925 (OR-BD-74): a wall of `needle` rows the round may see (`visible`,
/// session `s1`) and rows of another round (`hidden`, session `s-other`),
/// sown in `seq` order under `scan_budget` 10 and `page_rows` 4 -- three
/// visible pages of four, four and two rows.
fn needle_wall(visible: &[i64], hidden: &[i64]) -> Hive {
    let mut h = Hive::with(&[
        ("history", "scan_budget", json!(10)),
        ("history", "page_rows", json!(4)),
    ]);
    let mut seqs: Vec<i64> = visible.iter().chain(hidden).copied().collect();
    seqs.sort_unstable();
    for seq in seqs {
        let hour = 1 + (seq / 100 % 20) as u32;
        if hidden.contains(&seq) {
            h.audience = Some(round(&["a"]));
            h.sow(
                seq,
                "s-other",
                &format!("th{seq}"),
                "user",
                &user(&format!("needle hidden {seq}")),
                &at(20, hour),
            );
        } else {
            h.audience = Some(ROUND_E.to_string());
            h.sow(
                seq,
                "s1",
                &format!("t{seq}"),
                "user",
                &user(&format!("needle number {seq}")),
                &at(20, hour),
            );
        }
    }
    h.audience = Some(ROUND_E.to_string());
    h
}

/// One call: its error code, its answer and the ledger round trips it took.
fn asked(h: &mut Hive, tool: &str, args: Value) -> (String, Value, usize) {
    h.trips = 0;
    let (m, p) = h.history(tool, args);
    (error_of(&m), p, h.trips)
}

/// The same call on the visible rows alone and with the hidden ones among
/// them: the same answer, word for word, and the same ledger round trips.
/// GH #932: the store filters every read of the round (`round_where`), so a
/// hidden row never comes back and costs no read -- before, it could cost one
/// round trip more (the read that planned the rest). Returns the answer.
fn alike(visible: &[i64], hidden: &[i64], tool: &str, args: Value) -> Value {
    let (c1, p1, t1) = asked(&mut needle_wall(visible, &[]), tool, args.clone());
    let (c2, p2, t2) = asked(&mut needle_wall(visible, hidden), tool, args.clone());
    assert_eq!(
        (c2.as_str(), &p2),
        (c1.as_str(), &p1),
        "{tool} {args}: the hidden rows {hidden:?} moved the answer"
    );
    assert_eq!(
        t2, t1,
        "{tool} {args}: {t2} ledger round trips, {t1} without the hidden rows"
    );
    p1
}

/// GH #925 (review I-1, OR-BD-74): `scan_budget` and the pages of a search
/// count only the rows the round may see. Rows of another round among them --
/// in the newest page, in the last one, after the budget's last row -- move
/// nothing the answer says: `scanned`, `total_hits`, `cut_by`, the hits and
/// the page where `limit` stops are those of the same wall without them, so a
/// cut is no counter of the hidden rows. Since GH #932 they cost no ledger
/// round trip either: the store never hands them back (`alike`).
#[test]
fn a_budget_counts_only_the_rows_the_round_may_see() {
    if !shipped() {
        return;
    }
    let visible: Vec<i64> = (1..=14).map(|i| i * 100).collect();
    // Seven hidden rows, fewer than (2 - 1) x `scan_budget`.
    let hidden = [1_450, 1_250, 950, 650, 450, 50, 30];
    let p = alike(
        &visible,
        &hidden,
        "history_search",
        json!({"query": "needle", "limit": 20}),
    );
    assert_eq!(p["scanned"], 10, "{p}");
    assert_eq!(p["total_hits"], 10, "{p}");
    assert_eq!(p["cut_by"], "scan_budget", "{p}");
    assert_eq!(p["hits"].as_array().map(Vec::len), Some(10), "{p}");
    assert_eq!(p["hits"][9]["seq"], 500, "the tenth visible row: {p}");
    // `limit` stops at the visible page that fills it and counts it whole.
    let p = alike(
        &visible,
        &hidden,
        "history_search",
        json!({"query": "needle", "limit": 5}),
    );
    assert_eq!(p["stopped_at_limit"], true, "{p}");
    assert_eq!(p["total_hits"], 8, "two visible pages: {p}");
    for args in [
        json!({"query": "needle", "limit": 1}),
        json!({"query": "needle", "limit": 6, "context": 1}),
        json!({"query": "number (3|7|9)00$", "mode": "regex", "limit": 20}),
    ] {
        alike(&visible, &hidden, "history_search", args);
    }
    // Exactly the budget, then only hidden rows: no cut, as without them.
    let p = alike(
        &visible[..10],
        &[1_450, 50, 40, 30],
        "history_search",
        json!({"query": "needle", "limit": 20}),
    );
    assert_eq!(p["truncated_scan"], false, "{p}");
    // The newest page wholly hidden; hidden rows only in the last page.
    for hidden in [&[1_410, 1_420, 1_430, 1_440][..], &[610, 620, 630][..]] {
        alike(
            &visible,
            hidden,
            "history_search",
            json!({"query": "needle", "limit": 20}),
        );
    }
    // The round trips: the first read, ceil(10 / 4) = 3 pages, the
    // neighbours' two -- 6, with nine hidden rows as without them (GH #932:
    // no read that plans the rest, no wider neighbour read).
    let crowd = [1_410, 1_420, 1_430, 1_440, 610, 620, 630, 450, 440];
    let args = json!({"query": "needle", "limit": 20, "context": 1});
    let p = alike(&visible, &crowd, "history_search", args.clone());
    assert_eq!(p["scanned"], 10, "{p}");
    let (_, _, trips) = asked(&mut needle_wall(&visible, &crowd), "history_search", args);
    assert_eq!(trips, 1 + 3 + 2, "{trips} ledger round trips");
}

/// GH #925 (OR-BD-74, OR-BD-86) and GH #932: a wall full of hidden rows
/// answers as the wall without them, however many there are. Before GH #932
/// hidden rows were read to a bound of their own (twice `scan_budget` rows in
/// all), and past it the search answered in the cut form -- 25 hidden rows
/// before three visible ones cut it. The store filters every read of the round
/// now (`round_where`): a hidden row is never read, so there is no bound for
/// it to reach and no cut it could cause.
#[test]
fn hidden_rows_past_the_old_bound_leave_the_search_as_without_them() {
    if !shipped() {
        return;
    }
    let old = [10, 20, 30];
    // 9 hidden rows (below the old bound), 25 (past it), 100 (far past it).
    for n in [9, 25, 100] {
        let crowd: Vec<i64> = (100..100 + n).collect();
        let p = alike(
            &old,
            &crowd,
            "history_search",
            json!({"query": "needle", "limit": 8}),
        );
        assert_eq!(p["truncated_scan"], false, "{n} hidden: {p}");
        assert_eq!(p["scanned"], 3, "{n} hidden: {p}");
        assert_eq!(p["total_hits"], 3, "{n} hidden: {p}");
    }
}

/// `history_read` of the turn range `t1`..`t2` under `scan_budget` 10: turn
/// `t1` holds the first half of `n` rows (`seq` 100, 200, ...), `t2` the
/// rest; the `hidden` rows (another round's) stand among them in the same
/// session.
fn turn_range(n: i64, hidden: &[i64]) -> (String, Value, usize) {
    let mut h = Hive::with(&[("history", "scan_budget", json!(10))]);
    let mut seqs: Vec<i64> = (1..=n)
        .map(|i| i * 100)
        .chain(hidden.iter().copied())
        .collect();
    seqs.sort_unstable();
    for seq in seqs {
        let turn = if seq <= n * 50 { "t1" } else { "t2" };
        let text = if hidden.contains(&seq) {
            h.audience = Some(round(&["a"]));
            format!("hidden {seq}")
        } else {
            h.audience = Some(ROUND_E.to_string());
            format!("row {seq}")
        };
        h.sow(seq, "s1", turn, "user", &user(&text), &at(20, 1));
    }
    h.audience = Some(ROUND_E.to_string());
    asked(
        &mut h,
        "history_read",
        json!({"from_turn": "t1", "to_turn": "t2"}),
    )
}

/// GH #925 (OR-BD-74): an outline covers the newest `scan_budget` rows the
/// round may see, and a turn range is too large by the rows it may see --
/// hidden rows neither shorten the one nor refuse the other, and a refusal
/// names the count the same wall without them names.
#[test]
fn an_outline_and_a_range_count_only_the_rows_the_round_may_see() {
    if !shipped() {
        return;
    }
    let visible: Vec<i64> = (1..=14).map(|i| i * 100).collect();
    let p = alike(
        &visible,
        &[1_450, 1_440, 1_430, 1_250, 950],
        "history_outline",
        json!({}),
    );
    assert_eq!(p["sessions"][0]["turns"], 10, "{p}");
    assert_eq!(p["truncated_scan"], true, "{p}");
    let (c1, p1, t1) = turn_range(8, &[]);
    let (c2, p2, t2) = turn_range(8, &[150, 250, 350, 450, 550]);
    assert_eq!(c1, "", "{p1}");
    assert_eq!(
        (c2, &p2),
        (c1, &p1),
        "eight rows it may see are no range too large"
    );
    // GH #932: the store never hands a hidden row back, so they cost no read.
    assert_eq!(t2, t1, "{t2} ledger round trips, {t1} without");
    let (c1, p1, _) = turn_range(12, &[]);
    let (c2, p2, _) = turn_range(12, &[150, 250, 350]);
    assert_eq!(c1, "too_large", "{p1}");
    assert_eq!(p1["block_count_at_least"], 11, "{p1}");
    assert_eq!(
        (c2, &p2),
        (c1, &p1),
        "the refusal names what the round may see"
    );
}

/// GH #925 (OR-BD-86, closing review I-2) and GH #932: walls with the same
/// visible rows and different numbers of hidden rows (another round's, on top
/// of the wall) answer every call of every tool word for word as the wall
/// without hidden rows, in the same ledger round trips -- however many hidden
/// rows there are. Under `scan_budget` 10 the closing review read the hidden
/// count off the answer at the row bound (11/12/14/16/19 hidden -> `scanned`
/// 9/8/6/4/1, a range's `block_count_at_least` 21 - hidden, the sessions of an
/// outline); OR-BD-86 closed that with one cut form past the bound, the one bit
/// hidden rows still moved. GH #932 closes the bit: the store filters every
/// read of the round (`round_where`), no hidden row is read, so no bound is
/// reached and the walls that answered in the cut form answer in full.
#[test]
fn every_wall_answers_as_it_does_without_hidden_rows() {
    if !shipped() {
        return;
    }
    let visible: Vec<i64> = (1..=14).map(|i| i * 100).collect();
    let on_top = |k: i64| -> Vec<i64> { (1..=k).map(|j| 1_400 + j).collect() };
    // The search: below, at and past the old bound of 20 rows in all.
    for (args, hidden) in [
        (json!({"query": "needle", "limit": 20}), [9, 10, 13, 19]),
        (
            json!({"query": "needle", "limit": 8, "context": 1}),
            [10, 13, 16, 19],
        ),
        (
            json!({"query": "number (3|7|9|11|13)00$", "mode": "regex", "limit": 20}),
            [9, 10, 14, 19],
        ),
    ] {
        for k in hidden {
            let p = alike(&visible, &on_top(k), "history_search", args.clone());
            assert!(p.get("detail").is_none(), "{args}, {k} hidden: {p}");
        }
    }
    // The outline: 10 hidden rows were below the old bound, 11 and more cut it.
    for k in [10, 11, 15, 19] {
        let p = alike(&visible, &on_top(k), "history_outline", json!({}));
        assert!(p.get("detail").is_none(), "{k} hidden: {p}");
        assert_eq!(p["sessions"][0]["session_id"], "s1", "{k} hidden: {p}");
    }
    // The turn range: eight visible rows, `k` hidden ones in turn `t1`; past
    // 20 rows in all it was refused without a count, now it is the plain read.
    let hidden = |k: i64| -> Vec<i64> { (1..=k).map(|j| 100 + j).collect() };
    let (c1, p1, t1) = turn_range(8, &[]);
    assert_eq!(c1, "", "{p1}");
    for k in [12, 13, 16, 30] {
        let (c2, p2, t2) = turn_range(8, &hidden(k));
        assert_eq!((&c2, &p2), (&c1, &p1), "{k} hidden rows: the plain read");
        assert_eq!(t2, t1, "{k} hidden: {t2} ledger round trips, {t1} without");
    }
}

/// Session `s1` around one `the needle` row: `older` visible rows before it
/// and `newer` after it (`seq` 1000 apart), with `before` and `after` rows of
/// another round of the SAME session right beside it.
fn near_wall(
    over: &[(&str, &str, Value)],
    older: i64,
    newer: i64,
    before: i64,
    after: i64,
) -> Hive {
    let mut h = Hive::with(over);
    let needle = (older + 1) * 1_000;
    let visible = (1..=older + 1 + newer).map(|i| i * 1_000);
    let hidden = (1..=before)
        .map(|j| needle - j)
        .chain((1..=after).map(|j| needle + j));
    let mut rows: Vec<(i64, bool)> = visible
        .map(|s| (s, false))
        .chain(hidden.map(|s| (s, true)))
        .collect();
    rows.sort_unstable();
    for (seq, is_hidden) in rows {
        let text = if is_hidden {
            h.audience = Some(round(&["a"]));
            format!("an aside {seq}")
        } else {
            h.audience = Some(ROUND_E.to_string());
            if seq == needle {
                "the needle".to_string()
            } else {
                format!("row {seq}")
            }
        };
        h.sow(
            seq,
            "s1",
            &format!("t{seq}"),
            "user",
            &user(&text),
            &at(20, 1),
        );
    }
    h.audience = Some(ROUND_E.to_string());
    h
}

/// GH #925 (OR-BD-86, closing review I-2) and GH #932: the neighbours of a hit
/// are the `context` nearest rows the round may see in its session, as on the
/// wall without hidden rows, in the same ledger round trips -- for any number
/// of hidden rows beside it. Before OR-BD-86, `context` + 4 rows were read and
/// hidden ones dropped: 5 (`context` 1) or 6 (`context` 2) hidden rows beside
/// a hit took a neighbour away; then a side whose read held a hidden row was
/// read again, wider (one round trip more), and past its share of the row
/// bound (15 hidden rows on one side under `scan_budget` 10) the whole search
/// answered in the cut form. The store filters the neighbour reads now
/// (`round_where`): no hidden row is read, no side is read again, no cut.
#[test]
fn the_neighbours_are_the_nearest_rows_the_round_may_see() {
    if !shipped() {
        return;
    }
    let ids_of = |p: &Value, side: &str| -> Vec<String> {
        p["hits"][0][side]
            .as_array()
            .unwrap_or_else(|| panic!("`{side}` neighbours: {p}"))
            .iter()
            .map(|n| n["excerpt"].as_str().unwrap_or("").to_string())
            .collect()
    };
    for width in [1, 2] {
        let args = json!({"query": "needle", "context": width});
        let (c1, p1, t1) = asked(
            &mut near_wall(&[], 6, 6, 0, 0),
            "history_search",
            args.clone(),
        );
        assert_eq!(c1, "", "{p1}");
        let (before, after) = if width == 1 {
            (&["row 6000"][..], &["row 8000"][..])
        } else {
            (&["row 5000", "row 6000"][..], &["row 8000", "row 9000"][..])
        };
        assert_eq!(ids_of(&p1, "before"), before, "{p1}");
        assert_eq!(ids_of(&p1, "after"), after, "{p1}");
        for (b, a) in [(1, 0), (5, 0), (6, 6), (0, 6), (8, 3), (40, 40)] {
            let (c2, p2, t2) = asked(
                &mut near_wall(&[], 6, 6, b, a),
                "history_search",
                args.clone(),
            );
            assert_eq!(
                (c2.as_str(), &p2),
                (c1.as_str(), &p1),
                "context {width}, {b} hidden before and {a} after the hit"
            );
            assert_eq!(
                t2, t1,
                "{t2} ledger round trips, {t1} without the hidden rows"
            );
        }
    }
    // The old share of the row bound: `scan_budget` 10 -> 20 rows for the one
    // side short of its window (`context` 2: six rows the round may see); 15
    // and more hidden rows beside the hit cut the search before GH #932.
    // `limit` 1 and `page_rows` 1 stop the scan itself after the hit.
    let over = [
        ("history", "scan_budget", json!(10)),
        ("history", "page_rows", json!(1)),
    ];
    let args = json!({"query": "needle", "context": 2, "limit": 1});
    let (c1, p1, t1) = asked(
        &mut near_wall(&over, 20, 0, 0, 0),
        "history_search",
        args.clone(),
    );
    assert_eq!(c1, "", "{p1}");
    assert_eq!(ids_of(&p1, "before"), ["row 19000", "row 20000"], "{p1}");
    for b in [14, 15, 18, 60] {
        let (c2, p2, t2) = asked(
            &mut near_wall(&over, 20, 0, b, 0),
            "history_search",
            args.clone(),
        );
        assert_eq!(
            (c2.as_str(), &p2),
            (c1.as_str(), &p1),
            "{b} hidden rows beside the hit"
        );
        assert_eq!(t2, t1, "{b} hidden: {t2} ledger round trips, {t1} without");
    }
}

// ======================================================= 5. the audience gate

/// The cells of this hive that write or read `wall`, `calls`, `marks`,
/// `summaries` or `pins` (GH #925) -- `./writer` too: it hands the wall rows
/// of the session it closes to the memory, and only those the close round may
/// see (closing review I-3, OR-BD-86).
const GATED_CELLS: [&str; 6] = ["intake", "policy", "push", "handover", "history", "writer"];

/// The gate functions of every gated cell, read with `ast`, run against one
/// table: the affinity rows (`templates/affinity/README.md`) plus the edges --
/// `*`, a row without an audience, the declared empty round, a round that is
/// no JSON array, the round-less read of the running session (only its rows
/// that declare no round, `[]`, or name `*`, review M-7 and PP-BD-12), and the
/// intersection a summary or a handover block carries. GH #932 adds the store
/// half of the gate, `round_where` with its `ROUNDLESS` list: the `where`
/// terms every ledger read of a round carries, so a hidden row is never read.
/// It is pinned as the same text in every cell like the Python half -- a cell
/// whose store filter drifted from the others would read rows they never see.
const GATE_TABLE: &str = r##"# The gate table (GH #925): runs the gate functions of every curator cell that
# carries them and prints one JSON document. Input: JSON {cell: script} on stdin.
import ast, json, sys
NAMES = ("audience_of", "allowed", "passes", "audience_meet", "round_where")
# Top-level constants a gate function reads (GH #932: `round_where` names the
# round-less audiences out of `ROUNDLESS`), pinned as the same text too.
CONSTS = ("ROUNDLESS",)
scripts = json.load(sys.stdin)
src_of, results = {}, {}
E, A, B, C = "member:e", "member:a", "member:b", "member:c"
L = lambda *xs: json.dumps(list(xs))
CASES = [
    ["allowed", [L(E, A, B, C), L(E, A, B)], False],
    ["allowed", [L(E, A), L(E, A, B)], True],
    ["allowed", [L(E, B), L(E, A)], False],
    ["allowed", [L(E, A), L("*")], True],
    ["allowed", [L(E, A), None], False],
    ["allowed", [L(E, A), ""], False],
    ["allowed", ["[]", L(E)], True],
    ["allowed", [None, L("*")], False],
    ["allowed", ["not json", L(E, A)], False],
    ["allowed", ['"member:e"', L(E, A)], False],
    ["allowed", [[A, E], '["member:e","member:b","member:a"]'], True],
    ["audience_of", ['["member:e","member:a","member:a"]'], '["member:a","member:e"]'],
    ["audience_of", [[B, A]], '["member:a","member:b"]'],
    ["audience_of", ["[]"], "[]"],
    ["audience_of", [" [ \"*\" ] "], '["*"]'],
    ["audience_of", [""], None],
    ["audience_of", [None], None],
    ["audience_of", ["x"], None],
    ["audience_of", ['{"a": 1}'], None],
    ["audience_of", [5], None],
    # GH #932 / PP-BD-12: a round-less row carries `[]`; a row WITHOUT an
    # audience (NULL) is one from before the rule and passes no read, the
    # round-less one of its own session neither.
    ["passes", [{"session_id": "s1"}, None, "s1"], False],
    ["passes", [{"session_id": "s2", "audience_set": L("*")}, None, "s1"], False],
    ["passes", [{}, None, ""], False],
    ["passes", [{"session_id": "s2", "audience_set": L(E, A, B)}, L(E, A), "s1"], True],
    ["passes", [{"session_id": "s1", "audience_set": L(E)}, L(E, A), "s1"], False],
    ["passes", [{"session_id": "s1"}, "", "s1"], False],
    # Review M-7: without a round, a row a declared round said in the running
    # session stays -- only what no round declared (`[]`, PP-BD-12), or what
    # names `*`, passes (GH #932: `[]` passes now, NULL no longer does).
    ["passes", [{"session_id": "s1", "audience_set": L(E, B)}, None, "s1"], False],
    ["passes", [{"session_id": "s1", "audience_set": "[]"}, None, "s1"], True],
    ["passes", [{"session_id": "s2", "audience_set": "[]"}, None, "s1"], False],
    ["passes", [{"session_id": "s1", "audience_set": None}, None, "s1"], False],
    ["passes", [{"session_id": "s1", "audience_set": L("*")}, None, "s1"], True],
    ["passes", [{"session_id": "s1", "audience_set": L(E, A)}, "", "s1"], False],
    ["audience_meet", [[L(E, A, B), L(E, A)]], '["member:a","member:e"]'],
    ["audience_meet", [[L("*"), L(E, A)]], '["member:a","member:e"]'],
    ["audience_meet", [[L("*"), L("*")]], '["*"]'],
    ["audience_meet", [[L(E, A), None]], None],
    ["audience_meet", [[]], None],
    ["audience_meet", [[L(E), L(A)]], "[]"],
    # GH #932, the store half: a declared, non-empty round reads with `covers`
    # (its canonical members); none, an empty text or the declared EMPTY round
    # (which `covers` refuses) reads only the round-less and `*` rows of the
    # running session; without a session nothing may be read at all.
    ["round_where", [L(E, A), "s1"], {"audience_set": {"covers": [A, E]}}],
    ["round_where", [[B, A], ""], {"audience_set": {"covers": [A, B]}}],
    ["round_where", [L("*"), "s1"], {"audience_set": {"covers": ["*"]}}],
    ["round_where", [None, "s1"], {"session_id": "s1", "audience_set": {"in": ["[]", L("*")]}}],
    ["round_where", ["", "s1"], {"session_id": "s1", "audience_set": {"in": ["[]", L("*")]}}],
    ["round_where", ["[]", "s1"], {"session_id": "s1", "audience_set": {"in": ["[]", L("*")]}}],
    ["round_where", [None, ""], None],
    ["round_where", ["[]", None], None],
]
for cell, script in sorted(scripts.items()):
    tree = ast.parse(script)
    defs = {n.name: n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name in NAMES}
    consts = {t.id: n for n in tree.body if isinstance(n, ast.Assign)
              for t in n.targets if isinstance(t, ast.Name) and t.id in CONSTS}
    src_of[cell] = {k: ast.get_source_segment(script, v)
                    for k, v in list(defs.items()) + list(consts.items())}
    ns = {"json": json}
    for name in CONSTS:
        if name in consts:
            exec(compile(ast.Module(body=[consts[name]], type_ignores=[]), cell, "exec"), ns)
    for name in NAMES:
        if name in defs:
            exec(compile(ast.Module(body=[defs[name]], type_ignores=[]), cell, "exec"), ns)
    bad = []
    for fn, args, want in CASES:
        if fn not in ns:
            bad.append("%s missing" % fn)
            continue
        got = ns[fn](*args)
        if got != want:
            bad.append("%s%r = %r, want %r" % (fn, tuple(args), got, want))
    results[cell] = bad
print(json.dumps({"sources": src_of, "failures": results}))"##;

#[test]
fn the_audience_gate_is_one_rule_in_every_cell() {
    if !shipped() {
        return;
    }
    let scripts: Map<String, Value> = GATED_CELLS
        .iter()
        .map(|c| (c.to_string(), Value::String(script_of(c))))
        .collect();
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(GATE_TABLE)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(Value::Object(scripts).to_string().as_bytes())
        .expect("write the scripts");
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "the table ran: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: Value = sj::from_slice(&out.stdout).expect("one JSON document");
    for cell in GATED_CELLS {
        assert_eq!(
            report["failures"][cell],
            json!([]),
            "`{cell}` answers the gate table"
        );
    }
    let first = &report["sources"][GATED_CELLS[0]];
    for name in [
        "audience_of",
        "allowed",
        "passes",
        "audience_meet",
        "round_where",
        "ROUNDLESS",
    ] {
        assert!(
            first[name].is_string(),
            "`{}` defines `{name}`",
            GATED_CELLS[0]
        );
    }
    for cell in &GATED_CELLS[1..] {
        assert_eq!(
            &report["sources"][cell], first,
            "`{cell}` carries the gate as the same text as `{}`",
            GATED_CELLS[0]
        );
    }
}

// ================================================== 5b. the meet of rows

/// The cells that write a row made of other rows (GH #932, PP-BD-12): `policy`
/// (the summary, a model pin) and `handover` (the note, the leaf) through
/// `meet_of_rows`, `push` (the addendum) through `addendum_audience`.
const MEET_CELLS: [&str; 3] = ["policy", "handover", "push"];

/// The meet functions of `MEET_CELLS`, read with `ast`, run against one table.
/// Since PP-BD-12 `[]` marks a round-less row, which the round-less reads of
/// its session take; an EMPTY meet of declared rounds ({a} and {b}, or {e,a}
/// and a round-less row) must therefore come out None -- the row keeps its
/// column NULL and reaches no round -- and never `[]`. `meet_of_rows` is pinned
/// as the same text in `policy` and `handover`; `push` answers the same rule
/// for its two-sided `addendum_audience`. The row cases run the writers: the
/// insert row `policy` builds with `with_meet`, the addendum mark `push`
/// builds with `stamped(..., meet=True)`, and the note mark the `handover`
/// script itself writes in its `note` step, run whole over a closed session
/// whose rows carry the given audiences.
const MEET_TABLE: &str = r##"# The meet table (GH #932, PP-BD-12): runs the meet functions of the curator
# cells that write a row made of other rows and prints one JSON document.
# Input: JSON {cell: script} on stdin (`policy`, `handover`, `push`).
import ast, json, subprocess, sys
NAMES = ("audience_of", "audience_meet", "meet_of_rows", "with_meet",
         "addendum_audience", "stamped")
scripts = json.load(sys.stdin)
src_of, results = {}, {}
E, A, B = "member:e", "member:a", "member:b"
L = lambda *xs: json.dumps(list(xs))
EA = '["member:a","member:e"]'
# `meet_of_rows`, the same rows in `policy` and `handover`: an EMPTY meet is
# the round-less `[]` only when every source naming no `*` is round-less too;
# an empty meet of declared rounds names nobody -- None, never `[]`.
MEET = [
    ["meet_of_rows", [[L(A), L(B)]], None],
    ["meet_of_rows", [[L(E, A), "[]"]], None],
    ["meet_of_rows", [["[]", L(E, A)]], None],
    ["meet_of_rows", [["[]", "[]"]], "[]"],
    ["meet_of_rows", [["[]", L("*")]], "[]"],
    ["meet_of_rows", [[L("*"), "[]"]], "[]"],
    ["meet_of_rows", [[L(E, A), L("*")]], EA],
    ["meet_of_rows", [[L(E, A, B), L(E, A)]], EA],
    ["meet_of_rows", [[L("*"), L("*")]], '["*"]'],
    ["meet_of_rows", [[L(E, A), None]], None],
    ["meet_of_rows", [[None, "[]"]], None],
    ["meet_of_rows", [["[]", "not json"]], None],
    ["meet_of_rows", [[]], None],
    ["meet_of_rows", [["[]"]], "[]"],
    ["meet_of_rows", [[L(E, A), L(E, B), "[]"]], None],
]
# `addendum_audience(round, gap)` in `push`: the same rule for the addendum --
# both round-less (or the gap names `*`) is `[]`; a declared side against a
# round-less one, or two rounds that share nobody, is None.
ADDENDUM = [
    ["addendum_audience", [None, "[]"], "[]"],
    ["addendum_audience", ["[]", "[]"], "[]"],
    ["addendum_audience", [None, L("*")], "[]"],
    ["addendum_audience", [None, L(E, A)], None],
    ["addendum_audience", ["[]", L(E, A)], None],
    ["addendum_audience", [None, None], None],
    ["addendum_audience", [L(E, A), "[]"], None],
    ["addendum_audience", [L(A), L(B)], None],
    ["addendum_audience", [L(E, A), None], None],
    ["addendum_audience", [L(E, A), L(E, A)], EA],
    ["addendum_audience", [L(E, A), L(A, B)], '["member:a"]'],
    ["addendum_audience", [L(E, A), L("*")], EA],
    ["addendum_audience", [L(A, E, A), L(E, A, B)], EA],
]
# The rows: what a writer puts in the column when the meet of declared rounds
# is empty -- no `audience_set` at all (NULL), never `[]`.
ROW = {"hash": "h"}
ROWS = {
    "policy": [
        # the summary (`s-row`) and a model pin (`f-pin`): `with_meet`
        ["with_meet", [ROW, ["meet_of_rows", [[L(A), L(B)]]]], {"hash": "h"}],
        ["with_meet", [ROW, ["meet_of_rows", [[L(E, A), "[]"]]]], {"hash": "h"}],
        ["with_meet", [ROW, ["meet_of_rows", [["[]", "[]"]]]], {"hash": "h", "audience_set": "[]"}],
        ["with_meet", [ROW, ["meet_of_rows", [[L(E, A), L(E, A, B)]]]], {"hash": "h", "audience_set": EA}],
    ],
    "push": [
        # the addendum mark: `stamped(..., meet=True)` over `addendum_audience`
        ["stamped", [ROW, ["addendum_audience", [L(A), L(B)]], True], {"hash": "h"}],
        ["stamped", [ROW, ["addendum_audience", [None, L(E, A)]], True], {"hash": "h"}],
        ["stamped", [ROW, ["addendum_audience", [None, "[]"]], True], {"hash": "h", "audience_set": "[]"}],
        ["stamped", [ROW, ["addendum_audience", [L(E, A), L(E, A)]], True], {"hash": "h", "audience_set": EA}],
    ],
}
WRAP = ("import io, json, sys\n"
        "_d = json.load(sys.stdin)\n"
        "sys.stdin = io.StringIO(_d['doc'])\n"
        "exec(compile(_d['script'], 'cell', 'exec'), {'__name__': '__main__'})\n")


def run_cell(script, doc):
    p = subprocess.run([sys.executable, "-c", WRAP], capture_output=True, text=True,
                       input=json.dumps({"script": script, "doc": json.dumps(doc)}))
    return json.loads(p.stdout or "[]"), p.stderr


def note_mark(script, auds):
    """The `handover` note of a closed session whose rows carry `auds`: the
    `w-mark` row the `note` step writes (two steps: the block check, then the
    writes)."""
    ctx = {"cur_phase": "note", "cur_call": "s3", "session_id": "s3"}
    wall = [dict({"seq": i + 1, "session_id": "s3", "turn_id": "t%d" % i, "kind": "user",
                  "hash": "h%d" % i, "final": 0, "at": "x"},
                 **({} if a is None else {"audience_set": a}))
            for i, a in enumerate(auds)]
    data = {"r-wall": wall,
            "r-pend": [{"key": "pending:handover:s3",
                        "value": json.dumps({"text": "a note", "model": "m"})}]}
    ops, err = ["r-wall", "r-marks", "r-pend"], ""
    for _ in range(3):
        body = {"messages": [{"origin": "tool", "id": cid, "text": json.dumps(data.get(cid, []))}
                             for cid in ops]}
        doc = {"body": body, "params": {},
               "envelope": {"header": {"hop": {"operation": "select"}, "context": ctx}}}
        out, err = run_cell(script, doc)
        if not out:
            break
        msgs = {m["id"]: json.loads(m["text"]) for m in out[0].get("messages", [])}
        if "w-mark" in msgs:
            return msgs["w-mark"]["row"]
        ops = list(msgs)
    return {"error": "no w-mark", "stderr": err[-400:]}


for cell, script in sorted(scripts.items()):
    tree = ast.parse(script)
    defs = {n.name: n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name in NAMES}
    src_of[cell] = {k: ast.get_source_segment(script, v) for k, v in defs.items()}
    ns = {"json": json}
    for name in NAMES:
        if name in defs:
            exec(compile(ast.Module(body=[defs[name]], type_ignores=[]), cell, "exec"), ns)
    cases = (MEET if cell in ("policy", "handover") else ADDENDUM) + ROWS.get(cell, [])
    bad = []
    for fn, args, want in cases:
        # A row case hands the writer the value of an inner call `[name, args]`.
        inner = [a[0] for a in args if isinstance(a, list) and a and a[0] in NAMES]
        if fn not in ns or any(n not in ns for n in inner):
            bad.append("%s missing" % "/".join([fn] + inner))
            continue
        args = [ns[a[0]](*a[1]) if isinstance(a, list) and a and a[0] in NAMES else a
                for a in args]
        got = ns[fn](*args)
        if got != want:
            bad.append("%s%r = %r, want %r" % (fn, tuple(args), got, want))
    if cell == "handover":
        for auds, want in [([L(A), L(B)], None), ([L(A), "[]"], None),
                           ([L(E, A), L(E, B)], '["member:e"]'), (["[]", "[]"], "[]"),
                           ([L(E, A), L(E, A, B)], EA), ([L(E, A), None], None)]:
            row = note_mark(script, auds)
            got = row.get("audience_set", "<absent>")
            if "error" in row or got != (want if want is not None else "<absent>"):
                bad.append("note over %r: %r, want audience_set %r" % (auds, row, want))
    results[cell] = bad
print(json.dumps({"sources": src_of, "failures": results}))"##;

#[test]
fn an_empty_meet_of_declared_rounds_is_null_never_round_less() {
    if !shipped() {
        return;
    }
    let scripts: Map<String, Value> = MEET_CELLS
        .iter()
        .map(|c| (c.to_string(), Value::String(script_of(c))))
        .collect();
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(MEET_TABLE)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(Value::Object(scripts).to_string().as_bytes())
        .expect("write the scripts");
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "the table ran: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: Value = sj::from_slice(&out.stdout).expect("one JSON document");
    for cell in MEET_CELLS {
        assert_eq!(
            report["failures"][cell],
            json!([]),
            "`{cell}` answers the meet table"
        );
    }
    let policy = &report["sources"]["policy"]["meet_of_rows"];
    assert!(policy.is_string(), "`policy` defines `meet_of_rows`");
    assert_eq!(
        &report["sources"]["handover"]["meet_of_rows"], policy,
        "`handover` carries `meet_of_rows` as the same text as `policy`"
    );
    for (cell, name) in [
        ("policy", "with_meet"),
        ("push", "addendum_audience"),
        ("push", "stamped"),
    ] {
        assert!(
            report["sources"][cell][name].is_string(),
            "`{cell}` defines `{name}`"
        );
    }
}

/// GH #929 reserve, as a history search spends it: every ledger round trip is
/// two routing decisions on the chain of the call, and the rest of the S3
/// segment (brain -> dispatcher -> curator -> `./history` -> `tool_result` ->
/// curator entry) is fixed -- the chain-length lock measured 18 used at four
/// round trips, so 10.
const S3_FIXED: usize = 10;
/// The most a segment between two seams may use: the default ttl 64 less the
/// reserve 16 (GH #929).
const SEGMENT_MAX: usize = 64 - 16;

/// GH #925 (OR-BD-78, OR-BD-86) and GH #932: the ledger round trips of the
/// longest search at `scan_budget` `budget` and `page_rows` `page` -- the
/// first read (`search-rows`), one per page the budget needs (`search-page`,
/// ceil(budget / page): each page's bundle reads the page, its bodies and
/// what follows), and the neighbours' two (`search-near`, `search-near-2`).
/// Before GH #932 a hidden row added two more: the read that planned the rest
/// of the wall (OR-BD-74) and the wider read beside a hit (OR-BD-86). The
/// store filters every read of the round now (`round_where`), so neither is
/// ever taken. It depends on the number of pages alone, not on the rows in
/// them, which is what lets
/// `the_longest_search_takes_the_round_trips_worked_out_from_its_settings`
/// prove it on a small wall.
fn longest_search_trips(budget: usize, page: usize) -> usize {
    1 + budget.div_ceil(page) + 2
}

/// GH #925 (OR-BD-78, OR-BD-86): at the SHIPPED settings the longest search
/// stays inside the GH #929 reserve: 1 + ceil(5000 / 400) + 2 = 16 ledger
/// round trips, 10 + 2 x 16 = 42 on the chain (18 and 46 before GH #932
/// dropped the two reads hidden rows cost).
/// Worked out from `params` alone, so a changed knob fails here without a
/// colony and without a clock; that the script takes exactly these trips is
/// pinned by the run below.
#[test]
fn the_longest_search_at_the_shipped_settings_fits_the_chain_reserve() {
    if !shipped() {
        return;
    }
    let params = &cell_config("history")["params"];
    let knob = |k: &str| -> usize {
        params[k]
            .as_u64()
            .or_else(|| params[k].as_str().and_then(|s| s.parse().ok()))
            .unwrap_or_else(|| panic!("history `{k}` is a number: {params}")) as usize
    };
    let (budget, page) = (knob("scan_budget"), knob("page_rows"));
    let bound = longest_search_trips(budget, page);
    assert!(
        S3_FIXED + 2 * bound <= SEGMENT_MAX,
        "scan_budget {budget} / page_rows {page}: up to {bound} ledger round trips, a \
         history call of {} on the chain -- over the reserve of {SEGMENT_MAX} (GH #929)",
        S3_FIXED + 2 * bound
    );
}

/// GH #925 (OR-BD-78, OR-BD-86) and GH #932: the longest search takes EXACTLY
/// the round trips `longest_search_trips` works out -- a wall past the budget,
/// the one hit in its oldest scanned row with `context` 2, one row of another
/// round on top and six of the hit's own session on either side of it. The
/// hidden rows stay on the wall: they cost no round trip and take no
/// neighbour away, since the store never hands them back.
///
/// Run at small settings, not the shipped ones: at 5000 / 400 the wall is
/// 6000 rows, 1.7 s here, and the public CI runner (2026-10-01) spent the
/// shipped 3000 ms `time_budget_ms` at 4000 of them -- `cut_by` `time_budget`,
/// a cut of its own and not the row bounds this test is about. The clock
/// stays as shipped; the wall shrinks instead. 50 / 4 is the shipped ratio
/// (13 pages, the last one short: the same 16 trips over 60 rows), 48 / 4
/// ends on a full page (12 pages, 15 trips), 20 / 20 is one page (4 trips).
#[test]
fn the_longest_search_takes_the_round_trips_worked_out_from_its_settings() {
    if !shipped() {
        return;
    }
    for (budget, page) in [(50_usize, 4_usize), (48, 4), (20, 20)] {
        let at_settings = format!("scan_budget {budget} / page_rows {page}");
        let mut h = Hive::with(&[
            ("history", "scan_budget", json!(budget)),
            ("history", "page_rows", json!(page)),
        ]);
        let newest = (budget + budget / 5) as i64;
        let hit = (newest - budget as i64 + 1) * 10;
        for seq in (1..=newest).map(|i| i * 10) {
            h.audience = Some(ROUND_E.to_string());
            let text = if seq == hit {
                format!("needle number {seq}")
            } else {
                format!("hay number {seq}")
            };
            h.sow(
                seq,
                "s1",
                &format!("t{seq}"),
                "user",
                &user(&text),
                &at(20, 1),
            );
        }
        h.audience = Some(round(&["a"]));
        h.sow(
            newest * 10 + 5,
            "s-other",
            "t-other",
            "user",
            &user("needle hidden"),
            &at(20, 2),
        );
        for seq in (1..=6).flat_map(|d| [hit - d, hit + d]) {
            h.sow(
                seq,
                "s1",
                &format!("t{seq}"),
                "user",
                &user(&format!("an aside {seq}")),
                &at(20, 1),
            );
        }
        let (code, p, trips) = asked(
            &mut h,
            "history_search",
            json!({"query": "needle", "limit": 8, "context": 2}),
        );
        assert_eq!(code, "", "{at_settings}: {p}");
        assert_ne!(
            p["cut_by"], "time_budget",
            "{at_settings}: the shipped `time_budget_ms` ran out before the row bounds -- \
             the host was too slow even for this small wall; a cut of its own, not the \
             round trips this test pins: {p}"
        );
        assert_eq!(
            p["total_hits"], 1,
            "{at_settings}: the hit in the oldest scanned row: {p}"
        );
        assert_eq!(p["cut_by"], "scan_budget", "{at_settings}: {p}");
        let near = |side: &str| -> Vec<String> {
            p["hits"][0][side]
                .as_array()
                .unwrap_or_else(|| panic!("{at_settings}: `{side}`: {p}"))
                .iter()
                .map(|n| n["excerpt"].as_str().unwrap_or("").to_string())
                .collect()
        };
        assert_eq!(
            near("before"),
            [
                format!("hay number {}", hit - 20),
                format!("hay number {}", hit - 10)
            ],
            "{at_settings}: the nearest rows the round may see, past six hidden ones: {p}"
        );
        assert_eq!(
            near("after"),
            [
                format!("hay number {}", hit + 10),
                format!("hay number {}", hit + 20)
            ],
            "{at_settings}: {p}"
        );
        assert_eq!(
            trips,
            longest_search_trips(budget, page),
            "{at_settings}: the ledger round trips of the longest search"
        );
    }
}

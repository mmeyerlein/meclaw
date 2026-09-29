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
        let msg = Msg {
            context: obj(context),
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
        self.db
            .execute(
                "INSERT INTO wall (seq, session_id, turn_id, iter, kind, hash, nth, final, \
                 episode_idx, at) VALUES (?1, ?2, ?3, 0, ?4, ?5, 0, 1, NULL, ?6)",
                rusqlite::params![seq, session, turn, kind, hash, when],
            )
            .expect("a wall row");
    }

    /// One `marks` row (README § 2, written by T's TRIM and topic ingress).
    fn mark(&mut self, seq: i64, session: &str, turn: &str, kind: &str, value: &str) {
        self.db
            .execute(
                "INSERT INTO marks (seq, session_id, turn_id, kind, value, at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![seq, session, turn, kind, value, at(28, 12)],
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
        let ctx = json!({"session_id": session, "turn_id": turn, "iter": "1",
                         "curator_call": "cc-893"});
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
    // The question of s1 was released (a `#` form is read too) and pinned
    // again later: the later word wins (OR-KY.T.13), it is no released block.
    h.mark(6, "s1", "t1", "release", &short(&w.ada));
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
        ("page_rows", 250),
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

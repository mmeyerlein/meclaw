//! GH #949 -- an app reads its round off the curator's wall, and nothing else.
//!
//! An app of a member that runs a round of its own (a nightly pass, a digest, a
//! follow-up) could pull nothing but the memory hive's recall: the curator's
//! ledger -- the record of every turn its model saw -- had no reader outside
//! the hive (`./history` answers the model's own `history_*` calls). The
//! curator's `./reader` is that reader, on the hive lane `in_read`, answered
//! on `read`:
//!
//! - `turns`: the wall rows of the round, oldest first after the cursor
//!   `since`, at most `limit` a page, each with the text of its block; `next`
//!   is the cursor of the rest, '' at the end;
//! - `sessions`: the sessions of the round with their first and last row.
//!
//! The round is `context.audience_set` and nothing else, and the lane is
//! fail-closed on it: without a round -- none, unparsable, or the declared
//! empty one -- the answer is `missing_audience` and not one ledger op runs.
//! With one, every read selects through the store's `covers` (GH #932): a row
//! reaches the round only when its own audience holds the round or names `*`,
//! a row without an audience (NULL, from before the rule) reaches no round, and
//! the pages, cursors and counts of a round are those of the same wall without
//! the rows of other rounds. A store that refuses a read is `store_error`,
//! never an empty page. `hop.read_tag` is echoed on every answer.
//!
//! Two layers:
//!
//! 1. **The hive**: the shipped curator edges (`. -> ./reader`, the ledger
//!    round trip, `./reader -> .`), evaluated by the colony's own `apply_edges`
//!    and CEL; the shipped `./reader` script under python3; the ledger an
//!    in-memory SQLite with the shipped schema, every op through the store
//!    cell's own dispatcher -- so `covers` is the real one. Every emission of
//!    the reader is checked against the cell's `contract.emits`.
//! 2. **The road**: the shipped member, its container as `examples/organism`
//!    grows it, the generation `sam`, its three brains with their curators,
//!    and the edges `install_app` renders for two apps that declare `reads`
//!    (`gh949_an_app_declares_reads.rs` locks the rendering). A question
//!    emitted by an app's cell walks to the curator of the brain `hop.organ`
//!    names, the reader reads THAT brain's ledger, and the answer walks back
//!    to the asking cell -- never to the other app. A question that names no
//!    brain takes no edge at the generation.
//!
//! **Whose round.** The round a read is answered in is NOT the app's word.
//! The context an app's message carries is written by the app's own template
//! -- nothing stops an edge inside it from setting `audience_set` to anything,
//! and `["agent:<a>"]` covers every row the agent is part of. So the question
//! edge `install_app` draws stamps the MEMBER's round,
//! `["agent:<generation>","member:<ctx.member_person>"]`, over whatever the
//! message carried (review C-1); the road reads exactly that round's rows,
//! whatever round the app forged (`a_forged_round_reads_only_the_members_round`).
//!
//! **A read without a running turn.** A timer strike is a fresh root without
//! a context: `crates/meclaw-cells/src/timer/cell.rs` emits it through the
//! `OriginSink` (`parent_message_id` none, a fresh trace, step 3 of the fire
//! arm) and `build_fire_content` puts the order's `emit_headers` into the HOP
//! and nothing into the context (`docs/cell-types.md` § `timer`). On the road
//! that no longer matters: the stamp gives an app woken by its own timer its
//! member's round, and the read is answered (same test, the case with no
//! context at all). The hive itself stays fail-closed: a read that reaches
//! the reader with no round is `missing_audience` (`a_read_without_a_round_
//! reads_nothing`).
//!
//! Red before GH #949: `templates/curator/reader/` does not exist and the
//! curator has no edge on `in_read`; the first assertion of every test of
//! layer 1 (`the reader ships`) fails, and the road ends at the generation.
//!
//! Guarded like every template-reading test (GH #49): a tree that does not
//! carry the curator template skips; the road skips without the member, the
//! assistant, its brains, the builder or the organism example.

#[path = "support/curator_hive.rs"]
mod curator_hive;

use curator_hive::{Hive, cell_config, obj, read_json, repo, run_python, said, user};
use meclaw_colony::config::{EdgeSpec, HiveParams};
use meclaw_colony::edge_table::{Edge, EdgeTable, apply_edges};
use meclaw_core::serde_json::{self as sj, Map, Value, json};
use meclaw_core::{Headers, Path, Uuid};
use meclaw_testing::{emit_all, shipped_script};
use std::collections::{BTreeMap, VecDeque};
use std::io::Write;
use std::process::{Command, Stdio};

// ═══════════════════════════════════════════════════════════════ the rounds

/// The round of every question, as a channel stamps it (unsorted, spaces).
const EA: &str = r#"["member:e", "member:a"]"#;
/// The rows' audiences, canonical as `./intake` writes them.
const CANON_EA: &str = r#"["member:a","member:e"]"#;
const CANON_EB: &str = r#"["member:b","member:e"]"#;
const CANON_EAB: &str = r#"["member:a","member:b","member:e"]"#;
/// The member's round on the road (review C-1): the generation `sam` is the
/// agent, the installing wish names the person `a` -- as the question edge
/// stamps it, and canonical as a row of it is written.
const MEMBER_ROUND: &str = r#"["agent:sam","member:a"]"#;
/// Another member's round with the same agent: what a forged `["agent:sam"]`
/// would read and the member's round does not.
const CANON_SAM_B: &str = r#"["agent:sam","member:b"]"#;
const STAR: &str = r#"["*"]"#;
const EB: &str = r#"["member:e", "member:b"]"#;
/// A round no row was written for except the universal one.
const EC: &str = r#"["member:e", "member:c"]"#;

/// One wall row of a fixture: (second, audience, session, turn, kind, element,
/// final). `None` is a row from before the audience rule (column NULL).
type Row = (
    i64,
    Option<&'static str>,
    &'static str,
    &'static str,
    &'static str,
    Value,
    i64,
);

/// The standard wall: two rows of {e,a}, one of {e,b}, one of {e,a,b} (covers
/// {e,a} and {e,b}), one for everybody, one from before the rule and one of a
/// round-less call (`[]`, PP-BD-12). Every text carries the token `rw949`.
fn standard_wall() -> Vec<Row> {
    vec![
        (
            1,
            Some(CANON_EA),
            "s1",
            "t1",
            "user",
            user("rw949 ea: the parcel left Lisbon"),
            0,
        ),
        (
            2,
            Some(CANON_EA),
            "s1",
            "t1",
            "assistant",
            said("rw949 ea reply: noted, Porto"),
            1,
        ),
        (
            3,
            Some(CANON_EB),
            "s2",
            "t2",
            "user",
            user("rw949 eb: the key lies under the stone"),
            0,
        ),
        (
            4,
            Some(CANON_EAB),
            "s1",
            "t3",
            "user",
            user("rw949 eab: all three of us"),
            0,
        ),
        (
            5,
            Some(STAR),
            "s3",
            "t4",
            "user",
            user("rw949 star: for everybody"),
            0,
        ),
        (
            6,
            None,
            "s1",
            "t5",
            "user",
            user("rw949 null: from before the rule"),
            0,
        ),
        (
            7,
            Some("[]"),
            "s1",
            "t6",
            "user",
            user("rw949 empty: a round-less call"),
            0,
        ),
    ]
}

/// What only rows the round {e,a} may not see say.
const HIDDEN_FROM_EA: [&str; 3] = ["rw949 eb:", "rw949 null:", "rw949 empty:"];

fn stamp(second: i64) -> chrono::DateTime<chrono::Utc> {
    use chrono::TimeZone;
    chrono::Utc
        .with_ymd_and_hms(2026, 10, 1, 8, 0, 0)
        .single()
        .expect("a fixed instant")
        + chrono::Duration::seconds(second)
}

fn sow(h: &mut Hive, rows: &[Row]) {
    for (second, aud, session, turn, kind, el, final_) in rows {
        h.row_under(*aud, stamp(*second), session, turn, kind, el, *final_);
    }
}

fn seq_of(second: i64) -> i64 {
    stamp(second).timestamp_micros()
}

// ═══════════════════════════════════════════════════════════════ the edges

fn shipped() -> bool {
    curator_hive::shipped()
}

/// Red before GH #949.
fn the_reader_ships() {
    assert!(
        repo("templates/curator/reader/config.json").is_file(),
        "the curator carries no `./reader`: an app has no way to read its round"
    );
}

fn hive_edges_of(p: &std::path::Path) -> Vec<EdgeSpec> {
    let params = read_json(p)["params"].clone();
    let hp: HiveParams =
        sj::from_value(params).unwrap_or_else(|e| panic!("{}: params: {e}", p.display()));
    hp.graph.edges
}

fn hive_edges(rel: &str) -> Vec<EdgeSpec> {
    hive_edges_of(&repo(rel))
}

fn specs(values: &[Value], label: &str) -> Vec<EdgeSpec> {
    values
        .iter()
        .map(|e| sj::from_value(e.clone()).unwrap_or_else(|err| panic!("{label}: edge {e}: {err}")))
        .collect()
}

fn abs(base: &str, endpoint: &str) -> String {
    match endpoint {
        "." => base.to_string(),
        other => format!("{base}/{}", other.trim_start_matches("./")),
    }
}

fn add_edges(table: &mut EdgeTable, base: &str, specs: &[EdgeSpec], label: &str) {
    for spec in specs {
        let condition = spec.condition.as_ref().map(|src| {
            meclaw_colony::cel_eval::parse_condition(src)
                .unwrap_or_else(|e| panic!("{label}: condition {src:?}: {e}"))
        });
        let modifier = spec.modifier.as_ref().map(|m| {
            meclaw_colony::cel_eval::parse_modifier(m)
                .unwrap_or_else(|(k, e)| panic!("{label}: modifier {k}: {e}"))
        });
        table.insert(Edge {
            id: Uuid::now_v7(),
            from: Path::new(&abs(base, &spec.from)),
            to: Path::new(&abs(base, &spec.to)),
            condition,
            modifier,
            is_default: spec.is_default,
            lane: spec.lane.clone(),
            tap: spec.tap,
        });
    }
}

// ═══════════════════════════════════════════════════════════════ the world

/// One emission of a cell: its hop and its body.
type Out = (Map<String, Value>, Map<String, Value>);

/// Where a message came to rest: the path, its headers, its body.
type Rest = (String, Headers, Map<String, Value>);

/// Edges, the ledgers of the curators on them, and the shipped reader.
struct World {
    table: EdgeTable,
    /// Curator path -> its ledger.
    ledgers: BTreeMap<String, Hive>,
    script: String,
    params: Map<String, Value>,
    /// Every emission of a reader, for the contract check.
    emitted: Vec<Out>,
    /// Every ledger op that ran, by curator path.
    ops: Vec<(String, Value)>,
    /// Every reader that ran, by path.
    ran: Vec<String>,
}

impl World {
    fn with_reader(
        table: EdgeTable,
        ledgers: BTreeMap<String, Hive>,
        over: &[(&str, Value)],
    ) -> Self {
        let cfg = cell_config("reader");
        let script = cfg["params"]["script_inline"]
            .as_str()
            .expect("the reader is a code cell with an inline script")
            .to_string();
        let mut params = obj(cfg["params"].clone());
        params.remove("script_inline");
        for (k, v) in over {
            assert!(params.contains_key(*k), "no such reader knob: {k}");
            params.insert((*k).to_string(), v.clone());
        }
        Self {
            table,
            ledgers,
            script,
            params,
            emitted: Vec::new(),
            ops: Vec::new(),
            ran: Vec::new(),
        }
    }

    /// The curator alone, at `/c`, its ledger sown with `rows`.
    fn hive(rows: &[Row], over: &[(&str, Value)]) -> Self {
        let mut t = EdgeTable::new();
        add_edges(
            &mut t,
            "/c",
            &hive_edges("templates/curator/config.json"),
            "curator",
        );
        let mut h = Hive::new();
        sow(&mut h, rows);
        Self::with_reader(t, BTreeMap::from([("/c".to_string(), h)]), over)
    }

    /// The reader's step on one message, as the code runner hands it over.
    fn run_reader(&mut self, at: &str, hs: &Headers, body: &Map<String, Value>) -> Vec<Out> {
        self.ran.push(at.to_string());
        let doc = json!({
            "envelope": {"header": {"context": hs.context, "hop": hs.hop},
                         "target": at, "reply_to": ""},
            "body": body,
            "params": self.params,
        });
        let out = run_python(&self.script, &doc.to_string());
        let err = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(out.status.success(), "the reader exited non-zero: {err}");
        let emitted: Value = sj::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("not JSON ({e}): {}", String::from_utf8_lossy(&out.stdout)));
        let list = match emitted {
            Value::Array(a) => a,
            other => vec![other],
        };
        let outs: Vec<Out> = list
            .into_iter()
            .map(|m| {
                let mut body = obj(m);
                let hop = body
                    .remove("header")
                    .and_then(|h| h.as_object().cloned())
                    .unwrap_or_default();
                (hop, body)
            })
            .collect();
        self.emitted.extend(outs.iter().cloned());
        outs
    }

    /// The ledger's answer in the store cell's own reply shape. A refused op
    /// travels on like any answer (`Hive::store` with `ledger_may_refuse`).
    fn ledger_answer(&mut self, at: &str, body: &Map<String, Value>) -> Out {
        use meclaw_cells::store::ops::dispatch;
        use meclaw_cells::store::output::{BundleLeg, build_bundle_result, build_tool_result};
        let curator = at.trim_end_matches("/ledger").to_string();
        let calls: Vec<Value> = body
            .get("messages")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|m| m["type"] == "tool_call")
            .collect();
        let args = |c: &Value| -> Value {
            sj::from_str(c["text"].as_str().unwrap_or("")).expect("op json")
        };
        for c in &calls {
            self.ops.push((curator.clone(), args(c)));
        }
        let db = &self
            .ledgers
            .get(&curator)
            .unwrap_or_else(|| panic!("no ledger at {curator}"))
            .db;
        let (out, hop) = if calls.len() == 1 {
            let c = &calls[0];
            match dispatch(db, &args(c)) {
                Ok(outcome) => {
                    build_tool_result(&outcome, c["id"].as_str().unwrap_or("").to_string(), 0)
                }
                Err(e) => (
                    json!({"messages": [{"origin": "tool", "type": "tool_result",
                                         "text": e, "id": c["id"]}]}),
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
                    let a = args(c);
                    match dispatch(db, &a) {
                        Ok(outcome) => BundleLeg::from_outcome(&outcome, id, 0),
                        Err(e) => BundleLeg::refusal(
                            a["operation"].as_str().unwrap_or("error"),
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
        (hop, obj(out))
    }

    /// Route one message from `from` until nothing takes it further, running
    /// the reader and the ledger where an edge delivers to them. Returns where
    /// every branch came to rest: a path no edge leaves on that message, or an
    /// app's cell it was delivered to.
    fn drive(&mut self, from: &str, headers: Headers, body: Value) -> Vec<Rest> {
        let mut rest = Vec::new();
        let mut queue = VecDeque::from([(from.to_string(), headers, obj(body))]);
        let mut steps = 0;
        while let Some((here, hs, body)) = queue.pop_front() {
            steps += 1;
            assert!(steps < 400, "the road does not come to rest");
            let out = apply_edges(&self.table, &Path::new(&here), &hs);
            if out.is_empty() {
                rest.push((here, hs, body));
                continue;
            }
            for d in out {
                let to = d.target.as_str().to_string();
                let hs = d.headers_out;
                // A cell of a curator is known by its hive, not by the word
                // `curator` in its path: the hive-alone world mounts it at `/c`,
                // and `ends_with("/curator/reader")` let `/c/reader` pass by as
                // a plain path (Fix-Runde 2 of GH #949: every read rested there).
                let (hive, cell) = to.rsplit_once('/').unwrap_or(("", ""));
                let of_a_curator = self.ledgers.contains_key(hive);
                let reader = of_a_curator && cell == "reader";
                let ledger = of_a_curator && cell == "ledger";
                if reader {
                    for (hop, b) in self.run_reader(&to, &hs, &body) {
                        queue.push_back((
                            to.clone(),
                            Headers::from_parts(hs.context.clone(), hop),
                            b,
                        ));
                    }
                } else if ledger {
                    let (hop, b) = self.ledger_answer(&to, &body);
                    queue.push_back((to.clone(), Headers::from_parts(hs.context.clone(), hop), b));
                } else if is_app_cell(&to) {
                    rest.push((to, hs, body.clone()));
                } else {
                    queue.push_back((to, hs, body.clone()));
                }
            }
        }
        rest
    }

    /// One question on the curator's hive path, in `round` (JSON null: no
    /// `audience_set` key at all). Returns the one answer that left the hive.
    fn ask(&mut self, round: Value, hop: Value, body: Value) -> Out {
        let mut ctx = Map::new();
        ctx.insert("session_id".into(), json!("s-app"));
        if !round.is_null() {
            ctx.insert("audience_set".into(), round);
        }
        let mut hop = obj(hop);
        hop.insert("route".into(), json!("in_read"));
        let rest = self.drive("/c", Headers::from_parts(ctx, hop), body);
        assert_eq!(
            rest.len(),
            1,
            "one question, one answer: {:?}",
            rest.iter().map(|r| (&r.0, &r.1.hop)).collect::<Vec<_>>()
        );
        let (path, hs, body) = rest.into_iter().next().expect("checked");
        assert_eq!(path, "/c", "the answer leaves through the hive path");
        assert_eq!(
            hs.hop.get("route"),
            Some(&json!("read")),
            "the answer travels on `read`: {:?}",
            hs.hop
        );
        for k in ["cur_origin", "cur_phase", "cur_call", "cur_reason"] {
            assert!(
                !hs.context.contains_key(k),
                "`{k}` left the hive: {:?}",
                hs.context
            );
        }
        (hs.hop, body)
    }
}

fn is_app_cell(p: &str) -> bool {
    p.contains("/apps/") && p.ends_with("/sink")
}

fn code(hop: &Map<String, Value>) -> &str {
    hop.get("error_code")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("every answer carries `hop.error_code`: {hop:?}"))
}

fn rows_of(body: &Map<String, Value>) -> Vec<Value> {
    body.get("rows")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("an answer carries `rows`: {body:?}"))
}

fn texts(body: &Map<String, Value>) -> Vec<String> {
    rows_of(body)
        .iter()
        .map(|r| r["text"].as_str().unwrap_or("").to_string())
        .collect()
}

fn next_of(body: &Map<String, Value>) -> String {
    body.get("next")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("an answer carries `next`: {body:?}"))
        .to_string()
}

fn turns() -> Value {
    json!({"messages": [], "what": "turns"})
}

fn tag(t: &str) -> Value {
    json!({"read_tag": t})
}

// ═══════════════════════════════════════════════════════ 1. the hive

/// No round, an empty text, the declared empty round, a text that is no JSON
/// array: each is answered `missing_audience` with no rows, the tag echoed,
/// and not one ledger op ran. A timer strike is the first case (module doc).
#[test]
fn a_read_without_a_round_reads_nothing() {
    if !shipped() {
        return;
    }
    the_reader_ships();
    let mut w = World::hive(&standard_wall(), &[]);
    for round in [Value::Null, json!(""), json!("[]"), json!("not a round")] {
        let (hop, body) = w.ask(round.clone(), tag("t-none"), turns());
        assert_eq!(code(&hop), "missing_audience", "{round}: {hop:?}");
        assert_eq!(hop.get("read_tag"), Some(&json!("t-none")), "{round}");
        assert!(rows_of(&body).is_empty(), "{round}: {body:?}");
        assert_eq!(next_of(&body), "", "{round}");
        let (hop, body) = w.ask(
            round.clone(),
            tag("t-none"),
            json!({"messages": [], "what": "sessions"}),
        );
        assert_eq!(code(&hop), "missing_audience", "{round}: {hop:?}");
        assert!(rows_of(&body).is_empty(), "{round}: {body:?}");
    }
    assert!(
        w.ops.is_empty(),
        "a read without a round touched the ledger: {:?}",
        w.ops
    );
}

/// Exactly the rows whose audience covers the round, in `seq` order, with
/// their session, turn, kind, origin, final flag and text -- and never a row of
/// another round, a row from before the rule or a row of a round-less call.
#[test]
fn a_round_reads_only_the_rows_that_cover_it() {
    if !shipped() {
        return;
    }
    the_reader_ships();
    let mut w = World::hive(&standard_wall(), &[]);

    let (hop, body) = w.ask(json!(EA), tag("t-ea"), turns());
    assert_eq!(code(&hop), "", "{hop:?}");
    assert_eq!(hop.get("read_tag"), Some(&json!("t-ea")));
    assert_eq!(hop.get("read_what"), Some(&json!("turns")));
    assert_eq!(
        texts(&body),
        [
            "rw949 ea: the parcel left Lisbon",
            "rw949 ea reply: noted, Porto",
            "rw949 eab: all three of us",
            "rw949 star: for everybody",
        ],
        "{body:?}"
    );
    let rows = rows_of(&body);
    assert_eq!(rows[0]["seq"], json!(seq_of(1)));
    assert_eq!(rows[0]["session_id"], "s1");
    assert_eq!(rows[0]["turn_id"], "t1");
    assert_eq!(rows[0]["kind"], "user");
    assert_eq!(rows[0]["origin"], "user");
    assert_eq!(rows[0]["final"], json!(0));
    assert_eq!(rows[1]["origin"], "assistant");
    assert_eq!(rows[1]["final"], json!(1));
    assert!(
        rows[0]["at"]
            .as_str()
            .is_some_and(|a| a.starts_with("2026-10-01T08:00:01"))
    );
    assert_eq!(next_of(&body), "", "the whole round fits one page");
    let all = sj::to_string(&body).expect("serialise");
    for hidden in HIDDEN_FROM_EA {
        assert!(!all.contains(hidden), "{hidden} reached {{e,a}}: {all}");
    }

    // The other round sees its own rows, the wide one and the universal one.
    let (hop, body) = w.ask(json!(EB), tag("t-eb"), turns());
    assert_eq!(code(&hop), "");
    assert_eq!(
        texts(&body),
        [
            "rw949 eb: the key lies under the stone",
            "rw949 eab: all three of us",
            "rw949 star: for everybody",
        ]
    );

    // A round nobody spoke in reads the universal row and nothing else -- an
    // answer, not a refusal.
    let (hop, body) = w.ask(json!(EC), tag("t-ec"), turns());
    assert_eq!(code(&hop), "");
    assert_eq!(texts(&body), ["rw949 star: for everybody"]);

    // The store filtered: every wall read of the reader carried `covers` of
    // the round it served, never a read without it.
    for (_, op) in &w.ops {
        if op["table"] == "wall" {
            assert!(
                op["where"]["audience_set"]["covers"].is_array(),
                "a wall read without the store's gate: {op}"
            );
        }
    }
}

/// Paging over the round: the same pages, rows and cursors on a wall with rows
/// of other rounds between and after the visible ones as on the wall without
/// them -- where a page ends hangs on the rows the round may see alone.
#[test]
fn pages_follow_the_cursor_over_the_rounds_rows_only() {
    if !shipped() {
        return;
    }
    the_reader_ships();
    let visible: Vec<Row> = (0..5)
        .map(|i| {
            let s = 10 * (i + 1);
            (
                s,
                Some(CANON_EA),
                "s1",
                "t1",
                "user",
                user(&format!("rw949 page row {i}")),
                0,
            )
        })
        .collect();
    let mut beside = visible.clone();
    for s in [5, 15, 25, 35, 45, 55, 60, 70] {
        beside.push((
            s,
            Some(CANON_EB),
            "s1",
            "t9",
            "user",
            user(&format!("rw949 eb {s}")),
            0,
        ));
    }
    beside.push((65, None, "s1", "t8", "user", user("rw949 null"), 0));

    let pages = |w: &mut World| -> Vec<(Vec<Value>, String)> {
        let mut out = Vec::new();
        let mut since = String::new();
        for _ in 0..10 {
            let (hop, body) = w.ask(
                json!(EA),
                tag("t-page"),
                json!({"messages": [], "what": "turns", "limit": 2, "since": since}),
            );
            assert_eq!(code(&hop), "", "{hop:?}");
            let next = next_of(&body);
            out.push((rows_of(&body), next.clone()));
            if next.is_empty() {
                return out;
            }
            since = next;
        }
        panic!("the pages never ended: {out:?}");
    };
    let alone = pages(&mut World::hive(&visible, &[]));
    let mixed = pages(&mut World::hive(&beside, &[]));
    assert_eq!(
        alone, mixed,
        "a page of the round moved beside rows it may not see"
    );
    let cursors: Vec<&str> = alone.iter().map(|(_, n)| n.as_str()).collect();
    assert_eq!(
        cursors,
        [
            seq_of(20).to_string().as_str(),
            seq_of(40).to_string().as_str(),
            ""
        ],
        "two rows a page, the cursor the last seq of the page, '' at the end"
    );
    assert_eq!(alone[2].0.len(), 1);
    assert!(
        !sj::to_string(&mixed)
            .expect("serialise")
            .contains("rw949 eb")
    );
    assert!(
        !sj::to_string(&mixed)
            .expect("serialise")
            .contains("rw949 null")
    );
}

/// A session narrows the turns; `sessions` lists the round's sessions with
/// their first and last row and their number of turns, newest first, and never
/// a session only another round spoke in.
#[test]
fn sessions_and_a_session_are_the_rounds_own() {
    if !shipped() {
        return;
    }
    the_reader_ships();
    let mut w = World::hive(&standard_wall(), &[]);

    let (hop, body) = w.ask(
        json!(EA),
        tag("t-s1"),
        json!({"messages": [], "what": "turns", "session_id": "s1"}),
    );
    assert_eq!(code(&hop), "");
    assert_eq!(
        texts(&body),
        [
            "rw949 ea: the parcel left Lisbon",
            "rw949 ea reply: noted, Porto",
            "rw949 eab: all three of us",
        ]
    );

    // A session of another round is a session this round has no rows in.
    let (hop, body) = w.ask(
        json!(EA),
        tag("t-s2"),
        json!({"messages": [], "what": "turns", "session_id": "s2"}),
    );
    assert_eq!(code(&hop), "");
    assert!(rows_of(&body).is_empty(), "{body:?}");

    let (hop, body) = w.ask(
        json!(EA),
        tag("t-sessions"),
        json!({"messages": [], "what": "sessions", "session_id": "s2"}),
    );
    assert_eq!(
        code(&hop),
        "",
        "`session_id` is not looked at for sessions: {hop:?}"
    );
    assert_eq!(hop.get("read_what"), Some(&json!("sessions")));
    assert_eq!(
        rows_of(&body),
        [
            json!({"session_id": "s3", "first_seq": seq_of(5), "last_seq": seq_of(5),
                   "first_at": rows_first_at(5), "last_at": rows_first_at(5), "turns": 1}),
            json!({"session_id": "s1", "first_seq": seq_of(1), "last_seq": seq_of(4),
                   "first_at": rows_first_at(1), "last_at": rows_first_at(4), "turns": 2}),
        ],
        "{body:?}"
    );
    assert_eq!(body.get("truncated"), Some(&json!(false)));
    assert_eq!(next_of(&body), "");

    let (_, body) = w.ask(
        json!(EA),
        tag("t-one"),
        json!({"messages": [], "what": "sessions", "limit": 1}),
    );
    assert_eq!(rows_of(&body).len(), 1);
    assert_eq!(rows_of(&body)[0]["session_id"], "s3");
    assert_eq!(
        body.get("truncated"),
        Some(&json!(true)),
        "more sessions than the answer holds is said"
    );

    // After a cursor only the sessions with later rows.
    let (_, body) = w.ask(
        json!(EA),
        tag("t-after"),
        json!({"messages": [], "what": "sessions", "since": seq_of(4).to_string()}),
    );
    assert_eq!(
        rows_of(&body)
            .iter()
            .map(|s| s["session_id"].clone())
            .collect::<Vec<_>>(),
        [json!("s3")]
    );
}

fn rows_first_at(second: i64) -> String {
    stamp(second).format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string()
}

/// A page holds `read_budget` characters of text: rows go whole while it
/// lasts, the first row always goes (cut and marked `cut` when it alone is
/// larger), so a reader always moves on and never loops on one row.
#[test]
fn a_page_holds_its_budget_and_always_moves_on() {
    if !shipped() {
        return;
    }
    the_reader_ships();
    let wall: Vec<Row> = [
        (1, "abcd"),
        (2, "efgh"),
        (3, "ijkl"),
        (4, "0123456789ABCDEF"),
        (5, "z"),
    ]
    .into_iter()
    .map(|(s, t)| (s, Some(CANON_EA), "s1", "t1", "user", user(t), 0))
    .collect();
    let mut w = World::hive(&wall, &[("read_budget", json!(10))]);
    let mut got = Vec::new();
    let mut since = String::new();
    for _ in 0..10 {
        let (hop, body) = w.ask(
            json!(EA),
            tag("t-budget"),
            json!({"messages": [], "what": "turns", "since": since}),
        );
        assert_eq!(code(&hop), "");
        got.push(
            rows_of(&body)
                .iter()
                .map(|r| {
                    (
                        r["text"].as_str().unwrap_or("").to_string(),
                        r.get("cut").cloned(),
                    )
                })
                .collect::<Vec<_>>(),
        );
        since = next_of(&body);
        if since.is_empty() {
            break;
        }
    }
    assert_eq!(
        got,
        vec![
            vec![("abcd".to_string(), None), ("efgh".to_string(), None)],
            vec![("ijkl".to_string(), None)],
            vec![("0123456789".to_string(), Some(json!(true)))],
            vec![("z".to_string(), None)],
        ]
    );
}

/// The question is checked before anything is read, in the order `read_tag`,
/// `what`, `session_id`, `since`, `limit`: the first key that fails is named
/// in `hop.detail` with `invalid_input`, no rows, the tag echoed (bounded).
#[test]
fn a_bad_question_is_named_and_reads_nothing() {
    if !shipped() {
        return;
    }
    the_reader_ships();
    let mut w = World::hive(&standard_wall(), &[]);
    let long = "x".repeat(65);
    let cases: Vec<(Value, Value, &str)> = vec![
        (json!({}), turns(), "read_tag"),
        (json!({"read_tag": ""}), turns(), "read_tag"),
        (json!({"read_tag": long}), turns(), "read_tag"),
        (json!({"read_tag": 7}), turns(), "read_tag"),
        (tag("t"), json!({"messages": []}), "what"),
        (tag("t"), json!({"what": "Turns"}), "what"),
        (
            tag("t"),
            json!({"what": "turns", "session_id": 3}),
            "session_id",
        ),
        (
            tag("t"),
            json!({"what": "turns", "session_id": "s".repeat(257)}),
            "session_id",
        ),
        (tag("t"), json!({"what": "turns", "since": -1}), "since"),
        (tag("t"), json!({"what": "turns", "since": true}), "since"),
        (tag("t"), json!({"what": "turns", "since": "1.5"}), "since"),
        (
            tag("t"),
            json!({"what": "turns", "since": "\u{0663}"}),
            "since",
        ),
        (
            tag("t"),
            json!({"what": "turns", "since": "99999999999999999999"}),
            "since",
        ),
        (tag("t"), json!({"what": "turns", "limit": 0}), "limit"),
        (tag("t"), json!({"what": "turns", "limit": 101}), "limit"),
        (tag("t"), json!({"what": "turns", "limit": 20.0}), "limit"),
        (tag("t"), json!({"what": "turns", "limit": "x"}), "limit"),
        (tag("t"), json!({"what": "sessions", "limit": 0}), "limit"),
    ];
    for (hop, body, key) in cases {
        let (h, b) = w.ask(json!(EA), hop.clone(), body.clone());
        assert_eq!(code(&h), "invalid_input", "{hop} {body}: {h:?}");
        assert_eq!(h.get("detail"), Some(&json!(key)), "{hop} {body}: {h:?}");
        assert!(rows_of(&b).is_empty(), "{hop} {body}");
        let echoed = h.get("read_tag").and_then(Value::as_str).unwrap_or("");
        assert!(echoed.chars().count() <= 64, "an unbounded echo: {echoed}");
        if let Some(t) = hop.get("read_tag").and_then(Value::as_str) {
            assert!(t.starts_with(echoed), "{t} echoed as {echoed}");
        }
    }
    assert!(
        w.ops.is_empty(),
        "a refused question touched the ledger: {:?}",
        w.ops
    );

    // The bounds themselves are accepted.
    for body in [
        json!({"what": "turns", "limit": 1}),
        json!({"what": "turns", "limit": "100"}),
        json!({"what": "turns", "since": "0", "session_id": ""}),
        json!({"what": "sessions", "session_id": 3}),
    ] {
        let (h, _) = w.ask(json!(EA), tag(&"t".repeat(64)), body.clone());
        assert_eq!(code(&h), "", "{body}: {h:?}");
        assert_eq!(h.get("read_tag"), Some(&json!("t".repeat(64))));
    }
}

/// "The read failed" never looks like "there is nothing": a round the store
/// refuses to express (more members than `covers` takes) is `store_error`
/// with the store's code, while a round with no rows is an empty answer.
#[test]
fn a_refused_read_is_never_an_empty_page() {
    if !shipped() {
        return;
    }
    the_reader_ships();
    let mut w = World::hive(&standard_wall(), &[]);
    let crowd: Vec<String> = (0..65).map(|i| format!("member:m{i}")).collect();
    let round = sj::to_string(&crowd).expect("serialise");
    for what in ["turns", "sessions"] {
        let (hop, body) = w.ask(
            json!(round),
            tag("t-crowd"),
            json!({"messages": [], "what": what}),
        );
        assert_eq!(code(&hop), "store_error", "{what}: {hop:?}");
        assert!(
            hop.get("detail")
                .and_then(Value::as_str)
                .is_some_and(|d| !d.is_empty()),
            "{what}: a store error names the store's code: {hop:?}"
        );
        assert!(rows_of(&body).is_empty());
        assert_eq!(hop.get("read_tag"), Some(&json!("t-crowd")));
    }
    // The round without rows has to be one: the standard wall's row for
    // everybody (`["*"]`) covers `member:z` and was rightly read for it
    // (Fix-Runde 2 of GH #949), so this ask runs on the wall without it.
    let unshared: Vec<Row> = standard_wall()
        .into_iter()
        .filter(|r| r.1 != Some(STAR))
        .collect();
    let mut w = World::hive(&unshared, &[]);
    let (hop, body) = w.ask(json!(r#"["member:z"]"#), tag("t-z"), turns());
    assert_eq!(
        code(&hop),
        "",
        "a round without rows is answered, not refused"
    );
    assert!(rows_of(&body).is_empty(), "{:?}", rows_of(&body));
}

/// Every emission of the reader across the questions of this file keeps the
/// cell's `contract.emits`: the declared routes, hop keys and body slots, of
/// the declared types.
#[test]
fn every_emission_keeps_the_cells_contract() {
    if !shipped() {
        return;
    }
    the_reader_ships();
    let mut w = World::hive(&standard_wall(), &[("read_budget", json!(12))]);
    let questions = [
        (json!(EA), tag("c1"), turns()),
        (json!(EA), tag("c2"), json!({"what": "turns", "limit": 1})),
        (json!(EA), tag("c3"), json!({"what": "sessions"})),
        (Value::Null, tag("c4"), turns()),
        (json!(EA), tag("c5"), json!({"what": "nope"})),
        (
            json!(
                r#"["a","b","c","d","e","f","g","h","i","j","k","l","m","n","o","p","q","r","s","t","u","v","w","x","y","z","a1","b1","c1","d1","e1","f1","g1","h1","i1","j1","k1","l1","m1","n1","o1","p1","q1","r1","s1","t1","u1","v1","w1","x1","y1","z1","a2","b2","c2","d2","e2","f2","g2","h2","i2","j2","k2","l2","m2"]"#
            ),
            tag("c6"),
            turns(),
        ),
    ];
    for (round, hop, body) in questions {
        w.ask(round, hop, body);
    }
    assert!(w.emitted.len() >= 6, "{:?}", w.emitted.len());
    let contract = cell_config("reader")["contract"]["emits"].clone();
    let hop_spec = contract["hop"].as_object().expect("emits.hop").clone();
    let body_spec = contract["body"].as_object().expect("emits.body").clone();
    let routes: Vec<Value> = hop_spec["route"]["values"]
        .as_array()
        .expect("route values")
        .clone();
    let typed = |v: &Value, t: &str| match t {
        "string" => v.is_string(),
        "array" => v.is_array(),
        "boolean" => v.is_boolean(),
        "number" => v.is_number(),
        "object" => v.is_object(),
        _ => false,
    };
    for (hop, body) in &w.emitted {
        assert!(
            routes.contains(&hop["route"]),
            "an undeclared route: {hop:?}"
        );
        for (k, v) in hop {
            let spec = hop_spec
                .get(k)
                .unwrap_or_else(|| panic!("hop key `{k}` is not in contract.emits.hop: {hop:?}"));
            assert!(
                typed(v, spec["type"].as_str().unwrap_or("")),
                "hop `{k}` = {v}"
            );
        }
        for (k, spec) in &hop_spec {
            if spec["required"] == json!(true) {
                assert!(hop.contains_key(k), "required hop `{k}` missing: {hop:?}");
            }
        }
        for (k, v) in body {
            let spec = body_spec.get(k).unwrap_or_else(|| {
                panic!("body slot `{k}` is not in contract.emits.body: {body:?}")
            });
            assert!(
                typed(v, spec["type"].as_str().unwrap_or("")),
                "body `{k}` = {v}"
            );
        }
        for (k, spec) in &body_spec {
            if spec["required"] == json!(true) {
                assert!(
                    body.contains_key(k),
                    "required body `{k}` missing: {body:?}"
                );
            }
        }
    }
}

/// The audience gate is one rule in this hive (`curator_history.rs`,
/// `the_audience_gate_is_one_rule_in_every_cell`): the reader carries the
/// gate functions and `ROUNDLESS` as the same text as `./history`.
#[test]
fn the_reader_carries_the_hives_one_gate() {
    if !shipped() {
        return;
    }
    the_reader_ships();
    const SEGMENTS: &str = r#"
import ast, json, sys
NAMES = ("audience_of", "allowed", "passes", "audience_meet", "round_where", "ROUNDLESS")
out = {}
for cell, src in json.load(sys.stdin).items():
    seg = {}
    for n in ast.parse(src).body:
        if isinstance(n, ast.FunctionDef) and n.name in NAMES:
            seg[n.name] = ast.get_source_segment(src, n)
        if isinstance(n, ast.Assign):
            for t in n.targets:
                if isinstance(t, ast.Name) and t.id in NAMES:
                    seg[t.id] = ast.get_source_segment(src, n)
    out[cell] = seg
print(json.dumps(out))
"#;
    let scripts = json!({
        "history": curator_hive::script_of("history"),
        "reader": curator_hive::script_of("reader"),
    });
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(SEGMENTS)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(scripts.to_string().as_bytes())
        .expect("write the scripts");
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let seg: Value = sj::from_slice(&out.stdout).expect("one JSON document");
    assert_eq!(
        seg["history"].as_object().map(|o| o.len()),
        Some(6),
        "the gate of ./history is the reference: {seg}"
    );
    assert_eq!(
        seg["reader"], seg["history"],
        "the reader's gate drifted from the hive's"
    );
}

// ═══════════════════════════════════════════════════════ 2. the road

const MEMBER: &str = "/m";
const BOX: &str = "/m/assistants";
const GEN: &str = "/m/assistants/sam";
const GENERATION: &str = "sam";
const BRAINS: [&str; 3] = ["talky", "talky-chat", "cogny"];
const PROBE_APP: &str = "probe-app";
const OTHER_APP: &str = "other-app";

fn road_shipped() -> bool {
    shipped()
        && [
            "templates/member/config.json",
            "templates/assistant/config.json",
            "templates/talky/config.json",
            "templates/cogny/config.json",
            "templates/builder/recipes/config.json",
            "examples/organism/grow-assistant.json",
        ]
        .iter()
        .all(|rel| repo(rel).is_file())
}

/// What `install_app` draws for an app declaring `reads: "./sink"`,
/// member-relative: the SHIPPED renderer over stdin.
fn install_edges(app: &str) -> Vec<Value> {
    let out = emit_all(
        &shipped_script(&repo("templates/builder/recipes/config.json").to_string_lossy()),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!({"recipe": "install_app", "request": "…",
                                         "params": {"scope": "/os/orgs/acme/members/alex",
                                                    "app": app,
                                                    "template": format!("{app}@1.0.0"),
                                                    "screen": "display",
                                                    "generation": GENERATION,
                                                    "ctx": {"member_person": "a"},
                                                    "declaration": {"reads": "./sink"}}})
                                  .to_string()}],
        }),
    );
    let first = out.first().expect("the renderer emits");
    assert!(
        first["header"]["error_code"].is_null(),
        "{app}: install_app refused `reads`: {first}"
    );
    first["manifest"][0]["diff"]["add_edges"]
        .as_array()
        .unwrap_or_else(|| panic!("{app}: no add_edges in {first}"))
        .clone()
}

/// The container as `examples/organism` grows it, its generation renamed.
fn container_edges() -> Vec<Value> {
    let grown = read_json(&repo("examples/organism/grow-assistant.json"));
    let raw = sj::to_string(&grown["diff"]["add_edges"]).expect("serialise");
    let renamed = raw
        .replace("./scribe", &format!("./{GENERATION}"))
        .replace("'scribe'", &format!("'{GENERATION}'"))
        .replace("/assistants/scribe/", &format!("/assistants/{GENERATION}/"));
    assert!(
        !renamed.contains("scribe"),
        "the example's generation was not renamed whole"
    );
    sj::from_str(&renamed).expect("reparse")
}

/// The member, its container, the generation, each brain with its curator
/// and that curator's ledger, and the edges of two reading apps. Each ledger
/// is sown with the standard wall plus one row that names its brain, so an
/// answer says which ledger it was read from.
fn road() -> World {
    let mut t = EdgeTable::new();
    add_edges(
        &mut t,
        MEMBER,
        &hive_edges("templates/member/config.json"),
        "member",
    );
    add_edges(
        &mut t,
        BOX,
        &specs(&container_edges(), "container"),
        "container",
    );
    add_edges(
        &mut t,
        GEN,
        &hive_edges("templates/assistant/config.json"),
        "assistant",
    );
    let mut ledgers = BTreeMap::new();
    for brain in BRAINS {
        let at = format!("{GEN}/{brain}");
        let composite = if brain == "cogny" {
            "templates/cogny/config.json"
        } else {
            // `talky-chat` is a ref to `talky`: the same edges.
            "templates/talky/config.json"
        };
        add_edges(&mut t, &at, &hive_edges(composite), brain);
        let curator = format!("{at}/curator");
        add_edges(
            &mut t,
            &curator,
            &hive_edges("templates/curator/config.json"),
            "curator",
        );
        let mut h = Hive::new();
        let mut wall = standard_wall();
        let own = format!("rw949 {brain}: only this brain's wall");
        wall.push((8, Some(MEMBER_ROUND), "s1", "t7", "user", user(&own), 0));
        wall.push((
            9,
            Some(CANON_SAM_B),
            "s2",
            "t8",
            "user",
            user("rw949 sam-b: another member of the agent"),
            0,
        ));
        sow(&mut h, &wall);
        ledgers.insert(curator, h);
    }
    for app in [PROBE_APP, OTHER_APP] {
        add_edges(&mut t, MEMBER, &specs(&install_edges(app), app), app);
    }
    World::with_reader(t, ledgers, &[])
}

/// A question as the app's cell emits it: the context of the message it
/// reacts to (here a turn of `round`, or none), its own hop.
fn question(round: Option<&str>, organ: Option<&str>) -> Headers {
    let mut ctx = obj(json!({"session_id": "s-916", "channel": "chat:949"}));
    if let Some(r) = round {
        ctx.insert("audience_set".into(), json!(r));
    }
    let mut hop = obj(json!({"route": "read", "read_tag": "app-q1"}));
    if let Some(o) = organ {
        hop.insert("organ".into(), json!(o));
    }
    Headers::from_parts(ctx, hop)
}

fn cell_of(app: &str) -> String {
    format!("{MEMBER}/apps/{app}/sink")
}

/// The question reaches the curator of exactly the brain it names, that
/// reader reads that brain's ledger in the app's round, and the answer comes
/// back to the asking cell once -- without the asker's stamp, with the round
/// untouched -- and never to the other app.
#[test]
fn an_apps_read_reaches_the_named_brain_and_comes_back_to_it_alone() {
    if !road_shipped() {
        return;
    }
    the_reader_ships();
    for brain in BRAINS {
        let mut w = road();
        let rest = w.drive(
            &cell_of(PROBE_APP),
            question(Some(EA), Some(brain)),
            turns(),
        );
        let at_probe: Vec<&Rest> = rest.iter().filter(|r| r.0 == cell_of(PROBE_APP)).collect();
        assert_eq!(
            rest.len(),
            1,
            "{brain}: one question, one answer, one place: {:?}",
            rest.iter().map(|r| (&r.0, &r.1.hop)).collect::<Vec<_>>()
        );
        assert_eq!(
            at_probe.len(),
            1,
            "{brain}: the answer reaches the asking cell"
        );
        let (_, hs, body) = at_probe[0];
        assert_eq!(hs.hop.get("route"), Some(&json!("read")));
        assert_eq!(hs.hop.get("read_tag"), Some(&json!("app-q1")));
        assert_eq!(code(&hs.hop), "");
        assert!(
            !hs.context.contains_key("read_caller"),
            "the stamp ends at the asker"
        );
        assert_eq!(
            hs.context.get("audience_set"),
            Some(&json!(MEMBER_ROUND)),
            "the round is the member's, as the question edge stamped it"
        );
        let got = texts(body);
        assert!(
            got.contains(&format!("rw949 {brain}: only this brain's wall")),
            "{brain}: read off another ledger: {got:?}"
        );
        for other in BRAINS.iter().filter(|b| **b != brain) {
            assert!(
                !got.iter()
                    .any(|t| t.starts_with(&format!("rw949 {other}:"))),
                "{brain}: {other}'s wall reached the answer: {got:?}"
            );
        }
        for hidden in HIDDEN_FROM_EA {
            assert!(
                !got.iter().any(|t| t.starts_with(hidden)),
                "{brain}: {got:?}"
            );
        }
        assert_eq!(
            w.ran
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            1,
            "{brain}: exactly one reader ran: {:?}",
            w.ran
        );
        assert_eq!(w.ran[0], format!("{GEN}/{brain}/curator/reader"));
        assert!(!rest.iter().any(|r| r.0 == cell_of(OTHER_APP)));
    }
}

/// The other side of the stamp: a question of the other app is answered to
/// the other app and never to the probe.
#[test]
fn each_reader_hears_only_its_own_answer() {
    if !road_shipped() {
        return;
    }
    the_reader_ships();
    let mut w = road();
    let rest = w.drive(
        &cell_of(OTHER_APP),
        question(Some(EA), Some("talky")),
        turns(),
    );
    assert_eq!(
        rest.len(),
        1,
        "{:?}",
        rest.iter().map(|r| &r.0).collect::<Vec<_>>()
    );
    assert_eq!(rest[0].0, cell_of(OTHER_APP));
}

/// Through the road the round is the MEMBER's (review C-1): whatever round
/// the app's message claims -- its member's own, another person's, the
/// universal one, the agent alone, the person alone, none, or no context at
/// all (an app woken by its own timer) -- the question edge stamps
/// `["agent:sam","member:a"]` over it, and the answer holds exactly the rows
/// that round may see: the member's own row and the one for everybody. Never
/// another member's round of the same agent, never a row of a round without
/// the agent.
///
/// Red before the fix: the claimed round reached the reader -- `["agent:sam"]`
/// read `sam-b`, `["member:e", "member:a"]` read the `ea` rows, and no round
/// was `missing_audience`.
#[test]
fn a_forged_round_reads_only_the_members_round() {
    if !road_shipped() {
        return;
    }
    the_reader_ships();
    let want = vec![
        "rw949 star: for everybody".to_string(),
        "rw949 talky: only this brain's wall".to_string(),
    ];
    let mut claims: Vec<Headers> = [
        Some(MEMBER_ROUND),
        Some(EA),
        Some(EC),
        Some(STAR),
        Some(r#"["agent:sam"]"#),
        Some(r#"["member:a"]"#),
        None,
    ]
    .into_iter()
    .map(|r| question(r, Some("talky")))
    .collect();
    claims.push(Headers::from_parts(
        Map::new(),
        obj(json!({"route": "read", "read_tag": "night", "organ": "talky"})),
    ));
    for claim in claims {
        let label = format!("{:?}", claim.context.get("audience_set"));
        let mut w = road();
        let rest = w.drive(&cell_of(PROBE_APP), claim, turns());
        assert_eq!(rest.len(), 1, "{label}");
        assert_eq!(rest[0].0, cell_of(PROBE_APP), "{label}");
        assert_eq!(code(&rest[0].1.hop), "", "{label}: the read is answered");
        assert_eq!(
            rest[0].1.context.get("audience_set"),
            Some(&json!(MEMBER_ROUND)),
            "{label}"
        );
        let mut got = texts(&rest[0].2);
        got.sort();
        assert_eq!(got, want, "{label}: the member's round and nothing else");
    }
}

/// A question that names no brain, or one the generation does not hold, takes
/// no edge at the generation: no brain is guessed, no reader runs, nothing is
/// read, and nothing reaches an app.
#[test]
fn a_read_without_a_brain_goes_nowhere() {
    if !road_shipped() {
        return;
    }
    the_reader_ships();
    for organ in [None, Some("brain"), Some("tools"), Some("")] {
        let mut w = road();
        let rest = w.drive(&cell_of(PROBE_APP), question(Some(EA), organ), turns());
        assert_eq!(
            rest.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
            [GEN],
            "{organ:?}: the question ends at the generation's door"
        );
        assert_eq!(rest[0].1.hop.get("route"), Some(&json!("in_read")));
        assert!(
            w.ran.is_empty() && w.ops.is_empty(),
            "{organ:?}: {:?}",
            w.ran
        );
    }
}

/// An answer is never a question: a `read` that carries `hop.error_code` --
/// every answer does -- emitted again by an app's cell takes no edge at all,
/// so a cell that echoes what it got cannot send it round.
#[test]
fn an_answer_never_asks_again() {
    if !road_shipped() {
        return;
    }
    the_reader_ships();
    let mut w = road();
    for err in ["", "missing_audience"] {
        let mut hs = question(Some(EA), Some("talky"));
        hs.hop.insert("error_code".into(), json!(err));
        let rest = w.drive(&cell_of(PROBE_APP), hs, turns());
        assert_eq!(
            rest.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
            [cell_of(PROBE_APP).as_str()],
            "an echoed answer went somewhere"
        );
    }
    assert!(w.ran.is_empty());
}

/// The way back is a legal v-lane by the colony's own rule table
/// (`v_lane_verdict`, ADR-0020): the answer DOCKS at each brain's rim because
/// the generation declares `read` with those three connect points, and the
/// app's cell is a connect point its own hive names. Without the generation's
/// `at` the mutation door refuses the edge (`v_lane_no_connect_point`) -- the
/// control below takes it away and sees that refusal.
#[test]
fn the_way_back_is_a_v_lane_the_colony_admits() {
    use meclaw_colony::mutation::hive_contract::{HiveContract, Lane, contract_from_cell_dir};
    use meclaw_colony::mutation::port_boundary::v_lane_verdict;
    if !road_shipped() {
        return;
    }
    the_reader_ships();
    let generation = contract_from_cell_dir(&repo("templates/assistant"), GEN)
        .expect("the assistant level carries a contract");
    // The app's own declaration, as a template that declares `reads` writes
    // it: the question leaves its cell, the answer docks there.
    let lane = |route: &str| Lane {
        route: route.to_string(),
        context: Vec::new(),
        at: vec!["./sink".to_string()],
        required: false,
        because: "the app reads its round off a brain's wall".to_string(),
    };
    let app = HiveContract {
        hive_path: format!("{MEMBER}/apps/{PROBE_APP}"),
        accepts: vec![lane("read")],
        emits: vec![lane("read")],
    };
    let sink = cell_of(PROBE_APP);
    for brain in BRAINS {
        let rim = format!("{GEN}/{brain}");
        for (end, other) in [(rim.as_str(), sink.as_str()), (sink.as_str(), rim.as_str())] {
            let verdict =
                v_lane_verdict("read", end, other, &[], &[generation.clone(), app.clone()]);
            assert!(
                verdict.violations.is_empty(),
                "{brain}: the way back is refused at {end}: {:?}",
                verdict.violations
            );
        }
    }

    // The control: the generation without the connect points.
    let mut bare = generation.clone();
    for l in bare.emits.iter_mut().filter(|l| l.route == "read") {
        l.at.clear();
    }
    let verdict = v_lane_verdict("read", &format!("{GEN}/talky"), &sink, &[], &[bare, app]);
    assert!(
        verdict
            .violations
            .iter()
            .any(|(e, _)| format!("{e:?}").contains("VLaneNoConnectPoint")),
        "without `at` the generation owes a connect point: {:?}",
        verdict.violations
    );
}

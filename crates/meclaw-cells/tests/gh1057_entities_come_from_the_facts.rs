//! GH #1057 — `entities` and `entity_edges` have a producer again.
//!
//! Since the extractor was removed (#298) nothing wrote either table: measured
//! on a 6-month synthetic history, 877 facts, 26 subject aliases, 0 entities,
//! 0 edges — and the graph leg of recall (#520) walked an empty graph, so 10 of
//! 20 two-hop questions ("in which city does the flatmate of my grandson live
//! now?") were answered without the second person's current value.
//!
//! Owner ruling (2026-10-07, option A): a deterministic producer without a
//! model call. After every fact write it runs over the new facts; the nightly
//! run consolidates the delta since the last run (merge on new alias rows,
//! evidence of closed facts taken away, mentions of entities born since), and
//! the same window twice leaves the same state. Pinned here:
//!
//! 1. the producer turns a mini history into entities and edges, each edge with
//!    the claim as relation, the count as weight and the fact ids as evidence,
//!    channel and audience inherited;
//! 2. resolution only through `subject_aliases`: two unlinked look-alike
//!    spellings stay two entities, an alias row merges them at night;
//! 3. idempotence: the night over the state it wrote writes nothing, and a
//!    replayed producer batch adds nothing;
//! 4. zero model cost: every message the cell emits goes to the store (or, for
//!    the night, back to itself over the counted `entity` round edge);
//! 5. recall's graph leg ranks the node the question's parts meet in first and
//!    brings its current value -- the connecting fact -- into the leg;
//! 6. the review of 2026-10-07 (one case per finding): no duplicate edge per
//!    tick, unique ids and graph indexes in the store, a retraction is no
//!    evidence, the producer never reopens what the night closed, ticks plus
//!    the night end where the night alone ends, the longest spelling wins and a
//!    proper name needs its capital, a tick reads neither a hub nor edges by
//!    endpoint, the night reads its delta in bundles of at most 500 ops within
//!    the hop budget, a path needs every edge visible, a path episode needs the
//!    question, the cap is a full node page, and the anchor the question names
//!    starts the walk.
//!
//! Everything runs the shipped `params.script_inline` against real stdin
//! documents, the multi-step cases against a store held in the test (`SIM`).
//! No colony, no store, no provider, nothing spent.

use std::io::Write;
use std::process::{Command, Stdio};

use meclaw_core::serde_json::{Value, json};

const ENTITY: &str = "../../templates/memory-hive/entity-glue/config.json";
const RECALL: &str = "../../templates/memory-hive/recall/config.json";
const EXTRACT: &str = "../../templates/memory-hive/extract-glue/config.json";
const DREAM: &str = "../../templates/memory-hive/dream-glue/config.json";
const HIVE: &str = "../../templates/memory-hive/config.json";
const AUD: &str = r#"["member:m"]"#;

fn config(path: &str) -> Value {
    let raw = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    meclaw_core::serde_json::from_str(&raw).expect("config json")
}

fn script_of(path: &str) -> String {
    meclaw_testing::resolve_script_vars(
        config(path)["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
    )
}

/// Run `script` on `doc` (stdin, GH #279); `epilogue` runs after it in the
/// same globals when the script exits, and its stdout is the result.
fn python(script: &str, doc: &Value, epilogue: &str) -> Value {
    let src = format!(
        concat!(
            "import sys, io, json\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "_epi = {}\n",
            "if _epi:\n",
            "    _out = io.StringIO(); _real = sys.stdout; sys.stdout = _out\n",
            "try:\n",
            "    exec(compile(_script, 'cell', 'exec'), globals())\n",
            "except SystemExit:\n",
            "    pass\n",
            "if _epi:\n",
            "    sys.stdout = _real\n",
            "    exec(_epi, globals())\n"
        ),
        meclaw_core::serde_json::to_string(script).unwrap(),
        meclaw_core::serde_json::to_string(&meclaw_testing::code_stdin(doc).to_string()).unwrap(),
        meclaw_core::serde_json::to_string(epilogue).unwrap(),
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
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "python exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    meclaw_core::serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not json ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

fn entity_glue(doc: &Value) -> Vec<Value> {
    match python(&script_of(ENTITY), doc, "") {
        Value::Array(a) => a,
        other => vec![other],
    }
}

/// Every write op of the bundles the cell emitted, plus the routes it used.
fn writes(out: &[Value]) -> (Vec<Value>, Vec<String>) {
    let mut ops = Vec::new();
    let mut routes = Vec::new();
    for m in out {
        routes.push(m["header"]["route"].as_str().unwrap_or("").to_string());
        for t in m["messages"].as_array().into_iter().flatten() {
            let v: Value = meclaw_core::serde_json::from_str(t["text"].as_str().unwrap_or("null"))
                .unwrap_or(Value::Null);
            ops.push(v);
        }
    }
    (ops, routes)
}

/// A store the cell can be driven against end to end, in one python process:
/// tables of rows WITHOUT a primary key (an insert of a present id is booked
/// as a duplicate, which is what the real store does without an index), the
/// where forms the cell uses, `search` as a phrase match over the claim and the
/// subject. `pump` hands every bundle the cell emits to it and the reply back
/// until the cell parks, counting the hops since the last budget restore (a
/// door or the counted `entity` round edge) the way the colony's TTL does.
const SIM: &str = r##"
import io, json, random, re, sys

class Store:
    def __init__(self, tables=None):
        self.t = {k: [dict(r) for r in v] for k, v in (tables or {}).items()}
        self.dup_inserts, self.reads, self.bundles, self.max_hops, self.steps = 0, [], [], 0, 0
    def rows(self, table):
        return self.t.setdefault(table, [])
    @staticmethod
    def match(row, where):
        for col, cond in (where or {}).items():
            v = row.get(col)
            if not isinstance(cond, dict):
                if v != cond:
                    return False
                continue
            for o, a in cond.items():
                if (o == "in" and v not in a) or (o == "eq" and v != a) or (o == "neq" and v == a) \
                        or (o == "gt" and not (v is not None and str(v) > str(a))) \
                        or (o == "is_null" and (v is None) != a) \
                        or (o == "or_null" and v is not None and not Store.match(row, {col: a})):
                    return False
        return True
    def apply(self, a):
        op, table = a.get("operation"), a.get("table")
        if op in ("select", "search"):
            rs = [r for r in self.rows(table) if self.match(r, a.get("where"))]
            if op == "search":
                ph = [" ".join(re.findall(r"\w+", p.lower())) for p in re.findall(r'"([^"]*)"', a["match"])]
                def hit(r):
                    t = " %s " % " ".join(re.findall(r"\w+", ("%s | %s" % (r.get("claim"), r.get("canonical_subject"))).lower()))
                    return any(p and " %s " % p in t for p in ph)
                rs = [r for r in rs if hit(r)]
            for o in reversed(a.get("order_by") or []):
                rs.sort(key=lambda r: str(r.get(o["col"]) or ""), reverse=o.get("dir") == "desc")
            rs = rs[:a["limit"]] if a.get("limit") else rs
            self.reads.append((table, a.get("where"), len(rs)))
            return [{c: r.get(c) for c in a["columns"]} if a.get("columns") else dict(r) for r in rs]
        if op == "insert":
            if any(r.get("id") == a["row"].get("id") for r in self.rows(table)):
                self.dup_inserts += 1
            self.rows(table).append(dict(a["row"]))
            return {"rows_affected": 1}
        if op == "update":
            n = 0
            for r in self.rows(table):
                if self.match(r, a.get("where")):
                    r.update(a["set"])
                    n += 1
            return {"rows_affected": n}
        raise ValueError(op)
    def live(self, table):
        return sorted(json.dumps(r, sort_keys=True) for r in self.rows(table) if not r.get("valid_until"))
    def writes(self):
        return [o for b in self.bundles for o in b if o["operation"] in ("insert", "update")]

# What the cell said on stderr -- the colony log's journal lines.
JOURNAL = []

def patched(**consts):
    """The shipped cell with some of its constants set otherwise (a small
    write bundle, a short night), compiled as `CODE` is."""
    src = SRC
    for name, value in consts.items():
        line = re.search(r"^%s = .*$" % name, src, re.M).group(0)
        src = src.replace(line, "%s = %r" % (name, value), 1)
    return compile(src, "cell", "exec")

def run_cell(ctx, hop, messages):
    doc = {"envelope": {"header": {"context": ctx, "hop": hop}}, "body": {"messages": messages}, "params": {}}
    real_in, real_out, real_err = sys.stdin, sys.stdout, sys.stderr
    sys.stdin, sys.stdout, sys.stderr = io.StringIO(json.dumps(doc)), io.StringIO(), io.StringIO()
    try:
        exec(CODE, {"__name__": "cell"})
    except SystemExit:
        pass
    finally:
        out = sys.stdout.getvalue()
        JOURNAL.extend(l for l in sys.stderr.getvalue().splitlines() if l)
        sys.stdin, sys.stdout, sys.stderr = real_in, real_out, real_err
    v = json.loads(out)
    return v if isinstance(v, list) else [v]

def pump(store, phase, body, clock=""):
    queue = [({"mem_phase": phase, "ent_clock": clock}, {},
              [{"origin": "assistant", "type": "text", "text": json.dumps(body)}], 0)]
    runs = 0
    while queue:
        runs += 1
        assert runs < 5000, "the cell never parks"
        ctx, hop, msgs, used = queue.pop(0)
        store.max_hops = max(store.max_hops, used)
        for m in run_cell(ctx, hop, msgs):
            h = m["header"]
            if h["route"] == "entity":
                assert int(h["ent_step"]) < 1024, h
                store.steps += 1
                queue.append(({"mem_phase": h["phase"], "ent_clock": h.get("ent_clock", "")}, {},
                              m["messages"], 0))
                continue
            assert h["route"] == "gstore", h
            calls = [(c["id"], json.loads(c["text"])) for c in m["messages"]]
            store.bundles.append([a for _, a in calls])
            res = [(cid, store.apply(a)) for cid, a in calls]
            ctx2 = {"mem_phase": h["phase"], "ent_clock": h.get("ent_clock", ""), "store_origin": "entity"}
            if len(calls) >= 2:
                queue.append((ctx2, {"operation": "bundle"}, [{"origin": "tool", "type": "tool_result",
                              "id": cid, "text": json.dumps(r)} for cid, r in res], used + 2))
            else:
                queue.append((ctx2, {"operation": calls[0][1]["operation"]}, [{"origin": "tool",
                              "type": "tool_result", "text": json.dumps(res[0][1])}], used + 2))

def night(store, at, since=""):
    store.rows("consolidation_log").append({"run_id": "run-" + at, "delta_from": since,
                                            "delta_to": at, "status": "done"})
    pump(store, "entity-night", {"night": at}, clock=at)

def tick(store, facts):
    for f in facts:
        store.rows("facts").append(dict(f))
    pump(store, "entity-new", {"fact_ids": [f["id"] for f in facts]})

AUD = '["member:m"]'
def fact(i, ep, subj, claim, day, src="", cs=None):
    return {"id": i, "episode_id": ep, "subject": subj, "canonical_subject": subj.lower(),
            "claim": claim, "source": src, "channel": "tg:private", "audience_set": AUD,
            "valid_from": "2025-%sT09:00:00Z" % day, "recorded_at": "2025-%sT09:00:01Z" % day,
            "closure_source": cs, "expired_at": None}

def base():
    return [fact("m1", "e1", "peer m", "The member met Anna Alt and Bert Berg.", "01-01", src="m"),
            fact("m2", "e2", "peer m", "The member saw Anna Alt and Bert Berg again.", "01-02", src="m"),
            fact("a1", "e3", "Anna Alt", "Anna Alt likes soup.", "01-03"),
            fact("a2", "e4", "Anna Alt", "Anna Alt has a cat.", "01-04"),
            fact("b1", "e5", "Bert Berg", "Bert Berg plays chess.", "01-05"),
            fact("b2", "e6", "Bert Berg", "Bert Berg moved house.", "01-06")]

def edges(store, a, b):
    return [r for r in store.rows("entity_edges") if r["src_entity"] == a and r["dst_entity"] == b]

def crowd(n_ent=60, per=16, member=900, seed=3):
    rng = random.Random(seed)
    syl = ["ka", "lo", "mi", "ra", "te", "su", "no", "vi", "da", "pe", "gu", "ze"]
    names = set()
    while len(names) < n_ent:
        names.add(" ".join("".join(rng.choice(syl) for _ in range(3)).capitalize() for _ in range(2)))
    names, out = sorted(names), []
    def add(subj, claim, src):
        i = len(out) + 1
        out.append(dict(fact("c%05d" % i, "ce%04d" % (i // 3), subj, claim, "01-01", src=src),
                        recorded_at="2025-01-01T09:00:00.%06dZ" % i))
    for _ in range(member):
        add("peer m", "The member met %s." % rng.choice(names), "m")
    for n in names:
        for _ in range(per):
            a, b = rng.sample(names, 2)
            add(n, "%s met %s and %s." % (n, a, b) if rng.random() < 0.5 else "%s likes soup." % n, "")
    return out, names
"##;

/// Run `scenario` (python, it prints one JSON value) after [`SIM`], with the
/// shipped entity-glue compiled as `CODE` (its source as `SRC`).
fn sim(scenario: &str) -> Value {
    let program = format!(
        "{SIM}\nSRC = {}\nCODE = compile(SRC, 'cell', 'exec')\n{scenario}",
        meclaw_core::serde_json::to_string(&script_of(ENTITY)).unwrap()
    );
    python(
        &program,
        &json!({"header": {"context": {}, "hop": {}}, "messages": []}),
        "",
    )
}

fn rows_of(ops: &[Value], table: &str) -> Vec<Value> {
    ops.iter()
        .filter(|o| o["table"] == table && o["operation"] == "insert")
        .map(|o| o["row"].clone())
        .collect()
}

#[test]
fn the_producer_writes_entities_and_edges_with_their_evidence() {
    let v = sim(r#"
h = [fact("f1", "e1", "Greta Grimmbach", "The member described Greta Grimmbach as their grandson.", "01-11"),
     fact("f2", "e2", "Greta Grimmbach", "Greta Grimmbach has a dog called Socke.", "02-01"),
     fact("f3", "e3", "Frieda Quastberg", "Frieda Quastberg lives in Bremen.", "02-02"),
     fact("f6", "e6", "frieda quastberg", "frieda quastberg plays chess.", "02-03"),
     fact("f7", "e7", "Frieda Quastberg", "Frieda Quastberg sings.", "02-03"),
     fact("f4", "e4", "Frieda Quastberg and Greta Grimmbach",
          "Frieda Quastberg and Greta Grimmbach are each other's flatmates.", "02-04"),
     fact("f5", "e5", "Frieda Quastberg", "Frieda Quastberg moved from Bremen to Salzburg.", "05-22")]
s = Store({"facts": h[:5]})
tick(s, h[5:])
print(json.dumps({"ops": s.writes()}))
"#);
    let ops: Vec<Value> = v["ops"].as_array().unwrap().clone();
    let ents = rows_of(&ops, "entities");
    let keys: Vec<&str> = ents
        .iter()
        .map(|e| e["subject_key"].as_str().unwrap())
        .collect();
    assert_eq!(
        keys,
        vec!["frieda quastberg", "greta grimmbach"],
        "two candidates, the compound subject is none"
    );
    assert_eq!(
        ents[0]["canonical_name"], "Frieda Quastberg",
        "the most frequent of two spellings"
    );
    let edges = rows_of(&ops, "entity_edges");
    let g2f = edges
        .iter()
        .find(|e| e["src_entity"] == "greta grimmbach" && e["dst_entity"] == "frieda quastberg")
        .expect("an edge greta -> frieda");
    assert_eq!(g2f["evidence"], json!(r#"["f4"]"#));
    assert_eq!(g2f["weight"], 1);
    assert_eq!(
        g2f["relation"],
        "Frieda Quastberg and Greta Grimmbach are each other's flatmates."
    );
    assert_eq!(g2f["episode_id"], "e4");
    assert_eq!(g2f["channel"], "tg:private");
    assert_eq!(g2f["audience_set"], AUD);
    assert!(
        edges
            .iter()
            .any(|e| e["src_entity"] == "frieda quastberg" && e["dst_entity"] == "greta grimmbach"),
        "one row per direction"
    );
}

#[test]
fn the_ingress_hands_the_new_fact_ids_and_the_producer_only_reads() {
    let s = script_of(EXTRACT);
    assert!(
        s.contains(r#""route": "entity", "phase": "entity-new""#),
        "the ingress calls the producer"
    );
    let out = entity_glue(&json!({
        "header": {"context": {"mem_phase": "entity-new"}, "hop": {}},
        "messages": [{"origin": "assistant", "type": "text", "text": r#"{"fact_ids": ["f4", "f5"]}"#}]
    }));
    let (ops, routes) = writes(&out);
    assert_eq!(routes, vec!["gstore"]);
    assert!(ops.iter().all(|o| o["operation"] == "select"), "{ops:?}");
    assert_eq!(ops[0]["where"]["id"]["in"], json!(["f4", "f5"]));
    let hive = config(HIVE).to_string();
    for edge in [
        "./entity-glue",
        "'entity-night'",
        "'entity-new'",
        "store_origin == 'entity'",
        "int(hop.ent_step) < 1024",
    ] {
        assert!(hive.contains(edge), "the hive wires {edge}");
    }
    assert!(
        script_of(DREAM).contains(r#""route": "entity", "phase": "entity-night""#),
        "the night consolidates"
    );
}

#[test]
fn two_unlinked_spellings_stay_two_until_an_alias_links_them() {
    let v = sim(r#"
f = [fact("a1", "e1", "Herr Brandt", "Herr Brandt repairs clocks.", "01-01"),
     fact("a2", "e2", "Herr Brandt", "Herr Brandt lives upstairs.", "01-02"),
     fact("b1", "e3", "Hans Brandt", "Hans Brandt plays chess.", "01-03"),
     fact("b2", "e4", "Hans Brandt", "Hans Brandt has a cat.", "01-04")]
s = Store({"facts": f})
night(s, "2025-01-05T00:00:00Z")
ents = [r["subject_key"] for r in s.rows("entities")]
# An alias row now links them; the store has not re-canonicalised yet.
s.rows("subject_aliases").append({"alias": "herr brandt", "canonical": "hans brandt"})
s.bundles.clear()
night(s, "2026-10-07T03:00:00Z", "2025-01-05T00:00:00Z")
print(json.dumps({"ents": ents, "updates": [o for o in s.writes() if o["table"] == "entities"]}))
"#);
    assert_eq!(
        v["ents"].as_array().unwrap().len(),
        2,
        "no similarity merge: {v}"
    );
    let closed = v["updates"].as_array().unwrap();
    assert_eq!(
        closed.len(),
        2,
        "the alias' entity is closed and the survivor re-written: {v}"
    );
    let merged = closed
        .iter()
        .find(|o| o["set"]["merged_into"] == "hans brandt")
        .expect("merged");
    assert_eq!(
        merged["set"]["valid_until"], "2026-10-07T03:00:00Z",
        "the window end of the night"
    );
}

#[test]
fn the_same_window_twice_is_the_same_state() {
    let v = sim(r#"
s = Store({"facts": base()})
night(s, "2025-01-10T00:00:00Z")
first = len(s.writes())
s.bundles.clear()
night(s, "2025-01-10T00:00:00Z", "2025-01-09T00:00:00Z")
again = s.writes()
s.bundles.clear()
s.rows("facts").pop()
tick(s, [base()[-1]])
print(json.dumps({"first": first, "again": again, "replay": s.writes()}))
"#);
    assert!(v["first"].as_u64().unwrap() > 0);
    assert_eq!(
        v["again"],
        json!([]),
        "the night over its own state writes nothing"
    );
    assert_eq!(
        v["replay"],
        json!([]),
        "a replayed producer batch adds nothing"
    );
}

// ── review 2026-10-07 (OR-K2-2), one case per finding ─────────────────────

/// Finding 1 (Critical): the first producer derived edges from EVERY fact of
/// a touched subject but read stored edges only by its keys, so an edge
/// between two untouched entities that an old member fact names was inserted
/// again on every tick (the store has no primary key). Measured: 2 duplicate
/// inserts per tick of "The member went swimming.".
#[test]
fn a_tick_never_inserts_an_edge_twice() {
    let v = sim(r#"
s = Store({"facts": base()})
night(s, "2025-01-10T00:00:00Z")
for t in range(3):
    tick(s, [fact("s%d" % t, "e1%d" % t, "peer m", "The member went swimming.", "02-0%d" % (t + 1), src="m")])
ids = [r["id"] for r in s.rows("entity_edges")]
print(json.dumps({"dup_inserts": s.dup_inserts, "rows": len(ids), "distinct": len(set(ids))}))
"#);
    assert_eq!(v["dup_inserts"], 0, "{v}");
    assert_eq!(v["rows"], v["distinct"], "{v}");
}

/// Finding 1/6: the deterministic ids are unique in the store too, and the
/// reads by key and by endpoint have an index.
#[test]
fn the_store_declares_unique_ids_and_the_graph_indexes() {
    let idx = &config("../../templates/memory-hive/store/config.json")["params"]["indexes"];
    for (name, table, on, unique) in [
        ("entities_id", "entities", "id", true),
        ("entity_edges_id", "entity_edges", "id", true),
        ("entities_subject_key", "entities", "subject_key", false),
        ("entity_edges_src", "entity_edges", "src_entity", false),
        ("entity_edges_dst", "entity_edges", "dst_entity", false),
    ] {
        assert_eq!(idx[name]["table"], table, "{name}: {idx}");
        assert_eq!(idx[name]["on"], json!([on]), "{name}");
        assert_eq!(
            idx[name]["unique"].as_bool().unwrap_or(false),
            unique,
            "{name}"
        );
    }
}

/// Finding 2: close-glue writes a retraction as `close:<session>:retract`;
/// the first cut looked for a `retract` prefix, so a retracted fact stayed
/// evidence and made its edge.
#[test]
fn a_retracted_fact_is_no_evidence() {
    let v = sim(r#"
f = base() + [fact("r1", "e7", "Anna Alt", "Anna Alt and Cora Cohn are sisters.", "01-07", cs="close:s9:retract"),
              fact("c1", "e8", "Cora Cohn", "Cora Cohn sings.", "01-08"),
              fact("c2", "e9", "Cora Cohn", "Cora Cohn paints.", "01-09")]
s = Store({"facts": f})
night(s, "2025-01-10T00:00:00Z")
print(json.dumps({"edge": [r for r in edges(s, "anna alt", "cora cohn") if not r.get("valid_until")],
                  "cited": any("r1" in str(r.get("evidence")) for r in s.rows("entity_edges"))}))
"#);
    assert_eq!(v["edge"], json!([]), "{v}");
    assert_eq!(v["cited"], false);
}

/// Finding 3: the producer united its evidence with a row the night had
/// closed, reopening it with the forgotten fact ids (`["b2","b5","b6"]`).
#[test]
fn the_producer_never_reopens_what_the_night_closed() {
    let v = sim(r#"
f = [fact("b2", "e1", "Greta Grimm", "Greta Grimm and Frieda Quast share a flat.", "01-01"),
     fact("b5", "e2", "Greta Grimm", "Greta Grimm and Frieda Quast cook together.", "01-02"),
     fact("g3", "e3", "Frieda Quast", "Frieda Quast lives in Bremen.", "01-03"),
     fact("g4", "e4", "Frieda Quast", "Frieda Quast moved to Salzburg.", "01-04"),
     fact("g5", "e6", "Greta Grimm", "Greta Grimm sings.", "01-06"),
     fact("g6", "e7", "Greta Grimm", "Greta Grimm paints.", "01-07")]
s = Store({"facts": f})
night(s, "2025-01-10T00:00:00Z")
for r in s.rows("facts"):
    if r["id"] in ("b2", "b5"):
        r.update(closure_source="forget_request:t1", expired_at="2025-01-11T00:00:00Z")
night(s, "2025-01-12T00:00:00Z", "2025-01-10T00:00:00Z")
tick(s, [fact("b6", "e8", "Frieda Quast", "Frieda Quast phoned Greta Grimm.", "01-13")])
after_tick = [[r.get("valid_until"), r.get("evidence")] for r in edges(s, "greta grimm", "frieda quast")]
night(s, "2025-01-14T00:00:00Z", "2025-01-12T00:00:00Z")
after_night = [[r.get("valid_until"), r.get("evidence")] for r in edges(s, "greta grimm", "frieda quast")]
print(json.dumps({"after_tick": after_tick, "after_night": after_night}))
"#);
    assert_eq!(
        v["after_tick"],
        json!([["2025-01-12T00:00:00Z", "[]"]]),
        "closed by the night, untouched by the tick: {v}"
    );
    assert_eq!(
        v["after_night"],
        json!([[null, r#"["b6"]"#]]),
        "the night opens it with the new evidence only: {v}"
    );
}

/// Finding 3 (ruling): a day of producer ticks plus the night ends in the
/// rows the night alone ends in -- the first cut inserted duplicates along
/// the way, which no night removes.
#[test]
fn producer_ticks_and_the_night_end_where_the_night_alone_ends() {
    let v = sim(r#"
later = [fact("s1", "e20", "peer m", "The member went swimming.", "02-01", src="m"),
         fact("d1", "e21", "Dora Dahl", "Dora Dahl met Anna Alt.", "02-02"),
         fact("d2", "e22", "Dora Dahl", "Dora Dahl and Bert Berg play tennis.", "02-03"),
         fact("a3", "e23", "Anna Alt", "Anna Alt called Dora Dahl.", "02-04", src="m")]
out = {}
for name in ("ticks", "alone"):
    s = Store({"facts": base()})
    night(s, "2025-01-10T00:00:00Z")
    for f in later:
        if name == "ticks":
            tick(s, [f])
        else:
            s.rows("facts").append(dict(f))
    night(s, "2025-03-01T00:00:00Z", "2025-01-10T00:00:00Z")
    out[name] = {"entities": s.live("entities"), "edges": s.live("entity_edges")}
print(json.dumps({"same": out["ticks"] == out["alone"], "edges": len(out["alone"]["edges"])}))
"#);
    assert_eq!(v["same"], true, "{v}");
    assert_eq!(v["edges"], 12, "member, Anna, Bert and Dora, pairwise: {v}");
}

/// Finding 4: a spelling matched as a word anywhere in the lower-cased claim,
/// so "Mahler" got an edge from every "Moritz Mahler" fact and the neighbour
/// Rose one from "bought a rose". The longest spelling wins, a proper name is
/// matched with its capital (coordinator: alias spellings only, no similarity).
#[test]
fn a_name_inside_a_longer_name_and_a_lower_case_noun_are_no_mention() {
    let v = sim(r#"
f = [fact("y1", "e1", "Mahler", "Mahler repairs bikes.", "01-01"),
     fact("y2", "e2", "Mahler", "Mahler lives upstairs.", "01-02"),
     fact("k1", "e3", "Moritz Mahler", "Moritz Mahler plays the cello.", "01-03"),
     fact("k2", "e4", "Moritz Mahler", "Moritz Mahler bakes bread.", "01-04"),
     fact("r1", "e5", "Rose", "Rose sings in a choir.", "01-05"),
     fact("r2", "e6", "Rose", "Rose has a garden.", "01-06"),
     fact("x1", "e7", "Anna Alt", "Anna Alt called Moritz Mahler.", "01-07"),
     fact("x2", "e8", "Anna Alt", "Anna Alt bought a rose.", "01-08")]
s = Store({"facts": f})
night(s, "2025-01-10T00:00:00Z")
print(json.dumps(sorted({r["dst_entity"] for r in s.rows("entity_edges") if r["src_entity"] == "anna alt"})))
"#);
    assert_eq!(v, json!(["moritz mahler"]));
}

/// Finding 4, second cut: a subject that joins names ("and", a comma, "&")
/// is no name and masks none of them; a subject that is one name and no
/// entity yet ("Moritz Mahler") still hides the shorter entity inside it.
#[test]
fn a_compound_subject_masks_no_name_but_a_longer_name_does() {
    let v = sim(r#"
f = [fact("y1", "e1", "Mahler", "Mahler repairs bikes.", "01-01"),
     fact("y2", "e2", "Mahler", "Mahler lives upstairs.", "01-02"),
     fact("g1", "e3", "Greta Grimmbach", "Greta Grimmbach has a dog.", "01-03"),
     fact("g2", "e4", "Greta Grimmbach", "Greta Grimmbach sings.", "01-04"),
     fact("q1", "e5", "Frieda Quastberg", "Frieda Quastberg plays chess.", "01-05"),
     fact("q2", "e6", "Frieda Quastberg", "Frieda Quastberg lives in Bremen.", "01-06"),
     fact("a1", "e7", "Anna Alt", "Anna Alt paints.", "01-07"),
     fact("a2", "e8", "Anna Alt", "Anna Alt called Moritz Mahler.", "01-08"),
     fact("k1", "e9", "Moritz Mahler", "Moritz Mahler plays the cello.", "01-09"),
     fact("c1", "e10", "Frieda Quastberg and Greta Grimmbach",
          "Frieda Quastberg and Greta Grimmbach are each other's flatmates.", "01-10"),
     fact("c2", "e11", "Mahler, Frieda Quastberg", "Mahler, Frieda Quastberg went fishing.", "01-11"),
     fact("c3", "e12", "Greta Grimmbach & Mahler", "Greta Grimmbach & Mahler share a car.", "01-12")]
s = Store({"facts": f})
night(s, "2025-01-20T00:00:00Z")
print(json.dumps(sorted({r["src_entity"] + " > " + r["dst_entity"] for r in s.rows("entity_edges")})))
"#);
    assert_eq!(
        v,
        json!([
            "frieda quastberg > greta grimmbach",
            "frieda quastberg > mahler",
            "greta grimmbach > frieda quastberg",
            "greta grimmbach > mahler",
            "mahler > frieda quastberg",
            "mahler > greta grimmbach"
        ]),
        "every joined subject links its names; anna alt never reaches mahler"
    );
}

/// Coordinator ruling OR-K2-4: a subject that joins names is never an entity
/// itself, even with two facts from two episodes -- as one it covered both
/// names as the longest spelling. Its facts link the names the SUBJECT
/// carries, found only by entity keys and `subject_aliases` spellings as whole
/// words: "Frieda" is an alias, "Gretta" is no spelling of anyone, and
/// "Mahlerhof" is not "Mahler".
#[test]
fn a_compound_subject_is_no_entity_and_its_facts_link_its_names() {
    let v = sim(r#"
f = [fact("y1", "e1", "Mahler", "Mahler repairs bikes.", "01-01"),
     fact("y2", "e2", "Mahler", "Mahler lives upstairs.", "01-02"),
     fact("g1", "e3", "Greta Grimmbach", "Greta Grimmbach has a dog.", "01-03"),
     fact("g2", "e4", "Greta Grimmbach", "Greta Grimmbach sings.", "01-04"),
     fact("q1", "e5", "Frieda Quastberg", "Frieda Quastberg plays chess.", "01-05"),
     fact("q2", "e6", "Frieda Quastberg", "Frieda Quastberg lives in Bremen.", "01-06"),
     fact("c1", "e10", "Frieda and Greta Grimmbach", "They share a flat.", "01-10"),
     fact("c2", "e11", "Frieda and Greta Grimmbach", "They went to the lake.", "01-11"),
     fact("c3", "e12", "Gretta or Mahlerhof", "They argue about the car.", "01-12"),
     fact("c4", "e13", "Gretta or Mahlerhof", "They made up again.", "01-13")]
al = [{"alias": "frieda", "canonical": "frieda quastberg"}]
night_store = Store({"facts": f, "subject_aliases": al})
night(night_store, "2025-01-20T00:00:00Z")
tick_store = Store({"facts": f[:6], "subject_aliases": al})
night(tick_store, "2025-01-07T00:00:00Z")
tick(tick_store, f[6:8])
tick(tick_store, f[8:])
def graph(s):
    return {"entities": sorted(r["subject_key"] for r in s.rows("entities")),
            "edges": sorted("%s > %s %s" % (r["src_entity"], r["dst_entity"], r["evidence"])
                            for r in s.rows("entity_edges") if not r.get("valid_until"))}
print(json.dumps({"night": graph(night_store), "ticks": graph(tick_store)}))
"#);
    let want = json!({
        "entities": ["frieda quastberg", "greta grimmbach", "mahler"],
        "edges": [
            "frieda quastberg > greta grimmbach [\"c1\", \"c2\"]",
            "greta grimmbach > frieda quastberg [\"c1\", \"c2\"]"
        ]
    });
    assert_eq!(v["night"], want, "the night: {v}");
    assert_eq!(v["ticks"], want, "the producer: {v}");
}

/// Coordinator ruling OR-K2-5: a backfill that runs again over a graph whose
/// hub is already settled (an interrupted backfill leaves an entity
/// `pending`, and the next night resumes it from its record) kept writing every
/// edge as `co` and the hub step only re-kinds edges into an entity whose OWN
/// kind changes -- measured on a copy of the K2M snapshot: 95 `co-hub` edges
/// flipped to `co`, and the walk's hub gate stood open. The edge kind comes
/// from the kind the whole graph gave the entity; a second backfill writes no
/// edge.
#[test]
fn a_second_backfill_keeps_every_hub_edge_and_writes_no_edge() {
    let v = sim(r#"
facts, names = crowd(n_ent=20, per=4, member=80)
s = Store({"facts": facts})
night(s, "2025-01-02T00:00:00Z")
def hub_edges():
    return sorted(r["id"] for r in s.rows("entity_edges")
                  if r.get("edge_kind") == "co-hub" and not r.get("valid_until"))
before = hub_edges()
for r in s.rows("entities"):
    if r["subject_key"] == names[0].lower():
        r["kind"] = "pending"
# The night's record as an interrupted backfill leaves it: back at the start.
for r in s.rows("scratch"):
    r["payload"] = json.dumps({"m": "b1", "c": "", "f": "2025-01-02T00:00:00Z"})
s.bundles.clear()
night(s, "2025-01-03T00:00:00Z", "2025-01-02T00:00:00Z")
w = s.writes()
print(json.dumps({"hubs": sorted(r["subject_key"] for r in s.rows("entities") if r["kind"] == "hub"),
                  "before": len(before), "kept": hub_edges() == before,
                  "edge_writes": [o for o in w if o["table"] == "entity_edges"][:3],
                  "entity_writes": [o.get("set") for o in w if o["table"] == "entities"]}))
"#);
    assert!(
        v["hubs"].as_array().unwrap().contains(&json!("peer m")),
        "the member is a hub: {v}"
    );
    assert!(
        v["before"].as_u64().unwrap() > 12,
        "hub edges worth the name: {v}"
    );
    assert_eq!(v["kept"], json!(true), "no hub edge flips: {v}");
    assert_eq!(v["edge_writes"], json!([]), "{v}");
    assert_eq!(
        v["entity_writes"],
        json!([{"kind": "subject"}]),
        "only the pending entity is settled: {v}"
    );
}

/// Review K2-R2 N1 (Critical): the night ran the backfill only on a graph
/// without entities, and on a grown hive the first producer tick before the
/// first night made some -- every night after was a delta, the old facts were
/// never derived (one tick and two nights: 2 entities and 2 edges where the
/// backfill over the same facts makes 7 and 14). The night now decides by its
/// own record, written when a backfill finished.
#[test]
fn a_tick_before_the_first_night_leaves_the_backfill_to_run() {
    let v = sim(r#"
def graph(s):
    return {"entities": s.live("entities"), "edges": s.live("entity_edges")}
grown = base() + [fact("c1", "e7", "Anna Alt", "Anna Alt met Bert Berg at the lake.", "01-07")]
s = Store({"facts": grown, "consolidation_log": [{"run_id": "r0", "delta_from": "",
           "delta_to": "2025-01-20T00:00:00Z", "status": "done"}]})
tick(s, [fact("t1", "e9", "Anna Alt", "Anna Alt baked bread.", "02-01")])
after_tick = len(s.live("entities"))
night(s, "2025-02-10T00:00:00Z", "2025-01-20T00:00:00Z")
night(s, "2025-02-11T00:00:00Z", "2025-02-10T00:00:00Z")
alone = Store({"facts": [dict(f) for f in s.rows("facts")]})
night(alone, "2025-02-11T00:00:00Z")
print(json.dumps({"after_tick": after_tick, "same": graph(s) == graph(alone),
                  "entities": len(s.live("entities")), "edges": len(s.live("entity_edges")),
                  "record": [json.loads(r["payload"])["m"] for r in s.rows("scratch")],
                  "created_at": [r["created_at"] for r in s.rows("scratch")]}))
"#);
    assert_eq!(v["after_tick"], 1, "the tick made Anna an entity: {v}");
    assert_eq!(v["same"], true, "tick + nights = the backfill alone: {v}");
    assert_eq!(v["entities"], 3, "{v}");
    assert_eq!(v["edges"], 6, "{v}");
    assert_eq!(v["record"], json!(["done"]), "{v}");
    assert_eq!(
        v["created_at"],
        json!([null]),
        "the scratch sweep deletes created_at < cutoff; the record has none: {v}"
    );
}

/// Review K2-R2 N2: a delta night of more than one write bundle wrote the
/// entity rows (merged, gone, reopened) first and derived the next round from
/// them -- with 5 ops a bundle 18 of 20 edges of a merged entity stayed open
/// and 1 of 20 re-hung edges was written. Edges first, closures after them,
/// entity rows last.
#[test]
fn a_merge_larger_than_one_write_bundle_closes_every_old_edge() {
    let v = sim(r#"
names = ["Paul Ast%d" % i for i in range(10)]
f = []
for i, n in enumerate(names):
    f += [fact("p%da" % i, "q%da" % i, n, "%s plays chess." % n, "01-%02d" % (i + 1)),
          fact("p%db" % i, "r%db" % i, n, "%s likes tea." % n, "02-%02d" % (i + 1))]
f += [fact("k%d" % i, "k%d" % i, "Anni Kurz", "Anni Kurz met %s." % n, "03-%02d" % (i + 1))
      for i, n in enumerate(names)]
f += [fact("b1", "b1", "Anna Berg", "Anna Berg lives in Bonn.", "04-01"),
      fact("b2", "b2", "Anna Berg", "Anna Berg sings.", "04-02")]
out = {}
for w in (5, 500):
    CODE = patched(WRITE_OPS=w)
    s = Store({"facts": f})
    night(s, "2025-05-01T00:00:00Z")
    s.rows("subject_aliases").append({"alias": "anni kurz", "canonical": "anna berg"})
    night(s, "2025-05-02T00:00:00Z", "2025-05-01T00:00:00Z")
    out[w] = {"open_at_old": sum(1 for r in s.rows("entity_edges") if not r.get("valid_until")
                                 and "anni kurz" in (r["src_entity"], r["dst_entity"])),
              "rehung": sum(1 for r in s.rows("entity_edges") if not r.get("valid_until")
                            and "anna berg" in (r["src_entity"], r["dst_entity"])),
              "old": [(r["valid_until"] is not None, r["merged_into"]) for r in s.rows("entities")
                      if r["subject_key"] == "anni kurz"],
              "rows": [s.live("entities"), s.live("entity_edges")]}
print(json.dumps({"small": {k: v for k, v in out[5].items() if k != "rows"},
                  "same": out[5]["rows"] == out[500]["rows"]}))
"#);
    assert_eq!(
        v["small"],
        json!({"open_at_old": 0, "rehung": 20, "old": [[true, "anna berg"]]}),
        "{v}"
    );
    assert_eq!(v["same"], true, "5 ops a bundle end where 500 end: {v}");
}

/// Review K2-R2 N3: every page costs a step, and the round edge passes
/// fewer than 1024 -- a longer backfill ended at the edge without a word,
/// started over the next night and died at the same place, `pending`
/// forever. The night parks where it stands, says so, and resumes.
#[test]
fn a_backfill_longer_than_one_night_resumes_where_it_stopped() {
    let v = sim(r#"
names = ["Paul Ast%d" % i for i in range(6)]
f = []
for i, n in enumerate(names):
    f += [fact("a%da" % i, "e%da" % i, n, "%s plays chess." % n, "01-%02d" % (i + 1)),
          fact("a%db" % i, "e%db" % i, n, "%s likes tea." % n, "02-%02d" % (i + 1))]
k = 0
for i, n in enumerate(names):
    for j, m in enumerate(names):
        if i != j:
            f.append(fact("m%03d" % k, "em%d" % k, n, "%s met %s." % (n, m), "03-%02d" % (k % 28 + 1)))
            k += 1
CODE = patched(BACKFILL_FACTS=2, STEP_LIMIT=21)
s = Store({"facts": f})
nights = []
for d in range(1, 5):
    night(s, "2025-04-%02dT00:00:00Z" % d, "2025-04-%02dT00:00:00Z" % (d - 1) if d > 1 else "")
    nights.append({"pending": sum(1 for r in s.rows("entities") if r["kind"] == "pending"),
                   "record": json.loads(s.rows("scratch")[0]["payload"])["m"]})
CODE = compile(SRC, "cell", "exec")
alone = Store({"facts": [dict(x) for x in f]})
night(alone, "2025-04-04T00:00:00Z")
print(json.dumps({"nights": nights, "journal": [l for l in JOURNAL if "paused" in l][:1],
                  "same": [s.live("entities"), s.live("entity_edges")]
                          == [alone.live("entities"), alone.live("entity_edges")],
                  "edges": len(alone.live("entity_edges"))}))
"#);
    let nights = v["nights"].as_array().unwrap();
    assert!(
        nights[0]["pending"].as_u64().unwrap() > 0 && nights[0]["record"] != "done",
        "the first night does not finish: {v}"
    );
    assert_eq!(nights[3], json!({"pending": 0, "record": "done"}), "{v}");
    assert_eq!(v["journal"].as_array().unwrap().len(), 1, "it says so: {v}");
    assert_eq!(v["same"], true, "{v}");
    assert_eq!(v["edges"], 30, "{v}");
}

/// Review K2-R2 N4: the masks were the subjects in hand. A tick before the
/// first "Moritz Mahler" fact wrote Anna-Mahler, the backfill did not, and
/// no night repaired it once the window was past. Masks now come from the
/// store by key, and a night re-derives the older facts that spell a subject
/// first stored in its window.
#[test]
fn masks_come_from_the_store_so_every_path_writes_the_same_edges() {
    let v = sim(r#"
f = [fact("a1", "e1", "Anna Berg", "Anna Berg lives in Bonn.", "01-01"),
     fact("a2", "e2", "Anna Berg", "Anna Berg sings.", "01-02"),
     fact("b1", "e3", "Mahler", "Mahler repairs bikes.", "01-03"),
     fact("b2", "e4", "Mahler", "Mahler lives upstairs.", "01-04"),
     fact("c2", "e5", "Anna Berg", "Anna Berg called Mahler.", "01-05"),
     fact("c1", "e6", "Anna Berg", "Anna Berg met Moritz Mahler.", "01-06"),
     fact("z1", "e7", "Moritz Mahler", "Moritz Mahler plays the cello.", "01-07")]
def edges(s):
    return sorted("%s > %s %s" % (r["src_entity"], r["dst_entity"], r["evidence"])
                  for r in s.rows("entity_edges") if not r.get("valid_until"))
out = {}
for page in (2, 200):
    CODE = patched(BACKFILL_FACTS=page)
    s = Store({"facts": [dict(x) for x in f]})
    night(s, "2025-02-01T00:00:00Z")
    out["backfill/%d" % page] = edges(s)
CODE = compile(SRC, "cell", "exec")
s = Store({})
for x in f[:5] + [f[6], f[5]]:
    tick(s, [x])
out["ticks, the name first"] = edges(s)
s = Store({})
for x in f[:6]:
    tick(s, [x])
night(s, "2025-01-06T12:00:00Z")
tick(s, [f[6]])
night(s, "2025-02-01T00:00:00Z", "2025-01-06T12:00:00Z")
out["ticks, a night between"] = edges(s)
print(json.dumps(out))
"#);
    let want = json!(["anna berg > mahler [\"c2\"]", "mahler > anna berg [\"c2\"]"]);
    for (path, got) in v.as_object().unwrap() {
        assert_eq!(got, &want, "{path}: {v}");
    }
}

/// Review K2-R2 N5: a tick read every entity and every alias on each of its
/// hops -- 300 entities and 300 aliases made 1 203 rows for one fact. It now
/// reads the rows its facts can name, an alias chain included.
#[test]
fn a_tick_reads_only_the_rows_its_facts_can_name() {
    let v = sim(r#"
f = []
for i in range(120):
    n = "Paul Ast%03d" % i
    f += [fact("p%03da" % i, "q%03da" % i, n, "%s plays chess." % n, "01-01"),
          fact("p%03db" % i, "r%03db" % i, n, "%s likes tea." % n, "01-02")]
al = [{"alias": "pa%03d" % i, "canonical": "paul ast%03d" % i} for i in range(120)]
al += [{"alias": "zed", "canonical": "zedd"}, {"alias": "zedd", "canonical": "paul ast002"}]
s = Store({"facts": f, "subject_aliases": al})
night(s, "2025-01-05T00:00:00Z")
s.reads.clear()
tick(s, [fact("n1", "en1", "Paul Ast000", "Paul Ast000 met Zed at the lake.", "01-06")])
rows = {}
for (t, w, n) in s.reads:
    rows[t] = rows.get(t, 0) + n
print(json.dumps({"rows": rows, "edges": sorted("%s > %s" % (r["src_entity"], r["dst_entity"])
                                                for r in s.rows("entity_edges"))}))
"#);
    // 120 entities and 242 aliases: 240 and 244 rows before, on every hop.
    assert!(
        v["rows"]["entities"].as_u64().unwrap() <= 12,
        "entities by key: {v}"
    );
    assert!(
        v["rows"]["subject_aliases"].as_u64().unwrap() <= 24,
        "aliases by key: {v}"
    );
    assert_eq!(
        v["edges"],
        json!(["paul ast000 > paul ast002", "paul ast002 > paul ast000"]),
        "a mention through an alias chain: {v}"
    );
}

/// Finding 6: a tick read every fact of every touched subject -- the member
/// is touched by nearly every write -- and the stored edges by src/dst.
#[test]
fn a_tick_never_reads_a_hub_and_reads_edges_only_by_id() {
    let v = sim(r#"
f = base() + [fact("h%d" % i, "eh%d" % i, "peer m", "The member likes tea number %d." % i,
                   "01-%02d" % (10 + i % 18), src="m") for i in range(30)]
s = Store({"facts": f})
night(s, "2025-02-10T00:00:00Z")
s.reads.clear()
tick(s, [fact("t1", "et", "Anna Alt", "Anna Alt told the member about a trip.", "03-01", src="m")])
print(json.dumps({"member": [w for (t, w, n) in s.reads if t == "facts" and "peer m" in json.dumps(w)],
                  "edges": [w for (t, w, n) in s.reads if t == "entity_edges"],
                  "fact_rows": sum(n for (t, w, n) in s.reads if t == "facts")}))
"#);
    assert_eq!(
        v["member"],
        json!([]),
        "the hub's facts are never read: {v}"
    );
    for w in v["edges"].as_array().unwrap() {
        assert!(w.get("id").is_some(), "edges are read by id: {w}");
    }
    assert!(v["fact_rows"].as_u64().unwrap() <= 4, "{v}");
}

/// Finding 6: the night derived the whole graph in ONE bundle -- 15 306 ops
/// for 10k facts x 500 entities against a 15 s cell timeout. Now: a paged
/// backfill on an empty graph, a delta afterwards, no write bundle over 500
/// ops, no hop chain over the budget the round edge restores.
#[test]
fn the_night_reads_its_delta_and_writes_at_most_500_ops_a_bundle() {
    let v = sim(r#"
facts, names = crowd()
s = Store({"facts": facts})
night(s, "2025-01-02T00:00:00Z")
first = max(len(b) for b in s.bundles)
n_edges = len(s.rows("entity_edges"))
s.reads.clear(); s.bundles.clear()
s.rows("facts").append(dict(facts[950], id="z1", episode_id="ez1", recorded_at="2025-01-03T00:00:00Z",
                            claim="%s met %s at the lake." % (names[3], names[4])))
night(s, "2025-01-04T00:00:00Z", "2025-01-02T00:00:00Z")
print(json.dumps({"first_max": first, "edges": n_edges,
                  "delta_max": max(len(b) for b in s.bundles),
                  "unfiltered": [t for (t, w, n) in s.reads if t == "facts" and not w],
                  "fact_rows": sum(n for (t, w, n) in s.reads if t == "facts"),
                  "hops": s.max_hops}))
"#);
    assert!(
        v["edges"].as_u64().unwrap() > 1000,
        "a graph worth paging: {v}"
    );
    assert!(v["first_max"].as_u64().unwrap() <= 500, "{v}");
    assert!(v["delta_max"].as_u64().unwrap() <= 500, "{v}");
    assert_eq!(
        v["unfiltered"],
        json!([]),
        "the delta never reads every fact: {v}"
    );
    assert!(v["fact_rows"].as_u64().unwrap() <= 20, "{v}");
    assert!(
        v["hops"].as_u64().unwrap() <= 40,
        "within the TTL of 48: {v}"
    );
}

/// Deep review kf89, N6: every write bundle of the delta night derived the
/// WHOLE window again (every fact recorded and closed since, their counts and
/// mention searches), so a bulk day cost (edges / 500) full derivations. The
/// window goes a page at a time now, and the pages end where one derivation of
/// the whole window ends: 36 facts recorded and 15 closed in ONE window, twelve
/// subjects born in it, paged by 10 facts and 5 ops -- a small page, not a big
/// window, makes the pages (kern2 KF-F N6: the 1 860-fact crowd took 162 s).
/// Both streams page: one page of closed facts (they sort first), then four
/// that end the recorded ones.
#[test]
fn the_delta_night_reads_its_window_a_page_at_a_time() {
    let v = sim(r#"
facts, names = crowd(n_ent=12, per=3, member=40)
member = [f for f in facts if f["subject"] == "peer m"]
rest = [f for f in facts if f["subject"] != "peer m"]
out = {}
for page, ops in ((10, 5), (10 ** 6, 500)):
    CODE = patched(BACKFILL_FACTS=page, WRITE_OPS=ops)
    s = Store({"facts": member})
    night(s, "2025-01-02T00:00:00Z")
    for i, f in enumerate(rest):
        s.rows("facts").append(dict(f, recorded_at="2025-01-03T09:00:00.%06dZ" % i))
    for f in s.rows("facts")[:30:2]:
        f.update(closure_source="close:x:retract", expired_at="2025-01-03T10:00:00Z")
    s.reads.clear()
    night(s, "2025-01-04T00:00:00Z", "2025-01-02T00:00:00Z")
    window = [(json.dumps(w, sort_keys=True), n) for (t, w, n) in s.reads
              if t == "facts" and w and ("recorded_at" in w or "expired_at" in w)]
    out[page] = {"rows": [s.live("entities"), s.live("entity_edges")],
                 "window_max": max(n for _, n in window),
                 "pages": len({w for w, _ in window if "recorded_at" in w})}
print(json.dumps({"same": out[10]["rows"] == out[10 ** 6]["rows"],
                  "paged_max": out[10]["window_max"], "whole_max": out[10 ** 6]["window_max"],
                  "pages": out[10]["pages"], "whole_pages": out[10 ** 6]["pages"],
                  "entities": len(out[10]["rows"][0]), "edges": len(out[10]["rows"][1])}))
"#);
    assert_eq!(v["whole_max"], 36, "one page holds the whole window: {v}");
    assert_eq!(v["whole_pages"], 1, "{v}");
    assert_eq!(
        v["paged_max"], 10,
        "no read of the window returns more than one page: {v}"
    );
    assert_eq!(
        v["pages"], 5,
        "15 closed and 36 recorded facts, 10 a page: 1 + 4 pages: {v}"
    );
    assert_eq!(
        v["same"], true,
        "the pages end where the whole window ends: {v}"
    );
    assert!(v["entities"].as_u64().unwrap() > 12, "{v}");
    assert!(
        v["edges"].as_u64().unwrap() > 50,
        "more writes than one bundle a page: {v}"
    );
}

/// Deep review kf89, N7: a newborn's mention search was cut at 200 facts
/// without a word (a mention beyond it never got an edge). A full page is
/// said in the journal.
#[test]
fn a_full_mention_search_is_said_in_the_journal() {
    let v = sim(r#"
CODE = patched(NEWBORN_HITS=3)
s = Store({"facts": base() + [fact("o%d" % i, "eo%d" % i, "peer m", "The member met Dora Dahl.",
                                    "01-%02d" % (10 + i), src="m") for i in range(5)]})
night(s, "2025-02-01T00:00:00Z")
s.rows("facts").extend([fact("d1", "e21", "Dora Dahl", "Dora Dahl met Anna Alt.", "02-02"),
                        fact("d2", "e22", "Dora Dahl", "Dora Dahl plays tennis.", "02-03")])
del JOURNAL[:]
night(s, "2025-03-01T00:00:00Z", "2025-02-01T00:00:00Z")
print(json.dumps({"full": [l for l in JOURNAL if "came back full" in l and "dora dahl" in l]}))
"#);
    assert!(
        !v["full"].as_array().unwrap().is_empty(),
        "a full mention page is said: {v}"
    );
}

/// Deep review kf89, N7: 16 fact pages and 32 walk starts were fixed in the
/// script, and a walked node past the 16th got no page and no word. Both are
/// params, and a walk that reached more nodes than get a page reports the cut.
#[test]
fn the_graph_page_sizes_are_params_and_a_node_without_a_page_is_the_cap() {
    let doc = json!({"header": {"context": {"mem_phase": "-", "recall_id": "r",
                                            "recall_query": "Who are the flatmates of Bo?",
                                            "audience_now": AUD, "channel": "tg:private"},
                                "hop": {}}, "messages": []});
    let epi = r#"
import ast
_defs = [n for n in ast.parse(_script).body if isinstance(n, ast.FunctionDef)]
exec(compile(ast.Module(body=_defs, type_ignores=[]), "defs", "exec"), globals())
seq = [["n%02d" % i, None, None] for i in range(GRAPH_FACT_PAGES + 1)]
over = graph_leg_of({"page_paths": len(seq), "fact_nodes": len(seq), "seq": seq}, [])
at = graph_leg_of({"page_paths": len(seq) - 1, "fact_nodes": len(seq) - 1, "seq": seq[:-1]}, [])
print(json.dumps({"over": over.get("capped_at") if isinstance(over, dict) else None,
                  "at": at.get("capped_at") if isinstance(at, dict) else None,
                  "pages": GRAPH_FACT_PAGES}))
"#;
    let v = python(&script_of(RECALL), &doc, epi);
    assert_eq!(v["pages"], 16, "{v}");
    assert_eq!(v["over"], v["pages"], "{v}");
    assert_eq!(v["at"], Value::Null, "{v}");

    let anchors: Vec<Value> = (0..12)
        .map(|i| json!({"id": format!("e{i}"), "canonical_name": format!("Aa Name{i:02}"),
                        "subject_key": format!("aa name{i:02}"), "kind": "subject", "valid_until": null}))
        .collect();
    let doc = json!({
        "header": {"context": {"mem_phase": "t1-join", "recall_id": "r", "memory_tier": "1",
                               "recall_query": "Where does Aa Name03 live now?",
                               "audience_now": AUD, "channel": "tg:private"},
                   "hop": {"operation": "bundle"}},
        "params": {"tier1_graph_start": 4},
        "messages": [
            {"origin": "tool", "type": "tool_result", "id": "r-join-anchor", "text": json!(anchors).to_string()},
            {"origin": "tool", "type": "tool_result", "id": "r-join-anchor-key", "text": "[]"},
            {"origin": "tool", "type": "tool_result", "id": "r-join-sem", "text": "[]"}]});
    let out = match python(&script_of(RECALL), &doc, "") {
        Value::Array(a) => a,
        other => vec![other],
    };
    let (calls, _) = writes(&out);
    let walk = calls
        .iter()
        .find(|c| c["operation"] == "traverse")
        .expect("a walk");
    let start = walk["start"].as_array().unwrap();
    assert_eq!(
        start.len(),
        4,
        "the walk starts at `tier1_graph_start`: {walk}"
    );
    assert!(start.contains(&json!("aa name03")), "{walk}");
}

/// Finding 5: the gate saw only the LAST edge of a path, so A -[a room the
/// asker is not in]-> B -[visible]-> C handed over C.
#[test]
fn a_path_reaches_a_node_only_over_edges_the_asker_may_see() {
    let hidden =
        json!({"episode_id": "ep-h", "channel": "tg:other", "audience_set": r#"["member:x"]"#});
    let open = json!({"episode_id": "ep-o", "channel": "tg:private", "audience_set": AUD});
    let paths = json!({"paths": [
        {"node": "bo berg", "depth": 1, "weight_sum": 1, "path": ["al alt", "bo berg"], "edge": hidden},
        {"node": "cy cohn", "depth": 2, "weight_sum": 2, "path": ["al alt", "bo berg", "cy cohn"], "edge": open},
        {"node": "di dahl", "depth": 1, "weight_sum": 1, "path": ["al alt", "di dahl"], "edge": open}]});
    let legs = json!({"kw-ep": [], "kw-fact": [], "temporal": [], "beliefs": [],
                      "anchors": ["Al Alt"], "axis": {}, "model": {"model_id": "m-1", "dim": 1024}});
    let scratch = |leg: &str, p: &Value| json!({"request_id": "r", "leg": leg, "payload": p.to_string(), "fired": 0});
    let doc = json!({
        "header": {"context": {"mem_phase": "t1-legs", "recall_id": "r", "memory_tier": "1",
                               "recall_query": "Al Alt", "audience_now": AUD, "channel": "tg:private"},
                   "hop": {"operation": "bundle"}},
        "messages": [
            {"origin": "tool", "type": "tool_result", "id": "r-legs-graph", "text": paths.to_string()},
            {"origin": "tool", "type": "tool_result", "id": "r-legs-sem-aud", "text": "[]"},
            {"origin": "tool", "type": "tool_result", "id": "r-legs-read",
             "text": json!([scratch("legs", &legs), scratch("sem", &json!([]))]).to_string()}]});
    let out = match python(&script_of(RECALL), &doc, "") {
        Value::Array(a) => a,
        other => vec![other],
    };
    let (calls, _) = writes(&out);
    let asked: Vec<Value> = calls
        .iter()
        .filter(|c| c["table"] == "facts")
        .map(|c| c["where"]["canonical_subject"].clone())
        .collect();
    assert_eq!(asked, vec![json!("di dahl")], "{out:?}");
}

/// Findings 7 and 9: a path's episode needs the question in its relation (the
/// facts did since loop 2), and a FULL node page is what the cap reports.
#[test]
fn a_path_episode_needs_the_question_and_a_full_page_is_the_cap() {
    let doc = json!({"header": {"context": {"mem_phase": "-", "recall_id": "r",
                                            "recall_query": "Who are the flatmates of Bo?",
                                            "audience_now": AUD, "channel": "tg:private"},
                                "hop": {}}, "messages": []});
    // The parked phase leaves the cell before the leg's functions are
    // defined; the epilogue lifts every function out of the shipped source.
    let epi = r#"
import ast
_defs = [n for n in ast.parse(_script).body if isinstance(n, ast.FunctionDef)]
exec(compile(ast.Module(body=_defs, type_ignores=[]), "defs", "exec"), globals())
walk = {"page_paths": 2, "seq": [["bo berg", "ep-1", 0], ["cy cohn", "ep-2", 1]]}
eps = [h for h in graph_leg_of(walk, []) if h["kind"] == "episode"]
def rows(node, n):
    return [{"id": "%s-%d" % (node[:2], i), "canonical_subject": node, "claim": "x", "audience_set": _AUD,
             "channel": "tg:private", "valid_from": "2025-01-%02d" % (1 + i % 28)} for i in range(n)]
_AUD = '["member:m"]'
full = graph_leg_of({"page_paths": 1, "seq": [["bo berg", None, None]]}, rows("bo berg", GRAPH_NODE_PAGE))
spread = graph_leg_of({"page_paths": 3, "seq": [[n, None, None] for n in ("bo berg", "cy cohn", "di dahl")]},
                      rows("bo berg", 34) + rows("cy cohn", 34) + rows("di dahl", 34))
print(json.dumps({"eps": eps, "full": full.get("capped_at") if isinstance(full, dict) else None,
                  "spread": spread.get("capped_at") if isinstance(spread, dict) else None,
                  "page": GRAPH_NODE_PAGE}))
"#;
    let v = python(&script_of(RECALL), &doc, epi);
    assert_eq!(
        v["eps"],
        json!([{"kind": "episode", "id": "ep-2"}]),
        "an edge whose relation shares nothing with the question brings no episode: {v}"
    );
    assert_eq!(v["full"], v["page"], "{v}");
    assert_eq!(v["spread"], Value::Null, "{v}");
}

/// Finding 8: the walk starts at most 32 anchors; they were cut in alphabetical
/// order, so on a large hive the one the question names fell out.
#[test]
fn the_anchor_the_question_names_starts_the_walk() {
    let mut anchors: Vec<Value> = (0..40)
        .map(|i| json!({"id": format!("e{i}"), "canonical_name": format!("Aa Name{i:02}"),
                        "subject_key": format!("aa name{i:02}"), "kind": "subject", "valid_until": null}))
        .collect();
    anchors.push(
        json!({"id": "z", "canonical_name": "Zora Zett", "subject_key": "zora zett",
                        "kind": "subject", "valid_until": null}),
    );
    let doc = json!({
        "header": {"context": {"mem_phase": "t1-join", "recall_id": "r", "memory_tier": "1",
                               "recall_query": "Where does Zora Zett live now?",
                               "audience_now": AUD, "channel": "tg:private"},
                   "hop": {"operation": "bundle"}},
        "messages": [
            {"origin": "tool", "type": "tool_result", "id": "r-join-anchor", "text": json!(anchors).to_string()},
            {"origin": "tool", "type": "tool_result", "id": "r-join-anchor-key", "text": "[]"},
            {"origin": "tool", "type": "tool_result", "id": "r-join-sem", "text": "[]"}]});
    let out = match python(&script_of(RECALL), &doc, "") {
        Value::Array(a) => a,
        other => vec![other],
    };
    let (calls, _) = writes(&out);
    let walk = calls
        .iter()
        .find(|c| c["operation"] == "traverse")
        .expect("a walk");
    assert!(
        walk["start"]
            .as_array()
            .unwrap()
            .contains(&json!("zora zett")),
        "{walk}"
    );
}

#[test]
fn the_graph_leg_brings_the_connecting_fact() {
    let doc = json!({"header": {"context": {"mem_phase": "-", "recall_id": "r",
                                            "recall_query": "In which city does the flatmate of my grandson live now?",
                                            "audience_now": AUD, "channel": "tg:private"},
                                "hop": {}}, "messages": []});
    let epi = r#"
paths = [
  {"node": "frieda quastberg", "depth": 1, "weight_sum": 1, "path": ["greta grimmbach", "frieda quastberg"],
   "edge": {"relation": "Frieda Quastberg and Greta Grimmbach are each other's flatmates."}},
  {"node": "edda dornbusch", "depth": 1, "weight_sum": 3, "path": ["greta grimmbach", "edda dornbusch"],
   "edge": {"relation": "Peer said they heard Greta Grimmbach had a ferret."}},
]
order = [r["node"] for r in rank_walk(paths, {"greta grimmbach": 1}, query)]
facts = [
  {"id": "x1", "claim": "Peer said they had potato soup for lunch.", "valid_from": "2025-07-01"},
  {"id": "x2", "claim": "Frieda Quastberg lives in Bremen.", "valid_from": "2025-02-02"},
  {"id": "x3", "claim": "Peer said pigeons nest on the balcony.", "valid_from": "2025-06-30"},
  {"id": "x4", "claim": "Frieda Quastberg moved from Bremen to Salzburg.", "valid_from": "2025-05-22"},
  {"id": "x5", "claim": "Peer said the tram driver waited.", "valid_from": "2025-06-29"},
]
top = [r["id"] for r in rank_node_facts(facts, query, 2)]
anchors = sorted(name_anchors("", ["Frieda Quastberg and Greta Grimmbach are each other's flatmates."]))
pet = [r["id"] for r in rank_node_facts([
  {"id": "p1", "claim": "Peer said they had potato soup.", "predicate": "lunch", "valid_from": "2025-07-01"},
  {"id": "p2", "claim": "Greta Grimmbach now has a ferret called Pixel.", "predicate": "has_pet", "valid_from": "2025-06-01"},
], "What pet does the best friend of my former colleague have now?", 3)]
print(json.dumps({"order": order, "top": top, "anchors": anchors, "cap": GRAPH_NODE_FACTS, "pet": pet}))
"#;
    let v = python(&script_of(RECALL), &doc, epi);
    assert_eq!(
        v["order"],
        json!(["frieda quastberg", "edda dornbusch"]),
        "the flatmate edge first"
    );
    assert_eq!(
        v["top"],
        json!(["x2", "x4"]),
        "the question's fact, then the one that changed its value"
    );
    for a in ["Frieda Quastberg", "Greta Grimmbach", "greta grimmbach"] {
        assert!(
            v["anchors"].as_array().unwrap().contains(&json!(a)),
            "{a} in {:?}",
            v["anchors"]
        );
    }
    assert_eq!(v["cap"], 3);
    assert_eq!(
        v["pet"],
        json!(["p2"]),
        "snake_case predicates meet the question; small talk gets no seat"
    );
    let s = script_of(RECALL);
    assert!(
        s.contains(r#""edge_kind": {"or_null": {"neq": "co-hub"}}"#),
        "the walk never enters a hub"
    );
    assert!(
        s.contains(r#""r-join-anchor-key""#),
        "anchors find the produced entities by key"
    );
}

/// A hop of the cell with the given phase, operation and error code; the
/// epilogue runs after the script parked (an unknown phase parks).
fn hop_doc(phase: &str, op: &str, error_code: &str) -> Value {
    let mut hop = json!({"operation": op});
    if !error_code.is_empty() {
        hop["error_code"] = json!(error_code);
    }
    json!({
        "header": {"context": {"mem_phase": phase, "ent_clock": "2026-03-03T00:00:00Z"},
                   "hop": hop},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "x", "text": "[]"}]
    })
}

/// Deep review of kf89 (N8): a store refusal arrives looking like an answer;
/// read as one it was an empty read, and the tick derived from nothing. It
/// ends the hop now, said in the journal.
#[test]
fn a_store_refusal_ends_the_hop() {
    let out = entity_glue(&hop_doc(
        r#"ent-p|{"ids":["f1"]}"#,
        "bundle",
        "store_unavailable",
    ));
    assert!(out.is_empty(), "a refused read was derived from: {out:?}");
}

/// Review KF-F (F2): only a refused READ ends the hop. A refused night write
/// -- a tick racing the night into a `unique_violation`, a transient store
/// error -- starts the next step, which derives anew from the store and so
/// heals it; parked, it ended the night, and no later night fetches the rest
/// of its window.
#[test]
fn a_refused_night_write_derives_anew() {
    for code in ["unique_violation", "store_unavailable"] {
        let out = entity_glue(&hop_doc(
            r#"ent-nw|{"k":0,"m":"d","n":3,"r":1,"s":0}"#,
            "bundle",
            code,
        ));
        assert_eq!(out.len(), 1, "{code}: {out:?}");
        assert_eq!(out[0]["header"]["route"], "entity", "{code}");
        assert_eq!(
            out[0]["header"]["phase"], r#"ent-n|{"k":0,"m":"d","n":3,"r":1,"s":1}"#,
            "{code}"
        );
    }
}

/// Review N5: the closed facts of a night are read from the PREVIOUS night's
/// window start on -- a tick that wrote a forgotten fact back into an edge's
/// evidence during the night is healed by the next one. Review T2: without a
/// `done` run the window starts at the newest earlier run of any status, not
/// at the start of the whole history.
#[test]
fn the_night_window_overlaps_and_falls_back_to_the_last_run() {
    let closed = python(
        &script_of(ENTITY),
        &hop_doc("unknown", "select", ""),
        "pl, ops, t, _ = night_delta({'f': '2026-02-02', 'pf': '2026-02-01'}, {}, '2026-02-03')\n\
         print(json.dumps([a['where'] for c, a in pl.calls if c.startswith('g-closed')]))",
    );
    assert_eq!(closed, json!([{"expired_at": {"gt": "2026-02-01"}}]));
    let from = python(
        &script_of(ENTITY),
        &hop_doc("unknown", "select", ""),
        "print(json.dumps([night_from([{'status': 'skipped', 'delta_from': 'A', \
         'delta_to': 'B'}], 'C'), night_prev([{'status': 'done', 'delta_from': 'A', \
         'delta_to': 'B'}], 'C')]))",
    );
    assert_eq!(from, json!(["B", "A"]));
}

//! GH #951 -- the graph's cells keep their contract, and a set found
//! complete is not overtaken by the next announcement.
//!
//! No colony: the WHOLE shipped scripts of `index` and `query` run one
//! delivery at a time, the way the code cell feeds them (the document on
//! stdin, the emissions on stdout), and every store bundle they emit is
//! answered by the store stand-in of `support/graph_space_pure.rs` (`SIM`,
//! here with the two unique indexes the store declares and a `traverse` that
//! finds nothing). The test plays the sources.
//!
//! 1. `a_set_found_complete_is_not_overtaken_by_the_next_announcement`
//!    (OR-BC.G.6, C1 finding G "no lock on the G.6 window"): the set of v1
//!    is found complete -- `announced` is v1 in the bundle that completes it
//!    -- and its phase-1 bundle is held back; v2 is announced; THEN v1's
//!    phase 1 lands, then v2's answers. The store ends at v2: its version,
//!    its nodes only, no parked pull, the round v2 announced. Once for a
//!    file space (three parts) and once for an object hive (two parts, no
//!    `near`, a round). Red before: an object hive was pulled for `near` too,
//!    no pull named its source, and no round was kept.
//! 2. `every_emission_keeps_the_cell_contract` (C1 finding G "no harness run
//!    of the error answer against `contract.emits`"): every message `index`
//!    and `query` emit on a small world -- store bundles, pulls, a normal
//!    answer of every op, the refusals (`bad_address`, `bad_request`,
//!    `unknown_op`, a question without `op_id`) -- carries only body and hop
//!    keys its cell's `contract.emits` declares, each of the declared type
//!    and value set, every required one present. The runtime validates every
//!    emission and a mismatch is a silent drop; this is the unit-level
//!    proof. Red before: `stats` answered `sources`, `nodes`, `edges`, ...
//!    which the contract did not declare.
//!
//! Free of a paid provider by construction. Guarded like every
//! template-reading test (GH #49).

#[path = "support/graph_space_pure.rs"]
mod pure;

use meclaw_core::serde_json::{self as sj, Map, Value, json};
use std::io::Write;
use std::process::{Command, Stdio};

/// The cells as the runner runs them, and the store between them.
const DRIVER: &str = r##"
import io, json, sys

UNIQUE = {"sources": ("source",), "pulls": ("source", "version", "part")}


def run_cell(cell, hop, ctx, body):
    """One delivery to a shipped code cell, the way the runner feeds it."""
    src, params = CELLS[cell]
    doc = {"params": params, "body": body,
           "envelope": {"header": {"hop": hop, "context": ctx}}}
    keep_in, keep_out = sys.stdin, sys.stdout
    sys.stdin, sys.stdout = io.StringIO(json.dumps(doc)), io.StringIO()
    try:
        exec(compile(src, cell, "exec"), {"__name__": "__cell__"})
    except SystemExit:
        pass
    finally:
        out = sys.stdout.getvalue()
        sys.stdin, sys.stdout = keep_in, keep_out
    got = json.loads(out or "[]")
    return got if isinstance(got, list) else [got]


def store_reply(db, msg):
    """The store's answer to one bundle, over the stand-in: a unique index on
    `sources.source` and on `pulls (source, version, part)`, a `traverse`
    that finds nothing. -> (hop, context, body) for the asking cell."""
    ops = [(m["id"], json.loads(m["text"])) for m in msg.get("messages") or []]
    turns, results = [], []
    for cid, op in ops:
        code, payload = "", None
        key = UNIQUE.get(op.get("table")) if op.get("operation") == "insert" else None
        if key and any(all(r.get(k) == op["row"].get(k) for k in key)
                       for r in db.get(op["table"], [])):
            code = "unique_violation"
        elif op.get("operation") == "traverse":
            payload = {"paths": [], "truncated": False}
        else:
            payload = run_bundle(db, [(cid, op)])[cid]
        turns.append({"origin": "tool", "type": "tool_result", "id": cid,
                      "text": json.dumps(payload)})
        results.append({"tool_call_id": cid, "error_code": code})
    h = msg.get("header") or {}
    return ({"operation": "bundle"},
            {"cur_origin": h.get("reply_cell"), "cur_phase": h.get("phase"),
             "cur_call": h.get("cur_call")},
            {"messages": turns, "results": results})


def settle(db, cell, msgs, hold=None, log=None):
    """Answer every store bundle among `msgs`, and among what the cell sends
    back, until only outbound messages are left. `hold` keeps a bundle back
    (returned, not answered); `log` collects every emission. -> (out, held)"""
    out, held, todo = [], [], list(msgs)
    while todo:
        m = todo.pop(0)
        if log is not None:
            log.append([cell, m])
        if (m.get("header") or {}).get("route") in ("index_store", "query_store"):
            if hold and hold(m):
                held.append(m)
                continue
            todo.extend(run_cell(cell, *store_reply(db, m)))
        else:
            out.append(m)
    return out, held


def deliver(db, cell, hop, ctx, body, hold=None, log=None):
    return settle(db, cell, run_cell(cell, hop, ctx, body), hold, log)


def announce(db, S, version, path, nodes, links, aud=None, log=None):
    body = {"source": S, "version": version, "path": path, "fmt": "", "parser": "test",
            "mark": "", "nodes": nodes, "links": links, "tomb": "", "messages": []}
    if aud is not None:
        body["audience_set"] = aud
    return deliver(db, "index", {"route": "source_changed"}, {}, body, log=log)[0]


def answer_pull(db, pull, body, hold=None, log=None):
    h = pull["header"]
    b = dict(body, ok=True, op=h["op"], op_id=h["op_id"], messages=[])
    return deliver(db, "index", {"route": "in_pulled", "op": h["op"], "op_id": h["op_id"]},
                   {}, b, hold, log)


def answers_for(nodes, links, near=None):
    return {"outline": {"nodes": nodes}, "links": {"links": links},
            "near": {"near": near or []}}


def index_source(db, S, version, path, nodes, links, aud=None, near=None, log=None):
    pulls = announce(db, S, version, path, len(nodes), len(links), aud, log)
    ans = answers_for(nodes, links, near)
    for p in pulls:
        answer_pull(db, p, ans[p["header"]["op"]], log=log)
    return pulls


def ask(db, op, args, ctx=None, op_id="q1", log=None):
    out, _ = deliver(db, "query", {"route": "in_graph", "op": op, "op_id": op_id}, ctx or {},
                     {"op": op, "args": args, "messages": []}, log=log)
    return out


def g6_window(S, path, aud1, aud2, nodes1, nodes2):
    """OR-BC.G.6: the set of v1 is found complete (its phase-1 bundle is
    written and held back), v2 is announced, THEN v1's phase 1 lands, then
    v2's pulls are answered. -> what the store holds at the end."""
    db = {}
    v1, v2 = "111111111111", "222222222222"
    pulls1 = announce(db, S, v1, path, len(nodes1), 0, aud1)
    ans1 = answers_for(nodes1, [])
    for p in pulls1[:-1]:
        answer_pull(db, p, ans1[p["header"]["op"]])
    _, held = answer_pull(db, pulls1[-1], ans1[pulls1[-1]["header"]["op"]],
                          hold=lambda m: (m.get("header") or {}).get("phase") == "p1")
    pulls2 = announce(db, S, v2, path, len(nodes2), 0, aud2)
    settle(db, "index", held)
    ans2 = answers_for(nodes2, [])
    for p in pulls2:
        answer_pull(db, p, ans2[p["header"]["op"]])
    return {"held": len(held),
            "pulls": [[p["header"]["op"], p["header"].get("source", "")] for p in pulls1 + pulls2],
            "source": [[s.get("version"), s.get("announced"), s.get("audience_set", "")]
                       for s in db.get("sources", []) if s.get("source") == S],
            "nodes": sorted(n["addr"] for n in db.get("nodes", [])),
            "parked": len(db.get("pulls", []))}


def emissions():
    """Every message `index` and `query` emit on a small world: an announced
    file, an announced object, a question of every op in a covering round,
    the refusals, a removal. -> {cell: [message]}"""
    db, ilog, qlog = {}, [], []
    F, D, O = "fh-f00000000001", "fh-d00000000002", "ob-0b0000000001"
    index_source(db, F, "111111111111", "/w/f.md", [{"anchor": "sec:x", "kind": "sec"}], [],
                 near=[{"file": D, "score": 0.5}], log=ilog)
    index_source(db, D, "111111111111", "/w/d.md", [{"anchor": "sec:d", "kind": "sec"}],
                 [{"kind": "link", "from_anchor": "sec:d", "target_name": "f.md#x"},
                  {"kind": "link", "from_anchor": "sec:d", "target_name": O}], log=ilog)
    index_source(db, O, "aaaaaaaaaaaa", "", [{"anchor": "", "kind": "object", "name": "Kettle"}],
                 [{"kind": "doc", "from_anchor": "", "target_name": F + "#sec:x"},
                  {"kind": "owner", "from_anchor": "", "target_name": "member:p"}],
                 aud='["member:p"]', log=ilog)
    ctx = {"audience_now": '["member:p"]'}
    for i, (op, args) in enumerate([
            ("resolve", {"name": "Kettle"}), ("callers", {"addr": F + "#sec:x"}),
            ("dependents", {"addr": F + "#sec:x", "depth": 2}), ("deps", {"addr": D}),
            ("path", {"from": D, "to": F}), ("similar", {"addr": F}), ("broken", {}),
            ("stats", {}), ("deps", {"addr": "ob-0b0000000777"}),
            ("callers", {"addr": "nowhere"}), ("dependents", {"addr": F, "depth": 9}),
            ("frobnicate", {})]):
        ask(db, op, args, ctx, "q%d" % i, log=qlog)
    deliver(db, "query", {"route": "in_graph", "op": "stats"}, ctx,
            {"op": "stats", "args": {}, "messages": []}, log=qlog)
    deliver(db, "index", {"route": "source_changed"}, {},
            {"source": F, "path": "/w/f.md", "tomb": True, "messages": []}, log=ilog)
    return {"index": [m for _, m in ilog], "query": [m for _, m in qlog]}
"##;

const PROGRAM: &str = r#"
import io, json, sys
OUT = sys.stdout
inp = json.load(sys.stdin)
sys.stdin = io.StringIO("")
scope = {"CELLS": {k: (v["src"], v["params"]) for k, v in inp["cells"].items()}}
exec(compile(inp["extra"], "extra", "exec"), scope)
scope["ARGS"] = inp.get("args")
OUT.write(json.dumps(eval(inp["probe"], scope)))
"#;

fn config(cell: &str) -> Value {
    let p = pure::repo(&format!("templates/graph-space/{cell}/config.json"));
    let raw = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    sj::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// `probe` evaluated beside the driver, with both shipped cells loaded.
fn drive(probe: &str) -> Value {
    let mut cells = Map::new();
    for cell in ["index", "query"] {
        let mut params = config(cell)["params"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        let src = params
            .remove("script_inline")
            .and_then(|v| v.as_str().map(str::to_string))
            .expect("script_inline");
        cells.insert(cell.into(), json!({"src": src, "params": params}));
    }
    let doc = json!({"cells": cells, "extra": format!("{}\n{DRIVER}", pure::SIM),
                     "probe": probe, "args": null});
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(PROGRAM)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(doc.to_string().as_bytes())
        .expect("write the document");
    drop(sink);
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "the driver failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    sj::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("not JSON ({e}): {}", String::from_utf8_lossy(&out.stdout)))
}

#[test]
fn a_set_found_complete_is_not_overtaken_by_the_next_announcement() {
    if !pure::shipped() {
        eprintln!("templates/graph-space did not travel into this tree -- skipped (GH #49)");
        return;
    }
    const F: &str = "fh-f00000000001";
    const O: &str = "ob-0b0000000001";
    let file = drive(&format!(
        "g6_window('{F}', '/w/a.md', None, None, [{{'anchor': 'sec:one'}}], \
         [{{'anchor': 'sec:two'}}])"
    ));
    let object = drive(&format!(
        "g6_window('{O}', '', '[\"member:p\",\"agent:a\"]', '[\"agent:a\",\"member:p\"]', \
         [{{'anchor': '', 'name': 'Kettle'}}, {{'anchor': 'slot:what'}}], \
         [{{'anchor': '', 'name': 'Kettle'}}, {{'anchor': 'slot:how'}}])"
    ));
    let parts = |s: &str, ps: &[&str]| -> Value {
        let one: Vec<Value> = ps.iter().map(|p| json!([p, s])).collect();
        Value::Array([one.clone(), one].concat())
    };
    assert_eq!(
        object["pulls"],
        parts(O, &["outline", "links"]),
        "an object hive is pulled for `outline` and `links` only, each pull naming its source \
         in `hop.source`: {object}"
    );
    assert_eq!(
        file["pulls"],
        parts(F, &["outline", "links", "near"]),
        "a file space is pulled in three parts, each naming its source: {file}"
    );
    for (got, round, nodes) in [
        (&file, "", json!([format!("{F}#sec:two")])),
        (
            &object,
            r#"["agent:a","member:p"]"#,
            json!([O, format!("{O}#slot:how")]),
        ),
    ] {
        assert_eq!(
            got["held"],
            json!(1),
            "the set completed into one held phase 1: {got}"
        );
        assert_eq!(
            got["source"],
            json!([["222222222222", "222222222222", round]]),
            "the store ends at the new version, with the round it announced (canonical): {got}"
        );
        assert_eq!(
            got["nodes"], nodes,
            "only the new version's nodes -- an object's root at its own address: {got}"
        );
        assert_eq!(got["parked"], json!(0), "no parked pull is left: {got}");
    }
}

fn type_fits(ty: &str, v: &Value) -> bool {
    match ty {
        "string" | "blob_uuid" => v.is_string(),
        "number" => v.is_number(),
        "boolean" => v.is_boolean(),
        "object" => v.is_object(),
        "array" => v.is_array(),
        _ => false,
    }
}

/// Every key of `got` declared in `decl`, of its type and value set; every
/// required key of `decl` present.
fn check(what: &str, decl: &Value, got: &Map<String, Value>, out: &mut Vec<String>) {
    for (k, v) in got {
        let Some(spec) = decl.get(k) else {
            out.push(format!("{what}: `{k}` is not declared ({v})"));
            continue;
        };
        let ty = spec["type"].as_str().unwrap_or_default();
        if !type_fits(ty, v) {
            out.push(format!("{what}: `{k}` is declared {ty}, emitted {v}"));
        }
        if let Some(values) = spec["values"].as_array()
            && !values.contains(v)
        {
            out.push(format!("{what}: `{k}` = {v} is not one of {values:?}"));
        }
    }
    for (k, spec) in decl.as_object().into_iter().flatten() {
        if spec["required"] == json!(true) && !got.contains_key(k) {
            out.push(format!("{what}: required `{k}` is missing"));
        }
    }
}

#[test]
fn every_emission_keeps_the_cell_contract() {
    if !pure::shipped() {
        eprintln!("templates/graph-space did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let got = drive("emissions()");
    let mut bad = Vec::new();
    for cell in ["index", "query"] {
        let emits = config(cell)["contract"]["emits"].clone();
        let msgs = got[cell].as_array().cloned().unwrap_or_default();
        assert!(!msgs.is_empty(), "{cell} emitted nothing: {got}");
        for m in &msgs {
            let mut body = m.as_object().cloned().unwrap_or_default();
            let hop = body
                .remove("header")
                .and_then(|h| h.as_object().cloned())
                .unwrap_or_default();
            let route = hop["route"].as_str().unwrap_or_default().to_string();
            check(
                &format!("{cell} {route} body"),
                &emits["body"],
                &body,
                &mut bad,
            );
            check(
                &format!("{cell} {route} hop"),
                &emits["hop"],
                &hop,
                &mut bad,
            );
            if route == "pull" {
                assert_eq!(
                    hop.get("source"),
                    body.get("source"),
                    "a pull names its source in `hop.source` (#951): {m}"
                );
            }
        }
    }
    assert!(
        bad.is_empty(),
        "emissions outside their cell's contract.emits:\n{}",
        bad.join("\n")
    );
    let answers: Vec<&Value> = got["query"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["header"]["route"] == json!("answer"))
        .collect();
    let mut ok_ops: Vec<String> = answers
        .iter()
        .filter(|m| m["ok"] == json!(true))
        .filter_map(|m| m["op"].as_str().map(str::to_string))
        .collect();
    ok_ops.sort();
    ok_ops.dedup();
    assert_eq!(
        ok_ops,
        [
            "broken",
            "callers",
            "dependents",
            "deps",
            "path",
            "resolve",
            "similar",
            "stats"
        ],
        "a normal answer of every op was checked"
    );
    let mut codes: Vec<String> = answers
        .iter()
        .filter(|m| m["ok"] == json!(false))
        .filter_map(|m| m["error"]["code"].as_str().map(str::to_string))
        .collect();
    codes.sort();
    assert_eq!(
        codes,
        ["bad_address", "bad_request", "bad_request", "unknown_op"],
        "the refusals were checked: {answers:#?}"
    );
}

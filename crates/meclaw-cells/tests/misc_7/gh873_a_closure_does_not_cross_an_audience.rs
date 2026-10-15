//! GH #873 -- the nightly run closes no fact on the word of a fact whose
//! audience does not cover it.
//!
//! Source and audience are two axes. A statement the member makes in a room
//! (audience `{member, agent, peer}`) and one the member makes privately
//! (`{member, agent}`) share a source -- the member's own side -- and can share
//! an axis. If the judge rules the private one the successor and the closure
//! stands, the room sees its fact "superseded by something you may not see"
//! (`supersession_unknown`): a statement about private knowledge that crossed
//! the audience boundary.
//!
//! The closure is written in `canon-judged` deliberately without any condition
//! on the two rows (the judge sees the verdict, not the rows), and `sup-axes` is
//! the one place that holds every closure of every axis to the rules afterwards
//! -- since 3.5.0 (#849) it withdraws one that crosses two sources. So this
//! measures the END state of a night, not the judge's write: the judgement is
//! applied to an emulated store, the store is read back through the real
//! `sup-scope` and `sup-axes` selects (projected onto exactly the columns the
//! script asks for, the way the store answers), and whatever `sup-axes` writes
//! is applied too. Both `sup-axes` branches run: the normal page and the full
//! page (`dream_axis_limit` at the row count).
//!
//! Everything is the REAL shipped `params.script_inline` of `dream-glue`, run
//! over stdin with injected store replies and an injected verdict; no colony, no
//! model, nothing paid.
//!
//! The cases (plan table K1-K5, rulings OR-FD-D2/D3):
//!
//! | case | closed fact           | successor               | end state |
//! |------|-----------------------|-------------------------|-----------|
//! | K1   | own, {m, a}           | peer-heard, {a, p}      | withdrawn (source rule, control) |
//! | K2   | own, {m, a, p} (room) | own, {m, a} (private)   | withdrawn |
//! | K3   | own, {m, a}           | own, {m, a, p}          | stands |
//! | K4   | own, {m, a}           | own, `*`                | stands |
//! | K5   | no audience (old row) | own, {m, a}             | stands (no boundary) |
//! | K6   | own, {m, a, p} (room) | the SAME words, {m, a}  | open (re-assertion, orchestrator ruling OR-FD-62) |

use meclaw_core::serde_json::{self, Value, json};
use meclaw_testing::{code_stdin, run_shipped_script, shipped_script};

const DREAM: &str = "../../templates/memory-hive/dream-glue/config.json";
const RUN: &str = "r873";
const TO: &str = "2026-09-28T03:00:00Z";

const MEMBER: &str = "member:m";
const AGENT: &str = "agent:a";
const PEER: &str = "peer:p";
/// The claims are the one content a withdrawal log line must never carry.
const CLOSED_CLAIM: &str = "meets the team on tuesdays";
const SUCCESSOR_CLAIM: &str = "meets the team on thursdays";

fn args_of(msg: &Value) -> Value {
    let text = msg["messages"][0]["text"].as_str().expect("op text");
    serde_json::from_str(text).unwrap_or(Value::Null)
}

/// One run of the real script; the emissions and the stderr it wrote.
fn run(doc: &Value) -> (Vec<Value>, String) {
    let out = run_shipped_script(&shipped_script(DREAM), &code_stdin(doc).to_string());
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "dream-glue exited non-zero: {stderr}");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not json ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    let msgs = match v {
        Value::Array(a) => a,
        other => vec![other],
    };
    (msgs, stderr)
}

/// The two facts of one axis, both open, as the store holds them. `None` for an
/// audience is a row written before the column existed (SQL NULL).
fn store_rows(
    closed_source: &str,
    closed_aud: Option<Value>,
    succ_source: &str,
    succ_aud: Option<Value>,
) -> Vec<Value> {
    let aud = |a: Option<Value>| a.map_or(Value::Null, |v| Value::String(v.to_string()));
    let fact = |id: &str, claim: &str, from: &str, source: &str, audience: Value| {
        json!({"id": id, "subject": "user", "canonical_subject": "user",
               "predicate": "meets_team_on", "canonical_predicate": "meets_team_on",
               "claim": claim, "canonical_claim": claim,
               "valid_from": from, "valid_until": null, "recorded_at": from,
               "expired_at": null, "superseded_by": null, "closure_source": "",
               "session_id": "s-".to_string() + id, "source": source,
               "audience_set": audience})
    };
    vec![
        fact(
            "f1",
            CLOSED_CLAIM,
            "2026-09-10T10:00:00Z",
            closed_source,
            aud(closed_aud),
        ),
        fact(
            "f2",
            SUCCESSOR_CLAIM,
            "2026-09-20T10:00:00Z",
            succ_source,
            aud(succ_aud),
        ),
    ]
}

/// Does a row satisfy one `where` clause the script wrote?
fn matches(row: &Value, clause: &Value) -> bool {
    clause
        .as_object()
        .expect("where")
        .iter()
        .all(|(col, want)| {
            let have = row.get(col).cloned().unwrap_or(Value::Null);
            match want {
                Value::Object(o) if o.contains_key("is_null") => {
                    o["is_null"] == json!(have.is_null())
                }
                Value::Object(o) if o.contains_key("in") => {
                    o["in"].as_array().expect("in list").contains(&have)
                }
                Value::Object(o) => panic!("the emulated store does not know {o:?} on {col}"),
                other => *other == have,
            }
        })
}

/// A select, answered the way the store answers it: the matching rows, each
/// carrying exactly the columns the script asked for and no other.
fn select(rows: &[Value], args: &Value) -> Value {
    let columns: Vec<String> = args["columns"]
        .as_array()
        .expect("columns")
        .iter()
        .map(|c| c.as_str().expect("column").to_string())
        .collect();
    let empty = json!({});
    let clause = args.get("where").unwrap_or(&empty);
    Value::Array(
        rows.iter()
            .filter(|r| matches(r, clause))
            .map(|r| {
                let mut out = serde_json::Map::new();
                for c in &columns {
                    out.insert(c.clone(), r.get(c).cloned().unwrap_or(Value::Null));
                }
                Value::Object(out)
            })
            .collect(),
    )
}

/// Apply every `facts` update of an emission to the emulated store; the number
/// applied.
fn apply(rows: &mut [Value], msgs: &[Value]) -> usize {
    let mut n = 0;
    for a in msgs.iter().map(args_of) {
        if a["operation"] != "update" || a["table"] != "facts" {
            continue;
        }
        n += 1;
        for r in rows.iter_mut().filter(|r| matches(r, &a["where"])) {
            for (k, v) in a["set"].as_object().expect("set") {
                r[k.as_str()] = v.clone();
            }
        }
    }
    n
}

/// The store's answer to one op, as the store edge delivers it back.
fn reply(op: &Value, rows: Value, params: &Value) -> Value {
    json!({
        "header": {
            "context": {"store_origin": "dream", "mem_phase": op["header"]["phase"],
                        "dream_run": RUN, "dream_to": TO},
            "hop": {"operation": args_of(op)["operation"], "rows_affected": 1}
        },
        "params": params,
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r", "text": rows.to_string()}]
    })
}

/// The one op of an emission that carries `phase` on its hop.
fn op_for<'a>(msgs: &'a [Value], phase: &str) -> &'a Value {
    msgs.iter()
        .find(|m| m["header"]["phase"] == phase)
        .unwrap_or_else(|| panic!("no op for {phase}: {msgs:?}"))
}

struct Night {
    /// f1 after the whole night.
    closed: Value,
    /// stderr of the `sup-axes` run.
    log: String,
}

impl Night {
    fn stands(&self) -> bool {
        self.closed["superseded_by"] == "f2"
            && self.closed["expired_at"].is_string()
            && self.closed["closure_source"] == format!("judge:{RUN}")
    }

    fn withdrawn(&self) -> bool {
        self.closed["superseded_by"].is_null()
            && self.closed["expired_at"].is_null()
            && self.closed["closure_source"] == ""
    }
}

/// One night over the two facts: the judge closes f1 with f2, then the
/// materialisation reads the store back and writes what it writes.
fn night(rows: Vec<Value>, full_page: bool) -> Night {
    night_judged(rows, full_page, true)
}

/// The same night, with or without the judge's closure of f1 by f2. Without
/// it the judge answers an empty verdict and only the arithmetic of `sup-axes`
/// closes anything -- the re-assertion of one statement.
fn night_judged(mut rows: Vec<Value>, full_page: bool, judged_closure: bool) -> Night {
    // A full page is `len(rows) >= dream_axis_limit`; both facts are one axis.
    let params = if full_page {
        json!({"dream_axis_limit": rows.len()})
    } else {
        json!({})
    };

    // 1. canon-judged: the verdict is WRITTEN, on every case -- the judge
    //    decides what is true, and that half is not what this issue changes.
    let verdict = if judged_closure {
        json!({"closures": [{
            "subject": "user", "predicate": "meets_team_on", "closed": "f1",
            "superseded_by": "f2", "ended_at": "2026-09-20T10:00:00Z",
            "reason": "the member moved the meeting to thursdays"}]})
    } else {
        json!({"closures": []})
    };
    let (judged, _) = run(&json!({
        "header": {"context": {"store_origin": "dream", "mem_phase": "canon-judged",
                               "dream_run": RUN, "dream_to": TO},
                   "hop": {"finish_reason": "stop"}},
        "params": params,
        "messages": [{"origin": "assistant", "type": "text", "text": verdict.to_string()}]
    }));
    assert_eq!(
        apply(&mut rows, &judged),
        usize::from(judged_closure),
        "the judge wrote the wrong number of closures: {judged:?}"
    );
    if judged_closure {
        assert_eq!(
            rows[0]["superseded_by"], "f2",
            "canon-judged did not close f1: {judged:?}"
        );
    }

    // 2. The round's end hands over to the arithmetic through its one door.
    let canon_done = op_for(&judged, "canon-done");
    let (scoped, _) = run(&reply(canon_done, json!([]), &params));
    let scope = op_for(&scoped, "sup-scope");
    let (axes, _) = run(&reply(scope, select(&rows, &args_of(scope)), &params));
    let axes_op = op_for(&axes, "sup-axes");

    // 3. sup-axes over the rows exactly as the store projects them.
    let (written, log) = run(&reply(axes_op, select(&rows, &args_of(axes_op)), &params));
    apply(&mut rows, &written);
    Night {
        closed: rows[0].clone(),
        log,
    }
}

fn both_pages(rows: &[Value]) -> [(&'static str, Night); 2] {
    [
        ("normal page", night(rows.to_vec(), false)),
        ("full page", night(rows.to_vec(), true)),
    ]
}

#[test]
fn k1_a_peer_heard_successor_is_withdrawn_by_the_source_rule() {
    // The control: the rule of 3.5.0 (#849). If this is red, the source rule
    // itself is broken and nothing below means what it says.
    let rows = store_rows(
        "",
        Some(json!([AGENT, MEMBER])),
        PEER,
        Some(json!([AGENT, PEER])),
    );
    for (page, n) in both_pages(&rows) {
        assert!(
            n.withdrawn(),
            "{page}: a peer's word closed the member's own statement: {}",
            n.closed
        );
    }
}

#[test]
fn k2_a_private_successor_does_not_close_a_room_fact() {
    let rows = store_rows(
        "",
        Some(json!([AGENT, MEMBER, PEER])),
        "",
        Some(json!([AGENT, MEMBER])),
    );
    for (page, n) in both_pages(&rows) {
        assert!(
            n.withdrawn(),
            "{page}: a room fact stays closed by a private statement the room cannot see: {}",
            n.closed
        );
        assert!(
            n.log.contains("1 closure(s) across audiences withdrawn"),
            "{page}: the withdrawal leaves no count in the log: {}",
            n.log
        );
        for secret in [CLOSED_CLAIM, SUCCESSOR_CLAIM, MEMBER, PEER] {
            assert!(
                !n.log.contains(secret),
                "{page}: the log line carries content ({secret}): {}",
                n.log
            );
        }
    }
}

#[test]
fn k3_a_successor_heard_by_more_closes() {
    // Ruling OR-FD-D2: subset, not equality. A statement made before more
    // people may replace the private one -- the private audience can see it.
    // The order of the set is not part of it.
    let rows = store_rows(
        "",
        Some(json!([MEMBER, AGENT])),
        "",
        Some(json!([PEER, AGENT, MEMBER])),
    );
    for (page, n) in both_pages(&rows) {
        assert!(
            n.stands(),
            "{page}: a wider successor lost its closure: {}",
            n.closed
        );
        assert!(!n.log.contains("across audiences"), "{page}: {}", n.log);
    }
}

#[test]
fn k4_a_universal_successor_closes() {
    let rows = store_rows("", Some(json!([AGENT, MEMBER])), "", Some(json!(["*"])));
    for (page, n) in both_pages(&rows) {
        assert!(
            n.stands(),
            "{page}: a successor everyone may hear lost its closure: {}",
            n.closed
        );
    }
}

#[test]
fn k5_rows_without_an_audience_keep_todays_behaviour() {
    // Ruling OR-FD-D3: a row written before the audience column carries no
    // boundary. Three shapes: the closed row old, the successor old, both old;
    // plus an empty set, which is what a blanked column reads back as.
    let shapes = [
        ("closed NULL", None, Some(json!([AGENT, MEMBER]))),
        (
            "closed empty",
            Some(json!([])),
            Some(json!([AGENT, MEMBER])),
        ),
        ("successor NULL", Some(json!([AGENT, MEMBER, PEER])), None),
        ("both NULL", None, None),
    ];
    for (shape, closed, succ) in shapes {
        let rows = store_rows("", closed, "", succ);
        for (page, n) in both_pages(&rows) {
            assert!(
                n.stands(),
                "{shape}, {page}: an old row lost its closure: {}",
                n.closed
            );
        }
    }
}

/// One statement said again and again: the same claim on every row, one row a
/// day from f1 on, each with its own audience.
fn reassertions(audiences: &[Value]) -> Vec<Value> {
    let mut rows = Vec::new();
    for (n, audience) in audiences.iter().enumerate() {
        let id = format!("f{}", n + 1);
        let from = format!("2026-09-{:02}T10:00:00Z", 10 + n);
        rows.push(
            json!({"id": id, "subject": "user", "canonical_subject": "user",
            "predicate": "meets_team_on", "canonical_predicate": "meets_team_on",
            "claim": CLOSED_CLAIM, "canonical_claim": CLOSED_CLAIM,
            "valid_from": from, "valid_until": null, "recorded_at": from,
            "expired_at": null, "superseded_by": null, "closure_source": "",
            "session_id": format!("s-{id}"), "source": "",
            "audience_set": audience.to_string()}),
        );
    }
    rows
}

#[test]
fn k6_a_private_reassertion_does_not_close_a_room_fact() {
    // Review I-1, orchestrator ruling OR-FD-62: the night closes on a second
    // path besides the judge -- the same statement said again later closes the
    // earlier assertion (`next_reassertion`, same `statement_key`). The key
    // carries the source (#849) but not the audience, so the room's words said
    // again in private closed the room's fact on the private word. Both with
    // and without a judge's closure: a judged closure withdrawn across the
    // audience falls back to exactly this arithmetic.
    //
    // The full page without a judge is not a case: it materialises no
    // re-assertion and no judge closes, so no closure is ever written there and
    // the variant held on the old script too (review D-NR-M2). The full page
    // WITH a judge is the withdrawal of a judged closure (49682c708), kept as
    // the control that this fix left it standing.
    let room_then_private = reassertions(&[json!([AGENT, MEMBER, PEER]), json!([AGENT, MEMBER])]);
    for (judged, page, full) in [
        (false, "normal page", false),
        (true, "normal page", false),
        (true, "full page", true),
    ] {
        let n = night_judged(room_then_private.clone(), full, judged);
        assert!(
            n.withdrawn(),
            "judged {judged}, {page}: a room fact stays closed by its private re-assertion: {}",
            n.closed
        );
        for secret in [CLOSED_CLAIM, MEMBER, PEER] {
            assert!(
                !n.log.contains(secret),
                "judged {judged}, {page}: the log line carries content ({secret}): {}",
                n.log
            );
        }
    }

    // The re-assertion itself is not what this changes: a private statement
    // said again before the room is closed by it, as before (the full page
    // materialises no re-assertion at all, so only the normal page shows it).
    let private_then_room = reassertions(&[json!([AGENT, MEMBER]), json!([AGENT, MEMBER, PEER])]);
    let n = night_judged(private_then_room, false, false);
    assert!(
        n.closed["superseded_by"] == "f2" && n.closed["expired_at"].is_string(),
        "a wider re-assertion no longer closes the private one: {}",
        n.closed
    );

    // And the room's words said in private and then in the room again: the
    // room's fact is closed by the next assertion the room can hear, f3.
    let room_private_room = reassertions(&[
        json!([AGENT, MEMBER, PEER]),
        json!([AGENT, MEMBER]),
        json!([AGENT, MEMBER, PEER]),
    ]);
    let n = night_judged(room_private_room, false, false);
    assert!(
        n.closed["superseded_by"] == "f3" && n.closed["expired_at"] == "2026-09-12T10:00:00Z",
        "the room fact is not closed by the room's own later assertion: {}",
        n.closed
    );
}

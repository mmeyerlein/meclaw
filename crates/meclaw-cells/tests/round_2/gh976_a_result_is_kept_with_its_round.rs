//! A result is kept with its round, and read back only by that round (GH #976,
//! PE-DP-9 / D1 review E M-5).
//!
//! The residents' road of GH #965 shows a hive's last results on a screen. A
//! screen belongs to a round (`audience_set`), so a kept result has to carry the
//! round of the run that made it, and a read has to return only the results
//! whose round covers the asking round. Two shipped hives keep results this way:
//!
//! * `daily-digest`: `./shelf` writes every formatted digest into `./store`
//!   (`digests {id, at, audience_set, title, items}`) with the round of its run
//!   (`context.digest_round`), and answers `in_read` (op `last`).
//! * `research-assistant`: `./archive` writes `question_id`, `at` and
//!   `audience_set` beside every answer into `./memory`, and `./shelf` answers
//!   `in_read` (op `last`) from table `answers`.
//!
//! What these locks hold, per script:
//!
//! 1. The round is kept canonical (sorted, deduplicated JSON text); a run
//!    without a round, or with a malformed one, is kept with `""`. `["*"]`
//!    stays `["*"]`, and `*` is never added where it was absent.
//! 2. A read without a round reads no row: no store call, one `answer` with
//!    `{ok: false, error: {code: no_round}}`.
//! 3. A read answers only the rows whose round covers the asking round; a row
//!    with an empty or missing round covers nothing.
//! 4. The answer leaves on route `answer` with `op_id` mirrored from the
//!    asking context.
//!
//! Every test runs the SHIPPED `script_inline`, read from the template config at
//! test time, under python3 -- never a copy of it.

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

const DIGEST_SHELF: &str = "templates/daily-digest/shelf/config.json";
const RESEARCH_ARCHIVE: &str = "templates/research-assistant/archive/config.json";
const RESEARCH_SHELF: &str = "templates/research-assistant/shelf/config.json";

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The shipped script, or `None` where the template library does not travel
/// (the published tree, R2b / GH #49): the lock then skips instead of failing.
fn script_of(rel: &str) -> Option<String> {
    let raw = std::fs::read_to_string(repo(rel)).ok()?;
    let v: Value = serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{rel}: {e}"));
    let script = v["params"]["script_inline"]
        .as_str()
        .unwrap_or_else(|| panic!("{rel}: no params.script_inline"));
    Some(meclaw_testing::resolve_script_vars(script))
}

/// Run one script over one stdin document, the script handed to python3 on
/// stdin (GH #279: argv is capped at 128 KiB).
fn run(script: &str, body: Value, hop: Value, context: Value) -> Vec<Value> {
    let doc = json!({
        "body": body,
        "envelope": {"header": {"hop": hop, "context": context}},
        "params": {}
    });
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        serde_json::to_string(script).unwrap(),
        serde_json::to_string(&doc.to_string()).unwrap(),
    );
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3 runs");
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write the program");
    drop(sink);
    let out = child.wait_with_output().expect("wait for python3");
    assert!(
        out.status.success(),
        "the script exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "the script did not answer JSON ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    match v {
        Value::Array(list) => list,
        other => vec![other],
    }
}

fn have_python() -> bool {
    Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// The arguments of the one store tool_call an emission list carries.
fn the_store_call(out: &[Value], route: &str) -> Value {
    assert_eq!(out.len(), 1, "exactly one emission expected: {out:?}");
    assert_eq!(
        out[0]["header"]["route"], route,
        "the store call leaves on route {route}: {out:?}"
    );
    let turn = &out[0]["messages"][0];
    assert_eq!(
        turn["type"], "tool_call",
        "a store call is a tool_call: {out:?}"
    );
    serde_json::from_str(turn["text"].as_str().expect("tool_call text"))
        .expect("the tool_call text is JSON")
}

/// The one `answer` an emission list carries; no store call beside it.
fn the_answer(out: &[Value]) -> &Value {
    assert_eq!(out.len(), 1, "exactly one answer expected: {out:?}");
    assert!(
        !out.iter().any(|m| m["header"]["route"] == "shelf_store"),
        "an answer goes out alone, never beside a store call: {out:?}"
    );
    assert_eq!(out[0]["header"]["route"], "answer", "route answer: {out:?}");
    &out[0]
}

fn store_reply(rows: Value) -> Value {
    json!({"messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": rows.to_string()}]})
}

fn formatted_digest() -> Value {
    json!({"messages": [{"origin": "assistant", "type": "text",
                          "text": "Daily digest:\nFirst thing happened. Second thing too!"}]})
}

/// The round a keep writes, for one `context.digest_round` (`None` = absent).
fn digest_round_kept(script: &str, round: Option<Value>) -> Value {
    let ctx = match round {
        Some(r) => json!({"digest_round": r}),
        None => json!({}),
    };
    let args = the_store_call(
        &run(script, formatted_digest(), json!({}), ctx),
        "shelf_store",
    );
    assert_eq!(args["operation"], "insert", "a keep is an insert: {args}");
    assert_eq!(
        args["table"], "digests",
        "a keep writes table digests: {args}"
    );
    args["row"].clone()
}

#[test]
fn a_digest_is_kept_with_its_round() {
    let Some(script) = script_of(DIGEST_SHELF) else {
        return;
    };
    if !have_python() {
        return;
    }
    // Canonical: sorted and deduplicated, whether the round came as JSON text or
    // as a list.
    let row = digest_round_kept(&script, Some(json!(r#"["r-b","r-a"]"#)));
    assert_eq!(row["audience_set"], r#"["r-a", "r-b"]"#, "{row}");
    let row = digest_round_kept(&script, Some(json!(["r-b", "r-a", "r-a"])));
    assert_eq!(row["audience_set"], r#"["r-a", "r-b"]"#, "{row}");

    // The rest of the row: a positive millisecond time, the title, the items
    // as a JSON text holding a list.
    assert!(
        row["at"].as_i64().is_some_and(|at| at > 0),
        "at is an int > 0: {row}"
    );
    assert_eq!(row["title"], "Daily digest", "{row}");
    assert!(
        row["id"].as_str().is_some_and(|id| !id.is_empty()),
        "every row has an id: {row}"
    );
    let items: Value = serde_json::from_str(row["items"].as_str().expect("items is text"))
        .expect("items is JSON text");
    assert_eq!(
        items,
        json!(["First thing happened.", "Second thing too!"]),
        "items is the digest as a list, the title cut off: {row}"
    );

    // No round, or a malformed one, is kept as "" -- never widened.
    for round in [
        None,
        Some(json!("not json")),
        Some(json!("")),
        Some(json!([])),
        Some(json!([""])),
        Some(json!({"r-a": true})),
    ] {
        let shown = format!("{round:?}");
        let row = digest_round_kept(&script, round);
        assert_eq!(
            row["audience_set"], "",
            "a run with round {shown} is kept with an empty round: {row}"
        );
    }

    // `*` stays where it was, and is never added where it was not.
    let row = digest_round_kept(&script, Some(json!(["*"])));
    assert_eq!(row["audience_set"], r#"["*"]"#, "{row}");
    let row = digest_round_kept(&script, Some(json!(["r-a"])));
    assert_eq!(row["audience_set"], r#"["r-a"]"#, "{row}");
    assert!(
        !row["audience_set"].as_str().unwrap_or("").contains('*'),
        "a round is never widened to *: {row}"
    );
}

#[test]
fn an_insert_receipt_is_swallowed() {
    let Some(script) = script_of(DIGEST_SHELF) else {
        return;
    };
    if !have_python() {
        return;
    }
    // The store's receipt of a kept digest, as it arrives with the run's context.
    let out = run(
        &script,
        store_reply(json!({"rows_affected": 1})),
        json!({"operation": "insert", "rows_affected": 1}),
        json!({"digest_round": r#"["r-a"]"#}),
    );
    assert!(
        out.is_empty(),
        "an insert receipt is swallowed, never echoed: {out:?}"
    );
}

#[test]
fn a_read_without_a_round_reads_no_row() {
    let Some(script) = script_of(DIGEST_SHELF) else {
        return;
    };
    if !have_python() {
        return;
    }
    for ctx in [
        json!({"shelf_origin": "read", "shelf_op_id": "res:t1"}),
        json!({"shelf_origin": "read", "shelf_op_id": "res:t1", "audience_set": ""}),
        json!({"shelf_origin": "read", "shelf_op_id": "res:t1", "audience_set": "[]"}),
    ] {
        let out = run(
            &script,
            json!({"messages": []}),
            json!({"route": "in_read", "op": "last", "op_id": "res:t1"}),
            ctx.clone(),
        );
        assert!(
            !out.iter().any(|m| m["messages"]
                .as_array()
                .is_some_and(|ms| ms.iter().any(|t| t["type"] == "tool_call"))),
            "a read without a round calls no store ({ctx}): {out:?}"
        );
        let a = the_answer(&out);
        assert_eq!(a["header"]["op_id"], "res:t1", "op_id mirrored: {a}");
        assert_eq!(a["header"]["op"], "last", "{a}");
        assert_eq!(a["ok"], false, "{a}");
        assert_eq!(a["error"]["code"], "no_round", "{a}");
    }
}

/// Rows of four rounds, newest first as the store returns them for
/// `order_by at desc`.
fn digest_rows() -> Value {
    json!([
        {"id": "d-ab", "at": 4000, "audience_set": r#"["r-a", "r-b"]"#,
         "title": "Daily digest", "items": r#"["Lead of ab.", "More."]"#},
        {"id": "d-c", "at": 3000, "audience_set": r#"["r-c"]"#,
         "title": "Daily digest", "items": r#"["Lead of c."]"#},
        {"id": "d-empty", "at": 2500, "audience_set": "",
         "title": "Daily digest", "items": r#"["Lead of nobody."]"#},
        {"id": "d-null", "at": 2000, "audience_set": null,
         "title": "Daily digest", "items": r#"["Lead of null."]"#},
        {"id": "d-star", "at": 1000, "audience_set": r#"["*"]"#,
         "title": "Daily digest", "items": r#"["Lead of all."]"#}
    ])
}

fn ids(list: &Value) -> Vec<String> {
    list.as_array()
        .expect("a list")
        .iter()
        .map(|d| d["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[test]
fn a_read_answers_only_the_rows_of_its_round() {
    let Some(script) = script_of(DIGEST_SHELF) else {
        return;
    };
    if !have_python() {
        return;
    }
    // A read WITH a round asks the store, newest first.
    let args = the_store_call(
        &run(
            &script,
            json!({"messages": []}),
            json!({"route": "in_read", "op": "last", "op_id": "res:t1"}),
            json!({"shelf_origin": "read", "shelf_op_id": "res:t1",
                   "audience_set": r#"["r-a"]"#}),
        ),
        "shelf_store",
    );
    assert_eq!(args["operation"], "select", "{args}");
    assert_eq!(args["table"], "digests", "{args}");
    assert_eq!(
        args["order_by"],
        json!([{"col": "at", "dir": "desc"}]),
        "{args}"
    );

    // The store's reply: only the covering rows, in the store's order.
    for (asking, want) in [
        (json!(r#"["r-a"]"#), vec!["d-ab", "d-star"]),
        (json!(["r-b", "r-a"]), vec!["d-ab", "d-star"]),
        (json!(["r-c"]), vec!["d-c", "d-star"]),
        (json!(["r-a", "r-c"]), vec!["d-star"]),
        (json!(["r-z"]), vec!["d-star"]),
    ] {
        let out = run(
            &script,
            store_reply(digest_rows()),
            json!({"operation": "select", "rows_affected": 5}),
            json!({"shelf_origin": "read", "shelf_op_id": "res:t2", "audience_set": asking}),
        );
        let a = the_answer(&out);
        assert_eq!(a["header"]["op_id"], "res:t2", "op_id mirrored: {a}");
        assert_eq!(a["header"]["op"], "last", "{a}");
        assert_eq!(a["ok"], true, "{a}");
        assert_eq!(ids(&a["digests"]), want, "round {asking}: {a}");
        for d in a["digests"].as_array().unwrap() {
            assert!(d["at"].as_i64().is_some_and(|at| at > 0), "{d}");
            assert!(d["when"].as_str().is_some_and(|w| !w.is_empty()), "{d}");
            assert_eq!(d["lead"], d["items"][0], "the lead is the first item: {d}");
            assert!(
                d["audience_set"].is_array(),
                "each digest carries its round: {d}"
            );
        }
    }

    // The store's row shape `{rows: [...]}` reads the same.
    let out = run(
        &script,
        store_reply(json!({"rows": digest_rows()})),
        json!({"operation": "select"}),
        json!({"shelf_origin": "read", "shelf_op_id": "res:t3", "audience_set": ["r-c"]}),
    );
    assert_eq!(ids(&the_answer(&out)["digests"]), vec!["d-c", "d-star"]);
}

#[test]
fn an_unknown_op_is_refused() {
    let Some(script) = script_of(DIGEST_SHELF) else {
        return;
    };
    if !have_python() {
        return;
    }
    let out = run(
        &script,
        json!({"messages": []}),
        json!({"route": "in_read", "op": "all", "op_id": "res:t9"}),
        json!({"shelf_origin": "read", "shelf_op_id": "res:t9", "audience_set": ["r-a"]}),
    );
    let a = the_answer(&out);
    assert_eq!(a["ok"], false, "{a}");
    assert_eq!(a["error"]["code"], "unknown_op", "{a}");
    assert_eq!(a["header"]["op_id"], "res:t9", "{a}");
}

/// The row `./archive` writes for one final planner answer.
fn research_row(script: &str, context: Value) -> Value {
    let out = run(
        script,
        json!({"messages": [{"origin": "assistant", "type": "text",
                              "text": "The answer, short and kept."}]}),
        json!({"finish_reason": "stop"}),
        context,
    );
    assert_eq!(out.len(), 1, "one insert: {out:?}");
    let args: Value = serde_json::from_str(
        out[0]["messages"][0]["text"]
            .as_str()
            .expect("tool_call text"),
    )
    .expect("tool_call JSON");
    assert_eq!(args["operation"], "insert", "{args}");
    assert_eq!(args["table"], "answers", "{args}");
    args["row"].clone()
}

#[test]
fn a_research_answer_carries_round_and_time() {
    let Some(script) = script_of(RESEARCH_ARCHIVE) else {
        return;
    };
    if !have_python() {
        return;
    }
    let row = research_row(
        &script,
        json!({"question": "What is new?", "turn_id": "turn-7",
               "audience_set": ["r-b", "r-a"]}),
    );
    assert_eq!(
        row["question_id"], "turn-7",
        "question_id is the turn id: {row}"
    );
    assert_eq!(row["question"], "What is new?", "{row}");
    assert_eq!(row["answer"], "The answer, short and kept.", "{row}");
    assert!(
        row["at"].as_i64().is_some_and(|at| at > 0),
        "at is an int > 0: {row}"
    );
    assert_eq!(
        row["audience_set"], r#"["r-a", "r-b"]"#,
        "canonical round: {row}"
    );

    let row = research_row(
        &script,
        json!({"question": "What is new?", "turn_id": "turn-8"}),
    );
    assert_eq!(row["audience_set"], "", "no round -> empty round: {row}");
    assert_eq!(row["question_id"], "turn-8", "{row}");

    let row = research_row(
        &script,
        json!({"question": "q", "turn_id": "turn-9", "audience_set": "not json"}),
    );
    assert_eq!(
        row["audience_set"], "",
        "malformed round -> empty round: {row}"
    );

    // The store's receipt coming back is swallowed.
    let out = run(
        &script,
        store_reply(json!({"rows_affected": 1})),
        json!({"operation": "insert"}),
        json!({}),
    );
    assert!(out.is_empty(), "an insert receipt is swallowed: {out:?}");
}

#[test]
fn the_research_shelf_answers_only_the_rows_of_its_round() {
    let Some(script) = script_of(RESEARCH_SHELF) else {
        return;
    };
    if !have_python() {
        return;
    }
    // Without a round: no store call, `no_round`.
    let out = run(
        &script,
        json!({"messages": []}),
        json!({"route": "in_read", "op": "last", "op_id": "res:t1"}),
        json!({"store_origin": "shelf", "shelf_op_id": "res:t1"}),
    );
    let a = the_answer(&out);
    assert_eq!(a["error"]["code"], "no_round", "{a}");
    assert_eq!(a["header"]["op_id"], "res:t1", "{a}");

    // With a round: a select on `answers`, newest first.
    let args = the_store_call(
        &run(
            &script,
            json!({"messages": []}),
            json!({"route": "in_read", "op": "last", "op_id": "res:t2"}),
            json!({"store_origin": "shelf", "shelf_op_id": "res:t2", "audience_set": ["r-a"]}),
        ),
        "shelf_store",
    );
    assert_eq!(args["operation"], "select", "{args}");
    assert_eq!(args["table"], "answers", "{args}");
    assert_eq!(
        args["order_by"],
        json!([{"col": "at", "dir": "desc"}]),
        "{args}"
    );

    // An unknown op is refused.
    let out = run(
        &script,
        json!({"messages": []}),
        json!({"route": "in_read", "op": "all"}),
        json!({"store_origin": "shelf", "shelf_op_id": "res:t3", "audience_set": ["r-a"]}),
    );
    assert_eq!(the_answer(&out)["error"]["code"], "unknown_op");

    // The store's reply: only the covering rows.
    let rows = json!([
        {"id": "a-1", "question_id": "turn-1", "question": "Q1", "answer": "A1",
         "at": 4000, "audience_set": r#"["r-a"]"#},
        {"id": "a-2", "question_id": "turn-2", "question": "Q2", "answer": "A2",
         "at": 3000, "audience_set": r#"["r-b"]"#},
        {"id": "a-3", "question_id": "turn-3", "question": "Q3", "answer": "A3",
         "at": 2000, "audience_set": ""},
        {"id": "a-4", "question_id": "turn-4", "question": "Q4", "answer": "A4",
         "at": 1000, "audience_set": null}
    ]);
    for (asking, want) in [
        (json!(["r-a"]), vec!["a-1"]),
        (json!(r#"["r-b"]"#), vec!["a-2"]),
        (json!(["r-z"]), vec![]),
    ] {
        let out = run(
            &script,
            store_reply(rows.clone()),
            json!({"operation": "select"}),
            json!({"store_origin": "shelf", "shelf_op_id": "res:t4", "audience_set": asking}),
        );
        let a = the_answer(&out);
        assert_eq!(a["header"]["op_id"], "res:t4", "{a}");
        assert_eq!(a["ok"], true, "{a}");
        let got = ids(&a["answers"]);
        assert_eq!(got, want, "round {asking}: {a}");
        if let Some(first) = a["answers"].as_array().and_then(|l| l.first())
            && first["id"] == "a-1"
        {
            assert_eq!(first["question_id"], "turn-1", "{a}");
            assert_eq!(first["answer"], "A1", "{a}");
            assert_eq!(first["at"], 4000, "{a}");
            assert_eq!(first["audience_set"], json!(["r-a"]), "{a}");
        }
    }
}

const DIGEST_HIVE: &str = "templates/daily-digest/config.json";

/// The row a keep writes for one full run context.
fn digest_row_of(script: &str, ctx: Value) -> Value {
    let args = the_store_call(
        &run(script, formatted_digest(), json!({}), ctx),
        "shelf_store",
    );
    assert_eq!(args["operation"], "insert", "a keep is an insert: {args}");
    args["row"].clone()
}

#[test]
fn a_scheduled_digest_is_kept_for_its_holder() {
    let Some(script) = script_of(DIGEST_SHELF) else {
        return;
    };
    if !have_python() {
        return;
    }
    // A scheduled run (the clock's edge writes `digest_origin` 'schedule' and
    // overwrites any caller value) is kept with the holder mark, whatever
    // `digest_round` says -- the round is the holding member's, never a chosen one.
    for round in [
        None,
        Some(json!(["r-a"])),
        Some(json!(["*"])),
        Some(json!("holder")),
        Some(json!("not json")),
    ] {
        let shown = format!("{round:?}");
        let mut ctx = json!({"digest_origin": "schedule"});
        if let Some(r) = round {
            ctx["digest_round"] = r;
        }
        let row = digest_row_of(&script, ctx);
        assert_eq!(
            row["audience_set"], "holder",
            "a scheduled run (digest_round {shown}) is kept for its holder (OR-NL-158): {row}"
        );
    }

    // A demanded run (the `. -> ./fetcher` edge writes 'parent') that forges the
    // holder mark as its round is NOT the holder's: it is no round, kept as "".
    for forged in [json!("holder"), json!(["holder"])] {
        let row = digest_row_of(
            &script,
            json!({"digest_origin": "parent", "digest_round": forged.clone()}),
        );
        let want = if forged.is_array() {
            json!(r#"["holder"]"#)
        } else {
            json!("")
        };
        assert_eq!(
            row["audience_set"], want,
            "a demanded run's digest_round {forged} is never the holder mark: {row}"
        );
        assert_ne!(row["audience_set"], "holder", "{row}");
    }
    // A demanded run with a real round keeps it, as before.
    let row = digest_row_of(
        &script,
        json!({"digest_origin": "parent", "digest_round": ["r-b", "r-a"]}),
    );
    assert_eq!(row["audience_set"], r#"["r-a", "r-b"]"#, "{row}");
}

#[test]
fn a_holders_digest_is_read_under_the_round_of_the_read() {
    let Some(script) = script_of(DIGEST_SHELF) else {
        return;
    };
    if !have_python() {
        return;
    }
    let rows = json!([
        {"id": "d-holder", "at": 5000, "audience_set": "holder",
         "title": "Daily digest", "items": r#"["Lead of the holder."]"#},
        {"id": "d-c", "at": 3000, "audience_set": r#"["r-c"]"#,
         "title": "Daily digest", "items": r#"["Lead of c."]"#}
    ]);
    // The asking round (the member's, stamped by the builder's read edge) takes
    // the holder's digest, and it is answered with that round as its own.
    for (asking, want_round) in [
        (json!(["r-b", "r-a"]), json!(["r-a", "r-b"])),
        (json!(r#"["r-c"]"#), json!(["r-c"])),
    ] {
        let out = run(
            &script,
            store_reply(rows.clone()),
            json!({"operation": "select"}),
            json!({"shelf_origin": "read", "shelf_op_id": "res:h1", "audience_set": asking}),
        );
        let a = the_answer(&out);
        assert_eq!(a["ok"], true, "{a}");
        let got = ids(&a["digests"]);
        assert_eq!(got[0], "d-holder", "the holder's digest is read: {a}");
        assert_eq!(
            a["digests"][0]["audience_set"], want_round,
            "the holder's digest answers with the round of the read ({asking}): {a}"
        );
        assert_eq!(a["digests"][0]["lead"], "Lead of the holder.", "{a}");
        if want_round == json!(["r-c"]) {
            assert_eq!(got, vec!["d-holder", "d-c"], "{a}");
        } else {
            assert_eq!(got, vec!["d-holder"], "{a}");
        }
    }

    // Without an asking round nothing is read, holder or not: `no_round`.
    let out = run(
        &script,
        json!({"messages": []}),
        json!({"route": "in_read", "op": "last", "op_id": "res:h2"}),
        json!({"shelf_origin": "read", "shelf_op_id": "res:h2"}),
    );
    assert_eq!(the_answer(&out)["error"]["code"], "no_round");
    // A store reply reaching a read without a round answers no holder row either.
    let out = run(
        &script,
        store_reply(rows),
        json!({"operation": "select"}),
        json!({"shelf_origin": "read", "shelf_op_id": "res:h3"}),
    );
    let a = the_answer(&out);
    assert!(
        ids(&a["digests"]).is_empty(),
        "no asking round -> no holder row: {a}"
    );
}

#[test]
fn the_clock_marks_a_scheduled_run_and_writes_no_round() {
    let Ok(raw) = std::fs::read_to_string(repo(DIGEST_HIVE)) else {
        return;
    };
    let v: Value = serde_json::from_str(&raw).expect("hive config JSON");
    let edges = v["params"]["graph"]["edges"]
        .as_array()
        .expect("params.graph.edges");
    let clock: Vec<&Value> = edges
        .iter()
        .filter(|e| e["from"] == "./clock" && e["to"] == "./fetcher")
        .collect();
    assert_eq!(clock.len(), 1, "one clock edge: {clock:?}");
    let set = &clock[0]["modifier"]["set_context"];
    assert_eq!(
        set["digest_origin"], "'schedule'",
        "the clock's edge marks a scheduled run: {set}"
    );
    assert!(
        set.get("digest_round").is_none(),
        "the clock's edge writes no digest_round -- the holder's round is the read's (OR-NL-158): {set}"
    );
}

/// Y fix round 1, M-3: the edge a caller enters by drops the hive's own read marks from
/// the caller's context. A context that already carries `shelf_origin 'read'` (or the
/// research hive's `store_origin`) would otherwise steer the finished result into the
/// read branch -- a `store_error` answer instead of a kept result.
#[test]
fn the_entry_edges_drop_a_callers_shelf_marks() {
    for (hive, route, marks) in [
        (DIGEST_HIVE, "in_digest", ["shelf_origin", "shelf_op_id"]),
        (
            "templates/research-assistant/config.json",
            "in_turn",
            ["store_origin", "shelf_op_id"],
        ),
    ] {
        let Ok(raw) = std::fs::read_to_string(repo(hive)) else {
            return;
        };
        let v: Value = serde_json::from_str(&raw).expect("hive config JSON");
        let entry: Vec<&Value> = v["params"]["graph"]["edges"]
            .as_array()
            .expect("params.graph.edges")
            .iter()
            .filter(|e| {
                e["from"] == "."
                    && e["condition"]
                        .as_str()
                        .is_some_and(|c| c.contains(&format!("hop.route == '{route}'")))
            })
            .collect();
        assert_eq!(
            entry.len(),
            1,
            "{hive}: one entry edge on {route}: {entry:?}"
        );
        let deleted = entry[0]["modifier"]["delete_context"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for mark in marks {
            assert!(
                deleted.contains(&json!(mark)),
                "{hive}: the {route} edge deletes the caller's `{mark}`: {}",
                entry[0]
            );
        }
    }
}

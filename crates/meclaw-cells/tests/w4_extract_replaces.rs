//! Statement identity W4 -- the extractor closes, in the turn, what it can SEE
//! (GitHub #13, ruling Q2 option C with its own guard rail).
//!
//! W3 gave the store its producer: the nightly judge reads the axes of a memory
//! with the whole store in front of it and closes what was replaced. That is the
//! load-bearing mechanism and it runs at 03:00Z. W4 is the other half of the same
//! ruling -- the north star of this memory is "resolve the conflict IN THE TURN",
//! and the extractor is the only party that is present in the turn.
//!
//! So the extraction lane gains three things, and exactly three:
//!
//! 1. its vocabulary window (P1: the axes this memory already carries) now also
//!    carries the OPEN STATEMENTS of those axes, hard budgeted, because an
//!    extractor that cannot see a value cannot replace it;
//! 2. a fact may carry `replaces`, naming a statement FROM THAT WINDOW;
//! 3. a valid `replaces` becomes exactly the W3 write form -- `expired_at`,
//!    `superseded_by`, `closure_source` -- attributed to the batch.
//!
//! The guard rail is ruling Q2 rail 3 and it is mechanical, not advisory: a
//! `replaces` naming anything the window did not contain is discarded and logged.
//! The extractor may only close what it was provably shown. The night validates
//! the rest -- it sees recently extract-closed statements on its axis pages and
//! may contradict them, which is a revert in the W3 shape.
//!
//! Everything here runs the REAL `params.script_inline` of `extract-glue` and
//! `dream-glue` against injected store replies, so no model is called and nothing
//! costs anything. Whether an extractor USES `replaces` well is a model property
//! and belongs to the paid scenario C15; the free scenario C14 proves the chain
//! end to end on a real colony with the extractor's answer handed in.

use std::io::Write;
use std::process::{Command, Stdio};

const EXTRACT_CONFIG: &str = "../../templates/memory-hive/extract-glue/config.json";
const DREAM_CONFIG: &str = "../../templates/memory-hive/dream-glue/config.json";

/// `${VAR:-default}` becomes the default, a bare `${VAR}` becomes the empty string --
/// the same substitution the colony performs when it instantiates the template.
fn resolve_vars(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    let mut rest = script;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail
            .find('}')
            .expect("unterminated ${...} in script_inline");
        if let Some((_, default)) = tail[..end].split_once(":-") {
            out.push_str(default);
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

fn script_of(path: &str) -> String {
    let raw = std::fs::read_to_string(path).expect("template config");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("config json");
    resolve_vars(
        v["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
    )
}

/// Run a real script with a real stdin document; returns (messages, stderr).
fn run(script: &str, doc: serde_json::Value) -> (Vec<serde_json::Value>, String) {
    let out = run_script_on_stdin(script, &meclaw_testing::code_stdin(&doc).to_string());
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "script exited non-zero: {err}");
    let msgs = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    (msgs, err)
}

fn emit_x(doc: serde_json::Value) -> Vec<serde_json::Value> {
    run(&script_of(EXTRACT_CONFIG), doc).0
}

const BATCH: &str = "b1";

/// One store reply of the extraction lane as the edge delivers it.
fn x_reply(phase: &str, operation: &str, rows: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"store_origin": "extract", "mem_phase": phase, "batch_id": BATCH},
            "hop": {"operation": operation, "rows_affected": 1}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r", "text": rows.to_string()}]
    })
}

fn args_of(msg: &serde_json::Value) -> serde_json::Value {
    let text = msg["messages"][0]["text"].as_str().expect("op text");
    serde_json::from_str(text).expect("op args")
}

/// The op the lane parks under one scratch kind, if it emitted one.
fn parked(msgs: &[serde_json::Value], kind: &str) -> Option<serde_json::Value> {
    msgs.iter()
        .map(args_of)
        .find(|a| a["table"] == "scratch" && a["row"]["kind"] == kind)
        .map(|a| {
            serde_json::from_str(a["row"]["payload"].as_str().expect("payload"))
                .expect("parked payload")
        })
}

/// One fact row in the shape the vocabulary leg projects it.
fn fact_row(
    id: &str,
    subject: &str,
    predicate: &str,
    claim: &str,
    from: &str,
) -> serde_json::Value {
    serde_json::json!({
        "id": id, "subject": subject, "canonical_subject": subject,
        "predicate": predicate, "canonical_predicate": predicate,
        "claim": claim, "canonical_claim": claim,
        "valid_from": from, "recorded_at": from, "expired_at": null
    })
}

/// A memory with one single-valued axis, one enumeration and one closed value.
fn vocab_rows() -> serde_json::Value {
    serde_json::json!([
        fact_row("f1", "user", "favorite_editor", "favorite editor is helix", "2026-03-05T00:00:00Z"),
        fact_row("f2", "user", "has_child", "has a child named ada", "2026-02-01T00:00:00Z"),
        fact_row("f3", "user", "has_child", "has a child named ben", "2026-04-01T00:00:00Z"),
        {"id": "f4", "subject": "user", "canonical_subject": "user",
         "predicate": "lives_in", "canonical_predicate": "lives_in",
         "claim": "lives in zone-a", "canonical_claim": "lives in zone-a",
         "valid_from": "2026-01-06T00:00:00Z", "recorded_at": "2026-01-06T00:00:00Z",
         "expired_at": "2026-02-06T00:00:00Z"}
    ])
}

/// The cap of the window's own page, as the template declares it
/// (`MEMORY_EXTRACT_WINDOW_ROWS`). Pinned against the read below, so a changed
/// default cannot leave the boundary tests measuring a bound nobody applies.
const WINDOW_MAX_ROWS: usize = 256;
/// `WINDOW_POOL_AXES` of the template: twice the offered budget since GH #67, so
/// the subject-matter selection at the prompt phase has something to choose from.
const WINDOW_POOL_AXES: usize = 16;

/// One row of the recency page, in the shape its narrow projection has.
fn page_row(subject: &str, predicate: &str, from: &str) -> serde_json::Value {
    serde_json::json!({
        "subject": subject, "canonical_subject": subject,
        "predicate": predicate, "canonical_predicate": predicate,
        "valid_from": from
    })
}

#[test]
fn the_leg_opens_with_a_bounded_recency_page_over_the_open_facts() {
    // GH #68: the read that used to open this leg was a `select` over `facts` with
    // an empty `where`, no `limit` and no `distinct` -- the whole table travelled
    // the mailbox so that two consumers could fold it down to a few hundred bytes.
    // The leg now opens with the question the WINDOW actually asks: which axes did
    // this memory touch most recently, of the ones that are still open.
    let msgs = emit_x(x_reply("vocab-fetch", "insert", serde_json::json!([])));
    assert_eq!(msgs.len(), 1, "one read, not a fan-out");
    let args = args_of(&msgs[0]);
    assert_eq!(args["operation"], "select");
    assert_eq!(args["table"], "facts");
    assert_eq!(
        args["where"],
        serde_json::json!({"expired_at": {"is_null": true}}),
        "a closed statement cannot be closed again, so it never belongs on this page"
    );
    assert_eq!(
        args["order_by"][0],
        serde_json::json!({"col": "valid_from", "dir": "desc"}),
        "stated ordering: most recently asserted first -- a replacement lands on \
         the axis the conversation just touched"
    );
    assert!(
        args["limit"].as_i64().unwrap_or_default() >= 1,
        "the page has no bound: {args}"
    );
    assert_eq!(
        args["columns"],
        serde_json::json!([
            "subject",
            "canonical_subject",
            "predicate",
            "canonical_predicate",
            "valid_from"
        ]),
        "this page only PICKS axes, so it carries no claim, no id and no history \
         -- the rows it does carry stay cheap"
    );
}

/// A recency page holding two axes of one subject.
fn scan_page() -> serde_json::Value {
    serde_json::json!([
        page_row("user", "favorite_editor", "2026-03-05T00:00:00Z"),
        page_row("user", "has_child", "2026-02-01T00:00:00Z"),
        page_row("user", "has_child", "2026-04-01T00:00:00Z")
    ])
}

#[test]
fn the_window_reads_its_candidate_axes_whole() {
    // The paging (GH #68), and the reason it is paging rather than one capped
    // scan: rule 2 of the window is a COUNT ("an axis with more open statements
    // than one page is SKIPPED"), and a count can only be taken on a complete
    // axis. The recency page picks the axes, this read fetches them entirely --
    // a bucket seen through a cut-off scan looks like a short axis, and a
    // replacement on it deletes an answer that is true.
    let msgs = emit_x(x_reply("window-scan", "select", scan_page()));
    assert_eq!(msgs.len(), 1, "one read, not a fan-out");
    let args = args_of(&msgs[0]);
    assert_eq!(args["operation"], "select");
    assert_eq!(args["table"], "facts");
    assert_eq!(
        args["where"]["expired_at"],
        serde_json::json!({"is_null": true})
    );
    assert_eq!(
        args["where"]["canonical_subject"]["in"],
        serde_json::json!(["user"]),
        "the axes of the page, and nothing else, decide what this read costs"
    );
    assert_eq!(
        args["where"]["canonical_predicate"]["in"],
        serde_json::json!(["favorite_editor", "has_child"])
    );
    assert_eq!(
        args["order_by"],
        serde_json::json!([{"col": "canonical_subject"}, {"col": "canonical_predicate"},
                           {"col": "valid_from"}, {"col": "id"}]),
        "stated ordering: axis-major, so a full page has exactly ONE axis it \
         cannot prove complete -- its last"
    );
    assert_eq!(
        args["limit"].as_u64().unwrap_or_default() as usize,
        WINDOW_MAX_ROWS,
        "the second page is bounded too"
    );
    let cols = args["columns"].as_array().expect("columns").clone();
    for needed in [
        "id",
        "claim",
        "canonical_claim",
        "valid_from",
        "recorded_at",
    ] {
        assert!(
            cols.iter().any(|c| c == needed),
            "the replacement window needs {needed}, got {}",
            args["columns"]
        );
    }
}

#[test]
fn a_memory_without_an_open_statement_pages_nothing_and_still_hints() {
    // No open fact, no window -- and the leg must not stall there: the axis hint
    // is independent of the window and the batch is waiting behind it.
    let msgs = emit_x(x_reply("window-scan", "select", serde_json::json!([])));
    assert_eq!(msgs.len(), 1);
    assert_eq!(
        args_of(&msgs[0])["distinct"],
        serde_json::json!(true),
        "the leg went on with the axis hint: {msgs:?}"
    );
}

#[test]
fn the_axis_hint_is_deduplicated_at_the_store_and_capped() {
    // The other half of #68, and the one the issue is named after. The hint asks
    // a SET question -- which axes exist -- so the dedup happens where the rows
    // are and the cap counts axes instead of facts.
    let msgs = emit_x(x_reply("window-page", "select", vocab_rows()));
    let read = msgs
        .iter()
        .map(args_of)
        .find(|a| a["operation"] == "select")
        .expect("the axis hint is read");
    assert_eq!(read["table"], "facts");
    assert_eq!(
        read["distinct"],
        serde_json::json!(true),
        "an undeduplicated hint read hands the whole memory to the mailbox to \
         produce a list of axes: {read}"
    );
    assert_eq!(
        read["where"],
        serde_json::json!({}),
        "the AXIS hint still reads every row: an axis whose values are all closed \
         is still the axis a new value belongs on"
    );
    assert_eq!(
        read["columns"],
        serde_json::json!([
            "subject",
            "canonical_subject",
            "predicate",
            "canonical_predicate"
        ]),
        "and only the four columns an axis IS -- the dedup is over exactly them"
    );
    assert_eq!(
        read["order_by"],
        serde_json::json!([{"col": "canonical_subject"}, {"col": "canonical_predicate"},
                           {"col": "subject"}, {"col": "predicate"}]),
        "stated ordering, subject-major: the cap then cuts whole subjects off the \
         tail instead of half an axis list out of the middle"
    );
    assert!(
        read["limit"].as_i64().unwrap_or_default() >= 1,
        "the hint read has no bound: {read}"
    );
}

#[test]
fn every_read_of_the_leg_states_an_order_and_a_bound() {
    // The pin of GH #68 over the whole leg rather than per read: whatever the
    // vocabulary leg asks the store, the ANSWER is bounded -- because the answer
    // is what travels the mailbox. The one read with an empty `where` is the axis
    // hint, and it pays for that with a store-side dedup.
    for (phase, operation, rows) in [
        ("vocab-fetch", "insert", serde_json::json!([])),
        ("window-scan", "select", scan_page()),
        ("window-page", "select", vocab_rows()),
    ] {
        for op in emit_x(x_reply(phase, operation, rows)).iter().map(args_of) {
            if op["operation"] != "select" {
                continue;
            }
            assert!(
                op["limit"].as_i64().unwrap_or_default() >= 1,
                "phase {phase} reads without a bound: {op}"
            );
            assert!(
                !op["order_by"].as_array().map(Vec::is_empty).unwrap_or(true),
                "phase {phase} caps a read whose order is undefined: {op}"
            );
            if op["where"] == serde_json::json!({}) {
                assert_eq!(
                    op["distinct"],
                    serde_json::json!(true),
                    "phase {phase} scans every row of the table: {op}"
                );
            }
        }
    }
}

#[test]
fn the_window_is_parked_next_to_the_vocabulary_under_the_same_batch() {
    // A code cell holds no state and the window has to survive the model call, so
    // it is parked exactly the way the batch and the vocabulary are (DEC-009).
    // Under the SAME key, because the apply phase reads that key back as one
    // result set and the guard rail is checked there.
    let msgs = emit_x(x_reply("vocab", "select", vocab_rows()));
    let vocab = parked(&msgs, "vocab").expect("the vocabulary hint is still parked");
    assert_eq!(
        vocab["user"],
        serde_json::json!(["favorite_editor", "has_child", "lives_in"]),
        "the P1 hint is unchanged -- the reads moved, the payload did not"
    );
    let msgs = emit_x(x_reply("window-page", "select", vocab_rows()));
    // GH #67: the page parks the POOL. Which of these axes reaches the prompt is
    // decided one phase later, where the turns are -- the claim of this pin is
    // unchanged, it is about which axes the READ offers at all.
    let window = parked(&msgs, "window-pool").expect("the pool is parked");
    for op in msgs.iter().map(args_of) {
        if op["table"] == "scratch" {
            assert_eq!(op["row"]["key"], BATCH, "both halves park under the batch");
        }
    }
    let names: Vec<&str> = window
        .as_array()
        .expect("window is a list of axes")
        .iter()
        .map(|a| a["predicate"].as_str().unwrap_or_default())
        .collect();
    assert!(
        names.contains(&"favorite_editor") && names.contains(&"has_child"),
        "every axis with open statements is offered: refusing to replace an \
         enumeration is the EXTRACTOR's answer, not the lane's guess ({names:?})"
    );
    assert!(
        !names.contains(&"lives_in"),
        "a closed statement has its answer already and cannot be closed again \
         ({names:?})"
    );
}

#[test]
fn the_window_carries_the_id_the_value_and_the_instant() {
    let msgs = emit_x(x_reply("window-page", "select", vocab_rows()));
    let window = parked(&msgs, "window-pool").expect("pool");
    let axis = window
        .as_array()
        .expect("axes")
        .iter()
        .find(|a| a["predicate"] == "has_child")
        .cloned()
        .expect("the enumeration axis");
    assert_eq!(axis["subject"], "user");
    assert_eq!(
        axis["statements"],
        serde_json::json!([
            {"id": "f2", "claim": "has a child named ada", "since": "2026-02-01T00:00:00Z",
             "last_asserted": "2026-02-01T00:00:00Z"},
            {"id": "f3", "claim": "has a child named ben", "since": "2026-04-01T00:00:00Z",
             "last_asserted": "2026-04-01T00:00:00Z"}
        ]),
        "oldest first, one entry per STATEMENT, with the id a `replaces` has to name. \
         The second instant is GH #71: the apply phase compares it against the fact \
         that wants to replace the statement, and on a statement asserted once the \
         two are the same value"
    );
}

#[test]
fn two_assertions_of_one_value_are_one_statement_and_offer_the_live_one() {
    // W2's identity, one lane over: the extractor is shown VALUES, not rows, and
    // the id it may close is the assertion still standing. Offering the older one
    // would collide with the closure the chain re-derives by itself every night.
    let rows = serde_json::json!([
        fact_row(
            "y1",
            "user",
            "practices",
            "yoga twice a week",
            "2026-01-05T00:00:00Z"
        ),
        fact_row(
            "y2",
            "user",
            "practices",
            "yoga twice a week",
            "2026-05-05T00:00:00Z"
        )
    ]);
    let msgs = emit_x(x_reply("window-page", "select", rows));
    let window = parked(&msgs, "window-pool").expect("pool");
    assert_eq!(
        window[0]["statements"],
        serde_json::json!([
            {"id": "y2", "claim": "yoga twice a week", "since": "2026-01-05T00:00:00Z",
             "last_asserted": "2026-05-05T00:00:00Z"}
        ]),
        "one statement, named by its live assertion, dated by its first and -- since \
         GH #71 -- by its last: a statement still being asserted after the fact that \
         wants to replace it is not ended by it"
    );
}

#[test]
fn an_axis_longer_than_one_page_is_skipped_and_the_window_is_capped() {
    // The budget, and it is two rules rather than one. A page that is TOO LONG is
    // dropped whole (the W3 lesson: a producer that sees six of seventy plans
    // cannot tell it is a bucket, and a wrong closure there deletes an answer that
    // is true), and the number of axes is capped because this window is prompt
    // payload on the lane that runs while someone is talking.
    let mut rows = vec![];
    for i in 0..7 {
        rows.push(fact_row(
            &format!("p{i}"),
            "user",
            "planned_activity",
            &format!("plan number {i}"),
            &format!("2026-04-0{}T00:00:00Z", i + 1),
        ));
    }
    // ten axes of two statements each, so the cap has something to cut
    for i in 0..10 {
        for j in 0..2 {
            rows.push(fact_row(
                &format!("a{i}-{j}"),
                "user",
                &format!("axis_{i}"),
                &format!("value {i}-{j}"),
                &format!("2026-0{}-0{}T00:00:00Z", i % 9 + 1, j + 1),
            ));
        }
    }
    let msgs = emit_x(x_reply(
        "window-page",
        "select",
        serde_json::Value::Array(rows),
    ));
    let window = parked(&msgs, "window-pool").expect("pool");
    let axes = window.as_array().expect("axes");
    let names: Vec<&str> = axes
        .iter()
        .map(|a| a["predicate"].as_str().unwrap_or_default())
        .collect();
    assert!(
        !names.contains(&"planned_activity"),
        "a bucket bigger than the page is SKIPPED, not truncated ({names:?})"
    );
    assert!(
        axes.len() <= WINDOW_POOL_AXES,
        "the pool is capped over all axes, got {} axes",
        axes.len()
    );
    assert_eq!(
        names[0], "axis_8",
        "most recently asserted axis first -- a replacement lands on an axis the \
         conversation just touched, and the budget has to spend itself there ({names:?})"
    );
}

/// A window page of `n` rows in the order the store returns it (axis-major):
/// one short axis at the head, a long one filling the middle, one short axis at
/// the tail.
fn full_page(n: usize) -> serde_json::Value {
    let mut rows = vec![
        fact_row(
            "h1",
            "user",
            "aaa_head",
            "head value one",
            "2026-01-01T00:00:00Z",
        ),
        fact_row(
            "h2",
            "user",
            "aaa_head",
            "head value two",
            "2026-01-02T00:00:00Z",
        ),
    ];
    let tail = 3;
    for i in 0..n - rows.len() - tail {
        rows.push(fact_row(
            &format!("m{i}"),
            "user",
            "mmm_middle",
            &format!("middle value {i}"),
            "2026-02-01T00:00:00Z",
        ));
    }
    for i in 0..tail {
        rows.push(fact_row(
            &format!("t{i}"),
            "user",
            "zzz_tail",
            &format!("tail value {i}"),
            "2026-08-0{i}T00:00:00Z",
        ));
    }
    assert_eq!(rows.len(), n);
    serde_json::Value::Array(rows)
}

/// The predicates the window offers for a given page.
fn offered_axes(page: serde_json::Value) -> Vec<String> {
    let msgs = emit_x(x_reply("window-page", "select", page));
    parked(&msgs, "window-pool")
        .unwrap_or_else(|| serde_json::json!([]))
        .as_array()
        .expect("axes")
        .iter()
        .map(|a| a["predicate"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[test]
fn a_page_that_fills_its_cap_drops_the_one_axis_it_cannot_prove_complete() {
    // GH #68, the boundary. A page that came back FULL is a page the store may
    // have cut, and because it is axis-major there is exactly one axis that may
    // continue past it: the last. Its statement count is unknown, and counting an
    // unknown is precisely the bucket mistake ("a producer that sees six of
    // seventy plans cannot tell it is a bucket") in a page-shaped disguise. So it
    // is dropped, while every axis before it stays proven and offered.
    let cut = offered_axes(full_page(WINDOW_MAX_ROWS));
    assert!(
        cut.contains(&"aaa_head".to_string()),
        "a store big enough to trip the cap lost the window it still had: {cut:?}"
    );
    assert!(
        !cut.contains(&"zzz_tail".to_string()),
        "the unproven axis at the cut was offered anyway: {cut:?}"
    );
    // One row short of the cap the store proved it returned everything, so the
    // very same axis is offered. The bound is the ONLY difference between the two.
    let whole = offered_axes(full_page(WINDOW_MAX_ROWS - 1));
    assert!(
        whole.contains(&"zzz_tail".to_string()) && whole.contains(&"aaa_head".to_string()),
        "a page that fits under the cap must offer its tail axis: {whole:?}"
    );
    assert!(
        !whole.contains(&"mmm_middle".to_string()),
        "the bucket in the middle is refused for its LENGTH, cap or no cap: {whole:?}"
    );
}

/// The instructions the extractor receives for a batch that meets `window` at
/// the join-less meeting point.
fn instructions_for(window: Option<serde_json::Value>) -> String {
    let items = serde_json::json!([
        {"episode_id": "e9", "sender": "user", "content": "I use zed now."}
    ]);
    let mut rows = vec![
        serde_json::json!({"key": BATCH, "kind": "batch", "payload": items.to_string()}),
        serde_json::json!({"key": BATCH, "kind": "vocab",
                           "payload": "{\"user\": [\"favorite_editor\"]}"}),
    ];
    if let Some(w) = window {
        rows.push(serde_json::json!({"key": BATCH, "kind": "window-pool",
                                     "payload": w.to_string()}));
    }
    let msgs = emit_x(x_reply("prompt", "select", serde_json::Value::Array(rows)));
    let call = msgs
        .iter()
        .find(|m| m["header"]["route"] == "extract")
        .expect("the extractor call");
    call["system"]["instructions"]["text"]
        .as_str()
        .expect("instructions text")
        .to_string()
}

/// One offered axis in the shape the window is parked in.
fn offered_window() -> serde_json::Value {
    serde_json::json!([{
        "subject": "user", "predicate": "favorite_editor",
        "statements": [
            {"id": "f1", "claim": "favorite editor is helix", "since": "2026-03-05T00:00:00Z"}
        ]
    }])
}

#[test]
fn the_prompt_shows_the_open_statements_with_the_id_a_replaces_has_to_name() {
    // Guard rail 3 read from the other end: the extractor can only ever name what
    // this block contains, so the block IS the contract. The id has to be in the
    // rendered text -- a window the model cannot quote is a window it cannot use.
    let text = instructions_for(Some(offered_window()));
    assert!(
        text.contains("favorite editor is helix"),
        "the current VALUE is missing from the prompt:\n{text}"
    );
    assert!(
        text.contains("f1"),
        "the id is the reference the parse path validates, so it has to be \
         rendered:\n{text}"
    );
    assert!(
        text.contains("2026-03-05T00:00:00Z"),
        "since when the statement holds is what lets a model tell an update from \
         an addition:\n{text}"
    );
    assert!(
        text.contains("replaces"),
        "the answer shape has to name the field or nothing can come back:\n{text}"
    );
}

#[test]
fn the_prompt_rules_are_the_p1_discipline_one_dimension_over() {
    // The three rules ruling Q2 option C asks for, in the prompt rather than in a
    // comment: replace only the same matter, never an enumeration, and when in
    // doubt do not -- because the night is the party that can still decide it.
    let text = instructions_for(Some(offered_window()));
    let lower = text.to_lowercase();
    for needle in [
        "enumerat",
        "coexist",
        "not in the list",
        "leave `replaces` out",
    ] {
        assert!(
            lower.contains(&needle.to_lowercase()),
            "the replacement rules are missing {needle:?}:\n{text}"
        );
    }
    assert!(
        text.contains("has_child") || lower.contains("second child"),
        "the enumeration case needs a NAMED example, the way every other rule of \
         this prompt carries one:\n{text}"
    );
}

#[test]
fn a_batch_without_a_window_carries_no_replacement_block_at_all() {
    // The inline ingress and every batch from before this package meet no window
    // row. The prompt must then not offer a field whose only legal values do not
    // exist -- and the P1 prompt has to survive untouched, which is what makes
    // this package additive.
    let text = instructions_for(None);
    assert!(
        !text.contains("`replaces`"),
        "a prompt without a window invited a replacement it cannot honour:\n{text}"
    );
    assert!(text.contains("WORLD STATE") && text.contains("snake_case"));
    assert!(
        text.contains("favorite_editor"),
        "the P1 axis hint is independent of the window and stays"
    );
}

/// The extractor's answer as the return edge delivers it.
fn extractor_answer(text: &str) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"store_origin": "extract", "mem_phase": "parse", "batch_id": BATCH},
            "hop": {"finish_reason": "stop"}
        },
        "messages": [{"origin": "assistant", "type": "text", "text": text}]
    })
}

const ONE_REPLACEMENT: &str = r#"{"facts":[
  {"episode_id":"e9","subject":"user","predicate":"favorite_editor",
   "claim":"favorite editor is zed","fact_kind":"world","replaces":"f1","confidence":90}]}"#;

#[test]
fn the_replaces_signal_survives_the_validator_into_the_staged_payload() {
    // The lane is a state machine over a stateless cell: what `parse` does not
    // carry into `scratch` is gone by the time the facts are written. The signal
    // travels with its fact, not next to it -- one fact replaces one statement,
    // and a second list would have to be re-joined to the first.
    let msgs = emit_x(extractor_answer(ONE_REPLACEMENT));
    let staged = parked(&msgs, "payload").expect("the payload is staged");
    assert_eq!(staged["facts"][0]["replaces"], "f1");
    assert_eq!(
        staged["facts"][0]["claim"], "favorite editor is zed",
        "and the fact itself is untouched by the new field"
    );
}

#[test]
fn a_fact_without_the_field_stages_an_empty_signal() {
    // `replaces` is OPTIONAL, and the P1/P2 answer shape has to keep working
    // unchanged: the overwhelming majority of extracted facts replace nothing.
    let msgs = emit_x(extractor_answer(
        r#"{"facts":[{"episode_id":"e9","subject":"user","predicate":"has_child",
                      "claim":"has a child named ben","fact_kind":"world"}]}"#,
    ));
    let staged = parked(&msgs, "payload").expect("payload");
    assert_eq!(staged["facts"][0]["replaces"], "");
}

#[test]
fn a_replaces_that_is_not_a_plain_reference_is_flattened_not_trusted() {
    // A model that answers with an object, a list or a number must not put a
    // structure into the `where` of a store op. The validator drops everything
    // it cannot read as ONE reference -- the same discipline every other field of
    // this validator follows, and the reason bad items never abort a batch.
    let msgs = emit_x(extractor_answer(
        r#"{"facts":[{"episode_id":"e9","subject":"user","predicate":"favorite_editor",
                      "claim":"favorite editor is zed","fact_kind":"world",
                      "replaces":{"id":"f1"}},
                     {"episode_id":"e9","subject":"user","predicate":"lives_in",
                      "claim":"lives in zone-b","fact_kind":"world",
                      "replaces":["f1","f2"]}]}"#,
    ));
    let staged = parked(&msgs, "payload").expect("payload");
    assert_eq!(staged["facts"][0]["replaces"], "");
    assert_eq!(staged["facts"][1]["replaces"], "");
}

/// The window as the vocabulary phase parks it: one axis, one open statement.
fn parked_window() -> String {
    serde_json::json!([{
        "subject": "user", "predicate": "favorite_editor",
        "statements": [
            {"id": "f1", "claim": "favorite editor is helix", "since": "2026-03-05T00:00:00Z"}
        ]
    }])
    .to_string()
}

/// One staged fact in the shape `parse` leaves it in.
fn staged_fact(predicate: &str, claim: &str, replaces: &str) -> serde_json::Value {
    serde_json::json!({
        "episode_id": "e9", "subject": "user", "predicate": predicate, "claim": claim,
        "fact_kind": "world", "valid_from": "", "valid_until": null, "confidence": 90,
        "replaces": replaces, "claim_hash": claim.replace(' ', "_")
    })
}

/// Run the apply phase over a staged payload, with or without a parked window.
fn apply(facts: serde_json::Value, window: Option<String>) -> (Vec<serde_json::Value>, String) {
    let payload = serde_json::json!({"facts": facts, "entities": [], "edges": []});
    let episodes = serde_json::json!({"e9": {"happened_at": "2026-06-05T00:00:00Z",
                                             "session_id": "s2"}});
    let mut rows = vec![
        serde_json::json!({"key": BATCH, "kind": "payload", "payload": payload.to_string()}),
        serde_json::json!({"key": BATCH, "kind": "known", "payload": "[]"}),
        serde_json::json!({"key": BATCH, "kind": "episodes",
                           "payload": episodes.to_string()}),
    ];
    if let Some(w) = window {
        rows.push(serde_json::json!({"key": BATCH, "kind": "window", "payload": w}));
    }
    run(
        &script_of(EXTRACT_CONFIG),
        x_reply("apply", "select", serde_json::Value::Array(rows)),
    )
}

/// Every `facts` op the apply phase emitted, in order.
fn fact_ops(msgs: &[serde_json::Value], operation: &str) -> Vec<serde_json::Value> {
    msgs.iter()
        .map(args_of)
        .filter(|a| a["table"] == "facts" && a["operation"] == operation)
        .collect()
}

#[test]
fn a_valid_replaces_becomes_exactly_the_write_form_the_judge_uses() {
    // The whole point of the handover: a closure is a closure. Three columns on
    // the closed statement, one `update` on `facts`, and the read path
    // (span_end / span_successor / chain_target) cannot tell the two producers
    // apart -- which is what keeps the rendering, the currency marker and the
    // withdrawal identical for both.
    let (msgs, _) = apply(
        serde_json::json!([staged_fact(
            "favorite_editor",
            "favorite editor is zed",
            "f1"
        )]),
        Some(parked_window()),
    );
    let inserted = fact_ops(&msgs, "insert");
    assert_eq!(inserted.len(), 1, "the new fact is written");
    let new_id = inserted[0]["row"]["id"].as_str().expect("id").to_string();
    let closures = fact_ops(&msgs, "update");
    assert_eq!(closures.len(), 1, "one replaces, one closure: {closures:?}");
    assert_eq!(
        closures[0]["set"],
        serde_json::json!({"expired_at": "2026-06-05T00:00:00Z",
                           "superseded_by": new_id,
                           "closure_source": "extract:b1"}),
        "the instant is the START of the replacing statement (its event time, not \
         the ingest), the successor is the fact just written, and the attribution \
         names the author AND the batch whose scratch row carries the reason"
    );
    assert_eq!(
        closures[0]["where"],
        serde_json::json!({"id": "f1", "canonical_subject": "user",
                           "canonical_predicate": "favorite_editor",
                           "expired_at": {"is_null": true}}),
        "the axis echo comes from the WINDOW rather than from the model, so a row \
         that moved axis since the prompt is not closed by a stale reference -- \
         and `expired_at is_null` means an extractor closure never overwrites a \
         judged one"
    );
}

#[test]
fn a_replaces_outside_the_window_is_discarded_and_says_so() {
    // Ruling Q2 guard rail 3, and it is the only thing this package really owns:
    // the extractor writes WITHOUT the gate, so what it may close is bounded by
    // what it was provably shown. A well-formed id of a real statement is
    // discarded here for one reason only -- it was not in the window.
    let (msgs, err) = apply(
        serde_json::json!([staged_fact(
            "favorite_editor",
            "favorite editor is zed",
            "somewhere-else"
        )]),
        Some(parked_window()),
    );
    assert_eq!(
        fact_ops(&msgs, "insert").len(),
        1,
        "the FACT is still written"
    );
    assert!(
        fact_ops(&msgs, "update").is_empty(),
        "a closure on an unseen statement was written: {msgs:?}"
    );
    assert!(
        err.contains("window"),
        "a discarded closure has to be visible somewhere, got stderr {err:?}"
    );
}

#[test]
fn a_batch_that_was_shown_no_window_closes_nothing_at_all() {
    // The inline ingress and every batch from before this package. There is no
    // window row, so there is no proof, so there is no closure -- and the facts
    // are written exactly as they always were.
    let (msgs, _) = apply(
        serde_json::json!([staged_fact(
            "favorite_editor",
            "favorite editor is zed",
            "f1"
        )]),
        None,
    );
    assert_eq!(fact_ops(&msgs, "insert").len(), 1);
    assert!(fact_ops(&msgs, "update").is_empty());
}

#[test]
fn the_enumeration_case_closes_nothing_because_nothing_asked_it_to() {
    // The second son, which is the case the whole track exists for. Whether the
    // MODEL refuses to replace here is a prompt property and belongs to the paid
    // run; what is mechanical -- and what this pins -- is that no closure exists
    // without an explicit `replaces`. The lane never infers one from the data
    // again (that arithmetic is exactly what W2 removed).
    let (msgs, _) = apply(
        serde_json::json!([
            staged_fact("has_child", "has a child named ben", ""),
            staged_fact("favorite_editor", "favorite editor is helix", "")
        ]),
        Some(parked_window()),
    );
    assert_eq!(fact_ops(&msgs, "insert").len(), 2);
    assert!(
        fact_ops(&msgs, "update").is_empty(),
        "the lane closed a statement nobody named: {msgs:?}"
    );
}

#[test]
fn the_extraction_lane_may_only_close_never_delete_and_never_rewrite() {
    // Ruling Q2 guard rail 1, as a property of every op the apply phase emits --
    // the same assertion W3 makes about the judge. `claim`, `valid_from` and
    // `valid_until` of the closed statement are not this lane's to touch: that is
    // what keeps the whole closure revertible.
    let (msgs, _) = apply(
        serde_json::json!([staged_fact(
            "favorite_editor",
            "favorite editor is zed",
            "f1"
        )]),
        Some(parked_window()),
    );
    for op in msgs.iter().map(args_of) {
        assert_ne!(op["operation"], "delete", "the lane deleted: {op}");
        if op["operation"] == "update" && op["table"] == "facts" {
            for k in op["set"].as_object().expect("set").keys() {
                assert!(
                    ["expired_at", "superseded_by", "closure_source"].contains(&k.as_str()),
                    "the extractor wrote {k} -- the closure columns are its whole \
                     write surface"
                );
            }
        }
    }
}

#[test]
fn the_batch_receipt_carries_the_reason_of_every_closure() {
    // Ruling Q2 guard rail 2, in this lane's own currency. The night folds its
    // reasons into `consolidation_log` because that is where a run quits; the
    // extraction lane quits a batch in `scratch` under the batch key -- which is
    // exactly the key `closure_source` names, so a closed fact reads back to the
    // turn that replaced it, and one batch reverts with one `where`.
    let (msgs, _) = apply(
        serde_json::json!([staged_fact(
            "favorite_editor",
            "favorite editor is zed",
            "f1"
        )]),
        Some(parked_window()),
    );
    let receipt = parked(&msgs, "extract-closures").expect("the batch receipt row");
    let entry = &receipt[0];
    assert_eq!(entry["id"], "f1");
    assert_eq!(entry["subject"], "user");
    assert_eq!(entry["predicate"], "favorite_editor");
    assert_eq!(
        entry["replaced_claim"], "favorite editor is helix",
        "the value that stopped being current, as the window showed it"
    );
    assert_eq!(
        entry["claim"], "favorite editor is zed",
        "and the value that replaced it -- the two together ARE the reason, which \
         is why this lane needs no prose one: the extractor's justification is the \
         turn it just read, and the episode below names it"
    );
    assert_eq!(entry["episode_id"], "e9");
    let closure = fact_ops(&msgs, "update")[0].clone();
    assert_eq!(entry["superseded_by"], closure["set"]["superseded_by"]);
    assert_eq!(entry["expired_at"], closure["set"]["expired_at"]);
}

#[test]
fn a_batch_that_closed_nothing_writes_no_receipt_row() {
    // The receipt grows a row only when there is something to receipt, the same
    // way the night's payload grows a key only then.
    let (msgs, _) = apply(
        serde_json::json!([staged_fact("has_child", "has a child named ben", "")]),
        Some(parked_window()),
    );
    assert!(parked(&msgs, "extract-closures").is_none());
}

#[test]
fn a_fact_the_dedup_drops_closes_nothing() {
    // The re-delivery case: the same turn extracted twice writes no second fact,
    // so there is no successor -- and a closure whose `superseded_by` names a row
    // that was never written would point into nothing. No new statement, no
    // replacement.
    let payload = serde_json::json!({
        "facts": [staged_fact("favorite_editor", "favorite editor is zed", "f1")],
        "entities": [], "edges": []
    });
    let episodes = serde_json::json!({"e9": {"happened_at": "2026-06-05T00:00:00Z",
                                             "session_id": "s2"}});
    let rows = serde_json::json!([
        {"key": BATCH, "kind": "payload", "payload": payload.to_string()},
        {"key": BATCH, "kind": "known", "payload": "[\"e9:favorite_editor_is_zed\"]"},
        {"key": BATCH, "kind": "episodes", "payload": episodes.to_string()},
        {"key": BATCH, "kind": "window", "payload": parked_window()}
    ]);
    let (msgs, _) = run(&script_of(EXTRACT_CONFIG), x_reply("apply", "select", rows));
    assert!(fact_ops(&msgs, "insert").is_empty());
    assert!(fact_ops(&msgs, "update").is_empty());
}

// --------------------------------------------------------- the night validates
//
// Ruling Q2 guard rail 3 has a second half: the extractor writes WITHOUT the
// gate, so the nightly round has to be able to look at what it closed and
// contradict it. The axis page W3 built already carries everything needed for
// that judgement -- it only has to include the statements a RECENT extract
// closure ended, and the contradiction is a revert in the W3 shape (clear the
// attribution, let the same night's re-derive withdraw the columns).

const RUN: &str = "r1";
const TO: &str = "2026-08-12T03:00:00Z";

/// One store reply of the dream lane as the edge delivers it.
fn d_reply(phase: &str, operation: &str, rows: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"store_origin": "dream", "mem_phase": phase,
                        "dream_run": RUN, "dream_to": TO},
            "hop": {"operation": operation, "rows_affected": 1}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r", "text": rows.to_string()}]
    })
}

fn emit_d(doc: serde_json::Value) -> Vec<serde_json::Value> {
    run(&script_of(DREAM_CONFIG), doc).0
}

/// One open fact row as the canonicalisation scan projects it.
fn scan_row(id: &str, predicate: &str, claim: &str, from: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id, "subject": "user", "canonical_subject": "user",
        "predicate": predicate, "canonical_predicate": predicate,
        "claim": claim, "canonical_claim": claim,
        "valid_from": from, "recorded_at": from,
        "expired_at": null, "closure_source": null
    })
}

/// The same row, closed by somebody.
fn closed_row(
    id: &str,
    predicate: &str,
    claim: &str,
    from: &str,
    source: &str,
) -> serde_json::Value {
    let mut row = scan_row(id, predicate, claim, from);
    row["expired_at"] = serde_json::json!("2026-08-11T09:00:00Z");
    row["closure_source"] = serde_json::json!(source);
    row
}

/// The inventory the scan parks for the meeting point.
fn parked_scan(rows: serde_json::Value) -> serde_json::Value {
    let msgs = emit_d(d_reply("canon-scan", "select", rows));
    let args = args_of(&msgs[0]);
    assert_eq!(args["row"]["kind"], "canon-scan");
    serde_json::from_str(args["row"]["payload"].as_str().expect("payload")).expect("scan payload")
}

#[test]
fn the_nightly_scan_reaches_back_over_the_recent_extract_closures() {
    // One select, one column, one filter: `or_null` is "expired_at >= cutoff OR
    // expired_at is null", i.e. the open statements PLUS everything closed
    // recently. Widening the scan is what lets the round see a producer that
    // wrote without it -- and the lookback is what keeps the page bounded on a
    // memory that has been closing statements for years.
    // The scan hangs on the park of the judged cardinality since W5 (#13, ruling
    // Q3) and on the identity questions' own closed read since GH #73; the
    // widening this pin is about is unaffected by which door opens it. What GH
    // #73 does NOT do is widen this page further: the review lane keeps its event
    // clock, and the identity questions got a read of their own precisely because
    // that clock is the wrong bound for their question (`f10`).
    let msgs = emit_d(d_reply("canon-closed", "select", serde_json::json!([])));
    let args = args_of(&msgs[0]);
    assert_eq!(args["phase"], serde_json::Value::Null);
    assert_eq!(
        args["where"],
        serde_json::json!({"expired_at": {"or_null": {"gte": "2026-08-05T03:00:00Z"}}}),
        "the cutoff is derived from `delta_to`, the only clock this lane has, so a \
         re-run reads the same window back"
    );
    let cols = args["columns"].as_array().expect("columns").clone();
    assert!(
        cols.iter().any(|c| c == "closure_source"),
        "without the attribution the round cannot tell WHOSE closure it is looking \
         at, and it may only contradict the extractor: {}",
        args["columns"]
    );
    assert!(cols.iter().any(|c| c == "expired_at"));
}

#[test]
fn an_axis_the_extractor_closed_is_put_back_in_front_of_the_judge() {
    // The case the whole second half exists for: the extractor decided in the
    // turn, one statement is closed, one is open -- and W3's rule ("more than one
    // OPEN statement") would never look at this axis again. The closure would
    // stand forever without anyone with the whole store in front of them ever
    // having seen it.
    let rows = serde_json::json!([
        scan_row(
            "e2",
            "favorite_editor",
            "favorite editor is zed",
            "2026-08-10T00:00:00Z"
        ),
        closed_row(
            "e1",
            "favorite_editor",
            "favorite editor is helix",
            "2026-03-05T00:00:00Z",
            "extract:b1"
        )
    ]);
    let axes = parked_scan(rows)["axes"].as_array().expect("axes").clone();
    assert_eq!(axes.len(), 1, "the axis is offered: {axes:?}");
    assert_eq!(axes[0]["predicate"], "favorite_editor");
    assert_eq!(
        axes[0]["statements"],
        serde_json::json!([
            {"id": "e2", "claim": "favorite editor is zed", "since": "2026-08-10T00:00:00Z",
             "last_asserted": "2026-08-10T00:00:00Z", "assertions": 1}
        ]),
        "the open side is unchanged -- W3's shape, not a second one"
    );
    assert_eq!(
        axes[0]["closed"],
        serde_json::json!([
            {"id": "e1", "claim": "favorite editor is helix", "since": "2026-03-05T00:00:00Z",
             "ended_at": "2026-08-11T09:00:00Z", "closed_by": "extract:b1"}
        ]),
        "and the closed side names its author, because the revert has to echo it"
    );
}

#[test]
fn a_closure_the_night_itself_wrote_is_not_offered_for_review() {
    // The round may contradict the EXTRACTOR, never a judgement (its own or an
    // earlier night's): that would make "only close, never delete" (ruling Q2
    // rail 1) revocable by the same party that wrote it. The arithmetic residue
    // is out for the same reason -- ruling Q4 already withdraws that one, and it
    // needs no opinion.
    let rows = serde_json::json!([
        scan_row(
            "e2",
            "favorite_editor",
            "favorite editor is zed",
            "2026-08-10T00:00:00Z"
        ),
        closed_row(
            "e1",
            "favorite_editor",
            "favorite editor is helix",
            "2026-03-05T00:00:00Z",
            "judge:r0"
        ),
        scan_row("h2", "lives_in", "lives in zone-b", "2026-08-10T00:00:00Z"),
        closed_row(
            "h1",
            "lives_in",
            "lives in zone-a",
            "2026-01-06T00:00:00Z",
            ""
        )
    ]);
    let axes = parked_scan(rows)["axes"].as_array().expect("axes").clone();
    assert!(
        axes.is_empty(),
        "a closure that is not the extractor's was put up for review: {axes:?}"
    );
}

#[test]
fn a_night_without_extract_closures_offers_exactly_what_w3_offered() {
    // The additive proof. The `closed` key exists only when there is something
    // closed to show, so every night on a store the extractor never closed on
    // builds byte for byte the payload it built before this package -- and the
    // W3 refusals (one open statement, a bucket bigger than the page) are
    // untouched.
    let mut rows = vec![
        scan_row(
            "e1",
            "favorite_editor",
            "favorite editor is helix",
            "2026-03-05T00:00:00Z",
        ),
        scan_row(
            "e2",
            "favorite_editor",
            "favorite editor is vscode",
            "2026-06-05T00:00:00Z",
        ),
        scan_row("h1", "lives_in", "lives in zone-a", "2026-01-06T00:00:00Z"),
    ];
    for i in 0..7 {
        rows.push(scan_row(
            &format!("p{i}"),
            "planned_activity",
            &format!("plan number {i}"),
            &format!("2026-04-0{}T00:00:00Z", i + 1),
        ));
    }
    let axes = parked_scan(serde_json::Value::Array(rows))["axes"]
        .as_array()
        .expect("axes")
        .clone();
    assert_eq!(
        axes,
        vec![serde_json::json!({
            "subject": "user", "predicate": "favorite_editor",
            "statements": [
                {"id": "e1", "claim": "favorite editor is helix",
                 "since": "2026-03-05T00:00:00Z", "last_asserted": "2026-03-05T00:00:00Z",
                 "assertions": 1},
                {"id": "e2", "claim": "favorite editor is vscode",
                 "since": "2026-06-05T00:00:00Z", "last_asserted": "2026-06-05T00:00:00Z",
                 "assertions": 1}
            ]
        })],
        "the W3 payload, unchanged: no `closed` key, one open axis, the single \
         statement and the oversized bucket both refused"
    );
}

#[test]
fn a_closed_statement_is_no_longer_part_of_the_identity_questions() {
    // The widened scan feeds THREE sections, and only the third one asked for it.
    // A predicate whose every value was closed must not reappear in the alias
    // inventory, or the round would spend its budget re-judging the names of
    // statements nobody holds any more.
    let rows = serde_json::json!([
        scan_row(
            "e2",
            "favorite_editor",
            "favorite editor is zed",
            "2026-08-10T00:00:00Z"
        ),
        closed_row(
            "g1",
            "Lieblingseditor",
            "favorite editor is helix",
            "2026-03-05T00:00:00Z",
            "extract:b1"
        )
    ]);
    let parked = parked_scan(rows);
    assert!(
        parked["predicates"].get("Lieblingseditor").is_none(),
        "a closed statement was offered as an identity question: {}",
        parked["predicates"]
    );
    assert!(parked["predicates"].get("favorite_editor").is_some());
}

/// The parked halves, as the meeting-point select hands them back.
fn meeting_point(axes: serde_json::Value) -> serde_json::Value {
    let scan = serde_json::json!({"predicates": {"favorite_editor": ["user"]},
                                  "context": {}, "axes": axes});
    serde_json::json!([
        {"key": RUN, "kind": "canon-scan", "payload": scan.to_string()},
        {"key": RUN, "kind": "canon-pairs", "payload": "[]"}
    ])
}

/// One axis carrying an open statement and the one the extractor closed.
fn reviewed_axis() -> serde_json::Value {
    serde_json::json!([{
        "subject": "user", "predicate": "favorite_editor",
        "statements": [
            {"id": "e2", "claim": "favorite editor is zed", "since": "2026-08-10T00:00:00Z",
             "last_asserted": "2026-08-10T00:00:00Z", "assertions": 1}
        ],
        "closed": [
            {"id": "e1", "claim": "favorite editor is helix", "since": "2026-03-05T00:00:00Z",
             "ended_at": "2026-08-11T09:00:00Z", "closed_by": "extract:b1"}
        ]
    }])
}

/// The judge's answer as the return edge delivers it.
fn judgement(text: &str) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"store_origin": "dream", "mem_phase": "canon-judged",
                        "dream_run": RUN, "dream_to": TO},
            "hop": {"finish_reason": "stop"}
        },
        "messages": [{"origin": "assistant", "type": "text", "text": text}]
    })
}

/// Every `facts` update a judgement produced, in order.
fn judged_updates(msgs: &[serde_json::Value]) -> Vec<serde_json::Value> {
    msgs.iter()
        .map(args_of)
        .filter(|a| a["operation"] == "update" && a["table"] == "facts")
        .collect()
}

#[test]
fn the_review_travels_in_the_same_single_call() {
    // The P5 ruling stands and W4 does not spend a second call on it: the closure
    // the extractor wrote is reviewed inside the axis section that already
    // exists, by a judge that is already reading that axis.
    let msgs = emit_d(d_reply(
        "canon-ask",
        "select",
        meeting_point(reviewed_axis()),
    ));
    assert_eq!(msgs.len(), 1, "one call a night, not one per question");
    assert_eq!(msgs[0]["header"]["route"], "judge");
    let payload: serde_json::Value =
        serde_json::from_str(msgs[0]["messages"][0]["text"].as_str().expect("payload"))
            .expect("payload json");
    assert_eq!(
        payload["axes"],
        reviewed_axis(),
        "the closed side reaches the judge exactly as the scan parked it"
    );
    let text = msgs[0]["system"]["instructions"]["text"]
        .as_str()
        .expect("instructions");
    assert!(
        text.contains("reopenings"),
        "the answer shape has to name the contradiction or nothing can come back"
    );
    assert!(
        text.contains("closed_by"),
        "the attribution is copied back by the judge, so it has to be asked for"
    );
}

#[test]
fn a_contradiction_clears_exactly_the_attribution_and_nothing_else() {
    // The revert the W3 receipt describes, applied to the other producer: there
    // is no closure table, the attribution is a column, so clearing it plus the
    // re-derive of this very night withdraws `expired_at` and `superseded_by` on
    // their own. The judge does not write those two back -- doing that here would
    // be a second source of truth for a value the chain derives.
    let msgs = emit_d(judgement(
        r#"{"reopenings":[{"subject":"user","predicate":"favorite_editor","statement":"e1",
                           "closed_by":"extract:b1",
                           "reason":"helix and zed are two editors the user uses side by side"}]}"#,
    ));
    let updates = judged_updates(&msgs);
    assert_eq!(
        updates.len(),
        1,
        "one contradiction, one update: {updates:?}"
    );
    assert_eq!(
        updates[0]["set"],
        serde_json::json!({"closure_source": ""}),
        "the revert is the attribution and nothing else"
    );
    assert_eq!(
        updates[0]["where"],
        serde_json::json!({"id": "e1", "canonical_subject": "user",
                           "canonical_predicate": "favorite_editor",
                           "closure_source": "extract:b1"}),
        "the axis echo AND the exact attribution the payload showed: a garbled \
         copy matches no row, and a judgement can never be reverted this way"
    );
}

#[test]
fn a_contradiction_that_cannot_be_read_back_or_aims_at_a_judgement_is_dropped() {
    // The same refusal discipline the closures already carry. The interesting one
    // is the last: `judge:` is a well-formed attribution of a well-formed row,
    // and it is refused because rail 1 says a judgement may only CLOSE -- a round
    // that could revoke a judgement would be able to delete after all.
    let msgs = emit_d(judgement(
        r#"{"reopenings":[{"subject":"user","predicate":"favorite_editor","statement":"e1",
                           "closed_by":"extract:b1","reason":"  "},
                          {"subject":"","predicate":"","statement":"e1",
                           "closed_by":"extract:b1","reason":"no axis"},
                          {"subject":"user","predicate":"favorite_editor","statement":"",
                           "closed_by":"extract:b1","reason":"no statement"},
                          {"subject":"user","predicate":"favorite_editor","statement":"e1",
                           "closed_by":"judge:r0","reason":"an earlier night was wrong"},
                          {"subject":"user","predicate":"favorite_editor","statement":"e1",
                           "closed_by":"","reason":"the arithmetic residue"},
                          "not an object"]}"#,
    ));
    assert!(
        judged_updates(&msgs).is_empty(),
        "a contradiction the store cannot justify was written anyway: {msgs:?}"
    );
    assert_eq!(
        args_of(&msgs[0])["operation"],
        "canonicalize",
        "and the round still ends in its single re-derive"
    );
}

#[test]
fn the_contradiction_is_written_before_any_alias_moves_an_identity() {
    // Same argument as the closures: the `where` echoes the axis the judge was
    // shown, and `canonicalize` is what moves an identity. Written after the
    // re-derive the same `where` would match nothing.
    let msgs = emit_d(judgement(
        r#"{"entities":[{"alias":"user:u1","canonical":"user"}],
            "reopenings":[{"subject":"user","predicate":"favorite_editor","statement":"e1",
                           "closed_by":"extract:b1","reason":"the two coexist"}]}"#,
    ));
    let ops: Vec<String> = msgs
        .iter()
        .map(|m| {
            args_of(m)["operation"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    let revert_at = ops.iter().position(|o| o == "update").expect("the revert");
    let derive_at = ops
        .iter()
        .position(|o| o == "canonicalize")
        .expect("the re-derive");
    assert!(revert_at < derive_at, "the revert came too late: {ops:?}");
}

#[test]
fn the_run_receipt_carries_the_reason_of_every_contradiction() {
    // Ruling Q2 guard rail 2 in both directions: the closure names its reason and
    // so does the verdict that takes one back. Same place, same payload, and
    // absent when nothing was contradicted.
    let rows = serde_json::json!([
        {"key": RUN, "kind": "verdicts", "payload": "{\"beliefs\": []}"},
        {"key": RUN, "kind": "beliefs", "payload": "[]"},
        {"key": RUN, "kind": "canon-reopenings",
         "payload": "[{\"id\":\"e1\",\"subject\":\"user\",\"predicate\":\"favorite_editor\",\
                       \"closed_by\":\"extract:b1\",\"reason\":\"the two editors coexist\"}]"}
    ]);
    let msgs = emit_d(d_reply("apply-run", "select", rows));
    let closing = msgs
        .iter()
        .map(args_of)
        .find(|a| a["table"] == "consolidation_log")
        .expect("the run is closed");
    let verdicts: serde_json::Value =
        serde_json::from_str(closing["set"]["verdicts"].as_str().expect("verdicts"))
            .expect("verdict payload");
    assert_eq!(verdicts["reopenings"][0]["id"], "e1");
    assert_eq!(
        verdicts["reopenings"][0]["reason"],
        "the two editors coexist"
    );
    assert_eq!(verdicts["reopenings"][0]["closed_by"], "extract:b1");
}

/// Hand a probe program to python3 **on stdin**, never in argv.
///
/// A probe embeds the whole shipped script as a literal, and a single argv
/// string is capped at 128 KiB (`MAX_ARG_STRLEN`). The recall script crossed
/// that line in W2, and the failure mode is an opaque `ArgumentListTooLong`
/// that looks like a broken test rather than like a size limit. `python3 -`
/// reads and compiles the whole program from stdin before it runs a line of
/// it, so the probe's own `sys.stdin` replacement below is unaffected.
fn run_python(src: &str) -> std::process::Output {
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    // Dropped, not merely borrowed: python reads until EOF.
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    child.wait_with_output().expect("wait")
}

/// Run a shipped script over a real stdin document, handing the script to
/// python3 **on stdin** instead of in argv.
///
/// A single argv string is capped at 128 KiB (`MAX_ARG_STRLEN`) and the shipped
/// scripts have grown to within a few KB of that line, so `python3 -c <whole
/// script>` is a harness that breaks on size rather than on behaviour (GH #279,
/// precedent 89a522e4). stdin carries the program, so the document rides inside
/// it and is put under `sys.stdin` before the script runs. From there the script
/// executes exactly as `python3 -c` ran it: same `__main__` globals, same
/// stdout, same exit status.
fn run_script_on_stdin(script: &str, stdin_doc: &str) -> std::process::Output {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        serde_json::to_string(script).unwrap(),
        serde_json::to_string(stdin_doc).unwrap(),
    );
    run_python(&src)
}

/// Run a probe against the module body of the real dream script. The `park()`
/// exit at its end is swallowed so the probe can call the helpers the script
/// defines -- the construction `w2_statement_chain.rs` introduced.
fn run_probe(probe: &str) -> String {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO('{{\"envelope\": {{}}, \"body\": {{}}, \"params\": {{}}}}')\n",
            "_sink, _real = io.StringIO(), sys.stdout\n",
            "sys.stdout = _sink\n",
            "try:\n",
            "    exec(compile(_script, 'dream-glue', 'exec'), globals())\n",
            "except SystemExit:\n",
            "    pass\n",
            "finally:\n",
            "    sys.stdout = _real\n",
            "{}"
        ),
        serde_json::to_string(&script_of(DREAM_CONFIG)).unwrap(),
        probe
    );
    let out = run_python(&src);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// One axis closed by the EXTRACTOR plus the materialisation loop of a night.
const EXTRACT_CLOSED_AXIS: &str = r#"
rows = [
 {"id":"e1","subject":"user","canonical_subject":"user","predicate":"favorite_editor",
  "canonical_predicate":"favorite_editor","claim":"helix","canonical_claim":"helix",
  "valid_from":"2026-03-05T00:00:00Z","valid_until":None,"recorded_at":"2026-03-05T00:00:00Z",
  "expired_at":"2026-08-10T00:00:00Z","superseded_by":"e2","closure_source":"extract:b1",
  "session_id":"s1"},
 {"id":"e2","subject":"user","canonical_subject":"user","predicate":"favorite_editor",
  "canonical_predicate":"favorite_editor","claim":"zed","canonical_claim":"zed",
  "valid_from":"2026-08-10T00:00:00Z","valid_until":None,"recorded_at":"2026-08-10T00:00:00Z",
  "expired_at":None,"superseded_by":None,"closure_source":None,"session_id":"s2"}
]

def a_round(rs):
    """One nightly round: derive, then write the differences back."""
    by = {str(r["id"]): r for r in rs}
    out = derive_supersessions(rs)
    for fid, end, succ in out:
        by[str(fid)]["expired_at"] = end
        by[str(fid)]["superseded_by"] = succ
    return out
"#;

#[test]
fn an_extract_closure_is_stable_round_after_round() {
    // The pin the W3 handover asked W4 to copy: a closure is a closure, so the
    // nightly re-derive has to leave an ATTRIBUTED extractor closure exactly as
    // alone as it leaves a judged one. Three rounds, because a rule that survives
    // one may still drift on the second.
    let probe = EXTRACT_CLOSED_AXIS.to_string()
        + r#"
seen = [a_round(rows) for _ in range(3)]
print(seen[0] == seen[1] == seen[2], seen[2][0], rows[0]["closure_source"])
"#;
    assert_eq!(
        run_probe(&probe),
        "True ('e1', '2026-08-10T00:00:00Z', 'e2') extract:b1",
        "an extractor closure did not survive repeated rounds unchanged"
    );
}

#[test]
fn the_contradiction_takes_the_closure_back_in_the_same_night() {
    // The other half of the same mechanism, and the reason the revert needs to
    // write only ONE column: the round clears the attribution BEFORE `sup-axes`,
    // so the re-derive of that same night finds a closure the chain cannot
    // reproduce and withdraws it to NULL (ruling Q4). Without the ordering the
    // reopened statement would stay closed until the following night.
    let probe = EXTRACT_CLOSED_AXIS.to_string()
        + r#"
rows[0]["closure_source"] = ""
print(a_round(rows)[0], rows[0]["expired_at"], rows[0]["superseded_by"])
"#;
    assert_eq!(
        run_probe(&probe),
        "('e1', None, None) None None",
        "a contradicted closure was left standing"
    );
}

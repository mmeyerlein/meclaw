//! 0.3.x follow-up F5 -- the axes the currency question exists for become
//! reachable by it (GitHub #66).
//!
//! The nightly round asks a judge which of the open statements of one axis
//! replaced which. Two bounds decided which axes ever got asked, and the
//! track-end measurement counted both: of 185 axes carrying more than one open
//! statement, 113 fitted the per-axis page and 72 (39 percent) did NOT and were
//! dropped rather than truncated. Those 72 are on that corpus precisely the
//! bucket axes the whole track was opened for -- `planned_activity`,
//! `plans_to_*`, `interested_in`, `has_experience`, `uses`, `practices`.
//!
//! The rule that dropped them is right and is not weakened here: a judge shown
//! six of seventy plans cannot tell it is looking at a bucket, so a truncated
//! page would be worse than none. What changes is the question such an axis is
//! asked. Cardinality first:
//!
//! ```text
//! over-cap axis -> cardinality of its RELATION (cheap: predicate + a sample)
//!    multi  -> terminal. Values coexist, no currency question, ever.
//!    single -> paged currency, most recent statements first, across nights.
//!    none   -> no page tonight; the relation heads the cardinality question.
//! ```
//!
//! The paging is sound without a cursor because closing is the only way to leave
//! the open set: every statement that SURVIVES a page is still among the most
//! recent open ones and is therefore on the next page too. The current value is
//! carried between pages by that same rule, so a judgement never compares two
//! statements that were not in one prompt together.
//!
//! Everything here runs the REAL `params.script_inline` of the `code` cell
//! against injected store replies, so no model is called and nothing costs
//! anything.

use std::io::Write;
use std::process::{Command, Stdio};

const GLUE_CONFIG: &str = "../../templates/memory-hive/dream-glue/config.json";

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

fn glue_script() -> String {
    let raw = std::fs::read_to_string(GLUE_CONFIG).expect("config");
    let config: serde_json::Value = serde_json::from_str(&raw).expect("config json");
    resolve_vars(config["params"]["script_inline"].as_str().expect("script"))
}

/// Run the real script with a real stdin document and return the emitted messages.
fn emit(doc: serde_json::Value) -> Vec<serde_json::Value> {
    let script = glue_script();
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(&script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(&meclaw_testing::code_stdin_bytes(&doc))
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "dream-glue exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("message array")
}

const RUN: &str = "r1";
const TO: &str = "2026-08-12T03:00:00Z";

/// One store reply as the edge delivers it.
fn store_reply(phase: &str, operation: &str, rows: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"store_origin": "dream", "mem_phase": phase,
                        "dream_run": RUN, "dream_to": TO},
            "hop": {"operation": operation, "rows_affected": 1}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r", "text": rows.to_string()}]
    })
}

fn args_of(msg: &serde_json::Value) -> serde_json::Value {
    let text = msg["messages"][0]["text"].as_str().expect("op text");
    serde_json::from_str(text).expect("op args")
}

/// One open fact row in the shape the canonicalisation scan projects.
fn row(id: &str, predicate: &str, claim: &str, from: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id, "subject": "user", "canonical_subject": "user",
        "predicate": predicate, "canonical_predicate": predicate,
        "claim": claim, "canonical_claim": claim,
        "valid_from": from, "recorded_at": from
    })
}

/// `count` open statements of one relation, oldest first, one day apart.
fn bucket(predicate: &str, count: usize) -> Vec<serde_json::Value> {
    (0..count)
        .map(|i| {
            row(
                &format!("{predicate}-{i}"),
                predicate,
                &format!("{predicate} value {i}"),
                &format!("2026-04-{:02}T00:00:00Z", i + 1),
            )
        })
        .collect()
}

/// The inventory the scan parks for the meeting point.
fn parked_scan(rows: Vec<serde_json::Value>) -> serde_json::Value {
    let msgs = emit(store_reply(
        "canon-scan",
        "select",
        serde_json::Value::Array(rows),
    ));
    let args = args_of(&msgs[0]);
    assert_eq!(args["row"]["kind"], "canon-scan");
    serde_json::from_str(args["row"]["payload"].as_str().expect("payload")).expect("scan payload")
}

/// The messages the ask phase emits for a parked scan plus a parked verdict map.
fn round(scan: &serde_json::Value, judged: &serde_json::Value) -> Vec<serde_json::Value> {
    emit(store_reply(
        "canon-ask",
        "select",
        serde_json::json!([
            {"key": RUN, "kind": "canon-scan", "payload": scan.to_string()},
            {"key": RUN, "kind": "canon-pairs", "payload": "[]"},
            {"key": RUN, "kind": "canon-card", "payload": judged.to_string()},
            {"key": RUN, "kind": "canon-refused", "payload": "[]"}
        ]),
    ))
}

/// The payload one night put to the judge.
fn asked(scan: &serde_json::Value, judged: &serde_json::Value) -> serde_json::Value {
    let msgs = round(scan, judged);
    assert_eq!(msgs[0]["header"]["route"], "judge", "no call was made");
    serde_json::from_str(msgs[0]["messages"][0]["text"].as_str().expect("payload"))
        .expect("payload json")
}

/// The instruction block of that same night.
fn instructions(scan: &serde_json::Value, judged: &serde_json::Value) -> String {
    let msgs = round(scan, judged);
    assert_eq!(msgs[0]["header"]["route"], "judge", "no call was made");
    msgs[0]["system"]["instructions"]["text"]
        .as_str()
        .expect("instructions")
        .to_string()
}

/// The coverage receipt the night parked, if it parked one.
fn coverage(scan: &serde_json::Value, judged: &serde_json::Value) -> Option<serde_json::Value> {
    for msg in round(scan, judged) {
        let args = args_of(&msg);
        if args["row"]["kind"] == "canon-pages" {
            return Some(
                serde_json::from_str(args["row"]["payload"].as_str().expect("payload"))
                    .expect("coverage json"),
            );
        }
    }
    None
}

/// The axis entries of a payload, by predicate.
fn axis(payload: &serde_json::Value, predicate: &str) -> Option<serde_json::Value> {
    payload["axes"]
        .as_array()
        .expect("axes")
        .iter()
        .find(|a| a["predicate"] == predicate)
        .cloned()
}

const EMPTY: fn() -> serde_json::Value = || serde_json::json!({});

// ------------------------------------------------------------------ the scan

#[test]
fn an_axis_bigger_than_one_page_leaves_the_scan_as_a_candidate() {
    // The defect itself. Seven open statements is one more than a page holds, and
    // until #66 that was the end of the axis: not offered, not counted, not
    // answerable. It is still not offered as a whole axis -- half a bucket is the
    // thing that must never be shown -- but it now leaves the scan carrying the
    // number the triage needs.
    let scan = parked_scan(bucket("planned_activity", 7));
    let names: Vec<&str> = scan["axes"]
        .as_array()
        .expect("axes")
        .iter()
        .map(|a| a["predicate"].as_str().unwrap_or_default())
        .collect();
    assert!(
        !names.contains(&"planned_activity"),
        "an over-cap axis is never offered as a complete axis: {names:?}"
    );
    let candidates = scan["paged"]["axes"].as_array().expect("paged section");
    assert_eq!(candidates.len(), 1, "the candidate is missing: {scan}");
    assert_eq!(candidates[0]["predicate"], "planned_activity");
    assert_eq!(candidates[0]["subject"], "user");
    assert_eq!(
        candidates[0]["page"],
        serde_json::json!({"open_statements": 7, "shown": 6}),
        "the page has to say how much of the axis it is"
    );
    assert_eq!(
        scan["paged"]["total"], 1,
        "the true number of over-cap axes travels next to the bounded pool"
    );
}

#[test]
fn the_page_of_an_over_cap_axis_is_its_most_recent_statements() {
    // Recency is not a preference, it is what makes the paging sound without a
    // cursor: closing is the only way to leave the open set, so the survivors of
    // a page are still the most recent open statements and are on the next page
    // too. The order inside the page is the one every other entry carries.
    let scan = parked_scan(bucket("planned_activity", 9));
    let page = scan["paged"]["axes"][0]["statements"]
        .as_array()
        .expect("statements")
        .clone();
    let ids: Vec<&str> = page
        .iter()
        .map(|s| s["id"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        ids,
        vec![
            "planned_activity-3",
            "planned_activity-4",
            "planned_activity-5",
            "planned_activity-6",
            "planned_activity-7",
            "planned_activity-8"
        ],
        "the six MOST RECENT statements, oldest of them first"
    );
    assert_eq!(
        page[0]["claim"], "planned_activity value 3",
        "a page carries the same fields as any other entry: {page:?}"
    );
}

#[test]
fn a_store_without_a_bucket_axis_parks_what_it_always_parked() {
    // The invariance half: the section is present only when there IS such an
    // axis, so every night of every store that never grew one parks byte for byte
    // what it parked before this change.
    let mut rows = bucket("favorite_editor", 2);
    rows.extend(bucket("has_child", 3));
    let scan = parked_scan(rows);
    assert!(
        scan.get("paged").is_none(),
        "an empty candidate list is no key at all: {scan}"
    );
    assert_eq!(
        scan["axes"].as_array().expect("axes").len(),
        2,
        "and both axes are offered whole: {scan}"
    );
}

#[test]
fn an_axis_pushed_over_the_page_by_its_closures_alone_is_untouched() {
    // The deliberate non-change. What #66 measured and fixed is the axis whose
    // OPEN statements do not fit. An axis whose open statements fit and which the
    // extractor closures next to them push over the bound needs a review page
    // rather than a currency page; it keeps the behaviour it had.
    let mut rows = bucket("planned_activity", 4);
    for i in 0..4 {
        let mut closed = row(
            &format!("closed-{i}"),
            "planned_activity",
            &format!("old plan {i}"),
            "2026-01-01T00:00:00Z",
        );
        closed["expired_at"] = serde_json::json!("2026-08-10T00:00:00Z");
        closed["closure_source"] = serde_json::json!("extract:batch-1");
        rows.push(closed);
    }
    let scan = parked_scan(rows);
    assert!(
        scan.get("paged").is_none(),
        "the closure half of the page is not what #66 pages: {scan}"
    );
    assert!(
        scan["axes"].as_array().expect("axes").is_empty(),
        "and it is still skipped, exactly as before: {scan}"
    );
}

// ----------------------------------------------------------------- the triage

#[test]
fn a_relation_this_memory_calls_enumerating_is_never_asked_again() {
    // Outcome one, and the terminal one. The 63 of 64 `multi` verdicts of the
    // track-end run were the judge saying exactly this about exactly these axes;
    // the skip becomes that answer instead of a constructional hole. Both halves
    // of the precedence: `has_child` stands on the seed list, `collects` was
    // judged by an earlier night.
    let mut rows = bucket("has_child", 7);
    rows.extend(bucket("collects", 8));
    let scan = parked_scan(rows);
    let judged = serde_json::json!({"collects": "multi"});
    let payload = asked(&scan, &judged);
    for gone in ["has_child", "collects"] {
        assert!(
            axis(&payload, gone).is_none(),
            "an enumerating relation was put to the currency question anyway: {payload}"
        );
    }
    let receipt = coverage(&scan, &judged).expect("a coverage receipt");
    assert_eq!(receipt["over_cap"], 2);
    assert_eq!(receipt["enumerating"], 2, "both were answered for good");
    assert_eq!(receipt["functional"], 0);
    assert_eq!(receipt["undecided"], 0);
    assert!(
        receipt["paged"].as_array().expect("paged").is_empty(),
        "nothing was paged and the receipt says so: {receipt}"
    );
}

#[test]
fn a_relation_this_memory_calls_functional_is_judged_one_page_at_a_time() {
    // Outcome two. Same two sources of the verdict, and the marker the judge
    // needs to know it is looking at a part: without it the page would be exactly
    // the truncation the old rule refused to perform.
    let mut rows = bucket("job_title", 7);
    rows.extend(bucket("desk_at", 9));
    let scan = parked_scan(rows);
    let judged = serde_json::json!({"desk_at": "single"});
    let payload = asked(&scan, &judged);
    let seeded = axis(&payload, "job_title").expect("the seeded functional axis");
    assert_eq!(
        seeded["page"],
        serde_json::json!({"open_statements": 7, "shown": 6})
    );
    let judged_axis = axis(&payload, "desk_at").expect("the judged functional axis");
    assert_eq!(
        judged_axis["page"],
        serde_json::json!({"open_statements": 9, "shown": 6})
    );
    let receipt = coverage(&scan, &judged).expect("a coverage receipt");
    assert_eq!(receipt["functional"], 2);
    assert_eq!(
        receipt["paged"],
        serde_json::json!([
            {"subject": "user", "predicate": "desk_at",
             "open_statements": 9, "shown": 6, "remaining": 3},
            {"subject": "user", "predicate": "job_title",
             "open_statements": 7, "shown": 6, "remaining": 1}
        ]),
        "what each page showed and what it left for a later night"
    );
}

#[test]
fn an_over_cap_axis_of_an_undecided_relation_is_asked_the_cheap_question_first() {
    // Outcome three, and the reason the triage is cardinality-FIRST: the
    // expensive question cannot be asked about this axis at all until the cheap
    // one is answered, and the cheap one costs a predicate and six values. It is
    // already at the head of that list, because the list is ranked by the busiest
    // axis of a relation and an over-cap axis is the busiest there is.
    let scan = parked_scan(bucket("interested_in", 9));
    let payload = asked(&scan, &EMPTY());
    assert!(
        axis(&payload, "interested_in").is_none(),
        "an undecided relation must not be paged: {payload}"
    );
    assert_eq!(
        payload["cardinality"][0]["predicate"], "interested_in",
        "the relation heads the cheap question: {payload}"
    );
    let receipt = coverage(&scan, &EMPTY()).expect("a coverage receipt");
    assert_eq!(receipt["undecided"], 1);
    assert_eq!(receipt["functional"], 0);
    assert_eq!(receipt["enumerating"], 0);
}

#[test]
fn the_pages_are_taken_out_of_the_axis_budget_and_never_added_to_it() {
    // The night that finally reaches a bucket axis must not be the night every
    // payload grows. Eight axes that fit plus one that does not still make eight
    // entries -- the page takes a slot, it does not open one.
    let mut rows: Vec<serde_json::Value> = Vec::new();
    for i in 0..8 {
        rows.extend(bucket(&format!("axis_{i}"), 2));
    }
    rows.extend(bucket("job_title", 7));
    let scan = parked_scan(rows);
    let payload = asked(&scan, &EMPTY());
    let axes = payload["axes"].as_array().expect("axes");
    assert_eq!(
        axes.len(),
        8,
        "the budget is the budget, pages included: {payload}"
    );
    assert!(
        axis(&payload, "job_title").is_some(),
        "the page has to be one of them: {payload}"
    );
    assert!(
        axis(&payload, "axis_7").is_none(),
        "and the slot comes off the least busy end of the ranking: {payload}"
    );
}

// --------------------------------------------------------------- the question

#[test]
fn a_night_that_pages_an_axis_says_so_and_declares_no_new_key() {
    // The F4 discipline: a paragraph renders when its data is there, and the
    // answer shape shrinks and grows with the questions. A page is not a sixth
    // question -- it is the same three answers about a part of an axis -- so it
    // brings a paragraph and no key.
    let scan = parked_scan(bucket("job_title", 7));
    let text = instructions(&scan, &EMPTY());
    for needed in [
        "Some axes of question 3 arrive as a PAGE.",
        "`open_statements` is how many open statements that axis holds",
        "judge the page against ITSELF",
        "an axis is only ever paged after this memory has decided that its relation is \
         FUNCTIONAL",
    ] {
        assert!(text.contains(needed), "the page paragraph lost {needed:?}");
    }
    let shape_start = text.find('{').expect("a shape");
    let shape_end = text.find("}.\n\n").expect("the end of the shape");
    let shape = &text[shape_start..shape_end + 1];
    assert!(
        !shape.contains("page"),
        "a page declares no answer key of its own: {shape}"
    );
    assert!(
        text.contains("3. `axes`") && text.contains("5. `same_value`"),
        "and it stands with the two questions that read the axis page: {text}"
    );
    let paged_at = text.find("Some axes of question 3").expect("the paragraph");
    let currency_at = text.find("3. `axes`").expect("question 3");
    let rewording_at = text.find("5. `same_value`").expect("question 5");
    assert!(
        currency_at < paged_at && paged_at < rewording_at,
        "the paragraph belongs between the two questions that read the page"
    );
}

#[test]
fn a_night_without_a_page_is_never_told_what_a_page_is() {
    // The other half of the same discipline, and the invariance half of this
    // package: a store that never grew a bucket axis is asked byte for byte what
    // it was asked before, so the verdicts of such a night need no re-measuring.
    let mut rows = bucket("favorite_editor", 2);
    rows.extend(bucket("has_child", 3));
    let scan = parked_scan(rows);
    let text = instructions(&scan, &EMPTY());
    assert!(
        !text.contains("arrive as a PAGE"),
        "a night without a page was told about pages: {text}"
    );
    assert!(
        text.contains("3. `axes`"),
        "while the question the paragraph belongs to is asked as always: {text}"
    );
}

#[test]
fn a_night_that_pages_nothing_emits_exactly_what_it_always_emitted() {
    // The second message is the whole surface this package adds to the wire. It
    // appears when there is something to receipt and never otherwise, so the ask
    // phase of a store without a bucket axis is unchanged to the message.
    let scan = parked_scan(bucket("favorite_editor", 2));
    let msgs = round(&scan, &EMPTY());
    assert_eq!(msgs.len(), 1, "one call a night, not one per question");
    assert_eq!(msgs[0]["header"]["route"], "judge");
}

// -------------------------------------------------------------- the soundness

#[test]
fn every_statement_that_survives_a_page_is_on_the_next_page() {
    // The carry, and the reason it needs no cursor and no second mechanism. The
    // page is the recency prefix of the OPEN statements and closing is the only
    // way to leave that set, so whatever the judge did not close is still in
    // front of it next time -- together with the value it decided is current.
    // That is what makes "a judgement never compares two statements that were
    // never in one prompt together" true by construction rather than by rule.
    let first = parked_scan(bucket("job_title", 12));
    let page_one: Vec<String> = first["paged"]["axes"][0]["statements"]
        .as_array()
        .expect("page")
        .iter()
        .map(|s| s["id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        first["paged"]["axes"][0]["page"],
        serde_json::json!({"open_statements": 12, "shown": 6})
    );
    // The night closed four of the six it saw and kept the two most recent.
    let survivors = ["job_title-10", "job_title-11"];
    let second = parked_scan(closed_by_the_judge(&page_one, &survivors, 12));
    let page_two: Vec<String> = second["paged"]["axes"][0]["statements"]
        .as_array()
        .expect("page")
        .iter()
        .map(|s| s["id"].as_str().unwrap_or_default().to_string())
        .collect();
    for survivor in survivors {
        assert!(
            page_two.contains(&survivor.to_string()),
            "a statement that survived page one fell off page two: {page_two:?}"
        );
    }
    assert_eq!(
        second["paged"]["axes"][0]["page"],
        serde_json::json!({"open_statements": 8, "shown": 6}),
        "and the axis shrank by exactly what was closed"
    );
    assert!(
        page_two.contains(&"job_title-2".to_string()),
        "while statements the first page never reached came into view: {page_two:?}"
    );
}

/// The same bucket after a night closed everything on `page` except `survivors`.
fn closed_by_the_judge(
    page: &[String],
    survivors: &[&str],
    count: usize,
) -> Vec<serde_json::Value> {
    bucket("job_title", count)
        .into_iter()
        .map(|mut r| {
            let id = r["id"].as_str().unwrap_or_default().to_string();
            if page.contains(&id) && !survivors.contains(&id.as_str()) {
                r["expired_at"] = serde_json::json!("2026-08-12T03:00:00Z");
                r["closure_source"] = serde_json::json!("judge:r1");
            }
            r
        })
        .collect()
}

#[test]
fn an_axis_the_pages_have_worn_down_is_judged_whole_again() {
    // Where the paging ENDS, and it ends by itself. Once the closures have taken
    // the axis under the page bound it stops being a candidate and becomes an
    // ordinary entry of the currency question -- no marker, no coverage row, and
    // no rule anywhere that has to notice the transition.
    let first = parked_scan(bucket("job_title", 9));
    let page_one: Vec<String> = first["paged"]["axes"][0]["statements"]
        .as_array()
        .expect("page")
        .iter()
        .map(|s| s["id"].as_str().unwrap_or_default().to_string())
        .collect();
    let second = parked_scan(closed_by_the_judge(&page_one, &["job_title-8"], 9));
    assert!(
        second.get("paged").is_none(),
        "four statements are not a page: {second}"
    );
    let axes = second["axes"].as_array().expect("axes");
    assert_eq!(axes.len(), 1);
    assert_eq!(axes[0]["predicate"], "job_title");
    assert!(
        axes[0].get("page").is_none(),
        "an axis shown whole must never carry a page marker: {second}"
    );
    assert_eq!(
        axes[0]["statements"].as_array().expect("statements").len(),
        4,
        "the whole of what is left: {second}"
    );
}

// ---------------------------------------------------------------- the receipt

/// The scratch rows the apply phase reads back, with a coverage receipt in them.
fn run_scratch(pages: Option<&str>) -> serde_json::Value {
    let mut rows = vec![
        serde_json::json!({"key": RUN, "kind": "verdicts", "payload": "{\"beliefs\":[]}"}),
        serde_json::json!({"key": RUN, "kind": "beliefs", "payload": "[]"}),
    ];
    if let Some(payload) = pages {
        rows.push(serde_json::json!({"key": RUN, "kind": "canon-pages", "payload": payload}));
    }
    serde_json::Value::Array(rows)
}

/// The `consolidation_log` update a run closes with.
fn close_op(msgs: &[serde_json::Value]) -> serde_json::Value {
    msgs.iter()
        .map(args_of)
        .find(|a| a["table"] == "consolidation_log")
        .expect("the run was never closed")
}

#[test]
fn the_run_receipt_states_the_coverage_of_the_axes_it_cannot_show_whole() {
    // The issue asks for the coverage to be STATED, and this lane has exactly one
    // place where it quits a dream artefact: the verdict payload of
    // consolidation_log. So the coverage lands where the closures, the
    // reopenings, the cardinality verdicts and the rewordings already land -- one
    // receipt, one key, one revert.
    let pages = "{\"over_cap\":9,\"triaged\":9,\"enumerating\":6,\"undecided\":2,\
                 \"functional\":1,\"paged\":[{\"subject\":\"user\",\"predicate\":\"job_title\",\
                 \"open_statements\":70,\"shown\":6,\"remaining\":64}]}";
    let msgs = emit(store_reply("apply-run", "select", run_scratch(Some(pages))));
    let verdicts = close_op(&msgs)["set"]["verdicts"]
        .as_str()
        .expect("verdicts")
        .to_string();
    let parsed: serde_json::Value = serde_json::from_str(&verdicts).expect("verdict json");
    assert_eq!(parsed["pages"]["over_cap"], 9);
    assert_eq!(parsed["pages"]["enumerating"], 6);
    assert_eq!(parsed["pages"]["paged"][0]["predicate"], "job_title");
    assert_eq!(
        parsed["pages"]["paged"][0]["remaining"], 64,
        "how much of the axis is still to come is the coverage: {parsed}"
    );
}

#[test]
fn a_night_that_paged_nothing_receipts_what_it_always_did() {
    // Same rule as every other key of this payload: absent when there is nothing
    // to say, so a store without a bucket axis writes byte for byte the receipt
    // it wrote before.
    let msgs = emit(store_reply("apply-run", "select", run_scratch(None)));
    let verdicts = close_op(&msgs)["set"]["verdicts"]
        .as_str()
        .expect("verdicts")
        .to_string();
    assert_eq!(
        verdicts, "{\"beliefs\": []}",
        "an untouched receipt is the receipt of the night before"
    );
}

#[test]
fn a_night_with_nothing_to_ask_still_says_what_it_answered_for_good() {
    // The cheapest night of all: every over-cap axis of the store enumerates, so
    // there is no question left to buy and the round makes no call. That the
    // round LOOKED is still a fact about the night, and a receipt that only
    // existed when a model was called could never show the terminal answers.
    let scan = parked_scan(bucket("has_child", 7));
    let msgs = round(&scan, &EMPTY());
    assert_eq!(
        msgs[0]["header"]["phase"], "sup-scope",
        "a night without a question makes no call: {msgs:?}"
    );
    assert_eq!(
        msgs.len(),
        2,
        "and it still receipts its coverage: {msgs:?}"
    );
    let receipt: serde_json::Value = serde_json::from_str(
        args_of(&msgs[1])["row"]["payload"]
            .as_str()
            .expect("payload"),
    )
    .expect("coverage json");
    assert_eq!(receipt["over_cap"], 1);
    assert_eq!(receipt["enumerating"], 1);
}

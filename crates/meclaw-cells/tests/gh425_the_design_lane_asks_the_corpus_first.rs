//! GH #425 — the design lane consults the corpus before it consults a model,
//! and a corpus that is down must not be able to hang a build.
//!
//! `builder-librarian` answers on `brief` even when retrieval failed, marked
//! `degraded` (`templates/builder-librarian/retrieve/config.json`, the terminal
//! arm). The `brief` cell carries that mark on rather than treating it as a
//! failure of its own — retrieval is an ENHANCEMENT — and it TELLS the composer,
//! because a model that was not told it is working without a corpus writes with
//! the confidence of one that was given one.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_one, shipped_script};

const BRIEF: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/brief/config.json"
);

const LIBRARIAN_REF: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/librarian/config.json"
);

fn run_brief(hop: Value, messages: Value) -> Value {
    emit_one(
        &shipped_script(BRIEF),
        &json!({
            "target": "/os/builder/brief",
            "header": {"hop": hop, "context": {}},
            "ttl": 64,
            "messages": messages,
        }),
    )
}

fn instructions_of(out: &Value) -> String {
    out["system"]["instructions"]["text"]
        .as_str()
        .expect("system.instructions.text — the shape builder-hive/brief ships")
        .to_string()
}

#[test]
fn a_degraded_briefing_still_reaches_the_composer_and_says_so() {
    let out = run_brief(
        json!({"route": "brief", "stage": "briefed", "hits": 0, "degraded": true}),
        json!([
            {"origin": "user", "type": "text", "id": "", "text": "build me a digest pipeline"},
            {"origin": "tool", "type": "tool_result", "id": "",
             "text": "(retrieval unavailable: query_timeout)"}
        ]),
    );
    assert_eq!(out["header"]["route"], json!("compose"));
    assert_eq!(out["header"]["degraded"], json!(true));
    assert!(
        instructions_of(&out).contains("no patterns"),
        "the composer must be TOLD it is working without the corpus, or it writes \
         with the confidence of somebody who was given one"
    );
    assert_eq!(
        out["messages"][0]["text"],
        json!("build me a digest pipeline"),
        "the question survives the briefing — a pile of patterns with no question \
         is what phase B of the librarian exists to prevent"
    );
}

#[test]
fn an_empty_result_set_is_degraded_even_though_the_lookup_succeeded() {
    // The librarian's SUCCESS arm answers "(no matching patterns)" with no
    // `degraded` mark. Reading that as a corpus is how a model gets told to
    // lean on nothing while believing it was handed something.
    let out = run_brief(
        json!({"route": "brief", "stage": "briefed", "hits": 0}),
        json!([
            {"origin": "user", "type": "text", "id": "", "text": "grow me a thing"},
            {"origin": "tool", "type": "tool_result", "id": "", "text": "(no matching patterns)"}
        ]),
    );
    assert_eq!(out["header"]["degraded"], json!(true));
    assert!(instructions_of(&out).contains("no patterns"));
}

#[test]
fn a_real_briefing_reaches_the_composer_undegraded_and_carries_the_patterns() {
    let out = run_brief(
        json!({"route": "brief", "stage": "briefed", "hits": 3}),
        json!([
            {"origin": "user", "type": "text", "id": "", "text": "hang a drain on the error lane"},
            {"origin": "tool", "type": "tool_result", "id": "",
             "text": "### config.md -- required_drains (spec) [d-17]\na drain is …"}
        ]),
    );
    assert_eq!(out["header"]["route"], json!("compose"));
    assert_eq!(out["header"]["degraded"], json!(false));
    let text = instructions_of(&out);
    assert!(
        text.contains("required_drains"),
        "the patterns reach the composer"
    );
    assert!(
        text.contains("add_nodes") && text.contains("move_nodes"),
        "the composer is told the diff keys that EXIST — a manifest naming an \
         invented operation is refused at position k, after k-1 have applied"
    );
}

#[test]
fn the_corpus_is_referenced_and_never_copied() {
    // ADR-0011. The corpus is a build product of docs/, the cookbook and the
    // template catalogue; copying it would mean keeping it current twice, and
    // GH #205's lesson is that a stale corpus is worse than none because BM25
    // ranks a wrong answer exactly as high as a true one.
    let raw = std::fs::read_to_string(LIBRARIAN_REF).expect("the librarian ref");
    let cfg: Value = meclaw_core::serde_json::from_str(&raw).expect("json");
    assert_eq!(cfg["cell"]["type"], json!("ref"));
    let template = cfg["cell"]["template"].as_str().expect("a template");
    assert!(
        template.starts_with("builder-librarian@"),
        "expected a pinned builder-librarian, got {template}"
    );
    assert!(
        template.contains('@'),
        "a bare name resolves to the highest version and would adopt drift \
         silently (templates/meclaw-os/template.json § THE TWO REFS)"
    );
}

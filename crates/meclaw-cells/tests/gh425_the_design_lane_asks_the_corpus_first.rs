//! GH #425 — the design lane asks the corpus, and a corpus that is down must
//! not be able to hang a build.
//!
//! **What moved.** The lane used to PREFETCH one lookup into the briefing, so
//! degradation was a property of the single pass and `brief` stamped
//! `hop.degraded`. It is a tool loop now: the corpus is one of four eyes the
//! model may call, and degradation is a property of ONE ROUND, observed where
//! the round happens. The two tests that asserted the mark at `brief` —
//! `a_degraded_briefing_still_reaches_the_composer_and_says_so` and
//! `an_empty_result_set_is_degraded_even_though_the_lookup_succeeded` — live at
//! the `lib` cell now, as
//! `a_degraded_brief_is_an_observation_and_still_answers`
//! (`crates/meclaw-cells/tests/builder_lib_adapts_both_ways.rs`).
//!
//! **What stayed here.** That the briefing reaches the composer with the
//! question intact and the diff vocabulary named, and that the corpus is
//! REFERENCED and never copied — the two statements this file has always been
//! about that a tool loop does not touch.

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
        .expect("system.instructions.text — the shape the brief step ships")
        .to_string()
}

#[test]
fn a_briefing_reaches_the_composer_carrying_the_question_and_the_vocabulary() {
    // Recalibrated with the tool loop: there is no prefetched corpus in this
    // body any more, so what is asserted is what the seeder still OWES the
    // composer — the route, the thread it belongs to, the diff vocabulary, and
    // the question itself. A pile of patterns with no question is what phase B
    // of the librarian existed to prevent, and that has not changed.
    let out = run_brief(
        json!({"route": "brief", "stage": "briefed"}),
        json!([
            {"origin": "user", "type": "text", "id": "", "text": "hang a drain on the error lane"}
        ]),
    );
    assert_eq!(out["header"]["route"], json!("compose"));
    assert!(
        out["header"]["build_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty()),
        "a build is a thread now; every round has to be able to name what it \
         belongs to"
    );
    let text = instructions_of(&out);
    assert!(
        text.contains("add_nodes") && text.contains("move_nodes"),
        "the composer is told the diff keys that EXIST — a manifest naming an \
         invented operation is refused at position k, after k-1 have applied"
    );
    assert_eq!(
        out["messages"][0]["text"],
        json!("hang a drain on the error lane"),
        "the question survives the briefing"
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

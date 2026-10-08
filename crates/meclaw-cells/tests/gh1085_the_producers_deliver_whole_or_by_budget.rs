//! GH #1085 (R-IG-1) -- the producers behind the curator deliver their content
//! whole, or cut to the budget the request carries, with a mark that names
//! what was shown and the total.
//!
//! Before this change six cells cut content on its way to a model by a fixed
//! number, whatever the window of the model that reads it: `summarizer/prep`
//! every older turn and every tool preview to 200 characters,
//! `memory-hive/tool` its result to 8000 (the mark only on the header),
//! `memory-hive/recall` every item to 400 (at most 800) and every tier-0
//! episode to 400, `objects/brief` the brief to 600 characters and three facts
//! of 200, `objects/push` the facts to three of 200, and the object graph its
//! one-liners to 300. Now the only number is the request's `input_soft`
//! (context, `recall_input_soft` as its alias) and a share of it (design § 2):
//! 50 % for the summarizer's own call, 10 % for a tool result, 5 % for the
//! memory bundle, whose items have no length of their own.
//!
//! Fix round (GH #1085, the window of the RECEIVING model): a producer no
//! request names a window to falls back to `input_soft_fallback`, the catalog
//! row of its reader -- the summarizer's writer and an asker of the memory
//! without a curator both take the smallest chat window of the catalog
//! (237500 tokens) -- and the summarizer learns ONCE from a writer that
//! refuses its prompt as over `input_hard`. The tier-0 bundle grows with the
//! window by the factor of tier 1, and the graph space keeps a one-liner whole.
//!
//! The locks, per producer: content over the old number arrives whole without
//! a budget; content over the budget arrives cut WITH the mark
//! `...[cut: <shown> of <total> chars shown; <hint>]`. Every probe runs the
//! shipped `params.script_inline` (never a copy). No colony, no store, no model.

#[path = "support/objects_hive.rs"]
mod objects_hive;

use meclaw_core::serde_json::{self as sj, Value, json};
use meclaw_testing::{emit_all, shipped_script};

const PREP: &str = "../../templates/summarizer/prep/config.json";
const TOOL: &str = "../../templates/memory-hive/tool/config.json";
const RECALL: &str = "../../templates/memory-hive/recall/config.json";
const BRIEF: &str = "../../templates/objects/brief/config.json";
const INDEX: &str = "../../templates/graph-space/index/config.json";

/// OR-IG-9: `input_soft` of the row the writer is born on (`ctx.model`, a
/// birth token, so the `light` tier `openai/gpt-6-luna`) -- what prep's
/// fallback carries; the drift lock over every `input_soft_fallback` holds it
/// to the row prep names.
const WRITER_SOFT: usize = 250_000;

/// A fallback an operator sets to a wide row (237 500 tokens, the Anthropic
/// rows): the window the recall bundles are sized by when no ask names one.
/// The shipped recall fallback is the smallest chat row, under the old floor.
const WIDE_FALLBACK: usize = 237_500;

/// The smallest chat row of the shipped catalog (active or explicit, chat
/// wire): the row recall's fallback names, since an ask without a window has
/// no born model the memory knows (OR-IG-9).
fn smallest_chat_row() -> (String, u64) {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates/llm-registry/store/seed/models.jsonl");
    std::fs::read_to_string(&p)
        .expect("models.jsonl")
        .lines()
        .filter_map(|l| sj::from_str::<Value>(l).ok())
        .filter(|r| matches!(r["status"].as_str(), Some("active" | "explicit")))
        .filter(|r| {
            matches!(
                r["wire_dialect"].as_str(),
                Some("chat_completions" | "responses")
            )
        })
        .filter_map(|r| {
            Some((
                r["model_id"].as_str()?.to_string(),
                r["input_soft"].as_u64()?,
            ))
        })
        .min_by_key(|(id, soft)| (*soft, id.clone()))
        .expect("a chat row")
}

/// Every chat row whose `input_soft` is the smallest one: two rows of one
/// window size (`openai/gpt-4o` and `openai/gpt-4o-mini`, GH #1097) are both
/// the smallest, and a shipped fallback may name either.
fn smallest_chat_rows() -> Vec<String> {
    let (_, soft) = smallest_chat_row();
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates/llm-registry/store/seed/models.jsonl");
    std::fs::read_to_string(&p)
        .expect("models.jsonl")
        .lines()
        .filter_map(|l| sj::from_str::<Value>(l).ok())
        .filter(|r| matches!(r["status"].as_str(), Some("active" | "explicit")))
        .filter(|r| {
            matches!(
                r["wire_dialect"].as_str(),
                Some("chat_completions" | "responses")
            )
        })
        .filter(|r| r["input_soft"].as_u64() == Some(soft))
        .filter_map(|r| r["model_id"].as_str().map(str::to_string))
        .collect()
}

/// Run the shipped script of `config` with `params`, swallow its exit, then
/// `probe` in the same globals; stdout of the probe.
fn probe(config: &str, params: Value, probe: &str) -> String {
    let program = format!(
        concat!(
            "import sys, io\n",
            "_real = {}\n",
            "_out = sys.stdout\n",
            "sys.stdout = io.StringIO()\n",
            "try:\n",
            "    exec(compile(_real, 'cell', 'exec'), globals())\n",
            "except SystemExit:\n",
            "    pass\n",
            "finally:\n",
            "    sys.stdout = _out\n",
            "{}"
        ),
        sj::to_string(&shipped_script(config)).expect("script"),
        probe
    );
    let stdin = json!({"envelope": {}, "body": {}, "params": params}).to_string();
    let out = meclaw_testing::run_shipped_script(&program, &stdin);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn shipped_param(config: &str, key: &str) -> Value {
    let cfg: Value = sj::from_str(&std::fs::read_to_string(config).expect("config")).expect("json");
    cfg["params"][key].clone()
}

/// `n` characters of filler, then `tail`, so a cut is visible as a lost tail.
fn long(n: usize, tail: &str) -> String {
    format!("{}{tail}", "x".repeat(n))
}

fn mark_total(text: &str, total: usize) -> bool {
    text.contains("...[cut: ") && text.contains(&format!(" of {total} chars shown; "))
}

// ═════════════════════════════════════════════════════════ summarizer/prep

fn turn(origin: &str, text: &str) -> Value {
    json!({"origin": origin, "type": "text", "text": text})
}

fn batch(context: Value, turns: Vec<Value>, rounds: Value) -> Value {
    json!({"header": {"context": context,
                      "hop": {"route": "in_batch", "session_id": "s1"}},
           "messages": turns, "rounds": rounds, "params": {"recent_turns": 1}})
}

fn prompt(out: &[Value]) -> String {
    assert_eq!(out.len(), 1, "one batch, one prompt: {out:?}");
    out[0]["messages"][0]["text"]
        .as_str()
        .expect("prompt text")
        .to_string()
}

fn tool_round(text: &str) -> Value {
    json!([{"turn_id": "t1", "iter": 0, "role": "tool",
            "turn": {"origin": "tool", "type": "tool_result", "id": "c1", "text": text},
            "fired": 1}])
}

#[test]
fn the_summarizer_gets_older_turns_and_tool_texts_whole_without_a_budget() {
    let old = long(1000, "OLD-TAIL");
    let result = long(1000, "TOOL-TAIL");
    let p = prompt(&emit_all(
        &shipped_script(PREP),
        &batch(
            json!({}),
            vec![turn("user", &old), turn("assistant", "recent answer")],
            tool_round(&result),
        ),
    ));
    assert!(
        p.contains(&old),
        "an older turn over the old 200 arrives whole"
    );
    assert!(
        p.contains(&result),
        "a tool text over the old 200 arrives whole"
    );
    assert!(p.contains("recent answer"), "{p}");
    assert!(!p.contains("[cut:"), "nothing is cut without a budget: {p}");
}

fn batch_with(context: Value, turns: Vec<Value>, rounds: Value, fallback: u64) -> Value {
    let mut b = batch(context, turns, rounds);
    b["params"]["input_soft_fallback"] = json!(fallback);
    b
}

#[test]
fn the_summarizer_cuts_to_half_its_writers_window_with_a_mark() {
    // The writer's window 400 tokens: half of it, at 3 characters a token, is 600.
    let p = prompt(&emit_all(
        &shipped_script(PREP),
        &batch_with(
            json!({}),
            vec![
                turn("user", &long(1000, "A-TAIL")),
                turn("assistant", &long(1000, "B-TAIL")),
                turn("user", "the recent question"),
            ],
            tool_round(&long(1000, "TOOL-TAIL")),
            400,
        ),
    ));
    assert!(
        p.chars().count() <= 600,
        "the prompt keeps to its budget ({} chars): {p}",
        p.chars().count()
    );
    assert!(
        p.contains("the recent question"),
        "the recent turns go first: {p}"
    );
    assert!(
        mark_total(&p, 1006),
        "a cut item says what was shown of how much: {p}"
    );
    assert!(!p.contains("A-TAIL") && !p.contains("TOOL-TAIL"), "{p}");
}

#[test]
fn the_summarizer_never_takes_the_window_of_the_conversation() {
    // Fix review G1 M2 (OR-IG-4): `input_soft` / `recall_input_soft` on the
    // request are the window of the model that ASKED (the conversation), not
    // of `./writer`, which reads the prompt. A small conversation window must
    // not cut the day for a writer with a wider one.
    for key in ["input_soft", "recall_input_soft"] {
        let mut ctx = sj::Map::new();
        ctx.insert(key.into(), json!(400));
        let a = long(1000, "A-TAIL");
        let p = prompt(&emit_all(
            &shipped_script(PREP),
            &batch_with(
                Value::Object(ctx),
                vec![turn("user", &a), turn("user", "the recent question")],
                tool_round(&long(1000, "TOOL-TAIL")),
                0,
            ),
        ));
        assert!(
            p.contains(&a) && p.contains("TOOL-TAIL") && !p.contains("[cut:"),
            "{key}: the conversation's window changes nothing: {p}"
        );
    }
}

#[test]
fn the_summarizer_without_a_request_window_keeps_to_half_of_its_writers_catalog_row() {
    // The close batch carries no `input_soft` (no curator asks the
    // summarizer): the shipped fallback is the writer's window. A day of
    // 40 older turns of 10 000 characters is 400 000 characters; half of
    // 250 000 tokens, at three characters a token, is 375 000.
    assert_eq!(
        shipped_param(PREP, "input_soft_fallback"),
        json!(WRITER_SOFT)
    );
    assert_eq!(
        shipped_param(PREP, "input_soft_fallback_row"),
        json!("openai/gpt-6-luna"),
        "the row the writer is born on (OR-IG-9)"
    );
    let mut turns: Vec<Value> = (0..40)
        .map(|i| turn("user", &long(10_000, &format!("OLD-{i}-TAIL"))))
        .collect();
    turns.push(turn("user", "the recent question"));
    let p = prompt(&emit_all(
        &shipped_script(PREP),
        &json!({"header": {"context": {}, "hop": {"route": "in_batch", "session_id": "s1"}},
                "messages": turns, "rounds": [],
                "params": {"recent_turns": 1, "input_soft_fallback": WRITER_SOFT}}),
    ));
    let budget = WRITER_SOFT * 3 / 2;
    assert!(
        p.chars().count() <= budget,
        "{} > {budget}",
        p.chars().count()
    );
    assert!(
        p.contains("the recent question"),
        "the recent turn goes first"
    );
    assert!(
        mark_total(&p, 10_010),
        "an older turn says what of how much"
    );
    assert!(!p.contains("OLD-0-TAIL"), "and is cut, never silently");
}

/// The writer's refusal of a prompt over its `input_hard`, as the llm cell
/// sends it back: the refused prompt in `messages`, the number in
/// `meta.error`.
fn refusal(context: Value, refused: &[Value], hard: u64) -> Value {
    json!({"header": {"context": context,
                      "hop": {"route": "in_error", "finish_reason": "error",
                              "error_code": "invalid_input"}},
           "messages": refused,
           "meta": {"error": {"source": "window", "kind": "input_over_hard",
                              "input_estimate": 99_999, "input_hard": hard}},
           "params": {}})
}

#[test]
fn the_summarizer_learns_once_from_a_writer_that_refuses_the_prompt() {
    let first = emit_all(
        &shipped_script(PREP),
        &batch(
            json!({}),
            vec![
                turn("user", &long(6000, "A-TAIL")),
                turn("assistant", &long(6000, "B-TAIL")),
                turn("user", "the recent question"),
            ],
            tool_round(&long(6000, "TOOL-TAIL")),
        ),
    );
    let sent = prompt(&first);
    assert!(sent.contains("A-TAIL"), "the first prompt is whole: {sent}");
    // input_hard 2000 tokens: half of it, at three characters a token, is 3000.
    let ctx = json!({"session_id": "s1"});
    let again = emit_all(
        &shipped_script(PREP),
        &refusal(
            ctx,
            first[0]["messages"].as_array().expect("messages"),
            2000,
        ),
    );
    assert_eq!(again.len(), 1, "{again:?}");
    assert_eq!(again[0]["header"]["route"], "llm", "{again:?}");
    assert_eq!(again[0]["header"]["window_refit"], "1");
    assert_eq!(again[0]["header"]["session_id"], "s1");
    let p = prompt(&again);
    assert!(
        p.chars().count() <= 3000,
        "{} chars: {p}",
        p.chars().count()
    );
    assert!(p.contains("the recent question"), "{p}");
    assert!(
        mark_total(&p, 6006),
        "the mark names the length before any cut: {p}"
    );
    assert!(p.starts_with("Session s1 closed with 3 turns."), "{p}");
    // Once: a second refusal leaves on the error lane.
    let twice = emit_all(
        &shipped_script(PREP),
        &refusal(
            json!({"session_id": "s1", "window_refit": "1"}),
            again[0]["messages"].as_array().expect("messages"),
            500,
        ),
    );
    assert_eq!(twice[0]["header"]["route"], "summary_error", "{twice:?}");
    assert_eq!(twice[0]["header"]["error_code"], "invalid_input");
}

// ═════════════════════════════════════════════════════════ memory-hive/tool

fn tool_answer(context: Value, text: &str) -> Value {
    let mut ctx = context.as_object().cloned().unwrap_or_default();
    ctx.insert("memory_call_id".into(), json!("call-1"));
    json!({"header": {"context": ctx, "hop": {"route": "bundle"}},
           "messages": [{"origin": "tool", "type": "tool_result", "id": "recall",
                         "text": text}]})
}

fn result_of(out: &[Value]) -> (String, String) {
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(out[0]["header"]["route"], "tool_result");
    (
        out[0]["messages"][0]["text"]
            .as_str()
            .expect("text")
            .to_string(),
        out[0]["header"]["memory_capped"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    )
}

#[test]
fn the_memory_tool_result_arrives_whole_without_a_budget() {
    let text = long(20_000, "BUNDLE-TAIL");
    let (got, capped) = result_of(&emit_all(
        &shipped_script(TOOL),
        &tool_answer(json!({}), &text),
    ));
    assert_eq!(got, text, "a result over the old 8000 arrives whole");
    assert_eq!(capped, "0");
}

#[test]
fn the_memory_tool_result_is_cut_to_a_tenth_of_the_window_with_the_mark_in_the_text() {
    // recall_input_soft 1000 tokens: a tenth of it, at 3 characters a token, is 300.
    let text = long(20_000, "BUNDLE-TAIL");
    let (got, capped) = result_of(&emit_all(
        &shipped_script(TOOL),
        &tool_answer(json!({"recall_input_soft": "1000"}), &text),
    ));
    assert!(got.starts_with(&"x".repeat(300)), "{got}");
    assert!(
        got.contains("...[cut: 300 of 20011 chars shown; "),
        "the mark is IN the text the model reads: {got}"
    );
    assert!(!got.contains("BUNDLE-TAIL"));
    assert_eq!(capped, "1", "and the header still says it");
}

// ═════════════════════════════════════════════════════════ memory-hive/recall

fn fact_row(id: &str, claim: &str) -> Value {
    json!({"id": id, "session_id": "s-1", "subject": "person:example",
           "predicate": "prefers", "claim": claim,
           "canonical_subject": "person:example", "canonical_predicate": "prefers",
           "canonical_claim": claim, "valid_from": "2026-01-05T09:00:00.000000Z",
           "valid_until": null, "recorded_at": "2026-01-05T09:00:00.000000Z",
           "expired_at": null, "superseded_by": null, "episode_id": "ep-x",
           "fact_kind": "state", "confidence": 90})
}

fn episode_row(id: &str, content: &str) -> Value {
    json!({"id": id, "session_id": "s-1", "sender": "user", "content": content,
           "happened_at": "2026-01-02T09:00:00.000000Z",
           "recorded_at": "2026-01-02T09:00:00.000000Z"})
}

/// The `t1-emit` hop of a tier-1 request: one fact and one episode were fused,
/// and the hydration hands their rows back.
fn t1_emit(params: Value, fact: Value, episode: Value) -> Value {
    let fused = json!({
        "candidates": [
            {"kind": "fact", "id": "f-1", "score": 0.09, "legs": ["keyword"]},
            {"kind": "episode", "id": "e-1", "score": 0.08, "legs": ["keyword"]}
        ],
        "legs_present": ["keyword"], "leg_sizes": {"keyword": 2},
        "leg_capped": {}, "semantic_degraded": false});
    let rows = json!([
        {"request_id": "r1", "leg": "fused", "payload": fused.to_string(), "fired": 1},
        {"request_id": "r1", "leg": "hyd-ep", "payload": json!([episode]).to_string(), "fired": 1},
        {"request_id": "r1", "leg": "hyd-fact", "payload": json!([fact]).to_string(), "fired": 1},
        {"request_id": "r1", "leg": "hyd-axis", "payload": "[]", "fired": 1}
    ]);
    json!({"header": {"context": {"mem_phase": "t1-emit", "recall_id": "r1", "memory_tier": "1",
                                  "recall_query": "what do I prefer?",
                                  "recall_as_of": "2026-08-12T00:00:00Z",
                                  "recall_window_from": "", "recall_window_to": ""},
                      "hop": {"operation": "select"}},
           "messages": [{"origin": "tool", "type": "tool_result", "id": "r",
                         "text": rows.to_string()}],
           "params": params})
}

fn bundle_of(out: &[Value]) -> (Value, String) {
    let m = out
        .iter()
        .find(|m| m["header"]["route"] == "bundle")
        .unwrap_or_else(|| panic!("no bundle: {out:?}"));
    let payload: Value = sj::from_str(
        m["system"]["memory"]["bundle"]["text"]
            .as_str()
            .expect("bundle text"),
    )
    .expect("bundle json");
    let text = m["messages"]
        .as_array()
        .and_then(|a| a.last())
        .and_then(|t| t["text"].as_str())
        .unwrap_or_default()
        .to_string();
    (payload, text)
}

#[test]
fn a_tier1_item_over_the_old_400_arrives_whole() {
    let claim = long(1500, "CLAIM-TAIL");
    let content = long(1500, "EPISODE-TAIL");
    let (payload, text) = bundle_of(&emit_all(
        &shipped_script(RECALL),
        &t1_emit(
            json!({}),
            fact_row("f-1", &claim),
            episode_row("e-1", &content),
        ),
    ));
    let texts: Vec<&str> = payload["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .filter_map(|c| c["text"].as_str())
        .collect();
    assert!(texts.contains(&claim.as_str()), "{payload}");
    assert!(texts.contains(&content.as_str()), "{payload}");
    assert!(
        text.contains("CLAIM-TAIL") && text.contains("EPISODE-TAIL"),
        "{text}"
    );
    assert!(!text.contains("[cut:"), "{text}");
}

#[test]
fn a_tier1_item_over_the_bundle_budget_is_cut_with_a_mark_not_dropped() {
    // A bundle of 200 tokens and a first-ranked fact of 3000 characters: the
    // fact alone does not fit the bundle, so it is cut with a mark -- never
    // an empty bundle, never a fact that silently loses its tail.
    let claim = long(3000, "CLAIM-TAIL");
    let (payload, text) = bundle_of(&emit_all(
        &shipped_script(RECALL),
        &t1_emit(
            json!({"tier1_tokens": 200}),
            fact_row("f-1", &claim),
            episode_row("e-1", "short"),
        ),
    ));
    let first = payload["candidates"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("the first item is seated: {payload}"));
    assert!(mark_total(first, 3010), "{first}");
    assert!(!first.contains("CLAIM-TAIL"), "{first}");
    assert!(
        payload["token_estimate"].as_u64().unwrap_or(u64::MAX) <= 200,
        "the bundle keeps its budget: {payload}"
    );
    assert!(
        mark_total(&text, 3010),
        "the rendered line carries it too: {text}"
    );
}

#[test]
fn an_ask_without_a_window_sizes_the_bundle_by_the_smallest_catalog_row() {
    // No curator asks (no `input_soft`). OR-IG-9: the shipped fallback is the
    // smallest chat row -- 5 % of it is under the old 2000 tokens, so the
    // bundle keeps them. A fallback set to a wide row (237 500) gives 11 875
    // tokens: a fact of 30 000 characters (about 10 000 tokens) arrives
    // whole; one of 60 000 is cut with its mark and the bundle keeps its
    // budget.
    let (_, soft) = smallest_chat_row();
    assert_eq!(shipped_param(RECALL, "input_soft_fallback"), json!(soft));
    let row = shipped_param(RECALL, "input_soft_fallback_row");
    assert!(
        smallest_chat_rows().iter().any(|id| json!(id) == row),
        "the fallback row {row} is not one of the smallest chat rows {:?}",
        smallest_chat_rows()
    );
    let floor = probe(
        RECALL,
        json!({}),
        &format!("print(package_limits({soft})['tier1_tokens'])"),
    );
    assert_eq!(floor, "2000", "5 % of {soft} is under the old floor");
    let shipped = json!({"input_soft_fallback": WIDE_FALLBACK});
    let claim = long(30_000, "CLAIM-TAIL");
    let (payload, _) = bundle_of(&emit_all(
        &shipped_script(RECALL),
        &t1_emit(
            shipped.clone(),
            fact_row("f-1", &claim),
            episode_row("e-1", "short"),
        ),
    ));
    assert_eq!(payload["candidates"][0]["text"], json!(claim), "{payload}");
    let claim = long(60_000, "CLAIM-TAIL");
    let (payload, _) = bundle_of(&emit_all(
        &shipped_script(RECALL),
        &t1_emit(
            shipped,
            fact_row("f-1", &claim),
            episode_row("e-1", "short"),
        ),
    ));
    let first = payload["candidates"][0]["text"].as_str().expect("seated");
    assert!(mark_total(first, 60_010), "{first}");
    assert!(payload["token_estimate"].as_u64().unwrap_or(u64::MAX) <= 11_875);
}

#[test]
fn the_tier0_bundle_grows_with_the_window_like_tier1() {
    // package_limits: one factor for both bundles, tier 0 at 1200 x factor.
    let sizes = probe(
        RECALL,
        json!({}),
        "L0, L1 = package_limits(None), package_limits(250000)\n\
         print(L0['tier0_tokens'], L1['tier0_tokens'], L1['tier1_tokens'])",
    );
    assert_eq!(sizes, "1200 7500 12500", "factor 6.25 for both");
    // A number set on the knob wins over the window.
    let fixed = probe(
        RECALL,
        json!({"tier0_tokens": 900}),
        "print(package_limits(250000)['tier0_tokens'])",
    );
    assert_eq!(fixed, "900");
}

#[test]
fn the_package_names_no_item_length_any_more() {
    let script = shipped_script(RECALL);
    assert!(
        !script.contains("tier1_item_chars") && !script.contains("tier0_episode_chars"),
        "no per-item length is left in the recall cell"
    );
    let cfg: Value = sj::from_str(&std::fs::read_to_string(RECALL).expect("config")).expect("json");
    for knob in ["tier1_item_chars", "tier0_episode_chars"] {
        assert!(cfg["params"].get(knob).is_none(), "params.{knob}");
        assert!(
            cfg["params"]["contract"]["settings"].get(knob).is_none()
                && cfg["contract"]["settings"].get(knob).is_none(),
            "contract.settings.{knob}"
        );
    }
}

/// An episode row of the tier-0 leg page, in the round the request asks in --
/// the audience gate runs before the budget, and a row it hides takes no seat.
fn tier0_episode(id: &str, content: &str) -> Value {
    let mut row = episode_row(id, content);
    row["channel"] = json!("group:trip");
    row["audience_set"] = json!(r#"["member:alex"]"#);
    row
}

fn tier0(params: Value, episodes: Value) -> Value {
    json!({"header": {"context": {"mem_phase": "legs", "recall_id": "r1", "memory_tier": "0",
                                  "recall_query": "", "recall_as_of": "2026-09-21T00:00:00Z",
                                  "recall_window_from": "", "recall_window_to": "",
                                  "audience_now": ["member:alex"], "channel": "group:trip",
                                  "session_id": "s-1"},
                      "hop": {"operation": "bundle", "rows_affected": 1, "bundle_errors": 0}},
           "messages": [
               {"origin": "tool", "type": "tool_result", "id": "r-leg-episodes",
                "text": episodes.to_string()},
               {"origin": "tool", "type": "tool_result", "id": "r-leg-beliefs", "text": "[]"},
               {"origin": "tool", "type": "tool_result", "id": "r-leg-foresight", "text": "[]"}],
           "results": [
               {"tool_call_id": "r-leg-episodes", "operation": "select", "rows_affected": 1},
               {"tool_call_id": "r-leg-beliefs", "operation": "select", "rows_affected": 0},
               {"tool_call_id": "r-leg-foresight", "operation": "select", "rows_affected": 0}],
           "params": params})
}

#[test]
fn a_tier0_episode_over_the_old_400_arrives_whole_and_one_over_the_budget_is_marked() {
    let content = long(1500, "EPISODE-TAIL");
    let (payload, text) = bundle_of(&emit_all(
        &shipped_script(RECALL),
        &tier0(json!({}), json!([tier0_episode("e-1", &content)])),
    ));
    assert_eq!(
        payload["episodes"][0]["content"],
        json!(content),
        "{payload}"
    );
    assert!(text.contains("EPISODE-TAIL"), "{text}");

    // No window, a fallback set to a wide row: tier 0 grows by the factor of
    // tier 1 (11 875 / 2000) to 7125 tokens. An episode of 30 000 characters does
    // not fit the bundle on its own -- it is cut with a mark, and the next one
    // is named as dropped rather than lost in silence.
    let big = long(30_000, "BIG-TAIL");
    let (payload, text) = bundle_of(&emit_all(
        &shipped_script(RECALL),
        &tier0(
            json!({"input_soft_fallback": WIDE_FALLBACK}),
            json!([
                tier0_episode("e-1", &big),
                tier0_episode("e-2", "second episode")
            ]),
        ),
    ));
    let first = payload["episodes"][0]["content"]
        .as_str()
        .unwrap_or_else(|| panic!("the first episode is seated: {payload}"));
    assert!(mark_total(first, 30_008), "{first}");
    assert!(
        first.len() > 8000,
        "more than the old 1200-token bundle held: {}",
        first.len()
    );
    assert!(payload["token_estimate"].as_u64().unwrap_or(u64::MAX) <= 7125);
    assert!(mark_total(&text, 30_008), "{text}");
    assert!(
        text.contains("...[dropped: 1 episodes over the bundle budget"),
        "a list the budget ended says so in the text: {text}"
    );
}

// ═════════════════════════════════════════════════════════ objects

fn object_row(what: &str) -> Value {
    json!({"id": "ob-0123456789ab", "rev": 4, "type": "thing",
           "aliases": json!(["Blue Bike"]).to_string(), "state": "active",
           "audience_set": r#"["member:p"]"#,
           "slots": json!({"what": what, "where": long(400, "WHERE-TAIL"), "who": [],
                           "when": "", "why": "", "how": ""}).to_string(),
           "refs": json!({"doc": [], "related": []}).to_string()})
}

#[test]
fn a_brief_carries_every_fact_and_every_part_whole() {
    if !objects_hive::shipped() {
        return;
    }
    let facts: Vec<String> = (0..5).map(|i| long(300, &format!("FACT-{i}"))).collect();
    let row = object_row(&long(400, "WHAT-TAIL"));
    let text = objects_hive::pure("brief", "render(ARGS[0], ARGS[1])", json!([row, facts]));
    let text = text.as_str().expect("a text");
    for tail in ["WHAT-TAIL", "WHERE-TAIL", "FACT-0", "FACT-4"] {
        assert!(text.contains(tail), "{tail} arrives: {text}");
    }
    assert!(!text.contains("[cut:"), "{text}");
}

#[test]
fn a_brief_asked_with_a_window_is_cut_to_a_tenth_of_it_with_a_mark() {
    // input_soft 500 tokens: a tenth of it, at 3 characters a token, is 150.
    let row = object_row(&long(400, "WHAT-TAIL"));
    let out = emit_all(
        &shipped_script(BRIEF),
        &json!({"header": {"context": {"input_soft": 500},
                           "hop": {"route": "brief_render", "reply_cell": "tools"}},
                "row": row, "facts": ["a fact"], "messages": []}),
    );
    let text = out[0]["text"].as_str().expect("text");
    assert!(text.starts_with("thing: Blue Bike\nwhat: "), "{text}");
    assert!(text.contains("...[cut: 150 of "), "{text}");
    assert!(!text.contains("WHAT-TAIL"), "{text}");
}

#[test]
fn the_push_hands_every_fact_of_the_answer_on_whole() {
    if !objects_hive::shipped() {
        return;
    }
    let id = "ob-0123456789ab";
    let cands: Vec<Value> = (0..5)
        .map(|i| json!({"kind": "fact", "subject": id, "text": long(500, &format!("F{i}"))}))
        .collect();
    let bundle = json!({"subject": id, "answers": "direct", "complete": true,
                        "candidates": cands});
    let body = json!({"system": {"memory": {"bundle": {"text": bundle.to_string()}}}});
    let facts = objects_hive::pure("push", "facts_of(bundle_of(ARGS))", body);
    let facts = facts.as_array().expect("facts");
    assert_eq!(facts.len(), 5, "no count of its own: {facts:?}");
    assert!(
        facts.iter().all(|f| f.as_str().unwrap_or("").len() == 502),
        "no length of its own: {facts:?}"
    );
    let ask = objects_hive::pure("push", "ask_facts(ARGS)", json!({"id": id}));
    assert!(
        ask.get("limit").is_none(),
        "memory's own page decides how many: {ask}"
    );
}

#[test]
fn an_object_one_liner_arrives_whole_in_both_cells() {
    if !objects_hive::shipped() {
        return;
    }
    let what = long(500, "WHAT-TAIL");
    for cell in ["push", "source"] {
        let nodes = objects_hive::pure(cell, "object_nodes(ARGS)", object_row(&what));
        let one = nodes[0]["oneline"].as_str().expect("oneline");
        assert_eq!(one, format!("thing: {what}"), "{cell}");
        assert_eq!(nodes[1]["name"], "what", "{cell}: {nodes}");
        assert_eq!(
            nodes[1]["oneline"],
            json!(what),
            "{cell}: a slot line, whole"
        );
    }
}

// ═════════════════════════════════════════════════════════ graph-space/index

#[test]
fn the_graph_space_keeps_a_one_liner_whole() {
    // The index cut every announced one-liner to 300 characters and undid the
    // object hive's whole line on the reading side (Review G1b M5).
    let out = probe(
        INDEX,
        json!({}),
        "import json\n\
         n = clean_nodes([{'anchor': 'a', 'oneline': 'x' * 1000 + 'ONE-TAIL'}])\n\
         print(json.dumps(n[0]['oneline'][-8:]), len(n[0]['oneline']))",
    );
    assert_eq!(out, "\"ONE-TAIL\" 1008");
}

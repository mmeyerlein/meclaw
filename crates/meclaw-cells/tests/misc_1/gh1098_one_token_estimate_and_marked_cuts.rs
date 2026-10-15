//! GH #1098 (wave "old limits", bundle MC-S2) -- one token estimate, a tier-0
//! bundle that grows with the window, and cuts that say so.
//!
//! Before, the templates estimated tokens three ways: `./policy` of the
//! curator at UTF-8 bytes / 3 (the llm cell's rule) for the window and at
//! characters / 4 for the aim of a rebuild, the memory hive's recall at
//! `len(json) // 4 + 1` per bundle item and its writer at `len // 4` per
//! queued episode. The recall bundle the curator budgets at 5 % of the window
//! was in the curator's own eyes a third larger than that. Now one block --
//! `token-estimate v1`, copied byte for byte between two marker lines, beside
//! the content-budget block whose CHARS_PER_TOKEN it counts with -- is the
//! only estimate, and a scan finds every copy and every estimate of another
//! rate. What is pinned:
//!
//! 1. every copy of the block is the canonical text, the three cells that
//!    estimate carry it, and every carrier carries the content-budget block
//!    (the one constant); its IMAGE_TOKENS is the llm cell's;
//! 2. no template script estimates tokens at a rate of its own;
//! 3. the tier-0 item counts grow with the asker's window like every tier-1
//!    count, and so do the graph leg's anchors and starts (the starts never
//!    past what one walk page can leave);
//! 4. name anchors and anchor entities the graph leg cut are named in the
//!    bundle (`complete_reason`), not only on stderr;
//! 5. the presenter's topic ceiling is derived from the decider's question
//!    limit, and a built-in topic past it is named; a full page of curator
//!    candidates says `more` behind the candidates' part.

use meclaw_core::serde_json::{self, Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const START: &str = "# >>> token-estimate v1";
const END: &str = "# <<< token-estimate v1";
const CB_START: &str = "# >>> content-budget v1";

/// The block, from its opening marker line to its closing one.
const CANONICAL: &str = r#"# >>> token-estimate v1 (GH #1098)
# The one token estimate of the templates: a request as the llm cell
# estimates it before it refuses one over `input_hard` (`window.rs`
# `estimate_prompt_tokens`) for a model whose catalogue row states no rates
# of its own -- UTF-8 bytes of every string and object key, 4 for a number
# or a bool, over CHARS_PER_TOKEN of the content-budget block (the same
# three bytes a token every budget of the templates is sized with), rounded
# up; a string starting `data:` (an inline image) counts IMAGE_TOKENS, the
# cell's own `llm::window::IMAGE_TOKENS`.
IMAGE_TOKENS = 1024


def estimate_tokens(value):
    """Tokens of `value` (any JSON value) by the rule above."""
    nbytes, images, todo = 0, 0, [value]
    while todo:
        v = todo.pop()
        if isinstance(v, str):
            if v.startswith("data:"):
                images += 1
            else:
                nbytes += len(v.encode("utf-8"))
        elif isinstance(v, dict):
            for k, x in v.items():
                nbytes += len(str(k).encode("utf-8"))
                todo.append(x)
        elif isinstance(v, (list, tuple)):
            todo.extend(v)
        elif isinstance(v, (bool, int, float)):
            nbytes += 4
    return tokens_of_bytes(nbytes) + images * IMAGE_TOKENS


def tokens_of_bytes(nbytes):
    """Tokens of `nbytes` bytes (or characters) of text: over
    CHARS_PER_TOKEN, rounded up."""
    return -(-max(0, int(nbytes)) // CHARS_PER_TOKEN)
# <<< token-estimate v1"#;

/// The cells that estimate tokens; each must carry the block.
const CARRIERS: &[&str] = &["curator/policy", "memory-hive/recall", "memory-hive/writer"];

const RECALL: &str = "../../templates/memory-hive/recall/config.json";

fn repo(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn script(cell: &str) -> String {
    let raw = std::fs::read_to_string(repo(&format!("templates/{cell}/config.json")))
        .unwrap_or_else(|e| panic!("{cell}: {e}"));
    let v: Value = serde_json::from_str(&raw).expect("config json");
    v["params"]["script_inline"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

/// Every `params.script_inline` under `templates/`, by cell path.
fn every_script() -> Vec<(String, String)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                walk(&p, out);
            } else if p.file_name().and_then(|n| n.to_str()) == Some("config.json") {
                out.push(p);
            }
        }
    }
    let root = repo("templates");
    let mut files = Vec::new();
    walk(&root, &mut files);
    let mut out = Vec::new();
    for f in files {
        let raw = std::fs::read_to_string(&f).expect("config.json");
        let Ok(v) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        if let Some(s) = v["params"]["script_inline"].as_str() {
            let rel = f
                .strip_prefix(&root)
                .unwrap_or(&f)
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            out.push((rel, s.to_string()));
        }
    }
    out
}

/// The copies of the block in `text`, or why a marker is alone.
fn blocks_in(text: &str) -> Vec<Result<String, String>> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(START) {
        let tail = &rest[i..];
        match tail.find(END) {
            Some(j) => {
                out.push(Ok(tail[..j + END.len()].to_string()));
                rest = &tail[j + END.len()..];
            }
            None => {
                out.push(Err("an opening marker without its closing one".into()));
                rest = "";
            }
        }
    }
    out
}

/// Run `program` with the shipped `script` loaded first (its `park()` exit
/// swallowed); stdout of the program, trimmed.
fn probe(script: &str, params: Value, program: &str) -> String {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_real = {}\n",
            "_sink, _out = io.StringIO(), sys.stdout\n",
            "sys.stdout = _sink\n",
            "try:\n",
            "    exec(compile(_real, 'cell', 'exec'), globals())\n",
            "except SystemExit:\n",
            "    pass\n",
            "finally:\n",
            "    sys.stdout = _out\n",
            "{}"
        ),
        serde_json::to_string(script).unwrap(),
        program
    );
    let stdin = json!({"envelope": {}, "body": {}, "params": params}).to_string();
    let out = meclaw_testing::run_shipped_script(&src, &stdin);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// The shipped recall cell on one stdin document; its emitted messages.
fn recall(doc: &Value) -> Vec<Value> {
    let script = meclaw_testing::shipped_script(RECALL);
    let stdin = meclaw_testing::code_stdin(doc).to_string();
    let out = meclaw_testing::run_shipped_script(&script, &stdin);
    assert!(
        out.status.success(),
        "recall exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    match serde_json::from_slice::<Value>(&out.stdout).expect("stdout is json") {
        Value::Array(a) => a,
        other => vec![other],
    }
}

/// Every store op the emitted bundles carry, in call order.
fn ops(msgs: &[Value]) -> Vec<Value> {
    let mut out = Vec::new();
    for m in msgs {
        for t in m["messages"].as_array().into_iter().flatten() {
            if let Ok(v) = serde_json::from_str::<Value>(t["text"].as_str().unwrap_or("null")) {
                out.push(v);
            }
        }
    }
    out
}

// ============================================================ 1. one block

#[test]
fn every_copy_of_the_estimate_is_the_canonical_text_beside_the_one_constant() {
    let mut copies = 0;
    let mut findings = Vec::new();
    for (cell, s) in every_script() {
        let blocks = blocks_in(&s);
        for b in &blocks {
            match b {
                Ok(copy) if copy == CANONICAL => copies += 1,
                Ok(_) => findings.push(format!("{cell}: the token-estimate block drifted")),
                Err(why) => findings.push(format!("{cell}: {why}")),
            }
        }
        if !blocks.is_empty() {
            // The block counts with CHARS_PER_TOKEN; it is only ever the
            // content-budget block's, defined before the estimate runs.
            match (s.find(CB_START), s.find(START)) {
                (Some(cb), Some(te)) if cb < te => {}
                _ => findings.push(format!(
                    "{cell}: the token-estimate block without the content-budget block before it"
                )),
            }
        }
    }
    for cell in CARRIERS {
        if blocks_in(&script(cell)).len() != 1 {
            findings.push(format!(
                "{cell}: estimates tokens and carries no single block"
            ));
        }
    }
    assert!(findings.is_empty(), "{findings:#?}");
    assert!(copies >= CARRIERS.len(), "{copies} copies");
}

#[test]
fn the_image_rate_is_the_llm_cells() {
    let window =
        std::fs::read_to_string(repo("crates/meclaw-cells/src/llm/window.rs")).expect("window.rs");
    let line = window
        .lines()
        .find(|l| l.contains("const IMAGE_TOKENS: u64 ="))
        .expect("IMAGE_TOKENS in window.rs");
    let rust: u64 = line
        .rsplit('=')
        .next()
        .unwrap()
        .trim()
        .trim_end_matches(';')
        .replace('_', "")
        .parse()
        .expect("a number");
    assert!(
        CANONICAL.contains(&format!("\nIMAGE_TOKENS = {rust}\n")),
        "the block's IMAGE_TOKENS is window.rs's {rust}"
    );
}

#[test]
fn the_estimate_counts_like_the_llm_cell() {
    // `window.rs` `estimate_prompt_tokens` without a row: bytes of every
    // string and key / 3 rounded up, 4 bytes a number or a bool, 1024 an
    // inline image; the same in every carrier.
    for cell in CARRIERS {
        let got = probe(
            &script(cell),
            json!({}),
            "print([estimate_tokens({'ab': 'xyz'}), estimate_tokens(['\\u00e4']), \
             estimate_tokens({'u': 'data:image/png;base64,AAAA'}), \
             estimate_tokens([1, True, 2.5, None]), tokens_of_bytes(7), tokens_of_bytes(0)])",
        );
        assert_eq!(got, "[2, 1, 1025, 4, 3, 0]", "{cell}");
    }
}

// =================================================== 2. no rate of its own

#[test]
fn no_template_estimates_tokens_at_a_rate_of_its_own() {
    // What the three estimates of before looked like: a length over four, and
    // a per-token constant beside the content-budget block's.
    let mut findings = Vec::new();
    for (cell, s) in every_script() {
        for (n, line) in s.lines().enumerate() {
            let code = line.split('#').next().unwrap_or("");
            let quarter = code.contains("len(") && {
                let t: String = code.split_whitespace().collect();
                t.contains(")//4") || t.contains(")/4") || t.contains("/4.0")
            };
            let own_rate = code.trim_start().starts_with("BYTES_PER_TOKEN")
                || code.contains("EST_CHARS_PER_TOKEN");
            if quarter || own_rate {
                findings.push(format!("{cell}:{}: {}", n + 1, line.trim()));
            }
        }
    }
    assert!(findings.is_empty(), "{findings:#?}");
}

#[test]
fn a_queued_episode_is_estimated_by_the_block() {
    let s = script("memory-hive/writer");
    assert!(
        s.contains("\"token_est\": max(1, estimate_tokens(content))"),
        "the writer's queue item counts its tokens with the block"
    );
}

// ======================================== 3. tier 0 and the graph leg grow

#[test]
fn the_tier0_counts_and_the_graph_leg_grow_with_the_window() {
    let s = meclaw_testing::shipped_script(RECALL);
    let read = "print(MAX_EPISODES, MAX_BELIEFS, MAX_FORESIGHT, GRAPH_ANCHORS, GRAPH_START)";
    // No window: the counts of before, exactly.
    assert_eq!(
        probe(
            &s,
            json!({}),
            &format!("apply_package_limits(None)\n{read}")
        ),
        "12 20 10 96 32"
    );
    // luna (input_soft 250k): the factor of tier 1, 6.25; the starts stop at
    // what one walk page of 200 nodes at depth 2 can leave, 200 // 3.
    assert_eq!(
        probe(
            &s,
            json!({}),
            &format!("apply_package_limits(250000)\n{read}")
        ),
        "75 125 63 600 66"
    );
    // A number set on the cell wins over the window.
    assert_eq!(
        probe(
            &s,
            json!({"tier0_max_episodes": 5, "tier1_graph_start": 4}),
            &format!("apply_package_limits(250000)\n{read}")
        ),
        "5 125 63 600 4"
    );
}

#[test]
fn a_tier0_request_asks_the_store_as_deep_as_its_window() {
    let doc = |soft: &str| {
        json!({
            "header": {"context": {"recall_id": "", "mem_phase": "", "memory_tier": "0",
                                   "recall_query": "What did we talk about?",
                                   "recall_input_soft": soft,
                                   "audience_now": "[\"member:alex\"]", "channel": "tg:private"},
                       "hop": {"route": "in_query", "phase": "recall"}},
            "messages": []
        })
    };
    let limits = |soft: &str| -> Vec<Value> {
        let mut out = Vec::new();
        for op in ops(&recall(&doc(soft))) {
            if op["operation"] == "select"
                && ["episodes", "beliefs", "facts"].contains(&op["table"].as_str().unwrap_or(""))
            {
                out.push(op["limit"].clone());
            }
        }
        out
    };
    assert_eq!(limits(""), vec![json!(12), json!(20), json!(10)]);
    assert_eq!(limits("250000"), vec![json!(75), json!(125), json!(63)]);
}

// ============================================== 4. the graph leg's cuts said

const AUD: &str = r#"["member:m"]"#;

#[test]
fn anchor_entities_past_the_start_are_parked_as_a_cut() {
    let anchors: Vec<Value> = (0..40)
        .map(|i| {
            json!({"id": format!("e{i}"), "canonical_name": format!("Aa Name{i:02}"),
                   "subject_key": format!("aa name{i:02}"), "kind": "subject",
                   "valid_until": null})
        })
        .collect();
    let doc = json!({
        "header": {"context": {"mem_phase": "t1-join", "recall_id": "r", "memory_tier": "1",
                               "recall_query": "Where does Aa Name03 live now?",
                               "audience_now": AUD, "channel": "tg:private"},
                   "hop": {"operation": "bundle"}},
        "messages": [
            {"origin": "tool", "type": "tool_result", "id": "r-join-anchor",
             "text": json!(anchors).to_string()},
            {"origin": "tool", "type": "tool_result", "id": "r-join-anchor-key", "text": "[]"},
            {"origin": "tool", "type": "tool_result", "id": "r-join-sem", "text": "[]"}]});
    let all = ops(&recall(&doc));
    let walk = all
        .iter()
        .find(|o| o["operation"] == "traverse")
        .expect("a walk");
    assert_eq!(walk["start"].as_array().unwrap().len(), 32, "{walk}");
    let cut = all
        .iter()
        .find(|o| o["operation"] == "insert" && o["row"]["leg"] == "start-cut")
        .expect("the start cut is parked");
    assert_eq!(cut["row"]["payload"], "[32, 40]", "{cut}");
    let read = all
        .iter()
        .rfind(|o| o["operation"] == "select" && o["table"] == "recall_scratch")
        .expect("the parked legs are read back");
    assert!(
        read["where"]["leg"]["in"]
            .as_array()
            .unwrap()
            .contains(&json!("start-cut")),
        "{read}"
    );
}

#[test]
fn the_bundle_names_the_graph_legs_cuts() {
    let fused = |cuts: Option<Value>| {
        let mut f = json!({
            "candidates": [{"kind": "episode", "id": "e0", "score": 0.09,
                            "legs": ["keyword"], "agreement": 1}],
            "legs_present": ["keyword"],
            "leg_sizes": {"keyword": 1, "semantic": 0, "graph": 0, "temporal": 0},
            "leg_sizes_raw": {"keyword": 1, "semantic": 0, "graph": 0, "temporal": 0},
            "leg_capped": {},
            "semantic_degraded": true
        });
        if let Some(c) = cuts {
            f["graph_cuts"] = c;
        }
        f
    };
    let hyd_ep = json!([{"id": "e0", "session_id": "s-1", "sender": "user",
                         "content": "coffee", "happened_at": "2026-01-01T09:00:00Z",
                         "recorded_at": "2026-01-01T09:00:00Z"}]);
    let doc = |f: Value| {
        let rows: Vec<Value> = [
            ("fused", f),
            ("hyd-ep", hyd_ep.clone()),
            ("hyd-fact", json!([])),
            ("hyd-axis", json!([])),
        ]
        .into_iter()
        .map(|(leg, p)| {
            json!({"request_id": "r1", "leg": leg, "payload": p.to_string(),
                                "fired": 1})
        })
        .collect();
        json!({
            "header": {
                "context": {"mem_phase": "t1-emit", "recall_id": "r1", "memory_tier": "1",
                            "recall_query": "what do I prefer?",
                            "recall_as_of": "2026-08-12T00:00:00Z",
                            "recall_window_from": "", "recall_window_to": "",
                            "audience_now": ["u1"], "channel": "chat"},
                "hop": {"operation": "select"}
            },
            "messages": [{"origin": "tool", "type": "tool_result", "id": "r",
                          "text": json!(rows).to_string()}]
        })
    };
    let bundle = |f: Value| -> Value {
        let msgs = recall(&doc(f));
        let text = msgs
            .iter()
            .find(|m| m["header"]["route"] == "bundle")
            .expect("a bundle")["system"]["memory"]["bundle"]["text"]
            .as_str()
            .expect("bundle text")
            .to_string();
        serde_json::from_str(&text).expect("bundle json")
    };
    let whole = bundle(fused(None));
    assert_eq!(whole["complete"], true, "{whole}");
    let cut = bundle(fused(Some(
        json!({"anchors": [96, 120], "start": [32, 40]}),
    )));
    assert_eq!(cut["complete"], false, "{cut}");
    let reason = cut["complete_reason"].as_str().expect("a reason");
    assert!(
        reason.contains("96 of 120 name anchors") && reason.contains("32 of 40 anchor entities"),
        "{reason}"
    );
}

// ============================== 5. presenter topics and curator candidates

#[test]
fn the_topic_ceiling_is_the_deciders_question_limit() {
    let decisions =
        std::fs::read_to_string(repo("crates/meclaw-cells/src/llm/translate_decisions.rs"))
            .expect("translate_decisions.rs");
    let line = decisions
        .lines()
        .find(|l| l.contains("const MAX_QUESTIONS: usize ="))
        .expect("MAX_QUESTIONS");
    let rust: usize = line
        .rsplit('=')
        .next()
        .unwrap()
        .trim()
        .trim_end_matches(';')
        .parse()
        .expect("a number");
    let stage = script("presenter/stage");
    assert!(
        stage.contains(&format!("\nDECIDER_QUESTIONS = {rust}\n")),
        "the presenter mirrors the decider's {rust}"
    );
    assert!(stage.contains("\nMAX_TOPICS = (DECIDER_QUESTIONS - 1) // 2\n"));
}

#[test]
fn a_builtin_topic_past_the_ceiling_is_named() {
    let stage = repo("templates/presenter/stage/stage.py");
    let driver = r#"
import importlib.util, json, sys
spec = importlib.util.spec_from_file_location("stage", sys.argv[1])
st = importlib.util.module_from_spec(spec)
sys.argv = [sys.argv[1]]
spec.loader.exec_module(st)
st.check_topic = lambda *a, **k: None
st.catalogue = lambda params: {}
out = st.builtin_topics({"builtin_topics": [{"topic": "t%02d" % i} for i in range(40)]})
print(len(out))
"#;
    let mut child = Command::new("python3")
        .arg("-")
        .arg(&stage)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(driver.as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "31");
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(err.matches("left out").count(), 9, "{err}");
    assert!(err.contains("t31"), "{err}");
}

#[test]
fn a_full_candidate_page_says_more() {
    let s = script("curator/push");
    assert!(s.contains("more = len(found) >= CANDIDATE_ROWS"));
    assert!(s.contains("...[more: a full page of %d candidate rows was read"));
}

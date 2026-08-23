//! Wave 5 -- the close pass gets a lane of its own (GitHub #300, ruling Q9).
//!
//! Per-turn extraction (#298) buys speed at the cost of perspective: the front
//! model annotates the turn it has just answered and knows nothing about the
//! turn after it. Ruling Q9 answers that with the powerful variant -- when a
//! session closes, ONE strong-model call reads the whole session and sharpens
//! what the turns left rough. This file locks the plumbing that pass runs on,
//! and nothing else: the lane, the two cells, the eight edges. Task 16 fills
//! the state machine, Task 17 the prompt.
//!
//! Three properties are worth naming, because each is a way the wiring could
//! look finished and be wrong:
//!
//! * **The lane's context is a DECLARATION with teeth.** Since #291 an
//!   `accepts` entry carries a `context` array and the mutation validator
//!   refuses a caller whose edge does not promote every key in it. So the
//!   assertion here is on the ARRAY, never on the `because` prose: a
//!   `because`-only check would go green over a lane that refuses every caller
//!   there is.
//! * **The internal ingress promotes nothing the lane needs.** The three keys
//!   arrive from the CALLER's edge; `. -> ./close-glue` stamps only the two
//!   routing keys. An internal edge that promoted `session_id` would satisfy
//!   the lane check with a value nobody supplied.
//! * **The write path is the inline ingress, not a second one.** The close pass
//!   hands blocks to `./extract-glue`, which already canonicalises, deduplicates
//!   and supersedes -- and it stamps `store_origin: 'inline'` doing it, because
//!   from there on it IS an inline chain. A `'close'` stamp there would route
//!   extract-glue's own store echoes into `close-glue`, which is why the last
//!   test in this file asserts that no edge does.

use std::io::Write;
use std::process::{Command, Stdio};

const HIVE_CONFIG: &str = "../../templates/memory-hive/config.json";
const GLUE_CONFIG: &str = "../../templates/memory-hive/close-glue/config.json";
const CLOSER_CONFIG: &str = "../../templates/memory-hive/closer/config.json";

/// A shipped `config.json` of this hive, parsed.
fn config(path: &str) -> serde_json::Value {
    let raw = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{path} is not json: {e}"))
}

fn edges() -> Vec<serde_json::Value> {
    config(HIVE_CONFIG)["params"]["graph"]["edges"]
        .as_array()
        .expect("params.graph.edges")
        .clone()
}

/// The one edge from `from` to `to` whose condition contains `needle` (or the
/// one edge from `from` to `to` at all, where `needle` is empty).
fn edge(from: &str, to: &str, needle: &str) -> serde_json::Value {
    let found: Vec<serde_json::Value> = edges()
        .into_iter()
        .filter(|e| {
            e["from"] == from
                && e["to"] == to
                && (needle.is_empty()
                    || e["condition"].as_str().is_some_and(|c| c.contains(needle)))
        })
        .collect();
    assert_eq!(
        found.len(),
        1,
        "expected exactly one {from} -> {to} edge matching {needle:?}, found {found:?}"
    );
    found.into_iter().next().expect("checked above")
}

/// `${VAR:-default}` becomes the default, a bare `${VAR}` becomes the empty
/// string -- the same substitution the colony performs when it instantiates the
/// template.
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
    let v = config(GLUE_CONFIG);
    let src = v["params"]["script_inline"]
        .as_str()
        .expect("close-glue script_inline")
        .to_string();
    resolve_vars(&src)
}

/// Run a shipped script over a real stdin document, handing the script to
/// python3 **on stdin** instead of in argv (GH #279).
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
    child.wait_with_output().expect("wait")
}

/// Run the real script with a real stdin document and return the emitted
/// messages.
fn emit(doc: serde_json::Value) -> Vec<serde_json::Value> {
    let out = run_script_on_stdin(
        &glue_script(),
        &meclaw_testing::code_stdin(&doc).to_string(),
    );
    assert!(
        out.status.success(),
        "close-glue exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

#[test]
fn the_hive_accepts_the_close_pass_with_the_context_it_needs() {
    let accepts = config(HIVE_CONFIG)["params"]["contract"]["accepts"]
        .as_array()
        .expect("params.contract.accepts")
        .clone();
    let lane = accepts
        .iter()
        .find(|e| e["route"] == "in_close_pass")
        .expect("the hive accepts `in_close_pass`");
    // The ARRAY, not the prose. The validator walks this list per caller edge
    // and refuses the mutation for every key that is not promoted; a `because`
    // sentence naming the same three keys would be a comment.
    assert_eq!(
        lane["context"],
        serde_json::json!(["session_id", "audience_set", "channel"]),
        "the lane declares the three keys the write lanes of this hive demand"
    );
    assert!(
        lane["because"].as_str().is_some_and(|b| !b.is_empty()),
        "and says why: {lane}"
    );
}

#[test]
fn the_report_is_a_declared_way_out() {
    let hive = config(HIVE_CONFIG);
    let emits = hive["params"]["contract"]["emits"]
        .as_array()
        .expect("params.contract.emits");
    assert!(
        emits.iter().any(|e| e["route"] == "close_report"),
        "the hive emits `close_report`: {emits:?}"
    );
}

#[test]
fn both_drain_pairings_are_required() {
    let hive = config(HIVE_CONFIG);
    let drains = hive["params"]["required_drains"]
        .as_array()
        .expect("params.required_drains");
    for emits in ["close_report", "reject"] {
        assert!(
            drains
                .iter()
                .any(|d| d["accepts"] == "in_close_pass" && d["emits"] == emits),
            "a caller wiring `in_close_pass` must drain `{emits}`: {drains:?}"
        );
    }
}

#[test]
fn the_ingress_stamps_the_two_routing_keys_and_nothing_else() {
    let e = edge(".", "./close-glue", "in_close_pass");
    let set = e["modifier"]["set_context"]
        .as_object()
        .expect("set_context");
    assert_eq!(
        set.get("store_origin").map(|v| v.as_str()),
        Some(Some("'close'"))
    );
    assert_eq!(
        set.get("mem_phase").map(|v| v.as_str()),
        Some(Some("'window'"))
    );
    // The lane check is satisfied by the CALLER's edge. An internal edge that
    // promoted `session_id` here would let a caller that never supplied one
    // through the validator with an empty string.
    assert_eq!(
        set.len(),
        2,
        "the internal ingress promotes nothing the lane declares: {set:?}"
    );
}

#[test]
fn the_store_round_trip_is_the_dream_lane_shape() {
    let out = edge("./close-glue", "./store", "cstore");
    let set = out["modifier"]["set_context"]
        .as_object()
        .expect("set_context on the store op");
    assert_eq!(
        set.get("store_origin").map(|v| v.as_str()),
        Some(Some("'close'"))
    );
    assert_eq!(
        set.get("mem_phase").map(|v| v.as_str()),
        Some(Some("hop.phase")),
        "the phase the glue asked for is the phase that comes back"
    );

    let back = edge("./store", "./close-glue", "store_origin");
    assert_eq!(
        back["condition"], "context.store_origin == 'close'",
        "only this lane's own echoes come back here"
    );
    assert_eq!(
        back["modifier"]["set_context"]["mem_phase"], "context.mem_phase",
        "and the phase survives the round trip"
    );
}

#[test]
fn the_model_hangs_on_its_own_two_edges() {
    let out = edge("./close-glue", "./closer", "close");
    assert_eq!(out["condition"], "has(hop.route) && hop.route == 'close'");
    let back = edge("./closer", "./close-glue", "");
    let set = back["modifier"]["set_context"]
        .as_object()
        .expect("set_context on the verdict");
    assert_eq!(
        set.get("store_origin").map(|v| v.as_str()),
        Some(Some("'close'"))
    );
    assert_eq!(
        set.get("mem_phase").map(|v| v.as_str()),
        Some(Some("'verdict'"))
    );
}

#[test]
fn the_pass_reaches_the_caller_on_two_doors() {
    for route in ["close_report", "reject"] {
        let e = edge("./close-glue", ".", route);
        assert_eq!(
            e["condition"],
            format!("has(hop.route) && hop.route == '{route}'")
        );
    }
}

#[test]
fn the_write_path_is_the_inline_ingress() {
    // Q9 points 2 and 3 without a second write path: the close pass hands
    // blocks to the cell that already canonicalises, deduplicates, supersedes
    // and enqueues embeddings, instead of learning to do all four again.
    let e = edge("./close-glue", "./extract-glue", "close_write");
    assert_eq!(
        e["condition"],
        "has(hop.route) && hop.route == 'close_write'"
    );
    let set = e["modifier"]["set_context"]
        .as_object()
        .expect("set_context on the hand-off");
    // 'inline', not 'close': from here on it IS an inline chain, and
    // `./extract-glue -> ./store` re-stamps `store_origin: 'extract'` for its
    // own round trips.
    assert_eq!(
        set.get("store_origin").map(|v| v.as_str()),
        Some(Some("'inline'"))
    );
    assert_eq!(
        set.get("mem_phase").map(|v| v.as_str()),
        Some(Some("'inline'"))
    );
    // The only thing that travels to tell the two producers apart -- edge
    // truth, never a body claim.
    assert_eq!(set.get("close_pass").map(|v| v.as_str()), Some(Some("'1'")));
}

#[test]
fn no_echo_of_the_inline_chain_comes_back_to_the_close_glue() {
    // The other half of the stamp above, and the reason it is not a detail: the
    // inline chain's store echoes carry `store_origin` 'inline' and 'extract'.
    // An edge routing either into `close-glue` would give the close pass a
    // second, unaccounted state machine driven by somebody else's writes.
    for e in edges() {
        if e["to"] != "./close-glue" {
            continue;
        }
        let condition = e["condition"].as_str().unwrap_or("");
        for foreign in ["'inline'", "'extract'", "'episode'", "'embed'"] {
            assert!(
                !condition.contains(foreign),
                "an edge routes a foreign store echo into close-glue: {e}"
            );
        }
    }
}

#[test]
fn the_closer_is_a_model_cell_with_no_model_of_its_own() {
    let closer = config(CLOSER_CONFIG);
    assert_eq!(closer["cell"]["type"], "llm");
    assert_eq!(
        closer["contract"]["capabilities"],
        serde_json::json!(["network:llm"])
    );
    // A model default in code is how a run silently measures a different model
    // than the report claims (P5 finding) -- and Q9 rules the model CLASS, so a
    // default here would revoke the ruling quietly.
    assert!(
        closer["contract"]["settings"]["model"]["default"].is_null(),
        "the closer declares no default model: {}",
        closer["contract"]["settings"]["model"]
    );
    assert_eq!(
        closer["params"]["model"], "${MODEL_CLOSER}",
        "and reads the one env var that names the class"
    );
}

#[test]
fn the_window_phase_reads_the_session_whole() {
    // The script-level half: fed the phase its own ingress edge stamps, the
    // glue asks the store for the turns of the session it was handed.
    let msgs = emit(serde_json::json!({
        "header": {
            "context": {"mem_phase": "window", "store_origin": "close",
                        "session_id": "s-300", "channel": "c-300",
                        "audience_set": r#"["member:user","agent:assistant"]"#},
            "hop": {"route": "in_close_pass"}
        },
        "messages": [{"origin": "user", "type": "text", "id": "m1", "text": "close"}]
    }));
    let ops: Vec<serde_json::Value> = msgs
        .iter()
        .filter(|m| m["header"]["route"] == "cstore")
        .map(|m| {
            let text = m["messages"][0]["text"].as_str().expect("op text");
            serde_json::from_str::<serde_json::Value>(text).expect("op args")
        })
        .collect();
    assert!(
        !ops.is_empty(),
        "the window phase emits at least one store op: {msgs:?}"
    );
    let read = ops
        .iter()
        .find(|a| a["operation"] == "select" && a["table"] == "episodes")
        .unwrap_or_else(|| panic!("the window reads the episodes of the session: {ops:?}"));
    assert_eq!(
        read["where"]["session_id"], "s-300",
        "and only of the session it was handed"
    );
    assert!(
        read["order_by"].is_array(),
        "ordered -- a prefix of an unordered set is not a page: {read}"
    );
    assert!(
        read["limit"].as_u64().is_some_and(|n| n > 0),
        "and bounded -- a memory that has been running for years must not put a whole table on a mailbox: {read}"
    );
}

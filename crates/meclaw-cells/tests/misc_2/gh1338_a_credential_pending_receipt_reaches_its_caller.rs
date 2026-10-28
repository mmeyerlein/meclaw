//! GH #1338: a `credential_pending` receipt reaches its caller.
//!
//! A cell that fetches its secret through an access grant
//! (`credential_grant_id`) parks a call while the sealed box is in flight. When
//! the round outlasts `credential_wait_ms`, the cell answers the call with a
//! receipt: `hop.error_code == "credential_pending"` and no `hop.route`
//! (`llm/cell.rs` `emit_credential_pending`, `code/cell.rs`
//! `emit_credential_pending`, `web_search.rs`, `mcp/cell.rs`). The receipt goes
//! out through the cell's own out-edges. Where none takes it, it dead-letters
//! as `no_route` and the call is lost: measured in a lab colony under load on
//! 2026-10-28 (the round took 11 s against the 10 s default, the box opened one
//! second after the receipt had dead-lettered, the caller waited for an answer
//! that never came).
//!
//! The class is a property of the wiring, not of three templates, so the first
//! lock walks the tree: every cell under `templates/` that can carry a grant
//! (its `config.json` has `params.credential_grant_id`, shipped empty or set,
//! because a consumer sets an empty one through `override_params`) needs an
//! out-edge of its template that takes the receipt. The edges are evaluated by
//! the colony's own CEL (`cel_eval`), against the receipt each cell type
//! really emits.
//!
//! The other locks hold the three templates the walk found: the receipt
//! becomes exactly one result on the caller's existing lane, the wait is the
//! longer one, and the backstop of the `code` cell still covers it.

use meclaw_colony::cel_eval::{
    apply_modifier, evaluate_condition, parse_condition, parse_modifier,
};
use meclaw_colony::config::EdgeSpec;
use meclaw_core::Headers;
use meclaw_core::serde_json::{self, Map, Value, json};
use std::path::PathBuf;

/// The cell types that answer a call with a `credential_pending` receipt.
const EMITTERS: &[&str] = &["llm", "code", "web_search", "mcp"];

/// The wait the three repaired templates ship: a caller that waits a minute
/// for a vault round under load gets its answer; 10 s lost the call.
const WAIT_MS: u64 = 60_000;

fn core_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn templates_dir() -> PathBuf {
    core_root().join("templates")
}

fn read_json(path: &std::path::Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn template_config(template: &str) -> Value {
    let path = templates_dir().join(template).join("config.json");
    read_json(&path).unwrap_or_else(|| panic!("read {}", path.display()))
}

fn cell_config(template: &str, cell: &str) -> Value {
    let path = templates_dir()
        .join(template)
        .join(cell)
        .join("config.json");
    read_json(&path).unwrap_or_else(|| panic!("read {}", path.display()))
}

/// The hop of the `credential_pending` receipt a cell type emits -- the
/// header keys its emission carries, never a `route`.
fn receipt_hop(kind: &str) -> Map<String, Value> {
    let v = match kind {
        "llm" => {
            json!({"finish_reason": "error", "error_code": "credential_pending", "latency_ms": 0})
        }
        "code" => json!({"finish_reason": "error", "error_code": "credential_pending"}),
        "web_search" => json!({
            "operation": "web_search", "duration_ms": 0,
            "finish_reason": "error", "error_code": "credential_pending"
        }),
        "mcp" => json!({"mcp_tool": "x", "duration_ms": 0, "error_code": "credential_pending"}),
        other => panic!("no receipt for cell type {other}"),
    };
    v.as_object().cloned().expect("object")
}

fn edges(template_cfg: &Value) -> Vec<EdgeSpec> {
    template_cfg["params"]["graph"]["edges"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|e| serde_json::from_value(e.clone()).expect("an edge parses"))
                .collect()
        })
        .unwrap_or_default()
}

fn takes(edge: &EdgeSpec, context: &Map<String, Value>, hop: &Map<String, Value>) -> bool {
    match &edge.condition {
        None => true,
        Some(c) if c.trim().is_empty() => true,
        Some(c) => {
            let compiled = parse_condition(c).expect("the condition compiles");
            evaluate_condition(&compiled, context, hop).unwrap_or(false)
        }
    }
}

/// Every `context.<key> == '<literal>'` a condition compares, so the walk can
/// try each value a template stamps (a turn of the parent, a turn of the
/// channel, a phase) instead of guessing one.
fn context_literals(condition: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = condition;
    while let Some(at) = rest.find("context.") {
        rest = &rest[at + "context.".len()..];
        let key: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        let after = rest[key.len()..].trim_start();
        let Some(after) = after.strip_prefix("==") else {
            continue;
        };
        let after = after.trim_start();
        let Some(after) = after.strip_prefix('\'') else {
            continue;
        };
        if let Some(end) = after.find('\'') {
            out.push((key, after[..end].to_string()));
        }
    }
    out
}

/// The contexts a receipt can travel with: none, and each literal one of the
/// conditions compares, alone and all together.
fn candidate_contexts(conditions: &[&str]) -> Vec<Map<String, Value>> {
    let pairs: Vec<(String, String)> = conditions
        .iter()
        .flat_map(|c| context_literals(c))
        .collect();
    let mut out = vec![Map::new()];
    let mut all = Map::new();
    for (k, v) in &pairs {
        let mut one = Map::new();
        one.insert(k.clone(), Value::String(v.clone()));
        out.push(one);
        all.insert(k.clone(), Value::String(v.clone()));
    }
    out.push(all);
    out
}

/// A literal a modifier stamps (`"'embed'"`), or `None` for an expression.
fn stamped_literal(v: &Value) -> Option<String> {
    let s = v.as_str()?.trim();
    let inner = s.strip_prefix('\'')?.strip_suffix('\'')?;
    (!inner.contains('\'')).then(|| inner.to_string())
}

/// The contexts a call reaches `./<cell>` with. Each in-edge contributes the
/// literals its modifier stamps for the keys the out-edges compare
/// (memory-hive/embed: `mem_phase` `embed` from the writer, `embed-query`
/// from recall), laid over every candidate of `candidate_contexts`: a key the
/// edge does not stamp is the caller's and passes on, so it may hold any
/// value (re-check R-1 of #1338: replacing the candidate dropped the passed
/// `turn_origin` and a lost parent half stayed green). An in-edge that
/// stamps none of the keys contributes every candidate unchanged.
/// A reply to a `code` cell's own request is no call and contributes nothing:
/// an in-edge from `X` whose condition asks for `context.k == 'v'` that the
/// cell's own out-edge to `X` stamps (memory-hive: `./store` answers
/// `./embed` under `store_origin == 'embed'`; the script parks on the
/// acknowledgement, nobody waits for a result of it). Only a `code` cell
/// parks: for `llm`, `web_search` and `mcp` every incoming message is a call
/// with a credential round (re-check R-2 of #1338).
fn call_contexts(tcfg: &Value, cell: &str, kind: &str, conds: &[&str]) -> Vec<Map<String, Value>> {
    let from = format!("./{cell}");
    let raw: Vec<Value> = tcfg["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let keys: Vec<String> = conds
        .iter()
        .flat_map(|c| context_literals(c))
        .map(|(k, _)| k)
        .collect();
    let stamps = |e: &Value| -> Vec<(String, String)> {
        e["modifier"]["set_context"]
            .as_object()
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| stamped_literal(v).map(|l| (k.clone(), l)))
                    .collect()
            })
            .unwrap_or_default()
    };
    let is_reply = |e: &Value| -> bool {
        if kind != "code" {
            return false;
        }
        let src = e["from"].as_str().unwrap_or_default();
        let cond = e["condition"].as_str().unwrap_or_default();
        let asked = context_literals(cond);
        raw.iter()
            .filter(|o| o["from"] == from.as_str() && o["to"] == src)
            .flat_map(&stamps)
            .any(|kv| asked.contains(&kv))
    };
    let mut out = Vec::new();
    for e in raw.iter().filter(|e| e["to"] == from.as_str()) {
        if is_reply(e) {
            continue;
        }
        let own: Map<String, Value> = stamps(e)
            .into_iter()
            .filter(|(k, _)| keys.contains(k))
            .map(|(k, v)| (k, Value::String(v)))
            .collect();
        out.extend(candidate_contexts(conds).into_iter().map(|mut ctx| {
            ctx.extend(own.clone());
            ctx
        }));
    }
    if out.is_empty() {
        out = candidate_contexts(conds);
    }
    out
}

/// `(template, cell, type)` of every cell that can carry a grant and lives in
/// a template with a graph -- a single-cell template has no edges of its own,
/// its consumer draws them.
fn grant_cells() -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let mut templates: Vec<_> = std::fs::read_dir(templates_dir())
        .expect("templates/")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    templates.sort();
    for t in templates {
        let Some(tcfg) = read_json(&t.join("config.json")) else {
            continue;
        };
        if tcfg["params"]["graph"]["edges"].as_array().is_none() {
            continue;
        }
        let mut cells: Vec<_> = std::fs::read_dir(&t)
            .expect("template dir")
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        cells.sort();
        for c in cells {
            let Some(cfg) = read_json(&c.join("config.json")) else {
                continue;
            };
            let kind = cfg["cell"]["type"].as_str().unwrap_or_default();
            if EMITTERS.contains(&kind) && cfg["params"]["credential_grant_id"].is_string() {
                out.push((
                    t.file_name().unwrap().to_string_lossy().into_owned(),
                    c.file_name().unwrap().to_string_lossy().into_owned(),
                    kind.to_string(),
                ));
            }
        }
    }
    out
}

/// Lock 1 (the class): no cell that can carry a grant leaves its
/// `credential_pending` receipt without an out-edge. Before GH #1338:
/// `egon/worker`, `slack-agent/worker` and `file-space/embed`.
#[test]
fn every_grant_cell_has_an_edge_for_its_credential_pending_receipt() {
    let cells = grant_cells();
    for known in [
        ("egon", "worker"),
        ("slack-agent", "worker"),
        ("file-space", "embed"),
    ] {
        // GH #49: `egon` and `slack-agent` stay private and are absent from the public clone.
        if !templates_dir().join(known.0).exists() {
            continue;
        }
        assert!(
            cells
                .iter()
                .any(|(t, c, _)| (t.as_str(), c.as_str()) == known),
            "the walk must see {known:?}; it found {cells:?}"
        );
    }
    let mut lost = Vec::new();
    for (t, c, kind) in &cells {
        let from = format!("./{c}");
        let out: Vec<EdgeSpec> = edges(&template_config(t))
            .into_iter()
            .filter(|e| e.from == from)
            .collect();
        let conds: Vec<&str> = out.iter().filter_map(|e| e.condition.as_deref()).collect();
        let hop = receipt_hop(kind);
        // Every context a call arrives with, not one: a template whose
        // receipt edges split by context (egon/slack-agent: a turn of the
        // parent, a turn of the channel) is wired only when each context finds
        // an edge; with `any` dropping the channel edge stayed green, and the
        // recall lane of memory-hive/embed (`embed-query`) had none at all
        // while the write lane had one (review M-2 of #1338).
        let routed = call_contexts(&template_config(t), c, kind, &conds)
            .iter()
            .all(|ctx| out.iter().any(|e| takes(e, ctx, &hop)));
        if !routed {
            lost.push(format!("{t}/{c} ({kind}, {} out-edges)", out.len()));
        }
    }
    assert!(
        lost.is_empty(),
        "a credential_pending receipt finds no out-edge and dead-letters as no_route: {lost:?}"
    );
}

/// The edges of `template` from `from` that take `hop` under `context`, each
/// with the headers its modifier leaves.
fn route(template: &str, from: &str, context: Value, hop: Value) -> Vec<(String, Headers)> {
    let headers = Headers {
        context: context.as_object().cloned().unwrap_or_default(),
        hop: hop.as_object().cloned().unwrap_or_default(),
    };
    edges(&template_config(template))
        .into_iter()
        .filter(|e| e.from == from && takes(e, &headers.context, &headers.hop))
        .map(|e| {
            let out = match &e.modifier {
                None => headers.clone(),
                Some(m) => {
                    let compiled = parse_modifier(m).expect("the modifier compiles");
                    apply_modifier(&compiled, &headers).expect("the modifier applies")
                }
            };
            (e.to.clone(), out)
        })
        .collect()
}

fn llm_receipt() -> Value {
    Value::Object(receipt_hop("llm"))
}

/// Lock 2 (egon, slack-agent): a turn the parent handed in on `in_turn` gets
/// exactly one `answer`, carrying the error code; a turn of the channel leaves
/// on the template's error lane `error`, the lane a refusal of `./access`
/// already takes. `turn_origin` is gone either way.
#[test]
fn a_chat_worker_receipt_is_one_result_for_its_turn() {
    for t in ["egon", "slack-agent"] {
        // GH #49: both stay private and are absent from the public clone.
        if !templates_dir().join(t).exists() {
            continue;
        }
        let parent = route(
            t,
            "./worker",
            json!({"turn_origin": "parent"}),
            llm_receipt(),
        );
        assert_eq!(
            parent.len(),
            1,
            "{t}: a parent turn gets exactly one result: {parent:?}"
        );
        let (to, h) = &parent[0];
        assert_eq!(to, ".", "{t}: the result goes to the caller");
        assert_eq!(h.hop["route"], "answer", "{t}: on the caller's lane");
        assert_eq!(
            h.hop["error_code"], "credential_pending",
            "{t}: and says why"
        );
        assert!(
            !h.context.contains_key("turn_origin"),
            "{t}: the turn is closed"
        );

        for ctx in [json!({"turn_origin": "channel"}), json!({})] {
            let channel = route(t, "./worker", ctx.clone(), llm_receipt());
            assert_eq!(
                channel.len(),
                1,
                "{t} {ctx}: exactly one result: {channel:?}"
            );
            let (to, h) = &channel[0];
            assert_eq!(to, ".", "{t} {ctx}");
            assert_eq!(h.hop["route"], "error", "{t} {ctx}: on the error lane");
            assert!(!h.context.contains_key("turn_origin"), "{t} {ctx}");
        }

        // A real answer still goes exactly one way (no second result).
        let stop = route(
            t,
            "./worker",
            json!({"turn_origin": "parent"}),
            json!({"finish_reason": "stop"}),
        );
        assert_eq!(stop.len(), 1, "{t}: an answer is not doubled: {stop:?}");

        // The contract says what the lanes carry, and a parent or the builder
        // reads it from there (review M-1338-1): `answer` and `error` name the
        // receipt, `model_refused` no longer claims `stop` is the worker's
        // only way out.
        let emits = &template_config(t)["params"]["contract"]["emits"];
        let because = |route: &str| -> String {
            emits
                .as_array()
                .and_then(|a| a.iter().find(|e| e["route"] == route))
                .and_then(|e| e["because"].as_str())
                .unwrap_or_else(|| panic!("{t}: contract.emits names `{route}`"))
                .to_string()
        };
        for lane in ["answer", "error"] {
            assert!(
                because(lane).contains("credential_pending"),
                "{t}: contract.emits `{lane}` names the credential_pending receipt"
            );
        }
        assert!(
            !because("model_refused").contains("take `stop` alone"),
            "{t}: contract.emits `model_refused` still says the worker's out-edges take `stop` alone"
        );

        let w = cell_config(t, "worker");
        assert_eq!(
            w["params"]["credential_wait_ms"], WAIT_MS,
            "{t}/worker waits a minute"
        );
        assert!(
            w["contract"]["settings"]["credential_wait_ms"]["description"].is_string(),
            "{t}/worker states why it waits longer than the default"
        );
    }
}

/// Lock 3 (file-space): the receipt of `./embed` reaches `./derive` on the
/// lane `embedded`, the one way `./derive` learns about vectors, so the job of
/// `cur_job` ends with its failure (`vectors` missing) instead of waiting
/// forever. And the backstop covers the longer wait: a call waits for its
/// credential inside `handle()`, then for a run ticket behind the others
/// (`grant_slot.rs`, GH #1061 lock), then runs.
#[test]
fn a_file_space_embed_receipt_ends_its_job() {
    let ctx = json!({"cur_job": "d-1", "cur_phase": "embedded"});
    let out = route(
        "file-space",
        "./embed",
        ctx,
        Value::Object(receipt_hop("code")),
    );
    assert_eq!(out.len(), 1, "exactly one result: {out:?}");
    let (to, h) = &out[0];
    assert_eq!(to, "./derive");
    assert_eq!(h.hop["route"], "embedded");
    assert_eq!(h.hop["error_code"], "credential_pending");
    assert_eq!(h.context["cur_job"], "d-1", "the job is still named");

    let cfg = cell_config("file-space", "embed");
    let p = &cfg["params"];
    let wait = p["credential_wait_ms"]
        .as_u64()
        .expect("credential_wait_ms");
    assert_eq!(wait, WAIT_MS);
    assert!(cfg["contract"]["settings"]["credential_wait_ms"]["description"].is_string());
    let mc = p["max_concurrency"].as_u64().expect("max_concurrency");
    let wait_max = p["credential_wait_max"]
        .as_u64()
        .expect("credential_wait_max");
    let t = p["external_timeout_ms"]
        .as_u64()
        .expect("external_timeout_ms");
    let worst = wait + (wait_max + 1).div_ceil(mc) * t + t;
    let mt = cfg["cell"]["message_timeout"]
        .as_u64()
        .expect("message_timeout");
    assert!(
        mt > worst,
        "message_timeout {mt} must exceed the worst call {worst} ms"
    );
}

/// Lock 4 (memory-hive): a receipt of `./embed` on the recall lane reaches
/// `./recall` as its query vector, so the fan-in fuses the other legs instead
/// of waiting forever (the embed script's own rule: the read lane always gets
/// an answer). The write lane keeps its edge to `./writer`.
#[test]
fn a_memory_hive_embed_receipt_answers_recall() {
    let receipt = Value::Object(receipt_hop("code"));
    let out = route(
        "memory-hive",
        "./embed",
        json!({"mem_phase": "embed-query", "recall_id": "r-1"}),
        receipt.clone(),
    );
    assert_eq!(out.len(), 1, "exactly one result: {out:?}");
    let (to, h) = &out[0];
    assert_eq!(to, "./recall");
    assert_eq!(
        h.context["mem_phase"], "t1-qvec",
        "recall's query-vector phase"
    );
    assert_eq!(h.context["recall_id"], "r-1", "the recall is still named");

    let write = route(
        "memory-hive",
        "./embed",
        json!({"mem_phase": "embed"}),
        receipt,
    );
    assert_eq!(write.len(), 1, "{write:?}");
    assert_eq!(write[0].0, "./writer");
}

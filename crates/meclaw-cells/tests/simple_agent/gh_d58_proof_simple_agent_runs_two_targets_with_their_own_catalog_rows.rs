//! D58 proof (GH #1475, contract point 1) -- `simple-agent@2` runs two
//! targets with their own catalogue rows.
//!
//! WHY this file exists: R-58-9 Nr. 6 asks for a composite with another
//! number of model cells than talky running on the curator's new contract
//! (every curate names its target, every model cell reports its row and its
//! window profile over its own edge). `simple-agent@2` has two model cells,
//! `brain` and `strong`. Before D58 both stood under one label and the
//! composite told them apart by a context field of its own (`sa_stage`,
//! `fast`/`strong`) that the curator knew by name (leser-A-weiche § 5:
//! policy.py 3148, 7315, 8626). Strand target took the name out of the
//! curator; this pin holds that the composite no longer splits by a stage
//! either: the door names the target itself, the composite carries that
//! name to the curate, and each target reports its own row and profile and
//! keeps its own window bound.
//!
//! What it pins, in one run of the shipped template (Form::V2, `strong` on):
//!
//! 1. No file of `templates/simple-agent/` names `sa_stage` (edges, the
//!    door's contract and script).
//! 2. `brain` and `strong` each report their package line and their own
//!    profile (`package_line`, `window_profile` with a window bound) and
//!    answer the curator's ask (`in_catalog`, `catalog_ask`).
//! 3. A fast turn's curate names `brain`, a strong turn's names `strong`, and
//!    each model call leaves for the cell it was cut for.
//! 4. The curator holds two rows and two bounds, each from its own cell --
//!    a row pushed into `brain` never sizes `strong` and the other way round.
//! 5. No message of the run carries `sa_stage`, on its hop or its context.
//!
//! Real time with hang marks only (AGENTS rule 11); no clock, no sleep.

// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;
#[allow(unused_imports)]
use crate::simple_agent_loop; // harness_fold: mod simple_agent_loop;
use meclaw_core::serde_json::{self as sj, Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use mock_openai::canned_chat_completion;
use simple_agent_loop::{
    AUDIENCE, BRAIN_MODEL, STRONG_MODEL, as_map, boot, next_on, read_json, repo, shipped,
    wait_route,
};

/// The context field the composite split its two models by before D58.
const STAGE_FIELD: &str = "sa_stage";

/// The two targets of the composite, by the names it wires them under.
const TARGETS: [&str; 2] = ["brain", "strong"];

/// A catalogue row as the colony's registry pushes it into one model cell;
/// the bounds differ per cell, so a value the curator holds names its source.
fn package(model: &str, input_soft: i64, input_hard: i64) -> Value {
    json!({"model": model, "input_soft": input_soft, "input_hard": input_hard,
           "max_output": 4096, "cache_mode": "off", "cache_ttl_s": 0,
           "wire_dialect": "chat_completions", "supported_params": [],
           "chars_per_token": 3.5})
}

/// The registry's push of `package` into the cell `cell` of the agent.
fn push(cell: &str, package: Value) -> Message {
    MessageBuilder::new(Path::new("/agent"))
        .hop(as_map(&json!({"route": "in_model",
                            "subscriber": format!("/agent/{cell}"),
                            "rank": "target"})))
        .context(Map::new())
        .body(Body::Inline(json!({"system": {}, "params": package})))
        .ttl(200)
        .build()
}

/// A turn on the agent's rim, `stage` on its hop when given.
fn turn_on(turn_id: &str, text: &str, stage: Option<&str>) -> Message {
    let mut head = json!({"route": "turn", "turn_id": turn_id, "session_id": "s1",
                          "channel": "c1", "audience_set": AUDIENCE});
    if let Some(s) = stage {
        head["stage"] = json!(s);
    }
    MessageBuilder::new(Path::new("/agent"))
        .hop(as_map(&head))
        .context(Map::new())
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .ttl(400)
        .build()
}

/// Every file under `dir` whose text names `needle`, relative to the repo.
fn files_naming(dir: &std::path::Path, needle: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if std::fs::read_to_string(&p).is_ok_and(|t| t.contains(needle)) {
                out.push(
                    p.strip_prefix(repo(""))
                        .map(|r| r.display().to_string())
                        .unwrap_or_else(|_| p.display().to_string()),
                );
            }
        }
    }
    out.sort();
    out
}

/// The shipped edges of the composite.
fn edges() -> Vec<Value> {
    read_json(&repo("templates/simple-agent/config.json"))["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Whether an edge `from -> to` exists whose condition names `needle`.
fn has_edge(edges: &[Value], from: &str, to: &str, needle: &str) -> bool {
    edges.iter().any(|e| {
        e["from"] == from && e["to"] == to && e["condition"].as_str().unwrap_or("").contains(needle)
    })
}

/// The logged messages from under `from` to under `to`: `(hop, context, body)`.
async fn logged(
    h: &meclaw_testing::ColonyHandle,
    from: &str,
    to: &str,
) -> Vec<(Value, Value, Value)> {
    h.read_log(meclaw_colony::api_dto::MessageLogFilter::default())
        .await
        .iter()
        .filter(|r| r.from_path.starts_with(from) && r.to_path.starts_with(to))
        .map(|r| {
            let hdr: Value = sj::from_str(&r.headers_json).unwrap_or_default();
            let body: Value = r
                .body_payload
                .as_deref()
                .and_then(|b| sj::from_str(b).ok())
                .unwrap_or_default();
            (hdr["hop"].clone(), hdr["context"].clone(), body)
        })
        .collect()
}

/// The curator's ledger of the run (the store cell `curator/ledger`), opened
/// read-only once the colony is quiet.
fn ledger(td: &tempfile::TempDir) -> rusqlite::Connection {
    let dir = td.path().join("main/agent/curator/ledger");
    let mut stack = vec![dir.clone()];
    let mut found = Vec::new();
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().is_some_and(|n| n == "cell.db") {
                found.push(p);
            }
        }
    }
    let db = found
        .first()
        .unwrap_or_else(|| panic!("no cell.db of the curator's ledger under {}", dir.display()));
    rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap_or_else(|e| panic!("{}: {e}", db.display()))
}

/// The state row `key` of the curator's ledger, empty when there is none.
fn state(db: &rusqlite::Connection, key: &str) -> String {
    db.query_row("SELECT value FROM state WHERE key = ?1", [key], |r| {
        r.get::<_, String>(0)
    })
    .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d58_proof_simple_agent_runs_two_targets_with_their_own_catalog_rows() {
    if !shipped() {
        return;
    }
    let mut failures: Vec<String> = Vec::new();

    // 1. The composite names its targets; no stage splits them.
    let naming = files_naming(&repo("templates/simple-agent"), STAGE_FIELD);
    if !naming.is_empty() {
        failures.push(format!(
            "the composite still splits its two models by `{STAGE_FIELD}`: {naming:?}"
        ));
    }

    // 2. Each target reports its own row and its own profile, and answers
    //    the curator's ask.
    let e = edges();
    for t in TARGETS {
        let cfg = read_json(&repo(&format!("templates/simple-agent/{t}/config.json")));
        if cfg["params"]["package_line"] != json!(true) {
            failures.push(format!("{t}: no `package_line`"));
        }
        if !cfg["params"]["window_profile"]["window_target"].is_number() {
            failures.push(format!(
                "{t}: no window bound of its own in its profile: {}",
                cfg["params"]["window_profile"]
            ));
        }
        let cell = format!("./{t}");
        if !has_edge(&e, &cell, "./curator", "hop.route == 'package'") {
            failures.push(format!("{t}: no edge of its package line to `in_catalog`"));
        }
        if !has_edge(&e, "./curator", &cell, &format!("hop.catalog_for == '{t}'")) {
            failures.push(format!("{t}: no edge of the curator's ask by its name"));
        }
    }

    // 3.-5. One run: two rows pushed, a fast turn and a strong turn.
    let (td, stubs, h, mut ports) = boot(
        vec![
            canned_chat_completion("Quickly.", "stop"),
            canned_chat_completion("Deeply.", "stop"),
        ],
        json!({"door": {"strong": STRONG_MODEL}}),
    )
    .await;
    let rows = [
        ("brain", package(BRAIN_MODEL, 111_000, 222_000)),
        ("strong", package(STRONG_MODEL, 333_000, 444_000)),
    ];
    for (t, p) in &rows {
        h.send(push(t, p.clone())).await;
        wait_route(
            &h,
            &format!("/agent/{t}"),
            "/agent/curator",
            "in_catalog",
            1,
        )
        .await;
    }
    let mut seen = Vec::new();
    h.send(turn_on("t-1", "Say hello.", None)).await;
    let a1 = next_on(&h, &mut ports, "answer", &mut seen).await;
    h.send(turn_on("t-2", "Think it through.", Some("strong")))
        .await;
    let a2 = next_on(&h, &mut ports, "answer", &mut seen).await;
    h.quiesce().await;
    let stage = |m: &Message| simple_agent_loop::body_of(m)["answer"]["stage"].clone();
    if stage(&a1) != json!("fast") || stage(&a2) != json!("strong") {
        failures.push(format!(
            "the rim's stages: {} and {}",
            stage(&a1),
            stage(&a2)
        ));
    }
    let reqs = stubs.brain.recorded_requests().await;
    let models: Vec<Option<&str>> = reqs.iter().map(|r| r.model()).collect();
    if models != vec![Some(BRAIN_MODEL), Some(STRONG_MODEL)] {
        failures.push(format!("the models asked: {models:?}"));
    }

    // Each target's package line carries its own profile.
    for t in TARGETS {
        let lines = logged(&h, &format!("/agent/{t}"), "/agent/curator").await;
        let line = lines
            .iter()
            .find(|(hop, _, _)| hop["route"] == "in_catalog")
            .map(|(_, _, b)| b.clone())
            .unwrap_or_default();
        if !line["window_profile"]["window_target"].is_number() {
            failures.push(format!("{t}: its package line carries no profile: {line}"));
        }
    }

    // 3. A curate names the target of its turn; the call leaves for it.
    let curates: Vec<Value> = logged(&h, "/agent/collector", "/agent/curator")
        .await
        .into_iter()
        .filter(|(hop, _, _)| hop["route"] == "in_curate")
        .map(|(hop, _, _)| hop)
        .collect();
    let target_of_turn = |turn: &str| -> Vec<Value> {
        curates
            .iter()
            .filter(|hop| hop["turn_id"] == turn)
            .map(|hop| hop["target"].clone())
            .collect()
    };
    if target_of_turn("t-1") != vec![json!("brain")] {
        failures.push(format!(
            "the fast turn's curate names {:?}",
            target_of_turn("t-1")
        ));
    }
    if target_of_turn("t-2") != vec![json!("strong")] {
        failures.push(format!(
            "the strong turn's curate names {:?}",
            target_of_turn("t-2")
        ));
    }
    for t in TARGETS {
        let calls: Vec<Value> = logged(&h, "/agent/curator", &format!("/agent/{t}"))
            .await
            .into_iter()
            .filter(|(hop, _, _)| hop["route"] == "model_call")
            .map(|(hop, _, _)| hop["target"].clone())
            .collect();
        if calls != vec![json!(t)] {
            failures.push(format!("the calls that reached `{t}`: {calls:?}"));
        }
    }

    // 4. Two rows, two bounds, each from its own cell.
    let db = ledger(&td);
    for (t, p) in &rows {
        let held: Value = sj::from_str(&state(&db, &format!("catalog:{t}"))).unwrap_or_default();
        let row = if held.get("package").is_some() {
            held["package"].clone()
        } else {
            held.clone()
        };
        if row["input_soft"] != p["input_soft"] || row["input_hard"] != p["input_hard"] {
            failures.push(format!("the row the curator holds for `{t}`: {held}"));
        }
        let bound = state(&db, &format!("catalog:{t}:input_soft"));
        if bound.trim().parse::<i64>().ok() != p["input_soft"].as_i64() {
            failures.push(format!(
                "the bound the curator keeps for `{t}` from its answer: {bound:?}"
            ));
        }
    }

    // 5. No message of the run carries the stage.
    let carrying: Vec<String> = logged(&h, "/", "/")
        .await
        .into_iter()
        .filter(|(hop, ctx, _)| hop.get(STAGE_FIELD).is_some() || ctx.get(STAGE_FIELD).is_some())
        .map(|(hop, _, _)| hop["route"].as_str().unwrap_or("-").to_string())
        .collect();
    if !carrying.is_empty() {
        failures.push(format!(
            "messages of the run carry `{STAGE_FIELD}` (routes): {carrying:?}"
        ));
    }

    assert!(failures.is_empty(), "{}", failures.join("\n  "));
}

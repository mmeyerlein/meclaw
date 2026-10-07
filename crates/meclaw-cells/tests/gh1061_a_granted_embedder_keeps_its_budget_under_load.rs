//! GH #1061 (Vault-Grants, strand V4, review finding I1 of V3): a stateless
//! cell on a grant keeps its `message_timeout` budget under sustained load.
//!
//! Since GH #1060 a `code` or `web_search` cell with a grant lets
//! `max_concurrency + credential_wait_max + 1` calls into `handle` at once
//! (`grant_slot::dispatcher_bound`): `max_concurrency` of them hold a run
//! ticket, the rest wait for one, first in first out. Before #1060 those calls
//! waited in the mailbox, where no clock ran; now they wait INSIDE the
//! `message_timeout` backstop of their worker. So the last of the
//! `credential_wait_max + 1` waiters must get its ticket and finish its own
//! call inside that budget:
//!
//! ```text
//! ceil((credential_wait_max + 1) / max_concurrency) × T + T  ≤  message_timeout
//! ```
//!
//! with `T` the per-call operation timeout of the cell (`params.timeout_ms`,
//! the provider call a code script declares, else `params.external_timeout_ms`,
//! the roundtrip timeout of a `web_search`) and `message_timeout` the cell's own
//! or, without one, the colony's `message_timeout_default_ms`. With the default
//! `credential_wait_max` of 16, `file-space/embed` (max_concurrency 2, 20 s,
//! 90 s) would need 9 × 20 + 20 = 200 s.
//!
//! Two locks: the rule over every shipped granted cell (static), and a
//! scaled-down run of the same arithmetic through the real `code` factory and
//! dispatcher (dynamic).

#[path = "support/gh1060.rs"]
mod support;

use meclaw_cells::code::CodeCellFactory;
use meclaw_core::serde_json::{self, Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use support::{
    Stub, error_code, is_credential_request, recipient_of, sealed_box, spawn_with_message_timeout,
};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The granted cells this wave ships (presence guard, GH #49): the sweep below
/// must find at least these, so a sweep over a wrong root cannot pass empty.
const KNOWN_GRANTED: &[&str] = &[
    "templates/file-space/embed/config.json",
    "templates/memory-hive/embed/config.json",
    "templates/tools/web_search/config.json",
    "templates/_cell-types/web_search-min/config.json",
    "templates/research-assistant/searcher/config.json",
];

fn json_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<_> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    paths.sort();
    for p in paths {
        if p.is_dir() {
            json_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "json") {
            out.push(p);
        }
    }
}

/// Every `{cell: {type: code|web_search}, params: {credential_grant_id, ..}}`
/// object inside `v`, wherever it sits (a cell config or a cell inlined in a
/// template).
fn granted_cells<'a>(v: &'a Value, out: &mut Vec<&'a Value>) {
    match v {
        Value::Object(map) => {
            let ty = map
                .get("cell")
                .and_then(|c| c.get("type"))
                .and_then(Value::as_str);
            let granted = map
                .get("params")
                .and_then(Value::as_object)
                .is_some_and(|p| p.contains_key("credential_grant_id"));
            if matches!(ty, Some("code" | "web_search")) && granted {
                out.push(v);
            }
            for child in map.values() {
                granted_cells(child, out);
            }
        }
        Value::Array(items) => {
            for child in items {
                granted_cells(child, out);
            }
        }
        _ => {}
    }
}

/// The worst ticket wait plus one own call (the rule of the module note).
fn worst_case_ms(wait_max: u64, mc: u64, op_ms: u64) -> u64 {
    (wait_max + 1).div_ceil(mc) * op_ms + op_ms
}

/// Lock (a): every shipped granted `code` / `web_search` cell fits its worst
/// ticket wait plus its own call into its `message_timeout`, with the values it
/// declares — and declares `credential_wait_max` in params and contract alike.
#[test]
fn gh1061_every_shipped_granted_cell_fits_its_ticket_wait_into_its_message_timeout() {
    let root = repo_root();
    let mut files = Vec::new();
    json_files(&root.join("templates"), &mut files);

    let default_wait_max = meclaw_cells::credential::default_credential_wait_max() as u64;
    let default_mt = meclaw_colony::ColonyConfig::default().message_timeout_default_ms;

    let mut found: Vec<String> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    for p in &files {
        let Ok(raw) = std::fs::read_to_string(p) else {
            continue;
        };
        let Ok(doc) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        let mut cells = Vec::new();
        granted_cells(&doc, &mut cells);
        let rel = p.strip_prefix(&root).unwrap_or(p).display().to_string();
        for cell in cells {
            found.push(rel.clone());
            let params = &cell["params"];
            let Some(mc) = params["max_concurrency"].as_u64().filter(|n| *n >= 1) else {
                failures.push(format!("{rel}: params.max_concurrency is not declared"));
                continue;
            };
            let (op_name, op_ms) = match (
                params["timeout_ms"].as_u64(),
                params["external_timeout_ms"].as_u64(),
            ) {
                (Some(t), _) => ("timeout_ms", t),
                (None, Some(t)) => ("external_timeout_ms", t),
                (None, None) => {
                    failures.push(format!(
                        "{rel}: neither params.timeout_ms nor params.external_timeout_ms is declared"
                    ));
                    continue;
                }
            };
            let (mt_name, mt_ms) = match cell["cell"]["message_timeout"].as_u64() {
                Some(t) => ("cell.message_timeout", t),
                None => ("colony message_timeout_default_ms", default_mt),
            };
            let declared = params["credential_wait_max"].as_u64();
            let wait_max = declared.unwrap_or(default_wait_max);
            let worst = worst_case_ms(wait_max, mc, op_ms);
            if worst > mt_ms {
                failures.push(format!(
                    "{rel}: ceil((credential_wait_max {wait_max}{dflt} + 1) / max_concurrency {mc}) \
                     × {op_name} {op_ms} + {op_ms} = {worst} ms > {mt_name} {mt_ms} ms — the last \
                     call waiting for a run ticket dies with message_timeout under load",
                    dflt = if declared.is_none() { " (default)" } else { "" },
                ));
            }
            match declared {
                None => failures.push(format!(
                    "{rel}: params.credential_wait_max is not declared (the default {default_wait_max} \
                     is sized for a mailbox wait, not for one inside message_timeout)"
                )),
                Some(w) => {
                    let s = &cell["contract"]["settings"]["credential_wait_max"];
                    if s["type"] != "number" || s["secret"] != false || s["default"] != json!(w) {
                        failures.push(format!(
                            "{rel}: contract.settings.credential_wait_max must be \
                             {{type: number, secret: false, default: {w}}}, is {s}"
                        ));
                    }
                }
            }
        }
    }

    for known in KNOWN_GRANTED {
        assert!(
            found.iter().any(|f| f == known),
            "presence guard (GH #49): the sweep did not find the granted cell {known} — \
             found {found:?}"
        );
    }
    assert!(
        failures.is_empty(),
        "{} granted cell(s) break ceil((wait_max + 1) / mc) × T + T ≤ message_timeout (GH #1061):\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}

// ─────────────────────────────────────────────────────────────── dynamic

/// The red run: set this to `Some(16)` (the substrate default) and lock (b)
/// fails with `message_timeout` — 17 waiters behind 2 tickets, the last one
/// waits ceil(17 / 2) = 9 calls (9 × S) plus its own S = 10 S against a budget
/// of 4.5 S. With the shipped value (2): 3 waiters, ceil(3 / 2) = 2 calls plus
/// its own = 3 S ≤ 4.5 S, every call answers.
const RED_RUN_WAIT_MAX: Option<u64> = None;

/// One scaled provider call, in ms. Large against a cold python start (the
/// margin of the shipped ratio is 1.5 S, so per-call overhead up to S / 2 stays
/// green), small enough to keep the lock under 20 s.
const S_MS: u64 = 1_000;

/// `(max_concurrency, credential_wait_max, message_timeout / timeout_ms)` of the
/// shipped `file-space/embed`.
fn shipped_file_space_embed() -> (u64, u64, f64) {
    let path = repo_root().join("templates/file-space/embed/config.json");
    let cfg: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("file-space/embed config"))
            .expect("config json");
    let p = &cfg["params"];
    let mc = p["max_concurrency"]
        .as_u64()
        .expect("params.max_concurrency");
    let wait_max = p["credential_wait_max"].as_u64().unwrap_or_else(|| {
        panic!("file-space/embed declares no params.credential_wait_max (GH #1061)")
    });
    let mt = cfg["cell"]["message_timeout"]
        .as_u64()
        .expect("cell.message_timeout");
    let op = p["timeout_ms"].as_u64().expect("params.timeout_ms");
    (mc, wait_max, mt as f64 / op as f64)
}

fn calling_script(url: &str) -> String {
    format!(
        r#"
import json, sys, urllib.request
sys.stdin.read()
urllib.request.urlopen("{url}", timeout=20).read()
print(json.dumps({{"header": {{}}, "messages": [{{"origin": "tool", "type": "tool_result", "id": "", "text": "ok"}}]}}))
"#
    )
}

fn call(i: usize) -> Value {
    json!({"messages": [{"origin": "user", "type": "text", "text": format!("{i}")}]})
}

/// Lock (b): the shipped `file-space/embed` arithmetic, scaled from 20 s to
/// `S_MS`: a code cell on a grant, `max_concurrency` and `credential_wait_max`
/// from the template, `message_timeout` = S × (90 s / 20 s). After the box is
/// open, a sustained burst of 3 × (mc + wait_max + 1) calls — three full
/// dispatcher bounds, so waiters keep queueing behind running calls — and every
/// one of them answers from the script; none dies on the backstop.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1061_a_granted_code_cell_answers_every_call_of_a_sustained_burst() {
    let (mc, shipped_wait_max, ratio) = shipped_file_space_embed();
    let wait_max = RED_RUN_WAIT_MAX.unwrap_or(shipped_wait_max);
    let message_timeout = Duration::from_millis((S_MS as f64 * ratio) as u64);

    let stub = Stub::start(Duration::from_millis(S_MS), "{}".into());
    let mut cell = spawn_with_message_timeout(
        Arc::new(CodeCellFactory),
        json!({"runner": "python3", "script_inline": calling_script(&stub.url()),
               "max_concurrency": mc, "credential_grant_id": "g-1061",
               "credential_wait_max": wait_max}),
        Some(message_timeout),
    );

    // Open the box with one call.
    cell.send(call(0)).await;
    let request = cell.next().await;
    assert!(is_credential_request(&request), "{request}");
    cell.send(sealed_box(&recipient_of(&request), "stub-secret-1061"))
        .await;
    let first = cell.next().await;
    assert_eq!(error_code(&first), None, "the first call ran: {first}");

    // The sustained burst, all at once into the mailbox.
    let burst = usize::try_from(3 * (mc + wait_max + 1)).expect("burst size");
    for i in 1..=burst {
        cell.send(call(i)).await;
    }
    for n in 0..burst {
        let reply = cell.next().await;
        assert_eq!(
            error_code(&reply),
            None,
            "reply {n} of {burst}: a call died waiting for a run ticket (mc {mc}, \
             credential_wait_max {wait_max}, S {S_MS} ms, message_timeout {message_timeout:?}): {reply}"
        );
        assert_eq!(
            reply["messages"][0]["text"], "ok",
            "reply {n} of {burst} is the script's answer: {reply}"
        );
    }
    let extra = cell.drain(Duration::from_millis(300)).await;
    assert!(
        extra.is_empty(),
        "nothing after the burst's answers (no second round, no backstop): {extra:?}"
    );
    assert_eq!(
        stub.requests(),
        burst + 1,
        "every call reached the endpoint once"
    );
    assert!(
        stub.max_in_flight() <= usize::try_from(mc).expect("mc"),
        "the run tickets kept the bound of {mc} (max in flight {})",
        stub.max_in_flight()
    );
}

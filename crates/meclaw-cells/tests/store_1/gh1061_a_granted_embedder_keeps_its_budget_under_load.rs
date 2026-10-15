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
//! with `T` the time one call holds its run ticket and `message_timeout` the
//! cell's own or, without one, the colony's `message_timeout_default_ms`. `T`
//! is `params.external_timeout_ms`: a `code` cell holds its ticket until the
//! script has answered (`code/cell.rs`), so for the A-timeout of the whole run
//! (`code/pool.rs` `Job::timeout`), plus `params.attachment_timeout_ms` where a
//! cell declares it (reads under the ticket); a `web_search` cell for its
//! roundtrip. `params.timeout_ms` of an embed script bounds one provider
//! attempt inside the run and is no `T` (re-check tv-br M6, C-M1). With the
//! default `credential_wait_max` of 16, `file-space/embed` (max_concurrency 2,
//! 42 s, 139 s) would need 9 × 42 + 42 = 420 s.
//!
//! Three locks: the rule over every shipped granted cell (static, here); the
//! same arithmetic RUN with the shipped values through the real dispatcher and
//! run tickets under the paused clock, where a time verdict is exact
//! (`meclaw-cells/src/grant_slot.rs`,
//! `gh1061_every_call_of_a_sustained_burst_gets_its_ticket_inside_message_timeout`);
//! and a sustained burst through the real `code` factory with python children
//! (dynamic, here), which judges conditions, not tempo.

// harness_fold: #[path = "support/gh1060.rs"]
#[allow(unused_imports)]
use crate::support; // harness_fold: mod support;

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
/// must find every one of these that is on disk, so a sweep over a wrong root
/// cannot pass empty. Two of them (`_cell-types/web_search-min`,
/// `research-assistant`) are private templates and do not travel with the
/// export; in the public clone each entry is guarded per file by
/// `.exists()`, and at least one must be there (the anti-vacuum floor).
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
            let Some(external_ms) = params["external_timeout_ms"].as_u64() else {
                failures.push(format!(
                    "{rel}: params.external_timeout_ms (the time a call holds its run ticket) \
                     is not declared"
                ));
                continue;
            };
            let (op_name, op_ms) = match params["attachment_timeout_ms"].as_u64() {
                Some(att) => (
                    "(external_timeout_ms + attachment_timeout_ms)",
                    external_ms + att,
                ),
                None => ("external_timeout_ms", external_ms),
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

    let mut present = 0;
    for known in KNOWN_GRANTED {
        if !root.join(known).exists() {
            continue; // GH #49: a private template, absent from the public clone
        }
        present += 1;
        assert!(
            found.iter().any(|f| f == known),
            "presence guard (GH #49): the sweep did not find the granted cell {known} — \
             found {found:?}"
        );
    }
    assert!(
        present > 0,
        "presence guard (GH #49): none of {KNOWN_GRANTED:?} is under {} -- the sweep root is wrong",
        root.display()
    );
    assert!(
        failures.is_empty(),
        "{} granted cell(s) break ceil((wait_max + 1) / mc) × T + T ≤ message_timeout (GH #1061):\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}

// ─────────────────────────────────────────────────────────────── dynamic

/// `(max_concurrency, credential_wait_max)` of the shipped `file-space/embed`.
fn shipped_file_space_embed() -> (u64, u64) {
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
    (mc, wait_max)
}

fn calling_script(url: &str) -> String {
    format!(
        r#"
import json, sys, urllib.request
sys.stdin.read()
urllib.request.urlopen("{url}", timeout={read_s}).read()
print(json.dumps({{"header": {{}}, "messages": [{{"origin": "tool", "type": "tool_result", "id": "", "text": "ok"}}]}}))
"#,
        // tv-br M10o (B-M2 class): the script's read wait outlasts the stub's
        // hold (one scaled HANG), never a literal 20 s below the marks.
        read_s = 2 * meclaw_testing::deadline::hang_ms() / 1000
    )
}

/// The grant of lock (c); the slot task's queue line is keyed by it.
const GRANT: &str = "g-1061";

fn call(i: usize) -> Value {
    json!({"messages": [{"origin": "user", "type": "text", "text": format!("{i}")}]})
}

/// Lock (c): a code cell on a grant with `max_concurrency` and
/// `credential_wait_max` from the shipped `file-space/embed`. After the box is
/// open, a sustained burst of 3 × (mc + wait_max + 1) calls — three full
/// dispatcher bounds, so waiters keep queueing behind running calls — and every
/// one of them answers from the script, reaches the endpoint once and never
/// more than `mc` at a time.
///
/// The first wave is held by event, not for `S` ms (re-check R K7, tv-br
/// M10q-b): the stub keeps `mc` requests until the slot task has logged the
/// full dispatcher bound's other `wait_max + 1` calls as queued for a run
/// ticket. So the bound is reached (`max_in_flight == mc`, the positive
/// witness) while the queue is full, and a cell without its tickets sends its
/// extra calls into the closed gate and fails -- however late they come. The
/// old 1 s hold (`S_MS`) showed the overlap only if the python starts landed
/// inside it, which a CPU quota undid (re-check R, store_1 M1). Later waves
/// flow; one call holds its ticket for as long as its script takes.
///
/// WHY the backstop is the product backstop (twice ten hang marks) and not the scaled 4.5 S any more (TV-Z1,
/// TV-Z2, pattern M4): with S of real time and a python child per call, the
/// verdict "no call dies on `message_timeout`" was a tempo verdict. It went red
/// 1 in 160 loaded runs (1 in 80 on the owner's host; tv-br M5, reply 4 of 15,
/// `message_timeout` 6.95 s, while the same iteration's organism test took
/// 136.6 s against its usual 24-30 s); quiet and under busy loops alone the
/// round was 1.02-1.09 s with ticket hand-over gaps of 50-211 ms (M6 probes),
/// no lost ticket, FIFO kept. The arithmetic itself is now run exactly under
/// the paused clock (lock (b) in `grant_slot.rs`); here a stuck ticket shows as
/// the hang of `next()`, not as a slow verdict.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1061_a_granted_code_cell_answers_every_call_of_a_sustained_burst() {
    // Re-check R-D4: the recorder is installed before anything this test does
    // could log (`start_gated` reads it), never after the setup.
    let _ = meclaw_testing::log_recorder::global();
    let (mc, wait_max) = shipped_file_space_embed();
    // Re-check R H4 / K8: B and the credential round from the one product
    // pair (`credential_wait_ms` = A = `product_wait_ms`, B = twice that),
    // never a second formula and never the 10 s default of the round.
    let message_timeout = Duration::from_millis(meclaw_testing::deadline::product_backstop_ms());
    let bound = usize::try_from(mc).expect("mc");

    let stub = Stub::start_gated(bound, "{}".into());
    let mut cell = spawn_with_message_timeout(
        Arc::new(CodeCellFactory),
        json!({"runner": "python3", "script_inline": calling_script(&stub.url()),
               "max_concurrency": mc, "credential_grant_id": GRANT,
               "credential_wait_max": wait_max,
               "credential_wait_ms": meclaw_testing::deadline::product_wait_ms()}),
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

    // The sustained burst, all at once into the mailbox, its first wave held
    // at the closed gate until the cell shows the rest queued.
    stub.close();
    let burst = usize::try_from(3 * (mc + wait_max + 1)).expect("burst size");
    for i in 1..=burst {
        cell.send(call(i)).await;
    }
    stub.hold_wave(GRANT, usize::try_from(wait_max + 1).expect("queued"))
        .await;
    for n in 0..burst {
        let reply = cell.next().await;
        assert_eq!(
            error_code(&reply),
            None,
            "reply {n} of {burst}: a call died waiting for a run ticket (mc {mc}, \
             credential_wait_max {wait_max}, message_timeout {message_timeout:?}): {reply}"
        );
        assert_eq!(
            reply["messages"][0]["text"], "ok",
            "reply {n} of {burst} is the script's answer: {reply}"
        );
    }
    // Absence by quiescence (dev-rules § 11, re-check tv-br M10g): once every
    // delivery's consume mark is in, a second round or a backstop answer
    // would have had to come before it -- no quiet window decides.
    let extra = cell.settle().await;
    assert!(
        extra.is_empty(),
        "nothing after the burst's answers (no second round, no backstop): {extra:?}"
    );
    assert_eq!(
        stub.requests(),
        burst + 1,
        "every call reached the endpoint once"
    );
    assert_eq!(
        stub.max_in_flight(),
        bound,
        "the run tickets reached and kept the bound of {mc}"
    );
}

/// Re-check tv-br M6 M5 (TV-Z3): the mark lock (c) lays a stuck ticket on --
/// `next()` of a cell that emits nothing more -- is a `HANG:` marker that names
/// its condition and the last observed state, not a bare literal timeout.
#[tokio::test(start_paused = true)]
async fn gh1061_the_next_emission_mark_is_a_hang_marker_with_its_state() {
    let (tx, mut out) = tokio::sync::mpsc::channel(1);
    let waiter = tokio::spawn(async move {
        let mut taken = support::Taken::default();
        support::next_emission(&mut out, &mut taken).await
    });
    let panic = waiter.await.expect_err("a cell that emits nothing hangs");
    drop(tx);
    let payload = panic.into_panic();
    let msg = payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
        .unwrap_or_default();
    assert!(
        msg.starts_with("HANG: the next emission of the cell"),
        "the mark is a HANG: marker naming its condition: {msg}"
    );
    assert!(
        msg.contains("count: 0") && msg.contains("last: None"),
        "the mark names the emissions taken so far: {msg}"
    );
}

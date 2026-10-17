//! GH #1225 (K3, ruling K14 A) -- a translation that failed for want of its
//! key is asked again at the next boot of the colony.
//!
//! Measured on soul2me 0.22 against meclaw 0.64.3: the model announcement of a
//! birth fires in the one-shot apply of stage 1, while the vault is still empty
//! and locked. `./translate` asks the access hive for its bearer, the broker
//! denies, the translator answers `meta.error {source: "auth"}`, and the hand
//! journalled `translation_failed` and never asked again -- not even after a
//! restart with an open vault, because a known brain announced again is
//! nothing. No subscriber of that requirement ever got its package.
//!
//! | claim | test |
//! |---|---|
//! | the real translator's denied round is an `auth` failure, and the hand keeps exactly that one waiting, with a mark | [`a_denied_credential_round_is_kept_waiting_and_a_wire_failure_is_not`] |
//! | only an announcement of a BOOT receipt reads the waiting questions, and it stamps them in the same bundle, so each is asked once per boot | [`only_a_boot_reads_the_waiting_questions_and_asks_each_once_per_boot`] |
//! | the last page of a boot's retry forgets only the questions no page of that run asked | [`a_paged_boot_retry_forgets_only_what_no_page_asked`] |
//! | ...nor one an earlier page found held open by another op and so did not ask | [`a_paged_boot_retry_keeps_what_an_earlier_page_found_held_open`] |
//! | a boot drops the claims an earlier run held on the questions it asks, before its world read (F-KL-1) | [`a_boot_takes_over_the_claims_of_its_waiting_questions`] |
//! | a key that arrives after its round's deadline reads the waiting questions like a boot; a box of an earlier process stays on the boot road (F-KL-1) | [`a_key_that_arrives_late_asks_the_waiting_questions_again`] |
//! | an empty vault at birth, a boot that is still without the key (stays open, marked twice), a boot with it (`in_model` arrives, the brain runs the answer), and a boot after that asks nothing -- each boot over the colony's own boot receipt, announced twice as the shell does, starting one retry | [`a_translation_without_its_key_is_asked_again_at_the_next_boot`] |
//! | a translation that failed for another reason is not asked at boot | [`a_translation_that_failed_otherwise_is_not_asked_at_boot`] |
//! | the shell's boot receipt reaches the hand with its `form` | [`the_shells_announcement_keeps_the_receipts_form`] |
//!
//! Counted, never timed: the translator stand-in appends one line per question
//! to a file, and every claim about "asked again" or "asked nothing" is that
//! count.

// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::LlmParams;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::llm::LlmCell;
use meclaw_cells::store::StoreCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, StatefulCell, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, Headers, Message, MessageBuilder, OutputSink, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use mock_openai::{MockOpenAI, canned_chat_completion};
use std::sync::Arc;

use tokio::sync::mpsc;

const GRANT: &str = "grant:openrouter@colony-os/llm-registry-translate";
const FAST: &str = "test/fast";
const BIRTH: &str = "birth/brain";
const BRAIN: &str = "/gen/brain";
const R_TALK: &str = "Talks with one person in real time; short answers, low latency, tools, a \
                      modest price.";

fn registry_root() -> Option<std::path::PathBuf> {
    let root =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/llm-registry");
    root.join("hand/config.json").exists().then_some(root)
}

// ───────────────────────────────────────────────────────── the hand, run alone

/// The shipped hand, run once over one message the way a `code` cell runs it.
fn run_hand(root: &std::path::Path, flat: Value) -> Vec<Value> {
    use std::io::Write;
    let cfg: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(root.join("hand/config.json")).unwrap(),
    )
    .unwrap();
    let script = cfg["params"]["script_inline"].as_str().unwrap_or_default();
    let stdin_doc = meclaw_testing::code_stdin(&flat).to_string();
    let src = format!(
        "import sys, io\n_script = {}\nsys.stdin = io.StringIO({})\n\
         exec(compile(_script, 'cell', 'exec'), globals())\n",
        meclaw_core::serde_json::to_string(script).unwrap(),
        meclaw_core::serde_json::to_string(&stdin_doc).unwrap(),
    );
    let mut child = std::process::Command::new("python3")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("python3");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(src.as_bytes())
        .expect("write program");
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "hand exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    match meclaw_core::serde_json::from_slice::<Value>(&out.stdout).expect("hand output is JSON") {
        Value::Array(a) => a,
        one => vec![one],
    }
}

/// Every store op the hand's output asks for, parsed.
fn store_ops(out: &[Value]) -> Vec<Value> {
    out.iter()
        .filter(|m| m["header"]["route"] == "rstore")
        .flat_map(|m| m["messages"].as_array().cloned().unwrap_or_default())
        .map(|t| meclaw_core::serde_json::from_str(t["text"].as_str().unwrap_or("null")).unwrap())
        .collect()
}

fn journalled(ops: &[Value], reason: &str) -> usize {
    ops.iter()
        .filter(|o| {
            o["operation"] == "insert"
                && o["table"] == "resolutions"
                && o["row"]["reason"] == reason
        })
        .count()
}

fn waits_written(ops: &[Value]) -> Vec<Value> {
    ops.iter()
        .filter(|o| o["operation"] == "insert" && o["table"] == "credential_waits")
        .map(|o| o["row"].clone())
        .collect()
}

/// The translator's answer as it arrives at the hand: its hop and body, with
/// the context the internal edge put on the question.
fn translated(hop: Value, body: Value, carry: Value) -> Value {
    let mut flat = body;
    flat["header"] = json!({
        "hop": hop,
        "context": {"registry_origin": "translate", "lr_phase": "translated",
                    "lr_carry": carry.to_string()},
    });
    flat
}

fn sink_for(tx: &mpsc::Sender<CellEmission>) -> OutputSink {
    OutputSink::new(
        tx.clone(),
        Path::new("/llm"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        Headers::new(),
        None,
    )
}

fn turn_to(cell: &str, body: Value) -> Message {
    MessageBuilder::new(Path::new(cell))
        .body(Body::Inline(body))
        .build()
}

/// The answer the REAL translator gives when the broker denies its key (the
/// measured round: `vault.deliver` -> `vault_invalid_input`), fed to the hand.
#[tokio::test]
async fn a_denied_credential_round_is_kept_waiting_and_a_wire_failure_is_not() {
    let Some(root) = registry_root() else {
        return;
    };
    let td = tempfile::TempDir::new().unwrap();
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db"))
        .expect("cell.db");
    let mut db = meclaw_colony::DbConn::wrap(conn, None);
    let mut cell = LlmCell::new(
        LlmParams::parse(&json!({
            "provider": "openai", "model": "test/translator", "api_key": "",
            "credential_grant_id": GRANT, "base_url": "http://127.0.0.1:9/v1",
            "external_timeout_ms": 5_000u64, "credential_wait_ms": 600_000u64,
        }))
        .expect("params"),
        reqwest::Client::builder().build().expect("http client"),
    );
    let (tx, mut rx) = mpsc::channel(64);
    let question =
        json!({"messages": [{"origin": "user", "type": "text", "text": "which model?"}]});
    cell.handle(turn_to("/llm", question), &sink_for(&tx), &mut db)
        .await;
    let request = rx.try_recv().expect("the credential request").content;
    let call_id = request["messages"][0]["id"]
        .as_str()
        .expect("call id")
        .to_string();
    let denial = json!({"messages": [{"origin": "tool", "type": "tool_result", "id": call_id,
        "text": json!({"outcome": "denied", "reason_code": "vault_invalid_input",
                       "grant_id": GRANT}).to_string()}]});
    cell.handle(turn_to("/llm", denial), &sink_for(&tx), &mut db)
        .await;
    let answered = rx
        .try_recv()
        .expect("the parked question is answered")
        .content;
    assert_eq!(answered["meta"]["error"]["source"], "auth", "{answered}");

    let mut hop = answered["header"].clone();
    let mut body = answered.clone();
    body.as_object_mut().unwrap().remove("header");
    let carry = json!({"rh": "r1", "ch": "c1", "cid": "claim-1", "requirement": R_TALK});
    let ops = store_ops(&run_hand(
        &root,
        translated(hop.clone(), body.clone(), carry.clone()),
    ));
    assert_eq!(journalled(&ops, "translation_failed"), 1, "{ops:?}");
    assert_eq!(
        journalled(&ops, "translation_awaits_credential"),
        1,
        "the mark: {ops:?}"
    );
    let waits = waits_written(&ops);
    assert_eq!(waits.len(), 1, "one waiting question: {ops:?}");
    assert_eq!(waits[0]["requirement_hash"], "r1");
    assert_eq!(waits[0]["catalogue_hash"], "c1");
    assert_eq!(waits[0]["tries"], 1, "{waits:?}");
    assert!(
        ops.iter().any(|o| o["operation"] == "delete"
            && o["table"] == "open_questions"
            && o["where"]["claim"] == "claim-1"),
        "the open question closes as before: {ops:?}"
    );

    // The same failure from the wire (a provider's 401, a 500): as before --
    // journalled, closed, and nothing waits for a boot.
    hop["error_code"] = json!("auth");
    body["meta"]["error"]["source"] = json!("wire");
    let ops = store_ops(&run_hand(&root, translated(hop, body, carry)));
    assert_eq!(journalled(&ops, "translation_failed"), 1, "{ops:?}");
    assert_eq!(
        journalled(&ops, "translation_awaits_credential"),
        0,
        "{ops:?}"
    );
    assert!(waits_written(&ops).is_empty(), "nothing waits: {ops:?}");
    assert!(
        ops.iter().any(|o| o["operation"] == "delete"
            && o["table"] == "credential_waits"
            && o["where"]["requirement_hash"] == "r1"),
        "a question that got past its key waits no longer: {ops:?}"
    );
}

fn announcement(form: &str, trace: &str) -> Value {
    json!({
        "trace_id": trace,
        "header": {
            "hop": {"route": "in_hand", "form": form},
            "context": {"model_announcer": "meclaw-os", "model_generation": "/gen",
                        "model_announced": json!([{"cell_path": BRAIN, "start_model": BIRTH,
                                                   "requirement": R_TALK}]).to_string()},
        },
        "messages": [],
    })
}

fn boot_echo(rows: Value, trace: &str) -> Value {
    json!({
        "header": {
            "hop": {"operation": "bundle", "rows_affected": 0, "bundle_errors": 0},
            "context": {"lr_phase": "boot_waits", "lr_carry": json!({"boot": trace}).to_string()},
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "w", "text": rows.to_string()}],
    })
}

fn reads_waits(out: &[Value]) -> bool {
    store_ops(out)
        .iter()
        .any(|o| o["operation"] == "select" && o["table"] == "credential_waits")
}

#[test]
fn only_a_boot_reads_the_waiting_questions_and_asks_each_once_per_boot() {
    let Some(root) = registry_root() else {
        return;
    };
    let trace = "0192a000-0000-7000-8000-000000000001";
    assert!(
        !reads_waits(&run_hand(&root, announcement("manifest", trace))),
        "a mutation's receipt does not read them"
    );

    // A boot reads them and stamps them in ONE bundle: the store runs a bundle
    // whole, so the second announcement of the same boot (the shell sends two)
    // finds every row stamped and reads nothing -- whatever the order of the
    // two reads and the two echoes.
    let out = run_hand(&root, announcement("boot", trace));
    let bundle = out
        .iter()
        .find(|m| m["header"]["phase"] == "boot_waits")
        .unwrap_or_else(|| panic!("a boot reads them: {out:?}"));
    let ops = store_ops(std::slice::from_ref(bundle));
    let unasked = json!({"asked_boot": {"neq": trace}});
    assert_eq!(ops.len(), 2, "{ops:?}");
    assert_eq!(ops[0]["operation"], "select", "{ops:?}");
    assert_eq!(ops[0]["table"], "credential_waits", "{ops:?}");
    assert_eq!(ops[0]["where"], unasked, "{ops:?}");
    assert_eq!(ops[1]["operation"], "update", "{ops:?}");
    assert_eq!(ops[1]["table"], "credential_waits", "{ops:?}");
    assert_eq!(ops[1]["set"], json!({"asked_boot": trace}), "{ops:?}");
    assert_eq!(ops[1]["where"], unasked, "{ops:?}");

    // What the read found is asked -- through the hand's own retranslate,
    // narrowed to exactly that question, and nothing more is written.
    let rows = json!([
        {"requirement_hash": "r-open", "catalogue_hash": "c", "tries": 1, "asked_boot": "an-older-boot"},
    ]);
    let out = run_hand(&root, boot_echo(rows, trace));
    let ops = store_ops(&out);
    assert!(
        !ops.iter().any(|o| o["table"] == "credential_waits"),
        "the stamp went with the read: {ops:?}"
    );
    let world = out
        .iter()
        .find(|m| m["header"]["phase"] == "world")
        .unwrap_or_else(|| panic!("the retry reads its world: {out:?}"));
    let carry: Value =
        meclaw_core::serde_json::from_str(world["header"]["carry"].as_str().unwrap()).unwrap();
    assert_eq!(carry["cmd"]["op"], "retranslate", "{carry}");
    assert_eq!(carry["cmd"]["only"], json!(["r-open"]), "{carry}");
    assert_eq!(carry["cmd"]["boot"], trace, "{carry}");
    assert_eq!(carry["cmd"]["tries"], json!({"r-open": 1}), "{carry}");
    assert!(
        !out.iter().any(|m| m["header"]["route"] == "ack"),
        "nobody asked, so nobody is answered: {out:?}"
    );

    // The second announcement's read found nothing: nothing.
    assert_eq!(
        run_hand(&root, boot_echo(json!([]), trace)),
        Vec::<Value>::new()
    );

    // A retry that fails on its key again stays waiting, one try more, under
    // this boot, and is marked again.
    let carry = json!({"rh": "r-open", "ch": "c", "cid": "claim-2", "requirement": R_TALK,
                       "boot": trace, "tries": 1});
    let ops = store_ops(&run_hand(
        &root,
        translated(
            json!({"finish_reason": "error", "error_code": "credential_pending"}),
            json!({"messages": [], "meta": {"error": {"source": "auth", "detail": "did not arrive; retry"}}}),
            carry,
        ),
    ));
    let waits = waits_written(&ops);
    assert_eq!(waits.len(), 1, "{ops:?}");
    assert_eq!(waits[0]["tries"], 2, "{waits:?}");
    assert_eq!(waits[0]["asked_boot"], trace, "{waits:?}");
    assert_eq!(
        journalled(&ops, "translation_awaits_credential"),
        1,
        "{ops:?}"
    );
}

/// The last page of a boot's retry, as the store answers its read: a catalogue
/// of one model, and the one subscriber of this page with its requirement.
fn last_page_of_a_boot_retry(trace: &str) -> Value {
    let cmd = json!({
        "op": "retranslate", "internal": true, "actor": "boot", "tier": "", "model_id": "",
        "call_id": "", "only": ["r-a", "r-b", "r-gone"], "boot": trace,
        "tries": {"r-a": 1, "r-b": 1, "r-gone": 1},
        "page": 2, "asked": ["r-a"],
        "probe": {"page": 2, "ch": "c", "rhs": ["r-b"], "ids": [FAST],
                  "cutoff": "2000-01-01T00:00:00Z"},
    });
    let model = json!({"model_id": FAST, "provider": "gateway", "base_url": "", "wire_dialect": "",
                       "context_window": 32000, "cost_in": 1, "cost_out": 1,
                       "caps": {"tools": true}, "traits": {}, "strengths": "quick",
                       "status": "active", "note": "", "package": {}, "prompt": ""});
    let sub = json!({"cell_path": "/gen/b", "tier": "", "pinned": 0, "start_model": BIRTH,
                     "package_hash": "", "model_id": "", "rank": "", "reason": "", "since": "",
                     "requirement": R_TALK, "requirement_hash": "r-b"});
    let results = [
        json!([model]),
        json!([]),
        json!([]),
        json!([sub]),
        json!([]),
        json!([]),
        json!([]),
    ];
    json!({
        "header": {
            "hop": {"operation": "bundle", "rows_affected": 0, "bundle_errors": 0},
            "context": {"lr_phase": "world", "lr_carry": json!({"cmd": cmd}).to_string()},
        },
        "messages": results.iter().enumerate()
            .map(|(i, r)| json!({"origin": "tool", "type": "tool_result", "id": format!("w{i}"),
                                 "text": r.to_string()}))
            .collect::<Vec<_>>(),
    })
}

/// A boot's retry pages through the subscribers of its requirements. Its last
/// page forgets the waiting questions that no page of THIS run found -- not
/// the ones an earlier page asked, whose answer (or second failure) is still
/// to come and must find the row with its `tries`.
#[test]
fn a_paged_boot_retry_forgets_only_what_no_page_asked() {
    let Some(root) = registry_root() else {
        return;
    };
    let ops = store_ops(&run_hand(
        &root,
        last_page_of_a_boot_retry("0192a000-0000-7000-8000-000000000002"),
    ));
    let mut forgotten: Vec<String> = ops
        .iter()
        .filter(|o| o["operation"] == "delete" && o["table"] == "credential_waits")
        .map(|o| {
            o["where"]["requirement_hash"]
                .as_str()
                .unwrap_or("")
                .to_string()
        })
        .collect();
    forgotten.sort();
    assert_eq!(forgotten, vec!["r-gone".to_string()], "{ops:?}");
}

/// One page of a boot's retry as the store answers its read, the page bound at
/// one subscriber: a catalogue of one model, the page's subscribers, and per
/// requirement of the page its three reads -- no translation for this
/// catalogue, none of any, and the open question of another op where `open`.
fn boot_retry_page(cmd: &Value, subs: &[Value], open: &[bool]) -> Value {
    let model = json!({"model_id": FAST, "provider": "gateway", "base_url": "", "wire_dialect": "",
                       "context_window": 32000, "cost_in": 1, "cost_out": 1,
                       "caps": {"tools": true}, "traits": {}, "strengths": "quick",
                       "status": "active", "note": "", "package": {}, "prompt": ""});
    let mut results = vec![
        json!([model]),
        json!([]),
        json!([]),
        Value::Array(subs.to_vec()),
    ];
    for held in open {
        results.push(json!([]));
        results.push(json!([]));
        results.push(if *held {
            json!([{"claim": "another-op"}])
        } else {
            json!([])
        });
    }
    json!({
        "params": {"subscriber_rows": 1},
        "header": {
            "hop": {"operation": "bundle", "rows_affected": 0, "bundle_errors": 0},
            "context": {"lr_phase": "world", "lr_carry": json!({"cmd": cmd}).to_string()},
        },
        "messages": results.iter().enumerate()
            .map(|(i, r)| json!({"origin": "tool", "type": "tool_result", "id": format!("w{i}"),
                                 "text": r.to_string()}))
            .collect::<Vec<_>>(),
    })
}

fn forgotten_waits(ops: &[Value]) -> Vec<String> {
    let mut out: Vec<String> = ops
        .iter()
        .filter(|o| o["operation"] == "delete" && o["table"] == "credential_waits")
        .map(|o| {
            o["where"]["requirement_hash"]
                .as_str()
                .unwrap_or("")
                .to_string()
        })
        .collect();
    out.sort();
    out
}

/// A page of a boot's retry that finds a waiting question held open by another
/// op does not ask it -- that op's answer or failure is still to come. The
/// question was found all the same, so the last page, where it is no longer,
/// keeps its row and `tries`; only the one no page found is forgotten.
#[test]
fn a_paged_boot_retry_keeps_what_an_earlier_page_found_held_open() {
    let Some(root) = registry_root() else {
        return;
    };
    let trace = "0192a000-0000-7000-8000-000000000003";
    let cmd = json!({
        "op": "retranslate", "internal": true, "actor": "boot", "tier": "", "model_id": "",
        "call_id": "", "only": ["r-gone", "r-held"], "boot": trace,
        "tries": {"r-gone": 1, "r-held": 1}, "page": 1,
        "probe": {"page": 1, "ch": "c", "rhs": ["r-held"], "ids": [FAST],
                  "cutoff": "2000-01-01T00:00:00Z"},
    });
    let sub = json!({"cell_path": "/gen/a", "tier": "", "pinned": 0, "start_model": BIRTH,
                     "package_hash": "", "model_id": "", "rank": "", "reason": "", "since": "",
                     "requirement": R_TALK, "requirement_hash": "r-held"});

    // The first page is full and holds the question another op asked: not
    // asked again, nothing forgotten, and the next page is read.
    let out = run_hand(&root, boot_retry_page(&cmd, &[sub], &[true]));
    let ops = store_ops(&out);
    assert!(
        !ops.iter()
            .any(|o| o["operation"] == "insert" && o["table"] == "open_questions"),
        "a question another op holds open is not asked again: {ops:?}"
    );
    assert_eq!(forgotten_waits(&ops), Vec::<String>::new(), "{ops:?}");
    let next = out
        .iter()
        .find(|m| m["header"]["phase"] == "world")
        .unwrap_or_else(|| panic!("a full page reads the next one: {out:?}"));
    let carry: Value =
        meclaw_core::serde_json::from_str(next["header"]["carry"].as_str().unwrap()).unwrap();
    assert_eq!(carry["cmd"]["page"], 2, "{carry}");

    // The last page holds no subscriber of either requirement.
    let ops = store_ops(&run_hand(&root, boot_retry_page(&carry["cmd"], &[], &[])));
    assert_eq!(forgotten_waits(&ops), vec!["r-gone".to_string()], "{ops:?}");
}

/// The index of the first output message holding a store op that matches.
fn first_with(out: &[Value], pred: impl Fn(&Value) -> bool) -> Option<usize> {
    out.iter()
        .position(|m| store_ops(std::slice::from_ref(m)).iter().any(&pred))
}

/// F-KL-1, cause 1: the claims of a run that died. Measured on a production
/// colony: a boot asked its four waiting questions and claimed them, the
/// process restarted before the answer, and the claims (open for
/// `OPEN_QUESTION_SECONDS`) made the next two boots skip all four. A boot
/// restarted the translator, so a question parked in its RAM did not survive:
/// the boot drops the claims of exactly the questions it is about to ask,
/// in a bundle BEFORE its world read, and nothing else.
#[test]
fn a_boot_takes_over_the_claims_of_its_waiting_questions() {
    let Some(root) = registry_root() else {
        return;
    };
    let trace = "0192a000-0000-7000-8000-000000000003";
    let rows = json!([
        {"requirement_hash": "r-a", "catalogue_hash": "c", "tries": 1, "asked_boot": "an-older-boot"},
        {"requirement_hash": "r-b", "catalogue_hash": "c", "tries": 2, "asked_boot": ""},
        {"requirement_hash": "r-this", "catalogue_hash": "c", "tries": 1, "asked_boot": trace},
    ]);
    let out = run_hand(&root, boot_echo(rows, trace));
    let drop_claims = |o: &Value| o["operation"] == "delete" && o["table"] == "open_questions";
    let at = first_with(&out, drop_claims)
        .unwrap_or_else(|| panic!("the boot drops the claims: {out:?}"));
    let ops = store_ops(std::slice::from_ref(&out[at]));
    assert_eq!(
        ops,
        vec![json!({"operation": "delete", "table": "open_questions",
                    "where": {"requirement_hash": {"in": ["r-a", "r-b"]}}})],
        "only the claims of the questions this boot asks: {out:?}"
    );
    let world = out
        .iter()
        .position(|m| m["header"]["phase"] == "world")
        .unwrap_or_else(|| panic!("the retry reads its world: {out:?}"));
    assert!(
        at < world,
        "the claims go first, so the world read finds none: {out:?}"
    );

    // Nothing waiting, or everything already asked by this boot: nothing.
    assert_eq!(
        run_hand(&root, boot_echo(json!([]), trace)),
        Vec::<Value>::new()
    );
    let asked = json!([{"requirement_hash": "r-this", "catalogue_hash": "c", "tries": 1,
                        "asked_boot": trace}]);
    assert_eq!(
        run_hand(&root, boot_echo(asked, trace)),
        Vec::<Value>::new()
    );
}

/// F-KL-1, cause 2: the key that arrives after the round's deadline. The
/// translator (`credential_late_notice`, pinned on the cell in
/// `llm::cell::tests::gh1225_*`) reports the box once; the hand reads the
/// waiting questions exactly as a boot does, under a mark of its own, and the
/// boot path asks them. Nothing is journalled and nothing is written.
#[test]
fn a_key_that_arrives_late_asks_the_waiting_questions_again() {
    let Some(root) = registry_root() else {
        return;
    };
    let carry = json!({"rh": "r1", "ch": "c1", "cid": "claim-1", "requirement": R_TALK});
    let notice = json!({"messages": [], "meta": {"provider": "openai",
        "credential": {"outcome": "arrived_late", "grant_id": GRANT}}});
    let out = run_hand(&root, translated(json!({}), notice, carry.clone()));
    assert_eq!(out.len(), 1, "one bundle: {out:?}");
    assert_eq!(out[0]["header"]["phase"], "boot_waits", "{out:?}");
    let mark =
        meclaw_core::serde_json::from_str::<Value>(out[0]["header"]["carry"].as_str().unwrap())
            .unwrap()["boot"]
            .as_str()
            .unwrap_or_default()
            .to_string();
    assert!(mark.starts_with("credential:") && mark.len() > 11, "{mark}");
    let ops = store_ops(&out);
    let unasked = json!({"asked_boot": {"neq": mark}});
    assert_eq!(ops.len(), 2, "{ops:?}");
    assert_eq!(ops[0]["operation"], "select", "{ops:?}");
    assert_eq!(ops[0]["table"], "credential_waits", "{ops:?}");
    assert_eq!(ops[0]["where"], unasked, "{ops:?}");
    assert_eq!(ops[1]["operation"], "update", "{ops:?}");
    assert_eq!(ops[1]["set"], json!({"asked_boot": mark}), "{ops:?}");
    assert_eq!(ops[1]["where"], unasked, "{ops:?}");

    // Fail-closed: the box of an EARLIER process is refused by the cell (it
    // cannot open it) and stays on today's road -- journalled, kept waiting
    // for the next boot, never read as an arrival.
    let refused = json!({"messages": [], "meta": {"provider": "openai", "error": {
        "source": "auth", "detail": "a sealed box arrived that this cell never asked for"}}});
    let out = run_hand(
        &root,
        translated(
            json!({"finish_reason": "error", "error_code": "invalid_input"}),
            refused,
            carry,
        ),
    );
    assert!(
        !out.iter().any(|m| m["header"]["phase"] == "boot_waits"),
        "{out:?}"
    );
    let ops = store_ops(&out);
    assert_eq!(journalled(&ops, "translation_failed"), 1, "{ops:?}");
    assert_eq!(
        journalled(&ops, "translation_awaits_credential"),
        1,
        "{ops:?}"
    );
    assert_eq!(waits_written(&ops).len(), 1, "{ops:?}");
}

/// The shell turns every receipt reaching `/os` into the judge's announcement
/// and restamps only its route, so the boot receipt's `form` arrives.
#[test]
fn the_shells_announcement_keeps_the_receipts_form() {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates/meclaw-os/config.json");
    let Ok(raw) = std::fs::read_to_string(&p) else {
        return;
    };
    let cfg: Value = meclaw_core::serde_json::from_str(&raw).unwrap();
    let edges = cfg["params"]["graph"]["edges"].as_array().expect("edges");
    let bridge = edges
        .iter()
        .find(|e| {
            e["to"] == "./llm-registry"
                && e["condition"]
                    .as_str()
                    .unwrap_or("")
                    .contains("mutation_committed")
        })
        .expect("the receipt's bridge into the registry");
    let modifier = &bridge["modifier"];
    assert_eq!(
        modifier["set_hop"],
        json!({"route": "'in_hand'"}),
        "{bridge}"
    );
    assert!(modifier.get("delete_hop").is_none(), "{bridge}");
    let reg = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates/llm-registry/config.json");
    let cfg: Value =
        meclaw_core::serde_json::from_str(&std::fs::read_to_string(reg).unwrap()).unwrap();
    for e in cfg["params"]["graph"]["edges"].as_array().expect("edges") {
        if e["to"] == "./hand" {
            assert!(e["modifier"].get("delete_hop").is_none(), "{e}");
            assert!(e["modifier"]["set_hop"].get("form").is_none(), "{e}");
        }
    }
}

// ─────────────────────────────────────────────────────────── the colony, rebooted

/// The translator's stand-in: one line per question into `asked`; with a file
/// `key` it answers FAST, with a file `wire` it fails on the wire, else it
/// fails exactly as the real cell does when its key does not arrive.
const TRANSLATOR: &str = r#"
import sys, json, os
doc = json.load(sys.stdin)
d = doc["params"]["dir"]
with open(os.path.join(d, "asked"), "a") as f:
    f.write("1\n")
if os.path.exists(os.path.join(d, "key")):
    out = {"header": {"finish_reason": "stop"},
           "messages": [{"origin": "assistant", "type": "text",
                         "text": json.dumps({"model_id": "test/fast", "reason": "it is fast"})}]}
elif os.path.exists(os.path.join(d, "wire")):
    out = {"header": {"finish_reason": "error", "error_code": "provider_error"},
           "messages": [], "meta": {"error": {"source": "wire", "detail": "HTTP 500"}}}
else:
    out = {"header": {"finish_reason": "error", "error_code": "credential_pending"},
           "messages": [], "meta": {"error": {"source": "auth",
           "detail": "the bearer credential was requested from the access hive but did not arrive; retry"}}}
sys.stdout.write(json.dumps(out))
"#;

/// A receipt as the shell hands it on: `form` from the text it is sent.
const RECEIPT: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
form = str(d["messages"][-1]["text"])
sys.stdout.write(json.dumps({"header": {"route": "receipt", "form": form}, "messages": []}))
"#;

/// One store op over the boot-graph edge into `./store`.
const ADMIN: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
raw = str(d["messages"][-1]["text"])
sys.stdout.write(json.dumps({"header": {"route": "astore"},
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "a1", "text": raw}]}))
"#;

/// The commanding side of the hand port.
const OPERATOR: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
raw = str(d["messages"][-1]["text"])
sys.stdout.write(json.dumps({"header": {"route": "command"},
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "c1", "text": raw}]}))
"#;

fn code_cell(script: &str, params: Value, emits_hop: Value) -> Value {
    let mut p = json!({"runner": "python3", "script_inline": script, "external_timeout_ms": 15000});
    for (k, v) in params.as_object().unwrap() {
        p[k] = v.clone();
    }
    json!({
        "cell": {"type": "code"},
        "params": p,
        "contract": {
            "version": "1.0.0", "settings": {}, "multi_send_capable": true,
            "emits": {"body": {"messages": {"type": "array", "required": true},
                               "meta": {"type": "object", "required": false}},
                      "hop": emits_hop},
            "consumes": {"body": {"messages": {"type": "array", "required": false},
                                  "system": {"type": "object", "required": false}}},
            "capabilities": ["shell:exec"]
        },
        "description": {"purpose": "Test stand-in.", "use_when": "Test fixture only.",
                        "not_in_scope": "Not a template."}
    })
}

fn route_only(route: &str) -> Value {
    json!({"route": {"type": "string", "values": [route], "required": false}})
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(v).unwrap()).unwrap();
}

fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        if from.is_dir() {
            copy_cells(&from, &dst.join(entry.file_name()));
        } else if entry.file_name() == "config.json" {
            std::fs::copy(&from, dst.join(entry.file_name())).unwrap();
        }
    }
}

/// The registry as shipped (without its seeds), its translator replaced by the
/// stand-in, one brain under a generation, and copies of everything the hand
/// says -- to the store, to the translator, to the brain -- at the sink, so
/// every wait below is on an event.
/// `meclaw-os`' bridge from the receipt lane into the registry's hand: the
/// route restamped, the announcement put on, `form` left as it came.
fn bridge(announced: &str, condition: &str) -> Value {
    json!({"from": ".", "to": "./llm_registry/hand", "condition": condition,
           "modifier": {"set_hop": {"route": "'in_hand'"},
                        "set_context": {"model_announcer": "'meclaw-os'", "model_generation": "'/gen'",
                                        "model_announced": format!("'{announced}'")},
                        "delete_context": ["actor"]}})
}

/// From now on every boot of the tree leaves the colony's own boot receipt
/// (`form: "boot"`) at the root hive -- the real path, not a stand-in.
fn receipts_at_the_root(td: &tempfile::TempDir) {
    write(
        td.path(),
        "colony.json",
        &json!({"schema_version": 1, "mutation_receipts": {"to": "/"}}),
    );
}

fn build_tree(td: &tempfile::TempDir, registry: &std::path::Path, brain_url: &str) {
    let root = td.path();
    let announced =
        json!([{"cell_path": BRAIN, "start_model": BIRTH, "requirement": R_TALK}]).to_string();
    write(
        root,
        "main/config.json",
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
            // The colony's own receipts (`colony.json` `mutation_receipts`),
            // bridged as `meclaw-os` bridges them -- and announced TWICE, as
            // the shell announces every one (its `.` edge and `orgs`'
            // `model_subscribe`), with one trace.
            bridge(&announced, "has(hop.route) && hop.route == 'mutation_committed'"),
            bridge(&announced, "has(hop.route) && hop.route == 'mutation_committed' && has(hop.form)"),
            {"from": "./receipts", "to": "./llm_registry/hand",
             "condition": "has(hop.route) && hop.route == 'receipt'",
             "modifier": {"set_hop": {"route": "'in_hand'"},
                          "set_context": {"model_announcer": "'meclaw-os'", "model_generation": "'/gen'",
                                          "model_announced": format!("'{announced}'")}}},
            {"from": "./operator", "to": "./llm_registry/hand",
             "condition": "has(hop.route) && hop.route == 'command'",
             "modifier": {"set_context": {"actor": "'operator'"}}},
            {"from": "./llm_registry/hand", "to": "./gen/brain",
             "condition": "has(hop.route) && hop.route == 'update' && hop.subscriber == '/gen/brain'"},
            {"from": "./llm_registry/hand", "to": "/sink",
             "condition": "has(hop.route) && (hop.route == 'update' || hop.route == 'error' || hop.route == 'answer' || hop.route == 'ack' || hop.route == 'rstore' || hop.route == 'translate')"},
            {"from": "./llm_registry/store", "to": "/sink",
             "condition": "context.registry_origin == 'hand' && (context.lr_phase == 'written' || context.lr_phase == 'boot_waits')"},
            {"from": "./llm_registry/translate", "to": "/sink", "condition": "has(hop.finish_reason)"},
            {"from": "./admin", "to": "./llm_registry/store",
             "condition": "has(hop.route) && hop.route == 'astore'",
             "modifier": {"set_context": {"registry_origin": "'admin'"}}},
            {"from": "./llm_registry/store", "to": "/sink", "condition": "context.registry_origin == 'admin'"},
            {"from": "./gen/brain", "to": "/sink", "condition": "has(hop.finish_reason)"}
        ]}}}),
    );
    write(
        root,
        "main/receipts/config.json",
        &code_cell(
            RECEIPT,
            json!({}),
            json!({
        "route": {"type": "string", "values": ["receipt"], "required": false},
        "form": {"type": "string", "required": false}}),
        ),
    );
    write(
        root,
        "main/admin/config.json",
        &code_cell(ADMIN, json!({}), route_only("astore")),
    );
    write(
        root,
        "main/operator/config.json",
        &code_cell(OPERATOR, json!({}), route_only("command")),
    );
    write(
        root,
        "main/gen/config.json",
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": []}}}),
    );
    write(
        root,
        "main/gen/brain/config.json",
        &json!({
            "cell": {"type": "llm"},
            "params": {"provider": "openai", "model": BIRTH, "api_key": "test-key-1225",
                       "base_url": brain_url, "requirement": R_TALK},
            "contract": {"version": "1.0.0", "settings": {},
                         "emits": {"body": {"messages": {"type": "array", "required": true},
                                            "meta": {"type": "object", "required": false}},
                                   "hop": {"finish_reason": {"type": "string", "required": true},
                                           "error_code": {"type": "string", "required": false}}},
                         "consumes": {"body": {"messages": {"type": "array", "required": false},
                                               "system": {"type": "object", "required": false}}},
                         "capabilities": ["network:llm", "db:own"]},
            "description": {"purpose": "A subscribed brain.", "use_when": "Test fixture only.",
                            "not_in_scope": "Not a template."}
        }),
    );
    copy_cells(registry, &root.join("main/llm_registry"));
    write(
        root,
        "main/llm_registry/translate/config.json",
        &code_cell(
            TRANSLATOR,
            // GH #1235 (T6-16): a `code` cell without `params.sandbox` boots
            // under the default-deny profile, and the stand-in counts its asks
            // in the tree's root -- so it declares that one writable directory.
            json!({"dir": root.to_string_lossy(),
                   "sandbox": {"trust": "restricted", "network": "deny",
                               "filesystem": {"runtime": true,
                                              "write": [root.to_string_lossy()]}}}),
            json!({"finish_reason": {"type": "string", "required": true},
               "error_code": {"type": "string", "required": false}}),
        ),
    );
}

async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<Message>) {
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            ("store".to_string(), Arc::new(StoreCellFactory)),
            ("llm".to_string(), Arc::new(LlmCellFactory)),
        ]
    };
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(1024);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");
    (h, sink_rx)
}

fn text_to(cell: &str, text: &str) -> Message {
    MessageBuilder::new(Path::new(cell))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .ttl(400)
        .build()
}

fn hop_of(m: &Message, key: &str) -> String {
    m.headers
        .hop
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn ctx_of(m: &Message, key: &str) -> String {
    m.headers
        .context
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn body_of(m: &Message) -> &Value {
    match &m.body {
        Body::Inline(v) => v,
        Body::Blob(_) => panic!("inline expected"),
    }
}

/// Everything the sink saw up to and including the first message `want` takes.
async fn recv_until(
    rx: &mut mpsc::Receiver<Message>,
    label: &str,
    want: impl Fn(&Message) -> bool,
) -> Vec<Message> {
    let mut seen = Vec::new();
    for _ in 0..400 {
        let m = tokio::time::timeout(meclaw_testing::deadline::HANG, rx.recv())
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| {
                let hops: Vec<_> = seen
                    .iter()
                    .map(|m: &Message| format!("{:?}", m.headers.hop))
                    .collect();
                panic!("nothing more arrived while waiting for {label}; saw {hops:?}")
            });
        let hit = want(&m);
        seen.push(m);
        if hit {
            return seen;
        }
    }
    panic!("{label} never arrived");
}

async fn recv_matching(
    rx: &mut mpsc::Receiver<Message>,
    label: &str,
    want: impl Fn(&Message) -> bool,
) -> Message {
    recv_until(rx, label, want).await.pop().unwrap()
}

fn is_translation(m: &Message) -> bool {
    !hop_of(m, "finish_reason").is_empty() && ctx_of(m, "registry_origin") == "translate"
}

fn is_echo(m: &Message, phase: &str) -> bool {
    !hop_of(m, "operation").is_empty()
        && ctx_of(m, "registry_origin") == "hand"
        && ctx_of(m, "lr_phase") == phase
}

async fn admin(h: &ColonyHandle, rx: &mut mpsc::Receiver<Message>, op: Value) -> Value {
    h.send(text_to("/admin", &op.to_string())).await;
    let m = recv_matching(rx, "admin answer", |m| {
        ctx_of(m, "registry_origin") == "admin"
    })
    .await;
    let text = body_of(&m)["messages"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    meclaw_core::serde_json::from_str(&text).unwrap_or(Value::Null)
}

async fn rows(
    h: &ColonyHandle,
    rx: &mut mpsc::Receiver<Message>,
    table: &str,
    columns: Value,
    filter: Value,
) -> Vec<Value> {
    admin(h, rx, json!({"operation": "select", "table": table, "columns": columns, "where": filter, "limit": 100}))
        .await
        .as_array()
        .cloned()
        .unwrap_or_default()
}

async fn waits(h: &ColonyHandle, rx: &mut mpsc::Receiver<Message>) -> Vec<Value> {
    rows(
        h,
        rx,
        "credential_waits",
        json!(["requirement_hash", "tries", "asked_boot"]),
        json!({}),
    )
    .await
}

async fn marks(h: &ColonyHandle, rx: &mut mpsc::Receiver<Message>) -> usize {
    rows(
        h,
        rx,
        "resolutions",
        json!(["reason"]),
        json!({"reason": "translation_awaits_credential"}),
    )
    .await
    .len()
}

fn asked(td: &tempfile::TempDir) -> usize {
    std::fs::read_to_string(td.path().join("asked"))
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

/// A receipt goes in; the translator answers; the hand's bundle on that answer
/// lands in the store. Returns everything the sink saw on the way.
async fn receipt_to_translation(
    h: &ColonyHandle,
    rx: &mut mpsc::Receiver<Message>,
    form: &str,
) -> Vec<Message> {
    h.send(text_to("/receipts", form)).await;
    let mut seen = recv_until(rx, "the translator's answer", is_translation).await;
    seen.extend(recv_until(rx, "the hand's bundle on it", |m| is_echo(m, "written")).await);
    seen
}

/// Extends `seen` until it holds `n` messages `want` takes.
async fn until(
    rx: &mut mpsc::Receiver<Message>,
    seen: &mut Vec<Message>,
    label: &str,
    n: usize,
    want: impl Fn(&Message) -> bool,
) {
    while seen.iter().filter(|m| want(m)).count() < n {
        let more = recv_until(rx, label, &want).await;
        seen.extend(more);
    }
}

/// A `show` into the hand -- one task, one mailbox: everything the hand got
/// before it is handled, and its writes are in the store's queue, once the
/// view is at the sink.
async fn settled(h: &ColonyHandle, rx: &mut mpsc::Receiver<Message>) -> Vec<Message> {
    h.send(text_to("/operator", r#"{"op":"show"}"#)).await;
    recv_until(rx, "the view", |m| hop_of(m, "route") == "answer").await
}

/// The boot receipt of a boot just made, settled: both announcements of it
/// have read the waiting questions, and the hand is done with both reads.
async fn boot_settled(h: &ColonyHandle, rx: &mut mpsc::Receiver<Message>) -> Vec<Message> {
    let mut seen = Vec::new();
    until(
        rx,
        &mut seen,
        "both reads of the waiting questions",
        2,
        |m| is_echo(m, "boot_waits"),
    )
    .await;
    seen.extend(settled(h, rx).await);
    seen
}

/// How many retries a boot started: the first read of a `retranslate` the
/// boot asked for. Two announcements of one boot start one.
fn retries_started(seen: &[Message]) -> usize {
    seen.iter()
        .filter(|m| hop_of(m, "route") == "rstore" && hop_of(m, "phase") == "world")
        .filter(|m| {
            let carry: Value =
                meclaw_core::serde_json::from_str(&hop_of(m, "carry")).unwrap_or(Value::Null);
            carry["cmd"]["op"] == "retranslate"
                && carry["cmd"]["boot"].is_string()
                && carry["cmd"].get("probe").is_none()
                && carry["cmd"]["page"].as_i64().unwrap_or(1) == 1
        })
        .count()
}

fn retried(seen: &[Message]) -> bool {
    seen.iter().any(|m| {
        hop_of(m, "route") == "translate"
            || (hop_of(m, "route") == "rstore" && hop_of(m, "carry").contains("retranslate"))
    })
}

/// Boot 1, the birth: one catalogue row, then the announcement from a
/// mutation's receipt -- the brain becomes a subscriber, its requirement is
/// asked once and the answer fails.
async fn birth(td: &tempfile::TempDir, registry: &std::path::Path, brain_url: &str) {
    build_tree(td, registry, brain_url);
    let (h, mut rx) = boot(td).await;
    admin(
        &h,
        &mut rx,
        json!({"operation": "insert", "table": "models",
        "row": {"model_id": FAST, "provider": "gateway", "base_url": "", "wire_dialect": "",
                "context_window": 32000, "cost_in": 1, "cost_out": 1, "caps": {"tools": true},
                "traits": {}, "strengths": "quick", "status": "active", "note": "TEST ROW",
                "package": {}, "prompt": ""}}),
    )
    .await;
    receipt_to_translation(&h, &mut rx, "manifest").await;
    assert_eq!(asked(td), 1);
    assert_eq!(
        rows(
            &h,
            &mut rx,
            "resolutions",
            json!(["reason"]),
            json!({"reason": "translation_failed"})
        )
        .await
        .len(),
        1
    );
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_translation_without_its_key_is_asked_again_at_the_next_boot() {
    let Some(registry) = registry_root() else {
        return;
    };
    let brain = MockOpenAI::start(
        (0..4)
            .map(|i| canned_chat_completion(&format!("answer {i}"), "stop"))
            .collect(),
    )
    .await;
    let td = tempfile::TempDir::new().unwrap();
    birth(&td, &registry, &format!("{}/v1", brain.base_url)).await;

    receipts_at_the_root(&td);

    // Boot 2, the vault still without the key: its receipt, announced twice,
    // asks once and fails again -- still waiting, one try more, marked a
    // second time, nothing pushed.
    let (h, mut rx) = boot(&td).await;
    let mut seen = boot_settled(&h, &mut rx).await;
    until(
        &mut rx,
        &mut seen,
        "the translator's answer",
        1,
        is_translation,
    )
    .await;
    seen.extend(settled(&h, &mut rx).await);
    assert_eq!(
        retries_started(&seen),
        1,
        "two announcements of one boot, one retry"
    );
    assert!(
        !seen.iter().any(|m| hop_of(m, "route") == "update"),
        "nothing is pushed"
    );
    assert_eq!(asked(&td), 2, "one question for this boot");
    let w = waits(&h, &mut rx).await;
    assert_eq!(w.len(), 1, "still waiting: {w:?}");
    assert_eq!(w[0]["tries"], 2, "{w:?}");
    assert!(
        !w[0]["asked_boot"].as_str().unwrap_or("").is_empty(),
        "{w:?}"
    );
    assert_eq!(
        marks(&h, &mut rx).await,
        2,
        "marked at birth and at this boot"
    );
    h.shutdown().await;

    // Boot 3, the key there: asked once, answered, stored -- the brain gets its
    // package and runs it, and nothing waits any more.
    std::fs::write(td.path().join("key"), "open").unwrap();
    let (h, mut rx) = boot(&td).await;
    let mut seen = boot_settled(&h, &mut rx).await;
    until(&mut rx, &mut seen, "the push", 1, |m| {
        hop_of(m, "route") == "update"
    })
    .await;
    seen.extend(settled(&h, &mut rx).await);
    assert_eq!(retries_started(&seen), 1, "one retry for this boot");
    let push = seen
        .iter()
        .find(|m| hop_of(m, "route") == "update")
        .expect("the push");
    assert_eq!(hop_of(push, "subscriber"), BRAIN);
    assert_eq!(
        body_of(push)["params"]["model"],
        FAST,
        "{:?}",
        body_of(push)
    );
    assert_eq!(asked(&td), 3);
    assert_eq!(waits(&h, &mut rx).await, Vec::<Value>::new());
    h.send(text_to(BRAIN, "ping")).await;
    recv_matching(&mut rx, "the brain's answer", |m| {
        !hop_of(m, "finish_reason").is_empty() && ctx_of(m, "registry_origin").is_empty()
    })
    .await;
    let snaps = brain.recorded_requests().await;
    assert_eq!(
        snaps.last().and_then(|s| s.model()),
        Some(FAST),
        "the brain runs the answer"
    );
    h.shutdown().await;

    // Boot 4: answered already, nothing waits -- no question.
    let (h, mut rx) = boot(&td).await;
    let seen = boot_settled(&h, &mut rx).await;
    assert_eq!(
        retries_started(&seen),
        0,
        "a boot after the answer asks nothing"
    );
    assert!(!retried(&seen), "a boot after the answer asks nothing");
    assert_eq!(asked(&td), 3);
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_translation_that_failed_otherwise_is_not_asked_at_boot() {
    let Some(registry) = registry_root() else {
        return;
    };
    let td = tempfile::TempDir::new().unwrap();
    std::fs::write(td.path().join("wire"), "500").unwrap();
    birth(&td, &registry, "http://127.0.0.1:9/v1").await;

    std::fs::remove_file(td.path().join("wire")).unwrap();
    std::fs::write(td.path().join("key"), "open").unwrap();
    receipts_at_the_root(&td);
    let (h, mut rx) = boot(&td).await;
    let seen = boot_settled(&h, &mut rx).await;
    assert_eq!(
        waits(&h, &mut rx).await,
        Vec::<Value>::new(),
        "nothing waits"
    );
    assert!(!retried(&seen), "a wire failure is not asked at boot");
    assert_eq!(asked(&td), 1);
    assert_eq!(
        rows(&h, &mut rx, "translations", json!(["model_id"]), json!({})).await,
        Vec::<Value>::new()
    );
    h.shutdown().await;
}

//! GH #973 M-B1, fix round 1 -- the night orders a verdict's chains and lets
//! the store resolve them.
//!
//! The first build resolved the chains of one verdict in the script:
//! `a -> b, b -> c` went out as `a -> c, b -> c`. With `b -> d` bound by a
//! member, the store kept `b -> d` (right) and wrote `a -> c`, so `a` and `b`
//! -- one identity to the judge -- ended on two (review I-1). And the script
//! compared spellings raw while the store keys a normalising binding on the
//! normal form, so `a -> "The Box", "the box" -> c` was no chain to the
//! script (review M-2). Ordered and compared in the store's key, the links
//! land targets first and every alias ends on what holds.
//!
//! A `set_alias` the store did not write is said, and the run receipt lists a
//! rewording as merged only when the store wrote it (review M-1).
//!
//! Measured at the receiver: the binding is made on the booted hive through
//! `in_alias`, the round's ops come out of the shipped `dream-glue` script and
//! land, in the order the script emits them, in the colony's own `cell.db`
//! through the store's own dispatcher (`support::store_op`).

#[path = "support/memory_hive_subject.rs"]
mod support;

use meclaw_core::serde_json::{Value, json};
use support::*;

const GLUE: &str = "../../templates/memory-hive/dream-glue/config.json";

/// Run the shipped `dream-glue` over `header` and `messages` -> (the store ops
/// it emits, its stderr).
fn glue(header: Value, messages: Value) -> (Vec<Value>, String) {
    let doc = json!({"header": header, "messages": messages});
    let out = meclaw_testing::run_shipped_script(
        &meclaw_testing::shipped_script(GLUE),
        &meclaw_testing::code_stdin(&doc).to_string(),
    );
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "dream-glue: {stderr}");
    let msgs: Vec<Value> =
        meclaw_core::serde_json::from_slice(&out.stdout).expect("a message array");
    let ops = msgs
        .iter()
        .filter_map(|m| m["messages"][0]["text"].as_str())
        .filter_map(|t| meclaw_core::serde_json::from_str::<Value>(t).ok())
        .collect();
    (ops, stderr)
}

/// The `set_alias` ops the night writes for the judge's answer `text`, in
/// the order they go out.
fn round_of(text: &str) -> Vec<Value> {
    let (ops, _) = glue(
        json!({"context": {"store_origin": "dream", "mem_phase": "canon-judged",
                           "dream_run": "r-973", "dream_to": "2026-10-03T03:00:00Z"},
               "hop": {"finish_reason": "stop"}}),
        json!([{"origin": "assistant", "type": "text", "text": text}]),
    );
    ops.into_iter()
        .filter(|a| a["operation"] == json!("set_alias"))
        .collect()
}

/// A program over the shipped script's functions, its stdout. The script
/// runs over a message of no phase of its own first (it parks), so every
/// function is defined.
fn glue_probe(program: &str) -> String {
    let script = meclaw_testing::shipped_script(GLUE);
    let src = format!(
        concat!(
            "import sys, io, json\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO(json.dumps({{\"envelope\": {{\"header\": {{\"context\": ",
            "{{\"mem_phase\": \"probe\", \"dream_run\": \"r\", \"dream_to\": \"t\"}}}}}}, ",
            "\"body\": {{}}, \"params\": {{}}}}))\n",
            "_sink, _real = io.StringIO(), sys.stdout\n",
            "sys.stdout = _sink\n",
            "try:\n",
            "    exec(compile(_script, 'cell', 'exec'), globals())\n",
            "except SystemExit:\n",
            "    pass\n",
            "finally:\n",
            "    sys.stdout = _real\n",
            "{}\n"
        ),
        meclaw_core::serde_json::to_string(&script).expect("script"),
        program
    );
    let mut child = std::process::Command::new("python3")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("python3");
    {
        use std::io::Write;
        let mut sink = child.stdin.take().expect("stdin");
        sink.write_all(src.as_bytes()).expect("write");
    }
    let out = child.wait_with_output().expect("python3 ran");
    assert!(
        out.status.success(),
        "probe: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A booted hive whose member bound `held` to `ob-1` on `in_alias`; the
/// colony is stopped again, the night's ops land through the dispatcher.
async fn member_bound(td: &tempfile::TempDir, held: &[&str]) -> std::path::PathBuf {
    build(
        td,
        &[
            fact(
                "f-1",
                "Firewall X",
                "firewall x",
                ROUND_EA,
                "2026-09-01T10:00:00Z",
            ),
            fact(
                "f-2",
                "the box",
                "the box",
                ROUND_EB,
                "2026-09-02T10:00:00Z",
            ),
        ],
    );
    let (h, mut rx) = boot(td).await;
    let mut seen = Vec::new();
    for (i, alias) in held.iter().enumerate() {
        let tag = format!("t-{i}");
        h.send(in_alias(
            json!({"aliases": [{"alias": alias, "canonical": "ob-1"}]}),
            &tag,
        ))
        .await;
        let a = ack(&mut rx, &mut seen, &tag).await;
        assert_eq!(body_of(&a)["done"], json!(1));
    }
    h.shutdown().await;
    db(td)
}

fn subject_aliases(db: &std::path::Path) -> Vec<Vec<String>> {
    rows(
        db,
        "SELECT alias, canonical FROM subject_aliases ORDER BY alias",
    )
}

/// Review I-1: the verdict says `Firewall X -> the box -> crate`, a member
/// holds `the box -> ob-1`. The member's binding stays, and `Firewall X` --
/// the box to the judge -- ends where the box ends.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_verdict_chain_ends_on_the_binding_that_holds() {
    if !shipped() {
        eprintln!("gh973: the memory hive is not in this tree, skipped (GH #49)");
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    let db = member_bound(&td, &["the box"]).await;
    let ops = round_of(
        r#"{"entities": [{"alias": "Firewall X", "canonical": "the box"},
                         {"alias": "the box", "canonical": "crate"}]}"#,
    );
    let order: Vec<(Value, Value)> = ops
        .iter()
        .map(|o| (o["alias"].clone(), o["canonical"].clone()))
        .collect();
    assert_eq!(
        order,
        vec![
            (json!("the box"), json!("crate")),
            (json!("Firewall X"), json!("the box")),
        ],
        "the target's own link goes out first, the targets as judged"
    );
    let answers: Vec<Value> = ops
        .into_iter()
        .map(|op| {
            let out = store_op(&db, op);
            assert_eq!(out.error_code, None);
            out.payload
        })
        .collect();
    assert_eq!(
        answers[0],
        json!({"alias": "the box", "column": "subject", "canonical": "ob-1",
               "conflict": true}),
        "the member's binding is not bent, and the answer says which alias"
    );
    assert_eq!(
        subject_aliases(&db),
        vec![
            vec!["firewall x".to_string(), "ob-1".to_string()],
            vec!["the box".to_string(), "ob-1".to_string()],
        ],
        "one identity to the judge, one identity in the table"
    );
}

/// Review M-2: the chain is seen in the store's key. `Firewall X -> The Box`
/// and `the box -> crate` are one chain under the normalising subject
/// binding; both spellings end on `crate`. A cycle in normal form writes
/// nothing, and a link onto its own normal form says nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_verdict_chain_is_read_in_the_stores_normal_form() {
    if !shipped() {
        eprintln!("gh973: the memory hive is not in this tree, skipped (GH #49)");
        return;
    }
    assert_eq!(
        round_of(
            r#"{"entities": [{"alias": "Firewall X", "canonical": "the box"},
                             {"alias": "The  Box", "canonical": "firewall x"},
                             {"alias": "CRATE", "canonical": "crate"}]}"#,
        ),
        Vec::<Value>::new(),
        "a cycle in normal form, and a spelling onto itself"
    );
    let td = tempfile::TempDir::new().expect("tempdir");
    // An unrelated binding wakes the store (its alias table is made at spawn).
    let db = member_bound(&td, &["Mill"]).await;
    for op in round_of(
        r#"{"entities": [{"alias": "Firewall X", "canonical": "The Box"},
                         {"alias": "the box", "canonical": "crate"}]}"#,
    ) {
        let out = store_op(&db, op);
        assert_eq!(out.error_code, None);
        assert_eq!(out.rows_affected, 1, "{:?}", out.payload);
    }
    assert_eq!(
        subject_aliases(&db),
        vec![
            vec!["firewall x".to_string(), "crate".to_string()],
            vec!["mill".to_string(), "ob-1".to_string()],
            vec!["the box".to_string(), "crate".to_string()],
        ]
    );
}

/// The script keys a spelling exactly as the store does: its copy of the
/// normal form is the writer's, byte for byte (gh948 holds the writer to the
/// store), and its list of normalising dimensions is the store's.
#[test]
fn the_glues_normal_form_is_the_stores() {
    if !shipped() {
        return;
    }
    let g = meclaw_testing::shipped_script(GLUE);
    let w = meclaw_testing::shipped_script(WRITER);
    for head in ["COMPOSE_ROWS = (", "WHITE_SPACE = ", "def normalize_text("] {
        assert_eq!(block_of(&g, head), block_of(&w, head), "`{head}` drifted");
    }
    let probe = [
        "Firewall X",
        "  THE\tbox ",
        "Cafe\u{0301}",
        "\u{0130}stanbul",
    ];
    let got: Vec<String> = meclaw_core::serde_json::from_str(&glue_probe(&format!(
        "print(json.dumps([normalize_text(s) for s in {}]))",
        meclaw_core::serde_json::to_string(&probe).expect("probe")
    )))
    .expect("normal forms");
    let want: Vec<String> = probe
        .iter()
        .map(|s| meclaw_cells::store::query::normalize::normalize(s))
        .collect();
    assert_eq!(got, want);
    let store = read_json(&repo("templates/memory-hive/store/config.json"));
    let mut normalising: Vec<String> = store["params"]["canonical"]["facts"]
        .as_array()
        .expect("the bindings")
        .iter()
        .filter(|b| b["normalize"] == json!(true))
        .map(|b| b["source"].as_str().expect("source").to_string())
        .collect();
    normalising.sort();
    let glue: Vec<String> =
        meclaw_core::serde_json::from_str(&glue_probe("print(json.dumps(sorted(NORMALISING)))"))
            .expect("NORMALISING");
    assert_eq!(glue, normalising);
}

/// Review M-1: a `set_alias` the store did not write is said on stderr, and
/// on the claim dimension every answer is parked for the run receipt.
#[test]
fn a_refused_alias_is_said_and_parked_for_the_receipt() {
    if !shipped() {
        return;
    }
    let answer = |payload: Value| {
        glue(
            json!({"context": {"mem_phase": "canon-alias", "dream_run": "r-973",
                               "dream_to": "2026-10-03T03:00:00Z"},
                   "hop": {"operation": "set_alias", "rows_affected": 0}}),
            json!([{"origin": "tool", "type": "tool_result", "text": payload.to_string()}]),
        )
    };
    let (ops, stderr) = answer(json!({"alias": "the box", "column": "subject",
                                      "canonical": "ob-1", "conflict": true}));
    assert_eq!(ops, Vec::<Value>::new(), "a subject answer parks nothing");
    assert!(
        stderr.contains("subject alias 'the box' not written -- bound to 'ob-1'"),
        "{stderr}"
    );
    let refusal = json!({"alias": "x", "column": "claim", "conflict": true,
                         "refused": "alias_cycle"});
    let (ops, stderr) = answer(refusal.clone());
    assert!(
        stderr.contains("claim alias 'x' not written -- refused: alias_cycle"),
        "{stderr}"
    );
    assert_eq!(ops.len(), 1, "{ops:?}");
    assert_eq!(
        (
            ops[0]["operation"].clone(),
            ops[0]["row"]["kind"].clone(),
            ops[0]["row"]["key"].clone()
        ),
        (json!("insert"), json!("canon-claim-answer"), json!("r-973"))
    );
    let parked: Value =
        meclaw_core::serde_json::from_str(ops[0]["row"]["payload"].as_str().expect("payload"))
            .expect("json");
    assert_eq!(parked, refusal);
    let (ops, stderr) = answer(json!({"alias": "y", "column": "claim",
                                      "canonical": "z", "conflict": false}));
    assert_eq!(ops.len(), 1, "a written claim is parked too");
    assert!(!stderr.contains("not written"), "{stderr}");
}

/// Review M-1: the receipt lists a rewording as merged only when the store
/// wrote it, with the canonical that holds; a refused or bound one is listed
/// apart, and one without an answer (an older store) stays as judged.
#[test]
fn the_receipt_lists_only_the_claims_the_store_wrote() {
    if !shipped() {
        return;
    }
    let got: Value = meclaw_core::serde_json::from_str(&glue_probe(
        r#"print(json.dumps(claims_landed(
    [{"alias": "x", "canonical": "y", "reason": "r"},
     {"alias": "p", "canonical": "q", "reason": "r"},
     {"alias": "m", "canonical": "n", "reason": "r"}],
    [json.dumps({"alias": "x", "column": "claim", "conflict": True,
                 "refused": "alias_cycle"}),
     json.dumps({"alias": "p", "column": "claim", "canonical": "q2",
                 "conflict": False})])))"#,
    ))
    .expect("json");
    assert_eq!(
        got,
        json!([
            [{"alias": "p", "canonical": "q2", "reason": "r"},
             {"alias": "m", "canonical": "n", "reason": "r"}],
            [{"alias": "x", "canonical": "y", "reason": "r", "held": "",
              "refused": "alias_cycle"}]
        ])
    );
}

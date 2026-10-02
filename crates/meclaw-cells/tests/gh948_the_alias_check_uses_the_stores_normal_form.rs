//! GH #948 -- the alias check uses the store's normal form.
//!
//! The store keys its alias table on `normalize` (case fold, Latin-1
//! composition, whitespace collapse -- `store/query/normalize.rs`), and the
//! writer checks a spelling against that table before it binds it. A script
//! cannot import, so the writer carries a copy; a copy that keys one spelling
//! differently from the store would wave through exactly the rebinding the
//! check exists to refuse. This file holds the copy against the store's own
//! function over one table, against recall's copy byte for byte, and at the
//! receiver: spellings the store treats as one are refused as one.

#[path = "support/memory_hive_subject.rs"]
mod support;

use meclaw_core::serde_json::{Value, json};
use support::*;

/// Spellings that probe every effect and its edges: case, runs of whitespace
/// (the store's whitespace, not Python's -- U+001C..U+001F are NOT whitespace
/// to the store), NFD marks inside and outside the table, the sharp s that a
/// case FOLD would turn into "ss" and a LOWERCASE does not, final sigma, a
/// dotted capital I, and letters outside Latin-1.
const TABLE: [&str; 24] = [
    "Firewall X",
    "FIREWALL  X",
    "  firewall\tx\n",
    "Cafe\u{0301}",
    "CAF\u{00c9}",
    "Stra\u{00df}e",
    "STRASSE",
    "\u{1e9e}",
    "\u{0130}stanbul",
    "\u{039f}\u{0394}\u{039f}\u{03a3}",
    "\u{039f}\u{0394}\u{039f}\u{03a3} \u{0391}",
    "x\u{001c}y",
    "x\u{001f} y",
    "a\u{00a0}b",
    "a\u{3000}b\u{2028}c",
    "a\u{200b}b",
    "e\u{0304}x",
    "\u{0301}x",
    "A\u{030a}NGSTRO\u{0308}M",
    "\u{01c4}",
    "\u{2160}\u{2161}",
    "\u{ff21}\u{ff22}",
    "",
    "   ",
];

fn twin(config: &str) -> Vec<String> {
    let program = format!(
        "print(json.dumps([normalize_text(s) for s in {}]))",
        meclaw_core::serde_json::to_string(&TABLE).expect("table")
    );
    meclaw_core::serde_json::from_str(&probe(config, &program)).expect("twin output")
}

#[test]
fn gh948_the_writers_copy_is_the_stores_normal_form() {
    if !shipped() {
        return;
    }
    let want: Vec<String> = TABLE
        .iter()
        .map(|s| meclaw_cells::store::query::normalize::normalize(s))
        .collect();
    assert_eq!(twin(WRITER), want, "writer");
    // Recall's copy keys the subject question; it is the same function.
    assert_eq!(twin(RECALL), want, "recall");
    // And the two copies are one text, so a fix lands in both or in neither.
    let w = meclaw_testing::shipped_script(WRITER);
    let r = meclaw_testing::shipped_script(RECALL);
    for head in ["COMPOSE_ROWS = (", "WHITE_SPACE = ", "def normalize_text("] {
        assert_eq!(block_of(&w, head), block_of(&r, head), "`{head}` drifted");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh948_the_alias_check_uses_the_stores_normal_form() {
    if !shipped() {
        eprintln!("gh948: the memory hive is not in this tree, skipped (GH #49)");
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    build(
        &td,
        &[fact(
            "f-1",
            "Firewall X",
            "firewall x",
            ROUND_EA,
            "2026-09-01T10:00:00Z",
        )],
    );
    let (h, mut rx) = boot(&td).await;
    let mut seen = Vec::new();

    h.send(in_alias(
        json!({"aliases": [{"alias": "Firewall X", "canonical": "ob-1"},
                           {"alias": "Caf\u{00e9}", "canonical": "ob-2"},
                           {"alias": "Stra\u{00df}e", "canonical": "ob-3"}]}),
        "t-bind",
    ))
    .await;
    let a = ack(&mut rx, &mut seen, "t-bind").await;
    assert_eq!(body_of(&a)["done"], json!(3));

    // Three spellings the store treats as the bound ones, one it does not.
    h.send(in_alias(
        json!({"aliases": [{"alias": "FIREWALL  X", "canonical": "ob-9"},
                           {"alias": "  firewall\tx ", "canonical": "ob-9"},
                           {"alias": "Cafe\u{0301}", "canonical": "ob-9"},
                           {"alias": "STRASSE", "canonical": "ob-9"}]}),
        "t-probe",
    ))
    .await;
    let a = ack(&mut rx, &mut seen, "t-probe").await;
    let refused: Vec<Value> = body_of(&a)["refused"].as_array().expect("refused").clone();
    assert_eq!(
        refused,
        vec![
            json!({"alias": "FIREWALL  X", "error_code": "alias_taken"}),
            json!({"alias": "  firewall\tx ", "error_code": "alias_taken"}),
            json!({"alias": "Cafe\u{0301}", "error_code": "alias_taken"}),
        ]
    );
    assert_eq!(body_of(&a)["done"], json!(1), "the sharp s is not \"ss\"");
    let db = db(&td);
    assert_eq!(
        rows(
            &db,
            "SELECT canonical FROM subject_aliases WHERE alias = 'firewall x'"
        ),
        vec![vec!["ob-1".to_string()]]
    );
    assert_eq!(
        rows(
            &db,
            "SELECT canonical FROM subject_aliases WHERE alias = 'strasse'"
        ),
        vec![vec!["ob-9".to_string()]]
    );
    h.shutdown().await;
}

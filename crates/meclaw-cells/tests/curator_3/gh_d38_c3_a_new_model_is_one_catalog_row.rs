//! GH #1450 (D38 C3b, R-38-4 Nr. 3, R-IG-1) -- a new model is one catalogue row.
//!
//! WHY this file exists: what the cut knows about a model -- its budget,
//! its hard line, how its provider caches -- stands in the catalogue
//! (llm-registry), never in the curator's code (R-38-4 Nr. 3, R-IG-1: numbers
//! only out of the catalogue). So a model nobody shipped, named only by a
//! row on the `in_catalog` lane (GH #1449), is cut for from its first call
//! without one changed line: the call's snapshot names the row's model and
//! its numbers.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

fn cut_of(h: &Hive, call: &Msg) -> Option<Value> {
    let has = h.rows("SELECT name FROM pragma_table_info('snapshots') WHERE name = 'cut'");
    if has.is_empty() {
        return None;
    }
    let id = call.hop["curator_call"].as_str().unwrap_or_default();
    let rows = h.rows(&format!("SELECT cut FROM snapshots WHERE call_id = '{id}'"));
    let text = rows.first()?.first()?.as_str()?.to_string();
    sj::from_str::<Value>(&text).ok()
}

fn catalog(h: &mut Hive, cell: &str, row: Value) {
    h.out.clear();
    h.lane(
        "in_catalog",
        json!({}),
        json!({"package_of": format!("/talky/{cell}")}),
        json!({"messages": [], "package": row}),
    );
}

/// A row no template, satz or test fixture ships: invented numbers, an
/// invented id, a cache mode of its own.
fn invented() -> Value {
    json!({"model": "test/never-shipped-7", "input_soft": 77777, "input_hard": 88888,
           "max_output": 1234, "price_tiers": null, "cache_mode": "implicit",
           "cache_ttl_s": 123, "wire_dialect": "chat_completions",
           "supported_params": ["temperature"], "chars_per_token": 2.5})
}

#[test]
fn an_invented_row_sizes_the_first_call() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    catalog(&mut h, "brain", invented());
    let call = h.curate("s1", "t1", 0, json!([user("hello")]), mode("Be brief."));
    let cut = cut_of(&h, &call)
        .expect("the snapshot names the cut of the first call (column `cut` of `snapshots`)");
    assert_eq!(cut["model"], json!("test/never-shipped-7"), "{cut}");
    assert_eq!(cut["budget"]["soft"], json!(77777), "{cut}");
    let hard = cut["budget"]["hard"].as_i64().unwrap_or(0);
    assert!(hard > 0 && hard <= 88888, "the row's hard line: {cut}");
    assert_eq!(cut["cache_mode"], json!("implicit"), "{cut}");
    assert_eq!(cut["cache_ttl_s"], json!(123), "{cut}");
}

#[test]
fn the_policy_code_names_no_model() {
    if !shipped() {
        return;
    }
    // Comments may tell a measured story about a model; code may not branch
    // on one. A model family in a code line is model knowledge outside the
    // catalogue.
    let script = script_of("policy");
    let families = [
        "haiku", "sonnet", "opus", "claude-", "mimo", "qwen", "gpt-", "luna", "gemini",
    ];
    for (n, line) in script.lines().enumerate() {
        let code = line.split('#').next().unwrap_or("").to_lowercase();
        for f in families {
            assert!(
                !code.contains(f),
                "policy line {}: `{f}` in code -- a model belongs in its catalogue row: {line}",
                n + 1
            );
        }
    }
}

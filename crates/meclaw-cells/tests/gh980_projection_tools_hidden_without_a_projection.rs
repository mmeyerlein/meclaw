//! GH #980 -- the projection tools are on a menu only where the owner gave
//! the space a projection.
//!
//! `file_ws_exec`, `file_ws_export` and `file_ws_push` refuse every call of a
//! space whose projection has no `base_path` (`no_base_path`). A model that
//! sees them there sees three tools that always refuse, so the shipped
//! `./schemas` leaves them off unless its knob `projection_tools` is "1" --
//! the line the owner writes beside the projection's `base_path`. The shipped
//! default is "0".
//!
//! Measured at the menu's answer: the shipped `templates/file-space/schemas`
//! program runs under python3 on a real stdin document (the
//! `support/file_space_hive.rs` runner), once per knob value, asked for `*`
//! and for the three names one by one. Guarded like every template-reading
//! test (GH #49).

#[path = "support/file_space_hive.rs"]
mod file_space_hive;

use file_space_hive::*;
use meclaw_core::serde_json::{Value, json};

const PROJECTION: [&str; 3] = ["file_ws_exec", "file_ws_export", "file_ws_push"];

/// The menu `./schemas` answers for `tools` under `params`.
fn menu(params: Value, tools: Value) -> Value {
    let doc = json!({
        "envelope": {"header": {"hop": {"route": "in_schemas"}, "context": {}}},
        "body": {"tools": tools},
        "params": params,
    });
    let out = run_python(&script_of("schemas"), &doc.to_string());
    assert!(
        out.status.success(),
        "./schemas did not answer: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    meclaw_core::serde_json::from_slice(&out.stdout).expect("the menu is JSON")
}

fn names(answer: &Value) -> Vec<String> {
    answer["schemas"]
        .as_array()
        .expect("schemas[]")
        .iter()
        .map(|s| s["name"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[test]
fn projection_tools_hidden_without_a_projection() {
    if !shipped() {
        eprintln!("the file space did not travel into this tree -- skipped (GH #49)");
        return;
    }
    // The shipped knob is off.
    assert_eq!(
        cell_config("schemas")["params"]["projection_tools"],
        json!("0"),
        "the shipped `./schemas` keeps the projection tools off"
    );

    // Off (shipped, absent, "0"): `*` names 37 tools and none of the three; a
    // caller that names one of them hears it is unknown.
    for params in [json!({"projection_tools": "0"}), json!({})] {
        let all = menu(params.clone(), json!(["*"]));
        let got = names(&all);
        assert_eq!(
            got.len(),
            37,
            "{params}: 37 tools without a projection: {got:?}"
        );
        for p in PROJECTION {
            assert!(
                !got.iter().any(|n| n == p),
                "{params}: `{p}` on the menu: {got:?}"
            );
        }
        let asked = menu(params.clone(), json!(PROJECTION));
        assert!(names(&asked).is_empty(), "{params}: {asked}");
        assert_eq!(asked["unknown"], json!(PROJECTION), "{params}: {asked}");
        assert_eq!(
            asked["header"]["error_code"],
            json!("tool_unknown"),
            "{asked}"
        );
    }

    // On: all 40, the three at the end of the menu, each declared in full.
    let all = menu(json!({"projection_tools": "1"}), json!(["*"]));
    let got = names(&all);
    eprintln!("gh980 menu: off=37 on={}", got.len());
    assert_eq!(got.len(), 40, "40 tools with a projection: {got:?}");
    assert_eq!(
        &got[37..],
        &PROJECTION,
        "the three projection tools close the menu"
    );
    let asked = menu(json!({"projection_tools": "1"}), json!(PROJECTION));
    assert_eq!(names(&asked), PROJECTION, "{asked}");
    assert_eq!(asked["unknown"], json!([]), "{asked}");
    for s in asked["schemas"].as_array().unwrap() {
        assert_eq!(
            s["parameters"]["additionalProperties"],
            json!(false),
            "{}: a closed schema",
            s["name"]
        );
    }
}

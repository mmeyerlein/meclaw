//! GH #661 — the design lane asks for an operator-set param before it submits.
//!
//! The door refuses a node grown without such a param
//! (`crates/meclaw-colony/tests/gh661_an_operator_set_param_is_not_a_default.rs`).
//! That is the enforcement, and it is enough to keep a colony correct — but it
//! costs a round: the wish is drafted, submitted, refused, and the refusal has
//! to be read back into a repair. The lane can spend that round earlier by
//! reading the declaration itself.
//!
//! Two surfaces, and neither works alone. The BRIEFING says which rule applies:
//! a param marked operator-set has no usable default, the door refuses a node
//! without it, and a wish that does not name the value goes back as
//! `wish_incomplete` rather than being submitted. The CATALOGUE says which
//! params it applies to — it is the only surface on which the lane learns that,
//! because `contract.settings` of a template it has not instantiated is nowhere
//! else in its reach.
//!
//! This is a drift lock in the sense of the development rules § 2d: it greps the
//! SENTENCE out of the shipped briefing and asserts the MECHANISM behind it, so
//! prose that outlived its marking (or a marking nobody explained) is red.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_all, shipped_script};

const BRIEF: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/brief/config.json"
);

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The instructions the brief hands the composer.
fn briefing() -> String {
    let all = emit_all(
        &shipped_script(BRIEF),
        &json!({
            "target": "/os/builder/brief",
            "header": {"hop": {"route": "brief", "stage": "briefed", "hits": 3},
                       "context": {}},
            "ttl": 64,
            "messages": [
                {"origin": "user", "type": "text", "id": "",
                 "text": "a telephone channel for alex"},
                {"origin": "tool", "type": "tool_result", "id": "",
                 "text": "### config.md -- contract.settings (spec) [d-1]\na setting …"}
            ],
        }),
    );
    let leg: Value = all
        .into_iter()
        .find(|m| m["header"]["route"] == "compose")
        .expect("the brief's leg to the composer");
    leg["system"]["instructions"]["text"]
        .as_str()
        .expect("system.instructions.text")
        .to_string()
}

/// One catalogue `PARAMS —` cell entry for a template written on the spot,
/// rendered by the generator itself rather than by a second copy of its rules.
fn param_entry(dir: &std::path::Path) -> Option<String> {
    let script = format!(
        "import sys, json\n\
         sys.path.insert(0, {tools:?})\n\
         import build_librarian_seed as b\n\
         print(json.dumps([e for _r, _t, e in b._param_cells({dir:?})]))\n",
        tools = repo("workshop/tools").to_str()?,
        dir = dir.to_str()?,
    );
    let out = std::process::Command::new("python3")
        .arg("-c")
        .arg(script)
        .output()
        .ok()?;
    if !out.status.success() {
        panic!(
            "the catalogue generator refused the fixture: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let listed: Vec<String> =
        meclaw_core::serde_json::from_slice(&out.stdout).expect("the generator answers JSON");
    listed.into_iter().next()
}

fn write_template(dir: &std::path::Path, config: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("template.json"), r#"{"name":"fixture"}"#).unwrap();
    std::fs::write(dir.join("config.json"), config).unwrap();
}

#[test]
fn the_briefing_and_the_catalogue_agree_with_the_contract() {
    // ── The sentence. ───────────────────────────────────────────────────────
    let text = briefing();
    assert!(
        text.contains("[operator-set]"),
        "the briefing must name the MARKING the catalogue carries, or the lane \
         knows a rule and cannot tell which params it applies to"
    );
    assert!(
        text.contains("operator_param_unset"),
        "and the code the door answers with, because a refusal the model cannot \
         name is one it cannot repair"
    );
    assert!(
        text.contains("wish_incomplete"),
        "and what to do instead of submitting: ask, in the form the fast lane \
         already speaks for a mount it cannot render"
    );

    // ── The mechanism. ──────────────────────────────────────────────────────
    // The generator lives under workshop/, which never ships. In a public
    // clone the sentence half above is the whole test, and the catalogue half
    // is proven where the file is (the gate host). A presence guard, the same
    // form as gh308 and gh325: the file decides, never a flag — and the skip
    // says so (final review M-5).
    let generator = repo("workshop/tools/build_librarian_seed.py");
    if !generator.exists() {
        eprintln!(
            "SKIPPED: {} is not in this tree — the catalogue half runs in the private tree",
            generator.display()
        );
        return;
    }
    let td = tempfile::TempDir::new().unwrap();
    let marked = td.path().join("marked");
    write_template(
        &marked,
        r#"{"cell":{"type":"code"},"params":{"ws_url":"ws://127.0.0.1:7777/x","retries":3},
            "contract":{"version":"0.1.0","consumes":{},"settings":{
                "ws_url":{"type":"string","operator_set":true,"default":"ws://127.0.0.1:7777/x"},
                "retries":{"type":"number","default":3}}}}"#,
    );
    let Some(line) = param_entry(&marked) else {
        // No python3 here — the gate host and CI have one, so the generator is
        // run there. A skip that says nothing is a green test that proved
        // nothing (final review M-5).
        eprintln!("SKIPPED: no python3 — the catalogue generator was not run");
        return;
    };
    assert!(
        line.contains("ws_url [operator-set]"),
        "a declared operator-set param is marked in the catalogue line: {line}"
    );
    assert!(
        !line.contains("retries [operator-set]"),
        "and an ordinary default is not — a marking on everything marks nothing: {line}"
    );

    let plain = td.path().join("plain");
    write_template(
        &plain,
        r#"{"cell":{"type":"code"},"params":{"ws_url":""},
            "contract":{"version":"0.1.0","consumes":{},"settings":{
                "ws_url":{"type":"string","default":""}}}}"#,
    );
    let control = param_entry(&plain).expect("the generator answers");
    assert!(
        !control.contains("[operator-set]"),
        "the same param name and the same empty default, undeclared, carries no \
         marking: the catalogue publishes the DECLARATION and never a guess: {control}"
    );
}

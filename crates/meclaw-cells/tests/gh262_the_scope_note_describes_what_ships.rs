//! GH #262 — the field a reader consults to learn where a template's authority
//! STOPS must describe the template that ships.
//!
//! `templates/affinity/template.json` promised a human gate on the proposal
//! lane: *"a memory-born proposal lands in `proposals` with status `open` and
//! waits for a human `decide_proposal`"*. R-AF-1 decided the opposite, with the
//! reasoning written out, and `gate` implements R-AF-1: a proposal is accepted
//! as it arrives unless the caller deliberately passes `auto_accept: false`.
//! The behaviour is not what drifted — the description is.
//!
//! # Why this can be pinned at all
//!
//! `not_in_scope` is prose, and prose in general cannot be held against code.
//! One thing about it can: the two STATUS values the lane writes are literals,
//! this file reads them out of the real `gate` script by running it, and a text
//! that describes the lane has to use them the way the script does —
//!
//! 1. the status a plain `propose` writes is named where the scope of the
//!    template is described, and
//! 2. the other status is never named in a sentence that does not also name the
//!    knob that produces it, so the exception can never be written down as the
//!    rule.
//!
//! That is a narrow rule and it is honest about being narrow: an author who
//! invents a wording outside the marker list below escapes it. It catches
//! exactly the drift class of #262 — a sentence that says the system waits for
//! a person when it does not — across every `not_in_scope` this template ships
//! plus its README, which is where the same sentence had settled in three
//! places at once.

use std::io::Write;
use std::process::{Command, Stdio};

const TEMPLATE_ROOT: &str = "../../templates/affinity";

/// Whether the private template travels with this checkout (GH #49 form):
/// `affinity` is not in `PUBLIC_TEMPLATES`, so in a public clone this file
/// skips instead of failing on a dead `templates/` reference.
fn shipped() -> Option<std::path::PathBuf> {
    let root = std::path::PathBuf::from(TEMPLATE_ROOT);
    root.join("template.json").exists().then_some(root)
}

/// `${VAR:-default}` becomes the default, a bare `${VAR}` the empty string —
/// the substitution the colony performs at instantiation.
fn resolve_vars(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    let mut rest = script;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail
            .find('}')
            .expect("unterminated ${...} in script_inline");
        if let Some((_, default)) = tail[..end].split_once(":-") {
            out.push_str(default);
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

fn read_json(p: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{p:?}: {e}")))
        .unwrap_or_else(|e| panic!("{p:?}: {e}"))
}

/// The status one `propose` call actually writes, taken from the real `gate`
/// script rather than from a constant in this file.
fn status_of_a_proposal(root: &std::path::Path, extra: serde_json::Value) -> String {
    let config = read_json(&root.join("gate/config.json"));
    let script = resolve_vars(config["params"]["script_inline"].as_str().expect("script"));

    let mut args = serde_json::json!({
        "op": "propose",
        "source_ref": "episode:262",
        "entity_ref": "entity:alex",
        "field_path": "interests",
        "value": {"topic": "sailing"}
    });
    if let Some(extra) = extra.as_object() {
        for (k, v) in extra {
            args[k] = v.clone();
        }
    }
    let doc = serde_json::json!({
        "messages": [{"origin": "assistant", "type": "tool_call", "id": "c262",
                      "text": args.to_string()}],
        "header": {"hop": {}, "context": {"actor": "agent:scribe"}}
    });

    let mut child = Command::new("python3")
        .arg("-c")
        .arg(&script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(&meclaw_testing::code_stdin_bytes(&doc))
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "gate exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let emitted: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("not JSON ({e}): {}", String::from_utf8_lossy(&out.stdout)));

    for m in &emitted {
        let Some(text) = m["messages"][0]["text"].as_str() else {
            continue;
        };
        let Ok(op) = serde_json::from_str::<serde_json::Value>(text) else {
            continue;
        };
        if op["operation"] == "insert" && op["table"] == "proposals" {
            return op["row"]["status"]
                .as_str()
                .expect("a proposal row carries a status")
                .to_string();
        }
    }
    panic!("the gate wrote no proposal row: {emitted:?}");
}

/// Every piece of shipped prose that describes this template to a reader.
fn shipped_prose(root: &std::path::Path) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let manifest = root.join("template.json");
    out.push((
        "template.json description.not_in_scope".to_string(),
        read_json(&manifest)["description"]["not_in_scope"]
            .as_str()
            .expect("a template names what it does not do")
            .to_string(),
    ));
    let mut configs: Vec<std::path::PathBuf> = Vec::new();
    collect_configs(root, &mut configs);
    configs.sort();
    for p in configs {
        let v = read_json(&p);
        if let Some(text) = v["description"]["not_in_scope"].as_str() {
            out.push((
                format!("{} description.not_in_scope", p.display()),
                text.to_string(),
            ));
        }
    }
    out.push((
        "README.md".to_string(),
        std::fs::read_to_string(root.join("README.md")).expect("the template has a README"),
    ));
    out
}

fn collect_configs(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let p = entry.path();
        if p.is_dir() {
            collect_configs(&p, out);
        } else if entry.file_name() == "config.json" {
            out.push(p);
        }
    }
}

/// Prose as sentences, with every hard wrap and table cell flattened first —
/// a marker phrase must not escape by falling across a line break.
fn sentences(text: &str) -> Vec<String> {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    flat.split(". ").map(|s| s.to_string()).collect()
}

// ═══════════════════════════════════════════════════════════════════════ pins

/// GH #262, claim 1. The status a plain `propose` writes is the status the
/// template's own scope note names.
#[test]
fn the_scope_note_names_the_status_a_proposal_actually_gets() {
    let Some(root) = shipped() else {
        return;
    };
    let default_status = status_of_a_proposal(&root, serde_json::json!({}));
    let marker = format!("`{default_status}`");

    for file in ["template.json", "gate/config.json"] {
        let text = read_json(&root.join(file))["description"]["not_in_scope"]
            .as_str()
            .expect("not_in_scope")
            .to_string();
        assert!(
            text.contains(&marker),
            "{file} describes the proposal lane but never names {marker}, which is \
             the status `gate` writes for a proposal nobody asked to hold back \
             (R-AF-1): {text}"
        );
    }
}

/// GH #262, claim 2. The status only `auto_accept: false` produces is never
/// described without it — the exception may not be written down as the rule.
///
/// This is the sentence that shipped and was false: *"a memory-born proposal
/// lands in `proposals` with status `open` and waits for a human
/// `decide_proposal`"*.
#[test]
fn no_shipped_sentence_describes_the_exception_as_the_default() {
    let Some(root) = shipped() else {
        return;
    };
    let gated_status = status_of_a_proposal(&root, serde_json::json!({"auto_accept": false}));
    let default_status = status_of_a_proposal(&root, serde_json::json!({}));
    assert_ne!(
        gated_status, default_status,
        "this pin only means something while the two lanes differ"
    );

    // The wordings that claim a proposal is held for somebody. The backticked
    // status is derived; the two phrases are the ones that actually drifted.
    let markers = [
        format!("`{gated_status}`"),
        "waits for a human".to_string(),
        "a proposal waits".to_string(),
    ];

    for (label, text) in shipped_prose(&root) {
        for sentence in sentences(&text) {
            let Some(hit) = markers.iter().find(|m| sentence.contains(m.as_str())) else {
                continue;
            };
            assert!(
                sentence.contains("auto_accept"),
                "{label}: '{hit}' describes the lane as waiting, but the sentence \
                 does not name the knob that makes it wait, so it reads as the \
                 default — and the default is `{default_status}` (R-AF-1): {sentence}"
            );
        }
    }
}

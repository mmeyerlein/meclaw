//! R3 ruling -- a rendered fact candidate carries its AXIS, not just its object.
//!
//! The bundle's JSON has always held `subject` and `predicate` on every fact
//! candidate, but the model reads the TEXT block next to it, and that block
//! rendered the claim alone: `- [fact keyword] Athen`. Without the axis the
//! consumer cannot tell WHICH question that value answers -- a favourite city,
//! a birthplace and a next holiday destination all render identically, and the
//! only thing that told them apart lived in the JSON nobody reads.
//!
//! Same discipline as every other annotation of `render_candidate_line` (O-5):
//! one place, and a candidate that carries no axis renders byte for byte as it
//! did before.

use std::io::Write;
use std::process::{Command, Stdio};

const RECALL_CONFIG: &str = "../../templates/memory-hive/recall/config.json";

/// `${VAR:-default}` becomes the default, a bare `${VAR}` becomes the empty string --
/// the same substitution the colony performs when it instantiates the template.
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

fn recall_script() -> String {
    let raw = std::fs::read_to_string(RECALL_CONFIG).expect("template config");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("config json");
    resolve_vars(
        v["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
    )
}

/// Hand a probe program to python3 **on stdin**, never in argv.
///
/// A probe embeds the whole shipped script as a literal, and a single argv
/// string is capped at 128 KiB (`MAX_ARG_STRLEN`). The recall script crossed
/// that line in W2, and the failure mode is an opaque `ArgumentListTooLong`
/// that looks like a broken test rather than like a size limit. `python3 -`
/// reads and compiles the whole program from stdin before it runs a line of
/// it, so the probe's own `sys.stdin` replacement below is unaffected.
fn run_python(src: &str) -> std::process::Output {
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    // Dropped, not merely borrowed: python reads until EOF.
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    child.wait_with_output().expect("wait")
}

/// Run a probe against the module body of the real script. The `park()` exit at
/// its end is swallowed so the probe can call the helpers the script defines.
fn run_probe(probe: &str) -> String {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO('{{\"envelope\": {{}}, \"body\": {{}}, \"params\": {{}}}}')\n",
            "_sink, _real = io.StringIO(), sys.stdout\n",
            "sys.stdout = _sink\n",
            "try:\n",
            "    exec(compile(_script, 'recall', 'exec'), globals())\n",
            "except SystemExit:\n",
            "    pass\n",
            "finally:\n",
            "    sys.stdout = _real\n",
            "{}"
        ),
        serde_json::to_string(&recall_script()).unwrap(),
        probe
    );
    let out = run_python(&src);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn a_fact_line_names_the_axis_the_value_answers() {
    // The whole ruling in one line: subject and predicate stand in front of the
    // object, separated from it by a colon, and everything that came after the
    // value keeps its place.
    let probe = r#"
print(render_candidate_line({"kind": "fact", "legs": ["keyword"],
                             "subject": "user", "predicate": "favorite_city",
                             "text": "Athen"}))
print(render_candidate_line({"kind": "fact", "legs": ["temporal"],
                             "subject": "user:u1", "predicate": "favorite_editor",
                             "text": "vscode",
                             "history": [{"claim": "Helix", "until": "2026-08-08T19:28:00Z"}]}))
"#;
    assert_eq!(
        run_probe(probe),
        "- [fact keyword] user favorite_city: Athen\n\
         - [fact temporal] user:u1 favorite_editor: vscode \
         (previously: Helix until 2026-08-08)"
    );
}

#[test]
fn the_axis_sits_in_front_of_every_other_annotation() {
    // Reading order of one line: which question, which value, when it held,
    // whether it still holds, what it replaced. The axis is the FIRST half
    // because it is what the rest of the line is about.
    let probe = r#"
print(render_candidate_line({
    "kind": "fact", "legs": ["temporal"], "subject": "user",
    "predicate": "personal_record", "text": "27:12", "intent": "planned",
    "span": {"from": "2023-05-23T00:00:00Z", "until": "2023-05-30T00:00:00Z"},
    "superseded": "25:50",
    "history": [{"claim": "31:04", "until": "2023-05-23T00:00:00Z"}],
    "seen": 2}))
"#;
    assert_eq!(
        run_probe(probe),
        "- [fact temporal] user personal_record: 27:12 (planned) \
         [2023-05-23 -> 2023-05-30] (superseded by: 25:50) \
         (previously: 31:04 until 2023-05-23) (seen: 2)"
    );
}

#[test]
fn a_candidate_without_an_axis_renders_exactly_as_before() {
    // O-5's standing promise. An episode has no axis at all, and a fact row that
    // predates the columns carries only half of one -- neither may grow a
    // dangling colon or a stray space.
    let probe = r#"
print(render_candidate_line({"kind": "episode", "legs": ["keyword"],
                             "text": "Nein, doch vscode."}))
print(render_candidate_line({"kind": "fact", "legs": ["keyword"], "text": "27:12"}))
print(render_candidate_line({"kind": "fact", "legs": ["keyword"],
                             "subject": None, "predicate": None, "text": "27:12"}))
"#;
    assert_eq!(
        run_probe(probe),
        "- [episode keyword] Nein, doch vscode.\n\
         - [fact keyword] 27:12\n\
         - [fact keyword] 27:12"
    );
}

#[test]
fn half_an_axis_renders_the_half_that_is_there() {
    // A row written before the subject or the predicate column existed is not an
    // error, and losing the half it DOES carry would be a worse answer than
    // rendering it. Same conservatism as `axis_key`/`subject_key` one level up.
    let probe = r#"
print(render_candidate_line({"kind": "fact", "legs": ["keyword"],
                             "subject": "user", "text": "Athen"}))
print(render_candidate_line({"kind": "fact", "legs": ["keyword"],
                             "predicate": "favorite_city", "text": "Athen"}))
"#;
    assert_eq!(
        run_probe(probe),
        "- [fact keyword] user: Athen\n- [fact keyword] favorite_city: Athen"
    );
}

//! GH #670 -- a scope recognises its own outdated vocabulary.
//!
//! The compose cell defines its components on the bootstrap pass and never
//! again: `bootstrap` is true when the page has no `/` or a root that is not
//! ours. A screen that is already running would, after a template swap, get
//! new code and keep the old vocabulary. So the root carries a fingerprint of
//! `components()` as a prop, and a read pass whose root holds another
//! fingerprint redefines everything -- once per change of vocabulary, not per
//! tick. Same economy the screen already has for an application's components
//! ("the definitions only travel when they changed"), now for its own language.
//!
//! The script is run the way a `code` cell runs it: as a subprocess, with the
//! read-pass document on stdin, and the bundle it answers is what the display
//! would receive. Skips when `python3` is absent or the templates do not ship,
//! like every other interpreter guard in this tree (R2b).

use std::io::Write;
use std::process::{Command, Stdio};

use meclaw_core::serde_json::{Value, json};

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const COMPOSE: &str = "templates/display/compose/compose.py";

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

/// The fingerprint the shipped script computes (`compose.VOCAB`), the one a
/// local copy of `components()` computes the same way, and the one a copy with
/// a single changed template computes. `None` when there is no `python3` on
/// this host.
fn probe() -> Option<Value> {
    let out = Command::new("python3")
        .arg("-c")
        .arg(
            "import hashlib, importlib.util, json, sys\n\
             spec = importlib.util.spec_from_file_location('compose', sys.argv[1])\n\
             compose = importlib.util.module_from_spec(spec)\n\
             spec.loader.exec_module(compose)\n\
             def fp(c):\n\
             \treturn hashlib.sha256(json.dumps(c, sort_keys=True).encode('utf-8')).hexdigest()[:12]\n\
             same = compose.components()\n\
             moved = compose.components()\n\
             moved[0]['template'] = moved[0]['template'] + ' '\n\
             print(json.dumps({'vocab': compose.VOCAB, 'same': fp(same), 'moved': fp(moved),\n\
                               'count': len(same)}))",
        )
        .arg(repo(COMPOSE))
        .output()
        .ok()?;
    assert!(
        out.status.success(),
        "compose.py did not answer:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(meclaw_core::serde_json::from_slice(&out.stdout).expect("the probe is JSON"))
}

/// Run the shipped script over one document on stdin, the way a `code` cell
/// does, and return the calls of the bundle it answers with -- each one the
/// parsed `text` of a `tool_call` turn. An empty answer is an empty list.
/// `None` when there is no `python3` on this host.
fn read_pass(objects: Option<&Value>) -> Option<Vec<Value>> {
    // The display's answer to the `query`: nothing at all before the first
    // bootstrap, the objects it holds afterwards.
    let messages = match objects {
        None => json!([]),
        Some(objs) => json!([{
            "origin": "tool",
            "type": "tool_result",
            "id": "d-query",
            "text": json!({"objects": objs}).to_string(),
        }]),
    };
    let doc = json!({
        "body": {"messages": messages},
        "envelope": {"header": {
            "hop": {"operation": "query"},
            "context": {
                "display_origin": "read",
                "display_views": json!({"views": [], "define": []}).to_string(),
            },
        }},
    });
    let mut child = Command::new("python3")
        .arg(repo(COMPOSE))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(doc.to_string().as_bytes())
        .expect("the document reaches the script");
    let out = child.wait_with_output().expect("the script ends");
    assert!(
        out.status.success(),
        "compose.py failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let answer: Value =
        meclaw_core::serde_json::from_slice(&out.stdout).expect("the answer is JSON");
    let calls = match &answer {
        Value::Array(list) if list.is_empty() => Vec::new(),
        Value::Object(emission) => emission["messages"]
            .as_array()
            .expect("a bundle has messages")
            .iter()
            .map(|turn| {
                assert_eq!(turn["type"], "tool_call");
                meclaw_core::serde_json::from_str(turn["text"].as_str().expect("a call"))
                    .expect("a call is JSON")
            })
            .collect(),
        other => panic!("one emission or none: {other}"),
    };
    Some(calls)
}

/// The objects a bootstrap creates, as the display would hold them and answer
/// a later `query` with: id, parent, ord and props of every `object.create`.
fn held_after(calls: &[Value]) -> Value {
    Value::Array(
        calls
            .iter()
            .filter(|c| c["op"] == "object.create")
            .map(|c| {
                json!({
                    "id": c["id"],
                    "parent": c["parent"],
                    "ord": c["ord"],
                    "props": c["props"],
                })
            })
            .collect(),
    )
}

fn ops(calls: &[Value]) -> Vec<&str> {
    calls
        .iter()
        .map(|c| c["op"].as_str().unwrap_or(""))
        .collect()
}

fn count(calls: &[Value], op: &str) -> usize {
    calls.iter().filter(|c| c["op"] == op).count()
}

/// A page whose root is ours but carries another fingerprint is not a
/// bootstrap -- and still gets every component of the scope defined again,
/// with the root brought up to the fingerprint of the code that runs.
#[test]
fn a_swapped_scope_redefines_its_vocabulary_without_a_bootstrap() {
    if !library_ships() {
        return;
    }
    let Some(probe) = probe() else {
        return;
    };
    let Some(first) = read_pass(None) else {
        return;
    };
    let mut held = held_after(&first);
    let root = held
        .as_array_mut()
        .expect("a list")
        .iter_mut()
        .find(|o| o["id"] == "display.root")
        .expect("the bootstrap creates the root");
    root["props"]["vocab"] = json!("0000deadbeef");

    let calls = read_pass(Some(&held)).expect("python3 answered once already");
    let defines = count(&calls, "component.define");
    assert_eq!(
        defines,
        probe["count"].as_u64().expect("a count") as usize,
        "every component of the scope is defined again: {:?}",
        ops(&calls)
    );
    assert_eq!(
        count(&calls, "object.create"),
        0,
        "not a bootstrap: nothing is created: {:?}",
        ops(&calls)
    );
    assert_eq!(
        count(&calls, "page.set"),
        0,
        "not a bootstrap: the page stands"
    );
    // The definitions come first -- an object that names a component may only
    // be written once the component exists -- and the root is then brought up
    // to the fingerprint of the vocabulary that was just defined.
    assert!(
        ops(&calls)[..defines]
            .iter()
            .all(|op| *op == "component.define"),
        "the definitions lead the bundle: {:?}",
        ops(&calls)
    );
    let update = calls
        .iter()
        .find(|c| c["op"] == "object.update" && c["id"] == "display.root")
        .unwrap_or_else(|| panic!("the root is brought up to date: {:?}", ops(&calls)));
    assert_eq!(update["props"]["vocab"], probe["vocab"]);
}

/// The same page with the fingerprint the code computes: no definition, no
/// root update, nothing at all -- the screen is what the code would build.
#[test]
fn an_unchanged_vocabulary_sends_no_definition() {
    if !library_ships() {
        return;
    }
    let Some(probe) = probe() else {
        return;
    };
    let Some(first) = read_pass(None) else {
        return;
    };
    assert_eq!(
        count(&first, "component.define"),
        probe["count"].as_u64().expect("a count") as usize,
        "the bootstrap defines the whole scope"
    );
    let held = held_after(&first);
    let root = held
        .as_array()
        .expect("a list")
        .iter()
        .find(|o| o["id"] == "display.root")
        .expect("the bootstrap creates the root");
    assert_eq!(
        root["props"]["vocab"], probe["vocab"],
        "the root is created with the fingerprint of the code that created it"
    );

    let calls = read_pass(Some(&held)).expect("python3 answered once already");
    assert_eq!(
        count(&calls, "component.define"),
        0,
        "an unchanged vocabulary travels nowhere: {:?}",
        ops(&calls)
    );
    assert!(
        !calls
            .iter()
            .any(|c| c["op"] == "object.update" && c["id"] == "display.root"),
        "the root is not rewritten: {:?}",
        ops(&calls)
    );
    assert!(
        calls.is_empty(),
        "a screen that matches the code gets no bundle at all: {:?}",
        ops(&calls)
    );
}

/// The fingerprint is twelve hex characters over `components()` as JSON, and
/// it moves when one template string does. Computed here over a local copy of
/// the list, never by patching the source.
#[test]
fn the_fingerprint_moves_when_a_component_does() {
    if !library_ships() {
        return;
    }
    let Some(probe) = probe() else {
        return;
    };
    let vocab = probe["vocab"].as_str().expect("VOCAB is a string");
    assert_eq!(vocab.len(), 12, "twelve characters: {vocab}");
    assert!(vocab.chars().all(|c| c.is_ascii_hexdigit()), "hex: {vocab}");
    assert_eq!(
        probe["same"], probe["vocab"],
        "the fingerprint is sha256 of components() as sorted JSON, cut to twelve"
    );
    assert_ne!(
        probe["moved"], probe["vocab"],
        "one changed template string moves the fingerprint"
    );
}

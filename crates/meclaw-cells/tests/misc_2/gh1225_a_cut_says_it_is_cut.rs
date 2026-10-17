//! GH #1225 (KX, OR-KF-43) -- a text cut to a bound says it was cut.
//!
//! The class script `silent_cut` of the curator wave found eleven cuts outside
//! the curator that clipped a text to a fixed number and said nothing: a name,
//! the head of the last tool result, a digest item, a file's or a node's one
//! line, a speaker. R-IG-1: a bound is never silent, and it comes from a param,
//! never from a number in the script.
//!
//! Each of the eight scripts now carries one helper, `clip(text, cap)` (in
//! `file-space/derive` and `read`, beside the content budget's own `clip`, the
//! extractor twin's `_ex_clip`), whose cut ends in `…` and stays inside the bound,
//! and each bound is a declared param with the old number as its default:
//! `params`, `contract.settings` and the script's own fallback agree. The probes
//! run the shipped cut with a small bound and see the mark; a text that fits is
//! left whole.

use meclaw_core::serde_json::{self as sj, Value, json};
use std::io::Write;
use std::process::{Command, Stdio};

fn config(template: &str) -> Option<Value> {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates")
        .join(template)
        .join("config.json");
    let raw = std::fs::read_to_string(&p).ok()?;
    Some(sj::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display())))
}

fn python(src: &str, stdin: &str) -> String {
    let mut child = Command::new("python3")
        .arg("-I")
        .arg("-c")
        .arg(src)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(stdin.as_bytes())
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "python failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// The pure half of a script under `params` (imports, defs, upper-case
/// constants, in file order), then `probe` evaluated with `ARGS`.
const LOADER: &str = r#"
import ast, io, json, sys
inp = json.load(sys.stdin)
sys.stdin = io.StringIO("")
src, params = inp["src"], inp["params"]
scope = {"P": params, "knobs": params,
         "doc": {"params": params, "body": {}, "envelope": {"header": {}}}}
for n in ast.parse(src).body:
    if isinstance(n, (ast.Import, ast.ImportFrom, ast.FunctionDef)) or (
            isinstance(n, ast.Assign)
            and all(isinstance(t, ast.Name) and t.id.isupper() for t in n.targets)):
        try:
            exec(compile(ast.Module(body=[n], type_ignores=[]), "cell", "exec"), scope)
        except Exception:
            pass
scope["ARGS"] = inp.get("args")
print(json.dumps(eval(inp["probe"], scope)))
"#;

/// `probe` over the pure half of `template`'s script, its shipped params with
/// `over` laid on top. None when the tree has no such template.
fn probe(template: &str, over: Value, probe: &str, args: Value) -> Option<Value> {
    let cfg = config(template)?;
    let mut params = cfg["params"].as_object().cloned().unwrap_or_default();
    let src = params
        .remove("script_inline")
        .and_then(|v| v.as_str().map(str::to_string))
        .expect("script_inline");
    for (k, v) in over.as_object().cloned().unwrap_or_default() {
        params.insert(k, v);
    }
    let doc = json!({"src": src, "params": params, "probe": probe, "args": args});
    let out = python(LOADER, &doc.to_string());
    Some(sj::from_str(out.trim()).unwrap_or_else(|e| panic!("{template}: {e}: {out}")))
}

/// A cut of `bound`: inside the bound and ending in the mark.
fn assert_cut(template: &str, got: &Value, bound: usize) {
    let s = got.as_str().unwrap_or_else(|| panic!("{template}: {got}"));
    assert!(
        s.ends_with('…') && s.chars().count() <= bound && s.chars().count() > 1,
        "{template}: a cut to {bound} must end in the mark and stay inside the bound: {s:?}"
    );
}

/// params, contract.settings and the script's fallback carry one value.
fn assert_knob(template: &str, knob: &str, needle: &str, default: i64) {
    let Some(cfg) = config(template) else {
        return;
    };
    assert_eq!(
        cfg["params"][knob],
        json!(default),
        "{template}: params.{knob} is the old bound"
    );
    let set = &cfg["contract"]["settings"][knob];
    assert_eq!(
        set["default"],
        json!(default),
        "{template}: contract.settings.{knob}.default"
    );
    assert_eq!(
        set["type"],
        json!("number"),
        "{template}: {knob} is a number"
    );
    let desc = set["description"].as_str().unwrap_or("");
    assert!(
        desc.contains('…'),
        "{template}: the description of {knob} names the mark: {desc}"
    );
    let src = cfg["params"]["script_inline"].as_str().expect("script");
    let at = src
        .find(needle)
        .unwrap_or_else(|| panic!("{template}: the script does not read {needle}"));
    let rest = &src[at + needle.len()..];
    let lit = rest[..rest.find([')', '}', ','].as_ref()).expect("end")].trim();
    assert_eq!(
        lit,
        default.to_string(),
        "{template}: the script's fallback for {knob} drifted from the param"
    );
}

const LONG: &str = "Abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyz";

#[test]
fn gh1225_every_cut_bound_is_a_param_with_the_old_default() {
    assert_knob("affinity/brief", "name_cap", "_int(\"name_cap\", ", 64);
    assert_knob(
        "collector/assemble",
        "partial_head",
        "_int(\"partial_head\", ",
        200,
    );
    assert_knob(
        "daily-digest/shelf",
        "item_chars",
        "_int(\"item_chars\", ",
        240,
    );
    assert_knob(
        "file-space/derive",
        "oneline_max",
        "knob_int(\"oneline_max\", ",
        120,
    );
    assert_knob(
        "file-space/derive",
        "node_oneline_max",
        "\"node_oneline_max\": ",
        120,
    );
    assert_knob(
        "file-space/read",
        "node_oneline_max",
        "_int(\"node_oneline_max\", ",
        120,
    );
    assert_knob(
        "file-space/ingest",
        "oneline_max",
        "_int(\"oneline_max\", ",
        120,
    );
    assert_knob(
        "librarian/index",
        "oneline_max",
        "_int(\"oneline_max\", ",
        300,
    );
    assert_knob(
        "memory-drain/drain",
        "speaker_max",
        "_int(\"speaker_max\", ",
        120,
    );
}

#[test]
fn gh1225_a_cut_name_or_head_ends_in_the_mark() {
    // affinity: the record's display name, released whole, longer than the cap.
    if let Some(got) = probe(
        "affinity/brief",
        json!({"name_cap": 12}),
        "released_name(ARGS, ARGS)",
        json!(LONG),
    ) {
        assert_cut("affinity/brief", &got, 12);
    }
    // collector: the head of the last result of a capped round.
    if let Some(got) = probe(
        "collector/assemble",
        json!({"partial_head": 20}),
        "partial_answer_turn([{'type': 'tool_call', 'id': 'c1'}, \
         {'type': 'tool_result', 'text': ARGS}], {'c1': 'web_search'})['text']",
        json!(LONG),
    ) {
        let text = got.as_str().expect("text");
        let head = text.rsplit("The last result began: ").next().unwrap_or("");
        let head = head.strip_suffix('.').unwrap_or(head);
        assert_cut("collector/assemble", &json!(head), 20);
    }
    // memory-drain: a peer's name out of an import document; the schema's
    // 120 stays the ceiling whatever the param says.
    let turn = |cap: i64| {
        probe(
            "memory-drain/drain",
            json!({"speaker_max": cap}),
            "turns_of({'messages': [{'type': 'text', 'origin': 'peer', \
             'text': 'hi', 'speaker': ARGS}]})[0]['speaker']",
            json!(LONG.repeat(3)),
        )
    };
    if let Some(got) = turn(10) {
        assert_cut("memory-drain/drain", &got, 10);
    }
    if let Some(got) = turn(500) {
        assert_cut("memory-drain/drain", &got, 120);
    }
}

#[test]
fn gh1225_a_cut_one_line_ends_in_the_mark() {
    if let Some(got) = probe(
        "file-space/ingest",
        json!({"oneline_max": 15}),
        "address_line('fh-0123456789ab', 'abcdef012345', 'n.txt', 0, ARGS, False)",
        json!(LONG),
    ) {
        let line = got.as_str().expect("line");
        let one = line
            .split("\": ")
            .nth(1)
            .and_then(|s| s.strip_suffix(']'))
            .unwrap_or("");
        assert_cut("file-space/ingest", &json!(one), 15);
    }
    if let Some(got) = probe(
        "librarian/index",
        json!({"oneline_max": 15}),
        "described_ops('fh-0123456789ab', 'abcdef012345', {'oneline': ARGS}, 't')[0][1]['set']['oneline']",
        json!(LONG),
    ) {
        assert_cut("librarian/index", &got, 15);
    }
    // derive: the summary's one line and a file without a text view.
    if let Some(got) = probe(
        "file-space/derive",
        json!({}),
        "parse_summary(ARGS + '\\nmore', 15)[0]",
        json!(LONG),
    ) {
        assert_cut("file-space/derive", &got, 15);
    }
    if let Some(got) = probe(
        "file-space/derive",
        json!({}),
        "name_oneline(ARGS + '.bin', 'application/octet-stream', '', 10, 15)",
        json!(LONG),
    ) {
        assert_cut("file-space/derive", &got, 15);
    }
    // the extractor twin: a node's line under the cell's own limits.
    for (cell, limits) in [
        ("file-space/derive", "ex_limits()"),
        ("file-space/read", "LIMITS"),
    ] {
        if let Some(got) = probe(
            cell,
            json!({"node_oneline_max": 15}),
            &format!(
                "[n['oneline'] for n in extract('a.py', 'text/x-python', \
                 ('def ' + ARGS + '():\\n    pass\\n').encode('utf-8'), None, {limits})['nodes']]"
            ),
            json!(LONG),
        ) {
            let line = got
                .as_array()
                .expect("nodes")
                .iter()
                .find(|l| l.as_str().is_some_and(|s| s.starts_with("def ")))
                .unwrap_or_else(|| panic!("{cell}: no def node: {got}"))
                .clone();
            assert_cut(cell, &line, 15);
        }
    }
}

/// A minified JSON with CRLF line ends: every key sits on the one long line,
/// and the extractor makes one node per key. The node's line is cut where the
/// extractor makes it, so no node holds more than the bound before
/// `nodes_max` applies -- the size of the node values, not a process RSS, is
/// what this measures (re-check KX I-1).
#[test]
fn gh1225_a_node_line_is_cut_where_it_is_made() {
    let keys = 50;
    let body = format!(
        "{{{}}}\r\n",
        (0..keys)
            .map(|i| format!("\"k{i:03}\": \"{}\"", "v".repeat(400)))
            .collect::<Vec<_>>()
            .join(",")
    );
    for (cell, limits) in [
        ("file-space/derive", "ex_limits()"),
        ("file-space/read", "LIMITS"),
    ] {
        let Some(got) = probe(
            cell,
            json!({"node_oneline_max": 15}),
            &format!(
                "[(seen := []), \
                 EXTRACTORS.__setitem__('json', [(lambda f: lambda *a: seen.append(f(*a)) or seen[-1])(EXTRACTORS['json'][0])]), \
                 (out := extract('x.json', 'application/json', ARGS.encode('utf-8'), None, {limits})), \
                 [max(len(n['oneline']) for n in seen[0]['nodes']), len(seen[0]['nodes']), \
                  [n['oneline'] for n in out['nodes']]]][-1]"
            ),
            json!(body),
        ) else {
            continue;
        };
        let made = got[0].as_u64().expect("longest made line");
        assert_eq!(got[1], json!(keys), "{cell}: one node per key: {}", got[1]);
        assert!(
            made <= 15,
            "{cell}: the extractor holds a node line of {made} chars -- it must cut it where it makes it, to node_oneline_max 15"
        );
        let lines = got[2].as_array().expect("nodes");
        assert_eq!(lines.len(), keys, "{cell}: every key node is kept");
        for l in lines {
            assert_cut(cell, l, 15);
        }
    }
}

#[test]
fn gh1225_a_cut_digest_item_ends_in_the_mark() {
    let Some(cfg) = config("daily-digest/shelf") else {
        return;
    };
    let src = cfg["params"]["script_inline"].as_str().expect("script");
    let doc = json!({
        "params": {"item_chars": 20},
        "body": {"messages": [{"origin": "assistant", "type": "text",
                               "text": format!("{LONG} end. Short one.")}]},
        "envelope": {"header": {"hop": {}, "context": {"digest_round": "[\"m\"]"}}},
    });
    let out: Value = sj::from_str(python(src, &doc.to_string()).trim()).expect("json");
    let args: Value =
        sj::from_str(out[0]["messages"][0]["text"].as_str().expect("call")).expect("args");
    let items: Vec<String> =
        sj::from_str(args["row"]["items"].as_str().expect("items")).expect("list");
    assert_cut("daily-digest/shelf", &json!(items[0]), 20);
    assert_eq!(
        items.last().map(String::as_str),
        Some("Short one."),
        "{items:?}"
    );
}

#[test]
fn gh1225_a_text_that_fits_is_left_whole() {
    for (template, call) in [
        ("affinity/brief", "clip(ARGS, 5)"),
        ("collector/assemble", "clip(ARGS, 5)"),
        ("daily-digest/shelf", "clip(ARGS, 5)"),
        ("file-space/derive", "_ex_clip(ARGS, 5)"),
        ("file-space/read", "_ex_clip(ARGS, 5)"),
        ("file-space/ingest", "clip(ARGS, 5)"),
        ("librarian/index", "clip(ARGS, 5)"),
        ("memory-drain/drain", "clip(ARGS, 5)"),
    ] {
        if let Some(got) = probe(template, json!({}), call, json!("abcde")) {
            assert_eq!(got, json!("abcde"), "{template}: {call}");
        }
        if let Some(got) = probe(template, json!({}), call, json!("abcdef")) {
            assert_eq!(got, json!("abcd…"), "{template}: {call}");
        }
    }
}

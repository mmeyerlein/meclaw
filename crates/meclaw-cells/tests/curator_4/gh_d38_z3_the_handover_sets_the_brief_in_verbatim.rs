//! GH #1440 (D38 Z3) -- the handover form: `./handover` sets an itemised
//! summary into its block word for word, and stays as it is.
//!
//! WHY this file exists: Z3 changes what a summary looks like, not how the
//! handover is made (plan T2 § Z3, the contract: `handover_text()` stays
//! unchanged and sets the itemised summary in verbatim). A handover that
//! re-flowed, sorted or cut the lines would undo the brief at the one place a
//! new session reads it first. The block keeps the reading order topic,
//! pins, commitments, summary, turns (GH #896), and the brief stands under
//! the note title as one piece.
//!
//! Loads the pure half of the shipped `curator/handover` script (imports,
//! defs, upper-case constants -- the loader of `support/curator_hive.rs`
//! `policy_scope`, here over `./handover`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

const LOADER: &str = r#"
import ast, io, json, sys
inp = json.load(sys.stdin)
sys.stdin = io.StringIO("")
src, params = inp["src"], inp["params"]
keep = [n for n in ast.parse(src).body
        if isinstance(n, (ast.Import, ast.ImportFrom, ast.FunctionDef))
        or (isinstance(n, ast.Assign)
            and all(isinstance(t, ast.Name) and t.id.isupper() for t in n.targets))]
scope = {"P": params, "ctx": {}, "hop": {},
         "doc": {"params": params, "body": {}, "envelope": {}}}
for n in keep:
    exec(compile(ast.Module(body=[n], type_ignores=[]), "handover", "exec"), scope)
scope["ARGS"] = inp.get("args")
print(json.dumps(eval(inp["probe"], scope)))
"#;

fn handover_scope(probe: &str, args: Value) -> Value {
    let mut params = cell_config("handover")["params"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    params.remove("script_inline");
    let doc = json!({"src": script_of("handover"), "params": params, "probe": probe,
                     "args": args});
    let out = run_python(LOADER, &doc.to_string());
    assert!(
        out.status.success(),
        "the pure half of ./handover does not load: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    sj::from_slice(&out.stdout).expect("json")
}

const BRIEF: &str = "- topic (2026-11-05): the garden [#0a1b2c3d4e5f]\n\
                     - person (2026-11-05) hannah/lives_in: Hannah moved to Porto. [asserted] \
                     [superseded 2026-11-04] [#1a2b3c4d5e6f]\n\
                     - promised (2026-11-05) call/hannah: The companion calls Hannah on Sunday. \
                     [#2a3b4c5d6e7f]";

#[test]
fn the_handover_sets_the_brief_in_verbatim() {
    if !shipped() {
        return;
    }
    let got = handover_scope(
        "handover_text((), (), 'the garden', ARGS, ['user: how are the tomatoes?'])",
        json!(BRIEF),
    );
    let block = got.as_str().expect("a block").to_string();
    let title = handover_scope("NOTE_TITLE", Value::Null);
    let title = title.as_str().expect("a title");
    assert!(
        block.contains(&format!("{title}\n{BRIEF}")),
        "the brief stands under the note title, word for word: {block}"
    );
    let (t, s, u) = (
        block.find("the garden").expect("the topic"),
        block.find(title).expect("the summary"),
        block.find("how are the tomatoes").expect("the turns"),
    );
    assert!(t < s && s < u, "topic, summary, turns: {block}");
}

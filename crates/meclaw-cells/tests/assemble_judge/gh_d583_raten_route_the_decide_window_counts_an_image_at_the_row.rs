//! GH #1532 (wave d58-3, strand raten) -- `talky/route` counts an inline
//! image of the decide window at the catalog row the view's answer names.
//!
//! Before, the route weighed every image of the earlier turns at a fixed 1024
//! tokens (`IMAGE_TOKENS`), whatever model the window was for; the llm cell
//! counts it at the row's `tokens_per_image` (`window.rs`
//! `estimate_prompt_tokens`). Now the answer of the view names it
//! (`hop.tokens_per_image`), the route counts with it, and without it the
//! estimate is the one of before.
//!
//! The shipped script runs as a `code` cell runs it, over one `in_view`;
//! then the probe reads the number the run left and the judge block it
//! builds at a fixed budget (no model).

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::{config_of, lane, now_ms, params_of};
use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Stdio};

const ROUTE: &str = "templates/talky/route/config.json";

/// Run the shipped route over one `in_view` whose hop carries `extra`, then
/// `program` in the script's globals; the probe's own stdout.
fn probe(extra: Value, program: &str) -> String {
    let script = config_of(ROUTE)["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string();
    let mut hop = json!({"state": "How is the weather tomorrow?",
                         "asked_at": (now_ms() + 3_600_000).to_string()});
    for (k, v) in extra.as_object().expect("extra object") {
        hop[k] = v.clone();
    }
    let mut doc = lane(
        "in_view",
        hop,
        json!({"turn_id": "t2"}),
        json!([{"origin": "user", "type": "text", "text": "Good morning."}]),
    );
    doc["params"] = params_of(ROUTE, &[]);
    let stdin_doc = meclaw_testing::code_stdin(&doc).to_string();
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "_sink, _out = io.StringIO(), sys.stdout\n",
            "sys.stdout = _sink\n",
            "try:\n",
            "    exec(compile(_script, 'cell', 'exec'), globals())\n",
            "except SystemExit:\n",
            "    pass\n",
            "finally:\n",
            "    sys.stdout = _out\n",
            "{}\n"
        ),
        serde_json::to_string(&script).unwrap(),
        serde_json::to_string(&stdin_doc).unwrap(),
        program
    );
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "route probe: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// The image cost the run counts with, and the hint line of a judge block of
/// one image turn and one hint at a budget of 1100 tokens: at 1024 the hint
/// fits, at 1600 it is cut with a mark.
const PROGRAM: &str = "print(IMAGE_TOKENS, estimate_tokens('data:image/png;base64,AAAA'), \
     'cut:' in judge_block({'turns': [{'role': 'user', 'text': 'data:image/png;base64,AAAA'}], \
     'hints': [{'question': 'Its concerts?', 'answer': 'yes', 'class': 'simple'}]}, 'x', 1100))";

#[test]
fn gh_d583_raten_route_an_image_counts_the_cost_the_view_names() {
    assert_eq!(
        probe(json!({"tokens_per_image": 1600}), PROGRAM),
        "1600 1600 True"
    );
    // A stamp as text counts as well.
    assert_eq!(
        probe(json!({"tokens_per_image": "1600"}), PROGRAM),
        "1600 1600 True"
    );
}

#[test]
fn gh_d583_raten_route_without_a_row_an_image_is_the_cores() {
    for extra in [
        json!({}),
        json!({"tokens_per_image": null}),
        json!({"tokens_per_image": 0}),
        json!({"tokens_per_image": "many"}),
        json!({"tokens_per_image": true}),
    ] {
        assert_eq!(probe(extra.clone(), PROGRAM), "1024 1024 False", "{extra}");
    }
}

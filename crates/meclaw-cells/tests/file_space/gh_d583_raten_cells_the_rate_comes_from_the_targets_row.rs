//! GH #1532 (wave d58-3, strand raten) -- five producers size what they hand a
//! model at that model's rate, not at a fixed three characters a token.
//!
//! `builder-librarian/retrieve`, `file-space/read`, `file-space/derive`,
//! `firewall/screen` and `summarizer/prep` carried `content-budget v1`, whose
//! `budget_chars` multiplied a window by a fixed `CHARS_PER_TOKEN = 3`. The llm
//! cell counts a prompt at the catalog row's `chars_per_token`
//! (`window.rs` `estimate_prompt_tokens`), so a share of a window was measured
//! in other tokens than the window it fills. Now each carries
//! `content-budget v2`, and every budget takes the rate from the same place
//! as its window:
//!
//! * a window on the hop or in the context: the `chars_per_token` of the same
//!   map (the request's row);
//! * a window a refusal named: the `chars_per_token` the refusal names beside
//!   it (`meta.error`, the refusing model's row);
//! * a fallback window of a param: no rate is known there -- the core's three,
//!   exactly as before.
//!
//! A row with rate 4 counts at 4; without one the count is the old one.

// harness_fold: #[path = "support/file_space_hive.rs"]
#[allow(unused_imports)]
use crate::file_space_hive; // harness_fold: mod file_space_hive;

use file_space_hive::*;
use meclaw_core::serde_json::{self as sj, Map, Value, json};
use std::io::Write;
use std::process::{Command, Stdio};

/// The five cells of this strand.
const CELLS: [&str; 5] = [
    "builder-librarian/retrieve",
    "file-space/derive",
    "file-space/read",
    "firewall/screen",
    "summarizer/prep",
];

fn template_config(cell: &str) -> Value {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates")
        .join(cell)
        .join("config.json");
    let raw = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    sj::from_str(&raw).expect("config json")
}

fn script(cell: &str) -> String {
    template_config(cell)["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string()
}

/// Run the shipped script of `cell` on one message (its own params as
/// shipped), then evaluate `program` in the script's globals; its stdout.
/// With an empty `program` the stdout is what the cell emitted.
fn probe(cell: &str, context: Value, hop: Value, body: Value, program: &str) -> String {
    let mut params: Map<String, Value> = template_config(cell)["params"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    params.remove("script_inline");
    let doc = json!({"params": params, "body": body,
                     "envelope": {"header": {"context": context, "hop": hop}}});
    let src = if program.is_empty() {
        format!(
            "import sys, io\nsys.stdin = io.StringIO({})\nexec(compile({}, 'cell', 'exec'), globals())\n",
            sj::to_string(&doc.to_string()).unwrap(),
            sj::to_string(&script(cell)).unwrap(),
        )
    } else {
        format!(
            concat!(
                "import sys, io\n",
                "sys.stdin = io.StringIO({})\n",
                "_sink, _out = io.StringIO(), sys.stdout\n",
                "sys.stdout = _sink\n",
                "try:\n",
                "    exec(compile({}, 'cell', 'exec'), globals())\n",
                "except SystemExit:\n",
                "    pass\n",
                "finally:\n",
                "    sys.stdout = _out\n",
                "{}\n"
            ),
            sj::to_string(&doc.to_string()).unwrap(),
            sj::to_string(&script(cell)).unwrap(),
            program
        )
    };
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
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A text of `n` characters that never repeats a run long enough to look
/// like a mark.
fn long(prefix: &str, n: usize) -> String {
    let mut s = String::from(prefix);
    let mut i = 0usize;
    while s.chars().count() < n {
        s.push_str(&format!(" w{i}"));
        i += 1;
    }
    let mut s: String = s.chars().take(n).collect();
    if s.ends_with(' ') {
        s.pop();
        s.push('x');
    }
    s
}

// ═════════════════════════════════════════════════════════════ the block

#[test]
fn every_cell_carries_exactly_one_v2_block_and_no_v1() {
    for cell in CELLS {
        let s = script(cell);
        assert_eq!(
            s.matches("# >>> content-budget v2 (").count(),
            1,
            "{cell}: one block that takes the target's rate"
        );
        assert_eq!(s.matches("# <<< content-budget v2").count(), 1, "{cell}");
        assert!(
            !s.contains("content-budget v1"),
            "{cell}: the fixed-rate block is gone"
        );
    }
}

/// Every call of `budget_chars` passes a rate -- the window's own, or `None`
/// where the window is a param's fallback whose row's rate the cell cannot
/// know (the core's three). Parsed, not grepped: a call over two lines
/// counts as one.
#[test]
fn every_budget_of_the_five_cells_passes_a_rate() {
    for cell in CELLS {
        // The script on stdin: a shipped script is longer than one argument
        // may be.
        let program = concat!(
            "import ast, sys\n",
            "src = sys.stdin.read()\n",
            "bad = [n.lineno for n in ast.walk(ast.parse(src)) if isinstance(n, ast.Call) ",
            "and getattr(n.func, 'id', '') == 'budget_chars' and len(n.args) + len(n.keywords) != 3]\n",
            "print(bad)\n"
        );
        let mut child = Command::new("python3")
            .args(["-c", program])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("python3");
        let mut sink = child.stdin.take().expect("stdin");
        sink.write_all(script(cell).as_bytes())
            .expect("write script");
        drop(sink);
        let out = child.wait_with_output().expect("wait");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "[]",
            "{cell}: budget_chars calls without a rate at these lines; stderr {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

// ══════════════════════════════════════════════════════ firewall: screen

#[test]
fn the_screen_caps_a_turn_at_the_rate_beside_the_window() {
    let cell = "firewall/screen";
    let cap =
        |context: Value, hop: Value| probe(cell, context, hop, json!({}), "print(size_cap())");
    // A quarter of a 1000-token window: 250 tokens.
    assert_eq!(
        cap(json!({"input_soft": 1000, "chars_per_token": 4}), json!({})),
        "1000"
    );
    assert_eq!(
        cap(
            json!({}),
            json!({"input_soft": 1000, "chars_per_token": "4"})
        ),
        "1000"
    );
    // No rate: the core's three, the cap as before.
    assert_eq!(cap(json!({"input_soft": 1000}), json!({})), "750");
    assert_eq!(
        cap(
            json!({"input_soft": 1000, "chars_per_token": "four"}),
            json!({})
        ),
        "750"
    );
    // The rate of the map whose window sizes the turn: the hop's smaller
    // window states none, the context's rate belongs to another window.
    assert_eq!(
        cap(
            json!({"input_soft": 2000, "chars_per_token": 4}),
            json!({"input_soft": 1000})
        ),
        "750"
    );
    // No window: no cap below the carrier (GH #1532, OR-583.raten.oc4.1).
    assert_eq!(cap(json!({}), json!({})), "None");
}

/// GH #1532 (OR-583.raten.oc4.1): live, no edge hands the screen a window, so
/// a fallback window would refuse every turn over its quarter. Contents are
/// bounded only by the catalogue, the curator and the carrier: without a known
/// window the screen does not cap, and a 50 KB turn comes through whole.
#[test]
fn without_a_window_a_50_kb_turn_comes_through_whole() {
    let turn = "a".repeat(50 * 1024);
    let out = probe(
        "firewall/screen",
        json!({"channel": "test", "user_id": "u"}),
        json!({"route": "in_turn"}),
        json!({"messages": [{"origin": "user", "type": "text", "text": turn}]}),
        "",
    );
    let v: Value = sj::from_str(&out).unwrap_or_else(|e| panic!("not JSON ({e}): {out}"));
    let first = match &v {
        Value::Array(a) => a.first().cloned().unwrap_or(Value::Null),
        other => other.clone(),
    };
    assert_eq!(first["header"]["route"], "fwstore", "{v}");
}

// ══════════════════════════════════════════════════════ file-space: read

#[test]
fn a_read_takes_the_rate_beside_the_window_of_its_request() {
    let cell = "file-space/read";
    let max = |context: Value, hop: Value| probe(cell, context, hop, json!({}), "print(MAX_CHARS)");
    // A tenth of a 1000-token window: 100 tokens.
    assert_eq!(
        max(json!({}), json!({"input_soft": 1000, "chars_per_token": 4})),
        "400"
    );
    assert_eq!(
        max(
            json!({"recall_input_soft": "1000", "chars_per_token": "4.5"}),
            json!({})
        ),
        "450"
    );
    assert_eq!(max(json!({}), json!({"input_soft": 1000})), "300");
    assert_eq!(max(json!({}), json!({})), "None");
}

const F: &str = "fh-0a0b0c0d1532";

/// 100 lines of 1000 characters.
fn wide_space() -> Space {
    let text: String = (0..100)
        .map(|_| format!("{}\n", "w".repeat(1000)))
        .collect();
    let mut s = Space::with(
        "/m/files",
        &[(
            "derive",
            "script_inline",
            json!("import sys, json\njson.load(sys.stdin)\nsys.stdout.write('[]')\n"),
        )],
    );
    s.seed_text(F, "/a/wide.txt", &[text.as_str()], &[]);
    s
}

/// A direct read on `in_read` whose hop names the window: the window and its
/// rate are parked with the request and size the answer once the store has
/// answered (`hop` lasts one hop).
#[test]
fn a_parked_read_keeps_the_rate_of_its_window() {
    // 30 000 tokens, a tenth: 9000 characters at three (8 lines of ~1007),
    // 12 000 at four (11 lines).
    for (hop, to) in [
        (json!({"input_soft": 30000, "chars_per_token": 4}), 11),
        (json!({"input_soft": 30000}), 8),
    ] {
        let mut s = wide_space();
        let got = s.request("in_read", "read", Some(F), json!({}), hop.clone());
        assert_eq!(got["ok"], true, "{hop}: {got}");
        assert_eq!(got["to"], to, "{hop}: {got}");
    }
}

/// The real road of a model's read: the tool edge's call through `./tools`,
/// the curator's window and rate in the persistent context.
#[test]
fn a_read_on_the_tool_road_takes_the_contexts_rate() {
    for (context, to) in [
        (
            json!({"tool_caller": "cogny", "input_soft": 30000, "chars_per_token": 4}),
            11,
        ),
        (json!({"tool_caller": "cogny", "input_soft": 30000}), 8),
    ] {
        let mut s = wide_space();
        let before = s.out.len();
        s.lane(
            "in_tool",
            context.clone(),
            json!({"tool_name": "file_read", "tool_call_id": "c1"}),
            json!({"messages": [{"origin": "assistant", "type": "tool_call", "id": "c1",
                                 "text": json!({"file": "/a/wide.txt"}).to_string()}]}),
        );
        let res: Vec<Msg> = s.out[before..]
            .iter()
            .filter(|m| m.route() == "tool_result")
            .cloned()
            .collect();
        assert_eq!(res.len(), 1, "{res:?}; stderr {:?}", s.stderr);
        let got: Value = sj::from_str(res[0].messages()[0]["text"].as_str().expect("text"))
            .expect("a JSON answer");
        assert_eq!(got["to"], to, "{context}: {got}");
    }
}

// ══════════════════════════════════════════════ builder-librarian: retrieve

fn briefing(context: Value, hop_extra: Value) -> String {
    let rows: Vec<Value> = (0..3)
        .map(|i| {
            json!({"id": format!("doc-{i}"), "source": "s", "section": format!("sec{i}"),
                   "kind": "template", "text": long(&format!("row{i}"), 3000)})
        })
        .collect();
    let mut hop = json!({"operation": "search"});
    for (k, v) in hop_extra.as_object().cloned().unwrap_or_default() {
        hop[k] = v;
    }
    let out = probe(
        "builder-librarian/retrieve",
        context,
        hop,
        json!({"messages": [{"origin": "tool", "type": "tool_result", "id": "lib1",
                             "text": Value::Array(rows).to_string()}]}),
        "",
    );
    let v: Value = sj::from_str(&out).unwrap_or_else(|e| panic!("not JSON ({e}): {out}"));
    let first = match v {
        Value::Array(a) => a.into_iter().next().expect("one emission"),
        other => other,
    };
    first["messages"][1]["text"]
        .as_str()
        .expect("the briefing")
        .to_string()
}

#[test]
fn librarian_rows_fill_the_window_at_its_rate() {
    // 20 000 tokens, a tenth: 6000 characters at three -- the second row of
    // 3000 is cut; 8000 at four -- it travels whole.
    let row1 = long("row1", 3000);
    for (context, hop) in [
        (
            json!({"orig_request": "q", "input_soft": 20000, "chars_per_token": 4}),
            json!({}),
        ),
        (
            json!({"orig_request": "q"}),
            json!({"input_soft": 20000, "chars_per_token": "4"}),
        ),
    ] {
        let b = briefing(context.clone(), hop.clone());
        assert!(
            b.contains(&row1),
            "{context} {hop}: the second row whole: {b}"
        );
        assert!(
            b.contains("[cut:"),
            "{context} {hop}: the third is cut: {b}"
        );
    }
    let b = briefing(json!({"orig_request": "q", "input_soft": 20000}), json!({}));
    assert!(
        !b.contains(&row1),
        "no rate: three, the second row cut: {b}"
    );
}

// ══════════════════════════════════════════════════════ summarizer: prep

fn refit(error: Value) -> String {
    let text = format!(
        "Session s1 closed with 1 turns.\n\n## The last turns\n- user: {}",
        "w".repeat(20_000)
    );
    let out = probe(
        "summarizer/prep",
        json!({}),
        json!({"route": "in_error", "error_code": "invalid_input", "session_id": "s1"}),
        json!({"meta": {"error": error},
               "messages": [{"origin": "user", "type": "text", "text": text}]}),
        "",
    );
    let v: Value = sj::from_str(&out).unwrap_or_else(|e| panic!("not JSON ({e}): {out}"));
    v[0]["messages"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("a prompt: {v}"))
        .to_string()
}

#[test]
fn a_refused_day_is_sized_at_the_rate_the_refusal_names() {
    // Half of 2000 tokens: 1000, at the writer row's 4 characters a token.
    let got = refit(json!({"kind": "input_over_hard", "input_hard": 2000, "chars_per_token": 4.0}));
    assert!(
        got.contains("...[cut: 4000 of "),
        "{}",
        &got[got.len() - 120..]
    );
    // A refusal that names no rate: the core's three, as before.
    let got = refit(json!({"kind": "input_over_hard", "input_hard": 2000}));
    assert!(
        got.contains("...[cut: 3000 of "),
        "{}",
        &got[got.len() - 120..]
    );
}

// ══════════════════════════════════════════════════════ file-space: derive

#[test]
fn a_refusal_names_the_rate_beside_its_window() {
    let got = pure(
        "derive",
        "[refused_rate(b) for b in ARGS]",
        json!([
            {"meta": {"error": {"kind": "input_over_hard", "input_hard": 32000, "chars_per_token": 4.0}}},
            {"meta": {"error": {"kind": "input_over_window", "context_window": 8000, "chars_per_token": 3.5}}},
            {"meta": {"error": {"kind": "input_over_hard", "input_hard": 32000}}},
            {"meta": {"error": {"kind": "http_error", "chars_per_token": 4.0}}},
            {}
        ]),
    );
    assert_eq!(got, json!([4.0, 3.5, null, null, null]));
}

/// The real road: a file's summary is refused by the summarizer's model as
/// over its window; asked again, the file takes half of that window at the
/// rate the refusal named.
#[test]
fn a_summary_asked_again_takes_the_refusals_rate() {
    for (error, limit) in [
        (
            json!({"kind": "input_over_hard", "input_hard": 2000, "chars_per_token": 4.0}),
            4000,
        ),
        (json!({"kind": "input_over_hard", "input_hard": 2000}), 3000),
    ] {
        let row = format!("{}\n", "s".repeat(99));
        let text = row.repeat(200);
        let mut sp = Space::with("/x/files", &[("derive", "embed", json!("0"))]);
        let a = sp.request(
            "in_write",
            "create",
            None,
            json!({"path": "/big.txt", "text": text, "notify": "1"}),
            json!({}),
        );
        assert_eq!(a["ok"], true, "{a}");
        let (cell, req) = sp
            .llm
            .pop_front()
            .unwrap_or_else(|| panic!("a summarizer request; stderr {:?}", sp.stderr));
        assert_eq!(cell, "summarizer");
        let refusal = Msg {
            context: req.context.clone(),
            hop: obj(
                json!({"error_code": "invalid_input", "finish_reason": "error",
                            "model": "stub-model"}),
            ),
            body: obj(json!({"meta": {"error": error}, "messages": []})),
        };
        sp.pump("./summarizer", refusal);
        let (cell, req) = sp
            .llm
            .pop_front()
            .unwrap_or_else(|| panic!("asked again; stderr {:?}", sp.stderr));
        assert_eq!(cell, "summarizer");
        let asked = req.messages()[0]["text"].as_str().unwrap_or("").to_string();
        assert!(
            asked.contains(&format!("...[cut: {limit} of ")),
            "{error}: {}",
            &asked[asked.len().saturating_sub(200)..]
        );
    }
}

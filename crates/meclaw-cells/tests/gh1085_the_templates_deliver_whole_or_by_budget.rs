//! GH #1085 (R-IG-1, R-IG-2) -- the templates outside the curator's path
//! deliver whole, or by the receiving model's window, and say every cut.
//!
//! Before, each of these cells held a number of its own: a file read stopped
//! at 25 000 characters, a summary read 48 000, a section was embedded up to
//! 8000 in silence, a librarian row 1200/4000/1600, a reviewer's verdict 4000,
//! a builder's refusal 600, a kept answer 280, and the firewall refused every
//! turn over 16 000 characters -- whatever the model behind it could read. The
//! locks, per cell:
//!
//! * content over the old number arrives whole when no window is known;
//! * with a window (`input_soft`, tokens) it takes its share of it, at three
//!   characters a token, and the cut carries the one mark with the total;
//! * the firewall refuses (never cuts) a turn over a quarter of the window,
//!   and a turn over the carrier's ceiling, each with its size and the bound;
//! * the model's own text (a summary, a verdict, a refusal it must repair) is
//!   never cut.
//!
//! The scripts are the shipped `params.script_inline` programs; the file
//! space runs as the shipped hive over its own simulated store.

#[path = "support/file_space_hive.rs"]
mod file_space_hive;

use file_space_hive::*;
use meclaw_core::serde_json::{self as sj, Map, Value, json};
use std::io::Write;
use std::process::{Command, Stdio};

// ═══════════════════════════════════════════════════════════════ the runner

fn template_config(cell: &str) -> Value {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates")
        .join(cell)
        .join("config.json");
    let raw = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    sj::from_str(&raw).expect("config json")
}

/// GH #49: `coder-pipeline` and `research-assistant` do not travel with the
/// public tree; a test of theirs returns when the template is not there.
fn shipped(template: &str) -> bool {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates")
        .join(template)
        .is_dir()
}

/// Run the shipped script of `cell` (a path under `templates/`) over one
/// message; the emissions as a list.
fn run(cell: &str, over: Value, context: Value, hop: Value, body: Value) -> Vec<Value> {
    let cfg = template_config(cell);
    let mut params: Map<String, Value> = cfg["params"].as_object().cloned().unwrap_or_default();
    let script = params
        .remove("script_inline")
        .and_then(|v| v.as_str().map(str::to_string))
        .expect("script_inline");
    for (k, v) in over.as_object().cloned().unwrap_or_default() {
        params.insert(k, v);
    }
    let doc = json!({"params": params, "body": body,
                     "envelope": {"header": {"context": context, "hop": hop}}});
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        sj::to_string(&script).unwrap(),
        sj::to_string(&doc.to_string()).unwrap(),
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
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let v: Value = sj::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "{cell}: not JSON ({e}): {stdout}\nstderr: {}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    match v {
        Value::Array(a) => a,
        other => vec![other],
    }
}

// ═══════════════════════════════════════════════════════════ the catalogue

/// The rows of the shipped llm-registry catalogue (the schema line skipped).
fn catalogue() -> Vec<Value> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates/llm-registry/store/seed/models.jsonl");
    std::fs::read_to_string(&p)
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| sj::from_str::<Value>(l).expect("a json row"))
        .filter(|r| r.get("schema").is_none())
        .collect()
}

/// OR-IG-9: the window a producer falls back on -- the `input_soft` of the
/// catalogue row its shipped `<key>_row` param names, the model its reader is
/// born on.
fn named_window(cell: &str, key: &str) -> u64 {
    let cfg = template_config(cell);
    let row = cfg["params"][format!("{key}_row")]
        .as_str()
        .unwrap_or_else(|| panic!("{cell}: params.{key}_row names the row"))
        .to_string();
    catalogue()
        .into_iter()
        .find(|r| r["model_id"] == row.as_str())
        .unwrap_or_else(|| panic!("{cell}: the catalogue has the row {row}"))["input_soft"]
        .as_u64()
        .expect("input_soft")
}

/// OR-IG-9: the row a reader born on a birth token (`ctx.model`,
/// `model_surface`, `MODEL_FILE_SPACE`) is held to -- the llm-registry's
/// `light` tier, the tier of a talky-class or small, inexpensive model.
fn light_tier() -> String {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates/llm-registry/store/seed/tiers.jsonl");
    std::fs::read_to_string(&p)
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
        .lines()
        .filter_map(|l| sj::from_str::<Value>(l).ok())
        .find(|r| r["tier"] == "light" && r["active"] == 1)
        .and_then(|r| r["model_id"].as_str().map(str::to_string))
        .expect("a light tier")
}

/// The window of the embedding row of the model the file space embeds with
/// (`file-space/embed`'s default `model`).
fn embedding_window() -> u64 {
    let model = template_config("file-space/embed")["contract"]["settings"]["model"]["default"]
        .as_str()
        .expect("the embedder's default model")
        .to_string();
    let row = catalogue()
        .into_iter()
        .find(|r| r["model_id"] == model.as_str() && r["wire_dialect"] == "embeddings")
        .unwrap_or_else(|| panic!("the catalogue has an embedding row for {model}"));
    row["input_soft"].as_u64().expect("input_soft")
}

/// `share` of `soft` tokens in characters, as `budget_chars` counts them.
fn chars_of(soft: u64, share: f64) -> usize {
    (soft as f64 * share * 3.0) as usize
}

fn texts(out: &[Value]) -> Vec<String> {
    out.iter()
        .flat_map(|m| m["messages"].as_array().cloned().unwrap_or_default())
        .filter_map(|t| t["text"].as_str().map(str::to_string))
        .collect()
}

/// The text of a message `n` characters long that never repeats a run long
/// enough to look like a mark, and never ends in a blank (two cells fold
/// blanks, so a trailing one would not come back).
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

// ══════════════════════════════════════════════════════ file-space: read

const F: &str = "fh-0a0b0c0d1085";

/// 100 lines of 1000 characters: 100 891 characters in read form, four times
/// the old 25 000.
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

/// One `file_read` of the model, the way the tool edge hands it over: the
/// window on the hop or in the context. The parsed tool_result.
fn file_read(s: &mut Space, context: Value, hop_window: Option<u64>) -> Value {
    let mut hop = json!({"tool_name": "file_read", "tool_call_id": "c1"});
    if let Some(w) = hop_window {
        hop["input_soft"] = json!(w);
    }
    let before = s.out.len();
    s.lane(
        "in_tool",
        context,
        hop,
        json!({"messages": [{"origin": "assistant", "type": "tool_call", "id": "c1",
                             "text": json!({"file": "/a/wide.txt"}).to_string()}]}),
    );
    let res: Vec<Msg> = s.out[before..]
        .iter()
        .filter(|m| m.route() == "tool_result")
        .cloned()
        .collect();
    assert_eq!(
        res.len(),
        1,
        "one tool_result: {res:?}; stderr {:?}",
        s.stderr
    );
    let text = res[0].messages()[0]["text"]
        .as_str()
        .expect("text")
        .to_string();
    sj::from_str(&text).expect("a JSON answer")
}

/// What lines a..b take in read form, `<n>:<h4>|<text>` and a newline between.
fn read_form(a: usize, b: usize) -> usize {
    (a..=b)
        .map(|n| n.to_string().len() + 6 + 1000)
        .sum::<usize>()
        + (b - a)
}

#[test]
fn a_file_over_the_old_25000_is_read_whole_without_a_window() {
    let mut s = wide_space();
    let got = file_read(&mut s, json!({"tool_caller": "cogny"}), None);
    assert_eq!(got["ok"], true, "{got}");
    assert_eq!(got["to"], 100, "every line, not the 24 of before: {got}");
    assert!(
        got.get("more").is_none() && got.get("cut").is_none(),
        "{got}"
    );
    assert_eq!(got["text"].as_str().unwrap().len(), read_form(1, 100));
}

#[test]
fn a_read_takes_a_tenth_of_the_window_and_names_where_the_rest_starts() {
    // 30 000 tokens: a tenth of it at three characters a token is 9000.
    let mut s = wide_space();
    let got = file_read(&mut s, json!({"tool_caller": "cogny"}), Some(30_000));
    assert_eq!(got["to"], 8, "{got}");
    assert_eq!(got["more"]["from"], 9, "{got}");
    let shown = got["text"].as_str().unwrap().len();
    assert_eq!(shown, read_form(1, 8));
    assert_eq!(
        got["cut"],
        format!(
            "...[cut: {shown} of {} chars shown; read with from=9]",
            read_form(1, 100)
        ),
        "{got}"
    );
}

#[test]
fn the_window_in_the_context_sizes_the_read_too() {
    // The curator's field (and its older name) in the persistent context.
    for key in ["input_soft", "recall_input_soft"] {
        let mut s = wide_space();
        let got = file_read(&mut s, json!({"tool_caller": "cogny", key: "30000"}), None);
        assert_eq!(got["to"], 8, "{key}: {got}");
        // Both: the tighter one.
        let mut s = wide_space();
        let got = file_read(
            &mut s,
            json!({"tool_caller": "cogny", key: "60000"}),
            Some(30_000),
        );
        assert_eq!(got["to"], 8, "{key}: the smaller window wins: {got}");
    }
}

/// The one-line form a read shows: `<n>:<h4>|<text>`.
fn read_line_len(n: usize, text_len: usize) -> usize {
    n.to_string().len() + 6 + text_len
}

#[test]
fn a_single_line_over_the_budget_is_cut_with_the_mark() {
    // Before, the first line of a read passed whole whatever its size: one
    // minified line could be any number of times the budget.
    let lines = json!(["w".repeat(20_000), "x"]);
    let got = pure(
        "read",
        "(globals().update(MAX_CHARS=9000), list(window(ARGS, 1, len(ARGS))))[1]",
        lines,
    );
    let text = got[0].as_str().unwrap();
    let total = read_line_len(1, 20_000);
    assert!(text.starts_with("1:"), "{}", &text[..20]);
    assert!(
        text.ends_with(&format!(
            "...[cut: 9000 of {total} chars shown; line 1 goes on past the budget of the window; no read shows more of one line]"
        )),
        "{}",
        &text[text.len() - 120..]
    );
    assert_eq!(
        [&got[1], &got[2], &got[3]],
        [&json!(1), &json!(1), &json!({"from": 2, "to": 2})],
        "line 1 shown (cut), the next read starts at 2"
    );
    // No budget: the line whole.
    let got = pure(
        "read",
        "list(window(ARGS, 1, len(ARGS)))[0]",
        json!(["w".repeat(20_000)]),
    );
    assert_eq!(got.as_str().unwrap().len(), total);
}

#[test]
fn search_reads_long_lines_whole_and_cuts_them_only_at_the_budget() {
    let long = format!("{}needle", "x".repeat(5000));
    // No budget: a needle past the old 4000 is found, its line and its context
    // lines come whole.
    let got = pure(
        "read",
        "list(search_lines(ARGS, 'needle', 'exact', 1, 20))",
        json!(["before", long, "after"]),
    );
    assert_eq!(got[1], 1, "{got}");
    let hit = &got[0][0];
    assert_eq!(hit["text"], long);
    assert!(hit.get("long_line").is_none(), "{hit}");
    let got = pure(
        "read",
        "list(search_lines(ARGS, 'hit', 'exact', 1, 20))",
        json!(["hit", long]),
    );
    let after = got[0][0]["after"][0].as_str().unwrap();
    assert_eq!(after.len(), read_line_len(2, long.len()), "context whole");
    // A budget smaller than the first hit: its parts share it, each cut with
    // the mark -- never a hit past the budget, never a silent cut.
    let got = pure(
        "read",
        "(globals().update(MAX_CHARS=3000), list(search_lines(ARGS, 'needle', 'exact', 1, 20)))[1]",
        json!(["before", long, "after"]),
    );
    let hit = &got[0][0];
    let text = hit["text"].as_str().unwrap();
    assert!(
        text.ends_with(&format!(
            "...[cut: 1000 of {} chars shown; line 2 goes on past the budget of the window; no read shows more of one line]",
            long.len()
        )),
        "{text}"
    );
    assert_eq!(hit["before"], json!([format!("1:{}|before", h4("before"))]));
}

/// Fix review G2 M-6: a regular expression is not run over a line longer
/// than the budget (its backtracking has no bound; a minified line of a few
/// MB ran the cell into its timeout) -- the search is refused with the line
/// and the numbers, never cut. No budget: every line is searched.
#[test]
fn a_regex_over_a_line_past_the_budget_is_refused_with_the_number() {
    let long = "z".repeat(5000);
    let got = pure(
        "read",
        "(globals().update(MAX_CHARS=3000), regex_over_budget(ARGS))[1]",
        json!(["short", long, "short"]),
    );
    assert_eq!(got, json!([2, 5000]));
    let got = pure("read", "regex_over_budget(ARGS)", json!(["short", long]));
    assert_eq!(got, Value::Null, "no budget, no refusal");
    let script = script_of("read");
    assert!(
        script.contains("over = regex_over_budget(lines) if mode == \"regex\" else None")
            && script.contains("fail(S, \"too_long\", \"line %d is %d chars > %d, the budget"),
        "the search lane refuses before it runs the expression"
    );
}

#[test]
fn a_diff_cut_at_the_budget_says_what_it_showed_of_how_much() {
    let got = pure(
        "read",
        "(globals().update(MAX_CHARS=25000), list(unified([], ARGS, 'a', 'b')))[1]",
        json!(vec!["z".repeat(4000); 10]),
    );
    let text = got[0].as_str().unwrap();
    let mark = got[1].as_str().expect("a mark, not a flag");
    let full = pure(
        "read",
        "len(unified([], ARGS, 'a', 'b')[0])",
        json!(vec!["z".repeat(4000); 10]),
    );
    assert_eq!(
        mark,
        format!(
            "...[cut: {} of {full} chars shown; read the two versions for the rest]",
            text.len()
        )
    );
    let got = pure("read", "list(unified([], ARGS, 'a', 'b'))[1]", json!(["z"]));
    assert_eq!(got, Value::Null, "a whole diff carries no mark");
}

// ═════════════════════════════════════════════════════ file-space: derive

#[test]
fn a_summary_reads_the_file_whole_or_half_the_window_with_the_mark() {
    let text = long("# Head\nbody", 60_000);
    let got = pure(
        "derive",
        "[summary_input('/a.md', ARGS[0], [], None), \
          summary_input('/a.md', ARGS[0], [{'section': 'Tail', 'from_line': 3, 'to_line': 3}], 20)]",
        json!([text]),
    );
    assert_eq!(
        got[0],
        format!("Path: /a.md\n\n{text}"),
        "no window: whole, over the old 48 000"
    );
    let cut = got[1].as_str().unwrap();
    assert!(
        cut.ends_with(
            "...[cut: 20 of 60000 chars shown; budget of the window; sections of the rest: Tail]"
        ),
        "{cut}"
    );
}

#[test]
fn a_section_over_the_embedder_bound_is_several_pieces_never_one_cut_one() {
    let sec = json!({"section": "S", "from_line": 10, "to_line": 13,
                     "text": "aaaa\nbbbb\ncccc\ndddddddddddd"});
    let got = pure(
        "derive",
        "[embed_pieces(ARGS, None), embed_pieces(ARGS, 9)]",
        json!([sec]),
    );
    assert_eq!(got[0], json!([sec]), "no bound known: the section whole");
    assert_eq!(
        got[1],
        json!([
            {"section": "S", "from_line": 10, "to_line": 11, "text": "aaaa\nbbbb"},
            {"section": "S", "from_line": 12, "to_line": 12, "text": "cccc"},
            {"section": "S", "from_line": 13, "to_line": 13,
             "text": "ddddddddd...[cut: 9 of 12 chars shown; the rest of line 13 is not embedded]"}
        ])
    );
}

#[test]
fn a_refusal_names_the_window_the_summary_is_asked_again_with() {
    let got = pure(
        "derive",
        "[refused_window(b) for b in ARGS]",
        json!([
            {"meta": {"error": {"kind": "input_over_hard", "input_hard": 32000}}},
            {"meta": {"error": {"kind": "input_over_window", "context_window": 8000}}},
            {"meta": {"error": {"kind": "http_error"}}},
            {}
        ]),
    );
    assert_eq!(got, json!([32000, 8000, null, null]));
}

/// OR-IG-9: the two readers of `./derive` have the windows of the rows they
/// are born on -- the summarizer a birth token (`MODEL_FILE_SPACE`), so the
/// `light` tier; the embedder its default model's embedding row.
#[test]
fn the_derive_fallbacks_are_rows_of_the_catalogue() {
    let cfg = template_config("file-space/derive");
    let settings = &cfg["contract"]["settings"];
    let embed_model =
        template_config("file-space/embed")["contract"]["settings"]["model"]["default"]
            .as_str()
            .expect("the embedder's default model")
            .to_string();
    for (key, row, want, says) in [
        (
            "input_soft_fallback",
            light_tier(),
            named_window("file-space/derive", "input_soft_fallback"),
            "the llm-registry's tier for it is `light`",
        ),
        (
            "embed_input_soft_fallback",
            embed_model,
            embedding_window(),
            "the model `./embed` is born on",
        ),
    ] {
        assert_eq!(
            cfg["params"][format!("{key}_row")],
            row.as_str(),
            "params.{key}_row"
        );
        assert_eq!(cfg["params"][key], want, "params.{key}");
        assert_eq!(settings[key]["default"], want, "settings.{key}");
        assert!(
            settings[key]["description"]
                .as_str()
                .is_some_and(|d| d.contains(says)),
            "settings.{key}: {}",
            settings[key]
        );
    }
    for gone in ["summary_input_soft", "embed_input_tokens"] {
        assert!(
            cfg["params"].get(gone).is_none() && settings.get(gone).is_none(),
            "{gone} is renamed (OR-IG-8)"
        );
    }
}

/// The real road of a summary: a file is created, `./derive` sends it to the
/// summarizer -- no window on any hop, the summarizer has stamped nothing yet.
/// The shipped fallback sizes it: half the catalogue row, cut with the mark.
#[test]
fn a_summary_on_the_real_road_takes_half_the_catalogue_fallback() {
    let limit = chars_of(
        named_window("file-space/derive", "input_soft_fallback"),
        0.5,
    );
    let row = format!("{}\n", "s".repeat(99));
    let text = row.repeat(limit / 100 + 50);
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
        .front()
        .cloned()
        .unwrap_or_else(|| panic!("a summarizer request; stderr {:?}", sp.stderr));
    assert_eq!(cell, "summarizer");
    let asked = req.messages()[0]["text"].as_str().unwrap_or("").to_string();
    let mark = format!("...[cut: {limit} of ");
    assert!(asked.contains(&mark), "{}", &asked[asked.len() - 200..]);
    assert!(
        asked.contains("chars shown; budget of the window"),
        "{}",
        &asked[asked.len() - 200..]
    );
    assert!(asked.len() < limit + 400, "{}", asked.len());
}

/// Z (GH #1085): a write's diff over its page (`diff_lines`) says what it
/// shows of how much and how to get the rest -- the mark `./read`'s diff
/// carries -- beside the old `truncated` flag, which stays (compatible).
#[test]
fn a_write_diff_over_its_page_says_what_it_showed_of_how_much() {
    let mut sp = Space::with("/x/files", &[("derive", "embed", json!("0"))]);
    let old: String = (0..300).map(|i| format!("old line {i}\n")).collect();
    let new: String = (0..300).map(|i| format!("new line {i}\n")).collect();
    let a = sp.request(
        "in_write",
        "create",
        None,
        json!({"path": "/d.txt", "text": old}),
        json!({}),
    );
    assert_eq!(a["ok"], true, "{a}");
    let file = a["file"].as_str().expect("file").to_string();
    let base = a["version"].clone();
    let b = sp.request(
        "in_write",
        "overwrite",
        Some(&file),
        json!({"base": base, "text": new}),
        json!({}),
    );
    assert_eq!(b["truncated"], true, "{b}");
    let diff = b["diff"].as_str().expect("diff");
    assert_eq!(diff.lines().count(), 400, "the page is `diff_lines`");
    let cut = b["cut"].as_str().unwrap_or_else(|| panic!("the mark: {b}"));
    assert!(
        cut.starts_with(&format!("...[cut: {} of ", diff.len())),
        "{cut}"
    );
    assert!(
        cut.ends_with(" chars shown; read the two versions for the rest]"),
        "{cut}"
    );
    // A diff inside its page carries neither.
    let c = sp.request(
        "in_write",
        "overwrite",
        Some(&file),
        json!({"base": b["version"].clone(), "text": "short\n"}),
        json!({}),
    );
    assert!(
        c.get("cut").is_none() && c.get("truncated").is_none(),
        "{c}"
    );
}

/// The real road of an embedding: `./derive` hands `./embed` its sections,
/// each within the embedding row's window as shipped; a line over it is one
/// piece, cut with the mark (here the embedder itself is unreachable, which
/// only fails the job after the sections were sent).
#[test]
fn sections_on_the_real_road_fit_the_embedding_row() {
    let limit = chars_of(embedding_window(), 1.0);
    let long_line = "e".repeat(limit + 1000);
    let text = format!("{long_line}\nshort line\n");
    let mut sp = Space::with(
        "/x/files",
        &[
            ("embed", "endpoint", json!("http://127.0.0.1:9/embeddings")),
            ("embed", "retries", json!(0)),
        ],
    );
    let a = sp.request(
        "in_write",
        "create",
        None,
        json!({"path": "/long.txt", "text": text, "notify": "1"}),
        json!({}),
    );
    assert_eq!(a["ok"], true, "{a}");
    let sent: Vec<String> = sp
        .sent
        .iter()
        .filter(|m| m["to"] == "./embed")
        .flat_map(|m| {
            let t = m["body"]["messages"][0]["text"].as_str().unwrap_or("{}");
            sj::from_str::<Value>(t).unwrap_or_default()["texts"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .filter_map(|t| t.as_str().map(str::to_string))
        .collect();
    assert!(
        !sent.is_empty(),
        "derive sent sections; stderr {:?}",
        sp.stderr
    );
    let cut = format!(
        "{}...[cut: {limit} of {} chars shown; the rest of line 1 is not embedded]",
        "e".repeat(limit),
        limit + 1000
    );
    assert!(
        sent.contains(&cut),
        "{:?}",
        sent.iter().map(|t| t.len()).collect::<Vec<_>>()
    );
    assert!(
        sent.iter().all(|t| t.len() <= cut.len()),
        "{:?}",
        sent.iter().map(|t| t.len()).collect::<Vec<_>>()
    );
}

// ══════════════════════════════════════════════ builder-librarian: retrieve

fn briefing(context: Value) -> String {
    let rows: Vec<Value> = (0..3)
        .map(|i| {
            json!({"id": format!("doc-{i}"), "source": "s", "section": format!("sec{i}"),
                   "kind": "template", "text": long(&format!("row{i}"), 3000)})
        })
        .collect();
    let out = run(
        "builder-librarian/retrieve",
        json!({}),
        context,
        json!({"operation": "search"}),
        json!({"messages": [{"origin": "tool", "type": "tool_result", "id": "lib1",
                             "text": Value::Array(rows).to_string()}]}),
    );
    out[0]["messages"][1]["text"]
        .as_str()
        .expect("the briefing")
        .to_string()
}

#[test]
fn librarian_rows_travel_whole_without_a_window() {
    let b = briefing(json!({"orig_request": "q"}));
    for i in 0..3 {
        assert!(
            b.contains(&long(&format!("row{i}"), 3000)),
            "row {i} whole, over the old 1200/4000: {b}"
        );
    }
    assert!(!b.contains("[cut:") && !b.contains("TRUNCATED"), "{b}");
}

#[test]
fn librarian_rows_fill_a_tenth_of_the_window_then_one_cut_then_dropped() {
    // 20 000 tokens: a tenth of it is 6000 characters.
    let b = briefing(json!({"orig_request": "q", "input_soft": 20000}));
    assert!(b.contains(&long("row0", 3000)), "the best row whole: {b}");
    assert!(
        b.contains("chars shown; budget of the window; ask catalogue_lookup for it by name]"),
        "{b}"
    );
    assert!(b.contains("...[dropped: doc-2 ("), "{b}");
    assert!(
        !b.contains(&long("row1", 3000)),
        "the second row is cut: {b}"
    );
}

// ═══════════════════════════════════════ the model's own text is never cut

#[test]
fn the_coder_and_the_reviewer_hand_each_other_their_whole_text() {
    if !shipped("coder-pipeline") {
        return;
    }
    let summary = long("summary", 10_000);
    let out = run(
        "coder-pipeline/revprep",
        json!({}),
        json!({"task": "t"}),
        json!({}),
        json!({"messages": [{"origin": "assistant", "type": "text", "text": summary}]}),
    );
    assert!(texts(&out)[0].contains(&summary), "over the old 6000");

    let verdict = format!("REFINE {}", long("fix", 6000));
    let out = run(
        "coder-pipeline/revout",
        json!({}),
        json!({"turn_id": "t1"}),
        json!({}),
        json!({"messages": [{"origin": "assistant", "type": "text", "text": verdict}]}),
    );
    assert!(
        texts(&out).iter().any(|t| t.contains(&verdict)),
        "over the old 4000: {out:?}"
    );

    let said = long("no tool call", 3000);
    let out = run(
        "coder-pipeline/dispatch",
        json!({}),
        json!({}),
        json!({}),
        json!({"messages": [{"origin": "assistant", "type": "text", "text": said}]}),
    );
    assert!(texts(&out)[0].contains(&said), "over the old 1500");
}

/// The research planner's window reaches the tools it calls (Z, GH #1085):
/// the planner's hop lasts one emission and `./dispatch` emits anew -- before
/// this, `search` and `fetch` answered whole behind it. Hop first, then the
/// context, then the row the planner is born on (OR-IG-9); 0 hands on none.
#[test]
fn the_research_dispatch_hands_its_window_to_every_tool_call() {
    if !shipped("research-assistant") {
        return;
    }
    let call = |id: &str, name: &str| {
        json!({"origin": "assistant", "type": "tool_call", "id": id,
               "text": json!({"name": name, "arguments": {"query": "q"}}).to_string()})
    };
    let body = json!({"messages": [call("c1", "search"), call("c2", "fetch")]});
    let windows = |over: Value, context: Value, hop: Value| -> Vec<Value> {
        let out = run(
            "research-assistant/dispatch",
            over,
            context,
            hop,
            body.clone(),
        );
        let sends: Vec<Value> = out
            .iter()
            .filter(|m| m["header"]["tool_name"].is_string())
            .map(|m| m["header"]["input_soft"].clone())
            .collect();
        assert_eq!(sends.len(), 2, "one send per call: {out:?}");
        sends
    };
    assert_eq!(
        windows(
            json!({}),
            json!({"input_soft": 40000}),
            json!({"input_soft": 50000})
        ),
        vec![json!(50000), json!(50000)],
        "the hop wins"
    );
    assert_eq!(
        windows(json!({}), json!({"input_soft": 40000}), json!({})),
        vec![json!(40000), json!(40000)],
        "then the context"
    );
    let planner = template_config("research-assistant/planner")["params"]["model"].clone();
    let cfg = template_config("research-assistant/dispatch");
    assert_eq!(
        cfg["params"]["input_soft_fallback_row"], planner,
        "the planner's born row"
    );
    let soft = named_window("research-assistant/dispatch", "input_soft_fallback");
    assert_eq!(
        windows(json!({}), json!({}), json!({})),
        vec![json!(soft), json!(soft)],
        "then the row the planner is born on"
    );
    assert_eq!(
        windows(json!({"input_soft_fallback": 0}), json!({}), json!({})),
        vec![Value::Null, Value::Null],
        "0: no window, no key"
    );
}

/// The coder's window reaches the tools it calls: the llm cell's hop lasts one
/// hop, and `./dispatch` is that hop.
#[test]
fn the_coder_dispatch_hands_its_window_to_every_tool_call() {
    if !shipped("coder-pipeline") {
        return;
    }
    let call = |id: &str, name: &str| {
        json!({"origin": "assistant", "type": "tool_call", "id": id,
               "text": json!({"name": name, "arguments": {"command": "ls"}}).to_string()})
    };
    let body = json!({"messages": [call("c1", "fs"), call("c2", "runner")]});
    let out = run(
        "coder-pipeline/dispatch",
        json!({}),
        json!({}),
        json!({"input_soft": 50000}),
        body.clone(),
    );
    let sends: Vec<&Value> = out
        .iter()
        .filter(|m| m["header"]["tool_name"].is_string())
        .collect();
    assert_eq!(sends.len(), 2, "{out:?}");
    for m in &sends {
        assert_eq!(m["header"]["input_soft"], 50000, "{m}");
    }
    // In the context only, it rides on as well; with none, no key at all.
    let out = run(
        "coder-pipeline/dispatch",
        json!({}),
        json!({"input_soft": 40000}),
        json!({}),
        body.clone(),
    );
    assert!(
        out.iter()
            .filter(|m| m["header"]["tool_name"].is_string())
            .all(|m| m["header"]["input_soft"] == 40000),
        "{out:?}"
    );
    // OR-IG-9: before any stamp the shipped fallback rides on -- the row of
    // the model the coder is born on (its literal), not a hand number.
    let coder = template_config("coder-pipeline/coder")["params"]["model"].clone();
    let cfg = template_config("coder-pipeline/dispatch");
    assert_eq!(
        cfg["params"]["input_soft_fallback_row"], coder,
        "the coder's born row"
    );
    let soft = named_window("coder-pipeline/dispatch", "input_soft_fallback");
    assert_eq!(cfg["params"]["input_soft_fallback"], soft);
    let setting = &cfg["contract"]["settings"]["input_soft_fallback"];
    assert_eq!(setting["default"], soft);
    assert!(
        setting["description"]
            .as_str()
            .is_some_and(|d| d.contains("the model `./coder` is born on")),
        "{setting}"
    );
    let out = run(
        "coder-pipeline/dispatch",
        json!({}),
        json!({}),
        json!({}),
        body.clone(),
    );
    let sends: Vec<&Value> = out
        .iter()
        .filter(|m| m["header"]["tool_name"].is_string())
        .collect();
    assert_eq!(sends.len(), 2, "{out:?}");
    assert!(
        sends.iter().all(|m| m["header"]["input_soft"] == soft),
        "{out:?}"
    );
    // An owner who blanks the fallback (0): no window, no key at all.
    let out = run(
        "coder-pipeline/dispatch",
        json!({"input_soft_fallback": 0}),
        json!({}),
        json!({}),
        body,
    );
    assert!(
        out.iter().all(|m| m["header"].get("input_soft").is_none()),
        "{out:?}"
    );
}

#[test]
fn a_builder_refusal_reaches_the_model_with_its_whole_reason() {
    let reason = long("the manifest names", 2000);
    let refusal = json!({"messages": [{"origin": "tool", "type": "text", "text": reason}]});
    let out = run(
        "tools/build-draft",
        json!({}),
        json!({}),
        json!({"route": "in_build_result", "error_code": "invalid_manifest", "tool_call_id": "c1"}),
        refusal.clone(),
    );
    assert!(texts(&out).iter().any(|t| t.contains(&reason)), "{out:?}");
    let out = run(
        "tools/build-apply",
        json!({}),
        json!({}),
        json!({"route": "in_build_result", "error_code": "invalid_manifest", "tool_call_id": "c1"}),
        refusal.clone(),
    );
    assert!(texts(&out).iter().any(|t| t.contains(&reason)), "{out:?}");
    for repairs in [0, 2] {
        let out = run(
            "builder/weave",
            json!({}),
            json!({"repairs": repairs}),
            json!({"route": "in_receipt", "error_code": "invalid_manifest"}),
            refusal.clone(),
        );
        assert!(
            texts(&out).iter().any(|t| t.contains(&reason)),
            "repairs {repairs}: {out:?}"
        );
    }
}

// ═══════════════════════════════════════════════════════════ the firewall

/// One turn through the shipped screen, its params as shipped (`over` on
/// top) -- the fallback window among them.
fn screen_with(text: &str, over: Value, hop_window: Option<u64>) -> Value {
    let mut hop = json!({"route": "in_turn"});
    if let Some(w) = hop_window {
        hop["input_soft"] = json!(w);
    }
    let out = run(
        "firewall/screen",
        over,
        json!({"channel": "test", "user_id": "u"}),
        hop,
        json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
    );
    assert_eq!(out.len(), 1, "{out:?}");
    out[0]["header"].clone()
}

fn screen(text: &str, hop_window: Option<u64>) -> Value {
    screen_with(text, json!({}), hop_window)
}

/// The road a turn takes in a member: the member's firewall is the shipped
/// template, by reference, and overrides none of its params -- so the
/// fallback the screen ships is the one a running member screens with.
#[test]
fn the_member_screens_with_the_catalogue_fallback() {
    let cfg = template_config("firewall/screen");
    // OR-IG-9: the row of the model talky's brain is born on -- a birth
    // token (`model_surface`), so the `light` tier.
    assert_eq!(
        cfg["params"]["input_soft_fallback_row"],
        light_tier().as_str()
    );
    let soft = named_window("firewall/screen", "input_soft_fallback");
    assert_eq!(cfg["params"]["input_soft_fallback"], soft, "the param");
    let setting = &cfg["contract"]["settings"]["input_soft_fallback"];
    assert_eq!(setting["default"], soft, "the declared default");
    // The prose says which row (development-rules § 2d), the value above is it.
    assert!(
        setting["description"]
            .as_str()
            .is_some_and(|d| d.contains("the model talky's brain is born on")),
        "{setting}"
    );
    let member = template_config("member/firewall");
    assert_eq!(member["cell"]["type"], "ref", "{member}");
    assert!(
        member.get("override_params").is_none() && member.get("params").is_none(),
        "the member keeps the screen's params: {member}"
    );
}

#[test]
fn a_50_kb_turn_comes_through_whole() {
    let turn = "a".repeat(50 * 1024);
    // As shipped, on the member's road: no window on the turn, so the cap is a
    // quarter of the catalogue fallback -- and a 50 KB turn is inside it.
    let h = screen(&turn, None);
    assert_eq!(h["route"], "fwstore", "{h}");
    // 120 000 tokens on the turn: a quarter of it is 90 000 characters.
    assert_eq!(screen(&turn, Some(120_000))["route"], "fwstore");
}

#[test]
fn without_a_window_the_cap_is_a_quarter_of_the_catalogue_fallback() {
    let cap = chars_of(named_window("firewall/screen", "input_soft_fallback"), 0.25);
    // The fallback is in force: one character over its quarter is refused
    // with the size, the cap itself passes.
    assert_eq!(screen(&"a".repeat(cap), None)["route"], "fwstore");
    let h = screen(&"a".repeat(cap + 1), None);
    assert_eq!(h["route"], "reject", "{h}");
    assert_eq!(h["rule_id"], "size-cap");
    assert_eq!(
        h["reject_detail"],
        format!("too_long: {} > {cap} chars", cap + 1)
    );
    // A window on the turn wins over the fallback, a larger one too.
    let wide = named_window("firewall/screen", "input_soft_fallback") * 2;
    assert_eq!(screen(&"a".repeat(cap + 1), Some(wide))["route"], "fwstore");
    // An owner who sets the fallback to 0: no cap below the carrier.
    assert_eq!(
        screen_with(
            &"a".repeat(cap + 1),
            json!({"input_soft_fallback": 0}),
            None
        )["route"],
        "fwstore"
    );
}

#[test]
fn a_turn_over_a_quarter_of_the_window_is_refused_with_its_size() {
    let h = screen(&"a".repeat(100_000), Some(120_000));
    assert_eq!(h["route"], "reject", "{h}");
    assert_eq!(h["reject_reason"], "oversize");
    assert_eq!(h["rule_id"], "size-cap");
    assert_eq!(h["reject_detail"], "too_long: 100000 > 90000 chars");
}

#[test]
fn a_turn_over_the_carrier_ceiling_is_refused_in_bytes() {
    let ceiling = 4096 * 1024;
    // No window at all (the fallback blanked): only the carrier bounds a turn.
    let no_window = json!({"input_soft_fallback": 0});
    let ok = "a".repeat(ceiling);
    assert_eq!(
        screen_with(&ok, no_window.clone(), None)["route"],
        "fwstore",
        "the ceiling itself passes"
    );
    // One two-byte character over it: the carrier counts bytes. The hardline
    // runs before the size cap, so the shipped fallback does not change it.
    let over = format!("{}\u{e4}", "a".repeat(ceiling - 1));
    let h = screen_with(&over, no_window, None);
    assert_eq!(
        screen(&over, None)["rule_id"],
        "hardline:body-ceiling",
        "the hardline comes first"
    );
    assert_eq!(h["rule_id"], "hardline:body-ceiling", "{h}");
    assert_eq!(
        h["reject_detail"],
        format!("too_long: {} > {ceiling} bytes", ceiling + 1)
    );
}

// ══════════════════════════════════════════════ the shelf, the digest, the fetcher

#[test]
fn a_kept_answer_is_read_whole() {
    if !shipped("research-assistant") {
        return;
    }
    let answer = long("the answer", 1000);
    let rows = json!([{"id": "a-1", "question_id": "q-1", "question": "Q", "answer": answer,
                       "at": 1_759_000_000_000i64, "audience_set": "[\"member:p\"]"}]);
    let out = run(
        "research-assistant/shelf",
        json!({}),
        json!({"shelf_op_id": "o1", "audience_set": "[\"member:p\"]"}),
        json!({"route": "shelf_store"}),
        json!({"messages": [{"origin": "tool", "type": "tool_result", "text": rows.to_string()}]}),
    );
    assert_eq!(
        out[0]["answers"][0]["answer"], answer,
        "over the old 280: {out:?}"
    );
}

#[test]
fn a_digest_is_whole_up_to_one_telegram_message_and_marks_the_rest() {
    let page = long("page", 2000);
    let out = run(
        "daily-digest/format",
        json!({}),
        json!({"chat_id": "1"}),
        json!({}),
        json!({"messages": [{"origin": "tool", "type": "tool_result", "text": page}]}),
    );
    assert_eq!(texts(&out)[0], format!("Daily digest:\n{page}"));

    let page = long("page", 10_000);
    let out = run(
        "daily-digest/format",
        json!({}),
        json!({"chat_id": "1"}),
        json!({}),
        json!({"messages": [{"origin": "tool", "type": "tool_result", "text": page}]}),
    );
    let t = &texts(&out)[0];
    assert!(
        t.chars().count() <= 4096,
        "one Telegram message: {}",
        t.len()
    );
    assert!(t.chars().count() > 4000, "the room is used: {}", t.len());
    assert!(
        t.ends_with(" of 10000 chars shown; the page itself has the rest]"),
        "{t}"
    );
}

#[test]
fn the_fetcher_sets_no_number_of_its_own() {
    let cfg = template_config("fetcher");
    assert!(
        cfg["params"].get("max_bytes").is_none(),
        "the cell's window decides, not the template: {}",
        cfg["params"]
    );
}

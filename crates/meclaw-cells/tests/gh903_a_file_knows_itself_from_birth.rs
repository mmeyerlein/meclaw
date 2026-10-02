//! GH #903 -- a file knows itself from its birth: after `create` and after
//! every head move of the main line the file space holds a one-line and a
//! short summary and one embedding per section of the new version, and
//! `files.oneline` is the new one line; a working edit gets none of it and
//! wakes nothing; `search` with mode `semantic` and `ask` answer out of the
//! embeddings of the one file and version asked about, and `ask` hands the
//! summarizer the chosen sections, never the whole file.
//!
//! The seam lock of strand E (README § 3 of wave File Hive B1): the shipped
//! space in one process (`support/file_space_hive.rs`) -- every script is the
//! shipped one, every edge the shipped one under the colony's CEL, the store
//! the store cell's own dispatcher. The summarizer is the harness's `llm`
//! recorder; `./embed` is the shipped code cell, speaking real HTTP to a stub
//! endpoint on 127.0.0.1 that this file runs (no paid provider, no local
//! model). The commit case of a workspace stands in the V lock, so V and E
//! merge in either order.

#[path = "support/file_space_hive.rs"]
mod file_space_hive;

use file_space_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

const DIM: usize = 64;
/// The stub gives every text that names one of these words the SAME vector,
/// so a question about a topic finds that topic's section at distance 0.
const TOPICS: [&str; 8] = [
    "budget", "team", "risk", "timeline", "vendor", "travel", "hardware", "legal",
];

fn e_shipped() -> bool {
    repo("templates/file-space/embed/config.json").is_file()
        && repo("templates/file-space/summarizer/config.json").is_file()
}

/// An OpenAI-compatible embeddings endpoint on 127.0.0.1: one vector of
/// `dimensions` signs per input, a pure function of the input's topic. Every
/// request's inputs are recorded.
struct Stub {
    url: String,
    seen: Arc<Mutex<Vec<Vec<String>>>>,
}

fn vector(text: &str, dim: usize) -> Vec<f64> {
    let low = text.to_lowercase();
    let seed = TOPICS
        .iter()
        .find(|t| low.contains(*t))
        .map(|t| t.to_string())
        .unwrap_or(low);
    let mut bits = String::new();
    let mut n = 0;
    while bits.len() * 4 < dim {
        bits.push_str(&sha256_hex(format!("{seed}/{n}").as_bytes()));
        n += 1;
    }
    (0..dim)
        .map(|i| {
            let nib = u8::from_str_radix(&bits[i / 4..i / 4 + 1], 16).unwrap();
            if nib >> (3 - (i % 4)) & 1 == 1 {
                1.0
            } else {
                -1.0
            }
        })
        .collect()
}

fn stub() -> Stub {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the stub");
    let url = format!("http://{}/v1/embeddings", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut conn) = conn else { continue };
            let mut reader = BufReader::new(conn.try_clone().unwrap());
            let mut len = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                let low = line.to_ascii_lowercase();
                if let Some(v) = low.strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; len];
            reader.read_exact(&mut body).unwrap();
            let req: Value = sj::from_slice(&body).unwrap_or(Value::Null);
            let dim = req["dimensions"].as_u64().unwrap_or(DIM as u64) as usize;
            let inputs: Vec<String> = req["input"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|t| t.as_str().unwrap_or("").to_string())
                .collect();
            let data: Vec<Value> = inputs
                .iter()
                .enumerate()
                .map(|(i, t)| json!({"index": i, "embedding": vector(t, dim)}))
                .collect();
            log.lock().unwrap().push(inputs);
            let out = json!({"data": data, "usage": {"prompt_tokens": 3}}).to_string();
            let _ = write!(
                conn,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{out}",
                out.len()
            );
        }
    });
    Stub { url, seen }
}

fn calls(s: &Stub) -> usize {
    s.seen.lock().unwrap().len()
}

/// The shipped space with `./embed` pointed at the stub (params, the way an
/// instance resolves `MEMORY_EMBED_*`), plus `over`.
fn space(s: &Stub, over: &[(&str, &str, Value)]) -> Space {
    let mut all: Vec<(&str, &str, Value)> = vec![
        ("embed", "endpoint", json!(s.url)),
        ("embed", "model", json!("stub-embed")),
        ("embed", "dim", json!(DIM.to_string())),
    ];
    all.extend(over.iter().cloned());
    Space::with("/x/files", &all)
}

/// Eight sections, one topic each, far apart in the stub's space.
fn plan_text() -> String {
    let mut t = String::from("# Plan\nThe plan of the file hive.\n");
    let said = [
        ("Budget", "The budget for 2027 is forty thousand euro."),
        ("Team", "Three people build the team hive."),
        ("Risk", "The largest risk is the store."),
        ("Timeline", "The timeline ends in December."),
        ("Vendor", "No vendor is needed."),
        ("Travel", "Travel happens twice."),
        ("Hardware", "The hardware stays as it is."),
        ("Legal", "Legal reviews the licence."),
    ];
    for (h, s) in said {
        t.push_str(&format!("\n## {h}\n{s}\n"));
    }
    t
}

fn col(sp: &Space, sql: &str) -> Vec<String> {
    sp.rows(sql)
        .into_iter()
        .map(|r| {
            r[0].as_str()
                .map(str::to_string)
                .unwrap_or_else(|| r[0].to_string())
        })
        .collect()
}

fn count(sp: &Space, sql: &str) -> i64 {
    sp.rows(sql)[0][0].as_i64().unwrap()
}

/// `create` from outside; returns (file id, full version). OR-FH.E.5: the
/// argument names of `create` are W's (`path`, `text`, `notify`).
fn create(sp: &mut Space, path: &str, text: &str, notify: bool) -> (String, String) {
    let mut args = json!({"path": path, "text": text});
    if notify {
        args["notify"] = json!("1");
    }
    let a = sp.request("in_write", "create", None, args, json!({}));
    assert_eq!(a["ok"], json!(true), "create: {a}");
    let file = a["file"]
        .as_str()
        .expect("the answer names the file")
        .to_string();
    let head = col(sp, &format!("SELECT head FROM files WHERE file = '{file}'"));
    (file, head[0].clone())
}

/// The summarizer's request is the oldest the recorder holds, and it is ours.
fn summarize(sp: &mut Space, text: &str, finish: &str) -> Msg {
    assert_eq!(
        sp.llm.front().map(|(c, _)| c.as_str()),
        Some("summarizer"),
        "the summarizer holds a request; stderr {:?}",
        sp.stderr
    );
    sp.llm_answer(text, finish)
}

/// R-FH-1 second condition at the seam: every store call of the run named
/// its file (the exemptions are S's) and every select its limit.
fn assert_scoped(sp: &Space) {
    let breaches = sp.unscoped();
    assert!(breaches.is_empty(), "{breaches:#?}");
}

/// `in_derive` as `./write` sends it after a swing of the main line (W's
/// `derive_msg`): the body `{file, version}`, `notify`/`caller` on the hop.
fn in_derive(sp: &mut Space, file: &str, version: &str, notify: bool, caller: &str) {
    in_derive_hop(sp, file, version, notify, caller, json!({}));
}

/// `in_derive` with `extra` on the hop (e.g. a `ws` a sender might carry).
fn in_derive_hop(
    sp: &mut Space,
    file: &str,
    version: &str,
    notify: bool,
    caller: &str,
    extra: Value,
) {
    let mut hop = json!({"route": "in_derive", "notify": if notify { "1" } else { "" },
                         "caller": caller, "op_id": "w-test"});
    for (k, v) in obj(extra) {
        hop[k] = v;
    }
    let msg = Msg {
        context: obj(json!({})),
        hop: obj(hop),
        body: obj(json!({"file": file, "version": version})),
    };
    sp.pump("./write", msg);
}

fn head_of(sp: &Space, file: &str) -> String {
    col(sp, &format!("SELECT head FROM files WHERE file = '{file}'"))[0].clone()
}

/// A head move of the main line: `replace` with `base`.
fn swing(sp: &mut Space, file: &str, old: &str, new: &str) -> String {
    let base = head_of(sp, file);
    let a = sp.request(
        "in_write",
        "replace",
        Some(file),
        json!({"old": old, "new": new, "expected": 1, "base": &base[..12]}),
        json!({}),
    );
    assert_eq!(a["ok"], json!(true), "{a}");
    head_of(sp, file)
}

fn user_text(m: &Msg) -> String {
    m.messages()
        .iter()
        .filter_map(|x| x["text"].as_str().map(str::to_string))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_new_file_has_its_summaries_and_embeddings_and_says_so_once() {
    if !shipped() || !e_shipped() {
        return;
    }
    let s = stub();
    let mut sp = space(&s, &[]);
    let (file, v1) = create(&mut sp, "/plan.md", &plan_text(), true);

    // Embeddings come first (the plan's order), the summary second.
    assert_eq!(calls(&s), 1, "one embedding request for the sections");
    let sections = s.seen.lock().unwrap()[0].clone();
    assert_eq!(
        sections.len(),
        9,
        "the title part and eight headings: {sections:?}"
    );
    let req = summarize(
        &mut sp,
        "The plan of the file hive.\n\nBudget, team, risk and the rest of the plan.",
        "stop",
    );
    assert!(
        user_text(&req).contains("forty thousand euro"),
        "the summary reads the file"
    );

    let rows = sp.rows(&format!(
        "SELECT level, text, model FROM summaries WHERE file = '{file}' AND version = '{v1}' \
         ORDER BY level"
    ));
    assert_eq!(
        rows,
        vec![
            vec![
                json!("oneline"),
                json!("The plan of the file hive."),
                json!("stub-model")
            ],
            vec![
                json!("short"),
                json!("Budget, team, risk and the rest of the plan."),
                json!("stub-model")
            ],
            // GH #947: the version's topics ride the summary, an empty list
            // when the answer has no `TAGS:` line.
            vec![json!("tags"), json!("[]"), json!("stub-model")],
        ]
    );
    let emb = sp.rows(&format!(
        "SELECT section, from_line, to_line, dim, model, length(blob) FROM embeddings \
         WHERE file = '{file}' AND version = '{v1}' ORDER BY from_line"
    ));
    assert_eq!(emb.len(), 9);
    assert_eq!(emb[1][0], json!("Budget"));
    assert_eq!((emb[1][1].clone(), emb[1][2].clone()), (json!(4), json!(6)));
    assert_eq!(emb[1][3], json!(DIM));
    assert_eq!(emb[1][4], json!("stub-embed"));
    assert_eq!(
        col(
            &sp,
            &format!("SELECT oneline FROM files WHERE file = '{file}'")
        ),
        vec!["The plan of the file hive."]
    );
    // `notify` from outside: exactly one `derived` at the parent.
    let derived = sp.routed("derived");
    assert_eq!(derived.len(), 1, "{derived:?}");
    assert_eq!(derived[0].body["ok"], json!(true));
    assert_eq!(derived[0].body["file"], json!(file));
    assert_eq!(derived[0].body["version"], json!(&v1[..12]));
    assert_eq!(
        derived[0].body["oneline"],
        json!("The plan of the file hive.")
    );
    assert!(sp.store_errors.is_empty(), "{:?}", sp.store_errors);
    assert_scoped(&sp);
}

#[test]
fn a_head_move_derives_again_and_a_working_edit_wakes_nothing() {
    if !shipped() || !e_shipped() {
        return;
    }
    let s = stub();
    let mut sp = space(&s, &[]);
    let (file, v1) = create(&mut sp, "/plan.md", &plan_text(), false);
    summarize(&mut sp, "First.\n\nThe first plan.", "stop");
    assert!(sp.routed("derived").is_empty(), "no notify, no derived");

    // A head move of the main line (a write with `base`). OR-FH.E.5: the
    // argument names of `replace` are W's.
    let a = sp.request(
        "in_write",
        "replace",
        Some(&file),
        json!({"old": "forty thousand", "new": "fifty thousand", "expected": 1,
               "base": &v1[..12]}),
        json!({}),
    );
    assert_eq!(a["ok"], json!(true), "{a}");
    let v2 = col(
        &sp,
        &format!("SELECT head FROM files WHERE file = '{file}'"),
    )[0]
    .clone();
    assert_ne!(v1, v2);
    assert_eq!(calls(&s), 2, "the new version is embedded");
    summarize(&mut sp, "Second.\n\nThe second plan.", "stop");
    // Summaries: one line, short, tags (GH #947).
    for (table, n) in [("summaries", 3), ("embeddings", 9)] {
        assert_eq!(
            count(
                &sp,
                &format!("SELECT COUNT(*) FROM {table} WHERE file = '{file}' AND version = '{v2}'")
            ),
            n,
            "{table} of the new version"
        );
    }
    assert_eq!(
        col(
            &sp,
            &format!("SELECT oneline FROM files WHERE file = '{file}'")
        ),
        vec!["Second."]
    );

    // A working edit: a workspace sown on the main line, a write with `ws`.
    let before = (
        count(&sp, "SELECT COUNT(*) FROM summaries"),
        count(&sp, "SELECT COUNT(*) FROM embeddings"),
    );
    let seq = sp.rows(&format!("SELECT head_seq FROM files WHERE file = '{file}'"))[0][0]
        .as_i64()
        .unwrap();
    sp.seed_ws("ws-e", "draft", "/", seq);
    // OR-FH-81: `hop.ws` names the workspace by its name.
    let a = sp.request(
        "in_write",
        "replace",
        Some(&file),
        json!({"old": "twice", "new": "three times", "expected": 1, "base": &v2[..12]}),
        json!({"ws": "draft"}),
    );
    assert_eq!(a["ok"], json!(true), "{a}");
    assert_eq!(
        (
            count(&sp, "SELECT COUNT(*) FROM summaries"),
            count(&sp, "SELECT COUNT(*) FROM embeddings")
        ),
        before,
        "a working version gets no summary and no embedding"
    );
    assert_eq!(calls(&s), 2, "no request at the embedding stub");
    assert!(sp.llm.is_empty(), "no request at the summarizer");
    assert_scoped(&sp);
}

#[test]
fn semantic_search_finds_the_section_in_this_file_only() {
    if !shipped() || !e_shipped() {
        return;
    }
    let s = stub();
    let mut sp = space(&s, &[]);
    let (file, v1) = create(&mut sp, "/plan.md", &plan_text(), false);
    summarize(&mut sp, "Plan.\n\nThe plan.", "stop");
    // A second file with the same budget section: its rows must never answer
    // for the first (R-FH-1, second condition).
    let (other, _) = create(
        &mut sp,
        "/other.md",
        "## Budget\nThe budget is elsewhere.\n",
        false,
    );
    summarize(&mut sp, "Other.\n\nAnother file.", "stop");
    assert_ne!(file, other);

    let a = sp.request(
        "in_read",
        "search",
        Some(&file),
        json!({"pattern": "how much money is in the budget", "mode": "semantic", "limit": 3}),
        json!({"mode": "semantic"}),
    );
    assert_eq!(a["ok"], json!(true), "{a}");
    assert_eq!(a["file"], json!(file));
    assert_eq!(a["version"], json!(&v1[..12]));
    let hits = a["hits"].as_array().expect("hits");
    assert_eq!(hits.len(), 3, "the limit holds");
    assert_eq!(hits[0]["section"], json!("Budget"));
    assert_eq!(
        (hits[0]["from_line"].clone(), hits[0]["to_line"].clone()),
        (json!(4), json!(6))
    );
    assert_eq!(hits[0]["score"], json!(1.0));
    assert_eq!(
        hits[0]["preview"],
        json!([
            format!("4:{}|## Budget", h4("## Budget")),
            format!(
                "5:{}|The budget for 2027 is forty thousand euro.",
                h4("The budget for 2027 is forty thousand euro.")
            ),
            format!("6:{}|", h4(""))
        ])
    );
    let sim: Vec<&Value> = sp
        .store_ops
        .iter()
        .filter(|(_, op)| op["operation"] == "similar")
        .map(|(_, op)| op)
        .collect();
    assert_eq!(sim.len(), 1);
    assert_eq!(
        sim[0]["where"]["file"],
        json!(file),
        "similar ranks one file"
    );
    assert_eq!(sim[0]["where"]["version"], json!(v1), "and one version");

    // Exactly one of the two `in_read` doors takes a request: `./read` the
    // exact search, `./derive` the semantic one.
    let a = sp.request(
        "in_read",
        "search",
        Some(&file),
        json!({"pattern": "forty"}),
        json!({}),
    );
    assert_eq!(a["ok"], json!(true), "{a}");
    assert!(
        a.get("hits").is_none() || a["mode"] != json!("semantic"),
        "{a}"
    );
    assert_scoped(&sp);
}

#[test]
fn ask_hands_the_summarizer_the_chosen_lines_and_answers_with_sources() {
    if !shipped() || !e_shipped() {
        return;
    }
    let s = stub();
    let mut sp = space(&s, &[]);
    let (file, v1) = create(&mut sp, "/plan.md", &plan_text(), false);
    summarize(
        &mut sp,
        "Plan.\n\nThe plan of the file hive, section by section.",
        "stop",
    );

    let before = sp.out.len();
    sp.lane(
        "in_read",
        json!({}),
        json!({"op": "ask", "op_id": "ask-1"}),
        json!({"op": "ask", "file": "/plan.md", "args": {"question": "How large is the budget?"}}),
    );
    assert_eq!(sp.out.len(), before, "no answer before the model spoke");
    let req = summarize(&mut sp, "Forty thousand euro.\nSOURCES: 4-6", "stop");
    let sent = user_text(&req);
    assert!(
        sent.contains("forty thousand euro"),
        "the budget lines go out: {sent}"
    );
    assert!(
        sent.contains("The plan of the file hive, section by section."),
        "the summary too"
    );
    let missing = [
        "Three people",
        "largest risk",
        "ends in December",
        "No vendor",
        "happens twice",
        "stays as it is",
        "reviews the licence",
    ]
    .iter()
    .filter(|t| !sent.contains(*t))
    .count();
    assert!(missing >= 3, "the sections, never the whole file: {sent}");
    let brief = req.body["system"]["instructions"]["text"]
        .as_str()
        .unwrap_or("");
    assert!(
        brief.contains("Answer only from these lines; cite line ranges"),
        "the fixed brief: {brief}"
    );

    let answers: Vec<&Msg> = sp.out[before..]
        .iter()
        .filter(|m| m.route() == "answer")
        .collect();
    assert_eq!(answers.len(), 1);
    let a = &answers[0].body;
    assert_eq!(answers[0].hop["op_id"], json!("ask-1"));
    assert_eq!(a["ok"], json!(true), "{a:?}");
    assert_eq!(a["file"], json!(file));
    assert_eq!(a["version"], json!(&v1[..12]));
    assert_eq!(a["answer"], json!("Forty thousand euro."));
    assert_eq!(a["sources"], json!([{"from_line": 4, "to_line": 6}]));
    assert!(
        count(&sp, "SELECT COUNT(*) FROM pending WHERE op_id LIKE 'r-%'") == 0,
        "the ask left no parked row"
    );
    assert_scoped(&sp);
}

#[test]
fn without_embeddings_the_answer_is_not_indexed() {
    if !shipped() || !e_shipped() {
        return;
    }
    let s = stub();
    let mut sp = space(&s, &[("derive", "embed", json!("0"))]);
    let (file, v1) = create(&mut sp, "/plan.md", &plan_text(), false);
    summarize(&mut sp, "Plan.\n\nThe plan.", "stop");
    assert_eq!(calls(&s), 0, "`embed` \"0\" embeds nothing");
    for (op, args, hop) in [
        (
            "search",
            json!({"pattern": "budget", "mode": "semantic"}),
            json!({"mode": "semantic"}),
        ),
        ("ask", json!({"question": "budget?"}), json!({})),
    ] {
        let a = sp.request("in_read", op, Some(&file), args, hop);
        assert_eq!(a["ok"], json!(false), "{a}");
        assert_eq!(a["error"]["code"], json!("not_indexed"), "{a}");
        // OR-FH-83: a refusal has no top-level `version`; it is `current`.
        assert_eq!(a["error"]["current"], json!(&v1[..12]), "{a}");
        assert!(a.get("version").is_none(), "{a}");
    }
    assert!(sp.llm.is_empty(), "no model for an unindexed file");
    assert_scoped(&sp);
}

#[test]
fn a_failed_summary_writes_nothing_half() {
    if !shipped() || !e_shipped() {
        return;
    }
    let s = stub();
    let mut sp = space(&s, &[]);
    let (file, _) = create(&mut sp, "/plan.md", &plan_text(), true);
    summarize(&mut sp, "", "error");
    assert_eq!(
        count(
            &sp,
            &format!("SELECT COUNT(*) FROM summaries WHERE file = '{file}'")
        ),
        0
    );
    assert_eq!(
        count(
            &sp,
            &format!("SELECT COUNT(*) FROM embeddings WHERE file = '{file}'")
        ),
        0,
        "the embeddings of a job that failed are not written either"
    );
    let derived = sp.routed("derived");
    assert_eq!(derived.len(), 1);
    assert_eq!(derived[0].body["ok"], json!(false));
    assert!(
        sp.stderr.iter().any(|e| e.contains("summary of")),
        "{:?}",
        sp.stderr
    );
    assert_eq!(
        count(&sp, "SELECT COUNT(*) FROM pending WHERE op_id LIKE 'd-%'"),
        0,
        "the failed job left no parked row"
    );
    assert_scoped(&sp);
}

#[test]
fn an_older_version_that_arrives_late_writes_nothing() {
    if !shipped() || !e_shipped() {
        return;
    }
    // OR-FH.E.7: the version comparison runs over `line`, never over the
    // order of arrival.
    let s = stub();
    let mut sp = space(&s, &[]);
    let (file, v1) = create(&mut sp, "/plan.md", &plan_text(), false);
    summarize(&mut sp, "First.\n\nThe first plan.", "stop");
    let v2 = swing(&mut sp, &file, "forty thousand", "fifty thousand");
    summarize(&mut sp, "Second.\n\nThe second plan.", "stop");
    let rows_of = |sp: &Space| {
        (
            count(sp, "SELECT COUNT(*) FROM summaries"),
            count(sp, "SELECT COUNT(*) FROM embeddings"),
        )
    };
    let before = (rows_of(&sp), calls(&s));

    in_derive(&mut sp, &file, &v1, true, "");
    assert_eq!(
        (rows_of(&sp), calls(&s)),
        before,
        "v1 after v2: no row, no request"
    );
    assert!(sp.llm.is_empty(), "no request at the summarizer");
    assert_eq!(head_of(&sp, &file), v2);
    assert_eq!(
        col(
            &sp,
            &format!("SELECT oneline FROM files WHERE file = '{file}'")
        ),
        vec!["Second."]
    );
    let derived = sp.routed("derived");
    assert_eq!(derived.len(), 1, "{derived:?}");
    assert_eq!(
        derived[0].body["ok"],
        json!(false),
        "the late job says it wrote nothing"
    );
    assert_eq!(
        count(&sp, "SELECT COUNT(*) FROM pending WHERE op_id LIKE 'd-%'"),
        0
    );
    assert_scoped(&sp);
}

#[test]
fn without_summary_on_commit_a_head_move_gets_embeddings_only() {
    if !shipped() || !e_shipped() {
        return;
    }
    let s = stub();
    let mut sp = space(&s, &[("derive", "summary_on_commit", json!("0"))]);
    let (file, _) = create(&mut sp, "/plan.md", &plan_text(), false);
    // The birth gets its summary anyway (R-27-4 point 6).
    summarize(&mut sp, "Born.\n\nThe plan at birth.", "stop");
    let v2 = swing(&mut sp, &file, "forty thousand", "fifty thousand");
    assert!(sp.llm.is_empty(), "no summary for a head move");
    assert_eq!(calls(&s), 2, "but the new version is embedded");
    assert_eq!(
        count(
            &sp,
            &format!("SELECT COUNT(*) FROM embeddings WHERE file = '{file}' AND version = '{v2}'")
        ),
        9
    );
    assert_eq!(
        count(
            &sp,
            &format!("SELECT COUNT(*) FROM summaries WHERE file = '{file}' AND version = '{v2}'")
        ),
        0
    );
    // OR-FH-72: `write` sets the first line only at `create`, so the birth's
    // one line stays the file's one line.
    assert_eq!(
        col(
            &sp,
            &format!("SELECT oneline FROM files WHERE file = '{file}'")
        ),
        vec!["Born."]
    );
    assert_scoped(&sp);
}

#[test]
fn a_failed_embedding_writes_nothing_and_wakes_no_model() {
    if !shipped() || !e_shipped() {
        return;
    }
    let s = stub();
    let mut sp = Space::with(
        "/x/files",
        &[
            (
                "embed",
                "endpoint",
                json!("http://127.0.0.1:9/v1/embeddings"),
            ),
            ("embed", "model", json!("stub-embed")),
            ("embed", "dim", json!(DIM.to_string())),
            ("embed", "retries", json!(0)),
        ],
    );
    let (file, _) = create(&mut sp, "/plan.md", &plan_text(), true);
    assert!(
        sp.llm.is_empty(),
        "OR-FH.E.8: no summary after a failed embedding"
    );
    for table in ["summaries", "embeddings"] {
        assert_eq!(
            count(
                &sp,
                &format!("SELECT COUNT(*) FROM {table} WHERE file = '{file}'")
            ),
            0,
            "{table}"
        );
    }
    let derived = sp.routed("derived");
    assert_eq!(derived.len(), 1, "{derived:?}");
    assert_eq!(derived[0].body["ok"], json!(false));
    assert!(
        sp.stderr.iter().any(|e| e.contains("embedding")),
        "{:?}",
        sp.stderr
    );
    assert_eq!(
        count(&sp, "SELECT COUNT(*) FROM pending WHERE op_id LIKE 'd-%'"),
        0
    );
    assert_eq!(calls(&s), 0);
    assert_scoped(&sp);
}

#[test]
fn a_derived_for_a_caller_never_leaves_the_hive() {
    if !shipped() || !e_shipped() {
        return;
    }
    // The edge `./derive -> .` lets `derived` out only with an empty
    // `caller`; a B2 caller draws its own edge (I: `caller == 'ingest'`).
    let s = stub();
    let mut sp = space(&s, &[]);
    let (file, v1) = create(&mut sp, "/plan.md", &plan_text(), false);
    summarize(&mut sp, "Plan.\n\nThe plan.", "stop");
    let v2 = swing(&mut sp, &file, "forty thousand", "fifty thousand");
    summarize(&mut sp, "Plan two.\n\nThe plan again.", "stop");
    assert_ne!(v1, v2);
    // A fresh job of the head, asked by a caller inside the member.
    in_derive(&mut sp, &file, &v2, true, "ingest");
    summarize(&mut sp, "Plan two.\n\nThe plan again.", "stop");
    assert!(
        sp.routed("derived").is_empty(),
        "{:?}",
        sp.routed("derived")
    );
    assert_scoped(&sp);
}

#[test]
fn the_two_in_read_doors_never_both_open() {
    if !shipped() || !e_shipped() {
        return;
    }
    let hive = hive_config();
    let doors: Vec<(String, String)> = hive["params"]["graph"]["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["from"] == "." && (e["to"] == "./read" || e["to"] == "./derive"))
        .map(|e| {
            (
                e["to"].as_str().unwrap().to_string(),
                e["condition"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(doors.len(), 2, "{doors:?}");
    let ops = ["read", "search", "ask", "info", "summary", "history", ""];
    let modes = [
        None,
        Some("exact"),
        Some("regex"),
        Some("semantic"),
        Some(""),
    ];
    for op in ops {
        for mode in modes {
            let mut hop = json!({"route": "in_read", "op_id": "x"});
            if !op.is_empty() {
                hop["op"] = json!(op);
            }
            if let Some(m) = mode {
                hop["mode"] = json!(m);
            }
            let hop = obj(hop);
            let open: Vec<&String> = doors
                .iter()
                .filter(|(_, c)| {
                    let c = meclaw_colony::cel_eval::parse_condition(c).unwrap();
                    matches!(
                        meclaw_colony::cel_eval::evaluate_condition(&c, &Default::default(), &hop),
                        Ok(true)
                    )
                })
                .map(|(to, _)| to)
                .collect();
            let semantic = op == "ask" || (op == "search" && mode == Some("semantic"));
            let want = if semantic { "./derive" } else { "./read" };
            assert_eq!(open, vec![want], "op {op:?} mode {mode:?}");
        }
    }
}

#[test]
fn embed_answers_in_input_order_and_never_stays_silent() {
    if !e_shipped() {
        return;
    }
    let s = stub();
    let mut params = cell_config("embed")["params"].as_object().cloned().unwrap();
    params.remove("script_inline");
    let run = |params: &sj::Map<String, Value>, texts: Vec<String>| -> Value {
        let doc = json!({"envelope": {"header": {"context": {}, "hop": {}}},
                         "params": params,
                         "body": {"messages": [{"origin": "assistant", "type": "tool_call",
                                                "id": "e", "text": json!({"texts": texts}).to_string()}]}});
        let out = run_python(&script_of("embed"), &doc.to_string());
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let msgs: Value = sj::from_slice(&out.stdout).unwrap();
        assert_eq!(msgs.as_array().unwrap().len(), 1, "one answer: {msgs}");
        assert_eq!(msgs[0]["header"]["route"], json!("embedded"));
        sj::from_str(msgs[0]["messages"][0]["text"].as_str().unwrap()).unwrap()
    };
    params.insert("endpoint".into(), json!(s.url));
    params.insert("dim".into(), json!("16"));
    // 70 texts go out in two batches (64 + 6) and come back as one list.
    let texts: Vec<String> = (0..70)
        .map(|i| {
            if i == 69 {
                "the budget".to_string()
            } else {
                format!("text {i}")
            }
        })
        .collect();
    let got = run(&params, texts);
    assert_eq!(calls(&s), 2);
    assert_eq!(s.seen.lock().unwrap()[1].len(), 6);
    let v = got["vectors"].as_array().unwrap();
    assert_eq!(v.len(), 70);
    assert_eq!(got["dim"], json!(16));
    assert_eq!(
        v[69].as_str().unwrap().len(),
        4,
        "16 bits are two bytes, four base64 chars"
    );
    // The memory hive's binarisation, byte for byte: a query packed any other
    // way would make hamming() measure noise.
    let packed = pure("embed", "binarize(ARGS)", json!(vector("the budget", 16)));
    assert_eq!(v[69], packed);
    // A dead endpoint: `vectors: null` and the error -- never silence.
    params.insert("endpoint".into(), json!("http://127.0.0.1:9/v1/embeddings"));
    params.insert("retries".into(), json!(0));
    let got = run(&params, vec!["a".into()]);
    assert_eq!(got["vectors"], Value::Null);
    assert!(
        got["error"].as_str().unwrap().contains("unreachable"),
        "{got}"
    );
}

fn oneline_of(sp: &Space, file: &str) -> String {
    col(
        sp,
        &format!("SELECT oneline FROM files WHERE file = '{file}'"),
    )[0]
    .clone()
}

#[test]
fn a_slow_older_job_never_lays_its_one_line_over_a_newer_head() {
    if !shipped() || !e_shipped() {
        return;
    }
    // OR-FH-72 (review E I-1a): `files.oneline` is written only where `head`
    // is still the job's version. The v1 job is left waiting for its summary
    // while the head swings to v2; then v1 answers FIRST.
    let s = stub();
    let mut sp = space(&s, &[]);
    let (file, v1) = create(&mut sp, "/plan.md", &plan_text(), false);
    assert_eq!(sp.llm.len(), 1, "v1 waits for its summary");
    let was = oneline_of(&sp, &file);
    let v2 = swing(&mut sp, &file, "forty thousand", "fifty thousand");
    assert_ne!(v1, v2);
    assert_eq!(sp.llm.len(), 2, "v2 waits too, behind v1");

    summarize(&mut sp, "First.\n\nThe first plan.", "stop");
    assert_eq!(
        count(
            &sp,
            &format!("SELECT COUNT(*) FROM summaries WHERE file = '{file}' AND version = '{v1}'")
        ),
        3,
        "v1's rows are v1's (one line, short, tags): rows are kept per version"
    );
    assert_eq!(
        oneline_of(&sp, &file),
        was,
        "the head is v2: v1's one line never reaches files.oneline"
    );
    summarize(&mut sp, "Second.\n\nThe second plan.", "stop");
    assert_eq!(oneline_of(&sp, &file), "Second.");
    assert_scoped(&sp);
}

#[test]
fn in_derive_with_a_ws_on_the_hop_derives_a_main_line_version_as_without() {
    if !shipped() || !e_shipped() {
        return;
    }
    // OR-FH-73 (review E I-1b): no drop on `hop.ws` -- a main-line version is
    // derived whatever the hop carries; `line` alone decides.
    let s = stub();
    let mut sp = space(&s, &[]);
    let (file, v1) = create(&mut sp, "/plan.md", &plan_text(), false);
    summarize(&mut sp, "Born.\n\nThe plan at birth.", "stop");
    let measure = |sp: &Space, s: &Stub| {
        (
            calls(s),
            count(
                sp,
                &format!(
                    "SELECT COUNT(*) FROM summaries WHERE file = '{file}' AND version = '{v1}'"
                ),
            ),
            count(
                sp,
                &format!(
                    "SELECT COUNT(*) FROM embeddings WHERE file = '{file}' AND version = '{v1}'"
                ),
            ),
        )
    };
    let delta = |a: (usize, i64, i64), b: (usize, i64, i64)| (b.0 - a.0, b.1 - a.1, b.2 - a.2);

    let m0 = measure(&sp, &s);
    in_derive(&mut sp, &file, &v1, true, "");
    summarize(&mut sp, "Again.\n\nThe plan again.", "stop");
    let m1 = measure(&sp, &s);
    in_derive_hop(&mut sp, &file, &v1, true, "", json!({"ws": "draft"}));
    summarize(&mut sp, "Again.\n\nThe plan again.", "stop");
    let m2 = measure(&sp, &s);

    assert_eq!(
        delta(m0, m1),
        (1, 3, 0),
        "the reference: one embedding request, three summary rows (one line, short, tags \
         -- GH #947), 9 rows replaced"
    );
    assert_eq!(
        delta(m1, m2),
        delta(m0, m1),
        "with `ws` on the hop: the same"
    );
    assert_eq!(m2.2, 9);
    let derived = sp.routed("derived");
    assert_eq!(derived.len(), 2, "{derived:?}");
    assert!(
        derived.iter().all(|d| d.body["ok"] == json!(true)),
        "{derived:?}"
    );
    assert_scoped(&sp);
}

#[test]
fn a_working_version_sent_to_in_derive_gets_nothing() {
    if !shipped() || !e_shipped() {
        return;
    }
    // OR-FH-73 (review E I-1b): a version without a `line` row -- a working
    // version -- is never derived, even when `in_derive` names it.
    let s = stub();
    let mut sp = space(&s, &[]);
    let (file, v1) = create(&mut sp, "/plan.md", &plan_text(), false);
    summarize(&mut sp, "Born.\n\nThe plan at birth.", "stop");
    let seq = sp.rows(&format!("SELECT head_seq FROM files WHERE file = '{file}'"))[0][0]
        .as_i64()
        .unwrap();
    sp.seed_ws("ws-e", "draft", "/", seq);
    let a = sp.request(
        "in_write",
        "replace",
        Some(&file),
        json!({"old": "twice", "new": "three times", "expected": 1, "base": &v1[..12]}),
        json!({"ws": "draft"}),
    );
    assert_eq!(a["ok"], json!(true), "{a}");
    let working = col(
        &sp,
        &format!("SELECT working FROM ws_files WHERE ws = 'ws-e' AND file = '{file}'"),
    )[0]
    .clone();
    assert_ne!(working, v1);
    let before = (
        count(&sp, "SELECT COUNT(*) FROM summaries"),
        count(&sp, "SELECT COUNT(*) FROM embeddings"),
        calls(&s),
    );

    in_derive(&mut sp, &file, &working, true, "");
    assert_eq!(
        (
            count(&sp, "SELECT COUNT(*) FROM summaries"),
            count(&sp, "SELECT COUNT(*) FROM embeddings"),
            calls(&s),
        ),
        before,
        "no row, no request at the embedding stub"
    );
    assert!(sp.llm.is_empty(), "no request at the summarizer");
    let derived = sp.routed("derived");
    assert_eq!(derived.len(), 1, "{derived:?}");
    assert_eq!(derived[0].body["ok"], json!(false));
    assert!(
        sp.stderr.iter().any(|e| e.contains("not on the main line")),
        "{:?}",
        sp.stderr
    );
    assert_eq!(
        count(&sp, "SELECT COUNT(*) FROM pending WHERE op_id LIKE 'd-%'"),
        0
    );
    assert_scoped(&sp);
}

/// `op` (`search` semantic or `ask`) at `addr` with `extra` on the hop; the
/// summarizer, if asked, answers with the budget lines. Returns the ONE answer.
fn semantic(sp: &mut Space, op: &str, addr: &str, extra: Value) -> Value {
    let op_id = sp.next_op_id();
    let (args, mut hop) = if op == "search" {
        (
            json!({"pattern": "the budget", "mode": "semantic"}),
            json!({"mode": "semantic"}),
        )
    } else {
        (json!({"question": "How large is the budget?"}), json!({}))
    };
    hop["op"] = json!(op);
    hop["op_id"] = json!(op_id);
    for (k, v) in obj(extra) {
        hop[k] = v;
    }
    let before = sp.out.len();
    sp.lane(
        "in_read",
        json!({}),
        hop,
        json!({"op": op, "file": addr, "args": args}),
    );
    if !sp.llm.is_empty() {
        summarize(sp, "Forty thousand euro.\nSOURCES: 4-6", "stop");
    }
    let mine: Vec<Value> = sp.out[before..]
        .iter()
        .filter(|m| m.route() == "answer" && m.hop.get("op_id") == Some(&json!(op_id)))
        .map(|m| Value::Object(m.body.clone()))
        .collect();
    assert_eq!(
        mine.len(),
        1,
        "{op} at {addr}: {mine:?}; stderr {:?}",
        sp.stderr
    );
    mine.into_iter().next().unwrap()
}

#[test]
fn semantic_ops_resolve_every_address_form_and_refuse_with_the_readme_codes() {
    if !shipped() || !e_shipped() {
        return;
    }
    // Review E I-2: the address and error paths of `search` semantic and
    // `ask` (OR-FH-71, OR-FH-81, OR-FH.E.12, README error table), one row each.
    let s = stub();
    let mut sp = space(&s, &[]);
    let (file, v1) = create(&mut sp, "/plan.md", &plan_text(), false);
    summarize(&mut sp, "Plan.\n\nThe plan.", "stop");
    let v1_seq = sp.rows(&format!(
        "SELECT seq FROM line WHERE file = '{file}' AND version = '{v1}'"
    ))[0][0]
        .as_i64()
        .unwrap();
    let v2 = swing(&mut sp, &file, "forty thousand", "fifty thousand");
    summarize(&mut sp, "Plan two.\n\nThe plan again.", "stop");
    let v2_seq = sp.rows(&format!("SELECT head_seq FROM files WHERE file = '{file}'"))[0][0]
        .as_i64()
        .unwrap();

    // `draft`: opened on v1, untouched. `edit`: opened on v2, the file touched.
    sp.seed_ws("ws-d", "draft", "/", v1_seq);
    sp.seed_ws("ws-t", "edit", "/", v2_seq);
    let a = sp.request(
        "in_write",
        "replace",
        Some(&file),
        json!({"old": "twice", "new": "three times", "expected": 1, "base": &v2[..12]}),
        json!({"ws": "edit"}),
    );
    assert_eq!(a["ok"], json!(true), "{a}");
    let working = col(
        &sp,
        &format!("SELECT working FROM ws_files WHERE ws = 'ws-t' AND file = '{file}'"),
    )[0]
    .clone();
    sp.seed_snap(&file, "s1", &v1);

    // Two more versions of the file whose names share their first 4 hex.
    let mut seen: std::collections::HashMap<String, String> = Default::default();
    let pair = (0..)
        .find_map(|n| {
            let t = format!("variant {n}\n");
            let p = sha256_hex(t.as_bytes())[..4].to_string();
            seen.insert(p.clone(), t.clone()).map(|other| (p, other, t))
        })
        .unwrap();
    let (prefix, ta, tb) = pair;
    let va = sp.seed_version(&file, ta.as_bytes(), &[], &v2);
    let vb = sp.seed_version(&file, tb.as_bytes(), &[], &v2);
    let mut candidates: Vec<String> = col(
        &sp,
        &format!("SELECT version FROM versions WHERE file = '{file}' AND version LIKE '{prefix}%' ORDER BY version"),
    )
    .iter()
    .map(|v| v[..12].to_string())
    .collect();
    candidates.sort();
    assert!(
        candidates.contains(&va[..12].to_string()) && candidates.contains(&vb[..12].to_string())
    );
    let versions = col(
        &sp,
        &format!("SELECT version FROM versions WHERE file = '{file}' LIMIT 100"),
    );
    let unknown = (0u32..0x10000)
        .map(|n| format!("{n:04x}"))
        .find(|p| !versions.iter().any(|v| v.starts_with(p.as_str())))
        .unwrap();

    // A removed file, a file with a lost block, a binary file without pages.
    let (gone, _) = create(&mut sp, "/gone.md", "## Budget\nGone.\n", false);
    summarize(&mut sp, "Gone.\n\nA removed file.", "stop");
    sp.seed_tomb(&gone);
    let (broken, _) = create(&mut sp, "/broken.md", "## Budget\nBroken.\n", false);
    summarize(&mut sp, "Broken.\n\nA broken file.", "stop");
    sp.db
        .execute("DELETE FROM blocks WHERE file = ?1", [&broken])
        .unwrap();
    let bin = "fh-0000000000b1";
    sp.seed_file(bin, "/bin.dat", "binary", "application/octet-stream");
    let vbin = sp.seed_version(bin, b"\x00\x01\x02 raw bytes\x00", &[], "");
    sp.seed_head(bin, &vbin, 1000, "create");

    let at = |suffix: &str| format!("{file}{suffix}");
    // (row, address, hop extra, Ok(version) | Err(code))
    let rows: Vec<(&str, String, Value, Result<String, &str>)> = vec![
        (
            "head by id",
            file.clone(),
            json!({}),
            Ok(v2[..12].to_string()),
        ),
        (
            "head by path",
            "/plan.md".into(),
            json!({}),
            Ok(v2[..12].to_string()),
        ),
        (
            "@<v>",
            at(&format!("@{}", &v1[..8])),
            json!({}),
            Ok(v1[..12].to_string()),
        ),
        (
            "@snap:",
            at("@snap:s1"),
            json!({}),
            Ok(v1[..12].to_string()),
        ),
        (
            "untouched @ws:",
            at("@ws:draft"),
            json!({}),
            Ok(v1[..12].to_string()),
        ),
        (
            "untouched hop.ws",
            file.clone(),
            json!({"ws": "draft"}),
            Ok(v1[..12].to_string()),
        ),
        (
            "touched @ws:",
            at("@ws:edit"),
            json!({}),
            Err("not_indexed"),
        ),
        (
            "touched hop.ws",
            file.clone(),
            json!({"ws": "edit"}),
            Err("not_indexed"),
        ),
        (
            "hop.ws unknown",
            file.clone(),
            json!({"ws": "nope"}),
            Err("ws_unknown"),
        ),
        ("@ws: unknown", at("@ws:nope"), json!({}), Err("ws_unknown")),
        (
            "@snap: unknown",
            at("@snap:nope"),
            json!({}),
            Err("snap_unknown"),
        ),
        (
            "@<v> unknown",
            at(&format!("@{unknown}")),
            json!({}),
            Err("version_unknown"),
        ),
        (
            "@<v> ambiguous",
            at(&format!("@{prefix}")),
            json!({}),
            Err("version_ambiguous"),
        ),
        ("tombstoned", gone.clone(), json!({}), Err("tombstoned")),
        ("corrupt", broken.clone(), json!({}), Err("corrupt")),
        ("no_text", bin.to_string(), json!({}), Err("no_text")),
    ];
    for op in ["search", "ask"] {
        for (row, addr, extra, want) in &rows {
            let a = semantic(&mut sp, op, addr, extra.clone());
            match want {
                Ok(v) => {
                    assert_eq!(a["ok"], json!(true), "{op} {row}: {a}");
                    assert_eq!(a["version"], json!(v), "{op} {row}: {a}");
                    if op == "search" {
                        assert_eq!(a["hits"][0]["section"], json!("Budget"), "{op} {row}: {a}");
                    } else {
                        assert_eq!(
                            a["sources"],
                            json!([{"from_line": 4, "to_line": 6}]),
                            "{op} {row}: {a}"
                        );
                    }
                }
                Err(code) => {
                    assert_eq!(a["ok"], json!(false), "{op} {row}: {a}");
                    assert_eq!(a["error"]["code"], json!(code), "{op} {row}: {a}");
                    assert!(a.get("version").is_none(), "{op} {row}: {a}");
                    match *code {
                        "not_indexed" => assert_eq!(
                            a["error"]["current"],
                            json!(&working[..12]),
                            "{op} {row}: the working version is named"
                        ),
                        "version_ambiguous" => {
                            let mut got = strings(&a["error"]["candidates"]);
                            got.sort();
                            assert_eq!(got, candidates, "{op} {row}");
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    assert!(sp.llm.is_empty());
    assert_scoped(&sp);
}

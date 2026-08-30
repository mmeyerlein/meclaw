//! GH #513 — the catalogue row of `scriptlet` published the params and not the
//! stdin/stdout contract of the code those params carry.
//!
//! Measured on the e14 rebuild, wish 10: a five-cell feed strand
//! (`clock -> scriptlet -> fetcher -> scriptlet -> shelf`) that the composer
//! declared CORRECTLY — right templates, flat `override_params`, the
//! `http_status` range condition on the fetcher's exit edge — and that did not
//! run, because both scripts it wrote were written against a guessed wire.
//!
//! ```text
//! feed-ask,    stdout:  {"content": "{\"url\": …}"}      -> contract_violation
//! feed-dedupe, stdin:   data["messages"] … m["turns"]    -> nothing to read
//! ```
//!
//! Both shapes stand in `templates/scriptlet/README.md` § *The script contract*.
//! The README is not what a composer reads; the catalogue row is, and the row
//! published `PARAMS` plus one `PORTS` sentence whose only word about the output
//! was *"one message per content JSON the script writes"* — without saying what
//! a content JSON is, and without a syllable about the three-object stdin.
//!
//! `scriptlet@1.0.1` publishes it: a `SCRIPT` line beside `PORTS`, with two
//! worked scripts in it. This file is that line's drift lock in the sense of
//! `docs/development-rules.md` § 2d, and it does BOTH halves —
//!
//! 1. it GREPS the sentence: the row says what the three stdin objects are,
//!    where the payload sits, that a turn carries `text`, and that a
//!    `{"content": …}` is not a body slot;
//! 2. it RUNS THE MECHANISM: each script is lifted out of the shipped
//!    `template.json` and handed, verbatim, to a real `code` cell instantiated
//!    from the shipped `scriptlet` template through an ordinary manifest — the
//!    same path an `override_params` takes — and what leaves that cell is
//!    asserted. The counter-example is run through the same cell, so the
//!    sentence about `contract_violation` is a measurement rather than a claim.
//!
//! **R2b guard.** Every read is guarded by [`shipped`]: in a tree that does not
//! carry the template, these tests skip rather than fail on a dead reference.

use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, ColonyMsg, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, Path};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, MessageBuilder};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// Generous failure-marker timeout (CONTRIBUTING.md 30 s convention).
const RECV_TIMEOUT: Duration = Duration::from_secs(30);

/// The shape the row warns against — the one `feed-ask` actually wrote.
const THE_SHAPE_THAT_FAILS: &str =
    "import sys, json\njson.load(sys.stdin)\nprint(json.dumps({\"content\": \"{}\"}))\n";

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The shipped descriptor, or `None` where this tree does not carry it.
fn shipped() -> Option<Value> {
    let raw = std::fs::read_to_string(repo("templates/scriptlet/template.json")).ok()?;
    meclaw_core::serde_json::from_str(&raw).ok()
}

/// The `SCRIPT` line of `description.examples` — the surface under test.
fn script_line(tpl: &Value) -> String {
    tpl["description"]["examples"]
        .as_array()
        .expect("the descriptor carries examples")
        .iter()
        .filter_map(|e| e.as_str())
        .find(|e| e.trim_start().starts_with("SCRIPT"))
        .unwrap_or_else(|| {
            panic!(
                "`templates/scriptlet/template.json` publishes no `SCRIPT` line, so the \
                 one template in the library whose whole purpose is to carry authored \
                 code publishes nothing about the contract that code has to meet (GH #513)"
            )
        })
        .to_string()
}

/// The fenced blocks of the `SCRIPT` line, in order: the pass-through first,
/// the transform second. They are what a composer copies, so they are what is
/// run — lifted out of the published bytes rather than restated here.
fn published_scripts(tpl: &Value) -> Vec<String> {
    let line = script_line(tpl);
    let mut out = Vec::new();
    let mut rest = line.as_str();
    while let Some(open) = rest.find("```") {
        let tail = &rest[open + 3..];
        let Some(close) = tail.find("```") else { break };
        out.push(tail[..close].trim_matches('\n').to_string());
        rest = &tail[close + 3..];
    }
    out
}

fn version(tpl: &Value) -> String {
    tpl["version"]
        .as_str()
        .expect("the descriptor declares a version")
        .to_string()
}

// ================================================================== HALF ONE

/// The sentence itself: what a composer is told before it writes a line.
#[test]
fn the_row_says_what_a_content_json_is_and_where_the_payload_sits() {
    let Some(tpl) = shipped() else { return };
    let line = script_line(&tpl);

    for phrase in [
        "`envelope`",
        "`body`",
        "`params`",
        "doc[\"body\"][\"messages\"]",
        "`text`",
        "`turns`",
        "{\"content\": ...}",
        "contract_violation",
        "doc[\"envelope\"][\"header\"]",
        "`hop`",
    ] {
        assert!(
            line.contains(phrase),
            "the SCRIPT line does not name {phrase:?} — both measured defects of GH \
             #513 were about exactly these words: {line}"
        );
    }

    assert_eq!(
        published_scripts(&tpl).len(),
        2,
        "the line publishes a pass-through AND a transform: one worked read and \
         one worked write, which is what the two broken scripts each got wrong"
    );
}

/// The line is only published if it TRAVELS. A catalogue row over the corpus
/// generator's 4000-character cap is split into `-cont` rows, and the retriever
/// hands a `template` hit over whole up to its own catalogue window — so a
/// descriptor that grew past the cap would publish the `SCRIPT` line into a
/// continuation the composer never asks for.
#[test]
fn the_script_line_is_inside_the_one_catalogue_row_the_corpus_carries() {
    let Some(tpl) = shipped() else { return };
    let Ok(raw) =
        std::fs::read_to_string(repo("templates/builder-librarian/store/seed/docs.jsonl"))
    else {
        return; // R2b: this tree does not carry the corpus.
    };
    let rows: Vec<Value> = raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| meclaw_core::serde_json::from_str(l).ok())
        .filter(|r: &Value| r["kind"] == "template" && r["section"] == "scriptlet")
        .collect();

    assert_eq!(
        rows.len(),
        1,
        "`scriptlet` must be ONE catalogue row: a descriptor over the generator's \
         cap is chunked, and a `SCRIPT` line in a continuation row is a contract \
         published where nobody looks"
    );
    let text = rows[0]["text"].as_str().unwrap_or_default();
    // The row carries the descriptor as JSON, so the line travels ESCAPED —
    // comparing against the raw string would pass only for a line without a
    // newline in it, which is every line except this one.
    let escaped = meclaw_core::serde_json::to_string(&script_line(&tpl)).expect("a string");
    assert!(
        text.contains(escaped.trim_matches('"')),
        "the corpus is stale — regenerate it with `workshop/tools/build_librarian_seed.py`"
    );
}

// ================================================================== HALF TWO

fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![
        (
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        ),
        ("store".to_string(), Arc::new(StoreCellFactory)),
    ]
}

fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).expect("create destination");
    for entry in std::fs::read_dir(src).expect("read source") {
        let entry = entry.expect("dir entry");
        let target = dst.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).expect("copy");
        }
    }
}

/// A colony with a root hive, a sink, and the shipped `scriptlet` in its library.
fn tree() -> tempfile::TempDir {
    let td = tempfile::TempDir::new().expect("tempdir");
    let root = td.path();
    std::fs::create_dir_all(root.join("main")).expect("root hive dir");
    std::fs::write(
        root.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .expect("write the root hive");
    for name in ["scriptlet", "shelf"] {
        copy_tree(
            &repo("templates").join(name),
            &root.join("templates").join(name),
        );
    }
    td
}

async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<Message>) {
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (tx, rx) = mpsc::channel::<Message>(64);
    h.spawn(Path::new("/sink"), move || CaptureCell::new(tx.clone()))
        .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("a colony with nothing but a sink boots");
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: td.path().join("templates"),
            ack: ack_tx,
        })
        .await
        .expect("rescan sent");
    ack_rx
        .await
        .expect("rescan acked")
        .expect("the library must register");
    (h, rx)
}

/// Instantiate one `scriptlet` per script, exactly the way a manifest does it:
/// `add_nodes` naming `scriptlet@<shipped version>` with a flat
/// `override_params.script_inline`, and one edge each onto the sink.
async fn grow(h: &ColonyHandle, tpl: &Value, cells: &[(&str, &str)]) {
    let reference = format!("scriptlet@{}", version(tpl));
    let nodes: Vec<Value> = cells
        .iter()
        .map(|(name, script)| {
            json!({"name": name, "template": reference,
                   "override_params": {"script_inline": script}})
        })
        .collect();
    grow_nodes(h, nodes, cells.iter().map(|(n, _)| *n).collect()).await;
}

/// The same act, for nodes a caller built itself: one `add_nodes` and one edge
/// onto the sink per node.
async fn grow_nodes(h: &ColonyHandle, nodes: Vec<Value>, names: Vec<&str>) {
    let edges: Vec<Value> = names
        .iter()
        .map(|name| json!({"from": format!("./{name}"), "to": "./sink"}))
        .collect();
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload: json!({"scope": "/", "diff": {"add_nodes": nodes, "add_edges": edges}}),
            reply_to: None,
            trace_id: meclaw_core::Uuid::now_v7(),
            parent_message_id: meclaw_core::Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("mutation sent");
    let outcome = ack_rx.await.expect("mutation acked");
    assert!(
        matches!(outcome, meclaw_colony::MutationOutcome::Committed { .. }),
        "the published scripts must be instantiable by an ordinary declaration: {outcome:?}"
    );
}

/// One turn in, and the emission the cell handed to the sink.
async fn drive(
    h: &ColonyHandle,
    rx: &mut mpsc::Receiver<Message>,
    target: &str,
    text: &str,
) -> Message {
    h.send(
        MessageBuilder::new(target)
            .with_inline_messages(vec![
                json!({"origin": "user", "type": "text", "id": "t1", "text": text}),
            ])
            .build(),
    )
    .await;
    tokio::time::timeout(RECV_TIMEOUT, rx.recv())
        .await
        .unwrap_or_else(|_| panic!("nothing reached the sink from {target} in 30 s"))
        .expect("the sink channel stays open")
}

fn turns(m: &Message) -> Vec<Value> {
    match &m.body {
        Body::Inline(v) => v["messages"].as_array().cloned().unwrap_or_default(),
        Body::Blob(_) => Vec::new(),
    }
}

fn hop<'a>(m: &'a Message, key: &str) -> Option<&'a Value> {
    m.headers.hop.get(key)
}

/// **The load-bearing test.** Both published scripts, and the shape the line
/// warns against, run through real `code` cells grown from the shipped template.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_published_scripts_run_in_a_real_code_cell_and_the_warned_shape_does_not() {
    let Some(tpl) = shipped() else { return };
    let scripts = published_scripts(&tpl);
    assert_eq!(scripts.len(), 2, "the line publishes two worked scripts");

    let td = tree();
    let (h, mut rx) = boot(&td).await;
    grow(
        &h,
        &tpl,
        &[
            ("pass", scripts[0].as_str()),
            ("xform", scripts[1].as_str()),
            ("guessed", THE_SHAPE_THAT_FAILS),
        ],
    )
    .await;

    // (a) The pass-through: the turns it was given, handed straight back.
    let m = drive(&h, &mut rx, "/pass", "a turn to hand back").await;
    assert_eq!(
        hop(&m, "error_code"),
        None,
        "the published pass-through must not raise: {:?}",
        m.headers.hop
    );
    assert_eq!(hop(&m, "exit_code"), Some(&json!(0)), "{:?}", m.headers.hop);
    assert_eq!(
        turns(&m)
            .first()
            .and_then(|t| t["text"].as_str())
            .unwrap_or_default(),
        "a turn to hand back",
        "the pass-through reads `doc[\"body\"][\"messages\"]` and writes it back"
    );

    // (b) The transform: body slots out, a tool_call the next cell can read.
    let m = drive(&h, &mut rx, "/xform", "https://example.org/feed.xml").await;
    assert_eq!(
        hop(&m, "error_code"),
        None,
        "the published transform must not raise: {:?}",
        m.headers.hop
    );
    assert_eq!(
        hop(&m, "route"),
        Some(&json!("fetch")),
        "the optional `header` section is what the colony reads as `hop`: {:?}",
        m.headers.hop
    );
    let turn = turns(&m).first().cloned().unwrap_or(Value::Null);
    assert_eq!(turn["type"], json!("tool_call"), "{turn}");
    let args: Value =
        meclaw_core::serde_json::from_str(turn["text"].as_str().unwrap_or("null")).expect("args");
    assert_eq!(
        args["url"],
        json!("https://example.org/feed.xml"),
        "the transform lifts the turn's `text` — the key the broken script called `turns`"
    );

    // (c) The counter-example, through the same cell type and the same door:
    //     the sentence about `contract_violation` is measured here, not claimed.
    let m = drive(&h, &mut rx, "/guessed", "anything").await;
    assert_eq!(
        hop(&m, "error_code"),
        Some(&json!("contract_violation")),
        "a `{{\"content\": …}}` emission is what the row warns about; if it \
         travelled, the warning would be prose about nothing: {:?}",
        m.headers.hop
    );

    h.shutdown().await;
}

// =========================================================== THE NEIGHBOUR

// The same gap, one cell downstream. Measured on the verification run of this
// very issue: with the `SCRIPT` line published, the composer wrote both scripts
// to the contract -- and the shelf then refused every insert with
//
// ```text
// insert: unknown argument "values" (known: operation, table, row)
// ```
//
// 40 legs, all of them. `shelf`'s row published the OPERATION names
// (`insert`, `select`, …) and not one of the keys an operation reads, so the
// store's own refusal printed the list the corpus did not carry -- which is
// GH #505's finding on the arguments instead of on the params. `shelf@1.0.1`
// publishes an `ARGS` line with two WORKED calls in it, and this is its lock.

/// The shipped `shelf` descriptor, or `None` where the tree does not carry it.
fn shipped_shelf() -> Option<Value> {
    let raw = std::fs::read_to_string(repo("templates/shelf/template.json")).ok()?;
    meclaw_core::serde_json::from_str(&raw).ok()
}

/// The `ARGS` line of `shelf`'s `description.examples`.
fn args_line(tpl: &Value) -> String {
    tpl["description"]["examples"]
        .as_array()
        .expect("the descriptor carries examples")
        .iter()
        .filter_map(|e| e.as_str())
        .find(|e| e.trim_start().starts_with("ARGS"))
        .unwrap_or_else(|| {
            panic!(
                "`templates/shelf/template.json` publishes no `ARGS` line, so a \
                 composer is told which OPERATIONS exist and never which keys one \
                 reads -- and an unknown key is refused, not ignored"
            )
        })
        .to_string()
}

/// The worked calls at the end of the `ARGS` line, parsed as the JSON they are.
fn worked_calls(tpl: &Value) -> Vec<Value> {
    let line = args_line(tpl);
    let tail = line
        .split_once("Worked: ")
        .expect("the ARGS line ends in worked calls")
        .1;
    let mut out = Vec::new();
    let bytes: Vec<char> = tail.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == '{' {
            let mut depth = 0usize;
            let start = i;
            while i < bytes.len() {
                match bytes[i] {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            let raw: String = bytes[start..=i].iter().collect();
                            out.push(meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| {
                                panic!("worked call is not JSON ({e}): {raw}")
                            }));
                            break;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
        }
        i += 1;
    }
    out
}

/// One store call in, and the answer the shelf handed to the sink.
async fn call(
    h: &ColonyHandle,
    rx: &mut mpsc::Receiver<Message>,
    target: &str,
    args: &Value,
) -> Message {
    h.send(
        MessageBuilder::new(target)
            .with_inline_messages(vec![json!({
                "origin": "assistant", "type": "tool_call", "id": "c1",
                "text": args.to_string()
            })])
            .build(),
    )
    .await;
    tokio::time::timeout(RECV_TIMEOUT, rx.recv())
        .await
        .unwrap_or_else(|_| panic!("nothing reached the sink from {target} in 30 s"))
        .expect("the sink channel stays open")
}

#[test]
fn the_args_line_names_the_key_the_measured_run_got_wrong() {
    let Some(tpl) = shipped_shelf() else { return };
    let line = args_line(&tpl);
    for phrase in [
        "`row`",
        "`values`",
        "`where`",
        "`set`",
        "`columns`",
        "REFUSED",
    ] {
        assert!(
            line.contains(phrase),
            "the ARGS line does not name {phrase:?}: {line}"
        );
    }
    let calls = worked_calls(&tpl);
    assert_eq!(
        calls.len(),
        2,
        "the line publishes a worked WRITE and a worked READ"
    );
    assert_eq!(calls[0]["operation"], json!("insert"));
    assert_eq!(calls[1]["operation"], json!("select"));

    // The ANSWER shape belongs to the same measurement: the run's dedupe read
    // `result.get("rows")` off a body that is a bare array, found nothing, and
    // re-inserted all forty items on every tick.
    let ports = tpl["description"]["examples"][0]
        .as_str()
        .expect("examples[0] is the PORTS paragraph");
    for phrase in ["JSON ARRAY", "{\"rows\": [...]}"] {
        assert!(
            ports.contains(phrase),
            "the PORTS line does not say what a READ's turn text IS ({phrase:?}): {ports}"
        );
    }
}

/// **The neighbour's load-bearing test.** The published calls run against a real
/// `store` cell grown from the shipped `shelf`, and the key the row warns
/// against is refused by that same cell.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_published_calls_run_against_a_real_shelf_and_the_warned_key_does_not() {
    let Some(tpl) = shipped_shelf() else { return };
    let calls = worked_calls(&tpl);
    let table = calls[0]["table"]
        .as_str()
        .expect("the worked call names a table");
    let columns: Vec<(String, Value)> = calls[0]["row"]
        .as_object()
        .expect("the worked insert carries a `row` object")
        .keys()
        .map(|k| (k.clone(), json!("text")))
        .collect();
    let schema =
        json!({table: columns.into_iter().collect::<meclaw_core::serde_json::Map<_, _>>()});

    let td = tree();
    let (h, mut rx) = boot(&td).await;
    grow_nodes(
        &h,
        vec![json!({
            "name": "rack",
            "template": format!("shelf@{}", version(&tpl)),
            "override_params": {"schema": schema}
        })],
        vec!["rack"],
    )
    .await;

    // (a) The worked write.
    let m = call(&h, &mut rx, "/rack", &calls[0]).await;
    assert_eq!(
        hop(&m, "rows_affected"),
        Some(&json!(1)),
        "the published insert must land one row: {:?}",
        m.headers.hop
    );

    // (b) The worked read finds it.
    let m = call(&h, &mut rx, "/rack", &calls[1]).await;
    assert_eq!(
        hop(&m, "rows_affected"),
        Some(&json!(1)),
        "the published select must find the row the published insert wrote: {:?}",
        m.headers.hop
    );
    let read_back = turns(&m)
        .first()
        .and_then(|t| t["text"].as_str())
        .unwrap_or_default()
        .to_string();
    let parsed: Value = meclaw_core::serde_json::from_str(&read_back)
        .unwrap_or_else(|e| panic!("a read answer is JSON ({e}): {read_back}"));
    assert!(
        parsed.is_array(),
        "a read's turn text is a bare ARRAY of row objects, which is what the \
         PORTS line now says -- the measured run read `.get(\"rows\")` off it, \
         found nothing and re-inserted every item on every tick: {read_back}"
    );

    // (c) The key the line warns against, through the same cell. The refusal
    //     text is the store's own and it names the whole set -- which is the
    //     sentence the ARGS line exists to publish ahead of the failure.
    let mut wrong = calls[0].clone();
    let row = wrong["row"].take();
    wrong.as_object_mut().expect("an object").remove("row");
    wrong["values"] = row;
    let m = call(&h, &mut rx, "/rack", &wrong).await;
    let said = turns(&m)
        .first()
        .and_then(|t| t["text"].as_str())
        .unwrap_or_default()
        .to_string();
    assert!(
        said.contains("unknown argument") && said.contains("\"values\"") && said.contains("row"),
        "a `values` key must be refused by name, not ignored -- 40 legs of one \
         measured run died on exactly this: {said}"
    );

    h.shutdown().await;
}

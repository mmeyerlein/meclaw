//! GH #447 -- an export becomes a seed set on disk, in a running colony.
//!
//! `memory-hive` has been able to hand its remembered content out as a
//! versioned document since 2.2.0 (#243), one `dump` part per content table.
//! Nothing drained the lane, so in a grown colony every part became a `no_route`
//! dead letter: the walk ran, the document existed for the length of a message,
//! and nobody could point at a file afterwards. `member` closes that since 1.2.0 with
//! one cell, and this file drives the cell for real -- the SHIPPED
//! `templates/member/export-sink/config.json`, script and all.
//!
//! What is proved here is the half a file-reading test cannot reach: the bytes
//! on disk. The hive's own walk (which tables, in which order, with which
//! completeness marker) is pinned by `gh243_a_memory_can_leave_a_hive_and_arrive_in_another`
//! and is not repeated; this colony feeds the sink parts of exactly the shape
//! that walk produces.
//!
//! **One deliberate substitution, and it is named rather than hidden.** The
//! shipped sink runs behind `params.sandbox` with `trust: "restricted"` and the
//! export directory as its only write root. A `restricted` profile is
//! fail-closed against the HOST -- no Landlock in the kernel, no namespaces, and
//! the spawn fails -- so a colony test that kept the profile would measure the
//! machine it runs on instead of the sink. The profile is therefore replaced
//! here, and the shipped one is asserted separately, in
//! `gh447_the_member_fires_the_close_pass` §
//! `the_sink_writes_only_under_the_directory_it_declares`. Nothing else about
//! the cell is touched: the script, the timeout, the concurrency cap and the
//! contract are the shipped bytes.
//!
//! Four properties, one per way this could look finished and be wrong:
//!
//! 0. **One directory per hive** (GH #471). A part names the hive it came out
//!    of, and that name is the directory it is filed under. Three hives export
//!    since #471 and two of them -- `memory-hive` and `affinity` -- both have a
//!    table called `entities`, so the flat sink this file used to pin would
//!    have written one over the other without a word.
//! 1. **The seed format is the birth format.** Line 1 is the store's own schema
//!    declaration, every line after it is one row. That is what makes "export
//!    the old hive, birth a new one from its parts" a mechanical operation.
//! 2. **`absent` is not `rows: []`.** An absent table writes NO file (this hive
//!    never had the table); an empty one writes a file carrying only its schema
//!    line (it had the table and remembered nothing). Collapsing the two would
//!    turn a missing table into an empty one on the next birth.
//! 3. **The dead-letter queue stays empty**, and the completion word arrives.
//!    An export that produced the right files and dead-lettered its own receipt
//!    would pass a file-only check and still be a lane nobody can wait on.

use meclaw_cells::code::CodeCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, from_str, json, to_string_pretty};
use meclaw_core::{Body, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use std::sync::Arc;
use std::time::Duration;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The four parts this colony walks: two ordinary tables, one that is present
/// and empty, one that is absent -- and the last one carries the walk's own
/// completeness marker, exactly as the porter stamps it.
fn parts() -> Vec<Value> {
    let head = |table: &str, part: u64, final_: bool, absent: bool| {
        json!({
            "format": "meclaw-memory-export/1",
            "hive_template": "memory-hive",
            "export_id": "exp-447",
            "exported_at": "2026-08-28T00:00:00Z",
            "table": table,
            "part": part,
            "of": 4,
            "final": final_,
            "absent": absent,
            "key": ["id"],
        })
    };
    let with = |mut v: Value, schema: Value, rows: Value| {
        v["schema"] = schema;
        v["rows"] = rows;
        v
    };
    vec![
        with(
            head("episodes", 1, false, false),
            json!({"id": "text", "session_id": "text", "audience_set": "text"}),
            json!([
                {"id": "e1", "session_id": "s1", "audience_set": "a"},
                {"id": "e2", "session_id": "s1", "audience_set": "a"}
            ]),
        ),
        with(
            head("topics", 2, false, false),
            json!({"id": "text", "label": "text"}),
            json!([{"id": "t1", "label": "the export"}]),
        ),
        // present, remembered nothing
        with(
            head("skills", 3, false, false),
            json!({"id": "text", "name": "text"}),
            json!([]),
        ),
        // older than the declaration: never had the table
        with(head("beliefs", 4, true, true), json!({}), json!([])),
    ]
}

/// A code cell that turns one probe into the four `dump` messages the porter
/// would have produced, headers and all.
fn emitter_config() -> String {
    let script = r#"
import sys, json
doc = json.load(sys.stdin)
out = []
for part in doc["params"]["parts"]:
    out.append({"header": {"route": "dump", "dump_kind": "export_part",
                           "port_run": "run-447",
                           "port_table": part["table"],
                           "export_part": part["part"],
                           "export_of": part["of"],
                           "export_final": "1" if part["final"] else "0"},
                "messages": [{"origin": "assistant", "type": "text",
                              "text": json.dumps(part)}]})
sys.stdout.write(json.dumps(out))
"#;
    to_string_pretty(&json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": script, "parts": parts(),
                   "sandbox": {"trust": "trusted"}},
        "contract": {"version": "1.0.0", "settings": {}, "multi_send_capable": true,
                     "emits": {"body": {"messages": {"type": "array", "required": true}},
                               "hop": {"route": {"type": "string", "required": true},
                                       "dump_kind": {"type": "string", "required": false},
                                       "port_run": {"type": "string", "required": false},
                                       "port_table": {"type": "string", "required": false},
                                       "export_part": {"type": "number", "required": false},
                                       "export_of": {"type": "number", "required": false},
                                       "export_final": {"type": "string", "required": false}}},
                     "consumes": {}}
    }))
    .unwrap()
}

/// The drain for `export_done`: it writes one receipt file, so the completion
/// word is proved by something that had to arrive rather than by the absence of
/// a dead letter.
fn receipt_config(dir: &str) -> String {
    let script = r#"
import sys, json, os
doc = json.load(sys.stdin)
hop = (doc["envelope"].get("header") or {}).get("hop") or {}
with open(os.path.join(doc["params"]["receipt_dir"], "receipt.json"), "w") as fh:
    fh.write(json.dumps({"route": hop.get("route"), "export_id": hop.get("export_id"),
                         "export_hive": hop.get("export_hive"),
                         "export_of": hop.get("export_of")}, sort_keys=True))
sys.stdout.write(json.dumps([]))
"#;
    to_string_pretty(&json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": script, "receipt_dir": dir,
                   "sandbox": {"trust": "trusted"}},
        "contract": {"version": "1.0.0", "settings": {}, "multi_send_capable": true,
                     "emits": {}, "consumes": {}}
    }))
    .unwrap()
}

/// The shipped sink, with its `params.export_dir` pointed at this test's
/// directory and its sandbox profile replaced (see the file header for why).
fn sink_config(export_dir: &str) -> String {
    let raw = std::fs::read_to_string(repo("templates/member/export-sink/config.json"))
        .expect("the shipped export sink");
    let mut c: Value = from_str(&raw).expect("the shipped sink is json");
    assert_eq!(
        c["params"]["sandbox"]["trust"], "restricted",
        "the shipped sink is the one behind a boundary; if this ever reads \
         `trusted`, the substitution below is hiding a real regression"
    );
    c["params"]["export_dir"] = json!(export_dir);
    c["params"]["sandbox"] = json!({"trust": "trusted"});
    to_string_pretty(&c).unwrap()
}

fn read_lines(p: &std::path::Path) -> Vec<Value> {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| from_str(l).unwrap_or_else(|e| panic!("{} line is not json: {e}", p.display())))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_export_becomes_a_seed_set_and_nothing_dead_letters() {
    let td = tempfile::TempDir::new().unwrap();
    let export_dir = td.path().join("exports");
    let receipt_dir = td.path().join("receipts");
    std::fs::create_dir_all(&export_dir).unwrap();
    std::fs::create_dir_all(&receipt_dir).unwrap();

    for cell in ["emitter", "sink", "receipt"] {
        std::fs::create_dir_all(td.path().join("main").join(cell)).unwrap();
    }
    std::fs::write(
        td.path().join("main/config.json"),
        to_string_pretty(&json!({
            "cell": {"type": "hive"},
            "params": {"graph": {"edges": [
                {"from": "./emitter", "to": "./sink",
                 "condition": "has(hop.route) && hop.route == 'dump'"},
                {"from": "./sink", "to": "./receipt",
                 "condition": "has(hop.route) && hop.route == 'export_done'"}
            ]}}
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(td.path().join("main/emitter/config.json"), emitter_config()).unwrap();
    std::fs::write(
        td.path().join("main/sink/config.json"),
        sink_config(export_dir.to_str().unwrap()),
    )
    .unwrap();
    std::fs::write(
        td.path().join("main/receipt/config.json"),
        receipt_config(receipt_dir.to_str().unwrap()),
    )
    .unwrap();

    let code: Arc<dyn CellFactory> = Arc::new(CodeCellFactory);
    let h = ColonyHandle::new_with_factories_at(&td, vec![("code".to_string(), code.clone())]);
    let mut registry = CellFactoryRegistry::new();
    registry.insert("code".to_string(), code);
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap must succeed");

    h.send(
        MessageBuilder::new(Path::new("/emitter"))
            .body(Body::Inline(json!({"messages": []})))
            .build(),
    )
    .await;

    // The completion word is the last thing that happens, so waiting for the
    // receipt waits for the whole walk.
    let receipt = receipt_dir.join("receipt.json");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline && !receipt.exists() {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        receipt.exists(),
        "no export_done reached its drain -- dead letters: {:?}",
        h.drain_dead_letters()
            .await
            .iter()
            .map(|d| (d.sender_path.as_str().to_string(), d.reason.as_code()))
            .collect::<Vec<_>>()
    );
    let r: Value = from_str(&std::fs::read_to_string(&receipt).unwrap()).unwrap();
    assert_eq!(r["route"], "export_done");
    assert_eq!(r["export_id"], "exp-447");
    assert_eq!(r["export_of"], 4, "the receipt names the size of the walk");

    // 0. the part was filed under the hive that produced it, not flat
    let seed = export_dir.join("memory-hive").join("seed");
    assert!(
        !export_dir.join("seed").exists(),
        "the sink filed the parts flat -- two hives with a table of the same \
         name would overwrite each other (GH #471)"
    );
    assert_eq!(
        r["export_hive"], "memory-hive",
        "the completion word names the hive that finished, because three of \
         them travel per member export"
    );

    // 1. the seed format: schema first, one row per line after it
    let episodes = read_lines(&seed.join("episodes.jsonl"));
    assert_eq!(episodes.len(), 3, "one schema line plus two rows");
    assert_eq!(
        episodes[0],
        json!({"schema": {"id": "text", "session_id": "text", "audience_set": "text"}}),
        "line 1 is the store's own declaration, verbatim -- that is what makes \
         this file a seed"
    );
    assert_eq!(episodes[1]["id"], "e1");
    assert_eq!(episodes[2]["id"], "e2");
    assert_eq!(read_lines(&seed.join("topics.jsonl")).len(), 2);

    // 2. absent is not empty
    let skills = read_lines(&seed.join("skills.jsonl"));
    assert_eq!(
        skills.len(),
        1,
        "a table that exists and remembered nothing is a file with its schema \
         line and no rows"
    );
    assert!(
        !seed.join("beliefs.jsonl").exists(),
        "an ABSENT table writes no file at all -- a hive that never had the \
         table must not be reborn with an empty one"
    );

    // the marker, and it stands beside a complete set
    let marker: Value =
        from_str(&std::fs::read_to_string(seed.join("export_final.json")).unwrap()).unwrap();
    assert_eq!(marker["export_id"], "exp-447");
    assert_eq!(marker["of"], 4);
    assert_eq!(marker["format"], "meclaw-memory-export/1");
    assert_eq!(marker["hive"], "memory-hive");

    // and the member-level one beside the directories, which names every hive
    // that has finished so far -- here one, because this colony has one sender
    let member_marker: Value =
        from_str(&std::fs::read_to_string(export_dir.join("export_final.json")).unwrap()).unwrap();
    assert_eq!(member_marker["format"], "meclaw-member-export/1");
    assert_eq!(
        member_marker["hives"],
        json!(["memory-hive"]),
        "the member-level marker is rebuilt from what stands on disk, so it \
         names the hives whose walks finished and no others"
    );
    assert_eq!(member_marker["parts"][0]["of"], 4);

    // 3. nothing died on the way
    let dl = h.drain_dead_letters().await;
    assert!(
        dl.is_empty(),
        "an export that dead-letters is the state this lane was built to end; \
         got {:?}",
        dl.iter()
            .map(|d| (
                d.sender_path.as_str().to_string(),
                d.resolved_target.as_str().to_string(),
                d.reason.as_code()
            ))
            .collect::<Vec<_>>()
    );

    h.shutdown().await;
}

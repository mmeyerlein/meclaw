//! GH #243 — a memory can leave a hive and arrive in another one.
//!
//! `memory-hive@2.1.0` had no way to take the content a hive had accumulated
//! OUT of it, and no way to put such content INTO another one. The only
//! substrate-native content path was the JSONL seeder, and that is birth-only:
//! a `cell.db` that already exists means an inert seed. So every migration,
//! every backup and every "re-run the benchmark against the same remembered
//! state" was a hand-built `sqlite3` pipeline reaching around exactly the
//! boundary #132 and #160 exist to keep closed.
//!
//! 2.2.0 answers that with two lanes on the hive path — `in_export` and
//! `in_import` — served by one new code cell, `porter`, and a declared
//! versioned document (`meclaw-memory-export/1`) whose parts double as the
//! store's own `seed/<table>.jsonl` files.
//!
//! # What is under test, and how honestly
//!
//! The same P5/`cellrun` pattern the other memory-hive script tests use: the
//! REAL `params.script_inline` of the REAL shipped `config.json`, one hop per
//! process, `${VAR:-default}` resolved the way the colony resolves it. Store
//! REPLIES are fixtures — they are data, and data is what a transfer moves.
//! The store cell itself lives in `meclaw-cells`, which depends on this crate,
//! so a colony carrying the real hive can only be booted downstream of here.
//!
//! Everything is guarded on the template being present (GH #49): the public
//! tree does not ship `templates/memory-hive`, and there the body does not run.
//!
//! # The claims
//!
//! 1. The document covers **exactly** the content tables the shipped store
//!    declares — the mirror inside the script and `store/config.json` cannot
//!    drift apart unnoticed, because a column added to the store and not to
//!    the mirror is a column that silently stops travelling.
//! 2. Every part projects every declared column, `audience_set`, `channel` and
//!    `speaker` among them. A column nobody selects never leaves the store.
//! 3. A part carries the schema header that makes it a seed file, and the walk
//!    ends with a part that says it is the last one.
//! 4. **Provenance survives the transfer byte for byte** — and a part that lost
//!    it on the way is refused whole, with nothing written. That is the one
//!    failure this lane must not have: an imported row whose participant set
//!    did not survive is a row that may be told to anyone.
//! 5. An audience that is present but EMPTY stays empty. Empty means invisible;
//!    inventing one would be the laundering itself.
//! 6. The same document applied twice writes nothing the second time.
//! 7. The two store-KEYED families arrive as `set_alias` / `reject_pair` — the
//!    store's own upserts — and never as `insert`. That is the half of #243 the
//!    JSONL seeder cannot reach.
//! 8. The final part re-derives every identity dimension.
//! 9. The hive declares the lanes and makes their drains mandatory.

use meclaw_core::serde_json::{Map, Value, json};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

// ------------------------------------------------------------------ harness

/// The shipped template, or `None` in a tree that does not carry it (GH #49).
fn hive_root() -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/memory-hive");
    p.join("config.json").is_file().then_some(p)
}

fn config_at(rel: &str) -> Value {
    let p = hive_root().expect("template").join(rel);
    meclaw_core::serde_json::from_str(&std::fs::read_to_string(p).expect("read")).expect("json")
}

fn hive_config() -> Value {
    config_at("config.json")
}

/// `${VAR:-default}` becomes the default, a bare `${VAR}` the empty string —
/// the substitution the colony performs at instantiation.
fn resolve_vars(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    let mut rest = script;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail.find('}').expect("unterminated ${...}");
        if let Some((_, default)) = tail[..end].split_once(":-") {
            out.push_str(default);
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

fn porter_script() -> String {
    resolve_vars(
        config_at("porter/config.json")["params"]["script_inline"]
            .as_str()
            .expect("porter has no script_inline"),
    )
}

/// The three-object stdin document the substrate builds.
fn stdin_doc(flat: &Value) -> Value {
    let mut envelope = Map::new();
    let mut slots = Map::new();
    for (k, v) in flat.as_object().expect("a flat message object") {
        if k == "header" {
            envelope.insert(k.clone(), v.clone());
        } else {
            slots.insert(k.clone(), v.clone());
        }
    }
    json!({"envelope": envelope, "body": slots, "params": {}})
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

fn python(src: String) -> String {
    let out = run_python(&src);
    assert!(
        out.status.success(),
        "porter stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Drives ONE real hop of the shipped `porter`: `body` goes in as stdin,
/// whatever the script emitted comes back parsed.
fn run_hop(body: &Value) -> Vec<Value> {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "_sink, _real = io.StringIO(), sys.stdout\n",
            "sys.stdout = _sink\n",
            "try:\n",
            "    exec(compile(_script, 'cell', 'exec'), globals())\n",
            "except SystemExit:\n",
            "    pass\n",
            "finally:\n",
            "    sys.stdout = _real\n",
            "_real.write(_sink.getvalue())\n"
        ),
        meclaw_core::serde_json::to_string(&porter_script()).unwrap(),
        meclaw_core::serde_json::to_string(&stdin_doc(body).to_string()).unwrap(),
    );
    let raw = python(src);
    meclaw_core::serde_json::from_str::<Value>(&raw)
        .unwrap_or_else(|e| panic!("porter emitted was not JSON ({e}): {raw}"))
        .as_array()
        .unwrap_or_else(|| panic!("an emission is an array: {raw}"))
        .clone()
}

/// Reads the script's own declaration tables back out of it — the same objects
/// the running cell walks. Nothing is re-typed here: what the test compares
/// against `store/config.json` is the artefact, not a copy of it.
fn declarations() -> Value {
    let src = format!(
        concat!(
            "import sys, io, json\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO('{{\"envelope\":{{}},\"body\":{{}},\"params\":{{}}}}')\n",
            "_sink, _real = io.StringIO(), sys.stdout\n",
            "sys.stdout = _sink\n",
            "try:\n",
            "    exec(compile(_script, 'cell', 'exec'), globals())\n",
            "except SystemExit:\n",
            "    pass\n",
            "finally:\n",
            "    sys.stdout = _real\n",
            "_real.write(json.dumps({{'SCHEMA': SCHEMA, 'WALK': WALK, 'KEYS': KEYS,\n",
            "                        'PROVENANCE': PROVENANCE, 'DERIVED': DERIVED,\n",
            "                        'ALIAS_DIM': ALIAS_DIM, 'REJECT_DIM': REJECT_DIM,\n",
            "                        'FORMAT': FORMAT}}))\n"
        ),
        meclaw_core::serde_json::to_string(&porter_script()).unwrap(),
    );
    meclaw_core::serde_json::from_str(&python(src)).expect("declaration dump is JSON")
}

fn route_of(msg: &Value) -> String {
    msg["header"]["route"].as_str().unwrap_or_default().into()
}

/// The store-native args of a `tool_call` turn.
fn args_of(msg: &Value) -> Value {
    let text = msg["messages"][0]["text"].as_str().unwrap_or("null");
    meclaw_core::serde_json::from_str(text).unwrap_or(Value::Null)
}

/// The document part carried by a `dump` message.
fn part_of(msg: &Value) -> Value {
    let text = msg["messages"][0]["text"].as_str().unwrap_or("null");
    meclaw_core::serde_json::from_str(text).unwrap_or(Value::Null)
}

fn store_ops(out: &[Value]) -> Vec<Value> {
    out.iter()
        .filter(|m| route_of(m) == "pstore")
        .map(args_of)
        .collect()
}

// ------------------------------------------------------------- vocabulary

const A: &str = "member:alex";
const S: &str = "agent:scribe";
const CH: &str = "tg:4711";

fn aud(members: &[&str]) -> String {
    meclaw_core::serde_json::to_string(members).unwrap()
}

/// One episode row as the store would return it, with the three provenance
/// columns of the audience gate on it.
fn episode_row(id: &str, audience: &str) -> Value {
    json!({"id": id, "session_id": "s-1", "turn_id": "t-1", "sender": "user",
           "speaker": A, "channel": CH, "audience_set": audience,
           "content": "alex prefers tea", "happened_at": "2026-08-19T10:00:00Z",
           "recorded_at": "2026-08-19T10:00:00Z"})
}

// ------------------------------------------------------- driving the export

struct Walk {
    /// The `select` the porter issued for each table, in walk order.
    selects: Vec<(String, Value)>,
    /// The document parts it emitted, in walk order.
    parts: Vec<Value>,
}

/// Runs the whole export walk of the shipped script, answering every read with
/// whatever `rows_for` says that table holds.
fn walk_export(rows_for: &dyn Fn(&str) -> Vec<Value>) -> Walk {
    let mut walk = Walk {
        selects: Vec::new(),
        parts: Vec::new(),
    };
    let mut out = run_hop(&json!({
        "header": {"context": {"mem_phase": "export"}, "hop": {}},
        "messages": []
    }));
    assert_eq!(out.len(), 1, "the walk starts with exactly one read");
    let mut next = out.remove(0);
    for _ in 0..64 {
        let table = next["header"]["port_table"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let run = next["header"]["port_run"]
            .as_str()
            .unwrap_or("")
            .to_string();
        walk.selects.push((table.clone(), args_of(&next)));
        let rows = rows_for(&table);
        let mut got = run_hop(&json!({
            "header": {"context": {"mem_phase": "export-page", "port_run": run,
                                   "port_table": table, "store_origin": "porter"},
                       "hop": {"operation": "select"}},
            "messages": [{"origin": "tool", "type": "tool_result",
                          "text": Value::Array(rows).to_string()}]
        }));
        assert_eq!(route_of(&got[0]), "dump", "a page answers with its part");
        walk.parts.push(part_of(&got[0]));
        if got.len() == 1 {
            return walk;
        }
        next = got.remove(1);
    }
    panic!("the export walk did not terminate");
}

// ------------------------------------------------------- driving the import

/// Runs one part through the whole import chain and returns the FINAL emission
/// — the ops that actually change the target store. `present` is what the
/// target already holds, as the probe would answer it.
fn import_part(part: &Value, present: &[Value]) -> Vec<Value> {
    let first = run_hop(&json!({
        "header": {"context": {"mem_phase": "import"}, "hop": {}},
        "messages": [{"origin": "assistant", "type": "text", "text": part.to_string()}]
    }));
    // A refusal, or a table the store keys itself: terminal on the first hop.
    if first.len() < 2 || route_of(&first[1]) != "pstore" {
        return first;
    }
    let probe = args_of(&first[1]);
    if probe["operation"] != "select" || probe["table"] == "scratch" {
        return first;
    }
    let key = first[0]["header"]["port_run"].as_str().unwrap().to_string();
    let table = part["table"].as_str().unwrap().to_string();
    let ctx = |phase: &str| {
        json!({"mem_phase": phase, "port_run": key, "port_table": table,
               "store_origin": "porter"})
    };
    let known = run_hop(&json!({
        "header": {"context": ctx("import-known"), "hop": {"operation": "select"}},
        "messages": [{"origin": "tool", "type": "tool_result",
                      "text": Value::Array(present.to_vec()).to_string()}]
    }));
    assert_eq!(known.len(), 1, "the probe answer is parked, nothing else");
    let merged = run_hop(&json!({
        "header": {"context": ctx("import-merge"), "hop": {"operation": "insert"}},
        "messages": [{"origin": "tool", "type": "tool_result", "text": "null"}]
    }));
    assert_eq!(args_of(&merged[0])["table"], "scratch");
    // The join-less store's meeting point: both sets under one key, read back
    // in a single select (the pattern `store/config.json` names).
    let scratch = json!([
        {"key": key, "kind": "part", "payload": part.to_string()},
        {"key": key, "kind": "known", "payload": Value::Array(present.to_vec()).to_string()},
    ]);
    run_hop(&json!({
        "header": {"context": ctx("import-apply"), "hop": {"operation": "select"}},
        "messages": [{"origin": "tool", "type": "tool_result", "text": scratch.to_string()}]
    }))
}

// =================================================================== claims

/// CLAIM 1 — the document covers exactly the content tables the store declares.
///
/// The script carries a mirror of `store/config.json` because a `script_inline`
/// cannot import one. That mirror is where a transfer silently starts dropping
/// a column: a column added to the store and not here would simply stop
/// travelling, and nothing would say so. So the mirror is compared against the
/// shipped declaration, column by column and type by type.
#[test]
fn the_document_covers_every_content_table_the_shipped_store_declares() {
    let Some(_) = hive_root() else { return };
    let store = config_at("store/config.json");
    let declared = store["params"]["schema"].as_object().unwrap();
    let decl = declarations();
    let mirror = decl["SCHEMA"].as_object().unwrap();

    // The three MACHINE tables are lane state, not memory, and `emb_models` is
    // the receiving hive's own configuration. All four are excluded on purpose;
    // this list IS the decision, so adding a table to the store without a
    // decision about it turns the assertion below red.
    let excluded = [
        "pending_extraction",
        "recall_scratch",
        "scratch",
        "emb_models",
    ];
    for (table, cols) in declared {
        if excluded.contains(&table.as_str()) {
            assert!(
                !mirror.contains_key(table),
                "{table} is on the exclusion list but travels anyway"
            );
            continue;
        }
        let mine = mirror
            .get(table)
            .unwrap_or_else(|| panic!("{table} is declared by the store but never travels"));
        assert_eq!(
            mine, cols,
            "the porter's mirror of {table} has drifted from store/config.json"
        );
    }
    // And the six tables the STORE creates itself out of `params.canonical`,
    // which `params.schema` never mentions — the alias table and the refusal
    // log of every identity dimension.
    for spec in store["params"]["canonical"]["facts"].as_array().unwrap() {
        for key in ["aliases", "rejected"] {
            let t = spec[key].as_str().unwrap();
            assert!(
                mirror.contains_key(t),
                "{t} is a store-owned table of the {} dimension and does not travel",
                spec["source"]
            );
        }
    }
    let walk: Vec<&str> = decl["WALK"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    let mut sorted = walk.clone();
    sorted.sort_unstable();
    let mut keys: Vec<&str> = mirror.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        sorted, keys,
        "the walk and the mirror name different tables"
    );
    // Order is load-bearing: the identity tables a fact derives its canonical
    // columns from have to be in the store before the fact is.
    let pos = |t: &str| walk.iter().position(|x| *x == t).unwrap();
    for dim in ["predicate", "subject", "claim"] {
        assert!(
            pos(&format!("{dim}_aliases")) < pos("facts"),
            "{dim}_aliases must travel before facts"
        );
    }
    assert!(pos("episodes") < pos("facts"), "episodes before facts");
}

/// CLAIM 2 — a part projects every column the table declares, provenance first.
///
/// The read path learned this the hard way (#244): a filter over a column
/// nobody selected is a filter that never fires. One axis further on, a column
/// nobody selects is a column that never leaves the store — and the three that
/// matter most are the ones that say who was there.
#[test]
fn every_part_projects_the_columns_the_audience_gate_lives_in() {
    let Some(_) = hive_root() else { return };
    let decl = declarations();
    let mirror = decl["SCHEMA"].as_object().unwrap();
    let walk = walk_export(&|_| vec![]);

    for (table, select) in &walk.selects {
        assert_eq!(select["operation"], "select", "{table}");
        assert_eq!(select["table"], table.as_str());
        let mut got: Vec<&str> = select["columns"]
            .as_array()
            .unwrap_or_else(|| panic!("{table} was read without a columns array"))
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        got.sort_unstable();
        let mut want: Vec<&str> = mirror[table]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        want.sort_unstable();
        assert_eq!(got, want, "{table} does not export every declared column");
        assert!(
            select.get("limit").is_none(),
            "{table} was read with a limit: a truncated part lies about being a table"
        );
        assert!(
            select.get("order_by").is_some(),
            "{table} was read unordered: two exports of one hive would not be diffable"
        );
    }
    for (table, cols) in decl["PROVENANCE"].as_object().unwrap() {
        let select = &walk
            .selects
            .iter()
            .find(|(t, _)| t == table)
            .unwrap_or_else(|| panic!("{table} is audience-bearing but never read"))
            .1;
        for col in cols.as_array().unwrap() {
            assert!(
                select["columns"].as_array().unwrap().contains(col),
                "{table} is exported without {col}"
            );
        }
    }
    // The one identity column that is neither audience nor key and would still
    // be lost silently.
    let eps = &walk
        .selects
        .iter()
        .find(|(t, _)| t == "episodes")
        .unwrap()
        .1;
    assert!(
        eps["columns"]
            .as_array()
            .unwrap()
            .contains(&json!("speaker")),
        "episodes are exported without who spoke"
    );
}

/// CLAIM 3 — a part is a seed file, and the walk says where it ends.
#[test]
fn a_part_carries_the_seed_header_and_the_walk_declares_its_last_one() {
    let Some(_) = hive_root() else { return };
    let store = config_at("store/config.json");
    let decl = declarations();
    let format = decl["FORMAT"].as_str().unwrap();
    let walk = walk_export(&|t| {
        if t == "episodes" {
            vec![episode_row("e1", &aud(&[S, A]))]
        } else {
            vec![]
        }
    });

    assert_eq!(
        walk.parts.len(),
        decl["WALK"].as_array().unwrap().len(),
        "one part per table, no more and no fewer"
    );
    let last = walk.parts.len();
    for (i, part) in walk.parts.iter().enumerate() {
        assert_eq!(
            part["format"], format,
            "part {i} declares no format version"
        );
        assert_eq!(part["part"], json!(i + 1));
        assert_eq!(part["of"], json!(last));
        assert_eq!(part["final"], json!(i + 1 == last));
        // The header line of a `seed/<table>.jsonl`, byte for byte: the store's
        // own declaration for that table. This is what makes writing a part out
        // as a seed file mechanical rather than an interpretation.
        let table = part["table"].as_str().unwrap();
        if let Some(declared) = store["params"]["schema"].get(table) {
            assert_eq!(
                part["schema"], *declared,
                "the {table} part's schema header is not the store's declaration"
            );
        }
        assert!(part["rows"].is_array(), "part {i} carries no rows array");
    }
    // One export id for the whole document — a transfer is one thing.
    let ids: std::collections::BTreeSet<&str> = walk
        .parts
        .iter()
        .map(|p| p["export_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 1, "the parts of one export disagree on its id");

    let episodes = walk
        .parts
        .iter()
        .find(|p| p["table"] == "episodes")
        .unwrap();
    assert_eq!(episodes["rows"][0]["audience_set"], json!(aud(&[S, A])));
    assert_eq!(episodes["rows"][0]["speaker"], json!(A));
    assert_eq!(episodes["rows"][0]["channel"], json!(CH));
}

/// CLAIM 4a — provenance survives the transfer byte for byte.
///
/// The import copies the participant set, the room and the speaker exactly as
/// the source decided them. It never intersects again, never defaults, never
/// falls back to the role — provenance is never rewritten (ADR-0002 E12).
#[test]
fn an_imported_row_arrives_with_the_audience_it_was_exported_with() {
    let Some(_) = hive_root() else { return };
    let audience = aud(&[S, A]);
    let walk = walk_export(&|t| {
        if t == "episodes" {
            vec![episode_row("e1", &audience)]
        } else {
            vec![]
        }
    });
    let part = walk
        .parts
        .iter()
        .find(|p| p["table"] == "episodes")
        .unwrap()
        .clone();

    let out = import_part(&part, &[]);
    let ops = store_ops(&out);
    let inserts: Vec<&Value> = ops.iter().filter(|o| o["operation"] == "insert").collect();
    assert_eq!(inserts.len(), 1, "the one exported episode was not written");
    let row = &inserts[0]["row"];
    assert_eq!(inserts[0]["table"], "episodes");
    assert_eq!(
        row["audience_set"],
        json!(audience),
        "the audience was lost"
    );
    assert_eq!(row["channel"], json!(CH), "the room was lost");
    assert_eq!(row["speaker"], json!(A), "who spoke was lost");
    assert_eq!(row["content"], json!("alex prefers tea"));
    assert_eq!(row["happened_at"], json!("2026-08-19T10:00:00Z"));
}

/// CLAIM 4b — a part that lost a provenance column is refused, and NOTHING is
/// written.
///
/// This is the failure the lane exists to make impossible. An imported row
/// whose participant set did not survive is a row that may be told to anyone,
/// and no downstream can reconstruct one honestly. A refusal and a silent
/// untagged write are different answers.
#[test]
fn a_part_that_lost_its_audience_column_is_refused_and_writes_nothing() {
    let Some(_) = hive_root() else { return };
    let decl = declarations();
    let walk = walk_export(&|t| {
        if t == "episodes" {
            vec![episode_row("e1", &aud(&[S, A]))]
        } else {
            vec![]
        }
    });
    let good = walk
        .parts
        .iter()
        .find(|p| p["table"] == "episodes")
        .unwrap()
        .clone();

    for (col, reason) in [
        ("audience_set", "missing_audience"),
        ("channel", "missing_channel"),
    ] {
        let mut part = good.clone();
        part["schema"].as_object_mut().unwrap().remove(col);
        for row in part["rows"].as_array_mut().unwrap() {
            row.as_object_mut().unwrap().remove(col);
        }
        let out = run_hop(&json!({
            "header": {"context": {"mem_phase": "import"}, "hop": {}},
            "messages": [{"origin": "assistant", "type": "text", "text": part.to_string()}]
        }));
        assert_eq!(out.len(), 1, "a refusal emits one message and no store op");
        assert_eq!(route_of(&out[0]), "reject");
        assert_eq!(out[0]["header"]["reject_reason"], json!(reason));
        assert_eq!(out[0]["header"]["port_table"], json!("episodes"));
        assert!(
            store_ops(&out).is_empty(),
            "a refused part reached the store anyway"
        );
    }

    // Every audience-bearing table refuses on the same terms — the gate is not
    // an episodes special case.
    for (table, cols) in decl["PROVENANCE"].as_object().unwrap() {
        let mut part = json!({
            "format": decl["FORMAT"], "table": table, "part": 1, "of": 1, "final": false,
            "schema": decl["SCHEMA"][table], "rows": [{"id": "x"}],
        });
        for col in cols.as_array().unwrap() {
            part["schema"]
                .as_object_mut()
                .unwrap()
                .remove(col.as_str().unwrap());
        }
        let out = run_hop(&json!({
            "header": {"context": {"mem_phase": "import"}, "hop": {}},
            "messages": [{"origin": "assistant", "type": "text", "text": part.to_string()}]
        }));
        assert_eq!(
            route_of(&out[0]),
            "reject",
            "{table} accepted an untagged part"
        );
        assert!(store_ops(&out).is_empty(), "{table} wrote an untagged row");
    }
}

/// CLAIM 5 — an empty audience stays empty. It is never invented.
///
/// A row from before the gate is invisible (contract ruling R2, evaluated
/// first). Transferring it preserves that; degrading it to `["*"]` or to the
/// asking round would BE the laundering, one axis over from where #244 stopped
/// it.
#[test]
fn an_empty_audience_survives_the_transfer_as_an_empty_one() {
    let Some(_) = hive_root() else { return };
    let walk = walk_export(&|t| {
        if t == "episodes" {
            vec![episode_row("legacy", "")]
        } else {
            vec![]
        }
    });
    let part = walk
        .parts
        .iter()
        .find(|p| p["table"] == "episodes")
        .unwrap()
        .clone();
    // The column is declared, so the part is not a LOSS in transit — it is a
    // faithful copy of a row that never had an audience. Fail-closed applies to
    // loss, not to absence at the source: refusing here would make a pre-gate
    // hive unmovable.
    let ops = store_ops(&import_part(&part, &[]));
    let inserts: Vec<&Value> = ops.iter().filter(|o| o["operation"] == "insert").collect();
    assert_eq!(inserts.len(), 1);
    assert_eq!(
        inserts[0]["row"]["audience_set"],
        json!(""),
        "an untagged row was given an audience it never had"
    );
}

/// CLAIM 6 — the same document applied twice writes nothing the second time.
///
/// `params.schema` declares no keys, so a repeated insert would simply
/// duplicate. Idempotency is what makes the document a backup and a merge
/// rather than only a birth seed, and it is bought with the probe.
#[test]
fn the_same_document_applied_twice_writes_nothing_the_second_time() {
    let Some(_) = hive_root() else { return };
    let walk = walk_export(&|t| {
        if t == "episodes" {
            vec![
                episode_row("e1", &aud(&[S, A])),
                episode_row("e2", &aud(&[A])),
            ]
        } else {
            vec![]
        }
    });
    let part = walk
        .parts
        .iter()
        .find(|p| p["table"] == "episodes")
        .unwrap()
        .clone();

    let fresh = store_ops(&import_part(&part, &[]));
    assert_eq!(
        fresh.iter().filter(|o| o["operation"] == "insert").count(),
        2,
        "a fresh target did not take both rows"
    );

    // Second run: the probe finds both keys already there.
    let out = import_part(&part, &[json!({"id": "e1"}), json!({"id": "e2"})]);
    assert!(
        store_ops(&out).is_empty(),
        "the second application wrote rows the target already had"
    );
    let receipt = out.iter().find(|m| route_of(m) == "dump").expect("receipt");
    assert_eq!(receipt["header"]["dump_kind"], json!("import_receipt"));
    assert_eq!(receipt["header"]["rows_written"], json!(0));

    // Half applied is half written, and only the half that is missing.
    let half = store_ops(&import_part(&part, &[json!({"id": "e1"})]));
    let inserts: Vec<&Value> = half.iter().filter(|o| o["operation"] == "insert").collect();
    assert_eq!(inserts.len(), 1);
    assert_eq!(inserts[0]["row"]["id"], json!("e2"));
}

/// CLAIM 7 — the store-keyed families arrive as upserts, never as inserts.
///
/// This is the half of #243 the JSONL seeder cannot reach. The seeder builds a
/// table out of the header line alone — column and type, no key — so a
/// `seed/claim_aliases.jsonl` wins with a table that has no PRIMARY KEY and
/// silently costs `set_alias` the upsert property the nightly GC depends on.
/// A hive that receives its aliases through this lane never has that problem:
/// the table was created by the store with its key, and only the op that owns
/// it ever writes it.
#[test]
fn the_store_keyed_identity_tables_arrive_as_upserts_and_never_as_inserts() {
    let Some(_) = hive_root() else { return };
    let decl = declarations();
    let walk = walk_export(&|t| match t {
        "predicate_aliases" => vec![json!({"alias": "Lieblingseditor",
                                           "canonical": "favorite_editor",
                                           "recorded_at": "2026-08-19T03:00:00Z"})],
        "subject_rejected_pairs" => vec![json!({"left_value": "robin", "right_value": "robyn",
                                                "recorded_at": "2026-08-19T03:00:00Z"})],
        _ => vec![],
    });

    let alias = walk
        .parts
        .iter()
        .find(|p| p["table"] == "predicate_aliases")
        .unwrap();
    let out = run_hop(&json!({
        "header": {"context": {"mem_phase": "import"}, "hop": {}},
        "messages": [{"origin": "assistant", "type": "text", "text": alias.to_string()}]
    }));
    let ops = store_ops(&out);
    assert_eq!(ops.len(), 1, "one alias, one op");
    assert_eq!(ops[0]["operation"], "set_alias");
    // The op names the BOUND table and the dimension, never the alias table —
    // the binding in `params.canonical` resolves the rest.
    assert_eq!(ops[0]["table"], "facts");
    assert_eq!(ops[0]["column"], "predicate");
    assert_eq!(ops[0]["alias"], "Lieblingseditor");
    assert_eq!(ops[0]["canonical"], "favorite_editor");
    assert_eq!(ops[0]["recorded_at"], "2026-08-19T03:00:00Z");

    let refused = walk
        .parts
        .iter()
        .find(|p| p["table"] == "subject_rejected_pairs")
        .unwrap();
    let ops = store_ops(&run_hop(&json!({
        "header": {"context": {"mem_phase": "import"}, "hop": {}},
        "messages": [{"origin": "assistant", "type": "text", "text": refused.to_string()}]
    })));
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0]["operation"], "reject_pair");
    assert_eq!(ops[0]["table"], "facts");
    assert_eq!(ops[0]["column"], "subject");
    assert_eq!(ops[0]["left"], "robin");
    assert_eq!(ops[0]["right"], "robyn");

    // No keyed table has an insert path at all: it is not in KEYS, so the
    // probe-and-insert branch can never be reached for it.
    let keys = decl["KEYS"].as_object().unwrap();
    for family in ["ALIAS_DIM", "REJECT_DIM"] {
        for table in decl[family].as_object().unwrap().keys() {
            assert!(
                !keys.contains_key(table),
                "{table} has an insert key and could be inserted into"
            );
        }
    }
}

/// CLAIM 8 — the final part re-derives every identity dimension.
///
/// A fact's canonical columns are store-owned: they travel in the document for
/// diffability but are stripped before insert and derived again from the alias
/// tables that travelled too. That is transfer, not re-judgement — a
/// deterministic function of transferred data, with no model in it. The
/// canonicalise on the last part is what repairs a document applied in any
/// other order than the one the export emits.
#[test]
fn the_final_part_re_derives_every_identity_dimension() {
    let Some(_) = hive_root() else { return };
    let store = config_at("store/config.json");
    let decl = declarations();
    let walk = walk_export(&|t| {
        if t == "facts" {
            vec![json!({"id": "f1", "episode_id": "e1", "session_id": "s-1",
                        "channel": CH, "audience_set": aud(&[S, A]),
                        "subject": "alex", "canonical_subject": "member:alex",
                        "predicate": "Lieblingseditor", "canonical_predicate": "favorite_editor",
                        "claim": "vim", "canonical_claim": "vim", "claim_hash": "h",
                        "fact_kind": "preference", "valid_from": "2026-08-19T10:00:00Z",
                        "valid_until": "", "recorded_at": "2026-08-19T10:00:00Z",
                        "expired_at": "", "superseded_by": "", "closure_source": "",
                        "confidence": 90})]
        } else {
            vec![]
        }
    });

    // The derived columns are IN the document …
    let facts_part = walk.parts.iter().find(|p| p["table"] == "facts").unwrap();
    for col in decl["DERIVED"]["facts"].as_array().unwrap() {
        assert!(
            facts_part["rows"][0].get(col.as_str().unwrap()).is_some(),
            "the document dropped {col}, so it cannot be diffed against its source"
        );
    }
    // … and OUT of the insert, because the store owns them.
    let ops = store_ops(&import_part(facts_part, &[]));
    let row = &ops[0]["row"];
    for col in decl["DERIVED"]["facts"].as_array().unwrap() {
        assert!(
            row.get(col.as_str().unwrap()).is_none(),
            "the import asserted {col}, which the store derives"
        );
    }
    assert_eq!(
        row["predicate"],
        json!("Lieblingseditor"),
        "written value lost"
    );
    assert_eq!(row["audience_set"], json!(aud(&[S, A])));

    // The last part canonicalises every declared dimension, once each.
    let last = walk.parts.last().unwrap();
    assert_eq!(last["final"], json!(true));
    let ops = store_ops(&import_part(last, &[]));
    let dims: std::collections::BTreeSet<&str> = ops
        .iter()
        .filter(|o| o["operation"] == "canonicalize")
        .map(|o| o["column"].as_str().unwrap())
        .collect();
    let declared: std::collections::BTreeSet<&str> = store["params"]["canonical"]["facts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["source"].as_str().unwrap())
        .collect();
    assert_eq!(
        dims, declared,
        "the transfer does not re-derive every identity dimension the store binds"
    );
    for o in ops.iter().filter(|o| o["operation"] == "canonicalize") {
        assert_eq!(o["table"], "facts");
    }
}

/// CLAIM 9 — a document this lane cannot read is refused, never guessed at.
#[test]
fn a_document_this_lane_cannot_read_is_refused_rather_than_guessed_at() {
    let Some(_) = hive_root() else { return };
    let decl = declarations();
    let cases: Vec<(Value, &str)> = vec![
        (json!({"table": "episodes", "rows": []}), "import_format"),
        (
            json!({"format": "some-other-format/9", "table": "episodes", "rows": []}),
            "import_format",
        ),
        (
            json!({"format": decl["FORMAT"], "table": "scratch", "rows": [],
                   "schema": {"key": "text"}}),
            "import_unknown_table",
        ),
        (
            json!({"format": decl["FORMAT"], "table": "entities", "rows": []}),
            "import_schema_drift",
        ),
        (
            json!({"format": decl["FORMAT"], "table": "entities", "rows": [],
                   "schema": {"id": "text", "canonical_name": "text", "kind": "text",
                              "aliases": "json", "tomorrows_column": "text"}}),
            "import_schema_drift",
        ),
    ];
    for (body, reason) in cases {
        let out = run_hop(&json!({
            "header": {"context": {"mem_phase": "import"}, "hop": {}},
            "messages": [{"origin": "assistant", "type": "text", "text": body.to_string()}]
        }));
        assert_eq!(out.len(), 1, "{reason}: a refusal is one message");
        assert_eq!(route_of(&out[0]), "reject", "{reason}");
        assert_eq!(out[0]["header"]["reject_reason"], json!(reason));
        assert!(
            store_ops(&out).is_empty(),
            "{reason} still reached the store"
        );
    }
}

/// CLAIM 10 — the hive declares the lanes and makes their drains mandatory.
///
/// A lane is a public contract surface. An export nobody drains reads the whole
/// store and delivers it to nobody; a refused transfer nobody drains looks
/// exactly like a successful one.
#[test]
fn the_hive_declares_the_transfer_lanes_and_makes_their_drains_mandatory() {
    let Some(_) = hive_root() else { return };
    let hive = hive_config();
    let params = &hive["params"];
    let accepts: Vec<&str> = params["contract"]["accepts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["route"].as_str().unwrap())
        .collect();
    let emits: Vec<&str> = params["contract"]["emits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["route"].as_str().unwrap())
        .collect();
    assert!(accepts.contains(&"in_export"), "no export lane");
    assert!(accepts.contains(&"in_import"), "no import lane");
    assert!(emits.contains(&"dump"), "no lane the document leaves on");

    let drains: Vec<(&str, &str)> = params["required_drains"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| (d["accepts"].as_str().unwrap(), d["emits"].as_str().unwrap()))
        .collect();
    for pair in [
        ("in_export", "dump"),
        ("in_export", "reject"),
        ("in_import", "dump"),
        ("in_import", "reject"),
    ] {
        assert!(drains.contains(&pair), "{pair:?} is not an enforced drain");
    }

    // The hive is an island with no interior address (GH #197): the lanes reach
    // the porter through the hive's OWN graph, and nothing outside names it.
    assert_eq!(params["ports"], json!([]));
    let edges = params["graph"]["edges"].as_array().unwrap();
    let has = |from: &str, to: &str, needle: &str| {
        edges.iter().any(|e| {
            e["from"] == from
                && e["to"] == to
                && e["condition"].as_str().unwrap_or_default().contains(needle)
        })
    };
    assert!(
        has(".", "./porter", "in_export"),
        "in_export reaches nobody"
    );
    assert!(
        has(".", "./porter", "in_import"),
        "in_import reaches nobody"
    );
    assert!(
        has("./porter", "./store", "pstore"),
        "the porter cannot read"
    );
    assert!(has("./porter", ".", "dump"), "the document cannot leave");
    assert!(has("./porter", ".", "reject"), "a refusal cannot leave");
    // The reply edge is what makes the state machine a chain; without it the
    // walk stops after its first read.
    assert!(
        edges.iter().any(|e| e["from"] == "./store"
            && e["to"] == "./porter"
            && e["condition"]
                .as_str()
                .unwrap_or_default()
                .contains("store_origin == 'porter'")),
        "store replies never come back to the porter"
    );
    // Both door edges stamp the phase themselves, so a caller carrying a stale
    // `mem_phase` from an earlier question (GH #152 — context is persistent)
    // cannot start the walk in the middle of it.
    for lane in ["in_export", "in_import"] {
        let e = edges
            .iter()
            .find(|e| {
                e["from"] == "."
                    && e["to"] == "./porter"
                    && e["condition"].as_str().unwrap_or_default().contains(lane)
            })
            .unwrap();
        assert!(
            e["modifier"]["set_context"]["mem_phase"].is_string(),
            "{lane} does not stamp its own phase"
        );
        assert_eq!(
            e["modifier"]["set_context"]["store_origin"],
            json!("'porter'")
        );
    }
}

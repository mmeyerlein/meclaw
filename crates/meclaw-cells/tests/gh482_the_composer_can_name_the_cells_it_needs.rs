//! GH #482 direction 2 -- the composer could not name a `code` cell, a `store`
//! or a `web_fetch`, so a unit made of ordinary cells had no parts to build from.
//!
//! Measured on a throwaway colony with a hosted model as the composer, on the
//! first wish no recipe covered: *"a feed cell that fetches three feeds every
//! ten minutes and emits one headline document per new item"*. It spent all
//! seven rounds, and every one of them well, looking for four templates that
//! did not exist -- `timer`, `web_fetch`, `code`, `store`. A feed is exactly
//! those four cells, and the library shipped a single-cell template for none of
//! them. `templates/_cell-types/README.md` said so deliberately: a type whose
//! shape a living template already carries needs no skeleton.
//!
//! That sentence answers *where an author copies a form from*. It does not
//! answer *what a mutation can name*, and the two are different questions:
//! `add_nodes` resolves a template by `name@version` out of the registry, so the
//! shape inside `collector` is not addressable. `clock@1.0.0` closed the first
//! quarter of the gap (#484); this closes the other three.
//!
//! Four claims are pinned here, and the last one is the issue itself:
//!
//! 1. EACH TEMPLATE IS ONE CELL OF ONE TYPE AND NOTHING ELSE. A skeleton that
//!    carried a second cell, or a purpose, would be one more composite the
//!    composer has to take apart.
//! 2. NOTHING IN THEM IS A `${uuid7}` TOKEN. That substitution has no
//!    filesystem-side producer, so a tree written straight to disk refuses to
//!    BOOT on it -- and the engine reads the WHOLE config, so a config that
//!    merely NAMES the token in its prose is refused just the same (#484).
//! 3. THE PROSE AND THE CONFIG NAME THE SAME DEFAULTS (development-rules § 2d).
//!    The numbers are derived from the config inside the test.
//! 4. A MANIFEST CAN BUILD THE FEED THE COMPOSER COULD NOT NAME. A colony boots
//!    with none of the four, ONE mutation instantiates all of them and wires
//!    them, and a headline document lands in the store without a restart.

use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::{TimerCellFactory, WebFetchCellFactory};
use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{CellFactory, CellFactoryRegistry, ColonyMsg, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mock_http::{MockResponse, start_mock_server};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// The three templates this issue adds, and the cell type each one IS.
const NEW: [(&str, &str); 3] = [
    ("fetcher", "web_fetch"),
    ("scriptlet", "code"),
    ("shelf", "store"),
];

/// The clock from #484 completes the set the feed needs.
const FEED: [&str; 4] = ["clock", "fetcher", "scriptlet", "shelf"];

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn template_dir(name: &str) -> std::path::PathBuf {
    repo("templates").join(name)
}

/// A README with its line wrapping taken out, so a sentence the prose breaks
/// across two lines is still one sentence to `contains`. Prose is wrapped for
/// readers; a drift lock that could be defeated by a reflow would be a lock on
/// the formatting rather than on the claim.
fn flow(readme: &str) -> String {
    readme.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn json_at(template: &str, rel: &str) -> Value {
    let raw = std::fs::read_to_string(template_dir(template).join(rel))
        .unwrap_or_else(|e| panic!("templates/{template}/{rel}: {e}"));
    meclaw_core::serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("templates/{template}/{rel} is not JSON: {e}"))
}

// ===================================================================== THE FORM

#[test]
fn each_new_template_is_one_cell_of_the_type_it_stands_for() {
    for (name, cell_type) in NEW {
        let tpl = json_at(name, "template.json");
        assert_eq!(tpl["name"], json!(name));
        // The three shipped at 1.0.0 and stay on the 1 line; the exact digit
        // is NOT restated here (§ 4a -- a present-tense version claim written
        // in two places has one place that is wrong from the next third-digit
        // repair on, and `scriptlet` took one for GH #513). What is pinned is
        // the PAIR: descriptor and README heading name the same full version,
        // asserted below where the reason for it is written down.
        let version = tpl["version"].as_str().expect("a declared version");
        assert!(
            version.starts_with("1."),
            "{name} is on the 1 line: {version}"
        );
        for slot in ["purpose", "use_when", "not_in_scope", "examples"] {
            assert!(
                !tpl["description"][slot].is_null(),
                "{name}/template.json needs the {slot} slot the catalogue serves"
            );
        }

        let cfg = json_at(name, "config.json");
        assert_eq!(
            cfg["cell"]["type"],
            json!(cell_type),
            "the whole content of {name} is one {cell_type} cell"
        );

        // "One cell and nothing else" as a property of the DIRECTORY: a sub-cell
        // would be a second actor with an opinion, which is what a composer
        // taking a composite apart already has too much of.
        let mut files: Vec<String> = std::fs::read_dir(template_dir(name))
            .unwrap_or_else(|e| panic!("templates/{name}: {e}"))
            .map(|e| {
                let e = e.expect("dir entry");
                assert!(
                    !e.file_type().expect("file type").is_dir(),
                    "{name} has no sub-cells: {:?}",
                    e.path()
                );
                e.file_name().to_string_lossy().to_string()
            })
            .collect();
        files.sort();
        assert_eq!(
            files,
            vec![
                "README.md".to_string(),
                "config.json".to_string(),
                "template.json".to_string()
            ],
            "{name}: three files -- the declaration, the cell, and the prose"
        );

        // The name is its own word, not one taken from the substrate glossary
        // (development-rules § 3, ruled as Q19): the second element is a cell
        // TYPE, and a template named after one is a review defect.
        assert_ne!(
            name, cell_type,
            "a template named after a built-in cell type is a review defect"
        );

        // The README's H1 is the catalogue's own version claim, and #335 reads
        // it; assert the pair here too, where the reason for it is written down.
        let raw = std::fs::read_to_string(template_dir(name).join("README.md"))
            .unwrap_or_else(|e| panic!("templates/{name}/README.md: {e}"));
        let readme = flow(&raw);
        assert_eq!(
            raw.lines().next().unwrap_or_default(),
            format!("# `{name}@{version}`"),
            "{name}: the README names itself at the version it ships"
        );
        assert!(
            readme.contains("a **flat** params object"),
            "{name}: the README must publish the override form the validator \
             accepts -- a single-cell template has no inner cell to address, and \
             the path-keyed form is refused with `schema`"
        );
        assert!(
            readme.contains("issues/482"),
            "{name}: the README says which absence it fills"
        );
    }
}

#[test]
fn nothing_in_them_is_an_instantiation_substitution() {
    for (name, _) in NEW {
        for file in ["config.json", "template.json", "README.md"] {
            let raw = std::fs::read_to_string(template_dir(name).join(file))
                .unwrap_or_else(|e| panic!("templates/{name}/{file}: {e}"));
            assert!(
                !raw.contains("${uuid7"),
                "templates/{name}/{file}: an instantiation substitution has no \
                 filesystem-side producer, so a tree written straight to disk \
                 refuses to boot on it -- and the engine reads the WHOLE config, \
                 so naming the token in prose is refused just the same (#484)"
            );
        }
    }
}

/// Drift lock (development-rules § 2d): every number in the knob tables is
/// DERIVED from the shipped config here rather than typed into the assertion.
#[test]
fn the_prose_and_the_config_name_the_same_defaults() {
    for (name, _) in NEW {
        let cfg = json_at(name, "config.json");
        let readme = flow(
            &std::fs::read_to_string(template_dir(name).join("README.md"))
                .unwrap_or_else(|e| panic!("templates/{name}/README.md: {e}")),
        );
        let params = cfg["params"].as_object().expect("params object");
        assert!(
            !params.is_empty(),
            "{name}: a skeleton with no parameter is a skeleton nobody can aim"
        );
        for (key, value) in params {
            assert!(
                readme.contains(&format!("`{key}`")),
                "{name}: the knob table must name the param `{key}` it ships"
            );
            // Only the scalars: a schema and a script are shown in their own
            // blocks, and quoting them into a table would be unreadable.
            if value.is_number() || value.is_boolean() {
                assert!(
                    readme.contains(&format!("`{value}`")),
                    "{name}: the knob table claims a default for `{key}` that \
                     the config does not ship ({value})"
                );
            }
        }
    }

    // Each of the three names the ONE thing it exists to be given.
    for (name, marker) in [
        ("fetcher", "no URL of its own"),
        ("scriptlet", "shipped blank"),
        ("shelf", "no opinion about what goes on it"),
    ] {
        let readme = flow(
            &std::fs::read_to_string(template_dir(name).join("README.md"))
                .unwrap_or_else(|e| panic!("templates/{name}/README.md: {e}")),
        );
        assert!(
            readme.to_lowercase().contains(&marker.to_lowercase()),
            "{name}: the README states what this template is; the assertions \
             above are what keep the sentence true"
        );
    }
}

/// The `scriptlet` declares multi-send in its CONTRACT, and that is the half an
/// `override_params` cannot repair: a manifest may hand the cell another script,
/// never another contract. A blank `code` cell that could emit only one message
/// would be half a code cell.
#[test]
fn the_scriptlet_declares_the_half_an_override_cannot_reach() {
    let cfg = json_at("scriptlet", "config.json");
    assert_eq!(
        cfg["contract"]["multi_send_capable"],
        json!(true),
        "a replacement script that writes an array is the point"
    );
    assert!(
        cfg["params"]["script_inline"].is_string(),
        "the logic is a PARAM, so a declaration can hand it over"
    );
    assert!(
        cfg["params"]["sandbox"].is_null(),
        "no sandbox key: a cell instantiated without one gets the default-deny \
         profile, and a param the template does not declare is a param no \
         override_params can set"
    );
    let readme = flow(
        &std::fs::read_to_string(template_dir("scriptlet").join("README.md")).expect("README.md"),
    );
    assert!(
        readme.contains("code.author"),
        "an override_params carrying a script is executable behaviour arriving \
         with a manifest, and the README names the price"
    );
    assert!(
        readme.contains("code_author_denied"),
        "the refusal means THIS COLONY DOES NOT ALLOW IMPORTED EXECUTION and \
         never that the manifest was malformed -- a composer reading it as a \
         form error repairs a draft that was never broken"
    );
}

/// The `shelf` hands its shape over: `params.schema` is a bootstrap declaration,
/// so an override at instantiation is baked into the `cell.db` by DDL before the
/// cell ever wakes. That is what makes one template two shelves.
#[test]
fn the_shelf_hands_its_shape_over() {
    let cfg = json_at("shelf", "config.json");
    let schema = cfg["params"]["schema"].as_object().expect("params.schema");
    assert_eq!(
        schema.len(),
        1,
        "one table: a skeleton that shipped two would be deciding which of them \
         a caller meant"
    );
    for (table, cols) in schema {
        let cols = cols
            .as_object()
            .unwrap_or_else(|| panic!("{table} columns"));
        for (col, ty) in cols {
            assert!(
                ["text", "int", "json"].contains(&ty.as_str().unwrap_or_default()),
                "{table}.{col}: a store column is text, int or json"
            );
        }
    }
    for absent in ["fts", "canonical", "write_surface"] {
        assert!(
            cfg["params"][absent].is_null(),
            "{absent} is a real declaration of the cell type and this skeleton \
             does not carry it -- which also puts it out of an override's reach, \
             because an override names a param the template already declares"
        );
    }
}

// =================================================================== THE COLONY

fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![
        (
            "timer".to_string(),
            Arc::new(TimerCellFactory) as Arc<dyn CellFactory>,
        ),
        ("web_fetch".to_string(), Arc::new(WebFetchCellFactory)),
        ("code".to_string(), Arc::new(CodeCellFactory)),
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

/// A colony with a root hive, no feed, and a library that holds the four.
fn tree() -> tempfile::TempDir {
    let td = tempfile::TempDir::new().expect("tempdir");
    let root = td.path();
    std::fs::create_dir_all(root.join("main")).expect("root hive dir");
    std::fs::write(
        root.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .expect("write the root hive");
    for name in FEED {
        copy_tree(&template_dir(name), &root.join("templates").join(name));
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
        .expect("a colony without a feed boots");
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

async fn mutate(h: &ColonyHandle, payload: Value) -> meclaw_colony::MutationOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to: None,
            trace_id: meclaw_core::Uuid::now_v7(),
            parent_message_id: meclaw_core::Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("mutation sent");
    ack_rx.await.expect("mutation acked")
}

async fn registry_paths(h: &ColonyHandle) -> Vec<String> {
    let (ack_tx, ack_rx) = oneshot::channel::<ReadRegistryReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: None,
            path_prefix: None,
            cell_type: None,
            active: None,
            limit: 1000,
            ack: ack_tx,
        })
        .await
        .expect("registry read sent");
    ack_rx
        .await
        .expect("registry read acked")
        .entries
        .into_iter()
        .map(|e| e.path)
        .collect()
}

fn turn_texts(m: &Message) -> Vec<String> {
    match &m.body {
        Body::Inline(v) => v["messages"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|t| t["text"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default(),
        Body::Blob(_) => Vec::new(),
    }
}

/// The script that turns a tick into the one request this feed makes. A clock
/// has nothing to say, so somebody has to say it -- and that somebody is a
/// `scriptlet`, which is precisely the cell the composer could not name.
fn ask_script(url: &str) -> String {
    format!(
        "import sys, json\n\
         json.load(sys.stdin)\n\
         call = {{\"origin\": \"assistant\", \"type\": \"tool_call\", \"id\": \"feed-1\",\n\
         \x20       \"text\": json.dumps({{\"url\": \"{url}\"}})}}\n\
         sys.stdout.write(json.dumps({{\"header\": {{\"route\": \"fetch\"}},\n\
         \x20                          \"messages\": [call]}}))\n"
    )
}

/// The dedupe half of the feed: one headline document per item, keyed by a
/// stable id the script computes before it inserts. A `store` ships no
/// constraints, so this is where dedupe lives -- with whoever writes.
const DEDUPE_SCRIPT: &str = r#"import sys, json, hashlib
from datetime import datetime, timezone

doc = json.load(sys.stdin)
body = doc["body"]

payload = ""
for m in body.get("messages") or []:
    if m.get("type") == "tool_result" and m.get("text"):
        payload = m["text"]

try:
    items = json.loads(payload).get("items") or []
except Exception:
    items = []

out = []
seen = set()
for item in items:
    link = item.get("link") or ""
    key = hashlib.sha256(link.encode("utf-8")).hexdigest()[:16]
    if not link or key in seen:
        continue
    seen.add(key)
    args = {"operation": "insert", "table": "headlines",
            "row": {"id": key, "at": datetime.now(timezone.utc).isoformat(),
                    "title": item.get("title") or "", "link": link}}
    out.append({"header": {"route": "store", "tool_call_id": key},
                "messages": [{"origin": "assistant", "type": "tool_call",
                              "id": key, "text": json.dumps(args)}]})
sys.stdout.write(json.dumps(out))
"#;

/// **The load-bearing test.** The wish the composer could not answer, run
/// against the library: ONE manifest names four templates and five nodes, draws
/// the edges between them, and a headline document is in the store one tick
/// later -- without a restart, and without anybody writing a class.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_manifest_builds_the_feed_out_of_four_named_templates() {
    let feed_json =
        br#"{"items":[{"title":"a substrate learns to fetch","link":"https://example.org/1"},
                                  {"title":"the same item again","link":"https://example.org/1"},
                                  {"title":"a second headline","link":"https://example.org/2"}]}"#;
    let (addr, _srv) = start_mock_server(MockResponse::ok_json(feed_json)).await;

    let td = tree();
    let (h, mut rx) = boot(&td).await;

    let before = registry_paths(&h).await;
    for node in ["/tick", "/ask", "/feed", "/dedupe", "/headlines"] {
        assert!(
            !before.iter().any(|p| p == node),
            "the point of the exercise is that the colony had no feed: {before:?}"
        );
    }

    let outcome = mutate(
        &h,
        json!({"scope": "/", "diff": {
            "add_nodes": [
                {"name": "tick", "template": "clock@1.0.0",
                 "override_params": {"schedules": [{
                     "schedule_id": "0190a3f2-0000-7000-8000-000000000482",
                     "schedule_name": "tick", "cron": "* * * * * *",
                     "emit_to": ".", "emit_body": {"messages": []},
                     "emit_headers": {}}]}},
                {"name": "ask", "template": "scriptlet@1.0.1",
                 "override_params": {"script_inline": ask_script(&format!("http://{addr}/feed.json"))}},
                {"name": "feed", "template": "fetcher@1.0.0",
                 "override_params": {"allow_private_networks": true, "external_timeout_ms": 10000}},
                {"name": "dedupe", "template": "scriptlet@1.0.1",
                 "override_params": {"script_inline": DEDUPE_SCRIPT}},
                {"name": "headlines", "template": "shelf@1.0.2",
                 "override_params": {"schema": {"headlines": {
                     "id": "text", "at": "text", "title": "text", "link": "text"}}}}
            ],
            "add_edges": [
                {"from": "./tick", "to": "./ask",
                 "condition": "has(hop.schedule_name) && hop.schedule_name == 'tick'"},
                {"from": "./ask", "to": "./feed",
                 "condition": "has(hop.route) && hop.route == 'fetch'"},
                // The RANGE form, and deliberately so: a numeric hop key binds
                // as a CEL uint, so `hop.http_status == 200` compares a uint
                // against an int literal and is never true. Measured on a
                // throwaway colony -- `> 100` and `== 200u` both match, `== 200`
                // does not. `daily-digest` ships this same range.
                {"from": "./feed", "to": "./dedupe",
                 "condition": "has(hop.http_status) && hop.http_status >= 200 && hop.http_status < 300"},
                {"from": "./dedupe", "to": "./headlines",
                 "condition": "has(hop.route) && hop.route == 'store'"},
                {"from": "./headlines", "to": "./sink"}
            ]
        }}),
    )
    .await;
    assert!(
        matches!(outcome, meclaw_colony::MutationOutcome::Committed { .. }),
        "a feed must be nameable by ordinary declarations: {outcome:?}"
    );

    let after = registry_paths(&h).await;
    for node in ["/tick", "/ask", "/feed", "/dedupe", "/headlines"] {
        assert!(
            after.iter().any(|p| p == node),
            "{node} did not grow: {after:?}"
        );
    }

    // 30 s is the repo's failure-marker convention; the clock fires every
    // second, so a chain that never completes still fails in bounded time.
    let mut inserted: Vec<String> = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while inserted.len() < 2 && tokio::time::Instant::now() < deadline {
        let Ok(Some(m)) = tokio::time::timeout_at(deadline, rx.recv()).await else {
            break;
        };
        assert_ne!(
            m.headers.hop.get("error_code").and_then(|v| v.as_str()),
            Some("unknown_table"),
            "the shelf grew a table the manifest did not declare: {:?}",
            m.headers.hop
        );
        if m.headers.hop.get("operation") == Some(&json!("insert")) {
            assert_eq!(
                m.headers.hop.get("rows_affected"),
                Some(&json!(1)),
                "one headline, one row: {:?}",
                m.headers.hop
            );
            inserted.extend(turn_texts(&m));
        }
    }

    assert_eq!(
        inserted.len(),
        2,
        "two distinct links in the feed, three entries: one headline document \
         per NEW item, and the dedupe is the scriptlet's stable id -- a store \
         ships no constraints, so this is the only place it can live"
    );

    h.shutdown().await;
}

/// The briefing's own sentence about the absence, and the four templates it now
/// names instead.
///
/// The head used to say there was no blank single-cell template for a `store`,
/// a `timer` or a `web_fetch` — true when it was written and refutable by the
/// catalogue on the first lookup once these three shipped beside `clock`. A
/// prompt a model can disprove is one it stops believing, which is why the
/// sentence is held to the tree rather than to its author's memory: a drift lock
/// in the sense of `docs/development-rules.md` § 2d, with both halves — it
/// greps the sentence AND resolves every name in it against a shipped template
/// of the cell type the sentence claims for it.
#[test]
fn the_briefing_names_the_blank_templates_the_library_actually_ships() {
    let brief: meclaw_core::serde_json::Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../templates/builder/brief/config.json"),
        )
        .expect("the shipped brief"),
    )
    .expect("parses");
    let head = brief["params"]["script_inline"]
        .as_str()
        .expect("the brief renders its head from a script");

    assert!(
        head.contains("BLANK single-cell template for four ordinary cell types"),
        "the head no longer names the four blank templates, so a composer is \
         back to writing a class for a cell the catalogue already carries"
    );
    assert!(
        !head.contains("none \"\n    \"at all for a `store`"),
        "the head still carries the retracted absence claim"
    );

    for (name, cell_type) in [
        ("clock", "timer"),
        ("fetcher", "web_fetch"),
        ("scriptlet", "code"),
        ("shelf", "store"),
    ] {
        assert!(
            head.contains(&format!("`{name}`")),
            "the head stopped naming `{name}`, which is one of the four types a \
             composer could not name before"
        );
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../templates")
            .join(name);
        assert!(
            root.join("template.json").is_file(),
            "the head names `{name}`, which is not a shipped template -- the \
             catalogue refutes the prompt on the first lookup"
        );
        let config: meclaw_core::serde_json::Value = meclaw_core::serde_json::from_str(
            &std::fs::read_to_string(root.join("config.json")).expect("a shipped config.json"),
        )
        .expect("parses");
        assert_eq!(
            config["cell"]["type"].as_str(),
            Some(cell_type),
            "the head offers `{name}` as a blank {cell_type}, and it is not one"
        );
    }
}

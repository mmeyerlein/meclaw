//! GH #960 — an app that declares `shows` is reached by the presenter, in
//! either order of installation, and nothing of that road dead-letters.
//!
//! # Why this file exists
//!
//! The presenter asks the apps of its member for their screen topics at every
//! `mutation_committed` (`in_show` naming no app) and one app for the data of
//! a topic (`in_show` with `hop.show_app`); the apps answer on `show_topics` /
//! `show_data`. `install_app` draws the member's half of that road from the
//! declaration word `shows: {"at": "./<cell>"}` and from the presenter's own
//! installation (`gh960_shows_is_checked_at_install.rs` locks the rendered
//! edges). This file boots what it renders.
//!
//! # What is measured, at the receiver
//!
//! - **Either order.** An app installed BEFORE the presenter is asked by it at
//!   the presenter's own installation; an app installed AFTER it is asked at
//!   its own. Neither installation names the other: both halves meet at the
//!   container `./apps`.
//! - **Nothing flows without a presenter.** The app installed first hears
//!   nothing until the presenter stands — counted over the whole run: it hears
//!   exactly the two questions the presenter asks after it is installed.
//! - **A presenter alone stays quiet.** Its topics question into an empty
//!   container is answered by the DEFAULT edge as an answer of nobody
//!   (`show_app` empty, no topics) — that answer is the sentinel: it arrives,
//!   and no dead letter of the road was left before it. Once an app stands,
//!   the default is silent (a regular edge matched).
//! - **Fan-out and addressing.** One topics question reaches both apps of a
//!   member; a data question naming one app reaches that app and no other —
//!   read off the colony's own `message_log` after shutdown, never by waiting.
//! - **The stamp is the builder's.** Every app answers with a FORGED
//!   `show_app`; the presenter hears the app's real name and its declared cell.
//! - **No brain on the road.** No `in_show`, `show_topics` or `show_data` is
//!   ever delivered outside `./apps` — not to `./assistants`, `./firewall` or
//!   `./channels` (`message_log`).
//! - **The seal holds.** Every app here is sealed (`ports: []`); the edges end
//!   at its rim, so every installation commits.
//!
//! # What is booted
//!
//! The SHIPPED `member` with `code` doubles in place of its holders and no
//! generation, and three app TEMPLATES grown by mutation: a presenter
//! stand-in (instance `presenter`, `listens: ["mutation_committed"]`) and two
//! apps with `shows`. The colony's mutation receipts go to the root, which
//! hands them to the member — the real `mutation_committed` of each
//! installation. Every cell reports what it heard on `error`, the lane a member
//! carries out of the level, through a TEST witness edge the install mutation
//! carries beside what the recipe rendered.
//!
//! Guarded like every other template-reading test (GH #49): the public export
//! ships a subset of the library, and a tree without the member or the
//! builder's recipes is skipped rather than judged.

use meclaw_cells::code::CodeCellFactory;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Message, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// A path inside this repository, from the crate's manifest directory.
fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped() -> Option<std::path::PathBuf> {
    let member = repo("templates/member");
    (member.join("config.json").is_file()
        && repo("templates/builder/recipes/config.json").is_file())
    .then_some(member)
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().expect("a parent directory")).expect("create the directory");
    std::fs::write(
        p,
        meclaw_core::serde_json::to_string_pretty(v).expect("serialise"),
    )
    .expect("write");
}

/// Copy the template cell by cell: only `config.json` files travel, so the tree
/// under test IS the template and nothing else.
fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).expect("create the directory");
    for entry in std::fs::read_dir(src).expect("the template directory is readable") {
        let entry = entry.expect("directory entry");
        let from = entry.path();
        if from.is_dir() {
            copy_cells(&from, &dst.join(entry.file_name()));
        } else if entry.file_name() == "config.json" {
            std::fs::copy(&from, dst.join("config.json")).expect("copy the config");
        }
    }
}

// ═══════════════════════════════════════════════════════════════ the names

/// Where the member stands.
const MEMBER: &str = "/person";
/// The presenter, known to the recipe by this instance name.
const PRESENTER: &str = "presenter";
/// The app the presenter stand-in asks for data.
const ASKED: &str = "probe-shows";
/// A second app with `shows`, never asked for data.
const OTHER: &str = "probe-shows-two";
/// The cell both apps declare.
const AT: &str = "./show";
/// The three lanes of the road.
const ROAD: [&str; 3] = ["in_show", "show_topics", "show_data"];
/// `install_app` takes a screen whatever the declaration says; no app here
/// declares one, so this literal lands in no edge.
const UNUSED_SCREEN: &str = "display-main";

// ══════════════════════════════════════════════════════════════ the doubles

/// A cell that answers nothing: the holders no round here reaches.
const INERT: &str = r#"
import sys, json
json.load(sys.stdin)
sys.stdout.write(json.dumps([]))
"#;

/// The presenter stand-in. At every `mutation_committed` it asks for topics
/// once (`in_show`, no app named). Every answer is reported on `error` with the
/// app and cell the BUILDER stamped; a topics answer of `__ASKED__` is followed
/// by one data question naming that app.
const STAGE: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = (doc["envelope"].get("header") or {}).get("hop") or {}
body = doc.get("body") or {}
route = str(hop.get("route") or "")
out = []
if route == "mutation_committed":
    out.append({"header": {"route": "in_show"}, "op": "topics", "messages": []})
elif route in ("show_topics", "show_data"):
    app = str(hop.get("show_app") or "")
    topics = [t.get("topic", "") for t in (body.get("topics") or []) if isinstance(t, dict)]
    out.append({"header": {"route": "error", "error_code": "presenter_heard",
                           "heard_route": route, "heard_app": app,
                           "heard_at": str(hop.get("show_at") or ""),
                           "heard_topics": ",".join(sorted(topics)),
                           "heard_topic": str(body.get("topic") or "")},
                "messages": []})
    if route == "show_topics" and app == "__ASKED__":
        out.append({"header": {"route": "in_show", "show_app": app},
                    "op": "data", "topic": "probe-" + app, "turn_id": "t-960",
                    "messages": []})
sys.stdout.write(json.dumps(out))
"#;

/// The show cell of an app. Every question is reported on `error`; it is
/// answered with a FORGED `show_app` (and `show_at`), which the builder's
/// stamp on the answer edge has to overwrite.
const SHOW: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = (doc["envelope"].get("header") or {}).get("hop") or {}
body = doc.get("body") or {}
route = str(hop.get("route") or "")
out = []
if route == "in_show":
    op = str(body.get("op") or "")
    out.append({"header": {"route": "error", "error_code": "app_heard",
                           "heard_by": "__WHO__", "heard_op": op,
                           "heard_app": str(hop.get("show_app") or "")},
                "messages": []})
    if op == "topics":
        out.append({"header": {"route": "show_topics", "show_app": "forged",
                               "show_at": "./forged"},
                    "topics": [{"topic": "probe-__WHO__", "title": "Probe",
                                "describe": "A probe topic."}],
                    "messages": []})
    elif op == "data":
        out.append({"header": {"route": "show_data", "show_app": "forged"},
                    "topic": str(body.get("topic") or ""),
                    "turn_id": str(body.get("turn_id") or ""),
                    "sets": {}, "messages": []})
sys.stdout.write(json.dumps(out))
"#;

/// A `code` double with a fixed script. `multi_send_capable` is on so a double
/// may answer with NOTHING or with two messages.
fn double(script: &str, purpose: &str) -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {
            "runner": "python3",
            "script_inline": script,
            "external_timeout_ms": 10000
        },
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {"body": {"messages": {"type": "array", "required": false}}},
            "consumes": {"body": {"messages": {"type": "array", "required": false}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": purpose,
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

fn cond(routes: &[&str]) -> String {
    format!(
        "has(hop.route) && ({})",
        routes
            .iter()
            .map(|r| format!("hop.route == '{r}'"))
            .collect::<Vec<_>>()
            .join(" || ")
    )
}

// ═════════════════════════════════════════════════════════════ the app templates

/// One SEALED app template under `templates/<name>`: a hive with `ports: []`,
/// its contract, its inner graph and one code cell.
fn write_app_template(
    root: &std::path::Path,
    name: &str,
    contract: Value,
    inner: Vec<Value>,
    cell: (&str, &str),
) {
    write(
        root,
        &format!("templates/{name}/config.json"),
        &json!({
            "cell": {"type": "hive"},
            "params": {
                "ports": [],
                "contract": contract,
                "graph": {"edges": inner}
            },
            "description": {
                "purpose": "Fixture app for GH #960.",
                "use_when": "Test fixture only.",
                "not_in_scope": "Not a shipped template."
            }
        }),
    );
    write(
        root,
        &format!("templates/{name}/{}/config.json", cell.0),
        &double(cell.1, "Fixture app cell for GH #960."),
    );
    write(
        root,
        &format!("templates/{name}/template.json"),
        &json!({"name": name, "version": "1.0.0", "tags": ["app"], "author": "meclaw"}),
    );
}

fn write_templates(root: &std::path::Path) {
    write_app_template(
        root,
        PRESENTER,
        json!({
            "accepts": [
                {"route": "mutation_committed", "because": "the graph moved: ask the apps for their topics again"},
                {"route": "show_topics", "because": "an app's screen topics, stamped with the app by the member's edge"},
                {"route": "show_data", "because": "the data of one topic, stamped with the app by the member's edge"}
            ],
            "emits": [
                {"route": "in_show", "because": "the topics question to every app with shows, or the data question to one"},
                {"route": "error", "because": "the stand-in's report of what it heard"}
            ]
        }),
        vec![
            json!({"from": ".", "to": "./stage",
                   "condition": cond(&["mutation_committed", "show_topics", "show_data"])}),
            json!({"from": "./stage", "to": ".", "condition": cond(&["in_show", "error"])}),
        ],
        ("stage", &STAGE.replace("__ASKED__", ASKED)),
    );
    for who in [ASKED, OTHER] {
        write_app_template(
            root,
            who,
            json!({
                "accepts": [
                    {"route": "in_show", "because": "the presenter's question: topics, or the data of one topic"}
                ],
                "emits": [
                    {"route": "show_topics", "because": "this app's screen topics"},
                    {"route": "show_data", "because": "the data of one topic"},
                    {"route": "error", "because": "the probe's report of what it heard"}
                ]
            }),
            vec![
                json!({"from": ".", "to": AT, "condition": cond(&["in_show"])}),
                json!({"from": AT, "to": ".",
                       "condition": cond(&["show_topics", "show_data", "error"])}),
            ],
            ("show", &SHOW.replace("__WHO__", who)),
        );
    }
}

// ══════════════════════════════════════ the wiring an installing mutation draws

fn declaration(app: &str) -> Value {
    if app == PRESENTER {
        json!({"listens": ["mutation_committed"]})
    } else {
        json!({"shows": {"at": AT}})
    }
}

/// **The install diff, as the builder renders it** — `install_app` over the
/// declaration, read out of `manifest[0].diff` unchanged.
fn rendered_diff(app: &str) -> Value {
    let out = meclaw_testing::emit_all(
        &meclaw_testing::shipped_script(
            repo("templates/builder/recipes/config.json")
                .to_str()
                .expect("a utf-8 path"),
        ),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!({"recipe": "install_app", "request": "…",
                                         "params": {"scope": MEMBER, "app": app,
                                                    "template": format!("{app}@1.0.0"),
                                                    "screen": UNUSED_SCREEN,
                                                    "declaration": declaration(app)}})
                                      .to_string()}],
        }),
    );
    let first = out.first().expect("the recipe emitted nothing");
    assert!(
        first["header"]["error_code"].is_null(),
        "the recipe refused the declaration of {app}: {first}"
    );
    first["manifest"][0]["diff"].clone()
}

/// The colony around the member: the receipts go to the root, which hands
/// them to the member, and a drain for every lane the member emits at its rim.
fn main_config() -> Value {
    let mut edges = vec![json!({
        "from": ".", "to": "./person",
        "condition": "has(hop.route) && hop.route == 'mutation_committed'"
    })];
    for lane in [
        "answer",
        "bundle",
        "ack",
        "reject",
        "error",
        "write",
        "turn_write",
        "build",
        "close_report",
        "export_done",
        "dump",
        "pack_ack",
    ] {
        edges.push(json!({"from": "./person", "to": "/sink",
                          "condition": format!("has(hop.route) && hop.route == '{lane}'")}));
    }
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}})
}

fn build_tree(td: &tempfile::TempDir, member: &std::path::Path) {
    let root = td.path();
    write(
        root,
        "colony.json",
        &json!({"schema_version": 1, "mutation_receipts": {"to": "/"}}),
    );
    write(root, "main/config.json", &main_config());
    copy_cells(member, &root.join("main/person"));
    for holder in [
        "access",
        "affinity",
        "memory-hive",
        "file-space",
        "graph-space",
        "librarian",
        "objects",
        "firewall",
    ] {
        write(
            root,
            &format!("main/person/{holder}/config.json"),
            &double(INERT, "Inert double for a holder no round here reaches."),
        );
    }
    write_templates(root);
    std::fs::write(root.join(".env"), "").expect("write an empty .env");
}

// ═════════════════════════════════════════════════════════════════ the colony

struct Colony {
    td: tempfile::TempDir,
    h: ColonyHandle,
    rx: mpsc::Receiver<Message>,
    seen: Vec<Message>,
}

/// Boot the member with NO app installed; the apps come by mutation.
async fn start() -> Colony {
    let member = shipped().expect("guarded by the caller");
    let td = tempfile::tempdir().expect("a temporary directory");
    build_tree(&td, &member);

    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![(
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        )]
    };
    let h = ColonyHandle::new_with_factories_at(&td, factories());
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(256);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: td.path().join("templates"),
            ack: ack_tx,
        })
        .await
        .expect("the colony is up");
    ack_rx
        .await
        .expect("the scan answers")
        .expect("the template scan succeeds");
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the shipped member must boot");
    Colony {
        td,
        h,
        rx: sink_rx,
        seen: Vec::new(),
    }
}

/// Install one app: the rendered diff plus the witness edge that carries its
/// reports onto the container, where the member's `./apps -> .` takes `error`
/// out of the level. It must COMMIT — no seal, v-lane or contract refusal.
async fn install(c: &Colony, app: &str) {
    let mut diff = rendered_diff(app);
    diff["add_edges"]
        .as_array_mut()
        .expect("the recipe renders edges")
        .push(json!({
            "from": format!("./apps/{app}"), "to": "./apps",
            "condition": "has(hop.route) && hop.route == 'error'"
        }));
    let (ack_tx, ack_rx) = oneshot::channel();
    c.h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload: json!({"scope": MEMBER, "diff": diff}),
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("the colony is up");
    let outcome = ack_rx.await.expect("the mutation answers");
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "installing {app} is one ordinary mutation: {outcome:?}"
    );
}

fn hop_of(m: &Message, key: &str) -> String {
    m.headers
        .hop
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// One answer the presenter heard: `(route, app, at, topics, topic)`.
type Heard = (String, String, String, String, String);

fn presenter_heard(seen: &[Message]) -> Vec<Heard> {
    seen.iter()
        .filter(|m| hop_of(m, "error_code") == "presenter_heard")
        .map(|m| {
            (
                hop_of(m, "heard_route"),
                hop_of(m, "heard_app"),
                hop_of(m, "heard_at"),
                hop_of(m, "heard_topics"),
                hop_of(m, "heard_topic"),
            )
        })
        .collect()
}

/// The questions the app `who` heard: `(op, show_app)`.
fn app_heard(seen: &[Message], who: &str) -> Vec<(String, String)> {
    seen.iter()
        .filter(|m| hop_of(m, "error_code") == "app_heard" && hop_of(m, "heard_by") == who)
        .map(|m| (hop_of(m, "heard_op"), hop_of(m, "heard_app")))
        .collect()
}

/// Wait for an EVENT: collect what reaches the sink until `done` holds over
/// everything seen so far. The deadline is the failure marker, not a window.
async fn until(c: &mut Colony, what: &str, done: impl Fn(&[Message]) -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !done(&c.seen) {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(left, c.rx.recv()).await {
            Ok(Some(m)) => c.seen.push(m),
            _ => panic!(
                "{what} did not happen within 30 s; the sink saw: {:#?}\ndead letters: {:#?}",
                c.seen
                    .iter()
                    .map(|m| m.headers.hop.clone())
                    .collect::<Vec<_>>(),
                dead_letters(c.td.path())
            ),
        }
    }
}

/// `(error_code, resolved_target, hop.route)` of every dead letter.
fn dead_letters(root: &std::path::Path) -> Vec<(String, String, String)> {
    let Ok(conn) = rusqlite::Connection::open(root.join("colony.db")) else {
        return Vec::new();
    };
    let Ok(mut st) = conn
        .prepare("SELECT error_code, resolved_target, message_json FROM dead_letters ORDER BY id")
    else {
        return Vec::new();
    };
    st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })
    .map(|rows| {
        rows.filter_map(Result::ok)
            .map(|(code, target, msg)| {
                let m: Value = meclaw_core::serde_json::from_str(&msg).unwrap_or(Value::Null);
                let route = m["headers"]["hop"]["route"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                (code, target, route)
            })
            .collect()
    })
    .unwrap_or_default()
}

/// Every delivery of a lane of the road, off the colony's own `message_log`:
/// `(to_path, hop)`. Read after shutdown, which flushes the log writer.
fn road_deliveries(root: &std::path::Path) -> Vec<(String, Value)> {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare("SELECT to_path, headers FROM message_log ORDER BY rowid")
        .expect("message_log");
    st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .expect("query")
        .filter_map(Result::ok)
        .filter_map(|(to, headers)| {
            let h: Value = meclaw_core::serde_json::from_str(&headers).unwrap_or(Value::Null);
            let route = h["hop"]["route"].as_str().unwrap_or_default().to_string();
            if ROAD.contains(&route.as_str()) {
                Some((to, h["hop"].clone()))
            } else {
                None
            }
        })
        .collect()
}

/// Shut down, then judge what the run left behind: no dead letter of the road,
/// and no lane of it delivered outside the member's `./apps`.
async fn finish(c: Colony) -> Vec<(String, Value)> {
    let Colony { td, h, .. } = c;
    h.shutdown().await;
    let dead: Vec<_> = dead_letters(td.path())
        .into_iter()
        .filter(|(_, _, route)| ROAD.contains(&route.as_str()))
        .collect();
    assert!(dead.is_empty(), "the road left dead letters: {dead:#?}");
    let road = road_deliveries(td.path());
    assert!(
        !road.is_empty(),
        "the message_log carries the road (a silent log would prove nothing)"
    );
    let apps = format!("{MEMBER}/apps");
    for (to, hop) in &road {
        assert!(
            to == &apps || to.starts_with(&format!("{apps}/")),
            "a lane of the road was delivered outside ./apps — no brain, firewall or \
             channel is on it: {to} {hop}"
        );
    }
    road
}

fn skip() -> bool {
    if shipped().is_none() {
        eprintln!("member/builder did not travel into this tree -- skipped (GH #49)");
        return true;
    }
    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("no python3 -- skipped");
        return true;
    }
    false
}

/// The answer the asked app's data question came back with — the event every
/// round below ends on.
fn data_came_back(seen: &[Message]) -> bool {
    presenter_heard(seen)
        .iter()
        .any(|(r, app, _, _, _)| r == "show_data" && app == ASKED)
}

// ═══════════════════════════════════════════════════════════ the measurements

/// (a) + (c): the app stands first, the presenter comes after it. Until the
/// presenter stands nothing flows: the app hears, over the whole run, exactly
/// the presenter's topics question and its data question — no more.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_installed_before_the_presenter_is_asked_by_it() {
    if skip() {
        return;
    }
    let mut c = start().await;
    install(&c, ASKED).await;
    install(&c, PRESENTER).await;
    until(&mut c, "the asked app's data answer", data_came_back).await;

    let heard = presenter_heard(&c.seen);
    let topics: Vec<&Heard> = heard.iter().filter(|h| h.0 == "show_topics").collect();
    assert_eq!(
        topics.len(),
        1,
        "one topics answer, from the one app: {heard:#?}"
    );
    assert_eq!(
        (
            topics[0].1.as_str(),
            topics[0].2.as_str(),
            topics[0].3.as_str()
        ),
        (ASKED, AT, "probe-probe-shows"),
        "the presenter hears the app's topic, stamped with the app and the cell the \
         builder knows — not the forged stamp the app wrote"
    );
    let data: Vec<&Heard> = heard.iter().filter(|h| h.0 == "show_data").collect();
    assert_eq!(data.len(), 1, "{heard:#?}");
    assert_eq!(data[0].1, ASKED);
    assert_eq!(
        data[0].4, "probe-probe-shows",
        "the data answer names its topic"
    );

    assert_eq!(
        app_heard(&c.seen, ASKED),
        vec![
            ("topics".to_string(), String::new()),
            ("data".to_string(), ASKED.to_string())
        ],
        "the app heard nothing before the presenter stood, then its two questions"
    );
    finish(c).await;
}

/// (b) + (e): the presenter stands first and alone. Its topics question into
/// the empty container comes back as an answer of nobody and dead-letters
/// nothing; the app installed after it is asked at its own installation, and
/// then the empty answer is silent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_presenter_installed_first_finds_an_app_installed_after_it() {
    if skip() {
        return;
    }
    let mut c = start().await;
    install(&c, PRESENTER).await;
    // The sentinel of the presenter alone: its question is ANSWERED, by the
    // default edge, as nobody's.
    until(&mut c, "the empty answer to a presenter alone", |seen| {
        presenter_heard(seen)
            .iter()
            .any(|(r, app, _, _, _)| r == "show_topics" && app.is_empty())
    })
    .await;
    let alone = dead_letters(c.td.path());
    assert!(
        !alone
            .iter()
            .any(|(_, _, route)| ROAD.contains(&route.as_str())),
        "a presenter alone leaves no dead letter of the road: {alone:#?}"
    );

    install(&c, ASKED).await;
    until(&mut c, "the asked app's data answer", data_came_back).await;

    let heard = presenter_heard(&c.seen);
    let empty: Vec<&Heard> = heard
        .iter()
        .filter(|h| h.0 == "show_topics" && h.1.is_empty())
        .collect();
    assert_eq!(
        empty.len(),
        1,
        "the empty answer came once, while no app stood; once one stands the default \
         is silent: {heard:#?}"
    );
    assert_eq!(
        (empty[0].2.as_str(), empty[0].3.as_str()),
        ("", ""),
        "an answer of nobody names no cell and carries no topic"
    );
    let topics: Vec<&Heard> = heard
        .iter()
        .filter(|h| h.0 == "show_topics" && h.1 == ASKED)
        .collect();
    assert_eq!(topics.len(), 1, "{heard:#?}");
    assert_eq!(
        (topics[0].2.as_str(), topics[0].3.as_str()),
        (AT, "probe-probe-shows")
    );
    assert_eq!(
        app_heard(&c.seen, ASKED),
        vec![
            ("topics".to_string(), String::new()),
            ("data".to_string(), ASKED.to_string())
        ],
        "{:#?}",
        c.seen
    );
    finish(c).await;
}

/// (d): two apps with `shows`. One topics question reaches both; the data
/// question naming one reaches that one and no other.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_topics_question_reaches_both_apps_and_a_data_question_only_its_own() {
    if skip() {
        return;
    }
    let mut c = start().await;
    install(&c, ASKED).await;
    install(&c, OTHER).await;
    install(&c, PRESENTER).await;
    until(&mut c, "both topics answers and the data answer", |seen| {
        let heard = presenter_heard(seen);
        data_came_back(seen)
            && heard
                .iter()
                .any(|(r, app, _, _, _)| r == "show_topics" && app == OTHER)
    })
    .await;

    let mut topics: Vec<(String, String, String)> = presenter_heard(&c.seen)
        .into_iter()
        .filter(|h| h.0 == "show_topics")
        .map(|h| (h.1, h.2, h.3))
        .collect();
    topics.sort();
    assert_eq!(
        topics,
        vec![
            (
                ASKED.to_string(),
                AT.to_string(),
                "probe-probe-shows".to_string()
            ),
            (
                OTHER.to_string(),
                AT.to_string(),
                "probe-probe-shows-two".to_string()
            ),
        ],
        "one question, two answers, each stamped with its own app"
    );
    assert!(
        !presenter_heard(&c.seen)
            .iter()
            .any(|(r, app, _, _, _)| r == "show_topics" && app.is_empty()),
        "with apps behind the container the empty answer never comes"
    );

    let road = finish(c).await;
    let other = format!("{MEMBER}/apps/{OTHER}");
    let asked = format!("{MEMBER}/apps/{ASKED}");
    let questions_to = |prefix: &str| -> Vec<String> {
        road.iter()
            .filter(|(to, hop)| {
                (to == prefix || to.starts_with(&format!("{prefix}/"))) && hop["route"] == "in_show"
            })
            .map(|(_, hop)| hop["show_app"].as_str().unwrap_or_default().to_string())
            .collect()
    };
    assert!(
        !questions_to(&other).iter().any(|a| a == ASKED),
        "the data question naming {ASKED} never reached {OTHER}: {:#?}",
        questions_to(&other)
    );
    assert!(
        questions_to(&other).iter().any(String::is_empty),
        "{OTHER} was reached by the topics question"
    );
    assert!(
        questions_to(&asked).iter().any(|a| a == ASKED),
        "the data question reached {ASKED}"
    );
}

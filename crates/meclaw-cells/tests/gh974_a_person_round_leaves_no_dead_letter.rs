//! GH #974 — growing a colony level by level and a person's round in it leave
//! only the dead letters this file's class table declares; the way's own
//! messages never dead-letter; a real misroute still does.
//!
//! **The ruling behind it.** meclaw is event-driven: a message that reaches the
//! top of a graph nobody listens at is dead-lettered, and that is the audit, not
//! a defect. The coding proof run and the orga lab counted such dead letters
//! after every stage and every round (21 in one round). They are not removed
//! here; they are CLASSIFIED. `fixtures/gh974_dead_letter_classes.json` lists
//! every class observed in the lab's own form -- lane, reason, emitter, target
//! -- with why it is no defect. The table is data: a new class is a new row with
//! a reason, or it is a bug at its emitter.
//!
//! **What is measured here, at the receiving end.** The same stages in the same
//! form, against the shipped library: an empty root, the shell `meclaw-os` added
//! at `/`, then an org, a member and an assistant, each as a `grow_level` WISH
//! sent to `/os/builder` and the drafted manifest applied at the mutation door;
//! then a channel grown into the member, a person's turns on it and the close
//! of the session. Every dead letter is drained and must match a class. Waits
//! are for events: the draft's `in_build_result`, each receipt, the answer on
//! the channel, the tool's result at the asker, the archive echo of every
//! participant turn, the close's batch and report. Only after the LAST step's
//! last expected event is the sentinel sent (a turn to a member that does not
//! exist) and its dead letter awaited, so the absence of everything else is
//! read after something that HAD to arrive did, and nothing the run still owed
//! can arrive behind it.
//!
//! **The way's own messages** -- the turn, the answer, a tool's call and its
//! result -- never dead-letter, and no class may declare one: a person's words
//! come in on a channel the member grew (GH #468, the lab grows it as a level),
//! and the answer goes back on it. The way test makes the brain call a real
//! tool (`memory_recall`, answered by the member's memory) and reads the result
//! arriving at the asker before it reads the absence of a dead letter.
//!
//! **Substitution:** every `llm` cell of the library copy answers from a local
//! stub (`support/gh929_member_road.rs`), every cron is pushed out of the run.
//!
//! Guarded like every template-reading test (GH #49).

#[path = "mock_openai.rs"]
mod mock_openai;
#[path = "support/gh929_member_road.rs"]
mod road;

use meclaw_cells::WebCellFactory;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationDoorOutcome, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::{ColonyHandle, NEVER_CRON};
use mock_openai::{MockOpenAI, canned_chat_completion, canned_tool_calls};
use road::{Stubs, as_map, configs_under, read_json, write_json};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;

/// Failure marker (the 30 s convention would cut a cold builder render short on
/// a loaded host; nothing below waits for it unless an event never comes).
const DEADLINE: Duration = Duration::from_secs(90);
const ORG: &str = "lab";
const MEMBER: &str = "member";
const AGENT: &str = "agent";
const MEMBER_PATH: &str = "/os/orgs/lab/members/member";
const GENERATION_PATH: &str = "/os/orgs/lab/members/member/assistants/agent";
/// The channel the way test grows (the `channel` level): a probe connector
/// that takes what reaches it and says nothing. A library connector would dial
/// its provider; the screen is no way out either (OR-NL.W.6 in the report:
/// growing a `display` into a running member stops at `resume_requires_stopped_cell`).
const GROWN_CHANNEL: &str = "probe";
const PROBE_CHANNEL: &str = "gh974-probe-channel";
/// The round of a person's turn, as the lab sends it.
const AUDIENCE: &str = r#"["human:member","agent:agent"]"#;
/// The lanes of the way itself: the turn, the answer, a tool's call and its
/// result. Measured on a `memory_recall` round (fix round 1): the call leaves
/// the dispatcher on `tool`, enters the member's memory on `tool_call`, its
/// answer leaves it on `tool_result` and re-enters the asking surface on
/// `in_tool`.
const WAY: &[&str] = &[
    "in_turn",
    "answer",
    "tool",
    "tool_call",
    "tool_result",
    "in_tool",
];
/// The brain's tool call: the member's own memory answers it (GH #552), so
/// the call leaves the assistant and its result comes back the way a real
/// tool's does. A refusal would answer as a `tool_result` too.
const TOOL_CALL_ID: &str = "gh974-recall";
const TOOL: &str = "memory_recall";
const TOOL_ARGS: &str = r#"{"query":"what is on today"}"#;

fn repo(rel: &str) -> std::path::PathBuf {
    road::repo(rel)
}

fn shipped() -> bool {
    [
        "templates/meclaw-os/config.json",
        "templates/builder/recipes/config.json",
        "templates/assistant/config.json",
    ]
    .iter()
    .all(|rel| repo(rel).is_file())
}

/// The library's cells plus the display channel's `web` cell, which the
/// assistant stage grows with the member (a fixture table of its own; the
/// display binds nothing since 2.0.0).
fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    let mut f = road::factories();
    f.push((
        "web".to_string(),
        Arc::new(WebCellFactory::default()) as Arc<dyn CellFactory>,
    ));
    f
}

/// A channel connector for the way test: one `code` cell that keeps nothing
/// and answers nothing -- the arrival in `message_log` is the measurement.
fn probe_channel(dir: &std::path::Path) {
    std::fs::create_dir_all(dir).expect("mkdir probe channel");
    write_json(
        &dir.join("template.json"),
        &json!({"name": PROBE_CHANNEL, "version": "1.0.0", "tags": ["channel"],
                "description": {
                    "purpose": "Test fixture for GH #974: a channel that takes what reaches it.",
                    "use_when": "Test fixture only.",
                    "not_in_scope": "Not a template: it reaches nobody."},
                "author": "meclaw"}),
    );
    write_json(
        &dir.join("config.json"),
        &json!({"cell": {"type": "code"},
                "params": {"runner": "python3", "external_timeout_ms": 15000,
                           "script_inline": "import sys\nsys.stdin.read()\nsys.stdout.write('[]')\n"},
                "contract": {"version": "1.0.0", "settings": {}, "multi_send_capable": true,
                             "emits": {"body": {"messages": {"type": "array", "required": false}}},
                             "consumes": {"body": {"messages": {"type": "array", "required": false}}},
                             "capabilities": ["shell:exec"]},
                "description": {
                    "purpose": "Test fixture for GH #974: takes a channel's traffic and says nothing.",
                    "use_when": "Test fixture only.",
                    "not_in_scope": "Not a template."}}),
    );
}

/// One row of `fixtures/gh974_dead_letter_classes.json`.
#[derive(Debug)]
struct Class {
    route: String,
    reason: String,
    sender: String,
    target: String,
}

fn classes() -> Vec<Class> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/gh974_dead_letter_classes.json");
    let v = read_json(&p);
    v["classes"]
        .as_array()
        .expect("a classes array")
        .iter()
        .map(|c| {
            let s = |k: &str| {
                c[k].as_str()
                    .unwrap_or_else(|| panic!("class without {k}: {c}"))
                    .to_string()
            };
            assert!(
                !c["why"].as_str().unwrap_or_default().is_empty(),
                "a class without a reason is a hidden defect: {c}"
            );
            Class {
                route: s("route"),
                reason: s("reason"),
                sender: s("sender"),
                target: s("target"),
            }
        })
        .collect()
}

/// The dead letters no class declares.
fn undeclared(dead: &[Dead]) -> Vec<String> {
    let table = classes();
    dead.iter()
        .filter(|d| {
            !table.iter().any(|c| {
                c.route == d.route
                    && c.reason == d.code
                    && c.sender == d.sender
                    && c.target == d.target
            })
        })
        .map(Dead::say)
        .collect()
}

/// `<name>@<version>` of the library template, read off its `template.json`.
fn pin(name: &str) -> String {
    let v = read_json(&repo(&format!("templates/{name}/template.json")));
    format!("{name}@{}", v["version"].as_str().expect("a version"))
}

fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).expect("mkdir");
    for entry in std::fs::read_dir(src).expect("readable") {
        let from = entry.expect("entry").path();
        let to = dst.join(from.file_name().expect("a name"));
        if from.is_dir() {
            copy_tree(&from, &to);
        } else {
            std::fs::copy(&from, &to).expect("copy");
        }
    }
}

/// Every cron of the library copy out of the run's way. Schedule ids stay as
/// they are: the library is instantiated more than once, and a fixed id would be
/// shared by every instance.
fn quiet_crons(templates: &std::path::Path) {
    let mut files = Vec::new();
    configs_under(templates, &mut files);
    for f in files {
        let mut cfg = read_json(&f);
        if cfg["cell"]["type"] != "timer" {
            continue;
        }
        let Some(schedules) = cfg["params"]["schedules"].as_array_mut() else {
            continue;
        };
        for s in schedules.iter_mut() {
            if s.get("cron").is_some() {
                s["cron"] = json!(NEVER_CRON);
            }
        }
        write_json(&f, &cfg);
    }
}

// ═══════════════════════════════════════════════════════════════ the colony

/// One dead letter as the report needs it: who, where, why, on which lane.
#[derive(Clone, Debug)]
struct Dead {
    sender: String,
    original: String,
    target: String,
    code: String,
    route: String,
    error: String,
    body: String,
}

impl Dead {
    fn say(&self) -> String {
        format!(
            "{} -> {} [{}] route={} error_code={} body={}",
            self.sender, self.target, self.code, self.route, self.error, self.body
        )
    }
}

struct Lab {
    td: tempfile::TempDir,
    h: ColonyHandle,
    /// Held: a dropped stub stops answering.
    _stub: MockOpenAI,
    _closer: MockOpenAI,
    _brain: MockOpenAI,
}

impl Lab {
    async fn boot() -> Self {
        let stub = MockOpenAI::start(vec![canned_chat_completion(road::REPLY, "stop")]).await;
        // The closer answers the shape it is asked for (an empty verdict): the
        // background stub's prose would fail the verdict (`closer_failed`) and
        // turn every close pass into a refusal the lab's model does not make.
        let closer = MockOpenAI::start(vec![canned_chat_completion(
            r#"{"nothing_to_add": true, "add": [], "sharpen": [], "correct": [], "close_topics": []}"#,
            "stop",
        )])
        .await;
        // The surfaces' brain: the first turn asks a tool, every answer after
        // the result is the reply.
        let brain = MockOpenAI::start(vec![
            canned_tool_calls(vec![(TOOL_CALL_ID, TOOL, TOOL_ARGS)]),
            canned_chat_completion(road::REPLY, "stop"),
        ])
        .await;
        let td = tempfile::TempDir::new().expect("tempdir");
        let root = td.path();
        let templates = root.join("templates");
        copy_tree(&repo("templates"), &templates);
        quiet_crons(&templates);
        probe_channel(&templates.join(PROBE_CHANNEL));
        road::point_llms_at_stubs(
            &templates,
            &Stubs {
                scripted: HashMap::from([
                    ("memory-hive/closer".to_string(), closer.base_url.clone()),
                    ("talky/brain".to_string(), brain.base_url.clone()),
                ]),
                background: stub.base_url.clone(),
            },
        );
        road::write_env(root, &templates, &stub.base_url);
        // The lab's colony: receipts on, an empty root, the shell grown as a stage.
        write_json(
            &root.join("colony.json"),
            &json!({"schema_version": 1, "mutation_receipts": {"to": "/os"}}),
        );
        write_json(
            &root.join("main/config.json"),
            &json!({"cell": {"type": "hive"}}),
        );

        let h = ColonyHandle::new_with_factories_at(&td, factories());
        let (ack_tx, ack_rx) = oneshot::channel();
        h.inbox_tx
            .send(ColonyMsg::RescanTemplates {
                templates_root: templates.clone(),
                ack: ack_tx,
            })
            .await
            .expect("rescan");
        ack_rx
            .await
            .expect("rescan ack")
            .expect("GH #440: the rescan must not have aborted");
        let mut registry = CellFactoryRegistry::new();
        for (name, f) in factories() {
            registry.insert(name, f);
        }
        bootstrap_from_filesystem(root, &registry, &h.runtime())
            .await
            .expect("the empty root must boot");
        Lab {
            td,
            h,
            _stub: stub,
            _closer: closer,
            _brain: brain,
        }
    }

    fn root(&self) -> &std::path::Path {
        self.td.path()
    }

    async fn drain(&self) -> Vec<Dead> {
        self.h
            .drain_dead_letters()
            .await
            .iter()
            .map(|d| {
                let hop = &d.message.headers.hop;
                let mut error = hop
                    .get("error_code")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if error.is_empty()
                    && let Body::Inline(b) = &d.message.body
                {
                    error = b["error"]["code"]
                        .as_str()
                        .or_else(|| b["code"].as_str())
                        .unwrap_or_default()
                        .to_string();
                }
                Dead {
                    sender: d.sender_path.as_str().to_string(),
                    original: d.original_target.as_str().to_string(),
                    target: d.resolved_target.as_str().to_string(),
                    code: d.reason.as_code().to_string(),
                    route: hop
                        .get("route")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    error,
                    body: match &d.message.body {
                        Body::Inline(b) => b.to_string().chars().take(240).collect(),
                        _ => String::new(),
                    },
                }
            })
            .collect()
    }

    async fn apply(&self, payload: Value) -> MutationDoorOutcome {
        let (ack_tx, ack_rx) = oneshot::channel();
        self.h
            .inbox_tx
            .send(ColonyMsg::MutationDoor {
                payload,
                reply_to: None,
                trace_id: Uuid::now_v7(),
                parent_message_id: Uuid::now_v7(),
                ack: ack_tx,
            })
            .await
            .expect("send manifest");
        ack_rx.await.expect("manifest ack")
    }

    /// The highest rowid of `message_log`, the mark a wait reads after.
    fn mark(&self) -> i64 {
        let conn = rusqlite::Connection::open(self.root().join("colony.db")).expect("colony.db");
        conn.query_row("SELECT COALESCE(MAX(rowid), 0) FROM message_log", [], |r| {
            r.get(0)
        })
        .unwrap_or(0)
    }

    /// `(to_path, route, body_payload)` of every delivery after `mark`.
    fn since(&self, mark: i64) -> Vec<(String, String, String)> {
        let conn = rusqlite::Connection::open(self.root().join("colony.db")).expect("colony.db");
        let Ok(mut st) = conn.prepare(
            "SELECT to_path, headers, COALESCE(body_payload, '') FROM message_log \
             WHERE rowid > ?1 ORDER BY rowid",
        ) else {
            return Vec::new();
        };
        st.query_map([mark], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map(|rows| {
            rows.filter_map(Result::ok)
                .map(|(to, headers, body)| {
                    let h: Value =
                        meclaw_core::serde_json::from_str(&headers).unwrap_or(Value::Null);
                    let route = h["hop"]["route"].as_str().unwrap_or_default().to_string();
                    (to, route, body)
                })
                .collect()
        })
        .unwrap_or_default()
    }

    /// Deliveries after `mark` that bring the brain's own tool call's result
    /// back into the asking generation: lane `in_tool` (the member's memory
    /// answers on `tool_result`, the member hands it down on `in_tool`) with
    /// `TOOL_CALL_ID` on the hop. Other results reach a generation too (a
    /// start-up schema answer); only this one is the call's.
    fn results_of_the_call(&self, mark: i64) -> usize {
        let conn = rusqlite::Connection::open(self.root().join("colony.db")).expect("colony.db");
        conn.query_row(
            "SELECT COUNT(*) FROM message_log WHERE rowid > ?1 AND to_path LIKE ?2 \
             AND (headers LIKE ?3 OR COALESCE(body_payload, '') LIKE ?3) \
             AND headers LIKE '%\"in_tool\"%'",
            rusqlite::params![
                mark,
                format!("{GENERATION_PATH}%"),
                format!("%{TOOL_CALL_ID}%")
            ],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n as usize)
        .unwrap_or(0)
    }

    /// `from -> to route` of every delivery after `mark` that names the
    /// brain's tool call -- the call's own trace, for a failure message.
    fn trace_of_the_call(&self, mark: i64) -> Vec<String> {
        let conn = rusqlite::Connection::open(self.root().join("colony.db")).expect("colony.db");
        let Ok(mut st) = conn.prepare(
            "SELECT from_path, to_path, headers FROM message_log WHERE rowid > ?1 \
             AND (headers LIKE ?2 OR COALESCE(body_payload, '') LIKE ?2 \
                  OR headers LIKE ?3 OR COALESCE(body_payload, '') LIKE ?3) ORDER BY rowid",
        ) else {
            return Vec::new();
        };
        st.query_map(
            rusqlite::params![mark, format!("%{TOOL_CALL_ID}%"), format!("%{TOOL}%")],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        )
        .map(|rows| {
            rows.filter_map(Result::ok)
                .map(|(from, to, headers)| {
                    let h: Value =
                        meclaw_core::serde_json::from_str(&headers).unwrap_or(Value::Null);
                    format!(
                        "{from} -> {to} route={} hop={}",
                        h["hop"]["route"], h["hop"]
                    )
                    .chars()
                    .take(400)
                    .collect()
                })
                .collect()
        })
        .unwrap_or_default()
    }

    /// Wait until a delivery after `mark` satisfies `pred`; returns it.
    async fn wait(
        &self,
        mark: i64,
        what: &str,
        pred: impl Fn(&(String, String, String)) -> bool,
    ) -> (String, String, String) {
        let end = std::time::Instant::now() + DEADLINE;
        loop {
            if let Some(hit) = self.since(mark).into_iter().find(|d| pred(d)) {
                return hit;
            }
            if std::time::Instant::now() > end {
                let dead: Vec<String> = self.drain().await.iter().map(Dead::say).collect();
                panic!("{what} never arrived; dead letters so far: {dead:#?}");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Wait until at least `n` deliveries after `mark` satisfy `pred`.
    async fn wait_count(
        &self,
        mark: i64,
        n: usize,
        what: &str,
        pred: impl Fn(&(String, String, String)) -> bool,
    ) {
        let end = std::time::Instant::now() + DEADLINE;
        loop {
            let seen = self.since(mark).iter().filter(|d| pred(d)).count();
            if seen >= n {
                return;
            }
            if std::time::Instant::now() > end {
                let dead: Vec<String> = self.drain().await.iter().map(Dead::say).collect();
                panic!("{what}: {seen} of {n} arrived; dead letters so far: {dead:#?}");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// A person's turn on the grown channel, waited until its answer arrived
    /// on that channel.
    async fn turn(&self, text: &str) {
        let mark = self.mark();
        self.h
            .send(person(MEMBER_PATH, text, Some(GROWN_CHANNEL)))
            .await;
        self.wait(mark, "the answer on the grown channel", |(to, route, _)| {
            to.starts_with(&format!("{MEMBER_PATH}/channels/{GROWN_CHANNEL}")) && route == "answer"
        })
        .await;
    }

    /// The archive echoes a run of `turns` turns owes since `mark`: one
    /// `turn_write` per participant turn (the person's and the model's), handed
    /// up to the top. They are the background a turn leaves behind its answer;
    /// the sentinel is sent only after they all arrived.
    async fn echoes(&self, mark: i64, turns: usize) {
        self.wait_count(
            mark,
            2 * turns,
            "the archive echoes of the turns",
            |(to, route, _)| to == "/os" && route == "turn_write",
        )
        .await;
    }

    /// The channel level, as the lab grows it: the probe connector into the
    /// member, bound to the assistant.
    async fn grow_channel(&self) {
        self.grow(
            "channel",
            json!({"scope": MEMBER_PATH, "level": "channel", "name": GROWN_CHANNEL,
                   "template": format!("{PROBE_CHANNEL}@1.0.0"), "assistant": AGENT,
                   "bind_chat": "974", "ctx": {"member_person": MEMBER},
                   // A channel is born asleep unless the wish says so (GH #468);
                   // the probe dials nobody, so it is armed at once.
                   "birth": "active"}),
        )
        .await;
    }

    /// The shell, as the lab adds it: one node at `/`, no edge.
    async fn shell(&self) {
        let mark = self.mark();
        let out = self
            .apply(json!({"manifest": [{"scope": "/", "diff": {
                "add_nodes": [{"name": "os", "template": pin("meclaw-os")}],
                "add_edges": []}}]}))
            .await;
        assert!(out.is_committed(), "the shell must commit; got {out:?}");
        // The shell's own receipt is the first one /os can take.
        self.wait(mark, "the shell's receipt at /os", |(to, route, _)| {
            to == "/os" && route == "mutation_committed"
        })
        .await;
    }

    /// One level, as the lab grows it: the `grow_level` WISH to `/os/builder`,
    /// its draft read off the trace, the draft applied at the mutation door.
    async fn grow(&self, level: &str, params: Value) {
        let mark = self.mark();
        let wish = json!({"request": format!("grow a {level}"), "recipe": "grow_level",
                          "params": params});
        let msg = MessageBuilder::new(Path::new("/os/builder"))
            .hop(as_map(&json!({"route": "in_build"})))
            .body(Body::Inline(json!({"messages": [{
                "origin": "tool", "type": "tool_call", "id": format!("gh974-{level}"),
                "text": wish.to_string()}]})))
            .build();
        self.h.send(msg).await;
        // The draft as the lab reads it off the trace: the builder's
        // `in_build_result`, wherever the shell's graph carries it.
        let (_, _, body) = self
            .wait(mark, &format!("the {level} draft"), |(_, route, body)| {
                route == "in_build_result" && body.contains("manifest")
            })
            .await;
        let draft: Value = meclaw_core::serde_json::from_str(&body).expect("the draft is json");
        let manifest = draft["manifest"].clone();
        assert!(
            manifest.is_array(),
            "the builder drafted no manifest for the {level}: {draft}"
        );
        let mark = self.mark();
        let out = self.apply(json!({"manifest": manifest})).await;
        assert!(
            out.is_committed(),
            "the {level} draft must commit; got {out:?}"
        );
        self.wait(
            mark,
            &format!("the {level} receipt at /os"),
            |(to, route, _)| to == "/os" && route == "mutation_committed",
        )
        .await;
    }

    /// Boot and grow the four stages; every dead letter on the way, the boot's
    /// own included, is returned for the class table.
    async fn grown() -> (Self, Vec<Dead>) {
        let lab = Lab::boot().await;
        let mut dead = lab.drain().await;
        lab.shell().await;
        dead.extend(lab.drain().await);
        lab.grow(
            "org",
            json!({"scope": "/os", "level": "org", "name": ORG, "template": pin("org")}),
        )
        .await;
        dead.extend(lab.drain().await);
        lab.grow(
            "member",
            json!({"scope": "/os/orgs/lab", "level": "member", "name": MEMBER,
                   "template": pin("member")}),
        )
        .await;
        dead.extend(lab.drain().await);
        lab.grow(
            "assistant",
            json!({"scope": MEMBER_PATH, "level": "assistant", "name": AGENT,
                   "template": pin("assistant"), "door": true,
                   "ctx": {"model": "stub-ctx-model", "model_fast": "stub-ctx-fast",
                           "model_surface": "stub-ctx-surface"},
                   "override_params": {
                       "talky/session-keeper/close": {"idle_ms": 1},
                       "talky-chat/session-keeper/close": {"idle_ms": 1}}}),
        )
        .await;
        dead.extend(lab.drain().await);
        (lab, dead)
    }

    /// The sentinel: a turn to a member this colony never grew. Its dead letter
    /// is the event every absence above is read after.
    async fn sentinel(&self, dead: &mut Vec<Dead>) -> Vec<Dead> {
        let nobody = "/os/orgs/lab/members/nobody";
        self.h.send(person(nobody, "is anybody there", None)).await;
        let end = std::time::Instant::now() + DEADLINE;
        loop {
            dead.extend(self.drain().await);
            let (hits, rest): (Vec<Dead>, Vec<Dead>) = dead
                .drain(..)
                .partition(|d| d.target.starts_with(nobody) || d.original.starts_with(nobody));
            *dead = rest;
            if !hits.is_empty() {
                return hits;
            }
            assert!(
                std::time::Instant::now() < end,
                "the misroute to {nobody} never dead-lettered"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

/// A person's words at a member's door, in the lab's form: no channel (spoken),
/// or the named channel node.
fn person(member: &str, text: &str, channel_node: Option<&str>) -> Message {
    let mut ctx = json!({"channel": "lab:974", "audience_set": AUDIENCE});
    if let Some(node) = channel_node {
        ctx["channel_node"] = json!(node);
    }
    MessageBuilder::new(Path::new(member))
        .hop(as_map(&json!({"route": "in_turn"})))
        .context(as_map(&ctx))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .build()
}

fn say(dead: &[Dead]) -> Vec<String> {
    dead.iter().map(Dead::say).collect()
}

fn report(stage: &str, dead: &[Dead]) {
    for d in dead {
        eprintln!("gh974 {stage} dead letter: {}", d.say());
    }
}

/// The way's own lanes that dead-lettered.
fn on_the_way(dead: &[Dead]) -> Vec<String> {
    dead.iter()
        .filter(|d| WAY.contains(&d.route.as_str()))
        .map(Dead::say)
        .collect()
}

// ═══════════════════════════════════════════════════════════════════ tests

/// The table is data and stays honest: every row names a lane, a reason, an
/// emitter, a target and why it is no defect, and none hides the way itself.
#[test]
fn the_class_table_declares_no_lane_of_the_way() {
    let table = classes();
    assert!(!table.is_empty(), "the observed classes are declared");
    for c in &table {
        assert!(
            !["reject", "error"].contains(&c.route.as_str()),
            "a refusal is never a class -- it dead-letters to be seen (GH #284): {c:?}"
        );
        // The way's own lanes are never a class, wherever they died: a turn,
        // an answer or a tool's result that dead-letters is a break on the
        // road a person's words take (OR-NL-171 (3)).
        assert!(
            !WAY.contains(&c.route.as_str()),
            "a class may not hide the way's own lane: {c:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn growing_a_level_leaves_only_declared_dead_letters() {
    if !shipped() {
        return;
    }
    let (lab, mut dead) = Lab::grown().await;
    let hits = lab.sentinel(&mut dead).await;
    assert_eq!(hits.len(), 1, "the sentinel: {:#?}", say(&hits));
    report("growing", &dead);
    let bad = undeclared(&dead);
    assert!(
        bad.is_empty(),
        "growing shell, org, member and assistant left dead letters no class \
         declares (fixtures/gh974_dead_letter_classes.json): {bad:#?}"
    );
    lab.h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_person_round_leaves_only_declared_dead_letters() {
    if !shipped() {
        return;
    }
    let (lab, grown) = Lab::grown().await;
    // The channel the person speaks on, grown as a level (GH #468).
    lab.grow_channel().await;
    let mut dead = lab.drain().await;
    report("channel", &dead);

    // Two turns on the grown channel; the first asks a tool.
    let round = lab.mark();
    lab.turn("the plumber came").await;
    lab.turn("and the roofer").await;
    let turns = lab.drain().await;
    report("turns", &turns);
    dead.extend(turns);

    // The session closes (idle_ms 1): the close pass, the memory's writes and
    // the close report.
    let mark = lab.mark();
    lab.h
        .send(
            MessageBuilder::new(Path::new(GENERATION_PATH))
                .hop(as_map(&json!({"route": "in_sweep"})))
                .body(Body::Inline(json!({"messages": [
                    {"origin": "user", "type": "text", "text": "sweep"}]})))
                .build(),
        )
        .await;
    lab.wait(mark, "the close report at /os", |(to, route, _)| {
        to == "/os" && route == "close_report"
    })
    .await;
    // The last step's last events: the close's batch echo and every turn's
    // archive echo. Only then the sentinel.
    lab.wait(mark, "the close batch's echo at /os", |(to, route, _)| {
        to == "/os" && route == "write"
    })
    .await;
    lab.echoes(round, 2).await;

    let hits = lab.sentinel(&mut dead).await;
    assert_eq!(hits.len(), 1, "the sentinel: {:#?}", say(&hits));
    report("round", &dead);
    let mut bad = undeclared(&grown);
    bad.extend(undeclared(&dead));
    assert!(
        bad.is_empty(),
        "a person's round left dead letters no class declares (measured 21 \
         undeclared before GH #974): {bad:#?}"
    );
    lab.h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_ways_own_messages_never_dead_letter() {
    if !shipped() {
        return;
    }
    let (lab, grown) = Lab::grown().await;
    assert!(
        on_the_way(&grown).is_empty(),
        "growing: {:#?}",
        on_the_way(&grown)
    );
    lab.grow_channel().await;
    let mut dead = lab.drain().await;
    report("channel", &dead);

    // A turn on the channel the member grew: the turn, the brain's tool call,
    // its result and the answer travel the road a person's words take.
    let mark = lab.mark();
    lab.turn("what is on today").await;
    // The tool's result HAS to arrive at the asker, or the absence below
    // would say nothing about it.
    let end = std::time::Instant::now() + DEADLINE;
    while lab.results_of_the_call(mark) == 0 {
        if std::time::Instant::now() > end {
            let dead: Vec<String> = lab.drain().await.iter().map(Dead::say).collect();
            let asked = lab._brain.recorded_requests().await.len();
            panic!(
                "the result of the brain's {TOOL} call ({TOOL_CALL_ID}) never reached \
                 the asker; brain requests {asked}; the call's trace: {:#?}; dead \
                 letters so far: {dead:#?}",
                lab.trace_of_the_call(mark)
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    lab.echoes(mark, 1).await;
    let hits = lab.sentinel(&mut dead).await;
    assert_eq!(hits.len(), 1, "the sentinel: {:#?}", say(&hits));
    report("way", &dead);
    let broke = on_the_way(&dead);
    assert!(
        broke.is_empty(),
        "the way's own messages dead-lettered on a grown channel: {broke:#?}"
    );
    lab.h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_real_misroute_still_dead_letters() {
    if !shipped() {
        return;
    }
    let lab = Lab::boot().await;
    lab.drain().await;
    lab.shell().await;
    lab.grow(
        "org",
        json!({"scope": "/os", "level": "org", "name": ORG, "template": pin("org")}),
    )
    .await;
    let mut dead = lab.drain().await;
    let hits = lab.sentinel(&mut dead).await;
    assert_eq!(
        hits.len(),
        1,
        "a turn to a member that does not exist dead-letters exactly once: {:#?}",
        say(&hits)
    );
    assert_eq!(hits[0].route, "in_turn", "the sentinel is the turn itself");
    lab.h.shutdown().await;
}

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

// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;
// harness_fold: #[path = "support/gh929_member_road.rs"]
#[allow(unused_imports)]
use crate::road; // harness_fold: mod road;

use meclaw_cells::WebCellFactory;
use meclaw_cells::vault::VaultCellFactory;
use meclaw_colony::api_dto::{MessageLogDto, MessageLogFilter};
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationDoorOutcome, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::{ColonyHandle, LogMark, NEVER_CRON, deadline, wait};
use mock_openai::{MockOpenAI, canned_chat_completion, canned_tool_calls};
use road::{Stubs, as_map, configs_under, read_json, write_json};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::oneshot;

/// The seconds a log mark reads back before its newest row (see `Lab::mark`):
/// a bound on the rows each poll reads, not a wait.
const MARK_SLACK_S: i64 = 2;
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
fn factories(vault: bool) -> Vec<(String, Arc<dyn CellFactory>)> {
    let mut f = road::factories();
    f.push((
        "web".to_string(),
        Arc::new(WebCellFactory::default()) as Arc<dyn CellFactory>,
    ));
    // GH #1092: the shell's broker with its vault, as a colony runs it. A
    // vault with no deposit answers every spend `vault_locked`.
    if vault {
        f.push((
            "vault".to_string(),
            Arc::new(VaultCellFactory) as Arc<dyn CellFactory>,
        ));
    }
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
        // The channel's own operation timeout (A) and backstop (B) are the
        // test's product deadlines on its success path (TV-Z9): one source,
        // scaled by the one time factor, far above every hang mark below. A
        // literal 15 s A was a clock this file set itself, unscaled, on the
        // python3 runner every answer of the round reaches.
        &json!({"cell": {"type": "code",
                         "message_timeout": meclaw_testing::deadline::product_backstop_ms()},
                "params": {"runner": "python3",
                           "external_timeout_ms": meclaw_testing::deadline::product_wait_ms(),
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
    /// Held: the colony root lives as long as the lab.
    _td: tempfile::TempDir,
    h: ColonyHandle,
    /// Held: a dropped stub stops answering.
    _stub: MockOpenAI,
    _closer: MockOpenAI,
    _brain: MockOpenAI,
}

impl Lab {
    async fn boot() -> Self {
        Self::boot_with(false).await
    }

    /// The lab, with the `vault` cell type registered or not.
    async fn boot_with(vault: bool) -> Self {
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

        let h = ColonyHandle::new_with_factories_at(&td, factories(vault));
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
        for (name, f) in factories(vault) {
            registry.insert(name, f);
        }
        bootstrap_from_filesystem(root, &registry, &h.runtime())
            .await
            .expect("the empty root must boot");
        Lab {
            _td: td,
            h,
            _stub: stub,
            _closer: closer,
            _brain: brain,
        }
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

    /// The point in the message log a wait reads after.
    ///
    /// Read through the colony (`ColonyHandle::log_mark`, TV-A3), never from
    /// `colony.db`. `since` bounds every later read to the rows of the last
    /// seconds: the newest row's `created_at` (seconds) when the mark was
    /// taken, less [`MARK_SLACK_S`]. A row's `created_at` is its message's
    /// creation stamp, not the writer's commit time, so the bound holds for the
    /// rows of messages created after the mark -- every row a wait here looks
    /// for, since the test sends the message that starts the round only after
    /// taking the mark. An older message committed late can fall below it;
    /// `round_of` in a hang diagnosis may miss such a row. The slack only
    /// covers a host clock that steps back a little. Without it each poll
    /// would read the whole log through the colony's own task, under load,
    /// and slow down the very round it waits for.
    async fn mark(&self) -> Mark {
        // The newest row through the paged reader (`read_log`, newest first):
        // a raw one-row `ReadMessages` would be a page read as the whole (809
        // `no_reader_takes_one_page_of_the_log_for_the_whole`).
        let newest = self
            .h
            .read_log(MessageLogFilter::default())
            .await
            .first()
            .map_or(0, |row| row.created_at);
        Mark {
            log: self.h.log_mark().await,
            since: newest - MARK_SLACK_S,
        }
    }

    /// Every delivery after `mark`, oldest first; only those to `to_prefix`
    /// when one is given.
    async fn since(&self, mark: &Mark, to_prefix: Option<&str>) -> Vec<MessageLogDto> {
        let mut rows = self
            .h
            .log_since(
                &mark.log,
                MessageLogFilter {
                    since: Some(mark.since),
                    to_path_prefix: to_prefix.map(str::to_string),
                    ..Default::default()
                },
            )
            .await;
        rows.reverse();
        rows
    }

    /// The deliveries after `mark` that satisfy `pick`, read whenever the log
    /// moved (`log_seq`, the writer's commit counter: the event "rows were
    /// committed"), under the hang mark (`wait::until_some_with`, 30 s scaled
    /// by `MECLAW_TEST_TIME_FACTOR`). On a hang the failure names what was
    /// awaited, the dead letters so far and the round `under` the given
    /// subtrees -- never a verdict on speed.
    async fn wait_for<T>(
        &self,
        mark: &Mark,
        to_prefix: Option<&str>,
        what: &str,
        under: &[&str],
        pick: impl Fn(&[MessageLogDto]) -> Option<T>,
    ) -> T {
        let seq = self.h.subscribe_log_seq();
        let read_at = std::cell::Cell::new(None);
        let (seq, read_at, pick) = (&seq, &read_at, &pick);
        wait::until_some_with(
            what,
            move || async move {
                let now = *seq.borrow();
                if read_at.get() == Some(now) {
                    return None;
                }
                read_at.set(Some(now));
                pick(&self.since(mark, to_prefix).await)
            },
            || async move {
                format!(
                    "dead letters so far: {:#?}; the round under {under:?}: {:#?}",
                    say(&self.drain().await),
                    self.round_of(mark, under).await
                )
            },
        )
        .await
    }

    /// Wait until a delivery after `mark` satisfies `pred`; returns it as
    /// `(to_path, route, body_payload)`.
    async fn wait(
        &self,
        mark: &Mark,
        what: &str,
        under: &[&str],
        pred: impl Fn(&(String, String, String)) -> bool,
    ) -> (String, String, String) {
        self.wait_for(mark, None, what, under, |rows| {
            rows.iter().map(delivery).find(|d| pred(d))
        })
        .await
    }

    /// GH #1092: wait until a delivery after `mark` satisfies `pred` on its
    /// target and its whole hop (a receipt's `error_code` rides there).
    async fn wait_hop(
        &self,
        mark: &Mark,
        what: &str,
        under: &[&str],
        pred: impl Fn(&str, &Value) -> bool,
    ) {
        self.wait_for(mark, None, what, under, |rows| {
            rows.iter()
                .any(|row| pred(&row.to_path, &headers(row)["hop"]))
                .then_some(())
        })
        .await
    }

    /// Wait until at least `n` deliveries after `mark` satisfy `pred`.
    async fn wait_count(
        &self,
        mark: &Mark,
        n: usize,
        what: &str,
        pred: impl Fn(&(String, String, String)) -> bool,
    ) {
        self.wait_for(mark, None, what, &["/os"], |rows| {
            (rows.iter().map(delivery).filter(|d| pred(d)).count() >= n).then_some(())
        })
        .await
    }

    /// GH #1092: the deliveries after `mark` from or to these subtrees --
    /// `from -> to route [error_code] context keys` -- for a failure message.
    async fn round_of(&self, mark: &Mark, under: &[&str]) -> Vec<String> {
        self.since(mark, None)
            .await
            .iter()
            .filter(|row| {
                under
                    .iter()
                    .any(|u| row.from_path.starts_with(u) || row.to_path.starts_with(u))
            })
            .map(|row| {
                let h = headers(row);
                let keys: Vec<String> = h["context"]
                    .as_object()
                    .map(|c| c.keys().cloned().collect())
                    .unwrap_or_default();
                format!(
                    "{} -> {} {} [{}] grant={} op={} ctx={keys:?}",
                    row.from_path,
                    row.to_path,
                    h["hop"]["route"].as_str().unwrap_or_default(),
                    h["hop"]["error_code"].as_str().unwrap_or_default(),
                    h["hop"]["grant_id"].as_str().unwrap_or_default(),
                    h["hop"]["operation"].as_str().unwrap_or_default(),
                )
            })
            .collect()
    }

    /// Deliveries after `mark` that bring the brain's own tool call's result
    /// back into the asking generation: lane `in_tool` (the member's memory
    /// answers on `tool_result`, the member hands it down on `in_tool`) with
    /// `TOOL_CALL_ID` on the hop. Other results reach a generation too (a
    /// start-up schema answer); only this one is the call's.
    fn is_the_calls_result(row: &MessageLogDto) -> bool {
        let body = row.body_payload.as_deref().unwrap_or_default();
        row.to_path.starts_with(GENERATION_PATH)
            && (row.headers_json.contains(TOOL_CALL_ID) || body.contains(TOOL_CALL_ID))
            && row.headers_json.contains("\"in_tool\"")
    }

    /// A person's turn on the grown channel, waited until its answer arrived
    /// on that channel.
    async fn turn(&self, text: &str) {
        let mark = self.mark().await;
        self.h
            .send(person(MEMBER_PATH, text, Some(GROWN_CHANNEL)))
            .await;
        // The answer's delivery to the channel node is the event; the
        // channel's own python3 run on it comes after that row and cannot
        // turn it into an error. An error the channel raised on an earlier
        // delivery travels up its `error` lane and is a dead letter or a
        // refusal the class table never declares (a refusal is never a
        // class), so the dead-letter check, not this wait, reads it.
        //
        // A turn is a whole round of the grown organism (the brain's tool
        // call, the recall, the channel's code cell), so it waits under its
        // own mark (`deadline::TURN_HANG`, whose doc carries the measured
        // basis and why `HANG` does not carry a turn). The wait reads
        // the log whenever the writer committed rows (`log_seq`), never on a
        // clock of its own.
        let channel = format!("{MEMBER_PATH}/channels/{GROWN_CHANNEL}");
        let mut seq = self.h.subscribe_log_seq();
        deadline::turn_hang_with(
            "the answer on the grown channel",
            async {
                loop {
                    seq.borrow_and_update();
                    let rows = self.since(&mark, None).await;
                    if rows
                        .iter()
                        .map(delivery)
                        .any(|(to, route, _)| to.starts_with(&channel) && route == "answer")
                    {
                        return;
                    }
                    if seq.changed().await.is_err() {
                        // The writer is gone: nothing commits again, the mark names it.
                        std::future::pending::<()>().await;
                    }
                }
            },
            || async {
                format!(
                    "dead letters so far: {:#?}; the round under {:?}: {:#?}",
                    say(&self.drain().await),
                    [MEMBER_PATH],
                    self.round_of(&mark, &[MEMBER_PATH]).await
                )
            },
        )
        .await;
    }

    /// The archive echoes a run of `turns` turns owes since `mark`: one
    /// `turn_write` per participant turn (the person's and the model's), handed
    /// up to the top. They are the background a turn leaves behind its answer;
    /// the sentinel is sent only after they all arrived.
    async fn echoes(&self, mark: &Mark, turns: usize) {
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
        // GH #1061 (#801): the translator asks the shell's broker for its
        // key, and this lab deposits none. Anonymous, it speaks to the lab's
        // stub as it did when the key came out of `.env`; the shipped shell
        // with its grant and no deposit is GH #1092's own test below.
        self.shell_as(json!({"llm-registry/translate": {"credential_grant_id": ""}}))
            .await;
    }

    /// The shell with these `override_params` (`{}` = as shipped).
    async fn shell_as(&self, override_params: Value) {
        let mark = self.mark().await;
        let out = self
            .apply(json!({"manifest": [{"scope": "/", "diff": {
                "add_nodes": [{"name": "os", "template": pin("meclaw-os"),
                    "override_params": override_params}],
                "add_edges": []}}]}))
            .await;
        assert!(out.is_committed(), "the shell must commit; got {out:?}");
        // The shell's own receipt is the first one /os can take.
        self.wait(
            &mark,
            "the shell's receipt at /os",
            &[],
            |(to, route, _)| to == "/os" && route == "mutation_committed",
        )
        .await;
    }

    /// One level, as the lab grows it: the `grow_level` WISH to `/os/builder`,
    /// its draft read off the trace, the draft applied at the mutation door.
    async fn grow(&self, level: &str, params: Value) {
        let mark = self.mark().await;
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
            .wait(
                &mark,
                &format!("the {level} draft"),
                &["/os/builder"],
                |(_, route, body)| route == "in_build_result" && body.contains("manifest"),
            )
            .await;
        let draft: Value = meclaw_core::serde_json::from_str(&body).expect("the draft is json");
        let manifest = draft["manifest"].clone();
        assert!(
            manifest.is_array(),
            "the builder drafted no manifest for the {level}: {draft}"
        );
        let mark = self.mark().await;
        let out = self.apply(json!({"manifest": manifest})).await;
        assert!(
            out.is_committed(),
            "the {level} draft must commit; got {out:?}"
        );
        self.wait(
            &mark,
            &format!("the {level} receipt at /os"),
            &[],
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
        // Polled: a dead letter has no hook of its own. Drained letters are
        // kept for the class table, the sentinel's own are returned.
        let pool = std::cell::RefCell::new(std::mem::take(dead));
        let pool_ref = &pool;
        let hits = wait::until_some_with(
            &format!("the misroute to {nobody} dead-letters"),
            move || async move {
                let pool = pool_ref;
                let fresh = self.drain().await;
                let mut pool = pool.borrow_mut();
                pool.extend(fresh);
                let (hits, rest): (Vec<Dead>, Vec<Dead>) = pool
                    .drain(..)
                    .partition(|d| d.target.starts_with(nobody) || d.original.starts_with(nobody));
                *pool = rest;
                (!hits.is_empty()).then_some(hits)
            },
            move || async move { format!("dead letters so far: {:#?}", say(&pool_ref.borrow())) },
        )
        .await;
        *dead = pool.into_inner();
        hits
    }
}

/// `(to_path, route, body_payload)` of one delivery.
fn delivery(row: &MessageLogDto) -> (String, String, String) {
    let route = headers(row)["hop"]["route"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    (
        row.to_path.clone(),
        route,
        row.body_payload.clone().unwrap_or_default(),
    )
}

/// The headers of one delivery as JSON (`Null` if they do not parse).
fn headers(row: &MessageLogDto) -> Value {
    meclaw_core::serde_json::from_str(&row.headers_json).unwrap_or(Value::Null)
}

/// A point in the colony's message log (`Lab::mark`).
struct Mark {
    log: LogMark,
    /// Every read after the mark starts at this `created_at` second.
    since: i64,
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
    let round = lab.mark().await;
    lab.turn("the plumber came").await;
    lab.turn("and the roofer").await;
    let turns = lab.drain().await;
    report("turns", &turns);
    dead.extend(turns);

    // The session closes (idle_ms 1): the close pass, the memory's writes and
    // the close report.
    let mark = lab.mark().await;
    lab.h
        .send(
            MessageBuilder::new(Path::new(GENERATION_PATH))
                .hop(as_map(&json!({"route": "in_sweep"})))
                .body(Body::Inline(json!({"messages": [
                    {"origin": "user", "type": "text", "text": "sweep"}]})))
                .build(),
        )
        .await;
    lab.wait(
        &mark,
        "the close report at /os",
        &[GENERATION_PATH],
        |(to, route, _)| to == "/os" && route == "close_report",
    )
    .await;
    // The last step's last events: the close's batch echo and every turn's
    // archive echo. Only then the sentinel.
    lab.wait(
        &mark,
        "the close batch's echo at /os",
        &[GENERATION_PATH],
        |(to, route, _)| to == "/os" && route == "write",
    )
    .await;
    lab.echoes(&round, 2).await;

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
    let mark = lab.mark().await;
    lab.turn("what is on today").await;
    // The tool's result HAS to arrive at the asker, or the absence below
    // would say nothing about it.
    lab.wait_for(
        &mark,
        Some(GENERATION_PATH),
        &format!("the result of the brain's {TOOL} call ({TOOL_CALL_ID}) reaches the asker"),
        &[MEMBER_PATH],
        |rows| rows.iter().any(Lab::is_the_calls_result).then_some(()),
    )
    .await;
    lab.echoes(&mark, 1).await;
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

/// GH #1092 — the shipped shell in a colony with no vault deposit leaves only
/// declared dead letters. Its translator spends a grant (#801), the broker
/// denies the round `vault_locked`, and in 0.62.0 that denial died at
/// `/os/access` (`hive_no_route`, sender `/os/access/invoke`; 8 of 150 orga
/// lab runs red on exactly this). The translator's turn is answered either
/// way -- by the denial when it comes home, by the round's deadline when it
/// does not -- so the receipt is the event every absence is read after, and
/// the discriminator is the dead-letter queue, not the clock.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_shell_with_no_vault_deposit_leaves_only_declared_dead_letters() {
    if !shipped() {
        return;
    }
    let lab = Lab::boot_with(true).await;
    let mut dead = lab.drain().await;
    let mark = lab.mark().await;
    lab.shell_as(json!({})).await;
    lab.wait_hop(
        &mark,
        "the translator's credential_pending receipt",
        &["/os/access", "/os/llm-registry"],
        |to, hop| to.starts_with("/os/llm-registry/") && hop["error_code"] == "credential_pending",
    )
    .await;
    let hits = lab.sentinel(&mut dead).await;
    assert_eq!(hits.len(), 1, "the sentinel: {:#?}", say(&hits));
    report("no vault deposit", &dead);
    let bad = undeclared(&dead);
    assert!(
        bad.is_empty(),
        "a shell with no vault deposit left dead letters no class declares \
         (fixtures/gh974_dead_letter_classes.json): {bad:#?}\nthe round: {:#?}",
        lab.round_of(&mark, &["/os/access", "/os/llm-registry"])
            .await
    );
    lab.h.shutdown().await;
}

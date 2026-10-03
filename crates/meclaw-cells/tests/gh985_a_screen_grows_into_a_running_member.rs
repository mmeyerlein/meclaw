//! GH #985 — a screen draft grows into a RUNNING member, end to end.
//!
//! The finding (wave lab, OR-NL.W.6): a `screen` level (`display`) drafted by
//! the builder's `grow_level` and applied at the mutation door into a member
//! that was already running was refused, `resume_requires_stopped_cell` at
//! `…/channels/display/clock`. No test applied a screen draft end to end before
//! (gh466/gh472/gh543 check the rendering only).
//!
//! The lab here grows the shell, an org and a member exactly as the builder
//! drafts them (the `grow_level` wish to `/os/builder`, the draft read off the
//! trace, applied at the door), then a screen into that running member, and
//! reads at the receiver: a view sent to the member's channels on the new
//! screen's name arrives at the screen's own compose cell. Measured with this
//! lab: a screen under a name of its own grows; the refusal came from drafting
//! the member's OWN screen `display` a second time (second test).
//!
//! **Substitution:** every `llm` cell of the library copy answers from a local
//! stub, every cron is pushed out of the run. Guarded like every
//! template-reading test (GH #49).

#[path = "mock_openai.rs"]
mod mock_openai;
#[path = "support/gh929_member_road.rs"]
mod road;

use meclaw_cells::WebCellFactory;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationDoorOutcome, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, MessageBuilder, Path, Uuid};
use meclaw_testing::{ColonyHandle, NEVER_CRON};
use mock_openai::{MockOpenAI, canned_chat_completion};
use road::{Stubs, as_map, configs_under, read_json, write_json};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;

/// Failure marker only (a cold builder render on a loaded host); every wait
/// below ends on an event.
const DEADLINE: Duration = Duration::from_secs(90);
const MEMBER_PATH: &str = "/os/orgs/lab/members/member";
const CHANNELS: &str = "/os/orgs/lab/members/member/channels";

fn repo(rel: &str) -> std::path::PathBuf {
    road::repo(rel)
}

fn shipped() -> bool {
    [
        "templates/meclaw-os/config.json",
        "templates/builder/recipes/config.json",
        "templates/member/config.json",
        "templates/display/config.json",
    ]
    .iter()
    .all(|rel| repo(rel).is_file())
}

fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    let mut f = road::factories();
    f.push((
        "web".to_string(),
        Arc::new(WebCellFactory::default()) as Arc<dyn CellFactory>,
    ));
    f
}

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

struct Lab {
    td: tempfile::TempDir,
    h: ColonyHandle,
    _stub: MockOpenAI,
}

impl Lab {
    async fn boot() -> Self {
        let stub = MockOpenAI::start(vec![canned_chat_completion(road::REPLY, "stop")]).await;
        let td = tempfile::TempDir::new().expect("tempdir");
        let root = td.path();
        let templates = root.join("templates");
        copy_tree(&repo("templates"), &templates);
        quiet_crons(&templates);
        road::point_llms_at_stubs(
            &templates,
            &Stubs {
                scripted: HashMap::new(),
                background: stub.base_url.clone(),
            },
        );
        road::write_env(root, &templates, &stub.base_url);
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
            .expect("the rescan must not abort");
        let mut registry = CellFactoryRegistry::new();
        for (name, f) in factories() {
            registry.insert(name, f);
        }
        bootstrap_from_filesystem(root, &registry, &h.runtime())
            .await
            .expect("the empty root must boot");
        Lab { td, h, _stub: stub }
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

    fn mark(&self) -> i64 {
        let conn = rusqlite::Connection::open(self.td.path().join("colony.db")).expect("colony.db");
        conn.query_row("SELECT COALESCE(MAX(rowid), 0) FROM message_log", [], |r| {
            r.get(0)
        })
        .unwrap_or(0)
    }

    /// `(to_path, route, body_payload)` of every delivery after `mark`.
    fn since(&self, mark: i64) -> Vec<(String, String, String)> {
        let conn = rusqlite::Connection::open(self.td.path().join("colony.db")).expect("colony.db");
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
                let dead: Vec<String> = self
                    .h
                    .drain_dead_letters()
                    .await
                    .iter()
                    .map(|d| {
                        format!(
                            "{} -> {} [{}]",
                            d.sender_path.as_str(),
                            d.resolved_target.as_str(),
                            d.reason.as_code()
                        )
                    })
                    .collect();
                panic!("{what} never arrived; dead letters so far: {dead:#?}");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn shell(&self) {
        let mark = self.mark();
        let out = self
            .apply(json!({"manifest": [{"scope": "/", "diff": {
                "add_nodes": [{"name": "os", "template": pin("meclaw-os")}],
                "add_edges": []}}]}))
            .await;
        assert!(out.is_committed(), "the shell must commit; got {out:?}");
        self.wait(mark, "the shell's receipt at /os", |(to, route, _)| {
            to == "/os" && route == "mutation_committed"
        })
        .await;
    }

    /// The builder's draft of one level: the `grow_level` wish to
    /// `/os/builder`, the draft read off the trace.
    async fn draft(&self, level: &str, params: Value) -> Value {
        let mark = self.mark();
        let wish = json!({"request": format!("grow a {level}"), "recipe": "grow_level",
                          "params": params});
        let msg = MessageBuilder::new(Path::new("/os/builder"))
            .hop(as_map(&json!({"route": "in_build"})))
            .body(Body::Inline(json!({"messages": [{
                "origin": "tool", "type": "tool_call", "id": format!("w6-{level}"),
                "text": wish.to_string()}]})))
            .build();
        self.h.send(msg).await;
        let (_, _, body) = self
            .wait(mark, &format!("the {level} draft"), |(_, route, body)| {
                route == "in_build_result" && body.contains("manifest")
            })
            .await;
        let draft: Value = meclaw_core::serde_json::from_str(&body).expect("the draft is json");
        assert!(
            draft["manifest"].is_array(),
            "the builder drafted no manifest for the {level}: {draft}"
        );
        draft["manifest"].clone()
    }

    async fn grow(&self, level: &str, params: Value) -> MutationDoorOutcome {
        let manifest = self.draft(level, params).await;
        let mark = self.mark();
        let out = self.apply(json!({"manifest": manifest})).await;
        if out.is_committed() {
            self.wait(
                mark,
                &format!("the {level} receipt at /os"),
                |(to, route, _)| to == "/os" && route == "mutation_committed",
            )
            .await;
        }
        out
    }

    /// The shell, an org and a member, each committed: a running member with
    /// the screen and the app it always gets (GH #543).
    async fn member() -> Self {
        let lab = Lab::boot().await;
        lab.shell().await;
        let out = lab
            .grow(
                "org",
                json!({"scope": "/os", "level": "org", "name": "lab", "template": pin("org")}),
            )
            .await;
        assert!(out.is_committed(), "the org must commit; got {out:?}");
        let out = lab
            .grow(
                "member",
                json!({"scope": "/os/orgs/lab", "level": "member", "name": "member",
                       "template": pin("member")}),
            )
            .await;
        assert!(out.is_committed(), "the member must commit; got {out:?}");
        lab
    }

    /// A view for the screen `name`, sent where a member's application sends
    /// it: to the member's channels, on the screen's name.
    async fn view_reaches(&self, name: &str) {
        let mark = self.mark();
        let msg = MessageBuilder::new(Path::new(CHANNELS))
            .hop(as_map(&json!({"route": "view"})))
            .context(as_map(&json!({"channel_node": name,
                                    "audience_set": r#"["member:member"]"#})))
            .body(Body::Inline(
                json!({"view": {"id": "w6-probe", "title": "probe"}}),
            ))
            .build();
        self.h.send(msg).await;
        let compose = format!("{CHANNELS}/{name}/compose");
        self.wait(mark, &format!("the view at {compose}"), |(to, route, _)| {
            to == &compose && route == "in_view"
        })
        .await;
    }
}

/// A second screen, drafted by the builder, grows into the running member and
/// a view reaches it — the member's own screen still answers beside it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_screen_draft_grows_into_a_running_member_end_to_end() {
    if !shipped() {
        return;
    }
    let lab = Lab::member().await;
    lab.view_reaches("display").await;
    let out = lab
        .grow(
            "screen",
            json!({"scope": MEMBER_PATH, "level": "screen", "name": "wall",
                   "template": pin("display")}),
        )
        .await;
    assert!(
        out.is_committed(),
        "a screen draft must grow into a running member; got {out:?}"
    );
    lab.view_reaches("wall").await;
    lab.view_reaches("display").await;
    lab.h.shutdown().await;
}

/// The finding, measured: the screen the lab drafted was the member's OWN
/// screen, `display`, which the member level grows with the member (GH #543).
/// Re-adding a running subtree is a resume of running cells, and the spec
/// answers exactly that (`docs/meclaw-overview.md` § Mutations-Fehlercodes,
/// `resume_requires_stopped_cell`, at the first running cell inside it). Pinned
/// here: the refusal is pre-destructive and the standing screen keeps answering.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_screen_the_member_has_drafted_again_is_refused_and_keeps_answering() {
    if !shipped() {
        return;
    }
    let lab = Lab::member().await;
    let out = lab
        .grow(
            "screen",
            json!({"scope": MEMBER_PATH, "level": "screen", "name": "display",
                   "template": pin("display")}),
        )
        .await;
    assert!(!out.is_committed(), "a second display was staged: {out:?}");
    let said = format!("{out:?}");
    assert!(
        said.contains("resume_requires_stopped_cell")
            && said.contains(&format!("{CHANNELS}/display/")),
        "the standing screen was not answered as a resume of its running cells: {said}"
    );
    lab.view_reaches("display").await;
    lab.h.shutdown().await;
}

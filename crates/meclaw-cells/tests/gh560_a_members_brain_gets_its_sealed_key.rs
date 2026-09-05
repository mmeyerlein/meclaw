//! GH #560 — the person's own broker hands the person's own brain its key,
//! over a v-lane, and nothing of the value is on record.
//!
//! WHAT THIS FILE IS
//! =================
//! Authorisation and key ownership are two different jobs. The shell's `access`
//! answers the submitter's policy questions; the provider keys an agent's brains
//! burn belong to the **person**. GH #560 gives `templates/member` an `access`
//! occupant of its own and wires each brain to it with the two edges of the
//! credential lane — as **v-lanes** (GH #559), because between
//! `<member>/assistants/<agent>/talky/brain` and `<member>/access` lie three
//! levels and the innermost of them (`talky`) is sealed.
//!
//! Two claims, one test each:
//!
//! | claim | test |
//! |---|---|
//! | the member's brain answers with a key it never had, over the v-lane alone | [`a_the_members_brain_gets_its_sealed_key`] |
//! | a v-lane onto a cell the target names no connect point for is refused | [`b_a_lane_without_a_connect_point_is_refused_by_name`] |
//!
//! WHY THE WHOLE TREE IS GROWN
//! ===========================
//! The point of the issue is the DISTANCE. A test that put the brain next to the
//! broker would measure `examples/vault-pilot` a second time
//! (`gh452_the_vault_pilot_grows_a_granted_credential.rs` already does, end to
//! end). What is new here is that the sealed box crosses `assistants`, the
//! generation and the sealed `talky` rim in ONE hop, because `talky@4.6.1`
//! declares `./brain` as the connect point of that lane and Stage 6 checks it.
//! So the member and the generation are grown from the shipped library through
//! the mutation door, exactly the way `templates/member/README.md` §
//! *The credential v-lanes* prescribes, and the edges under test are the ones
//! that README writes out.
//!
//! **Substitutions, named rather than hidden.** One: the `llm` cell type is
//! real for the one brain this file is about (`…/scribe/talky/brain`, against a
//! mock provider) and an inert lazy double everywhere else. A member carries
//! brains in its memory hive and a generation carries one more in `cogny`;
//! spawning those for real would open connections to a provider over the
//! network, and none of them is what is being measured. The double is the same
//! device `gh475_a_member_reaches_the_keeper_it_holds.rs` uses, for the same
//! reason — here it is merely narrowed to a path instead of applied to the type.
//!
//! Guarded like every template-reading test (GH #49): a tree that does not carry
//! the library is skipped, never judged.

use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_cells::vault::VaultCellFactory;
use meclaw_cells::{
    BashCellFactory, EditCellFactory, FileCellFactory, LlmCellFactory, McpCellFactory,
    WebFetchCellFactory, WebSearchCellFactory,
};
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationDoorOutcome, RespawnFn, SpawnedCellKind,
    WakeFn, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Map, Value, from_str, json, to_string_pretty};
use meclaw_core::{Body, JsonValue, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mock_http::{MockResponse, start_mock_server_capturing};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// The credential the member's vault holds. A fixture, not a key — the shape is
/// deliberately unmistakable if it ever shows up where it must not.
const SECRET: &str = "sk-test-not-a-key-gh560";
/// The vault passphrase, named by `params.unlock_env` in the manifest.
const PASSPHRASE: &str = "a passphrase nobody guesses gh560";
/// The environment variable the manifest names in `unlock_env`.
const UNLOCK_ENV: &str = "GH560_MEMBER_VAULT_PASSPHRASE";
/// The credential's catalogue name.
const CRED_REF: &str = "cred:example-provider:primary";
const MEMBER: &str = "alex";
const AGENT: &str = "scribe";
/// The grant the `talky` brain names. One grant per CONSUMER, because the
/// answer edge is addressed by `hop.grant_id`: two brains sharing one handle
/// would both be handed every box, and a box a cell never asked for costs the
/// other cell's parked turns their receipt.
const GRANT_TALKY: &str = "grant:example-provider-primary@member-alex/talky";
const GRANT_COGNY: &str = "grant:example-provider-primary@member-alex/cogny";

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped() -> bool {
    [
        "templates/member/config.json",
        "templates/member/access/config.json",
        "templates/access/store/config.json",
        "templates/assistant/config.json",
        "templates/talky/config.json",
        "templates/cogny/config.json",
    ]
    .iter()
    .all(|rel| repo(rel).is_file())
}

// ═══════════════════════════════════════════════════ the brain that is not real

/// A lazy factory that accepts every `llm` params block and runs nothing.
///
/// It stands in for every brain this file is NOT about — the memory hive's and
/// `cogny`'s. A spawned `llm` opens an HTTP client against whatever `base_url`
/// its template names, and the library's brains name a real provider.
struct InertLlm;

impl CellFactory for InertLlm {
    fn validate_params(&self, params: &JsonValue) -> Result<(), String> {
        // The real factory's judgement, so a params block this tree could not
        // boot is refused here too.
        LlmCellFactory.validate_params(params)
    }

    fn is_lazy(&self) -> bool {
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_cell(
        self: Arc<Self>,
        _path: Path,
        _params: JsonValue,
        _outputs_tx: mpsc::Sender<meclaw_core::CellEmission>,
        _cell_dir: std::path::PathBuf,
        _contract: meclaw_colony::ContractView,
        _colony_inbox_tx: mpsc::Sender<ColonyMsg>,
        _idle_timeout: Option<Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<Duration>,
        _blob_store: Option<Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let capacity = mailbox_capacity.max(1);
        let (sender, receiver) = mpsc::channel::<Message>(capacity);
        let wake: WakeFn = Box::new(|mut rx: mpsc::Receiver<Message>| {
            tokio::spawn(async move { while rx.recv().await.is_some() {} });
            let (stop_tx, _stop_rx) = oneshot::channel::<()>();
            let (_death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
            (stop_tx, death_ack_rx)
        });
        let respawn: RespawnFn = Box::new(move || {
            let (tx, mut rx) = mpsc::channel::<Message>(capacity);
            let (peace_tx, peace_rx) = oneshot::channel::<()>();
            let (_backstop_tx, backstop_rx) = oneshot::channel::<()>();
            let join = tokio::spawn(async move {
                let _peace_keep = peace_tx;
                while rx.recv().await.is_some() {}
            });
            (tx, join, peace_rx, backstop_rx)
        });
        let (stop_tx, _stop_rx) = oneshot::channel::<()>();
        let (_death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
        Ok(SpawnedCellKind::Dormant {
            sender,
            receiver,
            wake,
            stop_tx,
            death_ack_rx,
            respawn,
        })
    }
}

/// The real `llm` for one path, the double for every other.
///
/// The factory is handed the cell's path, so the substitution can be exactly as
/// wide as the claim: one brain answers a provider, and no other brain in this
/// tree opens a socket.
struct BrainUnderTest {
    real_at: String,
    real: LlmCellFactory,
    inert: Arc<InertLlm>,
}

impl CellFactory for BrainUnderTest {
    fn validate_params(&self, params: &JsonValue) -> Result<(), String> {
        self.real.validate_params(params)
    }

    fn is_lazy(&self) -> bool {
        // The double is lazy and the real cell is not; a factory answers this
        // once for the type, so the honest answer is the eager one — a cell
        // that is spawned at boot is spawned at boot either way.
        LlmCellFactory.is_lazy()
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        params: JsonValue,
        outputs_tx: mpsc::Sender<meclaw_core::CellEmission>,
        cell_dir: std::path::PathBuf,
        contract: meclaw_colony::ContractView,
        colony_inbox_tx: mpsc::Sender<ColonyMsg>,
        idle_timeout: Option<Duration>,
        cell_timeout: i64,
        message_timeout: Option<Duration>,
        blob_store: Option<Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        if path.as_str() == self.real_at {
            return Arc::new(LlmCellFactory).spawn_cell(
                path,
                params,
                outputs_tx,
                cell_dir,
                contract,
                colony_inbox_tx,
                idle_timeout,
                cell_timeout,
                message_timeout,
                blob_store,
                mailbox_capacity,
            );
        }
        Arc::clone(&self.inert).spawn_cell(
            path,
            params,
            outputs_tx,
            cell_dir,
            contract,
            colony_inbox_tx,
            idle_timeout,
            cell_timeout,
            message_timeout,
            blob_store,
            mailbox_capacity,
        )
    }
}

fn factories(real_brain: &str) -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![
        (
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        ),
        ("store".to_string(), Arc::new(StoreCellFactory)),
        ("timer".to_string(), Arc::new(TimerCellFactory)),
        ("bash".to_string(), Arc::new(BashCellFactory)),
        ("edit".to_string(), Arc::new(EditCellFactory)),
        ("file".to_string(), Arc::new(FileCellFactory)),
        ("mcp".to_string(), Arc::new(McpCellFactory)),
        ("vault".to_string(), Arc::new(VaultCellFactory)),
        ("web_fetch".to_string(), Arc::new(WebFetchCellFactory)),
        ("web_search".to_string(), Arc::new(WebSearchCellFactory)),
        (
            "llm".to_string(),
            Arc::new(BrainUnderTest {
                real_at: real_brain.to_string(),
                real: LlmCellFactory,
                inert: Arc::new(InertLlm),
            }),
        ),
    ]
}

// ══════════════════════════════════════════════════════════════ the plumbing

fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

fn write_json(path: &std::path::Path, v: &Value) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, to_string_pretty(v).unwrap()).unwrap();
}

/// Every `${VAR}` the library references WITHOUT a default, bound to a dummy,
/// plus the four crons pushed out of this run's reach — a nightly close or a
/// dream firing mid-run would emit into edges this topology never drew.
fn dummy_env(source: &std::path::Path) -> String {
    let mut names = std::collections::BTreeSet::new();
    let mut stack = vec![source.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let Ok(raw) = std::fs::read_to_string(&p) else {
                continue;
            };
            let mut rest = raw.as_str();
            while let Some(start) = rest.find("${") {
                rest = &rest[start + 2..];
                let Some(end) = rest.find('}') else { break };
                let name = &rest[..end];
                if !name.contains(":-")
                    && !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
                {
                    names.insert(name.to_string());
                }
                rest = &rest[end + 1..];
            }
        }
    }
    // Only the provider lane is left to fill: since GH #138 the two crons this
    // helper used to append (`AFFINITY_PUSH_CRON`, `KEEPER_NIGHT_CRON`) are
    // params of the cells that read them, and are pushed out of the run's way
    // with an `override_params` entry instead of with a line here.
    names
        .into_iter()
        .map(|n| format!("{n}=dummy-{n}\n"))
        .collect()
}

/// The nightly consolidation, pushed to a date this run cannot reach.
///
/// It was a `MEMORY_DREAM_CRON=` line in the `.env` above until GH #138. The
/// hive's schedule is a LITERAL of `memory-hive/clock`'s own params now, so
/// such a line would be read by nothing at all: the night would fire into this
/// run and nobody would say so. `override_params` replaces the whole
/// `schedules` key -- the key that EXISTS under a timer's params, which is the
/// only kind GH #294 accepts -- and the timer plans on what it finds there
/// (`crates/meclaw-cells/tests/gh138_memory_hive_params.rs` is the proof).
fn quiet_night() -> Value {
    json!({"schedules": [{
        "schedule_id": "0190a3f2-0000-7000-8000-00000000dead",
        "schedule_name": "nightly-dream",
        "cron": "0 0 4 1 1 *",
        "emit_to": "../dream-glue",
        "emit_body": {"messages": [{"origin": "user", "type": "text", "text": "nightly-dream"}]},
        "emit_headers": {}
    }]})
}

/// The record hive's push tick, pushed to a date this run cannot reach.
///
/// It was an `AFFINITY_PUSH_CRON=` line in the `.env` above until GH #138. The
/// hive's cadence is a LITERAL of `affinity/clock`'s own params now, so such a
/// line would be read by nothing at all: the lane would tick into this run
/// every five minutes and nobody would say so. `override_params` replaces the
/// whole `schedules` key -- the key that EXISTS under a timer's params, which
/// is the only kind GH #294 accepts -- and the timer plans on what it finds
/// there (`crates/meclaw-cells/tests/gh138_affinity_firewall_params.rs` is the
/// proof).
fn quiet_push() -> Value {
    json!({"schedules": [{
        "schedule_id": "0190a3f2-0000-7000-8000-00000000beef",
        "schedule_name": "affinity-push",
        "cron": "0 0 4 1 1 *",
        "emit_to": "../push",
        "emit_body": {"messages": [{"origin": "user", "type": "text", "text": "affinity-push"}]},
        "emit_headers": {}
    }]})
}

/// The shell the member is grown into: one container and one terminal drain.
async fn boot(td: &tempfile::TempDir, real_brain: &str) -> ColonyHandle {
    let root = td.path();
    if !root.join("templates").is_dir() {
        copy_tree(&repo("templates"), &root.join("templates"));
        // The keeper's nightly close sweep, pushed to a date this run cannot reach.
        // It was a `KEEPER_NIGHT_CRON` line in the `.env` below until GH #138: the
        // schedule is a LITERAL of `session-keeper/night`'s own params now, so such
        // a line is read by nothing at all -- the sweep would fire into this run and
        // nobody would say so. The library copy is this tree's own, so writing the
        // key into it is what an `override_params` entry does to a staged config
        // (`crates/meclaw-cells/tests/gh138_keeper_summarizer_dispatcher_params.rs`
        // is the proof that the timer plans on what it finds there).
        meclaw_testing::quiet_keeper_night(&root.join("templates/session-keeper"));
        let mut edges = vec![json!({"from": ".", "to": "./members",
                    "condition": "has(hop.route) && hop.route == 'in_turn'"})];
        for lane in [
            "answer",
            "ack",
            "reject",
            "error",
            "write",
            "turn_write",
            "prune",
            "build",
            "bundle",
            "close_report",
            "export_done",
            "pack_ack",
        ] {
            edges.push(json!({"from": "./members", "to": "./sink",
                              "condition": format!("has(hop.route) && hop.route == '{lane}'")}));
        }
        write_json(
            &root.join("main/config.json"),
            &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}}),
        );
        write_json(
            &root.join("main/members/config.json"),
            &json!({"cell": {"type": "hive"}}),
        );
        // The shipped terminal, verbatim: every lane the member raises ends
        // somewhere, so nothing this file is not about dead-letters.
        write_json(
            &root.join("main/sink/config.json"),
            &from_str::<Value>(
                &std::fs::read_to_string(repo("templates/terminal/config.json")).unwrap(),
            )
            .unwrap(),
        );
        std::fs::write(root.join(".env"), dummy_env(&root.join("templates"))).unwrap();
    }

    let h = ColonyHandle::new_with_factories_at(td, factories(real_brain));
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: root.join("templates"),
            ack: ack_tx,
        })
        .await
        .expect("rescan");
    ack_rx.await.expect("rescan ack").expect("rescan aborted");
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories(real_brain) {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(root, &registry, &h.runtime())
        .await
        .expect("the shell must boot");
    h
}

async fn apply(h: &ColonyHandle, payload: Value) -> MutationDoorOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
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

// ═════════════════════════════════════════════════════════════ the manifests

fn member_manifest() -> Value {
    json!({"manifest": [{
        "scope": "/members",
        "diff": {
            "add_nodes": [{"name": MEMBER, "template": "member@1.6.0",
                           "override_params": {
                               "access/vault": {"unlock_env": UNLOCK_ENV},
                               "memory-hive/clock": quiet_night(),
                               "affinity/clock": quiet_push()}}],
            "add_edges": [
                {"from": ".", "to": format!("./{MEMBER}"),
                 "condition": "has(hop.route) && hop.route == 'in_turn'"},
                {"from": format!("./{MEMBER}"), "to": ".",
                 "condition": "has(hop.route) && hop.route == 'answer'"},
                {"from": format!("./{MEMBER}"), "to": ".",
                 "condition": "has(hop.route) && hop.route == 'error'"},
            ],
        }
    }]})
}

/// One generation, wired the way `templates/assistant/README.md` §
/// *Instantiating* prescribes, with the one brain under test pointed at the
/// mock and stripped of every static bearer.
fn assistant_manifest(base_url: &str) -> Value {
    let guarded = |routes: &str| {
        json!({"from": "./assistants", "to": format!("./assistants/{AGENT}"),
               "condition": format!(
                   "has(hop.route) && ({routes}) && has(context.assistant) && context.assistant == '{AGENT}'")})
    };
    // GH #562 — the memory road is a v-lane too, and it is drawn here for the
    // plainest reason there is: a turn raises an ambient `recall` inside this
    // generation, and without an exit for it the leg dead-letters at the
    // surface's own path and the turn never reaches the credential round this
    // file is about. Same four edges the shipped recipe renders.
    let v_lane_recall = |asker: &str, down: bool| {
        let deep = format!("./assistants/{AGENT}/{asker}");
        if down {
            let mut cond = format!(
                "has(hop.route) && hop.route == 'in_bundle' && has(context.assistant) && \
                 context.assistant == '{AGENT}'"
            );
            let mut e = json!({"from": "./assistants", "to": deep, "lane": "in_bundle"});
            if asker == "cogny" {
                cond.push_str(" && has(hop.recall_caller) && hop.recall_caller == 'cogny'");
            } else {
                e["default"] = json!(true);
            }
            e["condition"] = json!(cond);
            e
        } else {
            json!({"from": deep, "to": "./assistants", "lane": "recall",
                   "condition": "has(hop.route) && hop.route == 'recall'",
                   "modifier": {"set_context": {"recall_caller": format!("'{asker}'")}}})
        }
    };
    let mut add_edges = vec![
        guarded("hop.route == 'in_turn'"),
        v_lane_recall("cogny", true),
        v_lane_recall("talky", true),
        v_lane_recall("talky", false),
        v_lane_recall("cogny", false),
        guarded("hop.route == 'in_build_result'"),
        guarded("hop.route == 'in_export' || hop.route == 'in_import'"),
    ];
    for lane in [
        "answer",
        "write",
        "turn_write",
        "extraction",
        "prune",
        "error",
        "build",
        "dump",
    ] {
        add_edges.push(
            json!({"from": format!("./assistants/{AGENT}"), "to": "./assistants",
                   "condition": format!("has(hop.route) && hop.route == '{lane}'")}),
        );
    }
    json!({"manifest": [{
        "scope": format!("/members/{MEMBER}"),
        "ctx": {"model": "double/no-network", "model_fast": "double/no-network",
                "model_surface": "gpt-4o-mini"},
        "diff": {
            "add_nodes": [{"name": format!("assistants/{AGENT}"),
                           "template": "assistant@2.5.0",
                           "override_params": {
                               // The brain under test: no bearer of its own
                               // (an empty string is not a bearer, GH #271), a
                               // grant instead, and a provider on this host.
                               "talky/brain": {"api_key": "",
                                               "base_url": base_url,
                                               "credential_grant_id": GRANT_TALKY,
                                               "external_timeout_ms": 5000,
                                               "credential_wait_ms": 30000},
                               "cogny/brain": {"api_key": "",
                                               "credential_grant_id": GRANT_COGNY}}}],
            "add_edges": add_edges,
        }
    }]})
}

/// One credential v-lane pair for one surface of one generation, plus the grant
/// the pair spends. This is the form `templates/member/README.md` §
/// *The credential v-lanes* documents, written out here so a drift in the
/// README is a drift against a running proof.
fn v_lane_edges(surface: &str, grant: &str) -> Vec<Value> {
    let brain = format!("./assistants/{AGENT}/{surface}/brain");
    vec![
        json!({
            "from": brain,
            "to": "./access",
            "lane": "credential_request",
            "condition": "has(hop.route) && hop.route == 'credential_request'",
            "modifier": {
                "set_hop": {"route": "'in_invoke'"},
                "set_context": {"requester": format!("'agent:{AGENT}/{surface}'")}
            }
        }),
        json!({
            "from": "./access",
            "to": brain,
            "lane": "in_sealed",
            "condition": format!(
                "has(hop.route) && hop.route == 'ack' && has(hop.operation) && \
                 hop.operation == 'vault.deliver' && has(hop.grant_id) && \
                 hop.grant_id == '{grant}'"),
            "modifier": {"set_hop": {"route": "'in_sealed'"}}
        }),
    ]
}

fn grant_row(grant: &str, surface: &str) -> Value {
    json!({
        "grant_id": grant,
        "requester": format!("agent:{AGENT}/{surface}"),
        "capability": "credential.read",
        "subject": format!("member:{MEMBER}"),
        "scope": {"actions": ["vault.deliver"]},
        "cred_ref": CRED_REF,
        "purpose": format!("authenticate {AGENT}'s {surface} brain against the provider"),
        "issued_at": "2026-01-01T00:00:00.000000Z",
        "expires_at": "2099-01-01T00:00:00.000000Z",
        "rule_id": format!("{MEMBER}-credential-read"),
        "constraints": {"rate_per_min": 60}
    })
}

fn granted_event(grant: &str, id: &str) -> Value {
    json!({
        "id": id,
        "grant_id": grant,
        "event": "granted",
        "at": "2026-01-01T00:00:00.000000Z",
        "actor": "operator",
        "reason_code": "",
        "detail": {"why": "seeded with the colony: credential_grant_id is immutable, so the \
                           grant has to exist before the brain ever asks"}
    })
}

/// The lanes and the rows, in one declaration at the member's own scope — which
/// is where they belong: the lowest common ancestor of a brain and the broker
/// is the member, and an edge lives in the graph of that level.
fn v_lane_manifest() -> Value {
    let mut add_edges = v_lane_edges("talky", GRANT_TALKY);
    add_edges.extend(v_lane_edges("cogny", GRANT_COGNY));
    json!({"manifest": [{
        "scope": format!("/members/{MEMBER}"),
        "diff": {
            "add_edges": add_edges,
            "seed_rows": [
                {"target": "./access/store", "table": "grants",
                 "rows": [grant_row(GRANT_TALKY, "talky"), grant_row(GRANT_COGNY, "cogny")]},
                {"target": "./access/store", "table": "grant_events",
                 "rows": [granted_event(GRANT_TALKY, "ev-gh560-0000000001"),
                          granted_event(GRANT_COGNY, "ev-gh560-0000000002")]},
            ],
        }
    }]})
}

// ═════════════════════════════════════════════════════════════ reading back

fn member_dir(root: &std::path::Path) -> std::path::PathBuf {
    root.join(format!("main/members/{MEMBER}"))
}

/// Fill the member's vault the way `meclaw --vault-add` does: straight into its
/// own `cell.db`, with no colony running. A message would put the value into the
/// very log this lane exists to keep it out of.
fn seed_vault_secret(root: &std::path::Path) {
    use meclaw_cells::vault::crypto::MasterKey;
    use meclaw_cells::vault::store as vs;
    let dir = member_dir(root).join("access/vault");
    let conn = meclaw_colony::persist::open_or_create_cell_db(&dir.join("cell.db")).unwrap();
    vs::apply_ddl(&conn).unwrap();
    let salt = vs::salt_or_create(&conn).unwrap();
    let key = MasterKey::derive(PASSPHRASE.as_bytes(), &salt).unwrap();
    let (nonce, ct) = key.seal(SECRET.as_bytes()).unwrap();
    vs::put(&conn, CRED_REF, &nonce, &ct, &vs::now_iso()).unwrap();
}

/// Every `message_log` row of the colony, headers and body as one string each.
fn log_rows(root: &std::path::Path) -> Vec<String> {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare("SELECT headers, COALESCE(body_payload, '') FROM message_log")
        .expect("message_log");
    st.query_map([], |r| {
        Ok(format!(
            "{} {}",
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?
        ))
    })
    .expect("query")
    .filter_map(Result::ok)
    .collect()
}

/// The `edges` table of the colony: `(from, to, lane)`.
fn edge_lanes(root: &std::path::Path) -> Vec<(String, String, String)> {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare("SELECT from_path, to_path, COALESCE(lane, '') FROM edges")
        .expect("edges");
    st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })
    .expect("query")
    .filter_map(Result::ok)
    .collect()
}

fn store_rows(root: &std::path::Path, sql: &str) -> Vec<String> {
    let db = member_dir(root).join("access/store/cell.db");
    if !db.exists() {
        return Vec::new();
    }
    let Ok(conn) = rusqlite::Connection::open(&db) else {
        return Vec::new();
    };
    let Ok(mut st) = conn.prepare(sql) else {
        return Vec::new();
    };
    let Ok(rows) = st.query_map([], |r| r.get::<_, String>(0)) else {
        return Vec::new();
    };
    rows.filter_map(Result::ok).collect()
}

fn chat_answer() -> MockResponse {
    MockResponse::ok_json(
        json!({
            "id": "chatcmpl-1", "object": "chat.completion", "created": 1,
            "model": "gpt-4o-mini",
            "choices": [{"index": 0, "finish_reason": "stop",
                         "message": {"role": "assistant", "content": "pong"}}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        })
        .to_string()
        .as_bytes(),
    )
}

/// Poll until `f` answers `true`, or give up after ~60 s. Failure markers are
/// generous on purpose (CLAUDE.md § Coding-Standards): this waits on a round
/// through eight cells under whatever load the rest of the suite puts on the
/// host.
async fn until(what: &str, mut f: impl FnMut() -> bool) {
    for _ in 0..240 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("{what} never happened");
}

/// `set_var` is unsafe in edition 2024 (a concurrent `getenv` would be a data
/// race); sound here because every test in this file writes the same value and
/// the cell that reads it does not exist yet.
fn arm_passphrase() {
    unsafe { std::env::set_var(UNLOCK_ENV, PASSPHRASE) };
}

fn turn(text: &str) -> Message {
    let mut hop = Map::new();
    hop.insert("route".to_string(), json!("in_turn"));
    let mut ctx = Map::new();
    ctx.insert("channel".to_string(), json!("gh560"));
    ctx.insert(
        "audience_set".to_string(),
        json!(format!(r#"["member:{MEMBER}","agent:{AGENT}"]"#)),
    );
    ctx.insert("assistant".to_string(), json!(AGENT));
    MessageBuilder::new(Path::new(&format!("/members/{MEMBER}/assistants/{AGENT}")))
        .hop(hop)
        .context(ctx)
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .ttl(400)
        .build()
}

// ═════════════════════════════════════════════════════════════════════ pins

/// The claim the issue is titled after.
///
/// A member with an `access` of its own, a generation four levels in, and two
/// edges between them that name their lane. One turn arrives, the brain holds
/// nothing, asks over the v-lane, gets its box back over the v-lane, and answers
/// the very same turn — with the bearer that was sealed in the person's vault.
/// The value is on no wire this colony logged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_the_members_brain_gets_its_sealed_key() {
    if !shipped() {
        return;
    }
    arm_passphrase();
    let (addr, _server, captured) =
        start_mock_server_capturing(vec![chat_answer(), chat_answer(), chat_answer()]).await;
    let base_url = format!("http://{addr}/v1");
    let real_brain = format!("/members/{MEMBER}/assistants/{AGENT}/talky/brain");

    let td = tempfile::TempDir::new().unwrap();
    let h = boot(&td, &real_brain).await;

    let outcome = apply(&h, member_manifest()).await;
    assert!(
        outcome.is_committed(),
        "growing the member with its own access must commit; got {outcome:?}"
    );
    let outcome = apply(&h, assistant_manifest(&base_url)).await;
    assert!(
        outcome.is_committed(),
        "growing the generation must commit; got {outcome:?}"
    );
    // THE mutation: four deep edges that skip three levels and land on a rim
    // inside a sealed hive. It commits only because `talky@4.6.1` and
    // `cogny@4.6.1` name `./brain` as the connect point of both lanes — Stage 6
    // refuses it by name otherwise, which is what test (b) measures.
    let outcome = apply(&h, v_lane_manifest()).await;
    assert!(
        outcome.is_committed(),
        "the credential v-lanes must commit; got {outcome:?}"
    );

    // The CREDENTIAL v-lanes, told from the others by the lane they name: since
    // GH #562 a generation is also wired to the memory on four v-lanes of its
    // own, and this file is not about those.
    let drawn: Vec<(String, String, String)> = edge_lanes(td.path())
        .into_iter()
        .filter(|(_, _, lane)| lane == "credential_request" || lane == "in_sealed")
        .collect();
    assert_eq!(
        drawn.len(),
        4,
        "four credential v-lanes were declared and the edge table carries {}: {drawn:?}",
        drawn.len()
    );
    assert!(
        drawn.iter().any(|(from, to, lane)| from
            == &format!("/members/{MEMBER}/assistants/{AGENT}/talky/brain")
            && to == &format!("/members/{MEMBER}/access")
            && lane == "credential_request"),
        "the ask half of talky's lane is not in the edge table: {drawn:?}"
    );

    // The vault directory exists only after the member grew, so the credential
    // goes in here — the same write `meclaw --vault-add` performs.
    h.shutdown().await;
    seed_vault_secret(td.path());

    // Second boot: the vault holds a secret, the store holds two grants, and
    // nothing else has changed. That is the state an operator's first start is
    // in, and everything below happens without another gesture.
    let h = boot(&td, &real_brain).await;
    h.send(turn("ping")).await;

    until("the vault.deliver spend", || {
        store_rows(
            td.path(),
            "SELECT outcome FROM usage WHERE operation = 'vault.deliver'",
        )
        .contains(&"ok".to_string())
    })
    .await;

    until("the provider call for the very first turn", || {
        captured.try_lock().map(|c| !c.is_empty()).unwrap_or(false)
    })
    .await;

    let seen = captured.lock().await.clone();
    let last = seen.last().expect("the provider was called");
    assert_eq!(
        last.headers.get("authorization").map(String::as_str),
        Some(format!("Bearer {SECRET}").as_str()),
        "the bearer on the wire is not the member vault's value: {:?}",
        last.headers
    );

    h.shutdown().await;

    // The ciphertext is on record — that IS the delivery, journalled like every
    // other message — and the plaintext is not. Both halves, because either one
    // alone proves nothing: a log with no box means the lane never ran, and a
    // log with the value means the lane ran and leaked.
    let log = log_rows(td.path());
    let boxes: Vec<&String> = log
        .iter()
        .filter(|r| {
            r.contains("\"epk\"") && r.contains("\"nonce\"") && r.contains("\"ciphertext\"")
        })
        .collect();
    assert!(
        !boxes.is_empty(),
        "no sealed box was journalled at all — the credential lane did not run, \
         so the absence of the plaintext below would prove nothing"
    );
    let hits: Vec<String> = log
        .into_iter()
        .filter(|r| r.contains(SECRET) || r.contains(PASSPHRASE))
        .collect();
    assert!(hits.is_empty(), "the credential is on record: {hits:?}");
}

/// The counter-proof, and the reason the `at` list is a promise rather than a
/// comment: the same lane onto a cell `talky` names no connect point for is
/// refused by name, and the refusal says which string to add.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn b_a_lane_without_a_connect_point_is_refused_by_name() {
    if !shipped() {
        return;
    }
    arm_passphrase();
    let td = tempfile::TempDir::new().unwrap();
    let real_brain = format!("/members/{MEMBER}/assistants/{AGENT}/talky/brain");
    let h = boot(&td, &real_brain).await;
    assert!(apply(&h, member_manifest()).await.is_committed());
    assert!(
        apply(&h, assistant_manifest("http://127.0.0.1:1/v1"))
            .await
            .is_committed()
    );

    let outcome = apply(
        &h,
        json!({"manifest": [{
            "scope": format!("/members/{MEMBER}"),
            "diff": {"add_edges": [{
                "from": "./access",
                // A cell of `talky` that is not the connect point. It is an
                // interior node of a sealed hive either way; what makes the
                // refusal SAY SOMETHING is the lane.
                "to": format!("./assistants/{AGENT}/talky/collector"),
                "lane": "in_sealed",
                "condition": "has(hop.route) && hop.route == 'ack'"
            }]}
        }]}),
    )
    .await;

    let rendered = format!("{outcome:?}");
    assert!(
        !outcome.is_committed(),
        "a v-lane onto a cell no `at` names must be refused: {rendered}"
    );
    assert!(
        rendered.contains("v_lane_no_connect_point"),
        "the refusal must name the code, so the reader knows it is the lane and \
         not the seal that stopped this: {rendered}"
    );
    assert!(
        rendered.contains("./brain"),
        "the refusal must name the connect point `talky` DOES declare, so the \
         fix is readable out of the message: {rendered}"
    );
    h.shutdown().await;
}

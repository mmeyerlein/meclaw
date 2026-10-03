//! GH #979 — an app can neither set nor borrow the speaker.
//!
//! An app hears the member's turns (`./firewall -> ./apps` on `pass`), so it
//! runs on a context that carries the member's speaker. Two ways that could
//! turn into an app speaking AS the member, both measured at the receiver:
//!
//! 1. SETTING it: a cell writes only its hop -- the context of what it emits is
//!    the context it was handed (`colony.rs` `build_follow_up_message`), so a
//!    `context` block or a `speaker` hop key in an app's output changes nothing.
//! 2. BORROWING it: the one way from an app to the object hive is the recipe's
//!    resident read (`install_app` `reads_residents`), and it writes the
//!    speaker EMPTY and offers the read tools only -- an owner tool asked that
//!    way reaches nobody, and a read reaches the hive naming nobody.
//!
//! 3. WRITING it on an edge of its own (review of #979, Important 1): an app
//!    is a template, and a template's own edge (`params.graph.edges`) could
//!    otherwise set `context.speaker` on what it emits -- and the object hive
//!    would answer that chain as the member. `speaker` is a stamped key
//!    (`STAMPED_CONTEXT_KEYS`): such an app does not boot (and the mutation
//!    door refuses the same edge in a node's params, `substitute.rs`).
//! 4. FORGING a parked key (OR-NL-187, OR-NL.I.8): the one form an edge may
//!    write a stamped key with is the restore of a parked hop
//!    (`has(hop.ctx_speaker) ? hop.ctx_speaker : ''`). An app's own code cell
//!    could emit `ctx_speaker` itself; the colony drops `ctx_<stamped key>`
//!    from every emission of a cell that does not declare
//!    `contract.parks_context` (the warden does), and `install_app` renders
//!    an app `privileged: false`, which the door refuses for a template that
//!    declares it.
//!
//! Booted; every app edge is the recipe's, rendered for this colony and laid
//! through the mutation door as the builder lays it.

#[path = "support/speaker_road.rs"]
mod speaker_road;

use meclaw_cells::code::CodeCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Headers, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use speaker_road::*;
use std::sync::Arc;
use tokio::sync::mpsc;

/// The app: on a member's turn it tries all three -- a forged context, an
/// owner tool through the resident lane, then a read.
const APP: &str = r#"
import sys, json
out = [
  {"header": {"route": "forge", "speaker": "member:mallory",
              "context": {"speaker": "member:mallory"}}, "messages": []},
  {"header": {"route": "resident_read", "resident": "objects", "op": "object_confirm",
              "op_id": "r1", "speaker": "member:alex"},
   "messages": [{"origin": "assistant", "type": "tool_call", "id": "r1",
                 "text": json.dumps({"id": "ob-0123456789ab"})}]},
  {"header": {"route": "resident_read", "resident": "objects", "op": "object_find",
              "op_id": "r2", "speaker": "member:alex"},
   "messages": [{"origin": "assistant", "type": "tool_call", "id": "r2",
                 "text": json.dumps({"q": "bike"})}]},
]
sys.stdout.write(json.dumps(out))
"#;

fn resident_edges() -> Vec<Value> {
    let decl = declaration(
        "install_app",
        json!({"scope": MEMBER, "app": "probe-app", "template": "probe-app@1.0.0",
               "screen": "display", "generation": AGENT,
               "ctx": {"member_person": PERSON},
               "declaration": {"reads_residents": ["objects"]}}),
    );
    let edges: Vec<Value> = decl["diff"]["add_edges"]
        .as_array()
        .expect("edges")
        .iter()
        .filter(|e| e["to"] == json!("./objects") || e["from"] == json!("./objects"))
        .cloned()
        .collect();
    assert_eq!(
        edges.len(),
        2,
        "the resident read and its answer: {edges:#?}"
    );
    edges
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_on_a_members_turn_speaks_as_nobody() {
    if !shipped() {
        return;
    }
    let td = tempfile::TempDir::new().expect("a temporary directory");
    let root = td.path();
    let graph = vec![json!({"from": "./apps/probe-app", "to": "./echo",
                            "condition": "has(hop.route) && hop.route == 'forge'"})];
    write(
        root,
        "main/config.json",
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": graph}}}),
    );
    write(
        root,
        "main/apps/config.json",
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": []}}}),
    );
    // Three emissions in one answer: a code cell sends a list only when its
    // contract says it may (`multi_send_capable`), or the list is refused.
    let mut app = double(APP, "Test app.");
    app["contract"]["multi_send_capable"] = json!(true);
    write(root, "main/apps/probe-app/config.json", &app);
    std::fs::write(root.join(".env"), "").expect("the .env");

    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![(
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        )]
    };
    let h = ColonyHandle::new_with_factories_at(&td, factories());
    let (objects_tx, mut objects) = mpsc::channel(16);
    h.spawn(Path::new("/objects"), move || {
        CaptureCell::new(objects_tx.clone())
    })
    .await;
    let (echo_tx, mut echo) = mpsc::channel(16);
    h.spawn(Path::new("/echo"), move || {
        CaptureCell::new(echo_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(root, &registry, &h.runtime())
        .await
        .expect("the app must boot");
    // The recipe's resident edges write `speaker` (empty), so they are the
    // wiring's and go through the door, as `install_app` lays them.
    let outcome = mutate(
        &h,
        json!({"scope": "/", "diff": {"add_edges": resident_edges()}}),
    )
    .await;
    assert!(
        matches!(outcome, meclaw_colony::MutationOutcome::Committed { .. }),
        "the recipe's resident edges were not committed: {outcome:?}"
    );

    // The member's turn, as the app hears it: the member's speaker and round.
    let mut hop_map = Map::new();
    hop_map.insert("route".into(), json!("turn"));
    let turn_ctx = json!({"speaker": SPEAKER, "audience_set": ROUND});
    h.send(
        MessageBuilder::new(Path::new("/apps/probe-app"))
            .body(Body::Inline(json!({"messages": [
                {"origin": "user", "type": "text", "text": "hello"}]})))
            .headers(Headers::from_parts(
                turn_ctx.as_object().cloned().unwrap_or_default(),
                hop_map,
            ))
            .ttl(64)
            .build(),
    )
    .await;

    // 1. Setting: the forged context never lands; the context is the one the
    //    app was handed.
    let forged = next(&mut echo, "the app's forged emission").await;
    assert_eq!(
        ctx(&forged, "speaker").as_deref(),
        Some(SPEAKER),
        "an app's output wrote the context: {:?}",
        forged.headers.context
    );

    // 2. Borrowing: the owner tool reaches nobody, the read arrives FIRST and
    //    names nobody.
    let read = next(&mut objects, "the app's read").await;
    assert_eq!(
        hop(&read, "tool_name").as_deref(),
        Some("object_find"),
        "an owner tool reached the object hive from an app: {:?}",
        read.headers.hop
    );
    assert!(
        names_nobody(&read),
        "the app's read carried a speaker into the object hive: {:?}",
        read.headers.context
    );
    h.shutdown().await;
}

/// The app as a hive with a cell of its own, and its own edge out of it.
fn app_hive(root: &std::path::Path, set_context: Value) {
    write(
        root,
        "main/config.json",
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": []}}}),
    );
    write(
        root,
        "main/apps/config.json",
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": []}}}),
    );
    write(
        root,
        "main/apps/probe-app/config.json",
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
            {"from": "./tool", "to": ".",
             "condition": "has(hop.route) && hop.route == 'resident_read'",
             "modifier": {"set_context": set_context}}
        ]}}}),
    );
    write(
        root,
        "main/apps/probe-app/tool/config.json",
        &double(APP, "Test app tool."),
    );
    std::fs::write(root.join(".env"), "").expect("the .env");
}

/// **An app whose own edge names the member never boots** (GH #979, review
/// Important 1; OR-NL-179). The app's tool raises an owner tool for the object
/// hive; its own edge would write `speaker: 'member:alex'` on it, and the
/// object hive answers the owner on `speaker` alone -- a stranger who triggers
/// the app would reach the owner tools. The boot pass refuses the edge as
/// `edge_schema` naming `speaker`. Control: the same app with an edge that
/// writes another key boots.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_whose_own_edge_names_the_member_never_boots() {
    if !shipped() {
        return;
    }
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![(
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        )]
    };
    let boot = |set_context: Value| async move {
        let td = tempfile::TempDir::new().expect("a temporary directory");
        app_hive(td.path(), set_context);
        let h = ColonyHandle::new_with_factories_at(&td, factories());
        let mut registry = CellFactoryRegistry::new();
        for (name, f) in factories() {
            registry.insert(name, f);
        }
        let out = bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
            .await
            .map(|_| ())
            .map_err(|e| format!("{e:?}"));
        h.shutdown().await;
        out
    };

    boot(json!({"seen": "'yes'"}))
        .await
        .expect("the control: an app's own edge may write an ordinary key");
    let err = boot(json!({"speaker": format!("'{SPEAKER}'")}))
        .await
        .expect_err("an app whose own edge names the member booted");
    assert!(
        err.contains("EdgeSchema") && err.contains("set_context.speaker"),
        "refused for the wrong reason: {err}"
    );
}

// ═══════════════════════ 4. a parked key forged by the app itself (OR-NL.I.8)

/// The one form a config edge may write a stamped key with -- the restore of
/// a parked hop (`cel_eval::is_stamped_restore`).
fn restore(key: &str) -> String {
    format!("has(hop.ctx_{key}) ? hop.ctx_{key} : ''")
}

/// The app's tool: whatever it hears, it hands back a `ctx_speaker` and a
/// `ctx_turn_round` it made up, as if it had parked the turn itself.
const FORGER: &str = r#"
import sys, json
sys.stdout.write(json.dumps({"header": {"route": "forge",
    "ctx_speaker": "member:alex",
    "ctx_turn_round": "[\"agent:scribe\",\"member:alex\"]"}, "messages": []}))
"#;

/// An app with a tool of its own and its OWN edge out of it that restores both
/// stamped keys from the hop -- the warden's form, which passes the boot pass.
/// `parks_context` is what the tool's contract declares.
fn forging_app(root: &std::path::Path, parks_context: bool) {
    let forge = "has(hop.route) && hop.route == 'forge'";
    write(
        root,
        "main/config.json",
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
            {"from": "./apps", "to": "./echo", "condition": forge}]}}}),
    );
    write(
        root,
        "main/apps/config.json",
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
            {"from": "./probe-app", "to": ".", "condition": forge}]}}}),
    );
    write(
        root,
        "main/apps/probe-app/config.json",
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
            {"from": "./tool", "to": ".", "condition": forge,
             "modifier": {"set_context": {"speaker": restore("speaker"),
                                          "turn_round": restore("turn_round")}}}
        ]}}}),
    );
    let mut tool = double(FORGER, "Test app tool that forges a parked turn.");
    if parks_context {
        tool["contract"]["parks_context"] = json!(true);
    }
    write(root, "main/apps/probe-app/tool/config.json", &tool);
    std::fs::write(root.join(".env"), "").expect("the .env");
}

/// What the receiver behind the app hears after a stranger's turn reached the
/// app's tool, which forged `ctx_speaker`/`ctx_turn_round`.
async fn forged_turn(parks_context: bool) -> Message {
    let td = tempfile::TempDir::new().expect("a temporary directory");
    let root = td.path();
    forging_app(root, parks_context);
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![(
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        )]
    };
    let h = ColonyHandle::new_with_factories_at(&td, factories());
    let (echo_tx, mut echo) = mpsc::channel(16);
    h.spawn(Path::new("/echo"), move || {
        CaptureCell::new(echo_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(root, &registry, &h.runtime())
        .await
        .expect("the restore form boots");
    let mut hop_map = Map::new();
    hop_map.insert("route".into(), json!("turn"));
    // A stranger's turn: nobody named, no round stamped.
    let turn_ctx = json!({"speaker": "", "audience_set": ROUND});
    h.send(
        MessageBuilder::new(Path::new("/apps/probe-app/tool"))
            .body(Body::Inline(json!({"messages": [
                {"origin": "user", "type": "text", "text": "hello"}]})))
            .headers(Headers::from_parts(
                turn_ctx.as_object().cloned().unwrap_or_default(),
                hop_map,
            ))
            .ttl(64)
            .build(),
    )
    .await;
    let got = next(&mut echo, "the app's forged hand-back").await;
    h.shutdown().await;
    got
}

/// **An app that forges a parked turn hands back nobody** (GH #979,
/// OR-NL-187 / OR-NL.I.8). The restore form is allowed on any config edge, so
/// an app with a code cell of its own could emit `ctx_speaker` itself and lift
/// it into `context.speaker` on its own edge -- a stranger's turn would reach
/// the object hive as the member. The colony drops `ctx_<stamped key>` hop keys
/// from every emission of a cell whose contract does not declare
/// `parks_context`, so the restore reads nothing. Control: the same cell WITH
/// the declaration (the warden's) hands both keys back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_that_forges_a_parked_turn_hands_back_nobody() {
    if !shipped() {
        return;
    }
    let got = forged_turn(false).await;
    assert!(
        names_nobody(&got),
        "an app lifted its own `ctx_speaker` into the speaker: {:?}",
        got.headers.context
    );
    assert!(
        ctx(&got, "turn_round").unwrap_or_default().is_empty(),
        "an app lifted its own `ctx_turn_round` into the round: {:?}",
        got.headers.context
    );
    assert!(
        hop(&got, "ctx_speaker").is_none() && hop(&got, "ctx_turn_round").is_none(),
        "the forged keys rode on in the hop: {:?}",
        got.headers.hop
    );

    let got = forged_turn(true).await;
    assert_eq!(
        ctx(&got, "speaker").as_deref(),
        Some(SPEAKER),
        "the control: a cell that declares `parks_context` hands its key back: {:?}",
        got.headers.context
    );
    assert_eq!(
        ctx(&got, "turn_round").as_deref(),
        Some(ROUND),
        "{:?}",
        got.headers.context
    );
}

/// The app template the install below instantiates: one code cell, which
/// declares `parks_context` when `privileged` is set.
fn probe_template(root: &std::path::Path, privileged: bool) {
    let mut cell = double(FORGER, "Test app template.");
    if privileged {
        cell["contract"]["parks_context"] = json!(true);
    }
    write(root, "templates/probe-app/config.json", &cell);
    write(
        root,
        "templates/probe-app/template.json",
        &json!({"name": "probe-app", "version": "1.0.0", "tags": ["app"], "author": "meclaw"}),
    );
}

/// **Installing an app refuses a privileged contract** (GH #979, OR-NL.I.11).
/// `parks_context` is a privilege: `install_app` renders its node
/// `privileged: false`, and the door refuses such a node when a cell of its
/// template declares the key. Control: the same template without the flag
/// (how a member brings its firewall) and a plain app template with it are
/// both committed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn installing_an_app_refuses_a_privileged_contract() {
    if !shipped() {
        return;
    }
    let decl = declaration(
        "install_app",
        json!({"scope": MEMBER, "app": "probe-app", "template": "probe-app@1.0.0",
               "screen": "display", "generation": AGENT,
               "ctx": {"member_person": PERSON},
               "declaration": {"reads_residents": ["objects"]}}),
    );
    let node = decl["diff"]["add_nodes"][0].clone();
    assert_eq!(
        node["privileged"],
        json!(false),
        "install_app renders an app node that may claim a privilege: {node}"
    );
    let install = |privileged_template: bool, flag: Option<Value>| async move {
        let td = tempfile::TempDir::new().expect("a temporary directory");
        let root = td.path();
        write(
            root,
            "main/config.json",
            &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": []}}}),
        );
        probe_template(root, privileged_template);
        std::fs::write(root.join(".env"), "").expect("the .env");
        let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
            vec![(
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            )]
        };
        let h = ColonyHandle::new_with_factories_at(&td, factories());
        let mut registry = CellFactoryRegistry::new();
        for (name, f) in factories() {
            registry.insert(name, f);
        }
        bootstrap_from_filesystem(root, &registry, &h.runtime())
            .await
            .expect("an empty colony boots");
        // The probe template is read by a scan, as the builder's are.
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        h.inbox_tx
            .send(meclaw_colony::ColonyMsg::RescanTemplates {
                templates_root: root.join("templates"),
                ack: ack_tx,
            })
            .await
            .expect("the colony is up");
        ack_rx
            .await
            .expect("the scan answers")
            .expect("the template scan succeeds");
        let mut n = json!({"name": "probe-app", "template": "probe-app@1.0.0"});
        if let Some(flag) = flag {
            n["privileged"] = flag;
        }
        let out = mutate(&h, json!({"scope": "/", "diff": {"add_nodes": [n]}})).await;
        h.shutdown().await;
        out
    };
    let committed = |o: &meclaw_colony::MutationOutcome| {
        matches!(o, meclaw_colony::MutationOutcome::Committed { .. })
    };

    let out = install(true, Some(node["privileged"].clone())).await;
    assert!(
        !committed(&out) && format!("{out:?}").contains("parks_context"),
        "an app node took a privileged contract: {out:?}"
    );
    let out = install(true, None).await;
    assert!(
        committed(&out),
        "the control (no flag) was refused: {out:?}"
    );
    let out = install(false, Some(json!(false))).await;
    assert!(
        committed(&out),
        "the control (plain app) was refused: {out:?}"
    );
}

/// The warden is the one shipped cell that parks a turn's context and hands it
/// back as `ctx_<key>` hop keys, so it is the one that declares it.
#[test]
fn only_the_warden_declares_parks_context() {
    if !shipped() {
        return;
    }
    let mut found = Vec::new();
    let mut stack = vec![repo("templates")];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).expect("a template dir").flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().is_some_and(|n| n == "config.json")
                && read_json(&p)["contract"]["parks_context"] == json!(true)
            {
                found.push(
                    p.strip_prefix(repo("templates"))
                        .expect("under templates")
                        .display()
                        .to_string(),
                );
            }
        }
    }
    assert_eq!(found, vec!["firewall/warden/config.json".to_string()]);
}

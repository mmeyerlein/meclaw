//! GH #940 — a foreign chat never speaks in the member's round.
//!
//! The ingress edge of a grown channel stamps `context.audience_set`, the round
//! every turn on that channel is spoken in, and a connector raises a turn for
//! EVERY chat that writes to it. A bot is reachable by anybody who finds its
//! handle, so an unbound channel let a stranger's chat speak in the round of
//! the member — with the member's memory behind the audience gate, which reads
//! the round and nothing else.
//!
//! So a channel is BOUND. `grow_level` takes `bind_chat`, the one chat id whose
//! turns carry the round, and renders it into the ingress condition:
//!
//! ```text
//! !has(hop.error_code) && has(hop.chat_id)
//!   && (string(hop.chat_id) == '<bind>' || string(hop.chat_id).startsWith('<bind>:'))
//! ```
//!
//! `string()` because Telegram stamps a NUMBER and Slack a STRING; the prefix
//! form because a Slack id is composite (`<channel>[:<thread_ts>]`) and a
//! thread lives inside its channel. A turn from any other chat finds no edge:
//! no turn, no round, a dead letter, and nothing is said to the stranger.
//!
//! Two halves, measured where they are decided. The renderer half runs the
//! SHIPPED recipe and evaluates what it rendered with the REAL evaluator the
//! substrate uses. The colony half boots a colony, applies the rendered edges
//! through the mutation door with the binding read from the colony's `.env`,
//! and reads the result at the receivers: the recorder that stands for the
//! member's brain, and the dead-letter list.
//!
//! No model and no network: `code` doubles and one capture cell.

use meclaw_cells::code::CodeCellFactory;
use meclaw_colony::cel_eval::{evaluate_condition, parse_condition};
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, DeadLetter, MutationOutcome,
    bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Headers, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::wait::wait_for_message_log_count;
use meclaw_testing::{ColonyHandle, emit_one, shipped_script};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

const RECIPES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/recipes/config.json"
);

const MEMBER: &str = "/os/orgs/acme/members/alex";
const AGENT: &str = "scribe";
const PERSON: &str = "alex";
/// The round the ingress edge stamps, as the value a holder reads.
const ROUND: &str = r#"["agent:scribe","member:alex"]"#;

/// The channel templates whose ingress is bound before the turn is raised
/// (`SELF_BOUND_CHANNELS` in `recipes`), at the versions the tree ships.
const SELF_BOUND: [&str; 4] = [
    "chat-channel@1.0.1",
    "voice@2.6.1",
    "web@2.3.0",
    "terminal@1.0.2",
];
const TELEGRAM: &str = "telegram-connector@2.2.0";
const SLACK: &str = "slack-agent@2.1.7";

// ══════════════════════════════════════════════════════════════ the renderer

fn run_recipes(params: Value) -> Value {
    emit_one(
        &shipped_script(RECIPES),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!({"recipe": "grow_level", "request": "…",
                                         "params": params}).to_string()}],
        }),
    )
}

/// A channel wish named after its template, with the person named and the
/// binding as given.
fn wish(template: &str, bind: Option<&str>) -> Value {
    let name = template.split('@').next().expect("a template name");
    let mut params = json!({
        "scope": MEMBER, "level": "channel", "name": name,
        "template": template, "assistant": AGENT,
        "ctx": {"member_person": PERSON}});
    if let Some(b) = bind {
        params["bind_chat"] = json!(b);
    }
    params
}

fn declaration(params: Value) -> Value {
    let out = run_recipes(params);
    assert!(
        out["header"]["error_code"].is_null(),
        "the recipe refused a wish it is supposed to render: {out}"
    );
    out["manifest"]
        .as_array()
        .unwrap_or_else(|| panic!("no manifest: {out}"))[0]
        .clone()
}

/// The edge that raises a turn: `./<node> -> .`, on a condition that starts
/// with `!has(hop.error_code)`.
fn ingress(decl: &Value) -> Value {
    let node = format!(
        "./{}",
        decl["diff"]["add_nodes"][0]["name"]
            .as_str()
            .expect("the node the declaration grows")
    );
    decl["diff"]["add_edges"]
        .as_array()
        .expect("add_edges")
        .iter()
        .find(|e| {
            e["from"] == json!(node)
                && e["to"] == json!(".")
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.starts_with("!has(hop.error_code)"))
        })
        .unwrap_or_else(|| panic!("no ingress edge out of {node}: {decl}"))
        .clone()
}

fn bound_ingress(template: &str, bind: &str) -> Value {
    ingress(&declaration(wish(template, Some(bind))))
}

/// Would this edge take a message with that hop? `Err` is the substrate's
/// skip, so it is `false` here for the same reason it is there.
fn fires(edge: &Value, hop: Value) -> bool {
    let cond = edge["condition"].as_str().expect("a condition");
    let compiled = parse_condition(cond).expect("the condition compiles");
    let headers = Headers::from_parts(Map::new(), hop.as_object().cloned().unwrap_or_default());
    evaluate_condition(&compiled, &headers.context, &headers.hop).unwrap_or(false)
}

fn payload(out: &Value) -> Value {
    meclaw_core::serde_json::from_str(out["messages"][0]["text"].as_str().expect("a payload"))
        .expect("json payload")
}

/// The refusal a channel wish without a usable binding gets: named, with the
/// one missing parameter, and with nothing rendered.
fn assert_unbound(params: Value, why: &str) {
    let out = run_recipes(params);
    assert_eq!(
        out["header"],
        json!({"operation": "recipe", "error_code": "channel_unbound",
               "recipe": "grow_level"}),
        "{why}: {out}"
    );
    assert!(
        out["manifest"].is_null(),
        "{why}: a refusal renders nothing, and an empty manifest would be a \
         failure wearing the face of an honest answer: {out}"
    );
    let said = payload(&out);
    assert_eq!(said["missing"], json!(["params.bind_chat"]), "{why}");
    assert!(
        said["reason"]
            .as_str()
            .is_some_and(|r| r.contains("bind_chat")),
        "{why}: the refusal does not name what it needs: {said}"
    );
}

/// Telegram stamps the chat id as a NUMBER, and a string of the same digits
/// names the same chat. Both reach the round; a group id with its sign does
/// too.
#[test]
fn the_bound_chat_fires_as_a_number_and_as_a_string() {
    let edge = bound_ingress(TELEGRAM, "111");
    assert!(
        fires(&edge, json!({"chat_id": 111})),
        "the bound chat as a JSON number found no ingress edge — Telegram \
         stamps a number, so the member's own chat would go silent"
    );
    assert!(
        fires(&edge, json!({"chat_id": "111"})),
        "the bound chat as a string found no ingress edge"
    );

    let group = bound_ingress(TELEGRAM, "-1001234");
    assert!(fires(&group, json!({"chat_id": -1_001_234})));
    assert!(fires(&group, json!({"chat_id": "-1001234"})));
}

/// Every other chat finds no edge — not as a number, not as a string — and
/// neither does a hop that names no chat or carries an error.
#[test]
fn a_foreign_chat_finds_no_ingress_edge() {
    let edge = bound_ingress(TELEGRAM, "111");
    for (hop, what) in [
        (json!({"chat_id": 222}), "a foreign chat as a number"),
        (json!({"chat_id": "222"}), "a foreign chat as a string"),
        (
            json!({"chat_id": 1111}),
            "a chat whose digits extend the binding",
        ),
        (json!({"user_id": "111"}), "a hop that names no chat"),
        (
            json!({"chat_id": 111, "error_code": "upstream_failed"}),
            "the bound chat carrying an error",
        ),
    ] {
        assert!(
            !fires(&edge, hop.clone()),
            "{what} ({hop}) took the ingress edge and would speak in the \
             member's round"
        );
    }
}

/// Slack's chat id is `<channel>[:<thread_ts>]`. A thread lives inside its
/// channel and is read by the same people, so a channel binding takes its
/// threads — and nothing that merely starts with the same letters.
#[test]
fn a_slack_binding_takes_its_threads_and_no_other_channel() {
    let c1 = bound_ingress(SLACK, "C1");
    assert!(fires(&c1, json!({"chat_id": "C1"})));
    assert!(
        fires(&c1, json!({"chat_id": "C1:1700.1"})),
        "a thread of the bound channel found no ingress edge"
    );
    assert!(
        !fires(&c1, json!({"chat_id": "C10"})),
        "a channel whose id merely starts with the binding took the edge"
    );

    let c2 = bound_ingress(SLACK, "C2");
    assert!(
        !fires(&c2, json!({"chat_id": "C1:1700.1"})),
        "a thread of a foreign channel took the ingress edge"
    );
}

/// A channel wish that does not say which chat it is bound to is ASKED, under
/// a code of its own, and nothing is rendered.
#[test]
fn a_channel_wish_without_a_binding_is_asked_and_renders_nothing() {
    assert_unbound(wish(TELEGRAM, None), "no binding at all");
    assert_unbound(wish(TELEGRAM, Some("   ")), "a blank binding");
    // A JSON number is the natural Telegram spelling; it is asked too, and
    // the answer says that a string is expected (review M-1).
    let mut numeric = wish(TELEGRAM, None);
    numeric["bind_chat"] = json!(111);
    assert_unbound(numeric.clone(), "a number");
    let reason = payload(&run_recipes(numeric))["reason"].clone();
    assert!(
        reason.as_str().is_some_and(|r| r.contains("string")),
        "{reason}"
    );
}

/// The person is asked FIRST: a wish missing both hears the round's question
/// before the chat's.
#[test]
fn the_person_is_asked_before_the_chat() {
    let mut params = wish(TELEGRAM, None);
    params
        .as_object_mut()
        .expect("params")
        .remove("ctx")
        .expect("the wish named a person");
    let out = run_recipes(params);
    assert_eq!(out["header"]["error_code"], json!("wish_incomplete"));
    assert_eq!(payload(&out)["missing"], json!(["ctx.member_person"]));
}

/// `${NAME}` is the colony's to bind, from its `.env`, when the mutation is
/// applied — so the renderer writes it into the condition as it is.
#[test]
fn an_environment_binding_is_rendered_literally_for_the_colony_to_bind() {
    let edge = bound_ingress(TELEGRAM, "${TG_CHAT}");
    assert_eq!(
        edge["condition"],
        json!(
            "!has(hop.error_code) && has(hop.chat_id) && \
             (string(hop.chat_id) == '${TG_CHAT}' || \
             string(hop.chat_id).startsWith('${TG_CHAT}:'))"
        ),
        "the environment binding did not reach the condition verbatim"
    );
}

/// A default would unbind the channel whenever the variable is unset, and a
/// quote or a backslash would have to be escaped inside a CEL string. All
/// three are refused rather than repaired.
#[test]
fn a_default_a_quote_or_a_backslash_in_the_binding_is_refused() {
    assert_unbound(
        wish(TELEGRAM, Some("${TG_CHAT:-111}")),
        "an environment binding with a default",
    );
    assert_unbound(wish(TELEGRAM, Some("a'b")), "a binding with a quote");
    assert_unbound(wish(TELEGRAM, Some("a\\b")), "a binding with a backslash");
    assert_unbound(
        wish(TELEGRAM, Some("$TG_CHAT")),
        "a dollar outside the one environment form",
    );
}

/// The channels whose chat id is not a foreign network identity grow without
/// a binding and keep the plain ingress condition.
#[test]
fn the_self_bound_channels_grow_without_a_binding() {
    for template in SELF_BOUND {
        let edge = ingress(&declaration(wish(template, None)));
        assert_eq!(
            edge["condition"],
            json!("!has(hop.error_code)"),
            "{template} is self-bound and must keep the plain ingress"
        );
    }
}

/// A Telegram chat id is a number any account produces by writing to the bot,
/// and any member of a workspace can open a channel with a Slack app. Neither
/// connector ever grows without a binding.
#[test]
fn the_network_connectors_are_never_self_bound() {
    for template in [TELEGRAM, SLACK] {
        assert_unbound(wish(template, None), template);
    }
}

/// A code that travels must be declared: the documented `error_code` strings
/// are public contract.
#[test]
fn the_recipes_cell_declares_the_code_it_now_emits() {
    let raw = std::fs::read_to_string(RECIPES).expect("the shipped config");
    let cfg: Value = meclaw_core::serde_json::from_str(&raw).expect("json");
    let values = cfg["contract"]["emits"]["hop"]["error_code"]["values"]
        .as_array()
        .expect("the error_code enum");
    assert!(
        values.contains(&json!("channel_unbound")),
        "`recipes` emits `channel_unbound` and does not declare it"
    );
}

// ══════════════════════════════════════════════════════════════ the colony

/// The bound chat, as the colony's `.env` carries it, and the stranger's.
const BOUND_CHAT: i64 = 111;
const FOREIGN_CHAT: i64 = 222;
/// The node the channel stands under in its container.
const NODE: &str = "telegram";

/// Puts one wake on the connector's wire, carrying the chat the outside world
/// would have written from.
const DRIVER: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
sys.stdout.write(json.dumps({
    "header": {"route": "in_wire", "wire_chat": hop.get("wire_chat")},
    "messages": doc["body"].get("messages", [])}))
"#;

/// The connector, doubled: every wake becomes an inbound message stamped with
/// the chat it came from, exactly as a connector stamps it — the number as a
/// number. It knows nothing about bindings; that is the edge's job.
const CONNECTOR: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
sys.stdout.write(json.dumps({
    "header": {"chat_id": hop.get("wire_chat"), "user_id": "u-1"},
    "messages": doc["body"].get("messages", [])}))
"#;

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

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().expect("a parent directory")).expect("create the directory");
    std::fs::write(
        p,
        meclaw_core::serde_json::to_string_pretty(v).expect("serialise"),
    )
    .expect("write");
}

/// The member's `channels` container with the connector standing in it, a
/// driver outside, and the root's one way out of the container: a turn goes
/// to the brain. The container's own graph is EMPTY — the edges between the
/// connector and its container are the recipe's, applied below.
///
/// The connector is booted from the tree rather than grown by the mutation: a
/// recipe-grown channel is born asleep (GH #472) and armed by a second act, and
/// what is under test is the ingress edge, not the arming.
fn build_tree(root: &std::path::Path) {
    write(
        root,
        "main/config.json",
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
            {"from": "./driver", "to": format!("./channels/{NODE}"),
             "condition": "has(hop.route) && hop.route == 'in_wire'"},
            {"from": "./channels", "to": "./brain",
             "condition": "has(hop.route) && hop.route == 'turn'"}
        ]}}}),
    );
    write(
        root,
        "main/channels/config.json",
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": []}}}),
    );
    write(
        root,
        "main/driver/config.json",
        &double(
            DRIVER,
            "Test driver: puts one wake on the connector's wire.",
        ),
    );
    write(
        root,
        &format!("main/channels/{NODE}/config.json"),
        &double(CONNECTOR, "Test double for the connector of one channel."),
    );
    // The binding lives in the colony's environment, where the operator keeps
    // it; the manifest names the variable.
    std::fs::write(root.join(".env"), format!("TG_CHAT={BOUND_CHAT}\n")).expect("write the .env");
}

/// The recipe's declaration for this channel, standing at the container this
/// colony has: the scope moves to `/channels` and the node, which the tree
/// already holds, leaves the diff. Every edge is the recipe's, byte for byte.
fn rendered_edges(bind: &str) -> Value {
    let mut params = wish(TELEGRAM, Some(bind));
    params["name"] = json!(NODE);
    let mut decl = declaration(params);
    let edges = decl["diff"]["add_edges"].clone();
    decl["scope"] = json!("/channels");
    decl["diff"] = json!({"add_edges": edges});
    decl
}

async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<Message>) {
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![(
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        )]
    };
    let h = ColonyHandle::new_with_factories_at(td, factories());
    // The recorder stands for the member's brain: everything that reaches it
    // has been stamped with the member's round.
    let (brain_tx, brain_rx) = mpsc::channel::<Message>(32);
    h.spawn(Path::new("/brain"), move || {
        CaptureCell::new(brain_tx.clone())
    })
    .await;

    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the container, the connector and the driver must boot");
    (h, brain_rx)
}

async fn mutate(h: &ColonyHandle, payload: Value) -> MutationOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("send mutation");
    ack_rx.await.expect("mutation ack")
}

/// One wake from `chat`, into the driver.
fn wake(chat: i64, text: &str) -> Message {
    let mut hop = Map::new();
    hop.insert("wire_chat".into(), json!(chat));
    MessageBuilder::new(Path::new("/driver"))
        .body(Body::Inline(json!({"messages": [
            {"origin": "user", "type": "text", "text": text}
        ]})))
        .hop(hop)
        .ttl(64)
        .build()
}

/// The receipts are generous on purpose: a `code` double starts a python, and a
/// loaded machine is not a defect of the edge under test.
const RECEIPT_TIMEOUT: Duration = Duration::from_secs(60);

async fn recv_bounded(rx: &mut mpsc::Receiver<Message>) -> Message {
    tokio::time::timeout(RECEIPT_TIMEOUT, rx.recv())
        .await
        .expect("the brain went quiet — the bound chat's turn never arrived")
        .expect("the brain's channel closed")
}

/// Drain the dead-letter list until one entry satisfies `want`. The list is
/// DRAINED by every read, so what was seen is kept.
async fn wait_for_dead_letter(h: &ColonyHandle, want: impl Fn(&DeadLetter) -> bool) -> DeadLetter {
    let deadline = std::time::Instant::now() + RECEIPT_TIMEOUT;
    let mut seen: Vec<DeadLetter> = Vec::new();
    loop {
        seen.extend(h.drain_dead_letters().await);
        if let Some(i) = seen.iter().position(&want) {
            return seen.swap_remove(i);
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the stranger's message never became a dead letter; seen: {:?}",
            seen.iter()
                .map(|d| (
                    d.sender_path.as_str().to_string(),
                    d.resolved_target.as_str().to_string(),
                    d.reason.as_code()
                ))
                .collect::<Vec<_>>()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn chat_of(h: &Headers) -> Option<&Value> {
    h.hop.get("chat_id")
}

/// The lock on a booted colony. The edges are the recipe's, rendered for
/// `bind_chat: "${TG_CHAT}"` and bound from the colony's `.env` by the mutation
/// door; the connector double raises one turn from the bound chat and one from
/// a stranger's.
///
/// Read at the receivers: the bound chat's turn reaches the brain exactly once,
/// stamped with the member's round; the stranger's reaches nothing that sees a
/// round and lies in the dead-letter list under the substrate's own reason.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_foreign_chat_dead_letters_and_only_the_bound_chat_reaches_the_brain() {
    let td = tempfile::TempDir::new().expect("a temporary directory");
    build_tree(td.path());
    let (h, mut brain) = boot(&td).await;

    // Fail-closed first: a variable the environment does not carry is no
    // binding, and the door refuses the whole declaration.
    match mutate(&h, rendered_edges("${TG_CHAT_NOT_SET}")).await {
        MutationOutcome::Rejected { error_code, .. } => assert_eq!(
            error_code, "env_var_missing",
            "an unset binding variable must fail the mutation by name"
        ),
        other => panic!("an unset binding variable was applied: {other:?}"),
    }

    let outcome = mutate(&h, rendered_edges("${TG_CHAT}")).await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "the recipe's edges, bound from the .env, were not committed: {outcome:?}"
    );

    // The stranger writes first. Its message must die at the connector: the
    // only edges out of it are the bound ingress and the error edge, and it
    // takes neither.
    let foreign = wake(FOREIGN_CHAT, "hello from a stranger");
    let foreign_trace = foreign.trace_id;
    h.send(foreign).await;
    let dead = wait_for_dead_letter(&h, |d| d.message.trace_id == foreign_trace).await;
    assert_eq!(
        dead.reason.as_code(),
        "no_route",
        "the stranger's turn must find no edge at all"
    );
    assert_eq!(dead.sender_path, Path::new(&format!("/channels/{NODE}")));
    assert_eq!(chat_of(&dead.message.headers), Some(&json!(FOREIGN_CHAT)));
    assert!(
        !dead.message.headers.context.contains_key("audience_set"),
        "the dead-lettered stranger carries a round — the ingress modifier ran \
         on it: {:?}",
        dead.message.headers.context
    );

    // The member writes. Its turn reaches the brain, stamped with the round.
    let own = wake(BOUND_CHAT, "hello");
    let own_trace = own.trace_id;
    h.send(own).await;
    let got = recv_bounded(&mut brain).await;
    assert_eq!(
        got.trace_id, own_trace,
        "the first thing the brain heard is not the bound chat's turn — \
         the stranger's turn reached the round: {:?}",
        got.headers
    );
    assert_eq!(got.headers.context.get("audience_set"), Some(&json!(ROUND)));
    assert_eq!(got.headers.context.get("chat_id"), Some(&json!(BOUND_CHAT)));
    assert_eq!(got.headers.context.get("channel_node"), Some(&json!(NODE)));
    assert_eq!(got.headers.hop.get("route"), Some(&json!("turn")));

    // Exactly once, without a time window: a fence from the same chat, sent
    // only now. Everything the first turn could still have produced was routed
    // before the fence existed, and the brain's mailbox is in order.
    let fence = wake(BOUND_CHAT, "fence");
    let fence_trace = fence.trace_id;
    h.send(fence).await;
    let mut heard = vec![got];
    loop {
        let m = recv_bounded(&mut brain).await;
        let done = m.trace_id == fence_trace;
        heard.push(m);
        if done {
            break;
        }
    }
    let traces: Vec<Uuid> = heard.iter().map(|m| m.trace_id).collect();
    assert_eq!(
        traces,
        vec![own_trace, fence_trace],
        "the brain heard the bound chat's turn more than once, or heard the stranger"
    );
    assert!(
        heard
            .iter()
            .all(|m| chat_of(&m.headers) == Some(&json!(BOUND_CHAT))),
        "a turn from another chat reached the brain"
    );

    // And in the message log, the stranger's trace never left the connector:
    // no hop of it reached the container or anything above it, and no hop of
    // it carried a round.
    let db = td.path().join("colony.db");
    let foreign_trace = foreign_trace.to_string();
    wait_for_message_log_count(&db, &foreign_trace, 2, RECEIPT_TIMEOUT).await;
    let conn = rusqlite::Connection::open(&db).expect("colony.db");
    let mut stmt = conn
        .prepare("SELECT to_path, headers FROM message_log WHERE trace_id = ? ORDER BY rowid")
        .expect("prepare");
    let rows: Vec<(String, String)> = stmt
        .query_map([&foreign_trace], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("query")
        .map(|r| r.expect("row"))
        .collect();
    let reached: Vec<&str> = rows.iter().map(|(to, _)| to.as_str()).collect();
    let connector = format!("/channels/{NODE}");
    assert!(
        reached.contains(&"/driver") && reached.contains(&connector.as_str()),
        "the stranger's trace is not in the log as far as the connector: {reached:?}"
    );
    assert!(
        !reached.contains(&"/channels") && !reached.contains(&"/brain"),
        "the stranger's trace went further than the connector: {reached:?}"
    );
    assert!(
        rows.iter()
            .all(|(_, headers)| !headers.contains("audience_set")),
        "a hop of the stranger's trace carried a round: {rows:?}"
    );

    h.shutdown().await;
}

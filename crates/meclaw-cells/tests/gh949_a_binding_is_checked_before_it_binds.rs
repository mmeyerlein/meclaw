//! GH #949 — a binding is checked before it binds.
//!
//! `bind_chat` (GH #940) and `bind_user` (GH #949) are written into
//! single-quoted CEL strings on the ingress edge of a channel: the condition
//! that takes the bound chat, and the modifier that names the speaker. A value
//! that cannot stand inside such a string breaks the edge, and one that closes
//! the string EXTENDS it — `1' || true || '1` turns "this chat" into "every
//! chat", the unbound channel the binding exists to prevent.
//!
//! Two places decide, and this file measures both:
//!
//! * **the recipe**, for what the wish spells out. A literal carrying a quote,
//!   a backslash, a stray `$`, a line break or any other control character —
//!   C0, DEL, C1, U+2028, U+2029 — is asked as `channel_unbound` with the
//!   parameter it names, and nothing is rendered. Before GH #949 a line break
//!   or a control character rendered green and failed one hop later, as a CEL
//!   parse error at the mutation door (`edge_schema`), and `bind_user` did not
//!   exist.
//! * **the mutation door**, for the value behind `${NAME}`. The recipe never
//!   sees it: the colony reads its `.env` into a map and binds the token when
//!   the manifest is applied (`substitute_mutation_diff`). The door must never
//!   commit a binding that breaks or extends the CEL string: a value with a
//!   quote (`'` or `"`), a backslash or a control character, bound into an
//!   edge's condition or modifier, refuses the whole declaration as
//!   `env_value_unsafe`, naming the variable and never the value.
//!
//! No model and no network.

use meclaw_cells::code::CodeCellFactory;
use meclaw_colony::cel_eval::{
    apply_modifier, evaluate_condition, parse_condition, parse_modifier,
};
use meclaw_colony::config::ModifierSpec;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Headers, Path, Uuid};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, emit_one, shipped_script};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

const RECIPES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/recipes/config.json"
);

const MEMBER: &str = "/os/orgs/acme/members/alex";
const TELEGRAM: &str = "telegram-connector@2.1.0";
const SPEAKER: &str = "member:alex";

/// Every class of literal the recipe refuses, by name. The value is interior:
/// a binding is trimmed before it is judged, so a class only at the ends would
/// test the trim.
const BAD: [(&str, &str); 17] = [
    ("a line feed", "11\n1"),
    ("a carriage return", "11\r1"),
    ("a tab", "1\t1"),
    ("NUL", "1\u{0}1"),
    ("ESC", "1\u{1b}1"),
    ("the last C0 control", "1\u{1f}1"),
    ("DEL", "1\u{7f}1"),
    ("the first C1 control (NEL)", "1\u{85}1"),
    ("the last C1 control", "1\u{9f}1"),
    ("the line separator", "1\u{2028}1"),
    ("the paragraph separator", "1\u{2029}1"),
    ("a quote", "1'1"),
    ("a backslash", "1\\1"),
    ("a dollar outside the environment form", "$TG_CHAT"),
    ("an environment default", "${TG_CHAT:-111}"),
    ("two environment tokens", "${TG_A}${TG_B}"),
    ("an environment token inside a literal", "x${TG_CHAT}"),
];

/// Literals and the environment form that bind.
const GOOD: [&str; 6] = ["111", "-1001234", "C1", "C1:1700.1", "chat-ä", "${TG_CHAT}"];

// ══════════════════════════════════════════════════════════════ the recipe

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

/// A Telegram channel wish with the person named and the two bindings as
/// given; `None` leaves the key out.
fn wish(chat: Option<Value>, user: Option<Value>) -> Value {
    let mut params = json!({
        "scope": MEMBER, "level": "channel", "name": "telegram",
        "template": TELEGRAM, "assistant": "scribe",
        "ctx": {"member_person": "alex"}});
    if let Some(c) = chat {
        params["bind_chat"] = c;
    }
    if let Some(u) = user {
        params["bind_user"] = u;
    }
    params
}

fn payload(out: &Value) -> Value {
    meclaw_core::serde_json::from_str(out["messages"][0]["text"].as_str().expect("a payload"))
        .expect("json payload")
}

/// The refusal: named, with the one parameter it asks for, nothing rendered.
fn assert_unbound(params: Value, key: &str, why: &str) {
    let out = run_recipes(params);
    assert_eq!(
        out["header"],
        json!({"operation": "recipe", "error_code": "channel_unbound",
               "recipe": "grow_level"}),
        "{why}: {out}"
    );
    assert!(
        out["manifest"].is_null(),
        "{why}: a refusal renders nothing: {out}"
    );
    let said = payload(&out);
    assert_eq!(
        said["missing"],
        json!([format!("params.{key}")]),
        "{why}: {said}"
    );
    assert!(
        said["reason"].as_str().is_some_and(|r| r.contains(key)),
        "{why}: the refusal does not name what it needs: {said}"
    );
}

/// The ingress edge of a rendered wish.
fn ingress(params: Value) -> Value {
    let out = run_recipes(params);
    assert!(
        out["header"]["error_code"].is_null(),
        "the recipe refused a binding it is supposed to render: {out}"
    );
    out["manifest"][0]["diff"]["add_edges"]
        .as_array()
        .expect("add_edges")
        .iter()
        .find(|e| {
            e["from"] == json!("./telegram")
                && e["to"] == json!(".")
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.starts_with("!has(hop.error_code)"))
        })
        .unwrap_or_else(|| panic!("no ingress edge: {out}"))
        .clone()
}

fn fires(edge: &Value, hop: Value) -> bool {
    let cond = edge["condition"].as_str().expect("a condition");
    let compiled = parse_condition(cond).expect("the condition compiles");
    let h = Headers::from_parts(Map::new(), hop.as_object().cloned().unwrap_or_default());
    evaluate_condition(&compiled, &h.context, &h.hop).unwrap_or(false)
}

fn speaker_after(edge: &Value, hop: Value) -> String {
    let spec: ModifierSpec =
        meclaw_core::serde_json::from_value(edge["modifier"].clone()).expect("modifier spec");
    let compiled = parse_modifier(&spec).expect("the modifier compiles");
    let h = Headers::from_parts(Map::new(), hop.as_object().cloned().unwrap_or_default());
    let out = apply_modifier(&compiled, &h).expect("the modifier evaluates");
    out.context
        .get("speaker")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Red before GH #949: the control-character classes rendered for
/// `bind_chat`, and `bind_user` was not read at all, so every row of the
/// second half rendered.
#[test]
fn every_class_of_bad_literal_is_asked_as_channel_unbound() {
    for (what, value) in BAD {
        assert_unbound(wish(Some(json!(value)), None), "bind_chat", what);
        assert_unbound(
            wish(Some(json!("111")), Some(json!(value))),
            "bind_user",
            what,
        );
    }
}

/// The chat is asked first: a channel without one has nothing for a sender to
/// be proven in.
#[test]
fn the_chat_is_asked_before_the_sender() {
    assert_unbound(
        wish(Some(json!("1\n1")), Some(json!("2\n2"))),
        "bind_chat",
        "both bad",
    );
    assert_unbound(
        wish(None, Some(json!("111"))),
        "bind_chat",
        "a sender without a chat",
    );
}

/// A sender is a string like the chat. A number is asked with the reason that a
/// string is expected; blank, `false`, a list or an object are asked too. A
/// JSON `null` is an absent key, as the builder README says.
#[test]
fn a_sender_that_is_not_a_string_is_asked() {
    let numeric = wish(Some(json!("111")), Some(json!(111)));
    assert_unbound(numeric.clone(), "bind_user", "a number");
    assert!(
        payload(&run_recipes(numeric))["reason"]
            .as_str()
            .is_some_and(|r| r.contains("string")),
        "the refusal of a number does not say a string is expected"
    );
    for (what, value) in [
        ("an empty string", json!("")),
        ("a blank string", json!("   ")),
        ("false", json!(false)),
        ("a list", json!(["111"])),
        ("an object", json!({"id": "111"})),
    ] {
        assert_unbound(wish(Some(json!("111")), Some(value)), "bind_user", what);
    }
    let edge = ingress(wish(Some(json!("111")), Some(Value::Null)));
    assert_eq!(
        speaker_after(&edge, json!({"chat_id": 111, "user_id": 111})),
        SPEAKER,
        "a null bind_user is an absent one: the chat id is the proof"
    );
}

/// What binds: plain ids of both networks, a thread, a non-ASCII letter and the
/// environment form. Each renders an edge the real parser compiles; a literal
/// takes exactly its own chat, and as a sender it names exactly its own user.
#[test]
fn plain_literals_and_the_environment_form_bind() {
    for value in GOOD {
        let as_chat = ingress(wish(Some(json!(value)), None));
        // The sender is bound in a chat whose id is none of the values, so the
        // chat-id proof can never stand in for it.
        let as_user = ingress(wish(Some(json!("999")), Some(json!(value))));
        if value.starts_with('$') {
            assert!(
                as_chat["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains(&format!("'{value}'"))),
                "the environment form is the colony's to bind: {as_chat}"
            );
            assert!(
                as_user["modifier"]["set_context"]["speaker"]
                    .as_str()
                    .is_some_and(|c| c.contains(&format!("'{value}'"))),
                "{as_user}"
            );
            continue;
        }
        assert!(fires(&as_chat, json!({"chat_id": value})), "{value}");
        assert!(
            !fires(&as_chat, json!({"chat_id": format!("{value}0")})),
            "{value}"
        );
        assert_eq!(
            speaker_after(&as_chat, json!({"chat_id": value, "user_id": value})),
            SPEAKER,
            "{value}"
        );
        assert_eq!(
            speaker_after(&as_user, json!({"chat_id": "999", "user_id": value})),
            SPEAKER,
            "{value}"
        );
        assert_eq!(
            speaker_after(&as_user, json!({"chat_id": "999", "user_id": "999"})),
            "",
            "{value}: the chat id stood in for a named sender"
        );
    }
}

// ══════════════════════════════════════════════════════════════ the door

/// The connector, doubled. Nothing is sent through it here: it only has to
/// stand, so the door knows the node the rendered edges start at.
const CONNECTOR: &str = r#"
import sys, json
doc = json.load(sys.stdin)
sys.stdout.write(json.dumps({"header": {}, "messages": doc["body"].get("messages", [])}))
"#;

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().expect("a parent directory")).expect("create the directory");
    std::fs::write(
        p,
        meclaw_core::serde_json::to_string_pretty(v).expect("serialise"),
    )
    .expect("write");
}

/// A `channels` container with one connector, a brain the container turns go
/// to, and a `.env` with one good value and two that close the CEL string —
/// one that breaks it, one that extends it into "every chat".
fn build_tree(root: &std::path::Path) {
    write(
        root,
        "main/config.json",
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
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
        "main/channels/telegram/config.json",
        &json!({
            "cell": {"type": "code"},
            "params": {"runner": "python3", "script_inline": CONNECTOR,
                       "external_timeout_ms": 10000},
            "contract": {
                "version": "1.0.0",
                "settings": {},
                "emits": {"body": {"messages": {"type": "array", "required": false}}},
                "consumes": {"body": {"messages": {"type": "array", "required": false}}},
                "capabilities": ["shell:exec"]
            },
            "description": {"purpose": "Test double for the connector of one channel.",
                            "use_when": "Test fixture only.",
                            "not_in_scope": "Not a template."}
        }),
    );
    std::fs::write(
        root.join(".env"),
        "TG_CHAT=111\nTG_USER=111\nTG_QUOTE=1'1\nTG_INJECT=1' || true || '1\n",
    )
    .expect("write the .env");
}

/// The recipe's declaration for the channel, standing at the container this
/// colony has (the node is the tree's, so it leaves the diff).
fn rendered_edges(chat: &str, user: Option<&str>) -> Value {
    let out = run_recipes(wish(Some(json!(chat)), user.map(|u| json!(u))));
    assert!(out["header"]["error_code"].is_null(), "{out}");
    let mut decl = out["manifest"][0].clone();
    let edges = decl["diff"]["add_edges"].clone();
    decl["scope"] = json!("/channels");
    decl["diff"] = json!({"add_edges": edges});
    decl
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

/// A colony with the tree above, booted, and the recorder for the brain kept
/// alive beside it.
async fn door(td: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<meclaw_core::Message>) {
    build_tree(td.path());
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![(
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        )]
    };
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (brain_tx, brain_rx) = mpsc::channel(8);
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
        .expect("the container and the connector must boot");
    (h, brain_rx)
}

/// The door's refusal of a value from the `.env`: the whole declaration, under
/// its own code, naming the variable and never the value. A value bound from
/// the environment may be a secret, and `details` travels into the mutation
/// log and the builder's receipt.
fn assert_refused_by_name(outcome: &MutationOutcome, var: &str, value: &str, what: &str) {
    match outcome {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => {
            assert_eq!(error_code, "env_value_unsafe", "{what}: {outcome:?}");
            assert!(
                details.contains(&format!("${{{var}}}")),
                "{what}: the refusal does not name the variable: {details}"
            );
            assert!(
                !details.contains(value),
                "{what}: the refusal carries the value: {details}"
            );
        }
        MutationOutcome::Committed { .. } => panic!(
            "{what}: the door committed a binding whose value is not a plain id: {outcome:?}"
        ),
    }
}

/// A quote from the `.env` breaks the CEL string, and the door refuses the
/// whole declaration where it binds the value (`env_value_unsafe`) — as the
/// chat, and since GH #949 as the sender too. A plain id from the same `.env`
/// binds.
///
/// Red before GH #949: `bind_user` was not read, so the sender's quote never
/// reached an edge and the declaration was committed; and the chat's quote was
/// refused only one step later, as a CEL parse error of the edge
/// (`edge_schema`), under a code that names the edge and not the variable.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_quote_from_the_environment_never_binds() {
    let td = tempfile::TempDir::new().expect("a temporary directory");
    let (h, _brain) = door(&td).await;

    for (what, chat, user) in [
        ("a quote in the chat", "${TG_QUOTE}", None),
        ("a quote in the sender", "${TG_CHAT}", Some("${TG_QUOTE}")),
    ] {
        let outcome = mutate(&h, rendered_edges(chat, user)).await;
        assert_refused_by_name(&outcome, "TG_QUOTE", "1'1", what);
    }

    let outcome = mutate(&h, rendered_edges("${TG_CHAT}", Some("${TG_USER}"))).await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "a plain id from the .env did not bind: {outcome:?}"
    );

    h.shutdown().await;
}

/// The value that closes the CEL string and reopens it (`1' || true || '1`)
/// PARSES, so the door's CEL check does not see it: committed, the ingress
/// takes every chat — the unbound channel — and, as the sender, names everybody
/// the member. The recipe cannot refuse it (it only ever sees the token), so
/// the door has to, where it binds the value.
///
/// Red before GH #949: the mutation door bound the value and committed the
/// declaration. Now the substitution pass refuses an environment value that a
/// CEL string cannot hold (`'`, `"`, `\`, control characters) where it binds
/// it into an edge's condition or modifier (`substitute_mutation_diff`), as
/// `env_value_unsafe`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reopened_string_from_the_environment_never_binds() {
    let td = tempfile::TempDir::new().expect("a temporary directory");
    let (h, _brain) = door(&td).await;

    for (what, chat, user) in [
        ("a reopened string in the chat", "${TG_INJECT}", None),
        (
            "a reopened string in the sender",
            "${TG_CHAT}",
            Some("${TG_INJECT}"),
        ),
    ] {
        let outcome = mutate(&h, rendered_edges(chat, user)).await;
        assert_refused_by_name(&outcome, "TG_INJECT", "|| true", what);
    }

    h.shutdown().await;
}

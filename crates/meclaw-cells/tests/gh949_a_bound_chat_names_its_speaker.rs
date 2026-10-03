//! GH #949 — a bound chat names its speaker, and only with a sender proof.
//!
//! The ingress edge of a bound channel (GH #940) stamps the member's ROUND on
//! every turn of the bound chat. Since GH #949 it also names WHO said the turn:
//! `context.speaker = 'member:<person>'`, the very spelling of the member's
//! entry in that round, which the memory files beside every user episode.
//!
//! The bound chat alone does not prove the speaker: a channel may be bound to a
//! group or a Slack channel, where everybody who writes is somebody. So the
//! stamp needs a SENDER PROOF on the hop the connector raised:
//!
//! ```text
//! has(hop.user_id) && string(hop.user_id) == '<bind_user>'   -- bind_user named
//! has(hop.user_id) && string(hop.user_id) == '<bind_chat>'   -- otherwise
//! ```
//!
//! The second form holds only in a one-to-one chat, where the chat id IS the
//! sender's id. Without a proof the key is empty — the round is unchanged, the
//! turn is still the member's conversation, it just names nobody. The
//! self-bound templates, apps and every other edge stamp no speaker at all, and
//! no shipped edge on the way to a tool deletes it; the one edge that does is
//! the session keeper's close, which is not a turn of the speaker.
//!
//! Three halves, measured where they are decided: the renderer (the SHIPPED
//! recipe, its output run through the REAL evaluator the substrate uses), the
//! shipped tree (a sweep over every template edge), and a booted colony that
//! applies the rendered edges through the mutation door with the bindings read
//! from its `.env` and reads the result at the recorder standing for the brain.
//!
//! No model and no network: `code` doubles and one capture cell.

use meclaw_cells::code::CodeCellFactory;
use meclaw_colony::cel_eval::{
    apply_modifier, evaluate_condition, parse_condition, parse_modifier,
};
use meclaw_colony::config::ModifierSpec;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Headers, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, emit_all, shipped_script};
use std::path::PathBuf;
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
/// The speaker, as the member's entry in that round spells it.
const SPEAKER: &str = "member:alex";

/// The channel templates whose ingress is bound before the turn is raised
/// (`SELF_BOUND_CHANNELS` in `recipes`), at the versions the tree ships.
const SELF_BOUND: [&str; 4] = [
    "chat-channel@1.0.1",
    "voice@2.5.0",
    "web@2.2.0",
    "terminal@1.0.2",
];
const TELEGRAM: &str = "telegram-connector@2.1.0";
const SLACK: &str = "slack-agent@2.1.2";

// ══════════════════════════════════════════════════════════════ the renderer

fn run_recipes(recipe: &str, params: Value) -> Vec<Value> {
    emit_all(
        &shipped_script(RECIPES),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!({"recipe": recipe, "request": "…",
                                         "params": params}).to_string()}],
        }),
    )
}

/// A channel wish named after its template, with the person named and the
/// two bindings as given.
fn wish(template: &str, chat: Option<&str>, user: Option<&str>) -> Value {
    let name = template.split('@').next().expect("a template name");
    let mut params = json!({
        "scope": MEMBER, "level": "channel", "name": name,
        "template": template, "assistant": AGENT,
        "ctx": {"member_person": PERSON}});
    if let Some(c) = chat {
        params["bind_chat"] = json!(c);
    }
    if let Some(u) = user {
        params["bind_user"] = json!(u);
    }
    params
}

fn declaration(recipe: &str, params: Value) -> Value {
    let out = run_recipes(recipe, params);
    let first = out.first().expect("an emission");
    assert!(
        first["header"]["error_code"].is_null(),
        "the recipe refused a wish it is supposed to render: {first}"
    );
    first["manifest"]
        .as_array()
        .unwrap_or_else(|| panic!("no manifest: {first}"))[0]
        .clone()
}

fn edges(decl: &Value) -> Vec<Value> {
    decl["diff"]["add_edges"]
        .as_array()
        .expect("add_edges")
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
    edges(decl)
        .into_iter()
        .find(|e| {
            e["from"] == json!(node)
                && e["to"] == json!(".")
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.starts_with("!has(hop.error_code)"))
        })
        .unwrap_or_else(|| panic!("no ingress edge out of {node}: {decl}"))
}

fn bound_ingress(template: &str, chat: &str, user: Option<&str>) -> Value {
    ingress(&declaration("grow_level", wish(template, Some(chat), user)))
}

fn map(v: &Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

/// One inbound message as a connector emits it: everything on the hop, and
/// whatever the chain carried in on the context.
fn inbound(context: Value, hop: Value) -> Headers {
    Headers::from_parts(map(&context), map(&hop))
}

/// Would this edge take that message? `Err` is the substrate's skip, so it is
/// `false` here for the same reason it is there.
fn fires(edge: &Value, headers: &Headers) -> bool {
    let cond = edge["condition"].as_str().expect("a condition");
    let compiled = parse_condition(cond).expect("the condition compiles");
    evaluate_condition(&compiled, &headers.context, &headers.hop).unwrap_or(false)
}

/// Run a rendered edge's modifier through the REAL evaluator the substrate
/// uses. A modifier that fails to evaluate skips the whole edge there, so a
/// failure here is a turn that would have vanished.
fn traverse(edge: &Value, headers: &Headers) -> Headers {
    let spec: ModifierSpec =
        meclaw_core::serde_json::from_value(edge["modifier"].clone()).expect("modifier spec");
    let compiled = parse_modifier(&spec).expect("the modifier compiles");
    apply_modifier(&compiled, headers).expect("the modifier evaluates")
}

/// The context a turn leaves the channel with, or `None` when the ingress edge
/// does not take it.
fn turn(edge: &Value, hop: Value) -> Option<Headers> {
    let h = inbound(json!({}), hop);
    fires(edge, &h).then(|| traverse(edge, &h))
}

fn speaker_of(h: &Headers) -> Option<&str> {
    h.context.get("speaker").and_then(Value::as_str)
}

fn round_of(h: &Headers) -> Option<&str> {
    h.context.get("audience_set").and_then(Value::as_str)
}

/// The member's own chat, the sender proven: the turn names the member, in the
/// spelling of the member's entry in the round.
///
/// Red before GH #949: the ingress modifier stamped no `speaker`, so
/// `speaker_of` is `None`.
#[test]
fn the_member_in_their_own_chat_is_named_as_the_speaker() {
    let edge = bound_ingress(TELEGRAM, "111", None);
    for hop in [
        json!({"chat_id": 111, "user_id": 111}),
        json!({"chat_id": "111", "user_id": "111"}),
        json!({"chat_id": 111, "user_id": "111", "platform": "telegram"}),
    ] {
        let h = turn(&edge, hop.clone())
            .unwrap_or_else(|| panic!("the bound chat found no ingress edge: {hop}"));
        assert_eq!(
            speaker_of(&h),
            Some(SPEAKER),
            "the member's own turn ({hop}) names no speaker: {:?}",
            h.context
        );
        assert_eq!(round_of(&h), Some(ROUND));
    }
    // One spelling: the speaker is the member's entry of the round it speaks in.
    let round: Vec<String> = meclaw_core::serde_json::from_str(ROUND).expect("the round");
    assert!(round.iter().any(|r| r == SPEAKER), "{round:?}");
}

/// The bound chat with another sender in it: the turn is the member's
/// conversation as today — same edge, same round — and names nobody. A
/// speaker some earlier hop left in the context is overwritten, not kept.
#[test]
fn a_foreign_sender_in_the_bound_chat_speaks_in_the_round_but_names_no_speaker() {
    let edge = bound_ingress(TELEGRAM, "111", None);
    for (hop, what) in [
        (json!({"chat_id": 111, "user_id": 222}), "another sender"),
        (
            json!({"chat_id": 111, "user_id": "222"}),
            "another sender as a string",
        ),
        (
            json!({"chat_id": 111, "user_id": 1111}),
            "a sender whose digits extend the binding",
        ),
        (
            json!({"chat_id": 111, "user_id": 11}),
            "a sender whose digits are a prefix of the binding",
        ),
    ] {
        let h = turn(&edge, hop.clone())
            .unwrap_or_else(|| panic!("{what}: the bound chat lost its ingress edge ({hop})"));
        assert_eq!(round_of(&h), Some(ROUND), "{what}: the round changed");
        assert_eq!(
            speaker_of(&h).unwrap_or_default(),
            "",
            "{what} ({hop}) was named as the member"
        );
    }
    // Whatever a chain carried in, the edge says what THIS turn proves.
    let carried = inbound(
        json!({"speaker": SPEAKER}),
        json!({"chat_id": 111, "user_id": 222}),
    );
    assert!(fires(&edge, &carried));
    let after = traverse(&edge, &carried);
    assert_eq!(
        speaker_of(&after),
        Some(""),
        "a carried-in speaker survived"
    );
}

/// A connector that stamps no sender (a channel post, a service message):
/// round as today, no speaker — and the modifier still evaluates, so the turn
/// does not vanish on the edge.
#[test]
fn a_turn_without_a_sender_names_no_speaker() {
    let edge = bound_ingress(TELEGRAM, "111", None);
    let h = turn(&edge, json!({"chat_id": 111})).expect("the bound chat takes the edge");
    assert_eq!(round_of(&h), Some(ROUND));
    assert_eq!(speaker_of(&h), Some(""), "{:?}", h.context);
}

/// `bind_user` names the sender that speaks as the member, wherever the bound
/// chat is a group: a Telegram group, a Slack channel and its threads. Without
/// it a group names nobody — a group's id is never a sender's — and with it the
/// chat id no longer stands in for the proof.
#[test]
fn a_named_sender_speaks_as_the_member_in_a_group() {
    let group = bound_ingress(TELEGRAM, "-1001234", Some("111"));
    let h = turn(&group, json!({"chat_id": -1_001_234, "user_id": 111})).expect("the group");
    assert_eq!(speaker_of(&h), Some(SPEAKER));
    assert_eq!(round_of(&h), Some(ROUND));
    let h = turn(&group, json!({"chat_id": -1_001_234, "user_id": 222})).expect("the group");
    assert_eq!(speaker_of(&h), Some(""), "another member of the group");

    // A group's id is never a sender's, so without `bind_user` the chat-id
    // proof never holds there.
    let unnamed = bound_ingress(TELEGRAM, "-1001234", None);
    for user in [111, 222] {
        let h = turn(&unnamed, json!({"chat_id": -1_001_234, "user_id": user})).expect("the group");
        assert_eq!(
            speaker_of(&h).unwrap_or_default(),
            "",
            "a group without bind_user named sender {user} as the member"
        );
    }

    let slack = bound_ingress(SLACK, "C1", Some("U1"));
    for chat in ["C1", "C1:1700.1"] {
        let h = turn(&slack, json!({"chat_id": chat, "user_id": "U1"})).expect("the channel");
        assert_eq!(speaker_of(&h), Some(SPEAKER), "{chat}");
    }
    let h = turn(&slack, json!({"chat_id": "C1", "user_id": "U2"})).expect("the channel");
    assert_eq!(speaker_of(&h), Some(""));

    // A Slack DM id is no user id: without `bind_user` nobody is named.
    let dm = bound_ingress(SLACK, "D1", None);
    let h = turn(&dm, json!({"chat_id": "D1", "user_id": "U1"})).expect("the DM");
    assert_eq!(speaker_of(&h), Some(""));

    // Named, the sender is the ONLY proof: the chat id does not stand in.
    let named = bound_ingress(TELEGRAM, "111", Some("999"));
    let h = turn(&named, json!({"chat_id": 111, "user_id": 111})).expect("the chat");
    assert_eq!(speaker_of(&h), Some(""));
    let h = turn(&named, json!({"chat_id": 111, "user_id": 999})).expect("the chat");
    assert_eq!(speaker_of(&h), Some(SPEAKER));
}

/// The expression as written, both forms. The environment form is the
/// colony's to bind when the mutation is applied, exactly like the chat.
#[test]
fn the_speaker_expression_is_rendered_as_written() {
    let set = |chat: &str, user: Option<&str>| -> Value {
        bound_ingress(TELEGRAM, chat, user)["modifier"]["set_context"]["speaker"].clone()
    };
    assert_eq!(
        set("111", None),
        json!("has(hop.user_id) && string(hop.user_id) == '111' ? 'member:alex' : ''")
    );
    assert_eq!(
        set("${TG_CHAT}", Some("${TG_USER}")),
        json!("has(hop.user_id) && string(hop.user_id) == '${TG_USER}' ? 'member:alex' : ''")
    );
    // The chat binding itself is untouched by the sender (GH #940's form).
    assert_eq!(
        bound_ingress(TELEGRAM, "${TG_CHAT}", Some("${TG_USER}"))["condition"],
        json!(
            "!has(hop.error_code) && has(hop.chat_id) && \
             (string(hop.chat_id) == '${TG_CHAT}' || \
             string(hop.chat_id).startsWith('${TG_CHAT}:'))"
        )
    );
}

/// The self-bound templates carry no sender id to prove anything with, and the
/// round is the only claim they make: no `speaker` key on their ingress, with
/// or without bindings in the wish, and none after a turn passes.
#[test]
fn the_self_bound_channels_never_name_a_speaker() {
    for template in SELF_BOUND {
        // GH #979 -- `web` with `bind_user` is the one exception: the proxy's
        // identity header IS a sender proof there
        // (`gh979_speaker_stamp_from_bound_channel.rs`, `mod web_session`); with a chat only
        // it still names nobody.
        let bound = if template.starts_with("web@") {
            wish(template, Some("111"), None)
        } else {
            wish(template, Some("111"), Some("111"))
        };
        for params in [wish(template, None, None), bound] {
            let edge = ingress(&declaration("grow_level", params));
            assert!(
                edge["modifier"]["set_context"].get("speaker").is_none(),
                "{template} stamps a speaker: {edge}"
            );
            let h = traverse(
                &edge,
                &inbound(json!({}), json!({"chat_id": "111", "user_id": "111"})),
            );
            assert!(
                !h.context.contains_key("speaker"),
                "{template}: {:?}",
                h.context
            );
        }
    }
}

/// An app's turn never passes a channel ingress, and nothing the recipe draws
/// for an app or a screen names a speaker.
#[test]
fn an_app_or_a_screen_never_stamps_a_speaker() {
    let mut rendered = Vec::new();
    rendered.extend(edges(&declaration(
        "grow_level",
        json!({"scope": MEMBER, "level": "screen", "name": "display",
               "template": "display@1.0.0"}),
    )));
    rendered.extend(edges(&declaration(
        "grow_level",
        json!({"scope": MEMBER, "level": "app", "name": "colony-view",
               "template": "colony-view@1.0.0", "screen": "display"}),
    )));
    rendered.extend(edges(&declaration(
        "install_app",
        json!({"scope": MEMBER, "app": "probe-app", "template": "probe-app@1.0.0",
               "screen": "display", "generation": "sam",
               "ctx": {"member_person": "alex"}, "declaration": {
                   "screen": {"out": ["view"], "back": ["event", "receipt"]},
                   "listens": ["turn", "answer", "partial", "mutation_committed", "close"],
                   "offers": [{"kind": "tool", "at": "./sink", "tools": ["probe_tool"]},
                              {"kind": "sidecar", "at": "./sink", "section": "probe"}],
                   "observes_tool_calls": {"at": "./sink", "tools": ["probe_tool"]},
                   "observes_tool_results": {"at": "./sink", "tools": ["probe_tool"]},
                   "pins": "./sink"}}),
    )));
    assert!(rendered.len() > 10, "{rendered:#?}");
    for e in &rendered {
        assert!(
            !e.to_string().contains("speaker"),
            "an app or screen edge touches the speaker: {e}"
        );
    }
}

/// The nearest point to a tool the renderer draws: the `tool` edge from a
/// surface of the generation into an app's cell (`install_app` offers), and
/// the observer's tap beside it. Run through the real evaluator on the context
/// a member's own turn left the channel with, the speaker reaches the tool.
#[test]
fn the_speaker_reaches_the_tool_edge() {
    let from_channel = turn(
        &bound_ingress(TELEGRAM, "111", None),
        json!({"chat_id": 111, "user_id": 111}),
    )
    .expect("the bound chat");
    assert_eq!(speaker_of(&from_channel), Some(SPEAKER));

    let install = declaration(
        "install_app",
        json!({"scope": MEMBER, "app": "probe-app", "template": "probe-app@1.0.0",
               "screen": "display", "generation": "sam",
               "ctx": {"member_person": "alex"}, "declaration": {
                   "offers": [{"kind": "tool", "at": "./sink", "tools": ["probe_tool"]}],
                   "observes_tool_calls": {"at": "./watch", "tools": ["probe_tool"]}}}),
    );
    let tool_edges: Vec<Value> = edges(&install)
        .into_iter()
        .filter(|e| e["lane"] == json!("tool"))
        .collect();
    assert_eq!(
        tool_edges.len(),
        4,
        "an offer and an observer, from both surfaces: {tool_edges:#?}"
    );
    let call = Headers::from_parts(
        from_channel.context.clone(),
        map(&json!({"route": "tool", "tool_name": "probe_tool"})),
    );
    for e in &tool_edges {
        assert!(fires(e, &call), "the tool call does not take {e}");
        let at_tool = traverse(e, &call);
        assert_eq!(
            speaker_of(&at_tool),
            Some(SPEAKER),
            "the tool edge {e} drops the speaker"
        );
        assert_eq!(round_of(&at_tool), Some(ROUND));
    }
}

// ══════════════════════════════════════════════════════════════ the tree

fn templates_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

fn configs(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            configs(&p, out);
        } else if p.file_name().and_then(|n| n.to_str()) == Some("config.json") {
            out.push(p);
        }
    }
}

/// Every shipped edge, as `(config path relative to templates/, edge)`.
fn shipped_edges() -> Vec<(String, Value)> {
    let root = templates_dir();
    let mut files = Vec::new();
    configs(&root, &mut files);
    let mut out = Vec::new();
    for f in files {
        let raw = std::fs::read_to_string(&f).expect("a shipped config is readable");
        let v: Value = meclaw_core::serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        let rel = f
            .strip_prefix(&root)
            .expect("under templates/")
            .display()
            .to_string();
        for e in v["params"]["graph"]["edges"]
            .as_array()
            .cloned()
            .unwrap_or_default()
        {
            out.push((rel.clone(), e));
        }
    }
    out
}

/// The speaker is minted in ONE place, the bound ingress the recipe renders,
/// and dropped in ONE place: the session keeper's close out of `./stamp`. A
/// close a round change sends mid-turn is not a turn of the speaker, and it
/// would otherwise leave with the speaker of the turn that ended the
/// generation. Nothing on the way from a channel to a brain or a tool deletes
/// it.
///
/// Red before GH #949: the close edge did not drop `speaker`.
#[test]
fn only_the_bound_ingress_names_a_speaker_and_only_a_close_drops_it() {
    let all = shipped_edges();
    // The floor catches a degenerate sweep; the tree carries ~900 edges.
    assert!(
        all.len() > 300,
        "the sweep must see the library: {}",
        all.len()
    );

    // GH #979: one shipped edge writes the key, and it mints nothing: the
    // firewall's warden hands a RELEASED turn back the speaker its own ingress
    // stamped (`hop.ctx_speaker`, carried out of the parked row by the warden
    // itself) -- or none -- instead of the speaker of whoever released it.
    let setters: Vec<String> = all
        .iter()
        .filter(|(_, e)| e["modifier"]["set_context"].get("speaker").is_some())
        .map(|(f, e)| {
            format!(
                "{f}: {} -> {} = {}",
                e["from"], e["to"], e["modifier"]["set_context"]["speaker"]
            )
        })
        .collect();
    assert_eq!(
        setters,
        vec![
            "firewall/config.json: \"./warden\" -> \".\" = \"has(hop.ctx_speaker) ? hop.ctx_speaker : ''\""
                .to_string()
        ],
        "a shipped edge names a speaker without a sender proof"
    );

    let droppers: Vec<(String, String, String, String)> = all
        .iter()
        .filter(|(_, e)| {
            e["modifier"]["delete_context"]
                .as_array()
                .is_some_and(|l| l.contains(&json!("speaker")))
        })
        .map(|(f, e)| {
            (
                f.clone(),
                e["from"].as_str().unwrap_or_default().to_string(),
                e["to"].as_str().unwrap_or_default().to_string(),
                e["condition"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert_eq!(
        droppers,
        vec![(
            "session-keeper/config.json".to_string(),
            "./stamp".to_string(),
            ".".to_string(),
            "has(hop.route) && hop.route == 'close'".to_string(),
        )],
        "the speaker is dropped by exactly the keeper's close"
    );
}

// ══════════════════════════════════════════════════════════════ the colony

/// The bound chats and the sender, as the colony's `.env` carries them.
const OWN_CHAT: i64 = 111;
const GROUP_CHAT: i64 = -1_001_234;
const OWN_USER: i64 = 111;
const OTHER_USER: i64 = 222;

/// Puts one wake on the wire of the named connector, carrying the chat and the
/// sender the outside world would have written from.
const DRIVER: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
header = {"route": "in_wire", "wire_node": hop.get("wire_node"),
          "wire_chat": hop.get("wire_chat")}
if hop.get("wire_user") is not None:
    header["wire_user"] = hop.get("wire_user")
sys.stdout.write(json.dumps({"header": header,
                             "messages": doc["body"].get("messages", [])}))
"#;

/// The connector, doubled: every wake becomes an inbound message stamped with
/// the chat and — when there is one — the sender, exactly as a connector
/// stamps them, numbers as numbers. It knows nothing about bindings or
/// speakers; that is the edge's job.
const CONNECTOR: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
header = {"chat_id": hop.get("wire_chat")}
if hop.get("wire_user") is not None:
    header["user_id"] = hop.get("wire_user")
sys.stdout.write(json.dumps({"header": header,
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

/// Two connectors in the member's `channels` container — the member's own
/// one-to-one chat and a group — a driver outside, and the root's one way out
/// of the container: a turn goes to the brain. The container's own graph is
/// EMPTY: every edge between a connector and its container is the recipe's,
/// applied below. The connectors are booted from the tree, not grown, for the
/// reason `gh940` gives (a recipe-grown channel is born asleep).
fn build_tree(root: &std::path::Path) {
    let to = |node: &str| {
        json!({"from": "./driver", "to": format!("./channels/{node}"),
               "condition": format!("has(hop.route) && hop.route == 'in_wire' && \
                                     has(hop.wire_node) && hop.wire_node == '{node}'")})
    };
    write(
        root,
        "main/config.json",
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
            to("own"),
            to("group"),
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
        &double(DRIVER, "Test driver: puts one wake on a connector's wire."),
    );
    for node in ["own", "group"] {
        write(
            root,
            &format!("main/channels/{node}/config.json"),
            &double(CONNECTOR, "Test double for the connector of one channel."),
        );
    }
    // The bindings live in the colony's environment, where the operator keeps
    // them; the manifests name the variables.
    std::fs::write(
        root.join(".env"),
        format!("TG_CHAT={OWN_CHAT}\nTG_GROUP={GROUP_CHAT}\nTG_USER={OWN_USER}\n"),
    )
    .expect("write the .env");
}

/// The recipe's declaration for one channel, standing at the container this
/// colony has: the scope moves to `/channels` and the node, which the tree
/// already holds, leaves the diff. Every edge is the recipe's, byte for byte.
fn rendered_edges(node: &str, chat: &str, user: Option<&str>) -> Value {
    let mut params = wish(TELEGRAM, Some(chat), user);
    params["name"] = json!(node);
    let mut decl = declaration("grow_level", params);
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
    // The recorder stands for the member's brain.
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
        .expect("the container, the connectors and the driver must boot");
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

/// One wake for `node`, from `chat`, by `user` (or by nobody).
fn wake(node: &str, chat: i64, user: Option<i64>) -> Message {
    let mut hop = Map::new();
    hop.insert("wire_node".into(), json!(node));
    hop.insert("wire_chat".into(), json!(chat));
    if let Some(u) = user {
        hop.insert("wire_user".into(), json!(u));
    }
    MessageBuilder::new(Path::new("/driver"))
        .body(Body::Inline(json!({"messages": [
            {"origin": "user", "type": "text", "text": "hello"}
        ]})))
        .hop(hop)
        .ttl(64)
        .build()
}

/// Generous on purpose: a `code` double starts a python, and a loaded machine
/// is not a defect of the edge under test.
const RECEIPT_TIMEOUT: Duration = Duration::from_secs(60);

/// Send one wake and read what the brain hears of it. One at a time: the
/// brain's mailbox is in order, so the next wake is sent only after this one
/// arrived, and nothing depends on a time window.
async fn heard(
    h: &ColonyHandle,
    brain: &mut mpsc::Receiver<Message>,
    node: &str,
    chat: i64,
    user: Option<i64>,
) -> Headers {
    let m = wake(node, chat, user);
    let trace = m.trace_id;
    h.send(m).await;
    let got = tokio::time::timeout(RECEIPT_TIMEOUT, brain.recv())
        .await
        .unwrap_or_else(|_| panic!("the brain went quiet — {node}/{chat}/{user:?} never arrived"))
        .expect("the brain's channel closed");
    assert_eq!(
        got.trace_id, trace,
        "the brain heard another turn than {node}/{chat}/{user:?}"
    );
    got.headers
}

/// The lock on a booted colony. Two channels, both rendered by the recipe with
/// their bindings named as environment variables and bound from the `.env` by
/// the mutation door: the member's own chat without `bind_user` (the chat id
/// is the proof) and a group with `bind_user`.
///
/// Read at the recorder for the brain: the proven sender is named
/// `member:alex`, everybody else in a bound chat speaks in the round and is
/// named by nobody. Red before GH #949: no turn carried a `speaker`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bound_chat_names_its_speaker_at_the_brain() {
    let td = tempfile::TempDir::new().expect("a temporary directory");
    build_tree(td.path());
    let (h, mut brain) = boot(&td).await;

    for decl in [
        rendered_edges("own", "${TG_CHAT}", None),
        rendered_edges("group", "${TG_GROUP}", Some("${TG_USER}")),
    ] {
        let outcome = mutate(&h, decl.clone()).await;
        assert!(
            matches!(outcome, MutationOutcome::Committed { .. }),
            "the recipe's edges, bound from the .env, were not committed: \
             {outcome:?}\n{decl}"
        );
    }

    let ctx_str = |hd: &Headers, k: &str| -> Option<String> {
        hd.context
            .get(k)
            .and_then(Value::as_str)
            .map(str::to_string)
    };

    // (a) the member in their own chat.
    let own = heard(&h, &mut brain, "own", OWN_CHAT, Some(OWN_USER)).await;
    assert_eq!(
        ctx_str(&own, "speaker").as_deref(),
        Some(SPEAKER),
        "{:?}",
        own.context
    );
    assert_eq!(ctx_str(&own, "audience_set").as_deref(), Some(ROUND));

    // (b) the bound chat, another sender: the round as today, nobody named.
    let other = heard(&h, &mut brain, "own", OWN_CHAT, Some(OTHER_USER)).await;
    assert_eq!(ctx_str(&other, "audience_set").as_deref(), Some(ROUND));
    assert_eq!(
        ctx_str(&other, "speaker").unwrap_or_default(),
        "",
        "{:?}",
        other.context
    );

    // (c) no sender on the hop at all.
    let none = heard(&h, &mut brain, "own", OWN_CHAT, None).await;
    assert_eq!(ctx_str(&none, "audience_set").as_deref(), Some(ROUND));
    assert_eq!(
        ctx_str(&none, "speaker").unwrap_or_default(),
        "",
        "{:?}",
        none.context
    );

    // (d) a group, the named sender and another member of it.
    let named = heard(&h, &mut brain, "group", GROUP_CHAT, Some(OWN_USER)).await;
    assert_eq!(
        ctx_str(&named, "speaker").as_deref(),
        Some(SPEAKER),
        "{:?}",
        named.context
    );
    assert_eq!(ctx_str(&named, "audience_set").as_deref(), Some(ROUND));
    let member = heard(&h, &mut brain, "group", GROUP_CHAT, Some(OTHER_USER)).await;
    assert_eq!(
        ctx_str(&member, "speaker").unwrap_or_default(),
        "",
        "{:?}",
        member.context
    );

    h.shutdown().await;
}

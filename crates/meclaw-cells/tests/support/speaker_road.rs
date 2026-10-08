//! The road of one turn from a channel to the object tools, booted -- for the
//! locks of GH #979 (`context.speaker` only from a proven channel identity).
//!
//! What GH #949 measured at a recorder standing for the brain, this measures
//! one storey further, at the receivers that DECIDE on the speaker: every edge
//! between them is the SHIPPED one, every hive is the shipped template.
//!
//! ```text
//! driver -> channels/<node> (connector double)
//!        -- the recipe's bound ingress (grow_level, applied through the door)
//!        -> channels
//!        -- member: ./channels -> ./firewall      (verbatim)
//!        -> firewall  (templates/firewall: screen, rules, warden, porter)
//!        -- member: ./firewall -> ./assistants    (verbatim)  + tap -> /turns
//!        -> assistants (a double standing for the brain: it places ONE tool
//!                       call the turn's text names, and hands the answer on)
//!        -- member: ./assistants -> ./objects     (verbatim)
//!        -> objects   (templates/objects, its store seeded with one candidate)
//!        -- member: ./objects -> ./assistants     (verbatim)
//!        -> assistants -> /ear
//! ```
//!
//! The phone way is the edge `templates/freeswitch/README.md` documents (the
//! recipe grows no telephone); the lock reads that README so the two cannot
//! drift.
//!
//! No model and no network: `code` doubles, `store` cells, capture cells.
#![allow(dead_code)]

use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
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

pub const MEMBER: &str = "/os/orgs/acme/members/alex";
pub const AGENT: &str = "scribe";
pub const PERSON: &str = "alex";
/// The round the ingress stamps, as a holder reads it.
pub const ROUND: &str = r#"["agent:scribe","member:alex"]"#;
/// The speaker, as the member's entry in that round spells it.
pub const SPEAKER: &str = "member:alex";
pub const TELEGRAM: &str = "telegram-connector@2.2.0";

/// The member's one-to-one chat (the chat id IS the sender's id).
pub const OWN_CHAT: i64 = 111;
/// A group bound with `bind_user`: the member writes there as `GROUP_USER`.
pub const GROUP_CHAT: i64 = -1_001_234;
pub const GROUP_USER: i64 = 444;
/// Somebody else, writing into the member's bound chat.
pub const STRANGER: i64 = 222;
/// A sender the firewall parks for a person (`hold` row).
pub const HELD: i64 = 333;
/// A chat nobody bound.
pub const UNBOUND_CHAT: i64 = 999;
/// The member's identity on the telephone, as the switch stamps it.
pub const LINE_USER: &str = "alex-line";

/// The candidate the objects store is born with, in the member's round.
pub const OBJECT: &str = "ob-0123456789ab";

pub fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// R2b / GH #49: a tree without the templates this road is made of skips.
pub fn shipped() -> bool {
    [
        "templates/builder/recipes/config.json",
        "templates/member/config.json",
        "templates/firewall/config.json",
        "templates/objects/config.json",
        "templates/freeswitch/README.md",
    ]
    .iter()
    .all(|p| repo(p).is_file())
}

pub fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

// ══════════════════════════════════════════════════════════════ the recipe

pub fn run_recipes(recipe: &str, params: Value) -> Vec<Value> {
    emit_all(
        &shipped_script(
            repo("templates/builder/recipes/config.json")
                .to_str()
                .expect("a utf-8 path"),
        ),
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

pub fn declaration(recipe: &str, params: Value) -> Value {
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

/// A channel wish for `template` in the member, named `name`.
pub fn channel_wish(name: &str, template: &str, chat: Option<&str>, user: Option<&str>) -> Value {
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

/// The recipe's edges for one channel, standing at `/channels` of this
/// colony: the node, which the tree already holds, leaves the diff. Every
/// edge is the recipe's, byte for byte.
pub fn rendered_channel(wish: Value) -> Value {
    let mut decl = declaration("grow_level", wish);
    let edges = decl["diff"]["add_edges"].clone();
    decl["scope"] = json!("/channels");
    decl["diff"] = json!({"add_edges": edges});
    decl
}

// ══════════════════════════════════════════════════════════════ the member

/// The ONE shipped member edge `from -> to` whose condition contains `cond`.
pub fn member_edge(from: &str, to: &str, cond: &str) -> Value {
    let cfg = read_json(&repo("templates/member/config.json"));
    let hits: Vec<Value> = cfg["params"]["graph"]["edges"]
        .as_array()
        .expect("member edges")
        .iter()
        .filter(|e| {
            e["from"] == json!(from)
                && e["to"] == json!(to)
                && e["condition"].as_str().is_some_and(|c| c.contains(cond))
        })
        .cloned()
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "member edge {from} -> {to} on {cond}: {hits:#?}"
    );
    hits[0].clone()
}

/// The ingress of a telephone, as `templates/freeswitch/README.md`
/// § *Wiring it into a member* writes it -- with the member and the person's
/// sender id filled in. The lock asserts that the README carries the very
/// `speaker` line below, so the documentation is what is booted.
pub fn phone_ingress() -> Value {
    json!({"from": "./channels/phone", "to": "./channels",
           "condition": "has(hop.route) && (hop.route == 'turn' || hop.route == 'partial' || hop.route == 'error')",
           "modifier": {"set_hop": {"route": "'turn'"},
                        "set_context": {"channel_node": "'phone'",
                                        "channel": "'phone'",
                                        "assistant": format!("'{AGENT}'"),
                                        "audience_set": format!("'[\"agent:{AGENT}\",\"member:{PERSON}\"]'"),
                                        "user_id": format!("has(hop.user_id) && hop.user_id != '' ? hop.user_id : '{LINE_USER}'"),
                                        "speaker": phone_speaker(LINE_USER, PERSON)}}})
}

/// The speaker expression of the telephone ingress: only the identity the
/// switch stamped (`hop.verified_user`, OR-NL-164) -- never `hop.user_id`,
/// which may be the `callers` fallback or a dialled number, and never the
/// edge's own literal fallback.
pub fn phone_speaker(user: &str, person: &str) -> String {
    format!(
        "has(hop.verified_user) && string(hop.verified_user) == '{user}' ? 'member:{person}' : ''"
    )
}

// ══════════════════════════════════════════════════════════════ the doubles

/// Puts one wake on the wire of the named connector.
const DRIVER: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
header = {"route": "in_wire"}
for k in ("wire_node", "wire_chat", "wire_user", "wire_extra"):
    if hop.get(k) is not None:
        header[k] = hop.get(k)
sys.stdout.write(json.dumps({"header": header,
                             "messages": doc["body"].get("messages", [])}))
"#;

/// A chat connector: chat and sender as the network stamps them, numbers as
/// numbers, plus whatever extra hop keys the outside world put on the wire
/// (a forged `speaker` among them). It knows nothing about bindings.
const CONNECTOR: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
header = {"chat_id": hop.get("wire_chat")}
if hop.get("wire_user") is not None:
    header["user_id"] = hop.get("wire_user")
header.update(hop.get("wire_extra") or {})
sys.stdout.write(json.dumps({"header": header,
                             "messages": doc["body"].get("messages", [])}))
"#;

/// The telephone, doubled: a turn carries the sender (`hop.user_id`) and,
/// where the switch verified it, the proof (`hop.verified_user`, on
/// `wire_extra`) -- `templates/freeswitch/README.md` § *Who is on the line*.
const PHONE: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
header = {"route": "turn", "session_id": "call-1", "call_id": "call-1", "call_state": "live"}
if hop.get("wire_user") is not None:
    header["user_id"] = hop.get("wire_user")
header.update(hop.get("wire_extra") or {})
sys.stdout.write(json.dumps({"header": header,
                             "messages": doc["body"].get("messages", [])}))
"#;

/// Stands for the brain. A turn whose text reads `TOOL <name> <json>` places
/// that one tool call; the answer to it is handed on as `heard`, its refusal
/// code beside it.
const BRAIN: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
msgs = doc["body"].get("messages", []) or []
route = hop.get("route")
if route == "in_turn":
    text = str((msgs[0] if msgs else {}).get("text") or "")
    if not text.startswith("TOOL "):
        sys.stdout.write("[]")
        sys.exit(0)
    name, args = text[5:].split(" ", 1)
    call = "c-" + name
    sys.stdout.write(json.dumps({"header": {"route": "tool", "tool_name": name,
                                            "tool_call_id": call},
                                 "messages": [{"origin": "assistant", "type": "tool_call",
                                               "id": call, "text": args}]}))
elif route == "in_tool":
    sys.stdout.write(json.dumps({"header": {"route": "heard",
                                            "refused": str(hop.get("error_code") or ""),
                                            "tool_call_id": str(hop.get("tool_call_id") or "")},
                                 "messages": msgs}))
else:
    sys.stdout.write("[]")
"#;

pub fn double(script: &str, purpose: &str) -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": script,
                   "external_timeout_ms": 10000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "emits": {"body": {"messages": {"type": "array", "required": false}}},
            "consumes": {"body": {"messages": {"type": "array", "required": false}}},
            "capabilities": ["shell:exec"]
        },
        "description": {"purpose": purpose, "use_when": "Test fixture only.",
                        "not_in_scope": "Not a template."}
    })
}

pub fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().expect("a parent")).expect("mkdir");
    std::fs::write(
        p,
        meclaw_core::serde_json::to_string_pretty(v).expect("json"),
    )
    .expect("write");
}

pub fn copy_template(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).expect("mkdir");
    for entry in std::fs::read_dir(src).expect("read_dir") {
        let from = entry.expect("entry").path();
        let name = from.file_name().expect("a name").to_owned();
        if from.is_dir() {
            copy_template(&from, &dst.join(&name));
        } else {
            std::fs::copy(&from, dst.join(&name)).expect("copy");
        }
    }
}

fn rules_seed() -> String {
    let schema = r#"{"schema": {"rule_id": "text", "kind": "text", "field": "text", "value": "text", "action": "text", "enabled": "int", "note": "text"}}"#;
    let mut out = String::from(schema);
    for who in [HELD, GROUP_USER] {
        out.push('\n');
        out.push_str(
            &json!({"rule_id": format!("hold-{who}"), "kind": "sender", "field": "user_id",
                    "value": who.to_string(), "action": "hold", "enabled": 1,
                    "note": "GH #979 fixture"})
            .to_string(),
        );
    }
    out.push('\n');
    out
}

fn objects_seed() -> String {
    let mut out = String::from(
        r#"{"schema": {"id": "text", "rev": "int", "type": "text", "aliases": "text", "state": "text", "origin": "text", "valid_since": "text", "audience_set": "text", "slots": "text", "refs": "text", "seen": "int", "turns": "text", "supersedes": "int", "recorded_at": "text"}}"#,
    );
    let at = "2026-10-01T00:00:00.000000Z";
    out.push('\n');
    out.push_str(
        &json!({"id": OBJECT, "rev": 1, "type": "thing",
                "aliases": json!(["Blue Bike"]).to_string(),
                "state": "candidate", "origin": "thing_seen", "valid_since": at,
                "audience_set": ROUND,
                "slots": json!({"what": "", "where": "", "who": [], "when": "",
                                "why": "", "how": ""}).to_string(),
                "refs": json!({"doc": [], "related": []}).to_string(),
                "seen": 1, "turns": json!(["t0"]).to_string(), "supersedes": 0,
                "recorded_at": at})
        .to_string(),
    );
    out.push('\n');
    out
}

/// The tree of the road (see the module doc). The channel container's own
/// graph holds only the telephone's documented ingress; the chat channels'
/// edges are the recipe's, applied by [`Road::bind_chats`].
pub fn build_tree(root: &std::path::Path) {
    let to = |node: &str| {
        json!({"from": "./driver", "to": format!("./channels/{node}"),
               "condition": format!("has(hop.route) && hop.route == 'in_wire' && \
                                     has(hop.wire_node) && hop.wire_node == '{node}'")})
    };
    let pass = member_edge("./firewall", "./assistants", "hop.route == 'pass'");
    let mut tap = pass.clone();
    tap["to"] = json!("./turns");
    let exits = "has(hop.route) && (hop.route == 'reject' || hop.route == 'hold')";
    write(
        root,
        "main/config.json",
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
            to("own"), to("group"), to("unbound"), to("phone"),
            // The telephone's documented ingress is NOT drawn here: it sets
            // `speaker`, which only the wiring may stamp (GH #979,
            // `STAMPED_CONTEXT_KEYS`), so [`Road::boot`] lays it through the
            // door at the level that holds `channels`, as a member does.
            member_edge("./channels", "./firewall", "hop.route == 'turn'"),
            pass,
            tap,
            {"from": "./firewall", "to": "./park", "condition": exits},
            member_edge("./assistants", "./objects", "hop.tool_name.startsWith('object_')"),
            member_edge("./objects", "./assistants", "hop.route == 'tool_result'"),
            {"from": "./assistants", "to": "./ear",
             "condition": "has(hop.route) && hop.route == 'heard'"}
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
        &double(DRIVER, "Test driver."),
    );
    for node in ["own", "group", "unbound"] {
        write(
            root,
            &format!("main/channels/{node}/config.json"),
            &double(CONNECTOR, "Test double for a chat connector."),
        );
    }
    write(
        root,
        "main/channels/phone/config.json",
        &double(PHONE, "Test double for the two halves of a telephone."),
    );
    write(
        root,
        "main/assistants/config.json",
        &double(BRAIN, "Test double standing for the brain."),
    );
    copy_template(&repo("templates/firewall"), &root.join("main/firewall"));
    std::fs::write(
        root.join("main/firewall/rules/seed/rules.jsonl"),
        rules_seed(),
    )
    .expect("rules seed");
    copy_template(&repo("templates/objects"), &root.join("main/objects"));
    std::fs::create_dir_all(root.join("main/objects/store/seed")).expect("mkdir");
    std::fs::write(
        root.join("main/objects/store/seed/objects.jsonl"),
        objects_seed(),
    )
    .expect("objects seed");
    std::fs::write(
        root.join(".env"),
        format!("TG_CHAT={OWN_CHAT}\nTG_GROUP={GROUP_CHAT}\nTG_USER={GROUP_USER}\n"),
    )
    .expect("the .env");
}

// ══════════════════════════════════════════════════════════════ the colony

/// Generous on purpose: every hop of the road starts a python.
pub const RECEIPT_TIMEOUT: Duration = Duration::from_secs(90);

pub struct Road {
    pub h: ColonyHandle,
    /// The turn as the brain is handed it (tap beside the member's pass edge).
    pub turns: mpsc::Receiver<Message>,
    /// The answer of the object tools, as the brain handed it on.
    pub ear: mpsc::Receiver<Message>,
    /// `reject` and `hold` out of the firewall.
    pub park: mpsc::Receiver<Message>,
    _td: tempfile::TempDir,
}

fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![
        (
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        ),
        ("store".to_string(), Arc::new(StoreCellFactory)),
    ]
}

/// One mutation through the door of `h`, answered.
pub async fn mutate(h: &ColonyHandle, payload: Value) -> MutationOutcome {
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

impl Road {
    /// Boots the tree and binds the two chats through the mutation door,
    /// from the `.env`: the member's own chat (the chat id is the proof) and
    /// a group with `bind_user`. The `unbound` connector stays unbound.
    pub async fn boot() -> Road {
        let td = tempfile::TempDir::new().expect("a temporary directory");
        build_tree(td.path());
        let h = ColonyHandle::new_with_factories_at(&td, factories());
        let mut rx = Vec::new();
        for at in ["/turns", "/ear", "/park"] {
            let (tx, r) = mpsc::channel::<Message>(64);
            h.spawn(Path::new(at), move || CaptureCell::new(tx.clone()))
                .await;
            rx.push(r);
        }
        let mut registry = CellFactoryRegistry::new();
        for (name, f) in factories() {
            registry.insert(name, f);
        }
        bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
            .await
            .expect("the road must boot");
        let park = rx.pop().expect("park");
        let ear = rx.pop().expect("ear");
        let turns = rx.pop().expect("turns");
        let road = Road {
            h,
            turns,
            ear,
            park,
            _td: td,
        };
        for decl in [
            json!({"scope": "/", "diff": {"add_edges": [phone_ingress()]}}),
            rendered_channel(channel_wish("own", TELEGRAM, Some("${TG_CHAT}"), None)),
            rendered_channel(channel_wish(
                "group",
                TELEGRAM,
                Some("${TG_GROUP}"),
                Some("${TG_USER}"),
            )),
        ] {
            let outcome = road.mutate(decl.clone()).await;
            assert!(
                matches!(outcome, MutationOutcome::Committed { .. }),
                "the recipe's edges were not committed: {outcome:?}\n{decl}"
            );
        }
        road
    }

    pub async fn mutate(&self, payload: Value) -> MutationOutcome {
        mutate(&self.h, payload).await
    }

    /// One wake on `node`'s wire; returns its trace id.
    pub async fn say(
        &self,
        node: &str,
        chat: Option<i64>,
        user: Option<Value>,
        extra: Option<Value>,
        text: &str,
    ) -> Uuid {
        let mut hop = Map::new();
        hop.insert("wire_node".into(), json!(node));
        if let Some(c) = chat {
            hop.insert("wire_chat".into(), json!(c));
        }
        if let Some(u) = user {
            hop.insert("wire_user".into(), u);
        }
        if let Some(x) = extra {
            hop.insert("wire_extra".into(), x);
        }
        let m = MessageBuilder::new(Path::new("/driver"))
            .body(Body::Inline(json!({"messages": [
                {"origin": "user", "type": "text", "text": text}
            ]})))
            .hop(hop)
            .ttl(64)
            .build();
        let trace = m.trace_id;
        self.h.send(m).await;
        trace
    }

    /// A person's answer about a parked turn, sent from a chain whose context
    /// is `context` (the releaser's own turn, say).
    pub async fn release(&self, hold_id: &str, context: Value) {
        let mut hop = Map::new();
        hop.insert("route".into(), json!("in_release"));
        hop.insert("hold_id".into(), json!(hold_id));
        hop.insert("decision".into(), json!("release"));
        hop.insert("decided_by".into(), json!("member:alex"));
        let mut b = MessageBuilder::new(Path::new("/firewall"))
            .body(Body::Inline(json!({"messages": []})))
            .ttl(64);
        b = b.headers(Headers::from_parts(
            context.as_object().cloned().unwrap_or_default(),
            hop,
        ));
        self.h.send(b.build()).await;
    }

    pub async fn shutdown(self) {
        self.h.shutdown().await;
    }
}

pub async fn next(rx: &mut mpsc::Receiver<Message>, what: &str) -> Message {
    tokio::time::timeout(RECEIPT_TIMEOUT, rx.recv())
        .await
        .unwrap_or_else(|_| panic!("{what}: nothing arrived"))
        .unwrap_or_else(|| panic!("{what}: the channel closed"))
}

pub fn ctx(m: &Message, k: &str) -> Option<String> {
    m.headers.context.get(k).map(|v| {
        v.as_str()
            .map(str::to_string)
            .unwrap_or_else(|| v.to_string())
    })
}

pub fn hop(m: &Message, k: &str) -> Option<String> {
    m.headers.hop.get(k).map(|v| {
        v.as_str()
            .map(str::to_string)
            .unwrap_or_else(|| v.to_string())
    })
}

/// "No member is named": the key absent or empty -- the two spellings every
/// reader of the speaker takes alike (`objects/tools` `is_owner`, the memory's
/// writer `or ""`); a modifier can only SET a value, so an ingress without a
/// proof writes the empty one, which also overwrites a carried-in claim.
pub fn names_nobody(m: &Message) -> bool {
    ctx(m, "speaker").unwrap_or_default().is_empty()
}

/// The text of the one tool answer the brain handed on, parsed.
pub fn answer_of(m: &Message) -> Value {
    let body = match &m.body {
        Body::Inline(v) => v.clone(),
        Body::Blob(_) => panic!("inline expected"),
    };
    let text = body["messages"][0]["text"]
        .as_str()
        .expect("an answer text");
    meclaw_core::serde_json::from_str(text).expect("the answer is JSON")
}

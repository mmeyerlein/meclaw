//! GH #522 — a chat is a generation, a node is an address, and one word was
//! doing both jobs.
//!
//! `templates/builder/recipes`, `_channel_level`, promoted the NODE name into
//! `context.channel`. `templates/session-keeper/README.md` describes that same
//! key as the chat identity — *"a Telegram/Slack `hop.chat_id`, a room, a phone
//! number — whatever a surface calls the same conversation partner"* — and a
//! session is keyed on it. On a grown topology every chat of one connector
//! therefore shared ONE generation: the idle clock, the nightly close and the
//! session id were computed over the union of all of them, the firewall
//! rate-limited them as one bucket, and the channel-local clause of the
//! audience gate could never match a row recorded under a chat id.
//!
//! The obvious repair was a regression, which is why #517 refused to make it in
//! passing. The rendered set has three edges and the third is the answer's way
//! back — `. -> ./telegram` — and `Edge.to` is a static path in this substrate,
//! so it has to say WHICH child of the container it is for (the address rule,
//! GH #454). A `channel` carrying a chat id routes no answer anywhere and the
//! agent goes mute on the surface it was reached on.
//!
//! So the two meanings are two keys. `context.channel_node` is the ADDRESS and
//! the edges read it; `context.channel` is the CHAT and the holders read it.
//! The data decided which way round: every `channel` column any colony ever
//! wrote — `sessions`, `episodes`, `facts`, `entity_edges`, the firewall's
//! `arrivals` — carries a chat id, because the hand-drawn e9-era edge promoted
//! `has(hop.chat_id) ? hop.chat_id : ''` and its answer edge carried no guard
//! at all (one connector in the hive). A colony that imports that history and
//! then writes the node name into the same column has two referents in one
//! column and no way to tell them apart.
//!
//! A SCREEN puts the same word in both, and that is not a special case: a
//! screen is one room, so its address and its conversation partner coincide.

use meclaw_colony::cel_eval::{
    apply_modifier, evaluate_condition, parse_condition, parse_modifier,
};
use meclaw_colony::config::ModifierSpec;
use meclaw_core::Headers;
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_testing::{emit_all, emit_one, shipped_script};
use std::path::PathBuf;

const RECIPES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/recipes/config.json"
);
const STAMP: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/session-keeper/stamp/config.json"
);

const MEMBER: &str = "/os/orgs/mm/members/egon";
const NODE: &str = "telegram";
/// Marcus' own chat with the live colony, and the value every imported row of
/// its history carries in its `channel` column.
const CHAT_A: &str = "300850023";
const CHAT_B: &str = "44117";

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../")
        .join(rel)
}

fn read(rel: &str) -> Option<String> {
    std::fs::read_to_string(repo(rel)).ok()
}

fn grow(params: Value) -> Vec<Value> {
    let out = emit_one(
        &shipped_script(RECIPES),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!({"recipe": "grow_level", "request": "…",
                                         "params": params}).to_string()}],
        }),
    );
    out["manifest"]
        .as_array()
        .unwrap_or_else(|| panic!("no manifest: {out}"))
        .clone()
}

fn channel_declaration() -> Value {
    grow(json!({"scope": MEMBER, "level": "channel", "name": NODE,
                "template": "telegram-connector@2.0.1", "assistant": "egon",
                "ctx": {"member_person": "marcus"}}))[0]
        .clone()
}

fn edges(decl: &Value) -> Vec<Value> {
    decl["diff"]["add_edges"]
        .as_array()
        .expect("add_edges")
        .clone()
}

/// The edge that raises a turn: `./<node> -> .` on `!has(hop.error_code)`.
fn ingress(decl: &Value) -> Value {
    edges(decl)
        .into_iter()
        .find(|e| e["condition"] == json!("!has(hop.error_code)"))
        .expect("the ingress edge")
}

/// The answer's way back: `. -> ./<node>`.
fn way_back(decl: &Value) -> Value {
    edges(decl)
        .into_iter()
        .find(|e| e["from"] == json!(".") && e["to"] == json!("./telegram"))
        .expect("the answer's way back")
}

fn map(v: &Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

/// Run a rendered edge's modifier through the REAL evaluator the substrate
/// uses. Asserting on the CEL source would only prove what the string says.
fn traverse(edge: &Value, headers: &Headers) -> Headers {
    let spec: ModifierSpec =
        meclaw_core::serde_json::from_value(edge["modifier"].clone()).expect("modifier spec");
    let compiled = parse_modifier(&spec).expect("the modifier compiles");
    apply_modifier(&compiled, headers).expect("the modifier evaluates")
}

/// Would this edge take that message? `Err` is the substrate's skip, so it is
/// `false` here for the same reason it is there.
fn fires(edge: &Value, headers: &Headers) -> bool {
    let cond = edge["condition"].as_str().expect("a condition");
    let compiled = parse_condition(cond).expect("the condition compiles");
    evaluate_condition(&compiled, &headers.context, &headers.hop).unwrap_or(false)
}

/// One inbound Telegram message as the connector emits it: everything on the
/// hop, nothing in context yet.
fn inbound(chat: &str) -> Headers {
    Headers::from_parts(
        Map::new(),
        map(
            &json!({"chat_id": chat, "user_id": chat, "platform": "telegram",
                    "message_id": "m1"}),
        ),
    )
}

fn ctx_str(h: &Headers, key: &str) -> String {
    h.context
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

// ════════════════════════════════════════════ the two keys, at the renderer

/// The ingress edge promotes both, and they are not the same expression.
#[test]
fn the_ingress_edge_promotes_the_node_and_the_chat_apart() {
    let set = ingress(&channel_declaration())["modifier"]["set_context"].clone();
    assert_eq!(
        set["channel_node"],
        json!("'telegram'"),
        "the ADDRESS is the node name, and it is a literal because a CEL guard \
         cannot read a node's params"
    );
    assert_eq!(
        set["channel"],
        json!("has(hop.chat_id) ? hop.chat_id : ''"),
        "the CHAT is what the surface calls the same conversation partner. As \
         the node name it gave every chat of this connector ONE session \
         generation, ONE rate bucket and ONE memory room"
    );
    assert_ne!(
        set["channel_node"], set["channel"],
        "one word for two jobs is the defect this issue is about"
    );
    // The guarded form is the member-level rule, and it is what keeps the
    // connector's own failure emissions from taking the edge down with them.
    assert_eq!(set["chat_id"], json!("has(hop.chat_id) ? hop.chat_id : ''"));
}

/// A screen and an app carry the same word in both — and still route on the
/// node. The rule holds for every kind of channel or it is not a rule.
#[test]
fn a_screen_and_an_app_carry_the_same_word_in_both() {
    let screen = grow(
        json!({"scope": MEMBER, "level": "screen", "name": "display-desk",
                             "template": "display@1.0.0"}),
    )[0]
    .clone();
    let up = edges(&screen)
        .into_iter()
        .find(|e| e["from"] == json!("./display-desk"))
        .expect("the screen's way up");
    let set = &up["modifier"]["set_context"];
    assert_eq!(set["channel_node"], json!("'display-desk'"));
    assert_eq!(
        set["channel"],
        json!("'display-desk'"),
        "a screen IS one room, so its address and its conversation partner are \
         the same word — that is why it needs no chat id and is not an exception"
    );
    let down = edges(&screen)
        .into_iter()
        .find(|e| e["to"] == json!("./display-desk"))
        .expect("the screen's way down");
    let guard = down["condition"].as_str().expect("a condition");
    assert!(
        guard.contains("context.channel_node == 'display-desk'"),
        "a view is addressed by the node like every other answer: {guard:?}"
    );

    let app = grow(
        json!({"scope": MEMBER, "level": "app", "name": "colony-view",
                          "template": "colony-view@1.0.0", "screen": "display-desk"}),
    )[0]
    .clone();
    let out = edges(&app)
        .into_iter()
        .find(|e| e["from"] == json!("./colony-view"))
        .expect("the app's way up");
    let set = &out["modifier"]["set_context"];
    assert_eq!(
        set["channel_node"],
        json!("'display-desk'"),
        "an app is display-blind: the screen it draws on is a literal in the \
         edge that LEAVES it, and it has to be the addressing key or the view \
         reaches no screen"
    );
    assert_eq!(set["channel"], json!("'display-desk'"));
}

// ═══════════════════════════════════ two chats, two generations, on the wire

/// Two messages from two chats of ONE connector, put through the rendered edge
/// with the real evaluator and then through the SHIPPED session keeper: two
/// lookups, two rows, two ids. Before the split both carried `telegram` and the
/// keeper could not tell the two conversations apart at all.
#[test]
fn two_chats_of_one_connector_open_two_generations() {
    let ingress = ingress(&channel_declaration());
    let a = traverse(&ingress, &inbound(CHAT_A));
    let b = traverse(&ingress, &inbound(CHAT_B));

    assert_eq!(ctx_str(&a, "channel"), CHAT_A);
    assert_eq!(ctx_str(&b, "channel"), CHAT_B);
    assert_eq!(
        ctx_str(&a, "channel_node"),
        ctx_str(&b, "channel_node"),
        "both chats came in through the same node, and that is the point: the \
         address is shared and the conversation is not"
    );

    // The keeper's ingress pass, twice, on the shipped script.
    let look_a = keeper_lookup(&a);
    let look_b = keeper_lookup(&b);
    assert_eq!(look_a["where"]["channel"], json!(CHAT_A));
    assert_eq!(look_b["where"]["channel"], json!(CHAT_B));
    assert_ne!(
        look_a["where"]["channel"], look_b["where"]["channel"],
        "one generation for every chat of a bot: the idle clock, the nightly \
         close and the session id would be computed over the union of all of \
         them"
    );

    // Neither chat has an open row, so each opens its own generation.
    let open_a = keeper_open(&a);
    let open_b = keeper_open(&b);
    assert_eq!(open_a["row"]["channel"], json!(CHAT_A));
    assert_eq!(open_b["row"]["channel"], json!(CHAT_B));
    let (id_a, id_b) = (
        open_a["row"]["session_id"].as_str().expect("session_id"),
        open_b["row"]["session_id"].as_str().expect("session_id"),
    );
    assert!(id_a.starts_with(&format!("{CHAT_A}-")), "{id_a}");
    assert!(id_b.starts_with(&format!("{CHAT_B}-")), "{id_b}");
    assert_ne!(
        id_a, id_b,
        "two conversations, two session ids — everything downstream hangs off \
         this one word"
    );
}

/// The keeper's `look` op for a turn arriving with these headers.
fn keeper_lookup(h: &Headers) -> Value {
    let out = emit_all(
        &shipped_script(STAMP),
        &json!({
            "target": "/…/session-keeper/stamp",
            "header": {"context": h.context, "hop": {"route": "in_turn"}},
            "ttl": 64,
            "messages": [{"origin": "user", "type": "text", "id": "", "text": "hello"}],
        }),
    );
    store_op(&out, "kstore")
}

/// The keeper's `open` op, one hop later: the store answered the lookup with no
/// row, which is the lazy beginning.
fn keeper_open(h: &Headers) -> Value {
    let mut context = h.context.clone();
    context.insert("ses_phase".into(), json!("look"));
    context.insert("store_origin".into(), json!("keeper-stamp"));
    context.insert(
        "keeper_body".into(),
        json!(
            json!({"messages": [{"origin": "user", "type": "text", "id": "",
                                   "text": "hello"}]})
            .to_string()
        ),
    );
    let out = emit_all(
        &shipped_script(STAMP),
        &json!({
            "target": "/…/session-keeper/stamp",
            "header": {"context": context,
                       "hop": {"operation": "select", "rows_affected": 1}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "x",
                          "text": "[]"}],
        }),
    );
    store_op(&out, "kstore")
}

fn store_op(out: &[Value], route: &str) -> Value {
    let msg = out
        .iter()
        .find(|m| m["header"]["route"] == json!(route))
        .unwrap_or_else(|| panic!("no emission on {route}: {out:?}"));
    let text = msg["messages"][0]["text"].as_str().expect("op text");
    meclaw_core::serde_json::from_str(text).expect("op json")
}

// ═══════════════════════════════════════════ and the answer still gets home

/// The other half, and the reason #517 would not touch this alone: the answer
/// of a chat has to reach the connector that raised it, and a second channel of
/// the same container must not take it.
#[test]
fn the_answer_reaches_the_connector_the_turn_came_from() {
    let decl = channel_declaration();
    let turn = traverse(&ingress(&decl), &inbound(CHAT_A));

    // What comes back down: the generation answered, context rode along.
    let answer = Headers::from_parts(turn.context.clone(), map(&json!({"route": "answer"})));
    assert!(
        fires(&way_back(&decl), &answer),
        "the answer of chat {CHAT_A} did not reach ./telegram — this is the \
         regression the naive repair produces, and the agent goes mute on the \
         surface it was reached on"
    );

    // A SECOND channel in the same container, and its way back must stay shut.
    let other = grow(json!({"scope": MEMBER, "level": "channel", "name": "slack",
                            "template": "telegram-connector@2.0.1", "assistant": "egon",
                            "ctx": {"member_person": "marcus"}}))[0]
        .clone();
    let others_way_back = edges(&other)
        .into_iter()
        .find(|e| e["from"] == json!(".") && e["to"] == json!("./slack"))
        .expect("the second channel's way back");
    assert!(
        !fires(&others_way_back, &answer),
        "a container may hold several channels, and `Edge.to` is static — the \
         guard is what keeps one answer from reaching both"
    );

    // And the chat is still on the message, for the connector to reply into.
    assert_eq!(ctx_str(&answer, "chat_id"), CHAT_A);
    assert_eq!(ctx_str(&answer, "channel"), CHAT_A);
}

/// One storey up, the member decides whether an answer belongs to a channel at
/// all — and it asks the same question with the same word.
#[test]
fn the_member_level_asks_for_the_node_and_not_for_the_chat() {
    let Some(raw) = read("templates/member/config.json") else {
        return; // a tree without the templates cannot make this assertion
    };
    let cfg: Value = meclaw_core::serde_json::from_str(&raw).expect("json");
    let edges = cfg["params"]["graph"]["edges"]
        .as_array()
        .expect("the member's graph");
    let into_channels: Vec<&Value> = edges
        .iter()
        .filter(|e| e["to"] == json!("./channels") && e["condition"].is_string())
        .filter(|e| {
            let c = e["condition"].as_str().unwrap_or_default();
            c.contains("'answer'") || c.contains("'view'")
        })
        .collect();
    assert_eq!(
        into_channels.len(),
        2,
        "the answer and the view are the two lanes that enter the container \
         from a sibling; the member moved and this test did not"
    );
    for e in into_channels {
        let c = e["condition"].as_str().unwrap_or_default();
        assert!(
            c.contains("context.channel_node != ''"),
            "{} -> ./channels still asks whether the CHAT is non-empty \
             ({c:?}). Under GH #522 that is an accident — a chat id happens to \
             be non-empty — and the honest question is whether a channel of \
             this member owns the turn",
            e["from"]
        );
    }

    // The turn of a chat answers it with yes, an operator's turn with no.
    let chat = Headers::from_parts(
        map(&json!({"channel_node": NODE, "channel": CHAT_A})),
        map(&json!({"route": "answer"})),
    );
    let stranger = Headers::from_parts(
        map(&json!({"channel": ""})),
        map(&json!({"route": "answer"})),
    );
    let answer_edge = edges
        .iter()
        .find(|e| {
            e["from"] == json!("./assistants")
                && e["to"] == json!("./channels")
                && e["condition"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("'answer'")
        })
        .expect("./assistants -> ./channels");
    assert!(fires(answer_edge, &chat));
    assert!(
        !fires(answer_edge, &stranger),
        "an answer to a turn that entered at the member's own rim door leaves \
         the level on `answer`; it does not go looking for a channel"
    );
}

// ═════════════════════════════════════════════════════ the published rule

/// The addressing rule lives in prose, in the one place with the most readers.
/// A renderer that splits the key and a README that still names the old one is
/// how the next reader rebuilds the defect by copying the documented form.
#[test]
fn the_address_rule_is_published_where_a_channel_is_wired() {
    let Some(readme) = read("templates/member/README.md") else {
        return;
    };
    assert!(
        readme.contains("The two channel keys"),
        "`templates/member/README.md` publishes the addressing rule and does \
         not carry the section that splits the key"
    );
    assert!(
        readme.contains("context.channel_node == '<name>'"),
        "the table row that tells a mutation how to draw its way back still \
         names the old key"
    );
    let Some(keeper) = read("templates/session-keeper/README.md") else {
        return;
    };
    assert!(
        keeper.contains("channel_node"),
        "the keeper's own README describes `context.channel` as the chat \
         identity — the sentence this defect contradicted — and has to say \
         where the address went, or the next reader takes it for the bug again"
    );
}

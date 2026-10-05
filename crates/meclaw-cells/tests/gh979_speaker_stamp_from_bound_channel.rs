//! GH #979 — `context.speaker` is stamped at the ingress ONLY from a proven
//! channel identity, and the stamp survives the firewall to the brain.
//!
//! # The measurement, before a line was edited
//!
//! A real Telegram turn of a downstream colony carried NO `speaker` on any of
//! its 202 messages, and its `user_id` ended at `firewall/screen`. Read hop by
//! hop in this repository:
//!
//! | hop | `speaker` | why |
//! |---|---|---|
//! | connector → `./channels` | stamped by the bound ingress the recipe renders (`grow_level`, GH #949) | `has(hop.user_id) && string(hop.user_id) == '<proof>' ? 'member:<p>' : ''` |
//! | `./channels` → `./firewall` (member) | unchanged | the edge sets `channel`, `counterpart`, `engine`, `has_file` only |
//! | `./screen` → `pass` (firewall) | **unchanged** | context rides through a cell; the pass edge deletes the firewall's keys AND `user_id` -- by design, the screen consumes it -- never `speaker` |
//! | `./warden` → `pass` (a RELEASED turn) | **the releaser's** | the released turn re-enters on the context of the `in_release` message; the parked turn's own keys come back only as `hop.ctx_<name>`, and no edge read `ctx_speaker` back |
//! | `./firewall` → `./assistants` → a tool (member) | unchanged | no shipped edge on the way deletes it (gh949 sweep) |
//!
//! So the downstream loss is not on this road: that colony draws its own
//! Telegram ingress without the speaker line (its follow-up builds it, the
//! expression is the recipe's). The one loss here is the hold: a stranger's
//! turn released by the member walked on AS the member, and the member's own
//! held turn walked on as nobody. Fixed on the warden's `pass` edge.
//!
//! # The three proofs
//!
//! (a) a bound chat (`bind_chat`, and `bind_user` in a group) -- here, booted;
//! (b) a verified caller -- the identity the switch stamped on the telephone's
//!     SIGNAL turns, and since OR-NL.I.2 on the MEDIA turns of that call too
//!     (`gh979_a_spoken_turn_names_the_verified_caller.rs`); a media turn of a
//!     session nobody named carries no sender and names nobody (below);
//!     (c) an authenticated web session -- the web cell's
//!     `identity_header` (report, patch after the recipe's next owner).
//!
//! Read at the receivers: the tap beside the member's pass edge (the turn as
//! the brain gets it). Booted, shipped firewall, shipped member edges.

#[path = "support/speaker_road.rs"]
mod speaker_road;

use meclaw_core::serde_json::{Value, json};
use speaker_road::*;

/// A round the turn was NOT spoken in: the releaser's own conversation.
const OTHER_ROUND: &str = r#"["agent:scribe","member:robin"]"#;

/// (a) The member in their own bound chat: named, in the round, behind the
/// shipped firewall.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_bound_chat_names_the_member_behind_the_firewall() {
    if !shipped() {
        return;
    }
    let mut road = Road::boot().await;
    let t = road
        .say("own", Some(OWN_CHAT), Some(json!(OWN_CHAT)), None, "hello")
        .await;
    let got = next(&mut road.turns, "the member's turn").await;
    assert_eq!(got.trace_id, t, "another turn arrived first");
    assert_eq!(
        ctx(&got, "speaker").as_deref(),
        Some(SPEAKER),
        "{:?}",
        got.headers.context
    );
    assert_eq!(ctx(&got, "audience_set").as_deref(), Some(ROUND));
    assert_eq!(
        ctx(&got, "user_id"),
        None,
        "the firewall consumes the sender id; the speaker is what survives it"
    );
    road.shutdown().await;
}

/// (b) A verified caller: the signal turn carries the identity the switch
/// stamped and names the member; words of the media half that carry no sender
/// -- a session the signalling half named to nobody -- name nobody: the edge's
/// own literal fallback for `user_id` is a firewall dimension, never a proof.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_verified_caller_names_the_member_and_an_unproven_line_names_nobody() {
    if !shipped() {
        return;
    }
    let readme = std::fs::read_to_string(repo("templates/freeswitch/README.md")).expect("README");
    let documented = phone_speaker("<the person's sender id>", "<member>");
    assert!(
        readme.contains(&format!("\"speaker\": \"{documented}\"")),
        "templates/freeswitch/README.md does not wire the speaker the lock boots: {documented}"
    );

    let mut road = Road::boot().await;
    let signal = road
        .say(
            "phone",
            None,
            Some(json!(LINE_USER)),
            Some(json!({"verified_user": LINE_USER})),
            "The caller is on the line. Greet them.",
        )
        .await;
    let got = next(&mut road.turns, "the signal turn").await;
    assert_eq!(got.trace_id, signal);
    assert_eq!(
        ctx(&got, "speaker").as_deref(),
        Some(SPEAKER),
        "{:?}",
        got.headers.context
    );

    let media = road.say("phone", None, None, None, "it's me").await;
    let got = next(&mut road.turns, "the media turn").await;
    assert_eq!(got.trace_id, media);
    assert!(names_nobody(&got), "{:?}", got.headers.context);

    let other = road
        .say(
            "phone",
            None,
            Some(json!("somebody-else")),
            Some(json!({"verified_user": "somebody-else"})),
            "hi",
        )
        .await;
    let got = next(&mut road.turns, "another caller").await;
    assert_eq!(got.trace_id, other);
    assert!(names_nobody(&got), "{:?}", got.headers.context);

    // OR-NL-164: the member's OWN sender id without the switch's proof -- the
    // `callers` fallback, or the number of a call this channel placed -- is
    // configuration, and names nobody.
    let fallback = road
        .say(
            "phone",
            None,
            Some(json!(LINE_USER)),
            None,
            "it's me, really",
        )
        .await;
    let got = next(&mut road.turns, "the fallback caller").await;
    assert_eq!(got.trace_id, fallback);
    assert!(
        names_nobody(&got),
        "a sender without the switch's proof was named: {:?}",
        got.headers.context
    );
    road.shutdown().await;
}

fn hold_id(m: &meclaw_core::Message) -> String {
    assert_eq!(
        hop(m, "route").as_deref(),
        Some("hold"),
        "{:?}",
        m.headers.hop
    );
    hop(m, "hold_id").expect("the notice names the parked turn")
}

/// The hold: a turn a person releases walks on as the turn it WAS -- the
/// speaker its ingress stamped, not the speaker of the chain that released it.
///
/// Red before GH #979: the stranger's turn, released from the member's own
/// turn, arrived named `member:alex` and in the releaser's round; the member's
/// own held turn, released by an operator, arrived naming nobody, in no round.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_released_turn_keeps_the_speaker_it_arrived_with() {
    if !shipped() {
        return;
    }
    let mut road = Road::boot().await;

    // A stranger in the member's own chat is parked, and the member releases it.
    road.say("own", Some(OWN_CHAT), Some(json!(HELD)), None, "let me in")
        .await;
    let notice = next(&mut road.park, "the stranger's hold").await;
    // The releaser's chain carries every key of the releaser's own turn
    // (OR-NL-179): their speaker and round, the stamped round, the tool
    // narrowing of their channel, their brief subject, chat, node and
    // generation.
    let releaser = json!({
        "speaker": SPEAKER, "audience_set": OTHER_ROUND, "turn_round": OTHER_ROUND,
        "tools_allow": "object_confirm,object_set", "tools_deny": "object_find",
        "counterpart": "peer:releaser", "counterpart_name": "Releaser",
        "chat_id": "releaser-chat", "channel_node": "releaser-node",
        "assistant": "releaser-gen"});
    road.release(&hold_id(&notice), releaser.clone()).await;
    let got = next(&mut road.turns, "the released stranger").await;
    // The turn keeps what it came with: none of the releaser's keys rides on.
    for (key, theirs) in releaser.as_object().expect("an object") {
        assert_ne!(
            ctx(&got, key).as_deref(),
            theirs.as_str(),
            "the releaser's `{key}` rode onto the released stranger: {:?}",
            got.headers.context
        );
    }
    // ... and what the ingress stamped on it comes back.
    assert_eq!(
        ctx(&got, "turn_round").as_deref(),
        Some(ROUND),
        "the released turn lost the round it was born in: {:?}",
        got.headers.context
    );
    assert_eq!(
        ctx(&got, "channel_node").as_deref(),
        Some("own"),
        "{:?}",
        got.headers.context
    );
    assert_eq!(
        ctx(&got, "assistant").as_deref(),
        Some(AGENT),
        "{:?}",
        got.headers.context
    );
    assert_eq!(hop(&got, "hold_id"), Some(hold_id(&notice)));
    // OR-NL-155 (I.4): two rounds -- the turn's own and the releaser's. The
    // released turn is heard in the round it was spoken in.
    assert_eq!(
        ctx(&got, "audience_set").as_deref(),
        Some(ROUND),
        "the releaser's round rode onto the released turn: {:?}",
        got.headers.context
    );
    assert!(
        names_nobody(&got),
        "the releaser's speaker rode onto a stranger's turn: {:?}",
        got.headers.context
    );

    // The member, writing as the bound sender of the group, is parked; an
    // operator with no speaker of their own releases it.
    road.say(
        "group",
        Some(GROUP_CHAT),
        Some(json!(GROUP_USER)),
        None,
        "it's me",
    )
    .await;
    let notice = next(&mut road.park, "the member's hold").await;
    road.release(&hold_id(&notice), Value::Object(Default::default()))
        .await;
    let got = next(&mut road.turns, "the released member").await;
    assert_eq!(hop(&got, "hold_id"), Some(hold_id(&notice)));
    assert_eq!(
        ctx(&got, "speaker").as_deref(),
        Some(SPEAKER),
        "the member's own turn lost its speaker in custody: {:?}",
        got.headers.context
    );
    assert_eq!(
        ctx(&got, "audience_set").as_deref(),
        Some(ROUND),
        "the member's held turn lost its round in custody: {:?}",
        got.headers.context
    );
    road.shutdown().await;
}

/// GH #979 — the third proof of a speaker: an authenticated web session.
///
/// The web cell carries the identity the authenticating proxy in front put in
/// `params.identity_header` as `hop.user_id` (`templates/web/README.md`). A
/// member's web channel grown by `grow_level` takes `bind_user`, the identity
/// the proxy spells for the member; the entry edge then names the member as the
/// turn's speaker only when `hop.user_id` equals it. Another identity, or no
/// header at all, names nobody; without `bind_user` the edge stamps no speaker
/// key (GH #949 `the_self_bound_channels_never_name_a_speaker`).
///
/// Measured where it is decided: the SHIPPED recipe renders the edge, and its
/// condition and modifier run through the REAL evaluator the substrate uses.
///
/// (Y, OR-NL.I.3) Its own module, so its names do not meet the speaker road's.
mod web_session {
    use meclaw_colony::cel_eval::{
        apply_modifier, evaluate_condition, parse_condition, parse_modifier,
    };
    use meclaw_colony::config::ModifierSpec;
    use meclaw_core::Headers;
    use meclaw_core::serde_json::{Map, Value, json};
    use meclaw_testing::{emit_all, shipped_script};

    const RECIPES: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../templates/builder/recipes/config.json"
    );
    const WEB_README: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../templates/web/README.md");

    const MEMBER: &str = "/os/orgs/acme/members/alex";
    const ROUND: &str = r#"["agent:scribe","member:alex"]"#;
    const SPEAKER: &str = "member:alex";
    const WEB: &str = "web@2.3.0";
    /// The member's identity as the proxy spells it.
    const WEB_USER: &str = "alex@example.org";

    fn web_wish(user: Option<&str>) -> Value {
        let mut params = json!({
            "scope": MEMBER, "level": "channel", "name": "web",
            "template": WEB, "assistant": "scribe",
            "ctx": {"member_person": "alex"}});
        if let Some(u) = user {
            params["bind_user"] = json!(u);
        }
        params
    }

    fn render(params: Value) -> Value {
        let out = emit_all(
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
        out.into_iter().next().expect("an emission")
    }

    fn ingress(params: Value) -> Value {
        let first = render(params);
        assert!(
            first["header"]["error_code"].is_null(),
            "the recipe refused a web wish: {first}"
        );
        let decl = &first["manifest"][0];
        decl["diff"]["add_edges"]
            .as_array()
            .expect("add_edges")
            .iter()
            .find(|e| {
                e["from"] == json!("./web")
                    && e["to"] == json!(".")
                    && e["condition"]
                        .as_str()
                        .is_some_and(|c| c.starts_with("!has(hop.error_code)"))
            })
            .cloned()
            .unwrap_or_else(|| panic!("no ingress edge out of ./web: {decl}"))
    }

    fn map(v: &Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap_or_default()
    }

    /// The context a web turn leaves the channel with; `None` when the edge does
    /// not take it.
    fn turn(edge: &Value, context: Value, hop: Value) -> Option<Headers> {
        let h = Headers::from_parts(map(&context), map(&hop));
        let cond = parse_condition(edge["condition"].as_str().expect("a condition"))
            .expect("the condition compiles");
        if !evaluate_condition(&cond, &h.context, &h.hop).unwrap_or(false) {
            return None;
        }
        let spec: ModifierSpec =
            meclaw_core::serde_json::from_value(edge["modifier"].clone()).expect("modifier spec");
        let m = parse_modifier(&spec).expect("the modifier compiles");
        Some(apply_modifier(&m, &h).expect("the modifier evaluates"))
    }

    fn speaker_of(h: &Headers) -> Option<&str> {
        h.context.get("speaker").and_then(Value::as_str)
    }

    /// The member's own session behind the proxy names the member; another viewer,
    /// or a socket the proxy named nobody on, names nobody — and a speaker a chain
    /// carried in is overwritten, not kept.
    ///
    /// Red before GH #979: `web` is self-bound, so the recipe ignored `bind_user`
    /// and the edge stamped no speaker (`speaker_of` is `None`).
    #[test]
    fn the_bound_identity_names_the_member_and_no_other() {
        let edge = ingress(web_wish(Some(WEB_USER)));
        let own = turn(
            &edge,
            json!({}),
            json!({"user_id": WEB_USER, "session_id": "s1"}),
        )
        .expect("a web turn takes the ingress edge");
        assert_eq!(speaker_of(&own), Some(SPEAKER), "{:?}", own.context);
        assert_eq!(
            own.context.get("audience_set").and_then(Value::as_str),
            Some(ROUND)
        );

        for (hop, what) in [
            (json!({"user_id": "robin@example.org"}), "another viewer"),
            (json!({"user_id": ""}), "an empty identity"),
            (json!({"session_id": "s2"}), "no identity header"),
        ] {
            let h = turn(&edge, json!({"speaker": SPEAKER}), hop.clone())
                .unwrap_or_else(|| panic!("{what}: the web turn lost its ingress edge"));
            assert_eq!(
                speaker_of(&h),
                Some(""),
                "{what} ({hop}) was named as the member: {:?}",
                h.context
            );
            assert_eq!(
                h.context.get("audience_set").and_then(Value::as_str),
                Some(ROUND),
                "{what}: the round changed"
            );
        }
    }

    /// Without `bind_user` a web channel stamps no speaker key at all; the
    /// environment form renders as written, for the colony to bind at the door;
    /// a default form is refused like on every other binding.
    #[test]
    fn without_bind_user_nobody_and_the_binding_forms_hold() {
        let plain = ingress(web_wish(None));
        assert!(
            plain["modifier"]["set_context"].get("speaker").is_none(),
            "{plain}"
        );

        let env = ingress(web_wish(Some("${WEB_USER}")));
        assert_eq!(
            env["modifier"]["set_context"]["speaker"],
            json!("has(hop.user_id) && string(hop.user_id) == '${WEB_USER}' ? 'member:alex' : ''")
        );

        let refused = render(web_wish(Some("${WEB_USER:-alex}")));
        assert!(
            !refused["header"]["error_code"].is_null(),
            "a default form was rendered: {refused}"
        );
    }

    /// The contract the operator reads: the web README names `bind_user`.
    #[test]
    fn the_web_readme_names_bind_user() {
        let readme = std::fs::read_to_string(WEB_README).expect("README");
        assert!(
            readme.contains("`bind_user` (GH #979)"),
            "templates/web/README.md"
        );
    }
}

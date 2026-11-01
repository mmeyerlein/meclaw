//! GH #896 -- the road a renewal travels, drawn in the shipped templates, both
//! ways.
//!
//! A renewed duplex session raises `renewed` in the media cell and needs its
//! answer back on the call's `in_advise`. Between the two stand five
//! templates, each with its own edge, and a lane that one of them forgot dies
//! as `hive_no_route` with no test red (the #803 lesson). So this file walks
//! the SHIPPED edges with the colony's own CEL (`meclaw_colony::cel_eval`),
//! hop by hop, the way the router would:
//!
//! * up: `voice` emits `renewed` -> `freeswitch` rim -> the member's
//!   `./channels -> ./assistants` (re-stamped `in_renewed`, `context.call_id`
//!   promoted) -> the assistant's `. -> ./talky` -> talky's `. -> ./curator`
//!   -> the curator's `. -> ./handover`;
//! * down: the curator's `./handover -> .` (`sidecar`, section `context`,
//!   `context.call_id` set) -> talky's `./curator -> .` -> the assistant's
//!   `./talky -> .` -> the member's `./assistants -> ./channels`, re-stamped
//!   `in_advise` -> the freeswitch rim's `. -> ./voice`.
//!
//! The container legs a builder renders per child (`./channels/<name>`,
//! `./assistants/<generation>`) are the instantiating mutation's, exactly as
//! for `delegation` (templates/member/README.md), and are not walked here.
//!
//! The round rides the whole road (GH #925): the channel's leg stamps the
//! call's `context.audience_set` on `renewed` as on every lane it promotes
//! (templates/voice/README.md), and `./handover` gates what the block may tell
//! by the round of the renewal it serves -- so no edge of the road may drop or
//! rewrite it, nor the `session_id` a renewal without a round keeps to.
//!
//! Guarded like every template-reading test (GH #49).

use meclaw_colony::cel_eval::{
    apply_modifier, evaluate_condition, parse_condition, parse_modifier,
};
use meclaw_colony::config::ModifierSpec;
use meclaw_core::Headers;
use meclaw_core::serde_json::{self as sj, Map, Value, json};
use std::path::PathBuf;

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const TEMPLATES: [&str; 5] = ["freeswitch", "member", "assistant", "talky", "curator"];

fn shipped() -> bool {
    TEMPLATES
        .iter()
        .all(|t| repo(&format!("templates/{t}/config.json")).is_file())
}

fn edges_of(template: &str) -> Vec<Value> {
    let p = repo(&format!("templates/{template}/config.json"));
    let raw = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    let v: Value = sj::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    v["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Every edge of `template` from `from` to `to` that carries `(context, hop)`,
/// with its modifier applied -- the headers the next hop sees.
fn carried(
    template: &str,
    from: &str,
    to: &str,
    context: &Map<String, Value>,
    hop: &Map<String, Value>,
) -> Vec<Headers> {
    let mut out = Vec::new();
    for e in edges_of(template) {
        if e["from"] != from || e["to"] != to {
            continue;
        }
        if let Some(c) = e["condition"].as_str() {
            let cond = parse_condition(c).unwrap_or_else(|x| panic!("{template}: {c}: {x}"));
            if !matches!(evaluate_condition(&cond, context, hop), Ok(true)) {
                continue;
            }
        }
        let h = Headers::from_parts(context.clone(), hop.clone());
        if e["modifier"].is_object() {
            let spec: ModifierSpec = sj::from_value(e["modifier"].clone()).expect("a modifier");
            let m = parse_modifier(&spec).expect("a modifier parses");
            match apply_modifier(&m, &h) {
                Ok(next) => out.push(next),
                Err(_) => continue,
            }
        } else {
            out.push(h);
        }
    }
    out
}

/// Exactly one edge carries it; its headers.
fn one(
    template: &str,
    from: &str,
    to: &str,
    context: &Map<String, Value>,
    hop: &Map<String, Value>,
) -> Headers {
    let mut got = carried(template, from, to, context, hop);
    assert_eq!(
        got.len(),
        1,
        "{template}: exactly one edge {from} -> {to} carries hop {hop:?} with context {context:?}"
    );
    got.remove(0)
}

/// The call's round as the channel's leg stamps it (GH #925): TEXT, one
/// member.
const ROUND: &str = r#"["member:e"]"#;

fn obj(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

#[test]
fn a_renewal_reaches_the_curator_and_its_handover_reaches_the_call() {
    if !shipped() {
        eprintln!("the templates of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    // Up. What `voice` emits (templates/voice/README.md, lane `renewed`), with
    // the context the channel's own edge minted when the call came in.
    // The context the channel's own edge minted when the call came in, the
    // call's round among it.
    let ctx = obj(
        json!({"session_id": "call-9", "channel": "call-9", "channel_node": "phone",
                         "audience_set": ROUND}),
    );
    let hop = obj(
        json!({"route": "renewed", "session_id": "call-9", "call_id": "call-9",
                         "platform": "voice", "mode": "auto", "engine": "duplex",
                         "renewal_n": 1, "renewed_at": 1_790_000_000_000_u64}),
    );
    let h = one("freeswitch", "./voice", ".", &ctx, &hop);
    let h = one("member", "./channels", "./assistants", &h.context, &h.hop);
    assert_eq!(h.hop["route"], "in_renewed");
    assert_eq!(
        h.context.get("call_id").and_then(Value::as_str),
        Some("call-9"),
        "the member promotes the call onto context"
    );
    assert!(
        carried("member", "./channels", "./firewall", &h.context, &h.hop).is_empty(),
        "a renewal is no turn: it goes round the firewall"
    );
    let h = one("assistant", ".", "./talky", &h.context, &h.hop);
    let h = one("talky", ".", "./curator", &h.context, &h.hop);
    let h = one("curator", ".", "./handover", &h.context, &h.hop);
    assert_eq!(h.hop["route"], "in_renewed");
    assert_eq!(h.hop["renewal_n"], 1);
    assert_eq!(
        h.context.get("audience_set").and_then(Value::as_str),
        Some(ROUND),
        "the renewal reaches the handover in the round of its call, which decides what the \
         block may tell (GH #925)"
    );
    assert_eq!(
        h.context.get("session_id").and_then(Value::as_str),
        Some("call-9"),
        "and with the session a renewal without a round keeps to (OR-BD-4)"
    );

    // Down. What `./handover` emits for it (templates/curator/README.md,
    // route `sidecar`), under the context the renewal came with.
    let back_hop = obj(
        json!({"route": "sidecar", "section": "context", "call_id": "call-9",
                              "renewal_n": 1}),
    );
    let mut back_ctx = h.context.clone();
    back_ctx.insert("cur_origin".into(), json!("curator-handover"));
    let h = one("curator", "./handover", ".", &back_ctx, &back_hop);
    assert!(
        !h.context.contains_key("cur_origin"),
        "the hive's own keys stay inside"
    );
    let h = one("talky", "./curator", ".", &h.context, &h.hop);
    let h = one("assistant", "./talky", ".", &h.context, &h.hop);
    let h = one("member", "./assistants", "./channels", &h.context, &h.hop);
    assert_eq!(h.hop["route"], "in_advise");
    assert_eq!(h.hop["section"], "context");
    assert_eq!(
        h.hop["renewal_n"], 1,
        "the takeover mark reaches the channel untouched"
    );
    assert_eq!(
        h.context.get("call_id").and_then(Value::as_str),
        Some("call-9")
    );
    assert_eq!(
        h.context.get("audience_set").and_then(Value::as_str),
        Some(ROUND),
        "the round rides back with the block"
    );
    let h = one("freeswitch", ".", "./voice", &h.context, &h.hop);
    assert_eq!(h.hop["route"], "in_advise");
}

/// The lanes a member declares at `./channels` as ports (`emits` with `at`).
fn member_ports_at_channels() -> Vec<String> {
    let p = repo("templates/member/config.json");
    let v: Value = sj::from_str(&std::fs::read_to_string(&p).expect("member config"))
        .expect("member config is JSON");
    v["params"]["contract"]["emits"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|e| {
            e["at"]
                .as_array()
                .is_some_and(|a| a.iter().any(|x| x == "./channels"))
        })
        .filter_map(|e| e["route"].as_str().map(str::to_string))
        .collect()
}

/// The routes a template's contract declares under `emits`.
fn emits_of(template: &str) -> Vec<String> {
    let p = repo(&format!("templates/{template}/config.json"));
    let v: Value =
        sj::from_str(&std::fs::read_to_string(&p).expect("config")).expect("config is JSON");
    v["params"]["contract"]["emits"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|e| e["route"].as_str().map(str::to_string))
        .collect()
}

/// live-2 fx-call (#1369): a duplex call's `usage` and `call_mark` leave the
/// media half (templates/voice/config.json, `emit_usage`, `call_marks_s`) and
/// the member declares both as ports at `./channels` -- but the freeswitch rim
/// edge `./voice -> .` let neither through, so both died inside the hive as
/// `hive_no_route` and the satz had nothing to draw. Walked with the colony's
/// own CEL: the voice emission crosses the rim on the SAME edge that carries
/// `delegation` (same `delete_context`), the hive declares both lanes, and the
/// member declares them at the container the satz binds the line to. The
/// container leg itself (`./channels/<line> -> ./channels`) is the satz's, as
/// for `seed_ask` (templates/member/README.md).
#[test]
fn usage_and_call_mark_cross_the_freeswitch_rim_to_the_member_port() {
    if !shipped() {
        eprintln!("the templates of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let ctx = obj(json!({"session_id": "call-9", "call_id": "call-9",
                         "phone_call": "call-9", "phone_origin": "signal"}));
    let delegation = obj(json!({"route": "delegation", "session_id": "call-9",
                                "call_id": "call-9", "engine": "duplex",
                                "delegation_id": "d-1", "offset_ms": 0}));
    let reference = one("freeswitch", "./voice", ".", &ctx, &delegation);
    for hop in [
        json!({"route": "usage", "session_id": "call-9", "call_id": "call-9",
               "platform": "voice", "mode": "auto", "engine": "duplex",
               "seconds": 312, "final": true, "complete": true}),
        json!({"route": "call_mark", "session_id": "call-9", "call_id": "call-9",
               "platform": "voice", "mode": "auto", "engine": "duplex",
               "elapsed_s": 900}),
    ] {
        let hop = obj(hop);
        let route = hop["route"].as_str().unwrap().to_string();
        let h = one("freeswitch", "./voice", ".", &ctx, &hop);
        assert_eq!(h.hop, hop, "{route}: the hop crosses the rim untouched");
        assert_eq!(
            h.context, reference.context,
            "{route}: the rim strips the hive's own keys exactly as on `delegation`"
        );
        assert!(
            emits_of("freeswitch").contains(&route),
            "{route}: the freeswitch contract declares the lane it lets out"
        );
        assert!(
            member_ports_at_channels().contains(&route),
            "{route}: the member declares it as a port at ./channels"
        );
    }
}

/// live-2 fx-call (#1369): the end of a call reaches the curator and its
/// `call_facts` come back out of the generation. Up: the member hands the
/// final `usage` to the generation as `in_call_end` (the satz's edge) ->
/// the assistant's `. -> ./talky` -> talky's `. -> ./curator` -> the curator's
/// `. -> ./history`. Down: `./history -> .` (`call_facts`, `context.call_id`
/// set) -> talky's `./curator -> .` -> the assistant's `./talky -> .`. Drawn
/// exactly like `in_seed_ask` / `seed`, the chat twin included.
#[test]
fn the_call_end_reaches_the_curator_and_its_facts_leave_the_generation() {
    if !shipped() {
        eprintln!("the templates of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let ctx = obj(json!({"session_id": "call-9", "call_id": "call-9",
                         "channel_node": "phone", "audience_set": ROUND}));
    let hop = obj(json!({"route": "in_call_end", "session_id": "call-9",
                         "call_id": "call-9", "final": true, "seconds": 312}));
    let h = one("assistant", ".", "./talky", &ctx, &hop);
    assert!(
        carried("assistant", ".", "./talky-chat", &ctx, &hop).is_empty(),
        "one surface takes it, guarded on the channel like `in_seed_ask`"
    );
    let h = one("talky", ".", "./curator", &h.context, &h.hop);
    let h = one("curator", ".", "./history", &h.context, &h.hop);
    assert_eq!(h.hop["route"], "in_call_end");
    assert_eq!(h.hop["call_id"], "call-9");

    let back = obj(json!({"route": "call_facts", "call_id": "call-9",
                          "session_id": "call-9"}));
    let mut bctx = h.context.clone();
    bctx.insert("cur_origin".into(), json!("curator-history"));
    let h = one("curator", "./history", ".", &bctx, &back);
    assert!(
        !h.context.contains_key("cur_origin"),
        "the hive's own keys stay inside"
    );
    assert_eq!(
        h.context.get("call_id").and_then(Value::as_str),
        Some("call-9")
    );
    let h = one("talky", "./curator", ".", &h.context, &h.hop);
    let h = one("assistant", "./talky", ".", &h.context, &h.hop);
    assert_eq!(h.hop["route"], "call_facts");
    for (t, lanes) in [
        ("curator", ("in_call_end", "call_facts")),
        ("talky", ("in_call_end", "call_facts")),
        ("assistant", ("in_call_end", "call_facts")),
    ] {
        assert!(
            emits_of(t).contains(&lanes.1.to_string()),
            "{t} declares {}",
            lanes.1
        );
    }
    // The chat twin: the same lane, the other surface.
    let mut chat = ctx.clone();
    chat.insert("channel_node".into(), json!("chat"));
    one("assistant", ".", "./talky-chat", &chat, &hop);
    one("assistant", "./talky-chat", ".", &h.context, &back);
}

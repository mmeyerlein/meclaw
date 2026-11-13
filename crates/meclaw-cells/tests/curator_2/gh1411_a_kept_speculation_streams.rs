//! #1411 (OR-D53-A1) -- a kept speculation streams like any answer.
//!
//! WHY this file exists: `./quick_spec` had no `partial` road (talky edges
//! `./quick_spec -> ./curator` on `in_spec_llm` and `model_refused` only), and
//! the curator held a spec answer whole until `spec_settle` released it as a
//! finished body (`spec_release`). A multi-sentence speculation that the turn
//! kept therefore spoke only at its end: Telegram 2/5 one message, T1 2/10
//! without a `partial`, fact turns answered whole after 822/1 050 ms (S9
//! round 2, reader B). The fix lets a spec partial through the curator, gated
//! on what the curator already knows: the turn's spec row is `await` (the
//! stamp is in and the normal call is stored as the fallback) and the stored
//! `needs` carries no `tool` (`spec_verdict` keeps every answer of such a
//! turn). Anything else -- `fired` (no stamp yet), `held`, `dropped`,
//! `released`, no row, another call's row, or a turn that needs a tool (the
//! verdict may still drop the answer: `no_tool_call`, `family:`) -- holds the
//! partial back, with one stderr line. A partial is cumulative, so a partial
//! held back costs no word: the next one carries it.
//!
//! The behaviour claims run the SHIPPED curator scripts (the curator-hive
//! harness runs them synchronously); the spec rows come from the shipped flow
//! (`spec_call` -> row `fired`, the stamp -> row `await`, a stamp that dooms
//! the speculation -> rows gone). States the flow never leaves standing
//! (`dropped`, `released`) are written into the fired row directly. No clock
//! is involved.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_colony::config::{EdgeSpec, HiveParams};
use meclaw_colony::edge_table::{Edge, EdgeTable, apply_edges};
use meclaw_core::serde_json::{self as sj, Map, Value, json};
use meclaw_core::{Headers, Path, Uuid};
use sha2::{Digest, Sha256};

const TALKY: &str = "templates/talky/config.json";
const QUICK_SPEC: &str = "templates/talky/quick_spec/config.json";
const CURATOR: &str = "templates/curator/config.json";
const BASE: &str = "/t";

fn spec_hive(spec_tools: &str) -> Hive {
    let mut h = Hive::with(&[
        ("lead", "quick_speculative", json!("1")),
        ("lead", "spec_tools", json!(spec_tools)),
    ])
    .reporting_composite("talky");
    h.check_emits = true;
    h
}

/// The collector's early curate (`hop.spec` "1", no path); the spec call.
fn spec_call(h: &mut Hive, turn: &str, text: &str) -> Msg {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "path": "", "spec": "1"});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": ""});
    h.lane("in_curate", ctx, hop, json!({"messages": [user(text)]}));
    let quick = h.routed("quick");
    assert_eq!(quick.len(), 1, "one spec call: {:?} {:?}", h.out, h.stderr);
    assert_eq!(quick[0].hop["spec"], json!("1"));
    quick[0].clone()
}

/// The normal curate of the turn at its route stamp.
fn seam(h: &mut Hive, turn: &str, path: &str, needs: &str, families: &str, text: &str) {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "path": path, "needs": needs, "families": families});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": path});
    let system = json!({"instructions": {"mode": {"text": "Be brief."},
                                         "sidecar": {"text": "End with a sidecar block."}}});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user(text)], "system": system}),
    );
}

/// The context a `./quick_spec` emission carries: the call's context with
/// the four keys the curator edge promoted, and `spec` "1".
fn spec_ctx(call: &Msg) -> Map<String, Value> {
    let mut ctx = call.context.clone();
    for k in ["curator_call", "turn_id", "session_id", "iter"] {
        ctx.insert(k.into(), call.hop[k].clone());
    }
    ctx.insert("spec".into(), json!("1"));
    ctx
}

/// The hop of a model partial (`llm/partial.rs` `partial_body`, the keys of
/// `PARTIAL_HOP_KEYS`), before an edge renames its route.
fn partial_hop(seq: u64, of: &str) -> Map<String, Value> {
    obj(json!({"route": "partial", "seq": seq, "final": false, "partial_of": of}))
}

/// One partial of the spec call as talky's `./quick_spec -> ./curator` edge
/// hands it in: lane `in_spec_partial`, the partial's own hop and body.
/// Returns the hop and the messages that went in.
fn spec_partial(h: &mut Hive, call: &Msg, seq: u64, text: &str) -> (Map<String, Value>, Value) {
    h.out.clear();
    h.stderr.clear();
    let hop = partial_hop(seq, "0190aaaa-0000-7000-8000-000000000001");
    let messages = json!([said(text)]);
    h.lane(
        "in_spec_partial",
        Value::Object(spec_ctx(call)),
        Value::Object(hop.clone()),
        json!({"messages": messages.clone()}),
    );
    (hop, messages)
}

/// The state of the turn's spec row (`spec:<session>#<turn>`), "" when gone.
fn row_state(h: &Hive, turn: &str) -> String {
    let raw = h.lead_state(&format!("spec:s1#{turn}"));
    let row: Value = sj::from_str(&raw).unwrap_or(Value::Null);
    row["state"].as_str().unwrap_or_default().to_string()
}

/// Rewrites the state of the turn's spec row in place (a fixture for the
/// states the shipped flow never leaves standing).
fn set_row_state(h: &mut Hive, turn: &str, state: &str) {
    // The spec row lives in the lead's store since GH #1447.
    let key = format!("spec:s1#{turn}");
    let raw = h.lead_state(&key);
    let mut row: Value = sj::from_str(&raw).expect("a spec row to rewrite");
    row["state"] = json!(state);
    let value = sj::to_string(&row).expect("JSON");
    h.lead_db
        .as_ref()
        .expect("the tree ships the lead")
        .execute(
            "UPDATE state SET value = ?1 WHERE key = ?2",
            rusqlite::params![value, key],
        )
        .expect("rewrite the spec row");
    assert_eq!(row_state(h, turn), state);
}

/// The stderr lines that say a spec partial of `turn` was held back.
fn held_back_lines(h: &Hive, turn: &str) -> Vec<String> {
    let needle = format!("spec partial of turn {turn} held back");
    h.stderr
        .iter()
        .flat_map(|e| e.lines().map(str::to_string).collect::<Vec<_>>())
        .filter(|l| l.contains(&needle))
        .collect()
}

/// Every message that left the hive, by route.
fn out_routes(h: &Hive) -> Vec<String> {
    h.out.iter().map(|m| m.route().to_string()).collect()
}

/// Asserts that exactly one `partial` left the hive and that it is the
/// partial that came in: hop keys and body byte for byte, only the route
/// back to `partial`, no curator-internal key in its context.
fn assert_passed(h: &Hive, hop_in: &Map<String, Value>, messages_in: &Value) {
    assert_eq!(
        out_routes(h),
        vec!["partial".to_string()],
        "exactly one send, route partial: {:?} {:?}",
        h.out,
        h.stderr
    );
    let out = &h.out[0];
    let mut want = hop_in.clone();
    want.insert("route".into(), json!("partial"));
    assert_eq!(out.hop, want, "the hop keys pass unchanged");
    let keys: Vec<&String> = out.body.keys().collect();
    assert_eq!(keys, vec!["messages"], "the body is the partial's own");
    assert_eq!(
        out.body["messages"], *messages_in,
        "the body passes unchanged"
    );
    let internal: Vec<&String> = out
        .context
        .keys()
        .filter(|k| k.starts_with("cur_"))
        .collect();
    assert!(
        internal.is_empty(),
        "curator-internal context leaked: {internal:?}"
    );
    assert_eq!(h.emit_violations, Vec::<String>::new());
}

/// Pin 1: the stamp is in, the turn needs no tool -- the row is `await`, so
/// whatever the spec answer says will be kept (`spec_verdict`). Its partial
/// passes: exactly one send on `partial`, hop keys and body as they came.
#[test]
fn gh1411_a_spec_partial_passes_once_the_turn_awaits_the_speculation() {
    if !shipped() {
        return;
    }
    let mut h = spec_hive("web,weather");
    let spec = spec_call(&mut h, "t1", "how are you today");
    seam(&mut h, "t1", "simple", "", "web", "how are you today");
    assert_eq!(
        row_state(&h, "t1"),
        "await",
        "fixture: the stamp stored the fallback"
    );
    let (hop, messages) = spec_partial(&mut h, &spec, 1, "I am fine, thank you.");
    assert_passed(&h, &hop, &messages);
    assert!(held_back_lines(&h, "t1").is_empty(), "{:?}", h.stderr);
    assert_eq!(row_state(&h, "t1"), "await", "a partial moves no row");
}

/// Pin 2: before the stamp (row `fired`) nothing goes; the next partial
/// after the stamp goes and, cumulative, carries the first sentence with it.
#[test]
fn gh1411_a_spec_partial_before_the_stamp_goes_nowhere() {
    if !shipped() {
        return;
    }
    let mut h = spec_hive("web,weather");
    let spec = spec_call(&mut h, "t1", "tell me about your day");
    assert_eq!(row_state(&h, "t1"), "fired", "fixture: no stamp yet");
    let first = "My day was quiet.";
    spec_partial(&mut h, &spec, 1, first);
    assert!(
        out_routes(&h).is_empty(),
        "no send before the stamp: {:?}",
        h.out
    );
    assert_eq!(held_back_lines(&h, "t1").len(), 1, "{:?}", h.stderr);
    seam(&mut h, "t1", "simple", "", "web", "tell me about your day");
    assert_eq!(row_state(&h, "t1"), "await");
    let both = "My day was quiet. I read a book in the garden.";
    let (hop, messages) = spec_partial(&mut h, &spec, 2, both);
    assert_passed(&h, &hop, &messages);
    let text = h.out[0].body["messages"][0]["text"]
        .as_str()
        .unwrap_or_default();
    assert!(text.starts_with(first), "cumulative: {text:?}");
}

/// Pin 3: the row is `await`, but the turn needs a tool -- the verdict may
/// still drop the answer (`no_tool_call`, `family:`), so a sentence of it
/// must not speak. Nothing goes; one stderr line names the turn.
#[test]
fn gh1411_a_spec_partial_of_a_turn_that_needs_a_tool_is_held_back() {
    if !shipped() {
        return;
    }
    let mut h = spec_hive("web,weather");
    let spec = spec_call(&mut h, "t1", "what is the weather in Rome");
    seam(
        &mut h,
        "t1",
        "simple",
        "tool",
        "weather",
        "what is the weather in Rome",
    );
    assert_eq!(
        row_state(&h, "t1"),
        "await",
        "fixture: a family inside spec_tools does not doom the speculation: {:?}",
        h.stderr
    );
    spec_partial(&mut h, &spec, 1, "It is sunny in Rome today.");
    assert!(out_routes(&h).is_empty(), "held back: {:?}", h.out);
    let lines = held_back_lines(&h, "t1");
    assert_eq!(lines.len(), 1, "{:?}", h.stderr);
    assert!(lines[0].contains("(await"), "{lines:?}");
    assert_eq!(h.emit_violations, Vec::<String>::new());
}

/// Pin 4: a dropped or released row, no row at all (the stamp doomed the
/// speculation and its rows went), or a row of another call -- the partial
/// goes nowhere, and one stderr line names the state.
#[test]
fn gh1411_a_spec_partial_of_a_dropped_or_released_turn_goes_nowhere() {
    if !shipped() {
        return;
    }
    for state in ["dropped", "released"] {
        let mut h = spec_hive("web,weather");
        let spec = spec_call(&mut h, "t1", "how are you today");
        set_row_state(&mut h, "t1", state);
        spec_partial(&mut h, &spec, 1, "I am fine, thank you.");
        assert!(out_routes(&h).is_empty(), "{state}: {:?}", h.out);
        let lines = held_back_lines(&h, "t1");
        assert_eq!(lines.len(), 1, "{state}: {:?}", h.stderr);
        assert!(lines[0].contains(&format!("({state})")), "{lines:?}");
        assert_eq!(h.emit_violations, Vec::<String>::new());
    }

    // No row: the stamp needs `orga`, outside `spec_tools` -- dropped at the
    // stamp, the turn's spec rows went with the drop.
    let mut h = spec_hive("web,weather");
    let spec = spec_call(&mut h, "t1", "move my dentist to friday");
    seam(
        &mut h,
        "t1",
        "simple",
        "tool",
        "orga",
        "move my dentist to friday",
    );
    assert_eq!(
        row_state(&h, "t1"),
        "",
        "fixture: the rows went at the stamp"
    );
    spec_partial(&mut h, &spec, 1, "Your dentist is moved.");
    assert!(out_routes(&h).is_empty(), "no row: {:?}", h.out);
    let lines = held_back_lines(&h, "t1");
    assert_eq!(lines.len(), 1, "{:?}", h.stderr);
    assert!(lines[0].contains("(none)"), "{lines:?}");

    // Another call's row: the turn awaits, but a partial of a call that is
    // not the row's own speaks for nobody.
    let mut h = spec_hive("web,weather");
    let spec = spec_call(&mut h, "t1", "how are you today");
    seam(&mut h, "t1", "simple", "", "web", "how are you today");
    assert_eq!(row_state(&h, "t1"), "await");
    let mut foreign = spec.clone();
    foreign
        .hop
        .insert("curator_call".into(), json!("not-the-spec-call"));
    spec_partial(&mut h, &foreign, 1, "I am fine, thank you.");
    assert!(out_routes(&h).is_empty(), "foreign call: {:?}", h.out);
    assert_eq!(held_back_lines(&h, "t1").len(), 1, "{:?}", h.stderr);
}

// ---------------------------------------------------------------- topology

/// talky's shipped edges as the colony's router holds them, rooted at `/t`.
fn talky_table() -> EdgeTable {
    let params = read_json(&repo(TALKY))["params"].clone();
    let hp: HiveParams = sj::from_value(params).expect("talky params");
    let mut t = EdgeTable::new();
    for spec in &hp.graph.edges {
        let spec: &EdgeSpec = spec;
        let abs = |ep: &str| match ep {
            "." => BASE.to_string(),
            other => format!("{BASE}/{}", other.trim_start_matches("./")),
        };
        t.insert(Edge {
            id: Uuid::now_v7(),
            from: Path::new(&abs(&spec.from)),
            to: Path::new(&abs(&spec.to)),
            condition: spec.condition.as_ref().map(|src| {
                meclaw_colony::cel_eval::parse_condition(src)
                    .unwrap_or_else(|e| panic!("condition {src:?}: {e}"))
            }),
            modifier: spec.modifier.as_ref().map(|m| {
                meclaw_colony::cel_eval::parse_modifier(m)
                    .unwrap_or_else(|(k, e)| panic!("modifier {k}: {e}"))
            }),
            is_default: spec.is_default,
            lane: spec.lane.clone(),
            tap: spec.tap,
        });
    }
    t
}

/// The cells one message reaches from `from` (`/t/<cell>`), with each hop.
fn reach(
    t: &EdgeTable,
    from: &str,
    ctx: &Map<String, Value>,
    hop: &Map<String, Value>,
) -> Vec<(String, Headers)> {
    let hs = Headers::from_parts(ctx.clone(), hop.clone());
    apply_edges(t, &Path::new(&format!("{BASE}/{from}")), &hs)
        .into_iter()
        .map(|d| (d.target.as_str().to_string(), d.headers_out))
        .collect()
}

fn edges(template: &str) -> Vec<Value> {
    read_json(&repo(template))["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .clone()
}

fn cond(e: &Value) -> &str {
    e["condition"].as_str().unwrap_or_default()
}

fn is_partial_edge(e: &Value) -> bool {
    cond(e).contains("hop.route == 'partial'")
}

/// Pin 5: the spec partial's road is `./quick_spec -> ./curator` on
/// `in_spec_partial`, inside the curator `. -> ./policy` and `./policy -> .`,
/// then `./curator -> .` on `partial` with the curator's keys (`spec`,
/// `curator_call`) out of the context -- never the splitter, the
/// dispatcher or the collector. Each llm cell of talky has EXACTLY ONE
/// `partial` edge, checked over the whole list (o45 M4: a check of the last
/// matching edge hid a doubled one). Only these edges are locked (OR-D53-53:
/// no hash of every talky edge).
#[test]
fn gh1411_the_spec_partial_reaches_only_the_rim() {
    if !shipped() {
        return;
    }
    let talky = edges(TALKY);
    for (cell, to) in [
        ("./quick", "."),
        ("./brain", "."),
        ("./quick_spec", "./curator"),
    ] {
        let list: Vec<&Value> = talky
            .iter()
            .filter(|e| e["from"] == cell && is_partial_edge(e))
            .collect();
        assert_eq!(list.len(), 1, "{cell}: exactly one partial edge: {list:?}");
        assert_eq!(list[0]["to"], json!(to), "{cell}: {list:?}");
    }
    let into_curator: Vec<&Value> = talky
        .iter()
        .filter(|e| e["from"] == "./quick_spec" && is_partial_edge(e))
        .collect();
    assert_eq!(
        into_curator[0]["modifier"]["set_hop"]["route"],
        json!("'in_spec_partial'")
    );
    let rim: Vec<&Value> = talky
        .iter()
        .filter(|e| e["from"] == "./curator" && is_partial_edge(e))
        .collect();
    assert_eq!(rim.len(), 1, "one curator partial edge: {rim:?}");
    assert_eq!(rim[0]["to"], json!("."));
    let gone: Vec<&str> = rim[0]["modifier"]["delete_context"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    for k in ["spec", "curator_call"] {
        assert!(gone.contains(&k), "the rim drops `{k}`: {gone:?}");
    }

    // The router: a spec partial out of `./quick_spec` reaches the curator
    // and nothing else; a partial out of the curator reaches the rim and
    // nothing else, without the curator's keys.
    let t = talky_table();
    let ctx = obj(json!({"session_id": "s1", "turn_id": "t1", "iter": "0",
                             "curator_call": "c-spec", "spec": "1"}));
    let hop = partial_hop(1, "0190aaaa-0000-7000-8000-000000000001");
    let from_spec = reach(&t, "quick_spec", &ctx, &hop);
    let to: Vec<&str> = from_spec.iter().map(|(to, _)| to.as_str()).collect();
    assert_eq!(to, vec!["/t/curator"], "{from_spec:?}");
    assert_eq!(from_spec[0].1.hop["route"], json!("in_spec_partial"));
    let from_curator = reach(&t, "curator", &ctx, &hop);
    let to: Vec<&str> = from_curator.iter().map(|(to, _)| to.as_str()).collect();
    assert_eq!(to, vec![BASE], "{from_curator:?}");
    for k in ["spec", "curator_call"] {
        assert!(
            !from_curator[0].1.context.contains_key(k),
            "`{k}` leaves with the partial: {:?}",
            from_curator[0].1.context
        );
    }

    // Inside the curator: one door in, one way out.
    let cur = edges(CURATOR);
    let door: Vec<&Value> = cur
        .iter()
        .filter(|e| cond(e).contains("hop.route == 'in_spec_partial'"))
        .collect();
    assert_eq!(door.len(), 1, "one door: {door:?}");
    assert_eq!(
        (door[0]["from"].clone(), door[0]["to"].clone()),
        (json!("."), json!("./policy"))
    );
    let out: Vec<&Value> = cur
        .iter()
        .filter(|e| e["from"] == "./policy" && is_partial_edge(e))
        .collect();
    assert_eq!(out.len(), 1, "one way out: {out:?}");
    assert_eq!(out[0]["to"], json!("."));
}

// ------------------------------------------------------------------ golden

/// talky's edges without the two #1411 spec-partial edges, before A1.
/// Re-blessed for GH #1440 (the role's self summary: `./brain -> ./curator`
/// on `in_summary_self`, the `summarize` exclusion on the brain's edges, the
/// three keys in the exits' `delete_context`) -- a wanted edge change, old
/// `3dfb4ed81595e8f5c08dfb86c6e2979392884e4656fc5f01523faf4f097406da`.
/// Re-blessed for D58 target (contract points 8 and 10, OR-d58-2.target.48:
/// the curator's roads onto the model cells on `model_call` by `hop.target`,
/// `curator_target` and `bridge` lifted into the context and dropped at every
/// exit, the errand on `errand`, the error edges know a bridge by
/// `context.bridge`) -- a wanted edge change, old
/// `70d98405aa07b68977d889968b501bd08326935b11bc4c89b43d0a7623f29ae2`.
/// Re-blessed for D58 lead (GH #1474, contract point 5: the curate edge
/// `./collector -> ./curator` names talky's main model, `hop.target`
/// `'brain'`; the lead leaves the target to the curate) -- a wanted edge
/// change, old
/// `9a10a92432c19de7f5dcf957de6e8d4bf2606722484dcecf5562346a760ec0a4`.
/// Re-blessed for D58 live (GH #1482: the voice's lanes into `./curator` set
/// `context.live`, `in_call_end` reaches it as `in_live_end`, the exits give
/// the voice its `context.call_id` back from the handle; that strand left
/// the hash at `25f3fa01...`, measured `87ffc1ca...` on its tree) and D58
/// judge (GH #1480, contract points 1, 7 and 8: the asks of `./route` and
/// `./screen_judge` on `view_ask` / `in_view_ask`, their answers on `view` /
/// `in_view`, the rim's contribution into `./curator` on `in_part` instead
/// of `in_screen_topics` -- five edges renamed, nothing else of them moved)
/// -- a wanted edge change, old
/// `25f3fa012b2424ae10090e2179ad63485051aef6966f09f25e6c9369d73dcb6f`.
///
/// Re-blessed for D58 note (GH #1477, contract point 6: the receipt of a
/// shown note leaves talky on `./curator -> .` `note_receipt`) over D58 lead
/// -- a wanted edge change, old (lead alone)
/// `25f3fa012b2424ae10090e2179ad63485051aef6966f09f25e6c9369d73dcb6f`.
/// Live and note together (D58 live GH #1482 merged over note GH #1477, master
/// e587bc0fd): both edge changes at once, old (note alone) `5d2d2a71aa7594e9ea71e63127f6e89492a3696071f8e35b5f26078f8a138d43`,
/// old (live alone) `87ffc1cabe16f9a9cf3fe1ab2495c2decf77f64e665da6234e93b32cce154cca`.
/// Re-blessed for D58 moveout (GH #1476, contract points 3 and 4: the kind of
/// a call travels in `hop.call_kind`, lifted into the context and dropped at
/// every exit where `bridge` was; the edges onto the quick cells lift
/// `hop.round_open` "1" onto `context.interim` "1"; the error edges know a
/// side call by `context.call_kind` "side") -- a wanted edge change, old
/// `6260b2c51418055ae94a69eea80ec3d9ed027c96ec790dadc1ef2cb622ebbe60`. The
/// lead's new edge on `in_feed` is the lead's (`lead_edge`), not counted here.
/// Bundle B (D58 GH #1478, #1479, #1481 beside #1480): the edges of frame, followup and feed
/// joined this tree; the hash is the one of their union, measured on it, old
/// `d16cbfab66089c7af0e95c3da095162e3cfa5f9d3761c340bbe9f45f8e3ed71b`.
/// Merge of master ac23674e66 (Bundle B) into D58 moveout (GH #1476): both
/// edge changes at once, measured on the union, old (moveout)
/// `96409ad0235c1cc92a4aa81e5b46986cac38f398903258fe21e6b2c861974284`, old (Bundle B)
/// `21bdc2f1efeece5fc97d5afd4bbd134d2232cb03f2acd80cc1bae696fe07bac6`.
const TALKY_EDGES_SHA256: &str = "8055d12c8e5285ed69d9df50941fe1a08d12668a18938be38401ee95feef685e";
/// `quick_spec/config.json` without the partial declarations and without its
/// prose (`description`), before A1.
const QUICK_SPEC_SHA256: &str = "6fb4049742f2723fd8d8b1185fe3280c1ecd1b286c0bee20264201b661567494";
/// The curator hive's edges, accepts and emits without the #1411 lane, its
/// door, its way out and the `partial` route, before A1.
/// Re-blessed for GH #1440 (the door `. -> ./policy` also takes
/// `in_summary_self`) -- a wanted edge change, old
/// `1c3e91d9792f67284977f46abdc9d13ddac7d9282c374462a1404047f2e2c252`.
/// Re-blessed for GH #1451 (D38 V1: the lane `in_note` names its optional
/// `item?`) -- a wanted contract text change, old
/// `72a5a36ab7f1398f0b58a60406431c8380d1335958e5206cc510bd4f45cd0da9`.
/// Re-blessed for D58 target (contract point 8: the hive's way out and its
/// `contract.emits` name one route `model_call` for every model call and
/// `errand` for an errand, no ways) -- a wanted edge and contract change, old
/// `8b68263bac338d0ad3e6e463948f6c0fe791fd7243df2d1079d4f9f34bad864b`.
/// Re-blessed for D58 target (N10: the push's ask leaves `./policy` after a
/// read of the profiles, so the edge `./policy -> ./push` drops the store's
/// `cur_*` context keys) -- a wanted edge change, old
/// `97818131cc9133c8d4b5f0b88f532e712b9dc2a5d17d778b025ca226e71f8349`.
/// Re-blessed for GH #1473 (the hive contract names registered sections,
/// `sections` `{lane?, cap_share?, mark?}`, instead of `memory` and `things`)
/// -- a wanted contract text change, old
/// `8b68263bac338d0ad3e6e463948f6c0fe791fd7243df2d1079d4f9f34bad864b`.
/// Re-blessed on the merge of D58 target over GH #1473: both changes above
/// together; target alone `5a181bef768bf19d8537bc74dca93c39d4cc44fb5ff3f93dbcf0fa966413c571`,
/// sections alone `8d1ced44fecc231876366ee06e207eb777223f9756ab7926454b96c8af197658`.
/// Re-blessed for D58 live (GH #1482: the hive emits `live_facts` instead of
/// `call_facts`, and the voice's lanes and exits name the handle `live`) and
/// D58 judge (GH #1480, contract points 1 to 4: the door `. -> ./history`
/// takes `in_view_ask` and `in_part` instead of `in_turns_ask` and
/// `in_screen_topics`, the exit `./history -> .` passes `view` instead of
/// `turns`, the accepts and emits of the hive say the same, `in_seed_line`
/// names place, order and lifetime instead of a kind, `seed` carries the read
/// context under `context` and `live_facts` the target's contributions) -- a
/// wanted contract change, old
/// `42e159b0537a6a09eb0d834f5905364b5552f30390f8bd29730e88c50d3745cf`.
/// Live and note together (D58 live GH #1482 merged over note GH #1477, master
/// e587bc0fd): both edge changes at once, old (note alone) `c0b8aa535eb07ba63f1d402e9cdea9673bde506edd154a2d901f5c746ecc8ca7`,
/// old (live alone) `004936a4775f0f95cd80fd9833286ca0d750acbed61c2cf8000a6f88d3ee7408`.
/// Re-blessed for D58 moveout (GH #1476, contract point 3: the hive clears
/// `call_kind`, the kind of a call it lifts beside `curator_call`, at every
/// exit where it clears `curator_call`, and the spec tap sets it empty with
/// `interim`) -- a wanted edge change, old
/// `1332074a4e9e30e141e049b07b313106f59bd838743732118fd76c1980fb1a70`.
/// Bundle B (D58 GH #1478, #1479, #1481 beside #1480): the curator hive's edges and ports of frame,
/// followup and feed joined this tree; the hash is the one of their union, measured on it, old
/// `38e1414655d598e1196683451f32dcc47de34b07a40d7f0574747266f754d9da`.
/// Merge of master ac23674e66 (Bundle B) into D58 moveout (GH #1476): both
/// edge changes at once, measured on the union, old (moveout)
/// `f9e1c439f3b62effeebbeb7b976ccdcbf4810b1936ba06c82c18c0e44cbb1f5d`, old (Bundle B)
/// `39e2786fe27be44f9930bfbd543cfceaf86c59620185325029b324a942e72f63`.
/// Re-blessed for the close of wave d58-2 (judge review M4, live delta m1):
/// the emit `view` says a target named `frame` or `topic` is refused with
/// `bad_arguments`, and `note_receipt` names the running session by its handle
/// `live` instead of the call -- wanted contract text, no edge moved, old
/// `e37dba7e0e9eaf11087f9f469a9313a8551d4f4425035880272ae123b92cce83`.
const CURATOR_SHA256: &str = "f28ba076b187976cf76a49727c30a54e47add7969442f29293178d8247fcc32f";

fn sorted(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let mut out = Map::new();
            for k in keys {
                out.insert(k.clone(), sorted(&m[k]));
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

fn digest(v: &Value) -> String {
    let text = sj::to_string(&sorted(v)).expect("JSON");
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

/// Pin 6: with `quick_speculative` "0" (shipped) A1 moves no byte but its
/// own declarations: talky's edges, `quick_spec`'s config and the curator
/// hive are the pre-A1 bytes once the new edges, lane, route and partial
/// declarations are taken out -- and those declarations are there, with
/// `partial_unit` "off" as shipped (`quick_spec` streams nothing). A spec
/// partial that reaches a curator with speculation off sends nothing.
#[test]
fn gh1411_speculation_off_moves_no_byte() {
    if !shipped() {
        return;
    }
    // talky
    let all = edges(TALKY);
    let new = |e: &Value| {
        is_partial_edge(e)
            && (e["from"] == "./quick_spec" || (e["from"] == "./curator" && e["to"] == "."))
    };
    // GH #1449 came after A1: the package lines into `./curator` on
    // `in_catalog` are their own lane, not the speculation's.
    let package_line = |e: &Value| {
        e["to"] == "./curator" && cond(e).starts_with("has(hop.route) && hop.route == 'package'")
    };
    // GH #1450 came after A1 too: the catalog question out of `./curator`
    // on `catalog_ask` is the catalog's own lane (OR-D53-53).
    let catalog_ask =
        |e: &Value| e["from"] == "./curator" && cond(e).contains("hop.route == 'catalog_ask'");
    // D58 lead (GH #1474): the lead is a hive of its own beside the curator;
    // the edges between them (the lanes to it, the verdict back on
    // `in_lead`, and since D58 moveout, GH #1476, its orders on `in_feed`)
    // are the lead's, not the speculation's.
    let lead_edge = |e: &Value| e["from"] == "./lead" || e["to"] == "./lead";
    assert_eq!(
        all.iter().filter(|e| lead_edge(e)).count(),
        3,
        "the three edges between talky's curator and its lead"
    );
    // D58 feed (GH #1481, contract point 7): talky hands an order to feed a
    // running session from its rim to the curator -- the feed's door, not
    // the speculation's.
    let feed_door = |e: &Value| {
        e["from"] == "." && e["to"] == "./curator" && cond(e).contains("hop.route == 'in_feed'")
    };
    assert_eq!(
        all.iter().filter(|e| feed_door(e)).count(),
        1,
        "talky's one door for an order to feed"
    );
    let mut old: Vec<Value> = all
        .iter()
        .filter(|e| {
            !new(e) && !package_line(e) && !catalog_ask(e) && !lead_edge(e) && !feed_door(e)
        })
        .cloned()
        .collect();
    // GH #1497 (review I2 of the prefix strand): the model call edges lift
    // the curator's refusal (`hop.tools_refused`) into the context for the
    // dispatcher; that one key is the refusal's, not this pin's.
    let mut lifted = 0;
    for e in old.iter_mut() {
        if let Some(ctx) = e
            .pointer_mut("/modifier/set_context")
            .and_then(Value::as_object_mut)
            && ctx.remove("tools_refused")
                == Some(json!("has(hop.tools_refused) ? hop.tools_refused : ''"))
        {
            lifted += 1;
        }
    }
    assert_eq!(lifted, 8, "the model call edges that lift the refusal");
    // GH #494 (gate of bundle D): the refusal is talky's interior key, so
    // every exit edge drops it beside `tools_hinted`; that entry is the
    // refusal's, not this pin's.
    let mut dropped = 0;
    for e in old.iter_mut() {
        if let Some(keys) = e
            .pointer_mut("/modifier/delete_context")
            .and_then(Value::as_array_mut)
        {
            let before = keys.len();
            keys.retain(|k| k != "tools_refused");
            dropped += before - keys.len();
        }
    }
    assert_eq!(dropped, 39, "the exit edges that drop the refusal");
    assert_eq!(
        digest(&Value::Array(old)),
        TALKY_EDGES_SHA256,
        "talky's edges moved beyond the spec partial"
    );
    assert_eq!(
        all.iter().filter(|e| new(e)).count(),
        2,
        "the two new edges"
    );

    // quick_spec
    let mut qs = read_json(&repo(QUICK_SPEC));
    assert_eq!(qs["params"]["partial_unit"], json!("off"), "off as shipped");
    assert!(qs["params"]["partial_min_chars"].is_number());
    for k in ["partial_unit", "partial_min_chars"] {
        assert!(qs["contract"]["settings"][k].is_object(), "setting {k}");
    }
    for k in ["seq", "final", "partial_of"] {
        assert!(
            qs["contract"]["emits"]["hop"][k].is_object(),
            "emits.hop.{k}"
        );
    }
    for k in ["partial_unit", "partial_min_chars"] {
        qs["params"].as_object_mut().expect("params").remove(k);
        qs["contract"]["settings"]
            .as_object_mut()
            .expect("settings")
            .remove(k);
    }
    for k in ["seq", "final", "partial_of"] {
        qs["contract"]["emits"]["hop"]
            .as_object_mut()
            .expect("hop")
            .remove(k);
    }
    // GH #1431: the two hop keys every llm cell declares since (the price
    // ladder and the cache-write price, `gh890_…`) -- not the speculation's.
    for k in ["cost_cache_write", "price_tiers"] {
        assert!(
            qs["contract"]["emits"]["hop"][k].is_object(),
            "emits.hop.{k}"
        );
        qs["contract"]["emits"]["hop"]
            .as_object_mut()
            .expect("hop")
            .remove(k);
    }
    // GH #1449: the package line (`package_line`, `hop.package_of`, the body
    // slot `package`, the route value `package`) -- not the speculation's.
    assert_eq!(qs["params"]["package_line"], json!(true), "package_line");
    qs["params"]
        .as_object_mut()
        .expect("params")
        .remove("package_line");
    qs["contract"]["emits"]["hop"]
        .as_object_mut()
        .expect("hop")
        .remove("package_of");
    if let Some(body) = qs["contract"]["emits"]["body"].as_object_mut() {
        body.remove("package");
    }
    if let Some(values) = qs["contract"]["emits"]["hop"]["route"]["values"].as_array_mut() {
        values.retain(|v| v != "partial" && v != "package");
    }
    // D58 target (GH #1483): the window profile the package line carries
    // (`window_profile` in params and settings, the body slots
    // `window_profile` and `system_order`) -- not the speculation's.
    // Its value is the fast target's profile (Bauer 6, OR-d58-2.target.16):
    // an object that names the cell's own instruction slot.
    assert_eq!(
        qs["params"]["window_profile"]["instructions"]["slot"],
        json!("quick"),
        "window_profile"
    );
    qs["params"]
        .as_object_mut()
        .expect("params")
        .remove("window_profile");
    qs["contract"]["settings"]
        .as_object_mut()
        .expect("settings")
        .remove("window_profile");
    if let Some(body) = qs["contract"]["emits"]["body"].as_object_mut() {
        body.remove("window_profile");
        body.remove("system_order");
    }
    qs.as_object_mut().expect("config").remove("description");
    assert_eq!(
        digest(&qs),
        QUICK_SPEC_SHA256,
        "quick_spec moved beyond its partial declarations"
    );

    // curator hive
    let cur = read_json(&repo(CURATOR));
    let p = &cur["params"];
    let c_edges: Vec<Value> = p["graph"]["edges"]
        .as_array()
        .expect("edges")
        .iter()
        .filter(|e| {
            !cond(e).contains("in_spec_partial")
                && !(e["from"] == "./policy" && is_partial_edge(e))
                // GH #1449: the door of the package line.
                && !cond(e).contains("'in_catalog'")
                // GH #1450: the catalog question out of the hive.
                && !cond(e).contains("'catalog_ask'")
        })
        // GH #1447: the lead's four edges (policy <-> lead <-> lead_state) --
        // a new cell of the hive, not the speculation's. GH #1474: the lead
        // is a hive of its own; its two rim edges (the lanes to it out of
        // `./policy`, the verdict `in_lead` back in) stand in their place.
        .filter(|e| {
            !["./lead", "./lead_state"].contains(&e["from"].as_str().unwrap_or(""))
                && !["./lead", "./lead_state"].contains(&e["to"].as_str().unwrap_or(""))
                && !cond(e).contains("'lead_ask'")
                && !cond(e).contains("'in_lead'")
                // D58 feed (GH #1481): the door of an order to feed a running
                // session -- the feed's, not the speculation's.
                && !cond(e).contains("'in_feed'")
        })
        .cloned()
        .collect();
    let lane = |list: &Value, route: &str| -> (Vec<Value>, usize) {
        let all = list.as_array().expect("list");
        let rest: Vec<Value> = all
            .iter()
            .filter(|a| a["route"] != route)
            .cloned()
            .collect();
        let n = all.len() - rest.len();
        (rest, n)
    };
    let (acc, n_acc) = lane(&p["contract"]["accepts"], "in_spec_partial");
    // GH #1449: the lane of the package line.
    let (acc, n_cat) = lane(&Value::Array(acc), "in_catalog");
    assert_eq!(n_cat, 1, "the package line's lane");
    // GH #1474: the lead's verdict lane and its three lanes out -- the
    // lead's, not the speculation's.
    let (acc, n_lead) = lane(&Value::Array(acc), "in_lead");
    assert_eq!(n_lead, 1, "the lead's verdict lane");
    // D58 feed (GH #1481): the lane of an order to feed a running session.
    let (acc, n_feed) = lane(&Value::Array(acc), "in_feed");
    assert_eq!(n_feed, 1, "the feed's lane");
    let (emi, n_emi) = lane(&p["contract"]["emits"], "partial");
    let mut emi = emi;
    for route in ["lead_ask", "lead_spec", "lead_note"] {
        let (rest, n) = lane(&Value::Array(emi), route);
        assert_eq!(n, 1, "the lane to the lead {route}");
        emi = rest;
    }
    assert_eq!((n_acc, n_emi), (1, 1), "the new lane and the new route");
    assert_eq!(
        digest(&json!({"edges": c_edges, "accepts": acc, "emits": emi})),
        CURATOR_SHA256,
        "the curator hive moved beyond the spec partial"
    );

    // Speculation off: a spec partial that arrives anyway sends nothing.
    let mut h = Hive::with(&[]).reporting_composite("talky");
    h.check_emits = true;
    let ctx = json!({"session_id": "s1", "turn_id": "t1", "iter": "0", "channel": "test",
                     "curator_call": "c-spec", "spec": "1"});
    h.lane(
        "in_spec_partial",
        ctx,
        Value::Object(partial_hop(1, "0190aaaa-0000-7000-8000-000000000001")),
        json!({"messages": [said("I am fine, thank you.")]}),
    );
    assert!(out_routes(&h).is_empty(), "{:?}", h.out);
    assert_eq!(h.emit_violations, Vec::<String>::new());
}

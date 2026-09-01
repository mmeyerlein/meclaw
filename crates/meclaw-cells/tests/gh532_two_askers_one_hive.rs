//! GH #532 — one memory hive, several askers, and every bundle finds the one
//! that asked.
//!
//! Since GH #122 a person's memory belongs to the MEMBER, and since GH #302 one
//! generation holds two askers side by side: the conversation surface
//! (`./talky`) and the reasoning core (`./cogny`). One hive, two askers, and
//! the answer has to come back to the one that asked — always.
//!
//! The constraint that shaped the mechanism is real and is checked one file
//! over:
//!
//! * **Only context survives the hive.** The `recall` cell forms its own hop, so
//!   a correlation key left on the hop is gone by the time the bundle comes back
//!   (GH #411, `gh411_member_promotes_memory_call_id.rs`).
//! * **A door guarded on context ALONE is condemned.** The template gate probes
//!   a hive's doors with a bare `hop.route` and an EMPTY context compartment
//!   (`gh173_shipped_hive_contracts.rs`,
//!   `every_lane_the_graph_opens_is_declared`). The inward carve-out of W7-R1
//!   (GH #286) exempts a door reading a HOP key the probe cannot carry; the
//!   context carve-out of GH #469 is on the exit side only.
//!
//! So the reply-to token travels in CONTEXT and arrives on the HOP, and the one
//! place that can change its compartment for every caller at once is the hive's
//! own exit. `context.recall_caller` goes out, `hop.recall_caller` comes back.
//!
//! What this file proves, and why it is one file rather than two: the first four
//! tests are facts about the shipped FILES, which is where a lost stamp would be
//! introduced; the last three drive the REAL router over the real templates —
//! the hive, the member, the `assistants` container as `examples/organism` wires
//! it, and the assistant — through a whole recall round trip in both directions,
//! twice at once. A file check alone would pass on a topology that cannot route;
//! a routing check alone would pass on a topology that routes by accident.
//!
//! No colony, no model, no store: the templates are read off the tree and the
//! router is asked what it would do.

use meclaw_colony::config::{EdgeSpec, HiveParams};
use meclaw_colony::edge_table::{Edge, EdgeTable, apply_edges};
use meclaw_core::serde_json::{Map, Value};
use meclaw_core::{Headers, Path, Uuid};

/// The member of the worked example, and the generation inside it. Any paths
/// do; these are the ones `examples/organism` uses, one segment shorter.
const MEMBER: &str = "/m";
const HIVE: &str = "/m/memory-hive";
const BOX: &str = "/m/assistants";
const GEN: &str = "/m/assistants/scribe";

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn read_json(rel: &str) -> Value {
    let p = repo(rel);
    let raw = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// The `params.graph.edges` of a shipped hive template, as the colony parses
/// them. Going through [`HiveParams`] rather than through raw JSON is the point:
/// `EdgeSpec` is `deny_unknown_fields`, so an edge key the boot would refuse
/// fails here instead of in a live colony.
fn hive_edges(rel: &str) -> Vec<EdgeSpec> {
    let cfg = read_json(rel);
    let params = cfg
        .get("params")
        .cloned()
        .unwrap_or_else(|| panic!("{rel}: no params"));
    let hp: HiveParams = meclaw_core::serde_json::from_value(params)
        .unwrap_or_else(|e| panic!("{rel}: params: {e}"));
    hp.graph.edges
}

/// The `add_edges` of an instantiation recipe, parsed the same strict way. The
/// per-assistant addressing edges live nowhere else: the `assistants` container
/// ships empty and open, and the mutation that instantiates a generation is what
/// draws them (`templates/member/assistants/config.json`).
fn recipe_edges(rel: &str) -> Vec<EdgeSpec> {
    let doc = read_json(rel);
    let raw = doc["diff"]["add_edges"]
        .as_array()
        .unwrap_or_else(|| panic!("{rel}: no diff.add_edges"))
        .clone();
    raw.into_iter()
        .map(|e| {
            meclaw_core::serde_json::from_value(e.clone())
                .unwrap_or_else(|err| panic!("{rel}: add_edges entry {e}: {err}"))
        })
        .collect()
}

/// Resolve a template-relative endpoint (`.` / `./child`) against the scope the
/// template was instantiated at — exactly what the colony does when it stages a
/// subtree.
fn abs(base: &str, endpoint: &str) -> String {
    match endpoint {
        "." => base.to_string(),
        other => format!("{base}/{}", other.trim_start_matches("./")),
    }
}

/// Add one template's (or one recipe's) edges to the table, rebased under
/// `base`.
fn add_edges(table: &mut EdgeTable, base: &str, specs: &[EdgeSpec], label: &str) {
    for spec in specs {
        let condition = spec.condition.as_ref().map(|src| {
            meclaw_colony::cel_eval::parse_condition(src)
                .unwrap_or_else(|e| panic!("{label}: condition {src:?}: {e}"))
        });
        let modifier = spec.modifier.as_ref().map(|m| {
            meclaw_colony::cel_eval::parse_modifier(m)
                .unwrap_or_else(|(k, e)| panic!("{label}: modifier {k}: {e}"))
        });
        table.insert(Edge {
            id: Uuid::now_v7(),
            from: Path::new(&abs(base, &spec.from)),
            to: Path::new(&abs(base, &spec.to)),
            condition,
            modifier,
            is_default: spec.is_default,
            lane: None,
        });
    }
}

/// The four levels of the shipped composition that a recall round trip crosses,
/// in one edge table: the member, its memory hive, the `assistants` container as
/// `examples/organism` wires it, and one generation.
fn shipped_table() -> EdgeTable {
    let mut t = EdgeTable::new();
    add_edges(
        &mut t,
        MEMBER,
        &hive_edges("templates/member/config.json"),
        "member",
    );
    add_edges(
        &mut t,
        HIVE,
        &hive_edges("templates/memory-hive/config.json"),
        "memory-hive",
    );
    add_edges(
        &mut t,
        GEN,
        &hive_edges("templates/assistant/config.json"),
        "assistant",
    );
    add_edges(
        &mut t,
        BOX,
        &recipe_edges("examples/organism/grow-assistant.json"),
        "grow-assistant",
    );
    t
}

fn headers(context: &[(&str, &str)], hop: &[(&str, &str)]) -> Headers {
    let map = |pairs: &[(&str, &str)]| -> Map<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), Value::String((*v).to_string())))
            .collect()
    };
    Headers::from_parts(map(context), map(hop))
}

fn hop_of(h: &Headers, key: &str) -> String {
    h.hop.get(key).and_then(|v| v.as_str()).unwrap_or("").into()
}

fn ctx_of(h: &Headers, key: &str) -> String {
    h.context
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .into()
}

/// Follow one message through the table until nothing takes it any further,
/// insisting at every step that exactly ONE edge does. A fan-out here would be
/// two bundles where the caller asked for one, and it is the failure this whole
/// mechanism exists to prevent — so it is an assertion, not a `Vec`.
fn walk(table: &EdgeTable, from: &str, headers: Headers) -> (Vec<String>, Headers) {
    let mut trace = vec![from.to_string()];
    let mut here = Path::new(from);
    let mut hs = headers;
    for _ in 0..24 {
        let out = apply_edges(table, &here, &hs);
        if out.is_empty() {
            return (trace, hs);
        }
        assert_eq!(
            out.len(),
            1,
            "at {} the message fans out to {:?} — a recall answer has exactly one addressee",
            here.as_str(),
            out.iter().map(|d| d.target.as_str()).collect::<Vec<_>>()
        );
        let d = out.into_iter().next().expect("checked non-empty");
        here = d.target;
        hs = d.headers_out;
        trace.push(here.as_str().to_string());
    }
    panic!("the walk did not settle in 24 hops: {trace:?}");
}

/// The request an asker inside the generation raises: the shape
/// `collector/assemble`'s `recall_ask` builds, with every key present and empty
/// rather than absent.
fn recall_request(call_id: &str) -> Headers {
    headers(
        &[
            ("assistant", "scribe"),
            ("session_id", "S-42"),
            ("audience_set", "[\"alex\"]"),
            ("channel", "chat:1"),
            ("turn_id", "S-42#7"),
        ],
        &[
            ("route", "recall"),
            ("recall_query", "what did we decide"),
            ("memory_tier", "1"),
            ("memory_call_id", call_id),
            ("recall_window_from", ""),
            ("recall_window_to", ""),
            ("turn_id", "S-42#7"),
        ],
    )
}

// ─────────────────────────────────────────────── facts about the shipped files

fn edge<'a>(specs: &'a [EdgeSpec], from: &str, to: &str, needle: &str) -> &'a EdgeSpec {
    let hits: Vec<&EdgeSpec> = specs
        .iter()
        .filter(|e| {
            e.from == from
                && e.to == to
                && e.condition.as_deref().is_some_and(|c| c.contains(needle))
        })
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "expected exactly one {from} -> {to} edge mentioning {needle:?}, found {}",
        hits.len()
    );
    hits[0]
}

#[test]
fn each_asker_stamps_its_own_name_on_the_way_out() {
    let specs = hive_edges("templates/assistant/config.json");
    for (from, expected) in [("./talky", "'talky'"), ("./cogny", "'cogny'")] {
        let e = edge(&specs, from, ".", "'recall'");
        let m = e
            .modifier
            .as_ref()
            .unwrap_or_else(|| panic!("{from} -> . on recall must carry a modifier"));
        assert_eq!(
            m.set_context.get("recall_caller").map(String::as_str),
            Some(expected),
            "{from}'s recall exit must stamp recall_caller — a token that is only written when it is \
             MISSING inherits the other asker's name off an `in_advice` turn"
        );
    }
}

#[test]
fn the_core_reads_a_hop_key_and_the_surface_is_the_default() {
    let specs = hive_edges("templates/assistant/config.json");
    let core = edge(&specs, ".", "./cogny", "in_bundle");
    let cond = core.condition.as_deref().expect("guarded");
    assert!(
        cond.contains("hop.recall_caller"),
        "the core's door must read the token off the HOP: a door guarded on context alone can never \
         fire under the gate's probe (gh173, every_lane_the_graph_opens_is_declared) — got {cond:?}"
    );
    assert!(
        !cond.contains("context."),
        "and it must not fall back to context, which would make the door untestable — got {cond:?}"
    );
    assert!(!core.is_default, "the guarded door is the REGULAR one");

    let surface = edge(&specs, ".", "./talky", "in_bundle");
    assert!(
        surface.is_default,
        "the surface's door must be the DEFAULT, so a bundle with no token, an empty one or an \
         unknown one lands where every bundle landed before instead of dead-lettering here"
    );
}

#[test]
fn the_hive_hands_the_token_back_on_the_hop_and_never_deletes_it() {
    let specs = hive_edges("templates/memory-hive/config.json");
    for lane in ["'bundle'", "'reject'"] {
        let e = edge(&specs, "./recall", ".", lane);
        let m = e
            .modifier
            .as_ref()
            .expect("the recall exits carry modifiers");
        let expr = m
            .set_hop
            .get("recall_caller")
            .unwrap_or_else(|| panic!("the {lane} exit must put the token on the hop"));
        assert!(
            expr.contains("context.recall_caller"),
            "it is read off the context, which is the only compartment that survives this hive \
             (GH #411) — got {expr:?}"
        );
        assert!(
            expr.contains("has(context.recall_caller)"),
            "an absent token must resolve to the empty string: a CEL error on a missing key SKIPS \
             the edge, and the answer would never leave the hive — got {expr:?}"
        );
        assert!(
            !m.delete_context.iter().any(|k| k == "recall_caller"),
            "the {lane} exit deletes the hive's own bookkeeping, never the caller's token"
        );
    }
}

#[test]
fn the_members_recall_doors_leave_the_session_alone() {
    // The tier-0 leg of `recall` reads `context.session_id` and answers with THAT
    // session's episodes; a door that promoted it off an absent `hop.session_id`
    // would blank a good value, and one that deleted it would send every recall
    // to a session called "default".
    let specs = hive_edges("templates/member/config.json");
    let doors: Vec<&EdgeSpec> = specs
        .iter()
        .filter(|e| {
            e.to == "./memory-hive"
                && e.modifier.as_ref().is_some_and(|m| {
                    m.set_hop
                        .get("route")
                        .is_some_and(|r| r.contains("in_query"))
                })
        })
        .collect();
    assert_eq!(doors.len(), 2, "two recall doors, as GH #411 found them");
    for d in doors {
        let m = d.modifier.as_ref().expect("guarded above");
        assert!(
            !m.set_context.contains_key("session_id"),
            "a recall door must not re-stamp the session"
        );
        assert!(
            !m.delete_context.iter().any(|k| k == "session_id"),
            "and must not drop it"
        );
    }

    let recall = read_json("templates/memory-hive/recall/config.json");
    let script = recall["params"]["script_inline"].as_str().expect("inline");
    assert!(
        script.contains("session_id = str(ctx.get(\"session_id\", \"default\"))"),
        "the recall cell reads the session off the context — if that moved, the assertion above \
         is guarding nothing"
    );
}

// ──────────────────────────────────────────────────── the router, end to end

#[test]
fn a_core_recall_reaches_the_hive_carrying_its_own_name() {
    let t = shipped_table();
    let (trace, arrived) = walk(&t, &format!("{GEN}/cogny"), recall_request("c-core-1"));
    assert_eq!(
        trace.last().map(String::as_str),
        Some("/m/memory-hive/recall"),
        "the core's recall must reach the hive's recall cell: {trace:?}"
    );
    assert_eq!(ctx_of(&arrived, "recall_caller"), "cogny");
    assert_eq!(
        ctx_of(&arrived, "session_id"),
        "S-42",
        "the session the core is asking about survives the four levels untouched"
    );
    assert_eq!(ctx_of(&arrived, "memory_call_id"), "c-core-1");
    assert_eq!(ctx_of(&arrived, "recall_query"), "what did we decide");
    assert_eq!(hop_of(&arrived, "route"), "in_query");
}

#[test]
fn two_simultaneous_recalls_come_home_to_the_two_askers() {
    let t = shipped_table();

    // Both legs leave at the same time, from the two occupants of ONE generation,
    // and neither knows about the other. Routing is per message, so "at the same
    // time" is exactly this: two chains, two contexts, one table.
    let (_, core_at_hive) = walk(&t, &format!("{GEN}/cogny"), recall_request("c-core"));
    let (_, surface_at_hive) = walk(&t, &format!("{GEN}/talky"), recall_request("c-surface"));
    assert_eq!(ctx_of(&core_at_hive, "recall_caller"), "cogny");
    assert_eq!(ctx_of(&surface_at_hive, "recall_caller"), "talky");

    // What the `recall` cell does when it answers: the substrate carries the
    // persistent context of the request into the emission and lets the cell form
    // its own hop (`Headers::carry_context_with_hop`). So the bundle starts with
    // the request's context and a hop of the cell's own making — which is
    // precisely why a correlation key on the hop would be lost here.
    let answer = |at_hive: &Headers, call_id: &str| -> Headers {
        Headers::from_parts(
            at_hive.context.clone(),
            [
                ("route", "bundle"),
                ("memory_call_id", call_id),
                ("turn_id", "S-42#7"),
            ]
            .iter()
            .map(|(k, v)| ((*k).to_string(), Value::String((*v).to_string())))
            .collect(),
        )
    };

    let hive_recall = format!("{HIVE}/recall");
    let (core_trace, core_home) = walk(&t, &hive_recall, answer(&core_at_hive, "c-core"));
    let (surface_trace, surface_home) =
        walk(&t, &hive_recall, answer(&surface_at_hive, "c-surface"));

    assert_eq!(
        core_trace.last().map(String::as_str),
        Some("/m/assistants/scribe/cogny"),
        "the core's bundle must reach the core: {core_trace:?}"
    );
    assert_eq!(
        surface_trace.last().map(String::as_str),
        Some("/m/assistants/scribe/talky"),
        "and the surface's must reach the surface: {surface_trace:?}"
    );
    assert_eq!(hop_of(&core_home, "route"), "in_bundle");
    assert_eq!(hop_of(&surface_home, "route"), "in_bundle");
    assert_eq!(
        hop_of(&core_home, "memory_call_id"),
        "c-core",
        "and each carries its own call id, which is what makes it a tool RESULT rather than the \
         ambient leg of some other turn (GH #78, GH #411)"
    );
    assert_eq!(hop_of(&surface_home, "memory_call_id"), "c-surface");
}

#[test]
fn a_bundle_with_no_token_still_goes_where_every_bundle_went() {
    // The compatibility half, and the reason the surface's door is a `default`
    // rather than a second guard: an outside caller, an older instance, a lane
    // nobody stamped. None of them dead-letters at the generation's path.
    let t = shipped_table();
    for token in ["", "talky", "somebody-else"] {
        let mut hop = vec![("route", "in_bundle"), ("turn_id", "S-42#7")];
        if !token.is_empty() {
            hop.push(("recall_caller", token));
        }
        let (trace, _) = walk(
            &t,
            BOX,
            headers(&[("assistant", "scribe"), ("session_id", "S-42")], &hop),
        );
        assert_eq!(
            trace.last().map(String::as_str),
            Some("/m/assistants/scribe/talky"),
            "token {token:?} must land on the surface: {trace:?}"
        );
    }
}

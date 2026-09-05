//! GH #533 — the member's `in_recall` lane finally has the exit it promised.
//!
//! `member@1.4.0` accepted a question against this person's memory *from
//! outside the member* and said, in its own `because`, that the answer "comes
//! back on `bundle` through whatever edge the caller drew". There was no such
//! edge and nowhere to draw one: `bundle` was not in the member's `emits`, so
//! the answer took the ONE return edge the level had —
//! `./memory-hive -> ./assistants` — and died inside the container as
//! `no_route`, or, worse, was handed to whichever generation a stray
//! `context.assistant` happened to name. The lane had been a promise the level
//! could not keep since it shipped.
//!
//! The mechanism that fixes it is [ADR-0019]'s reply-to token, built in GH
//! #532: the asker stamps `context.<lane>_caller`, everything in between
//! carries it untouched, and the hive's own exit hands it back on the hop. The
//! outside asker is simply a third value — `'outside'`, stamped by the member's
//! own door, because at this boundary the value space of the token is this
//! level's.
//!
//! Four things had to be true at once, and this file checks each of them where
//! it can go wrong:
//!
//! 1. the member DECLARES `bundle` and pairs it with `in_recall`, so a caller
//!    that asks and does not subscribe is told at the mutation;
//! 2. the exit is guarded on the token and the way DOWN is the `default`, so an
//!    assistant's bundle, an unknown token and no token at all keep landing
//!    exactly where every bundle landed before;
//! 3. a `reject` reaches the asker in both directions — down into the
//!    generation as `in_bundle` carrying `hop.reject_reason`, so a refused
//!    recall is a typed error in the round instead of an idle window; out of
//!    the level for the outside asker;
//! 4. `org` and `meclaw-os` carry the new lane out, or an outside recall
//!    answered at the member dies one level UP instead of one level down.
//!
//! Like `gh532_two_askers_one_hive.rs`, the first half are facts about the
//! shipped FILES and the second half drives the REAL router over them, all six
//! levels of the worked example at once. No colony, no model, no store.
//!
//! [ADR-0019]: ../../../plans/adr/0019-one-hive-several-askers-the-reply-to-token.md

use meclaw_colony::config::{EdgeSpec, HiveParams};
use meclaw_colony::edge_table::{Edge, EdgeTable, apply_edges};
use meclaw_core::serde_json::{Map, Value};
use meclaw_core::{Headers, Path, Uuid};

/// The worked example of `examples/organism`, six levels deep — the shell, the
/// namespace, the person, their memory, the container and one generation.
const OS: &str = "/os";
const ORGS: &str = "/os/orgs";
const ORG: &str = "/os/orgs/acme";
const MEMBERS: &str = "/os/orgs/acme/members";
const MEMBER: &str = "/os/orgs/acme/members/alex";
const HIVE: &str = "/os/orgs/acme/members/alex/memory-hive";
const RECALL: &str = "/os/orgs/acme/members/alex/memory-hive/recall";
const WRITER: &str = "/os/orgs/acme/members/alex/memory-hive/writer";
const BOX: &str = "/os/orgs/acme/members/alex/assistants";
const GEN: &str = "/os/orgs/acme/members/alex/assistants/scribe";

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

/// The `params` of a shipped hive template, as the colony parses them —
/// `HiveParams` and `EdgeSpec` are `deny_unknown_fields`, so a key the boot
/// would refuse fails here instead of in a live colony.
fn hive_params(rel: &str) -> HiveParams {
    let cfg = read_json(rel);
    let params = cfg
        .get("params")
        .cloned()
        .unwrap_or_else(|| panic!("{rel}: no params"));
    meclaw_core::serde_json::from_value(params).unwrap_or_else(|e| panic!("{rel}: params: {e}"))
}

fn hive_edges(rel: &str) -> Vec<EdgeSpec> {
    hive_params(rel).graph.edges
}

/// The lane lists of a shipped template's contract.
fn lanes(rel: &str) -> (Vec<String>, Vec<String>) {
    let c = hive_params(rel)
        .contract
        .unwrap_or_else(|| panic!("{rel}: no contract"));
    (
        c.accepts.iter().map(|l| l.route.clone()).collect(),
        c.emits.iter().map(|l| l.route.clone()).collect(),
    )
}

/// The `add_edges` of an instantiation recipe, parsed the same strict way.
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

fn abs(base: &str, endpoint: &str) -> String {
    match endpoint {
        "." => base.to_string(),
        other => format!("{base}/{}", other.trim_start_matches("./")),
    }
}

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

/// The whole of `examples/organism` as one edge table: three shipped levels,
/// the memory hive, one generation and the three recipes that wire them.
fn shipped_table() -> EdgeTable {
    let mut t = EdgeTable::new();
    add_edges(
        &mut t,
        OS,
        &hive_edges("templates/meclaw-os/config.json"),
        "meclaw-os",
    );
    add_edges(&mut t, ORG, &hive_edges("templates/org/config.json"), "org");
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
        ORGS,
        &recipe_edges("examples/organism/grow-org.json"),
        "grow-org",
    );
    add_edges(
        &mut t,
        MEMBERS,
        &recipe_edges("examples/organism/grow-member.json"),
        "grow-member",
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

/// Follow one message until nothing takes it further, insisting that exactly
/// ONE edge does at every step. A fan-out on this lane is two answers to one
/// question, which is what the guarded exit and the `default` beside it exist
/// to prevent — so it is an assertion, not a `Vec`.
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

// ─────────────────────────────────────────────── facts about the shipped files

#[test]
fn the_member_declares_the_lane_and_owes_its_caller_the_answer() {
    let (accepts, emits) = lanes("templates/member/config.json");
    assert!(accepts.iter().any(|l| l == "in_recall"));
    assert!(
        emits.iter().any(|l| l == "bundle"),
        "the member accepts a question from outside and must DECLARE the answer — an exit edge \
         without the lane is a level lying in the other direction, and \
         gh302_member_holds_the_memory reads both halves. Got {emits:?}"
    );

    let drains = hive_params("templates/member/config.json")
        .required_drains
        .unwrap_or_default();
    assert!(
        drains.iter().any(|d| matches!(
            d,
            meclaw_colony::config::DrainSpec::Lane(l)
                if l.accepts == "in_recall" && l.emits == "bundle"
        )),
        "the question and the answer are one pairing: a caller that stamps `in_recall` into this \
         level and takes no `bundle` off it has rebuilt GH #533 one level up. Got {drains:?}"
    );
}

#[test]
fn the_member_stamps_the_outside_asker_at_its_own_door() {
    let specs = hive_edges("templates/member/config.json");
    let door = edge(&specs, ".", "./memory-hive", "in_recall");
    let m = door.modifier.as_ref().expect("the door carries a modifier");
    assert_eq!(
        m.set_context.get("recall_caller").map(String::as_str),
        Some("'outside'"),
        "the door STAMPS the token, it does not carry what the caller sent: the value space of \
         `recall_caller` at this boundary is this level's, and a token only written when it is \
         missing is inherited from whatever chain last set it (ADR-0019). An outside caller with \
         askers of its own tells its rounds apart on a hop key of its own, which rides through \
         this hive untouched"
    );
    // A `memory_call_id` was promoted here until GH #552, because the shipped
    // collector served `memory_recall` itself and told a called bundle apart from
    // the ambient leg by it. Both halves live in the memory hive now, so this
    // lane has one meaning again and the door promotes no correlation at all.
    assert!(
        !m.set_context.contains_key("memory_call_id"),
        "the `in_query` lane has ONE meaning since GH #552 — a question — and the hive's own \
         `bundle` exit branches on exactly this key to tell a tool round from the ambient one: \
         a member door that set it would send every outside question through the adapter"
    );
}

#[test]
fn the_exit_is_guarded_on_the_token_and_the_way_down_is_the_default() {
    let specs = hive_edges("templates/member/config.json");

    let out = edge(&specs, "./memory-hive", ".", "'bundle'");
    let cond = out.condition.as_deref().expect("guarded");
    assert!(
        cond.contains("hop.recall_caller") && cond.contains("'outside'"),
        "the exit takes the OUTSIDE asker's bundle and nothing else. It reads the hop, which is \
         the compartment the hive hands the token back in (ADR-0019) — got {cond:?}"
    );
    assert!(
        !out.is_default,
        "the guarded exit is the REGULAR edge; the default is the way down"
    );
    assert_eq!(
        out.modifier
            .as_ref()
            .and_then(|m| m.set_hop.get("route"))
            .map(String::as_str),
        Some("'bundle'"),
        "and it RESTATES its own lane — a no-op for the message and the only way the contract \
         check can see it: `hive_contract::exit_exists` probes an exit with a bare `hop.route`, \
         which this guard can never satisfy, and GH #176's carve-out counts an edge that NAMES \
         the lane it carries. Without the restamp the member declares an emit the substrate says \
         it has no exit for, and gh173 refuses the template"
    );

    let down = edge(&specs, "./memory-hive", "./assistants", "'bundle'");
    assert!(
        down.is_default,
        "the way down must be the DEFAULT: an assistant's token, an unknown one and none at all \
         land where every bundle landed before this lane existed, instead of dead-lettering at \
         the member's own path"
    );
}

#[test]
fn a_refused_recall_has_a_way_back_to_the_asker_that_made_it() {
    let specs = hive_edges("templates/member/config.json");

    let down = edge(&specs, "./memory-hive", "./assistants", "'reject'");
    let cond = down.condition.as_deref().expect("guarded");
    assert!(
        cond.contains("hop.recall_caller") && cond.contains("!= 'outside'"),
        "a refusal of a recall an ASKER INSIDE raised goes back down — told from the hive's other \
         refusals by the reply-to token, which only the recall exits stamp — got {cond:?}"
    );
    assert_eq!(
        down.modifier
            .as_ref()
            .and_then(|m| m.set_hop.get("route"))
            .map(String::as_str),
        Some("'in_bundle'"),
        "and it arrives on the lane the assistant already sorts by token (GH #532) and the \
         collector already ends its memory leg on: `hop.reject_reason` rides along, so the round \
         sees a typed refusal instead of waiting out its idle window for a bundle that will never \
         come. A lane of its own would have to be wired by every parent and every recipe for a \
         message that carries no new shape"
    );

    let out = edge(&specs, "./memory-hive", ".", "'reject'");
    let cond = out.condition.as_deref().expect("guarded");
    assert!(
        cond.contains("!has(hop.recall_caller)") && cond.contains("'outside'"),
        "…and the exit keeps every OTHER refusal this hive raises — the writer's, the porter's, \
         the close pass's, none of which carry the token — plus the outside asker's own. The two \
         are mutually exclusive by construction rather than by routing phase, because the hive \
         has exactly one default and it is the bundle's — got {cond:?}"
    );
}

#[test]
fn the_two_levels_above_carry_the_answer_out() {
    for (rel, container) in [
        ("templates/org/config.json", "./members"),
        ("templates/meclaw-os/config.json", "./orgs"),
    ] {
        let (_, emits) = lanes(rel);
        assert!(
            emits.iter().any(|l| l == "bundle"),
            "{rel} accepts `in_recall` and must carry the answer out: a lane that goes down and \
             does not come back is a question with no answer. Got {emits:?}"
        );
        let specs = hive_edges(rel);
        edge(&specs, container, ".", "'bundle'");
    }
}

#[test]
fn the_shipped_recipes_draw_the_drain_they_owe() {
    // The `required_drains` trigger reads the caller's own `set_hop.route`
    // (GH #237), and a level is addressed by CONDITION rather than by stamp —
    // so the declaration cannot see these three, and this is what does.
    for (rel, child) in [
        ("examples/organism/grow-member.json", "./alex"),
        ("examples/organism/grow-org.json", "./acme"),
    ] {
        let specs = recipe_edges(rel);
        edge(&specs, child, ".", "'bundle'");
    }

    let manifest = read_json("examples/organism/grow.manifest.json");
    let entries = manifest["manifest"]
        .as_array()
        .expect("the manifest is a list of entries");
    for child in ["./acme", "./alex"] {
        let drawn = entries.iter().any(|entry| {
            entry["diff"]["add_edges"].as_array().is_some_and(|edges| {
                edges.iter().any(|e| {
                    e["from"] == *child
                        && e["to"] == "."
                        && e["condition"]
                            .as_str()
                            .is_some_and(|c| c.contains("'bundle'"))
                })
            })
        });
        assert!(
            drawn,
            "the one-shot manifest grows the same levels as the per-step recipes and must draw \
             the same edges — {child} has no bundle drain"
        );
    }
}

// ──────────────────────────────────────────────────── the router, end to end

/// The shape an outside asker sends: the recall keys the member's door reads,
/// and the round it is asking in. Nothing names a generation — that is the
/// whole point of the lane.
fn outside_recall() -> Headers {
    headers(
        &[("audience_set", "[\"alex\"]"), ("channel", "chat:1")],
        &[
            ("route", "in_recall"),
            ("recall_query", "what did we decide"),
            ("memory_tier", "1"),
            ("recall_window_from", ""),
            ("recall_window_to", ""),
        ],
    )
}

/// What the `recall` cell emits: the substrate carries the request's persistent
/// context into the emission and lets the cell form its OWN hop
/// (`Headers::carry_context_with_hop`) — which is precisely why the token has to
/// travel in context and come back on the hop.
fn hive_answer(at_hive: &Headers, route: &str, extra: &[(&str, &str)]) -> Headers {
    let mut hop: Vec<(&str, &str)> = vec![("route", route)];
    hop.extend_from_slice(extra);
    Headers::from_parts(
        at_hive.context.clone(),
        hop.iter()
            .map(|(k, v)| ((*k).to_string(), Value::String((*v).to_string())))
            .collect(),
    )
}

#[test]
fn a_question_from_outside_reaches_the_memory_carrying_the_levels_own_token() {
    let t = shipped_table();
    let (trace, arrived) = walk(&t, OS, outside_recall());
    assert_eq!(
        trace.last().map(String::as_str),
        Some(RECALL),
        "an `in_recall` at the shell must reach the person's recall cell, five levels down: \
         {trace:?}"
    );
    assert_eq!(
        ctx_of(&arrived, "recall_caller"),
        "outside",
        "and it arrives stamped as the third value of the reply-to token"
    );
    assert_eq!(ctx_of(&arrived, "audience_now"), "[\"alex\"]");
    assert_eq!(hop_of(&arrived, "route"), "in_query");
}

#[test]
fn the_bundle_leaves_the_member_and_the_two_levels_above_it() {
    let t = shipped_table();
    let (_, at_hive) = walk(&t, OS, outside_recall());

    let (trace, home) = walk(&t, RECALL, hive_answer(&at_hive, "bundle", &[]));
    assert_eq!(
        trace.last().map(String::as_str),
        Some(OS),
        "the answer to a question the shell let through must come back OUT of the shell — the \
         defect was that it went down into `./assistants` and died there: {trace:?}"
    );
    assert!(
        !trace.iter().any(|p| p.starts_with(BOX)),
        "and it must never enter the assistants container on the way: {trace:?}"
    );
    assert_eq!(
        hop_of(&home, "route"),
        "bundle",
        "it leaves on the lane the three levels now declare, untranslated"
    );
    assert_eq!(
        hop_of(&home, "recall_caller"),
        "outside",
        "carrying the token that addressed it, which no level in between reads"
    );
    // The caller's own correlation key rides on the hop, untouched by every level
    // it passes: `recall_caller` is what this road addresses by, and the id a
    // caller with several askers of its own uses is that caller's business.
}

#[test]
fn a_refusal_of_an_outside_question_comes_home_the_same_way() {
    let t = shipped_table();
    let (_, at_hive) = walk(&t, OS, outside_recall());

    let (trace, home) = walk(
        &t,
        RECALL,
        hive_answer(
            &at_hive,
            "reject",
            &[("reject_reason", "missing_audience"), ("phase", "request")],
        ),
    );
    assert_eq!(
        trace.last().map(String::as_str),
        Some(OS),
        "a refused question is an ANSWER for the asker: {trace:?}"
    );
    assert_eq!(hop_of(&home, "route"), "reject");
    assert_eq!(hop_of(&home, "reject_reason"), "missing_audience");
}

#[test]
fn an_askers_bundle_inside_the_member_goes_exactly_where_it_always_went() {
    // The compatibility half. The member's new exit is guarded on its OWN
    // token, so the level below keeps its whole vocabulary — including tokens
    // this level has never heard of.
    let t = shipped_table();
    for (token, target) in [
        ("cogny", format!("{GEN}/cogny")),
        ("talky", format!("{GEN}/talky")),
        ("", format!("{GEN}/talky")),
        ("somebody-else", format!("{GEN}/talky")),
    ] {
        let (trace, home) = walk(
            &t,
            HIVE,
            headers(
                &[("assistant", "scribe"), ("session_id", "S-42")],
                &[
                    ("route", "bundle"),
                    ("recall_caller", token),
                    ("turn_id", "S-42#7"),
                ],
            ),
        );
        assert_eq!(
            trace.last().map(String::as_str),
            Some(target.as_str()),
            "token {token:?} must land on {target}: {trace:?}"
        );
        assert_eq!(hop_of(&home, "route"), "in_bundle");
    }
}

#[test]
fn a_refused_recall_reaches_the_asker_instead_of_its_idle_window() {
    let t = shipped_table();
    for (token, target) in [
        ("cogny", format!("{GEN}/cogny")),
        ("talky", format!("{GEN}/talky")),
        ("", format!("{GEN}/talky")),
    ] {
        let (trace, home) = walk(
            &t,
            HIVE,
            headers(
                &[("assistant", "scribe"), ("session_id", "S-42")],
                &[
                    ("route", "reject"),
                    ("recall_caller", token),
                    ("reject_reason", "missing_channel"),
                    ("turn_id", "S-42#7"),
                ],
            ),
        );
        assert_eq!(
            trace.last().map(String::as_str),
            Some(target.as_str()),
            "a refused recall of {token:?} must reach the one that asked: {trace:?}"
        );
        assert_eq!(
            hop_of(&home, "route"),
            "in_bundle",
            "on the lane the collector already ends its memory leg on"
        );
        assert_eq!(
            hop_of(&home, "reject_reason"),
            "missing_channel",
            "with the reason still on the hop — the round reads the refusal without opening the \
             body"
        );
    }
}

#[test]
fn every_other_refusal_of_this_hive_leaves_the_level_as_it_always_did() {
    // The writer, the porter and the close pass raise `reject` too and carry no
    // reply-to token at all. Sorting a recall's refusal must not divert theirs
    // into a container that has no lane for them.
    let t = shipped_table();
    let (trace, home) = walk(
        &t,
        WRITER,
        headers(
            &[("session_id", "S-42")],
            &[("route", "reject"), ("reject_reason", "missing_audience")],
        ),
    );
    assert_eq!(
        trace.last().map(String::as_str),
        Some(OS),
        "a write this hive refused leaves the member, the org and the shell, exactly as before \
         GH #533: {trace:?}"
    );
    assert_eq!(hop_of(&home, "route"), "reject");
}

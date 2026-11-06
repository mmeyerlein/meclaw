//! GH #1411 (Pin 1) -- a model's `partial` reaches the surface channels and
//! nothing else.
//!
//! The `partial` lane of an `llm` cell (`docs/cell-types.md` § llm) carries a
//! half answer. Only the surface may show it: a ledger, a window, a memory, a
//! counselor or an app that held it would hold words the person may never be
//! told. The route name is shared: the voice cell's caller transcript runs on
//! `partial` as well, without `partial_of`; a model partial always carries
//! `partial_of`. This lock reads the instantiated road of the lane -- the
//! shipped `talky`, `assistant` and `member` edges and the edges `grow_level`
//! renders for a generation and a channel and `install_app` renders for an
//! app that listens on `partial` -- and walks it as the substrate does: a
//! message at a node fires EVERY edge out of it, a hive node (`.`,
//! `./channels`) included, in whichever direction the edge points. For every
//! node a model partial can stand on ([`ON_ROAD`]) it holds:
//!
//! 1. an edge on [`ROADS`] (model -> talky rim -> assistant rim ->
//!    `./channels` -> the channel node) is drawn exactly once, names the route
//!    `partial` and refuses `hop.finish_reason`; from `./assistants` on, where
//!    the road meets the hive `./channels` the caller's transcript crosses as
//!    well, it requires `hop.partial_of` ([`MODEL_ONLY`]). The talky and
//!    assistant exits cannot: the lane-door check probes an emitted lane with
//!    `hop.route` alone (`hive_contract`, `exit_exists`), and inside them only a
//!    model emits the route;
//! 2. every other edge out of the node is closed to a model partial: a
//!    top-level conjunct requires `hop.finish_reason` or
//!    `hop.refused_subscriber` (a partial carries neither), refuses
//!    `hop.partial_of`, or pins the route to an equality (or a list of them)
//!    without `partial`.
//!
//! By induction a model partial then stands on no other node: it never
//! reaches `./splitter`, `./dispatcher`, `./collector`, `./curator`,
//! `./apps`, memory or a peer. Only the lane's own edges and the closure of
//! the nodes on its road are locked; no other edge is hashed.
//! Static: no colony, no provider.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_all, shipped_script};
use std::path::PathBuf;

const MEMBER: &str = "/os/orgs/acme/members/alex";
const AGENT: &str = "scribe";
const CHANNEL: &str = "telegram";
const APP: &str = "probe";

/// The roads of a model partial: (where, from, to).
const ROADS: &[(&str, &str, &str)] = &[
    ("talky", "./brain", "."),
    ("talky", "./quick", "."),
    // GH #1411 A1: a kept speculation streams -- through the curator, which
    // passes a spec partial only while the turn awaits it (curator_2
    // `gh1411_a_kept_speculation_streams`), then out of the curator's rim.
    ("talky", "./quick_spec", "./curator"),
    ("talky", "./curator", "."),
    ("curator", "./policy", "."),
    ("assistant", "./talky", "."),
    ("assistant", "./talky-chat", "."),
    ("member", "./assistants", "./channels"),
    (
        "grow_level assistant",
        "./assistants/scribe",
        "./assistants",
    ),
    ("grow_level channel", "./channels", "./channels/telegram"),
];

/// The roads that tell a model's partial from the caller's by
/// `hop.partial_of`: the ones that reach `./channels` and leave it.
const MODEL_ONLY: &[&str] = &["member", "grow_level channel"];

/// The nodes a model partial stands on, per level: the model cells, the
/// rims, and the hives it crosses.
const ON_ROAD: &[(&str, &[&str])] = &[
    (
        "talky",
        &["./quick", "./brain", "./quick_spec", "./curator", "."],
    ),
    // Inside the curator a spec partial runs on `in_spec_partial` from the
    // hive path to `./policy`; what leaves `./policy` is a model partial.
    ("curator", &["./policy"]),
    ("assistant", &["./talky", "./talky-chat", "."]),
    ("member", &["./assistants", "./channels"]),
    (
        "grow_level assistant",
        &["./assistants/scribe", "./assistants"],
    ),
    ("grow_level channel", &["./channels"]),
    ("install_app", &["./channels"]),
];

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn edges_of(template: &str) -> Vec<Value> {
    let p = repo(&format!("templates/{template}/config.json"));
    let raw = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    let cfg: Value = meclaw_core::serde_json::from_str(&raw).expect("config json");
    cfg["params"]["graph"]["edges"]
        .as_array()
        .unwrap_or_else(|| panic!("{template}: no edges"))
        .clone()
}

/// The edges `recipe` renders for `params`, paths made relative to the
/// member so they read like the member's own.
fn grown(recipe: &str, params: Value) -> Vec<Value> {
    let out = emit_all(
        &shipped_script(
            repo("templates/builder/recipes/config.json")
                .to_str()
                .expect("utf-8"),
        ),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!({"recipe": recipe, "request": "…",
                                         "params": params}).to_string()}],
        }),
    );
    let first = out.first().expect("an emission");
    assert!(first["header"]["error_code"].is_null(), "refused: {first}");
    let decl = &first["manifest"][0];
    let scope = decl["scope"].as_str().expect("scope");
    let rel = |p: &str| -> String {
        let abs = if p.starts_with('/') {
            p.to_string()
        } else if p == "." {
            // the declaration's own scope: `.` is the container itself
            scope.to_string()
        } else {
            format!("{scope}/{}", p.trim_start_matches("./"))
        };
        abs.strip_prefix(&format!("{MEMBER}/"))
            .map(|r| format!("./{r}"))
            .unwrap_or(abs)
    };
    decl["diff"]["add_edges"]
        .as_array()
        .expect("add_edges")
        .iter()
        .map(|e| {
            let mut e = e.clone();
            e["from"] = json!(rel(e["from"].as_str().unwrap()));
            e["to"] = json!(rel(e["to"].as_str().unwrap()));
            e
        })
        .collect()
}

fn levels() -> Vec<(&'static str, Vec<Value>)> {
    vec![
        ("talky", edges_of("talky")),
        ("curator", edges_of("curator")),
        ("assistant", edges_of("assistant")),
        ("member", edges_of("member")),
        (
            "grow_level assistant",
            grown(
                "grow_level",
                json!({"scope": MEMBER, "level": "assistant", "name": AGENT,
                       "template": "a-template@1.0.0"}),
            ),
        ),
        (
            "grow_level channel",
            grown(
                "grow_level",
                json!({"scope": MEMBER, "level": "channel", "name": CHANNEL,
                       "template": "telegram-connector@2.2.7", "assistant": AGENT,
                       "bind_chat": "4711", "ctx": {"member_person": "alex"}}),
            ),
        ),
        (
            "install_app",
            grown(
                "install_app",
                json!({"scope": MEMBER, "app": APP, "template": "probe@1.0.0",
                       "screen": "display", "generation": AGENT,
                       "ctx": {"member_person": "alex"},
                       "declaration": {"listens": ["partial"]}}),
            ),
        ),
    ]
}

fn cond(e: &Value) -> &str {
    e["condition"].as_str().unwrap_or("")
}

/// `s` split at the top-level occurrences of `sep` (outside parentheses and
/// quotes).
fn split_top<'a>(s: &'a str, sep: &str) -> Vec<&'a str> {
    let (mut depth, mut quoted, mut start, mut out) = (0i32, false, 0usize, Vec::new());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\'' => quoted = !quoted,
            b'(' if !quoted => depth += 1,
            b')' if !quoted => depth -= 1,
            _ if !quoted && depth == 0 && s[i..].starts_with(sep) => {
                out.push(s[start..i].trim());
                i += sep.len();
                start = i;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    out.push(s[start..].trim());
    out
}

/// `s` without one pair of parentheses around the whole of it.
fn unwrap(s: &str) -> &str {
    let s = s.trim();
    if !s.starts_with('(') {
        return s;
    }
    // the first `(` must close at the very end, not earlier
    let (mut depth, mut quoted) = (0i32, false);
    for (i, c) in s.char_indices() {
        match c {
            '\'' => quoted = !quoted,
            '(' if !quoted => depth += 1,
            ')' if !quoted => {
                depth -= 1;
                if depth == 0 {
                    return if i == s.len() - 1 { s[1..i].trim() } else { s };
                }
            }
            _ => {}
        }
    }
    s
}

/// The top-level conjuncts of a condition, `None` when a top-level `||`
/// would open it past any of them.
fn conjuncts(c: &str) -> Option<Vec<&str>> {
    let c = unwrap(c);
    if split_top(c, "||").len() > 1 {
        return None;
    }
    Some(split_top(c, "&&"))
}

/// `true` when `t` lets only routes other than `partial` through: an
/// equality `hop.route == '<x>'`, or a disjunction whose every branch starts
/// with one.
fn pins_route_off_partial(t: &str) -> bool {
    let t = unwrap(t);
    let branches = split_top(t, "||");
    branches.iter().all(|b| {
        let b = unwrap(b);
        let first = split_top(b, "&&")[0];
        let first = unwrap(first);
        if first != b {
            return pins_route_off_partial(first);
        }
        match first.strip_prefix("hop.route == '") {
            Some(rest) => rest
                .strip_suffix('\'')
                .is_some_and(|x| !x.contains('\'') && x != "partial"),
            None => false,
        }
    })
}

fn closed(c: &str) -> bool {
    conjuncts(c).is_some_and(|cs| {
        cs.iter().any(|t| {
            *t == "has(hop.finish_reason)"
                || *t == "has(hop.refused_subscriber)"
                || *t == "!has(hop.partial_of)"
                || pins_route_off_partial(t)
                || closed_disjunction(t)
        })
    })
}

/// `true` when `t` is a parenthesised disjunction whose every branch is
/// itself closed. GH #1440: talky's self-summary edge takes
/// `(has(hop.finish_reason) || has(hop.refused_subscriber))` -- a partial
/// carries neither key, so no branch can let one through.
fn closed_disjunction(t: &str) -> bool {
    let inner = unwrap(t);
    if inner == t.trim() {
        return false;
    }
    let branches = split_top(inner, "||");
    branches.len() > 1 && branches.iter().all(|b| closed(b))
}

fn has_conjunct(c: &str, t: &str) -> bool {
    conjuncts(c).is_some_and(|cs| cs.contains(&t))
}

#[test]
fn partials_reach_only_surface_channels() {
    let mut seen: Vec<(String, String, String)> = Vec::new();
    for (level, edges) in levels() {
        let on_road = ON_ROAD
            .iter()
            .find(|(l, _)| *l == level)
            .map(|(_, n)| *n)
            .unwrap_or_else(|| panic!("no road nodes for {level}"));
        for e in &edges {
            let from = e["from"].as_str().unwrap_or("");
            if !on_road.contains(&from) {
                continue;
            }
            let road = (
                level.to_string(),
                from.to_string(),
                e["to"].as_str().unwrap_or("").to_string(),
            );
            let c = cond(e);
            let is_road = c.contains("hop.route == 'partial'")
                && ROADS.iter().any(|(l, f, t)| {
                    (*l, *f, *t) == (road.0.as_str(), road.1.as_str(), road.2.as_str())
                });
            if !is_road {
                assert!(
                    closed(c),
                    "{level}: {from} -> {} could fire on a model partial: {c}",
                    road.2
                );
                continue;
            }
            // The generation's exit into its container is the answer's own
            // shape (`_out`): the assistant rim in front of it already refused
            // a finished answer and a caller's partial, so it is the one road
            // without the guards.
            if level == "grow_level assistant" {
                assert_eq!(
                    c, "has(hop.route) && hop.route == 'partial'",
                    "the generation's partial exit is the answer's shape: {road:?}"
                );
            } else {
                assert!(
                    has_conjunct(c, "!has(hop.finish_reason)"),
                    "a partial edge must refuse a finished answer: {road:?}: {c}"
                );
                assert_eq!(
                    has_conjunct(c, "has(hop.partial_of)"),
                    MODEL_ONLY.contains(&level),
                    "only the roads at `./channels` tell a model's partial by `partial_of`: {road:?}: {c}"
                );
            }
            seen.push(road);
        }
    }
    for (l, f, t) in ROADS {
        let n = seen
            .iter()
            .filter(|(a, b, c)| (a.as_str(), b.as_str(), c.as_str()) == (*l, *f, *t))
            .count();
        assert_eq!(
            n, 1,
            "the road {l}: {f} -> {t} is drawn exactly once, drawn {n}"
        );
    }
}

#[test]
fn the_closure_reads_a_condition_as_the_substrate_does() {
    for open in [
        "has(hop.route) && hop.route != 'answer'",
        "has(hop.route) && hop.route == 'partial'",
        "has(hop.route) && (hop.route == 'answer' || hop.route == 'partial')",
        "has(hop.route) && hop.route == 'answer' || true",
        "has(hop.finish_reason) || has(hop.route)",
        "has(context.summarize) && (has(hop.finish_reason) || has(hop.route))",
        "",
    ] {
        assert!(!closed(open), "open read as closed: {open}");
    }
    for shut in [
        "has(hop.route) && hop.route == 'answer'",
        "has(hop.route) && (hop.route == 'in_tool' || hop.route == 'in_menu') && has(context.tool_caller)",
        "has(hop.route) && ((hop.route == 'event' && has(hop.owner)) || (hop.route == 'receipt' && has(hop.owner)))",
        "has(hop.finish_reason) && hop.finish_reason == 'stop'",
        "has(hop.route) && hop.route == 'partial' && !has(hop.partial_of)",
        "has(context.summarize) && context.summarize == '1' && (has(hop.finish_reason) || has(hop.refused_subscriber))",
    ] {
        assert!(closed(shut), "closed read as open: {shut}");
    }
}

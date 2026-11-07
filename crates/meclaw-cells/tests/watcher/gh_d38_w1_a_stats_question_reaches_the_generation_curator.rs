//! GH #1444 -- the watcher's `stats` question never reached a curator, and no
//! answer could have come back: the shell carried the lane only to its own rim
//! (`./argus -> .` on `stats`, `. -> ./argus` on `in_stats`), and nothing at
//! that level joined the loop to the organisations under `./orgs`. Measured on
//! a production colony: every role goal unanswered, the gate blind.
//!
//! The way down needs a NAME at every container level: the doors of `./orgs`,
//! of an organisation's `./members` and of a member's `./assistants` are
//! STRICT for `in_stats` (GH #926) -- a question that names no organisation,
//! member and assistant in `context.org`, `context.member` and
//! `context.assistant` reaches nobody. Those names belong to the set that grows
//! the colony, so the question edge at the shell is the set's to draw (one per
//! generation it watches). The way back names nobody: an answer leaves every
//! level on `stats` unchanged, and the shell restamps it `in_stats` into
//! `./argus` -- that edge the shell carries itself.
//!
//! The composition here is the shipped tree of edges, level by level -- the
//! shell, argus, an organisation, a member, an assistant, its talkys and cogny
//! and their curators -- with the container doors of the organism example
//! (`examples/organism`, organisation `acme`, member `alex`, assistant
//! `scribe`) and the one question edge in the form a set draws it. Every hop
//! is evaluated by the colony's own edge evaluator and modifier, so a condition
//! or a restamp that does not hold fails here as it would in a colony.

use meclaw_colony::cel_eval::{
    apply_modifier, evaluate_condition, parse_condition, parse_modifier,
};
use meclaw_colony::config::ModifierSpec;
use meclaw_core::Headers;
use meclaw_core::serde_json::{self, Map, Value, json};

const GEN: &str = "/os/orgs/acme/members/alex/assistants/scribe";

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn read(rel: &str) -> Option<Value> {
    let raw = std::fs::read_to_string(repo(rel)).ok()?;
    Some(serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{rel}: {e}")))
}

struct Edge {
    from: String,
    to: String,
    condition: String,
    modifier: Value,
}

fn resolve(scope: &str, rel: &str) -> String {
    if rel.starts_with('/') {
        return rel.to_string();
    }
    match rel.strip_prefix('.') {
        Some("") => scope.to_string(),
        Some(rest) if rest.starts_with('/') => format!("{scope}{rest}"),
        _ => panic!("an edge endpoint this walk does not read: {rel:?} in {scope}"),
    }
}

fn edges_of(scope: &str, list: &Value, out: &mut Vec<Edge>) {
    for e in list.as_array().expect("an edge list") {
        out.push(Edge {
            from: resolve(scope, e["from"].as_str().expect("from")),
            to: resolve(scope, e["to"].as_str().expect("to")),
            condition: e["condition"].as_str().unwrap_or("true").to_string(),
            modifier: e.get("modifier").cloned().unwrap_or(json!({})),
        });
    }
}

/// The question edge in the form a set draws it at the shell: one per
/// generation, the three names the strict doors read, the lane restamped.
fn question_edge() -> Value {
    json!([{"from": "./argus", "to": "./orgs",
            "condition": "has(hop.route) && hop.route == 'stats'",
            "modifier": {"set_hop": {"route": "'in_stats'"},
                         "set_context": {"org": "'acme'", "member": "'alex'",
                                         "assistant": "'scribe'"}}}])
}

/// The flat edge table of the composition, or None where a part of it did not
/// travel into this tree.
fn composition() -> Option<Vec<Edge>> {
    let mut out = Vec::new();
    let tpl = |name: &str| read(&format!("templates/{name}/config.json"));
    let shipped = |scope: &str, name: &str, out: &mut Vec<Edge>| -> Option<()> {
        edges_of(scope, &tpl(name)?["params"]["graph"]["edges"], out);
        Some(())
    };
    shipped("/os", "meclaw-os", &mut out)?;
    shipped("/os/argus", "argus", &mut out)?;
    shipped("/os/orgs/acme", "org", &mut out)?;
    shipped("/os/orgs/acme/members/alex", "member", &mut out)?;
    shipped(GEN, "assistant", &mut out)?;
    for brain in ["talky", "talky-chat"] {
        shipped(&format!("{GEN}/{brain}"), "talky", &mut out)?;
        shipped(&format!("{GEN}/{brain}/curator"), "curator", &mut out)?;
    }
    shipped(&format!("{GEN}/cogny"), "cogny", &mut out)?;
    shipped(&format!("{GEN}/cogny/curator"), "curator", &mut out)?;
    for grow in ["grow-org", "grow-member", "grow-assistant"] {
        let m = read(&format!("examples/organism/{grow}.json"))?;
        let scope = m["scope"].as_str().expect("a scope").to_string();
        edges_of(&scope, &m["diff"]["add_edges"], &mut out);
    }
    edges_of("/os", &question_edge(), &mut out);
    Some(out)
}

/// Every place a message comes to rest when `at` emits it: a path no edge
/// takes it on from, with the headers it arrived with.
fn walk(edges: &[Edge], at: &str, start: Headers) -> Vec<(String, Headers)> {
    let mut rest = Vec::new();
    let mut queue = vec![(at.to_string(), start, 0usize)];
    while let Some((path, h, depth)) = queue.pop() {
        assert!(depth < 32, "a routing loop at {path}");
        let mut moved = false;
        for e in edges.iter().filter(|e| e.from == path) {
            let cond = parse_condition(&e.condition)
                .unwrap_or_else(|err| panic!("{} -> {}: {err}", e.from, e.to));
            if !evaluate_condition(&cond, &h.context, &h.hop).unwrap_or(false) {
                continue;
            }
            let spec: ModifierSpec = serde_json::from_value(e.modifier.clone())
                .unwrap_or_else(|err| panic!("{} -> {}: {err}", e.from, e.to));
            let m = parse_modifier(&spec).unwrap_or_else(|err| panic!("{err:?}"));
            let next = apply_modifier(&m, &h).unwrap_or_else(|err| panic!("{err}"));
            queue.push((e.to.clone(), next, depth + 1));
            moved = true;
        }
        if !moved {
            rest.push((path, h));
        }
    }
    rest
}

fn obj(v: Value) -> Map<String, Value> {
    v.as_object().expect("an object").clone()
}

/// The ask as `argus/meter` emits it (`stats_ask`).
fn ask(role: Option<&str>) -> Headers {
    let mut hop = obj(json!({"route": "stats", "stats_tag": "wait:w1",
                             "stats_from": "2026-11-07T00:00:00Z",
                             "stats_to": "2026-11-07T06:00:01Z",
                             "stats_kinds": "[\"calls\",\"window\"]", "stats_samples": 10}));
    if let Some(r) = role {
        hop.insert("stats_role".into(), json!(r));
    }
    Headers::from_parts(
        obj(json!({"argus_origin": "meter", "ar_phase": "stats"})),
        hop,
    )
}

#[test]
fn gh_d38_w1_a_stats_question_reaches_the_generation_curator() {
    let Some(edges) = composition() else {
        eprintln!("a part of the composition did not travel into this tree -- skipped (GH #49)");
        return;
    };
    // `talky` is also the brain of a question that names no role.
    for (role, brain) in [
        (Some("talky"), "talky"),
        (None, "talky"),
        (Some("talky-chat"), "talky-chat"),
        (Some("cogny"), "cogny"),
    ] {
        let rest = walk(&edges, "/os/argus/meter", ask(role));
        let curators: Vec<&(String, Headers)> =
            rest.iter().filter(|(p, _)| p.ends_with("/stats")).collect();
        assert_eq!(
            curators.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(),
            vec![format!("{GEN}/{brain}/curator/stats")],
            "the question for {role:?} reaches exactly the curator of {brain}: {:?}",
            rest.iter().map(|(p, _)| p).collect::<Vec<_>>()
        );
        let (_, h) = curators[0];
        assert_eq!(h.hop["route"], "in_stats", "{:?}", h.hop);
        assert_eq!(
            h.hop["stats_tag"], "wait:w1",
            "the tag travels down: {:?}",
            h.hop
        );

        // The answer, as the curator's `./stats` emits it, comes back to the
        // meter on `in_stats` with its tag -- and nowhere else.
        let answer = Headers::from_parts(
            h.context.clone(),
            obj(
                json!({"route": "stats", "stats_tag": "wait:w1", "error_code": "",
                       "detail": ""}),
            ),
        );
        let back = walk(&edges, &format!("{GEN}/{brain}/curator/stats"), answer);
        assert_eq!(
            back.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(),
            vec!["/os/argus/meter"],
            "the answer for {role:?} comes back to the meter and nowhere else"
        );
        let (_, h) = &back[0];
        assert_eq!(h.hop["route"], "in_stats", "{:?}", h.hop);
        assert_eq!(h.hop["stats_tag"], "wait:w1", "{:?}", h.hop);
        assert_eq!(h.context["argus_origin"], "meter", "{:?}", h.context);
        for key in ["org", "member", "assistant"] {
            assert!(
                !h.context.contains_key(key),
                "the address the question carried down must not ride on into the \
                 watcher: context.{key} in {:?}",
                h.context
            );
        }
    }
}

#[test]
fn gh_d38_w1_a_question_that_names_nobody_reaches_no_curator() {
    let Some(mut edges) = composition() else {
        return;
    };
    // Without the set's question edge the shell carries the ask to its rim
    // only: the strict doors below need the names, and the shell has none.
    edges.retain(|e| !(e.from == "/os/argus" && e.to == "/os/orgs"));
    let rest = walk(&edges, "/os/argus/meter", ask(Some("talky")));
    assert!(
        rest.iter().all(|(p, _)| !p.ends_with("/stats")),
        "a question that names no generation must reach no curator: {:?}",
        rest.iter().map(|(p, _)| p).collect::<Vec<_>>()
    );
}

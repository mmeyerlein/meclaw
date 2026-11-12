//! D58 proof (GH #1475, contract point 2) -- a fifth model cell needs no
//! change in the curator.
//!
//! WHY this file exists: R-58-9 Nr. 6 measures the generic curator by one
//! claim: a composite that gains a model cell touches its own tree and
//! nothing under `templates/curator/`. Before D58 the curator knew talky's
//! four model cells by name (a table of roles, a way per route stamp, fixed
//! labels on its edges); a fifth would have meant a new role and a new way
//! inside the curator. Now a target is a name the composite wires, its row
//! and profile arrive over the cell's own edge, and the lead names the
//! target per stamp from its knobs.
//!
//! What the test does: it takes talky as shipped and adds, in a temporary
//! tree of its own, a fifth `llm` cell (`deep`, a copy of talky's brain with
//! its profile), the edges between it and `./curator`, and one entry in the
//! lead's knob `path_routes` (stamp `deep` -> target `deep`). Then a turn
//! the lead sends to `deep`:
//!
//! 1. leaves under `hop.target` `deep`, cut by the row `deep` reported (a
//!    hard line under the window: the call stands under an overflow cut
//!    that took an older round off a window larger than that line, while
//!    the roomy brain's call before it is not cut);
//! 2. reaches the cell `./deep` over the composite's edges, and its answer
//!    comes back to the curator as the answer of `deep`;
//! 3. lands on the wall: the next window of the session carries it.
//!
//! The test writes nothing under `templates/curator/` and changes nothing
//! there: the tree's bytes are the same before and after.
//!
//! Runs the shipped curator hive and lead hive (`support/curator_hive.rs`)
//! and the composite's edges through the colony's own router; no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_lead.rs"]
#[allow(unused_imports)]
use crate::curator_lead; // harness_fold: mod curator_lead;
// harness_fold: #[path = "support/curator_target.rs"]
#[allow(unused_imports)]
use crate::curator_target; // harness_fold: mod curator_target;

use curator_hive::*;
use curator_lead::*;
use curator_target::*;
use meclaw_colony::config::{EdgeSpec, HiveParams};
use meclaw_colony::edge_table::{Edge, EdgeTable, apply_edges};
use meclaw_core::serde_json::{self as sj, Value, json};
use meclaw_core::{Headers, Path, Uuid};

/// The fifth model cell: its name in the composite, which is its target.
const FIFTH: &str = "deep";

/// The route stamp the lead sends to the fifth cell.
const STAMP: &str = "deep";

/// The cell of talky the fifth is copied from.
const MODEL: &str = "brain";

/// The soft and hard input lines of the fifth cell's catalogue row: tight,
/// so its window is cut where the roomy brain's is not.
const FIFTH_SOFT: i64 = 20;
const FIFTH_HARD: i64 = 30;

/// Where the composite stands in the router of this test.
const BASE: &str = "/t";

/// Every file under `dir`, `(relative path, bytes)`, sorted.
fn tree_bytes(dir: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let rel = p
                    .strip_prefix(dir)
                    .map(|r| r.display().to_string())
                    .unwrap_or_default();
                out.push((rel, std::fs::read(&p).unwrap_or_default()));
            }
        }
    }
    out.sort();
    out
}

/// The edge `e` of talky's model cell rewritten for the fifth cell: its
/// endpoint and every quoted name of the cell in its condition.
fn for_the_fifth(e: &Value) -> Value {
    let text = sj::to_string(e).expect("json");
    let text = text
        .replace(&format!("\"./{MODEL}\""), &format!("\"./{FIFTH}\""))
        .replace(&format!("'{MODEL}'"), &format!("'{FIFTH}'"));
    sj::from_str(&text).expect("json")
}

/// talky with a fifth model cell, laid out under `<td>/talky`: the cell (a
/// copy of talky's brain with its profile) and the edges between it and the
/// curator. Returns the composite's config.
fn talky_with_a_fifth(td: &tempfile::TempDir) -> Value {
    let dir = td.path().join("talky");
    let mut cell = read_json(&repo(&format!("templates/talky/{MODEL}/config.json")));
    // The fifth keeps no recent rounds of its own (brain keeps 40), so its
    // hard line has an older round to take: the cut is visible in numbers.
    cell["params"]["window_profile"]["keep_recent"] = json!(0);
    cell["params"]["window_profile"]["keep_recent_tokens"] = json!(0);
    std::fs::create_dir_all(dir.join(FIFTH)).expect("dir");
    std::fs::write(
        dir.join(FIFTH).join("config.json"),
        sj::to_string_pretty(&cell).expect("json"),
    )
    .expect("write the cell");
    let mut comp = read_json(&repo("templates/talky/config.json"));
    let added: Vec<Value> = comp["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|e| {
            let (f, t) = (e["from"].as_str(), e["to"].as_str());
            let me = format!("./{MODEL}");
            (f == Some(me.as_str()) && t == Some("./curator"))
                || (f == Some("./curator") && t == Some(me.as_str()))
        })
        .map(for_the_fifth)
        .collect();
    assert!(!added.is_empty(), "talky wires its {MODEL} to the curator");
    comp["params"]["graph"]["edges"]
        .as_array_mut()
        .expect("edges")
        .extend(added);
    std::fs::write(
        dir.join("config.json"),
        sj::to_string_pretty(&comp).expect("json"),
    )
    .expect("write the composite");
    comp
}

/// The composite's edges as the colony's router holds them, at [`BASE`].
fn table(comp: &Value) -> EdgeTable {
    let hp: HiveParams = sj::from_value(comp["params"].clone()).expect("hive params");
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

/// The cells one message from `<BASE>/<from>` reaches, with their headers.
fn reach(t: &EdgeTable, from: &str, ctx: Value, hop: Value) -> Vec<(String, Headers)> {
    let hs = Headers::from_parts(obj(ctx), obj(hop));
    let mut out: Vec<(String, Headers)> =
        apply_edges(t, &Path::new(&format!("{BASE}/{from}")), &hs)
            .into_iter()
            .map(|d| (d.target.as_str().to_string(), d.headers_out))
            .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// The lead's `path_routes` as talky sets them, with the fifth's entry.
fn routes_with_the_fifth() -> String {
    let shipped = LEAD_WIRING
        .iter()
        .find(|(k, _)| *k == "path_routes")
        .map(|(_, v)| v.to_string())
        .unwrap_or_default();
    format!("{shipped},{STAMP}:{FIFTH}")
}

#[test]
fn gh_d58_proof_a_fifth_model_cell_needs_no_change_in_the_curator() {
    if !shipped() {
        return;
    }
    let curator_before = tree_bytes(&repo("templates/curator"));
    let td = tempfile::TempDir::new().expect("tempdir");
    let comp = talky_with_a_fifth(&td);
    let cell = read_json(&td.path().join("talky").join(FIFTH).join("config.json"));
    let profile = cell["params"]["window_profile"].clone();
    assert!(
        profile.is_object() && profile.as_object().is_some_and(|m| !m.is_empty()),
        "the fifth cell carries a profile of its own: {profile}"
    );

    // The curator and the lead as talky ships them, one knob entry more.
    let mut h = wired(&[("lead", "path_routes", json!(routes_with_the_fifth()))]);
    announce(
        &mut h,
        MODEL,
        roomy("test/brain"),
        read_json(&repo(&format!("templates/talky/{MODEL}/config.json")))["params"]
            ["window_profile"]
            .clone(),
        json!([]),
    );
    announce(
        &mut h,
        FIFTH,
        row("test/deep", FIFTH_SOFT, FIFTH_HARD),
        profile,
        json!([]),
    );

    // A talk turn first, so the wall holds something to cut.
    let first = curate_for(
        &mut h,
        Some(MODEL),
        "t1",
        0,
        json!([user("Tell me about the river walk.")]),
        json!({"path": "talk", "needs": [], "families": []}),
    );
    let first = the_call(&h, &first);
    assert_eq!(
        first.hop.get("target"),
        Some(&json!(MODEL)),
        "{:?} {:?}",
        first.hop,
        h.stderr
    );
    assert!(!has_overflow_cut(&first), "the roomy row cuts nothing");
    answer(
        &mut h,
        &first,
        "stop",
        warm_usage(),
        json!([said("It is a fine walk.")]),
    );

    // 1. The turn the lead sends to the fifth cell.
    let calls = curate_for(
        &mut h,
        Some(MODEL),
        "t2",
        0,
        json!([user("Think about the old bridge in depth.")]),
        json!({"path": STAMP, "needs": [], "families": []}),
    );
    let call = the_call(&h, &calls);
    assert_eq!(
        call.hop.get("target"),
        Some(&json!(FIFTH)),
        "the lead's entry sends the turn to the fifth cell: {:?} {:?}",
        call.hop,
        h.stderr
    );
    assert!(
        has_overflow_cut(&call),
        "the call is cut by the fifth cell's own row (its hard line): {:?}",
        call.hop
    );
    // The cut stands at the line of the fifth's row, not at a label: the
    // window was larger than `input_hard` of `deep`, and the guard took at
    // least one older round off it. The brain's row (roomy) cut nothing.
    let cut = call.hop["cuts"]
        .as_array()
        .and_then(|c| c.iter().find(|x| x["what"] == "overflow_cut"))
        .cloned()
        .unwrap_or_default();
    let (shown, total, rounds) = (
        cut["shown"].as_i64().unwrap_or(-1),
        cut["total"].as_i64().unwrap_or(-1),
        cut["rounds"].as_i64().unwrap_or(-1),
    );
    assert!(
        total > FIFTH_HARD && rounds >= 1 && (0..total).contains(&shown),
        "the window is cut at the hard line of the fifth's row ({FIFTH_HARD}): {cut}"
    );

    // 2. The composite delivers it to the fifth cell and brings its answer
    //    back as the answer of that target.
    let t = table(&comp);
    let got = reach(
        &t,
        "curator",
        call.context.clone().into(),
        call.hop.clone().into(),
    );
    let to: Vec<&String> = got.iter().map(|(to, _)| to).collect();
    assert_eq!(
        to,
        vec![&format!("{BASE}/{FIFTH}")],
        "the call reaches the fifth cell and no other"
    );
    let ctx = got[0].1.context.clone();
    assert_eq!(ctx.get("curator_target"), Some(&json!(FIFTH)), "{ctx:?}");
    let back = reach(
        &t,
        FIFTH,
        Value::Object(ctx.clone()),
        json!({"finish_reason": "stop", "model": "test/deep"}),
    );
    assert!(
        back.iter().any(|(to, hs)| to == &format!("{BASE}/curator")
            && hs.hop.get("route") == Some(&json!("in_llm"))
            && hs.context.get("curator_target") == Some(&json!(FIFTH))),
        "the fifth's answer comes back to the curator as its own: {back:?}"
    );
    answer(
        &mut h,
        &call,
        "stop",
        warm_usage(),
        json!([said("The bridge is older than the town.")]),
    );

    // 3. The answer stands on the wall: the next window carries it.
    let next = curate_for(
        &mut h,
        Some(MODEL),
        "t3",
        0,
        json!([user("And then?")]),
        json!({"path": "talk", "needs": [], "families": []}),
    );
    let next = the_call(&h, &next);
    assert!(
        texts(&next)
            .iter()
            .any(|t| t.contains("The bridge is older than the town.")),
        "the fifth's answer reached the wall: {:?}",
        texts(&next)
    );

    // Nothing under templates/curator/ was written.
    assert!(
        tree_bytes(&repo("templates/curator")) == curator_before,
        "the test changed a file under templates/curator/"
    );
}

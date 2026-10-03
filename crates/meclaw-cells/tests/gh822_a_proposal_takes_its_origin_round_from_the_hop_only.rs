//! GH #822 E.5 (PE-BG-16) — a proposal row records the ROUND it was made in,
//! and only the edge may say which round that is.
//!
//! soul2me's counselor (task D) may only propose a value inside the round its
//! source spoke in, and `affinity.proposals` had no column for that round: the
//! task was dropped ("a proposal row has no column for the round of its
//! source"). `origin_round` is that column, `''` by default (no round, which a
//! round-bound reader drops — fail-closed).
//!
//! Pinned here at the shipped `gate` script (run over stdin the way a `code`
//! cell runs it) and at the shipped door edge:
//! 1. `propose` writes `context.audience_set` into `origin_round`; a
//!    `decide_proposal` row writes `''` and inherits the round of the row its
//!    `supersedes` names (two-round lock, ruling A);
//! 2. a body that names `origin_round` is ignored, with or without a round on
//!    the edge;
//! 3. the write door promotes the round by the read door's precedence
//!    (edge-pinned `context.audience_set` first, then `hop.audience_set`).

#[path = "support/assemble_cell.rs"]
mod assemble_cell;

use assemble_cell::{config_of, run_cell};
use serde_json::{Value, json};

const GATE: &str = "templates/affinity/gate/config.json";
const HIVE: &str = "templates/affinity/config.json";
const ROUND: &str = "[\"member:alex\",\"guest:bo\"]";

fn gate(args: Value, context: Value) -> Vec<Value> {
    run_cell(
        GATE,
        &[],
        json!({
            "messages": [{"origin": "assistant", "type": "tool_call", "id": "c1",
                          "text": args.to_string()}],
            "header": {"hop": {}, "context": context}
        }),
    )
    .0
}

/// The `proposals` rows the gate inserted, in emission order.
fn proposal_rows(out: &[Value]) -> Vec<Value> {
    out.iter()
        .filter(|m| m["header"]["route"] == "astore")
        .filter_map(|m| serde_json::from_str::<Value>(m["messages"][0]["text"].as_str()?).ok())
        .filter(|a| a["table"] == "proposals" && a["operation"] == "insert")
        .map(|a| a["row"].clone())
        .collect()
}

fn propose(body_round: Option<&str>) -> Value {
    let mut v = json!({
        "op": "propose",
        "source_ref": "counselor",
        "entity_ref": "entity:bo",
        "field_path": "aieos.interests.favorites.music_genre",
        "value": "jazz",
        "auto_accept": false
    });
    if let Some(r) = body_round {
        v["origin_round"] = json!(r);
        v["audience_set"] = json!(["*"]);
    }
    v
}

#[test]
fn a_proposal_takes_its_origin_round_from_the_hop_only() {
    // The round on the edge is what the row records -- the body's is not read.
    let rows = proposal_rows(&gate(
        propose(Some("[\"*\"]")),
        json!({"actor": "app:counselor", "audience_set": ROUND}),
    ));
    assert_eq!(rows.len(), 1, "one proposal row");
    assert_eq!(rows[0]["origin_round"], ROUND);

    // No round on the edge: no round at all, whatever the body claims.
    let rows = proposal_rows(&gate(
        propose(Some("[\"member:alex\"]")),
        json!({"actor": "app:counselor"}),
    ));
    assert_eq!(rows[0]["origin_round"], "", "fail-closed: no round");

    // And the plain call without any round still carries the column.
    let rows = proposal_rows(&gate(propose(None), json!({"actor": "app:counselor"})));
    assert_eq!(rows[0]["origin_round"], "");
}

/// The round a round-bound reader (soul2me task D) uses for a row: its own
/// `origin_round` when it was made by `propose`, otherwise the round of the row
/// its `supersedes` names -- and no row named, no round (fail-closed). This is
/// the reader contract of `templates/affinity/README.md` § round, written out
/// so the lock below can hold the gate's rows against it.
fn effective_round(rows: &[Value], row: &Value) -> String {
    let own = row["origin_round"].as_str().unwrap_or("");
    let sup = row["supersedes"].as_str().unwrap_or("");
    if sup.is_empty() {
        return own.to_string();
    }
    rows.iter()
        .find(|r| r["id"] == sup)
        .map(|r| effective_round(rows, r))
        .unwrap_or_default()
}

/// Review N-B E #1, orchestrator ruling A — the two-round lock. bo speaks in
/// round R1 = `[alex, bo]`, the counselor proposes the value there; alex judges
/// it later in R2 = `[alex]`. The verdict row carries NO round (`''`), so the
/// value can never count as spoken in R2, a room without bo: a reader follows
/// `supersedes` back to R1. A verdict whose origin row is unknown has no round.
#[test]
fn a_verdict_has_no_round_and_the_value_keeps_the_round_of_its_proposal() {
    const R1: &str = "[\"guest:bo\",\"member:alex\"]";
    const R2: &str = "[\"member:alex\"]";
    let mut p = propose(None);
    p["id"] = json!("prop:r1");
    let proposed = proposal_rows(&gate(
        p,
        json!({"actor": "app:counselor", "audience_set": R1}),
    ));
    assert_eq!(proposed[0]["origin_round"], R1);

    let verdict = proposal_rows(&gate(
        json!({
            "op": "decide_proposal", "id": "prop:r1", "status": "accepted",
            "source_ref": "counselor", "entity_ref": "entity:bo",
            "field_path": "aieos.interests.favorites.music_genre",
            "value": "jazz", "origin_round": R2
        }),
        json!({"actor": "member:alex", "audience_set": R2}),
    ));
    assert_eq!(verdict.len(), 1, "one verdict row");
    assert_eq!(
        verdict[0]["origin_round"], "",
        "a verdict stamps no round of its own"
    );
    assert_eq!(verdict[0]["supersedes"], "prop:r1");

    let store: Vec<Value> = proposed.iter().chain(verdict.iter()).cloned().collect();
    let round = effective_round(&store, &verdict[0]);
    assert_eq!(round, R1, "the value keeps the round bo spoke in");
    assert_ne!(round, R2, "and never counts in a round without bo");

    // Fail-closed: the origin row is not there -- no round at all.
    assert_eq!(effective_round(&verdict, &verdict[0]), "");
}

/// One precedence, two doors: the write door promotes the round exactly as the
/// read door does, so a cell downstream of a pinning edge cannot pick its room
/// by stamping `hop.audience_set` on the write lane either.
#[test]
fn the_write_door_promotes_the_round_like_the_read_door() {
    let hive = config_of(HIVE);
    let edges = hive["params"]["graph"]["edges"]
        .as_array()
        .expect("the hive declares its edges");
    let door = |to: &str, route: &str| {
        edges
            .iter()
            .find(|e| {
                e["from"] == "."
                    && e["to"] == to
                    && e["condition"].as_str().is_some_and(|c| c.contains(route))
            })
            .unwrap_or_else(|| panic!("the door to {to} for {route}"))
            .clone()
    };
    let read = door("./brief", "'in_brief'");
    let write = door("./gate", "'in_propose'");
    let expr = &write["modifier"]["set_context"]["audience_set"];
    assert!(
        expr.is_string(),
        "the write door promotes the round: {write}"
    );
    assert_eq!(
        expr, &read["modifier"]["set_context"]["audience_set"],
        "the same precedence on both doors"
    );
}

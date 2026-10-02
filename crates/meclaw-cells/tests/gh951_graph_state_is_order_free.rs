//! GH #951 -- the graph's store does not depend on the order its sources
//! arrive in (C1 finding G: "the suffix rule is order-dependent in the
//! store").
//!
//! No colony: phase 1 and phase 2 of the shipped `index` script over the
//! store stand-in (`support/graph_space_pure.rs`, `SIM`). Three python
//! sources: `/app/a.py` imports `util:f` and calls `f`; `/x/util.py` and
//! `/y/util.py` both define `f`. The import names a module by its suffix, so
//! with both `util` sources indexed it is ambiguous.
//!
//! 1. `a_later_source_with_the_same_suffix_leaves_the_edge_open`: in every
//!    order the three sources can arrive in, A's two edges end `unresolved`
//!    with no address -- the state a fresh index reads -- and never `broken`:
//!    an ambiguity lost nothing. Measured red before: when A was indexed
//!    while only one `util` existed, the second one left the edge
//!    `resolved` to the first (`r-open` read only open edges).
//! 2. `removing_the_second_source_resolves_the_edge_again`: from the state
//!    of every order, the removal of `/y/util.py` (the shipped tomb, the
//!    reads it opens, phase 2 over them) leaves exactly the edges a fresh
//!    index of A and `/x/util.py` writes. Red before: a removal decided
//!    nothing but the edges into the removed source.
//!
//! Orders are sequential (each source fully indexed before the next); the
//! interleaving of two phases is gh945 `a_stale_pull_never_overwrites`'s.
//! Free of a paid provider by construction. Guarded like every
//! template-reading test (GH #49).

#[path = "support/graph_space_pure.rs"]
mod pure;

use meclaw_core::serde_json::{Value, json};

const WORLD: &str = r#"
def suffix_world():
    def spec(S, path, nodes, links):
        lang = lang_of("", path)
        return ({"source": S, "version": "v1", "path": path, "lang": lang,
                 "module": module_of(path, lang)}, nodes, links)
    A, B1, B2 = "fh-a00000000001", "fh-b10000000001", "fh-b20000000002"
    return A, B1, B2, {
        "A": spec(A, "/app/a.py", [{"anchor": "def:g"}],
                  [{"kind": "import", "from_anchor": "", "target_name": "util:f"},
                   {"kind": "call", "from_anchor": "def:g", "target_name": "f"}]),
        "B1": spec(B1, "/x/util.py", [{"anchor": "def:f"}], []),
        "B2": spec(B2, "/y/util.py", [{"anchor": "def:f"}], [])}


def edge_rows(db):
    return sorted([e["from_addr"], e["kind"], e["state"], e["to_addr"], e["to_source"], e["since"]]
                  for e in db.get("edges", []) if e["class"] == "extracted")


def announced(specs, names):
    return {"sources": [source_row(specs[n][0]["source"], specs[n][0]["path"],
                                   specs[n][0]["lang"], specs[n][0]["module"], "v1")
                        for n in names]}


def remove(db, S, path, stamp):
    """A removal through the shipped steps: the tomb bundle, the reads of the
    edges it may have opened, phase 2 over them -- as the lanes run them."""
    out = run_bundle(db, tomb_ops(S, "", stamp, path, ""))
    opened = [r for cid in sorted(out) if cid.startswith("r-open") for r in out[cid]]
    reads = reopen_reads(S, path, opened)
    if not reads:
        return
    rows = p1_rows(run_bundle(db, reads))
    run_bundle(db, phase2_ops(S, [], rows, stamp + "2", True))


def every_order(remove_b2):
    A, B1, B2, specs = suffix_world()
    out = {}
    for order in ARGS:
        db = announced(specs, ["A", "B1", "B2"])
        interleave(db, specs, order)
        if remove_b2:
            remove(db, B2, specs["B2"][0]["path"], "T")
        out[" ".join(order)] = edge_rows(db)
    fresh = announced(specs, ["A", "B1"])
    interleave(fresh, specs, ["B11", "B12", "A1", "A2"])
    return {"orders": out, "fresh without B2": edge_rows(fresh)}
"#;

/// The six orders three sources can arrive in, each indexed to the end.
fn orders() -> Value {
    let names = ["A", "B1", "B2"];
    let mut out = Vec::new();
    for a in 0..3 {
        for b in 0..3 {
            for c in 0..3 {
                if a == b || b == c || a == c {
                    continue;
                }
                let steps: Vec<String> = [names[a], names[b], names[c]]
                    .iter()
                    .flat_map(|n| [format!("{n}1"), format!("{n}2")])
                    .collect();
                out.push(json!(steps));
            }
        }
    }
    assert_eq!(out.len(), 6);
    Value::Array(out)
}

fn run(remove_b2: bool) -> Value {
    let extra = format!("{}\n{WORLD}", pure::SIM);
    let probe = if remove_b2 {
        "every_order(True)"
    } else {
        "every_order(False)"
    };
    pure::pure_with("index", &extra, probe, orders())
}

#[test]
fn a_later_source_with_the_same_suffix_leaves_the_edge_open() {
    if !pure::shipped() {
        eprintln!("templates/graph-space did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let got = run(false);
    let a = "fh-a00000000001";
    let open = json!([
        [a, "import", "unresolved", "", "", ""],
        [format!("{a}#def:g"), "call", "unresolved", "", "", ""]
    ]);
    for (order, edges) in got["orders"].as_object().expect("one result per order") {
        assert_eq!(
            edges, &open,
            "order {order}: two sources answer to `util`, so A's edges are unresolved with no \
             address -- what a fresh index reads, and not `broken`: {got}"
        );
    }
}

#[test]
fn removing_the_second_source_resolves_the_edge_again() {
    if !pure::shipped() {
        eprintln!("templates/graph-space did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let got = run(true);
    let a = "fh-a00000000001";
    let b1 = "fh-b10000000001";
    let fresh = json!([
        [a, "import", "resolved", format!("{b1}#def:f"), b1, ""],
        [
            format!("{a}#def:g"),
            "call",
            "resolved",
            format!("{b1}#def:f"),
            b1,
            ""
        ]
    ]);
    assert_eq!(
        got["fresh without B2"], fresh,
        "the reference: a fresh index of A and /x/util.py resolves both edges: {got}"
    );
    for (order, edges) in got["orders"].as_object().expect("one result per order") {
        assert_eq!(
            edges, &fresh,
            "order {order}: once /y/util.py is removed the store is the fresh index without \
             it: {got}"
        );
    }
}

//! GH #951 -- a removal breaks only what was extracted, and a neighbour by
//! meaning at a vanished anchor goes (C1 finding G: "`tomb_ops` breaks
//! `inferred` edges too; an `inferred` edge onto a vanished anchor stays
//! `resolved`").
//!
//! No colony: phase 1 and phase 2 of the shipped `index` script over the
//! store stand-in (`support/graph_space_pure.rs`, `SIM`). `/p/x.py` defines
//! `f` and `k`; `/p/y.py` imports `p.x:f` and its `near` names three
//! neighbours in x: `#def:f`, `#def:k` and the file itself. So y holds one
//! `extracted` edge (`resolved` to `x#def:f`) and three `inferred` ones.
//!
//! 1. `a_removal_breaks_the_extracted_edge_and_drops_the_inferred_ones`:
//!    x is removed (the shipped tomb bundle). The import is `broken` and
//!    keeps its address; every inferred edge into x is gone -- a lost guess
//!    is no loss to report. Measured red before: the three inferred edges
//!    read `broken`.
//! 2. `an_inferred_edge_onto_a_vanished_anchor_is_dropped`: x is indexed
//!    again with `def:f` renamed `def:h`. The import breaks as before
//!    (OR-BC-55 (4)); the inferred edge onto `x#def:f` is deleted, the ones
//!    onto `x#def:k` and onto x itself stay `resolved`. Red before: the edge
//!    onto `x#def:f` stayed `resolved`.
//!
//! Free of a paid provider by construction. Guarded like every
//! template-reading test (GH #49).

#[path = "support/graph_space_pure.rs"]
mod pure;

use meclaw_core::serde_json::{Value, json};

const WORLD: &str = r##"
def neighbour_world():
    def spec(S, path, nodes, links):
        lang = lang_of("", path)
        return ({"source": S, "version": "v1", "path": path, "lang": lang,
                 "module": module_of(path, lang)}, nodes, links)
    X, Y = "fh-c00000000001", "fh-d00000000002"
    return X, Y, {
        "X": spec(X, "/p/x.py", [{"anchor": "def:f"}, {"anchor": "def:k"}], []),
        "Xh": spec(X, "/p/x.py", [{"anchor": "def:h"}, {"anchor": "def:k"}], []),
        "Y": spec(Y, "/p/y.py", [{"anchor": "def:g"}],
                  [{"kind": "import", "from_anchor": "", "target_name": "p.x:f"}])}


def index(db, specs, name, near, stamp):
    """Phase 1 and 2 of one source through the shipped functions, with its
    `near` neighbours (the stand-in's `interleave` pulls none)."""
    src, nodes, links = specs[name]
    rows = p1_rows(run_bundle(db, phase1_ops(src, clean_nodes(nodes), clean_links(links),
                                             clean_near(near, src["source"]), stamp + "1")))
    run_bundle(db, phase2_ops(src["source"], [r["anchor"] for r in rows["r-anch"]], rows,
                              stamp + "2"))


def view(db):
    return sorted([e["from_addr"], e["kind"], e["class"], e["state"], e["to_addr"]]
                  for e in db.get("edges", []))


def copy(db):
    return {t: [dict(r) for r in rs] for t, rs in db.items()}


def removal_and_rename():
    X, Y, specs = neighbour_world()
    near = [{"addr": X + "#def:f", "score": 0.9}, {"addr": X + "#def:k", "score": 0.8},
            {"addr": X, "score": 0.7}]
    db = {"sources": [source_row(X, "/p/x.py", "python", "p.x", "v1"),
                      source_row(Y, "/p/y.py", "python", "p.y", "v1")]}
    index(db, specs, "X", [], "A")
    index(db, specs, "Y", near, "B")
    out = {"indexed": view(db)}
    renamed = copy(db)
    run_bundle(db, tomb_ops(X, "", "T"))
    out["removed"] = view(db)
    index(renamed, specs, "Xh", [], "R")
    out["renamed"] = view(renamed)
    return out
"##;

const X: &str = "fh-c00000000001";
const Y: &str = "fh-d00000000002";

fn run() -> Value {
    let extra = format!("{}\n{WORLD}", pure::SIM);
    let got = pure::pure_with("index", &extra, "removal_and_rename()", json!(null));
    let f = format!("{X}#def:f");
    let k = format!("{X}#def:k");
    assert_eq!(
        got["indexed"],
        json!([
            [Y, "import", "extracted", "resolved", f],
            [Y, "similar", "inferred", "resolved", X],
            [Y, "similar", "inferred", "resolved", f],
            [Y, "similar", "inferred", "resolved", k]
        ]),
        "the start: one extracted edge, three inferred ones: {got}"
    );
    got
}

#[test]
fn a_removal_breaks_the_extracted_edge_and_drops_the_inferred_ones() {
    if !pure::shipped() {
        eprintln!("templates/graph-space did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let got = run();
    assert_eq!(
        got["removed"],
        json!([[Y, "import", "extracted", "broken", format!("{X}#def:f")]]),
        "a removal breaks the extracted edge and deletes every inferred edge into the source, \
         never `broken`: {got}"
    );
}

#[test]
fn an_inferred_edge_onto_a_vanished_anchor_is_dropped() {
    if !pure::shipped() {
        eprintln!("templates/graph-space did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let got = run();
    assert_eq!(
        got["renamed"],
        json!([
            [Y, "import", "extracted", "broken", format!("{X}#def:f")],
            [Y, "similar", "inferred", "resolved", X],
            [Y, "similar", "inferred", "resolved", format!("{X}#def:k")]
        ]),
        "the inferred edge onto the renamed anchor is gone, the others stay resolved: {got}"
    );
}

//! P15 task 4 -- window parameters as a pure function inside the shipped recall script.
//!
//! The helper is deliberately duplicated from `p15_recall_chain.rs`: sharing it
//! between two integration tests would need a file of its own, and the probe is a
//! handful of lines. Same contract as there -- the REAL `params.script_inline` runs,
//! `${VAR:-default}` is resolved the way the colony resolves it at instantiation, the
//! module body runs against a stub stdin and its `park()` exit is swallowed.

use std::process::Command;

fn recall_script() -> String {
    let raw = std::fs::read_to_string("../../templates/memory-hive/recall/config.json")
        .expect("recall config");
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    resolve_vars(v["params"]["script_inline"].as_str().unwrap())
}

/// `${VAR:-default}` becomes the default, a bare `${VAR}` becomes the empty string --
/// the same substitution the colony performs when it instantiates the template.
fn resolve_vars(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    let mut rest = script;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail
            .find('}')
            .expect("unterminated ${...} in script_inline");
        if let Some((_, default)) = tail[..end].split_once(":-") {
            out.push_str(default);
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

fn run_probe_window(probe: &str) -> String {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO('{{\"envelope\": {{}}, \"body\": {{}}, \"params\": {{}}}}')\n",
            "_sink, _real = io.StringIO(), sys.stdout\n",
            "sys.stdout = _sink\n",
            "try:\n",
            "    exec(compile(_script, 'recall', 'exec'), globals())\n",
            "except SystemExit:\n",
            "    pass\n",
            "finally:\n",
            "    sys.stdout = _real\n",
            "{}"
        ),
        serde_json::to_string(&recall_script()).unwrap(),
        probe
    );
    let out = Command::new("python3")
        .arg("-c")
        .arg(src)
        .output()
        .expect("python3");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn window_parsing_covers_all_three_cases() {
    let probe = r#"
print(read_window({"recall_window_from":"","recall_window_to":""}))
print(read_window({"recall_window_from":"2026-08-03T00:00:00Z",
                   "recall_window_to":"2026-08-10T00:00:00Z"}))
try:
    read_window({"recall_window_from":"2026-08-03T00:00:00Z","recall_window_to":""})
    print("NO-RAISE")
except WindowError as e:
    print("reject:", e)
"#;
    assert_eq!(
        run_probe_window(probe),
        "None\n('2026-08-03T00:00:00Z', '2026-08-10T00:00:00Z')\nreject: half-open window"
    );
}

/// Ruling A: a half-open window leaves through the reject port at the REQUEST
/// entry — before the leg fan, so a caller bug never costs a full tier-1 fan-out.
/// Same shape as `extract-glue`'s reject, so the parent drains it the same way.
#[test]
fn a_half_open_window_leaves_through_the_reject_port() {
    let probe = r#"
try:
    read_window({"recall_window_from":"","recall_window_to":"2026-08-10T00:00:00Z"})
    print("NO-RAISE")
except WindowError as e:
    m = reject_message(e)
    print(m["header"]["route"], m["header"]["phase"])
    print(m["messages"][0]["type"], m["messages"][0]["text"])
"#;
    assert_eq!(
        run_probe_window(probe),
        "reject request\ntool_result recall rejected: half-open window"
    );
}

#[test]
fn window_intersects_the_whole_chain_not_just_the_current_fact() {
    let probe = r#"
rows = [
 {"id":"a","subject":"s","predicate":"editor","claim":"Helix",
  "valid_from":"2026-08-08T19:18:34Z","valid_until":None,"recorded_at":"2026-08-08T19:18:39Z"},
 {"id":"b","subject":"s","predicate":"editor","claim":"vscode",
  "valid_from":"2026-08-08T19:28:00Z","valid_until":None,"recorded_at":"2026-08-08T19:28:03Z"},
]
hits = window_hits(rows, "2026-08-03T00:00:00Z", "2026-08-10T17:00:00Z")
print("|".join(f["claim"] for f in hits))
# a window BEFORE both matches nothing
print(len(window_hits(rows, "2026-07-01T00:00:00Z", "2026-07-02T00:00:00Z")))
# a window that lies only inside the Helix span matches Helix alone
print("|".join(f["claim"] for f in
      window_hits(rows, "2026-08-08T19:20:00Z", "2026-08-08T19:25:00Z")))
"#;
    assert_eq!(run_probe_window(probe), "Helix|vscode\n0\nHelix");
}

/// R7: the keyword leg matches on a word-END prefix, so a singular query token
/// still reaches the plural index term. The quoting is what keeps caller text
/// away from the FTS operator grammar and must survive the change untouched.
#[test]
fn keyword_tokens_carry_a_trailing_prefix_star() {
    let probe = r#"
print(fts_match("welchen Lieblingseditor nutzt alex"))
print(fts_match('he said "or" and -x-'))
print(repr(fts_match("a of the")))
"#;
    assert_eq!(
        run_probe_window(probe),
        "\"welchen\"* OR \"Lieblingseditor\"* OR \"nutzt\"* OR \"alex\"*\n\
         \"said\"* OR \"-x-\"*\n\
         ''"
    );
}

#[test]
fn an_exact_score_tie_is_decided_by_the_temporal_leg() {
    // Ruling O-2: symmetric ranks (keyword 1/2, temporal 2/1) make the RRF sums
    // bit-identical -- 1/61 + 1/62 either way. Before the ruling the winner was
    // the smaller uuid, i.e. a coin flip across runs. Now the temporal leg
    // decides, in BOTH input orders.
    let probe = r#"
def mk(i): return {"kind": "fact", "id": i}
a = {"keyword": [mk("a"), mk("b")], "semantic": [], "graph": [],
     "temporal": [mk("b"), mk("a")]}
b = {"keyword": [mk("b"), mk("a")], "semantic": [], "graph": [],
     "temporal": [mk("a"), mk("b")]}
ra, sa, _la = fuse_rank(a)
rb, _sb, _lb = fuse_rank(b)
print("|".join(k[1] for k in ra))
print("|".join(k[1] for k in rb))
print(sa[("fact","a")] == sa[("fact","b")])
"#;
    // "b" first in case A although "a" is the smaller id -> the id no longer decides.
    assert_eq!(run_probe_window(probe), "b|a\na|b\nTrue");
}

#[test]
fn a_candidate_the_temporal_leg_never_saw_sorts_behind_one_it_did() {
    let probe = r#"
def mk(i): return {"kind": "fact", "id": i}
lists = {"keyword": [mk("a"), mk("b")], "semantic": [], "graph": [],
         "temporal": [mk("b")]}
r, s, _l = fuse_rank(lists)
print("|".join(k[1] for k in r), s[("fact","a")] == s[("fact","b")])
"#;
    // b: keyword rank 2 + temporal rank 1; a: keyword rank 1 only -> 1/62 + 1/61
    // vs 1/61. No tie here, but the pin keeps the fallback ordering honest.
    assert_eq!(run_probe_window(probe), "b|a False");
}

/// Ruling O-4: the temporal leg stops voting in POINT mode. Measured on 50 identical
/// LongMemEval extractions, paired: `W_TEMPORAL=0` gives R@1 84.0 vs 74.0 and R@5 98.0
/// vs 96.0, flips 11:1 (sign test p=0.0063), and in 100 runs the leg never carried a hit
/// alone. Its value is the as-of cut, not the fusion vote — so the leg keeps running,
/// it just contributes a zero term.
#[test]
fn in_point_mode_the_temporal_leg_no_longer_votes() {
    let probe = r#"
def mk(i): return {"kind": "fact", "id": i}
lists = {"keyword": [mk("a"), mk("b")], "semantic": [mk("a"), mk("b")], "graph": [],
         "temporal": [mk("b"), mk("c")]}
w = fusion_weights({})
print(w["temporal"], w["keyword"])
r, s, l = fuse_rank(lists, w)
print("|".join(k[1] for k in r))
print(round(s[("fact","c")], 8), l[("fact","c")])
print(round(s[("fact","a")], 8), round(s[("fact","b")], 8))
"#;
    // a: keyword 1 + semantic 1. b: keyword 2 + semantic 2 + temporal 1 -- under the old
    // weight the third leg alone carried b past a (0.04865151 > 0.03278689). It does not
    // any more. c is a temporal-ONLY candidate: it stays a candidate, with score 0.
    assert_eq!(
        run_probe_window(probe),
        "0.0 1.0\na|b|c\n0.0 ['temporal']\n0.03278689 0.03225806"
    );
}

/// The counterpart: in WINDOW mode the leg keeps its weight, because the one paired win of
/// the weighted arm was a temporal-reasoning question and window mode is where the temporal
/// ordering IS the answer (O-2). Same lists, opposite outcome.
#[test]
fn in_window_mode_the_temporal_leg_keeps_its_weight() {
    let probe = r#"
def mk(i): return {"kind": "fact", "id": i}
lists = {"keyword": [mk("a"), mk("b")], "semantic": [mk("a"), mk("b")], "graph": [],
         "temporal": [mk("b"), mk("c")]}
w = fusion_weights({"recall_window_from": "2026-08-01T00:00:00Z",
                    "recall_window_to": "2026-09-01T00:00:00Z"})
print(w["temporal"])
r, s, _l = fuse_rank(lists, w)
print("|".join(k[1] for k in r))
print(round(s[("fact","b")], 8), round(s[("fact","a")], 8), round(s[("fact","c")], 8))
"#;
    assert_eq!(
        run_probe_window(probe),
        "1.0\nb|a|c\n0.04865151 0.03278689 0.01612903"
    );
}

/// O-4 must not eat O-2: the tie-break reads the temporal POSITION, not the temporal score.
/// Both candidates sit on keyword 1/2 against semantic 2/1, which is a bit-identical sum
/// either way — with the leg's weight at 0 the temporal RANK still decides, in both input
/// orders.
#[test]
fn the_o2_tie_break_survives_a_zero_weight_temporal_leg() {
    let probe = r#"
def mk(i): return {"kind": "fact", "id": i}
a = {"keyword": [mk("a"), mk("b")], "semantic": [mk("b"), mk("a")], "graph": [],
     "temporal": [mk("b"), mk("a")]}
b = {"keyword": [mk("a"), mk("b")], "semantic": [mk("b"), mk("a")], "graph": [],
     "temporal": [mk("a"), mk("b")]}
w = fusion_weights({})
ra, sa, _la = fuse_rank(a, w)
rb, _sb, _lb = fuse_rank(b, w)
print("|".join(k[1] for k in ra))
print("|".join(k[1] for k in rb))
print(sa[("fact","a")] == sa[("fact","b")], round(sa[("fact","a")], 8), w["temporal"])
"#;
    assert_eq!(run_probe_window(probe), "b|a\na|b\nTrue 0.03252247 0.0");
}

/// Ruling O-4b: when several hits of one `(subject, predicate)` axis collapse onto a single
/// candidate (R4 projection, first hit wins), the survivor carries the UNION of the legs of
/// ALL collapsed hits — deduplicated and in `LEG_ORDER`. `found by leg X` is a statement
/// about DISCOVERY, not about vote weight, and the attribution belongs to the axis rather
/// than to whichever hit happened to rank first.
#[test]
fn the_axis_collapse_unites_the_legs_of_every_collapsed_hit() {
    let probe = r#"
# The T2 shape: one hit found by keyword only (the superseded fact, keyword rank 1) and one
# found by the temporal leg only. Both project onto the same current fact of the axis.
first = {"rank": 1, "kind": "fact", "id": "berlin", "score": 0.01639344, "legs": ["keyword"]}
merge_legs(first, ["temporal"])
print(first["legs"], first["score"], first["rank"], first["id"])
# LEG_ORDER decides the order, not the order of arrival.
late = {"score": 0.5, "legs": ["temporal"]}
merge_legs(late, ["graph", "keyword"])
print(late["legs"], late["score"])
# A leg that is already there stays exactly once.
dup = {"legs": ["keyword", "temporal"]}
merge_legs(dup, ["temporal", "keyword"])
print(dup["legs"])
"#;
    // Score, rank and id are the FIRST winner's and stay untouched: this is attribution,
    // never re-ranking.
    assert_eq!(
        run_probe_window(probe),
        "['keyword', 'temporal'] 0.01639344 1 berlin\n\
         ['keyword', 'graph', 'temporal'] 0.5\n\
         ['keyword', 'temporal']"
    );
}

#[test]
fn a_window_over_a_multivalued_axis_leaves_every_span_open() {
    // Ruling O-3 in the window leg: only an explicit valid_until closes a span
    // on an enumeration axis. Before the ruling both sons "ended" when the third
    // arrived, so a window after that instant returned nothing at all. The two
    // sessions on the coexisting pair are the W1 session guard (#13, ruling Q3):
    // since then one instant is evidence only across two conversations.
    let probe = r#"
rows = [
 {"id":"a","subject":"s","predicate":"hat Sohn","claim":"Mika","session_id":"s1",
  "valid_from":"2026-01-01T00:00:00Z","valid_until":None,"recorded_at":"2026-01-01T00:00:01Z"},
 {"id":"b","subject":"s","predicate":"hat Sohn","claim":"Noa","session_id":"s2",
  "valid_from":"2026-01-01T00:00:00Z","valid_until":None,"recorded_at":"2026-01-01T00:00:02Z"},
 {"id":"c","subject":"s","predicate":"hat Sohn","claim":"Nova",
  "valid_from":"2026-06-01T00:00:00Z","valid_until":"2026-07-01T00:00:00Z",
  "recorded_at":"2026-06-01T00:00:00Z"},
]
hits = window_hits(rows, "2026-08-01T00:00:00Z", "2026-12-01T00:00:00Z")
print("|".join(f["claim"] for f in hits))
print(fact_span("a", rows)["span"], fact_span("c", rows)["span"]["until"])
"#;
    assert_eq!(
        run_probe_window(probe),
        "Mika|Noa\n{'from': '2026-01-01T00:00:00Z', 'until': None} 2026-07-01T00:00:00Z"
    );
}

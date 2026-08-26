//! GH #297 -- a leg that does not vote does not nominate.
//!
//! Ruling O-4 says the temporal leg "earns its keep through the as-of / window
//! CUT, not through its fusion vote" (`fusion_weights` in the shipped recall
//! script). In point mode its weight is `W_TEMPORAL_POINT` (0.0), so every
//! candidate it contributes enters the score table with a term of exactly 0.0 --
//! and then *stays* there, because the ranking sorted over the whole table. The
//! zero-weight leg kept filling the bundle it was not allowed to vote in.
//!
//! Same probe pattern as `p15_recall_fusion_budget.rs`: the REAL
//! `params.script_inline` runs, never a copy -- the `${VAR:-default}` literals
//! are resolved the way the colony resolves them at instantiation, the module
//! body runs against a stub stdin, and the `park()` exit at its end is swallowed
//! so the probe can call `fuse_rank` directly with synthetic leg lists.

use std::io::Write;
use std::process::{Command, Stdio};

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

/// Hand a probe program to python3 **on stdin**, never in argv.
///
/// A probe embeds the whole shipped script as a literal, and a single argv
/// string is capped at 128 KiB (`MAX_ARG_STRLEN`). The recall script crossed
/// that line in W2, and the failure mode is an opaque `ArgumentListTooLong`
/// that looks like a broken test rather than like a size limit. `python3 -`
/// reads and compiles the whole program from stdin before it runs a line of
/// it, so the probe's own `sys.stdin` replacement below is unaffected.
fn run_python(src: &str) -> std::process::Output {
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    // Dropped, not merely borrowed: python reads until EOF.
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    child.wait_with_output().expect("wait")
}

fn run_probe(probe: &str) -> String {
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
    let out = run_python(&src);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Shared preamble: a compact writer for synthetic leg entries.
const MK: &str = r#"
mkf = lambda i: {"kind": "fact", "id": i}
"#;

// ---------------------------------------------------------------- full-document harness
// The probe above reaches into the module's functions. The tests further down
// drive whole PHASES, because the semantic floor is a seam between two of them:
// `t1-sem` has the distances and no bit width, `t1-fuse` has the bit width and
// the distances the first phase carried forward.

/// Run the shipped script over a real stdin document, handing the program to
/// python3 **on stdin** (a3ce843a: a single argv string is capped at 128 KiB and
/// the recall script crossed that line in W2).
fn emit(doc: serde_json::Value) -> Vec<serde_json::Value> {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        serde_json::to_string(&recall_script()).unwrap(),
        serde_json::to_string(&meclaw_testing::code_stdin(&doc).to_string()).unwrap(),
    );
    let out = run_python(&src);
    assert!(
        out.status.success(),
        "recall exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// Every store op an emission carries, in call order.
///
/// Since GH #418 a recall hop puts N ops into ONE message (#295), so an op is a
/// CALL of a message rather than a message of its own. Reading calls instead of
/// `messages[0]` is what keeps the assertions below measuring the op they were
/// written for instead of the message boundary around it.
fn ops_of(msgs: &[serde_json::Value]) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    for m in msgs {
        let Some(turns) = m["messages"].as_array() else {
            continue;
        };
        for t in turns {
            let Some(text) = t["text"].as_str() else {
                continue;
            };
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
                out.push(v);
            }
        }
    }
    out
}

/// The payload a phase wrote into `recall_scratch` under `leg`.
fn scratch_payload(msgs: &[serde_json::Value], leg: &str) -> serde_json::Value {
    for args in ops_of(msgs) {
        if args["operation"] == "insert" && args["row"]["leg"] == leg {
            let payload = args["row"]["payload"].as_str().expect("payload string");
            return serde_json::from_str(payload).expect("payload json");
        }
    }
    panic!("no scratch insert for leg {leg}");
}

/// The tier-1 fan, as the store answers it: ONE bundle, one leg per call.
///
/// Since GH #418 the five legs that need no query vector are one message and one
/// reply, and each is computed by the body that used to be its own phase. A leg
/// that is not named here is not in the reply -- which is what a query with no
/// usable token (no keyword pages) produces, and what the receiving side reads
/// as the empty list the substitute inserts used to be.
fn fan_reply(legs: &[(&str, serde_json::Value)]) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"mem_phase": "t1-fan", "recall_id": "r1", "memory_tier": "1",
                        "recall_query": "what do I prefer?",
                        "recall_as_of": "2026-08-12T00:00:00Z",
                        "recall_window_from": "", "recall_window_to": "",
                        "audience_now": ["u1"], "channel": "chat"},
            "hop": {"operation": "bundle", "rows_affected": 1, "bundle_errors": 0}
        },
        "messages": legs.iter().map(|(id, rows)| serde_json::json!(
            {"origin": "tool", "type": "tool_result", "id": id, "text": rows.to_string()}))
            .collect::<Vec<_>>(),
        "results": legs.iter().map(|(id, _)| serde_json::json!(
            {"tool_call_id": id, "operation": "select", "rows_affected": 1,
             "duration_ms": 0})).collect::<Vec<_>>()
    })
}

/// One leg out of the single `legs` row the fan parks.
fn fan_leg(msgs: &[serde_json::Value], leg: &str) -> serde_json::Value {
    scratch_payload(msgs, "legs")[leg].clone()
}

/// The document a phase of a tier-1 request receives.
fn phase_doc(phase: &str, op: &str, rows: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"mem_phase": phase, "recall_id": "r1", "memory_tier": "1",
                        "recall_query": "what do I prefer?",
                        "recall_as_of": "2026-08-12T00:00:00Z",
                        "recall_window_from": "", "recall_window_to": "",
                        "audience_now": ["u1"], "channel": "chat"},
            "hop": {"operation": op}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r",
                      "text": rows.to_string()}]
    })
}

/// What `similar` answers with: the columns the leg asked for plus the computed
/// `distance` (`store::query::DISTANCE_COLUMN`, smaller is better).
fn similar_rows(distances: &[i64]) -> serde_json::Value {
    serde_json::json!(
        distances
            .iter()
            .enumerate()
            .map(|(i, d)| serde_json::json!({
                "owner_id": format!("f{i}"), "model_id": "m-1",
                "owner_table": "facts", "distance": d
            }))
            .collect::<Vec<_>>()
    )
}

/// The companion page the semantic leg is gated with — every owning fact
/// visible to this round, so nothing but the FLOOR can remove a row.
fn sem_aud_rows(n: usize) -> serde_json::Value {
    serde_json::json!(
        (0..n)
            .map(|i| serde_json::json!({
                "id": format!("f{i}"), "channel": "chat", "audience_set": ["u1"]
            }))
            .collect::<Vec<_>>()
    )
}

/// One `recall_scratch` row as `t1-fuse`'s select returns it.
fn leg_row(leg: &str, payload: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({"request_id": "r1", "leg": leg,
                       "payload": payload.to_string(), "fired": 1})
}

/// The ids of a keyword leg's rank list, in the order the leg wrote them.
fn leg_ids(payload: &serde_json::Value) -> Vec<String> {
    let hits = if payload.is_array() {
        payload.clone()
    } else {
        payload["hits"].clone()
    };
    hits.as_array()
        .expect("rank list")
        .iter()
        .map(|h| h["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// What `search` answers with: the columns the leg asked for plus the computed
/// `rank` column (`bm25`, **smaller is better**, normally negative).
fn search_rows(prefix: &str, ranks: &[serde_json::Value]) -> serde_json::Value {
    serde_json::json!(
        ranks
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let mut row = serde_json::json!({
                    "id": format!("{prefix}{i}"), "channel": "chat",
                    "audience_set": ["u1"], "claim": "c", "content": "c"
                });
                if !r.is_null() {
                    row["rank"] = r.clone();
                }
                row
            })
            .collect::<Vec<_>>()
    )
}

/// The reply that reaches the FUSION.
///
/// Since GH #418 there is no `t1-fuse` phase to hand a scratch page to: a leg
/// phase parks and reads the parking place back in one bundle, and the hop whose
/// read-back holds every required leg is the hop that fuses. So a fusion fixture
/// is a bundle reply whose trailing `r-<phase>-read` turn carries the legs.
/// `t1-temporal` stands in for the completing hop -- it has to be one of them,
/// and which one it is says nothing about the fusion.
fn fusion_reply(legs: &[(&str, serde_json::Value)]) -> serde_json::Value {
    let mut store_legs = serde_json::Map::new();
    let (mut sem, mut sem_aud) = (serde_json::json!([]), serde_json::json!([]));
    let mut graph: Vec<serde_json::Value> = Vec::new();
    for (name, payload) in legs {
        match *name {
            "sem" => sem = payload.clone(),
            "sem-aud" => sem_aud = payload.clone(),
            // The graph leg is handed over as the WALK it is computed from: the
            // ranking, the dedup and the gate on the edge all run in this hop.
            "graph" => graph = payload.as_array().cloned().unwrap_or_default(),
            _ => {
                store_legs.insert((*name).to_string(), payload.clone());
            }
        }
    }
    let n = graph.len();
    let paths: Vec<serde_json::Value> = graph
        .iter()
        .enumerate()
        .map(|(i, c)| {
            serde_json::json!({"node": format!("n{i}"), "depth": 1,
                               "weight_sum": (n - i) as i64,
                               "edge": {"episode_id": c["id"], "channel": "chat",
                                        "audience_set": ["u1"]}})
        })
        .collect();
    let page = serde_json::json!([
        {"request_id": "r1", "leg": "legs", "fired": 0,
         "payload": serde_json::Value::Object(store_legs).to_string()},
        {"request_id": "r1", "leg": "sem", "fired": 0, "payload": sem.to_string()},
    ]);
    serde_json::json!({
        "header": {
            "context": {"mem_phase": "t1-legs", "recall_id": "r1", "memory_tier": "1",
                        "recall_query": "what do I prefer?",
                        "recall_as_of": "2026-08-12T00:00:00Z",
                        "recall_window_from": "", "recall_window_to": "",
                        "audience_now": ["u1"], "channel": "chat"},
            "hop": {"operation": "bundle", "rows_affected": 1, "bundle_errors": 0}
        },
        "messages": [
            {"origin": "tool", "type": "tool_result", "id": "r-legs-graph",
             "text": serde_json::json!({"paths": paths}).to_string()},
            {"origin": "tool", "type": "tool_result", "id": "r-legs-sem-aud",
             "text": sem_aud.to_string()},
            {"origin": "tool", "type": "tool_result", "id": "r-legs-read",
             "text": page.to_string()},
        ],
        "results": [
            {"tool_call_id": "r-legs-graph", "operation": "traverse",
             "rows_affected": 1, "duration_ms": 0},
            {"tool_call_id": "r-legs-sem-aud", "operation": "select",
             "rows_affected": 1, "duration_ms": 0},
            {"tool_call_id": "r-legs-read", "operation": "select",
             "rows_affected": 2, "duration_ms": 0},
        ]
    })
}

/// The fusion document: the semantic leg exactly as `t1-sem` wrote it, its
/// companion audience page, the `model` leg carrying the embedding's bit width,
/// and the three other legs empty.
fn fuse_doc(sem: &serde_json::Value, model: serde_json::Value, n: usize) -> serde_json::Value {
    fusion_reply(&[
        ("kw-ep", serde_json::json!([])),
        ("kw-fact", serde_json::json!([])),
        ("graph", serde_json::json!([])),
        ("temporal", serde_json::json!([])),
        ("sem", sem.clone()),
        ("sem-aud", sem_aud_rows(n)),
        ("model", model),
    ])
}

/// A fusion document built from an explicit list of scratch legs — the four
/// rank lists, `sem-aud`, `model` and whatever bookkeeping legs a test names.
fn fuse_doc_legs(legs: &[(&str, serde_json::Value)]) -> serde_json::Value {
    fusion_reply(legs)
}

/// The semantic leg as the shipped `t1-sem` phase writes it for these distances.
fn sem_leg(distances: &[i64]) -> serde_json::Value {
    // #418: the semantic leg is computed in the reply of `t1-join` -- the
    // rendezvous asked `similar` there, and the walk it is parked next to is
    // asked in the same message. The leg's own body is unchanged.
    let join = serde_json::json!({
        "header": {
            "context": {"mem_phase": "t1-join", "recall_id": "r1", "memory_tier": "1",
                        "recall_query": "what do I prefer?",
                        "recall_as_of": "2026-08-12T00:00:00Z",
                        "recall_window_from": "", "recall_window_to": "",
                        "audience_now": ["u1"], "channel": "chat"},
            "hop": {"operation": "bundle", "rows_affected": 1, "bundle_errors": 0}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r-join-sem",
                      "text": similar_rows(distances).to_string()}],
        "results": [{"tool_call_id": "r-join-sem", "operation": "similar",
                     "rows_affected": 1, "duration_ms": 0}]
    });
    scratch_payload(&emit(join), "sem")
}

/// The ids the fusion nominated, in order.
fn fused_ids(fused: &serde_json::Value) -> Vec<String> {
    fused["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .map(|c| c["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Distances of a page of five: two clearly closer than a coin flip, one
/// exactly AT it, two beyond. `dim` is 1024, so the floor sits at 512 bits.
const FIVE: [i64; 5] = [10, 200, 512, 700, 900];

#[test]
fn the_semantic_leg_returns_fewer_than_its_limit_when_the_store_has_nothing_close() {
    // `similar` always answers with `limit` rows if the table holds that many —
    // it ranks, it does not filter, so a query with no near neighbour at all
    // still gets twenty. Half the bits differing is where a RANDOM binary vector sits,
    // so a hit at or beyond that distance is one the store itself cannot tell
    // from noise, and it votes in the fusion exactly as loudly as a real hit.
    let sem = sem_leg(&FIVE);
    // Step one of the seam: the distance the store computed survives the leg.
    assert_eq!(
        sem[0]["distance"], 10,
        "the leg keeps the distance it was ranked by: {sem}"
    );

    let fused = scratch_payload(
        &emit(fuse_doc(&sem, serde_json::json!({"dim": 1024}), 5)),
        "fused",
    );

    assert_eq!(
        fused["leg_sizes"]["semantic"], 2,
        "three of five hits are at or beyond the coin flip: {fused}"
    );
    assert_eq!(
        fused_ids(&fused),
        vec!["f0".to_string(), "f1".to_string()],
        "and the survivors keep the leg's own order: {fused}"
    );
    // The pre-floor count is what tells "memory is empty" from "nothing cleared
    // the floor" — one is a store with nothing in it, the other a store whose
    // answer was all noise, and a reader has to be able to say which.
    assert_eq!(
        fused["leg_sizes_raw"]["semantic"], 5,
        "the size before the cut is recorded: {fused}"
    );
}

#[test]
fn a_leg_with_no_bit_width_in_hand_keeps_every_hit() {
    // A floor without a scale is a guess, and a guess must not cut. The bit
    // width comes from `emb_models` via the `model` leg; when that leg never
    // arrived, or arrived without a usable `dim`, the leg travels untouched.
    for model in [
        serde_json::json!({"model_id": "m-1"}),
        serde_json::json!({"model_id": "m-1", "dim": 0}),
        serde_json::json!({"model_id": "m-1", "dim": serde_json::Value::Null}),
        serde_json::json!({"model_id": "m-1", "dim": "wide"}),
    ] {
        let sem = sem_leg(&FIVE);
        let fused = scratch_payload(&emit(fuse_doc(&sem, model.clone(), 5)), "fused");

        assert_eq!(
            fused["leg_sizes"]["semantic"], 5,
            "no scale, no cut ({model}): {fused}"
        );
        assert_eq!(
            fused["leg_sizes_raw"]["semantic"], 5,
            "raw and cut agree when nothing was cut ({model}): {fused}"
        );
    }
}

#[test]
fn a_zero_weight_leg_does_not_fill_the_bundle() {
    // Point mode (`fusion_weights` returns W with temporal := W_TEMPORAL_POINT).
    // One keyword candidate, six semantic ones, an empty graph leg and twenty
    // temporal candidates nobody else saw. The temporal leg has no vote here, so
    // the bundle must be exactly the seven candidates the voting legs nominated.
    //
    // Measured before the fix (2026-08-21): 20 entries, of which 13 were `t*`
    // carrying a score of exactly 0.0 -- candidates that reached the bundle
    // without a single leg voting for them.
    let probe = format!(
        "{}{}",
        MK,
        r#"
lists = {"keyword": [mkf("answer")],
         "semantic": [mkf("w%d" % i) for i in range(6)],
         "graph": [],
         "temporal": [mkf("t%d" % i) for i in range(20)]}
w = dict(W)
w["temporal"] = W_TEMPORAL_POINT
ranked, scores, legs, _voters = fuse_rank(lists, w)
print(len(ranked))
print(any(k[1].startswith("t") for k in ranked))
print([k[1] for k in ranked])
"#
    );
    assert_eq!(
        run_probe(&probe),
        "7\nFalse\n['answer', 'w0', 'w1', 'w2', 'w3', 'w4', 'w5']"
    );
}

#[test]
fn the_temporal_leg_still_breaks_an_exact_tie_at_weight_zero() {
    // The O-2 pin, restated so the repair cannot take it down. Symmetric ranks
    // across keyword and semantic produce bit-identical sums (1/61 + 1/62 either
    // way), and adding the zero-weight leg's 0.0 term does not perturb the float.
    // Both candidates are nominated by the voting legs, so both survive the
    // filter -- and the one the temporal leg SAW must still sort first.
    let probe = format!(
        "{}{}",
        MK,
        r#"
lists = {"keyword": [mkf("A"), mkf("B")],
         "semantic": [mkf("B"), mkf("A")],
         "graph": [],
         "temporal": [mkf("B")]}
w = dict(W)
w["temporal"] = W_TEMPORAL_POINT
ranked, scores, legs, _voters = fuse_rank(lists, w)
print([k[1] for k in ranked])
print(scores[("fact", "A")] == scores[("fact", "B")])
"#
    );
    assert_eq!(run_probe(&probe), "['B', 'A']\nTrue");
}

#[test]
fn in_window_mode_the_temporal_leg_nominates_again() {
    // Window mode (`fusion_weights` returns W unchanged): there the temporal
    // ordering IS the answer (O-2), the leg votes, and therefore it nominates.
    // The same lists that shrink to seven in point mode fill the bundle here.
    let probe = format!(
        "{}{}",
        MK,
        r#"
lists = {"keyword": [mkf("answer")],
         "semantic": [mkf("w%d" % i) for i in range(6)],
         "graph": [],
         "temporal": [mkf("t%d" % i) for i in range(20)]}
ranked, scores, legs, _voters = fuse_rank(lists, W)
print(len(ranked))
print(sum(1 for k in ranked if k[1].startswith("t")))
print(all(scores[k] > 0.0 for k in ranked))
"#
    );
    assert_eq!(run_probe(&probe), "20\n13\nTrue");
}

// -------------------------------------------------- the keyword floor (#297, Q8)
// `search` RANKS, it does not filter — with a trailing-prefix FTS query (`fts_match`)
// a single strong hit arrives together with a tail of prefix-token noise, and every
// one of those noise rows votes in the fusion as loudly as the strong one. The floor
// is a FRACTION OF THE LEG'S OWN BEST RANK, so no leg can be measured against
// another leg's scale.

/// The shape #297 measured: one strong hit, two more real ones, and a tail of
/// prefix-token noise. With the default ratio 0.10 the floor sits at -0.9.
const SIX: [f64; 6] = [-9.0, -8.2, -7.4, -0.4, -0.2, -0.05];

fn six_ranks() -> Vec<serde_json::Value> {
    SIX.iter().map(|r| serde_json::json!(r)).collect()
}

#[test]
fn the_keyword_leg_drops_hits_its_own_best_dwarfs() {
    let msgs = emit(fan_reply(&[(
        "r-fan-kw-ep",
        search_rows("e", &six_ranks()),
    )]));
    let leg = fan_leg(&msgs, "kw-ep");
    assert_eq!(
        leg_ids(&leg),
        vec!["e0".to_string(), "e1".to_string(), "e2".to_string()],
        "three hits clear a floor of 0.10 * -9.0, and they keep the leg's order: {leg}"
    );
}

#[test]
fn a_leg_whose_best_hit_carries_no_signal_is_not_cut() {
    // A floor without a scale is a guess and a guess must not cut. `bm25` is
    // normally negative; an absent, null or zero best rank is a leg carrying no
    // usable signal at all, and then nothing is removed.
    for ranks in [
        vec![serde_json::Value::Null; 4],
        vec![serde_json::json!(null), serde_json::json!(null)],
        vec![
            serde_json::json!(0.0),
            serde_json::json!(0.0),
            serde_json::json!(0.0),
        ],
    ] {
        let n = ranks.len();
        let msgs = emit(fan_reply(&[("r-fan-kw-ep", search_rows("e", &ranks))]));
        let leg = fan_leg(&msgs, "kw-ep");
        assert_eq!(
            leg_ids(&leg).len(),
            n,
            "no usable best rank, no cut ({ranks:?}): {leg}"
        );
    }
}

#[test]
fn the_fact_search_is_floored_by_its_own_best_hit_not_the_episode_one() {
    // The fact leg's own distribution is shallower than the episode leg's above:
    // best -5.0, so the floor is -0.5 and the -0.4 tail row goes. Measured
    // against the episode leg's -9.0 the same row would have survived.
    let rows = serde_json::json!([
        {"id": "f0", "channel": "chat", "audience_set": ["u1"],
         "subject": "user:Alice", "claim": "c", "rank": -5.0},
        {"id": "f1", "channel": "chat", "audience_set": ["u1"],
         "subject": "user:Bob", "claim": "c", "rank": -1.0},
        {"id": "f2", "channel": "chat", "audience_set": ["u1"],
         "subject": "user:Zorro", "claim": "c", "rank": -0.4},
    ]);
    let msgs = emit(fan_reply(&[("r-fan-kw-fact", rows)]));
    let leg = fan_leg(&msgs, "kw-fact");
    assert_eq!(
        leg_ids(&leg),
        vec!["f0".to_string(), "f1".to_string()],
        "the cut uses the fact leg's own maximum: {leg}"
    );

    // ... and the graph anchors are read off the SURVIVING rows only, so a hit
    // below the floor cannot seed the graph leg through the back door either.
    // #418: the anchors are PARKED with the leg rather than asked for at once --
    // the walk lives behind the rendezvous, and asking before the vector is
    // there buys nothing. WHAT they are built from is unchanged.
    let anchors: Vec<String> = fan_leg(&msgs, "anchors")
        .as_array()
        .expect("anchor list")
        .iter()
        .map(|n| n.as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        anchors.contains(&"Alice".to_string()) && anchors.contains(&"Bob".to_string()),
        "the survivors seed the graph leg: {anchors:?}"
    );
    assert!(
        !anchors.iter().any(|a| a.eq_ignore_ascii_case("zorro")),
        "a hit below the floor seeds nothing: {anchors:?}"
    );
}

#[test]
fn the_diagnostic_reports_both_the_raw_and_the_floored_leg_size() {
    // Without this pair of numbers a floor regression is invisible in the eval:
    // `leg_sizes` alone reports the same 3 whether the store held three rows or
    // six of which three were noise.
    let msgs = emit(fan_reply(&[(
        "r-fan-kw-ep",
        search_rows("e", &six_ranks()),
    )]));
    let kw_ep = fan_leg(&msgs, "kw-ep");
    let kw_ep_raw = fan_leg(&msgs, "kw-ep-raw");

    let fused = scratch_payload(
        &emit(fuse_doc_legs(&[
            ("kw-ep", kw_ep),
            ("kw-ep-raw", kw_ep_raw),
            ("kw-fact", serde_json::json!([])),
            ("graph", serde_json::json!([])),
            ("temporal", serde_json::json!([])),
            ("sem", serde_json::json!([])),
            ("sem-aud", serde_json::json!([])),
            ("model", serde_json::json!({"dim": 1024})),
        ])),
        "fused",
    );
    assert_eq!(fused["leg_sizes"]["keyword"], 3, "floored: {fused}");
    assert_eq!(fused["leg_sizes_raw"]["keyword"], 6, "raw: {fused}");

    // ... and both numbers reach the trace `t1-emit` hands on.
    let emitted = emit(phase_doc(
        "t1-emit",
        "select",
        serde_json::json!([leg_row("fused", &fused)]),
    ));
    let diag = &emitted[0]["recall_diagnostic"];
    let raw = diag["leg_sizes_raw"]["keyword"].as_i64().expect("raw size");
    let cut = diag["leg_sizes"]["keyword"].as_i64().expect("cut size");
    assert!(
        raw > cut,
        "the diagnostic reports both sides of the floor ({raw} > {cut}): {diag}"
    );
}

// ----------------------------------------------- agreement is scored (#297, Q8)
// RRF pays for RANK, and a rank is one leg's opinion. Two legs that INDEPENDENTLY
// found the same row is a different kind of evidence than one leg finding it a
// little higher, and the plain sum leaves that difference in the leg tags nobody
// sums. `agreement` counts the VOTING legs behind a candidate and prices it.

/// A `t1-emit` document with an explicit window and an explicit set of scratch legs.
///
/// The window is what tells point mode from window mode at this phase — the
/// context slot is persistent, so `t1-emit` reads the same answer `t1-fuse` did.
fn emit_doc(window: (&str, &str), legs: &[(&str, serde_json::Value)]) -> serde_json::Value {
    let rows = serde_json::json!(
        legs.iter()
            .map(|(name, payload)| leg_row(name, payload))
            .collect::<Vec<_>>()
    );
    serde_json::json!({
        "header": {
            "context": {"mem_phase": "t1-emit", "recall_id": "r1", "memory_tier": "1",
                        "recall_query": "what do I prefer?",
                        "recall_as_of": "2026-08-12T00:00:00Z",
                        "recall_window_from": window.0, "recall_window_to": window.1,
                        "audience_now": ["u1"], "channel": "chat"},
            "hop": {"operation": "select"}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r",
                      "text": rows.to_string()}]
    })
}

/// The bundle a tier-1 emission carries in `system.memory.bundle`.
fn bundle_text(msgs: &[serde_json::Value]) -> String {
    msgs.iter()
        .find(|m| m["header"]["route"] == "bundle")
        .expect("a tier-1 request emits a bundle message")["system"]["memory"]["bundle"]["text"]
        .as_str()
        .expect("bundle text")
        .to_string()
}

#[test]
fn a_two_leg_hit_outscores_a_one_leg_hit_at_the_same_rank() {
    // `a` is at keyword rank 2 and nowhere else; `b` is at semantic rank 2 AND at
    // graph rank 2. Both sit at the same rank, so RRF's own term is identical --
    // the only difference between them is how many legs agreed.
    let probe = format!(
        "{}{}",
        MK,
        r#"
lists = {"keyword": [mkf("k0"), mkf("a")],
         "semantic": [mkf("s0"), mkf("b")],
         "graph": [mkf("g0"), mkf("b")],
         "temporal": []}
ranked, scores, legs, voters = fuse_rank(lists, W)
print(repr(scores[("fact", "a")]))
print(repr(scores[("fact", "b")]))
print(voters[("fact", "a")], voters[("fact", "b")])
"#
    );
    let out = run_probe(&probe);
    // Exact values, so the multiplier is pinned rather than merely the order:
    // 1/62 for the one-leg hit, 2/62 * (1 + 0.5 * 1) for the two-leg one.
    assert_eq!(
        out, "0.016129032258064516\n0.04838709677419355\n1 2",
        "the agreement factor is priced, not just the ranks"
    );
    let lines: Vec<&str> = out.lines().collect();
    let one: f64 = lines[0].parse().expect("one-leg score");
    let two: f64 = lines[1].parse().expect("two-leg score");
    assert!(
        two > 2.0 * one,
        "the gap must EXCEED the plain RRF sum ({two} vs {}), otherwise agreement \
         is still only a tag",
        2.0 * one
    );
}

#[test]
fn agreement_counts_voting_legs_only() {
    // The precise defect #297 measured: "rank 4, found by TWO legs, identical
    // score". The second leg was the temporal one, which has no vote in point
    // mode -- so there was no second opinion to pay for. `legs` still lists both
    // (discovery, ruling O-4b); `agreement` says how much of that discovery counted.
    let probe = format!(
        "{}{}",
        MK,
        r#"
lists = {"keyword": [mkf("c")], "semantic": [], "graph": [], "temporal": [mkf("c")]}
w = dict(W)
w["temporal"] = W_TEMPORAL_POINT
ranked, scores, legs, voters = fuse_rank(lists, w)
print(voters[("fact", "c")])
print(repr(scores[("fact", "c")]))
print(legs[("fact", "c")])
"#
    );
    assert_eq!(
        run_probe(&probe),
        "1\n0.01639344262295082\n['keyword', 'temporal']",
        "one voting leg, no bonus -- and the discovery attribution is untouched"
    );
}

#[test]
fn the_agreement_count_reaches_the_diagnostic_record() {
    // End to end: the keyword and the semantic leg both find `f0`, so the fusion
    // records an agreement of two, and the record `t1-emit` hands on carries it.
    let fused = scratch_payload(
        &emit(fuse_doc_legs(&[
            ("kw-ep", serde_json::json!([])),
            ("kw-fact", serde_json::json!([{"kind": "fact", "id": "f0"}])),
            ("graph", serde_json::json!([])),
            ("temporal", serde_json::json!([])),
            ("sem", serde_json::json!([{"kind": "fact", "id": "f0"}])),
            (
                "sem-aud",
                serde_json::json!([{"id": "f0", "channel": "chat", "audience_set": ["u1"]}]),
            ),
            ("model", serde_json::json!({"dim": 1024})),
        ])),
        "fused",
    );
    assert_eq!(
        fused["candidates"][0]["agreement"], 2,
        "two voting legs found it: {fused}"
    );

    let msgs = emit(emit_doc(
        ("", ""),
        &[
            ("fused", fused),
            (
                "hyd-fact",
                serde_json::json!([{"id": "f0", "claim": "coffee", "subject": "user:m",
                                    "predicate": "likes", "valid_from": "2026-01-01T00:00:00Z",
                                    "channel": "chat", "audience_set": ["u1"]}]),
            ),
            ("hyd-ep", serde_json::json!([])),
            ("hyd-axis", serde_json::json!([])),
        ],
    ));
    let diag = &msgs
        .iter()
        .find(|m| m["header"]["route"] == "bundle")
        .expect("a bundle message")["recall_diagnostic"];
    assert_eq!(
        diag["candidates"][0]["agreement"], 2,
        "the record carries the count: {diag}"
    );
    // ... and it stays there. `agreement` is retrieval bookkeeping (#279's list):
    // a reader answering a question can do nothing with it, and every byte of it
    // is prompt budget the answer does not get.
    let text = bundle_text(&msgs);
    assert!(
        !text.contains("agreement"),
        "the payload carries no vote bookkeeping: {text}"
    );
}

#[test]
fn a_capped_leg_without_a_vote_does_not_make_the_answer_incomplete() {
    // The completeness seam, ruled with the agreement factor: a cap is a reason
    // to doubt the ANSWER only if the capped leg could have added a candidate.
    // In point mode the temporal leg does not vote and therefore nominates
    // nothing (Task 8) -- a full temporal page cannot have cost the bundle a
    // single row, and reporting it as a cut is a hedge with no cause behind it.
    // The raw flag stays in the diagnostic either way: what the page reported is
    // a fact about the run, and only its READING as a cut is mode-dependent.
    let fused = serde_json::json!({
        "candidates": [{"kind": "episode", "id": "e0", "score": 0.09,
                        "legs": ["keyword"], "agreement": 1}],
        "legs_present": ["keyword", "temporal"],
        "leg_sizes": {"keyword": 1, "semantic": 0, "graph": 0, "temporal": 20},
        "leg_sizes_raw": {"keyword": 1, "semantic": 0, "graph": 0, "temporal": 20},
        "leg_capped": {"temporal": 20},
        "semantic_degraded": true
    });
    let hyd_ep = serde_json::json!([{"id": "e0", "session_id": "s-1", "sender": "user",
                                     "content": "coffee", "happened_at": "2026-01-01T09:00:00Z",
                                     "recorded_at": "2026-01-01T09:00:00Z"}]);
    let legs = |window: (&str, &str)| {
        emit_doc(
            window,
            &[
                ("fused", fused.clone()),
                ("hyd-ep", hyd_ep.clone()),
                ("hyd-fact", serde_json::json!([])),
                ("hyd-axis", serde_json::json!([])),
            ],
        )
    };

    let msgs = emit(legs(("", "")));
    let b: serde_json::Value = serde_json::from_str(&bundle_text(&msgs)).expect("bundle json");
    assert_eq!(
        b["complete"], true,
        "a leg with no vote cannot have cut the answer: {}",
        b["complete_reason"]
    );
    assert!(
        b.get("complete_reason").is_none(),
        "a complete bundle names no cut: {b}"
    );
    let diag = &msgs
        .iter()
        .find(|m| m["header"]["route"] == "bundle")
        .expect("a bundle message")["recall_diagnostic"];
    assert_eq!(
        diag["leg_capped"]["temporal"], 20,
        "what the page reported is untouched: {diag}"
    );

    // Window mode: there the temporal leg votes (O-4), so its cap counts again.
    let msgs = emit(legs(("2026-01-01T00:00:00Z", "2026-02-01T00:00:00Z")));
    let b: serde_json::Value = serde_json::from_str(&bundle_text(&msgs)).expect("bundle json");
    assert_eq!(b["complete"], false, "a voting leg's cap is a cut: {b}");
    let reason = b["complete_reason"].as_str().expect("a reason");
    assert!(
        reason.contains("temporal") && reason.contains("20"),
        "the reason names the leg and its limit: {reason:?}"
    );
}

// ------------------------------------- a recall that finds nothing says so (#297, Q8)
// A bundle with no candidate used to be a document with a header and no rows.
// A model handed that reads it as "the lookup happened, here is what it holds"
// and answers the question from somewhere else -- which is the failure the empty
// state exists to prevent. The bundle now SAYS it holds no answer, in one
// sentence, and the header carries the same verdict for a router.

/// A fused record with no surviving candidate. `raw` is the pre-floor map
/// (`leg_sizes_raw`, Task 10) — `None` stands for an in-flight request that
/// predates the floors and carries no raw information at all.
fn empty_fused(raw: Option<serde_json::Value>) -> serde_json::Value {
    let mut fused = serde_json::json!({
        "candidates": [],
        "legs_present": [],
        "leg_sizes": {"keyword": 0, "semantic": 0, "graph": 0, "temporal": 0},
        "leg_capped": {},
        // The floor emptied the semantic list, so this flag is true here EXACTLY
        // as it is for a dead embedder — which is why the sentence below may
        // never read it as one.
        "semantic_degraded": true
    });
    if let Some(raw) = raw {
        fused["leg_sizes_raw"] = raw;
    }
    fused
}

/// The bundle message of a tier-1 emission.
fn bundle_message(msgs: &[serde_json::Value]) -> serde_json::Value {
    msgs.iter()
        .find(|m| m["header"]["route"] == "bundle")
        .expect("a tier-1 request emits a bundle message")
        .clone()
}

/// A `t1-emit` run over a fused record with empty hydration pages.
fn emit_fused(fused: serde_json::Value) -> Vec<serde_json::Value> {
    emit(emit_doc(
        ("", ""),
        &[
            ("fused", fused),
            ("hyd-fact", serde_json::json!([])),
            ("hyd-ep", serde_json::json!([])),
            ("hyd-axis", serde_json::json!([])),
        ],
    ))
}

#[test]
fn a_bundle_with_no_relevant_candidate_says_so() {
    let msgs = emit_fused(empty_fused(Some(serde_json::json!(
        {"keyword": 0, "semantic": 0, "graph": 0, "temporal": 0}
    ))));
    let msg = bundle_message(&msgs);
    let b: serde_json::Value = serde_json::from_str(&bundle_text(&msgs)).expect("bundle json");

    assert_eq!(
        b["answers"], "none",
        "the bundle states that it answers nothing: {b}"
    );
    assert!(
        b["candidates"].as_array().expect("a list").is_empty(),
        "and the list stays empty rather than being filled with a placeholder: {b}"
    );

    let text = msg["messages"][0]["text"].as_str().expect("rendered turn");
    assert_eq!(
        text.lines().count(),
        1,
        "one sentence, not a document with a header and no rows: {text:?}"
    );
    assert!(
        text.contains("Nothing in this memory answers"),
        "and the sentence says exactly that: {text:?}"
    );
    // The vocabulary of a document that HAS rows: a section header promises
    // something follows it, and the run's own description belongs to the
    // diagnostic (#279/#281).
    for word in ["FACTS", "WHAT WAS SAID", "RRF", "candidates"] {
        assert!(
            !text.contains(word),
            "an empty bundle does not speak like a full one ({word}): {text:?}"
        );
    }
    assert_eq!(
        msg["header"]["recall_empty"], "1",
        "and a router can read the same verdict off the header: {}",
        msg["header"]
    );
}

#[test]
fn a_bundle_that_answers_says_so_too() {
    let fused = serde_json::json!({
        "candidates": [{"kind": "episode", "id": "e0", "score": 0.09,
                        "legs": ["keyword"], "agreement": 1}],
        "legs_present": ["keyword"],
        "leg_sizes": {"keyword": 1, "semantic": 0, "graph": 0, "temporal": 0},
        "leg_sizes_raw": {"keyword": 1, "semantic": 0, "graph": 0, "temporal": 0},
        "leg_capped": {},
        "semantic_degraded": true
    });
    let msgs = emit(emit_doc(
        ("", ""),
        &[
            ("fused", fused),
            ("hyd-fact", serde_json::json!([])),
            (
                "hyd-ep",
                serde_json::json!([{"id": "e0", "session_id": "s-1", "sender": "user",
                                    "content": "coffee", "happened_at": "2026-01-01T09:00:00Z",
                                    "recorded_at": "2026-01-01T09:00:00Z"}]),
            ),
            ("hyd-axis", serde_json::json!([])),
        ],
    ));
    let msg = bundle_message(&msgs);
    let b: serde_json::Value = serde_json::from_str(&bundle_text(&msgs)).expect("bundle json");

    assert_eq!(
        b["answers"], "direct",
        "a bundle with rows answers from them: {b}"
    );
    assert!(
        msg["header"].get("recall_empty").is_none(),
        "and the marker is absent unless it is true, like every other one: {}",
        msg["header"]
    );
    let text = msg["messages"][0]["text"].as_str().expect("rendered turn");
    assert!(
        text.starts_with("WHAT THIS MEMORY HOLDS"),
        "the asserting header is untouched where there is something to assert: {text:?}"
    );
}

#[test]
fn the_empty_state_names_the_floor_when_a_floor_emptied_it() {
    // "Memory holds nothing about this" and "memory answered, and none of the
    // answers was close enough to count" are different states, and a caller has
    // to be able to tell them apart: the first is a gap in what was stored, the
    // second a question this store cannot serve well. `leg_sizes` reports 0 for
    // both; only the pre-floor map separates them.
    let msgs = emit_fused(empty_fused(Some(serde_json::json!(
        {"keyword": 4, "semantic": 2, "graph": 0, "temporal": 0}
    ))));
    let text = bundle_message(&msgs)["messages"][0]["text"]
        .as_str()
        .expect("rendered turn")
        .to_string();

    assert_eq!(text.lines().count(), 1, "still one sentence: {text:?}");
    assert!(
        text.contains("Nothing in this memory answers"),
        "the verdict is unchanged -- the reason is what is added: {text:?}"
    );
    // The COUNT, verbatim: the as-of day of every fixture here contains a '6',
    // so a bare digit search pins nothing at all.
    assert!(
        text.contains("6 stored hits came back for it")
            && text.contains("none of them cleared the relevance floor"),
        "four keyword hits and two semantic ones were found, and none of them \
         cleared its leg's floor: {text:?}"
    );
    // The semantic leg was emptied BY THE FLOOR, which sets `semantic_degraded`
    // exactly as a dead embedder does. The sentence a model reads may not turn
    // that into a claim about broken machinery — the raw count says the leg
    // answered, and that is the discriminator (Task 9 handoff).
    for word in ["embed", "degraded", "unavailable"] {
        assert!(
            !text.contains(word),
            "a floored leg is not a broken one ({word}): {text:?}"
        );
    }
}

#[test]
fn an_empty_bundle_does_not_hedge_about_a_list_it_does_not_have() {
    // The interaction of the empty state with #280's completeness sentence: the
    // keyword leg came back at its limit AND votes (point mode), so the bundle is
    // incomplete — and "Not everything that matches is here -- capped legs …"
    // under "Nothing in this memory answers this question" qualifies rows that do
    // not exist. Worse, it says the opposite of what the empty state is for: a
    // model reads "there is more" and answers from somewhere else. What the empty
    // case has to say about completeness it says in its own sentence.
    let mut fused = empty_fused(Some(serde_json::json!(
        {"keyword": 20, "semantic": 0, "graph": 0, "temporal": 0}
    )));
    fused["leg_capped"] = serde_json::json!({"keyword": 20});
    let msgs = emit_fused(fused);
    let msg = bundle_message(&msgs);
    let text = msg["messages"][0]["text"].as_str().expect("rendered turn");

    assert_eq!(text.lines().count(), 1, "one sentence, no hedge: {text:?}");
    assert!(
        !text.contains("Not everything that matches is here"),
        "a list that does not exist cannot be incomplete: {text:?}"
    );
    assert!(
        text.contains("20 stored hits came back for it"),
        "and the sentence carries the completeness information it DOES have: {text:?}"
    );

    // Nothing became harder to debug (#296): the cut is still in the bundle JSON
    // the caller parses, and the cap itself is still in the diagnostic.
    let b: serde_json::Value = serde_json::from_str(&bundle_text(&msgs)).expect("bundle json");
    assert_eq!(b["complete"], false, "the cut is recorded: {b}");
    assert!(
        b["complete_reason"]
            .as_str()
            .expect("a reason")
            .contains("keyword"),
        "…and it names the leg: {b}"
    );
    assert_eq!(
        msg["recall_diagnostic"]["leg_capped"]["keyword"], 20,
        "…and the page's own report is untouched: {}",
        msg["recall_diagnostic"]
    );
}

#[test]
fn an_empty_bundle_without_raw_leg_sizes_makes_no_claim_about_a_floor() {
    // Rollout guard: a request that was in flight when the floors landed carries
    // no `leg_sizes_raw` at all, and one that predates a leg's floor can report a
    // raw count BELOW the floored one. Neither may produce a negative count or a
    // floor clause invented out of missing information.
    for raw in [
        None,
        Some(serde_json::json!({})),
        Some(serde_json::json!({"keyword": 0, "semantic": 0})),
    ] {
        let msgs = emit_fused(empty_fused(raw.clone()));
        let text = bundle_message(&msgs)["messages"][0]["text"]
            .as_str()
            .expect("rendered turn")
            .to_string();
        assert!(
            text.contains("Nothing in this memory answers") && !text.contains("floor"),
            "no raw information, no claim about a floor ({raw:?}): {text:?}"
        );
    }
}

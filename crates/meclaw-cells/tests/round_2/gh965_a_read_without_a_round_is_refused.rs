//! GH #965 -- an app reads the member's RESIDENTS (`reads_residents`), and the
//! round of that read is the builder's word, never the app's.
//!
//! A screen app (the presenter) shows what the member's residents hold: the
//! memory, the files, the knowledge graph, the objects, the library, the
//! colony's counts, the last digest and the last research answers (GH #976).
//! Those residents are not apps and answer no `in_show`; the
//! app reads them over their own read lanes, one edge per resident, drawn by
//! the recipe from a declaration that names them:
//!
//! - ONE question edge from the app's RIM onto the resident's lane, on
//!   `resident_read` naming that resident; it stamps the asker
//!   (`context.resident_caller`), the asker's id (`context.resident_op`) and
//!   the MEMBER's round in `audience_now` AND `audience_set` -- over whatever
//!   round the app's message carries (review C-1 of GH #949 and of GH #965: a
//!   round an app sets on its own message is a self-report, and a resident's
//!   own pulls carry the context on to the next resident).
//! - per answering route ONE edge back onto the rim, guarded on the asker AND
//!   on the mark of this read (a `res:` id or the `resident` reply-to token),
//!   so the answers of a resident's inner pulls stay inside the member
//!   (review I-1); restamped `resident_answer` with the resident, the route it
//!   answered on, the asker's id and the round of the answer -- the member's,
//!   or `["*"]` for the colony's counts (counts and never content); every key
//!   the question stamped is deleted there (review I-2).
//! - the member's own catch-all edges on those answer routes leave a resident
//!   read alone, so an answer reaches the asker and nobody else.
//!
//! Pure script tests: the SHIPPED `recipes` and `classify` run over stdin,
//! nothing is booted (technique of `gh949_an_app_declares_reads.rs`); the
//! rendered edges and the member's own are then run through the colony's CEL
//! evaluator and modifier (technique of GH #951's locks), so a header is
//! followed edge by edge as the router follows it.

use meclaw_colony::cel_eval::{
    apply_modifier, evaluate_condition, parse_condition, parse_modifier,
};
use meclaw_colony::config::ModifierSpec;
use meclaw_core::Headers;
use meclaw_core::serde_json::{self, Map, Value, json};
use meclaw_testing::{emit_all, emit_one, shipped_script};

const RECIPES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/recipes/config.json"
);
const CLASSIFY: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/classify/config.json"
);
const MEMBER_CONFIG: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/member/config.json"
);

const MEMBER: &str = "/os/orgs/acme/members/alex";
/// The member's round as a CEL literal: generation `sam`, person `alex`.
const MEMBER_ROUND: &str = r#"'["agent:sam","member:alex"]'"#;
const EVERYBODY: &str = r#"'["*"]'"#;

/// The keys the member's `facts` edge writes into an object read's context.
const OBJECTS_INNER: [&str; 7] = [
    "channel",
    "memory_tier",
    "objects_subject",
    "recall_as_of",
    "recall_query",
    "recall_window_from",
    "recall_window_to",
];

/// What marks the answer of THIS read on the way back (review I-1): the `res:`
/// id the question edge wrote, or the `resident` reply-to token.
const RES_OP_ID: &str = "has(hop.op_id) && hop.op_id.startsWith('res:')";
const RES_RECALL: &str = "has(hop.recall_caller) && hop.recall_caller == 'resident'";
const RES_CALL_ID: &str = "has(hop.tool_call_id) && hop.tool_call_id.startsWith('res:')";
const RES_BRIEF: &str = "has(context.brief_caller) && context.brief_caller == 'resident'";

/// Every resident, its cell inside the member, the lane it is read on, the
/// mark its answer is recognised by, the routes that answer, and the round
/// its answer is stamped with. The ROUND of the question is the member's for
/// every one of them, in both keys (review C-1).
/// (resident, cell, lane, mark, answering routes, answer round).
type Resident = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static [&'static str],
    &'static str,
);

const RESIDENTS: [Resident; 9] = [
    (
        "memory-hive",
        "./memory-hive",
        "in_query",
        RES_RECALL,
        &["bundle", "reject"],
        MEMBER_ROUND,
    ),
    (
        "file-space",
        "./file-space",
        "in_read",
        RES_OP_ID,
        &["answer"],
        MEMBER_ROUND,
    ),
    (
        "graph-space",
        "./graph-space",
        "in_graph",
        RES_OP_ID,
        &["answer"],
        MEMBER_ROUND,
    ),
    (
        "objects",
        "./objects",
        "in_tool",
        RES_CALL_ID,
        &["tool_result"],
        MEMBER_ROUND,
    ),
    (
        "librarian",
        "./librarian",
        "in_lib",
        RES_OP_ID,
        &["answer"],
        MEMBER_ROUND,
    ),
    (
        "affinity",
        "./affinity",
        "in_brief",
        RES_BRIEF,
        &["answer", "error"],
        MEMBER_ROUND,
    ),
    (
        "colony-view",
        "./apps/colony-view",
        "in_read",
        RES_OP_ID,
        &["answer"],
        EVERYBODY,
    ),
    // GH #976 (PE-DP-9): the member's two apps that keep a result with the
    // round it was made for; each row carries its own round, the answer is
    // stamped with the member's.
    (
        "daily-digest",
        "./apps/daily-digest",
        "in_read",
        RES_OP_ID,
        &["answer"],
        MEMBER_ROUND,
    ),
    (
        "research-assistant",
        "./apps/research-assistant",
        "in_read",
        RES_OP_ID,
        &["answer"],
        MEMBER_ROUND,
    ),
];

fn run_recipes(params: &Value) -> Vec<Value> {
    emit_all(
        &shipped_script(RECIPES),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!({"recipe": "install_app", "request": "…",
                                         "params": params}).to_string()}],
        }),
    )
}

fn edges(params: &Value) -> Vec<Value> {
    let all = run_recipes(params);
    let first = all.first().expect("an emission");
    assert!(first["header"]["error_code"].is_null(), "refused: {first}");
    first["manifest"][0]["diff"]["add_edges"]
        .as_array()
        .expect("edges")
        .clone()
}

fn payload(out: &Value) -> Value {
    serde_json::from_str(out["messages"][0]["text"].as_str().expect("a payload")).expect("json")
}

fn refusal(params: &Value) -> Value {
    let all = run_recipes(params);
    let first = all.first().expect("an emission");
    assert_eq!(
        first["header"]["error_code"],
        json!("app_declaration_invalid"),
        "{first}"
    );
    payload(first)
}

fn params_for(app: &str, declaration: Value) -> Value {
    json!({"scope": MEMBER, "app": app, "template": format!("{app}@1.0.0"),
           "screen": "display", "generation": "sam", "ctx": {"member_person": "alex"},
           // The member has every resident: the two member apps (GH #976) are drawn
           // only when named (Y fix round 1, I-1).
           "residents_present": all_residents(),
           "declaration": declaration})
}

fn all_residents() -> Value {
    json!(RESIDENTS.iter().map(|r| r.0).collect::<Vec<_>>())
}

fn classify(params: Value) -> Value {
    emit_one(
        &shipped_script(CLASSIFY),
        &json!({
            "target": "/os/builder/classify",
            "header": {"hop": {"route": "in_build"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_call", "id": "c1",
                          "text": json!({"request": "install an app",
                                         "recipe": "install_app",
                                         "params": params}).to_string()}],
        }),
    )
}

/// Red before GH #965: `reads_residents` is not in the vocabulary, the whole
/// declaration is refused and `edges()` fails on the refusal.
#[test]
fn each_resident_gets_one_question_edge_stamped_with_the_members_round() {
    let got = edges(&params_for(
        "viewer",
        json!({"reads_residents": all_residents()}),
    ));
    for (name, cell, lane, _, _, _) in RESIDENTS {
        let q: Vec<&Value> = got
            .iter()
            .filter(|e| e["from"] == json!("./apps/viewer") && e["to"] == json!(cell))
            .collect();
        assert_eq!(q.len(), 1, "{name}: {got:#?}");
        let q = q[0];
        let cond = q["condition"].as_str().expect("a condition");
        assert!(
            cond.starts_with(
                "has(hop.route) && hop.route == 'resident_read' && !has(hop.error_code)"
            ),
            "{name}: a resident_read that carries an error code is an answer: {cond}"
        );
        assert!(
            cond.contains(&format!("hop.resident == '{name}'")),
            "{name}: the edge carries only the read naming this resident: {cond}"
        );
        assert!(cond.contains("has(hop.op_id) && hop.op_id != ''"), "{cond}");
        assert_eq!(
            q["modifier"]["set_hop"]["route"],
            json!(format!("'{lane}'")),
            "{name}"
        );
        let ctx = &q["modifier"]["set_context"];
        assert_eq!(ctx["resident_caller"], "'viewer'", "{name}");
        assert_eq!(ctx["resident_op"], "hop.op_id", "{name}");
        // Review C-1: the member's round in BOTH keys for every resident --
        // also for those that hold no round themselves, because their own
        // pulls carry the context on (the library's `symbol`/`related` reach
        // the graph space, which reads `audience_now`, else `audience_set`).
        for k in ["audience_now", "audience_set"] {
            assert_eq!(
                ctx[k], MEMBER_ROUND,
                "{name}: the member's round, written by the edge, in {k}"
            );
        }
        // Both reply-to marks on every question, blank unless the read is
        // theirs -- an app cannot forge one onto another resident's read.
        let (recall, brief) = match name {
            "memory-hive" => ("'resident'", "''"),
            "affinity" => ("''", "'resident'"),
            _ => ("''", "''"),
        };
        assert_eq!(ctx["recall_caller"], recall, "{name}");
        assert_eq!(ctx["brief_caller"], brief, "{name}");
        assert!(q.get("lane").is_none() && q.get("tap").is_none() && q.get("default").is_none());
    }
}

/// The way back: one edge per answering route, guarded on the asker, stamped
/// with the round of the answer -- the member's, or everybody's for the counts.
#[test]
fn each_answer_goes_back_to_the_asker_alone_with_its_round() {
    let got = edges(&params_for(
        "viewer",
        json!({"reads_residents": all_residents()}),
    ));
    let mut n = 0;
    for (name, cell, _, mark, answers, round) in RESIDENTS {
        let question = got
            .iter()
            .find(|e| e["from"] == json!("./apps/viewer") && e["to"] == json!(cell))
            .expect("the question edge");
        let mut stamped: Vec<String> = question["modifier"]["set_context"]
            .as_object()
            .expect("a context stamp")
            .keys()
            .cloned()
            .collect();
        if name == "objects" {
            // `object_brief` asks the memory over the member's `facts` edge,
            // which writes these into the asking context; they end with the
            // answer too (follow-up review 1, minor).
            stamped.extend(OBJECTS_INNER.iter().map(|k| k.to_string()));
        }
        stamped.sort();
        for route in answers {
            let back: Vec<&Value> = got
                .iter()
                .filter(|e| {
                    e["from"] == json!(cell)
                        && e["to"] == json!("./apps/viewer")
                        && e["condition"]
                            .as_str()
                            .is_some_and(|c| c.contains(&format!("hop.route == '{route}'")))
                })
                .collect();
            assert_eq!(back.len(), 1, "{name}/{route}: {got:#?}");
            let e = back[0];
            // Review I-1: the asker AND the mark of this very read.
            assert_eq!(
                e["condition"],
                json!(format!(
                    "has(hop.route) && hop.route == '{route}' && has(context.resident_caller) \
                     && context.resident_caller == 'viewer' && {mark}"
                ))
            );
            let hop = &e["modifier"]["set_hop"];
            assert_eq!(hop["route"], "'resident_answer'");
            assert_eq!(hop["resident"], json!(format!("'{name}'")));
            assert_eq!(hop["resident_status"], json!(format!("'{route}'")));
            assert_eq!(
                hop["op_id"], "has(context.resident_op) ? context.resident_op : ''",
                "the asker gets its own id back, not the prefixed one"
            );
            assert_eq!(hop["resident_round"], round, "{name}/{route}");
            // Review I-2: every key the question wrote ends where the answer
            // arrives -- the round, the channel, the recall keys, the marks.
            assert_eq!(
                e["modifier"]["delete_context"],
                json!(stamped),
                "{name}/{route}: the stamp ends where the answer arrives"
            );
            n += 1;
        }
    }
    assert_eq!(
        got.len(),
        RESIDENTS.len() + n,
        "one question and its ways back each: {got:#?}"
    );
}

/// A round the app writes on its own message never reaches a resident: the
/// edge writes the round key itself, so the app's value is overwritten (the
/// edge runs after every edge of the app). A memory read also cannot pick its
/// channel, its as-of instant or its window -- only its query and tier.
#[test]
fn the_app_cannot_choose_the_round_it_reads_in() {
    let got = edges(&params_for(
        "viewer",
        json!({"reads_residents": ["memory-hive"]}),
    ));
    let q = got
        .iter()
        .find(|e| e["to"] == json!("./memory-hive"))
        .expect("the memory edge");
    let ctx = &q["modifier"]["set_context"];
    assert_eq!(ctx["audience_now"], MEMBER_ROUND);
    assert_eq!(ctx["channel"], "'display'", "the screen is the channel");
    for k in ["recall_as_of", "recall_window_from", "recall_window_to"] {
        assert_eq!(ctx[k], "''", "{k}");
    }
    assert_eq!(
        ctx["recall_query"],
        "has(hop.recall_query) ? hop.recall_query : ''"
    );
    assert_eq!(
        ctx["memory_tier"],
        "has(hop.memory_tier) ? hop.memory_tier : ''"
    );
    assert_eq!(ctx["recall_caller"], "'resident'");
    // Two apps never hear each other's answers.
    let other = edges(&params_for(
        "other",
        json!({"reads_residents": ["memory-hive"]}),
    ));
    for e in &other {
        assert!(!e.to_string().contains("viewer"), "{e}");
    }
}

/// The residents that mirror `op_id` get it prefixed `res:` -- `gs:` and `lib:`
/// are the graph's and the library's own pulls, and a member edge routes an
/// answer on those prefixes -- and an emptied caller (file-space lets an answer
/// out only with an empty one).
#[test]
fn a_mirrored_id_cannot_steer_an_answer_into_a_residents_own_pull() {
    let got = edges(&params_for(
        "viewer",
        json!({"reads_residents": all_residents()}),
    ));
    for cell in [
        "./file-space",
        "./graph-space",
        "./librarian",
        "./apps/colony-view",
    ] {
        let q = got
            .iter()
            .find(|e| e["from"] == json!("./apps/viewer") && e["to"] == json!(cell))
            .expect("a question edge");
        assert_eq!(
            q["modifier"]["set_hop"]["op_id"], "'res:' + hop.op_id",
            "{cell}"
        );
        assert_eq!(q["modifier"]["set_hop"]["caller"], "''", "{cell}");
        assert!(
            q["condition"].as_str().unwrap().contains("has(hop.op)"),
            "{cell}"
        );
    }
}

/// The object hive is read through its READ tools only: `object_set` and
/// `object_confirm` are a model's writes, never a screen's, and the read
/// carries no speaker (the owner-only answers stay closed).
#[test]
fn the_objects_are_read_through_their_read_tools_only() {
    let got = edges(&params_for(
        "viewer",
        json!({"reads_residents": ["objects"]}),
    ));
    let q = got
        .iter()
        .find(|e| e["to"] == json!("./objects"))
        .expect("the objects edge");
    let cond = q["condition"].as_str().unwrap();
    assert!(
        cond.ends_with("&& has(hop.op) && (hop.op == 'object_find' || hop.op == 'object_brief')"),
        "{cond}"
    );
    assert_eq!(q["modifier"]["set_hop"]["tool_name"], "hop.op");
    assert_eq!(
        q["modifier"]["set_hop"]["tool_call_id"],
        "'res:' + hop.op_id"
    );
    assert_eq!(q["modifier"]["set_context"]["speaker"], "''");
}

#[test]
fn affinity_is_asked_as_the_generation_in_the_members_round() {
    let got = edges(&params_for(
        "viewer",
        json!({"reads_residents": ["affinity"]}),
    ));
    let q = got
        .iter()
        .find(|e| e["to"] == json!("./affinity"))
        .expect("the affinity edge");
    let ctx = &q["modifier"]["set_context"];
    assert_eq!(ctx["asker"], "'agent:sam'");
    assert_eq!(ctx["audience_set"], MEMBER_ROUND);
    assert_eq!(ctx["brief_caller"], "'resident'");
}

#[test]
fn an_unknown_empty_or_repeated_resident_is_refused_by_field() {
    let p = refusal(&params_for(
        "viewer",
        json!({"reads_residents": ["memory-hive", "diary"]}),
    ));
    assert_eq!(p["field"], "reads_residents[1]", "{p}");
    assert!(
        p["known"]
            .as_array()
            .unwrap()
            .contains(&json!("colony-view")),
        "{p}"
    );
    for bad in [
        json!([]),
        json!("memory-hive"),
        json!(["objects", "objects"]),
    ] {
        let p = refusal(&params_for("viewer", json!({"reads_residents": bad})));
        assert_eq!(p["field"], "reads_residents", "{p}");
    }
}

/// A read in the member's round needs the member's person: without it the
/// wish is asked, at the renderer and at the switch, exactly as for `reads`.
#[test]
fn a_read_without_a_person_or_a_generation_is_asked() {
    let mut p = params_for("viewer", json!({"reads_residents": ["file-space"]}));
    p.as_object_mut().unwrap().remove("ctx");
    let all = run_recipes(&p);
    assert_eq!(
        all[0]["header"]["error_code"], "wish_incomplete",
        "{:#?}",
        all[0]
    );

    let mut p = params_for("viewer", json!({"reads_residents": ["file-space"]}));
    p.as_object_mut().unwrap().remove("generation");
    let out = classify(p);
    let text = out.to_string();
    assert!(
        text.contains("generation"),
        "the switch asks for the generation: {out}"
    );
}

/// The member's own edges on those answer routes leave a resident read alone:
/// the memory's `reject` to the assistants, the objects' `tool_result` to the
/// assistants and affinity's `answer`/`error` out of the member would each
/// carry the answer to a second reader.
#[test]
fn the_members_catch_all_edges_leave_a_resident_read_alone() {
    let cfg: Value =
        serde_json::from_str(&std::fs::read_to_string(MEMBER_CONFIG).expect("member config"))
            .expect("json");
    let edges = cfg["params"]["graph"]["edges"].as_array().expect("edges");
    let find = |from: &str, to: &str, frag: &str| -> String {
        let hits: Vec<&Value> = edges
            .iter()
            .filter(|e| {
                e["from"] == json!(from)
                    && e["to"] == json!(to)
                    && e["condition"].as_str().is_some_and(|c| c.contains(frag))
            })
            .collect();
        assert_eq!(hits.len(), 1, "{from} -> {to} [{frag}]");
        hits[0]["condition"].as_str().unwrap().to_string()
    };
    let c = find("./memory-hive", "./assistants", "hop.route == 'reject'");
    assert!(c.ends_with("&& hop.recall_caller != 'resident'"), "{c}");
    let c = find("./objects", "./assistants", "hop.route == 'tool_result'");
    assert!(
        c.ends_with("&& (!has(context.resident_caller) || context.resident_caller == '')"),
        "{c}"
    );
    for frag in [
        "hop.route == 'answer' && has(hop.subscriber)",
        "hop.route == 'error'",
    ] {
        let c = find("./affinity", ".", frag);
        assert!(
            c.ends_with("&& (!has(context.brief_caller) || context.brief_caller != 'resident')"),
            "{c}"
        );
    }
}

// ═══════════════════════════════════ the edges as the colony runs them (CEL)

/// The member's round as the context holds it once the edge has run.
const MEMBER_ROUND_VALUE: &str = r#"["agent:sam","member:alex"]"#;
/// A round the app writes on its own message: the generation alone -- a
/// SMALLER round than the member's, so it covers MORE rows (a row is visible
/// when its audience covers the round).
const APP_ROUND: &str = r#"["agent:sam"]"#;

fn obj(v: &Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

/// The edges among `edges` from `from` to `to` whose condition holds for the
/// header -- exactly what the colony's router evaluates.
fn holding(edges: &[Value], from: &str, to: &str, context: &Value, hop: &Value) -> Vec<Value> {
    let (c, h) = (obj(context), obj(hop));
    edges
        .iter()
        .filter(|e| {
            e["from"] == json!(from)
                && e["to"] == json!(to)
                && e["condition"].as_str().is_some_and(|x| {
                    let cond = parse_condition(x).unwrap_or_else(|err| panic!("{x}: {err}"));
                    matches!(evaluate_condition(&cond, &c, &h), Ok(true))
                })
        })
        .cloned()
        .collect()
}

/// The header after `edge`'s modifier, as `(context, hop)`.
fn cross(edge: &Value, context: &Value, hop: &Value) -> (Value, Value) {
    let spec: ModifierSpec =
        serde_json::from_value(edge["modifier"].clone()).expect("a modifier spec");
    let m = parse_modifier(&spec).expect("a modifier");
    let h = Headers::from_parts(obj(context), obj(hop));
    let out = apply_modifier(&m, &h).expect("the modifier applies");
    (Value::Object(out.context), Value::Object(out.hop))
}

fn member_edges() -> Vec<Value> {
    let cfg: Value =
        serde_json::from_str(&std::fs::read_to_string(MEMBER_CONFIG).expect("member config"))
            .expect("json");
    cfg["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .expect("edges")
}

/// What the app's own message carries: its own round in both keys, and both
/// reply-to marks forged -- everything a self-reporting app could try.
fn forged_context() -> Value {
    json!({"audience_now": APP_ROUND, "audience_set": APP_ROUND,
           "recall_caller": "resident", "brief_caller": "resident"})
}

/// The app's `resident_read` of `resident`, crossed over the one question edge
/// that holds for it: `(context, hop)` as the resident receives it.
fn ask(edges: &[Value], resident: &str, op: &str) -> (Value, Value) {
    let cell = RESIDENTS
        .iter()
        .find(|r| r.0 == resident)
        .expect("a resident")
        .1;
    let hop = json!({"route": "resident_read", "resident": resident, "op": op,
                     "op_id": "q1", "recall_query": "", "memory_tier": ""});
    let ctx = forged_context();
    let q = holding(edges, "./apps/viewer", cell, &ctx, &hop);
    assert_eq!(q.len(), 1, "{resident}: one question edge holds: {q:#?}");
    cross(&q[0], &ctx, &hop)
}

/// Review C-1 of GH #965, run as the colony runs it: an app writes its own,
/// smaller round on its message; every resident -- and every pull a resident
/// makes on its behalf, here the library's `symbol`/`related` question to the
/// graph space over the member's own edge -- receives the MEMBER's round in
/// both keys the graph space reads (`audience_now`, else `audience_set`).
/// Red before the fix: the library's question kept the app's round, and so
/// did the graph's.
#[test]
fn an_apps_own_round_reaches_no_resident_and_no_pull_of_one() {
    let edges = edges(&params_for(
        "viewer",
        json!({"reads_residents": all_residents()}),
    ));
    for (name, ..) in RESIDENTS {
        let op = if name == "objects" {
            "object_find"
        } else {
            "find"
        };
        let (ctx, _) = ask(&edges, name, op);
        for k in ["audience_now", "audience_set"] {
            assert_eq!(
                ctx[k], MEMBER_ROUND_VALUE,
                "{name}: the app's round {APP_ROUND} never reaches it ({k}): {ctx}"
            );
        }
    }
    let member = member_edges();
    for op in ["symbol", "related"] {
        let (ctx, _) = ask(&edges, "librarian", op);
        let pull = json!({"route": "pull", "op": "resolve", "op_id": "lib:g:t1"});
        let to_graph = holding(&member, "./librarian", "./graph-space", &ctx, &pull);
        assert_eq!(
            to_graph.len(),
            1,
            "the library's graph question: {to_graph:#?}"
        );
        let (at_graph, hop) = cross(&to_graph[0], &ctx, &pull);
        assert_eq!(hop["route"], "in_graph");
        for k in ["audience_now", "audience_set"] {
            assert_eq!(
                at_graph[k], MEMBER_ROUND_VALUE,
                "`{op}`: the graph space reads the member's round, never the app's ({k})"
            );
        }
    }
}

/// Review I-1 of GH #965: the asking context rides on through a resident's
/// OWN pulls, so their answers carry the asker too. None of them reaches the
/// app -- only the answer of the read itself does, once:
/// - the graph space's `gs:` pull answered by the file space,
/// - the library's `lib:g:` / `lib:f:` pulls answered by the two spaces,
/// - the object hive's memory question (`recall_caller 'objects'`),
/// - an affinity answer inside an object read, although the app forged
///   `brief_caller 'resident'` on its own message.
///
/// Red before the fix: every one of them left as a `resident_answer`.
#[test]
fn an_inner_pull_of_a_resident_never_reaches_the_app() {
    let edges = edges(&params_for(
        "viewer",
        json!({"reads_residents": all_residents()}),
    ));
    let none = |from: &str, ctx: &Value, hop: &Value, what: &str| {
        let hits = holding(&edges, from, "./apps/viewer", ctx, hop);
        assert!(hits.is_empty(), "{what} reached the app: {hits:#?}");
    };
    let one = |from: &str, ctx: &Value, hop: &Value, what: &str| -> (Value, Value) {
        let hits = holding(&edges, from, "./apps/viewer", ctx, hop);
        assert_eq!(hits.len(), 1, "{what}: {hits:#?}");
        cross(&hits[0], ctx, hop)
    };

    // The graph space reads; its pull of a file comes back from the files.
    let (g, gq) = ask(&edges, "graph-space", "near");
    assert_eq!(gq["op_id"], "res:q1");
    none(
        "./file-space",
        &g,
        &json!({"route": "answer", "op": "read", "op_id": "gs:p1"}),
        "the files' answer to the graph's own pull",
    );
    one(
        "./graph-space",
        &g,
        &json!({"route": "answer", "op": "near", "op_id": "res:q1"}),
        "the graph's answer to the read",
    );

    // The library reads; it asks both spaces on its own.
    let (l, _) = ask(&edges, "librarian", "symbol");
    none(
        "./graph-space",
        &l,
        &json!({"route": "answer", "op": "resolve", "op_id": "lib:g:t1"}),
        "the graph's answer to the library's pull",
    );
    none(
        "./file-space",
        &l,
        &json!({"route": "answer", "op": "list", "op_id": "lib:f:t:t2"}),
        "the files' answer to the library's pull",
    );
    one(
        "./librarian",
        &l,
        &json!({"route": "answer", "op": "symbol", "op_id": "res:q1"}),
        "the library's answer to the read",
    );

    // The object hive reads; `object_brief` asks the memory on its own, over
    // the member's edge, which names itself the reply-to.
    let (o, oq) = ask(&edges, "objects", "object_brief");
    assert_eq!(oq["tool_call_id"], "res:q1");
    let facts = json!({"route": "facts", "audience_now": MEMBER_ROUND_VALUE});
    let member = member_edges();
    let to_memory = holding(&member, "./objects", "./memory-hive", &o, &facts);
    assert_eq!(to_memory.len(), 1, "{to_memory:#?}");
    let (om, _) = cross(&to_memory[0], &o, &facts);
    assert_eq!(om["recall_caller"], "objects");
    none(
        "./memory-hive",
        &om,
        &json!({"route": "bundle", "recall_caller": "objects"}),
        "the memory's answer to the object hive's own question",
    );
    none(
        "./affinity",
        &o,
        &json!({"route": "answer"}),
        "an affinity answer inside an object read (the app forged the mark)",
    );
    one(
        "./objects",
        &o,
        &json!({"route": "tool_result", "tool_call_id": "res:q1"}),
        "the object hive's answer to the read",
    );

    // The memory and affinity read: their own answers, once.
    let (m, _) = ask(&edges, "memory-hive", "recall");
    none(
        "./memory-hive",
        &m,
        &json!({"route": "reject"}),
        "an inner reject that names no reply-to",
    );
    one(
        "./memory-hive",
        &m,
        &json!({"route": "bundle", "recall_caller": "resident"}),
        "the memory's answer to the read",
    );
    let (a, _) = ask(&edges, "affinity", "brief");
    one(
        "./affinity",
        &a,
        &json!({"route": "answer"}),
        "affinity's answer to the read",
    );
}

/// Review I-2 of GH #965: once the answer is at the app, nothing the question
/// stamped is left in its context -- not the round, the channel, the recall
/// keys, the speaker, the asker or the marks. What the app carried before
/// the read and the stamp did not touch stays.
#[test]
fn the_answer_leaves_no_stamp_in_the_apps_context() {
    let edges = edges(&params_for(
        "viewer",
        json!({"reads_residents": all_residents()}),
    ));
    let answers = [
        (
            "memory-hive",
            json!({"route": "bundle", "recall_caller": "resident"}),
        ),
        ("file-space", json!({"route": "answer", "op_id": "res:q1"})),
        ("graph-space", json!({"route": "answer", "op_id": "res:q1"})),
        (
            "objects",
            json!({"route": "tool_result", "tool_call_id": "res:q1"}),
        ),
        ("librarian", json!({"route": "answer", "op_id": "res:q1"})),
        ("affinity", json!({"route": "answer"})),
        ("colony-view", json!({"route": "answer", "op_id": "res:q1"})),
    ];
    // `object_brief`: the object hive's own memory question (the member's
    // `facts` edge) wrote its keys into the context before the answer.
    let member = member_edges();
    let (o, _) = ask(&edges, "objects", "object_brief");
    let facts = json!({"route": "facts", "audience_now": MEMBER_ROUND_VALUE, "subject": "ob-1"});
    let to_memory = holding(&member, "./objects", "./memory-hive", &o, &facts);
    assert_eq!(to_memory.len(), 1, "{to_memory:#?}");
    let (mut o, _) = cross(&to_memory[0], &o, &facts);
    for k in OBJECTS_INNER {
        assert!(o.get(k).is_some(), "the facts edge writes {k}: {o}");
    }
    o["turn_id"] = json!("t-1");
    let hop = json!({"route": "tool_result", "tool_call_id": "res:q1"});
    let back = holding(&edges, "./objects", "./apps/viewer", &o, &hop);
    assert_eq!(back.len(), 1, "{back:#?}");
    let (after, _) = cross(&back[0], &o, &hop);
    let after_map = obj(&after);
    assert_eq!(
        after_map.keys().map(String::as_str).collect::<Vec<_>>(),
        vec!["turn_id"],
        "object_brief: neither the stamp nor the inner facts keys stay: {after}"
    );

    for (name, hop) in answers {
        let cell = RESIDENTS
            .iter()
            .find(|r| r.0 == name)
            .expect("a resident")
            .1;
        let op = if name == "objects" {
            "object_find"
        } else {
            "find"
        };
        let (mut ctx, _) = ask(&edges, name, op);
        ctx["turn_id"] = json!("t-1");
        let back = holding(&edges, cell, "./apps/viewer", &ctx, &hop);
        assert_eq!(back.len(), 1, "{name}: {back:#?}");
        let (after, hop) = cross(&back[0], &ctx, &hop);
        assert_eq!(hop["route"], "resident_answer");
        assert_eq!(hop["op_id"], "q1", "{name}");
        let after_map = obj(&after);
        let left: Vec<&str> = after_map.keys().map(String::as_str).collect();
        assert_eq!(
            left,
            vec!["turn_id"],
            "{name}: the stamp of the read is gone, the app's own key stays: {after}"
        );
    }
}

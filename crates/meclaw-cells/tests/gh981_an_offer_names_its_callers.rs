//! GH #981 -- two more words an app declaration may say.
//!
//! 1. `offers[].callers`: the brains of the generation a tool offer is drawn
//!    from. Without it an offer is offered to the two surfaces, exactly as
//!    before -- every declaration written before the word renders the same
//!    edges (the live fixtures of `gh599`/`gh916` stay byte for byte). With it,
//!    the core (`cogny`) can be handed tools of its own: one `tool` v-lane per
//!    caller guarded on the names, and ONE menu tick per (caller, cell). Which
//!    names a caller's menu shows is the app's to decide by
//!    `context.tool_caller`; the recipe filters no schemas. A tool name reaches
//!    one cell per caller.
//! 2. `runs: {at, brain}`: an app hands one brain of its generation a RUN --
//!    one turn whose tool results and end the app hears. One door edge from the
//!    app's rim onto the brain's rim (`run_turn` -> `in_turn`, the routing
//!    budget restored, no `lane`), one TAP per road
//!    a tool result takes into the generation, and one v-lane `run_answer` off
//!    the brain's rim. Everything a run is keyed on (`run_id`, `run_app`, the
//!    session, the round, the speaker) is written by the edge, never read off
//!    the app's context; and it is keyed on `run_id`, never on
//!    `consult_class`, which the first tool call deletes.
//!
//! Pure script tests: the SHIPPED `classify` and `recipes` are run over stdin,
//! nothing is booted. Same technique as `gh949_an_app_declares_reads.rs`.

use meclaw_core::serde_json::{self, Value, json};
use meclaw_testing::{emit_all, emit_one, shipped_script};
use std::collections::BTreeSet;

const RECIPES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/recipes/config.json"
);
const CLASSIFY: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/classify/config.json"
);
const ASSISTANT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/assistant/config.json"
);

const MEMBER: &str = "/os/orgs/acme/members/alex";
const MEMBER_ROUND: &str = r#"'["agent:sam","member:alex"]'"#;
const COGNY: &str = "./assistants/sam/cogny";
const TALKY: &str = "./assistants/sam/talky";
const TALKY_CHAT: &str = "./assistants/sam/talky-chat";
/// Where a tool result enters the generation from the member, plus the
/// generation's own tool hive.
const HOLDERS: [&str; 7] = [
    "./memory-hive",
    "./file-space",
    "./objects",
    "./librarian",
    "./apps",
    "./channels",
    "./assistants/sam/tools",
];

fn shipped() -> bool {
    [RECIPES, CLASSIFY, ASSISTANT]
        .iter()
        .all(|p| std::path::Path::new(p).is_file())
}

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

fn params(declaration: Value) -> Value {
    json!({"scope": MEMBER, "app": "probe-app", "template": "probe-app@1.0.0",
           "screen": "display", "generation": "sam", "ctx": {"member_person": "alex"},
           "declaration": declaration})
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

fn from<'a>(got: &'a [Value], cell: &str) -> Vec<&'a Value> {
    got.iter().filter(|e| e["from"] == json!(cell)).collect()
}

fn with_lane<'a>(got: &[&'a Value], lane: &str) -> Vec<&'a Value> {
    got.iter()
        .copied()
        .filter(|e| e["lane"] == json!(lane))
        .collect()
}

fn as_set(got: &[Value]) -> BTreeSet<String> {
    got.iter().map(Value::to_string).collect()
}

const RUNS: &str = r#"{"at": "./inbox", "brain": "cogny"}"#;

fn runs() -> Value {
    serde_json::from_str(RUNS).expect("runs")
}

/// An offer without `callers` and the same offer naming the two surfaces
/// render the same edges: the default is the old behaviour, word for word.
#[test]
fn an_offer_without_callers_draws_what_it_drew() {
    if !shipped() {
        return;
    }
    let plain = edges(&params(json!({"offers": [
        {"kind": "tool", "at": "./tools", "tools": ["probe_find", "probe_note"]},
        {"kind": "sidecar", "at": "./show", "section": "probe"}]})));
    let named = edges(&params(json!({"offers": [
        {"kind": "tool", "at": "./tools", "tools": ["probe_find", "probe_note"],
         "callers": ["talky", "talky-chat"]},
        {"kind": "sidecar", "at": "./show", "section": "probe"}]})));
    assert_eq!(
        plain, named,
        "the default callers are the two surfaces, in order"
    );
    assert!(from(&plain, COGNY).is_empty(), "{plain:#?}");
}

/// Red before GH #981: `callers` is no offer key (`app_declaration_invalid`).
/// The core gets exactly the names offered to it, and one menu tick for the
/// cell; the surfaces keep theirs and never see the core's names.
#[test]
fn an_offer_names_its_callers() {
    if !shipped() {
        return;
    }
    let got = edges(&params(json!({"offers": [
        {"kind": "tool", "at": "./tools", "tools": ["probe_find", "probe_note"]},
        {"kind": "tool", "at": "./tools", "tools": ["probe_find", "probe_check"],
         "callers": ["cogny"]}]})));
    let core = from(&got, COGNY);
    let calls = with_lane(&core, "tool");
    assert_eq!(calls.len(), 1, "{core:#?}");
    assert_eq!(calls[0]["to"], "./apps/probe-app/tools");
    assert_eq!(
        calls[0]["condition"],
        "has(hop.route) && hop.route == 'tool' && has(hop.tool_name) && \
         (hop.tool_name == 'probe_find' || hop.tool_name == 'probe_check')"
    );
    assert_eq!(
        calls[0]["modifier"]["set_context"],
        json!({"tool_caller": "'cogny'", "assistant": "'sam'", "offer_tool": "hop.tool_name"}),
        "stamped like a surface's call"
    );
    let ticks = with_lane(&core, "schemas");
    assert_eq!(ticks.len(), 1, "one tick per (caller, cell): {core:#?}");
    assert_eq!(ticks[0]["to"], "./apps/probe-app/tools");
    assert_eq!(core.len(), 2, "{core:#?}");
    for surface in [TALKY, TALKY_CHAT] {
        let mine = from(&got, surface);
        let calls = with_lane(&mine, "tool");
        assert_eq!(calls.len(), 1, "{mine:#?}");
        let cond = calls[0]["condition"].as_str().expect("a condition");
        assert!(
            cond.contains("'probe_note'") && !cond.contains("probe_check"),
            "{surface} sees its own names only: {cond}"
        );
        assert_eq!(with_lane(&mine, "schemas").len(), 1, "{mine:#?}");
    }
    // The result exit lets out every offered name, once.
    let exit: Vec<&Value> = got
        .iter()
        .filter(|e| {
            e["from"] == json!("./apps/probe-app")
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains("tool_result"))
        })
        .collect();
    assert_eq!(exit.len(), 1, "{got:#?}");
    for t in ["probe_find", "probe_note", "probe_check"] {
        assert!(
            exit[0]["condition"]
                .as_str()
                .is_some_and(|c| c.contains(&format!("'{t}'"))),
            "{t}: {:#?}",
            exit[0]
        );
    }
}

/// A cell only the core is offered tools at gets no surface tick: a surface
/// would ask a menu that has nothing for it.
#[test]
fn a_cell_offered_to_the_core_alone_is_not_ticked_by_a_surface() {
    if !shipped() {
        return;
    }
    let got = edges(&params(json!({"offers": [
        {"kind": "tool", "at": "./tools", "tools": ["probe_find"]},
        {"kind": "tool", "at": "./core", "tools": ["probe_run"], "callers": ["cogny"]}]})));
    for surface in [TALKY, TALKY_CHAT] {
        let ticks: Vec<&Value> = with_lane(&from(&got, surface), "schemas");
        assert_eq!(ticks.len(), 1, "{ticks:#?}");
        assert_eq!(ticks[0]["to"], "./apps/probe-app/tools");
    }
    let core: Vec<&Value> = with_lane(&from(&got, COGNY), "schemas");
    assert_eq!(core.len(), 1, "{core:#?}");
    assert_eq!(core[0]["to"], "./apps/probe-app/core");
}

/// One tool name, one cell, per caller: two offers handing the same caller the
/// same name would draw two v-lanes for one call. Disjoint callers may share a
/// name.
#[test]
fn a_tool_name_reaches_one_cell_per_caller() {
    if !shipped() {
        return;
    }
    let p = refusal(&params(json!({"offers": [
        {"kind": "tool", "at": "./tools", "tools": ["probe_find"]},
        {"kind": "tool", "at": "./other", "tools": ["probe_find"], "callers": ["talky"]}]})));
    assert_eq!(p["field"], "offers[1].tools", "{p}");
    let got = edges(&params(json!({"offers": [
        {"kind": "tool", "at": "./tools", "tools": ["probe_find"]},
        {"kind": "tool", "at": "./other", "tools": ["probe_find"], "callers": ["cogny"]}]})));
    assert_eq!(with_lane(&from(&got, COGNY), "tool").len(), 1, "{got:#?}");
}

#[test]
fn callers_name_brains_of_the_generation() {
    if !shipped() {
        return;
    }
    for bad in [
        json!([]),
        json!(["bob"]),
        json!(["cogny", "cogny"]),
        json!("cogny"),
        json!([3]),
    ] {
        let p = refusal(&params(json!({"offers": [
            {"kind": "tool", "at": "./tools", "tools": ["probe_find"], "callers": bad.clone()}]})));
        assert_eq!(p["field"], "offers[0].callers", "{bad}: {p}");
        assert_eq!(
            p["known"],
            json!(["talky", "talky-chat", "cogny"]),
            "{bad}: {p}"
        );
    }
}

/// The plan's hook (PE "prefix roads"): a name the core is offered by an app
/// must not also match one of the core's own prefix roads in the assistant
/// level (`file_`, `lib_`, `object_`, `memory_recall`, ...), or one call would
/// have two targets. The app prefixes its names; this pins that the prefixes
/// the level routes on are the ones an app must stay out of.
#[test]
fn an_app_name_of_the_core_has_exactly_one_target() {
    if !shipped() {
        return;
    }
    let names = ["probe_find", "probe_note", "probe_check"];
    let assistant: Value =
        serde_json::from_str(&std::fs::read_to_string(ASSISTANT).expect("assistant"))
            .expect("json");
    let mut prefixes = Vec::new();
    let mut exact = Vec::new();
    for e in assistant["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
    {
        if e["from"] != json!("./cogny") {
            continue;
        }
        let c = e["condition"].as_str().unwrap_or_default();
        if !c.contains("hop.route == 'tool'") {
            continue;
        }
        for (marker, out) in [
            ("hop.tool_name.startsWith('", &mut prefixes),
            ("hop.tool_name == '", &mut exact),
        ] {
            let mut rest = c;
            while let Some(i) = rest.find(marker) {
                rest = &rest[i + marker.len()..];
                let end = rest.find('\'').expect("a closing quote");
                out.push(rest[..end].to_string());
                rest = &rest[end..];
            }
        }
    }
    assert!(
        prefixes.contains(&"file_".to_string()),
        "the scan finds the prefix roads: {prefixes:?}"
    );
    for n in names {
        assert!(
            !prefixes.iter().any(|p| n.starts_with(p.as_str())) && !exact.iter().any(|x| x == n),
            "{n} would take a road of the level as well: {prefixes:?} {exact:?}"
        );
    }
    let got = edges(&params(json!({"offers": [
        {"kind": "tool", "at": "./tools", "tools": names, "callers": ["cogny"]}]})));
    for n in names {
        let targets: Vec<&Value> = with_lane(&from(&got, COGNY), "tool")
            .into_iter()
            .filter(|e| {
                e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains(&format!("'{n}'")))
            })
            .collect();
        assert_eq!(targets.len(), 1, "{n}: {got:#?}");
    }
}

/// Red before GH #981: `runs` is not in the vocabulary. One door, one tap per
/// road a tool result takes into the generation, one end.
#[test]
fn runs_draws_the_door_the_taps_and_the_end() {
    if !shipped() {
        return;
    }
    let got = edges(&params(json!({"runs": runs()})));
    assert_eq!(got.len(), 1 + HOLDERS.len() + 1, "{got:#?}");

    let door = from(&got, "./apps/probe-app");
    assert_eq!(door.len(), 1, "{got:#?}");
    let d = door[0];
    // Straight onto the brain's rim: the assistant level draws no edge for a
    // run and declares no lane for it (an entry lane there must be one an
    // occupant takes, GH #302). No `lane`: the app's end sits in the member's
    // app container, which declares no contract a v-lane could dock at.
    assert_eq!(d["to"], COGNY);
    assert!(d.get("lane").is_none() && d.get("tap").is_none(), "{d:#?}");
    assert_eq!(
        d["modifier"]["restore_ttl"], true,
        "a door: one model call on the core's side, like the consult"
    );
    assert_eq!(
        d["condition"],
        "has(hop.route) && hop.route == 'run_turn' && has(hop.run_id) && \
         hop.run_id.matches('^[a-z0-9-]{1,40}$') && \
         (!has(context.run_chain) || int(context.run_chain) < 16)",
        "the door restores the budget, so it carries its own bound (GH #82): \
         a causal chain starts at most 16 runs"
    );
    assert_eq!(d["modifier"]["set_hop"], json!({"route": "'in_turn'"}));
    let ctx = &d["modifier"]["set_context"];
    assert_eq!(ctx["run_id"], "hop.run_id");
    assert_eq!(
        ctx["run_app"], "'probe-app'",
        "the app is the builder's literal"
    );
    assert_eq!(
        ctx["session_id"], "'run:probe-app:' + hop.run_id",
        "one session per run, in the app's namespace: another app naming the \
         same id never continues this one"
    );
    assert_eq!(
        ctx["run_chain"], "has(context.run_chain) ? int(context.run_chain) + 1 : 1",
        "the counter the bound reads, stamped by the door alone"
    );
    assert_eq!(ctx["consult_class"], "'run'");
    assert_eq!(ctx["assistant"], "'sam'");
    assert_eq!(ctx["audience_set"], MEMBER_ROUND, "the member's round");
    assert_eq!(
        ctx["audience_now"], MEMBER_ROUND,
        "the round a resident reads first (the graph space), over whatever the \
         app wrote (review C-1 of GH #965)"
    );
    assert_eq!(ctx["turn_round"], MEMBER_ROUND);
    assert_eq!(ctx["speaker"], "''", "a run is said by nobody (GH #979)");
    assert_eq!(ctx["channel_node"], "''");

    for holder in HOLDERS {
        let taps = from(&got, holder);
        assert_eq!(taps.len(), 1, "{holder}: {got:#?}");
        let t = taps[0];
        assert_eq!(t["to"], "./apps/probe-app/inbox");
        assert_eq!(t["lane"], "run_tool_result");
        assert_eq!(t["tap"], true, "a tap takes nothing from the brain");
        assert_eq!(
            t["condition"],
            "has(hop.route) && hop.route == 'tool_result' && has(context.run_app) && \
             context.run_app == 'probe-app' && has(context.run_id) && context.run_id != '' \
             && has(context.tool_caller) && context.tool_caller == 'cogny'"
        );
        let hop = &t["modifier"]["set_hop"];
        assert_eq!(hop["route"], "'run_tool_result'");
        assert_eq!(hop["run_id"], "context.run_id");
        assert!(
            hop["answerer"].is_string() && hop["ok"].is_string(),
            "{t:#?}"
        );
    }

    let end = with_lane(&from(&got, COGNY), "run_answer");
    assert_eq!(end.len(), 1, "{got:#?}");
    let e = end[0];
    assert_eq!(e["to"], "./apps/probe-app/inbox");
    assert!(e.get("tap").is_none(), "{e:#?}");
    assert_eq!(
        e["condition"],
        "has(hop.route) && (hop.route == 'answer' || hop.route == 'error' || \
         hop.route == 'model_refused') && has(context.run_app) && \
         context.run_app == 'probe-app' && has(context.run_id) && context.run_id != ''"
    );
    let hop = &e["modifier"]["set_hop"];
    assert_eq!(hop["route"], "'run_answer'");
    assert_eq!(
        hop["capped"],
        "has(hop.round_capped) && hop.round_capped == '1'"
    );
    assert!(hop["error"].is_string(), "{e:#?}");
}

/// OR-LP-24: a run is keyed on `run_id`, never on `consult_class` -- the first
/// tool call deletes the label (`_call_stamp`), and the recall edge of the
/// core does too.
#[test]
fn a_run_is_keyed_on_its_id_never_on_its_label() {
    if !shipped() {
        return;
    }
    let got = edges(&params(json!({"runs": runs(),
        "offers": [{"kind": "tool", "at": "./tools", "tools": ["probe_find"],
                    "callers": ["cogny"]}]})));
    for e in &got {
        let c = e["condition"].as_str().unwrap_or_default();
        assert!(!c.contains("consult_class"), "{e:#?}");
        let dels: Vec<&str> = e["modifier"]["delete_context"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        assert!(
            !dels.iter().any(|d| d.starts_with("run_")),
            "no edge of the installation deletes a run key: {e:#?}"
        );
    }
}

/// The holders the member does not have get no tap: an edge off a node that
/// does not stand would take the whole installation with it.
#[test]
fn residents_present_bounds_the_holder_taps() {
    if !shipped() {
        return;
    }
    let mut p = params(json!({"runs": runs()}));
    p["residents_present"] = json!(["memory-hive"]);
    let got = edges(&p);
    let froms: BTreeSet<String> = got
        .iter()
        .filter(|e| e["lane"] == json!("run_tool_result"))
        .map(|e| e["from"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        froms,
        BTreeSet::from(
            [
                "./memory-hive",
                "./apps",
                "./channels",
                "./assistants/sam/tools"
            ]
            .map(str::to_string)
        )
    );
}

#[test]
fn runs_is_closed() {
    if !shipped() {
        return;
    }
    for (bad, field) in [
        (json!({"at": "./inbox", "brain": "talky"}), "runs.brain"),
        (
            json!({"at": "./inbox", "brain": "talky-chat"}),
            "runs.brain",
        ),
        (json!({"at": "./inbox"}), "runs.brain"),
        (json!({"brain": "cogny"}), "runs.at"),
        (json!({"at": "inbox", "brain": "cogny"}), "runs.at"),
        (
            json!({"at": "./inbox", "brain": "cogny", "busy": 1}),
            "runs",
        ),
        (json!("./inbox"), "runs"),
    ] {
        let p = refusal(&params(json!({"runs": bad.clone()})));
        assert_eq!(p["field"], field, "{bad}: {p}");
    }
}

#[test]
fn the_vocabulary_knows_runs_and_the_switch_asks_for_what_it_needs() {
    if !shipped() {
        return;
    }
    let p = refusal(&params(json!({"runz": runs()})));
    assert!(
        p["known"]
            .as_array()
            .expect("known")
            .contains(&json!("runs")),
        "{p}"
    );
    let mut wish = params(json!({}));
    wish["declaration"] = json!("runs: ./inbox");
    let out = classify(wish);
    assert!(
        payload(&out)["known"]
            .as_array()
            .expect("known")
            .contains(&json!("runs")),
        "{out}"
    );

    let mut no_gen = params(json!({"runs": runs()}));
    no_gen.as_object_mut().expect("params").remove("generation");
    let out = classify(no_gen);
    assert_eq!(
        out["header"]["error_code"],
        json!("recipe_params_incomplete"),
        "{out}"
    );
    assert_eq!(payload(&out)["missing"], json!(["generation"]), "{out}");

    let mut no_person = params(json!({"runs": runs()}));
    no_person.as_object_mut().expect("params").remove("ctx");
    let out = classify(no_person.clone());
    assert_eq!(
        out["header"]["error_code"],
        json!("wish_incomplete"),
        "{out}"
    );
    let all = run_recipes(&no_person);
    assert_eq!(all[0]["header"]["error_code"], json!("wish_incomplete"));
    assert!(all[0]["manifest"].is_null());
}

/// Without the two words an installation is what it was: no edge names a run.
#[test]
fn an_app_without_runs_draws_no_run_edge() {
    if !shipped() {
        return;
    }
    let got = edges(&params(json!({
        "offers": [{"kind": "tool", "at": "./tools", "tools": ["probe_find"]}],
        "listens": ["answer"], "reads": "./sink"})));
    assert!(!as_set(&got).iter().any(|e| e.contains("run_")), "{got:#?}");
}

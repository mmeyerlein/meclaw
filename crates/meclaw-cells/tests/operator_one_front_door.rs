//! GH #446 / GH #556 — `operator@1.2.0`: one front door into the OS, and the
//! one thing it adds is IDENTITY.
//!
//! The substrate stamps `envelope.reply_to` on a **cell's** emission, and the
//! submitter's gate reads the requester off exactly that and nothing else. An
//! agent inside a colony is a cell and has a path; a person with a shell is
//! not, so a `POST /messages` reaches the gate with no sender and is refused as
//! `requester_unknown` — while the one route that does work,
//! `POST /colony/mutations`, walks past the gate and the broker entirely.
//!
//! This hive lends that person a path. Everything below measures the two halves
//! of that: the boundary (sealed, one occupant per subject, an unknown lane is
//! an answer) and the identity (what leaves `./intake` is what the gate
//! accepts).
//!
//! Since GH #556 the SUBMITTER lives in here, as the `submit` occupant of this
//! hive rather than as a station of the shell beside it. Three things follow,
//! and each of them is measured below: the cell that was called `submit`
//! through 1.0.0 is `intake` — named for what it does rather than for the
//! subject it serves, because the name `submit` now belongs to the ref beside
//! it; `apply` and the receipt back are INTERIOR edges of this hive
//! (`./intake -> ./submit`, `./submit -> ./intake`) instead of two shell edges
//! across two stations; and what crosses this rim is what the submitter needs
//! from outside and nothing else — `ask` out with `in_verdict` back, `mutate`
//! on its way to a door that lives in the birth topology of the colony's root,
//! and `sub_receipt` for the two answers the front door cannot give.
//!
//! It measures no authentication, because there is none to measure. That is the
//! template's own not-in-scope and the last test in this file holds it there.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::code_wire::{emit_all, run_shipped_script, shipped_script};

const HIVE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/operator/config.json"
);
const TPL: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/operator/template.json"
);
/// The code cell that turns a posted request into a message with a sender. It
/// was called `submit` through `operator@1.0.0`; `submit` is the ref onto the
/// submitter hive next door since GH #556.
const INTAKE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/operator/intake/config.json"
);
const DRAFTS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/operator/drafts/config.json"
);
const EXPORT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/operator/export/config.json"
);
const LIFECYCLE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/operator/lifecycle/config.json"
);
const UNKNOWN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/operator/unknown/config.json"
);
const GATE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/submit/gate/config.json"
);
const OS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/meclaw-os/config.json"
);

/// The path the substrate stamps on the front door's own emission — the cell
/// that serves `in_submit`, and therefore the identity the gate reads.
const OPERATOR_INTAKE: &str = "/os/operator/intake";

/// Where the submitter stands since GH #556: an occupant of the front door
/// rather than a station of the shell. It is the address the shell's `ask` edge
/// stamps as the broker's `requester`, and the address of the gate below.
const SUBMIT_HIVE: &str = "/os/operator/submit";

fn read(path: &str) -> Value {
    meclaw_core::serde_json::from_str(&std::fs::read_to_string(path).expect(path)).expect(path)
}

fn edges(cfg: &Value) -> Vec<Value> {
    cfg["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .clone()
}

fn routes(cfg: &Value, side: &str) -> Vec<String> {
    cfg["params"]["contract"][side]
        .as_array()
        .expect(side)
        .iter()
        .map(|l| l["route"].as_str().expect("route").to_string())
        .collect()
}

/// One request at the hive, served by the occupant its door names.
fn run(cell: &str, flat: &Value) -> Vec<Value> {
    emit_all(&shipped_script(cell), flat)
}

fn args_call(text: &Value) -> Value {
    json!([{ "origin": "assistant", "type": "tool_call", "id": "c1",
             "text": text.to_string() }])
}

// ── the boundary ─────────────────────────────────────────────────────────────

#[test]
fn the_hive_is_sealed_and_the_path_is_the_only_address() {
    let hive = read(HIVE);
    assert_eq!(
        hive["params"]["ports"],
        json!([]),
        "an empty ports list is the statement `the hive path is the only address`; \
         no key at all would mean unsealed, which means unfinished"
    );
}

#[test]
fn every_accepted_lane_has_a_door_and_every_emitted_lane_an_exit() {
    let hive = read(HIVE);
    let es = edges(&hive);

    // The rim of this hive after GH #556, pinned rather than derived: three of
    // these lanes arrived WITH the submitter and none of them is the front
    // door's own, so a lane silently appearing or vanishing here is a change to
    // what the level around this hive has to wire.
    assert_eq!(
        routes(&hive, "accepts"),
        vec![
            "in_submit",
            "in_dump",
            "in_lifecycle",
            "in_draft",
            "in_verdict",
            "export_done"
        ]
    );
    assert_eq!(
        routes(&hive, "emits"),
        vec!["export", "receipt", "ask", "mutate", "sub_receipt"],
        "`apply` and `in_receipt` are NOT rim lanes since GH #556 — they are the \
         two interior edges between `./intake` and `./submit`"
    );

    for lane in routes(&hive, "accepts") {
        assert!(
            es.iter().any(|e| {
                e["from"] == "."
                    && e["to"] != json!(".")
                    && e["condition"]
                        .as_str()
                        .is_some_and(|c| c.contains(&format!("'{lane}'")))
            }),
            "accepted lane {lane} has no door"
        );
    }
    for lane in routes(&hive, "emits") {
        // An exit either fires ON the lane or RE-STAMPS an occupant's own lane
        // onto it on the way out — `sub_receipt` is the second shape: the
        // submitter raises `receipt`, and the edge that lets the two shapes the
        // front door cannot answer for out of the rim is what names the lane.
        assert!(
            es.iter().any(|e| {
                e["to"] == "."
                    && (e["condition"]
                        .as_str()
                        .is_some_and(|c| c.contains(&format!("'{lane}'")))
                        || e["modifier"]["set_hop"]["route"] == json!(format!("'{lane}'")))
            }),
            "emitted lane {lane} has no exit"
        );
    }
}

#[test]
fn one_occupant_per_subject_and_the_default_cannot_catch_an_answer() {
    let hive = read(HIVE);
    let es = edges(&hive);
    for (lane, occupant) in [
        ("in_submit", "./intake"),
        ("in_draft", "./intake"),
        ("in_verdict", "./submit"),
        ("in_dump", "./export"),
        ("export_done", "./export"),
        ("in_lifecycle", "./lifecycle"),
    ] {
        assert!(
            es.iter().any(|e| {
                e["from"] == "."
                    && e["to"] == json!(occupant)
                    && e["condition"]
                        .as_str()
                        .is_some_and(|c| c.contains(&format!("hop.route == '{lane}'")))
            }),
            "{lane} must reach {occupant}"
        );
    }
    // The guarded default. Edges from `.` fire on messages AT the hive path,
    // and an occupant's answer passes through that path on its way out — so a
    // default door with no guard would hand every receipt back to `unknown`
    // and loop the hive against itself.
    let default = es
        .iter()
        .find(|e| e["default"] == json!(true))
        .expect("a default door");
    assert_eq!(default["to"], "./unknown");
    let guard = default["condition"].as_str().expect("a guard");
    for own in routes(&read(HIVE), "emits") {
        assert!(
            guard.contains(&format!("hop.route != '{own}'")),
            "the default door must exclude this hive's own lane {own}: {guard}"
        );
    }
}

#[test]
fn an_unknown_lane_is_a_receipt_and_never_a_dead_letter() {
    for (asked, shown) in [("in_restart", "in_restart"), ("", "(none)")] {
        let out = run(
            UNKNOWN,
            &json!({
                "target": "/os/operator",
                "header": { "hop": { "route": asked, "tool_call_id": "op:c1" },
                            "context": {} },
                "ttl": 64, "messages": [], "params": {}
            }),
        );
        assert_eq!(out.len(), 1, "one answer, never a silence");
        let h = &out[0]["header"];
        assert_eq!(h["route"], "receipt", "on the one lane out");
        assert_eq!(h["error_code"], "unknown_route");
        assert_eq!(h["route_asked"], asked, "the lane, verbatim");
        assert!(
            out[0]["messages"][0]["text"]
                .as_str()
                .expect("a sentence")
                .contains(shown)
        );
    }
}

// ── the identity ─────────────────────────────────────────────────────────────

fn a_manifest() -> Value {
    json!([
        { "scope": "/os/orgs/acme", "ctx": {}, "diff": { "add_edges": [] } }
    ])
}

fn submit_request(manifest: &Value, pin: &str) -> Vec<Value> {
    let mut hop = json!({ "route": "in_submit" });
    if !pin.is_empty() {
        hop["manifest_sha256"] = json!(pin);
    }
    run(
        INTAKE,
        &json!({
            "target": "/os/operator",
            "header": { "hop": hop, "context": {} },
            "ttl": 64,
            "manifest": manifest,
            "messages": [],
            "params": {}
        }),
    )
}

#[test]
fn a_submission_leaves_with_the_digest_drawn_over_the_bytes_it_forwards() {
    let manifest = a_manifest();
    let out = submit_request(&manifest, "");
    assert_eq!(out.len(), 1, "one emission on `apply`");
    let h = &out[0]["header"];
    assert_eq!(h["route"], "apply");
    assert_eq!(h["declaration_count"], 1);
    assert_eq!(out[0]["manifest"], manifest, "verbatim, byte for byte");
    let sha = h["manifest_sha256"].as_str().expect("a digest");
    assert_eq!(sha.len(), 64, "a sha256 hex digest is 64 characters");
    // The prefix is what the level above tells an operator's receipt by: the
    // submitter hands the id back verbatim off its own flight row, and the
    // colony's answer begins a fresh trace, so the context it travelled in is
    // gone by then.
    assert!(
        h["tool_call_id"]
            .as_str()
            .expect("an id")
            .starts_with("op:"),
        "the round is marked on the id"
    );
}

#[test]
fn a_pin_that_does_not_match_is_refused_here_rather_than_three_hops_later() {
    let out = submit_request(&a_manifest(), "deadbeef");
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["header"]["route"], "receipt");
    assert_eq!(out[0]["header"]["error_code"], "manifest_digest_mismatch");
    assert_eq!(out[0]["header"]["expected"], "deadbeef");
}

#[test]
fn an_empty_submission_says_so() {
    let out = submit_request(&json!([]), "");
    assert_eq!(out[0]["header"]["error_code"], "manifest_missing");
}

#[test]
fn what_leaves_the_front_door_is_what_the_submitters_gate_accepts() {
    // THE test of this template. The same manifest and the same digest, handed
    // to the shipped gate on the lane the edge `./intake -> ./submit` re-stamps
    // it onto, with the front door's own path as `reply_to` — the one the
    // substrate would have stamped, because the emitting node is a CELL. The
    // hop is one edge long since GH #556 and the identity on it is unchanged:
    // what moved is where the submitter STANDS, not who it is told to attribute
    // a mutation to.
    let manifest = a_manifest();
    let apply = &submit_request(&manifest, "")[0];
    let sha = apply["header"]["manifest_sha256"].as_str().expect("digest");

    let out = emit_all(
        &shipped_script(GATE),
        &json!({
            "target": SUBMIT_HIVE,
            "reply_to": OPERATOR_INTAKE,
            "header": { "hop": { "route": "in_apply", "manifest_sha256": sha,
                                 "tool_call_id": apply["header"]["tool_call_id"] },
                        "context": {} },
            "ttl": 64,
            "manifest": apply["manifest"],
            "messages": [],
            "params": {}
        }),
    );
    assert_eq!(out.len(), 2, "park, then ask — not a refusal");
    assert!(
        out.iter()
            .all(|m| m["header"]["error_code"] != json!("requester_unknown")),
        "the whole point: the submission is no longer anonymous"
    );
    let park: Value = meclaw_core::serde_json::from_str(
        out[0]["messages"][0]["text"].as_str().expect("a tool_call"),
    )
    .expect("json");
    assert_eq!(
        park["row"]["requester"], OPERATOR_INTAKE,
        "the operator's mutations carry a name into the log"
    );
    let ask: Value = meclaw_core::serde_json::from_str(
        out[1]["messages"][0]["text"].as_str().expect("a tool_call"),
    )
    .expect("json");
    assert_eq!(ask["subject"], OPERATOR_INTAKE);
    assert_eq!(ask["capability"], "colony.mutate");
}

#[test]
fn a_receipt_comes_back_as_one_sentence_and_its_counts() {
    let out = run(
        INTAKE,
        &json!({
            "target": "/os/operator",
            "header": { "hop": { "route": "in_receipt", "operation": "submit",
                                 "applied": 2, "tool_call_id": "op:c1",
                                 "manifest_sha256": "abc" }, "context": {} },
            "ttl": 64,
            "messages": [{ "origin": "tool", "type": "tool_result", "id": "op:c1",
                           "text": "manifest applied: 2 declaration(s)" }],
            "params": {}
        }),
    );
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["header"]["route"], "receipt");
    assert_eq!(out[0]["header"]["applied"], 2);
    assert!(
        out[0]["messages"][0]["text"]
            .as_str()
            .expect("text")
            .contains("2 declaration")
    );

    let refused = run(
        INTAKE,
        &json!({
            "target": "/os/operator",
            "header": { "hop": { "route": "in_receipt",
                                 "error_code": "code_author_denied",
                                 "tool_call_id": "op:c1" }, "context": {} },
            "ttl": 64,
            "messages": [{ "origin": "tool", "type": "tool_result", "id": "",
                           "text": "{\"detail\": \"no rule\"}" }],
            "params": {}
        }),
    );
    assert_eq!(refused[0]["header"]["error_code"], "code_author_denied");
    assert!(
        refused[0]["messages"][0]["text"]
            .as_str()
            .expect("text")
            .contains("code_author_denied")
    );
}

// ── export ───────────────────────────────────────────────────────────────────

#[test]
fn an_export_carries_an_address_and_refuses_to_broadcast() {
    let out = run(
        EXPORT,
        &json!({
            "target": "/os/operator",
            "header": { "hop": { "route": "in_dump", "tool_call_id": "op:c1" },
                        "context": {} },
            "ttl": 64,
            "messages": args_call(&json!({ "target": "/os/orgs/acme/members/alex" })),
            "params": {}
        }),
    );
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["header"]["route"], "export");
    assert_eq!(out[0]["header"]["target"], "/os/orgs/acme/members/alex");

    let nowhere = run(
        EXPORT,
        &json!({
            "target": "/os/operator",
            "header": { "hop": { "route": "in_dump" }, "context": {} },
            "ttl": 64, "messages": args_call(&json!({})), "params": {}
        }),
    );
    assert_eq!(nowhere[0]["header"]["route"], "receipt");
    assert_eq!(nowhere[0]["header"]["error_code"], "export_target_missing");
}

// ── lifecycle ────────────────────────────────────────────────────────────────

fn lifecycle(args: &Value) -> Vec<Value> {
    run(
        LIFECYCLE,
        &json!({
            "target": "/os/operator",
            "header": { "hop": { "route": "in_lifecycle", "tool_call_id": "op:c1" },
                        "context": {} },
            "ttl": 64, "messages": args_call(args), "params": {}
        }),
    )
}

#[test]
fn birth_defaults_to_asleep_and_that_is_a_state_the_door_reads() {
    let out = lifecycle(&json!({
        "op": "birth", "scope": "/os/orgs/acme",
        "node": { "name": "probe", "template": "terminal" }
    }));
    assert_eq!(out.len(), 1);
    assert_eq!(
        out[0]["header"]["route"], "submit",
        "handed to the occupant next door"
    );
    let decl = &out[0]["manifest"][0];
    assert_eq!(decl["scope"], "/os/orgs/acme");
    assert_eq!(decl["diff"]["add_nodes"][0]["birth"], "inactive");
    assert_eq!(decl["diff"]["add_nodes"][0]["template"], "terminal");
    assert!(
        decl["diff"].get("add_edges").is_none(),
        "no edge was asked for and none is invented"
    );
}

#[test]
fn wake_and_sleep_are_edges_because_the_substrate_has_no_other_word_for_them() {
    let wake = lifecycle(&json!({
        "op": "wake", "scope": "/os/orgs/acme",
        "edges": [{ "from": "./hub", "to": "./probe" }]
    }));
    assert_eq!(
        wake[0]["manifest"][0]["diff"]["add_edges"][0]["to"], "./probe",
        "the edge IS the wake: activity is derived from the edge table"
    );
    let sleep = lifecycle(&json!({
        "op": "sleep", "scope": "/os/orgs/acme",
        "edges": [{ "from": "./hub", "to": "./probe" }]
    }));
    assert_eq!(
        sleep[0]["manifest"][0]["diff"]["remove_edges"][0]["to"],
        "./probe"
    );
    assert!(
        sleep[0]["manifest"][0]["diff"].get("add_edges").is_none(),
        "one operation per word"
    );
}

#[test]
fn a_lifecycle_request_that_names_no_mechanism_is_refused_rather_than_guessed_at() {
    for (args, code) in [
        (
            json!({ "op": "restart", "scope": "/os" }),
            "unknown_lifecycle_op",
        ),
        (
            json!({ "op": "wake", "scope": "/os" }),
            "wake_edges_missing",
        ),
        (
            json!({ "op": "sleep", "scope": "/os" }),
            "sleep_edges_missing",
        ),
        (json!({ "op": "birth", "scope": "/os" }), "node_missing"),
        (
            json!({ "op": "birth", "scope": "relative" }),
            "scope_missing",
        ),
    ] {
        let out = lifecycle(&args);
        assert_eq!(out.len(), 1, "{code}");
        assert_eq!(out[0]["header"]["route"], "receipt", "{code}");
        assert_eq!(out[0]["header"]["error_code"], code);
        assert!(
            out.iter().all(|m| m.get("manifest").is_none()),
            "{code}: nothing was composed"
        );
    }
}

// ── the front door carries it, and the shell doors the lanes ─────────────────

#[test]
fn the_front_door_carries_the_submission_to_the_submitter_and_the_receipt_back() {
    // GH #556 MOVED this proof, it did not retire it: both edges used to be the
    // shell's, across two of its stations, for what is one job. They are
    // interior edges of the front door now, and the road they describe is the
    // same one — `apply` re-stamped `in_apply` on the way in, `receipt`
    // re-stamped `in_receipt` on the way back.
    let hive = read(HIVE);
    let es = edges(&hive);
    let apply = es
        .iter()
        .find(|e| e["from"] == "./intake" && e["to"] == "./submit")
        .expect("intake -> submit");
    assert_eq!(apply["condition"], "has(hop.route) && hop.route == 'apply'");
    assert_eq!(apply["modifier"]["set_hop"]["route"], "'in_apply'");

    let back = es
        .iter()
        .find(|e| e["from"] == "./submit" && e["to"] == "./intake")
        .expect("submit -> intake");
    assert_eq!(back["modifier"]["set_hop"]["route"], "'in_receipt'");
    // R-Zielfluss (a): the edge is UNGUARDED, and that is a consequence rather
    // than a looseness — this cell is the only sender of `in_apply`, so every
    // receipt the submitter raises belongs to a round that started here. The
    // assertion is therefore on the premise and not on a condition string: if a
    // second `in_apply` sender ever appears, this is the test that says so.
    let cond = back["condition"].as_str().expect("a condition");
    assert_eq!(cond, "has(hop.route) && hop.route == 'receipt'");
    let senders: Vec<&str> = es
        .iter()
        .filter(|e| {
            e["to"] == "./submit" && e["modifier"]["set_hop"]["route"] == json!("'in_apply'")
        })
        .map(|e| e["from"].as_str().expect("a from"))
        .collect();
    assert_eq!(
        senders,
        vec!["./intake"],
        "the unguarded receipt edge above is only correct while the front door's \
         own cell is the ONE sender of `in_apply`: {senders:?}"
    );

    // And the shell no longer draws either of them, because it no longer has a
    // `./submit` to draw them to. An edge that survived the move would deliver
    // the same submission twice, from two levels.
    let shell = edges(&read(OS));
    let stale: Vec<&Value> = shell
        .iter()
        .filter(|e| e["from"] == "./submit" || e["to"] == "./submit")
        .collect();
    assert!(
        stale.is_empty(),
        "the submitter is not an occupant of the shell since GH #556, so no shell \
         edge may name `./submit`: {stale:?}"
    );
}

#[test]
fn the_shell_doors_every_caller_lane_to_the_front_door_and_takes_the_answer_out() {
    let os = read(OS);
    let es = edges(&os);
    for lane in ["in_submit", "in_dump", "in_lifecycle"] {
        assert!(
            es.iter().any(|e| {
                e["from"] == "."
                    && e["to"] == json!("./operator")
                    && e["condition"]
                        .as_str()
                        .is_some_and(|c| c.contains(&format!("hop.route == '{lane}'")))
            }),
            "the shell must door {lane} to the front door"
        );
    }
    assert!(
        es.iter()
            .any(|e| e["from"] == "./operator" && e["to"] == "."),
        "and take the receipt back out"
    );

    // The three lanes the level had to learn with GH #556, because the
    // submitter's reach crosses the front door's rim now instead of the shell's
    // own: the question out to the broker standing beside it, the verdict back,
    // and the checked manifest on its way to the colony's root.
    let ask = es
        .iter()
        .find(|e| {
            e["from"] == "./operator"
                && e["to"] == "./access"
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains("hop.route == 'ask'"))
        })
        .expect("operator -> access on ask");
    assert_eq!(ask["modifier"]["set_hop"]["route"], "'in_request'");
    assert_eq!(
        ask["modifier"]["set_context"]["requester"],
        format!("'{SUBMIT_HIVE}'"),
        "R-AC-1: the broker's `requester` is what the EDGE says, and the edge \
         says the submitter hive — never the front door as a whole"
    );
    let verdict = es
        .iter()
        .find(|e| {
            e["from"] == "./access"
                && e["to"] == "./operator"
                && e["modifier"]["set_hop"]["route"] == json!("'in_verdict'")
        })
        .expect("access -> operator on the verdict");
    assert!(
        verdict["condition"]
            .as_str()
            .expect("a condition")
            .contains("context.sub_ask == '1'"),
        "only the answer to a SUBMISSION's question comes back in on this lane"
    );
    assert!(
        es.iter().any(|e| {
            e["from"] == "./operator"
                && e["to"] == "."
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains("hop.route == 'mutate'"))
        }),
        "`mutate` leaves the shell too: the door it ends at is birth topology of \
         the colony's root and never an edge this template draws"
    );

    assert_eq!(
        read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../templates/meclaw-os/operator/config.json"
        ))["cell"]["template"],
        "operator@1.2.0",
        "pinned exactly: a bare name would adopt a new front door on a bump"
    );
    assert!(
        !std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../templates/meclaw-os/submit"
        ))
        .exists(),
        "the shell has ONE fewer occupant since GH #556, and a directory left \
         behind would be a second submitter nobody wired"
    );
}

// ── identity, never authentication ───────────────────────────────────────────

#[test]
fn nothing_in_this_hive_authenticates_anything() {
    // The template says it in prose; this is the half that asserts the
    // mechanism. A token check, a header comparison or a secret in any of the
    // four scripts would be a permission layer the substrate does not have, in
    // the one place a reader has to be able to trust. Four and not six: `drafts`
    // is a store and runs nothing, and `submit` is a ref that brings its own
    // sandbox block from its own template rather than widening this one.
    for cell in [INTAKE, EXPORT, LIFECYCLE, UNKNOWN] {
        let cfg = read(cell);
        // The CODE, without its comments: the prose is allowed to say the word
        // "secret" in order to say there is none, and a gate that could not
        // tell those apart would be a gate authors write around.
        let script: String = cfg["params"]["script_inline"]
            .as_str()
            .expect("script_inline")
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n")
            .to_lowercase();
        for word in [
            "authorization",
            "bearer",
            "api_key",
            "secret",
            "password",
            "token",
            "signature",
        ] {
            assert!(
                !script.contains(word),
                "{cell} reads `{word}` — this hive supplies an identity and \
                 verifies no claim"
            );
        }
        assert_eq!(
            cfg["params"]["sandbox"],
            json!({"trust": "restricted", "network": "deny",
                   "filesystem": {"runtime": true}}),
            "{cell}: the tightest block in the library, and the union says so"
        );
    }
    let tpl = read(TPL);
    let not_in_scope = tpl["description"]["not_in_scope"]
        .as_str()
        .expect("not_in_scope");
    assert!(
        not_in_scope.contains("NEVER AUTHENTICATION"),
        "the prose and the mechanism are one drift lock, not two"
    );
    assert_eq!(tpl["version"], "1.2.0");
    assert_eq!(
        tpl["sandbox_union"]["trust"], "restricted",
        "the widest axis over every occupant, and here they are all the same"
    );
}

#[test]
fn no_cell_of_the_front_door_can_reach_the_mutation_door() {
    // The guardrail of R6 is PRECISED by GH #556 rather than dropped, and this
    // is the precise form. The submitter moved in, so `mutate` crosses this rim
    // as a declared lane — but the front door's own cells are exactly as far
    // from `/colony/mutations` as a model's tool call is, and the reach itself
    // did not change hands: the edge that finally carries the lane there lives
    // in the birth topology of the colony's root, because `/colony/mutations`
    // is not an endpoint a mutation may draw at any scope.
    //
    // Two halves, and neither of them is `the word does not appear in this
    // hive` any more — the hive's own prose is where the retraction is written
    // down, and a test that forbade the string would forbid saying it.

    // (1) No CELL OF THE FRONT DOOR names a colony endpoint at all — not in its
    //     config, not in the script it runs. `submit` is not in this list on
    //     purpose: it is a `ref` onto a template that ships its own gate and is
    //     measured where that template is measured.
    for cell in [INTAKE, EXPORT, LIFECYCLE, UNKNOWN, DRAFTS] {
        let raw = std::fs::read_to_string(cell).expect(cell);
        assert!(
            !raw.contains("/colony/"),
            "{cell} names a colony endpoint — a front-door cell parses a \
             request, formats a message and emits it, and reaches nothing"
        );
    }

    // (2) No EDGE this hive draws names one either, and none of them leaves the
    //     hive's own subtree: every destination is the rim or an occupant, so
    //     the lane that ends at the mutation door has to be carried there by a
    //     level this file cannot write.
    let hive = read(HIVE);
    for e in edges(&hive) {
        for side in ["from", "to"] {
            let node = e[side].as_str().unwrap_or_default();
            assert!(
                !node.contains("/colony/"),
                "an edge of this hive names {node}: the reach onto the mutation \
                 door is birth topology and never an edge drawn in here"
            );
        }
        let to = e["to"].as_str().unwrap_or_default();
        assert!(
            to == "." || to.starts_with("./"),
            "a template has no edge leaving its own subtree: {to}"
        );
    }

    // (3) And `mutate` is a lane of the RIM rather than of a front-door cell:
    //     the only edge that raises it comes from `./submit`, which is the ref
    //     and not a cell of this template.
    let raisers: Vec<String> = edges(&hive)
        .iter()
        .filter(|e| {
            e["to"] == "."
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains("hop.route == 'mutate'"))
        })
        .map(|e| e["from"].as_str().expect("a from").to_string())
        .collect();
    assert_eq!(
        raisers,
        vec!["./submit"],
        "no cell of the front door raises `mutate`, and none may: {raisers:?}"
    );
    // And the digest is one definition here too — the helper block travels
    // between its markers so `gh425_the_digest_is_one_definition` can compare
    // it against the builder's and the submitter's.
    let script = read(INTAKE)["params"]["script_inline"]
        .as_str()
        .expect("script")
        .to_string();
    assert!(script.contains("# --8<-- digest-helper") && script.contains("# --8<-- end"));
    let probe = format!(
        "import json, hashlib\n{}\nimport sys\nsys.stdout.write(digest(json.load(sys.stdin)))\n",
        script
            .split("# --8<-- digest-helper")
            .nth(1)
            .and_then(|s| s.split("# --8<-- end").next())
            .map(|s| format!("# --8<-- digest-helper{s}"))
            .expect("the block")
    );
    let out = run_shipped_script(&probe, &a_manifest().to_string());
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.stdout.len(), 64);
}

// ── R-Zielfluss (a): one submission front door, not two ──────────────────────

/// The path a `tools/build-apply` call takes into the front door and the receipt's
/// way back to the assistant that made it.
const TOOLS_APPLY: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/tools/build-apply/config.json"
);

/// The marker the shell puts on an assistant's submission, and the only thing
/// that tells the two callers of `in_submit` apart.
const AGENT_ID: &str = "op:agent:c2";

#[test]
fn the_shell_hangs_an_assistants_apply_on_the_front_door_and_not_on_the_submitter() {
    // R-Zielfluss (a). The container used to reach `./submit` directly, so a
    // colony had two submission fronts: an assistant's and an operator's. There
    // is one now, and the assertion is in both directions — the new edge is
    // there AND the old one is gone, because an edge that survives a re-hang
    // fans out and delivers the same receipt twice.
    let es = edges(&read(OS));

    assert!(
        !es.iter()
            .any(|e| e["from"] == "./orgs" && e["to"] == "./submit"),
        "`./orgs -> ./submit` is the direct path R-Zielfluss (a) removed"
    );

    let inbound = es
        .iter()
        .find(|e| {
            e["from"] == "./orgs"
                && e["to"] == "./operator"
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains("'build'") && c.contains("'apply'"))
        })
        .expect("orgs -> operator on build/apply");
    assert_eq!(inbound["modifier"]["set_hop"]["route"], "'in_submit'");
    assert_eq!(
        inbound["modifier"]["set_context"]["operator_caller"], "'agent'",
        "the LEVEL says which door this is, on the way in — a caller that said \
         it itself would be making a claim. It is CONTEXT and not hop, because \
         a receipt the gate refuses before parking carries an empty id and \
         context is what survives that road"
    );

    // Edges fan out, so the front door's one `receipt` lane needs both of its
    // destinations guarded, or every operator receipt is also delivered to an
    // assistant that never asked and the other way round.
    let down = es
        .iter()
        .find(|e| {
            e["from"] == "./operator"
                && e["to"] == "./orgs"
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains("'receipt'"))
        })
        .expect("operator -> orgs on receipt");
    assert!(
        down["condition"]
            .as_str()
            .expect("a condition")
            .contains("hop.submitter_kind == 'agent'"),
        "only an assistant's receipt goes back down"
    );
    assert_eq!(down["modifier"]["set_hop"]["route"], "'in_build_result'");
    assert_eq!(down["modifier"]["set_hop"]["build_op"], "'apply'");

    // `./operator -> .` is more than one edge since GH #556 — the front door's
    // rim carries `mutate` out too — so the guard is read off the RECEIPT edge
    // by name and not off the first match.
    let out = es
        .iter()
        .find(|e| {
            e["from"] == "./operator"
                && e["to"] == "."
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains("hop.route == 'receipt'"))
        })
        .expect("operator -> . on receipt");
    let cond = out["condition"].as_str().expect("a condition");
    assert!(
        cond.contains("hop.submitter_kind != 'agent'"),
        "and only a person's leaves the colony: {cond}"
    );

    // The submitter has no edge of its own at this level at all any more, so
    // there is no second road a receipt could take back to the container: it
    // leaves through the front door that submitted it or not at all.
    assert!(
        !es.iter()
            .any(|e| e["from"] == "./submit" || e["to"] == "./submit"),
        "the shell draws no edge to or from `./submit` since GH #556 — the \
         submitter's receipt reaches the container through the front door"
    );
}

#[test]
fn an_assistants_apply_reaches_the_gate_under_the_front_doors_identity() {
    // Phase A of the agent lane, measured on the shipped scripts rather than
    // asserted about them: the marker the shell stamps becomes a marked id, and
    // what the gate reads off the envelope is the front door's path.
    let manifest = a_manifest();
    let out = run(
        INTAKE,
        &json!({
            "target": "/os/operator",
            "header": { "hop": { "route": "in_submit", "tool_call_id": "c2" },
                        "context": { "operator_caller": "agent" } },
            "ttl": 64,
            "manifest": manifest,
            "messages": [],
            "params": {}
        }),
    );
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["header"]["route"], "apply");
    assert_eq!(
        out[0]["header"]["tool_call_id"], AGENT_ID,
        "the door is written INTO the id, because the colony's answer to a \
         mutation begins a fresh trace and carries nothing else of the round"
    );

    let sha = out[0]["header"]["manifest_sha256"]
        .as_str()
        .expect("a digest");
    let gate = emit_all(
        &shipped_script(GATE),
        &json!({
            "target": SUBMIT_HIVE,
            "reply_to": OPERATOR_INTAKE,
            "header": { "hop": { "route": "in_apply", "manifest_sha256": sha,
                                 "tool_call_id": AGENT_ID },
                        "context": {} },
            "ttl": 64,
            "manifest": out[0]["manifest"],
            "messages": [],
            "params": {}
        }),
    );
    assert_eq!(gate.len(), 2, "park, then ask — not a refusal");
    let park: Value = meclaw_core::serde_json::from_str(
        gate[0]["messages"][0]["text"]
            .as_str()
            .expect("a tool_call"),
    )
    .expect("json");
    assert_eq!(
        park["row"]["requester"], OPERATOR_INTAKE,
        "an agent's manifest is attributed to the front door it came through"
    );
    assert_eq!(
        park["row"]["tool_call_id"], AGENT_ID,
        "the flight row keeps the marked id, and hands it back verbatim"
    );
}

#[test]
fn the_receipt_of_an_assistants_apply_comes_back_under_the_id_the_tool_call_used() {
    // Phase B of the agent lane, and the reason the marker is stripped again:
    // the fan-in of the round waits for `c2`, and a tool_result under
    // `op:agent:c2` is a round that never ends.
    let back = run(
        INTAKE,
        &json!({
            "target": "/os/operator",
            "header": { "hop": { "route": "in_receipt", "operation": "submit",
                                 "applied": 1, "tool_call_id": AGENT_ID },
                        "context": {} },
            "ttl": 64,
            "messages": [{ "origin": "tool", "type": "tool_result", "id": AGENT_ID,
                           "text": "manifest applied: 1 declaration(s)" }],
            "params": {}
        }),
    );
    assert_eq!(back.len(), 1);
    assert_eq!(back[0]["header"]["route"], "receipt");
    assert_eq!(
        back[0]["header"]["submitter_kind"], "agent",
        "the marker read off the id becomes the hop key the shell routes on"
    );
    assert_eq!(back[0]["header"]["tool_call_id"], "c2");
    assert_eq!(back[0]["messages"][0]["id"], "c2");

    // And the last hop: the shell re-stamps it `in_build_result`, three levels
    // relay it unchanged, and the assistant's own `apply` tool renders it as
    // the tool_result of the call that started this.
    let result = emit_all(
        &shipped_script(TOOLS_APPLY),
        &json!({
            "target": "/os/orgs/acme/members/alex/assistants/scribe/tools",
            "header": { "hop": { "route": "in_build_result", "build_op": "apply",
                                 "applied": 1,
                                 "tool_call_id": back[0]["header"]["tool_call_id"] },
                        "context": {} },
            "ttl": 64,
            "messages": back[0]["messages"].clone(),
            "params": {}
        }),
    );
    assert_eq!(result.len(), 1);
    assert_eq!(
        result[0]["header"]["tool_call_id"], "c2",
        "the round closes under the id it opened with"
    );
    assert_eq!(result[0]["messages"][0]["id"], "c2");
    assert!(
        result[0]["messages"][0]["text"]
            .as_str()
            .expect("text")
            .contains("1 declaration")
    );
}

#[test]
fn a_person_at_the_rim_is_not_marked_as_an_agent() {
    // The other half of the discriminator, asserted rather than assumed: a
    // request with no `submitter_kind` gets the plain marker and its receipt
    // carries no key, so the guarded rim edge takes it and the guarded edge
    // into the container does not.
    let out = submit_request(&a_manifest(), "");
    let id = out[0]["header"]["tool_call_id"]
        .as_str()
        .expect("an id")
        .to_string();
    assert!(id.starts_with("op:") && !id.starts_with("op:agent:"));

    let back = run(
        INTAKE,
        &json!({
            "target": "/os/operator",
            "header": { "hop": { "route": "in_receipt", "applied": 1,
                                 "tool_call_id": id }, "context": {} },
            "ttl": 64,
            "messages": [{ "origin": "tool", "type": "tool_result", "id": id,
                           "text": "manifest applied: 1 declaration(s)" }],
            "params": {}
        }),
    );
    assert!(
        back[0]["header"].get("submitter_kind").is_none(),
        "an operator's receipt carries no agent marker"
    );

    // A caller that marks its OWN id does not get to choose the door: the
    // prefix records which edge a request arrived on, and that is this level's
    // fact rather than the caller's claim.
    let forged = run(
        INTAKE,
        &json!({
            "target": "/os/operator",
            "header": { "hop": { "route": "in_submit", "tool_call_id": AGENT_ID },
                        "context": {} },
            "ttl": 64,
            "manifest": a_manifest(),
            "messages": [],
            "params": {}
        }),
    );
    assert_eq!(
        forged[0]["header"]["tool_call_id"], "op:c2",
        "the claimed marker is stripped and the real one written"
    );
}

#[test]
fn a_refused_round_is_still_an_agents_round_when_the_id_came_back_empty() {
    // The road that the id alone cannot carry, and the reason there are two
    // carriers. A manifest the gate refuses BEFORE it parks anything leaves no
    // flight row, so the receipt comes back with an empty `tool_call_id` —
    // measured, not assumed: it is what `requester_not_permitted` does in the
    // shipped submitter. Read off the id alone, such a round would look like an
    // operator's and the receipt would leave the colony instead of reaching the
    // assistant that submitted.
    let back = run(
        INTAKE,
        &json!({
            "target": "/os/operator",
            "header": { "hop": { "route": "in_receipt",
                                 "error_code": "requester_not_permitted",
                                 "tool_call_id": "" },
                        "context": { "operator_caller": "agent",
                                     "build_call_id": "c2" } },
            "ttl": 64,
            "messages": [{ "origin": "tool", "type": "tool_result", "id": "",
                           "text": "{\"detail\": \"the broker refused this submission\"}" }],
            "params": {}
        }),
    );
    assert_eq!(back.len(), 1);
    assert_eq!(back[0]["header"]["error_code"], "requester_not_permitted");
    assert_eq!(
        back[0]["header"]["submitter_kind"], "agent",
        "context is the carrier that survives a refusal inside the level, and \
         the id is the one that survives the mutation door — the two failures \
         are disjoint, which is why both are read"
    );
    // And the id stays empty rather than being invented: `tools/build-apply` reads
    // `context.build_call_id` first for exactly this case, and a made-up id
    // would be the one thing that could close the wrong round.
    assert!(back[0]["header"].get("tool_call_id").is_none());

    let result = emit_all(
        &shipped_script(TOOLS_APPLY),
        &json!({
            "target": "/os/orgs/acme/members/alex/assistants/scribe/tools",
            "header": { "hop": { "route": "in_build_result", "build_op": "apply",
                                 "error_code": "requester_not_permitted" },
                        "context": { "build_call_id": "c2" } },
            "ttl": 64,
            "messages": back[0]["messages"].clone(),
            "params": {}
        }),
    );
    assert_eq!(result[0]["header"]["tool_call_id"], "c2");
    assert!(
        result[0]["messages"][0]["text"]
            .as_str()
            .expect("text")
            .contains("requester_not_permitted")
    );
}

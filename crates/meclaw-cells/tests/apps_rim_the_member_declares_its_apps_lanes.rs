//! The apps rim — `member@1.7.0` and `assistant@2.6.0` (rulings 2026-09-04/05).
//!
//! An app is a sub-form of a member: it LISTENS on lanes of the person
//! (`turn`, `answer`, `partial`, and the tool answers of the generation), it
//! OFFERS a tool to the assistant, and it WRITES a view onto a screen. All
//! three are edges at the rim — never an intercept, never a rewiring of what is
//! already there.
//!
//! # Two owners, and which half is in the library
//!
//! The ruling of 2026-09-05 splits the wiring in two, and this file is the
//! proof of the split:
//!
//! * **The template DECLARES.** `member@1.7.0` names the observer lanes on its
//!   own `./apps` container with `at: ["./apps"]` — `turn` and `partial` as
//!   emits, `tool_result` and `tool_schemas` as accepts. A declaration makes
//!   the member a mandatory hop for a v-lane carrying one of those lanes from
//!   outside (ADR-0020) and lets the level's own consistency tests know the
//!   lanes exist. It draws nothing.
//! * **The installing mutation DRAWS.** The fan-out edges
//!   `./firewall|./assistants|./channels -> ./apps` and the container binding
//!   `./apps -> ./apps/<app>` belong to the manifest that installs an app —
//!   *whoever listens orders it*, the same rule `voice` follows with
//!   `emit_partials`. A member with no listening app therefore carries no such
//!   edge and dead-letters nothing, which is why this test asserts their
//!   ABSENCE from the library as sharply as it asserts the declarations'
//!   presence.
//!
//! The two restamp edges `./apps -> ./assistants` are the exception, and for a
//! measurable reason: without an installed app nothing ever arrives on them, so
//! they cost a member without apps nothing at all. They are the mirror of the
//! memory's pair (GH #552).
//!
//! Everything here is a fact about the FILES — no colony, no runtime, the same
//! reasoning as `gh173_shipped_hive_contracts` and
//! `gh302_member_holds_the_memory`. Guarded like every other template-reading
//! test (GH #49): a template that did not travel into this tree is skipped
//! rather than judged.

use meclaw_colony::config::{EdgeSpec, HiveParams, LaneSpec};
use meclaw_core::serde_json::Value;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// `templates/<name>`, or `None` when this tree did not ship it (GH #49).
fn shipped(name: &str) -> Option<std::path::PathBuf> {
    let p = repo("templates").join(name);
    p.join("config.json").is_file().then_some(p)
}

/// The `params` of a hive `config.json`, through the substrate's own reader.
fn hive_params(dir: &std::path::Path) -> HiveParams {
    let p = dir.join("config.json");
    let raw = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    let cfg: Value =
        meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    let params = cfg
        .get("params")
        .cloned()
        .unwrap_or_else(|| panic!("{}: no params block", dir.display()));
    meclaw_core::serde_json::from_value(params)
        .unwrap_or_else(|e| panic!("{}/config.json: params: {e}", dir.display()))
}

/// The version a shipped top-level template declares, as a string.
fn declared_version(name: &str) -> Option<String> {
    let p = repo("templates").join(name).join("template.json");
    let raw = std::fs::read_to_string(p).ok()?;
    let v: Value = meclaw_core::serde_json::from_str(&raw).ok()?;
    v.get("version").and_then(Value::as_str).map(str::to_string)
}

/// One lane of a contract list, blaming the template when it is gone.
fn lane<'a>(lanes: &'a [LaneSpec], route: &str, what: &str) -> &'a LaneSpec {
    lanes
        .iter()
        .find(|l| l.route == route)
        .unwrap_or_else(|| panic!("{what} has no lane `{route}` any more"))
}

/// True iff this edge's condition names `route` the way every door and exit in
/// these templates writes it.
fn condition_names(e: &EdgeSpec, route: &str) -> bool {
    let needle = format!("hop.route == '{route}'");
    e.condition
        .as_deref()
        .is_some_and(|c| c.contains(needle.as_str()))
}

/// The single `hop.route` literal this edge's condition names, if it names
/// exactly one — the way an unmodified door names the lane it carries.
fn stated_lane(e: &EdgeSpec) -> Option<&str> {
    let c = e.condition.as_deref()?;
    let at = c.find("hop.route == '")?;
    let rest = &c[at + "hop.route == '".len()..];
    let end = rest.find('\'')?;
    Some(&rest[..end])
}

/// A single-quoted CEL string literal, or `None` for anything computed. Same
/// reading as `hive_contract::constant_route`.
fn constant(src: &str) -> Option<&str> {
    let t = src.trim();
    let inner = t.strip_prefix('\'')?.strip_suffix('\'')?;
    (!inner.contains('\'')).then_some(inner)
}

/// The constant lane this edge STAMPS on what it takes, if any — the second way
/// an edge can name a lane (GH #176).
fn stamped_lane(e: &EdgeSpec) -> Option<&str> {
    e.modifier
        .as_ref()
        .and_then(|m| m.set_hop.get("route"))
        .and_then(|s| constant(s.as_str()))
}

// ─────────────────────────── (a) the declarations at the apps container

/// The member names the observer lanes on `./apps` — and only names them.
///
/// `at` is the whole statement: this lane docks on `./apps`, it is no lane of
/// this rim, and the level takes part in it, so nothing may carry it PAST the
/// member as a v-lane (ADR-0020). `answer` is deliberately NOT in that list:
/// it IS a lane of this rim, it leaves the level as a guarded default, and an
/// `at` on it would switch off the exit check that guards exactly that
/// (`docs/development-rules.md` § 8b — the parent carries it, so the rim does).
#[test]
fn the_member_declares_the_observer_lanes_on_its_apps_container() {
    let Some(member) = shipped("member") else {
        return;
    };
    let hp = hive_params(&member);
    let c = hp.contract.expect("the member declares a contract");

    // `sidecar` joined the two observer lanes in member@1.7.0 (GH #607). It is
    // declared the same way and for the same reason — the lane docks on the
    // container, both its ends are inside this level — and it differs in exactly
    // one place, which the next test measures: the edge INTO the container is
    // the level's, because the rim cannot know the section names.
    for route in ["turn", "partial", "sidecar"] {
        let l = lane(&c.emits, route, "templates/member (emits)");
        assert_eq!(
            l.at,
            vec!["./apps".to_string()],
            "the member must declare `{route}` as an emits lane docking on `./apps` — \
             the app that listens gets its edge from the installing mutation, but the \
             DECLARATION is the level's own (rulings 2026-09-04/05)"
        );
    }
    for route in ["tool_result", "tool_schemas"] {
        let l = lane(&c.accepts, route, "templates/member (accepts)");
        assert_eq!(
            l.at,
            vec!["./apps".to_string(), "./channels".to_string()],
            "an app's answer and its offer enter this level at `./apps` and end inside \
             `./assistants` — both siblings in here, exactly like `tool`/`schemas` for the \
             memory (GH #552), so `at` says so. Since 2026-09-06 `./channels` stands beside \
             it: a CHANNEL may offer a tool of its own (the phone channel does), and the \
             level says so at a second rim rather than making one of the two rims a \
             special case. The two are one mechanism, not two"
        );
    }

    let answer = lane(&c.emits, "answer", "templates/member (emits)");
    assert!(
        answer.at.is_empty(),
        "`answer` IS a lane of this rim — it leaves the level as a guarded default. An `at` \
         on it would tell the boundary check the parent does not carry it, and the exit \
         check for the rim lane would stop looking (development-rules § 8b). Found: {:?}",
        answer.at
    );
}

// ─────────────────────────── (b) the member draws no observer edge of its own

/// Ruling of 2026-09-05: whoever listens orders it.
///
/// The edges into `./apps` are exactly the ones that were already there before
/// the apps rim existed — the screen's own events and receipts (GH #459) and
/// the mutation receipt (GH #553). No `pass`, no `turn`, no `answer`, no
/// `partial`: those are drawn by the manifest that installs an app, and a
/// member with no such app must not carry an edge that dead-letters into an
/// empty container.
#[test]
fn the_member_draws_no_observer_edge_of_its_own() {
    let Some(member) = shipped("member") else {
        return;
    };
    let hp = hive_params(&member);

    let mut into: Vec<(String, Option<String>)> = hp
        .graph
        .edges
        .iter()
        .filter(|e| e.to == "./apps")
        .map(|e| (e.from.clone(), stated_lane(e).map(str::to_string)))
        .collect();
    into.sort();
    assert_eq!(
        into,
        vec![
            (".".to_string(), Some("mutation_committed".to_string())),
            ("./assistants".to_string(), Some("sidecar".to_string())),
            ("./channels".to_string(), Some("event".to_string())),
            ("./channels".to_string(), Some("receipt".to_string())),
        ],
        "the library ships the three edges into `./apps` it shipped before the apps rim, \
         and — since member@1.7.0 (GH #607) — the sidecar edge. That one IS the level's, \
         and it is the second exception to *whoever listens orders it*: the rim cannot \
         know the sections, because a section is named by whoever OFFERED it and an app \
         is installed long after this template was written, so the level carries every \
         non-`memory` section into the container on ONE edge and the installing mutation \
         draws `./apps -> ./apps/<app>` on the section by name. It costs a member without \
         apps nothing measurable, for the same reason the two restamp edges cost it \
         nothing: a section is only ever written because it was OFFERED. The OBSERVER \
         edges `./firewall -> ./apps` (pass→turn), `./assistants -> ./apps` (answer) and \
         `./channels -> ./apps` (partial) stay the installing mutation's — those lanes \
         exist whether or not anybody listens — see `templates/member/README.md` \
         § Installing an app"
    );

    for banned in ["pass", "turn", "answer", "partial"] {
        assert!(
            !hp.graph
                .edges
                .iter()
                .any(|e| e.to == "./apps" && condition_names(e, banned)),
            "an edge in the member template carries `{banned}` into `./apps`. That edge is \
             the installing mutation's, not the library's (ruling 2026-09-05)"
        );
    }
}

/// The answer's way out of the level is untouched by the apps rim.
///
/// The app's edge is a REGULAR fan-out guarded on the channel key, and a
/// regular edge suppresses the default of the same sender. That is exactly why
/// the app's edge carries the same guard the channel edge does: on the
/// channel-less case no regular edge fires, so the default still does — and an
/// answer to an operator still leaves the member.
#[test]
fn the_answer_exit_to_the_parent_is_still_the_guarded_default() {
    let Some(member) = shipped("member") else {
        return;
    };
    let hp = hive_params(&member);

    let exits: Vec<&EdgeSpec> = hp
        .graph
        .edges
        .iter()
        .filter(|e| e.from == "./assistants" && e.to == "." && condition_names(e, "answer"))
        .collect();
    assert_eq!(
        exits.len(),
        1,
        "there is exactly one way an answer leaves this level, and it is a default (GH #283)"
    );
    assert!(
        exits[0].is_default,
        "`./assistants -> .` on `answer` must stay `default: true`: a second UNGUARDED \
         regular exit would deliver every reply twice, and losing the default would silence \
         the answer to a caller that named no channel of this member"
    );
}

// ─────────────────────────── (c) the two restamp edges out of the container

/// An app's tool answer and its offer re-enter the generation that asked.
///
/// The same shape the memory's answer has since GH #552: the level restamps
/// rather than forwards, so what re-enters `./assistants` is an ordinary
/// `in_tool` / `in_menu` and the grow recipe's container edge carries it the
/// rest of the way on `context.assistant`.
///
/// `context.tool_answerer` is NOT stamped here, and that is the difference to
/// the memory pair: the memory is one instance with a literal name, an app is
/// whatever the mutation instantiated — so the binding edge of that mutation
/// stamps the name, the way `channel_node` is stamped on an app's view edge.
#[test]
fn an_apps_tool_result_and_offer_re_enter_the_generation() {
    let Some(member) = shipped("member") else {
        return;
    };
    let hp = hive_params(&member);

    let restamps: Vec<&EdgeSpec> = hp
        .graph
        .edges
        .iter()
        .filter(|e| e.from == "./apps" && e.to == "./assistants")
        .collect();
    assert_eq!(
        restamps.len(),
        2,
        "the member ships exactly two edges out of `./apps` into `./assistants` — the answer \
         and the offer of an installed app"
    );

    for (lane_in, lane_out) in [("tool_result", "in_tool"), ("tool_schemas", "in_menu")] {
        let e = restamps
            .iter()
            .find(|e| stated_lane(e) == Some(lane_in))
            .unwrap_or_else(|| panic!("no `./apps -> ./assistants` edge on `{lane_in}`"));
        assert_eq!(
            stamped_lane(e),
            Some(lane_out),
            "`{lane_in}` must be RESTAMPED to `{lane_out}` on the way in, the way the \
             memory's answer is (GH #552) — a forwarded lane would arrive at a generation \
             that has no door for it"
        );
        assert!(
            e.modifier
                .as_ref()
                .is_none_or(|m| !m.set_context.contains_key("tool_answerer")),
            "`tool_answerer` is stamped by the installing mutation's binding edge, because \
             only that mutation knows the instance name of the app. A literal here would \
             name every app the same and collapse the menu merge of GH #529"
        );
    }
}

// ─────────────────────────── (d) the assistant's connect points

/// The assistant opens its brain rims for the call and the menu tick, and names
/// where an observer of a tool answer docks.
///
/// `tool` and `schemas` leave the level today on a named edge to the member's
/// memory. Since 2.5.1 they also name their connect points, so a v-lane may
/// start at either brain rim and reach an app's own connect point directly
/// (ADR-0020: the source side is checked against the target hive of the source,
/// which is the generation).
///
/// `tool_result` is the new one and it is deliberately NOT a rim lane: both
/// ends of a tool round are inside this level and stay there. `at: ["./tools"]`
/// is what makes it declarable at all without owing the member an exit
/// (`docs/development-rules.md` § 8b) — and it is what lets an app observe the
/// answer as a v-lane fan-out.
#[test]
fn the_assistant_opens_its_brain_rims_for_tool_and_schemas() {
    let Some(assistant) = shipped("assistant") else {
        return;
    };
    let hp = hive_params(&assistant);
    let c = hp
        .contract
        .expect("templates/assistant declares a contract");

    let brains = vec!["./talky".to_string(), "./cogny".to_string()];
    for route in ["tool", "schemas"] {
        let l = lane(&c.emits, route, "templates/assistant (emits)");
        assert_eq!(
            l.at, brains,
            "since 2.5.1 `{route}` names both brain rims as connect points: an app's tool is \
             offered where the call is made"
        );
    }

    let tr = lane(&c.emits, "tool_result", "templates/assistant (emits)");
    assert_eq!(
        tr.at,
        vec!["./tools".to_string()],
        "the answer of this generation's own tool hive docks at `./tools` — that is the one \
         place an app can observe it from, and saying so is what keeps the lane off this rim"
    );
}

// ─────────────────────────── (e) the versions moved with the declarations

/// A contract that grew is a template that moved (`docs/development-rules.md`
/// § 4): the declarations above are the patch, and the patch has a number.
#[test]
fn the_versions_moved_with_the_declarations() {
    if let Some(v) = declared_version("member") {
        assert_eq!(
            v, "1.7.0",
            "the apps-rim declarations and the two restamp edges shipped as 1.6.2; GH \
             #598 took the receipt restamp edge back out again as 1.6.3; since GH #607 \
             the level is 1.7.0, which adds the `sidecar` lane and the two edges that \
             sort it — the second digit, because the level does something it never \
             promised before"
        );
    }
    if let Some(v) = declared_version("assistant") {
        assert_eq!(
            v, "2.6.0",
            "the connect points on `tool`/`schemas` and the new `tool_result` lane shipped \
             as assistant@2.5.1; since GH #607 the level also emits `sidecar`, which makes \
             it 2.6.0 — one lane added, none taken away"
        );
    }
}

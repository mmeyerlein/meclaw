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

    // Three rims since `assistant@2.7.0`: one talky per channel that asks for its own,
    // and a tool offered where the call is made has to be offerable at every rim a
    // call can be made from (GH #709).
    let brains = vec![
        "./talky".to_string(),
        "./talky-chat".to_string(),
        "./cogny".to_string(),
    ];
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
            v, "2.5.17",
            "the apps-rim declarations and the two restamp edges shipped as 1.6.2; GH \
             #598 took the receipt restamp edge back out again as 1.6.3; GH #607 made the \
             level 1.7.0 with the `sidecar` lane and the two edges that sort it; and since \
             GH #709 it is 1.8.0, because the app rim carries `withdraw` beside `view` and \
             an app can take a view down that it could only let fade before; and since \
             welle-live it is 1.9.0, because the level wires the channel whose model \
             answers on its own timeline (`in_delegation` up, `in_advise` back down) — the second \
             digit each time, because the level does something it never promised before; \
             1.9.1 only pins `affinity@3.4.0`, 1.9.2 only names `display@2.7.0` and \
             1.9.3 pins `memory-hive@3.4.1` and routes a keeper part on its path (GH #712), which is \
             the third digit every time; GH #834 makes it 1.10.0, the brief road through \
             `./affinity`, a second digit that leaves the apps rim as it was; 1.10.1 only pins \
             `affinity@3.6.0` (GH #848) and `memory-hive@3.5.0` (GH #849), the third digit; \
             1.10.2 only pins `memory-hive@3.6.0` (GH #858), the third digit again, and \
             1.10.3 only pins `affinity@3.6.1` (GH #864) and `memory-hive@3.6.1` (GH #863), \
             the third digit once more, as 1.10.4 only pins `memory-hive@3.6.2` (GH #873) \
             and derives `./assistants` from `assistant@2.9.4` (GH #871); 1.10.5 only derives \
             `./assistants` from `assistant@2.9.5` (GH #886); GH #877 and GH #889 make it \
             2.0.0, the first digit: `pack_ack` and `prune` no longer leave the level, \
             and the apps rim stays as it was; GH #896 makes it 2.1.0, the second \
             digit: a channel's `renewed` reaches `./assistants`, and the apps rim \
             stays as it was again; GH #907 and GH #908 make it 2.2.0, the second \
             digit: `./file-space` joins as a fifth holder, reached by the file tools \
             and by a channel's documents, and the apps rim stays as it was once more; \
             2.2.1 only pins `memory-hive@3.6.4` and derives `./assistants` from \
             `assistant@3.3.0` (GH #916), the third digit; GH #926 makes it 2.3.0, the \
             second digit: `in_stats` and `stats` cross the level to and from \
             `./assistants`, and the apps rim stays as it was; it pins \
             `file-space@1.1.1` (GH #929) and derives `./assistants` from \
             `assistant@3.4.0`; 2.3.1 only pins `affinity@3.7.0` and \
             `memory-hive@3.7.0` and derives `./assistants` from `assistant@3.4.1`, \
             the third digit; 2.3.2 only derives `./assistants` from `assistant@3.4.2` \
             (GH #941), the third digit; GH #945 makes it 2.4.0, the second digit: \
             `./graph-space` joins as a sixth holder, fed by the file space's \
             `source_changed`, and the apps rim stays as it was; it pins \
             `file-space@1.2.0`, `affinity@3.8.0` and `memory-hive@3.7.1` and derives \
             `./assistants` from `assistant@3.5.0`; GH #950 and GH #951 make it 2.5.0, the \
             second digit: `./librarian` and `./objects` join as the seventh and eighth \
             holders, and the apps rim stays as it was; it pins `file-space@1.3.0`, \
             `memory-hive@3.8.0` and `graph-space@1.1.0` and derives \
             `./assistants` from `assistant@3.6.0`; 2.5.1 only pins `affinity@3.9.0` \
             and `memory-hive@3.8.1` and derives `./assistants` from `assistant@3.6.1` \
             (GH #965, GH #937), the third digit; 2.5.2 only pins `affinity@3.10.0`, \
             `file-space@1.3.1`, `librarian@1.0.1`, `memory-hive@3.8.2` and `firewall@2.4.0` \
             and derives `./assistants` from `assistant@3.6.2` (GH #979), the third digit; \
             2.5.3 only pins `file-space@1.4.0` and `memory-hive@3.8.3` and derives \
             `./assistants` from `assistant@3.7.0` (GH #980, GH #981, GH #993), the third digit; \
             2.5.4 only pins `file-space@1.4.1` and derives `./assistants` from \
             `assistant@3.7.1` (GH #995), the third digit; 2.5.5 only pins \
             `file-space@1.4.2` (GH #996), the third digit; 2.5.6 only pins \
             `file-space@1.4.3` and `memory-hive@3.8.4` and derives `./assistants` \
             from `assistant@3.7.2` (GH #999), the third digit; 2.5.7 only pins \
             `memory-hive@3.9.0` and derives `./assistants` from `assistant@3.7.3` \
             (GH #1018, GH #1019), the third digit; 2.5.8 only pins `affinity@3.10.1` \
             (GH #1021), the third digit; 2.5.9 only pins `memory-hive@3.9.1` and \
             derives `./assistants` from `assistant@3.7.4` (GH #1037), the third digit; \
             2.5.10 only derives `./assistants` from `assistant@3.7.5` (GH #1038), the third \
             digit; 2.5.11 promotes `recall_input_soft`, pins `memory-hive@3.10.0` and \
             derives `./assistants` from `assistant@3.7.6` (GH #1040), the third digit; \
             2.5.12 only pins `memory-hive@3.11.0` (GH #1039), the third digit; 2.5.13 \
             promotes `recall_input_soft` on the `tool_call` door, pins `memory-hive@3.11.1` \
             and derives `./assistants` from `assistant@3.7.7` (GH #1044), the third digit; \
             2.5.14 only pins `memory-hive@3.11.2` (GH #1042), the third digit; \
             2.5.15 only derives `./assistants` from `assistant@3.7.8` (GH #1036), the third digit; \
             2.5.16 only pins `memory-hive@3.12.0` and derives `./assistants` from \
             `assistant@3.7.9` (GH #1079), the third digit; 2.5.17 only pins `access@2.5.1`, \
             `file-space@1.4.5` and `memory-hive@3.12.1` and derives `./assistants` from \
             `assistant@3.7.10` (GH #801), the third digit"
        );
    }
    if let Some(v) = declared_version("assistant") {
        assert_eq!(
            v, "3.7.10",
            "the connect points on `tool`/`schemas` and the new `tool_result` lane shipped \
             as assistant@2.5.1; GH #607 added `sidecar` and made it 2.6.0; GH #709 made it \
             2.7.0, because the level holds one talky per channel that asks for its own and \
             `<assistant>/talky-chat` is an address a caller can wire; and since welle-live \
             it is 2.8.0, because a duplex voice call reaches the surface on a lane of its \
             own, `in_delegation` — a lane or an address added, none taken away; GH #799 \
             re-points its two refs at `talky@5.2.1` and the number does NOT move, because \
             2.8.0 has not shipped and an unreleased version is extended, never superseded; \
             GH #712 draws the transfer rim at both talkys and pins `talky@5.2.2`, the \
             third digit; and GH #728 rides the same 2.8.1, a repair: the consult edges \
             drop `turn_id` and the ref markers carry `late_after_ms`, no lane added or \
             taken away; GH #834 makes it 2.9.0, the brief lanes at both surfaces; \
             GH #845 makes it 2.9.1, a repair: the consult edges drop the channel's tool \
             scope, no lane added or taken away; GH #858 makes it 2.9.2, only the pins \
             `talky@5.4.1` and `cogny@5.1.1`; GH #863 makes it 2.9.3, only the pins \
             `talky@5.4.2` and `cogny@5.1.2` again; GH #871 makes it 2.9.4, only the pins \
             `talky@5.4.3` and `cogny@5.1.3`, whose collectors show an earlier answer with \
             its block; GH #886 makes it 2.9.5, only the pins `talky@5.4.4` and \
             `cogny@5.1.4`, whose brains declare `hop.model`; GH #889 makes it 3.0.0, the \
             first digit: `in_prune` and `prune` left with `talky@6.0.0`, and it pins \
             `cogny@5.2.0`; GH #894 and GH #896 make it 3.1.0, the second digit: \
             `in_renewed` joins the level, the consult edges carry the core's question \
             and the surface's reply, and it pins `talky@6.1.0` and `cogny@5.3.0`; GH #904 \
             makes it 3.1.1, only the pins `talky@6.1.1` and `cogny@5.3.1`, whose \
             curators keep one standing cache-clock order; GH #908 makes it 3.2.0, the \
             second digit: `./talky`, `./talky-chat` and `./cogny` reach the member's \
             `file_` tools on edges of their own, a road added and no lane taken away; \
             GH #916 makes it 3.3.0, the second digit: `in_pin` joins the level and \
             reaches the curator of each brain, and it pins `talky@6.2.0` and \
             `cogny@5.4.0`; GH #926 makes it 3.4.0, the second digit: `in_stats` \
             joins the level and reaches the curator of the brain `hop.stats_role` \
             names, `stats` leaves it, and it pins `talky@6.3.0` and `cogny@5.5.0`; \
             GH #929 rides along: the door of `in_turn` restores the TTL; 3.4.1 only \
             pins `talky@6.4.0` and `cogny@5.6.0`, the third digit; 3.4.2 only pins \
             `talky@6.4.1` and `cogny@5.6.1` (GH #941), the third digit; GH #944 makes \
             it 3.5.0, the second digit: `./talky` and `./talky-chat` carry the read \
             tools `file_outline` and `file_links`, and it pins `talky@6.4.2` and \
             `cogny@5.6.2`; GH #949, GH #950 and GH #951 make it 3.6.0, the second digit: \
             `in_candidate` and `in_read` join the level, `candidate_ack`, `read` and \
             `thing_seen` leave it, `./talky` and `./talky-chat` carry the directory, \
             catalogue and object tools, and it pins `talky@6.5.0` and `cogny@5.7.0`; \
             3.6.1 only pins `tools@1.4.5` (GH #937), the third digit; 3.6.2 only pins \
             `talky@6.6.0` and `cogny@5.7.1`, the third digit; GH #981 makes it 3.7.0, the \
             second digit: `run_answer` and `run_tool_result` leave the level for the app \
             that handed a run, and it pins `talky@6.6.1` and `cogny@5.8.0`; 3.7.1 only \
             stamps the call's name as `context.called_tool` on the brains' tool edges \
             (GH #995), the third digit; 3.7.2 only pins `talky@6.6.2` and `cogny@5.8.1` \
             (GH #999), the third digit; 3.7.3 only pins `talky@6.6.3` and `cogny@5.8.2` \
             (GH #1018), the third digit; 3.7.4 only pins `talky@6.6.4` and \
             `cogny@5.8.3` (GH #1037), the third digit; 3.7.5 only pins `talky@6.6.5` \
             and `cogny@5.8.4` (GH #1038), the third digit; 3.7.6 only pins \
             `talky@6.6.6` and `cogny@5.8.5` (GH #1040), the third digit; 3.7.7 only pins \
             `talky@6.6.7` and `cogny@5.8.6` (GH #1044), the third digit; 3.7.8 only pins \
             `talky@6.6.8` and `cogny@5.8.7` (GH #1036), the third digit; 3.7.9 only pins \
             `talky@6.7.0` and `cogny@5.9.0` (GH #1079), the third digit; 3.7.10 only pins \
             `talky@6.7.1` and `cogny@5.9.1` (GH #801), the third digit"
        );
    }
}

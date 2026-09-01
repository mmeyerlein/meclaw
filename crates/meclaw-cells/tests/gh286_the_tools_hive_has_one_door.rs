//! GH #286 — the tools hive has ONE door, and the door is a contract.
//!
//! The tool surface of an assistant is today N tool cells wired as peers of the
//! brain, which means the set of tools lives in the CALLER's edge table: adding
//! one is a change to the caller, and replacing all of them with a single
//! code-executing cell is a change to every edge that pointed at one of them.
//! `tools@1.0.0` is the answer #286 asks for — one node, one contract
//! (`tool_call` in, `tool_result` out) — and the property that makes the swap
//! honest is not that the hive exists but that it is **sealed**: with
//! `params.ports: []` the hive path is the only address, so no caller can grow
//! an edge to a single tool and thereby pin the set again.
//!
//! Four facts about the FILES, checkable with no colony and no runtime — the
//! same shape as `gh173_shipped_hive_contracts` and `gh196_shipped_hive_ports`:
//!
//! 1. The template is a hive and it is sealed.
//! 2. It has ONE result lane, `tool_result`, whatever the tool was.
//! 3. It DECLARES the one lane on which a tool of this surface reaches out of
//!    the assistant, and the return lane that answers it.
//! 4. Every lane pair is written in the LANE form of `required_drains`
//!    (GH #237), and the substrate's own reader KEEPS every one of them.
//!
//! **Points 2 and 3 were one assertion until R6** (GH #425, the builder intake),
//! and the change is sanctioned rather than accidental. The old wording was
//! "exactly one lane in, exactly one lane out", and the argument under it was
//! about tool RESULTS: a second RESULT lane puts the choice of tool back into
//! the caller's edge table, which is the coupling this hive exists to remove.
//! That argument is untouched and still enforced below.
//!
//! What R6 adds is a different class. This surface now carries a tool that
//! reaches OUT of the assistant -- a build request travels to `/os/builder` and
//! an answer comes back -- and the honest place for that fact is the contract,
//! not a footnote. It is the same defect `sandbox_union` was written to fix one
//! level down: a radius that exists collectively and is invisible because it is
//! spread over several files. So the count moved from "one and one" to "one
//! result lane, and one declared reach with its return".
//!
//! Point 4 is the one that cannot be read off the JSON alone, and it is the one
//! most likely to rot: the lane form names two routes of the hive's own
//! `params.contract`, and `collect_required_drains` **drops** an entry naming a
//! lane the contract does not have — with a `tracing::warn!` and nothing else.
//! A hive that renamed a lane and forgot the pairing would therefore look
//! exactly like a hive that insists on it. So the declaration is planted in a
//! throwaway colony root and read by the real collector, exactly as
//! `gh202_shipped_drain_requirements` does, rather than by a second opinion
//! about what a lane form looks like.
//!
//! **GH #464 adds a third lane pair**, `in_schemas` / `tool_schemas`, and the
//! reasoning above carries over unchanged: it is not a second RESULT lane, it is
//! a question about this hive. A result belongs to a call somebody made; a
//! declaration belongs to a start-up question, and folding the two would make
//! every collector guess which of them it was handed. So the shape is now "one
//! result lane, one declared reach with its return, and one declaration lane
//! with its return".
//!
//! **What this file deliberately does NOT assert.** Not the edges — the internal
//! dispatch (one conditioned door per tool, one guarded default per lane, one
//! exit per occupant) is asserted in `gh286_one_call_reaches_exactly_one_tool`,
//! where it can be driven as well as read. Not the occupant COUNT either: a new
//! occupant is what adding a tool LOOKS like, and a test that fixed the number
//! would fail on exactly the change this template exists to make cheap. What is
//! asserted about the occupants is what #286 measured and what a later
//! declaration is measured against: each named occupant is the shipped cell type
//! of its own name, `bash` carries the sandbox block verbatim, and the cells that
//! carry none carry none for a reason this file states.

use meclaw_colony::CellFactory;
use meclaw_colony::config::{DrainSpec, HiveParams};
use meclaw_colony::mutation::required_drains::{DrainKind, DrainRequirement};
use meclaw_core::serde_json::Value;

/// Where the synthetic hive lives while it is being read. Any path does; what
/// matters is that the collector resolves it the way a colony resolves one.
const HIVE: &str = "/h";

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

fn tools_root() -> std::path::PathBuf {
    templates_root().join("tools")
}

/// One `config.json` of the shipped tree, parsed.
fn config_at(rel: &str) -> Value {
    let p = tools_root().join(rel);
    let raw = std::fs::read_to_string(&p)
        .unwrap_or_else(|e| panic!("templates/tools/{rel}: {e} — the template must be on disk"));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("templates/tools/{rel}: {e}"))
}

/// The hive's `params`, read by the SUBSTRATE's own reader.
///
/// `HiveParams` is `deny_unknown_fields`, so this also answers the question a
/// hand-rolled read would not: does a colony boot with this file at all.
fn hive_params() -> HiveParams {
    let val = config_at("config.json");
    let params = val
        .get("params")
        .cloned()
        .expect("templates/tools/config.json declares params");
    meclaw_core::serde_json::from_value(params)
        .unwrap_or_else(|e| panic!("templates/tools/config.json: params: {e}"))
}

/// Plant the hive's `config.json` as `/h` of a throwaway colony root and let the
/// substrate say which drain requirements it sees. Only `config.json` is
/// needed: that file IS the declaration, and reading it per mutation is how a
/// live colony learns what a hive insists on.
fn requirements_the_substrate_reads() -> Vec<DrainRequirement> {
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();
    std::fs::create_dir_all(root.join("main/h")).unwrap();
    std::fs::write(root.join("main/config.json"), r#"{"cell":{"type":"hive"}}"#).unwrap();
    std::fs::copy(
        tools_root().join("config.json"),
        root.join("main/h/config.json"),
    )
    .unwrap();
    let paths = [meclaw_core::Path::new(HIVE)];
    meclaw_colony::mutation::required_drains::collect_required_drains(root, paths.iter())
}

// ─────────────────────────────────────────────────────────── 1. the seal

#[test]
fn the_tools_hive_is_sealed_so_the_hive_path_is_the_only_address() {
    let val = config_at("config.json");
    assert_eq!(
        val.get("cell")
            .and_then(|c| c.get("type"))
            .and_then(Value::as_str),
        Some("hive"),
        "templates/tools is not a hive — the whole construct is one NODE with one contract"
    );
    let ports = hive_params()
        .ports
        .expect("templates/tools declares params.ports — absent is the OPEN state, and an open tool surface puts the set of tools back into the caller's edge table");
    assert!(
        ports.is_empty(),
        "templates/tools declares the ports {ports:?}. The seal is the point: a caller that \
         can address one tool cell pins the set of tools, and then adding a tool is a change \
         to the caller. The tool surface is a contract, not a set of addresses."
    );
}

// ──────────────────────────────────────────────────────── 2./3. the lanes

#[test]
fn the_contract_is_one_result_lane_and_one_declared_reach() {
    let params = hive_params();
    let contract = params
        .contract
        .expect("templates/tools declares params.contract — a sealed hive that says nowhere which lanes is addressed by path and lane with nothing to address it by");

    let accepts: Vec<&str> = contract.accepts.iter().map(|l| l.route.as_str()).collect();
    assert_eq!(
        accepts,
        vec!["tool_call", "in_build_result", "in_schemas"],
        "the tools hive takes a call in, takes the builder's answer back in, and — since \
         1.2.0 (GH #464) — takes a caller's declaration of the tool NAMES it uses. The \
         third lane is a question about this hive rather than a fourth thing a caller has \
         to know: it exists so that the list of tools stops being typed into every \
         caller's prompt by hand."
    );

    let emits: Vec<&str> = contract.emits.iter().map(|l| l.route.as_str()).collect();
    assert_eq!(
        emits,
        vec!["tool_result", "build", "tool_schemas"],
        "there is exactly ONE result lane, `tool_result` — whatever the tool was. A \
         second RESULT lane would put the choice of tool back into the caller's edge \
         table, which is the coupling this hive exists to remove; a tool's own refusal \
         travels as `hop.error_code` on that lane, not as a route of its own. `build` \
         is not a result: it is the REACH of this surface, the one lane on which a tool \
         of this assistant addresses something outside it, and it is declared here \
         rather than left to be discovered in an edge table. `tool_schemas` is not a \
         result either: a result belongs to a call somebody made, a declaration belongs \
         to a start-up question, and folding the two would make every collector guess \
         which of them it was handed."
    );

    for lane in contract.accepts.iter().chain(contract.emits.iter()) {
        assert!(
            !lane.because.trim().is_empty(),
            "lane '{}' states no `because`. It travels verbatim into a refusal, and a refusal \
             that cannot say what it protects is one people route around.",
            lane.route
        );
    }
}

// ──────────────────────────────────────────────────────── 4. the pairing

#[test]
fn the_two_lanes_are_paired_in_the_lane_form() {
    let declared = hive_params()
        .required_drains
        .expect("templates/tools declares params.required_drains");
    // Three pairings since GH #464, one per direction the surface is wired in:
    // send me `tool_call` and subscribe to `tool_result`; send me
    // `in_build_result` and subscribe to `build`; send me `in_schemas` and
    // subscribe to `tool_schemas`. The third rests on the first's argument: a
    // caller that asks what there is and does not take the answer back starts
    // with an empty menu and no way to learn that it is empty.
    let want = [
        ("tool_call", "tool_result"),
        ("in_build_result", "build"),
        ("in_schemas", "tool_schemas"),
    ];
    assert_eq!(
        declared.len(),
        want.len(),
        "the tools hive states {} pairings, one per direction it is wired in",
        want.len()
    );
    for (spec, (accepts, emits)) in declared.iter().zip(want) {
        match spec {
            DrainSpec::Lane(d) => {
                assert_eq!((d.accepts.as_str(), d.emits.as_str()), (accepts, emits));
                assert!(
                    !d.because.trim().is_empty(),
                    "the pairing {accepts}/{emits} states no `because`"
                );
            }
            DrainSpec::Port(d) => panic!(
                "the pairing is written in the PORT form (port '{}'), which a sealed hive can never \
                 trigger: `port_is_wired_from_outside` needs an edge onto a child, and `ports: []` \
                 refuses exactly those. Use the lane form (GH #237).",
                d.port
            ),
        }
    }
}

#[test]
fn the_substrate_keeps_the_pairing_it_is_handed() {
    // The half the file cannot answer about itself: `collect_required_drains`
    // DROPS a lane pairing whose names are not in the hive's own contract, with
    // a warning and nothing else — so a declaration that no longer applies
    // reads exactly like one that does.
    let reqs = requirements_the_substrate_reads();
    let want = [
        ("tool_call", "tool_result"),
        ("in_build_result", "build"),
        ("in_schemas", "tool_schemas"),
    ];
    assert_eq!(
        reqs.len(),
        want.len(),
        "the substrate's reader kept {} of the {} pairings templates/tools declares — a dropped \
         entry is a hive that looks like it insists and does not. The lane form names two \
         routes of this hive's OWN params.contract; check that both halves of every pairing \
         are in `accepts` and `emits`.",
        reqs.len(),
        want.len()
    );
    for (req, (want_accepts, want_emits)) in reqs.iter().zip(want) {
        assert_eq!(req.hive_path, HIVE);
        match &req.kind {
            DrainKind::Lane { accepts, emits } => {
                assert_eq!(
                    (accepts.as_str(), emits.as_str()),
                    (want_accepts, want_emits)
                );
            }
            DrainKind::Port { port_path, .. } => {
                panic!(
                    "the reader collected a PORT requirement ('{port_path}') — see the lane-form test above"
                )
            }
        }
    }
}

// ─────────────────────────────────────────── the occupants and their radius

/// The sandbox block GH #286 measured on the shipped instance, verbatim. It is
/// written out here rather than read from `bash`'s own file, because a test that
/// reads the value it is checking checks nothing.
const BASH_SANDBOX: &str =
    r#"{"trust":"restricted","network":"deny","filesystem":{"runtime":true}}"#;

#[test]
fn the_tool_occupants_are_the_shipped_cell_types() {
    // `build-draft` and `build-apply` joined with R6 (GH #425), under those two
    // names since `tools@1.4.0` (GH #554); `file`, `edit` and `schemas`
    // with GH #464. No cell type is invented in this hive: every occupant is the
    // library's own cell of that name, or a `code` cell where what is needed is
    // a carrier rather than a capability. `mcp` and `vault` were on this list
    // until `tools@1.3.0` and left with GH #547 — the converse of the sentence
    // above does not hold, and a cell type existing is not a reason for a
    // directory here.
    for (dir, cell_type) in [
        ("bash", "bash"),
        ("web_fetch", "web_fetch"),
        ("web_search", "web_search"),
        ("file", "file"),
        ("edit", "edit"),
        ("build-draft", "code"),
        ("build-apply", "code"),
        ("schemas", "code"),
    ] {
        let val = config_at(&format!("{dir}/config.json"));
        assert_eq!(
            val.get("cell")
                .and_then(|c| c.get("type"))
                .and_then(Value::as_str),
            Some(cell_type),
            "templates/tools/{dir} is not a `{cell_type}` cell. Each occupant is a copy of a \
             shape the library already carries — no cell type is invented in this hive."
        );
        assert!(
            val.get("contract").is_some(),
            "templates/tools/{dir} keeps no contract block of its own"
        );
    }
}

#[test]
fn bash_carries_the_sandbox_the_issue_measured_and_the_substrate_accepts_it() {
    let val = config_at("bash/config.json");
    let params = val
        .get("params")
        .expect("templates/tools/bash declares params");
    let sandbox = params
        .get("sandbox")
        .unwrap_or_else(|| panic!("templates/tools/bash declares NO params.sandbox. It is the one occupant that does, and the declaration is what a replacement is measured against."));
    let expected: Value = meclaw_core::serde_json::from_str(BASH_SANDBOX).unwrap();
    assert_eq!(
        sandbox, &expected,
        "templates/tools/bash's sandbox is not the block GH #286 measured on the shipped \
         instance. Widening it is a decision, not a cleanup: it moves the blast radius of \
         the whole hive."
    );
    // And the substrate agrees it is a profile at all — a sandbox block the
    // cell factory refuses is a boot failure, not a documentation defect.
    meclaw_cells::BashCellFactory
        .validate_params(params)
        .expect("the bash factory reads templates/tools/bash's params");
}

/// The occupants that carry no `params.sandbox`, and the two different reasons
/// for it. `web_fetch`/`web_search`: egress IS the job. `file`/`edit`: the cell
/// does its own I/O rather than starting a runner, so a PROCESS sandbox bounds
/// nothing it uses — the fence is `params.base_path`. Either way the absence is
/// a measurement, and it is what the declared `sandbox_union` is a union over.
/// `mcp` and `vault` were the fifth and sixth until `tools@1.3.0` (GH #547).
#[test]
fn the_unsandboxed_occupants_carry_no_sandbox_and_that_is_the_shipped_truth() {
    for dir in ["web_fetch", "web_search", "file", "edit"] {
        let val = config_at(&format!("{dir}/config.json"));
        let params = val
            .get("params")
            .unwrap_or_else(|| panic!("templates/tools/{dir} declares params"));
        assert!(
            params.get("sandbox").is_none(),
            "templates/tools/{dir} grew a `params.sandbox`. That is not a tightening to wave \
             through: for the two web cells egress IS what they do and `network: \"deny\"` \
             would turn them off; for the two file cells a process sandbox bounds nothing \
             they use, and their fence is `params.base_path`. The absence is the measurement #286 took. If it is ever added on purpose, \
             the declared blast radius of this hive changes with it and this assertion is \
             where that shows."
        );
    }
}

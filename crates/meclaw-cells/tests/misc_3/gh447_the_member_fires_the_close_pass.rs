//! GH #447 -- the member fires the memory's close pass, and the export lane
//! has a drain that lands on disk.
//!
//! Three lanes were built and then left without a caller, which is the dead-lane
//! class of `docs/development-rules.md` § 2c:
//!
//! * `memory-hive`'s `in_close_pass` (#300) was pinned by tests that read the
//!   HIVE's own files. No composition level ever drew an edge onto it, and
//!   `templates/member/template.json` said so out loud.
//! * `talky` kept a `summarizer` that read the same session close a second time,
//!   into a volatile `system.handover` slot of one generation's own prompt state
//!   (retired in the same change; the pins for that half live in the talky tree).
//! * `in_export` (#243) had no drain at all, so an export produced `no_route`
//!   dead letters instead of files.
//!
//! Everything below is a fact about the FILES -- the same reasoning as
//! `gh302_member_holds_the_memory`: whether a level's wiring matches the
//! contract of the hive it wires is checkable before anything is instantiated.
//! The companion colony test (`gh447_an_export_lands_as_a_seed_set`) drives the
//! sink for real.
//!
//! Two properties are worth naming, because each is a way this wiring could look
//! finished and be wrong:
//!
//! * **The close-pass edge is a FAN-OUT, not a redirection.** The same `write`
//!   still leaves the level, because the archive above and the memory below want
//!   different things from one event. An edge that replaced the exit would take
//!   a lane away from every caller that already drains it.
//! * **Every transfer drain must test `hop.route` and NOTHING else.**
//!   `required_drains` decides by running the described hop through the real edge
//!   evaluator, so an edge additionally guarded on a second hop key evaluates
//!   false under the probe and reads as no drain at all -- the mutation that
//!   wires the export would be refused, and the refusal would name a lane that
//!   looks wired. Since GH #555 that is two lanes: `in_export` pairs with
//!   `export_done` and `in_import` with `dump`.

use meclaw_core::serde_json::Value;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn config(rel: &str) -> Value {
    let p = repo(rel);
    let raw = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{} is not json: {e}", p.display()))
}

fn edges(rel: &str) -> Vec<Value> {
    config(rel)["params"]["graph"]["edges"]
        .as_array()
        .expect("params.graph.edges")
        .clone()
}

fn lanes(rel: &str, side: &str) -> Vec<String> {
    config(rel)["params"]["contract"][side]
        .as_array()
        .expect("contract side")
        .iter()
        .map(|l| l["route"].as_str().expect("route").to_string())
        .collect()
}

/// Every edge from `from` to `to` whose condition mentions `needle`.
fn matching(rel: &str, from: &str, to: &str, needle: &str) -> Vec<Value> {
    edges(rel)
        .into_iter()
        .filter(|e| {
            e["from"] == from
                && e["to"] == to
                && e["condition"].as_str().is_some_and(|c| c.contains(needle))
        })
        .collect()
}

fn only(rel: &str, from: &str, to: &str, needle: &str) -> Value {
    let found = matching(rel, from, to, needle);
    assert_eq!(
        found.len(),
        1,
        "expected exactly one {from} -> {to} edge in {rel} matching {needle:?}, found {found:?}"
    );
    found.into_iter().next().expect("checked above")
}

const MEMBER: &str = "templates/member/config.json";
const ORG: &str = "templates/org/config.json";
const SHELL: &str = "templates/meclaw-os/config.json";
const HIVE: &str = "templates/memory-hive/config.json";

// ---------------------------------------------------------------- close pass

/// The lane is sent, and it is sent off the one event that means a session
/// ended: the close batch an assistant raises on `write`.
#[test]
fn the_member_turns_a_closed_session_into_the_hives_close_pass() {
    let e = only(MEMBER, "./assistants", "./memory-hive", "'write'");
    assert_eq!(
        e["modifier"]["set_hop"]["route"], "'in_close_pass'",
        "the close batch has to arrive on the hive's own lane name"
    );
}

/// Since #291 an `accepts` entry carries a `context` array and the mutation
/// validator refuses a caller whose edge does not promote every key in it. So
/// the assertion is against the ARRAY the hive declares, never against a list
/// copied into this file: a key the hive adds later goes red here rather than at
/// somebody's mutation.
#[test]
fn the_close_pass_edge_promotes_every_key_the_lane_declares() {
    let declared: Vec<String> = config(HIVE)["params"]["contract"]["accepts"]
        .as_array()
        .expect("accepts")
        .iter()
        .find(|a| a["route"] == "in_close_pass")
        .expect("the hive still has an in_close_pass lane")["context"]
        .as_array()
        .expect("the lane declares a context array")
        .iter()
        .map(|k| k.as_str().expect("key").to_string())
        .collect();
    assert!(
        !declared.is_empty(),
        "a close pass without a session is not a close pass"
    );

    let e = only(MEMBER, "./assistants", "./memory-hive", "'write'");
    let promoted = e["modifier"]["set_context"].as_object().expect(
        "the edge promotes the lane's keys itself -- `.` is the door and \
                 nothing is upstream of it, so the edge is the only setter root",
    );
    for key in &declared {
        assert!(
            promoted.contains_key(key),
            "the close-pass edge must promote {key:?}; it promotes {:?}",
            promoted.keys().collect::<Vec<_>>()
        );
    }
}

/// The fan-out property: the close batch still leaves the level. A caller that
/// drains `write` today keeps getting it.
#[test]
fn the_close_batch_still_leaves_the_level() {
    assert_eq!(
        matching(MEMBER, "./assistants", ".", "'write'").len(),
        1,
        "the exit edge for `write` is untouched -- the close pass is a fan-out, \
         and a level that redirected the lane would take it away from every \
         archive already wired to it"
    );
    assert!(
        lanes(MEMBER, "emits").contains(&"write".to_string()),
        "`write` stays a declared emit of this level"
    );
}

/// `memory-hive` pairs `in_close_pass` with BOTH `close_report` and `reject`,
/// and a pairing without a drain outside the hive refuses the mutation that
/// wires the ingress. The receipt is new here; the refusal lane was already out.
#[test]
fn both_close_pass_drains_leave_the_hive() {
    let required: Vec<String> = config(HIVE)["params"]["required_drains"]
        .as_array()
        .expect("required_drains")
        .iter()
        .filter(|d| d["accepts"] == "in_close_pass")
        .map(|d| d["emits"].as_str().expect("emits").to_string())
        .collect();
    assert!(
        required.contains(&"close_report".to_string()) && required.contains(&"reject".to_string()),
        "the hive still pairs the close pass with a receipt and a refusal; got {required:?}"
    );

    for lane in required {
        assert_eq!(
            matching(MEMBER, "./memory-hive", ".", &format!("'{lane}'")).len(),
            1,
            "the member has to carry `{lane}` out of the hive, or the mutation \
             that wires the close pass is refused with required_drain_missing"
        );
        assert!(
            lanes(MEMBER, "emits").contains(&lane),
            "an edge to `.` on a lane the level does not declare is the mirror \
             image of a declared lane with no edge -- `{lane}` needs both"
        );
    }
}

// -------------------------------------------------------------------- export

#[test]
fn the_member_declares_the_export_lane_and_opens_a_door_for_it() {
    assert!(
        lanes(MEMBER, "accepts").contains(&"in_export".to_string()),
        "the level accepts in_export"
    );
    let e = only(MEMBER, ".", "./memory-hive", "'in_export'");
    assert!(
        e.get("modifier").is_none(),
        "the lane is named the same on both sides, so the door translates \
         nothing: {e:?}"
    );
}

/// The drain that made the lane real. The plainness of the condition is the
/// load-bearing half — and since GH #555 the destination is the LEVEL's own rim
/// rather than a cell of it: the hive's store writes its seed set itself, so
/// what still travels on `dump` is the receipt of an applied import part, and it
/// crosses this level like every other receipt.
#[test]
fn every_dump_part_reaches_a_plain_route_test_and_leaves_the_level() {
    let e = only(MEMBER, "./memory-hive", ".", "'dump'");
    let cond = e["condition"].as_str().expect("condition");
    assert_eq!(
        cond, "has(hop.route) && hop.route == 'dump'",
        "the drain probe runs the described hop through the real edge evaluator, \
         so an edge guarded on anything the probe does not describe (hop.dump_kind, \
         say) evaluates false and reads as NO drain at all"
    );
}

#[test]
fn both_export_drains_leave_the_hive() {
    let required: Vec<String> = config(HIVE)["params"]["required_drains"]
        .as_array()
        .expect("required_drains")
        .iter()
        .filter(|d| d["accepts"] == "in_export")
        .map(|d| d["emits"].as_str().expect("emits").to_string())
        .collect();
    assert!(
        required.contains(&"export_done".to_string()) && required.contains(&"reject".to_string()),
        "the hive pairs the export with its completion word and a refusal; got {required:?}"
    );
    assert_eq!(
        matching(MEMBER, "./memory-hive", ".", "'dump'").len(),
        1,
        "`dump` leaves the hive exactly once"
    );
    assert_eq!(
        matching(MEMBER, "./memory-hive", ".", "'export_done'").len(),
        1,
        "and so does the completion word the hive says for itself (GH #555)"
    );
    assert_eq!(
        matching(MEMBER, "./memory-hive", ".", "'reject'").len(),
        1,
        "the hive's refusals still leave the level"
    );
}

/// The completion word is a lane of the level, with an edge behind it — one per
/// holder since GH #555, because the holder that finished says so itself.
#[test]
fn the_level_says_export_done_and_nothing_inside_reads_it() {
    assert!(
        lanes(MEMBER, "emits").contains(&"export_done".to_string()),
        "the level declares the completion word"
    );
    for holder in ["./memory-hive", "./affinity", "./firewall", "./assistants"] {
        assert_eq!(
            matching(MEMBER, holder, ".", "'export_done'").len(),
            1,
            "{holder} says `export_done` for itself and the level carries it out"
        );
    }
    assert!(
        !std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../templates/member/export-sink"
        ))
        .exists(),
        "the level still owns a cell that writes somebody else's files (GH #555)"
    );
}

// -------------------------------------------------------------- the fence

/// The sink used to be the boundary; since GH #555 the boundary is a `params`
/// declaration of the store that writes. It is asserted here for the same
/// reason the sandbox write root was: a level that says an export lands
/// somewhere owes a statement about WHERE, and one that says it in prose only
/// says it nowhere the substrate reads.
#[test]
fn every_holder_that_exports_declares_the_fence_it_writes_inside() {
    for (hive, cell) in [
        ("memory-hive", "store"),
        ("affinity", "store"),
        ("firewall", "rules"),
    ] {
        let c = config(&format!("templates/{hive}/{cell}/config.json"));
        let base = c["params"]["transfer"]["base_path"]
            .as_str()
            .unwrap_or_else(|| panic!("{hive}/{cell} declares no params.transfer.base_path"));
        assert!(
            std::path::Path::new(base).is_absolute(),
            "{hive}/{cell}: the fence must be absolute, got {base:?} — a relative \
             one is a loud boot error, which is the point"
        );
    }
}

// ------------------------------------------------------- the levels above

/// The two levels above are transit and nothing else: a lane crosses, no cell
/// is added, no container fills.
#[test]
fn org_and_the_shell_carry_the_new_lanes_and_add_nothing() {
    assert!(
        lanes(ORG, "accepts").contains(&"in_export".to_string()),
        "org accepts in_export"
    );
    assert_eq!(matching(ORG, ".", "./members", "'in_export'").len(), 1);
    for lane in ["close_report", "export_done"] {
        assert!(
            lanes(ORG, "emits").contains(&lane.to_string()),
            "org emits {lane}"
        );
        assert_eq!(
            matching(ORG, "./members", ".", &format!("'{lane}'")).len(),
            1,
            "org carries {lane} out"
        );
    }
    assert!(
        config(ORG)["params"].get("ports").is_none(),
        "org stays an open level -- adding a lane must not seal it"
    );

    // The shell takes the receipt out; `export_done` is consumed INSIDE it by
    // the front door that asked for the export, which is why it is not a lane
    // of this level. A shell that re-declared it would offer an exit no message
    // ever takes.
    assert!(
        lanes(SHELL, "emits").contains(&"close_report".to_string()),
        "the shell carries the close-pass receipt out"
    );
    assert_eq!(matching(SHELL, "./orgs", ".", "'close_report'").len(), 1);
    // The shell reaches the org's `in_export` from inside: what an operator
    // addresses it on is the front door's business and is deliberately not
    // asserted here, but the lane it lands on below is this change's.
    let down: Vec<Value> = edges(SHELL)
        .into_iter()
        .filter(|e| e["to"] == "./orgs" && e["modifier"]["set_hop"]["route"] == "'in_export'")
        .collect();
    assert_eq!(
        down.len(),
        1,
        "exactly one edge inside the shell turns an operator's demand into the \
         org's in_export; found {down:?}"
    );
    assert_eq!(
        matching(SHELL, "./orgs", "./operator", "'export_done'").len(),
        1,
        "the completion word is consumed INSIDE the shell, by the front door that \
         asked -- which is why it is not a lane of this level"
    );
}

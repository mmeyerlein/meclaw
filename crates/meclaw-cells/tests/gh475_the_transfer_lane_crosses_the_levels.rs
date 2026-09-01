//! GH #475 — the four levels between a member and the ledger it cannot
//! recompute, read off the shipped files.
//!
//! Since GH #471 a member's `in_export` fans out to `memory-hive`, `affinity`
//! and `firewall`, and each answers with its part. `session-keeper` answers the
//! same lane — proven hive to hive in
//! `gh471_a_keeper_carries_its_sessions.rs` — but it stands inside a
//! GENERATION, four levels down, and neither `assistant` nor `talky` forwarded a
//! transfer lane. So the sessions, the one table that remembers which
//! conversation belongs to which channel, stayed behind on every rebuild, and a
//! member reborn from its own export greeted a person it had been talking to for
//! a year as a stranger. Nothing anywhere reported it: opening a generation is a
//! perfectly ordinary event.
//!
//! What is asserted here is the SHAPE of the shipped files, which no colony test
//! reads. The behaviour is driven end to end in
//! `gh475_a_member_reaches_the_keeper_it_holds.rs`.
//!
//! Four properties, and each of them is a decision somebody could undo by
//! accident:
//!
//! 1. **The lane is declared at every level it crosses.** A lane a level does
//!    not declare is a message that dies as `no_route` at that boundary, and the
//!    two levels in between are pure transit — they own no store the lane could
//!    stop at.
//! 2. **Every door is PLAIN.** `in_export` and `in_import` are named the same on
//!    both sides of every one of these boundaries, so a `set_hop` would rename a
//!    lane onto itself — and it would hide the pairing from the drain probe,
//!    which runs the described hop through the real edge evaluator.
//! 3. **The `dump` drain is plain too, and for the same probe.** An edge that
//!    additionally tested `hop.dump_kind` evaluates false under the probe and
//!    reads as no drain at all, so the mutation that wires the ingress is
//!    refused. That is a rule with a measurement behind it, not a style.
//! 4. **The member NAMES the generation instead of fanning out to it.** The
//!    fourth export target is guarded on `context.assistant`. Two measurable
//!    reasons: a member with two generations holds two ledgers and they are not
//!    one document, and the export sink files a part under the hive it came out
//!    of, so two keepers would both claim `<export_dir>/session-keeper/` and the
//!    directory would keep whichever walk finished last. The guard is also what
//!    keeps an ordinary export of a member with no generation from dead-lettering
//!    into an empty container.
//!
//! Guarded like every template-reading test (GH #49): a tree that does not carry
//! the library is skipped, never judged.

use meclaw_core::serde_json::{Value, from_str};

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The shipped config at `rel`, or `None` when this checkout has no library.
fn shipped(rel: &str) -> Option<Value> {
    let text = std::fs::read_to_string(repo(rel)).ok()?;
    Some(from_str(&text).expect("a shipped config is json"))
}

/// The routes one side of a contract declares, in file order.
fn routes(config: &Value, side: &str) -> Vec<String> {
    config["params"]["contract"][side]
        .as_array()
        .map(|lanes| {
            lanes
                .iter()
                .filter_map(|l| l["route"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn edges(config: &Value) -> Vec<Value> {
    config["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The edges of `config` that carry `route`, from `from` to `to`.
fn carrying(config: &Value, from: &str, to: &str, route: &str) -> Vec<Value> {
    let needle = format!("hop.route == '{route}'");
    edges(config)
        .into_iter()
        .filter(|e| {
            e["from"] == from
                && e["to"] == to
                && e["condition"]
                    .as_str()
                    .unwrap_or_default()
                    .contains(needle.as_str())
        })
        .collect()
}

/// The three levels the lane crosses, from the outside in: the level's config,
/// the endpoint it hands the lane to, the name it is known by in prose, and
/// whether the level is pure TRANSIT.
///
/// The two transit levels own no store the lane could stop at, so their doors
/// carry nothing at all. The keeper's own door is the one that may carry
/// something, and does: it stamps `context.port_phase`, which is how the porter
/// tells the two legs of its own walk apart. What NO door may do is rename the
/// lane — see the assertion.
const CHAIN: [(&str, &str, &str, bool); 3] = [
    (
        "templates/assistant/config.json",
        "./talky",
        "the generation",
        true,
    ),
    (
        "templates/talky/config.json",
        "./session-keeper",
        "the conversation surface",
        true,
    ),
    (
        "templates/session-keeper/config.json",
        "./porter",
        "the keeper itself",
        false,
    ),
];

/// (1) + (2) — every level in the chain declares both lanes and `dump`, and
/// every door that carries one is unmodified.
#[test]
fn every_level_between_the_member_and_the_keeper_declares_the_transfer_lanes() {
    for (rel, endpoint, what, transit) in CHAIN {
        let Some(config) = shipped(rel) else { return };
        let accepts = routes(&config, "accepts");
        let emits = routes(&config, "emits");

        for lane in ["in_export", "in_import"] {
            assert!(
                accepts.iter().any(|r| r == lane),
                "{what} ({rel}) does not accept `{lane}`. A lane a level does not \
                 declare is a message that dies as `no_route` at its boundary, one \
                 hop before the hive that would have answered it: {accepts:?}"
            );
            let doors = carrying(&config, ".", endpoint, lane);
            assert_eq!(
                doors.len(),
                1,
                "{what} ({rel}) owes `{lane}` exactly one door onto {endpoint}; a \
                 declared lane with no door is `hive_contract` at the next mutation \
                 of the colony, and two doors are two deliveries of one part: \
                 {doors:?}"
            );
            assert!(
                doors[0]["modifier"]["set_hop"]["route"].is_null(),
                "the `{lane}` door of {what} ({rel}) RENAMES the lane. It is named \
                 the same on both sides of this boundary, so a `set_hop` here \
                 renames a lane onto itself — and hides the pairing from the drain \
                 probe, which puts the described hop through the real edge \
                 evaluator: {:?}",
                doors[0]["modifier"]
            );
            assert!(
                !transit || doors[0]["modifier"].is_null(),
                "the `{lane}` door of {what} ({rel}) carries a modifier at all. This \
                 level is pure transit: it owns no store the lane could stop at and \
                 nothing here reads a part, so a key stamped here would be a key \
                 this level invented about somebody else's document: {:?}",
                doors[0]["modifier"]
            );
        }

        assert!(
            emits.iter().any(|r| r == "dump"),
            "{what} ({rel}) does not emit `dump`. The parts a walk produces have to \
             leave every level between the keeper and whoever asked, or the export \
             runs, reads the whole ledger and reaches nobody: {emits:?}"
        );
        let exits = carrying(&config, endpoint, ".", "dump");
        assert_eq!(
            exits.len(),
            1,
            "{what} ({rel}) declares `dump` and no single edge carries it out of \
             {endpoint}: {exits:?}"
        );
        let condition = exits[0]["condition"].as_str().unwrap_or_default();
        assert!(
            !condition.contains("dump_kind"),
            "the `dump` exit of {what} ({rel}) additionally tests `hop.dump_kind`. \
             The drain probe describes a hop carrying `hop.route` and nothing else, \
             so such an edge evaluates FALSE under it and reads as no drain at \
             all — and the mutation that wires the ingress is refused with \
             `required_drain_missing`: {condition}"
        );
    }
}

/// (3) — the two transit levels pair what they forward, which is what makes a
/// caller that wires the ingress without the drain a refused mutation rather
/// than a silent export into nothing.
#[test]
fn the_two_transit_levels_pair_the_lanes_they_forward() {
    for rel in [
        "templates/assistant/config.json",
        "templates/talky/config.json",
    ] {
        let Some(config) = shipped(rel) else { return };
        let pairs: Vec<(String, String)> = config["params"]["required_drains"]
            .as_array()
            .map(|d| {
                d.iter()
                    .filter_map(|p| {
                        Some((
                            p["accepts"].as_str()?.to_string(),
                            p["emits"].as_str()?.to_string(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        for lane in ["in_export", "in_import"] {
            assert!(
                pairs.iter().any(|(a, e)| a == lane && e == "dump"),
                "{rel} forwards `{lane}` and does not pair it with `dump`. The hive \
                 two levels down pairs both of its transfer lanes with both of their \
                 exits, and a level that forwards a paired lane without carrying the \
                 pairing lets a caller wire an export whose parts have nowhere to \
                 go: {pairs:?}"
            );
        }
    }
}

/// (4) — the member's fourth export target is NAMED, and its import door reads
/// the holder off the hop the way the other two do.
#[test]
fn the_member_names_the_generation_whose_ledger_it_wants() {
    let Some(member) = shipped("templates/member/config.json") else {
        return;
    };

    let fan_out = carrying(&member, ".", "./assistants", "in_export");
    assert_eq!(
        fan_out.len(),
        1,
        "the member owes `in_export` exactly one edge into ./assistants, the \
         container that holds its generations: {fan_out:?}"
    );
    let condition = fan_out[0]["condition"].as_str().unwrap_or_default();
    assert!(
        condition.contains("has(context.assistant)")
            && condition.contains("context.assistant != ''"),
        "the fourth export target is UNGUARDED. Two things break at once: a member \
         with two generations holds two session ledgers and they are not one \
         document — and the export sink files a part under the hive it came out of, \
         so both keepers would claim `<export_dir>/session-keeper/` and the \
         directory would hold whichever walk finished last, silently. The guard is \
         also what keeps an ordinary export of a member with no generation at all \
         from dead-lettering into an empty container: {condition}"
    );
    assert!(
        fan_out[0]["default"].as_bool() != Some(true),
        "the fourth target is a guarded DEFAULT. A default fires when no regular \
         edge decided, which for `in_export` is never — the three holders are \
         regular edges and all three fire: {:?}",
        fan_out[0]
    );

    let import = carrying(&member, ".", "./assistants", "in_import");
    assert_eq!(
        import.len(),
        1,
        "the member owes `in_import` exactly one edge into ./assistants: {import:?}"
    );
    let condition = import[0]["condition"].as_str().unwrap_or_default();
    assert!(
        condition.contains("hop.import_hive == 'session-keeper'"),
        "the import door reads the holder off the BODY. A body is model-writable \
         and an edge is not, which is why the other two holders are named on \
         `hop.import_hive` and this one has to be as well — and the name is the \
         hive's (`session-keeper`), not the endpoint's, because the sink files a \
         part under the hive it came out of: {condition}"
    );
    assert!(
        import[0]["modifier"].is_null(),
        "the import door carries a modifier: {:?}",
        import[0]["modifier"]
    );

    let sink = carrying(&member, "./assistants", "./export-sink", "dump");
    assert_eq!(
        sink.len(),
        1,
        "the parts a generation's keeper walks out have no drain at this level. \
         Undrained, an export of the fourth holder runs and reaches nobody — and \
         the pairing every level between here and the keeper declares makes the \
         instantiating mutation refuse rather than the message vanish: {sink:?}"
    );
    let condition = sink[0]["condition"].as_str().unwrap_or_default();
    assert!(
        !condition.contains("dump_kind"),
        "the sink edge additionally tests `hop.dump_kind` and therefore reads as no \
         drain under the probe: {condition}"
    );
}

/// The keeper's document is filed under a name, and that name is what the sink
/// turns into a directory. It is written down in three places — the porter that
/// stamps it, the member's prose and the example that reads the directory back —
/// and a mismatch between any two of them is a document filed where nobody looks.
#[test]
fn the_name_the_keeper_stamps_is_the_name_the_import_door_and_the_example_use() {
    let Some(member) = shipped("templates/member/config.json") else {
        return;
    };
    let Ok(porter) = std::fs::read_to_string(repo("templates/session-keeper/porter/config.json"))
    else {
        return;
    };
    assert!(
        porter.contains(r#"HIVE = \"session-keeper\""#),
        "the keeper's porter no longer stamps `session-keeper` as its hive. That \
         string is the directory the member's export sink files its parts under and \
         the word `hop.import_hive` carries on the way back; changing it in one \
         place files a document where nothing looks for it"
    );

    let import = carrying(&member, ".", "./assistants", "in_import");
    assert!(
        import[0]["condition"]
            .as_str()
            .unwrap_or_default()
            .contains("'session-keeper'")
    );

    let Ok(example) = std::fs::read_to_string(repo("examples/memory-import/build_import.py"))
    else {
        return;
    };
    assert!(
        example.contains("\"session-keeper\": \"assistant\""),
        "`examples/memory-import/build_import.py` no longer knows the keeper as a \
         hive it cannot seed at birth but CAN carry in after boot. Without that row \
         a session document is silently dropped from the transfer, which reads \
         exactly like one that was never exported"
    );
}

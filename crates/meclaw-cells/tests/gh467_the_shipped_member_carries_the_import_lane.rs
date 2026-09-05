//! GH #467 — the import door is on the SHIPPED level, not on a derived copy.
//!
//! `memory-hive` has accepted `in_import` since 2.2.0, and for one release the
//! only way through the member was a patch: `examples/memory-import/` rewrote
//! the level's `config.json` on its way out, so a member grown the ordinary way
//! had no second step at all. The lane now ships (`member@1.5.0`), and the
//! example copies it like every other line of the level.
//!
//! This file is the drift lock that ruling owes (`docs/development-rules.md`
//! § 2d): the public template surface states a countable promise — *seven
//! inbound lanes and eleven outbound ones* — and this test greps that sentence
//! AND derives both numbers from the contract the substrate reads. Grepping the
//! sentence alone pins a string; asserting the mechanism alone lets the prose
//! drift away from it.
//!
//! The behaviour behind the lane is driven end to end in
//! `gh467_a_member_is_born_with_its_history.rs`; what is asserted here is the
//! shape of the shipped file, which no colony test reads.
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

/// The lanes of a level's RIM, in the order the contract list declares them.
///
/// GH #559 / #562 (ADR-0020): an entry that carries `at` names connect points
/// BELOW the hive path, so it is not a lane addressed at the level's own path
/// and is not counted where the prose says it is. `member@1.5.0` declares two
/// such — `recall` and `in_bundle`, both ends of that road inside the level —
/// and they exist to make this level a mandatory hop, not to be sent here.
fn routes(contract: &Value, side: &str) -> Vec<String> {
    contract[side]
        .as_array()
        .map(|lanes| {
            lanes
                .iter()
                .filter(|l| {
                    l.get("at")
                        .and_then(|a| a.as_array())
                        .is_none_or(|a| a.is_empty())
                })
                .filter_map(|l| l["route"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The English number word for a small count — the spelling template prose uses.
fn word(n: usize) -> &'static str {
    const WORDS: [&str; 21] = [
        "zero",
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
        "nineteen",
        "twenty",
    ];
    WORDS.get(n).copied().unwrap_or("out of range")
}

/// The door itself: one accepted lane, one plain edge, and a hive that takes it.
///
/// The edge is deliberately unmodified, exactly as `in_export`'s is. `in_import`
/// is named the same on both sides of the boundary, so a `set_hop` here would be
/// a rename that means nothing — and `memory-hive` pairs its ingress lanes with
/// drains the level above has to take, which is why the receipt lane is asserted
/// beside the door rather than assumed.
#[test]
fn the_shipped_member_declares_the_import_lane_and_the_edge_that_serves_it() {
    let (Some(member), Some(hive)) = (
        shipped("templates/member/config.json"),
        shipped("templates/memory-hive/config.json"),
    ) else {
        return;
    };

    let accepts = routes(&member["params"]["contract"], "accepts");
    assert!(
        accepts.iter().any(|r| r == "in_import"),
        "the shipped member does not accept `in_import`. Without it the second \
         step of `examples/memory-import/` exists only for a derived template, \
         and a member grown the ordinary way can never be told anything its \
         birth seed did not carry: {accepts:?}"
    );

    let edges = member["params"]["graph"]["edges"]
        .as_array()
        .expect("the member declares edges");
    let doors: Vec<&Value> = edges
        .iter()
        .filter(|e| {
            e["from"] == "."
                && e["condition"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("'in_import'")
        })
        .collect();
    // FOUR doors since GH #475, and exactly one of them can fire per message:
    // three regular edges guarded on `hop.import_hive`, plus the memory hive as
    // the guarded default (GH #283), which the router evaluates only when no
    // regular edge decided. Two doors that could both fire would be two
    // deliveries of one part; two doors where one is the default are the one
    // shape the substrate has for "and everything else".
    assert_eq!(
        doors.len(),
        4,
        "the member fans `in_import` to the three holders it owns and to the \
         container that holds its generations, and a declared lane with no door \
         is `hive_contract` at the next mutation of the colony; got {doors:?}"
    );
    let guarded: Vec<&&Value> = doors
        .iter()
        .filter(|e| !e["default"].as_bool().unwrap_or(false))
        .collect();
    let defaults: Vec<&&Value> = doors
        .iter()
        .filter(|e| e["default"].as_bool().unwrap_or(false))
        .collect();
    assert_eq!(
        defaults.len(),
        1,
        "exactly one of the four is the default; more than one would make the \
         holder a part lands in depend on edge order: {defaults:?}"
    );
    assert_eq!(
        defaults[0]["to"], "./memory-hive",
        "a part that names no holder is a part from before GH #471, and the \
         memory hive is the only place it could have come from"
    );
    let mut named: Vec<(String, String)> = guarded
        .iter()
        .map(|e| {
            (
                e["to"].as_str().unwrap_or_default().to_string(),
                e["condition"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    named.sort();
    assert_eq!(
        named.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(),
        vec!["./affinity", "./assistants", "./firewall"],
        "the three holders a part can NAME are the two whose stores write their \
         documents beside the memory's, and — since GH #475 — the container \
         through which a part reaches the session keeper of one generation: \
         {named:?}"
    );
    // The name in the hop is the HIVE the part came out of, which is the same
    // word its directory carries. For two of the three that word is
    // also the endpoint; for the keeper it is not, because the hive stands four
    // levels below the endpoint the member can address.
    for (to, cond) in &named {
        let hive = match to.as_str() {
            "./assistants" => "session-keeper",
            other => other.trim_start_matches("./"),
        };
        assert!(
            cond.contains(&format!("hop.import_hive == '{hive}'")),
            "the door for {to} reads the holder off the hop, because a body is \
             model-writable and an edge is not; got {cond}"
        );
    }
    for door in &doors {
        assert!(
            door["modifier"].is_null(),
            "every door is plain: `in_import` is the name on both sides of this \
             boundary, so a modifier here would rename a lane onto itself and \
             hide the pairing from the drain probe; got {:?}",
            door["modifier"]
        );
    }

    let hive_accepts = routes(&hive["params"]["contract"], "accepts");
    assert!(
        hive_accepts.iter().any(|r| r == "in_import"),
        "the member sends `in_import` into `./memory-hive`, which accepts \
         {hive_accepts:?} — a door onto a lane the hive does not declare is a \
         message that dies as `no_route` one hop after it crossed the level"
    );

    // The receipt half the lane's own `because` promises, and since GH #555 it
    // LEAVES the level. Until then it ended inside the member, in a cell that
    // read it and said nothing about it — the one arrangement GH #284 forbids,
    // and it only ever ended there because that cell existed for the EXPORT
    // half. The export writes its own files now, so the receipt travels the way
    // every other receipt of this level does.
    assert!(
        edges.iter().any(|e| {
            e["from"] == "./memory-hive"
                && e["to"] == "."
                && e["condition"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("'dump'")
        }),
        "the `dump` exit is gone, and with it the only way an import receipt has \
         out of this level"
    );
    assert!(
        routes(&member["params"]["contract"], "emits")
            .iter()
            .any(|r| r == "dump"),
        "the member carries `dump` out of its holders and does not declare it"
    );
}

/// The drift lock proper: the countable promise on the public surface, checked
/// against the contract the substrate reads.
///
/// The two numbers used to be hand-kept, and both were wrong at once — the level
/// gained `pack_ack` outward and `in_import` inward while the sentence still
/// said *six and ten*. A number in template prose is either derived from the
/// code inside the test or it stands exactly once (§ 2d); this derives it.
#[test]
fn the_levels_own_description_counts_the_lanes_it_declares() {
    let Some(member) = shipped("templates/member/config.json") else {
        return;
    };
    let contract = &member["params"]["contract"];
    let inbound = routes(contract, "accepts").len();
    let outbound = routes(contract, "emits").len();

    let use_when = member["description"]["use_when"]
        .as_str()
        .expect("the member's description says when to instantiate it");
    let sentence = format!(
        "wire its {} inbound lanes and its {} outbound ones in the same mutation",
        word(inbound),
        word(outbound)
    );
    assert!(
        use_when.contains(&sentence),
        "the member declares {inbound} accepts and {outbound} emits, so its own \
         `description.use_when` must say {sentence:?}. A count in template prose \
         that nothing derives is the class W5 measured: the sentence outlives \
         the mechanism and no test is red, because no test ever reads it.\n\
         use_when: {use_when}"
    );

    // The library entry says the same thing in its own words, and the two used to
    // drift apart one at a time. Same derivation, second surface.
    let Some(meta) = shipped("templates/member/template.json") else {
        return;
    };
    let meta_use_when = meta["description"]["use_when"]
        .as_str()
        .expect("the library entry says when to instantiate it");
    let meta_sentence = format!(
        "wire its {} inbound and {} outbound lanes in the same mutation",
        word(inbound),
        word(outbound)
    );
    assert!(
        meta_use_when.contains(&meta_sentence),
        "`templates/member/template.json` must say {meta_sentence:?} — it is the \
         entry a builder reads before it draws a single edge, and a wrong count \
         there is a mutation that leaves a lane unwired.\nuse_when: {meta_use_when}"
    );

    let entry = meta["description"]["examples"][0]
        .as_str()
        .expect("the first example is the PORTS block");
    assert!(
        entry.contains(&format!(
            "Entry lanes, all {} addressed at the member path itself",
            word(inbound)
        )) && entry.contains(&format!(
            "Exits, {} lanes, all leaving the member path",
            word(outbound)
        )),
        "the PORTS block of `templates/member/template.json` counts the lanes a \
         third time and must count {inbound} in and {outbound} out:\n{entry}"
    );
}

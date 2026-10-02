//! GH #951: a thing a conversation keeps naming becomes a row of the objects
//! hive (O.2/O.3/O.5). The shipped `templates/objects` in one process
//! (`support/objects_hive.rs`: the cells' own scripts, the hive's own edges,
//! the store behind its own dispatcher, every emission held to its cell's
//! `contract.emits`).
//!
//! 1. **Three turns make a row active**, append-only: every change is a new
//!    rev that supersedes the one before.
//! 2. **A turn counts once**, however often it names the thing.
//! 3. **A round is never widened**: a sighting from a round the row does not
//!    cover starts a candidate row of its own; one without a round writes
//!    nothing.
//! 4. **Only the owner confirms**, at once: an agent of the round, a turn
//!    without a speaker, a member outside the round, a member who is not the
//!    configured owner and a bare `user_id` are all `owner_only`.
//! 5. **The normal form is the memory store's** (`normalize.rs`), so an alias
//!    and a fact subject equate the same spellings.
//! 6. **The menu and the checker hold one list.**
//! 7. **A name is never an address** (review O I-2): a name that reads as an
//!    object or file id is neither a sighting nor an alias.

#[path = "support/objects_hive.rs"]
mod objects_hive;

use meclaw_core::serde_json::json;
use objects_hive::*;

const R: &str = r#"["agent:a","member:p"]"#;
const NARROW: &str = r#"["member:p"]"#;
const WIDER: &str = r#"["agent:a","member:p","peer:q"]"#;

#[test]
fn three_turns_make_a_row_active() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    h.thing_seen(Some(R), "t1", &["Blue  Bike"]);
    let rows = h.heads();
    assert_eq!(rows.len(), 1, "{rows:?}");
    let id = rows[0]["id"].as_str().unwrap().to_string();
    assert!(is_object_id(&id), "{id}");
    assert_eq!(rows[0]["state"], json!("candidate"));
    assert_eq!(rows[0]["seen"], json!(1));
    assert_eq!(rows[0]["audience_set"], json!(R), "the canonical round");
    assert_eq!(col(&rows[0], "aliases"), json!(["Blue Bike"]));
    assert!(
        h.out.is_empty(),
        "a candidate announces nothing: {:?}",
        h.out
    );

    h.thing_seen(Some(R), "t2", &["blue bike"]);
    let row = h.head(&id);
    assert_eq!(
        (row["state"].clone(), row["seen"].clone()),
        (json!("candidate"), json!(2))
    );

    h.thing_seen(Some(R), "t3", &["BLUE BIKE"]);
    let row = h.head(&id);
    assert_eq!(row["state"], json!("active"), "promote_after = 3: {row}");
    assert_eq!(row["seen"], json!(3));
    assert_eq!(col(&row, "turns"), json!(["t1", "t2", "t3"]));
    assert_eq!(h.heads().len(), 1, "one object");
    assert_eq!(
        h.routed("source_changed").len(),
        1,
        "one announcement: the promotion"
    );
    // Append-only: every rev is still there, each superseding the one before.
    let revs: Vec<_> = h
        .query("SELECT rev, supersedes FROM objects ORDER BY rev")
        .into_iter()
        .map(|r| (r["rev"].clone(), r["supersedes"].clone()))
        .collect();
    assert_eq!(
        revs,
        vec![
            (json!(1), json!(0)),
            (json!(2), json!(1)),
            (json!(3), json!(2))
        ]
    );
    assert!(h.store_errors.is_empty(), "{:?}", h.store_errors);
}

#[test]
fn the_same_turn_counts_once() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    h.thing_seen(Some(R), "t1", &["Blue Bike"]);
    // The same turn again, and twice in one sighting: nothing changes, no rev.
    h.thing_seen(Some(R), "t1", &["blue bike", "BLUE  BIKE"]);
    assert_eq!(
        h.count("objects"),
        1,
        "no new rev for a turn already counted"
    );
    h.thing_seen(Some(R), "t2", &["Blue Bike"]);
    h.thing_seen(Some(R), "t2", &["Blue Bike"]);
    let rows = h.heads();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["seen"], json!(2), "two turns, two sightings");
    assert_eq!(rows[0]["state"], json!("candidate"));
    assert_eq!(h.count("objects"), 2);
    assert!(h.out.is_empty(), "{:?}", h.out);
}

#[test]
fn a_round_the_row_does_not_cover_starts_its_own_candidate() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    h.thing_seen(Some(R), "t1", &["Blue Bike"]);
    // A narrower round is covered: it counts on the row.
    h.thing_seen(Some(NARROW), "t2", &["blue bike"]);
    assert_eq!(h.heads().len(), 1);
    assert_eq!(h.head_in(R)["seen"], json!(2));
    // `peer:q` is not in the row's round: its own candidate, no merge.
    h.thing_seen(Some(WIDER), "t3", &["Blue Bike"]);
    assert_eq!(h.heads().len(), 2, "{:?}", h.heads());
    let first = h.head_in(R);
    let second = h.head_in(WIDER);
    assert_ne!(first["id"], second["id"]);
    assert_eq!(
        first["seen"],
        json!(2),
        "the first row did not count the wider round"
    );
    assert_eq!(first["audience_set"], json!(R), "a round is never widened");
    assert_eq!(second["state"], json!("candidate"));
    assert_eq!(second["seen"], json!(1));
    // The wider round counts on its own row from now on.
    h.thing_seen(Some(WIDER), "t4", &["blue bike"]);
    assert_eq!(h.head_in(WIDER)["seen"], json!(2));
    assert_eq!(h.head_in(R)["seen"], json!(2));

    // Without a round (absent or no list of strings) nothing is written.
    let before = h.count("objects");
    h.thing_seen(None, "t5", &["Blue Bike"]);
    h.thing_seen(Some("member:p"), "t6", &["Blue Bike"]);
    h.thing_seen(Some(""), "t7", &["Blue Bike"]);
    assert_eq!(h.count("objects"), before, "no round, no row");
    assert_eq!(
        h.stderr
            .iter()
            .filter(|l| l.contains("without a round"))
            .count(),
        3,
        "{:?}",
        h.stderr
    );
}

#[test]
fn the_owner_confirms_at_once_and_nobody_else() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    h.thing_seen(Some(R), "t1", &["Blue Bike"]);
    let id = h.head_in(R)["id"].as_str().unwrap().to_string();

    // An agent of the round, no speaker, a member outside the round.
    for speaker in [Some("agent:a"), None, Some("member:z")] {
        let (hop, v) = h.tool("object_confirm", json!({"id": id}), Some(R), speaker);
        assert_eq!(v["error"]["code"], json!("owner_only"), "{speaker:?}: {v}");
        assert_eq!(hop["error_code"], json!("owner_only"));
    }
    // A platform id and plain membership in the round are no substitute.
    let ctx = json!({"tool_caller": "talky", "audience_now": R, "audience_set": R,
                     "user_id": "member:p"});
    let (_, v) = h.tool_ctx("object_confirm", json!({"id": id}), ctx);
    assert_eq!(v["error"]["code"], json!("owner_only"), "{v}");
    let row = h.head(&id);
    assert_eq!(
        (row["rev"].clone(), row["state"].clone()),
        (json!(1), json!("candidate")),
        "a refusal writes nothing"
    );

    // The owner: active at once, announced once.
    let (hop, v) = h.tool(
        "object_confirm",
        json!({"id": id}),
        Some(R),
        Some("member:p"),
    );
    assert_eq!(v["ok"], json!(true), "{v}");
    assert!(hop.get("error_code").is_none(), "{hop}");
    assert_eq!(v["state"], json!("active"));
    let row = h.head(&id);
    assert_eq!(row["state"], json!("active"));
    assert_eq!(row["seen"], json!(1), "confirmed after one sighting");
    let sc = h.routed("source_changed");
    assert_eq!(sc.len(), 1);
    assert_eq!(
        sc[0].body["version"], v["version"],
        "the answer names the version"
    );

    // With the param `owner` set, another member of the round is no owner.
    let both = r#"["member:p","member:x"]"#;
    let mut h2 = Hive::with(&[("tools", "owner", json!("member:p"))]);
    h2.thing_seen(Some(both), "t1", &["Blue Bike"]);
    let id2 = h2.head_in(both)["id"].as_str().unwrap().to_string();
    let (_, v) = h2.tool(
        "object_confirm",
        json!({"id": id2}),
        Some(both),
        Some("member:x"),
    );
    assert_eq!(v["error"]["code"], json!("owner_only"), "{v}");
    let (_, v) = h2.tool(
        "object_confirm",
        json!({"id": id2}),
        Some(both),
        Some("member:p"),
    );
    assert_eq!(v["ok"], json!(true), "{v}");
}

#[test]
fn a_name_in_the_form_of_an_address_is_no_name() {
    if !shipped() {
        return;
    }
    // Review O I-2 (audience): memory keys an alias and a fact subject in one
    // normal form, so a "name" spelled like `ob-<12 hex>` could stand for
    // another row's id -- and that row's facts would answer this row's
    // question. Neither a sighting nor `object_set alias` takes one, in any
    // case or with an anchor.
    let mut h = Hive::new();
    for t in ["t1", "t2", "t3"] {
        h.thing_seen(
            Some(R),
            t,
            &[
                "ob-0123456789ab",
                "OB-0123456789AB",
                "fh-0123456789ab#sec:x",
                "Blue Bike",
            ],
        );
    }
    let rows = h.heads();
    assert_eq!(rows.len(), 1, "only the name became a row: {rows:?}");
    assert_eq!(col(&rows[0], "aliases"), json!(["Blue Bike"]));
    let id = rows[0]["id"].as_str().unwrap().to_string();
    for value in ["ob-00000000000a", "Ob-00000000000A", "fh-0123456789ab"] {
        let (_, v) = h.tool(
            "object_set",
            json!({"id": id, "slot": "alias", "value": value}),
            Some(R),
            Some("member:p"),
        );
        assert_eq!(v["error"]["code"], json!("bad_request"), "{value}: {v}");
    }
    assert_eq!(col(&h.head(&id), "aliases"), json!(["Blue Bike"]));
    // A name that merely starts like one is still a name.
    let (_, v) = h.tool(
        "object_set",
        json!({"id": id, "slot": "alias", "value": "ob-1 the old bike"}),
        Some(R),
        Some("member:p"),
    );
    assert_eq!(v["ok"], json!(true), "{v}");
}

#[test]
fn the_normal_form_is_the_memory_stores() {
    if !shipped() {
        return;
    }
    // The python copy of `normalize` in `./gate` against the rust function
    // itself (crates/meclaw-cells/src/store/query/normalize.rs).
    let inputs = [
        "Blue Bike",
        "  blue\t\n bike  ",
        "BLUE\u{00A0}BIKE",
        "Cafe\u{0301}",
        "CAF\u{00C9}",
        "Stra\u{00DF}e",
        "A\u{030A}ngstro\u{0308}m",
        "\u{00C7}a va",
        "x\u{3000}y\u{2003}z\u{2028}w",
        "a\u{001C}b",
        "e\u{0301}\u{0301}",
        "",
        "   ",
    ];
    let py = pure("gate", "[norm_key(x) for x in ARGS]", json!(inputs));
    for (i, x) in inputs.iter().enumerate() {
        let rust = meclaw_cells::store::query::normalize::normalize(x);
        assert_eq!(py[i], json!(rust), "norm_key({x:?})");
    }
    // The table the spec names, independent of both copies.
    for (x, want) in [
        ("Cafe\u{0301}", "caf\u{00E9}"),
        ("CAF\u{00C9}", "caf\u{00E9}"),
        ("  Blue \t Bike ", "blue bike"),
        ("BLUE\u{00A0}BIKE", "blue bike"),
    ] {
        assert_eq!(meclaw_cells::store::query::normalize::normalize(x), want);
        assert_eq!(
            pure("gate", "norm_key(ARGS)", json!(x)),
            json!(want),
            "{x:?}"
        );
    }
    // `./tools` searches with the same function, word for word.
    assert_eq!(block("gate", "norm-key"), block("tools", "norm-key"));
    assert_eq!(block("gate", "round"), block("tools", "round"));
}

#[test]
fn the_menu_and_the_checker_hold_one_list() {
    if !shipped() {
        return;
    }
    let a = pure("schemas", "OBJECT_OFFER", json!(null));
    let b = pure("tools", "OBJECT_OFFER", json!(null));
    assert_eq!(
        a, b,
        "./schemas and ./tools carry OBJECT_OFFER word for word"
    );
    let names: Vec<_> = a
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        names,
        [
            "object_find",
            "object_brief",
            "object_set",
            "object_confirm"
        ]
    );
    let mut h = Hive::new();
    h.lane(
        "in_schemas",
        json!({"cur_origin": "stale"}),
        json!({}),
        json!({"tools": ["object_set", "file_read"]}),
    );
    let m = h.routed("tool_schemas");
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].body["unknown"], json!(["file_read"]));
    assert_eq!(m[0].body["schemas"][0]["name"], json!("object_set"));
}

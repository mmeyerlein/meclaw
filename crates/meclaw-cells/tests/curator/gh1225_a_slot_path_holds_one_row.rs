//! GH #1225 (K-4, OR-KF-3) -- a slot path holds one row.
//!
//! WHY this file exists: `./intake` read `slots` in step A and wrote in step B
//! -- `update` when A had seen the path, `insert` when not. Two messages in
//! flight both read "missing" and both inserted; an `update` over such copies
//! was left out whenever the copy A happened to read already matched. Measured
//! on soul-col01: every one of 30 ledgers held copies, `tools.*` up to 56 of
//! one path, `instructions.sidecar` 12 rows in 4 versions, and `window_est`
//! counted each copy (679 448 instead of 31 994 tokens, x21). Now the ledger
//! declares the unique index `slots_path` and every slot write of the intake
//! is ONE `insert` with `replace` on its path: step A only decides WHETHER to
//! write. A pack write replaces only pack rows, so a collector row that lands
//! after A read makes it fail at the index and roll back -- the collector
//! keeps the path, and that is no error. An old ledger heals the GH #954 way:
//! the index cannot be built over its copies, every write of a path takes all
//! of them along, and the next wake builds it.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// The slot rows of `path` as `(hash, owner)`, in hash order.
fn slot_rows(h: &Hive, path: &str) -> Vec<(String, String)> {
    h.rows(&format!(
        "SELECT hash, owner FROM slots WHERE path = '{path}' ORDER BY hash"
    ))
    .into_iter()
    .map(|r| {
        (
            r[0].as_str().unwrap_or("").to_string(),
            r[1].as_str().unwrap_or("").to_string(),
        )
    })
    .collect()
}

/// The paths the ledger holds more than once.
fn duplicates(h: &Hive) -> Vec<Value> {
    h.rows("SELECT path FROM slots GROUP BY path HAVING count(*) > 1 ORDER BY path")
        .into_iter()
        .map(|r| r[0].clone())
        .collect()
}

/// The block hash of one slot leaf, as `./intake` keys it.
fn leaf_hash(path: &str, text: &str) -> String {
    sha256_hex(&canonical(&json!({"path": path, "text": text})))
}

fn slots_lane(system: Value) -> (&'static str, Value, Value, Value) {
    ("in_slots", json!({}), json!({}), json!({"system": system}))
}

fn pack_lane(system: Value) -> (&'static str, Value, Value, Value) {
    ("in_pack", json!({}), json!({}), json!({"system": system}))
}

fn has_index(h: &Hive, name: &str) -> Option<String> {
    h.rows(&format!(
        "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = '{name}'"
    ))
    .first()
    .map(|r| r[0].as_str().unwrap_or("").to_string())
}

#[test]
fn two_slot_writes_in_flight_leave_one_row() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    // A, A, B, B: both messages read "missing" before either writes.
    h.lanes_together(vec![
        slots_lane(json!({"instructions": {"sidecar": {"text": "first version"}}})),
        slots_lane(json!({"instructions": {"sidecar": {"text": "second version"}}})),
    ]);
    assert_eq!(
        slot_rows(&h, "instructions.sidecar"),
        [(
            leaf_hash("instructions.sidecar", "second version"),
            "collector".to_string()
        )],
        "one row, the one sent last: {:?}",
        h.stderr
    );
    // A pack write and a collector write in flight on one path: the collector
    // holds it whichever lands first.
    for order in [["pack", "collector"], ["collector", "pack"]] {
        let mut h = Hive::new();
        h.ledger_may_refuse = true;
        let lanes = order
            .iter()
            .map(|who| {
                let tree = json!({"identity": {"text": format!("{who} identity")}});
                if *who == "pack" {
                    pack_lane(tree)
                } else {
                    slots_lane(tree)
                }
            })
            .collect();
        h.lanes_together(lanes);
        assert_eq!(
            slot_rows(&h, "identity"),
            [(
                leaf_hash("identity", "collector identity"),
                "collector".to_string()
            )],
            "{order:?}: {:?}",
            h.stderr
        );
        assert!(
            h.stderr.iter().all(|e| !e.contains("failed")),
            "{order:?}: the collector holding the path is no failure: {:?}",
            h.stderr
        );
    }
}

#[test]
fn a_stale_pack_write_never_beats_the_collector() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    h.ledger_may_refuse = true;
    // Step A of the pack read an empty ledger; the collector's row lands
    // before its step B writes.
    let collector = leaf_hash("identity", "the collector's identity");
    h.sql_at_phase = Some((
        "done".to_string(),
        format!(
            "INSERT INTO slots (path, hash, owner, at) \
             VALUES ('identity', '{collector}', 'collector', '2026-10-15T00:00:00.000000Z')"
        ),
    ));
    h.lane(
        "in_pack",
        json!({}),
        json!({}),
        json!({"system": {"identity": {"text": "the pack's identity"},
                          "persona": {"text": "the pack's persona"}}}),
    );
    assert!(h.sql_at_phase.is_none(), "the collector's row landed");
    assert_eq!(
        slot_rows(&h, "identity"),
        [(collector, "collector".to_string())],
        "the collector keeps its path: {:?}",
        h.stderr
    );
    // The rest of the pack lands; nothing is said as a failure.
    assert_eq!(
        slot_rows(&h, "persona"),
        [(
            leaf_hash("persona", "the pack's persona"),
            "pack".to_string()
        )]
    );
    assert!(
        h.stderr.iter().all(|e| !e.contains("failed")),
        "{:?}",
        h.stderr
    );
    // The pack's own path is the pack's: a second pack replaces its row, and
    // a collector write takes the path over.
    h.lane(
        "in_pack",
        json!({}),
        json!({}),
        json!({"system": {"persona": {"text": "a newer persona"}}}),
    );
    assert_eq!(
        slot_rows(&h, "persona"),
        [(leaf_hash("persona", "a newer persona"), "pack".to_string())]
    );
    h.lane(
        "in_slots",
        json!({}),
        json!({}),
        json!({"system": {"persona": {"text": "the collector's persona"}}}),
    );
    assert_eq!(
        slot_rows(&h, "persona"),
        [(
            leaf_hash("persona", "the collector's persona"),
            "collector".to_string()
        )]
    );
    assert_eq!(duplicates(&h), Vec::<Value>::new());
}

#[test]
fn an_old_ledger_heals_its_slot_duplicates_and_the_index_follows() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let ledger = meclaw_cells::store::StoreParams::parse(&cell_config("ledger")["params"])
        .expect("the ledger params parse");
    assert!(
        ledger.indexes["slots_path"].unique,
        "the ledger declares the unique index"
    );
    // A ledger from before the index: copies of three paths, the way soul-col01
    // holds them -- four versions of one, three equal rows of another, two
    // pack rows of a third.
    h.db.execute_batch("DROP INDEX slots_path").expect("drop");
    let seeded = [
        ("instructions.sidecar", "v1", "collector"),
        ("instructions.sidecar", "v2", "collector"),
        ("instructions.sidecar", "v3", "collector"),
        ("instructions.sidecar", "v4", "collector"),
        ("tools.search", "the search tool", "collector"),
        ("tools.search", "the search tool", "collector"),
        ("tools.search", "the search tool", "collector"),
        ("persona", "old persona", "pack"),
        ("persona", "old persona", "pack"),
    ];
    for (path, text, owner) in seeded {
        h.db.execute(
            "INSERT INTO slots (path, hash, owner, at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                path,
                leaf_hash(path, text),
                owner,
                "2026-10-01T00:00:00.000000Z"
            ],
        )
        .expect("seed a copy");
    }
    // The first wake: the index is not built, said, and every other index stands.
    let err = meclaw_cells::store::ddl::apply_index_ddl(&h.db, &ledger.indexes)
        .expect_err("no unique index over copies");
    assert!(err.contains("slots_path"), "{err}");
    assert_eq!(has_index(&h, "slots_path"), None);
    for name in ledger.indexes.keys().filter(|n| *n != "slots_path") {
        assert!(has_index(&h, name).is_some(), "{name} is built");
    }
    // Every path written once more, with the text it already holds: the
    // copies go in the same step.
    h.lane(
        "in_slots",
        json!({}),
        json!({}),
        json!({"system": {"instructions": {"sidecar": {"text": "v4"}},
                          "tools": {"search": {"text": "the search tool"}}}}),
    );
    h.lane(
        "in_pack",
        json!({}),
        json!({}),
        json!({"system": {"persona": {"text": "old persona"}}}),
    );
    assert_eq!(duplicates(&h), Vec::<Value>::new(), "{:?}", h.stderr);
    assert_eq!(
        slot_rows(&h, "instructions.sidecar"),
        [(
            leaf_hash("instructions.sidecar", "v4"),
            "collector".to_string()
        )]
    );
    assert_eq!(slot_rows(&h, "tools.search").len(), 1);
    assert_eq!(slot_rows(&h, "persona").len(), 1);
    // The next wake builds it -- in `sqlite_master`, not only in a log line.
    meclaw_cells::store::ddl::apply_index_ddl(&h.db, &ledger.indexes).expect("the index follows");
    let sql = has_index(&h, "slots_path").expect("slots_path stands");
    assert!(sql.starts_with("CREATE UNIQUE INDEX"), "{sql}");
}

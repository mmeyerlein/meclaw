//! GH #1225 (A2, A4, OR-KF-3/4) -- an old ledger heals and the index follows.
//!
//! WHY this file exists: the ledger declares two new unique indexes,
//! `wall_episode` on (session_id, audience_set, episode_idx) and `pins_round`
//! on (hash, source, audience_set). A ledger from before them may hold the
//! very copies they forbid -- two turns under one episode index, one pin
//! twice -- and a unique index cannot be built over copies. The GH #954 way
//! (form of OR-KF.KD.I.2): the first wake says so and builds every other
//! index, the cell's own writes take the copies away -- `./intake` clears
//! the index of the later copies of an episode once per child and round
//! (`episode_healed`), a pin written again takes every copy of its
//! (hash, source) along -- and the next wake builds the index.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

fn has_index(h: &Hive, name: &str) -> Option<String> {
    h.rows(&format!(
        "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = '{name}'"
    ))
    .first()
    .map(|r| r[0].as_str().unwrap_or("").to_string())
}

fn ledger() -> meclaw_cells::store::StoreParams {
    meclaw_cells::store::StoreParams::parse(&cell_config("ledger")["params"])
        .expect("the ledger params parse")
}

/// The first wake over the copies: `name` is not built, said, and every
/// other index stands.
fn first_wake(h: &Hive, name: &str) {
    let ledger = ledger();
    let err = meclaw_cells::store::ddl::apply_index_ddl(&h.db, &ledger.indexes)
        .expect_err("no unique index over copies");
    assert!(err.contains(name), "{err}");
    assert_eq!(has_index(h, name), None);
    for other in ledger.indexes.keys().filter(|n| *n != name) {
        assert!(has_index(h, other).is_some(), "{other} is built");
    }
}

/// The next wake builds it -- in `sqlite_master`, not only in a log line.
fn next_wake(h: &Hive, name: &str) {
    meclaw_cells::store::ddl::apply_index_ddl(&h.db, &ledger().indexes).expect("the index follows");
    let sql = has_index(h, name).unwrap_or_else(|| panic!("{name} stands"));
    assert!(sql.starts_with("CREATE UNIQUE INDEX"), "{sql}");
}

#[test]
fn an_old_ledger_heals_its_episode_copies_and_the_index_follows() {
    if !shipped() {
        return;
    }
    assert!(ledger().indexes["wall_episode"].unique, "declared unique");
    let mut h = Hive::new();
    h.db.execute_batch("DROP INDEX wall_episode").expect("drop");
    // Two turns of the person under one index, the way a child killed between
    // two A answers left them.
    for (turn, words, hour) in [
        ("t1", "My dog is Bello.", 10),
        ("t2", "My cat is Mimi.", 11),
    ] {
        let (seq, _) = h.row(days_ago(1, hour), "s-heal", turn, "user", &user(words), 0);
        h.db.execute(
            "UPDATE wall SET episode_idx = 0 WHERE seq = ?1",
            rusqlite::params![seq],
        )
        .expect("the copy");
    }
    first_wake(&h, "wall_episode");
    // The next round of the session heals, once.
    h.curate(
        "s-heal",
        "t3",
        0,
        json!([user("And a parrot.")]),
        mode("Be brief."),
    );
    let twice = h.rows(
        "SELECT episode_idx FROM wall WHERE episode_idx IS NOT NULL \
         GROUP BY session_id, audience_set, episode_idx HAVING count(*) > 1",
    );
    assert!(twice.is_empty(), "{twice:?} {:?}", h.stderr);
    assert_eq!(
        h.rows("SELECT value FROM marks WHERE kind = 'episode_healed'"),
        [[json!("1")]],
        "said once"
    );
    next_wake(&h, "wall_episode");
}

#[test]
fn an_old_ledger_heals_its_pin_copies_and_the_index_follows() {
    if !shipped() {
        return;
    }
    assert!(ledger().indexes["pins_round"].unique, "declared unique");
    let mut h = Hive::new();
    h.lane(
        "in_pin",
        json!({}),
        json!({}),
        json!({"pins": [{"text": "keep me", "source": "probe"}]}),
    );
    h.db.execute_batch("DROP INDEX pins_round").expect("drop");
    h.db.execute_batch("INSERT INTO pins SELECT * FROM pins")
        .expect("the copy");
    assert_eq!(h.rows("SELECT 1 FROM pins").len(), 2);
    first_wake(&h, "pins_round");
    // The pin written again: its copies go in the same step.
    h.lane(
        "in_pin",
        json!({}),
        json!({}),
        json!({"pins": [{"text": "keep me", "source": "probe"}]}),
    );
    assert_eq!(h.rows("SELECT 1 FROM pins").len(), 1, "{:?}", h.stderr);
    next_wake(&h, "pins_round");
}

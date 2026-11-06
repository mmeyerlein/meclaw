//! GH #1225 (re-check I-2, I-4, I-5, M-1, M-7) -- `./intake` never writes
//! over a read that failed, heals every copy of an older ledger, and says
//! each mark in the one form every curator cell writes.
//!
//! WHY this file exists: the re-check of the intake cluster measured four
//! gaps. A refused read of a candidate's stored row (`a-cands`) became "no
//! row": the push replaced the stored row and a used `once` candidate came
//! back, its cooldown gone; a refused block of a candidate still counted as
//! `stored`. The heal of an older ledger reached only the session the person
//! was talking in -- copies in a quiet session, or a pin the model wrote
//! twice, kept the unique indexes `wall_episode`/`pins_round` from ever
//! being built. And `pending_swept`/`store_error` of `./intake` carried a
//! bare count / leg name where `./policy` writes `{keys, older_than_s}` /
//! `{step, legs}`, with a sweep default of an hour against the policy's ten
//! minutes.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const SOURCE: &str = "probe-src";

fn cand(h: &mut Hive, text: &str) {
    h.out.clear();
    h.lane(
        "in_candidate",
        json!({"audience_set": ROUND_E}),
        json!({"ack": "1"}),
        json!({"source": SOURCE, "candidates": [{"id": "c1", "text": text, "once": true}]}),
    );
}

fn ack(h: &Hive) -> Value {
    let acks = h.routed("candidate_ack");
    assert_eq!(acks.len(), 1, "one receipt: {:?}", h.stderr);
    Value::Object(acks[0].body.clone())
}

fn mark_values(h: &Hive, kind: &str) -> Vec<Value> {
    h.rows(&format!("SELECT value FROM marks WHERE kind = '{kind}'"))
        .into_iter()
        .map(|r| {
            meclaw_core::serde_json::from_str(r[0].as_str().unwrap_or("")).unwrap_or(r[0].clone())
        })
        .collect()
}

#[test]
fn a_refused_read_of_the_stored_candidate_writes_nothing() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    cand(&mut h, "the first text");
    assert_eq!(ack(&h)["stored"], json!(1));
    h.db.execute_batch(
        "UPDATE candidates SET used_at = '2026-10-01T00:00:00.000000Z', last_seq = 7",
    )
    .expect("used once");
    h.ledger_may_refuse = true;
    h.refuse_legs = vec![("a-cands".into(), "store_error".into())];
    cand(&mut h, "the first text, pushed again");
    assert!(h.refuse_legs.is_empty(), "the read failed");
    let a = ack(&h);
    assert_eq!(a["stored"], json!(0), "{a}");
    assert_eq!(
        a["refused"],
        json!([{"id": "c1", "error_code": "store_error"}]),
        "{a}"
    );
    assert_eq!(
        h.rows("SELECT used_at, last_seq FROM candidates"),
        vec![vec![json!("2026-10-01T00:00:00.000000Z"), json!(7)]],
        "the used row stands: {:?}",
        h.stderr
    );
}

#[test]
fn a_refused_block_of_a_candidate_is_no_stored_candidate() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    h.ledger_may_refuse = true;
    h.refuse_legs = vec![("bc-b*".into(), "store_error".into())];
    cand(&mut h, "a text whose block is refused");
    assert!(h.refuse_legs.is_empty(), "the block write failed");
    let a = ack(&h);
    assert_eq!(a["stored"], json!(0), "{a}");
    assert_eq!(
        a["refused"],
        json!([{"id": "c1", "error_code": "store_error"}]),
        "{a}"
    );
}

#[test]
fn copies_in_a_quiet_session_and_of_a_pin_heal_on_any_round() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    h.lane(
        "in_pin",
        json!({}),
        json!({}),
        json!({"pins": [{"text": "keep me", "source": "probe"}]}),
    );
    h.db.execute_batch("DROP INDEX wall_episode; DROP INDEX pins_round")
        .expect("drop");
    h.db.execute_batch("INSERT INTO pins SELECT * FROM pins")
        .expect("the pin copy");
    for (turn, words, hour) in [
        ("q1", "My dog is Bello.", 10),
        ("q2", "My cat is Mimi.", 11),
    ] {
        let (seq, _) = h.row(days_ago(1, hour), "s-quiet", turn, "user", &user(words), 0);
        h.db.execute(
            "UPDATE wall SET episode_idx = 0 WHERE seq = ?1",
            rusqlite::params![seq],
        )
        .expect("the copy");
    }
    // A round of ANOTHER session, and no pin written again.
    h.curate(
        "s-live",
        "t1",
        0,
        json!([user("Hello.")]),
        mode("Be brief."),
    );
    let twice = h.rows(
        "SELECT episode_idx FROM wall WHERE episode_idx IS NOT NULL \
         GROUP BY session_id, audience_set, episode_idx HAVING count(*) > 1",
    );
    assert!(twice.is_empty(), "{twice:?} {:?}", h.stderr);
    assert_eq!(h.rows("SELECT 1 FROM pins").len(), 1, "{:?}", h.stderr);
    let ledger = meclaw_cells::store::StoreParams::parse(&cell_config("ledger")["params"])
        .expect("the ledger params parse");
    meclaw_cells::store::ddl::apply_index_ddl(&h.db, &ledger.indexes).expect("the indexes follow");
    // Healed once: the next round reads no copy again.
    h.ledger_ops.clear();
    h.curate(
        "s-live",
        "t2",
        0,
        json!([user("Hello again.")]),
        mode("Be brief."),
    );
    assert_eq!(mark_values(&h, "episode_healed"), vec![json!(1)]);
}

#[test]
fn the_marks_of_intake_wear_the_one_form_of_the_curator() {
    if !shipped() {
        return;
    }
    let p = &cell_config("intake")["params"];
    assert_eq!(p["pending_sweep_s"], json!("600"));
    assert_eq!(
        cell_config("intake")["contract"]["settings"]["pending_sweep_s"]["default"],
        json!("600")
    );
    let mut h = Hive::new();
    let at = (chrono::Utc::now() - chrono::Duration::seconds(900))
        .format("%Y-%m-%dT%H:%M:%S%.6fZ")
        .to_string();
    let key = "pending:0f0e0d0c-0000-4000-8000-0000000000aa";
    h.db.execute(
        "INSERT INTO state (key, value) VALUES (?1, ?2)",
        rusqlite::params![
            key,
            json!({"lane": "in_curate", "parked_at": at}).to_string()
        ],
    )
    .expect("park");
    h.ledger_may_refuse = true;
    h.refuse_legs = vec![("a-plan".into(), "store_error".into())];
    h.curate(
        "s-form",
        "t1",
        0,
        json!([user("Hello.")]),
        mode("Be brief."),
    );
    assert_eq!(
        mark_values(&h, "pending_swept"),
        vec![json!({"keys": [key], "older_than_s": 600})],
        "{:?}",
        h.stderr
    );
    assert_eq!(
        mark_values(&h, "store_error"),
        vec![json!({"step": "a", "legs": {"a-plan": "store_error"}})],
        "{:?}",
        h.stderr
    );
}

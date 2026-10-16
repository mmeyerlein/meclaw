//! GH #1225 (K-4, OR-KF-3) -- one slot row per path, read and written.
//!
//! `slots` had no key on `path`, and two writes in flight left copies of one
//! path (the review measured 21 copies in 4 versions); every reader of the
//! policy counted each -- the window estimate, the leaves sent, `call_blocks`.
//! Every read now keeps the newest row of a path (`at`, on a tie the smallest
//! `hash`), and the policy writes its own slot in one step (`insert` with
//! `replace {path}`), never as a delete + insert pair or as an update an
//! earlier read picked.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const PATH: &str = "persona.voice";

/// A ledger from before the unique index on `slots(path)`: the copies two
/// writes in flight left stand until each path is written again (#954), and
/// the index follows on a wake without them -- so the readers meet them.
fn old_ledger(h: &Hive) {
    h.db.execute("DROP INDEX IF EXISTS slots_path", []).unwrap();
}

/// The block of version `v` of the slot, and its hash.
fn version(h: &Hive, v: usize) -> String {
    let el = json!({"path": PATH, "text": format!("voice version {v}: {}", "w".repeat(400))});
    let body = canonical(&el);
    let hash = sha256_hex(&body);
    h.db.execute(
        "INSERT INTO blocks (hash, kind, chars, body, first_seen) VALUES (?1, 'system', ?2, ?3, \
         '2026-10-15T00:00:00.000000Z')",
        rusqlite::params![hash, 400i64, body],
    )
    .unwrap();
    hash
}

/// A hive whose ledger holds `copies` slot rows of PATH, version `i % 4` each
/// (version 3 the newest by `at`); returns the call of one turn.
fn hive_with(copies: usize) -> (Hive, Msg, Vec<String>) {
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    old_ledger(&h);
    let hashes: Vec<String> = (0..4).map(|v| version(&h, v)).collect();
    for i in 0..copies {
        let v = if copies == 1 { 3 } else { i % 4 };
        h.db.execute(
            "INSERT INTO slots (path, hash, owner, at) VALUES (?1, ?2, 'pack', ?3)",
            [
                PATH.to_string(),
                hashes[v].clone(),
                format!("2026-10-15T08:00:0{v}.000000Z"),
            ],
        )
        .unwrap();
    }
    let call = h.curate("s", "t1", 0, json!([user("hello")]), mode("Be brief."));
    (h, call, hashes)
}

fn estimate_of(h: &Hive, call: &Msg) -> String {
    h.state(&format!(
        "window_est:{}",
        call.hop["curator_call"].as_str().unwrap()
    ))
}

#[test]
fn duplicate_slots_count_once() {
    let (h21, call21, hashes) = hive_with(21);
    let (h1, call1, _) = hive_with(1);
    assert_ne!(estimate_of(&h1, &call1), "", "{:?}", h1.stderr);
    assert_eq!(
        estimate_of(&h21, &call21),
        estimate_of(&h1, &call1),
        "21 copies count as one"
    );
    let system = call21.body["system"].to_string();
    assert!(system.contains("voice version 3"), "{system}");
    for v in 0..3 {
        assert!(
            !system.contains(&format!("voice version {v}")),
            "only the newest: {system}"
        );
    }
    let id = call21.hop["curator_call"].as_str().unwrap();
    for (v, hash) in hashes.iter().enumerate() {
        let n = h21.rows(&format!(
            "SELECT COUNT(*) FROM call_blocks WHERE call_id = '{id}' AND hash = '{hash}'"
        ))[0][0]
            .as_i64()
            .unwrap();
        assert_eq!(n, i64::from(v == 3), "version {v} in call_blocks");
    }
}

#[test]
fn the_summary_slot_is_written_in_one_step() {
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "keep_recent", json!(1)),
    ]);
    // Two rows of the round's summary slot, as two writes in flight left them.
    old_ledger(&h);
    let slot = summary_slot(ROUND_E);
    for at in ["2026-10-15T08:00:00.000000Z", "2026-10-15T08:00:01.000000Z"] {
        h.db.execute(
            "INSERT INTO slots (path, hash, owner, at) VALUES (?1, ?2, 'curator', ?3)",
            [slot.clone(), "0".repeat(64), at.to_string()],
        )
        .unwrap();
    }
    let base = chrono::Utc::now() - chrono::Duration::seconds(50);
    for i in 0..4i64 {
        let at = base + chrono::Duration::milliseconds(100 * i);
        h.row(
            at,
            "s",
            &format!("r{i}"),
            "user",
            &user(&"z".repeat(1000)),
            0,
        );
    }
    let pkg = json!({"input_soft": 300, "input_hard": 100000, "cost_in": 10.0,
                     "cost_cached_in": 0.0, "cache_expires_at": "2099-01-01T00:00:00Z",
                     "tokens_prompt": 5000});
    turn(&mut h, "s", "t1", &"y".repeat(1200), "a", pkg);
    h.fire(&last_add(&h));
    assert_eq!(h.summ.len(), 1, "{:?}", h.stderr);
    h.answer("Earlier, briefly.", "stop");
    let rows = h.rows(&format!("SELECT hash FROM slots WHERE path = '{slot}'"));
    assert_eq!(rows.len(), 1, "one row of the path: {rows:?}");
    assert_ne!(rows[0][0], json!("0".repeat(64)), "the new summary's leaf");
    let writes: Vec<Value> = h
        .ledger_ops
        .iter()
        .filter(|(from, a)| from.ends_with("policy") && a["table"] == "slots")
        .map(|(_, a)| a.clone())
        .filter(|a| a["operation"] != "select" && a["operation"] != "delete")
        .collect();
    assert!(!writes.is_empty());
    for w in writes {
        assert_eq!(w["operation"], "insert", "{w}");
        assert_eq!(w["replace"], json!({"path": slot}), "{w}");
    }
}

/// The order of one row per path (KD § 1 K-4, review M-2): the newest by
/// `at`; on a tie the smallest `hash`; a row without a readable `at` is the
/// oldest. Paths keep the order of their first row.
#[test]
fn a_tie_on_at_keeps_the_smallest_hash() {
    let at = "2026-10-15T08:00:00.000000Z";
    let rows = json!([
        {"path": "a", "hash": "b".repeat(64), "at": at},
        {"path": "b", "hash": "1".repeat(64), "at": ""},
        {"path": "a", "hash": "a".repeat(64), "at": at},
        {"path": "b", "hash": "2".repeat(64), "at": "2026-10-15T07:00:00.000000Z"},
        {"path": "a", "hash": "c".repeat(64), "at": at},
        {"path": "b", "hash": "0".repeat(64)},
    ]);
    let (got, err) = policy_scope(
        json!({}),
        "[[r['path'], r['hash']] for r in newest_slots(ARGS)]",
        rows,
    );
    assert!(!err.contains("Traceback"), "{err}");
    assert_eq!(
        got,
        json!([["a", "a".repeat(64)], ["b", "2".repeat(64)]]),
        "tie: smallest hash; without `at`: oldest"
    );
}

/// The round's own summary leaf laid again (phase `relay`, GH #949) is written
/// in one step too: the `y-slot` write is an `insert` with `replace {path}`
/// (KD § 1 K-4, review M-3).
#[test]
fn the_relayed_summary_slot_is_written_in_one_step() {
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "keep_recent", json!(1)),
    ]);
    let base = chrono::Utc::now() - chrono::Duration::seconds(50);
    for i in 0..4i64 {
        let at = base + chrono::Duration::milliseconds(100 * i);
        h.row(
            at,
            "s",
            &format!("r{i}"),
            "user",
            &user(&"z".repeat(1000)),
            0,
        );
    }
    let pkg = json!({"input_soft": 300, "input_hard": 100000, "cost_in": 10.0,
                     "cost_cached_in": 0.0, "cache_expires_at": "2099-01-01T00:00:00Z",
                     "tokens_prompt": 5000});
    turn(&mut h, "s", "t1", &"y".repeat(1200), "a", pkg);
    h.fire(&last_add(&h));
    assert_eq!(h.summ.len(), 1, "{:?}", h.stderr);
    h.answer("Earlier, briefly.", "stop");
    let slot = summary_slot(ROUND_E);
    // The round's plan and its leaf fell together (`drop_ops`): its next
    // call lays the leaf again from its newest `summaries` row.
    h.db.execute(
        &format!(
            "DELETE FROM state WHERE key = 'window_plan:{}'",
            round_key(ROUND_E)
        ),
        [],
    )
    .unwrap();
    h.db.execute(&format!("DELETE FROM slots WHERE path = '{slot}'"), [])
        .unwrap();
    h.ledger_ops.clear();
    let back = h.curate("s", "t2", 0, json!([user("again")]), mode("Be brief."));
    let leaf = back.body["system"]["history"]["summary"]["text"]
        .as_str()
        .unwrap_or("");
    assert!(
        leaf.ends_with("Earlier, briefly."),
        "the relay laid the leaf again: {:?} {:?}",
        back.body.get("system"),
        h.stderr
    );
    let rows = h.rows(&format!("SELECT hash FROM slots WHERE path = '{slot}'"));
    assert_eq!(rows.len(), 1, "one row of the path: {rows:?}");
    let writes: Vec<Value> = h
        .ledger_ops
        .iter()
        .filter(|(from, a)| from.ends_with("policy") && a["table"] == "slots")
        .map(|(_, a)| a.clone())
        .filter(|a| a["operation"] != "select")
        .collect();
    assert_eq!(writes.len(), 1, "relay: one write of the slot: {writes:?}");
    assert_eq!(writes[0]["operation"], "insert", "{writes:?}");
    assert_eq!(writes[0]["replace"], json!({"path": slot}), "{writes:?}");
}

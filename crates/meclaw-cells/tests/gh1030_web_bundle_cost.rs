//! GH #1030 — a bundle of child updates costs the `web` cell per op, not per op
//! times the bundle.
//!
//! Measured on a deployed village page (pool of 1 600 flat figures, one
//! viewer, `answer: errors`): 800 updates per bundle took p50 63 ms, 1 600
//! took 278 ms — twice the ops, 4.4 times the time — so 4 000 child updates/s
//! only fit a 250 ms tick at a bundle p95 of 314 ms.

#[path = "support/web_fixture.rs"]
mod web_fixture;

use std::time::Instant;

use meclaw_cells::web::{ops, render};
use meclaw_core::serde_json::{self, Value, json};
use web_fixture::{Lab, Shape, duration_ms, seed_db, update};

/// The pool of the measured village page.
const POOL: usize = 1_600;

/// A file-backed `web` database with the cell's own equipment (WAL,
/// `synchronous = NORMAL`, `persist::setup_cell_db`), seeded with `shape`.
fn file_db(dir: &std::path::Path, shape: Shape) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open(dir.join("cell.db")).expect("open");
    conn.pragma_update(None, "journal_mode", "WAL")
        .expect("wal");
    conn.pragma_update(None, "synchronous", "NORMAL")
        .expect("synchronous");
    meclaw_cells::web::db::setup_web_schema(&conn).expect("schema");
    seed_db(&conn, shape);
    conn
}

/// `n` figure updates, one per figure, the values moved by `run`.
fn bundle(n: usize, run: usize) -> Vec<Value> {
    (0..n)
        .map(|i| {
            update(
                &format!("fig-{}", (i + run * 13) % POOL),
                json!({"x": (run * 7 + i) as i64, "y": (run + i) as i64}),
            )
        })
        .collect()
}

/// What one bundle of `n` figure updates leaves in the cell's write-ahead log,
/// in frames (one frame = one page written). The log is emptied first
/// (`wal_checkpoint(TRUNCATE)` through a connection of the test's own, while
/// the cell is idle between calls), then read back after the answer.
async fn wal_frames_of_one_bundle(lab: &mut Lab, n: usize, run: usize) -> i64 {
    let db = lab.cell_db();
    let checkpoint = |mode: &str| -> (i64, i64) {
        let conn = rusqlite::Connection::open(&db).expect("open the cell's database");
        conn.query_row(&format!("PRAGMA wal_checkpoint({mode})"), [], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        })
        .expect("checkpoint")
    };
    let (busy, _) = checkpoint("TRUNCATE");
    assert_eq!(busy, 0, "the cell held its database while idle");
    let answer = lab.call(bundle(n, run)).await;
    let legs = answer["messages"]
        .as_array()
        .map(|m| m.iter().filter(|t| t["type"] == "tool_result").count())
        .unwrap_or_default();
    assert_eq!(legs, n, "every leg is answered: {answer}");
    let (_, frames) = checkpoint("PASSIVE");
    frames
}

/// The lock of #1030, a counter and not a wall clock: a bundle is written
/// once. Before, every leg was its own autocommit transaction on its own trip
/// to the blocking pool — one WAL frame (at least) per leg, and the profile on
/// the gate host (release, 1 600 updates on the pool of 1 600) split the
/// 245 ms of one bundle into 100 ms of autocommitted legs (23 ms of the same
/// legs inside one transaction), ≈ 110 ms of trips to the blocking pool and
/// 34 ms of re-render. After, the bundle's legs run in one call and one
/// transaction: the log grows by the pages the bundle touched (the figure
/// rows of the lab lie ≈ 35 to a page), so the frames per leg stay far below
/// one and do not grow with the bundle.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1030_a_bundle_is_written_once_and_not_once_per_leg() {
    let mut lab = Lab::start(Shape::with_figures(POOL)).await;
    // Warm-up: the first write pays for the statement cache.
    lab.call(bundle(10, 0)).await;
    for (run, n) in [(1usize, 200usize), (2, 400)] {
        let frames = wal_frames_of_one_bundle(&mut lab, n, run).await;
        let per_leg = frames as f64 / n as f64;
        println!("LAB bundle of {n} legs: {frames} WAL frames, {per_leg:.3} per leg");
        // Measured before: ≥ 1 frame per leg (one commit each). One
        // transaction writes the touched pages once: ≈ 0.03 per leg here.
        assert!(
            per_leg <= 0.25,
            "a bundle of {n} legs wrote {frames} WAL frames ({per_leg:.3} per leg): \
             each leg is still its own commit"
        );
    }
}

fn p50(xs: &mut [f64]) -> f64 {
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    xs[xs.len() / 2]
}

/// The profile of #1030: where the time of one bundle goes, stage by stage,
/// for 200 … 1 600 updates on the pool of 1 600 — and the same bundle through
/// a live cell with one viewer. Run alone, optimised:
/// `scripts/strand.sh test 'binary(gh1030_web_bundle_cost)' --run-ignored only --release --no-capture`.
#[ignore = "profile: run alone, optimised"]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1030_profile_of_one_bundle() {
    println!(
        "PROFILE n | apply ms | apply-in-tx ms | rerender ms | encode ms | frame KB | lab duration ms | lab wall ms"
    );
    for n in [200usize, 400, 800, 1_600] {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let conn = file_db(dir.path(), Shape::with_figures(POOL));
        let mut pages = render::materialize_all(&conn).expect("materialize");
        let (mut apply, mut apply_tx, mut rr, mut enc, mut kb) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new(), 0usize);
        for run in 0..12 {
            // The handler's way: one autocommit write per leg.
            let t0 = Instant::now();
            let mut merged = ops::Touched::default();
            for op in bundle(n, run) {
                let (outcome, touched) = ops::apply(&conn, &op);
                assert!(!outcome.is_error(), "leg refused");
                merged.slots.extend(touched.slots);
            }
            merged.slots.sort();
            merged.slots.dedup();
            let t1 = Instant::now();
            let done = render::rerender(&conn, &pages, &merged).expect("rerender");
            let t2 = Instant::now();
            let bytes: usize = done
                .frames
                .iter()
                .map(|(_, f)| serde_json::to_string(f).expect("encode").len())
                .sum();
            let t3 = Instant::now();
            pages = done.pages;
            // The same legs inside one transaction, for the commit share.
            let t4 = Instant::now();
            conn.execute_batch("BEGIN").expect("begin");
            for op in bundle(n, run + 100) {
                let _ = ops::apply(&conn, &op);
            }
            conn.execute_batch("COMMIT").expect("commit");
            let t5 = Instant::now();
            if run >= 2 {
                apply.push((t1 - t0).as_secs_f64() * 1e3);
                rr.push((t2 - t1).as_secs_f64() * 1e3);
                enc.push((t3 - t2).as_secs_f64() * 1e3);
                apply_tx.push((t5 - t4).as_secs_f64() * 1e3);
                kb = bytes / 1024;
            }
        }
        // Rerender saw the transaction's writes too; resync the pages.
        drop(conn);

        let mut lab = Lab::start(Shape::with_figures(POOL)).await;
        let _viewer = lab.viewer().await;
        let (mut dur, mut wall) = (Vec::new(), Vec::new());
        for run in 0..12 {
            let t0 = Instant::now();
            let answer = lab.call(bundle(n, run)).await;
            let w = t0.elapsed().as_secs_f64() * 1e3;
            if run >= 2 {
                dur.push(duration_ms(&answer) as f64);
                wall.push(w);
            }
        }
        println!(
            "PROFILE {n} | {:.1} | {:.1} | {:.1} | {:.1} | {kb} | {:.0} | {:.1}",
            p50(&mut apply),
            p50(&mut apply_tx),
            p50(&mut rr),
            p50(&mut enc),
            p50(&mut dur),
            p50(&mut wall),
        );
    }
}

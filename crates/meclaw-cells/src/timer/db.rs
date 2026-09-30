//! Phase-10-B: the `cell.db.schedules` table + persist helpers. Sync rusqlite,
//! called via `DbConn::call`. CHECK constraints on the cell's own table are
//! allowed (the phase-9 no-phase-jumping rule applied only to `store`'s
//! `params.schema`).

use crate::timer::schedule::{ActiveSchedule, ScheduleKind, ScheduleRow};
use chrono::{DateTime, SecondsFormat, Utc};
use meclaw_core::{Path, Uuid};
use rusqlite::Connection;

/// Idempotent DDL for the `schedules` table. Calling it repeatedly is safe
/// (`CREATE TABLE IF NOT EXISTS`). Invoked by the factory once per spawn.
///
/// GH #922: a `cell.db` created before the `catch_up` column existed gets it
/// here (`ALTER TABLE ... ADD COLUMN`, default 0 = the old behaviour), only
/// when it is missing -- the pattern of `persist::migrations` in the colony
/// crate. No row is rewritten or deleted.
pub fn setup_timer_schema(conn: &Connection) -> rusqlite::Result<()> {
    create_schedules_table(conn)?;
    let has_catch_up = conn
        .prepare("SELECT 1 FROM pragma_table_info('schedules') WHERE name = 'catch_up'")?
        .exists([])?;
    if !has_catch_up {
        conn.execute(
            "ALTER TABLE schedules ADD COLUMN catch_up INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    Ok(())
}

fn create_schedules_table(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schedules (
            schedule_id        TEXT PRIMARY KEY NOT NULL,
            schedule_name      TEXT NOT NULL,
            kind               TEXT NOT NULL CHECK(kind IN ('cron','at')),
            cron_expr          TEXT,
            at_utc             TEXT,
            emit_to            TEXT NOT NULL,
            emit_body_json     TEXT NOT NULL,
            emit_headers_json  TEXT NOT NULL,
            status             TEXT NOT NULL CHECK(status IN ('active','completed','removed')),
            iteration_n        INTEGER NOT NULL DEFAULT 0,
            created_at         TEXT NOT NULL,
            catch_up           INTEGER NOT NULL DEFAULT 0
        );",
    )
}

/// INSERT one schedule row. The caller ensures `schedule_id` is new: the
/// handler's `add` goes through `add_schedule`, which looks first (GH #690);
/// the seed and the tests insert directly. A PK violation yields a rusqlite
/// error.
pub fn insert_schedule(conn: &Connection, row: &ScheduleRow) -> rusqlite::Result<()> {
    let (kind, cron_expr, at_utc, body, hdrs) = columns_of(row);
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    conn.execute(
        "INSERT INTO schedules
           (schedule_id, schedule_name, kind, cron_expr, at_utc, emit_to,
            emit_body_json, emit_headers_json, status, iteration_n, created_at, catch_up)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        rusqlite::params![
            row.schedule_id.to_string(),
            row.schedule_name,
            kind,
            cron_expr,
            at_utc.as_deref(),
            row.emit_to.as_str(),
            body,
            hdrs,
            row.status,
            row.iteration_n as i64,
            now,
            row.catch_up
        ],
    )?;
    Ok(())
}

/// The row's schedule columns as SQLite takes them: `kind`, `cron_expr`,
/// `at_utc`, `emit_body_json`, `emit_headers_json`. Shared by the INSERT and
/// the revival UPDATE (GH #690).
///
/// GH #231: `at` is stored with millisecond precision. Second-truncation
/// silently moved a schedule up to a second EARLIER than the caller asked
/// for, which is enough to land an accepted one-shot in the past before it
/// was ever planned — the same disappearance from the other end.
fn columns_of(row: &ScheduleRow) -> (&'static str, Option<&str>, Option<String>, String, String) {
    let (kind, cron_expr, at_utc) = match &row.kind {
        ScheduleKind::Cron(s) => ("cron", Some(s.as_str()), None),
        ScheduleKind::At(t) => (
            "at",
            None,
            Some(t.to_rfc3339_opts(SecondsFormat::Millis, true)),
        ),
    };
    let body = serde_json::to_string(&row.emit_body).expect("emit_body serializable");
    let hdrs = serde_json::to_string(&serde_json::Value::Object(row.emit_headers.clone()))
        .expect("emit_headers serializable");
    (kind, cron_expr, at_utc, body, hdrs)
}

/// What an `add` did with the row it named (GH #690).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddOutcome {
    /// No row carried the id: INSERTed.
    Inserted,
    /// An `active` row carried the id with the same kind and moment: nothing
    /// changed, and the caller answers as if it had.
    Same,
    /// A `removed` row carried the id: it is `active` again, with the moment
    /// and the emission of the new order.
    Revived,
    /// A row carried the id and is not the same order: `schedule_id_exists`.
    Exists,
    /// `rearm: true` (GH #904): a row carried the id in any status and is not
    /// the identical active order -- it now holds the new order, `active`, with
    /// its `rowid` and so its place in the firing order (GH #613).
    Rearmed,
}

/// An `add` that takes a repeated order as one order (GH #690).
///
/// `schedule_id` is the PRIMARY KEY, and `mark_removed` is a soft delete
/// (no-delete), so a caller whose ids are derived from the moment -- the
/// display's compose cell orders `due:<second>` -- meets its own removed row
/// when it revisits a second, and its own active row when it orders the
/// same second again. Neither is a collision: an active row with the same
/// kind and moment is `Same` (no write), a removed row is `Revived` (UPDATE,
/// the row keeps its `rowid` and so its place in the firing order, GH #613),
/// a fresh id is `Inserted`, and only a row that is a different order --
/// active with another moment, or `completed` -- is `Exists`.
///
/// `rearm` (GH #904): a caller that keeps ONE standing order under a fixed id
/// -- the curator's cache clock, which ordered a fresh id per brain call and
/// so grew `schedules` by one row per call (PP-7) -- replaces the row in any
/// status with the new order (`Rearmed`), in place: no DELETE, the `rowid`
/// stays. Only the identical active order (same kind, moment, name, target
/// and emission) is `Same`. Without `rearm` nothing here changes (GH #690).
pub fn add_schedule(
    conn: &Connection,
    row: &ScheduleRow,
    rearm: bool,
) -> rusqlite::Result<AddOutcome> {
    let Some(cur) = load_schedule(conn, row.schedule_id)? else {
        insert_schedule(conn, row)?;
        return Ok(AddOutcome::Inserted);
    };
    let same_order = match (&cur.kind, &row.kind) {
        (ScheduleKind::Cron(a), ScheduleKind::Cron(b)) => a == b,
        (ScheduleKind::At(a), ScheduleKind::At(b)) => a == b,
        _ => false,
    };
    if rearm {
        let identical = cur.status == "active"
            && same_order
            && cur.schedule_name == row.schedule_name
            && cur.emit_to == row.emit_to
            && cur.emit_body == row.emit_body
            && cur.emit_headers == row.emit_headers
            && cur.catch_up == row.catch_up;
        if identical {
            return Ok(AddOutcome::Same);
        }
        let changed = replace_order(conn, row, false)?;
        return Ok(if changed == 1 {
            AddOutcome::Rearmed
        } else {
            AddOutcome::Exists
        });
    }
    if cur.status == "active" && same_order {
        return Ok(AddOutcome::Same);
    }
    if cur.status != "removed" {
        return Ok(AddOutcome::Exists);
    }
    let changed = replace_order(conn, row, true)?;
    Ok(if changed == 1 {
        AddOutcome::Revived
    } else {
        AddOutcome::Exists
    })
}

/// UPDATE the row `row.schedule_id` to hold `row`'s order, `active`, from
/// iteration 0 -- in place, so it keeps its `rowid` (GH #613). With
/// `only_removed` the revival takes only a `removed` row (GH #690); a rearm
/// takes any (GH #904). Returns rows_changed.
fn replace_order(
    conn: &Connection,
    row: &ScheduleRow,
    only_removed: bool,
) -> rusqlite::Result<usize> {
    let (kind, cron_expr, at_utc, body, hdrs) = columns_of(row);
    conn.execute(
        "UPDATE schedules SET
            schedule_name = ?2, kind = ?3, cron_expr = ?4, at_utc = ?5, emit_to = ?6,
            emit_body_json = ?7, emit_headers_json = ?8, status = 'active', iteration_n = 0,
            catch_up = ?10
          WHERE schedule_id = ?1 AND (?9 = 0 OR status = 'removed')",
        rusqlite::params![
            row.schedule_id.to_string(),
            row.schedule_name,
            kind,
            cron_expr,
            at_utc.as_deref(),
            row.emit_to.as_str(),
            body,
            hdrs,
            only_removed,
            row.catch_up,
        ],
    )
}

/// SELECT row by primary key. Returns `Ok(None)` when no row exists.
pub fn load_schedule(conn: &Connection, id: Uuid) -> rusqlite::Result<Option<ScheduleRow>> {
    let mut stmt = conn.prepare(
        "SELECT schedule_name, kind, cron_expr, at_utc, emit_to,
                emit_body_json, emit_headers_json, status, iteration_n, catch_up
           FROM schedules WHERE schedule_id = ?1",
    )?;
    let mut rows = stmt.query([id.to_string()])?;
    match rows.next()? {
        None => Ok(None),
        Some(r) => Ok(Some(row_from_sqlite(id, r)?)),
    }
}

/// UPDATE the carried fields of an existing row. Returns rows_changed — the
/// caller (handler) checks `== 1` for "known" vs. `== 0` for "unknown".
/// `kind` is NOT exposed as an update argument: `modify` does not switch the type
/// (cell-types.md l.425-429). A cron/at update runs through `cron_expr_new` or
/// `at_utc_new` separately — the caller ensures only the field matching the
/// existing kind is set.
pub fn modify_schedule_fields(
    conn: &Connection,
    id: Uuid,
    cron_expr_new: Option<&str>,
    name_new: Option<&str>,
    emit_to_new: Option<&str>,
    at_utc_new: Option<DateTime<Utc>>,
) -> rusqlite::Result<usize> {
    // GH #231: millisecond precision, same reason as `insert_schedule`.
    let at_s = at_utc_new.map(|t| t.to_rfc3339_opts(SecondsFormat::Millis, true));
    conn.execute(
        "UPDATE schedules SET
            schedule_name = COALESCE(?2, schedule_name),
            cron_expr     = COALESCE(?3, cron_expr),
            at_utc        = COALESCE(?4, at_utc),
            emit_to       = COALESCE(?5, emit_to)
          WHERE schedule_id = ?1",
        rusqlite::params![id.to_string(), name_new, cron_expr_new, at_s, emit_to_new],
    )
}

/// GH #922: set the row's `catch_up` flag (the `modify` op's `catch_up`).
/// The caller has checked the row is a one-shot. Returns rows_changed.
pub fn set_catch_up(conn: &Connection, id: Uuid, catch_up: bool) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE schedules SET catch_up = ?2 WHERE schedule_id = ?1",
        rusqlite::params![id.to_string(), catch_up],
    )
}

/// Status='removed'. No-delete conformant (no DELETE). Returns rows_changed.
pub fn mark_removed(conn: &Connection, id: Uuid) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE schedules SET status='removed' WHERE schedule_id = ?1 AND status='active'",
        [id.to_string()],
    )
}

/// Status='completed' (for a one-shot after firing). Returns rows_changed.
pub fn mark_completed(conn: &Connection, id: Uuid) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE schedules SET status='completed' WHERE schedule_id = ?1 AND status='active'",
        [id.to_string()],
    )
}

/// iteration_n += 1 (for a repeating schedule after firing). Returns rows_changed.
pub fn bump_iteration(conn: &Connection, id: Uuid) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE schedules SET iteration_n = iteration_n + 1
            WHERE schedule_id = ?1 AND status='active'",
        [id.to_string()],
    )
}

/// SELECT all `status='active'` rows; converts them into the I/O working copy.
/// **Filters past one-shots out** (cell-types.md § `timer`, "Past firings are
/// discarded"): `at <= now` is not taken into the I/O set (it
/// stays in the DB with status='active' — read-only relief here; a later
/// `modify`/`remove` op still addresses it by id). Only cron and future-at
/// entries land in the Vec -- plus, since GH #922, the past one-shots that
/// carry `catch_up` ([`due_for_catch_up`]); the selection is [`plan_active`].
///
/// GH #231: a dropped one-shot is logged. The spec says such a schedule is "not
/// scheduled and only logged", and the log was the missing half — the drop
/// happened without a word anywhere. This is the restart path, where dropping is
/// right; an `at` that ran out of lead time in flight is refused at the op
/// instead (`at_in_past`, see `timer::cell`), so nothing reaches here that a
/// caller is still waiting on.
///
/// GH #613: the order of the returned Vec is load-bearing — it is the order in
/// which schedules that fall on the same second fire — so the SELECT names one
/// instead of leaning on the plan SQLite happens to pick. A scan of this table
/// walks it in `rowid` order today because nothing indexes `status`, but that is
/// an accident of the current plan; an index added later would reorder the
/// result silently, and the firing order is a documented promise.
/// `ORDER BY rowid` is the schedule order: SQLite hands out `rowid` as a
/// monotonic insert counter (this table is not `WITHOUT ROWID`), and rows are
/// INSERTed in exactly the order the schedules were created, first the
/// `params.schedules` seed in config order, then every `add` op as it arrives.
/// The two columns that look like candidates are not: `created_at` is written at
/// second precision, so two schedules created in the same second tie — which is
/// the very case at hand — and `schedule_id` is a UUID v7 assigned by the
/// CALLER, so it orders by whenever the caller happened to mint it, not by when
/// the timer took the schedule on.
pub fn load_active_filter_past(
    conn: &Connection,
    now: DateTime<Utc>,
) -> rusqlite::Result<Vec<ActiveSchedule>> {
    let mut stmt = conn.prepare(
        "SELECT schedule_name, kind, cron_expr, at_utc, emit_to,
                emit_body_json, emit_headers_json, status, iteration_n, catch_up,
                schedule_id
           FROM schedules WHERE status='active' ORDER BY rowid",
    )?;
    let mut rows = Vec::new();
    let mut q = stmt.query([])?;
    while let Some(r) = q.next()? {
        let id_s: String = r.get(10)?;
        let id = Uuid::parse_str(&id_s).expect("uuid parse");
        rows.push(row_from_sqlite(id, r)?);
    }
    Ok(plan_active(&rows, now))
}

/// GH #922: the one-shots a (re)spawn takes up although their moment has
/// passed -- `active` (not fired: a strike marks a one-shot `completed`
/// before it emits), `catch_up` set, `at <= now` -- ordered by `at`, a tie
/// kept in the order the rows came in (rowid, GH #613).
///
/// The boundary is `<=`, the one [`plan_active`] draws for "past": with `<`
/// a row due exactly at the load instant would be neither planned (not
/// ahead) nor caught up, and the partition would lose it.
///
/// Each comes back planned at its own moment, so the strike the I/O task
/// pushes for it carries `scheduled_at == at` and passes the stale-strike
/// guard of GH #904 in `handle_event`; a past instant sleeps zero.
pub fn due_for_catch_up(rows: &[ScheduleRow], now: DateTime<Utc>) -> Vec<ActiveSchedule> {
    let mut due: Vec<(DateTime<Utc>, usize)> = rows
        .iter()
        .enumerate()
        .filter_map(|(i, r)| match r.kind {
            ScheduleKind::At(at) if r.catch_up && r.status == "active" && at <= now => {
                Some((at, i))
            }
            _ => None,
        })
        .collect();
    due.sort_by_key(|&(at, i)| (at, i));
    due.into_iter()
        .map(|(at, i)| ActiveSchedule {
            schedule_id: rows[i].schedule_id,
            kind: ScheduleKind::At(at),
        })
        .collect()
}

/// The I/O working copy from the active rows (in rowid order): the caught-up
/// one-shots first ([`due_for_catch_up`], GH #922), then every cron row and
/// every one-shot still ahead of `now`, in schedule order. A past one-shot
/// without `catch_up` is dropped and logged, as it always was (GH #231).
pub fn plan_active(rows: &[ScheduleRow], now: DateTime<Utc>) -> Vec<ActiveSchedule> {
    let mut out = due_for_catch_up(rows, now);
    for a in &out {
        if let ScheduleKind::At(t) = a.kind {
            tracing::info!(
                schedule_id = %a.schedule_id,
                at = %t.to_rfc3339_opts(SecondsFormat::Millis, true),
                now = %now.to_rfc3339_opts(SecondsFormat::Millis, true),
                "timer: missed one-shot with catch_up fires late"
            );
        }
    }
    for r in rows.iter().filter(|r| r.status == "active") {
        match &r.kind {
            ScheduleKind::Cron(_) => {}
            ScheduleKind::At(t) if *t > now => {}
            ScheduleKind::At(t) => {
                if !r.catch_up {
                    tracing::info!(
                        schedule_id = %r.schedule_id,
                        at = %t.to_rfc3339_opts(SecondsFormat::Millis, true),
                        now = %now.to_rfc3339_opts(SecondsFormat::Millis, true),
                        "timer: one-shot in the past is not planned (row stays active in cell.db)"
                    );
                }
                continue;
            }
        }
        out.push(ActiveSchedule {
            schedule_id: r.schedule_id,
            kind: r.kind.clone(),
        });
    }
    out
}

fn row_from_sqlite(id: Uuid, r: &rusqlite::Row<'_>) -> rusqlite::Result<ScheduleRow> {
    let kind_s: String = r.get(1)?;
    let cron_expr: Option<String> = r.get(2)?;
    let at_utc: Option<String> = r.get(3)?;
    let kind = match kind_s.as_str() {
        "cron" => ScheduleKind::Cron(cron_expr.expect("cron row has cron_expr")),
        "at" => ScheduleKind::At(
            at_utc
                .expect("at row has at_utc")
                .parse::<DateTime<Utc>>()
                .expect("at_utc parsable"),
        ),
        other => panic!("unknown kind in db: {other}"),
    };
    let emit_body: serde_json::Value =
        serde_json::from_str(&r.get::<_, String>(5)?).expect("emit_body json");
    let emit_headers: serde_json::Value =
        serde_json::from_str(&r.get::<_, String>(6)?).expect("emit_headers json");
    Ok(ScheduleRow {
        schedule_id: id,
        schedule_name: r.get(0)?,
        kind,
        emit_to: Path::new(&r.get::<_, String>(4)?),
        emit_body,
        emit_headers: emit_headers.as_object().cloned().unwrap_or_default(),
        status: r.get(7)?,
        iteration_n: r.get::<_, i64>(8)? as u64,
        catch_up: r.get(9)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timer::schedule::{ScheduleKind, ScheduleRow};
    use meclaw_core::{Path, Uuid};
    use serde_json::{Map, json};

    #[test]
    fn insert_then_load_round_trips_cron_row() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        setup_timer_schema(&conn).unwrap();
        let id = Uuid::now_v7();
        let row = ScheduleRow {
            schedule_id: id,
            schedule_name: "daily".into(),
            kind: ScheduleKind::Cron("0 0 9 * * *".into()),
            emit_to: Path::new("/x"),
            emit_body: json!({"messages": []}),
            emit_headers: Map::new(),
            status: "active".into(),
            iteration_n: 0,
            catch_up: false,
        };
        insert_schedule(&conn, &row).unwrap();
        let loaded = load_schedule(&conn, id).unwrap().expect("present");
        assert_eq!(loaded.schedule_name, "daily");
        assert!(matches!(loaded.kind, ScheduleKind::Cron(ref s) if s == "0 0 9 * * *"));
        assert_eq!(loaded.status, "active");
        assert_eq!(loaded.iteration_n, 0);
    }

    fn cron_fixture(id: Uuid, cron: &str, name: &str) -> ScheduleRow {
        ScheduleRow {
            schedule_id: id,
            schedule_name: name.into(),
            kind: ScheduleKind::Cron(cron.into()),
            emit_to: Path::new("/dst"),
            emit_body: serde_json::json!({}),
            emit_headers: serde_json::Map::new(),
            status: "active".into(),
            iteration_n: 0,
            catch_up: false,
        }
    }

    fn at_fixture(id: Uuid, at: chrono::DateTime<chrono::Utc>, name: &str) -> ScheduleRow {
        ScheduleRow {
            schedule_id: id,
            schedule_name: name.into(),
            kind: ScheduleKind::At(at),
            emit_to: Path::new("/dst"),
            emit_body: serde_json::json!({}),
            emit_headers: serde_json::Map::new(),
            status: "active".into(),
            iteration_n: 0,
            catch_up: false,
        }
    }

    #[test]
    fn load_active_filter_past_drops_once_in_past_keeps_future_once_and_cron() {
        use chrono::{TimeZone, Utc};
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        setup_timer_schema(&conn).unwrap();

        let past_at = Utc.with_ymd_and_hms(2000, 1, 1, 0, 0, 0).unwrap();
        let future = Utc.with_ymd_and_hms(2099, 1, 1, 0, 0, 0).unwrap();
        let cron_id = Uuid::now_v7();
        let past_id = Uuid::now_v7();
        let fut_id = Uuid::now_v7();
        insert_schedule(&conn, &cron_fixture(cron_id, "0 0 9 * * *", "c")).unwrap();
        insert_schedule(&conn, &at_fixture(past_id, past_at, "p")).unwrap();
        insert_schedule(&conn, &at_fixture(fut_id, future, "f")).unwrap();

        let active = load_active_filter_past(&conn, Utc::now()).unwrap();
        let ids: Vec<Uuid> = active.iter().map(|a| a.schedule_id).collect();
        assert!(ids.contains(&cron_id));
        assert!(ids.contains(&fut_id));
        assert!(
            !ids.contains(&past_id),
            "once-in-past must drop out of the I/O set"
        );
    }

    /// GH #613: the I/O working copy comes back in schedule order, and schedule
    /// order is insertion order. Three schedules created inside one second would
    /// tie on `created_at` (second precision), and their caller-minted UUID v7s
    /// are handed over in REVERSE order here — so a SELECT ordered by either
    /// column fails this outright.
    ///
    /// It does NOT go red on the unordered SELECT this replaced: with no index
    /// on `status` the scan walks `rowid` anyway. That is the point of the test.
    /// The promise "same second, schedule order" is now written down in one
    /// place (the `ORDER BY`) and checked in another, so an index added to this
    /// table later cannot quietly take the order away.
    #[test]
    fn load_active_returns_the_schedules_in_insertion_order() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        setup_timer_schema(&conn).unwrap();
        let mut ids = [Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7()];
        ids.reverse();
        for (n, id) in ids.iter().enumerate() {
            insert_schedule(&conn, &cron_fixture(*id, "0 0 9 * * *", &format!("s{n}"))).unwrap();
        }

        let active = load_active_filter_past(&conn, Utc::now()).unwrap();

        assert_eq!(
            active.iter().map(|a| a.schedule_id).collect::<Vec<_>>(),
            ids.to_vec(),
            "the working copy must follow the order the rows were INSERTed in"
        );
    }

    #[test]
    fn modify_updates_cron_keeps_type_and_emit_fields() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        setup_timer_schema(&conn).unwrap();
        let id = Uuid::now_v7();
        insert_schedule(&conn, &cron_fixture(id, "0 0 9 * * *", "old-name")).unwrap();
        modify_schedule_fields(
            &conn,
            id,
            Some("*/5 * * * * *"),
            Some("new-name"),
            None,
            None,
        )
        .unwrap();
        let r = load_schedule(&conn, id).unwrap().unwrap();
        assert!(matches!(r.kind, ScheduleKind::Cron(ref s) if s == "*/5 * * * * *"));
        assert_eq!(r.schedule_name, "new-name");
    }

    #[test]
    fn mark_removed_sets_status_no_delete() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        setup_timer_schema(&conn).unwrap();
        let id = Uuid::now_v7();
        insert_schedule(&conn, &cron_fixture(id, "0 0 9 * * *", "x")).unwrap();
        let changed = mark_removed(&conn, id).unwrap();
        assert_eq!(changed, 1);
        assert_eq!(load_schedule(&conn, id).unwrap().unwrap().status, "removed");
    }

    #[test]
    fn mark_completed_sets_status_for_once_after_fire() {
        use chrono::TimeZone;
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        setup_timer_schema(&conn).unwrap();
        let id = Uuid::now_v7();
        let mut row = cron_fixture(id, "* * * * * *", "x");
        row.kind = ScheduleKind::At(chrono::Utc.with_ymd_and_hms(2099, 1, 1, 0, 0, 0).unwrap());
        insert_schedule(&conn, &row).unwrap();
        let n = mark_completed(&conn, id).unwrap();
        assert_eq!(n, 1);
        assert_eq!(
            load_schedule(&conn, id).unwrap().unwrap().status,
            "completed"
        );
    }

    #[test]
    fn bump_iteration_increments_for_repeating_active() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        setup_timer_schema(&conn).unwrap();
        let id = Uuid::now_v7();
        insert_schedule(&conn, &cron_fixture(id, "*/1 * * * * *", "x")).unwrap();
        assert_eq!(bump_iteration(&conn, id).unwrap(), 1);
        assert_eq!(load_schedule(&conn, id).unwrap().unwrap().iteration_n, 1);
        assert_eq!(bump_iteration(&conn, id).unwrap(), 1);
        assert_eq!(load_schedule(&conn, id).unwrap().unwrap().iteration_n, 2);
    }

    /// GH #690: a repeated order is one order. The same id with the same
    /// moment on an active row changes nothing and is not an error; the same
    /// id on a removed row is revived in place (status, moment, emission --
    /// and the rowid it had, so its place in the firing order); the same id
    /// with another moment on an active row is the collision it always was.
    #[test]
    fn add_takes_a_repeated_order_as_one_order_and_revives_a_removed_one() {
        use chrono::TimeZone;
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        setup_timer_schema(&conn).unwrap();
        let id = Uuid::now_v7();
        let at = chrono::Utc.with_ymd_and_hms(2099, 1, 1, 0, 0, 30).unwrap();
        let later = chrono::Utc.with_ymd_and_hms(2099, 1, 1, 0, 0, 31).unwrap();
        let order = at_fixture(id, at, "due");
        assert_eq!(
            add_schedule(&conn, &order, false).unwrap(),
            AddOutcome::Inserted
        );

        // Same id, same moment, active: one order.
        assert_eq!(
            add_schedule(&conn, &order, false).unwrap(),
            AddOutcome::Same
        );
        let n: i64 = conn
            .query_row("SELECT count(*) FROM schedules", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "no second row");

        // Same id, another moment, active: the collision.
        let moved = at_fixture(id, later, "due");
        assert_eq!(
            add_schedule(&conn, &moved, false).unwrap(),
            AddOutcome::Exists
        );
        let kept = load_schedule(&conn, id).unwrap().unwrap();
        assert!(
            matches!(kept.kind, ScheduleKind::At(t) if t == at),
            "untouched"
        );

        // Removed, then ordered again: revived, and with the new order's
        // moment and emission, not the old row's.
        assert_eq!(mark_removed(&conn, id).unwrap(), 1);
        assert_eq!(
            add_schedule(&conn, &moved, false).unwrap(),
            AddOutcome::Revived
        );
        let back = load_schedule(&conn, id).unwrap().unwrap();
        assert_eq!(back.status, "active");
        assert!(matches!(back.kind, ScheduleKind::At(t) if t == later));
        let active = load_active_filter_past(&conn, chrono::Utc::now()).unwrap();
        assert_eq!(active.len(), 1, "planned again: {active:?}");
        assert_eq!(active[0].schedule_id, id);
        let n: i64 = conn
            .query_row("SELECT count(*) FROM schedules", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "revived in place, no DELETE, no second row");

        // A completed one-shot is not revived: it fired, and an order under
        // its id for another moment is a different order.
        assert_eq!(mark_completed(&conn, id).unwrap(), 1);
        assert_eq!(
            add_schedule(&conn, &order, false).unwrap(),
            AddOutcome::Exists
        );
        assert_eq!(
            load_schedule(&conn, id).unwrap().unwrap().status,
            "completed"
        );
    }

    fn rowid_of(conn: &Connection, id: Uuid) -> i64 {
        conn.query_row(
            "SELECT rowid FROM schedules WHERE schedule_id = ?1",
            [id.to_string()],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn count(conn: &Connection) -> i64 {
        conn.query_row("SELECT count(*) FROM schedules", [], |r| r.get(0))
            .unwrap()
    }

    /// GH #904 (PP-7): `rearm` makes one id hold one standing order. Over a
    /// `completed`, a `removed` and an active row with another moment the new
    /// order replaces the row -- moment, emission, `active`, iteration 0 -- in
    /// place (same `rowid`, so the firing order of GH #613 holds, and no
    /// second row); the identical active order is `Same` and writes nothing;
    /// a cron row is rearmed the same way (it never becomes `completed`).
    #[test]
    fn rearm_replaces_the_row_in_every_status_and_keeps_its_rowid() {
        use chrono::TimeZone;
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        setup_timer_schema(&conn).unwrap();
        // A neighbour inserted before, so the rowid under test is not 1 by accident.
        insert_schedule(&conn, &cron_fixture(Uuid::now_v7(), "0 0 9 * * *", "n")).unwrap();
        let id = Uuid::now_v7();
        let t1 = chrono::Utc.with_ymd_and_hms(2099, 1, 1, 0, 0, 30).unwrap();
        let t2 = chrono::Utc.with_ymd_and_hms(2099, 1, 1, 0, 5, 0).unwrap();
        let t3 = chrono::Utc.with_ymd_and_hms(2099, 1, 1, 0, 9, 0).unwrap();
        let mut first = at_fixture(id, t1, "cache");
        first.emit_body = serde_json::json!({"curator_call": "c1"});
        assert_eq!(
            add_schedule(&conn, &first, true).unwrap(),
            AddOutcome::Inserted
        );
        let rowid = rowid_of(&conn, id);

        // The identical active order: nothing written.
        assert_eq!(add_schedule(&conn, &first, true).unwrap(), AddOutcome::Same);

        // Active, another moment and emission: replaced (without rearm: Exists).
        let mut second = at_fixture(id, t2, "cache");
        second.emit_body = serde_json::json!({"curator_call": "c2"});
        assert_eq!(
            add_schedule(&conn, &second, false).unwrap(),
            AddOutcome::Exists
        );
        assert_eq!(
            add_schedule(&conn, &second, true).unwrap(),
            AddOutcome::Rearmed
        );
        let r = load_schedule(&conn, id).unwrap().unwrap();
        assert!(matches!(r.kind, ScheduleKind::At(t) if t == t2));
        assert_eq!(r.emit_body, serde_json::json!({"curator_call": "c2"}));
        assert_eq!(r.status, "active");

        // Active, same moment, another emission: rearm is not fooled into `Same`.
        let mut same_moment = second.clone();
        same_moment.emit_body = serde_json::json!({"curator_call": "c2b"});
        assert_eq!(
            add_schedule(&conn, &same_moment, true).unwrap(),
            AddOutcome::Rearmed
        );
        assert_eq!(
            load_schedule(&conn, id).unwrap().unwrap().emit_body,
            serde_json::json!({"curator_call": "c2b"})
        );

        // Fired (`completed`, with an iteration behind it): armed again.
        assert_eq!(bump_iteration(&conn, id).unwrap(), 1);
        assert_eq!(mark_completed(&conn, id).unwrap(), 1);
        let mut third = at_fixture(id, t3, "cache");
        third.emit_body = serde_json::json!({"curator_call": "c3"});
        assert_eq!(
            add_schedule(&conn, &third, false).unwrap(),
            AddOutcome::Exists
        );
        assert_eq!(
            add_schedule(&conn, &third, true).unwrap(),
            AddOutcome::Rearmed
        );
        let r = load_schedule(&conn, id).unwrap().unwrap();
        assert_eq!(r.status, "active");
        assert_eq!(r.iteration_n, 0);
        assert!(matches!(r.kind, ScheduleKind::At(t) if t == t3));
        let active = load_active_filter_past(&conn, chrono::Utc::now()).unwrap();
        assert!(
            active.iter().any(|a| a.schedule_id == id),
            "planned again: {active:?}"
        );

        // Removed: armed again too.
        assert_eq!(mark_removed(&conn, id).unwrap(), 1);
        assert_eq!(
            add_schedule(&conn, &first, true).unwrap(),
            AddOutcome::Rearmed
        );
        assert_eq!(load_schedule(&conn, id).unwrap().unwrap().status, "active");

        // A cron row under the id (another kind, too): replaced in place.
        let cron = cron_fixture(id, "*/5 * * * * *", "cache");
        assert_eq!(
            add_schedule(&conn, &cron, true).unwrap(),
            AddOutcome::Rearmed
        );
        let r = load_schedule(&conn, id).unwrap().unwrap();
        assert!(matches!(r.kind, ScheduleKind::Cron(ref c) if c == "*/5 * * * * *"));
        let next = cron_fixture(id, "*/7 * * * * *", "cache");
        assert_eq!(
            add_schedule(&conn, &next, true).unwrap(),
            AddOutcome::Rearmed
        );

        assert_eq!(rowid_of(&conn, id), rowid, "in place: the rowid stays");
        assert_eq!(
            count(&conn),
            2,
            "one row for the id, no second row, no DELETE"
        );
    }

    /// (label, at-or-cron, catch_up, status, expected in the result)
    type Case = (
        &'static str,
        Option<DateTime<Utc>>,
        bool,
        &'static str,
        bool,
    );

    /// GH #922: the boot's selection of missed one-shots, as a table. Taken:
    /// `active`, `at` rows, `catch_up` set, `at <= now` -- the boundary is the
    /// one `plan_active` draws for "past" (an `at == now` row is neither
    /// planned nor dropped, it is caught up), so the two can never both skip a
    /// row. Not taken: a row without the flag, a completed (fired) or removed
    /// row, a row still ahead, a cron row. Order: by `at`, a tie by the order
    /// the rows came in (= rowid, GH #613).
    #[test]
    fn due_for_catch_up_takes_only_marked_missed_active_one_shots_in_at_order() {
        use chrono::TimeZone;
        let now = chrono::Utc.with_ymd_and_hms(2030, 1, 1, 12, 0, 0).unwrap();
        let s = chrono::Duration::seconds;
        // (label, at-or-cron, catch_up, status, expected in the result)
        let table: Vec<Case> = vec![
            ("late-2", Some(now - s(2)), true, "active", true),
            ("late-9", Some(now - s(9)), true, "active", true),
            ("boundary", Some(now), true, "active", true),
            ("unflagged", Some(now - s(5)), false, "active", false),
            ("fired", Some(now - s(5)), true, "completed", false),
            ("removed", Some(now - s(5)), true, "removed", false),
            ("ahead", Some(now + s(1)), true, "active", false),
            ("cron", None, true, "active", false),
            ("tie-with-late-2", Some(now - s(2)), true, "active", true),
        ];
        let rows: Vec<ScheduleRow> = table
            .iter()
            .map(|(label, at, catch_up, status, _)| {
                let mut r = match at {
                    Some(t) => at_fixture(Uuid::now_v7(), *t, label),
                    None => cron_fixture(Uuid::now_v7(), "0 0 9 * * *", label),
                };
                r.catch_up = *catch_up;
                r.status = (*status).into();
                r
            })
            .collect();
        let got: Vec<String> = due_for_catch_up(&rows, now)
            .iter()
            .map(|a| {
                rows.iter()
                    .find(|r| r.schedule_id == a.schedule_id)
                    .unwrap()
                    .schedule_name
                    .clone()
            })
            .collect();
        assert_eq!(
            got,
            vec!["late-9", "late-2", "tie-with-late-2", "boundary"],
            "only the marked, missed, active one-shots, in `at` order"
        );
        for (label, _, _, _, expected) in &table {
            assert_eq!(got.iter().any(|g| g == label), *expected, "row {label}");
        }
        // Each is planned at its own moment, so the strike carries it as
        // `scheduled_at` and passes the stale-strike guard (GH #904).
        let first = &due_for_catch_up(&rows, now)[0];
        assert!(matches!(first.kind, ScheduleKind::At(t) if t == now - s(9)));
    }

    /// GH #922: the plan a (re)spawn and every op snapshot hand the I/O task
    /// holds the caught-up rows next to cron and future rows; a past row
    /// without the flag is still dropped, exactly as before.
    #[test]
    fn plan_active_keeps_marked_missed_rows_and_still_drops_unmarked_ones() {
        use chrono::TimeZone;
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        setup_timer_schema(&conn).unwrap();
        let past = chrono::Utc.with_ymd_and_hms(2000, 1, 1, 0, 0, 0).unwrap();
        let future = chrono::Utc.with_ymd_and_hms(2099, 1, 1, 0, 0, 0).unwrap();
        let cron_id = Uuid::now_v7();
        let marked = Uuid::now_v7();
        let unmarked = Uuid::now_v7();
        let fut_id = Uuid::now_v7();
        insert_schedule(&conn, &cron_fixture(cron_id, "0 0 9 * * *", "c")).unwrap();
        let mut m = at_fixture(marked, past, "m");
        m.catch_up = true;
        insert_schedule(&conn, &m).unwrap();
        insert_schedule(&conn, &at_fixture(unmarked, past, "u")).unwrap();
        insert_schedule(&conn, &at_fixture(fut_id, future, "f")).unwrap();

        let ids: Vec<Uuid> = load_active_filter_past(&conn, Utc::now())
            .unwrap()
            .iter()
            .map(|a| a.schedule_id)
            .collect();
        assert!(ids.contains(&cron_id));
        assert!(ids.contains(&fut_id));
        assert!(ids.contains(&marked), "a marked missed row is caught up");
        assert!(
            !ids.contains(&unmarked),
            "an unmarked missed row is dropped"
        );

        // Once it fired it is `completed` and never taken up again.
        assert_eq!(mark_completed(&conn, marked).unwrap(), 1);
        let ids: Vec<Uuid> = load_active_filter_past(&conn, Utc::now())
            .unwrap()
            .iter()
            .map(|a| a.schedule_id)
            .collect();
        assert!(!ids.contains(&marked), "fired once, taken up never again");
    }

    /// GH #922: the flag is stored per row and read back; `rearm` and a
    /// revival carry the new order's flag; `set_catch_up` flips it on a row.
    #[test]
    fn catch_up_round_trips_and_follows_rearm_and_set_catch_up() {
        use chrono::TimeZone;
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        setup_timer_schema(&conn).unwrap();
        let id = Uuid::now_v7();
        let t1 = chrono::Utc.with_ymd_and_hms(2099, 1, 1, 0, 0, 0).unwrap();
        let t2 = chrono::Utc.with_ymd_and_hms(2099, 1, 1, 0, 1, 0).unwrap();
        let mut row = at_fixture(id, t1, "x");
        row.catch_up = true;
        assert_eq!(
            add_schedule(&conn, &row, false).unwrap(),
            AddOutcome::Inserted
        );
        assert!(load_schedule(&conn, id).unwrap().unwrap().catch_up);

        // rearm with the same moment but without the flag: not `Same`.
        let mut plain = row.clone();
        plain.catch_up = false;
        assert_eq!(
            add_schedule(&conn, &plain, true).unwrap(),
            AddOutcome::Rearmed
        );
        assert!(!load_schedule(&conn, id).unwrap().unwrap().catch_up);

        assert_eq!(set_catch_up(&conn, id, true).unwrap(), 1);
        assert!(load_schedule(&conn, id).unwrap().unwrap().catch_up);
        assert_eq!(set_catch_up(&conn, Uuid::now_v7(), true).unwrap(), 0);

        // A revival takes the new order's flag.
        assert_eq!(mark_removed(&conn, id).unwrap(), 1);
        let mut revived = at_fixture(id, t2, "x");
        revived.catch_up = false;
        assert_eq!(
            add_schedule(&conn, &revived, false).unwrap(),
            AddOutcome::Revived
        );
        assert!(!load_schedule(&conn, id).unwrap().unwrap().catch_up);
    }

    /// GH #922: a `cell.db` written before the column existed keeps working --
    /// the schema setup adds `catch_up` (default 0), no row is lost, and every
    /// standing row reads as "no catch-up" (unchanged behaviour).
    #[test]
    fn setup_adds_the_catch_up_column_to_an_older_table() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE schedules (
                schedule_id        TEXT PRIMARY KEY NOT NULL,
                schedule_name      TEXT NOT NULL,
                kind               TEXT NOT NULL CHECK(kind IN ('cron','at')),
                cron_expr          TEXT,
                at_utc             TEXT,
                emit_to            TEXT NOT NULL,
                emit_body_json     TEXT NOT NULL,
                emit_headers_json  TEXT NOT NULL,
                status             TEXT NOT NULL CHECK(status IN ('active','completed','removed')),
                iteration_n        INTEGER NOT NULL DEFAULT 0,
                created_at         TEXT NOT NULL
            );",
        )
        .unwrap();
        let id = Uuid::now_v7();
        conn.execute(
            "INSERT INTO schedules (schedule_id, schedule_name, kind, at_utc, emit_to,
                 emit_body_json, emit_headers_json, status, iteration_n, created_at)
             VALUES (?1, 'old', 'at', '2000-01-01T00:00:00.000Z', '/dst', '{}', '{}',
                 'active', 0, '2000-01-01T00:00:00Z')",
            [id.to_string()],
        )
        .unwrap();

        setup_timer_schema(&conn).expect("upgrade");
        setup_timer_schema(&conn).expect("and idempotent after the upgrade");

        let r = load_schedule(&conn, id)
            .unwrap()
            .expect("the old row survives");
        assert!(!r.catch_up, "an old row is not caught up");
        assert!(
            load_active_filter_past(&conn, Utc::now())
                .unwrap()
                .is_empty(),
            "an old past row is still dropped, as before"
        );
    }

    #[test]
    fn setup_timer_schema_is_idempotent() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        setup_timer_schema(&conn).expect("first call");
        setup_timer_schema(&conn).expect("second call — must be idempotent");
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='schedules'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }
}

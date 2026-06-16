//! Phase-10-B: `cell.db.schedules`-Tabelle + Persist-Helpers. Sync rusqlite;
//! Aufruf via `DbConn::call`. CHECK-Constraints auf der cell-eigenen Tabelle
//! sind erlaubt (Phase-9-Anti-Vorgriff galt nur fuer `store`-`params.schema`).

use crate::timer::schedule::{ActiveSchedule, ScheduleKind, ScheduleRow};
use chrono::{DateTime, SecondsFormat, Utc};
use meclaw_core::{Path, Uuid};
use rusqlite::Connection;

/// Idempotente DDL fuer die `schedules`-Tabelle. Mehrfach-Aufruf ist safe
/// (`CREATE TABLE IF NOT EXISTS`). Wird in der Factory pro Spawn aufgerufen.
pub fn setup_timer_schema(conn: &Connection) -> rusqlite::Result<()> {
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
            created_at         TEXT NOT NULL
        );",
    )
}

/// INSERT eine Schedule-Row. Caller stellt sicher, dass `schedule_id` neu
/// ist (add-Dup ist Handler-Ebene). PK-Verletzung → rusqlite-Error.
pub fn insert_schedule(conn: &Connection, row: &ScheduleRow) -> rusqlite::Result<()> {
    let (kind, cron_expr, at_utc) = match &row.kind {
        ScheduleKind::Cron(s) => ("cron", Some(s.as_str()), None),
        ScheduleKind::At(t) => (
            "at",
            None,
            Some(t.to_rfc3339_opts(SecondsFormat::Secs, true)),
        ),
    };
    let body = serde_json::to_string(&row.emit_body).expect("emit_body serializable");
    let hdrs = serde_json::to_string(&serde_json::Value::Object(row.emit_headers.clone()))
        .expect("emit_headers serializable");
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    conn.execute(
        "INSERT INTO schedules
           (schedule_id, schedule_name, kind, cron_expr, at_utc, emit_to,
            emit_body_json, emit_headers_json, status, iteration_n, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
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
            now
        ],
    )?;
    Ok(())
}

/// SELECT row by primary key. Returns `Ok(None)` falls keine Row existiert.
pub fn load_schedule(conn: &Connection, id: Uuid) -> rusqlite::Result<Option<ScheduleRow>> {
    let mut stmt = conn.prepare(
        "SELECT schedule_name, kind, cron_expr, at_utc, emit_to,
                emit_body_json, emit_headers_json, status, iteration_n
           FROM schedules WHERE schedule_id = ?1",
    )?;
    let mut rows = stmt.query([id.to_string()])?;
    match rows.next()? {
        None => Ok(None),
        Some(r) => Ok(Some(row_from_sqlite(id, r)?)),
    }
}

/// UPDATE die getragenen Felder einer existierenden Row. Returns rows_changed
/// — Caller (Handler) prueft `== 1` fuer „bekannt" vs. `== 0` fuer „unknown".
/// `kind` ist NICHT als Update-Argument exposed: `modify` wechselt den Typ
/// nicht (cell-types.md Z.425–429). Cron/At-Update laeuft ueber `cron_expr_new`
/// bzw. `at_utc_new` separat — Caller stellt sicher, dass nur das jeweilige
/// Feld zum vorhandenen Kind passt.
pub fn modify_schedule_fields(
    conn: &Connection,
    id: Uuid,
    cron_expr_new: Option<&str>,
    name_new: Option<&str>,
    emit_to_new: Option<&str>,
    at_utc_new: Option<DateTime<Utc>>,
) -> rusqlite::Result<usize> {
    let at_s = at_utc_new.map(|t| t.to_rfc3339_opts(SecondsFormat::Secs, true));
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

/// Status='removed'. No-Delete-konform (kein DELETE). Returns rows_changed.
pub fn mark_removed(conn: &Connection, id: Uuid) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE schedules SET status='removed' WHERE schedule_id = ?1 AND status='active'",
        [id.to_string()],
    )
}

/// Status='completed' (fuer once nach Fire). Returns rows_changed.
pub fn mark_completed(conn: &Connection, id: Uuid) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE schedules SET status='completed' WHERE schedule_id = ?1 AND status='active'",
        [id.to_string()],
    )
}

/// iteration_n += 1 (fuer repeating nach Fire). Returns rows_changed.
pub fn bump_iteration(conn: &Connection, id: Uuid) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE schedules SET iteration_n = iteration_n + 1
            WHERE schedule_id = ?1 AND status='active'",
        [id.to_string()],
    )
}

/// SELECT aller `status='active'` Rows; konvertiert in I/O-Arbeitskopie.
/// **Filtert past-onces raus** (cell-types.md Z.431–436): `at < now` wird
/// nicht in die Io-Menge uebernommen (bleibt mit status='active' in der DB
/// liegen — Reading-only entlastet hier; eine spaetere `modify`/`remove`-Op
/// adressiert sie weiterhin per id). Nur cron + future-at landen im Vec.
pub fn load_active_filter_past(
    conn: &Connection,
    now: DateTime<Utc>,
) -> rusqlite::Result<Vec<ActiveSchedule>> {
    let mut stmt = conn.prepare(
        "SELECT schedule_id, kind, cron_expr, at_utc
           FROM schedules WHERE status='active'",
    )?;
    let mut out = Vec::new();
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let id_s: String = r.get(0)?;
        let id = Uuid::parse_str(&id_s).expect("uuid parse");
        let kind_s: String = r.get(1)?;
        let cron_expr: Option<String> = r.get(2)?;
        let at_utc: Option<String> = r.get(3)?;
        let kind = match kind_s.as_str() {
            "cron" => ScheduleKind::Cron(cron_expr.unwrap()),
            "at" => {
                let t: DateTime<Utc> = at_utc.unwrap().parse().expect("at_utc parse");
                if t <= now {
                    continue;
                }
                ScheduleKind::At(t)
            }
            _ => continue,
        };
        out.push(ActiveSchedule {
            schedule_id: id,
            kind,
        });
    }
    Ok(out)
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
            "once-in-past muss aus Io-Menge raus"
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

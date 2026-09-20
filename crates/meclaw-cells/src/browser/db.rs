//! `cell.db.pages` — what this browser was showing when it was last awake.
//!
//! The browser is a cache; this table is the state (R-G4). A page that is open
//! is a row, a page that was closed is not, and a cell that comes back reads
//! its rows and opens them again (OR-G17) rather than waiting for an app to
//! notice. There is no `gone` state for anybody to answer.
//!
//! Sync rusqlite, like every other cell-type schema module: called in the
//! factory before `DbConn::wrap` (outside the restart corridor) and from the
//! handler through `DbConn::call_with_timeout`.

use rusqlite::{Connection, params};

/// One row of `pages`.
#[derive(Debug, Clone, PartialEq)]
pub struct PageRow {
    /// The page id. The app's card id, and the topic suffix (OR-G3).
    pub page: String,
    /// The full address, fragment included (OR-G39).
    pub url: String,
    /// The browser context this page lives in — an identity, not a window.
    pub context: String,
    /// Viewport width in CSS pixels.
    pub viewport_w: u32,
    /// Viewport height in CSS pixels.
    pub viewport_h: u32,
    /// Device pixel ratio.
    pub viewport_dpr: f64,
    /// Whether the page is emulating a phone.
    pub mobile: bool,
    /// The last state this page was persisted in.
    pub state: String,
    /// Unix milliseconds of the first open.
    pub opened_at: i64,
    /// Unix milliseconds of the last change.
    pub updated_at: i64,
}

/// Idempotent DDL. Safe across restarts.
pub fn setup_browser_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS pages (
            page         TEXT PRIMARY KEY,
            url          TEXT NOT NULL,
            context      TEXT NOT NULL,
            viewport_w   INTEGER NOT NULL,
            viewport_h   INTEGER NOT NULL,
            viewport_dpr REAL NOT NULL,
            mobile       INTEGER NOT NULL,
            state        TEXT NOT NULL,
            opened_at    INTEGER NOT NULL,
            updated_at   INTEGER NOT NULL
        );",
    )
}

/// Write a page's row, or update the one that stands there.
///
/// `opened_at` survives an update: a page that navigated is the same page, and
/// the row is what a restart reopens from.
pub fn upsert_page(conn: &Connection, row: &PageRow) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO pages
            (page, url, context, viewport_w, viewport_h, viewport_dpr, mobile, state,
             opened_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(page) DO UPDATE SET
            url = ?2, context = ?3, viewport_w = ?4, viewport_h = ?5, viewport_dpr = ?6,
            mobile = ?7, state = ?8, updated_at = ?10",
        params![
            row.page,
            row.url,
            row.context,
            row.viewport_w,
            row.viewport_h,
            row.viewport_dpr,
            row.mobile as i64,
            row.state,
            row.opened_at,
            row.updated_at,
        ],
    )?;
    Ok(())
}

/// Remove a page's row. Called when a page is closed, and only then: a
/// suspended page is still a page and keeps its row.
pub fn delete_page(conn: &Connection, page: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM pages WHERE page = ?1", params![page])?;
    Ok(())
}

/// Every page this cell was holding, oldest first — what a new life reopens.
pub fn open_pages(conn: &Connection) -> rusqlite::Result<Vec<PageRow>> {
    let mut stmt = conn.prepare(
        "SELECT page, url, context, viewport_w, viewport_h, viewport_dpr, mobile, state,
                opened_at, updated_at
         FROM pages ORDER BY opened_at",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(PageRow {
            page: r.get(0)?,
            url: r.get(1)?,
            context: r.get(2)?,
            viewport_w: r.get(3)?,
            viewport_h: r.get(4)?,
            viewport_dpr: r.get(5)?,
            mobile: r.get::<_, i64>(6)? != 0,
            state: r.get(7)?,
            opened_at: r.get(8)?,
            updated_at: r.get(9)?,
        })
    })?;
    rows.collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(page: &str, url: &str) -> PageRow {
        PageRow {
            page: page.to_string(),
            url: url.to_string(),
            context: "default".to_string(),
            viewport_w: 960,
            viewport_h: 600,
            viewport_dpr: 2.0,
            mobile: false,
            state: "active".to_string(),
            opened_at: 1_000,
            updated_at: 1_000,
        }
    }

    #[test]
    fn a_page_is_one_row_and_a_navigation_keeps_it() {
        let conn = Connection::open_in_memory().expect("a database");
        setup_browser_schema(&conn).expect("ddl");
        upsert_page(&conn, &row("card-1", "https://example.com/a")).expect("insert");
        let mut second = row("card-1", "https://example.com/b#frag");
        second.updated_at = 2_000;
        upsert_page(&conn, &second).expect("update");
        let all = open_pages(&conn).expect("read");
        assert_eq!(all.len(), 1, "a navigation is not a second page");
        assert_eq!(all[0].url, "https://example.com/b#frag");
        assert_eq!(
            all[0].opened_at, 1_000,
            "and the page is as old as it was: the row is what a restart reopens"
        );
        assert_eq!(all[0].updated_at, 2_000);
    }

    #[test]
    fn a_closed_page_leaves_no_row_behind() {
        let conn = Connection::open_in_memory().expect("a database");
        setup_browser_schema(&conn).expect("ddl");
        upsert_page(&conn, &row("card-1", "https://example.com/a")).expect("insert");
        upsert_page(&conn, &row("card-2", "https://example.com/b")).expect("insert");
        delete_page(&conn, "card-1").expect("delete");
        let left: Vec<String> = open_pages(&conn)
            .expect("read")
            .into_iter()
            .map(|r| r.page)
            .collect();
        assert_eq!(left, vec!["card-2".to_string()]);
    }
}

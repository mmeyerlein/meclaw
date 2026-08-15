//! The store's FTS5 tokenizer: `unicode61` plus the light stemmer (0.2.0 P3,
//! ruling Q6, issue #14).
//!
//! FTS5 runs the table's tokenizer over BOTH the indexed text and the query
//! text, which is the whole reason the fix lives here and not in the recall
//! lane: fold once, in one place, and a plural question meets the singular fact
//! it answers without anyone expanding anything.
//!
//! It is a **wrapper**, not a replacement. Splitting text into words, folding
//! case and dropping diacritics is `unicode61`'s job and stays there — this
//! module finds that tokenizer through the FTS5 API, drives it, and rewrites
//! only the token text on the way out ([`super::stem`]).
//!
//! Registration goes through the FTS5 extension API on the rusqlite `bundled`
//! connection. No SQLite extension is loaded and no native dependency is added
//! (memory-hive spec: everything on `rusqlite` bundled) — the `fts5_api` handle
//! is fetched with the documented `SELECT fts5(?)` pointer call.
//!
//! The tokenizer lives on the CONNECTION, exactly like `hamming`, so every path
//! that opens a `store` `cell.db` has to install it: wake, respawn and the
//! `DbConn` re-open after a dropped call future (#59). A connection without it
//! cannot even read an index that declares it — FTS5 resolves the tokenizer
//! name when the virtual table is opened.

use rusqlite::ffi;
use std::ffi::{CStr, c_char, c_int, c_void};

/// The tokenizer name the FTS5 declaration refers to (`tokenize='…'`).
///
/// Part of the on-disk index declaration: it is stored verbatim in
/// `sqlite_master`, which is what lets [`crate::store::ddl::apply_fts_ddl`]
/// recognise an index built before this package and rebuild it.
///
/// **Versioned on purpose.** The name is the ONLY migration signal an index
/// carries: `apply_fts_ddl` rebuilds an index whose declaration does not name
/// the current tokenizer, and leaves every other index alone. A change to the
/// stemming rules therefore has to change the NAME as well — otherwise an index
/// built by the old rules keeps its old terms forever while the query side folds
/// by the new ones, and the two silently stop meeting. Rule: **every change to
/// [`super::stem`] bumps the `_vN` suffix here and appends the previous spelling
/// to [`LEGACY_TOKENIZER_NAMES`].**
pub const TOKENIZER_NAME: &str = "meclaw_stem_v1";

/// The same name for the C API.
const TOKENIZER_NAME_C: &CStr = c"meclaw_stem_v1";

/// Every tokenizer name this store has ever written into an index declaration,
/// current one excluded.
///
/// These are registered on the connection ALONGSIDE the current tokenizer, and
/// that is not politeness — it is what makes the rebuild reachable at all. The
/// tokenizer of an FTS5 virtual table is resolved when the table is OPENED, so a
/// connection that does not know `meclaw_stem` cannot so much as read the column
/// list of an index declaring it (`existing_fts_columns` → `no such tokenizer`),
/// and `apply_fts_ddl` would fail as a spawn error before it ever got to drop and
/// rebuild the thing. The legacy entries all resolve to the SAME implementation:
/// they exist to be openable and then dropped, never to be written again.
const LEGACY_TOKENIZER_NAMES: &[&CStr] = &[c"meclaw_stem"];

/// The tokenizer this one wraps. `unicode61` is FTS5's default, so an index
/// built before this package tokenized its text exactly this way — the rebuild
/// therefore changes the token TEXT and nothing else.
const BASE_TOKENIZER_C: &CStr = c"unicode61";

/// The pointer type of the `SELECT fts5(?)` call, spelled as SQLite spells it.
const FTS5_API_PTR_TYPE: &CStr = c"fts5_api_ptr";

/// Register the stemming tokenizer on a `store` `cell.db` connection — under the
/// current name AND under every legacy name.
///
/// Called wherever such a connection is born (the factory's `WakeFn` and
/// `RespawnFn`) and wherever one is re-born (`DbConn`'s re-open setup). Calling
/// it twice on one connection is harmless — FTS5 replaces the entry under the
/// same name.
///
/// An error here is not fatal by itself, but it is consequential: an existing
/// index that declares the tokenizer cannot be opened at all without it, so the
/// caller logs loudly and the cell comes up without its keyword leg rather than
/// silently answering fewer questions.
pub fn register(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    // SAFETY: `handle()` yields the live `sqlite3*` of a connection this thread
    // owns for the duration of the call (`DbConn` hands the connection to one
    // blocking closure at a time), and every pointer below is either that handle
    // or one SQLite itself just returned.
    unsafe {
        let api = fts5_api(conn.handle())?;
        let Some(create_tokenizer) = (*api).xCreateTokenizer else {
            return Err(fts5_error("fts5_api carries no xCreateTokenizer"));
        };
        register_under(api, create_tokenizer, TOKENIZER_NAME_C)?;
        for legacy in LEGACY_TOKENIZER_NAMES {
            register_under(api, create_tokenizer, legacy)?;
        }
    }
    Ok(())
}

/// Install this module's tokenizer implementation under one name.
///
/// # Safety
/// `api` must be the live `fts5_api` of a connection the caller owns, and
/// `create_tokenizer` its own `xCreateTokenizer`.
unsafe fn register_under(
    api: *mut ffi::fts5_api,
    create_tokenizer: XCreateTokenizer,
    name: &CStr,
) -> rusqlite::Result<()> {
    let mut module = ffi::fts5_tokenizer {
        xCreate: Some(x_create),
        xDelete: Some(x_delete),
        xTokenize: Some(x_tokenize),
    };
    // The api pointer rides along as the tokenizer's context so `x_create`
    // can look the base tokenizer up. It outlives every tokenizer instance
    // (it belongs to the database handle), so there is nothing to destroy.
    let rc = unsafe {
        create_tokenizer(
            api,
            name.as_ptr(),
            api.cast::<c_void>(),
            &raw mut module,
            None,
        )
    };
    if rc != ffi::SQLITE_OK {
        return Err(rusqlite::Error::SqliteFailure(
            ffi::Error::new(rc),
            Some(format!(
                "could not register tokenizer {}",
                name.to_string_lossy()
            )),
        ));
    }
    Ok(())
}

/// The `xCreateTokenizer` member of `fts5_api`, named so the helper above can
/// take it as a parameter.
type XCreateTokenizer = unsafe extern "C" fn(
    *mut ffi::fts5_api,
    *const c_char,
    *mut c_void,
    *mut ffi::fts5_tokenizer,
    Option<unsafe extern "C" fn(*mut c_void)>,
) -> c_int;

/// Fetch the `fts5_api` handle of a connection.
///
/// This is the shape SQLite's own documentation prescribes: prepare
/// `SELECT fts5(?)`, bind a pointer of type `fts5_api_ptr`, step once. There is
/// no other public way in — and no header to link against, which is why the
/// call is spelled out here instead of hidden behind a helper crate.
///
/// # Safety
/// `db` must be a live `sqlite3*` owned by the caller for the duration.
unsafe fn fts5_api(db: *mut ffi::sqlite3) -> rusqlite::Result<*mut ffi::fts5_api> {
    unsafe {
        let mut stmt: *mut ffi::sqlite3_stmt = std::ptr::null_mut();
        let rc = ffi::sqlite3_prepare_v2(
            db,
            c"SELECT fts5(?)".as_ptr(),
            -1,
            &raw mut stmt,
            std::ptr::null_mut(),
        );
        if rc != ffi::SQLITE_OK {
            return Err(rusqlite::Error::SqliteFailure(
                ffi::Error::new(rc),
                Some("FTS5 is not available on this connection".into()),
            ));
        }
        let mut api: *mut ffi::fts5_api = std::ptr::null_mut();
        let rc = ffi::sqlite3_bind_pointer(
            stmt,
            1,
            (&raw mut api).cast::<c_void>(),
            FTS5_API_PTR_TYPE.as_ptr(),
            None,
        );
        if rc == ffi::SQLITE_OK {
            ffi::sqlite3_step(stmt);
        }
        ffi::sqlite3_finalize(stmt);
        if api.is_null() {
            return Err(fts5_error("SELECT fts5(?) returned no api pointer"));
        }
        Ok(api)
    }
}

/// A misuse of the FTS5 API that carries no SQLite result code of its own.
fn fts5_error(what: &str) -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(ffi::Error::new(ffi::SQLITE_ERROR), Some(what.to_string()))
}

/// One live tokenizer instance: the base module's function table plus the
/// `unicode61` instance this one drives.
struct StemTokenizer {
    base: ffi::fts5_tokenizer,
    inner: *mut ffi::Fts5Tokenizer,
}

/// The relay handed to `unicode61` in place of FTS5's own token sink, so every
/// token passes through the stemmer before it reaches the index or the matcher.
struct Relay {
    ctx: *mut c_void,
    token: XTokenFn,
}

/// FTS5's token sink signature.
type XTokenFn =
    unsafe extern "C" fn(*mut c_void, c_int, *const c_char, c_int, c_int, c_int) -> c_int;

/// `xCreate` — build a `unicode61` instance and wrap it.
///
/// Any arguments in the `tokenize=` declaration are handed to the base
/// tokenizer unchanged, so `tokenize='meclaw_stem remove_diacritics 2'` keeps
/// working the way the same argument works on `unicode61`.
unsafe extern "C" fn x_create(
    ctx: *mut c_void,
    args: *mut *const c_char,
    n_args: c_int,
    out: *mut *mut ffi::Fts5Tokenizer,
) -> c_int {
    unsafe {
        let api = ctx.cast::<ffi::fts5_api>();
        if api.is_null() || out.is_null() {
            return ffi::SQLITE_ERROR;
        }
        let Some(find) = (*api).xFindTokenizer else {
            return ffi::SQLITE_ERROR;
        };
        let mut base = ffi::fts5_tokenizer {
            xCreate: None,
            xDelete: None,
            xTokenize: None,
        };
        let mut base_ctx: *mut c_void = std::ptr::null_mut();
        let rc = find(
            api,
            BASE_TOKENIZER_C.as_ptr(),
            &raw mut base_ctx,
            &raw mut base,
        );
        if rc != ffi::SQLITE_OK {
            return rc;
        }
        let Some(base_create) = base.xCreate else {
            return ffi::SQLITE_ERROR;
        };
        let mut inner: *mut ffi::Fts5Tokenizer = std::ptr::null_mut();
        let rc = base_create(base_ctx, args, n_args, &raw mut inner);
        if rc != ffi::SQLITE_OK {
            return rc;
        }
        *out = Box::into_raw(Box::new(StemTokenizer { base, inner })).cast();
        ffi::SQLITE_OK
    }
}

/// `xDelete` — the base instance goes down with the wrapper.
unsafe extern "C" fn x_delete(tokenizer: *mut ffi::Fts5Tokenizer) {
    unsafe {
        if tokenizer.is_null() {
            return;
        }
        let wrapper = Box::from_raw(tokenizer.cast::<StemTokenizer>());
        if let Some(base_delete) = wrapper.base.xDelete {
            base_delete(wrapper.inner);
        }
    }
}

/// `xTokenize` — let `unicode61` do the splitting, stem what comes out.
///
/// `flags` is passed through untouched. FTS5 uses it to tell document text from
/// query text and to mark the last token of a prefix query; the fold has to be
/// the same in every one of those cases, or the two sides would stop meeting.
unsafe extern "C" fn x_tokenize(
    tokenizer: *mut ffi::Fts5Tokenizer,
    ctx: *mut c_void,
    flags: c_int,
    text: *const c_char,
    n_text: c_int,
    token: Option<XTokenFn>,
) -> c_int {
    unsafe {
        if tokenizer.is_null() {
            return ffi::SQLITE_ERROR;
        }
        let wrapper = &*tokenizer.cast::<StemTokenizer>();
        let (Some(base_tokenize), Some(token)) = (wrapper.base.xTokenize, token) else {
            return ffi::SQLITE_ERROR;
        };
        let mut relay = Relay { ctx, token };
        base_tokenize(
            wrapper.inner,
            (&raw mut relay).cast::<c_void>(),
            flags,
            text,
            n_text,
            Some(relay_token),
        )
    }
}

/// The relay callback: fold the token, keep the byte offsets.
///
/// The stem is always a PREFIX of the token ([`super::stem::stem`]), so the
/// pointer FTS5 already holds stays valid and only the length shrinks — no
/// buffer, no copy, and `iStart`/`iEnd` keep pointing at the ORIGINAL word in
/// the source text, which is what snippet and offset functions read.
///
/// A token that is not valid UTF-8 is passed through untouched rather than
/// dropped: `unicode61` works on bytes, and losing a token would silently make
/// a row unfindable.
unsafe extern "C" fn relay_token(
    ctx: *mut c_void,
    flags: c_int,
    token: *const c_char,
    n_token: c_int,
    start: c_int,
    end: c_int,
) -> c_int {
    unsafe {
        let relay = &*ctx.cast::<Relay>();
        let len = usize::try_from(n_token).unwrap_or(0);
        // An empty or absent token is handed on as it came: building a slice
        // from a null pointer would be undefined behaviour even at length zero.
        let folded = if token.is_null() || len == 0 {
            len
        } else {
            let bytes = std::slice::from_raw_parts(token.cast::<u8>(), len);
            match std::str::from_utf8(bytes) {
                Ok(text) => super::stem::stem(text).len(),
                Err(_) => len,
            }
        };
        (relay.token)(
            relay.ctx,
            flags,
            token,
            c_int::try_from(folded).unwrap_or(n_token),
            start,
            end,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A connection with the tokenizer installed and one FTS5 table that
    /// declares it, holding the rows the caller names.
    fn indexed(rows: &[&str]) -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        register(&conn).unwrap();
        conn.execute_batch(&format!(
            "CREATE VIRTUAL TABLE t USING fts5(body, tokenize='{TOKENIZER_NAME}');"
        ))
        .unwrap();
        for row in rows {
            conn.execute("INSERT INTO t(body) VALUES (?1)", [row])
                .unwrap();
        }
        conn
    }

    /// How the recall lane asks: quoted token plus the trailing prefix star.
    fn hits(conn: &rusqlite::Connection, term: &str) -> i64 {
        conn.query_row(
            "SELECT count(*) FROM t WHERE t MATCH ?1",
            [format!("\"{term}\"*")],
            |r| r.get(0),
        )
        .unwrap()
    }

    /// Issue #14 itself, end to end through SQLite: the plural question finds
    /// the singular fact. This is the assertion the whole package exists for —
    /// before it, `"lieblingseditoren"*` scored zero against this row.
    #[test]
    fn a_german_plural_query_finds_the_singular_row() {
        let conn = indexed(&["hat lieblingseditor helix"]);
        assert_eq!(hits(&conn, "lieblingseditoren"), 1, "plural finds singular");
        assert_eq!(
            hits(&conn, "lieblingseditor"),
            1,
            "and so does the singular"
        );
    }

    /// The counter-direction, which prefix matching already covered and which
    /// must not regress: the singular question finds the plural fact.
    #[test]
    fn a_german_singular_query_finds_the_plural_row() {
        let conn = indexed(&["hat lieblingseditoren helix"]);
        assert_eq!(hits(&conn, "lieblingseditor"), 1);
        assert_eq!(hits(&conn, "lieblingseditoren"), 1);
    }

    /// The same in both directions for German inflection classes the suffix
    /// table names, and for English.
    #[test]
    fn both_languages_meet_in_both_directions() {
        for (indexed_text, query) in [
            ("die katze schlaeft", "katzen"),
            ("zwei katzen schlafen", "katze"),
            ("drei tage frei", "tag"),
            ("ein tag frei", "tage"),
            ("das kind spielt", "kindern"),
            ("favorite editor is helix", "editors"),
            ("favorite editors are helix", "editor"),
            ("one box arrived", "boxes"),
            ("two boxes arrived", "box"),
        ] {
            let conn = indexed(&[indexed_text]);
            assert_eq!(
                hits(&conn, query),
                1,
                "{query:?} must reach {indexed_text:?}"
            );
        }
    }

    /// What the index really holds, read off the index itself.
    ///
    /// No `MATCH` can show this: the query text runs through the very same
    /// tokenizer, so searching for `katzen` searches for `katz` and hits. The
    /// `fts5vocab` table is the only way to see the stored terms — and it is
    /// what turns "the fold happened" from an inference into a reading.
    #[test]
    fn the_index_holds_the_folded_terms_and_nothing_else() {
        let conn = indexed(&["zwei katzen im haus"]);
        conn.execute_batch("CREATE VIRTUAL TABLE t_terms USING fts5vocab(t, 'row');")
            .unwrap();
        let terms: Vec<String> = conn
            .prepare("SELECT term FROM t_terms ORDER BY term")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            terms,
            vec!["haus", "im", "katz", "zwei"],
            "the plural is folded, the -s guard holds, the short words stay whole"
        );
    }

    /// The tokenizer must not fold two unrelated words together: an index that
    /// answers everything answers nothing.
    #[test]
    fn unrelated_words_stay_apart() {
        let conn = indexed(&["das haus am see"]);
        assert_eq!(hits(&conn, "seen"), 0, "see is not the singular of seen");
        assert_eq!(hits(&conn, "hausboot"), 0);
        assert_eq!(hits(&conn, "haus"), 1);
    }

    /// An identifier is what the keyword leg exists for (scenario K1). Folding
    /// must leave it alone.
    #[test]
    fn an_identifier_survives_the_fold() {
        let conn = indexed(&["invoice INV-2024-0815 is paid"]);
        assert_eq!(hits(&conn, "INV"), 1);
        assert_eq!(hits(&conn, "0815"), 1);
    }

    /// `rebuild` re-tokenizes from the content table — the command the FTS
    /// migration leans on. It has to run through this tokenizer too, otherwise
    /// a migrated index would hold unfolded terms while queries arrive folded.
    #[test]
    fn a_rebuild_re_tokenizes_through_the_stemmer() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        register(&conn).unwrap();
        conn.execute_batch(&format!(
            "CREATE TABLE facts (claim TEXT);
             INSERT INTO facts VALUES ('hat lieblingseditor helix');
             CREATE VIRTUAL TABLE facts_fts USING fts5(claim, content='facts',
                 content_rowid='rowid', tokenize='{TOKENIZER_NAME}');
             INSERT INTO facts_fts(facts_fts) VALUES ('rebuild');"
        ))
        .unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM facts_fts WHERE facts_fts MATCH '\"lieblingseditoren\"*'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "the rebuild must index folded terms");
    }

    /// Without the registration the very same declaration is a loud SQL error —
    /// which is why the factory installs the tokenizer on every connection that
    /// is born, and why `DbConn` re-installs it after a re-open. A silent
    /// fallback would be worse: the index would exist and answer wrongly.
    #[test]
    fn an_unregistered_tokenizer_is_a_loud_error() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let err = conn
            .execute_batch(&format!(
                "CREATE VIRTUAL TABLE t USING fts5(body, tokenize='{TOKENIZER_NAME}');"
            ))
            .unwrap_err();
        assert!(
            err.to_string().contains("tokenizer"),
            "expected a tokenizer error, got: {err}"
        );
    }

    /// Registering twice on one connection must not fail: the wake path and the
    /// `DbConn` re-open setup can both run against the same connection.
    #[test]
    fn registration_is_repeatable() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        register(&conn).unwrap();
        register(&conn).unwrap();
        conn.execute_batch(&format!(
            "CREATE VIRTUAL TABLE t USING fts5(body, tokenize='{TOKENIZER_NAME}');
             INSERT INTO t(body) VALUES ('zwei katzen');"
        ))
        .unwrap();
        assert_eq!(hits(&conn, "katze"), 1);
    }
}

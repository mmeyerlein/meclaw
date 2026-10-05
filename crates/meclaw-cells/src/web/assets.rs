//! GH #393: the files a `web` cell serves under its own origin.
//!
//! The `assets` table shipped with the type (GH #380) and nothing delivered it.
//! This module is the missing half: it reads the table once into an immutable
//! snapshot, which the I/O half then answers out of the same way it answers out
//! of the materialised page map — no database on the request path, no cell call
//! (R-W8-4a).
//!
//! # Assets are seed data, today
//!
//! There is **no op that writes `assets`** — [`crate::web::ops`] has
//! `object.*`, `component.define`, `page.set` and `query`, and none of them
//! touches this table. So the snapshot is built once, at start, and does not
//! change for the life of the cell; the shape it is published in ([`AssetMap`]
//! behind a `watch` channel) is nonetheless the shape a write would need, so a
//! future op that adds a file re-publishes here and changes nothing else.
//!
//! # Why the reader takes TEXT as well as BLOB
//!
//! `crate::web::seed` maps every JSON string onto SQLite `TEXT`, because a seed
//! file is JSON and JSON has no byte string. A seeded asset body therefore sits
//! in the `BLOB NOT NULL` column **as text** — SQLite stores what it is given,
//! the column type is a hint — and `FromSql for Vec<u8>` refuses that with
//! `InvalidType`. So the read goes through [`rusqlite::types::ValueRef`] and
//! takes either storage class.
//!
//! The alternative was to change the DDL or to make the seed loader guess which
//! column wants bytes. Both are more expensive than a tolerant reader: the
//! table is already shipped, a migration on a freshly released schema costs
//! every existing `cell.db`, and a loader that guessed per column would be a
//! second place where the schema is written down.

use flate2::Compression;
use flate2::write::GzEncoder;
use rusqlite::Connection;
use rusqlite::types::ValueRef;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Write;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// One row of `assets`, ready to be written to a response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    /// The `Content-Type` this file is served with — from the row, never from
    /// the path. What a file *is* is stated by whoever put it there; guessing
    /// from an extension would make a display's answer depend on a table of
    /// suffixes nobody declared.
    pub content_type: String,
    /// The file itself.
    pub body: Vec<u8>,
    /// GH #1002: the strong validator of [`Self::body`], quotes included
    /// (`"<32 hex>"`). Computed once, when the snapshot is loaded.
    pub etag: String,
    /// GH #1002: the gzip variant, when the file is text of at least
    /// [`PRECOMPRESS_MIN_BYTES`] and the variant is smaller. Computed once,
    /// when the snapshot is loaded — never per request (R-H4-3).
    pub gz: Option<Vec<u8>>,
}

impl Asset {
    /// A file with its validator and, where it pays, its gzip variant.
    pub fn new(content_type: String, body: Vec<u8>) -> Self {
        let etag = etag_of(&body);
        let gz = precompress(&content_type, &body);
        Self {
            content_type,
            body,
            etag,
            gz,
        }
    }

    /// The first twelve hex digits of the validator: what a `?v=` stamp
    /// carries (see [`stamp_of`]).
    pub fn stamp(&self) -> &str {
        stamp_of(&self.etag)
    }
}

/// GH #1002: below this a text file is served as stored.
///
/// R-H4-3 (the owner, 05.10.): compression only for files of at least 16 KB.
/// Under that the bytes saved are a few hundred, a gzip member costs 18 bytes
/// of header and trailer by itself, and the variant would be a second copy of
/// every small file for nothing.
pub const PRECOMPRESS_MIN_BYTES: usize = 16 * 1024;

/// Every gzip variant this process has computed. Read by the locks that pin
/// "computed when loaded, never per request" (GH #1002 T4): a request path
/// that compressed would move this counter.
static COMPRESSIONS: AtomicU64 = AtomicU64::new(0);

/// How many gzip variants this process has computed so far.
pub fn compressions() -> u64 {
    COMPRESSIONS.load(Ordering::Relaxed)
}

/// The strong validator of `body`: `"` + the first 16 bytes of its SHA-256 in
/// hex + `"`. SHA-256 because the crate already carries it (`sha2`); 128 bits
/// are far past any collision a cache could meet.
pub fn etag_of(body: &[u8]) -> String {
    let digest = Sha256::digest(body);
    let mut out = String::with_capacity(34);
    out.push('"');
    for b in &digest[..16] {
        out.push_str(&format!("{b:02x}"));
    }
    out.push('"');
    out
}

/// The `?v=` stamp of a validator: its first twelve hex digits. Short,
/// because it is written into every page's `<script src>`; twelve hex digits
/// (48 bits) still make a stale stamp that happens to match a later file
/// practically impossible.
pub fn stamp_of(etag: &str) -> &str {
    let inner = etag.trim_matches('"');
    &inner[..inner.len().min(12)]
}

/// Whether a content type is text a gzip variant pays for.
fn compressible(content_type: &str) -> bool {
    let ct = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    ct.starts_with("text/")
        || matches!(
            ct.as_str(),
            "application/javascript" | "application/json" | "image/svg+xml"
        )
}

/// The gzip variant of `body`, or `None` when the file is not text, smaller
/// than [`PRECOMPRESS_MIN_BYTES`], or would not get smaller.
///
/// Level 9 because this runs once per file per life: the measured city's
/// stylesheet (250 KB) and the LiveView client (122 KB) take a few
/// milliseconds each at load, and every request after that pays nothing.
pub fn precompress(content_type: &str, body: &[u8]) -> Option<Vec<u8>> {
    if body.len() < PRECOMPRESS_MIN_BYTES || !compressible(content_type) {
        return None;
    }
    COMPRESSIONS.fetch_add(1, Ordering::Relaxed);
    let mut enc = GzEncoder::new(Vec::with_capacity(body.len() / 2), Compression::best());
    enc.write_all(body).ok()?;
    let gz = enc.finish().ok()?;
    (gz.len() < body.len()).then_some(gz)
}

/// One of the client files compiled into the binary, ready to be served.
#[derive(Debug)]
pub struct ClientFile {
    /// Its content type.
    pub content_type: &'static str,
    /// The file.
    pub body: &'static str,
    /// Its strong validator, as [`Asset::etag`].
    pub etag: String,
    /// Its gzip variant, as [`Asset::gz`].
    pub gz: Option<Vec<u8>>,
}

impl ClientFile {
    /// The `?v=` stamp the shell writes into its `<script src>`.
    pub fn stamp(&self) -> &str {
        stamp_of(&self.etag)
    }
}

/// The names [`meclaw_surface::bundle`] answers, in one list.
const CLIENT_FILES: [&str; 4] = [
    "phoenix.min.js",
    "phoenix_live_view.min.js",
    "boot.js",
    "display-mic-worklet.js",
];

/// Every client file with its validator and variant, built once per process.
///
/// A `OnceLock` because the files are `&'static str` compiled into the binary:
/// their validators and variants never change while it runs. The I/O half
/// calls this when it starts (eager init), so the first page request does not
/// pay for the compression.
pub fn client_files() -> &'static BTreeMap<&'static str, ClientFile> {
    static FILES: OnceLock<BTreeMap<&'static str, ClientFile>> = OnceLock::new();
    FILES.get_or_init(|| {
        CLIENT_FILES
            .iter()
            .filter_map(|name| {
                let (content_type, body) = meclaw_surface::bundle(name)?;
                Some((
                    *name,
                    ClientFile {
                        content_type,
                        body,
                        etag: etag_of(body.as_bytes()),
                        gz: precompress(content_type, body.as_bytes()),
                    },
                ))
            })
            .collect()
    })
}

/// One client file by name, or `None` for a name the closed list lacks.
pub fn client_file(name: &str) -> Option<&'static ClientFile> {
    client_files().get(name)
}

/// Every file this cell serves, by the path it answers on.
///
/// A `BTreeMap` for the same reason [`crate::web::render::PageMap`] is one: an
/// operator comparing two dumps should not have to sort them first.
pub type AssetMap = BTreeMap<String, Asset>;

/// Read the whole `assets` table into a snapshot.
///
/// A row whose body is neither text nor a blob — which a seed can produce by
/// writing a number where a file belongs — is **skipped with its path named**
/// rather than failing the load: one malformed row must not cost every other
/// file in the same cell, and the path in the log is what makes it fixable.
pub fn load_assets(conn: &Connection) -> rusqlite::Result<AssetMap> {
    let mut stmt = conn.prepare("SELECT path, content_type, body FROM assets ORDER BY path")?;
    let mut rows = stmt.query([])?;
    let mut out = AssetMap::new();
    while let Some(row) = rows.next()? {
        let path: String = row.get(0)?;
        let content_type: String = row.get(1)?;
        let body = match row.get_ref(2)? {
            // The hand-written case, and what an op would write.
            ValueRef::Blob(b) => b.to_vec(),
            // The seeded case: `json_to_sql` had a JSON string and stored TEXT.
            ValueRef::Text(t) => t.to_vec(),
            other => {
                let kind = other.data_type();
                tracing::warn!(
                    asset = %path,
                    storage = %kind,
                    "web: asset body is neither text nor a blob — this file is not served"
                );
                continue;
            }
        };
        out.insert(path, Asset::new(content_type, body));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::db::setup_web_schema;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().expect("open in memory");
        setup_web_schema(&conn).expect("schema");
        conn
    }

    #[test]
    fn a_body_stored_as_text_reads_back_as_its_bytes() {
        // The seeded case. `FromSql for Vec<u8>` is an `InvalidType` here, which
        // is the whole reason this function exists.
        let conn = db();
        conn.execute(
            "INSERT INTO assets (path, content_type, body) VALUES (?1, ?2, ?3)",
            rusqlite::params!["/a.css", "text/css", "body{}"],
        )
        .expect("insert text");
        let map = load_assets(&conn).expect("load");
        assert_eq!(map["/a.css"].body, b"body{}".to_vec());
        assert_eq!(map["/a.css"].content_type, "text/css");
    }

    #[test]
    fn a_body_stored_as_a_blob_reads_back_as_its_bytes() {
        let conn = db();
        let bytes: Vec<u8> = vec![0x00, 0xff, 0x10, 0x0a];
        conn.execute(
            "INSERT INTO assets (path, content_type, body) VALUES (?1, ?2, ?3)",
            rusqlite::params!["/f.bin", "application/octet-stream", bytes.clone()],
        )
        .expect("insert blob");
        let map = load_assets(&conn).expect("load");
        // Bytes no UTF-8 decode would survive: the reader hands them through.
        assert_eq!(map["/f.bin"].body, bytes);
    }

    #[test]
    fn a_body_that_is_a_number_is_skipped_and_the_others_survive() {
        let conn = db();
        conn.execute(
            "INSERT INTO assets (path, content_type, body) VALUES ('/n', 'text/plain', 7)",
            [],
        )
        .expect("insert number");
        conn.execute(
            "INSERT INTO assets (path, content_type, body) VALUES ('/ok', 'text/plain', 'x')",
            [],
        )
        .expect("insert text");
        let map = load_assets(&conn).expect("load");
        assert!(!map.contains_key("/n"), "a number is not a file");
        assert_eq!(map["/ok"].body, b"x".to_vec(), "and it costs nobody else");
    }

    #[test]
    fn gh1002_only_large_text_gets_a_variant_and_it_is_the_file() {
        use std::io::Read;
        let big = "a{b:c}\n"
            .repeat(PRECOMPRESS_MIN_BYTES / 7 + 1)
            .into_bytes();
        let gz = precompress("text/css; charset=utf-8", &big).expect("large text");
        let mut back = Vec::new();
        flate2::read::GzDecoder::new(&gz[..])
            .read_to_end(&mut back)
            .expect("gunzip");
        assert_eq!(back, big);
        assert!(precompress("text/css", &big[..PRECOMPRESS_MIN_BYTES - 1]).is_none());
        assert!(precompress("image/png", &big).is_none());
        assert!(precompress("application/json", &big).is_some());
        assert!(precompress("image/svg+xml", &big).is_some());
    }

    #[test]
    fn gh1002_a_validator_is_strong_and_follows_the_bytes() {
        let a = etag_of(b"one");
        assert!(
            a.starts_with('"') && a.ends_with('"') && a.len() == 34,
            "{a}"
        );
        assert_ne!(a, etag_of(b"two"));
        assert_eq!(stamp_of(&a).len(), 12);
        let names: Vec<_> = client_files().keys().copied().collect();
        assert_eq!(names.len(), 4, "every client file has its entry: {names:?}");
        assert!(
            client_file("phoenix_live_view.min.js")
                .and_then(|f| f.gz.as_ref())
                .is_some()
        );
        assert!(
            client_file("boot.js").is_some_and(|f| f.gz.is_none()),
            "boot.js is small"
        );
    }

    #[test]
    fn an_empty_table_is_an_empty_snapshot() {
        // A cell with no assets is the normal case, not an error.
        assert!(load_assets(&db()).expect("load").is_empty());
    }
}

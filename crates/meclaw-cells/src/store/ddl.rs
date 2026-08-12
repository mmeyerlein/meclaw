//! Synthesizes `CREATE TABLE IF NOT EXISTS` statements from the
//! validated 2-stage schema map. Sync (rusqlite); called by the
//! StoreCellFactory before `tokio::spawn(cell_task_stateful)`
//! (Phase-7.5 Ad-hoc-DDL-Pattern).

use std::collections::BTreeMap;

/// Validate an identifier that is about to be **created**.
///
/// The catalog cannot vet a name that does not exist yet, so `create_table` and
/// `params.schema` — the only two paths that format a fresh identifier into DDL —
/// gate it by syntax instead: ASCII word characters, not starting with a digit,
/// at most 63 characters, no `sqlite_` prefix (SQLite's own namespace) and no
/// `_fts` suffix (reserved for the FTS index tables of P3).
pub fn check_new_identifier(kind: &str, name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name.len() <= 63
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !ok {
        return Err(format!(
            "{kind} {name:?}: only [A-Za-z_][A-Za-z0-9_]{{0,62}} allowed"
        ));
    }
    let lowered = name.to_ascii_lowercase();
    if lowered.starts_with("sqlite_") {
        return Err(format!("{kind} {name:?}: sqlite_ prefix is reserved"));
    }
    if lowered.ends_with("_fts") {
        return Err(format!(
            "{kind} {name:?}: _fts suffix is reserved for FTS indexes"
        ));
    }
    Ok(())
}

/// Map declared schema-type strings to SQLite column types.
fn sqlite_type(t: &str) -> &'static str {
    match t {
        "int" => "INTEGER",
        "text" => "TEXT",
        "json" => "TEXT", // SQLite JSON1 lives in TEXT columns by convention.
        _ => unreachable!("StoreParams::parse validates allowed types"),
    }
}

/// Apply schema DDL. Idempotent (`CREATE TABLE IF NOT EXISTS`). Order
/// is deterministic because the schema is a BTreeMap.
///
/// A table that already exists is **grown** to the declaration: every declared
/// column the table lacks is added with `ALTER TABLE ADD COLUMN` (0.2.0 P2).
/// `CREATE TABLE IF NOT EXISTS` alone is a no-op on an existing table, so
/// without this an existing `cell.db` would silently miss a column the running
/// code already reads — the same catch-up property the FTS declaration has.
/// Strictly additive: an existing column that the declaration does not name is
/// never touched, never retyped and never dropped (no-delete).
pub fn apply_schema_ddl(
    conn: &rusqlite::Connection,
    schema: &BTreeMap<String, BTreeMap<String, String>>,
) -> rusqlite::Result<()> {
    for (table, cols) in schema {
        let col_clause = cols
            .iter()
            .map(|(c, t)| format!("\"{c}\" {}", sqlite_type(t)))
            .collect::<Vec<_>>()
            .join(", ");
        let stmt = format!("CREATE TABLE IF NOT EXISTS \"{table}\" ({col_clause})");
        conn.execute(&stmt, [])?;
        let present = table_columns(conn, table)?;
        for (c, t) in cols {
            if !present.contains(c) {
                conn.execute(
                    &format!(
                        "ALTER TABLE \"{table}\" ADD COLUMN \"{c}\" {}",
                        sqlite_type(t)
                    ),
                    [],
                )?;
            }
        }
    }
    Ok(())
}

/// The column names of an existing table, as SQLite spells them.
fn table_columns(
    conn: &rusqlite::Connection,
    table: &str,
) -> rusqlite::Result<std::collections::BTreeSet<String>> {
    let mut st = conn.prepare("SELECT name FROM pragma_table_info(?1)")?;
    st.query_map([table], |r| r.get::<_, String>(0))?.collect()
}

/// The SQL expression that derives one canonical value: the alias table wins,
/// the original is the fallback (0.2.0 P2, ruling Q3).
///
/// Every identifier comes from `params.canonical`, which is closed-set validated
/// against `params.schema` at parse time — the same standard under which
/// [`apply_schema_ddl`] formats a table name into DDL.
/// A normalising binding (0.2.0 P4) wraps every side of the lookup in
/// `meclaw_norm`: the written value, the alias key it is looked up under, and the
/// canonical value that comes back. That is what makes normalisation equality an
/// automatic merge — two spellings that differ only in case, whitespace or
/// Unicode composition produce the SAME key and therefore the same identity —
/// and it is why an alias written for one spelling covers all of them.
pub(crate) fn canonical_derive_expr(table: &str, spec: &crate::store::CanonicalSpec) -> String {
    let crate::store::CanonicalSpec {
        source, aliases, ..
    } = spec;
    let written = if spec.normalize {
        format!("meclaw_norm(\"{table}\".\"{source}\")")
    } else {
        format!("\"{table}\".\"{source}\"")
    };
    let canonical = if spec.normalize {
        "meclaw_norm(\"canonical\")"
    } else {
        "\"canonical\""
    };
    format!(
        "COALESCE((SELECT {canonical} FROM \"{aliases}\" WHERE \"alias\" = {written}), {written})"
    )
}

/// Apply the canonical-column DDL for the declared bindings (0.2.0 P2, ruling Q3).
///
/// Three effects, all idempotent:
/// 1. the store-owned alias table exists (`alias` is its PRIMARY KEY, which is
///    what makes `set_alias` an upsert and therefore re-runnable by the nightly
///    GC);
/// 2. the store-owned rejected-pair table exists when the binding declares one
///    (0.2.0 P5) — the memory of a NEGATIVE judgement, so the GC stops
///    re-proposing a pair it has already turned down;
/// 3. every row whose target column is still empty is derived ONCE from its
///    original plus the alias table.
///
/// The backfill is the catch-up property the FTS index already has: a `cell.db`
/// that has been running without the column gets it filled on the next spawn,
/// instead of leaving all identity-sensitive reads blind until the first dream
/// run. It only ever touches EMPTY targets — a value the GC derived stays, and
/// re-deriving everything is the explicit `canonicalize` op, never a boot effect.
///
/// The originals are not read-modify-written anywhere here: `source` keeps its
/// bytes, which is what makes the whole mechanism revertible (drop the alias row,
/// re-derive).
pub fn apply_canonical_ddl(
    conn: &rusqlite::Connection,
    canonical: &BTreeMap<String, Vec<crate::store::CanonicalSpec>>,
) -> Result<(), String> {
    for (table, specs) in canonical {
        for spec in specs {
            let aliases = &spec.aliases;
            conn.execute(
                &format!(
                    "CREATE TABLE IF NOT EXISTS \"{aliases}\" (\"alias\" TEXT PRIMARY KEY, \
                     \"canonical\" TEXT NOT NULL, \"recorded_at\" TEXT)"
                ),
                [],
            )
            .map_err(|e| format!("alias table {aliases}: {e}"))?;
            // 0.2.0 P5: the refusal log, when the binding declares one. The pair is
            // the PRIMARY KEY so a re-judged pair is an upsert, and the two columns
            // are stored in a fixed order by the op — an unordered pair with an
            // ordered key, so ("a","b") and ("b","a") are one row.
            if let Some(rejected) = &spec.rejected {
                conn.execute(
                    &format!(
                        "CREATE TABLE IF NOT EXISTS \"{rejected}\" (\"left_value\" TEXT NOT NULL, \
                         \"right_value\" TEXT NOT NULL, \"recorded_at\" TEXT, \
                         PRIMARY KEY (\"left_value\", \"right_value\"))"
                    ),
                    [],
                )
                .map_err(|e| format!("rejected-pair table {rejected}: {e}"))?;
            }
            let target = &spec.target;
            conn.execute(
                &format!(
                    "UPDATE \"{table}\" SET \"{target}\" = {} WHERE \"{target}\" IS NULL OR \"{target}\" = ''",
                    canonical_derive_expr(table, spec)
                ),
                [],
            )
            .map_err(|e| format!("canonical backfill on {table}.{target}: {e}"))?;
        }
    }
    Ok(())
}

/// Apply the FTS5 index DDL for the declared tables (P3).
///
/// Strategy: **external content** (`content='<table>'`) plus insert/update/delete
/// triggers. Contentless FTS5 cannot be updated or rebuilt, and the store very
/// much has an `update` op (the memory lanes supersede facts and update
/// beliefs), so external content is the only shape that stays correct — and it
/// stores an index, not a copy of the text.
///
/// Idempotent by design, which is also how an existing `cell.db` catches up:
/// if the index table is missing it is created, the triggers are (re-)declared
/// and the index is rebuilt **once** from the base table, so rows written before
/// the declaration become searchable. If it is already there, nothing happens —
/// no rebuild on every boot.
///
/// A pre-existing index whose column list differs from the declaration is a loud
/// error — with two explicit exceptions, both of which the declaration itself
/// justifies:
/// - **additive** drift: the existing columns are a proper prefix of the declared
///   ones, so the index is dropped and rebuilt instead (P15/R8);
/// - **canonical** drift: the existing list becomes the declared one by replacing
///   a canonical binding's `source` column with its `target` (0.2.0 P2). That is
///   the migration ruling Q3 prescribes — the keyword leg stops indexing the
///   written spelling and starts indexing its alias-resolved twin — and it is a
///   pure substitution, not a silent loss of searchability.
///
/// The two compose (0.2.0 P4): the substitution is applied first, the additive
/// rule then runs over its result, so a store that skipped a release migrates in
/// one step instead of being refused.
///
/// Every other column drift (removal, reordering) stays loud.
///
/// A third class sits next to those two and is about the TOKENIZER rather than
/// the columns (0.2.0 P3, ruling Q6): every index this function builds declares
/// [`crate::store::query::fts_tokenizer::TOKENIZER_NAME`], and an existing index
/// that does not is rebuilt through it — that is how a `cell.db` written before
/// this package gets morphological matching without a migration tool. The class
/// is bound to the declaration in the same way the other two are: the name is a
/// constant of the store, so "the index does not declare it" is a statement
/// about this code, not a guess about the database.
///
/// The connection must already carry the tokenizer
/// ([`crate::store::query::install_connection_extensions`]); without it an index
/// declaring the name cannot even be opened, and this function fails loudly
/// rather than quietly building an unstemmed index.
///
/// Identifiers come from `params.schema`, which is syntax-gated by
/// [`check_new_identifier`] at parse time.
pub fn apply_fts_ddl(
    conn: &rusqlite::Connection,
    fts: &BTreeMap<String, Vec<String>>,
    canonical: &BTreeMap<String, Vec<crate::store::CanonicalSpec>>,
) -> Result<(), String> {
    let tokenizer = crate::store::query::fts_tokenizer::TOKENIZER_NAME;
    for (table, cols) in fts {
        let index = format!("{table}_fts");
        // Read off `sqlite_master` BEFORE the catalog lookup: this is plain
        // table text and needs no tokenizer, while `pragma_table_info` on an
        // FTS index opens the virtual table.
        let stemmed = index_declares_stem_tokenizer(conn, &index).map_err(|e| e.to_string())?;
        let existing = existing_fts_columns(conn, &index).map_err(|e| e.to_string())?;
        if let Some(found) = existing {
            if found == *cols && stemmed {
                continue;
            }
            // Every binding of the table substitutes its own source for its
            // target, so a store that gained a SECOND identity dimension migrates
            // in one step (0.2.0 P4) instead of needing an intermediate release.
            //
            // Each column substitutes INDEPENDENTLY (statement identity W2): a
            // declaration may move one dimension onto its canonical twin and keep
            // another written, and it does — the claim binding exists for identity,
            // while the keyword leg keeps indexing the written claim, because the
            // claim IS the text a question searches. Substituting all or nothing
            // would refuse a store that skipped the release in between. The variant
            // count is 2^k over the bound columns of one index, a handful at most.
            let variants: Vec<Vec<String>> = found.iter().fold(vec![Vec::new()], |acc, col| {
                let target = canonical
                    .get(table)
                    .and_then(|specs| specs.iter().find(|s| &s.source == col))
                    .map(|s| s.target.clone());
                let mut next = Vec::with_capacity(acc.len() * 2);
                for prefix in acc {
                    if let Some(t) = &target {
                        let mut swapped = prefix.clone();
                        swapped.push(t.clone());
                        next.push(swapped);
                    }
                    let mut kept = prefix;
                    kept.push(col.clone());
                    next.push(kept);
                }
                next
            });
            // An FTS index is a rebuildable projection over never-deleted source
            // text -- the same property the embedding generations rely on.
            // Dropping it destroys no truth, so a purely additive declaration may
            // rebuild instead of failing. Any other drift (removal, reordering)
            // stays loud: it would silently lose searchability.
            //
            // The two classes COMPOSE: the substitution is applied first and the
            // additive rule then runs over its result, so an index written before
            // P2 can migrate onto a P4 declaration that both swaps a column and
            // appends one. Without that composition the pre-P2 store would be
            // refused outright, which is a spawn failure, not a migration.
            //
            // The tokenizer never makes a refused column drift acceptable: the
            // column list has to be migratable on its own, and only then does a
            // missing tokenizer add its own reason to rebuild.
            let additive = |from: &[String]| from.len() < cols.len() && cols[..from.len()] == *from;
            let columns_migratable = variants.iter().any(|v| v == cols || additive(v.as_slice()));
            if !columns_migratable {
                return Err(format!(
                    "fts column drift on {index}: declared {cols:?}, existing {found:?} — \
                     not additive, refusing to rebuild"
                ));
            }
            // The triggers go with the table: `DROP TABLE` does not remove them,
            // and the `CREATE TRIGGER IF NOT EXISTS` below would keep the stale
            // column list alive -- rows written after the migration would never
            // reach the new column.
            conn.execute_batch(&format!(
                "DROP TRIGGER IF EXISTS \"{index}_ai\";
                 DROP TRIGGER IF EXISTS \"{index}_ad\";
                 DROP TRIGGER IF EXISTS \"{index}_au\";
                 DROP TABLE \"{index}\";"
            ))
            .map_err(|e| e.to_string())?;
        }
        let col_list = cols
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let new_vals = cols
            .iter()
            .map(|c| format!("new.\"{c}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let old_vals = cols
            .iter()
            .map(|c| format!("old.\"{c}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let ddl = format!(
            "CREATE VIRTUAL TABLE IF NOT EXISTS \"{index}\" USING fts5({col_list}, \
               content='{table}', content_rowid='rowid', tokenize='{tokenizer}');
             CREATE TRIGGER IF NOT EXISTS \"{index}_ai\" AFTER INSERT ON \"{table}\" BEGIN
               INSERT INTO \"{index}\"(rowid, {col_list}) VALUES (new.rowid, {new_vals});
             END;
             CREATE TRIGGER IF NOT EXISTS \"{index}_ad\" AFTER DELETE ON \"{table}\" BEGIN
               INSERT INTO \"{index}\"(\"{index}\", rowid, {col_list})
                 VALUES ('delete', old.rowid, {old_vals});
             END;
             CREATE TRIGGER IF NOT EXISTS \"{index}_au\" AFTER UPDATE ON \"{table}\" BEGIN
               INSERT INTO \"{index}\"(\"{index}\", rowid, {col_list})
                 VALUES ('delete', old.rowid, {old_vals});
               INSERT INTO \"{index}\"(rowid, {col_list}) VALUES (new.rowid, {new_vals});
             END;
             INSERT INTO \"{index}\"(\"{index}\") VALUES ('rebuild');"
        );
        conn.execute_batch(&ddl).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Whether an existing FTS index already declares the store's stemming
/// tokenizer (0.2.0 P3).
///
/// Read off the `CREATE VIRTUAL TABLE` text SQLite keeps in `sqlite_master`,
/// which is the only place the tokenizer of an index is visible — `pragma_table_info`
/// reports columns, never the tokenizer. Missing index ⇒ `false`, so a fresh
/// table takes the ordinary create path.
///
/// The comparison is against the exact spelling [`apply_fts_ddl`] emits. A
/// hand-written index that means the same thing differently (double quotes,
/// extra spacing) is therefore rebuilt once and then reads as declared — the
/// check is self-healing rather than clever, which is what keeps it idempotent.
fn index_declares_stem_tokenizer(
    conn: &rusqlite::Connection,
    index: &str,
) -> rusqlite::Result<bool> {
    use rusqlite::OptionalExtension;
    let declaration: Option<String> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name = ?1",
            [index],
            |r| r.get(0),
        )
        .optional()?;
    let needle = format!(
        "tokenize='{}'",
        crate::store::query::fts_tokenizer::TOKENIZER_NAME
    );
    Ok(declaration.is_some_and(|sql| sql.contains(&needle)))
}

/// The column list of an existing FTS index table, or `None` if it does not exist.
fn existing_fts_columns(
    conn: &rusqlite::Connection,
    index: &str,
) -> rusqlite::Result<Option<Vec<String>>> {
    use rusqlite::OptionalExtension;
    let present: Option<String> = conn
        .query_row(
            "SELECT name FROM sqlite_master WHERE type='table' AND name = ?1",
            [index],
            |r| r.get(0),
        )
        .optional()?;
    if present.is_none() {
        return Ok(None);
    }
    let mut st = conn.prepare("SELECT name FROM pragma_table_info(?1)")?;
    let cols = st
        .query_map([index], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(Some(cols))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// A connection equipped the way the factory equips one.
    ///
    /// The `store` cell installs `hamming` and the `meclaw_stem` FTS5 tokenizer
    /// on every connection it opens, BEFORE any DDL runs. That order is not
    /// cosmetic: an index declaring the tokenizer cannot be opened at all
    /// without it, so a test on a bare connection would measure a state the
    /// running system never has.
    fn conn_with_extensions() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::query::install_connection_extensions(&conn).unwrap();
        conn
    }

    /// Tripwire: FTS5 must be compiled into the bundled SQLite. `libsqlite3-sys`
    /// passes `-DSQLITE_ENABLE_FTS5` unconditionally in the bundled build, so this
    /// needs no `Cargo.toml` feature — but a dependency bump could silently drop
    /// it, and the whole `search` op rests on it. Empirical, not inferred.
    #[test]
    fn fts5_is_compiled_in() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE VIRTUAL TABLE probe USING fts5(x);\
             INSERT INTO probe(x) VALUES('hello world');",
        )
        .expect("bundled SQLite must ship FTS5");
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM probe WHERE probe MATCH 'world'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "FTS5 MATCH must return the indexed row");
    }

    fn fts_decl(table: &str, cols: &[&str]) -> BTreeMap<String, Vec<String>> {
        BTreeMap::from([(
            table.to_string(),
            cols.iter().map(|c| c.to_string()).collect(),
        )])
    }

    #[test]
    fn fts_ddl_creates_index_triggers_and_backfills_existing_rows() {
        let conn = conn_with_extensions();
        // an "old" cell.db: base table with data, no FTS anywhere
        conn.execute("CREATE TABLE facts (id TEXT, claim TEXT)", [])
            .unwrap();
        conn.execute("INSERT INTO facts VALUES ('f1','acme ships v1')", [])
            .unwrap();

        apply_fts_ddl(&conn, &fts_decl("facts", &["claim"]), &BTreeMap::new()).unwrap();

        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM facts_fts WHERE facts_fts MATCH 'ships'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            n, 1,
            "existing rows must be indexed by the one-time rebuild"
        );

        // triggers keep the index in sync with insert / update / delete
        conn.execute("INSERT INTO facts VALUES ('f2','acme builds alpha')", [])
            .unwrap();
        conn.execute(
            "UPDATE facts SET claim='acme builds beta' WHERE id='f2'",
            [],
        )
        .unwrap();
        let hit: i64 = conn
            .query_row(
                "SELECT count(*) FROM facts_fts WHERE facts_fts MATCH 'beta'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hit, 1);
        let stale: i64 = conn
            .query_row(
                "SELECT count(*) FROM facts_fts WHERE facts_fts MATCH 'alpha'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stale, 0, "update must not leave a stale index row");
        conn.execute("DELETE FROM facts WHERE id='f2'", []).unwrap();
        let gone: i64 = conn
            .query_row(
                "SELECT count(*) FROM facts_fts WHERE facts_fts MATCH 'beta'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(gone, 0, "delete must remove the index row");

        // second call is a no-op: no error, and no second rebuild
        apply_fts_ddl(&conn, &fts_decl("facts", &["claim"]), &BTreeMap::new()).unwrap();
        let again: i64 = conn
            .query_row(
                "SELECT count(*) FROM facts_fts WHERE facts_fts MATCH 'ships'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(again, 1, "rebuild must not run twice");
    }

    #[test]
    fn fts_ddl_rebuilds_on_additive_column_drift() {
        let conn = conn_with_extensions();
        conn.execute_batch(
            "CREATE TABLE facts (id TEXT PRIMARY KEY, claim TEXT, predicate TEXT);
             INSERT INTO facts VALUES ('1','Helix','has preferred editor');",
        )
        .unwrap();
        apply_fts_ddl(&conn, &fts_decl("facts", &["claim"]), &BTreeMap::new()).unwrap();
        // the declaration grows by one column -> rebuild instead of failing
        apply_fts_ddl(
            &conn,
            &fts_decl("facts", &["claim", "predicate"]),
            &BTreeMap::new(),
        )
        .unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM facts_fts WHERE facts_fts MATCH '\"preferred\"*'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "the new column must be searchable after the rebuild");

        // Trigger receipt: DROP TABLE leaves the base table's triggers in place,
        // and `CREATE TRIGGER IF NOT EXISTS` would then keep the OLD column list
        // alive. A row written AFTER the migration must reach the new column, so
        // the rebuild alone is not proof.
        conn.execute(
            "INSERT INTO facts VALUES ('2','Zed','has favorite editors')",
            [],
        )
        .unwrap();
        let fresh: i64 = conn
            .query_row(
                "SELECT count(*) FROM facts_fts WHERE facts_fts MATCH '\"favorite\"*'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            fresh, 1,
            "the insert trigger must maintain the new column too"
        );
    }

    /// 0.2.0 P2 (ruling Q3): the keyword leg moves from the WRITTEN predicate to
    /// its alias-resolved twin. On an existing `cell.db` that is a same-length
    /// column swap — not additive drift — so without the canonical class the spawn
    /// would fail loudly and the store would come up without its index.
    #[test]
    fn fts_ddl_rebuilds_on_canonical_column_drift() {
        let conn = conn_with_extensions();
        conn.execute_batch(
            "CREATE TABLE facts (id TEXT PRIMARY KEY, claim TEXT, predicate TEXT,
                                 canonical_predicate TEXT);
             INSERT INTO facts VALUES ('1','Helix','Lieblingseditor','favorite_editor');",
        )
        .unwrap();
        // the P15 state: the index carries the written spelling
        apply_fts_ddl(
            &conn,
            &fts_decl("facts", &["claim", "predicate"]),
            &BTreeMap::new(),
        )
        .unwrap();
        // the P2 declaration: same length, one column REPLACED
        apply_fts_ddl(
            &conn,
            &fts_decl("facts", &["claim", "canonical_predicate"]),
            &canon_decl(),
        )
        .unwrap();

        let hit: i64 = conn
            .query_row(
                "SELECT count(*) FROM facts_fts WHERE facts_fts MATCH '\"favorite\"*'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hit, 1, "the canonical column must be searchable");
        let stale: i64 = conn
            .query_row(
                "SELECT count(*) FROM facts_fts WHERE facts_fts MATCH '\"Lieblingseditor\"*'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            stale, 0,
            "the written spelling is no longer what the keyword leg indexes"
        );

        // the triggers moved with the index: a row written AFTER the migration
        // reaches the new column (the rebuild alone would not prove that)
        conn.execute(
            "INSERT INTO facts VALUES ('2','vscode','editor','preferred_editor')",
            [],
        )
        .unwrap();
        let fresh: i64 = conn
            .query_row(
                "SELECT count(*) FROM facts_fts WHERE facts_fts MATCH '\"preferred\"*'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(fresh, 1);
    }

    /// Statement identity W2 (GitHub #13, ruling Q1): the claim gains a canonical
    /// binding, and the FTS declaration deliberately keeps indexing the WRITTEN
    /// claim — the claim IS the text a keyword question searches, so replacing it
    /// with its alias-resolved twin would cost recall on the original wording.
    ///
    /// That combination is what this test exists for. The substitution rule maps
    /// EVERY binding's source onto its target when it compares an existing index
    /// against the declaration, so a store still carrying the P15 index
    /// (`claim, predicate`) now substitutes to `canonical_claim,
    /// canonical_predicate` and matches neither the declaration nor its additive
    /// prefix. The migration has to keep working across the skipped release, so
    /// the rule considers each column's substitution on its own rather than all
    /// of them at once.
    #[test]
    fn fts_ddl_migrates_a_p15_index_onto_a_three_dimension_declaration() {
        let conn = conn_with_extensions();
        conn.execute_batch(
            "CREATE TABLE facts (id TEXT PRIMARY KEY, claim TEXT, canonical_claim TEXT,
                                 predicate TEXT, canonical_predicate TEXT,
                                 subject TEXT, canonical_subject TEXT);
             INSERT INTO facts VALUES ('1','Helix','Helix','Lieblingseditor',
                                       'favorite_editor','user','user');",
        )
        .unwrap();
        // the P15 state: the index carries the written spellings and nothing else
        apply_fts_ddl(
            &conn,
            &fts_decl("facts", &["claim", "predicate"]),
            &BTreeMap::new(),
        )
        .unwrap();
        let spec = |source: &str, target: &str, aliases: &str| crate::store::CanonicalSpec {
            source: source.to_string(),
            target: target.to_string(),
            aliases: aliases.to_string(),
            normalize: false,
            rejected: None,
        };
        let canonical = BTreeMap::from([(
            "facts".to_string(),
            vec![
                spec("predicate", "canonical_predicate", "predicate_aliases"),
                spec("subject", "canonical_subject", "subject_aliases"),
                spec("claim", "canonical_claim", "claim_aliases"),
            ],
        )]);
        apply_fts_ddl(
            &conn,
            &fts_decl(
                "facts",
                &["claim", "canonical_predicate", "canonical_subject"],
            ),
            &canonical,
        )
        .expect("a store that skipped a release must migrate, not refuse to come up");
        let hit: i64 = conn
            .query_row(
                "SELECT count(*) FROM facts_fts WHERE facts_fts MATCH '\"favorite\"*'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hit, 1, "the canonical predicate must be searchable");
        let claim: i64 = conn
            .query_row(
                "SELECT count(*) FROM facts_fts WHERE facts_fts MATCH '\"Helix\"*'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            claim, 1,
            "the WRITTEN claim stays indexed: the claim binding exists for identity, \
             not for search"
        );
    }

    /// 0.2.0 P3 (ruling Q6): every index this function builds declares the
    /// stemming tokenizer. Read off `sqlite_master`, because that is the only
    /// place an index's tokenizer is visible — and the only thing the drift
    /// check can look at.
    #[test]
    fn fts_ddl_declares_the_stemming_tokenizer() {
        let conn = conn_with_extensions();
        conn.execute("CREATE TABLE facts (id TEXT, claim TEXT)", [])
            .unwrap();
        apply_fts_ddl(&conn, &fts_decl("facts", &["claim"]), &BTreeMap::new()).unwrap();
        let sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name = 'facts_fts'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(sql.contains("tokenize='meclaw_stem'"), "got: {sql}");
        assert!(index_declares_stem_tokenizer(&conn, "facts_fts").unwrap());
        assert!(!index_declares_stem_tokenizer(&conn, "nothing_fts").unwrap());
    }

    /// 0.2.0 P3: the tokenizer drift class. An index written by a previous
    /// release carries the same COLUMNS and a different tokenizer, so neither
    /// the additive nor the canonical class sees anything — without this class
    /// an existing store would keep an unstemmed index forever while every
    /// query arrived folded, which is worse than the state before the package.
    #[test]
    fn fts_ddl_rebuilds_an_index_built_without_the_stemmer() {
        let conn = conn_with_extensions();
        conn.execute_batch(
            "CREATE TABLE facts (id TEXT PRIMARY KEY, claim TEXT);
             INSERT INTO facts VALUES ('1','hat lieblingseditor helix');
             CREATE VIRTUAL TABLE facts_fts USING fts5(claim, content='facts',
                 content_rowid='rowid');
             INSERT INTO facts_fts(facts_fts) VALUES ('rebuild');",
        )
        .unwrap();
        let before: i64 = conn
            .query_row(
                "SELECT count(*) FROM facts_fts WHERE facts_fts MATCH '\"lieblingseditoren\"*'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            before, 0,
            "this is the issue-#14 state the class exists for"
        );

        apply_fts_ddl(&conn, &fts_decl("facts", &["claim"]), &BTreeMap::new()).unwrap();

        let after: i64 = conn
            .query_row(
                "SELECT count(*) FROM facts_fts WHERE facts_fts MATCH '\"lieblingseditoren\"*'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(after, 1, "the plural query now reaches the singular row");

        // The triggers moved with the index: a row written AFTER the migration
        // is folded too. The rebuild alone would not prove that.
        conn.execute("INSERT INTO facts VALUES ('2','zwei katzen')", [])
            .unwrap();
        let fresh: i64 = conn
            .query_row(
                "SELECT count(*) FROM facts_fts WHERE facts_fts MATCH '\"katze\"*'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(fresh, 1, "the insert trigger indexes through the tokenizer");

        // …and the second boot is a no-op again: the declaration is now met.
        apply_fts_ddl(&conn, &fts_decl("facts", &["claim"]), &BTreeMap::new()).unwrap();
        let again: i64 = conn
            .query_row(
                "SELECT count(*) FROM facts_fts WHERE facts_fts MATCH '\"lieblingseditoren\"*'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(again, 1);
    }

    /// The tokenizer never buys a refused column drift. A store whose FTS
    /// declaration lost a column stays a loud spawn error, migration or not.
    #[test]
    fn a_missing_tokenizer_does_not_excuse_a_refused_column_drift() {
        let conn = conn_with_extensions();
        conn.execute_batch(
            "CREATE TABLE facts (id TEXT PRIMARY KEY, claim TEXT, predicate TEXT);
             CREATE VIRTUAL TABLE facts_fts USING fts5(claim, predicate, content='facts',
                 content_rowid='rowid');",
        )
        .unwrap();
        let err =
            apply_fts_ddl(&conn, &fts_decl("facts", &["claim"]), &BTreeMap::new()).unwrap_err();
        assert!(err.contains("fts column drift"), "got: {err}");
    }

    /// The exception is tied to the DECLARATION, not to the column names: without
    /// the binding the very same swap is the loud drift it always was.
    #[test]
    fn fts_ddl_canonical_drift_needs_the_declaration() {
        let conn = conn_with_extensions();
        conn.execute_batch(
            "CREATE TABLE facts (id TEXT PRIMARY KEY, claim TEXT, predicate TEXT,
                                 canonical_predicate TEXT);",
        )
        .unwrap();
        apply_fts_ddl(
            &conn,
            &fts_decl("facts", &["claim", "predicate"]),
            &BTreeMap::new(),
        )
        .unwrap();
        let err = apply_fts_ddl(
            &conn,
            &fts_decl("facts", &["claim", "canonical_predicate"]),
            &BTreeMap::new(),
        )
        .unwrap_err();
        assert!(err.contains("fts column drift"), "got: {err}");
    }

    #[test]
    fn fts_ddl_still_fails_loudly_on_non_additive_drift() {
        let conn = conn_with_extensions();
        conn.execute_batch("CREATE TABLE facts (id TEXT PRIMARY KEY, claim TEXT, predicate TEXT);")
            .unwrap();
        apply_fts_ddl(
            &conn,
            &fts_decl("facts", &["claim", "predicate"]),
            &BTreeMap::new(),
        )
        .unwrap();
        // dropping a column is NOT additive drift
        let err =
            apply_fts_ddl(&conn, &fts_decl("facts", &["claim"]), &BTreeMap::new()).unwrap_err();
        assert!(err.contains("fts column drift"), "got: {err}");
        // neither is reordering
        let err2 = apply_fts_ddl(
            &conn,
            &fts_decl("facts", &["predicate", "claim"]),
            &BTreeMap::new(),
        )
        .unwrap_err();
        assert!(err2.contains("fts column drift"), "got: {err2}");
    }

    #[test]
    fn fts_ddl_without_declaration_is_a_noop() {
        let conn = conn_with_extensions();
        conn.execute("CREATE TABLE facts (id TEXT)", []).unwrap();
        apply_fts_ddl(&conn, &BTreeMap::new(), &BTreeMap::new()).unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name LIKE '%_fts%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0);
    }

    /// 0.2.0 P2: an existing `cell.db` must grow into a declaration that gained a
    /// column, without a migration tool and without losing a row. `CREATE TABLE IF
    /// NOT EXISTS` is a no-op on an existing table, so the ALTER is the only thing
    /// that gets `canonical_predicate` onto a store that has been running.
    #[test]
    fn schema_ddl_adds_a_declared_column_an_existing_table_lacks() {
        let conn = conn_with_extensions();
        conn.execute_batch(
            "CREATE TABLE facts (id TEXT, predicate TEXT);
             INSERT INTO facts VALUES ('f1','favorite editor');",
        )
        .unwrap();
        let mut cols = BTreeMap::new();
        cols.insert("id".to_string(), "text".to_string());
        cols.insert("predicate".to_string(), "text".to_string());
        cols.insert("canonical_predicate".to_string(), "text".to_string());
        let schema = BTreeMap::from([("facts".to_string(), cols)]);

        apply_schema_ddl(&conn, &schema).unwrap();

        let (pred, canon): (String, Option<String>) = conn
            .query_row(
                "SELECT predicate, canonical_predicate FROM facts WHERE id='f1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("the row must survive the migration");
        assert_eq!(pred, "favorite editor", "the original stays byte-identical");
        assert_eq!(canon, None, "a fresh column starts NULL, never invented");

        // idempotent: a second boot must not fail on the column that now exists
        apply_schema_ddl(&conn, &schema).unwrap();
    }

    /// No-delete in DDL form: the migration only ever ADDS. A column that lives in
    /// the database but not in the declaration keeps its rows.
    #[test]
    fn schema_ddl_never_removes_an_undeclared_column() {
        let conn = conn_with_extensions();
        conn.execute_batch(
            "CREATE TABLE facts (id TEXT, legacy TEXT);
             INSERT INTO facts VALUES ('f1','keep me');",
        )
        .unwrap();
        let mut cols = BTreeMap::new();
        cols.insert("id".to_string(), "text".to_string());
        schema_apply(&conn, cols);
        let kept: String = conn
            .query_row("SELECT legacy FROM facts WHERE id='f1'", [], |r| r.get(0))
            .expect("the undeclared column must still be there");
        assert_eq!(kept, "keep me");
    }

    fn canon_decl() -> BTreeMap<String, Vec<crate::store::CanonicalSpec>> {
        BTreeMap::from([(
            "facts".to_string(),
            vec![crate::store::CanonicalSpec {
                source: "predicate".to_string(),
                target: "canonical_predicate".to_string(),
                aliases: "predicate_aliases".to_string(),
                normalize: false,
                rejected: None,
            }],
        )])
    }

    fn canon_of(conn: &rusqlite::Connection, id: &str) -> Option<String> {
        conn.query_row(
            "SELECT canonical_predicate FROM facts WHERE id = ?1",
            [id],
            |r| r.get(0),
        )
        .unwrap()
    }

    /// 0.2.0 P2: the boot backfill is what makes an EXISTING store answer through
    /// the canonical column at all — without it every identity-sensitive read
    /// would be blind until the first dream run.
    #[test]
    fn canonical_ddl_creates_the_alias_table_and_backfills_empty_targets() {
        let conn = conn_with_extensions();
        conn.execute_batch(
            "CREATE TABLE facts (id TEXT, predicate TEXT, canonical_predicate TEXT);
             INSERT INTO facts VALUES ('f1','favorite editor',NULL);
             INSERT INTO facts VALUES ('f2','Lieblingseditor','');",
        )
        .unwrap();

        apply_canonical_ddl(&conn, &canon_decl()).unwrap();

        // no alias known yet -> the canonical value IS the original
        assert_eq!(canon_of(&conn, "f1").as_deref(), Some("favorite editor"));
        assert_eq!(canon_of(&conn, "f2").as_deref(), Some("Lieblingseditor"));
        // the alias table exists and is upsertable (PRIMARY KEY on alias)
        conn.execute_batch(
            "INSERT INTO predicate_aliases (alias, canonical) VALUES ('Lieblingseditor','favorite_editor')
             ON CONFLICT(alias) DO UPDATE SET canonical = excluded.canonical;",
        )
        .unwrap();

        // a second boot does NOT re-derive: the column is filled, so the alias that
        // arrived later is the explicit `canonicalize` op's job, never a boot effect
        apply_canonical_ddl(&conn, &canon_decl()).unwrap();
        assert_eq!(canon_of(&conn, "f2").as_deref(), Some("Lieblingseditor"));

        // the original is untouched by all of it
        let orig: String = conn
            .query_row("SELECT predicate FROM facts WHERE id='f2'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(orig, "Lieblingseditor");
    }

    /// 0.2.0 P5: the refusal log is additive DDL, exactly like the alias table —
    /// an existing `cell.db` grows it on the next wake, and a store whose binding
    /// does not declare one keeps the P2/P4 shape.
    #[test]
    fn canonical_ddl_creates_the_rejected_pair_table_only_when_declared() {
        let conn = conn_with_extensions();
        conn.execute_batch(
            "CREATE TABLE facts (id TEXT, predicate TEXT, canonical_predicate TEXT);",
        )
        .unwrap();

        apply_canonical_ddl(&conn, &canon_decl()).unwrap();
        assert!(
            !table_exists(&conn, "predicate_rejected_pairs"),
            "an undeclared refusal log is not created behind the operator's back"
        );

        let mut decl = canon_decl();
        decl.get_mut("facts").unwrap()[0].rejected = Some("predicate_rejected_pairs".into());
        apply_canonical_ddl(&conn, &decl).unwrap();
        assert!(table_exists(&conn, "predicate_rejected_pairs"));
        // The pair is the PRIMARY KEY, which is what makes a repeated refusal an
        // upsert rather than a second row — the same property `set_alias` needs.
        conn.execute_batch(
            "INSERT INTO predicate_rejected_pairs (left_value, right_value, recorded_at)
                 VALUES ('a','b','t1')
             ON CONFLICT(left_value, right_value) DO UPDATE SET recorded_at = excluded.recorded_at;
             INSERT INTO predicate_rejected_pairs (left_value, right_value, recorded_at)
                 VALUES ('a','b','t2')
             ON CONFLICT(left_value, right_value) DO UPDATE SET recorded_at = excluded.recorded_at;",
        )
        .unwrap();
        let n: i64 = conn
            .query_row("SELECT count(*) FROM predicate_rejected_pairs", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(n, 1);
        // and a second wake over the same declaration changes nothing
        apply_canonical_ddl(&conn, &decl).unwrap();
        let n: i64 = conn
            .query_row("SELECT count(*) FROM predicate_rejected_pairs", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(n, 1);
    }

    fn table_exists(conn: &rusqlite::Connection, name: &str) -> bool {
        conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
            [name],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
            > 0
    }

    /// A row minted while an alias already exists is backfilled THROUGH it.
    #[test]
    fn canonical_ddl_backfill_resolves_a_known_alias() {
        let conn = conn_with_extensions();
        conn.execute_batch(
            "CREATE TABLE facts (id TEXT, predicate TEXT, canonical_predicate TEXT);
             CREATE TABLE predicate_aliases (alias TEXT PRIMARY KEY, canonical TEXT NOT NULL,
                                             recorded_at TEXT);
             INSERT INTO predicate_aliases VALUES ('Lieblingseditor','favorite_editor',NULL);
             INSERT INTO facts VALUES ('f1','Lieblingseditor',NULL);",
        )
        .unwrap();
        apply_canonical_ddl(&conn, &canon_decl()).unwrap();
        assert_eq!(canon_of(&conn, "f1").as_deref(), Some("favorite_editor"));
    }

    #[test]
    fn canonical_ddl_without_declaration_is_a_noop() {
        let conn = conn_with_extensions();
        apply_canonical_ddl(&conn, &BTreeMap::new()).unwrap();
        let n: i64 = conn
            .query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    fn schema_apply(conn: &rusqlite::Connection, cols: BTreeMap<String, String>) {
        apply_schema_ddl(conn, &BTreeMap::from([("facts".to_string(), cols)])).unwrap();
    }

    #[test]
    fn applies_two_tables_with_types() {
        let mut schema = BTreeMap::new();
        let mut t1 = BTreeMap::new();
        t1.insert("id".to_string(), "int".to_string());
        t1.insert("name".to_string(), "text".to_string());
        schema.insert("users".to_string(), t1);
        let mut t2 = BTreeMap::new();
        t2.insert("body".to_string(), "json".to_string());
        schema.insert("events".to_string(), t2);

        let conn = conn_with_extensions();
        apply_schema_ddl(&conn, &schema).unwrap();

        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('users','events')",
                [], |r| r.get(0),
            ).unwrap();
        assert_eq!(n, 2);
    }
}

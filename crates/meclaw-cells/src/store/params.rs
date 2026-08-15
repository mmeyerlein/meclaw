//! Phase-9 store params: 2-stage schema map + optional query_timeout_ms.

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    #[test]
    fn parses_minimal_schema_one_table() {
        let raw = json!({
            "schema": { "items": { "id": "int", "name": "text" } }
        });
        let p = StoreParams::parse(&raw).unwrap();
        assert_eq!(p.schema.len(), 1);
        let cols = &p.schema["items"];
        assert_eq!(cols.get("id"), Some(&"int".to_string()));
        assert_eq!(cols.get("name"), Some(&"text".to_string()));
        assert!(p.query_timeout_ms.is_none());
    }

    #[test]
    fn parses_query_timeout_ms() {
        let raw = json!({
            "schema": { "t": { "c": "text" } },
            "query_timeout_ms": 12345
        });
        let p = StoreParams::parse(&raw).unwrap();
        assert_eq!(p.query_timeout_ms, Some(12345));
    }

    #[test]
    fn rejects_missing_schema() {
        let r = StoreParams::parse(&json!({}));
        assert!(r.is_err());
    }

    #[test]
    fn rejects_non_object_schema() {
        let r = StoreParams::parse(&json!({ "schema": [] }));
        assert!(r.is_err());
    }

    #[test]
    fn rejects_non_object_table_def() {
        let r = StoreParams::parse(&json!({ "schema": { "t": "text" } }));
        assert!(r.is_err());
    }

    #[test]
    fn rejects_non_string_column_type() {
        let r = StoreParams::parse(&json!({ "schema": { "t": { "c": 1 } } }));
        assert!(r.is_err());
    }

    /// R9: identifiers declared here are formatted into `CREATE TABLE` DDL, so
    /// they are gated by syntax at parse time — one parse path, so a hostile
    /// schema never reaches `validate` OR `spawn`.
    #[test]
    fn params_schema_rejects_injection_shaped_identifiers() {
        assert!(StoreParams::parse(&json!({"schema":{"t\"x":{"a":"int"}}})).is_err());
        assert!(StoreParams::parse(&json!({"schema":{"t":{"a\"b":"int"}}})).is_err());
        assert!(StoreParams::parse(&json!({"schema":{"sqlite_x":{"a":"int"}}})).is_err());
        assert!(StoreParams::parse(&json!({"schema":{"t_fts":{"a":"int"}}})).is_err());
        assert!(StoreParams::parse(&json!({"schema":{"9t":{"a":"int"}}})).is_err());
        // the shipped shapes stay valid
        assert!(
            StoreParams::parse(&json!({"schema":{"pending_extraction":{"batch_id":"text"}}}))
                .is_ok()
        );
    }

    #[test]
    fn parses_and_validates_fts_declaration() {
        let p = StoreParams::parse(&json!({
            "schema": {"facts": {"id":"text","claim":"text"}},
            "fts": {"facts": ["claim"]}
        }))
        .unwrap();
        assert_eq!(p.fts["facts"], vec!["claim".to_string()]);

        // table not declared in schema
        assert!(
            StoreParams::parse(&json!({"schema":{"t":{"a":"text"}},"fts":{"x":["a"]}})).is_err()
        );
        // column not in that table
        assert!(
            StoreParams::parse(&json!({"schema":{"t":{"a":"text"}},"fts":{"t":["b"]}})).is_err()
        );
        // empty column list
        assert!(StoreParams::parse(&json!({"schema":{"t":{"a":"text"}},"fts":{"t":[]}})).is_err());
        // int columns cannot be indexed
        assert!(
            StoreParams::parse(&json!({"schema":{"t":{"a":"int"}},"fts":{"t":["a"]}})).is_err()
        );
        // a `rank` column would collide with the bm25 result column
        assert!(
            StoreParams::parse(&json!({"schema":{"t":{"a":"text","rank":"text"}},
                                       "fts":{"t":["a"]}}))
            .is_err()
        );
        // fts must be an object of arrays of strings
        assert!(StoreParams::parse(&json!({"schema":{"t":{"a":"text"}},"fts":[]})).is_err());
        assert!(StoreParams::parse(&json!({"schema":{"t":{"a":"text"}},"fts":{"t":[1]}})).is_err());
        // absent fts is the default
        assert!(
            StoreParams::parse(&json!({"schema":{"t":{"a":"text"}}}))
                .unwrap()
                .fts
                .is_empty()
        );
    }

    /// 0.2.0 P2 (ruling Q3): the canonical binding is the store's single statement
    /// about predicate identity, so every part of it is closed-set validated here —
    /// one parse path, so a broken binding never reaches DDL or an op.
    #[test]
    fn parses_and_validates_canonical_declaration() {
        let ok = json!({
            "schema": {"facts": {"predicate": "text", "canonical_predicate": "text"}},
            "canonical": {"facts": {"source": "predicate",
                                    "target": "canonical_predicate",
                                    "aliases": "predicate_aliases"}}
        });
        let p = StoreParams::parse(&ok).unwrap();
        assert_eq!(
            p.canonical["facts"].len(),
            1,
            "the single form is ONE binding"
        );
        assert_eq!(p.canonical["facts"][0].source, "predicate");
        assert_eq!(p.canonical["facts"][0].target, "canonical_predicate");
        assert_eq!(p.canonical["facts"][0].aliases, "predicate_aliases");
        assert!(
            !p.canonical["facts"][0].normalize,
            "normalisation is opt-in — the predicate binding of P2 keeps its bytes"
        );

        let broken = |patch: meclaw_core::serde_json::Value| {
            let mut v = ok.clone();
            v["canonical"] = patch;
            StoreParams::parse(&v)
        };
        // table not declared in schema
        assert!(
            broken(
                json!({"nope": {"source": "predicate", "target": "canonical_predicate",
                                   "aliases": "predicate_aliases"}})
            )
            .is_err()
        );
        // source / target not columns of that table
        assert!(
            broken(
                json!({"facts": {"source": "nope", "target": "canonical_predicate",
                                    "aliases": "predicate_aliases"}})
            )
            .is_err()
        );
        assert!(
            broken(json!({"facts": {"source": "predicate", "target": "nope",
                                    "aliases": "predicate_aliases"}}))
            .is_err()
        );
        // the original must stay readable NEXT to the derived value
        assert!(
            broken(
                json!({"facts": {"source": "predicate", "target": "predicate",
                                    "aliases": "predicate_aliases"}})
            )
            .is_err()
        );
        // the alias table is store-owned: syntax-gated, and never double-declared
        assert!(
            broken(
                json!({"facts": {"source": "predicate", "target": "canonical_predicate",
                                    "aliases": "alias\"; DROP TABLE facts; --"}})
            )
            .is_err()
        );
        assert!(
            broken(
                json!({"facts": {"source": "predicate", "target": "canonical_predicate",
                                    "aliases": "facts"}})
            )
            .is_err()
        );
        // shape errors
        assert!(broken(json!([])).is_err());
        assert!(broken(json!({"facts": "predicate"})).is_err());
        assert!(
            broken(
                json!({"facts": {"source": "predicate", "target": "canonical_predicate",
                                    "aliases": "predicate_aliases", "extra": 1}})
            )
            .is_err()
        );
        // absent canonical is the default
        assert!(
            StoreParams::parse(&json!({"schema": {"t": {"a": "text"}}}))
                .unwrap()
                .canonical
                .is_empty()
        );
    }

    /// 0.2.0 P4 (ruling Q5): one table carries TWO identity dimensions — the
    /// relation (P2) and the entity. The declaration therefore accepts a LIST of
    /// bindings next to the single-binding form P2 shipped, and the single form
    /// keeps parsing so a `config.json` in the field survives the upgrade.
    #[test]
    fn parses_two_bindings_on_one_table() {
        let raw = json!({
            "schema": {"facts": {"subject": "text", "canonical_subject": "text",
                                 "predicate": "text", "canonical_predicate": "text"}},
            "canonical": {"facts": [
                {"source": "predicate", "target": "canonical_predicate",
                 "aliases": "predicate_aliases"},
                {"source": "subject", "target": "canonical_subject",
                 "aliases": "subject_aliases", "normalize": true}
            ]}
        });
        let p = StoreParams::parse(&raw).unwrap();
        let b = &p.canonical["facts"];
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].source, "predicate");
        assert!(!b[0].normalize);
        assert_eq!(b[1].source, "subject");
        assert_eq!(b[1].target, "canonical_subject");
        assert_eq!(b[1].aliases, "subject_aliases");
        assert!(
            b[1].normalize,
            "the entity dimension normalises (ruling Q5)"
        );
        // and the whole thing round-trips through the params overlay core
        let v = meclaw_core::serde_json::to_value(&p).unwrap();
        assert_eq!(StoreParams::parse(&v).unwrap().canonical, p.canonical);
    }

    /// Every way two bindings could collide is refused at parse time — a store
    /// that survives here can always be turned into DDL and SQL, which is the
    /// same standard the single-binding form is held to.
    #[test]
    fn two_bindings_may_not_collide() {
        let base = json!({
            "schema": {"facts": {"subject": "text", "canonical_subject": "text",
                                 "predicate": "text", "canonical_predicate": "text"},
                       "beliefs": {"holder": "text", "canonical_holder": "text"}},
        });
        let with = |canonical: meclaw_core::serde_json::Value| {
            let mut v = base.clone();
            v["canonical"] = canonical;
            StoreParams::parse(&v)
        };
        let subject = json!({"source": "subject", "target": "canonical_subject",
                             "aliases": "subject_aliases"});
        // the shape that must WORK, so every refusal below is about the collision
        assert!(
            with(json!({"facts": [subject.clone(),
                                  {"source": "predicate", "target": "canonical_predicate",
                                   "aliases": "predicate_aliases"}]}))
            .is_ok()
        );
        // same source twice
        assert!(with(json!({"facts": [subject.clone(), subject.clone()]})).is_err());
        // two bindings writing the same derived column
        assert!(
            with(json!({"facts": [subject.clone(),
                                  {"source": "predicate", "target": "canonical_subject",
                                   "aliases": "predicate_aliases"}]}))
            .is_err()
        );
        // one binding's target is another's source — a derived value would feed
        // an identity of its own
        assert!(
            with(
                json!({"facts": [{"source": "subject", "target": "predicate",
                                   "aliases": "subject_aliases"},
                                  {"source": "predicate", "target": "canonical_predicate",
                                   "aliases": "predicate_aliases"}]})
            )
            .is_err()
        );
        // one alias table serving two bindings, here even across tables
        assert!(
            with(json!({"facts": [subject.clone()],
                        "beliefs": [{"source": "holder", "target": "canonical_holder",
                                     "aliases": "subject_aliases"}]}))
            .is_err()
        );
        // an empty list says nothing and is a declaration error, not a no-op
        assert!(with(json!({"facts": []})).is_err());
        // normalize is a bool, not a mode string
        assert!(
            with(
                json!({"facts": [{"source": "subject", "target": "canonical_subject",
                                   "aliases": "subject_aliases", "normalize": "yes"}]})
            )
            .is_err()
        );
    }

    /// 0.2.0 P5: a binding may name a second store-owned table, the one that
    /// remembers a NEGATIVE judgement. Optional, so every declaration written for
    /// P2 and P4 keeps parsing, and held to exactly the same standard as the alias
    /// table: syntax-gated, never declared in `schema`, never shared.
    #[test]
    fn a_binding_may_declare_a_rejected_pair_table() {
        let base = json!({
            "schema": {"facts": {"subject": "text", "canonical_subject": "text",
                                 "predicate": "text", "canonical_predicate": "text"}},
        });
        let with = |canonical: meclaw_core::serde_json::Value| {
            let mut v = base.clone();
            v["canonical"] = canonical;
            StoreParams::parse(&v)
        };
        let subject = json!({"source": "subject", "target": "canonical_subject",
                             "aliases": "subject_aliases", "rejected": "subject_rejected_pairs"});
        let p = with(json!({"facts": [subject.clone()]})).unwrap();
        assert_eq!(
            p.canonical["facts"][0].rejected.as_deref(),
            Some("subject_rejected_pairs")
        );
        // absent is the default and stays absent
        assert!(
            with(
                json!({"facts": [{"source": "subject", "target": "canonical_subject",
                                   "aliases": "subject_aliases"}]})
            )
            .unwrap()
            .canonical["facts"][0]
                .rejected
                .is_none()
        );
        // syntax-gated like the alias table
        assert!(
            with(
                json!({"facts": [{"source": "subject", "target": "canonical_subject",
                                   "aliases": "subject_aliases",
                                   "rejected": "r\"; DROP TABLE facts; --"}]})
            )
            .is_err()
        );
        // never a table the schema already declares
        assert!(
            with(
                json!({"facts": [{"source": "subject", "target": "canonical_subject",
                                   "aliases": "subject_aliases", "rejected": "facts"}]})
            )
            .is_err()
        );
        // never the alias table of any binding, and never shared between two
        assert!(
            with(
                json!({"facts": [{"source": "subject", "target": "canonical_subject",
                                   "aliases": "subject_aliases",
                                   "rejected": "subject_aliases"}]})
            )
            .is_err()
        );
        assert!(
            with(json!({"facts": [subject.clone(),
                                  {"source": "predicate", "target": "canonical_predicate",
                                   "aliases": "predicate_aliases",
                                   "rejected": "subject_rejected_pairs"}]}))
            .is_err()
        );
        assert!(
            with(json!({"facts": [subject.clone(),
                                  {"source": "predicate", "target": "canonical_predicate",
                                   "aliases": "subject_rejected_pairs"}]}))
            .is_err()
        );
    }

    /// An int column cannot carry an alias-resolved identity.
    #[test]
    fn canonical_rejects_a_non_text_column() {
        assert!(
            StoreParams::parse(&json!({
                "schema": {"facts": {"predicate": "text", "canonical_predicate": "int"}},
                "canonical": {"facts": {"source": "predicate",
                                        "target": "canonical_predicate",
                                        "aliases": "predicate_aliases"}}
            }))
            .is_err()
        );
    }

    #[test]
    fn canonical_is_immutable_at_runtime_and_round_trips() {
        let p = StoreParams::parse(&json!({
            "schema": {"facts": {"predicate": "text", "canonical_predicate": "text"}},
            "canonical": {"facts": {"source": "predicate",
                                    "target": "canonical_predicate",
                                    "aliases": "predicate_aliases"}}
        }))
        .unwrap();
        let upd = json!({"canonical": {}}).as_object().unwrap().clone();
        assert!(
            crate::params_overlay::apply_update(&p, &upd).is_err(),
            "canonical is bootstrap-only, like schema and fts"
        );
        // the overlay core round-trips params through to_value -> merge -> parse
        let v = meclaw_core::serde_json::to_value(&p).unwrap();
        assert_eq!(StoreParams::parse(&v).unwrap().canonical, p.canonical);

        let empty = StoreParams::parse(&json!({"schema": {"t": {"a": "text"}}})).unwrap();
        let ev = meclaw_core::serde_json::to_value(&empty).unwrap();
        assert!(
            ev.get("canonical").is_none(),
            "an empty binding must not serialize"
        );
    }

    #[test]
    fn fts_is_immutable_at_runtime() {
        let p = StoreParams::parse(&json!({"schema":{"t":{"a":"text"}}})).unwrap();
        let upd = json!({"fts": {"t": ["a"]}}).as_object().unwrap().clone();
        assert!(
            crate::params_overlay::apply_update(&p, &upd).is_err(),
            "fts is bootstrap-only, like schema"
        );
    }

    /// The overlay core round-trips params through `to_value` → merge → `parse`;
    /// an empty `fts` must serialize to an ABSENT key, not `{}`/`null`.
    #[test]
    fn empty_fts_round_trips_losslessly() {
        let p = StoreParams::parse(&json!({"schema":{"t":{"a":"text"}}})).unwrap();
        let v = meclaw_core::serde_json::to_value(&p).unwrap();
        assert!(v.get("fts").is_none(), "empty fts must not serialize");
        StoreParams::parse(&v).expect("round trip must parse");
    }

    /// The shipped template must keep parsing — the gate is a hardening, not a
    /// migration (compat receipt: plans/p3-fixtures/identifier-compat-scan.py).
    ///
    /// Fixture: `tests/fixtures/memory_hive_store_config.json` — a snapshot of
    /// `templates/memory-hive/store/config.json`, deliberately beside
    /// the crate: `templates/memory-hive/` is private and never exported, so reading the
    /// template directly made this test unrunnable in the public repo (same
    /// defect class as the P10 export fix, `b20bbe5`). The copy is a snapshot on
    /// purpose — it does NOT track the template. If the template's params grow a
    /// shape this gate should see, refresh the snapshot by hand.
    #[test]
    fn memory_hive_template_params_still_parse() {
        let raw = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/memory_hive_store_config.json"
        ))
        .unwrap();
        let v: Value = meclaw_core::serde_json::from_str(&raw).unwrap();
        StoreParams::parse(&v["params"]).expect("shipped template must stay valid");
    }
}

use meclaw_core::serde_json::Value;
use serde::Serialize;
use std::collections::BTreeMap;

/// Parsed `store` cell params.
///
/// Phase 9: `schema` is a 2-stage map `{ "<table>": { "<col>": "<type>" } }`.
///
/// Allowed column types (Phase 9): `"text"`, `"int"`, `"json"`.
/// Constraints (PK, NOT NULL, UNIQUE, defaults, indices) are deferred
/// (see brainstorm E6).
///
/// `Serialize` (β): the generic params-overlay core round-trips a params struct
/// through `serde_json::to_value` → merge → `parse`. `query_timeout_ms` carries
/// `skip_serializing_if` so a `None` serializes to an ABSENT key (not `null`) —
/// the manual `parse` rejects a `null` `query_timeout_ms`, so omitting it keeps
/// the round-trip lossless.
#[derive(Debug, Clone, Serialize)]
pub struct StoreParams {
    /// Schema definition: outer key is table name, inner key is column name,
    /// value is column type (`"text"`, `"int"`, or `"json"`).
    pub schema: BTreeMap<String, BTreeMap<String, String>>,
    /// Optional query timeout in milliseconds applied to user-facing queries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query_timeout_ms: Option<u64>,
    /// Opt-in canonical columns: `{ "<table>": [{source, target, aliases}, …] }`
    /// (0.2.0 P2, ruling Q3; the list form is 0.2.0 P4, ruling Q5).
    ///
    /// A binding says: this table carries a `source` column whose *identity* is
    /// an alias-resolved twin in `target`, and the mapping lives in the store-owned
    /// table `aliases` (columns `alias` → `canonical`). The store fills `target`
    /// itself on every write and can re-derive the whole column from `source` plus
    /// the alias table (`canonicalize` op) — the originals are never rewritten.
    ///
    /// A table may carry SEVERAL bindings, because a fact has more than one
    /// identity dimension: the relation (`predicate`) and the entity it is about
    /// (`subject`). The declaration therefore accepts a list; a bare object is
    /// read as a list of one, so a `config.json` written for P2 keeps parsing.
    ///
    /// Bootstrap-only like `schema` and `fts`: the binding drives DDL (alias table,
    /// backfill, FTS index shape), so changing it at runtime would desync the
    /// database from the declaration.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub canonical: BTreeMap<String, Vec<CanonicalSpec>>,
    /// Opt-in full-text indexes: `{ "<table>": ["<column>", …] }` (P3).
    ///
    /// A separate top-level key rather than an entry inside `schema`, because a
    /// `schema` table entry is a column→type map — an `"fts"` key there would be
    /// indistinguishable from a column named `fts` (plan ruling R6). Validated
    /// against `schema` right here, so the one parse path covers it.
    ///
    /// Known limit: only tables declared in `schema` can carry an index —
    /// tables created at runtime via the `create_table` op cannot (the FTS DDL
    /// is bootstrap work of the factory, not a message effect).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub fts: BTreeMap<String, Vec<String>>,
    /// GH #132 — **opt-in** writer boundary: who may change this store's data.
    ///
    /// [`WriteSurface::Open`] (the default, and what an absent key means) is the
    /// historical behaviour: whoever is wired to the store's port may write.
    /// [`WriteSurface::Internal`] declares the write surface internal to the
    /// **owning hive scope** — the store's own parent path. A write op from a
    /// sender outside that scope is refused at the cell, loudly
    /// (`error_code: "write_denied"`), before it reaches the database. Reads
    /// stay free from anywhere.
    ///
    /// Serialised only when it is NOT the default, so an existing `config.json`
    /// and the β params-overlay round-trip keep their exact bytes.
    #[serde(default, skip_serializing_if = "WriteSurface::is_open")]
    pub write_surface: WriteSurface,
}

/// GH #132 — how a `store`'s write surface is bounded.
///
/// Deliberately an enum rather than a boolean: "who may write" is a place on a
/// scale, not a switch, and a later surface (say: nobody, an append-only phase)
/// extends the value set without changing the key. The JSON form is the value
/// string, so the declaration reads `"write_surface": "internal"`.
/// Deserialisation is deliberately NOT derived: the closed value set is checked
/// in [`StoreParams::parse`], the one parse path `validate_params` and
/// `spawn_cell` share, so a misspelled surface is an operator-readable error
/// rather than a serde message. `Serialize` is required — the β params-overlay
/// round-trips the struct through `serde_json::to_value` and back through
/// `parse`, and `"internal"` has to survive that trip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteSurface {
    /// Every sender wired to the store may write. Default, and what every
    /// `config.json` written before GH #132 means.
    #[default]
    Open,
    /// Only senders inside the store's own parent scope (the owning hive) may
    /// write. Reads are unaffected.
    Internal,
}

impl WriteSurface {
    /// `skip_serializing_if` predicate — keeps the default out of the serialised
    /// form, so no pre-existing params document changes shape.
    pub fn is_open(&self) -> bool {
        matches!(self, WriteSurface::Open)
    }
}

/// One canonical-column binding of [`StoreParams::canonical`].
///
/// `source` is the column a writer fills, `target` the store-owned column every
/// identity-sensitive read consumes, `aliases` the table that maps one spelling
/// onto another. All three are validated against `schema` at parse time, so a
/// binding that survives can always be turned into DDL and SQL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CanonicalSpec {
    /// The written column — never modified by the store.
    pub source: String,
    /// The derived column — store-owned, filled on write, re-derivable.
    pub target: String,
    /// The store-owned alias table (`alias` → `canonical`).
    pub aliases: String,
    /// Normalise this dimension before it becomes an identity (0.2.0 P4, ruling
    /// Q5): Unicode composition, case fold, whitespace collapse
    /// ([`crate::store::query::normalize::normalize`]).
    ///
    /// Opt-in, and it is the ONLY automatic merge the store performs — two
    /// spellings equal after normalisation ARE one entity, which is provable
    /// rather than judged. With it on, the alias table is keyed on the normal
    /// form as well, so an alias written for one spelling covers every spelling
    /// that normalises onto it.
    ///
    /// Off by default: a relation key is minted canonical by the extractor
    /// already (ruling Q2), and P2's predicate binding keeps its bytes.
    pub normalize: bool,
    /// The store-owned table of NEGATIVE judgements (0.2.0 P5).
    ///
    /// The alias table records that two spellings ARE one identity. Nothing
    /// recorded that two of them are NOT, so the nightly GC re-proposed every pair
    /// it had already rejected, every night. A `canonical` value of NULL cannot
    /// express it (the column is `NOT NULL`, and it would also mean "resolves to
    /// nothing"), so the refusal gets a table of its own: an unordered pair plus
    /// the moment it was judged.
    ///
    /// Optional. Without it `reject_pair` is refused and
    /// [`crate::store::ops`]'s candidate feed simply has nothing to exclude —
    /// which is exactly the P2/P4 behaviour, so every declaration written before
    /// this package keeps its meaning.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejected: Option<String>,
}

impl crate::params_overlay::OverlayParams for StoreParams {
    /// `schema` (immutable) + `query_timeout_ms` (mutable, path C).
    const KNOWN_KEYS: &'static [&'static str] = &[
        "schema",
        "query_timeout_ms",
        "fts",
        "canonical",
        "write_surface",
    ];
    /// `schema` is bootstrap-only — it is baked into `cell.db` via DDL at spawn;
    /// changing it at runtime would desync the live tables from the declared
    /// schema. Rejected on any update attempt.
    ///
    /// `write_surface` (GH #132) is immutable for a different reason: a boundary
    /// that a message can switch off is not a boundary. It is declared in the
    /// `config.json` and stays there.
    const IMMUTABLE_KEYS: &'static [&'static str] =
        &["schema", "fts", "canonical", "write_surface"];
    fn parse(raw: &Value) -> Result<Self, String> {
        StoreParams::parse(raw)
    }
}

impl StoreParams {
    /// Parse raw params from a JSON value.
    ///
    /// Returns `Err` with an operator-readable message on missing or malformed
    /// `schema`, unsupported column types, or non-integer `query_timeout_ms`.
    pub fn parse(raw: &Value) -> Result<Self, String> {
        let obj = raw.as_object().ok_or("params must be a JSON object")?;
        let schema_v = obj.get("schema").ok_or("params.schema required")?;
        let schema_obj = schema_v
            .as_object()
            .ok_or("params.schema must be an object")?;
        if schema_obj.is_empty() {
            return Err("params.schema must declare at least one table".into());
        }
        let mut schema = BTreeMap::new();
        for (table, cols_v) in schema_obj {
            crate::store::ddl::check_new_identifier("params.schema table", table)?;
            let cols_obj = cols_v
                .as_object()
                .ok_or_else(|| format!("params.schema.{table} must be an object"))?;
            if cols_obj.is_empty() {
                return Err(format!(
                    "params.schema.{table} must declare at least one column"
                ));
            }
            let mut cols = BTreeMap::new();
            for (col, ty_v) in cols_obj {
                crate::store::ddl::check_new_identifier(
                    &format!("params.schema.{table} column"),
                    col,
                )?;
                let ty = ty_v
                    .as_str()
                    .ok_or_else(|| format!("params.schema.{table}.{col} must be a string"))?;
                if !matches!(ty, "text" | "int" | "json") {
                    return Err(format!(
                        "params.schema.{table}.{col}: unsupported type {ty:?} (allowed: text/int/json)"
                    ));
                }
                cols.insert(col.clone(), ty.to_string());
            }
            schema.insert(table.clone(), cols);
        }
        let query_timeout_ms = match obj.get("query_timeout_ms") {
            None => None,
            Some(v) => Some(v.as_u64().ok_or("query_timeout_ms must be an integer")?),
        };
        let canonical = parse_canonical(obj.get("canonical"), &schema)?;
        let fts = parse_fts(obj.get("fts"), &schema)?;
        // GH #132: closed value set, one parse path — an unknown surface never
        // reaches spawn as a silently-open store.
        let write_surface = match obj.get("write_surface") {
            None => WriteSurface::Open,
            Some(v) => match v.as_str() {
                Some("open") => WriteSurface::Open,
                Some("internal") => WriteSurface::Internal,
                _ => {
                    return Err("params.write_surface must be \"open\" or \"internal\"".to_string());
                }
            },
        };
        Ok(StoreParams {
            schema,
            query_timeout_ms,
            canonical,
            fts,
            write_surface,
        })
    }
}

/// Validate `params.canonical` against the already-parsed `schema` (0.2.0 P2).
///
/// Same discipline as [`parse_fts`]: every rule is a closed-set check against the
/// declared tables and columns, plus a syntax gate on the alias table name — it is
/// the one identifier here that does not exist in `schema` yet, because the store
/// creates it itself.
fn parse_canonical(
    raw: Option<&Value>,
    schema: &BTreeMap<String, BTreeMap<String, String>>,
) -> Result<BTreeMap<String, Vec<CanonicalSpec>>, String> {
    let Some(raw) = raw else {
        return Ok(BTreeMap::new());
    };
    let obj = raw
        .as_object()
        .ok_or("params.canonical must be an object")?;
    let mut out: BTreeMap<String, Vec<CanonicalSpec>> = BTreeMap::new();
    // Alias tables are store-owned and created by name, so two bindings sharing
    // one would silently mix two identity dimensions in a single mapping. The set
    // spans the whole declaration, not one table. The rejected-pair table of
    // 0.2.0 P5 is store-owned in exactly the same sense and shares the set, so a
    // name can never be an alias table here and a refusal log there.
    let mut alias_tables: std::collections::BTreeSet<String> = Default::default();
    for (table, list_v) in obj {
        let declared = schema
            .get(table)
            .ok_or_else(|| format!("params.canonical.{table}: not declared in params.schema"))?;
        // The list form (0.2.0 P4) and the single-object form (P2) are the same
        // declaration with one entry, so both walk the identical validation.
        let specs: Vec<&Value> = match list_v {
            Value::Array(a) => {
                if a.is_empty() {
                    return Err(format!(
                        "params.canonical.{table}: an empty binding list declares nothing — omit \
                         the table instead"
                    ));
                }
                a.iter().collect()
            }
            Value::Object(_) => vec![list_v],
            _ => {
                return Err(format!(
                    "params.canonical.{table} must be a binding object or a list of them"
                ));
            }
        };
        let mut bindings: Vec<CanonicalSpec> = Vec::with_capacity(specs.len());
        for spec_v in specs {
            let spec_obj = spec_v
                .as_object()
                .ok_or_else(|| format!("params.canonical.{table} must be an object"))?;
            for key in spec_obj.keys() {
                if !matches!(
                    key.as_str(),
                    "source" | "target" | "aliases" | "normalize" | "rejected"
                ) {
                    return Err(format!(
                        "params.canonical.{table}: unknown key {key:?} (allowed: \
                         source/target/aliases/normalize/rejected)"
                    ));
                }
            }
            let field = |name: &str| -> Result<String, String> {
                spec_obj
                    .get(name)
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .ok_or_else(|| format!("params.canonical.{table}.{name} must be a string"))
            };
            let source = field("source")?;
            let target = field("target")?;
            let aliases = field("aliases")?;
            let normalize = match spec_obj.get("normalize") {
                None => false,
                Some(v) => v.as_bool().ok_or_else(|| {
                    format!("params.canonical.{table}.normalize must be a boolean")
                })?,
            };
            for col in [&source, &target] {
                match declared.get(col).map(String::as_str) {
                    Some("text") => {}
                    Some(other) => {
                        return Err(format!(
                            "params.canonical.{table}.{col}: type {other:?} cannot be canonicalised \
                             (text only)"
                        ));
                    }
                    None => {
                        return Err(format!(
                            "params.canonical.{table}.{col}: not a column of that table"
                        ));
                    }
                }
            }
            if source == target {
                return Err(format!(
                    "params.canonical.{table}: source and target must be different columns — the \
                     original has to stay readable next to the derived value"
                ));
            }
            crate::store::ddl::check_new_identifier("params.canonical alias table", &aliases)?;
            if schema.contains_key(&aliases) {
                return Err(format!(
                    "params.canonical.{table}.aliases: {aliases:?} is declared in params.schema — \
                     the alias table is store-owned and must not carry a second declaration"
                ));
            }
            if !alias_tables.insert(aliases.clone()) {
                return Err(format!(
                    "params.canonical.{table}.aliases: {aliases:?} is already the alias table of \
                     another binding — one mapping cannot serve two identity dimensions"
                ));
            }
            let rejected = match spec_obj.get("rejected") {
                None => None,
                Some(v) => {
                    let name = v.as_str().ok_or_else(|| {
                        format!("params.canonical.{table}.rejected must be a string")
                    })?;
                    crate::store::ddl::check_new_identifier(
                        "params.canonical rejected table",
                        name,
                    )?;
                    if schema.contains_key(name) {
                        return Err(format!(
                            "params.canonical.{table}.rejected: {name:?} is declared in \
                             params.schema — the rejected-pair table is store-owned and must not \
                             carry a second declaration"
                        ));
                    }
                    if !alias_tables.insert(name.to_string()) {
                        return Err(format!(
                            "params.canonical.{table}.rejected: {name:?} is already a store-owned \
                             table of another binding — a refusal log cannot be shared"
                        ));
                    }
                    Some(name.to_string())
                }
            };
            // Two bindings on one table must stay independent: a shared source or
            // target would make the derived value depend on evaluation order, and
            // a target that is another binding's source would give a store-owned
            // column an identity of its own.
            for other in &bindings {
                if other.source == source {
                    return Err(format!(
                        "params.canonical.{table}: {source:?} is bound twice"
                    ));
                }
                if other.target == target {
                    return Err(format!(
                        "params.canonical.{table}: {target:?} is the target of two bindings"
                    ));
                }
                if other.target == source || other.source == target {
                    return Err(format!(
                        "params.canonical.{table}: {source:?}/{target:?} chains onto another \
                         binding — a derived column is never a source"
                    ));
                }
            }
            bindings.push(CanonicalSpec {
                source,
                target,
                aliases,
                normalize,
                rejected,
            });
        }
        out.insert(table.clone(), bindings);
    }
    Ok(out)
}

/// Validate `params.fts` against the already-parsed `schema` (R6).
///
/// Every rule is a closed-set check against the declared tables and columns, so
/// a declaration that survives here can always be turned into DDL.
fn parse_fts(
    raw: Option<&Value>,
    schema: &BTreeMap<String, BTreeMap<String, String>>,
) -> Result<BTreeMap<String, Vec<String>>, String> {
    let Some(raw) = raw else {
        return Ok(BTreeMap::new());
    };
    let obj = raw.as_object().ok_or("params.fts must be an object")?;
    let mut out = BTreeMap::new();
    for (table, cols_v) in obj {
        let declared = schema
            .get(table)
            .ok_or_else(|| format!("params.fts.{table}: not declared in params.schema"))?;
        if declared.contains_key("rank") {
            return Err(format!(
                "params.fts.{table}: table has a column named \"rank\", which collides with the \
                 bm25 result column of the search op"
            ));
        }
        let arr = cols_v
            .as_array()
            .ok_or_else(|| format!("params.fts.{table} must be an array of column names"))?;
        if arr.is_empty() {
            return Err(format!("params.fts.{table} must name at least one column"));
        }
        let mut cols = Vec::with_capacity(arr.len());
        for c in arr {
            let col = c
                .as_str()
                .ok_or_else(|| format!("params.fts.{table}: column names must be strings"))?;
            match declared.get(col).map(String::as_str) {
                Some("text") | Some("json") => {}
                Some(other) => {
                    return Err(format!(
                        "params.fts.{table}.{col}: type {other:?} cannot be indexed (text/json only)"
                    ));
                }
                None => {
                    return Err(format!(
                        "params.fts.{table}.{col}: not a column of that table"
                    ));
                }
            }
            cols.push(col.to_string());
        }
        out.insert(table.clone(), cols);
    }
    Ok(out)
}

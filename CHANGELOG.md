# Changelog

All notable changes to MeClaw are documented in this file. One entry per released
package. The format loosely follows [Keep a Changelog](https://keepachangelog.com/);
versioning follows SemVer (0.x: minor/patch bumps for additive features).

## [0.1.3] — 2026-08-08

### Added

- **store: query layer.** `where` accepts comparison operators (`eq`, `neq`, `lt`,
  `lte`, `gt`, `gte`, `in`, `is_null`, `or_null(<op>)`) next to bare equality;
  new `order_by` (multi-column, `asc`/`desc`) and `limit` (integer >= 1, no
  implicit default). Bi-temporal as-of queries, top-k and recency now run in the
  store instead of fetch-all plus filtering in a code cell.
- **store: `search` operation** over SQLite FTS5. Opt in per table via the new
  `params.fts` (`{"<table>": ["<column>", ...]}`); every result row carries a
  `rank` column (bm25, smaller is better). External-content index plus triggers;
  an existing `cell.db` builds its index once on the next spawn, so rows written
  before the declaration become searchable.
- **memory-hive template**: recall legs and the dream lane push their predicates
  into the store; `store` declares full-text indexes on `episodes.content` and
  `facts.claim` (the keyword recall leg itself lands in P5).

### Changed

- **store: identifiers are resolved against the SQLite catalog.** Table and
  column names are matched against `sqlite_master`/`pragma_table_info` and only
  the catalog's own spelling is ever written into a statement; caller text
  reaches SQL exclusively as a bound parameter.
- **store: `select` with an unknown column now reports `unknown_column`** instead
  of the generic `sql_error` (the code was always specified, only the classifier
  missed this path). No new error codes were introduced.

### Security

- **store: identifier syntax gate on the two DDL paths.** `create_table` and
  `params.schema` accept `[A-Za-z_][A-Za-z0-9_]{0,62}` only, reject the `sqlite_`
  prefix and the reserved `_fts` suffix. Both used to format caller strings
  straight into DDL.

## [0.1.2] — 2026-08-07

### Added

- **`memory-hive@1`** — a 9-cell agent-memory topology template (`store`, `writer`, `recall`,
  `extract-glue`, `extractor`, `dream-glue`, `dreamer`, `cron`, `embed`) built entirely from
  existing cell types, with **no substrate changes**:
  - **Bi-temporal facts** — `valid_from`/`valid_until` (event time) alongside
    `recorded_at`/`expired_at` (system time) plus `superseded_by`, so "what is true now",
    "what was true in May" and "what did we believe in May" are all answerable. Nothing is
    ever deleted: supersession stamps an expiry, belief retraction flips a flag.
  - **Batched extraction** — an accumulating gate (~512 tokens or a 30-minute-old item) keeps
    the LLM cost per turn at zero; the synchronous write path stays LLM-free and immediate.
    A second, inline ingress accepts pre-extracted payloads from a front-line model; both go
    through one validator and one `(episode_id, claim_hash)` dedup.
  - **Idempotent nightly consolidation** — the delta window derives from the run log and every
    written value derives from the window end, so a replayed run leaves memory byte-identical
    and a missed timer firing needs no catch-up.
  - **Embedding lane with graceful degradation** — a dead embedder leaves rows queued with
    `NULL` blobs; writes and recall keep working and the hive never hard-fails on it.
  - Recall ships tier 0 only: a deterministic, token-budgeted context bundle. Higher tiers
    (multi-leg retrieval, synthesis) and the store-side query layer they need are next up.

  Ships in the **private builder workspace**; public packaging of the builder core is pending.

### Notes

- The template works against the current equality-only `store` ops by design (no `ORDER BY`,
  `LIMIT`, `LIKE` or `IS NULL`): temporal and freshness filtering happens in its `code` cells
  until the store gains a query layer.
- New roadmap defer: `cell-types.md` § `code` states that a successful script's stderr is
  logged at warn level, while the implementation only sets the `had_stderr` header. Needs a
  ruling (align the code or shorten the spec).

## [0.1.1] — 2026-08-07

### Added

- **Message browser** — the colony's message log is now browsable:
  - `GET /colony/messages`: read-only list endpoint over `message_log` with keyset
    pagination, filters (`to_path` incl. prefix, `from_path`, `trace_id`,
    `correlation_id`, `body_kind`, time range), a two-stage query (indexed predicates
    first, residual filters under an explicit `scan_budget`, default 5000 / hard cap
    50000) and optional on-demand blob resolution (`?resolve_blob=true`).
  - `/ui/messages`: list view with filter form, keyset paging and truncated payload
    preview. Truncated scans are always disclosed in the UI.
  - `/ui/message`: envelope detail view with `context` and `hop` headers rendered
    separately, pretty-printed payload, lazy blob loading, and pivot navigation
    (trace view, parent-message chain, correlation, reply-to, dead letters).
  - Dead-letter view: new "Original" column linking to the originating message where
    it exists in the message log.

### Notes

- Messages that fail before the log write exist only as dead letters; the dead-letter
  entry itself carries the full message. Tracked as a documented deferral.
- The new endpoint is read-only and not EDA-dispatchable (like `/colony/dead_letters`).

## [0.1.0] — 2026-06-17

Initial public release: the MeClaw DSL substrate — directory tree as topology, 12
built-in cell types plus hive scoping, colony actor runtime with hot/cold lifecycle,
graph mutations and templates, long-running cells, HTTP API + web UI, stdio
direct-mode bridge, English specification (overview, cell types, config).

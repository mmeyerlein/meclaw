# Changelog

All notable changes to MeClaw are documented in this file. One entry per released
package. The format loosely follows [Keep a Changelog](https://keepachangelog.com/);
versioning follows SemVer (0.x: minor/patch bumps for additive features).

## [0.1.7] — 2026-08-08

A reusable stdio-child core, and the `mcp` cell's second transport riding on it.

### Added

- **`stdio_child`: spawn a child process, speak line-JSON, supervise its life.**
  A new module in `meclaw-cells` that owns the parts every future child-process
  consumer needs and none of the parts any single one of them owns: spawning
  (`ChildSpec`/`StdioChild`), newline-delimited JSON framing tolerant of blank
  lines and non-JSON banners, request/response correlation through an injected
  key extractor, lifecycle events, and killing plus reaping. The I/O sub-task of
  the dual-task pattern owns the child outright — the handler holds no pipe and
  talks to it over the two channels the substrate already provides, so a
  request/response call stays a plain `await` instead of deadlocking against the
  handler's own `select!`.
- **`mcp` speaks stdio.** `params.transport: "stdio"` runs the provider as a
  child process (`command`, `args`, `env`, `cwd`, `kill_grace_ms`) and performs
  the same `initialize` / `tools/list` / `tools/call` protocol over line-JSON.
  `transport` is optional and defaults to `http`: every configuration written
  before this release parses to exactly the same result, and the HTTP path is
  untouched.
- **Post-init liveness for the stdio transport.** The long-running stream read
  carries the signal the HTTP transport never had. When the child dies, the
  in-flight call is answered with a typed `mcp_error` **first**, and only then
  does the cell panic — `one_for_one` restarts it with a fresh child, and after
  the restart limit the registry entry is retained as `failed`. Nothing is lost
  to the panic, because the emit completes before it.
- **Orphan reaping, proven rather than asserted.** `kill_on_drop` plus an
  explicit kill-and-wait; the test reads the child's pid from a file and waits
  for `/proc/<pid>` to disappear, which rules out both a survivor and a zombie
  in one check.

### Fixed

- **A late request after the child died no longer waits for its timeout.** The
  handler's `select!` is biased towards its mailbox over its event channel, so
  it can accept one more message before it has seen the death. The serve loop
  now keeps draining commands after the child is gone and answers each one
  immediately with the child's fate, instead of parking and letting a known
  death surface as a spurious `provider_timeout` a full A-timeout later.

## [0.1.6] — 2026-08-08

The server-rendered operator UI speaks English. This is a small functional
release: the only behaviour that changes is the rendered text.

### Changed

- **Operator UI renders English end to end.** Every string the `/ui/*` pages
  emit — empty states, filter labels, table headers, pivot links, the
  pagination arrow, the dashboard's consistency disclaimer, the header
  compartment captions and the blob-resolution notices — is now English, with
  one term per concept across all seven pages. Route names, query parameters,
  field names and error tokens (`missing_blob_id`, `malformed_blob_id`,
  `blob_unreadable`) are untouched: they are API surface, not copy. No markup,
  layout or logic changed.
- **Tests asserting on rendered text moved with it.** Ten assertions match UI
  copy through `contains()`; each was flipped to the English text first,
  observed red, and only then was the string translated. Two of the ten were
  not in any inventory — they were invisible to the German-text heuristic
  because their literals carry neither an umlaut nor a listed function word.
  The lesson is recorded with them: coupling is found by reading the files,
  not by trusting a scanner's hit set.
- **German test fixtures anglicized.** The `"hallo welt"` fixture (four test
  sites across three crates, eight literals) became `"hello world"`. Each site
  is inside `#[cfg(test)]` or under `tests/`; none has runtime effect. The
  FTS5 tripwire keeps its shape — it indexes two tokens and matches on the
  *second* one, so `MATCH 'welt'` became `MATCH 'world'`, not `'hello'`.

## [0.1.5] — 2026-08-08

The memory hive gets its full read path. No Rust behaviour changed in this release —
everything below lives in the private builder workspace (templates, fixtures, evals);
the only tracked source change is a rename of public test fixtures to generic names.

### Added

- **Recall tier 1 — four retrieval legs, fused, no LLM.** A query fans out into
  keyword (`search` over episodes and facts), semantic (`similar` over binarized
  embeddings), graph (entity anchors → `traverse`, yielding the episodes the edges
  came from) and temporal (an as-of `select`). Each leg returns a ranked id list;
  the lists are merged with **reciprocal rank fusion** (`Σ w/(K+rank)`, K=60) in a
  code cell, hydrated in one round and cut to a token budget. Ties break by best
  rank, then a fixed leg priority, then kind and id — two identical requests
  produce byte-identical candidate lists.
- **Degradation as arithmetic, not as a special case.** An empty leg contributes no
  fusion term, so a dead embedder makes the result mathematically identical to a
  fusion of the remaining three legs. The embedding lane's query mode therefore
  *always* answers — with a vector or with `degraded: true` — because silence would
  hang the fan-in forever.
- **Recall tier 2 (`dialectic`).** An answer synthesised over the tier-1 candidates
  with the source priority beliefs → facts → episodes and a **mandatory gap
  statement**. The gap is enforced by the caller, not hoped for: an answer without
  one is still delivered but carries `gap_missing`, and a provider error downgrades
  to the tier-1 candidates instead of going silent.
- **As-of recall.** Any tier can be evaluated at a past instant, so "what was true in
  May" is a parameter rather than a promise.
- **Historical ingest.** A turn may carry its own event time; the write path keeps
  the caller's `happened_at` and stamps `recorded_at` from its own clock — which is
  exactly the bi-temporal split the schema is built on.
- **Explicit extraction flush.** An operator (or an ingest job) can drain the
  extraction queue immediately instead of waiting for the batch gate's age timeout.
- **Scenario suite as the development gate.** One case per capability — a hand-written
  mini corpus with known gold facts, defined queries and deterministic assertions.
  17 cases, 55 assertions; 13 of them cost nothing because facts enter through the
  inline ingress rather than through a model. Ships in the private builder workspace.

### Fixed

- **Facts inherited the ingest instant as their event time.** An extracted fact whose
  `valid_from` the model did not state fell back to "now", so an as-of query answered
  about the ingest rather than about the conversation. The fallback is now a chain:
  what the extractor claims → when the episode happened → our clock.
- **A superseded fact could still be recalled.** Only the temporal leg filtered
  `expired_at`; the keyword and semantic legs kept ranking invalidated facts. The
  filter now sits at hydration and therefore covers every leg. The raw episode that
  mentioned the old value stays retrievable on purpose — episodes are append-only.
- **A session-boot recall without a query was swallowed.** The echo guard keyed on the
  query being non-empty, which is precisely what the deterministic tier-0 bundle does
  not have. Request detection now keys on what the port edge promotes.
- **The batch claim was unbounded.** The extraction gate claimed every pending row, so
  a bulk ingest turned hundreds of turns into a single model call. Batches are now
  bounded by the token threshold and an item cap.
- **A fenced JSON answer stalled the extraction lane.** Model output wrapped in a code
  fence failed to parse and the batch was requeued forever. Fences are stripped, and
  an answer that stays unparseable is parked for inspection instead of spinning.

### Measured

First eval numbers, on the **smoke stage only — 10 questions, all of them the easiest
category** (`single-session-user`) and therefore no statement about the whole set:
retrieval Recall@5 100 %, Recall@1 100 %, MRR 1.0; judged end-to-end 90 % by a judge
model, 80 % under a strict manual reading. Model identity for every call is taken from
the provider's `response.model`, never from configuration. Details and the honest
caveats live with the project, not in this repo.

## [0.1.4] — 2026-08-08

### Added

- **store: `traverse` operation.** Multi-hop walk over a declared edge table via
  a recursive CTE. The caller names the table plus the column roles (`src`,
  `dst`, optional `kind` and `weight`), the start node(s), an optional `where`
  over the edge rows and an optional projection of further edge columns — every
  identifier is resolved against the SQLite catalog, every value is bound. The
  result is a set of **paths** (end node, depth, the nodes walked through, the
  last edge's attributes and the accumulated weight), so scoring stays with the
  caller instead of being guessed in the store.
- **store: traversal guards.** `max_depth` (default 2, hard cap 5) and
  `max_nodes` (default 200, hard cap 5000) are mandatory by construction; a
  value beyond the cap is rejected, never silently clamped. Cycles are
  eliminated per path, so a walk always terminates and no path visits a node
  twice. Hitting the node cap sets `truncated` in the payload — the result never
  shrinks silently.
- **store: `similar` operation.** Nearest-neighbour ranking over a column of
  binarized embedding vectors, combinable with `where`, `order_by` and `limit`.
  Every row carries a `distance` column (hamming distance, smaller is better);
  without an explicit ordering the result is ranked best-first with `rowid` as
  the tiebreaker. Rows whose vector is NULL — the embedding backfill queue — are
  excluded, because NULL would otherwise sort to the top.
- **store: `hamming(a, b)` scalar function**, registered on every `store`
  connection (wake and respawn alike). Arguments may be base64 text or a blob;
  unequal vector lengths, malformed base64 and non-vector arguments raise a
  regular `sql_error`. Comparing across embedding generations is a caller error
  and now fails loudly instead of producing a plausible, wrong ranking.

With this, all four retrieval legs — temporal, keyword, graph and semantic — are
answerable inside the store.

### Changed

- `rusqlite` gains the `functions` feature (needed for the registered scalar
  function). No new dependency, no lockfile change, and no loadable SQLite
  extension.

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

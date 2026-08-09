# Changelog

All notable changes to MeClaw are documented in this file. One entry per released
package. The format loosely follows [Keep a Changelog](https://keepachangelog.com/);
versioning follows SemVer (0.x: minor/patch bumps for additive features).

## [0.1.10] — 2026-08-09

Subscription auth: the `llm` cell learns a second credential and a second wire.

### Added

- **An `auth` dimension on the `llm` cell (`api_key` | `oauth_subscription`).**
  Model access no longer has to be pay-per-token. A cell can present a rotating
  OAuth token from a token store instead of a static key — no CLI harness
  between the cell and the model, which is the whole point: an agent harness
  that pre-prompts, loops and tools on its own is exactly what an `llm` cell
  must not have in front of it. The seam is vendor-neutral by construction;
  one vendor is implemented, and a second is a set of params rather than a
  rebuild.
- **A second wire dialect: Responses.** Beside chat-completions the translate
  boundary now speaks the Responses shape — typed `input[]` items, a top-level
  `instructions` slot, `max_output_tokens`, flat tool schemas. It is a
  **separate axis from `provider`**: the same vendor with a different wire is
  not a different provider, so the provider constraint stays untouched and
  `auth × wire_dialect` becomes the matrix. The wire is pinned against a
  reference implementation rather than reverse-engineered, and the fixtures are
  the drift detectors.
- **A single-refresher token broker.** The refresh token rotates, so two cells
  refreshing one store concurrently would earn a permanent `refresh_token_reused`
  and force a human back through a login. All cells in a process therefore share
  one broker actor that performs the refresh itself: single-flight by
  construction, no lock, no wait loop. A cell that hit a 401 names the token
  generation it used, so a concurrent refresher wins instead of racing.
- **A two-level error taxonomy.** The spec's `error_code` enum stays closed;
  the discriminator a failover edge actually needs — `quota_exhausted` with its
  reset time, `auth_expired`, `auth_permanent` with `re_login_required` —
  arrives in `meta.error`. Failover itself remains topology: the cell emits a
  typed error and stops. It does not retry, it does not fall back, and it never
  loops.

### Changed

- `api_key` is now optional in `llm` params, because a subscription lane has no
  key. Exactly one credential per cell is enforced at spawn, and the whole auth
  dimension is immutable at runtime — `wire_dialect` and the OAuth overrides
  decide *which endpoint* a credential is presented to, so a mutable one would
  let a message redirect an existing token somewhere new.
- The token store is written as a **patch, not a rewrite**. It is the vendor
  CLI's own credential file that MeClaw is a second writer of; rotation touches
  three token fields and a timestamp and leaves every unknown field alone. A
  naive rewrite would have destroyed an interactive login on the first rotation.

### Notes

- The existing `api_key`/chat-completions path is unchanged down to the byte,
  pinned by a regression test that freezes the serialized request body, the
  path and the exact set of request headers.
- Streaming is a **transport** detail here, not an output feature: the wire
  streams because the subscription backend accepts nothing else, while the cell
  stays atomically-emitting and folds the whole stream into one message.
- Secret hygiene extends the existing key discipline to the token path — no
  token in config, logs, messages, `meta` or error text; redacting `Debug`;
  atomic `0600` writes — and is covered by an explicit audit test rather than
  by convention.

## [0.1.9] — 2026-08-09

MeClaw calls MeClaw: a whole child colony, driven as one cell.

### Added

- **The `subcolony` cell type.** A child colony runs as its own `meclaw`
  process and behaves, from the parent tree's point of view, like a single
  cell: one path, one mailbox, one contract. The child's internal tree is
  invisible and **not addressable** from outside. That is composition, not
  federation — and it is pinned by negative tests rather than merely intended.
  Cross-colony routing is a non-goal, not a deferred feature. The thirteenth
  built-in cell type, long-running and dual-task, built on the P7 stdio-child
  core.
- **A JSON wire for the stdin/stdout bridge (`--stdio-format <text|json>`).**
  A `meclaw` process is now addressable as a structured endpoint, not only as
  a line of text: request and reply frames carry the envelope the text format
  cannot express (`trace_id`, `ttl`, `context`), a `ready` frame announces the
  boot, and unreadable input is answered with a typed error instead of being
  swallowed. **`text` remains the default** and is unchanged, down to the byte.
- **Composition semantics that are tested, not assumed.** The parent's
  `trace_id` is *carried* into the child, so one conversation stays one trace
  across two colonies and two message logs. The TTL is *decremented* crossing
  the boundary — on top of the routing hop — so a sub-colony cycle dies exactly
  like any routing cycle; at zero the crossing is refused rather than made one
  last time. Nothing else crosses unless the facade declares it: `context` only
  through an explicit mapping, `hop` never, in either direction.
- **Secret isolation as a side effect of the process boundary.** The child is
  started with a wiped environment plus an explicit passthrough list, in its
  own process group, so neither the parent's secrets nor the child's process
  tree outlive their scope.

### Notes

- Two failure classes are treated differently on purpose. A **deterministic**
  failure — the child speaks another protocol version, never boots, cannot be
  spawned — does not panic: the cell stays up and refuses every request with
  the reason, because a restart would reproduce the failure exactly and burning
  the restart budget on a certainty only turns one clear error into a process
  storm. A **transient** failure — the child dies mid-conversation — releases
  whoever was waiting with a typed error first and then restarts, because there
  a restart is the cure.
- The protocol version and the release version are separate fields, and only
  the protocol version is asserted. A parent and a sealed child colony are
  expected to run different builds; that is the point of the boundary.
- No task register: not because a request is idempotent (it is not — a request
  can make the child write to its store), but because there is no automatic
  re-fire path. Whoever asked decides whether to ask again.
- No new dependencies.

## [0.1.8] — 2026-08-09

An agent harness — Claude Code in print mode — supervised as a cell.

### Added

- **The `harness` cell type.** A full agent harness runs as a supervised child
  process driven from the topology: a message starts a task, the harness's
  progress streams back as typed emissions, and its outcome arrives as a
  structured result. One child process **per task** — the workspace differs per
  task, and a process boundary is the natural transaction boundary for work that
  changes files. Long-running, dual-task, and the twelfth built-in cell type.
- **A task register that refuses to repeat itself.** Every other cell type is
  idempotent: replay a message, get the same answer. A harness task mutates a
  repository, so replaying it is not the same answer — it is a second run
  against a tree somebody may already be reviewing. `cell.db.harness_tasks` is
  therefore a tombstone register, not a work queue: the row is committed
  **before** the child is spawned, a repeated `task_id` is refused outright, and
  a supervisor restart turns every unfinished row into "unknown outcome, inspect
  the workspace" — never into a new run. There is no code path from the table
  back to a running task.
- **A dead child is normal here.** For `mcp` the child *is* the cell's ability
  to answer, so its death is a panic. For a harness the child is one task, and
  its exit is how a task ends: the cell classifies the outcome, closes the
  tombstone, emits the result, and goes back to waiting. The I/O sub-task
  cycles — idle, spawn, stream, idle — instead of parking.
- **Five typed emissions.** `accepted` answers the requesting message inside its
  trace and hands back the `task_id`; `progress`, `question`, `result` and
  `error` travel the origin lane to `params.emit_to`, correlated by that id. The
  result header carries only what was **observed** — the workspace we assigned,
  the status we decided, and the numbers the harness reported about itself
  (session, model, turns, cost). It deliberately carries no branch or commit:
  the harness's own summary travels as prose, and verifying it is a follow-up
  step in the topology, not a field to be trusted.
- **A stop lever.** `cancel` marks the task as cancelled **before** killing it,
  so whoever reads the table next sees a deliberate cancellation rather than a
  mystery, then tears down the whole process group. Proven against a task that
  never ends on its own, with the kill required to land promptly rather than
  outlast a timeout.
- **A permission channel, wired but off by default.** A `can_use_tool` control
  request becomes a `question` emission; an `answer` message becomes the
  control response. With `approval: "off"` (the default) a question is reported
  **and** refused in the same breath, so a harness is never left waiting for an
  answer nobody will give.
- **Process-group reaping in the stdio-child core.** An agent harness spawns
  process trees — shells, search tools, sub-agents — and `kill_on_drop` reaches
  only the direct child. `ChildSpec.process_group` starts the child as a group
  leader; teardown escalates SIGTERM → grace → SIGKILL across the **group**, and
  a `Drop` guard covers the paths that never reach an explicit teardown (task
  abort, peer panic, colony exit). The test proves both the child and its
  grandchild leave `/proc`, and a control case shows the grandchild surviving
  without the group — so the proof discriminates. `mcp` is unaffected.
- **Environment containment.** `ChildSpec.env_clear` wipes the inherited
  environment before applying an explicit list, so a child sees exactly what it
  was handed. The `harness` cell type uses it with a short passthrough
  allow-list; `mcp` keeps inheriting as before.
- **`serve_child_until_exit`.** The serve loop, but returning the child's fate
  instead of parking on it. `serve_child` is now its parking epilogue, so both
  consumers share one loop.

### Changed

- **The serve loop accepts commands that are not for the child.** Its command
  type is now `TryInto<ChildCommand>`: a consumer may send control messages of
  its own over the same channel, and one that cannot be delivered to the child
  is skipped with a warning rather than read as a shutdown. `mcp` is unchanged —
  an existing `From` impl satisfies the looser bound for free.

### Notes

- **`harness` is not a sandbox.** It runs with the permissions of the colony
  process and brings its own tools. The dependable limits are the environment
  allow-list and the canonicalised workspace clamp; a measured run confirmed
  that the vendor's `--allowedTools` flag **widens** what a harness may do
  rather than bounding it. Treat `harness` the way `bash` is treated: only in
  topologies you trust.

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

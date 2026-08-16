# Cell types

Detailed spec of the built-in cell types. On conflict between this file and `meclaw-overview.md`, the overview wins. It is the single source of truth.

> New here? [`README.md`](README.md) is the map of this directory and [`glossary.md`](glossary.md) defines the vocabulary this file assumes.

> **Concurrency note**: every cell is registered in colony's registry with a uniform `ActorHandle` (`mpsc::Sender<Message>`); what runs behind the mailbox depends on the cell class. Stateful cells: **one** long-lived `cell_task` with a direct `handle()` call. Stateless cells: **one** long-lived `stateless_dispatcher` task that spawns a short-lived worker task per message (concurrency limit per cell via `params.max_concurrency`, default unbounded). Long-running cells (`proxy`, `timer`, `mcp`): **two** Tokio tasks (handler + I/O), communicating over an internal mpsc. See `meclaw-overview.md`, sections "Cell model", "Stateless-cell dispatcher" and "Long-running cells: dual task". Cell state is always single-threaded-accessible from the perspective of the respective handler task; `Mutex`/`RwLock`/atomics in cell code are forbidden.

> **Timeout discipline**: every I/O operation in cell code (HTTP, DB, subprocess, filesystem, MCP) is wrapped with its own `tokio::time::timeout` (concept A, "operation timeout"). On Elapsed: the cell emits a regular error message and ends `handle()` normally, no restart. Configuration per cell instance via `params.external_timeout_ms` (or a semantically fitting name, e.g. `query_timeout_ms` for `store`). In addition, the substrate backstop `cell.message_timeout` (concept B) takes effect as a coarse protection for cell hangs from unknown causes. Details and recommended defaults per cell type: `meclaw-overview.md` section "Timeouts".

**Cell emission modes** (detail in `meclaw-overview.md` section "Cell emission modes w.r.t. `messages[]`"):

- **atomic-emitting**: the cell emits a fresh `messages[]` containing only its own contribution, no pass-through. Default for all tool endpoints, sources and LLM inference.
- **stream-propagating**: the incoming `messages[]` is passed through and augmented with its own contribution. Not present in the built-in set. Buildable application-specifically via `code` cells.
- **script-determined** *(special case)*: emission mode arises per execution from the script output, only `code`.

## Overview

| Type | Task | Actor? | Emission mode | Phase |
|---|---|---|---|---|
| `hive` | **scope marker** (authority and mutation boundary for a path prefix) + **logical transit node** in the routing graph | **no**, no actor, no mailbox, no `cell.db` | — (transit, no delivery) | 4 |
| `store` | typed SQLite storage with schema + seed | yes, stateful | atomic-emitting | 9 |
| `llm` | LLM inference, holds system state + blob cache | yes, stateful | atomic-emitting | 8 |
| `bash` | shell execution (one-shot only) | stateless | atomic-emitting | 7 |
| `code` | programmable body constructor (Python first) | yes, **stateless** (stateless dispatcher), phase limitation, stateful `code` with `cell.db` deferred | script-determined | 9 |
| `web_fetch` | HTTP client | stateless | atomic-emitting | 7 |
| `web_search` | search-provider client | stateless | atomic-emitting | 7 |
| `file` | filesystem CRUD with security boundary | stateless | atomic-emitting | 7 |
| `edit` | file-editing operations | stateless | atomic-emitting | 7 |
| `proxy` | external-chat bridge (Telegram first), dual task | yes, long-running | atomic-emitting (user turn per external message) | 10 |
| `timer` | periodic event emitter, second-accurate, dual task | yes, long-running | atomic-emitting (schedule body) | 10 |
| `mcp` | MCP-provider bridge, dual task | yes, long-running | atomic-emitting | 10 |
| `harness` | agent harness (Claude Code) as a supervised child process | yes, stateful | long-running | P8 |
| `subcolony` | child colony as one cell (opaque composition facade) | yes, long-running | atomic-emitting | P9 |
| `vault` | sealed secret store — **no operation returns a secret** | yes, stateful | atomic-emitting | W15 |

**Status per cell type / per phase** (which cell is live today, which deferred) → `PROGRESS.md` § Status.

---

## `hive`: scope marker + logical transit node (not an actor)

**Not a cell type in the classical sense**, but a scope marker with an additional transit role in the routing graph. A directory with `config.json` `type: "hive"` marks a path prefix as the authority and mutation boundary for its subtree. There is **no** hive task, **no** hive mailbox, **no** hive-owned `cell.db`, **no** `ActorHandle` entry in colony's registry. Routing, lifecycle, mutation validation and UUID assignment run centrally through the colony. See `meclaw-overview.md` sections "Authority model" and "Concurrency and parallelism".

**Effect in the DSL**: directory nesting groups cells into a logical unit (e.g. `/main/tool-loop/dispatcher`, `/main/tool-loop/collector`). Mutations can use the hive path as a scope field. All diff operations within it are resolved relative to this path prefix, and colony rejects mutations whose paths would lie outside the scope.

**Effect in routing, transit, no delivery**: a hive is from the sender's view an **addressable target**, in the substrate a **transit hop**. When a message with `target = <hive-path>` arrives, colony does **not** deliver it into a mailbox (there is none). Instead it evaluates the hive's out-edges (`EdgeTable` entries with `from = <hive-path>`) as part of its single routing layer: CEL `condition` against headers, apply `modifier`, regular routing hop per match to the respective `to` path, TTL decremented per hop. No hive-owned evaluator, no separate routing logic. See `meclaw-overview.md` section "Hive paths as target: transit evaluation". On no matching out-edge: dead letter with `error_code = "hive_no_route"`. Graph reads for a hive scope run over `/colony/graph?scope=<hive_path>` (see `meclaw-overview.md` section "Visibility / read paths").

**Connectivity of the hive**: whether a hive is active is decided exclusively by the edges of the
parent level that reference its path. Its internal wiring does not count (see
`meclaw-overview.md` § Connectivity and activity). A disconnected hive deactivates its
entire subtree. This is exactly what makes hives the attachment point for complex templates: an
instantiated subtree template is attached to its hive path via edges. The attacher does not need
to know the internal structure.

**`params`**: **exclusively `graph`, `ports` and `required_drains`** (the `HiveParams` deserializer is `deny_unknown_fields`; any other key is a boot error):
- `graph` (optional): initial desired graph for the subtree (format see `meclaw-overview.md` section "Graph schema"). Colony reads this at filesystem bootstrap and enters the declared cells into the registry and the edges into `colony.db`. After the first bootstrap, the persisted edge table in `colony.db` is the truth. `params.graph` is only an initial hint.
- `ports` (optional, GH #133): array of short names of DIRECT children — the endpoints a parent is meant to wire. **Opt-in, and the presence of the key is the switch.** Without it nothing changes: every interior node may be wired from anywhere, which is the behaviour every topology shipped before this field. With it the hive scope is **sealed** and colony's mutation validation rejects an `add_edges` endpoint that reaches past the port (`error_code: "hive_port_boundary"`, pre-destructive — nothing is staged, spawned or wired). What stays legal: an edge between two nodes inside the hive (any depth), an edge onto the **hive path itself** (the transit address), an edge onto a declared port, and the hive marker wiring its own children. What is rejected: an edge that pairs an interior **non-port** node with an endpoint outside the hive — in either direction, because a reply lane wired straight out of an interior cell bypasses the port exactly as an inbound lane does. An empty list is legal and means "the hive path is the only address". Two deliberate limits: the check covers a mutation's `add_edges` and **not the bootstrap** (see below), and a port is a **direct** child — a node below a port is not the port.

  **Why the seal does not cover the bootstrap** (ruling 2026-08-15): the birth topology is the **sovereign design of the colony author**. Whoever writes the `params.graph` of a parent scope is describing the colony they intend, with the whole tree in front of them — that is authorship, not a breach. The seal guards against what happens **afterwards**: a runtime mutation, possibly written by a model, that reaches into a hive it did not build. So a `params.graph` may legitimately wire a deep endpoint into a sealed hive at boot, and several shipped topologies do exactly that. Boot-time enforcement is not ruled out forever, but it would arrive as its **own opt-in switch** — never by silently widening this one, because that would retroactively invalidate birth topologies that are correct today.

- `required_drains` (optional, GH #147): array of `{port, hop, because}` — **port pairs that belong together**. Read as: *if anything outside this hive is wired to `port`, then `port` must have an edge that carries a message with hop `hop` out of the hive.* The classic case is an ingress whose refusals leave on a reject egress: with no consumer, the refusal is a dead end and nobody ever learns the work was not done. **Opt-in like `ports`** — without the key, everything stays as it was.

  A mutation that breaks the pairing is rejected pre-destructively with `error_code: "required_drain_missing"`, and the rejection carries the hive's own `because` sentence verbatim, because a refusal that cannot say what it protects is one people route around. Wiring **both edges in the SAME mutation** is explicitly the intended answer — the check runs against the post-state precisely so that it is.

  The check works by sending the described hop through the **real edge conditions** (`apply_edges`, the same function that routes at runtime) rather than by comparing condition text. `hop.route=='reject'`, `hop.route in ['reject','error']` and `hop.route != 'bundle'` are all three correct drains, and a string comparison would call two of them broken. An edge that stays inside the hive does not count: the refusal has to **leave**. The rule says nothing about the destination — whether the drain is a good one is the parent's business; whether one exists is not.

  The **bootstrap is warned, never refused** — for the same reason the port seal leaves the boot alone: the birth topology is authorship. A tree that has been running for weeks is not stopped from starting; it says the sentence and carries on.

  ```json
  "params": { "ports": ["brief", "gate"], "graph": { "edges": [ … ] },
              "required_drains": [ { "port": "gate", "hop": { "route": "reject" },
                                     "because": "a refused input leaves the hive here" } ] }
  ```

No scope-owned `dead_letters` override: the dead-letter queue is always `/colony/dead_letters` (hive = authority and mutation boundary, **not** DLQ boundary). Otherwise no hive-type-owned fields. In particular no routing configuration, no mailbox size, no own emission-mode statement. Hives have no actor and no mailbox; their routing role is passive transit evaluation by colony over the `params.graph` edges.

---

## `store`: typed persistent storage

**Task**: CRUD cell with its own `cell.db`. Schema and column types can be defined in `params.schema`; the cell creates the tables from it. Dynamically it can also create a new table per message. Table and column names pass a syntax gate (P3, 2026-08-08): `[A-Za-z_][A-Za-z0-9_]{0,62}`, no `sqlite_` prefix, no `_fts` suffix. The only strings ever formatted into SQL are what the SQLite catalog (`sqlite_master`/`pragma_table_info`) itself returned or values from an internal enum — caller text reaches statements exclusively as bind parameters.

**Emission mode**: atomic-emitting. Per query message one response message with the result as a turn.

**Input format** (Phase-9 brainstorm E7, analogous to `bash`): structured JSON args in the `tool_call` turn. Mandatory field `operation` (`"insert"`/`"select"`/`"update"`/`"delete"`/`"create_table"`/`"search"`/`"traverse"`/`"similar"`/`"set_alias"`/`"canonicalize"`/`"alias_candidates"`/`"reject_pair"`) + `table`, plus operation-specific fields:

- `insert`: `row` (object `{ "<column>": <value> }`).
- `select`: `columns` (**mandatory**, array of column names with at least one entry; the projection) + optional `where`, `order_by` (array of `{ "col": "<column>", "dir": "asc"|"desc" }`, multi-column) and `limit` (integer ≥ 1, **no** implicit default, no cap — the runaway guard is `query_timeout_ms`) plus `distinct` (bool, default `false`, GH #68): it deduplicates **the projection** — two rows that agree on every requested column are one answer, and a `limit` then counts answers instead of rows. That lets the store settle a set question (which values does this column combination carry?) where the rows are, instead of shipping all of them over the mailbox first. Under `distinct` every `order_by` column has to be **projected** (otherwise `invalid_input`): SQLite accepts the other form and sorts by a value the deduplicated rows disagree on — which row survives and where it lands would be unspecified, and a prefix of an unspecified order is not one. There is **no** projectionless `SELECT *`: if `columns` is missing or empty, the cell answers with `finish_reason: "error"` and `error_code: "invalid_input"` (no cell crash; doc-to-code correction, ruling 2026-08-08). The result is an array of row objects, projected onto the requested columns.
- `update`: `set` (object) + optional `where`.
- `delete`: optional `where`.
- `create_table`: `columns` as a **2-level map** `{ "<column>": "<type>" }` (types `text`/`int`/`json`), **not** `schema`.
- `search` (P3): `match` (**mandatory** — FTS5 query syntax) + `columns` (**mandatory**, as in `select`) + optional `where`/`order_by`/`limit`. Since 0.2.0 the `match` text runs through the same stemming tokenizer as the index (see `params.fts`), so search term and index term are folded the same way. Only on tables with a `params.fts` declaration (otherwise `invalid_input`). Every result row additionally carries a `rank` column (bm25, smaller is better); without `order_by`, `rank` is the default ordering.
- `set_alias` (0.2.0): `alias` + `canonical` (both **mandatory**, non-empty) + optional `recorded_at` and `column`. Writes into the alias table of `table`'s `params.canonical` binding — an **upsert** on `alias`, so calling it again with the same alias is a correction, not a second row. `table` names the **bound** table (e.g. `facts`), never the alias table. `column` names the `source` column of the binding meant and is **mandatory as soon as the table carries more than one binding** (otherwise `invalid_input`) — an alias is a statement about EXACTLY one identity dimension. Without a binding ⇒ `invalid_input`. Resolution is **one hop** and never transitive: whoever writes an alias writes it already resolved. Under a normalising binding, `alias` and `canonical` are stored in their normal form, so one judgement covers every spelling that differs only in case, whitespace or Unicode composition.
- `canonicalize` (0.2.0): re-derives the bindings' target columns for **every** row from the original plus the alias table. Optional `column` (0.2.0 P4) narrows it to one dimension; without `column` **every** binding of the table runs. `rows_affected` counts only the rows whose value actually **changed**, summed over the dimensions — a second run over unchanged data reports 0. This is also the **revert path**: remove the alias row with `delete`, run `canonicalize`, and every row falls back onto its untouched original.
- `alias_candidates` (0.2.0 P4): returns **candidate pairs** of similar values of a binding's derived column — the feed of the nightly GC. Args: `column` (mandatory as soon as the table carries more than one binding) + optional `limit` (default 20), `min_score` (`0.0`–`1.0`, default `0.5`) and `max_values` (default 500, cap 5000 — the comparison set, quadratic in runtime). The result is an array of `{ left, right, score }`, sorted by `score` descending and then alphabetically, hence **stable** across runs. The score is a trigram Dice coefficient over the normal form of both sides (hand-built, no extension). Pairs that are already **settled** are excluded, in both directions: accepted ones (both sides point at one identity through the alias table) and, since 0.2.0 P5, refused ones (the pair sits in the binding's `rejected` table). Otherwise the GC would propose the same settled pairs every night and pay a top-tier model again for every refusal it already made. The op **merges nothing**: it reads, scores and sorts — the judgement is the GC's, and what it persists is an ordinary `set_alias`.
- `reject_pair` (0.2.0 P5): remembers a judgement's **No** — two candidates are NOT the same identity. Args: `left` + `right` (both **mandatory**, non-empty, not equal) + optional `recorded_at` and `column` (mandatory as soon as the table carries more than one binding). Writes into the binding's `rejected` table — an **upsert** on the pair, so re-judging is a correction, not a second row. The pair is **unordered**: both sides go through the same key an alias does (under a normalising binding, therefore the normal form) and are stored in a fixed order, so `(a,b)` and `(b,a)` are one row. Without `params.canonical.rejected` ⇒ `invalid_input` — there would be nowhere for the refusal to live. Effect: `alias_candidates` stops proposing the pair. The revert is the ordinary `delete` on that table, after which the pair is a question again. The op touches **no** row of the bound table.
- `traverse` (P4): multi-hop over an edge table via a recursive CTE, **directed** `src`→`dst`. Args: `table` + column roles `src`/`dst` (optional `kind`/`weight` — all catalog-validated), `start` (bind value), optional `where` (full operator set, applied per edge) and `columns` (additional edge columns in the path rows), guards `max_depth` (default 2, cap 5) and `max_nodes` (default 200, cap 5000) — values above the cap ⇒ **reject** (`invalid_input`), no silent clamping. Cycle elimination per path including the start node (an edge back to the origin is pruned). The result is an **object payload** `{ paths, truncated, max_depth, max_nodes }`; every path row carries end node, depth, path array, edge attributes and accumulated weight. **No** `order_by` (BFS-style expansion; the order within one depth is not part of the contract); `truncated: true` makes the `max_nodes` cutoff visible.
- `similar` (P4): similarity ranking over a vector column via the registered `hamming()` scalar function. Args: `table`, vector column, query vector (bind), optional `where`/`order_by`/`limit`, `columns` (must **not** contain `distance`). Every result row carries `distance` (smaller is better); default ordering is `distance` ascending with a `rowid` tiebreaker. Vectors are **Base64 TEXT** (primary; real BLOBs are additionally accepted — a native blob write path is a roadmap defer), strict Base64 (reject on alphabet, padding and length errors), `NULL` → `NULL`; **a length mismatch between two vectors ⇒ loud `sql_error`** (a mismatch is almost always a breach of the embedding-generation discipline, never a silent skip). The op **always** implicitly adds `<vector column> IS NOT NULL` — `NULL` embeddings (backfill queue) would otherwise rank first. Known limits: no enforced model equality (the caller filters `model_id` itself), no ANN index — full scan over the filtered set.

`columns` thus has a different form depending on the operation: with `select` an **array of column names** (projection), with `create_table` a **2-level type map**. `where`: per column either a bare value (shorthand for `eq`) or an operator object with exactly one key out of `eq`, `neq`, `lt`, `lte`, `gt`, `gte`, `in` (array), `is_null` (bool), `or_null` (wrapping exactly one comparison operator, depth 1). An object with an unknown key ⇒ `invalid_input`. The operator forms apply uniformly to `select`/`search`/`update`/`delete` (one shared `build_where` path). `schema` is exclusively the `params` block (bootstrap tables). Phase 9 accepts only `tool_call` turns; direct use with `user`/`system` origin (see below) is a Phase-9 limitation.

**Two frozen properties of a read, so that a topology can rely on them.** `limit` **without** `order_by` returns an **unspecified** selection: SQLite is free to return any rows in any order, and a re-run on unchanged data may answer differently. A prefix of an unordered set is not a page — whoever pages, sorts. And `select` **without** `limit` is **uncapped**: there is no implicit default and no hidden ceiling, so a growing table eventually answers with the whole table over one mailbox. The only guard is `query_timeout_ms`, which bounds the time and not the row count. Both are deliberate (a silent cap would turn a complete answer into a truncated one without saying so) and both stay that way.

**Body format of the response**: `messages[]` with a single turn. In tool-loop use typically `{ origin: "tool", type: "tool_result", text: "<json-serialized result>", id: "<tool_call_id>" }`. In direct use outside a tool-loop, the `origin` may also be `user` or `system` depending on the application convention; `id` is then omitted.

**Output header** (`hop` compartment, expires on the next cell emission): `operation`, `rows_affected`, `duration_ms`, optional `error_code`.

**Failure classification** (Phase-9 brainstorm E5, analogous to `bash`) — **two families** (doc-to-code alignment, GH #109):

1. **SQL level ⇒ a regular `tool_result` turn** with `header.error_code` (`"sql_error"` / `"unknown_table"` / `"unknown_column"` / `"type_mismatch"` / `"constraint_violation"`) and **no** `finish_reason: "error"`. These are the failures the query finds while running: constraint violation, type mismatch, unknown table/column. Rationale: the LLM/caller reads the code and decides (retry, schema correction, different operation).
2. **Args level ⇒ an error message WITH `finish_reason: "error"`.** Three codes: `"write_denied"` (GH #132 — the store declares `write_surface: "internal"` and the sender lies outside the owning hive scope; the op is refused before it reaches the database, and unlike the two below the refusal keeps the `tool_call_id` so it stays correlatable in a tool loop), `"invalid_input"` (malformed body, no `tool_call` turn, `text` that is not JSON, unknown operation, missing projection, unknown operator key, guard violation, rejected `params` update) and `"query_timeout"` (`query_timeout_ms` interrupted the running query). This class either never reaches the database or is aborted halfway — there is no result a `tool_result` could report on. **The earlier wording sorted `invalid_input` and `query_timeout` into family 1; that was the documentation, not the code** (the doc comment in `store/cell.rs` has always described the split). Pinned in `crates/meclaw-cells/tests/fitness_store.rs`. **As-built detail**: the turn of an `invalid_input`/`query_timeout` rejection carries an **empty** `id` — such an answer cannot be correlated by `tool_call_id` inside a tool loop, only by ordering. `write_denied` is the exception: it is decided after the `tool_call` has been parsed, so the `id` is known and travels back.

Only internal errors (DB corruption, spawn error) trigger a cell crash + restart. Since P3, `unknown_column` also covers `select`/`where`/`order_by` (previously only the `insert` path via the SQLite error text). The `traverse`/`similar` failure cases (P4) map onto the existing codes — `invalid_input` for guard/arg violations, `unknown_table`/`unknown_column` via the catalog, `sql_error` for vector mismatch, `query_timeout` — **no new code**.

**`params`**:

- `schema` (Phase-9 brainstorm E6): 2-level map `{ "<table>": { "<column>": "<type>" } }` with types `text` / `int` / `json`. Constraints (PK / NOT NULL / UNIQUE / default / index) are **deferred** in Phase 9. A separate design pass is needed. An **existing** table is grown into the declaration at spawn: every declared column it lacks is added via `ALTER TABLE ADD COLUMN` (0.2.0) — `CREATE TABLE IF NOT EXISTS` alone is a no-op on an existing table, and an existing `cell.db` would otherwise silently carry one column less than the running code reads. Strictly additive: an existing column the declaration does not name is never touched, never retyped, never dropped (no-delete).
- `canonical` (0.2.0): map `{ "<table>": [ { "source": "<column>", "target": "<column>", "aliases": "<table>", "normalize": <bool>, "rejected": "<table>" }, … ] }` — declares one column the **derived identity** of another. A table may carry **several** bindings (0.2.0 P4: a fact has two identity dimensions, the relation and the entity it is about); a bare object instead of the list is read as a list of one, so a `config.json` written for 0.2.0 P2 keeps parsing unchanged. `source` is the written column (stays byte-identical), `target` the **store-owned** derived column, `aliases` the mapping table the store creates itself (`alias` PRIMARY KEY → `canonical`, plus `recorded_at`). Effect: `insert` and `update` fill `target` from the alias table on every write (a caller-supplied `target` value is dropped — the column belongs to the store); at spawn the store creates the alias table and backfills **empty** `target` values once (the same catch-up property the FTS index has). `source`/`target` must be different `text` columns of the declared table, `aliases` a syntactically valid name that is **not** in `params.schema`. Two bindings of one table have to stay independent: the same `source`, the same `target`, a `target` that is another binding's `source`, or a shared alias table (across tables as well) are declaration errors. `normalize` (default `false`, 0.2.0 P4) enables the **only automatic merge** the store performs: values are put into their normal form before lookup and storage (Unicode composition, case fold, whitespace collapse), so two spellings with the same normal form are ONE identity from the mint on — provable rather than judged. Anything beyond that (typos, edit distance) the store never merges itself; it reports it through `alias_candidates`. Normalisation composes the Latin-1 Supplement marks; a mark it does not cover leaves both spellings **different** — the price is a missed merge, never a wrong one. `rejected` (optional, 0.2.0 P5) names a second store-owned table: the memory of a judgement's **No** (`left_value`, `right_value` as PRIMARY KEY, plus `recorded_at`). The alias table cannot carry it — its `canonical` is `NOT NULL`, and a NULL there would read as "resolves to nothing". It is created additively at spawn, filled by `reject_pair` and excluded by `alias_candidates`; without it the store behaves exactly as in 0.2.0 P4. Held to the same rules as `aliases`: syntactically valid, not in `params.schema`, never shared with another binding. Immutable like `schema`.
- `fts` (P3): map `{ "<table>": ["<column>", …] }` — enables an FTS5 full-text index (external-content table + triggers) over the listed columns. Only tables from `params.schema`, only `text`/`json` columns; **no FTS for tables created via `create_table`** (known limit P3). Immutable like `schema`. Existing `cell.db`s build the index once on the next spawn — including rows written before the declaration. If the declaration drifts from the columns of the **existing index**, the shape of the drift decides (P15): if the existing columns are a **proper prefix** of the declared ones (purely **additive** drift, appended at the end), the index **and its three triggers** are dropped and rebuilt from the base table — the triggers have to go along, otherwise `CREATE TRIGGER IF NOT EXISTS` would keep the old column list alive and rows written after the migration would never reach the new column. Second exception (0.2.0): if the declared list arises from the existing one by replacing a `params.canonical` binding's `source` with its `target` (**canonical** drift), it is rebuilt as well — that is exactly the migration by which the keyword leg moves from the written spelling to its canonical twin. The two classes **compose** (0.2.0 P4): the substitution is applied first and the additive rule then runs over its result, so a store that skipped a release migrates in ONE wake instead of being refused. Every other column drift (column removed, reordered) stays a loud spawn error.

- **Stemming (0.2.0)**: every FTS index is declared with the store's own FTS5 tokenizer `meclaw_stem_v1` (`tokenize='meclaw_stem_v1'`). The tokenizer is a **wrapper around `unicode61`** — splitting text into words, case folding and diacritic removal stay there; what is added is a conservative light stemmer over the individual token. FTS5 runs a table's tokenizer over the indexed text **and** over the query text, which is how a plural and a singular meet on one term: since 0.2.0 `"lieblingseditoren"*` reaches the index term `lieblingseditor`, which was impossible before (FTS5 can only prefix-match at the **end** of a word). Two steps, each firing at most once, on the already case- and diacritic-folded token, minimum stem **3 characters**: (1) `-s` when the preceding character is one of `b d f g h k l m n r t w` (English plural, German genitive — the guard keeps `haus`, `atlas`, `bonus` whole); (2) `-ern` (>5 characters), else `-em/-en/-er/-es` (>4 characters), else `-e` (>3 characters). No Snowball, no derivational morphology, no umlaut expansion — the restraint is deliberate (over-stemming on German compounds). **Migration**: a third drift class next to additive and canonical — an existing index that does not declare the tokenizer is dropped and rebuilt through it. This happens automatically on the next spawn, with no tool and no manual step. The name is **versioned** (`_v1`) precisely because it is the only migration signal an index carries: a change to the stemming rules bumps the suffix, which turns the rebuild on for every existing index, and the previous spelling is kept registered on the connection so the old index stays openable long enough to be dropped. **Connection-bound**: the tokenizer lives on the SQLite connection, not in the file — a connection without it cannot even open an index that declares it. The substrate registers it on every birth path of a `store` connection (wake, respawn, `DbConn` re-open); an external tool that opens the `cell.db` directly cannot read the `<t>_fts` table — the base tables are unaffected.
- `write_surface` (optional, GH #132): `"open"` (default, and what an absent key means) or `"internal"`. **Opt-in writer boundary.** `"open"` is the historical behaviour — whoever is wired to the store's port may write. `"internal"` declares the write surface internal to the **owning hive scope**, which is the store's own parent path: a write op whose sender lies outside that scope is refused at the cell with `finish_reason: "error"` and `error_code: "write_denied"`, before anything reaches the database. **Reads stay free from anywhere** — `select`, `search`, `traverse`, `similar` and `alias_candidates` are never bounded. Bounded are `insert`, `update`, `delete`, `create_table`, `set_alias`, `reject_pair`, `canonicalize` and the β `params` slot (it persists into `cell.db`, so it is a write by the same definition). The sender is the one the **colony** stamps (`reply_to`), never a body field — an identity a caller could write into the body is not an identity; a message without a sender (a source message from an ingress or an event) is outside the scope, fail-closed. `write_surface` is **immutable**: a boundary a message can switch off is not a boundary. Known limit: a store sitting directly under the colony root has `/` as its owning scope, which contains every cell — the declaration is then inert, and the store logs a warning at spawn saying so. Enforcement is at the cell, because the substrate's edges carry no read/write distinction (the op travels in the message, not in the edge); the wiring-level half of the same boundary is the hive port declaration (`params.ports`, GH #133), which stops an outside cell from addressing the store at all.
- `query_timeout_ms` (concept A, see overview § Timeouts): per-query enforced timeout via `DbConn`'s `InterruptHandle`; demonstrably also interrupts a running recursive CTE (`traverse`).
- Optional seed data (convention path `seed/<table>.jsonl`). Seed takes effect only on `OpenStatus::Created` of the `cell.db` (see overview § Seed concept).

**Runtime param updates (β, `config.md` § Access L.20):** like `llm` (see there): top-level `params` body slot, partial, last-write-wins, persisted in the `cell.db`, replayed over the birth params on wake/respawn. **Mutable:** `query_timeout_ms`: takes effect **immediately live** (the running `DbConn` adopts the new A-timeout for the next query, without wake/respawn). **Immutable per `store`:** `schema`, `fts`, `canonical` (bootstrap-only, baked into the `cell.db` via DDL at spawn; a runtime change would desynchronize the live tables from the declared schema) and `write_surface` (GH #132 — a boundary a message can switch off is not a boundary). An update attempt on one of these or an unknown key ⇒ loud reject (`error_code: "invalid_input"`), no partial apply. **Under `write_surface: "internal"` the params slot is itself a write** (it persists into `cell.db`): an update from outside the owning hive scope is refused with `write_denied` **before** the merge, so not even an overlay is left behind.

---

## `llm`: LLM inference via provider adapter

**Task**: bridge to an LLM provider. Consumes and emits universal body format (see `meclaw-overview.md` section "Body format (universal)"). **No inner loop**: exactly one provider call per inference message. Iteration (tool-loops, ReAct, plan-and-execute, …) arises through topology.

**Emission mode**: atomic-emitting. Per inference call the `llm` cell emits exactly one new assistant turn. The incoming `messages[]` is **not** passed through. Whoever wants to hold the conversation thread together across multiple steps builds that via topology (e.g. a memory hive in front of the `llm` cell that aggregates history and passes it to the next call). Consistent with the "messages are atomic" discipline and the cell-emission-mode table in `meclaw-overview.md`.

**Inference trigger**: exclusively `messages[]`. System updates (paths under `system.*`) accumulate in `cell.db` without a provider call.

**State in `cell.db`**:
- `system.*`: accumulative-replace per path. Bootstrap context (persona, tool schemas, facts). Updates arrive per message from arbitrary cells; the sender does not know the structure. **Since GH #118 these message writes are gated** (size and slot limits always, an optional slot allowlist) — see "System write gate" below. Leaves sit in `cell.db` as `{"text": …}`: since GH #86 a `{text_id}` leaf no longer reaches the cell at all, the substrate resolves it at the delivery boundary. A row persisted **before** #86 can never cross that boundary again — if a leaf read back from `cell.db` still carries `text_id`, that is a **loud error of the call** since GH #95 (`error_code: "provider_error"`, `meta.error.source: "translate"`, every affected `slot_path` named) instead of a silent drop out of the system prompt; no provider call, no restart. The way out is to re-send the slot with inline text, whose upsert overwrites the row.
- `messages[]`: last-received as-is (no appended turns). Blob refs are already resolved here; since GH #19 the substrate expands `messages_id`/`text_id` before `handle()`, and the cell never sees a pointer.
- **Not in cell.db**: appended assistant turn (output), blob cache (in-memory only).

**Seed (`seed/system.jsonl`, GH #99)**: the static layer underneath the accumulated `system.*` state. It lets a template ship a default identity instead of starting the cell selfless and waiting for the first `system.*` update message — the agent is operational from boot on (degraded until it is briefed, never wrong). Same format as the `store` seed (overview § seed concept): line 1 is the schema header, lines 2+ are the rows.

```
{"schema": {"slot_path": "text", "value": "json"}}
{"slot_path": "identity", "value": {"text": "You are a research assistant."}}
{"slot_path": "instructions.tone", "value": {"text": "Answer briefly."}}
```

- A row is **exactly one leaf**: `slot_path` is the dotted slot path, `value` the UBF leaf. The semantics are those of an ordinary `system.*` update (upsert per path); `updated_at` is stamped by the loader at seed time, the file does not carry it.
- **Plain-text leaves only.** A `{"text_id": …}` leaf in a seed is a **loud configuration error at spawn time** — the substrate resolves that pointer class at the delivery boundary (GH #86), which a leaf written directly into `cell.db` never passes. Rejected as well: a nested subtree as `value` (then the nesting belongs in the `slot_path`), an empty `slot_path`, a missing header.
- The seed applies **only on `OpenStatus::Created`** of the `cell.db` — a re-open is never re-seeded, otherwise the template default would overwrite the grown identity on every restart (overview § seed concept).
- The seed is parsed **before** the `cell.db` is created: a rejected seed leaves behind no empty `cell.db` that would make the repaired seed look like a "resume" and skip it forever. The same parse path hangs off `meclaw --validate` (validate-equals-spawn).
- **A missing file is not an error** (the normal case for every cell without a seed).

**`params`**:
```json
{
  "provider":    "openai",
  "model":       "gpt-4o",
  "api_key":     "${OPENAI_KEY}",
  "base_url":    null,
  "temperature": 0.7,
  "max_tokens":  4096,

  "external_timeout_ms":   110000,
  "attachment_timeout_ms": 5000,

  "system_order":   ["identity", "facts", "instructions", "tools"],
  "provider_extra": { },

  "system_max_slots":      256,
  "system_max_leaf_bytes": 65536,
  "system_writable":       [ ],

  "http_referer": "${OPENROUTER_HTTP_REFERER}",
  "x_title":      "${OPENROUTER_X_TITLE}",

  "reasoning_effort": null,
  "reasoning":        null,

  "auth":                 "api_key",
  "auth_ref":             null,
  "wire_dialect":         null,
  "oauth_token_endpoint": null,
  "oauth_client_id":      null,
  "oauth_originator":     null,
  "oauth_client_version": null
}
```

- `external_timeout_ms` (concept A, see overview § Timeouts): A-timeout around the provider HTTP call (`tokio::time::timeout`), default `110000` (110 s). On Elapsed: regular error message with `finish_reason: "error"`, `error_code: "timeout"`.
- `attachment_timeout_ms` (concept A, GH #87): A-timeout around **one** `attachments[]` blob read from the store, default `5000` (5 s). Much smaller than `external_timeout_ms`, because a blob read is a local filesystem read, not a provider round trip. On Elapsed: regular error message with `finish_reason: "error"`, `error_code: "timeout"`, whose detail names the attachment id. Without effect for a cell without `consumes.body.attachments`, which never reads a blob.

- `provider` (Phase 8): **`"openai"` only** (including OpenAI-compatible endpoints via `base_url`). The value is set up as an enum, but Phase 8 implements exclusively the OpenAI translate. Further providers (in particular `"anthropic"`, Messages API native) are **deferred**, no fixed phase reference (see "Multi-provider" below). A non-`openai` value is in Phase 8 a `model_not_found`/`invalid_input`-equivalent configuration error at spawn.
- `auth` (P10): **`"api_key"`** (default) | **`"oauth_subscription"`**. Selects the credential source, **not** the provider. Exactly **one** credential per cell: `api_key` is required for `"api_key"` and forbidden for `"oauth_subscription"`; `auth_ref` the other way round. Any violation is a configuration error at spawn whose message **never** names a param value.
- `auth_ref` (P10): path to an OAuth token store in the Codex `auth.json` format. Required for `auth: "oauth_subscription"`, forbidden otherwise. **No default, deliberately.** An implicit `~/.codex/auth.json` would let a cell rotate the `refresh_token` of a live interactive session; sharing a store is therefore a config decision, not a code decision.
- `wire_dialect` (P10): **`"chat_completions"`** | **`"responses"`**; `null` derives it (`api_key` → chat-completions, `oauth_subscription` → responses). A separate axis **orthogonal to `provider`**: the Responses API is the same vendor with a different wire shape, not a different provider, so the `provider` constraint above is untouched. `auth: "oauth_subscription"` with `"chat_completions"` is a configuration error (the subscription backend speaks Responses only).
- `oauth_token_endpoint` / `oauth_client_id` / `oauth_originator` (P10): overrides for the OAuth refresh defaults and the `originator` request header. `null` = provider default. They exist so an endpoint drift is fixable without a release, and so tests can point at a fake.
- `oauth_client_version` (P14): value of the `version` request header on the subscription lane. `null` = provider default (`0.147.0`). Same rationale as above, only sharper: the backend **gates model availability on this header** — an unexpected value is answered with `400` and "The '<model>' model requires a newer version of Codex." Without this param, a backend-side bump of the floor would kill the lane until the next release. On the metered lane the header stays this crate's own version.
- `base_url` overrides the provider default (useful for local/proxied endpoints like LiteLLM, Ollama, vllm, all over the OpenAI-compatible wire).
- `system_order`: optional order of the `system.*` sub-slots when concatenating into the provider system string. Sub-slots not listed come afterwards in alphabetical order.
- `system_max_slots` / `system_max_leaf_bytes` / `system_writable` (GH #118): the write gate in front of the persistent `system` tree. See "System write gate" below.
- `temperature` / `max_tokens`: on the `oauth_subscription` lane **neither is transmitted** — the backend rejects them with "Unsupported parameter" (P14, measured live). On the official Responses API and on chat-completions they work unchanged, which is why the cut is on `auth`, not on `wire_dialect`. A caller who does need one on the subscription lane sets it via `provider_extra` (that overlay runs after the body inserts and wins).
- `provider_extra`: free JSON block for provider-specific knobs (Phase 8: e.g. OpenAI `seed`). Overlay over common params on conflicts. Provider-foreign knobs (e.g. Anthropic `cache_control`) are active only with the respective provider translate.
- `http_referer` / `x_title`: optional provider attribution (OpenRouter `HTTP-Referer` / `X-Title`). **Regular params** (audit ruling A4, params-uniform): set in `config.json`, substituted via `${VAR}` from `.env` like any other param, **no** code path reads `.env` directly, **no** special header mechanics. Unset (`null`/omitted) ⇒ the header is **not** sent. The wire target (HTTP request header instead of request body) is decided by the translate boundary (see "Provider translate" below).
- `reasoning_effort` / `reasoning` (GH #124, **`wire_dialect: "chat_completions"` only**): the deliberation budget for a thinking-class model. `reasoning_effort` is the shorthand (`"low"` / `"medium"` / `"high"`, or whatever else the provider accepts — the value is **passed through, not validated**) and becomes `"reasoning": {"effort": …}` in the request body; `reasoning` is the object block taken over **verbatim** (e.g. `{"effort": "low", "exclude": true}` or a `max_tokens` budget) for everything the shorthand cannot express. **If both are set, `reasoning` wins** (the strictly more expressive form; they are **not** merged). **Regular params** like `http_referer`/`x_title` (audit ruling A4, params-uniform) and changeable at runtime via the `params` slot — a deliberation budget is a knob, not an identity. Unset (`null`/omitted) ⇒ the field is **not** sent and the request is byte-identical to one without these params. On the Responses lane the topic is already covered by the dialect (`reasoning` items, P10) — these two params have **no** effect there. `provider_extra` is overlaid **afterwards** and therefore also wins over a `reasoning` block set this way.

**Runtime param updates (W4b, `config.md` § Access L.20):** params are cell **content**, not topology state. They change per **message**, not per mutation. The form is a **top-level `params` body slot** (1:1 with the `config.json` `params` block), partial, **last-write-wins per key**:

```json
{ "params": { "model": "gpt-4o-mini", "temperature": 0.4 } }
```

Order within a message: the `params` slot is merged **first** + persisted in the `cell.db`, **then** a possibly co-sent `system`/`messages` inference runs with the **updated** params (the same call already uses the new model / the new attribution). A **params-only** message (slot without `system`/`messages`) persists and stays silent (no emit, analogous to system-only). `config.json` thereby diverges from the live state, **intended**; on wake/respawn the cell replays its `cell.db` overlay over the birth params (`config.json` remains the instantiation snapshot). **Reset = `cell.db` wipe ⇒ bootstrap params** back.

**Immutable per llm** (update attempt ⇒ **loud reject**, `error_code: "invalid_input"`, **no** partial apply): `api_key` (credential, secret hygiene, mirror of the A4 `Authorization` ruling), `provider` (Phase-8 identity) and the entire P10 auth dimension — `auth`, `auth_ref`, `wire_dialect`, `oauth_token_endpoint`, `oauth_client_id`, `oauth_originator`, `oauth_client_version`. Rationale for the extension: `auth`/`auth_ref` are credential identity, and `wire_dialect`/`oauth_*` decide **which endpoint** a credential is presented to; if they were mutable, a message could redirect an existing token to a new destination. GH #118 adds the system write gate — `system_max_slots`, `system_max_leaf_bytes`, `system_writable`: a message allowed to raise its own limit or clear its own allowlist would not be gated at all. **Unknown** param keys ⇒ likewise loud reject (no silent no-op). A malformed value (wrong type) ⇒ reject (all-or-nothing). The reject detail names only the key/the rule, **never** a param value.

**System write gate (GH #118)**: the `system` tree is long-term state. It is rebuilt into the prompt on **every** `handle()`, it carries the tool menu (`system.tools.*`), and it survives restarts. Without a gate, **any** cell with an edge to the `llm` cell could overwrite identity, instructions and tools durably, in any size and any number. The gate has two independent halves:

- **Bounds, always on.** `system_max_leaf_bytes` (default `65536`) bounds **one** leaf, `system_max_slots` (default `256`) bounds the number of **distinct** slots in the tree. Exceeding either is a **loud reject**, never a truncation and never a silent drop. The slot budget counts the **tree**, not the batch: overwriting a slot that already exists does not grow the tree, so a cell at its limit can keep refreshing its `handover` and merely cannot open new subtrees.
- **Allowlist, opt-in.** `system_writable` is a list of slot-path **prefixes** (relative to the `system` subtree: `"handover"`, **not** `"system.handover"`) that a **message** may write. Prefixes match on segment boundaries only (`"identity"` covers `identity` and `identity.soul`, never `identityx`). **Empty (the default) means no allowlist is configured** and every slot path stays writable, which is the pre-#118 behaviour: a system update addressed straight at the cell (the `@external` operator lane) and every topology writer (`handover` from the summarizer hive, `tools.*` from MCP discovery, `memory`/`consult` from the collector hive, `identity`/`instructions` from a persona cell) keep working unchanged.

All three are **immutable per llm** (see "Immutable per llm" above): a message able to raise its own limit or clear its own allowlist would not be gated at all.

**Careful when pinning: in the reference templates the identity arrives by message.** The persona cells of `bot-basic`, `slack-agent` and `egon` send `system.identity.soul` (and `instructions.style`) ahead of **every** turn, as a regular message. Leaving `identity` out of `system_writable` without unhooking that cell first turns every turn into an `invalid_input` reject instead of a write, from the next restart on. Truly freezing the identity is therefore a **topology** step (persona cell out, `seed/system.jsonl` in), not a config step alone. To close only the unknown surface, declare the prefixes that actually occur, typically `["identity", "instructions", "handover", "tools", "memory", "consult"]`.

**The reject is loud, on two channels**: a `WARN` line on the tracing target `meclaw::llm::system_gate` (fields `reason` in `not_writable` | `leaf_too_large` | `too_many_slots`, and `slot`) **and** a regular error message with `finish_reason: "error"`, `error_code: "invalid_input"` (the same closed enum as the params-update reject, because it is the same class of event: a message asking for something it is not entitled to), `meta.error.source: "parse"`, and a detail naming the slot path and the violated rule. **Never** a leaf value in the detail or in the log (the same secret hygiene as the params reject: a system leaf is prompt material). The incoming `messages[]` travel unchanged (gate-1 pass-through, so failover edges stay usable).

**All-or-nothing across the whole transaction**: a rejected `system` write also rolls back the `messages[]` half of the same message and does **not** reach the provider. There is no half-applied body.

**The seed does not pass this gate.** `seed/system.jsonl` is **configuration** on the same trust tier as `config.json`; the same hand writes both, side by side in the cell directory. It is the same tier split the params-update reject already draws (`config.json` may set `api_key`, a message may not). Checking a declaration against the seed that sits next to it would be circular, and it would lock a pinned cell out of its own identity at boot. The intended shape is the opposite: **seed the identity at boot, then pin the message-writable surface to what has to stay live** (typically `handover` and `tools`). The seed stays validated by `parse_system_seed` (see "Seed" above).

**Tool definitions**: live in `system.tools.<tool_name>.text` as JSON strings. The adapter parses them at the provider call and builds the provider-native tool set. Tools are **not** concatenated into the system-prompt string. Extracted separately. Tool calls and tool results are their own `messages[]` turn types (`type: "tool_call"` / `"tool_result"` with `id` as the correlation anchor, pass-through value from the provider).

**Attachments (`attachments[]`) — vision input (GH #87)**: an `llm` cell consumes file attachments **exactly when its contract declares `consumes.body.attachments`** (`config.md` § consumes; declaring is binding, which makes the slot mandatory on every inbound message). Only then does it receive a **read-only store handle** at spawn and resolve the `blob_id` refs **itself, at `handle()` time**. The substrate never inlines them (owner ruling GH #19, see `meclaw-overview.md` § "`attachments[]` schema"). A cell **without** the declaration holds no handle: the slot travels past it untouched and its provider request is byte-identical to the one without the slot.

- **What is consumed**: `image/*`. On `chat_completions` each attachment becomes an `image_url` content part of the request's last `user` message (whose `content` turns from a string into a content array: the text first, then the images); the URL is a self-contained `data:<mime>;base64,<…>` URL, so no dereferenceable link leaves the colony. The authority on the MIME type is the **sidecar**, which is what the store committed.
- **Failure modes are cell errors, not dead letters**. The message was delivered correctly; it is the attachment behind it that is unreadable. A non-image MIME type and a missing or uncommitted blob yield a regular error message with `error_code: "invalid_input"`; an elapsed `attachment_timeout_ms` yields `error_code: "timeout"`. Every detail names the **attachment id** and the reason, and the inbound `messages[]` travel along unchanged (gate-1 pass-through, so failover edges stay usable). An attachment that cannot be read does **not** reach the provider.
- **The declared MIME type is checked before the read**: a 40 MB PDF is rejected without ever entering memory.
- **Wire dialect**: implemented for **both** dialects (GH #87 `chat_completions`, GH #94 `responses`). On `wire_dialect: "responses"` each attachment becomes an `input_image` item in the typed `input[]`: `{"type": "input_image", "image_url": "data:<mime>;base64,<…>"}` — on this wire `image_url` is a **string**, not an object (pinned reference `ContentItem::InputImage`, `openai/codex` @ `266c6920`, `protocol/src/models.rs:716-734`). The items attach to the last `user` message of the `input[]`; without a `user` message they become an appended `user` message of their own. The error taxonomy and the data-URL form are identical on both dialects.

**Output body**:
- `messages[]` = only the new assistant turn (no pass-through of the incoming `messages[]`)
- `system.*` is **not** emitted (private cell state)
- `meta` (cell-specific top-level slot): `{ provider, model, response_id, latency_ms, started_at, tokens_cache_read?, tokens_cache_creation?, … }`

**Output header** (`hop` compartment, expires on the next cell emission):

| Header | Content |
|---|---|
| `finish_reason` | `"stop"` \| `"length"` \| `"tool_calls"` \| `"content_filter"` \| `"error"`, mandatory |
| `tokens_prompt` | input token count |
| `tokens_completion` | output token count |
| `model` | model the provider actually used |
| `error_code` | only on `finish_reason == "error"`: `"rate_limit"` \| `"auth"` \| `"timeout"` \| `"model_not_found"` \| `"provider_error"` \| `"invalid_input"` (W4b: param-update reject, immutable/unknown/malformed key) |

**The `error_code` enum is additively extensible.** New failure classes may add a value; existing values never change their spelling or their meaning (the same promise the dead-letter and mutation codes carry, `docs/meclaw-overview.md`). **A CEL condition must therefore not assume the list is complete** — match on the codes you handle and give the rest a default lane, because an unmatched code is a future release, not a bug in your topology. A planned addition is `quota_exhausted` (`docs/roadmap.md`), and it will arrive this way: an added value, nothing renamed.

**`meta.error` fine classification (P10).** The subscription lane deliberately took **no** new enum value; the discriminator a failover edge needs lives in `meta.error` instead:

| Case | `error_code` | `meta.error.kind` | extra |
|---|---|---|---|
| subscription quota spent | `rate_limit` | `quota_exhausted` | `resets_at` (unix seconds), `plan_type` |
| plan does not cover the model | `rate_limit` | `plan_not_included` | — |
| ordinary rate limit | `rate_limit` | `rate_limited` | — |
| token expired, even after refresh + one retry | `auth` | `auth_expired` | — |
| refresh token permanently dead | `auth` | `auth_permanent` | `re_login_required: true` |
| token store missing/unreadable | `auth` | `auth_store_unavailable` | — |
| 5xx / overload | `provider_error` | `transient` | — |
| upstream error **inside a 200 body** (GH #75) | that of the stated status | that of the stated status, else coarse (`rate_limited` / `unauthorized` / `model_not_found` / `provider_error`) | `in_body: true`, `upstream_status` (when stated), `upstream_message` |

Pre-P10 failure paths emit **no** `kind` — their message is unchanged.

**An error inside a 200 body (GH #75).** An OpenAI-compatible gateway reports an upstream failure
as a regular `HTTP 200` whose body carries **no** `choices` at all, only a top level `error`
object (`{"error": {"message": …, "code": 429}}`). That is not a malformed body, it is the normal
signal. The cell classifies this shape **before** it reads `choices[0]`, and through the **same**
status table a real HTTP status goes through: a 429 in the body lands in exactly the lane an HTTP
429 lands in (`rate_limit`), 401/403 in `auth`, 5xx in `provider_error` with `kind: transient`. If
the body states no status, the prose decides (rate-limit shaped sentences ⇒ `rate_limit`),
otherwise `provider_error`. `meta.error.source` is `wire` (not `parse`), and
`meta.error.upstream_message` carries the provider's own sentence. `missing choices[0]` stays
reserved for a body that has **neither** `choices` **nor** `error`.

**Phase instrumentation (GH #124).** Per provider call the cell writes **one** `INFO` line to the tracing target `meclaw::llm::latency` (`RUST_LOG=meclaw::llm::latency=info`) — tracing fields only, **no** UBF slot, **no** substrate change. Fields: `dialect`, `model`, `outcome` (`ok` or the `error_code`), `persist_ms` (body parse + the `cell.db` write transaction), `translate_ms` (system-tree read-back, tools, prompt concatenation, `attachments[]` resolution, request build), `provider_ttfb_ms` (until the response head — the time that sits with the provider), `wire_total_ms` (full HTTP roundtrip including body/SSE drain), `wire_attempts` (>1 only for the subscription lane's 401-refresh retry), `handle_ms` (the whole `handle()`) and `unaccounted_ms` = `handle_ms` − (`persist_ms` + `translate_ms` + `wire_total_ms`). The phases are **complete**: a large `unaccounted_ms` is itself a finding ("the time is in none of the measured phases"), not a gap in the record. An `Option` field without a value is **omitted**, never rendered as `0` — a line without `wire_total_ms` is a call that never reached the provider, and a line with `wire_total_ms` but without `provider_ttfb_ms` is a call the provider never answered. Additionally, on `DEBUG`, one detail line per built request (`request_bytes`, `input_turns`, `tools`, `image_parts`, `system_prompt_chars`) — **sizes and counts only**, never conversation content and never a credential. Paths that end **before** the request build (body-parse reject, params reject, system-only silence) emit **no** line: they called no provider.

**Aggregation over loops** (total cost, cumulative tokens): **not a cell feature**. A separate aggregator hive in the topology groups over `correlation_id` and augments pass-through headers (`cost_total_usd`, `tokens_total`). Rationale in `meclaw-overview.md` section "Metadata aggregation is topology".

**Error model**: provider errors (rate limit, auth, timeout etc.) are **regular output messages** with `finish_reason: "error"` + `error_code`, `messages[]` unchanged (no turn appended), `meta.error` with detail info. Topology can do failover via edge condition. Only internal errors (panic, bad params) trigger a cell crash + restart.

**Streaming**: not supported as **output** (single-message output), post-roadmap. This is distinct from the **transport**: the Responses dialect streams on the wire (`stream: true` is mandatory on the subscription backend, which has no non-streaming path). The cell consumes the SSE body **fully and non-incrementally** and folds it into **one** atomic message — this cell's atomicity guarantee is unchanged.

**Multi-provider**: Phase 8 implements **exclusively the OpenAI translate** (one provider per instance). **Anthropic is deferred, no fixed phase reference.** The cell logic (UBF consumption, `system.*` accumulation in `cell.db`, tool-definition extraction, atomic-emit, error model) is provider-agnostic; provider-specific is solely the translate (see "Provider translate" below). Failover/A-B test over multiple providers runs via topology (two `llm` cells + dispatcher hive under one hive scope), not cell-internally. Additionally conceivable post-roadmap: a cell-internal provider list for robust provider connection (the cell guarantees "communication to the provider works" via retries/failover).

**Provider translate (translation boundary)**: the `llm` cell is **provider-agnostic**. It consumes exclusively universal body format, accumulates `system.*` as UBF in its `cell.db` (UBF is thereby also its internal/persistent format) and emits exactly one assistant turn as UBF. All provider knowledge lives in a translation function (here "translate", synonymous with the "LLM provider adapter" named in `meclaw-overview.md`), which knows two directions: **UBF → provider-native request** (system concatenation, `messages[]` mapping, `system.tools.*` → provider-native tool set) and **provider-native response → UBF** (assistant turn including any `type: "tool_call"` turns, headers like `finish_reason`/tokens, `meta` slot). Consequences every Phase-8 implementer must observe:
- **No loop.** Exactly one provider call per inference message, then emit. Iteration is topology (see `meclaw-overview.md` "Iteration is topology").
- **No composing/decomposing of tool calls.** The cell does not assemble tool calls and does not resolve any. `tool_call`/`tool_result` are pure UBF `messages[]` turn types with `id` as the pass-through correlation anchor (value from the provider). Tool schemas are translated by the translate from `system.tools.*` into the provider-native tool set, that is format translation, not a tool-loop.
- **Wire merge of consecutive `tool_call` turns (request build, ruling 2026-06-11).** During UBF→request mapping the translate merges **consecutive** assistant `tool_call` turns into **one** provider-native assistant message with `tool_calls[]`. The OpenAI wire contract requires that an assistant message with `tool_calls` is immediately followed by `tool` messages for each `tool_call_id` (Run-4b wire finding: one-call messages before collected results → 400). This is pure wire-format translation within the translate boundary, not composing at the UBF level: UBF stays unchanged (one turn = one call = one `id`), `id`s stay pass-through. The response return path stays unchanged (each provider `tool_calls[i]` → its own UBF turn).
- **Provider-native JSON never leaves the translate boundary.** The cell core sees exclusively UBF; provider-specific structures exist only within the translate.
- **Param → wire-target mapping (audit ruling A4).** The translate boundary decides the wire target per param, request-body JSON vs. HTTP request header. Provider knowledge thus resides exclusively in the translate. The explicit table:

  | param | wire target |
  |---|---|
  | `model`, `temperature`, `max_tokens`, `provider_extra` (overlay) | request-body JSON |
  | `reasoning_effort` | request-body JSON `reasoning` as `{"effort": …}` (chat-completions only) |
  | `reasoning` | request-body JSON `reasoning`, verbatim (chat-completions only; wins over `reasoning_effort`) |
  | `http_referer` | HTTP header `HTTP-Referer` |
  | `x_title` | HTTP header `X-Title` |

  The header table is a **closed allow-list**: `Authorization` is **not** a params-controllable header. It is the `api_key` bearer and is set solely by the wire layer; a params attempt to override it is ignored (secret hygiene). Only set (`Some`) attribution params produce a header; unset ⇒ no header.

From this follows directly the deferral cleanliness: a further provider (e.g. Anthropic) is solely a second translate plus an enum value, the cell logic, `cell.db` semantics and the error model stay unchanged.

**Second wire dialect (P10): Responses.** Beside chat-completions the translate knows the Responses dialect — same translation boundary, same UBF semantics, different wire shape: `messages[]` → typed `input[]` items (`input_text`/`output_text`, `function_call`/`function_call_output`), system prompt → top-level `instructions`, `max_tokens` → `max_output_tokens`, tool schemas flat instead of nested. `store: false` is set (the subscription backend does not persist), `include: ["reasoning.encrypted_content"]` only on the subscription lane. The answer is read from `response.output_item.done`, **not** from the deltas; `reasoning` items never reach UBF.

The wire is pinned against the reference implementation `github.com/openai/codex` @ `266c6920d9b82fe4d68959529565256b12a9be99` (endpoint, header set, body shape, SSE events, refresh flow, error taxonomy); the test fixtures are the drift detectors. Endpoint: `https://chatgpt.com/backend-api/codex/responses` (subscription, **without** `/v1`) or `https://api.openai.com/v1/responses` (API key). The two are **not** interchangeable — a subscription token against the metered endpoint fails on missing scopes.

**Token lifecycle (P10, `auth: "oauth_subscription"` only).** The access token is **not a param** — it is cell-external, rotating state in the store behind `auth_ref`. Refresh is **purely reactive**: call → `401` → refresh → **exactly one** retry → typed error. No timer, no polling, no backoff loop; failover on quota exhaustion is **topology**, not cell logic (see error model).

**One refresher per process.** The `refresh_token` **rotates** on every refresh — two cells refreshing the same store concurrently produce `refresh_token_reused` and kill the login permanently. All cells in a process therefore share **one** token broker: an actor that performs the refresh call itself, which guarantees single-flight by construction (no lock, no wait loop). A cell that wants to refresh after a `401` names the token generation it used; if someone else refreshed meanwhile, it receives their fresh token instead of a second rotation. **Limit:** this serializes **within one process** — a CLI running in parallel on the same store can still collide (see `roadmap.md`).

**Secret hygiene (the `api_key` discipline extended to the token path).** A token is **never** in `config.json`, **never** in the `message_log`, **never** in an emitted message or its `meta`, **never** in an error text and **never** in a log. `Debug` output of the token types redacts its values; third-party error texts are stripped of token-shaped strings before being passed on. The store is written atomically (temp file + `rename`) and carries Unix mode `0600`.

**Store co-ownership.** The store also belongs to the vendor CLI — MeClaw is a **second writer** there, not the owner. Rotation therefore **patches instead of overwriting**: only `tokens.access_token`, `tokens.refresh_token`, `tokens.account_id` and `last_refresh` are touched, and all unknown fields (`auth_mode`, `id_token`, `OPENAI_API_KEY`, …) survive unchanged. A naive rewrite would destroy an interactive login on the first rotation.

---

## `bash`: shell execution

**Task**: runs shell commands, **one-shot only** (`cell.timeout > 0`, the cell terminates after each message). A persistent mode (`cell.timeout: -1`, long-lived interactive shell session) is **by-design not introduced** (architecture ruling 2026-06-08, design record in `archive/roadmap-resolved.md`): stateful, fragile, hard to sandbox. `cwd`/`env` continuity across multiple commands (if needed) runs via persisting `cwd`/`env` in the `bash` `cell.db` + passing it per one-shot call, not via a living shell. For program logic, body manipulation or multi-send see `code` (the choice heuristic is at the start of the `code` section).

**State model**: `bash` is **stateless** in the classical sense (stateless dispatcher, short-lived worker tasks) and has **no `cell.db`**, consistent with the Phase-7 discipline "tool cells without `cell.db`". Shell state (cwd, env vars, history, open processes) is not held across calls; each call starts a fresh shell.

**Emission mode**: atomic-emitting. Per executed command one `tool_result` turn.

**Body format of the response**: `messages[]` with one turn `{ origin: "tool", type: "tool_result", text: "<stdout-plus-possibly-stderr>", id: "<tool_call_id if present>" }`.

**stderr convention**: stderr lives **not** in its own header or body slot, but is appended in `text` after the stdout portion, demarcated by clear sentinel markers (inserted only when stderr is non-empty):

```
<stdout-content>

##meclaw-stderr-start##
<stderr-content>
##meclaw-stderr-end##
```

This way an LLM consumer reads the full tool output naturally (stdout first, stderr explicitly marked), and edges can route quickly via `header.had_stderr` before the `text` parse. Rejected were: stderr as its own header string (would break the "headers = small" discipline with large compiler outputs / stack traces), stderr as its own top-level body slot (breaks the natural LLM-consumer model "reading tool output means reading `text`" and increases slot inflation), and stderr always as a JSON struct in `text` (`{stdout, stderr, exit_code}`, not directly LLM-readable without a parse step).

**Output header** (`hop` compartment, expires on the next cell emission): `operation` (= `"bash"`), `exit_code`, `duration_ms`, `had_stderr` (mandatory, always set), `bytes` (**full** length of the combined output before a cut), optional `truncated` (`true` when `max_bytes` cut — GH #83, see below).

**`params`**: typically the command to execute or the script-path convention. `max_bytes` (byte cap on the returned output, default `262144` = 256 KiB, GH #83). Optional `sandbox` (S4/GH #35, completed in GH #85; schema in `config.md` § `params`), the process sandbox block for the spawned shell.

**Size cap (`max_bytes`, GH #83).** A command with runaway stdout has the same multiplying effect inside a tool loop as an uncapped `web_fetch` body: the tool result becomes a thread row and re-enters the prompt on **every** subsequent round. `max_bytes` (default 256 KiB — the same generous-but-finite value `web_fetch` uses) cuts the **combined** `text` (stdout plus the stderr sentinel block, if any) on a UTF-8 boundary. A trim is visible, never silent: `text` ends in `… [truncated, <N> bytes total]`, `header.truncated: true`, and `header.bytes` reports the **full** size before the cut. A cut may clip the stderr block — the marker and `header.had_stderr` stay the reliable signals. Inside an agent loop the value belongs much lower.

**Phase-7 conventions** (Slice-2 decisions):
- **`exit ≠ 0` is a NORMAL tool_result**: `exit_code` always in the header (even =0). The LLM/caller reads the code and decides. Consistent with Claude Code's Bash tool.
- **Only spawn failure, timeout + invalid input = error**: `error_code: "io_error"` (spawn) or `"timeout"` (external_timeout elapsed) or `"invalid_input"` (missing/invalid `command` field).
- **`exit_code = -1`** on signal-killed/abnormal termination (platform-unspecific convention). On timeout additionally `error_code: "timeout"`.
- **stderr sentinel format** (insert only when stderr non-empty):
  ```
  <stdout>

  ##meclaw-stderr-start##
  <stderr>
  ##meclaw-stderr-end##
  ```
- **`had_stderr: bool`** header ALWAYS set (true/false).
- **Security boundary via `params.sandbox`** (S4, GH #35; completed in GH #85). **Without** a `sandbox` block bash still has full FS access via the shell and full network, and the phase-7 trust model applies unchanged to that case; **a bash cell instantiated from a template gets the block automatically**, though (the default-deny cut, `config.md` § `params`). **With** `sandbox: {"trust": "restricted", ...}` the shell starts under a Landlock filesystem allowlist, inside a fresh network namespace when `network: "deny"`, under the declared `limits` (cgroup v2) and behind the declared `syscalls` filter (seccomp-bpf). Schema, the operating requirement for the caps and the fail-closed rule: `config.md` § `params`. **Particularly relevant for a shell:** under `syscalls.foreign_signals: "deny"` the script may only signal itself, so a `kill $!` on its own background job fails with `EPERM`.
- **Shell**: `/bin/sh -c <command>`. `cwd`/`shell` as params deferred (operator sets via `cd /x && cmd` inline).
- **No persistent bash** (`cell.timeout: -1`): by-design dropped (architecture ruling 2026-06-08). `bash` is one-shot only, not a deferred option.
- **Input minimal**: `{"command": "..."}`.
- **Defaults**: `max_concurrency: 4`, `external_timeout_ms: 60000`, `max_bytes: 262144`.

---

## `code`: programmable body constructor

**Choice of `bash` vs `code`** (for AI builders and template authors):

- Do you only need to "issue a command, emit stdout/stderr as a `tool_result` turn"? → **`bash`** (always one-shot, also for command sequences, `cwd`/`env` continuity if needed via the `bash` `cell.db` per call, see § `bash`).
- Do you need program logic that manipulates the body, makes several messages from one (multi-send), sets headers deliberately, or reworks incoming `messages[]`? → **`code`**.

**Task**: runs user-supplied program in a declared language (Python first; Node and others later). Unlike `bash`, `code` is a **body constructor**: the script gets the incoming message as JSON, builds the outgoing content JSON entirely itself: headers, `messages[]`, own top-level slots, routing-relevant headers for edges. This makes `code` the Swiss army knife for application-specific logic: dissecting LLM outputs, extracting tool calls, transform logic, multi-send dispatchers.

**Rationale for this role**: a simple subprocess wrapper analogous to `bash` would not cover this task surface. Body manipulation, multi-send and header routing need program logic, not just stdout-to-text. Rejected were: (a) `code` as a bash-like wrapper with "scalar lift" (the cell extracts only scalar header values from stdout, does not cover the real application surface, leaves the body untouched), (b) separate transform cells for each of these tasks (would enlarge the cell-type catalog with no added value), (c) making `bash` and `code` formally identical (would make `bash` unnecessarily heavy). With the body-constructor model the catalog stays lean, without having to invent new cell types. Trade-off: `code` and `bash` are not formally symmetric, that is intended and explicitly resolved for AI builders via the choice heuristic above.

**Emission mode**: **script-determined**: atomic-emitting or stream-propagating, depending on whether the script passes through the incoming `messages[]` or builds it anew. `code` is the only cell type without a fixed emission mode.

**Script interface**:
- **stdin**: a JSON document of **exactly three objects** (since 0.9.0) — `envelope`, `body`, `params`. All three are **always** set and always objects, even when empty.
- **stdout**: complete content JSON in exactly the form every other cell also produces. `header` section (optional) plus top-level slots. The **wire format is unchanged**: the script still writes a `header` section. Colony interprets this as `hop` (the isolated cell output, expires on the next cell emission), the rest becomes `message.body`. The script does **not** write `context` (that is solely edge authority). **The stdin structure changes nothing about stdout** — the emission is the same as before 0.9.0.

**The three objects on stdin.** `build_stdin_json` (`crates/meclaw-cells/src/code/wire.rs`) builds:

```json
{ "envelope": { "header": {"context": {}, "hop": {}},
                "target": "/x", "trace_id": "...", "ttl": 64,
                "reply_to": "/sink" },
  "body":     { "messages": [], "…": "further body slots" },
  "params":   { "window_size": 7 } }
```

- **`envelope`** — everything the substrate puts around the payload: `header` (both compartments `context` + `hop`), `target`, `trace_id`, `ttl`, plus `reply_to`, `parent_message_id` and `correlation_id` when the message carries them.
- **`body`** — the body slots of the incoming message (e.g. `messages`, `system`, cell-owned slots), verbatim.
- **`params`** — the read-only copy of the cell's own configuration, `{}` when nothing is left.

**The top level is closed by construction.** A script reads its payload from `body` — it does **not** derive it by subtracting a hard-coded envelope key list. That was precisely the failure class the structure ends: with a subtraction script every **new** top-level field fell into the body automatically and travelled on with the outgoing message. Future wire data therefore travels **inside** one of the three objects rather than beside them — and a body slot can no longer shadow `envelope` or `params`, because it no longer shares their namespace.

**The `params` are a read-only copy.** The cell reads nothing back from it; whatever the script changes there dies with the process. Two classes do **not** travel, recursively at every nesting level: (a) **credentials** — the keys `api_key`, `auth`, `auth_ref`, `token`, `secret`, `password` exactly, plus anything ending in `_key`, `_token`, `_secret` or `_password` (`auth` is an exact key and not a prefix, so `author` keeps travelling; `max_tokens` is a budget and keeps travelling too); (b) the script's **own source** (`script_inline` / `script_path`), which would double the wire payload of every single message without giving the script anything it is not already. A `code` cell is thereby configurable **per instance** without forking its script; `${VAR}` substitution (which applies at bootstrap **and** at mutation instantiation) stays the route for colony-global values. The context route is still **no** substitute for either: the `/colony` reply comes back with an empty context, a two-phase cell would lose its configuration exactly when it needs it.

**Multi-send**: when `multi_send_capable: true` (the source is `contract.multi_send_capable` from the cell's `config.json`; the earlier Phase-9 `params.multi_send_capable` bridge is **removed**), the script may write, instead of a single content JSON, a **JSON array** of content JSONs to stdout. The cell discriminates by the JSON root type:

- **JSON object** → one outgoing message (standard case).
- **JSON array** → N outgoing messages, one per element. Order: array order.

If `multi_send_capable: false` and the script writes an array → contract violation, error message with `error_code: "multi_send_not_declared"`. If `multi_send_capable: true` and the script writes an object → allowed, treated as an array of length 1.

Each emitted message runs **independently** through the cell's outgoing edges. Colony evaluates all edge conditions freshly per emitted message; one message can land at edge A, the next at edge B.

Wire example:

```json
[
  { "header": { "msg_type": "tool_call" },
    "messages": [{ "origin": "assistant", "type": "tool_call", "id": "call_a", "text": "..." }] },
  { "header": { "msg_type": "tool_call" },
    "messages": [{ "origin": "assistant", "type": "tool_call", "id": "call_b", "text": "..." }] },
  { "header": { "msg_type": "user_visible" },
    "messages": [{ "origin": "assistant", "type": "text", "text": "Three tools are being called in parallel." }] }
]
```

Rejected were: multi-send via NDJSON (line-delimited JSON, brings no advantage, because the cell waits for script end, no streaming need), multi-send with an explicit wrapper (`{ "messages": [...] }` as wrapper for the array, unnecessary, JSON-type discrimination suffices).

**Cell standard headers** (set by the cell itself after script end, **override** the script output for these keys):
- `exit_code` (number)
- `duration_ms` (number)
- `had_stderr` (bool)

The script cannot hijack these keys. Process metadata belongs to the cell.

**stderr** on a successful script run (exit 0): is **not** injected into the script output (the script's body construction stays clean). `header.had_stderr` is set, the stderr content lands in `log.jsonl` with warn level. On a script error (exit ≠ 0, see failure model) the cell instead emits an error message with stderr in the `bash` convention.

**Failure model** (complete `error_code` list):
- stdin not valid JSON (incoming message unparsable) → error with `error_code: "invalid_input"`, **no** DB write.
- script spawn fails (runner not startable) → error with `error_code: "io_error"`.
- `external_timeout_ms` elapsed (script run too long) → error with `error_code: "script_timeout"`.
- script exit ≠ 0 → cell discards the script output and emits an error message with `header.finish_reason: "error"`, `header.error_code: "script_failed"`, `header.exit_code`, `header.had_stderr`. Body: `tool_result` turn with stderr in the `bash` sentinel-marker form (stdout, then demarcated stderr block).
- script stdout not valid JSON → error with `error_code: "invalid_json"`.
- script stdout is valid JSON but not the wire shape → error with `error_code: "invalid_json"`. Two cases, both of them foreign input rather than a bug: an emitted message that is not an object (`[1]`, `["x"]` — the top level may be an array, but every ELEMENT must be an object), and a `header` key whose value is not an object (`{"header": 5}`). The reject is total: in a multi-send, a bad shape in any message yields exactly one `invalid_json` reply and zero regular emissions.
- script writes a JSON array without `multi_send_capable` → error with `error_code: "multi_send_not_declared"`.
- script stdout valid, but `contract.emits` violated → error with `error_code: "contract_violation"`. This `code` validation runs **always-on** (unconditionally, independent of build profile and `colony.json` `strict_validation`, `code` is the only user-script-driven trust boundary; see `meclaw-overview.md` § "Schema validation: timing and scope" and `docs/config.md` § Schema format and validation).

**`params`**: typically `runner` (canonically `"python3"` in Phase 9, `CodeParams::parse` rejects other values with `'params.runner: only "python3" is supported in Phase 9'`. Background: on the target platforms Ubuntu 24 / Python 3.12 the real binary is `/usr/bin/python3`, `python` deliberately does not exist there), script path or inline code, `external_timeout_ms` (concept A, see overview § Timeouts; default `60000`). **`multi_send_capable` is not (any longer) in `params`**. It comes from `contract.multi_send_capable` (see Multi-send above). Optional `sandbox` (S4/GH #35, completed in GH #85; schema in `config.md` § `params`), the process sandbox block for the spawned runner. A script under `trust: "restricted"` reads only the declared paths, so a script delivered as `script_path` rather than `script_inline` must itself live under one of them. **A code cell instantiated from a template without a block of its own gets the default-deny profile** (`config.md` § `params`); its runtime set is enough for a `script_inline`, while a `script_path` needs a declaration.

**`cell.db` for `code`** (Phase-9 brainstorm E9): **deferred** in Phase 9. DB access from script logic runs via topology (`code` → multi-send → `store`), not in-process. Whoever needs a collector/state pattern in `code` lifts that into a separate design pass.

---

## `web_fetch`: outbound HTTP client

**Task**: pure HTTP tool. Stateless (no `cell.db`). **Only `GET` is implemented** (Phase-7 Slice-3, see Phase-7 conventions below); `POST`/`PUT`/`PATCH`/`DELETE` including `method`/`headers`/`body` are a roadmap defer.

**Emission mode**: atomic-emitting. Per HTTP call one `tool_result` turn.

**Body format of the response**: `messages[]` with one turn `{ origin: "tool", type: "tool_result", text: "<response body>", id: "<tool_call_id>" }`. On a large body the **entire** output message is offloaded (from Phase 12) as `Body::Blob`, **whole-body offload** at the delivery boundary (`blob_inline_max_bytes` threshold, `resolve_blob_for_delivery`), **not** an in-message `text_id` pointer. This cell produces no in-message pointers; that the substrate can now **resolve** them (GH #19) does not change that: whole-body offload stays the form in which a large response leaves the wire.

**Output header**: `operation` (= `"web_fetch"`), `http_status`, `content_type`, `duration_ms`, `bytes`, optional `truncated`, optional `redirects` + `final_url` (only when at least one redirect was followed, GH #117).

**`params`**: `max_bytes` (byte cap on the returned body, default `262144` = 256 KiB, GH #83), `max_concurrency`, `external_timeout_ms`, `allow_private_networks` (default `false`, GH #117), `max_redirects` (default `5`, GH #117); later `base_url`, default `headers`, optional auth configuration.

**SSRF hardening (`allow_private_networks` / `max_redirects`, GH #117).** `web_fetch` runs **inside the daemon process** — no child is spawned, so `sandbox.network: "deny"` can never cover this cell. The cell therefore enforces its own egress policy, and that policy is **default-deny**.

- **Private-network deny.** Every target address is screened against a range matrix, **after** DNS resolution. IPv4: `0.0.0.0/8`, `10.0.0.0/8`, `100.64.0.0/10`, `127.0.0.0/8`, `169.254.0.0/16`, `172.16.0.0/12`, `192.0.0.0/24`, `192.168.0.0/16`, `198.18.0.0/15`, `224.0.0.0/4`, `240.0.0.0/4`. IPv6: `::`, `::1`, `100::/64`, `fc00::/7`, `fe80::/10`, `fec0::/10`, `ff00::/8` — and every v6 form that **embeds** a v4 address (`::ffff:a.b.c.d`, the deprecated `::a.b.c.d`, NAT64 `64:ff9b::/96`, 6to4 `2002::/16`) is judged by the address it embeds. Obfuscated spellings (`http://2130706433/`, `http://0177.0.0.1/`) are normalised by the URL parser before the deny ever sees them.
- **No DNS-rebinding window.** The pre-flight check produces the readable refusal; what makes it true is a dedicated reqwest DNS resolver that returns screened addresses and never hands a blocked one to the connector. reqwest therefore connects only to addresses that passed — the address that was checked **is** the address that is dialled. A name whose address set contains a private address is refused whole (deny-if-any).
- **Redirect policy.** Redirects are followed by the **cell**, not by reqwest (`redirect::Policy::none()`): reqwest's policy closure is synchronous and cannot resolve a name, so a hop it followed would never see the deny again — that is the classic bypass (public URL → `302 Location: http://169.254.169.254/`). Every hop, the first included, is re-screened **before** the connect; `max_redirects` caps the chain (exceeding it → `too_many_redirects`). A `3xx` **without** a `Location` is not a redirect but a document with a 3xx status and goes back as a normal `tool_result`. A `Location` naming a foreign scheme, and a downgrade from `https` to `http`, are refused (`invalid_redirect`); the upgrade direction `http` → `https` stays allowed. The **whole** chain lives inside **one** operation timeout (rule 12 A) — a redirect budget is not a time budget.
- **Opt-out, two tiers.** `allow_private_networks: true` opens the ranges a local topology can legitimately live in (loopback, RFC 1918, ULA, CGNAT, site-local) — for mock servers in tests and services on the same host. **Link-local (`169.254.0.0/16`, `fe80::/10`) stays shut in both tiers**: that is where the cloud metadata endpoint `169.254.169.254` sits, and nobody deliberately runs anything there. A non-boolean value for the opt-out is a param reject, never a silent reinterpretation.
- **A refusal is a regular error message** (rule-12 shape), not a panic and not a dead letter: `error_code: "target_blocked"`, on its own lane. Neither `io_error` (which reads as "network problem, retry") nor `invalid_input` (which reads as "the URL is broken") — a blocked target is neither, and the only repair is a different target.

**Size cap (`max_bytes`, GH #83).** A fetched body is a tool result, and inside a tool loop a tool result is re-sent to the model on **every** subsequent round — one large fetch does not cost one prompt, it costs every remaining prompt of the turn. `max_bytes` is therefore **generous but finite**: the default passes an ordinary document whole and stops a multi-megabyte payload. A trim is visible, never silent: `text` ends in `… [truncated, <N> bytes total]`, `header.truncated: true` (the declared header finally has a producer), and `header.bytes` reports the **full** size the server sent, not the size of what survived. Inside an agent loop the value belongs much lower (the worked example uses 32 KiB).

**Phase-7 conventions** (Slice-3 decisions):
- **GET only** in Slice 3. `method`/`headers`/`body` deferred.
- **Input minimal**: `{"url": "..."}`.
- **non-2xx HTTP status = NORMAL tool_result** with `http_status` header. The LLM/caller reads the status. Only DNS/connect/timeout/invalid input and the egress policy produce error messages (`io_error` / `timeout` / `invalid_input` / `target_blocked` / `too_many_redirects` / `invalid_redirect`).
- **The input gate PARSES the URL (GH #110)**: missing, non-string, empty, **syntactically broken** (`"not-a-url"`, `"http://"`) and foreign-scheme URLs (anything but `http`/`https`, `file://` included) are `invalid_input`, and the text quotes the URL. Before, the gate only checked presence and type; a syntax error surfaced from reqwest and was mapped to `io_error` — an agent repairing its own call reads `io_error` as "network problem, retry" and thus applies the wrong repair forever. A broken URL is a broken **call**. `io_error` stays what it should be: DNS, connect, transport. The URL itself travels on unchanged (the parser never re-serializes it).
- **TLS**: rustls (`rustls-tls` feature of reqwest); no OpenSSL/native-tls in the tree.
- **Header**: `operation: "web_fetch"`, `http_status: u16` (mandatory), `content_type: String`, `duration_ms`, `bytes`; after a redirect also `redirects: u64` and `final_url: String` (GH #117 — a body that came from somewhere other than the requested url says so).
- **Truncation**: `max_bytes` (GH #83, see above) cuts visibly; below it large bodies stay inline in `text` and are offloaded as a whole-body blob when needed.
- **`reqwest::Client` per cell instance** (internally Arc, no Mutex). Build error at spawn → spawn error. RespawnFn clones the initially built client.
- **Defaults**: `max_concurrency: 32`, `external_timeout_ms: 30000`, `max_bytes: 262144`, `allow_private_networks: false`, `max_redirects: 5`.

---

## `web_search`: web-search client

**Task**: pure search tool, talks to an external search provider (e.g. Brave, Tavily, SerpAPI). Stateless (no `cell.db`).

**Emission mode**: atomic-emitting. Per search request one `tool_result` turn.

**Body format of the response**: `messages[]` with a `tool_result` turn whose `text` contains the search results as a JSON list (title, URL, snippet per hit). On large result lists (from Phase 12) whole-body offload of the entire message as `Body::Blob` at the delivery boundary, **not** via an in-message `text_id` pointer.

**Output header**: `operation` (= `"web_search"`), `result_count` (**full** provider count, even when the list was cut), `duration_ms`, `bytes` (**full** size of the provider response), optional `truncated` (`true` when `max_results` or `max_bytes` cut — GH #83, see below).

**error_codes**: `io_error` (DNS/connect error), `timeout` (external_timeout elapsed), `invalid_input` (missing/invalid `query`). A merely non-conformant provider response is **not** an error (see Phase-7 conventions: `result_count=0`, body passed through).

**`params`**: typically provider `base_url` and API token (via `${VAR}` substitution). `max_results` (list cap, default `10`, GH #83) and `max_bytes` (byte backstop on the `text`, default `262144` = 256 KiB, GH #83).

**List cap (`max_results`) + byte backstop (`max_bytes`), GH #83.** A result list is a tool result, and inside a tool loop it re-enters the prompt on **every** subsequent round. A conforming list with more than `max_results` hits (default 10 — a full first page) is trimmed in place: the JSON stays valid and carries the cut **visibly where the model reads** (`"truncated": true`, `"total_results": <N>` in the object — the hop header does not travel into the thread row). `header.result_count` keeps the full provider count, `header.bytes` the full response size. `max_bytes` is the backstop for what the list cap cannot catch (a non-conforming pass-through body, absurdly large snippets) — the identical `web_fetch` convention: cut on a UTF-8 boundary, `… [truncated, <N> bytes total]` marker in `text`. When neither cap bites, the provider body passes through **byte-identical** (no re-serialization).

**Phase-7 conventions** (Slice-3 decisions):
- **Generic JSON wrapper**: the cell does GET `<params.endpoint>?q=<query>` with optional `params.api_key` as bearer token. Expects response `{"results":[{"title","url","snippet"}]}`.
- **Provider-specific adapters** (Brave, Tavily, SerpAPI, …) are **deferred**. Application topology via a `code` cell (Phase 9) or builder-hive normalizes.
- **Input**: `{"query": "..."}`.
- **Graceful on non-conformant response**: `result_count=0` when the `results` key is missing or not an array. The body is ALWAYS passed through in `text`, **no hard error**.
- **Header**: `operation: "web_search"`, `result_count: u64`, `duration_ms`, `bytes`. (The `http_status` header is deferred here, parity with web_fetch would be more consistent, but is post-Slice-3.)
- **Truncation**: `max_results` + `max_bytes` (GH #83, see above) cut visibly; below them large result lists stay inline in `text` and are offloaded as a whole-body blob when needed (Phase 12).
- **`reqwest::Client` per cell instance** (analogous to web_fetch). Build error at spawn → spawn error. RespawnFn clones the client.
- **Defaults**: `max_concurrency: 8`, `external_timeout_ms: 15000`, `max_results: 10`, `max_bytes: 262144`.

---

## `file`: filesystem operations

**Task**: CRUD for files within a security boundary. Path traversal outside the boundary is rejected. Stateless.

**Emission mode**: atomic-emitting. Per operation (`read`/`write`/`list`/`stat`) one `tool_result` turn.

**Body format of the response**: `messages[]` with a `tool_result` turn. On `read`, `text` contains the file content (on large files from Phase 12 whole-body offload of the entire message as `Body::Blob` at the delivery boundary, **not** via an in-message `text_id` pointer). On `write`/`list`/`stat`, `text` contains a JSON-structured status (bytes written, file list, stat info).

**Output header**: `operation` (`"read"`/`"write"`/`"list"`/`"stat"`), `bytes`, `duration_ms`, optional `encoding` (only `read` with `mode: "base64"`, GH #106).

**`params`**: `base_path` (mandatory; security boundary).

**Read modes and byte ranges (GH #106).** By default `read` is a **text** read: `text` carries the file content, `bytes` its byte length, and a non-UTF-8 file is a typed `io_error`. Two optional arguments open up the rest:

- **`mode`**: `"text"` (default; absent and `null` mean the same) or `"base64"`. In base64 mode `text` is standard-alphabet base64 (RFC 4648 §4, padded) of the **raw** bytes, the emission additionally carries `header.encoding: "base64"`, and `header.bytes` stays the **raw** byte count (not the encoded length). That makes a `.pyc`, an object file or a PNG header inspectable at all instead of merely refused. The encoder is hand-rolled (closed tech-stack allow-list) and pinned against the RFC vectors.
- **`offset` / `limit`**: a window in **BYTES**, in either mode. `offset` >= 0 (default 0), `limit` >= 1 (default: the rest of the file); `limit: 0` and non-integer values ⇒ `invalid_input`. A window running past the end is **clamped**, and an `offset` at or past the end is an **empty read** (`bytes: 0`), not an error — that is the "you are at the end" paging signal.
- **Byte semantics go past UTF-8**: a window can land mid-character. In text mode that is the same typed `io_error` as any other non-UTF-8 read, and the text names the way out (`mode: "base64"`). Base64 mode does not have the problem — that is what it is for.
- **The three arguments belong to `read`**: on `write`/`list`/`stat` they are `invalid_input` rather than silently ignored. A silently dropped `offset` on a `write` would let the caller believe in a partial write that never happened.
- **The default contract is untouched**: without `mode`, `offset` and `limit` the old path runs exactly as before, including the **absence** of the `encoding` header.

**Phase-7 conventions** (Slice-1 decisions):
- **`target = reply_to`**: FileCell emits to `msg.reply_to`; fallback `/colony/dead_letters` if `reply_to` is missing. Edges in the topology can override the target.
- **`tool_call.text` is JSON args**: `{"op": "read"|"write"|"list"|"stat", "path": "<rel>", "content"?: "<str for write>", "mode"?: "text"|"base64", "offset"?: <u64>, "limit"?: <u64 >= 1>}` (the last three for `read` only, GH #106).
- **`write` without auto-mkdir**: the parent dir MUST exist. Missing parent → `io_error`. Symlink-safe via parent canonicalize.
- **`write` error texts are a contract (GitHub #79)**: `io_error` is ambiguous on the write path, so `text` names the condition and the parent as the caller wrote it — `parent directory does not exist: notes (write does not create directories)`, `parent path is not a directory: notes`, `parent directory not accessible: notes (permission denied)`. Failures of the write stage itself (after the parent resolved) carry the prefix `write failed:` plus the named reason (`permission denied`, `read-only filesystem`, `no space left on device`, …). The `error_code` stays `io_error` in every case — the texts are the distinction, not the taxonomy.
- **Security boundary, two stages (GH #107)**: **lexical first, canonicalized second.** Stage 1 runs **before any filesystem access**: a plain component walk over the relative path (`.` is nothing, a name descends, `..` ascends) — climbing above the base is `path_outside_boundary`, **even if a later component would come back in** (`../<base-name>/x`); deciding that would mean resolving names outside the fence, which is exactly what is being closed. Stage 2 is unchanged: `canonicalize` resolves symlinks and the canonical path must live under `base_path` (on the write path, the canonical parent). The ordering is the point: `canonicalize` fails `not_found` on a missing target, so `../missing` used to report `not_found` while `../existing` reported `path_outside_boundary` — a (weak) **existence oracle** for the world outside. Now **every** escape attempt answers identically, whatever is or is not out there; on the read path **and** the write path (a missing parent outside the fence is an escape, not an `io_error`). `..` **inside** the boundary stays ordinary path arithmetic. Absolute paths remain `invalid_input`.
- **Default `max_concurrency`**: 8.
- **error_codes**: `invalid_input`, `path_outside_boundary`, `not_found`, `not_a_directory`, `not_a_file`, `io_error`.

---

## `edit`: file-editing operations

**Task**: edits files within a security boundary (typically: find/replace, insert-at-line, patch). Stateless.

**Emission mode**: atomic-emitting. Per edit operation one `tool_result` turn.

**Body format of the response**: `messages[]` with a `tool_result` turn. `text` contains the status of the edit operation (e.g. "3 occurrences replaced" or a diff snippet). On error (file not found, pattern does not match) the error is described structured in `text`; `header.error_code` marks the class.

**Output header**: `operation`, `matches_changed`, `bytes`, `duration_ms`, optional `error_code`.

**`params`**: `base_path` (mandatory; security boundary).

**Phase-7 conventions** (Slice-2 decisions):
- **Ops in Slice 2**: `find_replace` + `insert_at_line`. **Patch is deferred** (a separate diff-format design pass is needed).
- **`find_replace` = replace ALL**: all occurrences are replaced. The `matches_changed` header gives the count.
- **0 matches → `ERR_PATTERN_NOT_FOUND`**: the caller wanted to replace, the pattern was not there → error (no normal tool_result with `matches_changed: 0`).
- **`expected_matches` — the expectation guard (GH #105)**: optional argument of `find_replace`, an integer ≥ 1. When it is set and the actual match count differs, the file is **not touched**; the answer is `error_code: "unexpected_match_count"` and `text` names **both** numbers (expected and found). Reason: replace-ALL with an ambiguous pattern silently patches sites the caller never saw — the highest-risk failure mode while coding. With the guard the count becomes a **precondition** instead of an after-the-fact report. **Without** the argument the behaviour is unchanged (replace-ALL, `matches_changed` as a report) — `null` counts as "not set". `expected_matches: 0` ⇒ `invalid_input` (the guard counts sites that are **meant** to change, and none of them is not an edit intent), as does a non-integer value. **Precedence**: 0 matches stay `pattern_not_found`, guard or no guard — "your pattern is not in this file" is a different repair from "your pattern is not unique enough". On `insert_at_line` the argument ⇒ `invalid_input` (there is no match count there; silently ignoring it would fake a guard that never runs).
- **`insert_at_line` is 1-based and insert-BEFORE**: `line = 1` → at the very start; `line = file_lines + 1` → at the very end. `line < 1` or `line > file_lines + 1` → `invalid_input`.
- **`content` is normalized to a whole line (GH #108)**: when `content` does not end in `\n`, the cell appends exactly **one**. Before, `content` was spliced verbatim between the line slices and, lacking its own terminator, fused with the line it displaced (`"X"` at line 2 of `"a\nb\n"` produced `"a\nXb\n"`) — a silently broken file that only the next compile run reported. The operation is called `insert_at_line`, so the cell closes the line it is asked to insert. The alternative "just document it" was **rejected**: the failure is silent at edit time, and documentation prevents nothing silent. Two edges: **empty `content`** stays empty (it starts no line; a caller who wants a blank one writes `"\n"`), and the **FILE's own missing final newline** is left alone — appending to a file without a trailing `\n` still lands on its last line, because the opposite would rewrite a line the caller never named.
- **Shares FileCell's security boundary**: same `base_path` logic (extracted into `meclaw-cells/src/boundary.rs`) — including the two-stage fence from GH #107 (lexical pre-check ahead of the existence check, see § `file`).
- **Not atomic**: read-modify-write without tempfile+rename (consistent with FileCell::write). Crash mid-way = OS-level problem. Atomic edits are post-roadmap.
- **Concurrent edit on the same file**: race condition possible (no lock in Phase 7). The caller topology serializes if needed.
- **Input**:
  - `{"op": "find_replace", "path": "<rel>", "find": "<str>", "replace": "<str>", "expected_matches"?: <u64 ≥ 1>}`
  - `{"op": "insert_at_line", "path": "<rel>", "line": <u32>, "content": "<str>"}`
- **Default `max_concurrency`**: 8.
- **error_codes**: reuse from file + new `pattern_not_found` + `unexpected_match_count` (GH #105).

---

## `proxy`: external-chat-platform bridge

**Task**: long-running. Bridges to an external chat-platform provider. Since P12 there are **two platform variants behind `params.platform`** (optional, default `"telegram"`, so every config written before P12 keeps parsing to exactly the same result): `telegram` (Bot API over HTTP long poll) and `slack` (Socket Mode, WebSocket push). One instance bridges exactly one platform. Holds in `cell.db` a cursor for update offsets, so that restarts do not process messages twice.

**Concurrency setup**: **two Tokio tasks per instance** (handler + I/O), communicating over an internal mpsc (see `meclaw-overview.md`, section "Long-running cells: dual task"). From the topology's view the cell stays a single address with a single external mailbox; the dual structure is internal and prescribed for this cell type.

- **Handler task**: does `tokio::select!` over the external mailbox (inbound from topology) and the internal channel (provider events from the I/O task). Holds the entire cell state (cursor in `cell.db`, in-memory session maps). Sets order and state mutations alone, no Mutex.
- **I/O task**: polls Telegram (long-poll or webhook reader), serializes incoming user messages into event frames and pushes them into the internal mpsc. Holds no cell state, no direct `cell.db` access.

This way a 30s long-poll never blocks an inbound message from the topology and vice versa.

**Emission mode**: atomic-emitting (towards topology). One external chat message from the user → one emitted meclaw message with exactly one user-origin turn. The proxy is the **source** of the conversation thread, not mid-stream. It has no incoming `messages[]` to pass through.

**Body format of the outbound message** (Telegram → topology):
```json
{
  "messages": [
    { "origin": "user", "type": "text", "text": "<typed by the user>" }
  ]
}
```

Plus a header with platform metadata: `chat_id`, `user_id`, `platform: "telegram"`, optional `message_id` (platform-own ID, pass-through for later replies).

**Inbound behavior** (topology → Telegram): the proxy consumes incoming meclaw messages, extracts the last assistant turn from `messages[]` and sends its `text` to the chat platform. In doing so it emits **nothing** back into the topology, a pure sink. Routing to the right chat conversation runs via `chat_id` from the headers.

**Inbound error paths**: if the inbound body is not inline-readable (no inline UBF), the proxy emits `error_code: "invalid_body"`. If the `chat_id` header is missing, `error_code: "missing_chat_id"` (fallback `/colony/dead_letters`). If `messages[]` contains no sendable assistant turn, `error_code: "missing_assistant_turn"`. If the send to the chat platform fails (network error, Telegram API error, invalid `chat_id`), `error_code: "send_failed"`. All error replies go to `msg.reply_to` (fallback `/colony/dead_letters`) and carry a non-conversation origin (no `user`/`assistant` turn) and do not count as a conversation emission, the pure-sink discipline ("emits nothing into the conversation flow") remains preserved.

**`params`**: typically platform credentials (bot token via `${VAR}`) and polling configuration (long-poll interval, timeout). Optional `query_timeout_ms` (A-timeout for `cell.db` ops via `DbConn::call_with_timeout`, e.g. cursor persist).

**Runtime param updates (β, `config.md` § Access L.20):** like `llm` (see there): top-level `params` body slot, persisted in the `cell.db`, replayed on wake/respawn. **Mutable over all three propagation paths:** `send_timeout_ms` (path A, handle-side, the next `sendMessage` uses it), `long_poll_timeout_ms`/`long_poll_request_secs`/`base_url` (path B, the handler signals the I/O task via an internal reconfig channel, the next poll uses them; on a `base_url` change handler and I/O task rebuild their `TelegramClient` live (`with_base_url`) and **retain the immutable `bot_token` from the existing state**, the token never crosses the params surface; the W7 tripwire `long_poll_timeout_ms > long_poll_request_secs*1000` is re-enforced at merge), `query_timeout_ms` (path C, the running `DbConn`). **Immutable per `proxy`:** `bot_token` + `emit_to` (credential/routing identity). `base_url` is a config URL (like `llm.base_url`), **not** a credential → mutable. An update attempt on an immutable or an unknown key or a W7 violation ⇒ loud reject (`error_code: "invalid_input"`), no partial apply. A params-only message persists and stays silent.

**Slack variant (P12)**

A second platform of the same cell type, not a new cell type. Enabled via `params.platform: "slack"`. One instance == one Slack app == one bot token == one bot identity.

- **`params`**: `app_token` (app-level token `xapp-…`, for `apps.connections.open`) and `bot_token` (bot token `xoxb-…`, for `chat.postMessage`) are **required**, are supplied only as `${VAR}`, are **immutable**, and are **redacted** in `Debug`, in logs and in error messages: the `bot_token` IS the bot's identity in the workspace. Alongside them `emit_to` (required), `base_url` (default `https://slack.com/api`), the A-timeouts `connect_timeout_ms`/`send_timeout_ms`/`query_timeout_ms`, `idle_timeout_ms` (idle deadline of the socket mode read loop, default `120000` = four missed pings at Slack's slowest documented cadence; every frame resets the deadline and an elapsed one becomes `ConnectionEnd::Transient` feeding the reconnect machinery), `envelope_dedup_secs` (retention of the `seen_envelopes` dedup table) and `thread_follow` (default `true`). Optional `bot_user_id` (`U…`), used solely for the defensive self-filter R4.
- **Socket Mode instead of long poll**: the I/O task fetches a short-lived `wss://` URL via `apps.connections.open`, connects, and waits for the `hello` frame (it carries our own `app_id`, the input value for R3). After that the loop is **purely frame-driven**: no tick, no interval, no "check whether anything arrived" (standing rule NO POLLING). A reconnect happens exclusively on an event (`disconnect` frame, WS close, stream error); the backoff only damps the failure case before the next connection attempt and is never a query cycle.
- **Acknowledgement duty**: the ack goes out **immediately after the frame is decoded**, **before** any filter and before the handler ever sees the event. Silence is not the same as ignoring: an unacknowledged envelope gets redelivered by Slack. A crash between ack and persist loses at most one event, the same trade the Telegram path already made with state-before-emit. Against network-level redeliveries the handler deduplicates on the `envelope_id` (`cell.db` table `seen_envelopes`).
- **`chat_id` is a composite STRING**: `"C…"` for a DM, or `"C…:<thread_ts>"` inside a thread; a documented convention whose thread part is optional. Emit path and reply path share exactly one build-and-split function, so the two spellings cannot drift apart. Additionally in the `hop`: `platform: "slack"`, `slack_channel`, `slack_thread_ts`, `slack_event_ts`.
- **Addressing (D-5)**: a mention in the channel root **opens a thread at its own `ts`**, and every answer runs into the opened thread. A mention inside a thread stays in that thread, a DM stays threadless. A follow-up without a mention is processed only when the cell owns the thread (`thread_owner` table in `cell.db`, switch `thread_follow`). **A bot never answers inside a foreign thread**, not even when the ownership check fails on a database error.
- **Loop guard R1–R5**, on by default; R1–R4 stateless in the I/O task, R5 in the handler:

  | Rule | Condition | Effect |
  |---|---|---|
  | R1 | `event.bot_id` present | ignore |
  | R2 | `event.subtype == "bot_message"` | ignore |
  | R3 | `event.app_id` == our own `app_id` (from the `hello` frame) | ignore |
  | R4 | `event.user` == `params.bot_user_id` (when set) | ignore |
  | R5 | channel `message` without a mention and without thread ownership | ignore |

  Ignored events are acknowledged anyway.

  **Live lesson (2026-08-09, against the real Slack API):** `app_mention` is routed selectively to the mentioned app, whereas `message.channels` reaches **every** subscribed app. Every channel message therefore arrives at the bots twice or more, carrying the same `ts` but **different** `envelope_id`s, which is exactly why the envelope dedup does not catch it. The guard is thus a **correctness condition, not politeness**: without it a bot emits the same user message twice into the agent tree, and every bot answers the mentions addressed to all the others.

  **Bot detection runs on `bot_id`** (Slack sets it together with `bot_profile` on bot-authored messages), **not on `subtype`**: `subtype: "bot_message"` is classic-app behaviour, unreliable on its own, and stays only a second line of defence. R3 reads `event.app_id`, the **sending** app, never `payload.api_app_id`, which names the **receiving** app, equals our own on every inbound event, and would discard all traffic.
- **Beta asymmetry towards Telegram, named honestly:** the Slack variant accepts **no** runtime param updates today. It builds exclusively from the birth params; there is neither an overlay restore nor a `params` update path in the handler. Recorded as a defer in `roadmap.md` ("β params overlay for the Slack variant").

---

## `timer`: periodic event emitter

**Task**: long-running. Cron-like scheduling cell. Holds the active schedule list in `cell.db`. **The cron format is 6-field Quartz style** (`Second Minute Hour DayOfMonth Month DayOfWeek`), so that second granularity is natively expressible. The scheduler resolution is correspondingly second-accurate. Firing happens exactly at the configured second, no polling grid. Can send one-off as well as repeating events.

**Concurrency setup**: **two Tokio tasks per instance** (handler + I/O), communicating over an internal mpsc (see `meclaw-overview.md`, section "Long-running cells: dual task"). Prescribed for this cell type, not optional.

- **Handler task**: does `tokio::select!` over the external mailbox (schedule creation/modification/deletion) and the internal channel (timer firings from the I/O task). Holds the in-memory schedule list and persists it to `cell.db`. Sets order and state mutations alone.
- **I/O task**: computes the next-due schedule entry, waits for it with `tokio::time::sleep_until`, pushes a firing event frame into the internal mpsc, computes the next wait point. On schedule changes (add/modify/remove) the handler task sends a reconfigure hint to the I/O task, which redoes its sleep computation. Holds no cell state, no direct `cell.db` access.

This way the timer is second-accurate, without the mailbox processing being able to disturb the sleep timing and vice versa.

**Emission mode**: atomic-emitting. The timer **produces no content of its own**. It sends what was passed along as the body template at schedule creation, at the configured time.

**Schedule identity**: each schedule has a **`schedule_id` (UUID v7) as a unique
key**, assigned by the caller in the creation message (or in the `params.schedules`
entry at instantiation). `schedule_name`, by contrast, is a **non-unique
human-readable label** (may occur multiple times) and serves only readability + the
fire header. Modification and deletion always address **via `schedule_id`**, never via
`schedule_name`.

**Where the op lives — two admitted places (GitHub #81)**: either as **top-level slots of the body**
(the form for config-born ops, for the HTTP ingress, and for any cell feeding the timer directly) or
as **structured JSON args in the `tool_call` turn** (analogous to `store` and `bash`, see there). If
the body carries a `tool_call` turn, that turn wins — its own parse errors are reported rather than
falling back to the top level and pointing at the wrong level of the message. This makes the timer
**usable as a tool lane without a bridge cell**: the dispatcher unwraps `{name, arguments}` into a
`tool_call` turn, the timer reads the args like every other tool cell, and answers with a
`tool_result` on the same `id` (see "Ack" below).

**Operation per message** via the mandatory field `op: "add" | "modify" | "remove" | "trigger"`
(default `add`, if omitted):

```json
{
  "op":            "add",
  "schedule_id":   "0190a3f2-...-v7",
  "schedule_name": "daily-standup",
  "cron":          "0 0 9 * * *",
  "emit_to":       "/main/standup_hive",
  "emit_body":     { "messages": [{ "origin": "user", "type": "text", "text": "..." }] },
  "emit_headers":  { "msg_type": "standup_trigger" }
}
```

```json
{ "op": "remove", "schedule_id": "0190a3f2-...-v7" }
```

```json
{ "op": "trigger", "schedule_id": "0190a3f2-...-v7" }
```

`modify` carries `schedule_id` plus the fields to change (e.g. a new `cron`).

**Semantics** (strict, no heuristic):
- `add` = INSERT; an existing `schedule_id` → error (no implicit upsert).
- `modify` = UPDATE of the carried fields; an unknown `schedule_id` → error.
- `remove` = deactivate the schedule (status update in `cell.db`, **No-Delete-conformant**, no
  row deletion); an unknown `schedule_id` → error.
- `trigger` = fire an existing schedule **once, now**, without changing its plan (GitHub #17).
  The op carries nothing but the `schedule_id`; everything else, `emit_to`, `emit_body`,
  `emit_headers`, comes from the row, because the schedule already IS the description of what
  is to be fired. An unknown or non-active (`removed`/`completed`) `schedule_id` → error.

**What `trigger` delivers, the firing itself rather than a similar one**: the handler checks
only existence and status and hands the firing to the I/O task, which pushes the same fire frame
the `sleep_until` arm pushes. Everything after that is identical: the same race check, the same
state-before-emit (`iteration_n` bump resp. `mark_completed`), the same `OriginSink` emission
with the full auto-header set. A triggered repeating schedule counts its `iteration_n` on as
usual and keeps its next cron occurrence; a triggered one-off counts as `completed` afterwards
and no longer fires at its own `at` (race check in `handle_event`). The op itself writes nothing
to the schedule and emits nothing, which is why a triggered run is indistinguishable from a
cron-fired one.

**Validation & error surfacing**: on `add`/`modify` a `cron` expression is validated against the 6-field Quartz parser. Invalid expressions are rejected (no silently stored, never-firing schedule arises). All op errors are emitted as a message to the `reply_to` of the op message (`parent_message_id` = the consumed op message), with `header.error_code` for: `invalid_body` (body not inline-readable), `parse_error` (op message unparsable beyond the cron check), `schedule_id_exists` (add on an existing `schedule_id`), `schedule_not_found` (modify/remove/trigger on an unknown `schedule_id`, and trigger on a non-active one), `kind_mismatch` (modify type switch once↔repeating), `invalid_cron` (invalid cron expression).

Two `parse_error` forms are deliberately separated (GitHub #81): **"no op object at the body top level"** (the body carries only carrier slots such as `messages` — the message names them and the two admitted places) and **"the op object is there, its `schedule_id` is not"**. The first is the answer to a message in which the op never arrived; calling that a missing `schedule_id` points at the wrong field.

**Ack (GitHub #81)**: if the op arrived as a `tool_call` turn, `add`/`modify`/`remove`/`trigger` answer on success with a message to `reply_to` (fallback `msg.target`) carrying exactly one `tool_result` turn with the **inbound `tool_call` `id`**; `header` carries `msg_type: "timer_op_ack"`, `op` and `schedule_id`. Errors on the same lane carry the same turn plus `finish_reason: "error"` — a tool loop closes on the failure path too, instead of waiting for a result that never comes. **Without** an inbound `tool_call` `id` nothing changes: successful ops on the raw-body path stay unacked, and its errors keep their shape (`messages: []` + `meta.detail`). The firing itself is untouched by this — it goes via `OriginSink` to `emit_to`, not to the caller. Runnable example: `tests/fixtures/gh81-remind-lane/` (dispatcher → timer → ack/fire lanes, pinned in `crates/meclaw-cli/tests/gh81_remind_lane_e2e.rs`).

**Op messages over the HTTP API**: the op body is an ordinary UBF body, the op fields being
cell-specific top-level slots. Whoever feeds an op in through `POST /messages` additionally
declares the central slot the message honestly has: an op message carries no conversation turns,
hence `"messages": []`. Without a central slot the ingress validation rejects it with
`422 invalid_ubf_body` (overview § Schema validation, edge). Example:

```json
{ "target": "/main/nightly",
  "body": { "messages": [], "op": "trigger", "schedule_id": "0190a3f2-...-v7" } }
```

The op stays colony-validated throughout: the HTTP layer checks the envelope, the cell checks
the op (`schedule_not_found`, `invalid_cron`, …). A scheduled lane is therefore triggerable once
from outside, without restarting the colony and without writing past the timer's `cell.db`
(GitHub #17).

**One-off vs. repeating**: a repeating schedule carries `cron` (6-field Quartz). A one-off one carries `at` instead (RFC-3339-Z, UTC) and **no** `cron`. The fields are exclusive (exactly one per schedule). `iteration_n` is emitted only on repeating schedules (omitted on once). `modify` may not switch the type (once↔repeating), for that `remove` + `add`.

```json
{ "op": "add", "schedule_id": "0190a3f2-...-v7", "schedule_name": "one-shot-reminder", "at": "2026-06-01T09:00:00Z", "emit_to": "/main/x", "emit_body": { "messages": [] } }
```

**Past firings are discarded** (POC behavior): the timer plans exclusively the
next firing *after now* (`find_next_occurrence`). A one-off schedule whose time
already lies in the past (at creation or restart time) is not scheduled and
only logged. Repeating schedules do not catch up missed firings. They fire from the
next future occurrence. Rationale: the timer has no relevance/priority
classification and cannot decide whether a missed event is still to be delivered.

The body can contain arbitrary universal body slots: `messages[]`, own top-level slots, or also empty (header trigger only).

**Headers emitted on schedule firing** (timer-automatic, in addition to `emit_headers`):

| Header | Content |
|---|---|
| `event_id` | UUID v7 of this single event |
| `schedule_id` | unique UUID-v7 key of the triggering schedule |
| `schedule_name` | human-readable label of the schedule |
| `scheduled_at` | planned time (RFC-3339-Z, UTC) |
| `fired_at` | actual fire time (RFC-3339-Z, UTC) |
| `iteration_n` | on repeating schedules: 0, 1, 2, … |

**Contract quirk**: `emits.body` is wildcard-like (what the schedule defines), `emits.header` is strictly the fixed set above (plus what the schedule passes under `emit_headers`).

**`params`**: typically none. Schedules are created at runtime per message (or optionally initially via `params.schedules`). `params.schedules` entries carry the same schema (each with `schedule_id` as UUID v7), and the initial seed takes effect only on a fresh `cell.db` (`OpenStatus::Created` gate, analogous to the Phase-9 `store` seed). Otherwise each restart re-seeds the config schedules into duplicates. Optional `query_timeout_ms` (default 5000) sets the A-timeout for `cell.db` accesses (rusqlite `InterruptHandle` via `DbConn`). It applies to **all** cell.db ops of the cell (`add`/`modify`/`remove` + the fire-side reads/writes) that run via `DbConn::call_with_timeout`.

**Runtime param updates (β, `config.md` § Access L.20):** like `llm` (see there): top-level `params` body slot, persisted in the `cell.db`, replayed on wake/respawn. The **only** overlay-capable field is `query_timeout_ms`. It takes effect **immediately live** (the running `DbConn` adopts the new A-timeout for the next cell.db op, without wake/respawn). `schedules` are **not** overlay-capable: they change exclusively via the `add`/`modify`/`remove` ops (they carry live state `status`/`iteration_n` in the `cell.db`). The immutable set is **empty**; an update on `schedules` or an unknown key ⇒ loud reject (`error_code: "invalid_input"`). A params-only message persists and stays silent.

---

## `mcp`: MCP-platform bridge

**Task**: long-running. Bridges to an external MCP provider (Model Context Protocol). Holds in `cell.db` states as applicable (e.g. tool-discovery cache, session handles). **Two transports**: `http` (HTTP + JSON-RPC, a fresh connect per call; `initialize` / `tools/list` / `tools/call`) and `stdio` (a child process, line JSON over stdin/stdout, since 0.1.7). `params.transport` is optional and defaults to `http`. Server-pushed notifications, SSE and auto-reconnect remain a roadmap defer.

**Concurrency setup**: **two Tokio tasks per instance** (handler + I/O), communicating over an internal mpsc (see `meclaw-overview.md`, section "Long-running cells: dual task"). Prescribed for this cell type, not optional.

- **Handler task**: does `tokio::select!` over the external mailbox (tool-call requests from the topology, discovery requests) and the internal channel (server-pushed events or tool responses from the I/O task). Holds the entire cell state (discovery cache, session handles, in-flight map of correlated tool calls).
- **I/O task**: talks to the MCP provider — on `http` over HTTP + JSON-RPC (no persistent stream); on `stdio` it **owns the child process entirely** and holds the long-running stream read, the handler holds no pipe and talks to it over the internal reconfig channel. Serializes responses into event frames and pushes them into the internal mpsc. Holds no cell state, no direct `cell.db` access.

This way a long-running provider call never blocks the acceptance of new tool-call requests from the topology.

**Sandboxing does not cover this cell type today.** The default-deny cut of GH #85 (`config.md` § `params`) reaches exactly three types — `bash`, `code` and `harness` — and `params.sandbox` is ignored everywhere else. The child process this cell spawns therefore runs with the **full rights of the colony daemon**: the daemon's environment, the daemon's filesystem view, the daemon's network. That is the state as built, tracked in [#96](https://github.com/mmeyerlein/meclaw/issues/96), and it is why an untrusted backend belongs on a machine you do not mind. Bringing the cut here later will be **opt-in per template**, not a silent tightening: a topology that works today would otherwise stop working on an upgrade, which is exactly the kind of break this project does not do quietly.

**Post-init backend death**: on `http` the cell holds **no** persistent connection. **Every** tool call connects anew. If the MCP backend dies transiently *after* the discovery, the cell therefore recovers **automatically on the next tool call** (the fresh connect succeeds again); a permanently dead backend manifests per call as `provider_timeout` or `mcp_error`. A death detection *between* calls does not exist on `http` (`run_http_io` pends after the discovery; a roadmap defer, trigger = SSE build-out). On `stdio` the stream read carries the liveness signal: on EOF or exit an open call first receives a regular error message (`mcp_error`, the detail names exit code/signal/EOF), then the cell panics → `one_for_one` with a fresh child process; after `restart_limit` the registry entry is retained as `failed`. No new `error_code`.

**Emission mode**: atomic-emitting. Per MCP tool call one response message with the result as a turn.

**Body format of the response**: `messages[]` with a `tool_result` turn, `text` contains the MCP tool answer (typically JSON-structured). On large answers (from Phase 12) whole-body offload of the entire message as `Body::Blob` at the delivery boundary, **not** via an in-message `text_id` pointer.

**Discovery**: MCP tools that this provider offers are made available via a discovery message. The cell can play out its `system.tools.*` slots to an `llm` cell, so that the latter presents the tools to the LLM. The exact mechanism is a Phase-10 detail.

**Output header**: `mcp_tool` (name of the called tool), `duration_ms`, optional `error_code`. Canonical `mcp` `error_code` values: `"mcp_error"` (JSON-RPC/protocol error of the provider, e.g. `tools/call` error response) and `"provider_timeout"` (`external_timeout_ms` elapsed at the provider call, both transports).

**`params`**: `transport` (optional, `"http"` default | `"stdio"`); on `http` the provider `endpoint` (HTTP URL for JSON-RPC) + auth credentials (via `${VAR}`); on `stdio` `command` (required), optional `args`, `env`, `cwd`, `kill_grace_ms` (default 2000). `endpoint` and `command` at the same time ⇒ loud reject. Plus discovery configuration, optional `external_timeout_ms` (A-timeout, `error_code: "provider_timeout"`) as well as `query_timeout_ms` (A-timeout for `cell.db` ops via `DbConn::call_with_timeout`).

**Runtime param updates (β, `config.md` § Access L.20):** like `llm` (see there): top-level `params` body slot, persisted in the `cell.db`, replayed on wake/respawn. **Mutable:** `external_timeout_ms`: takes effect **immediately live** (path A, the next `call_tool` uses it; the I/O task has post-discovery **no** live-re-readable value, hence purely handle-side), and `query_timeout_ms` (path C, the running `DbConn` adopts the new A-timeout for the next cell.db op). **Immutable per `mcp`:** `endpoint` + `auth` (bearer), credential/identity, as well as `transport`, `command`, `args`, `env`, `cwd`, `kill_grace_ms` (process identity of the child). An update attempt on it or an unknown key ⇒ loud reject (`error_code: "invalid_input"`), no partial apply. A params-only message persists and stays silent.

---

## `harness`: agent harness as a supervised child process

**Task**: long-running. Operates a full agent harness (today: Claude Code in print mode) as a supervised child process out of the topology — the harness pre-prompts, loops and uses its own tools; that is exactly the point (delegating whole coding tasks instead of single model calls). One child process **per task**: session continuity comes from the harness's `--resume`, not from process lifetime.

**Concurrency setup**: **two Tokio tasks per instance** (handler + I/O), prescribed — see `meclaw-overview.md`, section "Long-running cells: dual task". The I/O task owns the child process entirely (on the `stdio_child` core, like `mcp` stdio); the handler owns the task register and the emissions. Exactly **one** task at a time per cell — parallelism is a topology matter (multiple cells, each with its own worktree).

**Task lifecycle**: `Booted` → idle → `start_task` → spawn → frame stream → child end → idle. An ending child process is the **normal case** here and does **not** panic the cell — the counter-semantics to `mcp` stdio, where child death ends the cell.

**Non-idempotency (core invariant)**: the `cell.db` table `harness_tasks` is a tombstone register. The row is `running` **before** the spawn; after a supervisor restart every unfinished row is set to `unknown` and reported exactly once as `unknown_outcome` ("inspect worktree") — **never** restarted. A `task_id` runs exactly once (dedup); `task_id` is therefore a required input, not a generated fallback.

**Emission mode**: long-running, stateful.

**Body format of the emissions**: five forms — `accepted` (synchronous as a `tool_result` in the trace of the triggering message; carries the `task_id` as the anchor) and `progress` / `question` / `result` / `error` (origin emissions to `params.emit_to`, each with a fresh trace; correlation via `header.task_id`).

**Output header**: `harness_event`, `task_id`, `session_id`, `status`, `workspace`, `duration_ms`, `num_turns`, `cost_usd`, `model`, `phase`, `tool_name`, `request_id`, `error_code`. **The header carries only what was observed** — no `branch`/`commit`: the harness's self-report stays prose in the turn; verification (tests, diff inspection) is a follow-up step of the topology.

**Failure classification** (`error_code`, closed): `invalid_input`, `harness_busy`, `workspace_invalid`, `spawn_failed`, `startup_timeout`, `harness_crashed`, `cancelled`, `unknown_outcome`, `query_timeout`.

**Precedence when two of them apply** (pinned, W13): a `start_task` is checked in the order **occupancy → workspace → tombstone**, and the first refusal is the one reported. The case that makes this visible is a repeated `task_id` arriving while a task runs: both `harness_busy` and the dedup rejection hold, and **`harness_busy` wins**. That order is not incidental — the dedup verdict comes out of the tombstone INSERT, which is the same statement that claims the slot, so deciding dedup first would mean an extra read on every start to change nothing but a label. The same payload answers `invalid_input` once the harness is free again. Pin: `busy_beats_dedup_when_both_apply`.

**Cancel**: a `cancel` message (with `task_id`) sets the tombstone to `cancelled` **before** the child is stopped (process-group kill including grandchildren) and emits the task end marked `cancelled`; the cell accepts new tasks afterwards. Cancel is the stop lever for the deliberately unbounded task runtime (see overview § Timeouts).

**`params`**: `adapter` (required; today only `"claude-code"`), `emit_to` (required), `workspace_root` (required, canonicalized — tasks run only below it), `command`, `model` (from `${VAR}`), `permission_mode`, `max_turns`, `max_budget_usd`, `allowed_tools`, `extra_args`, `env`, `env_passthrough`, `approval` (`off` | `channel`), `startup_timeout_ms`, `external_timeout_ms`, `query_timeout_ms`, `kill_grace_ms`.

**Sandbox (GH #85)**: `harness` reads the same `params.sandbox` block as `bash` and `code` (schema in `config.md` § `params`) and hands it to the stdio child process. It sits **next to** `env_clear`/`env_passthrough` and the canonicalized `cwd` clamp, not in their place: the three answer different questions. Process-group and reaping semantics are unchanged, and a sandboxed child still leads its own group. `sandbox` is on the runtime overlay's **immutable** list: a params update touching it is rejected as `Immutable`. A `harness` cell instantiated from a template without a block of its own gets the default-deny profile, so a harness that is supposed to write a workspace declares one (or takes an explicit `trust: "trusted"`).

**Trust model (empirically established 2026-08-09, the state before GH #85)**: `harness` is **not a sandbox by itself**. The harness brings its own tools (shell, file access, network) and runs with the rights of the colony process. The load-bearing V1 barriers are **`env_clear` + `env_passthrough`** (the harness does not see the colony's secrets) and the **canonicalized cwd clamp** under `workspace_root`. **`allowed_tools` is explicitly NOT an upper bound**: the CLI treats `--allowedTools` **additively** to what the permission mode allows anyway — in the acceptance smoke, `Bash` ran despite `allowed_tools: ["Write"]`. `allowed_tools` extends, it does not restrict. Since GH #85: **without** `params.sandbox` a hand-written `harness` cell still runs exactly like that, and **with** the block it runs under the same boundary as `code` and `bash`.

**Runtime param updates (β)**: mutable `model`, `max_turns`, `max_budget_usd`, `startup_timeout_ms`, `external_timeout_ms`, `query_timeout_ms` — take effect from the **next** task. Immutable and a loud reject (`invalid_input`): `adapter`, `command`, `emit_to`, `workspace_root`, `env`, `env_passthrough`, `permission_mode`, `allowed_tools`, `extra_args`, `approval`, `kill_grace_ms` — that is the containment boundary. A params-only message persists and stays silent.
---

## `subcolony`: a child colony as one cell

**Task**: long-running. Operates a complete child colony as **one** cell in the parent graph. The child is a real `meclaw` binary with its own `{root}`, its own `colony.json`, its own `colony.db` and its own cell tree, supervised as a child process on the stdin/stdout bridge in JSON mode (`--stdio-format json`, wire v1, see `meclaw-overview.md` § Stdin/stdout bridge). **No in-process nesting**: a colony stays one process with one tree; nesting happens across the process boundary. The facade is therefore an **opaque composition boundary**: from the outside the child colony is exactly one addressable cell.

**Concurrency setup**: **two Tokio tasks per instance** (handler + I/O), prescribed, see `meclaw-overview.md`, section "Long-running cells: dual task". The I/O task owns the child process entirely (on the `stdio_child` core, like `mcp` stdio and `harness`); the handler owns the request path and the emissions. The cell holds **no** lock and **no** shared state: the pending requests live in the serve loop, the cell state in the handler task.

**Boot handshake**: spawn (`--root <root> --stdio-format json`, `env_clear`, own process group; `--daemon` and `--api` are **never** set, because stdin EOF must end the child) → the child writes exactly one `ready` frame, **after** its bootstrap succeeded and **before** it reads stdin. `boot_timeout_ms` clamps the A-timeout on that frame. **`v` is the protocol integer and is asserted strictly; `version` is the child's release version and is reported only. Version skew between parent and child is the feature, not the fault.**

**Boot failures and restart cost**: deterministic boot failures (foreign protocol, absent `ready`, spawn failure) do not panic: the cell stays up and rejects every request loudly with the reason — the restart budget is not burned on a certainty. Only transient child death goes into `one_for_one`. One restart cycle costs a full child-colony boot; `boot_timeout_ms` is the upper bound of that cost and therefore the quantity to reckon with in the context of `cell.restart_limit`.

**No automatic re-fire path**: in-flight requests fail loudly with `subcolony_gone` when the child dies; a retry is the requester's decision, never the substrate's. A request is explicitly **not** free to repeat — it may already have triggered store writes inside the child.

**Sandboxing does not cover this cell type today.** The default-deny cut of GH #85 (`config.md` § `params`) reaches exactly three types — `bash`, `code` and `harness` — and `params.sandbox` is ignored everywhere else. The child process this cell spawns therefore runs with the **full rights of the colony daemon**: the daemon's environment, the daemon's filesystem view, the daemon's network. That is the state as built, tracked in [#96](https://github.com/mmeyerlein/meclaw/issues/96), and it is why an untrusted backend belongs on a machine you do not mind. Bringing the cut here later will be **opt-in per template**, not a silent tightening: a topology that works today would otherwise stop working on an upgrade, which is exactly the kind of break this project does not do quietly.

**Consume**: any UBF body with `messages[]`. **No `tool_call` wrapper**: a sub-colony is an ordinary cell in the flow (llm-shaped), not a tool cell. That is the operational meaning of "behaves like ONE cell".

**Emit, three forms**:

| Form | Lane | Target | Body |
|---|---|---|---|
| `reply` | `OutputSink` (requester's trace) | `msg.reply_to ?? msg.target` | `{"header":{"subcolony_event":"reply"},"messages":[…from the child…]}` |
| `error` | `OutputSink` (requester's trace) | `msg.reply_to ?? msg.target` | `{"header":{"subcolony_event":"error","error_code":…},"messages":[{"origin":"assistant","type":"text","text":<detail>}]}` |
| `unsolicited` | `OriginSink` (fresh trace) | `params.emit_to` (only when set) | `{"header":{"subcolony_event":"unsolicited"},"messages":[…from the child…]}` |

Body discipline as with `harness`: everything structural goes into the `header` slot, the turn carries text only.

**Headers across the process boundary**: only the body crosses the process boundary — `hop` never crosses, in either direction. The `header` slot of a child emission is lifted into the `hop` compartment inside the child already and is consumed there. A parent edge therefore conditions on `hop.subcolony_event`, which the facade sets itself; a child that wants to signal more says it in the body.

**Failure classification** (`error_code`, closed): `subcolony_unavailable` (spawn or boot failed), `protocol_mismatch` (foreign protocol integer in the `ready` frame), `boot_timeout`, `request_timeout`, `subcolony_gone` (child died during the request or is shutting down), `ttl_exhausted`, `invalid_input` (body without `messages[]`), `child_error` (an `error` frame from the child).

**Trace and TTL**: `trace_id` is **carried** across the boundary, not regenerated — a trace runs through the child colony and stays correlatable. `ttl` is **decremented** on the crossing (`ttl - 1`); at `ttl == 0` there is **no** crossing but an `error` emission `ttl_exhausted`. TTL is thus the recursion budget of the composition: a child that calls the parent facade back dies like any other routing loop. The correlation key of request and reply is a **freshly generated per request** `context.turn_id` (the parent message's own would not be unique under fan-out); `turn_id` is therefore a reserved target key in the `context_in` mapping and is rejected loudly at params parse time.

**Opacity**: the child tree is **not addressable** from the outside — there is no path reach-through to a cell inside the child, and the `context` of the child's reply stays in the child (the reply travels in the parent requester's trace). Mutations of the child tree run exclusively over the **child's own operator surface** (its `/colony/mutations`), never over the parent mutation path. This is **composition, not federation**.

**Contract drift (operator responsibility)**: the facade's contract lives in the **parent `config.json`** (`consumes`/`emits` as with every cell type). The boot handshake asserts only what it can assert cheaply: the protocol integer and the existence of the `ready` frame. Whether the child's reality matches the parent's declaration is **operator responsibility** and is not checked by the substrate; a child-published port manifest is a roadmap defer.

**`params`**:

| Key | Type | Default | Mutable (β) | Meaning |
|---|---|---|---|---|
| `root` | string | **required** | no | Filesystem root of the child colony. Canonicalized at parse time (existence + `is_dir`), like `harness.workspace_root` |
| `command` | string | `"meclaw"` | no | The child binary. Explicitly configurable ⇒ version skew is a config decision |
| `env` | object | `{}` | no | Explicit environment of the child |
| `env_passthrough` | array | `["PATH","HOME","USER","LANG","TERM"]` | no | Survives `env_clear: true`, the secret isolation of the child colony |
| `context_in` | object | `{}` | no | **Explicit mapping** parent `context` key → child `context` key. Default: nothing crosses the boundary. `turn_id` as a target ⇒ loud reject |
| `emit_to` | string | — | no | Optional origin lane for uncorrelated child egress frames |
| `boot_timeout_ms` | u64 | `30000` | yes | A-timeout on the `ready` frame |
| `request_timeout_ms` | u64 | `120000` | yes | A-timeout on the correlated reply. Generous: a child colony may contain an `llm` cell |
| `external_timeout_ms` | u64 | `30000` | yes | A-timeout around every stdin write |
| `query_timeout_ms` | u64 | `5000` | yes | A-timeout for `cell.db` ops |
| `kill_grace_ms` | u64 | `5000` | no | SIGTERM→SIGKILL grace of the child process group |

The **immutability boundary is the containment boundary** (the same line as with `harness`): `root`, `command`, `env`, `env_passthrough`, `context_in`, `emit_to`, `kill_grace_ms` are immutable; an update attempt on them or an unknown key ⇒ loud reject (`error_code: "invalid_input"`), no partial apply. A params-only message persists and stays silent.

**Rule 12 (timeouts)**: `cell.message_timeout` (concept B, the substrate backstop) must sit **clearly above** `request_timeout_ms` (concept A), otherwise the backstop fires before the facade can report its typed `request_timeout`. A convention as with all cell types, not enforced in code; see `meclaw-overview.md` § Timeouts.

**Emission mode**: long-running, stateful, atomic-emitting.

---

## `vault`: a sealed secret store with no read

**The promise is the type, not a policy.** This cell type's route surface contains no `get`. It contains `put`, `rotate`, `use`, `revoke`, `status`, `unlock`, `lock` — and nothing else. A fully compromised model at the other end of an edge can ask the vault to **use** a secret inside a granted scope; it cannot ask to see one, because the question has no name here.

The difference from "a store plus a rule" is the point: a rule is an argument that can be won — by a prompt, by a bug, by a future exception "just for the migration". A missing operation is not.

**Two callers, and they may do different things:**

| caller | recognised by | may |
|---|---|---|
| **user channel** | `reply_to == None` — a source message. No edge can produce one, because the colony stamps `reply_to` on everything a cell emits | `put`, `rotate`, `unlock`, `lock`, `status`, `revoke` |
| **broker** | `reply_to == params.broker` | `use`, `status`, `revoke` |
| everyone else | — | nothing. Refused **before** the operation is looked at, with an audit row |

That the broker may **not** `put` is deliberate: otherwise an agent that had captured the broker could swap the vault's contents for its own.

**The grant check belongs to the broker, not to the vault.** A cell cannot query another cell inside one `handle()` — that is the actor model, not a limitation of this type. So the work is split where it can be done: the broker validates the grant against the grants store on its own lane, and the vault does the one thing only it can do — check who is talking — and records the `grant_id` it was handed.

**Injection at unlock.** `use` signs (HMAC-SHA256): the secret does work and stays home, the ssh-agent shape. The one case that genuinely needs the value — a connector authenticating to a platform — is served by `params.inject_map`, which names which secret reaches which cell under which param key. The delivery happens **once at unlock**, never per request, and never to an address a message could choose: the map is configuration and the body is not read. The requester learns which name went where, never the value.

**Unlock attestation.** Before accepting key material the vault verifies its **own inbound edges** against `params.broker` + `params.sealed_neighbors`. If anything else is wired to it, it stays locked and names the path. The reason: the port boundary applies to *mutations*, and the birth topology is deliberately exempt (author sovereignty). A `code` cell has filesystem access, so it can rewrite the tree on disk and let the **next boot** draw an edge no mutation would have been allowed to add — the gate laundered through a reboot. It still can; it simply never gets the key. An unverifiable neighbourhood fails closed exactly like a wrong one.

This is the **one** place where a cell looks at the topology, and it is a deliberate, narrow exception to "cells know no topology": read-only, only the edges into its own path, and only ever to refuse.

**A woken vault is always locked.** The key lives in the task and dies with it. A vault that could resume its unlocked state across a sleep would have to keep the key somewhere that survives the sleep, and no such place exists that is not a worse version of the problem the vault solves.

**Crypto:** argon2id from the passphrase against a per-store salt; XChaCha20-Poly1305 per secret with its own 24-byte nonce.

**No-delete:** a `put` onto an existing name **is** a rotation (a new version); `revoke` flips a status. Yesterday's ciphertext stays on disk — that is what makes a revocation auditable rather than a hole. `revoke` deliberately needs no passphrase: being locked out must never stop you disabling a credential that leaked.

**`params`**:

| key | type | default | meaning |
|---|---|---|---|
| `broker` | string | **required** | The one sender the vault answers at all. Absolute (`/main/access/invoke`) or hive-relative (`./invoke`, resolved against its own path — which is what makes it a template) |
| `key_source` | string | `"auto"` | `auto` \| `prompt` \| `systemd-cred` \| `plainfile`. Names a **source**, never material |
| `credential_name` | string | `"vault_key"` | file under `$CREDENTIALS_DIRECTORY` for `systemd-cred` |
| `key_file` | string | — | required for `plainfile`. Refused if group or others can read it — the same answer ssh gives for a loose private key |
| `unlock_ttl_ms` | u64 | — | re-lock after this long |
| `sealed_neighbors` | array | `[]` | further expected edge neighbours for the attestation |
| `inject_map` | object | `{}` | `{name: {to, key}}` — which secret is handed to which cell at unlock |
| `external_timeout_ms` | u64 | `5000` | A-timeout around reading key material (rule 12) |

**Storage** (its own `cell.db`): `vault_meta` (the salt — not secret; it exists so two vaults with one passphrase do not share a key), `vault_secrets` (one row per `(name, version)`), `vault_audit` (every operation, refusals included).

**Honest limit:** a determined `code` cell in the same process can read the vault's memory while it is unlocked. The designed answer is **placement** (own process, own user) — a deployment property that changes no edge. An agent that develops the substrate itself is out of scope by definition; no vault holds against that, and claiming otherwise would be the more dangerous statement.

**Emission mode**: stateful (lazy), atomic-emitting. One `tool_result` turn per message, plus the injections at unlock.

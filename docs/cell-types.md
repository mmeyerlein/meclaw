# Cell types

Detailed spec of the built-in cell types. On conflict between this file and `meclaw-overview.md`, the overview wins. It is the single source of truth.

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

**`params`**: **exclusively `graph`** (the `HiveParams` deserializer is `deny_unknown_fields`; any other key is a boot error):
- `graph` (optional): initial desired graph for the subtree (format see `meclaw-overview.md` section "Graph schema"). Colony reads this at filesystem bootstrap and enters the declared cells into the registry and the edges into `colony.db`. After the first bootstrap, the persisted edge table in `colony.db` is the truth. `params.graph` is only an initial hint.

No scope-owned `dead_letters` override: the dead-letter queue is always `/colony/dead_letters` (hive = authority and mutation boundary, **not** DLQ boundary). Otherwise no hive-type-owned fields. In particular no routing configuration, no mailbox size, no own emission-mode statement. Hives have no actor and no mailbox; their routing role is passive transit evaluation by colony over the `params.graph` edges.

---

## `store`: typed persistent storage

**Task**: CRUD cell with its own `cell.db`. Schema and column types can be defined in `params.schema`; the cell creates the tables from it. Dynamically it can also create a new table per message. Table and column names pass a syntax gate (P3, 2026-08-08): `[A-Za-z_][A-Za-z0-9_]{0,62}`, no `sqlite_` prefix, no `_fts` suffix. The only strings ever formatted into SQL are what the SQLite catalog (`sqlite_master`/`pragma_table_info`) itself returned or values from an internal enum — caller text reaches statements exclusively as bind parameters.

**Emission mode**: atomic-emitting. Per query message one response message with the result as a turn.

**Input format** (Phase-9 brainstorm E7, analogous to `bash`): structured JSON args in the `tool_call` turn. Mandatory field `operation` (`"insert"`/`"select"`/`"update"`/`"delete"`/`"create_table"`/`"search"`/`"traverse"`/`"similar"`) + `table`, plus operation-specific fields:

- `insert`: `row` (object `{ "<column>": <value> }`).
- `select`: `columns` (**mandatory**, array of column names with at least one entry; the projection) + optional `where`, `order_by` (array of `{ "col": "<column>", "dir": "asc"|"desc" }`, multi-column) and `limit` (integer ≥ 1, **no** implicit default, no cap — the runaway guard is `query_timeout_ms`). There is **no** projectionless `SELECT *`: if `columns` is missing or empty, the cell answers with `finish_reason: "error"` and `error_code: "invalid_input"` (no cell crash; doc-to-code correction, ruling 2026-08-08). The result is an array of row objects, projected onto the requested columns.
- `update`: `set` (object) + optional `where`.
- `delete`: optional `where`.
- `create_table`: `columns` as a **2-level map** `{ "<column>": "<type>" }` (types `text`/`int`/`json`), **not** `schema`.
- `search` (P3): `match` (**mandatory** — FTS5 query syntax) + `columns` (**mandatory**, as in `select`) + optional `where`/`order_by`/`limit`. Only on tables with a `params.fts` declaration (otherwise `invalid_input`). Every result row additionally carries a `rank` column (bm25, smaller is better); without `order_by`, `rank` is the default ordering.
- `traverse` (P4): multi-hop over an edge table via a recursive CTE, **directed** `src`→`dst`. Args: `table` + column roles `src`/`dst` (optional `kind`/`weight` — all catalog-validated), `start` (bind value), optional `where` (full operator set, applied per edge) and `columns` (additional edge columns in the path rows), guards `max_depth` (default 2, cap 5) and `max_nodes` (default 200, cap 5000) — values above the cap ⇒ **reject** (`invalid_input`), no silent clamping. Cycle elimination per path including the start node (an edge back to the origin is pruned). The result is an **object payload** `{ paths, truncated, max_depth, max_nodes }`; every path row carries end node, depth, path array, edge attributes and accumulated weight. **No** `order_by` (BFS-style expansion; the order within one depth is not part of the contract); `truncated: true` makes the `max_nodes` cutoff visible.
- `similar` (P4): similarity ranking over a vector column via the registered `hamming()` scalar function. Args: `table`, vector column, query vector (bind), optional `where`/`order_by`/`limit`, `columns` (must **not** contain `distance`). Every result row carries `distance` (smaller is better); default ordering is `distance` ascending with a `rowid` tiebreaker. Vectors are **Base64 TEXT** (primary; real BLOBs are additionally accepted — a native blob write path is a roadmap defer), strict Base64 (reject on alphabet, padding and length errors), `NULL` → `NULL`; **a length mismatch between two vectors ⇒ loud `sql_error`** (a mismatch is almost always a breach of the embedding-generation discipline, never a silent skip). The op **always** implicitly adds `<vector column> IS NOT NULL` — `NULL` embeddings (backfill queue) would otherwise rank first. Known limits: no enforced model equality (the caller filters `model_id` itself), no ANN index — full scan over the filtered set.

`columns` thus has a different form depending on the operation: with `select` an **array of column names** (projection), with `create_table` a **2-level type map**. `where`: per column either a bare value (shorthand for `eq`) or an operator object with exactly one key out of `eq`, `neq`, `lt`, `lte`, `gt`, `gte`, `in` (array), `is_null` (bool), `or_null` (wrapping exactly one comparison operator, depth 1). An object with an unknown key ⇒ `invalid_input`. The operator forms apply uniformly to `select`/`search`/`update`/`delete` (one shared `build_where` path). `schema` is exclusively the `params` block (bootstrap tables). Phase 9 accepts only `tool_call` turns; direct use with `user`/`system` origin (see below) is a Phase-9 limitation.

**Body format of the response**: `messages[]` with a single turn. In tool-loop use typically `{ origin: "tool", type: "tool_result", text: "<json-serialized result>", id: "<tool_call_id>" }`. In direct use outside a tool-loop, the `origin` may also be `user` or `system` depending on the application convention; `id` is then omitted.

**Output header** (`hop` compartment, expires on the next cell emission): `operation`, `rows_affected`, `duration_ms`, optional `error_code`.

**Failure classification** (Phase-9 brainstorm E5, analogous to `bash`): SQL errors (constraint violation, type mismatch, unknown table/column) are **regular `tool_result` turns** with `header.error_code` (`"sql_error"` / `"unknown_table"` / `"unknown_column"` / `"type_mismatch"` / `"constraint_violation"` / `"query_timeout"` / `"invalid_input"` for malformed args or unknown operation), **not** `finish_reason: "error"`. Rationale: the LLM/caller reads the code and decides (retry, schema correction, different operation). Only internal errors (DB corruption, spawn error) trigger a cell crash + restart. Since P3, `unknown_column` also covers `select`/`where`/`order_by` (previously only the `insert` path via the SQLite error text). The `traverse`/`similar` failure cases (P4) map onto the existing codes — `invalid_input` for guard/arg violations, `unknown_table`/`unknown_column` via the catalog, `sql_error` for vector mismatch, `query_timeout` — **no new code**.

**`params`**:

- `schema` (Phase-9 brainstorm E6): 2-level map `{ "<table>": { "<column>": "<type>" } }` with types `text` / `int` / `json`. Constraints (PK / NOT NULL / UNIQUE / default / index) are **deferred** in Phase 9. A separate design pass is needed.
- `fts` (P3): map `{ "<table>": ["<column>", …] }` — enables an FTS5 full-text index (external-content table + triggers) over the listed columns. Only tables from `params.schema`, only `text`/`json` columns; **no FTS for tables created via `create_table`** (known limit P3). Immutable like `schema`. Existing `cell.db`s build the index once on the next spawn — including rows written before the declaration; a declared column drifting from the live schema ⇒ loud spawn error.
- `query_timeout_ms` (concept A, see overview § Timeouts): per-query enforced timeout via `DbConn`'s `InterruptHandle`; demonstrably also interrupts a running recursive CTE (`traverse`).
- Optional seed data (convention path `seed/<table>.jsonl`). Seed takes effect only on `OpenStatus::Created` of the `cell.db` (see overview § Seed concept).

**Runtime param updates (β, `config.md` § Access L.20):** like `llm` (see there): top-level `params` body slot, partial, last-write-wins, persisted in the `cell.db`, replayed over the birth params on wake/respawn. **Mutable:** `query_timeout_ms`: takes effect **immediately live** (the running `DbConn` adopts the new A-timeout for the next query, without wake/respawn). **Immutable per `store`:** `schema` (bootstrap-only, baked into the `cell.db` via DDL at spawn; a runtime change would desynchronize the live tables from the declared schema). An update attempt on `schema` or an unknown key ⇒ loud reject (`error_code: "invalid_input"`), no partial apply.

---

## `llm`: LLM inference via provider adapter

**Task**: bridge to an LLM provider. Consumes and emits universal body format (see `meclaw-overview.md` section "Body format (universal)"). **No inner loop**: exactly one provider call per inference message. Iteration (tool-loops, ReAct, plan-and-execute, …) arises through topology.

**Emission mode**: atomic-emitting. Per inference call the `llm` cell emits exactly one new assistant turn. The incoming `messages[]` is **not** passed through. Whoever wants to hold the conversation thread together across multiple steps builds that via topology (e.g. a memory hive in front of the `llm` cell that aggregates history and passes it to the next call). Consistent with the "messages are atomic" discipline and the cell-emission-mode table in `meclaw-overview.md`.

**Inference trigger**: exclusively `messages[]`. System updates (paths under `system.*`) accumulate in `cell.db` without a provider call.

**State in `cell.db`**:
- `system.*`: accumulative-replace per path. Bootstrap context (persona, tool schemas, facts). Updates arrive per message from arbitrary cells; the sender does not know the structure.
- `messages[]`: last-received as-is (blob refs unresolved, no appended turns).
- **Not in cell.db**: appended assistant turn (output), blob cache (in-memory only).

**`params`**:
```json
{
  "provider":    "openai",
  "model":       "gpt-4o",
  "api_key":     "${OPENAI_KEY}",
  "base_url":    null,
  "temperature": 0.7,
  "max_tokens":  4096,

  "external_timeout_ms": 110000,

  "system_order":   ["identity", "facts", "instructions", "tools"],
  "provider_extra": { },

  "http_referer": "${OPENROUTER_HTTP_REFERER}",
  "x_title":      "${OPENROUTER_X_TITLE}",

  "auth":                 "api_key",
  "auth_ref":             null,
  "wire_dialect":         null,
  "oauth_token_endpoint": null,
  "oauth_client_id":      null,
  "oauth_originator":     null
}
```

- `external_timeout_ms` (concept A, see overview § Timeouts): A-timeout around the provider HTTP call (`tokio::time::timeout`), default `110000` (110 s). On Elapsed: regular error message with `finish_reason: "error"`, `error_code: "timeout"`.

- `provider` (Phase 8): **`"openai"` only** (including OpenAI-compatible endpoints via `base_url`). The value is set up as an enum, but Phase 8 implements exclusively the OpenAI translate. Further providers (in particular `"anthropic"`, Messages API native) are **deferred**, no fixed phase reference (see "Multi-provider" below). A non-`openai` value is in Phase 8 a `model_not_found`/`invalid_input`-equivalent configuration error at spawn.
- `auth` (P10): **`"api_key"`** (default) | **`"oauth_subscription"`**. Selects the credential source, **not** the provider. Exactly **one** credential per cell: `api_key` is required for `"api_key"` and forbidden for `"oauth_subscription"`; `auth_ref` the other way round. Any violation is a configuration error at spawn whose message **never** names a param value.
- `auth_ref` (P10): path to an OAuth token store in the Codex `auth.json` format. Required for `auth: "oauth_subscription"`, forbidden otherwise. **No default, deliberately.** An implicit `~/.codex/auth.json` would let a cell rotate the `refresh_token` of a live interactive session; sharing a store is therefore a config decision, not a code decision.
- `wire_dialect` (P10): **`"chat_completions"`** | **`"responses"`**; `null` derives it (`api_key` → chat-completions, `oauth_subscription` → responses). A separate axis **orthogonal to `provider`**: the Responses API is the same vendor with a different wire shape, not a different provider, so the `provider` constraint above is untouched. `auth: "oauth_subscription"` with `"chat_completions"` is a configuration error (the subscription backend speaks Responses only).
- `oauth_token_endpoint` / `oauth_client_id` / `oauth_originator` (P10): overrides for the OAuth refresh defaults and the `originator` request header. `null` = provider default. They exist so an endpoint drift is fixable without a release, and so tests can point at a fake.
- `base_url` overrides the provider default (useful for local/proxied endpoints like LiteLLM, Ollama, vllm, all over the OpenAI-compatible wire).
- `system_order`: optional order of the `system.*` sub-slots when concatenating into the provider system string. Sub-slots not listed come afterwards in alphabetical order.
- `provider_extra`: free JSON block for provider-specific knobs (Phase 8: e.g. OpenAI `seed`). Overlay over common params on conflicts. Provider-foreign knobs (e.g. Anthropic `cache_control`) are active only with the respective provider translate.
- `http_referer` / `x_title`: optional provider attribution (OpenRouter `HTTP-Referer` / `X-Title`). **Regular params** (audit ruling A4, params-uniform): set in `config.json`, substituted via `${VAR}` from `.env` like any other param, **no** code path reads `.env` directly, **no** special header mechanics. Unset (`null`/omitted) ⇒ the header is **not** sent. The wire target (HTTP request header instead of request body) is decided by the translate boundary (see "Provider translate" below).

**Runtime param updates (W4b, `config.md` § Access L.20):** params are cell **content**, not topology state. They change per **message**, not per mutation. The form is a **top-level `params` body slot** (1:1 with the `config.json` `params` block), partial, **last-write-wins per key**:

```json
{ "params": { "model": "gpt-4o-mini", "temperature": 0.4 } }
```

Order within a message: the `params` slot is merged **first** + persisted in the `cell.db`, **then** a possibly co-sent `system`/`messages` inference runs with the **updated** params (the same call already uses the new model / the new attribution). A **params-only** message (slot without `system`/`messages`) persists and stays silent (no emit, analogous to system-only). `config.json` thereby diverges from the live state, **intended**; on wake/respawn the cell replays its `cell.db` overlay over the birth params (`config.json` remains the instantiation snapshot). **Reset = `cell.db` wipe ⇒ bootstrap params** back.

**Immutable per llm** (update attempt ⇒ **loud reject**, `error_code: "invalid_input"`, **no** partial apply): `api_key` (credential, secret hygiene, mirror of the A4 `Authorization` ruling), `provider` (Phase-8 identity) and the entire P10 auth dimension — `auth`, `auth_ref`, `wire_dialect`, `oauth_token_endpoint`, `oauth_client_id`, `oauth_originator`. Rationale for the extension: `auth`/`auth_ref` are credential identity, and `wire_dialect`/`oauth_*` decide **which endpoint** a credential is presented to; if they were mutable, a message could redirect an existing token to a new destination. **Unknown** param keys ⇒ likewise loud reject (no silent no-op). A malformed value (wrong type) ⇒ reject (all-or-nothing). The reject detail names only the key/the rule, **never** a param value.

**Tool definitions**: live in `system.tools.<tool_name>.text` as JSON strings. The adapter parses them at the provider call and builds the provider-native tool set. Tools are **not** concatenated into the system-prompt string. Extracted separately. Tool calls and tool results are their own `messages[]` turn types (`type: "tool_call"` / `"tool_result"` with `id` as the correlation anchor, pass-through value from the provider).

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

**`meta.error` fine classification (P10).** The `error_code` enum above is **closed** and deliberately gains no new value for the subscription lane. The discriminator a failover edge needs lives in `meta.error` instead:

| Case | `error_code` | `meta.error.kind` | extra |
|---|---|---|---|
| subscription quota spent | `rate_limit` | `quota_exhausted` | `resets_at` (unix seconds), `plan_type` |
| plan does not cover the model | `rate_limit` | `plan_not_included` | — |
| ordinary rate limit | `rate_limit` | `rate_limited` | — |
| token expired, even after refresh + one retry | `auth` | `auth_expired` | — |
| refresh token permanently dead | `auth` | `auth_permanent` | `re_login_required: true` |
| token store missing/unreadable | `auth` | `auth_store_unavailable` | — |
| 5xx / overload | `provider_error` | `transient` | — |

Pre-P10 failure paths emit **no** `kind` — their message is unchanged.

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

**Output header** (`hop` compartment, expires on the next cell emission): `operation` (= `"bash"`), `exit_code`, `duration_ms`, `had_stderr` (mandatory, always set), `bytes` (length of the `text`), optional `truncated` (on long stdout).

**`params`**: typically the command to execute or the script-path convention.

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
- **No security boundary**: bash has full FS access via the shell. Trust model: the bash cell runs only in trustworthy topologies. Sandbox build-out is post-roadmap.
- **Shell**: `/bin/sh -c <command>`. `cwd`/`shell` as params deferred (operator sets via `cd /x && cmd` inline).
- **No persistent bash** (`cell.timeout: -1`): by-design dropped (architecture ruling 2026-06-08). `bash` is one-shot only, not a deferred option.
- **Input minimal**: `{"command": "..."}`.
- **Defaults**: `max_concurrency: 4`, `external_timeout_ms: 60000`.

---

## `code`: programmable body constructor

**Choice of `bash` vs `code`** (for AI builders and template authors):

- Do you only need to "issue a command, emit stdout/stderr as a `tool_result` turn"? → **`bash`** (always one-shot, also for command sequences, `cwd`/`env` continuity if needed via the `bash` `cell.db` per call, see § `bash`).
- Do you need program logic that manipulates the body, makes several messages from one (multi-send), sets headers deliberately, or reworks incoming `messages[]`? → **`code`**.

**Task**: runs user-supplied program in a declared language (Python first; Node and others later). Unlike `bash`, `code` is a **body constructor**: the script gets the incoming message as JSON, builds the outgoing content JSON entirely itself: headers, `messages[]`, own top-level slots, routing-relevant headers for edges. This makes `code` the Swiss army knife for application-specific logic: dissecting LLM outputs, extracting tool calls, transform logic, multi-send dispatchers.

**Rationale for this role**: a simple subprocess wrapper analogous to `bash` would not cover this task surface. Body manipulation, multi-send and header routing need program logic, not just stdout-to-text. Rejected were: (a) `code` as a bash-like wrapper with "scalar lift" (the cell extracts only scalar header values from stdout, does not cover the real application surface, leaves the body untouched), (b) separate transform cells for each of these tasks (would enlarge the cell-type catalog with no added value), (c) making `bash` and `code` formally identical (would make `bash` unnecessarily heavy). With the body-constructor model the catalog stays lean, without having to invent new cell types. Trade-off: `code` and `bash` are not formally symmetric, that is intended and explicitly resolved for AI builders via the choice heuristic above.

**Emission mode**: **script-determined**: atomic-emitting or stream-propagating, depending on whether the script passes through the incoming `messages[]` or builds it anew. `code` is the only cell type without a fixed emission mode.

**Script interface**:
- **stdin**: JSON-serialized incoming message, everything the cell reads per its `contract.consumes` and the standard message convention (`header`, body slots, plus the envelope fields `target`, `reply_to`, `trace_id`, `parent_message_id`, `correlation_id`, `ttl`).
- **stdout**: complete content JSON in exactly the form every other cell also produces. `header` section (optional) plus top-level slots. The **wire format is unchanged**: the script still writes a `header` section. Colony interprets this as `hop` (the isolated cell output, expires on the next cell emission), the rest becomes `message.body`. The script does **not** write `context` (that is solely edge authority).

**The script never sees its `params`.** `build_stdin_json` (`crates/meclaw-cells/src/code/wire.rs`) puts exactly the body, `header` (both compartments `context` + `hop`), `target`, `reply_to`, `trace_id` and `ttl` (plus `parent_message_id` / `correlation_id`, if set) on stdin, **no** `params`. Configuration of a `code` script therefore runs exclusively via `${VAR}` substitution (which applies at bootstrap **and** at mutation instantiation) or via the message itself. The context route is explicitly **no** substitute: the `/colony` reply comes back with an empty context, a two-phase cell would lose its configuration exactly when it needs it.

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
    "messages": [{ "origin": "assistant", "type": "text", "text": "Drei Tools werden parallel angefragt." }] }
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
- script writes a JSON array without `multi_send_capable` → error with `error_code: "multi_send_not_declared"`.
- script stdout valid, but `contract.emits` violated → error with `error_code: "contract_violation"`. This `code` validation runs **always-on** (unconditionally, independent of build profile and `colony.json` `strict_validation`, `code` is the only user-script-driven trust boundary; see `meclaw-overview.md` § "Schema validation: timing and scope" and `docs/config.md` § Schema format and validation).

**`params`**: typically `runner` (canonically `"python3"` in Phase 9, `CodeParams::parse` rejects other values with `'params.runner: only "python3" is supported in Phase 9'`. Background: on the target platforms Ubuntu 24 / Python 3.12 the real binary is `/usr/bin/python3`, `python` deliberately does not exist there), script path or inline code, `external_timeout_ms` (concept A, see overview § Timeouts; default `60000`). **`multi_send_capable` is not (any longer) in `params`**. It comes from `contract.multi_send_capable` (see Multi-send above).

**`cell.db` for `code`** (Phase-9 brainstorm E9): **deferred** in Phase 9. DB access from script logic runs via topology (`code` → multi-send → `store`), not in-process. Whoever needs a collector/state pattern in `code` lifts that into a separate design pass.

---

## `web_fetch`: outbound HTTP client

**Task**: pure HTTP tool. Stateless (no `cell.db`). **Only `GET` is implemented** (Phase-7 Slice-3, see Phase-7 conventions below); `POST`/`PUT`/`PATCH`/`DELETE` including `method`/`headers`/`body` are a roadmap defer.

**Emission mode**: atomic-emitting. Per HTTP call one `tool_result` turn.

**Body format of the response**: `messages[]` with one turn `{ origin: "tool", type: "tool_result", text: "<response body>", id: "<tool_call_id>" }`. On a large body the **entire** output message is offloaded (from Phase 12) as `Body::Blob`, **whole-body offload** at the delivery boundary (`blob_inline_max_bytes` threshold, `resolve_blob_for_delivery`), **not** an in-message `text_id` pointer. In-message pointers (`text_id`/`messages_id`) have **no producer** today (D-025 deferred).

**Output header**: `operation` (= `"web_fetch"`), `http_status`, `content_type`, `duration_ms`, `bytes`, optional `truncated`.

**`params`**: typically `base_url`, default `headers`, optional auth configuration.

**Phase-7 conventions** (Slice-3 decisions):
- **GET only** in Slice 3. `method`/`headers`/`body` deferred.
- **Input minimal**: `{"url": "..."}`.
- **non-2xx HTTP status = NORMAL tool_result** with `http_status` header. The LLM/caller reads the status. Only DNS/connect/timeout/invalid input produce error messages (`io_error` / `timeout` / `invalid_input` on missing/invalid `url`).
- **TLS**: rustls (`rustls-tls` feature of reqwest); no OpenSSL/native-tls in the tree.
- **Header**: `operation: "web_fetch"`, `http_status: u16` (mandatory), `content_type: String`, `duration_ms`, `bytes`.
- **Truncation/blob**: deferred (Phase 12), large bodies inline in `text`.
- **`reqwest::Client` per cell instance** (internally Arc, no Mutex). Build error at spawn → spawn error. RespawnFn clones the initially built client.
- **Defaults**: `max_concurrency: 32`, `external_timeout_ms: 30000`.

---

## `web_search`: web-search client

**Task**: pure search tool, talks to an external search provider (e.g. Brave, Tavily, SerpAPI). Stateless (no `cell.db`).

**Emission mode**: atomic-emitting. Per search request one `tool_result` turn.

**Body format of the response**: `messages[]` with a `tool_result` turn whose `text` contains the search results as a JSON list (title, URL, snippet per hit). On large result lists (from Phase 12) whole-body offload of the entire message as `Body::Blob` at the delivery boundary, **not** via an in-message `text_id` pointer (D-025 deferred).

**Output header**: `operation` (= `"web_search"`), `result_count`, `duration_ms`, `bytes`.

**error_codes**: `io_error` (DNS/connect error), `timeout` (external_timeout elapsed), `invalid_input` (missing/invalid `query`). A merely non-conformant provider response is **not** an error (see Phase-7 conventions: `result_count=0`, body passed through).

**`params`**: typically provider `base_url` and API token (via `${VAR}` substitution).

**Phase-7 conventions** (Slice-3 decisions):
- **Generic JSON wrapper**: the cell does GET `<params.endpoint>?q=<query>` with optional `params.api_key` as bearer token. Expects response `{"results":[{"title","url","snippet"}]}`.
- **Provider-specific adapters** (Brave, Tavily, SerpAPI, …) are **deferred**. Application topology via a `code` cell (Phase 9) or builder-hive normalizes.
- **Input**: `{"query": "..."}`.
- **Graceful on non-conformant response**: `result_count=0` when the `results` key is missing or not an array. The body is ALWAYS passed through in `text`, **no hard error**.
- **Header**: `operation: "web_search"`, `result_count: u64`, `duration_ms`, `bytes`. (The `http_status` header is deferred here, parity with web_fetch would be more consistent, but is post-Slice-3.)
- **Truncation/blob**: deferred (Phase 12).
- **`reqwest::Client` per cell instance** (analogous to web_fetch). Build error at spawn → spawn error. RespawnFn clones the client.
- **Defaults**: `max_concurrency: 8`, `external_timeout_ms: 15000`.

---

## `file`: filesystem operations

**Task**: CRUD for files within a security boundary. Path traversal outside the boundary is rejected. Stateless.

**Emission mode**: atomic-emitting. Per operation (`read`/`write`/`list`/`stat`) one `tool_result` turn.

**Body format of the response**: `messages[]` with a `tool_result` turn. On `read`, `text` contains the file content (on large files from Phase 12 whole-body offload of the entire message as `Body::Blob` at the delivery boundary, **not** via an in-message `text_id` pointer, D-025 deferred). On `write`/`list`/`stat`, `text` contains a JSON-structured status (bytes written, file list, stat info).

**Output header**: `operation` (`"read"`/`"write"`/`"list"`/`"stat"`), `bytes`, `duration_ms`.

**`params`**: `base_path` (mandatory; security boundary).

**Phase-7 conventions** (Slice-1 decisions):
- **`target = reply_to`**: FileCell emits to `msg.reply_to`; fallback `/colony/dead_letters` if `reply_to` is missing. Edges in the topology can override the target.
- **`tool_call.text` is JSON args**: `{"op": "read"|"write"|"list"|"stat", "path": "<rel>", "content"?: "<str for write>"}`.
- **`write` without auto-mkdir**: the parent dir MUST exist. Missing parent → `io_error`. Symlink-safe via parent canonicalize.
- **Security boundary**: all paths canonicalized against `base_path` (symlinks resolved); traversal/absolute-rel/symlink-escape → `path_outside_boundary` or `invalid_input`.
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
- **`insert_at_line` is 1-based and insert-BEFORE**: `line = 1` → at the very start; `line = file_lines + 1` → at the very end. `line < 1` or `line > file_lines + 1` → `invalid_input`.
- **Shares FileCell's security boundary**: same `base_path` logic (extracted into `meclaw-cells/src/boundary.rs`).
- **Not atomic**: read-modify-write without tempfile+rename (consistent with FileCell::write). Crash mid-way = OS-level problem. Atomic edits are post-roadmap.
- **Concurrent edit on the same file**: race condition possible (no lock in Phase 7). The caller topology serializes if needed.
- **Input**:
  - `{"op": "find_replace", "path": "<rel>", "find": "<str>", "replace": "<str>"}`
  - `{"op": "insert_at_line", "path": "<rel>", "line": <u32>, "content": "<str>"}`
- **Default `max_concurrency`**: 8.
- **error_codes**: reuse from file + new `pattern_not_found`.

---

## `proxy`: external-chat-platform bridge

**Task**: long-running. Bridges to an external chat-platform provider (Telegram first; further platforms follow). Holds in `cell.db` a cursor for update offsets, so that restarts do not process messages twice.

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

**Operation per message** via the mandatory field `op: "add" | "modify" | "remove"`
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

`modify` carries `schedule_id` plus the fields to change (e.g. a new `cron`).

**Semantics** (strict, no heuristic):
- `add` = INSERT; an existing `schedule_id` → error (no implicit upsert).
- `modify` = UPDATE of the carried fields; an unknown `schedule_id` → error.
- `remove` = deactivate the schedule (status update in `cell.db`, **No-Delete-conformant**, no
  row deletion); an unknown `schedule_id` → error.

**Validation & error surfacing**: on `add`/`modify` a `cron` expression is validated against the 6-field Quartz parser. Invalid expressions are rejected (no silently stored, never-firing schedule arises). All op errors are emitted as a message to the `reply_to` of the op message (`parent_message_id` = the consumed op message), with `header.error_code` for: `invalid_body` (body not inline-readable), `parse_error` (op message unparsable beyond the cron check), `schedule_id_exists` (add on an existing `schedule_id`), `schedule_not_found` (modify/remove on an unknown `schedule_id`), `kind_mismatch` (modify type switch once↔repeating), `invalid_cron` (invalid cron expression). Successful ops are not acked.

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

**Post-init backend death**: on `http` the cell holds **no** persistent connection. **Every** tool call connects anew. If the MCP backend dies transiently *after* the discovery, the cell therefore recovers **automatically on the next tool call** (the fresh connect succeeds again); a permanently dead backend manifests per call as `provider_timeout` or `mcp_error`. A death detection *between* calls does not exist on `http` (`run_http_io` pends after the discovery; a roadmap defer, trigger = SSE build-out). On `stdio` the stream read carries the liveness signal: on EOF or exit an open call first receives a regular error message (`mcp_error`, the detail names exit code/signal/EOF), then the cell panics → `one_for_one` with a fresh child process; after `restart_limit` the registry entry is retained as `failed`. No new `error_code`.

**Emission mode**: atomic-emitting. Per MCP tool call one response message with the result as a turn.

**Body format of the response**: `messages[]` with a `tool_result` turn, `text` contains the MCP tool answer (typically JSON-structured). On large answers (from Phase 12) whole-body offload of the entire message as `Body::Blob` at the delivery boundary, **not** via an in-message `text_id` pointer (D-025 deferred).

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

**Cancel**: a `cancel` message (with `task_id`) sets the tombstone to `cancelled` **before** the child is stopped (process-group kill including grandchildren) and emits the task end marked `cancelled`; the cell accepts new tasks afterwards. Cancel is the stop lever for the deliberately unbounded task runtime (see overview § Timeouts).

**`params`**: `adapter` (required; today only `"claude-code"`), `emit_to` (required), `workspace_root` (required, canonicalized — tasks run only below it), `command`, `model` (from `${VAR}`), `permission_mode`, `max_turns`, `max_budget_usd`, `allowed_tools`, `extra_args`, `env`, `env_passthrough`, `approval` (`off` | `channel`), `startup_timeout_ms`, `external_timeout_ms`, `query_timeout_ms`, `kill_grace_ms`.

**Trust model (empirically established 2026-08-09)**: `harness` is **not a sandbox**. The harness brings its own tools (shell, file access, network) and runs with the rights of the colony process. The load-bearing V1 barriers are **`env_clear` + `env_passthrough`** (the harness does not see the colony's secrets) and the **canonicalized cwd clamp** under `workspace_root`. **`allowed_tools` is explicitly NOT an upper bound**: the CLI treats `--allowedTools` **additively** to what the permission mode allows anyway — in the acceptance smoke, `Bash` ran despite `allowed_tools: ["Write"]`. `allowed_tools` extends, it does not restrict. Sandbox/container build-out = roadmap defer, as with `bash`; `harness` cells run only in trusted topologies.

**Runtime param updates (β)**: mutable `model`, `max_turns`, `max_budget_usd`, `startup_timeout_ms`, `external_timeout_ms`, `query_timeout_ms` — take effect from the **next** task. Immutable and a loud reject (`invalid_input`): `adapter`, `command`, `emit_to`, `workspace_root`, `env`, `env_passthrough`, `permission_mode`, `allowed_tools`, `extra_args`, `approval`, `kill_grace_ms` — that is the containment boundary. A params-only message persists and stays silent.
---

## `subcolony`: a child colony as one cell

**Task**: long-running. Operates a complete child colony as **one** cell in the parent graph. The child is a real `meclaw` binary with its own `{root}`, its own `colony.json`, its own `colony.db` and its own cell tree, supervised as a child process on the stdin/stdout bridge in JSON mode (`--stdio-format json`, wire v1, see `meclaw-overview.md` § Stdin/stdout bridge). **No in-process nesting**: a colony stays one process with one tree; nesting happens across the process boundary. The facade is therefore an **opaque composition boundary**: from the outside the child colony is exactly one addressable cell.

**Concurrency setup**: **two Tokio tasks per instance** (handler + I/O), prescribed, see `meclaw-overview.md`, section "Long-running cells: dual task". The I/O task owns the child process entirely (on the `stdio_child` core, like `mcp` stdio and `harness`); the handler owns the request path and the emissions. The cell holds **no** lock and **no** shared state: the pending requests live in the serve loop, the cell state in the handler task.

**Boot handshake**: spawn (`--root <root> --stdio-format json`, `env_clear`, own process group; `--daemon` and `--api` are **never** set, because stdin EOF must end the child) → the child writes exactly one `ready` frame, **after** its bootstrap succeeded and **before** it reads stdin. `boot_timeout_ms` clamps the A-timeout on that frame. **`v` is the protocol integer and is asserted strictly; `version` is the child's release version and is reported only. Version skew between parent and child is the feature, not the fault.**

**Boot failures and restart cost**: deterministic boot failures (foreign protocol, absent `ready`, spawn failure) do not panic: the cell stays up and rejects every request loudly with the reason — the restart budget is not burned on a certainty. Only transient child death goes into `one_for_one`. One restart cycle costs a full child-colony boot; `boot_timeout_ms` is the upper bound of that cost and therefore the quantity to reckon with in the context of `cell.restart_limit`.

**No automatic re-fire path**: in-flight requests fail loudly with `subcolony_gone` when the child dies; a retry is the requester's decision, never the substrate's. A request is explicitly **not** free to repeat — it may already have triggered store writes inside the child.

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

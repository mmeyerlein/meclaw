# `config.json` format

Detailed spec of the `config.json` format per cell and per hive scope marker. In case of conflict between this file and `meclaw-overview.md`, the overview wins. It is the single source of truth.

## Supreme rule

**A cell does not know what happens before or after it.** It knows only its own contract (input/output schema), its params, and the message it is currently processing. It has **no** knowledge of sender paths, receiver paths, hop history, routing strategies, or other cells.

Messages are atomic. Trace reconstruction lives in the central message log in `colony.db` (filterable by path prefix), not in the message.

**Envelope fields are read-only from the cell's perspective.** `id`, `trace_id`, `parent_message_id`, `correlation_id`, `target`, `reply_to`, `ttl`, `created_at` are set exclusively by colony during routing. A cell can neither write them in its content JSON nor manipulate them via an edge (see `meclaw-overview.md` section "Envelope setter authority"). Anyone wanting a reply target other than the sender solves it application-specifically via header-based routing.

**From the cell's perspective, the world is single-threaded.** A `handle()` call runs to completion before the next starts. The cell task pulls sequentially from the mpsc mailbox. Cell code therefore contains no `Mutex`, no `RwLock`, no atomics, no reentrancy defense. The system's parallelism lives outside the cell, see `meclaw-overview.md`, section "Concurrency and parallelism".

## Access

- **Authority**: Only the colony reads and writes `config.json`. The **only writer is instantiation** (exactly once). **Read-once:** the running cell task **never re-reads** `config.json` after startup; `config.json` is the **instantiation snapshot**, not a live document.
- **At instantiation**: colony copies the template, assigns a new UUID v7, performs `${VAR}`/`${ctx.*}`/`${uuid7:*}` substitution, writes the result into the instance's `config.json`. **The node reference is the filesystem directory name** (the path segment under `{root}`), **not** a `cell.name` field. The `config.json` carries no `name`. When resolving the root chain, the `${...}` substitution wins over the `template.json` template name. Naming collisions with siblings inside the same hive scope are rejected by colony in the single-stage mutation validation, see `meclaw-overview.md` section "Naming collisions".
- **After instantiation**: `config.json` is semantically frozen, the bootstrap snapshot. No one writes into it anymore, neither colony nor the cell itself. **Dynamic cell state** (changed params) lives exclusively in `cell.db`; **colony state** (registry, edge table, `cell_id`, message log, mutations) lives in `colony.db`. After the snapshot, `config.json` carries neither of the two forward (see `meclaw-overview.md` section "Lifecycle of `config.json` and `cell.db`"). The graph of a topology lives centrally in colony's registry and `colony.db`, not in the `config.json` of the hive scope marker (its `params.graph` is only an initial bootstrap hint).
- **Cells do not read `config.json`.** The colony hands the cell the `params` block at startup. Param updates come afterwards via message and are persisted by the cell in its `cell.db` (`config.json` diverges from the live state, by design; cell reset = `cell.db` wipe → cell starts again from the bootstrap state).

## Structure

```json
{
  "cell":        { ... },
  "params":      { ... },
  "contract":    { ... },
  "description": { ... }
}
```

### Block definition (canonical)

Two of the four blocks have fundamentally different authority. This separation is the root of the entire file:

- **`cell` block = colony substrate.** These fields control **how the colony** instantiates, registers, and supervises the cell. They are **never** handed to the cell. The cell sees only its `params` block plus the message it is currently processing. Allowed keys: `id`, `type`, `timeout`, `restart_limit`, `idle_timeout_ms`, `mailbox_size`, `message_timeout` (details in the `cell` table below). A key declared in the `cell` block that is not allowed is a boot error.
- **`params` block = handed 1:1 opaque to the cell.** After `${VAR}`/`${ctx.*}`/`${uuid7:*}` substitution, the colony passes it through to the cell **unchanged** and does **not** interpret its content. Cell-type-specific (each cell type defines its own `params` structure, see `cell-types.md`). **Sole exception:** at the hive scope marker, the colony reads `params.graph` as the initial desired graph (the hive is not an actor, so it does not get a `params` block "handed" to it).

**Only `id` and `type` are immutable.** They identify the node instance and its cell type across the entire lifetime. **Effectiveness rule** for all other fields: changes to `cell` or `params` fields (via new instantiation at the path or a new template) take effect **at the next spawn/wake** of the cell. The running cell task does not re-read `config.json` (see § Access, "Read-once").

**Special case hive scope marker** (`cell.type: "hive"`): only `cell` and `params` are relevant. `params` contains only the optional `graph` block (initial desired graph, see `meclaw-overview.md` section "Graph schema"), no `dead_letters` override (the `HiveParams` deserializer is `deny_unknown_fields`; the DLQ is always `/colony/dead_letters`). A `contract` block is not evaluated, because hive scope markers are not actors and do not participate in the message flow (see `cell-types.md` section `hive`). In the `cell` block, only `id` and `type` are relevant. `timeout`, `message_timeout`, `idle_timeout_ms`, and `mailbox_size` are ignored (no actor, no mailbox, no `handle()` call). A `description` is allowed, but only serves discovery by builders; `emits_meaning` and `consumes_meaning` are omitted.

### `cell`

| Key | Content |
|---|---|
| `id` | `cell_id` (UUID v7). **Set during the copy operation template → instance**, the **only time** it is written. Instantiation reads it from the freshly written `config.json` and persists it into the **never-deleting `colony.db`**, which from then on is the **authoritative** source of the `cell_id` (`config.json` is only the bootstrap imprint). Afterwards **never reassigned**, not even on reconnect, resume, or reboot. (The re-dedicated `swap_nodes` graph swap pivots edges onto a different implementation with its **own** `id` and leaves the old cell with its `id` preserved but disconnected. It transfers **no** `cell_id`, see `meclaw-overview.md` § Mutation operations.) |
| `type` | Cell type (`hive`, `store`, `llm`, `bash`, `code`, `web_fetch`, `web_search`, `file`, `edit`, `proxy`, `timer`, `mcp`). Together with `id`, the **immutable** part of the `cell` block. |
| `restart_limit` | *(optional)* Maximum restart attempts by the supervisor before the cell is marked as `failed`. Default `5`. See `meclaw-overview.md` section "Restart strategy". |
| `timeout` | Hot/cold mode (see `meclaw-overview.md` section "Hot/cold cell model"): `0` = default (idle-timeout model, Awake↔Asleep), `>0` = one-shot (despawn after each message), `-1` = persistent (typically `proxy`/`timer`/`mcp`, never despawn). Phase-13 activation; before that, all cells are permanently a task. |
| `idle_timeout_ms` | *(optional, from Phase 13)* Idle duration in ms, after which a stateful cell with `cell.timeout: 0` despawns itself (Awake→Asleep). Overrides the colony default from `colony.json` `idle_timeout_default_ms`. Ignored if `cell.timeout != 0` (at `>0`, one-shot despawn after each message takes effect; at `-1`, the cell is persistent and never despawns). |
| `message_timeout` | *(optional)* Substrate backstop per `handle()` call in ms, see `meclaw-overview.md` section "Timeouts" (concept B). Overrides the colony default from `colony.json` `message_timeout_default_ms`. `0` or `-1` = no backstop (for long-running cells). **Not** the primary timeout for I/O operations. `params.external_timeout_ms` (concept A) is responsible for that. `cell.message_timeout` should be considerably more generous than `params.external_timeout_ms`, so that normally A takes effect first. |
| `mailbox_size` | *(optional, from Phase 5)* Bounded-mpsc capacity; overrides the colony default (`colony.json` `mailbox_default_capacity`, default 1000). See overview section "Mailbox size". |

### `params`

**`max_concurrency`** (*optional, only for stateless cells, from Phase 7*) lives in the **`params`** block, not in the `cell` block: maximum number of concurrently running worker tasks in the stateless-cell dispatcher (see `meclaw-overview.md` section "Stateless-cell dispatcher"). Default: high value (effectively unbounded for typical load paths). Configurable per cell: e.g. `web_fetch` with `32` (HTTP provider rate limits), `file` with `8` (disk I/O), `bash` one-shot with `4` (process resource limit). For stateful and long-running cells the value is ignored.

Cell-type-specific. Each cell type defines its own `params` structure (see `cell-types.md`). The colony hands this block to the cell at startup; afterwards param updates via message are possible (last-write-wins, persisted in `cell.db`). **Form** (W4b): the update message carries a **top-level `params` body slot** (1:1 this `params` block, partial), pure cell content, no header gate; the cell merges + persists it itself and replays the overlay at wake/respawn over the birth params (`config.json` stays untouched). Which fields are runtime-changeable or immutable (e.g. credentials, security boundaries) is cell-type-specific (see `cell-types.md`, e.g. `llm` § Runtime param updates).

`${VAR}` substitution from `.env` is performed by the colony before handover to the cell. `${ctx.<key>}` and `${uuid7:<label>}` are resolved at mutation application (see overview section "Variable substitution").

**Convention for I/O cells**: every cell that performs I/O operations of indeterminate duration (HTTP, DB, subprocess, filesystem, MCP calls) declares a `params.external_timeout_ms` field (or a semantically more fitting name like `query_timeout_ms` for `store`). The cell implementation wraps **every** such operation with `tokio::time::timeout` and, on elapsed, emits a regular error message (`header.finish_reason: "error"`, cell-type-specific `error_code` like `provider_timeout` / `query_timeout` / `script_timeout`). This is concept A in `meclaw-overview.md` section "Timeouts", the primary protection, set precisely per operation, manageable by the operator. **`cell.message_timeout`** (in the `cell` block) is the coarse backstop for cell hangs and lies considerably above `external_timeout_ms` (concept B in the same section).

### `contract`

The `contract` keys are organized by **enforcement level**; not all of them are substrate-enforced in v0.1.0:

| Key | Enforcement (v0.1.0) |
|---|---|
| `emits` | **substrate-enforced**: validated always-on at the `code` type (P13/D-017); remaining emitting cell types post-v0.1.0 (see § Schema format and validation; contract validation for the rest is a roadmap defer). |
| `version`, `settings`, `consumes` | **substrate-enforced**: presence + JSON type at config load (boot hard fail; mutation reject `contract_incomplete`). |
| `capabilities` | **discovery-only**: hint for builder composer/audit tools, **no runtime check** until the hardening (see `capabilities` note below). |

**`version` format:** non-empty string, freely choosable (no semver requirement). **`settings` format:** object `{ "<key>": SettingSpec }` (see § SettingSpec), empty object permitted. **`consumes` format:** object (see § consumes), empty object permitted.

Optional: `tools`, `multi_send_capable`.

**Body follows the universal body format**: top-level slots are primarily `system` and `messages[]` (see `meclaw-overview.md` section "Body format (universal)"). Cells may declare their own top-level slots (`meta`, `delta`, `event` etc.). `emits.body` and `consumes.body` declare the slots this cell writes or reads. Unknown top-level slots in an incoming message are ignored by the consumer.

#### `emits`: what the cell writes into its output message

Split into `body` (the actual content) and `hop` (the isolated cell output to routing metadata). Cells emit **only** `hop`. `context` is solely edge authority and does not appear in `emits` (see overview section "Headers vs. body: write model"). The cell produces content JSON; colony interprets `content.header` as `hop` and takes the rest as `message.body`.

```json
"emits": {
  "body": {
    "<key>": <EmitSpec>,
    ...
  },
  "hop": {
    "<key>": <EmitSpec>,
    ...
  }
}
```

**`EmitSpec`**:
```json
{
  "type":     "string|number|boolean|object|array|blob_uuid",
  "values":   ["..."],
  "required": true
}
```

- `values` optional, only sensible for `type: string` (enum whitelist).
- `required` defaults to `true`.

#### `consumes`: what the cell reads from the incoming message

Split into `body` (content slots) and **the two header compartments** `context` (persistent) and `hop` (exactly this hop). Cells read all three read-only; they have **no** knowledge of who set the value when. That is a topology matter. The lifetime of a header is determined **purely structurally** by the compartment name (`context` = persistent, `hop` = hop-local/expires). There is **no** per-key lifetime annotation.

```json
"consumes": {
  "body": {
    "<key>": <ConsumeSpec>,
    ...
  },
  "context": {
    "<key>": <ConsumeSpec>,
    ...
  },
  "hop": {
    "<key>": <ConsumeSpec>,
    ...
  }
}
```

**`ConsumeSpec`**:
```json
{
  "type":     "string|number|boolean|object|array|blob_uuid",
  "required": true
}
```

- If a required value is missing: cell is not called, error message to `reply_to` (if set), otherwise dead letter.
- **Mutation/locality validator**: the build-time validator uses `emits.hop` (what the cell produces) together with `consumes.context` + `consumes.hop` (what the downstream cell expects) to statically check locality and reachability of a header value. A `hop` value is only available at the immediately following hop (unless an edge carries it forward via `set_context`), a `context` value across the entire lifecycle. Hive transits participate in the fan-in intersection: an edge with a hive `from` is a transit pass-through and contributes `set_hop` of this edge ∪ the intersection of the contributions of all inbound edges of the hive (recursively across multi-stage transits, cycle-safe). The same key walk the runtime performs at transit (`hop` expires only at a cell emission, not at the transit). **Participation/status filter at boot:** at bootstrap, the locality checker carries contract obligations **only for active nodes**, nodes that participate in the active graph. A registered but **disconnected/inactive** node (persisted `colony.db` status at reboot **or** island derived as inactive from t0 at first boot) is pure bookkeeping: it is rehydrated (stable `cell_id`), but at boot is subject to **no** contract enforcement. The full check resides at the **mutation moment** that connects it (participation rule + transit-aware intersection). Thus the check is uniform across both boot kinds: inactive ⇒ no boot obligation; active-and-wired ⇒ sharply checked.

**Enforcement state:** The substrate-side required-`consumes` check runs at the delivery boundary (before `handle()`): missing/type-wrong required key → error message to `reply_to` (`error_code: "consumes_violation"`), otherwise dead letter (same token). **The error reply is delivered DIRECTLY to `reply_to`** (registry lookup via `route()`), not routed via the consumer's out-edges. It is feedback to a known sender, not a routing target (W2b Ruling, Marcus 2026-06-12; see `meclaw-overview.md` § Routing errors "Outputs arm: three disjoint cases", case 2). A catch-all out-edge of the consumer does not redirect the error reply.

#### Schema format and validation

- Schemas follow **JSON Schema Draft 2020-12** (Rust: `jsonschema` crate).
- **`code` = always-on trust boundary (no opt-out):** the `emits` validation of the `code` output runs **unconditionally** (`validate_emits = true`), independent of the build profile **and** of `colony.json` `strict_validation`. `code` is the only user-script-driven output whose correctness does not follow from cell discipline; therefore it is always checked.
- **Remaining emitting cell types:** `emits` validation runs centrally at the colony's outputs arm following the debug-on/`strict_validation` model: in the debug build always active, in the release build per `colony.json` `strict_validation: true|false` (default `false`, schema see `meclaw-overview.md` section "`colony.json` schema").
- **`strict_validation` role:** thus controls **only** the future non-`code` emits validation in the release build. The flag has **no** influence on the always-on `code` path.

**Enforcement state:** `code` always-on (in-cell, two-pass, unchanged); all remaining emitting cell types are validated **centrally at the colony's emission boundary** (outputs arm), flag-gated following the debug-on/`strict_validation` model. **Asymmetry by design:** `code` checks in-cell always-on with all-or-nothing two-pass; the rest runs centrally, flag-gated and per-emission. This is intended and not drift. Violation: emission is discarded; with `input_reply_to` error reply (`error_code: "contract_violation"`), otherwise dead letter (same token). **Two registered boundaries of the central check (debug net, not a trust boundary, Marcus ratification 2026-06-10):** (a) error replies to an `input_reply_to` that points to a `/colony/*` endpoint or a hive path are silently discarded (only the cell-path cascade is followed); (b) a cell that emits in the µs window between task spawn and the landing of its `SetNodeContract` entry (self-emitting types at boot) passes the check fail-open (absent entry ⇒ vacuous check).

#### `capabilities`: fixed list

| Capability | Meaning |
|---|---|
| `network:llm` | may contact LLM providers |
| `network:http` | may make arbitrary HTTP calls |
| `network:search` | may contact search providers |
| `network:mcp` | may contact MCP providers |
| `network:proxy` | may contact chat-platform providers |
| `fs:read` | may read the filesystem (within boundary) |
| `fs:write` | may write the filesystem |
| `shell:exec` | may execute shell commands |
| `db:own` | may read/write its own `cell.db` |
| `mutate-graph` | wants to trigger graph mutations (discovery hint, no runtime check until the hardening) |

Extensible as needed, documented centrally in `meclaw-core`.

**Note on permissions until the hardening**: The capabilities in this phase are **discovery hints** for builder composer and audit tools, **not a runtime check**. This applies in particular to `mutate-graph`: whether a cell actually _can_ mutate depends solely on the topology (does an edge to `/colony/mutations` exist?). Post-roadmap hardening may add capability tokens that are checked at runtime. See overview section "Permissions" in the mutation format.

#### `ToolSpec`

Declares which tools the cell offers to its LLM (or external consumers). **Not a routing endpoint**. Where tool calls are routed is decided by the topology.

```json
{
  "name":   "<tool-name>",
  "schema": { ... }
}
```

#### `SettingSpec`

```json
{
  "type":        "string|number|boolean|object|array",
  "secret":      false,
  "default":     "<value>",
  "description": "<text>"
}
```

#### Flags

- `multi_send_capable`: cell can produce multiple output messages from a single input. Activates the cell-type-specific multi-send wire format, for `code` e.g. the JSON-array format on stdout (see `cell-types.md`). Each emitted message runs independently through the outgoing edges; colony evaluates freshly per message. The value comes from `contract.multi_send_capable` (bool, default `false`). The former `params.multi_send_capable` bridge is removed. A `params` value is ignored by the `code` factory.

### `description`

Six keys, **builder-enforced**, not substrate-enforced: the structure takes effect as soon as the builder/composer consumes it (the same discovery contract surface for the LLM builder that writes edges, and reviewer/operator), not as boot validation in the substrate.

| Slot | Content |
|---|---|
| `purpose` | Why does this cell exist? What problem does it solve? (1-2 sentences) |
| `use_when` | When does the composer reach for this template? Preconditions, alternatives. |
| `not_in_scope` | What does this cell deliberately **not** do? Helps the builder exclude the cell when it does not fit. |
| `emits_meaning` | Semantics of the `contract.emits` entries: what do they mean beyond type info? |
| `consumes_meaning` | Semantics of the `contract.consumes` entries. |
| `examples` | Concrete input/output examples; at least one. |

**At hive scope markers** (`cell.type: "hive"`): `description` describes the scope purpose (what does this hive bundle? when does the builder use it? what does not belong in it?). `emits_meaning` and `consumes_meaning` are omitted, since hive scope markers do not participate in the message flow.

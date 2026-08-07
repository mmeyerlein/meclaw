# meclaw: system description

File-based, LLM-oriented actor workflow system for agentic harnesses. Rust binary, Linux. Specialized in LLM-typical flows with strongly simplified flow management, considerably simpler than BPMN or Serverless Workflow, with no claim to their generality.

## What is meclaw

A workflow system whose topology lives as a directory tree in the filesystem. Every node is a cell (actor); directories with `type: "hive"` mark authority and mutation boundaries. The topology is mutated at runtime by cells themselves, typically through **builder-hives** (multi-stage hive scopes with an llm cell, a diff-constructor hive, and a validator hive) that translate natural-language requests into graph mutations. Cells communicate exclusively via atomic messages with a universal body format. LLM inference, tool calls, persistent storage, long-running bridges (Telegram, timer, MCP) are all cell types.

The DSL is hierarchical (directories, paths `/main/sub/leaf`); the actor substrate underneath is flat (cells are registered directly in a central colony registry, routing is an O(1) lookup). This separation is intentional. The DSL stays naturally readable for humans and builder LLMs, the implementation stays consistent with the Tokio idiom and concentrates the concurrency complexity at exactly one place (colony).

## Classification (comparison axis)

| System | What meclaw shares | What meclaw does differently |
|---|---|---|
| Erlang/OTP | actors, mailbox, supervisor | topology as a file, not as code; LLM-specialized |
| NATS | subject-based routing | compute nodes in addition to transport |
| Node-RED | nodes are dumb, the graph routes | CLI/filesystem-first, durable, LLM-specialized |
| LangGraph | graph for agentic flows | language-agnostic, file-based, persistent |
| Temporal | durable execution, message log | lightweight, decentralized, filesystem DSL |
| BPMN, Serverless Workflow | workflow engine with a declarative definition | strongly simplified; covers only LLM flow patterns, no claim to generality |

---

## Core principles

- **Filesystem is the single source of truth.** Directory tree with a `config.json` per node = topology.
- **A cell knows nothing about topology.** No knowledge of sender, receiver, hop history, other cells. It knows only its contract, its params, the current message.
- **Messages are atomic.** Trace reconstruction via `parent_message_id` from the central message log, never in the message.
- **The DSL is the CPU.** Domain-free. Knows only routing, messaging, cell lifecycle. Everything on top is OS.
- **Cells in the `templates/` folder are classes, cells in the directory tree are instances.** A cell is topologically neutral; what it is follows from its location. Lazy instantiation copies a template hive into the tree.
- **The graph decides everything.** Routing, filtering, fan-out, exclusively through edges.
- **Hierarchy is DSL, the substrate is flat.** Directory nesting and hierarchical paths in the DSL, implemented flat as a central path registry in the colony. Routing in an O(1) lookup, no hop-by-hop cascade.
- **Hives are scope markers.** Directories with `type: "hive"` mark authority and mutation boundaries for their subtree. No own actor type, no own task, no own mailbox. They additionally act as a **logical transit node in the routing graph**, evaluated by colony, not an actor, not a delivery.
- **Colony is the authority.** For lifecycle, registry, templates, routing. All cells register directly with colony. It writes `config.json` **only on instantiation** (the re-dedicated `swap_nodes` graph swap no longer rewrites an existing `config.json`, see § Mutation operations).
- **Everything is a message, on the data plane** (cell-to-cell traffic, tool calls, uniform body format). **Control commands to the colony** (mutations, param updates, supervisor events, external API calls) **are internally typed inbox commands**, same UBF data model, same sequential colony task. Entry paths including the live cell entry, a cell emitting to `/colony/mutations` is dispatched directly: see § Mutation format.
- **Tool-loops are topology, not cell responsibility.** llm cells have no inner loop.
- **The swarm is self-modifying.** Builder-hives (hive scopes with several specialized cells) generate new topology at runtime via scoped mutations.
- **No empty directories.** Every directory in the tree needs a `config.json`, otherwise it does not exist.
- **UUID v7 everywhere.** IDs are time-sorted (messages, cells, templates, blobs, traces).
- **UTC everywhere.** All points in time in the system are UTC, no local time zone, no TZ offset. The serialization format is field/context-specific (for example envelope `created_at` as Unix seconds `i64`; human-readable/log timestamps like blob sidecar `created_at` or `timer` header `scheduled_at`/`fired_at` as ISO-8601/RFC-3339 with `Z`). Local time zones / `chrono-tz` are deferred for v0.1.
- **Agentic-first, human-second.** Discussions resolve in favor of the variant that is optimal for agent-driven builders.
- **Highly parallel, multithreaded, concurrent, by design.** Tokio multi-thread runtime, every cell and the colony as its own task, sequentiality guarantees only where the architecture sets them explicitly. Full detail in the section "Concurrency and parallelism", **read it before every implementation decision**.

---

## Concurrency and parallelism

> **The entire system is highly parallel, multithreaded, and concurrent.** This is not optional, not a later optimization, and not an implementation detail; it is a basic assumption of every single architecture decision and every cell or colony implementation. Anyone who has not read this section has not understood the system.

### Runtime

**Tokio multi-thread runtime** (work-stealing scheduler), default flavor `multi_thread`. Number of worker threads: default = number of CPU cores (Tokio default). No `current_thread` runtime in library or binary code. No `block_on` in library code; library APIs are `async` throughout. (Exception for pure unit tests without a topology: see "Test infrastructure".)

### What runs as its own Tokio task

| Actor | Tasks per instance | Inbound |
|---|---|---|
| **Stateful cell** (e.g. `llm`, `store`, `code` with `cell.db`) | 1 long-lived task | own mpsc mailbox |
| **Stateless cell** (e.g. `web_fetch`, `web_search`, `file`, `edit`, `bash` one-shot) | 1 long-lived dispatcher task + one short-lived worker task per message | own mpsc mailbox |
| **Long-running cell** (`proxy`, `timer`, `mcp`) | **2 work tasks** (handler + I/O), conceptual work-task count; encapsulated in **one** outer glue supervision task with exactly one `JoinHandle` (see "Long-running cells: double task") | external mailbox + internal channel |
| **Colony** | 1 long-lived task | own mpsc mailbox (central routing + `/colony/*` endpoints) |
| **HTTP API** (`axum`) | Tokio-native task-per-request | translates each request into a message and hands it to colony |

**Hives are not tasks.** They are pure scope markers in the filesystem (`config.json` with `type: "hive"`) and in the path scheme. Routing, authority boundaries, and mutation scoping are based on path prefixes, not on own actors or mailboxes. When a hive path is addressed as a message target, it additionally acts as a **logical transit node** in colony's one routing layer (details see "Hive paths as target: transit evaluation"), not an actor, not a mailbox delivery.

Tokio tasks are ~3 KB stack, not an OS thread. Thousands of sleeping cells have practically no overhead, only their mailbox channels in colony's registry (see "Hot/cold cell model").

### Sequentiality: what the architecture guarantees

These sequentiality islands hold **by design**. Cell and colony code may rely on them and must **not establish them themselves**, no `Mutex`, no `RwLock`, no atomics, no defense against reentrancy:

- **Within a cell**: one `handle()` call runs through completely before the next starts. Follows from the mpsc pull semantics of the one cell task. **Cell state is effectively single-threaded accessible from the cell's perspective.**
- **Within the colony**: routing lookups, mutation, registry, and template operations run sequentially through colony's single task. A mutation can cut in atomically between two routing steps; other messages pause in colony's mailbox until the mutation is finished. No parallel writing of `config.json`, no parallel creation of staging directories for the same mutation.
- **Long-running cells, from the state perspective**: from the perspective of the state-holding handler task, inbound (mailbox) and outbound (provider event) also run sequentially; the I/O task only buffers events into an internal channel, the handler processes one after another.

### Parallelism: what the architecture enforces

The counterpart: everything not in the list above runs **in parallel** and thereby uses all worker threads of the Tokio scheduler:

- **Between cells**: all cell tasks are independent. While cell A runs an LLM call, cell B can run a DB query in parallel, cell C an HTTP fetch, on up to N cores at once.
- **Fan-out**: if a cell emits a message and it has several matching outgoing edges in colony's edge table, colony dispatches in one routing step to all relevant handles; the receiver tasks continue independently in parallel.
- **Stateless cells**: the cell dispatcher task spawns a short-lived worker task per incoming message. Worker tasks have no persistent state and terminate after emit. A hundred parallel web fetches are a hundred worker tasks that the scheduler distributes over the worker threads. Concurrency limit per cell configurable via `params.max_concurrency` (see "Stateless cell dispatcher").
- **Long-running cells, from the I/O perspective**: the I/O task runs independently of the handler task. A 30s Telegram long poll does not block the processing of an incoming meclaw message.
- **Colony and cells**: colony processes routing decisions sequentially but hands messages to the receiver cells immediately; receiver processing runs in parallel across all worker threads.

### Long-running cells: double task

Cells of type `proxy`, `timer`, and `mcp` have a cell-internal double-task setup. From the topology perspective the cell remains a single address with a single external mailbox; the double structure is an implementation detail of these cell types, but **mandatory for these cell types**, not optional.

```
external mailbox (mpsc) ──►  [ Handler-Task ]  ◄── internal mpsc ── [ I/O-Task ]
                                  │                                      │
                                  │  holds cell state                    │  polls provider
                                  │  (cell.db, Schedules,                │  / waits on timer
                                  │  Cursor, Session-Handles)            │  / reads WebSocket
                                  │                                      │
                                  ▼                                      ▼
                            tokio::select!                       sends event frames
                            over both inputs                     into the internal mpsc
```

- **I/O task**: polls the provider (Telegram long poll, MCP stream), waits for the next cron firing via `tokio::time::sleep_until`, holds a WebSocket. On an event: serializes it to an internal event type and sends it via internal mpsc to the handler task. This task **never touches cell state directly** and **holds no `outputs_tx`**; it pushes exclusively into the internal channel, all topology-directed emissions run through the handler.
- **Handler task**: does `tokio::select!` over the external mailbox and the internal channel from the I/O task. Both sources are serialized into a single sequential stream in which one event is processed after another. **Cell state is mutated exclusively in this task**, no mutex needed, no lock negotiation. The handler holds the `outputs_tx` (cloned once at spawn) and calls `cell.handle(...).await` or `cell.handle_event(...).await`, which in turn call `outputs_tx.send`.

With this:
- Inbound from topology and outbound from the provider can never touch the same state at the same time (race-free from the cell's perspective).
- A long provider poll (30s long poll) does not block the mailbox; the handler is not trapped in I/O, the I/O task waits on its own.
- A long mailbox backlog only means that provider events pile up in the internal channel, not that the provider connection breaks down.
- **Output backpressure cascade**: a full `outputs` mailbox blocks the handler on `outputs_tx.send` → the internal channel fills up → the I/O task blocks on the push into the internal channel → external polling throttles itself (e.g. Telegram long-poll frequency). Backward propagation without additional mechanisms, liveness-safe below the saturation boundary (§ Backpressure → liveness boundary).

**Restart behavior**: if either the I/O task or the handler task panics, the entire cell is re-instantiated by colony's supervisor (one_for_one); both sub-tasks are set up anew together.

**`run_io` lifetime contract (A1′)**: the I/O task function (`run_io`) runs for the **entire lifetime of the cell**; it polls/waits endlessly and returns only when the cell as a whole ends (teardown: handler closing the internal channels, disconnect, or panic). A **clean, voluntary return from `run_io` while the cell is still alive is a contract violation**: it would shut down the I/O side while the handler keeps running, and it opens the latent "io-finish-first" loss class (the outer `select!` over both `JoinHandle`s could win on the I/O completion and abort the surviving handler sibling along with unprocessed events). No real cell (`proxy`/`timer`/`mcp`) triggers this today, all I/O loops are endless (`pending`/loop); the invariant explicitly forbids the class for future implementers.

### Bottleneck and solution

Colony's mailbox is single-consumer and sequential. At very high routing throughput it becomes a potential bottleneck. For the load tier foreseen within the roadmap horizon (LLM-centric flows with second-scale latencies per step) this is far below the limit. If performance later becomes a driver, the solution paths are additive: the routing table could become read-mostly, edge evaluation could be pulled out of the routing path, or multiple colony instances could serve different subtrees (cross-colony federation, post-roadmap). Currently: no optimization, the system stays simple.

### Backpressure

Bounded mpsc mailboxes (default 1000 per cell, from phase 1) plus `block` as the **only** strategy: when a mailbox is full, the sender waits (`send().await`) until there is room. With this, backpressure propagates backward through the graph, without silent message loss **on the live backpressure path** (the cell panic/restart path is excluded from this and loses waiting messages, see § Cell robustness), without explicit drop logic, without a per-cell strategy choice.

**Two backward cascades** exist symmetrically:

1. **Inbox backpressure**: a full cell mailbox makes the sender (colony during routing) block → colony drains its routing inbox more slowly → upstream cells that write to colony also block.
2. **Output backpressure**: each cell writes its emits into a central `outputs` mailbox (see "Output path"). A full outputs mailbox makes the cell block on `outputs_tx.send().await` → the cell drains its own inbox more slowly → see (1).

A fully dead cell is caught by message timeout + `one_for_one` restart (see "Cell robustness").

**Liveness boundary (honest state).** `block` backpressure is liveness-safe as long as inflow ≤ outflow (LLM-centric target profile: second-scale steps, far below the mailbox capacity of 1000). Under **sustained over-saturation** across a **closed wait chain** (cell A blocks on B, B on … on A, a cycle of `send().await` waiters) a **wait-cycle deadlock** arises: none of the senders gets free, the cycle stands permanently. Particularly exposed are backstop-less long-running source cells (`message_timeout` `0`/`-1`, no backstop, § Timeouts) in the cycle. **TTL does NOT catch this**; TTL counts routing hops (colony decrements per routing decision); in the deadlock no message flows, so TTL is never decremented. This case lies beyond the roadmap load profile and is registered as a post-MVP roadmap item.

For **stateless cells**, inbox backpressure plays out at the dispatcher; the dispatcher slows itself down via the `Semaphore` (see "Stateless cell dispatcher"), worker tasks themselves can only be stuck on output backpressure.

### What cell implementers do **not** have to consider

- No `Arc<Mutex<...>>` over cell state.
- No `RwLock` negotiation.
- No atomics or lock-free data structures.
- No reentrant calls into one's own `handle()`.
- No "what if two messages at the same time …" defense.

**From the cell's perspective the world is single-threaded.** The parallelism lies outside the cell, in the Tokio scheduler and in colony's routing distribution. Whoever writes `Mutex` in cell code has not understood the concurrency model; code review rejects it.

### What cell implementers **do** have to consider

- **`handle()` is async, all I/O via `.await`.** A synchronous `std::thread::sleep`, a blocking DB driver, or a synchronous network call blocks the Tokio worker thread and sabotages parallelism for all other tasks on that worker. Forbidden.
- **Long CPU bursts** (>1 ms without an `.await` point): explicitly insert `tokio::task::yield_now().await` or offload via `tokio::task::spawn_blocking`. Otherwise a CPU burst blocks other tasks on the same worker.
- **No `block_on`** in cell or colony code. Never. Not even in tests, when the test boots a real topology.
- **No assumption about ordering between cells.** Cell A and cell B emit in parallel; which message arrives first depends on the scheduler. Whoever needs ordering builds it via topology (collector hive with a correlation ID).

---

## Authority model

The colony is the only write authority in the system. Hives are scope markers in the filesystem, not own actors; they define authority and mutation boundaries for their subtree without themselves owning a mailbox or routing logic. They additionally act as logical transit nodes in the routing graph; their transit evaluation is **colony authority** (colony evaluates the hive out-edges, no hive-own evaluation code).

| Authority | Carrier |
|---|---|
| Reads/writes `config.json` (only on instantiation) | Colony |
| Instantiates cells (template copy, UUID assignment) | Colony |
| Holds the central `HashMap<Path, ActorHandle>` registry | Colony |
| Holds the mutation log (audit trail) in `colony.db` | Colony |
| Routes messages (O(1) lookup with upstream path resolution) | Colony |
| Lifecycle of all cells (start/stop/restart) | Colony |
| Templates registry | Colony |
| `.env` substitution at bootstrap | Colony |
| Central message log (filterable by path prefix) | Colony |
| Authority scope for mutations (path-prefix-based) | Hive (scope marker) |

**Hive as scope marker**: a directory with `config.json` `type: "hive"` marks a path prefix as an authority boundary for scoped mutations. There is **no** hive actor, **no** hive-own `cell.db`, **no** hive-own routing table. The DSL effect ("directory nesting groups cells into a unit") is preserved, the implementation is flat. Transit edges of a hive (edges with `from = <hive-path>`) live in **colony's one `EdgeTable`**, the same data structure as cell edges, indexed by `from`. Colony evaluates them as part of its one routing layer, no separate hive routing logic and no hive-own evaluator.

**Lifecycle of `config.json`**: colony writes `config.json` **exclusively on instantiation** (template copy with UUID assignment and `${VAR}` substitution). After instantiation it is never touched again, a bootstrap snapshot. (The re-dedicated `swap_nodes` graph swap does not rewrite an existing `config.json`; it swings edges, see § Mutation operations.) The live state of a cell lives in its `cell.db`. A global topology truth lives in colony's registry (in-memory) and in `colony.db` (persisted).

**Instantiation and cell_id stability**: instantiation happens **exactly when** no cell directory exists at the
target path. When processing a graph, colony checks, per declared node, whether the
directory exists in the tree, `params.graph` at bootstrap as well as a mutation diff at runtime: if it is missing, colony copies the referenced template to the
target path and thereby assigns a fresh UUID v7 as `cell_id` (plus `${VAR}` substitution);
if it exists, the operation is a **reconnect/resume**, no new `cell_id`, `config.json`
is not touched, `cell.db` is resumed (M1). Resume requires type equality; if the
`type` of the existing node deviates from the template, the mutation is rejected with `resume_type_mismatch`
(no silent resume). **`cell_id` is assigned exactly once and
never changed or reassigned afterward.** Templates themselves are id-less; IDs arise
exclusively when copying into the tree. On instantiation, colony records the node with
its `cell_id` in `colony.db`; entries there are never deleted, only marked inactive
(see § Connectivity and activity).

**Flow on graph mutation (EDA, no success ack)**:

1. Someone (cell, builder, external API) sends a mutation message to `/colony/mutations` with `hop.msg_type == "mutation"` and a diff (see "Mutation format"). The diff carries a path prefix as scope (typically the path of a hive scope marker). The sender assumes success.
2. Colony validates in a single stage: schema, match patterns against the current registry, cycle check in the post_state, edge schema compatibility, template existence, filesystem preparation, `.env` variables. On error: logging + reply to `reply_to` (if set). With a flat substrate, colony has all the information needed for single-stage validation (no old two-stage model anymore).
3. On success: mark the mutation as `in_flight` in `colony.db`, build all new cell directories under `{root}/.staging/<mutation_id>/<cell_name>/` (`config.json` with substituted values and assigned UUIDs, possibly `cell.db` from seed), then move them sequentially via `rename(2)` to the final paths (atomic per directory on POSIX), atomic per directory but **NOT transactional across all directories**: if a `rename(2)` fails after others have already succeeded, the earlier renames stand in the live tree (audit model, no rollback). The substrate handles this half-state loudly (strict-fail, § Validation) instead of papering over it as a clean reject. Registry edits are executed: new cells spawn and are registered under their path, disconnected cells are marked inactive, the registry entry and the filesystem remain, the tasks end gracefully (see "Connectivity and activity"). Then mark as `committed` in `colony.db`. On a crash between `in_flight` and `committed`: recovery pass at the next startup (see "Startup algorithm"). For the rationale of the staging pattern see "Filesystem layout" → `.staging/`.
4. Cell inits run asynchronously. On init failure: restart one_for_one (N retries, default 5), then `failed` status. Symptoms visible via the routing cascade (`reply_to` → `/colony/dead_letters`). **No** success reply to the builder.

**Cross-colony federation** (several `meclaw` instances with different colonies that communicate with each other) is post-roadmap. The architecture, colony as authority unit, unique paths within a colony, will not prevent it, but does not implement it now.

---

## Graph schema

The graph of a meclaw colony is a directed graph of **nodes** (= cells, registered in colony's `HashMap<Path, ActorHandle>`) and **edges** (= routing rules between paths, held in colony's edge table). Hive scope markers are **not cells and not actors**, no mailbox, no `RegistryEntry`. But they are **logical transit nodes in the routing graph**: their paths can be the `from`/`to` endpoint of an edge, and colony evaluates them as part of its one routing layer (see "Hive paths as target: transit evaluation").

The same schema describes the graph in two write usages:

1. **Bootstrap**: `params.graph` in the `config.json` of a hive scope marker provides the initial desired state for its subtree on first instantiation. Colony reads this at the filesystem bootstrap (see "Startup algorithm") and registers the declared cells.
2. **Runtime diff**: a builder sends a mutation message with a diff to `/colony/mutations` (see "Mutation format"). Colony computes the post_state from it and executes scoped registry edits.

### Schema

```json
{
  "nodes": {
    "<name>": {
      "template": "<template-ref>",
      "override_params": { ... }
    }
  },
  "edges": [
    {
      "from": "<path-relative-to-scope>",
      "to":   "<path-relative-to-scope>",
      "condition": "<CEL-Boolean, default true>",
      "modifier":  { "set_context": { "<key>": "<CEL>" }, "delete_context": ["<key>"], "set_hop": { "<key>": "<CEL>" }, "delete_hop": ["<key>"] }
    }
  ]
}
```

**`nodes`**: mapping `name → { template, override_params? }`. The name is the path component and must be unique within the same hive scope; collisions are rejected during mutation validation. `template` is a template reference in the form `<name>` (highest available version) or `<name>@<version>` (see "Resolution `name@version`"). `override_params` is optional and overlays the default params of the template.

**`edges`**: a list; order irrelevant. Required fields: `from`, `to` (paths relative to the hive scope in which the schema is declared). The paths may lie at **any depth** within the scope (`./name` as well as `./unit/dispatch`); a depth restriction does not exist; this holds symmetrically in the bootstrap `params.graph` path and in the mutation diff (R12 ruling 2026-06-11). Optional: `condition` (CEL boolean, default `true` = always matches), `modifier` (operations object with `set_context`/`delete_context`/`set_hop`/`delete_hop`, default `null` = identity, see "Edge model" for schema and example). Edges operate strictly on the header layer.

**Shape difference between write side and read side**: in the write schema (bootstrap and mutation diff) edges reference nodes by name (`./<name>`). UUIDs arise only at runtime; node UUIDs are assigned by colony on instantiation, edge UUIDs by colony on creation. The read schema (`/colony/graph?scope=...` or HTTP `GET /graph?scope=...`, see "Visibility / read paths") additionally shows `id`, `path`, `graph_version`, etc.; the read side is the runtime-projected form of the same structure.

---

## Mutation format (builder → colony)

A mutation is a message to `/colony/mutations` with `hop.msg_type == "mutation"` whose body carries a **diff** plus a **scope**. Colony validates in a single stage, executes, replies to `reply_to` on error. Entry paths today: the HTTP edge (phase 12, direct translation) and the internal bootstrap inbox command. A message emitted by a cell to `/colony/mutations` is **dispatched directly** (W2b ruling, Marcus 2026-06-12): the outputs arm recognizes a `/colony/*` target and routes it BEFORE edge evaluation via `route()` to the virtual endpoint (see § Routing errors "Outputs arm: three disjoint cases", case 1), no out-edge needed/possible. Cell-emitted mutations/reads (EDA) are thereby a first-class delivery path; an unknown `/colony/<x>` endpoint lands in the DLQ as `colony_endpoint_unimplemented`.

```json
{
  "scope": "/main/agent-pool",
  "diff": {
    "add_nodes":    [ { "name": "...", "template": "...", "override_params": {} } ],
    "remove_nodes": [ { "match": { ... } } ],
    "add_edges":    [ { "from": "...", "to": "...", "condition": "...", "modifier": { "set_context": {}, "delete_context": [], "set_hop": {}, "delete_hop": [] } } ],
    "remove_edges": [ { "match": { ... } } ],
    "swap_nodes":   [ { "match": { ... }, "with": { "template": "..." } } ]
  },
  "ctx": { "key": "value" }
}
```

**`scope`** is an absolute path prefix, typically the path of a hive scope marker. All relative paths in the diff are resolved against this scope. Mutations whose paths would lie outside the scope are rejected during validation.

**`diff`** contains the change operations. Order irrelevant; colony computes the post_state after applying all operations and validates _that_, not partial states. Thereby an `add_edges` edge may point to a node that newly arrives in the same diff via `add_nodes`.

**`ctx`** provides values for `${ctx.<key>}` substitutions in the diff (see "Variable substitution"). It is resolved when applying the diff, before validation runs.

### Mutation operations

| Operation | Effect |
|---|---|
| `add_nodes` | Instantiate new cells in the scope (template reference, optional `override_params`). `override_params` applies only to single-cell templates; on subtree templates it is rejected (`schema`; R10 ruling 2026-06-11): a sub-cell addressing does not exist, sub-cell parameterization runs via `${ctx.*}` substitution in the template. |
| `remove_nodes` | Removes all edges in which the referenced nodes participate → the nodes are disconnected and marked inactive (including subtree cascade at hives). Registry entry, filesystem, and `cell_id` remain (no-delete, see "Connectivity and activity"). |
| `add_edges` | New edges in colony's edge table, scoped |
| `remove_edges` | Remove edges from the edge table, scoped |
| `swap_nodes` | **Graph swap**: swings **all external edges** of an implementation (`match`) atomically onto another (`with`), the other being either freshly instantiated from a template **or** an already existing cell. The old cell remains **disconnected and preserved** (no-delete policy; swappable back at any time by swinging the edges back). `swap_nodes` is thereby a pure edge/topology diff, **no** `config.json` rewrite of an existing cell, **no** `cell.db` migration, **no** `cell_id` takeover (the new implementation has its own identity), and inherits the atomicity model of the edge mutation. |

**Match pattern for `remove_*` and `swap_nodes`**: a pattern references nodes/edges by properties (`name`, `template`, for edges `from`/`to`/`condition`/`modifier`), **not by UUID**. The pattern must have at least one hit in the current registry, otherwise the mutation is rejected. Names are unique per scope (naming-collision reject in validation); a UUID reference as a disambiguation fallback is **not** provided.

### Validation

Single-stage in colony. Before application, the hypothetical post_state is computed and checked against the following criteria:

- **Schema**: the diff conforms to the JSON schema (see `docs/config.md` for details of the individual operations).
- **Match patterns**: each pattern in `remove_*`/`swap_nodes` hits ≥1 element in the pre_state.
- **Naming uniqueness**: no two nodes have the same name within the same scope after applying the diff.
- **Cycle freedom**: the post_state graph has no cycles over `from`/`to` edges (insofar as the application forbids cycles; meclaw-core does not generally reject on cycles).
- **Edge schema compatibility**: all edges reference existing nodes in the post_state; `condition` parses as valid CEL; `modifier` (if set) conforms to the `{set?, delete?}` schema, and all expressions in `modifier.set.*` parse as valid CEL. Edge endpoints resolve relative to the mutation `scope` at **any depth within the scope** (`./name`, `./unit/dispatch`), against the post_state, diff-new nodes included (also subtree nodes at depth). Containment stays sharp: endpoints that resolve outside the scope (`../x`, absolute paths) are `scope_out_of_bounds` (§ Scope), the parent wires downward into its own subtree, never out. A depth path to a non-existent node is `edge_schema`.
- **Template existence**: all `add_nodes`/`swap_nodes` reference templates that exist in colony's templates registry.
- **`.env` variables**: all `${ENV_VAR}` in the `override_params` have values in `.env`.

On an error **before** the atomic rename phase (schema, match, cycle, edge schema, template, `.env`, staging build): the entire diff is rejected, no partial commit, the live tree untouched. **From** the rename phase onward the audit model applies: an error after the first successful `rename(2)` is no longer a clean reject (earlier renames already stand in the live tree); the substrate strict-fails loudly (panic), and the half-state is made visible at the next boot as non-registered orphan dirs (§ Startup algorithm), never silently adopted. Error message to `reply_to` (if set) with:

```json
{
  "error_code": "<code>",
  "details": "<human-readable>",
  "context": { ... }
}
```

`error_code` is an enum: `schema` | `match_no_hit` | `naming_collision` | `cycle` | `edge_schema` | `template_missing` | `env_var_missing` | `unsupported_substitution` | `ctx_key_missing` | `scope_out_of_bounds` | `unknown_cell_type` | `stop_wiring_unavailable` | `term_timeout` | `resume_requires_stopped_cell` | `subtree_resume_unsupported` | `resume_type_mismatch` | `contract_incomplete`.

These strings are part of the stable mutation API contract. Notes on the substrate codes:

- `ctx_key_missing`: a `${ctx.<key>}` substitution in the diff references a key that is missing in the `ctx` block of the mutation (see § Variable substitution → `${ctx.<key>}`). Emitted by `resolve_ctx_token` (`mutation/substitute.rs`).
- `scope_out_of_bounds`: a top-level diff path (`add_nodes[].name`, `*_edges[].from`/`.to`, `match.name`) resolves outside the mutation `scope` (see § Scope). Scope containment check before any FS/registry mutation (pre-14 audit B4); emitted by `validate_scope_containment` (`mutation/validate.rs`).
- `unknown_cell_type`: `add_nodes`/`swap_nodes` references a cell type without a registered factory.
- `stop_wiring_unavailable`: disconnect/swap of a cell whose stop wiring is not restorable after a term_timeout survivor (F5 guard, permanent backstop).
- `term_timeout`: death-ack timeout on disconnect/swap of an awake cell → full rollback + reject.
- `resume_requires_stopped_cell`: the resume path requires a stopped cell.
- `subtree_resume_unsupported`: subtree template at an already occupied root path. No producer today; the earlier F4 reject was superseded by per-node resume (paket-5 T12, commits `d280de4` validate-phase per-node subtree resume + `549422d` producer removal); the enum string remains reserved.
- `resume_type_mismatch`: resume (single-cell as well as subtree) at an occupied path whose existing `type` deviates from the template (F2 ruling, paket 5).
- `contract_incomplete`: a `config.json` to be loaded (boot walk or mutation staging, non-hive) does not declare the required keys `contract.version`/`settings`/`consumes`, or declares them type-wrong (`docs/config.md` § contract).

`uuid_provider_exhausted` is **not** live code: the enum variant `MutationError::UuidProviderExhausted` was dead code (`Uuid::now_v7()` is infallible, never constructed; additionally mapped as the string `"uuid_provider"` instead of `"uuid_provider_exhausted"`) and **was removed with paket 7** (D-034; verified 2026-06-10: 0 code hits). The note remains as re-discovery protection.

### Scope

A mutation covers **one** scope (= one path prefix). Sub-scopes (= nested hive markers in the filesystem) are mutated via their own mutation messages to `/colony/mutations` with their own scope path. A single mutation cannot address several scopes at once; this keeps mutations local and race-free.

### No concurrency protection (CAS)

In the current phase no concurrent builders per scope are expected, no `expected_version` or similar. If this becomes relevant later (post-roadmap), it can be retrofitted additively. Currently: colony's sequential mailbox processing serializes concurrent mutations automatically.

### Permissions

Until hardening: **no permission layer**. Whoever can deliver a mutation message to `/colony/mutations` routing-wise can mutate. Permission is a topology question, not an identity check. The `mutate-graph` capability in the cell contract is a **discovery hint** (for the builder composer and audit tools), not a runtime check. HTTP API in phase 12 without auth, protection via the `--api <bind>` flag (default: no port; opt-in via e.g. `--api 127.0.0.1:7777` for local-only).

---

## Architecture building blocks

| Term | Description |
|---|---|
| **Colony** | The overall system and the only authority. Holds the central `HashMap<Path, ActorHandle>` registry, routes all messages, manages lifecycle, templates, `config.json`. Has path `/colony`. Runs as its own Tokio task with its own mailbox. |
| **Hive** | Directory with `config.json` `type: "hive"`. **Scope marker** for the authority boundary and mutation scope of a path prefix, no own actor, no mailbox, no own `cell.db`. Additionally acts as a **logical transit node** in the routing graph, evaluated by colony. The hierarchy effect in the DSL remains; the implementation is flat. |
| **Cell type** | Behavioral classification of an addressable cell: `llm`, `bash`, `code`, `store`, `web_fetch`, `web_search`, `file`, `edit`, `proxy`, `timer`, `mcp`. Each cell type brings its own `params` schema and capability set. Cells with one of these values are kept by colony as actors in the `HashMap<Path, ActorHandle>` registry. |
| **Cell** | Directory with `config.json` of a particular cell type. Topologically neutral, the role follows from the location (template or instance). |
| **Hive scope marker** | Directory with `config.json` `type: "hive"`. **Not an actor**: no Tokio task, no mailbox, no `cell.db`, no `ActorHandle` entry in the cell registry. Nevertheless a **junction in the system**: colony keeps a separate hive scope table (path prefix, authority boundary, mutation scope, initial `params.graph`). At filesystem bootstrap the hive marker is recorded; on mutations it acts as a scope boundary. **Addressable as a transit target**, colony forwards based on the hive out-edges, never delivers (see "Hive paths as target: transit evaluation" and `cell-types.md` section `hive`). |
| **Template** | A cell (or cell subtree including hive scope markers) in the `templates/` folder. Role: class / blueprint. Copied on instantiation. |
| **Instance** | A cell in the directory tree (path freely choosable). Role: living object with a path, UUID, own Tokio task, possibly `cell.db`. Recorded in colony's cell registry under its path. |
| **Edge** | Connection between a cell output and the next input. Carries a condition + modifier. Lives in the graph (colony's edge table). Has a UUID v7 (colony-assigned). |
| **Graph** | The set of all nodes (= cells in the registry) and edges. Lives entirely in colony's registry (in-memory) and `colony.db` (persisted). Initial state from the filesystem bootstrap + `params.graph` hints from hive scope markers. Dynamically changeable via mutations. |
| **Message** | The unit of communication. Atomic, small. Carries routing data + headers + body reference. |
| **Blob** | Large message body. Stored separately in the `blobs/` directory, referenced by UUID v7. |
| **Path** | Address of an instance. Linux-style: `/`, `.`, `..`. Plus `/colony` as a virtual endpoint. Path resolution is a pure string operation before the registry lookup. |
| **Session** | Application convention for a logical conversation bracket (typically propagated via the `session_id` header). Not a core concept, meclaw-core knows no sessions, applications choose their own granularity. |
| **Seed** | JSONL file per cell DB, schema in line 1, data after. Source for the DB bootstrap. |

---

## Filesystem layout

```
{root}/
├── colony.json              # colony-wide behavior defaults (optional)
├── colony.db                # SQLite: registry, templates, mutation log, central message log
├── log.jsonl                # tracing JSONL
├── .env                     # secret substitution source
├── .staging/                # atomic mutation staging (see below)
│   └── <mutation_id>/
├── blobs/                   # blob storage
│   └── <uuid7>.json
├── templates/               # template library (classes)
│   └── <template_name>/
│       ├── template.json
│       ├── config.json
│       └── seed/
│           └── <table>.jsonl
└── <root-cell>/             # root cell (usually a hive scope marker), path `/`
    ├── config.json
    ├── cell.db              # only if the cell is stateful
    ├── seed/                # optional
    │   └── <table>.jsonl
    └── <sub-cell>/          # further cells in the subtree
        └── ...
```

**No prescribed `main/sessions/archived/` separation.** Paths are chosen deliberately by the trigger of instantiation (builder, CLI, API). Common conventions arise from the application logic, not from the core.

**`.staging/`**: temporary directory for mutations that stand between validation and commit. Colony builds new cell directories here completely (with substituted `config.json` values and possibly `cell.db` from seed), then a single `rename(2)` to the target path, atomic per directory on POSIX. Advantages: broken half-instantiations cannot lie in the live tree, recovery at startup is simple (everything in `.staging/` without a commit marker → delete). Rejected were: direct writing at target paths with backup files (does not solve the half-instances problem) and a `.tombstones/` directory for deleted cells (the no-delete policy makes that superfluous).

**Every cell instance** has the same structure: a directory with `config.json` (bootstrap snapshot), optionally `cell.db` (live state), optionally `seed/`. Sub-cells as further directories within, insofar as the cell type permits it (common for `hive` scope markers, not for other cell types).

---

## `colony.json`: schema

The colony-wide configuration file in `{root}`. Contains exclusively **behavior defaults for cells and colony**, not operations configuration (paths, logging, those remain CLI flags, see "CLI"). The separation is consistent with the nginx-style philosophy: per-run operations go via flags, behavior defaults live in the file.

```json
{
  "schema_version": 1,

  "mailbox_default_capacity": 1000,
  "message_timeout_default_ms": 60000,
  "idle_timeout_default_ms":   60000,
  "message_default_ttl":          64,
  "restart_max_retries":           5,

  "blob_inline_max_bytes":         65536,
  "blob_max_recursion_depth":      64,

  "strict_validation":          false,

  "log_default_level":          "info"
}
```

**Keys**:

| Key | Meaning |
|---|---|
| `schema_version` | Version marker for migration compatibility |
| `mailbox_default_capacity` | Default capacity of the **regular cell mailboxes** (bounded mpsc); overridable per cell via `cell.mailbox_size`. **Shadows only the regular cell mailbox** (AMBIG-001 ruling B, 2026-06-06); the dead-letter queue and disconnect mailbox capacities are **fixed constants** and are **not** overridden by this field. |
| `message_timeout_default_ms` | Default for the **substrate backstop** per `handle()` call (concept B, see "Timeouts"). On exceedance: the cell task is killed, the supervisor restarts. **Not** the primary I/O protection; `params.external_timeout_ms` (concept A) is responsible for that. The value should be considerably more generous than the longest expected I/O operations. Overridable per cell via `cell.message_timeout`. |
| `idle_timeout_default_ms` | Default for the idle duration per stateful cell with `cell.timeout: 0`; after this time without a new message the cell despawns itself (Awake→Asleep, see "Hot/cold cell model"). Overridable per cell via `cell.idle_timeout_ms` in `config.json`. The value takes effect only from phase 13. |
| `message_default_ttl` | Default TTL for source messages (protective limit against routing loops). Colony decrements per routing hop; at `0` the message goes **directly** into the dead-letter queue (`ttl_expired`, direct-to-DLQ, no step-1 `reply_to` reply attempt as with the routing-error cascade; see "Routing algorithm"). Builders can set the value per initial message. Recommendation: 64. |
| `restart_max_retries` | Maximum number of `one_for_one` restarts per cell before `failed` status. **This `colony.json` field is parsed-but-not-applied today:** the effective cap comes from the substrate constant `DEFAULT_RESTART_LIMIT` (5), overridable per cell via `config.json` `cell.restart_limit`; the `colony.json` wiring of this field is post-16. |
| `blob_inline_max_bytes` | Threshold above which a body is offloaded as a blob (smaller bodies stay inline in the message) |
| `blob_max_recursion_depth` | Hard limit for recursive blob reference resolution (see "Blob storage"). **The `colony.json` override of this field is parsed-but-not-applied today**; recursive blob resolution itself is roadmap defer D-025 (0 producers, § Routing errors `blob_recursion_too_deep`); the depth limit **and** its `colony.json` wiring arise with D-025. |
| `strict_validation` | Release-build default: whether JSON schema validation against `emits`/`consumes` is active (debug build: always `true`) |
| `log_default_level` | Tracing default level. **This `colony.json` field is parsed-but-not-applied today:** the effective default comes from the `--log-level` flag or `info`; `colony.json` does not (yet) feed the log level; the wiring is post-16. |

Cells can override individual values via their `config.json` `params` or their `contract.settings`; then the local value applies.

**What deliberately is not in `colony.json`**: paths (`--templates`, `--blobs`, `--env`, `--log`) and logging configuration (`--log-level`, `--log-filter`) remain CLI flags. Rationale: per-run operations (e.g. test roots, alternative blob paths, debug-logging sessions) should not intrude into colony's own configuration. Rejected were: `colony.json` as a required file (friction without added value for quick start), mirroring all CLI flags in `colony.json` too (duplicate configuration sources with merge and conflict-resolution needs), per-scope configuration in `colony.json` (the file is colony-wide; per scope we can retrofit post-roadmap if a real need arises).

---

## `/colony` as a virtual endpoint

`/colony/*` are paths not existing in the filesystem tree, but **virtual endpoints** that colony itself handles. They are built into colony's routing algorithm: every path beginning with `/colony/` is read by colony as an internal operation, not as a registry lookup.

**Symmetry between internal API and external API**: every `/colony/<endpoint>` is at once a **message target** for internal senders (cells, builder, routing) and an **HTTP route** for the external API (phase 12, axum layer). The HTTP layer is a **thin translation layer** that converts an HTTP request into a `Message` with `target = "/colony/<endpoint>"` and sends it through the same routing path (internally: translation into the typed `ColonyMsg::{Mutation, Read*, …}` inbox variant with a oneshot-ack reply; **symmetry = same colony-task sequence + same UBF data model, not literal `route()`**; cell→`/colony/*` routing implemented since W2b, the outputs arm dispatches `/colony/*` targets directly, see § Routing errors "Outputs arm: three disjoint cases"). With this the HTTP API is not its own sub-system with its own endpoints, but a second way of writing/reading for the existing internal endpoints. The OpenAPI spec (via `utoipa`) and the internal routing table share the definition.

| Path | Purpose | Filter / query parameters | Writing? | Phase |
|---|---|---|---|---|
| `/colony/dead_letters` | Dead-letter queue: unresolvable routes, expired TTLs, routing errors | `?since=<ts>`, `?limit=<N>`, `?error_code=<code>` | both (read + drain) | 2 |
| `/colony/registry` | Read the cell registry (list of all registered cells with paths, IDs, types, status). `?path=` for a single cell. Including inactive nodes with the `active` field. | `?path_prefix=<path>`, `?type=<celltype>`, `?path=<exact>`, `?active=true\|false` | no | 4 |
| `/colony/templates` | Read the templates registry (for builder discovery) | `?type=<celltype>`, `?name=<name>` | no | 5 |
| `/colony/templates/rescan` | Trigger to re-read the templates directory (replacement for a `--rescan-templates` restart) | — | yes | 5 |
| `/colony/mutations` | Mutation pipeline; builders send mutation diffs here | — (diff in the body) | yes | 6 |
| `/colony/graph` | Read the topology of a scope (nodes + edges, runtime-projected) | `?scope=<path>` (default root) | no | 6 |
| `/colony/trace` | Read the message log, built as a tree by `parent_message_id` when `trace_id` is set | `?trace_id=<uuid>`, `?path_prefix=<path>`, `?correlation_id=<uuid>` *(inert today, `correlation_id` is not originally set, see § Envelope setter authority)*, `?error=true`, `?since=<ts>`, `?limit=<N>` | no | 11 |
| `/colony/messages` | Browse the message log: newest-first list with filters + single message | `?id=<uuid>`, `?trace_id=<uuid>`, `?parent_message_id=<uuid>`, `?correlation_id=<uuid>`, `?to_path_prefix=<path>`, `?from_path_prefix=<path>`, `?body_kind=inline\|blob`, `?since=<ts>`, `?until=<ts>`, `?before_created_at=<ts>&before_id=<uuid>` (keyset cursor), `?limit=<N>`, `?scan_budget=<N>`, `?resolve_blob=true` | no | P1 |
| `/colony/events` | Subscribe to a live event stream (routing decisions, mutation commits, restarts, dead letters) | — (subscription-style) | no | 14 |

`/colony` itself (without a sub-path) is not addressable; requests there → error. `/colony/cell` does not exist as its own endpoint; individual cells are read via `/colony/registry?path=<path>`.

**Reply body form (reads):** colony answers in the universal body format with a top-level slot named after the endpoint: `registry`, `dead_letters`, `templates`, `trace`, `messages`, `mutations` (audit read), `rescan` (outcome). Analogous to the `graph` slot (see "Visibility").

**Request body form (cell emissions / EDA):** a cell that emits to a `/colony/*` endpoint carries the endpoint-specific call as a top-level slot in the UBF body:

- **`/colony/mutations`**: top-level `{ scope, diff, ctx }`: the mutation diff plus the `scope` plus an optional `ctx` substitution context (for the canonical form see § "Mutation format (builder → colony)"). The only writable EDA endpoint.
- **`/colony/registry`, `/colony/templates`, `/colony/graph`, `/colony/trace` (reads)**: top-level `{ query: { … } }`: a `query` object whose fields correspond to the HTTP query parameters of the endpoint (`registry`: `path`/`path_prefix`/`cell_type`/`active`/`limit`; `templates`: `cell_type`/`name`/`limit`; `trace`: `trace_id`/`path_prefix`/`correlation_id`/`only_error`/`since`/`limit`; `graph`: `scope`). If `query` or a single field is missing, the defaults apply (`limit` default 100, hard cap 1000). The read reply goes to the sender path (reply body form above).
- **`/colony/dead_letters`**: **not** EDA-dispatchable and **not** body-operation-controlled: read vs. drain is decided by the HTTP method (`GET` = read / `DELETE` = drain) or the dedicated `ColonyMsg::ReadDeadLetters`/`DrainDeadLetters` inbox variant, **no** `body.operation` field (state W2d/W6d; the earlier `body.operation == "drain"` model is superseded). A cell emission to `/colony/dead_letters` is hard-rejected (see endpoint classification below).

**Endpoint classification for cell emissions (EDA, W2d ruling Marcus 2026-06-12):** the outputs arm dispatches a `/colony/*` emission target directly (§ Routing errors "Outputs arm: three disjoint cases", case 1), but not every endpoint is reachable from a cell:

- **`/colony/mutations`: EDA-writable.** A cell-emitted mutation is executed (13.5-A6). The only writable dispatch endpoint.
- **`/colony/registry`, `/colony/templates`, `/colony/graph`, `/colony/trace`: read-only, EDA-readable.** A cell may *read* them via emission (reply to its path); they are not writable.
- **`/colony/dead_letters`: read-only, NOT EDA-dispatchable.** The DLQ is read/drained exclusively via the dedicated inbox variants `ColonyMsg::ReadDeadLetters`/`DrainDeadLetters` (HTTP `GET`/`DELETE`), **never** via a routing dispatch. An emission to `/colony/dead_letters` is therefore always an illegitimate write to a READ endpoint: it is **hard-rejected** (one `colony_endpoint_unimplemented` DLQ entry, sender pass-through, terminal), **never** re-injected as a read reply. This prevents the source loop that the pre-W2d hardcoded fallback `unwrap_or("/colony/dead_letters")` triggered at the atomic-emitting cell types (DLQ-listing reply back to the emitting cell → re-emission, ttl-uncapped).
- **`/colony/messages` — read-only, NOT EDA-dispatchable (P1 ruling 2026-08-07, analogous to `dead_letters`).** Operator read surface over the message log, reachable exclusively via the dedicated inbox variant `ColonyMsg::ReadMessages` (HTTP `GET`), **never** via a routing dispatch. A cell emission to `/colony/messages` is hard-rejected like `dead_letters`. If topologies ever need message queries, that is a separate design pass over the `store`, not over colony endpoints.
- **Unknown `/colony/<x>`** ⇒ `colony_endpoint_unimplemented` (as before).

**`?limit=<N>` defaults** (for `dead_letters`, `trace`, `messages`, `mutations` audit read): default **100**, hard cap **1000**, **no config knob** (no speculative spec surface). The cap also brakes the routing-loop stall of DB-heavy reads. **`?scan_budget=<N>` (`messages` only):** upper bound on the rows read in stage 1 of the two-stage query (indexed predicates first, residual filters `from_path_prefix`/`body_kind`/`correlation_id` afterwards) — default **5000**, hard cap **50000**; an exhausted budget is reported in the reply as `scan_truncated` (result possibly incomplete, never silently).

---

## CLI

```
meclaw [options]
```

### Modes

The default mode is **direct mode**: a stdin/stdout bridge to the root cell, all on a single `meclaw` invocation. For interactive sessions, simple pipes, tests. The process is **stdin-driven**; closing stdin (EOF, e.g. the end of a pipe) drains the in-flight work and exits with exit code 0 (Unix pipe semantics, `cat input.jsonl | meclaw` terminates like `grep`). A shutdown signal (SIGINT/SIGTERM) acts additionally.

`--daemon` (from phase 12): **decouples the process lifecycle from stdin**; stdin EOF no longer ends the process; the only shutdown triggers are SIGINT/SIGTERM (and the internal watchdog). That is the meaning of "daemon": a long-running process that does not hang on its input pipe. The stdin/stdout bridge is **not switched off** in the process; the mechanism is preserved, but in daemon operation it typically runs empty (systemd `Type=simple` provides stdin as `/dev/null` → immediate EOF without input, stdout into the journal), analogous to nginx, whose stdio exists in daemon mode but remains unused. meclaw does **not** daemonize itself (no `fork`/`setsid`); that is systemd `Type=simple`-conformant; backgrounding is the outside world's business (systemd/nohup). External control runs via the HTTP API + web UI, both opt-in via `--api`.

`--api <bind>` (from phase 12): activates the HTTP API and the operator web UI on the given bind address, e.g. `--api 127.0.0.1:7777` (local-only) or `--api 0.0.0.0:7777` (all interfaces). **Without `--api` no port is opened**; the default is API/UI off. The HTTP API is a thin translation layer over the `/colony/*` endpoints (see "/colony as a virtual endpoint"): each HTTP request becomes a `Message` with `target = "/colony/<endpoint>"` and is sent through the same routing path. The web UI sits on the same bind port under `/ui/*` (see "Web UI" below). `--api` can be set independently of `--daemon` (direct mode + parallel API allowed).

`--validate` (from phase 12): dry run, filesystem bootstrap, schema checks, template resolution, mutation replay from `colony.db`, but no cell spawns, no HTTP listen. Exit code 0 if everything is consistent, otherwise an error list on stderr.

`--rescan-templates`: rebuilds the templates registry from the filesystem. Default: templates are scanned at the first startup and persisted in `colony.db`. If you have edited `templates/` manually (add/remove), run `--rescan-templates` once.

### Flags

**Flags are introduced phase by phase.** At any point `clap` knows only the flags of the already completed phases; unknown flags are rejected with an unknown-flag error. `meclaw --help` thus shows the respective functional CLI surface without misleading "accepts-but-does-nothing" flags. The phase column below states in which phase a flag is first declared and becomes functional in `clap`.

| Flag | Phase | Default | Meaning |
|---|---|---|---|
| `--root <path>` | 0 | `.` | Filesystem root of the colony |
| `--log <path>` | 0 | `<root>/log.jsonl` | Tracing JSONL path |
| `--log-level <level>` | 0 | `info` (`colony.json log_default_level` is not consulted today) | Tracing level |
| `--log-filter <filter>` | 0 | none | `RUST_LOG`-style filter |
| `--version` | 0 | — | Version info |
| `--help` | 0 | — | Help |
| `--env <path>` | 6 | `<root>/.env` | `.env` file for variable substitution |
| `--templates <path>` | 11 | `<root>/templates` | Templates directory |
| `--rescan-templates` | 11 | off | Rebuild the templates registry |
| `--blobs <path>` | 12 | `<root>/blobs` | Blob storage directory |
| `--daemon` | 12 | off | Lifecycle decoupled from stdin, shutdown only via signal/watchdog, stdin EOF does not end (the bridge mechanism remains) |
| `--api <bind>` | 12 | off (no port) | HTTP API + web UI on bind address; e.g. `127.0.0.1:7777` or `0.0.0.0:7777` |
| `--validate` | 12 | off | Dry run |

Deliberately **no own subcommands** (`meclaw start`, `meclaw mutate`, etc.). nginx-style: one binary, many flags, one mode switch (`--daemon`, `--validate`). Operations are the outside world's business (systemd, a wrapper script, a builder LLM).

**Info-only flags are side-effect-free**: `--version` and `--help` print their information to stdout and exit with 0, without initializing the tracing subscriber, without filesystem writes (in particular no `log.jsonl` creation), without subprocess spawn. They act before the subscriber setup. Tests for the subscriber setup path happen via direct unit tests of the setup function, not via CLI subprocess calls.

### Web UI (operator inspection)

`--api <bind>` activates, besides the JSON API, an **operator web UI** on the same port, under the path prefix `/ui/*`. Root `/` redirects to `/ui/`. The web UI is:

- **Server-rendered HTML** via `maud` (see tech stack). No JavaScript, no auto-refresh, no CSS framework, browser `F5` is the refresh button.
- **Read-only**. No mutate forms; mutations are a builder-hive matter (see "Dynamics / builder pattern"), which in turn uses the JSON API or internal routes.
- **Symmetric to the `/colony/*` endpoints**: the same data, different rendering. A web UI route internally calls the same read endpoint as an API route.

| Web UI path | Content | Data source |
|---|---|---|
| `/ui/` | Dashboard: cells overview, status counts, latest errors, latest dead letters, **no consistent snapshot (three independent reads from three moments)** | aggregated from `/colony/registry` + `/colony/dead_letters` + `/colony/trace?error=true` |
| `/ui/registry` | Table of all cells with a filter form (path prefix, type) | `/colony/registry` |
| `/ui/graph` | Topology of a scope (nodes as a list, edges as a table), form: `?scope=` | `/colony/graph` |
| `/ui/dead_letters` | List of the most recent dead letters with `error_code`, path, body preview, "Original" link to the originating message in the message browser (where present in the `message_log`) | `/colony/dead_letters` |
| `/ui/trace` | Trace search form (`trace_id`, `path_prefix`, `error`, `since`, `limit`), result as tree HTML by `parent_message_id` | `/colony/trace` |
| `/ui/templates` | Template overview with filter `?type=` | `/colony/templates` |
| `/ui/messages` | Message list newest-first with filter form + keyset paging, truncated payload, scan-budget disclosure | `/colony/messages` |
| `/ui/message` | Single message: `hop`/`context` headers rendered separately, payload pretty-printed, blob on demand, pivots (trace, parent chain, correlation, `reply_to`, dead letters) | `/colony/messages?id=` |

Auth (once phase-12 hardening): uniform middleware in front of `axum`'s router, applies to the JSON API and the web UI alike. Until then: local discipline (`--api 127.0.0.1:7777` as the safe default).

### Stdin/stdout bridge (direct mode)

In the default mode a stdin/stdout bridge runs: stdin is converted into messages to the root cell (one line = one message), stdout shows messages emitted from the root cell. **The v0.1.0 format is text** (grep/Unix-conformant, structurally identical to `proxy`): one stdin line of raw text → a UBF body with exactly one `user` turn (`{messages:[{origin:"user",type:"text",text:"<zeile>"}]}`) plus a fresh `turn_id`; to stdout the `text` of the last `assistant` turn of an emitted message is written (analogous to the `proxy` inbound). A structured JSON line format (full headers/body/`ttl`, selectable by flag, cf. HTTP `POST /messages`) is **post-v0.1.0**.

**Discipline**: the bridge is an I/O detail of the `meclaw-cli` crate, **not a cell type**. The root cell stays exchangeable (hive scope, llm cell, builder, …) without bridge code needing adjustment. Rejected were: an own `stdio` cell type (it would break the "cells know no topology" discipline, because the cell would implicitly know it is hanging on a stdin endpoint, and additionally an unnecessary entry in the cell-type catalog) as well as "interactive use only via `proxy` or HTTP API" (contradicts the self-description "brings the LLM into the Unix shell").

**Topological form (ingress/egress like a `proxy` cell)**: the bridge plays the (in the substrate non-existent) parent hive of the root; it shoves messages between the stdio level and the `/` level, exactly like a hive between its levels, only with JSON↔message translation. The root cell **must** therefore be a hive (`type: "hive"`): only the hive carries the graph, and the graph is the assembly point of the colony. **Ingress** (stdin → topology): the bridge is the birth point of the message and establishes the initial `context` directly (a sanctioned entry edge, symmetric to the HTTP ingress, see § Metadata aggregation), with the same context triad as the `proxy`: a fixed well-known `user_id` (the stdio user, identical across all runs), a `chat_id` per process run (start until EOF = one stdio session, analogous to the `proxy` `chat_id`), and a fresh `turn_id` per stdin line; emitted with `sender = @external` to the root cell. **Egress** (topology → stdout): a message that runs back to the root hive `/` and there matches no further out-edge is translated to stdout instead of being dead-lettered as `HiveNoRoute`; that is "one level up = outward", and at the root hive this level is stdio. stdio is thus an **absolute endpoint** (pure sink, like the `proxy` inbound behavior); later coupling of several colonies over pipes is an outlook, not a v0.1.0 scope.

**Lifecycle**: in direct mode (neither `--daemon` nor `--api`), stdin EOF is a shutdown trigger (drain + exit 0, see § CLI § Modes). `--api` without `--daemon` lets the bridge keep running (direct mode + parallel API), EOF also ends it. `--daemon` decouples the lifecycle from stdin: the bridge remains as a mechanism, EOF does not end it, shutdown only via signal/watchdog.

---

## Path addressing

```
/<hive>/<sub_hive>/<cell>    absolute, from colony root
/memory/2026-05-16/cache     example with application-specific hierarchy
/colony/registry             virtual colony endpoint
./cell_y                     relative to the sender path
../other_sub/cell_z          relative to the sender's parent directory
```

- `{root}` = filesystem starting point of meclaw, in path notation `/`.
- Path resolution is a **pure string operation** on the sender path and the target expression. `.`, `..`, the `/` prefix are normalized to an absolute path.
- The lookup is O(1) on colony's central `HashMap<Path, ActorHandle>`. No hop-by-hop, no cascade across several routing tables.
- Paths are eternally stable (see "No-delete policy").

### Routing algorithm (central in colony)

```rust
async fn route(&self, sender_path: &Path, msg: Message) {
    // 1. path resolution: normalize target against sender_path
    let target = resolve_path(sender_path, &msg.target);
    
    // 2. escape hatch for colony paths
    if target.starts_with("/colony") {
        self.handle_colony_target(target, msg).await;
        return;
    }
    
    // 3. Registry-Lookup
    match self.registry.get(&target) {
        Some(handle) => {
            // log to central message log (filterable by path prefix)
            self.log_message(&sender_path, &target, &msg).await;
            let _ = handle.send(msg).await;
        }
        None => {
            // dead-letter cascade
            self.handle_unresolved(target, msg).await;
        }
    }
}
```

**Path resolution examples**:

| Sender path | `msg.target` | Resolved to |
|---|---|---|
| `/main/agent` | `./tool` | `/main/agent/tool` |
| `/main/agent` | `../collector` | `/main/collector` |
| `/main/agent` | `/other/cell` | `/other/cell` |
| `/main/agent` | `/colony/templates` | `/colony/templates` |

**Hive paths as target: transit evaluation**: a `target` that points to a hive scope marker is **addressable**. The hive itself has no actor and thus no mailbox; colony never delivers. Instead it evaluates the hive as a **logical transit node** in the same routing layer that also serves cell targets: it takes the out-edges of the hive (`EdgeTable` entries with `from = <hive-path>`), checks their CEL `condition` against the headers (CEL standard semantics as everywhere, see "Edge model"), applies the `modifier`, and per hit triggers a **regular routing hop** to the respective `to` path. The TTL of the message is decremented **per hop** (a hive transit hop counts the same as a cell-to-cell hop). From the sender's perspective the hive is thus an addressable target, in the substrate a transit hop in the one routing layer, no bypass, no separate hive routing logic.

**Special case: no out-edge matches** (edge list empty or all CEL conditions evaluate to `false`): the message goes to `/colony/dead_letters` with the **own `error_code` `hive_no_route`** (canonical string, new `DeadLetterReason::HiveNoRoute`). Deliberately not as `unresolved_path`: the hive was reachable, but the routing graph did not forward it; this distinction is builder observability (hive dead end vs. typo path), not an internal implementation detail.

Mutations for a hive scope still go to `/colony/mutations` with the hive path as the scope field in the mutation body, not to the hive path as the `target`.

**Invariant: `route()` is pure.** All logging, sync, metric evaluation lives in a **wrapper at the call site** (see `route_with_log` in [`crates/meclaw-colony/src/colony.rs`](../crates/meclaw-colony/src/colony.rs)) that does the pre-check + snapshot before the `route()` call and the log send after the return. NOT in `route()` itself. *(Baseline note: the original "body byte-identical to the **phase-4-done** state" wording is superseded since the **hive-transit re-baseline (2026-06-04)**; `route()` has since carried the `hive_scopes` param, a `RouteAction` return, and the HiveTransit branch. The corridor gate runs against the frozen fixture `plans/phase-13.5-hive-transit-fixtures/expected_route_body.txt`, no longer against `phase-4-done`.)*

**T33 near-miss (phase 5)**: a subagent extended the `route()` signature, under borrow pressure (`&Connection` not `Send` across async points), by a `log_tx` parameter + a log-send block in the body. Caught in review, reverted in commit `1736e8a`. Lesson: subagents tend to extend route() "minimally invasively"; execute prompts must repeat the pure invariant **explicitly per task**.

**Body verify command** (to be run per `route()`-relevant commit), comparison against the frozen hive-transit fixture:

```bash
diff <(git show HEAD:crates/meclaw-colony/src/colony.rs | sed -n '/^async fn route(/,/^}$/p') \
     plans/phase-13.5-hive-transit-fixtures/expected_route_body.txt
```

Empty = body byte-identical to the sanctioned hive-transit state. The `#[rustfmt::skip]` attribute over `route()` keeps the committed form frozen against this character-wise gate (`cargo fmt` may run freely over the workspace). With nested closures with `^}$` lines that could close the `sed` pattern too early: check instead the full `git diff <hive-transit-baseline-tag> HEAD -- crates/meclaw-colony/src/colony.rs` and confirm that NO hunk falls into the `fn route` definition (`route_with_log` hunks are allowed; `route()` proper is not).

### Path resolution: edge cases

`Path::resolve(sender, target)` **always** returns a `Path`, **never** a `Result`. Rationale: resolution is a pure string normalization (see above); it cannot "fail". Whether the resulting path exists is a separate question that the downstream registry lookup answers. Thereby `route()` has exactly **one** error source (unknown path → cascade), not two (resolution error vs. lookup miss). The following inputs are thereby unambiguously defined:

| Input | Behavior | Rationale |
|---|---|---|
| `../` beyond the root (e.g. sender `/a`, target `../../x`) | **Clamp to `/`** (yields `/x`) | Linux convention (`cd / && cd ..` stays `/`). No error path needed. |
| Empty target `""` | → `sender_path` (identical to `.`) | Empty string = "no hop". |
| Bare name without a prefix (e.g. `cell`, no `/`, `./`, `../`) | → relative to the sender (like `./cell`) | The most natural interpretation for a prefix-less string; shell-analogous. On non-existence it lands via a regular registry miss in `dead_letters` (reason `UnresolvedPath`), no special error needed. |
| Trailing slash (`/a/b/`) | → normalized (`/a/b`) | Consistent key form. |

### Behavior on routing errors (cascade)

When a resolved path does not exist in colony's registry (cell removed by mutation, subtree never instantiated, typo):

1. If `reply_to` is set: error message back to `reply_to`.
2. If `reply_to == None`: send to `/colony/dead_letters`.
3. Colony logs with `trace_id`, resolved path, original target, reason.

**Phase delineation (phase 2 vs. from phase 3)**: step 1 (`reply_to` reply) and the `trace_id` part of step 3 presuppose the UBF header, which arises only in **phase 3**. In **phase 2** the message is trivial (`{ target, payload }`, no `reply_to`, no `trace_id`). The phase-2 cascade is therefore **reduced**: an unresolvable path goes **always directly** to `/colony/dead_letters` (step 2), with logging of resolved path, original target, and reason; the `reply_to` branching and the `trace_id` logging are added in phase 3 when the header fields exist. Pre-building the `reply_to` branching in phase 2 is a phase overreach.

**Dead-letter queue properties**: `/colony/dead_letters` is a colony-internal construct, **not** an entry in `HashMap<Path, ActorHandle>`; `/colony/*` paths are intercepted in routing as virtual endpoints, before the registry lookup. **Persistence (phase-16 W6d / audit A6):** the DLQ is **persistent in `colony.db`** (table `dead_letters`, `schema_version` 4), no longer a volatile in-memory `VecDeque`. It is the last remaining diagnostic truth after the loss of message persistence and now survives colony shutdown/crash. The DB is the **only** truth: read and drain query the table directly; an in-memory `VecDeque` serves only as a transient hand-off buffer that the single-owner `colony_task` flushes into the DB after every event (never a second mirror). **No drop-oldest** anymore (the bounded ring-buffer eviction is gone); the diagnostic entries are preserved, not evicted; a fire-and-forget write keeps backpressure away from routing. Each row carries the six localization fields (`DeadLetterDto`; since P1 additionally `message_id` — parsed from `message_json`, `None` for legacy rows) plus the full serialized message envelope (`message_json`), so that the drain reconstructs the complete `DeadLetter`. Unimplemented `/colony/<x>` paths (all except `/colony/dead_letters` in phase 2) as well as `/colony` without a sub-path also land in the queue, with reason `ColonyEndpointUnimplemented` or `ColonyEndpointInvalid`, observable instead of crashy. **Read/drain symmetry**: spec-conformantly `/colony/dead_letters` is a message target (readable via `reply_to` roundtrip, from phase 3 / HTTP API phase 12). In phase 2, without `reply_to`, reading happens via a dedicated internal test hook (`ColonyMsg::DrainDeadLetters` with a `oneshot` reply); that is a phase-2 stopgap, not the final symmetric design.

**Canonical `error_code` strings**: every dead-letter reason (internally a `DeadLetterReason` enum variant) has a canonical string representation that is exposed in the dead-letter queue as the `error_code` field (relevant for the `?error_code=` filter of the phase-12 API): `unresolved_path`, `hive_no_route`, `no_route`, `cell_inactive`, `ttl_expired` (from phase 3, when TTL exists), `colony_endpoint_unimplemented`, `colony_endpoint_invalid`, `blob_unavailable`, `blob_recursion_too_deep`, `invalid_ubf_body`, `consumes_violation`, `contract_violation`. These strings are part of the stable API contract; new reasons extend the list, existing ones do not change their string form.

Notes on the delivery-boundary codes:

- `blob_unavailable`: blob resolution failure at the delivery boundary (`Body::Blob` uuid not findable); live since A8.
- `blob_recursion_too_deep`: exceedance of the depth limit in recursive blob reference resolution (see § Blob references). The string is part of the stable contract, **but the code is not produced today** (0 producers for `messages_id`/`text_id` pointers); recursive blob resolution itself is a roadmap defer (D-025).
- **`invalid_ubf_body`: debug-vs-release contract**: this code is constructed **only in the debug build**. The UBF structure validator in the `colony_task` `outputs_rx` arm runs under `#[cfg(debug_assertions)]` and DLQs malformed cell emissions as `invalid_ubf_body`. In the **release build** this structure validation of cell outputs is **inactive**; the string remains canonical and stable, but its occurrence is build-profile-dependent (D-033). (Not to be confused with the `contract.emits`/`consumes` schema validation, which is controlled via `colony.json` `strict_validation` and is its own safety net.)
- `consumes_violation`: a message missed the substrate-side required `consumes` check at the delivery boundary; the cell was not invoked (`docs/config.md` § consumes).
- `contract_violation`: a non-`code` emission violated its `contract.emits` at the central check of the outputs arm (flag-gated); the emission was discarded. With `input_reply_to`, an error reply is routed instead (no DLQ entry). Same canonical token as the `code`-in-cell reply (cell-types.md).
- `no_route`: a **cell emission** that matches no out-edge of its sender (edge list empty or all CEL conditions `false`) lands in the DLQ (the cell analogue to `hive_no_route`). No implicit identity fallback anymore (ruling A1): default routing is a **settable catch-all out-edge** (an unconditional edge from the sender = the "default edge"). The entry is self-localizing (four fields, see below).

**Outputs arm: three disjoint cases for a cell emission** (ruling A1 + Marcus 2026-06-12). When processing a cell emission in the outputs arm, exactly one of three paths applies, in this order:

1. **`em.target` is a `/colony/*` endpoint** ⇒ **direct ColonyDispatch** (registry/virtual-endpoint lookup via `route()`), BEFORE edge evaluation. `/colony/*` are virtual service endpoints (see "/colony as a virtual endpoint"), not topology nodes; an out-edge is there neither needed nor possible; the A1 no_route rule does not apply. An unknown `/colony/<x>` endpoint ⇒ `colony_endpoint_unimplemented`. This is the delivery path for cell-emitted mutations/reads (EDA).
2. **Substrate-generated error reply to a known sender** (`consumes_violation`, `message_timeout` backstop, `contract_violation`) ⇒ **directly to `reply_to`** (registry lookup via `route()`), NOT via out-edges. It is feedback to a known sender, not a routing target; a missing out-edge may neither redirect it nor turn it into `no_route`. An unresolvable `reply_to` ⇒ DLQ (cascade one-shot).
3. **Normal emission without a matching out-edge** ⇒ `no_route` DLQ (see above). **A1 governs exclusively case 3.**

`cell_inactive` = the target path exists (cell or hive) but is disconnected/inactive (see
§ Connectivity and activity); also applies to mailbox residue on disconnect.

**The cascade is one-shot, not recursive**: error replies (step 1) themselves set no `reply_to`; they are terminal. If an error reply is itself not deliverable, step 2 (dead letter) takes effect automatically, without a further cascade attempt. Maximum cascade depth is thus two hops: original error → reply attempt → dead letter. Rejected were: header-based loop detection (`is_cascade` flag, superfluous envelope state), TTL as a cascade backstop (conflates hop distance with cascade depth), multi-level configurable cascade depth (over-engineering for one pathology scenario).

### Mutation race safety

Colony processes its mailbox sequentially. Mutations are normal messages to `/colony/mutations`. While a mutation runs (including filesystem staging and registry edits), other messages pause in colony's mailbox. After mutation completion colony processes the next messages with the new registry state. If a message targets a cell removed in the meantime: cascade above.

Between cells and colony there is no race; all routing decisions run through colony's sequential mailbox. Parallelism arises only at delivery to the receiver cells, which all have their own Tokio tasks.

### Wildcards

None. Fan-out is solved at the edge level (1 output → several edges). Pub/sub patterns can be discussed separately later if needed; they are not currently planned.

### Routing symmetry

Cell → other cell and cell → colony both run through the same routing path (path resolution + registry lookup). `/colony/*` paths are virtual endpoints in the same registry, no asymmetry, no escape hatch needed.

---

## Cell model

- **Directory** with `config.json` (written by colony **only on instantiation**, then a bootstrap snapshot).
- **Optional**: `cell.db` (SQLite, persistent parameters and state, cell authority, `db:own` capability).
- **Optional**: `seed/<table>.jsonl` (bootstrap data + export target).
- **Uniform actor concept**: every cell is registered in colony's `HashMap<Path, ActorHandle>` registry with **one** `ActorHandle`, uniform for all cell classes, no sum type. The handle is at its core an `mpsc::Sender<Message>` (plus path and cell-type metadata). Colony's routing code is thereby identical for all cells: `handle.send(msg).await`.
- **Three spawn strategies** behind this uniform handle, depending on the cell class:
  - **Stateful**: not reentrant → 1 long-lived `cell_task` Tokio task that pulls the mailbox in a loop and calls `cell.handle()` directly. Cell state is single-threaded accessible from the cell's perspective (see "Concurrency and parallelism").
  - **Stateless**: reentrant → 1 long-lived `stateless_dispatcher` Tokio task pulls the mailbox and spawns a short-lived worker task per message that runs `factory.invoke()` and terminates. Concurrency limit per cell configurable via `tokio::sync::Semaphore` in the dispatcher (`params.max_concurrency`, see below).
  - **Long-running** (`proxy`/`timer`/`mcp`): double-task pattern (handler task + I/O task, see "Long-running cells: double task"). Both sub-tasks together under one logical cell identity.
- **No inner loop in cell code**: cells only wait for incoming messages (or external events for `proxy`/`timer`/`mcp`). Iteration is a topology matter.
- **Contract**: every cell declares `contract.emits`, `contract.consumes`, `contract.settings`, `contract.capabilities` (see `config.md`).
- **Knowledge is limited**: a cell knows only message + params. Not: the sender path, the receiver path, other cells. Envelope fields (`id`, `trace_id`, `parent_message_id`, `correlation_id`, `target`, `reply_to`, `ttl`, `created_at`) are **read-only** from the cell's perspective; they are set exclusively by colony during routing (see "Envelope setter authority" in the message model).

### Output path

Cells emit outputs via a cloned `outputs_tx` that goes to colony's central `outputs` mailbox. The cell trait signature is uniform for all cell classes:

```rust
trait Cell: Send {
    fn handle(
        &mut self,
        msg: Message,
        outputs: &mpsc::Sender<OutputEnvelope>,
    ) -> impl Future<Output = ()> + Send;
}
```

**Why `impl Future<Output = ()> + Send` instead of `async fn`**: native AFIT (`async fn` in a trait, stable since Rust 1.75) binds no `Send` guarantee to the returned future. In generic contexts like `cell_task<C: Cell>(…)` or `ColonyHandle::spawn<C, F>(…)` the compiler then does not know that `cell.handle(…).await` leads to a future that may travel to a worker thread via `tokio::spawn`, but multi-thread Tokio needs exactly that (see "Concurrency and parallelism"). Return type notation (`C: Cell<handle(..): Send>`) would be the more elegant solution but is not yet stable at the spec state. The explicit `impl Future + Send` in the trait return is the idiomatic stable-Rust workaround; every cell implementation either writes `fn handle(...) -> impl Future<Output = ()> + Send { async move { ... } }` or, more commonly, keeps `async fn` with an additional `Send` bound via a `where` clause. The `Cell: Send` supertrait is analogously mandatory because cells are passed via channel messages to `cell_task` spawns.

**What a cell emits (`CellOutput`)**: a cell **never** emits a finished `Message`; it does not know the envelope fields (see "Envelope setter authority"). It pushes `CellOutput` values over `outputs_tx`:

```rust
struct CellOutput {
    target: Path,                  // set directly by the cell in phase 3; from phase 4 typically from edge evaluation
    content: serde_json::Value,    // content JSON with optional "header" section; colony extracts header → message.headers, rest → body
}
```

In phase 3 (before the edges) the cell sets `target` directly in the `CellOutput`; from phase 4 the edge evaluation overlays this `target`. The `content` JSON is decomposed by colony: `content.header` → `message.headers`, the rest → `body: Body::Inline(...)`.

**Who attaches the parent context: `cell_task`, not colony**: a cell runs in its **own** `cell_task` (the actor substrate from phase 1). Colony does **not** call `cell.handle()` directly; were it to do so, the cell would run in colony's task and block it for the duration of the `handle()` call, which breaks the "one task per actor" model. Instead: `cell_task` holds the consumed incoming `Message` as a local stack variable and enriches each pushed `CellOutput` with the context that only `cell_task` knows, `parent_message_id` (= `id` of the consumed message), `trace_id` (copied from the consumed message), and its own `sender_path`. This enriched package goes to colony's `outputs` mailbox. Colony sets the remaining envelope fields (`id`, `reply_to` = `sender_path`, `ttl` decremented, `created_at`) and routes. No shared state, no lock, the parent context is local task state, consistent with the concurrency model.

**Where `outputs_tx` lives**:

| Cell class | `outputs_tx` lives | Who calls `outputs_tx.send().await` |
|---|---|---|
| stateful | in the `cell_task` local (cloned once at spawn) | `cell.handle()` |
| stateless | passed through as a parameter at worker spawn | `factory.invoke()` in the worker |
| long-running handler task | in the handler local (cloned once at spawn) | `cell.handle()` |
| long-running I/O task | has **no** `outputs_tx` | — (the I/O task only pushes internally to the handler) |

**`outputs` mailbox consumption**: colony's main loop runs as a `tokio::select!` over (a) its own routing inbox (incoming messages from the HTTP API, mutations, re-routed messages) and (b) the `outputs` mailbox (cell emissions). Both paths land in the same routing logic: edge evaluation, header modification, target resolution, then either `handle.send` (target is a cell) or hive-transit evaluation (target is a hive scope marker, see "Hive paths as target: transit evaluation"). Thereby there is exactly **one** routing layer, no bypass paths, no asymmetry between "internally emitted" and "externally fed in", and hive transit is a branch of this one layer, not a parallel path.

**Emit frequency per `handle()` call**: atomic-emitting cells call `outputs.send` once, stream-propagating `code` cells can send several times (multi-send). Backpressure takes effect equally on every `send` call (see the "Backpressure" section).

### Stateless cell dispatcher

The dispatcher task of a stateless cell runs as:

```rust
async fn stateless_dispatcher<F: StatelessCell + 'static>(
    own_path: Path,
    mut mailbox: mpsc::Receiver<Message>,
    outputs_tx: mpsc::Sender<CellEmission>,
    cell: Arc<F>,
    max_concurrency: usize,
) {
    let sem = Arc::new(Semaphore::new(max_concurrency));
    while let Some(msg) = mailbox.recv().await {
        let permit = sem.clone().acquire_owned().await.expect("semaphore closed");
        let outputs = outputs_tx.clone();
        let cell = cell.clone();
        let path = own_path.clone();
        tokio::spawn(async move {
            let sink = OutputSink::new(
                outputs, path, msg.id, msg.trace_id, msg.ttl, msg.headers.clone(),
            );
            cell.handle(msg, &sink).await;
            drop(permit);
        });
    }
}
```

**Choice A: generic `<F: StatelessCell + 'static>` instead of `Arc<dyn StatelessFactory>`**: `StatelessCell` uses RPITIT (`impl Future` as a return type in the trait), which makes the trait not object-safe; `Arc<dyn StatelessCell>` does not compile. Instead monomorphization per cell type: `stateless_dispatcher::<FileCell>`, `stateless_dispatcher::<BashCell>`, etc. Each cell instance gets its own monomorphized dispatcher entry point.

**Per-message `OutputSink`**: the worker (not the dispatcher) builds the `OutputSink` from the message metadata (`msg.id`, `msg.trace_id`, `msg.ttl`, `msg.headers`). The sink encapsulates `outputs_tx` + its own path and provides the `emit()` interface for `cell.handle()`.

**Permit drop at the worker end**: `drop(permit)` at the end of the worker closure releases the semaphore slot only when `cell.handle()` has completed. Thereby `max_concurrency` is a hard cap on actually concurrently running `handle()` calls, not just on spawned tasks.

`max_concurrency` is an optional cell param (`params.max_concurrency`, see `config.md`). **Default values per cell type** (phase 7):

| Cell | Default `max_concurrency` | Rationale |
|---|---|---|
| `file` | 8 | Disk I/O, the OS I/O queue saturates early |
| `bash` | 4 | Subprocesses, resource-intensive (memory, FDs, scheduler) |
| `edit` | 8 | Disk I/O like `file` |
| `web_fetch` | 32 | HTTP provider rate limits typically tolerant, the connection pool limits anyway |
| `web_search` | 8 | Search APIs are more strictly rate-limited than simple HTTP GETs |

The dispatcher task is a **real concurrency guard**, not a mere spawn loop: through `acquire_owned().await` it slows itself down before it pulls further from the mailbox; thereby under overload the mailbox fills up, senders (colony during routing) block, backpressure propagates cleanly backward.

### Long-running cells: double task

Cell types that continuously take in external events (`proxy`/`timer`/`mcp`) use, instead of a single cell task, a **double-task pattern per instance**. Both sub-tasks belong to the same logical cell, communicate via an internal `mpsc::channel`, and share a single `ActorHandle` address with an external mailbox in colony's registry.

**Motivation**: a 30-second long poll to Telegram, a `tokio::time::sleep_until` until the next schedule firing, or a blocking MCP SSE read must never block the acceptance of new messages from the topology, and conversely a full external mailbox must not stall the polling. A single task could only solve this via `tokio::select!` between an unboundedly long future and `mailbox.recv()`, with the risk that the cancellation of the future loses provider state on every new mailbox item (e.g. a Telegram update cursor half advanced). The double-task pattern decouples polling and mailbox frequency completely.

**Structure**:

- **Handler task**: holds the entire cell state (e.g. cursor in `cell.db`, session maps, in-flight correlation tables, schedule list). Does `tokio::select!` over (a) the **external mailbox** from colony's routing and (b) an **internal mpsc** into which the I/O task pushes provider events. Processes both sources sequentially, thereby single-threaded from the state perspective, no `Mutex`. It alone sets ordering and state mutations, holds the `outputs_tx`, and is the only one of the two sub-tasks that emits toward the topology.
- **I/O task**: holds **no** cell state, has **no** `outputs_tx`, has **no** direct `cell.db` access. Does the unboundedly long I/O operation (long poll, sleep, SSE read), serializes incoming events into event frames, pushes them into the internal mpsc. Receives reconfigure hints from the handler as needed (e.g. "the schedule was changed, your next sleep point no longer applies") over a second internal channel.

**Skeleton** (generic, without cell-type specifics):

```rust
async fn long_running_cell_spawn(
    mailbox: mpsc::Receiver<Message>,
    outputs_tx: mpsc::Sender<OutputEnvelope>,
    cell: Box<dyn LongRunningCell>,
) {
    let (events_tx, events_rx) = mpsc::channel(64);
    let (reconfig_tx, reconfig_rx) = mpsc::channel(8);

    tokio::spawn(io_task(cell.io_state(), events_tx, reconfig_rx));
    tokio::spawn(handler_task(mailbox, outputs_tx, events_rx, reconfig_tx, cell));
}
```

**Important: outer glue supervision task (AUDIT-PRE14-001):** the skeleton above is deliberately simplified. A pure fire-and-forget `tokio::spawn` of both sub-tasks would **swallow sub-task panics**; a panicking handler or I/O task would remain unobserved by the supervisor, the cell would be silently dead without a restart. The real spawn (`cell_task_long_running`) is therefore itself an **outer task with exactly one `JoinHandle`** that the supervisor observes (the `RespawnFn` signature thereby remains byte-identical to the single-task pattern). This outer task `tokio::select!`s over the `JoinHandle`s of both sub-tasks, **aborts the surviving sibling** on the first completion, **awaits both results** (not just the winning `select!` arm; a handler panic closes `run_io` too via the dropped `reconfig_tx`, and vice versa), and re-raises a panic via `std::panic::resume_unwind` → the supervisor sees `was_panic=true` (`one_for_one` restart). The panic propagation is thereby **order-independent** of the `select!` outcome. (The B backstop `cell.message_timeout` remains deferred for long-running, see the phase-7.5/9 limitations.)

**Backpressure behavior**: the internal mpsc from the I/O task to the handler is bounded. When the handler is overloaded (e.g. the topology takes tool results more slowly than the provider delivers them), the I/O task blocks on the push; the external polling frequency throttles itself, the TCP buffer at the provider self-regulates, no drop mechanism needed. Consistent with the system-wide `block`-only backpressure strategy (see "Backpressure strategy").

**Hot/cold model**: long-running cells are **permanently awake**; idle despawn makes no sense here by definition, because the raison d'être is the continuous external polling. `cell.timeout: -1` is the typical configuration (see "Hot/cold cell model").

**Message-timeout backstop**: long-running handlers typically have `cell.message_timeout: 0` or `-1` (no backstop), because a single `handle()` call here can definitionally be long (e.g. a long-running MCP tool call). Operation timeouts (concept A, `params.external_timeout_ms`) remain mandatory for every I/O operation in the handler, see "Timeouts".

**Cell-type-specific manifestation**: the concrete role assignment (what exactly the I/O task polls, what the handler holds) is a cell-type matter and is described in `cell-types.md` per type, see `proxy`, `timer`, `mcp`.

**Rejected** were: (a) a single task with `tokio::select!` over the mailbox and the I/O future, cancellation of the I/O future on every new mailbox item loses provider state. (b) `tokio::spawn` of a short-lived I/O task per mailbox message, does not fit the continuous polling character (long poll/sleep/stream). (c) clamping a long-running cell as two separate cells under a hive, would split provider state across two `cell.db`s, lose the atomicity of the internal channel, and break the "one address per cell" discipline in the registry.

### Lifecycle of `config.json` and `cell.db`

| File | Who writes | When |
|---|---|---|
| `config.json` (cell) | Colony | **only** on instantiation (template copy, UUID assignment, `${VAR}` substitution); the `swap_nodes` graph swap does not rewrite an existing `config.json` (see § Mutation operations) |
| `cell.db` (cell) | the cell itself | after param updates via message |
| `colony.db` | Colony | on instantiation + mutation commit + templates scan + message-log write |

After instantiation, `config.json` is a frozen bootstrap snapshot. Live state lives exclusively in `cell.db`. Param updates that come via message are persisted by the cell in its `cell.db`; `config.json` is **not** co-written. On a cell reset (e.g. wipe of the `cell.db`) the cell starts with its bootstrap state from `config.json`.

**Hive scope markers** have a `config.json` with `type: "hive"` and a `params.graph` field (initial desired graph for their subtree). They have **no** `cell.db`; the running graph lives in colony's registry and `colony.db`.

**Connection ownership model (phase 6.5)**: the `cell.db` connection lives in the
`cell_task_stateful` stack frame, not in a cell field. Cells implement
`StatefulCell` (in `meclaw-colony`, not `meclaw-core`, a layer separation as
with `CellFactory`) and get `&mut DbConn` as the handle param (phase-9 update,
previously `&mut rusqlite::Connection`). `cell_task_stateful` is the only
authority over the cell.db lifecycle, it opens at spawn via
`open_or_create_cell_db` (M1 resume-with-state), reopens on restart via the
factory RespawnFn closure, closes at mailbox disconnect or
cell-task panic via Drop. Cell impls are agnostic.

Thereby the E3 variant-3 pragma (snapshot-before-output) from phase 5 is retired:
cells can mutate state AFTER or between output emits, because `&mut DbConn`
can be held across `.await` (directly in the handle() async block).

**`DbConn` substrate pass (phase 9)**: `DbConn` encapsulates `rusqlite::Connection`
+ `rusqlite::InterruptHandle` (Send, single move into the timer task, no `Clone`,
no mutex exception). rusqlite calls are offloaded via `DbConn::call(|c| { ... }).await`
onto `tokio::task::spawn_blocking`; a real `query_timeout`
interrupts hanging queries via `InterruptHandle`. The closure is
`Send + 'static`, owned input/owned output. `DbConn::wrap(conn, query_timeout)`
sits between `open_or_create_cell_db` and `tokio::spawn(cell_task_stateful)`,
sync, no `.await`, the RespawnFn corridor stays unviolated.
`QueryTimeout` is a full-fledged `thiserror` error. The pass is
behavior-neutral: the phase-5/6.5/8 demos stayed green without assert adjustment.

**State identity model (M1 resume-with-state, phase 6.5)**: a renewed
`add_nodes` at a path with an existing `cell.db` reopens that DB
(resume, all rows remain). Schema migration is `CellFactory`
responsibility at spawn: the factory checks `schema_version` and migrates or
returns Err (mutation rejected). (`swap_nodes` no longer migrates
a `cell.db`; the re-dedicated graph swap instantiates or uses its own
implementation with its own `cell.db`, see § Mutation operations.) The wipe path is
deferred, no mutation op, an operator action outside the mutation flow.
Consistent with § No-delete policy "can be reconnected at any time via a renewed
add_nodes (with the same path/name)".

**`OpenStatus` discriminator (phase 9)**:
`open_or_create_cell_db_with_status` returns `(Connection, OpenStatus)` with
`OpenStatus::Created | Resumed`. `OpenStatus::Created` is the seed trigger
for `store` (the factory calls `load_seed_if_present` exclusively on
`Created`, otherwise duplicate rows). `Resumed` means an existing `cell.db`
was reopened, no re-initialization.

**Open-failure story**: `open_or_create_cell_db` panics on an FS-IO error,
DB corruption, or permissions. The factory closure does `.expect(...)` →
initial spawn: bootstrap/mutation error. Restart: supervisor loop until
`restart_limit` (default 5) → cell `failed`. Same mode as a
deterministically panicking cell (see § Restart strategy).

**Canonical ordering at panic hooks / backstop cancellation** (phase-5 test mocks; phase-6+ application for real cells with `cell.message_timeout` cancellation analogous):

1. `counter += 1` (or per-call state update, sync).
2. `write_snapshot_with(...)`: sync, persists pre-panic/pre-cancel state.
3. Cancel/panic check: sync, BEFORE the async output.
4. Output emit (async).

**Rationale**: panic/cancel BEFORE output ensures that an aborted cell emits NO output (otherwise the cascade keeps running AND the trace would get an extra hop that does not match the cell-state reality). The snapshot BEFORE panic/cancel ensures that the restart overlay sees the correct pre-abort state.

---

## Cell types (overview)

The overview table (type · task · actor kind · emission mode · phase) is canonical in [`cell-types.md` § Overview](cell-types.md#overview). The detail spec per built-in cell type is likewise in `cell-types.md`. Status per cell type / per phase → `PROGRESS.md`.

---

## Edge model

- Connects 1 output → 1 input (1 output can have several edges → fan-out → parallel).
- **`condition`** (CEL boolean) decides whether the edge is responsible. Reads **exclusively** the two header compartments of the source cell emission via the namespaces `context.*` (persistent) and `hop.*` (exactly this hop, = the isolated cell output ∘ the edge modifier; see "Headers vs. body: write model").
- **`modifier`** (operations object with CEL expressions as values) is the **sole header authority**: it promotes/computes `context.*` and refines `hop.*` before forwarding. Schema:

  ```json
  "modifier": {
    "set_context":    { "<key>": "<CEL over context.* + hop.*>" },
    "delete_context": [ "<key>" ],
    "set_hop":        { "<key>": "<CEL over context.* + hop.*>" },
    "delete_hop":     [ "<key>" ]
  }
  ```

  - `set_context` / `set_hop`: a map of keys to CEL expressions. Each expression has read access to **both** compartments (`context.*` and `hop.*`, read-only maps) and provides the new value. If the key already exists in the target compartment, it is overwritten; if it does not exist, it is created. Thereby `set_*` covers the two operations "set a new value" and "modify an existing value", separately per compartment.
  - `set_context` is CEL-valued and thereby covers both: **promoting** a hop value to context (`"set_context": { "turn_id": "hop.turn_id" }`) AND **computing** (`"set_context": { "iter": "int(context.iter) + 1" }`, the `int()` cast is necessary because CEL deserializes a JSON integer as `uint` and `uint + int` is not defined).
  - `delete_context` / `delete_hop`: a list of keys that are removed from the respective compartment.
  - All four fields optional. A missing or empty modifier = identity (`context` passed through unchanged, `hop` forwarded unchanged).
  - **Evaluation semantics**: all `set_*` expressions read the **incoming** (pre-modifier) state of both compartments as a fixed context. Order per compartment: first `set_*`, then `delete_*`, so that a set value cannot be accidentally deleted again by the same modifier piece (whoever wants that anyway writes two edges).
  - **Rationale of the schema choice**: CEL is a pure expression language without side effects; a CEL evaluation provides *one* value, it does not mutate inputs. Thereby "set/delete a header" is not directly expressible in CEL. Rejected: (a) a modifier as a CEL script that returns a complete headers map, forces edges to explicitly list all passing values, otherwise they are implicitly deleted; (b) a modifier as a patch map with a `null` sentinel for deletion, collides with `null` as a legitimate value. The variant chosen here makes all operations explicit, without sentinel values, separates them by compartment (`context` vs. `hop`), and stays trivially generable for AI builders.

  Example:
  ```json
  "modifier": {
    "set_hop": {
      "msg_type": "hop.finish_reason == 'tool_calls' ? 'tool_call' : 'final_response'",
      "tier":     "hop.priority == 'high' ? 'gold' : 'standard'"
    },
    "delete_hop": ["internal_debug_marker"]
  }
  ```
  Response metadata (`finish_reason`, `priority`, etc.) lives in the `hop` compartment. `context` values (`session_id`, `turn_id`, `user_id`, etc.) pass through unchanged because they are mentioned neither in `set_context` nor in `delete_context`.

- **Fan-out and the compartments**: on fan-out (1 output → N edges) colony copies the `context` **identically** into each of the N produced messages; the cell never touches `context`. Branch-specific content lives in the `hop` (written by the cell) or is set per branch via the respective edge modifier.

- **Edges operate strictly on the header layer.** Body slots and envelope fields (`target`, `reply_to`, `ttl`, trace IDs, etc.) are outside the edge scope. Whoever needs a body transform or envelope logic builds a `code` cell (see the cell-type description). Content-aware routing goes via "the cell sets a header (`hop`), the edge conditions on it". Rationale: symmetry between condition and modifier (both read `context.*` + `hop.*`, a simple mental model), routing performance (edge evaluation never has to resolve blob-referenced body fields), self-documentation (the graph shows directly which header values lead to which targets), body stability (cells can evolve body-slot schemas without edges breaking). Rejected were: a modifier may modify the body (would force blob resolution in the routing path and overlaps with `code` cells), a condition may read the body (the same performance concern, plus it takes away the pressure to emit header-disciplined outputs), a modifier may rewrite `target`/`reply_to` (makes the graph non-declarative, weakens the read API and audit).
- **Edge identity**: every edge has a UUID v7, assigned by colony on creation. Visible in the read API and in the mutation log. In the mutation surface (builder diff), however, edges are usually referenced via a **match pattern** over their properties (`from`, `to`, `condition`, `modifier`); the UUID is a fallback for disambiguation in the pathology case.
- **Edge table**: edges live centrally in colony's edge table, indexed by `from` path for fast fan-out lookup. Cells do not know their edges; colony evaluates them after a cell emission.

---

## Message model

```rust
struct Message {
    id: Uuid,                          // v7, time-sorted, set by colony
    trace_id: Uuid,                    // root message ID, constant across trace
    parent_message_id: Option<Uuid>,   // None at source, otherwise set automatically by colony
    correlation_id: Option<Uuid>,      // optional, for req/resp pairing

    target: Path,
    reply_to: Option<Path>,
    ttl: u32,                          // routing-step-based, decremented on every colony routing decision

    headers: serde_json::Map<String, Value>,  // routing metadata
    body: Body,                               // content (inline or blob)

    created_at: i64,                   // Unix seconds (SystemTime → as_secs() as i64), not milliseconds
}

enum Body {
    Inline(serde_json::Value),    // < blob_inline_max_bytes (default 64 KB, colony-configurable)
    Blob(Uuid),                   // ≥ threshold, in blobs/<uuid>.<ext> + blobs/<uuid>.<ext>.meta.json (see "blob storage")
}
```

**TTL semantics (flat)**: `ttl` is a protective limit against uncontrolled routing loops. Colony decrements on every routing decision, so once per cell-to-cell hop. At `ttl == 0` the message goes **directly** into the dead-letter queue (`ttl_expired`, direct-to-DLQ, **not** via the routing-error cascade with its step-1 `reply_to` reply attempt; an expired TTL is terminal, see "Routing algorithm"). Default in `colony.json` via `message_default_ttl` (recommendation: 64). Builders can set the value per initial message (`ttl` field in `POST /messages`, only positive integers, otherwise `422 invalid_ttl`). **Hierarchy**: an explicit `ttl` field of the initial message > `colony.json` `message_default_ttl` > the const seed `MESSAGE_DEFAULT_TTL` (=64); cells never set `ttl` (envelope setter authority). Not to be confused with `message_timeout_default_ms`, which addresses the maximum processing time _within_ a cell.

### Envelope setter authority

Envelope fields (`id`, `trace_id`, `parent_message_id`, `correlation_id`, `reply_to`, `ttl`, `target`, `created_at`) are **set exclusively by colony during routing**. Cells cannot write them; the content JSON that a cell emits has no mechanism for envelope fields, and edge modifiers operate strictly on headers (see "Edge model"). Concretely:

| Field | Who sets | When |
|---|---|---|
| `id` | Colony | on every new message (UUID v7) |
| `trace_id` | Colony | newly on a source message; otherwise copied from the parent |
| `parent_message_id` | Colony | taken over from the consumed incoming message, `None` on source messages |
| `correlation_id` | **no originating producer today**, a reserved envelope field for future req/resp pairing. Correlation currently runs via the **context header convention** (e.g. `turn_id`, see § Metadata aggregation), **not** via `correlation_id` | — (field reserved, no originating producer; the `?correlation_id=` filter on `/colony/trace` is therefore inert today) |
| `target` | the trigger layer (cell output determined by edges, the HTTP API by the endpoint) | on routing |
| `reply_to` | **Colony**, automatically to the absolute path of the sender | on every routing decision |
| `ttl` | Colony, newly stamped on source messages from `colony.json` `message_default_ttl` (seed: const `MESSAGE_DEFAULT_TTL`, =64); the HTTP ingress takes an explicit `ttl` request field per initial message as an override (hierarchy see "TTL semantics"); decremented per hop | newly on a source message, decremented afterward |
| `created_at` | Colony | on message creation |

**`reply_to` special case**: for messages fed in via the HTTP API (`POST /messages`), colony sets `reply_to` to a virtual API request path or leaves it `None`; the HTTP response is returned via the request channel, not via routing. Cells that want a reply target other than their own path (e.g. "the tool result should go to the collector hive, not back to the LLM") solve this **application-specifically via header-based routing**, e.g. a header `reply_target`, set by the original sender, and an edge that conditions on it. Thereby the substrate stays minimal: no reserved envelope-slot convention in the cell output, no envelope-write modifier, no softening of the edge-model spec.

**`reply_to == None`: terminal chain without a matching out-edge (actual behavior, W2-revised)**: the JSON ingress sets no `reply_to` today (leaves it `None`). A cell emission thereby triggered that subsequently matches **no** out-edge passes through the following chain since phase-16 W2 (rulings A1 + W2d):

1. The built-in cell types set the target of their op/error replies to the inbound `reply_to`; at `None` they fall back since **W2d** to their own `msg.target` (the path to which the cell was addressed), **no** longer to the `/colony/dead_letters` READ endpoint. The upstream hardcoded fallback `unwrap_or("/colony/dead_letters")` at the atomic-emitting cell types is removed.
2. Outputs arm (A1, three disjoint cases): the emission matches **no** out-edge, there is **no** identity decision anymore. It dead-letters as `no_route` (`DeadLetterReason::NoRoute`, the cell analogue to `hive_no_route`): self-localizing with `sender → resolved_target`, `trace_id`, `created_at`. Default routing is a **settable catch-all out-edge**. **Terminal**, no re-inject, no loop.
3. (Special case) if the cell emits explicitly to a `/colony/*` target: `/colony/mutations` is executed; `/colony/dead_letters` and the other read endpoints are hard-rejected or read respectively (§ "Endpoint classification for cell emissions"). The pre-W2d source loop (DLQ-listing reply back to the cell → re-emission, ttl-uncapped) is thereby eliminated at the root.

A `no_route` DLQ signature after a `reply_to`-less ingress probe (e.g. a store/sink op reply without a wired out-edge) is thus the expected actual behavior of this chain, not a routing bug; the op reply deliberately no longer reaches the sender (routing is explicit, not implicit-identity). The distinct no-match diagnostic signal for cell emissions (`no_route`) has existed since W2a.

### Headers vs. body: write model

Headers live in **two structurally separate compartments** with different lifetime and write authority:

- **`context`**: persistent, travels over the **entire** message lifecycle. **The sole write/delete authority: the edge.** Carries correlation/long-lived content (`turn_id`, `session_id`, `iter`).
- **`hop`**: exactly **one** hop. Is the isolated contract output of the immediately preceding cell, refined by the traversed edge modifier. Is **completely replaced** (expires) at the **next** cell emission. Carries the cell product/routing control/response metadata (`operation`, `finish_reason`, `msg_type`, `route`, `agent_target`, `rows_affected`, `error_code`).

From this follows the write model:

- **Cells write content as JSON** with an optional `"header"` section. Colony interprets this section **as `hop`**; it is the isolated cell output. **Cells never write `context`** (that is solely edge authority), and cells do **not** inherit the `hop` of their predecessor.
- **Cells read everything** read-only: `context`, `hop`, body slots, and the envelope fields. They write **exclusively** their isolated output → this becomes the new `hop`. (Read declaration in `contract.consumes.context.<key>` / `contract.consumes.hop.<key>`, body values in `contract.consumes.body.<key>`, for the split see `config.md`.)
- **Delete authority is an edge matter**: cells have no delete mechanism. An edge removes values via `delete_hop` (the hop compartment) or `delete_context` (the context compartment).
- **Edges (conditions and modifiers) are the sole header authority**: conditions read `context.*` + `hop.*` (read-only, a CEL boolean), the modifier writes `context` (`set_context`/`delete_context`) and refines the `hop` (`set_hop`/`delete_hop`) via the explicit operations schema (see "Edge model"). Body slots and envelope fields are outside the edge scope.
- **Default: the `hop` expires.** A value lives exactly **one** hop, unless an edge promotes it via `set_context` to `context` (fail-loud: forget to promote → the value disappears at the next cell emission).
- **Conflict rule**: within a compartment, replace (last-write-wins); the two compartments do not overlap. The audit trail lives in the central message log via the `parent_message_id` chain, not in the headers.
- **Composition along the routing path (R3 ruling, K-H7):** if a message passes through several edges in sequence (multi-stage hive transits, or cell emission → transit edge → further hive transit), same-key modifiers compose **left-to-right along the path**: each edge applies its `set_context`/`set_hop` to the header state **already transformed** by the preceding edge. If several edges set the same key, **the later, consumer-near, "inner" edge wins** (last-write-wins along the path). This is the same replace semantics as within a compartment, only drawn over the hop/transit sequence (`hop` expires at the path end on the next cell emission, `context` stays persistent). **Pin:** every transit hop writes its own `message_log` transit row, so that the composed end state stays traceable via the `parent_message_id` chain.

### Standard header convention (application level)

meclaw-core does not know these keys semantically; they are conventions for applications:

| Key | Meaning |
|---|---|
| `turn_id` | One user interaction (e.g. one chat turn from a proxy cell) |
| `session_id` | One logical session bracket |
| `user_id` / `chat_id` | external identifiers (proxy platform) |
| `locale` | language/locale of the request |

These standard keys live in the `context` compartment (persistent over the lifecycle). **Invariant: `context` is written exclusively by edges and by the ingress-at-birth, never by cells.** There are thus exactly two entry paths by which a key initially reaches `context`:

1. **Source cells** (`proxy`, `timer`): emit their values as `hop`; their **out-edge** promotes via `modifier.set_context` to `context`, in-graph, visible, no implicit substrate behavior. The source/proxy template pattern bakes this entry promotion in (default-present, but visible as a real edge).
2. **HTTP ingress**: is the birth point of the message and **establishes the initial `context` directly**, a sanctioned, tightly bounded context source (the ingress *is* the entry edge). The keys lifted to `context` by the ingress are **declared** (which HTTP headers → `context`: `turn_id`, `session_id`, `user_id`, `chat_id`, `locale`), so that they are auditable and the mutation validator treats the ingress as a **reachability root** for `consumes.context` (otherwise it would wrongly report `turn_id` as unreachable).

This ingress exception is **strictly limited to the birth point**, not a cell loophole: **cells cannot write `context` directly**, they emit exclusively `hop`. **Further header conventions are freely choosable**; applications, builders, or pattern templates can define arbitrary own keys (e.g. `msg_type`, `priority`, `tool_call_id`); whether a key is persistent (`context`) or hop-local (`hop`) follows purely structurally from the compartment. meclaw-core does not distinguish between "standard" and "application" headers; all are generic map entries in the respective compartment.

### Edge expression language

CEL (Common Expression Language) via the `cel` crate (GitHub project `cel-rust`; the crate name on crates.io is `cel`). An independent Google standard, safe (not Turing-complete), expressive enough for all known routing patterns.

**Feature scope (empirically verified against `evaluate_condition`, cel 0.13.0, 2026-06-11)**: the CEL standard macro `has()` is available in edge conditions; `has(hop.priority)` evaluates to `false` on a missing key (no eval error); equivalent for key-existence checks is the `in` operator (`'priority' in hop`). Besides that, the substrate tests demonstrate comparisons, ternaries, string methods (`contains`), and numeric casts (`int()`, necessary for arithmetic on JSON integers, see "Edge model" for the `uint` deserialization quirk).

### Atomicity

Messages are atomic, **two fixed header slots** (`context` and `hop`), one value per name per compartment, **no growing hop list**, no versioning, and no annotation history in the envelope. The two-compartment separation (see "Headers vs. body: write model") is structural, not historical: `hop` is completely replaced at every cell emission, `context` is overwritten via replace; in both cases it stays at exactly one slot per name. Real history (accumulated tokens, an iteration trail, expected tool-call IDs) belongs in a `store` cell or an aggregator, **not** in the header (see § "Metadata aggregation is topology"). Trace reconstruction runs via the `parent_message_id` chain in the central message log in `colony.db`.

---

## Body format (universal)

All cells emit and consume the same body. Thereby no cell needs a format adapter between inputs of different source types, `proxy` user turns, `bash` stdout, `web_fetch` response body, `file` read content, `code` script output, `llm` inference result: all in the same structure.

### Top-level slots

Three central slots, `system`, `messages` **or** `attachments`, of which **at least one** must be set (a schema `anyOf` over exactly these three `required` branches). A pure file upload (only `attachments`, without `system`/`messages`) is a legitimate message form.

| Slot | Meaning |
|---|---|
| `system` | Optional system context: identity (persona), tools schema, bootstrap instructions, facts, session state. Nested via sub-slots like `system.identity.soul.text`. The full slot path is required; abbreviated notation is forbidden. |
| `messages[]` | Chronological list of conversation turns. |
| `attachments[]` | List of blob-referenced file attachments (actively consumed from phase 12, the slot is name-reserved from phase 3, see "Reserved slot names" and the `attachments[]` schema). As an `anyOf` branch already a valid top-level shape today. |

**Reserved slot names** (name-reserved from phase 3, implementation as stated):

| Slot | Phase active | Meaning |
|---|---|---|
| `attachments[]` | 12 | List of blob-referenced file attachments (PDF, TXT, images, audio, etc.) with typed metadata, see "`attachments[]` schema" below. The slot name is reserved from phase 3: no cell may use it otherwise, even though the substrate does not yet actively consume the slot. |

Cells can create **their own top-level slots** (e.g. `meta`, `delta`, `event`, `graph`), as long as they do not collide with reserved slot names. Consumers ignore unknown top-level slots and unknown `system` sub-slots.

**Schema validation: timing and scope**: the UBF body validation against the JSON schema runs at points of different sharpness, **edge validation always-on, interior correctness as a debug net** (no trusted-exemption carve-out):

- **Edge (trust boundary): always-on, even in release:** every source body fed in via the HTTP API (`POST /messages`, the JSON as well as the multipart path) is validated against the schema **before** routing; a violation is rejected with `422` (`invalid_ubf_body`) instead of letting a malformed body into the routing layer. The attachments-only shape synthesized at multipart upload is a valid `anyOf` branch here. On the multipart path there is no client-authored UBF body: the client uploads files (`multipart/form-data`), the substrate synthesizes an `attachments[]` body from them that is schema-valid by construction (also the file-less case `{"attachments":[]}`). The always-on validation runs there as defense-in-depth but cannot fail by construction; a client-reachable `422` arises only on the JSON path.
- **`code` output: always-on (no opt-out):** the `code` cell is the only user-script-driven output source and thereby itself a trust boundary; its `contract.emits` validation runs **unconditionally** (`validate_emits = true`, independent of the build profile and `colony.json` `strict_validation`, see `docs/config.md` § Schema format and validation).
- **Built-in cell output: debug net:** the UBF structure validation of bodies emitted by substrate cells runs under `#[cfg(debug_assertions)]`, dev/test builds catch schema violations hard and DLQ them (`invalid_ubf_body`), release builds have zero validation overhead. This is a correctness safety net for the trusted substrate cells, **not** a trust-boundary carve-out: the untrusted edges (HTTP ingress, `code`) are covered always-on above. Additionally there (in the outputs arm) the central `contract.emits` validation of the non-`code` types runs, flag-gated (`resolve_validate_emits`): a violation → an error reply to the `input_reply_to` (`error_code: "contract_violation"`), otherwise the DLQ (the same token); `code` stays exempt from this (in-cell always-on).

The schema lies as a static document in `meclaw-core` (via `include_str!`, compiled once into a validator) and knows `attachments`-only as a valid shape. The `attachments[]` slot is anchored with its correct (phase-12) schema even though no phase-3 cell fills it; whoever filled it with foreign content would hit the validation. Thus the name reservation is syntactically enforced, without resolver code.

### `messages[]` schema

Each entry is either a **turn object**, a **turn pointer**, or a **bulk pointer**:

```json
// turn object, inline
{ "origin": "user|assistant|tool|system",
  "type":   "text|tool_call|tool_result|image|audio",
  "text":   "<inline-string>",
  "id":     "<required for tool_call/tool_result>" }

// turn pointer, a single turn content in the blob
{ "text_id": "<UUIDv7>" }

// bulk pointer, reference to a body document in the blob; its messages[] is expanded inline
{ "messages_id": "<UUIDv7>" }
```

- `origin` (required, enum): who spoke the turn.
- `type` (required): determines the semantic format. `image`/`audio` reserved for multi-modal.
- `text` (inline) **or** `text_id` (pointer), exclusive per slot.
- `id` is required on `tool_call`/`tool_result` and is the correlation anchor for the collector aggregation. Values are pass-through from the provider (`tool_call_id`).

### `attachments[]` schema (slot name reserved from phase 3, active from phase 12)

A list of typed file attachments that lie as blobs in the `blobs/` directory (see "Blob storage"). Each entry is an object with:

```json
{
  "blob_id":    "<UUIDv7>",
  "mime_type":  "application/pdf",
  "filename":   "report.pdf",
  "size_bytes": 124573,
  "sha256":     "abc..."   // optional; omitted in phase 12 (no consumer)
}
```

- `blob_id` (required): UUID v7, references the two blob files `blobs/<blob_id>.<ext>` + `blobs/<blob_id>.<ext>.meta.json`.
- `mime_type` (required): MIME type of the content. Authoritative in the sidecar; duplicated here for a fast read without a sidecar fetch.
- `filename` (optional): the original filename on upload via the HTTP API; `null` for system-generated attachments.
- `size_bytes` (required), `sha256` (optional): duplicated from the sidecar, same reason. `sha256` is not mandatory in phase 12 (see the sidecar schema note). **Schema-drift note (D-027):** the UBF JSON schema (`ubf-body.json`) lists `sha256` as **required** in `attachments[]` today, stricter than this spec. Latent (attachments become active only in phase 12); alignment of the schema to "optional" is pending at the attachments activation slice.

Cells that consume attachments declare `consumes.body.attachments` in the contract (see `config.md`). Cells that do not, ignore the slot. Thereby attachment processing is a **cell capability**, not a substrate detail, an `llm` cell with a vision model declares `consumes.body.attachments` and loads images via the storage abstraction, a text-only LLM cell does not see the slot.

Attachments are **separate from `messages[]`**: a conversation stays purely textual, attachments hang as a parallel list off the body. Thereby PDF attachments do not collide with the `messages[]` turn semantics (`origin`, `type: tool_call|...`), and LLM provider adapters can build them into their API call in a cell-type-specific way (e.g. with OpenAI as an `image_url` content block).

### `system` sub-slot structure

`system` is an application-defined tree. Leaves are `{text}` or `{text_id}` containers, analogous to turn objects/pointers. Example:

```json
"system": {
  "identity": {
    "soul": { "text": "Du bist ein..." },
    "body": { "text_id": "01HXY..." }
  },
  "facts": {
    "user_name": { "text": "Alice" }
  },
  "tools": {
    "web_fetch":  { "text": "{\"description\":\"...\",\"parameters\":{...}}" },
    "calculator": { "text_id": "01HXZ..." }
  }
}
```

**Tool definitions are a special case**: their `text` values are JSON strings with the tool definition (name, description, JSON-schema parameters). meclaw-core does not know this; the LLM provider adapter parses the format. Thereby the `{text|text_id}` leaf discipline stays universal.

**Concatenation into the provider system string**: at inference the LLM adapter builds a single string from the tree, joined with `\n\n` between sub-slots. Order: first the sub-slots listed in `params.system_order` (in the order given there), then all the rest alphabetically. Within sub-trees an alphabetical DFS walk is done. `system.tools.*` is **exempt** from this concatenation; tools are pulled out separately as a provider-native tool set.

### Replace semantics

Each slot is set atomically, not additively:

- updating `system.X.Y.Z`: this path is replaced, other paths stay unchanged (accumulative-replace per path)
- updating `messages[]`: the entire array is replaced
- whoever wants to append a turn sends the full desired list

History management is thereby an **application matter** (e.g. via a dedicated memory-hive topology), not a core feature. An `llm` cell fed directly by a proxy sees exactly what the proxy sends, typically a single turn, and answers exactly that.

### Blob references are universal

Every blob is itself a complete body document. `text_id` references "the text content of a body slot", `messages_id` references "a body document; take its `messages[]` and expand inline". Both hit the same cache (`blob_uuid → parsed body`), a cell that once resolved a blob holds it in-memory until restart. Cache invalidation does not exist, because blob UUIDs are immutable.

**Recursion is allowed but hard-limited**: a blob's `messages[]` can itself contain `messages_id` pointers, which are resolved further. **Self-cycles** (a blob references itself) are excluded by UUID immutability, the UUID of a blob exists only after content fixing, so it cannot stand in its own content. **Mutual cycles** (A→B→A over two blobs) are **not** excluded by this; they are caught, like pathological depth in general, by the hard depth limit. Pathological depth is caught by a **hard depth limit**, default **64**; the `colony.json` field `blob_max_recursion_depth` is **provided** as an override **but parsed-but-not-applied today** (see the section "`colony.json`: schema"). The depth limit itself **and** its `colony.json` wiring arise with the roadmap defer D-025 (recursive blob resolution has 0 producers today, § Routing errors `blob_recursion_too_deep`, consistent with the "code not produced today" finding there). On exceedance: a resolution failure with `error_code: "blob_recursion_too_deep"`, handled via the existing routing cascade (back to `reply_to` or to `/colony/dead_letters`). Resolution failures from non-findable blobs go the same way. Rejected were: the `message_timeout` backstop alone (poor diagnostics, a timeout means "too slow", not "too deep", and resource-intensive before detection), unbounded depth with a stack overflow as an implicit backstop (would crash the resolving cell task).

### Cell emission modes regarding `messages[]`

Cells fall into two classes, depending on how they handle the incoming `messages[]`, plus a special case:

| Emission mode | Behavior | Examples |
|---|---|---|
| **Stream-propagating** | The incoming `messages[]` is passed through + its own contribution (typically 1 turn) appended. The conversation thread stays along the chain. | guardrail/transform hive (modify passing turns), aggregator hive |
| **Atomic-emitting** | The cell emits a fresh `messages[]` with only its own contribution, no pass-through. Typical for sources (external events), tool endpoints, and LLM inference. | `llm` (assistant turn alone); `bash`/`web_fetch`/`web_search`/`file`/`edit` (tool_result turn); `proxy` (user turn from an external chat); `timer` (a schedule-configured body); `store`/`mcp` (an atomic query/tool response) |
| **Script-determined** *(special case)* | The cell emission mode arises per execution from what the script writes, can be atomic or stream-propagating. The only cell type in this class. | `code` (programmable body constructor, see `cell-types.md`) |

Which emission mode a cell is belongs to its job description and is stated in `cell-types.md`. The decision follows from the cell type, not from the topology.

**Consequence**: stream chains are effectively **append-only** with respect to `messages[]`. Atomic-emitting cells break the chain deliberately; whoever wants to bring the conversation context back to the LLM builds a **collector topology** that aggregates tool-result messages and joins them with the conversation thread (an application pattern, not core).

**Multi-send wire format**: cells that declare `contract.multi_send_capable: true` may emit several output messages per input. The concrete wire format is cell-type-specific; for `code` see the cell-type description in `cell-types.md`. Every emitted message runs independently through the outgoing edges of the cell, colony evaluates afresh per emitted message, routing can diverge per message.

### Output: what the cell emits, what it persists

Two different things:

| | In the output message? | In `cell.db`? |
|---|---|---|
| Incoming `messages[]` (with blob refs unresolved) | yes, passed through | yes, last-received as-is |
| Own new turn (e.g. assistant turn from an LLM call) | yes, appended | **no**, the output is not persisted back into the cell state |
| `system.*` | **no**, private cell state | yes, accumulative-replace per path |
| `meta` slot (cell-specific metadata) | yes | no, each call sets its own |
| Blob cache | no (in-memory) | no (re-fetchable on restart) |

Thereby the cell state does not drift on its own: what the cell holds came from outside. It does not write itself an own "truth" about the conversation.

### Metadata aggregation is topology

Response metadata (`tokens_prompt`, `tokens_completion`, `model`) lives in the `hop` compartment and **expires** at the next cell emission, an `llm` cell that runs several times in the tool-loop produces a fresh `hop` on every call. There is no accumulated header view over the loop; every hop carries only the metadata of its own call.

Whoever wants totals (accumulated tokens, USD cost, latency sum): a **separate aggregator hive** in the pipeline, correlated via an application convention in the `context` compartment (e.g. `turn_id`), holds the totals in its own `cell.db` and adds to them. Aggregation is never a cell-type responsibility, it is application topology. Real history belongs in the `store`, not in the header (see § "Atomicity").

---

## Iteration is topology (not a core feature)

llm cells have **no inner loop**. They make a provider call, give the response out as one message, done. Any iteration (tool-loop, ReAct, plan-and-execute, etc.) arises through graph topology and is **application logic**, not meclaw-core.

meclaw brings **no** prefabricated tool-loop topologies, dispatcher hive, or collector hive. The builder (human or AI) composes such patterns from the basic building blocks (hive scopes as grouping, `code` cells, `store` cells, `llm` cells). What meclaw-core guarantees: the topological composition is possible because cells are dumb and edges decide.

**Example topology for a tool-loop** (for illustration, not as a prescription):

```
[llm] ──► [dispatcher (code-cell)] ──► fan-out via edge conditions
                                  ├─► [proxy]            (intermediate user message)
                                  ├─► [tool-A]           ┐
                                  ├─► [tool-B]           ├─► [collector (code-cell)] ─► [llm]
                                  ├─► [tool-C]           ┘    (same turn_id, next iteration)
                                  └─► [collector]        (expect-Notification)
```

The dispatcher hive decomposes the LLM output into several typed messages (e.g. tool calls, intermediate responses, expect notifications). The collector hive collects tool answers and sends them aggregated back to the LLM. Both usually live under a `hive` scope marker that groups the tool-loop sub-topology as a unit (e.g. `/main/tool-loop/`).

**Loop counter and correlation in the two-compartment model:**

- The loop counter (`iter`) lives as **`context`** (persistent over the iterations) and is incremented on the **loopback edge** via `modifier.set_context: { "iter": "int(context.iter) + 1" }` (the `int()` cast is necessary because CEL deserializes a JSON integer as `uint` and `uint + int` is not defined). Cells never touch `iter`, incrementing is edge authority.
- The **collector correlation matches over tool-call IDs** (set difference: expected IDs ⊆ received IDs), **not over counting**, so that it is idempotent as well as out-of-order- and duplicate-proof. The **expected ID set lives in the `store`** (real history → `store`), not in the header; the header carries per tool result only its `tool_call_id` (hop-local).

**Routing conditions** on the edges use regular CEL expressions on `context.*`/`hop.*` keys (e.g. `hop.msg_type == "tool_call"`). Such header conventions are **application conventions**, meclaw-core does not know them as a special case.

---

## Template system

### Template definition

- Templates are cells (or whole subtrees including hive scope markers) under `templates/`. Their role: class / blueprint.
- **The directory structure within `templates/` is freely choosable**, sub-folders, groups, namespaces are allowed. The scanner finds templates by the `template.json` file.
- **Identification by name**, because template-internal graphs need stable name references (UUIDs are assigned only after instantiation).
- **Versioning optional**: directory name `<name>@<version>` (e.g. `llm-openai@2.1.0/`) or simply `<name>/` (counts as unversioned).

### `template.json`: template index

Every template has a `template.json` in its root directory that describes the template as a class (separate from `config.json`, which describes the cell to be instantiated).

```json
{
  "name": "llm-openai",
  "version": "2.1.0",
  "description": {
    "purpose": "...",
    "use_when": "...",
    "not_in_scope": "...",
    "examples": [...]
  },
  "tags": ["llm", "openai", "completion"],
  "author": "@author",
  "license": "MIT",
  "homepage": "..."
}
```

`template.json` describes exclusively the template itself (metadata for discovery), no statement about the internal cell-type structure.

The `description` in `template.json` has exactly **four slots** (`purpose`, `use_when`, `not_in_scope`, `examples`); the **six-slot form** (additionally `emits_meaning`/`consumes_meaning`) applies to cell-`config` descriptions, see `config.md` § `description`. (Ruling 2026-06-10.)

### Templates registry (in `colony.db`)

Colony holds a persistent registry. Schema:

| Column | Content |
|---|---|
| `template_id` | UUID v7 (internal, primary key) |
| `name` | from `template.json` |
| `version` | from `template.json` or `NULL` |
| `filesystem_path` | where the template lies |
| `description_json` | cached description block |
| `tags_json` | cached tags |
| `author` | optional |
| `scanned_at` | timestamp |
| `embedding` | later, for semantic search |

### Scan strategy

- **At start**: the registry is loaded from `colony.db`, **no filesystem scan**. A fast start.
- **First-time start (empty registry)**: an automatic scan.
- **Manual rescan** via the CLI flag `meclaw --rescan-templates` or the API `POST /colony/templates/rescan`.

### Resolution `name@version`

Referenced in the graph via:

```json
"template": "llm-openai"           // → highest available version
"template": "llm-openai@2.1.0"     // → exactly this version
```

- Without a version: the highest SemVer version. Unversioned templates count as smaller than all versioned ones.
- SemVer ranges (`^`, `~`) only post-roadmap (marketplace relevance).

### Behavior on errors

- **Template referenced but not in the registry**: instantiation fails, an error message to `reply_to` (if set), the mutation is rejected. Colony logs. Batch mutation: the entire batch is rejected.
- **Registry entry present but the directory gone** (e.g. a manual `rm -rf`):
  - Lazy check at the instantiation attempt → error + automatic removal from the registry.
  - On `--rescan-templates`: all registry entries without a directory are deleted.
  - Existing instances that had referenced the template keep running (they have their own filesystem copy).

### Instantiation flow (colony)

1. Colony receives a mutation message to `/colony/mutations` in which an `add_nodes` entry describes a cell to be instantiated (fields: `name`, `template`, optional `override_params`).
2. Lookup in the registry: `template_ref → filesystem_path`.
3. Copies `templates/<path>/` recursively into the staging directory (`.staging/<mutation_id>/<name>/`).
4. Generates a new UUID v7 for all copied cells and edges.
5. Patches `config.json` with the new UUIDs. **The name stays as in the template (or as given in `override_params`)**; on a collision with sibling names within the same scope the mutation is rejected, see "Naming collisions" below.
6. Performs `${VAR}`, `${ctx.*}`, and `${uuid7:*}` substitution (see "Variable substitution").
7. Initializes `cell.db` from `seed/`, if present.
8. Atomic `rename(2)` from staging to the target path.
9. Registers the instance in colony's `HashMap<Path, ActorHandle>` and spawns the actor task: for **stateful** cells the `cell_task` loop, for **stateless** cells the `stateless_dispatcher` loop (with a `Semaphore` from `params.max_concurrency`), for **long-running** cells the double-task pattern (handler + I/O). In all three cases the mailbox is allocated as a bounded mpsc (default capacity 1000, overridable via `cell.mailbox_size` from phase 5).
10. Passes `params` to the instance at start.

### Discovery (for AI builders)

`GET /colony/templates` provides the template list from the registry with:
- `name`, `version`, `template_id`
- the full `description` block
- `tags`
- (later) a vector embedding for semantic search

Phase 11: plain text matching + tag filter. Post-roadmap: embedding index, vector search via API.

### Lifecycle of templates

- **Templates are read-only classes and are never automatically removed**, they are library blueprints and stand ready for future instantiations, even when no instance currently references them.
- **Manual removal** exclusively via the filesystem: delete the template directory in `templates/`, then `--rescan-templates` (or `POST /colony/templates/rescan`) so that the registry takes over the state.
- **No dedicated tooling** (no CLI subcommand, no API endpoints for cleanup), templates are library material, the FS discipline suffices.

---

## Seed concept (JSONL format)

DBs are **never stored as binary files** in templates, instead version-safe JSONL.

```
<cell>/seed/<table>.jsonl
```

Format:
```
{"schema": {"col1": "text", "col2": "int", "col3": "json"}}
{"col1": "value", "col2": 42, "col3": {...}}
{"col1": "value2", "col2": 43, "col3": {...}}
```

- **Line 1**: schema declaration.
- **Lines 2+**: records.
- **On fresh `cell.db` creation** (`OpenStatus::Created`, see § Lifecycle): colony reads the seed, builds `cell.db` anew. On reopening an existing `cell.db` (`OpenStatus::Resumed`) it is **not** re-seeded, otherwise duplicate rows.
- **Export**: the cell receives an `EXPORT` message, writes the current DB state as JSONL into `seed/`. File name: a UUIDv7 or `YYYY-MM-DD_<counter>.jsonl`.
- **Advantage**: no binary DB schema drift, grep-able, append-friendly.

---

## Variable substitution

meclaw knows **three substitution sources**, all with `${...}` syntax. Where each is allowed and who substitutes:

| Token | Source | Who substitutes | When |
|---|---|---|---|
| `${ENV_VAR}` | from `.env` in the root | Colony | when reading `config.json` (instantiation) and in mutation diffs (mutation validation) |
| `${ctx.<key>}` | from the header/body of the **mutation message** itself | Colony | on mutation application |
| `${uuid7:label}` | freshly generated per label | Colony | on mutation application |

All three sources are substituted exclusively by colony, the flat substrate has no intermediate layer that would have its own tokens.

### `${ENV_VAR}` from `.env`

- `.env` file in the root: classic key=value format.
- Substitution by colony, before `params` are passed to the cell. The cell sees only the substituted value.
- **POSIX-style default** supported: `${VAR:-fallback}` provides `fallback` when `VAR` is empty or unset. `${VAR}` without a default is strict, if the variable is missing there is an error (see error behavior below).
- **Escape:** `$${...}` escapes to literal `${...}`; substitution runs exclusively on instantiation.
- The strict variant `${VAR:?error_msg}` (bash-style) is **not** supported; any other `${VAR<op>...}` form besides `${VAR}` and `${VAR:-fallback}` is rejected with `unsupported_substitution` (no silent pass-through).

### `${ctx.<key>}` from the mutation context

- Allowed only in mutation diffs (not in `config.json` on the filesystem side).
- Access to the `ctx` block of the mutation message: `${ctx.user_id}` → the value of the `ctx.user_id` field. The resolution is **strict** from the `ctx` block of the mutation, no fallback to other sources; a missing key → reject with `ctx_key_missing` (see the `error_code` enum).
- Allows the requester to inject application-own identifiers (`user_id`, `session_id`, `turn_id`) into names and `override_params`, for which the requester places them **explicitly in the `ctx` block** (no automatic reading from the `headers.context` compartment of the mutation message).

### `${uuid7:label}` fresh UUIDs

- Generates a UUID v7 on the first occurrence of a label in a mutation. All further occurrences of **the same label** in the same diff get the same value.
- Different labels → different UUIDs.
- Labels are freely choosable (`sess`, `s1`, `worker_a`, ...) and valid only within the one mutation message, forgotten after mutation completion.
- **The form without a label (`${uuid7}` plain) does not exist**, explicit labels are mandatory for unambiguous semantics (prevents the foot-gun where every occurrence would unintentionally be a new UUID).

Example with all three sources combined:

```json
{
  "scope": "/main",
  "diff": {
    "add_nodes": [
      {
        "name":     "session_${uuid7:s}",
        "template": "session-scope@1.0",
        "override_params": {
          "user_id": "${ctx.user_id}",
          "api_key": "${OPENAI_KEY}"
        }
      },
      { "name": "worker_${uuid7:w}", "template": "worker@1.0" }
    ],
    "add_edges": [
      { "from": "./dispatcher",            "to": "./session_${uuid7:s}" },
      { "from": "./session_${uuid7:s}",    "to": "./worker_${uuid7:w}"   },
      { "from": "./worker_${uuid7:w}",     "to": "./collector"           }
    ]
  },
  "ctx": { "user_id": "alice" }
}
```

### Error behavior

| Error | When caught | Reaction |
|---|---|---|
| Missing `${ENV_VAR}` without a default at the initial colony bootstrap | before pipeline start | daemon failed-to-start, exit code != 0, error on stderr/log |
| Missing `${ENV_VAR}` without a default at mutation validation | mutation validation | error reply to `reply_to` (if set), mutation rejected |
| Missing `${ctx.<key>}` | mutation validation | error reply to `reply_to`, mutation rejected |
| Cell-init follow-on error from an invalid substituted value (e.g. an invalid API key) | cell init after commit | restart one_for_one, after N retries `failed` status |
| `${uuid7:label}` | never missing (always generated) | — |
| Name collision in the `post_state` after substitution | mutation validation | error reply to `reply_to`, mutation rejected (see "Naming collisions") |

### Naming collisions

Strict default: if a mutation produces a node name that occurs twice within the same scope in the `post_state`, the entire mutation is rejected (`error_code: "naming_collision"` in the error reply). No auto-suffix, no path magic. Whoever needs bulk instantiation with a uniqueness guarantee uses `${uuid7:label}` or `${ctx.<key>}` with application-stable tokens.

**Requester discovery** after the mutation: if the requester needs to know the resolved name and no application-stable token is available, there are two ways:
1. Generate the UUID yourself (outside the mutation) and insert it as a literal, the requester knows the name beforehand.
2. After the mutation, query `/colony/registry` (via HTTP `GET /instances?path=...` or via a message to `/colony/registry`), the registry provides for each instance `id`, `name`, `path`, `type`, `status`. UUIDs are time-sorted (v7), the newest cells are found at the end of the list.

---

## Blob storage

### Layout

Blobs live in the `blobs/` directory (default `{root}/blobs/`, CLI-overridable via `--blobs`). Every blob consists of **two files**:

```
blobs/<uuid-v7>.<ext>            # blob content
blobs/<uuid-v7>.<ext>.meta.json  # sidecar with authoritative metadata
```

- **`<uuid-v7>`** is the blob ID, time-sorted.
- **`<ext>`** is the native file extension, derived from the MIME type. Currently (phase 3+): `.json` for offloaded UBF bodies. From phase 12+: `.pdf`, `.txt`, `.png`, `.jpg`, more, depending on the `attachments[]` slot convention (see "Body format (universal)").

**Sidecar schema** (`.meta.json`):

```json
{
  "schema_version": 1,
  "mime_type":      "application/json",
  "size_bytes":     123456,
  "sha256":         "abc...",   // optional; omitted in phase 12 (no consumer)
  "created_at":     "2026-05-19T10:23:45Z",
  "filename":       null
}
```

- `mime_type`: authoritative MIME info. Consumers read the sidecar, not the extension (the extension is only operator convenience for `ls blobs/`).
- `filename`: the original filename on upload via the HTTP API (e.g. `"report.pdf"`); `null` for system-generated blobs.
- `sha256` (optional): a content hash for integrity checks and dedup potential (both post-roadmap). **Not computed in phase 12** (no consumer), the field may be missing; a recompute pass comes conditionally, if a dedup path ever lands.
- `schema_version`: for future sidecar extensions without a migration break.

### Behavior

- **Threshold** (default 64 KB) for offloading configurable via `blob_inline_max_bytes` in `colony.json`.
- **On writing**: a UBF body ≥ the threshold → is offloaded as `blobs/<uuid>.json`, the sidecar is co-written, only `Blob(uuid)` remains in the message. (The `==` boundary case is **inclusive**, the `Body` enum canonically implements `≥`; the prose is aligned to that here.)
- **On attachments** (from phase 12+): files uploaded via the HTTP API (`multipart/form-data`) are stored as `blobs/<uuid>.<ext>` with the real MIME type. The associated message carries an `attachments[]` slot entry with `{blob_id, mime_type, filename, size_bytes, sha256}` (`sha256` optional). Write order: first the blob file (`tmp` → `rename(2)`), then the sidecar (`.meta.json`) as a **commit marker**, likewise via an atomic `rename(2)`. Reader convention: a blob counts as complete exactly when its sidecar exists; blobs without a sidecar are ignored. (The phase-13 readers hang on this contract.)
- **On reading by a cell**: the cell consumes an `attachments[]` element or a `Blob(uuid)` body and calls a storage abstraction that co-loads the sidecar and returns content + MIME info. JSON bodies are still deserialized transparently for the cell as `serde_json::Value`.
- **No automatic GC**, blobs fall under the no-delete policy like the rest of `{root}/`. Disk-space management is an operations matter (external archiving via rsync, tarball, S3, etc.).

### Phase binding

| Phase | What |
|---|---|
| 3 | UBF body offload as `blobs/<uuid>.json` + sidecar (MIME `application/json`). The layout is future-proof, but practically only JSON blobs |
| 12 | Real attachments: `multipart/form-data` upload via the HTTP API, native extensions (`.pdf`, `.txt`, `.png`, `.jpg`), the operator web UI shows attachments in the trace view, the `attachments[]` body slot active |
| 13+ | Cell-type-specific consumers (LLM with vision, `code` cell with file processing, `store` cell with file indexing) |

---

## No-delete policy (event-sourcing at the filesystem level)

- **No file in `{root}` is ever deleted or moved.** Only new files/directories arise.
- **Instances are immortal**: a once-instantiated cell stays forever on the filesystem, keeps its `cell.db`, is findable via UUID.
- **Disconnect instead of delete**: cells no longer needed lose their edges
  (`remove_edges`/`remove_nodes`) and thereby become inactive, no longer routed, no
  tasks. They continue to exist on the filesystem and in `colony.db` and can be reconnected at any time
  via `add_edges` (or a renewed `add_nodes` at the same path), with
  the same `cell_id` and a resumed `cell.db` (see "Connectivity and activity").
- **Paths are eternally stable**: a central advantage for the "cells know no topology, but paths are reliable" discipline.
- **Hierarchy as builder discipline**: the trigger of instantiation (builder, CLI, API) chooses path and name deliberately to avoid root-directory pollution (e.g. `memory/2026-05-16_user_xyz/`).
- **Audit trail built in**: every state ever run, every message log is preserved.
- **Backup strategy trivial**: the whole `{root}` is a snapshot, Git-capable.
- **An operations concern, not core**: very old directories can be archived externally (rsync, tarball, S3, etc.), meclaw itself does not participate in this.
- **Carve-out (spawn-reject residue)**: no-delete holds absolutely for **registered** cells. The **only** exception is the cleanup of **fresh, never-registered** directories at a spawn reject (`sweep_reject_residue`, `crates/meclaw-colony/src/colony.rs`): an `add_nodes`/`swap` dir just renamed from staging into the live tree whose spawn fails is removed, it was never a living, registered cell. Adoption targets (`adopt`, a dir pre-placed by the builder with its own `cell.db`) are protected by the `preexisting_target` guard and are **never** deleted.

---

## Startup algorithm

1. Colony starts with `{root}` (default CWD or `--root`).
2. **Mutation recovery**: colony scans `colony.db` for mutation entries with status `in_flight` (interrupted in-flight at the last crash). Per entry: delete the staging directory `{root}/.staging/<mutation_id>/` (if present), mark the mutation as `failed` with `failure_reason: "crash_during_commit"`. Cell directories already renamed to their final paths remain as orphans in the live tree (the no-delete policy holds), colony considers them at the filesystem bootstrap (step 4) by their `config.json`. Additionally, `.staging/<mutation_id>/` directories without an associated `colony.db` entry are cleaned up as well. Mechanism and trade-offs: see the section "Filesystem layout" → `.staging/`.

   **Bootstrap recovery (first apply)**: the first apply writes a durable `bootstrap_in_flight` marker into the `meta` table of `colony.db` BEFORE the first cell spawn; its deletion runs atomically in the same transaction as the `InitialApply` bundle (edges + hive_scopes) at the apply end. If the boot-state classification finds the marker, the last first apply was interrupted (a crash between the per-cell registry upserts and the bundle): the boot is classified as **FirstBoot** and the apply runs again as an idempotent resume, a deterministic rebuild from the filesystem (the FS is the source; the registry upserts are `cell_id`-stable via the identity overlay, the bundle is `INSERT OR IGNORE`). No operator intervention, no "delete the DB". A table mixed state (e.g. registry non-empty, edges/hive_scopes empty) **without** a marker stays `Inconsistent` (external corruption, a strict-fail boot panic).
3. **Templates registry**: colony reads the templates registry from `colony.db`. If empty or `--rescan-templates`: a scan of `templates/`.
4. **Registry rehydration + filesystem validation**: colony rehydrates the registry from
   `colony.db`, known paths keep their persisted `cell_id` and their
   active/inactive status. The recursive tree walk validates the filesystem state against the
   persisted state. **Registration happens exclusively through instantiation/mutation,
   never through boot discovery (A5b):** at the **first bootstrap** the walk is the source, every
   unknown `config.json` node is recorded as a new entry. On a **reboot**, by contrast,
   an unknown (missing from the persisted registry state) `config.json` node, such as a
   manually created directory, is **not adopted, only reported** (a consistency view:
   WARN in the ops log; in `--validate` listed as a warning, exit 0, with `--strict` as an error). Such
   a node becomes a registered part of the graph only through a mutation on its path
   (adoption path "2b", see § Mutation format). For no already known path
   is a new `cell_id` assigned. Hive scope markers: read the `params.graph` hint and enter it as
   declarative edges for the scope (insofar as not yet persisted in `colony.db`).
   **Derived activity from the first bootstrap onward (the-one-rule):** the first bootstrap applies
   the same activation rule as the mutation recompute (§ Connectivity and activity): the
   computation is seeded from the `params.graph` edges (like a mutation from its
   `involved` set), and **only the newly recorded nodes reached by it** are brought to their
   edge-derived state. **Islands** (sub-hives whose internal edges seed their own
   scope) thus boot **inactive**, their permanent runners do not spawn. A **non-reached**
   node (an edge-less single cell) keeps its instantiation activity (**grace**, active),
   symmetric to mutation time, no blanket initial-active and no boot-only special rule.
   The root `/` is by definition always active; an already known node keeps its
   persisted status (reboot).
5. **Hydrate the edge table**: colony reads the persisted edge table from `colony.db`. On conflicts between `params.graph` hints and persisted edges, the persisted state wins (hints are only the initial desired state at first instantiation).
6. **Spawn long-running cells**: for each **active** `proxy`/`timer`/`mcp` start the
   double-task pattern directly (no lazy wake). Inactive long-running cells are not
   started.
7. **Start mailbox pumping**: colony starts its own routing loop (mailbox consume). The HTTP API and web UI bind on the `--api <bind>` address if the flag is set; otherwise the HTTP layer is inactive.
8. Colony booted.

**Boot endpoint existence check:** before the first cell spawn, colony checks that every
`params.graph` edge endpoint is **resolvable**, against the filesystem plan (cells + hives), the
**already running registry** (cells registered at runtime before the bootstrap), **or** a
`/colony/*` endpoint. An endpoint that points to none of these is a dead/typo'd edge and
leads to a **loud boot fail** (a precise message: edge ID + missing path) instead of a silently
ignored dead edge. **`--validate`** (a static dry run without a running colony) cannot in principle see
runtime-spawned cells and therefore reports a non-resolvable endpoint
as a **warning** (exit 0, the nginx `-t` role); the **`--strict`** flag raises these warnings to
**errors** (exit ≠ 0), the operator decides, not the internal logic.

Since all instances are persistent, this start is very fast: cells are not created anew, but rebooted from the existing filesystem (`cell.db` is read, tasks possibly spawned, see the hot/cold cell model).

---

## Connectivity and activity (active/inactive)

Every node of the graph, cell as well as hive, is at every point in time **active** or **inactive**.
The state is **fully derived from the edge table**; there is no own
activation or deactivation command in the mutation surface.

**Connectivity rule**: a node is **connected** when it participates, on its level, that is in the
enclosing scope, in at least one edge, as `from` **or** as `to`.
A single incoming or outgoing edge suffices; sources like `timer`/`proxy` (by design without
incoming edges) are connected via their outgoing edges.

**Hive sharpening:** for the connectivity of a **hive**, **only external
edges** count, in two forms: (a) edges of the parent level that reference the **hive path** as `from` or
`to`, and (b) edges that **cross the scope boundary of the hive**, exactly one
endpoint lies strictly within the hive subtree, the other outside (depth-port
wiring, e.g. `/anchor → /unit/dispatch`; R12 ruling 2026-06-11). The **internal**
wiring of the hive subtree (both endpoints inside) is
**meaningless** for the hive connectivity: a hive with an ever-so-richly-wired interior but without a single
external edge is **unconnected** (and thereby inactive, along with its entire subtree). Conversely,
a single external edge, referencing or crossing, already keeps the hive connected,
independent of whether anything is wired internally.

**Activity rule (recursive)**: a node is **active** exactly when it is itself connected
**and** its parent hive is active. The root is by definition always active. Thereby:
a disconnected hive deactivates its **entire subtree**, independent of its
internal wiring.

**Invariant (task ⇔ active)**: Tokio tasks run exclusively for active cells.
Long-running cells (`proxy`/`timer`/`mcp`) run exactly when they are active. Stateful
cells additionally follow, within "active", the hot/cold model (lazy wake), the two
axes are orthogonal (see § Hot/cold cell model).

**The-one-activation-rule (event-driven derivation, boot as well as mutation):** the activity
of a node is the **result of the last connectivity computation that reached it**;
a node never reached keeps its **instantiation activity**. This **one rule** holds
identically in both contexts, that is the point: the **first bootstrap** seeds the computation
from the `params.graph` edges (like a mutation from its `involved` set) and recomputes **only the
nodes reached by it**; a **mutation** seeds from the diff edge endpoints. It takes effect as soon as
a connectivity recompute reaches the scope of a node. **Freshly instantiated nodes
start active** and are brought to their edge-derived state by the first recompute that touches their
scope: at a **subtree** `add_nodes` (or an **island** at boot) the internal edges seed the recompute
over the own scope, so that inactive-derived subtree/island nodes do not eager-spawn in the first place. At a pure
**single-cell** `add_nodes` **without** an edge, and symmetrically at an **edge-less single cell at boot**, the recompute trigger is
missing; the node stays **active** for lack of a trigger (grace). This is intended: an edge-less node produces
**no** transient spawn-then-stop, because the recompute never reaches it. **Edge case (deliberately
symmetric):** an edge-less single cell **within** an unconnected sub-hive likewise keeps the grace (no edge seed in
its scope), should this residual grace ever disturb, it is changed on **both** paths at once, **never**
boot-only. (The edge case, a
single-cell `add_nodes` of a long-running cell whose diff edges derive it inactive, is
fixed: the activity gate before the eager spawn evaluates the POST-STATE edge view and
registers the cell inactive without a task spawn (paket-3 P3-C1).)

**Disconnect** (the last edge of a node is removed, typically via `remove_edges` or
`remove_nodes`):

- Colony recomputes the connectivity of the affected scope after every mutation and
  marks disconnected nodes, at hives including the entire subtree, as
  **inactive**. The marking is persisted in `colony.db`.
- Running tasks end gracefully: a running `handle()` call runs to its end, then the
  task ends. At long-running cells the handler and I/O task are stopped (external polling ends).
  If the cell blocks during the disconnect on a full `outputs`, the `term_timeout` reject applies
  (an atomic rollback); drain support during the disconnect window is post-v0.1.0.
- Residue in the mailbox of a deactivated cell runs into the dead-letter queue with
  `error_code: "cell_inactive"`.
- The registry entry, filesystem, `cell.db`, and `cell_id` remain fully preserved,
  disconnect is a stilling, not a deletion (no-delete policy).
- Inactive nodes do not participate in routing: every routing decision to an inactive
  path goes into the dead-letter queue with `error_code: "cell_inactive"`. Deliberately **not**
  `unresolved_path`: the path exists, is only stilled, the distinction is
  builder observability (a stilled node vs. a typo path).

**Reconnect** (a node receives an edge again, typically via `add_edges` or a renewed
`add_nodes` at the existing path):

- The node, and recursively its subtree, insofar as it is internally connected, is again
  marked active.
- Long-running cells of the reactivated subtree are started **immediately** (as at the
  colony startup, the step "spawn long-running cells").
- **Stateful** cells start **lazily** at the first message receipt (hot/cold model,
  wake-on-message). **Stateless** cells start, like long-running, **eagerly** (immediately
  on reconnect): they have no wake path, "lazy stateless" is not representable.
- Every `cell.db` is resumed (M1 resume-with-state), no re-initialization, `cell_id`
  unchanged, `config.json` not rewritten.

**Island activation (the official way).** An **island**, a subtree/sub-hive that at boot
was derived inactive for lack of an external edge (§ Hive sharpening; internally wired arbitrarily,
but unconnected to the parent level), is activated exclusively via an **`add_edges` mutation
that introduces an edge crossing the scope boundary into the island** (a *crossing-in edge*:
exactly one endpoint lies within the island subtree, the other outside, an intermediate hive
needs this crossing entry edge to be connected at all, a purely internal edge
does not suffice). This mutation seeds the connectivity recompute over the island scope; the
activation cascades from there recursively through the internally connected subtree (K-H5-proven).
This is the **only** sanctioned activation path; the earlier runbook trick "boot the
materialized instance subtree via re-root" (daily-digest) is thereby **superseded**, a topology activates
islands through wiring, not through re-rooting.

**Visibility**: `/colony/registry` still shows inactive nodes (field
`active: true|false` per entry); an optional filter `?active=true|false`.

**Rationale of the derived state**: deriving active/inactive from the edge table keeps
the mutation surface minimal (no new ops), makes the state reconstructable from the graph at any time,
and prevents drift between "declared deactivated" and "actually
unwired". Rejected were: an explicit `deactivate` mutation op (a second truth besides
the edge table) and a reachability computation via graph traversal from the root
(more expensive and stricter than necessary, the local edge participation plus the parent chain suffices and is
O(1) checkable per node).

---

## Hot/cold cell model (scaling)

With thousands of persistent instances but only a few active at once: dynamic spawning/despawning of Tokio tasks.

**Delineation from active/inactive**: the hot/cold model applies only to **active** stateful cells.
Active/inactive (§ Connectivity and activity) is an orthogonal, edge-derived axis persisted in
`colony.db`; `NotYetSpawned`/`Awake`/`Asleep` is the in-memory
lifecycle status within "active". Inactive cells have no lifecycle status, they
have no task and are not routed.

**Three states** per cell in colony's registry bookkeeping (applies to **stateful** cells; stateless and long-running see the clarifications below):

| Status | Meaning | Resources |
|---|---|---|
| `NotYetSpawned` | The cell exists on the FS, never spawned since colony start | mailbox channel allocated, no task |
| `Awake(JoinHandle)` | The cell runs as a Tokio task | task + mailbox + cell.db connection |
| `Asleep` | The cell despawned itself after an idle timeout | mailbox channel allocated, no task |

**Lifecycle**:
```
NotYetSpawned ──[first message]──→ Awake ──[idle timeout]──→ Asleep
                                     ↑                          │
                                     └──[new message]───────────┘
```

**Cell-task pattern** (stateful, with idle timeout + message-timeout backstop):
```rust
async fn cell_task(
    mut mailbox: mpsc::Receiver<Message>,
    outputs_tx: mpsc::Sender<OutputEnvelope>,
    cell: Box<dyn Cell>,
    message_timeout: Option<Duration>,  // backstop B, see "Timeouts"
) {
    loop {
        tokio::select! {
            Some(msg) = mailbox.recv() => {
                let result = match message_timeout {
                    Some(t) => tokio::time::timeout(t, cell.handle(msg, &outputs_tx)).await,
                    None    => { cell.handle(msg, &outputs_tx).await; Ok(()) }
                };
                if result.is_err() {
                    emit_backstop_timeout(&outputs_tx, /*...*/).await;
                    break;  // task ends, supervisor restarts
                }
            }
            _ = tokio::time::sleep(IDLE_TIMEOUT) => {
                if mailbox.is_empty() {
                    cell.shutdown().await;
                    break;
                }
            }
        }
    }
}
```

Operation timeouts (A) for I/O live **within** `cell.handle()`, see "Timeouts" for the clean separation.

**`IDLE_TIMEOUT`** in the pattern is configurable: a global default in `colony.json` `idle_timeout_default_ms` (recommendation: 60000), overridable per cell via `cell.idle_timeout_ms` in `config.json`. Takes effect only at `cell.timeout: 0`.

**`cell.timeout` from `config.json`** controls the behavior of stateful cells:
- `0` (default): idle-timeout model (Awake → Asleep)
- `> 0`: one-shot (despawn after every message)
- `-1`: persistent (proxy, timer, mcp, never despawn)

**Stateless cells do not have this three-state model.** The dispatcher task (see the cell model, "Stateless cell dispatcher") is permanently awake, it holds no persistent state, has almost no idle cost (a `mailbox.recv().await`-blocked task, ~3 KB stack), and a despawn-respawn cycle would save nothing except the mailbox channel itself. The sleep/wake mechanism is conceptually stateful-only, because it presupposes state preservation between `Asleep` and `Awake` via the `cell.db`.

**Long-running cells** (double-task) are permanently awake. Their existence is their raison d'être, the I/O task does external polling, the handler task waits for incoming events. Idle-asleep would contradict the purpose.

Tokio tasks are ~3 KB stack, not an OS thread. Thousands of sleeping cells have practically no overhead, only mailbox channels in colony's registry. Only awake stateful cells have relevant costs.

**Phases**: this model is activated in phase 13. Until then all cells are permanently tasks (see "Roadmap").

---

## Cell robustness (supervision, backpressure, timeouts)

### Restart strategy

- **`one_for_one`** as the only strategy, supervised by colony. When a cell panics, exactly that cell is re-instantiated by colony's supervisor.
- Rationale: cells are decoupled by design (know no topology), the OTP strategies `one_for_all`/`rest_for_one` solve problems that meclaw architecturally does not have.
- State preservation on restart: `cell.db` is reloaded, in-memory state is lost.
- **Restart limit**: default 5 attempts per cell (overridable via `cell.restart_limit` from phase 5). Immediate restart without backoff, no sliding window, a deterministically panicking cell fails quickly after 5 attempts and is marked `failed` in the registry. Routing to a `failed` cell runs into the dead-letter cascade (phase 2+). A `failed` cell returns via the normal reconnect semantics (line 1404) when a mutation directly addresses it (edge endpoint or resume); incidental recomputes do not reactivate it. The reactivation resets the restart counter. Rejected were: exponential backoff (solves no known pathology in phase 1, adds test-determinism complexity), OTP-style sliding window (`max_restarts in max_seconds`, the same trade-off). Harder guarantees (transactional in-flight preservation) are a phase-6 mutations topic.
- **Channel mechanics on restart**: the `mpsc::channel` pair of a panicking cell does not survive the panic, the `Receiver` is dropped during the stack unwind of the `cell_task`, the previous `Sender` in the registry thereby points to a closed channel (`SendError`). On restart the supervisor creates a fresh `mpsc::channel(1000)` pair, calls the respawn closure with the new `Receiver` (a new `cell_task`, a fresh `JoinHandle`), replaces the `Sender` in the registry atomically. In-flight messages on the old channel, `id=1` (the panic trigger) **plus all waiting mailbox messages `id=2..N`**, are **lost** with the receiver drop; the respawned cell gets a fresh, empty mailbox. Current state, accepted: **the panic/restart path has no silent-loss protection** (unlike the live backpressure path, § Backpressure). The future survival guarantee, `id=1` stays lost, `id=2..N` **must** survive and be delivered in order, is registered as a PRE-MVP roadmap requirement (**no** longer a phase-6 mutation replay, the earlier forward reference is thereby resolved).

- **Tripwire (phase 5+): `handle_cell_died` is await-free between the RespawnFn call and the registry sender swap.** In the implementation ([`crates/meclaw-colony/src/colony.rs`](../crates/meclaw-colony/src/colony.rs), `handle_cell_died`) there is **no `.await` point** between `(entry.respawn)()` (the RespawnFn closure, sync, contains `build_cell_with_open_db` and `tokio::spawn(cell_task)`) and `entry.handle = ActorHandle::new(...)` (the sender swap). The `tokio::select!` loop in `colony_task` processes a `ColonyMsg::CellDied` event iteration completely before it returns to the next `inbox.recv()`, **the serial loop is thereby the restart ordering barrier**. The phase-5 quiescence tests (Q8/Q9, counter-restore requirement) hang on this barrier: the test waits for the `spawn_count` increment, then sends the next message, it lands in the inbox and is received in the next loop iteration, at which point the sender swap has definitely happened.

  **Phase-6+ consequence**: every status persistence (`failed`/`disconnected` in the registry table, the mutation replay path) that introduces a `colony_db.send_op(...).await`-like point **between the RespawnFn and the sender swap** BREAKS the restart race safety, Q8/Q9 + every comparable test sync mechanism flakes. If an await becomes mandatory: reassess the restart barrier (in_flight counter, cell-completion signal, test-harness hook), **do NOT wave it through**.

  The inactive marking from § Connectivity and activity does not touch this corridor, it
  is set exclusively in the mutation path (`handle_mutation`), never in the
  restart handling.

### Mailbox size

- **Phase 1+**: bounded with default 1000.
- **Phase 5+**: overridable per cell via `cell.mailbox_size` in `config.json`.

Rationale of the phase-1 choice: bounded mailboxes are a **concurrency property** of the substrate. Introducing them retroactively would violate the "concurrency-first" architecture guideline of the roadmap (phases 2–4 would develop against unbounded channels, with different race and timing behavior than the later production substrate). `mpsc::channel(1000)` instead of `mpsc::unbounded_channel()` is one line of code difference, no bootstrap effort.

### Backpressure strategy

- **`block` is the only strategy** in the entire system, no cell-, colony-, or path-specific overrides. When a mailbox is full, the sender blocks (`mpsc::Sender::send().await`) until room frees up. Thereby backpressure propagates backward through the graph, **without silent message loss on this live backpressure path** (a blocking sender instead of a drop), **this no-loss scope does not apply to the cell panic/restart path**, which loses waiting mailbox messages (§ Restart strategy, "Channel mechanics on restart"). No drop logic, no per-routing-step strategy evaluation.
- **Implementation**: `ActorHandle` is a trivial wrapper around `mpsc::Sender<Message>`; `handle.send(msg).await` is one line. No `try_send` path, no branching logic, no wrapper crate.
- **Consequence for hanging cells**: a fully dead cell is detected by the **message timeout** (see below), the `handle()` call is aborted, the cell marked crashed, the `one_for_one` restart takes effect; the respawned cell starts with a **fresh, empty** mailbox (the still-waiting messages of the old mailbox are thereby **lost**, the same panic/restart loss as under "Channel mechanics on restart", the future survival guarantee registered as a roadmap item; **not** to be misunderstood as "drained"). A `tracing` warn log on `send` operations that block > a threshold gives early diagnostics.
- **Consequence for long-running cells**: the I/O task in the double-task pattern (`proxy`/`timer`/`mcp`) blocks on the push into the internal mpsc as soon as the handler is overloaded. This throttles the external polling frequency on its own, the desired behavior, the TCP buffer at the provider self-regulates.
- **Rejected were** (before the commitment to `block`-only): `drop_newest` (silent loss, agentic-LLM reliability breaks), `drop_oldest` (not Tokio-mpsc-natural, needs a custom wrapper that violates the "one task per actor" spec or forces an additional crate), `deadletter` (silent loss with an audit trail, semantically confusing relative to the existing routing cascade to `/colony/dead_letters` on routing errors). Whoever needs a different strategy (e.g. "prioritize the newest data") builds it **application-specifically via a `code` cell as a priority filter**, consistent with "iteration is topology".

### Timeouts: two concepts, cleanly separated

meclaw has **two different timeout mechanisms** with different purposes. They are called in the spec **operation timeout** (A) and **message timeout** (B). Whoever conflates the two gets either false restarts (the cell-hanger backstop too tight) or undetected hangers (no backstop), see the rule of thumb further below.

#### A. Operation timeout (cell discipline, `params.external_timeout_ms`)

**Purpose**: every I/O operation that can take an indeterminately long time gets a `tokio::time::timeout` wrapper in the cell code. Applies to HTTP calls (`web_fetch`, `llm`), DB queries (`store`), subprocesses (`bash`), filesystem operations (`file`, `edit`), MCP tool calls.

**Behavior on elapsed**: the cell catches the `Err(Elapsed)` result, builds a **regular error message** (`header.finish_reason: "error"`, `header.error_code` cell-type-specific like `provider_timeout`/`query_timeout`/`script_timeout`), emits it via `outputs_tx`, **the `handle()` call ends regularly**. **No** cell restart, **no** task killing.

**Configuration**: per cell via `params.external_timeout_ms` (a convention; individual cell types can choose semantically more fitting names, e.g. `params.query_timeout_ms` for `store`). The cell-type default in `cell-types.md`. Fully in the operator's hand, who knows that their self-hosted LLM needs 90s and the cloud provider answers in 5s.

**Cell-code example**:
```rust
match tokio::time::timeout(params.external_timeout, http_client.post(url).send()).await {
    Ok(Ok(response))  => /* normal processing */,
    Ok(Err(http_err)) => emit_error("provider_error", http_err, &outputs).await,
    Err(_elapsed)     => emit_error("provider_timeout", /*...*/, &outputs).await,
}
```

#### B. Message timeout (substrate backstop, `cell.message_timeout`)

**Purpose**: a backstop for pathology cases in which the cell hangs for an unknown reason, a cell-code bug without a clean operation timeout, a tokenizer loop, a JSON-parsing pathology, internally jammed state. **Not** the primary timeout for I/O.

**Behavior on elapsed**: the `tokio::time::timeout` wrapper around the **entire** `handle()` call ends it with `Err(Elapsed)`. The cell task is thereupon terminated (`break` from the `cell_task` loop), the supervisor detects it, the restart takes effect (`one_for_one`). Colony emits a generic timeout error message to `reply_to` (`header.finish_reason: "error"`, `header.error_code: "message_timeout"`). The trait-object state of the cell is lost, `cell.db` is reloaded at the re-spawn.

**Configuration**: a global default in `colony.json` `message_timeout_default_ms` (recommendation: 60000), overridable per cell via `cell.message_timeout` in `config.json`. A value of `0` or `-1` = no backstop (typically `proxy`/`timer`/`mcp`, which run long by definition).

**Cell-task pattern** (stateful, with backstop):
```rust
async fn cell_task(
    mut mailbox: mpsc::Receiver<Message>,
    outputs_tx: mpsc::Sender<OutputEnvelope>,
    cell: Box<dyn Cell>,
    message_timeout: Option<Duration>,
) {
    while let Some(msg) = mailbox.recv().await {
        let result = match message_timeout {
            Some(t) => tokio::time::timeout(t, cell.handle(msg, &outputs_tx)).await,
            None    => { cell.handle(msg, &outputs_tx).await; Ok(()) }
        };
        if result.is_err() {
            emit_backstop_timeout(&outputs_tx, /*...*/).await;
            break;  // task ends, supervisor restarts
        }
    }
}
```

The stateless dispatcher and the long-running handler use the same wrapper pattern around their respective `handle` call.

**Stateless specifics (worker vs. dispatcher):** the **worker task** spawned per message **is ephemeral**, it is **not** observed by the supervisor and **not restarted**; a worker panic ends silently with its message (discipline: `handle()` is panic-free, all I/O converted to error messages). The **supervised unit is solely the long-lived dispatcher task**: if it dies, the supervisor restarts it (and thereby the ability to spawn new workers), not the individual workers.

#### Rule of thumb for the configuration

**B generous, A precise.** Operation timeouts (A) are the actual protective layer for I/O, those are set tight in a cell-type-specific way. The message timeout (B) as a backstop lies **considerably above**, so that normally A always takes effect first and produces a clean error message. Only when the cell really hangs for an unknown reason does B step in.

Example `store` cell with complex queries:

```json
"cell":   { "type": "store", "message_timeout": 300000 }   // 5 min backstop
"params": { "external_timeout_ms":          60000 }        // 60s query protection
```

Behavior:
- A query takes 5s → normal.
- A query takes 70s → A fires after 60s, the cell emits a `query_timeout` error message, keeps running.
- The cell code has an SQLite deadlock bug → B fires after 5 min, the task is killed, restart.

#### Cell-type defaults (phase 7/8 final, preliminary recommendations)

| Cell type | `params.external_timeout_ms` (A) | `cell.message_timeout` (B, default) |
|---|---|---|
| `llm` | 110000 (110s) | 120000 (120s) |
| `web_fetch` | 25000 (25s) | 30000 (30s) |
| `web_search` | 25000 (25s) | 30000 (30s) |
| `bash` one-shot | 50000 (50s) | 60000 (60s) |
| `file` / `edit` | 10000 (10s) | 15000 (15s) |
| `store` | 60000 (60s) | 300000 (5 min) |
| `code` (stateful/stateless) | 60000 (60s) | 90000 (90s), an operator matter |
| `proxy` / `timer` / `mcp` | — (cell-type-internal, handler-specific) | `0` or `-1` (no backstop, long by definition) |

These defaults are set finally in phase 7/8, when the cell-type implementation becomes real. The operator overrides at any time per instance.

#### Foot-gun: a CPU loop without `.await`

`tokio::time::timeout` is **cooperative**, it aborts a future only at the next `.await` point. A pure CPU loop without `.await` (a pathology case) stays clinging to the worker thread, and neither A nor B takes effect. Countermeasures are code discipline (`tokio::task::yield_now().await` in long CPU loops, `tokio::task::spawn_blocking` for real blocking operations) and observation via `tokio-console` from phase 1 (see "Tech stack").

#### Other error paths (for delineation)

| Trigger | Behavior |
|---|---|
| An external call returns a clean error (HTTP 500, DB error) | the cell builds a regular error message, no timeout |
| Cell code panics (`unwrap`, OOB) | Tokio catches the panic, the supervisor detects it via `JoinError::is_panic()`, restart; the `message_timeout` backstop (concept B) triggers the same restart (the watcher classifies it as a backstop death kind, the panic has priority). |
| The cell removed by a mutation during a running `handle()` | **Graceful**: the running call runs to completion (the drop of the mailbox receiver only closes the inbox for new messages), then the task ends. **No** `abort()`, that would lose the drop cleanup. |
| An input validation error (a missing body slot) | the cell builds a regular error message, no timeout |
| The LLM answers `finish_reason: error` | a cell-type-specific error path, no timeout |

### Hanger detection

- **No explicit heartbeat.** The message timeout covers this implicitly, a cell that is too long in `handle()` is aborted.
- Watchdog mechanisms only post-roadmap, if needed then.

---

## API (HTTP)

- **Implementation**: a module in the colony (`meclaw-api` crate, in the binary), not as a cell type. Colony translates HTTP requests into typed `ColonyMsg` inbox commands (a oneshot-ack reply); the sequentiality of the colony loop is the symmetry guarantee. The "everything is a message" discipline stays preserved on the **data plane**.
- **Stack**: `axum` (Tokio-native, async).
- **Surface**: REST. gRPC if needed later as a second surface.
- **OpenAPI spec**: generated via `utoipa`.
- **Auth**: none in phase 12. Locally usable. Post-roadmap hardening (a bearer token via `${API_TOKEN}`, later capability tokens).
- **WebSocket** (`/events`): live topology events from phase 14 for visualization tools.
- Active in daemon mode (direct mode optional via flag).
- **Symmetry to internal message routing**: every HTTP endpoint is a thin wrapper that translates the HTTP request into a regular message and routes it to the appropriate authority (typically colony). Cells within a builder-hive reach the same data via direct messages, e.g. a graph read comes via a message to `/colony/graph?scope=...` or via HTTP `GET /colony/graph?scope=...` with an identical response schema.

### Visibility / read paths

The visibility layer is an **operator-oriented view** onto the canonical `/colony/*` endpoint list (see "/colony as a virtual endpoint"). It says: "if I want to do operation X, which endpoint do I use?". It defines no new endpoints, all paths named here are already listed in the canonical table.

| Operator question | Endpoint (internal + HTTP) | Filter |
|---|---|---|
| Which cells are running right now? | `/colony/registry` | `?path_prefix=`, `?type=` |
| The status of a single cell? | `/colony/registry` | `?path=<exact>` |
| What does the topology of a subtree look like? | `/colony/graph` | `?scope=<path>` |
| Which templates exist? | `/colony/templates` | `?type=` *(a silent no-op today, `cell_type` lies in the FS, not in `colony.db`; active from phase 14)*, `?name=` |
| What is in the dead-letter queue? | `/colony/dead_letters` | `?since=` *(functional, filters via `WHERE created_at >= ?` on the `created_at` timestamp of the dead-lettered message, since W2a/W2d)*, `?limit=`, `?error_code=` |
| What happened in trace X? | `/colony/trace` | `?trace_id=<uuid>` |
| Which errors occurred most recently? | `/colony/trace` | `?error=true&limit=20` |
| Which mutations are committed? | `/colony/mutations` | `?since=` (a read on the mutation log in `colony.db`) |
| A live stream of the routing decisions? | `/colony/events` | (subscription) |

HTTP routes are 1:1 the internal paths (see "/colony as a virtual endpoint", the symmetry statement). The operator web UI renders the same data as HTML under `/ui/*` (see "Web UI").

**Graph query response schema**:

Colony answers in the universal body format with a top-level slot `graph`. Consumers read `body.graph.*`:

```json
{
  "graph": {
    "scope": "/main/router",
    "graph_version": 42,
    "nodes": [
      { "name": "...", "id": "01HXY...", "type": "...", "template_ref": "...", "path": "..." }
    ],
    "edges": [
      { "id": "01HXZ...", "from": "...", "to": "...", "condition": "...", "modifier": null }
    ]
  }
}
```

- **Slot wrapper** (`graph`): groups the related fields under a named top-level slot, consistent with the universal-body discipline "cells may create their own top-level slots".
- **`graph_version`** is **constant `0`** today; the counter growing monotonically per scope (counts up on every successful mutation for this scope, helps with polling diff) is **planned from phase 14**.
- **Granularity: shallow only**, one level per read. Sub-scopes are read via separate graph queries with their path as scope.
- **Edge UUIDs visible**: the builder can use them when it needs them for disambiguation in pathology cases (`remove_edges` with the `id` field).

**Push vs. pull**: pull (`GET /colony/registry?path_prefix=...` with a `graph_version` comparison for cache invalidation) from phase 12; push (`GET /colony/events`, WebSocket subscribe) from **phase 14**, reason: the event broadcast would have to be fired from the routing loop, `handle_cell_died`, and `handle_mutation`, that touches the await-free `handle_cell_died` corridor (the byte-identical gate) and needs its own design pass (broadcast mechanics, a slow-consumer drop policy, an event schema). Pull is available from phase 12, because the web UI itself does not need it (no JS, no auto-refresh), but external clients (observability tools, a live graph viewer) benefit from it. Cell-to-cell subscriptions as a pattern are possible later, but not a core feature.

### HTTP endpoints

HTTP endpoints are 1:1 the `/colony/*` paths (see "/colony as a virtual endpoint", the symmetry statement). axum takes an HTTP request, builds a message with `target = "/colony/<endpoint>"`, sends it through the same routing path as an internal message. Thereby this table is redundant to the canonical endpoint table, it only repeats it in HTTP-route form:

| HTTP route | Method | corresponds internally to |
|---|---|---|
| `/messages` | POST | a general message inlet: an HTTP body → a `Message` with an arbitrary `target` (e.g. `/main/agent/llm` or `/colony/...`); axum translates and hands it to colony's routing |
| `/colony/dead_letters` | GET/DELETE | `/colony/dead_letters` (read + drain) |
| `/colony/registry` | GET | `/colony/registry` (with a filter query) |
| `/colony/templates` | GET | `/colony/templates` |
| `/colony/templates/rescan` | POST | `/colony/templates/rescan` |
| `/colony/mutations` | GET/POST | `/colony/mutations` (POST: a new mutation, GET: the mutation-log audit) |
| `/colony/graph` | GET | `/colony/graph?scope=...` |
| `/colony/trace` | GET | `/colony/trace?trace_id=...&...` |
| `/colony/events` | GET (WS upgrade) | `/colony/events` (subscribe) |
| `/ui/*` | GET | (an HTML render layer over the same data, see "Web UI") |
| `/health` | GET | a health check of the HTTP layer itself (no routing through colony) |

`POST /messages` is the only HTTP endpoint that can inject a message with an arbitrary target. All other routes are 1:1 their internal `/colony/*` paths (the symmetry statement in the section "/colony as a virtual endpoint").

`POST /messages` is fire-and-forget in phase 12: a response of **202 Accepted** with `{message_id}`; any cell answer runs via the routing cascade, not back via HTTP. A synchronous request/response roundtrip (an ephemeral reply sink) is deferred to phase 13+. The JSON request body is `{target, body, headers?, ttl?}`: the optional `ttl` field sets the TTL of the initial message (only positive integers ≤ `u32::MAX`; any other value → `422 invalid_ttl`); without the field, `colony.json` `message_default_ttl` applies. The multipart path has no `ttl` form field (uploads, not conversation turns), there the `colony.json` default always applies.

HTTP status `/colony/mutations` (POST): **200** on `Committed`, **422 Unprocessable Entity** on `Rejected`, the full `MutationOutcome::Rejected` detail remains in the `mutation` slot of the body. (The status code is part of the HTTP data model; 422 is a faithful translation of the reject outcome, not a symmetry break.)

**Deliberately not via the API**: template upload. Templates come only via the filesystem or CLI (security + no-delete discipline).

---

## Persistence

- Per cell: `cell.db` (SQLite) in the cell directory for dynamic state, cell authority. (No own param/config history table: `CELL_DB_DDL` carries only `system`/`last_input`/`meta`; `last_input` is forensics, not history.)
- `colony.db`: the central database with the registry (path → cell ID + status + template), the templates registry, the mutation log, the central message log, and the edge table. Colony writes; cells do not read directly.
- Trace reconstruction via `parent_message_id` chaining in the central message log (a flat `SELECT`, the parent-child tree is built client-/UI-side; index `idx_msglog_parent`).
- Blobs separately as JSON files.
- An operations log: `{root}/log.jsonl` (see the section "Logging").

---

## Logging

**Default**: `{root}/log.jsonl` (JSON Lines, append-only, created by colony at start if not present).

**Engine**: `tracing` + `tracing-subscriber` with a JSON formatter. Every subsystem (colony routing, cell tasks, HTTP API) writes into the same stream via the `tracing::*` macros.

**Override**: the CLI flag `--log <path>` (default `{root}/log.jsonl`); `--log-level <level>` (default `info`); `--log-filter <expr>` (a per-module filter, e.g. `meclaw_core=debug,meclaw_colony=info`).

**Rotate**: not in the core. An operations matter via external `logrotate` or similar.

**Format per line**:
```json
{"ts":"2026-05-17T14:32:15.123Z","level":"error","event":"mutation_failed","error_code":"template_missing","scope":"/main","mutation_id":"01HXY...","correlation_id":"01HXZ...","details":{"template":"llm-anthropic@2.1.0"}}
```

**Relation to the mutation log and message log in `colony.db`**: three logs coexist, complementary.

| Log | Path | Purpose |
|---|---|---|
| Operations log | `{root}/log.jsonl` | the tracing stream of all subsystems, for operator/debug, grep/jq-friendly |
| Mutation log | a table in `colony.db` | a structured audit trail of only the mutations, queryable via the API `GET /mutations` |
| Message log | a table in `colony.db` | every routed message with `trace_id`, `parent_message_id`, `from_path`, `to_path`, filterable by path prefix for scoped tracing |

**Tracing and metrics**: not core architecture. The `tracing` crate is OTel-bridgeable (crate `tracing-opentelemetry`), metrics exposure is solvable via external tools / sidecars / API extensions. Whoever needs distributed tracing or Prometheus scraping hangs it on externally, no crate or endpoint provided in the core stack.

---

## Dynamics / builder pattern

- Every cell (or an external API client) may send mutation messages to `/colony/mutations`, permission is a topology question, not an identity check.
- The mutation format is a **diff** with five optional operations: `add_nodes`, `add_edges`, `remove_nodes`, `remove_edges`, `swap_nodes` (see the section "Mutation format" above), plus a `scope` field (the path prefix for the mutation) and a `ctx` block (for `${ctx.*}` substitution).
- Colony validates in a single stage, executes staging + an atomic filesystem rename + registry edits, completes the mutation in `colony.db`.
- **Builder-hive** = a **hive scope** (not a single actor) that bundles several specialized cells under a path prefix, typically an `llm` cell for natural-language request understanding and diff generation, a `code` cell for mutation diff construction and validation, optionally a `code` cell for template-discovery aggregation (reads on `/colony/templates`) and a collector or memory hive for multi-step builder conversations. The final mutation diff is emitted by the outermost cell of the builder-hive (or a dedicated output hive) to `/colony/mutations`. Rationale for a hive instead of a single cell: the builder task is multi-stage (understanding → discovery → diff construction → validation), each stage benefits from its own cell with a clear contract, and a hive bundles them as an authority and mutation boundary. Usually lives under `/main/builder/` or similar, outputs run via the normal edge topology to `/colony/mutations`.
- Consistent with the no-delete policy: cells are never deleted, they become inactive through edge withdrawal (the registry entry remains, marked inactive; filesystem and `cell_id` remain) and can be reactivated via `add_edges` or a renewed `add_nodes` at the same path; `swap_nodes` swings, for template upgrades, the external edges onto a new or different implementation (a graph swap, the old cell remains disconnected and preserved, see "Connectivity and activity" and § Mutation operations).
- **EDA**: no success ack to the builder. Only an error notification (to `reply_to` if set, plus logging).

---

## DSL

- **An own meclaw schema**, JSON-only, optimized for the agentic-first architecture.
- Validation against JSON Schema Draft 2020-12.
- Adopted independent standards: **CEL** (edge expressions), **SemVer** (template versions), **HTTP/OpenAPI conventions** (for auth, retry, timeout, self-defined, leaning on established practice).
- No compliance with workflow standards like CNCF Serverless Workflow, the fundamentally different architecture (filesystem DSL, an actor substrate with a central routing authority, self-modifying instead of declared) makes such a leaning not sensible.

---

## Tech stack

| Area | Choice |
|---|---|
| Language | Rust (edition 2024, `rust-toolchain.toml` with `channel = "stable"`, a rustup update of the workstation suffices, no explicit version pin/bump; a build break from a new stable is treated as a spec-conflict question) |
| Workspace resolver | `resolver = "3"` in the workspace `Cargo.toml` (the default for edition 2024, but to be set explicitly in the workspace manifest) |
| Async runtime | `tokio` (multi-thread flavor, work-stealing scheduler, see "Concurrency and parallelism") |
| Async observability (from phase 1) | `console-subscriber` for the `tokio-console` bridge; activated via `--cfg tokio_unstable` in `.cargo/config.toml` (phase 0) |
| CLI | `clap` |
| Logging | `tracing` + `tracing-subscriber` |
| Non-blocking log writer (from phase 1) | `tracing-appender` (a writer wrapper with a `WorkerGuard` for flush; complements `tracing-subscriber`'s synchronous writer once async cells log) |
| Serialization | `serde`, `serde_json` |
| DB | `rusqlite` (decided in phase 5; `sqlx` rejected, `rusqlite="0.39"` in four crates) |
| Graph (data structure) | `petgraph` |
| Edge expressions | `cel` (crate; GitHub project `cel-rust`) |
| HTTP API | `axum` |
| HTTP client (from phase 7) | `reqwest` with the `rustls` feature (async, hyper-based, native Tokio runtime usage, a static binary possible) |
| HTML templating (operator web UI, from phase 12) | `maud` (inline HTML in Rust macros, no external template directory) |
| OpenAPI generation (from phase 12) | `utoipa` |
| UUID | `uuid` with the `v7` feature |
| Cron parser (from phase 10) | `croner` (6-field Quartz style with seconds, `find_next_occurrence`; used only as a parser, **not** a scheduler crate) |
| Date/time (from phase 10) | `chrono` (a foreign dep of `croner`; at the same time the source for UTC ISO-8601 timestamps; `chrono-tz` / local time zones deferred) |
| Errors | `thiserror` (library errors) + `anyhow` (binary errors) |
| JSON schema | `jsonschema` (Draft 2020-12) |
| Test tmp directories (dev-deps, from phase 0) | `tempfile` |
| File watcher | not in scope |

---

## Repo structure

```
meclaw/
├── README.md                  # landing page (pitch + pointers, no roadmap table)
├── CONTRIBUTING.md            # contribution guide
├── LICENSE-MIT                # MIT license
├── LICENSE-APACHE             # Apache-2.0 license
├── docs/                      # authoritative specification (canonical)
│   ├── meclaw-overview.md
│   ├── cell-types.md
│   └── config.md
├── Cargo.toml                 # workspace manifest
├── crates/
│   ├── meclaw-core/           # actor trait, ActorHandle, Message struct, path resolution, CEL wrapper
│   ├── meclaw-colony/         # colony task, registry, lifecycle, templates, routing, mutations
│   ├── meclaw-cli/            # binary, clap, daemon, stdin/stdout bridge
│   ├── meclaw-cells/          # built-in cell types
│   ├── meclaw-api/            # HTTP API (axum)
│   └── meclaw-testing/        # test fixtures
├── examples/                  # example colonies
└── rust-toolchain.toml        # pinned Rust version
```

**Inter-crate dependencies are introduced phase by phase.** The workspace manifest and the individual `Cargo.toml`s hold in phase 0 only the external dependencies actually needed for the respective current phase goal. A crate gets a `path = "../<other-crate>"` dep only when the current phase consumes a concrete symbol from the other crate. The layering is a consequence of the phase imports, not an overreach, the final topology (`meclaw-colony` → `meclaw-core`, `meclaw-cells` → `meclaw-core`, `meclaw-api` → `meclaw-colony`, `meclaw-cli` → `meclaw-colony` + `meclaw-cells` + `meclaw-api`) arises organically over phases 1–4.

**`meclaw-testing` is always `[dev-dependencies]`**, in every crate that consumes it. It is by spec never a runtime dependency (see the section "Test infrastructure (`meclaw-testing`)").

---

## Test infrastructure (`meclaw-testing`)

The `meclaw-testing` crate provides test fixtures and helpers for unit, integration, and phase-demo tests of all other crates. The consumers are exclusively `#[cfg(test)]` modules and `tests/` targets, the crate is never a runtime dependency.

**What the crate provides**:

- **`TestRoot`**: a RAII wrapper around a tmp directory as `{root}`. On drop the tmp directory is cleaned up. This is the **only permitted exception to the no-delete policy**, because tmp paths are not part of the real live tree. Implemented via the `tempfile` crate; every test gets a unique path.
- **`ColonyHandle`**: an async test wrapper around a running colony. Methods: `send_message`, `wait_for_response`, `wait_for_dead_letter`, `query_registry`, `shutdown`. Uses throughout the `#[tokio::test(flavor = "multi_thread", worker_threads = 4)]` runtime, no `block_on` anywhere.
- **`MessageBuilder`**: a small builder API for test messages, eliminates UUID/timestamp boilerplate.
- **`MockCell` set** under `meclaw_testing::mocks`: prefabricated cells for testing, echo, header capture, delay, fail-on-demand, counter. Cover the test patterns needed in phases 1–6.
- **Topology fixtures** under `meclaw_testing::topologies::phase_N`: helper functions that build the demo topology of the respective phase from the roadmap. Every phase-demo test calls its matching fixture.

**What deliberately is not in the crate**:

- No **production cells**, those come from `meclaw-cells`.
- No **external provider mocks** (LLM provider, Telegram bot, MCP server), these live as test modules directly in the respective cell crates, because they are provider-specific and needed only there.
- No **helpers for operator workflows**, the HTTP API from phase 12 covers that.

**Conventions**:

- All helpers `async`, no `block_on` anywhere in the test or helper code.
- Unique tmp paths per test via the `tempfile` crate.
- `#[tokio::test(flavor = "multi_thread", worker_threads = 4)]` for every test that boots a real topology (cell spawns, colony task). `worker_threads = 4` is a convention, deterministic, fast enough, reproducible in CI. The `current_thread` flavor (`#[tokio::test]` without args) is permitted only in pure unit tests without a topology (pure-function tests, schema validation, path resolution, etc.).

**Phase 1** first builds the `meclaw-testing` scaffold (`TestRoot`, `ColonyHandle`, `MessageBuilder`, an echo `MockCell`, `topologies::phase_1`) and uses it consistently from then on. Rejected were: no own crate but per-crate test helpers (duplication, inconsistent patterns), test helpers in `meclaw-core` under `#[cfg(feature = "testing")]` (forces consumers to feature toggles, mixes test code with library code), phase-demo topologies in the respective crates (demos combine several crates, a central place is cleaner).

**Phase 0** has the `meclaw-testing` crate only as an empty shell (see roadmap phase 0). Tests in phase 0 use exclusively `std` and `tempfile`:

- **CLI integration tests** under `crates/meclaw-cli/tests/*.rs` via `std::process::Command::new(env!("CARGO_BIN_EXE_meclaw"))`. Assertions on exit code, stdout, stderr, and on side-effect freedom (no `log.jsonl` in the cwd after `--version`/`--help`).
- **Unit tests** for the clap definition (`Cli::parse_from(...)` roundtrip) and for subscriber-setup functions directly in the modules, with `tempfile::tempdir()` for isolated log paths. No subprocess needed.
- `tempfile` is permitted as a dev dependency from phase 0 (see the tech-stack table).

---

## Roadmap (phases)

Concurrency-first: the actor substrate stands in phase 1, everything else builds on it. Every phase delivers an observable demo.

| Phase | Content | Demo |
|---|---|---|
| 0 | Cargo workspace, crate skeletons (all 6 crates: empty `Cargo.toml`s + a minimal `src/lib.rs` or `src/main.rs`; content arises phase by phase), CLI skeleton (`--version`/`--help`/flag parsing), logging (`tracing` → `log.jsonl`), the `rust-toolchain.toml` pin, `.cargo/config.toml` with `--cfg tokio_unstable` (for `console-subscriber` from phase 1) | `meclaw --version` |
| 1 | **Actor substrate**: the Actor trait, ActorHandle, colony as the central `HashMap<Path, ActorHandle>`, the supervisor (one_for_one), bounded mpsc (1000) with `block` backpressure, the `meclaw-testing` scaffold | 2 echo hives + a supervisor restart, observable via tokio-console |
| 2 | **Path resolution**: pure functions for `/`, `.`, `..`, `/colony/...`; the dead-letter cascade as a string op; the colony routing loop | an echo actor under `/a/b/c` addressable via absolute (`/a/b/c`) and relative (`../`, `./`) paths; a message to `/missing` lands in `/colony/dead_letters` |
| 3 | **Universal body format**: `system + messages[] + slots`, JSON schema validation, `parent_message_id`, content.header extraction | an echo hive with a universal body, header propagation visible |
| 4 | **Filesystem bootstrap**: colony scans the tree, reads `config.json`, registers cells, reads `params.graph` hints from hive scope markers | colony starts from an `examples/` tree, the registry shows all cells |
| 5 | **cell.db + state persistence**: SQLite per cell, `colony.db` as the registry- and message-log persistence, trace reconstruction via `parent_message_id` | replay of a trace after a restart |
| 6 | **Mutations**: diff ops (`add_nodes`/`add_edges`/`remove_nodes`/`remove_edges`/`swap_nodes`), variable substitution (`${ENV_VAR}`, `${ctx.*}`, `${uuid7:label}`) in mutation diffs, single-stage validation, scoped registry edits, `.staging/` + an atomic rename, the mutation log | a live mutation of a running topology, recovery after a crash with `in_flight` |
| 7 | **Tool cells (atomic-emitting, without `cell.db`)**: `bash`, `file`, `edit`, `web_fetch`, `web_search` | a tool chain via messages |
| 8 | **`llm` cell (atomic-emitting, with `cell.db`)**: provider translate **OpenAI only** (Anthropic deferred, no fixed phase reference), `system.*` slot accumulation, tool-definition extraction to `system.tools.*`, the error model (`finish_reason: "error"`) | the LLM cell answers via the OpenAI provider, tokens/cost in the header |
| 9 | **Tool cells (with `cell.db`)**: `store` (schema + seed + dynamic tables + CRUD), `code` (Python runner first, a programmable body constructor, optional multi-send) | a `code` script writes into `store`, queries back |
| 10 | **Long-running cells (double-task)**: `proxy` (Telegram first), `timer` (cron-like, second-accurate, one-time + repeating), `mcp` (MCP bridge with discovery) | a Telegram message triggers a topology, a timer schedule emits after `n` seconds |
| 11 | **Templates**: the `templates/` scanner, `template.json`, the templates registry in `colony.db`, `name@version` resolution, seed-JSONL bootstrap, `--rescan-templates`, the instantiation flow (copy + UUID v7 + `${ENV_VAR}` substitution) | instantiate from a template via a mutation |
| 12 | **HTTP API + web UI + blob storage**: blob (`text_id`/`messages_id`, 64-KB default), a blob cache per cell, daemon mode (`--daemon`), the HTTP API with `axum` as a thin translation layer over `/colony/*` (opt-in via `--api <bind>`), OpenAPI via `utoipa`, the operator web UI via `maud` under `/ui/*`, the `--validate` mode | `meclaw --api 127.0.0.1:7777`; the web UI shows the cells overview, trace, dead letters |
| 13 | **Hot/cold cell model (stateful)**: the states `NotYetSpawned`/`Awake`/`Asleep`, `cell.timeout` semantics (`0`/`>0`/`-1`), idle despawn, wake-on-message. Applies only to stateful cells; stateless and long-running cells do not have this model | thousands of stateful instances, few awake |
| 14 | **Example topologies**: a tool-loop (dispatcher + collector as `code` cells under a hive scope), RAG, simple multi-agent | the tool-loop runs end-to-end, inspectable in the web UI as a trace |
| 15 | **Builder-hive + AI builder**: the builder-hive as a multi-stage hive scope (an llm cell for NL understanding + a `code` cell for diff construction and validation + optional a template-discovery aggregator), takes natural-language requests, emits mutation diffs to `/colony/mutations`, uses template discovery (`/colony/templates`) | the builder-hive builds a sub-topology from a prompt |
| 16 | **Schema freeze + audit**: a schema final review, a docs audit, a cross-reference check, a license decision | a documentation-stable tag |

**Sub-phases** (emergent substrate intermediate passes like 6.5 / 7.5 or doc consolidations like 9.5) are not a roadmap component. The status per phase and sub-phase, including the current phase and the last Git tag, lives exclusively in `PROGRESS.md`.

---

## What meclaw will _not_ do (scope discipline)

- No distributed cluster setup (if needed later: NATS as a transport underneath, cross-colony federation as an additive extension).
- No GUI / no editor (VSCode + filesystem suffice; visualization via the API from external tools).
- No covering of non-agentic workflows (no Airflow replacement, no BPMN replacement).
- No own LLM inference (cells call external providers).
- No cell-to-cell topology knowledge (cells stay dumb).
- No compliance with foreign workflow standards, an own schema, JSON-first.

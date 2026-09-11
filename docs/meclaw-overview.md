# meclaw system description

This file is the specification of the substrate: the cell model, the edge model, headers,
routing, mutations and the lifecycle. On conflict with any other file in this repository, this
one wins.

Read it when you have a concrete question. Which `error_code` a refusal carries, which key a
param takes, which lane a hive declares, what a mutation validates before it commits. For the
concepts behind the vocabulary read [`meclaw.md`](meclaw.md) first, and for the words themselves
[`glossary.md`](glossary.md).

Four sections carry the rest: *Core principles*, *Cell model*, *Edge model* and *Headers and
body, the write model*. Read those four, then jump by heading.

## What meclaw is

A substrate for agentic systems whose topology is a directory tree. Every node is a cell (an actor), and a
directory with `type: "hive"` bounds authority and mutation. Cells rewrite the topology at runtime,
usually through a builder hive: a hive scope holding an llm cell, a diff constructor and a
validator. The hive turns a request in plain language into a mutation manifest. The colony applies
that manifest, and the hive never does. Cells reach one another only through atomic messages, and
every message carries the same body format. LLM inference, tool calls, persistent storage and
long-running bridges (Telegram, timer, MCP) are all cell types.

## Core principles

- The filesystem is the single source of truth. A directory tree with a `config.json` per node is
  the topology.
- A cell knows nothing about topology: not the sender, not the receiver, no hop history and no
  other cells. It knows its contract, its params and the current message.
- Messages are atomic. A trace is reconstructed from `parent_message_id` in the central message log,
  never carried in the message.
- The DSL is the CPU: domain-free, it knows routing, messaging and cell lifecycle. Everything on
  top is OS.
- Cells in `templates/` are classes, cells in the tree are instances. A cell is topologically
  neutral; what it is follows from where it stands. Instantiation copies a template into the tree.
- The graph decides routing, filtering and fan-out, exclusively through edges.
- Hierarchy is DSL, the substrate is flat. Directory nesting and hierarchical paths above, a central
  path registry in the colony underneath. Routing is one O(1) lookup, never a hop-by-hop cascade.
- Hives are scope markers. Directories with `type: "hive"` mark the authority and mutation boundary
  of their subtree. A hive has no actor type, no task and no mailbox. In the routing graph it is
  also a logical transit node, evaluated by colony.
- The hive is the abstraction boundary. An edge from outside addresses the hive, never a cell inside
  it. Whoever stands outside knows the hive's contract, which messages it accepts and which it
  emits, and nothing about its inside.
- Colony is the authority for lifecycle, registry, templates and routing. All cells register
  directly with colony. It writes `config.json` only on instantiation.
- Everything is a message on the data plane (cell-to-cell traffic, tool calls, one body format).
  Control commands to the colony (mutations, param updates, supervisor events, external API calls)
  are internally typed inbox commands with the same body model and the same sequential colony task.
  A cell emitting to `/colony/mutations` is dispatched directly.
- Tool loops are topology. llm cells have no inner loop.
- The colony modifies itself. Builder hives draft new topology at runtime; the colony applies it,
  through scoped mutations. The shipped builder drafts and never applies.
- No empty directories. Every directory in the tree needs a `config.json`, otherwise it does not
  exist.
- UUID v7 everywhere. IDs are time-sorted (messages, cells, templates, blobs, traces).
- UTC everywhere. No timestamp carries a local zone or an offset. The serialisation format is
  field-specific: envelope `created_at` as Unix seconds `i64`, the `timer` header
  `scheduled_at`/`fired_at` as RFC-3339 with `Z`, the blob sidecar `created_at` as Unix seconds in a
  string. Local time zones and `chrono-tz` are deferred.
- Agent-first, human-second. Discussions resolve in favour of the variant that is better for
  agent-driven builders.
- Parallel by default. Tokio multi-thread runtime, every cell and the colony as its own task,
  sequentiality only where the architecture sets it explicitly. Read the next section before any
  implementation decision.

## Concurrency and parallelism

> Parallelism is a basic assumption of every architecture decision and every cell or colony
> implementation, never a later optimisation.

### Runtime

Tokio multi-thread runtime (work-stealing scheduler), default flavor `multi_thread`, worker threads
defaulting to the number of CPU cores. No `current_thread` runtime in library or binary code. No
`block_on` in library code; library APIs are `async` throughout. Pure unit tests without a topology
are the exception.

### What runs as its own Tokio task

| Actor | Tasks per instance | Inbound |
|---|---|---|
| Stateful cell (`llm`, `store`, `code` with `cell.db`) | 1 long-lived task | own mpsc mailbox |
| Stateless cell (`web_fetch`, `web_search`, `file`, `edit`, `bash` one-shot) | 1 long-lived dispatcher task plus one short-lived worker task per message | own mpsc mailbox |
| Long-running cell (`proxy`, `timer`, `mcp`, `web`, `voice`) | 2 work tasks (handler and I/O), wrapped in one outer supervision task with exactly one `JoinHandle` | external mailbox plus internal channel |
| Colony | 1 long-lived task | own mpsc mailbox (central routing plus `/colony/*` endpoints) |
| HTTP API (`axum`) | Tokio-native task per request | translates each request into a message and hands it to colony |

Hives are not tasks, only scope markers in the filesystem (`config.json` with `type: "hive"`) and in
the path scheme. Routing, authority boundaries and mutation scoping run off path prefixes. A hive
path addressed as a message target is a logical transit node in colony's one routing layer, never a
mailbox delivery.

Tokio tasks cost about 3 KB of stack and are not OS threads. Thousands of sleeping cells cost
practically nothing beyond their mailbox channels in colony's registry.

### What the architecture guarantees to be sequential

These islands hold by construction. Cell and colony code may rely on them and must not establish
them again: no `Mutex`, no `RwLock`, no atomics, no defence against reentrancy.

- Within a cell, one `handle()` call runs through completely before the next starts, from the mpsc
  pull semantics of the one cell task. Cell state is effectively single-threaded.
- Within the colony, routing lookups, mutations, registry and template operations run sequentially
  through colony's single task. A mutation cuts in atomically between two routing steps; other
  messages wait in colony's mailbox. No parallel `config.json` writes, no parallel staging
  directories for one mutation.
- In a long-running cell, inbound (mailbox) and outbound (provider event) arrive sequentially at the
  state-holding handler task. The I/O task only buffers events into an internal channel.

### What the architecture runs in parallel

Everything not listed above runs in parallel across all worker threads of the Tokio scheduler.

- Between cells: all cell tasks are independent. Cell A runs an LLM call while cell B runs a DB
  query, on up to N cores at once.
- Fan-out: when a cell emits and several outgoing edges match, colony dispatches in one routing step
  to all of them; the receiving tasks continue independently.
- Stateless cells: the dispatcher task spawns a short-lived worker task per incoming message. Worker
  tasks hold no persistent state and end after the emit. The per-cell limit is
  `params.max_concurrency`.
- Long-running cells: the I/O task runs independently of the handler task. A 30 s Telegram long poll
  does not block an incoming meclaw message.
- Colony and cells: colony decides routing sequentially but hands messages over immediately;
  receiver processing runs in parallel.

### Long-running cells and the double task

Cells of type `proxy`, `timer`, `mcp`, `web` and `voice` have a cell-internal double-task setup,
mandatory for these types. From the topology's perspective the cell is one address with one external
mailbox.

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

The I/O task polls the provider (Telegram long poll, MCP stream), waits for the next cron firing via
`tokio::time::sleep_until`, or holds a WebSocket. On an event it serialises to an internal event
type and sends it over the internal mpsc to the handler. It never touches cell state and holds no
`outputs_tx`; every topology-directed emission runs through the handler.

The handler task does `tokio::select!` over the external mailbox and the internal channel, so both
sources become one sequential stream and no mutex is needed. It holds the `outputs_tx` (cloned once
at spawn) and calls `cell.handle(...)` or `cell.handle_event(...)`, which call `outputs_tx.send`.

Inbound topology traffic and outbound provider events therefore never touch the same state at once.
A long mailbox backlog piles provider events into the internal channel instead of breaking the
provider connection. Output backpressure cascades backwards: a full `outputs` mailbox blocks the
handler on `outputs_tx.send`, the internal channel fills, the I/O task blocks on its push, and
external polling throttles itself. That is liveness-safe below the saturation boundary described
under Backpressure.

If either sub-task panics, colony's supervisor re-instantiates the whole cell (`one_for_one`) and
both sub-tasks are set up anew.

The `run_io` lifetime contract (A1') says the I/O task function runs for the entire lifetime of the
cell and returns only when the cell as a whole ends: teardown, disconnect or panic. A clean,
voluntary return while the cell is still alive is a contract violation, because the outer `select!`
over both `JoinHandle`s then wins on the I/O completion and aborts the surviving handler along with
unprocessed events (the io-finish-first loss class). No shipped cell triggers this; all I/O loops
are endless.

Colony's mailbox is single-consumer and sequential, so at high routing throughput it becomes a
potential bottleneck, far above the load tier this roadmap targets (LLM-centric flows with
second-scale latencies per step). The solution paths are additive: a read-mostly routing table, edge
evaluation pulled out of the routing path, or several colony instances serving different subtrees.

### Backpressure

Bounded mpsc mailboxes (default 1000 per cell) plus `block` as the only strategy: when a mailbox is
full the sender waits (`send().await`) until there is room. Backpressure propagates backward through
the graph without silent message loss on the live path, without drop logic and without a per-cell
strategy choice. The cell panic and restart path preserves the waiting messages too (GH #18); only
the message being processed is lost.

Two backward cascades exist symmetrically.

1. Inbox backpressure. A full cell mailbox makes the sender (colony during routing) block, colony
   drains its routing inbox more slowly, and upstream cells writing to colony block in turn.
2. Output backpressure. Each cell writes its emits into a central `outputs` mailbox. A full one
   makes the cell block on `outputs_tx.send().await`, so the cell drains its own inbox more slowly,
   which lands in case 1.

A fully dead cell is caught by the message timeout plus the `one_for_one` restart.

The honest limit: `block` backpressure is liveness-safe as long as inflow stays at or below outflow.
Under sustained over-saturation across a closed wait chain (cell A blocks on B, B on C, C on A) a
wait-cycle deadlock arises and stands permanently, and backstop-less long-running source cells
(`message_timeout` `0` or `-1`) in the cycle are particularly exposed. TTL counts routing hops, and
in the deadlock no message flows, so it does not catch this. The case lies beyond the roadmap load
profile and is registered as a post-MVP item.

For stateless cells, inbox backpressure plays out at the dispatcher, which slows itself down via its
`Semaphore`; worker tasks can only be stuck on output backpressure.

### What cell implementers can rely on

- No `Arc<Mutex<...>>` over cell state.
- No `RwLock` negotiation.
- No atomics, no lock-free data structures.
- No reentrant calls into one's own `handle()`.
- No defence against two messages arriving at once.

From the cell's perspective the world is single-threaded; the parallelism lies in the Tokio
scheduler and in colony's routing. Whoever writes `Mutex` in cell code has not understood the
concurrency model, and code review rejects it.

### What cell implementers have to do

- `handle()` is async and all I/O goes through `.await`. A synchronous `std::thread::sleep`, a
  blocking DB driver or a synchronous network call blocks the Tokio worker thread and sabotages
  every other task on it. Forbidden.
- Long CPU bursts (more than 1 ms without an `.await` point) get an explicit
  `tokio::task::yield_now().await` or move into `tokio::task::spawn_blocking`.
- No `block_on` in cell or colony code, not even in a test that boots a real topology.
- No assumption about ordering between cells. Which of two parallel emits arrives first is the
  scheduler's business. Whoever needs ordering builds it in the topology, with a collector hive and
  a correlation ID.

Beside cell and colony tasks the substrate may run process-wide infrastructure actors, such as the
token broker that serialises the OAuth refresh for all `llm` cells. They follow the same rule: one
task, state inside that task, no lock.

## Authority model

The colony is the only write authority. Hives are scope markers in the filesystem, not actors; they
define the authority and mutation boundary of their subtree without owning a mailbox or routing
logic. Colony evaluates the hive out-edges, and there is no hive-owned evaluation code.

| Authority | Carrier |
|---|---|
| Reads and writes `config.json` (only on instantiation) | Colony |
| Instantiates cells (template copy, UUID assignment) | Colony |
| Holds the central `HashMap<Path, ActorHandle>` registry | Colony |
| Holds the mutation log (audit trail) in `colony.db` | Colony |
| Routes messages (O(1) lookup with upstream path resolution) | Colony |
| Lifecycle of all cells (start, stop, restart) | Colony |
| Templates registry | Colony |
| `.env` substitution at bootstrap | Colony |
| Central message log (filterable by path prefix) | Colony |
| Authority scope for mutations (path-prefix-based) | Hive (scope marker) |

Transit edges of a hive (edges with `from = <hive-path>`) live in colony's one `EdgeTable`, the
same data structure as cell edges, indexed by `from`.

Colony writes `config.json` exclusively on instantiation (template copy with UUID assignment and
`${VAR}` substitution); afterwards it is a bootstrap snapshot and never touched again. The live
state of a cell lives in its `cell.db`, the global topology truth in colony's registry (in memory)
and in `colony.db` (persisted).

### Database isolation

One cell, one database (ruling 2026-08-17). A cell touches only its own `cell.db`; another cell's
`cell.db` and `colony.db` are closed to it, and a read counts as access. Whoever needs information
out of another cell's state sends a message: a message is versioned (UBF), logged (`message_log`),
authorised (edge) and consistent at one point in time, while a reader of foreign tables sees state
changing under it without the owner's invariants. This binds all Rust code, the substrate included.
In practice: `ATTACH` does not exist anywhere in the tree, every `cell.db` resolves against the
directory of the cell opening it, and `meclaw-api` has no SQLite dependency at all.

For topology knowledge the route is `/colony/graph`: nodes and edges as a reply to a message,
out of colony's in-memory registry, without touching a database.

For counts out of the colony's ledgers (message log, dead letters, mutation log) the route is
`/colony/ledger` (GH #267, ruling Q14 of 2026-08-21): aggregates over one time window, sums and
counters, no raw rows and no header contents, answered as a reply to a message. Taking the same
number out of `colony.db` with a `SELECT COUNT(*)` is as wrong as reading the graph there.

The rule has no exception (GH #160, ruling 2026-08-17); the last one, the `vault` cell's unlock
attestation, is gone. A cell that needs a fact about its own place in the graph declares it in its
contract and receives, at spawn, a read-only capability from the authority that owns the edge table:

```json
"consumes": { "topology": { "inbound_edges": { "type": "array", "required": true } } }
```

`consumes.topology` is a capability declaration in the same grammar as `body`, `context` and `hop`,
and no message compartment: nothing in it is validated against an incoming message, and a key
declared there never makes a message invalid. The only key the substrate knows is `inbound_edges`,
the `from` paths of every edge pointing at the cell's own path (`meclaw_colony::NeighbourhoodView`,
answered from colony's in-memory `EdgeTable`). Not the graph, not a scope, not its own outbound
edges, and never another cell's. Without the declaration the handle does not exist. An unverifiable
neighbourhood (no handle, no answer, a timeout) is treated exactly like a wrong one and the `vault`
stays LOCKED.

### Instantiation

**Instantiation and cell_id stability**: instantiation happens exactly when no cell directory exists
at the target path. When processing a graph, colony checks per declared node whether the directory
exists, at bootstrap for `params.graph` as well as at runtime for a mutation diff. If it is missing,
colony copies the referenced template to the target path and assigns a fresh UUID v7 as `cell_id`,
with `${VAR}` substitution. If it exists, the operation is a reconnect or resume: no new `cell_id`,
`config.json` untouched, `cell.db` resumed. A resume requires type equality; a `type` deviating from
the template is rejected with `resume_type_mismatch`. A `cell_id` is assigned exactly once and never
changed or reassigned. Templates themselves are id-less. On instantiation colony records the node
with its `cell_id` in `colony.db`; entries there are never deleted, only marked inactive.

The bootstrap instantiates exactly one class, the resolved `ref` marker (GH #424). A `config.json`
with `cell.type: "ref"` (or the key `cell.template`) in the root tree names the template that shall
stand at this position. The first boot resolves it and materialises it through the chain a mutation
takes (`mutation/subtree.rs::stage_subtree`), with the same registry resolution, version pinning and
refusals (`template_missing`, `template_ref_cycle`, `schema`). On a reboot the same marker is an
unresolved remnant and, per ruling A5b, is reported and never grown. The marker consumes itself, so
a second boot finds nothing left to grow and a node unhooked by `remove_nodes` cannot rise again;
the no-delete policy is untouched, because a marker has no `cell.db`, no `cell_id` and no registry
row.

Below `params.graph` the boot parser knows only `edges`. `GraphHints`
(`crates/meclaw-colony/src/config.rs`) sits under `deny_unknown_fields`, so a hive `config.json`
carrying the `nodes` block documented under Graph schema is a hard boot error. The only declaration
form at boot is the `ref` marker.

### The mutation flow

1. Someone (a cell, a builder, the external API) sends a mutation message to `/colony/mutations`
   with a diff. The target is the only thing that identifies it; no header is read. The diff carries
   a path prefix as scope, typically the path of a hive scope marker. The sender learns the outcome
   from the verdict reply to `reply_to`, if it set one.
2. Colony validates in a single stage: schema, match patterns against the current registry, cycle
   check in the post_state, edge schema compatibility, template existence, filesystem preparation,
   `.env` variables. On error it logs and replies to `reply_to` if set.
3. On success colony marks the mutation `in_flight` in `colony.db`, builds all new cell directories
   under `{root}/.staging/<mutation_id>/<cell_name>/` (`config.json` with substituted values and
   assigned UUIDs, possibly `cell.db` from seed), then moves them one after another with `rename(2)`
   to their final paths. That is atomic per directory on POSIX and not transactional across all of
   them: a `rename(2)` failing after others succeeded leaves the earlier renames in the live tree,
   and the substrate strict-fails loudly on that half-state. Registry edits then run: new cells
   spawn and register under their path, disconnected cells are marked inactive while the registry
   entry and the filesystem remain, and the tasks end gracefully. Then the mutation is marked
   `committed`. On a crash between `in_flight` and `committed` a recovery pass runs at the next
   startup.
4. Cell inits run asynchronously. On init failure the cell restarts `one_for_one` (5 retries by
   default), then takes `failed` status. Symptoms are visible via the routing cascade (`reply_to`
   and `/colony/dead_letters`). The mutation verdict goes as a reply to `reply_to` if set:
   `{"mutation":{"id":…,"outcome":"committed"}}` on success, `"outcome":"rejected"` plus
   `error_code` and `details` on rejection. The ack covers the mutation commit, not the success of
   the asynchronous cell inits.
5. The door leaves a receipt (GH #553, ruling R-0904-1). The verdict in step 4 reaches only whoever
   set `reply_to`, and neither `POST /colony/mutations` nor `meclaw --apply` sets one. When
   `colony.json` carries the `mutation_receipts` key, the door additionally emits one terminal event
   to the hive named there: `hop.route == "mutation_committed"` plus `mutation_id`/`mutation_ids`,
   `outcome`, `scope`, `form`. It knows no recipient; the hive's edges fan it out. Without the key
   nothing changes. **And the boot is the first one** (ruling O-0904-2): `form: "boot"`, nil id,
   right after `InitialApply`.

Cross-colony federation (several `meclaw` instances with different colonies talking to each other)
is post-roadmap. The architecture will not prevent it.

A colony carries many organisations and exactly one OS, and the OS hands out what is system-near
(ADR-0022, GH #543). An organisation is a namespace: no port band, no port assignment, no
configuration surface of its own; it asks the OS. System-near means scarce, colony-wide, and such
that two holders of one is a collision: a TCP port, a bind address, a socket, a mount name. How an
organisation asks is open. Today the builder at the shell level (`templates/meclaw-os`) gives
every grown member the mount of its screen as `<member>-display`, out of the `screen_mount`
recipe knob. The builder is part of the OS (ADR-0015), so that is never an organisation's own
right.

## Graph schema

The graph is a directed graph of nodes (cells, registered in colony's `HashMap<Path, ActorHandle>`)
and edges (routing rules between paths, held in colony's edge table).

The same schema describes the graph in two write usages.

1. Bootstrap. `params.graph` in the `config.json` of a hive scope marker
   provides the initial **edges** for its subtree; colony reads them at the filesystem bootstrap.
   The initial desired state for a position comes from a `cell.type: "ref"` marker in the root tree
   instead, resolved on the first boot through the chain a mutation takes (GH #424). At boot,
   `params.graph` carries the edges and a `ref` marker carries the node.
2. Runtime diff. A builder sends a mutation message with a diff to `/colony/mutations`. Colony
   computes the post_state from it and executes scoped registry edits.

### Schema

The `nodes` key in the block below is built **only in the mutation usage**; in a hive `config.json`
it aborts the boot.

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
      "modifier":  { "set_context": { "<key>": "<CEL>" }, "delete_context": ["<key>"], "set_hop": { "<key>": "<CEL>" }, "delete_hop": ["<key>"] },
      "default":   "<Boolean, default false>"
    }
  ]
}
```

`nodes` is a mapping `name → { template, override_params? }`. The name is the path component and
must be unique within the same hive scope; collisions are rejected during mutation validation.
`template` is a reference of the form `<name>` (the one registered version) or `<name>@<version>`.
`override_params` is optional and overlays the template's default params.

The key exists in the mutation diff only, and that is a decision, not a gap (GH #424). At boot you
declare a node with a `cell.type: "ref"` marker in its place, at runtime with a mutation diff
(`add_nodes`); a `nodes` block at boot would be a second instantiation language with its own
name-to-path lookup and its own override addressing. The schema stays in full because it describes
the mutation usage.

`edges` is a list and its order is irrelevant. Required fields are `from` and `to`, paths relative
to the declaring hive scope, at any depth within it (`./name` as well as `./unit/dispatch`),
symmetrically in `params.graph` and in the mutation diff (R12 ruling 2026-06-11). Optional fields:
`condition` (a CEL boolean, default `true`), `modifier` (an operations object with `set_context`,
`delete_context`, `set_hop`, `delete_hop` and `restore_ttl`, default `null` meaning identity), and
`default` (a boolean, default `false`; `true` makes the edge a default edge, consulted only after no
regular out-edge of the same sender fired, GH #283). Edges operate strictly on the header layer.

`.` names the scope root itself (GH #487). `{"from": "./firewall", "to": "."}` is the ordinary
spelling of a lane that leaves this level: `.`, and the `./` that means the same, resolves against
the declaring scope by the same rule as every other relative path (`Path::resolve`, stay at the
sender). In a bootstrap `params.graph` that is the hive whose `config.json` carries the schema, in a
mutation diff the `scope` of the declaration. It is resolved at the point of use and never in the
document, so a diff keeps the spelling it was submitted in. A scope whose root is not a registered
node, neither a cell nor a hive scope, stays `edge_schema`:
the rule widens the vocabulary, it invents no node. Where a hive scope marker sits at the root scope
`/`, `.` names it like any other.

Write side and read side have different shapes. In the write schema (bootstrap and mutation diff)
edges reference nodes by name (`./<name>`); UUIDs arise only at runtime, node UUIDs assigned by
colony on instantiation and edge UUIDs on creation. The read schema (`/colony/graph?scope=...` or
HTTP `GET /graph?scope=...`) additionally shows `id`, `path`, `graph_version` and more, the
runtime-projected form of the same structure.

## The hive boundary

The rule: an edge that crosses a hive boundary has the hive as its endpoint, never a cell inside it.
Access from outside is abstract and functional: an edge asks for something by content, and the inner
edge that receives the request knows what to do about it. This holds for all hives and all
templates, ruled 2026-08-18 (GH #197, GH #200) and written down here by GH #227. A hive without
`params.ports` is one where the rule holds and nothing checks it.

### The three requirements

Normative, in the order they get broken in practice.

1. The address is the hive. An edge from outside must have the hive path as its endpoint.
   `<hive>/<cell>` is not an address, and `<hive>/<subhive>/<cell>` is less of one, including where
   the substrate still resolves it today for want of a declaration. Where a declaration exists, the
   substrate has enforced the sentence on **delivery** too since GH #612: a message from outside the
   colony naming a cell inside a hive with `params.ports` is refused with `hive_boundary` rather
   than delivered past the door. What is delivered is what the hive declared itself — an entry in
   `params.ports`, or a connect point of a `params.contract` `accepts` lane, and then only on that
   lane. A **level** inside it is no exception: a hive inside a sealed hive is refused exactly like a
   cell, because otherwise the nested rim would be the way around the boundary — five shipped
   templates carry such a hive through a `ref` marker. The hive path itself is of course still an
   address; it does not lie inside itself. Internal traffic is not judged: inside the hive the
   graph is the hive's own business.
2. A lane is named functionally. A lane name (`hop.route`, declared in `params.contract`) must say
   what the caller wants, never where it lands inside. `writer`, `recall`, `render`, `refresh`,
   `policy`, `invoke` and `meter` are inner cell names; a lane is called `in_turn`, `in_batch`,
   `in_brief` or `in_propose`. Renaming a port into a lane of the same name satisfies the letter of
   requirement 1 and leaves the caller knowing the layout.
3. The inner edge is the only place structure may be known. Which cell handles a request becomes a
   condition on the hive's own distributing edge (`{"from": "."}`), where it is replaced together
   with the inside it describes.

A template is a class, and a class is interchangeable: one implementation replaces another with the
same contract and a different inside, without touching the callers. A caller that draws an edge to
`<hive>/keeper/stamp` breaks on the next replacement.

### What a template author has to do

Every hive template that ships satisfies this as of 2026-08-18
([#197](https://github.com/mmeyerlein/meclaw/issues/197),
[#228](https://github.com/mmeyerlein/meclaw/issues/228)), so a new one has worked examples.
`templates/README.md` § The hive boundary says the same where a template author reads; the order in
which an existing hive is brought there is in `rewiring.md` § Putting an existing hive behind its
boundary.

Four things, all checkable, all in the template's own files.

1. `params.ports: []` in the hive marker's `config.json`. The empty list is the statement that the
   hive path is the only address. A hive with no `ports` key is unsealed and therefore unfinished.
2. Doors from the inside: one edge per accepted lane, with `"from": "."`, a `condition` testing the
   lane, and a `to` naming the inner cell that serves it. That is requirement 3, written down.
3. `params.contract` with `accepts` and `emits`, in lane names per requirement 2. That is the list
   the substrate checks the doors against (`config.md` § `params.contract`).
4. No address in its prose that the boundary would refuse. `template.json` and the README describe
   lanes, not cells, and a `from:` or `to:` in a `description` slot is a wiring instruction (GH #203;
   the test `gh203_documented_port_addresses` puts that question to the real boundary validator).

### What it looks like

Outside, the caller knows a hive and a lane and nothing else:

```json
{"from": "./proxy", "to": "./talky",
 "modifier": {"set_hop": {"route": "'in_turn'"}}}
```

Inside, the hive distributes on its own, with edges whose `from` is the hive itself. Here, and only
here, is it written down which cell serves the lane:

```json
{"from": ".", "to": "./session-keeper",
 "condition": "has(hop.route) && hop.route == 'in_turn'"}
```

Where the inner target is itself a hive, the same rule applies one level down: the door names the
sub-hive's path and a lane, never a cell inside it.

Both of the following write structure outward, the second more quietly than the first:

<!-- gate:counter-example refused=./talky/session-keeper/stamp -->
```json
{"from": "./proxy", "to": "./talky/session-keeper/stamp"}
{"from": "./proxy", "to": "./memory",
 "modifier": {"set_hop": {"route": "'writer'"}}}
```

The first breaks requirement 1 and is refused with `hive_port_boundary` wherever ports are declared.
The second addresses the hive correctly and still breaks requirement 2: `writer` is the name of a
cell inside, and a hive that rebuilds its write path takes the lane with it. Functionally the lane
would be `in_episode`, take this turn into your memory.

Both shapes are already carried by the substrate: a hive path may be the `from` and the `to`
endpoint of an edge, and colony evaluates it as a transit node. The hive gets no mailbox and no
task; the evaluation is a branch of the one routing layer.

From outside the colony, the HTTP ingress asserts the same lane through the `hop` field of `POST
/messages` (GH #175), so a freshly sealed hive is verifiable without addressing any interior cell:

```json
{"target": "/talky", "hop": {"route": "in_turn"},
 "body": {"messages": [{"origin": "user", "type": "text", "text": "…"}]}}
```

A hive's contract is a statement about messages: which `hop.route` values it accepts, which it
emits, what a parent must promote to `context.*` first. What lies behind the boundary may change at
any time.

### What this means for `params.ports`

`params.ports` (GH #133) seals a hive: a mutation whose edge reaches past a declared port into the
interior is rejected with `hive_port_boundary`. That is the enforcement of the rule, which holds for
an undeclared hive too. A port is the name of a lane (see § The three requirements): `ports: []` and
`params.contract` are two halves of one shape, the address is the hive path and the lane is the
port. A hive whose `ports` are still interior cell names is mid-migration; a hive with no `ports` at
all has not started one.

Slots are a port that may stand empty (GH #285). A port entry has two forms: the short name of a
direct child as a string, or the slot form as an object (`{"name": "gen", "slot": true, "unbound":
"park"}`). The second declares the address before anything stands at it, so a hive that fills a lane
only later can still describe its own attachment surface, and whoever redeems the promise is a later
mutation.

The declaration buys exactly two exemptions: an edge onto the slot is not a dangling endpoint at
boot, and `add_edges` may wire it before it is filled. A path that is not declared as a slot and has
no occupant stays a hard error under `--validate-strict`. A slot is furthermore not a node: it is a
valid `add_edges` endpoint, and never a `remove_nodes` or `swap_nodes[].match` target (both answer
`match_no_hit`). Emptying it and wiring the address in the same diff commits: the declaration
outlives its occupant, and what remains is the declared empty slot.

`unbound` says what happens to a message that reaches the unbound slot over an edge. `drop` discards
it silently, `error` files a `slot_unbound` dead letter, and `park` holds it FIFO until the binding
and then releases the queue in emission order, bounded by `colony.json slot_park_max` (default 64;
above the bound the newest arrival is refused as `slot_park_overflow`, and a shutdown discards
whatever is still parked). Over an edge is the limit of the promise: a message addressing the slot
path directly from outside ends as `unresolved_path`. Slots have the same reach as the port
boundary, so a slot declared in the never-sealed root scope buys no exemption. Both forms and the
bound in full: `cell-types.md` § `hive`, Slots.

### The hive contract (`params.contract`)

The contract is a list of lanes in `params.contract`, a form the substrate can check (details and
the enforcement table: `config.md` § `params.contract`):

```json
"contract": {
  "accepts": [{"route": "in_batch", "context": ["session_id"],
               "because": "one closed session as a single write batch"}],
  "emits":   [{"route": "episode", "because": "one message per turn"}]
}
```

Three things are enforced, all of them for mutations only and all of them through the real router
instead of a text comparison (`hive_contract`).

1. An edge onto the hive path whose `set_hop.route` is constant must name a declared lane. The
   typo is refused instead of becoming a dead letter. Which list applies is decided by direction,
   not by the target (GH #602): an edge coming from outside ENTERS and is measured against
   `accepts`; an edge coming from a node strictly inside the hive LEAVES — the message crosses the
   hive path outwards — and is measured against `emits`. An edge whose `from` is the hive path
   itself leaves nothing and stays an entry.
2. Every `accepts` lane must have a door (`{"from": "."}` inward).
3. Every `emits` lane must lead back out through the hive path, either carried by a message that
   already has it or created by the out-door itself (GH #176). A door that recognises
   `hop.finish_reason` and turns it into a lane with `set_hop.route` is an exit for that lane; a
   door that names a different lane is not.

(2) and (3) keep the contract from decaying into decoration: rearranging the inside is free,
rearranging it so a promised lane loses its door is not. At boot it only warns, because the birth
topology is sovereign, the same rule GH #133 and GH #147 follow — and since GH #602 boot warns about
both halves; it used to run (2) and (3) alone, so a rim edge the mutation path refused was born here
in silence. No validator can check requirement
2, since `writer` is as valid a string as `in_episode`; that is where the rule depends on a reader.

### Where the library stands

Topologies built before this rule wire almost exclusively into internals: in a real colony
(2026-08-18) exactly one of 129 edges addressed a hive. The shipped library is not there yet either,
in three stages:

| State | Templates |
|---|---|
| `ports: []` plus `contract`, done | `collector`, `session-keeper`, `summarizer`, `memory-drain`, `affinity` (with the `ref` sub-units inside `talky` and `cogny`; the `summarizer` is part of no composition since `talky@4.3.0`) |
| ports are inner cell names, migration under way (GH #197) | `canvy`, `memory-hive`, `access`, `argus` |
| no `ports` key, migration not started | `talky`, `cogny`, `receptionist`, `firewall` and others |

For new hives the rule applies without exception. Existing ones are converted one at a time (state
the contract, build the `{"from": "."}` distribution inside, move the callers onto the hive, declare
`params.ports`), each conversion its own reviewable change. The order, and the five traps that are
the same every time, are in `rewiring.md` § Putting an existing hive behind its boundary.

Enforced today: `hive_port_boundary` where ports are declared and `hive_contract` where lanes are
declared, both for mutations only (the boot `params.graph` is the author's sovereign birth draft and
only warns). Not enforced today: everything else in this section, in particular whether a hive is
sealed and carries a contract at all, and what its lanes are called. Not enforced does not mean
optional. The rule binds, the substrate only checks part of it.

## Mutation format

A mutation is a message to `/colony/mutations` whose body carries a diff plus a scope. The target is
the only thing that identifies it: dispatch goes by path alone and no header is read. `hop.msg_type ==
"mutation"` is a widespread application convention (templates set the key and condition their
mutation edge on it), and meclaw-core checks it nowhere. Colony validates in a single stage,
executes, and replies to `reply_to` on error. Entry paths today are the HTTP edge and the internal
bootstrap inbox command. A message emitted by a cell to `/colony/mutations` is dispatched directly
(W2b ruling 2026-06-12): the outputs arm recognises a `/colony/*` target and routes it through
`route()` to the virtual endpoint before edge evaluation, so no out-edge is needed or possible. An
unknown `/colony/<x>` endpoint lands in the DLQ as `colony_endpoint_unimplemented`.

```json
{
  "scope": "/main/agent-pool",
  "diff": {
    "add_nodes":    [ { "name": "...", "template": "...", "override_params": {}, "birth": "active" } ],
    "remove_nodes": [ { "match": { ... } } ],
    "add_edges":    [ { "from": "...", "to": "...", "condition": "...", "modifier": { "set_context": {}, "delete_context": [], "set_hop": {}, "delete_hop": [] }, "default": false } ],
    "remove_edges": [ { "match": { ... } } ],
    "swap_nodes":   [ { "match": { ... }, "with": { "template": "..." } } ],
    "move_nodes":   [ { "match": { "name": "..." }, "to": "..." } ],
    "add_templates":[ { "name": "...", "files": { "template.json": "...", "config.json": "..." } } ],
    "seed_rows":    [ { "target": "...", "table": "...", "rows": [ { "<column>": "..." } ] } ]
  },
  "ctx": { "key": "value" }
}
```

The `diff` takes exactly these eight keys. A key no operation reads, whether a typo (`add_node`), a
key from a newer schema version or a guessed piece of vocabulary, is refused with `error_code:
schema`, and the refusal names the unreadable key and the legal ones. The check runs before
substitution and therefore before a single byte moves; in a manifest it runs at the entry's
position, with the earlier entries left applied.

`scope` is an absolute path prefix, typically the path of a hive scope marker. All relative paths in
the diff resolve against it, and paths that would lie outside it are rejected during validation.

`diff` contains the change operations. Order is irrelevant: colony computes the post_state after
applying all operations and validates that, never partial states. An `add_edges` edge may therefore
name any address the diff itself puts a node at, which is all three creating operations:
`add_nodes[].name`, the instantiate form of `swap_nodes[].with`, and `move_nodes[].to` (GH #198).
Relocating and wiring in one committed mutation is what `move_nodes` was built for. The converse
holds for addresses the diff vacates (`remove_nodes`, `swap_nodes[].match`, `move_nodes[].match`, GH #194):
those are no longer endpoints afterwards, and an edge naming one is rejected. The existing-node form
of `swap_nodes[].with` (no `template`) puts nothing anywhere, and `add_templates` and `seed_rows`
claim and vacate nothing, so neither contributes to the post_state.

`ctx` provides values for `${ctx.<key>}` substitutions in the diff, resolved when applying the diff,
before validation runs.

### The manifest body form

`/colony/mutations` additively takes a second body form (GH #422), the manifest: an ordered list of
ordinary mutation bodies in one body.

```json
{ "manifest": [
    { "scope": "/",   "diff": { "add_nodes": [ … ] } },
    { "scope": "/os", "diff": { "add_edges": [ … ] } }
] }
```

It is recognised by exactly one key, the top-level `manifest`. A body without it takes byte for byte
the path it has always taken; no other key discriminates. A body carrying `manifest` and
`diff`/`scope` is `schema`.

Every entry is byte for byte one single-form body. No `kind`, no `id`, no manifest-wide `ctx`; each
entry brings its own.

The colony rolls it off in order, every entry through the same one-stage validation a single body
gets, stopping at the first refusal, one receipt. An entry is judged against the tree the entries in
front of it grew: the registry, the hive scopes and the template library entry *k* changed are what
entry *k+1* sees. A manifest is therefore where an order carries semantics, and two submissions in
the same turn have no order at any door (measured: GH #585). Nothing rolls back: what applied stays
applied, and the receipt says at which position it stopped. The audit carries one
`mutation_log` row per applied entry plus the refusing one's `rejected` row.

```json
{ "manifest": { "outcome": "committed", "applied": 5, "ids": ["…","…","…","…","…"] } }
```

```json
{ "manifest": { "outcome": "rejected", "applied": 3, "ids": ["…","…","…"],
                "failed_at": 4, "id": "…", "error_code": "edge_schema",
                "details": "…", "remaining": 1 } }
```

`failed_at` is 1-based, because an operator counts entries and not indices. `remaining` is the
number of entries never looked at, and `id` is the refused entry's mutation id if it got one. No new
`error_code` is minted: the slot carries the refusing entry's own code, and a form-broken manifest
is `schema`. HTTP mapping is as for the single form: `committed` gives 200, `rejected` gives 422.

Manifest v1 carries mutations only. "Applied" has no meaning for a message, and `/colony/mutations`
is the mutation door, so arbitrarily addressed traffic through it would be the mixing § Scope,
concurrency and permissions rules out. Messages go through `POST /messages` or over an edge. It
stays additively extensible: an entry carries no `kind` discriminator today, and whoever wants
message entries later introduces one and lets its absence mean `"mutation"`.

Large bodies travel over the existing blob offload; the mutation door resolves a `Body::Blob` before
it dispatches (GH #432).

### Mutation operations

| Operation | Effect |
|---|---|
| `add_nodes` | Instantiate new cells in the scope (template reference, optional `override_params`). On a **single-cell template** `override_params` is a flat params object. On a **subtree template** it is addressed (GH #140): the keys are the paths of the cells inside the template, `""` being the subtree root, as in `{"assemble": {…}, "window": {…}}`. A key that names no cell of the template is rejected pre-destructively with `schema`, and the message lists the cells that do exist. One level down the same rule holds (GH #294, ruling Q6): every param key of an override entry must be a param the addressed cell carries under `params` in its template `config.json`, otherwise `schema`, and the message names the param, the cell, its cell type, the template and the params that do exist. It is a pure existence check on the template's raw `params` object, so instance substitution is irrelevant here; types and a `because` may arrive later as declarations. A cell with no `params` block has the empty set and refuses every override. Both forms go through the same check in validation, so they cannot drift apart. The wrong notation is named as a notation (GH #436): the path-keyed form on a single-cell template is refused with the sentence that a single-cell template takes a flat params object, under the same `error_code` (`schema`) and at the same pre-destructive position. Consequence for template authors: a param that is meant to be set per instance has to be declared in the template, and a default value is enough (`null` is a legal placeholder for an opt-in such as `ports`). `${ctx.*}` substitution remains the way for values the template itself distributes. Optional `birth` (GH #437) takes `"active"` (the default) or `"inactive"` and sets the entry's instantiation activity, not its hot/cold status. A node born inactive is registered, addressable and persisted inactive; **no task** is built, so a long-running cell does not open its upstream at birth. On a subtree the declaration holds for every cell of the tree, because a unit is born whole. An unknown value is rejected pre-destructively with `schema`. The declaration is durable (GH #491): it leaves a marker in the registry that every connectivity recompute honours, and it survives a restart. The wake is the existing reconnect, with no new operation and no new message, by the next mutation that addresses the node itself; a mutation elsewhere in the tree leaves it asleep. `swap_nodes[].with` has no `birth`, because a successor born inactive would leave the swapped edges pointing at nothing. |
| `remove_nodes` | **Addresses cells.** Removes every edge naming the matched path itself at one end, so the node is disconnected and marked inactive. Registry entry, filesystem and `cell_id` remain (no-delete). **Correction (GH #390):** this said "including subtree cascade at hives"; that is retracted, in both of the halves a reader took from it. (1) A hive path is not a `remove_nodes` target. `match.name` is resolved against the cell registry only; `swap_nodes` beside it asks the hive scopes too, `remove_nodes` does not. A hive has no registry row, so the entry is `match_no_hit`, and because validation is all-or-nothing the whole mutation fails on it, the well-formed entries beside it included. Edges with a hive at one end go through `remove_edges`, whose pattern is evaluated against the edge table and does not care what kind of node an endpoint is. (2) Edges do not cascade. Removal runs on exact path equality, so an edge between two **descendants** of the matched node survives. That is the same intent `swap_nodes` states (GH #256): the disconnected unit stays internally whole and therefore re-connectable instead of hollow. What does cascade over the subtree is the connectivity recompute: if a hive thereby loses its last boundary-crossing edge, its entire subtree flips to `active = false` and the tasks below it end. Reading "cascade" as "every edge below it goes" leaves edges standing that you believe are gone; the worked recipe for dissolving a hive is in `rewiring.md` § Disconnect the old hive. |
| `add_edges` | New edges in colony's edge table, scoped. Endpoints are paths relative to the `scope`, at any depth; `.` names the scope root itself (GH #487), the very spelling a `params.graph` uses for its own level. Optional fields as in the bootstrap schema: `condition`, `modifier` and, since v0.18.0 (GH #283), `default` (boolean, absent means `false`). `"default": true` puts the edge into the second routing phase; a non-boolean value is `edge_schema`, and an unguarded default edge commits with a `warn` log line. |
| `remove_edges` | Remove edges from the edge table, scoped; `match.from` and `match.to` read the same endpoint vocabulary as `add_edges`, `.` included (GH #487). Applied **before** `add_edges`, so an edge can be replaced in one mutation with the lane never missing in between. The other way round, the `match` pattern deleted the edge the same diff had just inserted (GH #158). |
| `swap_nodes` | **Graph swap**: swings all external edges of an implementation (`match`) atomically onto another (`with`), the other being either freshly instantiated from a template or an already existing cell. The old cell remains disconnected and preserved (no-delete; swappable back at any time by swinging the edges back). `swap_nodes` is a pure edge and topology diff: no `config.json` rewrite of an existing cell, no `cell.db` migration, no `cell_id` takeover (the new implementation has its own identity), and it inherits the atomicity model of the edge mutation. What "external" means for a subtree (GH #256): an edge is external when its other endpoint lies outside the subtree rooted at `match`. The wiring with which that root serves its own children (`<unit>` to `<unit>/<cell>` and back) is internal and is not carried along; it stays with the unit it belongs to. The old unit is thereby preserved whole, which is what makes swinging the edges back restore a working unit. On a leaf the difference is invisible, which is why it went unnoticed until GH #256. Conditions for the instantiate form: the `with` target path is free in the registry and on the filesystem, and the template named is not a single hive cell (`hive_template_single_cell`, GH #572; a hive has no factory and enters the world only as the root of a multi-cell subtree, and a subtree template is refused at this door with `schema` anyway). A directory already lying there that no registry row names (a hand-placed tree, the residue of an aborted migration) is refused by name rather than overwritten; taking it over is done with an `add_nodes` at the same path (a resume) or an `add_nodes[].adopt` stating the `cell.type` expected there. |
| `move_nodes` | **Relocation**: moves a cell to a different address, as in `{"match": {"name": "fetch"}, "to": "helpdesk/fetch"}`. A path is a cell's identity, which is why this is the only operation that changes one: the directory is moved with `rename(2)`, carrying `config.json`, `cell.id` and `cell.db`; the registry row is re-addressed by an UPDATE (`cell_id`, `created_at` and `instantiated_at` survive); and every edge naming the old path names the new one afterwards, condition and modifier verbatim. One committed mutation, with no window in which the lane is wired twice or not at all. Against `swap_nodes`: a swap swings edges onto a different implementation with its own identity and its own `cell.db`; a move keeps the same cell at a different address. Conditions: the target lies inside the mutation scope, the target is free (registry, hive scopes, filesystem), its parent directory already exists, and the source is not a hive and has nothing beneath it (a half-moved hive would leave its children addressed under a path that no longer exists, so it is refused by name). The parent hive's `params.graph` is not rewritten: since GH #168 the persisted edge table is the boot topology on a reboot, so the file is seed and not state. |
| `add_templates` | **Put a reusable template into the running colony's instance-local library** (GH #440). An entry is `{"name": …, "files": {"<relpath>": "<content>", …}}` and `template.json` is mandatory. The write always goes to `{templates_root}/local/<name>/`: the colony **builds** that path and never takes one from a field of the body, which is what puts the shipped library out of reach. The operation claims no address and vacates none, so it contributes nothing to the post_state. It runs first in the diff, so an `add_nodes` of the same diff can resolve the template by name; one level up the same holds inside a manifest, where a later entry resolves what an earlier one registered, and that is why the registration is a declaration and not a side channel. Two refusals, both pre-destructive: a name outside `^[a-z][a-z0-9-]{1,63}$` or a file path that climbs out of the directory is `invalid_template_name`; a name the registry already answers is `template_name_taken`, at its position rather than as an abort of the next rescan for everybody. The write is staging plus one `rename(2)`, so a concurrent rescan can never pick up a half-written `template.json`. A refused entry leaves nothing on disk. The files are not substituted (GH #611): `files` carries the bytes of the class and no value of this mutation, so it is written byte for byte, and every `${…}` in it binds at instantiation or at read time. The entry's other fields are substituted like every other part of the diff. |
| `seed_rows` | **Put rows into a store of a running colony** (GH #456). An entry is `{"target": "<path of a store>", "table": "<a table that store declares>", "rows": [ {…} ]}`. The eighth operation is the only one that changes what is inside a cell rather than where cells are, and it exists for the class of rows that are permissions and keys rather than data: an `access` policy row, a grant, a firewall rule, a subscriber. Those rows used to reach a running colony as a bare store message, a path with three holes: no digest, no access verdict before the write, and no `mutation_log` row. Through this door they have all three. It is checked against the post_state: the target is a registered `store` cell (any other type is `seed_target_not_a_store`, and so is a target where nothing stands), the table is named by its `params.schema` (otherwise `seed_table_undeclared`, and the refusal names the tables that do exist), and every key of every row is a declared column (otherwise `schema`). The target may have been created by the same diff. The operation claims no address and vacates none, so it contributes nothing to the post_state; it runs last in the diff, immediately before the commit, so that every refusal that can still happen has happened. Idempotent by declaration: a row already present, column for column, is counted and not written a second time. A store's declared tables carry no primary key, so nothing else would be idempotent, and `meclaw --apply` of the same manifest twice is therefore a no-op. It is the same seed mechanic and not a second one: the same JSON to SQL binding the staging seeder uses, the table built from the same declared column list, and a store-owned table left standing without its key is repaired by `ensure_keyed_table` at the next wake (GH #255). The write goes into the target's `cell.db` even while the cell is awake: the colony is the write authority, WAL plus `busy_timeout` serialise the second connection, and a `store` holds no in-memory view that could go stale. A `params.write_surface: "internal"` bounds messages, not this door: the reach here is the mutation scope and the access verdict over it. |

The match pattern for `remove_*` and `swap_nodes` references nodes and edges by properties (`name`,
`template`, and for edges `from`, `to`, `condition`, `modifier`, `default`), never by UUID. A
pattern is a pattern and not an identity: `{from, to}` alone hits every edge between the pair, so
pass `condition`, `modifier` and `default` too when exactly one is to be hit. With
`remove_edges[].match.default` (boolean, since v0.18.0, GH #283) absent the routing phase is
unconstrained; present, the edge must run in exactly that phase. The pattern must have at least one
hit in the current registry, otherwise the mutation is rejected.

### Validation

Single-stage in colony. Before application the hypothetical post_state is computed and checked
against these criteria.

- Schema. The diff conforms to the JSON schema (per-operation detail: `config.md`).
- Match patterns. Each pattern in `remove_*` and `swap_nodes` hits at least one element in the
  pre_state.
- Naming uniqueness. No two nodes have the same name within the same scope after applying the diff.
- Cycle freedom. The post_state graph has no cycles over `from`/`to` edges, insofar as the
  application forbids cycles; meclaw-core does not generally reject on cycles.
- Edge schema compatibility. All edges reference existing nodes in the post_state; `condition`
  parses as valid CEL; `modifier` conforms to the `{set?, delete?}` schema and all expressions in
  `modifier.set.*` parse as valid CEL; `default` is a boolean, and any other type is `edge_schema`
  (GH #283). Edge endpoints resolve relative to the mutation `scope` at any depth within the scope,
  against the post_state, diff-new nodes and this diff's own swap and move targets included.
  Spelling decides nothing: `foo` and `./foo` are the same node on either side of the diff.
  Endpoints that resolve outside the scope (`../x`, absolute paths) are `scope_out_of_bounds`,
  because the parent wires downward into its own subtree and never out. A depth path to a
  non-existent node is `edge_schema`.
- Template existence. All `add_nodes` and `swap_nodes` reference templates that exist in colony's
  templates registry.
- `.env` variables. All `${ENV_VAR}` in the `override_params` have values in `.env`.

On an error before the atomic rename phase (schema, match, cycle, edge schema, template, `.env`,
staging build) the entire diff is rejected: no partial commit, the live tree untouched.

### The collecting validator (GH #293)

What is accepted and what is refused does not change; the report is complete instead of partial. The
checks run in seven stages, in this order:

1. diff schema
2. template resolution (reference resolvable, `ref`s, rings)
3. `requires`, the `ctx` and `env` keys
4. post-state addresses (naming collision, match-no-hit, `override_params` addressing, cell type,
   the `adopt` grammar, `swap_nodes[].with`)
5. edge endpoints
6. contract locality (hive port boundary, inbound lanes, header contract locality)
7. `required_drains`

Inside a stage nothing stops: a diff with five independent violations of the same stage
is refused **once** and names all five. Between stages it stops at the first stage that produced any
entry at all, because an unresolved template makes every later endpoint error a consequence rather
than a cause.

`error_code` is the first entry's code within that stage, and `details` remains a single string:
every violation, one per line, in the form `<stage>/<code> <address>: <message> - <because>`, with
the address and `because` parts omitted where there are none. A contract's own `because`
(`required_drains[].because`, `LaneSpec.because`) travels verbatim for every affected entry.

Which `error_code` a multi-defect diff reports can have moved with the staging: a diff pairing an
unresolvable template with a missing `requires` key or a naming collision reports `template_missing`
(stage 2) instead of `requirement_missing` or `naming_collision`. The verdict is unchanged.
`error_code` is a stability surface ([`stability.md`](stability.md)), which is why this is written down.

Checks that are not stages (the resume and `adopt` filesystem guards, the subtree pre-checks, scope
containment, `remove_edges`, the relocation gate) keep their own single refusal where they are.
Their `details` stays the debug form (`MatchNoHit("x")`); only the staged rejects carry the rendered
line form.

The two post-state validations after the rename phase (`required_drain_missing`, `hive_contract`)
collect by the same rule; they run there because they need the post_state edge table.

Inside the rename phase the audit model applies: an error after the first successful `rename(2)` is
no longer a clean reject, because earlier renames already stand in the live tree. The substrate
strict-fails loudly (panic), and the half-state is made visible at the next boot as non-registered
orphan dirs, never silently adopted.

After the rename phase but before the commit a mutation can still be refused: by the two post-state
validations (`required_drain_missing`, `hive_contract`), by the two runtime conditions of a
disconnect (`stop_wiring_unavailable`, `term_timeout`), and by a failed cell spawn.
These rejects are **clean** again (GH #276): the in-RAM edge ops roll back and the freshly
renamed-in directories are removed, single cell directories as well as the rename-roots of a subtree
template; adopted and relocated ones stay (no-delete), and so does every node that was already there
on a merge resume. The `write_buffer` is discarded, so `colony.db` never sees a registry row, and
the `mutation_log` row is terminalised as `failed`. A cell that had already spawned `Awake` is
peace-stopped and its death ack waited for before its directory goes.

The error message to `reply_to`, if set, carries:

```json
{
  "error_code": "<code>",
  "details": "<human-readable>",
  "context": { ... }
}
```

### Mutation error codes

`error_code` is an enum: `schema` | `match_no_hit` | `naming_collision` | `cycle` | `edge_schema` |
`template_missing` | `env_var_missing` | `unsupported_substitution` | `ctx_key_missing` |
`scope_out_of_bounds` | `unknown_cell_type` | `stop_wiring_unavailable` | `term_timeout` |
`resume_requires_stopped_cell` | `subtree_resume_unsupported` | `resume_type_mismatch` |
`contract_incomplete` | `invalid_params` | `hive_port_boundary` | `hive_contract` |
`required_drain_missing` | `template_ref_cycle` | `requirement_missing` | `invalid_template_name` |
`template_name_taken` | `shutdown_draining` | `seed_target_not_a_store` | `seed_table_undeclared` |
`v_lane_no_connect_point` | `v_lane_mandatory_hop` | `v_lane_unanchored` |
`hive_template_single_cell`.

These strings are part of the stable mutation API contract, with the same promise the dead-letter
codes carry: new reject reasons extend the list, existing ones never change their string form. A
condition may match on a code, and must not assume the list is complete, because an unknown code is
a future code. Notes on the substrate codes:

- `ctx_key_missing`: a `${ctx.<key>}` substitution in the diff references a key missing from the
  `ctx` block of the mutation. Emitted by `resolve_ctx_token` (`mutation/substitute.rs`).
- `scope_out_of_bounds`: a top-level diff path (`add_nodes[].name`, `*_edges[].from`/`.to`,
  `match.name`) resolves outside the mutation `scope`. Checked before any filesystem or registry
  mutation; emitted by `validate_scope_containment` (`mutation/validate.rs`).
- `unknown_cell_type`: `add_nodes` or `swap_nodes` references a cell type without a registered
  factory.
- `stop_wiring_unavailable`: disconnect or swap of a cell whose stop wiring is not restorable after
  a term_timeout survivor (F5 guard, a permanent backstop).
- `term_timeout`: a death-ack timeout on disconnect or swap of an awake cell, which means a full
  rollback plus reject.
- `shutdown_draining`: a build order that arrived during the shutdown drain (GH #47). It happens
  before any staging, so the reject leaves no trace.
- `resume_requires_stopped_cell`: the resume path requires a stopped cell.
- `seed_target_not_a_store`: a `seed_rows` entry names a target that is not a `store`, whether
  nothing stands there, a cell of another type stands there, or its declaration cannot be read. Only
  a `store` owns declared tables (GH #456).
- `seed_table_undeclared`: a `seed_rows` entry names a table the target does not carry in
  `params.schema`; the refusal names the tables that do exist (GH #456).
- `subtree_resume_unsupported`: a subtree template at an already occupied root path. No producer
  today; the earlier F4 reject was superseded by per-node resume, and the enum string remains
  reserved.
- `resume_type_mismatch`: a resume (single-cell as well as subtree) at an occupied path whose
  existing `type` deviates from the template.
- `contract_incomplete`: a `config.json` to be loaded (boot walk or mutation staging, non-hive) does
  not declare the required keys `contract.version`, `settings` and `consumes`, or declares them
  type-wrong (`config.md` § contract).
- `invalid_params` (GH #404): the `params` block of a cell about to be instantiated does not
  deserialize for the cell type it names, the question `CellFactory::validate_params` asks of every
  cell at boot, asked when the `params` are written. Checked is the runtime view including the
  default-deny `sandbox` block, byte for byte what the boot reads back off the disk.
  Pre-destructive, emitted by `patch_and_substitute_config` (`mutation/stage.rs`) during staging;
  the message names the staged `config.json` as `<node>/config.json`, without a host path (GH #507),
  plus the factory's own reason verbatim. A cell type without a registered factory produces
  `unknown_cell_type` instead, and a hive marker produces nothing. The guard works forward: a tree
  that already carries the defect is not repaired by it.
- `hive_port_boundary` (GH #133): an `add_edges` endpoint reaches past a declared port into a hive
  while the edge's other endpoint lies outside it. Pre-destructive, emitted by
  `validate_hive_port_boundary` (`mutation/port_boundary.rs`) before staging. A hive without the
  declaration is not sealed and never produces this code. Mutations only (ruling 2026-08-15);
  boot-time enforcement, if it ever comes, arrives as its own opt-in switch.
- `hive_contract` (GH #173): a hive declared its interface as lanes (`params.contract`, opt-in) and
  something contradicts it. Three shapes: an `add_edges` edge onto the hive path stamps a constant
  `hop.route` the hive does not declare — measured against `accepts` from outside, and as an exit
  against `emits` from inside (GH #602); the hive's own graph no longer carries a lane it promises
  (an `accepts` lane with no door, an `emits` lane with no exit through the hive path); or a hive
  this diff gives birth to declares a lane `required` and no edge of the same diff delivers it, onto
  the hive path for a rim lane and onto one of its `at` connect points otherwise (apps rim,
  2026-09-05; checked once, at birth). Pre-destructive, emitted by `mutation/hive_contract.rs`,
  checked with the real router (`apply_edges`); an edge whose route is only knowable at runtime is
  not judged. Mutations only, and boot warns.
- `required_drain_missing` (GH #147, GH #237): a hive declared a pair in `params.required_drains`, a
  port with its drain or an accepted lane with the answer lane the caller has to take, and something
  from outside serves one half while the other is missing once the diff stands. Needs the post_state
  edge table, so it runs after staging and before the spawn and registry step; the reject is
  spurless. Emitted by `mutation/required_drains.rs`, checked with the real router.
- `template_ref_cycle` (GH #277): a template `ref` closes a ring, so a template already on the
  resolution stack is entered a second time. The stack is the guard, so composition needs no depth
  cap. Pre-destructive, emitted by `expand_ref` (`mutation/subtree.rs`) during parsing; the message
  renders the ring as `a@1.0.0 -> b@1.0.0 -> a@1.0.0`. A `ref` that points at nothing is
  `template_missing`, whose message names the reference plus the versions the registry does hold
  under that name, or `none`.
- `requirement_missing` (GH #292): an instantiation names a template that declares a key
  (`requires.ctx` or `requires.env`) the mutation does not supply. The set spans the named template
  and, through its `ref`s, every referenced one. Pre-destructive, emitted by `validate_requires`
  (`mutation/validate.rs`) before scope containment; the message names the template, the class, the
  key and the template's own `because` verbatim. A template without a `requires` block never
  produces this code. Both instantiating operations are covered (GH #347): an `add_nodes` entry and
  the instantiate form of `swap_nodes[].with`; the existing-node form stages nothing and owes
  nothing. A resume stages nothing and consumes no declared key, so the exemption belongs to
  `add_nodes` and not to the swap, which always stages. It is per node, not per entry (GH #347): the
  merge stages the missing children of a partially existing composite subtree and the contract holds
  for those, while the nodes the merge skips are exempt (`subtree::classify_subtree_nodes`). A `ref`
  belongs to the node it hangs under, and the named template's own declaration is owed as soon as
  the entry stages anything at all.
- `invalid_template_name` (GH #440): an `add_templates[]` entry names something that cannot become a
  directory under the local template root (outside `^[a-z][a-z0-9-]{1,63}$`), or a file path that
  climbs out of the template directory. Pre-destructive, emitted by
  `mutation::register::parse_entry`. The colony builds the target path and takes none from the body,
  hence a refusal, not a sanitisation.
- `template_name_taken` (GH #440): an `add_templates[]` entry names a template the registry already
  answers to. Refused at its position: the entries before it stay applied, the ones after it are
  never looked at. The same code carries a directory that already lies under `local/<name>/` without
  a registry row naming it, refused by name rather than overwritten, and since GH #443 two entries
  of one `add_templates` array naming the same template, checked against what this mutation has
  itself staged.
- `v_lane_no_connect_point` (GH #559): a v-lane ends inside a hive whose contract does not name the
  endpoint for that lane in an `at`. The opening is pronounced by the target itself and never taken
  by the caller; a contract without an `at` never produces this code.
- `v_lane_mandatory_hop` (GH #559): a v-lane wants to skip a level in between that carries the lane
  in its contract (`accepts`/`emits`) without an `at` that reaches the endpoint. Whoever declares a
  lane may not be passed by.
- `v_lane_unanchored` (GH #559): a `swap_nodes` replaces a subtree a v-lane ends in, and the new
  form has no such relative path or does not name it for this lane. The whole swap is refused.
- `hive_template_single_cell` (GH #572): an instantiating diff entry names a template whose root is
  a hive with nothing under it. A hive has no factory and enters the world only as the root of a
  multi-cell subtree. It holds at both instantiating doors (`add_nodes[].template` and the
  instantiate form of `swap_nodes[].with`). Pre-destructive, emitted by
  `reject_if_single_cell_hive_template` (`mutation/validate.rs`) at stage 4; the refusal names the
  shape that works (ADR 0021).

`uuid_provider_exhausted` is not live code. The enum variant `MutationError::UuidProviderExhausted`
was dead (`Uuid::now_v7()` is infallible) and was removed with paket 7 (D-034, verified 2026-06-10).
The note remains as re-discovery protection.

### Scope, concurrency and permissions

A mutation covers one scope, one path prefix. Sub-scopes (nested hive markers in the filesystem) are
mutated via their own mutation messages with their own scope path. A single mutation cannot address
several scopes at once, which keeps mutations local and race-free.

The mutation door carries no concurrency protection. No concurrent builders per scope are expected
in this phase, so `expected_version` and its relatives are absent; colony's sequential mailbox
processing serialises concurrent mutations by itself. It can be retrofitted additively.

meclaw has no permission layer either. That is a boundary, not a backlog item (ruling 2026-08-19).
Whoever can deliver a mutation message to `/colony/mutations` routing-wise can mutate. Permission is
a topology question, not an identity check, and the `mutate-graph` capability in the cell contract
is a discovery hint for the builder composer and audit tools rather than a runtime check.
Authentication belongs to a reverse proxy in front of this substrate; meclaw knows no identities, it
knows paths. What meclaw contributes is `--api <bind>`: no port by default, opt-in via `--api
127.0.0.1:7777` for local-only, and what stands in front of a `0.0.0.0` bind is yours.

Attribution is a different question. The substrate stamps `envelope.reply_to` on every cell
emission, so a submission drafted inside a colony reaches the mutation door under a name and lands
in `mutation_log` with it. A `POST` from outside has no such path, so a submitter reading the
envelope sees an anonymous request. The template answer is `operator` (`templates/operator/`), a
hive whose occupants turn an outside request into a message they emit themselves, so the request
acquires the identity of the cell that served it. That is identity and not authentication: the hive
verifies no claim.

## Architecture building blocks

| Term | Description |
|---|---|
| Hive scope marker | Directory with `config.json` `type: "hive"`. Not an actor: no Tokio task, no mailbox, no `cell.db`, no `ActorHandle` entry in the cell registry. Still a junction in the system: colony keeps a separate hive scope table (path prefix, authority boundary, mutation scope, initial `params.graph`). At filesystem bootstrap the hive marker is recorded; on mutations it is a scope boundary. Addressable as a transit target, where colony forwards based on the hive out-edges and never delivers (`cell-types.md` § `hive`). |
| Session | An application convention for a logical conversation bracket, typically propagated via the `session_id` header. Not a core concept; meclaw-core knows no sessions and applications choose their own granularity. |

## Filesystem layout

```
{root}/
├── colony.json              # colony-wide behavior defaults (optional)
├── colony.db                # SQLite: registry, templates, mutation log, central message log
├── log.jsonl                # tracing JSONL
├── colony.db-lease          # transient operational file: root lease of the running daemon (GH #121)
├── orphan-journal.jsonl     # transient operational file: spawn journal of tool children for the boot reap (GH #116)
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

meclaw prescribes no `main/sessions/archived/` separation. Paths are chosen by whatever triggers
the instantiation (builder, CLI, API), and conventions arise from the application logic.

`.staging/` is the temporary directory for mutations standing between validation and commit. Colony
builds new cell directories here completely, with substituted `config.json` values and possibly a
`cell.db` from seed, then does a single `rename(2)` to the target path, atomic per directory on
POSIX. Broken half-instantiations therefore cannot lie in the live tree, and recovery at startup
deletes everything in `.staging/` without a commit marker. Rejected: direct writing at target paths
with backup files, and a `.tombstones/` directory.

## The `colony.json` file

The colony-wide configuration file in `{root}`. It holds behaviour defaults for cells and colony,
never operations configuration: paths and logging stay CLI flags, nginx-style.

```json
{
  "schema_version": 1,

  "mailbox_default_capacity": 1000,
  "message_timeout_default_ms": 60000,
  "idle_timeout_default_ms":   60000,
  "message_default_ttl":          64,
  "ttl_notice":                 false,
  "restart_max_retries":           5,

  "mutation_receipts":          null,

  "blob_inline_max_bytes":         65536,
  "blob_max_recursion_depth":      64,
  "slot_park_max":                 64,

  "strict_validation":          false,

  "log_default_level":          "info",

  "shutdown_drain_timeout_ms":  10000,

  "watchdog_threshold":              5,
  "watchdog_period_ms":            100,
  "watchdog_on_trip":           "exit"
}
```

| Key | Meaning |
|---|---|
| `schema_version` | Version marker for migration compatibility |
| `mailbox_default_capacity` | Default capacity of the regular cell mailboxes (bounded mpsc); overridable per cell via `cell.mailbox_size`. It shadows only the regular cell mailbox (AMBIG-001 ruling B): the dead-letter queue and disconnect mailbox capacities are fixed constants and are not overridden by this field. |
| `message_timeout_default_ms` | Default for the substrate backstop per `handle()` call (concept B). On exceedance the cell task is killed and the supervisor restarts. It is not the primary I/O protection; `params.external_timeout_ms` (concept A) is responsible for that. The value should be considerably more generous than the longest expected I/O operation. Overridable per cell via `cell.message_timeout`. |
| `idle_timeout_default_ms` | Default idle duration per stateful cell with `cell.timeout: 0`; after this time without a new message the cell despawns itself (Awake to Asleep). Overridable per cell via `cell.idle_timeout_ms`. Takes effect from phase 13. |
| `message_default_ttl` | Default TTL for source messages, a protective limit against routing loops. Colony decrements per routing hop; at `0` the message goes directly into the dead-letter queue as `ttl_expired`, with no step-1 `reply_to` attempt. Builders can set the value per initial message. Recommendation: 64. |
| `ttl_notice` | GH #119, ruling 2026-08-14. Opt-in (default `false`): when `true`, a TTL death whose message carries a reply anchor (`reply_to`) additionally sends one terminal notice to that anchor, a substrate error reply in the canonical shape (`hop.finish_reason: "error"`, `hop.error_code: "ttl_expired"`, `hop.dead_target`, `hop.dead_message_id`; `context` travels unchanged). The notice is itself terminal (no `reply_to` of its own) and can therefore never produce a second notice. Without an anchor nothing changes (DLQ only). It is opt-in because the notice carries a fresh `message_default_ttl`, so a colony that turns it on has taken its loops out of the TTL guard and bounds them with the iteration counter instead, exactly the trade `modifier.restore_ttl` makes visible on an edge. Default `false` keeps the sharp, silent guard. |
| `mutation_receipts` | GH #553, ruling R-0904-1. Opt-in (default: the key is absent): `{"to": "<path>"}` says where the mutation door leaves its receipt. With it set, the colony emits exactly one terminal message per committed knock at `/colony/mutations` to that path: sender `/colony`, the mutation's `trace_id`, `parent_message_id` the knocking message, a fresh `message_default_ttl`, body `{"messages": []}`, and everything it says in the `hop`: `route: "mutation_committed"`, `mutation_id` (single form) or `mutation_ids` (manifest), `outcome: "committed"`, `scope`, and `form: "single" \| "manifest" \| "boot"`. Five keys and no more, because what changed is a question `/colony/graph` answers and a receipt that tried to answer it too would be a second, staler truth. It is the second place the substrate seeds a `hop` itself; the first is the HTTP ingress with the `hop` field of `POST /messages` (GH #175), and for the same reason: a message that arrives at a hive path without asserting a lane matches no door. `to` is a hive path by intent, so the receipt enters as a hive transit and the hive's own `{"from": "."}` edges decide who hears it, and the substrate never learns a single listener's address. A refusal, a manifest that stopped part-way included, leaves no receipt, and none is emitted during the shutdown drain. The receipt is terminal and is not a mutation itself, so it can neither start a round nor produce a second receipt. **And the boot is the first one** (ruling O-0904-2): right after `InitialApply` has put the edge table up, exactly one receipt with `form: "boot"` and a nil `mutation_id` goes out, so a restart fills a menu and a screen instead of answering `503` until the first mutation of the day. It is the only one that carries no `parent_message_id`, so it stands in the message log as `@external`, the booking of every source emission; a mutation's receipt carries the knocking message as its parent and stands under `/colony`. A receipt is an event and not a guaranteed delivery: it goes back into the colony's inbox as its own work, and when that inbox is full it is dropped with a `mutation_receipt_undeliverable` warning instead of blocking the loop. A listener therefore has to tolerate a missed receipt; it is healed by the next receipt, or at the latest by the next start's boot receipt. In the shipped library the chain is drawn one level at a time: `meclaw-os` accepts `mutation_committed` and carries it into `./orgs`, `org` into `./members`, `member` into `./assistants` and `./apps`, `assistant` into `./talky` and `./cogny`, and each of those into `./collector`. The lane keeps one name the whole way; only the collector turns it into its internal `in_menu_tick`, and `colony-view` turns it into its own `in_refresh`. Every level that declares the lane is a mandatory hop for it (ADR-0020). The two poll timers that used to ask the same questions every few minutes are gone with no replacement. It is opt-in because a receipt is traffic in a topology that did not have it before, and who hears it is the colony's decision. A `to` that does not start with `/` is a hard parse error. |
| `restart_max_retries` | Maximum number of `one_for_one` restarts per cell before `failed` status. This field is parsed but not applied today: the effective cap comes from the substrate constant `DEFAULT_RESTART_LIMIT` (5), overridable per cell via `config.json` `cell.restart_limit`. |
| `blob_inline_max_bytes` | Threshold above which a body is offloaded as a blob; smaller bodies stay inline in the message. |
| `blob_max_recursion_depth` | Hard limit for recursive in-message pointer resolution. Wired since GH #19: the value rides on the blob store and is read at the delivery boundary. `0` is a valid kill switch, expanding no pointer. On exceedance: `blob_recursion_too_deep`. |
| `slot_park_max` | How many messages one `park` slot may hold while nothing is bound behind it (default 64, GH #285). A `park` slot nobody ever fills would otherwise grow its queue for as long as the colony runs. At the bound the newest arrival is refused (`slot_park_overflow`), so the earliest context, the part a later reader cannot reconstruct, survives. `0` is a valid kill switch: every message onto an unbound `park` slot is refused and no empty queue is created either. The queue lives in the colony task and not on disk, so a colony shutdown discards whatever is still parked: it is a promise about the running colony's topology, not a durable outbox. |
| `strict_validation` | Release-build default for whether JSON schema validation against `emits`/`consumes` is active (a debug build is always `true`). |
| `log_default_level` | Tracing default level. This field is parsed but not applied today: the effective default comes from the `--log-level` flag or `info`. |
| `shutdown_drain_timeout_ms` | GH #47. How long the colony loop waits for quiescence after the shutdown signal before it cuts off (`u64`, default 10000). The drain lets every in-flight message and its follow-on hops run to their end and refuses new ingress (`shutdown_draining`). `0` means drain off, so the loop breaks off immediately as it did before GH #47, which makes it the rollback switch without a redeploy. At the deadline a `warn` line on stderr names what was left behind (`drain_incomplete`, `busy`); the exit code stays `0`. Under a process supervisor the value belongs together with the stop budget: after the signal the process needs this budget plus the teardown chain, which at the default stays comfortably within a `TimeoutStopSec=30`. Whoever raises the drain raises `TimeoutStopSec` with it. |
| `watchdog_threshold` | Number of consecutive silent supervisor periods after which the heartbeat watchdog trips (default 5). Must be at least 1; `0` is a hard parse error. |
| `watchdog_period_ms` | Length of one supervisor period in ms (default 100, the same rate as the colony loop's heartbeat). Must be at least 1; `0` is a hard parse error. Together with `watchdog_threshold` the default gives the limit 5 x 100 ms = 500 ms. |
| `watchdog_on_trip` | What a trip does: `"exit"` (default; graceful shutdown plus a non-zero exit, so a supervisor restarts and an alert fires) or `"log-only"` (the trip is logged loudly and structured, the colony keeps running). Any other value is a hard parse error. `log-only` covers silence only: a colony task that is gone (heartbeat channel closed) ends the process under both policies. |

Cells can override individual values via their `config.json` `params` or their `contract.settings`,
and then the local value applies.

Not in `colony.json`: paths (`--templates`, `--blobs`, `--env`, `--log`) and logging configuration
(`--log-level`, `--log-filter`) remain CLI flags. Rejected were `colony.json` as a required file,
mirroring all CLI flags in it, and per-scope configuration; per scope can be retrofitted
post-roadmap.

## `/colony` as a virtual endpoint

`/colony/*` paths do not exist in the filesystem tree; they are virtual endpoints built into
colony's routing algorithm: every path beginning with `/colony/` is read as an internal operation,
never as a registry lookup.

Internal API and external API are symmetric: every `/colony/<endpoint>` is at once a message target
for internal senders (cells, builder, routing) and an HTTP route. The HTTP layer converts a request
into a `Message` with `target = "/colony/<endpoint>"` and sends it through the same routing path,
internally as the typed `ColonyMsg::{Mutation, Read*, …}` inbox variant with a oneshot-ack reply.
Symmetry means the same colony-task sequence and the same body model, not a literal `route()` call.
A generated OpenAPI spec (via `utoipa`) is planned and not built, so the table below is the
description of the surface.

| Path | Purpose | Filter / query parameters | Writing? | Phase |
|---|---|---|---|---|
| `/colony/dead_letters` | Dead-letter queue: unresolvable routes, expired TTLs, routing errors | `?since=<ts>` (filters via `WHERE created_at >= ?` on the dead-lettered message's `created_at`, see `handle_read_dead_letters` in `colony_dispatch.rs`), `?limit=<N>`, `?error_code=<code>` | both (read + drain) | 2 |
| `/colony/registry` | Read the cell registry (all registered cells with paths, IDs, types, status). `?path=` for a single cell. Inactive nodes included, with the `active` field. | `?path_prefix=<path>`, `?type=<celltype>`, `?path=<exact>`, `?active=true\|false`, `?tag=<token>` | no | 4 |
| `/colony/templates` | Read the templates registry (for builder discovery) | `?type=<celltype>` (exact match on the template cell type; unknown values yield an empty list), `?name=<name>` | no | 5 |
| `/colony/templates/rescan` | Trigger a re-read of the templates directory | none | yes | 5 |
| `/colony/mutations` | Mutation pipeline; builders send mutation diffs here | none (the diff is in the body) | yes | 6 |
| `/colony/graph` | Read the topology of a scope (nodes and edges, runtime-projected) | `?scope=<path>` (default root), `?tag=<token>` | no | 6 |
| `/colony/trace` | Read the message log, built as a tree by `parent_message_id` when `trace_id` is set | `?trace_id=<uuid>`, `?path_prefix=<path>`, `?correlation_id=<uuid>` (inert today, `correlation_id` is not originally set), `?error=true`, `?since=<ts>`, `?limit=<N>` | no | 11 |
| `/colony/ledger` | Counts and sums out of `message_log`, `dead_letters` and `mutation_log` for one time window (aggregates, no raw rows) | `?since=<ts>` (inclusive, default `now - 3600`), `?until=<ts>` (exclusive, default `now`), `?path_prefix=<path>`, `?cycle_id=<id>`, `?group_by=model\|path\|error_code`, `?tag=<token>`, `?scan_budget=<N>` | no | 0.20 |
| `/colony/messages` | Browse the message log: a newest-first list with filters plus a single message | `?id=<uuid>`, `?trace_id=<uuid>`, `?parent_message_id=<uuid>`, `?correlation_id=<uuid>`, `?to_path_prefix=<path>`, `?from_path_prefix=<path>`, `?body_kind=inline\|blob`, `?since=<ts>`, `?until=<ts>`, `?before_created_at=<ts>&before_id=<uuid>` (keyset cursor), `?limit=<N>`, `?scan_budget=<N>`, `?resolve_blob=true` | no | P1 |
| `/colony/events` | Subscribe to a live event stream (routing decisions, mutation commits, restarts, dead letters) | none (subscription-style) | no | 14 |

`/colony` itself, without a sub-path, is not addressable and requests there are an error.
`/colony/cell` does not exist as its own endpoint; individual cells are read via
`/colony/registry?path=<path>`.

Colony answers reads in the universal body format with a top-level slot named after the endpoint:
`registry`, `dead_letters`, `templates`, `trace`, `messages`, `ledger` (an aggregate object, not a
list), `mutations` (the audit read) and `rescan` (the outcome), analogous to the `graph` slot.

A cell that emits to a `/colony/*` endpoint carries the endpoint-specific call as a top-level slot
in the body:

- `/colony/mutations` takes top-level `{ scope, diff, ctx }`: the mutation diff plus the `scope`
  plus an optional `ctx` substitution context. It is the only writable EDA endpoint.
- `/colony/registry`, `/colony/templates`, `/colony/graph`, `/colony/trace` and `/colony/ledger`
  take top-level `{ query: { … } }`, whose fields correspond to the HTTP query parameters of the
  endpoint (`registry`: `path`/`path_prefix`/`cell_type`/`active`/`limit`/`tag`; `templates`:
  `cell_type`/`name`/`limit`; `trace`:
  `trace_id`/`path_prefix`/`correlation_id`/`only_error`/`since`/`limit`; `graph`: `scope`/`tag`;
  `ledger`: `since`/`until`/`path_prefix`/`cycle_id`/`group_by`/`tag`/`scan_budget`). If `query` or
  a single field is missing, the defaults apply (`limit` default 100, hard cap 1000; on the `ledger`
  `since` defaults to `now - 3600`, `until` to `now`, `scan_budget` to 50000). The read reply goes
  to the sender path.
  At all five reads a filter that arrives is never silently dropped (GH #341, GH #359). If a field
  is present but unreadable, the endpoint answers with an error instead of the unfiltered holdings:
  `{"<slot>": {"status": "error", "error_code": "invalid_query", "details": "…"}}`, with no result
  list. Unreadable means `query` not an object,
  `scope`/`path`/`path_prefix`/`cell_type`/`name`/`group_by`/`tag`/`cycle_id` not a string,
  `active`/`only_error` not a boolean, `since`/`until`/`limit`/`scan_budget` not a number,
  `trace_id`/`correlation_id` not a valid UUID, `group_by` other than a declared value (`model`,
  `path`, `error_code`), or `cycle_id` longer than 64 characters. A field that is missing or `null`
  still means the documented default. Clamping applies only within the valid values: a `limit` that
  is a non-negative integer stays clamped to 1 through 1000, and a `scan_budget` that is one stays
  clamped to 1 through 200000. A negative or fractional `limit` is refused like any other unreadable
  filter, and so is a fractional `since` and a negative or fractional `until`/`scan_budget`. `tag`
  is truncated to 64 characters rather than refused, since it filters nothing; `cycle_id` is refused
  rather than truncated, because it does filter. An empty window is refused rather than answered: if
  resolving `since`/`until` yields `until <= since`, the `ledger` answers `invalid_query` with the
  details `empty window: until <= since` instead of zero counts. What is tested is the resolved
  window and not the one that was sent, so a caller who sends neither still gets an answer. No new
  `error_code`: the `ledger` refuses under the same `invalid_query` as the other four. Retracted (GH #341):
  `/colony/graph` accepted a top-level `{ scope }` as an alias for one release round, 0.18.0; the
  alias is removed and a top-level `scope` is now an `invalid_query` error. Send `{"query":
  {"scope": "<path>"}}` instead.
- `/colony/dead_letters` is neither EDA-dispatchable nor body-operation-controlled: read versus
  drain is decided by the HTTP method (`GET` reads, `DELETE` drains) or by the dedicated
  `ColonyMsg::ReadDeadLetters`/`DrainDeadLetters` inbox variant. The body carries no `operation`.
  A cell emission to `/colony/dead_letters` is hard-rejected.

Two properties of the query surface are frozen.

- An unknown query parameter is ignored. Every HTTP handler deserializes its query into a typed
  struct without `deny_unknown_fields`, so `?limt=5` is silently the default, not a `400`. A new
  filter is thereby an additive change, and a typo is a wrong answer rather than an error message.
  Check the parameter name against the endpoint table above before blaming the filter.
- Where the HTTP name and the EDA name differ, they stay differing. The HTTP query says `?type=` and
  `?error=`; the EDA `query` object says `cell_type` and `only_error`. Aligning them, if it ever
  happens, happens as an alias: the new name starts working, the old name keeps working. The old
  names are part of the contract whatever a future spelling looks like.

Three reads carry an opaque `tag`: `/colony/ledger`, `/colony/graph` and `/colony/registry`. It
never filters and never touches the data; it is truncated to 64 characters and comes back verbatim,
in the `graph` object and beside the `registry` list respectively. A `/colony` reply starts a fresh
trace, so a cell that asks twice has nothing else to tell the two answers apart with.

Endpoint classification for cell emissions (W2d ruling 2026-06-12): the outputs arm dispatches a
`/colony/*` emission target directly, and not every endpoint is reachable from a cell.

- `/colony/mutations` is EDA-writable. A cell-emitted mutation is executed. It is the only writable
  dispatch endpoint.
- `/colony/registry`, `/colony/templates`, `/colony/graph`, `/colony/trace` and `/colony/ledger` are
  read-only and EDA-readable. A cell may read them via emission, with the reply going to its path.
- `/colony/dead_letters` is read-only and NOT EDA-dispatchable. The DLQ is read and drained
  exclusively via the dedicated inbox variants `ColonyMsg::ReadDeadLetters`/`DrainDeadLetters` (HTTP
  `GET`/`DELETE`). An emission there is an illegitimate write to a read endpoint: hard-rejected with
  one `colony_endpoint_unimplemented` DLQ entry, sender pass-through, terminal, never re-injected as
  a read reply. This prevents the source loop that the pre-W2d hardcoded fallback
  `unwrap_or("/colony/dead_letters")` triggered at the atomic-emitting cell types.
- `/colony/messages` is read-only and NOT EDA-dispatchable (P1 ruling 2026-08-07, analogous to
  `dead_letters`), reachable exclusively via `ColonyMsg::ReadMessages` (HTTP `GET`). ~~If topologies
  ever need message queries, that is a separate design pass over the `store`, not over colony
  endpoints.~~ Retracted in half (GH #267, ruling Q14 of 2026-08-21): the line runs between raw rows
  and counts, so `/colony/messages` stays non-dispatchable because it hands out header content,
  while aggregates over the same log are EDA-readable at `/colony/ledger`.
- An unknown `/colony/<x>` gives `colony_endpoint_unimplemented`.

What the `ledger` sums, and along which axis (GH #463): the endpoint sums exactly what is in the hop
header, `message_log.headers` under `$.hop`, and nothing else. Since #463 the `llm` cell writes the
whole usage block there: `tokens_prompt`, `tokens_completion`, `tokens_cached` (cache-read tokens,
whichever spelling the provider used), `cost` (only when the provider reports a cost figure of its
own) and `latency_ms`. `latency_ms` also stays in the body's `meta`.
A figure the provider did not report is an **absent** header key, not a zero. Beside those the
`ledger` sums `duration_ms`, which the tool cells (`file`, `bash`, `web_search`, `mcp`, `vault`)
already wrote. Both `latency_ms` and `duration_ms` come with a sample counter (`latency_samples`,
`duration_samples`): a mean is the sum divided by the samples, never by `calls`, because most hops
in a window carry no duration at all.

`?group_by=` picks a **second** grouping beside `by_model`: `model` (the default, `$.hop.model`),
`path` (`to_path`, the receiving cell) or `error_code` (`$.hop.error_code`). `by_model` is answered
either way, so a caller that sends no `group_by` gets the #267 reply shape back, with `by_path` and
`by_error_code` absent rather than empty. Every axis leaves out its NULL group. The aggregates rule
is unchanged: a group key is the axis the caller asked to group along, and rows, envelopes and
header contents do not leave the endpoint.

`?limit=<N>` (for `dead_letters`, `trace`, `messages` and the `mutations` audit read) defaults to
100 with a hard cap of 1000 and no config knob. `?scan_budget=<N>` on `messages` is the upper bound
on the rows read in stage 1 of the two-stage query (indexed predicates first, residual filters
`from_path_prefix`/`body_kind`/`correlation_id` afterwards), default 5000 and hard cap 50000; an
exhausted budget is reported in the reply as `scan_truncated`, so a possibly incomplete result is
never silent. On the `ledger` the same field bounds each of the three windowed sub-queries
(`message_log`, `dead_letters`, `mutation_log`) on its own, default 50000, clamped into 1 through
200000 rather than refused. There `scan_truncated` means one of the sub-queries exhausted its
budget, set for all three without saying which. The bound is `>=`, so at the edge the flag
over-reports and never under-reports.

## CLI

```
meclaw [options]
```

### Modes

The default mode is direct mode: a stdin/stdout bridge to the root cell, all on a single `meclaw`
invocation, for interactive sessions, simple pipes and tests. The process is stdin-driven; closing
stdin (EOF, for instance the end of a pipe) **drains the in-flight work and exits with exit code 0**
(Unix pipe semantics, so `cat input.jsonl | meclaw` terminates like `grep`). A shutdown signal
(SIGINT or SIGTERM) acts additionally.

The drain is a state of the colony loop in its own right, not a wait inside one iteration. After the
shutdown signal the loop accepts no new ingress (a source that fires meanwhile, a timer tick or a
proxy poll, lands in the dead-letter queue as `shutdown_draining`, and a build order is refused with
the same `error_code`), and it does let every in-flight message and its follow-on hops run to their
end. It ends as soon as the colony is quiescent: colony inbox empty, emission channel empty, no cell
mailbox occupied, no `handle()` still out. A `handle()` that never returns cannot hold the drain
forever, because `colony.json shutdown_drain_timeout_ms` (default 10 s) cuts it off and the
warning on stderr **names** what was left behind (`drain_incomplete`, `busy`). A cut-off drain is
still exit code `0`, and the diagnosis is on stderr as with every Unix tool. A watchdog trip skips
the drain and still ends non-zero.

Under a process supervisor the two numbers belong together: after the signal the process needs the
drain budget plus the teardown chain (shutdown ack, colony join, bridge join). At the default of 10
s that stays comfortably within a `TimeoutStopSec=30`. Whoever raises the drain raises
`TimeoutStopSec` with it.

`--daemon` (from phase 12) decouples the process lifecycle from stdin; stdin EOF no longer ends the
process, and the only shutdown triggers are SIGINT/SIGTERM and the internal watchdog.
That the stdin/stdout bridge is **not switched off** in the process, the mechanism preserved and merely running empty in daemon operation (systemd `Type=simple` provides stdin as `/dev/null`, so an immediate EOF without input, and stdout into the journal), analogous to nginx, is *(specified, not built - see GH #254)*.
Today one predicate gates both halves (`direct_mode = api.is_none() && !daemon && apply.is_none()`,
`crates/meclaw-cli/src/lib.rs`; since GH #423 `--apply` switches direct mode off just as `--api` and
`--daemon` do): under `--daemon` neither a stdin reader nor an egress writer is spawned, there is no
`ready` frame, and unrouted root output lands in the dead-letter queue instead of reaching stdout.
meclaw does not daemonize itself (no `fork`, no `setsid`), which is systemd
`Type=simple`-conformant; backgrounding is the outside world's business. External control runs via
the HTTP API and web UI, both opt-in via `--api`.

`--api <bind>` (from phase 12) activates the HTTP API and the operator web UI on the given bind
address, for instance `127.0.0.1:7777` (local-only) or `0.0.0.0:7777`. Without `--api` no port is
opened. The HTTP API is a thin translation layer over the `/colony/*` endpoints, and the web UI sits
on the same bind port under `/ui/*`. `--api` can be set independently of `--daemon` and is not
additive to direct mode: direct mode is exactly "neither `--api` nor `--daemon` nor `--apply`", so
any `--api` switches the stdin/stdout bridge off.

`--validate` (from phase 12) is a dry run: filesystem bootstrap, schema checks, template resolution,
mutation replay from `colony.db`, but no cell spawns and no HTTP listen. Exit code 0 if everything
is consistent, otherwise an error list on stderr.

The exit-code contract is narrow: `0` means it worked, and anything else means it failed. No code
carries a diagnosis, and a script must not branch on the specific non-zero value, because the values
are free to move. The diagnosis is on stderr.

`--validate` output is free text for a human to read, not a format to parse. Lines are added,
reworded and reordered as new checks arrive; whoever needs a machine-readable verdict uses the exit
code.

`--validate-strict` promotes the warning classes of `--validate` to errors, and the set of promoted
classes grows. A tree that passes strict validation today may fail it after an upgrade because a new
class was added, so it is not described as CI-safe: pinning a build to `--validate-strict` means
accepting that a meclaw upgrade can turn a green pipeline red on unchanged files. Use plain
`--validate` for a gate that only moves when your tree does.

`--rescan-templates` rebuilds the templates registry from the filesystem. By default templates are
scanned at the first startup and persisted in `colony.db`. If you have edited `templates/` by hand,
run `--rescan-templates` once.

`--apply <file|->` (GH #423) hands one manifest to `/colony/mutations` right after the boot and
prints the receipt. `-` reads it from stdin. The position is the argument: only after the boot does
the tree stand that the manifest mutates. The printed receipt is terminal prose for the caller; the
mutation receipt (GH #553) is a different thing and goes into the topology, and a one-shot run whose
receipt the immediately following shutdown still eats is healed by the next start's boot receipt.

| Invocation | Behaviour |
|---|---|
| `meclaw --root R --apply f` | One-shot: boot, apply, receipt on stdout, graceful shutdown, exit 0 on `committed` and non-zero otherwise. The stdin/stdout bridge is off, as under `--daemon` |
| `meclaw --root R --daemon --apply f` | Boot, apply, receipt, keeps running. A `rejected` does not end the daemon: the colony stands, the mutation does not, which is the audit semantics of every mutation. The line goes to stderr |
| `meclaw --root R --api A --apply f` | as `--daemon --apply` |
| `meclaw --root R --validate --apply f` | `--validate` has precedence, with a `note:` line on stderr |
| `--apply` against a held root | `LeaseError::Held`, with the message the lease already writes today. That is the refusal, and it is right: against a running colony you mutate through its HTTP door, and that door takes the same manifest body form, so it is one `curl` instead of five |

The exit-code contract above holds unchanged. The receipt itself is free text to read; a `rejected`
names the position, the `error_code` and how to resume.

### Flags

Flags are introduced phase by phase. At any point `clap` knows only the flags of the already
completed phases, and unknown flags are rejected with an unknown-flag error. `meclaw --help` thus
shows the respective functional CLI surface without misleading accepts-but-does-nothing flags. The
phase column below states in which phase a flag is first declared and becomes functional in `clap`.

| Flag | Phase | Default | Meaning |
|---|---|---|---|
| `--root <path>` | 0 | `.` | Filesystem root of the colony |
| `--log <path>` | 0 | `<root>/log.jsonl` | Tracing JSONL path |
| `--log-level <level>` | 0 | `info` (`colony.json log_default_level` is not consulted today) | Tracing level |
| `--log-filter <filter>` | 0 | none | `RUST_LOG`-style filter |
| `--version` | 0 | none | Version info |
| `--help` | 0 | none | Help |
| `--env <path>` | 6 | `<root>/.env` | `.env` file for variable substitution |
| `--templates <path>` | 11 | `<root>/templates` | Templates directory |
| `--rescan-templates` | 11 | off | Rebuild the templates registry |
| `--blobs <path>` | 12 | `<root>/blobs` | Blob storage directory |
| `--daemon` | 12 | off | Lifecycle decoupled from stdin, shutdown only via signal/watchdog, stdin EOF does not end (the bridge mechanism remains: *(specified, not built - see GH #254)*, today the bridge is not spawned at all under `--daemon`) |
| `--api <bind>` | 12 | off (no port) | HTTP API plus web UI on a bind address, for instance `127.0.0.1:7777` or `0.0.0.0:7777` |
| `--validate` | 12 | off | Dry run |
| `--apply <path>` | GH #423 | none | Applies a mutation manifest right after the boot; `-` reads from stdin. Without `--daemon` or `--api` it is a one-shot (boot, apply, receipt, shutdown) whose exit code carries the verdict. Against a running colony use its HTTP door, which takes the same body form |
| `--validate-strict` | 16 | off | Modifier for `--validate` only, without effect on its own: promotes the static findings that are warnings by default, non-resolvable `params.graph` endpoints and unregistered cell directories at reboot, to errors (a non-zero exit). The set of promoted warning classes grows with `--validate` |
| `--stdio-format <text\|json>` | P9 | `text` | Format of the stdin/stdout bridge: `text` is the raw line format (default, unchanged), `json` is wire-v1 JSONL (envelope reach-through for `trace_id`, `ttl` and `context`, plus a `ready` handshake) |
| `--sandbox-probe` | GH #97 | off | A question about the host, not a colony run: which `params.sandbox` properties this host can enforce. Needs no colony root, creates neither `colony.db` nor `log.jsonl`, always exits 0, and takes precedence over `--validate`, `--api` and `--daemon`. The same report is appended informatively to `--validate` (detail and example output: `config.md` § `sandbox`) |
| `--vault <CELL_PATH>` | GH #151 | none | The `vault` cell this invocation talks to, as its colony path (`/main/access/vault`). Required by every `--vault-*` mode |
| `--vault-add <NAME>` | GH #151 | none | Store a secret under this name in `--vault`. The secret itself is read from stdin, never from an argument, where it would land in `ps` output and in shell history. Writes straight into the vault's own database: no message, no message log, no context window |
| `--vault-status` | GH #151 | off | List what `--vault` holds: names and versions, never content |
| `--vault-revoke <NAME>` | GH #151 | none | Revoke every active version of this name in `--vault`. Needs no passphrase, because being locked out must never stop you from disabling a leaked credential |
| `--vault-key-source <SOURCE>` | GH #151 | `auto` | Where the vault passphrase comes from. It says SOURCE deliberately: the switch must never be able to carry key material. Default `auto`, where a credentials directory (systemd) wins, else the terminal prompts |
| `--vault-key-file <PATH>` | GH #151 | none | Key file for `--vault-key-source plainfile`. Refused unless it is unreadable by group and others, the same answer ssh gives |

The colony has no subcommands (`meclaw start`, `meclaw mutate` and the like). nginx-style:
one binary, many flags, one mode switch (`--daemon`, `--validate`, `--sandbox-probe`). Operations
are the outside world's business, whether systemd, a wrapper script or a builder LLM.

**Withdrawn with GH #623**: this used to read "meclaw deliberately has no subcommands", without
qualification, for the whole binary. The sentence falls because it ran two different things
together. An operating mode is a flag and stays one: it says how this colony runs. A client
command operates no colony, it addresses one, and for that a subcommand is the right form. As a
flag, `--api` would carry two meanings at once, the bind address of one's own server and the
address of somebody else's. There is exactly one such command, `ask`, and a second one would again
be a client, never an operating mode.

**Info-only flags are side-effect-free**: `--version` and `--help` print their information to stdout
and exit with 0, without initializing the tracing subscriber, without filesystem writes (in
particular no `log.jsonl` creation), without subprocess spawn. They act before the subscriber setup.
Tests for the subscriber setup path happen via direct unit tests of the setup function, not via CLI
subprocess calls.

### `meclaw ask`

`meclaw ask` sends one turn to a running colony and prints the answer. It operates no colony: no
root lease, no `colony.db`, no `log.jsonl`, no tracing subscriber. It speaks the two routes the
quickstart took as well, `POST /messages` and `GET /colony/trace`, so the command moves no
contract surface.

```bash
meclaw ask --api 127.0.0.1:7777 --target /door "Say hello in one short sentence."
```

| Argument | Default | Meaning |
|---|---|---|
| `--api <host:port>` | none, mandatory | Address of the running colony's HTTP API. A default would be a promise about a topology the substrate never made |
| `--target <cell-path>` | none, mandatory | Colony path of the cell the turn is addressed to, for instance `/door`. Mandatory for the same reason |
| `TEXT` | none, mandatory | The turn itself, positional, sent as a `user` turn |
| `--channel <id>` | a fresh `ask-<uuid>` | Channel the turn belongs to. Two calls are two conversations unless you say otherwise |
| `--timeout <secs>` | `120` | How long the answer is waited for |
| `--json` | off | Prints the answering trace row as JSON instead of its text. The row is re-serialised, so its keys come out alphabetically; the values are the API's |

The flow is the quickstart's `jq` pipeline in one command: a `POST /messages` with the turn, which
the colony acknowledges with 202 `{message_id}`; then `GET /colony/trace?trace_id=<message_id>`
every 500 ms until a hop of that trace travels on `hop.route` `answer` or `error`; what is printed
is that hop's first turn. Polling is legitimate here: this is a client outside the substrate, not
a cell inside it.

Exit codes: `0` on `answer`, `1` on `error`, `2` when nothing answered before `--timeout`. A
transport failure and a non-2xx answer to the `POST` are `1` as well - an error is not a timeout -
with the message on stderr and nothing on stdout. In the wait that follows this no longer holds:
the turn has been accepted, a single failed trace read is retried until the budget is spent, and
only a wait in which not one read succeeded reports the transport failure. A target that does not
exist is still acknowledged with a 202 and the message dies in the router, so the command reads
`/colony/dead_letters` on every pass as well and ends with `1` as soon as an entry for the posted
message appears there - `error_code` and target on stderr, instead of sitting out the budget. The
entry is matched by its `message_id` and not by its trace (GH #640). A trace holds everything the
turn set off, so a lane that emits on the side and finds nobody to consume it dead-letters under
that same trace while the answer is still being worked on; only an entry that killed the posted
message itself is a verdict on it. An entry with no `message_id`, written before that field
existed, is left to the wait.

Three limits belong to this. When the answering row carries no readable turn, because its body is a
blob, the note goes to stderr, stdout stays empty, and the exit code is still the route's. The
trace is read with `limit=1000`, the API's own cap; a trace whose answer lies further back runs
into the timeout. And every single HTTP request has ten seconds, so a silent server cannot outlive
the budget.

## Display cells (`web`)

A display is a cell of its own, reached under a mount. It is of type `web` (`cell-types.md` §
`web`), names `params.mount`, holds its own `cell.db` and answers under `/<mount>/` on the colony's
listener. A colony may have as many as it likes, the meclaw-os tree one and the website another,
and each comes into being by mutation like any other cell.

That replaces the `/surface/` model and retracts it (GH #383): a surface at `GET
/surface/<cell-path>` declared via `cell.surface` no longer exists in any part, the route and the
parser are gone, and `cell.surface` is today an unknown key and therefore a hard boot refusal
(`config.md` § `cell`). Migrating a 1.x canvas: `templates/canvy/MIGRATION.md`.

Four things answer under a display's base, and the order matters because the last one is a
wildcard. The base is the sanitised `X-Forwarded-Prefix` a proxy sends plus `/<mount>`:

```
GET  <base>/live/websocket    the Phoenix socket (vsn 2.0.0)
GET  <base>/@client/<file>    the LiveView bundles, from the binary
GET  <base>/ and <base>/<route>   a page out of the pages table
GET  <base>/<anything else>   a file out of the assets table
```

The shell writes its own links out of that base, so a proxy may put a display on a domain root, on
a path or on a subdomain. `page.set` routes stay names (`/`, `/a/b`): the LiveView join carries the
page URL, and the base is stripped off it again. A header value that does not match
`^/[A-Za-z0-9._~/-]{0,200}$`, or one with a trailing slash, is ignored, and so are the two shapes
the grammar itself lets through: a leading `//`, which is a host and not a path, and any `..`
segment.

**One listener for the rest.** `--api` is the colony's one listener, and it is the only door a
colony opens. A surface cell registers a name (`params.mount`) and is reached on it under
`/<mount>/…`. The listener reads the first request line and hands the connection over unread to the
cell that holds the first path segment; from there the cell serves its own protocol on its own
routes. A connection is decided once, on that first line, and a second request on it is not
inspected again. A mounted cell whose handoff queue is full answers `503 surface busy` and closes. A
connection that sends no complete request line within five seconds, or a line above 4 KiB, is
dropped without an answer. `GET /colony/surfaces` publishes the mount table, one row per surface
with its `mount` and its `kind`, and `?format=traefik` the same table as a map for the proxy in
front.

One domain in front of one colony, with each surface on its own path:

```nginx
location /alex-display/ {
    proxy_pass http://127.0.0.1:7777;
    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
}

location /alex-voice/ {
    proxy_pass http://127.0.0.1:7777;
    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
}

location /colony/ {
    auth_basic "colony";
    auth_basic_user_file /etc/nginx/colony.htpasswd;
    proxy_pass http://127.0.0.1:7777;
}
```

A subdomain that carries several colonies gives each one a path of its own and names that path in
the header, so the shell keeps writing links the browser can follow:

```nginx
location ^~ /egon/ {
    proxy_pass http://127.0.0.1:7777/;
    proxy_set_header X-Forwarded-Prefix /egon;
    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
}
```

The display is then at `https://<host>/egon/alex-display/`. Several displays behind one domain
share one browser origin, so cookies, storage and whatever tells two members apart are the proxy's
business, and a page that keeps something in browser storage keys it by mount.

**The `pages` table is the only route source.** What a browser gets stands in the cell's database,
not in a declaration in the `cell` block and not in a second namespace: a route is a name (`/`,
`/a`, `/a/b`, segments of `[a-z0-9-]`), not a URL, and what no row names is a 404. A GET asks the
page map first and the asset map second, in one handler, because two competing wildcard routes would
let the router's matching order decide which table a path can reach at all. If both declare the same
path, the page answers.

Auth and TLS are external, forever (R-W8-2). This cell type does not authenticate and never will,
and a reverse proxy sits in front of the colony's listener. A display sees nothing but its own
database.

**Who renders what.** What is served is a snapshot the cell's handler half published earlier: the
route was rendered once and already sits in LiveView's packed form, so a page load costs zero cell
calls, touches no database and does no diff work. There is exactly one exception, and it carries no
new information: a viewer whose channel was full when the fan-out reached it is offered its route's
whole packed tree instead (GH #414), until it can take it. The last frame a viewer receives is
therefore always the newest state, never a diff onto a picture it never got. A wedged colony keeps
serving the page. Everything a display draws is rows in its database, so components are data rather
than code, and a new picture arrives as a message into the running colony.

**Two classes of browser event, and the declaration decides, not the name**
(R-W8-5). An `object:set` on a prop the component declared `editable` is local CRUD on the cell's
own `cell.db` plus a diff to every joined viewer. No message is created, and the event never leaves
the cell. Every other event is a semantic source emission on `hop.route = "event"`, in the same
shape the `proxy` cell uses for an inbound platform turn, and the header carries `event_name`,
`session_id` and `page_route`. With `params.identity_header` set, the value of that request header
rides along as `hop.user_id` and the ingress edge promotes it; an empty param stamps nothing,
because a header a client can set without a proxy in front is not an identity. The cell interprets
no event name of its own: what one means is
decided by the out-edges, which is what keeps a display ignorant of the topology it hangs in.
Working example: `templates/canvy`.

A display needs no return path any more. It answers its own browser: the listener hands it the
connection, so the answer never leaves the colony as a message at all.

Retracted: `--api` takes `EgressPolicy::Marked` (GH #383). `--api` opens no second door today; it
serves the `/colony/*` endpoints and the operator web UI and nothing else. What remains is the
policy itself, as substrate machinery:

| Policy | Meaning |
|---|---|
| `All` | Everything that dies at the root hive goes out. In direct mode stdout is the only consumer there, so a dead end is an answer. Today the only wired case. |
| `Marked(key)` | Only messages carrying that key in `context` go out; every other one lands unchanged in the dead-letter queue. Today without a caller in the shipped binary. |

The split exists for the DLQ: a door that silently swallowed every unroutable message would make
every "why did that message vanish" unanswerable.

The door is not a place (GH #163, ruling 2026-08-17). The policy decides what leaves and where it
leaves from. `All` stays at the root hive `/`; `Marked` needs no geography, because the marker is
stamped by the injecting layer and unforgeable by a cell. Since #163 a display's answer lane is `->
.` instead of `-> /`, so a `web` cell is installed into a running colony by mutation and serves
within the same boot.

The four absolute edges a mutation may draw are `-> /colony/graph` (GH #163), `-> /colony/registry`
(2026-08-27), `-> /colony/ledger` (GH #267) and `-> /colony/templates` (GH #496). They address no
cell but the authority's own read-only endpoints, dispatched before any edge is consulted, and the
graph is the sanctioned way to learn topology, because § Database isolation forbids reading
`colony.db`. The ledger qualifies because it **answers counts and never content**, sums over one
time window with no raw row and no header. The registry answers the colony's bookkeeping about its
own cells (`path`, `cell_id`, `cell_type`, `lifecycle_status`, `active`, `failed`), strictly less
than the graph hands out. The templates read answers `template_id`, `name`, `version`,
`filesystem_path` and `author`, classes rather than instances (GH #496). `/colony/templates/rescan`
stays out, because it is an effect, not a read. `/colony/mutations` (authority transfer),
`/colony/trace` and `/colony/dead_letters` (other cells' message content) stay out of bounds.

What the binary does not understand is an event name. The `web` cell reads it itself and decides
solely from the component's `editable` declaration whether it stays local or leaves as an emission;
what it means is decided by the edge that picks it up.

### Web UI (operator inspection)

`--api <bind>` activates, besides the JSON API, an operator web UI on the same port under the path
prefix `/ui/*`. Root `/` redirects to `/ui/`. It is server-rendered HTML via `maud`, with no
JavaScript, no auto-refresh and no CSS framework. It is read-only, with no mutate forms, because
mutations are the builder's matter (`builder` drafts, `submit` hands it in), and symmetric to the
`/colony/*` endpoints: a web UI route internally calls the same read endpoint as an API route.

| Web UI path | Content | Data source |
|---|---|---|
| `/ui/` | Dashboard: cells overview, status counts, latest errors, latest dead letters. No consistent snapshot: three independent reads from three moments | aggregated from `/colony/registry` plus `/colony/dead_letters` plus `/colony/trace?error=true` |
| `/ui/registry` | Table of all cells with a filter form (path prefix, type) | `/colony/registry` |
| `/ui/graph` | Topology of a scope (nodes as a list, edges as a table), form `?scope=` | `/colony/graph` |
| `/ui/dead_letters` | List of the most recent dead letters with `error_code`, path, body preview and an "Original" link to the originating message in the message browser, where present in the `message_log` | `/colony/dead_letters` |
| `/ui/trace` | Trace search form (`trace_id`, `path_prefix`, `error`, `since`, `limit`), result as tree HTML by `parent_message_id` | `/colony/trace` |
| `/ui/templates` | Template overview with filter `?type=` | `/colony/templates` |
| `/ui/messages` | Message list newest-first with a filter form plus keyset paging, truncated payload, scan-budget disclosure | `/colony/messages` |
| `/ui/message` | Single message: `hop` and `context` headers rendered separately, payload pretty-printed, blob on demand, pivots (trace, parent chain, correlation, `reply_to`, dead letters) | `/colony/messages?id=` |

Auth, once phase-12 hardening lands, is uniform middleware in front of `axum`'s router and applies
to the JSON API and the web UI alike. Until then: local discipline, with `--api 127.0.0.1:7777` as
the safe default.

### Stdin/stdout bridge (direct mode)

In the default mode a stdin/stdout bridge runs: stdin is converted into messages to the root cell
(one line is one message), and stdout shows messages emitted from the root cell. The default format
is text (grep- and Unix-conformant, structurally identical to `proxy`): one stdin line of raw text
becomes a body with exactly one `user` turn
(`{messages:[{origin:"user",type:"text",text:"<line>"}]}`) plus a fresh `turn_id`, and to stdout the
`text` of the last `assistant` turn of an emitted message is written. This text format is
byte-identical to v0.1.0 and remains the default.

Since P9 there is additionally the opt-in flag `--stdio-format json`, wire v1: one JSON line per
message, at startup a `ready` frame carrying the protocol integer `v` (asserted strictly) and the
reported release `version`, then `message` and `error` frames in both directions. Envelope
reach-through covers four fields. `trace_id` is carried (GH #190): absent or `null` mints a fresh
trace downstream, a UUID string carries exactly that trace unchanged across the process boundary,
and every other JSON type, as well as a string that is not a UUID, is `invalid_frame`. `ttl` is
decremented (GH #187): absent or `null` is the substrate default, a positive integer in
`1..=4294967295` is that hop budget, and every other value (a string, a negative number, a float,
`0`, or one above `u32::MAX`) is `invalid_frame`. `context` is explicitly mapped (GH #182): absent
or `null` is an empty context, an object lands verbatim in the `context` compartment, and every
other JSON type is `invalid_frame`, because a silently dropped context costs the sender the
`turn_id` it correlates the reply on. `hop` is an opt-in seed (GH #180): absent or `null` is an
empty hop, an object lands verbatim in the `hop` compartment, and every other JSON type is
`invalid_frame`.

The hop reach-through is one-directional. Inbound the caller asserts the lane (without it a line
sent at a hive path matches no door); outbound the frame carries `context` back and no `hop`,
because a hop is a single-hop compartment with no meaning on the far side. An additional optional
inbound field is not a v2, and neither is tightening an existing inbound field (GH #182, GH #187, GH #190):
what is frozen is the shape a reader must be able to parse and the negotiation step, not the
strictness of inbound validation. `v` is strict and there is no negotiation: the bridge asserts the
integer it expects and fails on anything else. A v2 can only ship together with a negotiation step,
never as a bumped integer on the same handshake, because a bare bump would leave every existing peer
with a hard failure and no way to ask for the old wire.

The bridge is an I/O detail of the `meclaw-cli` crate and not a cell type, so the root cell stays
exchangeable. Rejected were an own `stdio` cell type and "interactive use only via `proxy` or the
HTTP API".

Topologically the bridge plays the parent hive of the root, which does not exist in the substrate,
and shoves messages between the stdio level and the `/` level as a hive does between its levels. The
root cell must therefore be a hive (`type: "hive"`): only the hive carries the graph. Ingress (stdin
to topology): the bridge is the birth point of the message and establishes the initial `context`
directly, symmetric to the HTTP ingress, with the same context triad as the `proxy`: a fixed
well-known `user_id` (the stdio user, identical across all runs), a `chat_id` per process run (start
until EOF is one stdio session), and a fresh `turn_id` per stdin line; emitted with `sender =
@external` to the root cell. Egress (topology to stdout): a message that runs back to the root hive
`/` and matches no further out-edge there is translated to stdout instead of being dead-lettered as
`HiveNoRoute`. stdio is an absolute endpoint, a pure sink. In JSON mode stdio is additionally the
composition boundary for sub-colonies: a parent colony operates a whole child colony as one cell
(`cell-types.md` § `subcolony`). That is composition and not federation: the child tree stays
unaddressable from the outside, with no path reach-through to child cells and no parent mutation of
the child tree.

Lifecycle (§ Modes): in direct mode stdin EOF is a shutdown trigger, a drain plus exit 0, and since
GH #47 a real quiescence drain. Under `--api` as under `--daemon` the bridge is not spawned at all,
so stdin EOF is not a trigger and shutdown runs via signal or watchdog.
That the bridge remains as a mechanism is *(specified, not built - see GH #254)*.

## Path addressing

```
/<hive>/<sub_hive>/<cell>    absolute, from colony root
/memory/2026-05-16/cache     example with application-specific hierarchy
/colony/registry             virtual colony endpoint
./cell_y                     relative to the sender path
../other_sub/cell_z          relative to the sender's parent directory
```

- `{root}` is the filesystem starting point of meclaw, in path notation `/`.
- Path resolution is a pure string operation on the sender path and the target expression. `.`, `..`
  and the `/` prefix are normalised to an absolute path.
- The lookup is O(1) on colony's central `HashMap<Path, ActorHandle>`. No hop-by-hop, no cascade
  across several routing tables.
- Paths are stable until somebody changes one on purpose (see No-delete policy).

### Routing algorithm

`route()` does four things in this order. It normalises the target against the sender path. It hands
a `/colony/...` target to the virtual-endpoint handler and returns. Otherwise it looks the target up
in the registry: on a hit it writes the central message-log row and sends to the handle; on a miss
it runs the dead-letter cascade.

| Sender path | `msg.target` | Resolved to |
|---|---|---|
| `/main/agent` | `./tool` | `/main/agent/tool` |
| `/main/agent` | `../collector` | `/main/collector` |
| `/main/agent` | `/other/cell` | `/other/cell` |
| `/main/agent` | `/colony/templates` | `/colony/templates` |

A `target` that points to a hive scope marker is addressable, and colony never delivers there. It
evaluates the hive as a logical transit node in the same routing layer: it takes the out-edges of
the hive (`EdgeTable` entries with `from = <hive-path>`), checks their CEL `condition` against the
headers, applies the `modifier`, and per hit triggers a regular routing hop to the respective `to`
path. The TTL is decremented per hop.

If no out-edge matches (the edge list is empty or all CEL conditions evaluate to `false`), the
message goes to `/colony/dead_letters` with its own `error_code` `hive_no_route` (a canonical
string, `DeadLetterReason::HiveNoRoute`), and never `unresolved_path`: the hive was reachable and
the routing graph did not forward it, which is a distinction a builder needs.

A hive may consume that remainder by declaration (GH #283, since v0.18.0): one of its out-edges
(`{"from": "."}`) carrying `"default": true` takes exactly the traffic that would otherwise
dead-letter as `hive_no_route`. Everything § Edge model says about `default` applies unchanged, and
whatever a guard on it excludes still dead-letters as `hive_no_route`.

Mutations for a hive scope still go to `/colony/mutations` with the hive path in the scope field of
the mutation body, never to the hive path as the `target`.

The hive boundary is checked before it, at the call site and not in the corridor — the same
construction as the `cell_inactive` pre-check: a source message whose target is a cell inside a
sealed hive is refused there (`hive_boundary`, GH #612) before it enters the work queue, and it
therefore spends no TTL either.

`route()` is pure: all logging, sync and metric evaluation lives in the call-site wrapper
`route_with_log` ([`crates/meclaw-colony/src/colony.rs`](../crates/meclaw-colony/src/colony.rs)),
which does the pre-check and snapshot before the call and the log send after the return. The body is
byte-frozen against `.github/fixtures/expected_route_body.txt`, and `#[rustfmt::skip]` over
`route()` keeps the committed form stable while `cargo fmt` runs freely.

### Path resolution edge cases

`Path::resolve(sender, target)` always returns a `Path` and never a `Result`. Resolution is a pure
string normalisation and cannot fail; whether the resulting path exists is a separate question the
downstream registry lookup answers. `route()` therefore has exactly one error source.

| Input | Behaviour | Rationale |
|---|---|---|
| `../` beyond the root (sender `/a`, target `../../x`) | clamp to `/`, yielding `/x` | Linux convention (`cd / && cd ..` stays `/`). No error path needed. |
| Empty target `""` | resolves to `sender_path`, identical to `.` | An empty string means no hop. |
| Bare name without a prefix (`cell`, with no `/`, `./` or `../`) | relative to the sender, like `./cell` | The most natural reading of a prefix-less string, shell-analogous. On non-existence it lands via a regular registry miss in `dead_letters` (`UnresolvedPath`). |
| Trailing slash (`/a/b/`) | normalised to `/a/b` | Consistent key form. |

### Routing errors and the dead-letter queue

When a resolved path does not exist in colony's registry (a cell removed by mutation, a subtree
never instantiated, a typo):

1. If `reply_to` is set, an error message goes back to `reply_to`.
2. If `reply_to` is `None`, the message goes to `/colony/dead_letters`.
3. Colony logs with `trace_id`, resolved path, original target and reason.

`/colony/dead_letters` is a colony-internal construct and not an entry in `HashMap<Path,
ActorHandle>`. The DLQ is persistent in `colony.db` (table `dead_letters`, `schema_version` 4) and
survives shutdown and crash. The DB is the only truth: read and drain query the table directly, and
an in-memory `VecDeque` is a transient hand-off buffer the single-owner `colony_task` flushes after
every event. Old rows are never dropped, and a fire-and-forget write keeps backpressure away from
routing. Each row carries the six localisation fields (`DeadLetterDto`, plus `message_id` since P1,
`None` for legacy rows) and the full serialised envelope (`message_json`), so the drain reconstructs
the complete `DeadLetter`. Unimplemented `/colony/<x>` paths and `/colony` without a sub-path also
land in the queue, with reason `ColonyEndpointUnimplemented` or `ColonyEndpointInvalid`.

Every entry carries six locating fields plus `message_id`, and since GH #612 an optional `detail`:
the one reason-specific fact the locating fields cannot carry. It is one value per reason, machine
readable rather than prose — for `hive_boundary` the absolute path of the hive that refused the
address. Every other reason has none today and omits the field.

**Canonical `error_code` strings**: every dead-letter reason (internally a `DeadLetterReason` enum
variant) has a canonical string representation exposed in the dead-letter queue as the `error_code`
field, which is what the `?error_code=` filter matches: `unresolved_path`, `hive_no_route`,
`no_route`, `cell_inactive`, `ttl_expired`, `colony_endpoint_unimplemented`,
`colony_endpoint_invalid`, `blob_unavailable`, `blob_recursion_too_deep`, `invalid_ubf_body`,
`consumes_violation`, `contract_violation`, `slot_unbound`, `slot_park_overflow`,
`shutdown_draining`, `hive_boundary`. These strings are part of the stable API contract; new reasons
extend the list, existing ones do not change their string form. `shutdown_draining` (GH #47) carries a new source
emission that arrived during the shutdown drain; it is not routed, because that would start work the
drain would then have to wait for.

Notes on the delivery-boundary codes:

- `blob_unavailable`: blob resolution failed at the delivery boundary, because a `Body::Blob` uuid
  is not findable, or an in-message pointer names a blob that is missing or has a shape that cannot
  be spliced (GH #19).
- `blob_recursion_too_deep`: the recursive resolution of an in-message pointer either exceeded
  `blob_max_recursion_depth` or re-entered a blob already on the same path (a mutual cycle). Both
  carry the same code; whether it was depth or a cycle is in the log line rather than on the wire.
- `invalid_ubf_body` has a debug-versus-release contract: this code is constructed **only in the
  debug build**. The body structure validator in the `colony_task` `outputs_rx` arm runs under
  `#[cfg(debug_assertions)]` and DLQs malformed cell emissions as `invalid_ubf_body`. In the release
  build that structure validation is inactive; the string remains canonical and stable, but its
  occurrence is build-profile-dependent (D-033). Do not confuse it with the
  `contract.emits`/`consumes` schema validation, which is controlled via `colony.json`
  `strict_validation`.
- `consumes_violation`: a message missed the substrate-side required `consumes` check at the
  delivery boundary, and the cell was not invoked (`config.md` § consumes).
- `contract_violation`: a non-`code` emission violated its `contract.emits` at the central check of
  the outputs arm (flag-gated), and the emission was discarded. With `input_reply_to` an error reply
  is routed instead, with no DLQ entry. Same canonical token as the `code`-in-cell reply.
- `no_route`: a cell emission that matches no out-edge of its sender (an empty edge list, or all CEL
  conditions `false`) lands in the DLQ, the cell analogue to `hive_no_route`. The substrate has no
  implicit identity fallback (ruling A1), and an unconditional out-edge
  is **not a default: it is an always edge**. Retraction (GH #283, v0.18.0): this used to read "the
  substrate has no fallback construct today; a topology that wants a real default spells it out as
  the negation of every other arm", and that is withdrawn. An out-edge carrying `"default": true` is
  the declared consumer of exactly this `no_route` traffic (§ Edge model); the always-edge statement
  stays true for every edge without the key. The entry is self-localizing over four fields.
- `slot_unbound`: a message reached a hive's declared slot (a `params.ports` entry carrying `"slot":
  true`) while nothing was bound behind it, and the hive declared `"unbound": "error"` (GH #285).
  Not `unresolved_path`: the address is announced and empty. `resolved_target` is
  `<hive-path>/<slot-name>`. `"unbound": "drop"` produces no dead letter at all, a bound slot is an
  ordinary address, and a message that addresses the slot path directly from outside stays
  `unresolved_path`.
- `slot_park_overflow`: a message reached a declared slot with `"unbound": "park"` whose queue
  already held `colony.json slot_park_max` messages (GH #285). The newest arrival is refused, not
  the oldest. `resolved_target` is the slot address, as for `slot_unbound`. A `park` slot below the
  bound produces no dead letter: it holds the message and releases its queue, in emission order,
  once something is bound. A colony shutdown discards whatever is still parked.
- `cell_inactive`: the target path exists (a cell or a hive) and is disconnected or inactive; it
  also applies to mailbox residue on disconnect.
- `hive_boundary`: a message from **outside** the colony (the HTTP ingress, and every other source
  message) named an address **inside** a sealed hive, and the hive did not declare that address (GH
  #612). Not `unresolved_path`: the path exists. Not `hive_no_route`: the hive was never asked to
  forward. `resolved_target` is the address that was named, `detail` the path of the hive that
  refused it. What a hive declares is in § The hive boundary. The refusal happens **before** the
  routing corridor, so no `message_log` row is written: a boundary refusal lives in the dead-letter
  queue and never in `/colony/trace`. It is joinable all the same — the entry carries the
  `trace_id` and the `message_id` of the message that was posted.

When processing a cell emission in the outputs arm, exactly one of three disjoint paths applies, in
this order (ruling A1, 2026-06-12):

1. `em.target` is a `/colony/*` endpoint, so it takes a direct ColonyDispatch (registry or
   virtual-endpoint lookup via `route()`) before edge evaluation. An out-edge is there neither
   needed nor possible, and the A1 no_route rule does not apply. An unknown `/colony/<x>` endpoint
   gives `colony_endpoint_unimplemented`. This is the delivery path for cell-emitted mutations and
   reads.
2. A substrate-generated error reply to a known sender (`consumes_violation`, the `message_timeout`
   backstop, `contract_violation`) goes directly to `reply_to` (registry lookup via `route()`) and
   not via out-edges. It is feedback to a known sender, so a missing out-edge may neither redirect
   it nor turn it into `no_route`. An unresolvable `reply_to` gives a DLQ entry (a one-shot
   cascade).
3. A normal emission without a matching out-edge gives a `no_route` DLQ entry. A1 governs
   exclusively case 3.

The cascade is one-shot and not recursive: error replies (step 1) themselves set no `reply_to` and
are terminal. If an error reply is itself not deliverable, step 2 takes effect automatically.
Maximum cascade depth is therefore two hops: original error, reply attempt, dead letter. Rejected
were header-based loop detection, TTL as a cascade backstop, and a configurable cascade depth.

Colony processes its mailbox sequentially, so while a mutation runs, staging and registry edits
included, other messages wait. Afterwards colony routes with the new registry state, and a message
targeting a cell removed in the meantime goes through the cascade above.

meclaw knows no wildcards. Fan-out is solved at the edge level (one output, several edges). Pub/sub
patterns are not planned.

Routing is symmetric: cell to cell and cell to colony both run through the same path resolution plus
registry lookup, and `/colony/*` paths are virtual endpoints in the same registry.

## Cell model

- A directory with `config.json`, written by colony only on instantiation and a bootstrap snapshot
  afterwards.
- Optionally a `cell.db` (SQLite, persistent parameters and state, cell authority, `db:own`
  capability).
- Optionally `seed/<table>.jsonl` (bootstrap data and export target).
- One uniform actor concept: every cell is registered in colony's `HashMap<Path, ActorHandle>`
  registry with one `ActorHandle`, uniform for all cell classes and with no sum type. The handle is
  at its core an `mpsc::Sender<Message>` plus path and cell-type metadata, so colony's routing code
  is identical for all cells: `handle.send(msg).await`.
- Three spawn strategies behind that uniform handle. **Stateful** cells are not reentrant and get
  one long-lived `cell_task` Tokio task that pulls the mailbox in a loop and calls `cell.handle()`
  directly. **Stateless** cells are reentrant and get one long-lived `stateless_dispatcher` task
  that pulls the mailbox and spawns a short-lived worker task per message which runs
  `factory.invoke()` and terminates, with a per-cell concurrency limit via `tokio::sync::Semaphore`
  (`params.max_concurrency`). **Long-running** cells (`proxy`, `timer`, `mcp`, `web`, `voice`) use
  the double-task pattern, both sub-tasks under one logical cell identity.
- No inner loop in cell code: cells wait for incoming messages, or for external events in the
  long-running types. Iteration is a topology matter.
- Every cell declares `contract.emits`, `contract.consumes`, `contract.settings` and
  `contract.capabilities` (see `config.md`).
- Knowledge is limited: a cell knows message plus params, and not the sender path, the receiver path
  or other cells. Envelope fields (`id`, `trace_id`, `parent_message_id`, `correlation_id`,
  `target`, `reply_to`, `ttl`, `created_at`) are read-only from the cell's perspective and are set
  exclusively by colony during routing.

### Output path

Cells emit outputs via a cloned `outputs_tx` that goes to colony's central `outputs` mailbox. The
cell trait signature is uniform for all cell classes:

```rust
trait Cell: Send {
    fn handle(
        &mut self,
        msg: Message,
        outputs: &mpsc::Sender<OutputEnvelope>,
    ) -> impl Future<Output = ()> + Send;
}
```

It is `impl Future<Output = ()> + Send` instead of `async fn` because native AFIT binds no `Send`
guarantee to the returned future, and in generic contexts (`cell_task<C: Cell>(…)`,
`ColonyHandle::spawn<C, F>(…)`) the compiler would then not know the future may travel to a worker
thread via `tokio::spawn`. Return type notation would be better and is not yet stable. Every cell
implementation either writes `fn handle(...) -> impl Future<Output = ()> + Send { async move { ... }
}` or keeps `async fn` with a `Send` bound in a `where` clause. The `Cell: Send` supertrait is
mandatory for the same reason.

A cell never emits a finished `Message`; it does not know the envelope fields. It pushes
`CellOutput` values over `outputs_tx`:

```rust
struct CellOutput {
    target: Path,                  // set directly by the cell in phase 3; from phase 4 typically from edge evaluation
    content: serde_json::Value,    // content JSON with optional "header" section; colony extracts header → message.headers, rest → body
}
```

Colony decomposes the `content` JSON: `content.header` becomes `message.headers`, the rest becomes
`body: Body::Inline(...)`. From phase 4 the edge evaluation overlays the `target` the cell set.

The parent context is attached by `cell_task`, not by colony, which would otherwise run the cell in
its own task and block it for the duration of the call. `cell_task` holds the consumed incoming
`Message` as a local stack variable and enriches each pushed `CellOutput` with the context only it
knows: `parent_message_id` (the `id` of the consumed message), `trace_id` (copied from the consumed
message) and its own `sender_path`. That enriched package goes to colony's `outputs` mailbox, and
colony sets the remaining envelope fields (`id`, `reply_to` as `sender_path`, `ttl` decremented,
`created_at`) and routes, with no shared state and no lock.

| Cell class | `outputs_tx` lives | Who calls `outputs_tx.send().await` |
|---|---|---|
| stateful | in the `cell_task` local, cloned once at spawn | `cell.handle()` |
| stateless | passed through as a parameter at worker spawn | `factory.invoke()` in the worker |
| long-running handler task | in the handler local, cloned once at spawn | `cell.handle()` |
| long-running I/O task | has **no** `outputs_tx` | none; the I/O task only pushes internally to the handler |

Colony's main loop runs as a `tokio::select!` over its own routing inbox (incoming messages from the
HTTP API, mutations, re-routed messages) and the `outputs` mailbox (cell emissions). Both paths land
in the same routing logic: edge evaluation, header modification, target resolution, then either
`handle.send` for a cell target or hive-transit evaluation for a hive scope marker. The substrate
has exactly one routing layer with no bypass paths, and hive transit is a branch of it.

Atomic-emitting cells call `outputs.send` once per `handle()` call; stream-propagating `code` cells
can send several times. Backpressure takes effect equally on every `send` call.

### Stateless cell dispatcher

The dispatcher pulls its mailbox in a loop, acquires an owned permit from a `Semaphore` sized by
`max_concurrency`, and spawns one worker task per message that builds an `OutputSink` and runs
`cell.handle(msg, &sink).await` before dropping the permit.

It is generic over `<F: StatelessCell + 'static>` rather than `Arc<dyn StatelessFactory>`, because
`StatelessCell` uses RPITIT and the trait is therefore not object-safe: monomorphisation per cell
type, `stateless_dispatcher::<FileCell>` and so on.

The worker, not the dispatcher, builds the `OutputSink` from the message metadata (`msg.id`,
`msg.trace_id`, `msg.ttl`, `msg.headers`). The sink encapsulates `outputs_tx` plus its own path and
provides the `emit()` interface for `cell.handle()`. `drop(permit)` at the end of the worker closure
releases the semaphore slot only when `cell.handle()` has completed, so `max_concurrency` is a hard
cap on actually concurrent `handle()` calls rather than on spawned tasks.

`max_concurrency` is an optional cell param (`params.max_concurrency`, see `config.md`). Defaults
per cell type:

| Cell | Default `max_concurrency` | Rationale |
|---|---|---|
| `file` | 8 | Disk I/O; the OS I/O queue saturates early |
| `bash` | 4 | Subprocesses are resource-intensive (memory, FDs, scheduler) |
| `edit` | 8 | Disk I/O like `file` |
| `web_fetch` | 32 | HTTP provider rate limits are typically tolerant, and the connection pool limits anyway |
| `web_search` | 8 | Search APIs are more strictly rate-limited than simple HTTP GETs |

The dispatcher is a concurrency guard, not a spawn loop: `acquire_owned().await` slows it down
before it pulls further from the mailbox, so under overload the mailbox fills up and backpressure
propagates backward.

### The double-task spawn

The pattern itself is under § Concurrency and parallelism; what belongs here is why it exists and
how it is supervised. A 30-second long poll to Telegram, a `tokio::time::sleep_until` until the next
schedule firing, or a blocking MCP SSE read must never block the acceptance of new messages, and
conversely a full external mailbox must not stall the polling. A single task could only do that with
a `tokio::select!` that loses provider state whenever it cancels the I/O future. The I/O task
receives reconfigure hints from the handler over a second internal channel.

The naive spawn is two `tokio::spawn` calls, and that is not what runs: fire-and-forget would
swallow sub-task panics, leaving the cell silently dead without a restart. The real spawn
(`cell_task_long_running`) is itself an outer task with exactly one `JoinHandle` that the supervisor
observes, which keeps the `RespawnFn` signature byte-identical to the single-task pattern. That
outer task selects over the `JoinHandle`s of both sub-tasks, aborts the surviving sibling on the
first completion, awaits **both** results rather than only the winning `select!` arm (a handler
panic closes `run_io` too via the dropped `reconfig_tx`, and the other way round), and re-raises a
panic via `std::panic::resume_unwind`, so the supervisor sees `was_panic=true` and restarts
`one_for_one`. Panic propagation is therefore order-independent of the `select!` outcome
(AUDIT-PRE14-001).

The internal mpsc from the I/O task to the handler is bounded, so an overloaded handler throttles
the external polling frequency (see § Backpressure).

Long-running cells are permanently awake; idle despawn makes no sense here, because the reason they
exist is continuous external polling. `cell.timeout: -1` is the typical configuration. Their
handlers typically have `cell.message_timeout: 0` or `-1` (no backstop), because a single `handle()`
call here can be long, for instance a long-running MCP tool call. Operation timeouts
(`params.external_timeout_ms`) remain mandatory for every I/O operation in the handler. What exactly
the I/O task polls and what the handler holds is described in `cell-types.md` per type.

Rejected were a single task with `tokio::select!` over the mailbox and the I/O future, a short-lived
I/O task per mailbox message, and clamping a long-running cell as two cells under a hive.

### Lifecycle of `config.json` and `cell.db`

| File | Who writes | When |
|---|---|---|
| `config.json` (cell) | Colony | **only** on instantiation (template copy, UUID assignment, `${VAR}` substitution); the `swap_nodes` graph swap does not rewrite an existing `config.json` |
| `cell.db` (cell) | the cell itself | after param updates via message |
| `colony.db` | Colony | on instantiation, mutation commit, templates scan and message-log write |

After instantiation `config.json` is a frozen bootstrap snapshot. Live state lives exclusively in
`cell.db`. Param updates that come via message are persisted by the cell in its `cell.db`, and
`config.json` is not co-written. On a cell reset, for instance a wipe of the `cell.db`, the cell
starts with its bootstrap state from `config.json`.

Hive scope markers have a `config.json` with `type: "hive"` and a `params.graph` field (the initial
desired graph for their subtree). They have no `cell.db`; the running graph lives in colony's
registry and `colony.db`.

The `cell.db` connection lives in the `cell_task_stateful` stack frame, not in a cell field. Cells
implement `StatefulCell` (in `meclaw-colony`, not `meclaw-core`, the same layer separation as
`CellFactory`) and get `&mut DbConn` as the handle param. `cell_task_stateful` is the only authority
over the cell.db lifecycle: it opens at spawn via `open_or_create_cell_db`, reopens on restart via
the factory RespawnFn closure, and closes at mailbox disconnect or cell-task panic via Drop. Because
`&mut DbConn` can be held across `.await`, cells may mutate state after or between output emits.

`DbConn` encapsulates `rusqlite::Connection` plus `rusqlite::InterruptHandle`: `Send`, a single move
into the timer task, no `Clone`, no mutex exception. rusqlite calls are offloaded via
`DbConn::call(|c| { ... }).await` onto `tokio::task::spawn_blocking`, and a real `query_timeout`
interrupts hanging queries via `InterruptHandle`. The closure is `Send + 'static` with owned input
and owned output. `DbConn::wrap(conn, query_timeout)` sits between `open_or_create_cell_db` and
`tokio::spawn(cell_task_stateful)`, synchronous and without `.await`, so the RespawnFn corridor
stays unviolated. `QueryTimeout` is a full `thiserror` error.

A renewed `add_nodes` at a path with an existing `cell.db` reopens that DB as a resume, with all
rows remaining. Schema migration is `CellFactory` responsibility at spawn: the factory checks
`schema_version` and migrates, or returns `Err` and the mutation is rejected. `swap_nodes` migrates
no `cell.db`. The wipe path is deferred: emptying a `cell.db` is an operator action outside the
mutation flow, and the mutation vocabulary has no verb for it.

`open_or_create_cell_db_with_status` returns `(Connection, OpenStatus)` with `OpenStatus::Created |
Resumed`. `Created` is the seed trigger for `store` (the factory calls `load_seed_if_present`
exclusively on `Created`, otherwise it would duplicate rows). `Resumed` means an existing `cell.db`
was reopened, with no re-initialisation. On an FS-IO error, DB corruption or a permissions problem
`open_or_create_cell_db` panics: the factory closure does `.expect(...)`, so an initial spawn is a
bootstrap or mutation error, and a restart runs the supervisor loop until `restart_limit` and then
marks the cell `failed`.

Canonical ordering at panic hooks and backstop cancellation:

1. `counter += 1`, or the per-call state update, synchronous.
2. `write_snapshot_with(...)`, synchronous, persisting the pre-panic or pre-cancel state.
3. The cancel or panic check, synchronous, before the async output.
4. The output emit, async.

Panic or cancel before output keeps an aborted cell from emitting, and the snapshot before it keeps
the restart overlay on the correct pre-abort state.

## Cell types

The overview table (type, task, actor kind, emission mode, phase) is canonical in [`cell-types.md` §
Overview](cell-types.md#overview), and so is the detail spec per built-in cell type. Since which
release a cell type has been live is in `CHANGELOG.md`.

## Edge model

- An edge connects one output to one input. One output can have several edges, which is fan-out and
  runs in parallel.
- `condition` (a CEL boolean) decides whether the edge is responsible. It reads exclusively the two
  header compartments of the source cell emission, via the namespaces `context.*` (persistent) and
  `hop.*` (exactly this hop, the isolated cell output composed with the edge modifier).
- `modifier` (an operations object with CEL expressions as values) is the sole header authority: it
  promotes and computes `context.*` and refines `hop.*` before forwarding.

  ```json
  "modifier": {
    "set_context":    { "<key>": "<CEL over context.* + hop.*>" },
    "delete_context": [ "<key>" ],
    "set_hop":        { "<key>": "<CEL over context.* + hop.*>" },
    "delete_hop":     [ "<key>" ],
    "restore_ttl":    true
  }
  ```

  - `set_context` and `set_hop` map keys to CEL expressions. Each expression has read access to both
    compartments (`context.*` and `hop.*`, read-only maps) and provides the new value. An existing
    key in the target compartment is overwritten, a missing one is created, so `set_*` covers
    setting and modifying, separately per compartment.
  - `set_context` is CEL-valued and covers promoting a hop value to context (`"set_context": {
    "turn_id": "hop.turn_id" }`) as well as computing (`"set_context": { "iter": "int(context.iter) +
    1" }`). Correction (GH #500): the earlier claim that the `int()` cast is necessary is retracted;
    the substrate now binds explicitly, the cast is an identity, and the spelling stays valid.
  - `delete_context` and `delete_hop` are lists of keys removed from the respective compartment.
  - `restore_ttl` (boolean, default `false`, GH #82, ruling 2026-08-13) is the one modifier field
    that does not touch a header compartment. When `true`, colony lifts the follow-up's `ttl` back
    to `message_default_ttl`. The restore never accumulates (the budget, not `ttl + budget`) and
    never lowers. Envelope-setter authority is untouched: the edge declares, colony writes. A
    restoring edge takes its own cycle out of the TTL guard, so one without a `condition` is
    rejected at config load and at `add_edges` validation; the intended shape is the iteration
    counter the same edge already carries in `set_context` (see `store-backed-tool-loop.md`).
  - All five fields are optional. A missing or empty modifier is identity: `context` passed through
    unchanged, `hop` forwarded unchanged, `ttl` decremented as everywhere.
  - Evaluation semantics: all `set_*` expressions read the incoming, pre-modifier state of both
    compartments as a fixed context. Order per compartment is `set_*` first, then `delete_*`, so a
    set value cannot be deleted again by the same modifier piece. Whoever wants that anyway writes
    two edges.
  - CEL is a pure expression language, so "set or delete a header" is not directly expressible in
    it. Rejected were a modifier as a CEL script returning a complete headers map and a patch map
    with a `null` sentinel for deletion.

  ```json
  "modifier": {
    "set_hop": {
      "msg_type": "hop.finish_reason == 'tool_calls' ? 'tool_call' : 'final_response'",
      "tier":     "hop.priority == 'high' ? 'gold' : 'standard'"
    },
    "delete_hop": ["internal_debug_marker"]
  }
  ```

  Response metadata (`finish_reason`, `priority`) lives in the `hop` compartment. `context` values
  (`session_id`, `turn_id`, `user_id`) pass through unchanged, because they are mentioned neither in
  `set_context` nor in `delete_context`.

- `default` (boolean, default `false`, GH #283, live since v0.18.0) makes the edge a default edge.
  Spelling: `"default": true` beside `from` and `to`; with the key absent the edge is an ordinary
  one. The rule in one sentence: a default edge **fires exactly when no regular out-edge of the same sender**
  fired for this message. It is therefore the declared consumer for what would otherwise
  dead-letter as `no_route` from this sender.

  It is a phase and not a group. Colony first evaluates every regular out-edge of the sender,
  unchanged, in insertion order, every match a fan-out branch, and only if that produced nothing at
  all does it evaluate the same sender's `default` edges, through the same evaluation, so with
  `condition`, `modifier`, `restore_ttl` and the F3 skip rule exactly as everywhere else.

  - A default edge may carry a `condition`, and that is the recommended shape: the phase decides
    when the edge is consulted at all, and **the guard decides which part of that traffic it consumes**.
    Whatever the guard excludes still dead-letters as `no_route`.
  - Several defaults of one sender all fire if their guards hold. The second phase is fan-out too,
    with no dispatch group and no ordering semantics among the defaults.
  - An unguarded default edge is legal and takes everything the regular edges left behind. It earns
    a hint and never a refusal: at boot a line in the bootstrap plan's advisories, at `add_edges` a
    `warn` log line. The boot starts, the mutation commits, and `--validate-strict` does not promote
    the hint.
  - An edge without the key is completely unchanged. An unconditional out-edge stays an always edge
    that fires in addition to every matching edge, with no ordering and no first-match among the
    regular edges.
  - A hive's `{"from": "."}` out-edge may be a default too. It then consumes the traffic that would
    otherwise dead-letter as `hive_no_route`.

- On fan-out (one output, N edges) colony copies the `context` identically into each of the N
  produced messages; the cell never touches `context`. Branch-specific content lives in the `hop`,
  written by the cell, or is set per branch via the respective edge modifier.
- Edges operate strictly on the header layer, with exactly one sanctioned exception:
  `modifier.restore_ttl` (GH #82). Body slots and the remaining envelope fields (`target`,
  `reply_to`, trace IDs) are outside the edge scope, and whoever needs a body transform or envelope
  logic builds a `code` cell. Content-aware routing goes via "the cell sets a header (`hop`), the
  edge conditions on it", which keeps edge evaluation free of blob resolution and lets body-slot
  schemas evolve without edges breaking. Rejected were a modifier that may modify the body or
  rewrite `target`/`reply_to`, and a condition that may read the body.
- Every edge has a UUID v7, assigned by colony on creation, visible in the read API and in the
  mutation log. In the mutation surface edges are usually referenced via a match pattern over their
  properties (`from`, `to`, `condition`, `modifier`, `default`); the UUID is a fallback for
  disambiguation.
- Edges live centrally in colony's edge table, indexed by `from` path for fast fan-out lookup. Cells
  do not know their edges; colony evaluates them after a cell emission.

### v-lanes, the declared deep edge (GH #559)

A v-lane is the ruled use of what `add_edges` has always accepted without a depth restriction: an
edge in the graph of the lowest common ancestor of both endpoints, whose `from` and `to` are
scope-relative and may point arbitrarily deep into that subtree. It adds no edge class and no
routing rule, and it replaces a pass-through chain of N hops with one edge from rim to rim. Delivery
is untouched, because routing was always flat. What was missing was the declaration.

Two fields carry it, and both are a matter of contract (ruling 2026-08-31). `lane` on the edge entry
(`params.graph.edges[]`, `add_edges[]`, optional) is the name of the lane this edge runs; with the
field absent the edge is an ordinary one. The lane is named rather than guessed, because a validator
cannot read it reliably out of a CEL guard. `at` on `accepts[]`/`emits[]` of the hive contract (a
list of relative paths, optional) says where the lane connects at this hive: `"at": ["./talky"]`
says this lane ends at my occupant `talky`. An entry is always a `./…` path strictly below the
declaring hive; a lane meant to end on a hive's own rim is declared one level up, as `./<hive>` in
the parent's contract, and `"."` names no connect point and matches nothing.

`ports: []` stays literally true. The v-lane is the one exception the target template pronounces
itself: a connect point is a promise made by the contract, never a right taken by the caller.

Levels in between are transparent by default and a mandatory hop by declaration. Whoever takes
influence on a lane, by stamping, filtering or guarding, declares it and may then not be skipped.
This is checked in mutation validation, where `hive_port_boundary` sits, for both endpoints of an
edge carrying `lane` and for every level between the lowest common ancestor and the endpoint:

| Crossed level between LCA and endpoint | What the contract says about the lane | Result |
|---|---|---|
| unsealed | nothing | transparent, skipped |
| unsealed | lane declared (accepts/emits), without a matching `at` | `v_lane_mandatory_hop`, the skip is refused |
| sealed (`params.ports` present) | `at` contains the relative path leading to the endpoint | allowed |
| sealed | nothing, or `at` without a hit | today's `hive_port_boundary` stands |
| the target hive itself | `at` does not name the endpoint for this lane | `v_lane_no_connect_point` |

An edge without the `lane` field keeps exactly today's behaviour: an undeclared deep edge stays
permitted exactly as far as it is today.

The rule table reads `accepts` and `emits` undirected: a level that only emits `recall` is no more
skippable by a v-lane carrying `recall` inwards, and a level that would like to carry a lane in one
direction only becomes a mandatory hop in both.

The mandatory hop does not protect inside the level that draws. The levels judged are strictly
between the lowest common ancestor and the endpoint, so an edge `<gen>/talky ->
<member>/memory-hive` drawn in the member's own graph passes every gate even where the member
declares `recall` as a mandatory hop, and delivers `missing_audience` at runtime for want of the
stamps the member's own door would have written.

A connect point closes the rim for its lane (GH #562). The `at` is two statements: the lane docks
there, and it therefore does not arrive at the hive path. Both halves are enforced. The hive owes
the lane no door out of its own path, because the lane-door check skips an entry that names connect
points, which lets a migrated level strike the pass-through edge it no longer needs. And an
`add_edges` entry that states the lane into the hive path is refused `hive_contract`, naming the
connect points it should have ended on instead, which protects the caller who was addressing the rim
before the migration. A lane with no `at` is judged exactly as before.

A v-lane may end on a hive the same mutation creates (GH #562, GH #567). Until the node is staged
its declaration lives only in the template, so validation reads it there, and
it reads the **whole staged subtree** with `ref` markers resolved, because for a composite the rim
that pronounces the connect point is typically an occupant behind a `ref` (`talky` inside an
`assistant`). Both doors that stage a subtree feed the same list: `add_nodes`, and a `swap_nodes`
successor. The list is appended to and never substituted, so a path that already stands keeps the
contract it was born with and a diff cannot talk a live hive into a connect point by naming a
template. That holds for both readers of the list (GH #573), the port boundary and the swap's
re-anchor verdict, and the successor comes from the same deduplicated list, including one an earlier
`add_nodes` entry of the same diff contributed. The birth contracts reach exactly one check, the
port boundary.

The refusal is asymmetric: it holds in the `accepts` direction, where an edge delivering the lane at
the hive path finds no door behind it, and not in the `emits` direction, where such a lane has no
sender at the rim and never fires.

The third code belongs to rebuilding: `v_lane_unanchored`. When a v-lane ends inside a subtree a
`swap_nodes` replaces, it is re-anchored by its relative form, since the new implementation has the
same sub-path. If that path is missing, or the new form's contract does not name it as a connect
point for this lane, the whole swap is refused by name. An affected edge is identified purely by the
subtree membership of its endpoints; there is no owner field and no second bookkeeping beside the
edge table. `move_nodes` re-addresses v-lanes by itself, because the edge table names paths, and the
known remaining gap is unchanged: path literals inside conditions and modifiers do not travel.
`required_drains` hold unchanged, because validation knows edges rather than chains. Drawing a
v-lane is an ordinary mutation: `colony.mutate` over the scope is the guard, and there is no
separate deep-edge capability.

What this changes about the union rule: a level declares the union of what its occupants accept and
emit, and a skipped level no longer declares the pass-through lane, so the rim of a skipped level no
longer describes all of its occupants' traffic. That is the only sanctioned exception to the union
rule, written down as one in the contributor rules and in ADR-0020.

What it does not change is secrets. A credential still travels as a sealed box over ordinary edges,
pulled per request against an ephemeral recipient key and never pushed (`cell-types.md` § `vault`,
sealed delivery). On a v-lane the same ciphertext rides one hop instead of N; the plaintext still
exists only in the RAM of the requesting task.

### Apps at the rim of a member (ruling 2026-09-05)

An app is a sub-form of the member: an ordinary sealed hive (`ports: []`) instantiated by an
ordinary mutation into the member's `./apps` container. It has no port, no secret and no channel of
its own. Everything it learns it learns over edges the member's topology draws, and everything it
says it says on lanes that member already carries. There are exactly three ways to plug in, all at
the rim: an app may observe what the conversation carries, offer a tool to the member's assistant,
and write to a screen. It may not stand in the way, because an app is never an interception. Every
edge it gets is an additional one, so a path that existed without the app fires exactly as it did
before, which makes an app installable and removable without re-reading the member.

Three classes of edge, two owners. The ruling in one sentence: whoever listens orders it.

| Class | Owner | Example |
|---|---|---|
| the level's half, in the `member` template | the member | the declaration of the observer lanes (`at: ["./apps"]`), and the two restamping edges `./apps -> ./assistants` (`tool_result` to `in_tool`, `tool_schemas` to `in_menu`) that never fire while no app is installed |
| observer and binding edges, in the installing manifest, scope `<member>` | the mutation that instantiates the app | `./firewall -> ./apps` on `turn`, `./apps -> ./apps/<app>` on `turn`/`answer`/`partial`, `./apps/<app> -> ./apps` on `view`/`tool_schemas` |
| v-lanes, same manifest, same scope | the same mutation, permitted by the `at` of the app and of the assistant | `<gen>/talky -> <app>/show` on lane `tool` |

Observing: the template declares, the manifest draws. The member declares `turn` and `partial` as
`emits` with `at: ["./apps"]`, and `tool_result` and `tool_schemas` as `accepts` with the same
connect point; `answer` keeps its rim entry without `at`, because it is a rim lane and an `at` would
switch off the exit check that guards it. The edges that carry those lanes are not in the template:
the mutation that installs a listening app draws `./firewall -> ./apps` (the screened turn, with the
same hygiene as `./firewall -> ./assistants`), `./assistants -> ./apps` (the answer), `./channels ->
./apps` (the interim transcript) and the container binding `./apps -> ./apps/<app>`. A member with
no listening app carries no such edge and dead-letters nothing.

None of those edges needs the declaration mechanically. The declaration makes the member a mandatory
hop for a v-lane carrying one of those lanes in from outside, and keeps the rim a truthful
description of the traffic.

The observer edges for `turn` and `answer` are guarded on the channel (`has(context.channel_node) &&
context.channel_node != ''`), and the guard is load-bearing: a turn injected at the member's
`in_turn` door by an operator or a digest leaves through the member's guarded default exit, and a
regular fan-out edge beside it would kill that exit outright. Event and receipt turns raised by a
screen do not pass the firewall and are not observed as `turn`.

A second installation is idempotent by edge identity: an edge is the same edge when `from`, `to`,
`condition`, `modifier` and `default` are, and the edge table keeps identical edges once. A
diverging guard is a different edge, and then the turn arrives at the container twice, which is why
the installer of the future is the builder, reading `/colony/graph` and drawing only what is
missing.

Offering: the call leaves the brain's rim as a v-lane. The `tool` lane leaves `talky` at its own
rim, and the app gets the same shape one level lower down: an edge in the member's graph,
`<gen>/talky -> <app>/show` with `lane: "tool"`, guarded on the tool name, regular and therefore
evaluated before the assistant's default exit into `./tools`. Because it bypasses that exit it
carries the exit's stamps itself (`context.tool_caller`, `context.assistant`) and deletes the inside
markers the exit would have deleted. `schemas`, the menu question, runs the identical edge guarded
on its own route, as a third regular edge beside the two the assistant already fans the question out
over. Both ends pronounce their connect points: the assistant declares `tool` and `schemas` with
`at: ["./talky", "./cogny"]`, the app declares them with `at: ["./show"]`.

The way back is not a v-lane; it is the way the memory already takes. The app emits `tool_result`
and `tool_schemas` at its own rim, the manifest's binding edge carries both into the container and
stamps `context.tool_answerer` there (the level cannot stamp it, because only the mutation knows the
instance name), and the member's two restamping edges turn them into `in_tool` and `in_menu` for
`./assistants`. Nothing is added to the assistant's own rim, because an `at` on `in_tool` would
close the rim door every growth recipe draws.

An app answers its whole offer, whatever list was asked for: it replies with everything it offers
and an empty `unknown`, and the collector merges the rows of all answerers into one menu, where a
name an answerer did deliver wins. The app's tool thereby reaches the brain's `system.tools` without
the collector, its `params.tools` or the growth recipe being touched.

Observed tool results arrive as a v-lane fan-out, from two sources: one from the assistant's
`./tools`, one from the member's `./memory-hive`, both onto the app's `./stage`, both regular edges
that fire in addition to the existing exits. For the first the assistant declares `tool_result` with
`at: ["./tools"]`, which by the union rule's exception is not a rim lane. The second source is a
direct child of the scope and is not deep at all. The manifest deliberately draws no container edge
`./memory-hive -> ./apps`: the observed copy would run over `./apps -> ./assistants` and reach the
assistant a second time.

Writing is unchanged. The app emits `view` and `error` at its rim, the binding edge stamps the
screen it writes to, and the member's guarded `./apps -> ./channels` edge delivers it as `in_view`.

A required lane lets a hive insist on being wired at birth. An `accepts` entry may carry `required:
true`, meaning whoever instantiates me must wire this lane to me. It is checked in the post-state
stage of the mutation, against the post-state edge table, for every hive this diff gives birth to,
the same list and the same reasoning as the port boundary. A lane with `at` counts as wired when an
edge carrying that lane ends on one of its connect points; a rim lane counts as wired when the
router probe of an inbound edge lands on the hive path. Missing, the mutation is refused
`hive_contract` (its third shape), collecting rather than at the first find, before the commit, and
rolled back like its neighbours.

Its limits: only birth is judged. A later `remove_edges` that takes the lane's edge away is not
re-judged, boot does not even warn, and whether the emitter at the other end really delivers the
lane is nothing the substrate can know.

What is not here: the app loader is a later errand. Today an app is an ordinary template plus a
manifest written by hand, with no `app.json` read at start, no colony-level app listing and no `app
install` command; the builder as installer belongs to the same errand. The install manifest stands
with its placeholders in `templates/member/README.md` § Installing an app.

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
    Blob(Uuid),                   // ≥ threshold, in blobs/<uuid>.<ext> + blobs/<uuid>.<ext>.meta.json
}
```

TTL is a protective limit against uncontrolled routing loops. Colony decrements on every routing
decision, so once per cell-to-cell hop. At `ttl == 0` the message goes directly into the dead-letter
queue as `ttl_expired`, never via the routing-error cascade with its step-1 `reply_to` attempt,
because an expired TTL is terminal. The default lives in `colony.json` as `message_default_ttl`,
recommendation 64. Builders can set the value per initial message (the `ttl` field in `POST
/messages`, positive integers only, otherwise `422 invalid_ttl`). The hierarchy is: an explicit
`ttl` field of the initial message beats `colony.json` `message_default_ttl`, which beats the const
seed `MESSAGE_DEFAULT_TTL` of 64. Cells never set `ttl`. Do not confuse it with
`message_timeout_default_ms`, which addresses the maximum processing time inside a cell.

Sizing (GH #82): the recommendation of 64 is for flat topologies. The store-backed tool loop costs
about 12 hops per tool round (measured: six rounds are 76 hops), so on 64 an agent stops after five
rounds; the rule of thumb for that shape is `message_default_ttl >= 4 + rounds * 12` (hop table and
derivation in `store-backed-tool-loop.md`). Because an expired TTL is terminal and skips the
`reply_to` cascade, a death inside a fan-in is observable only as a dead-letter row plus an `ERROR`
log line naming the message. A loop is therefore bounded by an iteration counter in `context` on the
loopback edge, while TTL stays the substrate guard. `colony.json` `ttl_notice: true` turns on the
terminal notice (GH #119, ruling 2026-08-14): a TTL death with a `reply_to` then sends a substrate
error reply (`error_code: "ttl_expired"`) to that anchor, so a waiting fan-in can close instead of
parking. The notice is terminal, never cascades, and carries a fresh budget, which is why the switch
is opt-in.

A shape whose round is itself made of routing wants its budget back per round, which is what
`"modifier": { "restore_ttl": true }` declares (§ Edge model). No message ever rises above the
larger of its ingress budget and the colony default. The substrate rejects a restoring edge without
a `condition` at config load (`BootstrapError::EdgeTtlRestoreUnconditional`) and at `add_edges`
validation, because the runaway guard for such a loop is its iteration bound. The default
`message_default_ttl` stays 64.

### Envelope setter authority

Envelope fields (`id`, `trace_id`, `parent_message_id`, `correlation_id`, `reply_to`, `ttl`,
`target`, `created_at`) are set exclusively by colony during routing. Cells cannot write them: the
content JSON a cell emits has no mechanism for envelope fields, and edge modifiers operate on
headers, with the single sanctioned exception of `modifier.restore_ttl` (GH #82), a declaration that
colony evaluates and applies.

| Field | Who sets | When |
|---|---|---|
| `id` | Colony | on every new message (UUID v7) |
| `trace_id` | Colony | newly on a source message, otherwise copied from the parent |
| `parent_message_id` | Colony | taken over from the consumed incoming message, `None` on source messages |
| `correlation_id` | no originating producer today; a reserved envelope field for future req/resp pairing. Correlation currently runs via the context header convention (`turn_id`), not via `correlation_id` | none; the field is reserved, so the `?correlation_id=` filter on `/colony/trace` is inert today |
| `target` | the trigger layer (a cell output determined by edges, the HTTP API by the endpoint) | on routing |
| `reply_to` | Colony, automatically to the absolute path of the sender | on every routing decision |
| `ttl` | Colony, newly stamped on source messages from `colony.json` `message_default_ttl` (seed: const `MESSAGE_DEFAULT_TTL`, 64); the HTTP ingress takes an explicit `ttl` request field per initial message as an override; decremented per hop | newly on a source message, decremented afterward |
| `created_at` | Colony | on message creation |

For messages fed in via the HTTP API (`POST /messages`), colony sets `reply_to` to a virtual API
request path or leaves it `None`; the HTTP response is returned via the request channel rather than
via routing. Cells that want a reply target other than their own path solve this
application-specifically via header-based routing, for instance a header `reply_target` set by the
original sender plus an edge that conditions on it.

The JSON ingress sets no `reply_to` today and leaves it `None`. A cell emission thereby triggered
that subsequently matches no out-edge passes through the following chain (rulings A1 and W2d).

1. The built-in cell types set the target of their op and error replies to the inbound `reply_to`;
   at `None` they fall back to their own `msg.target`, the path to which the cell was addressed. The
   upstream hardcoded fallback `unwrap_or("/colony/dead_letters")` at the atomic-emitting cell types
   is removed.
2. In the outputs arm the emission matches no out-edge and there is no identity decision. It
   dead-letters as `no_route` (`DeadLetterReason::NoRoute`), self-localizing with sender, resolved
   target, `trace_id` and `created_at`. Retraction (GH #283, v0.18.0): this used to read "today a
   real default is spelled as the negation of every other arm; the construct that would replace that
   spelling is tracked in GH #283", and that is withdrawn. Since v0.18.0 an out-edge carrying
   `"default": true` catches exactly this case (§ Edge model). Terminal, no re-inject, no loop.
3. If the cell emits explicitly to a `/colony/*` target, `/colony/mutations` is executed while
   `/colony/dead_letters` and the other read endpoints are hard-rejected or read respectively.

A `no_route` DLQ signature after a `reply_to`-less ingress probe is therefore the expected behaviour
of this chain, not a routing bug.

### Headers and body, the write model

Headers live in two structurally separate compartments with different lifetime and write authority.
`context` is persistent and travels over the entire message lifecycle; the sole write and delete
authority is the edge, and it carries correlation and long-lived content (`turn_id`, `session_id`,
`iter`). `hop` lasts exactly one hop: it is the isolated contract output of the immediately
preceding cell, refined by the traversed edge modifier, and it is completely replaced at the next
cell emission. It carries the cell product, routing control and response metadata (`operation`,
`finish_reason`, `msg_type`, `route`, `agent_target`, `rows_affected`, `error_code`).

- Cells write content as JSON with an optional `"header"` section. Colony interprets that section as
  `hop`, the isolated cell output. Cells never write `context`, and cells do not inherit the `hop`
  of their predecessor.
- Cells read everything read-only: `context`, `hop`, body slots and the envelope fields. They write
  exclusively their isolated output, which becomes the new `hop`. The read declaration lives in
  `contract.consumes.context.<key>`, `contract.consumes.hop.<key>` and
  `contract.consumes.body.<key>` (see `config.md`).
- Delete authority is an edge matter: cells have no delete mechanism. An edge removes values via
  `delete_hop` or `delete_context`.
- Edges (conditions and modifiers) are the sole header authority. Conditions read `context.*` plus
  `hop.*` read-only as a CEL boolean; the modifier writes `context` (`set_context`,
  `delete_context`) and refines the `hop` (`set_hop`, `delete_hop`). Body slots and envelope fields
  are outside the edge scope.
- By default the `hop` expires. A value lives exactly one hop, unless an edge promotes it via
  `set_context` to `context`, and the failure mode is loud: forget to promote and the value
  disappears at the next cell emission.
- Conflict rule: within a compartment, replace (last write wins); the two compartments do not
  overlap. The audit trail lives in the central message log via the `parent_message_id` chain, never
  in the headers.
- Composition along the routing path (R3 ruling, K-H7): if a message passes through several edges in
  sequence, same-key modifiers compose left to right along the path, each edge applying its
  `set_context`/`set_hop` to the header state already transformed by the preceding edge. If several
  edges set the same key, the later, consumer-near, inner edge wins. Every transit hop writes its
  own `message_log` transit row, so the composed end state stays traceable via the
  `parent_message_id` chain.

### Standard header convention

meclaw-core does not know these keys semantically; they are conventions for applications.

| Key | Meaning |
|---|---|
| `turn_id` | One user interaction, for instance one chat turn from a proxy cell |
| `session_id` | One logical session bracket |
| `user_id` / `chat_id` | external identifiers from a proxy platform |
| `locale` | language or locale of the request |

`chat_id` is typed per platform: numeric on Telegram, a string on Slack (composite form
`"<channel>"` or `"<channel>:<thread_ts>"`, see `cell-types.md` § `proxy`). meclaw-core does not
validate the type.

These standard keys live in the `context` compartment. `context` is written exclusively by edges and
by the ingress at birth, never by cells, so there are exactly two entry paths by which a key
initially reaches `context`.

1. Source cells (`proxy`, `timer`) emit their values as `hop`, and their out-edge promotes via
   `modifier.set_context` to `context`, in-graph and visible. The source and proxy template pattern
   bakes this promotion in as a real edge.
2. The HTTP ingress is the birth point of the message and establishes the initial `context`
   directly. The keys it lifts to `context` are declared (`turn_id`, `session_id`, `user_id`,
   `chat_id`, `locale`), so that they are auditable and the mutation validator treats the ingress as
   a reachability root for `consumes.context`.

The ingress exception is strictly limited to the birth point and is not a loophole for cells.
Further header conventions are freely choosable (`msg_type`, `priority`, `tool_call_id`); whether a
key is persistent or hop-local follows purely from the compartment. meclaw-core does not distinguish
between standard and application headers.

### Edge expression language

CEL (Common Expression Language) via the `cel` crate (GitHub project `cel-rust`; the crate name on
crates.io is `cel`). An independent Google standard, safe because it is not Turing-complete, and
expressive enough for all known routing patterns.

Feature scope, empirically verified against `evaluate_condition` (cel 0.13.0, 2026-06-11): the CEL
standard macro `has()` is available in edge conditions, and `has(hop.priority)` evaluates to `false`
on a missing key with no eval error. The equivalent key-existence check is the `in` operator
(`'priority' in hop`). Beyond that the substrate tests demonstrate comparisons, ternaries, string
methods (`contains`) and numeric casts (`int()`, `uint()`).

Numbers in conditions and modifiers (GH #500): a number out of a header compartment binds as the CEL
type the CEL spec gives a JSON number, so `int` while it fits `i64`, `uint` only above `i64::MAX`,
and `double` otherwise. The form everybody writes first therefore holds: `hop.http_status == 200` is
true on a message carrying 200.

Two consequences for topologies written before the correction. The range form (`>= 200 && < 300`)
and the `int()` cast stay valid unchanged, since the cast is now an identity. A `u`-suffixed literal
(`== 200u`) no longer matches an int-bound number, so whoever wants unsigned semantics casts the
compartment instead: `uint(hop.k) == 200u`. No shipped template and no example spells such a
literal, and a sweep keeps it that way (`gh500_a_numeric_condition_fires.rs`).

A missing key and an eval error are two classes with one routing (GH #80). An unguarded `hop.k ==
'...'` is an eval error when `k` is absent, and the routing behaviour for it is unchanged spec F3:
skip the edge. The two classes differ in log level.

- A missing key (`cel::ExecutionError::NoSuchKey`) is `debug`. In a fan-out this is the steady
  state: every message without the key produces one miss per non-matching lane, and as a warning it
  would scale with lanes times messages.
- A genuine eval error (a type mismatch, incomparable values, a reference to an unbound variable
  such as `hopp.k`, a non-boolean result) is `warn`.

Honest limit: CEL cannot tell a legitimately absent key from a mistyped one, so `hop.toolname` and
an absent `hop.tool_name` are the same event. Topologies therefore guard optional keys with
`has(hop.k) && hop.k == '...'`, a form that produces no line at all. Every shipped `examples/` and
`templates/` topology runs the guarded form (sweep test `gh80_shipped_conditions_are_guarded`).

### Atomicity

Messages are atomic, with two fixed header slots (`context` and `hop`), one value per name per
compartment, no growing hop list, no versioning and no annotation history in the envelope. Real
history (accumulated tokens, an iteration trail, expected tool-call IDs) belongs in a `store` cell
or an aggregator. Trace reconstruction runs via the `parent_message_id` chain in the central message
log in `colony.db`.

Headers are unbounded by design. No size limit applies to the two compartments, not at the HTTP
ingress, not on a cell emission, and not when the envelope is persisted into the message log. That
is contract: an edge modifier writing a large `context` value is a legitimate topology, a cap would
silently break it, and a cap would be a breaking change and is not planned. What replaces it is
observation: header sizes are watched by a standing measurement whose last reading and query live in
[#141](https://github.com/mmeyerlein/meclaw/issues/141).

## Body format (universal)

All cells emit and consume the same body, so no cell needs a format adapter between inputs of
different source types. `proxy` user turns, `bash` stdout, `web_fetch` response bodies, `file` read
content, `code` script output and `llm` inference results all arrive in the same structure.

### Top-level slots

Three central slots, `system`, `messages` and `attachments`, of which at least one must be set (a
schema `anyOf` over exactly these three `required` branches). A pure file upload, only `attachments`
without `system` or `messages`, is a legitimate message form.

| Slot | Meaning |
|---|---|
| `system` | Optional system context: identity (persona), tools schema, bootstrap instructions, facts, session state. Nested via sub-slots like `system.identity.soul.text`. The full slot path is required; abbreviated notation is forbidden. |
| `messages[]` | Chronological list of conversation turns. |
| `attachments[]` | List of blob-referenced file attachments. Actively consumed from phase 12, the slot is name-reserved from phase 3. As an `anyOf` branch it is already a valid top-level shape today. |

Two slot names are reserved from phase 3. `attachments[]` becomes active in phase 12 and holds
blob-referenced file attachments (PDF, TXT, images, audio) with typed metadata; no cell may use the
name otherwise. `header` is **never a body slot**: a `header` object at the top level of a cell's
emitted content is lifted out and merged into the envelope's headers (input headers first, the
emitted `header` overlays, last write wins, `split_content_header` in
[`crates/meclaw-colony/src/colony.rs`](../crates/meclaw-colony/src/colony.rs)), so a cell that puts
payload under `header` loses it from the body.

Cells can create their own top-level slots (`meta`, `delta`, `event`, `graph`) as long as they do
not collide with reserved slot names. Consumers ignore unknown top-level slots and unknown `system`
sub-slots.

Schema validation runs at points of different sharpness: always-on at the edge, and a debug net for
interior correctness, with no trusted-exemption carve-out.

- Edge (the trust boundary), always-on, even in release: every source body fed in via the HTTP API,
  the JSON as well as the multipart path, is validated against the schema before routing, and a
  violation is rejected with `422` (`invalid_ubf_body`). The attachments-only shape synthesised at
  multipart upload is a valid `anyOf` branch, so a client-reachable `422` arises only on the JSON
  path.
- `code` output, always-on with no opt-out: the `code` cell is the only user-script-driven output
  source and therefore itself a trust boundary, so its `contract.emits` validation runs
  unconditionally (`validate_emits = true`, independent of the build profile and of `colony.json`
  `strict_validation`).
- Built-in cell output, a debug net: the body structure validation of substrate-cell emissions runs
  under `#[cfg(debug_assertions)]`, so dev and test builds DLQ violations as `invalid_ubf_body`
  while release builds have zero overhead. In the outputs arm the central `contract.emits`
  validation of the non-`code` types runs flag-gated (`resolve_validate_emits`): a violation gives
  an error reply to the `input_reply_to` (`error_code: "contract_violation"`), otherwise the DLQ
  under the same token. `code` stays exempt, being always-on in-cell.

The schema lies as a static document in `meclaw-core` (via `include_str!`, compiled once into a
validator) and knows `attachments`-only as a valid shape. The `attachments[]` slot is anchored with
its correct phase-12 schema even though no phase-3 cell fills it, so the name reservation is
syntactically enforced without resolver code.

### `messages[]` schema

Each entry is either a turn object, a turn pointer or a bulk pointer:

```json
// turn object, inline
{ "origin":      "user|assistant|tool|system",
  "type":        "text|tool_call|tool_result|image|audio",
  "text":        "<inline-string>",
  "id":          "<required for tool_call/tool_result>",
  "happened_at": "<optional: event time of this turn>" }

// turn pointer, a single turn content in the blob
{ "text_id": "<UUIDv7>" }

// bulk pointer, reference to a body document in the blob; its messages[] is expanded inline
{ "messages_id": "<UUIDv7>" }
```

- `origin` (required, enum): who spoke the turn.
- `type` (required): determines the semantic format. `image` and `audio` are reserved for
  multi-modal and are a label, never a container: the payload of a multi-modal turn always travels
  through `attachments[]` as a blob reference. That is frozen: there will be no inline `data:` field
  and no base64 in `text`.
- `text` (inline) or `text_id` (pointer), exclusive per slot.
- `id` is required on `tool_call` and `tool_result` and is the correlation anchor for the collector
  aggregation. Values are pass-through from the provider (`tool_call_id`).
- `happened_at` (optional, string) is the event time of this turn, as opposed to the moment a
  consumer received it. Consumers that stamp their own clock ignore the field, and a turn without it
  is valid unchanged.
- The turn object is closed (`additionalProperties: false` in
  `crates/meclaw-core/schemas/ubf-body.json` § `$defs.TurnObject`): exactly `origin`, `type`,
  `text`, `id` and `happened_at` are allowed. An additional field, for instance a tool name next to
  `type: "tool_call"`, makes the entire body `invalid_ubf_body`. Structural extra information
  belongs in the `header` slot.
- Why `happened_at` is the exception (GH #135): the `header` carries one time per message, and a
  batch of replayed turns carries one per turn. The opening is additive and named rather than
  `additionalProperties: true`, so every other extra field remains `invalid_ubf_body`.

### `attachments[]` schema

A list of typed file attachments that lie as blobs in the `blobs/` directory. Each entry is an
object with:

```json
{
  "blob_id":    "<UUIDv7>",
  "mime_type":  "application/pdf",
  "filename":   "report.pdf",
  "size_bytes": 124573,
  "sha256":     "abc..."   // optional; omitted in phase 12 (no consumer)
}
```

- `blob_id` (required): UUID v7, referencing the two blob files `blobs/<blob_id>.<ext>` and
  `blobs/<blob_id>.<ext>.meta.json`.
- `mime_type` (required): MIME type of the content. Authoritative in the sidecar, duplicated here
  for a fast read without a sidecar fetch.
- `filename` (optional): the original filename on upload via the HTTP API, `null` for
  system-generated attachments.
- `size_bytes` (required) and `sha256` (optional) are duplicated from the sidecar for the same
  reason. Schema-drift note (D-027): `ubf-body.json` lists `sha256` as required in `attachments[]`,
  stricter than this spec; it is latent until attachments go active in phase 12, and aligning the
  schema to optional is pending at that slice.

The owner of `attachments[]` resolution is the consuming cell (ruling GH #19). The substrate
resolves only pointers whose target is a body document (`messages_id`/`text_id`); an attachment is a
file of arbitrary size, and inlining a 40 MB PDF into the JSON body would defeat the blob store it
came from. The `blob_id` ref reaches the cell unchanged, and the cell reads the blob on demand at
`handle()` time. A cell whose contract declares `consumes.body.attachments` receives a read-only
handle on the colony's blob store at spawn (`AttachmentReader`, GH #87); without the declaration
there is no handle. Every read carries its own operation timeout, and a missing blob or a
non-consumable MIME type are cell errors rather than dead letters.

The switch is the declaration: `required` (default `true`) governs the ingress check, and a
`required: false` takes the key out of it entirely (neither presence nor type is checked) yet
declares the slot just as much and grants the handle (`config.md` § contract, GH #323). Cells that
do not declare it ignore the slot.

Attachments are separate from `messages[]`: a conversation stays purely textual and attachments hang
as a parallel list off the body, so PDF attachments do not collide with the `messages[]` turn
semantics and LLM provider adapters can build them into their API call cell-type-specifically.

### `system` sub-slot structure

`system` is an application-defined tree. Leaves are `{text}` or `{text_id}` containers, analogous to
turn objects and pointers.

```json
"system": {
  "identity": {
    "soul": { "text": "You are a..." },
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

A `{text_id}` leaf is resolved by the substrate (`resolve_blob_for_delivery`, GH #86), at the same
delivery boundary and under the same guards as the in-message pointers. The target document has the
same shape a `messages[]` `text_id` names: exactly one turn, whose `text` string fills the leaf,
which becomes `{"text": …}`. `system.tools.*` is no exception: the exemption covers concatenation,
and resolution reaches it. Depth counts pointer expansions rather than tree levels.

Tool definitions are a special case: their `text` values are JSON strings with the tool definition
(name, description, JSON-schema parameters). meclaw-core does not know this format; the LLM provider
adapter parses it.

At inference the LLM adapter builds a single string from the tree, joined with `\n\n` between
sub-slots. Order: first the sub-slots listed in `params.system_order`, in the order given there,
then all the rest alphabetically, with an alphabetical DFS walk within sub-trees. `system.tools.*`
is exempt from this concatenation, because tools are pulled out separately as a provider-native tool
set.

### Replace semantics

Each slot is set atomically rather than additively. Updating `system.X.Y.Z` replaces that path and
leaves other paths unchanged (accumulative replace per path); updating `messages[]` replaces the
entire array; whoever wants to append a turn sends the full desired list.

Revocation runs through the `$replace` marker (GH #264). A path that is not sent is not touched, so
a writer with fixed paths revokes by sending the slot with an empty rendering, and a writer whose
sub-keys are data cannot. For that, a node of the incoming `system` subtree carries the reserved key
`"$replace": true`, meaning that below this node exactly what this message brings holds. The root is
the node itself, never a path named elsewhere; everything at and below it is deleted in the same
transaction, before this message's leaves land. A marker with no leaves under it is the pure
revocation.

- Opt-in, always. A write without the marker replaces nothing, because `system.*` is accumulated by
  several independent writers under different paths (an identity pack, a recall bundle, a consult
  list).
- Segment boundary. A replace at `memory.recall` reaches `memory.recall` and `memory.recall.*`,
  never `memory.recallx`, the same rule by which `system_writable` matches its prefixes.
- A marker directly under `system` has the empty root and means the whole tree. Nested markers are
  the union of their roots.
- `$` is reserved. `"$replace"` takes only `true` or `false` (`false` is an explicit no-op); any
  other `$` key and any non-boolean is a shape error (`error_code: "invalid_input"`, nothing
  written, nothing deleted).
- Two writers over one root: the last one wins. A cell knows no topology and the write gate does not
  gate on the sender, so there is no identity with which to arbitrate. The protection is the scoping
  plus `system_writable`, which since GH #264 checks the replace root too and not only the leaves.

History management is thereby an application matter, for instance a dedicated memory-hive topology,
and not a core feature. An `llm` cell fed directly by a proxy sees exactly what the proxy sends,
typically a single turn, and answers exactly that.

### Blob references are universal

Every blob is itself a complete body document. Both in-message pointers are resolved by the
substrate at the delivery boundary (`resolve_blob_for_delivery`, GH #19), in the same place and at
the same moment as the whole-body `Body::Blob`, and necessarily before the `consumes` check. A cell
never sees a pointer.

- `messages_id` references a body document, and its `messages[]` is spliced inline in the pointer's
  place.
- `text_id` references one single turn: the referenced document must hold exactly one entry in its
  `messages[]`, and that entry replaces the pointer. It is the singular of `messages_id`, and the
  only reading that yields a schema-valid result, because the turn object is closed and requires
  `origin` and `type`. A document with zero or several entries is a shape error
  (`blob_unavailable`), never a silent truncation.

The `{text_id}` leaves in the `system` tree go the same way (GH #86): the same target document, the
same boundary, the same guards, the same error codes, and only the substitution differs. Per
delivery the `system` tree and `messages[]` are resolved against one working copy that is committed
only if both passes succeed: a body whose `messages[]` would resolve and whose `system` tree would
not is dead-lettered as it arrived, never delivered half-expanded.

The cache is pass-local: one resolution pass holds `blob_uuid → parsed body` for the duration of one
message, so two pointers to the same blob cost one read. A cache spanning a cell's lifetime, which
an earlier version of this paragraph claimed, is not built and is a roadmap item of its own. Cache
invalidation does not exist in either case, because blob UUIDs are immutable.

One other pointer class is explicitly not here: the `blob_id` refs in `attachments[]`, whose owner
is the consuming cell. They travel through the delivery boundary unchanged.

Recursion is allowed but hard-limited: a blob's `messages[]` can itself contain pointers. Two guards
run before the next read. First the hard depth limit, default 64, overridable via `colony.json`
`blob_max_recursion_depth`, where `0` expands no pointer at all. Second a visited set over the
current path, which catches mutual cycles (A to B to A) immediately; it applies per path rather than
globally, because the same blob on two sibling branches is a legitimate diamond.

Both report the same `error_code: "blob_recursion_too_deep"`; whether it was depth or a cycle is
named in the log line. It is handled via the routing cascade, back to `reply_to` or to
`/colony/dead_letters`, and failures from non-findable or misshaped blobs go the same way under
`blob_unavailable`. A failed resolution returns the body unchanged.

### Emission modes and what a cell persists

Cells fall into two classes, depending on how they handle the incoming `messages[]`, plus a special
case.

| Emission mode | Behaviour | Examples |
|---|---|---|
| Stream-propagating | The incoming `messages[]` is passed through plus its own contribution, typically one turn, appended. The conversation thread stays along the chain. | guardrail or transform hive (modify passing turns), aggregator hive |
| Atomic-emitting | The cell emits a fresh `messages[]` with only its own contribution, no pass-through. Typical for sources, tool endpoints and LLM inference. | `llm` (the assistant turn alone); `bash`/`web_fetch`/`web_search`/`file`/`edit` (a tool_result turn); `proxy` (a user turn from an external chat); `timer` (a schedule-configured body); `store`/`mcp` (an atomic query or tool response) |
| Script-determined (special case) | The emission mode arises per execution from what the script writes, and can be atomic or stream-propagating. The only cell type in this class. | `code` (a programmable body constructor) |

Which emission mode a cell belongs to is part of its job description and is stated in
`cell-types.md`. The decision follows from the cell type rather than from the topology.

Stream chains are therefore effectively append-only with respect to `messages[]`. Atomic-emitting
cells break the chain by definition, and whoever wants to bring the conversation context back to the
LLM builds a collector topology that aggregates tool-result messages and joins them with the
conversation thread. That is an application pattern rather than core.

Cells that declare `contract.multi_send_capable: true` may emit several output messages per input.
The wire format is cell-type-specific; for `code` see `cell-types.md`. Every emitted message runs
independently through the outgoing edges of the cell, and routing can diverge per message.

What the cell emits and what it persists are two different things:

| | In the output message? | In `cell.db`? |
|---|---|---|
| Incoming `messages[]` (with blob refs unresolved) | yes, passed through | yes, last-received as-is |
| Own new turn (an assistant turn from an LLM call) | yes, appended | **no**, the output is not persisted back into the cell state |
| `system.*` | **no**, private cell state | yes, accumulative-replace per path |
| `meta` slot (cell-specific metadata) | yes | no, each call sets its own |
| Blob cache | no (in-memory) | no (re-fetchable on restart) |

The cell state therefore does not drift on its own: what the cell holds came from outside, and it
does not write itself its own truth about the conversation.

Response metadata (`tokens_prompt`, `tokens_completion`, `model`) lives in the `hop` compartment and
expires at the next cell emission, so an `llm` cell in a tool loop produces a fresh `hop` on every
call. Whoever wants totals builds an aggregator hive, correlated via a `context` convention such as
`turn_id`. Aggregation is application topology, never a cell-type responsibility.

## Iteration is topology

llm cells have no inner loop. They make a provider call, give the response out as one message, and
are done. Any iteration (a tool loop, ReAct, plan-and-execute) arises through graph topology and is
application logic rather than meclaw-core.

meclaw brings no prefabricated tool-loop topologies, dispatcher hive or collector hive.

An example topology for a tool loop, for illustration and not as a prescription:

```
[llm] ──► [dispatcher (code-cell)] ──► fan-out via edge conditions
                                  ├─► [proxy]            (intermediate user message)
                                  ├─► [tool-A]           ┐
                                  ├─► [tool-B]           ├─► [collector (code-cell)] ─► [llm]
                                  ├─► [tool-C]           ┘    (same turn_id, next iteration)
                                  └─► [collector]        (expect-Notification)
```

The dispatcher hive decomposes the LLM output into several typed messages (tool calls, intermediate
responses, expect notifications). The collector hive collects tool answers and sends them aggregated
back to the LLM. Both usually live under a `hive` scope marker that groups the tool-loop
sub-topology as a unit.

The loop counter (`iter`) lives as `context` and is incremented on the loopback edge via
`modifier.set_context: { "iter": "int(context.iter) + 1" }`; the `int()` cast was necessary until GH #500
and is an identity since. Cells never touch `iter`, because incrementing is edge authority. In a
store-backed loop the same loopback edge also carries `modifier.restore_ttl` (GH #82), coupled to
the iteration condition.

The collector correlation matches over tool-call IDs, as a set difference where the expected IDs are
a subset of the received IDs, rather than over counting, so that it is idempotent as well as
out-of-order- and duplicate-proof. The expected ID set lives in the `store`; the header carries per
tool result only its `tool_call_id`, hop-local.

Routing conditions on the edges use regular CEL expressions on `context.*` and `hop.*` keys, for
instance `hop.msg_type == "tool_call"`. Such header conventions are application conventions, and
meclaw-core does not know them as a special case.

For a complete store-backed implementation, follow the [examples/telegram-research protocol
walkthrough](store-backed-tool-loop.md), which traces a two-tool round through the dispatcher,
store, collector and loopback edge.

## Template system

Templates are cells, or whole subtrees including hive scope markers, under `templates/`. Their role
is class or blueprint. The directory structure within `templates/` is freely choosable; the scanner
finds templates by the `template.json` file. Identification is by name, because template-internal
graphs need stable name references and UUIDs are assigned only after instantiation. The name is
unique across `templates/`: if two `template.json`s declare the same `name`,
the scan aborts with an error (`ScannerError::DuplicateName`), regardless of depth and `version` (GH #277).
Versioning is optional, with a directory name `<name>@<version>` or simply `<name>/`, which counts
as unversioned.

### The `template.json` index

Every template has a `template.json` in its root directory that describes the template as a class,
separate from `config.json`, which describes the cell to be instantiated.

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

`template.json` describes exclusively the template itself, metadata for discovery, and says nothing
about the internal cell-type structure. Its `description` has exactly four slots (`purpose`,
`use_when`, `not_in_scope`, `examples`); the six-slot form, additionally `emits_meaning` and
`consumes_meaning`, applies to cell `config` descriptions (`config.md` § `description`, ruling
2026-06-10).

### What an instantiation has to supply (`requires`)

An optional block (GH #292); a template without one requires nothing. It states machine-readably
what an instantiation must provide.

```json
"requires": {
  "ctx": {
    "model": {"type": "string", "required": true, "because": "the model the brain infers with"}
  },
  "env": {
    "OPENROUTER_API_KEY": {"because": "the cell infers"}
  }
}
```

A declared key without `required` is required, and `because` is quoted verbatim when a mutation
fails on it.

The two placeholder classes stay apart because they behave differently: `${ctx.X}` is resolved once,
onto disk, at instantiation and stands as a value in the instance's `config.json` afterwards, while
`${ENV_VAR}` stays a token on disk and binds again at every read. One pot for both would turn a
secret into an instance parameter, the materialisation GH #20 prevents.

The declaration is derived, not written down beside it. What a shipped template names under
`requires.ctx` is exactly the set of `${ctx.X}` occurring in its own `config.json` values, checked
in both directions: a placeholder without an entry is a template that rejects a mutation for a key
it never advertised, and an entry without a placeholder is a leaflet asking for a value nobody
reads. Prose does not count.

Authoring rule, param or ctx: anything that can differ between two instances of one template is a
param, addressable per instance through `override_params` (on a subtree template by the cell's path
inside the template, GH #140). `${ctx.X}` is mutation-wide, because one mutation carries exactly one
`ctx`, so every instance that mutation creates gets the same value.

### Templates registry (in `colony.db`)

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

- At start the registry is loaded from `colony.db` with no filesystem scan, which makes the start
  fast.
- A first-time start with an empty registry triggers an automatic scan.
- A manual rescan runs via the CLI flag `meclaw --rescan-templates` or the API `POST
  /colony/templates/rescan`. The endpoint answers `200` with `{"rescan":{"status":"ok"}}` when the
  scan ran through, and `422` with `{"rescan":{"status":"error","error":"<the scanner's own
  words>"}}` **when it aborted (GH #440)**, for instance on a name collision, which the scanner
  names with both directories. The EDA door returns the same wording in the same shape.
- `local/` is not a special case: the directory `add_templates` writes into is an ordinary
  subdirectory of the template root, found by the recursive scan like any other. `local/` is a
  convention of the writing side.
- The scan is recursive with no exclusion. `scan_templates_dir`
  (`crates/meclaw-colony/src/templates/scanner.rs`) walks the whole tree below `templates/`, and
  every directory with a `template.json` is registered regardless of depth and parent name.
  `templates/drafts/<name>/` is therefore fully instantiable, whatever the name suggests. Draft and
  staging material does not belong below `templates/`; builder staging lies in `<root>/staging/` and
  is promoted via `rename(2)`.

### Resolution `name@version`

```json
"template": "llm-openai"           // → the one registered version
"template": "llm-openai@2.1.0"     // → exactly this version
```

Without a version you get the one version registered under that name. Correction (GH #277): the
earlier rule "Without a version: the highest SemVer version" is dead under the uniqueness rule and
lives on only as a tie-break inside `TemplatesRegistry::resolve`, for a registry built by some means
other than the scan. SemVer ranges (`^`, `~`) are post-roadmap.

On errors: a template referenced but not in the registry fails the instantiation, sends an error
message to `reply_to` if set, and the mutation is rejected, with a batch mutation rejected entirely.
A registry entry whose directory is gone is checked lazily at the instantiation attempt (an error
plus automatic removal from the registry), and `--rescan-templates` deletes all registry entries
without a directory. Existing instances keep running, because they have their own filesystem copy.

### Instantiation flow

1. Colony receives a mutation message to `/colony/mutations` in which an `add_nodes` entry describes
   a cell to be instantiated (`name`, `template`, optional `override_params`).
2. Lookup in the registry: template reference to `filesystem_path`.
3. Copy `templates/<path>/` recursively into the staging directory
   (`.staging/<mutation_id>/<name>/`).
4. Generate a new UUID v7 for all copied cells and edges.
5. Patch `config.json` with the new UUIDs. The name stays as in the template, or as given in
   `override_params`; on a collision with sibling names within the same scope the mutation is
   rejected.
6. Resolve the instance class (`${ctx.*}`, `${uuid7:*}`). The environment class (`${VAR}`) stays
   literal in the written `config.json` and is resolved in memory only, for the cell being started.
7. Stamp the origin into `cell.provenance`: resolved template name, resolved template version,
   instantiation time in unix seconds. It is the same write as `cell.id` and never happens again.
   Correction (GH #277): the earlier "For a subtree template every node of the instance receives the
   same stamp" is retracted; every node receives the stamp of the template it is an instance of, and
   the composites that placed it are listed in `cell.provenance.template_chain`, outermost first
   (`config.md` § Origin).
8. Initialise `cell.db` from `seed/`, if present.
9. Atomic `rename(2)` from staging to the target path.
10. Register the instance in colony's `HashMap<Path, ActorHandle>` and spawn the actor task: the
    `cell_task` loop for stateful cells, the `stateless_dispatcher` loop for stateless cells (with a
    `Semaphore` from `params.max_concurrency`), the double-task pattern for long-running cells. In
    all three cases the mailbox is allocated as a bounded mpsc, default capacity 1000, overridable
    via `cell.mailbox_size`.
11. Pass `params` to the instance at start.

### Discovery and lifecycle of templates

`GET /colony/templates` provides the template list from the registry with `name`, `version`,
`template_id`, the full `description` block, `tags`, and later a vector embedding for semantic
search. Today it is plain text matching plus a tag filter; an embedding index and vector search are
post-roadmap.

Templates are read-only classes and are never automatically removed.

A template is added at runtime with `add_templates` (GH #440). It enters the instance-local library
(`{templates_root}/local/<name>/`) as a mutation declaration and is resolvable from that same
mutation onwards, while the shipped library is out of reach because the target path is built rather
than taken. It becomes visible only when the mutation commits (GH #443): the files land in the
mutation's own staging area, a later entry of the same diff resolves against them there, and the
`rename(2)` into the library runs immediately before the commit flush, so a refusal further down the
same mutation leaves the library as it found it.

Manual removal runs exclusively via the filesystem: delete the template directory in `templates/`,
then `--rescan-templates` or `POST /colony/templates/rescan` so that the registry takes over the
state. The diff has no `remove_templates` operation, and that is a decision: removing a class that
instances grew out of makes none of those instances invalid and none of them restorable.

## Seed concept (JSONL format)

DBs are never stored as binary files in templates. Instead they are version-safe JSONL at
`<cell>/seed/<table>.jsonl`:

```
{"schema": {"col1": "text", "col2": "int", "col3": "json"}}
{"col1": "value", "col2": 42, "col3": {...}}
{"col1": "value2", "col2": 43, "col3": {...}}
```

Line 1 is the schema declaration and lines 2 onwards are records. No binary DB means no schema
drift, and the file greps and appends like any text file.

On fresh `cell.db` creation (`OpenStatus::Created`) colony reads the seed and builds `cell.db` anew.
On reopening an existing `cell.db` (`OpenStatus::Resumed`) it is not re-seeded, otherwise there
would be duplicate rows.

Export (GH #253) is the inverse of the same mechanism: a message carrying the body slot `transfer`
with `{"operation": "export", "table": "<t>"}` is answered by the substrate
(`crates/meclaw-colony/src/db_transfer.rs`, called in `cell_task` before `handle()`), and the answer
is a document `{format, table, key, schema, rows}`. Write its `schema` object as line 1 and one row
per line after it and the result is a `seed/<table>.jsonl` the existing loader reads. Without
`table` the export answers with the inventory of content tables.

The export now writes the file itself, retracting the earlier "The export does NOT write the file
itself" (GH #555, ruling R-0904-3: cells manage their own files). `{"operation": "export", "to":
"<dir>"}` writes `<dir>/seed/<table>.jsonl` and, last, the marker `<dir>/seed/export_final.json`;
`{"operation": "import", "from": "<dir>"}` reads the same directory back. Nothing is ever written
into `{root}`, only into the absolute fence the cell declares for itself
(`params.transfer.base_path`, `config.md`), on a `to` or `from` the caller names, answered by a
receipt carrying `rows` and `seed_dir`. Format, fence, `error_code`s and the whole-or-nothing
promise: `cell-types.md` § Content transfer.

Import (GH #253): the same body slot with `{"operation": "import", …}` takes such a document into a
running cell, the half the seeder cannot reach. Three decisions: on a key collision the target wins,
always, never an update or an overwrite; the import is additive, with no delete and no
truncate-and-load; and a partial import is a state, not a failure, because everything checkable is
checked before the first write, the writes run in one transaction, and re-applying is idempotent.
With `"from": "<dir>"` (GH #555) the same document comes out of the directory instead of out of a
message, in one transaction over every table the call names (GH #261).

A seed builds the table without a key, and the owning cell type puts it back (GH #255). The staging
seeder creates every table from the header line alone (`CREATE TABLE IF NOT EXISTS`, no
constraints), before the cell has ever been awake. For a store-owned table of a `params.canonical`
binding (`aliases`, `rejected`) that would cost everything, because their ops are upserts on exactly
that key, so the `store` asserts the key at spawn: such a table standing without it is rebuilt with
it, every row comes along, duplicates collapse onto the key (the most recently `recorded_at` row
wins) and a column the declared shape does not know is carried over. A template may therefore ship
such a seed.

GH #398 draws a second line here: **A cell type with a fixed schema is not seeded at staging at all**.
The header line describes rows, not a schema: column names and a coarse type, with no key, no `NOT
NULL`, no default, no index and no column order. For a type whose tables are fixed in code the
staging seeder gets there first, and the cell's own `CREATE TABLE IF NOT EXISTS` then finds the
constraint-free table standing and leaves it. Such a type says so through
`CellFactory::owns_schema`, and staging writes nothing into its database: no tables, no rows, not
even the file. It creates its own tables and loads its own seed at first spawn
(`OpenStatus::Created`). A cell instantiated before the fix does not heal itself: instantiate it
again.

A `seed/` beside a type that owns its schema **is a refusal, not a quiet nothing** (GH #399).
Whoever declares `CellFactory::owns_schema` must load their own seed files. `web` does. `harness`,
`mcp`, `proxy`, `subcolony`, `timer` and `vault` declare it and deliberately have **no** loader, so
a `seed/*.jsonl` beside one of them could never load by anyone's hand, and the plan phase refuses
it, naming the file and the cell type. Only `*.jsonl` counts. `llm` is not among them and must not
be: its `system` table comes from the shared `setup_cell_db` DDL the seeder applies first, which is
why `templates/talky/brain/seed/system.jsonl` works.

A third writer is `seed_rows` at the mutation door (GH #456). It goes through the authority's door,
as a diff operation and therefore with a digest, a gate, an access verdict and a `mutation_log` row,
on the same mechanic (the same JSON to SQL binding, the table built from the declared column list,
`ensure_keyed_table` repairing a missing key at the next wake); only the moment differs, because the
colony is running and the target may be awake. It is meant for rows that are permissions and keys,
not for bulk data, which keeps going through the import slot. Unlike a seed file, a `seed_rows` is
substituted like every other part of the diff.

A seed is not variable-substituted. `${VAR}` in a seed row is written into `cell.db` verbatim:
bootstrap substitutes `config.json`, and the seed loader (`seed::load_seed_if_present`,
`crates/meclaw-cells/src/store/factory.rs`) does not, because resolving a variable into persisted
rows would freeze one boot's environment into the database forever. A value a seed row and a
`config.json` both need, such as the memory-hive template's embedding model id, is coupled by hand,
and the template README says so.

## Variable substitution

meclaw knows three substitution sources, all with `${...}` syntax.

| Token | Source | Who substitutes | When |
|---|---|---|---|
| `${ENV_VAR}` | from `.env` in the root | Colony | at every read of `config.json` (boot and instantiation) and in mutation diffs, in memory only, never on disk |
| `${ctx.<key>}` | from the header or body of the mutation message itself | Colony | on mutation application, once; the value is written to disk |
| `${uuid7:label}` | freshly generated per label | Colony | on mutation application, once; the value is written to disk |

All three sources are substituted exclusively by colony, because the flat substrate has no
intermediate layer that would have its own tokens.

Two classes, two owners. `${ctx.*}` and `${uuid7:*}` belong to the instance: resolved exactly once
at instantiation, standing as values in the `config.json` afterwards. `${ENV_VAR}` belongs to the
environment: the token survives instantiation literally and is re-bound at every read, so
instantiation materialises no secret, `contract.settings.*.default` included. The price is a
standing dependency: a variable that disappears later fails the boot loudly (`env_var_missing`).
Instances already materialised are not rewritten, so the rule applies forward.

A third destination is the files of a registered template (GH #611). `add_templates[].files` belongs
to neither class, because a class has no instance to bind to yet: the file bodies are written byte
for byte, and every `${…}` in them binds where it always binds, the instance class at instantiation
and the environment class at read time. A `${…}` in the prose of a README is therefore not an error.
The entry's other fields (`name`, `version`) are substituted like every other part of the diff.

### `${ENV_VAR}` from `.env`

- `.env` file in the root, classic key=value format.
- Substitution by colony in memory, before `params` are passed to the cell. The cell sees only the
  substituted value; the file on disk keeps the token.
- POSIX-style default supported: `${VAR:-fallback}` provides `fallback` when `VAR` is empty or
  unset. `${VAR}` without a default is strict, and a missing variable is an error.
- Escape: `$${...}` escapes to a literal `${...}`. The escape survives instantiation unchanged and
  is consumed only at read time.
- The strict variant `${VAR:?error_msg}` is not supported; any other `${VAR<op>...}` form besides
  `${VAR}` and `${VAR:-fallback}` is rejected with `unsupported_substitution`, with no silent
  pass-through.

### `${ctx.<key>}` from the mutation context

- Allowed only in mutation diffs, not in `config.json` on the filesystem side.
- Access to the `ctx` block of the mutation message: `${ctx.user_id}` gives the value of the
  `ctx.user_id` field. The resolution is strict from the `ctx` block, with no fallback; a missing
  key is a reject with `ctx_key_missing`.
- It lets the requester inject application-own identifiers (`user_id`, `session_id`, `turn_id`) into
  names and `override_params`, placed explicitly in the `ctx` block. Colony reads nothing
  automatically out of the `headers.context` compartment.

### `${uuid7:label}` fresh UUIDs

- Generates a UUID v7 on the first occurrence of a label in a mutation. All further occurrences of
  the same label in the same diff get the same value, and different labels get different UUIDs.
- Labels are freely choosable (`sess`, `s1`, `worker_a`) and valid only within the one mutation
  message, forgotten after mutation completion.
- The form without a label (`${uuid7}` plain) does not exist. Explicit labels are mandatory, and
  they prevent the foot-gun where every occurrence would unintentionally be a new UUID.

```json
{
  "scope": "/main",
  "diff": {
    "add_nodes": [
      {
        "name":     "session_${uuid7:s}",
        "template": "session-scope@1.0.0",
        "override_params": {
          "user_id": "${ctx.user_id}",
          "api_key": "${OPENAI_KEY}"
        }
      },
      { "name": "worker_${uuid7:w}", "template": "worker@1.0.0" }
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

| Error | When caught | Reaction |
|---|---|---|
| Missing `${ENV_VAR}` without a default at the initial colony bootstrap | before pipeline start | daemon failed to start, non-zero exit code, error on stderr and in the log |
| Missing `${ENV_VAR}` without a default at mutation validation | mutation validation | error reply to `reply_to` if set, mutation rejected |
| Missing `${ctx.<key>}` | mutation validation | error reply to `reply_to`, mutation rejected |
| Cell-init follow-on error from an invalid substituted value, for instance an invalid API key | cell init after commit | restart one_for_one, after N retries `failed` status |
| `${uuid7:label}` | never missing, always generated | none |
| Name collision in the `post_state` after substitution | mutation validation | error reply to `reply_to`, mutation rejected |

The naming default is strict: if a mutation produces a node name that occurs twice within the same
scope in the `post_state`, the entire mutation is rejected (`error_code: "naming_collision"`), with
no auto-suffix and no path magic. Whoever needs bulk instantiation with a uniqueness guarantee uses
`${uuid7:label}` or `${ctx.<key>}` with application-stable tokens.

If the requester needs to know the resolved name afterwards and no application-stable token is
available: generate the UUID outside the mutation and insert it as a literal, or query
`/colony/registry` afterwards, which provides `id`, `name`, `path`, `type` and `status` per instance
in time-sorted order.

## Blob storage

Blobs live in the `blobs/` directory (default `{root}/blobs/`, CLI-overridable via `--blobs`). Every
blob consists of two files:

```
blobs/<uuid-v7>.<ext>            # blob content
blobs/<uuid-v7>.<ext>.meta.json  # sidecar with authoritative metadata
```

`<uuid-v7>` is the blob ID, time-sorted. `<ext>` is the native file extension derived from the MIME
type: `.json` for offloaded bodies, and from phase 12 also `.pdf`, `.txt`, `.png`, `.jpg` and more,
depending on the `attachments[]` slot convention.

```json
{
  "schema_version": 1,
  "mime_type":      "application/json",
  "size_bytes":     123456,
  "sha256":         "abc...",   // optional; omitted in phase 12 (no consumer)
  "created_at":     "1747650225",
  "filename":       null
}
```

- `mime_type` is authoritative MIME info. Consumers read the sidecar, not the extension, which is
  only operator convenience for `ls blobs/`.
- `filename` is the original filename on upload via the HTTP API, `null` for system-generated blobs.
- `sha256` (optional) is a content hash for integrity checks and dedup potential, both post-roadmap.
  It is not computed in phase 12 and the field may be missing.
- `created_at` is Unix seconds as a string (`"1747650225"`, `unix_seconds_string` in
  [`crates/meclaw-colony/src/blob/disk.rs`](../crates/meclaw-colony/src/blob/disk.rs)), not
  ISO-8601, whatever an older revision of this document showed.
- `schema_version` allows future sidecar extensions without a migration break.

Two things about the blob layer are frozen behind that version field: a human-readable `created_at`
format, and sharding `blobs/` into uuid-prefix subdirectories (the `read_dir` scan in
`DiskBlobStore` is a known cost). Both are allowed only together with a `schema_version` bump. A
flat directory and a numeric string are contract until the number moves.

Behaviour:

- The offload threshold (default 64 KB) is configurable via `blob_inline_max_bytes` in
  `colony.json`.
- On writing, a body at or above the threshold is offloaded as `blobs/<uuid>.json`, the sidecar is
  co-written, and only `Blob(uuid)` remains in the message. The `==` boundary case is inclusive,
  because the `Body` enum canonically implements `≥`.
- On attachments (from phase 12), files uploaded via the HTTP API (`multipart/form-data`) are stored
  as `blobs/<uuid>.<ext>` with the real MIME type, and the associated message carries an
  `attachments[]` slot entry. Write order: first the blob file (`tmp` then `rename(2)`), then the
  sidecar as a commit marker, likewise via an atomic `rename(2)`. Reader convention: a blob counts
  as complete exactly when its sidecar exists, and blobs without a sidecar are ignored.
- On reading, the cell calls a storage abstraction that co-loads the sidecar and returns content
  plus MIME info; for `attachments[]` that is the `AttachmentReader` (GH #87), handed only to cells
  declaring `consumes.body.attachments`, with an operation timeout per read. JSON bodies are
  deserialised transparently as `serde_json::Value`.
- No automatic GC runs. Blobs fall under the no-delete policy like the rest of `{root}/`, and
  disk-space management is an operations matter (external archiving via rsync, tarball, S3).

The phase table that used to stand here is gone with the rest of the roadmap: phase 3 brought the
body offload, phase 12 the real attachments and the multipart upload, and cell-type-specific
consumers came after. `CHANGELOG.md` carries what shipped when.

## No-delete policy

- No file in `{root}` is ever deleted. Only new files and directories arise.
- Relocating is not deleting (GH #169): `move_nodes` renames a cell's directory with `rename(2)`.
  `config.json`, `cell.id` and `cell.db` travel as the same inode, the registry row is re-addressed
  rather than deleted and re-created, and every edge names the new address afterwards. The policy
  protects data and identity, not path constancy for its own sake. What it forbids is quiet
  disposal, which is why a move is a named, validated, atomically committed operation.
- Instances are immortal: a once-instantiated cell stays forever on the filesystem, keeps its
  `cell.db` and is findable via UUID.
- Disconnect instead of delete: cells no longer needed lose their edges (`remove_edges`,
  `remove_nodes`) and thereby become inactive, no longer routed, with no tasks. They continue to
  exist on the filesystem and in `colony.db` and can be reconnected at any time via `add_edges`, or
  a renewed `add_nodes` at the same path, with the same `cell_id` and a resumed `cell.db`.
- Paths are stable until somebody changes one on purpose. The only way to change a path is
  `move_nodes`, a mutation that stands in the mutation log and carries everything that keys on the
  path with it.
- Hierarchy is builder discipline: whatever triggers the instantiation chooses path and name
  deliberately to avoid root-directory pollution, for instance `memory/2026-05-16_user_xyz/`.
- The audit trail is built in, and the backup strategy is trivial, because the whole `{root}` is a
  snapshot and Git-capable. Archiving old directories externally is an operations concern.
- Carve-out for spawn-reject residue: no-delete holds absolutely for registered cells. The only
  exception is the cleanup of fresh, never-registered directories at a spawn reject
  (`sweep_reject_residue`, `crates/meclaw-colony/src/colony.rs`): an `add_nodes` or swap dir just
  renamed from staging into the live tree whose spawn fails is removed, because it was never a
  living, registered cell. Adoption targets are protected by the `preexisting_target` guard and are
  never deleted.

## Startup algorithm

1. Colony starts with `{root}` (the CWD by default, or `--root`).
2. Mutation recovery. Colony scans `colony.db` for mutation entries with status `in_flight`,
   interrupted at the last crash. Per entry it deletes the staging directory
   `{root}/.staging/<mutation_id>/` if present and marks the mutation `failed` with `failure_reason:
   "crash_during_commit"`. Cell directories already renamed to their final paths remain as orphans
   in the live tree, because the no-delete policy holds, and colony considers them at the filesystem
   bootstrap by their `config.json`. `.staging/<mutation_id>/` directories without an associated
   `colony.db` entry are cleaned up as well.

   Bootstrap recovery: the first apply writes a durable `bootstrap_in_flight` marker into the `meta`
   table of `colony.db` before the first cell spawn, and its deletion runs atomically in the same
   transaction as the `InitialApply` bundle (edges plus hive_scopes). If the boot-state
   classification finds the marker, the last first apply was interrupted, so the boot is classified
   as **FirstBoot** and the apply runs again as an idempotent resume: the FS is the source, the
   registry upserts are `cell_id`-stable via the identity overlay, and the bundle is `INSERT OR
   IGNORE`. No operator intervention, no deleting the DB. Without the marker (GH #89), **Reboot**
   means the InitialApply bundle has committed at least once, so edges or hive_scopes are non-empty;
   edge-less or cell-less contents (single-cell colonies without edges, hive-only roots, staged
   builds before wiring) are legitimate persisted shapes. Registry rows alone do not prove a reboot:
   runtime-spawned cells persist registry upserts before the first filesystem bootstrap ever runs,
   and that state classifies as FirstBoot. `Inconsistent`, a strict-fail boot panic, is reserved for
   a file whose persistence tables are unreadable; real data corruption inside readable tables is
   caught loudly at the read layer.
3. Templates registry. Colony reads the templates registry from `colony.db`. If it is empty, or
   `--rescan-templates` is given, it scans `templates/`.
4. Growth from references, FirstBoot only (GH #424). The planning pass classifies every
   `config.json` with `cell.type: "ref"` (or the key `cell.template`) as a declaration. On a
   FirstBoot each one is materialised through `mutation/subtree.rs::stage_subtree`, with the same
   resolution, substitution, seeds and refusals as at the mutation path, and the marker is replaced
   by what it names. The plan is then made again, and this repeats while markers remain, so a nested
   marker in the grown tree grows in the next pass. The bound is the number of markers of the first
   pass plus one. On a reboot nothing grows: a marker there is an `unregistered_node` and is
   reported (A5b). The growth runs before the apply, or the colony would spawn half a tree.
5. Registry rehydration and filesystem validation. Colony rehydrates the registry from `colony.db`,
   and known paths keep their persisted `cell_id` and their active or inactive status. The recursive
   tree walk validates the filesystem state against the persisted state. Registration happens
   exclusively through instantiation or mutation and never through boot discovery (A5b): at the
   first bootstrap the walk is the source and every unknown `config.json` node is recorded as a new
   entry, while on a reboot an unknown node is only reported and never adopted (a WARN in the ops
   log; in `--validate` a warning with exit 0, and with `--validate-strict` an error). Such a node
   becomes a registered part of the graph only through a mutation on its path. For no already known
   path is a new `cell_id` assigned. Hive scope markers: read the `params.graph` hint and enter it
   as declarative edges for the scope, insofar as it is not yet persisted. **Edges only, because
   this step instantiates no node**: nodes grew in step 4, and `params.graph` carries edges and
   nothing else. The `nodes` block described under Graph schema remains an error at the boot parser
   (GH #424).

   Derived activity from the first bootstrap onward follows the one activation rule of §
   Connectivity and activity: the computation is seeded from the `params.graph` edges, and only the
   newly recorded nodes reached by it are brought to their edge-derived state. Islands therefore
   boot inactive and their permanent runners do not spawn. The root `/` is by definition always
   active, and an already known node keeps its persisted status.
6. Hydrate the edge table. Colony reads the persisted edge table from `colony.db`. On conflicts
   between `params.graph` hints and persisted edges the persisted state wins, because hints are only
   the initial desired state at first instantiation.
7. Spawn long-running cells. For each active `proxy`, `timer`, `mcp`, `web` and `voice` cell, start
   the double-task pattern directly, with no lazy wake. Inactive long-running cells are not started.
   Emissions that arise during the first apply, because an eager I/O task polls from its spawn on,
   are held back by the colony until the InitialApply bundle has committed, and route afterwards in
   emission order (GH #389).
8. Start mailbox pumping. Colony starts its own routing loop. The HTTP API and web UI bind on the
   `--api <bind>` address if the flag is set; otherwise the HTTP layer is inactive.

Before the first cell spawn, colony checks that every `params.graph` edge endpoint is resolvable,
against the filesystem plan (cells plus hives), the already running registry, or a `/colony/*`
endpoint. An endpoint that points to none of these leads to a loud boot failure naming the edge ID
and the missing path. `--validate` cannot see runtime-spawned cells and therefore reports a
non-resolvable endpoint as a warning (exit 0, the nginx `-t` role); `--validate-strict` raises these
warnings to errors.

Since all instances are persistent, this start is fast: cells are rebooted from the existing
filesystem instead of created anew.

## Connectivity and activity

Every node of the graph, cell as well as hive, is at every point in time active or inactive. The
state is fully derived from the edge table, and there is no activation or deactivation command in
the mutation surface.

Connectivity rule: a node is connected when it participates, on its level, that is in the enclosing
scope, in at least one edge, as `from` or as `to`. A single incoming or outgoing edge suffices, so
sources like `timer` and `proxy` are connected via their outgoing edges.

For the connectivity of a hive, only external edges count. External means exactly one endpoint lies
in the unit and the other outside it, and the unit is the hive path together with its entire
subtree, not the subtree alone (GH #265). Both forms fall out of that single condition: an edge of
the parent level naming the hive path as `from` or `to`, and an edge naming a descendant without
touching the hive path at all (depth-port wiring, for instance `/anchor` to `/unit/dispatch`, R12
ruling 2026-06-11).

That the hive path belongs to its own unit is the load-bearing part: the wiring `<hive>` to
`<hive>/<cell>` is mandated by the hive boundary, and if it counted as a connection a swapped-out
generation would stay awake while nothing reaches it (GH #265, fixed).

The internal wiring is thereby meaningless for hive connectivity: a hive with a richly wired
interior and no external edge is unconnected, and inactive along with its entire subtree. Conversely
a single external edge, referencing or crossing, keeps the hive connected.

There are ways to reach a unit that are not edges: `POST /messages` may name any path as `target`,
and a source cell on the inside (`proxy`, `timer`) mints messages with no incoming edge. Both are
entries into a unit rather than connections of it, and neither makes an unwired hive active. A unit
with no external edge has no way out for an answer either, since a message running back to the hive
path is dead-lettered as `hive_no_route`.

Activity rule, recursive: a node is active exactly when it is itself connected and its parent hive
is active. The root is by definition always active. A disconnected hive therefore deactivates its
entire subtree, independent of its internal wiring. Tokio tasks run exclusively for active cells:
long-running cells run exactly when they are active, and stateful cells additionally follow the
hot/cold model within "active", the two axes being orthogonal.

The one activation rule, event-driven, for boot as well as mutation: the activity of a node is the
result of the last connectivity computation that reached it, and a node never reached keeps its
instantiation activity. The first bootstrap seeds the computation from the `params.graph` edges and
recomputes only the nodes reached by it; a mutation seeds from the diff edge endpoints. It takes
effect as soon as a connectivity recompute reaches the scope of a node. **Freshly instantiated nodes
start active unless the entry declares otherwise** (`add_nodes[].birth`, GH #437), and a node born
that way carries a durable marker (`colony.db` `registry.dormant`, GH #491): neither the recompute
of its own birth mutation nor any later one makes it active again, and it is woken solely by a
mutation that addresses it itself. Otherwise nodes are brought to their edge-derived state by the
first recompute that touches their scope. At a subtree `add_nodes`, or an island at boot, the
internal edges seed the recompute over the own scope, so inactive-derived subtree and island nodes
do not eager-spawn. At a pure single-cell `add_nodes` without an edge, and symmetrically at an
edge-less single cell at boot, the recompute trigger is missing and the node stays active for lack
of a trigger (grace), so it produces no transient spawn-then-stop. An edge-less single cell within
an unconnected sub-hive likewise keeps the grace. The other edge case, a single-cell `add_nodes` of
a long-running cell whose diff edges derive it inactive, is fixed: the activity gate before the
eager spawn evaluates the post-state edge view and registers the cell inactive without a task spawn.

Disconnect, when the last edge of a node is removed, typically via `remove_edges` or `remove_nodes`:

- Colony recomputes the connectivity of the affected scope after every mutation and marks
  disconnected nodes, at hives including the entire subtree, as inactive. The marking is persisted
  in `colony.db`.
- Running tasks end gracefully: a running `handle()` call runs to its end, then the task ends. At
  long-running cells the handler and I/O task are stopped, so external polling ends. If the cell
  blocks during the disconnect on a full `outputs`, the `term_timeout` reject applies as an atomic
  rollback; drain support during the disconnect window is post-v0.1.0.
- Residue in the mailbox of a deactivated cell runs into the dead-letter queue with `error_code:
  "cell_inactive"`.
- The registry entry, filesystem, `cell.db` and `cell_id` remain fully preserved, because a
  disconnect is a stilling, not a deletion.
- Inactive nodes do not participate in routing: every routing decision to an inactive path goes into
  the dead-letter queue with `error_code: "cell_inactive"`, never `unresolved_path`, because the
  path exists and is only stilled.

Reconnect happens when a node receives an edge again, typically via `add_edges` or a renewed
`add_nodes` at the existing path. This same path is the wake semantics of a node born inactive
(`add_nodes[].birth: "inactive"`, GH #437): there is neither an operation nor a message of its own
for it, and the next mutation that addresses it itself wakes it.

Reaching is not addressing (GH #491). A node born inactive is fully wired, because the declaration
marks the registry row instead of withholding the edges, and `affected_scope` expands every involved
path to its whole subtree, so a recompute that merely brushes it would derive it active again. A
declaration beats a recompute side effect: the birth sets a durable marker (`registry.dormant`,
schema v8) that every recompute honours. It falls exactly when a mutation addresses the node itself,
so when its path is in the `involved` set, as an endpoint of an `add_edges` say, or as the target of
a `swap_nodes`, and from then on the edge table alone decides. A mutation elsewhere in the tree
never wakes it, however far its recompute reaches. A node put to sleep with `remove_edges` carries
no marker and needs none: it has no edge, so no recompute can derive it active.

The reconnect itself is unchanged. The node, and recursively its subtree insofar as it is internally
connected, is again marked active. Long-running cells of the reactivated subtree are started
immediately, as at colony startup. Stateful cells start lazily at the first message receipt (the
hot/cold model, wake on message), while stateless cells start eagerly on reconnect like long-running
ones, because they have no wake path. Every `cell.db` is resumed, with no re-initialisation, an
unchanged `cell_id` and no `config.json` rewrite.

An island, a subtree or sub-hive that at boot was derived inactive for lack of an external edge, is
activated exclusively via an `add_edges` mutation that introduces an edge crossing the scope
boundary into the island. That crossing-in edge has exactly one endpoint within the island subtree
and the other outside; a purely internal edge does not suffice. This mutation seeds the connectivity
recompute over the island scope, and the activation cascades from there recursively through the
internally connected subtree. It is the only sanctioned activation path; the earlier runbook trick
of booting the materialised instance subtree via re-root is superseded.

Derived activity is written down (GH #495). It is not re-derived on every boot: the boot recompute
runs only for nodes without a registry row, and a rehydrated node takes its activity from the
persisted `status` column. Hence the rule: a registration writes down the activity it registers, and
a recompute writes down every flip, both halves, always, for the boot as for a mutation, single cell
as well as subtree. Unchanged by it: the `failed` status, which `inactive` never overwrites; the
grace of an edge-less cell, which is registered active; and the `dormant` marker, which only a
declared birth receives (ADR-0018).

`/colony/registry` still shows inactive nodes, with the field `active: true|false` per entry and an
optional filter `?active=true|false`.

Deriving active and inactive from the edge table keeps the state reconstructable from the graph and
prevents drift between "declared deactivated" and "actually unwired". Rejected were an explicit
`deactivate` op and a reachability traversal from the root.

## Hot/cold cell model

With many persistent instances but only a few active at once, Tokio tasks are spawned and
despawned dynamically. The hot/cold model applies only to active stateful cells. Active and inactive
is an orthogonal, edge-derived axis persisted in `colony.db`, while `NotYetSpawned`/`Awake`/`Asleep`
is the in-memory lifecycle status within "active". Inactive cells have no lifecycle status, no task,
and are not routed.

| Status | Meaning | Resources |
|---|---|---|
| `NotYetSpawned` | The cell exists on the FS and has never been spawned since colony start | mailbox channel allocated, no task |
| `Awake(JoinHandle)` | The cell runs as a Tokio task | task, mailbox, cell.db connection |
| `Asleep` | The cell despawned itself after an idle timeout | mailbox channel allocated, no task |

```
NotYetSpawned ──[first message]──→ Awake ──[idle timeout]──→ Asleep
                                     ↑                          │
                                     └──[new message]───────────┘
```

The stateful cell task selects over its mailbox and an idle sleep. A received message runs through
`cell.handle()`, wrapped in `tokio::time::timeout` when a message timeout is configured, and an
elapsed backstop emits the timeout error and ends the task so the supervisor restarts it. An idle
tick with an empty mailbox shuts the cell down and ends the task.

Operation timeouts (A) for I/O live within `cell.handle()`. `IDLE_TIMEOUT` is configurable: a global
default in `colony.json` `idle_timeout_default_ms` (recommendation 60000), overridable per cell via
`cell.idle_timeout_ms` in `config.json`, and it takes effect only at `cell.timeout: 0`.

`cell.timeout` from `config.json` controls the behaviour of stateful cells: `0` (the default) is the
idle-timeout model (Awake to Asleep), a value greater than 0 is one-shot (despawn after every
message), and `-1` is persistent (proxy, timer, mcp, never despawn).

Stateless cells do not have this three-state model. The dispatcher task is permanently awake, holds
no persistent state and has almost no idle cost, and the sleep and wake mechanism presupposes state
preservation between `Asleep` and `Awake` via the `cell.db`. Long-running cells are permanently
awake, because the I/O task does external polling and the handler task waits for incoming events.

## Supervision, backpressure and timeouts

### Restart strategy

- `one_for_one` is the only strategy, supervised by colony. When a cell panics, exactly that cell is
  re-instantiated. Cells are decoupled by design and know no topology, so the OTP strategies
  `one_for_all` and `rest_for_one` solve problems meclaw architecturally does not have.
- State preservation on restart: `cell.db` is reloaded, in-memory state is lost.
- The restart limit defaults to 5 attempts per cell, overridable via `cell.restart_limit`. Restart
  is immediate without backoff and there is no sliding window, so a deterministically panicking cell
  fails quickly after 5 attempts and is marked `failed` in the registry. Routing to a `failed` cell
  runs into the dead-letter cascade. A `failed` cell returns via the normal reconnect semantics when
  a mutation directly addresses it, as an edge endpoint or a resume; incidental recomputes do not
  reactivate it, and the reactivation resets the restart counter. Rejected were exponential backoff
  and a sliding window.
- Channel mechanics on restart: the `mpsc::channel` pair of a panicking cell does not survive the
  panic. The `Receiver` is dropped during the stack unwind of the `cell_task`, so the previous
  `Sender` in the registry points to a closed channel (`SendError`). On restart the supervisor
  creates a fresh `mpsc::channel(1000)` pair, calls the respawn closure with the new `Receiver` and
  replaces the `Sender` in the registry atomically. The message being processed is lost with the
  frame that was handling it. The waiting mailbox messages survive (GH #18): a `MailboxGuard`
  ([`crates/meclaw-colony/src/mailbox_rescue.rs`](../crates/meclaw-colony/src/mailbox_rescue.rs))
  owns the receiver for the whole life of the cell task, and its `Drop`, which runs on an unwind and
  on a task abort alike, drains the remainder into `ColonyMsg::MailboxRescued`. The colony holds it
  and delivers it in order to the successor after the respawn, and the ordering carries itself
  because the guard hands over strictly before the `JoinHandle` resolves. A death that leaves no
  successor, a normal exit whose entry is removed or an exhausted `restart_limit`, dead-letters the
  rescue instead.

The asymmetry is deliberate. The guard sits in `cell_task_stateful` and in the long-running handler
loop. `stateless_dispatcher` does not carry it: a stateless cell dispatches a short-lived worker per
message, so a panic there dies with its own message. Until the dispatcher's own mailbox is decided,
a stateless cell's waiting mailbox is still lost on a dispatcher death. The peaceful exits are
untouched: peace-stop, idle-sleep and one-shot hand the whole receiver over themselves and disarm
the guard while doing so.

`handle_cell_died` is await-free between the RespawnFn call and the registry sender swap. In the
implementation ([`crates/meclaw-colony/src/colony.rs`](../crates/meclaw-colony/src/colony.rs)) there
is no `.await` point between `(entry.respawn)()` and `entry.handle = ActorHandle::new(...)`. The
`tokio::select!` loop in `colony_task` processes a `ColonyMsg::CellDied` event iteration completely
before it returns to the next `inbox.recv()`, so the serial loop is the restart ordering barrier.
The quiescence tests hang on this barrier, and any status persistence introducing a
`colony_db.send_op(...).await`-like point between the RespawnFn and the sender swap breaks the
restart race safety. If an await ever becomes mandatory, the restart barrier is reassessed rather
than waved through. The inactive marking from § Connectivity and activity is set exclusively in the
mutation path (`handle_mutation`), never in the restart handling.

### Mailbox and backpressure

Mailboxes are bounded with a default of 1000, overridable per cell via `cell.mailbox_size` in
`config.json`.
`block` is the only backpressure strategy in the entire system, with no cell-, colony- or
path-specific overrides (semantics and the saturation limit: § Backpressure). `ActorHandle` is a
trivial wrapper around `mpsc::Sender<Message>`, so `handle.send(msg).await` is one line, with no
drop logic and no per-routing-step strategy evaluation.

A fully dead cell is detected by the message timeout, the `handle()` call is aborted, the cell
marked crashed and the `one_for_one` restart takes effect; the respawned cell starts with a fresh
mailbox into which the colony replays the rescued remainder in order. A `tracing` warn log on `send`
operations that block longer than a threshold gives early diagnostics.

Rejected before the commitment to `block`-only were `drop_newest`, `drop_oldest` and `deadletter`,
all of them silent loss. Whoever needs a different strategy builds it via a `code` cell as a
priority filter.

### Two timeout concepts

meclaw has two different timeout mechanisms with different purposes: the operation timeout (A) and
the message timeout (B). Whoever conflates the two gets either false restarts, when the cell-hanger
backstop is too tight, or undetected hangers, when there is no backstop.

#### A. Operation timeout (cell discipline, `params.external_timeout_ms`)

Every I/O operation that can take an indeterminately long time gets a `tokio::time::timeout` wrapper
in the cell code. It applies to HTTP calls (`web_fetch`, `llm`), DB queries (`store`), subprocesses
(`bash`), filesystem operations (`file`, `edit`) and MCP tool calls. On elapsed the cell catches the
`Err(Elapsed)` result, builds a regular error message (`header.finish_reason: "error"`,
`header.error_code` cell-type-specific such as `provider_timeout`, `query_timeout` or
`script_timeout`), emits it via `outputs_tx`, and the `handle()` call ends regularly, with no cell
restart and no task kill. Configuration is per cell via `params.external_timeout_ms`, a convention,
and individual cell types can choose semantically fitting names such as `params.query_timeout_ms`
for `store`. The cell-type default is in `cell-types.md`.

```rust
match tokio::time::timeout(params.external_timeout, http_client.post(url).send()).await {
    Ok(Ok(response))  => /* normal processing */,
    Ok(Err(http_err)) => emit_error("provider_error", http_err, &outputs).await,
    Err(_elapsed)     => emit_error("provider_timeout", /*...*/, &outputs).await,
}
```

#### B. Message timeout (substrate backstop, `cell.message_timeout`)

A backstop for pathology cases in which the cell hangs for an unknown reason: a cell-code bug
without a clean operation timeout, a tokenizer loop, a JSON-parsing pathology, internally jammed
state. It is not the primary timeout for I/O. On elapsed the `tokio::time::timeout` wrapper around
the entire `handle()` call ends it with `Err(Elapsed)`, the cell task is terminated (a `break` from
the `cell_task` loop), the supervisor detects it and the `one_for_one` restart takes effect. Colony
emits a generic timeout error message to `reply_to` (`header.finish_reason: "error"`,
`header.error_code: "message_timeout"`). The trait-object state of the cell is lost and `cell.db` is
reloaded at the respawn. Configuration is a global default in `colony.json`
`message_timeout_default_ms` (recommendation 60000), overridable per cell via
`cell.message_timeout`; a value of `0` or `-1` means no backstop, typically for `proxy`, `timer`,
`mcp`, `web` and `voice`. The stateless dispatcher and the long-running handler use the same wrapper
pattern around their respective `handle` call.

The stateless worker task spawned per message is ephemeral: it is not observed by the supervisor and
not restarted, and a worker panic ends silently with its message, under the discipline that
`handle()` is panic-free and all I/O is converted to error messages. The supervised unit is solely
the long-lived dispatcher task.

Rule of thumb: B generous, A precise. Operation timeouts are the actual protective layer for I/O and
are set tight in a cell-type-specific way. The message timeout as a backstop lies considerably
above, so that normally A takes effect first.

```json
"cell":   { "type": "store", "message_timeout": 300000 }   // 5 min backstop
"params": { "external_timeout_ms":          60000 }        // 60s query protection
```

A query taking 5 s is normal. A query taking 70 s makes A fire after 60 s, so the cell emits a
`query_timeout` error message and keeps running. An SQLite deadlock bug in the cell code makes B
fire after 5 min, so the task is killed and restarted.

| Cell type | `params.external_timeout_ms` (A) | `cell.message_timeout` (B, default) |
|---|---|---|
| `llm` | 110000 (110s) | 120000 (120s) |
| `web_fetch` | 25000 (25s) | 30000 (30s) |
| `web_search` | 25000 (25s) | 30000 (30s) |
| `bash` one-shot | 60000 (60s) | 90000 (90s) |
| `file` / `edit` | 10000 (10s) | 15000 (15s) |
| `store` | 60000 (60s) | 300000 (5 min) |
| `code` (stateful/stateless) | 60000 (60s) | 90000 (90s), an operator matter |
| `proxy` / `timer` / `mcp` / `harness` / `subcolony` | cell-type-internal, handler-specific | `0` or `-1` (no backstop, long by definition) |

These defaults are set finally in phase 7/8, and the operator overrides at any time per instance.
For `harness`, A sits on `startup_timeout_ms` and the stdin writes, while the task runtime is
deliberately unbounded, because a working coding agent may take minutes; the stop lever is the
`cancel` message. The `llm` A-timeout wraps the whole provider roundtrip including the complete
receipt of a streamed response body. The OAuth token refresh carries its own constant 30 s timeout,
deliberately not a param, because it runs inside the shared token broker.

`tokio::time::timeout` is cooperative and aborts a future only at the next `.await` point, so a pure
CPU loop without `.await` stays clinging to the worker thread and neither A nor B takes effect. The
countermeasures are code discipline (`tokio::task::yield_now().await` in long CPU loops,
`tokio::task::spawn_blocking` for real blocking operations) and observation via `tokio-console`.

| Trigger | Behaviour |
|---|---|
| An external call returns a clean error (HTTP 500, a DB error) | the cell builds a regular error message, no timeout |
| Cell code panics (`unwrap`, out of bounds) | Tokio catches the panic, the supervisor detects it via `JoinError::is_panic()` and restarts; the `message_timeout` backstop triggers the same restart, with the watcher classifying it as a backstop death kind and the panic taking priority |
| The cell is removed by a mutation during a running `handle()` | graceful: the running call runs to completion, because dropping the mailbox receiver only closes the inbox for new messages, then the task ends. No `abort()`, which would lose the drop cleanup |
| An input validation error, a missing body slot | the cell builds a regular error message, no timeout |
| The LLM answers `finish_reason: error` | a cell-type-specific error path, no timeout |

At cell level there is no explicit heartbeat, because the message timeout covers it implicitly. At
colony level there is the heartbeat watchdog: the colony task has no supervisor of its own, and it
is the one task whose death takes every cell with it.

### Heartbeat watchdog

The colony loop emits a liveness tick at the top of every iteration on a bounded channel
(`try_send`, never blocking), and an interval arm at the very bottom of the `biased select!` wakes
it about ten times a second even when there is nothing to do. A supervisor task outside the colony
task drains that channel once per `watchdog_period_ms` and counts empty periods, and after
`watchdog_threshold` consecutive empty periods that is a trip.

The tick carries a phase (GH #165). It is `Working` or `Parked`, not a bare `()`, and the loop says
it before it can block, because a blocked loop reports nothing. `Working` is emitted at the top of
the iteration, before the durable-write flush, and at the top of the inbox arm; `Parked` immediately
before the `select!`. Everything between a `Working` and the next `Parked` is one work item, so the
supervisor can ask how long the loop has been on one work item instead of only how long it has been
quiet.

Alongside the supervisor runs a second, working witness (GH #165): a `run_liveness_witness` task
that must finish one unit of real work per supervisor period (a trip through the run queue, a
freshly spawned task, a fixed CPU quantum) and reports it the way the colony reports its heartbeat.
It is judged by the same rule, `watchdog_threshold` consecutive periods with no completed unit.
`supervisor_lag` alone is too weak, because the supervisor is `sleep`-driven and a starved runtime
still wakes a timer roughly on schedule.

The watchdog detects a colony task that is gone (a panic kills the loop and closes the heartbeat
channel) and one that does not iterate for the full limit (wedged in an `.await`, or with a single
iteration that takes longer than the limit). It does not detect a live loop whose cells block each
other. The supervisor counts nothing until the filesystem bootstrap has completed, because a boot is
not a steady state, and a boot that fails never arms.

The limit is a statement about a single iteration: the default of 5 x 100 ms says no iteration may
take longer than half a second. It also covers what runs synchronously inside the colony task,
because the colony is the only write authority: an instantiating mutation creates cell directories,
opens `cell.db` files, runs migrations and spawns cells. On a debug build or a busy machine that can
exceed 500 ms, and the trip is then correctly measured and still not a defect. The three
`colony.json` fields exist for those cases (GH #84).

The watchdog carries two limits instead of one (GH #165). The 500 ms limit is unchanged and applies
to the parked loop. A loop that has declared a work item gets a separate, larger limit,
`WORK_ITEM_BUDGET_FACTOR = 10` times the window, 5 s by default. A declared work item that outlives
that limit too is fatal again: the budget bounds the suppression, it does not remove it.

Every `/colony/*` says its name (GH #439, GH #571). A mutation declares itself under its id and its
scope (`mutation <id> scope=<scope>`), a read under its endpoint (`colony-read /colony/graph`), both
through the same work pulse and both on the same `work_item_budget`. A read that takes long is
therefore a named work item (`slow_work_item`) instead of nameless silence (`colony_loop`).

| Policy | Behaviour |
|---|---|
| `exit` (default) | The same graceful shutdown path as SIGTERM, but with a non-zero exit: a supervisor does not see a clean stop, restarts and alerts. No self-restart, because the state of a Tokio task is not revivable. |
| `log-only` | The trip is logged loudly on stderr and via `tracing`, the colony keeps running and the supervisor keeps supervising (counter reset). For boxes on which a trip is more likely a measurement artefact than a fault: debug builds, test suites, developer machines. It covers silence only, because a colony task that is gone ends the process here too. |

The trip line is structured (GH #84), with the prefix unchanged and the diagnosis in brackets:

```
meclaw: watchdog trip - colony heartbeat lost for 5 consecutive supervisor periods of 100 ms
  [starved=colony_loop silent_for=500ms nominal_window=500ms supervisor_lag=0ms
   in_flight_work=false work_item_budget=5000ms witness=kept witness_missed=0/5
   beats_seen=3 armed_for=801ms colony_task=alive work_item=none
   cells_at_boot=3 on_trip=exit]
```

`starved` is the diagnosis, derived from three pieces of evidence: the witness, the loop's last
declared phase and `supervisor_lag`.

| `starved` | Meaning |
|---|---|
| `colony_task_gone` | The heartbeat channel is closed, so the task is dead (a panic) rather than slow. That is a proof, not an inference. |
| `host_runtime` | The independent witness failed the same rule in the same window: a task with no relation to the colony did not get through either. The observation says something about the host and nothing about the colony. |
| `slow_work_item` | The loop had declared a work item and is still inside it, below `work_item_budget`. An operation is taking long, which is not a defect. A `/colony/*` read is such a work item and declares itself as `colony-read <endpoint>`. |
| `stuck_work_item` | The same declared work item outlived `work_item_budget` too. An operation that never returns is a wedge whatever its name. |
| `process_scheduling` | The supervisor's own periods came in at least twice as slow as configured: the whole process was off CPU, and this observation says nothing against the colony loop. |
| `colony_loop` | Every control held: the supervisor kept its schedule, the witness kept finishing work, and the loop was parked with nothing in flight, and it still went quiet. This is the only silence that implicates the colony. |

`cells_at_boot` is deliberately the boot count rather than "active cells now": the registry belongs
to the colony, and at trip time the colony by definition is not answering.

Production keeps `exit`, and `exit` now means "on a corroborated finding" (GH #165). A trip ends the
process only when the evidence actually implicates the colony loop (`colony_loop` or
`stuck_work_item`) or the task is provably gone (`colony_task_gone`). `host_runtime`,
`slow_work_item` and `process_scheduling` are logged loudly, the supervisor keeps supervising and
the process lives.

## API (HTTP)

- Implementation: a module in the colony (`meclaw-api` crate, in the binary), not a cell type.
  Colony translates HTTP requests into typed `ColonyMsg` inbox commands with a oneshot-ack reply,
  and the sequentiality of the colony loop is the symmetry guarantee.
- Stack: `axum`, Tokio-native and async. Surface: REST, with gRPC as a possible second surface
  later.
- OpenAPI spec: planned. `utoipa` is in the dependency graph, there is not a single annotation in
  the code, and nothing emits a spec document. Until that changes, the canonical `/colony/*`
  endpoint table in this document is the API description.
- Auth: none, and no TLS. meclaw knows paths and no identities; who may reach the port is the
  reverse proxy's business.
- WebSocket (`/events`): live topology events from phase 14 for visualisation tools. Active in
  daemon mode, and optional in direct mode via flag.
- Every HTTP endpoint is a thin wrapper that translates the request into a regular message and
  routes it to the appropriate authority, typically colony. Cells within a builder hive reach the
  same data via direct messages, with an identical response schema.

### Read paths

The endpoint table under § `/colony` as a virtual endpoint is the operator's index too, and no route
here is new. HTTP routes are 1:1 the internal paths, and the operator web UI renders the same data
as HTML under `/ui/*`.

Colony answers a graph query in the universal body format with a top-level slot `graph`, so
consumers read `body.graph.*`:

```json
{
  "graph": {
    "scope": "/main/router",
    "graph_version": 42,
    "nodes": [
      { "name": "...", "id": "01HXY...", "type": "...", "template_ref": "...", "path": "..." }
    ],
    "edges": [
      { "id": "01HXZ...", "from": "...", "to": "...", "condition": "...", "modifier": null, "default": false }
    ]
  }
}
```

- The slot wrapper `graph` groups the related fields under a named top-level slot, consistent with
  the universal-body discipline.
- `graph_version` is constant `0` today; the counter growing monotonically per scope, counting up on
  every successful mutation for this scope and helping with a polling diff, is planned from phase 14.
- Granularity is shallow only, one level per read. Sub-scopes are read via separate graph queries
  with their path as scope.
- The edge object carries `id`, `from`, `to`, `condition`, `modifier`, `default` and `lane`.
  `condition`, `modifier` and `lane` are optional and absent when the edge has none, because a
  missing key is the statement that this edge has no condition. `lane` (string, GH #559) carries the
  lane name a v-lane declared; an ordinary edge carries the key not at all. `default` (boolean,
  since v0.18.0, GH #367) is there **always, on both values**: it names the edge's routing phase
  (`true` for a default edge), and a phase is never absent, because every edge runs in exactly one
  of the two. Omitting the key on `false` would leave a reader unable to tell "this edge is regular"
  from "this server does not report phases", which is the ambiguity the boot checks sat in, since
  they rebuild their edge table out of this answer.
- **Edge UUIDs visible**, and the query emits them. Using them for disambiguation in `remove_edges`, with the `id` field, is *(specified, not built - see GH #254)*:
  `validate_remove_edges` (`crates/meclaw-colony/src/mutation/validate.rs`) requires `match.from`
  and `match.to`, an `id` key is read on neither path, and an `id`-only match is rejected as
  `schema`. Edge identity today is `from` plus `to` plus `condition` plus `modifier` plus `default`,
  the routing phase having joined with GH #283. `lane` is NOT an identity term (GH #564): two edges
  that differ only in the lane name are one edge as far as deduplication is concerned, which is why
  any lane deviation on an identical edge is refused rather than quietly swallowed. This holds for
  two entries of the same diff as well (GH #564): the entries are compared against each other before
  anything is applied, because the apply arm dedups against the growing edge table and would
  otherwise insert the first lane and silently drop the second. The refusal names both entry indices
  and both lanes, and two entries that declare the same lane remain idempotent.

Push versus pull: pull (`GET /colony/registry?path_prefix=...` with a `graph_version` comparison for
cache invalidation) is available from phase 12; push (`GET /colony/events`, a WebSocket subscribe)
comes from phase 14, because the event broadcast would have to be fired from the routing loop,
`handle_cell_died` and `handle_mutation`, which touches the await-free `handle_cell_died` corridor
and needs its own design pass over broadcast mechanics, a slow-consumer drop policy and an event
schema.

### HTTP endpoints

HTTP endpoints are 1:1 the `/colony/*` paths. axum takes an HTTP request, builds a message with
`target = "/colony/<endpoint>"` and sends it through the same routing path as an internal message,
so this table repeats the canonical endpoint table in HTTP-route form.

| HTTP route | Method | corresponds internally to |
|---|---|---|
| `/messages` | POST | a general message inlet: an HTTP body becomes a `Message` with an arbitrary `target` (for instance `/main/agent/llm` or `/colony/...`); axum translates and hands it to colony's routing |
| `/colony/dead_letters` | GET/DELETE | `/colony/dead_letters` (read and drain) |
| `/colony/registry` | GET | `/colony/registry` (with a filter query) |
| `/colony/templates` | GET | `/colony/templates` |
| `/colony/templates/rescan` | POST | `/colony/templates/rescan` |
| `/colony/mutations` | GET/POST | `/colony/mutations` (POST is a new mutation, GET the mutation-log audit) |
| `/colony/graph` | GET | `/colony/graph?scope=...` |
| `/colony/surfaces` | GET | the mount table of the surface cells (`?format=traefik`: as a Traefik HTTP-provider document); a read of the registry, with no routing through `route()` |
| `/colony/trace` | GET | `/colony/trace?trace_id=...&...` |
| `/colony/ledger` | GET | `/colony/ledger?since=...&...` (aggregates; an unreadable filter is a `400 bad_query` here, where the message door puts `invalid_query` into the `ledger` slot) |
| `/colony/events` | GET (WS upgrade) | `/colony/events` (subscribe) |
| `/ui/*` | GET | an HTML render layer over the same data |
| `/health` | GET | health check: always `200`, JSON with `status` and `io_liveness` (the age of each long-running cell's last successful external round trip; a short-deadline read from the colony task, `null` when the colony does not answer, with no routing through `route()`) |

`POST /messages` is the only HTTP endpoint that can inject a message with an arbitrary target.

`POST /messages` is fire-and-forget in phase 12: a response of 202 Accepted with `{message_id}`, and
any cell answer runs via the routing cascade rather than back via HTTP. A synchronous request and
response roundtrip is deferred to phase 13 and later. The JSON request body is `{target, body,
headers?, hop?, ttl?}`. The optional `ttl` field sets the TTL of the initial message (positive
integers up to `u32::MAX` only, any other value giving `422 invalid_ttl`); without the field
`colony.json` `message_default_ttl` applies. The optional `headers` field is answered the same way:
absent or `null` means no inbound headers, an object goes into the `context` compartment, and every
other JSON type gives `422 invalid_headers`. The optional `hop` field (GH #175) is the opt-in seed
for the `hop` compartment: absent or `null` means an empty hop, an object lands verbatim in the
`hop` compartment, and every other JSON type gives `422 invalid_hop`. It is deliberately not
"headers go to hop": both compartments are named separately and the substrate never infers one from
the other, because a seeded hop is the caller asserting a lane, which a hive's `{"from": "."}` doors
condition on. Its reach is a modifier's reach and not one step further, so envelope names inside a
seeded hop stay inert data and the envelope-setter authority is untouched. The `headers` object is
not size-limited (sizes are watched by a standing measurement whose last reading lives in
[#141](https://github.com/mmeyerlein/meclaw/issues/141)). The multipart path has no `hop` and no
`ttl` form field, so there the `colony.json` default always applies. Multipart is the one producer
of the `attachments[]` slot: it streams every file into the blob store and answers, next to
`message_id`, with the `BlobRef`s it created, which the consuming cell declaring
`consumes.body.attachments` then reads. Because the synthesised upload body is attachments-only, the
usual flow is two-step: upload, then send the returned `BlobRef`s together with the conversation
turns over the JSON path.

Op bodies over `POST /messages` (GH #17): `body` is validated against the schema, so a pure control
message too, a timer op or a `params` update, needs one of the three central slots. The honest one
is `"messages": []`, and the op fields themselves travel next to it as cell-specific top-level slots
(`{"messages": [], "op": "trigger", "schedule_id": "…"}`). Without a central slot the ingress
answers `422 invalid_ubf_body`. The API deliberately offers no op route and no validation bypass:
the HTTP layer checks the envelope and the cell checks the op, so every cell op surface is reachable
from outside without the API having to know cell types.

HTTP status for `/colony/mutations` POST: 200 on `Committed`, 422 Unprocessable Entity on
`Rejected`, with the full `MutationOutcome::Rejected` detail remaining in the `mutation` slot of the
body.

The error envelope has exactly three shapes, and they are frozen. A client parses one of these and
never a fourth.

| shape | when | example |
|---|---|---|
| `{"error": "<token>"}` | a refusal that needs no elaboration | `{"error": "colony unavailable"}` (503), `{"error": "unsupported_media_type"}` (415) |
| `{"error": "<token>", "detail": "<free text>"}` | a refusal whose reason is specific to the request | `{"error": "bad_query", "detail": "trace_id is not a valid UUID"}` (400) |
| `{"mutation": {…}}` | every `/colony/mutations` POST, committed or rejected | `{"mutation": {"outcome": "rejected", "error_code": "template_missing", …}}` (422) |

The `error` token is machine-readable and stable; `detail` is free text for a human and may be
reworded at any time, so never match on it. A mutation reject does not use the error envelope: it is
a well-formed outcome of a well-formed request, so it comes back in the `mutation` slot with the
full detail intact.

A mutation reject is 422, never 400. That distinction carries meaning and is frozen: `400` means the
HTTP layer could not read the request (bad JSON, a malformed query parameter), while `422` means the
request was read and understood and the substrate refused what it asked for (a rejected diff, an
invalid body, an invalid TTL).

There is no separate upload endpoint for templates. A template reaches a colony through the
filesystem (`--templates`) or, since GH #440, as an `add_templates` entry of a mutation diff, which
writes it into the instance-local library under the no-delete rule (see § Mutation operations).
This corrects an older version of this paragraph that said templates never arrive over the API.

## Persistence

- Per cell: `cell.db` (SQLite) in the cell directory for dynamic state, cell authority. It holds no
  param or config history table: `CELL_DB_DDL` carries only `system`, `last_input` and `meta`, and
  `last_input` is forensics rather than history.
- `colony.db`: the central database with the registry (path to cell ID plus status plus template
  plus dormancy marker, GH #491), the templates registry, the mutation log, the central message log
  and the edge table. Colony writes; cells do not read directly.
- The `status` column is the activity the next boot reads (GH #495). The boot recompute runs only
  for nodes without a registry row, and a rehydrated node takes its activity from this column. So
  every registration writes down the activity it registers, and every recompute writes down every
  flip; otherwise a derived inactivity does not survive the process that computed it (ADR-0018).
  `failed` is a third value of the same column and is never overwritten by `inactive`, and the
  dormancy marker beside it answers the orthogonal question of whether the sleep was declared
  (ADR-0017).
- Trace reconstruction via `parent_message_id` chaining in the central message log: a flat `SELECT`,
  with the parent-child tree built client-side or UI-side (index `idx_msglog_parent`).
- Blobs separately as JSON files.
- An operations log at `{root}/log.jsonl`.

## Logging

The default is `{root}/log.jsonl` (JSON Lines, append-only, created by colony at start if not
present). The engine is `tracing` plus `tracing-subscriber` with a JSON formatter, and every
subsystem (colony routing, cell tasks, HTTP API) writes into the same stream. Overrides are the CLI
flags `--log <path>`, `--log-level <level>` (default `info`) and `--log-filter <expr>`, a per-module
filter such as `meclaw_core=debug,meclaw_colony=info`. Rotation is an operations matter via external
`logrotate` or similar.

```json
{"ts":"2026-05-17T14:32:15.123Z","level":"error","event":"mutation_failed","error_code":"template_missing","scope":"/main","mutation_id":"01HXY...","correlation_id":"01HXZ...","details":{"template":"llm-anthropic@2.1.0"}}
```

Three logs coexist and complement each other.

| Log | Path | Purpose |
|---|---|---|
| Operations log | `{root}/log.jsonl` | the tracing stream of all subsystems, for operator and debug use, grep- and jq-friendly |
| Mutation log | a table in `colony.db` | a structured audit trail of only the mutations, queryable via the API `GET /mutations` |
| Message log | a table in `colony.db` | every routed message with `trace_id`, `parent_message_id`, `from_path` and `to_path`, filterable by path prefix for scoped tracing |

Tracing and metrics are not core architecture. The `tracing` crate is OTel-bridgeable via
`tracing-opentelemetry`, and metrics exposure is solvable via external tools, sidecars or API
extensions. No crate or endpoint is provided in the core stack.

## Builder pattern

- Every cell, or an external API client, may send mutation messages to `/colony/mutations` (format
  and validation: § Mutation format).
- A builder hive is a hive scope, not a single actor. It bundles several specialised cells under a
  path prefix, typically an `llm` cell for natural-language request understanding and diff
  generation, a `code` cell for mutation diff construction and validation, optionally a `code` cell
  for template-discovery aggregation, and a collector or memory hive for multi-step builder
  conversations. The final mutation diff is emitted by the outermost cell of the builder hive, or by
  a dedicated output hive, to `/colony/mutations`. A hive, because the builder task is multi-stage
  and each stage benefits from its own cell with a clear contract. The shipped representative is
  `templates/builder/`, which drafts a manifest, together with `templates/submit/`, which hands it
  in.
- Cells are never deleted. They become inactive through edge withdrawal and can be reactivated via
  `add_edges` or a renewed `add_nodes` at the same path; `swap_nodes` swings the external edges onto
  another implementation for a template upgrade.
- `/colony/mutations` answers every mutation via `build_mutation_reply`
  (`crates/meclaw-colony/src/colony_dispatch.rs`) to `reply_to`. Without a set `reply_to` only the
  logging remains, and the receipt (GH #553), which is how `POST /colony/mutations` and `meclaw
  --apply` become observable at all (§ The mutation flow).

## DSL

An own meclaw schema, JSON-only, optimised for the agent-first architecture, validated against JSON
Schema Draft 2020-12. Adopted independent standards are CEL for edge expressions, SemVer for
template versions, and HTTP/OpenAPI conventions for auth, retry and timeout.

## Tech stack

| Area | Choice |
|---|---|
| Language | Rust (edition 2024, `rust-toolchain.toml` with an exact version pin, so rustup fetches the pinned toolchain and workstation and CI build with the same one; raising it is a commit of its own that handles the new lints in the same move, GH #406) |
| Workspace resolver | `resolver = "3"` in the workspace `Cargo.toml` (the default for edition 2024, but set explicitly in the workspace manifest) |
| Async runtime | `tokio` (multi-thread flavor, work-stealing scheduler) |
| Async observability | `console-subscriber` for the `tokio-console` bridge; activated via `--cfg tokio_unstable` in `.cargo/config.toml` |
| CLI | `clap` |
| Logging | `tracing` plus `tracing-subscriber` |
| Non-blocking log writer | `tracing-appender` (a writer wrapper with a `WorkerGuard` for flush; it complements `tracing-subscriber`'s synchronous writer once async cells log) |
| Serialization | `serde`, `serde_json` |
| DB | `rusqlite` (decided in phase 5; `sqlx` rejected; `rusqlite="0.39"` in four crates; since P4 with the `functions` feature in `meclaw-cells` for registered scalar functions like `hamming()`) |
| Graph (data structure) | `petgraph` |
| Edge expressions | `cel` (crate; GitHub project `cel-rust`) |
| HTTP API | `axum` |
| HTTP client | `reqwest` with the `rustls` feature (async, hyper-based, native Tokio runtime usage, a static binary possible) |
| HTML templating (operator web UI) | `maud` (inline HTML in Rust macros, no external template directory) |
| OpenAPI generation (planned; dependency wired, no annotations yet) | `utoipa` |
| UUID | `uuid` with the `v7` feature |
| Cron parser | `croner` (6-field Quartz style with seconds, `find_next_occurrence`; used only as a parser, not as a scheduler crate) |
| Date/time | `chrono` (a foreign dep of `croner`, and at the same time the source for UTC ISO-8601 timestamps; `chrono-tz` and local time zones deferred) |
| Errors | `thiserror` (library errors) plus `anyhow` (binary errors) |
| JSON schema | `jsonschema` (Draft 2020-12) |
| Test tmp directories (dev-deps) | `tempfile` |
| File watcher | not in scope |
| Process-group signals | `libc` 0.2, only `killpg`, `SIGTERM`, `SIGKILL` and `pid_t`, unix-only, one module (sanctioned 2026-08-08) |
| Crypto (vault) | RustCrypto, one choice per job: `argon2` (argon2id, passphrase to key), `chacha20poly1305` (XChaCha20-Poly1305, AEAD at rest), `hmac` plus `sha2` (HMAC-SHA256, what `vault.use` does with a secret instead of handing it out), `getrandom` (nonces and salt), `subtle` (constant-time comparison). Sanctioned 2026-08-16 |
| Key agreement | `x25519-dalek`, for the vault's sealed-box delivery (R3): the recipient names an ephemeral public key and the vault seals against it. Exactly one group, no curve choice at config level. The box key falls out of the row above via HMAC-SHA256; `hkdf` and `crypto_box` were deliberately left out. Sanctioned 2026-08-26 |

## Repo structure and tests

The crate layout is `meclaw-core` (actor trait, `ActorHandle`, `Message`, path resolution, CEL
wrapper), `meclaw-colony` (colony task, registry, lifecycle, templates, routing, mutations),
`meclaw-cells` (built-in cell types), `meclaw-api` (the axum HTTP API), `meclaw-cli` (binary, clap,
daemon, stdin/stdout bridge) and `meclaw-testing` (test fixtures). The dependency direction is
`meclaw-colony` and `meclaw-cells` on `meclaw-core`, `meclaw-api` on `meclaw-colony`, and
`meclaw-cli` on all three. `meclaw-testing` is always a `[dev-dependencies]` entry and never a
runtime dependency. It provides `TestRoot` (a RAII wrapper around a tmp directory), `ColonyHandle`
(an async wrapper around a running colony), `MessageBuilder`, the mock cells under
`meclaw_testing::mocks` and the per-phase topology fixtures. `TestRoot` is the only permitted
exception to the no-delete policy, because tmp paths are not part of the real live tree. Every test
that boots a real topology declares `#[tokio::test(flavor = "multi_thread", worker_threads = 4)]`;
the `current_thread` flavor is permitted only in pure unit tests without a topology, and there is no
`block_on` anywhere in test or helper code. The directory tree of the repository is in
[`README.md`](../README.md), and the contributor rules carry the test tiers and gates.

The phase roadmap that used to stand here has moved: what has shipped is in `CHANGELOG.md`, and what
is planned in `ROADMAP.md`. Phase numbers still appear in this document where a contract is dated by
one.

## What meclaw will not do

- No distributed cluster setup. If it is ever needed, NATS as a transport underneath and
  cross-colony federation as an additive extension.
- No GUI and no editor. VSCode plus the filesystem suffice, and visualisation runs via the API from
  external tools.
- No covering of non-agent workflows.
- No own LLM inference. Cells call external providers.
- No cell-to-cell topology knowledge. Cells stay dumb.
- No compliance with foreign workflow standards. An own schema, JSON-first.

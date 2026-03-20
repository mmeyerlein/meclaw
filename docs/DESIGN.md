# MeClaw — Design Principles
As of: 2026-03-20 — binding, non-negotiable

---

## 0. Fundamental Rules

### Everything in PostgreSQL
- No external process. No exceptions. No "small helper script".
- Allowed: all active PostgreSQL extensions (currently PG17)
- Preference: PostgreSQL built-ins first — extensions only when necessary
- pg_background for batch isolation (separate transactions)

### Event-Sourcing — absolute
- The core is built exclusively on events
- No polling at the SQL/agent level. Ever.
- Extensions may use OS-level I/O events internally (epoll, kqueue) — that's not polling
- Smart event-based: Trigger → Response → Trigger

### Messages as Foundation
- Everything is based on messages
- Messages are hierarchical:
  ```
  workspace / group
    └── channel
          └── message (e.g. Telegram → response)
                └── sub-messages (tool calls, LLM calls etc.)
  ```
- **Append-only**: Messages are NEVER updated, only new ones created
- **Auditable**: complete history always reconstructable
- **Status as DB column**: current state lives NOT in JSON payload, but as a separate column — queryable, trigger-capable
- **Channel-scoped**: every message belongs to exactly one channel

### Channels as Universal Primitive
- **Everything flows through channels** — external (Telegram, Slack), internal (messages table + triggers), tool calls
- A channel is an **append-only message stream** with a **shared extraction cache**
- A message is atomic and belongs to exactly one channel
- Channels have **no intelligence** — they are passive streams with an extraction layer
- Entities/events are extracted **once per channel**, not per agent
- Channel-level extraction is shared; agent-level ranking is personal

### Message States (OS Metaphor)
Messages are like tasks in an operating system:

| Status | Meaning | Required field |
|--------|---------|----------------|
| `ready` | Ready, waiting for assignment | — |
| `running` | Currently being processed | `assigned_to` (agent name) |
| `waiting` | Waiting for external result | `waiting_for` (e.g. `llm_response`, `tool_result`) |
| `done` | Complete, response sent | — |
| `failed` | Error | `error_message` |
| `blocked` | Blocked by dependency | `blocked_by` (message ID) |

### Complete Logging — absolute
- **Every event** in the system is logged. No exceptions.
- **Every message** stays in the system. Never delete.
- The system is fully reconstructable at any point in time.
- Combined with append-only: the system is a complete audit log of itself.

### No Agent Loops
- Agents do **not** loop themselves
- One agent: 1 input → 1 output → done
- Loop control is handled by the graph routing
- An agent never decides on its own whether it runs again

### Message Flow is Core
- Routing and message flow are core functionality — hardwired
- Routing intelligence (graph routing / swarming) is **outside** the core
  → can be swapped or adjusted at any time
- Current preference: AGE graph with Cypher queries

---

## Why epoll is Not Polling

pg_net uses **epoll** (Linux kernel):
- pg_net registers HTTP sockets with the Linux kernel
- Kernel monitors sockets passively for network activity
- When data arrives → kernel interrupt → pg_net worker wakes immediately
- No active querying — the worker sleeps until the kernel wakes it

This complies with our event-sourcing principles. ✅

---

## 1. Input — How Do Messages Arrive?

### Tool of Choice: pg_net
- Event-driven (epoll), non-blocking, C extension
- Used for both input (long-poll) and output (LLM calls, sends)
- PG18-compatible, actively maintained (Supabase)

### Long-Poll — not a Webhook, not a Hook
- "Long-poll" sounds like polling — it's not. It's an open HTTP connection that blocks until data arrives. Event-driven at the network level (epoll).
- **No webhooks for external services.** Ever. A webhook requires a public HTTP endpoint → external process → violates Rule 0.
- MeClaw reaches out, it doesn't get reached. We open the connection, not the external service.

### Principle: Self-Sustaining
- Response trigger starts a new connection immediately after each response
- No watchdog needed in the happy path (watchdog only as fallback on crash)

---

## pg_net Connection Lifecycle

### Telegram Long-Poll
```
net.http_get(getUpdates?timeout=25)
  → curl holds connection open
  → message arrives → immediate response → trigger → process → new http_get()
  → 25s timeout → empty response → trigger → immediate new http_get()
```
The response trigger is self-sustaining — it always starts a new connection.

### LLM Call
```
net.http_post(LLM/chat/completions, timeout=120s)
  → curl waits
  → LLM responds → immediate response → trigger → message done
  → no loop — LLM call is one-shot per message
```

**Timeout:** We set `timeout_milliseconds` in `net.http_post()` to 120s. On timeout: message → `failed`.

---

## pg_background for Batch Isolation

pg_net processes HTTP requests in batches. All requests landing in the same commit → same batch → block each other. Long-poll (25s) would block LLM calls.

**Solution:** `pg_background_launch()` starts a separate transaction → separate commit → separate pg_net batch. Used for:
- LLM calls (`llm_http_call_by_id` via `llm_jobs` staging table)
- Poll restart (`channel_bee_start` after receiving a message)

**Critical:** `pg_background.max_workers = 256` in postgresql.conf (default 16 is insufficient).

---

## 2. Agent Model

### Agent = Multi-Hive Root

An agent is the root of a hierarchical Hive tree:

- **Agent = AIEOS Identity + Brain + Channels + Multi-Hive**
- An agent owns at least one Hive, but can own multiple
- There are **no orphan Hives** — every Hive belongs to exactly one agent
- The agent boundary is the primary security boundary

### System Agent

The System Agent is the first agent that exists:

- Created during bootstrap, before any other agent
- Owns all infrastructure Hives: Router, Channel IO, Admin, Logging
- Has no user-facing personality — pure infrastructure
- Manages channel lifecycle, agent registration, system health

### Agent Types

| Type | Purpose | Example |
|------|---------|---------|
| `system` | Infrastructure management | System Agent (exactly one) |
| `agent` | User-facing AI assistant | Walter, Support Bot |
| `workspace` | Institutional memory container | gisela-workspace |

---

## 3. Channel Model

### Channels as Universal Primitive

A channel is the fundamental communication unit in MeClaw:

- **External channels:** Telegram, Slack, Web — bidirectional user communication
- **Internal channels:** messages table + triggers — inter-bee, inter-agent communication
- **Tool channels:** tool calls and results flow through channels

### Channel Properties

1. **Append-only stream** — messages are never mutated or deleted
2. **Shared extraction cache** — entities/events extracted once per channel
3. **No intelligence** — channels don't think; intelligence lives in agents
4. **Multi-subscriber** — multiple agents can subscribe to the same channel
5. **Scoped access** — agents must explicitly subscribe to a channel

### Extract Once, Rank Many

When a message arrives in a channel, extraction happens **once** at the channel level. Each subscribed agent then applies its own personal ranking (rewards, novelty, personality-fit) on top of the shared extraction. No duplication.

---

## 4. Entity Model

### Everything is an AIEOS Entity

All first-class objects in MeClaw are entities with AIEOS-compatible identity:

| Entity Type | AIEOS Identity | Brain | Channels |
|-------------|---------------|-------|----------|
| **Person (User)** | neural_matrix (observed), OCEAN (observed), explicit + observed profiles | — | Participates in channels |
| **Agent** | neural_matrix (defined), OCEAN, moral_compass, linguistics, capabilities | ✅ Personal | Subscribes to channels |
| **Workspace** | neural_matrix (institutional), capabilities | ✅ Institutional | Owns workspace channels |
| **Project** | Minimal (tags, scope) | — | Scope/tag within workspace |
| **Tool** | capabilities | — | Tool channels |
| **Concept** | embedding | — | — |

### Users Are Entities, Not a Separate Concept

There is no "user" table. A user is an entity with `type: person`:

- Carries AIEOS-compatible identity (partially observed, partially explicit)
- **explicit_profile:** self-reported data ("I live in Berlin")
- **observed_profile:** agent-learned data (communication style, preferences)
- Entity observations track agent's learnings about the user over time
- consolidation_bee merges observations nightly

### Workspaces Are Agents

A workspace is an agent with `type: workspace`:

- Has its own brain (institutional memory)
- Has its own channels (workspace-wide communication)
- Has AIEOS identity (institutional personality/culture)
- Projects are scopes/tags within the workspace — not separate agents

---

## 5. Knowledge Graph vs Execution Graph

MeClaw maintains two strictly separated graph types:

### Knowledge Graph (Persistent)

- **Append-only**, versioned via sequence numbers
- Contains: entities, events, relations, prototypes, associations
- Stored in AGE (`meclaw_graph`)
- Shared extraction layer (channel-level) + personal overlays (agent-level rewards)
- Volatile but reconstructable — Layer 1 can be rebuilt from Layer 0

### Execution Graph (Ephemeral)

- **Per-request**, built by the router/planner
- Contains: bee execution order, hive call stack, routing decisions
- Lives only for the duration of a single request
- Built from Hive definitions in AGE, but the instance is transient
- Discarded after request completion

**Rule:** Never mix persistent knowledge with ephemeral execution state. The knowledge graph accumulates learning. The execution graph is disposable infrastructure.

---

## 6. Storage Hierarchy

Five layers, from raw to abstract:

| Layer | Content | Scope | Mutability |
|-------|---------|-------|------------|
| **0** | Raw Messages | Channel (append-only) | Immutable |
| **1** | Extracted Entities, Events, Relations | Channel (shared) | Append-only, versioned via seq |
| **2** | Rewards, Novelty Scores | Agent (personal) | Mutable |
| **3** | Prototypes, Associations | Agent (personal) | Mutable |
| **4** | Decision Traces | Agent (personal) | Immutable |

### Properties

- **Layers 0-1 are shared:** multiple agents read the same raw data and extractions
- **Layers 2-4 are personal:** each agent has its own learned overlays
- **Layer 1 is recoverable:** can be rebuilt from Layer 0 by re-running extract_bee
- **Layers 2-3 are volatile but valuable:** learned knowledge, rebuildable but time-consuming

---

## 7. Security & Isolation

### Agent Boundary = Security Boundary

The agent is the primary security boundary in MeClaw:

| Boundary | Trust Level | Mechanism |
|----------|------------|-----------|
| **Within Agent** | Full trust | All hives share brain, graph, memory |
| **Between Agents (same instance)** | Controlled sharing | Channels with explicit subscriptions, agent-scoped brain tables |
| **Between Agents (different containers)** | No shared state | Network only (pg_net) |
| **Between Workspaces** | Complete isolation | Separate PostgreSQL instances |

### Scoping Model

Data access is controlled by three scope levels:

| Scope | Visibility | Use Case |
|-------|-----------|----------|
| **private** | Only the agent's own channels | Personal conversations, drafts |
| **shared** | All channels the agent subscribes to | Multi-agent collaboration |
| **workspace** | Institutional knowledge across workspace | Organizational memory, policies |

### Container Isolation (Deployment Level)

For maximum isolation: one agent per container.

```
Container A: Agent "walter"
  └─ PostgreSQL
     └─ MeClaw (receiver, llm, memory, tool bees)

Container B: Agent "support"
  └─ PostgreSQL
     └─ MeClaw (receiver, llm, escalation bees)
```

**Within a container:** Full trust. All bees share DB, graph, messages, memory.
**Between containers:** Network boundary. No access. Period.

### Why No Internal RBAC/Capabilities

- Bees are functions, not users. They run as `postgres`.
- plpython3u is "untrusted" — full OS access. This is intentional.
- SQL-level sandboxing would be theater. A `DO $$ import os; os.system('...') $$` bypasses any SQL restriction.
- The container is the real boundary: resource limits, network policy, ephemeral filesystem.
- Agent-level scoping handles data isolation; container-level handles code isolation.

### plpython3u = The Agent's Hands

- `subprocess.run()` — shell commands on the container host
- `open()` — read/write filesystem
- `import requests` — HTTP requests (parallel to pg_net)
- Generate and execute arbitrary Python code at runtime

An agent with plpython3u doesn't just have hands — it has a **3D printer for hands**. It can build any tool at runtime. Inside the database. No deployment needed.

### Multi-Tenant = Multi-Container

- No shared-DB multi-tenancy for untrusted tenants
- One agent (or trusted agent group) per container → maximum isolation
- Scaling = more containers, not more schemas
- Communication between containers: explicitly via network (pg_net), not via shared state

---

## 8. Scaling & High Availability

### HA: Streaming Replication + Watchdog

```
Primary (active)               Standby (hot)
├── MeClaw Agents (active)    ├── MeClaw Agents (passive)
├── pg_net ✓                   ├── pg_net ✗ (sleeping)
├── pg_cron ✓                  ├── pg_cron ✗
├── Triggers ✓                 ├── Triggers ✗
└── WAL Stream ────────────────└── receives WAL
```

- Failover via Patroni/etcd
- After promotion: pg_cron watchdog detects "no poll active" → starts poll → agents live
- Max 1 minute downtime (watchdog interval)

### Horizontal Scaling = Multi-Agent, Not Multi-Node

A single agent scales **vertically**. PostgreSQL on one server handles thousands of messages/s. An agent with 50 bees and 100 parallel tasks is no problem.

**Why no agent across multiple nodes:**
- pg_net, pg_cron, pg_background, triggers — only run on primary
- Agent is write-heavy (every bee = INSERT) → no multi-primary
- Distributed locks for trigger chains = unnecessary complexity

**Horizontal scaling = more agents, more containers:**
```
Kubernetes Cluster
├── Container: walter-agent (PG + MeClaw)
├── Container: support-agent (PG + MeClaw)
├── Container: analytics-agent (PG + MeClaw)
└── Container: walter-standby (HA replica)
```

More load = more agents = more containers. Kubernetes distributes.

### Read Scaling

What can run on replicas:
- **pgvector search** — memory/RAG is read-heavy
- **Analytics** — event stream via logical replication
- **Monitoring** — dashboards on standby instead of loading primary

### Communication Between Agents (Cross-Container)

- Via **pg_net** (HTTP) — no shared state, no distributed locks
- Agent A sends request to Agent B → Agent B responds → done
- No shared filesystem, no shared database

### Summary

| Level | Strategy |
|---|---|
| **HA** | Streaming Replication + Patroni + Watchdog |
| **Read scaling** | Replicas for pgvector/analytics |
| **Write scaling** | Vertical (sufficient for a single agent) |
| **Horizontal scaling** | Multi-Agent = Multi-Container |

---

*This document is binding. Changes only by explicit decision.*

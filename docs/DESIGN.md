# MeClaw — Design Principles
As of: 2026-03-17 — binding, non-negotiable

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
    └── conversation
          └── message (e.g. Telegram → response)
                └── sub-messages (tool calls, LLM calls etc.)
  ```
- **Append-only**: Messages are NEVER updated, only new ones created
- **Auditable**: complete history always reconstructable
- **Status as DB column**: current state lives NOT in JSON payload, but as a separate column — queryable, trigger-capable

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

## 2. Security & Isolation

### One Hive = One Container = One PostgreSQL Instance

MeClaw needs no internal security model. Isolation comes from the container.

```
Container A: Hive "support"
  └─ PostgreSQL
     └─ MeClaw (receiver, llm, memory, escalation bees)

Container B: Hive "assistant"
  └─ PostgreSQL
     └─ MeClaw (receiver, llm, tool bees)
```

**Within a Hive:** Full trust. All bees share DB, graph, messages, memory.
**Between Hives:** Container boundary. No access. Period.

### Why No Internal RBAC/Capabilities

- Bees are functions, not users. They run as `postgres`.
- plpython3u is "untrusted" — full OS access. This is intentional.
- SQL-level sandboxing would be theater. A `DO $$ import os; os.system('...') $$` bypasses any SQL restriction.
- The container is the real boundary: resource limits, network policy, ephemeral filesystem.

### plpython3u = The Agent's Hands

- `subprocess.run()` — shell commands on the container host
- `open()` — read/write filesystem
- `import requests` — HTTP requests (parallel to pg_net)
- Generate and execute arbitrary Python code at runtime

An agent with plpython3u doesn't just have hands — it has a **3D printer for hands**. It can build any tool at runtime. Inside the database. No deployment needed.

### Multi-Tenant = Multi-Container

- No shared-DB multi-tenancy
- One Hive per container → maximum isolation
- Scaling = more containers, not more schemas
- Communication between Hives: explicitly via network (pg_net), not via shared state

---

## 3. Scaling & High Availability

### HA: Streaming Replication + Watchdog

```
Primary (active)               Standby (hot)
├── MeClaw Hive (active)      ├── MeClaw Hive (passive)
├── pg_net ✓                   ├── pg_net ✗ (sleeping)
├── pg_cron ✓                  ├── pg_cron ✗
├── Triggers ✓                 ├── Triggers ✗
└── WAL Stream ────────────────└── receives WAL
```

- Failover via Patroni/etcd
- After promotion: pg_cron watchdog detects "no poll active" → starts poll → Hive lives
- Max 1 minute downtime (watchdog interval)

### Horizontal Scaling = Multi-Hive, Not Multi-Node

A single Hive scales **vertically**. PostgreSQL on one server handles thousands of messages/s. A Hive with 50 bees and 100 parallel tasks is no problem.

**Why no Hive across multiple nodes:**
- pg_net, pg_cron, pg_background, triggers — only run on primary
- Hive is write-heavy (every bee = INSERT) → no multi-primary
- Distributed locks for trigger chains = unnecessary complexity

**Horizontal scaling = more Hives, more containers:**
```
Kubernetes Cluster
├── Container: support-hive (PG + MeClaw)
├── Container: assistant-hive (PG + MeClaw)
├── Container: analytics-hive (PG + MeClaw)
└── Container: support-hive-standby (HA replica)
```

More load = more Hives = more containers. Kubernetes distributes.

### Read Scaling

What can run on replicas:
- **pgvector search** — memory/RAG is read-heavy
- **Analytics** — event stream via logical replication
- **Monitoring** — dashboards on standby instead of loading primary

### Communication Between Hives

- Via **pg_net** (HTTP) — no shared state, no distributed locks
- Hive A sends request to Hive B → Hive B responds → done
- No shared filesystem, no shared database

### Summary

| Level | Strategy |
|---|---|
| **HA** | Streaming Replication + Patroni + Watchdog |
| **Read scaling** | Replicas for pgvector/analytics |
| **Write scaling** | Vertical (sufficient for a single Hive) |
| **Horizontal scaling** | Multi-Hive = Multi-Container |

---

*This document is binding. Changes only by explicit decision.*

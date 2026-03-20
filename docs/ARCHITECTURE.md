# MeClaw Architecture

> Technical reference for the core architecture. For a high-level overview, see the [README](../README.md).

---

## Bee Model

### Core Idea: Flow-Based Programming

An agent is not a monolithic program. An agent is a **graph**.
Nodes are **Bees** — small, specialized workers.
Edges are **message flows** between Bees.

### What is a Bee?

- Receives exactly **one message** (input)
- Does exactly **one thing**
- Produces exactly **one message** (output)
- Knows neither the previous nor the next step
- Is stateless — state lives in the message and in the graph

### Bee Types

| Bee | Purpose | Implementation |
|-----|---------|----------------|
| `receiver_bee` | Start Telegram long-poll | `channel_bee_start()` via pg_net |
| `router_bee` | AGE graph query → set next_bee | `router_bee()` — only Bee that knows AGE |
| `context_bee` | Load conversation history + compress static prefix | `context_bee()` — tier-aware (5/15/30/50 msgs) |
| `call_bee` | Call a Hive (stack push/pop) | `call_bee()` + `do_return()` |
| `llm_bee` | LLM call via pg_background → pg_net | `llm_bee_v2()` + `llm_http_call_by_id()` |
| `tool_bee` | Execute tools from registry | `tool_bee()` — generic dispatcher |
| `sender_bee` | Telegram sendMessage | `sender_bee_v2()` |
| `extract_bee` | Extract entities/events/relations from channel | `extract_bee()` — channel-level, shared |
| `novelty_bee` | Compute novelty score for events | `novelty_bee()` — agent-level |
| `feedback_bee` | Retroactive reward from user reactions | `feedback_bee()` — agent-level |
| `retrieve_bee` | CTM-style memory retrieval | `retrieve_bee()` — agent-level |
| `consolidation_bee` | Nightly pruning, merging, mitosis | `consolidation_bee()` — pg_cron, agent-level |

---

## Agent Model

### Agent = Multi-Hive Root

An agent is the root of a hierarchical Hive tree. Every agent has at least one Hive, but can own multiple.

```
Agent (AIEOS Identity + Brain + Channels)
├── Hive "conversation" (receiver → context → llm → sender)
├── Hive "memory" (extract → novelty → feedback)
└── Hive "tools" (tool dispatcher → tool executors)
```

**Key changes from earlier model:**
- An agent is NOT a single Hive — it is the root that owns one or more Hives
- There are NO orphan Hives — every Hive belongs to exactly one agent
- An agent carries AIEOS identity, brain (personal memory), and channel subscriptions

### Agent = AIEOS Identity + Brain + Channels + Multi-Hive

| Component | What it is | Scope |
|-----------|-----------|-------|
| **AIEOS Identity** | neural_matrix, OCEAN traits, moral_compass, linguistics, capabilities | Defines the agent's personality |
| **Brain** | Personal rewards, novelty scores, prototypes, associations, decision traces | Agent-level, personal |
| **Channels** | Subscribed message streams (external + internal) | Shared extraction, personal ranking |
| **Hives** | Execution graphs — bee compositions for different tasks | Agent-owned |

### Hive Definition in AGE

```cypher
(:Agent {id: 'walter', type: 'agent'})
  -[:OWNS]-> (:Hive {id: 'walter-main'})
  -[:ENTRY]-> (:Bee {id: 'walter-receiver-bee', type: 'receiver_bee'})

(:Bee {id: 'walter-receiver-bee'}) -[:NEXT {condition: 'on_message'}]-> (:Bee {id: 'walter-call-bee'})
(:Bee {id: 'walter-call-bee'})     -[:NEXT {condition: 'on_return'}]->  (:Bee {id: 'walter-sender-bee'})

(:Agent {id: 'walter'})
  -[:OWNS]-> (:Hive {id: 'walter-memory'})
  -[:ENTRY]-> (:Bee {id: 'walter-extract-bee', type: 'extract_bee'})
```

Defining a new agent = inserting an Agent node + Hive nodes + Bee nodes + Edges in AGE. No code deployment.

### System Agent

The System Agent is a special agent that:

- **Is the first agent created** — exists before any other agent
- **Owns all infrastructure Hives:** Router, Channel IO, Admin Dashboard, Logging
- **Has no user-facing personality** — it is pure infrastructure
- **Manages:** channel lifecycle, agent registration, system health, rate limiting

```cypher
(:Agent {id: 'system', type: 'system'})
  -[:OWNS]-> (:Hive {id: 'system-router'})
  -[:OWNS]-> (:Hive {id: 'system-channel-io'})
  -[:OWNS]-> (:Hive {id: 'system-admin'})
  -[:OWNS]-> (:Hive {id: 'system-logging'})
```

The System Agent bootstraps the entire system. When MeClaw starts fresh, the System Agent is created first, then it provisions other agents.

---

## Channel Architecture

### Channels as Universal Primitive

Everything flows through channels. A channel is MeClaw's fundamental communication primitive.

| Channel Type | Examples | Direction |
|-------------|----------|-----------|
| **External** | Telegram, Slack, Web | User ↔ Agent |
| **Internal** | messages table + triggers | Bee ↔ Bee, Agent ↔ Agent |
| **Tool** | Tool calls and results | Agent ↔ Tool |

### Channel Properties

- **Append-only message stream** — messages are atomic and belong to exactly one channel
- **Shared extraction cache** — entities/events extracted once per channel, not per agent
- **No intelligence** — a channel does not think, decide, or route
- **Identity-free** — a channel has no AIEOS identity (unlike agents and workspaces)

### Channel Schema

```sql
CREATE TABLE meclaw.channels (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    channel_type TEXT NOT NULL,        -- 'telegram', 'slack', 'web', 'internal', 'tool'
    external_id TEXT,                  -- platform-specific ID (e.g., Telegram chat_id)
    config JSONB DEFAULT '{}',         -- platform-specific configuration
    extraction_status TEXT DEFAULT 'idle', -- 'idle', 'running', 'error'
    last_extracted_seq BIGINT DEFAULT 0,  -- up to which message seq extraction has run
    created_at TIMESTAMPTZ DEFAULT clock_timestamp(),
    updated_at TIMESTAMPTZ DEFAULT clock_timestamp()
);
```

### Agent-Channel Relationship

Agents subscribe to channels. Multiple agents can share a channel.

```sql
CREATE TABLE meclaw.agent_channels (
    agent_id TEXT REFERENCES meclaw.entities(id),
    channel_id UUID REFERENCES meclaw.channels(id),
    role TEXT DEFAULT 'participant',    -- 'owner', 'participant', 'observer'
    scope TEXT DEFAULT 'private',       -- 'private', 'shared', 'workspace'
    subscribed_at TIMESTAMPTZ DEFAULT clock_timestamp(),
    PRIMARY KEY (agent_id, channel_id)
);
```

### Channel-Level Extraction

```
Message arrives in Channel
      ↓
  extract_bee (channel-level, shared)
    → Entities, Events, Relations written to AGE graph
    → extraction is IDEMPOTENT — same message never extracted twice
    → channel.last_extracted_seq updated
      ↓
  Agent A reads shared extraction + applies personal rewards/novelty
  Agent B reads shared extraction + applies personal rewards/novelty
```

---

## Message Model

### Principle

- Every interaction = a message in `meclaw.messages`
- Messages are **append-only** — never updated (only `status`, `assigned_to`, `waiting_for` change)
- Every message belongs to exactly **one channel**
- Every state transition is logged as an event
- `clock_timestamp()` everywhere — never `now()`
- Extraction happens at the **channel level** — entities/events from messages are extracted once and shared

### Message Schema

```sql
id           UUID PRIMARY KEY DEFAULT gen_random_uuid()
seq          BIGINT GENERATED ALWAYS AS IDENTITY  -- global ordering
task_id      UUID REFERENCES meclaw.tasks(id)
channel_id   UUID REFERENCES meclaw.channels(id)
previous_id  UUID                      -- chain link
type         TEXT NOT NULL              -- see types below
sender       TEXT NOT NULL              -- which bee/user/agent
status       TEXT DEFAULT 'ready'       -- ready|running|waiting|done|failed
assigned_to  TEXT                       -- when running: which bee
waiting_for  TEXT                       -- when waiting: what for
next_bee     TEXT                       -- when ready: which bee takes over
content      JSONB NOT NULL DEFAULT '{}'
created_at   TIMESTAMPTZ DEFAULT clock_timestamp()
```

### Message Types

| Type | Description |
|------|-------------|
| `user_input` | User message (Telegram, Web) |
| `routing` | Router decision (next_bee set) |
| `llm_request` | LLM call initiated |
| `llm_result` | LLM response received |
| `tool_call` | LLM requests a tool call |
| `tool_result` | Tool execution result |
| `send_result` | Message delivery confirmation |
| `system` | System message |

### Status Transitions

```
ready → running → done
ready → running → waiting → done
ready → running → failed
```

Every transition is auto-logged via `trg_auto_log_message`.

---

## Routing Model

### Principle: Trigger-Based Routing

Every message change fires a trigger. The trigger dispatches to the next Bee.

```
INSERT message (status=done, type=user_input)
  → trg_on_message_done_dispatch
    → router_bee: Cypher query → finds next bee
      → INSERT routing message (status=ready, next_bee='context-bee')
        → trg_on_message_ready_dispatch
          → context_bee(msg_id)
```

### Condition Mapping

| Message Type | Edge Condition |
|---|---|
| `user_input` | `on_message` |
| `routing` (from context) | `on_message` |
| `llm_result` | `on_return` |
| `tool_call` | `on_tool_call` |
| `tool_result` | `on_tool_result` |

### Stack-Based Cross-Hive Calls

```
main-hive: receiver → call_bee (push "memory-hive") → ...
memory-hive: extract → novelty → feedback → (no NEXT) → stack return
main-hive: ... → call_bee (pop) → sender
```

The `call_bee` pushes the current position onto a stack in the message content. When a Hive has no NEXT edge, `do_return()` pops the stack and routes back.

---

## Execution Model

### pg_net Batch Isolation

pg_net processes HTTP requests in batches. All requests in the same commit → same batch → block each other. A 25s Telegram long-poll would block LLM calls.

**Solution:** `pg_background_launch()` creates a separate transaction → separate commit → separate pg_net batch.

```
Message arrives (long-poll response):
  → on_net_response_safe (Batch 1: telegram_poll)
    → router → context → llm_bee
      → pg_background: separate transaction → pg_net http_post(LLM) → COMMIT
  LLM responds:
    → on_net_response_safe (Batch 2: llm_response, separate from poll)
```

**Critical:** `pg_background.max_workers = 256` in postgresql.conf (default 16 is insufficient).

### LLM Job Staging

LLM calls use a staging table (`meclaw.llm_jobs`) with retry logic:
1. `llm_bee_v2` inserts a job with model config
2. `pg_background_launch` calls `llm_http_call_by_id(job_id)`
3. The function waits for the job to appear (retry loop, max 5s)
4. Resolves model via `meclaw.resolve_model()`
5. Fires `pg_net.http_post()` to the LLM provider

### Message Append-Only Rule

- Only `status`, `assigned_to`, `waiting_for` are updated
- Content, type, sender — immutable after creation
- State transitions always produce events, never silent updates

---

## Queue Model

MeClaw does **not** use an external message queue. The `messages` table IS the queue.

```
ready (next_bee set) → trigger fires → bee processes → done
```

Ordering: `created_at` (clock_timestamp). Concurrency: `FOR UPDATE SKIP LOCKED` where needed.

---

## Worker Model

### No External Workers

All processing happens inside PostgreSQL:
- **Triggers** — synchronous, in the inserting transaction
- **pg_background** — asynchronous, separate transaction
- **pg_cron** — watchdog only (1x/min), not in the hot path

### Admin Bee

The admin dashboard runs as a persistent `plpython3u` function inside `pg_background`:
- `http.server` (Python stdlib) on port 8080
- Uses `psycopg2` with `autocommit=True` for chat (separate connection = separate transactions)
- Watchdog via pg_cron restarts if crashed

---

## Agent-Channel-User Relationship Model

```
                    ┌──────────────┐
                    │ System Agent │
                    │ (bootstrap)  │
                    └──────┬───────┘
                           │ owns infrastructure hives
                    ┌──────┴───────┐
              ┌─────┤  Channels    ├─────┐
              │     └──────────────┘     │
              │                          │
      ┌───────┴───────┐          ┌───────┴───────┐
      │   Agent A     │          │   Agent B     │
      │ (walter)      │          │ (support)     │
      │ AIEOS Identity│          │ AIEOS Identity│
      │ + Brain       │          │ + Brain       │
      ├───────────────┤          ├───────────────┤
      │ Hive: main    │          │ Hive: main    │
      │ Hive: memory  │          │ Hive: support │
      └───────┬───────┘          └───────┬───────┘
              │                          │
              │ subscribes               │ subscribes
              ↓                          ↓
      ┌───────────────┐          ┌───────────────┐
      │ Channel: DM   │          │ Channel: DM   │
      │ (telegram)    │◄─shared─►│ (telegram)    │
      └───────┬───────┘          └───────────────┘
              │
              │ messages from
              ↓
      ┌───────────────┐
      │ User: Marcus  │
      │ (entity:      │
      │  person)      │
      │ AIEOS Identity│
      │ explicit +    │
      │ observed      │
      │ profiles      │
      └───────────────┘
```

### Key Relationships

- **Agent → Channel:** via `agent_channels` (subscribe with role + scope)
- **Channel → Message:** messages belong to exactly one channel
- **Agent → Entity (User):** via observations in `entity_observations`
- **Agent → Hive:** via `OWNS` edge in AGE graph
- **Workspace Agent → Agents:** workspace contains agents, provides institutional memory

---

## Schema Overview

### Core Tables

| Table | Purpose |
|---|---|
| `meclaw.messages` | All messages (append-only event log, channel-scoped) |
| `meclaw.tasks` | Task grouping (one per user interaction) |
| `meclaw.channels` | Channel config with extraction cache state |
| `meclaw.agent_channels` | Agent-to-channel subscriptions (role, scope) |
| `meclaw.entities` | All entities: persons, agents, workspaces (AIEOS-compatible) |
| `meclaw.entity_observations` | Agent observations about entities over time |
| `meclaw.events` | System event log |
| `meclaw.net_requests` | pg_net request tracking |
| `meclaw.llm_jobs` | LLM call staging (retry-safe) |
| `meclaw.tools` | Tool registry |
| `meclaw.llm_providers` | LLM provider config |
| `meclaw.llm_models` | Model definitions (tier system) |
| `meclaw.rate_limits` | Rate limiting config |

### Brain Tables (Memory Hive)

| Table | Purpose | Scope |
|---|---|---|
| `meclaw.brain_events` | Extracted events with embeddings | Channel (shared) + Agent (rewards) |
| `meclaw.prototypes` | Emergent concepts from patterns | Agent (personal) |
| `meclaw.prototype_associations` | Hebbian co-activation weights | Agent (personal) |
| `meclaw.decision_traces` | Decision audit trail | Agent (personal) |

### AGE Graph: `meclaw_graph`

- **Agent** nodes — with AIEOS identity, type (agent/system/workspace)
- **Hive** nodes — execution containers, owned by agents
- **Bee** nodes — with `type`, `config` (soul, model_id, etc.)
- **Entity** nodes — persons, projects, tools, concepts
- **Event** nodes — conversation turns (immutable, channel-scoped)
- **Prototype** nodes — emergent concepts (agent-scoped)
- **OWNS** edges — Agent → Hive
- **ENTRY** edges — Hive → first Bee
- **NEXT** edges — Bee → Bee with `condition` (on_message, on_return, on_tool_call, on_tool_result)
- **TEMPORAL** edges — Event → Event (sequence ordering)
- **ACTIVATES** edges — Event → Prototype
- **ASSOCIATED** edges — Prototype → Prototype (Hebbian)
- **INVOLVED_IN** edges — Entity → Event
- **CITES** edges — Decision → Event

### Key Views

| View | Purpose |
|---|---|
| `meclaw.channel_conversation` | User-visible messages (user_input + llm_result) |
| `meclaw.recent_events` | Latest system events |
| `meclaw.agent_memory_stats` | Per-agent memory statistics (event count, prototype count, avg reward) |

---

## Security & Isolation

### Agent Boundary is the Security Boundary

MeClaw's security model operates at the **agent level**, not just the container level:

- **Within an Agent:** Full trust. All hives share brain, graph, messages, memory.
- **Between Agents:** Controlled sharing via channels with explicit subscriptions.
- **Between Workspaces:** Complete isolation. No shared state.

### Container Isolation (Deployment Level)

For maximum isolation, agents can run in separate containers:

```
Container A: Agent "walter" (personal assistant)
  └─ PostgreSQL
     └─ MeClaw (receiver, llm, memory, tool bees)

Container B: Agent "support" (customer support)
  └─ PostgreSQL
     └─ MeClaw (receiver, llm, escalation bees)
```

### Shared-Instance Mode (Single PostgreSQL)

For simpler deployments, multiple agents share one PostgreSQL instance:

```
PostgreSQL Instance
└─ MeClaw Schema
   ├── System Agent (infrastructure hives)
   ├── Agent "walter" (own hives, own brain, subscribed channels)
   ├── Agent "support" (own hives, own brain, subscribed channels)
   └── Shared: channels, extraction cache, entity graph
```

Isolation in shared-instance mode:
- **Brain tables** are agent-scoped (agent_id column, row-level)
- **Channels** are explicitly subscribed (agent_channels table)
- **Extraction** is shared (channel-level, no duplication)
- **plpython3u** still has full OS access — container is the real boundary for untrusted code

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
- One agent (or agent group) per container → maximum isolation
- Scaling = more containers, not more schemas
- Communication between containers: explicitly via network (pg_net), not via shared state

---

## Key Functions

| Function | Purpose |
|---|---|
| `meclaw.router_bee(msg_id)` | Graph routing via AGE Cypher |
| `meclaw.llm_bee_v2(msg_id)` | LLM call orchestration |
| `meclaw.context_bee(msg_id)` | Load tier-aware conversation history + compress static prefix |
| `meclaw.extract_bee(channel_id, msg_id)` | Channel-level entity/event extraction |
| `meclaw.novelty_bee(agent_id, event_id)` | Agent-level novelty scoring |
| `meclaw.feedback_bee(agent_id, msg_id)` | Agent-level retroactive reward |
| `meclaw.retrieve_bee(agent_id, query)` | Agent-level CTM-style memory retrieval |
| `meclaw.consolidation_bee(agent_id)` | Agent-level nightly consolidation |
| `meclaw.tool_bee(msg_id)` | Generic tool executor |
| `meclaw.sender_bee_v2(msg_id)` | Telegram message delivery |
| `meclaw.channel_bee_start(channel_id)` | Start long-poll |
| `meclaw.on_net_response_safe(req_id)` | HTTP response handler |
| `meclaw.resolve_model(model_id, tier)` | Model resolution |
| `meclaw.admin_bee(port)` | Start admin web server |
| `meclaw.send(text)` | Convenience: send a message to the default channel |

---

*For design decisions and principles, see [DESIGN.md](DESIGN.md).*
*For the memory hive architecture, see [BRAIN.md](BRAIN.md).*
*For the tool system, see [TOOLS.md](TOOLS.md).*

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
| `context_bee` | Load conversation history | `context_bee()` — tier-aware (5/15/30/50 msgs) |
| `call_bee` | Call a Hive (stack push/pop) | `call_bee()` + `do_return()` |
| `llm_bee` | LLM call via pg_background → pg_net | `llm_bee_v2()` + `llm_http_call_by_id()` |
| `tool_bee` | Execute tools from registry | `tool_bee()` — generic dispatcher |
| `sender_bee` | Telegram sendMessage | `sender_bee_v2()` |

### Agent = Hive = Graph in AGE

```cypher
(:Hive {id: 'main-graph'})
  -[:ENTRY]-> (:Bee {id: 'main-receiver-bee', type: 'receiver_bee'})

(:Bee {id: 'main-receiver-bee'}) -[:NEXT {condition: 'on_message'}]-> (:Bee {id: 'main-call-bee'})
(:Bee {id: 'main-call-bee'})     -[:NEXT {condition: 'on_return'}]->  (:Bee {id: 'main-sender-bee'})

(:Hive {id: 'test-agent'})
  -[:ENTRY]-> (:Bee {id: 'test-context-bee', type: 'context_bee'})
```

Defining a new agent = inserting a new Hive + Bees + Edges in AGE. No code deployment.

---

## Message Model

### Principle

- Every interaction = a message in `meclaw.messages`
- Messages are **append-only** — never updated (only `status`, `assigned_to`, `waiting_for` change)
- Every state transition is logged as an event
- `clock_timestamp()` everywhere — never `now()`

### Message Schema

```sql
id           UUID PRIMARY KEY DEFAULT gen_random_uuid()
task_id      UUID REFERENCES meclaw.tasks(id)
channel_id   UUID REFERENCES meclaw.channels(id)
previous_id  UUID                      -- chain link
type         TEXT NOT NULL              -- see types below
sender       TEXT NOT NULL              -- which bee/user
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
main-graph: receiver → call_bee (push "test-agent") → ...
test-agent: context → llm → tool → llm → (no NEXT) → stack return
main-graph: ... → call_bee (pop) → sender
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

## Schema Overview

### Core Tables

| Table | Purpose |
|---|---|
| `meclaw.messages` | All messages (append-only event log) |
| `meclaw.tasks` | Task grouping (one per user interaction) |
| `meclaw.channels` | Channel config (Telegram, Web) |
| `meclaw.events` | System event log |
| `meclaw.net_requests` | pg_net request tracking |
| `meclaw.llm_jobs` | LLM call staging (retry-safe) |
| `meclaw.tools` | Tool registry |
| `meclaw.llm_providers` | LLM provider config |
| `meclaw.llm_models` | Model definitions (tier system) |
| `meclaw.rate_limits` | Rate limiting config |

### AGE Graph: `meclaw_graph`

- **Hive** nodes — agent containers
- **Bee** nodes — with `type`, `config` (soul, model_id, etc.)
- **ENTRY** edges — Hive → first Bee
- **EDGE** edges — Bee → Bee with `condition` (on_message, on_return, on_tool_call, on_tool_result)

### Key Views

| View | Purpose |
|---|---|
| `meclaw.channel_conversation` | User-visible messages (user_input + llm_result) |
| `meclaw.recent_events` | Latest system events |

---

## Key Functions

| Function | Purpose |
|---|---|
| `meclaw.router_bee(msg_id)` | Graph routing via AGE Cypher |
| `meclaw.llm_bee_v2(msg_id)` | LLM call orchestration |
| `meclaw.context_bee(msg_id)` | Load tier-aware conversation history |
| `meclaw.tool_bee(msg_id)` | Generic tool executor |
| `meclaw.sender_bee_v2(msg_id)` | Telegram message delivery |
| `meclaw.channel_bee_start(channel_id)` | Start long-poll |
| `meclaw.on_net_response_safe(req_id)` | HTTP response handler |
| `meclaw.resolve_model(model_id, tier)` | Model resolution |
| `meclaw.admin_bee(port)` | Start admin web server |
| `meclaw.send(text)` | Convenience: send a message to the default channel |

---

*For design decisions and principles, see [DESIGN.md](DESIGN.md).*
*For the tool system, see [TOOLS.md](TOOLS.md).*

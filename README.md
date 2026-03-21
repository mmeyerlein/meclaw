<p align="center">
  <img src="assets/meclaw-logo.svg" alt="MeClaw" width="400" />
</p>

<h3 align="center">The AI Agent OS that lives inside your database.</h3>

<p align="center">
  No runtime. No sidecar. No framework.<br/>
  Just PostgreSQL — thinking.
</p>

<p align="center">
  <a href="https://www.postgresql.org/"><img src="https://img.shields.io/badge/PostgreSQL_17-316192?style=flat-square&logo=postgresql&logoColor=white" alt="PostgreSQL 17" /></a>
  <a href="https://age.apache.org/"><img src="https://img.shields.io/badge/Apache_AGE-Graph_Routing-orange?style=flat-square" alt="Apache AGE" /></a>
  <a href="https://github.com/pgvector/pgvector"><img src="https://img.shields.io/badge/pgvector-Semantic_Memory-blue?style=flat-square" alt="pgvector" /></a>
  <a href="https://github.com/supabase/pg_net"><img src="https://img.shields.io/badge/pg__net-Async_HTTP-green?style=flat-square" alt="pg_net" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Apache_2.0-blue?style=flat-square" alt="Apache 2.0" /></a>
</p>

<p align="center">
  <a href="https://meclaw.ai">Website</a> ·
  <a href="#quick-start">Quick Start</a> ·
  <a href="#how-it-works">How It Works</a> ·
  <a href="#the-claw-ecosystem">Ecosystem</a> ·
  <a href="docs/">Docs</a>
</p>

---

> **🤖 100% Vibe-Coded.** This entire project — every SQL function, every trigger chain, every line of Python, this README, and even the website — was built through conversational AI. The human ([Marcus Meyer](https://linkedin.com/in/marcus-meyer-585429a3/)) designed the architecture and made decisions. The code was written by [Claude Opus 4.6](https://anthropic.com/claude) via [OpenClaw](https://github.com/openclaw/openclaw). No line was typed by hand.

---

## What is MeClaw?

Every AI agent framework runs **outside** the database. MeClaw runs **inside** it.

MeClaw turns PostgreSQL into an operating system for AI agents. Messages flow through trigger chains. Agents are nodes in a graph. LLM calls go out via async HTTP. Tools execute as SQL functions. Nothing leaves the database process.

```sql
SELECT meclaw.send('How many users signed up today?');
```

That's a real query. It fires a trigger chain that routes through a graph, loads conversation context, calls an LLM, executes a tool if needed, and delivers the response — all inside PostgreSQL.

## Quick Start

```bash
git clone https://github.com/mmeyerlein/meclaw.git
cd meclaw
cp config.example.sql config.sql   # Add your API keys
docker compose -f docker-compose.build.yml up -d
```

Open `http://localhost:8080` — your agent is running.

📖 **More install options** (Portainer, manual PostgreSQL): [docs/INSTALL.md](docs/INSTALL.md)

## How It Works

MeClaw uses a **Hive & Bee** architecture:

```
┌─────────────────────────────────────────────────┐
│  HIVE (= one PostgreSQL container)              │
│                                                  │
│  📥 Receiver ──▶ 🗺️ Router ──▶ 📚 Context      │
│       Bee           Bee            Bee           │
│                      │              │            │
│                      ▼              ▼            │
│                 🧠 LLM  ◀──▶  🔧 Tool           │
│                    Bee           Bee             │
│                      │                           │
│                      ▼                           │
│                 📤 Sender                        │
│                    Bee                           │
│                                                  │
│  Routing: Apache AGE graph (Cypher)              │
│  Events:  Trigger chains (zero polling)          │
│  LLM:    pg_net async HTTP (any provider)        │
│  Tools:  plpython3u (full OS access in sandbox)  │
│  State:  Append-only event log (ACID)            │
└─────────────────────────────────────────────────┘
```

**Bees** are pure functions — one input, one output, no loops. The **Router Bee** reads a Cypher graph to decide what happens next. Everything fires through PostgreSQL triggers. No orchestration layer. No event bus. No message queue. The database *is* the message queue.

### Multi-Provider LLM

```sql
INSERT INTO meclaw.llm_models (id, provider_id, model_name, tier) VALUES
  ('sonnet-4', 'openrouter', 'anthropic/claude-sonnet-4', 'large'),
  ('haiku',    'openrouter', 'anthropic/claude-3.5-haiku', 'medium'),
  ('local',    'vllm',       'Qwen/Qwen3.5-9B',           'small');
```

Each Bee can use a different model. Context history adapts automatically — 5 messages for small models, 30 for large ones.

### Built-in Tools

| Tool | What it does |
|---|---|
| `sql_read` | Run SELECT queries against the database |
| `sql_write` | INSERT, UPDATE, DELETE with audit trail |
| `python_exec` | Execute Python code (requests, urllib, math, os) |

The LLM decides when to use tools via OpenAI function calling. Tool results flow back through the same event chain.

### Admin Dashboard

A web UI served directly from PostgreSQL — no web server needed:

```sql
SELECT meclaw.admin_bee(8080);
-- Status · Graph Visualization · Event Log · Chat · Models
```

### Channels

| Channel | Status | How |
|---|---|---|
| Telegram | ✅ | Self-sustaining pg_net long-poll |
| Web Chat | ✅ | Built-in HTTP server |
| Slack | 🔜 | Webhook via HTTP server |

## The Claw Ecosystem

MeClaw is part of a thriving ecosystem of AI agent projects. Here's where it fits:

|  | OpenClaw | NanoBot | ZeroClaw | PicoClaw | NanoClaw | GoClaw | **MeClaw** |
|---|---|---|---|---|---|---|---|
| **Language** | TypeScript | Python | Rust | Go | TypeScript | Go | **SQL** |
| **Runtime** | Node.js | Python | Native binary | Native binary | Node.js | Native binary | **None (PostgreSQL)** |
| **RAM** | >1 GB | >100 MB | <5 MB | <10 MB | ~200 MB | ~35 MB | **Shares PG** |
| **Agent logic** | App code | App code | App code | App code | App code | App code | **Triggers + Graph** |
| **State** | Files + DB | SQLite/PG | Files | Files | Files | PostgreSQL | **PostgreSQL IS the state** |
| **Routing** | Config | Config | Config | Config | Config | Config + PG | **Cypher graph queries** |
| **Observability** | Logs | Logs | Logs | Logs | Logs | OTel | **`SELECT * FROM events`** |
| **Recovery** | Manual | Manual | Manual | Manual | Manual | Manual | **ACID + WAL + PITR** |

**The difference:** Every other project runs agents as an **application** that uses a database. MeClaw runs agents **as** the database.

## Design Principles

1. **Pure PostgreSQL** — If it can't be done with extensions, it doesn't get done. Zero external processes.
2. **Event-Driven** — Trigger chains fire on every INSERT. No polling in the hot path. No cron loops.
3. **Append-Only** — Every message, every LLM call, every routing decision — immutable event log.
4. **Container = Sandbox** — One Hive = one container. `plpython3u` gets full OS access because the container is the boundary.
5. **Bees Don't Loop** — An agent does one thing and hands off. The graph decides what's next.

## Extensions

MeClaw stands on the shoulders of these PostgreSQL extensions:

| Extension | Role |
|---|---|
| [Apache AGE](https://age.apache.org/) | Graph DB — agent routing via Cypher |
| [pg_net](https://github.com/supabase/pg_net) | Async HTTP — LLM calls, webhooks |
| [pg_cron](https://github.com/citusdata/pg_cron) | Watchdog only (not in the hot path) |
| [pg_background](https://github.com/vibhorkum/pg_background) | Background workers for LLM execution |
| [pgvector](https://github.com/pgvector/pgvector) | Vector search — semantic memory |
| [plpython3u](https://www.postgresql.org/docs/17/plpython.html) | Python inside SQL — tools + admin UI |

## Status

**v0.1.0** — Memory Hive. 30 SQL files, 6 Bees, Knowledge Graph, LLM extraction, 69 smoke tests. 100% vibe-coded.

- [x] Event-driven trigger chain
- [x] Apache AGE graph routing
- [x] Multi-provider LLM (OpenRouter, vLLM, any OpenAI-compatible)
- [x] Tool system (SQL, Python)
- [x] Tier-aware conversation history
- [x] Telegram + Web Chat channels
- [x] Admin dashboard
- [x] Rate limiting
- [x] Docker
- [ ] Semantic memory (pgvector embeddings)
- [ ] Skill & tool-set management
- [ ] Slack channel
- [ ] Multi-user

## License

[Apache 2.0](LICENSE) — Marcus Meyer, 2026

## Links

- 🌐 **Website:** [meclaw.ai](https://meclaw.ai)
- 📖 **Docs:** [docs/](docs/)
- 🐛 **Issues:** [GitHub Issues](https://github.com/mmeyerlein/meclaw/issues)

---

<p align="center">
  <b>MeClaw</b> — Your database already knows how to think.<br/>
  <i>You just haven't asked it yet.</i>
</p>

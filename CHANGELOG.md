# Changelog

## [0.1.0] — Unreleased

### Added
- **Core Architecture** — Event-driven trigger chain, append-only messages
- **Apache AGE Graph** — Hive/Bee routing with Cypher queries
- **Multi-Provider LLM** — OpenRouter, vLLM, OpenAI-compatible endpoints
- **Tool System** — `sql_read`, `sql_write`, `python_exec` with function calling
- **Conversation History** — Tier-aware context loading (5/15/30/50 messages)
- **Telegram Channel** — Self-sustaining pg_net long-poll
- **Web Admin Dashboard** — Status, graph visualization, event log, chat, model management
- **Rate Limiting** — Per-minute/hour/day limits for LLM calls
- **Docker Support** — Single `docker compose up` deployment
- **pg_search 0.15.10** — ParadeDB BM25 full-text search in Dockerfile (2026-03-18)
- **AIEOS v1.2 Integration** — Entity schema follows AI Entity Object Specification for all entity types (2026-03-19)

### Changed
- **BRAIN.md v2** — AIEOS identity, context compression, personality-aware retrieval (2026-03-19, commit 7a61fb8)
- **BRAIN.md → English** — Complete translation, no Denglisch (2026-03-19, commit e70d193)
- **Brain Schema Phase 1** — 7 new tables + views deployed (2026-03-20)
  - `entities` (AIEOS-compatible: neural_matrix, traits, dual profiles)
  - `agent_channels` (Agent ↔ Channel subscriptions with role + scope)
  - `brain_events` (append-only, channel-level extraction with BM25 index)
  - `prototypes` + `prototype_associations` (Hebbian, agent-scoped)
  - `entity_observations` (User-Profile Tracking)
  - `decision_traces` (immutable audit trail)
  - `agent_memory_stats` view
- **AGE Graph: Agent & Channel Nodes** — System Agent, Walter Agent, Channel nodes, OWNS/SUBSCRIBES/SERVES/COMMUNICATES_VIA edges (2026-03-20)
- **Seed Agents** — System Agent, Walter (AIEOS identity), Marcus Meyer (person entity with explicit + observed profiles) (2026-03-20)
- **Extract Bee** — Channel-level extraction trigger on message done (2026-03-20)
- **Retrieve Bee** — BM25 search with channel scoping + RRF foundation (2026-03-20)
- **Context Bee V2** — With memory retrieval integration (2026-03-20)
- **Embedding Service** — OpenRouter text-embedding-3-small via plpython3u, async via pg_background (2026-03-20)
- **RRF Retrieval** — BM25 + pgvector cosine fusion in retrieve_bee (2026-03-20)
- **Novelty Bee** — Agent-level novelty scoring + prototype creation (2026-03-20)
- **Feedback Bee** — Keyword-based sentiment → retroactive reward on previous events (2026-03-20)
- **MeClaw Watchdog Fix** — Checks poll age, not just existence (2026-03-20)
- **Phase 3: Graph Intelligence** (2026-03-20)
  - `messages.seq` column finally added (pg_cron pause trick)
  - AGE Temporal Edges — Event→Event sequence linking in extract_bee
  - Entity Resolution — `resolve_entity()` + `get_entity()` (canonical, alias, fuzzy)
  - Personality-Fit Scoring — agent + user neural_matrix influence retrieval ranking
  - retrieve_bee v3 — RRF + Graph Expansion + Personality-Fit + Recency + Novelty (6-signal ranking)
  - feedback_bee — Discounted reward propagation (γ=0.9, depth=5)
  - novelty_bee integrated into extract trigger
  - AGENTS.md Parser stub for codebase context ingestion
  - Embedding API key fixed (was using revoked key)
- **Phase 4: Consolidation & User Modeling** (2026-03-20)
  - `consolidation_bee` — nightly: prune associations, merge prototypes, decay decisions, Hebbian recalibration
  - `consolidation_nightly` — pg_cron at 03:00 UTC, runs all agents
  - `observe_entity` — upsert observations with confidence tracking
  - Entity observation consolidation — merge duplicates, increase confidence
  - `observed_profile` auto-update from consolidated observations
  - Workspace Agent stub (`meclaw:workspace:default`)
  - Prototype Mitosis flagging (high variance → decay)
  - Seeded 5 observations about Marcus → observed_profile populated
- **Docs v3 — Fundamental Architecture Redesign** (2026-03-20, commit ce143c0)
  - Channels as universal primitive (channel-level extraction, shared across agents)
  - Agent = Multi-Hive Root (hierarchical, no orphan hives)
  - System Agent (infrastructure owner, bootstrap)
  - Users = Entities with AIEOS identity (explicit + observed profiles)
  - Workspaces = Agents (institutional memory)
  - Scoping model (private / shared / workspace)
  - Knowledge Graph vs Execution Graph separation
  - Storage Hierarchy (5 layers)
  - Six Bees with scope annotations (channel vs agent level)

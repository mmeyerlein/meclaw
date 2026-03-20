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

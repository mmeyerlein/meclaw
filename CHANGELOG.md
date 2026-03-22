# Changelog

## [0.3.1] — 2026-03-22

### Upgraded
- **ParadeDB pg_search 0.15.10 → 0.22.2** — Fixes `rt_fetch used out-of-bounds` bug (GitHub #2462, #3135)
- **VM CPU: KVM → host passthrough** — Required for AVX2 (ParadeDB 0.19+)
- **VM specs: 2→4 cores, 4→8 GB RAM**

### Fixed
- **BM25 Query Sanitization** — `sanitize_bm25_query()` removes special chars that crash ParadeDB BM25 parser (apostrophes, parens, etc.)
- **time_filter_before date-only → end-of-day** — LLM returns "2023-05-28" parsed as 00:00:00, filtering out same-day events. Now extends to 23:59:59
- **VACUUM after embed + pipeline** — BM25 index refresh after INSERT/UPDATE cycles
- **DROP+CREATE BM25 index on reset** — Ensures clean index state between benchmark questions
- **pg_background timing** — Wait loop for async extract_bee triggers to complete before embed
- **paradedb.score → pdb.score** — v0.19.0 breaking change applied to all 29 occurrences

### Changed
- SQL files: 47 (added `sql/47_bm25_sanitize.sql`)
- Runner: `--smart` and `--decompose` flags for temporal-aware retrieval
- Benchmark results: 16/20 Smart retrieval (80%) with v0.22.2, 0 rt_fetch errors

## [0.3.0] — 2026-03-21

### Added
- **6-Signal Weighted Ranking** — `similarity*0.25 + reward*0.25 + novelty*0.15 + recency*0.10 + personality_fit*0.15 + graph_distance*0.10` in retrieve_bee
- **Reward Propagation** — Discounted returns (γ=0.9, depth 5) backward through event chain
- **Decision Traces** — `log_decision_trace`, `cite_events` with CITES edges in AGE graph
- **ACTIVATES Edges** — Event→Prototype edges in AGE (top-3 per event)
- **ASSOCIATION Edges** — Prototype→Prototype Hebbian edges mirrored in AGE
- **Prototype Mitosis** — `detect_conflicting_prototypes`, `split_prototype` for reward-conflicted prototypes
- **MemCell Nodes** — Boundary-detected conversation chunks via embedding distance, BELONGS_TO edges
- **LLM Re-Ranking (Stage 3)** — `retrieve_reranked` with gpt-4o-mini candidate re-ranking (~$0.35/500 questions)
- **Citation Authority** — `trending_precedents` and `stale_precedents` views
- **Runner: Signal Pipeline** — Explicit backfill for embeddings, entities, novelty, temporal, facts after feed
- **Runner: `--rerank`** — Enable LLM re-ranking in benchmark
- **Runner: `--top-k`** — Configurable result count (default: 10)
- **Runner: `--cumulative`** — Brain persists across questions
- **Runner: `--ctm`** — CTM drift retrieval mode
- **Runner: `--skip-extraction`** — Skip expensive LLM entity extraction

### Fixed
- **Duplicate brain_events** — Trigger + manual `extract_bee` call created 2x events per message
- **Trigger scope** — Changed from `user_input + llm_result` to `user_input` only (assistant messages don't create brain_events)
- **Wrong timestamps** — `brain_events.created_at` now inherits from `messages.created_at` instead of `NOW()`
- **pg_background exhaustion** — Exception handler prevents crash when worker slots full during bulk insert
- **Spurious rewards** — Runner resets feedback_bee rewards before retrieval (benchmark conversations have meaningless sentiment)
- **AGE Cypher in PL/pgSQL** — Converted `cite_events` and `build_memcells` to plpython3u to avoid `$$` quoting hell

### Infrastructure
- **45 SQL files** (was 39) — clean numbering 01-45
- **Test Suite** — 108 PASS, 4 SKIP, 0 FAIL

## [0.2.0] — 2026-03-21

### Added
- **LongMemEval Benchmark** — Runner + Evaluator for 500-question benchmark (`benchmarks/longmemeval_runner.py`, `longmemeval_eval.py`)
- **Dual LLM Judge** — Separate Context Quality and Answer Correctness judges (no contamination)
- **Batch Embeddings** — Single API call for up to 50 texts (55x speedup)
- **Fact-Augmented Key Expansion** — `facts_text` column with dual BM25 index on content + extracted entities/relations
- **Trigger Chain** — Automated pipeline: extract_bee → novelty_bee (async) → feedback_bee
- **CTM Retrieval** — Optional tick-based embedding drift retrieval (Stage 4 in retrieve_bee)
- **Prototype Activation** — Seed prototypes from events, centroid computation, novelty-driven creation
- **User Modeling** — Automatic preference extraction from conversations, auto-create sender entities
- **Temporal Edges** — AGE Event→Event temporal chain with TEMPORAL edge type
- **Entity→Event AGE Links** — INVOLVED_IN edges linking entities to events in Apache AGE graph
- **Graph-Based Retrieval** — Cypher traversal for related events via entity co-occurrence and temporal proximity
- **ACTION_PLAN.md** — Roadmap document for AGE Graph activation phases

### Changed
- **retrieve_bee v3** — 6-signal ranking: BM25 + Vector + Graph Expansion + facts_text + RRF fusion
- **extract_bee** — Preserves message timestamp in brain_events.created_at, creates AGE Event nodes + TEMPORAL edges
- **novelty_bee** — Integrated prototype update with centroid computation
- **feedback_bee v2** — Clean single signature (UUID, TEXT), dropped zombie overload
- **llm_extract_entities** — Extracts user preferences, auto-creates sender entities
- **Smoke Tests** — Data-dependent tests now SKIP instead of FAIL on fresh DB; 4 new tests for temporal edges, facts_text, graph retrieval, prototypes
- **X-Title Header** — All OpenRouter API calls identify as "MeClaw"

### Fixed
- **Ambiguous `event_id`** in retrieve_bee PL/pgSQL CTEs (column vs return variable)
- **Ambiguous `channel_id`** in retrieve_bee (table alias added)
- **Cypher escaping** — Apostrophes in entity names no longer crash AGE queries
- **get_query_embedding** — Non-cached version removed, only cached+retry version active
- **init-meclaw.sh** — All 39 SQL files in correct dependency order
- **SQL file numbering** — Resolved duplicate file numbers (35, 36) from parallel development
- **`pg_background` workers** — `max_worker_processes` increased to 32

### Infrastructure
- **39 SQL files** (was 33) — clean numbering 01-39
- **Test Suite** — 103 PASS, 9 SKIP, 0 FAIL (smoke + extended)

---

## [0.1.0] — 2026-03-20

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
- **Phase 5: CTM Retrieval + Multi-Agent** (2026-03-20)
  - `ctm_retrieve` — tick-based iterative retrieval (1-3 ticks, entropy convergence)
  - Query embedding drifts toward relevant concept region (α=0.3 blending)
  - Adaptive compute: simple queries converge in 1 tick, complex in 2-3
  - `share_channel` — cross-agent channel sharing with scope enforcement
  - `discover_agents` — AIEOS-compatible agent discovery by capability
  - `cross_agent_retrieve` — query another agent's memory through shared channels only
  - `generate_agent_keypair` — Ed25519 stub for future AIEOS signing
- **Phase 6: Real LLM Extraction** (2026-03-20, commit 7105ca9)
  - extract_bee v2: two-stage pipeline (raw content + async LLM extraction via pg_background)
  - `llm_extract_entities()`: gpt-4o-mini structured extraction (entities + relations + cost tracking)
  - `create_or_resolve_entity()`: auto-create or merge entities via resolve_entity()
  - AGE Graph helpers: `age_upsert_entity`, `age_link_entity_event`, `age_link_entities` (avoid SQL MERGE conflict)
  - `entity_events` junction table (entity ↔ event with relation types + confidence)
  - `backfill_extractions()`: process existing unextracted events
  - brain_events: `extracted`, `extracted_at`, `extraction_data` columns
- **Phase 7: Robustness & Error Tolerance** (2026-03-20, commit d92b754)
  - feedback_bee v2: negation detection + LLM-based sentiment for ambiguous cases
  - `llm_sentiment()`: structured sentiment classification (gpt-4o-mini, confidence-weighted rewards)
  - compute_embedding: 3x retry with exponential backoff + 429 rate limit handling
  - `embedding_cache` table: query embedding cache (500 entries, auto-evict)
  - `get_query_embedding` v2: cache-first, retry, rate limit aware
  - personality_fit v2: 5-dimensional keyword clusters (technical, emotional, creative, analytical, organizational) + user alignment
  - `hebbian_update()`: co-activation tracking via entity_events → prototype_associations
  - Prototype seeds for all discovered entities
- **Phase 7a: Smoke Tests** (2026-03-20, commit 7819589)
  - `run_smoke_tests()`: 69 assertions (schema, pipeline, function unit tests)
  - Schema: 15 tables, 26 functions, 4 indices, pg_cron, AGE graph, entities, providers
  - Pipeline: brain_events, embeddings, LLM extraction, retrieve_bee, entity_events
  - Unit: resolve_entity, create_or_resolve_entity, get_query_embedding, personality_fit, llm_sentiment
- **Phase 8: Swarm Foundation** (2026-03-20, commit 28b0158)
  - `llm_models` table: model registry with capabilities, cost, tier, speed, quality (4 models seeded)
  - `skills` table: 9 built-in skills with categories and requirements
  - `execution_plans` + `execution_steps`: DAG-based task execution with cost tracking
  - `concierge_bee`: gpt-4o-mini classifier (simple/moderate/complex)
  - `planner_bee`: generates execution DAG from skills + models
  - `dag_executor`: runs plan steps in dependency order with cost per step
  - `dag_feedback`: reward at plan level
  - `swarm_process`: full pipeline (classify → plan → execute)
  - `select_model`: picks cheapest model meeting skill requirements
- **Phase 9: Context Pipeline** (2026-03-20, commit d859498)
  - `markdown_compress()`: lossless token reduction (fillers, whitespace, tables, decorative markdown)
  - `context_cache` table: cached compressed static prefix (1h TTL)
  - `build_static_prefix()`: agent identity + user profile + skills → compressed
  - `context_bee_v3`: compressed prefix + CTM retrieval + cache breakpoint marker
  - `estimate_tokens()`: rough token count helper
  - `parse_agents_md()`: structured AGENTS.md parser (sections, rules extraction)
- **Phase 10: Extended Tests + Cost Monitoring** (2026-03-20, commit 4336084)
  - `run_extended_tests()`: 39 assertions for phases 8+9 + integration
  - `run_all_tests()`: combined smoke + extended = 108 assertions
  - `cost_summary` view: daily DAG execution cost per agent
  - `extraction_cost_summary` view: daily extraction token usage
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

# PROGRESS.md — Implementation Tracker

> Companion to [BRAIN.md](BRAIN.md) (architecture & concepts).
> This file tracks what's built, what's next, and known gaps.

---

## SQL Files

| # | File | Phase | Content |
|---|------|-------|---------|
| 01 | extensions | — | pg extensions (AGE, pgvector, pg_search, pg_cron, pg_background, plpython3u) |
| 02 | schema | — | Core tables (messages, tasks, channels, hives, bees, llm_jobs, events) |
| 03 | logging | — | Event logging helpers |
| 04 | channel_bee | — | Telegram long-poll channel |
| 05 | router_bee | — | Message routing + dispatch |
| 06 | llm_bee | — | LLM HTTP calls (multi-provider) |
| 07 | io_bees | — | Input/output message processing |
| 08 | triggers | — | Core trigger chain |
| 09 | age_graph | — | AGE graph bootstrap (hives, bees, edges) |
| 10 | seed | — | Initial data (providers, hives, channels) |
| 11 | start | — | Startup sequence (watchdog, admin_bee) |
| 12 | admin_bee | — | Admin dashboard, model management |
| 13 | context_bee | — | Context loading + conversation history |
| 14 | tools | — | Tool system (sql_read, sql_write, python_exec) |
| 15 | llm_providers | — | Multi-provider config (OpenRouter, vLLM) |
| 16 | brain_schema | 1 | Brain tables (brain_events, entities, prototypes, observations, decisions) |
| 17 | age_agents | 1 | AGE graph: Agent + Channel nodes, edges |
| 18 | seed_agents | 1 | Seed: System Agent, Walter, Marcus Meyer |
| 19 | extract_bee | 1 | Channel-level extraction → brain_events + embedding |
| 20 | retrieve_bee | 1 | BM25 + pgvector + RRF retrieval |
| 21 | context_bee_v2 | 1 | Memory-integrated context pipeline |
| 22 | embedding_bee | 2 | Embedding service (OpenRouter text-embedding-3-small) |
| 23 | novelty_bee | 2 | Novelty scoring + prototype creation |
| 24 | feedback_bee | 2 | Keyword-based sentiment → retroactive reward |
| 25 | phase3_graph_intelligence | 3 | Temporal edges, entity resolution, personality_fit, retrieve_bee v3 |
| 26 | consolidation_bee | 4 | Nightly: prune, merge, decay, observe, consolidate |
| 27 | ctm_retrieval | 5 | CTM tick-based retrieval, multi-agent sharing, AIEOS discovery |
| 28 | extract_bee_v2 | 6 | LLM entity+relation extraction, AGE graph helpers, entity_events |
| 29 | phase7_robustness | 7 | LLM sentiment, negation, retry/backoff, embedding cache, Hebbian |

---

## Phases

### Phase 1 — Foundation ✅ `58c4540`
- Schema: brain_events, entities, entity_observations, channels, agent_channels
- extract_bee (channel-level): raw content → brain_events + embeddings
- retrieve_bee: BM25 + pgvector + RRF
- context_bee_v2: memory retrieval integration
- System Agent + Walter bootstrap

### Phase 2 — Learning ✅ `1ff1063`
- novelty_bee: novelty score + prototype creation (threshold 0.7)
- feedback_bee: keyword sentiment → retroactive reward
- Reward-weighted ranking in retrieve_bee
- AIEOS neural_matrix seed for Walter
- Marcus Meyer entity with explicit_profile

### Phase 3 — Graph Intelligence ✅ `939e13d`
- messages.seq column (pg_cron pause trick)
- AGE temporal edges (Event→Event TEMPORAL)
- Entity resolution: resolve_entity (canonical, alias, fuzzy) + get_entity
- Personality-fit scoring (agent + user neural_matrix)
- retrieve_bee v3: 6-signal ranking (BM25+vector RRF, graph ±3, pfit, recency, novelty, reward)
- feedback_bee: discounted reward propagation (γ=0.9, depth=5)
- AGENTS.md parser stub

### Phase 4 — Consolidation & User Modeling ✅ `77a6e80`
- consolidation_bee (pg_cron 03:00 UTC): prune, merge (cosine>0.92), mitosis, Hebbian decay (0.95)
- observe_entity: upsert with confidence tracking
- observed_profile auto-update from consolidated observations
- Workspace Agent stub (meclaw:workspace:default)
- 5 seed observations for Marcus

### Phase 5 — CTM Retrieval + Multi-Agent ✅ `167a0ad`
- ctm_retrieve: 1-3 ticks, α=0.3 blending, Shannon entropy < 0.3 convergence
- share_channel: cross-agent sharing with scope
- discover_agents: AIEOS-compatible discovery
- cross_agent_retrieve: memory query through shared channels
- Ed25519 keypair stub

### Phase 6 — Real LLM Extraction ✅ `7105ca9`
- extract_bee v2: two-stage (raw + async LLM via pg_background)
- llm_extract_entities: gpt-4o-mini, structured JSON (entities + relations)
- create_or_resolve_entity: auto-create or merge
- AGE helpers: age_upsert_entity, age_link_entity_event, age_link_entities
- entity_events junction table (entity ↔ event, relation type, confidence)
- backfill_extractions: process unextracted events
- brain_events: extracted, extracted_at, extraction_data columns
- Cost tracking per extraction

### Phase 7 — Robustness ✅ `d92b754`
- feedback_bee v2: negation detection + LLM sentiment (llm_sentiment, gpt-4o-mini)
- compute_embedding: 3x retry, exponential backoff, 429 handling
- embedding_cache table (500 entries, auto-evict, MD5 key)
- get_query_embedding v2: cache-first, retry, rate limit aware
- personality_fit v2: 5 dimensions (technical, emotional, creative, analytical, organizational)
- hebbian_update: co-activation via entity_events → prototype_associations
- Prototype seeds for discovered entities

### Phase 7a — Smoke Tests ✅ `7819589`
> Minimal test suite before v0.1.0 release. `SELECT meclaw.run_smoke_tests();`

- [ ] Schema smoke: all tables, functions, indices, pg_cron jobs, AGE graph exist
- [ ] Pipeline smoke: message → extract → embedding → LLM extraction → retrieve
- [ ] Unit tests: resolve_entity, llm_sentiment, get_query_embedding, personality_fit, create_or_resolve_entity
- [ ] `30_smoke_tests.sql` — single function, PASS or explosion

### Phase 8 — Swarm Foundation ☐
> Prerequisite for autonomous Dev-Workflow "Hello World"

- [ ] concierge_bee: classifier (gpt-4o-mini), routes simple vs complex
- [ ] Multi-Model Pool: capabilities + cost in llm_providers/llm_models
- [ ] Skill Registry: structured defs in DB, queryable, with embedding
- [ ] planner_bee: top-tier LLM generates DAG from skill+model pool
- [ ] Feedback loop at DAG level (reward per DAG + per bee)

### Phase 9 — Context Pipeline ☐
> context_bee is basic — no compression, no caching

- [ ] Lossless markdown compression (20-40% token reduction)
- [ ] Anthropic cache breakpoint (stable prefix → high hit rate)
- [ ] Use ctm_retrieve instead of standard retrieve_bee
- [ ] AGENTS.md parser: full implementation

### Phase 10 — Tests & Validation ☐
> Every phase produces debt. This is where it gets paid.

- [ ] Unit: resolve_entity, personality_fit, observe_entity, feedback_bee sentiment, retrieve_bee ranking
- [ ] Integration: full message flow (user_input → extract → novelty → embedding → retrieve → context → llm → sender)
- [ ] Regression: schema validation, pg_cron jobs, embedding provider reachable
- [ ] Load: parallel messages, embedding timeouts, pg_background limits
- [ ] Cost: OpenRouter spend per day/week/agent

**Rule: Tests after every phase, not at the end.**

---

## Known Weaknesses (Self-Review 2026-03-20)

| Area | Issue | Severity | Phase |
|------|-------|----------|-------|
| ~~extract_bee~~ | ~~Raw content only, no LLM extraction~~ | ~~Critical~~ | ~~6~~ ✅ Fixed |
| ~~feedback_bee~~ | ~~No negation detection~~ | ~~High~~ | ~~7~~ ✅ Fixed |
| ~~personality_fit~~ | ~~Keyword-only~~ | ~~Medium~~ | ~~7~~ ✅ Fixed (5-dim) |
| ~~Embedding~~ | ~~No retry/backoff~~ | ~~Medium~~ | ~~7~~ ✅ Fixed |
| ~~Hebbian~~ | ~~No active update~~ | ~~Medium~~ | ~~7~~ ✅ Fixed |
| CTM Retrieval | Up to 12 API calls per retrieval (3 ticks) | Medium | Optimize later |
| Tests | Zero test coverage | High | 10 (incremental) |
| Context Pipeline | No compression, no cache breakpoint | Medium | 9 |
| Swarm | No routing, no planning, no skill registry | — | 8 |

---

## Infrastructure

- **VM:** `openclaw@10.235.40.50` | Container: `meclaw`
- **DB:** PostgreSQL, schema `meclaw`, user `postgres`
- **GitHub:** https://github.com/mmeyerlein/meclaw (branch: main)
- **OpenRouter Key:** dedicated MeClaw key, App-Header "MeClaw"
- **pg_cron Jobs:** watchdog (1), admin_bee_watchdog (2), consolidation-nightly 03:00 UTC (3)
- **Entities:** meclaw:agent:walter, meclaw:agent:system, meclaw:person:marcus-meyer, meclaw:workspace:default
- **Channels:** 00000000-...-01 (Telegram), 00000000-...-02 (internal)

---

*Last updated: 2026-03-20 — v0.1.0 released. Phases 1-7a complete.*

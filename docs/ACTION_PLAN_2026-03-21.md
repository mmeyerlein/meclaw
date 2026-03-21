# MeClaw Action Plan — BRAIN.md Gap Analysis & Roadmap

> **Version:** v0.3.0 (2026-03-21)
> **Basis:** BRAIN.md Architecture vs. Implementation Status
> **Benchmark:** 3/3 (100%) auf 3 Fragen mit Re-Ranking (vorher 1/3)
> **Full Benchmark (500 Fragen v0.2.0):** 12% Judge, 24.4% Substring — identisch mit v0.1.0
> **Root Cause:** Duplikate, falsche Timestamps, spurious Rewards, kein Re-Ranking
> **Status nach Fixes:** Alle 12 BRAIN.md Features implementiert, 3 kritische Bugs gefixt

---

## All Completed Phases

### v0.1.0 → v0.2.0 (Brain Activation)

| Phase | Feature | Commit |
|-------|---------|--------|
| ✅ A1 | Temporal Edges — Event→Event chain in AGE | `54cf0d7` |
| ✅ A2 | Entity→Event AGE Links — INVOLVED_IN edges | `2f32d16` |
| ✅ A3 | Graph-Based Retrieval — Cypher traversal in retrieve_bee | `4eec880` |
| ✅ B1 | Trigger Chain — extract→novelty(async)→feedback | `cf8aaa6` |
| ✅ B2 | CTM Retrieval — tick-based embedding drift (optional Stage 4) | `21e56e4` |
| ✅ C1 | Fact-Augmented Keys — facts_text + dual BM25 index | `dfcf013` |
| ✅ C2 | Prototypes — seed, centroid computation, novelty-driven creation | `00758c3` |
| ✅ C3 | User Modeling — preference extraction, auto-create sender entities | `1077d45` |
| ✅ D1 | Evaluator Fix — dual judge (context + answer), no contamination | `d45d63f` |

### v0.2.0 → v0.3.0 (Wave 1 + 2 + Bug Fixes)

| Phase | Feature | Commit |
|-------|---------|--------|
| ✅ E1 | 6-Signal Weighted Ranking (similarity, reward, novelty, recency, personality_fit, graph_distance) | `7307346` |
| ✅ E2 | Reward Propagation — γ=0.9, depth 5, single UPDATE statement | `330ed98` |
| ✅ E3 | Decision Traces + Citation Tracking + CITES edges + trending/stale views | `d2996b9`, `01d135a` |
| ✅ E4 | ACTIVATES Edges (Event → Prototype) in AGE | `087d410` |
| ✅ E5 | ASSOCIATION Edges (Prototype → Prototype) in AGE | `087d410` |
| ✅ E6 | Prototype Mitosis — detect + split + run_mitosis | `d2996b9` |
| ✅ E7 | CITATION Edges (Decision → Event) — included in E3 | `d2996b9` |
| ✅ E8 | MemCell Nodes — boundary detection + BELONGS_TO edges | `d2996b9`, `01d135a` |
| ✅ E9 | LLM-Guided Re-Ranking (Stage 3) — gpt-4o-mini, ~$0.35/500 questions | `d8ee5e7` |
| ✅ E12 | Stale Precedent Detection — included in E3 views | `d2996b9` |

### Bug Fixes (Critical)

| Fix | Description | Commit |
|-----|-------------|--------|
| 🔧 | Duplicate brain_events — trigger + manual extract_bee call | `bd72af6` |
| 🔧 | Trigger extracted llm_result → now only user_input | `bd72af6` |
| 🔧 | brain_events.created_at was NOW() instead of message timestamp | `bd72af6` |
| 🔧 | pg_background worker exhaustion — exception handler | `8120961` |
| 🔧 | Spurious rewards from feedback_bee in benchmark — reset before retrieval | `bd72af6` |
| 🔧 | Runner v2 — full signal pipeline (embed, extract, novelty, graph, facts) | `8f85c30` |

---

## Remaining Features (Not in BRAIN.md Gaps)

| Prio | Feature | Status | Notes |
|------|---------|--------|-------|
| 🟢 | E10: Cross-Agent Retrieval | Code exists (Phase 5), not tested | Multi-Agent — aktuell nur 1 Agent aktiv |
| 🟢 | E11: Workspace Agent | Entity stub exists | Multi-Agent — Voraussetzung für Team-Memory |
| 🟡 | Dedizierter Reranker | TODO | BGE/Jina self-hosted, 20-40x schneller als LLM, gpt-4o-mini besser für temporale Queries |
| 🟡 | LLM Extraction im Benchmark | Available (no --skip-extraction) | ~$10-15 für 500 Fragen, alle 6 Signale aktiv |
| 🟡 | Query Expansion | Not implemented | LLM reformuliert Query vor Retrieval |

---

## Benchmark History

| Version | Substrate Match | Judge | Notes |
|---------|----------------|-------|-------|
| v0.1.0 | 22.4% | 12.0% | Baseline (raw BM25+Vector) |
| v0.2.0 | 24.4% | 12.0% | No improvement (bugs masked all features) |
| v0.3.0 (3 Fragen) | 33% | **100%** | With Re-Ranking + bug fixes, all 3 correct |
| v0.3.0 (500 Fragen) | TBD | TBD | Pending |

---

## SQL Files (45 total)

```
01_extensions → 09_age_graph       Core infrastructure
10_seed → 15_llm_providers         Seeding & providers
16_brain_schema → 18_seed_agents   Brain tables & agents
19_extract_bee → 22_embedding_bee  Extract & embed pipeline
23_novelty_bee → 24_feedback_bee   Learning signals
25_phase3_graph_intelligence       Graph helpers
26_consolidation_bee               Nightly consolidation
27_ctm_retrieval                   CTM v1
28_extract_bee_v2                  LLM entity extraction
29_phase7_robustness               Robustness & caching
30_smoke_tests                     Test suite
31_phase8_swarm                    Swarm foundation
32_phase9_context_pipeline         Context compression
33_phase10_tests                   Extended tests
34_temporal_edges                  A1: Temporal edges + extract_bee v3
35_fact_keys                       C1: facts_text + BM25
36_trigger_chain                   B1: Full trigger chain
37_ctm_retrieval_v2                B2+E1: CTM + 6-Signal Ranking
38_prototypes_activation           C2: Prototype seeding
39_user_modeling                   C3: Preference extraction
40_e2_reward_propagation           E2: Discounted reward propagation
41_e4_e5_graph_edges               E4+E5: ACTIVATES + ASSOCIATED
42_e3_decision_traces              E3+E7+E12: Traces + Citations + Stale
43_e6_prototype_mitosis            E6: Prototype splitting
44_e8_memcell_nodes                E8: Conversation boundaries
45_e9_llm_reranking                E9: LLM Stage 3 re-ranking
```

---

*Erstellt: 2026-03-21 | Aktualisiert: 2026-03-21 18:55*

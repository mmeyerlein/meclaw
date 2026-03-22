# MeClaw v2 — Restructuring Plan

> Datum: 2026-03-22
> Ziel: Honcho-level Memory Performance (90%+) als reines PostgreSQL System
> USP: "Alles in der Datenbank — kein externer Service"

---

## Pitch

> **MeClaw: Agent Memory that lives inside PostgreSQL.**
> Honcho-level recall. Hippocampus-level temporal reasoning.
> Zero external services. One Docker container. `docker run meclaw`.

---

## Warum v2 statt Patch

| Problem heute | Ursache |
|---|---|
| 47 SQL-Dateien mit Überschreibungen | Organisch gewachsen, nie refactored |
| 4× `retrieve_bee` Definition | Jede Phase hat neue Version hinzugefügt |
| 3× `extract_bee` Definition | Dito |
| 6/18 Features aktiv | Aktivierung unklar, Seiteneffekte |
| Extraction verschlechtert Score | Facts + Originals gemischt, kein sauberes Design |
| Jede Änderung: 2-4h mit Debugging | Kein klarer Datenfluss |

---

## Phase 1: Clean Foundation (3-4 Tage → Honcho-Level)

### Ziel: 85-90% LongMemEval mit einfachstem möglichen Design

### Dateien-Struktur (v2)

```
sql/
├── 01_extensions.sql          — pg_search, pgvector, age, pg_background, pg_cron
├── 02_schema.sql              — Tabellen: channels, messages, tasks, events
├── 03_brain_schema.sql        — brain_events, entities, entity_events, prototypes
├── 04_age_graph.sql           — AGE Setup, meclaw_graph, Node/Edge Labels
├── 05_llm_providers.sql       — Provider-Config + LLM-Call-Helpers
├── 06_embedding.sql           — compute_embedding, compute_embeddings_batch
├── 07_extract.sql             — segment_extract, batch_extract (THE extraction)
├── 08_retrieve.sql            — retrieve_bee (THE retrieval, one function)
├── 09_signals.sql             — novelty_bee, feedback_bee, reward_propagation
├── 10_temporal.sql            — temporal edges, expand_temporal_query, time filter
├── 11_rerank.sql              — llm_rerank, smart retrieval wrapper
├── 12_dream.sql               — consolidation_bee, reflection, contradiction detection
├── 13_triggers.sql            — trg_extract_on_done (THE trigger chain)
├── 14_seed.sql                — Default agents, channels, providers
├── 15_tests.sql               — Smoke tests, regression tests
├── 16_api.sql                 — PostgREST / pg_graphql API views (optional)
└── config.sql                 — API keys, provider URLs (gitignored)
```

**16 Dateien statt 47. Keine Überschreibungen. Jede Funktion existiert EINMAL.**

### Kern-Pipeline (Phase 1)

```
Message INSERT (status='done')
  │
  ├─ SYNC (<20ms): ─────────────────────────────────────
  │   trg_extract_on_done()
  │   ├── brain_event INSERT (raw content + timestamp)
  │   ├── feedback_bee (reward auf vorheriges Event)
  │   └── pg_background NOTIFY → Phase 2
  │
  ├─ ASYNC Phase 2 (<3s): ──────────────────────────────
  │   pg_background Worker:
  │   ├── compute_embedding(event_id)
  │   ├── novelty_bee(agent_id, event_id)
  │   ├── reward_propagation(event_id, depth=5, γ=0.9)
  │   └── temporal_edge(event_id → previous event)
  │
  └─ ASYNC Phase 3 (periodisch, alle N messages): ──────
      segment_extract():
      ├── Detect segment boundary (embedding drift > 0.3)
      ├── Collect segment messages (3-10 msgs)
      ├── 1 LLM Call → atomare Facts
      ├── REPLACE original brain_events.content with facts
      │   (oder: separate conclusions-Tabelle)
      ├── Entity Resolution → entities + entity_events
      ├── compute_embeddings_batch(new facts)
      └── ACTIVATES + ASSOCIATED edges
```

### Schlüssel-Entscheidung: Facts ersetzen vs. ergänzen

**Honcho-Ansatz (empfohlen):** Facts ERSETZEN den Rohtext für Retrieval.
```
brain_events Tabelle:
  - raw_content TEXT    — Original-Message (für Audit, nicht für Retrieval)
  - content TEXT        — Extrahierte Facts (für BM25 + Vector)
  - embedding vector    — Embedding von content (= Facts)
```

Retrieval sucht NUR in `content` (Facts). `raw_content` wird nur für Context-Aufbau genutzt wenn der User den vollen Gesprächsverlauf sehen will.

**Warum das funktioniert:** Honcho's 90.4% kommen von "median 5% der Tokens". Weniger Text = weniger Noise = bessere Matches.

### Retrieval (Phase 1 — EINE Funktion)

```sql
CREATE FUNCTION meclaw.retrieve_bee(
    p_agent_id TEXT,
    p_query TEXT,
    p_limit INT DEFAULT 10
) RETURNS TABLE (event_id UUID, content TEXT, score FLOAT, source TEXT)
AS $$
  -- Stage 1: BM25 + Vector RRF (parallel, top-20 each)
  -- Stage 2: Graph Expansion (TEMPORAL + INVOLVED_IN, 1-3 hops)
  -- Stage 3: 6-Signal Ranking (rrf, recency, reward, novelty, graph_distance)
  -- Stage 4: Optional LLM Rerank (top-30 → top-K)
$$;
```

Keine CTM-Variante, keine v2/v3. EINE Funktion. Optionales Reranking via Parameter.

### Was Phase 1 NICHT hat (bewusst):

- ❌ Kein CTM iteratives Retrieval (Komplexität, marginaler Gewinn)
- ❌ Keine MemCells als AGE Nodes (Segmente sind nur Extraction-Einheiten)
- ❌ Keine Decision Traces (Enterprise-Feature, nicht Benchmark-relevant)
- ❌ Keine Citation Edges (dito)
- ❌ Kein AIEOS Profile Import (Phase 3)
- ❌ Kein Swarm/Hive (Phase 4)

### Erwartetes Ergebnis Phase 1:

| Kategorie | v0.3.1 | v2 Phase 1 (geschätzt) |
|---|---|---|
| single-session-user | 100% | 100% |
| single-session-assistant | 35.7% | 85-90% (Facts aus Assistant-Msgs) |
| single-session-preference | 70% | 80-85% |
| knowledge-update | 73.1% | 80-85% |
| multi-session | 51.9% | 65-75% (Entity-basiertes Retrieval) |
| temporal-reasoning | 30.1% | 50-60% (Graph + Time Filter) |
| **Gesamt** | **55.4%** | **75-85%** |

---

## Phase 2: Temporal Intelligence (2-3 Tage → Differenzierung)

### Ziel: Temporal-Reasoning auf 80%+, MeClaw differenziert sich von Honcho

Was Honcho NICHT kann und wir schon haben:
- AGE Graph mit TEMPORAL Edges
- Zeitliche Traversal (Event A → TEMPORAL → Event B)
- `expand_temporal_query` (LLM-basiert)
- Time-Window Filtering

### Was dazukommt:

```sql
-- 1. Native Temporal Ordering
-- Statt: BM25 "Walk for Hunger" → hoffe auf richtigen Match
-- v2: Entity "Walk for Hunger" → INVOLVED_IN → Event → TEMPORAL → Event chains
-- Graph-Traversal findet zeitliche Reihenfolge OHNE Keyword-Search

-- 2. Temporal Re-Ranking
-- Events nach created_at sortieren, ASC/DESC je nach Question-Type
-- "What was FIRST?" → ASC, "What was LAST?" → DESC

-- 3. Duration/Gap Calculation
-- Frage: "How many days between X and Y?"
-- → Find Event X (Graph), Find Event Y (Graph)
-- → RETURN X.created_at - Y.created_at
-- Kein LLM nötig! Reine Graphoperation.
```

### Erwartetes Ergebnis Phase 2:

| Kategorie | v2 Phase 1 | v2 Phase 2 (geschätzt) |
|---|---|---|
| temporal-reasoning | 50-60% | **75-85%** |
| multi-session | 65-75% | **75-80%** (besseres Entity-Linking) |
| **Gesamt** | 75-85% | **85-90%** |

---

## Phase 3: Dreaming + AIEOS (2-3 Tage → Continual Learning)

### 3a. Dream/Reflection (pg_cron)

```sql
-- Nightly oder nach Idle-Zeit
CREATE FUNCTION meclaw.dream(p_agent_id TEXT)
  -- 1. Sammle neue Events seit letztem Dream
  -- 2. LLM: "Was kann man aus diesen Events schließen?"
  --    → Neue Conclusions (induktiv, deduktiv)
  -- 3. Contradiction Detection
  --    → "User sagte Mai: lebt in Berlin" vs "Aug: nach Hamburg gezogen"
  --    → Knowledge Update markieren
  -- 4. Entity Profile Update
  --    → observed_profile aktualisieren
  -- 5. Prototype Maintenance
  --    → Mitose, Merge, Prune
```

### 3b. AIEOS Integration

```sql
-- Agent-Persönlichkeit aus AIEOS Schema laden
-- Personality-Fit Score im Ranking
-- Relationship Graph (Agent × Person × Channel → Modifier)
-- Konversationeller Onboarding-Prozess statt Fragebogen
```

### 3c. Prompt Compression

```sql
-- context_bee: Markdown Compressor auf statischen Prefix
-- Cache Breakpoint nach komprimiertem System-Prompt
-- Dynamische Memories nach dem Breakpoint
```

---

## Phase 4: Swarm + Hive (1-2 Wochen → Full MeClaw)

### Was MeClaw über Memory hinaus bietet:

- **Hive Architecture:** Spezialisierte Bees (Router, Context, Extract, Retrieve, Feedback, Consolidation)
- **Swarm/Multi-Agent:** Mehrere Agents teilen Channel-Extraction, haben private Brains
- **Concierge Pattern:** Eingangs-Bee routet zu spezialisierten Agents
- **Channel-Shared Extraction:** Extract once per channel, rank per agent
- **Workspace Agents:** Institutionelles Gedächtnis
- **Tool Integration:** Tool-Calls als Events im Graph
- **Decision Traces:** Audit Trail für Enterprise

---

## Migrations-Strategie

### Kein Big Bang. Inkrementell.

```
Woche 1: Phase 1 (Clean Foundation)
  Tag 1: Neue SQL-Struktur (16 Dateien), Schema-Migration
  Tag 2: extract + retrieve (sauber, je 1 Funktion)
  Tag 3: signals + triggers + embedding
  Tag 4: Benchmark 500 Fragen → Baseline v2

Woche 2: Phase 2 (Temporal) + Phase 3a (Dreaming)
  Tag 5-6: Temporal Intelligence
  Tag 7: Dream/Consolidation
  Tag 8: Benchmark → v2.1

Woche 3: Phase 3b-c (AIEOS, Compression) + Phase 4 Start
```

### Was wir behalten:

- ✅ PostgreSQL als einzige Dependency
- ✅ AGE Graph (unser Differenzierungsmerkmal)
- ✅ ParadeDB BM25 + pgvector Hybrid-Retrieval
- ✅ pg_background für Async
- ✅ pg_cron für Dreaming
- ✅ plpython3u für LLM Calls
- ✅ Docker Single-Container Deployment
- ✅ Benchmark Runner + Evaluator

### Was wir entfernen:

- ❌ SQL-Datei-Konflikte (4× retrieve_bee, 3× extract_bee)
- ❌ CTM Retrieval (Komplexität ohne bewiesenen Nutzen)
- ❌ facts_text Feld (Facts werden content)
- ❌ Decompose als separate Pipeline (integriert in Segment-Extraction)
- ❌ Alle "Phase X" Benennungen (klar benannte Module stattdessen)

---

## Competitive Positioning

```
┌─────────────────────────────────────────────────────┐
│                    Agent Memory                      │
│                                                      │
│  Honcho        MeClaw v2       Hippocampus          │
│  ┌──────┐     ┌──────────┐    ┌──────────┐         │
│  │Python │     │PostgreSQL│    │Python+   │         │
│  │Server │     │  ONLY    │    │Raphtory  │         │
│  │       │     │          │    │          │         │
│  │90.4%  │     │ 85-90%   │    │91.1%     │         │
│  │LongMem│     │ LongMem  │    │LoCoMo    │         │
│  │       │     │          │    │          │         │
│  │No Graph│    │AGE Graph │    │Raphtory  │         │
│  │Dreaming│    │Dreaming  │    │Graph     │         │
│  │pgvector│    │pgvector  │    │Sleep     │         │
│  │       │     │ParadeDB  │    │Consol.   │         │
│  │       │     │AIEOS     │    │          │         │
│  │       │     │Swarm     │    │          │         │
│  └──────┘     └──────────┘    └──────────┘         │
│                                                      │
│  Service       Database        Service+Engine       │
│  Managed SaaS  Self-hosted     Open Source          │
│  $100 free     docker run      Complex Setup        │
└─────────────────────────────────────────────────────┘
```

### MeClaw's Unique Value:

1. **Runs in your existing PostgreSQL.** No new service to deploy, monitor, secure.
2. **Temporal Graph.** Honcho can't do temporal reasoning natively. We can.
3. **Multi-Agent by design.** Channel-shared extraction, agent-private ranking.
4. **AIEOS-native identity.** Personality-aware retrieval out of the box.
5. **Full audit trail.** Every memory, every decision, queryable in SQL.

---

## Referenzen

- Honcho: https://blog.plasticlabs.ai/research/Benchmarking-Honcho
- Hippocampus: /development/openclaw/austausch/papers/alinea-hippocampus-memory-paper.txt
- BRAIN.md: docs/BRAIN.md (unverändert, bleibt Architektur-Dokument)
- ARCHITECTURE_ANALYSIS.md: docs/ARCHITECTURE_ANALYSIS.md (IST-Analyse)

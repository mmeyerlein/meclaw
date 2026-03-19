# BRAIN.md — Memory Hive Architecture

> "The LLM is the policy network in a model-free episodic RL setup, but it never gets a gradient update because all the learning happens in the memory layer."
> — Hippocampus LoCoMo Paper

---

## Vision

MeClaw's Memory Hive is a **continual learning system** built entirely in PostgreSQL.
It does not just store and retrieve — it learns which memories matter, and gets better over time.

The LLM stays untouched. All learning happens in the database.

**Key insight:** Memory systems store but don't learn. Fine-tuning learns but loses facts. MeClaw does both — online, in the flow of conversation, without gradient updates.

---

## Theoretical Foundation

### Complementary Learning Systems (CLS)

Inspired by biological memory:

- **Fast Learner (Hippocampus):** Records experiences in a single exposure. High-fidelity episodic traces. Implemented as the `extract_bee` + AGE graph.
- **Slow Learner (Cortex):** Extracts patterns, builds prototypes, bridges memory to the LLM. Implemented as the `consolidation_bee` (pg_cron, nightly).

### Why Graph > RAG

Standard RAG: `Similarity(query, memory) → rank`

Memory Hive: `Similarity × Novelty × Reward × Recency × GraphDistance → rank`

A correction from 6 months ago with high negative reward surfaces alongside yesterday's events — because the system knows it mattered.

### Why This Beats EverMemOS / Mem0 / Zep

| System | Stores | Retrieves | Learns |
|--------|--------|-----------|--------|
| Mem0, Zep | ✅ | ✅ similarity only | ❌ |
| EverMemOS | ✅ | ✅ hybrid BM25+vector | ❌ |
| MeClaw Memory Hive | ✅ | ✅ value-aware graph traversal | ✅ |

---

## Architecture: The Memory Hive

Five specialized Bees, one PostgreSQL database.

```
Incoming Message
      ↓
  extract_bee ──────────────────────────────────────→ AGE Graph
      │                                               (Entities, Events,
      ↓                                                Relations, Prototypes)
  novelty_bee ─── pgvector distance ──────────────→ novelty score on message
      │
      ↓
[Next Turn Starts]
      │
  feedback_bee ─── sentiment of user reply ───────→ retroactive reward on prior event
      │
      ↓
  retrieve_bee ─── CTM-style iterative retrieval ─→ ranked context for LLM
      │             (pg_search + pgvector + AGE traversal)
      ↓
   LLM Bee (unchanged, policy network)
      │
      ↓
[Nightly - pg_cron]
  consolidation_bee ─ prune weak edges ───────────→ AGE Graph Updated
                     ─ merge compatible prototypes──→
                     ─ recalibrate Hebbian weights──→
                     ─ split conflicting prototypes (mitosis)
```

---

## The Graph: AGE as Temporal Knowledge Graph

### Node Types

```cypher
(:Entity)      -- Person, Project, Tool, Concept (canonical IDs)
(:Event)       -- Ein Gespräch / Turn / Aktion (immutable)
(:Prototype)   -- Emergentes Konzept aus Mustern
(:Decision)    -- Entscheidung mit vollständigem Trace
(:MemCell)     -- Boundary-detected Gesprächs-Chunk
```

### Edge Types (5 semantische Typen)

```cypher
-- 1. TEMPORAL: Reihenfolge ist Struktur
(:Event)-[:TEMPORAL {seq: INT, at: TIMESTAMP}]->(:Event)

-- 2. ACTIVATION: Welches Konzept wurde aktiviert
(:Event)-[:ACTIVATES {weight: FLOAT}]->(:Prototype)

-- 3. ASSOCIATION: Hebbian Co-Aktivierung
(:Prototype)-[:ASSOCIATED {weight: FLOAT, updated_seq: INT}]->(:Prototype)

-- 4. ENTITY: Wer/Was war dabei (hard links, präzise)
(:Entity)-[:INVOLVED_IN]->(:Event)

-- 5. CITATION: Entscheidungs-Audit-Trail
(:Decision)-[:CITES {authority: INT, at: TIMESTAMP}]->(:Event)
```

### Entity Resolution

Canonical IDs mit Alias-Mapping:
```sql
-- Entitäts-Tabelle mit Aliases
CREATE TABLE meclaw.entities (
    id TEXT PRIMARY KEY,           -- 'meclaw:person:marcus-meyer'
    canonical_name TEXT,
    aliases TEXT[],                -- ['Marcus', 'Marcus Meyer', 'mm']
    entity_type TEXT,              -- 'person', 'project', 'tool', 'concept'
    created_seq BIGINT,
    vector vector(1536)            -- pgvector für fuzzy entity matching
);
```

"Marcus Meyer", "Marcus" und "mm" → alle resolven zu `meclaw:person:marcus-meyer`.

---

## Value-Aware Memory: Reward System

### Drei Reward-Quellen

**1. Novelty (Intrinsic Curiosity)**
```sql
-- Novelty = Abstand zum nächsten bekannten Prototype
novelty = 1 - MAX(cosine_similarity(new_embedding, prototype.vector))
-- Unbekanntes bekommt bis zu 5× Gewicht
```

**2. Implicit Feedback (Temporal Difference Learning)**
```
Turn N:   Walter sagt X
Turn N+1: Marcus sagt "genau richtig!" → Turn N reward += 0.8
Turn N+1: Marcus sagt "nein, falsch"   → Turn N reward -= 0.8
```
Sentiment-Analyse des *nächsten* Turns, rückwirkend auf vorherigen Event angewendet.

**3. Explicit Feedback**
```sql
-- Approval/Rejection eines Proposals
UPDATE meclaw.messages SET reward = reward + 10.0 WHERE id = $decision_id;  -- approved
UPDATE meclaw.messages SET reward = reward - 5.0  WHERE id = $decision_id;  -- rejected
```

### Reward Propagation (Discounted Returns)

```sql
-- Rückwärts propagieren: frühe Events in einer erfolgreichen Kette bekommen Credit
UPDATE meclaw.messages m
SET reward = reward + (outcome_reward * POW(0.9, seq_distance))
WHERE id IN (SELECT id FROM event_chain WHERE terminal_event = $success_event);
```

### Retrieval Ranking

```sql
ORDER BY (
    semantic_similarity * 0.35 +
    reward              * 0.30 +
    novelty             * 0.20 +
    recency             * 0.15
) DESC
```

---

## Retrieval: CTM-Style Iterative Graph Traversal

Inspired by the Continuous Thought Machine (Darlow et al., 2025):

### Tick-Based Retrieval

```
Tick 1: Query embedding → Prototypes aktivieren (Standard pgvector)
Tick 2: Top-Prototypes blend in query embedding → driftet zu relevantem Konzeptbereich
Tick 3: Konvergenz check (Entropie < threshold) → fertig
        Oder weiterer Drift...
```

Simple queries: 1 Tick. Ambiguous/complex queries: 2-3 Ticks.
**Adaptive compute — Tiefe entsteht aus Schwierigkeit, nicht aus fixem Parameter.**

### Stage 1: Fast Retrieval (parallel)

```sql
-- BM25 via pg_search
SELECT id, score FROM meclaw.memories
WHERE memories @@@ paradedb.phrase('content', $query)
LIMIT 20;

-- Semantic via pgvector
SELECT id, 1 - (embedding <=> $query_vec) as score FROM meclaw.memories
ORDER BY embedding <=> $query_vec LIMIT 20;

-- RRF Fusion
SELECT id, SUM(1.0 / (60 + rank)) as rrf_score
FROM (...) GROUP BY id ORDER BY rrf_score DESC LIMIT 20;
```

### Stage 2: Graph Expansion (AGE Cypher)

```cypher
-- Anchor Events finden, dann Graph traversieren
MATCH path = (anchor:Event {id: $anchor_id})
             -[:TEMPORAL|ACTIVATION|ASSOCIATION*1..3]->
             (related)
WHERE related.value_score > 0.3
RETURN related, length(path) as hops
ORDER BY hops, related.value_score DESC
```

Multi-hop Antworten entstehen am Ende von Ketten — nicht in einzelnen Dokumenten.

### Stage 3: LLM-Guided Re-ranking (optional)

Bei komplexen Queries: LLM liest Cluster-Summaries und entscheidet welche Kandidaten wirklich relevant sind. Günstig (Summaries, nicht Rohdaten), aber dramatisch präziser.

---

## Prototype Engine (PDP Layer)

### Prototypen entstehen aus Mustern

Wenn eine neue Observation zu keinem bestehenden Prototype passt (Novelty > 0.7):
```sql
INSERT INTO meclaw.prototypes (id, centroid, weight, value_stats)
VALUES ($new_id, $embedding, 1.0, '{}');
```

### Hebbian Learning: "Neurons that fire together, wire together"

```sql
-- Co-Aktivierung zweier Prototypes → Association-Edge stärken
UPDATE meclaw.prototype_associations
SET weight = weight + 0.1 * activation_a * activation_b
WHERE prototype_a = $pa AND prototype_b = $pb;
```

### Prototype Mitosis (Widerspruchs-Handling)

Wenn `gisela` mit `erfolg` (reward: +5) UND `einsamkeit` (reward: -2) assoziiert:
```cypher
-- Prototype splittet in zwei sub-Konzepte
MATCH (p:Prototype {id: 'gisela'})
CREATE (p1:Prototype {id: 'gisela-product', parent: 'gisela'})
CREATE (p2:Prototype {id: 'gisela-mission', parent: 'gisela'})
-- Edges umverteilen nach value_signal
```

---

## Sleep Consolidation (Nightly - pg_cron)

```sql
-- 1. Schwache Associations prunen
DELETE FROM meclaw.prototype_associations WHERE weight < 0.1;

-- 2. Ähnliche Prototypes mergen (compatible values)
-- Cosine similarity > 0.92 AND reward_stats compatible → merge

-- 3. Hebbian Gewichte rekalibrieren gegen Temporal-Graph Ground Truth

-- 4. Stale Precedents markieren
UPDATE meclaw.decisions
SET is_stale = true
WHERE last_cited_seq < current_seq - 1000  -- lang nicht mehr zitiert
AND was_once_influential = true;
```

### Citation Authority Curves

```sql
-- Trending precedent: wird Standard-Praxis
SELECT decision_id,
  COUNT(*) FILTER (WHERE cited_seq > now_seq - 50) as recent_citations,
  COUNT(*) FILTER (WHERE cited_seq < now_seq - 50) as old_citations
FROM meclaw.citations
GROUP BY decision_id
HAVING recent_citations > old_citations * 2;

-- Stale precedent: niemand folgt mehr
SELECT decision_id FROM meclaw.citations
GROUP BY decision_id
HAVING MAX(cited_seq) < now_seq - 500;
```

---

## Schema (Kern-Tabellen)

```sql
-- Events (immutable, append-only)
CREATE TABLE meclaw.brain_events (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    seq BIGINT GENERATED ALWAYS AS IDENTITY,
    message_id UUID REFERENCES meclaw.messages(id),
    content TEXT,
    embedding vector(1536),      -- pgvector
    reward FLOAT DEFAULT 0.0,
    novelty FLOAT DEFAULT 0.0,
    reward_updated_seq BIGINT DEFAULT 0,
    created_at TIMESTAMPTZ DEFAULT NOW()
);

-- Prototypes (emergente Konzepte)
CREATE TABLE meclaw.prototypes (
    id TEXT PRIMARY KEY,
    centroid vector(1536),
    weight FLOAT DEFAULT 1.0,
    activation_count INT DEFAULT 0,
    value_mean FLOAT DEFAULT 0.0,
    value_variance FLOAT DEFAULT 0.0,
    last_activated_seq BIGINT DEFAULT 0,
    created_seq BIGINT
);

-- Prototype Associations (Hebbian)
CREATE TABLE meclaw.prototype_associations (
    prototype_a TEXT REFERENCES meclaw.prototypes(id),
    prototype_b TEXT REFERENCES meclaw.prototypes(id),
    weight FLOAT DEFAULT 0.0,
    last_updated_seq BIGINT,
    PRIMARY KEY (prototype_a, prototype_b)
);

-- Entities (canonical IDs)
CREATE TABLE meclaw.entities (
    id TEXT PRIMARY KEY,
    canonical_name TEXT NOT NULL,
    aliases TEXT[],
    entity_type TEXT,
    embedding vector(1536),
    created_seq BIGINT
);

-- Decision Traces (immutable audit trail)
CREATE TABLE meclaw.decision_traces (
    id UUID PRIMARY KEY,
    seq BIGINT,
    query TEXT,
    evidence_ids UUID[],
    prototypes_activated TEXT[],
    q_value_estimates JSONB,
    action_taken TEXT,
    reward FLOAT DEFAULT 0.0,
    created_at TIMESTAMPTZ DEFAULT NOW()
);
```

---

## The Five Bees

### 1. extract_bee
**Trigger:** Nach jedem LLM-Turn
**Funktion:**
- LLM extrahiert Entities, Events, Relations aus dem Gespräch
- Entity Resolution gegen bestehende Entitäten
- Neue Nodes und Edges in AGE Graph
- Embedding berechnen (pgvector)
- Temporal-Edge zur vorherigen Event-Node

### 2. novelty_bee
**Trigger:** Nach extract_bee
**Funktion:**
- Abstand neues Event-Embedding zu nächstem Prototype
- novelty = 1 - max_cosine_similarity
- Update `brain_events.novelty`
- Ggf. neues Prototype anlegen wenn novelty > threshold

### 3. feedback_bee
**Trigger:** Beginn jedes Turns
**Funktion:**
- Sentiment-Analyse der User-Nachricht
- Positiv ("genau!", "danke", "perfekt") → vorheriger Event reward += 0.8
- Negativ ("falsch", "nein", "nicht richtig") → vorheriger Event reward -= 0.8
- Reward rückwärts durch Event-Chain propagieren (discounted)

### 4. retrieve_bee
**Trigger:** Vor jedem LLM-Call (context_bee integration)
**Funktion:**
- Stage 1: pg_search + pgvector + RRF → Top-20
- Stage 2: AGE Graph Expansion (1-3 Ticks, CTM-style)
- Stage 3: Value-aware Ranking (similarity × reward × novelty × recency)
- Output: Top-5 Memories für Kontext-Injection

### 5. consolidation_bee
**Trigger:** pg_cron, täglich 03:00 UTC
**Funktion:**
- Schwache Association-Edges prunen
- Compatible Prototypes mergen
- Conflicting Prototypes splitten (Mitosis)
- Hebbian Gewichte rekalibrieren
- Stale Precedents markieren
- Citation Authority Curves berechnen

---

## Implementation Plan (Iterativ)

### Phase 1 — Foundation (Minimal Viable Memory)
- [ ] Schema: `brain_events`, `entities` Tabellen
- [ ] `extract_bee`: Entities + Events in AGE + pgvector Embedding
- [ ] `retrieve_bee`: Stage 1 nur (pg_search + pgvector + RRF)
- [ ] Integration in context_bee

### Phase 2 — Learning
- [ ] `novelty_bee`: Novelty-Score + Prototype-Erstellung
- [ ] `feedback_bee`: Retroaktiver Reward aus User-Reaktion
- [ ] Reward-gewichtetes Ranking in retrieve_bee

### Phase 3 — Graph Intelligence
- [ ] AGE Temporal-Edges (Sequence-Numbers)
- [ ] Graph Expansion in retrieve_bee (Stage 2)
- [ ] Entity Resolution (Aliases, canonical IDs)

### Phase 4 — Consolidation
- [ ] `consolidation_bee` via pg_cron
- [ ] Prototype Mitosis
- [ ] Citation Layer + Authority Curves

### Phase 5 — CTM Retrieval
- [ ] Tick-based iterative Retrieval
- [ ] Adaptive Compute (Entropy als Convergence-Signal)

---

## Key Design Principles

1. **Alles in PostgreSQL** — kein externer Service, keine externe Runtime
2. **Append-only Events** — niemals löschen, Temporal-History ist heilig
3. **Reward als First-Class Data** — jede Memory hat einen Wert, der sich ändert
4. **LLM bleibt unberührt** — das Learning passiert in der Datenbank
5. **Iterativ bauen** — Phase 1 alleine ist schon besser als EverMemOS

---

## References

- Hippocampus Paper: https://sebhunte.vercel.app/blog/hippocampus-locomo (91.1% LoCoMo cumulative)
- EverMemOS: https://github.com/EverMind-AI/EverMemOS (93% LoCoMo isolated)
- Foundation Capital Context Graph Thesis
- Continuous Thought Machine: Darlow et al., 2025, Sakana AI
- LoCoMo Benchmark: Maharana et al., 2024
- Complementary Learning Systems Theory

---

*BRAIN.md ist das Architektur-Dokument für den MeClaw Memory Hive.*
*Stand: 2026-03-19 — Basis für alle brain_bee Implementierungen.*

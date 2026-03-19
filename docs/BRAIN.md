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
(:Entity)      -- Person, project, tool, or concept (canonical IDs)
(:Event)       -- A conversation turn or action (immutable)
(:Prototype)   -- An emergent concept derived from patterns
(:Decision)    -- A decision with a complete audit trace
(:MemCell)     -- A boundary-detected conversation chunk
```

### Edge Types (5 Semantic Types)

```cypher
-- 1. TEMPORAL: ordering is structure
(:Event)-[:TEMPORAL {seq: INT, at: TIMESTAMP}]->(:Event)

-- 2. ACTIVATION: which concept was triggered
(:Event)-[:ACTIVATES {weight: FLOAT}]->(:Prototype)

-- 3. ASSOCIATION: Hebbian co-activation
(:Prototype)-[:ASSOCIATED {weight: FLOAT, updated_seq: INT}]->(:Prototype)

-- 4. ENTITY: who or what was involved (hard links, precise)
(:Entity)-[:INVOLVED_IN]->(:Event)

-- 5. CITATION: decision audit trail
(:Decision)-[:CITES {authority: INT, at: TIMESTAMP}]->(:Event)
```

### Entity Resolution

Canonical IDs with alias mapping:
```sql
-- Entity table with aliases
CREATE TABLE meclaw.entities (
    id TEXT PRIMARY KEY,           -- 'meclaw:person:marcus-meyer'
    canonical_name TEXT,
    aliases TEXT[],                -- ['Marcus', 'Marcus Meyer', 'mm']
    entity_type TEXT,              -- 'person', 'project', 'tool', 'concept'
    created_seq BIGINT,
    vector vector(1536)            -- pgvector for fuzzy entity matching
);
```

"Marcus Meyer", "Marcus", and "mm" all resolve to `meclaw:person:marcus-meyer`.

---

## Value-Aware Memory: Reward System

### Three Reward Sources

**1. Novelty (Intrinsic Curiosity)**
```sql
-- Novelty = distance to the nearest known prototype
novelty = 1 - MAX(cosine_similarity(new_embedding, prototype.vector))
-- Unknown information receives up to 5× weight
```

**2. Implicit Feedback (Temporal Difference Learning)**
```
Turn N:   Walter says X
Turn N+1: Marcus says "exactly right!" → Turn N reward += 0.8
Turn N+1: Marcus says "no, that's wrong" → Turn N reward -= 0.8
```
Sentiment analysis of the *next* turn applied retroactively to the previous event.

**3. Explicit Feedback**
```sql
-- Approval or rejection of a proposal
UPDATE meclaw.messages SET reward = reward + 10.0 WHERE id = $decision_id;  -- approved
UPDATE meclaw.messages SET reward = reward - 5.0  WHERE id = $decision_id;  -- rejected
```

### Reward Propagation (Discounted Returns)

```sql
-- Propagate backward: early events in a successful chain receive credit
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
Tick 1: Query embedding → activate prototypes (standard pgvector)
Tick 2: Top prototypes blend into query embedding → drifts toward relevant concept region
Tick 3: Convergence check (entropy < threshold) → done
        Or continue drifting...
```

Simple queries: 1 tick. Ambiguous/complex queries: 2–3 ticks.
**Adaptive compute — depth emerges from query difficulty, not a fixed parameter.**

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
-- Find anchor events, then traverse the graph
MATCH path = (anchor:Event {id: $anchor_id})
             -[:TEMPORAL|ACTIVATION|ASSOCIATION*1..3]->
             (related)
WHERE related.value_score > 0.3
RETURN related, length(path) as hops
ORDER BY hops, related.value_score DESC
```

Multi-hop answers emerge at the end of chains — not inside individual documents.

### Stage 3: LLM-Guided Re-ranking (optional)

For complex queries: the LLM reads cluster summaries and decides which candidates are truly relevant. Cheap (summaries, not raw data), but dramatically more precise.

---

## Prototype Engine (PDP Layer)

### Prototypes emerge from patterns

When a new observation does not match any existing prototype (novelty > 0.7):
```sql
INSERT INTO meclaw.prototypes (id, centroid, weight, value_stats)
VALUES ($new_id, $embedding, 1.0, '{}');
```

### Hebbian Learning: "Neurons that fire together, wire together"

```sql
-- Co-activation of two prototypes → strengthen the association edge
UPDATE meclaw.prototype_associations
SET weight = weight + 0.1 * activation_a * activation_b
WHERE prototype_a = $pa AND prototype_b = $pb;
```

### Prototype Mitosis (Conflict Handling)

When `gisela` is associated with both `success` (reward: +5) and `loneliness` (reward: -2):
```cypher
-- Prototype splits into two sub-concepts
MATCH (p:Prototype {id: 'gisela'})
CREATE (p1:Prototype {id: 'gisela-product', parent: 'gisela'})
CREATE (p2:Prototype {id: 'gisela-mission', parent: 'gisela'})
-- Redistribute edges according to value signal
```

---

## Sleep Consolidation (Nightly - pg_cron)

```sql
-- 1. Prune weak associations
DELETE FROM meclaw.prototype_associations WHERE weight < 0.1;

-- 2. Merge similar prototypes (compatible values)
-- Cosine similarity > 0.92 AND reward_stats compatible → merge

-- 3. Recalibrate Hebbian weights against temporal graph ground truth

-- 4. Mark stale precedents
UPDATE meclaw.decisions
SET is_stale = true
WHERE last_cited_seq < current_seq - 1000  -- not cited in a long time
AND was_once_influential = true;
```

### Citation Authority Curves

```sql
-- Trending precedent: becoming standard practice
SELECT decision_id,
  COUNT(*) FILTER (WHERE cited_seq > now_seq - 50) as recent_citations,
  COUNT(*) FILTER (WHERE cited_seq < now_seq - 50) as old_citations
FROM meclaw.citations
GROUP BY decision_id
HAVING recent_citations > old_citations * 2;

-- Stale precedent: no one follows it anymore
SELECT decision_id FROM meclaw.citations
GROUP BY decision_id
HAVING MAX(cited_seq) < now_seq - 500;
```

---

## Schema (Core Tables)

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

-- Prototypes (emergent concepts)
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
**Trigger:** After every LLM turn
**Function:**
- LLM extracts entities, events, and relations from the conversation
- Entity resolution against existing entities
- New nodes and edges written to the AGE graph
- Embedding computed (pgvector)
- Temporal edge linked to the previous event node

### 2. novelty_bee
**Trigger:** After extract_bee
**Function:**
- Distance from new event embedding to nearest prototype
- novelty = 1 - max_cosine_similarity
- Update `brain_events.novelty`
- Create a new prototype if novelty > threshold

### 3. feedback_bee
**Trigger:** Start of every turn
**Function:**
- Sentiment analysis of the user's message
- Positive ("exactly!", "thanks", "perfect") → prior event reward += 0.8
- Negative ("wrong", "no", "that's not right") → prior event reward -= 0.8
- Propagate reward backward through event chain (discounted)

### 4. retrieve_bee
**Trigger:** Before every LLM call (context_bee integration)
**Function:**
- Stage 1: pg_search + pgvector + RRF → Top-20
- Stage 2: AGE graph expansion (1–3 ticks, CTM-style)
- Stage 3: Value-aware ranking (similarity × reward × novelty × recency)
- Output: Top-5 memories injected into context

### 5. consolidation_bee
**Trigger:** pg_cron, daily at 03:00 UTC
**Function:**
- Prune weak association edges
- Merge compatible prototypes
- Split conflicting prototypes (mitosis)
- Recalibrate Hebbian weights
- Mark stale precedents
- Compute citation authority curves

---

## Implementation Plan (Iterative)

### Phase 1 — Foundation (Minimal Viable Memory)
- [ ] Schema: `brain_events`, `entities` tables
- [ ] `extract_bee`: entities + events in AGE + pgvector embeddings
- [ ] `retrieve_bee`: stage 1 only (pg_search + pgvector + RRF)
- [ ] Integration into context_bee

### Phase 2 — Learning
- [ ] `novelty_bee`: novelty score + prototype creation
- [ ] `feedback_bee`: retroactive reward from user reactions
- [ ] Reward-weighted ranking in retrieve_bee

### Phase 3 — Graph Intelligence
- [ ] AGE temporal edges (sequence numbers)
- [ ] Graph expansion in retrieve_bee (stage 2)
- [ ] Entity resolution (aliases, canonical IDs)

### Phase 4 — Consolidation
- [ ] `consolidation_bee` via pg_cron
- [ ] Prototype mitosis
- [ ] Citation layer + authority curves

### Phase 5 — CTM Retrieval
- [ ] Tick-based iterative retrieval
- [ ] Adaptive compute (entropy as convergence signal)

---

## Key Design Principles

1. **Everything in PostgreSQL** — no external services, no external runtime
2. **Append-only events** — never delete; temporal history is sacred
3. **Reward as first-class data** — every memory has a value that changes over time
4. **LLM stays untouched** — all learning happens in the database
5. **Build iteratively** — Phase 1 alone already beats EverMemOS

---

## References

- Hippocampus Paper: https://sebhunte.vercel.app/blog/hippocampus-locomo (91.1% LoCoMo cumulative)
- EverMemOS: https://github.com/EverMind-AI/EverMemOS (93% LoCoMo isolated)
- Foundation Capital Context Graph Thesis
- Continuous Thought Machine: Darlow et al., 2025, Sakana AI
- LoCoMo Benchmark: Maharana et al., 2024
- Complementary Learning Systems Theory

---

*BRAIN.md is the architecture document for the MeClaw Memory Hive.*
*Last updated: 2026-03-19 — the foundation for all brain_bee implementations.*

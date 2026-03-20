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

## Standards & Influences

MeClaw follows established open standards rather than reinventing the wheel:

| Standard | What it defines | How MeClaw uses it |
|----------|----------------|-------------------|
| **AIEOS v1.2** | Portable agent identity (psychology, linguistics, capabilities) | Entity schema for ALL entities (agents, persons, workspaces); neural_matrix drives retrieval personality |
| **AGENTS.md** | Project instructions for coding agents | Input format when working in codebases |
| **Markdown Compressor** | Lossless/lossy token reduction for agent instructions | context_bee static prefix compression |

### AIEOS Integration (AI Entity Object Specification)

AIEOS (https://aieos.org) defines the "Soul Layer" of an entity as structured data. MeClaw adopts this for **all entities** in the graph — not just agents:

- **Neural Matrix** (0.0–1.0): creativity, empathy, logic, adaptability, charisma, reliability — influences retrieval ranking
- **OCEAN Traits**: openness, conscientiousness, extraversion, agreeableness, neuroticism — personality consistency
- **Moral Compass**: alignment, core values, conflict resolution style — behavioral boundaries
- **Linguistics**: formality level, forbidden words, catchphrases, vocabulary level — consistent voice
- **Capabilities**: skills with priority 1–10 — autonomous skill discovery and task orchestration

Every entity — person, agent, workspace — carries AIEOS-compatible identity. A person's neural_matrix can be partially observed over time. An agent's neural_matrix is fully defined at creation. A workspace's neural_matrix defines institutional personality.

---

## Channel Architecture

### Channels as Universal Primitive

Everything flows through channels. A channel is the fundamental communication primitive in MeClaw:

- **External channels:** Telegram, Slack, Web — user-facing message streams
- **Internal channels:** messages table + triggers — inter-bee communication
- **Tool channels:** tool calls and results flow through channels too

A channel is:
- An **append-only message stream** — messages are atomic and belong to exactly one channel
- A **shared extraction cache** — entities/events extracted once per channel, not per agent

### Channel-Level Extraction vs Agent-Level Brain

This is a critical architectural split:

```
Channel Level (shared)                    Agent Level (personal)
─────────────────────                    ─────────────────────
Messages (append-only)                   Rewards on events
extract_bee runs HERE                    Novelty scores
Entities extracted ONCE                  Prototypes & associations
Events extracted ONCE                    Decision traces
Relations extracted ONCE                 Personality-aware ranking
                                         Context pipeline
```

**Why?** If three agents share a channel, the entities in that conversation are extracted once. Each agent then applies its own reward signals, novelty scores, and personality-aware ranking on top of the shared extraction layer. No duplication. No divergent entity graphs for the same conversation.

### Channel Has No Intelligence

A channel does not think, decide, or route. It is a passive stream with an extraction layer. Intelligence lives in the agent's brain — the channel merely provides the raw material.

---

## Scoping Model

The brain operates on a shared knowledge graph with three scoping levels:

| Scope | What it sees | Who owns it |
|-------|-------------|-------------|
| **private** | Only the agent's own channels | Single agent |
| **shared** | All channels the agent has access to | Multiple agents |
| **workspace** | Institutional knowledge across the workspace | Workspace agent |

### How Scoping Works

- **Extraction** (Layer 1) is always channel-scoped and shared
- **Rewards, Novelty, Prototypes** (Layers 2-4) are always agent-scoped and personal
- **Retrieval** respects scope boundaries: an agent can only retrieve from channels it has access to
- **Workspace scope** provides institutional knowledge that all agents in a workspace can access

```
Workspace Agent (institutional memory)
├── Agent A (private channels + shared channels)
│   ├── Channel 1 (private to A)
│   ├── Channel 2 (shared with B)
│   └── Brain: personal rewards, prototypes, rankings
├── Agent B (private channels + shared channels)
│   ├── Channel 2 (shared with A)
│   ├── Channel 3 (private to B)
│   └── Brain: personal rewards, prototypes, rankings
└── Workspace channels (institutional)
```

---

## User Modeling

### Users Are Entities

A user is an entity (type: `person`) in the knowledge graph. There is no separate "user" concept — users are first-class entities with AIEOS-compatible identity, just like agents and workspaces.

### Dual-Profile Model

Every user entity carries two profile layers:

| Profile | Source | Mutability |
|---------|--------|------------|
| **explicit_profile** | Self-reported by the user ("I live in Berlin", "I prefer direct communication") | Updated when user provides new info |
| **observed_profile** | Learned by the agent from conversation patterns | Updated continuously by extract_bee and consolidation_bee |

### Entity Observations

The `entity_observations` table tracks an agent's observations about entities over time:

```sql
CREATE TABLE meclaw.entity_observations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    entity_id TEXT REFERENCES meclaw.entities(id),
    agent_id TEXT REFERENCES meclaw.entities(id),    -- which agent made this observation
    channel_id UUID REFERENCES meclaw.channels(id),  -- in which channel
    observation_type TEXT,           -- 'preference', 'behavior', 'fact', 'relationship'
    key TEXT,                        -- 'communication_style', 'timezone', 'interests'
    value JSONB,                     -- {value: 'direct', evidence: 'never uses filler words'}
    confidence FLOAT DEFAULT 0.5,   -- 0.0-1.0, increases with repeated observations
    observation_count INT DEFAULT 1, -- how many times observed
    first_observed_seq BIGINT,
    last_observed_seq BIGINT,
    created_at TIMESTAMPTZ DEFAULT clock_timestamp(),
    updated_at TIMESTAMPTZ DEFAULT clock_timestamp()
);
```

### Observation Lifecycle

1. **extract_bee** (channel-level): detects user preferences, behaviors, facts in conversation
2. **entity_observations** table: stores observations with confidence scores
3. **consolidation_bee** (nightly): merges repeated observations, increases confidence, prunes contradictions
4. **observed_profile** on entity: consolidated view, updated by consolidation_bee
5. **retrieve_bee**: uses both explicit and observed profiles for personality-aware retrieval

### Example

```
Turn 1: Marcus says "Kein Gelaber bitte" 
  → extract_bee: observation {key: 'communication_style', value: 'direct', confidence: 0.6}

Turn 47: Marcus says "Komm zum Punkt"
  → extract_bee: observation {key: 'communication_style', value: 'direct', confidence: 0.6}
  → consolidation_bee (nightly): merge → confidence: 0.85, observation_count: 2

Entity marcus-meyer.observed_profile:
  {communication_style: {value: 'direct', confidence: 0.85}}
```

---

## Workspace Agents

### Workspace = Agent (type: workspace)

A workspace is not a container or a namespace — it is a full agent with type `workspace`:

- **Own Brain:** institutional memory, workspace-level prototypes and associations
- **Own Channels:** workspace-wide communication channels
- **AIEOS Identity:** institutional personality (neural_matrix defines the workspace culture)
- **Institutional Memory:** knowledge that persists beyond individual agent lifetimes

### Projects as Scopes

Projects are not separate entities — they are scopes/tags within a workspace agent:

```
Workspace Agent "gisela-workspace"
├── AIEOS Identity (institutional personality)
├── Brain (institutional prototypes, rewards)
├── Channels
│   ├── #general (workspace-wide)
│   ├── #engineering (scoped: project=backend)
│   └── #design (scoped: project=ui)
└── Projects = scopes/tags on entities and events
    ├── project:backend
    ├── project:ui
    └── project:infrastructure
```

### Why Workspace = Agent?

- Uniform model: everything that has memory and personality is an agent
- Workspace agents can participate in conversations (as institutional voice)
- Workspace brain accumulates organizational knowledge that outlives individual agents
- AIEOS identity ensures consistent institutional personality

---

## Theoretical Foundation

### Complementary Learning Systems (CLS)

Inspired by biological memory:

- **Fast Learner (Hippocampus):** Records experiences in a single exposure. High-fidelity episodic traces. Implemented as the `extract_bee` (channel-level) + AGE graph.
- **Slow Learner (Cortex):** Extracts patterns, builds prototypes, bridges memory to the LLM. Implemented as the `consolidation_bee` (pg_cron, nightly, agent-level).

### Why Graph > RAG

Standard RAG: `Similarity(query, memory) → rank`

Memory Hive: `Similarity × Novelty × Reward × Recency × PersonalityFit × GraphDistance → rank`

A correction from 6 months ago with high negative reward surfaces alongside yesterday's events — because the system knows it mattered. An empathetic agent retrieves emotional context that a purely logical agent would skip.

### Why This Beats EverMemOS / Mem0 / Zep

| System | Stores | Retrieves | Learns | Identity-Aware | Channel-Shared |
|--------|--------|-----------|--------|----------------|----------------|
| Mem0, Zep | ✅ | ✅ similarity only | ❌ | ❌ | ❌ |
| EverMemOS | ✅ | ✅ hybrid BM25+vector | ❌ | ❌ | ❌ |
| MeClaw Memory Hive | ✅ | ✅ value-aware graph traversal | ✅ | ✅ AIEOS | ✅ shared extraction |

---

## Architecture: The Memory Hive

Six specialized Bees, one PostgreSQL database.
Extract runs at **channel level** (shared). Everything else runs at **agent level** (personal).

```
Incoming Message (into Channel)
      ↓
  ┌─ CHANNEL LEVEL (shared) ─────────────────────────────────────┐
  │                                                                │
  │  extract_bee ──────────────────────────────────→ AGE Graph     │
  │      │           (Entities, Events, Relations)   (shared)      │
  │      │           extracted ONCE per channel                    │
  │                                                                │
  └────────────────────────────────────────────────────────────────┘
      ↓
  ┌─ AGENT LEVEL (personal, per agent) ──────────────────────────┐
  │                                                                │
  │  context_bee ─── compress static prefix ──→ Optimized context  │
  │      │           (SOUL/AGENTS/Skills → compressed)             │
  │      ↓                                                         │
  │  novelty_bee ─── pgvector distance ───────→ novelty score      │
  │      │                                                         │
  │  [Next Turn Starts]                                            │
  │      ↓                                                         │
  │  feedback_bee ─── sentiment of user reply ─→ retroactive reward│
  │      ↓                                                         │
  │  retrieve_bee ─── CTM-style iterative ────→ ranked context     │
  │      │            personality_fit from agent's neural_matrix    │
  │      │            + user's neural_matrix influence ranking      │
  │      ↓                                                         │
  │  LLM Bee (unchanged, policy network)                           │
  │                                                                │
  └────────────────────────────────────────────────────────────────┘

[Nightly - pg_cron, AGENT LEVEL]
  consolidation_bee ─ prune weak edges ───────────→ AGE Graph Updated
                     ─ merge compatible prototypes──→
                     ─ recalibrate Hebbian weights──→
                     ─ split conflicting prototypes (mitosis)
                     ─ consolidate entity observations
```

---

## Storage Hierarchy

Five layers of memory, from raw to abstract:

| Layer | Content | Scope | Mutability |
|-------|---------|-------|------------|
| **Layer 0** | Raw Messages | Channel (append-only) | Immutable |
| **Layer 1** | Extracted Entities, Events, Relations | Channel (shared) | Append-only, versioned via seq |
| **Layer 2** | Rewards, Novelty Scores | Agent (personal) | Mutable (updated by feedback_bee) |
| **Layer 3** | Prototypes, Associations | Agent (personal) | Mutable (updated by consolidation_bee) |
| **Layer 4** | Decision Traces | Agent (personal) | Immutable (audit trail) |

### Layer Properties

- **Layers 0-1 are shared:** Multiple agents can read the same messages and extracted entities
- **Layers 2-4 are personal:** Each agent has its own reward signals, prototypes, and decision traces
- **Layers 0-1 are recoverable:** If Layer 1 is lost, it can be reconstructed from Layer 0 by re-running extract_bee
- **Layers 2-3 are volatile but valuable:** Reward signals and prototypes represent learned knowledge — they can be rebuilt but it takes time

---

## Knowledge Graph vs Execution Graph

MeClaw maintains a strict separation between two graph types:

### Knowledge Graph (Persistent)

- **Append-only**, versioned via seq numbers
- Contains: entities, events, relations, prototypes, associations
- Stored in AGE (`meclaw_graph`)
- Volatile but reconstructable — Layer 1 can be rebuilt from Layer 0
- Shared across agents (extraction layer) with personal overlays (reward layer)

### Execution Graph (Ephemeral)

- **Per-request**, built by the router/planner at runtime
- Contains: bee execution order, hive call stack, routing decisions
- Lives only for the duration of a single request
- Discarded after the request completes
- Built from Hive definitions in AGE but the execution instance is transient

**Why the separation matters:** The knowledge graph grows over time and represents accumulated learning. The execution graph is disposable infrastructure. Mixing them would pollute the knowledge graph with routing noise.

---

## The Graph: AGE as Temporal Knowledge Graph

### Node Types

```cypher
(:Entity)      -- Person, agent, workspace, project, tool, or concept (canonical IDs, AIEOS identity)
(:Event)       -- A conversation turn or action (immutable, channel-scoped)
(:Prototype)   -- An emergent concept derived from patterns (agent-scoped)
(:Decision)    -- A decision with a complete audit trace (agent-scoped)
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
-- Entity table with aliases and AIEOS-compatible identity
-- ALL entities follow this schema: persons, agents, workspaces
CREATE TABLE meclaw.entities (
    id TEXT PRIMARY KEY,              -- 'meclaw:person:marcus-meyer' or 'meclaw:agent:walter' or 'meclaw:workspace:gisela'
    canonical_name TEXT NOT NULL,
    aliases TEXT[],                    -- ['Marcus', 'Marcus Meyer', 'mm']
    entity_type TEXT,                  -- 'person', 'agent', 'workspace', 'project', 'tool', 'concept'
    -- AIEOS Psychology (Soul Layer) — all entity types
    neural_matrix JSONB,              -- {creativity: 0.7, empathy: 0.8, logic: 0.9, ...}
    traits JSONB,                     -- {ocean: {openness: 0.8, ...}, mbti: 'INTJ'}
    moral_compass JSONB,              -- {alignment: 'neutral-good', core_values: [...]}
    -- AIEOS Linguistics — agents and workspaces
    linguistics JSONB,                -- {formality: 0.3, forbidden_words: [...], catchphrases: [...]}
    -- AIEOS Capabilities — agents and workspaces
    capabilities JSONB,               -- [{name: 'sql_read', priority: 1}, ...]
    -- AIEOS Metadata
    aieos_entity_id UUID,             -- AIEOS registry UUID (optional, for agent-to-agent)
    aieos_public_key TEXT,            -- Ed25519 public key (optional, for signing)
    -- Dual Profile (persons primarily)
    explicit_profile JSONB,           -- self-reported: {location: 'Berlin', languages: ['de', 'en']}
    observed_profile JSONB,           -- agent-learned: {communication_style: {value: 'direct', confidence: 0.85}}
    -- Standard fields
    embedding vector(1536),
    created_seq BIGINT
);
```

"Marcus Meyer", "Marcus", and "mm" all resolve to `meclaw:person:marcus-meyer`.
Agent "Walter" lives at `meclaw:agent:walter` with full AIEOS identity.
Workspace "gisela" lives at `meclaw:workspace:gisela` with institutional AIEOS identity.

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

### Retrieval Ranking (Personality-Aware)

The agent's neural_matrix AND the user's neural_matrix influence what gets retrieved:

```sql
-- personality_fit: how well a memory aligns with the agent's cognitive profile
-- AND the user's observed/explicit profile
-- e.g., high-empathy agent boosts emotionally significant memories
-- e.g., user with observed_profile.communication_style='direct' → concise memories ranked higher

ORDER BY (
    semantic_similarity * 0.25 +
    reward              * 0.25 +
    novelty             * 0.15 +
    recency             * 0.10 +
    personality_fit     * 0.15 +
    graph_distance      * 0.10
) DESC
```

The `personality_fit` score is computed by comparing the memory's content-type vector against both the agent's neural_matrix weights and the user's neural_matrix (explicit + observed). Emotional memories score higher for empathetic agents talking to emotionally expressive users.

---

## Context Pipeline: Compression + Caching

### Static Prefix Compression (Markdown Compressor)

Inspired by https://github.com/oborchers/fractional-cto — the context_bee applies lossless compression to all static context files before they enter the LLM context window:

```
Raw Static Files (SOUL.md, AGENTS.md, Skills, BRAIN.md)
      ↓
  Lossless Compress (20-40% token reduction, zero risk)
    - Remove redundant whitespace and formatting
    - Consolidate repeated information
    - Strip decorative markdown and HTML comments
    - NEVER remove: specific values, behavioral rules, tool names, paths, edge cases
      ↓
  Compressed Static Prefix
      ↓
  ── Cache Breakpoint ──  (Anthropic prompt caching boundary)
      ↓
  Dynamic Context (retrieve_bee output, conversation history)
      ↓
  LLM
```

**Rules (from Markdown Compressor):**
- Always safe to remove: filler, restated information, hedging, verbose transitions, decorative markdown
- Never remove: specific values/thresholds, behavioral rules (NEVER/ALWAYS), tool names and paths, decision logic, output formats, edge cases, YAML frontmatter

### Cache Strategy

The compressed static prefix is stable across turns → high cache hit rate.
Dynamic memories change per turn → always after the cache breakpoint.

```
[System + Compressed SOUL + Compressed Skills] → CACHE BREAKPOINT → [Memories + History + Current]
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

-- 5. Consolidate entity observations
-- Merge repeated observations, increase confidence
-- Prune contradictory low-confidence observations
-- Update observed_profile on entity
UPDATE meclaw.entities e
SET observed_profile = (
    SELECT jsonb_object_agg(key, jsonb_build_object('value', value, 'confidence', confidence))
    FROM meclaw.entity_observations
    WHERE entity_id = e.id AND confidence > 0.5
);
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
    channel_id UUID REFERENCES meclaw.channels(id),  -- which channel this event was extracted from
    agent_id TEXT REFERENCES meclaw.entities(id),     -- which agent's brain (NULL for shared extraction)
    content TEXT,
    embedding vector(1536),      -- pgvector
    reward FLOAT DEFAULT 0.0,    -- agent-level (personal)
    novelty FLOAT DEFAULT 0.0,   -- agent-level (personal)
    reward_updated_seq BIGINT DEFAULT 0,
    created_at TIMESTAMPTZ DEFAULT NOW()
);

-- Prototypes (emergent concepts, agent-scoped)
CREATE TABLE meclaw.prototypes (
    id TEXT PRIMARY KEY,
    agent_id TEXT REFERENCES meclaw.entities(id),  -- which agent owns this prototype
    centroid vector(1536),
    weight FLOAT DEFAULT 1.0,
    activation_count INT DEFAULT 0,
    value_mean FLOAT DEFAULT 0.0,
    value_variance FLOAT DEFAULT 0.0,
    last_activated_seq BIGINT DEFAULT 0,
    created_seq BIGINT
);

-- Prototype Associations (Hebbian, agent-scoped)
CREATE TABLE meclaw.prototype_associations (
    prototype_a TEXT REFERENCES meclaw.prototypes(id),
    prototype_b TEXT REFERENCES meclaw.prototypes(id),
    weight FLOAT DEFAULT 0.0,
    last_updated_seq BIGINT,
    PRIMARY KEY (prototype_a, prototype_b)
);

-- Entities (AIEOS-compatible, canonical IDs — all types)
CREATE TABLE meclaw.entities (
    id TEXT PRIMARY KEY,              -- 'meclaw:person:marcus-meyer', 'meclaw:agent:walter', 'meclaw:workspace:gisela'
    canonical_name TEXT NOT NULL,
    aliases TEXT[],
    entity_type TEXT,                  -- 'person', 'agent', 'workspace', 'project', 'tool', 'concept'
    -- AIEOS Psychology (Soul Layer) — all entity types
    neural_matrix JSONB,              -- {creativity: 0.7, empathy: 0.8, logic: 0.9, ...}
    traits JSONB,                     -- {ocean: {...}, mbti: '', enneagram: '', temperament: ''}
    moral_compass JSONB,              -- {alignment: '', core_values: [], conflict_resolution: ''}
    -- AIEOS Linguistics
    linguistics JSONB,                -- {formality: 0.3, forbidden_words: [], catchphrases: []}
    -- AIEOS Capabilities
    capabilities JSONB,               -- [{name: '', priority: 1, description: '', uri: ''}]
    -- AIEOS Metadata (optional, for agent-to-agent discovery)
    aieos_entity_id UUID,
    aieos_public_key TEXT,            -- Ed25519
    -- Dual Profile (persons primarily, but available for all types)
    explicit_profile JSONB,           -- self-reported data
    observed_profile JSONB,           -- agent-learned, consolidated from entity_observations
    -- Standard fields
    embedding vector(1536),
    created_seq BIGINT
);

-- Entity Observations (agent's observations about entities over time)
CREATE TABLE meclaw.entity_observations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    entity_id TEXT REFERENCES meclaw.entities(id),
    agent_id TEXT REFERENCES meclaw.entities(id),
    channel_id UUID REFERENCES meclaw.channels(id),
    observation_type TEXT,           -- 'preference', 'behavior', 'fact', 'relationship'
    key TEXT,                        -- 'communication_style', 'timezone', 'interests'
    value JSONB,                     -- {value: 'direct', evidence: 'never uses filler words'}
    confidence FLOAT DEFAULT 0.5,   -- 0.0-1.0
    observation_count INT DEFAULT 1,
    first_observed_seq BIGINT,
    last_observed_seq BIGINT,
    created_at TIMESTAMPTZ DEFAULT clock_timestamp(),
    updated_at TIMESTAMPTZ DEFAULT clock_timestamp()
);

-- Decision Traces (immutable audit trail, agent-scoped)
CREATE TABLE meclaw.decision_traces (
    id UUID PRIMARY KEY,
    agent_id TEXT REFERENCES meclaw.entities(id),  -- which agent made this decision
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

## The Six Bees

### 1. context_bee — Agent Level
**Trigger:** Before every LLM call
**Scope:** Agent-level (personal)
**Function:**
- Load static context files (SOUL, AGENTS, Skills)
- Apply lossless compression (markdown compressor rules)
- Set cache breakpoint after compressed static prefix
- Append dynamic memories from retrieve_bee
- Append conversation history

### 2. extract_bee — Channel Level
**Trigger:** After every message in a channel
**Scope:** Channel-level (shared!)
**Function:**
- LLM extracts entities, events, and relations from the conversation
- Entity resolution against existing entities (AIEOS-compatible)
- New nodes and edges written to the AGE graph (shared)
- Entities carry AIEOS identity (neural_matrix, linguistics, capabilities)
- User observations written to entity_observations table
- Embedding computed (pgvector)
- Temporal edge linked to the previous event node
- **Runs ONCE per channel, not per agent** — results are shared

### 3. novelty_bee — Agent Level
**Trigger:** After extract_bee
**Scope:** Agent-level (personal)
**Function:**
- Distance from new event embedding to nearest prototype (agent's prototypes)
- novelty = 1 - max_cosine_similarity
- Update `brain_events.novelty` for this agent
- Create a new prototype if novelty > threshold

### 4. feedback_bee — Agent Level
**Trigger:** Start of every turn
**Scope:** Agent-level (personal)
**Function:**
- Sentiment analysis of the user's message
- Positive ("exactly!", "thanks", "perfect") → prior event reward += 0.8
- Negative ("wrong", "no", "that's not right") → prior event reward -= 0.8
- Propagate reward backward through event chain (discounted)
- Rewards are personal — different agents may reward the same event differently

### 5. retrieve_bee — Agent Level
**Trigger:** Before every LLM call (context_bee integration)
**Scope:** Agent-level (personal), reads from shared extraction layer
**Function:**
- Stage 1: pg_search + pgvector + RRF → Top-20 (from channels the agent has access to)
- Stage 2: AGE graph expansion (1–3 ticks, CTM-style)
- Stage 3: Personality-aware ranking (similarity × reward × novelty × recency × personality_fit × graph_distance)
- personality_fit derived from requesting agent's neural_matrix AND user's neural_matrix
- Respects scoping: private / shared / workspace
- Output: Top-5 memories injected into context

### 6. consolidation_bee — Agent Level
**Trigger:** pg_cron, daily at 03:00 UTC
**Scope:** Agent-level (personal)
**Function:**
- Prune weak association edges
- Merge compatible prototypes
- Split conflicting prototypes (mitosis)
- Recalibrate Hebbian weights
- Mark stale precedents
- Compute citation authority curves
- **Consolidate entity observations** — merge repeated observations, increase confidence, update observed_profile

---

## Implementation Plan (Iterative)

### Phase 1 — Foundation ✅ (2026-03-20, Commit 58c4540)
- [x] Schema: `brain_events`, `entities`, `entity_observations` tables
- [x] Schema: `channels` with extraction cache, `agent_channels`
- [x] `extract_bee` (channel-level): raw content → brain_events + embeddings
- [x] `retrieve_bee`: BM25 + pgvector + RRF
- [x] `context_bee_v2`: memory retrieval integration
- [x] System Agent + Walter Agent bootstrap in AGE + entities

### Phase 2 — Learning ✅ (2026-03-20, Commit 1ff1063)
- [x] `novelty_bee`: novelty score + prototype creation (threshold 0.7)
- [x] `feedback_bee`: keyword-based retroactive reward
- [x] Reward-weighted ranking in retrieve_bee
- [x] AIEOS neural_matrix seed for Walter
- [x] Marcus Meyer entity with explicit_profile

### Phase 3 — Graph Intelligence ✅ (2026-03-20, Commit 939e13d)
- [x] AGE temporal edges (Event→Event TEMPORAL)
- [x] Graph expansion in retrieve_bee (±3 neighbors)
- [x] Entity resolution (`resolve_entity`: canonical, alias, fuzzy)
- [x] Personality-fit scoring (agent + user neural_matrix)
- [x] Scoping via agent_channels filter
- [x] AGENTS.md parser stub
- [x] `messages.seq` column

### Phase 4 — Consolidation & User Modeling ✅ (2026-03-20, Commit 77a6e80)
- [x] `consolidation_bee` via pg_cron (03:00 UTC)
- [x] Entity observation consolidation (merge, confidence, prune)
- [x] `observed_profile` auto-updates from observations
- [x] Prototype mitosis flagging (high variance → decay)
- [x] `observe_entity()` upsert helper
- [x] Workspace Agent stub

### Phase 5 — CTM Retrieval + Multi-Agent ✅ (2026-03-20, Commit 167a0ad)
- [x] `ctm_retrieve`: tick-based iterative retrieval (1-3 ticks, entropy convergence)
- [x] Adaptive compute (Shannon entropy < 0.3 → stop)
- [x] `discover_agents`: AIEOS-compatible discovery
- [x] `share_channel` + `cross_agent_retrieve`: shared memory graph
- [x] Ed25519 keypair stub

---

### ⚠️ Reflection: Phases 1-5 are deployed but at ~70% depth.

The skeleton works end-to-end. The following phases address the gaps identified during self-review.

---

### Phase 6 — Real LLM Extraction ✅ (2026-03-20, Commit 7105ca9)
- [x] extract_bee v2: two-stage pipeline (raw content + async LLM extraction via pg_background)
- [x] `llm_extract_entities()`: gpt-4o-mini structured extraction (entities + relations)
- [x] `create_or_resolve_entity()`: auto-create or merge entities via resolve_entity()
- [x] AGE Graph: Entity nodes (age_upsert_entity) + INVOLVED_IN edges (age_link_entity_event)
- [x] AGE Graph: Typed relation edges between entities (age_link_entities)
- [x] `entity_events` junction table (entity ↔ event with relation types + confidence)
- [x] `backfill_extractions()`: process existing unextracted events
- [x] Cost tracking: tokens logged per extraction in events table
- [x] brain_events: `extracted`, `extracted_at`, `extraction_data` columns

### Phase 7 — Robustness & Error Tolerance ✅ (2026-03-20, Commit d92b754)
- [x] feedback_bee v2: negation detection ("ja, das ist falsch" ≠ positive)
- [x] feedback_bee v2: LLM-based sentiment for ambiguous cases (`llm_sentiment()`)
- [x] compute_embedding: 3x retry with exponential backoff + 429 rate limit handling
- [x] `embedding_cache` table: query embedding cache (500 entries, auto-evict)
- [x] `get_query_embedding` v2: cache-first, 3x retry, rate limit aware
- [x] personality_fit v2: 5-dimensional keyword clusters + user alignment bonus
- [x] Hebbian Learning: `hebbian_update()` — co-activation via entity_events → prototype_associations
- [x] Prototype seeds for all discovered entities

### Phase 8 — Swarm Foundation
> Prerequisite for the autonomous Dev-Workflow "Hello World"

- [ ] concierge_bee: classifier (gpt-4o-mini), routes simple vs complex
- [ ] Multi-Model Pool: model capabilities + cost in llm_providers/llm_models
- [ ] Skill Registry: skills as structured defs in DB, queryable, with embedding
- [ ] planner_bee: top-tier LLM generates DAG from skill+model pool
- [ ] Feedback loop at DAG level (reward per DAG + per bee)

### Phase 9 — Context Pipeline
> context_bee is basic — no compression, no caching

- [ ] context_bee: lossless markdown compression (20-40% token reduction)
- [ ] context_bee: Anthropic cache breakpoint (stable prefix → high hit rate)
- [ ] context_bee: use ctm_retrieve instead of standard retrieve_bee
- [ ] AGENTS.md parser: full implementation (not just stub)

### Phase 10 — Tests & Validation
> Every phase above produces technical debt. This is where it gets paid.

- [ ] Unit Tests: SQL assertions for all core functions
  - resolve_entity, personality_fit, observe_entity, consolidation_bee
  - feedback_bee sentiment detection (positive, negative, negation, neutral)
  - retrieve_bee ranking order (rewarded > neutral > punished)
- [ ] Integration Tests: end-to-end message flow
  - user_input → extract → novelty → embedding → retrieve → context → llm → sender
  - Trigger chain stability under parallel messages
- [ ] Regression Tests: run on every deploy
  - Schema validation (all tables, indices, functions exist)
  - pg_cron jobs active
  - Embedding provider reachable
- [ ] Load Tests: parallel messages, embedding timeouts, pg_background limits
- [ ] Cost Monitoring: OpenRouter spend per day/week/agent

**Rule: After every phase, run a test batch from Phase 10. Not all at the end — incremental.**

---

## Key Design Principles

1. **Everything in PostgreSQL** — no external services, no external runtime
2. **Append-only events** — never delete; temporal history is sacred
3. **Reward as first-class data** — every memory has a value that changes over time
4. **LLM stays untouched** — all learning happens in the database
5. **Build iteratively** — Phase 1 alone already beats EverMemOS
6. **Follow open standards** — AIEOS for identity, AGENTS.md for project context
7. **Personality is data** — an entity's soul is queryable structured data, not just prose
8. **Extract once, rank many** — channel-level extraction is shared; agent-level ranking is personal
9. **Users are entities** — no separate user concept; persons are first-class entities in the graph
10. **Workspaces are agents** — institutional memory follows the same model as personal memory

---

## References

- Hippocampus Paper: https://sebhunte.vercel.app/blog/hippocampus-locomo (91.1% LoCoMo cumulative)
- EverMemOS: https://github.com/EverMind-AI/EverMemOS (93% LoCoMo isolated)
- AIEOS: https://aieos.org — AI Entity Object Specification v1.2
- AGENTS.md: https://agents.md — Open format for coding agent instructions
- Markdown Compressor: https://github.com/oborchers/fractional-cto
- Foundation Capital Context Graph Thesis
- Continuous Thought Machine: Darlow et al., 2025, Sakana AI
- LoCoMo Benchmark: Maharana et al., 2024
- Complementary Learning Systems Theory

---

*BRAIN.md is the architecture document for the MeClaw Memory Hive.*
*Last updated: 2026-03-20 — Phases 1-7 complete, Phases 8-10 planned.*

# Deep Analysis: MeClaw Benchmark Strategy

> Created: 2026-03-21 19:00
> Basis: BRAIN.md, Alinea/Hippocampus Paper, LongMemEval Paper, current implementation v0.3.0
> Goal: Maximum benchmark improvement using available tools

---

## 1. What Alinea Does Differently (and Why 93.8% Temporal)

### 1.1 Raphtory = Temporal-First Graph

The decisive difference between Alinea and MeClaw is **not** the architecture (which is nearly identical — we derived ours from Alinea). The difference is **Raphtory as a temporal-first graph engine**.

In Raphtory, time is a **structural dimension**, not a property. Every edge, every property has a native temporal history. You can view the graph "windowed" at ANY point in time:

```
"What did the system know about Julia on March 15, 2023?"
→ Window graph to 2023-03-15
→ All edges, properties, entities visible as they were at that point
```

In Apache AGE (MeClaw), time is a **property on nodes/edges**:
```cypher
MATCH (e:Event) WHERE e.created_at < '2023-03-15' RETURN e
```

This works for simple queries, but for **temporal reasoning** it lacks:
- No native graph windowing
- No "state of the graph at point X"
- No temporally-aware traversal (paths that are temporally consistent)

### 1.2 How Alinea Solves Temporal Questions

For "What was the first problem after the service?":

**Alinea/Raphtory:**
1. Window graph to time of "first service" event
2. Forward-traverse temporal edges from service event
3. First event matching "problem/issue" = answer
4. Temporal ordering is inherent in graph structure

**MeClaw/AGE (current):**
1. BM25 search "car service issue problem"
2. Vector similarity on query embedding
3. RRF fusion → Top-K ranked by relevance, NOT by temporal order
4. LLM re-ranking helps, but doesn't understand graph structure

### 1.3 PostgreSQL as a Raphtory Substitute

Here's the good news: PostgreSQL CAN do temporal-windowed queries. Not natively like Raphtory, but via SQL:

```sql
-- "All events between service date and 7 days after"
SELECT * FROM meclaw.brain_events
WHERE channel_id = $ch
  AND created_at BETWEEN $service_date AND ($service_date + interval '7 days')
ORDER BY created_at ASC;
```

And with AGE Cypher + WHERE clauses:
```cypher
MATCH (service:Event)-[:TEMPORAL*1..5]->(next:Event)
WHERE service.created_at >= '2023-03-15' 
  AND next.created_at <= '2023-03-22'
RETURN next
ORDER BY next.created_at ASC
```

**We CAN do temporal windowing — we just don't do it.**

---

## 2. What LongMemEval Actually Tests

### 2.1 The 5+1 Categories

| Category | n | What Is Tested | Our Score (v0.2.0) |
|---|---|---|---|
| temporal-reasoning | 133 | Temporal ordering of events ("first", "before", "after") | 3% |
| multi-session | 133 | Connecting facts from >1 session | 2% → 0.8% |
| knowledge-update | 78 | Updated facts ("what is the latest status?") | 17% |
| single-session-user | 70 | Facts the user stated | 14% |
| single-session-assistant | 56 | Facts the assistant stated | 46% |
| single-session-preference | 30 | Implicit user preferences | 17% |

### 2.2 Key Insight from the LongMemEval Paper

The paper recommends 3 optimizations:

1. **Session Decomposition for Value Granularity**
   - Instead of storing entire sessions → decompose into atomic facts/statements
   - Each fact gets its own embedding
   - "GPS issue after service" becomes its own fact, not part of a 300-word message

2. **Fact-Augmented Key Expansion**
   - Extracted entities/facts as additional search keys
   - We have `facts_text` — but it's only populated with LLM extraction

3. **Time-Aware Query Expansion**
   - Ask for temporal context: "first issue after service" → "issue AND date > service_date"
   - LLM reformulates the query with time filter

### 2.3 `question_date` — Our Unused Ace

Every question has a `question_date`! E.g. "2023/04/10 23:07". This is the point in time when the question is asked — ALL relevant sessions lie before it.

We don't use this **at all**. The runner completely ignores question_date.

---

## 3. Where MeClaw Stands vs. What's Possible

### 3.1 Current Pipeline (v0.3.0, Reset Mode)

```
Feed Sessions (with correct timestamps) →
extract_bee (user_input only, created_at from message) →
Batch Embedding →
Signal Pipeline:
  - Temporal Edges (AGE)
  - Novelty Bee
  - [Optional] LLM Extraction → facts_text + entity graph
→ retrieve_bee (BM25 + Vector + 6-Signal + Graph) →
[Optional] LLM Re-Ranking →
Return Context
```

### 3.2 What's Missing for Maximum Benchmark Performance

| Feature | Estimated Impact | Effort | Available? |
|---|---|---|---|
| **Session Decomposition** | +15-25% | Medium | Partially (extract_bee + LLM) |
| **Time-Aware Query Expansion** | +10-20% temporal | Low | No, but easy |
| **question_date as retrieval filter** | +5-10% temporal | Low | Data available |
| **Prioritize has_answer turns** | +10-15% | Low | Data available |
| **Enable LLM extraction** | +5-10% | Costs $10-15 | Yes, drop --skip-extraction |
| **PostgreSQL Temporal Windowing** | +10-15% temporal | Medium | SQL available |
| **Snapshot-based graph windowing** | +5-10% temporal | High | Theoretically possible |

---

## 4. Concrete Strategy: Maximum Benchmark With Available Tools

### Phase 1: Quick Wins (no new SQL code needed)

#### 1a. question_date as Retrieval Filter
The question has a date. All sessions are temporally prior to it.
In the runner: when a question is asked, pass a time filter to retrieve_bee:
```python
# Only retrieve events BEFORE question_date
context = retrieve_bee(agent, query, limit=10, 
                       before_date=question_date)
```

#### 1b. Identify has_answer Turns
LongMemEval marks which turns contain the answer (`has_answer: true`).
→ We CANNOT use this for the benchmark (that would be cheating).
BUT: We can check whether our retrieval finds these turns → diagnostic.

#### 1c. Top-K + Re-Ranking Already Enabled
Already implemented. ✅

### Phase 2: Session Decomposition (Medium Effort)

Instead of storing the entire user message as one brain_event:

```
Message: "I recently had my car serviced on March 15th and 
the GPS started malfunctioning. Also, I'm thinking of getting 
the car detailed and looking at accessories."

→ Decomposition:
  Fact 1: "Car was serviced on March 15th" [date: 2023-03-15]
  Fact 2: "GPS system started malfunctioning after service" [date: after 2023-03-15]
  Fact 3: "Considering car detailing" [date: ~2023-03-15]
  Fact 4: "Looking at car accessories" [date: ~2023-03-15]
```

Each fact gets:
- Its own embedding
- Its own timestamp
- Its own entity links

Implementation:
- LLM call: "Extract atomic facts with timestamps from this message"
- Create one brain_event per fact
- Costs ~1 LLM call per message, but dramatically better retrieval granularity

### Phase 3: Time-Aware Query Expansion (Low Effort)

Before retrieval: LLM reformulates the question with temporal context.

```
Original: "What was the first issue I had with my new car after its first service?"
Expanded: "car issue problem after first service, chronologically first event after service date"
+ Temporal Filter: "events ordered by created_at ASC, after service event"
```

Implementation:
- 1 LLM call per question
- Output: (expanded_query, temporal_direction, temporal_anchor)
- temporal_direction: "after" | "before" | "between" | "latest" | "first"
- temporal_anchor: reference event or date

### Phase 4: PostgreSQL Temporal Retrieval (Medium Effort)

A dedicated retrieval path for temporal questions:

```sql
-- 1. Find anchor event (e.g. "first service")
SELECT id, created_at FROM meclaw.brain_events
WHERE content ILIKE '%first service%' OR content ILIKE '%serviced%'
ORDER BY created_at ASC LIMIT 1;

-- 2. Forward-traverse: next events after anchor
SELECT * FROM meclaw.brain_events
WHERE created_at > $anchor_date
  AND channel_id = $channel
ORDER BY created_at ASC
LIMIT 10;
```

Or with AGE:
```cypher
MATCH (anchor:Event)-[:TEMPORAL*1..10]->(next:Event)
WHERE anchor.id = $anchor_id
RETURN next ORDER BY next.created_at ASC
```

### Phase 5: Snapshot-Based Graph Windowing (High Effort, Optional)

PostgreSQL has no native temporal graph engine like Raphtory. BUT:
- **Savepoints**: `SAVEPOINT before_question; ... ROLLBACK TO before_question;`
- **Schema Snapshots**: Freeze graph state for each question
- **Materialized Views**: `CREATE MATERIALIZED VIEW graph_at_date AS SELECT ... WHERE created_at <= $date`

This is the Raphtory substitute — not as elegant, but functional.

---

## 5. Prioritization: ROI per Feature

| Prio | Feature | Impact | Effort | ROI |
|------|---------|--------|---------|-----|
| 🔴 1 | Session Decomposition (Fact-Level Events) | +15-25% | 4h | ★★★★★ |
| 🔴 2 | Time-Aware Query Expansion | +10-20% temporal | 2h | ★★★★ |
| 🔴 3 | Temporal Retrieval Path (SQL/AGE) | +10-15% temporal | 3h | ★★★★ |
| 🟡 4 | question_date as filter | +5-10% | 1h | ★★★ |
| 🟡 5 | LLM Extraction (drop --skip-extraction) | +5-10% | $15 | ★★★ |
| 🟢 6 | Snapshot Graph Windowing | +5-10% | 8h | ★★ |

**Recommendation: Implement Prio 1-3 → expected total impact: +30-50%**

From 12% to an estimated 42-62%. That would be close to "Optimized Naive RAG" (Paper: 30-50%) and well above the previous result.

For 85%+ (Alinea-level) we would likely need Session Decomposition + Temporal Retrieval + a strong reader/judge — and a better backbone (GPT-5 instead of gpt-4o-mini).

---

## 6. Why the Benchmark Hasn't Improved More

### The Fundamental Problem

Our benchmark currently tests: "Can you find the right information in ~18 user messages (after deduplication)?"

The answer is: yes, mostly — BM25+Vector finds relevant content in 90%+ of cases. The problem is:
1. **Granularity**: The RIGHT information is a subordinate clause in a long message
2. **Ranking**: Similar messages rank similarly (0.43-0.46 band)
3. **Temporal**: "First", "before", "after" requires temporal ordering, not similarity

All our features (6-signal, Graph, Prototypes, Hebbian) help with **long-term accumulation** — but the benchmark tests **isolated retrieval accuracy per question**.

### What ACTUALLY Helps

1. **Fact-level granularity** (Session Decomposition) → GPS is its own fact, not a subordinate clause
2. **Temporal queries** → "first after X" becomes a temporal filter, not keyword matching
3. **LLM re-ranking** → understands "first issue after service" semantically (already implemented)

---

## 7. Raphtory vs. PostgreSQL: What We Can Emulate

| Raphtory Feature | PostgreSQL Equivalent | Quality |
|---|---|---|
| Temporal Windowing | `WHERE created_at <= $date` | 90% — only lacks live property versioning |
| Native Temporal Edges | AGE TEMPORAL edges with created_at property | 80% — no bi-temporal model |
| Time-Travel Queries | Savepoints / Materialized Views | 60% — not as elegant |
| Property Evolution | Audit tables / JSONB history | 70% — manual |
| Temporal Aggregation | Window functions | 95% — PostgreSQL is strong here |

**Conclusion: PostgreSQL can emulate ~80% of Raphtory's temporal features.** The missing 20% (native property versioning, bi-temporal model) are not critical for the LongMemEval benchmark, because we don't need to track property changes over time — we only need event ordering and time-window queries.

---

## 8. Concrete Implementation Plan

### Step 1: Extend retrieve_bee with Temporal Filter
- New parameter: `p_before_date TIMESTAMPTZ DEFAULT NULL`
- If set: `WHERE be.created_at <= p_before_date`
- Runner passes `question_date` as filter

### Step 2: Session Decomposition in the Runner
- After feed_message: LLM call "decompose this message into atomic facts with dates"
- Per fact: own brain_event with own timestamp
- Costs ~1 LLM call per user message (~$3-5 for 500 questions)

### Step 3: Time-Aware Query Expansion
- Before retrieve: LLM reformulates question
- Detects temporal direction ("first after", "last before", "between")
- Generates time filter for retrieve_bee

### Step 4: Run Benchmark
- `--rerank --top-k 10` (already implemented)
- New: `--decompose` (Session Decomposition)
- New: `--time-aware` (Query Expansion)
- Expectation: 40-60% instead of 12%

---

*This analysis is the basis for the next development phase.*
*Priority: Session Decomposition > Time-Aware Expansion > Temporal Retrieval Filter*

# Deep Analysis: v0.3.1 Benchmark (500 Questions)

> Date: 2026-03-22
> Version: v0.3.1 | ParadeDB 0.22.2 | --smart --skip-extraction --top-k 10
> Runtime: ~35 min | 0 rt_fetch errors | 473/500 Smart, 27/500 Fallback

---

## Overall Result

| Metric | Value |
|---|---|
| Answer Correct | **277/500 (55.4%)** |
| Context Quality | 245/500 (49.0%) |
| Avg Quality | 1.93/3.00 |
| Score=3 (perfect) | 243/500 |
| Score=0 (irrelevant) | 41/500 |

---

## Smart vs. Fallback Quality

| | Smart (473) | Fallback (27) |
|---|---|---|
| Avg Quality | 1.93 | 1.78 |
| Answer Correct | 54.5% | **70.4%** |
| Score=3 | 232 (49%) | 11 (41%) |
| Score=0 | 40 (8.5%) | 1 (3.7%) |

**Surprise:** Fallback is BETTER at Answer Correct (70.4% vs 54.5%)!

**Why?** The fallback (`ORDER BY created_at DESC LIMIT 10`) returns the most recent events
— and those are relevant for most questions, because the relevant sessions were
fed last. Smart retrieval actively searches and sometimes finds the WRONG
content (40 Score=0 cases).

---

## Category Analysis

| Category | n | Ans% | Smart | FB | Score=0 | Problem |
|---|---|---|---|---|---|---|
| single-session-user | 70 | **100%** | 66 | 4 | 0 | ✅ Perfect |
| knowledge-update | 78 | **73.1%** | 75 | 3 | 1 | ✅ Good |
| single-session-preference | 30 | **70.0%** | 26 | 4 | 1 | ✅ Good |
| multi-session | 133 | **51.9%** | 125 | 8 | 11 | ⚠️ Cross-Session |
| single-session-assistant | 56 | **35.7%** | 56 | 0 | 8 | 🔴 Trigger Bug |
| temporal-reasoning | 133 | **30.1%** | 125 | 8 | 20 | 🔴 Query Expansion |

---

## Root Cause Analysis

### 🔴 Problem 1: single-session-assistant (35.7%)

**Root Cause:** `trg_extract_on_done` fires ONLY on `type='user_input'`.
Assistant responses are stored as `type='llm_result'` → no brain_event is created.

Questions in this category ask about things the **assistant** said
(e.g. "What did the assistant say about the shift schedule?"). Since this content is never
extracted as brain_events, retrieve_bee cannot find it.

**Fix:** Fire trigger on `llm_result` as well — BUT: this was deliberately disabled
due to a duplicates bug (commit bd72af6). Solution: separate extraction for assistant content
without the duplicate-triggering logic.

**Expected Impact:** +20-30% for this category → +2-3% overall

### 🔴 Problem 2: temporal-reasoning (30.1%, 20× Score=0)

**Root Cause:** `expand_temporal_query` (LLM) generates incorrect time filters or
irrelevant expanded queries.

Examples:
- "Walk for Hunger → Coastal Cleanup" → LLM finds no data → wrong context
- "When did I join Page Turners?" → expanded query matches wrong topic
- "How many days since I bought a smoker?" → context has "got a smoker today" but
  no purchase date

**The problem:** The expanded query searches for keywords, but the relevant facts
appear as subordinate clauses in long messages. BM25 matches the wrong topic.

**Fix Options:**
1. **Session Decomposition** (--decompose) → atomic facts instead of long messages
2. **Multi-Query Expansion** → generate multiple queries, merge results
3. **Fallback to original query** when expanded query returns Score=0

### ⚠️ Problem 3: multi-session (51.9%, 11× Score=0)

**Root Cause:** Cross-session reasoning requires facts from MULTIPLE sessions.
retrieve_bee typically finds events from one session, not both.

Example: "How many items of clothing do I need to pick up?" needs info from 3 sessions.

**Fix Options:**
1. **Entity-based retrieval** → find entity first, then all associated events
2. **Multi-hop retrieval** → first search → extract entities → second search
3. **facts_text usage** → LLM extraction (currently disabled via --skip-extraction)

---

## What We Can Improve With the Current Architecture

### Quick Wins (no new SQL code)

| Fix | Impact | Effort |
|---|---|---|
| Drop `--skip-extraction` → facts_text + entity retrieval | +5-10% | ~$15 API |
| Extract assistant messages (trigger for llm_result) | +2-3% | 1h |
| Fallback to original query when expanded Score=0 | +2-3% temporal | 30min |

### Medium Effort

| Fix | Impact | Effort |
|---|---|---|
| Optimize session decomposition (faster LLM) | +10-15% temporal | 2h |
| Multi-query expansion (3 queries instead of 1) | +5-10% temporal | 2h |
| Entity-based multi-hop retrieval | +5-10% multi-session | 4h |

### Theoretical Maximum With Current Architecture

With all fixes applied, estimated: **65-75% Answer Correct**

For 85%+ (Alinea-level) we would likely need:
- A stronger reader/generator (GPT-4o instead of gpt-4o-mini)
- Real temporal graph traversal (not just time filters)
- Multi-hop reasoning chain

---

## Conclusion

The smart pipeline is now working reliably (94.6% Smart, 0 errors).
The main problems are:

1. **Retrieval quality** — BM25+Vector sometimes finds wrong content
2. **Missing assistant events** — a design decision that weakens an entire category
3. **LLM query expansion** — sometimes misleading for complex temporal questions

The next 10-15% will come from: enabling LLM extraction + assistant trigger +
decomposition. The remaining 20-30% to reach Alinea-level require architectural changes.

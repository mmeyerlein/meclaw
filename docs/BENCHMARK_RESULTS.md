# MeClaw Benchmark Results — LongMemEval Oracle (500 Questions)

> Last updated: 2026-03-21

---

## Test 1: v0.3.0 `--smart` (without decompose) — 2026-03-21

**Flags:** `--smart --skip-extraction --top-k 10`
**Duration:** ~40 min (500 questions)
**Features:** Temporal Query Expansion + Time-Filtered Retrieval + LLM Re-Ranking

### Overall Result

| Metric | Value |
|---|---|
| Substring Match | 216/500 → **43.2%** |
| Context Quality (Judge A) | 246/500 → **49.2%** |
| Answer Correct (Judge B) | 257/500 → **51.4%** |
| Avg Context Quality Score | **1.91/3.00** |

### By Category

| Category | n | Substr | Ctx Hit | Ans Hit | Avg Q |
|---|---|---|---|---|---|
| knowledge-update | 78 | 55 (70.5%) | 66 (84.6%) | 55 (70.5%) | 2.72 |
| multi-session | 133 | 58 (43.6%) | 36 (27.1%) | 51 (38.3%) | 1.44 |
| single-session-assistant | 56 | 8 (14.3%) | 10 (17.9%) | 20 (35.7%) | 1.18 |
| single-session-preference | 30 | 0 (0.0%) | 23 (76.7%) | 18 (60.0%) | 2.50 |
| single-session-user | 70 | 58 (82.9%) | 64 (91.4%) | 69 (98.6%) | 2.83 |
| temporal-reasoning | 133 | 37 (27.8%) | 47 (35.3%) | 44 (33.1%) | 1.59 |

### Context Quality Distribution

| Score | Count | |
|---|---|---|
| 0 (irrelevant) | 50 | █████ |
| 1 (tangential) | 190 | ███████████████████ |
| 2 (partial) | 15 | █ |
| 3 (perfect) | 245 | ████████████████████████ |

### Files
- Results: `/tmp/longmemeval/results_smart_500.json`
- Eval: `/tmp/longmemeval/eval_smart_500.json`
- Log: `/tmp/longmemeval/log_smart_500.txt`

---

## Baseline: v0.2.0 (no Smart, no Re-Ranking) — 2026-03-21

**Flags:** `--skip-extraction --top-k 5`

| Metric | Value |
|---|---|
| Substring Match | 122/500 → **24.4%** |
| Reader Substring | 112/500 → **22.4%** |
| Answer Correct (Judge) | 60/500 → **12.0%** |
| Avg Context Quality Score | **0.47/3.00** |

### Files
- Results: `/tmp/longmemeval/results_v020_500.json`
- Eval: `/tmp/longmemeval/eval_v020_500.json`

---

## Comparison: Progress

| Metric | v0.2.0 | v0.3.0 --smart | Δ |
|---|---|---|---|
| Substring | 24.4% | **43.2%** | **+77%** |
| Answer Correct | 12.0% | **51.4%** | **+328%** |
| Avg Quality | 0.47 | **1.91** | **+306%** |

### Biggest Improvements
- single-session-user: → **98.6%** (previously ~14%)
- knowledge-update: → **70.5%** (previously ~17%)
- multi-session: → **38.3%** (previously ~2%)

### Weaknesses
- temporal-reasoning: 33.1% — Temporal Query Expansion helps, but without fact-level decomposition granularity is missing
- single-session-assistant: 35.7% — assistant messages are not extracted (--skip-extraction)
- single-session-preference: 0% Substring (but 60% Judge!) — implicit preferences never match literally

---

## Reference Values (LongMemEval Paper)

| System | Overall |
|---|---|
| GPT-4o (commercial, 128K) | 30-70% depending on category |
| Naive RAG | 20-30% |
| Alinea (Temporal Graph) | 93.8% temporal |
| MeClaw v0.3.0 --smart | **51.4%** |

---

## v0.3.1 — ParadeDB Upgrade Quick Test (20 Questions, 2026-03-22)

**ParadeDB:** 0.15.10 → 0.22.2 | **VM:** 4 cores, 8GB RAM, AVX2 CPU

| Metric | v0.3.0 (buggy BM25) | v0.3.1 (fixed) |
|---|---|---|
| rt_fetch errors | 490/500 | **0/20** |
| Smart retrieval | 9/500 (1.8%) | **16/20 (80%)** |
| Fallback | 491/500 (98%) | 4/20 (20%) |

**500-question test pending** — significantly better results expected.

## Next Tests

- [ ] 500-question benchmark with v0.3.1 --smart (ParadeDB 0.22.2)
- [ ] Test with --smart --decompose --limit 50
- [ ] Test without --skip-extraction (LLM extraction for facts_text + entities)

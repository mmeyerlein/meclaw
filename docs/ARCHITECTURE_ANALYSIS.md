# MeClaw Architektur-Analyse: IST vs SOLL

> Datum: 2026-03-22
> Kontext: v0.3.1 Benchmark (55.4%), Honcho (90.4%), Hippocampus/Alinea (91.1% LoCoMo)
> Ziel: Systematischer Plan für 85%+ LongMemEval

---

## Teil 1: IST-Zustand — Was passiert WIRKLICH bei einer Message?

### Aktueller Trigger-Chain (sql/36)

```
Message INSERT (status='done')
  ↓ SYNC
  trg_extract_on_done()
  ├── extract_bee(msg_id)                    [SYNC, ~5ms]
  │   └── INSERT INTO brain_events           raw content, kein LLM
  │       └── pg_background: compute_embedding()  [ASYNC, ~500ms]
  ├── novelty_bee(agent_id, event_id)        [ASYNC via pg_background, ~200ms]
  │   └── cosine distance zu nearest Prototype
  └── feedback_bee(msg_id, agent_id)         [SYNC, nur user_input, ~10ms]
      └── Sentiment der aktuellen msg → reward auf vorheriges Event
```

**Gesamte Ingestion-Latenz: ~15ms sync + async im Hintergrund**

### Was NICHT passiert (aber in BRAIN.md steht):

| Feature | SQL existiert | Im Trigger aktiv | Im Benchmark aktiv |
|---|---|---|---|
| Raw content → brain_event | sql/22 ✅ | ✅ Trigger | ✅ |
| Embedding | sql/22 ✅ | ✅ pg_background | ✅ (batch im Runner) |
| Novelty scoring | sql/23 ✅ | ✅ pg_background | ✅ |
| Feedback/Reward | sql/24 ✅ | ✅ sync | ✅ |
| **LLM Entity Extraction** | sql/28 ✅ | ❌ NICHT im Trigger | ❌ `--skip-extraction` |
| **facts_text** (extrahierte Fakten) | sql/35 ✅ | ❌ | ❌ |
| **Entity Resolution** | sql/28 ✅ | ❌ | ❌ |
| **ACTIVATES Edges** | sql/41 ✅ | ❌ | ❌ |
| **ASSOCIATED Edges** | sql/41 ✅ | ❌ | ❌ |
| **CITES Edges** | sql/42 ✅ | ❌ | ❌ |
| **Reward Propagation** | sql/40 ✅ | ❌ | ❌ |
| **Decision Traces** | sql/42 ✅ | ❌ | ❌ |
| **Prototype Mitosis** | sql/43 ✅ | ❌ | ❌ |
| **MemCell Boundaries** | sql/44 ✅ | ❌ | ❌ |
| **CTM Iterative Retrieval** | sql/37 ✅ | ❌ | ❌ (Flag existiert) |
| **Consolidation Bee** | sql/26 ✅ | ❌ kein pg_cron | ❌ |
| **Decompose** | sql/46 ✅ | ❌ | Nur Test 3 |
| **Dual-Query Retrieval** | sql/46 ✅ | — | Neu, ungetestet |

**Wir nutzen 6 von 18 implementierten Features.** Der Rest ist toter Code.

### Retrieval-Pipeline (aktuell, sql/22 retrieve_bee)

```
Query
  ↓
  BM25 (ParadeDB @@@ Operator, top-20)
  +
  pgvector (cosine distance, top-20)
  ↓
  RRF Fusion (1/(60+rank))
  ↓
  AGE Graph Expansion (TEMPORAL + INVOLVED_IN, 1-3 hops)
  ↓
  6-Signal Ranking:
    rrf_score * 0.40-0.50
    recency   * 0.20-0.35
    reward    * 0.15
    novelty   * 0.10-0.15
  ↓
  Top-K Events (raw content)
```

### Smart Retrieval (sql/46 retrieve_smart, wenn --smart)

```
Query
  ↓
  expand_temporal_query (1 LLM Call, gpt-4o-mini)
    → expanded_query, temporal_direction, time_filter_before/after
  ↓
  retrieve_temporal (= retrieve_bee + WHERE created_at Filter)
  ↓
  [neu: dual-query — expanded + original, merged]
  ↓
  llm_rerank (1 LLM Call, gpt-4o-mini, top-K aus Pool)
  ↓
  Top-K Events (raw content)
```

---

## Teil 2: Warum 55.4% und nicht 90%?

### Fehler-Taxonomie (223/500 falsch):

```
Problem A: Retrieval findet falsches Material      109 (49%)
  ├── temporal tangential (BM25 keyword mismatch)   59
  ├── tangential (andere Kategorien)                50
  └── [wird schlimmer bei expanded query miss]

Problem B: Context OK, Reader antwortet falsch      43 (19%)
  └── gpt-4o-mini rechnet/zählt falsch

Problem C: Kein Assistant-Content                   27 (12%)
  └── GEFIXT (Trigger auf llm_result erweitert)

Problem D: Komplett falsches Topic (Score=0)        34 (15%)
  └── BM25+Vector matchen komplett daneben

Problem E: Partial (teilweise richtig)              10 (4.5%)
```

### Root Causes im Vergleich zu Honcho/Hippocampus:

| Problem | Unsere Ursache | Honcho-Lösung | Hippocampus-Lösung |
|---|---|---|---|
| A: Falsches Material | BM25 auf rohem Text, single-shot | Atomare Conclusions statt roher Text; Agentic Multi-Turn Retrieval | Temporal Graph Traversal statt Keyword-Search; PageIndex-Style hierarchisches Navigieren |
| B: Reader-Fehler | gpt-4o-mini zu schwach für Zählen/Rechnen | Claude Haiku 4.5 (stärker) mit Tool-Calls | GPT-5.2 als Backbone |
| D: Falsches Topic | BM25 keyword collision | Vorverarbeitete Conclusions (Noise eliminiert) | Entity-basiertes Retrieval mit kanonischen IDs |

### Der Kern-Unterschied:

**Wir:** Speichern rohe Messages → suchen mit Keywords → hoffen auf Match
**Honcho:** Extrahieren atomare Conclusions bei Ingestion → suchen in verdichteten Fakten → weniger Noise
**Hippocampus:** Bauen temporalen Wissensgraph → traversieren Kanten → Retrieval ist Graph-Problem, kein Such-Problem

---

## Teil 3: SOLL-Architektur — Pipeline-Design

### Prinzipien:

1. **Ingestion MUSS schnell bleiben** — User wartet nicht auf Memory-Pipeline
2. **Alles was kann, MUSS async** — pg_background oder Queue
3. **Jede Message nur 1x durch LLM** — keine redundanten Klassifizierungs-Calls
4. **Batch-Processing wo möglich** — Embeddings, Extractions in Batches
5. **Segmentierung vor Atomisierung** — Message → Segment → Atomic Fact (nicht direkt)

### Neue Pipeline: 3 Phasen

```
╔═══════════════════════════════════════════════════════════════╗
║  PHASE 1: HOT PATH (sync, <20ms, blockiert Antwort NICHT)   ║
╠═══════════════════════════════════════════════════════════════╣
║                                                               ║
║  Message INSERT                                               ║
║    ↓                                                          ║
║  trg_extract_on_done() [SYNC]                                ║
║    ├── brain_event INSERT (raw content + timestamp)           ║
║    ├── feedback_bee (sentiment → reward auf vorheriges Event) ║
║    └── Queue: pg_background oder NOTIFY                      ║
║         → PHASE 2 Jobs einstellen                            ║
║                                                               ║
║  ↓ Message-Verarbeitung geht SOFORT weiter                   ║
║  ↓ Agent kann antworten OHNE auf Memory zu warten            ║
╚═══════════════════════════════════════════════════════════════╝

╔═══════════════════════════════════════════════════════════════╗
║  PHASE 2: WARM PATH (async, <5s, läuft im Hintergrund)      ║
╠═══════════════════════════════════════════════════════════════╣
║                                                               ║
║  pg_background Worker oder Listener:                          ║
║                                                               ║
║  2a. Embedding berechnen [~500ms]                            ║
║      → compute_embedding(event_id)                            ║
║                                                               ║
║  2b. Novelty scoring [~50ms]                                  ║
║      → novelty_bee(agent_id, event_id)                        ║
║      → Prototype-Centroid Update (running average)            ║
║                                                               ║
║  2c. Reward Propagation [~10ms]                               ║
║      → propagate_reward_backward(event_id, depth=5, γ=0.9)   ║
║                                                               ║
║  2d. Temporal Edge [~10ms]                                    ║
║      → TEMPORAL edge zum vorherigen Event in AGE              ║
║                                                               ║
║  KEIN LLM Call in Phase 2 — alles lokal/API-embedding         ║
╚═══════════════════════════════════════════════════════════════╝

╔═══════════════════════════════════════════════════════════════╗
║  PHASE 3: COLD PATH (async, batch, periodisch)               ║
╠═══════════════════════════════════════════════════════════════╣
║                                                               ║
║  Trigger: Segment-Boundary ODER Timer (z.B. alle 5 Min       ║
║           wenn neue unverarbeitete Events existieren)         ║
║                                                               ║
║  3a. Segment-Erkennung [~50ms]                                ║
║      → Embedding-Drift zwischen Events messen                 ║
║      → Topic-Boundary wenn drift > threshold                  ║
║      → MemCell-Node im AGE Graph                              ║
║                                                               ║
║  3b. LLM Extraction — 1 Call pro SEGMENT, nicht pro Message  ║
║      → Segment-Text (3-10 Messages) als Batch an LLM         ║
║      → Output: atomare Fakten + Entities + Relationen         ║
║      → Entity Resolution gegen kanonische IDs                 ║
║      → INVOLVED_IN Edges                                      ║
║      → ACTIVATES Edges (Event→Prototype)                      ║
║      → facts_text auf brain_events setzen                     ║
║                                                               ║
║  3c. Hebbian Update [~10ms]                                   ║
║      → Co-aktivierte Prototypes → ASSOCIATED Edges stärken    ║
║                                                               ║
║  WICHTIG: 1 LLM Call pro Segment (3-10 msgs), nicht pro msg! ║
╚═══════════════════════════════════════════════════════════════╝

╔═══════════════════════════════════════════════════════════════╗
║  PHASE 4: DREAM (offline, nightly oder nach Idle-Zeit)       ║
╠═══════════════════════════════════════════════════════════════╣
║                                                               ║
║  Wie Honcho's Dreamer / Hippocampus Sleep Consolidation:      ║
║                                                               ║
║  4a. Reflection                                               ║
║      → Neue Events seit letztem Dream zusammenfassen          ║
║      → Higher-Level Insights extrahieren                      ║
║      → "User bevorzugt kurze Antworten" (induktiv)            ║
║      → "User hat 3 Haustiere" (deduktiv aus mehreren Events) ║
║                                                               ║
║  4b. Prototype Maintenance                                    ║
║      → Mitose (konfligierende Prototypes splitten)            ║
║      → Merge (ähnliche Prototypes zusammenführen)             ║
║      → Prune (schwache Assoziationen entfernen)               ║
║                                                               ║
║  4c. Entity Profile Update                                    ║
║      → observed_profile aktualisieren                         ║
║      → Confidence-Scores anpassen                             ║
║      → AieOS-Felder inferieren wenn Evidenz stark genug       ║
║                                                               ║
║  4d. Contradiction Detection                                  ║
║      → Widersprüchliche Fakten finden und markieren           ║
║      → "User sagte Mai: lebt in Berlin" vs "User sagte Aug:  ║
║         ist nach Hamburg gezogen" → Knowledge Update           ║
║                                                               ║
║  Honcho nutzt claude-sonnet mit 8192 thinking budget dafür.  ║
║  Wir: gpt-4o-mini oder lokales Modell, kosteneffizient.      ║
╚═══════════════════════════════════════════════════════════════╝
```

### Retrieval-Pipeline (SOLL)

```
Query
  ↓
  ┌─ STUFE 1: Schnelle Vorfilterung (~50ms) ────────────┐
  │                                                       │
  │  BM25 auf facts_text (extrahierte Fakten, nicht raw) │
  │  + pgvector auf Event-Embeddings                      │
  │  + BM25/pgvector auf Segment-Summaries (MemCells)    │
  │  → RRF Fusion → Top-30 Kandidaten                    │
  └───────────────────────────────────────────────────────┘
  ↓
  ┌─ STUFE 2: Graph Expansion (~50ms) ──────────────────┐
  │                                                       │
  │  Von Top-5 Anchors:                                   │
  │  → TEMPORAL Edges (zeitlich benachbarte Events)       │
  │  → INVOLVED_IN (gleiche Entity → andere Events)       │
  │  → ACTIVATES → ASSOCIATED → ACTIVATES (Concept-Hop)  │
  │  → Deduplizieren, Pool auf ~50 Kandidaten             │
  └───────────────────────────────────────────────────────┘
  ↓
  ┌─ STUFE 3: Temporal Intelligence (~100ms) ────────────┐
  │                                                       │
  │  Wenn temporal Query erkannt:                         │
  │  → expand_temporal_query (1 LLM Call)                 │
  │  → Dual-Query (expanded + original)                   │
  │  → Time-Window Filter                                 │
  │  → Temporal Ordering (ASC/DESC nach Direction)         │
  └───────────────────────────────────────────────────────┘
  ↓
  ┌─ STUFE 4: LLM Re-Ranking (~500ms) ──────────────────┐
  │                                                       │
  │  Top-30 Kandidaten → Cluster-Summaries                │
  │  LLM bewertet: "Welche davon beantworten die Frage?" │
  │  → Top-K finale Auswahl                               │
  │                                                       │
  │  Optional: 2. Iteration wenn Score niedrig            │
  │  → Multi-Turn Agentic Retrieval (wie Honcho Dialectic)│
  └───────────────────────────────────────────────────────┘
  ↓
  Top-K Events → Context für LLM Reader
```

---

## Teil 4: Segmentierung — Die fehlende Abstraktionsebene

### Problem heute:
```
Rohe Messages → brain_events (1:1 Mapping) → BM25/Vector Search
```

Eine Message kann 3 Themen enthalten. Oder 5 Messages gehören zum selben Thema. BM25 sucht in zu großen oder zu kleinen Einheiten.

### Hierarchische Segmentierung:

```
Level 0: Atomare Messages (raw, append-only)
  ↓ Embedding-Drift > threshold → Boundary
Level 1: Segmente (3-10 Messages, 1 Thema)        ← MemCells (sql/44)
  ↓ LLM Extraction pro Segment
Level 2: Atomare Fakten (1 Fakt pro Eintrag)       ← facts_text / decompose
  ↓ Periodische Zusammenfassung
Level 3: Conclusions (induktiv/deduktiv)            ← Dream/Reflection
  ↓ Langzeit-Konsolidierung
Level 4: Entity Profile (observed_profile)          ← Consolidation Bee
```

### Segment-Boundary Detection (sql/44, schon implementiert!):

```sql
-- Embedding-Drift zwischen aufeinanderfolgenden Events
-- Wenn drift > 0.3 → neues Segment
SELECT e1.id, e2.id, 
       1 - (e1.embedding <=> e2.embedding) as similarity
FROM brain_events e1
JOIN brain_events e2 ON e2.seq = e1.seq + 1
WHERE similarity < 0.7  -- Topic Change!
```

### LLM-Call-Effizienz:

| Ansatz | LLM Calls pro 10 Messages | Qualität |
|---|---|---|
| Pro Message extrahieren | 10 | Gut, aber teuer |
| Pro Segment extrahieren (3-10 msgs) | 1-3 | Besser — mehr Kontext für Extraktion |
| Pro Segment + Dreaming | 1-3 + 1 batch | Best — Segment + Conclusions |
| **Honcho** | 1 pro ~5 msgs (batch) + Dream | SOTA |

**Unsere Optimierung:** 1 LLM Call pro Segment in Phase 3. Nicht pro Message.

---

## Teil 5: Async-Mechanismen in PostgreSQL

### Optionen für Async-Processing:

| Mechanismus | Latenz | Zuverlässigkeit | Komplexität |
|---|---|---|---|
| `pg_background` | <100ms | Mittel (Worker-Slots begrenzt) | Niedrig — haben wir |
| `LISTEN/NOTIFY` + externer Worker | <200ms | Hoch | Mittel — neuer Process |
| `pg_cron` | 1 min minimum | Hoch | Niedrig — haben wir |
| Python Worker (asyncio) | <100ms | Hoch | Mittel — im Runner |

### Empfehlung: Hybrid

```
Phase 1 (Hot):      SQL Trigger (sync, <20ms)
Phase 2 (Warm):     pg_background (async, <5s, schon implementiert)
Phase 3 (Cold):     pg_cron alle 5 Min ODER LISTEN/NOTIFY bei Segment-Boundary
Phase 4 (Dream):    pg_cron nightly ODER nach 8h Idle
```

### pg_background Limitierung:
- `max_worker_processes` begrenzt parallele Worker
- Exception Handler existiert (sql/36) — bei Erschöpfung wird übersprungen
- Backfill-Mechanismus fängt verpasste Events auf

---

## Teil 6: Implementierungsplan — Priorisiert nach Impact

### Sprint 1: Low-Hanging Fruit (1-2h, geschätzt +10-15%)

**Ziel:** Existierenden Code AKTIVIEREN, keinen neuen schreiben.

| # | Aktion | Aufwand | Expected Impact |
|---|---|---|---|
| 1.1 | `--skip-extraction` weglassen im Benchmark | 0 | +5-8% (entities + facts_text für BM25) |
| 1.2 | BM25 auf `facts_text` statt `content` | 30min | +3-5% (präzisere Matches) |
| 1.3 | Reader-Model: gpt-4o statt mini | 0 (Flag) | +3-5% (Problem B: 43 Fehler) |

### Sprint 2: Segment-Pipeline (4-6h, geschätzt +10-15%)

**Ziel:** Messages in Segmente gruppieren, pro Segment extrahieren.

| # | Aktion | Aufwand | Abhängigkeit |
|---|---|---|---|
| 2.1 | MemCell Boundary Detection aktivieren (sql/44) | 1h | — |
| 2.2 | Segment-Level LLM Extraction (statt pro Message) | 2h | 2.1 |
| 2.3 | Segment-Summaries als zusätzliche Retrieval-Targets | 1h | 2.2 |
| 2.4 | ACTIVATES + ASSOCIATED Edges in Trigger-Chain | 1h | 2.2 |

### Sprint 3: Agentic Retrieval (4-6h, geschätzt +10%)

**Ziel:** Multi-Turn Retrieval statt single-shot.

| # | Aktion | Aufwand | Abhängigkeit |
|---|---|---|---|
| 3.1 | Retrieve-Agent: LLM entscheidet ob Ergebnis ausreicht | 2h | — |
| 3.2 | Fallback-Chain: expanded → original → entity-based → recency | 1h | — |
| 3.3 | Multi-Hop: Entity aus Ergebnis 1 → zweite Suche | 2h | Sprint 1.1 |

### Sprint 4: Dream/Reflection (4-6h, geschätzt +5-10%)

**Ziel:** Offline-Reasoning wie Honcho's Dreamer.

| # | Aktion | Aufwand | Abhängigkeit |
|---|---|---|---|
| 4.1 | Consolidation Bee via pg_cron aktivieren (sql/26) | 1h | — |
| 4.2 | Reflection: Events → Higher-Level Conclusions | 3h | Sprint 2 |
| 4.3 | Contradiction Detection | 2h | 4.2 |
| 4.4 | Prototype Mitosis aktivieren (sql/43) | 30min | — |

### Sprint 5: Full Benchmark Run (2h)

| # | Aktion | Aufwand |
|---|---|---|
| 5.1 | 500 Fragen mit Sprint 1-2 Features | ~1h Runner + 30min Eval |
| 5.2 | Ergebnisse analysieren, nächste Prioritäten setzen | 30min |

---

## Teil 7: Geschätztes Score-Potenzial

| Konfiguration | Geschätzte Accuracy |
|---|---|
| Aktuell (v0.3.1 --smart) | **55.4%** |
| + Assistant Trigger (DONE) | ~59% |
| + LLM Extraction + facts_text BM25 | ~65% |
| + Segment-Pipeline + gpt-4o Reader | ~72% |
| + Agentic Retrieval + Multi-Hop | ~78% |
| + Dream/Reflection + Contradictions | ~82% |
| + Fine-tuned Extraction Model (wie Honcho) | ~87% |
| Honcho (Claude Haiku 4.5) | **90.4%** |
| Hippocampus (GPT-5.2) | **91.1%** (LoCoMo) |

**Realistisches Ziel mit aktueller Architektur: 78-82%**
**Für 85%+ braucht es: stärkere Models ODER fine-tuned Extraction**

---

## Referenzen

- BRAIN.md: `/home/marcus/.openclaw/workspace/meclaw/docs/BRAIN.md`
- Honcho Blog: https://blog.plasticlabs.ai/research/Benchmarking-Honcho
- Hippocampus Paper: `/development/openclaw/austausch/papers/alinea-hippocampus-memory-paper.txt`
- Park et al. 2023: https://arxiv.org/abs/2304.03442
- Park et al. 2024: https://arxiv.org/abs/2411.10109
- LongMemEval: https://arxiv.org/abs/2410.10813

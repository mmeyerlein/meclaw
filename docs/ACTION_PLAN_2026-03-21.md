# MeClaw Action Plan — AGE Graph & Brain Activation

> Status: BRAIN.md beschreibt eine komplette Architektur. 
> Realität: 10 Phasen gebaut, fast nichts verdrahtet.
> AGE Graph: 43 Events, 61 Entities, 0 TEMPORAL Edges, 0 Prototypes aktiv.
> Benchmark: 12% (raw), 22.4% (mit Reader) auf LongMemEval Oracle.
> Ziel: Alles was gebaut ist, aktivieren. AGE als echten Temporal Knowledge Graph nutzen.

---

## Phase A: AGE Graph zum Leben erwecken (KRITISCH)

### A1. Temporal Edges — Event→Event Kette ✅ DONE
**Impact: Temporal Reasoning von 3% → geschätzt 30-50%**

Was fehlt: extract_bee erstellt brain_events, aber verknüpft sie NICHT temporal im AGE Graph.

```cypher
-- So soll es aussehen:
(:Event {id: e1, at: '2023-03-15'})-[:TEMPORAL {seq: 1}]->
(:Event {id: e2, at: '2023-03-15'})-[:TEMPORAL {seq: 2}]->
(:Event {id: e3, at: '2023-03-16'})
```

Aufgabe:
- [ ] extract_bee: Nach brain_event INSERT → AGE Event-Node erstellen
- [ ] extract_bee: TEMPORAL Edge zum vorherigen Event (gleicher Channel) setzen
- [ ] created_at vom Message übernehmen (schon im Code, muss in AGE rein)
- [ ] retrieve_bee: Cypher-Query für "Events between date X and Y"

### A2. Entity→Event Verknüpfung in AGE (statt nur entity_events Tabelle) ✅ DONE
**Impact: Multi-Session von 2% → geschätzt 20-30%**

Was fehlt: `llm_extract_entities` schreibt in `entity_events` Tabelle, aber NICHT in den AGE Graph. Die AGE INVOLVED_IN Edges (115) kommen von woanders.

Aufgabe:
- [ ] extract_bee_v2 nach LLM-Extraction: `age_link_entity_event()` aufrufen
- [ ] Entity-Nodes in AGE aktualisieren wenn neue Entities gefunden werden
- [ ] RELATES_TO Edges aus extraction_data Relations erstellen

### A3. Graph-basiertes Retrieval aktivieren (retrieve_bee v3) ✅ DONE
**Impact: +10-15% über alle Kategorien**

Was fehlt: retrieve_bee nutzt nur BM25+Vector. Phase 3 hat retrieve_bee_v3 mit AGE Cypher Traversal — liegt ungenutzt rum.

Aufgabe:
- [ ] retrieve_bee v3 in 22_embedding_bee.sql als aktive Version setzen
- [ ] Stage 2 (Graph Expansion): Cypher 1-3 Hops ab Anchor-Events
- [ ] 6-Signal Ranking aktivieren: BM25 + Vector + Graph_Distance + Personality_Fit + Recency + Reward

---

## Phase B: Brain-Bees verdrahten (HOCH)

### B1. Trigger-Chain erweitern ✅ DONE
**Impact: Alle Bees arbeiten zusammen statt isoliert**

Aktuelle Chain: `message INSERT → trg_extract_on_done → extract_bee(raw) → FERTIG`

Ziel-Chain:
```
message INSERT
  → trg_extract_on_done
    → extract_bee (raw content → brain_event + AGE Event Node + TEMPORAL Edge)
    → [pg_background] compute_embedding (batch!)
    → [pg_background] llm_extract_entities (→ AGE Entity Nodes + INVOLVED_IN + RELATES_TO)
    → [pg_background] novelty_bee (novelty score + prototype creation)
  
[Nächster User-Turn]
  → feedback_bee (sentiment → retroactive reward auf vorherigen Event)

[Vor LLM Call]
  → context_bee_v3 (komprimierter Prefix + CTM-Retrieval + Cache-Breakpoint)
  → retrieve_bee_v3 (BM25 + Vector + Graph Expansion + 6-Signal Ranking)
```

Aufgabe:
- [ ] trg_extract_on_done: novelty_bee Aufruf nach extract_bee ergänzen
- [ ] Neuer Trigger: trg_feedback_on_user_input → feedback_bee
- [ ] context_bee_v3 als aktive Version verdrahten (statt v2)
- [ ] retrieve_bee_v3 als aktive Version setzen

### B2. CTM Retrieval aktivieren ✅ DONE
**Impact: Komplexe Queries besser beantworten (multi-hop)**

Gebaut in Phase 5, nicht verdrahtet. 1-3 Ticks, Embedding-Drift, Entropy-Convergence.

Aufgabe:
- [ ] ctm_retrieve als Option in retrieve_bee_v3 integrieren
- [ ] Fallback auf Standard-Retrieval wenn CTM keinen Gain bringt

---

## Phase C: Datenqualität & Indexing (MITTEL)

### C1. Fact-Augmented Key Expansion ✅ DONE
**Impact: +5% Accuracy (Paper-Referenz)**

Was BRAIN.md sagt: Extrahierte Fakten als Such-Keys.
Was wir haben: `extraction_data` mit Entities — wird nicht für Retrieval genutzt.

Aufgabe:
- [ ] BM25 Index erweitern: extraction_data JSON-Felder in durchsuchbaren Text wandeln
- [ ] Oder: Separate `fact_index` Tabelle mit extrahierten Fakten als Rows
- [ ] retrieve_bee: Facts als zusätzliche BM25-Source in RRF

### C2. Prototypes aktivieren ✅ DONE
**Impact: Novelty-Scoring + Konzept-Clustering**

Tabelle existiert, Code existiert (novelty_bee), Daten: 0 echte Prototypes.

Aufgabe:
- [ ] novelty_bee in Trigger-Chain (Phase B1)
- [ ] Initiale Prototypes aus den Benchmark-Daten erstellen
- [ ] Consolidation_bee testen (mergt ähnliche Prototypes)

### C3. User Modeling aktivieren ✅ DONE
**Impact: Personality-aware Retrieval**

5 Seed-Observations für Marcus, aber kein Live-Flow.

Aufgabe:
- [ ] extract_bee: User-Präferenzen erkennen → entity_observations schreiben
- [ ] Consolidation: Observations → observed_profile konsolidieren
- [ ] retrieve_bee: personality_fit Score nutzen

---

## Phase D: Evaluator fixen + Re-Benchmark (NACH A-C)

### D1. Evaluator-Bug ✅ DONE
Judge v2 (11.4%) < Judge v1 (12%) — sollte nicht passieren.
Vermutung: Judge sieht "INSUFFICIENT CONTEXT" vom Reader und scored strenger.

Aufgabe:
- [ ] Judge-Prompt anpassen: wenn Reader "INSUFFICIENT CONTEXT" sagt, nur Context bewerten
- [ ] Separate Metriken: "Reader Accuracy" und "Context Quality"

### D2. Neuer Benchmark-Run
**Nach Phase A-C:**
- Neuer Runner mit echten Timestamps
- AGE Graph wird befüllt
- Alle Bees aktiv
- Erwartung: 30-40% statt 12%

---

## Reihenfolge (am Block)

```
A1. Temporal Edges in AGE                    [~2h]
A2. Entity→Event in AGE Graph                [~1h]  
A3. retrieve_bee v3 aktivieren               [~2h]
B1. Trigger-Chain verdrahten                 [~2h]
B2. CTM Retrieval aktivieren                 [~1h]
C1. Fact-Augmented Keys                      [~1h]
C2. Prototypes aktivieren                    [~1h]
C3. User Modeling                            [~1h]
─────────────────────────────────────────────
D1. Evaluator fixen                          [~30min]
D2. Neuer Benchmark-Run                      [~1h]
```

**Gesamt: ~12h Arbeit → erwarteter Benchmark-Sprung von 12% auf 30-40%**

---

## Erwartete Benchmark-Ergebnisse nach Umsetzung

| Kategorie | Jetzt | Nach A-C | Paper Best |
|---|---|---|---|
| single-session-assistant | 46% | 60-70% | ~70% |
| knowledge-update | 17% | 35-45% | ~60% |
| single-session-user | 14% | 30-40% | ~60% |
| single-session-preference | 17% | 25-35% | ~50% |
| multi-session | 2% | 20-30% | ~50% |
| temporal-reasoning | 3% | 30-50% | 93.8% (Alinea) |
| **Gesamt** | **12%** | **30-40%** | **60-70%** |

Temporal bleibt der härteste Gap. Für >50% brauchen wir echte temporale Graph-Queries à la Alinea/Raphtory — AGE kann das, aber die Query-Logik ist komplex.

---

*Erstellt: 2026-03-21 | Basis: BRAIN.md + PROGRESS.md + Benchmark v1/v2*

# Tiefenanalyse: v0.3.1 Benchmark (500 Fragen)

> Datum: 2026-03-22
> Version: v0.3.1 | ParadeDB 0.22.2 | --smart --skip-extraction --top-k 10
> Laufzeit: ~35 Min | 0 rt_fetch Errors | 473/500 Smart, 27/500 Fallback

---

## Gesamtergebnis

| Metrik | Wert |
|---|---|
| Answer Correct | **277/500 (55.4%)** |
| Context Quality | 245/500 (49.0%) |
| Avg Quality | 1.93/3.00 |
| Score=3 (perfect) | 243/500 |
| Score=0 (irrelevant) | 41/500 |

---

## Smart vs. Fallback Qualität

| | Smart (473) | Fallback (27) |
|---|---|---|
| Avg Quality | 1.93 | 1.78 |
| Answer Correct | 54.5% | **70.4%** |
| Score=3 | 232 (49%) | 11 (41%) |
| Score=0 | 40 (8.5%) | 1 (3.7%) |

**Überraschung:** Fallback ist bei Answer Correct BESSER (70.4% vs 54.5%)!

**Warum?** Der Fallback (`ORDER BY created_at DESC LIMIT 10`) gibt die letzten Events 
zurück — und die sind bei den meisten Fragen relevant, weil die relevanten Sessions 
zuletzt gefüttert wurden. Smart-Retrieval sucht aktiv und findet manchmal FALSCHEN 
Content (40 Score=0 Fälle).

---

## Kategorie-Analyse

| Kategorie | n | Ans% | Smart | FB | Score=0 | Problem |
|---|---|---|---|---|---|---|
| single-session-user | 70 | **100%** | 66 | 4 | 0 | ✅ Perfekt |
| knowledge-update | 78 | **73.1%** | 75 | 3 | 1 | ✅ Gut |
| single-session-preference | 30 | **70.0%** | 26 | 4 | 1 | ✅ Gut |
| multi-session | 133 | **51.9%** | 125 | 8 | 11 | ⚠️ Cross-Session |
| single-session-assistant | 56 | **35.7%** | 56 | 0 | 8 | 🔴 Trigger-Bug |
| temporal-reasoning | 133 | **30.1%** | 125 | 8 | 20 | 🔴 Query Expansion |

---

## Root Cause Analyse

### 🔴 Problem 1: single-session-assistant (35.7%)

**Root Cause:** `trg_extract_on_done` feuert NUR auf `type='user_input'`.
Assistant-Antworten werden als `type='llm_result'` gespeichert → kein brain_event.

Die Fragen in dieser Kategorie fragen nach Dingen die der **Assistant** gesagt hat
(z.B. "Was hat der Assistant über den Schichtplan gesagt?"). Da diese Inhalte nie
als brain_events extrahiert werden, kann retrieve_bee sie nicht finden.

**Fix:** Trigger auch auf `llm_result` feuern — ABER: Das wurde bewusst deaktiviert
wegen Duplikaten-Bug (commit bd72af6). Lösung: Separate Extraktion für Assistant-Content
ohne den Duplikate-Trigger.

**Erwarteter Impact:** +20-30% für diese Kategorie → +2-3% gesamt

### 🔴 Problem 2: temporal-reasoning (30.1%, 20× Score=0)

**Root Cause:** `expand_temporal_query` (LLM) generiert falsche Zeitfilter oder
irrelevante expanded Queries.

Beispiele:
- "Walk for Hunger → Coastal Cleanup" → LLM findet keine Daten → falscher Context
- "When did I join Page Turners?" → expanded Query matched falsches Topic
- "How many days since I bought a smoker?" → Context hat "got a smoker today" aber 
  kein Kaufdatum

**Das Problem:** Die expanded Query sucht nach Keywords, aber die relevanten Fakten
stehen als Nebensätze in langen Messages. BM25 matched das falsche Thema.

**Fix-Optionen:**
1. **Session Decomposition** (--decompose) → atomare Fakten statt lange Messages
2. **Multi-Query Expansion** → mehrere Queries generieren, Ergebnisse mergen
3. **Fallback auf Original-Query** wenn expanded Query Score=0 liefert

### ⚠️ Problem 3: multi-session (51.9%, 11× Score=0)

**Root Cause:** Cross-Session Reasoning erfordert Fakten aus MEHREREN Sessions.
retrieve_bee findet typischerweise Events aus einer Session, nicht beide.

Beispiel: "How many items of clothing do I need to pick up?" braucht Info aus 3 Sessions.

**Fix-Optionen:**
1. **Entity-basiertes Retrieval** → erst Entity finden, dann alle zugehörigen Events
2. **Multi-Hop Retrieval** → erste Suche → Entities extrahieren → zweite Suche
3. **facts_text Nutzung** → LLM Extraction (aktuell via --skip-extraction deaktiviert)

---

## Was wir mit aktueller Architektur verbessern können

### Quick Wins (kein neuer Code in SQL)

| Fix | Impact | Aufwand |
|---|---|---|
| `--skip-extraction` weglassen → facts_text + entity retrieval | +5-10% | ~$15 API |
| Assistant-Messages extrahieren (Trigger für llm_result) | +2-3% | 1h |
| Fallback auf Original-Query wenn expanded Score=0 | +2-3% temporal | 30min |

### Mittlerer Aufwand

| Fix | Impact | Aufwand |
|---|---|---|
| Session Decomposition optimieren (schnellere LLM) | +10-15% temporal | 2h |
| Multi-Query Expansion (3 Queries statt 1) | +5-10% temporal | 2h |
| Entity-basiertes Multi-Hop Retrieval | +5-10% multi-session | 4h |

### Theoretisches Maximum mit aktueller Architektur

Mit allen Fixes geschätzt: **65-75% Answer Correct**

Für 85%+ (Alinea-Niveau) bräuchten wir wahrscheinlich:
- Einen stärkeren Reader/Generator (GPT-4o statt gpt-4o-mini)
- Echtes Temporal Graph Traversal (nicht nur Zeitfilter)
- Multi-Hop Reasoning Chain

---

## Fazit

Die Smart-Pipeline funktioniert jetzt zuverlässig (94.6% Smart, 0 Errors).
Die Hauptprobleme sind:

1. **Retrieval-Qualität** — BM25+Vector findet manchmal falschen Content
2. **Fehlende Assistant-Events** — Design-Entscheidung die eine ganze Kategorie schwächt
3. **LLM Query Expansion** — manchmal irreführend bei komplexen temporalen Fragen

Die nächsten 10-15% kommen von: LLM Extraction aktivieren + Assistant-Trigger + 
Decomposition. Die restlichen 20-30% zu Alinea brauchen architektonische Änderungen.

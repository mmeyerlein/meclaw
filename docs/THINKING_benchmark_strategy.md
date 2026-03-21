# Tiefenanalyse: MeClaw Benchmark-Strategie

> Erstellt: 2026-03-21 19:00
> Basis: BRAIN.md, Alinea/Hippocampus Paper, LongMemEval Paper, aktuelle Implementierung v0.3.0
> Ziel: Maximale Benchmark-Verbesserung mit vorhandenen Bordmitteln

---

## 1. Was Alinea anders macht (und warum 93.8% temporal)

### 1.1 Raphtory = Temporal-First Graph

Der entscheidende Unterschied zwischen Alinea und MeClaw ist **nicht** die Architektur (die ist fast identisch — wir haben sie von Alinea abgeleitet). Der Unterschied ist **Raphtory als temporal-first Graph Engine**.

In Raphtory ist Zeit eine **strukturelle Dimension**, nicht eine Property. Jede Edge, jede Property hat eine native temporal History. Man kann den Graph zu JEDEM Zeitpunkt "windowed" betrachten:

```
"Was wusste das System am 15. März 2023 über Julia?"
→ Window graph to 2023-03-15
→ Alle Edges, Properties, Entities sichtbar wie sie zu dem Zeitpunkt waren
```

In Apache AGE (MeClaw) ist Zeit eine **Property auf Nodes/Edges**:
```cypher
MATCH (e:Event) WHERE e.created_at < '2023-03-15' RETURN e
```

Das funktioniert für einfache Queries, aber für **Temporal Reasoning** fehlt:
- Kein natives Graph-Windowing
- Kein "Zustand des Graphen zu Zeitpunkt X"
- Kein temporal-aware Traversal (Pfade die zeitlich konsistent sind)

### 1.2 Wie Alinea temporale Fragen löst

Für "Was war das erste Problem nach dem Service?":

**Alinea/Raphtory:**
1. Window graph to time of "first service" event
2. Forward-traverse temporal edges from service event
3. First event matching "problem/issue" = answer
4. Temporal ordering is inherent in graph structure

**MeClaw/AGE (aktuell):**
1. BM25 search "car service issue problem"
2. Vector similarity auf Query-Embedding
3. RRF Fusion → Top-K ranked by relevance, NOT by temporal order
4. LLM Re-Ranking hilft, aber versteht keine Graph-Struktur

### 1.3 PostgreSQL als Raphtory-Ersatz

Hier die gute Nachricht: PostgreSQL KANN temporal-windowed Queries. Nicht nativ wie Raphtory, aber über SQL:

```sql
-- "Alle Events zwischen Service-Datum und 7 Tage danach"
SELECT * FROM meclaw.brain_events
WHERE channel_id = $ch
  AND created_at BETWEEN $service_date AND ($service_date + interval '7 days')
ORDER BY created_at ASC;
```

Und mit AGE Cypher + WHERE-Clauses:
```cypher
MATCH (service:Event)-[:TEMPORAL*1..5]->(next:Event)
WHERE service.created_at >= '2023-03-15' 
  AND next.created_at <= '2023-03-22'
RETURN next
ORDER BY next.created_at ASC
```

**Wir KÖNNEN temporales Windowing machen — wir tun es nur nicht.**

---

## 2. Was LongMemEval eigentlich testet

### 2.1 Die 5+1 Kategorien

| Kategorie | n | Was getestet wird | Unser Score (v0.2.0) |
|---|---|---|---|
| temporal-reasoning | 133 | Zeitliche Ordnung von Events ("first", "before", "after") | 3% |
| multi-session | 133 | Fakten aus >1 Session verbinden | 2% → 0.8% |
| knowledge-update | 78 | Aktualisierte Fakten ("was ist der neueste Stand?") | 17% |
| single-session-user | 70 | Fakten die der User gesagt hat | 14% |
| single-session-assistant | 56 | Fakten die der Assistant gesagt hat | 46% |
| single-session-preference | 30 | Implizite Präferenzen des Users | 17% |

### 2.2 Schlüssel-Insight aus dem LongMemEval Paper

Das Paper empfiehlt 3 Optimierungen:

1. **Session Decomposition for Value Granularity**
   - Statt ganze Sessions zu speichern → in atomare Fakten/Statements zerlegen
   - Jeder Fakt bekommt sein eigenes Embedding
   - "GPS issue after service" wird ein eigener Fakt, nicht Teil einer 300-Wörter-Nachricht

2. **Fact-Augmented Key Expansion**
   - Extrahierte Entities/Fakten als zusätzliche Such-Keys
   - Wir haben `facts_text` — aber es wird nur bei LLM Extraction befüllt

3. **Time-Aware Query Expansion**
   - Frage nach temporalem Kontext: "first issue after service" → "issue AND date > service_date"
   - LLM reformuliert die Query mit Zeitfilter

### 2.3 `question_date` — unser ungenutztes Ass

Jede Frage hat ein `question_date`! Z.B. "2023/04/10 23:07". Das ist der Zeitpunkt an dem die Frage gestellt wird — ALLE relevanten Sessions liegen davor.

Wir nutzen das **überhaupt nicht**. Der Runner ignoriert question_date komplett.

---

## 3. Wo MeClaw steht vs. was möglich ist

### 3.1 Aktuelle Pipeline (v0.3.0, Reset-Modus)

```
Feed Sessions (mit korrekten Timestamps) →
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

### 3.2 Was fehlt für maximalen Benchmark

| Feature | Impact (geschätzt) | Aufwand | Verfügbar? |
|---|---|---|---|
| **Session Decomposition** | +15-25% | Mittel | Teilweise (extract_bee + LLM) |
| **Time-Aware Query Expansion** | +10-20% temporal | Gering | Nein, aber einfach |
| **question_date als Retrieval-Filter** | +5-10% temporal | Gering | Daten vorhanden |
| **has_answer Turns priorisieren** | +10-15% | Gering | Daten vorhanden |
| **LLM Extraction aktivieren** | +5-10% | Kostet $10-15 | Ja, --skip-extraction weglassen |
| **PostgreSQL Temporal Windowing** | +10-15% temporal | Mittel | SQL verfügbar |
| **Snapshot-basiertes Graph-Windowing** | +5-10% temporal | Hoch | Theoretisch möglich |

---

## 4. Konkrete Strategie: Maximaler Benchmark mit Bordmitteln

### Phase 1: Quick Wins (kein neuer Code in SQL nötig)

#### 1a. question_date als Retrieval-Filter
Die Frage hat ein Datum. Alle Sessions liegen zeitlich davor. 
Im Runner: Wenn die Frage gestellt wird, gib retrieve_bee einen Zeitfilter mit:
```python
# Nur Events VOR dem question_date retrieven
context = retrieve_bee(agent, query, limit=10, 
                       before_date=question_date)
```

#### 1b. has_answer-Turns identifizieren
LongMemEval markiert welche Turns die Antwort enthalten (`has_answer: true`).
→ Wir können das NICHT für den Benchmark nutzen (das wäre Cheating).
ABER: Wir können prüfen ob unser Retrieval diese Turns findet → Diagnose.

#### 1c. Top-K + Re-Ranking bereits aktiviert
Schon implementiert. ✅

### Phase 2: Session Decomposition (mittlerer Aufwand)

Statt die ganze User-Message als einen brain_event zu speichern:

```
Nachricht: "I recently had my car serviced on March 15th and 
the GPS started malfunctioning. Also, I'm thinking of getting 
the car detailed and looking at accessories."

→ Decomposition:
  Fakt 1: "Car was serviced on March 15th" [date: 2023-03-15]
  Fakt 2: "GPS system started malfunctioning after service" [date: after 2023-03-15]
  Fakt 3: "Considering car detailing" [date: ~2023-03-15]
  Fakt 4: "Looking at car accessories" [date: ~2023-03-15]
```

Jeder Fakt bekommt:
- Eigenes Embedding
- Eigenen Timestamp
- Eigene Entity-Links

Implementierung:
- LLM Call: "Extrahiere atomare Fakten mit Zeitangaben aus dieser Nachricht"
- Pro Fakt einen brain_event erstellen
- Kostet ~1 LLM Call pro Message, aber dramatisch bessere Retrieval-Granularität

### Phase 3: Time-Aware Query Expansion (geringer Aufwand)

Vor dem Retrieval: LLM reformuliert die Frage mit temporalem Kontext.

```
Original: "What was the first issue I had with my new car after its first service?"
Expanded: "car issue problem after first service, chronologically first event after service date"
+ Temporal Filter: "events ordered by created_at ASC, after service event"
```

Implementierung:
- 1 LLM Call pro Frage
- Output: (expanded_query, temporal_direction, temporal_anchor)
- temporal_direction: "after" | "before" | "between" | "latest" | "first"
- temporal_anchor: Referenz-Event oder Datum

### Phase 4: PostgreSQL Temporal Retrieval (mittlerer Aufwand)

Für temporale Fragen einen speziellen Retrieval-Pfad:

```sql
-- 1. Finde Anchor-Event (z.B. "first service")
SELECT id, created_at FROM meclaw.brain_events
WHERE content ILIKE '%first service%' OR content ILIKE '%serviced%'
ORDER BY created_at ASC LIMIT 1;

-- 2. Forward-Traverse: Nächste Events nach Anchor
SELECT * FROM meclaw.brain_events
WHERE created_at > $anchor_date
  AND channel_id = $channel
ORDER BY created_at ASC
LIMIT 10;
```

Oder mit AGE:
```cypher
MATCH (anchor:Event)-[:TEMPORAL*1..10]->(next:Event)
WHERE anchor.id = $anchor_id
RETURN next ORDER BY next.created_at ASC
```

### Phase 5: Snapshot-basiertes Graph-Windowing (hoher Aufwand, optional)

PostgreSQL hat keine native temporale Graph-Engine wie Raphtory. ABER:
- **Savepoints**: `SAVEPOINT before_question; ... ROLLBACK TO before_question;`
- **Schema Snapshots**: Für jede Frage den Graph-Zustand einfrieren
- **Materialized Views**: `CREATE MATERIALIZED VIEW graph_at_date AS SELECT ... WHERE created_at <= $date`

Das ist der Raphtory-Ersatz — nicht so elegant, aber funktional.

---

## 5. Priorisierung: ROI pro Feature

| Prio | Feature | Impact | Aufwand | ROI |
|------|---------|--------|---------|-----|
| 🔴 1 | Session Decomposition (Fact-Level Events) | +15-25% | 4h | ★★★★★ |
| 🔴 2 | Time-Aware Query Expansion | +10-20% temporal | 2h | ★★★★ |
| 🔴 3 | Temporal Retrieval Pfad (SQL/AGE) | +10-15% temporal | 3h | ★★★★ |
| 🟡 4 | question_date als Filter | +5-10% | 1h | ★★★ |
| 🟡 5 | LLM Extraction (--skip-extraction weglassen) | +5-10% | $15 | ★★★ |
| 🟢 6 | Snapshot Graph-Windowing | +5-10% | 8h | ★★ |

**Empfehlung: Prio 1-3 implementieren → erwarteter Gesamtimpact: +30-50%**

Von 12% auf geschätzt 42-62%. Das wäre in der Nähe von "Naive RAG optimiert" (Paper: 30-50%) und deutlich über dem bisherigen.

Für 85%+ (Alinea-Niveau) bräuchten wir wahrscheinlich Session Decomposition + Temporal Retrieval + einen starken Reader/Judge — und einen besseren Backbone (GPT-5 statt gpt-4o-mini).

---

## 6. Warum der Benchmark bisher nicht gestiegen ist

### Das fundamentale Problem

Unser Benchmark testet aktuell: "Kannst du in ~18 user Messages (nach Deduplizierung) die richtige Information finden?"

Die Antwort ist: Ja, meistens — BM25+Vector findet relevanten Content in 90%+ der Fälle. Das Problem ist:
1. **Granularität**: Die RICHTIGE Information ist ein Nebensatz in einer langen Message
2. **Ranking**: Ähnliche Messages ranken ähnlich (0.43-0.46 Band)
3. **Temporal**: "First", "before", "after" erfordert zeitliche Ordnung, nicht Similarity

Alle unsere Features (6-Signal, Graph, Prototypes, Hebbian) helfen bei **langfristiger Akkumulation** — aber der Benchmark testet **isolierte Retrieval-Genauigkeit pro Frage**.

### Was WIRKLICH hilft

1. **Fact-Level Granularität** (Session Decomposition) → GPS ist eigener Fakt, nicht Nebensatz
2. **Temporale Queries** → "first after X" wird zu einem zeitlichen Filter, nicht Keyword-Match
3. **LLM Re-Ranking** → Versteht "first issue after service" semantisch (bereits implementiert)

---

## 7. Raphtory vs. PostgreSQL: Was wir emulieren können

| Raphtory Feature | PostgreSQL Equivalent | Qualität |
|---|---|---|
| Temporal Windowing | `WHERE created_at <= $date` | 90% — fehlt nur Live-Property-Versioning |
| Native Temporal Edges | AGE TEMPORAL Edges mit created_at Property | 80% — kein bi-temporales Modell |
| Time-Travel Queries | Savepoints / Materialized Views | 60% — nicht so elegant |
| Property Evolution | Audit-Tables / JSONB History | 70% — manuell |
| Temporal Aggregation | Window Functions | 95% — PostgreSQL ist hier stark |

**Fazit: PostgreSQL kann ~80% von Raphtory's temporal Features emulieren.** Die fehlenden 20% (native Property-Versioning, bi-temporales Modell) sind für den LongMemEval-Benchmark nicht kritisch, weil wir keine Property-Änderungen über Zeit tracken müssen — wir brauchen nur Event-Ordering und Time-Window-Queries.

---

## 8. Konkreter Umsetzungsplan

### Schritt 1: retrieve_bee mit Temporal-Filter erweitern
- Neuer Parameter: `p_before_date TIMESTAMPTZ DEFAULT NULL`
- Wenn gesetzt: `WHERE be.created_at <= p_before_date`
- Runner übergibt `question_date` als Filter

### Schritt 2: Session Decomposition im Runner
- Nach feed_message: LLM Call "decompose this message into atomic facts with dates"
- Pro Fakt: eigener brain_event mit eigenem Timestamp
- Kostet ~1 LLM Call pro User-Message (~$3-5 für 500 Fragen)

### Schritt 3: Time-Aware Query Expansion
- Vor retrieve: LLM reformuliert Frage
- Erkennt temporale Richtung ("first after", "last before", "between")
- Generiert Zeitfilter für retrieve_bee

### Schritt 4: Benchmark laufen lassen
- `--rerank --top-k 10` (bereits implementiert)
- Neu: `--decompose` (Session Decomposition)
- Neu: `--time-aware` (Query Expansion)
- Erwartung: 40-60% statt 12%

---

*Diese Analyse ist die Basis für die nächste Entwicklungsphase.*
*Priorität: Session Decomposition > Time-Aware Expansion > Temporal Retrieval Filter*

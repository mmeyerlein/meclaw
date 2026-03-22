# Research Summaries — Agent Personality & Behavioral Simulation

> Kontext: AieOS Relationship Graph, MeClaw Persona-Modellierung
> Erstellt: 2026-03-22

---

## 1. "Generative Agents: Interactive Simulacra of Human Behavior" (Park et al., 2023)

**Paper:** https://arxiv.org/abs/2304.03442
**Repo:** https://github.com/joonspk-research/generative_agents
**PDF:** Im Repo verlinkt

### Kernidee
25 KI-Agenten leben in einer Sims-artigen Welt ("Smallville"), planen ihren Tag, führen Gespräche, erinnern sich an Erlebnisse und bilden Beziehungen. Die Agenten organisieren autonom eine Valentine's Day Party.

### Persönlichkeitsmodell
**Kein psychometrischer Test.** Stattdessen ein Freitext-Profil:
- `innate`: Grundcharakter als Adjektive ("friendly, outgoing, hospitable")
- `learned`: Backstory als Prosa ("Isabella Rodriguez is a cafe owner...")
- `currently`: Aktuelle Ziele/Pläne
- `lifestyle`: Tagesrhythmus ("goes to bed around 11pm, wakes up around 6am")
- `daily_plan_req`: Tägliche Routine

→ **Kein Big Five, kein OCEAN, kein MBTI.** Reine narrative Beschreibung.

### Memory-Architektur (3-Stufen — DIREKT relevant für MeClaw!)
1. **Observation Stream** = Episodisches Gedächtnis (≈ brain_events)
2. **Reflection** = Periodische Zusammenfassung ("Was habe ich heute gelernt?")
   - Gewichtung: `recency × relevance × importance`
   - Recency Decay: 0.995 pro Zeitschritt
   - Wird getriggert wenn `importance_trigger` Schwelle erreicht
3. **Planning** = Tagesplan basierend auf Persönlichkeit + Reflections

### Beziehungen
- **Implizit** durch Observations: "Isabella talked to Klaus about the party"
- **Kein expliziter Relationship-Graph** — Beziehungen emergieren aus Erinnerungen
- `chatting_with_buffer`: Tracking wer mit wem redet

### Relevanz für MeClaw/AieOS
| Park et al. | MeClaw Äquivalent | Lücke |
|---|---|---|
| innate + learned | AieOS psychology + history | ✅ Abgedeckt |
| Observation Stream | brain_events | ✅ Abgedeckt |
| Reflection | ❌ fehlt | **TODO: Reflection-Bee** |
| Recency × Relevance × Importance | 6-Signal Ranking | ✅ Ähnlich |
| Emergente Beziehungen | ❌ expliziter Graph | **Unser Relationship Graph ist BESSER** |
| Freitext-Persona | AieOS Schema | AieOS strukturierter |

**Schlüssel-Erkenntnis:** Park et al. nutzen KEINEN Fragenkatalog. Sie schreiben eine Backstory als Prosa. Die Reflection-Architektur (automatische Zusammenfassung von Erlebnissen in höhere Abstraktionen) fehlt in MeClaw und wäre ein starker nächster Schritt.

---

## 2. "Generative Agent Simulations of 1,000 People" (Park et al., 2024)

**Paper:** https://arxiv.org/abs/2411.10109
**PDF:** docs/research/arxiv_2411.10109_agent_personality.pdf
**Stanford HAI Policy Brief:** docs/research/stanford_hai_simulating_human_behavior.pdf

### Kernidee
Skalierung von 25 auf 1.052 echte Personen. LLM simuliert individuelle Einstellungen und Verhalten basierend auf **qualitativen Interviews** (nicht Fragebögen!). Die Agenten replizieren Antworten auf den General Social Survey mit **85% Genauigkeit** — fast so gut wie die echten Personen sich selbst nach 2 Wochen replizieren.

### Persönlichkeitsmodell — Interview-basiert!
**Genau der Ansatz den Marcus vorschlägt:**
- 2-stündige qualitative Interviews mit echten Personen
- Interviews werden als Kontext für LLM bereitgestellt
- LLM generiert Antworten "as if" es die Person wäre
- **Keine Fragebögen, keine psychometrischen Tests als Input**
- Aber: Validierung GEGEN psychometrische Tests (Big Five, GSS)

### Validierung
- **General Social Survey (GSS):** 85% Übereinstimmung (vs. 89% Test-Retest Reliabilität der echten Personen)
- **Big Five Traits:** Vergleichbare Vorhersagegenauigkeit
- **Experimentelle Replikation:** Wirtschaftsspiele (Dictator Game, etc.)
- Weniger Bias als rein demographische Beschreibungen

### Relevanz für MeClaw/AieOS
- **Interview-basiertes Onboarding > Fragebögen:** Die 85% Genauigkeit kommt von narrativen Interviews, nicht von Big Five Scores
- **Konversation IST der Fragebogen:** Die tägliche Konversation mit dem User ist genau so ein "qualitatives Interview"
- **Honcho/Dream-Ansatz bestätigt:** Passive Extraktion aus Gesprächen funktioniert besser als explizite Assessments
- **Bias-Reduktion:** Narrative > Demographie für faire Simulation

---

## 3. Generative Agents Repo — Persona-Architektur im Detail

**Repo:** https://github.com/joonspk-research/generative_agents

### Persona-Datenstruktur (scratch.json)
```json
{
  "name": "Isabella Rodriguez",
  "age": 34,
  "innate": "friendly, outgoing, hospitable",           // ≈ AieOS neural_matrix
  "learned": "Isabella is a cafe owner who loves...",    // ≈ AieOS history
  "currently": "Isabella is planning a party...",        // ≈ current goals
  "lifestyle": "goes to bed around 11pm...",             // ≈ AieOS interests.lifestyle
  
  // Memory weights (≈ MeClaw 6-Signal)
  "recency_w": 1,
  "relevance_w": 1,
  "importance_w": 1,
  "recency_decay": 0.995,
  
  // Reflection triggers
  "daily_reflection_time": 180,      // Minuten zwischen Reflections
  "daily_reflection_size": 5,        // Anzahl Reflections pro Tag
  "importance_trigger_max": 150,     // Schwelle für Spontan-Reflection
  "concept_forget": 100              // Vergessens-Schwelle
}
```

### Was dort FEHLT und wir haben/planen:
- ❌ Kein OCEAN/Big Five (wir: AieOS Schema)
- ❌ Kein Relationship-Graph (wir: RELATES_TO Edges)
- ❌ Keine Kanal-Adaptation (wir: BEHAVES_IN Edges)
- ❌ Kein BM25/Vector Retrieval (wir: Hybrid-Retrieval)
- ❌ Kein Reward-Signal (wir: feedback_bee)

### Was dort existiert und uns FEHLT:
- ✅ **Reflection-Architektur** → Periodische Zusammenfassung zu Higher-Level Insights
- ✅ **Importance-Score** → Nicht alle Erinnerungen gleich wichtig
- ✅ **Concept Forget** → Aktives Vergessen unwichtiger Erinnerungen
- ✅ **Daily Planning** → Proaktive Tagesplanung basierend auf Persona

---

## Synthese: Was bedeutet das für uns?

### 1. Onboarding: Interview > Fragebogen
Park et al. 2024 beweist: Ein narratives Interview ist besser als Big Five Scores. Das bestätigt Marcus' Ansatz — die **Konversation selbst** ist der Fragebogen. Kein separater Assessment nötig.

### 2. AieOS Schema als Ziel, nicht als Input
Das AieOS Schema (OCEAN, Neural Matrix, etc.) ist die **Ziel-Repräsentation**, nicht der Input-Prozess. Man befüllt es NICHT durch einen Big Five Test, sondern durch:
1. Initiale Konversation (≈ Park's qualitatives Interview)
2. Passive Extraktion aus täglichen Gesprächen (≈ Honcho Dream)
3. Periodische Reflection (≈ Park's Reflection-Architektur)

### 3. Reflection-Bee als nächster MeClaw-Feature
Eine `reflection_bee` die periodisch:
- Neue brain_events zusammenfasst
- Higher-Level Insights extrahiert ("User mag keine formellen Antworten")
- AieOS-Felder aktualisiert (wenn Evidenz stark genug)
- Temporäre Emotionszustände setzt und verfallen lässt

### 4. Fragenkatalog → Konversationsleitfaden
Statt eines starren Fragebogens ein adaptiver Leitfaden:
- Agent merkt welche AieOS-Felder noch leer sind
- Stellt gelegentlich natürliche Fragen die diese Felder füllen
- "Du hast letztens was über Kochen erzähnt — kochst du gerne?" → interests.hobbies
- "Bist du eher der Typ der Pläne macht oder spontan entscheidet?" → mental_patterns.decision_making_style

### 5. AieOS GitHub — keine Verweise auf Forschung
Auf dem AieOS Repo gibt es KEINE Verweise auf psychologische Forschung, Big Five Tests, oder Park et al. Das Schema ist aus Praxis-Erfahrung entstanden, nicht aus akademischer Grundlage. Das ist sowohl eine Stärke (pragmatisch) als auch eine Schwäche (keine Validierung).

---

## Dateien

| Datei | Beschreibung |
|---|---|
| `stanford_hai_simulating_human_behavior.pdf` | Stanford HAI Policy Brief — Übersicht Agent Simulation |
| `arxiv_2411.10109_agent_personality.pdf` | Park et al. 2024 — 1.000 simulierte Personen |
| GitHub: joonspk-research/generative_agents | Park et al. 2023 — Original Generative Agents |

# Relationship Graph Specification

> Status: Draft v0.1 — 2026-03-22
> Author: Walter (KI-Assistent) + Marcus Meyer
> Context: AieOS Extension + MeClaw Runtime Implementation

---

## Problem

AieOS v1.2 definiert eine statische Persona: Neural Matrix, OCEAN Traits, Linguistics.
Ein Agent verhält sich identisch gegenüber allen Gesprächspartnern.

In der Realität ist Persönlichkeit **relational**:
- Gleicher Mensch spricht anders mit dem Chef als mit dem Partner
- Gleiche Person im DM vs. Gruppenchat = anderes Verhalten
- Vertrauen baut sich über Zeit auf und verändert Kommunikation

Kein bestehendes Agent-Framework modelliert das.

---

## Lösung: Persona Graph

### Drei Dimensionen

```
Effektive Persona = Basis-Profil (AieOS)
                  + Person-Modifier (RELATES_TO)
                  + Channel-Modifier (BEHAVES_IN)
                  + Situation-Modifier (optional)
```

### Datenmodell

#### 1. AieOS Extension: `relationships` Section

```json
{
  "relationships": {
    "@type": "aieos:EntityRelationships",
    "@description": "Behavioral modifiers per relationship and context.",
    
    "default_stance": {
      "trust_level": 0.3,
      "formality_offset": 0.0,
      "tone": "friendly-professional",
      "humor_allowed": false,
      "topics_blocked": [],
      "verbosity_offset": 0.0
    },

    "persons": [
      {
        "person_id": "uuid-or-identifier",
        "name": "Marcus",
        "type": "partner",
        "trust_level": 1.0,
        "formality_offset": -0.3,
        "tone": "casual",
        "humor_allowed": true,
        "topics_allowed": ["*"],
        "topics_blocked": [],
        "language": "de",
        "learned_preferences": {
          "greeting_style": "none",
          "emoji_frequency": "moderate",
          "max_message_length": null
        }
      }
    ],

    "contexts": [
      {
        "context_type": "dm",
        "formality_offset": 0.0,
        "verbosity_offset": 0.0,
        "emoji_allowed": true
      },
      {
        "context_type": "group",
        "formality_offset": 0.2,
        "verbosity_offset": -0.3,
        "emoji_allowed": true,
        "speak_only_when_relevant": true
      },
      {
        "context_type": "public",
        "formality_offset": 0.4,
        "verbosity_offset": -0.2,
        "emoji_allowed": false,
        "review_before_send": true
      },
      {
        "context_type": "phone",
        "formality_offset": 0.1,
        "verbosity_offset": 0.2,
        "emotional_coloring": "warm"
      }
    ]
  }
}
```

#### 2. MeClaw AGE Graph (Runtime)

```cypher
-- Person Node
CREATE (p:Person {
  person_id: 'uuid',
  name: 'Marcus',
  type: 'partner',
  trust_level: 1.0,
  first_contact: '2026-03-19',
  last_interaction: '2026-03-22',
  interaction_count: 47
})

-- Relationship Edge
CREATE (a:Agent)-[:RELATES_TO {
  type: 'partner',
  trust_level: 1.0,
  formality_offset: -0.3,
  tone: 'casual',
  humor_allowed: true,
  topics_allowed: '["*"]',
  topics_blocked: '[]',
  language: 'de',
  learned_at: '2026-03-19',
  last_updated: '2026-03-22'
}]->(p:Person)

-- Channel Context Edge
CREATE (a:Agent)-[:BEHAVES_IN {
  context_type: 'dm',
  formality_offset: 0.0,
  verbosity_offset: 0.0,
  emoji_allowed: true
}]->(ctx:Context {type: 'dm'})

-- Person-Context Override (optional, für spezifische Kombinationen)
CREATE (p:Person)-[:INTERACTS_VIA {
  context_type: 'group',
  formality_override: 0.5,
  note: 'In Gruppenkontext formeller mit diesem Chef'
}]->(ctx:Context {type: 'group'})
```

#### 3. Modifier-Kaskade (Berechnung)

```
Schritt 1: Basis-Werte aus AieOS Profil laden
  formality = aieos.linguistics.text_style.formality_level  (z.B. 0.3)
  humor     = aieos.psychology.neural_matrix.charisma        (z.B. 0.7)
  verbosity = aieos.linguistics.text_style.verbosity_level   (z.B. 0.5)

Schritt 2: Person-Modifier anwenden (RELATES_TO Edge)
  formality += person.formality_offset                       (0.3 + (-0.3) = 0.0)
  humor     = humor if person.humor_allowed else 0.1         (0.7)
  topics    = person.topics_allowed                          (["*"])

Schritt 3: Channel-Modifier anwenden (BEHAVES_IN Edge)
  formality += channel.formality_offset                      (0.0 + 0.0 = 0.0)
  verbosity += channel.verbosity_offset                      (0.5 + 0.0 = 0.5)

Schritt 4: Person×Channel Override (INTERACTS_VIA, optional)
  IF exists: override spezifische Werte

Schritt 5: Clamp auf [0.0, 1.0]

Ergebnis → System Prompt Modifikation
```

---

## Auto-Learning

Der Graph lernt aus jeder Interaktion:

### Trust-Learning

```
Nach jeder Konversation:
  IF positive_feedback (user reagiert positiv, Daumen hoch, bedankt sich):
    trust_level = min(1.0, trust_level + 0.02)
    interaction_count += 1
  
  IF negative_feedback (user korrigiert, beschwert sich, "zu formal/zu locker"):
    trust_level = max(0.0, trust_level - 0.05)
    
    # Spezifische Korrekturen:
    "zu locker"     → formality_offset += 0.1
    "zu formal"     → formality_offset -= 0.1
    "kein Humor"    → humor_allowed = false
    "mehr Emojis"   → emoji_frequency = "high"
```

### Topic-Learning

```
Neues Thema in Konversation:
  IF user initiiert UND positives Signal:
    topics_allowed.append(new_topic)
  
  IF user blockt ("darüber will ich nicht reden"):
    topics_blocked.append(topic)
```

### Decay

```
Periodisch (z.B. wöchentlich):
  days_since_last = now - last_interaction
  
  IF days_since_last > 30:
    trust_level = max(default_trust, trust_level - 0.01 * (days_since_last / 30))
    formality_offset = formality_offset * 0.95  # Drift zurück zum Default
```

---

## Integration in MeClaw

### context_bee Prompt-Build (7-Layer)

```
Layer 1: Soul (AieOS Basis-Profil)
         ↓
Layer 1b: Relationship Modifier ← NEU
         (RELATES_TO + BEHAVES_IN berechnen)
         ↓
Layer 2: User/Gateway System Prompt
Layer 3: Persistent Memory
Layer 4: Skill Index
Layer 5: Context (hot data, retrieval)
Layer 6: Platform Formatting
         ↓
Output: Modifizierter System Prompt
```

### SQL Design (MeClaw)

```sql
-- Neue Tabelle (oder AGE Graph, s.o.)
CREATE TABLE meclaw.person_relationships (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id TEXT NOT NULL REFERENCES meclaw.entities(id),
    person_name TEXT NOT NULL,
    person_identifier TEXT,  -- chat-id, email, etc.
    relationship_type TEXT DEFAULT 'stranger',
    trust_level FLOAT DEFAULT 0.3 CHECK (trust_level BETWEEN 0.0 AND 1.0),
    formality_offset FLOAT DEFAULT 0.0 CHECK (formality_offset BETWEEN -1.0 AND 1.0),
    tone TEXT DEFAULT 'friendly-professional',
    humor_allowed BOOLEAN DEFAULT false,
    topics_allowed JSONB DEFAULT '["*"]',
    topics_blocked JSONB DEFAULT '[]',
    language TEXT,
    interaction_count INT DEFAULT 0,
    first_contact TIMESTAMPTZ DEFAULT NOW(),
    last_interaction TIMESTAMPTZ DEFAULT NOW(),
    learned_preferences JSONB DEFAULT '{}',
    created_at TIMESTAMPTZ DEFAULT NOW(),
    updated_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE TABLE meclaw.context_modifiers (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id TEXT NOT NULL REFERENCES meclaw.entities(id),
    context_type TEXT NOT NULL,  -- 'dm', 'group', 'public', 'phone'
    formality_offset FLOAT DEFAULT 0.0,
    verbosity_offset FLOAT DEFAULT 0.0,
    emoji_allowed BOOLEAN DEFAULT true,
    max_message_length INT,
    speak_only_when_relevant BOOLEAN DEFAULT false,
    created_at TIMESTAMPTZ DEFAULT NOW()
);

-- Berechnet effektive Persona für eine Person in einem Kontext
CREATE OR REPLACE FUNCTION meclaw.get_effective_persona(
    p_agent_id TEXT,
    p_person_identifier TEXT,
    p_context_type TEXT DEFAULT 'dm'
)
RETURNS JSONB AS $$
  -- 1. Basis aus AieOS laden
  -- 2. Person-Modifier anwenden
  -- 3. Channel-Modifier anwenden
  -- 4. Clamp & Return
$$ LANGUAGE plpgsql;
```

---

## gisela.ai Relevanz

Für Gisela ist das besonders relevant:

| Beziehung | Ton | Besonderheiten |
|---|---|---|
| Senior:in (Nutzer:in) | warm, geduldig, einfach | Langsames Tempo, Wiederholungen ok, keine Tech-Sprache |
| Angehörige | sachlich-informativ | Status-Updates, Empfehlungen, respektvoll-distanziert |
| Pfleger:in | professionell | Medizinische Begriffe ok, konkret, zeitsparend |
| Gisela ↔ Gisela (Agent-Agent) | technisch | AieOS-native Kommunikation |

Gleiche Gisela-Instanz, vier komplett verschiedene Personas.

---

## Abgrenzung

| Modul | Zuständigkeit |
|---|---|
| AieOS Basis | Wer bin ich? (statisch) |
| **Relationship Graph** | **Wie verhalte ich mich gegenüber wem?** (dynamisch) |
| User-Modeling (sql/39) | Was weiß ich über den User? (Fakten) |
| Episodisches Gedächtnis | Was ist passiert? (Events) |

---

## Offene Fragen

1. **AieOS PR?** — Sollen wir einen Proposal an `entitai/aieos` schreiben?
2. **Graph vs. Tabelle?** — AGE Graph (flexibel, traversierbar) vs. relationale Tabelle (einfacher, schneller)?
3. **Migration:** Wie befüllt man den Graph für bestehende Kontakte? LLM über Chat-History?
4. **Privacy:** Person-Daten sind sensibel. Verschlüsselung? Opt-out?
5. **Export/Import:** AieOS JSON ↔ Graph Sync Mechanismus

---

## Nächste Schritte

1. [ ] Feedback von Marcus
2. [ ] SQL implementieren (Tabellen + get_effective_persona)
3. [ ] In context_bee integrieren (Layer 1b)
4. [ ] Auto-Learning in feedback_bee einbauen
5. [ ] AieOS Schema Extension als PR vorschlagen

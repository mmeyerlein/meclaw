-- MeClaw v0.1.0 — Phase C2: Prototypes aktivieren
-- Date: 2026-03-21
-- Ref: docs/BRAIN.md (Prototype Engine, Novelty Bee, Hebbian Learning)
--
-- Aufgaben:
-- 1. seed_prototypes_from_events(limit INT) — Initiale Prototypes aus brain_events
-- 2. novelty_bee v2: Centroid-Update via Running Average bei bekannten Patterns
-- 3. hebbian_update v2: robuster gegen fehlende Prototypes, auto-stub Entities

-- =============================================================================
-- 1. seed_prototypes_from_events — Diverse Prototypes aus Top-N brain_events
-- =============================================================================
-- Strategie: Greedy Diversity Selection ("MaxMin")
--   - Wähle erstes Event (höchster Reward) als Seed
--   - Jedes weitere Event = max. Min-Distanz zu allen bisherigen Prototypes
--   - Ergebnis: maximal diverse Abdeckung des Embedding-Raums
--
-- Wichtig: brain_events haben keinen eigenen agent_id (NULL), daher
-- werden Prototypes dem Standard-Agent p_agent_id zugeordnet.
-- Idempotent: Bestehende Prototypes werden nicht überschrieben.
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw.seed_prototypes_from_events(
    p_limit INT DEFAULT 10,
    p_agent_id TEXT DEFAULT 'meclaw:agent:walter'
)
RETURNS INT AS $$
DECLARE
    v_seed_id UUID;
    v_seed_embedding vector(1536);
    v_min_dist FLOAT;
    v_best_dist FLOAT;
    v_best_id UUID;
    v_best_embedding vector(1536);
    v_best_seq BIGINT;
    v_created INT := 0;
    v_seq BIGINT;
    v_proto_id TEXT;
    v_existing_count INT;
    v_candidate RECORD;
BEGIN
    -- Prüfe ob Agent in entities existiert
    IF NOT EXISTS (SELECT 1 FROM meclaw.entities WHERE id = p_agent_id) THEN
        RAISE EXCEPTION 'Agent % existiert nicht in entities', p_agent_id;
    END IF;

    -- Vorhandene Prototypes mit Centroid zählen
    SELECT COUNT(*) INTO v_existing_count
    FROM meclaw.prototypes WHERE agent_id = p_agent_id AND centroid IS NOT NULL;

    IF v_existing_count > 0 THEN
        RAISE NOTICE 'Agent % hat bereits % Prototypes mit Centroid. Füge nur neue hinzu.',
            p_agent_id, v_existing_count;
    END IF;

    -- Temporäre Hilfstabellen (ON COMMIT DROP = automatisch nach Transaktion)
    CREATE TEMP TABLE _seed_candidates ON COMMIT DROP AS
    SELECT be.id, be.seq, be.embedding, be.reward
    FROM meclaw.brain_events be
    WHERE be.embedding IS NOT NULL
    ORDER BY be.reward DESC, be.seq ASC;

    CREATE TEMP TABLE _selected_seeds ON COMMIT DROP AS
    SELECT id::uuid AS event_id, seq, embedding
    FROM meclaw.brain_events WHERE FALSE; -- leere Struktur

    -- Erste Auswahl: Bestes Event (Seed 0) — oder falls schon Prototypes da,
    -- nimm das diverseste Event relativ zu bestehenden Prototypes
    IF v_existing_count = 0 THEN
        -- Kein Prototype da: Bestes Event als ersten Seed
        SELECT id, seq, embedding
        INTO v_seed_id, v_seq, v_seed_embedding
        FROM _seed_candidates
        LIMIT 1;
    ELSE
        -- Bereits Prototypes da: Wähle diversestes Event
        SELECT c.id, c.seq, c.embedding,
            (SELECT MIN(c.embedding <=> p.centroid)
             FROM meclaw.prototypes p
             WHERE p.agent_id = p_agent_id AND p.centroid IS NOT NULL) AS min_dist
        INTO v_seed_id, v_seq, v_seed_embedding, v_min_dist
        FROM _seed_candidates c
        ORDER BY (
            SELECT MIN(c.embedding <=> p.centroid)
            FROM meclaw.prototypes p
            WHERE p.agent_id = p_agent_id AND p.centroid IS NOT NULL
        ) DESC
        LIMIT 1;

        -- Nur fortfahren wenn das Event noch neuartig genug ist
        IF v_min_dist IS NOT NULL AND (1.0 - v_min_dist) < 0.3 THEN
            RAISE NOTICE 'Alle Events sind bereits gut durch bestehende Prototypes abgedeckt (min_dist=%).', v_min_dist;
            DROP TABLE IF EXISTS _seed_candidates;
            DROP TABLE IF EXISTS _selected_seeds;
            RETURN 0;
        END IF;
    END IF;

    IF v_seed_id IS NULL THEN
        RAISE NOTICE 'Keine brain_events mit Embeddings gefunden.';
        DROP TABLE IF EXISTS _seed_candidates;
        DROP TABLE IF EXISTS _selected_seeds;
        RETURN 0;
    END IF;

    INSERT INTO _selected_seeds VALUES (v_seed_id, v_seq, v_seed_embedding);

    v_proto_id := p_agent_id || ':proto:' || gen_random_uuid()::text;
    INSERT INTO meclaw.prototypes (id, agent_id, centroid, weight, activation_count, last_activated_seq, created_seq)
    VALUES (v_proto_id, p_agent_id, v_seed_embedding, 1.0, 1, v_seq, v_seq)
    ON CONFLICT (id) DO NOTHING;
    v_created := v_created + 1;

    -- Greedy MaxMin: Wähle Event mit größter Min-Distanz zu allen bisherigen Seeds
    WHILE v_created < p_limit LOOP
        v_best_id := NULL;
        v_best_dist := -1.0;

        FOR v_candidate IN
            SELECT c.id, c.seq, c.embedding,
                (SELECT MIN(c.embedding <=> s.embedding) FROM _selected_seeds s) AS min_dist
            FROM _seed_candidates c
            WHERE c.id NOT IN (SELECT event_id FROM _selected_seeds)
            ORDER BY (SELECT MIN(c.embedding <=> s.embedding) FROM _selected_seeds s) DESC
            LIMIT 1
        LOOP
            -- Nur hinzufügen wenn noch ausreichend diverse (cosine distance > 0.2)
            IF v_candidate.min_dist > 0.2 THEN
                v_best_id := v_candidate.id;
                v_best_dist := v_candidate.min_dist;
                v_best_seq := v_candidate.seq;
                v_best_embedding := v_candidate.embedding;
            END IF;
        END LOOP;

        EXIT WHEN v_best_id IS NULL;

        INSERT INTO _selected_seeds VALUES (v_best_id, v_best_seq, v_best_embedding);

        v_proto_id := p_agent_id || ':proto:' || gen_random_uuid()::text;
        INSERT INTO meclaw.prototypes (id, agent_id, centroid, weight, activation_count, last_activated_seq, created_seq)
        VALUES (v_proto_id, p_agent_id, v_best_embedding, 1.0, 1, v_best_seq, v_best_seq)
        ON CONFLICT (id) DO NOTHING;
        v_created := v_created + 1;
    END LOOP;

    DROP TABLE IF EXISTS _seed_candidates;
    DROP TABLE IF EXISTS _selected_seeds;

    -- Log
    INSERT INTO meclaw.events (bee_type, event, payload)
    VALUES ('seed_prototypes_from_events', 'prototypes_seeded', jsonb_build_object(
        'agent_id', p_agent_id,
        'created', v_created,
        'limit', p_limit
    ));

    RAISE NOTICE 'seed_prototypes_from_events: % Prototypes für Agent % erstellt.', v_created, p_agent_id;
    RETURN v_created;
END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION meclaw.seed_prototypes_from_events IS
'Erstellt initiale Prototypes aus den Top-N brain_events mittels Greedy MaxMin Diversity.
Wählt Events so aus, dass maximale Abdeckung des Embedding-Raums erreicht wird.
Idempotent: respektiert bestehende Prototypes.
Parameter: p_limit = max neue Prototypes, p_agent_id = Ziel-Agent.';

-- =============================================================================
-- 2. novelty_bee v2 — Centroid-Update (Running Average) bei bekannten Patterns
-- =============================================================================
-- Kompatibel mit pgvector 0.8.x (kein vector*scalar Operator):
-- Nutzt vector_to_float4() + string_agg für element-wise Berechnung.
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw.novelty_bee(p_agent_id TEXT, p_event_id UUID)
RETURNS VOID AS $$
DECLARE
    v_event_embedding vector(1536);
    v_max_similarity FLOAT := 0.0;
    v_novelty FLOAT;
    v_nearest_prototype_id TEXT;
    v_prototype_count INT;
    v_activation INT;
    v_alpha FLOAT;
BEGIN
    -- Event-Embedding laden
    SELECT embedding INTO v_event_embedding
    FROM meclaw.brain_events WHERE id = p_event_id;

    IF v_event_embedding IS NULL THEN
        RETURN; -- Embedding noch nicht verfügbar, später verarbeiten
    END IF;

    -- Anzahl Prototypes des Agents mit Centroid
    SELECT COUNT(*) INTO v_prototype_count
    FROM meclaw.prototypes
    WHERE agent_id = p_agent_id AND centroid IS NOT NULL;

    -- Nächsten Prototype finden
    IF v_prototype_count > 0 THEN
        SELECT
            p.id,
            1.0 - (p.centroid <=> v_event_embedding)
        INTO v_nearest_prototype_id, v_max_similarity
        FROM meclaw.prototypes p
        WHERE p.agent_id = p_agent_id
          AND p.centroid IS NOT NULL
        ORDER BY p.centroid <=> v_event_embedding
        LIMIT 1;
    END IF;

    -- Novelty Score: 0 = identisch zu bekanntem Prototype, 1 = völlig neu
    v_novelty := 1.0 - COALESCE(v_max_similarity, 0.0);

    -- brain_event mit Novelty aktualisieren
    UPDATE meclaw.brain_events
    SET novelty = v_novelty
    WHERE id = p_event_id;

    -- Entscheidung: Neuer Prototype oder Update bestehender Centroid
    IF v_novelty > 0.7 OR v_prototype_count = 0 THEN
        -- Neues Konzept erkannt → Prototype erstellen
        INSERT INTO meclaw.prototypes (
            id, agent_id, centroid, weight, activation_count, last_activated_seq, created_seq
        ) VALUES (
            p_agent_id || ':proto:' || gen_random_uuid()::text,
            p_agent_id,
            v_event_embedding,
            1.0,
            1,
            (SELECT seq FROM meclaw.brain_events WHERE id = p_event_id),
            (SELECT seq FROM meclaw.brain_events WHERE id = p_event_id)
        );
    ELSE
        -- Bekanntes Muster → Centroid via Online Running Average anpassen
        -- alpha = 1/(n+2): Je mehr Aktivierungen, desto langsamer die Drift
        SELECT activation_count INTO v_activation
        FROM meclaw.prototypes WHERE id = v_nearest_prototype_id;

        v_alpha := 1.0 / (v_activation + 2.0);

        -- pgvector 0.8.x: Kein vector*scalar Operator → via vector_to_float4 + string_agg
        UPDATE meclaw.prototypes
        SET activation_count = activation_count + 1,
            last_activated_seq = (SELECT seq FROM meclaw.brain_events WHERE id = p_event_id),
            centroid = (
                SELECT ('[' || string_agg(
                    (c_val::float8 * (1.0 - v_alpha) + e_val::float8 * v_alpha)::text,
                    ','
                ) || ']')::vector
                FROM unnest(
                    vector_to_float4(centroid, 1536, false),
                    vector_to_float4(v_event_embedding, 1536, false)
                ) AS t(c_val, e_val)
            )
        WHERE id = v_nearest_prototype_id;
    END IF;

    -- Audit-Log
    INSERT INTO meclaw.events (bee_type, event, payload)
    VALUES ('novelty_bee', 'novelty_computed', jsonb_build_object(
        'event_id', p_event_id,
        'agent_id', p_agent_id,
        'novelty', v_novelty,
        'nearest_prototype', v_nearest_prototype_id,
        'new_prototype_created', v_novelty > 0.7 OR v_prototype_count = 0,
        'prototype_count_before', v_prototype_count
    ));
END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION meclaw.novelty_bee IS
'Agent-level novelty scoring v2. Berechnet Distanz von Event-Embedding zu nächstem Prototype.
Novelty > 0.7 → Neuer Prototype. Sonst → Centroid-Update via Online Running Average.
Kompatibel mit pgvector 0.8.x (via vector_to_float4 + string_agg, kein vector*scalar).';

-- =============================================================================
-- 3. hebbian_update v2 — Robuster, auto-stub für Entities ohne Prototype-Eintrag
-- =============================================================================
-- Entities werden als Konzept-Knoten im Prototype-Graphen genutzt.
-- Co-Aktivierung im gleichen Event stärkt ihre Assoziation (Hebb'sche Regel).
-- Falls noch kein Prototype-Eintrag für eine Entity: wird automatisch erstellt.
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw.hebbian_update(p_event_id UUID, p_reward_delta FLOAT)
RETURNS VOID AS $$
DECLARE
    v_entities TEXT[];
    v_entity_a TEXT;
    v_entity_b TEXT;
    v_hebbian_rate FLOAT := 0.1;
    v_current_seq BIGINT;
    v_agent_id TEXT;
BEGIN
    -- Alle beteiligten Entities dieses Events
    SELECT array_agg(entity_id) INTO v_entities
    FROM meclaw.entity_events
    WHERE event_id = p_event_id;

    IF v_entities IS NULL OR array_length(v_entities, 1) < 2 THEN
        RETURN; -- Keine Co-Aktivierung möglich
    END IF;

    SELECT COALESCE(MAX(seq), 0) INTO v_current_seq FROM meclaw.brain_events;

    -- Agent-ID des Events (Fallback: walter)
    SELECT COALESCE(agent_id, 'meclaw:agent:walter') INTO v_agent_id
    FROM meclaw.brain_events WHERE id = p_event_id;

    -- Auto-Stub: Alle Entities als Prototype-Knoten registrieren wenn noch nicht vorhanden
    -- Nutze Entity-Embedding als Centroid wenn verfügbar
    INSERT INTO meclaw.prototypes (id, agent_id, centroid, weight, activation_count, last_activated_seq, created_seq)
    SELECT
        e.entity_id,
        v_agent_id,
        ent.embedding,
        0.5,
        1,
        v_current_seq,
        v_current_seq
    FROM unnest(v_entities) AS e(entity_id)
    JOIN meclaw.entities ent ON ent.id = e.entity_id
    WHERE NOT EXISTS (SELECT 1 FROM meclaw.prototypes p WHERE p.id = e.entity_id)
    ON CONFLICT (id) DO NOTHING;

    -- Hebbian Paarweise-Verknüpfung: Co-Aktivierung stärkt Assoziation
    -- Positiver reward_delta = stärkt, Negativer = schwächt
    FOR i IN 1..array_length(v_entities, 1) LOOP
        FOR j IN (i+1)..array_length(v_entities, 1) LOOP
            -- Konsistente Reihenfolge (lexikografisch) für PK-Konsistenz
            IF v_entities[i] <= v_entities[j] THEN
                v_entity_a := v_entities[i];
                v_entity_b := v_entities[j];
            ELSE
                v_entity_a := v_entities[j];
                v_entity_b := v_entities[i];
            END IF;

            -- Upsert: Gewicht += rate * reward_delta
            INSERT INTO meclaw.prototype_associations (prototype_a, prototype_b, weight, last_updated_seq)
            VALUES (
                v_entity_a,
                v_entity_b,
                v_hebbian_rate * p_reward_delta,
                v_current_seq
            )
            ON CONFLICT (prototype_a, prototype_b) DO UPDATE
            SET
                weight = meclaw.prototype_associations.weight + v_hebbian_rate * p_reward_delta,
                last_updated_seq = v_current_seq;
        END LOOP;
    END LOOP;
END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION meclaw.hebbian_update IS
'Hebbian Learning v2: Co-aktivierte Entities im gleichen Event stärken ihre Assoziation.
Erstellt automatisch Prototype-Stubs für noch nicht registrierte Entities (inkl. Embedding als Centroid).
Positiver reward_delta = Verbindung stärker, negativer = schwächer.
Idempotent via ON CONFLICT.';

-- =============================================================================
-- 4. Initialisierung (idempotent — läuft bei jedem Deploy)
-- =============================================================================

-- 4a. Entity-Stubs: Alle bekannten Entities als Prototype-Knoten registrieren
INSERT INTO meclaw.prototypes (id, agent_id, centroid, weight, activation_count, last_activated_seq, created_seq)
SELECT DISTINCT
    ee.entity_id,
    'meclaw:agent:walter',
    e.embedding,   -- Entity-Embedding als Centroid (NULL wenn kein Embedding)
    0.5,
    (SELECT COUNT(*) FROM meclaw.entity_events ee2 WHERE ee2.entity_id = ee.entity_id),
    (SELECT COALESCE(MAX(seq), 0) FROM meclaw.brain_events),
    (SELECT COALESCE(MAX(seq), 0) FROM meclaw.brain_events)
FROM meclaw.entity_events ee
JOIN meclaw.entities e ON e.id = ee.entity_id
WHERE NOT EXISTS (SELECT 1 FROM meclaw.prototypes p WHERE p.id = ee.entity_id)
ON CONFLICT (id) DO NOTHING;

-- 4b. Initiales Seeding wenn keine Prototypes mit Centroid vorhanden
DO $$
DECLARE
    v_count INT;
    v_created INT;
BEGIN
    SELECT COUNT(*) INTO v_count
    FROM meclaw.prototypes
    WHERE agent_id = 'meclaw:agent:walter' AND centroid IS NOT NULL;

    IF v_count = 0 THEN
        RAISE NOTICE 'Keine semantischen Prototypes. Starte seed_prototypes_from_events(10)...';
        SELECT meclaw.seed_prototypes_from_events(10, 'meclaw:agent:walter') INTO v_created;
        RAISE NOTICE 'Seeding: % Prototypes erstellt.', v_created;
    ELSE
        RAISE NOTICE 'Bereits % Prototypes mit Centroid für walter.', v_count;
    END IF;
END $$;

-- 4c. Initialer Hebbian-Lauf für alle bestehenden entity_events
DO $$
DECLARE
    v_event_id UUID;
    v_assoc_count INT;
BEGIN
    SELECT COUNT(*) INTO v_assoc_count FROM meclaw.prototype_associations;

    IF v_assoc_count = 0 THEN
        RAISE NOTICE 'Keine prototype_associations. Führe Hebbian-Init für alle Events durch...';

        FOR v_event_id IN
            SELECT DISTINCT event_id FROM meclaw.entity_events
        LOOP
            PERFORM meclaw.hebbian_update(v_event_id, 0.1);
        END LOOP;

        SELECT COUNT(*) INTO v_assoc_count FROM meclaw.prototype_associations;
        RAISE NOTICE 'Hebbian-Init: % Assoziationen erstellt.', v_assoc_count;
    ELSE
        RAISE NOTICE 'Bereits % prototype_associations vorhanden.', v_assoc_count;
    END IF;
END $$;

-- 4d. Novelty für alle brain_events berechnen die noch keine Novelty haben
DO $$
DECLARE
    v_event RECORD;
    v_processed INT := 0;
    v_proto_count INT;
BEGIN
    SELECT COUNT(*) INTO v_proto_count
    FROM meclaw.prototypes
    WHERE agent_id = 'meclaw:agent:walter' AND centroid IS NOT NULL;

    IF v_proto_count = 0 THEN
        RAISE NOTICE 'Keine Prototypes mit Centroid. Novelty-Berechnung übersprungen.';
        RETURN;
    END IF;

    FOR v_event IN
        SELECT id FROM meclaw.brain_events
        WHERE embedding IS NOT NULL AND novelty = 0
        ORDER BY seq ASC
    LOOP
        PERFORM meclaw.novelty_bee('meclaw:agent:walter', v_event.id);
        v_processed := v_processed + 1;
    END LOOP;

    RAISE NOTICE 'Novelty-Berechnung: % Events verarbeitet.', v_processed;
END $$;

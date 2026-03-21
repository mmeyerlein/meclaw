-- MeClaw v0.1.0 — E4 + E5: AGE Graph Edges für Prototype-Aktivierungen & Assoziationen
-- Date: 2026-03-21
-- Ref: docs/BRAIN.md (Prototype Engine, AGE Graph Intelligence)
--
-- E4: ACTIVATES Edges   → (:Event)-[:ACTIVATES {weight}]->(:Prototype)
--     Top-3 aktivierte Prototypes pro Event (höchste Cosine-Similarity)
--     Wird in novelty_bee v3 integriert (nach Prototype-Matching)
--
-- E5: ASSOCIATION Edges → (:Prototype)-[:ASSOCIATED {weight}]->(:Prototype)
--     Spiegelt prototype_associations als AGE Edges
--     Update nach hebbian_update v3 via backfill_prototype_graph()
--
-- TECHNISCHES:
--   - Funktionen nutzen $func$ als Body-Delimiter (da Cypher-Format $c$...$c$ verwendet)
--   - AGE Setup innerhalb jeder Funktion via LOAD + SET LOCAL search_path
--   - Prototype-IDs werden für Cypher via replace() apostrophen-escaped

-- =============================================================================
-- Hilfsfunktion: AGE in aktueller Session laden
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw._age_setup()
RETURNS VOID AS $func$
BEGIN
    LOAD 'age';
    SET LOCAL search_path = ag_catalog, meclaw, public;
END;
$func$ LANGUAGE plpgsql;

-- =============================================================================
-- E4: create_activates_edges(p_event_id, p_agent_id)
-- Erstellt ACTIVATES Edges für die Top-3 Prototypes eines Events
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw.create_activates_edges(
    p_event_id UUID,
    p_agent_id TEXT
)
RETURNS VOID AS $func$
DECLARE
    v_event_embedding vector(1536);
    v_proto           RECORD;
    v_event_id_safe   TEXT;
    v_proto_id_safe   TEXT;
BEGIN
    PERFORM meclaw._age_setup();

    -- Event-Embedding laden
    SELECT embedding INTO v_event_embedding
    FROM meclaw.brain_events WHERE id = p_event_id;

    IF v_event_embedding IS NULL THEN
        RETURN;
    END IF;

    -- Event-ID für Cypher sanitizen
    v_event_id_safe := replace(p_event_id::TEXT, '''', '''''');

    -- Event-Node in AGE erstellen (MERGE = idempotent)
    EXECUTE format(
        'SELECT * FROM ag_catalog.cypher(''meclaw_graph'', $c$ MERGE (e:Event {id: %L}) RETURN e $c$) AS (v ag_catalog.agtype)',
        v_event_id_safe
    );

    -- Top-3 Prototypes nach Cosine-Similarity
    FOR v_proto IN
        SELECT
            p.id,
            1.0 - (p.centroid <=> v_event_embedding) AS similarity
        FROM meclaw.prototypes p
        WHERE p.agent_id = p_agent_id
          AND p.centroid IS NOT NULL
        ORDER BY p.centroid <=> v_event_embedding
        LIMIT 3
    LOOP
        -- Nur positive Similarity
        CONTINUE WHEN v_proto.similarity <= 0.0;

        -- Prototype-ID sanitizen
        v_proto_id_safe := replace(v_proto.id, '''', '''''');

        -- Prototype-Node in AGE erstellen wenn noch nicht vorhanden
        EXECUTE format(
            'SELECT * FROM ag_catalog.cypher(''meclaw_graph'', $c$ MERGE (p:Prototype {id: %L}) RETURN p $c$) AS (v ag_catalog.agtype)',
            v_proto_id_safe
        );

        -- ACTIVATES Edge erstellen / aktualisieren (MERGE + SET weight)
        EXECUTE format(
            'SELECT * FROM ag_catalog.cypher(''meclaw_graph'', $c$
                MATCH (e:Event {id: %L}), (p:Prototype {id: %L})
                MERGE (e)-[r:ACTIVATES]->(p)
                SET r.weight = %s
                RETURN r
            $c$) AS (v ag_catalog.agtype)',
            v_event_id_safe,
            v_proto_id_safe,
            v_proto.similarity
        );
    END LOOP;
END;
$func$ LANGUAGE plpgsql;

COMMENT ON FUNCTION meclaw.create_activates_edges IS
'E4: Erstellt ACTIVATES Edges im AGE Graph für die Top-3 aktivierten Prototypes eines Events.
Aufgerufen nach novelty_bee. Erstellt Event- und Prototype-Nodes wenn noch nicht vorhanden.
Gewicht = Cosine-Similarity zwischen Event-Embedding und Prototype-Centroid.';

-- =============================================================================
-- novelty_bee v3 — integriert E4 ACTIVATES Edges
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw.novelty_bee(p_agent_id TEXT, p_event_id UUID)
RETURNS VOID AS $func$
DECLARE
    v_event_embedding vector(1536);
    v_max_similarity FLOAT := 0.0;
    v_novelty FLOAT;
    v_nearest_prototype_id TEXT;
    v_prototype_count INT;
    v_activation INT;
    v_alpha FLOAT;
    v_new_proto_id TEXT;
    v_new_proto_seq BIGINT;
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
        v_new_proto_id := p_agent_id || ':proto:' || gen_random_uuid()::text;
        SELECT seq INTO v_new_proto_seq FROM meclaw.brain_events WHERE id = p_event_id;

        INSERT INTO meclaw.prototypes (
            id, agent_id, centroid, weight, activation_count, last_activated_seq, created_seq
        ) VALUES (
            v_new_proto_id,
            p_agent_id,
            v_event_embedding,
            1.0,
            1,
            v_new_proto_seq,
            v_new_proto_seq
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

    -- E4: ACTIVATES Edges für Top-3 Prototypes erstellen
    PERFORM meclaw.create_activates_edges(p_event_id, p_agent_id);

    -- Audit-Log
    INSERT INTO meclaw.events (bee_type, event, payload)
    VALUES ('novelty_bee', 'novelty_computed', jsonb_build_object(
        'event_id', p_event_id,
        'agent_id', p_agent_id,
        'novelty', v_novelty,
        'nearest_prototype', v_nearest_prototype_id,
        'new_prototype_created', v_novelty > 0.7 OR v_prototype_count = 0,
        'prototype_count_before', v_prototype_count,
        'activates_edges_created', true
    ));
END;
$func$ LANGUAGE plpgsql;

COMMENT ON FUNCTION meclaw.novelty_bee IS
'Agent-level novelty scoring v3 (mit E4 ACTIVATES Edges).
Berechnet Distanz von Event-Embedding zu nächstem Prototype.
Novelty > 0.7 → Neuer Prototype. Sonst → Centroid-Update via Online Running Average.
Erstellt nach Prototype-Matching ACTIVATES Edges im AGE Graph (Top-3 Prototypes).
Kompatibel mit pgvector 0.8.x (via vector_to_float4 + string_agg, kein vector*scalar).';

-- =============================================================================
-- E5: upsert_association_edge(prototype_a, prototype_b, weight)
-- Erstellt/aktualisiert eine ASSOCIATED Edge im AGE Graph
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw.upsert_association_edge(
    p_prototype_a TEXT,
    p_prototype_b TEXT,
    p_weight FLOAT
)
RETURNS VOID AS $func$
DECLARE
    v_proto_a_safe TEXT;
    v_proto_b_safe TEXT;
BEGIN
    PERFORM meclaw._age_setup();

    -- IDs sanitizen (Apostrophe escapen)
    v_proto_a_safe := replace(p_prototype_a, '''', '''''');
    v_proto_b_safe := replace(p_prototype_b, '''', '''''');

    -- Prototype-Nodes in AGE erstellen wenn noch nicht vorhanden
    EXECUTE format(
        'SELECT * FROM ag_catalog.cypher(''meclaw_graph'', $c$ MERGE (p:Prototype {id: %L}) RETURN p $c$) AS (v ag_catalog.agtype)',
        v_proto_a_safe
    );
    EXECUTE format(
        'SELECT * FROM ag_catalog.cypher(''meclaw_graph'', $c$ MERGE (p:Prototype {id: %L}) RETURN p $c$) AS (v ag_catalog.agtype)',
        v_proto_b_safe
    );

    -- ASSOCIATED Edge: MERGE + SET weight
    EXECUTE format(
        'SELECT * FROM ag_catalog.cypher(''meclaw_graph'', $c$
            MATCH (a:Prototype {id: %L}), (b:Prototype {id: %L})
            MERGE (a)-[r:ASSOCIATED]->(b)
            SET r.weight = %s
            RETURN r
        $c$) AS (v ag_catalog.agtype)',
        v_proto_a_safe,
        v_proto_b_safe,
        p_weight
    );
END;
$func$ LANGUAGE plpgsql;

COMMENT ON FUNCTION meclaw.upsert_association_edge IS
'E5: Erstellt oder aktualisiert eine ASSOCIATED Edge im AGE Graph zwischen zwei Prototypes.
Erstellt Prototype-Nodes wenn noch nicht vorhanden. Idempotent via MERGE.';

-- =============================================================================
-- E5: hebbian_update v3 — aktualisiert AGE ASSOCIATED Edges nach Hebbian Learning
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw.hebbian_update(p_event_id UUID, p_reward_delta FLOAT)
RETURNS VOID AS $func$
DECLARE
    v_entities TEXT[];
    v_entity_a TEXT;
    v_entity_b TEXT;
    v_hebbian_rate FLOAT := 0.1;
    v_current_seq BIGINT;
    v_agent_id TEXT;
    v_new_weight FLOAT;
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
    FOR i IN 1..array_length(v_entities, 1) LOOP
        FOR j IN (i+1)..array_length(v_entities, 1) LOOP
            -- Konsistente Reihenfolge (lexikografisch)
            IF v_entities[i] <= v_entities[j] THEN
                v_entity_a := v_entities[i];
                v_entity_b := v_entities[j];
            ELSE
                v_entity_a := v_entities[j];
                v_entity_b := v_entities[i];
            END IF;

            -- Upsert in prototype_associations, Gewicht zurückgeben
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
                last_updated_seq = v_current_seq
            RETURNING meclaw.prototype_associations.weight INTO v_new_weight;

            -- E5: AGE ASSOCIATED Edge aktualisieren
            PERFORM meclaw.upsert_association_edge(v_entity_a, v_entity_b, v_new_weight);
        END LOOP;
    END LOOP;
END;
$func$ LANGUAGE plpgsql;

COMMENT ON FUNCTION meclaw.hebbian_update IS
'Hebbian Learning v3 (mit E5 ASSOCIATED Edges im AGE Graph).
Co-aktivierte Entities im gleichen Event stärken ihre Assoziation.
Erstellt automatisch Prototype-Stubs für noch nicht registrierte Entities.
Spiegelt jede Assoziation als ASSOCIATED Edge im AGE Graph (MERGE + SET weight).
Idempotent via ON CONFLICT.';

-- =============================================================================
-- E5: backfill_prototype_graph()
-- Spiegelt alle bestehenden prototype_associations in den AGE Graph
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw.backfill_prototype_graph()
RETURNS INT AS $func$
DECLARE
    v_assoc RECORD;
    v_count INT := 0;
BEGIN
    FOR v_assoc IN
        SELECT prototype_a, prototype_b, weight
        FROM meclaw.prototype_associations
        ORDER BY prototype_a, prototype_b
    LOOP
        PERFORM meclaw.upsert_association_edge(v_assoc.prototype_a, v_assoc.prototype_b, v_assoc.weight);
        v_count := v_count + 1;
    END LOOP;

    -- Log
    INSERT INTO meclaw.events (bee_type, event, payload)
    VALUES ('backfill_prototype_graph', 'backfill_complete', jsonb_build_object(
        'associations_mirrored', v_count
    ));

    RAISE NOTICE 'backfill_prototype_graph: % Assoziationen in AGE Graph gespiegelt.', v_count;
    RETURN v_count;
END;
$func$ LANGUAGE plpgsql;

COMMENT ON FUNCTION meclaw.backfill_prototype_graph IS
'E5: Spiegelt alle bestehenden prototype_associations als ASSOCIATED Edges in den AGE Graph.
Idempotent: Nutzt MERGE — bestehende Edges werden nur geupdated.
Aufruf: SELECT meclaw.backfill_prototype_graph();';

-- =============================================================================
-- Initialisierung: Backfill falls prototype_associations vorhanden
-- =============================================================================
DO $do$
DECLARE
    v_assoc_count INT;
    v_mirrored INT;
BEGIN
    SELECT COUNT(*) INTO v_assoc_count FROM meclaw.prototype_associations;

    IF v_assoc_count > 0 THEN
        RAISE NOTICE 'Starte backfill_prototype_graph für % Assoziationen...', v_assoc_count;
        SELECT meclaw.backfill_prototype_graph() INTO v_mirrored;
        RAISE NOTICE 'Backfill abgeschlossen: % ASSOCIATED Edges in AGE.', v_mirrored;
    ELSE
        RAISE NOTICE 'Keine prototype_associations vorhanden — Backfill übersprungen.';
    END IF;
END $do$;

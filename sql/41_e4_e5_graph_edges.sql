-- MeClaw v0.1.0 — E4 + E5: AGE Graph Edges for Prototype Activations & Associations
-- Date: 2026-03-21
-- Ref: docs/BRAIN.md (Prototype Engine, AGE Graph Intelligence)
--
-- E4: ACTIVATES edges   → (:Event)-[:ACTIVATES {weight}]->(:Prototype)
--     Top-3 activated prototypes per event (highest cosine similarity)
--     Integrated into novelty_bee v3 (after prototype matching)
--
-- E5: ASSOCIATION edges → (:Prototype)-[:ASSOCIATED {weight}]->(:Prototype)
--     Mirrors prototype_associations as AGE edges
--     Updated after hebbian_update v3 via backfill_prototype_graph()
--
-- TECHNICAL:
--   - Functions use $func$ as body delimiter (since Cypher format uses $c$...$c$)
--   - AGE setup inside each function via LOAD + SET LOCAL search_path
--   - Prototype IDs are apostrophe-escaped via replace() for Cypher

-- =============================================================================
-- Helper function: load AGE in current session
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
-- Creates ACTIVATES edges for the Top-3 prototypes of an event
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

    -- Load event embedding
    SELECT embedding INTO v_event_embedding
    FROM meclaw.brain_events WHERE id = p_event_id;

    IF v_event_embedding IS NULL THEN
        RETURN;
    END IF;

    -- Sanitize event ID for Cypher
    v_event_id_safe := replace(p_event_id::TEXT, '''', '''''');

    -- Create event node in AGE (MERGE = idempotent)
    EXECUTE format(
        'SELECT * FROM ag_catalog.cypher(''meclaw_graph'', $c$ MERGE (e:Event {id: %L}) RETURN e $c$) AS (v ag_catalog.agtype)',
        v_event_id_safe
    );

    -- Top-3 prototypes by cosine similarity
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
        -- Only positive similarity
        CONTINUE WHEN v_proto.similarity <= 0.0;

        -- Sanitize prototype ID
        v_proto_id_safe := replace(v_proto.id, '''', '''''');

        -- Create prototype node in AGE if not yet present
        EXECUTE format(
            'SELECT * FROM ag_catalog.cypher(''meclaw_graph'', $c$ MERGE (p:Prototype {id: %L}) RETURN p $c$) AS (v ag_catalog.agtype)',
            v_proto_id_safe
        );

        -- Create / update ACTIVATES edge (MERGE + SET weight)
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
'E4: Creates ACTIVATES edges in the AGE graph for the Top-3 activated prototypes of an event.
Called after novelty_bee. Creates event and prototype nodes if not yet present.
Weight = cosine similarity between event embedding and prototype centroid.';

-- =============================================================================
-- novelty_bee v3 — integrates E4 ACTIVATES edges
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
    -- Load event embedding
    SELECT embedding INTO v_event_embedding
    FROM meclaw.brain_events WHERE id = p_event_id;

    IF v_event_embedding IS NULL THEN
        RETURN; -- Embedding not yet available, process later
    END IF;

    -- Count prototypes for this agent with centroid
    SELECT COUNT(*) INTO v_prototype_count
    FROM meclaw.prototypes
    WHERE agent_id = p_agent_id AND centroid IS NOT NULL;

    -- Find nearest prototype
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

    -- Novelty score: 0 = identical to known prototype, 1 = completely new
    v_novelty := 1.0 - COALESCE(v_max_similarity, 0.0);

    -- Update brain_event with novelty
    UPDATE meclaw.brain_events
    SET novelty = v_novelty
    WHERE id = p_event_id;

    -- Decision: new prototype or update existing centroid
    IF v_novelty > 0.7 OR v_prototype_count = 0 THEN
        -- New concept detected → create prototype
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
        -- Known pattern → adjust centroid via online running average
        -- alpha = 1/(n+2): the more activations, the slower the drift
        SELECT activation_count INTO v_activation
        FROM meclaw.prototypes WHERE id = v_nearest_prototype_id;

        v_alpha := 1.0 / (v_activation + 2.0);

        -- pgvector 0.8.x: no vector*scalar operator → via vector_to_float4 + string_agg
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

    -- E4: create ACTIVATES edges for Top-3 prototypes
    PERFORM meclaw.create_activates_edges(p_event_id, p_agent_id);

    -- Audit log
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
'Agent-level novelty scoring v3 (with E4 ACTIVATES edges).
Calculates distance from event embedding to nearest prototype.
Novelty > 0.7 → new prototype. Otherwise → centroid update via online running average.
After prototype matching, creates ACTIVATES edges in the AGE graph (Top-3 prototypes).
Compatible with pgvector 0.8.x (via vector_to_float4 + string_agg, no vector*scalar).';

-- =============================================================================
-- E5: upsert_association_edge(prototype_a, prototype_b, weight)
-- Creates/updates an ASSOCIATED edge in the AGE graph
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

    -- Sanitize IDs (escape apostrophes)
    v_proto_a_safe := replace(p_prototype_a, '''', '''''');
    v_proto_b_safe := replace(p_prototype_b, '''', '''''');

    -- Create prototype nodes in AGE if not yet present
    EXECUTE format(
        'SELECT * FROM ag_catalog.cypher(''meclaw_graph'', $c$ MERGE (p:Prototype {id: %L}) RETURN p $c$) AS (v ag_catalog.agtype)',
        v_proto_a_safe
    );
    EXECUTE format(
        'SELECT * FROM ag_catalog.cypher(''meclaw_graph'', $c$ MERGE (p:Prototype {id: %L}) RETURN p $c$) AS (v ag_catalog.agtype)',
        v_proto_b_safe
    );

    -- ASSOCIATED edge: MERGE + SET weight
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
'E5: Creates or updates an ASSOCIATED edge in the AGE graph between two prototypes.
Creates prototype nodes if not yet present. Idempotent via MERGE.';

-- =============================================================================
-- E5: hebbian_update v3 — updates AGE ASSOCIATED edges after Hebbian learning
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
    -- All entities involved in this event
    SELECT array_agg(entity_id) INTO v_entities
    FROM meclaw.entity_events
    WHERE event_id = p_event_id;

    IF v_entities IS NULL OR array_length(v_entities, 1) < 2 THEN
        RETURN; -- No co-activation possible
    END IF;

    SELECT COALESCE(MAX(seq), 0) INTO v_current_seq FROM meclaw.brain_events;

    -- Agent ID of the event (fallback: walter)
    SELECT COALESCE(agent_id, 'meclaw:agent:walter') INTO v_agent_id
    FROM meclaw.brain_events WHERE id = p_event_id;

    -- Auto-stub: register all entities as prototype nodes if not already present
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

    -- Hebbian pairwise linking: co-activation strengthens association
    FOR i IN 1..array_length(v_entities, 1) LOOP
        FOR j IN (i+1)..array_length(v_entities, 1) LOOP
            -- Consistent ordering (lexicographic)
            IF v_entities[i] <= v_entities[j] THEN
                v_entity_a := v_entities[i];
                v_entity_b := v_entities[j];
            ELSE
                v_entity_a := v_entities[j];
                v_entity_b := v_entities[i];
            END IF;

            -- Upsert into prototype_associations, return new weight
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

            -- E5: update AGE ASSOCIATED edge
            PERFORM meclaw.upsert_association_edge(v_entity_a, v_entity_b, v_new_weight);
        END LOOP;
    END LOOP;
END;
$func$ LANGUAGE plpgsql;

COMMENT ON FUNCTION meclaw.hebbian_update IS
'Hebbian Learning v3 (with E5 ASSOCIATED edges in the AGE graph).
Co-activated entities in the same event strengthen their association.
Automatically creates prototype stubs for not-yet-registered entities.
Mirrors each association as an ASSOCIATED edge in the AGE graph (MERGE + SET weight).
Idempotent via ON CONFLICT.';

-- =============================================================================
-- E5: backfill_prototype_graph()
-- Mirrors all existing prototype_associations into the AGE graph
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

    RAISE NOTICE 'backfill_prototype_graph: % associations mirrored into AGE graph.', v_count;
    RETURN v_count;
END;
$func$ LANGUAGE plpgsql;

COMMENT ON FUNCTION meclaw.backfill_prototype_graph IS
'E5: Mirrors all existing prototype_associations as ASSOCIATED edges into the AGE graph.
Idempotent: uses MERGE — existing edges are only updated.
Call: SELECT meclaw.backfill_prototype_graph();';

-- =============================================================================
-- Initialization: backfill if prototype_associations exist
-- =============================================================================
DO $do$
DECLARE
    v_assoc_count INT;
    v_mirrored INT;
BEGIN
    SELECT COUNT(*) INTO v_assoc_count FROM meclaw.prototype_associations;

    IF v_assoc_count > 0 THEN
        RAISE NOTICE 'Starting backfill_prototype_graph for % associations...', v_assoc_count;
        SELECT meclaw.backfill_prototype_graph() INTO v_mirrored;
        RAISE NOTICE 'Backfill complete: % ASSOCIATED edges in AGE.', v_mirrored;
    ELSE
        RAISE NOTICE 'No prototype_associations present — backfill skipped.';
    END IF;
END $do$;

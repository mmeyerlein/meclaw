-- MeClaw v0.1.0 — Phase C2: Activate Prototypes
-- Date: 2026-03-21
-- Ref: docs/BRAIN.md (Prototype Engine, Novelty Bee, Hebbian Learning)
--
-- Tasks:
-- 1. seed_prototypes_from_events(limit INT) — Initial prototypes from brain_events
-- 2. novelty_bee v2: centroid update via running average for known patterns
-- 3. hebbian_update v2: more robust against missing prototypes, auto-stub entities

-- =============================================================================
-- 1. seed_prototypes_from_events — Diverse prototypes from Top-N brain_events
-- =============================================================================
-- Strategy: Greedy Diversity Selection ("MaxMin")
--   - Choose first event (highest reward) as seed
--   - Each subsequent event = max. min-distance to all existing prototypes
--   - Result: maximum diverse coverage of the embedding space
--
-- Important: brain_events have no own agent_id (NULL), therefore
-- prototypes are assigned to the default agent p_agent_id.
-- Idempotent: existing prototypes are not overwritten.
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
    -- Check whether agent exists in entities
    IF NOT EXISTS (SELECT 1 FROM meclaw.entities WHERE id = p_agent_id) THEN
        RAISE EXCEPTION 'Agent % does not exist in entities', p_agent_id;
    END IF;

    -- Count existing prototypes with centroid
    SELECT COUNT(*) INTO v_existing_count
    FROM meclaw.prototypes WHERE agent_id = p_agent_id AND centroid IS NOT NULL;

    IF v_existing_count > 0 THEN
        RAISE NOTICE 'Agent % already has % prototypes with centroid. Adding new ones only.',
            p_agent_id, v_existing_count;
    END IF;

    -- Temporary helper tables (ON COMMIT DROP = auto-removed after transaction)
    CREATE TEMP TABLE _seed_candidates ON COMMIT DROP AS
    SELECT be.id, be.seq, be.embedding, be.reward
    FROM meclaw.brain_events be
    WHERE be.embedding IS NOT NULL
    ORDER BY be.reward DESC, be.seq ASC;

    CREATE TEMP TABLE _selected_seeds ON COMMIT DROP AS
    SELECT id::uuid AS event_id, seq, embedding
    FROM meclaw.brain_events WHERE FALSE; -- empty structure

    -- First selection: best event (Seed 0) — or if prototypes exist,
    -- take the most diverse event relative to existing prototypes
    IF v_existing_count = 0 THEN
        -- No prototype yet: best event as first seed
        SELECT id, seq, embedding
        INTO v_seed_id, v_seq, v_seed_embedding
        FROM _seed_candidates
        LIMIT 1;
    ELSE
        -- Prototypes already exist: choose most diverse event
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

        -- Only proceed if the event is still novel enough
        IF v_min_dist IS NOT NULL AND (1.0 - v_min_dist) < 0.3 THEN
            RAISE NOTICE 'All events are already well covered by existing prototypes (min_dist=%).', v_min_dist;
            DROP TABLE IF EXISTS _seed_candidates;
            DROP TABLE IF EXISTS _selected_seeds;
            RETURN 0;
        END IF;
    END IF;

    IF v_seed_id IS NULL THEN
        RAISE NOTICE 'No brain_events with embeddings found.';
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

    -- Greedy MaxMin: choose event with greatest min-distance to all existing seeds
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
            -- Only add if still sufficiently diverse (cosine distance > 0.2)
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

    RAISE NOTICE 'seed_prototypes_from_events: % prototypes created for agent %.', v_created, p_agent_id;
    RETURN v_created;
END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION meclaw.seed_prototypes_from_events IS
'Creates initial prototypes from the Top-N brain_events using Greedy MaxMin Diversity.
Selects events to achieve maximum coverage of the embedding space.
Idempotent: respects existing prototypes.
Parameters: p_limit = max new prototypes, p_agent_id = target agent.';

-- =============================================================================
-- 2. novelty_bee v2 — centroid update (running average) for known patterns
-- =============================================================================
-- Compatible with pgvector 0.8.x (no vector*scalar operator):
-- Uses vector_to_float4() + string_agg for element-wise calculation.
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

    -- Audit log
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
'Agent-level novelty scoring v2. Calculates distance from event embedding to nearest prototype.
Novelty > 0.7 → new prototype. Otherwise → centroid update via online running average.
Compatible with pgvector 0.8.x (via vector_to_float4 + string_agg, no vector*scalar).';

-- =============================================================================
-- 3. hebbian_update v2 — more robust, auto-stub for entities without prototype entry
-- =============================================================================
-- Entities are used as concept nodes in the prototype graph.
-- Co-activation in the same event strengthens their association (Hebbian rule).
-- If no prototype entry exists for an entity yet: it is created automatically.
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
    -- Use entity embedding as centroid if available
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
    -- Positive reward_delta = strengthen, negative = weaken
    FOR i IN 1..array_length(v_entities, 1) LOOP
        FOR j IN (i+1)..array_length(v_entities, 1) LOOP
            -- Consistent ordering (lexicographic) for PK consistency
            IF v_entities[i] <= v_entities[j] THEN
                v_entity_a := v_entities[i];
                v_entity_b := v_entities[j];
            ELSE
                v_entity_a := v_entities[j];
                v_entity_b := v_entities[i];
            END IF;

            -- Upsert: weight += rate * reward_delta
            INSERT INTO meclaw.prototype_associations (prototype_a, prototype_b, weight, last_updated_seq)
            VALUES (
                v_entity_a,
                v_entity_b,
                v_hebbian_rate * p_reward_delta,
                v_current_seq
            )
            ON CONFLICT (prototype_a, prototype_b) DO UPDATE
            SET weight = meclaw.prototype_associations.weight + v_hebbian_rate * p_reward_delta,
                last_updated_seq = v_current_seq;
        END LOOP;
    END LOOP;
END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION meclaw.hebbian_update IS
'Hebbian Learning v2: co-activated entities in the same event strengthen their association.
Automatically creates prototype stubs for not-yet-registered entities (including embedding as centroid).
Positive reward_delta = stronger connection, negative = weaker.
Idempotent via ON CONFLICT.';

-- =============================================================================
-- 4. Initialization (idempotent — runs on every deploy)
-- =============================================================================

-- 4a. Entity stubs: register all known entities as prototype nodes
INSERT INTO meclaw.prototypes (id, agent_id, centroid, weight, activation_count, last_activated_seq, created_seq)
SELECT DISTINCT
    ee.entity_id,
    'meclaw:agent:walter',
    e.embedding,   -- entity embedding as centroid (NULL if no embedding)
    0.5,
    (SELECT COUNT(*) FROM meclaw.entity_events ee2 WHERE ee2.entity_id = ee.entity_id),
    (SELECT COALESCE(MAX(seq), 0) FROM meclaw.brain_events),
    (SELECT COALESCE(MAX(seq), 0) FROM meclaw.brain_events)
FROM meclaw.entity_events ee
JOIN meclaw.entities e ON e.id = ee.entity_id
WHERE NOT EXISTS (SELECT 1 FROM meclaw.prototypes p WHERE p.id = ee.entity_id)
ON CONFLICT (id) DO NOTHING;

-- 4b. Initial seeding if no prototypes with centroid exist
DO $$
DECLARE
    v_count INT;
    v_created INT;
BEGIN
    SELECT COUNT(*) INTO v_count
    FROM meclaw.prototypes
    WHERE agent_id = 'meclaw:agent:walter' AND centroid IS NOT NULL;

    IF v_count = 0 THEN
        RAISE NOTICE 'No semantic prototypes found. Starting seed_prototypes_from_events(10)...';
        SELECT meclaw.seed_prototypes_from_events(10, 'meclaw:agent:walter') INTO v_created;
        RAISE NOTICE 'Seeding: % prototypes created.', v_created;
    ELSE
        RAISE NOTICE 'Already % prototypes with centroid for walter.', v_count;
    END IF;
END $$;

-- 4c. Initial Hebbian pass for all existing entity_events
DO $$
DECLARE
    v_event_id UUID;
    v_assoc_count INT;
BEGIN
    SELECT COUNT(*) INTO v_assoc_count FROM meclaw.prototype_associations;

    IF v_assoc_count = 0 THEN
        RAISE NOTICE 'No prototype_associations found. Running Hebbian init for all events...';

        FOR v_event_id IN
            SELECT DISTINCT event_id FROM meclaw.entity_events
        LOOP
            PERFORM meclaw.hebbian_update(v_event_id, 0.1);
        END LOOP;

        SELECT COUNT(*) INTO v_assoc_count FROM meclaw.prototype_associations;
        RAISE NOTICE 'Hebbian init: % associations created.', v_assoc_count;
    ELSE
        RAISE NOTICE 'Already % prototype_associations present.', v_assoc_count;
    END IF;
END $$;

-- 4d. Compute novelty for all brain_events that have no novelty yet
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
        RAISE NOTICE 'No prototypes with centroid. Novelty computation skipped.';
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

    RAISE NOTICE 'Novelty computation: % events processed.', v_processed;
END $$;

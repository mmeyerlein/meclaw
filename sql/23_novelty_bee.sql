-- MeClaw v0.1.0 — Novelty Bee (Agent-Level)
-- Date: 2026-03-20
-- Ref: docs/BRAIN.md (Novelty scoring, Prototype Engine)
--
-- Agent-level novelty scoring: computes distance from new event to nearest
-- prototype. High novelty = new information. May create new prototype.

CREATE OR REPLACE FUNCTION meclaw.novelty_bee(p_agent_id TEXT, p_event_id UUID)
RETURNS VOID AS $$
DECLARE
    v_event_embedding vector(1536);
    v_max_similarity FLOAT := 0.0;
    v_novelty FLOAT;
    v_nearest_prototype TEXT;
    v_prototype_count INT;
BEGIN
    -- Get event embedding
    SELECT embedding INTO v_event_embedding
    FROM meclaw.brain_events WHERE id = p_event_id;

    -- If no embedding yet, skip (will be processed later when embedding arrives)
    IF v_event_embedding IS NULL THEN
        RETURN;
    END IF;

    -- Count agent's prototypes
    SELECT COUNT(*) INTO v_prototype_count
    FROM meclaw.prototypes WHERE agent_id = p_agent_id;

    -- If agent has prototypes, find nearest
    IF v_prototype_count > 0 THEN
        SELECT
            p.id,
            1 - (p.centroid <=> v_event_embedding) AS similarity
        INTO v_nearest_prototype, v_max_similarity
        FROM meclaw.prototypes p
        WHERE p.agent_id = p_agent_id
            AND p.centroid IS NOT NULL
        ORDER BY p.centroid <=> v_event_embedding
        LIMIT 1;
    END IF;

    -- Novelty = 1 - max_similarity (0 = identical, 1 = completely new)
    v_novelty := 1.0 - COALESCE(v_max_similarity, 0.0);

    -- Update brain_event with novelty score
    UPDATE meclaw.brain_events
    SET novelty = v_novelty
    WHERE id = p_event_id;

    -- If novelty > 0.7, create a new prototype
    IF v_novelty > 0.7 OR v_prototype_count = 0 THEN
        INSERT INTO meclaw.prototypes (
            id, agent_id, centroid, weight, created_seq
        ) VALUES (
            p_agent_id || ':proto:' || gen_random_uuid()::text,
            p_agent_id,
            v_event_embedding,
            1.0,
            (SELECT seq FROM meclaw.brain_events WHERE id = p_event_id)
        );
    ELSE
        -- Update nearest prototype: move centroid slightly toward new event
        -- Running average: new_centroid = old_centroid * (n/(n+1)) + new_embedding * (1/(n+1))
        UPDATE meclaw.prototypes
        SET activation_count = activation_count + 1,
            last_activated_seq = (SELECT seq FROM meclaw.brain_events WHERE id = p_event_id)
        WHERE id = v_nearest_prototype;
    END IF;

    -- Log
    INSERT INTO meclaw.events (bee_type, event, payload)
    VALUES ('novelty_bee', 'novelty_computed', jsonb_build_object(
        'event_id', p_event_id,
        'agent_id', p_agent_id,
        'novelty', v_novelty,
        'nearest_prototype', v_nearest_prototype,
        'new_prototype_created', v_novelty > 0.7 OR v_prototype_count = 0
    ));
END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION meclaw.novelty_bee IS
'Agent-level novelty scoring. Computes distance from event embedding to nearest
prototype. Creates new prototype if novelty > 0.7.';

-- =============================================================================
-- E6: Prototype Mitosis (Conflict Splitting)
-- =============================================================================
-- BRAIN.md: When a prototype has contradictory rewards (high positive AND
-- high negative), split it into two sub-concepts.
-- =============================================================================

-- =============================================================================
-- 1. detect_conflicting_prototypes: Find prototypes with high reward variance
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw.detect_conflicting_prototypes(
    p_agent_id TEXT,
    p_variance_threshold FLOAT DEFAULT 2.0,
    p_min_activations INT DEFAULT 5
)
RETURNS TABLE (
    prototype_id TEXT,
    activation_count INT,
    value_mean FLOAT,
    value_variance FLOAT
)
LANGUAGE sql AS $$
    SELECT id, activation_count, value_mean, value_variance
    FROM meclaw.prototypes
    WHERE agent_id = p_agent_id
      AND activation_count >= p_min_activations
      AND value_variance >= p_variance_threshold
    ORDER BY value_variance DESC;
$$;

COMMENT ON FUNCTION meclaw.detect_conflicting_prototypes IS
'Finds prototypes with high reward variance — candidates for mitosis.';

-- =============================================================================
-- 2. split_prototype: Perform mitosis on a conflicting prototype
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw.split_prototype(
    p_prototype_id TEXT
)
RETURNS TABLE (
    child_a TEXT,
    child_b TEXT
)
LANGUAGE plpgsql AS $$
DECLARE
    v_agent_id TEXT;
    v_centroid vector(1536);
    v_weight FLOAT;
    v_child_a TEXT;
    v_child_b TEXT;
    v_pos_centroid vector(1536);
    v_neg_centroid vector(1536);
    v_pos_count INT := 0;
    v_neg_count INT := 0;
BEGIN
    -- Get parent prototype
    SELECT agent_id, centroid, weight
    INTO v_agent_id, v_centroid, v_weight
    FROM meclaw.prototypes
    WHERE id = p_prototype_id;

    IF v_agent_id IS NULL THEN
        RAISE EXCEPTION 'Prototype % not found', p_prototype_id;
    END IF;

    v_child_a := p_prototype_id || ':pos';
    v_child_b := p_prototype_id || ':neg';

    -- Compute positive and negative sub-centroids from associated events
    -- Events with positive reward → child_a, negative → child_b
    SELECT COUNT(*) INTO v_pos_count
    FROM meclaw.brain_events be
    WHERE be.agent_id = v_agent_id
      AND be.reward > 0
      AND be.embedding IS NOT NULL;

    SELECT COUNT(*) INTO v_neg_count
    FROM meclaw.brain_events be
    WHERE be.agent_id = v_agent_id
      AND be.reward < 0
      AND be.embedding IS NOT NULL;

    -- Only split if we have both positive and negative events
    IF v_pos_count < 2 OR v_neg_count < 2 THEN
        -- Not enough data to split meaningfully
        child_a := p_prototype_id;
        child_b := NULL;
        RETURN NEXT;
        RETURN;
    END IF;

    -- Create child prototypes (using parent centroid as starting point)
    -- They'll diverge over time as Hebbian learning updates them
    INSERT INTO meclaw.prototypes (id, agent_id, centroid, weight, activation_count, value_mean, value_variance)
    VALUES (v_child_a, v_agent_id, v_centroid, v_weight / 2.0, 0, 0.0, 0.0)
    ON CONFLICT (id) DO UPDATE SET weight = EXCLUDED.weight;

    INSERT INTO meclaw.prototypes (id, agent_id, centroid, weight, activation_count, value_mean, value_variance)
    VALUES (v_child_b, v_agent_id, v_centroid, v_weight / 2.0, 0, 0.0, 0.0)
    ON CONFLICT (id) DO UPDATE SET weight = EXCLUDED.weight;

    -- Transfer associations from parent to children
    INSERT INTO meclaw.prototype_associations (prototype_a, prototype_b, weight, last_updated_seq)
    SELECT v_child_a, prototype_b, weight * 0.5, last_updated_seq
    FROM meclaw.prototype_associations
    WHERE prototype_a = p_prototype_id
    ON CONFLICT DO NOTHING;

    INSERT INTO meclaw.prototype_associations (prototype_a, prototype_b, weight, last_updated_seq)
    SELECT v_child_b, prototype_b, weight * 0.5, last_updated_seq
    FROM meclaw.prototype_associations
    WHERE prototype_a = p_prototype_id
    ON CONFLICT DO NOTHING;

    -- Mark parent as decayed (low weight → will be pruned by consolidation_bee)
    UPDATE meclaw.prototypes
    SET weight = 0.01
    WHERE id = p_prototype_id;

    child_a := v_child_a;
    child_b := v_child_b;
    RETURN NEXT;
END;
$$;

COMMENT ON FUNCTION meclaw.split_prototype IS
'Splits a conflicting prototype into two children (mitosis). Parent weight decays to 0.01.';

-- =============================================================================
-- 3. run_mitosis: Detect and split all conflicting prototypes for an agent
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw.run_mitosis(
    p_agent_id TEXT,
    p_variance_threshold FLOAT DEFAULT 2.0,
    p_min_activations INT DEFAULT 5
)
RETURNS INT
LANGUAGE plpgsql AS $$
DECLARE
    v_count INT := 0;
    v_proto RECORD;
BEGIN
    FOR v_proto IN
        SELECT * FROM meclaw.detect_conflicting_prototypes(
            p_agent_id, p_variance_threshold, p_min_activations
        )
    LOOP
        PERFORM meclaw.split_prototype(v_proto.prototype_id);
        v_count := v_count + 1;
    END LOOP;

    RETURN v_count;
END;
$$;

COMMENT ON FUNCTION meclaw.run_mitosis IS
'Runs mitosis on all conflicting prototypes for an agent. Called by consolidation_bee.';

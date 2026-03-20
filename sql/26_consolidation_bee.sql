-- MeClaw v0.1.0 — Phase 4: Consolidation Bee + User Modeling
-- Date: 2026-03-20
-- Ref: docs/BRAIN.md (Sleep Consolidation, Entity Observations, Prototype Mitosis)
--
-- Nightly maintenance: prune, merge, consolidate, update profiles.
-- Runs via pg_cron at 03:00 UTC.

-- =============================================================================
-- 1. Core Consolidation Bee (runs nightly per agent)
-- =============================================================================

CREATE OR REPLACE FUNCTION meclaw.consolidation_bee(p_agent_id TEXT)
RETURNS JSONB AS $$
DECLARE
    v_pruned_assocs INT := 0;
    v_merged_protos INT := 0;
    v_split_protos INT := 0;
    v_consolidated_obs INT := 0;
    v_updated_profiles INT := 0;
    v_stale_decisions INT := 0;
    v_current_seq BIGINT;
BEGIN
    SELECT COALESCE(MAX(seq), 0) INTO v_current_seq FROM meclaw.brain_events;

    -- =========================================================================
    -- Step 1: Prune weak prototype associations (weight < 0.1)
    -- =========================================================================
    DELETE FROM meclaw.prototype_associations
    WHERE (prototype_a IN (SELECT id FROM meclaw.prototypes WHERE agent_id = p_agent_id)
        OR prototype_b IN (SELECT id FROM meclaw.prototypes WHERE agent_id = p_agent_id))
        AND weight < 0.1;
    GET DIAGNOSTICS v_pruned_assocs = ROW_COUNT;

    -- =========================================================================
    -- Step 2: Merge similar prototypes (cosine > 0.92, compatible rewards)
    -- =========================================================================
    WITH merge_candidates AS (
        SELECT p1.id AS id_a, p2.id AS id_b,
            1 - (p1.centroid <=> p2.centroid) AS similarity,
            ABS(p1.value_mean - p2.value_mean) AS reward_diff
        FROM meclaw.prototypes p1
        JOIN meclaw.prototypes p2 ON p1.id < p2.id
        WHERE p1.agent_id = p_agent_id AND p2.agent_id = p_agent_id
            AND p1.centroid IS NOT NULL AND p2.centroid IS NOT NULL
            AND 1 - (p1.centroid <=> p2.centroid) > 0.92
            AND ABS(p1.value_mean - p2.value_mean) < 1.0  -- compatible rewards
    )
    UPDATE meclaw.prototypes p SET
        -- Move centroid toward the merged prototype (weighted average)
        activation_count = p.activation_count + merged.activation_count,
        last_activated_seq = GREATEST(p.last_activated_seq, merged.last_activated_seq),
        value_mean = (p.value_mean * p.activation_count + merged.value_mean * merged.activation_count) 
                     / NULLIF(p.activation_count + merged.activation_count, 0)
    FROM merge_candidates mc
    JOIN meclaw.prototypes merged ON merged.id = mc.id_b
    WHERE p.id = mc.id_a;
    GET DIAGNOSTICS v_merged_protos = ROW_COUNT;

    -- Delete the merged-away prototypes
    DELETE FROM meclaw.prototype_associations
    WHERE prototype_a IN (
        SELECT mc.id_b FROM (
            SELECT p1.id AS id_a, p2.id AS id_b
            FROM meclaw.prototypes p1 JOIN meclaw.prototypes p2 ON p1.id < p2.id
            WHERE p1.agent_id = p_agent_id AND p2.agent_id = p_agent_id
                AND p1.centroid IS NOT NULL AND p2.centroid IS NOT NULL
                AND 1 - (p1.centroid <=> p2.centroid) > 0.92
        ) mc
    ) OR prototype_b IN (
        SELECT mc.id_b FROM (
            SELECT p1.id AS id_a, p2.id AS id_b
            FROM meclaw.prototypes p1 JOIN meclaw.prototypes p2 ON p1.id < p2.id
            WHERE p1.agent_id = p_agent_id AND p2.agent_id = p_agent_id
                AND p1.centroid IS NOT NULL AND p2.centroid IS NOT NULL
                AND 1 - (p1.centroid <=> p2.centroid) > 0.92
        ) mc
    );

    -- =========================================================================
    -- Step 3: Prototype Mitosis (split conflicting prototypes)
    -- High activation + high reward variance → concept has conflicting signals
    -- =========================================================================
    -- For now: flag prototypes with high variance for future splitting
    -- Full mitosis requires re-clustering which is expensive
    UPDATE meclaw.prototypes
    SET weight = weight * 0.9  -- decay conflicting prototypes
    WHERE agent_id = p_agent_id
        AND activation_count > 10
        AND value_variance > 2.0;
    GET DIAGNOSTICS v_split_protos = ROW_COUNT;

    -- =========================================================================
    -- Step 4: Consolidate Entity Observations
    -- Merge repeated observations, increase confidence, prune contradictions
    -- =========================================================================
    WITH obs_groups AS (
        SELECT entity_id, key,
            COUNT(*) as obs_count,
            MAX(confidence) as max_confidence,
            MAX(last_observed_seq) as latest_seq
        FROM meclaw.entity_observations
        WHERE agent_id = p_agent_id
        GROUP BY entity_id, key
        HAVING COUNT(*) > 1
    )
    UPDATE meclaw.entity_observations eo SET
        confidence = LEAST(1.0, eo.confidence + 0.1 * (og.obs_count - 1)),
        observation_count = og.obs_count,
        last_observed_seq = og.latest_seq,
        updated_at = clock_timestamp()
    FROM obs_groups og
    WHERE eo.entity_id = og.entity_id
        AND eo.key = og.key
        AND eo.agent_id = p_agent_id
        AND eo.id = (
            -- Keep the most confident observation per entity+key
            SELECT id FROM meclaw.entity_observations
            WHERE entity_id = og.entity_id AND key = og.key AND agent_id = p_agent_id
            ORDER BY confidence DESC, last_observed_seq DESC LIMIT 1
        );
    GET DIAGNOSTICS v_consolidated_obs = ROW_COUNT;

    -- Delete duplicate observations (keep only the consolidated one)
    DELETE FROM meclaw.entity_observations eo
    WHERE agent_id = p_agent_id
        AND EXISTS (
            SELECT 1 FROM meclaw.entity_observations eo2
            WHERE eo2.entity_id = eo.entity_id
                AND eo2.key = eo.key
                AND eo2.agent_id = p_agent_id
                AND eo2.confidence > eo.confidence
                AND eo2.id != eo.id
        );

    -- =========================================================================
    -- Step 5: Update observed_profile on entities
    -- =========================================================================
    UPDATE meclaw.entities e SET
        observed_profile = sub.profile,
        -- Update observed neural_matrix traits if communication observations exist
        traits = CASE
            WHEN e.traits IS NULL THEN NULL
            ELSE e.traits  -- Keep existing traits, extend via observations later
        END
    FROM (
        SELECT entity_id,
            jsonb_object_agg(
                key,
                jsonb_build_object('value', value->'value', 'confidence', confidence)
            ) AS profile
        FROM meclaw.entity_observations
        WHERE agent_id = p_agent_id AND confidence >= 0.5
        GROUP BY entity_id
    ) sub
    WHERE e.id = sub.entity_id;
    GET DIAGNOSTICS v_updated_profiles = ROW_COUNT;

    -- =========================================================================
    -- Step 6: Mark stale decisions
    -- =========================================================================
    UPDATE meclaw.decision_traces SET
        reward = reward * 0.95  -- slow decay for uncited decisions
    WHERE agent_id = p_agent_id
        AND seq IS NOT NULL
        AND seq < v_current_seq - 1000;
    GET DIAGNOSTICS v_stale_decisions = ROW_COUNT;

    -- =========================================================================
    -- Step 7: Recalibrate Hebbian weights
    -- Decay all association weights slightly (forgetting curve)
    -- =========================================================================
    UPDATE meclaw.prototype_associations SET
        weight = weight * 0.95
    WHERE prototype_a IN (SELECT id FROM meclaw.prototypes WHERE agent_id = p_agent_id)
       OR prototype_b IN (SELECT id FROM meclaw.prototypes WHERE agent_id = p_agent_id);

    -- Log consolidation results
    INSERT INTO meclaw.events (bee_type, event, payload)
    VALUES ('consolidation_bee', 'consolidation_complete', jsonb_build_object(
        'agent_id', p_agent_id,
        'pruned_associations', v_pruned_assocs,
        'merged_prototypes', v_merged_protos,
        'flagged_for_mitosis', v_split_protos,
        'consolidated_observations', v_consolidated_obs,
        'updated_profiles', v_updated_profiles,
        'decayed_decisions', v_stale_decisions,
        'current_seq', v_current_seq
    ));

    RETURN jsonb_build_object(
        'pruned_associations', v_pruned_assocs,
        'merged_prototypes', v_merged_protos,
        'flagged_for_mitosis', v_split_protos,
        'consolidated_observations', v_consolidated_obs,
        'updated_profiles', v_updated_profiles,
        'decayed_decisions', v_stale_decisions
    );
END;
$$ LANGUAGE plpgsql;

-- =============================================================================
-- 2. Nightly runner: consolidate all agents
-- =============================================================================

CREATE OR REPLACE FUNCTION meclaw.consolidation_nightly()
RETURNS VOID AS $$
DECLARE
    v_agent RECORD;
    v_result JSONB;
BEGIN
    FOR v_agent IN 
        SELECT DISTINCT e.id FROM meclaw.entities e
        WHERE e.entity_type = 'agent'
    LOOP
        v_result := meclaw.consolidation_bee(v_agent.id);
        RAISE NOTICE 'Consolidated agent %: %', v_agent.id, v_result;
    END LOOP;
END;
$$ LANGUAGE plpgsql;

-- =============================================================================
-- 3. pg_cron job (03:00 UTC nightly)
-- =============================================================================

SELECT cron.schedule(
    'consolidation-nightly',
    '0 3 * * *',
    'SELECT meclaw.consolidation_nightly()'
);

-- =============================================================================
-- 4. Entity observation helper: record an observation
-- =============================================================================

CREATE OR REPLACE FUNCTION meclaw.observe_entity(
    p_agent_id TEXT,
    p_entity_id TEXT,
    p_channel_id UUID,
    p_type TEXT,        -- 'preference', 'behavior', 'fact', 'relationship'
    p_key TEXT,         -- 'communication_style', 'timezone', etc.
    p_value JSONB,      -- {value: 'direct', evidence: '...'}
    p_confidence FLOAT DEFAULT 0.6
) RETURNS UUID AS $$
DECLARE
    v_existing_id UUID;
    v_obs_id UUID;
    v_current_seq BIGINT;
BEGIN
    SELECT COALESCE(MAX(seq), 0) INTO v_current_seq FROM meclaw.brain_events;

    -- Check if observation already exists
    SELECT id INTO v_existing_id FROM meclaw.entity_observations
    WHERE entity_id = p_entity_id AND agent_id = p_agent_id AND key = p_key
    ORDER BY confidence DESC LIMIT 1;

    IF v_existing_id IS NOT NULL THEN
        -- Update existing: increase confidence, update value if higher confidence
        UPDATE meclaw.entity_observations SET
            confidence = LEAST(1.0, confidence + 0.1),
            observation_count = observation_count + 1,
            last_observed_seq = v_current_seq,
            value = CASE WHEN p_confidence > confidence THEN p_value ELSE value END,
            updated_at = clock_timestamp()
        WHERE id = v_existing_id
        RETURNING id INTO v_obs_id;
    ELSE
        -- New observation
        INSERT INTO meclaw.entity_observations (
            entity_id, agent_id, channel_id, observation_type, key, value,
            confidence, first_observed_seq, last_observed_seq
        ) VALUES (
            p_entity_id, p_agent_id, p_channel_id, p_type, p_key, p_value,
            p_confidence, v_current_seq, v_current_seq
        ) RETURNING id INTO v_obs_id;
    END IF;

    RETURN v_obs_id;
END;
$$ LANGUAGE plpgsql;

-- =============================================================================
-- 5. Workspace Agent stub
-- =============================================================================

-- Create workspace entity if not exists
INSERT INTO meclaw.entities (id, canonical_name, entity_type, neural_matrix, traits)
VALUES (
    'meclaw:workspace:default',
    'MeClaw Default Workspace',
    'workspace',
    '{"logic": 0.8, "creativity": 0.6, "empathy": 0.5, "adaptability": 0.7, "charisma": 0.4, "reliability": 0.9}'::jsonb,
    '{"culture": "engineering-first", "values": ["transparency", "iteration", "autonomy"]}'::jsonb
) ON CONFLICT (id) DO NOTHING;

COMMENT ON FUNCTION meclaw.consolidation_bee IS
'Nightly consolidation for an agent: prune associations, merge prototypes,
consolidate observations, update profiles, decay stale decisions.';

COMMENT ON FUNCTION meclaw.observe_entity IS
'Record an observation about an entity. Upserts: increases confidence if
observation with same key exists, creates new otherwise.';

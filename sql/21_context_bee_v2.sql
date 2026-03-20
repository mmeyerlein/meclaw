-- MeClaw v0.1.0 — Context Bee V2 (Agent-Level, with Memory Retrieval)
-- Date: 2026-03-20
-- Ref: docs/BRAIN.md (Context Pipeline: Compression + Caching)
--
-- Extends the existing context_bee with:
--   1. Memory retrieval via retrieve_bee (injects relevant memories into context)
--   2. Agent identification (which agent is processing this message)
--   3. Prepares for Phase 2: static prefix compression + cache breakpoint
--
-- Does NOT replace the existing context_bee — adds a v2 version.
-- The graph routing can switch between context_bee and context_bee_v2.

-- =============================================================================
-- 1. Context Bee V2: with memory retrieval
-- =============================================================================

CREATE OR REPLACE FUNCTION meclaw.context_bee_v2(p_msg_id UUID)
RETURNS VOID AS $function$
DECLARE
    v_msg RECORD;
    v_channel_id UUID;
    v_history JSONB;
    v_content JSONB;
    v_current_input TEXT;
    v_model_id TEXT;
    v_tier TEXT;
    v_max_messages INT;
    v_agent_id TEXT;
    v_memories JSONB;
    v_memory_count INT := 0;
    v_llm_config TEXT;
    v_rec RECORD;
BEGIN
    -- Get message
    SELECT * INTO v_msg FROM meclaw.messages WHERE id = p_msg_id;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'context_bee_v2: message % not found', p_msg_id;
    END IF;

    v_channel_id := v_msg.channel_id;
    IF v_channel_id IS NULL THEN
        SELECT channel_id INTO v_channel_id FROM meclaw.tasks WHERE id = v_msg.task_id;
    END IF;

    -- Identify which agent owns this channel
    -- Priority: specific agent subscriber > system agent
    SELECT ac.agent_id INTO v_agent_id
    FROM meclaw.agent_channels ac
    JOIN meclaw.entities e ON e.id = ac.agent_id
    WHERE ac.channel_id = v_channel_id
        AND e.entity_type = 'agent'  -- not system, not workspace
        AND ac.role IN ('owner', 'participant')
    ORDER BY ac.role ASC  -- owner first
    LIMIT 1;

    -- Fallback to any agent on this channel
    IF v_agent_id IS NULL THEN
        SELECT ac.agent_id INTO v_agent_id
        FROM meclaw.agent_channels ac
        WHERE ac.channel_id = v_channel_id
        LIMIT 1;
    END IF;

    -- Get model config from LLM bee (same as original context_bee)
    v_model_id := NULL;
    BEGIN
        LOAD 'age';
        SET LOCAL search_path = ag_catalog, meclaw, public;
        SELECT trim(both '"' from c::text) INTO v_llm_config
        FROM cypher('meclaw_graph', $$
            MATCH (b:Bee) WHERE b.type = 'llm_bee' RETURN b.config
        $$) AS (c agtype)
        LIMIT 1;
        IF v_llm_config IS NOT NULL THEN
            v_model_id := (v_llm_config::jsonb)->>'model_id';
        END IF;
    EXCEPTION WHEN OTHERS THEN
        v_model_id := NULL;
    END;

    -- Tier resolution
    IF v_model_id IS NOT NULL THEN
        SELECT tier INTO v_tier FROM meclaw.llm_models WHERE id = v_model_id;
    END IF;

    v_max_messages := CASE v_tier
        WHEN 'small'     THEN 5
        WHEN 'medium'    THEN 15
        WHEN 'large'     THEN 30
        WHEN 'reasoning' THEN 50
        ELSE 10
    END;

    -- Get conversation history
    v_current_input := v_msg.content->>'input';
    v_history := meclaw.get_conversation_history(v_channel_id, 24, v_max_messages);

    -- Remove duplicate current message from history
    IF jsonb_array_length(v_history) > 0
       AND v_history->-1->>'content' = v_current_input
       AND v_history->-1->>'role' = 'user' THEN
        v_history := v_history - (jsonb_array_length(v_history) - 1);
    END IF;

    -- Memory retrieval (if agent is identified and query is non-trivial)
    v_memories := '[]'::jsonb;
    IF v_agent_id IS NOT NULL AND v_current_input IS NOT NULL AND length(v_current_input) > 3 THEN
        BEGIN
            SELECT jsonb_agg(jsonb_build_object(
                'content', r.content,
                'score', round(r.score::numeric, 4),
                'age_hours', round((EXTRACT(EPOCH FROM (clock_timestamp() - r.created_at)) / 3600.0)::numeric, 1),
                'reward', r.reward
            ))
            INTO v_memories
            FROM meclaw.retrieve_bee(v_agent_id, v_current_input, 5) r;

            v_memory_count := COALESCE(jsonb_array_length(v_memories), 0);
        EXCEPTION WHEN OTHERS THEN
            -- retrieve_bee might fail if BM25 index doesn't exist yet or no data
            v_memories := '[]'::jsonb;
            v_memory_count := 0;
        END;
    END IF;

    -- Log
    PERFORM meclaw.log_event(p_msg_id, v_msg.task_id, 'context_bee_v2', 'context_loaded',
        jsonb_build_object(
            'channel_id', v_channel_id,
            'agent_id', v_agent_id,
            'history_count', jsonb_array_length(v_history),
            'memory_count', v_memory_count,
            'model_id', COALESCE(v_model_id, 'unknown'),
            'tier', COALESCE(v_tier, 'default'),
            'max_messages', v_max_messages
        ));

    -- Build content: original content + history + memories
    v_content := v_msg.content || jsonb_build_object(
        'conversation_history', v_history,
        'memories', COALESCE(v_memories, '[]'::jsonb),
        'agent_id', v_agent_id
    );

    -- Create routing message
    INSERT INTO meclaw.messages (task_id, channel_id, previous_id, type, sender, status, content)
    VALUES (v_msg.task_id, v_channel_id, p_msg_id, 'routing', 'context_bee_v2', 'done', v_content);

    UPDATE meclaw.messages SET status = 'done' WHERE id = p_msg_id AND status != 'done';
END;
$function$ LANGUAGE plpgsql;

COMMENT ON FUNCTION meclaw.context_bee_v2 IS
'Agent-level context bee with memory retrieval. Injects relevant memories from
retrieve_bee into the LLM context alongside conversation history.
Phase 1: BM25 retrieval. Phase 2: + compression + cache breakpoint + personality-aware.';

-- =============================================================================
-- NOTE: To switch from context_bee to context_bee_v2 in the graph:
-- =============================================================================
-- UPDATE the graph to point to context_bee_v2:
--
-- LOAD 'age';
-- SET search_path = ag_catalog, public;
-- SELECT * FROM cypher('meclaw_graph', $$
--     MATCH (b:Bee {id: 'test-context-bee'})
--     SET b.type = 'context_bee_v2'
--     RETURN b.id, b.type
-- $$) AS (id agtype, type agtype);
--
-- And update the dispatch trigger to recognize 'context_bee_v2':
-- In 08_triggers.sql, trg_on_message_ready_dispatch should handle both.

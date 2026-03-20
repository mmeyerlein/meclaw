-- MeClaw v0.1.0 — Extract Bee (Channel-Level)
-- Date: 2026-03-20
-- Ref: docs/BRAIN.md (Channel Architecture, The Six Bees)
--
-- Channel-level extraction: entities, events, and relations extracted ONCE per
-- channel, shared across all agents subscribed to that channel.
--
-- Phase 1: LLM-based extraction via pg_net → pg_background
-- The extract_bee is triggered after every user_input message in a channel.

-- =============================================================================
-- 1. Extract Bee: main function
-- =============================================================================

CREATE OR REPLACE FUNCTION meclaw.extract_bee(p_msg_id UUID)
RETURNS VOID AS $$
DECLARE
    v_channel_id UUID;
    v_content TEXT;
    v_message_type TEXT;
    v_task_id UUID;
BEGIN
    -- Get message details
    SELECT channel_id, content->>'input', type, task_id
    INTO v_channel_id, v_content, v_message_type, v_task_id
    FROM meclaw.messages
    WHERE id = p_msg_id;

    -- Only extract from user_input and llm_result messages
    IF v_message_type NOT IN ('user_input', 'llm_result') THEN
        RETURN;
    END IF;

    -- Skip if no content
    IF v_content IS NULL OR v_content = '' THEN
        -- Try output field for llm_result
        SELECT content->>'output' INTO v_content
        FROM meclaw.messages WHERE id = p_msg_id;

        IF v_content IS NULL OR v_content = '' THEN
            RETURN;
        END IF;
    END IF;

    -- Create brain_event (shared, channel-level: agent_id = NULL)
    INSERT INTO meclaw.brain_events (
        message_id, channel_id, agent_id, content
    ) VALUES (
        p_msg_id, v_channel_id, NULL, v_content
    );

    -- Update channel extraction tracking
    UPDATE meclaw.channels
    SET last_extracted_seq = COALESCE(
        (SELECT MAX(seq) FROM meclaw.brain_events WHERE channel_id = v_channel_id),
        0
    ),
    extraction_status = 'idle',
    updated_at = clock_timestamp()
    WHERE id = v_channel_id;

    -- Log the extraction event
    INSERT INTO meclaw.events (msg_id, task_id, bee_type, event, payload)
    VALUES (
        p_msg_id, v_task_id, 'extract_bee', 'extraction_complete',
        jsonb_build_object(
            'channel_id', v_channel_id,
            'content_length', length(v_content)
        )
    );

    -- NOTE: Phase 2 will add:
    -- 1. LLM-based entity extraction (via pg_net call to LLM)
    -- 2. Entity resolution against meclaw.entities
    -- 3. AGE graph node/edge creation for extracted entities
    -- 4. pgvector embedding computation (via pg_net call to embedding API)
    -- 5. Entity observation creation for person entities

END;
$$ LANGUAGE plpgsql;

-- =============================================================================
-- 2. Embedding helper: compute embedding via pg_net (async)
-- =============================================================================

-- NOTE: This is a placeholder for Phase 2.
-- In Phase 1, embeddings are NULL. The retrieve_bee uses BM25 only.
-- Phase 2 will add:
--   meclaw.compute_embedding(brain_event_id UUID) → fires pg_net to embedding API
--   meclaw.on_embedding_response(req_id BIGINT) → updates brain_events.embedding

-- =============================================================================
-- 3. Entity extraction helper (Phase 2 placeholder)
-- =============================================================================

-- NOTE: Phase 2 will add LLM-based entity extraction:
--   meclaw.extract_entities_llm(brain_event_id UUID, content TEXT)
--     → pg_net POST to LLM with extraction prompt
--     → Response handler creates/updates entities in meclaw.entities
--     → Creates AGE graph edges (:Entity)-[:INVOLVED_IN]->(:Event)

-- =============================================================================
-- 4. Integration: trigger extract_bee after message done
-- =============================================================================

-- The existing trg_message_done_dispatch already fires on status='done'.
-- We hook into the routing to call extract_bee for user_input and llm_result.
-- This is handled by adding an extract_bee node to the AGE graph and routing.
--
-- For Phase 1, we add a simple trigger that calls extract_bee directly:

CREATE OR REPLACE FUNCTION meclaw.trg_extract_on_done()
RETURNS TRIGGER AS $$
BEGIN
    -- Only trigger on status change to 'done'
    IF NEW.status = 'done' AND (OLD.status IS NULL OR OLD.status != 'done') THEN
        -- Only extract from user_input and llm_result
        IF NEW.type IN ('user_input', 'llm_result') THEN
            PERFORM meclaw.extract_bee(NEW.id);
        END IF;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Drop existing trigger if any, then create
DROP TRIGGER IF EXISTS trg_extract_on_done ON meclaw.messages;
CREATE TRIGGER trg_extract_on_done
    AFTER INSERT OR UPDATE OF status ON meclaw.messages
    FOR EACH ROW
    EXECUTE FUNCTION meclaw.trg_extract_on_done();

COMMENT ON FUNCTION meclaw.extract_bee IS
'Channel-level extraction bee. Creates brain_events from user_input and llm_result messages.
Phase 1: stores raw content. Phase 2: adds LLM entity extraction, embeddings, AGE graph.';

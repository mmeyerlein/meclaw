-- MeClaw v0.1.0 — Phase A1: Temporal Edges in the AGE Graph
-- Date: 2026-03-21
-- 
-- Extends the AGE graph with:
--   1. Event nodes with full properties (seq, channel_id, created_at)
--   2. TEMPORAL edges between consecutive events in the same channel
--   3. Backfill for existing events without TEMPORAL edges
--
-- Changes to extract_bee: after brain_event INSERT, the AGE event node
-- and TEMPORAL edge are created synchronously (within the same call).
-- =============================================================================

-- =============================================================================
-- 1. Create / update AGE event node with properties
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw.age_upsert_event_node(
    p_event_id   TEXT,
    p_seq        BIGINT,
    p_channel_id TEXT,
    p_created_at TEXT  -- ISO 8601 timestamp as string
) RETURNS VOID AS $fn$
    try:
        plpy.execute("SET search_path = ag_catalog, meclaw, public")
        # Use MERGE + SET to create or update the event node
        plpy.execute("""
            SELECT * FROM cypher('meclaw_graph', $$
                MERGE (e:Event {event_id: '%s'})
                SET e.seq = %d, e.channel_id = '%s', e.created_at = '%s'
            $$) AS (v agtype)
        """ % (
            p_event_id,
            int(p_seq),
            p_channel_id,
            p_created_at
        ))
    except Exception as e:
        plpy.warning(f"age_upsert_event_node: {e}")
$fn$ LANGUAGE plpython3u;

COMMENT ON FUNCTION meclaw.age_upsert_event_node IS
'Creates (or updates) an AGE event node with seq, channel_id, and created_at.';

-- =============================================================================
-- 2. Set TEMPORAL edge to the previous event in the same channel
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw.age_link_temporal(
    p_event_id   TEXT,   -- UUID of the current event
    p_channel_id TEXT,   -- UUID of the channel (as text)
    p_seq        BIGINT  -- seq of the current event
) RETURNS BOOLEAN AS $fn$
    # Find the directly preceding event in the same channel (by seq)
    plan = plpy.prepare("""
        SELECT id::text AS prev_id
        FROM meclaw.brain_events
        WHERE channel_id = $1::uuid AND seq < $2
        ORDER BY seq DESC
        LIMIT 1
    """, ["text", "int8"])

    result = plan.execute([p_channel_id, p_seq])
    if not result:
        return False  # No predecessor → no TEMPORAL edge

    prev_event_id = result[0]["prev_id"]

    try:
        plpy.execute("SET search_path = ag_catalog, meclaw, public")
        plpy.execute("""
            SELECT * FROM cypher('meclaw_graph', $$
                MATCH (prev:Event {event_id: '%s'})
                MATCH (curr:Event {event_id: '%s'})
                MERGE (prev)-[:TEMPORAL]->(curr)
            $$) AS (v agtype)
        """ % (prev_event_id, p_event_id))
        return True
    except Exception as e:
        plpy.warning(f"age_link_temporal ({prev_event_id} → {p_event_id}): {e}")
        return False
$fn$ LANGUAGE plpython3u;

COMMENT ON FUNCTION meclaw.age_link_temporal IS
'Creates a TEMPORAL edge from the previous event to the current event in the same channel (seq-based).';

-- =============================================================================
-- 3. extract_bee — extended with AGE event node + TEMPORAL edge
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw.extract_bee(p_msg_id UUID)
RETURNS VOID AS $$
DECLARE
    v_channel_id  UUID;
    v_content     TEXT;
    v_message_type TEXT;
    v_task_id     UUID;
    v_event_id    UUID;
    v_seq         BIGINT;
    v_created_at  TIMESTAMPTZ;
BEGIN
    -- Load message details
    SELECT channel_id, content->>'input', type, task_id
    INTO v_channel_id, v_content, v_message_type, v_task_id
    FROM meclaw.messages
    WHERE id = p_msg_id;

    -- Only process user_input and llm_result
    IF v_message_type NOT IN ('user_input', 'llm_result') THEN
        RETURN;
    END IF;

    -- Get content from output field if input is empty
    IF v_content IS NULL OR v_content = '' THEN
        SELECT content->>'output' INTO v_content
        FROM meclaw.messages WHERE id = p_msg_id;

        IF v_content IS NULL OR v_content = '' THEN
            RETURN;
        END IF;
    END IF;

    -- Stage 1: create brain_event with timestamp from the message
    INSERT INTO meclaw.brain_events (
        message_id, channel_id, agent_id, content, extracted, created_at
    ) VALUES (
        p_msg_id, v_channel_id, NULL, v_content, FALSE,
        COALESCE((SELECT m.created_at FROM meclaw.messages m WHERE m.id = p_msg_id), NOW())
    ) RETURNING id, seq, created_at INTO v_event_id, v_seq, v_created_at;

    -- Stage 1b: create AGE event node with full properties
    BEGIN
        PERFORM meclaw.age_upsert_event_node(
            v_event_id::text,
            v_seq,
            v_channel_id::text,
            to_char(v_created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"')
        );
    EXCEPTION WHEN OTHERS THEN
        -- AGE errors do not block the message flow
        INSERT INTO meclaw.events (msg_id, task_id, bee_type, event, payload)
        VALUES (p_msg_id, v_task_id, 'extract_bee', 'age_event_node_failed',
            jsonb_build_object('error', SQLERRM, 'event_id', v_event_id));
    END;

    -- Stage 1c: TEMPORAL edge to the previous event in the same channel
    BEGIN
        PERFORM meclaw.age_link_temporal(
            v_event_id::text,
            v_channel_id::text,
            v_seq
        );
    EXCEPTION WHEN OTHERS THEN
        -- AGE errors do not block the message flow
        INSERT INTO meclaw.events (msg_id, task_id, bee_type, event, payload)
        VALUES (p_msg_id, v_task_id, 'extract_bee', 'age_temporal_edge_failed',
            jsonb_build_object('error', SQLERRM, 'event_id', v_event_id));
    END;

    -- Stage 1d: compute embedding asynchronously
    BEGIN
        PERFORM pg_background_launch(
            format('SELECT meclaw.compute_embedding(%L::uuid)', v_event_id)
        );
    EXCEPTION WHEN OTHERS THEN
        INSERT INTO meclaw.events (msg_id, task_id, bee_type, event, payload)
        VALUES (p_msg_id, v_task_id, 'extract_bee', 'embedding_bg_failed',
            jsonb_build_object('error', SQLERRM, 'event_id', v_event_id));
    END;

    -- Stage 2: LLM entity extraction asynchronously
    BEGIN
        PERFORM pg_background_launch(
            format('SELECT meclaw.llm_extract_entities(%L::uuid)', v_event_id)
        );
    EXCEPTION WHEN OTHERS THEN
        INSERT INTO meclaw.events (msg_id, task_id, bee_type, event, payload)
        VALUES (p_msg_id, v_task_id, 'extract_bee', 'llm_extraction_bg_failed',
            jsonb_build_object('error', SQLERRM, 'event_id', v_event_id));
    END;

    -- Update channel tracking
    UPDATE meclaw.channels
    SET last_extracted_seq = COALESCE(
        (SELECT MAX(seq) FROM meclaw.brain_events WHERE channel_id = v_channel_id), 0
    ),
    extraction_status = 'idle',
    updated_at = clock_timestamp()
    WHERE id = v_channel_id;

    -- Log extraction
    INSERT INTO meclaw.events (msg_id, task_id, bee_type, event, payload)
    VALUES (p_msg_id, v_task_id, 'extract_bee', 'extraction_queued',
        jsonb_build_object(
            'channel_id', v_channel_id,
            'content_length', length(v_content),
            'event_id', v_event_id,
            'seq', v_seq,
            'stages', ARRAY['age_event_node', 'temporal_edge', 'embedding', 'llm_extraction']
        ));
END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION meclaw.extract_bee IS
'Two-stage extraction: (1) brain_event + AGE event node + TEMPORAL edge, (2) embedding + entity extraction async.';

-- =============================================================================
-- 4. Backfill: populate existing AGE event nodes with properties
--    and create TEMPORAL edges for all events without a predecessor edge
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw.backfill_temporal_edges()
RETURNS TABLE(
    events_updated   INT,
    temporal_created INT,
    errors           INT
) AS $fn$
import time

plpy.execute("SET search_path = ag_catalog, meclaw, public")

# Load all brain_events (ordered by seq)
plan = plpy.prepare("""
    SELECT
        id::text AS event_id,
        seq,
        channel_id::text AS channel_id,
        to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS created_at_str
    FROM meclaw.brain_events
    ORDER BY seq ASC
""", [])
events = plan.execute()

updated = 0
created = 0
err = 0

for row in events:
    event_id    = row["event_id"]
    seq         = row["seq"]
    channel_id  = row["channel_id"]
    created_at  = row["created_at_str"]

    # Update AGE event node (set properties)
    try:
        plpy.execute("""
            SELECT meclaw.age_upsert_event_node('%s', %d, '%s', '%s')
        """ % (event_id, int(seq), channel_id, created_at))
        updated += 1
    except Exception as e:
        plpy.warning(f"backfill_temporal_edges: upsert failed for {event_id}: {e}")
        err += 1
        continue

    # Set TEMPORAL edge to predecessor
    try:
        result = plpy.execute("""
            SELECT meclaw.age_link_temporal('%s', '%s', %d)
        """ % (event_id, channel_id, int(seq)))
        if result and result[0]["age_link_temporal"]:
            created += 1
    except Exception as e:
        plpy.warning(f"backfill_temporal_edges: temporal link failed for {event_id}: {e}")
        err += 1

return [(updated, created, err)]
$fn$ LANGUAGE plpython3u;

COMMENT ON FUNCTION meclaw.backfill_temporal_edges IS
'Backfill: sets seq/channel_id/created_at on existing AGE event nodes and creates TEMPORAL edges.';

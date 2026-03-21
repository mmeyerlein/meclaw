-- =============================================================================
-- E8: MemCell Nodes (Boundary-Detected Conversation Chunks)
-- =============================================================================
-- BRAIN.md: (:MemCell) as boundary-detected conversation chunks in AGE graph.
-- A MemCell groups consecutive brain_events that belong to the same
-- conversational topic/context. Boundaries detected by embedding drift.
-- =============================================================================

-- =============================================================================
-- 1. detect_boundaries: Find topic boundaries via embedding distance
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw.detect_conversation_boundaries(
    p_channel_id UUID,
    p_threshold FLOAT DEFAULT 0.35  -- cosine distance threshold for boundary
)
RETURNS TABLE (
    event_id UUID,
    seq BIGINT,
    distance_to_prev FLOAT,
    is_boundary BOOLEAN
)
LANGUAGE plpgsql AS $$
BEGIN
    RETURN QUERY
    WITH ordered_events AS (
        SELECT
            be.id,
            be.seq,
            be.embedding,
            LAG(be.embedding) OVER (ORDER BY be.seq) AS prev_embedding
        FROM meclaw.brain_events be
        WHERE be.channel_id = p_channel_id
          AND be.embedding IS NOT NULL
        ORDER BY be.seq
    )
    SELECT
        oe.id AS event_id,
        oe.seq,
        CASE
            WHEN oe.prev_embedding IS NULL THEN 1.0  -- first event is always a boundary
            ELSE (oe.embedding <=> oe.prev_embedding)::FLOAT
        END AS distance_to_prev,
        CASE
            WHEN oe.prev_embedding IS NULL THEN TRUE
            ELSE (oe.embedding <=> oe.prev_embedding) >= p_threshold
        END AS is_boundary
    FROM ordered_events oe
    ORDER BY oe.seq;
END;
$$;

COMMENT ON FUNCTION meclaw.detect_conversation_boundaries IS
'Detects topic boundaries in a channel by measuring cosine distance between consecutive event embeddings.';

-- =============================================================================
-- 2. build_memcells: Group events into MemCells based on boundaries
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw.build_memcells(
    p_channel_id UUID,
    p_threshold FLOAT DEFAULT 0.35
)
RETURNS INT
LANGUAGE plpgsql AS $$
DECLARE
    v_count INT := 0;
    v_cell_id TEXT;
    v_cell_num INT := 0;
    v_prev_boundary BOOLEAN := TRUE;
    v_row RECORD;
    v_event_ids UUID[] := '{}';
    v_first_seq BIGINT;
    v_last_seq BIGINT;
BEGIN
    FOR v_row IN
        SELECT * FROM meclaw.detect_conversation_boundaries(p_channel_id, p_threshold)
    LOOP
        IF v_row.is_boundary AND array_length(v_event_ids, 1) > 0 THEN
            -- Flush current MemCell
            v_cell_id := format('memcell:%s:%s', p_channel_id, v_cell_num);

            -- Create MemCell node in AGE
            BEGIN
                EXECUTE format(
                    'LOAD ''age''; SET search_path = ag_catalog, meclaw, public; '
                    'SELECT * FROM cypher(''meclaw_graph'', $$ '
                    'MERGE (m:MemCell {id: ''%s''}) '
                    'SET m.channel_id = ''%s'', m.first_seq = %s, m.last_seq = %s, m.event_count = %s '
                    'RETURN m '
                    '$$) AS (v agtype)',
                    replace(v_cell_id, '''', ''''''),
                    replace(p_channel_id::text, '''', ''''''),
                    v_first_seq,
                    v_last_seq,
                    array_length(v_event_ids, 1)
                );

                -- Link events to MemCell
                DECLARE
                    v_eid UUID;
                BEGIN
                    FOREACH v_eid IN ARRAY v_event_ids LOOP
                        BEGIN
                            EXECUTE format(
                                'SELECT * FROM cypher(''meclaw_graph'', $$ '
                                'MATCH (m:MemCell {id: ''%s''}), (e:Event {id: ''%s''}) '
                                'MERGE (e)-[:BELONGS_TO]->(m) '
                                'RETURN m '
                                '$$) AS (v agtype)',
                                replace(v_cell_id, '''', ''''''),
                                replace(v_eid::text, '''', '''''')
                            );
                        EXCEPTION WHEN OTHERS THEN NULL;
                        END;
                    END LOOP;
                END;

                v_count := v_count + 1;
            EXCEPTION WHEN OTHERS THEN
                -- AGE errors shouldn't break the function
                NULL;
            END;

            -- Start new cell
            v_cell_num := v_cell_num + 1;
            v_event_ids := ARRAY[v_row.event_id];
            v_first_seq := v_row.seq;
            v_last_seq := v_row.seq;
        ELSE
            -- Add to current cell
            IF array_length(v_event_ids, 1) IS NULL THEN
                v_event_ids := ARRAY[v_row.event_id];
                v_first_seq := v_row.seq;
            ELSE
                v_event_ids := array_append(v_event_ids, v_row.event_id);
            END IF;
            v_last_seq := v_row.seq;
        END IF;
    END LOOP;

    -- Flush last cell
    IF array_length(v_event_ids, 1) > 0 THEN
        v_cell_id := format('memcell:%s:%s', p_channel_id, v_cell_num);
        BEGIN
            EXECUTE format(
                'LOAD ''age''; SET search_path = ag_catalog, meclaw, public; '
                'SELECT * FROM cypher(''meclaw_graph'', $$ '
                'MERGE (m:MemCell {id: ''%s''}) '
                'SET m.channel_id = ''%s'', m.first_seq = %s, m.last_seq = %s, m.event_count = %s '
                'RETURN m '
                '$$) AS (v agtype)',
                replace(v_cell_id, '''', ''''''),
                replace(p_channel_id::text, '''', ''''''),
                v_first_seq,
                v_last_seq,
                array_length(v_event_ids, 1)
            );
            v_count := v_count + 1;
        EXCEPTION WHEN OTHERS THEN NULL;
        END;
    END IF;

    RETURN v_count;
END;
$$;

COMMENT ON FUNCTION meclaw.build_memcells IS
'Groups consecutive events into MemCell nodes based on embedding distance boundaries. Creates BELONGS_TO edges in AGE.';

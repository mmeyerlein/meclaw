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
LANGUAGE plpython3u AS $func$
    import json

    # Get boundaries
    rows = plpy.execute("""
        SELECT event_id, seq, distance_to_prev, is_boundary
        FROM meclaw.detect_conversation_boundaries('%s', %s)
    """ % (p_channel_id, p_threshold))

    count = 0
    cell_num = 0
    event_ids = []
    first_seq = None
    last_seq = None

    def flush_cell(cell_num, event_ids, first_seq, last_seq, channel_id):
        if not event_ids:
            return 0
        cell_id = "memcell:%s:%d" % (channel_id, cell_num)
        cell_id_safe = cell_id.replace("'", "\\'")
        ch_safe = str(channel_id).replace("'", "\\'")

        try:
            plpy.execute("LOAD 'age'")
            plpy.execute("SET search_path = ag_catalog, meclaw, public")
            cypher = """
                MERGE (m:MemCell {id: '%s'})
                SET m.channel_id = '%s', m.first_seq = %d, m.last_seq = %d, m.event_count = %d
                RETURN m
            """ % (cell_id_safe, ch_safe, first_seq, last_seq, len(event_ids))
            plpy.execute("SELECT * FROM cypher('meclaw_graph', $$ %s $$) AS (v agtype)" % cypher)

            for eid in event_ids:
                eid_safe = str(eid).replace("'", "\\'")
                try:
                    cypher2 = """
                        MATCH (m:MemCell {id: '%s'}), (e:Event {id: '%s'})
                        MERGE (e)-[:BELONGS_TO]->(m)
                        RETURN m
                    """ % (cell_id_safe, eid_safe)
                    plpy.execute("SELECT * FROM cypher('meclaw_graph', $$ %s $$) AS (v agtype)" % cypher2)
                except:
                    pass
            return 1
        except Exception as e:
            plpy.warning("build_memcells error: %s" % str(e))
            return 0

    for row in rows:
        if row["is_boundary"] and event_ids:
            count += flush_cell(cell_num, event_ids, first_seq, last_seq, p_channel_id)
            cell_num += 1
            event_ids = [row["event_id"]]
            first_seq = row["seq"]
            last_seq = row["seq"]
        else:
            if not event_ids:
                event_ids = [row["event_id"]]
                first_seq = row["seq"]
            else:
                event_ids.append(row["event_id"])
            last_seq = row["seq"]

    # Flush last cell
    count += flush_cell(cell_num, event_ids, first_seq, last_seq, p_channel_id)

    return count
$func$;

COMMENT ON FUNCTION meclaw.build_memcells IS
'Groups consecutive events into MemCell nodes based on embedding distance boundaries. Creates BELONGS_TO edges in AGE.';

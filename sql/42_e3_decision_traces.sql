-- =============================================================================
-- E3: Decision Traces + Citation Tracking
-- =============================================================================
-- BRAIN.md: Every LLM decision gets a trace with evidence_ids, prototypes,
-- q_value_estimates. Citations track which events informed which decisions.
-- =============================================================================

-- =============================================================================
-- 1. log_decision_trace: Called after retrieve_bee to record what was used
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw.log_decision_trace(
    p_agent_id TEXT,
    p_query TEXT,
    p_evidence_ids UUID[],
    p_prototypes_activated TEXT[] DEFAULT NULL,
    p_q_value_estimates JSONB DEFAULT NULL,
    p_action_taken TEXT DEFAULT NULL
)
RETURNS UUID
LANGUAGE plpgsql AS $$
DECLARE
    v_trace_id UUID;
BEGIN
    INSERT INTO meclaw.decision_traces (
        agent_id, query, evidence_ids, prototypes_activated,
        q_value_estimates, action_taken
    ) VALUES (
        p_agent_id, p_query, p_evidence_ids, p_prototypes_activated,
        p_q_value_estimates, p_action_taken
    )
    RETURNING id INTO v_trace_id;

    RETURN v_trace_id;
END;
$$;

COMMENT ON FUNCTION meclaw.log_decision_trace IS
'Creates a decision trace entry recording which evidence (brain_events) and prototypes influenced a retrieval/decision.';

-- =============================================================================
-- 2. cite_events: Link a decision to its evidence events
--    Creates CITATION edges in AGE graph: (Decision)-[:CITES]->(Event)
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw.cite_events(
    p_trace_id UUID,
    p_evidence_ids UUID[]
)
RETURNS INT
LANGUAGE plpython3u AS $func$
    import json

    plan = plpy.prepare("SELECT COALESCE(MAX(seq), 0) as maxseq FROM meclaw.brain_events")
    row = plpy.execute(plan)
    v_seq = row[0]["maxseq"]

    count = 0
    for eid in p_evidence_ids:
        try:
            trace_id_safe = str(p_trace_id).replace("'", "\\'")
            eid_safe = str(eid).replace("'", "\\'")
            ts = plpy.execute("SELECT to_char(clock_timestamp() AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') as ts")[0]["ts"]

            cypher = """
                MERGE (d:Decision {id: '%s'})
                MERGE (e:Event {id: '%s'})
                MERGE (d)-[c:CITES]->(e)
                SET c.authority = %d, c.at = '%s'
                RETURN d
            """ % (trace_id_safe, eid_safe, v_seq, ts)

            plpy.execute("LOAD 'age'")
            plpy.execute("SET search_path = ag_catalog, meclaw, public")
            plpy.execute("SELECT * FROM cypher('meclaw_graph', $$ %s $$) AS (v agtype)" % cypher)
            count += 1
        except Exception as e:
            plpy.warning("cite_events error for %s: %s" % (eid, str(e)))

    return count
$func$;

COMMENT ON FUNCTION meclaw.cite_events IS
'Creates CITES edges in AGE graph linking a Decision to its evidence Events.';

-- =============================================================================
-- 3. Integrate into retrieve_bee: log trace after retrieval
--    We add a wrapper function that calls retrieve_bee + logs the trace
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw.retrieve_and_trace(
    p_agent_id TEXT,
    p_query TEXT,
    p_limit INT DEFAULT 5,
    p_ctm_enabled BOOLEAN DEFAULT FALSE
)
RETURNS TABLE (
    event_id UUID,
    content TEXT,
    score FLOAT,
    source TEXT
)
LANGUAGE plpgsql AS $$
DECLARE
    v_evidence_ids UUID[] := '{}';
    v_trace_id UUID;
    v_row RECORD;
    v_prototypes TEXT[];
BEGIN
    -- Collect results from retrieve_bee
    FOR v_row IN
        SELECT r.event_id, r.content, r.score, r.source
        FROM meclaw.retrieve_bee(p_agent_id, p_query, p_limit, p_ctm_enabled) r
    LOOP
        event_id := v_row.event_id;
        content := v_row.content;
        score := v_row.score;
        source := v_row.source;
        v_evidence_ids := array_append(v_evidence_ids, v_row.event_id);
        RETURN NEXT;
    END LOOP;

    -- Log decision trace
    IF array_length(v_evidence_ids, 1) > 0 THEN
        -- Get activated prototypes (top-3 by activation for the query)
        BEGIN
            SELECT array_agg(p.id ORDER BY pa.weight DESC)
            INTO v_prototypes
            FROM meclaw.prototype_associations pa
            JOIN meclaw.prototypes p ON p.id = pa.prototype_a
            WHERE pa.prototype_b IN (
                SELECT pp.id FROM meclaw.prototypes pp
                WHERE pp.agent_id = p_agent_id
                ORDER BY pp.activation_count DESC
                LIMIT 3
            )
            LIMIT 5;
        EXCEPTION WHEN OTHERS THEN
            v_prototypes := NULL;
        END;

        v_trace_id := meclaw.log_decision_trace(
            p_agent_id,
            p_query,
            v_evidence_ids,
            v_prototypes,
            jsonb_build_object('result_count', array_length(v_evidence_ids, 1)),
            'retrieve'
        );

        -- Create CITES edges in AGE
        PERFORM meclaw.cite_events(v_trace_id, v_evidence_ids);
    END IF;
END;
$$;

COMMENT ON FUNCTION meclaw.retrieve_and_trace IS
'Wrapper around retrieve_bee that also logs a decision trace and creates CITES edges in AGE.';

-- =============================================================================
-- 4. Citation authority helpers
-- =============================================================================

-- Trending precedents: decisions cited more recently than historically
CREATE OR REPLACE VIEW meclaw.trending_precedents AS
SELECT
    dt.id AS decision_id,
    dt.agent_id,
    dt.query,
    dt.created_at,
    COUNT(*) FILTER (WHERE be.seq > dt.seq - 50) AS recent_citations,
    COUNT(*) FILTER (WHERE be.seq <= dt.seq - 50) AS old_citations
FROM meclaw.decision_traces dt
JOIN unnest(dt.evidence_ids) AS eid ON TRUE
JOIN meclaw.brain_events be ON be.id = eid
GROUP BY dt.id, dt.agent_id, dt.query, dt.created_at, dt.seq
HAVING COUNT(*) FILTER (WHERE be.seq > dt.seq - 50) > COUNT(*) FILTER (WHERE be.seq <= dt.seq - 50) * 2;

-- Stale precedents: decisions whose evidence hasn't been cited recently
CREATE OR REPLACE VIEW meclaw.stale_precedents AS
SELECT
    dt.id AS decision_id,
    dt.agent_id,
    dt.query,
    dt.created_at,
    MAX(be.seq) AS last_evidence_seq,
    (SELECT MAX(seq) FROM meclaw.brain_events) AS current_max_seq,
    (SELECT MAX(seq) FROM meclaw.brain_events) - MAX(be.seq) AS staleness
FROM meclaw.decision_traces dt
JOIN unnest(dt.evidence_ids) AS eid ON TRUE
JOIN meclaw.brain_events be ON be.id = eid
GROUP BY dt.id, dt.agent_id, dt.query, dt.created_at
HAVING (SELECT MAX(seq) FROM meclaw.brain_events) - MAX(be.seq) > 500;

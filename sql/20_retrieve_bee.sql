-- MeClaw v0.1.0 — Retrieve Bee (Agent-Level)
-- Date: 2026-03-20
-- Ref: docs/BRAIN.md (Retrieval: CTM-Style Iterative Graph Traversal)
--
-- Agent-level retrieval: searches brain_events using BM25 (pg_search) +
-- pgvector similarity + Reciprocal Rank Fusion (RRF).
-- Respects scoping: only retrieves from channels the agent subscribes to.
--
-- Phase 1: BM25 + RRF (no embeddings yet, no personality_fit, no graph expansion)

-- =============================================================================
-- 1. BM25 Index on brain_events
-- =============================================================================

-- pg_search 0.15.10 BM25 index for full-text search
-- Uses the @@@ operator for querying
CREATE INDEX IF NOT EXISTS idx_brain_events_bm25
ON meclaw.brain_events
USING bm25 (id, content)
WITH (key_field='id', text_fields='{"content":{}}');

-- =============================================================================
-- 2. Retrieve Bee: main function
-- =============================================================================

CREATE OR REPLACE FUNCTION meclaw.retrieve_bee(
    p_agent_id TEXT,
    p_query TEXT,
    p_limit INT DEFAULT 5
)
RETURNS TABLE (
    event_id UUID,
    content TEXT,
    score FLOAT,
    source TEXT,           -- 'bm25', 'vector', 'rrf'
    channel_id UUID,
    reward FLOAT,
    created_at TIMESTAMPTZ
) AS $$
DECLARE
    v_channel_ids UUID[];
BEGIN
    -- 1. Get channels this agent can access (scoping)
    SELECT array_agg(ac.channel_id)
    INTO v_channel_ids
    FROM meclaw.agent_channels ac
    WHERE ac.agent_id = p_agent_id;

    IF v_channel_ids IS NULL OR array_length(v_channel_ids, 1) = 0 THEN
        RETURN;  -- Agent has no channels, no results
    END IF;

    -- 2. BM25 search (pg_search)
    -- Phase 1: BM25 only (no pgvector yet, embeddings are NULL)
    -- RRF fusion will be added in Phase 2 when embeddings are available
    RETURN QUERY
    WITH bm25_results AS (
        SELECT
            be.id AS event_id,
            be.content,
            paradedb.score(be.id) AS bm25_score,
            be.channel_id,
            COALESCE(be.reward, 0.0) AS reward,
            be.created_at,
            ROW_NUMBER() OVER (ORDER BY paradedb.score(be.id) DESC) AS bm25_rank
        FROM meclaw.brain_events be
        WHERE be.content @@@ p_query
            AND be.channel_id = ANY(v_channel_ids)
            -- Include shared events (agent_id IS NULL) and agent's own events
            AND (be.agent_id IS NULL OR be.agent_id = p_agent_id)
        ORDER BY paradedb.score(be.id) DESC
        LIMIT 20
    ),
    -- RRF scoring (Phase 1: BM25 only, Phase 2: + vector rank)
    rrf_scored AS (
        SELECT
            b.event_id,
            b.content,
            -- RRF formula: 1/(k + rank), k=60 is standard
            (1.0 / (60 + b.bm25_rank))::FLOAT AS rrf_score,
            'bm25'::TEXT AS source,
            b.channel_id,
            b.reward,
            b.created_at
        FROM bm25_results b
    )
    SELECT
        r.event_id,
        r.content,
        -- Final score: RRF + reward bonus (Phase 1 simplified)
        (r.rrf_score + r.reward * 0.01)::FLOAT AS score,
        r.source,
        r.channel_id,
        r.reward,
        r.created_at
    FROM rrf_scored r
    ORDER BY (r.rrf_score + r.reward * 0.01) DESC
    LIMIT p_limit;

END;
$$ LANGUAGE plpgsql;

-- =============================================================================
-- 3. Full RRF with vector search (Phase 2 placeholder)
-- =============================================================================

-- Phase 2 will add:
-- CREATE OR REPLACE FUNCTION meclaw.retrieve_bee_rrf(
--     p_agent_id TEXT,
--     p_query TEXT,
--     p_query_embedding vector(1536),
--     p_limit INT DEFAULT 5
-- ) RETURNS TABLE (...)
--
-- This will:
-- 1. BM25 search → rank
-- 2. pgvector cosine similarity → rank
-- 3. RRF fusion: 1/(60+bm25_rank) + 1/(60+vector_rank)
-- 4. Add reward * 0.25 + novelty * 0.15 + recency * 0.10 + personality_fit * 0.15
-- 5. Add graph_distance * 0.10 (AGE Cypher expansion)

-- =============================================================================
-- 4. Convenience: search with formatted output
-- =============================================================================

CREATE OR REPLACE FUNCTION meclaw.memory_search(
    p_agent_id TEXT,
    p_query TEXT,
    p_limit INT DEFAULT 5
)
RETURNS TABLE (
    content TEXT,
    score FLOAT,
    age_hours FLOAT,
    reward FLOAT
) AS $$
    SELECT
        r.content,
        r.score,
        EXTRACT(EPOCH FROM (clock_timestamp() - r.created_at)) / 3600.0 AS age_hours,
        r.reward
    FROM meclaw.retrieve_bee(p_agent_id, p_query, p_limit) r;
$$ LANGUAGE sql;

COMMENT ON FUNCTION meclaw.retrieve_bee IS
'Agent-level memory retrieval. Phase 1: BM25 search with channel scoping.
Phase 2: + pgvector RRF fusion, personality_fit, graph expansion.';

COMMENT ON FUNCTION meclaw.memory_search IS
'Convenience wrapper for retrieve_bee with human-readable output.';

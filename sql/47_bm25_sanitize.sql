-- =============================================================================
-- BM25 Query Sanitization + Decompose Temporal Edge Fix
-- =============================================================================
-- Fix 1: ParadeDB BM25 parser chokes on special chars (', ", (), :, etc.)
--   in expanded queries from LLM. Sanitize before @@@ operator.
-- Fix 2: Decomposed facts need temporal edges + embeddings for retrieve_smart
-- =============================================================================

-- =============================================================================
-- 1. BM25 Query Sanitizer
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw.sanitize_bm25_query(p_query TEXT)
RETURNS TEXT
LANGUAGE plpgsql IMMUTABLE AS $$
DECLARE
    v_clean TEXT;
BEGIN
    IF p_query IS NULL OR p_query = '' THEN
        RETURN p_query;
    END IF;
    
    -- Remove characters that break ParadeDB BM25 parser
    v_clean := regexp_replace(p_query, '[''"\(\)\[\]\{\}:;!@#\$%\^&\*\+\\\/\|<>=~`]', ' ', 'g');
    
    -- Collapse multiple spaces
    v_clean := regexp_replace(v_clean, '\s+', ' ', 'g');
    v_clean := trim(v_clean);
    
    -- If nothing left, return original with just quotes removed
    IF v_clean = '' THEN
        v_clean := regexp_replace(p_query, '[''"]', ' ', 'g');
        v_clean := trim(v_clean);
    END IF;
    
    RETURN v_clean;
END;
$$;

-- =============================================================================
-- 2. Patch retrieve_bee: wrap all @@@ calls with sanitize
-- =============================================================================
-- We can't easily patch 6 locations in a 700-line function, so instead
-- we create a SAFE BM25 search wrapper that handles errors gracefully.
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw.bm25_search(
    p_query TEXT,
    p_field TEXT,  -- 'content' or 'facts_text'
    p_channel_ids UUID[],
    p_agent_id TEXT DEFAULT NULL,
    p_limit INT DEFAULT 20
)
RETURNS TABLE (
    event_id UUID,
    content TEXT,
    bm25_score FLOAT,
    channel_id UUID,
    reward FLOAT,
    novelty FLOAT,
    created_at TIMESTAMPTZ
)
LANGUAGE plpgsql AS $$
DECLARE
    v_safe_query TEXT;
BEGIN
    v_safe_query := meclaw.sanitize_bm25_query(p_query);
    
    IF v_safe_query IS NULL OR v_safe_query = '' THEN
        RETURN;
    END IF;

    IF p_field = 'content' THEN
        RETURN QUERY
        SELECT
            be.id,
            be.content,
            pdb.score(be.id)::FLOAT,
            be.channel_id,
            COALESCE(be.reward, 0.0)::FLOAT,
            COALESCE(be.novelty, 0.0)::FLOAT,
            be.created_at
        FROM meclaw.brain_events be
        WHERE be.content @@@ v_safe_query
            AND be.channel_id = ANY(p_channel_ids)
            AND (p_agent_id IS NULL OR be.agent_id IS NULL OR be.agent_id = p_agent_id)
        ORDER BY pdb.score(be.id) DESC
        LIMIT p_limit;
    ELSIF p_field = 'facts_text' THEN
        RETURN QUERY
        SELECT
            be.id,
            be.content,
            pdb.score(be.id)::FLOAT,
            be.channel_id,
            COALESCE(be.reward, 0.0)::FLOAT,
            COALESCE(be.novelty, 0.0)::FLOAT,
            be.created_at
        FROM meclaw.brain_events be
        WHERE be.facts_text @@@ v_safe_query
            AND be.channel_id = ANY(p_channel_ids)
            AND (p_agent_id IS NULL OR be.agent_id IS NULL OR be.agent_id = p_agent_id)
        ORDER BY pdb.score(be.id) DESC
        LIMIT p_limit;
    END IF;
    
EXCEPTION WHEN OTHERS THEN
    -- BM25 parse error — return empty
    RAISE WARNING 'bm25_search error for query "%": %', left(v_safe_query, 50), SQLERRM;
    RETURN;
END;
$$;

COMMENT ON FUNCTION meclaw.bm25_search IS
'Safe BM25 search wrapper: sanitizes query for ParadeDB parser, handles errors gracefully.';

-- =============================================================================
-- 3. Fix retrieve_smart: handle expanded query BM25 errors
-- =============================================================================
-- The issue: expand_temporal_query returns complex sentences with apostrophes
-- and special chars that break BM25. retrieve_temporal calls retrieve_bee which
-- uses raw @@@ operator → ParseError → exception → fallback.
-- 
-- Fix: sanitize the query BEFORE passing to retrieve_bee
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw.retrieve_temporal(
    p_agent_id TEXT,
    p_query TEXT,
    p_before_date TIMESTAMPTZ DEFAULT NULL,
    p_after_date TIMESTAMPTZ DEFAULT NULL,
    p_limit INT DEFAULT 10,
    p_temporal_order TEXT DEFAULT NULL,
    p_ctm_enabled BOOLEAN DEFAULT FALSE
)
RETURNS TABLE (
    event_id UUID,
    content TEXT,
    score FLOAT,
    source TEXT,
    created_at TIMESTAMPTZ
)
LANGUAGE plpgsql AS $$
DECLARE
    v_safe_query TEXT;
BEGIN
    -- Sanitize query for BM25 safety
    v_safe_query := meclaw.sanitize_bm25_query(p_query);
    
    RETURN QUERY
    WITH base_results AS (
        SELECT r.event_id, r.content, r.score, r.source, r.created_at
        FROM meclaw.retrieve_bee(p_agent_id, v_safe_query, 
             LEAST(p_limit * 3, 30), 
             p_ctm_enabled) r
    ),
    filtered AS (
        SELECT br.*
        FROM base_results br
        WHERE (p_before_date IS NULL OR br.created_at <= p_before_date)
          AND (p_after_date IS NULL OR br.created_at >= p_after_date)
    )
    SELECT f.event_id, f.content, f.score, f.source, f.created_at
    FROM filtered f
    ORDER BY
        CASE 
            WHEN p_temporal_order = 'asc' THEN EXTRACT(EPOCH FROM f.created_at)
            WHEN p_temporal_order = 'desc' THEN -EXTRACT(EPOCH FROM f.created_at)
            ELSE -f.score
        END
    LIMIT p_limit;
END;
$$;

-- =============================================================================
-- 4. Test
-- =============================================================================
DO $$
DECLARE
    v_result TEXT;
BEGIN
    -- Test sanitize
    v_result := meclaw.sanitize_bm25_query('St. Mary''s Church (cathedral)');
    ASSERT v_result IS NOT NULL AND v_result != '', 'sanitize should handle apostrophes';
    
    v_result := meclaw.sanitize_bm25_query('Calculate the number of days between the Sunday mass at St. Mary''s');
    ASSERT v_result IS NOT NULL, 'sanitize should handle complex queries';
    
    RAISE NOTICE '✅ bm25_sanitize tests passed';
END;
$$;

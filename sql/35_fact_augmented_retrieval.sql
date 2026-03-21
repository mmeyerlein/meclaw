-- MeClaw — Phase C1: Fact-Augmented Key Expansion
-- Date: 2026-03-21
-- Ref: docs/BRAIN.md (Retrieval)
--
-- Adds facts_text column to brain_events that materializes extraction_data
-- (entities + relations) into a searchable text field.
-- Extends the BM25 index to cover facts_text.
-- Updates retrieve_bee to search both content and facts_text.
-- Updates llm_extract_entities to populate facts_text after extraction.

-- =============================================================================
-- 1. Add facts_text column to brain_events
-- =============================================================================

ALTER TABLE meclaw.brain_events
    ADD COLUMN IF NOT EXISTS facts_text TEXT;

-- =============================================================================
-- 2. Helper: build facts_text from entity_events + entities for a given event
-- =============================================================================

CREATE OR REPLACE FUNCTION meclaw.build_facts_text(p_event_id UUID)
RETURNS TEXT AS $$
DECLARE
    v_facts TEXT := '';
    v_entity_parts TEXT[] := '{}';
    v_relation_parts TEXT[] := '{}';
BEGIN
    -- Collect entity mentions (name + type)
    SELECT array_agg(fact ORDER BY fact)
    INTO v_entity_parts
    FROM (
        SELECT DISTINCT 'Entity: ' || e.canonical_name || ', Type: ' || e.entity_type AS fact
        FROM meclaw.entity_events ee
        JOIN meclaw.entities e ON e.id = ee.entity_id
        WHERE ee.event_id = p_event_id
          AND ee.relation_type = 'MENTIONED_IN'
    ) sub;

    -- Collect typed relations between entities
    SELECT array_agg(fact ORDER BY fact)
    INTO v_relation_parts
    FROM (
        SELECT DISTINCT
            'Relation: ' || e1.canonical_name || ' ' || ee1.relation_type || ' ' || e2.canonical_name AS fact
        FROM meclaw.entity_events ee1
        JOIN meclaw.entities e1 ON e1.id = ee1.entity_id
        JOIN meclaw.entity_events ee2 ON ee2.event_id = ee1.event_id
        JOIN meclaw.entities e2 ON e2.id = ee2.entity_id
        WHERE ee1.event_id = p_event_id
          AND ee1.relation_type NOT IN ('MENTIONED_IN', 'INVOLVED_IN')
          AND ee2.relation_type IN ('MENTIONED_IN', 'INVOLVED_IN')
          AND ee1.entity_id != ee2.entity_id
    ) sub;

    -- Concatenate
    IF v_entity_parts IS NOT NULL THEN
        v_facts := array_to_string(v_entity_parts, ' | ');
    END IF;

    IF v_relation_parts IS NOT NULL THEN
        IF v_facts <> '' THEN
            v_facts := v_facts || ' | ';
        END IF;
        v_facts := v_facts || array_to_string(v_relation_parts, ' | ');
    END IF;

    RETURN NULLIF(trim(v_facts), '');
END;
$$ LANGUAGE plpgsql STABLE;

-- =============================================================================
-- 3. Backfill facts_text for all already-extracted events
-- =============================================================================

UPDATE meclaw.brain_events be
SET facts_text = meclaw.build_facts_text(be.id)
WHERE be.extracted = TRUE
  AND be.extraction_data IS NOT NULL
  AND NOT (be.extraction_data ? 'skipped')
  AND NOT (be.extraction_data ? 'error');

-- =============================================================================
-- 4. Drop old BM25 index, recreate with facts_text
-- =============================================================================

DROP INDEX IF EXISTS meclaw.idx_brain_events_bm25;

CREATE INDEX idx_brain_events_bm25
ON meclaw.brain_events
USING bm25 (id, content, facts_text)
WITH (
    key_field = 'id',
    text_fields = '{"content": {}, "facts_text": {}}'
);

-- =============================================================================
-- 5. Updated retrieve_bee — searches content AND facts_text
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
    source TEXT,
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
        RETURN;
    END IF;

    -- 2. BM25 search across content + facts_text
    -- Phase C1: unified BM25 over both fields with RRF scoring
    RETURN QUERY
    WITH bm25_results AS (
        SELECT
            be.id AS bid,
            be.content AS bcontent,
            paradedb.score(be.id) AS bm25_score,
            be.channel_id AS bchan,
            COALESCE(be.reward, 0.0) AS breward,
            be.created_at AS bts,
            ROW_NUMBER() OVER (ORDER BY paradedb.score(be.id) DESC) AS bm25_rank
        FROM meclaw.brain_events be
        WHERE be.content @@@ p_query
            AND be.channel_id = ANY(v_channel_ids)
            AND (be.agent_id IS NULL OR be.agent_id = p_agent_id)
        LIMIT 20
    ),
    -- Facts BM25 search (boost if matched in facts_text)
    facts_results AS (
        SELECT
            be.id AS fid,
            be.content AS fcontent,
            paradedb.score(be.id) AS facts_score,
            be.channel_id AS fchan,
            COALESCE(be.reward, 0.0) AS freward,
            be.created_at AS fts,
            ROW_NUMBER() OVER (ORDER BY paradedb.score(be.id) DESC) AS facts_rank
        FROM meclaw.brain_events be
        WHERE be.facts_text @@@ p_query
            AND be.channel_id = ANY(v_channel_ids)
            AND (be.agent_id IS NULL OR be.agent_id = p_agent_id)
        LIMIT 20
    ),
    -- Union of candidate IDs
    all_candidates AS (
        SELECT bid AS cid FROM bm25_results
        UNION
        SELECT fid AS cid FROM facts_results
    ),
    rrf_scored AS (
        SELECT
            ac.cid,
            COALESCE(1.0 / (60.0 + b.bm25_rank), 0.0) +
            COALESCE(1.0 / (60.0 + f.facts_rank), 0.0) AS rrf_score,
            CASE
                WHEN b.bm25_rank IS NOT NULL AND f.facts_rank IS NOT NULL THEN 'rrf_both'
                WHEN f.facts_rank IS NOT NULL THEN 'facts_bm25'
                ELSE 'content_bm25'
            END AS src,
            COALESCE(b.bchan, f.fchan) AS chan_id,
            COALESCE(b.breward, f.freward, 0.0) AS rew,
            COALESCE(b.bts, f.fts) AS ts
        FROM all_candidates ac
        LEFT JOIN bm25_results b ON b.bid = ac.cid
        LEFT JOIN facts_results f ON f.fid = ac.cid
    )
    SELECT
        r.cid,
        be.content,
        (r.rrf_score + r.rew * 0.01)::FLOAT AS score,
        r.src,
        r.chan_id,
        r.rew,
        r.ts
    FROM rrf_scored r
    JOIN meclaw.brain_events be ON be.id = r.cid
    ORDER BY (r.rrf_score + r.rew * 0.01) DESC
    LIMIT p_limit;

END;
$$ LANGUAGE plpgsql;

-- =============================================================================
-- 6. Update llm_extract_entities to populate facts_text after extraction
-- =============================================================================

-- Patch: after entity/relation processing, call build_facts_text and store it.
-- We do this by adding a UPDATE step at the end of the extraction result commit.
-- The actual llm_extract_entities function is in 28_extract_bee_v2.sql (plpython3u).
-- We handle this via an AFTER trigger on brain_events.extraction_data changes.

CREATE OR REPLACE FUNCTION meclaw.trg_update_facts_text()
RETURNS TRIGGER AS $$
BEGIN
    -- Only fire when extraction completes (extracted flips to TRUE)
    IF NEW.extracted = TRUE AND (OLD.extracted IS DISTINCT FROM TRUE) THEN
        -- Don't block on errors — facts_text is best-effort
        BEGIN
            NEW.facts_text := meclaw.build_facts_text(NEW.id);
        EXCEPTION WHEN OTHERS THEN
            -- Ignore
        END;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS trg_update_facts_text ON meclaw.brain_events;
CREATE TRIGGER trg_update_facts_text
    BEFORE UPDATE OF extracted ON meclaw.brain_events
    FOR EACH ROW
    EXECUTE FUNCTION meclaw.trg_update_facts_text();

-- =============================================================================
-- 7. Convenience: search with facts_text shown
-- =============================================================================

DROP FUNCTION IF EXISTS meclaw.memory_search(TEXT, TEXT, INT);
CREATE OR REPLACE FUNCTION meclaw.memory_search(
    p_agent_id TEXT,
    p_query TEXT,
    p_limit INT DEFAULT 5
)
RETURNS TABLE (
    content TEXT,
    facts_text TEXT,
    score FLOAT,
    source TEXT,
    age_hours FLOAT,
    reward FLOAT
) AS $$
    SELECT
        r.content,
        be.facts_text,
        r.score,
        r.source,
        EXTRACT(EPOCH FROM (clock_timestamp() - r.created_at)) / 3600.0 AS age_hours,
        r.reward
    FROM meclaw.retrieve_bee(p_agent_id, p_query, p_limit) r
    JOIN meclaw.brain_events be ON be.id = r.event_id;
$$ LANGUAGE sql;

-- =============================================================================
-- 8. Smoke check: show facts_text for extracted events
-- =============================================================================

DO $$
DECLARE
    v_count INT;
    v_with_facts INT;
BEGIN
    SELECT COUNT(*) INTO v_count FROM meclaw.brain_events WHERE extracted = TRUE;
    SELECT COUNT(*) INTO v_with_facts FROM meclaw.brain_events WHERE facts_text IS NOT NULL AND facts_text <> '';
    RAISE NOTICE 'Phase C1: extracted=%, with facts_text=%', v_count, v_with_facts;
END;
$$;

COMMENT ON COLUMN meclaw.brain_events.facts_text IS
'Searchable facts derived from extraction_data: "Entity: X, Type: Y | Relation: A REL B"
Populated by build_facts_text() after llm_extract_entities completes.
Covered by the BM25 index for fact-augmented retrieval.';

COMMENT ON FUNCTION meclaw.retrieve_bee IS
'Agent-level memory retrieval. Phase C1: BM25 over content + facts_text with RRF fusion.
Phase 2+: + pgvector RRF, personality_fit, graph expansion.';

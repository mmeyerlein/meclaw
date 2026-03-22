-- =============================================================================
-- Temporal-Aware Retrieval: question_date Filter + Time-Aware Query Expansion
-- =============================================================================
-- Drei Features:
-- 1. retrieve_temporal: retrieve_bee Wrapper mit before_date Filter
-- 2. decompose_message: LLM zerlegt Messages in atomare Fakten
-- 3. expand_temporal_query: LLM erkennt temporale Richtung und Anchor
-- =============================================================================

-- =============================================================================
-- 1. retrieve_temporal: Retrieval mit Zeitfilter
-- =============================================================================
-- Filtert brain_events VOR question_date, dann normales retrieve_bee
-- Für temporale Fragen zusätzlich: ORDER BY created_at statt nur Score
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw.retrieve_temporal(
    p_agent_id TEXT,
    p_query TEXT,
    p_before_date TIMESTAMPTZ DEFAULT NULL,
    p_after_date TIMESTAMPTZ DEFAULT NULL,
    p_limit INT DEFAULT 10,
    p_temporal_order TEXT DEFAULT NULL,  -- 'asc' | 'desc' | NULL
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
BEGIN
    -- Use standard retrieve_bee, then filter+reorder results
    RETURN QUERY
    WITH base_results AS (
        SELECT r.event_id, r.content, r.score, r.source, r.created_at
        FROM meclaw.retrieve_bee(p_agent_id, p_query, 
             -- Fetch more candidates for temporal filtering
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
            ELSE -f.score  -- default: by score descending
        END
    LIMIT p_limit;
END;
$$;

COMMENT ON FUNCTION meclaw.retrieve_temporal IS
'Temporal-aware retrieval: filters events by date range and optionally orders by time instead of score.';

-- =============================================================================
-- 2. decompose_message: LLM zerlegt eine Nachricht in atomare Fakten
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw.decompose_message(
    p_content TEXT,
    p_session_date TEXT DEFAULT NULL
)
RETURNS TABLE (
    fact TEXT,
    fact_date TEXT,
    fact_type TEXT  -- 'event', 'preference', 'fact', 'opinion'
)
LANGUAGE plpython3u AS $func$
import json, urllib.request, ssl

if not p_content or len(p_content.strip()) < 20:
    return []

# Get API key
row = plpy.execute("SELECT api_key, base_url FROM meclaw.llm_providers WHERE id = 'openrouter'")
if not row:
    return [{"fact": p_content, "fact_date": p_session_date or "", "fact_type": "fact"}]

api_key = row[0]["api_key"]
base_url = row[0]["base_url"]

date_context = f" The conversation took place on {p_session_date}." if p_session_date else ""

prompt = f"""Extract atomic facts from this message. Each fact must be completely self-contained — a reader should understand it WITHOUT seeing the original message.

Message:{date_context}
"{p_content}"

Return a JSON array of objects with:
- "fact": the atomic fact as a clear, self-contained statement
- "fact_date": specific date mentioned (YYYY-MM-DD format) or "unknown"  
- "fact_type": one of "event", "preference", "fact", "opinion"

CRITICAL Rules:
- Each fact MUST include its own time reference. Never split a temporal reference from its fact.
  BAD: "User bought training pads for Luna" (missing when)
  GOOD: "User bought training pads for Luna about a month ago (around {p_session_date or 'unknown date'})"
- Resolve relative dates ("last week", "two months ago", "yesterday") to absolute dates using the conversation date
- Include BOTH the original relative phrase AND the resolved date in the fact text
  Example: "User started bird watching about two months ago (around January 2023)"
- Split compound sentences into individual facts, but preserve temporal context in EACH fact
- Skip generic filler ("that's great", "thanks")
- Keep each fact under 60 words
- Return ONLY valid JSON array

Example: [{{"fact": "User had car serviced on March 15th 2023", "fact_date": "2023-03-15", "fact_type": "event"}}, {{"fact": "GPS system started malfunctioning after the car service on March 15th 2023", "fact_date": "2023-03-15", "fact_type": "event"}}]"""

try:
    url = base_url.rstrip("/") + "/chat/completions"
    payload = json.dumps({
        "model": "openai/gpt-4o-mini",
        "messages": [{"role": "user", "content": prompt}],
        "temperature": 0.0,
        "max_tokens": 500,
        "response_format": {"type": "json_object"}
    }).encode()

    req = urllib.request.Request(url, data=payload, headers={
        "Authorization": f"Bearer {api_key}",
        "Content-Type": "application/json",
        "X-Title": "MeClaw"
    })

    ctx = ssl.create_default_context()
    resp = urllib.request.urlopen(req, timeout=30, context=ctx)
    result = json.loads(resp.read().decode())
    content = result["choices"][0]["message"]["content"]
    
    parsed = json.loads(content)
    if isinstance(parsed, dict):
        # Handle {"facts": [...]} wrapper
        for v in parsed.values():
            if isinstance(v, list):
                parsed = v
                break
    
    if isinstance(parsed, list):
        facts = []
        for item in parsed:
            if isinstance(item, dict) and item.get("fact"):
                facts.append({
                    "fact": item["fact"],
                    "fact_date": item.get("fact_date", "unknown"),
                    "fact_type": item.get("fact_type", "fact")
                })
        return facts
    
    return [{"fact": p_content, "fact_date": p_session_date or "", "fact_type": "fact"}]

except Exception as e:
    plpy.warning(f"decompose_message error: {e}")
    return [{"fact": p_content, "fact_date": p_session_date or "", "fact_type": "fact"}]
$func$;

COMMENT ON FUNCTION meclaw.decompose_message IS
'LLM-based session decomposition: splits a message into atomic facts with dates and types.';

-- =============================================================================
-- 3. expand_temporal_query: LLM erkennt temporale Richtung
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw.expand_temporal_query(
    p_question TEXT,
    p_question_date TEXT DEFAULT NULL
)
RETURNS TABLE (
    expanded_query TEXT,
    temporal_direction TEXT,  -- 'first_after', 'last_before', 'between', 'latest', 'earliest', 'none'
    temporal_anchor TEXT,     -- anchor event/date description
    time_filter_after TEXT,   -- ISO date for after-filter (or NULL)
    time_filter_before TEXT   -- ISO date for before-filter (or NULL)
)
LANGUAGE plpython3u AS $func$
import json, urllib.request, ssl

row = plpy.execute("SELECT api_key, base_url FROM meclaw.llm_providers WHERE id = 'openrouter'")
if not row:
    return [{"expanded_query": p_question, "temporal_direction": "none", 
             "temporal_anchor": None, "time_filter_after": None, "time_filter_before": None}]

api_key = row[0]["api_key"]
base_url = row[0]["base_url"]

date_ctx = f" The question is asked on {p_question_date}." if p_question_date else ""

prompt = f"""Analyze this question for temporal reasoning requirements.{date_ctx}

Question: "{p_question}"

Return JSON with:
- "expanded_query": the question rewritten for better keyword/semantic search (include key entities and topics)
- "temporal_direction": one of "first_after", "last_before", "between", "latest", "earliest", "none"
  - "first_after": asking about the FIRST event after some anchor ("What was the first X after Y?")
  - "last_before": asking about the LAST event before some anchor
  - "between": asking about events between two points
  - "latest": asking about the most recent state
  - "earliest": asking about the earliest occurrence
  - "none": no temporal reasoning needed
- "temporal_anchor": description of the anchor event or date (e.g., "first car service", "March 2023")
- "time_filter_after": ISO date (YYYY-MM-DD) for after-filter, or null
- "time_filter_before": ISO date (YYYY-MM-DD) for before-filter, or null

Return ONLY valid JSON object."""

try:
    url = base_url.rstrip("/") + "/chat/completions"
    payload = json.dumps({
        "model": "openai/gpt-4o-mini",
        "messages": [{"role": "user", "content": prompt}],
        "temperature": 0.0,
        "max_tokens": 300,
        "response_format": {"type": "json_object"}
    }).encode()

    req = urllib.request.Request(url, data=payload, headers={
        "Authorization": f"Bearer {api_key}",
        "Content-Type": "application/json",
        "X-Title": "MeClaw"
    })

    ctx = ssl.create_default_context()
    resp = urllib.request.urlopen(req, timeout=30, context=ctx)
    result = json.loads(resp.read().decode())
    content = result["choices"][0]["message"]["content"]
    
    parsed = json.loads(content)
    return [{
        "expanded_query": parsed.get("expanded_query", p_question),
        "temporal_direction": parsed.get("temporal_direction", "none"),
        "temporal_anchor": parsed.get("temporal_anchor"),
        "time_filter_after": parsed.get("time_filter_after"),
        "time_filter_before": parsed.get("time_filter_before")
    }]

except Exception as e:
    plpy.warning(f"expand_temporal_query error: {e}")
    return [{"expanded_query": p_question, "temporal_direction": "none",
             "temporal_anchor": None, "time_filter_after": None, "time_filter_before": None}]
$func$;

COMMENT ON FUNCTION meclaw.expand_temporal_query IS
'LLM-based temporal query expansion: detects temporal direction and generates time filters.';

-- =============================================================================
-- 4. retrieve_smart: Combines everything — temporal expansion + retrieval + reranking
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw.retrieve_smart(
    p_agent_id TEXT,
    p_query TEXT,
    p_question_date TEXT DEFAULT NULL,
    p_limit INT DEFAULT 10,
    p_rerank BOOLEAN DEFAULT TRUE,
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
    v_expanded RECORD;
    v_before_date TIMESTAMPTZ;
    v_after_date TIMESTAMPTZ;
    v_temporal_order TEXT;
    v_query TEXT;
    v_candidates JSONB;
    v_reranked JSONB;
    v_item JSONB;
BEGIN
    -- Step 1: Temporal Query Expansion
    SELECT * INTO v_expanded
    FROM meclaw.expand_temporal_query(p_query, p_question_date);

    v_query := COALESCE(v_expanded.expanded_query, p_query);
    
    -- Parse time filters (date-only values → end of day / start of day)
    IF v_expanded.time_filter_before IS NOT NULL THEN
        v_before_date := v_expanded.time_filter_before::TIMESTAMPTZ;
        -- If date-only (00:00:00), extend to end of day
        IF v_before_date = date_trunc('day', v_before_date) THEN
            v_before_date := v_before_date + interval '1 day' - interval '1 second';
        END IF;
    END IF;
    IF v_expanded.time_filter_after IS NOT NULL THEN
        v_after_date := v_expanded.time_filter_after::TIMESTAMPTZ;
    END IF;
    
    -- Use question_date as upper bound if no explicit before filter
    IF v_before_date IS NULL AND p_question_date IS NOT NULL THEN
        BEGIN
            v_before_date := meclaw.parse_benchmark_date(p_question_date);
            -- Ensure end-of-day if date portion only
            IF v_before_date IS NOT NULL AND v_before_date = date_trunc('day', v_before_date) THEN
                v_before_date := v_before_date + interval '1 day' - interval '1 second';
            END IF;
        EXCEPTION WHEN OTHERS THEN
            NULL;
        END;
    END IF;

    -- Determine temporal ordering
    IF v_expanded.temporal_direction IN ('first_after', 'earliest') THEN
        v_temporal_order := 'asc';
    ELSIF v_expanded.temporal_direction IN ('last_before', 'latest') THEN
        v_temporal_order := 'desc';
    ELSE
        v_temporal_order := NULL;
    END IF;

    -- Step 2: Dual-Query Retrieval (expanded + original, merged & deduplicated)
    -- Using both queries catches cases where the expanded query matches wrong topics
    IF p_rerank THEN
        -- 2a. Expanded query with time filter
        SELECT jsonb_agg(jsonb_build_object(
            'id', r.event_id::text,
            'content', r.content,
            'score', r.score,
            'source', r.source,
            'created_at', r.created_at
        ))
        INTO v_candidates
        FROM meclaw.retrieve_temporal(
            p_agent_id, v_query, v_before_date, v_after_date,
            LEAST(p_limit * 2, 20), v_temporal_order, p_ctm_enabled
        ) r;

        -- 2b. Original query (no expansion, with time filter) — catches what expansion missed
        IF v_query != p_query THEN
            SELECT COALESCE(v_candidates, '[]'::jsonb) || COALESCE(jsonb_agg(jsonb_build_object(
                'id', r.event_id::text,
                'content', r.content,
                'score', r.score * 0.9,  -- slight penalty for non-expanded
                'source', r.source,
                'created_at', r.created_at
            )), '[]'::jsonb)
            INTO v_candidates
            FROM meclaw.retrieve_temporal(
                p_agent_id, p_query, v_before_date, v_after_date,
                LEAST(p_limit * 2, 20), v_temporal_order, p_ctm_enabled
            ) r
            WHERE r.event_id::text NOT IN (
                SELECT e->>'id' FROM jsonb_array_elements(COALESCE(v_candidates, '[]'::jsonb)) e
            );
        END IF;

        IF v_candidates IS NULL OR jsonb_array_length(v_candidates) = 0 THEN
            -- Fallback without time filter using ORIGINAL query
            SELECT jsonb_agg(jsonb_build_object(
                'id', r.event_id::text,
                'content', r.content,
                'score', r.score,
                'source', r.source,
                'created_at', r.created_at
            ))
            INTO v_candidates
            FROM meclaw.retrieve_bee(p_agent_id, p_query, p_limit * 3, p_ctm_enabled) r;
        END IF;

        IF v_candidates IS NOT NULL AND jsonb_array_length(v_candidates) > 0 THEN
            -- Step 3: LLM Re-Ranking
            v_reranked := meclaw.llm_rerank(p_query, v_candidates, p_limit);
            
            FOR v_item IN SELECT * FROM jsonb_array_elements(v_reranked)
            LOOP
                event_id := (v_item->>'id')::UUID;
                content := v_item->>'content';
                score := COALESCE((v_item->>'relevance')::FLOAT, (v_item->>'score')::FLOAT);
                source := COALESCE(v_item->>'source', 'smart');
                created_at := (v_item->>'created_at')::TIMESTAMPTZ;
                RETURN NEXT;
            END LOOP;
        END IF;
    ELSE
        -- Without reranking: just temporal retrieval
        RETURN QUERY
        SELECT r.event_id, r.content, r.score, r.source, r.created_at
        FROM meclaw.retrieve_temporal(
            p_agent_id, v_query, v_before_date, v_after_date,
            p_limit, v_temporal_order, p_ctm_enabled
        ) r;
    END IF;
END;
$$;

-- Helper: Parse benchmark date format
CREATE OR REPLACE FUNCTION meclaw.parse_benchmark_date(p_date_str TEXT)
RETURNS TIMESTAMPTZ
LANGUAGE plpython3u AS $func$
import re
from datetime import datetime
if not p_date_str:
    return None
clean = re.sub(r'\s*\([A-Za-z]+\)\s*', ' ', p_date_str).strip()
try:
    dt = datetime.strptime(clean, "%Y/%m/%d %H:%M")
    return dt
except:
    try:
        dt = datetime.strptime(clean, "%Y-%m-%d")
        return dt
    except:
        return None
$func$;

COMMENT ON FUNCTION meclaw.retrieve_smart IS
'Smart retrieval: temporal query expansion + time-filtered retrieval + optional LLM re-ranking. The full pipeline.';

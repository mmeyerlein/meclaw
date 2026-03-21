-- =============================================================================
-- E9: LLM-Guided Re-Ranking (Stage 3 Retrieval)
-- =============================================================================
-- BRAIN.md: For complex queries, the LLM reads candidate summaries and
-- decides which are truly relevant. Cheap (summaries, not raw data),
-- but dramatically more precise.
-- =============================================================================

-- =============================================================================
-- 1. llm_rerank: Takes candidates + question, returns re-ranked list
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw.llm_rerank(
    p_question TEXT,
    p_candidates JSONB,  -- [{id, content, score}]
    p_top_k INT DEFAULT 5
)
RETURNS JSONB  -- [{id, content, relevance, reasoning}]
LANGUAGE plpython3u AS $func$
import json

candidates = json.loads(p_candidates) if isinstance(p_candidates, str) else p_candidates

if not candidates:
    return json.dumps([])

# Build numbered candidate list for the LLM
candidate_list = []
for i, c in enumerate(candidates):
    snippet = (c.get("content") or "")[:300]
    candidate_list.append(f"[{i+1}] {snippet}")

candidates_text = "\n".join(candidate_list)

prompt = f"""You are a memory retrieval judge. Given a question and a list of memory snippets, 
select the most relevant snippets that help answer the question.

QUESTION: {p_question}

MEMORY SNIPPETS:
{candidates_text}

Return a JSON array of the top {p_top_k} most relevant snippet numbers, ordered by relevance.
For each, provide: {{"rank": <number>, "reasoning": "<brief reason>"}}

Rules:
- Only include snippets that contain information relevant to answering the question
- A snippet mentioning the topic in passing is less relevant than one with specific details
- For temporal questions ("first", "when", "before/after"), prioritize snippets with dates or time references
- Return ONLY valid JSON array, no other text

Example: [{{"rank": 3, "reasoning": "directly mentions the GPS issue after service"}}, {{"rank": 1, "reasoning": "discusses car service timeline"}}]"""

# Call LLM via meclaw's llm infrastructure
try:
    row = plpy.execute("""
        SELECT api_key, base_url FROM meclaw.llm_providers WHERE id = 'openrouter'
    """)
    if not row:
        plpy.warning("llm_rerank: no openrouter provider")
        return p_candidates

    api_key = row[0]["api_key"]
    base_url = row[0]["base_url"]

    import urllib.request
    import ssl

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
        "X-Title": "MeClaw",
        "HTTP-Referer": "https://github.com/mmeyerlein/meclaw"
    })

    ctx = ssl.create_default_context()
    resp = urllib.request.urlopen(req, timeout=30, context=ctx)
    result = json.loads(resp.read().decode())

    content = result["choices"][0]["message"]["content"]
    
    # Parse LLM response
    try:
        parsed = json.loads(content)
        # Handle both {"rankings": [...]} and direct [...]
        if isinstance(parsed, dict):
            rankings = parsed.get("rankings") or parsed.get("results") or parsed.get("top") or []
            if not rankings:
                # Try first list-like value
                for v in parsed.values():
                    if isinstance(v, list):
                        rankings = v
                        break
        elif isinstance(parsed, list):
            rankings = parsed
        else:
            rankings = []
    except json.JSONDecodeError:
        # Try to extract array from response
        import re
        match = re.search(r'\[.*\]', content, re.DOTALL)
        if match:
            rankings = json.loads(match.group())
        else:
            plpy.warning(f"llm_rerank: could not parse response: {content[:200]}")
            return p_candidates

    # Build re-ranked result
    reranked = []
    seen = set()
    for r in rankings[:p_top_k]:
        rank_num = r.get("rank", 0)
        if isinstance(rank_num, int) and 1 <= rank_num <= len(candidates) and rank_num not in seen:
            seen.add(rank_num)
            c = candidates[rank_num - 1]
            c["relevance"] = len(rankings) - len(reranked)  # higher = more relevant
            c["reasoning"] = r.get("reasoning", "")
            reranked.append(c)

    # If LLM returned fewer than top_k, pad with original order
    if len(reranked) < p_top_k:
        for c in candidates:
            if c["id"] not in [r["id"] for r in reranked]:
                reranked.append(c)
                if len(reranked) >= p_top_k:
                    break

    return json.dumps(reranked)

except Exception as e:
    plpy.warning(f"llm_rerank error: {e}")
    # Fallback: return original candidates
    return json.dumps(candidates[:p_top_k])
$func$;

COMMENT ON FUNCTION meclaw.llm_rerank IS
'LLM-guided re-ranking of retrieval candidates. Stage 3 in the retrieval pipeline.';

-- =============================================================================
-- 2. retrieve_reranked: Full pipeline with LLM re-ranking
-- =============================================================================
CREATE OR REPLACE FUNCTION meclaw.retrieve_reranked(
    p_agent_id TEXT,
    p_query TEXT,
    p_limit INT DEFAULT 5,
    p_ctm_enabled BOOLEAN DEFAULT FALSE,
    p_rerank_pool INT DEFAULT 20  -- how many candidates to feed to LLM
)
RETURNS TABLE (
    event_id UUID,
    content TEXT,
    score FLOAT,
    source TEXT
)
LANGUAGE plpgsql AS $$
DECLARE
    v_candidates JSONB;
    v_reranked JSONB;
    v_item JSONB;
BEGIN
    -- Stage 1+2: Get broad candidate pool from retrieve_bee
    SELECT jsonb_agg(jsonb_build_object(
        'id', r.event_id::text,
        'content', r.content,
        'score', r.score,
        'source', r.source
    ))
    INTO v_candidates
    FROM meclaw.retrieve_bee(p_agent_id, p_query, p_rerank_pool, p_ctm_enabled) r;

    IF v_candidates IS NULL OR jsonb_array_length(v_candidates) = 0 THEN
        RETURN;
    END IF;

    -- Stage 3: LLM re-ranking
    v_reranked := meclaw.llm_rerank(p_query, v_candidates, p_limit);

    -- Return re-ranked results
    FOR v_item IN SELECT * FROM jsonb_array_elements(v_reranked)
    LOOP
        event_id := (v_item->>'id')::UUID;
        content := v_item->>'content';
        score := COALESCE((v_item->>'relevance')::FLOAT, (v_item->>'score')::FLOAT);
        source := COALESCE(v_item->>'source', 'reranked');
        RETURN NEXT;
    END LOOP;
END;
$$;

COMMENT ON FUNCTION meclaw.retrieve_reranked IS
'3-stage retrieval: BM25+Vector+Graph → 6-Signal Ranking → LLM Re-Ranking. Most precise but costs 1 LLM call per query.';

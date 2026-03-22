-- MeClaw v0.1.0 — Embedding Service
-- Date: 2026-03-20
-- Ref: docs/BRAIN.md (Retrieval, pgvector)
--
-- Computes embeddings for brain_events via pg_net → OpenRouter embedding API.
-- Uses pg_background for async execution (separate transaction).
-- Updates brain_events.embedding after response.

-- =============================================================================
-- 1. Embedding config table (or use llm_providers)
-- =============================================================================

-- We store embedding config in llm_providers alongside LLM configs
INSERT INTO meclaw.llm_providers (id, name, base_url, api_key, config)
VALUES (
    'embedding-openrouter',
    'OpenRouter Embeddings',
    'https://openrouter.ai/api/v1/embeddings',
    '***REMOVED***',
    '{"model": "openai/text-embedding-3-small", "dimensions": 1536}'::jsonb
) ON CONFLICT (id) DO UPDATE SET
    base_url = EXCLUDED.base_url,
    api_key = EXCLUDED.api_key,
    config = EXCLUDED.config;

-- =============================================================================
-- 2. Compute embedding for a brain_event (synchronous via plpython3u)
-- =============================================================================

CREATE OR REPLACE FUNCTION meclaw.compute_embedding(p_event_id UUID)
RETURNS VOID AS $fn$
    import json
    import requests

    # Get event content
    plan = plpy.prepare("SELECT content FROM meclaw.brain_events WHERE id = $1", ["uuid"])
    result = plan.execute([str(p_event_id)])
    if not result:
        plpy.warning(f"compute_embedding: event {p_event_id} not found")
        return
    content = result[0]["content"]
    if not content or len(content.strip()) < 3:
        return

    # Truncate to avoid token limits (roughly 8000 chars ≈ 2000 tokens)
    content = content[:8000]

    # Get embedding provider config
    plan2 = plpy.prepare("SELECT base_url, api_key, config FROM meclaw.llm_providers WHERE id = $1", ["text"])
    prov = plan2.execute(["embedding-openrouter"])
    if not prov:
        plpy.warning("compute_embedding: no embedding provider configured")
        return

    base_url = prov[0]["base_url"]
    api_key = prov[0]["api_key"]
    config = json.loads(prov[0]["config"]) if prov[0]["config"] else {}
    model = config.get("model", "openai/text-embedding-3-small")

    # Call embedding API
    try:
        resp = requests.post(
            base_url,
            headers={
                "Authorization": f"Bearer {api_key}",
                "Content-Type": "application/json",
                "HTTP-Referer": "https://meclaw.ai",
                "X-Title": "MeClaw"
            },
            json={"model": model, "input": content},
            timeout=30
        )
        resp.raise_for_status()
        data = resp.json()
        embedding = data["data"][0]["embedding"]

        # Update brain_event with embedding
        vec_str = "[" + ",".join(str(x) for x in embedding) + "]"
        update_plan = plpy.prepare(
            "UPDATE meclaw.brain_events SET embedding = $1::vector WHERE id = $2",
            ["text", "uuid"]
        )
        update_plan.execute([vec_str, str(p_event_id)])

    except Exception as e:
        plpy.warning(f"compute_embedding failed for {p_event_id}: {e}")
$fn$ LANGUAGE plpython3u;

-- =============================================================================
-- 3. Batch compute embeddings for all events without embedding
-- =============================================================================

CREATE OR REPLACE FUNCTION meclaw.compute_embeddings_batch(p_limit INT DEFAULT 100)
RETURNS INT AS $fn$
    import json
    import requests

    # Get embedding provider config
    plan_prov = plpy.prepare("SELECT base_url, api_key, config FROM meclaw.llm_providers WHERE id = $1", ["text"])
    prov = plan_prov.execute(["embedding-openrouter"])
    if not prov:
        plpy.warning("compute_embeddings_batch: no embedding provider configured")
        return 0

    base_url = prov[0]["base_url"]
    api_key = prov[0]["api_key"]
    config = json.loads(prov[0]["config"]) if prov[0]["config"] else {}
    model = config.get("model", "openai/text-embedding-3-small")
    batch_size = config.get("batch_size", 50)  # OpenAI supports up to 2048

    # Get events without embeddings
    plan_events = plpy.prepare("""
        SELECT id, content FROM meclaw.brain_events
        WHERE embedding IS NULL AND content IS NOT NULL AND length(content) >= 3
        ORDER BY seq ASC LIMIT $1
    """, ["int4"])
    events = plan_events.execute([p_limit])

    if not events:
        return 0

    # Collect all texts and IDs
    items = [(str(row["id"]), row["content"][:8000]) for row in events]
    count = 0

    # Process in batches (single API call per batch!)
    for batch_start in range(0, len(items), batch_size):
        batch = items[batch_start:batch_start + batch_size]
        ids = [b[0] for b in batch]
        texts = [b[1] for b in batch]

        try:
            # ONE API call for the entire batch
            resp = requests.post(
                base_url,
                headers={
                    "Authorization": f"Bearer {api_key}",
                    "Content-Type": "application/json",
                    "HTTP-Referer": "https://meclaw.ai",
                    "X-Title": "MeClaw"
                },
                json={"model": model, "input": texts},
                timeout=60
            )
            resp.raise_for_status()
            data = resp.json()

            # Update all embeddings from the batch response
            update_plan = plpy.prepare(
                "UPDATE meclaw.brain_events SET embedding = $1::vector WHERE id = $2",
                ["text", "uuid"]
            )
            for emb_item in data["data"]:
                idx = emb_item["index"]
                embedding = emb_item["embedding"]
                vec_str = "[" + ",".join(str(x) for x in embedding) + "]"
                update_plan.execute([vec_str, ids[idx]])
                count += 1

            plpy.notice(f"compute_embeddings_batch: {len(batch)} embeddings in 1 API call")

        except Exception as e:
            plpy.warning(f"compute_embeddings_batch: batch failed ({len(batch)} items): {e}")
            # Fallback: try one by one for this batch
            for event_id, content in batch:
                try:
                    resp = requests.post(
                        base_url,
                        headers={
                            "Authorization": f"Bearer {api_key}",
                            "Content-Type": "application/json",
                            "HTTP-Referer": "https://meclaw.ai",
                            "X-Title": "MeClaw"
                        },
                        json={"model": model, "input": content},
                        timeout=30
                    )
                    resp.raise_for_status()
                    embedding = resp.json()["data"][0]["embedding"]
                    vec_str = "[" + ",".join(str(x) for x in embedding) + "]"
                    update_plan.execute([vec_str, event_id])
                    count += 1
                except Exception as e2:
                    plpy.warning(f"compute_embeddings_batch: single fallback failed for {event_id}: {e2}")

    return count
$fn$ LANGUAGE plpython3u;

-- =============================================================================
-- 4. Update extract_bee to compute embedding after extraction
-- =============================================================================

CREATE OR REPLACE FUNCTION meclaw.extract_bee(p_msg_id UUID)
RETURNS VOID AS $fn$
DECLARE
    v_channel_id UUID;
    v_content TEXT;
    v_message_type TEXT;
    v_task_id UUID;
    v_event_id UUID;
    v_created_at TIMESTAMPTZ;
BEGIN
    -- Get message details (including timestamp for temporal indexing)
    SELECT channel_id, content->>'input', type, task_id, created_at
    INTO v_channel_id, v_content, v_message_type, v_task_id, v_created_at
    FROM meclaw.messages
    WHERE id = p_msg_id;

    -- Only extract from user_input and llm_result messages
    IF v_message_type NOT IN ('user_input', 'llm_result') THEN
        RETURN;
    END IF;

    -- Skip if no content
    IF v_content IS NULL OR v_content = '' THEN
        SELECT content->>'output' INTO v_content
        FROM meclaw.messages WHERE id = p_msg_id;

        IF v_content IS NULL OR v_content = '' THEN
            RETURN;
        END IF;
    END IF;

    -- Create brain_event with original timestamp (temporal indexing!)
    INSERT INTO meclaw.brain_events (
        message_id, channel_id, agent_id, content, created_at
    ) VALUES (
        p_msg_id, v_channel_id, NULL, v_content, COALESCE(v_created_at, clock_timestamp())
    ) RETURNING id INTO v_event_id;

    -- Compute embedding asynchronously via pg_background
    BEGIN
        PERFORM pg_background_launch(
            format('SELECT meclaw.compute_embedding(%L::uuid)', v_event_id)
        );
    EXCEPTION WHEN OTHERS THEN
        -- If pg_background fails, log but don't block the message flow
        INSERT INTO meclaw.events (msg_id, task_id, bee_type, event, payload)
        VALUES (p_msg_id, v_task_id, 'extract_bee', 'embedding_bg_failed',
            jsonb_build_object('error', SQLERRM, 'event_id', v_event_id));
    END;

    -- Update channel extraction tracking
    UPDATE meclaw.channels
    SET last_extracted_seq = COALESCE(
        (SELECT MAX(seq) FROM meclaw.brain_events WHERE channel_id = v_channel_id), 0
    ),
    extraction_status = 'idle',
    updated_at = clock_timestamp()
    WHERE id = v_channel_id;

    -- Log the extraction event
    INSERT INTO meclaw.events (msg_id, task_id, bee_type, event, payload)
    VALUES (p_msg_id, v_task_id, 'extract_bee', 'extraction_complete',
        jsonb_build_object('channel_id', v_channel_id, 'content_length', length(v_content), 'event_id', v_event_id));
END;
$fn$ LANGUAGE plpgsql;

-- =============================================================================
-- 5a. Graph Expansion Helper (plpython3u, can use AGE dynamically)
-- =============================================================================

CREATE OR REPLACE FUNCTION meclaw.graph_expand_events(
    p_anchor_ids TEXT[],   -- array of event_id UUIDs as text
    p_is_temporal BOOLEAN DEFAULT FALSE
)
RETURNS TABLE (event_id UUID, graph_hop INT) AS $fn$
    import re

    if not p_anchor_ids:
        return []

    seen = set()
    results = []

    for anchor_id in p_anchor_ids:
        # Sanitize UUID to prevent injection
        if not re.match(r'^[0-9a-f-]{36}$', anchor_id):
            continue

        # TEMPORAL traversal (1-3 hops, forward direction)
        try:
            plpy.execute("LOAD 'age'")
            plpy.execute("SET search_path = ag_catalog, meclaw, public")
            qry = """
                SELECT * FROM cypher('meclaw_graph', $$
                    MATCH (anchor:Event {{event_id: '{anchor}' }})-[:TEMPORAL*1..3]->(n:Event)
                    RETURN n.event_id AS nid
                $$) AS (nid ag_catalog.agtype)
            """.format(anchor=anchor_id).replace('{{', '{').replace('}}', '}')
            rows = plpy.execute(qry)
            for row in rows:
                nid = row['nid']
                if nid:
                    # Strip surrounding quotes from agtype string
                    nid_clean = str(nid).strip('"')
                    if nid_clean not in seen:
                        seen.add(nid_clean)
                        results.append((nid_clean, 1))
        except Exception as e:
            plpy.warning(f"graph_expand temporal from {anchor_id}: {e}")

        # Entity-based expansion via INVOLVED_IN (2 hops: Event->Entity->Event)
        try:
            plpy.execute("LOAD 'age'")
            plpy.execute("SET search_path = ag_catalog, meclaw, public")
            qry = """
                SELECT * FROM cypher('meclaw_graph', $$
                    MATCH (en:Entity)-[:INVOLVED_IN]->(anchor:Event {{event_id: '{anchor}' }})
                    MATCH (en)-[:INVOLVED_IN]->(n:Event)
                    WHERE n.event_id <> '{anchor}'
                    RETURN n.event_id AS nid
                $$) AS (nid ag_catalog.agtype)
            """.format(anchor=anchor_id).replace('{{', '{').replace('}}', '}')
            rows = plpy.execute(qry)
            for row in rows:
                nid = row['nid']
                if nid:
                    nid_clean = str(nid).strip('"')
                    if nid_clean not in seen:
                        seen.add(nid_clean)
                        results.append((nid_clean, 2))
        except Exception as e:
            plpy.warning(f"graph_expand entity from {anchor_id}: {e}")

    return results
$fn$ LANGUAGE plpython3u;

COMMENT ON FUNCTION meclaw.graph_expand_events IS
'Expand from anchor event IDs via AGE graph (TEMPORAL + INVOLVED_IN). Returns neighbor event_ids with hop distance.';

-- =============================================================================
-- 5. retrieve_bee v3: BM25 + Vector RRF + AGE Graph Expansion + 6-Signal Ranking
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
) AS $fn$
DECLARE
    v_channel_ids UUID[];
    v_query_embedding vector(1536);
    v_has_embeddings BOOLEAN;
    v_is_temporal_query BOOLEAN;
    v_now TIMESTAMPTZ;
    v_max_seq BIGINT;
BEGIN
    -- 1. Get channels this agent can access
    SELECT array_agg(ac.channel_id)
    INTO v_channel_ids
    FROM meclaw.agent_channels ac
    WHERE ac.agent_id = p_agent_id;

    IF v_channel_ids IS NULL OR array_length(v_channel_ids, 1) = 0 THEN
        RETURN;
    END IF;

    -- 2. Detect temporal query (for TEMPORAL-edge-first traversal)
    v_is_temporal_query := (
        lower(p_query) ~ '\m(first|before|after|when|how many days|earliest|latest|last time|since|until|between)\M'
    );

    -- 3. Check if we have embeddings, compute query embedding
    SELECT EXISTS(
        SELECT 1 FROM meclaw.brain_events be
        WHERE be.embedding IS NOT NULL AND be.channel_id = ANY(v_channel_ids)
        LIMIT 1
    ) INTO v_has_embeddings;

    IF v_has_embeddings THEN
        BEGIN
            SELECT meclaw.get_query_embedding(p_query) INTO v_query_embedding;
        EXCEPTION WHEN OTHERS THEN
            v_query_embedding := NULL;
        END;
    END IF;

    -- Get reference values for recency scoring
    SELECT clock_timestamp(), COALESCE(MAX(be2.seq), 1)
    INTO v_now, v_max_seq
    FROM meclaw.brain_events be2
    WHERE be2.channel_id = ANY(v_channel_ids);

    -- 4. Stage 1: BM25 + Vector RRF (top-20 anchors)
    -- 5. Stage 2: AGE Graph Expansion from top-5 anchors
    -- 6. Stage 3: 6-Signal Ranking
    RETURN QUERY
    WITH bm25_results AS (
        SELECT
            be.id AS event_id,
            be.content,
            pdb.score(be.id) AS bm25_score,
            be.channel_id,
            COALESCE(be.reward, 0.0) AS reward,
            COALESCE(be.novelty, 0.0) AS novelty,
            be.created_at,
            be.seq,
            ROW_NUMBER() OVER (ORDER BY pdb.score(be.id) DESC) AS bm25_rank
        FROM meclaw.brain_events be
        WHERE be.content @@@ p_query
            AND be.channel_id = ANY(v_channel_ids)
            AND (be.agent_id IS NULL OR be.agent_id = p_agent_id)
        ORDER BY pdb.score(be.id) DESC
        LIMIT 20
    ),
    vector_results AS (
        SELECT
            be.id AS event_id,
            be.content,
            1 - (be.embedding <=> v_query_embedding) AS vec_score,
            be.channel_id,
            COALESCE(be.reward, 0.0) AS reward,
            COALESCE(be.novelty, 0.0) AS novelty,
            be.created_at,
            be.seq,
            ROW_NUMBER() OVER (ORDER BY be.embedding <=> v_query_embedding) AS vec_rank
        FROM meclaw.brain_events be
        WHERE v_query_embedding IS NOT NULL
            AND be.embedding IS NOT NULL
            AND be.channel_id = ANY(v_channel_ids)
            AND (be.agent_id IS NULL OR be.agent_id = p_agent_id)
        ORDER BY be.embedding <=> v_query_embedding
        LIMIT 20
    ),
    -- Stage 1 RRF: combine BM25 and vector ranks
    stage1_rrf AS (
        SELECT
            COALESCE(b.event_id, v.event_id) AS event_id,
            COALESCE(b.content, v.content) AS content,
            COALESCE(1.0 / (60 + b.bm25_rank), 0) + COALESCE(1.0 / (60 + v.vec_rank), 0) AS rrf_score,
            COALESCE(b.channel_id, v.channel_id) AS channel_id,
            COALESCE(b.reward, v.reward, 0.0) AS reward,
            COALESCE(b.novelty, v.novelty, 0.0) AS novelty,
            COALESCE(b.created_at, v.created_at) AS created_at,
            COALESCE(b.seq, v.seq, 0) AS seq
        FROM bm25_results b
        FULL OUTER JOIN vector_results v ON b.event_id = v.event_id
    ),
    -- Top-5 anchors for graph expansion
    anchors AS (
        SELECT s1.event_id, s1.rrf_score
        FROM stage1_rrf s1
        ORDER BY s1.rrf_score DESC
        LIMIT 5
    ),
    -- Stage 2: AGE Graph Expansion via plpython3u helper
    -- Collects anchor event_ids as array, expands via TEMPORAL + INVOLVED_IN
    anchor_ids_arr AS (
        SELECT array_agg(a.event_id::TEXT) AS ids FROM anchors a
    ),
    all_graph_neighbors AS (
        SELECT ge.event_id, MIN(ge.graph_hop) AS min_hop
        FROM anchor_ids_arr,
        LATERAL meclaw.graph_expand_events(ids, v_is_temporal_query) ge
        GROUP BY ge.event_id
    ),
    -- Fetch brain_events data for graph neighbors (filter by channel access)
    graph_events AS (
        SELECT
            be.id AS event_id,
            be.content,
            be.channel_id,
            COALESCE(be.reward, 0.0) AS reward,
            COALESCE(be.novelty, 0.0) AS novelty,
            be.created_at,
            be.seq,
            COALESCE(gn.min_hop, 3) AS graph_hop
        FROM all_graph_neighbors gn
        JOIN meclaw.brain_events be ON be.id = gn.event_id
        WHERE be.channel_id = ANY(v_channel_ids)
            AND (be.agent_id IS NULL OR be.agent_id = p_agent_id)
    ),
    -- Graph-expanded candidates with RRF rank placeholder (rank = 20 + hop*10)
    graph_rrf AS (
        SELECT
            ge.event_id,
            ge.content,
            1.0 / (60 + 20 + ge.graph_hop * 10) AS rrf_score,
            ge.channel_id,
            ge.reward,
            ge.novelty,
            ge.created_at,
            ge.seq
        FROM graph_events ge
        -- Only include if NOT already in stage1_rrf
        WHERE ge.event_id NOT IN (SELECT s1b.event_id FROM stage1_rrf s1b)
    ),
    -- Merge stage1 + graph expansion
    all_candidates AS (
        SELECT s1.event_id, s1.content, s1.rrf_score, s1.channel_id, s1.reward, s1.novelty, s1.created_at, s1.seq
        FROM stage1_rrf s1
        UNION ALL
        SELECT gr.event_id, gr.content, gr.rrf_score, gr.channel_id, gr.reward, gr.novelty, gr.created_at, gr.seq
        FROM graph_rrf gr
    ),
    -- 6-Signal Ranking:
    --   1. BM25/Vector RRF score (base relevance)
    --   2. Graph distance bonus (captured via graph_rrf above)
    --   3. Recency (exponential decay: newer = better)
    --   4. Reward (reinforcement signal)
    --   5. Novelty (information value)
    -- (personality_fit omitted for now)
    scored AS (
        SELECT
            c.event_id,
            c.content,
            c.channel_id,
            c.reward,
            c.created_at,
            -- Recency: 0..1, half-life ~7 days
            EXP(-EXTRACT(EPOCH FROM (v_now - c.created_at)) / (7 * 86400.0)) AS recency_score,
            -- Normalize reward to 0..1 range (assume reward in -10..10)
            LEAST(1.0, GREATEST(0.0, (c.reward + 10.0) / 20.0)) AS reward_score,
            -- Novelty already 0..1
            COALESCE(c.novelty, 0.0) AS novelty_score,
            -- Base RRF
            c.rrf_score
        FROM all_candidates c
    ),
    final_scored AS (
        SELECT
            s.event_id,
            s.content,
            s.channel_id,
            s.reward,
            s.created_at,
            -- Weighted 6-signal score:
            -- RRF: 50%, Recency: 20%, Reward: 15%, Novelty: 15%
            -- For temporal queries, boost recency weight
            CASE WHEN v_is_temporal_query THEN
                s.rrf_score * 0.40 + s.recency_score * 0.35 + s.reward_score * 0.15 + s.novelty_score * 0.10
            ELSE
                s.rrf_score * 0.50 + s.recency_score * 0.20 + s.reward_score * 0.15 + s.novelty_score * 0.15
            END AS final_score,
            CASE
                WHEN s.rrf_score > 0 AND v_query_embedding IS NOT NULL THEN 'rrf_graph'::TEXT
                WHEN s.rrf_score > 0 THEN 'bm25_graph'::TEXT
                ELSE 'graph'::TEXT
            END AS source
        FROM scored s
    )
    SELECT
        fs.event_id,
        fs.content,
        fs.final_score::FLOAT AS score,
        fs.source,
        fs.channel_id,
        fs.reward,
        fs.created_at
    FROM final_scored fs
    ORDER BY fs.final_score DESC
    LIMIT p_limit;

EXCEPTION WHEN OTHERS THEN
    -- Fallback: if graph expansion fails (e.g. AGE not loaded), return pure RRF
    RETURN QUERY
    WITH bm25_results AS (
        SELECT
            be.id AS event_id,
            be.content,
            pdb.score(be.id) AS bm25_score,
            be.channel_id,
            COALESCE(be.reward, 0.0) AS reward,
            be.created_at,
            ROW_NUMBER() OVER (ORDER BY pdb.score(be.id) DESC) AS bm25_rank
        FROM meclaw.brain_events be
        WHERE be.content @@@ p_query
            AND be.channel_id = ANY(v_channel_ids)
            AND (be.agent_id IS NULL OR be.agent_id = p_agent_id)
        ORDER BY pdb.score(be.id) DESC
        LIMIT 20
    ),
    vector_results AS (
        SELECT
            be.id AS event_id,
            be.content,
            1 - (be.embedding <=> v_query_embedding) AS vec_score,
            be.channel_id,
            COALESCE(be.reward, 0.0) AS reward,
            be.created_at,
            ROW_NUMBER() OVER (ORDER BY be.embedding <=> v_query_embedding) AS vec_rank
        FROM meclaw.brain_events be
        WHERE v_query_embedding IS NOT NULL
            AND be.embedding IS NOT NULL
            AND be.channel_id = ANY(v_channel_ids)
            AND (be.agent_id IS NULL OR be.agent_id = p_agent_id)
        ORDER BY be.embedding <=> v_query_embedding
        LIMIT 20
    ),
    combined AS (
        SELECT
            COALESCE(b.event_id, v.event_id) AS event_id,
            COALESCE(b.content, v.content) AS content,
            COALESCE(1.0 / (60 + b.bm25_rank), 0) + COALESCE(1.0 / (60 + v.vec_rank), 0) AS rrf_score,
            COALESCE(b.channel_id, v.channel_id) AS channel_id,
            COALESCE(b.reward, v.reward, 0.0) AS reward,
            COALESCE(b.created_at, v.created_at) AS created_at
        FROM bm25_results b
        FULL OUTER JOIN vector_results v ON b.event_id = v.event_id
    )
    SELECT
        c.event_id,
        c.content,
        (c.rrf_score + c.reward * 0.01)::FLOAT AS score,
        CASE
            WHEN v_query_embedding IS NOT NULL THEN 'rrf_fallback'::TEXT
            ELSE 'bm25_fallback'::TEXT
        END AS source,
        c.channel_id,
        c.reward,
        c.created_at
    FROM combined c
    ORDER BY (c.rrf_score + c.reward * 0.01) DESC
    LIMIT p_limit;
END;
$fn$ LANGUAGE plpgsql;

-- =============================================================================
-- 6. Comments only — get_query_embedding (non-cached) was here.
--    REMOVED: the cached version in 29_phase7_robustness.sql (sql/29) takes
--    precedence and must NOT be overwritten here. get_query_embedding is now
--    defined only in 29_phase7_robustness.sql with embedding_cache support.
-- =============================================================================

COMMENT ON FUNCTION meclaw.compute_embedding IS
'Compute embedding for a single brain_event via OpenRouter embedding API.';

COMMENT ON FUNCTION meclaw.compute_embeddings_batch IS
'Batch compute embeddings for brain_events without embeddings. Rate-limited.';

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
-- 5. Update retrieve_bee with vector search + RRF
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
BEGIN
    -- 1. Get channels this agent can access
    SELECT array_agg(ac.channel_id)
    INTO v_channel_ids
    FROM meclaw.agent_channels ac
    WHERE ac.agent_id = p_agent_id;

    IF v_channel_ids IS NULL OR array_length(v_channel_ids, 1) = 0 THEN
        RETURN;
    END IF;

    -- 2. Check if we have any embeddings at all
    SELECT EXISTS(
        SELECT 1 FROM meclaw.brain_events be
        WHERE be.embedding IS NOT NULL AND be.channel_id = ANY(v_channel_ids)
        LIMIT 1
    ) INTO v_has_embeddings;

    -- 3. If we have embeddings, compute query embedding for vector search
    IF v_has_embeddings THEN
        BEGIN
            SELECT meclaw.get_query_embedding(p_query) INTO v_query_embedding;
        EXCEPTION WHEN OTHERS THEN
            v_query_embedding := NULL;
        END;
    END IF;

    -- 4. RRF Fusion: BM25 + Vector (when available)
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
            AND (be.agent_id IS NULL OR be.agent_id = p_agent_id)
        ORDER BY paradedb.score(be.id) DESC
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
    -- RRF: combine BM25 and vector ranks
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
            WHEN v_query_embedding IS NOT NULL THEN 'rrf'::TEXT
            ELSE 'bm25'::TEXT
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
-- 6. Helper: get query embedding (synchronous, for retrieve_bee)
-- =============================================================================

CREATE OR REPLACE FUNCTION meclaw.get_query_embedding(p_text TEXT)
RETURNS vector(1536) AS $fn$
    import json
    import requests

    plan_prov = plpy.prepare("SELECT base_url, api_key, config FROM meclaw.llm_providers WHERE id = $1", ["text"])
    prov = plan_prov.execute(["embedding-openrouter"])
    if not prov:
        return None

    base_url = prov[0]["base_url"]
    api_key = prov[0]["api_key"]
    config = json.loads(prov[0]["config"]) if prov[0]["config"] else {}
    model = config.get("model", "openai/text-embedding-3-small")

    text = p_text[:8000]

    try:
        resp = requests.post(
            base_url,
            headers={"Authorization": f"Bearer {api_key}", "Content-Type": "application/json", "HTTP-Referer": "https://meclaw.ai", "X-Title": "MeClaw"},
            json={"model": model, "input": text},
            timeout=30
        )
        resp.raise_for_status()
        data = resp.json()
        embedding = data["data"][0]["embedding"]
        return "[" + ",".join(str(x) for x in embedding) + "]"
    except Exception as e:
        plpy.warning(f"get_query_embedding failed: {e}")
        return None
$fn$ LANGUAGE plpython3u;

COMMENT ON FUNCTION meclaw.compute_embedding IS
'Compute embedding for a single brain_event via OpenRouter embedding API.';

COMMENT ON FUNCTION meclaw.compute_embeddings_batch IS
'Batch compute embeddings for brain_events without embeddings. Rate-limited.';

COMMENT ON FUNCTION meclaw.get_query_embedding IS
'Get embedding vector for a query string. Used by retrieve_bee for vector search.';

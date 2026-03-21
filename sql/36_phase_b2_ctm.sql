-- MeClaw Phase B2: CTM Retrieval Integration
-- Date: 2026-03-21
-- Ref: docs/BRAIN.md (CTM Retrieval, Tick-Based, Adaptive Compute)
-- Note: Drops old 3-param retrieve_bee (replaced by 4-param with p_ctm_enabled)
--
-- Integrates CTM Embedding-Drift as optional Stage 4 in retrieve_bee.
-- Pipeline overview:
--   CTM=OFF: Stage 1 (BM25) + Stage 2 (Vector RRF) + Stage 3 (AGE Graph)
--   CTM=ON:  Stage 1 (BM25) + Stage 2 (CTM Drift) → merged RRF final ranking
--
-- CTM uses STORED brain_events.embedding values → ZERO extra API calls beyond
-- the one get_query_embedding call that Stage 2 already does.
-- CTM activates only when p_ctm_enabled=TRUE. Default is FALSE.

-- =============================================================================
-- 1. CTM Drift Helper (plpython3u, uses stored embeddings only)
-- =============================================================================

CREATE OR REPLACE FUNCTION meclaw.ctm_drift_retrieve(
    p_agent_id TEXT,
    p_query_embedding vector(1536),
    p_anchor_event_ids UUID[],
    p_channel_ids UUID[],
    p_max_ticks INT DEFAULT 2,
    p_entropy_threshold FLOAT DEFAULT 0.35,
    p_limit INT DEFAULT 5
) RETURNS TABLE (
    event_id UUID,
    content TEXT,
    score FLOAT,
    source TEXT,
    channel_id UUID,
    reward FLOAT,
    created_at TIMESTAMPTZ,
    ticks_used INT
) AS $fn$
    import math

    if not p_query_embedding or not p_channel_ids:
        return []

    def compute_entropy(scores):
        """Shannon entropy (normalized). Low = converged, High = ambiguous."""
        if not scores or sum(scores) == 0:
            return 1.0
        total = sum(scores)
        probs = [s / total for s in scores if s > 0]
        n = len(probs)
        if n <= 1:
            return 0.0
        return -sum(p * math.log2(p) for p in probs if p > 0) / math.log2(n)

    def blend_embeddings(base, others, alpha=0.25):
        """Drift base toward average of others. alpha = drift strength."""
        if not others:
            return base
        dim = len(base)
        avg = [sum(o[i] for o in others) / len(others) for i in range(dim)]
        return [base[i] * (1 - alpha) + avg[i] * alpha for i in range(dim)]

    def parse_vector(v):
        """Parse plpgsql vector type to Python list of floats."""
        if v is None:
            return None
        if isinstance(v, list):
            return v
        s = str(v).strip()
        if s.startswith('[') and s.endswith(']'):
            return [float(x) for x in s[1:-1].split(',') if x.strip()]
        return None

    # Parse initial query embedding
    query_vec = parse_vector(p_query_embedding)
    if not query_vec:
        return []

    channel_ids_str = "{" + ",".join(str(c) for c in p_channel_ids) + "}"

    # Fetch stored embeddings of anchor events (drift seed for Tick 0)
    if p_anchor_event_ids:
        anchor_ids_str = "{" + ",".join(str(x) for x in p_anchor_event_ids) + "}"
        plan_anchors = plpy.prepare("""
            SELECT embedding
            FROM meclaw.brain_events
            WHERE id = ANY($1::uuid[]) AND embedding IS NOT NULL
            LIMIT 5
        """, ["text"])
        anchor_rows = plan_anchors.execute([anchor_ids_str])
        anchor_embeddings = [parse_vector(r["embedding"]) for r in anchor_rows
                             if r["embedding"] is not None]
        # Pre-drift the query embedding toward anchor embeddings (Tick 0)
        if anchor_embeddings:
            query_vec = blend_embeddings(query_vec, anchor_embeddings, alpha=0.25)

    best_results = []
    ticks_used = 0

    plan_search = plpy.prepare("""
        SELECT
            be.id AS event_id,
            be.content,
            1 - (be.embedding <=> $1::vector) AS vec_score,
            be.channel_id,
            COALESCE(be.reward, 0.0) AS reward,
            be.created_at,
            be.embedding
        FROM meclaw.brain_events be
        WHERE be.embedding IS NOT NULL
            AND be.channel_id = ANY($2::uuid[])
            AND (be.agent_id IS NULL OR be.agent_id = $3)
        ORDER BY be.embedding <=> $1::vector
        LIMIT 20
    """, ["text", "text", "text"])

    for tick in range(p_max_ticks):
        ticks_used = tick + 1

        # Build vector string for psql
        vec_str = "[" + ",".join(f"{x:.8f}" for x in query_vec) + "]"

        rows = plan_search.execute([vec_str, channel_ids_str, p_agent_id])
        if not rows:
            break

        best_results = rows
        scores = [float(r["vec_score"]) for r in rows[:p_limit]]

        # Check convergence
        entropy = compute_entropy(scores)
        if entropy < p_entropy_threshold:
            break  # Converged — early exit

        # Drift: blend toward top-3 results' stored embeddings
        top_embeddings = []
        for r in rows[:3]:
            emb = parse_vector(r["embedding"])
            if emb:
                top_embeddings.append(emb)

        if top_embeddings:
            query_vec = blend_embeddings(query_vec, top_embeddings, alpha=0.20)
        else:
            break  # No embeddings to drift toward

    # Build output
    output = []
    for r in best_results[:p_limit]:
        output.append((
            r["event_id"],
            r["content"],
            float(r["vec_score"]),
            f"ctm:tick{ticks_used}",
            r["channel_id"],
            float(r["reward"]),
            r["created_at"],
            ticks_used
        ))
    return output
$fn$ LANGUAGE plpython3u;

COMMENT ON FUNCTION meclaw.ctm_drift_retrieve IS
'CTM drift: iteratively blends query embedding toward top results using STORED
brain_events.embedding values. Zero extra API calls. Converges when score
entropy drops below threshold. Called by retrieve_bee when p_ctm_enabled=true.
Pre-drift uses anchor event embeddings from Stage 1 BM25 top results.';

-- =============================================================================
-- 2. Drop old 3-param retrieve_bee (replaced by 4-param, fully backward-compat)
-- =============================================================================
DROP FUNCTION IF EXISTS meclaw.retrieve_bee(text, text, integer);

-- =============================================================================
-- 3. retrieve_bee v4: optional CTM Stage as p_ctm_enabled flag
--    When FALSE (default): identical Stage 1-3 code path, zero overhead.
--    When TRUE: BM25 + CTM Drift merged via RRF, skips graph expansion.
-- =============================================================================

CREATE OR REPLACE FUNCTION meclaw.retrieve_bee(
    p_agent_id TEXT,
    p_query TEXT,
    p_limit INT DEFAULT 5,
    p_ctm_enabled BOOLEAN DEFAULT FALSE
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

    -- 2. Detect temporal query (for TEMPORAL-edge-first traversal, Stage 1-3 only)
    v_is_temporal_query := (
        lower(p_query) ~ '\m(first|before|after|when|how many days|earliest|latest|last time|since|until|between)\M'
    );

    -- 3. Get reference timestamp for recency scoring
    SELECT clock_timestamp(), COALESCE(MAX(be2.seq), 1)
    INTO v_now, v_max_seq
    FROM meclaw.brain_events be2
    WHERE be2.channel_id = ANY(v_channel_ids);

    -- =========================================================================
    -- CTM ENABLED PATH: BM25 + CTM Drift → merged RRF final ranking
    -- =========================================================================
    IF p_ctm_enabled THEN
        -- Get query embedding (same 1 API call as Stage 2 in normal path)
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

        -- Stage 1 (BM25) for CTM anchors: get top-5 BM25 results as seed
        -- Stage 2 (CTM): drift query embedding toward BM25 anchors, then iterate
        RETURN QUERY
        WITH bm25_results AS (
            SELECT
                be.id AS event_id,
                be.content,
                paradedb.score(be.id) AS bm25_score,
                be.channel_id,
                COALESCE(be.reward, 0.0) AS reward,
                COALESCE(be.novelty, 0.0) AS novelty,
                be.created_at,
                ROW_NUMBER() OVER (ORDER BY paradedb.score(be.id) DESC) AS bm25_rank
            FROM meclaw.brain_events be
            WHERE be.content @@@ p_query
                AND be.channel_id = ANY(v_channel_ids)
                AND (be.agent_id IS NULL OR be.agent_id = p_agent_id)
            ORDER BY paradedb.score(be.id) DESC
            LIMIT 20
        ),
        bm25_anchors AS (
            -- Top BM25 results used as CTM drift seed
            SELECT array_agg(b.event_id ORDER BY b.bm25_rank) AS anchor_ids
            FROM bm25_results b
            WHERE b.bm25_rank <= 5
        ),
        ctm_results AS (
            -- CTM drift: zero extra API calls, uses stored embeddings
            SELECT
                cd.event_id,
                cd.content,
                cd.score AS ctm_score,
                cd.source,
                cd.channel_id,
                cd.reward,
                cd.created_at,
                ROW_NUMBER() OVER (ORDER BY cd.score DESC) AS ctm_rank
            FROM bm25_anchors,
            LATERAL meclaw.ctm_drift_retrieve(
                p_agent_id,
                v_query_embedding,
                anchor_ids,
                v_channel_ids,
                2,    -- max 2 drift ticks (0 extra API calls)
                0.35, -- entropy convergence threshold
                p_limit * 4
            ) cd
        ),
        -- Merge BM25 + CTM via RRF
        merged_rrf AS (
            SELECT
                COALESCE(b.event_id, c.event_id) AS event_id,
                COALESCE(b.content, c.content) AS content,
                COALESCE(b.channel_id, c.channel_id) AS channel_id,
                COALESCE(b.reward, c.reward, 0.0) AS reward,
                COALESCE(b.created_at, c.created_at) AS created_at,
                COALESCE(1.0 / (60 + b.bm25_rank), 0)::FLOAT
                    + COALESCE(1.0 / (60 + c.ctm_rank), 0)::FLOAT AS rrf_score,
                COALESCE(b.novelty, 0.0) AS novelty,
                COALESCE(c.source, 'bm25') AS ctm_source
            FROM bm25_results b
            FULL OUTER JOIN ctm_results c ON b.event_id = c.event_id
        ),
        -- Final 4-signal ranking (recency + reward + novelty + RRF)
        final_scored AS (
            SELECT
                m.event_id,
                m.content,
                m.channel_id,
                m.reward,
                m.created_at,
                m.ctm_source AS source,
                m.rrf_score * 0.50
                    + EXP(-EXTRACT(EPOCH FROM (v_now - m.created_at)) / (7 * 86400.0)) * 0.20
                    + LEAST(1.0, GREATEST(0.0, (m.reward + 10.0) / 20.0)) * 0.15
                    + COALESCE(m.novelty, 0.0) * 0.15
                AS final_score
            FROM merged_rrf m
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

        RETURN;
    END IF;

    -- =========================================================================
    -- NORMAL PATH (default, CTM disabled): Stage 1-3 unchanged, zero overhead
    -- =========================================================================

    -- Check embeddings and compute query embedding for vector search
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

    RETURN QUERY
    WITH bm25_results AS (
        SELECT
            be.id AS event_id,
            be.content,
            paradedb.score(be.id) AS bm25_score,
            be.channel_id,
            COALESCE(be.reward, 0.0) AS reward,
            COALESCE(be.novelty, 0.0) AS novelty,
            be.created_at,
            be.seq,
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
    anchor_ids_arr AS (
        SELECT array_agg(a.event_id::TEXT) AS ids FROM anchors a
    ),
    all_graph_neighbors AS (
        SELECT ge.event_id, MIN(ge.graph_hop) AS min_hop
        FROM anchor_ids_arr,
        LATERAL meclaw.graph_expand_events(ids, v_is_temporal_query) ge
        GROUP BY ge.event_id
    ),
    -- Fetch brain_events data for graph neighbors
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
    -- Graph-expanded candidates
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
    -- 6-Signal Ranking
    scored AS (
        SELECT
            c.event_id,
            c.content,
            c.channel_id,
            c.reward,
            c.created_at,
            EXP(-EXTRACT(EPOCH FROM (v_now - c.created_at)) / (7 * 86400.0)) AS recency_score,
            LEAST(1.0, GREATEST(0.0, (c.reward + 10.0) / 20.0)) AS reward_score,
            COALESCE(c.novelty, 0.0) AS novelty_score,
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
    -- Fallback: pure RRF without graph (if AGE fails etc.)
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

COMMENT ON FUNCTION meclaw.retrieve_bee(text, text, integer, boolean) IS
'Agent-level memory retrieval with 4-stage pipeline:
CTM=OFF (default): Stage 1 (BM25) + Stage 2 (Vector RRF) + Stage 3 (AGE Graph)
CTM=ON: Stage 1 (BM25) + Stage 2 (CTM Drift) → merged RRF final ranking
CTM uses stored brain_events.embedding - ZERO extra API calls.
Pass p_ctm_enabled=TRUE to activate CTM mode.';

-- =============================================================================
-- 4. Benchmark helper for CTM comparison
-- =============================================================================

CREATE OR REPLACE FUNCTION meclaw.benchmark_retrieve(
    p_agent_id TEXT,
    p_query TEXT,
    p_limit INT DEFAULT 3
) RETURNS TABLE (
    mode TEXT,
    rank INT,
    score FLOAT,
    source TEXT,
    content_preview TEXT
) AS $$
DECLARE
    v_rank INT;
    r RECORD;
BEGIN
    -- Without CTM
    v_rank := 0;
    FOR r IN
        SELECT r2.score, r2.source, r2.content
        FROM meclaw.retrieve_bee(p_agent_id, p_query, p_limit, FALSE) r2
        ORDER BY r2.score DESC
    LOOP
        v_rank := v_rank + 1;
        mode := 'no_ctm';
        rank := v_rank;
        score := r.score;
        source := r.source;
        content_preview := LEFT(r.content, 80);
        RETURN NEXT;
    END LOOP;

    v_rank := 0;
    -- With CTM
    FOR r IN
        SELECT r2.score, r2.source, r2.content
        FROM meclaw.retrieve_bee(p_agent_id, p_query, p_limit, TRUE) r2
        ORDER BY r2.score DESC
    LOOP
        v_rank := v_rank + 1;
        mode := 'with_ctm';
        rank := v_rank;
        score := r.score;
        source := r.source;
        content_preview := LEFT(r.content, 80);
        RETURN NEXT;
    END LOOP;
END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION meclaw.benchmark_retrieve IS
'Compare retrieve_bee results with and without CTM for the same query.
Shows side-by-side ranking, scores, and source stage for each result.';

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
    -- ==========================================================================
    -- E1: 6-Signal Weighted Ranking (BRAIN.md)
    -- ==========================================================================
    w_similarity   CONSTANT FLOAT := 0.25;
    w_reward       CONSTANT FLOAT := 0.25;
    w_novelty      CONSTANT FLOAT := 0.15;
    w_recency      CONSTANT FLOAT := 0.10;
    w_personality  CONSTANT FLOAT := 0.15;
    w_graph_dist   CONSTANT FLOAT := 0.10;

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

        -- Stage 1 (BM25 + facts_text) for CTM anchors: get top-5 BM25 results as seed
        -- Stage 2 (CTM): drift query embedding toward BM25 anchors, then iterate
        RETURN QUERY
        WITH content_bm25 AS (
            SELECT
                be.id AS event_id,
                be.content,
                paradedb.score(be.id) AS bm25_score,
                be.channel_id,
                COALESCE(be.reward, 0.0) AS reward,
                COALESCE(be.novelty, 0.0) AS novelty,
                be.created_at
            FROM meclaw.brain_events be
            WHERE be.content @@@ p_query
                AND be.channel_id = ANY(v_channel_ids)
                AND (be.agent_id IS NULL OR be.agent_id = p_agent_id)
            ORDER BY paradedb.score(be.id) DESC
            LIMIT 20
        ),
        facts_bm25 AS (
            SELECT
                be.id AS event_id,
                be.content,
                paradedb.score(be.id) AS bm25_score,
                be.channel_id,
                COALESCE(be.reward, 0.0) AS reward,
                COALESCE(be.novelty, 0.0) AS novelty,
                be.created_at
            FROM meclaw.brain_events be
            WHERE be.facts_text @@@ p_query
                AND be.channel_id = ANY(v_channel_ids)
                AND (be.agent_id IS NULL OR be.agent_id = p_agent_id)
            ORDER BY paradedb.score(be.id) DESC
            LIMIT 20
        ),
        bm25_results AS (
            SELECT
                COALESCE(c.event_id, f.event_id) AS event_id,
                COALESCE(c.content, f.content) AS content,
                COALESCE(c.bm25_score, 0) + COALESCE(f.bm25_score, 0) AS bm25_score,
                COALESCE(c.channel_id, f.channel_id) AS channel_id,
                COALESCE(c.reward, f.reward, 0.0) AS reward,
                COALESCE(c.novelty, f.novelty, 0.0) AS novelty,
                COALESCE(c.created_at, f.created_at) AS created_at,
                ROW_NUMBER() OVER (
                    ORDER BY COALESCE(c.bm25_score, 0) + COALESCE(f.bm25_score, 0) DESC
                ) AS bm25_rank
            FROM content_bm25 c
            FULL OUTER JOIN facts_bm25 f ON c.event_id = f.event_id
        ),
        bm25_anchors AS (
            -- Top BM25+facts results used as CTM drift seed
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
        -- E1: 6-Signal Weighted Ranking (CTM path uses rrf_score as similarity proxy)
        final_scored AS (
            SELECT
                m.event_id,
                m.content,
                m.channel_id,
                m.reward,
                m.created_at,
                m.ctm_source AS source,
                -- similarity: normalize RRF score to ~[0,1] (max theoretical RRF for 2 sources = 1/61+1/61 ≈ 0.033)
                LEAST(1.0, m.rrf_score / 0.033)          * w_similarity
                    + LEAST(1.0, GREATEST(0.0, (COALESCE(m.reward, 0.0) + 10.0) / 20.0)) * w_reward
                    + LEAST(1.0, GREATEST(0.0, COALESCE(m.novelty, 0.0)))                * w_novelty
                    + 1.0 / (1.0 + EXTRACT(EPOCH FROM (v_now - m.created_at)) / 86400.0) * w_recency
                    + COALESCE(meclaw.personality_fit(p_agent_id, NULL::TEXT, m.content), 0.5) * w_personality
                    + 1.0                                                                  * w_graph_dist  -- CTM: no graph hops
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

    -- Normal path: BM25 across content AND facts_text (C1 + B2 merged)
    RETURN QUERY
    WITH content_bm25_n AS (
        SELECT
            be.id AS event_id,
            be.content,
            paradedb.score(be.id) AS bm25_score,
            be.channel_id,
            COALESCE(be.reward, 0.0) AS reward,
            COALESCE(be.novelty, 0.0) AS novelty,
            be.created_at,
            be.seq
        FROM meclaw.brain_events be
        WHERE be.content @@@ p_query
            AND be.channel_id = ANY(v_channel_ids)
            AND (be.agent_id IS NULL OR be.agent_id = p_agent_id)
        ORDER BY paradedb.score(be.id) DESC
        LIMIT 20
    ),
    facts_bm25_n AS (
        SELECT
            be.id AS event_id,
            be.content,
            paradedb.score(be.id) AS bm25_score,
            be.channel_id,
            COALESCE(be.reward, 0.0) AS reward,
            COALESCE(be.novelty, 0.0) AS novelty,
            be.created_at,
            be.seq
        FROM meclaw.brain_events be
        WHERE be.facts_text @@@ p_query
            AND be.channel_id = ANY(v_channel_ids)
            AND (be.agent_id IS NULL OR be.agent_id = p_agent_id)
        ORDER BY paradedb.score(be.id) DESC
        LIMIT 20
    ),
    bm25_results AS (
        SELECT
            COALESCE(c.event_id, f.event_id) AS event_id,
            COALESCE(c.content, f.content) AS content,
            COALESCE(c.bm25_score, 0) + COALESCE(f.bm25_score, 0) AS bm25_score,
            COALESCE(c.channel_id, f.channel_id) AS channel_id,
            COALESCE(c.reward, f.reward, 0.0) AS reward,
            COALESCE(c.novelty, f.novelty, 0.0) AS novelty,
            COALESCE(c.created_at, f.created_at) AS created_at,
            COALESCE(c.seq, f.seq, 0) AS seq,
            ROW_NUMBER() OVER (
                ORDER BY COALESCE(c.bm25_score, 0) + COALESCE(f.bm25_score, 0) DESC
            ) AS bm25_rank
        FROM content_bm25_n c
        FULL OUTER JOIN facts_bm25_n f ON c.event_id = f.event_id
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
    -- Graph-expanded candidates (with graph_hop for distance signal)
    graph_rrf AS (
        SELECT
            ge.event_id,
            ge.content,
            1.0 / (60 + 20 + ge.graph_hop * 10) AS rrf_score,
            ge.channel_id,
            ge.reward,
            ge.novelty,
            ge.created_at,
            ge.seq,
            ge.graph_hop
        FROM graph_events ge
        WHERE ge.event_id NOT IN (SELECT s1b.event_id FROM stage1_rrf s1b)
    ),
    -- Merge stage1 + graph expansion (stage1 events get hop=0 = "direct match")
    all_candidates AS (
        SELECT s1.event_id, s1.content, s1.rrf_score, s1.channel_id, s1.reward, s1.novelty, s1.created_at, s1.seq,
               0 AS graph_hop
        FROM stage1_rrf s1
        UNION ALL
        SELECT gr.event_id, gr.content, gr.rrf_score, gr.channel_id, gr.reward, gr.novelty, gr.created_at, gr.seq,
               gr.graph_hop
        FROM graph_rrf gr
    ),
    -- Retrieve per-event cosine similarity for similarity signal
    candidate_similarity AS (
        SELECT
            be.id AS event_id,
            CASE
                WHEN v_query_embedding IS NOT NULL AND be.embedding IS NOT NULL
                THEN LEAST(1.0, GREATEST(0.0, 1.0 - (be.embedding <=> v_query_embedding)))
                ELSE 0.5  -- neutral when no embedding
            END AS sim_score
        FROM meclaw.brain_events be
        WHERE be.id IN (SELECT ac.event_id FROM all_candidates ac)
    ),
    -- E1: 6-Signal Weighted Ranking
    -- Signals normalized to [0,1]:
    --   similarity  : cosine similarity from pgvector (already 0-1)
    --   reward      : COALESCE(reward, 0) rescaled from [-10,10] → [0,1]
    --   novelty     : COALESCE(novelty, 0) (already 0-1 stored)
    --   recency     : 1/(1 + age_days) decay
    --   personality : meclaw.personality_fit(agent, NULL, content) (returns 0-1)
    --   graph_dist  : 1/(1 + hop_count)
    scored AS (
        SELECT
            c.event_id,
            c.content,
            c.channel_id,
            c.reward,
            c.created_at,
            c.rrf_score,
            -- similarity signal
            COALESCE(cs.sim_score, 0.5) AS sig_similarity,
            -- reward signal: rescale [-10,10] → [0,1]
            LEAST(1.0, GREATEST(0.0, (COALESCE(c.reward, 0.0) + 10.0) / 20.0)) AS sig_reward,
            -- novelty signal
            LEAST(1.0, GREATEST(0.0, COALESCE(c.novelty, 0.0))) AS sig_novelty,
            -- recency signal: 1 / (1 + age_in_days)
            1.0 / (1.0 + EXTRACT(EPOCH FROM (v_now - c.created_at)) / 86400.0) AS sig_recency,
            -- personality_fit: (agent_id, user_id=NULL, content)
            COALESCE(
                meclaw.personality_fit(p_agent_id, NULL::TEXT, c.content),
                0.5
            ) AS sig_personality,
            -- graph distance signal: 1 / (1 + hops)
            1.0 / (1.0 + c.graph_hop::FLOAT) AS sig_graph_dist
        FROM all_candidates c
        LEFT JOIN candidate_similarity cs ON cs.event_id = c.event_id
    ),
    final_scored AS (
        SELECT
            s.event_id,
            s.content,
            s.channel_id,
            s.reward,
            s.created_at,
            -- Weighted sum of 6 signals (BRAIN.md weights)
            s.sig_similarity  * w_similarity
            + s.sig_reward    * w_reward
            + s.sig_novelty   * w_novelty
            + s.sig_recency   * w_recency
            + s.sig_personality * w_personality
            + s.sig_graph_dist  * w_graph_dist
            AS final_score,
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
    -- Fallback: pure RRF without graph (if AGE fails etc.) — includes facts_text BM25
    RETURN QUERY
    WITH content_bm25_fb AS (
        SELECT
            be.id AS event_id,
            be.content,
            paradedb.score(be.id) AS bm25_score,
            be.channel_id,
            COALESCE(be.reward, 0.0) AS reward,
            be.created_at
        FROM meclaw.brain_events be
        WHERE be.content @@@ p_query
            AND be.channel_id = ANY(v_channel_ids)
            AND (be.agent_id IS NULL OR be.agent_id = p_agent_id)
        ORDER BY paradedb.score(be.id) DESC
        LIMIT 20
    ),
    facts_bm25_fb AS (
        SELECT
            be.id AS event_id,
            be.content,
            paradedb.score(be.id) AS bm25_score,
            be.channel_id,
            COALESCE(be.reward, 0.0) AS reward,
            be.created_at
        FROM meclaw.brain_events be
        WHERE be.facts_text @@@ p_query
            AND be.channel_id = ANY(v_channel_ids)
            AND (be.agent_id IS NULL OR be.agent_id = p_agent_id)
        ORDER BY paradedb.score(be.id) DESC
        LIMIT 20
    ),
    bm25_results AS (
        SELECT
            COALESCE(c.event_id, f.event_id) AS event_id,
            COALESCE(c.content, f.content) AS content,
            COALESCE(c.bm25_score, 0) + COALESCE(f.bm25_score, 0) AS bm25_score,
            COALESCE(c.channel_id, f.channel_id) AS channel_id,
            COALESCE(c.reward, f.reward, 0.0) AS reward,
            COALESCE(c.created_at, f.created_at) AS created_at,
            ROW_NUMBER() OVER (
                ORDER BY COALESCE(c.bm25_score, 0) + COALESCE(f.bm25_score, 0) DESC
            ) AS bm25_rank
        FROM content_bm25_fb c
        FULL OUTER JOIN facts_bm25_fb f ON c.event_id = f.event_id
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
'Agent-level memory retrieval with E1: 6-Signal Weighted Ranking (BRAIN.md).
CTM=OFF (default): Stage 1 (BM25+facts_text RRF) + Stage 2 (Vector RRF) + Stage 3 (AGE Graph)
CTM=ON: Stage 1 (BM25+facts_text RRF) + Stage 2 (CTM Drift) → merged RRF final ranking
Final score = similarity*0.25 + reward*0.25 + novelty*0.15 + recency*0.10 + personality_fit*0.15 + graph_distance*0.10
CTM uses stored brain_events.embedding - ZERO extra API calls.
facts_text search (C1) integrated in all paths via FULL OUTER JOIN RRF.
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

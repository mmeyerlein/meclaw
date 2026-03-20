-- MeClaw v0.1.0 — Phase 5: CTM-Style Iterative Retrieval + Multi-Agent
-- Date: 2026-03-20
-- Ref: docs/BRAIN.md (CTM Retrieval, Tick-Based, Adaptive Compute)
--
-- Continuous Thought Machine inspired retrieval:
-- Query embedding drifts toward relevant concept region over multiple ticks.
-- Simple queries: 1 tick. Complex/ambiguous: 2-3 ticks.
-- Convergence = entropy of top results drops below threshold.

-- =============================================================================
-- 1. CTM-Style Iterative Retrieval
-- =============================================================================

CREATE OR REPLACE FUNCTION meclaw.ctm_retrieve(
    p_agent_id TEXT,
    p_query TEXT,
    p_max_ticks INT DEFAULT 3,
    p_limit INT DEFAULT 5,
    p_entropy_threshold FLOAT DEFAULT 0.3
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
    import json
    import math
    import requests

    # Get agent's channels
    plan_ch = plpy.prepare("""
        SELECT array_agg(channel_id) as channel_ids
        FROM meclaw.agent_channels WHERE agent_id = $1
    """, ["text"])
    ch_result = plan_ch.execute([p_agent_id])
    if not ch_result or not ch_result[0]["channel_ids"]:
        return []

    # Get embedding provider
    plan_prov = plpy.prepare("SELECT base_url, api_key, config FROM meclaw.llm_providers WHERE id = $1", ["text"])
    prov = plan_prov.execute(["embedding-openrouter"])
    if not prov:
        # Fallback to standard retrieve_bee
        plan_fb = plpy.prepare("""
            SELECT event_id, content, score, source, channel_id, reward, created_at
            FROM meclaw.retrieve_bee($1, $2, $3)
        """, ["text", "text", "int4"])
        results = plan_fb.execute([p_agent_id, p_query, p_limit])
        return [(r["event_id"], r["content"], r["score"], r["source"], 
                 r["channel_id"], r["reward"], r["created_at"], 1) for r in results]

    api_url = prov[0]["base_url"]
    api_key = prov[0]["api_key"]
    config = json.loads(prov[0]["config"]) if prov[0]["config"] else {}
    model = config.get("model", "openai/text-embedding-3-small")

    def get_embedding(text):
        resp = requests.post(
            api_url,
            headers={"Authorization": f"Bearer {api_key}", "Content-Type": "application/json"},
            json={"model": model, "input": text[:8000]},
            timeout=30
        )
        resp.raise_for_status()
        return resp.json()["data"][0]["embedding"]

    def compute_entropy(scores):
        """Shannon entropy of score distribution — low = converged, high = ambiguous"""
        if not scores or sum(scores) == 0:
            return 1.0
        total = sum(scores)
        probs = [s / total for s in scores if s > 0]
        return -sum(p * math.log2(p) for p in probs if p > 0) / max(math.log2(len(probs)), 1)

    def blend_embeddings(base, targets, alpha=0.3):
        """Drift base embedding toward target embeddings"""
        if not targets:
            return base
        avg_target = [sum(t[i] for t in targets) / len(targets) for i in range(len(base))]
        return [base[i] * (1 - alpha) + avg_target[i] * alpha for i in range(len(base))]

    # Initial query embedding
    try:
        query_vec = get_embedding(p_query)
    except Exception as e:
        plpy.warning(f"ctm_retrieve: embedding failed: {e}")
        plan_fb = plpy.prepare("""
            SELECT event_id, content, score, source, channel_id, reward, created_at
            FROM meclaw.retrieve_bee($1, $2, $3)
        """, ["text", "text", "int4"])
        results = plan_fb.execute([p_agent_id, p_query, p_limit])
        return [(r["event_id"], r["content"], r["score"], r["source"],
                 r["channel_id"], r["reward"], r["created_at"], 1) for r in results]

    best_results = []
    ticks_used = 0

    for tick in range(p_max_ticks):
        ticks_used = tick + 1

        # Vector search with current (drifted) embedding
        vec_str = "[" + ",".join(str(x) for x in query_vec) + "]"
        
        plan_search = plpy.prepare("""
            WITH vec_results AS (
                SELECT be.id AS event_id, be.content,
                    1 - (be.embedding <=> $1::vector) AS vec_score,
                    be.channel_id, COALESCE(be.reward, 0.0) AS reward,
                    be.created_at, be.novelty
                FROM meclaw.brain_events be
                WHERE be.embedding IS NOT NULL
                    AND be.channel_id = ANY($2::uuid[])
                    AND (be.agent_id IS NULL OR be.agent_id = $3)
                ORDER BY be.embedding <=> $1::vector
                LIMIT 20
            ),
            scored AS (
                SELECT v.*,
                    (v.vec_score * 0.30
                     + v.reward * 0.25
                     + COALESCE(v.novelty, 0) * 0.15
                     + (1.0 / (1 + EXTRACT(EPOCH FROM (clock_timestamp() - v.created_at)) / 86400.0)) * 0.10
                     + meclaw.personality_fit($3, 
                         (SELECT e.id FROM meclaw.entities e WHERE e.entity_type = 'person' LIMIT 1),
                         v.content) * 0.15
                     + CASE WHEN v.reward > 0 THEN 0.05 ELSE 0.0 END
                    ) AS final_score
                FROM vec_results v
            )
            SELECT event_id, content, final_score AS score, channel_id, reward, created_at
            FROM scored ORDER BY final_score DESC LIMIT $4
        """, ["text", "text", "text", "int4"])

        results = plan_search.execute([vec_str, 
            "{" + ",".join(str(c) for c in ch_result[0]["channel_ids"]) + "}",
            p_agent_id, p_limit * 2])

        if not results:
            break

        best_results = results
        scores = [float(r["score"]) for r in results[:p_limit]]

        # Check convergence
        entropy = compute_entropy(scores)
        if entropy < p_entropy_threshold:
            break  # Converged!

        # Drift: blend query embedding toward top results' content embeddings
        top_contents = [r["content"] for r in results[:3]]
        try:
            top_embeddings = [get_embedding(c) for c in top_contents]
            query_vec = blend_embeddings(query_vec, top_embeddings, alpha=0.3)
        except:
            break  # If embedding fails, stop drifting

    # Log
    plan_log = plpy.prepare("""
        INSERT INTO meclaw.events (bee_type, event, payload)
        VALUES ('ctm_retrieve', 'retrieval_complete', $1::jsonb)
    """, ["text"])
    plan_log.execute([json.dumps({
        "agent_id": p_agent_id,
        "query": p_query[:100],
        "ticks_used": ticks_used,
        "max_ticks": p_max_ticks,
        "results_count": len(best_results)
    })])

    # Return top results
    output = []
    for r in best_results[:p_limit]:
        output.append((
            r["event_id"], r["content"], float(r["score"]),
            f"ctm:tick{ticks_used}", r["channel_id"],
            float(r["reward"]), r["created_at"], ticks_used
        ))
    return output
$fn$ LANGUAGE plpython3u;

-- =============================================================================
-- 2. Multi-Agent Memory Sharing
-- =============================================================================

-- Grant an agent access to another agent's shared channels
CREATE OR REPLACE FUNCTION meclaw.share_channel(
    p_owner_agent TEXT,
    p_target_agent TEXT,
    p_channel_id UUID,
    p_role TEXT DEFAULT 'observer'
) RETURNS VOID AS $$
BEGIN
    -- Verify owner actually owns/participates in this channel
    IF NOT EXISTS (
        SELECT 1 FROM meclaw.agent_channels
        WHERE agent_id = p_owner_agent AND channel_id = p_channel_id
    ) THEN
        RAISE EXCEPTION 'Agent % does not have access to channel %', p_owner_agent, p_channel_id;
    END IF;

    -- Grant access
    INSERT INTO meclaw.agent_channels (agent_id, channel_id, role, scope)
    VALUES (p_target_agent, p_channel_id, p_role, 'shared')
    ON CONFLICT (agent_id, channel_id) DO UPDATE SET
        role = EXCLUDED.role,
        scope = 'shared';
END;
$$ LANGUAGE plpgsql;

-- =============================================================================
-- 3. Agent Discovery (AIEOS-compatible)
-- =============================================================================

CREATE OR REPLACE FUNCTION meclaw.discover_agents(
    p_capability TEXT DEFAULT NULL,
    p_entity_type TEXT DEFAULT 'agent'
) RETURNS TABLE (
    id TEXT,
    canonical_name TEXT,
    entity_type TEXT,
    capabilities JSONB,
    neural_matrix JSONB,
    channel_count BIGINT
) AS $$
    SELECT e.id, e.canonical_name, e.entity_type, e.capabilities, e.neural_matrix,
        (SELECT COUNT(*) FROM meclaw.agent_channels ac WHERE ac.agent_id = e.id) AS channel_count
    FROM meclaw.entities e
    WHERE e.entity_type = p_entity_type
        AND (p_capability IS NULL OR e.capabilities @> jsonb_build_array(jsonb_build_object('name', p_capability)))
    ORDER BY e.canonical_name;
$$ LANGUAGE sql;

-- =============================================================================
-- 4. Cross-Agent Memory Query
-- =============================================================================

CREATE OR REPLACE FUNCTION meclaw.cross_agent_retrieve(
    p_requesting_agent TEXT,
    p_target_agent TEXT,
    p_query TEXT,
    p_limit INT DEFAULT 5
) RETURNS TABLE (
    event_id UUID,
    content TEXT,
    score FLOAT,
    source TEXT,
    channel_id UUID,
    reward FLOAT,
    created_at TIMESTAMPTZ
) AS $$
DECLARE
    v_shared_channels UUID[];
BEGIN
    -- Find channels shared between requesting and target agent
    SELECT array_agg(ac1.channel_id) INTO v_shared_channels
    FROM meclaw.agent_channels ac1
    JOIN meclaw.agent_channels ac2 ON ac1.channel_id = ac2.channel_id
    WHERE ac1.agent_id = p_requesting_agent
        AND ac2.agent_id = p_target_agent
        AND ac2.scope = 'shared';

    IF v_shared_channels IS NULL OR array_length(v_shared_channels, 1) = 0 THEN
        RAISE NOTICE 'No shared channels between % and %', p_requesting_agent, p_target_agent;
        RETURN;
    END IF;

    -- Use target agent's retrieve_bee but limited to shared channels
    RETURN QUERY
    SELECT rb.event_id, rb.content, rb.score, rb.source, rb.channel_id, rb.reward, rb.created_at
    FROM meclaw.retrieve_bee(p_target_agent, p_query, p_limit) rb
    WHERE rb.channel_id = ANY(v_shared_channels);
END;
$$ LANGUAGE plpgsql;

-- =============================================================================
-- 5. Ed25519 Key Generation Stub (for future AIEOS signing)
-- =============================================================================

CREATE OR REPLACE FUNCTION meclaw.generate_agent_keypair(p_agent_id TEXT)
RETURNS JSONB AS $fn$
    try:
        from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
        from cryptography.hazmat.primitives import serialization
        import base64
        
        private_key = Ed25519PrivateKey.generate()
        public_key = private_key.public_key()
        
        pub_bytes = public_key.public_bytes(
            serialization.Encoding.Raw, serialization.PublicFormat.Raw
        )
        pub_b64 = base64.b64encode(pub_bytes).decode()
        
        # Store public key on entity
        plan = plpy.prepare(
            "UPDATE meclaw.entities SET aieos_public_key = $1 WHERE id = $2",
            ["text", "text"]
        )
        plan.execute([pub_b64, p_agent_id])
        
        # Return both (private key should be stored securely!)
        priv_bytes = private_key.private_bytes(
            serialization.Encoding.Raw, serialization.PrivateFormat.Raw,
            serialization.NoEncryption()
        )
        
        return '{"public_key": "' + pub_b64 + '", "status": "generated"}'
    except ImportError:
        return '{"status": "cryptography library not installed"}'
    except Exception as e:
        return '{"status": "error", "error": "' + str(e) + '"}'
$fn$ LANGUAGE plpython3u;

COMMENT ON FUNCTION meclaw.ctm_retrieve IS
'CTM-style iterative retrieval. Query embedding drifts toward relevant concept
region over 1-3 ticks. Converges when entropy drops below threshold.
Adaptive compute: simple queries = 1 tick, complex = 2-3 ticks.';

COMMENT ON FUNCTION meclaw.share_channel IS
'Grant another agent access to a shared channel for cross-agent memory sharing.';

COMMENT ON FUNCTION meclaw.discover_agents IS
'AIEOS-compatible agent discovery. Find agents by capability or type.';

COMMENT ON FUNCTION meclaw.cross_agent_retrieve IS
'Query another agent memory but only through shared channels. Respects scoping.';

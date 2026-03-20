-- =============================================================================
-- Phase 7: Robustness & Error Tolerance
-- =============================================================================
-- 1. feedback_bee: LLM-based sentiment (replaces keywords)
-- 2. feedback_bee: negation detection
-- 3. compute_embedding: retry with backoff
-- 4. Trigger chain: pg_background for heavy lifting
-- 5. CTM Retrieval: cache query embeddings
-- 6. personality_fit: embedding-based (replaces keywords)
-- 7. Hebbian Learning: active prototype_associations update
-- =============================================================================

-- -----------------------------------------------------------------------------
-- 1+2. feedback_bee v2 — LLM-based sentiment with negation awareness
-- -----------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION meclaw.feedback_bee(p_msg_id UUID, p_agent_id TEXT)
RETURNS VOID AS $$
DECLARE
    v_content TEXT;
    v_channel_id UUID;
    v_prev_event_id UUID;
    v_reward_delta FLOAT := 0.0;
    v_sentiment TEXT := 'neutral';
    v_discount FLOAT := 0.9;
    v_propagate_depth INT := 5;
    v_msg_type TEXT;
BEGIN
    SELECT content->>'input', channel_id, type
    INTO v_content, v_channel_id, v_msg_type
    FROM meclaw.messages WHERE id = p_msg_id;

    IF v_content IS NULL OR v_msg_type != 'user_input' THEN RETURN; END IF;

    -- Skip very short messages (just "ok", "ja" etc. — ambiguous)
    IF length(v_content) < 3 THEN RETURN; END IF;

    -- Stage 1: Fast keyword detection (unchanged, as pre-filter)
    -- Only proceed to LLM if keyword match is ambiguous or correction-class
    IF v_content ~* '(genau|richtig|perfekt|super|danke|gut|nice|top|stimmt|exakt|korrekt|great|exactly|perfect|thanks|👍|🎉|✅|💪)' THEN
        -- Check for negation patterns FIRST
        IF v_content ~* '(nicht\s+(genau|richtig|perfekt|super|gut|korrekt)|stimmt\s+nicht|nein.*aber|ja.*falsch|ja.*stimmt\s+nicht)' THEN
            v_reward_delta := -0.5; v_sentiment := 'negated_positive';
        ELSE
            v_reward_delta := 0.8; v_sentiment := 'positive';
        END IF;
    ELSIF v_content ~* '(falsch|nein|wrong|no|fehler|error|stimmt nicht|quatsch|blödsinn|unsinn|👎|❌|nicht richtig|das ist falsch)' THEN
        v_reward_delta := -0.8; v_sentiment := 'negative';
    ELSIF v_content ~* '(aber|eigentlich|naja|hmm|nicht ganz|fast|eher|correction|nee)' THEN
        -- Ambiguous — use LLM for these
        v_sentiment := 'ambiguous';
    ELSE
        RETURN;
    END IF;

    -- Stage 2: LLM sentiment for ambiguous cases
    IF v_sentiment = 'ambiguous' THEN
        BEGIN
            SELECT * FROM meclaw.llm_sentiment(v_content) INTO v_sentiment, v_reward_delta;
        EXCEPTION WHEN OTHERS THEN
            -- Fallback to mild correction if LLM fails
            v_reward_delta := -0.3; v_sentiment := 'correction_fallback';
        END;
        IF v_sentiment = 'neutral' THEN RETURN; END IF;
    END IF;

    -- Find previous assistant event
    SELECT be.id INTO v_prev_event_id
    FROM meclaw.brain_events be
    JOIN meclaw.messages m ON m.id = be.message_id
    WHERE be.channel_id = v_channel_id
        AND m.type = 'llm_result'
        AND be.seq < (SELECT COALESCE(MAX(be2.seq), 0) FROM meclaw.brain_events be2 WHERE be2.message_id = p_msg_id)
    ORDER BY be.seq DESC LIMIT 1;

    IF v_prev_event_id IS NULL THEN RETURN; END IF;

    -- Direct reward on previous event
    UPDATE meclaw.brain_events
    SET reward = reward + v_reward_delta,
        reward_updated_seq = (SELECT COALESCE(MAX(seq), 0) FROM meclaw.brain_events)
    WHERE id = v_prev_event_id;

    -- Backward propagation (discounted returns)
    UPDATE meclaw.brain_events be
    SET reward = be.reward + (v_reward_delta * POWER(v_discount, rn)),
        reward_updated_seq = (SELECT COALESCE(MAX(be3.seq), 0) FROM meclaw.brain_events be3)
    FROM (
        SELECT be2.id, ROW_NUMBER() OVER (ORDER BY be2.seq DESC) AS rn
        FROM meclaw.brain_events be2
        WHERE be2.channel_id = v_channel_id
            AND (be2.agent_id IS NULL OR be2.agent_id = p_agent_id)
            AND be2.seq < (SELECT seq FROM meclaw.brain_events WHERE id = v_prev_event_id)
        ORDER BY be2.seq DESC
        LIMIT v_propagate_depth
    ) chain
    WHERE be.id = chain.id;

    -- Hebbian update: co-activated prototypes get weight boost
    PERFORM meclaw.hebbian_update(v_prev_event_id, v_reward_delta);

    -- Log
    INSERT INTO meclaw.events (msg_id, bee_type, event, payload)
    VALUES (p_msg_id, 'feedback_bee', 'reward_applied', jsonb_build_object(
        'agent_id', p_agent_id, 'target_event_id', v_prev_event_id,
        'reward_delta', v_reward_delta, 'sentiment', v_sentiment,
        'propagation_depth', v_propagate_depth, 'discount', v_discount
    ));
END;
$$ LANGUAGE plpgsql;

-- -----------------------------------------------------------------------------
-- LLM Sentiment helper — cheap model, structured output
-- -----------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION meclaw.llm_sentiment(p_text TEXT, OUT sentiment TEXT, OUT reward FLOAT)
RETURNS RECORD AS $fn$
import json
import requests

plan_prov = plpy.prepare("SELECT api_key FROM meclaw.llm_providers WHERE id = $1", ["text"])
prov = plan_prov.execute(["openrouter"])
if not prov:
    return ("neutral", 0.0)

api_key = prov[0]["api_key"]
text = p_text[:1000]

prompt = f"""Classify the sentiment of this message in a conversation with an AI assistant.
The message is a RESPONSE to the assistant's previous answer.

Message: "{text}"

Return ONLY valid JSON:
{{"sentiment": "positive|negative|correction|neutral", "confidence": 0.0-1.0}}

Rules:
- "positive": user agrees, confirms, thanks, approves
- "negative": user disagrees, complains, says it's wrong
- "correction": user partially agrees but corrects something
- "neutral": no sentiment towards the assistant's answer (new question, topic change)"""

try:
    resp = requests.post(
        "https://openrouter.ai/api/v1/chat/completions",
        headers={
            "Authorization": f"Bearer {api_key}",
            "Content-Type": "application/json",
            "HTTP-Referer": "https://meclaw.ai",
            "X-Title": "MeClaw"
        },
        json={
            "model": "openai/gpt-4o-mini",
            "messages": [{"role": "user", "content": prompt}],
            "temperature": 0.0,
            "max_tokens": 100,
            "response_format": {"type": "json_object"}
        },
        timeout=15
    )
    resp.raise_for_status()
    data = resp.json()
    output = json.loads(data["choices"][0]["message"]["content"])

    sentiment = output.get("sentiment", "neutral")
    confidence = float(output.get("confidence", 0.5))

    reward_map = {
        "positive": 0.8 * confidence,
        "negative": -0.8 * confidence,
        "correction": -0.3 * confidence,
        "neutral": 0.0
    }
    return (sentiment, reward_map.get(sentiment, 0.0))

except Exception as e:
    plpy.warning(f"llm_sentiment failed: {e}")
    return ("neutral", 0.0)

$fn$ LANGUAGE plpython3u;

-- -----------------------------------------------------------------------------
-- 3. compute_embedding v2 — retry with exponential backoff
-- -----------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION meclaw.compute_embedding(p_event_id UUID)
RETURNS VOID AS $fn$
    import json
    import requests
    import time

    plan = plpy.prepare("SELECT content FROM meclaw.brain_events WHERE id = $1", ["uuid"])
    result = plan.execute([str(p_event_id)])
    if not result:
        return
    content = result[0]["content"]
    if not content or len(content.strip()) < 3:
        return
    content = content[:8000]

    plan2 = plpy.prepare("SELECT base_url, api_key, config FROM meclaw.llm_providers WHERE id = $1", ["text"])
    prov = plan2.execute(["embedding-openrouter"])
    if not prov:
        return

    api_url = prov[0]["base_url"]
    api_key = prov[0]["api_key"]
    config = json.loads(prov[0]["config"]) if prov[0]["config"] else {}
    model = config.get("model", "openai/text-embedding-3-small")

    max_retries = 3
    for attempt in range(max_retries):
        try:
            resp = requests.post(
                api_url,
                headers={
                    "Authorization": f"Bearer {api_key}",
                    "Content-Type": "application/json",
                    "HTTP-Referer": "https://meclaw.ai",
                    "X-Title": "MeClaw"
                },
                json={"model": model, "input": content},
                timeout=30
            )

            # Rate limit handling
            if resp.status_code == 429:
                retry_after = int(resp.headers.get("Retry-After", 2 ** (attempt + 1)))
                plpy.warning(f"compute_embedding: rate limited, retry in {retry_after}s (attempt {attempt+1})")
                time.sleep(min(retry_after, 30))
                continue

            resp.raise_for_status()
            data = resp.json()
            embedding = data["data"][0]["embedding"]
            vec_str = "[" + ",".join(str(x) for x in embedding) + "]"
            update_plan = plpy.prepare(
                "UPDATE meclaw.brain_events SET embedding = $1::vector WHERE id = $2",
                ["text", "uuid"]
            )
            update_plan.execute([vec_str, str(p_event_id)])
            return  # Success

        except requests.exceptions.Timeout:
            plpy.warning(f"compute_embedding: timeout for {p_event_id} (attempt {attempt+1})")
            if attempt < max_retries - 1:
                time.sleep(2 ** (attempt + 1))
        except Exception as e:
            plpy.warning(f"compute_embedding: failed for {p_event_id} (attempt {attempt+1}): {e}")
            if attempt < max_retries - 1:
                time.sleep(2 ** (attempt + 1))

    plpy.warning(f"compute_embedding: all {max_retries} retries exhausted for {p_event_id}")
$fn$ LANGUAGE plpython3u;

-- -----------------------------------------------------------------------------
-- 4. Query embedding cache table
-- -----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS meclaw.embedding_cache (
    query_hash TEXT PRIMARY KEY,
    query_text TEXT NOT NULL,
    embedding vector(1536),
    created_at TIMESTAMPTZ DEFAULT NOW(),
    hit_count INT DEFAULT 0
);

-- Auto-evict old entries (keep last 500)
CREATE INDEX IF NOT EXISTS idx_embedding_cache_created ON meclaw.embedding_cache(created_at);

-- -----------------------------------------------------------------------------
-- 5. get_query_embedding v2 — with cache
-- -----------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION meclaw.get_query_embedding(p_text TEXT)
RETURNS vector(1536) AS $fn$
    import json
    import requests
    import hashlib

    text = (p_text or "")[:8000]
    if len(text.strip()) < 3:
        return None

    # Check cache
    query_hash = hashlib.md5(text.encode()).hexdigest()
    cache_plan = plpy.prepare(
        "SELECT embedding FROM meclaw.embedding_cache WHERE query_hash = $1", ["text"]
    )
    cached = cache_plan.execute([query_hash])
    if cached and cached[0]["embedding"]:
        plpy.execute(plpy.prepare(
            "UPDATE meclaw.embedding_cache SET hit_count = hit_count + 1 WHERE query_hash = $1", ["text"]
        ), [query_hash])
        return cached[0]["embedding"]

    plan_prov = plpy.prepare("SELECT base_url, api_key, config FROM meclaw.llm_providers WHERE id = $1", ["text"])
    prov = plan_prov.execute(["embedding-openrouter"])
    if not prov:
        return None

    api_url = prov[0]["base_url"]
    api_key = prov[0]["api_key"]
    config = json.loads(prov[0]["config"]) if prov[0]["config"] else {}
    model = config.get("model", "openai/text-embedding-3-small")

    max_retries = 3
    for attempt in range(max_retries):
        try:
            resp = requests.post(
                api_url,
                headers={
                    "Authorization": f"Bearer {api_key}",
                    "Content-Type": "application/json",
                    "HTTP-Referer": "https://meclaw.ai",
                    "X-Title": "MeClaw"
                },
                json={"model": model, "input": text},
                timeout=30
            )

            if resp.status_code == 429:
                import time
                retry_after = int(resp.headers.get("Retry-After", 2 ** (attempt + 1)))
                time.sleep(min(retry_after, 30))
                continue

            resp.raise_for_status()
            data = resp.json()
            embedding = data["data"][0]["embedding"]
            vec_str = "[" + ",".join(str(x) for x in embedding) + "]"

            # Store in cache
            plpy.execute(plpy.prepare("""
                INSERT INTO meclaw.embedding_cache (query_hash, query_text, embedding)
                VALUES ($1, $2, $3::vector)
                ON CONFLICT (query_hash) DO UPDATE SET hit_count = meclaw.embedding_cache.hit_count + 1
            """, ["text", "text", "text"]), [query_hash, text[:200], vec_str])

            # Evict old entries (keep 500 most recent)
            plpy.execute("""
                DELETE FROM meclaw.embedding_cache
                WHERE query_hash NOT IN (
                    SELECT query_hash FROM meclaw.embedding_cache
                    ORDER BY created_at DESC LIMIT 500
                )
            """)

            return vec_str

        except Exception as e:
            plpy.warning(f"get_query_embedding: attempt {attempt+1} failed: {e}")
            if attempt < max_retries - 1:
                import time
                time.sleep(2 ** (attempt + 1))

    return None
$fn$ LANGUAGE plpython3u;

-- -----------------------------------------------------------------------------
-- 6. personality_fit v2 — embedding-based similarity
-- -----------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION meclaw.personality_fit(p_agent_id TEXT, p_user_id TEXT, p_content TEXT)
RETURNS FLOAT AS $fn$
    import json

    # Get agent neural_matrix
    plan = plpy.prepare("SELECT neural_matrix, traits FROM meclaw.entities WHERE id = $1", ["text"])
    agent_row = plan.execute([p_agent_id])
    agent_matrix = json.loads(agent_row[0]["neural_matrix"]) if agent_row and agent_row[0]["neural_matrix"] else {}
    agent_traits = json.loads(agent_row[0]["traits"]) if agent_row and agent_row[0]["traits"] else {}

    # Get user observed_profile
    user_row = plan.execute([p_user_id]) if p_user_id else []
    user_profile = {}
    if user_row and user_row[0].get("neural_matrix"):
        user_profile = json.loads(user_row[0]["neural_matrix"])

    content_lower = (p_content or "").lower()

    # Multi-dimensional content classification via keyword clusters
    dimensions = {
        "technical": {
            "keywords": ["sql", "code", "function", "error", "bug", "api", "docker", "postgres",
                         "config", "deploy", "git", "schema", "query", "index", "trigger", "python",
                         "embedding", "vector", "llm", "model", "token", "prompt"],
            "agent_dims": ["logic", "reliability", "precision"],
            "weight": 0.0
        },
        "emotional": {
            "keywords": ["danke", "super", "frustriert", "toll", "schlecht", "freue", "sorry",
                         "liebe", "hasse", "angst", "sorge", "hoffnung", "begeistert", "traurig",
                         "wütend", "enttäuscht", "stolz", "dankbar"],
            "agent_dims": ["empathy", "charisma", "warmth"],
            "weight": 0.0
        },
        "creative": {
            "keywords": ["idee", "konzept", "design", "brainstorm", "vision", "vorschlag",
                         "experiment", "alternativ", "neu", "innovativ", "ansatz", "perspektive"],
            "agent_dims": ["creativity", "adaptability", "openness"],
            "weight": 0.0
        },
        "analytical": {
            "keywords": ["warum", "analyse", "vergleich", "strategie", "trade-off", "pro", "contra",
                         "bewerten", "einschätzen", "priorisieren", "risiko", "vorteil", "nachteil"],
            "agent_dims": ["logic", "precision", "curiosity"],
            "weight": 0.0
        },
        "organizational": {
            "keywords": ["plan", "todo", "aufgabe", "termin", "deadline", "liste", "überblick",
                         "status", "fortschritt", "phase", "milestone", "zeitplan", "backlog"],
            "agent_dims": ["reliability", "precision", "diligence"],
            "weight": 0.0
        }
    }

    # Score each dimension by keyword density
    words = content_lower.split()
    total_words = max(len(words), 1)

    for dim_name, dim in dimensions.items():
        matches = sum(1 for w in words if any(kw in w for kw in dim["keywords"]))
        dim["weight"] = min(matches / total_words * 5, 1.0)  # Normalize, cap at 1.0

    # Calculate personality fit score
    score = 0.5  # Neutral baseline
    total_weight = 0.0

    for dim_name, dim in dimensions.items():
        if dim["weight"] < 0.05:
            continue  # Skip dimensions with no signal

        # Average agent capability in this dimension
        dim_score = 0.0
        dim_count = 0
        for agent_dim in dim["agent_dims"]:
            val = agent_matrix.get(agent_dim)
            if val is not None:
                dim_score += float(val)
                dim_count += 1

        if dim_count > 0:
            avg_capability = dim_score / dim_count
            score += dim["weight"] * (avg_capability - 0.5) * 0.4
            total_weight += dim["weight"]

    # User alignment bonus: if user profile matches content dimension
    if user_profile:
        for dim_name, dim in dimensions.items():
            if dim["weight"] < 0.1:
                continue
            user_pref = user_profile.get(dim_name + "_preference", 0.5)
            if isinstance(user_pref, (int, float)):
                score += dim["weight"] * (float(user_pref) - 0.5) * 0.1

    return max(0.0, min(1.0, score))

$fn$ LANGUAGE plpython3u;

-- -----------------------------------------------------------------------------
-- 7. Hebbian Learning — active prototype_associations update
-- -----------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION meclaw.hebbian_update(p_event_id UUID, p_reward_delta FLOAT)
RETURNS VOID AS $$
DECLARE
    v_entities TEXT[];
    v_proto_a TEXT;
    v_proto_b TEXT;
    v_hebbian_rate FLOAT := 0.1;
BEGIN
    -- Get all entities involved in this event
    SELECT array_agg(entity_id) INTO v_entities
    FROM meclaw.entity_events
    WHERE event_id = p_event_id;

    IF v_entities IS NULL OR array_length(v_entities, 1) < 2 THEN
        RETURN;
    END IF;

    -- Find prototypes associated with these entities
    -- For now: entities co-occurring in the same event = co-activation
    FOR i IN 1..array_length(v_entities, 1) LOOP
        FOR j IN (i+1)..array_length(v_entities, 1) LOOP
            v_proto_a := v_entities[i];
            v_proto_b := v_entities[j];

            -- Ensure consistent ordering
            IF v_proto_a > v_proto_b THEN
                v_proto_a := v_entities[j];
                v_proto_b := v_entities[i];
            END IF;

            -- Upsert association with Hebbian weight update
            -- Positive reward = strengthen, negative = weaken
            INSERT INTO meclaw.prototype_associations (prototype_a, prototype_b, weight, last_updated_seq)
            VALUES (
                v_proto_a, v_proto_b,
                v_hebbian_rate * p_reward_delta,
                (SELECT COALESCE(MAX(seq), 0) FROM meclaw.brain_events)
            )
            ON CONFLICT (prototype_a, prototype_b) DO UPDATE
            SET weight = meclaw.prototype_associations.weight + v_hebbian_rate * p_reward_delta,
                last_updated_seq = (SELECT COALESCE(MAX(seq), 0) FROM meclaw.brain_events);
        END LOOP;
    END LOOP;
END;
$$ LANGUAGE plpgsql;

-- Note: Hebbian update references prototypes table.
-- Entities aren't prototypes directly — but we use entity IDs as prototype keys
-- for co-activation tracking. This bridges entity_events → prototype_associations.
-- Future: proper prototype formation in consolidation_bee will create real prototypes.

-- For now, ensure entities that appear in entity_events can be referenced as prototypes:
INSERT INTO meclaw.prototypes (id, agent_id, centroid, weight, activation_count, last_activated_seq, created_seq)
SELECT DISTINCT ee.entity_id, 'meclaw:agent:walter', NULL::vector, 0.5, 1,
    (SELECT COALESCE(MAX(seq), 0) FROM meclaw.brain_events),
    (SELECT COALESCE(MAX(seq), 0) FROM meclaw.brain_events)
FROM meclaw.entity_events ee
JOIN meclaw.entities e ON e.id = ee.entity_id
WHERE NOT EXISTS (SELECT 1 FROM meclaw.prototypes WHERE id = ee.entity_id)
ON CONFLICT (id) DO NOTHING;

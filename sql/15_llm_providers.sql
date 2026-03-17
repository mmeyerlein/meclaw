-- MeClaw v0.1.0 — LLM Provider & Model Registry

-- =================================================================
-- 1. Providers
-- =================================================================
CREATE TABLE IF NOT EXISTS meclaw.llm_providers (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    base_url    TEXT NOT NULL,
    api_key     TEXT,                      -- NULL = kein Auth nötig (lokales vLLM)
    type        TEXT NOT NULL DEFAULT 'openai',  -- openai | anthropic | custom
    enabled     BOOLEAN DEFAULT true,
    priority    INT DEFAULT 10,            -- niedriger = bevorzugt
    config      JSONB DEFAULT '{}',        -- Extra-Config (rate limits, headers, etc.)
    created_at  TIMESTAMPTZ DEFAULT clock_timestamp()
);

-- =================================================================
-- 2. Models
-- =================================================================
CREATE TABLE IF NOT EXISTS meclaw.llm_models (
    id              TEXT PRIMARY KEY,
    provider_id     TEXT NOT NULL REFERENCES meclaw.llm_providers(id),
    model_name      TEXT NOT NULL,          -- API model name (z.B. 'anthropic/claude-sonnet-4-20250514')
    display_name    TEXT,                   -- Anzeigename
    tier            TEXT NOT NULL DEFAULT 'medium',  -- small | medium | large | reasoning
    max_tokens      INT DEFAULT 1024,
    timeout_ms      INT DEFAULT 120000,
    cost_per_1k_in  NUMERIC(10,6) DEFAULT 0,
    cost_per_1k_out NUMERIC(10,6) DEFAULT 0,
    supports_tools  BOOLEAN DEFAULT true,
    enabled         BOOLEAN DEFAULT true,
    config          JSONB DEFAULT '{}',     -- Extra (chat_template_kwargs, etc.)
    created_at      TIMESTAMPTZ DEFAULT clock_timestamp()
);

CREATE INDEX IF NOT EXISTS idx_llm_models_tier ON meclaw.llm_models (tier) WHERE enabled = true;

-- =================================================================
-- 3. Resolve Model: ID oder Tier → Provider + Model Details
-- =================================================================
CREATE OR REPLACE FUNCTION meclaw.resolve_model(p_model_id TEXT DEFAULT NULL, p_tier TEXT DEFAULT NULL)
RETURNS TABLE (
    model_id        TEXT,
    provider_id     TEXT,
    base_url        TEXT,
    api_key         TEXT,
    model_name      TEXT,
    max_tokens      INT,
    timeout_ms      INT,
    supports_tools  BOOLEAN,
    provider_type   TEXT,
    model_config    JSONB,
    provider_config JSONB
) AS $fn$
BEGIN
    -- Direkte Model-ID
    IF p_model_id IS NOT NULL THEN
        RETURN QUERY
        SELECT m.id, m.provider_id, p.base_url, p.api_key, m.model_name,
               m.max_tokens, m.timeout_ms, m.supports_tools, p.type,
               m.config, p.config
        FROM meclaw.llm_models m
        JOIN meclaw.llm_providers p ON m.provider_id = p.id
        WHERE m.id = p_model_id AND m.enabled = true AND p.enabled = true
        LIMIT 1;
        RETURN;
    END IF;

    -- Tier-basiert: bestes verfügbares Modell (Provider-Priority)
    IF p_tier IS NOT NULL THEN
        RETURN QUERY
        SELECT m.id, m.provider_id, p.base_url, p.api_key, m.model_name,
               m.max_tokens, m.timeout_ms, m.supports_tools, p.type,
               m.config, p.config
        FROM meclaw.llm_models m
        JOIN meclaw.llm_providers p ON m.provider_id = p.id
        WHERE m.tier = p_tier AND m.enabled = true AND p.enabled = true
        ORDER BY p.priority ASC
        LIMIT 1;
        RETURN;
    END IF;

    -- Fallback: irgendein enabled Modell
    RETURN QUERY
    SELECT m.id, m.provider_id, p.base_url, p.api_key, m.model_name,
           m.max_tokens, m.timeout_ms, m.supports_tools, p.type,
           m.config, p.config
    FROM meclaw.llm_models m
    JOIN meclaw.llm_providers p ON m.provider_id = p.id
    WHERE m.enabled = true AND p.enabled = true
    ORDER BY p.priority ASC
    LIMIT 1;
END;
$fn$ LANGUAGE plpgsql;

-- =================================================================
-- 4. Seed: vLLM Local + OpenRouter
-- =================================================================
INSERT INTO meclaw.llm_providers (id, name, base_url, api_key, type, priority) VALUES
    ('vllm-local', 'vLLM Local', 'http://10.235.74.1:8000/v1', NULL, 'openai', 10),
    ('openrouter', 'OpenRouter', 'https://openrouter.ai/api/v1', NULL, 'openai', 1)
ON CONFLICT (id) DO UPDATE SET
    base_url = EXCLUDED.base_url,
    priority = EXCLUDED.priority;

INSERT INTO meclaw.llm_models (id, provider_id, model_name, display_name, tier, max_tokens, cost_per_1k_in, cost_per_1k_out, supports_tools) VALUES
    ('qwen-9b', 'vllm-local', 'Qwen/Qwen3.5-9B', 'Qwen 3.5 9B', 'small', 1024, 0, 0, true)
ON CONFLICT (id) DO UPDATE SET
    model_name = EXCLUDED.model_name,
    tier = EXCLUDED.tier;

-- =================================================================
-- 5. llm_bee_v2: Multi-Model Support
-- =================================================================
CREATE OR REPLACE FUNCTION meclaw.llm_bee_v2(p_msg_id uuid, p_task_id uuid, p_bee_id text, p_content jsonb)
 RETURNS void
 LANGUAGE plpgsql
AS $function$
DECLARE
    v_bee_config TEXT;
    v_cfg        JSONB;
    v_soul       TEXT;
    v_model_id   TEXT;
    v_model_tier TEXT;
    v_resolved   RECORD;
    v_body       JSONB;
    v_headers    JSONB;
    v_job_id     UUID;
    v_net_req    BIGINT;
    v_messages   JSONB;
    v_history    JSONB;
    v_tools      JSONB;
    v_tool_results JSONB;
    v_tool_count INT;
    v_tr         JSONB;
BEGIN
    -- Rate Limit Check
    IF NOT meclaw.check_rate_limit('llm_per_minute')
       OR NOT meclaw.check_rate_limit('llm_per_hour')
       OR NOT meclaw.check_rate_limit('llm_per_day') THEN
        UPDATE meclaw.messages SET status = 'failed' WHERE id = p_msg_id;
        PERFORM meclaw.log_event(p_msg_id, p_task_id, 'llm_bee', 'rate_limited', '{}');
        RETURN;
    END IF;

    -- Bee-Config aus AGE laden
    LOAD 'age';
    SET LOCAL search_path = meclaw, ag_catalog, "$user", public;
    EXECUTE format(
        'SELECT cfg::text FROM cypher(''meclaw_graph'', $q$
             MATCH (b:Bee {id: %L}) RETURN b.config
         $q$) AS (cfg agtype) LIMIT 1',
        p_bee_id
    ) INTO v_bee_config;
    v_bee_config := trim(both '"' from v_bee_config);
    v_bee_config := replace(v_bee_config, '\"', '"');
    v_cfg := v_bee_config::jsonb;

    v_soul := COALESCE(v_cfg->>'soul', 'You are a helpful assistant.');

    -- Model Resolution: model_id > model_tier > Fallback
    v_model_id   := v_cfg->>'model_id';
    v_model_tier := v_cfg->>'model_tier';

    SELECT * INTO v_resolved
    FROM meclaw.resolve_model(v_model_id, v_model_tier);

    -- Fallback auf alte Config wenn kein Model in Registry
    IF v_resolved.model_id IS NULL THEN
        v_resolved.base_url      := COALESCE(v_cfg->>'llm_url', 'http://10.235.74.1:8000/v1');
        v_resolved.model_name    := COALESCE(v_cfg->>'llm_model', 'Qwen/Qwen3.5-9B');
        v_resolved.max_tokens    := COALESCE((v_cfg->>'max_tokens')::INT, 512);
        v_resolved.timeout_ms    := COALESCE((v_cfg->>'timeout_ms')::INT, 60000);
        v_resolved.supports_tools := true;
        v_resolved.api_key       := NULL;
        v_resolved.model_config  := '{}'::jsonb;
        v_resolved.model_id      := 'fallback';
    END IF;

    -- Messages-Array aufbauen
    v_messages := jsonb_build_array(
        jsonb_build_object('role', 'system', 'content', v_soul)
    );

    -- Conversation History
    v_history := p_content->'conversation_history';
    IF v_history IS NOT NULL AND jsonb_typeof(v_history) = 'array' AND jsonb_array_length(v_history) > 0 THEN
        FOR i IN 0..jsonb_array_length(v_history) - 1 LOOP
            v_messages := v_messages || jsonb_build_array(v_history->i);
        END LOOP;
    END IF;

    -- User Input
    v_messages := v_messages || jsonb_build_array(
        jsonb_build_object('role', 'user', 'content', COALESCE(p_content->>'input', ''))
    );

    -- Tool Results?
    v_tool_results := p_content->'tool_results';
    IF v_tool_results IS NOT NULL AND jsonb_typeof(v_tool_results) = 'array' THEN
        IF p_content ? 'assistant_message' THEN
            v_messages := v_messages || jsonb_build_array(p_content->'assistant_message');
        END IF;
        FOR i IN 0..jsonb_array_length(v_tool_results) - 1 LOOP
            v_tr := v_tool_results->i;
            v_messages := v_messages || jsonb_build_array(
                jsonb_build_object(
                    'role', 'tool',
                    'tool_call_id', v_tr->>'tool_call_id',
                    'content', (v_tr->'result')::text
                )
            );
        END LOOP;
    END IF;

    v_tool_count := COALESCE((p_content->>'tool_call_count')::int, 0);

    -- Body bauen
    v_body := jsonb_build_object(
        'model', v_resolved.model_name,
        'max_tokens', v_resolved.max_tokens,
        'messages', v_messages
    );

    -- chat_template_kwargs nur für lokales vLLM (nicht für OpenRouter)
    IF v_resolved.api_key IS NULL THEN
        v_body := v_body || jsonb_build_object(
            'chat_template_kwargs', jsonb_build_object('enable_thinking', false)
        );
    END IF;

    -- Extra model config mergen
    IF v_resolved.model_config IS NOT NULL AND v_resolved.model_config != '{}'::jsonb THEN
        v_body := v_body || v_resolved.model_config;
    END IF;

    -- Tools mitschicken
    v_tools := meclaw.get_tool_definitions();
    IF v_resolved.supports_tools AND jsonb_array_length(v_tools) > 0 AND v_tool_count < 5 THEN
        v_body := v_body || jsonb_build_object('tools', v_tools, 'tool_choice', 'auto');
    END IF;

    -- Headers (API Key wenn vorhanden)
    v_headers := '{"Content-Type": "application/json"}'::jsonb;
    IF v_resolved.api_key IS NOT NULL THEN
        v_headers := v_headers || jsonb_build_object('Authorization', 'Bearer ' || v_resolved.api_key);
    END IF;

    -- Job in Staging-Tabelle (erweitert um headers)
    INSERT INTO meclaw.llm_jobs (msg_id, url, body, timeout_ms)
    VALUES (p_msg_id, v_resolved.base_url || '/chat/completions', v_body, v_resolved.timeout_ms)
    RETURNING id INTO v_job_id;

    BEGIN
        PERFORM pg_background_launch(
            format('SELECT meclaw.llm_http_call_by_id(%L::uuid)', v_job_id)
        );
        PERFORM meclaw.log_event(p_msg_id, p_task_id, 'llm_bee', 'llm_request',
            jsonb_build_object(
                'model', v_resolved.model_name,
                'model_id', v_resolved.model_id,
                'provider', v_resolved.provider_id,
                'job_id', v_job_id,
                'via', 'pg_background',
                'history_count', COALESCE(jsonb_array_length(v_history), 0),
                'tool_count', v_tool_count,
                'tools_available', jsonb_array_length(v_tools)
            ));
    EXCEPTION WHEN OTHERS THEN
        DELETE FROM meclaw.llm_jobs WHERE id = v_job_id;
        SELECT net.http_post(
            url := v_resolved.base_url || '/chat/completions',
            body := v_body,
            headers := v_headers,
            timeout_milliseconds := v_resolved.timeout_ms
        ) INTO v_net_req;
        INSERT INTO meclaw.net_requests (net_req_id, type, ref_id)
        VALUES (v_net_req, 'llm_call', p_msg_id);
        PERFORM meclaw.log_event(p_msg_id, p_task_id, 'llm_bee', 'llm_request',
            jsonb_build_object(
                'model', v_resolved.model_name,
                'model_id', v_resolved.model_id,
                'provider', v_resolved.provider_id,
                'via', 'direct', 'error', SQLERRM
            ));
    END;

    UPDATE meclaw.messages
    SET status = 'waiting',
        waiting_for = 'llm_result',
        content = content || jsonb_build_object('prompt_messages', v_messages)
    WHERE id = p_msg_id;
END;
$function$;

-- Note: llm_http_call_by_id is defined in 06_llm_bee.sql (plpython3u + requests)

-- MeClaw v0.1.0 — LLM Bee (Multi-Provider via Registry)
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
$function$

;

CREATE OR REPLACE FUNCTION meclaw.llm_http_call_by_id(p_job_id uuid)
 RETURNS void
 LANGUAGE plpython3u
AS $function$
import json, time

# Wait for job to be visible (separate transaction from caller)
job = None
for i in range(50):
    rows = plpy.execute(plpy.prepare(
        "SELECT id, msg_id, url, body::text, timeout_ms FROM meclaw.llm_jobs WHERE id = $1", ["uuid"]
    ), [p_job_id])
    if rows.nrows() > 0:
        job = rows[0]
        break
    time.sleep(0.1)

if job is None:
    plpy.error(f"llm_http_call_by_id: job {p_job_id} not found after 5s")

msg_id = job["msg_id"]
url = job["url"]
body = json.loads(job["body"])
timeout_s = max(job["timeout_ms"] / 1000, 10)

# Build headers
headers = {"Content-Type": "application/json"}
if "openrouter" in url:
    rows = plpy.execute("SELECT api_key FROM meclaw.llm_providers WHERE id = 'openrouter' AND api_key IS NOT NULL")
    if rows.nrows() > 0:
        headers["Authorization"] = "Bearer " + rows[0]["api_key"]
    headers["HTTP-Referer"] = "https://meclaw.ai"
    headers["X-Title"] = "MeClaw"
elif "10.235.74" in url:
    rows = plpy.execute("SELECT api_key FROM meclaw.llm_providers WHERE id = 'vllm-local' AND api_key IS NOT NULL")
    if rows.nrows() > 0 and rows[0]["api_key"]:
        headers["Authorization"] = "Bearer " + rows[0]["api_key"]

plpy.execute(plpy.prepare(
    "SELECT meclaw.log_event($1::uuid, NULL, 'llm_bee', 'bg_llm_sent', $2::jsonb)",
    ["uuid", "jsonb"]
), [msg_id, json.dumps({"via": "requests", "url": url[:60]})])

# Direct HTTP call via requests (no pg_net dependency)
import requests as req
try:
    resp = req.post(url, json=body, headers=headers, timeout=timeout_s)
    status = resp.status_code
    resp_body = resp.text
except Exception as e:
    status = 0
    resp_body = json.dumps({"error": str(e)})

# Insert fake net_request so on_net_response_safe can find it
fake_id = -1
try:
    r = plpy.execute(plpy.prepare(
        "INSERT INTO meclaw.net_requests (net_req_id, type, ref_id) VALUES ($1, 'llm_call', $2::uuid) RETURNING net_req_id",
        ["bigint", "uuid"]
    ), [fake_id, msg_id])
except:
    # net_req_id conflict: use a different one
    import random
    fake_id = -(random.randint(1, 999999))
    plpy.execute(plpy.prepare(
        "INSERT INTO meclaw.net_requests (net_req_id, type, ref_id) VALUES ($1, 'llm_call', $2::uuid)",
        ["bigint", "uuid"]
    ), [fake_id, msg_id])

# Process response via existing handler
plpy.execute(plpy.prepare(
    "SELECT meclaw.on_net_response_safe($1::bigint, $2::int, $3::text, NULL)",
    ["bigint", "int", "text"]
), [fake_id, status, resp_body])

# Cleanup
plpy.execute(plpy.prepare("DELETE FROM meclaw.llm_jobs WHERE id = $1", ["uuid"]), [p_job_id])
$function$

;

CREATE OR REPLACE FUNCTION meclaw.check_rate_limit(p_limit_id text)
 RETURNS boolean
 LANGUAGE plpgsql
AS $function$
DECLARE
    v_limit record;
    v_count int;
BEGIN
    SELECT * INTO v_limit FROM meclaw.rate_limits WHERE id = p_limit_id;
    IF NOT FOUND THEN RETURN true; END IF;

    SELECT count(*) INTO v_count
    FROM meclaw.events
    WHERE event = 'llm_request'
    AND created_at >= clock_timestamp() - make_interval(secs => v_limit.window_sec);

    IF v_count >= v_limit.max_count THEN
        PERFORM meclaw.log_event(NULL, NULL, 'rate_limiter', 'rate_limit_hit',
            jsonb_build_object('limit', p_limit_id, 'count', v_count, 'max', v_limit.max_count));
        RETURN false;
    END IF;

    RETURN true;
END;
$function$

;


-- MeClaw v0.1.0 — Tool System
-- Registry, Tool-Implementierungen, tool_bee, LLM+Trigger Anpassungen

-- =================================================================
-- 1. Tool Registry
-- =================================================================
CREATE TABLE IF NOT EXISTS meclaw.tools (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    description TEXT NOT NULL,
    parameters  JSONB NOT NULL,
    handler     TEXT NOT NULL,
    enabled     BOOLEAN DEFAULT true,
    created_at  TIMESTAMPTZ DEFAULT clock_timestamp()
);

-- Tool-Definitionen für LLM-Request (OpenAI format)
CREATE OR REPLACE FUNCTION meclaw.get_tool_definitions()
RETURNS jsonb AS $fn$
    SELECT COALESCE(jsonb_agg(
        jsonb_build_object(
            'type', 'function',
            'function', jsonb_build_object(
                'name', id,
                'description', description,
                'parameters', parameters
            )
        )
    ), '[]'::jsonb) FROM meclaw.tools WHERE enabled = true;
$fn$ LANGUAGE sql;

-- =================================================================
-- 2. Tool Implementations
-- =================================================================

-- sql_read: Nur SELECT
CREATE OR REPLACE FUNCTION meclaw.tool_sql_read(p_args JSONB)
RETURNS JSONB AS $fn$
DECLARE
    v_query TEXT;
    v_result JSONB;
BEGIN
    v_query := trim(p_args->>'query');
    IF NOT (lower(v_query) LIKE 'select%') THEN
        RETURN jsonb_build_object('error', 'Only SELECT queries allowed');
    END IF;
    BEGIN
        EXECUTE format('SELECT COALESCE(jsonb_agg(row_to_json(t)), ''[]''::jsonb) FROM (%s) t', v_query)
        INTO v_result;
    EXCEPTION WHEN OTHERS THEN
        RETURN jsonb_build_object('error', SQLERRM);
    END;
    RETURN v_result;
END;
$fn$ LANGUAGE plpgsql;

-- sql_write: INSERT/UPDATE/DELETE mit Audit
CREATE OR REPLACE FUNCTION meclaw.tool_sql_write(p_args JSONB)
RETURNS JSONB AS $fn$
DECLARE
    v_query TEXT;
    v_count INT;
BEGIN
    v_query := trim(p_args->>'query');
    -- Block dangerous operations
    IF lower(v_query) LIKE '%drop %' OR lower(v_query) LIKE '%truncate %' THEN
        RETURN jsonb_build_object('error', 'DROP and TRUNCATE not allowed');
    END IF;
    BEGIN
        EXECUTE v_query;
        GET DIAGNOSTICS v_count = ROW_COUNT;
    EXCEPTION WHEN OTHERS THEN
        RETURN jsonb_build_object('error', SQLERRM);
    END;
    RETURN jsonb_build_object('rows_affected', v_count, 'status', 'ok');
END;
$fn$ LANGUAGE plpgsql;

-- python_exec: Beliebiger Python-Code
CREATE OR REPLACE FUNCTION meclaw.tool_python_exec(p_args JSONB)
RETURNS JSONB AS $fn$
import json, io, sys

code = json.loads(p_args)["code"]
old_stdout = sys.stdout
sys.stdout = buffer = io.StringIO()
result = None
try:
    exec_globals = {"plpy": plpy, "__builtins__": __builtins__}
    exec(code, exec_globals)
    stdout_output = buffer.getvalue()
    result = exec_globals.get("result", stdout_output or "ok")
    return json.dumps({"output": str(result)})
except Exception as e:
    return json.dumps({"error": str(e)})
finally:
    sys.stdout = old_stdout
$fn$ LANGUAGE plpython3u;

-- =================================================================
-- 3. Seed Tools
-- =================================================================
INSERT INTO meclaw.tools (id, name, description, parameters, handler) VALUES
(
    'sql_read', 'sql_read',
    'Execute a read-only SQL SELECT query against the PostgreSQL database. Returns rows as JSON array. Use this to inspect data, count records, or analyze the database.',
    '{"type": "object", "properties": {"query": {"type": "string", "description": "A SQL SELECT query to execute"}}, "required": ["query"]}',
    'meclaw.tool_sql_read'
),
(
    'sql_write', 'sql_write',
    'Execute a SQL write query (INSERT, UPDATE, DELETE, CREATE). Returns number of affected rows. DROP and TRUNCATE are blocked.',
    '{"type": "object", "properties": {"query": {"type": "string", "description": "A SQL write query to execute"}}, "required": ["query"]}',
    'meclaw.tool_sql_write'
),
(
    'python_exec', 'python_exec',
    'Execute arbitrary Python code. Use for calculations, data transformations, or anything SQL cannot do. Store return value in variable named "result" or print output.',
    '{"type": "object", "properties": {"code": {"type": "string", "description": "Python code to execute"}}, "required": ["code"]}',
    'meclaw.tool_python_exec'
)
ON CONFLICT (id) DO UPDATE SET
    description = EXCLUDED.description,
    parameters = EXCLUDED.parameters,
    handler = EXCLUDED.handler;

-- =================================================================
-- 4. tool_bee: Generischer Executor
-- =================================================================
CREATE OR REPLACE FUNCTION meclaw.tool_bee(p_msg_id UUID)
RETURNS void AS $fn$
DECLARE
    v_msg       RECORD;
    v_tool_name TEXT;
    v_tool_args JSONB;
    v_tool_call_id TEXT;
    v_tool      RECORD;
    v_result    JSONB;
    v_content   JSONB;
    v_tool_calls JSONB;
    i           INT;
    v_results   JSONB := '[]'::jsonb;
BEGIN
    SELECT * INTO v_msg FROM meclaw.messages WHERE id = p_msg_id;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'tool_bee: message % not found', p_msg_id;
    END IF;

    v_tool_calls := v_msg.content->'tool_calls';

    -- Jeder Tool-Call einzeln ausführen
    FOR i IN 0..jsonb_array_length(v_tool_calls) - 1 LOOP
        v_tool_name    := v_tool_calls->i->'function'->>'name';
        v_tool_args    := (v_tool_calls->i->'function'->>'arguments')::jsonb;
        v_tool_call_id := v_tool_calls->i->>'id';

        -- Handler aus Registry
        SELECT * INTO v_tool FROM meclaw.tools WHERE id = v_tool_name AND enabled = true;
        IF NOT FOUND THEN
            v_result := jsonb_build_object('error', format('Tool %s not found', v_tool_name));
        ELSE
            -- Handler aufrufen
            BEGIN
                EXECUTE format('SELECT %s($1)', v_tool.handler)
                INTO v_result
                USING v_tool_args;
            EXCEPTION WHEN OTHERS THEN
                v_result := jsonb_build_object('error', SQLERRM);
            END;
        END IF;

        PERFORM meclaw.log_event(p_msg_id, v_msg.task_id, 'tool_bee', 'tool_executed',
            jsonb_build_object(
                'tool', v_tool_name,
                'call_id', v_tool_call_id,
                'result_size', length(v_result::text),
                'has_error', v_result ? 'error'
            ));

        v_results := v_results || jsonb_build_object(
            'tool_call_id', v_tool_call_id,
            'name', v_tool_name,
            'result', v_result
        );
    END LOOP;

    -- Tool-Call Counter hochzählen
    v_content := v_msg.content || jsonb_build_object(
        'tool_results', v_results,
        'tool_call_count', COALESCE((v_msg.content->>'tool_call_count')::int, 0) + 1
    );

    -- tool_result Message → done → router_bee → zurück zu llm_bee
    INSERT INTO meclaw.messages (
        task_id, channel_id, previous_id, type, sender, status, content
    ) VALUES (
        v_msg.task_id, v_msg.channel_id, p_msg_id,
        'tool_result', 'tool_bee', 'done', v_content
    );

    UPDATE meclaw.messages SET status = 'done' WHERE id = p_msg_id AND status != 'done';
END;
$fn$ LANGUAGE plpgsql;

-- =================================================================
-- 5. on_net_response_safe: tool_calls erkennen
-- =================================================================
CREATE OR REPLACE FUNCTION meclaw.on_net_response_safe(p_net_id bigint, p_status integer, p_content text, p_error text)
 RETURNS void
 LANGUAGE plpgsql
AS $function$
DECLARE
    v_req           RECORD;
    v_body          JSONB;
    v_updates       JSONB;
    v_update        JSONB;
    v_msg           JSONB;
    v_text          TEXT;
    v_update_id     BIGINT;
    v_max_update_id BIGINT := 0;
    v_task_id       UUID;
    v_msg_id        UUID;
    v_channel_id    UUID;
    v_orig_msg      RECORD;
    v_choice        JSONB;
    v_output        TEXT;
    v_finish        TEXT;
    v_tool_calls    JSONB;
BEGIN
    SELECT type, ref_id INTO v_req
    FROM meclaw.net_requests WHERE net_req_id = p_net_id;
    IF NOT FOUND THEN RETURN; END IF;
    DELETE FROM meclaw.net_requests WHERE net_req_id = p_net_id;

    -- ── Telegram Poll ───────────────────────────────────
    IF v_req.type = 'telegram_poll' THEN
        v_channel_id := v_req.ref_id;
        IF p_status < 200 OR p_status >= 300 OR p_error IS NOT NULL THEN
            PERFORM meclaw.log_event(NULL, NULL, 'receiver_bee', 'poll_error',
                jsonb_build_object('status', p_status, 'error', p_error));
            PERFORM meclaw.channel_bee_start(v_channel_id, NULL);
            RETURN;
        END IF;
        v_body    := p_content::jsonb;
        v_updates := v_body->'result';
        FOR v_update IN SELECT * FROM jsonb_array_elements(v_updates) LOOP
            v_update_id := (v_update->>'update_id')::BIGINT;
            IF v_update_id > v_max_update_id THEN v_max_update_id := v_update_id; END IF;
            v_msg := v_update->'message';
            IF v_msg IS NULL OR v_msg->>'text' IS NULL THEN CONTINUE; END IF;
            v_text := v_msg->>'text';
            INSERT INTO meclaw.tasks (channel_id, status)
            VALUES (v_channel_id, 'running') RETURNING id INTO v_task_id;
            INSERT INTO meclaw.messages (
                task_id, channel_id, type, sender, status, next_bee, content
            ) VALUES (
                v_task_id, v_channel_id, 'user_input',
                v_msg->'from'->>'id', 'done', NULL,
                jsonb_build_object(
                    'input',              v_text,
                    'telegram_update_id', v_update_id,
                    'telegram_chat_id',   v_msg->'chat'->>'id',
                    'current_bee',        'main-receiver-bee',
                    'stack',              '[]'::jsonb
                )
            ) RETURNING id INTO v_msg_id;
            PERFORM meclaw.log_event(v_msg_id, v_task_id, 'receiver_bee', 'message_received',
                jsonb_build_object('update_id', v_update_id, 'text', left(v_text, 100)));
        END LOOP;
        IF v_max_update_id = 0 THEN
            PERFORM meclaw.channel_bee_start(v_channel_id, NULL);
        END IF;
        RETURN;
    END IF;

    -- ── LLM Response ────────────────────────────────────
    IF v_req.type = 'llm_call' THEN
        v_msg_id := v_req.ref_id;
        SELECT * INTO v_orig_msg FROM meclaw.messages WHERE id = v_msg_id;
        IF p_status < 200 OR p_status >= 300 THEN
            UPDATE meclaw.messages SET status='failed' WHERE id=v_msg_id;
            PERFORM meclaw.channel_bee_start('00000000-0000-0000-0000-000000000001'::uuid, NULL);
            RETURN;
        END IF;
        v_body   := p_content::jsonb;
        v_choice := v_body->'choices'->0;
        v_finish := v_choice->>'finish_reason';

        -- ── Tool Calls? ──────────────────────────────
        IF v_finish = 'tool_calls' THEN
            v_tool_calls := v_choice->'message'->'tool_calls';
            PERFORM meclaw.log_event(v_msg_id, v_orig_msg.task_id, 'llm_bee', 'tool_calls_received',
                jsonb_build_object('count', jsonb_array_length(v_tool_calls),
                    'tools', (SELECT jsonb_agg(tc->'function'->>'name') FROM jsonb_array_elements(v_tool_calls) tc)));

            -- tool_call Message erzeugen
            INSERT INTO meclaw.messages (
                task_id, channel_id, previous_id, type, sender, status, next_bee, content
            ) VALUES (
                v_orig_msg.task_id, v_orig_msg.channel_id, v_msg_id,
                'tool_call', 'llm_bee', 'done', NULL,
                v_orig_msg.content || jsonb_build_object(
                    'tool_calls',       v_tool_calls,
                    'assistant_message', v_choice->'message',
                    'usage',            v_body->'usage'
                )
            );
            UPDATE meclaw.messages SET status='done' WHERE id=v_msg_id;
            RETURN;
        END IF;

        -- ── Normal Stop ──────────────────────────────
        v_output := v_choice->'message'->>'content';
        IF v_output IS NULL OR v_output = '' THEN
            v_output := v_choice->'message'->>'reasoning';
        END IF;
        PERFORM meclaw.log_event(v_msg_id, v_orig_msg.task_id, 'llm_bee', 'llm_response',
            jsonb_build_object('finish', v_finish, 'tokens', v_body->'usage'->>'total_tokens'));
        INSERT INTO meclaw.messages (
            task_id, channel_id, previous_id, type, sender, status, next_bee, content
        ) VALUES (
            v_orig_msg.task_id, v_orig_msg.channel_id, v_msg_id,
            'llm_result', 'llm_bee', 'done', NULL,
            jsonb_build_object(
                'input',            v_orig_msg.content->>'input',
                'output',           v_output,
                'finish_reason',    v_finish,
                'prompt_messages',  v_orig_msg.content->'prompt_messages',
                'usage',            v_body->'usage',
                'current_bee',      v_orig_msg.content->>'current_bee',
                'stack',            v_orig_msg.content->'stack',
                'telegram_chat_id', v_orig_msg.content->>'telegram_chat_id'
            )
        );
        UPDATE meclaw.messages SET status='done' WHERE id=v_msg_id;
        RETURN;
    END IF;

    -- ── Telegram Send ───────────────────────────────────
    IF v_req.type = 'telegram_send' THEN
        PERFORM meclaw.log_event(v_req.ref_id, NULL, 'sender_bee',
            CASE WHEN p_status >= 200 AND p_status < 300 THEN 'message_sent' ELSE 'send_error' END,
            jsonb_build_object('status', p_status));
        IF NOT EXISTS (SELECT 1 FROM meclaw.net_requests WHERE type = 'telegram_poll') THEN
            PERFORM meclaw.channel_bee_start('00000000-0000-0000-0000-000000000001'::uuid, NULL);
        END IF;
        RETURN;
    END IF;
END;
$function$;

-- =================================================================
-- 6. llm_bee_v2: Tools einbauen + tool_result handling
-- =================================================================
CREATE OR REPLACE FUNCTION meclaw.llm_bee_v2(p_msg_id uuid, p_task_id uuid, p_bee_id text, p_content jsonb)
 RETURNS void
 LANGUAGE plpgsql
AS $function$
DECLARE
    v_config    TEXT;
    v_cfg       JSONB;
    v_llm_url   TEXT;
    v_model     TEXT;
    v_max_tok   INT;
    v_timeout   INT;
    v_soul      TEXT;
    v_body      JSONB;
    v_job_id    UUID;
    v_net_req   BIGINT;
    v_messages  JSONB;
    v_history   JSONB;
    v_tools     JSONB;
    v_tool_results JSONB;
    v_tool_count INT;
    v_tr        JSONB;
BEGIN
    -- Rate Limit Check
    IF NOT meclaw.check_rate_limit('llm_per_minute')
       OR NOT meclaw.check_rate_limit('llm_per_hour')
       OR NOT meclaw.check_rate_limit('llm_per_day') THEN
        UPDATE meclaw.messages SET status = 'failed' WHERE id = p_msg_id;
        PERFORM meclaw.log_event(p_msg_id, p_task_id, 'llm_bee', 'rate_limited', '{}');
        RETURN;
    END IF;

    LOAD 'age';
    SET LOCAL search_path = meclaw, ag_catalog, "$user", public;
    EXECUTE format(
        'SELECT cfg::text FROM cypher(''meclaw_graph'', $q$
             MATCH (b:Bee {id: %L}) RETURN b.config
         $q$) AS (cfg agtype) LIMIT 1',
        p_bee_id
    ) INTO v_config;
    v_config := trim(both '"' from v_config);
    v_config := replace(v_config, '\"', '"');
    v_cfg     := v_config::jsonb;

    v_llm_url := COALESCE(v_cfg->>'llm_url', 'http://10.235.74.1:8000/v1');
    v_model   := COALESCE(v_cfg->>'llm_model', 'Qwen/Qwen3.5-9B');
    v_max_tok := COALESCE((v_cfg->>'max_tokens')::INT, 512);
    v_timeout := COALESCE((v_cfg->>'timeout_ms')::INT, 60000);
    v_soul    := COALESCE(v_cfg->>'soul', 'You are a helpful assistant.');

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

    -- Tool Results? (Rückgabe von tool_bee)
    v_tool_results := p_content->'tool_results';
    IF v_tool_results IS NOT NULL AND jsonb_typeof(v_tool_results) = 'array' THEN
        -- Assistant message mit tool_calls anhängen
        IF p_content ? 'assistant_message' THEN
            v_messages := v_messages || jsonb_build_array(p_content->'assistant_message');
        END IF;
        -- Jedes Tool-Result als tool message
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

    -- Tool-Call Loop Protection
    v_tool_count := COALESCE((p_content->>'tool_call_count')::int, 0);

    -- Tools aus Registry holen
    v_tools := meclaw.get_tool_definitions();

    -- Body bauen
    v_body := jsonb_build_object(
        'model', v_model,
        'max_tokens', v_max_tok,
        'chat_template_kwargs', jsonb_build_object('enable_thinking', false),
        'messages', v_messages
    );

    -- Tools mitschicken (wenn vorhanden und Loop-Limit nicht erreicht)
    IF jsonb_array_length(v_tools) > 0 AND v_tool_count < 5 THEN
        v_body := v_body || jsonb_build_object(
            'tools', v_tools,
            'tool_choice', 'auto'
        );
    END IF;

    -- Job in Staging-Tabelle
    INSERT INTO meclaw.llm_jobs (msg_id, url, body, timeout_ms)
    VALUES (p_msg_id, v_llm_url || '/chat/completions', v_body, v_timeout)
    RETURNING id INTO v_job_id;

    BEGIN
        PERFORM pg_background_launch(
            format('SELECT meclaw.llm_http_call_by_id(%L::uuid)', v_job_id)
        );
        PERFORM meclaw.log_event(p_msg_id, p_task_id, 'llm_bee', 'llm_request',
            jsonb_build_object('model', v_model, 'job_id', v_job_id, 'via', 'pg_background',
                'history_count', COALESCE(jsonb_array_length(v_history), 0),
                'tool_count', v_tool_count,
                'tools_available', jsonb_array_length(v_tools)));
    EXCEPTION WHEN OTHERS THEN
        DELETE FROM meclaw.llm_jobs WHERE id = v_job_id;
        SELECT net.http_post(
            url := v_llm_url || '/chat/completions',
            body := v_body,
            headers := '{"Content-Type": "application/json"}'::jsonb,
            timeout_milliseconds := v_timeout
        ) INTO v_net_req;
        INSERT INTO meclaw.net_requests (net_req_id, type, ref_id)
        VALUES (v_net_req, 'llm_call', p_msg_id);
        PERFORM meclaw.log_event(p_msg_id, p_task_id, 'llm_bee', 'llm_request',
            jsonb_build_object('model', v_model, 'net_req_id', v_net_req, 'via', 'direct', 'error', SQLERRM,
                'history_count', COALESCE(jsonb_array_length(v_history), 0),
                'tool_count', v_tool_count));
    END;

    UPDATE meclaw.messages
    SET status = 'waiting',
        waiting_for = 'llm_result',
        content = content || jsonb_build_object('prompt_messages', v_messages)
    WHERE id = p_msg_id;
END;
$function$;

-- =================================================================
-- 7. Dispatch Trigger: tool_bee erkennen
-- =================================================================
CREATE OR REPLACE FUNCTION meclaw.trg_on_message_ready_dispatch()
 RETURNS trigger
 LANGUAGE plpgsql
AS $function$
BEGIN
    IF NEW.status != 'ready' OR NEW.next_bee IS NULL THEN
        RETURN NEW;
    END IF;
    BEGIN
        IF NEW.next_bee LIKE '%-call-bee' THEN
            PERFORM meclaw.router_bee(NEW.id);
        ELSIF NEW.next_bee LIKE '%-context-bee' THEN
            PERFORM meclaw.context_bee(NEW.id);
        ELSIF NEW.next_bee LIKE '%-tool-bee' THEN
            PERFORM meclaw.tool_bee(NEW.id);
        ELSIF NEW.next_bee LIKE '%-llm-bee' OR NEW.next_bee LIKE '%-sender-bee' OR NEW.next_bee LIKE '%-receiver-bee' THEN
            DECLARE
                v_bee_type TEXT;
            BEGIN
                LOAD 'age';
                SET LOCAL search_path = meclaw, ag_catalog, "$user", public;
                EXECUTE format(
                    'SELECT bee_type::text FROM cypher(''meclaw_graph'', $q$
                         MATCH (b:Bee {id: %L}) RETURN b.type
                     $q$) AS (bee_type agtype) LIMIT 1',
                    NEW.next_bee
                ) INTO v_bee_type;
                v_bee_type := trim(both '"' from v_bee_type);
                CASE v_bee_type
                    WHEN 'llm_bee'      THEN PERFORM meclaw.llm_bee_v2(NEW.id, NEW.task_id, NEW.next_bee, NEW.content);
                    WHEN 'sender_bee'   THEN PERFORM meclaw.sender_bee_v2(NEW.id, NEW.task_id, NEW.content);
                    WHEN 'receiver_bee' THEN PERFORM meclaw.receiver_bee_v2(NEW.id, NEW.task_id);
                    ELSE PERFORM meclaw.router_bee(NEW.id);
                END CASE;
            END;
        ELSE
            PERFORM meclaw.router_bee(NEW.id);
        END IF;
    EXCEPTION WHEN OTHERS THEN
        INSERT INTO meclaw.events (bee_type, event, payload)
        VALUES (NEW.next_bee, 'bee_error',
            jsonb_build_object('error', SQLERRM, 'msg_id', NEW.id));
    END;
    RETURN NEW;
END;
$function$;

-- =================================================================
-- 8. Router: tool_call + tool_result Conditions
-- =================================================================
-- router_bee braucht eine Anpassung: type='tool_call' → condition 'on_tool_call'
-- und type='tool_result' → condition 'on_message' (zurück zu llm_bee)
-- Das wird über die bestehende CASE-Logik gemacht (router_bee liest v_msg.type)

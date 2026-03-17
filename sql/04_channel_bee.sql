-- MeClaw v0.1.0 — Channel Bee (Telegram Long-Poll)
CREATE OR REPLACE FUNCTION meclaw.channel_bee_start(p_channel_id uuid DEFAULT '00000000-0000-0000-0000-000000000001'::uuid, p_offset bigint DEFAULT NULL::bigint)
 RETURNS void
 LANGUAGE plpgsql
AS $function$
DECLARE
    v_config    JSONB;
    v_token     TEXT;
    v_timeout   INT;
    v_offset    BIGINT;
    v_net_req   BIGINT;
BEGIN
    SELECT config INTO v_config FROM meclaw.channels WHERE id = p_channel_id;
    v_token   := v_config->>'bot_token';
    v_timeout := COALESCE((v_config->>'poll_timeout')::INT, 25);

    -- Offset: explizit übergeben oder aus DB berechnen
    IF p_offset IS NOT NULL THEN
        v_offset := p_offset;
    ELSE
        SELECT COALESCE(MAX((content->>'telegram_update_id')::BIGINT) + 1, 0)
        INTO v_offset
        FROM meclaw.messages
        WHERE channel_id = p_channel_id
          AND type = 'user_input'
          AND content->>'telegram_update_id' IS NOT NULL;
    END IF;

    SELECT net.http_get(
        url := format(
            'https://api.telegram.org/bot%s/getUpdates?timeout=%s&offset=%s',
            v_token, v_timeout, v_offset
        ),
        timeout_milliseconds := (v_timeout + 5) * 1000
    ) INTO v_net_req;

    INSERT INTO meclaw.net_requests (net_req_id, type, ref_id)
    VALUES (v_net_req, 'telegram_poll', p_channel_id)
    ON CONFLICT (net_req_id) DO NOTHING;

    PERFORM meclaw.log_event(NULL, NULL, 'receiver_bee', 'poll_started',
        jsonb_build_object('net_req_id', v_net_req, 'offset', v_offset));
END;
$function$

;
CREATE OR REPLACE FUNCTION meclaw.on_net_response()
 RETURNS trigger
 LANGUAGE plpgsql
AS $function$
BEGIN
    BEGIN
        PERFORM meclaw.on_net_response_safe(NEW.id, NEW.status_code, NEW.content::text, NEW.error_msg);
    EXCEPTION WHEN OTHERS THEN
        INSERT INTO meclaw.events (bee_type, event, payload)
        VALUES ('on_net_response', 'trigger_error',
            jsonb_build_object('error', SQLERRM, 'net_id', NEW.id));
    END;
    RETURN NEW;
END;
$function$

;

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
$function$

;


-- MeClaw v0.1.0 — Sender & Receiver Bees
CREATE OR REPLACE FUNCTION meclaw.sender_bee_v2(p_msg_id uuid, p_task_id uuid, p_content jsonb)
 RETURNS void
 LANGUAGE plpgsql
AS $function$
DECLARE
    v_config       JSONB;
    v_token        TEXT;
    v_chat_id      TEXT;
    v_output       TEXT;
    v_net_req      BIGINT;
    v_channel_type TEXT;
BEGIN
    v_output  := p_content->>'output';
    v_chat_id := p_content->>'telegram_chat_id';

    -- Fallback: llm_result aus task holen
    IF v_output IS NULL OR v_output = '' THEN
        SELECT m.content->>'output', m.content->>'telegram_chat_id'
        INTO v_output, v_chat_id
        FROM meclaw.messages m
        WHERE m.task_id = p_task_id
          AND m.type = 'llm_result'
        ORDER BY m.created_at DESC LIMIT 1;
    END IF;

    -- Channel-Type prüfen
    SELECT c.type INTO v_channel_type FROM meclaw.channels c
    WHERE c.id = (SELECT channel_id FROM meclaw.messages WHERE id = p_msg_id);

    SELECT config INTO v_config FROM meclaw.channels
    WHERE id = (SELECT channel_id FROM meclaw.messages WHERE id = p_msg_id);
    v_token := v_config->>'bot_token';

    UPDATE meclaw.messages SET status='running', assigned_to='main-sender-bee'
    WHERE id = p_msg_id;

    IF v_output IS NOT NULL AND v_output != ''
       AND v_chat_id IS NOT NULL AND v_chat_id != ''
       AND v_channel_type = 'telegram' THEN
        SELECT net.http_post(
            url     := format('https://api.telegram.org/bot%s/sendMessage', v_token),
            body    := jsonb_build_object('chat_id', v_chat_id, 'text', v_output),
            headers := '{"Content-Type": "application/json"}'::jsonb,
            timeout_milliseconds := 10000
        ) INTO v_net_req;

        INSERT INTO meclaw.net_requests (net_req_id, type, ref_id)
        VALUES (v_net_req, 'telegram_send', p_msg_id)
        ON CONFLICT (net_req_id) DO NOTHING;

        PERFORM meclaw.log_event(p_msg_id, p_task_id, 'sender_bee', 'send_triggered',
            jsonb_build_object('chat_id', v_chat_id, 'net_req_id', v_net_req));
    ELSE
        PERFORM meclaw.log_event(p_msg_id, p_task_id, 'sender_bee', 'send_skipped',
            jsonb_build_object('reason',
                CASE
                    WHEN v_channel_type != 'telegram' THEN 'non-telegram channel (' || COALESCE(v_channel_type, 'unknown') || ')'
                    WHEN v_output IS NULL OR v_output = '' THEN 'no output'
                    ELSE 'no chat_id'
                END));
    END IF;

    UPDATE meclaw.messages SET status='done' WHERE id = p_msg_id;
END;
$function$;

CREATE OR REPLACE FUNCTION meclaw.receiver_bee_v2(p_msg_id uuid, p_task_id uuid)
 RETURNS void
 LANGUAGE plpgsql
AS $function$
BEGIN
    UPDATE meclaw.messages SET status='running', assigned_to='main-receiver-bee'
    WHERE id = p_msg_id;

    -- Long-Poll — ALLEIN im nächsten Commit-Batch
    PERFORM meclaw.channel_bee_start();

    -- Done — Task komplett abgeschlossen
    UPDATE meclaw.messages SET status='done' WHERE id = p_msg_id;
    UPDATE meclaw.tasks SET status='done', updated_at=clock_timestamp()
    WHERE id = p_task_id;

    PERFORM meclaw.log_event(p_msg_id, p_task_id, 'receiver_bee', 'poll_restarted', '{}');
END;
$function$;

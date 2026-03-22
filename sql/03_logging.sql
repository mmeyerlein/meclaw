-- MeClaw v0.1.0 — Logging & Utility
CREATE OR REPLACE FUNCTION meclaw.log_event(p_msg_id uuid, p_task_id uuid, p_bee_type text, p_event text, p_payload jsonb DEFAULT '{}'::jsonb)
 RETURNS void
 LANGUAGE sql
AS $function$
    INSERT INTO meclaw.events (msg_id, task_id, bee_type, event, payload)
    VALUES (p_msg_id, p_task_id, p_bee_type, p_event, p_payload);
$function$

;

CREATE OR REPLACE FUNCTION meclaw.auto_log_message()
 RETURNS trigger
 LANGUAGE plpgsql
AS $function$
DECLARE
    v_event TEXT;
BEGIN
    IF TG_OP = 'INSERT' THEN
        v_event := 'message_created';
    ELSIF TG_OP = 'UPDATE' THEN
        IF OLD.status != NEW.status THEN
            v_event := 'status_changed.' || OLD.status || '->' || NEW.status;
        ELSE
            RETURN NEW;
        END IF;
    END IF;

    INSERT INTO meclaw.events (msg_id, task_id, bee_type, event, payload)
    VALUES (
        NEW.id, NEW.task_id,
        COALESCE(NEW.assigned_to, NEW.sender, 'system'),
        v_event,
        jsonb_build_object(
            'type',        NEW.type,
            'status',      NEW.status,
            'next_bee',    NEW.next_bee,
            'assigned_to', NEW.assigned_to,
            'waiting_for', NEW.waiting_for,
            'sender',      NEW.sender,
            'channel_id',  NEW.channel_id
        )
    );
    RETURN NEW;
END;
$function$

;

CREATE OR REPLACE FUNCTION meclaw.trace(p_msg_id uuid)
 RETURNS TABLE(id bigint, event text, bee_type text, payload jsonb, created_at timestamp with time zone)
 LANGUAGE sql
AS $function$
    SELECT id, event, bee_type, payload, created_at
    FROM meclaw.events
    WHERE msg_id = p_msg_id
    ORDER BY id;
$function$

;

CREATE OR REPLACE FUNCTION meclaw.watchdog()
 RETURNS void
 LANGUAGE plpgsql
AS $function$
DECLARE
    v_poll_running BOOLEAN;
BEGIN
    -- Check whether a long-poll is active
    SELECT EXISTS (
        SELECT 1 FROM meclaw.net_requests WHERE type = 'telegram_poll'
    ) INTO v_poll_running;

    IF NOT v_poll_running THEN
        PERFORM meclaw.log_event(NULL, NULL, 'watchdog', 'restarting_poll', '{}');
        PERFORM meclaw.channel_bee_start();
    END IF;
END;
$function$

;

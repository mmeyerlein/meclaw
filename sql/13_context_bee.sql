-- get_conversation_history: builds OpenAI messages[] from channel_conversation view
CREATE OR REPLACE FUNCTION meclaw.get_conversation_history(
    p_channel_id uuid,
    p_hours integer DEFAULT 24,
    p_max_messages integer DEFAULT 100
) RETURNS jsonb
LANGUAGE plpgsql AS $$
DECLARE
    v_history jsonb := '[]'::jsonb;
    v_rec record;
    v_cutoff timestamptz;
BEGIN
    v_cutoff := clock_timestamp() - make_interval(hours => p_hours);

    FOR v_rec IN
        SELECT role, text, created_at
        FROM meclaw.channel_conversation
        WHERE channel_id = p_channel_id
        AND created_at >= v_cutoff
        AND text IS NOT NULL
        AND text != ''
        ORDER BY created_at DESC
        LIMIT p_max_messages
    LOOP
        v_history := jsonb_build_object('role', v_rec.role, 'content', v_rec.text) || v_history;
    END LOOP;

    RETURN v_history;
END;
$$;

-- Context Bee: History-Länge abhängig vom Modell-Tier
-- small=5, medium=15, large=30, reasoning=50
-- Liest model_id aus der LLM-Bee Config im selben Hive

CREATE OR REPLACE FUNCTION meclaw.context_bee(p_msg_id uuid)
 RETURNS void
 LANGUAGE plpgsql
AS $function$
DECLARE
    v_msg record; v_channel_id UUID; v_history jsonb; v_content jsonb; v_current_input text;
    v_model_id text; v_tier text; v_max_messages integer;
    v_hive_name text; v_llm_config text;
BEGIN
    SELECT * INTO v_msg FROM meclaw.messages WHERE id = p_msg_id;
    IF NOT FOUND THEN RAISE EXCEPTION 'context_bee: message % not found', p_msg_id; END IF;

    v_channel_id := v_msg.channel_id;
    IF v_channel_id IS NULL THEN
        SELECT channel_id INTO v_channel_id FROM meclaw.tasks WHERE id = v_msg.task_id;
    END IF;

    -- model_id aus LLM-Bee im selben Hive holen
    v_model_id := NULL;
    BEGIN
        LOAD 'age';
        SET LOCAL search_path = ag_catalog, meclaw, public;
        
        -- Finde LLM-Bee Config im Graph (Bee mit type='llm_bee')
        SELECT trim(both '"' from c::text) INTO v_llm_config
        FROM cypher('meclaw_graph', $$
            MATCH (b:Bee)
            WHERE b.type = 'llm_bee'
            RETURN b.config
        $$) AS (c agtype)
        LIMIT 1;
        
        IF v_llm_config IS NOT NULL THEN
            v_model_id := (v_llm_config::jsonb)->>'model_id';
        END IF;
    EXCEPTION WHEN OTHERS THEN
        v_model_id := NULL;
    END;

    -- Tier auflösen
    IF v_model_id IS NOT NULL THEN
        SELECT tier INTO v_tier FROM meclaw.llm_models WHERE id = v_model_id;
    END IF;

    -- History-Länge nach Tier
    v_max_messages := CASE v_tier
        WHEN 'small'     THEN 5
        WHEN 'medium'    THEN 15
        WHEN 'large'     THEN 30
        WHEN 'reasoning' THEN 50
        ELSE 10  -- default fallback
    END;

    v_current_input := v_msg.content->>'input';
    v_history := meclaw.get_conversation_history(v_channel_id, 24, v_max_messages);

    -- Aktuelle Nachricht aus History entfernen (Duplikat vermeiden)
    IF jsonb_array_length(v_history) > 0
       AND v_history->-1->>'content' = v_current_input
       AND v_history->-1->>'role' = 'user' THEN
        v_history := v_history - (jsonb_array_length(v_history) - 1);
    END IF;

    PERFORM meclaw.log_event(p_msg_id, v_msg.task_id, 'context_bee', 'context_loaded',
        jsonb_build_object(
            'channel_id', v_channel_id,
            'history_count', jsonb_array_length(v_history),
            'model_id', COALESCE(v_model_id, 'unknown'),
            'tier', COALESCE(v_tier, 'default'),
            'max_messages', v_max_messages
        ));

    v_content := v_msg.content || jsonb_build_object('conversation_history', v_history);

    INSERT INTO meclaw.messages (task_id, channel_id, previous_id, type, sender, status, content)
    VALUES (v_msg.task_id, v_channel_id, p_msg_id, 'routing', 'context_bee', 'done', v_content);

    UPDATE meclaw.messages SET status = 'done' WHERE id = p_msg_id AND status != 'done';
END;
$function$;

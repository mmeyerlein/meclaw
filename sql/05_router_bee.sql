-- MeClaw v0.1.0 — Router Bee (AGE Graph Routing)
CREATE OR REPLACE FUNCTION meclaw.router_bee(p_msg_id uuid)
 RETURNS void
 LANGUAGE plpgsql
AS $function$
DECLARE
    v_msg       RECORD;
    v_content   JSONB;
    v_stack     JSONB;
    v_from_bee  TEXT;
    v_condition TEXT;
    v_next_bee  TEXT;
    v_next_type TEXT;
    v_new_id    UUID;
    v_frame     JSONB;
    v_return_bee TEXT;
    v_new_stack  JSONB;
    v_cfg        TEXT;
    v_target_hive TEXT;
    v_entry_bee  TEXT;
    v_entry_type TEXT;
    v_new_stack2 JSONB;
    v_new_content JSONB;
BEGIN
    LOAD 'age';
    SET LOCAL search_path = meclaw, ag_catalog, "$user", public;

    SELECT * INTO v_msg FROM meclaw.messages WHERE id = p_msg_id;
    IF NOT FOUND THEN RETURN; END IF;

    v_content  := v_msg.content;
    v_stack    := COALESCE(v_content->'stack', '[]'::jsonb);
    v_from_bee := COALESCE(v_content->>'current_bee', 'main-receiver-bee');

    v_condition := CASE v_msg.type
        WHEN 'user_input'   THEN 'on_message'
        WHEN 'llm_result'   THEN 'on_return'
        WHEN 'tool_call'    THEN 'on_tool_call'
        WHEN 'tool_result'  THEN 'on_tool_result'
        WHEN 'agent_output' THEN 'on_return'
        WHEN 'routing'      THEN COALESCE(v_content->>'routing_condition', 'on_message')
        ELSE 'default'
    END;

    PERFORM meclaw.log_event(p_msg_id, v_msg.task_id, 'router_bee', 'routing',
        jsonb_build_object('from_bee', v_from_bee, 'condition', v_condition));

    EXECUTE format(
        'SELECT bee_id::text, bee_type::text
         FROM cypher(''meclaw_graph'', $q$
             MATCH (a:Bee {id: %L})-[e:NEXT {condition: %L}]->(b:Bee)
             RETURN b.id, b.type
         $q$) AS (bee_id agtype, bee_type agtype) LIMIT 1',
        v_from_bee, v_condition
    ) INTO v_next_bee, v_next_type;

    v_next_bee  := trim(both '"' from v_next_bee);
    v_next_type := trim(both '"' from v_next_type);

    -- ── Kein nächster Node → Stack poppen oder done ────────
    IF v_next_bee IS NULL THEN
        IF jsonb_array_length(v_stack) > 0 THEN
            v_frame      := v_stack->-1;
            v_return_bee := v_frame->>'return_bee';
            v_new_stack  := v_stack - (jsonb_array_length(v_stack) - 1);

            v_new_content := v_content || jsonb_build_object(
                'current_bee',       v_return_bee,
                'routing_condition', 'on_return',
                'stack',             v_new_stack
            );

            INSERT INTO meclaw.messages (
                task_id, channel_id, previous_id, type, sender, status, next_bee, content
            )
            SELECT task_id, channel_id, p_msg_id,
                'routing', 'router_bee', 'ready', v_return_bee, v_new_content
            FROM meclaw.messages WHERE id = p_msg_id
            RETURNING id INTO v_new_id;

            PERFORM meclaw.log_event(v_new_id, v_msg.task_id, 'router_bee', 'stack_return',
                jsonb_build_object('return_bee', v_return_bee));
        ELSE
            UPDATE meclaw.tasks SET status='done', updated_at=clock_timestamp()
            WHERE id = v_msg.task_id;
            PERFORM meclaw.log_event(p_msg_id, v_msg.task_id, 'router_bee', 'task_done', '{}');
        END IF;
        RETURN;
    END IF;

    -- ── call_bee: Stack pushen ──────────────────────────────
    IF v_next_type = 'call_bee' THEN
        EXECUTE format(
            'SELECT bee_config::text FROM cypher(''meclaw_graph'', $q$
                 MATCH (b:Bee {id: %L}) RETURN b.config
             $q$) AS (bee_config agtype) LIMIT 1', v_next_bee
        ) INTO v_cfg;
        v_target_hive := (v_cfg::jsonb)->>'target_hive';

        EXECUTE format(
            'SELECT bee_id::text, bee_type::text FROM cypher(''meclaw_graph'', $q$
                 MATCH (h:Hive {id: %L})-[:ENTRY]->(b:Bee)
                 RETURN b.id, b.type
             $q$) AS (bee_id agtype, bee_type agtype) LIMIT 1', v_target_hive
        ) INTO v_entry_bee, v_entry_type;

        v_entry_bee  := trim(both '"' from v_entry_bee);

        v_new_stack2 := v_stack || jsonb_build_object(
            'return_bee', v_next_bee,
            'condition',  'on_return'
        );

        v_new_content := v_content || jsonb_build_object(
            'current_bee', v_entry_bee,
            'stack',       v_new_stack2
        );

        INSERT INTO meclaw.messages (
            task_id, channel_id, previous_id, type, sender, status, next_bee, content
        )
        SELECT task_id, channel_id, p_msg_id,
            'routing', 'router_bee', 'ready', v_entry_bee, v_new_content
        FROM meclaw.messages WHERE id = p_msg_id
        RETURNING id INTO v_new_id;

        PERFORM meclaw.log_event(v_new_id, v_msg.task_id, 'router_bee', 'call',
            jsonb_build_object('target_hive', v_target_hive, 'entry_bee', v_entry_bee));
        RETURN;
    END IF;

    -- ── Normale Bee ─────────────────────────────────────────
    v_new_content := v_content || jsonb_build_object(
        'current_bee', v_next_bee
    );

    INSERT INTO meclaw.messages (
        task_id, channel_id, previous_id, type, sender, status, next_bee, content
    )
    SELECT task_id, channel_id, p_msg_id,
        'routing', 'router_bee', 'ready', v_next_bee, v_new_content
    FROM meclaw.messages WHERE id = p_msg_id
    RETURNING id INTO v_new_id;

    PERFORM meclaw.log_event(v_new_id, v_msg.task_id, 'router_bee', 'dispatched',
        jsonb_build_object('next_bee', v_next_bee, 'type', v_next_type));
END;
$function$

;

CREATE OR REPLACE FUNCTION meclaw.call_bee(p_msg_id uuid, p_task_id uuid, p_bee_id text, p_config jsonb, p_stack jsonb, p_content jsonb)
 RETURNS void
 LANGUAGE plpgsql
AS $function$
DECLARE
    v_target_hive   TEXT;
    v_entry_bee     TEXT;
    v_entry_type    TEXT;
    v_entry_cfg     TEXT;
    v_new_stack     JSONB;
    v_new_msg_id    UUID;
BEGIN
    LOAD 'age';
    SET LOCAL search_path = meclaw, ag_catalog, "$user", public;

    v_target_hive := p_config->>'target_hive';

    -- Entry Bee + Type + Config laden
    EXECUTE format(
        'SELECT bee_id::text, bee_type::text, bee_config::text
         FROM cypher(''meclaw_graph'', $q$
             MATCH (h:Hive {id: %L})-[:ENTRY]->(b:Bee)
             RETURN b.id, b.type, b.config
         $q$) AS (bee_id agtype, bee_type agtype, bee_config agtype) LIMIT 1',
        v_target_hive
    ) INTO v_entry_bee, v_entry_type, v_entry_cfg;

    v_entry_bee  := trim(both '"' from v_entry_bee);
    v_entry_type := trim(both '"' from v_entry_type);

    IF v_entry_bee IS NULL THEN
        PERFORM meclaw.log_event(p_msg_id, p_task_id, 'call_bee', 'entry_not_found',
            jsonb_build_object('target_hive', v_target_hive));
        RETURN;
    END IF;

    -- Stack pushen: Return-Adresse = call_bee
    v_new_stack := p_stack || jsonb_build_object(
        'hive',       'main-graph',
        'return_bee', p_bee_id,
        'condition',  'on_return'
    );

    -- Original-Message auf waiting setzen
    UPDATE meclaw.messages
    SET status = 'waiting', waiting_for = 'call_return'
    WHERE id = p_msg_id;

    PERFORM meclaw.log_event(p_msg_id, p_task_id, 'call_bee', 'called',
        jsonb_build_object('target_hive', v_target_hive, 'entry_bee', v_entry_bee));

    -- Entry-Bee DIREKT aufrufen (nicht über router_bee)
    CASE v_entry_type
        WHEN 'llm_bee' THEN
            -- Neue Message für LLM mit Stack
            INSERT INTO meclaw.messages (
                task_id, channel_id, previous_id, type, sender, status, content
            )
            SELECT p_task_id, channel_id, p_msg_id, 'user_input', 'call_bee', 'waiting',
                jsonb_build_object(
                    'input',            p_content->>'input',
                    'current_bee',      v_entry_bee,
                    'stack',            v_new_stack,
                    'telegram_chat_id', p_content->>'telegram_chat_id'
                )
            FROM meclaw.messages WHERE id = p_msg_id
            RETURNING id INTO v_new_msg_id;

            -- LLM direkt aufrufen
            PERFORM meclaw.llm_bee(v_new_msg_id, p_task_id, v_entry_bee,
                                   v_entry_cfg::jsonb, 
                                   (SELECT content FROM meclaw.messages WHERE id = v_new_msg_id));
        ELSE
            PERFORM meclaw.log_event(p_msg_id, p_task_id, 'call_bee', 'unknown_entry_type',
                jsonb_build_object('type', v_entry_type));
    END CASE;
END;
$function$

;

CREATE OR REPLACE FUNCTION meclaw.do_return(p_msg_id uuid, p_task_id uuid, p_stack jsonb, p_content jsonb)
 RETURNS void
 LANGUAGE plpgsql
AS $function$
DECLARE
    v_frame         JSONB;
    v_return_bee    TEXT;
    v_new_stack     JSONB;
    v_new_msg_id    UUID;
BEGIN
    -- Obersten Frame vom Stack holen
    v_frame      := p_stack->-1;  -- letztes Element
    v_return_bee := v_frame->>'return_bee';
    v_new_stack  := p_stack - (jsonb_array_length(p_stack) - 1);

    -- Neue Return-Message anlegen
    INSERT INTO meclaw.messages (
        task_id, channel_id, previous_id, type, sender, status, content
    )
    SELECT
        p_task_id,
        channel_id,
        p_msg_id,
        'agent_output',
        'router_bee',
        'ready',
        jsonb_build_object(
            'input',        p_content->>'input',
            'output',       p_content->>'output',
            'current_bee',  v_return_bee,
            'stack',        v_new_stack,
            'telegram_chat_id', p_content->>'telegram_chat_id'
        )
    FROM meclaw.messages WHERE id = p_msg_id
    RETURNING id INTO v_new_msg_id;

    PERFORM meclaw.log_event(v_new_msg_id, p_task_id, 'router_bee', 'returned',
        jsonb_build_object('return_bee', v_return_bee, 'prev_msg', p_msg_id));
END;
$function$

;


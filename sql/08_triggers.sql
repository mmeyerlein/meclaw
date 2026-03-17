-- MeClaw v0.1.0 — Triggers
CREATE OR REPLACE FUNCTION meclaw.trg_on_message_done_dispatch()
 RETURNS trigger
 LANGUAGE plpgsql
AS $function$
BEGIN
    IF NEW.status = 'done' 
       AND (OLD.status IS NULL OR OLD.status != 'done')
       AND NEW.next_bee IS NULL THEN
        BEGIN
            PERFORM meclaw.router_bee(NEW.id);
        EXCEPTION WHEN OTHERS THEN
            INSERT INTO meclaw.events (bee_type, event, payload)
            VALUES ('router_bee', 'error', jsonb_build_object('error', SQLERRM, 'msg_id', NEW.id));
        END;
    END IF;
    RETURN NEW;
END;
$function$

;

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
$function$

;

-- Trigger Definitionen
CREATE OR REPLACE TRIGGER trg_auto_log_message AFTER INSERT OR UPDATE OF status ON meclaw.messages FOR EACH ROW EXECUTE FUNCTION meclaw.auto_log_message();
CREATE OR REPLACE TRIGGER trg_message_done_dispatch AFTER INSERT OR UPDATE OF status ON meclaw.messages FOR EACH ROW EXECUTE FUNCTION meclaw.trg_on_message_done_dispatch();
CREATE OR REPLACE TRIGGER trg_message_ready_dispatch AFTER INSERT OR UPDATE OF status ON meclaw.messages FOR EACH ROW EXECUTE FUNCTION meclaw.trg_on_message_ready_dispatch();
CREATE OR REPLACE TRIGGER trg_on_net_response AFTER INSERT ON net._http_response FOR EACH ROW EXECUTE FUNCTION meclaw.on_net_response();

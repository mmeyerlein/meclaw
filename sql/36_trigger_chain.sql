-- =============================================================================
-- Phase B1: Trigger-Chain verdrahten
-- =============================================================================
-- 1. trg_extract_on_done: novelty_bee async (pg_background) nach extract_bee
-- 2. feedback_bee Signatur fix (v2 hat p_msg_id zuerst, dann p_agent_id)
-- 3. Trigger-Loop-Schutz: feedback_bee darf keine neuen Trigger auslösen
-- =============================================================================

-- =============================================================================
-- 1. Updated trg_extract_on_done: novelty_bee nach extract_bee (async)
-- =============================================================================
-- Änderungen vs. vorher:
--   + novelty_bee wird async via pg_background nach extract_bee gefeuert
--   + feedback_bee wird mit korrekter v2-Signatur aufgerufen (p_msg_id, p_agent_id)
--   + Kein Breaking Change: bestehender Flow extract_bee → bleibt identisch
-- =============================================================================

CREATE OR REPLACE FUNCTION meclaw.trg_extract_on_done()
RETURNS TRIGGER AS $$
DECLARE
    v_agent_id TEXT;
    v_brain_event_id UUID;
BEGIN
    -- Only trigger on status change to 'done'
    IF NEW.status = 'done' AND (OLD.status IS NULL OR OLD.status != 'done') THEN
        -- Only extract from user_input (user messages create brain_events)
        -- llm_result intentionally excluded: assistant responses don't add to episodic memory
        IF NEW.type = 'user_input' THEN

            -- 1. Channel-level extraction (sync, fast)
            PERFORM meclaw.extract_bee(NEW.id);

            -- 2. novelty_bee: async via pg_background
            --    brain_event wurde gerade in extract_bee erstellt → per message_id suchen
            SELECT id INTO v_brain_event_id
            FROM meclaw.brain_events
            WHERE message_id = NEW.id
            ORDER BY seq DESC
            LIMIT 1;

            IF v_brain_event_id IS NOT NULL THEN
                -- Find agent for this channel
                SELECT ac.agent_id INTO v_agent_id
                FROM meclaw.agent_channels ac
                JOIN meclaw.entities e ON e.id = ac.agent_id
                WHERE ac.channel_id = NEW.channel_id
                    AND e.entity_type = 'agent'
                LIMIT 1;

                IF v_agent_id IS NOT NULL THEN
                    -- Async: novelty_bee läuft im Hintergrund, blockiert den Trigger nicht
                    -- Exception handler: if worker slots exhausted, skip silently
                    -- (novelty can be backfilled later via run_signal_pipeline)
                    BEGIN
                        PERFORM pg_background_launch(
                            format(
                                'SELECT meclaw.novelty_bee(%L, %L)',
                                v_agent_id,
                                v_brain_event_id
                            )
                        );
                    EXCEPTION WHEN OTHERS THEN
                        -- Log but don't fail — novelty_bee is non-critical
                        NULL;
                    END;
                END IF;
            END IF;

            -- 3. feedback_bee: nur bei user_input (User reagiert auf vorherigen Event)
            --    Signatur v2: (p_msg_id UUID, p_agent_id TEXT)
            --    Trigger-Loop-Schutz: feedback_bee ändert nur brain_events.reward,
            --    kein INSERT/UPDATE auf messages → kein Loop möglich
            IF NEW.type = 'user_input' AND NEW.channel_id IS NOT NULL THEN
                -- Find agent for this channel
                IF v_agent_id IS NULL THEN
                    SELECT ac.agent_id INTO v_agent_id
                    FROM meclaw.agent_channels ac
                    JOIN meclaw.entities e ON e.id = ac.agent_id
                    WHERE ac.channel_id = NEW.channel_id
                        AND e.entity_type = 'agent'
                    LIMIT 1;
                END IF;

                IF v_agent_id IS NOT NULL THEN
                    -- feedback_bee v2 Signatur: p_msg_id zuerst, dann p_agent_id
                    PERFORM meclaw.feedback_bee(NEW.id, v_agent_id);
                END IF;
            END IF;

        END IF;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Trigger neu erstellen (DROP + CREATE ist idempotent da trg_extract_on_done bereits existiert)
DROP TRIGGER IF EXISTS trg_extract_on_done ON meclaw.messages;
CREATE TRIGGER trg_extract_on_done
    AFTER INSERT OR UPDATE OF status ON meclaw.messages
    FOR EACH ROW
    EXECUTE FUNCTION meclaw.trg_extract_on_done();

COMMENT ON FUNCTION meclaw.trg_extract_on_done IS
'Phase B1: extract_bee (sync) → novelty_bee (async, pg_background) → feedback_bee (sync, user_input only).
Loop-safe: feedback_bee modifiziert nur brain_events.reward, nicht messages.';

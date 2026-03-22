-- =============================================================================
-- Phase B1: Wire up trigger chain
-- =============================================================================
-- 1. trg_extract_on_done: novelty_bee async (pg_background) after extract_bee
-- 2. feedback_bee signature fix (v2 has p_msg_id first, then p_agent_id)
-- 3. Trigger loop protection: feedback_bee must not fire new triggers
-- =============================================================================

-- =============================================================================
-- 1. Updated trg_extract_on_done: novelty_bee after extract_bee (async)
-- =============================================================================
-- Changes vs. before:
--   + novelty_bee is fired async via pg_background after extract_bee
--   + feedback_bee is called with correct v2 signature (p_msg_id, p_agent_id)
--   + No breaking change: existing flow extract_bee → remains identical
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
            --    brain_event was just created in extract_bee → look up by message_id
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
                    -- Async: novelty_bee runs in the background, does not block the trigger
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

            -- 3. feedback_bee: only for user_input (user reacts to previous event)
            --    Signature v2: (p_msg_id UUID, p_agent_id TEXT)
            --    Trigger loop protection: feedback_bee only changes brain_events.reward,
            --    no INSERT/UPDATE on messages → no loop possible
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
                    -- feedback_bee v2 signature: p_msg_id first, then p_agent_id
                    PERFORM meclaw.feedback_bee(NEW.id, v_agent_id);
                END IF;
            END IF;

        END IF;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Recreate trigger (DROP + CREATE is idempotent since trg_extract_on_done already exists)
DROP TRIGGER IF EXISTS trg_extract_on_done ON meclaw.messages;
CREATE TRIGGER trg_extract_on_done
    AFTER INSERT OR UPDATE OF status ON meclaw.messages
    FOR EACH ROW
    EXECUTE FUNCTION meclaw.trg_extract_on_done();

COMMENT ON FUNCTION meclaw.trg_extract_on_done IS
'Phase B1: extract_bee (sync) → novelty_bee (async, pg_background) → feedback_bee (sync, user_input only).
Loop-safe: feedback_bee only modifies brain_events.reward, not messages.';

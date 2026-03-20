-- MeClaw v0.1.0 — Feedback Bee (Agent-Level)
-- Date: 2026-03-20
-- Ref: docs/BRAIN.md (Value-Aware Memory: Reward System)
--
-- Agent-level retroactive reward from user reactions.
-- When a user responds positively/negatively, the PREVIOUS event gets reward.
-- Uses simple keyword-based sentiment (Phase 1). Phase 2: LLM-based.

CREATE OR REPLACE FUNCTION meclaw.feedback_bee(p_agent_id TEXT, p_msg_id UUID)
RETURNS VOID AS $$
DECLARE
    v_content TEXT;
    v_channel_id UUID;
    v_prev_event_id UUID;
    v_reward_delta FLOAT := 0.0;
    v_sentiment TEXT := 'neutral';
BEGIN
    -- Get current message content
    SELECT content->>'input', channel_id
    INTO v_content, v_channel_id
    FROM meclaw.messages WHERE id = p_msg_id;

    IF v_content IS NULL THEN
        RETURN;
    END IF;

    -- Simple keyword-based sentiment detection (Phase 1)
    -- Positive signals
    IF v_content ~* '(genau|richtig|perfekt|super|danke|gut|nice|top|stimmt|exakt|korrekt|great|exactly|perfect|thanks|yes|ja|👍|🎉|✅|💪)' THEN
        v_reward_delta := 0.8;
        v_sentiment := 'positive';
    -- Negative signals
    ELSIF v_content ~* '(falsch|nein|wrong|no|fehler|error|stimmt nicht|quatsch|blödsinn|unsinn|👎|❌|nicht richtig|das ist falsch)' THEN
        v_reward_delta := -0.8;
        v_sentiment := 'negative';
    -- Correction signals (moderate negative)
    ELSIF v_content ~* '(aber|eigentlich|naja|hmm|nicht ganz|fast|eher|correction)' THEN
        v_reward_delta := -0.3;
        v_sentiment := 'correction';
    ELSE
        -- Neutral: no reward change
        RETURN;
    END IF;

    -- Find the most recent brain_event from the assistant in this channel
    -- (the event that the user is reacting to)
    SELECT be.id INTO v_prev_event_id
    FROM meclaw.brain_events be
    JOIN meclaw.messages m ON m.id = be.message_id
    WHERE be.channel_id = v_channel_id
        AND m.type = 'llm_result'
        AND be.seq < (SELECT COALESCE(MAX(seq), 0) FROM meclaw.brain_events WHERE message_id = p_msg_id)
    ORDER BY be.seq DESC
    LIMIT 1;

    IF v_prev_event_id IS NULL THEN
        RETURN;
    END IF;

    -- Apply reward to the previous event
    UPDATE meclaw.brain_events
    SET reward = reward + v_reward_delta,
        reward_updated_seq = (SELECT COALESCE(MAX(seq), 0) FROM meclaw.brain_events)
    WHERE id = v_prev_event_id;

    -- Log
    INSERT INTO meclaw.events (msg_id, bee_type, event, payload)
    VALUES (p_msg_id, 'feedback_bee', 'reward_applied', jsonb_build_object(
        'agent_id', p_agent_id,
        'target_event_id', v_prev_event_id,
        'reward_delta', v_reward_delta,
        'sentiment', v_sentiment,
        'trigger_content', left(v_content, 100)
    ));
END;
$$ LANGUAGE plpgsql;

-- =============================================================================
-- Integration: call feedback_bee from extract trigger
-- =============================================================================

-- Update the extract trigger to also call feedback_bee for user_input messages
CREATE OR REPLACE FUNCTION meclaw.trg_extract_on_done()
RETURNS TRIGGER AS $$
DECLARE
    v_agent_id TEXT;
BEGIN
    IF NEW.status = 'done' AND (OLD.status IS NULL OR OLD.status != 'done') THEN
        IF NEW.type IN ('user_input', 'llm_result') THEN
            -- Channel-level extraction
            PERFORM meclaw.extract_bee(NEW.id);

            -- Agent-level feedback (only on user_input — user reacting to previous)
            IF NEW.type = 'user_input' AND NEW.channel_id IS NOT NULL THEN
                -- Find the agent for this channel
                SELECT ac.agent_id INTO v_agent_id
                FROM meclaw.agent_channels ac
                JOIN meclaw.entities e ON e.id = ac.agent_id
                WHERE ac.channel_id = NEW.channel_id
                    AND e.entity_type = 'agent'
                LIMIT 1;

                IF v_agent_id IS NOT NULL THEN
                    PERFORM meclaw.feedback_bee(v_agent_id, NEW.id);
                END IF;
            END IF;
        END IF;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION meclaw.feedback_bee IS
'Agent-level retroactive reward. Detects positive/negative sentiment in user messages
and applies reward to the previous assistant event. Phase 1: keyword-based.';

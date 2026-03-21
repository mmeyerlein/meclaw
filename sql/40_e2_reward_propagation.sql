-- =============================================================================
-- E2: Reward Propagation (Discounted Returns)
-- =============================================================================
-- Extends feedback_bee with backward reward propagation using discount factor γ=0.9.
-- When an event receives reward δ, the previous N events in the same channel
-- also get discounted reward:
--   event-1: δ * 0.9^1
--   event-2: δ * 0.9^2
--   ...up to depth 5
--
-- Safety: propagation is a single UPDATE with no recursive triggers.
-- =============================================================================

CREATE OR REPLACE FUNCTION meclaw.feedback_bee(p_msg_id UUID, p_agent_id TEXT)
RETURNS VOID AS $$
DECLARE
    v_content        TEXT;
    v_channel_id     UUID;
    v_prev_event_id  UUID;
    v_prev_seq       BIGINT;
    v_reward_delta   FLOAT  := 0.0;
    v_sentiment      TEXT   := 'neutral';
    v_discount       FLOAT  := 0.9;
    v_propagate_depth INT   := 5;
    v_msg_type       TEXT;
    v_current_seq    BIGINT;
BEGIN
    SELECT content->>'input', channel_id, type
    INTO v_content, v_channel_id, v_msg_type
    FROM meclaw.messages WHERE id = p_msg_id;

    IF v_content IS NULL OR v_msg_type != 'user_input' THEN RETURN; END IF;

    -- Skip very short messages (ambiguous: "ok", "ja" etc.)
    IF length(v_content) < 3 THEN RETURN; END IF;

    -- -------------------------------------------------------------------------
    -- Stage 1: Fast keyword sentiment detection
    -- -------------------------------------------------------------------------
    IF v_content ~* '(genau|richtig|perfekt|super|danke|gut|nice|top|stimmt|exakt|korrekt|great|exactly|perfect|thanks|👍|🎉|✅|💪)' THEN
        -- Negation check first
        IF v_content ~* '(nicht\s+(genau|richtig|perfekt|super|gut|korrekt)|stimmt\s+nicht|nein.*aber|ja.*falsch|ja.*stimmt\s+nicht)' THEN
            v_reward_delta := -0.5; v_sentiment := 'negated_positive';
        ELSE
            v_reward_delta := 0.8;  v_sentiment := 'positive';
        END IF;
    ELSIF v_content ~* '(falsch|nein|wrong|no|fehler|error|stimmt nicht|quatsch|blödsinn|unsinn|👎|❌|nicht richtig|das ist falsch)' THEN
        v_reward_delta := -0.8; v_sentiment := 'negative';
    ELSIF v_content ~* '(aber|eigentlich|naja|hmm|nicht ganz|fast|eher|correction|nee)' THEN
        v_sentiment := 'ambiguous';
    ELSE
        RETURN; -- No sentiment signal → skip
    END IF;

    -- -------------------------------------------------------------------------
    -- Stage 2: LLM sentiment for ambiguous cases
    -- -------------------------------------------------------------------------
    IF v_sentiment = 'ambiguous' THEN
        BEGIN
            SELECT * FROM meclaw.llm_sentiment(v_content) INTO v_sentiment, v_reward_delta;
        EXCEPTION WHEN OTHERS THEN
            v_reward_delta := -0.3; v_sentiment := 'correction_fallback';
        END;
        IF v_sentiment = 'neutral' THEN RETURN; END IF;
    END IF;

    -- -------------------------------------------------------------------------
    -- Find the seq of the current message's brain_event (upper bound)
    -- -------------------------------------------------------------------------
    SELECT COALESCE(MAX(be.seq), 0)
    INTO v_current_seq
    FROM meclaw.brain_events be
    WHERE be.message_id = p_msg_id;

    -- -------------------------------------------------------------------------
    -- Find the most recent assistant brain_event before the current user message
    -- -------------------------------------------------------------------------
    SELECT be.id, be.seq
    INTO v_prev_event_id, v_prev_seq
    FROM meclaw.brain_events be
    JOIN meclaw.messages m ON m.id = be.message_id
    WHERE be.channel_id = v_channel_id
        AND m.type = 'llm_result'
        AND be.seq < v_current_seq
    ORDER BY be.seq DESC
    LIMIT 1;

    IF v_prev_event_id IS NULL THEN RETURN; END IF;

    -- -------------------------------------------------------------------------
    -- Direct reward on the immediately preceding assistant event
    -- -------------------------------------------------------------------------
    UPDATE meclaw.brain_events
    SET reward             = reward + v_reward_delta,
        reward_updated_seq = (SELECT COALESCE(MAX(seq), 0) FROM meclaw.brain_events)
    WHERE id = v_prev_event_id;

    -- -------------------------------------------------------------------------
    -- Backward reward propagation with discount γ=0.9, depth=5
    --
    -- Finds up to v_propagate_depth events in the same channel with seq < v_prev_seq
    -- (i.e., the history before the direct reward target).
    -- Each gets: reward += v_reward_delta * γ^rn  (rn = 1..5)
    --
    -- Uses a single UPDATE — no loops, no recursive triggers.
    -- -------------------------------------------------------------------------
    UPDATE meclaw.brain_events be
    SET reward             = be.reward + (v_reward_delta * POWER(v_discount, chain.rn)),
        reward_updated_seq = (SELECT COALESCE(MAX(be3.seq), 0) FROM meclaw.brain_events be3)
    FROM (
        SELECT be2.id,
               ROW_NUMBER() OVER (ORDER BY be2.seq DESC) AS rn
        FROM meclaw.brain_events be2
        WHERE be2.channel_id = v_channel_id
          AND be2.seq < v_prev_seq
        ORDER BY be2.seq DESC
        LIMIT v_propagate_depth
    ) chain
    WHERE be.id = chain.id;

    -- -------------------------------------------------------------------------
    -- Hebbian learning: co-activated prototypes get weight boost
    -- -------------------------------------------------------------------------
    PERFORM meclaw.hebbian_update(v_prev_event_id, v_reward_delta);

    -- -------------------------------------------------------------------------
    -- Audit log
    -- -------------------------------------------------------------------------
    INSERT INTO meclaw.events (msg_id, bee_type, event, payload)
    VALUES (
        p_msg_id,
        'feedback_bee',
        'reward_applied',
        jsonb_build_object(
            'agent_id',          p_agent_id,
            'target_event_id',   v_prev_event_id,
            'reward_delta',      v_reward_delta,
            'sentiment',         v_sentiment,
            'propagation_depth', v_propagate_depth,
            'discount',          v_discount
        )
    );
END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION meclaw.feedback_bee IS
'E2: Reward Propagation. Detects sentiment in user messages and applies
discounted backward reward to the chain of preceding assistant events.
Direct event: +δ. Previous events: +δ*γ^1 .. +δ*γ^5 (γ=0.9, depth=5).
Phase 1: keyword-based, Phase 2: LLM for ambiguous cases.';

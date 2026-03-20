-- =============================================================================
-- Phase 7a: Smoke Tests — SELECT meclaw.run_smoke_tests();
-- =============================================================================
-- Three levels:
--   1. Schema smoke (tables, functions, indices, pg_cron, AGE)
--   2. Pipeline smoke (message → extract → embed → LLM extract → retrieve)
--   3. Function unit tests (resolve_entity, llm_sentiment, etc.)
-- =============================================================================

CREATE OR REPLACE FUNCTION meclaw.run_smoke_tests()
RETURNS TEXT AS $fn$
DECLARE
    v_pass INT := 0;
    v_fail INT := 0;
    v_results TEXT[] := '{}';
    v_val TEXT;
    v_count INT;
    v_float FLOAT;
    v_uuid UUID;
    v_vec vector(1536);
    v_sentiment TEXT;
    v_reward FLOAT;
    v_entity_id TEXT;
BEGIN
    -- Reset search_path to avoid AGE conflicts
    SET search_path = meclaw, public, pg_catalog;

    -- =========================================================================
    -- LEVEL 1: Schema Smoke Tests
    -- =========================================================================

    -- 1.1 Core tables exist
    FOR v_val IN
        SELECT unnest(ARRAY[
            'messages', 'tasks', 'channels', 'llm_jobs', 'events',
            'brain_events', 'entities', 'entity_observations', 'prototypes',
            'prototype_associations', 'decision_traces', 'agent_channels',
            'entity_events', 'embedding_cache', 'llm_providers'
        ])
    LOOP
        IF EXISTS (SELECT 1 FROM information_schema.tables WHERE table_schema = 'meclaw' AND table_name = v_val) THEN
            v_pass := v_pass + 1;
        ELSE
            v_fail := v_fail + 1;
            v_results := array_append(v_results, 'FAIL: table meclaw.' || v_val || ' missing');
        END IF;
    END LOOP;

    -- 1.2 Core functions exist
    FOR v_val IN
        SELECT unnest(ARRAY[
            'extract_bee', 'retrieve_bee', 'context_bee_v2', 'novelty_bee',
            'feedback_bee', 'consolidation_bee', 'compute_embedding',
            'compute_embeddings_batch', 'get_query_embedding', 'ctm_retrieve',
            'llm_extract_entities', 'create_or_resolve_entity', 'resolve_entity',
            'get_entity', 'observe_entity', 'personality_fit', 'llm_sentiment',
            'hebbian_update', 'backfill_extractions', 'discover_agents',
            'share_channel', 'cross_agent_retrieve', 'age_upsert_entity',
            'age_link_entity_event', 'age_link_entities', 'run_smoke_tests'
        ])
    LOOP
        IF EXISTS (SELECT 1 FROM pg_proc p JOIN pg_namespace n ON p.pronamespace = n.oid WHERE n.nspname = 'meclaw' AND p.proname = v_val) THEN
            v_pass := v_pass + 1;
        ELSE
            v_fail := v_fail + 1;
            v_results := array_append(v_results, 'FAIL: function meclaw.' || v_val || '() missing');
        END IF;
    END LOOP;

    -- 1.3 pg_cron jobs active
    SELECT COUNT(*) INTO v_count FROM cron.job WHERE active = true;
    IF v_count >= 3 THEN
        v_pass := v_pass + 1;
    ELSE
        v_fail := v_fail + 1;
        v_results := array_append(v_results, 'FAIL: expected >= 3 active pg_cron jobs, got ' || v_count);
    END IF;

    -- 1.4 AGE graph exists
    IF EXISTS (SELECT 1 FROM ag_catalog.ag_graph WHERE name::text = 'meclaw_graph') THEN
        v_pass := v_pass + 1;
    ELSE
        v_fail := v_fail + 1;
        v_results := array_append(v_results, 'FAIL: AGE graph meclaw_graph missing');
    END IF;

    -- 1.5 Core entities seeded
    FOR v_val IN
        SELECT unnest(ARRAY[
            'meclaw:agent:system', 'meclaw:agent:walter',
            'meclaw:person:marcus-meyer', 'meclaw:workspace:default'
        ])
    LOOP
        IF EXISTS (SELECT 1 FROM meclaw.entities WHERE id = v_val) THEN
            v_pass := v_pass + 1;
        ELSE
            v_fail := v_fail + 1;
            v_results := array_append(v_results, 'FAIL: entity ' || v_val || ' missing');
        END IF;
    END LOOP;

    -- 1.6 LLM providers configured
    FOR v_val IN
        SELECT unnest(ARRAY['openrouter', 'embedding-openrouter'])
    LOOP
        IF EXISTS (SELECT 1 FROM meclaw.llm_providers WHERE id = v_val AND api_key IS NOT NULL) THEN
            v_pass := v_pass + 1;
        ELSE
            v_fail := v_fail + 1;
            v_results := array_append(v_results, 'FAIL: llm_provider ' || v_val || ' missing or no api_key');
        END IF;
    END LOOP;

    -- 1.7 Key indices exist
    FOR v_val IN
        SELECT unnest(ARRAY[
            'idx_brain_events_unextracted',
            'idx_entity_events_event',
            'idx_entity_events_entity',
            'idx_embedding_cache_created'
        ])
    LOOP
        IF EXISTS (SELECT 1 FROM pg_indexes WHERE schemaname = 'meclaw' AND indexname = v_val) THEN
            v_pass := v_pass + 1;
        ELSE
            v_fail := v_fail + 1;
            v_results := array_append(v_results, 'FAIL: index ' || v_val || ' missing');
        END IF;
    END LOOP;

    -- =========================================================================
    -- LEVEL 2: Pipeline Smoke Tests
    -- =========================================================================

    -- 2.1 brain_events exist (extract_bee has fired at least once)
    SELECT COUNT(*) INTO v_count FROM meclaw.brain_events;
    IF v_count > 0 THEN
        v_pass := v_pass + 1;
    ELSE
        v_fail := v_fail + 1;
        v_results := array_append(v_results, 'FAIL: no brain_events (extract_bee never fired?)');
    END IF;

    -- 2.2 At least some events have embeddings
    SELECT COUNT(*) INTO v_count FROM meclaw.brain_events WHERE embedding IS NOT NULL;
    IF v_count > 0 THEN
        v_pass := v_pass + 1;
    ELSE
        v_fail := v_fail + 1;
        v_results := array_append(v_results, 'FAIL: no brain_events with embeddings');
    END IF;

    -- 2.3 At least some events have been LLM-extracted
    SELECT COUNT(*) INTO v_count FROM meclaw.brain_events WHERE extracted = TRUE AND (extraction_data)::jsonb ? 'entities_found';
    IF v_count > 0 THEN
        v_pass := v_pass + 1;
    ELSE
        v_fail := v_fail + 1;
        v_results := array_append(v_results, 'FAIL: no LLM-extracted brain_events');
    END IF;

    -- 2.4 retrieve_bee returns results
    BEGIN
        SELECT COUNT(*) INTO v_count FROM meclaw.retrieve_bee('meclaw:agent:walter', 'test', 1);
        IF v_count > 0 THEN
            v_pass := v_pass + 1;
        ELSE
            v_fail := v_fail + 1;
            v_results := array_append(v_results, 'FAIL: retrieve_bee returned 0 results');
        END IF;
    EXCEPTION WHEN OTHERS THEN
        v_fail := v_fail + 1;
        v_results := array_append(v_results, 'FAIL: retrieve_bee threw: ' || SQLERRM);
    END;

    -- 2.5 entity_events have been created (LLM extraction linked entities)
    SELECT COUNT(*) INTO v_count FROM meclaw.entity_events;
    IF v_count > 0 THEN
        v_pass := v_pass + 1;
    ELSE
        v_fail := v_fail + 1;
        v_results := array_append(v_results, 'FAIL: no entity_events (LLM extraction not linking?)');
    END IF;

    -- =========================================================================
    -- LEVEL 3: Function Unit Tests
    -- =========================================================================

    -- 3.1 resolve_entity: known entity
    IF meclaw.resolve_entity('Marcus') = 'meclaw:person:marcus-meyer'
       OR meclaw.resolve_entity('Marcus Meyer') = 'meclaw:person:marcus-meyer' THEN
        v_pass := v_pass + 1;
    ELSE
        v_fail := v_fail + 1;
        v_results := array_append(v_results, 'FAIL: resolve_entity(Marcus) != meclaw:person:marcus-meyer, got: ' || COALESCE(meclaw.resolve_entity('Marcus'), 'NULL'));
    END IF;

    -- 3.2 resolve_entity: known agent
    IF meclaw.resolve_entity('Walter') = 'meclaw:agent:walter' THEN
        v_pass := v_pass + 1;
    ELSE
        v_fail := v_fail + 1;
        v_results := array_append(v_results, 'FAIL: resolve_entity(Walter) != meclaw:agent:walter');
    END IF;

    -- 3.3 resolve_entity: unknown returns NULL
    IF meclaw.resolve_entity('xyzzy_nonexistent_42') IS NULL THEN
        v_pass := v_pass + 1;
    ELSE
        v_fail := v_fail + 1;
        v_results := array_append(v_results, 'FAIL: resolve_entity(nonexistent) should be NULL');
    END IF;

    -- 3.4 create_or_resolve_entity: existing entity (no duplicate)
    v_entity_id := meclaw.create_or_resolve_entity('Marcus Meyer', 'person');
    IF v_entity_id = 'meclaw:person:marcus-meyer' THEN
        v_pass := v_pass + 1;
    ELSE
        v_fail := v_fail + 1;
        v_results := array_append(v_results, 'FAIL: create_or_resolve_entity(Marcus Meyer) created duplicate: ' || COALESCE(v_entity_id, 'NULL'));
    END IF;

    -- 3.5 create_or_resolve_entity: new entity
    v_entity_id := meclaw.create_or_resolve_entity('Smoke Test Entity XYZ', 'concept');
    IF v_entity_id IS NOT NULL AND v_entity_id LIKE 'meclaw:concept:%' THEN
        v_pass := v_pass + 1;
        -- Cleanup
        DELETE FROM meclaw.entity_events WHERE entity_id = v_entity_id;
        DELETE FROM meclaw.prototypes WHERE id = v_entity_id;
        DELETE FROM meclaw.entities WHERE id = v_entity_id;
    ELSE
        v_fail := v_fail + 1;
        v_results := array_append(v_results, 'FAIL: create_or_resolve_entity(new) returned: ' || COALESCE(v_entity_id, 'NULL'));
    END IF;

    -- 3.6 get_query_embedding: returns vector + caches
    BEGIN
        v_vec := meclaw.get_query_embedding('smoke test embedding query');
        IF v_vec IS NOT NULL THEN
            v_pass := v_pass + 1;
            -- Check it was cached
            SELECT COUNT(*) INTO v_count FROM meclaw.embedding_cache WHERE query_text LIKE 'smoke test%';
            IF v_count > 0 THEN
                v_pass := v_pass + 1;
            ELSE
                v_fail := v_fail + 1;
                v_results := array_append(v_results, 'FAIL: get_query_embedding did not cache result');
            END IF;
            -- Cleanup cache entry
            DELETE FROM meclaw.embedding_cache WHERE query_text LIKE 'smoke test%';
        ELSE
            v_fail := v_fail + 1;
            v_results := array_append(v_results, 'FAIL: get_query_embedding returned NULL');
        END IF;
    EXCEPTION WHEN OTHERS THEN
        v_fail := v_fail + 1;
        v_results := array_append(v_results, 'FAIL: get_query_embedding threw: ' || SQLERRM);
    END;

    -- 3.7 personality_fit: returns value between 0 and 1
    BEGIN
        v_float := meclaw.personality_fit('meclaw:agent:walter', 'meclaw:person:marcus-meyer', 'SQL query für die Datenbank');
        IF v_float >= 0.0 AND v_float <= 1.0 THEN
            v_pass := v_pass + 1;
        ELSE
            v_fail := v_fail + 1;
            v_results := array_append(v_results, 'FAIL: personality_fit returned ' || v_float || ' (expected 0-1)');
        END IF;
    EXCEPTION WHEN OTHERS THEN
        v_fail := v_fail + 1;
        v_results := array_append(v_results, 'FAIL: personality_fit threw: ' || SQLERRM);
    END;

    -- 3.8 llm_sentiment: positive
    BEGIN
        SELECT * FROM meclaw.llm_sentiment('Das ist perfekt, genau richtig!') INTO v_sentiment, v_reward;
        IF v_sentiment = 'positive' AND v_reward > 0 THEN
            v_pass := v_pass + 1;
        ELSE
            v_fail := v_fail + 1;
            v_results := array_append(v_results, 'FAIL: llm_sentiment(positive) = ' || v_sentiment || '/' || v_reward);
        END IF;
    EXCEPTION WHEN OTHERS THEN
        v_fail := v_fail + 1;
        v_results := array_append(v_results, 'FAIL: llm_sentiment(positive) threw: ' || SQLERRM);
    END;

    -- 3.9 llm_sentiment: negative
    BEGIN
        SELECT * FROM meclaw.llm_sentiment('Nein, das ist komplett falsch.') INTO v_sentiment, v_reward;
        IF v_sentiment = 'negative' AND v_reward < 0 THEN
            v_pass := v_pass + 1;
        ELSE
            v_fail := v_fail + 1;
            v_results := array_append(v_results, 'FAIL: llm_sentiment(negative) = ' || v_sentiment || '/' || v_reward);
        END IF;
    EXCEPTION WHEN OTHERS THEN
        v_fail := v_fail + 1;
        v_results := array_append(v_results, 'FAIL: llm_sentiment(negative) threw: ' || SQLERRM);
    END;

    -- 3.10 llm_sentiment: correction
    BEGIN
        SELECT * FROM meclaw.llm_sentiment('Hmm, nicht ganz, eher so...') INTO v_sentiment, v_reward;
        IF v_sentiment IN ('correction', 'negative') AND v_reward < 0 THEN
            v_pass := v_pass + 1;
        ELSE
            v_fail := v_fail + 1;
            v_results := array_append(v_results, 'FAIL: llm_sentiment(correction) = ' || v_sentiment || '/' || v_reward);
        END IF;
    EXCEPTION WHEN OTHERS THEN
        v_fail := v_fail + 1;
        v_results := array_append(v_results, 'FAIL: llm_sentiment(correction) threw: ' || SQLERRM);
    END;

    -- =========================================================================
    -- RESULT
    -- =========================================================================

    IF v_fail = 0 THEN
        RETURN format('✅ ALL PASS — %s/%s tests passed', v_pass, v_pass);
    ELSE
        RETURN format('❌ %s FAILED, %s passed:' || E'\n' || array_to_string(v_results, E'\n'), v_fail, v_pass);
    END IF;
END;
$fn$ LANGUAGE plpgsql;

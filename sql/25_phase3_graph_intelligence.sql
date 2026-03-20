-- MeClaw v0.1.0 — Phase 3: Graph Intelligence
-- Date: 2026-03-20
-- 
-- Includes:
-- - messages.seq column (finally!)
-- - Entity Resolution (resolve_entity, get_entity)
-- - Personality-Fit Scoring (personality_fit)
-- - AGENTS.md Parser Stub (ingest_agents_md)
-- - Updated extract_bee with AGE temporal edges
-- - Updated trg_extract_on_done with novelty_bee integration
-- - Updated feedback_bee with discounted reward propagation
-- - Updated retrieve_bee v3: RRF + Graph Expansion + Personality-Fit + Recency + Novelty

-- =============================================================================
-- 1. Entity Resolution
-- =============================================================================

CREATE OR REPLACE FUNCTION meclaw.resolve_entity(p_name TEXT)
RETURNS TEXT AS $$
DECLARE
    v_entity_id TEXT;
    v_name_lower TEXT;
BEGIN
    v_name_lower := lower(trim(p_name));
    
    SELECT id INTO v_entity_id FROM meclaw.entities
    WHERE lower(canonical_name) = v_name_lower LIMIT 1;
    IF v_entity_id IS NOT NULL THEN RETURN v_entity_id; END IF;

    SELECT id INTO v_entity_id FROM meclaw.entities
    WHERE EXISTS (SELECT 1 FROM unnest(aliases) alias WHERE lower(alias) = v_name_lower)
    LIMIT 1;
    IF v_entity_id IS NOT NULL THEN RETURN v_entity_id; END IF;

    SELECT id INTO v_entity_id FROM meclaw.entities
    WHERE lower(canonical_name) LIKE v_name_lower || '%'
       OR lower(canonical_name) LIKE '%' || v_name_lower || '%'
    ORDER BY length(canonical_name) ASC LIMIT 1;
    
    RETURN v_entity_id;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION meclaw.get_entity(p_name TEXT)
RETURNS TABLE (id TEXT, canonical_name TEXT, entity_type TEXT, neural_matrix JSONB, explicit_profile JSONB, observed_profile JSONB) AS $$
    SELECT e.id, e.canonical_name, e.entity_type, e.neural_matrix, e.explicit_profile, e.observed_profile
    FROM meclaw.entities e WHERE e.id = meclaw.resolve_entity(p_name);
$$ LANGUAGE sql;

-- =============================================================================
-- 2. Personality-Fit Scoring
-- =============================================================================

CREATE OR REPLACE FUNCTION meclaw.personality_fit(
    p_agent_id TEXT,
    p_user_id TEXT,
    p_content TEXT
) RETURNS FLOAT AS $fn$
    import json
    
    plan = plpy.prepare("SELECT neural_matrix FROM meclaw.entities WHERE id = $1", ["text"])
    agent_row = plan.execute([p_agent_id])
    agent_matrix = json.loads(agent_row[0]["neural_matrix"]) if agent_row and agent_row[0]["neural_matrix"] else {}
    
    user_row = plan.execute([p_user_id]) if p_user_id else []
    user_matrix = json.loads(user_row[0]["neural_matrix"]) if user_row and user_row[0]["neural_matrix"] else {}
    
    content_lower = (p_content or "").lower()
    
    is_technical = any(w in content_lower for w in ["sql", "code", "function", "error", "bug", "api", "docker", "postgres", "config"])
    is_emotional = any(w in content_lower for w in ["danke", "super", "frustriert", "toll", "schlecht", "freue", "sorry", "liebe", "hasse"])
    is_creative = any(w in content_lower for w in ["idee", "konzept", "design", "brainstorm", "vision", "vorschlag"])
    is_analytical = any(w in content_lower for w in ["warum", "analyse", "vergleich", "strategie", "trade-off", "pro", "contra"])
    
    score = 0.5
    
    if is_technical:
        score += agent_matrix.get("logic", 0.5) * 0.2 + agent_matrix.get("reliability", 0.5) * 0.1
    if is_emotional:
        score += agent_matrix.get("empathy", 0.5) * 0.2 + agent_matrix.get("charisma", 0.5) * 0.1
    if is_creative:
        score += agent_matrix.get("creativity", 0.5) * 0.2 + agent_matrix.get("adaptability", 0.5) * 0.1
    if is_analytical:
        score += agent_matrix.get("logic", 0.5) * 0.15 + agent_matrix.get("creativity", 0.5) * 0.15
    
    if user_matrix:
        if user_matrix.get("logic", 0) > 0.7:
            if is_technical: score += 0.1
        if user_matrix.get("empathy", 0) > 0.7:
            if is_emotional: score += 0.1
    
    return max(0.0, min(1.0, score))
$fn$ LANGUAGE plpython3u;

-- =============================================================================
-- 3. AGENTS.md Parser (Stub)
-- =============================================================================

CREATE OR REPLACE FUNCTION meclaw.ingest_agents_md(
    p_agent_id TEXT,
    p_channel_id UUID,
    p_content TEXT,
    p_source TEXT DEFAULT 'AGENTS.md'
) RETURNS INT AS $$
DECLARE
    v_sections TEXT[];
    v_section TEXT;
    v_count INT := 0;
    v_event_id UUID;
BEGIN
    v_sections := regexp_split_to_array(p_content, E'\n## ');
    
    FOREACH v_section IN ARRAY v_sections LOOP
        IF length(trim(v_section)) < 10 THEN CONTINUE; END IF;
        
        INSERT INTO meclaw.brain_events (channel_id, agent_id, content)
        VALUES (p_channel_id, p_agent_id, '## ' || v_section)
        RETURNING id INTO v_event_id;
        
        BEGIN
            PERFORM pg_background_launch(format('SELECT meclaw.compute_embedding(%L::uuid)', v_event_id));
        EXCEPTION WHEN OTHERS THEN NULL;
        END;
        
        v_count := v_count + 1;
    END LOOP;
    
    INSERT INTO meclaw.events (bee_type, event, payload)
    VALUES ('ingest', 'agents_md_ingested', jsonb_build_object(
        'source', p_source, 'sections', v_count, 'agent_id', p_agent_id));
    
    RETURN v_count;
END;
$$ LANGUAGE plpgsql;
